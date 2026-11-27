//! Bounded Git subprocesses for every Mastermind read of a repository.
//!
//! Each command runs with repository-selected routing, hooks, filter drivers
//! and lazy fetches disabled, under one deadline and an output limit. On top of
//! the runner sit ref resolution, the changed-path inventory of a working tree,
//! baseline blob reads and the snapshot token that proves a working tree did
//! not change between two reads.

use crate::bounded_fs::{read_repository_file, BoundedReadError, ReadControl};
use crate::indexer::MAX_INDEXABLE_FILE_SIZE;
use serde::Serialize;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
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

/// Flagged entries of `ls-files -v`, the path plus three bytes each.
const FLAGGED_LISTING_LIMIT: usize = 64 * 1024 * 1024;

const DEFAULT_GIT_TIMEOUT_MS: u64 = 30_000;

const MAX_GIT_TIMEOUT_MS: u64 = 300_000;

const MAX_GIT_PATHSPEC_BYTES: usize = 4096;

/// Must stay a real `sleep`. `park_timeout` returns instantly when the thread
/// holds an unpark token, and the sibling drain threads talk over `mpsc`, which
/// parks and unparks this very thread — a stray token turns the wait into a
/// spin that burns a core until the git deadline expires.
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(5);

#[cfg(test)]
thread_local! {
    static TEST_GIT_INVOCATION: RefCell<Option<(std::path::PathBuf, Vec<OsString>)>> = const { RefCell::new(None) };
    static TEST_GIT_TIMEOUT: RefCell<Option<Duration>> = const { RefCell::new(None) };
}

pub(crate) fn git_timeout() -> Duration {
    #[cfg(test)]
    if let Some(timeout) = TEST_GIT_TIMEOUT.with(|value| *value.borrow()) {
        return timeout;
    }
    configured_git_timeout(std::env::var("MMCG_GIT_TIMEOUT_MS").ok().as_deref())
}

fn configured_git_timeout(value: Option<&str>) -> Duration {
    let millis = value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .unwrap_or(DEFAULT_GIT_TIMEOUT_MS)
        .min(MAX_GIT_TIMEOUT_MS);
    Duration::from_millis(millis)
}

/// Repository commands select their checkout with `current_dir`; ambient Git
/// routing from a hook or another worktree must not override that selection.
pub(crate) fn repository_git_command() -> Command {
    let mut command = Command::new("git");
    // The Docker Action supplies protected safe.directory entries through
    // Git's environment configuration because the checkout has another UID.
    // Preserve those trust decisions without restoring author/routing config.
    let config_count = std::env::var("GIT_CONFIG_COUNT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|count| *count <= 128)
        .unwrap_or(0);
    for index in 0..config_count {
        let key = std::env::var(format!("GIT_CONFIG_KEY_{index}"));
        if !key.is_ok_and(|key| key.eq_ignore_ascii_case("safe.directory")) {
            continue;
        }
        if let Some(value) = std::env::var_os(format!("GIT_CONFIG_VALUE_{index}")) {
            let mut setting = std::ffi::OsString::from("safe.directory=");
            setting.push(value);
            command.arg("-c").arg(setting);
        }
    }
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_QUARANTINE_PATH",
        "GIT_SHALLOW_FILE",
        "GIT_GRAFT_FILE",
        "GIT_REPLACE_REF_BASE",
        "GIT_NAMESPACE",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "GIT_PREFIX",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_NO_REPLACE_OBJECTS",
        "GIT_INTERNAL_SUPER_PREFIX",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env_remove(name);
    }
    // A read must not run a program named by the repository's own config. The
    // environment form reaches every caller without reordering their arguments.
    // Hooks point at a path with no hooks in it, because Git runs
    // `post-index-change` on any index write, including an opportunistic
    // refresh during `diff` or a write to a temporary index.
    command
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "core.fsmonitor")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "core.hooksPath")
        .env(
            "GIT_CONFIG_VALUE_1",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
    command
}

fn git_command(args: &[&str]) -> Command {
    #[cfg(test)]
    if let Some((program, prefix)) = TEST_GIT_INVOCATION.with(|value| value.borrow().clone()) {
        let mut command = Command::new(program);
        command.args(prefix).args(args);
        return command;
    }
    let mut command = repository_git_command();
    command.args(args);
    // Diff and baseline reads must interpret the same literal objects, and a
    // missing promisor object must not initiate a network fetch.
    command
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct WorkingTreeChangedFile {
    pub path: String,
    pub status: String,
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

/// Git repositories currently use full SHA-1 or SHA-256 object IDs. Keep one
/// lowercase validator for persisted revision bindings across the product.
pub(crate) fn is_full_git_oid(value: &str) -> bool {
    crate::hex::is_lower(value, 40) || crate::hex::is_lower(value, 64)
}

fn normalized_git_oid(bytes: Vec<u8>) -> Result<String, WorkingTreeDiffError> {
    let oid = String::from_utf8(bytes).map_err(|_| WorkingTreeDiffError::InvalidRef)?;
    let oid = oid.trim().to_ascii_lowercase();
    if !is_full_git_oid(&oid) {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    Ok(oid)
}

pub(crate) fn working_tree_read_error(error: BoundedReadError) -> WorkingTreeDiffError {
    match error {
        BoundedReadError::Interrupted | BoundedReadError::DeadlineExceeded => {
            WorkingTreeDiffError::GitTimeout
        }
        // Callers that read a source treat a file over the size cap like the
        // indexer does, as one it skips, before they get here.
        _ => WorkingTreeDiffError::SnapshotChanged,
    }
}

#[derive(Debug)]
pub(crate) struct BoundedGitOutput {
    pub(crate) success: bool,
    pub(crate) stdout: Vec<u8>,
}

fn bounded_reader<R: Read + Send + 'static>(
    reader: R,
    limit: usize,
) -> mpsc::Receiver<(Vec<u8>, bool)> {
    bounded_record_reader(reader, limit, None)
}

/// Reads up to `limit` bytes. With `retain`, NUL-terminated records it rejects
/// are dropped as they arrive and do not count against the limit.
fn bounded_record_reader<R: Read + Send + 'static>(
    mut reader: R,
    limit: usize,
    mut retain: Option<RecordFilter>,
) -> mpsc::Receiver<(Vec<u8>, bool)> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut retained = Vec::new();
        let mut record = Vec::new();
        let mut exceeded = false;
        let mut buffer = [0u8; 16 * 1024];
        let keep = |bytes: &[u8], retained: &mut Vec<u8>| {
            if retained.len() < limit + 1 {
                let count = bytes.len().min(limit + 1 - retained.len());
                retained.extend_from_slice(&bytes[..count]);
            }
            retained.len() > limit
        };
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => match retain.as_mut() {
                    None => exceeded |= keep(&buffer[..count], &mut retained),
                    Some(retain) => {
                        for chunk in buffer[..count].split_inclusive(|byte| *byte == 0) {
                            if record.len() <= limit {
                                record.extend_from_slice(chunk);
                            }
                            if chunk.ends_with(&[0]) {
                                if record.len() > limit || retain(&record[..record.len() - 1]) {
                                    exceeded |= keep(&record, &mut retained);
                                }
                                record.clear();
                            }
                        }
                    }
                },
                Err(_) => break,
            }
        }
        if retain.is_some_and(|mut retain| !record.is_empty() && retain(&record)) {
            exceeded |= keep(&record, &mut retained);
        }
        let _ = sender.send((retained, exceeded));
    });
    receiver
}

