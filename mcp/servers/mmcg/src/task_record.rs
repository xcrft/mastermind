//! Task records on disk: the controller's `state.json`, the artifacts that
//! live beside a spec, the controller lock and the initial worktree manifest.
//!
//! State persists beside a canonical task spec as `<task>/state.json`, so every
//! task has one controller-owned lifecycle record. Legacy flat specs keep using
//! `.mastermind/run-state/<basename>.json`; other repository-contained specs use
//! an exact-path-derived key under `.mastermind/run-state/.noncanonical/`.

use crate::bounded_fs::{self, BoundedReadError, ReadControl, RootCapability};
use crate::task_scaffold::is_task_entry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

/// Bytes the history and strict workflow snapshots may read until a task
/// pins a project budget.
pub(crate) const DEFAULT_SNAPSHOT_MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

const RUN_STATE_BYTE_LIMIT: u64 = 1024 * 1024;

/// The `state.json` format this binary reads and writes.
pub const RUN_STATE_FORMAT_VERSION: u32 = 1;

/// Controller-owned handshake between pre- and post-flight. Canonical task
/// specs keep it beside the spec as `<task>/state.json`; legacy flat specs use
/// `<repo_root>/.mastermind/run-state/<spec-basename>.json`.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RunState {
    /// State written before the field existed has the first format.
    #[serde(default = "first_run_state_format")]
    pub format_version: u32,
    /// User-facing lifecycle state consumed by `mastermind status` / `next`.
    #[serde(
        default = "default_run_status",
        deserialize_with = "deserialize_run_status"
    )]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_artifact: Option<String>,
    /// Canonical repository-relative path to the spec file pre-flight ran
    /// against. Legacy state can contain an absolute path.
    pub spec_path: String,
    /// Stable repository identity captured by pre-flight. Missing only from
    /// legacy state, which must be rebound by a new explicit pre-flight before
    /// its approval can be consumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_identity: Option<String>,
    /// Exact explicit hook handoff. Missing for tasks created without an intake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intake_revision: Option<String>,
    /// SHA-256 of the approved spec body. Legacy 16-digit hashes remain readable.
    /// Post-flight requires an exact match or a new explicit pre-flight.
    pub spec_hash: String,
    /// Immutable successful preflight contract, retained through failed retries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_approval_sha256: Option<String>,
    /// `git rev-parse HEAD` captured at the first pre-flight — the audit's `--since`.
    /// Retries preserve it so previously committed implementation stays in scope.
    pub baseline_ref: String,
    /// First-approved local inputs, retained across retries without absorbing implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_worktree: Option<crate::task_worktree::InitialWorktree>,
    /// SHA-256 binding a held strict audit to the exact declared touch files.
    /// Older state files deserialize without it and are not accepted as
    /// architecture-policy evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_snapshot_sha256: Option<String>,
    /// Missing in legacy records, which retain their original v1 digest rules.
    #[serde(default = "legacy_snapshot_version")]
    pub held_snapshot_version: u32,
    /// The audited inputs and outputs this iteration's semantic review covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_snapshot_sha256: Option<String>,
    /// Unix epoch seconds at pre-flight.
    pub started_at: u64,
    /// Iteration count — +1 on every pre-flight entry; first fresh run is `1`.
    /// Survives `--reset`, `--pre-only` and failed revalidation. Legacy state files
    /// lacking this field deserialize to `0` — "not yet counted".
    #[serde(default)]
    pub iteration: u32,
    /// Preserve the pre-flight docs/spec-only escape hatch across hand-off so
    /// post-flight does not turn an intentionally ungrounded task into a hard
    /// index-identity failure.
    #[serde(default)]
    pub allow_no_index: bool,
    /// Keep explicitly requested strict pre-flight checks on later retries.
    #[serde(default)]
    pub strict: bool,
    /// This iteration must retain a successful invocation bound to its context
    /// and permission policy. Legacy/manual handoffs have no delivery claim.
    #[serde(default)]
    pub invocation_required: bool,
    /// Structured acceptance also requires an explicit, revision-bound review.
    /// Unfinished legacy tasks derive this requirement from their spec.
    #[serde(default)]
    pub semantic_review_required: bool,
    /// Exact active local review bytes, never an independently signed verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_review_sha256: Option<String>,
    /// Profile audience selected for this approved iteration, retained on resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_client: Option<String>,
    /// Optional committed Git refresh, separate from the task's verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_refresh: Option<serde_json::Value>,
    /// Total bytes this task's history and strict workflow snapshots may read,
    /// pinned by each pre-flight from the project's setting and bound by the
    /// approval record. Missing from state written before budgets existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_max_total_bytes: Option<u64>,
}

impl RunState {
    pub(crate) fn snapshot_limit(&self) -> u64 {
        self.snapshot_max_total_bytes
            .unwrap_or(DEFAULT_SNAPSHOT_MAX_TOTAL_BYTES)
    }
}

