//! A bounded native hook policy, not an OS sandbox. Command-hook failures may
//! fall through to native permissions. Reconciliation refuses successful
//! receipts when an observed call has no matching durable allow decision.

use super::*;
#[cfg(unix)]
use serde_json::{json, Value};
#[cfg(unix)]
use std::collections::BTreeMap;
#[cfg(unix)]
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: u32 = 1;
#[cfg(unix)]
const MANIFEST_LIMIT: u64 = 128 * 1024;
#[cfg(unix)]
const LEDGER_LIMIT: u64 = 256 * 1024;
#[cfg(unix)]
const EVENT_LIMIT: usize = 128 * 1024;
const CALL_LIMIT: usize = 512;
#[cfg(unix)]
const PATH_LIMIT: usize = 4096;
const COVERAGE: &str = "observed_native_tool_use_only";
const ENFORCEMENT: &str = "conditional_native_hook_no_os_sandbox_command_failure_fallback";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub schema_version: u32,
    pub manifest_sha256: String,
    pub native_session_id: String,
    pub decisions_sha256: Option<String>,
    pub observed_calls: usize,
    pub allowed_calls: Option<usize>,
    pub denied_calls: Option<usize>,
    pub reconciled: bool,
    pub coverage: String,
    pub enforcement: String,
}

impl Evidence {
    pub(super) fn complete(&self) -> bool {
        self.schema_version == VERSION
            && hex(&self.manifest_sha256, 64)
            && valid_session(&self.native_session_id)
            && self.decisions_sha256.as_deref().is_some_and(|v| hex(v, 64))
            && self.observed_calls <= CALL_LIMIT
            && self.allowed_calls == Some(self.observed_calls)
            && self.denied_calls == Some(0)
            && self.reconciled
            && self.coverage == COVERAGE
            && self.enforcement == ENFORCEMENT
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    root: String,
    invocation_id: String,
    session_id: String,
    binding: Binding,
    policy_sha256: String,
    expires_at: u64,
    runner: Executable,
    write_paths: Vec<String>,
    commands: BTreeMap<String, String>,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    name: String,
    input_sha256: String,
    allowed: bool,
    reason: String,
}

#[cfg(unix)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    schema_version: u32,
    manifest_sha256: String,
    conflict: bool,
    decisions: BTreeMap<String, Decision>,
}

#[cfg(unix)]
pub(super) struct Prepared {
    manifest: Manifest,
    path: PathBuf,
    digest: String,
}

#[cfg(unix)]
fn paths(receipt: &Path) -> (PathBuf, PathBuf, PathBuf) {
    (
        receipt.with_extension("guard.json"),
        receipt.with_extension("guard-decisions.json"),
        receipt.with_extension("guard.lock"),
    )
}

#[cfg(unix)]
fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .map_err(|_| "invocation_guard_clock_unavailable".into())
}

fn valid_session(value: &str) -> bool {
    value.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|i| value.as_bytes()[i] == b'-')
        && value
            .bytes()
            .enumerate()
            .all(|(i, b)| [8, 13, 18, 23].contains(&i) || b.is_ascii_hexdigit())
}

#[cfg(unix)]
fn read_json<T: serde::de::DeserializeOwned>(
    root: &RootCapability,
    path: &Path,
    cap: u64,
) -> Result<T, String> {
    let bytes = read(root, path, cap, Instant::now() + Duration::from_secs(2))?;
    let value =
        crate::setup::parse_json_unique(&bytes).map_err(|_| "invocation_guard_invalid_json")?;
    serde_json::from_value(value).map_err(|_| "invocation_guard_invalid_schema".into())
}

#[cfg(unix)]
fn write_json(
    root: &RootCapability,
    path: &Path,
    value: &impl Serialize,
    cap: u64,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "invocation_guard_serialization_failed")?;
    if bytes.len() as u64 > cap {
        return Err("invocation_guard_output_limit".into());
    }
    bounded_fs::write_atomic_regular_file_expected_with_capability_mode(
        root,
        path,
        &bytes,
        0o600,
        AtomicWriteExpectation::Any,
    )
    .map_err(|_| "invocation_guard_write_failed".into())
}

