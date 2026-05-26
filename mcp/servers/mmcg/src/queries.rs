//! High-level query layer over the Store.
//!
//! Wraps the raw store methods with name-based lookup, structured response
//! types, and JSON serialization for the MCP layer.

use crate::store::{FileEntry, Store, Symbol, TaskSpecHit};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SymbolHit {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Extra locations when this hit collapses several declarations of the same
    /// symbol (e.g. C# partial classes split across files). The primary `file`/`line`
    /// fields still point to the canonical (lex-first) declaration; this list
    /// includes every declaration including the canonical one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<SymbolLocation>>,
    /// Decorators / attributes / modifiers captured from source
    /// (e.g. `",Fact,"`, `",partial,sealed,"`). Skipped from output when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decorators: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolLocation {
    pub file: String,
    pub line: u32,
}

impl From<Symbol> for SymbolHit {
    fn from(s: Symbol) -> Self {
        Self {
            name: s.name,
            kind: s.kind,
            file: s.file_path,
            line: s.line_start,
            signature: s.signature,
            locations: None,
            decorators: s.decorators,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SymbolHit>,
}

#[derive(Debug, Serialize)]
pub struct CallersResponse {
    pub target: String,
    pub count: u32,
    pub callers: Vec<SymbolHit>,
}

#[derive(Debug, Serialize)]
pub struct CalleesEntry {
    pub name: String,
    pub line: u32,
}

#[derive(Debug, Serialize)]
pub struct CalleesResponse {
    pub symbol: String,
    pub matched: Option<SymbolHit>,
    pub count: u32,
    pub callees: Vec<CalleesEntry>,
}

#[derive(Debug, Serialize)]
pub struct ImpactEntry {
    pub symbol: SymbolHit,
    pub depth: u32,
}

#[derive(Debug, Serialize)]
pub struct ImpactResponse {
    pub target: String,
    pub max_depth: u32,
    pub count: u32,
    pub impact: Vec<ImpactEntry>,
}

#[derive(Debug, Serialize)]
pub struct FilesResponse {
    pub prefix: Option<String>,
    pub count: u32,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub db_path: String,
    pub symbol_count: u32,
    pub file_count: u32,
}

pub fn search(
    store: &Store,
    name: &str,
    kind: Option<&str>,
    language: Option<&str>,
    collapse_partials: bool,
) -> rusqlite::Result<SearchResponse> {
    let raw = store.search_symbols(name, kind, language)?;
    let results = if collapse_partials {
        collapse_partial_hits(raw)
    } else {
        raw.into_iter().map(SymbolHit::from).collect()
    };
    Ok(SearchResponse {
        query: name.to_string(),
        results,
    })
}

/// Collapse multiple Symbol rows for the same partial-class declaration into a
/// single hit. A row is considered "partial" when its decorators field contains
/// `,partial,` (set by the C# extractor for `partial class` / `partial record`).
///
/// Rows that are NOT partial pass through unchanged — even if multiple rows
/// share a name. Two non-partial classes with the same name (unusual but
/// possible across namespaces) deserve to be reported as distinct hits.
///
/// The canonical hit is the lex-first by file path; its `locations` field lists
/// every declaration (including itself).
fn collapse_partial_hits(symbols: Vec<Symbol>) -> Vec<SymbolHit> {
    use std::collections::HashMap;

    // Group key: (name, kind). Language not in the key — partials are C#-only
    // and our SQL filters by language upstream.
    let mut groups: HashMap<(String, String), Vec<Symbol>> = HashMap::new();
    let mut order: Vec<(String, String)> = Vec::new();
    let mut passthrough: Vec<SymbolHit> = Vec::new();

    for sym in symbols {
        let is_partial = sym
            .decorators
            .as_deref()
            .map(|d| d.contains(",partial,"))
            .unwrap_or(false);
        if !is_partial {
            passthrough.push(SymbolHit::from(sym));
            continue;
        }
        let key = (sym.name.clone(), sym.kind.clone());
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(sym);
    }

    let mut out: Vec<SymbolHit> = Vec::with_capacity(passthrough.len() + order.len());
    out.extend(passthrough);
    for key in order {
        let mut rows = groups.remove(&key).unwrap();
        rows.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then(a.line_start.cmp(&b.line_start))
        });
        let canonical = rows[0].clone();
        let locations: Vec<SymbolLocation> = rows
            .iter()
            .map(|s| SymbolLocation {
                file: s.file_path.clone(),
                line: s.line_start,
            })
            .collect();
        let mut hit = SymbolHit::from(canonical);
        hit.locations = Some(locations);
        out.push(hit);
    }
    out
}