fn default_run_status() -> String {
    "approved".into()
}

pub(crate) fn validate_run_status(status: &str) -> Result<(), String> {
    if matches!(
        status,
        "history_review_required"
            | "learned"
            | "audit_required"
            | "approved"
            | "executing"
            | "held"
            | "drift"
            | "broken"
    ) {
        Ok(())
    } else {
        Err(format!("unsupported controller status {status:?}"))
    }
}

pub(crate) fn validate_run_state(state: &RunState) -> Result<(), String> {
    if state.format_version != RUN_STATE_FORMAT_VERSION {
        return Err(format!(
            "unsupported task state format {}",
            state.format_version
        ));
    }
    validate_run_status(&state.status)?;
    if state
        .preflight_approval_sha256
        .as_deref()
        .is_some_and(|pin| {
            pin.len() != 64
                || !pin
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    {
        return Err("controller preflight approval pin is invalid".into());
    }
    if let Some(initial) = &state.initial_worktree {
        initial.validate(&state.baseline_ref)?;
    }
    if state
        .snapshot_max_total_bytes
        .is_some_and(|max| !crate::onboarding::total_bytes_in_range(max))
    {
        return Err("controller snapshot byte budget is outside its range".into());
    }
    if let Some(revision) = state.semantic_review_sha256.as_deref() {
        if !state.semantic_review_required || !crate::hex::is_lower(revision, 64) {
            return Err("controller semantic review pin is invalid".into());
        }
    }
    if let Some(risk) = state.risk.as_deref() {
        if !matches!(risk, "low" | "medium" | "high") {
            return Err(format!("unsupported controller risk {risk:?}"));
        }
    }
    if let Some(next_step) = state.next_step.as_deref() {
        let compatible = match state.status.as_str() {
            "approved" | "executing" => next_step == "run_executor",
            "audit_required" => next_step == "run_audit",
            "history_review_required" => next_step == "review_history",
            "learned" => next_step == "close",
            "held" => matches!(next_step, "run_preflight" | "planner_review"),
            "drift" | "broken" => next_step == "planner_review",
            _ => false,
        };
        if !compatible {
            return Err(format!(
                "controller status {:?} cannot use next step {next_step:?}",
                state.status
            ));
        }
    }
    if let Some(identity) = state.repository_identity.as_deref() {
        let valid_identity = ["git-remote:sha256:", "git-worktree:sha256:"]
            .into_iter()
            .find_map(|prefix| identity.strip_prefix(prefix))
            .is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if !valid_identity {
            return Err("controller repository identity is invalid".into());
        }
        let normalized =
            bounded_fs::normalize_repository_relative_path(Path::new(&state.spec_path))
                .map_err(|_| "bound controller spec path is not canonical UTF-8".to_string())?;
        if normalized != state.spec_path {
            return Err("bound controller spec path is not canonical UTF-8".into());
        }
    }
    Ok(())
}

pub(crate) fn parse_run_state(bytes: &[u8]) -> Result<RunState, String> {
    parse_versioned_run_state(bytes)
        .map(|(state, _)| state)
        .map_err(|error| error.to_string())
}

/// A newer format can add fields this binary does not know, so its version is
/// read first and refused by name rather than reported as a damaged file. The
/// flag is true for state written before 4.0, which has no `format_version`.
fn parse_versioned_run_state(bytes: &[u8]) -> std::io::Result<(RunState, bool)> {
    let invalid = |error: String| std::io::Error::new(std::io::ErrorKind::InvalidData, error);
    let value = crate::strict_json::parse(bytes, crate::strict_json::Floats::Reject)
        .map_err(|error| invalid(error.to_string()))?;
    let format = value.get("format_version");
    let predates_format = format.is_none();
    if let Some(format) = format
        .and_then(serde_json::Value::as_u64)
        .filter(|&format| format > u64::from(RUN_STATE_FORMAT_VERSION))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "task state format {format} was written by a newer mastermind; this binary \
                 reads format {RUN_STATE_FORMAT_VERSION}, so continue the task with the newer \
                 mastermind binary"
            ),
        ));
    }
    let state: RunState =
        serde_json::from_value(value).map_err(|error| invalid(error.to_string()))?;
    validate_run_state(&state).map_err(invalid)?;
    Ok((state, predates_format))
}

fn deserialize_run_status<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let status = String::deserialize(deserializer)?;
    validate_run_status(&status).map_err(serde::de::Error::custom)?;
    Ok(status)
}

fn first_run_state_format() -> u32 {
    1
}

fn legacy_snapshot_version() -> u32 {
    1
}

#[derive(Debug, Eq, PartialEq)]
enum SpecArtifactLayout {
    Canonical(String),
    LegacyFlat(String),
    Noncanonical(String),
}

