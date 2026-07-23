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

use crate::indexer::{extractor_for_path, parse_blob};
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
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Default `run_git` / `git_show_blob` subprocess deadline when
/// `MMCG_GIT_TIMEOUT_MS` is unset — distinct from `GIT_TIMEOUT` above, which
/// bounds the separate `run_bounded_git` worktree-diff path.
const DEFAULT_RUN_GIT_TIMEOUT_MS: u64 = 30_000;
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
    static TEST_RUN_GIT_TIMEOUT: RefCell<Option<Duration>> = const { RefCell::new(None) };
}

fn git_timeout() -> Duration {
    #[cfg(test)]
    if let Some(timeout) = TEST_GIT_TIMEOUT.with(|value| *value.borrow()) {
        return timeout;
    }
    GIT_TIMEOUT
}

/// Deadline for `run_git` / `git_show_blob` — read from `MMCG_GIT_TIMEOUT_MS`
/// (default 30,000ms) on every call so it can't go stale within a long-lived
/// process; test-overridable via the same pattern as `git_timeout`.
fn run_git_timeout() -> Duration {
    #[cfg(test)]
    if let Some(timeout) = TEST_RUN_GIT_TIMEOUT.with(|value| *value.borrow()) {
        return timeout;
    }
    let ms = std::env::var("MMCG_GIT_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|&value| value > 0)
        .unwrap_or(DEFAULT_RUN_GIT_TIMEOUT_MS);
    Duration::from_millis(ms)
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
    validate_ref(repo_root, git_ref)?;

    let files_in_diff = git_diff_name_only(repo_root, git_ref)?;
    Ok(symbol_diff_over_files(
        store,
        repo_root,
        git_ref,
        git_ref,
        files_in_diff,
    ))
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
    let baseline_oid =
        resolve_commit(repo_root, git_ref).map_err(|e| worktree_scope_error(git_ref, e))?;
    let (files, _files_total, _truncated, _skipped_non_utf8) =
        collect_worktree_paths(repo_root, &baseline_oid)
            .map_err(|e| worktree_scope_error(git_ref, e))?;

    // Blobs come from the resolved oid so the old side can't drift if the ref
    // moves mid-audit; `git_ref` stays the caller-facing label.
    let files_in_diff = files.into_iter().map(|file| file.path).collect();
    Ok(symbol_diff_over_files(
        store,
        repo_root,
        &baseline_oid,
        git_ref,
        files_in_diff,
    ))
}

fn worktree_scope_error(git_ref: &str, error: WorkingTreeDiffError) -> DiffError {
    match error {
        WorkingTreeDiffError::InvalidRef => DiffError::GitRefMissing(git_ref.to_string()),
        other => DiffError::GitFailed(format!("worktree file scope: {other}")),
    }
}

/// `blob_ref` resolves the old side (`git show <blob_ref>:<path>`); `label` is
/// what the caller asked for and is echoed back in [`SymbolDiff::git_ref`].
fn symbol_diff_over_files(
    store: &Store,
    repo_root: &Path,
    blob_ref: &str,
    label: &str,
    files_in_diff: Vec<String>,
) -> SymbolDiff {
    let truncated = files_in_diff.len() > CHANGE_FILE_LIMIT;
    let mut added: Vec<SymbolRef> = Vec::new();
    let mut removed: Vec<SymbolRef> = Vec::new();
    let mut signature_changed: Vec<SignatureChange> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for rel in files_in_diff.iter().take(CHANGE_FILE_LIMIT) {
        match diff_file(store, repo_root, blob_ref, rel) {
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

    SymbolDiff {
        git_ref: label.to_string(),
        files_in_diff,
        added,
        removed,
        signature_changed,
        errors,
        truncated,
    }
}

pub fn symbols_changed_in_worktree(
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
) -> Result<WorkingTreeSymbolDiff, WorkingTreeDiffError> {
    let baseline_oid = resolve_commit(repo_root, git_ref)?;
    let head_oid = resolve_head(repo_root)?;
    let (files, files_total, files_truncated, skipped_non_utf8_paths) =
        collect_worktree_paths(repo_root, &baseline_oid)?;
    let snapshot_token = working_tree_snapshot_token(repo_root, &head_oid, &files)?;
    let old_blobs = baseline_blobs(repo_root, &baseline_oid, &files)?;

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

        let current_bytes = std::fs::read(repo_root.join(rel)).ok();
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
    let timeout = git_timeout();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
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

fn resolve_commit(repo: &Path, git_ref: &str) -> Result<String, WorkingTreeDiffError> {
    if git_ref.is_empty() || git_ref.starts_with('-') || git_ref.contains('\0') {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    let commit = format!("{git_ref}^{{commit}}");
    let output = run_bounded_git(
        repo,
        &["rev-parse", "--verify", "--end-of-options", &commit],
        None,
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
    let output = run_bounded_git(
        repo,
        &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
        None,
    )?;
    if !output.success {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    let oid = String::from_utf8(output.stdout).map_err(|_| WorkingTreeDiffError::InvalidRef)?;
    Ok(oid.trim().to_ascii_lowercase())
}

pub(crate) fn current_head_oid(repo: &Path) -> Result<String, WorkingTreeDiffError> {
    resolve_head(repo)
}

fn nul_fields(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
}

fn collect_worktree_paths(
    repo: &Path,
    baseline_oid: &str,
) -> Result<(Vec<WorkingTreeChangedFile>, Option<u32>, bool, u32), WorkingTreeDiffError> {
    let diff = run_bounded_git(
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
    let untracked = run_bounded_git(
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
    changed: BTreeMap<Vec<u8>, &'static str>,
) -> (Vec<WorkingTreeChangedFile>, Option<u32>, bool, u32) {
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

fn baseline_blobs(
    repo: &Path,
    baseline_oid: &str,
    files: &[WorkingTreeChangedFile],
) -> Result<BTreeMap<String, Option<Vec<u8>>>, WorkingTreeDiffError> {
    let mut input = Vec::new();
    for file in files {
        input.extend_from_slice(baseline_oid.as_bytes());
        input.push(b':');
        input.extend_from_slice(file.path.as_bytes());
        input.push(b'\n');
    }
    let output = run_bounded_git(repo, &["cat-file", "--batch"], Some(&input))?;
    if !output.success {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    let mut cursor = 0usize;
    let mut blobs = BTreeMap::new();
    for file in files {
        let Some(relative_newline) = output.stdout[cursor..]
            .iter()
            .position(|byte| *byte == b'\n')
        else {
            return Err(WorkingTreeDiffError::InvalidRef);
        };
        let header_end = cursor + relative_newline;
        let header = &output.stdout[cursor..header_end];
        cursor = header_end + 1;
        if header.ends_with(b" missing") {
            blobs.insert(file.path.clone(), None);
            continue;
        }
        let header_text =
            std::str::from_utf8(header).map_err(|_| WorkingTreeDiffError::InvalidRef)?;
        let size = header_text
            .split_whitespace()
            .nth(2)
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or(WorkingTreeDiffError::InvalidRef)?;
        if cursor + size >= output.stdout.len() {
            return Err(WorkingTreeDiffError::InvalidRef);
        }
        blobs.insert(
            file.path.clone(),
            Some(output.stdout[cursor..cursor + size].to_vec()),
        );
        cursor += size + 1;
    }
    Ok(blobs)
}

pub(crate) fn working_tree_snapshot_token(
    repo: &Path,
    head_oid: &str,
    files: &[WorkingTreeChangedFile],
) -> Result<String, WorkingTreeDiffError> {
    let index = run_bounded_git(repo, &["write-tree"], None)?;
    if !index.success {
        return Err(WorkingTreeDiffError::SnapshotChanged);
    }
    let mut digest = Sha256::new();
    digest.update(head_oid.as_bytes());
    digest.update(&index.stdout);
    for file in files {
        digest.update(file.path.as_bytes());
        digest.update([0]);
        digest.update(file.status.as_bytes());
        digest.update([0]);
        if file.status != "deleted" {
            let bytes = std::fs::read(repo.join(&file.path))
                .map_err(|_| WorkingTreeDiffError::SnapshotChanged)?;
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

/// Spawn `command`, poll it with `try_wait` while draining stdout/stderr on
/// background threads (so a full pipe can't masquerade as a hang), and kill
/// it if it hasn't exited by `deadline`.
fn run_with_deadline(
    mut command: Command,
    deadline: Duration,
) -> Result<std::process::Output, DiffError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let start = Instant::now();
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            DiffError::GitNotFound
        } else {
            DiffError::GitFailed(e.to_string())
        }
    })?;
    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| DiffError::GitFailed("no stdout pipe".to_string()))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| DiffError::GitFailed("no stderr pipe".to_string()))?;
    let stdout_rx = spawn_drain(stdout_pipe);
    let stderr_rx = spawn_drain(stderr_pipe);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < deadline => {
                std::thread::sleep(CHILD_POLL_INTERVAL);
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DiffError::GitTimeout);
            }
            Err(e) => return Err(DiffError::GitFailed(e.to_string())),
        }
    };
    let stdout = stdout_rx.recv().unwrap_or_default();
    let stderr = stderr_rx.recv().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn spawn_drain<R: Read + Send + 'static>(mut reader: R) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = reader.read_to_end(&mut buffer);
        let _ = sender.send(buffer);
    });
    receiver
}

fn run_git(repo: &Path, args: &[&str]) -> Result<std::process::Output, DiffError> {
    let mut command = git_command(args);
    command.current_dir(repo);
    run_with_deadline(command, run_git_timeout())
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
    let mut command = git_command(&["show", &spec]);
    command.current_dir(repo);
    let out = run_with_deadline(command, run_git_timeout()).map_err(|e| e.to_string())?;
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

    #[cfg(unix)]
    struct GitInvocationGuard;

    #[cfg(unix)]
    impl Drop for GitInvocationGuard {
        fn drop(&mut self) {
            TEST_GIT_INVOCATION.with(|value| value.borrow_mut().take());
            TEST_GIT_TIMEOUT.with(|value| value.borrow_mut().take());
            TEST_RUN_GIT_TIMEOUT.with(|value| value.borrow_mut().take());
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
        TEST_RUN_GIT_TIMEOUT.with(|value| *value.borrow_mut() = Some(timeout));
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

    #[cfg(unix)]
    #[test]
    fn git_timeout_kills_stuck_subprocess() {
        // The fake `git` is a PATH shim that sleeps past the deadline.
        let repo = env::temp_dir();
        let _guard = override_git(
            "python3",
            &["-c", "import signal; signal.pause()"],
            Duration::from_millis(50),
        );

        let started = Instant::now();
        let result = run_git(&repo, &["ignored"]);
        assert!(matches!(result, Err(DiffError::GitTimeout)));
        assert!(started.elapsed() < Duration::from_secs(1));

        let started = Instant::now();
        let result = git_show_blob(&repo, "HEAD", "ignored.py");
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
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