#[cfg(all(test, unix))]
fn run_bounded_git(
    repo: &Path,
    args: &[&str],
    input: Option<&[u8]>,
) -> Result<BoundedGitOutput, WorkingTreeDiffError> {
    run_bounded_git_controlled(repo, args, input, GIT_OUTPUT_LIMIT, None, None)
}

pub(crate) fn run_bounded_git_controlled(
    repo: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    output_limit: usize,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<BoundedGitOutput, WorkingTreeDiffError> {
    run_bounded_git_records(
        repo,
        git_command(args),
        input,
        output_limit,
        deadline,
        interrupted,
        None,
    )
}

/// With `retain`, only the stdout records it accepts are kept.
fn run_bounded_git_records(
    repo: &Path,
    mut command: Command,
    input: Option<&[u8]>,
    output_limit: usize,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
    retain: Option<RecordFilter>,
) -> Result<BoundedGitOutput, WorkingTreeDiffError> {
    let start = Instant::now();
    let mut child = command
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
    let stdout = bounded_record_reader(stdout_pipe, output_limit, retain);
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
    // Pipe workers can be scheduled after Git exits; keep the operation's deadline.
    let pipe_deadline = start + timeout;
    if let Some(writer) = writer {
        receive_git_pipe_until(pipe_deadline, interrupted, Instant::now, |remaining| {
            writer.recv_timeout(remaining)
        })?
        .map_err(|_| WorkingTreeDiffError::GitUnavailable)?;
    }
    let (stdout, stdout_exceeded) =
        receive_git_pipe_until(pipe_deadline, interrupted, Instant::now, |remaining| {
            stdout.recv_timeout(remaining)
        })?;
    let (_, stderr_exceeded) =
        receive_git_pipe_until(pipe_deadline, interrupted, Instant::now, |remaining| {
            stderr.recv_timeout(remaining)
        })?;
    if stdout_exceeded || stderr_exceeded {
        return Err(WorkingTreeDiffError::GitOutputLimit);
    }
    Ok(BoundedGitOutput {
        success: status.success(),
        stdout,
    })
}

fn receive_git_pipe_until<T>(
    deadline: Instant,
    interrupted: Option<&dyn Fn() -> bool>,
    mut now: impl FnMut() -> Instant,
    mut receive: impl FnMut(Duration) -> Result<T, mpsc::RecvTimeoutError>,
) -> Result<T, WorkingTreeDiffError> {
    loop {
        let remaining = deadline.saturating_duration_since(now());
        if remaining.is_zero() || interrupted.is_some_and(|check| check()) {
            return Err(WorkingTreeDiffError::GitTimeout);
        }
        match receive(remaining.min(CHILD_POLL_INTERVAL)) {
            Ok(output) => return Ok(output),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(WorkingTreeDiffError::GitTimeout);
            }
        }
    }
}

fn run_bounded_git_status_until(
    repo: &Path,
    args: &[&str],
    deadline: Instant,
) -> Result<bool, WorkingTreeDiffError> {
    let start = Instant::now();
    if start >= deadline {
        return Err(WorkingTreeDiffError::GitTimeout);
    }
    let mut child = git_command(args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| WorkingTreeDiffError::GitUnavailable)?;
    let timeout = git_timeout().min(deadline.saturating_duration_since(start));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(CHILD_POLL_INTERVAL);
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorkingTreeDiffError::GitTimeout);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorkingTreeDiffError::GitUnavailable);
            }
        }
    }
}

/// Run output-free, bounded existence probes against Git's tracked-file index.
/// Results retain the input order. A non-repository returns all `false`.
#[doc(hidden)]
pub fn tracked_pathspec_presence(
    repo: &Path,
    pathspecs: &[&str],
) -> Result<Vec<bool>, WorkingTreeDiffError> {
    if pathspecs.iter().any(|pathspec| {
        pathspec.is_empty() || pathspec.len() > MAX_GIT_PATHSPEC_BYTES || pathspec.contains('\0')
    }) {
        return Err(WorkingTreeDiffError::SnapshotChanged);
    }
    let deadline = Instant::now() + git_timeout();
    if !run_bounded_git_status_until(repo, &["rev-parse", "--git-dir"], deadline)? {
        return Ok(vec![false; pathspecs.len()]);
    }
    pathspecs
        .iter()
        .map(|pathspec| {
            run_bounded_git_status_until(
                repo,
                &[
                    "-c",
                    "core.fsmonitor=false",
                    "ls-files",
                    "--error-unmatch",
                    "--",
                    pathspec,
                ],
                deadline,
            )
        })
        .collect()
}