/// A relative spec path is repository-relative, whatever the working directory.
fn exact_spec_identity(repo_root: &Path, spec_path: &Path) -> Option<String> {
    let relative = if spec_path.is_absolute() {
        let requested_root = std::path::absolute(repo_root).ok()?;
        std::path::absolute(spec_path)
            .ok()?
            .strip_prefix(&requested_root)
            .ok()?
            .to_path_buf()
    } else {
        spec_path.to_path_buf()
    };
    bounded_fs::normalize_repository_relative_path(&relative).ok()
}

/// The first name under `.mastermind/tasks` of a repository-relative path.
fn task_entry(identity: &str) -> Option<&str> {
    identity
        .strip_prefix(".mastermind/tasks/")?
        .split('/')
        .next()
}

fn canonical_task_name(identity: &str) -> Option<&str> {
    let mut parts = identity.split('/');
    if parts.next() != Some(".mastermind") || parts.next() != Some("tasks") {
        return None;
    }
    let task = parts.next()?;
    if parts.next() == Some("spec.md") && parts.next().is_none() {
        Some(task)
    } else {
        None
    }
}

fn legacy_flat_spec_stem(identity: &str) -> Option<&str> {
    let mut parts = identity.split('/');
    if parts.next() != Some(".mastermind") || parts.next() != Some("tasks") {
        return None;
    }
    let file = parts.next()?;
    if parts.next().is_some() || !file.ends_with(".md") {
        return None;
    }
    Path::new(file).file_stem()?.to_str()
}

fn update_digest_with_native_path(digest: &mut Sha256, path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
        digest.update((units.len() as u64).to_le_bytes());
        for unit in units {
            digest.update(unit.to_le_bytes());
        }
    }
}

fn noncanonical_artifact_key(identity: Option<&str>, spec_path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(b"mastermind-noncanonical-spec-v1\0");
    match identity {
        Some(identity) => {
            digest.update(b"utf8\0");
            digest.update(identity.as_bytes());
        }
        None => {
            digest.update(b"native\0");
            update_digest_with_native_path(&mut digest, spec_path);
        }
    }
    let candidate = identity
        .and_then(|value| value.rsplit('/').next())
        .and_then(|value| Path::new(value).file_stem())
        .and_then(|value| value.to_str())
        .or_else(|| spec_path.file_stem().and_then(|value| value.to_str()))
        .unwrap_or("spec");
    let mut stem = String::new();
    let mut separator = false;
    for character in candidate.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !stem.is_empty() {
                stem.push('-');
            }
            stem.push(character);
            separator = false;
        } else {
            separator = true;
        }
        if stem.len() >= 48 {
            break;
        }
    }
    if stem.is_empty() {
        stem.push_str("spec");
    }
    format!("{stem}-{}", crate::hex::encode(&digest.finalize()))
}

fn spec_artifact_layout(repo_root: &Path, spec_path: &Path) -> SpecArtifactLayout {
    let identity = exact_spec_identity(repo_root, spec_path);
    if let Some(task) = identity.as_deref().and_then(canonical_task_name) {
        return SpecArtifactLayout::Canonical(task.to_string());
    }
    if let Some(stem) = identity.as_deref().and_then(legacy_flat_spec_stem) {
        return SpecArtifactLayout::LegacyFlat(stem.to_string());
    }
    SpecArtifactLayout::Noncanonical(noncanonical_artifact_key(identity.as_deref(), spec_path))
}

/// The executor report and the post-flight audit live beside the spec. A
/// file named `spec.md` is the only spec in its folder, so its artifacts keep
/// plain names. Other specs can share a folder, so their artifacts carry the
/// spec's file stem, as in `050-a.executor-report.md`.
fn spec_artifact_path(spec_path: &Path, name: &str) -> PathBuf {
    if spec_path.file_name() == Some(std::ffi::OsStr::new("spec.md")) {
        return spec_path.with_file_name(name);
    }
    let mut file = spec_path.file_stem().unwrap_or_default().to_os_string();
    file.push(".");
    file.push(name);
    spec_path.with_file_name(file)
}

pub fn executor_report_path(spec_path: &Path) -> PathBuf {
    spec_artifact_path(spec_path, "executor-report.md")
}

pub fn audit_file_path(spec_path: &Path) -> PathBuf {
    spec_artifact_path(spec_path, "audit.md")
}

/// Canonical specs own `<task>/state.json`. Legacy top-level task specs retain
/// their basename-keyed location. Every other repository-contained spec uses a
/// full exact-path digest so equal basenames cannot share controller state.
pub fn state_file_path(repo_root: &Path, spec_path: &Path) -> PathBuf {
    match spec_artifact_layout(repo_root, spec_path) {
        SpecArtifactLayout::Canonical(task) => repo_root
            .join(".mastermind/tasks")
            .join(task)
            .join("state.json"),
        SpecArtifactLayout::LegacyFlat(stem) => repo_root
            .join(".mastermind/run-state")
            .join(format!("{stem}.json")),
        SpecArtifactLayout::Noncanonical(key) => repo_root
            .join(".mastermind/run-state/.noncanonical")
            .join(format!("{key}.json")),
    }
}