#[derive(Debug, Serialize)]
pub struct CentralityHit {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub in_degree: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decorators: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CentralityResponse {
    pub count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    pub top: u32,
    pub results: Vec<CentralityHit>,
}

#[derive(Debug, Serialize)]
pub struct TaskSearchResponse {
    pub query: String,
    pub count: u32,
    pub results: Vec<TaskSpecHit>,
}

pub fn tasks(store: &Store, query: &str, top: u32) -> rusqlite::Result<TaskSearchResponse> {
    let results = store.search_task_specs(query, top)?;
    Ok(TaskSearchResponse {
        query: query.to_string(),
        count: results.len() as u32,
        results,
    })
}

#[derive(Debug, Serialize)]
pub struct DependencyCyclesResponse {
    pub count: u32,
    pub min_size: u32,
    /// Each entry is one cycle (SCC) — file paths in lexicographic order.
    pub cycles: Vec<Vec<String>>,
}

pub fn symbols_changed_since(
    store: &Store,
    repo_root: &std::path::Path,
    git_ref: &str,
) -> Result<crate::diff::SymbolDiff, crate::diff::DiffError> {
    crate::diff::symbols_changed_since(store, repo_root, git_ref)
}

pub fn dependency_cycles(
    store: &Store,
    language: Option<&str>,
    min_size: u32,
) -> rusqlite::Result<DependencyCyclesResponse> {
    let cycles = store.dependency_cycles(language, min_size as usize)?;
    Ok(DependencyCyclesResponse {
        count: cycles.len() as u32,
        min_size,
        cycles,
    })
}

pub fn centrality(
    store: &Store,
    prefix: Option<&str>,
    language: Option<&str>,
    kind: Option<&str>,
    top: u32,
) -> rusqlite::Result<CentralityResponse> {
    let raw = store.centrality(prefix, language, kind, top)?;
    let results: Vec<CentralityHit> = raw
        .into_iter()
        .map(|(s, in_degree)| CentralityHit {
            name: s.name,
            kind: s.kind,
            file: s.file_path,
            line: s.line_start,
            in_degree,
            signature: s.signature,
            decorators: s.decorators,
        })
        .collect();
    Ok(CentralityResponse {
        count: results.len() as u32,
        prefix: prefix.map(String::from),
        top,
        results,
    })
}

pub fn callers(
    store: &Store,
    name: &str,
    language: Option<&str>,
    edge_kind: Option<&str>,
) -> rusqlite::Result<CallersResponse> {
    let callers: Vec<SymbolHit> = store
        .callers_of(name, language, edge_kind)?
        .into_iter()
        .map(SymbolHit::from)
        .collect();
    Ok(CallersResponse {
        target: name.to_string(),
        count: callers.len() as u32,
        callers,
    })
}

pub fn callees(
    store: &Store,
    name: &str,
    language: Option<&str>,
    edge_kind: Option<&str>,
) -> rusqlite::Result<CalleesResponse> {
    let matched = store
        .search_symbols(name, None, language)?
        .into_iter()
        .next();
    let callees: Vec<CalleesEntry> = if let Some(ref sym) = matched {
        store
            .callees_of(sym.id, edge_kind)?
            .into_iter()
            .map(|(n, l)| CalleesEntry { name: n, line: l })
            .collect()
    } else {
        Vec::new()
    };
    Ok(CalleesResponse {
        symbol: name.to_string(),
        matched: matched.map(SymbolHit::from),
        count: callees.len() as u32,
        callees,
    })
}

pub fn impact(
    store: &Store,
    name: &str,
    max_depth: u32,
    language: Option<&str>,
) -> rusqlite::Result<ImpactResponse> {
    let depth = max_depth.clamp(1, 10);
    let impact: Vec<ImpactEntry> = store
        .impact_of(name, depth, language)?
        .into_iter()
        .map(|(s, d)| ImpactEntry {
            symbol: SymbolHit::from(s),
            depth: d,
        })
        .collect();
    Ok(ImpactResponse {
        target: name.to_string(),
        max_depth: depth,
        count: impact.len() as u32,
        impact,
    })
}

pub fn files(
    store: &Store,
    prefix: Option<&str>,
    language: Option<&str>,
) -> rusqlite::Result<FilesResponse> {
    // SQL LIKE pattern — match anything beginning with prefix
    let pattern = prefix.map(|p| {
        if p.ends_with('%') {
            p.to_string()
        } else {
            format!("{p}%")
        }
    });
    let files = store.files_under(pattern.as_deref(), language)?;
    Ok(FilesResponse {
        prefix: prefix.map(String::from),
        count: files.len() as u32,
        files,
    })
}

/// Parse a duration like "30s" / "10m" / "2h" / "1d" into seconds.
/// Errors on missing suffix, unknown suffix, or non-numeric prefix.
pub fn parse_duration(s: &str) -> Result<u64, String> {
    if s.len() < 2 {
        return Err(format!("duration too short: {s:?}"));
    }
    let (num_part, suffix) = s.split_at(s.len() - 1);
    let n: u64 = num_part
        .parse()
        .map_err(|_| format!("duration prefix not a non-negative integer: {num_part:?}"))?;
    let multiplier: u64 = match suffix {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        other => {
            return Err(format!(
                "unknown duration suffix {other:?}; expected s/m/h/d"
            ))
        }
    };
    Ok(n * multiplier)
}

#[derive(Debug, Serialize)]
pub struct RecentChangesResponse {
    pub since: String,
    pub window_secs: u64,
    pub count: u32,
    pub files: Vec<FileEntry>,
}

/// Files re-indexed within the last `since` window (e.g. "2h").
/// Useful for incident-response Phase 3 ("what's been touched recently?") and
/// debugging stale-index symptoms.
///
/// Note: `indexed_at` is stored in **milliseconds** by the indexer (see
/// `indexer.rs` — `as_millis() as i64`), so the threshold is computed in ms too.
pub fn recent_changes(store: &Store, since: &str) -> Result<RecentChangesResponse, String> {
    let window_secs = parse_duration(since)?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis() as i64;
    let threshold_ms = now_ms - (window_secs as i64) * 1000;
    let files = store
        .files_indexed_since(threshold_ms)
        .map_err(|e| e.to_string())?;
    Ok(RecentChangesResponse {
        since: since.to_string(),
        window_secs,
        count: files.len() as u32,
        files,
    })
}

#[derive(Debug, Serialize)]
pub struct UnreferencedResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub count: u32,
    pub symbols: Vec<SymbolHit>,
}