/// Return repository-relative paths changed between a validated commit and the
/// current HEAD under one literal repository prefix.
#[doc(hidden)]
pub fn changed_paths_under(
    repo: &Path,
    git_ref: &str,
    prefix: &str,
) -> Result<Vec<String>, WorkingTreeDiffError> {
    let prefix_path = Path::new(prefix);
    if prefix.is_empty()
        || prefix.len() > MAX_GIT_PATHSPEC_BYTES
        || prefix.contains('\0')
        || prefix_path.is_absolute()
        || prefix_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(WorkingTreeDiffError::SnapshotChanged);
    }
    let deadline = Some(Instant::now() + git_timeout());
    let baseline_oid = resolve_commit_controlled(repo, git_ref, deadline, None)?;
    let head_oid = resolve_head_controlled(repo, deadline, None)?;
    let literal_pathspec = format!(":(top,literal){prefix}");
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
            &baseline_oid,
            &head_oid,
            "--",
            &literal_pathspec,
        ],
        None,
        GIT_OUTPUT_LIMIT,
        deadline,
        None,
    )?;
    if !output.success {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    let mut paths = BTreeSet::new();
    for raw in nul_fields(&output.stdout) {
        let path = std::str::from_utf8(raw)
            .map_err(|_| WorkingTreeDiffError::SnapshotChanged)?
            .to_string();
        let parsed = Path::new(&path);
        if parsed.is_absolute()
            || parsed
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
            || !parsed.starts_with(prefix_path)
        {
            return Err(WorkingTreeDiffError::SnapshotChanged);
        }
        paths.insert(path);
    }
    Ok(paths.into_iter().collect())
}

#[cfg(test)]
fn resolve_commit(repo: &Path, git_ref: &str) -> Result<String, WorkingTreeDiffError> {
    resolve_commit_controlled(repo, git_ref, None, None)
}

pub(crate) fn resolve_commit_controlled(
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
    normalized_git_oid(output.stdout)
}

pub(crate) fn validate_commit_ref_controlled(
    repo: &Path,
    git_ref: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<(), WorkingTreeDiffError> {
    resolve_commit_controlled(repo, git_ref, deadline, interrupted).map(|_| ())
}

fn resolve_head(repo: &Path) -> Result<String, WorkingTreeDiffError> {
    resolve_head_controlled(repo, None, None)
}

pub(crate) fn resolve_head_controlled(
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
    normalized_git_oid(output.stdout)
}

pub(crate) fn git_diff_name_only_controlled(
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
        let path = std::str::from_utf8(raw).map_err(|_| WorkingTreeDiffError::SnapshotChanged)?;
        let path = crate::bounded_fs::normalize_repository_relative_path(Path::new(path))
            .map_err(|_| WorkingTreeDiffError::SnapshotChanged)?;
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
    collect_worktree_paths_controlled(repo, baseline_oid, None, None, None)
}

pub(crate) fn collect_worktree_paths_controlled(
    repo: &Path,
    baseline_oid: &str,
    ignored_path_prefix: Option<&str>,
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
        if ignored_path_prefix.is_some_and(|prefix| path_within_prefix(path, prefix)) {
            continue;
        }
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
        if ignored_path_prefix.is_some_and(|prefix| path_within_prefix(path, prefix)) {
            continue;
        }
        changed.insert(path.to_vec(), "untracked");
    }
    add_flagged_index_changes(
        repo,
        baseline_oid,
        ignored_path_prefix,
        &mut changed,
        deadline,
        interrupted,
    )?;

    Ok(finalize_changed_paths(changed))
}