/// Release notes retain human task names for canonical and legacy flat specs;
/// other paths use the same collision-resistant artifact key as controller state.
pub fn release_file_path(repo_root: &Path, spec_path: &Path) -> PathBuf {
    match spec_artifact_layout(repo_root, spec_path) {
        SpecArtifactLayout::Canonical(task) | SpecArtifactLayout::LegacyFlat(task) => repo_root
            .join(".mastermind/releases")
            .join(format!("{task}.md")),
        SpecArtifactLayout::Noncanonical(key) => repo_root
            .join(".mastermind/releases/.noncanonical")
            .join(format!("{key}.md")),
    }
}

/// Semantic-history review path. Canonical tasks keep it beside their spec;
/// legacy flat specs use the local run-state directory, and other specs use
/// their exact-path-derived noncanonical state namespace.
pub fn history_review_file_path(repo_root: &Path, spec_path: &Path) -> PathBuf {
    match spec_artifact_layout(repo_root, spec_path) {
        SpecArtifactLayout::Canonical(task) => repo_root
            .join(".mastermind/tasks")
            .join(task)
            .join("history-review.md"),
        SpecArtifactLayout::LegacyFlat(stem) => repo_root
            .join(".mastermind/run-state")
            .join(format!("{stem}-history-review.md")),
        SpecArtifactLayout::Noncanonical(key) => repo_root
            .join(".mastermind/run-state/.noncanonical")
            .join(format!("{key}-history-review.md")),
    }
}

/// Read + deserialize state. `Ok(None)` when the file is absent — "no prior
/// pre-flight" is the dominant non-error case.
pub fn load_state(path: &Path) -> std::io::Result<Option<RunState>> {
    Ok(load_state_and_format(path)?.map(|(state, _)| state))
}

/// Like `load_state`, and also says whether the state predates 4.0.
pub(crate) fn load_state_and_format(path: &Path) -> std::io::Result<Option<(RunState, bool)>> {
    let path = &std::path::absolute(path)?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let body = match bounded_fs::read_regular_file(
        parent,
        path,
        RUN_STATE_BYTE_LIMIT,
        RUN_STATE_BYTE_LIMIT,
        ReadControl::default(),
    ) {
        Ok(file) => file.bytes,
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None)
        }
        Err(error) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
    };
    parse_versioned_run_state(&body).map(Some)
}

pub fn save_state_in_repository(
    repo_root: &Path,
    path: &Path,
    state: &RunState,
) -> std::io::Result<()> {
    validate_run_state(state)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let body = serde_json::to_vec_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if body.len() as u64 > RUN_STATE_BYTE_LIMIT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "serialized state has {} bytes, limit is {RUN_STATE_BYTE_LIMIT}",
                body.len()
            ),
        ));
    }
    bounded_fs::write_atomic_regular_file(repo_root, path, &body, true)
        .map_err(std::io::Error::other)
}

/// Stable across toolchains and machines for persisted approval evidence.
pub(crate) fn hash_text(text: &str) -> String {
    crate::hex::sha256(text.as_bytes())
}

pub(crate) fn spec_hash_matches(expected: &str, text: &str) -> bool {
    if expected.len() == 16 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let mut legacy = DefaultHasher::new();
        text.hash(&mut legacy);
        return expected == format!("{:016x}", legacy.finish());
    }
    expected == hash_text(text)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ControllerIdentity {
    pub(crate) repository: String,
    pub(crate) spec_path: String,
}

pub(crate) fn validate_bound_state_identity(
    repository: &str,
    spec_path: &str,
    state: &RunState,
) -> Result<(), String> {
    let Some(saved_repository) = state.repository_identity.as_deref() else {
        return Err(
            "legacy state has no repository binding; run an explicit pre-flight to rebind it"
                .into(),
        );
    };
    if saved_repository != repository {
        return Err("saved state belongs to a different repository".into());
    }
    if state.spec_path != spec_path {
        return Err("saved state belongs to a different spec".into());
    }
    Ok(())
}

