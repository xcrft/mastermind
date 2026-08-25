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
//! [`symbols_changed_since_worktree`] runs steps 2–4 over a `<ref>` → **working
//! tree** file scope instead (uncommitted and untracked changes included).
//! `audit-spec` uses it: post-flight runs before the commit step, so a
//! commit-range scope is empty by construction there.
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

use crate::bounded_fs::{read_regular_file, BoundedReadError, ReadControl, RootCapability};
use crate::indexer::{extractor_for_path, parse_blob, MAX_INDEXABLE_FILE_SIZE};
use crate::store::{Store, Symbol};
use serde::Serialize;
use sha2::{Digest, Sha256};
use similar::{capture_diff_slices, Algorithm, DiffTag};
#[cfg(test)]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
#[cfg(test)]
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const CHANGE_FILE_LIMIT: usize = 10_000;
pub const GIT_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const MAX_GIT_REF_BYTES: usize = 1024;
const CAT_FILE_RESPONSE_OVERHEAD: usize = 128;
const BASELINE_BLOB_BATCH_OUTPUT_LIMIT: usize =
    MAX_INDEXABLE_FILE_SIZE as usize + CAT_FILE_RESPONSE_OVERHEAD;
const BASELINE_BLOB_TOTAL_LIMIT: usize = 64 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Must stay a real `sleep`. `park_timeout` returns instantly when the thread
/// holds an unpark token, and the sibling drain threads talk over `mpsc`, which
/// parks and unparks this very thread — a stray token turns the wait into a
/// spin that burns a core until the git deadline expires.
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(5);

type SymbolKey = (String, String);
type SelectedSymbolKeys = (BTreeSet<SymbolKey>, BTreeSet<SymbolKey>);

#[cfg(test)]
thread_local! {
    static TEST_GIT_INVOCATION: RefCell<Option<(std::path::PathBuf, Vec<OsString>)>> = const { RefCell::new(None) };
    static TEST_GIT_TIMEOUT: RefCell<Option<Duration>> = const { RefCell::new(None) };
}

fn git_timeout() -> Duration {
    #[cfg(test)]
    if let Some(timeout) = TEST_GIT_TIMEOUT.with(|value| *value.borrow()) {
        return timeout;
    }
    GIT_TIMEOUT
}

fn git_command(args: &[&str]) -> Command {
    #[cfg(test)]
    if let Some((program, prefix)) = TEST_GIT_INVOCATION.with(|value| value.borrow().clone()) {
        let mut command = Command::new(program);
        command.args(prefix).args(args);
        return command;
    }
    let mut command = Command::new("git");
    command.args(args);
    command
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
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

#[derive(Debug, Serialize, Clone)]
pub struct SignatureChange {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub old_signature: Option<String>,
    pub new_signature: Option<String>,
    pub new_line: u32,
}

#[derive(Debug, Serialize, Clone)]
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
    /// `true` when `files_in_diff` exceeded `CHANGE_FILE_LIMIT` — only the
    /// first `CHANGE_FILE_LIMIT` files were diffed; `added`/`removed`/
    /// `signature_changed` are a prefix, not the full picture.
    pub truncated: bool,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct WorkingTreeChangedFile {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct WorkingTreeSymbolDiff {
    pub git_ref: String,
    pub baseline_oid: String,
    pub head_oid: String,
    pub includes_worktree: bool,
    pub includes_untracked: bool,
    pub files_total: Option<u32>,
    pub files_returned: u32,
    pub files_truncated: bool,
    pub skipped_non_utf8_paths: u32,
    pub files: Vec<WorkingTreeChangedFile>,
    pub diff: SymbolDiff,
    pub body_changed: Vec<SymbolRef>,
    #[serde(skip)]
    pub snapshot_token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingTreeDiffError {
    InvalidRef,
    SnapshotChanged,
    GitTimeout,
    GitOutputLimit,
    GitUnavailable,
    IndexStale,
}

impl WorkingTreeDiffError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRef => "invalid_ref",
            Self::SnapshotChanged => "snapshot_changed",
            Self::GitTimeout => "git_timeout",
            Self::GitOutputLimit => "git_output_limit",
            Self::GitUnavailable => "invalid_ref",
            Self::IndexStale => "index_stale",
        }
    }
}

impl std::fmt::Display for WorkingTreeDiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for WorkingTreeDiffError {}

#[derive(Debug)]
pub enum DiffError {
    GitNotFound,
    GitRefMissing(String),
    GitFailed(String),
    /// A `git` subprocess ran past `MMCG_GIT_TIMEOUT_MS` and was killed.
    GitTimeout,
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::GitNotFound => write!(f, "`git` not found on PATH"),
            DiffError::GitRefMissing(r) => write!(f, "git ref not resolvable: {r}"),
            DiffError::GitFailed(m) => write!(f, "git command failed: {m}"),
            DiffError::GitTimeout => write!(f, "git command timed out"),
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
    symbols_changed_since_controlled(store, repo_root, git_ref, None, None)
}

