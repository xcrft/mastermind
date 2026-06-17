//! Symbol-level diff between a git ref and the current index.
//!
//! Given a `<git-ref>`, computes which symbols were **added**, **removed**, or
//! had their **signature changed** between that ref and the on-disk index. Used
//! by `mmcg_symbols_changed_since` so PR-review agents can answer "what symbols
//! did this branch touch?" without grep-rolling the diff.
//!
//! Implementation:
//! 1. `git diff --name-only <ref>..HEAD` → files changed in the range
//! 2. For each file, fetch its blob at `<ref>` via `git show <ref>:<path>`
//! 3. Parse the old blob through the same extractor (see [`parse_blob`])
//! 4. Compare old (path, name, kind) set against `Store::symbols_in_file`
//!
//! Files absent at `<ref>` produce only "added" entries. Files deleted in HEAD
//! produce only "removed" (the index has no current symbols for them).
//!
//! Limitations:
//! - Only files where git reports a change. Pure metadata edits (chmod, rename)
//!   without content change won't surface symbols.
//! - Uses `git` via `subprocess` — fails fast if git isn't on PATH.
//! - Signature comparison is exact-string; trivial reformatting (e.g. param
//!   line wrapping) appears as a change.
//! - Per-file resolution: a symbol moved `a.py`→`b.py` shows as removed-from-a
//!   + added-to-b, not "moved".