/// Changed paths for a gate, which cannot judge a partial list: `None` when
/// the list is over the limit or has a non-UTF-8 path.
pub(crate) fn complete_worktree_paths_controlled(
    repo: &Path,
    baseline_oid: &str,
    ignored_path_prefix: Option<&str>,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<Option<Vec<WorkingTreeChangedFile>>, WorkingTreeDiffError> {
    let (files, _, truncated, skipped) = collect_worktree_paths_controlled(
        repo,
        baseline_oid,
        ignored_path_prefix,
        deadline,
        interrupted,
    )?;
    Ok(inventory_complete(truncated, skipped).then_some(files))
}

pub(crate) fn inventory_complete(truncated: bool, skipped_non_utf8_paths: u32) -> bool {
    !truncated && skipped_non_utf8_paths == 0
}

/// `git diff` trusts an index entry marked assume-unchanged or skip-worktree
/// and reports the index blob in place of its file, so an edit behind either
/// flag is invisible to it. The baseline diff is run again on a copy of the
/// index with both flags cleared, and each flagged path takes its status from
/// there, so Git itself applies its attribute, mode, symlink and removal
/// rules. In a sparse checkout an absent skip-worktree file is what the
/// checkout leaves out, so it keeps the status its index entry gave it.
fn add_flagged_index_changes(
    repo: &Path,
    baseline_oid: &str,
    ignored_path_prefix: Option<&str>,
    changed: &mut BTreeMap<Vec<u8>, &'static str>,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<(), WorkingTreeDiffError> {
    // The baseline diff covers the whole repository, so the flagged pass
    // does too, from the top level, where every path is a full name.
    let top = run_bounded_git_controlled(
        repo,
        &["rev-parse", "--show-toplevel"],
        None,
        GIT_OUTPUT_LIMIT,
        deadline,
        interrupted,
    )?;
    if !top.success {
        return Err(WorkingTreeDiffError::GitUnavailable);
    }
    let top = os_relative_path(top.stdout.strip_suffix(b"\n").unwrap_or(&top.stdout))
        .ok_or(WorkingTreeDiffError::GitUnavailable)?
        .to_path_buf();
    let run = |command: Command, input: Option<&[u8]>, retain: Option<RecordFilter>| {
        let output = run_bounded_git_records(
            &top,
            command,
            input,
            FLAGGED_LISTING_LIMIT,
            deadline,
            interrupted,
            retain,
        )?;
        if output.success {
            Ok(output.stdout)
        } else {
            Err(WorkingTreeDiffError::GitUnavailable)
        }
    };
    // Only a sparse checkout explains an absent skip-worktree file. Without
    // one, the absence is a deletion, so the entry stays in the listing.
    let sparse = run_bounded_git_controlled(
        &top,
        &["config", "--type=bool", "--get", "core.sparseCheckout"],
        None,
        GIT_OUTPUT_LIMIT,
        deadline,
        interrupted,
    )?;
    let sparse = sparse.success && sparse.stdout.trim_ascii() == b"true";
    // Tags: lowercase is assume-unchanged, `S` and `s` are skip-worktree.
    // Other records are dropped while reading, so the bound applies to
    // flagged files only.
    let root = top.clone();
    let listing = run(
        git_command(&["ls-files", "-v", "-z", "--"]),
        None,
        Some(Box::new(move |entry: &[u8]| match entry {
            [b'S' | b's', b' ', ..] if !sparse => true,
            // A path the OS cannot represent cannot be present either.
            [b'S' | b's', b' ', relative @ ..] => os_relative_path(relative).is_some_and(|path| {
                std::fs::symlink_metadata(root.join(path))
                    .map_or_else(|error| !missing_path(&error), |_| true)
            }),
            [tag, b' ', ..] => tag.is_ascii_lowercase(),
            _ => true,
        })),
    )?;
    let mut flagged = BTreeSet::new();
    let (mut assumed, mut skipped) = (Vec::new(), Vec::new());
    for entry in nul_fields(&listing) {
        let [tag, b' ', path @ ..] = entry else {
            return Err(WorkingTreeDiffError::InvalidRef);
        };
        if ignored_path_prefix.is_some_and(|ignored| path_within_prefix(path, ignored)) {
            continue;
        }
        if matches!(tag, b'S' | b's') {
            skipped.extend_from_slice(path);
            skipped.push(0);
        }
        if *tag != b'S' {
            assumed.extend_from_slice(path);
            assumed.push(0);
        }
        flagged.insert(path.to_vec());
    }
    if flagged.is_empty() {
        return Ok(());
    }
    let index = run(
        git_command(&["rev-parse", "--git-path", "index"]),
        None,
        None,
    )?;
    let index = top.join(
        os_relative_path(index.strip_suffix(b"\n").unwrap_or(&index))
            .ok_or(WorkingTreeDiffError::GitUnavailable)?,
    );
    let copy = copy_index(&index)?;
    let with_copy = |args: &[&str]| {
        let mut command = git_command(args);
        command.env("GIT_INDEX_FILE", copy.path().join("index"));
        command
    };
    // One flag per call: together, `update-index` leaves skip-worktree set
    // on entries that have both.
    for (flag, paths) in [
        ("--no-assume-unchanged", &assumed),
        ("--no-skip-worktree", &skipped),
    ] {
        if !paths.is_empty() {
            run(
                with_copy(&["update-index", "-q", flag, "-z", "--stdin"]),
                Some(paths),
                None,
            )?;
        }
    }
    let diff = run(
        with_copy(&[
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
        ]),
        None,
        None,
    )?;
    // A flagged path absent from this diff matches the baseline, even when
    // the flagged index reported it.
    for path in &flagged {
        changed.remove(path);
    }
    let mut fields = nul_fields(&diff);
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        if flagged.contains(path) {
            let status = match status.first() {
                Some(b'A') => "added",
                Some(b'D') => "deleted",
                _ => "modified",
            };
            changed.insert(path.to_vec(), status);
        }
    }
    Ok(())
}

/// Drops a NUL-terminated output record when it returns false.
type RecordFilter = Box<dyn FnMut(&[u8]) -> bool + Send>;

/// A copy of the index in a private temporary directory, so a read-only
/// `.git` still works and no other user can claim the name. Git finds a split
/// index's shared part in the repository, not next to the copy. The copy keeps
/// the index's mtime, which Git's racy-entry check compares with entry times.
/// The directory, with any lock file Git left, is removed when dropped.
fn copy_index(index: &Path) -> Result<tempfile::TempDir, WorkingTreeDiffError> {
    let directory = tempfile::Builder::new()
        .prefix("mmcg-flagged-index-")
        .tempdir()
        .map_err(|_| WorkingTreeDiffError::GitUnavailable)?;
    let copy = directory.path().join("index");
    std::fs::copy(index, &copy)
        .and_then(|_| std::fs::metadata(index)?.modified())
        .and_then(|modified| {
            std::fs::File::options()
                .write(true)
                .open(&copy)?
                .set_modified(modified)
        })
        .map_err(|_| WorkingTreeDiffError::GitUnavailable)?;
    Ok(directory)
}

fn missing_path(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

#[cfg(unix)]
fn os_relative_path(raw: &[u8]) -> Option<&Path> {
    use std::os::unix::ffi::OsStrExt;
    Some(Path::new(std::ffi::OsStr::from_bytes(raw)))
}

#[cfg(not(unix))]
fn os_relative_path(raw: &[u8]) -> Option<&Path> {
    std::str::from_utf8(raw).ok().map(Path::new)
}

fn path_within_prefix(path: &[u8], prefix: &str) -> bool {
    let prefix = prefix.as_bytes();
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|tail| tail.starts_with(b"/"))
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
                path,
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
    // The controller holds these locks during the audit. Windows prevents
    // reading locked files, and their bytes are not implementation evidence.
        || path
            .strip_prefix(b".mastermind/tasks/")
            .is_some_and(|relative| {
                let mut parts = relative.split(|byte| *byte == b'/');
                parts.next().is_some_and(|task| !task.is_empty())
                    && parts.next() == Some(b"state.controller.lock".as_slice())
                    && parts.next().is_none()
            })
        || path
            .strip_prefix(b".mastermind/run-state/")
            .and_then(|file| file.strip_suffix(b".controller.lock"))
            .is_some_and(|stem| !stem.is_empty() && !stem.contains(&b'/'))
}

/// Fetch a bounded set of blobs from one resolved baseline commit. Requests
/// are split across bounded `git cat-file --batch` processes so the aggregate
/// size of a large diff cannot trip the per-process output limit. Missing
/// paths are returned as `None`, which is how temporal rewind represents files
/// added after the baseline.
#[cfg(test)]
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

