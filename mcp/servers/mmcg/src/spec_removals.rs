//! Bind removal acknowledgements to parser ordinals in one immutable baseline.

use crate::diff::{self, RemovedDeclarations};
use crate::indexer::{extractor_for_path, parse_baseline_blob};
use crate::spec::{ParsedSpec, SymbolSpec};
use crate::spec_symbols::{components, normalize_file, snapshot_scopes, Scope, Unresolved};
use crate::store::{PendingFile, PendingSymbol};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::time::Instant;

type Key = (String, usize);
const SYMBOL_LIMIT: usize = 200_000;

pub(crate) struct AcknowledgementError {
    pub name: String,
    pub file: Option<String>,
    pub error: Unresolved,
}

pub(crate) struct Plan {
    baseline: Result<Baseline, Unresolved>,
    acknowledged: HashSet<Key>,
    removed: HashSet<Key>,
    pub errors: Vec<AcknowledgementError>,
}

impl Plan {
    pub(crate) fn build(
        spec: &ParsedSpec,
        root: &Path,
        baseline_oid: &str,
        removed: &RemovedDeclarations,
        deadline: Instant,
        complete: bool,
    ) -> Option<Self> {
        let acknowledgements = &spec.frontmatter.as_ref()?.breaking_changes.removed_symbols;
        if acknowledgements.is_empty() {
            return None;
        }
        let mut scopes: Vec<_> = acknowledgements.iter().map(ack_scope).collect();
        for claim in &spec.pre_edit_snapshot {
            let matching = snapshot_scopes(spec, claim);
            if matching.is_empty() {
                scopes.push(Scope {
                    name: &claim.name,
                    file: None,
                    language: None,
                });
            } else {
                scopes.extend(matching);
            }
        }
        for touch in &spec.frontmatter.as_ref().expect("frontmatter").touches {
            for symbol in &touch.symbols {
                scopes.push(Scope {
                    name: symbol.name(),
                    file: Some(symbol.file().unwrap_or(&touch.file)),
                    language: symbol.language().or(touch.language.as_deref()),
                });
            }
        }
        let baseline = if complete {
            Baseline::load(root, baseline_oid, &scopes, removed, deadline)
        } else {
            Err(Unresolved::unavailable("diff_incomplete"))
        };
        let mut plan = Self {
            baseline,
            acknowledged: HashSet::new(),
            removed: removed
                .iter()
                .flat_map(|(file, indices)| indices.iter().map(|&index| (file.clone(), index)))
                .collect(),
            errors: Vec::new(),
        };
        for ack in acknowledgements {
            let resolved = plan
                .baseline
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|baseline| baseline.resolve_acknowledgement(ack));
            match resolved {
                Ok(key) => {
                    plan.acknowledged.insert(key);
                }
                Err(error) => plan.errors.push(AcknowledgementError {
                    name: ack.name().to_string(),
                    file: ack.file().map(str::to_string),
                    error,
                }),
            }
        }
        Some(plan)
    }

    pub(crate) fn unacknowledged(&self) -> Result<Vec<(String, String)>, Unresolved> {
        let baseline = self.baseline.as_ref().map_err(Clone::clone)?;
        let mut remaining = Vec::new();
        for key in &self.removed {
            let symbol = baseline.symbol(key)?;
            if symbol.kind != "module" && !self.acknowledged.contains(key) {
                let file = &baseline.files[&key.0];
                let name = ancestry(file, key.1, baseline.deadline)?.join(".");
                remaining.push((key.0.clone(), name));
            }
        }
        remaining.sort();
        Ok(remaining)
    }

    pub(crate) fn accepts_snapshot(
        &self,
        name: &str,
        scopes: &[Scope<'_>],
        signature: Option<&str>,
    ) -> Result<bool, Unresolved> {
        let baseline = self.baseline.as_ref().map_err(Clone::clone)?;
        let key = baseline.resolve(name, scopes, None)?;
        if !self.removed.contains(&key) || !self.acknowledged.contains(&key) {
            return Ok(false);
        }
        let symbol = baseline.symbol(&key)?;
        if signature.is_some_and(|signature| symbol.signature.as_deref() != Some(signature)) {
            return Err(Unresolved {
                reason: "signature_mismatch",
                matches: Some(1),
            });
        }
        Ok(true)
    }

    pub(crate) fn accepts_file_removal(&self, file: &str) -> bool {
        let Ok(file) = normalize_file(file) else {
            return false;
        };
        if self.baseline.is_err() {
            return false;
        }
        let mut keys = self.removed.iter().filter(|key| key.0 == file).peekable();
        keys.peek().is_some() && keys.all(|key| self.acknowledged.contains(key))
    }
}