use crate::indexer::{extractor_for_path, parse_blob};
use crate::store::{Store, Symbol};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Serialize, Clone)]
pub struct SymbolRef {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl From<Symbol> for SymbolRef {
    fn from(s: Symbol) -> Self {
        Self {
            file: s.file_path,
            name: s.name,
            kind: s.kind,
            line: s.line_start,
            signature: s.signature,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SignatureChange {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub old_signature: Option<String>,
    pub new_signature: Option<String>,
    pub new_line: u32,
}

#[derive(Debug, Serialize)]
pub struct SymbolDiff {
    /// The git ref the diff is computed against (whatever the user passed).
    pub git_ref: String,
    /// Files in scope (per `git diff --name-only <ref>..HEAD`). Includes
    /// unparseable files — listed but contributing no symbols.
    pub files_in_diff: Vec<String>,
    pub added: Vec<SymbolRef>,
    pub removed: Vec<SymbolRef>,
    pub signature_changed: Vec<SignatureChange>,
    /// Per-file errors (parse / blob-fetch failure). Non-fatal — other files
    /// still produce results. Aids caller debugging.
    pub errors: Vec<String>,
}

#[derive(Debug)]
pub enum DiffError {
    GitNotFound,
    GitRefMissing(String),
    GitFailed(String),
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::GitNotFound => write!(f, "`git` not found on PATH"),
            DiffError::GitRefMissing(r) => write!(f, "git ref not resolvable: {r}"),
            DiffError::GitFailed(m) => write!(f, "git command failed: {m}"),
        }
    }
}

impl std::error::Error for DiffError {}

/// Compute the symbol-level diff between `git_ref` and the current working
/// state (assumed already indexed in `store`).
///
/// `repo_root` is the directory paths are relative to — the same root used at
/// index time. `git_ref` must resolve via `git rev-parse`.
pub fn symbols_changed_since(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
) -> Result<SymbolDiff, DiffError> {
    validate_ref(repo_root, git_ref)?;

    let files_in_diff = git_diff_name_only(repo_root, git_ref)?;
    let mut added: Vec<SymbolRef> = Vec::new();
    let mut removed: Vec<SymbolRef> = Vec::new();
    let mut signature_changed: Vec<SignatureChange> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for rel in &files_in_diff {
        match diff_file(store, repo_root, git_ref, rel) {
            Ok(per_file) => {
                added.extend(per_file.added);
                removed.extend(per_file.removed);
                signature_changed.extend(per_file.signature_changed);
            }
            Err(e) => errors.push(format!("{rel}: {e}")),
        }
    }

    // Stable output order.
    added.sort_by(|a, b| (a.file.as_str(), a.line).cmp(&(b.file.as_str(), b.line)));
    removed.sort_by(|a, b| (a.file.as_str(), a.line).cmp(&(b.file.as_str(), b.line)));
    signature_changed.sort_by(|a, b| (a.file.as_str(), &a.name).cmp(&(b.file.as_str(), &b.name)));

    Ok(SymbolDiff {
        git_ref: git_ref.to_string(),
        files_in_diff,
        added,
        removed,
        signature_changed,
        errors,
    })
}

struct PerFileDiff {
    added: Vec<SymbolRef>,
    removed: Vec<SymbolRef>,
    signature_changed: Vec<SignatureChange>,
}

fn diff_file(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
    rel_path: &str,
) -> Result<PerFileDiff, String> {
    let extractor = extractor_for_path(Path::new(rel_path));

    // New side: from the live index. Empty if the file was deleted in HEAD or
    // never indexed.
    let new_symbols: Vec<Symbol> = store
        .symbols_in_file(rel_path)
        .map_err(|e| format!("symbols_in_file failed: {e}"))?
        .into_iter()
        .filter(|s| s.kind != "module")
        .collect();

    // Old side: parse blob at `git_ref`. Missing blob = file didn't exist at ref.
    let old_blob = match git_show_blob(repo_root, git_ref, rel_path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => Vec::new(),
        Err(e) => return Err(e),
    };

    let old_symbols: Vec<crate::store::PendingSymbol> = if old_blob.is_empty() {
        Vec::new()
    } else if let Some(ext) = extractor {
        let pending = parse_blob(rel_path, &old_blob, 0, ext.as_ref())
            .map_err(|e| format!("parse old blob: {e}"))?;
        pending
            .symbols
            .into_iter()
            .filter(|s| s.kind != "module")
            .collect()
    } else {
        // No extractor for this extension — can't diff symbols. Empty old side;
        // everything in new side becomes "added".
        Vec::new()
    };

    // Key by (name, kind). Same name+kind twice in one file is a
    // partial-class-style anomaly; accept the first match.
    let mut old_by_key: HashMap<(String, String), &crate::store::PendingSymbol> = HashMap::new();
    for s in &old_symbols {
        old_by_key
            .entry((s.name.clone(), s.kind.clone()))
            .or_insert(s);
    }
    let mut new_by_key: HashMap<(String, String), &Symbol> = HashMap::new();
    for s in &new_symbols {
        new_by_key
            .entry((s.name.clone(), s.kind.clone()))
            .or_insert(s);
    }

    let mut added: Vec<SymbolRef> = Vec::new();
    let mut removed: Vec<SymbolRef> = Vec::new();
    let mut signature_changed: Vec<SignatureChange> = Vec::new();

    // Added: in new, not in old.
    for ((name, kind), s) in &new_by_key {
        if !old_by_key.contains_key(&(name.clone(), kind.clone())) {
            added.push(SymbolRef::from((*s).clone()));
        }
    }
    // Removed: in old, not in new. Synthesize SymbolRef from PendingSymbol.
    for ((name, kind), s) in &old_by_key {
        if !new_by_key.contains_key(&(name.clone(), kind.clone())) {
            removed.push(SymbolRef {
                file: rel_path.to_string(),
                name: name.clone(),
                kind: kind.clone(),
                line: s.line_start,
                signature: s.signature.clone(),
            });
        }
    }
    // Signature changed: in both, but signature differs.
    for ((name, kind), new_s) in &new_by_key {
        if let Some(old_s) = old_by_key.get(&(name.clone(), kind.clone())) {
            if old_s.signature != new_s.signature {
                signature_changed.push(SignatureChange {
                    file: rel_path.to_string(),
                    name: name.clone(),
                    kind: kind.clone(),
                    old_signature: old_s.signature.clone(),
                    new_signature: new_s.signature.clone(),
                    new_line: new_s.line_start,
                });
            }
        }
    }

    Ok(PerFileDiff {
        added,
        removed,
        signature_changed,
    })
}

// ----- git plumbing -------------------------------------------------------

fn run_git(repo: &Path, args: &[&str]) -> Result<std::process::Output, DiffError> {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                DiffError::GitNotFound
            } else {
                DiffError::GitFailed(e.to_string())
            }
        })
}

fn validate_ref(repo: &Path, git_ref: &str) -> Result<(), DiffError> {
    let out = run_git(repo, &["rev-parse", "--verify", git_ref])?;
    if !out.status.success() {
        return Err(DiffError::GitRefMissing(git_ref.to_string()));
    }
    Ok(())
}