pub(crate) fn controller_identity(
    repo_root: &Path,
    spec_path: &Path,
) -> Result<ControllerIdentity, String> {
    let root = RootCapability::open(repo_root)
        .map_err(|error| format!("opening repository capability: {error}"))?;
    let relative = root
        .repository_relative(spec_path)
        .map_err(|error| format!("resolving repository-relative spec path: {error}"))?;
    let spec_path = bounded_fs::normalize_repository_relative_path(&relative)
        .map_err(|_| "spec path is not canonical repository-relative UTF-8".to_string())?;
    if let Some(entry) = task_entry(&spec_path).filter(|entry| !is_task_entry(entry)) {
        return Err(format!(
            "`{}` is not a task: names under .mastermind/tasks that start with `_` or `.` \
             hold drafts and shared files. Rename it to start a task",
            crate::terminal::escape(entry)
        ));
    }
    // The state, lock and artifacts follow the spec path, so two spellings of
    // one file, such as `SPEC.md` on a case-insensitive file system, would get
    // two controllers. A missing spec fails here, before any lock is created.
    let named = root.canonical_root().join(&relative);
    let on_disk = named.canonicalize().map_err(|error| {
        format!(
            "reading spec `{}`: {error}",
            crate::terminal::escape(&named.display().to_string())
        )
    })?;
    if on_disk != named {
        return Err(format!(
            "spec `{}` is stored as `{}`; run the task with that path",
            crate::terminal::escape(&named.display().to_string()),
            crate::terminal::escape(&on_disk.display().to_string())
        ));
    }
    let repository = crate::facts::repository_identity(root.canonical_root())
        .map_err(|error| format!("resolving repository identity: {error}"))?;
    root.verify()
        .map_err(|error| format!("repository changed while resolving task identity: {error}"))?;
    Ok(ControllerIdentity {
        repository,
        spec_path,
    })
}

pub(crate) fn legacy_state_matches_spec(
    repo_root: &Path,
    spec_path: &Path,
    current_spec_path: &str,
    state: &RunState,
) -> bool {
    if state.spec_path == current_spec_path {
        return true;
    }
    let resolved = if spec_path.is_absolute() {
        spec_path.to_path_buf()
    } else {
        repo_root.join(spec_path)
    };
    let stored = Path::new(&state.spec_path);
    if stored.is_absolute() && stored == resolved {
        return true;
    }
    let stored = if stored.is_absolute() {
        stored.to_path_buf()
    } else {
        repo_root.join(stored)
    };
    if stored
        .canonicalize()
        .ok()
        .zip(resolved.canonicalize().ok())
        .is_some_and(|(stored, current)| stored == current)
    {
        return true;
    }
    // A full pre-flight can migrate a legacy canonical task after its checkout
    // moved. Match path components exactly; treating a literal backslash as a
    // separator on Unix would alias a different task.
    let relative = Path::new(current_spec_path);
    let Ok(task_spec) = relative.strip_prefix(Path::new(".mastermind/tasks")) else {
        return false;
    };
    if task_spec.components().count() != 2
        || task_spec.file_name().and_then(|name| name.to_str()) != Some("spec.md")
    {
        return false;
    }
    Path::new(&state.spec_path).ends_with(relative)
}

pub(crate) fn validate_state_binding(
    repo_root: &Path,
    spec_path: &Path,
    current: &ControllerIdentity,
    state: &RunState,
    allow_legacy_rebind: bool,
) -> Result<(), String> {
    if state.repository_identity.is_some() {
        return validate_bound_state_identity(&current.repository, &current.spec_path, state);
    }
    if !legacy_state_matches_spec(repo_root, spec_path, &current.spec_path, state) {
        return Err("saved state belongs to a different spec".into());
    }
    if !allow_legacy_rebind {
        return Err(
            "legacy state has no repository binding; run an explicit pre-flight to rebind it"
                .into(),
        );
    }
    Ok(())
}

/// `run-task`, `review-task` and task intake binding lock through here. The
/// spec's identity is resolved first, so a missing spec or a `_draft` folder leaves no lock or
/// directory.
pub(crate) fn lock_task_controller(
    repo_root: &Path,
    spec_path: &Path,
) -> Result<(bounded_fs::StableFileLock, ControllerIdentity), String> {
    let identity = controller_identity(repo_root, spec_path)
        .map_err(|error| format!("resolving task identity: {error}"))?;
    let lock = controller_lock(repo_root, &state_file_path(repo_root, spec_path))
        .map_err(|error| format!("task controller is busy or unavailable: {error}"))?;
    Ok((lock, identity))
}