fn ack_scope(ack: &SymbolSpec) -> Scope<'_> {
    Scope {
        name: ack.name(),
        file: ack.file(),
        language: ack.language(),
    }
}

struct Baseline {
    files: BTreeMap<String, PendingFile>,
    deadline: Instant,
}

impl Baseline {
    fn load(
        root: &Path,
        oid: &str,
        scopes: &[Scope<'_>],
        removed: &RemovedDeclarations,
        deadline: Instant,
    ) -> Result<Self, Unresolved> {
        let mut requested: BTreeSet<_> = removed.keys().cloned().collect();
        let mut global = false;
        for scope in scopes {
            match scope.file {
                Some(file) => {
                    requested.insert(normalize_file(file)?);
                }
                None => global = true,
            }
        }
        let paths = regular_paths(root, oid, (!global).then_some(&requested), deadline)?;
        let blobs =
            diff::baseline_blobs_for_paths_controlled(root, oid, &paths, Some(deadline), None)
                .map_err(|error| Unresolved::unavailable(error.code()))?;
        let mut files = BTreeMap::new();
        let mut symbols = 0usize;
        for path in paths {
            check_deadline(deadline)?;
            let bytes = blobs
                .get(&path)
                .and_then(Option::as_deref)
                .ok_or_else(|| Unresolved::unavailable("baseline_blob_missing"))?;
            let extractor = extractor_for_path(Path::new(&path))
                .ok_or_else(|| Unresolved::unavailable("baseline_language_unavailable"))?;
            let mut file = parse_baseline_blob(&path, bytes, extractor.as_ref())
                .map_err(|_| Unresolved::unavailable("baseline_parse_failed"))?;
            symbols = symbols.saturating_add(file.symbols.len());
            if symbols > SYMBOL_LIMIT {
                return Err(Unresolved::unavailable("baseline_symbol_limit"));
            }
            file.edges = Vec::new();
            files.insert(path, file);
        }
        check_deadline(deadline)?;
        let baseline = Self { files, deadline };
        for (file, indices) in removed {
            for &index in indices {
                baseline.symbol(&(file.clone(), index))?;
            }
        }
        Ok(baseline)
    }

    fn symbol(&self, key: &Key) -> Result<&PendingSymbol, Unresolved> {
        self.files
            .get(&key.0)
            .and_then(|file| file.symbols.get(key.1))
            .ok_or_else(|| Unresolved::unavailable("baseline_identity_missing"))
    }

    fn resolve_acknowledgement(&self, ack: &SymbolSpec) -> Result<Key, Unresolved> {
        let scopes = [ack_scope(ack)];
        // Impl blocks have no distinct named definition. An explicit file and
        // their complete header can address one structural impl, without using
        // signatures to choose between function/method overloads.
        let key = if let (Some(_), Some(signature)) = (ack.file(), ack.signature()) {
            match self.resolve(ack.name(), &scopes, Some(signature)) {
                Err(error) if error.reason == "missing" => {
                    self.resolve(ack.name(), &scopes, None)?
                }
                result => result?,
            }
        } else {
            self.resolve(ack.name(), &scopes, None)?
        };
        let symbol = self.symbol(&key)?;
        if ack
            .signature()
            .is_some_and(|signature| symbol.signature.as_deref() != Some(signature))
        {
            return Err(Unresolved {
                reason: "signature_mismatch",
                matches: Some(1),
            });
        }
        Ok(key)
    }

    fn resolve(
        &self,
        name: &str,
        scopes: &[Scope<'_>],
        impl_header: Option<&str>,
    ) -> Result<Key, Unresolved> {
        let declared = components(name)?;
        let unscoped = [Scope {
            name,
            file: None,
            language: None,
        }];
        let scopes = if scopes.is_empty() { &unscoped } else { scopes };
        let mut matches = BTreeSet::new();
        for scope in scopes {
            let scoped_name = components(scope.name)?;
            let scoped_file = scope.file.map(normalize_file).transpose()?;
            for (path, file) in &self.files {
                check_deadline(self.deadline)?;
                if scoped_file
                    .as_ref()
                    .is_some_and(|selected| selected != path)
                    || scope
                        .language
                        .is_some_and(|language| language != file.language)
                {
                    continue;
                }
                for (index, symbol) in file.symbols.iter().enumerate() {
                    check_deadline(self.deadline)?;
                    if symbol.kind == "module"
                        || match impl_header {
                            Some(header) => {
                                symbol.kind != "impl" || symbol.signature.as_deref() != Some(header)
                            }
                            None => symbol.kind == "impl",
                        }
                    {
                        continue;
                    }
                    // Compound namespaces have the same final component as
                    // their equivalent nested declaration.
                    if symbol.name.rsplit(['.', ':']).next() != declared.last().copied() {
                        continue;
                    }
                    let chain = ancestry(file, index, self.deadline)?;
                    if chain.ends_with(&declared) && chain.ends_with(&scoped_name) {
                        matches.insert((path.clone(), index));
                    }
                }
            }
        }
        match matches.len() {
            0 => Err(Unresolved {
                reason: "missing",
                matches: Some(0),
            }),
            1 => Ok(matches.into_iter().next().expect("one match")),
            count => Err(Unresolved {
                reason: "ambiguous",
                matches: Some(count),
            }),
        }
    }
}

fn check_deadline(deadline: Instant) -> Result<(), Unresolved> {
    if Instant::now() >= deadline {
        Err(Unresolved::unavailable("git_timeout"))
    } else {
        Ok(())
    }
}

fn ancestry(file: &PendingFile, index: usize, deadline: Instant) -> Result<Vec<&str>, Unresolved> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(index);
    while let Some(index) = current {
        check_deadline(deadline)?;
        if !seen.insert(index) {
            return Err(Unresolved::unavailable("parent_cycle"));
        }
        let symbol = file
            .symbols
            .get(index)
            .ok_or_else(|| Unresolved::unavailable("parent_missing_from_file"))?;
        if symbol.kind != "module" {
            chain.extend(components(&symbol.name)?.into_iter().rev());
        }
        current = symbol.parent_index;
    }
    chain.reverse();
    Ok(chain)
}

fn regular_paths(
    root: &Path,
    oid: &str,
    requested: Option<&BTreeSet<String>>,
    deadline: Instant,
) -> Result<Vec<String>, Unresolved> {
    let mut batches: Vec<Vec<String>> = vec![Vec::new()];
    let mut argument_bytes = 0usize;
    if let Some(requested) = requested {
        if requested.len() > diff::CHANGE_FILE_LIMIT {
            return Err(Unresolved::unavailable("baseline_file_limit"));
        }
        for path in requested {
            if path.len() > 32 * 1024 {
                return Err(Unresolved::unavailable("invalid_file_scope"));
            }
            let batch = batches.last_mut().expect("one batch");
            if !batch.is_empty()
                && (batch.len() == 256 || argument_bytes + path.len() + 10 > 32 * 1024)
            {
                batches.push(Vec::new());
                argument_bytes = 0;
            }
            batches
                .last_mut()
                .expect("one batch")
                .push(format!(":(literal){path}"));
            argument_bytes += path.len() + 10;
        }
        if requested.is_empty() {
            return Ok(Vec::new());
        }
    }
    let mut paths = BTreeSet::new();
    let mut output_bytes = 0usize;
    for batch in batches {
        check_deadline(deadline)?;
        let mut args = vec!["ls-tree", "-r", "-z", "--full-tree", oid, "--"];
        args.extend(batch.iter().map(String::as_str));
        let output = diff::run_bounded_git_with_limit_until(
            root,
            &args,
            None,
            diff::GIT_OUTPUT_LIMIT,
            Some(deadline),
        )
        .map_err(|error| Unresolved::unavailable(error.code()))?;
        if !output.success || (!output.stdout.is_empty() && output.stdout.last() != Some(&0)) {
            return Err(Unresolved::unavailable("baseline_inventory_failed"));
        }
        output_bytes = output_bytes.saturating_add(output.stdout.len());
        if output_bytes > diff::GIT_OUTPUT_LIMIT {
            return Err(Unresolved::unavailable("baseline_inventory_limit"));
        }
        for entry in output
            .stdout
            .split(|&byte| byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            let entry = std::str::from_utf8(entry)
                .map_err(|_| Unresolved::unavailable("baseline_path_encoding"))?;
            let (metadata, path) = entry
                .split_once('\t')
                .ok_or_else(|| Unresolved::unavailable("baseline_inventory_failed"))?;
            let fields: Vec<_> = metadata.split_whitespace().collect();
            if fields.len() != 3 || requested.is_some_and(|requested| !requested.contains(path)) {
                return Err(Unresolved::unavailable("baseline_inventory_failed"));
            }
            if matches!(fields[0], "100644" | "100755")
                && fields[1] == "blob"
                && extractor_for_path(Path::new(path)).is_some()
            {
                paths.insert(path.to_string());
                if paths.len() > diff::CHANGE_FILE_LIMIT {
                    return Err(Unresolved::unavailable("baseline_file_limit"));
                }
            }
        }
    }
    Ok(paths.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    fn git(root: &Path, args: &[&str], input: Option<&[u8]>) -> String {
        let mut child = Command::new("git")
            .current_dir(root)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            child.stdin.take().unwrap().write_all(input).unwrap();
        } else {
            drop(child.stdin.take());
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q", "--initial-branch=main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            git(directory.path(), &args, None);
        }
        directory
    }

    fn commit_index(root: &Path) -> String {
        git(root, &["commit", "-q", "-m", "baseline"], None);
        git(root, &["rev-parse", "HEAD"], None)
    }

    #[test]
    fn baseline_removal_inventory_is_complete_bounded_and_regular_only() {
        let directory = repository();
        let root = directory.path();
        let blob = git(
            root,
            &["hash-object", "-w", "--stdin"],
            Some(b"def run(): pass\n"),
        );
        let ghost = git(
            root,
            &["hash-object", "-w", "--stdin"],
            Some(b"def ghost(): pass\n"),
        );
        let rows = format!("100644 {blob}\ta.py\n120000 {ghost}\tghost.py\n");
        git(
            root,
            &["update-index", "--index-info"],
            Some(rows.as_bytes()),
        );
        let baseline = commit_index(root);
        let scope = Scope {
            name: "run",
            file: None,
            language: None,
        };
        let loaded = Baseline::load(
            root,
            &baseline,
            &[scope],
            &BTreeMap::new(),
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        assert!(loaded.resolve("run", &[], None).is_ok());
        assert_eq!(
            loaded.resolve("ghost", &[], None).unwrap_err().reason,
            "missing"
        );
        assert!(!loaded.files.contains_key("ghost.py"));
        let requested = BTreeSet::from(["ghost.py".to_string()]);
        assert!(regular_paths(
            root,
            &baseline,
            Some(&requested),
            Instant::now() + Duration::from_secs(30)
        )
        .unwrap()
        .is_empty());

        // Build the large tree in Git's index without creating thousands of
        // working files or invoking the application indexer.
        let mut rows = String::new();
        for index in 0..diff::CHANGE_FILE_LIMIT {
            rows.push_str(&format!("100644 {blob}\tsrc/file_{index:05}.py\n"));
        }
        git(
            root,
            &["update-index", "--index-info"],
            Some(rows.as_bytes()),
        );
        let large = commit_index(root);
        assert_eq!(
            regular_paths(root, &large, None, Instant::now() + Duration::from_secs(30))
                .unwrap_err()
                .reason,
            "baseline_file_limit"
        );
        let requested = BTreeSet::from(["a.py".to_string()]);
        assert_eq!(
            regular_paths(
                root,
                &large,
                Some(&requested),
                Instant::now() + Duration::from_secs(30)
            )
            .unwrap(),
            vec!["a.py"]
        );
        assert_eq!(
            regular_paths(
                root,
                &large,
                None,
                Instant::now() - Duration::from_millis(1)
            )
            .unwrap_err()
            .reason,
            "git_timeout"
        );
    }

    #[test]
    fn baseline_removal_resolution_rejects_invalid_parents_and_incomplete_input() {
        let extractor = extractor_for_path(Path::new("a.py")).unwrap();
        let mut file = parse_baseline_blob(
            "a.py",
            b"class A:\n    def run(self): pass\n",
            extractor.as_ref(),
        )
        .unwrap();
        let index = file
            .symbols
            .iter()
            .position(|symbol| symbol.name == "run")
            .unwrap();
        file.symbols[index].parent_index = Some(index);
        let mut baseline = Baseline {
            files: BTreeMap::from([("a.py".into(), file)]),
            deadline: Instant::now() + Duration::from_secs(30),
        };
        assert_eq!(
            baseline.resolve("A.run", &[], None).unwrap_err().reason,
            "parent_cycle"
        );
        baseline.files.get_mut("a.py").unwrap().symbols[index].parent_index = Some(usize::MAX);
        assert_eq!(
            baseline.resolve("A.run", &[], None).unwrap_err().reason,
            "parent_missing_from_file"
        );
        let parsed = crate::spec::parse_str(
            "spec.md",
            "---\nbreaking_changes:\n  removed_symbols: [run]\n---\n",
        );
        let plan = Plan::build(
            &parsed,
            Path::new("."),
            "unused",
            &BTreeMap::new(),
            Instant::now() + Duration::from_secs(30),
            false,
        )
        .unwrap();
        assert_eq!(plan.errors[0].error.reason, "diff_incomplete");
        assert!(plan.acknowledged.is_empty());
        assert!(plan.accepts_snapshot("run", &[], None).is_err());
        assert!(!plan.accepts_file_removal("a.py"));
    }
}