/// Symbols that nothing references. See `Store::unreferenced` for false-positive caveats.
pub fn unreferenced(
    store: &Store,
    kind: Option<&str>,
    language: Option<&str>,
) -> rusqlite::Result<UnreferencedResponse> {
    let syms: Vec<SymbolHit> = store
        .unreferenced(kind, language)?
        .into_iter()
        .map(SymbolHit::from)
        .collect();
    Ok(UnreferencedResponse {
        kind: kind.map(String::from),
        language: language.map(String::from),
        count: syms.len() as u32,
        symbols: syms,
    })
}

#[derive(Debug, Serialize)]
pub struct ApiSurfaceResponse {
    pub prefix: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub count: u32,
    pub symbols: Vec<SymbolHit>,
}

/// Symbols under `prefix` that are referenced from outside `prefix`.
pub fn api_surface(
    store: &Store,
    prefix: &str,
    language: Option<&str>,
) -> rusqlite::Result<ApiSurfaceResponse> {
    let syms: Vec<SymbolHit> = store
        .api_surface(prefix, language)?
        .into_iter()
        .map(SymbolHit::from)
        .collect();
    Ok(ApiSurfaceResponse {
        prefix: prefix.to_string(),
        language: language.map(String::from),
        count: syms.len() as u32,
        symbols: syms,
    })
}

pub fn status(store: &Store) -> rusqlite::Result<StatusResponse> {
    Ok(StatusResponse {
        db_path: store.db_path().to_string_lossy().to_string(),
        symbol_count: store.symbol_count()?,
        file_count: store.file_count()?,
    })
}

#[derive(Debug, Serialize)]
pub struct ImportEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub line: u32,
}

#[derive(Debug, Serialize)]
pub struct ImportsResponse {
    pub file: String,
    pub count: u32,
    pub imports: Vec<ImportEntry>,
}