fn git_diff_name_only(repo: &Path, git_ref: &str) -> Result<Vec<String>, DiffError> {
    let range = format!("{git_ref}..HEAD");
    let out = run_git(repo, &["diff", "--name-only", &range])?;
    if !out.status.success() {
        return Err(DiffError::GitFailed(
            String::from_utf8_lossy(&out.stderr).to_string(),
        ));
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let mut files: Vec<String> = s
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// `Ok(None)` when the file didn't exist at `git_ref` (treated as "added in
/// HEAD"). `Ok(Some(bytes))` is the raw blob content.
fn git_show_blob(repo: &Path, git_ref: &str, rel_path: &str) -> Result<Option<Vec<u8>>, String> {
    let spec = format!("{git_ref}:{rel_path}");
    let out = Command::new("git")
        .args(["show", &spec])
        .current_dir(repo)
        .output()
        .map_err(|e| format!("spawn git show: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        // git returns "fatal: path '...' does not exist in '<ref>'" for a new
        // file. Treat as no-blob, not failure.
        if stderr.contains("does not exist") || stderr.contains("exists on disk, but not in") {
            return Ok(None);
        }
        return Err(format!("git show failed: {}", stderr.trim()));
    }
    Ok(Some(out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::process::Command;

    fn init_repo(name: &str) -> std::path::PathBuf {
        let dir = env::temp_dir().join(format!("mmcg-diff-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "--initial-branch=main"]);
        run(&dir, &["config", "user.email", "t@t"]);
        run(&dir, &["config", "user.name", "t"]);
        run(&dir, &["config", "commit.gpgsign", "false"]);
        dir
    }

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    #[test]
    fn end_to_end_added_removed_changed_signature() {
        let dir = init_repo("e2e");

        // Baseline: one file with two functions.
        write(
            &dir,
            "src/a.py",
            "def keep_same():\n    return 1\n\ndef will_be_removed():\n    return 2\n\ndef will_change_sig():\n    return 3\n",
        );
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["tag", "baseline"]);

        // HEAD: remove a function, change another's signature, add a file.
        write(
            &dir,
            "src/a.py",
            "def keep_same():\n    return 1\n\ndef will_change_sig(force: bool = False):\n    return 3\n",
        );
        write(&dir, "src/b.py", "def brand_new():\n    return 99\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "head"]);

        // Index the HEAD state into a fresh store.
        let db = dir.join(".mmcg.db");
        let mut store = Store::open(&db).unwrap();
        let indexer = crate::indexer::Indexer::new(&dir);
        indexer.index_all(&mut store, false).unwrap();

        let diff = symbols_changed_since(&store, &dir, "baseline").unwrap();
        assert_eq!(diff.git_ref, "baseline");
        // Two files changed in this range.
        assert_eq!(diff.files_in_diff, vec!["src/a.py", "src/b.py"]);

        let added_names: Vec<&str> = diff.added.iter().map(|s| s.name.as_str()).collect();
        let removed_names: Vec<&str> = diff.removed.iter().map(|s| s.name.as_str()).collect();
        let sig_names: Vec<&str> = diff
            .signature_changed
            .iter()
            .map(|s| s.name.as_str())
            .collect();

        assert!(added_names.contains(&"brand_new"), "missing brand_new");
        assert!(
            removed_names.contains(&"will_be_removed"),
            "missing will_be_removed in removed"
        );
        assert!(
            sig_names.contains(&"will_change_sig"),
            "missing will_change_sig in signature_changed"
        );
        // `keep_same` should NOT appear anywhere.
        assert!(!added_names.contains(&"keep_same"));
        assert!(!removed_names.contains(&"keep_same"));
        assert!(!sig_names.contains(&"keep_same"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_ref_errors_cleanly() {
        let dir = init_repo("missing_ref");
        write(&dir, "src/x.py", "def a(): pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "init"]);

        let db = dir.join(".mmcg.db");
        let store = Store::open(&db).unwrap();
        let result = symbols_changed_since(&store, &dir, "nonexistent-ref");
        assert!(matches!(result, Err(DiffError::GitRefMissing(_))));

        fs::remove_dir_all(&dir).ok();
    }
}