#[cfg(unix)]
fn runner() -> Result<Executable, String> {
    let path = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|_| "invocation_guard_runner_unavailable")?;
    let parent = RootCapability::open(path.parent().ok_or("invocation_guard_runner_unavailable")?)
        .map_err(|_| "invocation_guard_runner_unavailable")?;
    let bytes = read(
        &parent,
        &path,
        EXECUTABLE_LIMIT,
        Instant::now() + Duration::from_secs(3),
    )?;
    let path = path
        .to_str()
        .ok_or("invocation_guard_runner_path_invalid")?
        .to_owned();
    Ok(Executable {
        invocation_path: path.clone(),
        resolved_path: path,
        sha256: sha(&bytes),
    })
}

#[cfg(unix)]
fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(unix)]
fn protected(path: &str) -> bool {
    path.split('/').any(|part| {
        matches!(
            part.to_ascii_lowercase().as_str(),
            ".git" | ".claude" | ".codex" | ".mastermind"
        )
    }) || matches!(
        path.to_ascii_lowercase().as_str(),
        "claude.md" | "agents.md" | ".mcp.json"
    )
}

#[cfg(unix)]
impl Prepared {
    pub(super) fn new(task: &Task, receipt: &mut InvocationReceipt) -> Result<Self, String> {
        let fm = task
            .spec
            .frontmatter
            .as_ref()
            .filter(|fm| fm.has_file_scope())
            .ok_or("invocation_guard_explicit_scope_required")?;
        if fm
            .verify
            .iter()
            .any(|entry| matches!(entry, crate::spec::VerifyEntry::Command { .. }))
        {
            return Err("invocation_guard_observed_commands_required".into());
        }
        crate::verification_receipts::validate_declarations(&task.spec)?;
        let mut write_paths = crate::declared_files::paths(&task.spec)
            .map(crate::declared_files::normalize)
            .collect::<Result<Vec<_>, _>>()?;
        if write_paths.len() > 1024
            || write_paths
                .iter()
                .any(|p| protected(p) || p == &task.binding.spec_path)
        {
            return Err("invocation_guard_protected_scope".into());
        }
        let report = Path::new(&task.binding.spec_path).with_file_name("executor-report.md");
        write_paths.push(
            bounded_fs::normalize_repository_relative_path(&report)
                .map_err(|_| "invocation_guard_report_path_invalid")?,
        );
        write_paths.sort();
        write_paths.dedup();
        let runner = runner()?;
        let root = task
            .root
            .canonical_root()
            .to_str()
            .ok_or("invocation_guard_root_invalid")?
            .to_owned();
        let mut commands = BTreeMap::new();
        for entry in &fm.verify {
            if let crate::spec::VerifyEntry::Observed { run, .. } = entry {
                let spec_argument = format!("./{}", task.binding.spec_path);
                let id_argument = format!("--id={}", run.id);
                let command = [
                    runner.invocation_path.as_str(),
                    "verification",
                    "run",
                    &spec_argument,
                    "--root",
                    &root,
                    &id_argument,
                    "--json",
                ]
                .into_iter()
                .map(shell_word)
                .collect::<Vec<_>>()
                .join(" ");
                commands.insert(command, run.id.clone());
            }
        }
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| "invocation_guard_random_unavailable")?;
        random[6] = (random[6] & 15) | 0x40;
        random[8] = (random[8] & 63) | 0x80;
        let random = crate::hex::encode(&random);
        let session_id = format!(
            "{}-{}-{}-{}-{}",
            &random[..8],
            &random[8..12],
            &random[12..16],
            &random[16..20],
            &random[20..]
        );
        let manifest = Manifest {
            schema_version: VERSION,
            root,
            invocation_id: receipt.invocation_id.clone(),
            session_id,
            binding: task.binding.clone(),
            policy_sha256: receipt.policy_sha256.clone(),
            expires_at: now()?
                .checked_add(receipt.options.wall_timeout_secs + 30)
                .ok_or("invocation_guard_deadline_invalid")?,
            runner,
            write_paths,
            commands,
        };
        let digest = json_sha(&manifest)?;
        let (path, ledger, _) = paths(&task.receipt_path);
        write_json(&task.root, &path, &manifest, MANIFEST_LIMIT)?;
        write_json(
            &task.root,
            &ledger,
            &Ledger {
                schema_version: VERSION,
                manifest_sha256: digest.clone(),
                conflict: false,
                decisions: BTreeMap::new(),
            },
            LEDGER_LIMIT,
        )?;
        receipt.mediation = Some(Evidence {
            schema_version: VERSION,
            manifest_sha256: digest.clone(),
            native_session_id: manifest.session_id.clone(),
            decisions_sha256: None,
            observed_calls: 0,
            allowed_calls: None,
            denied_calls: None,
            reconciled: false,
            coverage: COVERAGE.into(),
            enforcement: ENFORCEMENT.into(),
        });
        Ok(Self {
            manifest,
            path,
            digest,
        })
    }

    pub(super) fn prompt(&self) -> Result<Vec<u8>, String> {
        Ok(format!("Guarded native execution is opted in. Only the following exact Bash strings may invoke the existing declared verification runner. Do not wrap, append, redirect, background or substitute these strings. All other Bash and MCP calls are denied. Edit/Write may target only declared files and this task's executor-report.md. A denied action must stay unresolved. These hooks are not an OS sandbox; native command-hook failure can fall back to native permissions, and missing mediation prevents an accepted invocation receipt.\n<mastermind-guard-commands-json>\n{}\n</mastermind-guard-commands-json>\n\n", serde_json::to_string(&self.manifest.commands).map_err(|_| "invocation_guard_serialization_failed")?).into_bytes())
    }

    pub(super) fn args(&self) -> Result<Vec<String>, String> {
        let settings = json!({"disableAllHooks":false,"disableClaudeAiConnectors":true,
            "hooks":{"PreToolUse":[{"matcher":"*","hooks":[{"type":"command","command":self.manifest.runner.invocation_path,
                "args":["invocation","guard","--manifest",self.path],"timeout":5,
                "statusMessage":"Mastermind: Check task tool boundary"}]}]}});
        Ok(vec![
            "--restricted".into(),
            "--setting-sources".into(),
            "".into(),
            "--settings".into(),
            serde_json::to_string(&settings)
                .map_err(|_| "invocation_guard_serialization_failed")?,
            "--strict-mcp-config".into(),
            "--mcp-config".into(),
            "{\"mcpServers\":{}}".into(),
            "--disable-slash-commands".into(),
            "--session-id".into(),
            self.manifest.session_id.clone(),
        ])
    }

    pub(super) fn session(&self) -> &str {
        &self.manifest.session_id
    }

    pub(super) fn reconcile(
        &self,
        observed: &BTreeMap<String, (String, String)>,
    ) -> (Evidence, Option<String>) {
        let mut evidence = Evidence {
            schema_version: VERSION,
            manifest_sha256: self.digest.clone(),
            native_session_id: self.manifest.session_id.clone(),
            decisions_sha256: None,
            observed_calls: observed.len(),
            allowed_calls: None,
            denied_calls: None,
            reconciled: false,
            coverage: COVERAGE.into(),
            enforcement: ENFORCEMENT.into(),
        };
        let result = self.reconcile_into(observed, &mut evidence);
        (evidence, result.err())
    }

    fn reconcile_into(
        &self,
        observed: &BTreeMap<String, (String, String)>,
        evidence: &mut Evidence,
    ) -> Result<(), String> {
        let root = RootCapability::open(Path::new(&self.manifest.root))
            .map_err(|_| "invocation_guard_root_unavailable")?;
        let manifest: Manifest = read_json(&root, &self.path, MANIFEST_LIMIT)?;
        if json_sha(&manifest)? != self.digest {
            return Err("invocation_guard_manifest_changed".into());
        }
        let receipt = receipt_path(
            root.canonical_root(),
            Path::new(&manifest.binding.spec_path),
        );
        let ledger: Ledger = read_json(&root, &paths(&receipt).1, LEDGER_LIMIT)?;
        if ledger.schema_version != VERSION
            || ledger.manifest_sha256 != self.digest
            || ledger.decisions.len() > CALL_LIMIT
        {
            return Err("invocation_guard_decisions_invalid".into());
        }
        // A failed native run still has useful observed coverage. Counts are
        // unknown only when its bound ledger could not be read or validated.
        evidence.decisions_sha256 = Some(json_sha(&ledger)?);
        evidence.allowed_calls = Some(ledger.decisions.values().filter(|d| d.allowed).count());
        evidence.denied_calls = Some(ledger.decisions.values().filter(|d| !d.allowed).count());
        if ledger.conflict
            || ledger.decisions.len() != observed.len()
            || observed.len() > CALL_LIMIT
            || observed.iter().any(|(id, (name, input))| {
                ledger
                    .decisions
                    .get(id)
                    .is_none_or(|d| !d.allowed || &d.name != name || &d.input_sha256 != input)
            })
        {
            return Err("invocation_guard_mediation_incomplete".into());
        }
        evidence.reconciled = true;
        Ok(())
    }
}

