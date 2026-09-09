//! Resolve a spec's named declaration independently of its recorded signature.

use crate::spec::{ParsedSpec, SymbolClaim, TouchEntry};
use crate::store::{Store, Symbol};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy)]
pub(crate) struct Scope<'a> {
    pub name: &'a str,
    pub file: Option<&'a str>,
    pub language: Option<&'a str>,
}

#[derive(Debug)]
pub(crate) struct Resolved {
    pub symbol: Symbol,
    pub language: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Unresolved {
    pub reason: &'static str,
    pub matches: Option<usize>,
}

impl Unresolved {
    pub(crate) fn unavailable(reason: &'static str) -> Self {
        Self {
            reason,
            matches: None,
        }
    }
}

pub(crate) fn components(name: &str) -> Result<Vec<&str>, Unresolved> {
    let parts: Vec<_> = name.split("::").flat_map(|part| part.split('.')).collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || part.contains(':') || part.trim() != *part)
    {
        return Err(Unresolved::unavailable("invalid_qualification"));
    }
    Ok(parts)
}

pub(crate) fn normalize_file(file: &str) -> Result<String, Unresolved> {
    let file = file.replace('\\', "/");
    let file = file.trim_start_matches("./");
    if file.is_empty()
        || file.contains('\0')
        || file.split('/').any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(Unresolved::unavailable("invalid_file_scope"));
    }
    Ok(file.to_string())
}

pub(crate) fn touch_scopes(touch: &TouchEntry) -> impl Iterator<Item = Scope<'_>> {
    touch.symbols.iter().map(|symbol| Scope {
        name: symbol.name(),
        file: Some(symbol.file().unwrap_or(&touch.file)),
        language: symbol.language().or(touch.language.as_deref()),
    })
}

/// An explicit touch can constrain the matching snapshot bullet. Different
/// qualified names never lend one another their file or language context.
pub(crate) fn snapshot_scopes<'a>(spec: &'a ParsedSpec, claim: &'a SymbolClaim) -> Vec<Scope<'a>> {
    let Ok(declared) = components(&claim.name) else {
        return Vec::new();
    };
    spec.frontmatter
        .iter()
        .flat_map(|frontmatter| &frontmatter.touches)
        .flat_map(touch_scopes)
        .filter(|scope| {
            components(scope.name).is_ok_and(|parts| {
                parts == declared
                    || (parts.last() == declared.last()
                        && (parts.len() == 1 || declared.len() == 1))
            })
        })
        .collect()
}

pub(crate) fn resolve_snapshot(
    store: &Store,
    spec: &ParsedSpec,
    claim: &SymbolClaim,
) -> Result<Resolved, Unresolved> {
    resolve(store, &claim.name, &snapshot_scopes(spec, claim))
}

