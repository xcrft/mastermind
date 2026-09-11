//! `mastermind run-task` — deterministic shell around the probabilistic agents.
//!
//! Two-phase orchestrator wrapping the mastermind workflow in mechanical gates:
//!
//! 1. `verify_spec` — pre-flight: missing symbols/files, snapshot drift,
//!    FIND-block staleness, VERIFY-command resolvability.
//! 2. **Risk report** — blast-radius totals, dependency-cycle membership of
//!    mentioned files, top centrality of snapshot symbols.
//! 3. Executor — hand-off message by default; `--exec` shells out to `claude -p`.
//! 4. `audit_spec` — post-flight drift: scope creep, snapshot drift, silent
//!    removals, missing planned tests.
//! 5. **Release notes draft** — H1 + Goals + Tests Plan + `git diff --stat` of
//!    baseline-to-worktree. To stdout AND `.mastermind/releases/<basename>.md` on Held.
//!
//! State persists beside a canonical task spec as `<task>/state.json`, so every
//! task has one controller-owned lifecycle record. Legacy flat specs keep using
//! `.mastermind/run-state/<basename>.json` to avoid a shared `tasks/state.json`.

use crate::audit_spec;
use crate::bounded_fs::{self, BoundedReadError, ReadControl, RootCapability};
use crate::indexer::{validate_index_root, Indexer};
use crate::spec::{self, ParsedSpec};
use crate::store::Store;
use crate::verify_spec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

const STRICT_EVIDENCE_FILE_LIMIT: usize = 1_000;
const STRICT_EVIDENCE_TOTAL_BYTE_LIMIT: u64 = 32 * 1024 * 1024;
const STRICT_EVIDENCE_GIT_BYTE_LIMIT: usize = 2 * 1024 * 1024;
const HISTORY_REVIEW_BYTE_LIMIT: u64 = 1024 * 1024;
const RUN_STATE_BYTE_LIMIT: u64 = 1024 * 1024;
pub(crate) const STRICT_SNAPSHOT_VERSION: u32 = 2;

/// Controller-owned handshake between pre- and post-flight. Canonical task
/// specs keep it beside the spec as `<task>/state.json`; legacy flat specs use
/// `<repo_root>/.mastermind/run-state/<spec-basename>.json`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RunState {
    /// User-facing lifecycle state consumed by `mastermind status` / `next`.
    #[serde(default = "default_run_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_artifact: Option<String>,
    /// Resolved path to the spec file pre-flight ran against.
    pub spec_path: String,
    /// SHA-256 of the approved spec body. Legacy 16-digit hashes remain readable.
    /// Post-flight requires an exact match or a new explicit pre-flight.
    pub spec_hash: String,
    /// `git rev-parse HEAD` captured at the first pre-flight — the audit's `--since`.
    /// Retries preserve it so previously committed implementation stays in scope.
    pub baseline_ref: String,
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
}

fn default_run_status() -> String {
    "approved".into()
}

fn legacy_snapshot_version() -> u32 {
    1
}

/// End-to-end result. Mapped to exit codes by `main.rs`: every `*Failed` /
/// `*Broken` variant exits non-zero so CI / scripts can react.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Pre-flight passed, state written, hand-off message printed (no `--exec`).
    PreReady,
    /// Pre-flight could not approve the spec. Prior approval is invalidated.
    PreFailed,
    /// Mechanical audit held; semantic review may still be required.
    PostHeld,
    /// Post-flight: warnings only. State kept for retry.
    PostDrift,
    /// Post-flight: contract-breaking findings. State kept.
    PostBroken,
    /// `--exec` shell-out to claude exited non-zero. State kept.
    ExecFailed,
}

/// Risk numbers surfaced after `verify_spec` — a short "what's at stake" summary
/// so the planner can spot runaway scope before inviting the executor in.
#[derive(Debug, Serialize)]
pub struct RiskReport {
    pub snapshot_symbols: u32,
    pub total_snapshot_callers: u32,
    pub worst_callers: Option<WorstSymbol>,
    pub mentioned_files: u32,
    pub files_in_cycles: Vec<String>,
    pub top_central_mentioned: Vec<CentralEntry>,
}

#[derive(Debug, Serialize)]
pub struct WorstSymbol {
    pub name: String,
    pub callers: u32,
}

#[derive(Debug, Serialize)]
pub struct CentralEntry {
    pub name: String,
    pub in_degree: u32,
}

/// Draft release notes assembled on a Held verdict. Markdown — pipes cleanly
/// into `gh pr create --body-file -` or any markdown viewer.
#[derive(Debug, Serialize)]
pub struct ReleaseNotes {
    pub title: String,
    pub goals: String,
    pub tests: String,
    pub diff_stat: String,
    pub audit_verdict: String,
}

/// Flags from `main.rs`. Single struct so the dispatcher signature stays stable
/// as options are added (next likely: `--json`).
#[derive(Debug, Clone, Copy)]
pub struct RunOpts {
    /// Restart pre-flight, preserving the task's baseline and iteration budget.
    pub reset: bool,
    /// Force pre-flight; never auto-resume into post-flight.
    pub pre_only: bool,
    /// Force post-flight; error if no state file exists.
    pub post_only: bool,
    /// Shell out to `claude -p` between phases. Default false — hand-off only.
    pub exec: bool,
    /// Skip the "index must exist and be non-empty" pre-check, for docs/spec-only
    /// specs that don't touch indexed source. Default false: a missing-or-empty
    /// index hard-fails pre-flight, since mmcg's core claim is "grounded in the
    /// codegraph" — ungrounded gates degrade to mandatory-section + file-existence
    /// checks only.
    pub allow_no_index: bool,
    /// Contract-driven mode: fold `verify_spec::strict_check` into pre-flight —
    /// require frontmatter scoping, file-scoped touches, and a runnable verify.
    pub strict: bool,
    /// Max pre-flight iterations on one spec before the dispatcher refuses.
    /// Default 3 — matches the `mastermind-task-planning` SKILL's "Iteration
    /// budget" and forge's `ErrorTracker.max_retries=3` anchor. 0 disables the
    /// budget (not recommended).
    pub max_iterations: u32,
    /// Bypass the iteration-budget check. Use only when the planner has decided
    /// the extra cycle is worth it (e.g. one specific defect kind to mop up).
    /// Auto-lesson append still fires, keeping the override visible to future
    /// planners.
    pub force_iteration: bool,
}

impl Default for RunOpts {
    fn default() -> Self {
        Self {
            reset: false,
            pre_only: false,
            post_only: false,
            exec: false,
            allow_no_index: false,
            strict: false,
            max_iterations: 3,
            force_iteration: false,
        }
    }
}

/// Canonical specs own `<task>/state.json`. A legacy flat spec retains the old
/// basename-keyed location; this avoids making every flat spec share
/// `.mastermind/tasks/state.json` while fixing the old `spec.json` collision
/// between canonical task folders.
pub fn state_file_path(repo_root: &Path, spec_path: &Path) -> PathBuf {
    if spec_path.file_name().and_then(|name| name.to_str()) == Some("spec.md") {
        let resolved = if spec_path.is_absolute() {
            spec_path.to_path_buf()
        } else {
            repo_root.join(spec_path)
        };
        return resolved.parent().unwrap_or(repo_root).join("state.json");
    }
    repo_root
        .join(".mastermind/run-state")
        .join(format!("{}.json", spec_basename(spec_path)))
}

/// Release notes path — `<repo_root>/.mastermind/releases/<spec-basename>.md`.
pub fn release_file_path(repo_root: &Path, spec_path: &Path) -> PathBuf {
    repo_root
        .join(".mastermind/releases")
        .join(format!("{}.md", spec_basename(spec_path)))
}

/// Semantic-history review path. Canonical tasks keep it beside their spec;
/// legacy flat specs use the local run-state directory.
pub fn history_review_file_path(repo_root: &Path, spec_path: &Path) -> PathBuf {
    if spec_path.file_name().and_then(|name| name.to_str()) == Some("spec.md") {
        let resolved = if spec_path.is_absolute() {
            spec_path.to_path_buf()
        } else {
            repo_root.join(spec_path)
        };
        return resolved
            .parent()
            .unwrap_or(repo_root)
            .join("history-review.md");
    }
    repo_root
        .join(".mastermind/run-state")
        .join(format!("{}-history-review.md", spec_basename(spec_path)))
}

fn ensure_history_review(
    repo_root: &Path,
    spec_path: &Path,
    release_path: &Path,
    snapshot: &str,
) -> std::io::Result<bool> {
    let path = history_review_file_path(repo_root, spec_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let root = RootCapability::open(repo_root).map_err(std::io::Error::other)?;
    match bounded_fs::read_regular_file_with_capability(
        &root,
        &path,
        HISTORY_REVIEW_BYTE_LIMIT,
        HISTORY_REVIEW_BYTE_LIMIT,
        ReadControl::default(),
    ) {
        Ok(previous) => {
            let same_snapshot = std::str::from_utf8(&previous.bytes)
                .ok()
                .and_then(history_review_fields)
                .is_some_and(|fields| fields.get("Audit snapshot").copied() == Some(snapshot));
            if same_snapshot {
                return Ok(false);
            }
            // Keep the exact previous review for provenance before replacing
            // its dispositions with a review of the new audited inputs.
            let archive = path.with_file_name(format!(
                "{}.{}.md",
                path.file_stem().unwrap_or_default().to_string_lossy(),
                crate::hex::encode(&Sha256::digest(&previous.bytes)),
            ));
            match bounded_fs::read_regular_file_with_capability(
                &root,
                &archive,
                HISTORY_REVIEW_BYTE_LIMIT,
                HISTORY_REVIEW_BYTE_LIMIT,
                ReadControl::default(),
            ) {
                Ok(existing) if existing.bytes == previous.bytes => {}
                Err(BoundedReadError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    bounded_fs::write_atomic_regular_file(
                        repo_root,
                        &archive,
                        &previous.bytes,
                        false,
                    )
                    .map_err(std::io::Error::other)?;
                }
                _ => {
                    return Err(std::io::Error::other(
                        "history review archive is unavailable",
                    ))
                }
            }
        }
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(std::io::Error::other(error)),
    }
    let spec = display_relative(repo_root, spec_path);
    let release = display_relative(repo_root, release_path);
    let audit = spec_path
        .parent()
        .map(|parent| parent.join("audit.md"))
        .unwrap_or_else(|| spec_path.with_file_name("audit.md"));
    let audit = display_relative(repo_root, &audit);
    let body = format!(
        "# History review — {}\n\n\
Complete this after semantic review. Replace each `pending` with `updated` or\n\
`not applicable`; do not create ceremonial CONTEXT or lesson entries.\n\n\
- **Audit snapshot:** {snapshot}\n\
- **Context:** pending\n\
- **Lesson:** pending\n\
- **Reason:** semantic review required\n\
- **Evidence:** `{spec}`; `{audit}`; `{release}`\n",
        spec_basename(spec_path),
    );
    bounded_fs::write_atomic_regular_file(repo_root, &path, body.as_bytes(), false)
        .map_err(std::io::Error::other)?;
    Ok(true)
}

/// Return true only after both durable-knowledge dispositions were reviewed
/// and the generated placeholder reason was replaced. The Markdown file remains
/// authoritative; lifecycle commands derive completion from it instead of
/// treating post-flight success as semantic review.
pub fn history_review_complete(review_path: &Path) -> bool {
    history_review_complete_for_snapshot(review_path, None)
}

/// Legacy reviews have no binding. Newly audited reviews must retain the
/// controller's snapshot marker as well as explicit semantic dispositions.
pub fn history_review_complete_for_snapshot(review_path: &Path, snapshot: Option<&str>) -> bool {
    let Ok(body) = read_history_review(review_path) else {
        return false;
    };
    history_review_body_complete(&body, snapshot)
}

pub(crate) fn history_review_body_complete(body: &str, snapshot: Option<&str>) -> bool {
    let Some(fields) = history_review_fields(body) else {
        return false;
    };
    let field = |name: &str| fields.get(name).copied();
    let disposition_complete = |value: Option<&str>| {
        value.is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "updated" | "not applicable"
            )
        })
    };
    let reason_reviewed = field("Reason").is_some_and(|reason| {
        !reason.is_empty() && !reason.eq_ignore_ascii_case("semantic review required")
    });
    disposition_complete(field("Context"))
        && disposition_complete(field("Lesson"))
        && reason_reviewed
        && snapshot.is_none_or(|expected| field("Audit snapshot") == Some(expected))
}

fn read_history_review(path: &Path) -> Result<String, String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file = path.file_name().ok_or("history review has no file name")?;
    let read = bounded_fs::read_regular_file(
        parent,
        Path::new(file),
        HISTORY_REVIEW_BYTE_LIMIT,
        HISTORY_REVIEW_BYTE_LIMIT,
        ReadControl::default(),
    )
    .map_err(|error| error.to_string())?;
    String::from_utf8(read.bytes).map_err(|_| "history review is not UTF-8".into())
}

fn history_review_fields(body: &str) -> Option<std::collections::BTreeMap<&'static str, &str>> {
    let mut fields = std::collections::BTreeMap::new();
    for line in crate::context_doctor::prose_lines(body) {
        for name in ["Context", "Lesson", "Reason", "Audit snapshot"] {
            if let Some(value) = line.text.strip_prefix(&format!("- **{name}:**")) {
                if fields.insert(name, value.trim()).is_some() {
                    return None;
                }
            }
        }
    }
    Some(fields)
}

struct HistoryInputs {
    snapshot: String,
    spec_body: String,
    executor_body: String,
}

fn history_input_snapshot(
    repo_root: &Path,
    spec_path: &Path,
    state: &RunState,
) -> Result<HistoryInputs, String> {
    if !matches!(state.baseline_ref.len(), 40 | 64)
        || !state
            .baseline_ref
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("history review requires an exact baseline object ID".into());
    }
    let root = RootCapability::open(repo_root).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    digest.update(b"mastermind-history-inputs-v1\0");
    digest.update(state.baseline_ref.as_bytes());
    digest.update(state.iteration.to_le_bytes());
    digest.update(state.started_at.to_le_bytes());
    let mut bytes_left = STRICT_EVIDENCE_TOTAL_BYTE_LIMIT;
    let spec = display_relative(repo_root, spec_path);
    let report = display_relative(repo_root, &spec_path.with_file_name("executor-report.md"));
    let mut read_input = |path: &str| -> Result<String, String> {
        let file = hash_history_file(&root, path, true, &mut digest, &mut bytes_left)?
            .ok_or_else(|| format!("history input `{path}` is missing"))?;
        String::from_utf8(file.bytes).map_err(|_| format!("history input `{path}` is not UTF-8"))
    };
    let spec_body = read_input(&spec)?;
    let executor_body = read_input(&report)?;
    if executor_body.len() as u64 > HISTORY_REVIEW_BYTE_LIMIT {
        return Err("executor report exceeds the 1 MiB limit".into());
    }
    let parsed = spec::parse_str(&spec, &spec_body);
    let mut paths = BTreeSet::new();
    for file in crate::declared_files::paths(&parsed) {
        paths.insert(
            crate::declared_files::normalize(file)
                .map_err(|_| format!("invalid history snapshot path `{file}`"))?,
        );
    }
    // Match the audit's working-tree scope, including files the executor did
    // not declare. Semantic history outputs are allowed to change after audit.
    for args in [
        vec![
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "diff.external=",
            "diff",
            "--name-only",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            &state.baseline_ref,
            "--",
        ],
        vec![
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    ] {
        let output = crate::diff::run_bounded_git_with_limit(
            repo_root,
            &args,
            None,
            STRICT_EVIDENCE_GIT_BYTE_LIMIT,
        )
        .map_err(|error| format!("history snapshot Git inventory: {}", error.code()))?;
        if !output.success {
            return Err("history snapshot Git inventory failed".into());
        }
        for raw in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|raw| !raw.is_empty())
        {
            let file = std::str::from_utf8(raw)
                .map_err(|_| "history snapshot contains a non-UTF-8 path")?;
            let file = crate::audit_bundle::normalize_relative_path(Path::new(file))
                .map_err(|_| "history snapshot contains an invalid path")?;
            let semantic_output = file == "CONTEXT.md"
                || (!file.contains('/')
                    && file.starts_with("CONTEXT-archive-")
                    && file.ends_with(".md"));
            if !file.starts_with(".mastermind/") && (!semantic_output || paths.contains(&file)) {
                paths.insert(file);
            }
        }
    }
    // Artifacts outside .mastermind (legacy flat specs) are bound separately.
    for artifact in [
        spec,
        report,
        display_relative(repo_root, &spec_path.with_file_name("audit.md")),
        display_relative(repo_root, &release_file_path(repo_root, spec_path)),
        display_relative(repo_root, &history_review_file_path(repo_root, spec_path)),
        display_relative(repo_root, &state_file_path(repo_root, spec_path)),
    ] {
        paths.remove(&artifact);
    }
    if paths.len() > STRICT_EVIDENCE_FILE_LIMIT {
        return Err("history snapshot exceeds the 1000-file limit".into());
    }
    for file in paths {
        let _ = hash_history_file(&root, &file, false, &mut digest, &mut bytes_left)?;
    }
    Ok(HistoryInputs {
        snapshot: crate::hex::encode(&digest.finalize()),
        spec_body,
        executor_body,
    })
}

