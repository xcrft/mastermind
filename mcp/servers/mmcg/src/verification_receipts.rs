//! Explicit, bounded command execution and local verification records.
//!
//! Records are writable by the repository owner. Their hashes bind local inputs;
//! they are not signatures, independent execution attestations, or a sandbox.

use crate::bounded_fs::{self, ReadControl, RootCapability};
use crate::spec::{ParsedSpec, VerifyEntry};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;
#[cfg(unix)]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RECEIPT_LIMIT: u64 = 128 * 1024;
const FILE_LIMIT: u64 = 4 * 1024 * 1024;
const TOTAL_FILE_LIMIT: u64 = 64 * 1024 * 1024;
const GIT_LIMIT: usize = 8 * 1024 * 1024;
const EXECUTABLE_LIMIT: u64 = 128 * 1024 * 1024;
#[cfg(unix)]
const OUTPUT_LIMIT: usize = 1024 * 1024;
const MAX_SNAPSHOT_FILES: usize = 10_000;
const MAX_INDEX_FILES: usize = 100_000;
const SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunDeclaration {
    pub id: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Passed,
    Failed,
    Timeout,
    Interrupted,
    OutputLimit,
    InputChanged,
    SpawnFailed,
    IoFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckEvidenceStatus {
    Current,
    Missing,
    Pending,
    Failed,
    Stale,
    Unavailable,
}

/// A redacted observation of one declared check. Local receipt contents remain
/// untrusted: only validated identifiers and fixed reason codes are exposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CheckEvidence {
    pub id: String,
    pub status: CheckEvidenceStatus,
    pub receipt_revision: Option<String>,
    pub run_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub repository_identity: String,
    pub spec_path: String,
    pub spec_sha256: String,
    pub baseline_oid: String,
    pub iteration: u32,
    pub preflight_started_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Executable {
    pub invocation_path: String,
    pub resolved_path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamDigest {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema_version: u32,
    pub run_id: String,
    pub check_id: String,
    pub attempt: u64,
    pub status: Status,
    pub reason: Option<String>,
    pub binding: Binding,
    pub declaration_sha256: String,
    pub command: String,
    pub declaration: RunDeclaration,
    pub executable: Option<Executable>,
    pub snapshot_version: u32,
    pub snapshot_before: Option<String>,
    pub snapshot_after: Option<String>,
    pub started_at: u64,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout: Option<StreamDigest>,
    pub stderr: Option<StreamDigest>,
    pub runner_version: String,
    pub platform: String,
    pub provenance: String,
}

impl Receipt {
    pub fn passed(&self) -> bool {
        self.status == Status::Passed
            && self.exit_code == Some(0)
            && self.signal.is_none()
            && self.reason.is_none()
            && self.snapshot_before.is_some()
            && self.snapshot_before == self.snapshot_after
            && self.executable.is_some()
            && self.stdout.is_some()
            && self.stderr.is_some()
    }
}

/// A stable display representation only. Execution always uses the argv array.
pub fn display_command(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if !arg.is_empty()
                && arg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_./:+-=@%".contains(&b))
            {
                arg.clone()
            } else {
                format!("'{}'", arg.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn declarations(spec: &ParsedSpec) -> Vec<(&str, &RunDeclaration)> {
    spec.frontmatter
        .iter()
        .flat_map(|fm| fm.verify.iter())
        .filter_map(|entry| match entry {
            VerifyEntry::Observed { cmd, run } => Some((cmd.trim(), run)),
            _ => None,
        })
        .collect()
}

pub(crate) fn has_observed(spec: &ParsedSpec) -> bool {
    !declarations(spec).is_empty()
}

pub(crate) fn validate_declarations(spec: &ParsedSpec) -> Result<(), String> {
    let runs = declarations(spec);
    if runs.len() > 32 {
        return Err("at most 32 observed verification declarations are supported".into());
    }
    let mut ids = BTreeSet::new();
    let mut commands = BTreeMap::<&str, usize>::new();
    for cmd in spec
        .frontmatter
        .iter()
        .flat_map(|fm| fm.verify.iter())
        .filter_map(VerifyEntry::command)
    {
        *commands.entry(cmd.trim()).or_default() += 1;
    }
    for (cmd, run) in runs {
        if run.id.is_empty()
            || run.id.len() > 64
            || !run
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || !ids.insert(&run.id)
        {
            return Err(
                "observed verification IDs must be unique 1..64 byte ASCII identifiers".into(),
            );
        }
        if commands.get(cmd) != Some(&1) {
            return Err(format!(
                "observed verification `{}` has an ambiguous cmd",
                run.id
            ));
        }
        if run.argv.is_empty()
            || run.argv.len() > 64
            || run.argv[0].is_empty()
            || run
                .argv
                .iter()
                .any(|arg| arg.len() > 8192 || arg.contains('\0'))
            || run.argv.iter().map(String::len).sum::<usize>() > 32768
            || !(1..=3600).contains(&run.timeout_secs)
        {
            return Err(format!(
                "observed verification `{}` has invalid argv or timeout",
                run.id
            ));
        }
        if run.cwd != "."
            && !bounded_fs::normalize_repository_relative_path(Path::new(&run.cwd))
                .is_ok_and(|normalized| normalized == run.cwd)
        {
            return Err(format!(
                "observed verification `{}` needs a canonical repository-relative cwd",
                run.id
            ));
        }
        let expected = display_command(&run.argv);
        if cmd != expected {
            return Err(format!(
                "observed verification `{}` cmd must be `{expected}`",
                run.id
            ));
        }
    }
    Ok(())
}

struct Context {
    root: RootCapability,
    spec: ParsedSpec,
    binding: Binding,
    directory: PathBuf,
}

fn context(spec_path: &Path, repo_root: &Path, deadline: Instant) -> Result<Context, String> {
    let root = RootCapability::open(repo_root).map_err(|e| format!("root_unavailable: {e}"))?;
    let relative = root
        .repository_relative(spec_path)
        .map_err(|e| e.to_string())?;
    let spec_path = bounded_fs::normalize_repository_relative_path(&relative)
        .map_err(|_| "invalid_spec_path")?;
    let body = bounded_fs::read_regular_file_with_capability(
        &root,
        &root.requested_root().join(&spec_path),
        FILE_LIMIT,
        FILE_LIMIT,
        ReadControl {
            deadline: Some(deadline),
            interrupted: Some(&interrupted),
        },
    )
    .map_err(|e| format!("spec_unavailable: {e}"))?
    .bytes;
    let body = String::from_utf8(body).map_err(|_| "spec_not_utf8")?;
    let spec = crate::spec::parse_str(&spec_path, &body);
    if spec.frontmatter_error.is_some() {
        return Err("invalid_spec_frontmatter".into());
    }
    validate_declarations(&spec)?;
    let state_path = crate::run_task::state_file_path(
        root.requested_root(),
        &root.requested_root().join(&spec_path),
    );
    let state_body = bounded_fs::read_regular_file_with_capability(
        &root,
        &state_path,
        RECEIPT_LIMIT,
        RECEIPT_LIMIT,
        ReadControl {
            deadline: Some(deadline),
            interrupted: Some(&interrupted),
        },
    )
    .map_err(|e| format!("preflight_state_unavailable: {e}"))?
    .bytes;
    let state = crate::run_task::parse_run_state(&state_body)?;
    let repository = crate::facts::repository_identity_until(root.canonical_root(), Some(deadline))
        .map_err(|e| format!("repository_identity_unavailable: {e}"))?;
    if interrupted() {
        return Err("verification_interrupted".into());
    }
    crate::run_task::validate_bound_state_identity(&repository, &spec_path, &state)?;
    if !crate::run_task::spec_hash_matches(&state.spec_hash, &body)
        || state.next_step.as_deref() == Some("run_preflight")
        || !crate::diff::is_full_git_oid(&state.baseline_ref)
        || state.iteration == 0
    {
        return Err("current_explicit_preflight_required".into());
    }
    let directory = if state_path
        .file_name()
        .is_some_and(|name| name == "state.json")
    {
        state_path.with_file_name("verification")
    } else {
        state_path.with_extension("verification")
    };
    let binding = Binding {
        repository_identity: repository,
        spec_path,
        spec_sha256: sha(body.as_bytes()),
        baseline_oid: state.baseline_ref,
        iteration: state.iteration,
        preflight_started_at: state.started_at,
    };
    root.verify().map_err(|e| e.to_string())?;
    Ok(Context {
        root,
        spec,
        binding,
        directory,
    })
}

fn sha(bytes: &[u8]) -> String {
    crate::hex::encode(&Sha256::digest(bytes))
}

fn declaration_hash(command: &str, run: &RunDeclaration) -> String {
    sha(&serde_json::to_vec(&(command, run)).expect("serializable declaration"))
}

#[cfg(unix)]
fn read_receipt(
    context: &Context,
    id: &str,
    deadline: Instant,
) -> Result<Option<(Receipt, String)>, String> {
    read_receipt_bytes(context, id, deadline)?
        .map(|bytes| decode_receipt(&bytes).map(|receipt| (receipt, sha(&bytes))))
        .transpose()
}

fn read_receipt_bytes(
    context: &Context,
    id: &str,
    deadline: Instant,
) -> Result<Option<Vec<u8>>, String> {
    let path = context.directory.join(format!("{id}.json"));
    let bytes = match bounded_fs::read_regular_file_with_capability(
        &context.root,
        &path,
        RECEIPT_LIMIT,
        RECEIPT_LIMIT,
        ReadControl {
            deadline: Some(deadline),
            interrupted: Some(&interrupted),
        },
    ) {
        Ok(file) => file.bytes,
        Err(bounded_fs::BoundedReadError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None)
        }
        Err(e) => return Err(format!("receipt_unavailable: {e}")),
    };
    Ok(Some(bytes))
}

fn decode_receipt(bytes: &[u8]) -> Result<Receipt, String> {
    let receipt: Receipt = serde_json::from_slice(bytes).map_err(|_| "receipt_invalid")?;
    if receipt.schema_version != 1
        || receipt.snapshot_version != SNAPSHOT_VERSION
        || receipt.attempt == 0
    {
        return Err("receipt_version_or_attempt_invalid".into());
    }
    Ok(receipt)
}

#[cfg(unix)]
fn write_receipt(context: &Context, receipt: &Receipt) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > RECEIPT_LIMIT {
        return Err("receipt_output_limit".into());
    }
    bounded_fs::write_atomic_regular_file(
        context.root.requested_root(),
        &context.directory.join(format!("{}.json", receipt.check_id)),
        &bytes,
        true,
    )
    .map_err(|e| format!("receipt_write_failed: {e}"))
}

#[cfg(unix)]
fn publish_receipt(
    context: &Context,
    receipt: &mut Receipt,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    // Completion linearizes at this final cancellation check. Cancellation
    // during earlier postflight reads must revoke success; signals arriving
    // after this decision do not retroactively revoke its atomic publication.
    if cancelled() {
        receipt.status = Status::Interrupted;
        receipt.reason = Some("verification_interrupted".into());
    }
    write_receipt(context, receipt)
}

/// Exact receipt files included in the task's semantic-history binding.
pub(crate) fn history_paths(
    spec: &ParsedSpec,
    root: &Path,
    spec_path: &Path,
) -> Result<Vec<PathBuf>, String> {
    validate_declarations(spec)?;
    let state = crate::run_task::state_file_path(root, spec_path);
    let directory = if state.file_name().is_some_and(|name| name == "state.json") {
        state.with_file_name("verification")
    } else {
        state.with_extension("verification")
    };
    Ok(declarations(spec)
        .iter()
        .map(|(_, run)| directory.join(format!("{}.json", run.id)))
        .collect())
}

/// Validate all opt-in obligations without executing anything. The returned
/// byte digests let the caller reject receipt changes during the audit.
pub(crate) fn audit_checks(
    spec: &ParsedSpec,
    root: &Path,
    baseline: &str,
    deadline: Instant,
) -> Result<Vec<String>, String> {
    current_digests(&inspect_checks(spec, root, baseline, deadline)?)
}

/// Consume only complete current observations. Inspection is separate so a
/// caller can derive both a report and its audit binding from the same reads.
pub(crate) fn current_digests(checks: &[CheckEvidence]) -> Result<Vec<String>, String> {
    checks
        .iter()
        .map(|check| {
            if check.status != CheckEvidenceStatus::Current {
                return Err(format!(
                    "{}: {}",
                    check.id,
                    check.reason.as_deref().unwrap_or("receipt_not_current")
                ));
            }
            check
                .receipt_revision
                .as_ref()
                .filter(|revision| valid_hex(revision, 64))
                .cloned()
                .ok_or_else(|| format!("{}: receipt_revision_unavailable", check.id))
        })
        .collect()
}

/// Inspect all declared checks against one stable repository snapshot. This
/// function is read-only and exposes no command, output, or filesystem path.
/// Shared read failures invalidate the whole observation instead of implying
/// that a receipt is absent. Individual malformed receipts remain unavailable.
pub(crate) fn inspect_checks(
    spec: &ParsedSpec,
    root: &Path,
    baseline: &str,
    deadline: Instant,
) -> Result<Vec<CheckEvidence>, String> {
    inspect_checks_with_mode(spec, root, baseline, deadline, InspectionMode::Audit)
}

/// Inspect the same bound inputs for a bounded repair decision. Only a normal
/// nonzero command exit with complete, current observations is `receipt_failed`.
/// This identifies a failed check, not its cause or permission to change code.
pub(crate) fn inspect_repair_checks(
    spec: &ParsedSpec,
    root: &Path,
    baseline: &str,
    deadline: Instant,
) -> Result<Vec<CheckEvidence>, String> {
    inspect_checks_with_mode(spec, root, baseline, deadline, InspectionMode::Repair)
}

/// Bind all observed tracked and nonignored worktree inputs independently of
/// controller state or iteration. The supplied baseline, HEAD and index remain
/// part of the existing snapshot contract; `.mastermind/` artifacts do not.
pub(crate) fn repair_input_revision(
    root: &Path,
    baseline: &str,
    deadline: Instant,
) -> Result<String, String> {
    let root = RootCapability::open(root).map_err(|_| "verification_root_unavailable")?;
    snapshot(&root, baseline, deadline).map_err(|_| "verification_snapshot_unavailable".into())
}

/// Pin only each declared top-level executable and its resolution. This does
/// not bind the semantics of argv, interpreted scripts, or test dependencies.
pub(crate) fn repair_executable_revisions(
    spec: &ParsedSpec,
    root: &Path,
    baseline: &str,
    deadline: Instant,
) -> Result<BTreeMap<String, String>, String> {
    validate_declarations(spec).map_err(|_| "verification_declarations_invalid")?;
    if !has_observed(spec) {
        return Ok(BTreeMap::new());
    }
    let (spec_path, current) = inspection_context(spec, root, baseline, deadline)?;
    let mut revisions = BTreeMap::new();
    for (_, run) in declarations(spec) {
        let executable = resolve_executable(&current.root, run, deadline)
            .map_err(|_| format!("{}: receipt_executable_unavailable", run.id))?;
        let bytes = serde_json::to_vec(&executable)
            .map_err(|_| format!("{}: receipt_executable_unavailable", run.id))?;
        revisions.insert(run.id.clone(), sha(&bytes));
    }
    recheck_inspection_context(&current, &spec_path, root, deadline)?;
    Ok(revisions)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InspectionMode {
    Audit,
    Repair,
}

fn inspection_context(
    spec: &ParsedSpec,
    root: &Path,
    baseline: &str,
    deadline: Instant,
) -> Result<(PathBuf, Context), String> {
    let spec_path = if Path::new(&spec.path).is_absolute() {
        PathBuf::from(&spec.path)
    } else {
        root.join(&spec.path)
    };
    let current =
        context(&spec_path, root, deadline).map_err(|_| "verification_context_unavailable")?;
    if current.binding.baseline_oid != baseline {
        return Err("receipt_baseline_mismatch".into());
    }
    // The caller may have parsed before another process edited or re-approved
    // the spec. Compare its entire parsed contract, ignoring only path spelling.
    let mut checked = spec.clone();
    checked.path.clone_from(&current.spec.path);
    if serde_json::to_value(&checked).map_err(|_| "receipt_spec_unavailable")?
        != serde_json::to_value(&current.spec).map_err(|_| "receipt_spec_unavailable")?
    {
        return Err("receipt_spec_changed".into());
    }
    Ok((spec_path, current))
}

fn recheck_inspection_context(
    current: &Context,
    spec_path: &Path,
    root: &Path,
    deadline: Instant,
) -> Result<(), String> {
    if context(spec_path, root, deadline)
        .map_err(|_| "verification_context_unavailable")?
        .binding
        != current.binding
    {
        return Err("receipt_task_changed".into());
    }
    current
        .root
        .verify()
        .map_err(|_| "verification_root_changed".into())
}

fn inspect_checks_with_mode(
    spec: &ParsedSpec,
    root: &Path,
    baseline: &str,
    deadline: Instant,
    mode: InspectionMode,
) -> Result<Vec<CheckEvidence>, String> {
    validate_declarations(spec).map_err(|_| "verification_declarations_invalid")?;
    if !has_observed(spec) {
        return Ok(Vec::new());
    }
    let (spec_path, current) = inspection_context(spec, root, baseline, deadline)?;
    // An unrun task has no verification directory or lock. Observing that
    // absence must not create state. No receipt may be accepted without a lock.
    let lock = match bounded_fs::inspect_path_kind_with_capability(
        &current.root,
        &current.directory,
        ReadControl {
            deadline: Some(deadline),
            interrupted: Some(&interrupted),
        },
    ) {
        Ok(bounded_fs::BoundedPathKind::Directory) => {
            Some(acquire_lock(&current, false).map_err(|reason| {
                if reason == "verification_busy" {
                    reason
                } else {
                    "verification_lock_unavailable".into()
                }
            })?)
        }
        Err(bounded_fs::BoundedReadError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            None
        }
        _ => return Err("verification_lock_unavailable".into()),
    };
    let snapshot = snapshot(&current.root, baseline, deadline)
        .map_err(|_| "verification_snapshot_unavailable")?;
    let mut evidence = Vec::new();
    for (cmd, run) in declarations(spec) {
        let bytes = read_receipt_bytes(&current, &run.id, deadline)
            .map_err(|_| format!("{}: receipt_unavailable", run.id))?;
        if bytes.is_some() && lock.is_none() {
            return Err("verification_lock_unavailable".into());
        }
        evidence.push(inspect_receipt(
            &current,
            cmd,
            run,
            &snapshot,
            bytes.as_deref(),
            deadline,
            mode,
        )?);
    }
    recheck_inspection_context(&current, &spec_path, root, deadline)?;
    Ok(evidence)
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn inspect_receipt(
    context: &Context,
    command: &str,
    run: &RunDeclaration,
    snapshot: &str,
    bytes: Option<&[u8]>,
    deadline: Instant,
    mode: InspectionMode,
) -> Result<CheckEvidence, String> {
    let mut evidence = CheckEvidence {
        id: run.id.clone(),
        status: CheckEvidenceStatus::Missing,
        receipt_revision: bytes.map(sha),
        run_id: None,
        reason: Some("receipt_missing".into()),
    };
    let Some(bytes) = bytes else {
        return Ok(evidence);
    };
    let receipt = match decode_receipt(bytes) {
        Ok(receipt) => receipt,
        Err(reason) => {
            evidence.status = CheckEvidenceStatus::Unavailable;
            evidence.reason = Some(reason);
            return Ok(evidence);
        }
    };
    evidence.run_id = valid_hex(&receipt.run_id, 32).then(|| receipt.run_id.clone());
    let classification = if receipt.provenance != "local_runner_unsigned" {
        Some((CheckEvidenceStatus::Unavailable, "receipt_source_unknown"))
    } else if evidence.run_id.is_none() {
        Some((CheckEvidenceStatus::Unavailable, "receipt_run_id_invalid"))
    } else if receipt.binding != context.binding
        || receipt.check_id != run.id
        || receipt.command != command
        || receipt.declaration != *run
        || receipt.declaration_sha256 != declaration_hash(command, run)
        || receipt.platform != std::env::consts::OS
        || receipt.runner_version != env!("CARGO_PKG_VERSION")
    {
        Some((CheckEvidenceStatus::Stale, "receipt_binding_mismatch"))
    } else {
        match receipt.status {
            Status::Pending => Some((CheckEvidenceStatus::Pending, "receipt_pending")),
            Status::InputChanged => Some((CheckEvidenceStatus::Stale, "receipt_inputs_changed")),
            Status::Passed => None,
            Status::Failed if mode == InspectionMode::Repair => None,
            Status::Failed => Some((CheckEvidenceStatus::Failed, "receipt_failed")),
            Status::Timeout
            | Status::Interrupted
            | Status::OutputLimit
            | Status::SpawnFailed
            | Status::IoFailed => {
                let status = if mode == InspectionMode::Repair {
                    CheckEvidenceStatus::Unavailable
                } else {
                    CheckEvidenceStatus::Failed
                };
                let reason = match receipt.status {
                    Status::Timeout => "receipt_timeout",
                    Status::Interrupted => "receipt_interrupted",
                    Status::OutputLimit => "receipt_output_limit",
                    Status::SpawnFailed => "receipt_spawn_failed",
                    Status::IoFailed => "receipt_io_failed",
                    _ => unreachable!(),
                };
                Some((status, reason))
            }
        }
    };
    if let Some((status, reason)) = classification {
        evidence.status = status;
        evidence.reason = Some(reason.into());
        return Ok(evidence);
    }
    let failed_check = mode == InspectionMode::Repair && receipt.status == Status::Failed;
    let valid_outcome = if failed_check {
        // Unix ExitStatus::code records 0..255; a signal has no exit code.
        // The runner assigns no reason to a normally completed failed command.
        receipt
            .exit_code
            .is_some_and(|code| (1..=255).contains(&code))
            && receipt.signal.is_none()
            && receipt.reason.is_none()
            && receipt.snapshot_before.is_some()
            && receipt.snapshot_before == receipt.snapshot_after
            && receipt.executable.is_some()
    } else {
        receipt.passed()
    };
    if !valid_outcome
        || [receipt.stdout.as_ref(), receipt.stderr.as_ref()]
            .iter()
            .any(|stream| {
                stream.is_none_or(|stream| {
                    stream.bytes > 1024 * 1024 || !valid_hex(&stream.sha256, 64)
                })
            })
    {
        evidence.status = CheckEvidenceStatus::Unavailable;
        evidence.reason = Some("receipt_outcome_invalid".into());
        return Ok(evidence);
    }
    if receipt.snapshot_after.as_deref() != Some(snapshot) {
        evidence.status = CheckEvidenceStatus::Stale;
        evidence.reason = Some("receipt_snapshot_changed".into());
        return Ok(evidence);
    }
    let executable = resolve_executable(&context.root, run, deadline)
        .map_err(|_| format!("{}: receipt_executable_unavailable", run.id))?;
    if receipt.executable.as_ref() != Some(&executable) {
        evidence.status = CheckEvidenceStatus::Stale;
        evidence.reason = Some("receipt_executable_changed".into());
        return Ok(evidence);
    }
    if failed_check {
        evidence.status = CheckEvidenceStatus::Failed;
        evidence.reason = Some("receipt_failed".into());
    } else {
        evidence.status = CheckEvidenceStatus::Current;
        evidence.reason = None;
    }
    Ok(evidence)
}

fn git(root: &Path, args: &[&str], deadline: Instant) -> Result<Vec<u8>, String> {
    if interrupted() {
        return Err("verification_interrupted".into());
    }
    let mut controlled = vec![
        "--literal-pathspecs",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "diff.external=",
    ];
    controlled.extend_from_slice(args);
    let result = crate::diff::run_bounded_git_with_control(
        root,
        &controlled,
        None,
        GIT_LIMIT,
        Some(deadline),
        Some(&interrupted),
    )
    .map_err(|e| format!("verification_snapshot_{}", e.code()))?;
    if !result.success {
        return Err("verification_snapshot_git_failed".into());
    }
    Ok(result.stdout)
}

fn snapshot(root: &RootCapability, baseline: &str, deadline: Instant) -> Result<String, String> {
    let first = snapshot_once(root, baseline, deadline)?;
    if snapshot_once(root, baseline, deadline)? != first {
        return Err("verification_snapshot_changed".into());
    }
    Ok(first)
}

fn snapshot_once(
    root: &RootCapability,
    baseline: &str,
    deadline: Instant,
) -> Result<String, String> {
    let repo = root.canonical_root();
    let baseline_commit = git(
        repo,
        &["rev-parse", "--verify", &format!("{baseline}^{{commit}}")],
        deadline,
    )?;
    if String::from_utf8_lossy(&baseline_commit).trim() != baseline {
        return Err("verification_snapshot_baseline_unavailable".into());
    }
    let head = git(repo, &["rev-parse", "--verify", "HEAD"], deadline)?;
    let mut digest = Sha256::new();
    digest.update(b"mastermind-verification-worktree-v1\0");
    digest.update(baseline.as_bytes());
    digest.update([0]);
    digest.update(head);
    let index = git(repo, &["ls-files", "--stage", "-z"], deadline)?;
    let entries = index
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    let mut paths = BTreeSet::new();
    if entries.len() > MAX_INDEX_FILES {
        return Err("verification_snapshot_index_limit".into());
    }
    for entry in entries {
        let tab = entry
            .iter()
            .position(|b| *b == b'\t')
            .ok_or("verification_snapshot_index_invalid")?;
        let path = snapshot_path(&entry[tab + 1..])?;
        if path.starts_with(".mastermind/") {
            continue;
        }
        let header = std::str::from_utf8(&entry[..tab])
            .map_err(|_| "verification_snapshot_index_invalid")?;
        let parts = header.split(' ').collect::<Vec<_>>();
        if parts.len() != 3 || !matches!(parts[0], "100644" | "100755") || parts[2] != "0" {
            return Err("verification_snapshot_unsupported_git_entry".into());
        }
        paths.insert(path);
        if paths.len() > MAX_SNAPSHOT_FILES {
            return Err("verification_snapshot_file_limit".into());
        }
        digest.update(entry);
        digest.update([0]);
    }
    for args in [
        vec![
            "diff",
            "--name-only",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            baseline,
            "--",
        ],
        vec!["ls-files", "--others", "--exclude-standard", "-z"],
    ] {
        for raw in git(repo, &args, deadline)?
            .split(|b| *b == 0)
            .filter(|raw| !raw.is_empty())
        {
            let path = snapshot_path(raw)?;
            if !path.starts_with(".mastermind/") {
                paths.insert(path);
            }
            if paths.len() > MAX_SNAPSHOT_FILES {
                return Err("verification_snapshot_file_limit".into());
            }
        }
    }
    let mut remaining = TOTAL_FILE_LIMIT;
    for path in paths {
        digest.update(path.as_bytes());
        digest.update([0]);
        let limit = remaining.min(FILE_LIMIT);
        match bounded_fs::read_regular_file_with_capability(
            root,
            &root.requested_root().join(&path),
            limit,
            limit,
            ReadControl {
                deadline: Some(deadline),
                interrupted: Some(&interrupted),
            },
        ) {
            Ok(file) => {
                remaining -= file.declared_len;
                digest.update(b"file\0");
                digest.update(file.declared_len.to_le_bytes());
                digest.update(file.identity.attributes().to_le_bytes());
                digest.update(file.bytes);
            }
            Err(bounded_fs::BoundedReadError::Io(e))
                if e.kind() == std::io::ErrorKind::NotFound =>
            {
                digest.update(b"missing\0");
            }
            Err(e) => return Err(format!("verification_snapshot_file_unavailable: {e}")),
        }
        digest.update([0]);
    }
    root.verify().map_err(|e| e.to_string())?;
    Ok(crate::hex::encode(&digest.finalize()))
}

fn snapshot_path(bytes: &[u8]) -> Result<String, String> {
    let path = std::str::from_utf8(bytes).map_err(|_| "verification_snapshot_non_utf8_path")?;
    bounded_fs::normalize_repository_relative_path(Path::new(path))
        .map_err(|_| "verification_snapshot_invalid_path".into())
}

fn cwd(root: &RootCapability, run: &RunDeclaration) -> Result<PathBuf, String> {
    if run.cwd == "." {
        return Ok(root.canonical_root().to_path_buf());
    }
    let path = root.requested_root().join(&run.cwd);
    if bounded_fs::inspect_path_kind_with_capability(root, &path, ReadControl::default())
        .map_err(|e| format!("cwd_unavailable: {e}"))?
        != bounded_fs::BoundedPathKind::Directory
    {
        return Err("cwd_not_directory".into());
    }
    Ok(root.canonical_root().join(&run.cwd))
}

fn resolve_executable(
    root: &RootCapability,
    run: &RunDeclaration,
    deadline: Instant,
) -> Result<Executable, String> {
    let directory = cwd(root, run)?;
    let program = Path::new(&run.argv[0]);
    let invocation = if program.is_absolute() {
        program.to_path_buf()
    } else if program.components().count() > 1 {
        directory.join(program)
    } else {
        let value = std::env::var_os("PATH").ok_or("executable_path_unavailable")?;
        let entries = std::env::split_paths(&value).collect::<Vec<_>>();
        if entries.is_empty() || entries.iter().any(|entry| !entry.is_absolute()) {
            return Err("executable_path_must_be_absolute".into());
        }
        entries
            .into_iter()
            .map(|entry| entry.join(program))
            .find(|candidate| candidate.is_file())
            .ok_or("executable_not_found")?
    };
    let resolved = invocation
        .canonicalize()
        .map_err(|_| "executable_unavailable")?;
    let parent = resolved.parent().ok_or("executable_invalid")?;
    let file = bounded_fs::read_regular_file(
        parent,
        &resolved,
        EXECUTABLE_LIMIT,
        EXECUTABLE_LIMIT,
        ReadControl {
            deadline: Some(deadline),
            interrupted: Some(&interrupted),
        },
    )
    .map_err(|e| format!("executable_unavailable: {e}"))?;
    #[cfg(unix)]
    if file.identity.attributes() & 0o111 == 0 {
        return Err("executable_not_executable".into());
    }
    Ok(Executable {
        invocation_path: invocation
            .to_str()
            .ok_or("executable_non_utf8_path")?
            .into(),
        resolved_path: resolved.to_str().ok_or("executable_non_utf8_path")?.into(),
        sha256: sha(&file.bytes),
    })
}

#[cfg(not(unix))]
fn acquire_lock(_context: &Context, _create: bool) -> Result<std::fs::File, String> {
    Err("verification_execution_unsupported_platform".into())
}

#[cfg(unix)]
fn acquire_lock(context: &Context, create: bool) -> Result<std::fs::File, String> {
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
    if create {
        let relative = context
            .root
            .repository_relative(&context.directory)
            .map_err(|e| e.to_string())?;
        context
            .root
            .ensure_directory(&relative)
            .map_err(|e| e.to_string())?;
    }
    let relative = context
        .root
        .repository_relative(&context.directory)
        .map_err(|e| e.to_string())?;
    let absolute = context.root.canonical_root().join(relative);
    let mut directory =
        Dir::open_ambient_dir("/", cap_std::ambient_authority()).map_err(|e| e.to_string())?;
    for component in absolute.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => {
                directory = directory
                    .open_dir_nofollow(name)
                    .map_err(|_| "verification_lock_unavailable")?;
            }
            _ => return Err("verification_lock_invalid_path".into()),
        }
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(create)
        .create(create)
        .mode(0o600)
        .follow(FollowSymlinks::No);
    let lock = directory
        .open_with("run.lock", &options)
        .map_err(|_| "verification_lock_unavailable")?
        .into_std();
    if !lock.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("verification_lock_not_regular".into());
    }
    lock.try_lock().map_err(|_| "verification_busy")?;
    context.root.verify().map_err(|e| e.to_string())?;
    Ok(lock)
}

/// Explicit foreground execution only. Unsupported platforms refuse before any
/// command is spawned; no read-only gate invokes this function.
#[cfg(not(unix))]
pub fn run(_spec_path: &Path, _repo_root: &Path, _check_id: &str) -> Result<Receipt, String> {
    Err("verification_execution_unsupported_platform".into())
}

#[cfg(unix)]
pub fn run(spec_path: &Path, repo_root: &Path, check_id: &str) -> Result<Receipt, String> {
    static RUNNING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _process_lock = RUNNING.try_lock().map_err(|_| "verification_busy")?;
    let _cancellation = Cancellation::install()?;
    let deadline = Instant::now() + crate::diff::git_timeout();
    let context = context(spec_path, repo_root, deadline)?;
    let (command, declaration) = declarations(&context.spec)
        .into_iter()
        .find(|(_, run)| run.id == check_id)
        .ok_or("observed_verification_not_declared")?;
    let _lock = acquire_lock(&context, true)?;
    let attempt = match read_receipt(&context, check_id, deadline)? {
        Some((receipt, _)) => receipt
            .attempt
            .checked_add(1)
            .ok_or("verification_attempt_overflow")?,
        None => 1,
    };
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| "verification_random_unavailable")?;
    let mut receipt = Receipt {
        schema_version: 1,
        run_id: crate::hex::encode(&random),
        check_id: check_id.into(),
        attempt,
        status: Status::Pending,
        reason: None,
        binding: context.binding.clone(),
        declaration_sha256: declaration_hash(command, declaration),
        command: command.into(),
        declaration: declaration.clone(),
        executable: None,
        snapshot_version: SNAPSHOT_VERSION,
        snapshot_before: None,
        snapshot_after: None,
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "clock_unavailable")?
            .as_secs(),
        duration_ms: 0,
        exit_code: None,
        signal: None,
        stdout: None,
        stderr: None,
        runner_version: env!("CARGO_PKG_VERSION").into(),
        platform: std::env::consts::OS.into(),
        provenance: "local_runner_unsigned".into(),
    };
    // Revoke the earlier success before preparation or spawn can fail/crash.
    write_receipt(&context, &receipt)?;
    let prepare = || -> Result<(String, Executable, PathBuf), String> {
        let snapshot = snapshot(&context.root, &context.binding.baseline_oid, deadline)?;
        let executable = resolve_executable(&context.root, declaration, deadline)?;
        let cwd = cwd(&context.root, declaration)?;
        if self::context(spec_path, repo_root, deadline)?.binding != context.binding {
            return Err("verification_task_changed".into());
        }
        Ok((snapshot, executable, cwd))
    };
    let (before, executable, directory) = match prepare() {
        Ok(inputs) => inputs,
        Err(reason) => {
            receipt.status = Status::InputChanged;
            receipt.reason = Some(reason);
            publish_receipt(&context, &mut receipt, &interrupted)?;
            return Ok(receipt);
        }
    };
    receipt.snapshot_before = Some(before);
    receipt.executable = Some(executable.clone());
    write_receipt(&context, &receipt)?;
    let observation = run_process(&executable, declaration, &directory);
    receipt.status = observation.status;
    receipt.reason = observation.reason;
    receipt.exit_code = observation.exit_code;
    receipt.signal = observation.signal;
    receipt.duration_ms = observation.duration_ms;
    receipt.stdout = observation.stdout;
    receipt.stderr = observation.stderr;
    let deadline = Instant::now() + crate::diff::git_timeout();
    let after = snapshot(&context.root, &context.binding.baseline_oid, deadline);
    let unchanged = after.as_ref().ok() == receipt.snapshot_before.as_ref()
        && self::context(spec_path, repo_root, deadline)
            .ok()
            .is_some_and(|current| current.binding == context.binding)
        && resolve_executable(&context.root, declaration, deadline)
            .as_ref()
            .ok()
            == Some(&executable);
    receipt.snapshot_after = after.ok();
    if !unchanged {
        receipt.status = Status::InputChanged;
        receipt.reason = Some("verification_inputs_changed_or_unavailable".into());
    }
    publish_receipt(&context, &mut receipt, &interrupted)?;
    Ok(receipt)
}

#[cfg(unix)]
struct Observation {
    status: Status,
    reason: Option<String>,
    exit_code: Option<i32>,
    signal: Option<i32>,
    duration_ms: u64,
    stdout: Option<StreamDigest>,
    stderr: Option<StreamDigest>,
}

#[cfg(unix)]
fn run_process(executable: &Executable, run: &RunDeclaration, cwd: &Path) -> Observation {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Command, Stdio};
    let started = Instant::now();
    let mut observation = Observation {
        status: Status::SpawnFailed,
        reason: Some("verification_spawn_failed".into()),
        exit_code: None,
        signal: None,
        duration_ms: 0,
        stdout: None,
        stderr: None,
    };
    if interrupted() {
        observation.status = Status::Interrupted;
        observation.reason = Some("verification_interrupted".into());
        return observation;
    }
    let mut command = Command::new(&executable.invocation_path);
    command
        .args(&run.argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return observation,
    };
    let mut process = OwnedProcess {
        child,
        terminated: false,
    };
    let stdout = read_pipe(process.child.stdout.take().expect("piped stdout"));
    let stderr = read_pipe(process.child.stderr.take().expect("piped stderr"));
    let mut output = [None, None];
    let pipes = [&stdout, &stderr];
    let outcome = loop {
        if interrupted() {
            break Err((Status::Interrupted, "verification_interrupted"));
        }
        if started.elapsed() >= Duration::from_secs(run.timeout_secs) {
            break Err((Status::Timeout, "verification_timeout"));
        }
        let mut pipe_error = None;
        for index in 0..2 {
            if output[index].is_none() {
                match pipes[index].try_recv() {
                    Ok(Ok(digest)) => output[index] = Some(digest),
                    Ok(Err(error)) => pipe_error = Some(error),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(_) => pipe_error = Some((Status::IoFailed, "verification_pipe_failed")),
                }
            }
        }
        if let Some(error) = pipe_error {
            break Err(error);
        }
        match process.child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(_) => break Err((Status::IoFailed, "verification_wait_failed")),
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    process.terminate();
    match outcome {
        Ok(status) => {
            observation.exit_code = status.code();
            observation.signal = status.signal();
            observation.status = if status.success() {
                Status::Passed
            } else {
                Status::Failed
            };
            observation.reason = None;
            for index in 0..2 {
                if output[index].is_none() {
                    match pipes[index].recv_timeout(Duration::from_millis(250)) {
                        Ok(Ok(digest)) => output[index] = Some(digest),
                        Ok(Err((status, reason))) => {
                            observation.status = status;
                            observation.reason = Some(reason.into());
                        }
                        Err(_) => {
                            observation.status = Status::IoFailed;
                            observation.reason = Some("verification_pipe_not_closed".into());
                        }
                    }
                }
            }
        }
        Err((status, reason)) => {
            observation.status = status;
            observation.reason = Some(reason.into());
        }
    }
    observation.duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    observation.stdout = output[0].take();
    observation.stderr = output[1].take();
    observation
}

#[cfg(unix)]
fn read_pipe<R: std::io::Read + Send + 'static>(
    reader: R,
) -> std::sync::mpsc::Receiver<Result<StreamDigest, (Status, &'static str)>> {
    use std::io::Read;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = reader
            .take(OUTPUT_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| (Status::IoFailed, "verification_pipe_failed"))
            .and_then(|_| {
                if bytes.len() > OUTPUT_LIMIT {
                    Err((Status::OutputLimit, "verification_output_limit"))
                } else {
                    Ok(StreamDigest {
                        bytes: bytes.len() as u64,
                        sha256: sha(&bytes),
                    })
                }
            });
        let _ = sender.send(result);
    });
    receiver
}