pub(crate) fn resolve(
    store: &Store,
    name: &str,
    scopes: &[Scope<'_>],
) -> Result<Resolved, Unresolved> {
    resolve_with_hook(store, name, scopes, || {})
}

fn resolve_with_hook(
    store: &Store,
    name: &str,
    scopes: &[Scope<'_>],
    after_lookup: impl FnOnce(),
) -> Result<Resolved, Unresolved> {
    let before = store
        .data_version()
        .map_err(|_| Unresolved::unavailable("index_version_unavailable"))?;
    let result = resolve_candidates(store, name, scopes, after_lookup);
    let after = store
        .data_version()
        .map_err(|_| Unresolved::unavailable("index_version_unavailable"))?;
    if before != after {
        return Err(Unresolved::unavailable("index_changed"));
    }
    result
}

fn resolve_candidates(
    store: &Store,
    name: &str,
    scopes: &[Scope<'_>],
    after_lookup: impl FnOnce(),
) -> Result<Resolved, Unresolved> {
    let declared = components(name)?;
    let leaf = declared.last().expect("split always has a component");
    let unscoped = [Scope {
        name,
        file: None,
        language: None,
    }];
    let scopes = if scopes.is_empty() { &unscoped } else { scopes };
    let mut files: HashMap<String, HashMap<i64, Symbol>> = HashMap::new();
    let mut matches: BTreeMap<i64, Resolved> = BTreeMap::new();
    let mut after_lookup = Some(after_lookup);
    for scope in scopes {
        let scoped_name = components(scope.name)?;
        let scoped_file = scope.file.map(normalize_file).transpose()?;
        if scoped_name.last() != Some(leaf) {
            continue;
        }
        let candidates = store
            .spec_symbol_candidates(leaf, scope.language)
            .map_err(|_| Unresolved::unavailable("symbol_query_failed"))?;
        if let Some(hook) = after_lookup.take() {
            hook();
        }
        for symbol in candidates {
            // An impl is a lexical container for methods, not another named
            // definition of its target type. Keep it in the ancestry cache.
            if matches!(symbol.kind.as_str(), "module" | "impl")
                || scoped_file
                    .as_ref()
                    .is_some_and(|file| symbol.file_path != *file)
            {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(entry) =
                files.entry(symbol.file_path.clone())
            {
                let symbols = store
                    .symbols_in_file(&symbol.file_path)
                    .map_err(|_| Unresolved::unavailable("parent_query_failed"))?;
                entry.insert(symbols.into_iter().map(|s| (s.id, s)).collect());
            }
            let chain = ancestry(&symbol, &files[&symbol.file_path])?;
            if !chain.ends_with(&declared) || !chain.ends_with(&scoped_name) {
                continue;
            }
            let language = scope.language.map(str::to_string);
            matches
                .entry(symbol.id)
                .and_modify(|resolved| {
                    if resolved.language != language {
                        resolved.language = None;
                    }
                })
                .or_insert(Resolved { symbol, language });
        }
    }
    match matches.len() {
        0 => Err(Unresolved {
            reason: "missing",
            matches: Some(0),
        }),
        1 => Ok(matches.into_values().next().expect("one match")),
        count => Err(Unresolved {
            reason: "ambiguous",
            matches: Some(count),
        }),
    }
}

fn ancestry<'a>(
    symbol: &'a Symbol,
    file: &'a HashMap<i64, Symbol>,
) -> Result<Vec<&'a str>, Unresolved> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(symbol);
    while let Some(symbol) = current {
        if !seen.insert(symbol.id) {
            return Err(Unresolved::unavailable("parent_cycle"));
        }
        if symbol.kind != "module" {
            // C# namespaces may be one App.Sub node or nested App -> Sub
            // nodes. Both describe the same lexical qualification.
            let parts = components(&symbol.name)?;
            chain.extend(parts.into_iter().rev());
        }
        current = match symbol.parent_id {
            Some(parent) => Some(
                file.get(&parent)
                    .ok_or_else(|| Unresolved::unavailable("parent_missing_from_file"))?,
            ),
            None => None,
        };
    }
    chain.reverse();
    Ok(chain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::{extractor_for_path, parse_blob};
    use std::path::Path;

    fn add_source(store: &mut Store, path: &str, source: &str) {
        let extractor = extractor_for_path(Path::new(path)).unwrap();
        let file = parse_blob(path, source.as_bytes(), 0, extractor.as_ref()).unwrap();
        store.commit_file(file).unwrap();
    }

    #[test]
    fn qualified_claims_keep_parent_file_and_language_scope() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("graph.db")).unwrap();
        add_source(
            &mut store,
            "service.py",
            "class A:\n    def run(self):\n        return 1\n\nclass B:\n    def run(self, flag=False):\n        return 2\n",
        );
        let selected = resolve(&store, "B.run", &[]).unwrap();
        assert_eq!(selected.symbol.line_start, 6);
        assert_eq!(
            selected.symbol.signature.as_deref(),
            Some("def run(self, flag=False)")
        );
        assert_eq!(
            resolve(&store, "B::run", &[]).unwrap().symbol.id,
            selected.symbol.id
        );
        let ambiguous = resolve(&store, "run", &[]).unwrap_err();
        assert_eq!(ambiguous.reason, "ambiguous");
        assert_eq!(ambiguous.matches, Some(2));
        assert_eq!(
            resolve(&store, "Missing.run", &[]).unwrap_err().reason,
            "missing"
        );
        assert_eq!(
            resolve(&store, "B:::run", &[]).unwrap_err().reason,
            "invalid_qualification"
        );
        add_source(
            &mut store,
            "other.py",
            "class B:\n    def run(self):\n        return 3\n",
        );
        assert_eq!(resolve(&store, "B.run", &[]).unwrap_err().matches, Some(2));
        let scope = Scope {
            name: "B.run",
            file: Some("service.py"),
            language: Some("python"),
        };
        assert_eq!(
            resolve(&store, "B.run", &[scope])
                .unwrap()
                .symbol
                .line_start,
            6
        );
        assert_eq!(
            resolve(
                &store,
                "B.run",
                &[Scope {
                    language: Some("rust"),
                    ..scope
                }]
            )
            .unwrap_err()
            .reason,
            "missing"
        );
        assert_eq!(
            resolve(
                &store,
                "B.run",
                &[Scope {
                    file: Some("absent.py"),
                    ..scope
                }]
            )
            .unwrap_err()
            .reason,
            "missing"
        );
    }

    #[test]
    fn rust_impl_and_csharp_namespace_scopes_resolve_without_collapsing_overloads() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("graph.db")).unwrap();
        add_source(
            &mut store,
            "lib.rs",
            "struct B;\nimpl B { fn own(&self) {} }\ntrait A { fn run(&self) {} }\ntrait C { fn run(&self) {} }\nimpl A for B { fn run(&self) {} }\nimpl C for B { fn run(&self) {} }\n",
        );
        assert_eq!(resolve(&store, "B", &[]).unwrap().symbol.kind, "struct");
        assert_eq!(resolve(&store, "B::own", &[]).unwrap().symbol.name, "own");
        let ambiguous = resolve(&store, "B::run", &[]).unwrap_err();
        assert_eq!(ambiguous.reason, "ambiguous");
        assert_eq!(ambiguous.matches, Some(2));
        for (file, source) in [
            (
                "qualified.cs",
                "namespace App.Sub { class Worker { public void Run() {} } }",
            ),
            (
                "nested.cs",
                "namespace App { namespace Sub { class Worker { public void Run() {} } } }",
            ),
            (
                "file_scoped.cs",
                "namespace App.Sub;\nclass Worker { public void Run() {} }\n",
            ),
        ] {
            add_source(&mut store, file, source);
            let scope = Scope {
                name: "App.Sub.Worker.Run",
                file: Some(file),
                language: Some("csharp"),
            };
            let resolved = resolve(&store, "App::Sub::Worker::Run", &[scope]).unwrap();
            assert_eq!(resolved.symbol.file_path, file);
            let namespace = Scope {
                name: "App.Sub",
                ..scope
            };
            assert_eq!(
                resolve(&store, "App.Sub", &[namespace])
                    .unwrap()
                    .symbol
                    .kind,
                "namespace"
            );
        }
        assert_eq!(
            resolve(&store, "App.Sub.Worker.Run", &[])
                .unwrap_err()
                .matches,
            Some(3)
        );
        assert_eq!(resolve(&store, "Sub", &[]).unwrap_err().matches, Some(3));
        add_source(
            &mut store,
            "outer.cs",
            "namespace Outer { namespace App.Sub { class Worker {} } }",
        );
        assert_eq!(
            resolve(&store, "Outer.App.Sub", &[]).unwrap().symbol.name,
            "App.Sub"
        );
    }

    #[test]
    fn concurrent_file_replacement_cannot_verify_an_old_candidate() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("graph.db");
        let mut store = Store::open(&db).unwrap();
        add_source(&mut store, "service.py", "def run():\n    return 1\n");
        let mut writer = Store::open(&db).unwrap();
        let error = resolve_with_hook(&store, "run", &[], || {
            add_source(
                &mut writer,
                "service.py",
                "def run(flag=False):\n    return 2\n",
            );
        })
        .unwrap_err();
        assert_eq!(error.reason, "index_changed");
        assert_eq!(
            resolve(&store, "run", &[])
                .unwrap()
                .symbol
                .signature
                .as_deref(),
            Some("def run(flag=False)")
        );
    }

    #[test]
    fn invalid_parent_graphs_and_query_errors_are_unresolved() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("graph.db");
        let mut store = Store::open(&db).unwrap();
        add_source(
            &mut store,
            "service.py",
            "class B:\n    def run(self):\n        pass\n",
        );
        let raw = rusqlite::Connection::open(&db).unwrap();
        raw.execute_batch(
            "PRAGMA foreign_keys = OFF; UPDATE symbols SET parent_id = id WHERE name = 'run'",
        )
        .unwrap();
        assert_eq!(
            resolve(&store, "B.run", &[]).unwrap_err().reason,
            "parent_cycle"
        );
        raw.execute_batch("UPDATE symbols SET parent_id = 999999 WHERE name = 'run'")
            .unwrap();
        assert_eq!(
            resolve(&store, "B.run", &[]).unwrap_err().reason,
            "parent_missing_from_file"
        );
        raw.execute_batch("DROP TABLE symbols").unwrap();
        let error = resolve(&store, "B.run", &[]).unwrap_err();
        assert_eq!(error.reason, "symbol_query_failed");
        assert_eq!(error.matches, None);
    }
}