fn hash_history_file(
    root: &RootCapability,
    path: &str,
    required: bool,
    digest: &mut Sha256,
    bytes_left: &mut u64,
) -> Result<Option<bounded_fs::BoundedFile>, String> {
    digest.update(path.as_bytes());
    digest.update([0]);
    let limit = (*bytes_left).min(crate::audit_bundle::BUNDLE_INPUT_MAX as u64);
    let file = match bounded_fs::read_regular_file_with_capability(
        root,
        &root.requested_root().join(path),
        limit,
        limit,
        ReadControl::default(),
    ) {
        Ok(file) => {
            *bytes_left -= file.declared_len;
            digest.update(b"file\0");
            digest.update(file.identity.attributes().to_le_bytes());
            digest.update(file.declared_len.to_le_bytes());
            digest.update(&file.bytes);
            Some(file)
        }
        Err(BoundedReadError::Io(error))
            if !required && error.kind() == std::io::ErrorKind::NotFound =>
        {
            digest.update(b"missing\0");
            None
        }
        Err(error) => return Err(format!("history snapshot `{path}`: {error}")),
    };
    digest.update([0]);
    Ok(file)
}

fn history_audit_snapshot(
    repo_root: &Path,
    spec_path: &Path,
    inputs: &str,
) -> Result<String, String> {
    let root = RootCapability::open(repo_root).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    digest.update(b"mastermind-history-audit-v1\0");
    digest.update(inputs.as_bytes());
    let mut bytes_left = STRICT_EVIDENCE_TOTAL_BYTE_LIMIT;
    for path in [
        spec_path.with_file_name("audit.md"),
        release_file_path(repo_root, spec_path),
    ] {
        let _ = hash_history_file(
            &root,
            &display_relative(repo_root, &path),
            true,
            &mut digest,
            &mut bytes_left,
        )?;
    }
    Ok(crate::hex::encode(&digest.finalize()))
}

fn current_history_snapshot(
    repo_root: &Path,
    spec_path: &Path,
    state: &RunState,
) -> Result<String, String> {
    let inputs = history_input_snapshot(repo_root, spec_path, state)?;
    history_audit_snapshot(repo_root, spec_path, &inputs.snapshot)
}

fn refresh_durable_history(store: &mut Store, repo_root: &Path) -> Result<u32, String> {
    Indexer::new(repo_root)
        .index_project_history(store)
        .map(|stats| stats.indexed)
        .map_err(|error| error.to_string())
}

fn open_validated_task_index(
    index_path: &Path,
    repo_root: &Path,
    allow_no_index: bool,
) -> Result<Option<Store>, String> {
    match std::fs::symlink_metadata(index_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_no_index => {
            return Ok(None);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "no index at `{}`; run `mastermind index .` first, or pass --allow-no-index for docs-only specs",
                index_path.display()
            ));
        }
        Err(error) => {
            return Err(format!(
                "cannot inspect index `{}`: {error}",
                index_path.display()
            ));
        }
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(format!(
                "index path `{}` is not a regular file",
                index_path.display()
            ));
        }
    }

    let store = Store::open_existing_without_migration(index_path).map_err(|error| {
        format!(
            "cannot open existing index `{}`: {error}",
            index_path.display()
        )
    })?;
    if !store
        .schema_current()
        .map_err(|error| format!("cannot inspect index schema: {error}"))?
    {
        return Err(format!(
            "index schema at `{}` is missing or outdated; rebuild with `mastermind index .`",
            index_path.display()
        ));
    }
    let symbols = store
        .symbol_count()
        .map_err(|error| format!("cannot query index `{}`: {error}", index_path.display()))?;
    if symbols == 0 {
        if store
            .meta_value("index_root")
            .map_err(|error| format!("cannot read index root: {error}"))?
            .is_some()
        {
            validate_index_root(&store, repo_root)
                .map_err(|error| format!("index/root mismatch: {error}"))?;
        }
        return if allow_no_index {
            Ok(None)
        } else {
            Err(format!(
                "index at `{}` is empty (0 symbols); run `mastermind index .` to populate, or pass --allow-no-index for docs-only specs",
                index_path.display()
            ))
        };
    }
    validate_index_root(&store, repo_root)
        .map_err(|error| format!("index/root mismatch: {error}"))?;
    Ok(Some(store))
}

fn open_current_task_snapshot(index_path: &Path, repo_root: &Path) -> Result<Store, String> {
    let store = Store::open_read_only(index_path).map_err(|error| {
        format!(
            "cannot freeze index `{}` for analysis: {error}",
            index_path.display()
        )
    })?;
    if !store
        .schema_current()
        .map_err(|error| format!("cannot inspect frozen index schema: {error}"))?
    {
        return Err("refreshed index schema is not current".into());
    }
    if store
        .symbol_count()
        .map_err(|error| format!("cannot query frozen index: {error}"))?
        == 0
    {
        return Err("refreshed index became empty before analysis".into());
    }
    validate_index_root(&store, repo_root)
        .map_err(|error| format!("frozen index/root mismatch: {error}"))?;
    store
        .ensure_source_snapshot_current()
        .map_err(|error| format!("index changed while freezing analysis snapshot: {error}"))?;
    Ok(store)
}

fn refresh_durable_history_at(index_path: &Path, repo_root: &Path) -> Result<u32, String> {
    let mut store = open_validated_task_index(index_path, repo_root, false)?
        .ok_or_else(|| "durable history requires a populated index".to_string())?;
    refresh_durable_history(&mut store, repo_root)
}

fn display_relative(repo_root: &Path, path: &Path) -> String {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    };
    resolved
        .strip_prefix(repo_root)
        .unwrap_or(&resolved)
        .to_string_lossy()
        .replace('\\', "/")
}

fn spec_basename(spec_path: &Path) -> String {
    if spec_path.file_name().and_then(|name| name.to_str()) == Some("spec.md") {
        if let Some(task_name) = spec_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
        {
            return task_name.to_string();
        }
    }
    spec_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("spec")
        .to_string()
}

/// Read + deserialize state. `Ok(None)` when the file is absent — "no prior
/// pre-flight" is the dominant non-error case.
pub fn load_state(path: &Path) -> std::io::Result<Option<RunState>> {
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
    let state: RunState = serde_json::from_slice(&body)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(state))
}

/// Persist state for callers that selected an explicit state path. Controller
/// flows should use `save_state_in_repository` so every path component remains
/// bound to the selected repository capability.
pub fn save_state(path: &Path, state: &RunState) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    save_state_in_repository(parent, path, state)
}

pub fn save_state_in_repository(
    repo_root: &Path,
    path: &Path,
    state: &RunState,
) -> std::io::Result<()> {
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

pub fn delete_state(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Stable across toolchains and machines for persisted approval evidence.
pub(crate) fn hash_text(text: &str) -> String {
    crate::hex::encode(&Sha256::digest(text.as_bytes()))
}

pub(crate) fn spec_hash_matches(expected: &str, text: &str) -> bool {
    if expected.len() == 16 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let mut legacy = DefaultHasher::new();
        text.hash(&mut legacy);
        return expected == format!("{:016x}", legacy.finish());
    }
    expected == hash_text(text)
}

fn read_preflight_spec(repo_root: &Path, spec_path: &Path) -> Result<String, String> {
    let limit = crate::audit_bundle::BUNDLE_INPUT_MAX as u64;
    let file =
        bounded_fs::read_regular_file(repo_root, spec_path, limit, limit, ReadControl::default())
            .map_err(|error| error.to_string())?;
    String::from_utf8(file.bytes).map_err(|_| "spec is not UTF-8".into())
}

fn preflight_required_state(state: &RunState, reason: &str) -> RunState {
    let mut blocked = state.clone();
    blocked.status = "held".into();
    blocked.risk = None;
    blocked.next_step = Some("run_preflight".into());
    blocked.blocking_reason = Some(reason.into());
    blocked.last_artifact = Some("spec.md".into());
    blocked.held_snapshot_sha256 = None;
    blocked.history_snapshot_sha256 = None;
    blocked
}

fn state_matches_spec(repo_root: &Path, spec_path: &Path, state: &RunState) -> bool {
    let resolved = if spec_path.is_absolute() {
        spec_path.to_path_buf()
    } else {
        repo_root.join(spec_path)
    };
    let stored = Path::new(&state.spec_path);
    if stored == resolved {
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
    // Canonical task artifacts can move with a checkout. Legacy basename-keyed
    // state must match the actual spec path, not another same-named file.
    let (Ok(root), Ok(current)) = (repo_root.canonicalize(), resolved.canonicalize()) else {
        return false;
    };
    let Ok(task_spec) = current.strip_prefix(root.join(".mastermind/tasks")) else {
        return false;
    };
    if task_spec.components().count() != 2
        || task_spec.file_name().and_then(|name| name.to_str()) != Some("spec.md")
    {
        return false;
    }
    let suffix = format!(
        ".mastermind/tasks/{}",
        task_spec.to_string_lossy().replace('\\', "/")
    );
    let stored = state.spec_path.replace('\\', "/");
    stored == suffix || stored.ends_with(&format!("/{suffix}"))
}

fn preflight_baseline(repo_root: &Path, previous: Option<&RunState>) -> Result<String, String> {
    let Some(previous) = previous else {
        return git_head(repo_root);
    };
    let baseline = &previous.baseline_ref;
    if !matches!(baseline.len(), 40 | 64) || !baseline.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("saved baseline must be an exact Git object ID".into());
    }
    let output = crate::diff::run_bounded_git_with_limit(
        repo_root,
        &["rev-parse", "--verify", &format!("{baseline}^{{commit}}")],
        None,
        STRICT_EVIDENCE_GIT_BYTE_LIMIT,
    )
    .map_err(|error| format!("saved baseline lookup failed: {}", error.code()))?;
    if !output.success || String::from_utf8_lossy(&output.stdout).trim() != baseline.as_str() {
        return Err(
            "saved baseline commit is unavailable; restore its Git history before retrying".into(),
        );
    }
    Ok(baseline.clone())
}

pub(crate) fn strict_workflow_snapshot(
    repo_root: &Path,
    baseline_ref: &str,
    touch_files: &[String],
) -> Result<String, String> {
    strict_workflow_snapshot_for_version(
        repo_root,
        baseline_ref,
        touch_files,
        STRICT_SNAPSHOT_VERSION,
    )
}

pub(crate) fn strict_workflow_snapshot_for_version(
    repo_root: &Path,
    baseline_ref: &str,
    touch_files: &[String],
    version: u32,
) -> Result<String, String> {
    if !matches!(version, 1 | STRICT_SNAPSHOT_VERSION) {
        return Err(format!("unsupported strict-workflow snapshot version {version}; re-audit with a supported runtime"));
    }
    if !matches!(baseline_ref.len(), 40 | 64)
        || !baseline_ref.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("strict-workflow baseline must be an exact Git object ID".into());
    }
    let mut paths = BTreeSet::new();
    for file in touch_files {
        let normalized = crate::declared_files::normalize(file)
            .map_err(|_| format!("invalid strict-workflow touch path `{file}`"))?;
        if normalized.starts_with(".mastermind/tasks/")
            || normalized.starts_with(".mastermind/releases/")
        {
            return Err(format!(
                "strict-workflow evidence cannot attest its own artifact path `{normalized}`"
            ));
        }
        paths.insert(normalized);
    }
    if paths.is_empty() || paths.len() > STRICT_EVIDENCE_FILE_LIMIT {
        return Err(format!(
            "strict-workflow evidence requires 1..={STRICT_EVIDENCE_FILE_LIMIT} touch files"
        ));
    }

    let root = RootCapability::open(repo_root)
        .map_err(|error| format!("strict-workflow repository root is unavailable: {error}"))?;
    let first = strict_snapshot_digest(&root, baseline_ref, &paths, version)?;
    if strict_snapshot_digest(&root, baseline_ref, &paths, version)? != first {
        return Err("strict-workflow files or Git modes changed during the snapshot".into());
    }
    Ok(first)
}

fn strict_snapshot_digest(
    root: &RootCapability,
    baseline_ref: &str,
    paths: &BTreeSet<String>,
    version: u32,
) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(format!("mastermind-strict-workflow-snapshot-v{version}\0").as_bytes());
    digest.update(baseline_ref.as_bytes());
    digest.update([0]);
    let git_modes = if version == 1 {
        let mut git_args = vec![
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "diff.external=",
            "diff",
            "--raw",
            "--no-abbrev",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            baseline_ref,
            "--",
        ];
        git_args.extend(paths.iter().map(String::as_str));
        digest.update(b"git-raw\0");
        digest.update(strict_snapshot_git(root, &git_args)?);
        digest.update([0]);
        None
    } else {
        let commit = strict_snapshot_git(
            root,
            &[
                "rev-parse",
                "--verify",
                &format!("{baseline_ref}^{{commit}}"),
            ],
        )?;
        if String::from_utf8_lossy(&commit).trim() != baseline_ref {
            return Err("strict-workflow baseline must identify an available commit".into());
        }
        Some(strict_snapshot_modes(root, paths)?)
    };
    let mut total_bytes = 0u64;

    for relative in paths {
        digest.update(relative.as_bytes());
        digest.update([0]);
        let limit = (crate::audit_bundle::BUNDLE_INPUT_MAX as u64)
            .min(STRICT_EVIDENCE_TOTAL_BYTE_LIMIT - total_bytes);
        let file = match bounded_fs::read_regular_file_with_capability(
            root,
            &root.requested_root().join(relative),
            limit,
            limit,
            ReadControl::default(),
        ) {
            Ok(file) => file,
            Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                digest.update(b"missing\0");
                continue;
            }
            Err(error) => {
                return Err(format!("strict-workflow touch file `{relative}`: {error}"));
            }
        };
        total_bytes += file.declared_len;
        digest.update(b"file\0");
        if let Some((trust_filemode, modes)) = &git_modes {
            if modes
                .get(relative)
                .is_some_and(|mode| !matches!(mode.as_str(), "100644" | "100755"))
            {
                return Err(format!(
                    "strict-workflow touch `{relative}` has an unsupported Git file type"
                ));
            }
            let mode = if *trust_filemode {
                #[cfg(unix)]
                {
                    if file.identity.attributes() & 0o100 != 0 {
                        "100755"
                    } else {
                        "100644"
                    }
                }
                #[cfg(not(unix))]
                {
                    return Err("strict-workflow cannot observe executable bits on this platform with core.filemode=true".into());
                }
            } else {
                modes.get(relative).map(String::as_str).unwrap_or("100644")
            };
            digest.update(mode.as_bytes());
            digest.update([0]);
        }
        digest.update(file.declared_len.to_le_bytes());
        digest.update(file.bytes);
        digest.update([0]);
    }
    Ok(crate::hex::encode(&digest.finalize()))
}

fn strict_snapshot_git(root: &RootCapability, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = crate::diff::run_bounded_git_with_limit(
        root.canonical_root(),
        args,
        None,
        STRICT_EVIDENCE_GIT_BYTE_LIMIT,
    )
    .map_err(|error| format!("strict-workflow Git evidence failed: {}", error.code()))?;
    if !output.success {
        return Err("strict-workflow Git evidence is unavailable".into());
    }
    Ok(output.stdout)
}