/// Recheck a complete filesystem/Git snapshot without parsing every changed
/// source file a third time. Callers that deliberately retain a partial
/// changed-file projection must use `validate_working_tree_projection_controlled`
/// and preserve its original truncation metadata.
pub(crate) fn validate_working_tree_snapshot_controlled(
    repo: &Path,
    baseline_oid: &str,
    expected_head_oid: &str,
    expected_files: &[WorkingTreeChangedFile],
    expected_token: &str,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<(), WorkingTreeDiffError> {
    validate_working_tree_projection_controlled(
        repo,
        baseline_oid,
        expected_head_oid,
        WorkingTreeSnapshotExpectation {
            files: expected_files,
            files_truncated: false,
            skipped_non_utf8_paths: 0,
            token: expected_token,
            ignored_path_prefix: None,
        },
        deadline,
        interrupted,
    )
}

pub(crate) struct WorkingTreeSnapshotExpectation<'a> {
    pub files: &'a [WorkingTreeChangedFile],
    pub files_truncated: bool,
    pub skipped_non_utf8_paths: u32,
    pub token: &'a str,
    pub ignored_path_prefix: Option<&'a str>,
}

/// Recheck the exact bounded changed-file projection used to build a partial
/// response. Changes beyond a stable file-limit prefix and non-UTF-8 paths are
/// outside that response's claim, but their truncation state must stay stable.
pub(crate) fn validate_working_tree_projection_controlled(
    repo: &Path,
    baseline_oid: &str,
    expected_head_oid: &str,
    expected: WorkingTreeSnapshotExpectation<'_>,
    deadline: Option<Instant>,
    interrupted: Option<&dyn Fn() -> bool>,
) -> Result<(), WorkingTreeDiffError> {
    if expected.ignored_path_prefix.is_some_and(|prefix| {
        prefix.is_empty()
            || expected
                .files
                .iter()
                .any(|file| path_within_prefix(file.path.as_bytes(), prefix))
    }) {
        return Err(WorkingTreeDiffError::SnapshotChanged);
    }
    if resolve_head_controlled(repo, deadline, interrupted)? != expected_head_oid {
        return Err(WorkingTreeDiffError::SnapshotChanged);
    }
    let (files, _, files_truncated, skipped_non_utf8_paths) = collect_worktree_paths_controlled(
        repo,
        baseline_oid,
        expected.ignored_path_prefix,
        deadline,
        interrupted,
    )?;
    if files_truncated != expected.files_truncated
        || skipped_non_utf8_paths != expected.skipped_non_utf8_paths
        || files != expected.files
    {
        return Err(WorkingTreeDiffError::SnapshotChanged);
    }
    let token = working_tree_snapshot_token_controlled(
        repo,
        expected_head_oid,
        &files,
        deadline,
        interrupted,
    )?;
    if token != expected.token {
        return Err(WorkingTreeDiffError::SnapshotChanged);
    }
    Ok(())
}