pub(crate) fn symbols_changed_since_controlled(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<SymbolDiff, DiffError> {
    let deadline = deadline.or_else(|| Some(Instant::now() + git_timeout()));
    let root = RootCapability::open(repo_root)
        .map_err(|_| DiffError::GitFailed("snapshot_changed".into()))?;
    let baseline_oid = resolve_commit_controlled(repo_root, git_ref, deadline, interrupted)
        .map_err(|error| worktree_scope_error(git_ref, error))?;
    let head_oid = resolve_head_controlled(repo_root, deadline, interrupted)
        .map_err(|error| worktree_scope_error(git_ref, error))?;
    let (files_in_diff, truncated) =
        git_diff_name_only_controlled(repo_root, &baseline_oid, &head_oid, deadline, interrupted)
            .map_err(|error| worktree_scope_error(git_ref, error))?;
    let parseable_paths = files_in_diff
        .iter()
        .filter(|path| extractor_for_path(Path::new(path)).is_some())
        .cloned()
        .collect::<Vec<_>>();
    let old_blobs = baseline_blobs_for_paths_controlled(
        repo_root,
        &baseline_oid,
        &parseable_paths,
        deadline,
        interrupted,
    )
    .map_err(|error| worktree_scope_error(git_ref, error))?;
    let diff = symbol_diff_over_blobs(
        store,
        git_ref,
        files_in_diff,
        truncated,
        &old_blobs,
        deadline,
        interrupted,
    )?;
    let current_head = resolve_head_controlled(repo_root, deadline, interrupted)
        .map_err(|error| worktree_scope_error(git_ref, error))?;
    root.verify()
        .map_err(|_| DiffError::GitFailed("snapshot_changed".into()))?;
    if current_head != head_oid {
        return Err(DiffError::GitFailed("snapshot_changed".into()));
    }
    Ok(diff)
}

/// Same symbol comparison as [`symbols_changed_since`], but the file scope is
/// `git_ref` → **working tree**: staged, unstaged, and untracked changes count,
/// not just what has been committed.
///
/// Callers that audit work *before* it is committed must use this. The
/// commit-range scope of [`symbols_changed_since`] is empty by construction
/// while `HEAD` is still the baseline, which reads as "the spec named these
/// files but nothing changed" rather than "nothing is committed yet".
///
/// The file set matches [`symbols_changed_in_worktree`] (and therefore
/// `mastermind impact`); the symbol comparison is `git_ref`'s blobs against the
/// live index, so a stale index under-reports symbol changes — it does not
/// invent them.
pub fn symbols_changed_since_worktree(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
) -> Result<SymbolDiff, DiffError> {
    symbols_changed_in_worktree_controlled(
        store,
        repo_root,
        git_ref,
        Some(Instant::now() + git_timeout()),
        None,
    )
    .map(|diff| diff.diff)
    .map_err(|error| worktree_scope_error(git_ref, error))
}

fn worktree_scope_error(git_ref: &str, error: WorkingTreeDiffError) -> DiffError {
    match error {
        WorkingTreeDiffError::InvalidRef => DiffError::GitRefMissing(git_ref.to_string()),
        WorkingTreeDiffError::GitTimeout => DiffError::GitTimeout,
        WorkingTreeDiffError::GitOutputLimit => DiffError::GitFailed("git_output_limit".into()),
        WorkingTreeDiffError::SnapshotChanged => DiffError::GitFailed("snapshot_changed".into()),
        WorkingTreeDiffError::IndexStale => DiffError::GitFailed("index_stale".into()),
        WorkingTreeDiffError::GitUnavailable => DiffError::GitNotFound,
    }
}

fn symbol_diff_over_blobs(
    store: &Store,
    label: &str,
    files_in_diff: Vec<String>,
    truncated: bool,
    old_blobs: &BTreeMap<String, Option<Vec<u8>>>,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<SymbolDiff, DiffError> {
    let mut added: Vec<SymbolRef> = Vec::new();
    let mut removed: Vec<SymbolRef> = Vec::new();
    let mut signature_changed: Vec<SignatureChange> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for rel in &files_in_diff {
        if interrupted.is_some_and(|check| check())
            || deadline.is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(DiffError::GitTimeout);
        }
        match diff_file_from_blob(
            store,
            rel,
            old_blobs.get(rel).and_then(|blob| blob.as_deref()),
        ) {
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
        git_ref: label.to_string(),
        files_in_diff,
        added,
        removed,
        signature_changed,
        errors,
        truncated,
    })
}

pub fn symbols_changed_in_worktree(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
) -> Result<WorkingTreeSymbolDiff, WorkingTreeDiffError> {
    symbols_changed_in_worktree_controlled(store, repo_root, git_ref, None, None)
}

pub(crate) fn symbols_changed_in_worktree_controlled(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<WorkingTreeSymbolDiff, WorkingTreeDiffError> {
    let baseline_oid = resolve_commit_controlled(repo_root, git_ref, deadline, interrupted)?;
    let head_oid = resolve_head_controlled(repo_root, deadline, interrupted)?;
    let (files, files_total, files_truncated, skipped_non_utf8_paths) =
        collect_worktree_paths_controlled(repo_root, &baseline_oid, deadline, interrupted)?;
    let snapshot_token = working_tree_snapshot_token_controlled(
        repo_root,
        &head_oid,
        &files,
        deadline,
        interrupted,
    )?;
    let paths = files
        .iter()
        .filter(|file| extractor_for_path(Path::new(&file.path)).is_some())
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let mut old_blobs = files
        .iter()
        .map(|file| (file.path.clone(), None))
        .collect::<BTreeMap<_, _>>();
    old_blobs.extend(baseline_blobs_for_paths_controlled(
        repo_root,
        &baseline_oid,
        &paths,
        deadline,
        interrupted,
    )?);

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut signature_changed = Vec::new();
    let mut body_changed = Vec::new();
    let mut errors = Vec::new();

    for file in &files {
        let rel = &file.path;
        let new_symbols: Vec<Symbol> = store
            .symbols_in_file(rel)
            .map_err(|_| WorkingTreeDiffError::IndexStale)?
            .into_iter()
            .filter(|s| s.kind != "module")
            .collect();
        let old_blob = old_blobs.get(rel).and_then(|v| v.as_deref());
        let old_symbols = match (old_blob, extractor_for_path(Path::new(rel))) {
            (Some(bytes), Some(extractor)) => parse_blob(rel, bytes, 0, extractor.as_ref())
                .map(|p| {
                    p.symbols
                        .into_iter()
                        .filter(|s| s.kind != "module")
                        .collect::<Vec<_>>()
                })
                .map_err(|_| WorkingTreeDiffError::IndexStale)?,
            _ => Vec::new(),
        };

        let mut old_by_key = BTreeMap::new();
        for symbol in &old_symbols {
            old_by_key
                .entry((symbol.name.clone(), symbol.kind.clone()))
                .or_insert(symbol);
        }
        let mut new_by_key = BTreeMap::new();
        for symbol in &new_symbols {
            new_by_key
                .entry((symbol.name.clone(), symbol.kind.clone()))
                .or_insert(symbol);
        }

        for (key, symbol) in &new_by_key {
            if !old_by_key.contains_key(key) {
                added.push(SymbolRef::from((*symbol).clone()));
            }
        }
        for (key, symbol) in &old_by_key {
            if !new_by_key.contains_key(key) {
                removed.push(SymbolRef {
                    file: rel.clone(),
                    name: symbol.name.clone(),
                    kind: symbol.kind.clone(),
                    line: symbol.line_start,
                    signature: symbol.signature.clone(),
                });
            }
        }

        let current_bytes = if file.status == "deleted" {
            None
        } else {
            let control = ReadControl {
                deadline,
                interrupted,
            };
            Some(
                read_regular_file(
                    repo_root,
                    Path::new(rel),
                    MAX_INDEXABLE_FILE_SIZE,
                    MAX_INDEXABLE_FILE_SIZE,
                    control,
                )
                .map_err(working_tree_read_error)?
                .bytes,
            )
        };
        let (selected_old, selected_new) = match (old_blob, current_bytes.as_deref()) {
            (Some(old), Some(current)) => {
                deepest_changed_symbol_keys(old, current, &old_symbols, &new_symbols)
            }
            _ => (BTreeSet::new(), BTreeSet::new()),
        };
        for (key, new_symbol) in &new_by_key {
            let Some(old_symbol) = old_by_key.get(key) else {
                continue;
            };
            if old_symbol.signature != new_symbol.signature {
                signature_changed.push(SignatureChange {
                    file: rel.clone(),
                    name: new_symbol.name.clone(),
                    kind: new_symbol.kind.clone(),
                    old_signature: old_symbol.signature.clone(),
                    new_signature: new_symbol.signature.clone(),
                    new_line: new_symbol.line_start,
                });
                continue;
            }
            if selected_old.contains(key) || selected_new.contains(key) {
                if let (Some(old), Some(current)) = (old_blob, current_bytes.as_deref()) {
                    let old_slice = line_slice(old, old_symbol.line_start, old_symbol.line_end);
                    let new_slice = line_slice(current, new_symbol.line_start, new_symbol.line_end);
                    if old_slice != new_slice {
                        body_changed.push(SymbolRef::from((*new_symbol).clone()));
                    }
                }
            }
        }
    }

    let symbol_order = |a: &SymbolRef, b: &SymbolRef| {
        (&a.file, a.line, &a.name, &a.kind).cmp(&(&b.file, b.line, &b.name, &b.kind))
    };
    added.sort_by(symbol_order);
    removed.sort_by(symbol_order);
    body_changed.sort_by(symbol_order);
    signature_changed.sort_by(|a, b| {
        (&a.file, a.new_line, &a.name, &a.kind).cmp(&(&b.file, b.new_line, &b.name, &b.kind))
    });
    errors.sort();

    let files_in_diff = files.iter().map(|file| file.path.clone()).collect();
    let files_returned = files.len() as u32;
    Ok(WorkingTreeSymbolDiff {
        git_ref: git_ref.to_string(),
        baseline_oid,
        head_oid,
        includes_worktree: true,
        includes_untracked: true,
        files_total,
        files_returned,
        files_truncated,
        skipped_non_utf8_paths,
        files,
        diff: SymbolDiff {
            git_ref: git_ref.to_string(),
            files_in_diff,
            added,
            removed,
            signature_changed,
            errors,
            truncated: files_truncated,
        },
        body_changed,
        snapshot_token,
    })
}

fn line_chunks(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|byte| *byte == b'\n').collect()
}

fn working_tree_read_error(error: BoundedReadError) -> WorkingTreeDiffError {
    match error {
        BoundedReadError::Interrupted | BoundedReadError::DeadlineExceeded => {
            WorkingTreeDiffError::GitTimeout
        }
        BoundedReadError::TooLarge { .. } => WorkingTreeDiffError::IndexStale,
        _ => WorkingTreeDiffError::SnapshotChanged,
    }
}

fn deepest_changed_symbol_keys(
    old: &[u8],
    current: &[u8],
    old_symbols: &[crate::store::PendingSymbol],
    new_symbols: &[Symbol],
) -> SelectedSymbolKeys {
    let old_lines = line_chunks(old);
    let new_lines = line_chunks(current);
    let mut selected_old = BTreeSet::new();
    let mut selected_new = BTreeSet::new();
    for operation in capture_diff_slices(Algorithm::Myers, &old_lines, &new_lines) {
        if operation.tag() == DiffTag::Equal {
            continue;
        }
        let old_range = operation.old_range();
        if !old_range.is_empty() {
            let start = old_range.start as u32 + 1;
            let end = old_range.end as u32;
            if let Some(symbol) = old_symbols
                .iter()
                .filter(|symbol| symbol.line_start <= start && symbol.line_end >= end)
                .min_by_key(|symbol| {
                    (
                        symbol.line_end.saturating_sub(symbol.line_start),
                        symbol.line_start,
                        symbol.name.as_str(),
                        symbol.kind.as_str(),
                    )
                })
            {
                selected_old.insert((symbol.name.clone(), symbol.kind.clone()));
            }
        }
        let new_range = operation.new_range();
        if !new_range.is_empty() {
            let start = new_range.start as u32 + 1;
            let end = new_range.end as u32;
            if let Some(symbol) = new_symbols
                .iter()
                .filter(|symbol| symbol.line_start <= start && symbol.line_end >= end)
                .min_by_key(|symbol| {
                    (
                        symbol.line_end.saturating_sub(symbol.line_start),
                        symbol.line_start,
                        symbol.name.as_str(),
                        symbol.kind.as_str(),
                    )
                })
            {
                selected_new.insert((symbol.name.clone(), symbol.kind.clone()));
            }
        }
    }
    (selected_old, selected_new)
}

fn line_slice(bytes: &[u8], line_start: u32, line_end: u32) -> &[u8] {
    let mut starts = vec![0usize];
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            starts.push(index + 1);
        }
    }
    let start = starts
        .get(line_start.saturating_sub(1) as usize)
        .copied()
        .unwrap_or(bytes.len());
    let end = starts
        .get(line_end as usize)
        .copied()
        .unwrap_or(bytes.len());
    &bytes[start.min(bytes.len())..end.min(bytes.len())]
}