pub(crate) fn controller_lock(
    repo_root: &Path,
    state_path: &Path,
) -> Result<crate::bounded_fs::StableFileLock, String> {
    let root = RootCapability::open(repo_root).map_err(|_| "controller_root_unavailable")?;
    let path = state_path.with_extension("controller.lock");
    let parent = path.parent().ok_or("controller_lock_unavailable")?;
    root.ensure_directory(parent)
        .map_err(|_| "controller_lock_unavailable")?;
    bounded_fs::try_locked_regular_file_with_capability(&root, &path)
        .map_err(|_| "controller_busy_or_unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::init_repo;
    use crate::run_task::STRICT_SNAPSHOT_VERSION;
    use std::fs;

    fn save_state(path: &Path, state: &RunState) -> std::io::Result<()> {
        let path = &std::path::absolute(path)?;
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        save_state_in_repository(parent, path, state)
    }

    #[test]
    fn state_file_roundtrips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        let state = RunState {
            format_version: RUN_STATE_FORMAT_VERSION,
            status: "approved".into(),
            risk: Some("low".into()),
            next_step: Some("run_executor".into()),
            blocking_reason: None,
            last_artifact: Some("spec.md".into()),
            spec_path: "specs/foo.md".into(),
            repository_identity: Some(format!("git-worktree:sha256:{}", "a".repeat(64))),
            intake_revision: None,
            spec_hash: "deadbeefcafef00d".into(),
            baseline_ref: "abc1234".into(),
            initial_worktree: None,
            preflight_approval_sha256: None,
            held_snapshot_sha256: Some("feedface".into()),
            held_snapshot_version: STRICT_SNAPSHOT_VERSION,
            history_snapshot_sha256: Some("reviewed-snapshot".into()),
            started_at: 123456,
            iteration: 0,
            allow_no_index: true,
            strict: false,
            invocation_required: false,
            semantic_review_required: false,
            semantic_review_sha256: None,
            profile_client: Some("task-reader".into()),
            profile_refresh: None,
            snapshot_max_total_bytes: None,
        };
        save_state(&path, &state).unwrap();
        let loaded = load_state(&path).unwrap().expect("present");
        assert_eq!(loaded.spec_path, state.spec_path);
        assert_eq!(loaded.repository_identity, state.repository_identity);
        assert_eq!(loaded.spec_hash, state.spec_hash);
        assert_eq!(loaded.baseline_ref, state.baseline_ref);
        assert_eq!(loaded.held_snapshot_sha256, state.held_snapshot_sha256);
        assert_eq!(loaded.held_snapshot_version, STRICT_SNAPSHOT_VERSION);
        let mut legacy = serde_json::to_value(&state).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("held_snapshot_version");
        legacy
            .as_object_mut()
            .unwrap()
            .remove("repository_identity");
        let legacy = serde_json::from_value::<RunState>(legacy).unwrap();
        assert_eq!(legacy.held_snapshot_version, 1);
        assert!(legacy.repository_identity.is_none());
        assert_eq!(
            loaded.history_snapshot_sha256,
            state.history_snapshot_sha256
        );
        assert_eq!(loaded.started_at, state.started_at);
        assert!(loaded.allow_no_index);
        let mut unknown = serde_json::to_value(&state).unwrap();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("stats".into(), serde_json::Value::String("learned".into()));
        fs::write(&path, serde_json::to_vec(&unknown).unwrap()).unwrap();
        assert_eq!(
            load_state(&path).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        for (field, value) in [("risk", "critical"), ("next_step", "close")] {
            let mut invalid = serde_json::to_value(&state).unwrap();
            invalid
                .as_object_mut()
                .unwrap()
                .insert(field.into(), serde_json::Value::String(value.into()));
            fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert_eq!(
                load_state(&path).unwrap_err().kind(),
                std::io::ErrorKind::InvalidData,
                "{field}"
            );
        }
        let mut unsupported = serde_json::to_value(&state).unwrap();
        unsupported
            .as_object_mut()
            .unwrap()
            .insert("status".into(), serde_json::Value::String("future".into()));
        fs::write(&path, serde_json::to_vec(&unsupported).unwrap()).unwrap();
        assert_eq!(
            load_state(&path).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        let mut unsupported = state.clone();
        unsupported.status = "future".into();
        assert_eq!(
            save_state(&path, &unsupported).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        let mut incompatible = state.clone();
        incompatible.next_step = Some("close".into());
        assert_eq!(
            save_state(&path, &incompatible).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        let mut invalid_identity = state.clone();
        invalid_identity.repository_identity = Some("git-remote:sha256:not-a-digest".into());
        assert_eq!(
            save_state(&path, &invalid_identity).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        let mut ambiguous_path = state.clone();
        ambiguous_path.spec_path = "specs\\foo.md".into();
        assert_eq!(
            save_state(&path, &ambiguous_path).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        fs::remove_file(&path).unwrap();
        assert!(load_state(&path).unwrap().is_none());
    }

    #[test]
    fn controller_state_is_bound_to_the_selected_repository() {
        let first = init_repo("state_binding_first");
        let second = init_repo("state_binding_second");
        let relative = Path::new(".mastermind/tasks/001-binding/spec.md");
        for root in [&first, &second] {
            fs::create_dir_all(root.join(relative).parent().unwrap()).unwrap();
            fs::write(root.join(relative), "# Binding\n").unwrap();
        }
        let first_identity = controller_identity(&first, &first.join(relative)).unwrap();
        let second_identity = controller_identity(&second, &second.join(relative)).unwrap();
        assert_ne!(first_identity.repository, second_identity.repository);
        let state: RunState = serde_json::from_value(serde_json::json!({
            "spec_path": first_identity.spec_path,
            "repository_identity": first_identity.repository,
            "spec_hash": "0".repeat(64),
            "baseline_ref": "0".repeat(40),
            "started_at": 1
        }))
        .unwrap();
        assert!(validate_state_binding(
            &first,
            &first.join(relative),
            &controller_identity(&first, &first.join(relative)).unwrap(),
            &state,
            false
        )
        .is_ok());
        assert!(validate_state_binding(
            &second,
            &second.join(relative),
            &second_identity,
            &state,
            true
        )
        .unwrap_err()
        .contains("different repository"));
        fs::remove_dir_all(first).ok();
        fs::remove_dir_all(second).ok();
    }

    #[test]
    fn legacy_state_requires_explicit_preflight_before_rebinding() {
        let root = init_repo("legacy_state_rebind");
        let spec = root.join(".mastermind/tasks/001-legacy/spec.md");
        fs::create_dir_all(spec.parent().unwrap()).unwrap();
        fs::write(&spec, "# Legacy\n").unwrap();
        let current = controller_identity(&root, &spec).unwrap();
        let state: RunState = serde_json::from_value(serde_json::json!({
            "spec_path": spec.display().to_string(),
            "spec_hash": "0".repeat(64),
            "baseline_ref": "0".repeat(40),
            "started_at": 1
        }))
        .unwrap();
        assert!(
            validate_state_binding(&root, &spec, &current, &state, false)
                .unwrap_err()
                .contains("explicit pre-flight")
        );
        assert!(validate_state_binding(&root, &spec, &current, &state, true).is_ok());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn only_a_spec_stored_under_its_own_task_name_gets_a_controller_lock() {
        let root = init_repo("task_controller_lock");
        let spec = root.join(".mastermind/tasks/001-task/spec.md");
        let draft = root.join(".mastermind/tasks/_draft/spec.md");
        for path in [&spec, &draft] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "# Task\n").unwrap();
        }
        let lock = |spec: &Path| lock_task_controller(&root, spec).map(|_| ()).unwrap_err();

        assert!(lock(&root.join("docs/missing.md")).contains("reading spec"));
        assert!(lock(&draft).contains("`_draft` is not a task"));
        // A case-insensitive file system finds the file and names its stored
        // spelling. A case-sensitive one finds nothing.
        let alias = lock(&root.join(".mastermind/tasks/001-task/SPEC.md"));
        assert!(
            alias.contains("is stored as") || alias.contains("reading spec"),
            "{alias}"
        );
        assert!(!root.join(".mastermind/run-state").exists());
        assert!(!draft.with_file_name("state.controller.lock").exists());

        let (_lock, identity) = lock_task_controller(&root, &spec).unwrap();
        assert_eq!(identity.spec_path, ".mastermind/tasks/001-task/spec.md");
        assert!(spec.with_file_name("state.controller.lock").exists());
        fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn controller_identity_rejects_a_literal_backslash_spec_alias() {
        let root = init_repo("backslash_spec_identity");
        let spec = root
            .join(".mastermind/tasks")
            .join("001\\alias")
            .join("spec.md");
        fs::create_dir_all(spec.parent().unwrap()).unwrap();
        fs::write(&spec, "# Ambiguous\n").unwrap();
        assert!(controller_identity(&root, &spec)
            .unwrap_err()
            .contains("canonical repository-relative UTF-8"));
        fs::remove_dir_all(root).ok();
    }

    // APFS rejects invalid UTF-8 directory names before identity validation.
    #[cfg(target_os = "linux")]
    #[test]
    fn controller_identity_rejects_a_non_utf8_spec_path() {
        use std::os::unix::ffi::OsStringExt;

        let root = init_repo("non_utf8_spec_identity");
        let spec = root
            .join(".mastermind/tasks")
            .join(std::ffi::OsString::from_vec(b"002-\xff".to_vec()))
            .join("spec.md");
        fs::create_dir_all(spec.parent().unwrap()).unwrap();
        fs::write(&spec, "# Ambiguous\n").unwrap();
        assert!(controller_identity(&root, &spec)
            .unwrap_err()
            .contains("canonical repository-relative UTF-8"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn canonical_tasks_have_distinct_state_and_release_paths() {
        let root = Path::new("/repo");
        let first = Path::new(".mastermind/tasks/001-first/spec.md");
        let second = Path::new(".mastermind/tasks/002-second/spec.md");

        assert_eq!(
            state_file_path(root, first),
            root.join(".mastermind/tasks/001-first/state.json")
        );
        assert_eq!(
            state_file_path(root, second),
            root.join(".mastermind/tasks/002-second/state.json")
        );
        assert_ne!(state_file_path(root, first), state_file_path(root, second));
        assert_eq!(
            release_file_path(root, first),
            root.join(".mastermind/releases/001-first.md")
        );
        assert_eq!(
            release_file_path(root, second),
            root.join(".mastermind/releases/002-second.md")
        );
        assert_eq!(
            history_review_file_path(root, first),
            root.join(".mastermind/tasks/001-first/history-review.md")
        );
        assert_eq!(
            history_review_file_path(root, second),
            root.join(".mastermind/tasks/002-second/history-review.md")
        );
    }

    #[test]
    fn only_the_exact_task_folder_shape_uses_task_local_artifacts() {
        let root = Path::new("/repo");

        assert!(matches!(
            spec_artifact_layout(root, Path::new(".mastermind/tasks/001-task/spec.md")),
            SpecArtifactLayout::Canonical(task) if task == "001-task"
        ));
        assert!(matches!(
            spec_artifact_layout(root, Path::new(".mastermind/tasks/spec.md")),
            SpecArtifactLayout::LegacyFlat(stem) if stem == "spec"
        ));
        assert_eq!(
            state_file_path(root, Path::new(".mastermind/tasks/spec.md")),
            root.join(".mastermind/run-state/spec.json")
        );
        assert!(matches!(
            spec_artifact_layout(root, Path::new("arbitrary/spec.md")),
            SpecArtifactLayout::Noncanonical(_)
        ));
    }

    #[test]
    fn legacy_flat_specs_keep_their_existing_artifact_paths() {
        let root = Path::new("/repo");
        let spec = Path::new(".mastermind/tasks/050-clean-add.md");

        assert_eq!(
            state_file_path(root, spec),
            root.join(".mastermind/run-state/050-clean-add.json")
        );
        assert_eq!(
            release_file_path(root, spec),
            root.join(".mastermind/releases/050-clean-add.md")
        );
        assert_eq!(
            history_review_file_path(root, spec),
            root.join(".mastermind/run-state/050-clean-add-history-review.md")
        );
    }

    #[test]
    fn equal_noncanonical_basenames_get_distinct_exact_path_artifacts() {
        let root = Path::new("/repo");
        let first = Path::new("a/same.md");
        let second = Path::new("b/same.md");

        let selectors: [fn(&Path, &Path) -> PathBuf; 3] =
            [state_file_path, release_file_path, history_review_file_path];
        for select in selectors {
            let first_path = select(root, first);
            let second_path = select(root, second);
            assert_ne!(first_path, second_path);
            assert!(first_path
                .components()
                .any(|component| component.as_os_str() == ".noncanonical"));
            assert!(second_path
                .components()
                .any(|component| component.as_os_str() == ".noncanonical"));
        }
    }

    #[test]
    fn specs_sharing_a_folder_get_their_own_report_and_audit() {
        for (first, second) in [
            (".mastermind/tasks/050-a.md", ".mastermind/tasks/051-b.md"),
            ("docs/specs/a.md", "docs/specs/b.md"),
        ] {
            let (first, second) = (Path::new(first), Path::new(second));
            assert_ne!(executor_report_path(first), executor_report_path(second));
            assert_ne!(audit_file_path(first), audit_file_path(second));
            assert_ne!(executor_report_path(first), audit_file_path(first));
        }
        let canonical = Path::new(".mastermind/tasks/001-x/spec.md");
        assert_eq!(
            executor_report_path(canonical),
            Path::new(".mastermind/tasks/001-x/executor-report.md")
        );
        assert_eq!(
            audit_file_path(canonical),
            Path::new(".mastermind/tasks/001-x/audit.md")
        );
    }

    #[test]
    fn hash_text_is_stable_for_same_input() {
        let a = hash_text("alpha\nbeta\n");
        let b = hash_text("alpha\nbeta\n");
        let c = hash_text("alpha\nbeta\ngamma\n");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
        assert_eq!(
            hash_text("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut legacy = DefaultHasher::new();
        "alpha\nbeta\n".hash(&mut legacy);
        let legacy_hash = format!("{:016x}", legacy.finish());
        assert!(spec_hash_matches(&legacy_hash, "alpha\nbeta\n"));
        assert!(!spec_hash_matches(&legacy_hash, "changed"));
        assert!(!spec_hash_matches("invalid", "alpha\nbeta\n"));
    }

    #[test]
    fn legacy_state_without_iteration_deserializes_to_zero() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("legacy.json");
        std::fs::write(
            &state_path,
            r#"{"spec_path":"foo.md","spec_hash":"abc","baseline_ref":"HEAD","started_at":0}"#,
        )
        .unwrap();

        let state = load_state(&state_path).unwrap().expect("loads");
        assert_eq!(state.iteration, 0, "missing field defaults to 0");
    }
}