#[derive(Debug, Serialize)]
pub struct ImportedByResponse {
    pub name: String,
    pub count: u32,
    pub files: Vec<String>,
}

pub fn imports(store: &Store, file: &str) -> rusqlite::Result<ImportsResponse> {
    let triples = store.imports_of(file)?;
    let imports: Vec<ImportEntry> = triples
        .into_iter()
        .map(|(name, path, line)| ImportEntry { name, path, line })
        .collect();
    Ok(ImportsResponse {
        file: file.to_string(),
        count: imports.len() as u32,
        imports,
    })
}

/// `match_kind`: "name" (default) matches the leaf binding;
/// "path" matches the fully-qualified import path exactly.
#[derive(Debug, Serialize)]
pub struct SymbolsInFileResponse {
    pub file: String,
    pub count: u32,
    pub symbols: Vec<SymbolHit>,
}

pub fn symbols_in_file(store: &Store, file: &str) -> rusqlite::Result<SymbolsInFileResponse> {
    let syms: Vec<SymbolHit> = store
        .symbols_in_file(file)?
        .into_iter()
        .map(SymbolHit::from)
        .collect();
    Ok(SymbolsInFileResponse {
        file: file.to_string(),
        count: syms.len() as u32,
        symbols: syms,
    })
}

#[derive(Debug, Serialize)]
pub struct OutlineNode {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub line_start: u32,
    pub line_end: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<OutlineNode>,
}

#[derive(Debug, Serialize)]
pub struct OutlineResponse {
    pub file: String,
    pub count: u32,
    pub nodes: Vec<OutlineNode>,
}

/// Build a tree of symbols in a file using `parent_id` chains.
/// Returns top-level nodes (parent_id IS NULL); each node contains its children
/// sorted by line. Single SELECT, in-memory tree construction.
pub fn outline(store: &Store, file: &str) -> rusqlite::Result<OutlineResponse> {
    use std::collections::HashMap;
    let flat = store.symbols_in_file(file)?;
    let total = flat.len() as u32;

    // Build child lists keyed by parent id (None = root).
    let mut children_of: HashMap<Option<i64>, Vec<crate::store::Symbol>> = HashMap::new();
    for sym in flat {
        children_of.entry(sym.parent_id).or_default().push(sym);
    }

    fn build(
        parent_id: Option<i64>,
        children_of: &mut HashMap<Option<i64>, Vec<crate::store::Symbol>>,
    ) -> Vec<OutlineNode> {
        let mut nodes = children_of.remove(&parent_id).unwrap_or_default();
        nodes.sort_by_key(|s| s.line_start);
        nodes
            .into_iter()
            .map(|s| OutlineNode {
                id: s.id,
                name: s.name,
                kind: s.kind,
                line_start: s.line_start,
                line_end: s.line_end,
                signature: s.signature,
                children: build(Some(s.id), children_of),
            })
            .collect()
    }

    let nodes = build(None, &mut children_of);
    Ok(OutlineResponse {
        file: file.to_string(),
        count: total,
        nodes,
    })
}