#[cfg(unix)]
pub(super) fn observe(
    calls: &mut BTreeMap<String, (String, String)>,
    block: &Value,
) -> Result<(), &'static str> {
    let id = block
        .get("id")
        .and_then(Value::as_str)
        .filter(|v| identifier(v, 256))
        .ok_or("invocation_guard_tool_id_invalid")?;
    let name = block
        .get("name")
        .and_then(Value::as_str)
        .filter(|v| identifier(v, 128))
        .ok_or("invocation_guard_tool_name_invalid")?;
    if name == "EndConversation" {
        return Ok(());
    } // Native event explicitly skips hooks.
    let input = block
        .get("input")
        .filter(|v| v.is_object())
        .ok_or("invocation_guard_tool_input_invalid")?;
    if calls.len() >= CALL_LIMIT
        || calls
            .insert(
                id.into(),
                (
                    name.into(),
                    json_sha(input).map_err(|_| "invocation_guard_tool_input_invalid")?,
                ),
            )
            .is_some()
    {
        return Err("invocation_guard_duplicate_or_excess_tool_calls");
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn validate_artifacts(task: &Task, receipt: &InvocationReceipt) -> Result<(), String> {
    let evidence = receipt
        .mediation
        .as_ref()
        .filter(|v| v.complete())
        .ok_or("invocation_guard_evidence_missing")?;
    let (path, ledger, _) = paths(&task.receipt_path);
    let manifest: Manifest = read_json(&task.root, &path, MANIFEST_LIMIT)?;
    let ledger: Ledger = read_json(&task.root, &ledger, LEDGER_LIMIT)?;
    if manifest.schema_version != VERSION
        || manifest.binding != receipt.binding
        || manifest.root != receipt.root
        || manifest.invocation_id != receipt.invocation_id
        || manifest.policy_sha256 != receipt.policy_sha256
        || manifest.session_id != evidence.native_session_id
        || json_sha(&manifest)? != evidence.manifest_sha256
        || ledger.manifest_sha256 != evidence.manifest_sha256
        || json_sha(&ledger)? != evidence.decisions_sha256.clone().unwrap_or_default()
        || ledger.schema_version != VERSION
        || ledger.conflict
        || Some(ledger.decisions.len()) != evidence.allowed_calls
        || ledger.decisions.values().any(|d| !d.allowed)
    {
        return Err("invocation_guard_evidence_changed".into());
    }
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn validate_artifacts(_task: &Task, _receipt: &InvocationReceipt) -> Result<(), String> {
    Err("invocation_runtime_unsupported_platform".into())
}

#[cfg(unix)]
fn current(
    root: &RootCapability,
    path: &Path,
    manifest: &Manifest,
    digest: &str,
) -> Result<(), String> {
    if manifest.schema_version != VERSION
        || !valid_session(&manifest.session_id)
        || manifest.expires_at < now()?
        || manifest.root != root.canonical_root().to_string_lossy()
    {
        return Err("invocation_guard_expired_or_invalid".into());
    }
    let receipt_path = receipt_path(
        root.canonical_root(),
        Path::new(&manifest.binding.spec_path),
    );
    if path != paths(&receipt_path).0 {
        return Err("invocation_guard_manifest_path_mismatch".into());
    }
    let receipt: InvocationReceipt = read_json(root, &receipt_path, RECEIPT_LIMIT)?;
    if receipt.status != Status::Pending
        || !receipt.options.guarded
        || receipt.schema_version != 2
        || receipt.binding != manifest.binding
        || receipt.invocation_id != manifest.invocation_id
        || receipt.root != manifest.root
        || receipt.policy != guarded_policy()
        || receipt.policy_sha256 != manifest.policy_sha256
        || receipt.mediation.as_ref().is_none_or(|e| {
            e.manifest_sha256 != digest || e.native_session_id != manifest.session_id
        })
    {
        return Err("invocation_guard_inactive_binding".into());
    }
    let name = receipt_path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("invocation_guard_lock_invalid")?;
    let lock_path = receipt_path.with_file_name(format!("{name}.lock"));
    if bounded_fs::try_locked_existing_regular_file_with_capability(root, &lock_path)
        .map_err(|_| "invocation_guard_owner_unavailable")?
        .is_some()
    {
        return Err("invocation_guard_owner_not_running".into());
    }
    let state_path = crate::run_task::state_file_path(
        root.canonical_root(),
        Path::new(&manifest.binding.spec_path),
    );
    let state = crate::run_task::parse_run_state(&read(
        root,
        &state_path,
        RECEIPT_LIMIT,
        Instant::now() + Duration::from_secs(1),
    )?)?;
    if !matches!(state.status.as_str(), "approved" | "executing")
        || state.next_step.as_deref() == Some("run_preflight")
        || state.spec_hash != manifest.binding.spec_sha256
        || state.baseline_ref != manifest.binding.baseline_oid
        || state.iteration != manifest.binding.iteration
        || state.started_at != manifest.binding.preflight_started_at
        || state.intake_revision != manifest.binding.intake_revision
    {
        return Err("invocation_guard_task_changed".into());
    }
    crate::run_task::validate_bound_state_identity(
        &manifest.binding.repository_identity,
        &manifest.binding.spec_path,
        &state,
    )?;
    crate::run_task::validate_intake_binding(
        root.canonical_root(),
        Path::new(&manifest.binding.spec_path),
        &state,
    )?;
    if sha(&read(
        root,
        Path::new(&manifest.binding.spec_path),
        SPEC_LIMIT,
        Instant::now() + Duration::from_secs(1),
    )?) != manifest.binding.spec_sha256
        || runner()? != manifest.runner
    {
        return Err("invocation_guard_inputs_changed".into());
    }
    Ok(())
}

#[cfg(unix)]
fn only_fields(input: &Value, fields: &[&str]) -> bool {
    input
        .as_object()
        .is_some_and(|object| object.keys().all(|key| fields.contains(&key.as_str())))
}

#[cfg(unix)]
fn checked_path(root: &RootCapability, path: &str, write: bool) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    if path.is_empty() || path.len() > PATH_LIMIT || path.chars().any(char::is_control) {
        return Err("guard_path_invalid".into());
    }
    if !write && (path == "." || Path::new(path) == root.canonical_root()) {
        return Ok(String::new());
    }
    let relative = root
        .repository_relative(Path::new(path))
        .map_err(|_| "guard_path_outside_root")?;
    let relative =
        crate::declared_files::normalize(relative.to_str().ok_or("guard_path_invalid")?)?;
    let control = ReadControl {
        deadline: Some(Instant::now() + Duration::from_secs(1)),
        interrupted: None,
    };
    let receipt =
        match bounded_fs::inspect_path_receipt_with_capability(root, Path::new(&relative), control)
        {
            Ok(receipt) => Some(receipt),
            Err(bounded_fs::BoundedReadError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(_) => return Err("guard_path_unavailable".into()),
        };
    if let Some(receipt) = receipt {
        if write && receipt.kind != bounded_fs::BoundedPathKind::RegularFile {
            return Err("guard_write_not_regular".into());
        }
        // Native tools use a pathname after this check; this refuses static
        // hardlink aliases, without claiming atomic enforcement of native I/O.
        let metadata = std::fs::symlink_metadata(root.canonical_root().join(&relative))
            .map_err(|_| "guard_path_unavailable")?;
        if metadata.file_type().is_symlink() || (metadata.is_file() && metadata.nlink() != 1) {
            return Err("guard_path_alias".into());
        }
        if bounded_fs::inspect_path_receipt_with_capability(root, Path::new(&relative), control)
            .map_err(|_| "guard_path_unavailable")?
            .identity
            != receipt.identity
        {
            return Err("guard_path_changed".into());
        }
    } else if !write
        || bounded_fs::inspect_absent_path(root, Path::new(&relative), control)
            .map_err(|_| "guard_path_unavailable")?
            .is_none()
    {
        return Err("guard_path_unavailable".into());
    }
    Ok(relative)
}

#[cfg(unix)]
fn authorize(
    root: &RootCapability,
    manifest: &Manifest,
    name: &str,
    input: &Value,
) -> Result<(), String> {
    let text = |field| {
        input
            .get(field)
            .and_then(Value::as_str)
            .ok_or("guard_tool_input_invalid")
    };
    match name {
        "Edit" | "Write" => {
            let fields: &[&str] = if name == "Edit" {
                &["file_path", "old_string", "new_string", "replace_all"]
            } else {
                &["file_path", "content"]
            };
            if !only_fields(input, fields) {
                return Err("guard_tool_input_unknown".into());
            }
            if name == "Edit" {
                text("old_string")?;
                text("new_string")?;
                if input.get("replace_all").is_some_and(|v| !v.is_boolean()) {
                    return Err("guard_tool_input_invalid".into());
                }
            } else {
                text("content")?;
            }
            let path = checked_path(root, text("file_path")?, true)?;
            let report =
                Path::new(&manifest.binding.spec_path).with_file_name("executor-report.md");
            if !manifest.write_paths.contains(&path)
                || (protected(&path) && Path::new(&path) != report)
                || path == manifest.binding.spec_path
            {
                return Err("guard_write_outside_scope_or_protected".into());
            }
        }
        "Read" => {
            if !only_fields(input, &["file_path", "offset", "limit", "pages"]) {
                return Err("guard_tool_input_unknown".into());
            }
            checked_path(root, text("file_path")?, false)?;
        }
        "Grep" | "Glob" => {
            let fields: &[&str] = if name == "Glob" {
                &["path", "pattern"]
            } else {
                &[
                    "path",
                    "pattern",
                    "glob",
                    "type",
                    "output_mode",
                    "-A",
                    "-B",
                    "-C",
                    "context",
                    "-n",
                    "-i",
                    "multiline",
                    "head_limit",
                    "offset",
                ]
            };
            if !only_fields(input, fields) {
                return Err("guard_tool_input_unknown".into());
            }
            text("pattern")?;
            checked_path(
                root,
                input
                    .get("path")
                    .map(|_| text("path"))
                    .transpose()?
                    .unwrap_or("."),
                false,
            )?;
            for pattern in [
                input.get("glob").and_then(Value::as_str),
                (name == "Glob").then(|| text("pattern").ok()).flatten(),
            ]
            .into_iter()
            .flatten()
            {
                if pattern.starts_with('/')
                    || pattern.contains('\\')
                    || pattern.split('/').any(|p| p == "..")
                {
                    return Err("guard_search_path_escape".into());
                }
            }
        }
        "Bash" => {
            if !only_fields(
                input,
                &[
                    "command",
                    "description",
                    "timeout",
                    "run_in_background",
                    "dangerouslyDisableSandbox",
                ],
            ) || input.get("run_in_background").is_some_and(|v| v != false)
                || input
                    .get("dangerouslyDisableSandbox")
                    .is_some_and(|v| v != false)
                || !manifest.commands.contains_key(text("command")?)
            {
                return Err("guard_shell_not_declared_runner".into());
            }
        }
        _ => return Err("guard_tool_not_supported".into()),
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn hook(path: &Path) -> Result<bool, String> {
    if !path.is_absolute() {
        return Err("invocation_guard_manifest_path_invalid".into());
    }
    let parent = RootCapability::open(
        path.parent()
            .ok_or("invocation_guard_manifest_path_invalid")?,
    )
    .map_err(|_| "invocation_guard_manifest_unavailable")?;
    let manifest: Manifest = read_json(&parent, path, MANIFEST_LIMIT)?;
    let root = RootCapability::open(Path::new(&manifest.root))
        .map_err(|_| "invocation_guard_root_unavailable")?;
    let digest = json_sha(&manifest)?;
    current(&root, path, &manifest, &digest)?;
    let bytes = read_event()?;
    let event =
        crate::setup::parse_json_unique(&bytes).map_err(|_| "invocation_guard_input_invalid")?;
    let text = |field| {
        event
            .get(field)
            .and_then(Value::as_str)
            .ok_or("invocation_guard_input_invalid")
    };
    if !only_fields(
        &event,
        &[
            "session_id",
            "prompt_id",
            "transcript_path",
            "cwd",
            "permission_mode",
            "hook_event_name",
            "tool_name",
            "tool_input",
            "tool_use_id",
            "agent_id",
            "agent_type",
        ],
    ) || text("hook_event_name")? != "PreToolUse"
        || text("session_id")? != manifest.session_id
        || text("cwd")? != manifest.root
        || event.get("agent_id").is_some_and(|v| !v.is_null())
        || event.get("agent_type").is_some_and(|v| !v.is_null())
        || event.get("permission_mode").is_some_and(|v| v != "dontAsk")
    {
        return Err("invocation_guard_event_binding_mismatch".into());
    }
    let id = text("tool_use_id")?;
    let name = text("tool_name")?;
    if !identifier(id, 256) || !identifier(name, 128) {
        return Err("invocation_guard_tool_id_invalid".into());
    }
    let input = event
        .get("tool_input")
        .filter(|v| v.is_object())
        .ok_or("invocation_guard_input_invalid")?;
    let authorization = authorize(&root, &manifest, name, input);
    let decision = Decision {
        name: name.into(),
        input_sha256: json_sha(input)?,
        allowed: authorization.is_ok(),
        reason: authorization
            .err()
            .unwrap_or_else(|| "guard_supported_call_allowed".into()),
    };
    let receipt = receipt_path(
        root.canonical_root(),
        Path::new(&manifest.binding.spec_path),
    );
    let (_, ledger_path, lock_path) = paths(&receipt);
    let _lock = bounded_fs::try_locked_regular_file_with_capability(&root, &lock_path)
        .map_err(|_| "invocation_guard_decision_busy")?;
    current(&root, path, &manifest, &digest)?;
    if json_sha(&read_json::<Manifest>(&root, path, MANIFEST_LIMIT)?)? != digest {
        return Err("invocation_guard_manifest_changed".into());
    }
    let mut ledger: Ledger = read_json(&root, &ledger_path, LEDGER_LIMIT)?;
    if ledger.schema_version != VERSION
        || ledger.manifest_sha256 != digest
        || ledger.conflict
        || ledger.decisions.len() > CALL_LIMIT
    {
        return Err("invocation_guard_decisions_invalid".into());
    }
    match ledger.decisions.get(id) {
        Some(prior) if prior != &decision => {
            ledger.conflict = true;
        }
        Some(_) => {}
        None if ledger.decisions.len() == CALL_LIMIT => {
            ledger.conflict = true;
        }
        None => {
            ledger.decisions.insert(id.into(), decision.clone());
        }
    }
    write_json(&root, &ledger_path, &ledger, LEDGER_LIMIT)?;
    let allowed = decision.allowed && !ledger.conflict;
    println!(
        "{}",
        json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":if allowed {"allow"} else {"deny"},
        "permissionDecisionReason":if ledger.conflict {"guard_tool_id_conflict"} else {&decision.reason}}})
    );
    Ok(allowed)
}

#[cfg(unix)]
fn read_event() -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("invocation_guard_input_timeout")?;
        let mut poll = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // The hook owns stdin; poll bounds a stalled or slowly streamed payload.
        let ready = unsafe { libc::poll(&mut poll, 1, remaining.as_millis().min(100) as i32) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err("invocation_guard_input_unavailable".into());
        }
        if ready == 0 {
            continue;
        }
        if poll.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err("invocation_guard_input_unavailable".into());
        }
        let count =
            unsafe { libc::read(libc::STDIN_FILENO, chunk.as_mut_ptr().cast(), chunk.len()) };
        if count < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err("invocation_guard_input_unavailable".into());
        }
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len() + count as usize > EVENT_LIMIT {
            return Err("invocation_guard_input_limit".into());
        }
        bytes.extend_from_slice(&chunk[..count as usize]);
    }
}

#[cfg(not(unix))]
pub(super) fn hook(_path: &Path) -> Result<bool, String> {
    Err("invocation_guard_unsupported_platform".into())
}