#[cfg(unix)]
struct OwnedProcess {
    child: std::process::Child,
    terminated: bool,
}

#[cfg(unix)]
impl OwnedProcess {
    fn terminate(&mut self) {
        if self.terminated {
            return;
        }
        self.terminated = true;
        // SAFETY: the child was created in its own process group. This is
        // lifecycle cleanup for that group, not containment of hostile code.
        unsafe {
            let _ = libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(unix)]
static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn interrupted() -> bool {
    #[cfg(unix)]
    {
        INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(unix)]
extern "C" fn interrupt(_signal: libc::c_int) {
    INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(unix)]
struct Cancellation {
    previous: [libc::sigaction; 2],
}

#[cfg(unix)]
impl Cancellation {
    fn install() -> Result<Self, String> {
        // SAFETY: libc initializes the saved handlers; the new handler only
        // updates an atomic flag. The run mutex serializes this registration.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            let mut previous: [libc::sigaction; 2] = std::mem::zeroed();
            action.sa_sigaction = interrupt as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            INTERRUPTED.store(false, std::sync::atomic::Ordering::Relaxed);
            if libc::sigaction(libc::SIGINT, &action, &mut previous[0]) != 0 {
                return Err("verification_signal_setup_failed".into());
            }
            if libc::sigaction(libc::SIGTERM, &action, &mut previous[1]) != 0 {
                libc::sigaction(libc::SIGINT, &previous[0], std::ptr::null_mut());
                return Err("verification_signal_setup_failed".into());
            }
            Ok(Self { previous })
        }
    }
}

#[cfg(unix)]
impl Drop for Cancellation {
    fn drop(&mut self) {
        // SAFETY: restore exactly the handlers saved for this invocation.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.previous[0], std::ptr::null_mut());
            libc::sigaction(libc::SIGTERM, &self.previous[1], std::ptr::null_mut());
        }
        INTERRUPTED.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn receipt_fixture() -> (tempfile::TempDir, Context, Receipt) {
        let directory = tempfile::tempdir().unwrap();
        let root = RootCapability::open(directory.path()).unwrap();
        root.ensure_directory(Path::new("verification")).unwrap();
        let binding = Binding {
            repository_identity: "test-repository".into(),
            spec_path: "spec.md".into(),
            spec_sha256: sha(b"spec"),
            baseline_oid: "a".repeat(40),
            iteration: 1,
            preflight_started_at: 1,
        };
        let context = Context {
            root,
            spec: crate::spec::parse_str("spec.md", ""),
            binding: binding.clone(),
            directory: directory.path().join("verification"),
        };
        let declaration = RunDeclaration {
            id: "test".into(),
            argv: vec!["/usr/bin/true".into()],
            cwd: ".".into(),
            timeout_secs: 1,
        };
        let successful = Receipt {
            schema_version: 1,
            run_id: "b".repeat(32),
            check_id: declaration.id.clone(),
            attempt: 1,
            status: Status::Passed,
            reason: None,
            binding,
            declaration_sha256: declaration_hash("/usr/bin/true", &declaration),
            command: "/usr/bin/true".into(),
            declaration,
            executable: Some(Executable {
                invocation_path: "/usr/bin/true".into(),
                resolved_path: "/usr/bin/true".into(),
                sha256: sha(b"executable"),
            }),
            snapshot_version: SNAPSHOT_VERSION,
            snapshot_before: Some(sha(b"snapshot")),
            snapshot_after: Some(sha(b"snapshot")),
            started_at: 1,
            duration_ms: 1,
            exit_code: Some(0),
            signal: None,
            stdout: Some(StreamDigest {
                bytes: 0,
                sha256: sha(b""),
            }),
            stderr: Some(StreamDigest {
                bytes: 0,
                sha256: sha(b""),
            }),
            runner_version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            provenance: "local_runner_unsigned".into(),
        };
        assert!(successful.passed());
        (directory, context, successful)
    }

    #[test]
    fn cancellation_after_process_exit_is_resolved_before_publication() {
        let (_directory, context, successful) = receipt_fixture();
        // Inject cancellation after a successful process observation and all
        // postflight reads. An uninterrupted observation can still be published.
        // No timing dependency or process-wide signal state is needed.
        for cancelled in [true, false] {
            let mut receipt = successful.clone();
            publish_receipt(&context, &mut receipt, &|| cancelled).unwrap();
            let persisted = read_receipt(&context, "test", Instant::now() + Duration::from_secs(1))
                .unwrap()
                .unwrap()
                .0;
            assert_eq!(persisted, receipt);
            if cancelled {
                assert_eq!(persisted.status, Status::Interrupted);
                assert_eq!(
                    persisted.reason.as_deref(),
                    Some("verification_interrupted")
                );
                assert!(!persisted.passed());
            } else {
                assert!(persisted.passed());
            }
        }
    }

    #[test]
    fn receipt_inspection_separates_current_from_missing_failed_stale_and_unknown_evidence() {
        let (_directory, context, mut receipt) = receipt_fixture();
        let deadline = Instant::now() + Duration::from_secs(10);
        receipt.executable =
            Some(resolve_executable(&context.root, &receipt.declaration, deadline).unwrap());
        let snapshot = receipt.snapshot_after.as_deref().unwrap();
        let inspect = |bytes: Option<&[u8]>| {
            inspect_receipt(
                &context,
                &receipt.command,
                &receipt.declaration,
                snapshot,
                bytes,
                deadline,
                InspectionMode::Audit,
            )
            .unwrap()
        };

        let missing = inspect(None);
        assert_eq!(missing.status, CheckEvidenceStatus::Missing);
        assert_eq!(missing.receipt_revision, None);
        assert_eq!(missing.run_id, None);
        assert!(current_digests(&[missing]).is_err());

        let malformed = inspect(Some(b"{ malformed /private/secret }"));
        assert_eq!(malformed.status, CheckEvidenceStatus::Unavailable);
        assert_eq!(
            malformed.receipt_revision.as_deref(),
            Some(sha(b"{ malformed /private/secret }").as_str())
        );
        assert_eq!(malformed.run_id, None);
        assert!(!serde_json::to_string(&malformed)
            .unwrap()
            .contains("/private/secret"));
        assert!(current_digests(&[malformed]).is_err());

        let bytes = serde_json::to_vec(&receipt).unwrap();
        let current = inspect(Some(&bytes));
        assert_eq!(current.status, CheckEvidenceStatus::Current);
        assert_eq!(current.reason, None);
        assert_eq!(current.run_id, Some(receipt.run_id.clone()));
        assert_eq!(
            current_digests(std::slice::from_ref(&current)).unwrap(),
            vec![sha(&bytes)]
        );
        let serialized = serde_json::to_string(&current).unwrap();
        for forbidden in [
            "/usr/bin/true",
            "test-repository",
            "stdout",
            "stderr",
            "command",
        ] {
            assert!(!serialized.contains(forbidden), "{serialized}");
        }

        for (case, status) in [
            ("pending", CheckEvidenceStatus::Pending),
            ("failed", CheckEvidenceStatus::Failed),
            ("unknown_source", CheckEvidenceStatus::Unavailable),
            ("bad_version", CheckEvidenceStatus::Unavailable),
            ("bad_run_id", CheckEvidenceStatus::Unavailable),
            ("bad_outcome", CheckEvidenceStatus::Unavailable),
            ("bad_stream", CheckEvidenceStatus::Unavailable),
            ("binding", CheckEvidenceStatus::Stale),
            ("declaration", CheckEvidenceStatus::Stale),
            ("snapshot", CheckEvidenceStatus::Stale),
            ("executable", CheckEvidenceStatus::Stale),
        ] {
            let mut changed = receipt.clone();
            match case {
                "pending" => {
                    changed.status = Status::Pending;
                    changed.reason = Some("/private/secret".into());
                }
                "failed" => {
                    changed.status = Status::Failed;
                    changed.exit_code = Some(7);
                    changed.reason = Some("/private/secret".into());
                }
                "unknown_source" => changed.provenance = "external_claim:/private/secret".into(),
                "bad_version" => changed.schema_version = 2,
                "bad_run_id" => changed.run_id = "/private/secret".into(),
                "bad_outcome" => changed.exit_code = None,
                "bad_stream" => changed.stdout.as_mut().unwrap().bytes = 1024 * 1024 + 1,
                "binding" => changed.binding.iteration += 1,
                "declaration" => changed.declaration.timeout_secs += 1,
                "snapshot" => {
                    changed.snapshot_before = Some(sha(b"different"));
                    changed.snapshot_after = changed.snapshot_before.clone();
                }
                "executable" => changed.executable.as_mut().unwrap().sha256 = sha(b"different"),
                _ => unreachable!(),
            }
            let bytes = serde_json::to_vec(&changed).unwrap();
            let observed = inspect(Some(&bytes));
            assert_eq!(observed.status, status, "{case}");
            assert_eq!(observed.receipt_revision, Some(sha(&bytes)), "{case}");
            assert!(
                current_digests(std::slice::from_ref(&observed)).is_err(),
                "{case}"
            );
            assert!(
                !serde_json::to_string(&observed)
                    .unwrap()
                    .contains("/private/secret"),
                "{case}"
            );
        }
    }

    fn repair_fixture() -> (tempfile::TempDir, ParsedSpec, Context, Receipt) {
        use std::os::unix::fs::PermissionsExt;
        let (directory, _, mut receipt) = receipt_fixture();
        let root = directory.path();
        std::fs::create_dir_all(root.join(".mastermind/tasks/001-check")).unwrap();
        std::fs::write(root.join(".gitignore"), ".mastermind/\nverification/\n").unwrap();
        std::fs::write(root.join("source.txt"), "original\n").unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("/usr/bin/git")
                .current_dir(root)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", root.join(".mastermind"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            output.stdout
        };
        git(&["init", "--quiet"]);
        git(&["add", ".gitignore", "source.txt"]);
        git(&[
            "-c",
            "user.name=Repair Test",
            "-c",
            "user.email=repair@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--quiet",
            "-m",
            "baseline",
        ]);
        let baseline = String::from_utf8(git(&["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let executable = root.join(".mastermind/check.sh");
        std::fs::write(&executable, "#!/bin/sh\nexit 7\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let body = "---\nverify:\n  - cmd: ./.mastermind/check.sh\n    run:\n      id: test\n      argv: [./.mastermind/check.sh]\n      cwd: .\n      timeout_secs: 1\n---\n# Synthetic repair receipt\n";
        let relative = ".mastermind/tasks/001-check/spec.md";
        let spec_path = root.join(relative);
        std::fs::write(&spec_path, body).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let repository = crate::facts::repository_identity_until(root, Some(deadline)).unwrap();
        let state = serde_json::json!({
            "status":"approved", "next_step":"run_executor", "spec_path":relative,
            "repository_identity":repository, "spec_hash":sha(body.as_bytes()),
            "baseline_ref":baseline, "started_at":1, "iteration":1,
        });
        std::fs::write(
            crate::run_task::state_file_path(root, &spec_path),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let context = context(&spec_path, root, deadline).unwrap();
        let (command, declaration) = declarations(&context.spec)[0];
        receipt.command = command.into();
        receipt.declaration = declaration.clone();
        receipt.declaration_sha256 = declaration_hash(command, declaration);
        receipt.binding = context.binding.clone();
        receipt.executable =
            Some(resolve_executable(&context.root, declaration, deadline).unwrap());
        receipt.snapshot_before = Some(snapshot(&context.root, &baseline, deadline).unwrap());
        receipt.snapshot_after = receipt.snapshot_before.clone();
        receipt.status = Status::Failed;
        receipt.exit_code = Some(7);
        let lock = acquire_lock(&context, true).unwrap();
        write_receipt(&context, &receipt).unwrap();
        drop(lock);
        let parsed = context.spec.clone();
        (directory, parsed, context, receipt)
    }

    #[test]
    fn repair_checks_require_fresh_failed_snapshot_and_executable() {
        let (directory, spec, context, receipt) = repair_fixture();
        let root = directory.path();
        let deadline = Instant::now() + Duration::from_secs(20);
        let baseline = &receipt.binding.baseline_oid;
        let revisions = repair_executable_revisions(&spec, root, baseline, deadline).unwrap();
        assert_eq!(revisions.len(), 1);
        assert_eq!(
            revisions["test"],
            sha(&serde_json::to_vec(receipt.executable.as_ref().unwrap()).unwrap())
        );
        assert!(valid_hex(&revisions["test"], 64));
        let check = inspect_repair_checks(&spec, root, baseline, deadline)
            .unwrap()
            .remove(0);
        assert_eq!(check.status, CheckEvidenceStatus::Failed);
        assert_eq!(check.reason.as_deref(), Some("receipt_failed"));

        std::fs::write(root.join("source.txt"), "changed\n").unwrap();
        let check = inspect_repair_checks(&spec, root, baseline, deadline)
            .unwrap()
            .remove(0);
        assert_eq!(check.status, CheckEvidenceStatus::Stale);
        assert_eq!(check.reason.as_deref(), Some("receipt_snapshot_changed"));
        let ordinary = inspect_checks(&spec, root, baseline, deadline)
            .unwrap()
            .remove(0);
        assert_eq!(ordinary.status, CheckEvidenceStatus::Failed);
        assert_eq!(ordinary.reason.as_deref(), Some("receipt_failed"));
        assert_eq!(
            repair_executable_revisions(&spec, root, baseline, deadline).unwrap(),
            revisions
        );

        std::fs::write(root.join("source.txt"), "original\n").unwrap();
        std::fs::write(root.join(".mastermind/check.sh"), "#!/bin/sh\nexit 8\n").unwrap();
        let check = inspect_repair_checks(&spec, root, baseline, deadline)
            .unwrap()
            .remove(0);
        assert_eq!(check.status, CheckEvidenceStatus::Stale);
        assert_eq!(check.reason.as_deref(), Some("receipt_executable_changed"));
        assert_ne!(
            repair_executable_revisions(&spec, root, baseline, deadline).unwrap(),
            revisions
        );
        assert_eq!(
            read_receipt(&context, "test", deadline).unwrap().unwrap().0,
            receipt
        );

        std::fs::remove_file(root.join(".mastermind/check.sh")).unwrap();
        assert!(inspect_repair_checks(&spec, root, baseline, deadline).is_err());
        assert!(repair_executable_revisions(&spec, root, baseline, deadline).is_err());
        assert!(repair_executable_revisions(&spec, root, baseline, Instant::now()).is_err());
    }

    #[test]
    fn repair_input_revision_detects_assumed_unchanged_files_and_ignores_controller_iteration() {
        let (directory, spec, _context, receipt) = repair_fixture();
        let root = directory.path();
        let baseline = &receipt.binding.baseline_oid;
        let deadline = Instant::now() + Duration::from_secs(20);
        let before = repair_input_revision(root, baseline, deadline).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("/usr/bin/git")
                .current_dir(root)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", root.join(".mastermind"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            output.stdout
        };
        git(&["update-index", "--assume-unchanged", "source.txt"]);
        assert_eq!(
            repair_input_revision(root, baseline, deadline).unwrap(),
            before
        );
        std::fs::write(root.join("source.txt"), "hidden dependency mutation\n").unwrap();
        assert!(git(&[
            "diff",
            "--name-only",
            "--no-ext-diff",
            baseline,
            "--",
            "source.txt"
        ])
        .is_empty());
        let changed = repair_input_revision(root, baseline, deadline).unwrap();
        assert_ne!(changed, before);

        // Re-approval metadata does not alter the input digest. The baseline
        // argument stays pinned independently of this mutable state file.
        let state_path = crate::run_task::state_file_path(root, &root.join(&spec.path));
        let mut state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
        state["iteration"] = serde_json::json!(2);
        state["baseline_ref"] = serde_json::json!("f".repeat(40));
        std::fs::write(state_path, serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(
            repair_input_revision(root, baseline, deadline).unwrap(),
            changed
        );
        assert!(repair_input_revision(root, baseline, Instant::now()).is_err());
    }

    #[test]
    fn repair_checks_reject_malformed_failure_and_abnormal_termination() {
        let (_directory, context, mut receipt) = receipt_fixture();
        let deadline = Instant::now() + Duration::from_secs(10);
        receipt.executable =
            Some(resolve_executable(&context.root, &receipt.declaration, deadline).unwrap());
        receipt.status = Status::Failed;
        receipt.exit_code = Some(7);
        for case in [
            "zero",
            "missing_exit",
            "negative_exit",
            "oversized_exit",
            "signal",
            "reason",
            "missing_before",
            "mismatched_snapshots",
            "missing_executable",
            "missing_stdout",
            "oversized_stream",
            "invalid_digest",
            "unknown_source",
            "unknown_schema",
            "pending",
            "timeout",
            "interrupted",
            "output_limit",
            "spawn_failed",
            "io_failed",
        ] {
            let mut changed = receipt.clone();
            match case {
                "zero" => changed.exit_code = Some(0),
                "missing_exit" => changed.exit_code = None,
                "negative_exit" => changed.exit_code = Some(-1),
                "oversized_exit" => changed.exit_code = Some(256),
                "signal" => changed.signal = Some(libc::SIGTERM),
                "reason" => changed.reason = Some("/private/untrusted-output".into()),
                "missing_before" => changed.snapshot_before = None,
                "mismatched_snapshots" => changed.snapshot_before = Some(sha(b"different")),
                "missing_executable" => changed.executable = None,
                "missing_stdout" => changed.stdout = None,
                "oversized_stream" => changed.stderr.as_mut().unwrap().bytes = 1024 * 1024 + 1,
                "invalid_digest" => changed.stderr.as_mut().unwrap().sha256 = "invalid".into(),
                "unknown_source" => changed.provenance = "unknown".into(),
                "unknown_schema" => changed.schema_version = 2,
                "pending" => changed.status = Status::Pending,
                "timeout" => changed.status = Status::Timeout,
                "interrupted" => changed.status = Status::Interrupted,
                "output_limit" => changed.status = Status::OutputLimit,
                "spawn_failed" => changed.status = Status::SpawnFailed,
                "io_failed" => changed.status = Status::IoFailed,
                _ => unreachable!(),
            }
            let bytes = serde_json::to_vec(&changed).unwrap();
            let check = inspect_receipt(
                &context,
                &receipt.command,
                &receipt.declaration,
                receipt.snapshot_after.as_deref().unwrap(),
                Some(&bytes),
                deadline,
                InspectionMode::Repair,
            )
            .unwrap();
            assert!(
                !matches!(
                    check.status,
                    CheckEvidenceStatus::Current | CheckEvidenceStatus::Failed
                ),
                "{case}: {check:?}"
            );
            assert_ne!(check.reason.as_deref(), Some("receipt_failed"), "{case}");
            assert!(!serde_json::to_string(&check)
                .unwrap()
                .contains("/private/untrusted-output"));
        }
    }
}