fn strict_snapshot_modes(
    root: &RootCapability,
    paths: &BTreeSet<String>,
) -> Result<(bool, BTreeMap<String, String>), String> {
    let config = strict_snapshot_git(
        root,
        &[
            "config",
            "--type=bool",
            "--default=true",
            "--get",
            "core.filemode",
        ],
    )?;
    let trust_filemode = match String::from_utf8_lossy(&config).trim() {
        "true" => true,
        "false" => false,
        _ => return Err("strict-workflow core.filemode is invalid".into()),
    };
    let mut args = vec![
        "--literal-pathspecs",
        "-c",
        "core.fsmonitor=false",
        "ls-files",
        "--stage",
        "-z",
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    let output = strict_snapshot_git(root, &args)?;
    let mut modes = BTreeMap::new();
    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let record =
            std::str::from_utf8(record).map_err(|_| "strict-workflow index path is not UTF-8")?;
        let (header, path) = record
            .split_once('\t')
            .ok_or("invalid strict-workflow index entry")?;
        if !paths.contains(path) {
            return Err("strict-workflow touch paths must identify individual files".into());
        }
        let fields = header.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 3 || fields[2] != "0" {
            return Err(format!(
                "strict-workflow index has unresolved stages for `{path}`"
            ));
        }
        let mode = match fields[0] {
            mode @ ("100644" | "100755" | "120000" | "160000") => mode,
            _ => {
                return Err(format!(
                    "strict-workflow index mode is unsupported for `{path}`"
                ))
            }
        };
        if modes.insert(path.to_string(), mode.to_string()).is_some() {
            return Err(format!(
                "strict-workflow index has duplicate entries for `{path}`"
            ));
        }
    }
    Ok((trust_filemode, modes))
}

fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn git_head(repo_root: &Path) -> Result<String, String> {
    let out = crate::diff::repository_git_command()
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("git rev-parse HEAD: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git rev-parse HEAD: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_diff_stat(repo_root: &Path, baseline_ref: &str) -> Result<String, String> {
    let run = |args: &[&str]| {
        let output = crate::diff::run_bounded_git_with_limit(
            repo_root,
            args,
            None,
            STRICT_EVIDENCE_GIT_BYTE_LIMIT,
        )
        .map_err(|error| format!("release diff: {}", error.code()))?;
        if !output.success {
            return Err("release diff: Git command failed".to_string());
        }
        Ok(output.stdout)
    };
    let stat = run(&[
        "-c",
        "core.fsmonitor=false",
        "diff",
        "--stat",
        "--no-ext-diff",
        "--no-textconv",
        baseline_ref,
        "--",
        ".",
        ":(exclude).mastermind",
    ])?;
    let mut body = String::from_utf8(stat)
        .map_err(|_| "release diff is not UTF-8")?
        .trim_end()
        .to_string();
    let raw = run(&[
        "-c",
        "core.fsmonitor=false",
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
    ])?;
    let mut untracked = BTreeSet::new();
    for path in raw.split(|byte| *byte == 0).filter(|path| !path.is_empty()) {
        let path = std::str::from_utf8(path).map_err(|_| "untracked release path is not UTF-8")?;
        if !path.starts_with(".mastermind/") {
            let path = crate::audit_bundle::normalize_relative_path(Path::new(path))
                .map_err(|_| "untracked release path is invalid")?;
            untracked.insert(path);
        }
    }
    if !untracked.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str("Untracked files:\n");
        body.push_str(&untracked.into_iter().collect::<Vec<_>>().join("\n"));
    }
    Ok(body)
}

/// First `# Title` line BEFORE any `##` section header. None when absent.
fn extract_h1_title(body: &str) -> Option<String> {
    for line in body.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("# ") {
            return Some(rest.trim().to_string());
        }
        if t.starts_with("##") {
            return None;
        }
    }
    None
}

/// Compute the risk report from a parsed spec + the live index. Pure: no I/O
/// beyond store queries. Failed or incomplete queries cannot establish zero risk.
pub fn compute_risk_report(spec: &ParsedSpec, store: &Store) -> Result<RiskReport, String> {
    let mut total_callers: u32 = 0;
    let mut worst: Option<WorstSymbol> = None;
    let mut central: Vec<CentralEntry> = Vec::new();

    for claim in &spec.pre_edit_snapshot {
        let resolved =
            crate::spec_symbols::resolve_snapshot(store, spec, claim).map_err(|error| {
                format!(
                    "snapshot identity for '{}' is unavailable: {}",
                    claim.name, error.reason
                )
            })?;
        let n = store
            .callers_of(&resolved.symbol.name, resolved.language.as_deref(), None)
            .map(|c| c.len() as u32)
            .map_err(|error| format!("caller risk for `{}` is unavailable: {error}", claim.name))?;
        total_callers = total_callers.saturating_add(n);
        if worst.as_ref().is_none_or(|w| n > w.callers) {
            worst = Some(WorstSymbol {
                name: claim.name.clone(),
                callers: n,
            });
        }
        if n > 0 {
            central.push(CentralEntry {
                name: claim.name.clone(),
                in_degree: n,
            });
        }
    }
    central.sort_by_key(|e| std::cmp::Reverse(e.in_degree));
    central.truncate(3);

    // Cycle membership: walk all SCCs of size ≥ 2 in any language; collect
    // mentioned files appearing inside.
    let mentioned: HashSet<&str> = spec.mentioned_files.iter().map(String::as_str).collect();
    let (cycles, cycles_truncated) = store
        .dependency_cycles(None, 2)
        .map_err(|error| format!("dependency-cycle risk is unavailable: {error}"))?;
    if cycles_truncated {
        return Err(
            "dependency-cycle risk is unknown: the indexed dependency graph exceeds the query limit"
                .into(),
        );
    }
    let mut files_in_cycles: Vec<String> = Vec::new();
    for cycle in cycles {
        for f in cycle {
            if mentioned.contains(f.as_str()) && !files_in_cycles.iter().any(|x| x == &f) {
                files_in_cycles.push(f);
            }
        }
    }

    Ok(RiskReport {
        snapshot_symbols: spec.pre_edit_snapshot.len() as u32,
        total_snapshot_callers: total_callers,
        worst_callers: worst,
        mentioned_files: spec.mentioned_files.len() as u32,
        files_in_cycles,
        top_central_mentioned: central,
    })
}

pub fn render_risk_report(r: &RiskReport) -> String {
    let mut out = String::new();
    out.push_str("\nRisk Report\n");
    out.push_str(&format!("  Snapshot symbols: {}\n", r.snapshot_symbols));
    out.push_str(&format!(
        "  Total snapshot callers: {}\n",
        r.total_snapshot_callers
    ));
    if let Some(w) = &r.worst_callers {
        out.push_str(&format!(
            "  Worst blast radius: {} (`{}`)\n",
            w.callers, w.name
        ));
    }
    out.push_str(&format!("  Mentioned files: {}\n", r.mentioned_files));
    if r.files_in_cycles.is_empty() {
        out.push_str("  Files in dependency cycles: 0\n");
    } else {
        out.push_str(&format!(
            "  ⚠️  Files in dependency cycles: {}\n",
            r.files_in_cycles.join(", ")
        ));
    }
    if !r.top_central_mentioned.is_empty() {
        out.push_str("  Top centrality of mentioned symbols:\n");
        for e in &r.top_central_mentioned {
            out.push_str(&format!("    - {} (in_degree={})\n", e.name, e.in_degree));
        }
    }
    out
}

pub fn compute_release_notes(
    spec: &ParsedSpec,
    spec_body: &str,
    repo_root: &Path,
    baseline_ref: &str,
    audit_verdict: &str,
) -> ReleaseNotes {
    let title = extract_h1_title(spec_body).unwrap_or_else(|| spec_basename(Path::new(&spec.path)));
    let goals = spec::section_body(spec, "Goals").unwrap_or("").to_string();
    let tests = spec::section_body(spec, "Tests Plan")
        .unwrap_or("")
        .to_string();
    let diff_stat = git_diff_stat(repo_root, baseline_ref)
        .unwrap_or_else(|e| format!("(diff unavailable: {e})"));
    ReleaseNotes {
        title,
        goals,
        tests,
        diff_stat,
        audit_verdict: audit_verdict.to_string(),
    }
}

pub fn render_release_notes(r: &ReleaseNotes) -> String {
    let goals = if r.goals.trim().is_empty() {
        "(no `## Goals` section in spec)".to_string()
    } else {
        r.goals.trim().to_string()
    };
    let tests = if r.tests.trim().is_empty() {
        "(no `## Tests Plan` section in spec)".to_string()
    } else {
        r.tests.trim().to_string()
    };
    format!(
        "# {title}\n\n## Summary\n\n{goals}\n\n## Tests\n\n{tests}\n\n## Diff\n\n```\n{diff}\n```\n\n---\nAudit: {verdict}\n",
        title = r.title,
        goals = goals,
        tests = tests,
        diff = r.diff_stat,
        verdict = r.audit_verdict,
    )
}

/// Top-level dispatcher — picks pre or post from flags + state presence, then
/// calls that phase function. Pure I/O orchestration; the computational pieces
/// above are independently testable.
pub fn run(spec_path: &Path, repo_root: &Path, index_path: &Path, opts: RunOpts) -> Outcome {
    let state_path = state_file_path(repo_root, spec_path);
    if opts.post_only && (opts.pre_only || opts.reset) {
        eprintln!("error: --post-only cannot be combined with --pre-only or --reset");
        return Outcome::PreFailed;
    }
    let existing = match load_state(&state_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "error: state file `{}` unreadable ({e}); restore it before retrying to retain the task baseline and iteration budget",
                state_path.display()
            );
            return Outcome::PreFailed;
        }
    };
    if existing
        .as_ref()
        .is_some_and(|state| !state_matches_spec(repo_root, spec_path, state))
    {
        eprintln!(
            "error: saved state belongs to a different spec; use a separate canonical task folder"
        );
        return Outcome::PreFailed;
    }

    // Explicit retries keep the first baseline and the durable iteration count.
    // Deleting state before validation would lose both on a failed --reset.
    if opts.pre_only || opts.reset || existing.is_none() {
        if opts.post_only {
            eprintln!(
                "error: --post-only requested but no state file at `{}`. Run pre-flight first.",
                state_path.display()
            );
            return Outcome::PreFailed;
        }
        return run_pre(
            spec_path,
            repo_root,
            index_path,
            &state_path,
            opts,
            existing.as_ref(),
        );
    }

    let state = existing.as_ref().unwrap();
    if state.next_step.as_deref() == Some("run_preflight") {
        eprintln!(
            "error: this task requires a new pre-flight. Review the spec, then run `mastermind run-task {} --pre-only`. The original baseline and prior strict/index options are retained.",
            spec_path.display()
        );
        return Outcome::PreFailed;
    }
    if opts.post_only {
        return run_post(spec_path, repo_root, index_path, state, &state_path);
    }

    if !opts.post_only {
        if let Some(state) = existing.as_ref() {
            let review_path = history_review_file_path(repo_root, spec_path);
            let review_complete = history_review_complete_for_snapshot(
                &review_path,
                state.history_snapshot_sha256.as_deref(),
            );
            if state.status == "learned" && review_complete {
                println!(
                    "Task already complete — state is `{}`. Use --reset to start a new iteration or --post-only to re-audit.",
                    state_path.display()
                );
                return Outcome::PostHeld;
            }
            if matches!(state.status.as_str(), "learned" | "history_review_required") {
                let snapshot = current_history_snapshot(repo_root, spec_path, state);
                if state.history_snapshot_sha256.is_none()
                    || snapshot.as_ref().ok() != state.history_snapshot_sha256.as_ref()
                {
                    let reason = snapshot.err().unwrap_or_else(|| {
                        "audited inputs changed or have no review binding".into()
                    });
                    let mut stale = state.clone();
                    stale.status = "audit_required".into();
                    stale.next_step = Some("run_audit".into());
                    stale.blocking_reason = Some(reason.clone());
                    stale.held_snapshot_sha256 = None;
                    stale.history_snapshot_sha256 = None;
                    if let Err(error) = save_state_in_repository(repo_root, &state_path, &stale) {
                        eprintln!("error: persisting required re-audit: {error}");
                    }
                    eprintln!("error: history review cannot close this task: {reason}. Re-run run-task to audit the current work.");
                    return Outcome::PostBroken;
                }
                if review_complete {
                    let mut store = match open_validated_task_index(
                        index_path,
                        repo_root,
                        state.allow_no_index,
                    ) {
                        Ok(store) => store,
                        Err(error) => {
                            eprintln!(
                                "error: validating index for semantic history refresh: {error}"
                            );
                            return Outcome::PostBroken;
                        }
                    };
                    if let Some(store) = store.as_mut() {
                        if let Err(error) = refresh_durable_history(store, repo_root) {
                            eprintln!(
                                "error: refreshing durable history before semantic completion: {error}"
                            );
                            return Outcome::PostBroken;
                        }
                    }
                    if current_history_snapshot(repo_root, spec_path, state)
                        .ok()
                        .as_ref()
                        != state.history_snapshot_sha256.as_ref()
                        || !history_review_complete_for_snapshot(
                            &review_path,
                            state.history_snapshot_sha256.as_deref(),
                        )
                    {
                        eprintln!("error: audited inputs or semantic review changed during history refresh; re-run run-task");
                        return Outcome::PostBroken;
                    }
                    let mut completed = state.clone();
                    completed.status = "learned".into();
                    completed.next_step = Some("close".into());
                    completed.last_artifact = Some("history-review.md".into());
                    if let Err(error) = save_state_in_repository(repo_root, &state_path, &completed)
                    {
                        eprintln!(
                            "error: persisting reviewed state `{}`: {error}",
                            state_path.display()
                        );
                        return Outcome::PostBroken;
                    }
                    println!("Task complete — semantic history review is resolved.");
                } else {
                    if state.status == "learned" {
                        let mut pending = state.clone();
                        pending.status = "history_review_required".into();
                        pending.next_step = Some("review_history".into());
                        if let Err(error) =
                            save_state_in_repository(repo_root, &state_path, &pending)
                        {
                            eprintln!("error: persisting required semantic review: {error}");
                            return Outcome::PostBroken;
                        }
                    }
                    println!(
                        "Mechanical audit is held; semantic history review is still required at `{}`.",
                        review_path.display()
                    );
                }
                return Outcome::PostHeld;
            }
        }
    }

    // Default mode + state present → resume post.
    let state = existing.unwrap();
    run_post(spec_path, repo_root, index_path, &state, &state_path)
}