/// `match_kind`: "name" (default) matches the leaf binding;
/// "path" matches the fully-qualified import path exactly.
/// `language` scopes to a single language (defends against monorepo name collisions).
pub fn imported_by(
    store: &Store,
    query: &str,
    match_kind: &str,
    language: Option<&str>,
) -> rusqlite::Result<ImportedByResponse> {
    let files = match match_kind {
        "path" => store.imported_by_path(query, language)?,
        _ => store.imported_by_name(query, language)?,
    };
    Ok(ImportedByResponse {
        name: query.to_string(),
        count: files.len() as u32,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::path::PathBuf;

    fn tmp_db(name: &str) -> PathBuf {
        let mut p = env::temp_dir();
        p.push(format!("mmcg-queries-{}-{}.db", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn parse_duration_units() {
        assert_eq!(parse_duration("30s").unwrap(), 30);
        assert_eq!(parse_duration("10m").unwrap(), 600);
        assert_eq!(parse_duration("2h").unwrap(), 7200);
        assert_eq!(parse_duration("1d").unwrap(), 86400);
        assert_eq!(parse_duration("0s").unwrap(), 0);
        assert!(parse_duration("").is_err());
        assert!(parse_duration("h").is_err()); // too short to have a number
        assert!(parse_duration("5y").is_err()); // unknown suffix
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn recent_changes_filters() {
        let path = tmp_db("recent_changes_filters");
        let store = Store::open(&path).unwrap();
        // indexer stores indexed_at in milliseconds — match its convention here
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        // file_a touched 30s ago, file_b touched 2h ago (all in ms)
        store.upsert_file("file_a.py", now_ms - 30_000, 5).unwrap();
        store
            .upsert_file("file_b.py", now_ms - 7_200_000, 3)
            .unwrap();

        // "1h" window catches only file_a
        let recent = recent_changes(&store, "1h").unwrap();
        assert_eq!(recent.count, 1);
        assert_eq!(recent.files[0].path, "file_a.py");

        // "3h" window catches both
        let wider = recent_changes(&store, "3h").unwrap();
        assert_eq!(wider.count, 2);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn outline_tree() {
        let path = tmp_db("outline_tree");
        let store = Store::open(&path).unwrap();
        // Class Foo at line 1, with method bar at line 5 and baz at line 10.
        let foo = store
            .insert_symbol("Foo", "class", "x.py", 1, 15, None, None)
            .unwrap();
        let _bar = store
            .insert_symbol(
                "bar",
                "method",
                "x.py",
                5,
                7,
                Some("def bar(self)"),
                Some(foo),
            )
            .unwrap();
        let _baz = store
            .insert_symbol(
                "baz",
                "method",
                "x.py",
                10,
                12,
                Some("def baz(self)"),
                Some(foo),
            )
            .unwrap();
        // Sibling top-level function at line 20.
        let _helper = store
            .insert_symbol("helper", "function", "x.py", 20, 22, None, None)
            .unwrap();

        let out = outline(&store, "x.py").unwrap();
        assert_eq!(out.count, 4);
        assert_eq!(out.nodes.len(), 2); // Foo + helper

        // Nodes ordered by line_start
        assert_eq!(out.nodes[0].name, "Foo");
        assert_eq!(out.nodes[0].children.len(), 2);
        assert_eq!(out.nodes[0].children[0].name, "bar");
        assert_eq!(out.nodes[0].children[1].name, "baz");

        assert_eq!(out.nodes[1].name, "helper");
        assert!(out.nodes[1].children.is_empty());

        std::fs::remove_file(&path).ok();
    }

    fn mk_sym(name: &str, kind: &str, file: &str, line: u32, decorators: Option<&str>) -> Symbol {
        Symbol {
            id: 0,
            name: name.to_string(),
            kind: kind.to_string(),
            file_path: file.to_string(),
            line_start: line,
            line_end: line,
            signature: None,
            parent_id: None,
            decorators: decorators.map(String::from),
        }
    }

    #[test]
    fn collapse_partials_groups_only_partial_rows() {
        let symbols = vec![
            mk_sym("User", "class", "User.B.cs", 3, Some(",partial,")),
            mk_sym("User", "class", "User.A.cs", 3, Some(",partial,")),
            mk_sym("User", "class", "User.C.cs", 3, Some(",partial,")),
            mk_sym("Service", "class", "Service.cs", 2, None),
        ];
        let hits = collapse_partial_hits(symbols);
        // 1 partial group (User) + 1 passthrough (Service) = 2
        assert_eq!(hits.len(), 2);

        let user = hits.iter().find(|h| h.name == "User").unwrap();
        // Canonical = lex-first file
        assert_eq!(user.file, "User.A.cs");
        let locs = user.locations.as_ref().expect("partial has locations");
        assert_eq!(locs.len(), 3);
        assert_eq!(locs[0].file, "User.A.cs");
        assert_eq!(locs[1].file, "User.B.cs");
        assert_eq!(locs[2].file, "User.C.cs");

        let service = hits.iter().find(|h| h.name == "Service").unwrap();
        assert!(service.locations.is_none());
    }

    #[test]
    fn collapse_partials_passes_non_partial_duplicates_unchanged() {
        // Two distinct non-partial classes named `Foo` in different namespaces —
        // these are NOT a partial collapse target and must remain separate hits.
        let symbols = vec![
            mk_sym("Foo", "class", "A/Foo.cs", 1, None),
            mk_sym("Foo", "class", "B/Foo.cs", 1, None),
        ];
        let hits = collapse_partial_hits(symbols);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.locations.is_none()));
    }
}