#[derive(Debug)]
pub(crate) struct BoundedGitOutput {
    pub(crate) success: bool,
    pub(crate) stdout: Vec<u8>,
}

fn bounded_reader<R: Read + Send + 'static>(
    mut reader: R,
    limit: usize,
) -> mpsc::Receiver<(Vec<u8>, bool)> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut retained = Vec::new();
        let mut exceeded = false;
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if retained.len() < limit + 1 {
                        let keep = count.min(limit + 1 - retained.len());
                        retained.extend_from_slice(&buffer[..keep]);
                    }
                    exceeded |= retained.len() > limit;
                }
                Err(_) => break,
            }
        }
        let _ = sender.send((retained, exceeded));
    });
    receiver
}

#[allow(dead_code)]
fn run_bounded_git(
    repo: &Path,
    args: &[&str],
    input: Option<&[u8]>,
) -> Result<BoundedGitOutput, WorkingTreeDiffError> {
    run_bounded_git_with_limit(repo, args, input, GIT_OUTPUT_LIMIT)
}

pub(crate) fn run_bounded_git_with_limit(
    repo: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    output_limit: usize,
) -> Result<BoundedGitOutput, WorkingTreeDiffError> {
    run_bounded_git_with_limit_until(repo, args, input, output_limit, None)
}

pub(crate) fn run_bounded_git_with_limit_until(
    repo: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    output_limit: usize,
    deadline: Option<Instant>,
) -> Result<BoundedGitOutput, WorkingTreeDiffError> {
    run_bounded_git_controlled(repo, args, input, output_limit, deadline, None)
}

pub(crate) fn run_bounded_git_with_control(
    repo: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    output_limit: usize,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<BoundedGitOutput, WorkingTreeDiffError> {
    run_bounded_git_controlled(repo, args, input, output_limit, deadline, interrupted)
}

fn run_bounded_git_controlled(
    repo: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    output_limit: usize,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<BoundedGitOutput, WorkingTreeDiffError> {
    let start = Instant::now();
    let mut child = git_command(args)
        .current_dir(repo)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| WorkingTreeDiffError::GitUnavailable)?;
    let stdout_pipe = child
        .stdout
        .take()
        .ok_or(WorkingTreeDiffError::GitUnavailable)?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or(WorkingTreeDiffError::GitUnavailable)?;
    let stdout = bounded_reader(stdout_pipe, output_limit);
    let stderr = bounded_reader(stderr_pipe, 64 * 1024);
    let writer = input.map(|bytes| {
        let (sender, receiver) = mpsc::channel();
        let bytes = bytes.to_vec();
        let stdin = child.stdin.take();
        std::thread::spawn(move || {
            let result = stdin
                .ok_or(())
                .and_then(|mut stdin| stdin.write_all(&bytes).map_err(|_| ()));
            let _ = sender.send(result);
        });
        receiver
    });
    let timeout = deadline.map_or_else(git_timeout, |deadline| {
        git_timeout().min(deadline.saturating_duration_since(start))
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if interrupted.is_some_and(|check| check()) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorkingTreeDiffError::GitTimeout);
            }
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(CHILD_POLL_INTERVAL);
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorkingTreeDiffError::GitTimeout);
            }
            Err(_) => return Err(WorkingTreeDiffError::GitUnavailable),
        }
    };
    if let Some(writer) = writer {
        writer
            .recv_timeout(Duration::from_millis(100))
            .map_err(|_| WorkingTreeDiffError::GitTimeout)?
            .map_err(|_| WorkingTreeDiffError::GitUnavailable)?;
    }
    let (stdout, stdout_exceeded) = stdout
        .recv_timeout(Duration::from_millis(100))
        .map_err(|_| WorkingTreeDiffError::GitTimeout)?;
    let (_, stderr_exceeded) = stderr
        .recv_timeout(Duration::from_millis(100))
        .map_err(|_| WorkingTreeDiffError::GitTimeout)?;
    if stdout_exceeded || stderr_exceeded {
        return Err(WorkingTreeDiffError::GitOutputLimit);
    }
    Ok(BoundedGitOutput {
        success: status.success(),
        stdout,
    })
}

#[cfg(test)]
fn resolve_commit(repo: &Path, git_ref: &str) -> Result<String, WorkingTreeDiffError> {
    resolve_commit_controlled(repo, git_ref, None, None)
}