fn run_pre(
    spec_path: &Path,
    repo_root: &Path,
    index_path: &Path,
    state_path: &Path,
    opts: RunOpts,
    previous: Option<&RunState>,
) -> Outcome {
    let opts = RunOpts {
        strict: opts.strict || previous.is_some_and(|state| state.strict),
        allow_no_index: opts.allow_no_index || previous.is_some_and(|state| state.allow_no_index),
        ..opts
    };
    let iteration = previous
        .map_or(0, |state| state.iteration)
        .saturating_add(1);
    let budget_exhausted = opts.max_iterations > 0 && iteration > opts.max_iterations;
    // Revalidation removes old approval even when it fails or is interrupted.
    // A refused attempt leaves the exhausted counter intact for the next call.
    if let Some(previous) = previous {
        let mut pending = preflight_required_state(previous, "pre-flight validation is required");
        if budget_exhausted && !opts.force_iteration {
            pending.blocking_reason = Some(format!(
                "iteration budget exhausted (limit {}); review the design before explicitly using --force-iteration",
                opts.max_iterations
            ));
        }
        pending.strict = opts.strict;
        pending.allow_no_index = opts.allow_no_index;
        if !budget_exhausted || opts.force_iteration {
            pending.iteration = iteration;
        }
        if let Err(error) = save_state_in_repository(repo_root, state_path, &pending) {
            eprintln!("error: invalidating previous pre-flight approval: {error}");
            return Outcome::PreFailed;
        }
    }
    if budget_exhausted {
        let _ = crate::lessons::append_iteration_budget_candidate(repo_root, spec_path, iteration);
    }
    if budget_exhausted && !opts.force_iteration {
        eprintln!(
            "❌ iteration budget exhausted: this task has used {} pre-flight attempt(s) (limit: {}).",
            iteration - 1,
            opts.max_iterations
        );
        eprintln!("   Stop and re-design the spec, or re-run with --force-iteration to override.");
        eprintln!(
            "   See `defect-taxonomy.md` in the mastermind-task-planning skill, kind `iteration_budget_exhausted`."
        );
        return Outcome::PreFailed;
    }

    let spec_body = match read_preflight_spec(repo_root, spec_path) {
        Ok(body) => body,
        Err(error) => {
            eprintln!("error: reading spec `{}`: {error}", spec_path.display());
            return Outcome::PreFailed;
        }
    };
    let parsed = spec::parse_str(&spec_path.display().to_string(), &spec_body);

    println!("=== Pre-flight: {} ===", spec_path.display());

    // Index existence + non-empty check — hard fail by default. mmcg's gates
    // are only as strong as the codegraph they reason from; against an absent
    // or empty index, verify-spec silently degrades to file-existence checks and
    // audit-spec to git-diff-only. Escape hatch `--allow-no-index` for docs-only
    // specs.
    let mut store = match open_validated_task_index(index_path, repo_root, opts.allow_no_index) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("❌ {error}");
            return Outcome::PreFailed;
        }
    };
    if let Some(mut index) = store.take() {
        let refresh = match Indexer::new(repo_root).index_all(&mut index, false) {
            Ok(stats) => stats,
            Err(error) => {
                eprintln!("❌ Refreshing index before pre-flight failed: {error}");
                return Outcome::PreFailed;
            }
        };
        if refresh.files_failed > 0 {
            eprintln!(
                "❌ Pre-flight index refresh failed for {} file(s); refusing to verify against stale graph data",
                refresh.files_failed
            );
            return Outcome::PreFailed;
        }
        drop(index);
        store = match open_current_task_snapshot(index_path, repo_root) {
            Ok(store) => Some(store),
            Err(error) => {
                eprintln!("❌ Cannot freeze refreshed pre-flight index: {error}");
                return Outcome::PreFailed;
            }
        };
    }

    // 1. verify-spec (store optional — without index, only mandatory-section +
    //    missing-file checks run).
    let mut verify = verify_spec::run(&parsed, store.as_ref(), repo_root);
    if opts.strict {
        for f in verify_spec::strict_check(&parsed) {
            verify.push_error(f);
        }
    }
    print!("{}", verify.render_text());
    if verify.has_failures() {
        eprintln!(
            "❌ verify-spec failed — spec is not approved. Fix errors above and re-run `mastermind run-task <spec> --pre-only`."
        );
        return Outcome::PreFailed;
    }

    // 2. risk report (needs an open store for caller counts; without one,
    //    reporting zeros would mislead).
    match &store {
        Some(store) => match compute_risk_report(&parsed, store) {
            Ok(risk) => print!("{}", render_risk_report(&risk)),
            Err(error) => {
                eprintln!("❌ Risk report is incomplete: {error}. Pre-flight cannot approve this task.");
                return Outcome::PreFailed;
            }
        },
        None => println!(
            "\nRisk Report\n  (no index at `{}` — run `mastermind index .` for blast-radius numbers)",
            index_path.display()
        ),
    }

    // 3. Keep all implementation since the original pre-flight in audit scope.
    let head = match preflight_baseline(repo_root, previous) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("error: resolving task baseline: {e}");
            return Outcome::PreFailed;
        }
    };
    if read_preflight_spec(repo_root, spec_path).ok().as_ref() != Some(&spec_body) {
        eprintln!("error: spec changed during pre-flight; review it and retry --pre-only");
        return Outcome::PreFailed;
    }
    if let Some(store) = &store {
        if let Err(error) = store.ensure_source_snapshot_current() {
            eprintln!("error: index changed during pre-flight analysis: {error}");
            return Outcome::PreFailed;
        }
    }
    let declared_risk = parsed
        .frontmatter
        .as_ref()
        .and_then(|frontmatter| frontmatter.risk.as_deref())
        .filter(|risk| matches!(*risk, "low" | "medium" | "high"))
        .unwrap_or("low");
    let resolved_spec_path = if spec_path.is_absolute() {
        spec_path.to_path_buf()
    } else {
        repo_root.join(spec_path)
    };
    let state = RunState {
        status: "approved".into(),
        risk: Some(declared_risk.into()),
        next_step: Some("run_executor".into()),
        blocking_reason: None,
        last_artifact: Some("spec.md".into()),
        spec_path: resolved_spec_path.display().to_string(),
        spec_hash: hash_text(&spec_body),
        baseline_ref: head.clone(),
        held_snapshot_sha256: None,
        held_snapshot_version: STRICT_SNAPSHOT_VERSION,
        history_snapshot_sha256: None,
        started_at: timestamp_now(),
        iteration,
        allow_no_index: opts.allow_no_index,
        strict: opts.strict,
    };
    if let Err(e) = save_state_in_repository(repo_root, state_path, &state) {
        eprintln!("error: writing state `{}`: {e}", state_path.display());
        return Outcome::PreFailed;
    }
    let head_short = &head[..head.len().min(8)];
    println!(
        "\nState: {} (baseline `{}`)",
        state_path.display(),
        head_short
    );

    // 4. executor: --exec (synchronous shell-out) or hand-off message.
    if opts.exec && !opts.pre_only {
        println!("\nInvoking executor (`claude -p`)...\n");
        match run_executor(spec_path, repo_root) {
            Ok(()) => {
                println!("\nExecutor returned 0. Continuing into post-flight.\n");
                return run_post(spec_path, repo_root, index_path, &state, state_path);
            }
            Err(e) => {
                eprintln!("\n❌ Executor failed: {e}");
                eprintln!(
                    "State kept at `{}`. After fixing, re-run `mastermind run-task {}`.",
                    state_path.display(),
                    spec_path.display()
                );
                return Outcome::ExecFailed;
            }
        }
    }

    println!(
        "\nNext: hand this spec to the implementation agent in your coding client. \
         It must write `<task>/executor-report.md`. Then re-run:\n  mastermind run-task {}\nto audit + draft release notes.",
        spec_path.display()
    );
    Outcome::PreReady
}

fn run_post(
    spec_path: &Path,
    repo_root: &Path,
    index_path: &Path,
    state: &RunState,
    state_path: &Path,
) -> Outcome {
    // A failed explicit re-audit must not leave an earlier learned state
    // eligible for completion or architecture-policy evidence.
    let mut auditing = state.clone();
    auditing.status = "audit_required".into();
    auditing.next_step = Some("run_audit".into());
    auditing.held_snapshot_sha256 = None;
    auditing.history_snapshot_sha256 = None;
    if let Err(error) = save_state_in_repository(repo_root, state_path, &auditing) {
        eprintln!("error: persisting audit-required state: {error}");
        return Outcome::PostBroken;
    }
    let inputs = match history_input_snapshot(repo_root, spec_path, state) {
        Ok(inputs) => inputs,
        Err(error) => {
            eprintln!("error: cannot bind the spec, executor report and implementation: {error}");
            auditing.status = "held".into();
            auditing.risk = Some("medium".into());
            auditing.next_step = Some("planner_review".into());
            auditing.blocking_reason = Some(format!("audit inputs unavailable: {error}"));
            auditing.last_artifact = Some("spec.md".into());
            let _ = save_state_in_repository(repo_root, state_path, &auditing);
            return Outcome::PostBroken;
        }
    };
    let inputs_before_audit = inputs.snapshot;
    let spec_body = inputs.spec_body;
    let parsed = spec::parse_str(&spec_path.display().to_string(), &spec_body);

    println!(
        "\n=== Post-flight: {} (baseline `{}`) ===",
        spec_path.display(),
        &state.baseline_ref[..state.baseline_ref.len().min(8)]
    );

    if !spec_hash_matches(&state.spec_hash, &spec_body) {
        let blocked = preflight_required_state(
            state,
            "spec changed since approval; review the revised contract and rerun pre-flight",
        );
        if let Err(error) = save_state_in_repository(repo_root, state_path, &blocked) {
            eprintln!("error: persisting required pre-flight: {error}");
        }
        eprintln!(
            "error: spec changed since pre-flight. Review the revised contract, then run `mastermind run-task {} --pre-only`. The original baseline and prior strict/index options are retained.",
            spec_path.display(),
        );
        return Outcome::PostBroken;
    }

    let validated = match open_validated_task_index(index_path, repo_root, state.allow_no_index) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("error: validating post-flight index: {error}");
            return Outcome::PostBroken;
        }
    };
    let durable_index = validated.is_some();
    let mut store = match validated {
        Some(store) => store,
        None => match Store::open_ephemeral() {
            Ok(store) => store,
            Err(error) => {
                eprintln!("error: creating ephemeral docs-only index: {error}");
                return Outcome::PostBroken;
            }
        },
    };
    if durable_index {
        let refresh = match Indexer::new(repo_root).index_all(&mut store, false) {
            Ok(stats) => stats,
            Err(error) => {
                eprintln!("error: refreshing index before post-flight: {error}");
                return Outcome::PostBroken;
            }
        };
        if refresh.files_failed > 0 {
            eprintln!(
                "error: post-flight index refresh failed for {} file(s); refusing to audit stale graph data",
                refresh.files_failed
            );
            return Outcome::PostBroken;
        }
        drop(store);
        store = match open_current_task_snapshot(index_path, repo_root) {
            Ok(store) => store,
            Err(error) => {
                eprintln!("error: freezing refreshed post-flight index: {error}");
                return Outcome::PostBroken;
            }
        };
    }

    let report_path = spec_path
        .parent()
        .unwrap_or(spec_path)
        .join("executor-report.md");
    let executor_report = match crate::executor_report::parse_canonical_str(&inputs.executor_body) {
        Ok(report) => report,
        Err(error) => {
            eprintln!(
                "error: post-flight requires a canonical executor report at `{}`: {error}",
                report_path.display()
            );
            let mut failed = auditing.clone();
            failed.status = "held".into();
            failed.risk = Some("medium".into());
            failed.next_step = Some("planner_review".into());
            failed.blocking_reason = Some(format!("executor report rejected: {error}"));
            failed.last_artifact = Some("executor-report.md".into());
            let _ = save_state_in_repository(repo_root, state_path, &failed);
            return Outcome::PostBroken;
        }
    };

    let audit = match audit_spec::run_with_report(
        &parsed,
        &store,
        repo_root,
        &state.baseline_ref,
        Some(&executor_report),
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: audit-spec: {e:?}");
            return Outcome::PostBroken;
        }
    };
    if let Err(error) = store.ensure_source_snapshot_current() {
        eprintln!("error: index changed during post-flight analysis: {error}");
        return Outcome::PostBroken;
    }
    let audit_body = audit.render_text();
    print!("{audit_body}");
    let audit_path = spec_path.parent().unwrap_or(spec_path).join("audit.md");
    if let Err(error) =
        bounded_fs::write_atomic_regular_file(repo_root, &audit_path, audit_body.as_bytes(), false)
    {
        eprintln!(
            "error: failed to persist `{}`: {error}",
            audit_path.display()
        );
        let mut failed = auditing.clone();
        failed.status = "held".into();
        failed.risk = Some("high".into());
        failed.next_step = Some("planner_review".into());
        failed.blocking_reason = Some("failed to persist audit.md".into());
        failed.last_artifact = Some("executor-report.md".into());
        let _ = save_state_in_repository(repo_root, state_path, &failed);
        return Outcome::PostBroken;
    }

    // Mechanical failures become deduplicated candidates. Only semantic review
    // can promote one to an active reusable lesson.
    match crate::lessons::append_audit_candidate(repo_root, spec_path, &audit) {
        Ok(true) => println!("  recorded lesson candidate → .mastermind/tasks/_lessons.md"),
        Err(e) => eprintln!("  warning: lessons append failed: {e}"),
        _ => {}
    }

    let verdict_label = match audit.verdict {
        audit_spec::Verdict::Held => "✅ Held",
        audit_spec::Verdict::Drift => "⚠️ Drift",
        audit_spec::Verdict::Broken => "❌ Broken",
    };
    let outcome = match audit.verdict {
        audit_spec::Verdict::Held => Outcome::PostHeld,
        audit_spec::Verdict::Drift => Outcome::PostDrift,
        audit_spec::Verdict::Broken => Outcome::PostBroken,
    };

    if let Some(hint) = comment_audit_hint(outcome, &state.baseline_ref) {
        println!("{hint}");
    }

    if matches!(outcome, Outcome::PostHeld) {
        if history_input_snapshot(repo_root, spec_path, state)
            .ok()
            .map(|inputs| inputs.snapshot)
            .as_deref()
            != Some(inputs_before_audit.as_str())
        {
            eprintln!("error: audit inputs changed during post-flight; re-audit the current work");
            return Outcome::PostBroken;
        }
        let held_snapshot_sha256 = match parsed.frontmatter.as_ref() {
            Some(frontmatter)
                if frontmatter.mode.as_deref() == Some("strict")
                    && frontmatter.code_paths().next().is_some() =>
            {
                let touches = frontmatter
                    .code_paths()
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                match strict_workflow_snapshot(repo_root, &state.baseline_ref, &touches) {
                    Ok(snapshot) => Some(snapshot),
                    Err(error) => {
                        eprintln!(
                            "warning: held audit has no architecture-policy snapshot: {error}"
                        );
                        None
                    }
                }
            }
            _ => None,
        };
        let notes = compute_release_notes(
            &parsed,
            &spec_body,
            repo_root,
            &state.baseline_ref,
            verdict_label,
        );
        let body = render_release_notes(&notes);
        println!("\n--- Release notes draft ---\n{body}");
        let release_path = release_file_path(repo_root, spec_path);
        if let Err(error) =
            bounded_fs::write_atomic_regular_file(repo_root, &release_path, body.as_bytes(), false)
        {
            eprintln!(
                "error: failed to write release notes `{}`: {error}",
                release_path.display()
            );
            return Outcome::PostBroken;
        }
        println!("Release notes saved to {}", release_path.display());
        let history_snapshot =
            match history_audit_snapshot(repo_root, spec_path, &inputs_before_audit) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    eprintln!("error: cannot bind the history review: {error}");
                    return Outcome::PostBroken;
                }
            };
        if let Err(error) =
            ensure_history_review(repo_root, spec_path, &release_path, &history_snapshot).map(
                |created| {
                    if created {
                        println!(
                            "History review saved to {}",
                            history_review_file_path(repo_root, spec_path).display()
                        );
                    }
                },
            )
        {
            eprintln!("error: failed to create history review: {error}");
            return Outcome::PostBroken;
        }
        if durable_index {
            if let Err(error) = refresh_durable_history_at(index_path, repo_root) {
                eprintln!("error: refreshing durable post-flight history: {error}");
                return Outcome::PostBroken;
            }
        }
        if current_history_snapshot(repo_root, spec_path, state)
            .ok()
            .as_deref()
            != Some(history_snapshot.as_str())
        {
            eprintln!(
                "error: audit evidence changed while recording history; re-audit the current work"
            );
            return Outcome::PostBroken;
        }
        let review_path = history_review_file_path(repo_root, spec_path);
        let mut complete = state.clone();
        if history_review_complete_for_snapshot(&review_path, Some(&history_snapshot)) {
            complete.status = "learned".into();
            complete.next_step = Some("close".into());
        } else {
            complete.status = "history_review_required".into();
            complete.next_step = Some("review_history".into());
        }
        complete.risk = Some("low".into());
        complete.blocking_reason = None;
        complete.last_artifact = Some("history-review.md".into());
        complete.held_snapshot_sha256 = held_snapshot_sha256;
        complete.held_snapshot_version = STRICT_SNAPSHOT_VERSION;
        complete.history_snapshot_sha256 = Some(history_snapshot);
        if let Err(error) = save_state_in_repository(repo_root, state_path, &complete) {
            eprintln!(
                "error: persisting post-flight state `{}`: {error}",
                state_path.display()
            );
            return Outcome::PostBroken;
        }
    } else {
        if durable_index {
            if let Err(error) = refresh_durable_history_at(index_path, repo_root) {
                eprintln!("error: refreshing durable failed-audit history: {error}");
                return Outcome::PostBroken;
            }
        }
        let mut failed = auditing.clone();
        failed.status = match outcome {
            Outcome::PostDrift => "drift",
            _ => "broken",
        }
        .into();
        failed.risk = Some(
            if matches!(outcome, Outcome::PostBroken) {
                "high"
            } else {
                "medium"
            }
            .into(),
        );
        failed.next_step = Some("planner_review".into());
        failed.blocking_reason = Some(format!("post-flight verdict: {verdict_label}"));
        failed.last_artifact = Some("audit.md".into());
        if let Err(error) = save_state_in_repository(repo_root, state_path, &failed) {
            eprintln!(
                "warning: persisting failed state `{}`: {error}",
                state_path.display()
            );
        }
        println!(
            "\nVerdict is {verdict_label} — release notes deferred. State kept at `{}` for re-run after fixes.",
            state_path.display()
        );
    }

    outcome
}