pub(crate) fn working_tree_snapshot_token_controlled(
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
            match read_repository_file(
                repo,
                Path::new(&file.path),
                MAX_INDEXABLE_FILE_SIZE,
                MAX_INDEXABLE_FILE_SIZE,
                ReadControl {
                    deadline,
                    interrupted,
                },
            ) {
                Ok(file) => digest.update(Sha256::digest(file.bytes)),
                // The indexer skips a source over the cap, so its size stands
                // for its content here.
                Err(BoundedReadError::TooLarge { size, .. }) => {
                    digest.update(b"too_large\0");
                    digest.update(size.to_be_bytes());
                }
                Err(error) => return Err(working_tree_read_error(error)),
            }
        }
    }
    Ok(crate::hex::encode(&digest.finalize()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::process::Command;

    pub(crate) fn init_repo(name: &str) -> std::path::PathBuf {
        let dir = env::temp_dir().join(format!("mmcg-diff-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "--initial-branch=main"]);
        run(&dir, &["config", "user.email", "t@t"]);
        run(&dir, &["config", "user.name", "t"]);
        run(&dir, &["config", "commit.gpgsign", "false"]);
        dir
    }

    pub(crate) fn run(dir: &Path, args: &[&str]) {
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

    pub(crate) fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    #[test]
    fn git_timeout_configuration_is_bounded() {
        assert_eq!(
            configured_git_timeout(None),
            Duration::from_millis(DEFAULT_GIT_TIMEOUT_MS)
        );
        assert_eq!(
            configured_git_timeout(Some("1250")),
            Duration::from_millis(1250)
        );
        assert_eq!(
            configured_git_timeout(Some("0")),
            Duration::from_millis(DEFAULT_GIT_TIMEOUT_MS)
        );
        assert_eq!(
            configured_git_timeout(Some("invalid")),
            Duration::from_millis(DEFAULT_GIT_TIMEOUT_MS)
        );
        assert_eq!(
            configured_git_timeout(Some("999999")),
            Duration::from_millis(MAX_GIT_TIMEOUT_MS)
        );
    }

    #[test]
    fn full_git_oid_accepts_sha1_and_sha256_only() {
        assert!(is_full_git_oid(&"0".repeat(40)));
        assert!(is_full_git_oid(&"a".repeat(64)));
        assert!(!is_full_git_oid(&"A".repeat(40)));
        assert!(!is_full_git_oid(&"0".repeat(39)));
        assert!(!is_full_git_oid(&"g".repeat(64)));
    }

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

    fn python_source_with_size(size: usize) -> String {
        assert!(size >= 2);
        format!("#{}\n", "x".repeat(size - 2))
    }

    fn flag(dir: &Path, flag: &str, paths: &[&str]) {
        let mut args = vec!["update-index", flag];
        args.extend_from_slice(paths);
        run(dir, &args);
    }

    fn listed(dir: &Path) -> Vec<(String, String)> {
        let (files, _, truncated, skipped) = collect_worktree_paths(dir, "HEAD").unwrap();
        assert!(!truncated);
        assert_eq!(skipped, 0);
        files
            .into_iter()
            .map(|file| (file.path, file.status))
            .collect()
    }

    #[test]
    fn edits_behind_index_flags_are_listed() {
        let dir = init_repo("index-flags");
        for name in ["assumed", "skipped", "both", "sparse", "gone", "same"] {
            write(&dir, name, "baseline\n");
        }
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        flag(
            &dir,
            "--assume-unchanged",
            &["assumed", "both", "gone", "same"],
        );
        flag(
            &dir,
            "--skip-worktree",
            &["skipped", "both", "sparse", "same"],
        );
        for name in ["assumed", "skipped", "both"] {
            write(&dir, name, "edited\n");
        }
        fs::remove_file(dir.join("sparse")).unwrap();
        fs::remove_file(dir.join("gone")).unwrap();

        let modified = |name: &str| (name.to_string(), "modified".to_string());
        let deleted = |name: &str| (name.to_string(), "deleted".to_string());
        assert_eq!(
            listed(&dir),
            [
                modified("assumed"),
                modified("both"),
                deleted("gone"),
                modified("skipped"),
                deleted("sparse"),
            ]
        );
        // Only a sparse checkout leaves a skip-worktree file absent on purpose.
        run(&dir, &["config", "core.sparseCheckout", "true"]);
        assert_eq!(
            listed(&dir),
            [
                modified("assumed"),
                modified("both"),
                deleted("gone"),
                modified("skipped"),
            ]
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// Git compares through the path's attributes and its own mode rules, and
    /// so must the check behind a flag, or untouched files read as edits.
    #[test]
    fn unchanged_files_behind_index_flags_compare_like_git() {
        let dir = init_repo("index-flags-unchanged");
        write(&dir, ".gitattributes", "*.conf text eol=crlf\n");
        write(&dir, "local.conf", "a\nb\n");
        write(&dir, "big.bin", &"x".repeat(6 * 1024 * 1024));
        write(&dir, "sub/nested.conf", "port = 1\n");
        write(&dir, "target", "t\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink("target", dir.join("link")).unwrap();
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        // Checkout writes CRLF bytes for the LF blob.
        fs::remove_file(dir.join("local.conf")).unwrap();
        run(&dir, &["checkout", "--", "local.conf"]);
        assert!(fs::read(dir.join("local.conf")).unwrap().contains(&b'\r'));
        flag(&dir, "--skip-worktree", &["local.conf", "sub/nested.conf"]);
        flag(&dir, "--assume-unchanged", &["big.bin"]);
        #[cfg(unix)]
        flag(&dir, "--assume-unchanged", &["link"]);

        assert_eq!(listed(&dir), []);

        write(&dir, "sub/nested.conf", "port = 2\n");
        let (files, ..) =
            collect_worktree_paths(&dir.join("sub"), &resolve_commit(&dir, "HEAD").unwrap())
                .unwrap();
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0].path, "sub/nested.conf");
        fs::remove_dir_all(&dir).ok();
    }

    /// The name exists only in Git objects, since APFS rejects it on disk.
    #[cfg(unix)]
    #[test]
    fn an_absent_non_utf8_path_behind_skip_worktree_is_unchanged() {
        use std::os::unix::ffi::OsStrExt;
        let dir = init_repo("index-flag-non-utf8");
        write(&dir, "a", "a\n");
        run(&dir, &["add", "-A"]);
        let blob = Command::new("git")
            .args(["hash-object", "-w", "a"])
            .current_dir(&dir)
            .output()
            .unwrap();
        let blob = String::from_utf8(blob.stdout).unwrap();
        let name = std::ffi::OsStr::from_bytes(b"data-\xff.bin");
        let cacheinfo = [
            b"100644,".as_slice(),
            blob.trim().as_bytes(),
            b",data-\xff.bin",
        ]
        .concat();
        let git = |args: &[&str], path: &std::ffi::OsStr| {
            let status = Command::new("git")
                .args(args)
                .arg(path)
                .current_dir(&dir)
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
        };
        git(
            &["update-index", "--add", "--cacheinfo"],
            std::ffi::OsStr::from_bytes(&cacheinfo),
        );
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["config", "core.sparseCheckout", "true"]);
        git(&["update-index", "--skip-worktree"], name);
        assert_eq!(listed(&dir), []);

        git(&["update-index", "--no-skip-worktree"], name);
        git(&["update-index", "--assume-unchanged"], name);
        let (_, _, _, skipped) = collect_worktree_paths(&dir, "HEAD").unwrap();
        assert_eq!(skipped, 1, "an absent assume-unchanged path is a deletion");
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn an_executable_bit_behind_an_index_flag_follows_core_filemode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = init_repo("index-flag-mode");
        write(&dir, "run.sh", "echo\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        flag(&dir, "--skip-worktree", &["run.sh"]);
        fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(listed(&dir), [("run.sh".into(), "modified".into())]);
        run(&dir, &["config", "core.fileMode", "false"]);
        assert_eq!(listed(&dir), []);
        fs::remove_dir_all(&dir).ok();
    }

    /// Shapes Git handles without the flag: a name `--stdin-paths` would cut,
    /// a parent that became a symlink or a file that became a directory
    /// (both removals), and a symlink checked out as a file.
    #[cfg(unix)]
    #[test]
    fn flagged_paths_of_every_shape_compare_like_git() {
        let dir = init_repo("index-flag-shapes");
        write(&dir, "cr\r", "same\n");
        write(&dir, "\"quoted", "baseline\n");
        write(&dir, "dir/file", "baseline\n");
        write(&dir, "conf", "baseline\n");
        std::os::unix::fs::symlink("target", dir.join("link")).unwrap();
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["config", "core.symlinks", "false"]);
        fs::remove_file(dir.join("link")).unwrap();
        run(&dir, &["checkout", "--", "link"]);
        assert!(fs::symlink_metadata(dir.join("link")).unwrap().is_file());
        flag(
            &dir,
            "--assume-unchanged",
            &["cr\r", "\"quoted", "dir/file", "conf", "link"],
        );
        write(&dir, "\"quoted", "edited\n");
        let outside = dir.with_extension("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::copy(dir.join("dir/file"), outside.join("file")).unwrap();
        fs::remove_dir_all(dir.join("dir")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("dir")).unwrap();
        fs::remove_file(dir.join("conf")).unwrap();
        write(&dir, "conf/inner", "x\n");

        assert_eq!(
            listed(&dir),
            [
                ("\"quoted".into(), "modified".into()),
                ("conf".into(), "deleted".into()),
                ("conf/inner".into(), "untracked".into()),
                ("dir".into(), "untracked".into()),
                ("dir/file".into(), "deleted".into()),
            ]
        );
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&outside).ok();
    }

    /// Git's own comparison decides: a CRLF blob under `core.autocrlf` is not
    /// renormalized, only the owner exec bit counts, an unreadable file is an
    /// edit rather than a failure, and a quoted subdirectory root works.
    #[cfg(unix)]
    #[test]
    fn flagged_files_follow_git_rules_for_eol_mode_and_access() {
        use std::os::unix::fs::PermissionsExt;
        let dir = init_repo("index-flag-git-rules");
        write(&dir, "crlf.txt", "a\r\nb\r\n");
        write(&dir, "group-exec", "x\n");
        write(&dir, "owner-exec", "x\n");
        write(&dir, "locked", "x\n");
        write(&dir, "\"sub/inner", "x\n");
        fs::set_permissions(dir.join("owner-exec"), fs::Permissions::from_mode(0o755)).unwrap();
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["config", "core.autocrlf", "true"]);
        flag(
            &dir,
            "--skip-worktree",
            &[
                "crlf.txt",
                "group-exec",
                "owner-exec",
                "locked",
                "\"sub/inner",
            ],
        );
        let mode = |name: &str, mode| {
            fs::set_permissions(dir.join(name), fs::Permissions::from_mode(mode)).unwrap()
        };
        mode("group-exec", 0o654);
        mode("owner-exec", 0o655);
        mode("locked", 0o000);
        write(&dir, "\"sub/inner", "edited\n");

        // Root reads the file anyway, and Git then finds it unchanged.
        let unreadable = fs::File::open(dir.join("locked")).is_err();
        let files = listed(&dir);
        mode("locked", 0o644);
        let modified = |name: &str| (name.to_string(), "modified".to_string());
        let mut expected = vec![modified("\"sub/inner"), modified("owner-exec")];
        if unreadable {
            expected.insert(1, modified("locked"));
        }
        assert_eq!(files, expected);
        // A subdirectory root still sees flagged edits elsewhere, as the
        // baseline diff sees unflagged ones.
        let (files, ..) = collect_worktree_paths(&dir.join("\"sub"), "HEAD").unwrap();
        let paths = files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["\"sub/inner", "owner-exec"]);
        fs::remove_dir_all(&dir).ok();
    }

    /// A flagged path takes its status against the baseline from the worktree,
    /// even when a staged change already listed it, and a split index needs
    /// nothing written to the repository.
    #[test]
    fn flagged_paths_take_their_baseline_status_from_the_worktree() {
        let dir = init_repo("index-flag-staged");
        for name in ["staged-gone", "staged-back", "conf", "sparse"] {
            write(&dir, name, "baseline\n");
        }
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        run(&dir, &["update-index", "--split-index"]);
        for name in ["staged-gone", "staged-back", "added-gone", "sparse"] {
            write(&dir, name, "staged\n");
            run(&dir, &["add", name]);
        }
        flag(
            &dir,
            "--assume-unchanged",
            &["staged-gone", "staged-back", "added-gone"],
        );
        // An absent sparse entry keeps the status of its index blob.
        run(&dir, &["config", "core.sparseCheckout", "true"]);
        flag(&dir, "--skip-worktree", &["conf", "sparse"]);
        fs::remove_file(dir.join("sparse")).unwrap();
        fs::remove_file(dir.join("staged-gone")).unwrap();
        fs::remove_file(dir.join("added-gone")).unwrap();
        write(&dir, "staged-back", "baseline\n");
        fs::remove_file(dir.join("conf")).unwrap();
        write(&dir, "conf/inner", "x\n");
        let git_files = || {
            let mut names = fs::read_dir(dir.join(".git"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>();
            names.sort();
            names
        };
        let before = git_files();

        assert_eq!(
            listed(&dir),
            [
                ("conf".into(), "deleted".into()),
                ("conf/inner".into(), "untracked".into()),
                ("sparse".into(), "modified".into()),
                ("staged-gone".into(), "deleted".into()),
            ]
        );
        assert_eq!(git_files(), before);
        fs::remove_dir_all(&dir).ok();
    }

    /// An entry as new as the index is racy, so Git checks its content. The
    /// copy must keep the index's mtime, or a same-size edit made in the same
    /// tick passes on stat data alone.
    #[test]
    fn a_racy_edit_behind_a_flag_is_listed() {
        let dir = init_repo("index-flag-racy");
        run(&dir, &["config", "core.trustctime", "false"]);
        let tick = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let stamp = |path: &Path| {
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(tick)
                .unwrap()
        };
        write(&dir, "port.conf", "port = 1\n");
        stamp(&dir.join("port.conf"));
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        flag(&dir, "--assume-unchanged", &["port.conf"]);
        fs::OpenOptions::new()
            .write(true)
            .open(dir.join("port.conf"))
            .unwrap()
            .write_all(b"port = 2\n")
            .unwrap();
        stamp(&dir.join("port.conf"));
        stamp(&dir.join(".git/index"));

        assert_eq!(listed(&dir), [("port.conf".into(), "modified".into())]);
        fs::remove_dir_all(&dir).ok();
    }

    /// Clearing flags writes an index, and Git runs `post-index-change` on
    /// every index write. A read must not run the repository's hooks.
    #[cfg(unix)]
    #[test]
    fn checking_flagged_files_runs_no_repository_hook() {
        let dir = init_repo("index-flag-hook");
        write(&dir, "conf", "baseline\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);
        flag(&dir, "--assume-unchanged", &["conf"]);
        write(&dir, "conf", "edited\n");
        let marker = dir.join("hook-ran");
        let hook = dir.join(".git/hooks/post-index-change");
        crate::test_executable::write_executable(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        );

        assert_eq!(listed(&dir), [("conf".into(), "modified".into())]);
        assert!(!marker.exists());
        fs::remove_dir_all(&dir).ok();
    }

    /// The flagged listing is bounded by what it keeps, not by the index.
    #[test]
    fn retained_records_alone_count_against_the_output_limit() {
        let mut output = b"H skip\0".repeat(4096);
        output.extend_from_slice(b"h keep\0S tail");
        let reader = bounded_record_reader(
            std::io::Cursor::new(output),
            16,
            Some(Box::new(|record: &[u8]| record[0] != b'H')),
        );
        assert_eq!(reader.recv().unwrap(), (b"h keep\0S tail".to_vec(), false));

        let reader = bounded_record_reader(
            std::io::Cursor::new(b"h one\0h two\0h three\0".to_vec()),
            12,
            Some(Box::new(|_: &[u8]| true)),
        );
        assert!(reader.recv().unwrap().1);
    }

    #[test]
    fn changed_path_projection_counts_non_utf8_entries_as_omitted() {
        let changed = BTreeMap::from([
            (b"src/app.py".to_vec(), "modified"),
            (b"untracked-\xff.bin".to_vec(), "untracked"),
        ]);

        let (files, total, truncated, skipped_non_utf8_paths) = finalize_changed_paths(changed);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/app.py");
        assert_eq!(total, Some(2));
        assert!(!truncated);
        assert_eq!(skipped_non_utf8_paths, 1);
    }

    #[cfg(unix)]
    #[test]
    fn changed_path_projection_does_not_alias_literal_backslashes() {
        let changed = BTreeMap::from([
            (b"src/pay.rs".to_vec(), "modified"),
            (b"src\\pay.rs".to_vec(), "untracked"),
        ]);

        let (files, total, truncated, skipped) = finalize_changed_paths(changed);
        let paths = files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["src/pay.rs", "src\\pay.rs"]);
        assert_eq!(total, Some(2));
        assert!(!truncated);
        assert_eq!(skipped, 0);
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
    fn git_pipe_completion_shares_the_operation_deadline() {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(1);
        let clock = std::cell::Cell::new(start);
        let ready_at = start + Duration::from_millis(250);
        let receive = |timeout| {
            let wake_at = clock.get() + timeout;
            if wake_at >= ready_at {
                clock.set(ready_at);
                Ok(b"git output".to_vec())
            } else {
                clock.set(wake_at);
                Err(mpsc::RecvTimeoutError::Timeout)
            }
        };
        assert_eq!(
            receive_git_pipe_until(deadline, None, || clock.get(), receive).unwrap(),
            b"git output"
        );

        clock.set(start);
        assert_eq!(
            receive_git_pipe_until::<()>(
                deadline,
                None,
                || clock.get(),
                |timeout| {
                    clock.set(clock.get() + timeout);
                    Err(mpsc::RecvTimeoutError::Timeout)
                }
            ),
            Err(WorkingTreeDiffError::GitTimeout)
        );
    }

    #[test]
    fn git_pipe_completion_observes_cancellation_before_the_deadline() {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(1);
        let clock = std::cell::Cell::new(start);
        let cancelled = std::cell::Cell::new(false);
        assert_eq!(
            receive_git_pipe_until::<()>(
                deadline,
                Some(&|| cancelled.get()),
                || clock.get(),
                |timeout| {
                    clock.set(clock.get() + timeout);
                    cancelled.set(true);
                    Err(mpsc::RecvTimeoutError::Timeout)
                }
            ),
            Err(WorkingTreeDiffError::GitTimeout)
        );
        assert!(clock.get() < deadline);
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
            let result = run_bounded_git(&repo, &["ignored"], Some(&vec![b'x'; 8 * 1024 * 1024]));
            assert_eq!(result.unwrap_err(), WorkingTreeDiffError::GitTimeout);
        }

        let pid_file = env::temp_dir().join(format!("mmcg-held-pipe-{}.pid", std::process::id()));
        let code = format!(
            "import os,signal,sys; data=sys.stdin.buffer.read(); pid=os.fork(); (open({:?},'w').write(str(pid)),os._exit(0)) if pid else signal.pause()",
            pid_file.to_string_lossy()
        );
        {
            let _guard = override_git("python3", &["-c", &code], Duration::from_secs(1));
            let result = run_bounded_git(&repo, &["ignored"], Some(b"request\n"));
            assert_eq!(result.unwrap_err(), WorkingTreeDiffError::GitTimeout);
        }
        if let Ok(pid) = fs::read_to_string(&pid_file) {
            let _ = Command::new("kill").args(["-TERM", pid.trim()]).status();
        }
        let _ = fs::remove_file(pid_file);
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
    fn commit_resolver_accepts_sha256_repository_oids() {
        let dir = env::temp_dir().join(format!(
            "mmcg-diff-{}-sha256-object-format",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        run(
            &dir,
            &[
                "init",
                "-q",
                "--object-format=sha256",
                "--initial-branch=main",
            ],
        );
        run(&dir, &["config", "user.email", "t@t"]);
        run(&dir, &["config", "user.name", "t"]);
        run(&dir, &["config", "commit.gpgsign", "false"]);
        write(&dir, "x.py", "def x():\n    pass\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "baseline"]);

        let resolved = resolve_commit(&dir, "HEAD").unwrap();
        assert_eq!(resolved.len(), 64);
        assert!(is_full_git_oid(&resolved));
        assert_eq!(resolve_head(&dir).unwrap(), resolved);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lookalike_controller_lock_paths_are_not_runtime_artifacts() {
        for path in [
            ".mastermind/tasks/001-review/state.json",
            ".mastermind/tasks/001-review/notes.controller.lock",
            ".mastermind/tasks/nested/001-review/state.controller.lock",
            ".mastermind/run-state/001-review.json",
            ".mastermind/run-state/nested/001-review.controller.lock",
            "src/state.controller.lock",
        ] {
            assert!(!is_mastermind_runtime_artifact(path.as_bytes()), "{path}");
        }
    }
}