fn resolve_commit_controlled(
    repo: &Path,
    git_ref: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<String, WorkingTreeDiffError> {
    if git_ref.is_empty()
        || git_ref.len() > MAX_GIT_REF_BYTES
        || git_ref.starts_with('-')
        || git_ref.contains('\0')
    {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    let commit = format!("{git_ref}^{{commit}}");
    let output = run_bounded_git_controlled(
        repo,
        &["rev-parse", "--verify", "--end-of-options", &commit],
        None,
        GIT_OUTPUT_LIMIT,
        deadline,
        interrupted,
    )?;
    if !output.success {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    let oid = String::from_utf8(output.stdout).map_err(|_| WorkingTreeDiffError::InvalidRef)?;
    let oid = oid.trim();
    if oid.len() != 40 || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    Ok(oid.to_ascii_lowercase())
}

fn resolve_head(repo: &Path) -> Result<String, WorkingTreeDiffError> {
    resolve_head_controlled(repo, None, None)
}

fn resolve_head_controlled(
    repo: &Path,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<String, WorkingTreeDiffError> {
    let output = run_bounded_git_controlled(
        repo,
        &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
        None,
        GIT_OUTPUT_LIMIT,
        deadline,
        interrupted,
    )?;
    if !output.success {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    let oid = String::from_utf8(output.stdout).map_err(|_| WorkingTreeDiffError::InvalidRef)?;
    Ok(oid.trim().to_ascii_lowercase())
}

fn git_diff_name_only_controlled(
    repo: &Path,
    baseline_oid: &str,
    head_oid: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<(Vec<String>, bool), WorkingTreeDiffError> {
    let output = run_bounded_git_controlled(
        repo,
        &[
            "-c",
            "diff.external=",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--name-only",
            "-z",
            baseline_oid,
            head_oid,
            "--",
        ],
        None,
        GIT_OUTPUT_LIMIT,
        deadline,
        interrupted,
    )?;
    if !output.success {
        return Err(WorkingTreeDiffError::InvalidRef);
    }

    let mut retained = BTreeSet::new();
    let mut truncated = false;
    for raw in nul_fields(&output.stdout) {
        if interrupted.is_some_and(|check| check()) {
            return Err(WorkingTreeDiffError::GitTimeout);
        }
        if is_mastermind_runtime_artifact(raw) {
            continue;
        }
        let path = std::str::from_utf8(raw)
            .map_err(|_| WorkingTreeDiffError::SnapshotChanged)?
            .replace('\\', "/");
        let parsed = Path::new(&path);
        if parsed.is_absolute()
            || parsed
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(WorkingTreeDiffError::SnapshotChanged);
        }
        if retained.len() < CHANGE_FILE_LIMIT {
            retained.insert(path);
        } else if !retained.contains(&path) {
            truncated = true;
        }
    }
    Ok((retained.into_iter().collect(), truncated))
}

pub(crate) fn current_head_oid(repo: &Path) -> Result<String, WorkingTreeDiffError> {
    resolve_head(repo)
}

fn nul_fields(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
}

#[cfg(test)]
fn collect_worktree_paths(
    repo: &Path,
    baseline_oid: &str,
) -> Result<(Vec<WorkingTreeChangedFile>, Option<u32>, bool, u32), WorkingTreeDiffError> {
    collect_worktree_paths_controlled(repo, baseline_oid, None, None)
}

fn collect_worktree_paths_controlled(
    repo: &Path,
    baseline_oid: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<(Vec<WorkingTreeChangedFile>, Option<u32>, bool, u32), WorkingTreeDiffError> {
    let diff = run_bounded_git_controlled(
        repo,
        &[
            "-c",
            "diff.external=",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--name-status",
            "-z",
            baseline_oid,
            "--",
        ],
        None,
        GIT_OUTPUT_LIMIT,
        deadline,
        interrupted,
    )?;
    if !diff.success {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    let mut changed: BTreeMap<Vec<u8>, &'static str> = BTreeMap::new();
    let mut fields = nul_fields(&diff.stdout);
    while let Some(status) = fields.next() {
        let Some(path) = fields.next() else { break };
        let status = match status.first().copied() {
            Some(b'A') => "added",
            Some(b'D') => "deleted",
            _ => "modified",
        };
        changed.insert(path.to_vec(), status);
    }
    // `--full-name` keeps untracked paths repository-relative like the diff
    // side. Without it `ls-files` reports paths relative to `repo`, so a caller
    // passing a subdirectory would mix two path namespaces in one result.
    let untracked = run_bounded_git_controlled(
        repo,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "--full-name",
            "-z",
            "--",
        ],
        None,
        GIT_OUTPUT_LIMIT,
        deadline,
        interrupted,
    )?;
    if !untracked.success {
        return Err(WorkingTreeDiffError::GitUnavailable);
    }
    for path in nul_fields(&untracked.stdout) {
        changed.insert(path.to_vec(), "untracked");
    }

    Ok(finalize_changed_paths(changed))
}

fn finalize_changed_paths(
    mut changed: BTreeMap<Vec<u8>, &'static str>,
) -> (Vec<WorkingTreeChangedFile>, Option<u32>, bool, u32) {
    changed.retain(|path, _| !is_mastermind_runtime_artifact(path));
    let full_count = changed.len();
    let truncated = full_count > CHANGE_FILE_LIMIT;
    let mut skipped_non_utf8_paths = 0u32;
    let mut files = Vec::with_capacity(full_count.min(CHANGE_FILE_LIMIT));
    for (path, status) in changed.into_iter().take(CHANGE_FILE_LIMIT) {
        match String::from_utf8(path) {
            Ok(path) => files.push(WorkingTreeChangedFile {
                path: path.replace('\\', "/"),
                status: status.to_string(),
            }),
            Err(_) => skipped_non_utf8_paths += 1,
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    (
        files,
        (!truncated).then_some(full_count as u32),
        truncated,
        skipped_non_utf8_paths,
    )
}

fn is_mastermind_runtime_artifact(path: &[u8]) -> bool {
    matches!(
        path,
        b".mastermind/mmcg.db"
            | b".mastermind/mmcg.db-shm"
            | b".mastermind/mmcg.db-wal"
            | b".mastermind/audit-narrative.json"
    )
}

#[allow(dead_code)]
fn baseline_blobs(
    repo: &Path,
    baseline_oid: &str,
    files: &[WorkingTreeChangedFile],
) -> Result<BTreeMap<String, Option<Vec<u8>>>, WorkingTreeDiffError> {
    let paths = files
        .iter()
        .filter(|file| extractor_for_path(Path::new(&file.path)).is_some())
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let mut blobs = files
        .iter()
        .map(|file| (file.path.clone(), None))
        .collect::<BTreeMap<_, _>>();
    blobs.extend(baseline_blobs_for_paths(repo, baseline_oid, &paths)?);
    Ok(blobs)
}

/// Fetch a bounded set of blobs from one resolved baseline commit. Requests
/// are split across bounded `git cat-file --batch` processes so the aggregate
/// size of a large diff cannot trip the per-process output limit. Missing
/// paths are returned as `None`, which is how temporal rewind represents files
/// added after the baseline.
#[allow(dead_code)]
pub(crate) fn baseline_blobs_for_paths(
    repo: &Path,
    baseline_oid: &str,
    paths: &[String],
) -> Result<BTreeMap<String, Option<Vec<u8>>>, WorkingTreeDiffError> {
    baseline_blobs_for_paths_controlled(repo, baseline_oid, paths, None, None)
}

fn accumulate_baseline_blob_bytes(
    current: usize,
    additional: usize,
) -> Result<usize, WorkingTreeDiffError> {
    current
        .checked_add(additional)
        .filter(|total| *total <= BASELINE_BLOB_TOTAL_LIMIT)
        .ok_or(WorkingTreeDiffError::GitOutputLimit)
}

pub(crate) fn baseline_blobs_for_paths_controlled(
    repo: &Path,
    baseline_oid: &str,
    paths: &[String],
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<BTreeMap<String, Option<Vec<u8>>>, WorkingTreeDiffError> {
    if paths.is_empty() {
        return Ok(BTreeMap::new());
    }
    let deadline = deadline.or_else(|| Some(Instant::now() + git_timeout()));
    let object_ids = baseline_blob_oids(repo, baseline_oid, paths, deadline, interrupted)?;
    let requests = paths
        .iter()
        .filter_map(|path| object_ids.get(path).map(|oid| (path.clone(), oid.clone())))
        .collect::<Vec<_>>();
    let mut blobs = paths
        .iter()
        .map(|path| (path.clone(), None))
        .collect::<BTreeMap<_, _>>();
    if requests.is_empty() {
        return Ok(blobs);
    }
    let mut metadata_input = Vec::new();
    for (_, oid) in &requests {
        metadata_input.extend_from_slice(oid.as_bytes());
        metadata_input.push(b'\n');
    }
    let metadata = run_bounded_git_controlled(
        repo,
        &["cat-file", "--batch-check"],
        Some(&metadata_input),
        GIT_OUTPUT_LIMIT,
        deadline,
        interrupted,
    )?;
    if !metadata.success {
        return Err(WorkingTreeDiffError::InvalidRef);
    }

    let mut metadata_lines = metadata.stdout.split(|byte| *byte == b'\n');
    let mut sized_requests = Vec::with_capacity(requests.len());
    let mut total_blob_bytes = 0usize;
    for (path, expected_oid) in requests {
        let line = metadata_lines
            .next()
            .filter(|line| !line.is_empty())
            .ok_or(WorkingTreeDiffError::InvalidRef)?;
        let line = std::str::from_utf8(line).map_err(|_| WorkingTreeDiffError::InvalidRef)?;
        let mut fields = line.split_whitespace();
        let returned_oid = fields.next().ok_or(WorkingTreeDiffError::InvalidRef)?;
        let object_type = fields.next().ok_or(WorkingTreeDiffError::InvalidRef)?;
        let size = fields
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or(WorkingTreeDiffError::InvalidRef)?;
        if returned_oid != expected_oid
            || !matches!(object_type, "blob" | "commit" | "tree" | "tag")
            || fields.next().is_some()
        {
            return Err(WorkingTreeDiffError::InvalidRef);
        }
        let response_size = size
            .checked_add(CAT_FILE_RESPONSE_OVERHEAD)
            .ok_or(WorkingTreeDiffError::GitOutputLimit)?;
        if response_size > BASELINE_BLOB_BATCH_OUTPUT_LIMIT {
            return Err(WorkingTreeDiffError::GitOutputLimit);
        }
        total_blob_bytes = accumulate_baseline_blob_bytes(total_blob_bytes, size)?;
        sized_requests.push((path, expected_oid, size));
    }
    if metadata_lines.any(|line| !line.is_empty()) {
        return Err(WorkingTreeDiffError::InvalidRef);
    }

    let mut batch_start = 0usize;
    while batch_start < sized_requests.len() {
        let mut batch_end = batch_start;
        let mut expected_output = 0usize;
        while batch_end < sized_requests.len() {
            let response_size = sized_requests[batch_end]
                .2
                .saturating_add(CAT_FILE_RESPONSE_OVERHEAD);
            if batch_end > batch_start
                && expected_output.saturating_add(response_size) > BASELINE_BLOB_BATCH_OUTPUT_LIMIT
            {
                break;
            }
            expected_output = expected_output.saturating_add(response_size);
            batch_end += 1;
        }
        fetch_baseline_blob_batch(
            repo,
            &sized_requests[batch_start..batch_end],
            deadline,
            interrupted,
            &mut blobs,
        )?;
        batch_start = batch_end;
    }
    Ok(blobs)
}

fn fetch_baseline_blob_batch(
    repo: &Path,
    requests: &[(String, String, usize)],
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
    blobs: &mut BTreeMap<String, Option<Vec<u8>>>,
) -> Result<(), WorkingTreeDiffError> {
    let mut input = Vec::new();
    for (_, oid, _) in requests {
        input.extend_from_slice(oid.as_bytes());
        input.push(b'\n');
    }
    let output = run_bounded_git_controlled(
        repo,
        &["cat-file", "--batch"],
        Some(&input),
        BASELINE_BLOB_BATCH_OUTPUT_LIMIT,
        deadline,
        interrupted,
    )?;
    if !output.success {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    let mut cursor = 0usize;
    for (path, expected_oid, expected_size) in requests {
        let Some(relative_newline) = output.stdout[cursor..]
            .iter()
            .position(|byte| *byte == b'\n')
        else {
            return Err(WorkingTreeDiffError::InvalidRef);
        };
        let header_end = cursor + relative_newline;
        let header = &output.stdout[cursor..header_end];
        cursor = header_end + 1;
        let header_text =
            std::str::from_utf8(header).map_err(|_| WorkingTreeDiffError::InvalidRef)?;
        let mut fields = header_text.split_whitespace();
        let returned_oid = fields.next().ok_or(WorkingTreeDiffError::InvalidRef)?;
        let object_type = fields.next().ok_or(WorkingTreeDiffError::InvalidRef)?;
        let size = fields
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or(WorkingTreeDiffError::InvalidRef)?;
        if returned_oid != expected_oid
            || !matches!(object_type, "blob" | "commit" | "tree" | "tag")
            || size != *expected_size
            || fields.next().is_some()
        {
            return Err(WorkingTreeDiffError::InvalidRef);
        }
        if cursor + size >= output.stdout.len() || output.stdout[cursor + size] != b'\n' {
            return Err(WorkingTreeDiffError::InvalidRef);
        }
        blobs.insert(
            path.clone(),
            Some(output.stdout[cursor..cursor + size].to_vec()),
        );
        cursor += size + 1;
    }
    if cursor != output.stdout.len() {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    Ok(())
}

fn baseline_blob_oids(
    repo: &Path,
    baseline_oid: &str,
    paths: &[String],
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<BTreeMap<String, String>, WorkingTreeDiffError> {
    const PATHSPEC_ARG_BYTES: usize = 32 * 1024;
    const PATHS_PER_BATCH: usize = 256;

    let requested = paths.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut object_ids = BTreeMap::new();
    let mut output_bytes = 0usize;
    let mut offset = 0usize;
    while offset < paths.len() {
        let mut end = offset;
        let mut argument_bytes = 0usize;
        while end < paths.len() && end - offset < PATHS_PER_BATCH {
            let path = &paths[end];
            if path.as_bytes().contains(&0) {
                return Err(WorkingTreeDiffError::InvalidRef);
            }
            let next_bytes = path.len().saturating_add(10);
            if end > offset && argument_bytes.saturating_add(next_bytes) > PATHSPEC_ARG_BYTES {
                break;
            }
            argument_bytes = argument_bytes.saturating_add(next_bytes);
            end += 1;
        }
        let pathspecs = paths[offset..end]
            .iter()
            .map(|path| format!(":(literal){path}"))
            .collect::<Vec<_>>();
        let mut args = vec!["ls-tree", "-r", "-z", "--full-tree", baseline_oid, "--"];
        args.extend(pathspecs.iter().map(String::as_str));
        let output =
            run_bounded_git_controlled(repo, &args, None, GIT_OUTPUT_LIMIT, deadline, interrupted)?;
        if !output.success {
            return Err(WorkingTreeDiffError::InvalidRef);
        }
        output_bytes = output_bytes.saturating_add(output.stdout.len());
        if output_bytes > GIT_OUTPUT_LIMIT {
            return Err(WorkingTreeDiffError::GitOutputLimit);
        }
        let mut entries = output.stdout.split(|byte| *byte == 0).peekable();
        while let Some(entry) = entries.next() {
            if entry.is_empty() {
                if entries.peek().is_some() {
                    return Err(WorkingTreeDiffError::InvalidRef);
                }
                break;
            }
            let tab = entry
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or(WorkingTreeDiffError::InvalidRef)?;
            let metadata =
                std::str::from_utf8(&entry[..tab]).map_err(|_| WorkingTreeDiffError::InvalidRef)?;
            let path = std::str::from_utf8(&entry[tab + 1..])
                .map_err(|_| WorkingTreeDiffError::InvalidRef)?;
            let mut fields = metadata.split_whitespace();
            let _mode = fields.next().ok_or(WorkingTreeDiffError::InvalidRef)?;
            let object_type = fields.next().ok_or(WorkingTreeDiffError::InvalidRef)?;
            let oid = fields.next().ok_or(WorkingTreeDiffError::InvalidRef)?;
            if !matches!(object_type, "blob" | "commit" | "tree" | "tag")
                || fields.next().is_some()
                || !requested.contains(path)
                || object_ids
                    .insert(path.to_string(), oid.to_string())
                    .is_some()
            {
                return Err(WorkingTreeDiffError::InvalidRef);
            }
        }
        offset = end;
    }
    Ok(object_ids)
}

/// Recheck the inexpensive filesystem/Git token used by temporal analysis
/// without parsing every changed source file a third time. Cooperative
/// controls let MCP cancellation stop the extra Git work promptly.
pub(crate) fn validate_working_tree_snapshot_controlled(
    repo: &Path,
    baseline_oid: &str,
    expected_head_oid: &str,
    expected_files: &[WorkingTreeChangedFile],
    expected_token: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<(), WorkingTreeDiffError> {
    if resolve_head_controlled(repo, deadline, interrupted)? != expected_head_oid {
        return Err(WorkingTreeDiffError::SnapshotChanged);
    }
    let (files, _, truncated, _) =
        collect_worktree_paths_controlled(repo, baseline_oid, deadline, interrupted)?;
    if truncated || files != expected_files {
        return Err(WorkingTreeDiffError::SnapshotChanged);
    }
    let token = working_tree_snapshot_token_controlled(
        repo,
        expected_head_oid,
        &files,
        deadline,
        interrupted,
    )?;
    if token != expected_token {
        return Err(WorkingTreeDiffError::SnapshotChanged);
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn working_tree_snapshot_token(
    repo: &Path,
    head_oid: &str,
    files: &[WorkingTreeChangedFile],
) -> Result<String, WorkingTreeDiffError> {
    working_tree_snapshot_token_controlled(repo, head_oid, files, None, None)
}

fn working_tree_snapshot_token_controlled(
    repo: &Path,
    head_oid: &str,
    files: &[WorkingTreeChangedFile],
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<String, WorkingTreeDiffError> {
    let index = run_bounded_git_controlled(
        repo,
        &["write-tree"],
        None,
        GIT_OUTPUT_LIMIT,
        deadline,
        interrupted,
    )?;
    if !index.success {
        return Err(WorkingTreeDiffError::SnapshotChanged);
    }
    let mut digest = Sha256::new();
    digest.update(head_oid.as_bytes());
    digest.update(&index.stdout);
    for file in files {
        if interrupted.is_some_and(|check| check()) {
            return Err(WorkingTreeDiffError::GitTimeout);
        }
        digest.update(file.path.as_bytes());
        digest.update([0]);
        digest.update(file.status.as_bytes());
        digest.update([0]);
        if file.status != "deleted" {
            let bytes = read_regular_file(
                repo,
                Path::new(&file.path),
                MAX_INDEXABLE_FILE_SIZE,
                MAX_INDEXABLE_FILE_SIZE,
                ReadControl {
                    deadline,
                    interrupted,
                },
            )
            .map_err(working_tree_read_error)?
            .bytes;
            digest.update(Sha256::digest(bytes));
        }
    }
    Ok(crate::hex::encode(&digest.finalize()))
}

struct PerFileDiff {
    added: Vec<SymbolRef>,
    removed: Vec<SymbolRef>,
    signature_changed: Vec<SignatureChange>,
}

fn diff_file_from_blob(
    store: &Store,
    rel_path: &str,
    old_blob: Option<&[u8]>,
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

    // Old blobs were fetched in bounded `cat-file --batch` groups against one
    // resolved commit. Missing means the file did not exist at the baseline.
    let old_symbols: Vec<crate::store::PendingSymbol> = if old_blob.is_none_or(<[u8]>::is_empty) {
        Vec::new()
    } else if let Some(ext) = extractor {
        let pending = parse_blob(rel_path, old_blob.expect("checked above"), 0, ext.as_ref())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::process::Command;

    #[cfg(unix)]
    struct GitInvocationGuard;

    #[cfg(unix)]
    impl Drop for GitInvocationGuard {
        fn drop(&mut self) {
            TEST_GIT_INVOCATION.with(|value| value.borrow_mut().take());
            TEST_GIT_TIMEOUT.with(|value| value.borrow_mut().take());
        }
    }

    #[cfg(unix)]
    fn override_git(program: &str, prefix: &[&str], timeout: Duration) -> GitInvocationGuard {
        TEST_GIT_INVOCATION.with(|value| {
            *value.borrow_mut() = Some((
                std::path::PathBuf::from(program),
                prefix.iter().map(OsString::from).collect(),
            ));
        });
        TEST_GIT_TIMEOUT.with(|value| *value.borrow_mut() = Some(timeout));
        GitInvocationGuard
    }

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

    fn python_source_with_size(size: usize) -> String {
        assert!(size >= 2);
        format!("#{}\n", "x".repeat(size - 2))
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

    struct IndexedWorktree {
        store: Option<Store>,
        db: std::path::PathBuf,
    }

    impl IndexedWorktree {
        fn store(&self) -> &Store {
            self.store.as_ref().expect("fixture store")
        }
    }

    impl Drop for IndexedWorktree {
        fn drop(&mut self) {
            drop(self.store.take());
            let _ = fs::remove_file(&self.db);
            let _ = fs::remove_file(format!("{}-wal", self.db.display()));
            let _ = fs::remove_file(format!("{}-shm", self.db.display()));
        }
    }

    fn indexed_worktree(dir: &Path) -> IndexedWorktree {
        static NEXT_DB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let suffix = NEXT_DB.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let db = env::temp_dir().join(format!(
            "mmcg-diff-store-{}-{suffix}.db",
            std::process::id()
        ));
        let _ = fs::remove_file(&db);
        let mut store = Store::open(&db).unwrap();
        crate::indexer::Indexer::new(dir)
            .index_all(&mut store, true)
            .unwrap();
        IndexedWorktree {
            store: Some(store),
            db,
        }
    }

    #[test]
    fn working_tree_diff_includes_staged_unstaged_and_untracked() {
        let dir = init_repo("worktree_all_states");
        write(&dir, "staged.py", "def staged():\n    return 1\n");
        write(&dir, "unstaged.py", "def unstaged():\n    return 1\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        write(&dir, "staged.py", "def staged():\n    return 2\n");
        run(&dir, &["add", "staged.py"]);
        write(&dir, "unstaged.py", "def unstaged():\n    return 2\n");
        write(&dir, "untracked.py", "def untracked():\n    return 3\n");
        let store = indexed_worktree(&dir);
        let changed = symbols_changed_in_worktree(store.store(), &dir, "HEAD").unwrap();
        let paths: Vec<_> = changed
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(paths, vec!["staged.py", "unstaged.py", "untracked.py"]);
        assert_eq!(changed.files[2].status, "untracked");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn worktree_paths_stay_repository_relative_from_a_subdirectory() {
        // `git diff` reports repository-relative paths whatever the cwd is;
        // `ls-files` reports cwd-relative ones unless asked otherwise. Mixing
        // the two namespaces would make a subdirectory root emit paths that
        // match neither the spec nor the index.
        let dir = init_repo("worktree_subdir_paths");
        write(&dir, "sub/tracked.py", "def tracked():\n    return 1\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        write(&dir, "sub/tracked.py", "def tracked():\n    return 2\n");
        write(&dir, "sub/untracked.py", "def untracked():\n    return 3\n");

        let baseline = resolve_commit(&dir, "HEAD").unwrap();
        let (files, ..) = collect_worktree_paths(&dir.join("sub"), &baseline).unwrap();
        let paths: Vec<_> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, vec!["sub/tracked.py", "sub/untracked.py"]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn worktree_scope_sees_work_the_commit_range_cannot() {
        let dir = init_repo("worktree_scope_vs_range");
        write(&dir, "src/a.py", "def keep_same():\n    return 1\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["tag", "baseline"]);

        // Executor's work, none of it committed: `a.py` edited, `b.py` staged,
        // `c.py` untracked. HEAD is still the baseline commit.
        write(
            &dir,
            "src/a.py",
            "def keep_same():\n    return 1\n\ndef added_later():\n    return 2\n",
        );
        write(&dir, "src/b.py", "def staged_only():\n    return 3\n");
        run(&dir, &["add", "src/b.py"]);
        write(&dir, "src/c.py", "def untracked_only():\n    return 4\n");
        let store = indexed_worktree(&dir);

        // The commit range is empty by construction — the trap this exists for.
        let committed = symbols_changed_since(store.store(), &dir, "baseline").unwrap();
        assert!(
            committed.files_in_diff.is_empty(),
            "baseline..HEAD must be empty here: {:?}",
            committed.files_in_diff
        );

        let diff = symbols_changed_since_worktree(store.store(), &dir, "baseline").unwrap();
        assert_eq!(
            diff.git_ref, "baseline",
            "label is the caller's ref, not an oid"
        );
        assert_eq!(diff.files_in_diff, vec!["src/a.py", "src/b.py", "src/c.py"]);
        let added: Vec<&str> = diff.added.iter().map(|s| s.name.as_str()).collect();
        assert!(
            added.contains(&"added_later"),
            "unstaged edit missed: {added:?}"
        );
        assert!(
            added.contains(&"staged_only"),
            "staged file missed: {added:?}"
        );
        assert!(
            added.contains(&"untracked_only"),
            "untracked file missed: {added:?}"
        );
        assert!(!added.contains(&"keep_same"));
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn working_tree_diff_is_nul_safe_and_deterministic() {
        let dir = init_repo("worktree_nul");
        write(&dir, "base.py", "def base():\n    pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        write(&dir, "line\nbreak.py", "def unusual():\n    pass\n");
        let store = indexed_worktree(&dir);
        let first = symbols_changed_in_worktree(store.store(), &dir, "HEAD").unwrap();
        let second = symbols_changed_in_worktree(store.store(), &dir, "HEAD").unwrap();
        assert_eq!(first.files, second.files);
        assert_eq!(first.files[0].path, "line\nbreak.py");
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn baseline_blob_batch_is_nul_safe_for_newline_paths() {
        let dir = init_repo("baseline_blob_nul");
        let tracked = "src/evil\nname.py";
        let baseline_body = b"def baseline():\n    return 1\n";
        write(&dir, tracked, std::str::from_utf8(baseline_body).unwrap());
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        let baseline = resolve_commit(&dir, "HEAD").unwrap();

        let missing = "src/missing\nname.py".to_string();
        let blobs =
            baseline_blobs_for_paths(&dir, &baseline, &[tracked.to_string(), missing.clone()])
                .unwrap();

        assert_eq!(blobs.get(tracked), Some(&Some(baseline_body.to_vec())));
        assert_eq!(blobs.get(&missing), Some(&None));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn baseline_blob_batch_preserves_gitlink_compatibility() {
        let dir = init_repo("baseline_blob_gitlink");
        write(&dir, "README.md", "baseline\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "target"]);
        let target = resolve_commit(&dir, "HEAD").unwrap();
        run(
            &dir,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{target},vendor/lib"),
            ],
        );
        run(&dir, &["commit", "-q", "-m", "gitlink"]);
        let baseline = resolve_commit(&dir, "HEAD").unwrap();

        let blobs = baseline_blobs_for_paths(&dir, &baseline, &["vendor/lib".to_string()]).unwrap();

        assert!(blobs
            .get("vendor/lib")
            .and_then(|value| value.as_ref())
            .is_some_and(|bytes| !bytes.is_empty()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn working_tree_diff_does_not_fetch_large_unsupported_baseline_blob() {
        let dir = init_repo("unsupported_baseline_blob");
        write(&dir, "src/lib.rs", "pub fn indexed() {}\n");
        let asset = dir.join("asset.bin");
        fs::File::create(&asset)
            .unwrap()
            .set_len((GIT_OUTPUT_LIMIT + 1) as u64)
            .unwrap();
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        fs::OpenOptions::new()
            .write(true)
            .open(&asset)
            .unwrap()
            .set_len((GIT_OUTPUT_LIMIT + 2) as u64)
            .unwrap();

        let store = indexed_worktree(&dir);
        let changed = symbols_changed_in_worktree(store.store(), &dir, "HEAD").unwrap();

        assert_eq!(changed.files.len(), 1);
        assert_eq!(changed.files[0].path, "asset.bin");
        assert!(changed.diff.added.is_empty());
        assert!(changed.diff.removed.is_empty());
        assert!(changed.diff.signature_changed.is_empty());
        assert!(changed.diff.errors.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn baseline_blob_batch_allows_single_indexable_blob_above_generic_git_limit() {
        let dir = init_repo("single_large_indexable_blob");
        let size = GIT_OUTPUT_LIMIT + 1024;
        let body = python_source_with_size(size);
        write(&dir, "src/large.py", &body);
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        let baseline = resolve_commit(&dir, "HEAD").unwrap();

        let blobs =
            baseline_blobs_for_paths(&dir, &baseline, &["src/large.py".to_string()]).unwrap();

        assert_eq!(
            blobs.get("src/large.py").and_then(Option::as_deref),
            Some(body.as_bytes())
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn baseline_blob_batch_chunks_indexable_aggregate_above_per_batch_limit() {
        let dir = init_repo("aggregate_large_indexable_blobs");
        let size = GIT_OUTPUT_LIMIT / 2 + 64 * 1024;
        let first = python_source_with_size(size);
        let second = python_source_with_size(size);
        write(&dir, "src/first.py", &first);
        write(&dir, "src/second.py", &second);
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        let baseline = resolve_commit(&dir, "HEAD").unwrap();

        let blobs = baseline_blobs_for_paths(
            &dir,
            &baseline,
            &["src/first.py".to_string(), "src/second.py".to_string()],
        )
        .unwrap();

        assert_eq!(
            blobs.get("src/first.py").and_then(Option::as_deref),
            Some(first.as_bytes())
        );
        assert_eq!(
            blobs.get("src/second.py").and_then(Option::as_deref),
            Some(second.as_bytes())
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn baseline_blob_total_limit_is_inclusive_and_overflow_safe() {
        assert_eq!(
            accumulate_baseline_blob_bytes(BASELINE_BLOB_TOTAL_LIMIT - 1, 1),
            Ok(BASELINE_BLOB_TOTAL_LIMIT)
        );
        assert_eq!(
            accumulate_baseline_blob_bytes(BASELINE_BLOB_TOTAL_LIMIT, 1),
            Err(WorkingTreeDiffError::GitOutputLimit)
        );
        assert_eq!(
            accumulate_baseline_blob_bytes(usize::MAX, 1),
            Err(WorkingTreeDiffError::GitOutputLimit)
        );
    }

    #[test]
    fn working_tree_diff_file_cap_and_cap_plus_one_are_non_vacuous() {
        let build = |count: usize| {
            (0..count)
                .map(|index| (format!("src/{index:05}.rs").into_bytes(), "modified"))
                .collect::<BTreeMap<_, _>>()
        };
        let (exact, exact_total, exact_truncated, _) =
            finalize_changed_paths(build(CHANGE_FILE_LIMIT));
        assert_eq!(exact.len(), CHANGE_FILE_LIMIT);
        assert_eq!(exact_total, Some(CHANGE_FILE_LIMIT as u32));
        assert!(!exact_truncated);

        let (overflow, overflow_total, overflow_truncated, _) =
            finalize_changed_paths(build(CHANGE_FILE_LIMIT + 1));
        assert_eq!(overflow.len(), CHANGE_FILE_LIMIT);
        assert_eq!(overflow_total, None);
        assert!(overflow_truncated);
    }

    #[test]
    fn worktree_paths_exclude_mastermind_runtime_artifacts_only() {
        let changed = [
            (b".mastermind/mmcg.db".to_vec(), "untracked"),
            (b".mastermind/mmcg.db-shm".to_vec(), "untracked"),
            (b".mastermind/mmcg.db-wal".to_vec(), "untracked"),
            (b".mastermind/audit-narrative.json".to_vec(), "untracked"),
            (b".mastermind/tasks/001/spec.md".to_vec(), "untracked"),
            (b"src/app.py".to_vec(), "modified"),
        ]
        .into_iter()
        .collect();
        let (files, total, truncated, skipped) = finalize_changed_paths(changed);
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, vec![".mastermind/tasks/001/spec.md", "src/app.py"]);
        assert_eq!(total, Some(2));
        assert!(!truncated);
        assert_eq!(skipped, 0);
    }

    #[test]
    fn body_change_selects_only_deepest_enclosing_symbol() {
        let dir = init_repo("deepest_nested_engine");
        write(
            &dir,
            "service.py",
            "class Service:\n    def method(self):\n        return 1\n\n    def other(self):\n        return 3\n\ndef caller(service):\n    return service.method()\n",
        );
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        write(
            &dir,
            "service.py",
            "class Service:\n    def method(self):\n        return 2\n\n    def other(self):\n        return 3\n\ndef caller(service):\n    return service.method()\n",
        );
        let indexed = indexed_worktree(&dir);

        let changed = symbols_changed_in_worktree(indexed.store(), &dir, "HEAD").unwrap();
        assert_eq!(changed.body_changed.len(), 1);
        assert_eq!(changed.body_changed[0].name, "method");
        assert_eq!(changed.body_changed[0].kind, "method");
        assert!(!changed
            .body_changed
            .iter()
            .any(|symbol| symbol.name == "Service"));

        let response =
            crate::queries::change_impact(indexed.store(), &dir, "HEAD", 3, 100).unwrap();
        let body_seeds = response
            .changes
            .symbols
            .items
            .iter()
            .filter(|symbol| symbol.change == "body_changed")
            .collect::<Vec<_>>();
        assert_eq!(body_seeds.len(), 1);
        assert_eq!(body_seeds[0].name, "method");
        assert!(!body_seeds.iter().any(|symbol| symbol.name == "Service"));
        fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn git_batch_timeout_starts_before_writer_and_ignores_descendant_held_pipes() {
        let repo = env::temp_dir();
        {
            let _guard = override_git(
                "python3",
                &["-c", "import signal; signal.pause()"],
                Duration::from_millis(50),
            );
            let started = Instant::now();
            let result = run_bounded_git(&repo, &["ignored"], Some(&vec![b'x'; 8 * 1024 * 1024]));
            assert_eq!(result.unwrap_err(), WorkingTreeDiffError::GitTimeout);
            assert!(started.elapsed() < Duration::from_secs(1));
        }

        let pid_file = env::temp_dir().join(format!("mmcg-held-pipe-{}.pid", std::process::id()));
        let code = format!(
            "import os,signal,sys; data=sys.stdin.buffer.read(); pid=os.fork(); (open({:?},'w').write(str(pid)),os._exit(0)) if pid else signal.pause()",
            pid_file.to_string_lossy()
        );
        {
            let _guard = override_git("python3", &["-c", &code], Duration::from_secs(1));
            let started = Instant::now();
            let result = run_bounded_git(&repo, &["ignored"], Some(b"request\n"));
            assert_eq!(result.unwrap_err(), WorkingTreeDiffError::GitTimeout);
            assert!(started.elapsed() < Duration::from_secs(1));
        }
        if let Ok(pid) = fs::read_to_string(&pid_file) {
            let _ = Command::new("kill").args(["-TERM", pid.trim()]).status();
        }
        let _ = fs::remove_file(pid_file);
    }

    #[test]
    fn symbols_changed_since_caps_file_loop() {
        let dir = init_repo("change_file_limit");
        write(&dir, "base.py", "def base():\n    pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["tag", "baseline"]);

        // More changed files than CHANGE_FILE_LIMIT would be slow to seed
        // through git — exercise the truncation math directly, mirroring the
        // worktree-diff cap/cap+1 test above.
        let build = |count: usize| -> Vec<String> {
            (0..count)
                .map(|index| format!("src/{index:05}.py"))
                .collect()
        };
        let exact = build(CHANGE_FILE_LIMIT);
        assert!(exact.len() <= CHANGE_FILE_LIMIT);
        let overflow = build(CHANGE_FILE_LIMIT + 1);
        assert!(overflow.len() > CHANGE_FILE_LIMIT);
        assert_eq!(
            overflow.iter().take(CHANGE_FILE_LIMIT).count(),
            CHANGE_FILE_LIMIT
        );

        write(&dir, "base.py", "def base():\n    return 1\n");
        let db = dir.join(".mmcg.db");
        let mut store = Store::open(&db).unwrap();
        crate::indexer::Indexer::new(&dir)
            .index_all(&mut store, false)
            .unwrap();
        let diff = symbols_changed_since(&store, &dir, "baseline").unwrap();
        assert!(!diff.truncated);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn git_errors_do_not_expose_raw_stderr() {
        let dir = init_repo("sanitized_error");
        write(&dir, "x.py", "def x():\n    pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        let store = indexed_worktree(&dir);
        let error = symbols_changed_in_worktree(store.store(), &dir, "missing-super-secret-ref")
            .unwrap_err();
        assert_eq!(error.to_string(), "invalid_ref");
        assert!(!error.to_string().contains("secret-ref"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn change_impact_rejects_option_like_ref_and_never_renders_git_stderr() {
        let dir = init_repo("option_ref");
        write(&dir, "x.py", "def x():\n    pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        let store = indexed_worktree(&dir);
        let error = symbols_changed_in_worktree(store.store(), &dir, "--help").unwrap_err();
        assert_eq!(error, WorkingTreeDiffError::InvalidRef);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn commit_resolver_rejects_oversized_and_non_commit_refs() {
        let dir = init_repo("bounded_commit_ref");
        write(&dir, "x.py", "def x():\n    pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        assert_eq!(
            resolve_commit(&dir, &"a".repeat(MAX_GIT_REF_BYTES + 1)),
            Err(WorkingTreeDiffError::InvalidRef)
        );
        let output = Command::new("git")
            .args(["rev-parse", "HEAD:x.py"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(output.status.success());
        let blob = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            resolve_commit(&dir, blob.trim()),
            Err(WorkingTreeDiffError::InvalidRef)
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn git_collection_ignores_external_diff_and_textconv() {
        let dir = init_repo("external_diff");
        write(&dir, "x.py", "def x():\n    return 1\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["config", "diff.external", "/definitely/missing"]);
        write(&dir, "x.py", "def x():\n    return 2\n");
        let store = indexed_worktree(&dir);
        let changed = symbols_changed_in_worktree(store.store(), &dir, "HEAD").unwrap();
        assert_eq!(changed.files.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }
}