fn comment_audit_hint(outcome: Outcome, baseline_ref: &str) -> Option<String> {
    matches!(outcome, Outcome::PostHeld | Outcome::PostDrift).then(|| {
        format!(
            "  next: inspect the comment delta vs `{baseline_ref}`; run `mastermind-comment-audit` only when it is non-empty"
        )
    })
}

/// Invoke `claude -p` synchronously on this spec, streaming stdout/stderr to the
/// user's terminal. Err on spawn failure or non-zero exit so the caller keeps
/// state for retry.
fn run_executor(spec_path: &Path, repo_root: &Path) -> Result<(), String> {
    let prompt = format!(
        "Implement the mastermind spec at `{}` using the mastermind-task-executor workflow. \
         Implement its approved outcomes inside Scope, prove the Acceptance Criteria, and run \
         the Final Verification commands. Repair implementation-caused failures in a bounded \
         loop, but stop for contract drift, missing prerequisites, or unsafe scope expansion. \
         Write the canonical report to \
         `<task>/executor-report.md`; do not write lifecycle state. Ensure `mmcg` is available \
         via your MCP configuration so verify/audit gates have the live index.",
        spec_path.display(),
    );
    let claude = crate::setup::resolve_native_cli("claude", repo_root)
        .map_err(|error| format!("resolve claude: {error}"))?;
    let status = Command::new(claude)
        .arg("-p")
        .arg(&prompt)
        .stdin(std::process::Stdio::null())
        .current_dir(repo_root)
        .status()
        .map_err(|e| {
            format!("spawn claude: {e} — is the Claude Code CLI installed and on PATH?")
        })?;
    if !status.success() {
        return Err(format!("claude exited with {status}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::Indexer;
    use std::env;
    use std::fs;
    use std::process::Command;

    #[test]
    fn comment_audit_hint_is_withheld_while_the_executor_still_iterates() {
        assert!(comment_audit_hint(Outcome::PostBroken, "main").is_none());

        for outcome in [Outcome::PostHeld, Outcome::PostDrift] {
            let hint = comment_audit_hint(outcome, "main")
                .unwrap_or_else(|| panic!("expected a hint for {outcome:?}"));
            assert!(hint.contains("mastermind-comment-audit"), "{hint}");
            assert!(hint.contains("only when it is non-empty"), "{hint}");
            assert!(hint.contains("main"), "{hint}");
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let p = env::temp_dir().join(format!(
            "mmcg-runtask-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn init_repo(dir: &Path) {
        for args in [
            ["init", "-q", "--initial-branch=main"].as_slice(),
            ["config", "user.email", "t@t"].as_slice(),
            ["config", "user.name", "t"].as_slice(),
            ["config", "commit.gpgsign", "false"].as_slice(),
        ] {
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
        // Ignore SQLite index files at repo root — tests put them there for
        // convenience, but they'd flood `git diff` with scope-creep noise.
        // `.mastermind/` is already filtered by audit_spec.
        fs::write(dir.join(".gitignore"), "idx.db\nidx.db-*\n").unwrap();
    }

    fn git(dir: &Path, args: &[&str]) {
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

    #[cfg(unix)]
    #[test]
    fn exec_uses_repo_root_as_child_working_directory() {
        const CHILD_ROOT: &str = "MMCG_RUN_TASK_CWD_TEST_ROOT";
        const CWD_CAPTURE: &str = "MMCG_RUN_TASK_CWD_TEST_CAPTURE";

        if let Some(root) = env::var_os(CHILD_ROOT) {
            let root = PathBuf::from(root);
            let spec_path = root.join(".mastermind/tasks/001-cwd/spec.md");
            let outcome = run(
                &spec_path,
                &root,
                &root.join("idx.db"),
                RunOpts {
                    exec: true,
                    allow_no_index: true,
                    ..Default::default()
                },
            );
            assert_eq!(outcome, Outcome::ExecFailed);
            return;
        }

        use std::os::unix::fs::PermissionsExt;

        let sandbox = tmp("executor_cwd");
        let root = sandbox.join("repo");
        let bin = sandbox.join("bin");
        let capture = sandbox.join("claude-cwd.txt");
        fs::create_dir_all(root.join(".mastermind/tasks/001-cwd")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        init_repo(&root);
        let spec_path = root.join(".mastermind/tasks/001-cwd/spec.md");
        fs::write(
            &spec_path,
            "# Executor cwd\n\n\
## Goals\n- Run the executor from the repository root.\n\
## Alternatives Considered\n- Keep the caller's cwd — rejected.\n\
## Tests Plan\n- Capture the child cwd.\n\
## Documentation Plan\n- n/a\n\
## Observability Plan\n- n/a\n\
## Performance Considerations\n- O(1)\n",
        )
        .unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "baseline"]);

        let fake_claude = bin.join("claude");
        fs::write(
            &fake_claude,
            "#!/bin/sh\npwd > \"$MMCG_RUN_TASK_CWD_TEST_CAPTURE\"\nexit 1\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake_claude).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_claude, permissions).unwrap();

        let mut path_entries = vec![bin];
        path_entries.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        let output = Command::new(env::current_exe().unwrap())
            .arg("exec_uses_repo_root_as_child_working_directory")
            .env(CHILD_ROOT, &root)
            .env(CWD_CAPTURE, &capture)
            .env("PATH", env::join_paths(path_entries).unwrap())
            .current_dir(&sandbox)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child test failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let actual = fs::read_to_string(&capture).unwrap();
        assert_eq!(
            Path::new(actual.trim()).canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
        fs::remove_dir_all(&sandbox).ok();
    }

    fn write_executor_report(spec_path: &Path, files: &[&str]) {
        let files_yaml = files
            .iter()
            .map(|file| format!("  - {file}"))
            .collect::<Vec<_>>()
            .join("\n");
        let report = format!(
            "<!-- mastermind:report-begin -->\n```yaml\n\
schema_version: 1\n\
spec: {}\n\
status: complete\n\
phases:\n  - id: \"1\"\n    status: done\n\
files_modified:\n{}\n\
claims: []\n\
defects: []\n\
verifications: []\n\
```\n<!-- mastermind:report-end -->\n",
            spec_path.display(),
            files_yaml
        );
        fs::write(
            spec_path
                .parent()
                .unwrap_or(spec_path)
                .join("executor-report.md"),
            report,
        )
        .unwrap();
    }

    #[test]
    fn state_file_roundtrips_through_json() {
        let dir = tmp("state_roundtrip");
        let path = dir.join("s.json");
        let state = RunState {
            status: "approved".into(),
            risk: Some("low".into()),
            next_step: Some("run_executor".into()),
            blocking_reason: None,
            last_artifact: Some("spec.md".into()),
            spec_path: "specs/foo.md".into(),
            spec_hash: "deadbeefcafef00d".into(),
            baseline_ref: "abc1234".into(),
            held_snapshot_sha256: Some("feedface".into()),
            held_snapshot_version: STRICT_SNAPSHOT_VERSION,
            history_snapshot_sha256: Some("reviewed-snapshot".into()),
            started_at: 123456,
            iteration: 0,
            allow_no_index: true,
            strict: false,
        };
        save_state(&path, &state).unwrap();
        let loaded = load_state(&path).unwrap().expect("present");
        assert_eq!(loaded.spec_path, state.spec_path);
        assert_eq!(loaded.spec_hash, state.spec_hash);
        assert_eq!(loaded.baseline_ref, state.baseline_ref);
        assert_eq!(loaded.held_snapshot_sha256, state.held_snapshot_sha256);
        assert_eq!(loaded.held_snapshot_version, STRICT_SNAPSHOT_VERSION);
        let mut legacy = serde_json::to_value(&state).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("held_snapshot_version");
        assert_eq!(
            serde_json::from_value::<RunState>(legacy)
                .unwrap()
                .held_snapshot_version,
            1
        );
        assert_eq!(
            loaded.history_snapshot_sha256,
            state.history_snapshot_sha256
        );
        assert_eq!(loaded.started_at, state.started_at);
        assert!(loaded.allow_no_index);
        delete_state(&path).unwrap();
        assert!(load_state(&path).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn history_review_requires_unambiguous_prose_and_matching_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("history-review.md");
        let complete = "- **Context:** updated\n- **Lesson:** not applicable\n- **Reason:** documented retry semantics\n";
        for invalid in [
            format!("```markdown\n{complete}```\n"),
            format!("~~~\n{complete}~~~\n"),
            format!("<!--\n{complete}-->\n"),
            complete
                .lines()
                .map(|line| format!("    {line}\n"))
                .collect(),
            complete.lines().map(|line| format!("> {line}\n")).collect(),
            format!("{complete}- **Context:** pending\n"),
            format!("{complete}- **Reason:** another answer\n"),
        ] {
            fs::write(&path, invalid).unwrap();
            assert!(!history_review_complete(&path));
        }
        fs::write(&path, format!("```md\n{complete}```\n{complete}")).unwrap();
        assert!(history_review_complete(&path));
        assert!(!history_review_complete_for_snapshot(
            &path,
            Some("current")
        ));
        fs::write(
            &path,
            format!(
                "- **Audit snapshot:** current\r\n{}",
                complete.replace('\n', "\r\n")
            ),
        )
        .unwrap();
        assert!(history_review_complete_for_snapshot(&path, Some("current")));
        assert!(!history_review_complete_for_snapshot(
            &path,
            Some("foreign")
        ));
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(HISTORY_REVIEW_BYTE_LIMIT + 1)
            .unwrap();
        assert!(!history_review_complete(&path));
    }

    fn history_snapshot_fixture() -> (tempfile::TempDir, PathBuf, RunState) {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/lib.py"), "def value(): return 0\n").unwrap();
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        let spec = root.path().join(".mastermind/tasks/001-review/spec.md");
        fs::create_dir_all(spec.parent().unwrap()).unwrap();
        let body = "---\nmode: strict\ntouches:\n  - file: src/lib.py\n---\n# Value\n";
        fs::write(&spec, body).unwrap();
        write_executor_report(&spec, &["src/lib.py"]);
        fs::write(spec.with_file_name("audit.md"), "✅ Held — Value\n").unwrap();
        let release = release_file_path(root.path(), &spec);
        fs::create_dir_all(release.parent().unwrap()).unwrap();
        fs::write(release, "# Value\nAudit: Held\n").unwrap();
        fs::write(root.path().join("src/lib.py"), "def value(): return 1\n").unwrap();
        let state: RunState = serde_json::from_value(serde_json::json!({
            "status": "history_review_required", "spec_path": spec.to_string_lossy(),
            "spec_hash": hash_text(body), "baseline_ref": git_head(root.path()).unwrap(),
            "started_at": 1, "iteration": 1
        }))
        .unwrap();
        (root, spec, state)
    }

    #[test]
    fn history_binding_tracks_audited_bytes_and_iteration_not_staging_or_history_outputs() {
        let (root, spec, state) = history_snapshot_fixture();
        let snapshot = current_history_snapshot(root.path(), &spec, &state).unwrap();
        for (path, body) in [
            ("CONTEXT.md", "durable decision\n"),
            ("CONTEXT-archive-2026.md", "older decisions\n"),
            (".mastermind/tasks/_lessons.md", "reviewed lesson\n"),
        ] {
            fs::write(root.path().join(path), body).unwrap();
        }
        assert_eq!(
            current_history_snapshot(root.path(), &spec, &state).unwrap(),
            snapshot
        );
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-qm", "same reviewed bytes"]);
        assert_eq!(
            current_history_snapshot(root.path(), &spec, &state).unwrap(),
            snapshot
        );
        let mut next = state.clone();
        next.iteration += 1;
        assert_ne!(
            current_history_snapshot(root.path(), &spec, &next).unwrap(),
            snapshot
        );
        for path in [
            root.path().join("src/lib.py"),
            spec.clone(),
            spec.with_file_name("executor-report.md"),
            spec.with_file_name("audit.md"),
            release_file_path(root.path(), &spec),
        ] {
            let before = fs::read(&path).unwrap();
            let mut changed = before.clone();
            changed.extend_from_slice(b"changed\n");
            fs::write(&path, changed).unwrap();
            assert_ne!(
                current_history_snapshot(root.path(), &spec, &state).unwrap(),
                snapshot,
                "{}",
                path.display()
            );
            fs::write(&path, before).unwrap();
        }
        fs::write(
            root.path().join("src/new.py"),
            "new untracked implementation\n",
        )
        .unwrap();
        assert_ne!(
            current_history_snapshot(root.path(), &spec, &state).unwrap(),
            snapshot
        );
        fs::remove_file(root.path().join("src/new.py")).unwrap();
        let body = fs::read_to_string(&spec).unwrap().replace(
            "  - file: src/lib.py",
            "  - file: src/lib.py\n  - file: CONTEXT.md",
        );
        fs::write(&spec, body).unwrap();
        let declared_context = current_history_snapshot(root.path(), &spec, &state).unwrap();
        fs::write(
            root.path().join("CONTEXT.md"),
            "changed declared documentation\n",
        )
        .unwrap();
        assert_ne!(
            current_history_snapshot(root.path(), &spec, &state).unwrap(),
            declared_context
        );
        fs::remove_file(spec.with_file_name("executor-report.md")).unwrap();
        assert!(current_history_snapshot(root.path(), &spec, &state).is_err());
    }

    #[test]
    fn changed_audit_generation_archives_review_and_requires_new_dispositions() {
        let (root, spec, state) = history_snapshot_fixture();
        let release = release_file_path(root.path(), &spec);
        let snapshot = current_history_snapshot(root.path(), &spec, &state).unwrap();
        assert!(ensure_history_review(root.path(), &spec, &release, &snapshot).unwrap());
        let review = history_review_file_path(root.path(), &spec);
        let body = fs::read_to_string(&review)
            .unwrap()
            .replace("pending", "not applicable")
            .replace(
                "**Reason:** semantic review required",
                "**Reason:** reviewed implementation; no durable lesson",
            );
        fs::write(&review, &body).unwrap();
        assert!(history_review_complete_for_snapshot(
            &review,
            Some(&snapshot)
        ));
        assert!(!ensure_history_review(root.path(), &spec, &release, &snapshot).unwrap());
        assert_eq!(fs::read_to_string(&review).unwrap(), body);
        // The rendered Held audit is unchanged, but the function body differs.
        fs::write(root.path().join("src/lib.py"), "def value(): return 2\n").unwrap();
        let new_snapshot = current_history_snapshot(root.path(), &spec, &state).unwrap();
        assert_ne!(new_snapshot, snapshot);
        assert!(ensure_history_review(root.path(), &spec, &release, &new_snapshot).unwrap());
        assert!(!history_review_complete_for_snapshot(
            &review,
            Some(&new_snapshot)
        ));
        let archive = review.with_file_name(format!(
            "history-review.{}.md",
            crate::hex::encode(&Sha256::digest(body.as_bytes()))
        ));
        assert_eq!(fs::read_to_string(archive).unwrap(), body);
    }

    #[test]
    fn history_snapshot_rejects_oversized_and_linked_inputs() {
        let (root, spec, state) = history_snapshot_fixture();
        let source = root.path().join("src/lib.py");
        fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap()
            .set_len(crate::audit_bundle::BUNDLE_INPUT_MAX as u64 + 1)
            .unwrap();
        assert!(current_history_snapshot(root.path(), &spec, &state).is_err());
        #[cfg(unix)]
        {
            fs::remove_file(&source).unwrap();
            std::os::unix::fs::symlink("../.mastermind/tasks/001-review/spec.md", &source).unwrap();
            assert!(current_history_snapshot(root.path(), &spec, &state).is_err());
        }
    }

    #[test]
    fn release_summary_includes_uncommitted_and_untracked_implementation() {
        let (root, spec, state) = history_snapshot_fixture();
        fs::write(root.path().join("src/new.py"), "def added(): pass\n").unwrap();
        let summary = git_diff_stat(root.path(), &state.baseline_ref).unwrap();
        assert!(summary.contains("src/lib.py"), "{summary}");
        assert!(
            summary.contains("Untracked files:\nsrc/new.py"),
            "{summary}"
        );
        assert!(!summary.contains(".mastermind"), "{summary}");
        git(root.path(), &["add", ".mastermind", "src/lib.py"]);
        fs::write(
            spec.with_file_name("audit.md"),
            "updated working artifact\n",
        )
        .unwrap();
        let staged = git_diff_stat(root.path(), &state.baseline_ref).unwrap();
        assert_eq!(staged, summary);
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
    fn risk_query_failure_is_distinct_from_observed_zero() {
        for with_snapshot in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let db = root.path().join("idx.db");
            let store = Store::open(&db).unwrap();
            let raw = rusqlite::Connection::open(&db).unwrap();
            raw.execute_batch(
                "INSERT INTO symbols(id,name,kind,file_path,line_start,line_end,language)
                 VALUES (1,'target','function','src.py',1,2,'python');",
            )
            .unwrap();
            let mut parsed = spec::parse_str("spec.md", "# Risk\n");
            if with_snapshot {
                parsed.pre_edit_snapshot.push(spec::SymbolClaim {
                    name: "target".into(),
                    callers: Some(0),
                    signature: None,
                    raw: String::new(),
                });
            }
            let healthy = compute_risk_report(&parsed, &store).unwrap();
            assert_eq!(healthy.total_snapshot_callers, 0);
            assert!(healthy.files_in_cycles.is_empty());
            raw.execute_batch("DROP TABLE edges").unwrap();
            let error = compute_risk_report(&parsed, &store).unwrap_err();
            assert!(
                error.contains(if with_snapshot {
                    "caller risk"
                } else {
                    "dependency-cycle risk"
                }),
                "{error}"
            );
        }
    }

    #[test]
    fn capped_dependency_graph_cannot_report_no_cycles() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("idx.db");
        let store = Store::open(&db).unwrap();
        let raw = rusqlite::Connection::open(&db).unwrap();
        // 225 * 224 distinct file pairs exceed the cycle query's 50,000 cap.
        raw.execute_batch(
            "WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<225)
             INSERT INTO symbols(id,name,kind,file_path,line_start,line_end,language)
             SELECT id,'Shared','class','src_' || id || '.py',1,2,'python' FROM n;
             INSERT INTO edges(from_id,to_name,kind,line)
             SELECT id,'Shared','imports',1 FROM symbols;",
        )
        .unwrap();
        let mut parsed = spec::parse_str("spec.md", "# Risk\n");
        parsed.mentioned_files = vec!["src_1.py".into()];
        assert!(store.dependency_cycles(None, 2).unwrap().1);
        assert!(compute_risk_report(&parsed, &store)
            .unwrap_err()
            .contains("unknown"));
    }

    #[test]
    fn strict_snapshot_v2_survives_staging_commit_and_detached_checkout() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        git(root.path(), &["config", "core.autocrlf", "false"]);
        for (name, body) in [
            ("source.txt", "before\n"),
            ("gone.txt", "delete\n"),
            ("old.txt", "rename\n"),
        ] {
            fs::write(root.path().join(name), body).unwrap();
        }
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        let baseline = git_head(root.path()).unwrap();
        fs::write(root.path().join("source.txt"), "after\n").unwrap();
        fs::remove_file(root.path().join("gone.txt")).unwrap();
        fs::rename(root.path().join("old.txt"), root.path().join("renamed.txt")).unwrap();
        fs::write(root.path().join("empty.txt"), "").unwrap();
        let paths = [
            "source.txt",
            "gone.txt",
            "old.txt",
            "renamed.txt",
            "empty.txt",
        ]
        .map(String::from)
        .to_vec();
        let approved = strict_workflow_snapshot(root.path(), &baseline, &paths).unwrap();
        let legacy =
            strict_workflow_snapshot_for_version(root.path(), &baseline, &paths, 1).unwrap();
        git(root.path(), &["add", "-A"]);
        assert_eq!(
            strict_workflow_snapshot(root.path(), &baseline, &paths).unwrap(),
            approved
        );
        assert_ne!(
            strict_workflow_snapshot_for_version(root.path(), &baseline, &paths, 1).unwrap(),
            legacy
        );
        git(root.path(), &["commit", "-qm", "implementation"]);
        assert_eq!(
            strict_workflow_snapshot(root.path(), &baseline, &paths).unwrap(),
            approved
        );

        let checkout_parent = tempfile::tempdir().unwrap();
        let checkout = checkout_parent.path().join("ci");
        git(
            root.path(),
            &[
                "worktree",
                "add",
                "--detach",
                checkout.to_str().unwrap(),
                "HEAD",
            ],
        );
        assert_eq!(
            strict_workflow_snapshot(&checkout, &baseline, &paths).unwrap(),
            approved
        );
        fs::write(checkout.join("source.txt"), "after\r\n").unwrap();
        assert_ne!(
            strict_workflow_snapshot(&checkout, &baseline, &paths).unwrap(),
            approved
        );
        fs::write(checkout.join("source.txt"), "after\n").unwrap();
        fs::write(checkout.join("gone.txt"), "").unwrap();
        assert_ne!(
            strict_workflow_snapshot(&checkout, &baseline, &paths).unwrap(),
            approved
        );
        fs::remove_file(checkout.join("gone.txt")).unwrap();
        assert_ne!(
            strict_workflow_snapshot(&checkout, &baseline, &paths[..4]).unwrap(),
            approved
        );
        assert!(strict_workflow_snapshot_for_version(&checkout, &baseline, &paths, 999).is_err());
        assert!(strict_workflow_snapshot(&checkout, &"0".repeat(40), &paths).is_err());
    }

    #[test]
    fn strict_snapshot_v2_uses_index_modes_when_filemode_is_disabled() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        git(root.path(), &["config", "core.filemode", "false"]);
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        let baseline = git_head(root.path()).unwrap();
        fs::write(root.path().join("run.sh"), "echo test").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                root.path().join("run.sh"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let paths = vec!["run.sh".into()];
        let approved = strict_workflow_snapshot(root.path(), &baseline, &paths).unwrap();
        git(root.path(), &["add", "run.sh"]);
        assert_eq!(
            strict_workflow_snapshot(root.path(), &baseline, &paths).unwrap(),
            approved
        );
        git(root.path(), &["update-index", "--chmod=+x", "run.sh"]);
        let executable = strict_workflow_snapshot(root.path(), &baseline, &paths).unwrap();
        assert_ne!(executable, approved);
        git(root.path(), &["commit", "-qm", "executable"]);
        assert_eq!(
            strict_workflow_snapshot(root.path(), &baseline, &paths).unwrap(),
            executable
        );
    }

    #[cfg(unix)]
    #[test]
    fn strict_snapshot_v2_tracks_owner_execute_bit_and_literal_paths() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        git(root.path(), &["config", "core.filemode", "true"]);
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        let baseline = git_head(root.path()).unwrap();
        let name = ":(glob)run*.sh";
        let source = root.path().join(name);
        fs::write(&source, "echo test").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap();
        let paths = vec![name.into()];
        let plain = strict_workflow_snapshot(root.path(), &baseline, &paths).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o654)).unwrap();
        assert_eq!(
            strict_workflow_snapshot(root.path(), &baseline, &paths).unwrap(),
            plain
        );
        fs::set_permissions(&source, fs::Permissions::from_mode(0o744)).unwrap();
        let executable = strict_workflow_snapshot(root.path(), &baseline, &paths).unwrap();
        assert_ne!(executable, plain);
        git(root.path(), &["--literal-pathspecs", "add", "--", name]);
        assert_eq!(
            strict_workflow_snapshot(root.path(), &baseline, &paths).unwrap(),
            executable
        );
    }

    #[test]
    fn strict_snapshot_v2_rejects_indexed_symlinks_materialized_as_regular_files() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        git(root.path(), &["config", "core.filemode", "false"]);
        git(root.path(), &["config", "core.symlinks", "false"]);
        fs::write(root.path().join("link.txt"), "target.txt").unwrap();
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        let baseline = git_head(root.path()).unwrap();
        let paths = vec!["link.txt".into()];
        assert!(strict_workflow_snapshot(root.path(), &baseline, &paths).is_ok());
        let blob = Command::new("git")
            .current_dir(root.path())
            .args(["rev-parse", "HEAD:link.txt"])
            .output()
            .unwrap();
        assert!(blob.status.success());
        let blob = String::from_utf8(blob.stdout).unwrap();
        git(
            root.path(),
            &[
                "update-index",
                "--cacheinfo",
                &format!("120000,{},link.txt", blob.trim()),
            ],
        );
        assert_eq!(
            fs::read(root.path().join("link.txt")).unwrap(),
            b"target.txt"
        );
        assert!(strict_workflow_snapshot(root.path(), &baseline, &paths)
            .unwrap_err()
            .contains("Git file type"));
        fs::remove_file(root.path().join("link.txt")).unwrap();
        assert!(
            strict_workflow_snapshot(root.path(), &baseline, &paths).is_ok(),
            "deleting an indexed link needs no file read"
        );
    }

    #[test]
    fn strict_snapshot_retains_v1_framing_for_files_and_deletions() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        for (name, body) in [
            ("empty.txt", ""),
            ("gone.txt", "old"),
            ("source.txt", "before"),
        ] {
            fs::write(root.path().join(name), body).unwrap();
        }
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        let baseline = git_head(root.path()).unwrap();
        fs::remove_file(root.path().join("gone.txt")).unwrap();
        fs::write(root.path().join("source.txt"), "after").unwrap();
        let paths = vec!["empty.txt".into(), "gone.txt".into(), "source.txt".into()];
        let raw = Command::new("git")
            .current_dir(root.path())
            .args([
                "-c",
                "diff.external=",
                "diff",
                "--raw",
                "--no-abbrev",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                &baseline,
                "--",
                "empty.txt",
                "gone.txt",
                "source.txt",
            ])
            .output()
            .unwrap();
        assert!(raw.status.success());
        let mut legacy = Sha256::new();
        legacy.update(b"mastermind-strict-workflow-snapshot-v1\0");
        legacy.update(baseline.as_bytes());
        legacy.update(b"\0git-raw\0");
        legacy.update(raw.stdout);
        legacy.update(b"\0empty.txt\0file\0");
        legacy.update(0u64.to_le_bytes());
        legacy.update(b"\0gone.txt\0missing\0source.txt\0file\0");
        legacy.update(5u64.to_le_bytes());
        legacy.update(b"after\0");
        assert_eq!(
            strict_workflow_snapshot_for_version(root.path(), &baseline, &paths, 1).unwrap(),
            crate::hex::encode(&legacy.finalize())
        );
    }

    #[test]
    fn strict_snapshot_enforces_per_file_and_total_byte_limits() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        let baseline = git_head(root.path()).unwrap();
        let limit = crate::audit_bundle::BUNDLE_INPUT_MAX as u64;
        let first = fs::File::create(root.path().join("a.txt")).unwrap();
        first.set_len(limit + 1).unwrap();
        assert!(strict_workflow_snapshot(root.path(), &baseline, &["a.txt".into()]).is_err());
        first.set_len(limit).unwrap();
        fs::File::create(root.path().join("b.txt"))
            .unwrap()
            .set_len(limit)
            .unwrap();
        let mut paths = vec!["a.txt".into(), "b.txt".into()];
        assert!(strict_workflow_snapshot(root.path(), &baseline, &paths).is_ok());
        fs::write(root.path().join("c.txt"), "x").unwrap();
        paths.push("c.txt".into());
        assert!(strict_workflow_snapshot(root.path(), &baseline, &paths).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn strict_snapshot_rejects_symlinks_including_dangling_paths() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        let baseline = git_head(root.path()).unwrap();
        fs::create_dir(root.path().join("real")).unwrap();
        fs::write(root.path().join("real/file.txt"), "file").unwrap();
        symlink("real", root.path().join("linked")).unwrap();
        symlink("absent", root.path().join("dangling")).unwrap();
        for path in ["linked/file.txt", "dangling", "dangling/child.txt"] {
            assert!(
                strict_workflow_snapshot(root.path(), &baseline, &[path.into()]).is_err(),
                "{path}"
            );
        }
        assert!(
            strict_workflow_snapshot(root.path(), &baseline, &["missing/child.txt".into()]).is_ok()
        );
    }

    #[test]
    fn extract_h1_title_finds_first_top_level_heading() {
        assert_eq!(
            extract_h1_title("# Add billing webhook\n\n## Goals\n- x"),
            Some("Add billing webhook".to_string())
        );
        assert_eq!(extract_h1_title("## Goals only"), None);
        assert_eq!(extract_h1_title(""), None);
        // H1 behind an H2 doesn't count — it's inside a section.
        assert_eq!(extract_h1_title("## Section\n# Not a title"), None);
    }

    #[test]
    fn render_risk_report_includes_worst_and_cycle_warning() {
        let r = RiskReport {
            snapshot_symbols: 2,
            total_snapshot_callers: 17,
            worst_callers: Some(WorstSymbol {
                name: "SessionStore".into(),
                callers: 12,
            }),
            mentioned_files: 3,
            files_in_cycles: vec!["src/a.rs".into(), "src/b.rs".into()],
            top_central_mentioned: vec![CentralEntry {
                name: "SessionStore".into(),
                in_degree: 12,
            }],
        };
        let out = render_risk_report(&r);
        assert!(out.contains("Snapshot symbols: 2"));
        assert!(out.contains("Worst blast radius: 12"));
        assert!(out.contains("SessionStore"));
        assert!(out.contains("src/a.rs, src/b.rs"));
        assert!(out.contains("in_degree=12"));
    }

    #[test]
    fn render_release_notes_handles_missing_sections() {
        let r = ReleaseNotes {
            title: "Add accessor".into(),
            goals: "".into(),
            tests: "".into(),
            diff_stat: " src/foo.rs | 3 +-\n 1 file changed".into(),
            audit_verdict: "✅ Held".into(),
        };
        let body = render_release_notes(&r);
        assert!(body.starts_with("# Add accessor"));
        assert!(body.contains("(no `## Goals` section"));
        assert!(body.contains("(no `## Tests Plan` section"));
        assert!(body.contains("```\n src/foo.rs"));
        assert!(body.contains("Audit: ✅ Held"));
    }

    #[test]
    fn pre_flight_writes_state_and_executor_handoff() {
        let dir = tmp("pre_writes_state");
        init_repo(&dir);
        // baseline commit so HEAD resolves
        fs::write(dir.join("src.txt"), "x\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);

        // minimal-passing spec
        let spec_dir = dir.join(".mastermind/tasks");
        fs::create_dir_all(&spec_dir).unwrap();
        let spec_path = spec_dir.join("042-thing.md");
        fs::write(
            &spec_path,
            "\
# Thing 042

## Goals
- Edit `src.txt`
## Alternatives Considered
- a — rejected: reason
## Tests Plan
- test_thing
## Documentation Plan
- README touch
## Observability Plan
- n/a
## Performance Considerations
- O(1)
",
        )
        .unwrap();

        let index_path = dir.join("idx.db");
        let _ = Store::open(&index_path).unwrap();
        let opts = RunOpts {
            pre_only: true,       // don't resume / exec
            allow_no_index: true, // fixture has no source — skip index check
            ..Default::default()
        };
        let outcome = run(&spec_path, &dir, &index_path, opts);
        assert_eq!(outcome, Outcome::PreReady);

        let state_path = state_file_path(&dir, &spec_path);
        let state = load_state(&state_path)
            .unwrap()
            .expect("pre-flight should have written state");
        assert!(!state.baseline_ref.is_empty());
        assert_eq!(state.spec_hash.len(), 64);
        assert!(state.spec_path.ends_with("042-thing.md"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pre_flight_failure_does_not_write_state() {
        let dir = tmp("pre_fails_no_state");
        init_repo(&dir);
        fs::write(dir.join("x.txt"), "x\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);

        // Spec missing mandatory sections → verify_spec fails.
        let spec_dir = dir.join(".mastermind/tasks");
        fs::create_dir_all(&spec_dir).unwrap();
        let spec_path = spec_dir.join("099-bad.md");
        fs::write(&spec_path, "# Bad\n\n## Goals\n\n").unwrap();
        let index_path = dir.join("idx.db");
        let _ = Store::open(&index_path).unwrap();

        let outcome = run(
            &spec_path,
            &dir,
            &index_path,
            RunOpts {
                pre_only: true,
                allow_no_index: true, // isolate failure to verify-spec
                ..Default::default()
            },
        );
        assert_eq!(outcome, Outcome::PreFailed);
        let state_path = state_file_path(&dir, &spec_path);
        assert!(
            load_state(&state_path).unwrap().is_none(),
            "no state file should have been written on failed pre-flight"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pre_flight_fails_without_index_by_default() {
        let dir = tmp("no_index_default_fails");
        init_repo(&dir);
        fs::write(dir.join("x.txt"), "x\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);

        let spec_dir = dir.join(".mastermind/tasks");
        fs::create_dir_all(&spec_dir).unwrap();
        let spec_path = spec_dir.join("080-thing.md");
        // Valid spec body — failure should be index-only, not verify-spec.
        fs::write(
            &spec_path,
            "\
# T

## Goals
- Edit `x.txt`
## Alternatives Considered
- a — rejected
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
",
        )
        .unwrap();
        let index_path = dir.join("idx.db");
        // Open the store to materialize the file but leave it empty.
        let _ = Store::open(&index_path).unwrap();

        // Default opts → hard-fail because index has 0 symbols.
        let outcome = run(
            &spec_path,
            &dir,
            &index_path,
            RunOpts {
                pre_only: true,
                ..Default::default()
            },
        );
        assert_eq!(outcome, Outcome::PreFailed);
        let state_path = state_file_path(&dir, &spec_path);
        assert!(
            load_state(&state_path).unwrap().is_none(),
            "no state file should have been written on index-empty failure"
        );

        // Same setup + --allow-no-index → succeeds.
        let outcome = run(
            &spec_path,
            &dir,
            &index_path,
            RunOpts {
                pre_only: true,
                allow_no_index: true,
                ..Default::default()
            },
        );
        assert_eq!(outcome, Outcome::PreReady);
        assert!(
            load_state(&state_path).unwrap().is_some(),
            "state should be written when allow_no_index permits"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pre_flight_rejects_an_index_from_another_repository() {
        let indexed_root = tmp("foreign_index_source");
        init_repo(&indexed_root);
        fs::create_dir_all(indexed_root.join("src")).unwrap();
        fs::write(
            indexed_root.join("src/lib.py"),
            "def foreign_symbol(): pass\n",
        )
        .unwrap();
        git(&indexed_root, &["add", "-A"]);
        git(&indexed_root, &["commit", "-q", "-m", "foreign baseline"]);
        let index_path = indexed_root.join("idx.db");
        let mut store = Store::open(&index_path).unwrap();
        Indexer::new(&indexed_root)
            .index_all(&mut store, false)
            .unwrap();
        drop(store);

        let target_root = tmp("foreign_index_target");
        init_repo(&target_root);
        fs::create_dir_all(target_root.join("src")).unwrap();
        fs::write(target_root.join("src/lib.py"), "def local_symbol(): pass\n").unwrap();
        git(&target_root, &["add", "-A"]);
        git(&target_root, &["commit", "-q", "-m", "target baseline"]);
        let task_dir = target_root.join(".mastermind/tasks/081-root-binding");
        fs::create_dir_all(&task_dir).unwrap();
        let spec_path = task_dir.join("spec.md");
        fs::write(
            &spec_path,
            "# Root binding\n\n## Goals\n- Edit `src/lib.py`\n## Alternatives Considered\n- none\n## Tests Plan\n- n/a\n## Documentation Plan\n- n/a\n## Observability Plan\n- n/a\n## Performance Considerations\n- O(1)\n",
        )
        .unwrap();

        assert_eq!(
            run(
                &spec_path,
                &target_root,
                &index_path,
                RunOpts {
                    pre_only: true,
                    ..Default::default()
                }
            ),
            Outcome::PreFailed,
            "a populated index must be bound to the repository it was built from"
        );
        assert!(
            load_state(&state_file_path(&target_root, &spec_path))
                .unwrap()
                .is_none(),
            "a root mismatch must fail before lifecycle state is written"
        );

        fs::remove_dir_all(indexed_root).ok();
        fs::remove_dir_all(target_root).ok();
    }

    #[test]
    fn allow_no_index_rejects_an_empty_index_bound_to_another_repository() {
        let (root, spec, _) = preflight_fixture();
        let foreign = tempfile::tempdir().unwrap();
        let index_path = foreign.path().join("empty.db");
        let store = Store::open(&index_path).unwrap();
        store
            .set_meta(
                "index_root",
                foreign.path().canonicalize().unwrap().to_str().unwrap(),
            )
            .unwrap();
        drop(store);

        assert_eq!(
            run(
                &spec,
                root.path(),
                &index_path,
                RunOpts {
                    pre_only: true,
                    allow_no_index: true,
                    ..Default::default()
                }
            ),
            Outcome::PreFailed
        );
        assert!(!state_file_path(root.path(), &spec).exists());
    }

    #[test]
    fn task_analysis_snapshot_rejects_results_after_the_index_advances() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::write(
            root.path().join("src/lib.py"),
            "def original_symbol(): pass\n",
        )
        .unwrap();

        let index_path = root.path().join("idx.db");
        let mut writer = Store::open(&index_path).unwrap();
        let stats = Indexer::new(root.path())
            .index_all(&mut writer, false)
            .unwrap();
        assert_eq!(stats.files_failed, 0);
        drop(writer);

        let snapshot = open_current_task_snapshot(&index_path, root.path()).unwrap();
        snapshot.ensure_source_snapshot_current().unwrap();

        let writer = Store::open_existing(&index_path).unwrap();
        writer.set_meta("snapshot_race", "advanced").unwrap();
        assert!(snapshot.ensure_source_snapshot_current().is_err());
    }

    #[test]
    fn auto_resume_post_held_emits_release_notes_and_completes_state() {
        let dir = tmp("autoresume_held");
        init_repo(&dir);
        // baseline: empty source file
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.py"), "def stays(): pass\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);

        let spec_dir = dir.join(".mastermind/tasks");
        fs::create_dir_all(&spec_dir).unwrap();
        let spec_path = spec_dir.join("050-clean-add.md");
        fs::write(
            &spec_path,
            "\
---
mode: strict
touches:
  - file: src/lib.py
    language: python
    symbols:
      - name: stays
verify:
  - cmd: python3 -m py_compile src/lib.py
---
# Clean add

## Goals
- Add `extra()` to `src/lib.py`
## Alternatives Considered
- a — rejected
## Tests Plan
- n/a
## Documentation Plan
- n/a
## Observability Plan
- n/a
## Performance Considerations
- O(1)
",
        )
        .unwrap();

        let index_path = dir.join("idx.db");
        let mut store = Store::open(&index_path).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();
        drop(store);

        // Pre-flight.
        let outcome = run(&spec_path, &dir, &index_path, RunOpts::default());
        assert_eq!(outcome, Outcome::PreReady);
        let state_path = state_file_path(&dir, &spec_path);
        assert!(load_state(&state_path).unwrap().is_some());

        // Simulate executor: add the new function, commit.
        fs::write(
            dir.join("src/lib.py"),
            "def stays(): pass\ndef extra(): pass\n",
        )
        .unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "executor"]);
        let write_report = || {
            let report = serde_json::json!({
                "schema_version": 1, "spec": spec_path.display().to_string(),
                "status": "complete", "phases": [{"id": "1", "status": "done"}],
                "files_modified": ["src/lib.py"], "claims": [], "defects": [],
                "verifications": [{"cmd": "python3 -m py_compile src/lib.py", "result": "pass"}]
            });
            fs::write(
                spec_path.parent().unwrap().join("executor-report.md"),
                report.to_string(),
            )
            .unwrap();
        };
        write_report();

        // Second run auto-resumes into post-flight. Production must refresh the
        // graph itself; callers should not need a manual `mastermind index .`
        // between executor handoff and audit.
        let outcome = run(&spec_path, &dir, &index_path, RunOpts::default());
        assert_eq!(outcome, Outcome::PostHeld);
        let store = Store::open(&index_path).unwrap();
        assert!(
            store
                .search_symbols("extra", None, None)
                .unwrap()
                .iter()
                .any(|symbol| symbol.file_path == "src/lib.py"),
            "post-flight must audit and retain the refreshed implementation graph"
        );
        drop(store);
        // Held → the mechanical audit is complete, but semantic history review
        // remains an explicit lifecycle phase.
        let completed = load_state(&state_path).unwrap().expect("review state");
        assert_eq!(completed.status, "history_review_required");
        assert!(
            completed.held_snapshot_sha256.is_some(),
            "a held strict task must persist an exact touch-file snapshot"
        );
        assert_eq!(completed.next_step.as_deref(), Some("review_history"));
        assert_eq!(
            completed.last_artifact.as_deref(),
            Some("history-review.md")
        );
        let release_path = release_file_path(&dir, &spec_path);
        assert!(release_path.exists(), "release notes file should exist");
        let body = fs::read_to_string(&release_path).unwrap();
        assert!(body.starts_with("# Clean add"));
        assert!(body.contains("Audit: ✅ Held"));
        let store = Store::open(&index_path).unwrap();
        assert!(
            store
                .search_project_history("Clean add", Some("release_notes"), 10)
                .unwrap()
                .iter()
                .any(|entry| entry.path.ends_with("050-clean-add.md")),
            "post-flight must make newly-written release notes immediately searchable"
        );
        drop(store);
        let review_path = history_review_file_path(&dir, &spec_path);
        let review = fs::read_to_string(&review_path).unwrap();
        assert!(review.contains("**Context:** pending"));
        assert!(review.contains("**Lesson:** pending"));

        // Losing an audited input while review is pending requires re-audit.
        fs::remove_file(spec_path.parent().unwrap().join("executor-report.md")).unwrap();
        assert_eq!(
            run(&spec_path, &dir, &index_path, RunOpts::default()),
            Outcome::PostBroken
        );
        assert_eq!(
            load_state(&state_path).unwrap().unwrap().status,
            "audit_required"
        );
        write_report();
        assert_eq!(
            run(&spec_path, &dir, &index_path, RunOpts::default()),
            Outcome::PostHeld
        );

        fs::write(
            dir.join("CONTEXT.md"),
            "semantic review captured the zebra routing invariant\n",
        )
        .unwrap();
        fs::write(
            &review_path,
            review
                .replace("**Context:** pending", "**Context:** updated")
                .replace("**Lesson:** pending", "**Lesson:** not applicable")
                .replace(
                    "**Reason:** semantic review required",
                    "**Reason:** captured zebra routing in CONTEXT.md",
                ),
        )
        .unwrap();
        assert_eq!(
            run(&spec_path, &dir, &index_path, RunOpts::default()),
            Outcome::PostHeld
        );
        assert_eq!(load_state(&state_path).unwrap().unwrap().status, "learned");
        let store = Store::open(&index_path).unwrap();
        assert!(
            store
                .search_project_history("zebra routing", Some("context"), 10)
                .unwrap()
                .iter()
                .any(|entry| entry.path == "CONTEXT.md"),
            "semantic completion must refresh edited durable history before reporting learned"
        );
        drop(store);
        // A completed historical task is stable as later work changes the
        // checkout. An explicit re-audit must clear completion before failing.
        fs::write(dir.join("src/lib.py"), "def unrelated_later_work(): pass\n").unwrap();
        fs::remove_file(spec_path.with_file_name("executor-report.md")).unwrap();
        assert_eq!(
            run(&spec_path, &dir, &index_path, RunOpts::default()),
            Outcome::PostHeld
        );
        assert_eq!(load_state(&state_path).unwrap().unwrap().status, "learned");
        assert_eq!(
            run(
                &spec_path,
                &dir,
                &index_path,
                RunOpts {
                    post_only: true,
                    ..RunOpts::default()
                }
            ),
            Outcome::PostBroken
        );
        let failed = load_state(&state_path).unwrap().unwrap();
        assert_ne!(failed.status, "learned");
        assert!(failed.history_snapshot_sha256.is_none());
        assert!(failed.held_snapshot_sha256.is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn post_flight_without_executor_report_fails_closed() {
        let dir = tmp("post_requires_report");
        init_repo(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.py"), "def stays(): pass\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);

        let task_dir = dir.join(".mastermind/tasks/051-report-required");
        fs::create_dir_all(&task_dir).unwrap();
        let spec_path = task_dir.join("spec.md");
        fs::write(
            &spec_path,
            "# Report required\n\n## Goals\n- Edit `src/lib.py`\n## Alternatives Considered\n- a — rejected\n## Tests Plan\n- n/a\n## Documentation Plan\n- n/a\n## Observability Plan\n- n/a\n## Performance Considerations\n- O(1)\n",
        )
        .unwrap();

        let index_path = dir.join("idx.db");
        let mut store = Store::open(&index_path).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();
        drop(store);

        assert_eq!(
            run(&spec_path, &dir, &index_path, RunOpts::default()),
            Outcome::PreReady
        );
        let state_path = state_file_path(&dir, &spec_path);
        assert_eq!(
            run(
                &spec_path,
                &dir,
                &index_path,
                RunOpts {
                    post_only: true,
                    ..Default::default()
                }
            ),
            Outcome::PostBroken
        );
        let state = load_state(&state_path).unwrap().unwrap();
        assert_eq!(state.status, "held");
        assert_eq!(state.next_step.as_deref(), Some("planner_review"));
        assert!(state
            .blocking_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("executor-report.md")));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn post_drift_keeps_state_no_release_notes() {
        let dir = tmp("autoresume_drift");
        init_repo(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.py"), "def stays(): pass\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);

        // Spec claims to touch ONLY src/lib.py.
        let spec_dir = dir.join(".mastermind/tasks");
        fs::create_dir_all(&spec_dir).unwrap();
        let spec_path = spec_dir.join("060-scope-creep.md");
        fs::write(
            &spec_path,
            "\
# Scope creep test

## Goals
- Update `src/lib.py`
## Alternatives Considered
- a — rejected
## Tests Plan
- n/a
## Documentation Plan
- n/a
## Observability Plan
- n/a
## Performance Considerations
- O(1)
",
        )
        .unwrap();

        let index_path = dir.join("idx.db");
        let mut store = Store::open(&index_path).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();
        drop(store);

        let _ = run(&spec_path, &dir, &index_path, RunOpts::default());
        let state_path = state_file_path(&dir, &spec_path);

        // Executor added an unmentioned file → scope creep / drift.
        fs::write(
            dir.join("src/lib.py"),
            "def stays(): pass\ndef tweaked(): pass\n",
        )
        .unwrap();
        fs::write(dir.join("src/sneaky.py"), "def extra(): pass\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "executor"]);
        let mut store = Store::open(&index_path).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();
        drop(store);
        write_executor_report(&spec_path, &["src/lib.py", "src/sneaky.py"]);

        let outcome = run(&spec_path, &dir, &index_path, RunOpts::default());
        assert_eq!(outcome, Outcome::PostDrift);
        // Drift → state kept, no release notes.
        assert!(load_state(&state_path).unwrap().is_some());
        let release_path = release_file_path(&dir, &spec_path);
        assert!(!release_path.exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn post_only_without_state_errors() {
        let dir = tmp("postonly_nostate");
        init_repo(&dir);
        fs::write(dir.join("x.txt"), "x\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);

        let spec_dir = dir.join(".mastermind/tasks");
        fs::create_dir_all(&spec_dir).unwrap();
        let spec_path = spec_dir.join("070-thing.md");
        fs::write(&spec_path, "# T\n## Goals\n- x\n").unwrap();
        let index_path = dir.join("idx.db");
        let _ = Store::open(&index_path).unwrap();

        let outcome = run(
            &spec_path,
            &dir,
            &index_path,
            RunOpts {
                post_only: true,
                ..Default::default()
            },
        );
        // PreFailed is the dispatcher's "couldn't get to post" signal —
        // main.rs exits non-zero for it.
        assert_eq!(outcome, Outcome::PreFailed);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn iteration_starts_at_one_on_fresh_preflight() {
        let dir = tmp("iter_fresh");
        init_repo(&dir);
        fs::write(dir.join("src.txt"), "x\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);

        let spec_dir = dir.join(".mastermind/tasks/050-iter");
        fs::create_dir_all(&spec_dir).unwrap();
        let spec_path = spec_dir.join("spec.md");
        fs::write(
            &spec_path,
            "# Iter 050\n\n## Goals\n- Edit `src.txt`\n## Alternatives Considered\n- a — rejected: r\n## Tests Plan\n- t\n## Documentation Plan\n- d\n## Observability Plan\n- n/a\n## Performance Considerations\n- O(1)\n",
        )
        .unwrap();

        let index_path = dir.join("idx.db");
        let _ = Store::open(&index_path).unwrap();
        let opts = RunOpts {
            pre_only: true,
            allow_no_index: true,
            ..Default::default()
        };
        let outcome = run(&spec_path, &dir, &index_path, opts);
        assert_eq!(outcome, Outcome::PreReady);

        let state_path = state_file_path(&dir, &spec_path);
        let state = load_state(&state_path).unwrap().expect("state written");
        assert_eq!(state.iteration, 1, "first pre-flight should be iteration 1");

        fs::remove_dir_all(&dir).ok();
    }

    fn preflight_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        fs::write(root.path().join("src.txt"), "before\n").unwrap();
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        let spec = root.path().join(".mastermind/tasks/001-retry/spec.md");
        fs::create_dir_all(spec.parent().unwrap()).unwrap();
        fs::write(&spec,
            "# Retry\n\n## Goals\n- Edit `src.txt`\n## Alternatives Considered\n- a — rejected: r\n## Tests Plan\n- t\n## Documentation Plan\n- d\n## Observability Plan\n- n/a\n## Performance Considerations\n- O(1)\n"
        ).unwrap();
        let db = root.path().join("idx.db");
        (root, spec, db)
    }

    #[test]
    fn spec_change_requires_explicit_preflight_without_losing_implemented_diff() {
        let (root, spec, db) = preflight_fixture();
        let body = fs::read_to_string(&spec).unwrap();
        // Text files need explicit scope; the prose path heuristic is for
        // recognized source extensions and does not infer a .txt touch.
        fs::write(
            &spec,
            format!("---\ntouches:\n  - file: src.txt\n---\n{body}"),
        )
        .unwrap();
        assert_eq!(
            run(
                &spec,
                root.path(),
                &db,
                RunOpts {
                    pre_only: true,
                    allow_no_index: true,
                    ..Default::default()
                }
            ),
            Outcome::PreReady
        );
        let state_path = state_file_path(root.path(), &spec);
        let first = load_state(&state_path).unwrap().unwrap();
        let approved_body = fs::read_to_string(&spec).unwrap();
        fs::write(root.path().join("src.txt"), "implemented\n").unwrap();
        git(root.path(), &["add", "src.txt"]);
        git(root.path(), &["commit", "-qm", "implementation"]);
        write_executor_report(&spec, &["src.txt"]);
        let revised = approved_body.replace("# Retry", "# Revised retry");
        fs::write(&spec, &revised).unwrap();
        assert_eq!(
            run(&spec, root.path(), &db, RunOpts::default()),
            Outcome::PostBroken
        );
        let blocked = load_state(&state_path).unwrap().unwrap();
        assert_eq!(blocked.status, "held");
        assert_eq!(blocked.next_step.as_deref(), Some("run_preflight"));
        assert!(blocked.held_snapshot_sha256.is_none());
        assert!(blocked.history_snapshot_sha256.is_none());
        assert!(!spec.with_file_name("audit.md").exists());
        assert!(!release_file_path(root.path(), &spec).exists());

        // Restoring old bytes alone does not bypass the failed approval gate.
        fs::write(&spec, &approved_body).unwrap();
        for post_only in [false, true] {
            assert_eq!(
                run(
                    &spec,
                    root.path(),
                    &db,
                    RunOpts {
                        post_only,
                        ..Default::default()
                    }
                ),
                Outcome::PreFailed
            );
        }
        fs::write(&spec, &revised).unwrap();
        assert_eq!(
            run(
                &spec,
                root.path(),
                &db,
                RunOpts {
                    pre_only: true,
                    ..Default::default()
                }
            ),
            Outcome::PreReady
        );
        let retried = load_state(&state_path).unwrap().unwrap();
        assert_eq!(retried.baseline_ref, first.baseline_ref);
        assert_ne!(retried.baseline_ref, git_head(root.path()).unwrap());
        assert_eq!(retried.iteration, 2);
        assert!(retried.allow_no_index);
        assert_eq!(retried.spec_hash, hash_text(&revised));
        assert!(git_diff_stat(root.path(), &retried.baseline_ref)
            .unwrap()
            .contains("src.txt"));
        assert_eq!(
            run(&spec, root.path(), &db, RunOpts::default()),
            Outcome::PostHeld
        );
        assert!(
            !db.exists(),
            "docs-only post-flight must not materialize an index"
        );
    }

    #[test]
    fn failed_revalidation_revokes_approval_and_preserves_strict_options() {
        let (root, spec, db) = preflight_fixture();
        let body = fs::read_to_string(&spec).unwrap();
        fs::write(&spec, format!(
            "---\ntouches:\n  - file: src.txt\n    symbols:\n      - name: target\nverify:\n  - cmd: git status --short\n---\n{body}"
        )).unwrap();
        assert_eq!(
            run(
                &spec,
                root.path(),
                &db,
                RunOpts {
                    pre_only: true,
                    strict: true,
                    allow_no_index: true,
                    ..Default::default()
                }
            ),
            Outcome::PreReady
        );
        let state_path = state_file_path(root.path(), &spec);
        let mut approved = load_state(&state_path).unwrap().unwrap();
        approved.held_snapshot_sha256 = Some("old-held".into());
        approved.history_snapshot_sha256 = Some("old-history".into());
        save_state_in_repository(root.path(), &state_path, &approved).unwrap();
        fs::write(&spec, body).unwrap();
        // This body passes non-strict pre-flight, but retries retain --strict.
        assert_eq!(
            run(
                &spec,
                root.path(),
                &db,
                RunOpts {
                    pre_only: true,
                    ..Default::default()
                }
            ),
            Outcome::PreFailed
        );
        let failed = load_state(&state_path).unwrap().unwrap();
        assert_eq!(failed.baseline_ref, approved.baseline_ref);
        assert_eq!(failed.iteration, 2);
        assert!(failed.strict && failed.allow_no_index);
        assert_eq!(failed.status, "held");
        assert_eq!(failed.next_step.as_deref(), Some("run_preflight"));
        assert!(failed.held_snapshot_sha256.is_none() && failed.history_snapshot_sha256.is_none());
    }

    #[test]
    fn reset_does_not_overwrite_corrupt_state_or_another_specs_state() {
        let (root, spec, db) = preflight_fixture();
        let state_path = state_file_path(root.path(), &spec);
        fs::write(&state_path, "{corrupt state").unwrap();
        assert_eq!(
            run(
                &spec,
                root.path(),
                &db,
                RunOpts {
                    reset: true,
                    allow_no_index: true,
                    ..Default::default()
                }
            ),
            Outcome::PreFailed
        );
        assert_eq!(fs::read_to_string(&state_path).unwrap(), "{corrupt state");
        let body = fs::read_to_string(&spec).unwrap();
        let first = root.path().join("a/same.md");
        let second = root.path().join("b/same.md");
        for path in [&first, &second] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, &body).unwrap();
        }
        assert_eq!(
            run(
                &first,
                root.path(),
                &db,
                RunOpts {
                    pre_only: true,
                    allow_no_index: true,
                    ..Default::default()
                }
            ),
            Outcome::PreReady
        );
        let legacy_state = state_file_path(root.path(), &first);
        let before = fs::read(&legacy_state).unwrap();
        assert_eq!(
            run(
                &second,
                root.path(),
                &db,
                RunOpts {
                    reset: true,
                    allow_no_index: true,
                    ..Default::default()
                }
            ),
            Outcome::PreFailed
        );
        assert_eq!(fs::read(&legacy_state).unwrap(), before);
    }

    #[test]
    fn conflicting_phase_flags_preserve_existing_approval() {
        let (root, spec, db) = preflight_fixture();
        assert_eq!(
            run(
                &spec,
                root.path(),
                &db,
                RunOpts {
                    pre_only: true,
                    allow_no_index: true,
                    ..Default::default()
                }
            ),
            Outcome::PreReady
        );
        let state_path = state_file_path(root.path(), &spec);
        let before = fs::read(&state_path).unwrap();
        for reset in [false, true] {
            assert_eq!(
                run(
                    &spec,
                    root.path(),
                    &db,
                    RunOpts {
                        reset,
                        pre_only: !reset,
                        post_only: true,
                        ..Default::default()
                    }
                ),
                Outcome::PreFailed
            );
            assert_eq!(fs::read(&state_path).unwrap(), before);
        }
    }

    #[test]
    fn preflight_does_not_approve_failed_cycle_query() {
        let (root, spec, db) = preflight_fixture();
        fs::write(
            root.path().join("a.py"),
            "from b import B\n\ndef A():\n    return 1\n",
        )
        .unwrap();
        fs::write(
            root.path().join("b.py"),
            "from a import A\n\ndef B():\n    return 1\n",
        )
        .unwrap();
        let mut store = Store::open(&db).unwrap();
        Indexer::new(root.path())
            .index_all(&mut store, false)
            .unwrap();
        let raw = rusqlite::Connection::open(&db).unwrap();
        raw.execute(
            "UPDATE symbols SET file_path=CAST(file_path AS BLOB) WHERE file_path='a.py'",
            [],
        )
        .unwrap();
        assert!(store.dependency_cycles(None, 2).is_err());
        drop(raw);
        drop(store);
        assert_eq!(
            run(
                &spec,
                root.path(),
                &db,
                RunOpts {
                    pre_only: true,
                    ..Default::default()
                }
            ),
            Outcome::PreFailed
        );
        assert!(!state_file_path(root.path(), &spec).exists());
    }

    #[test]
    fn allow_no_index_does_not_bypass_an_unreadable_existing_database() {
        let (root, spec, db) = preflight_fixture();
        fs::write(&db, "not a SQLite database").unwrap();
        assert_eq!(
            run(
                &spec,
                root.path(),
                &db,
                RunOpts {
                    pre_only: true,
                    allow_no_index: true,
                    ..Default::default()
                }
            ),
            Outcome::PreFailed
        );
        assert!(!state_file_path(root.path(), &spec).exists());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_state_symlink_cannot_start_a_new_baseline() {
        use std::os::unix::fs::symlink;
        let (root, spec, db) = preflight_fixture();
        let state_path = state_file_path(root.path(), &spec);
        let target = root.path().join("missing-state-target.json");
        symlink(&target, &state_path).unwrap();
        assert!(load_state(&state_path).is_err());
        assert_eq!(
            run(
                &spec,
                root.path(),
                &db,
                RunOpts {
                    reset: true,
                    allow_no_index: true,
                    ..Default::default()
                }
            ),
            Outcome::PreFailed
        );
        assert!(fs::symlink_metadata(&state_path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!target.exists());
    }

    #[test]
    fn reset_preserves_and_increments_iteration() {
        let dir = tmp("iter_reset");
        init_repo(&dir);
        fs::write(dir.join("src.txt"), "x\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);

        let spec_dir = dir.join(".mastermind/tasks/051-iter");
        fs::create_dir_all(&spec_dir).unwrap();
        let spec_path = spec_dir.join("spec.md");
        fs::write(
            &spec_path,
            "# Iter 051\n\n## Goals\n- Edit `src.txt`\n## Alternatives Considered\n- a — rejected: r\n## Tests Plan\n- t\n## Documentation Plan\n- d\n## Observability Plan\n- n/a\n## Performance Considerations\n- O(1)\n",
        )
        .unwrap();

        let index_path = dir.join("idx.db");
        let _ = Store::open(&index_path).unwrap();

        // First pre-flight → iteration 1
        let outcome = run(
            &spec_path,
            &dir,
            &index_path,
            RunOpts {
                pre_only: true,
                allow_no_index: true,
                ..Default::default()
            },
        );
        assert_eq!(outcome, Outcome::PreReady);
        let state_path = state_file_path(&dir, &spec_path);
        assert_eq!(load_state(&state_path).unwrap().unwrap().iteration, 1);

        // --reset → second pre-flight → iteration 2
        let outcome = run(
            &spec_path,
            &dir,
            &index_path,
            RunOpts {
                pre_only: true,
                allow_no_index: true,
                reset: true,
                ..Default::default()
            },
        );
        assert_eq!(outcome, Outcome::PreReady);
        assert_eq!(load_state(&state_path).unwrap().unwrap().iteration, 2);

        // --reset → third pre-flight → iteration 3
        let outcome = run(
            &spec_path,
            &dir,
            &index_path,
            RunOpts {
                pre_only: true,
                allow_no_index: true,
                reset: true,
                ..Default::default()
            },
        );
        assert_eq!(outcome, Outcome::PreReady);
        assert_eq!(load_state(&state_path).unwrap().unwrap().iteration, 3);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn iteration_budget_exhaustion_returns_pre_failed() {
        let dir = tmp("iter_budget_exhausted");
        init_repo(&dir);
        fs::write(dir.join("src.txt"), "x\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);

        let spec_dir = dir.join(".mastermind/tasks/052-iter");
        fs::create_dir_all(&spec_dir).unwrap();
        let spec_path = spec_dir.join("spec.md");
        fs::write(
            &spec_path,
            "# Iter 052\n\n## Goals\n- Edit `src.txt`\n## Alternatives Considered\n- a — rejected: r\n## Tests Plan\n- t\n## Documentation Plan\n- d\n## Observability Plan\n- n/a\n## Performance Considerations\n- O(1)\n",
        )
        .unwrap();

        let index_path = dir.join("idx.db");
        let _ = Store::open(&index_path).unwrap();
        let base_opts = || RunOpts {
            pre_only: true,
            allow_no_index: true,
            ..Default::default()
        };

        // Cycle through iterations 1, 2, 3
        for _ in 0..3 {
            run(
                &spec_path,
                &dir,
                &index_path,
                RunOpts {
                    reset: true,
                    ..base_opts()
                },
            );
        }
        let state_path = state_file_path(&dir, &spec_path);
        assert_eq!(load_state(&state_path).unwrap().unwrap().iteration, 3);

        // 4th attempt with --reset → would be iteration 4 → refused → PreFailed.
        let outcome = run(
            &spec_path,
            &dir,
            &index_path,
            RunOpts {
                reset: true,
                ..base_opts()
            },
        );
        assert_eq!(outcome, Outcome::PreFailed);

        // A refused --reset must not erase the exhausted count. Neither retry
        // flag can silently start a fresh task on the following invocation.
        for reset in [true, false] {
            assert_eq!(
                run(
                    &spec_path,
                    &dir,
                    &index_path,
                    RunOpts {
                        reset,
                        ..base_opts()
                    }
                ),
                Outcome::PreFailed
            );
            let refused = load_state(&state_path).unwrap().unwrap();
            assert_eq!(refused.iteration, 3);
            assert_eq!(refused.status, "held");
            assert_eq!(refused.next_step.as_deref(), Some("run_preflight"));
        }

        // Lesson appended
        let lessons = std::fs::read_to_string(dir.join(".mastermind/tasks/_lessons.md")).unwrap();
        assert!(lessons.contains("iteration_budget_exhausted"));
        assert!(lessons.contains("052-iter"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn force_iteration_bypasses_budget() {
        let dir = tmp("iter_force_bypass");
        init_repo(&dir);
        fs::write(dir.join("src.txt"), "x\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);

        let spec_dir = dir.join(".mastermind/tasks/053-iter");
        fs::create_dir_all(&spec_dir).unwrap();
        let spec_path = spec_dir.join("spec.md");
        fs::write(
            &spec_path,
            "# Iter 053\n\n## Goals\n- Edit `src.txt`\n## Alternatives Considered\n- a — rejected: r\n## Tests Plan\n- t\n## Documentation Plan\n- d\n## Observability Plan\n- n/a\n## Performance Considerations\n- O(1)\n",
        )
        .unwrap();

        let index_path = dir.join("idx.db");
        let _ = Store::open(&index_path).unwrap();
        let base_opts = || RunOpts {
            pre_only: true,
            allow_no_index: true,
            ..Default::default()
        };

        // Burn through the budget.
        for _ in 0..3 {
            run(
                &spec_path,
                &dir,
                &index_path,
                RunOpts {
                    reset: true,
                    ..base_opts()
                },
            );
        }

        // 4th attempt with --force-iteration → should succeed.
        let outcome = run(
            &spec_path,
            &dir,
            &index_path,
            RunOpts {
                reset: true,
                force_iteration: true,
                ..base_opts()
            },
        );
        assert_eq!(outcome, Outcome::PreReady);
        let state_path = state_file_path(&dir, &spec_path);
        assert_eq!(load_state(&state_path).unwrap().unwrap().iteration, 4);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn legacy_state_without_iteration_deserializes_to_zero() {
        let dir = tmp("iter_legacy_state");
        let state_path = dir.join("legacy.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &state_path,
            r#"{"spec_path":"foo.md","spec_hash":"abc","baseline_ref":"HEAD","started_at":0}"#,
        )
        .unwrap();

        let state = load_state(&state_path).unwrap().expect("loads");
        assert_eq!(state.iteration, 0, "missing field defaults to 0");

        fs::remove_dir_all(&dir).ok();
    }
}
