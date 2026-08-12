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
//!    baseline-to-HEAD. To stdout AND `.mastermind/releases/<basename>.md` on Held.
//!
//! State persists beside a canonical task spec as `<task>/state.json`, so every
//! task has one controller-owned lifecycle record. Legacy flat specs keep using
//! `.mastermind/run-state/<basename>.json` to avoid a shared `tasks/state.json`.

use crate::audit_spec;
use crate::indexer::{validate_index_root, Indexer};
use crate::spec::{self, ParsedSpec};
use crate::store::Store;
use crate::verify_spec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const STRICT_EVIDENCE_FILE_LIMIT: usize = 1_000;
const STRICT_EVIDENCE_TOTAL_BYTE_LIMIT: u64 = 32 * 1024 * 1024;
const STRICT_EVIDENCE_GIT_BYTE_LIMIT: usize = 2 * 1024 * 1024;

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
    /// Hash of the spec body at pre-flight. Re-checked at post-flight to warn if
    /// the spec was edited between phases.
    pub spec_hash: String,
    /// `git rev-parse HEAD` captured at pre-flight — the audit's `--since`.
    pub baseline_ref: String,
    /// SHA-256 binding a held strict audit to the exact declared touch files.
    /// Older state files deserialize without it and are not accepted as
    /// architecture-policy evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_snapshot_sha256: Option<String>,
    /// Unix epoch seconds at pre-flight.
    pub started_at: u64,
    /// Iteration count — +1 on every pre-flight entry; first fresh run is `1`.
    /// Survives `--reset` (dispatcher carries the old value forward before
    /// deleting state). Legacy state files lacking this field deserialize to the
    /// serde default `0` — "not yet counted".
    #[serde(default)]
    pub iteration: u32,
    /// Preserve the pre-flight docs/spec-only escape hatch across hand-off so
    /// post-flight does not turn an intentionally ungrounded task into a hard
    /// index-identity failure.
    #[serde(default)]
    pub allow_no_index: bool,
}

fn default_run_status() -> String {
    "approved".into()
}

/// End-to-end result. Mapped to exit codes by `main.rs`: every `*Failed` /
/// `*Broken` variant exits non-zero so CI / scripts can react.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Pre-flight passed, state written, hand-off message printed (no `--exec`).
    PreReady,
    /// `verify_spec` produced errors. State NOT written.
    PreFailed,
    /// Post-flight clean — release notes emitted, state marked complete.
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
    /// Delete any existing state file before deciding which phase to run.
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
) -> std::io::Result<bool> {
    let path = history_review_file_path(repo_root, spec_path);
    if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to write history review through a symlink",
        ));
    }
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
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
- **Context:** pending\n\
- **Lesson:** pending\n\
- **Reason:** semantic review required\n\
- **Evidence:** `{spec}`; `{audit}`; `{release}`\n",
        spec_basename(spec_path),
    );
    std::fs::write(path, body)?;
    Ok(true)
}

/// Return true only after both durable-knowledge dispositions were reviewed
/// and the generated placeholder reason was replaced. The Markdown file remains
/// authoritative; lifecycle commands derive completion from it instead of
/// treating post-flight success as semantic review.
pub fn history_review_complete(review_path: &Path) -> bool {
    let Ok(body) = std::fs::read_to_string(review_path) else {
        return false;
    };
    let field = |name: &str| {
        let prefix = format!("- **{name}:**");
        body.lines()
            .find_map(|line| line.trim().strip_prefix(&prefix))
            .map(str::trim)
    };
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
}

fn refresh_durable_history(store: &mut Store, repo_root: &Path) -> Result<u32, String> {
    Indexer::new(repo_root)
        .index_project_history(store)
        .map(|stats| stats.indexed)
        .map_err(|error| error.to_string())
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
    if !path.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(path)?;
    let state: RunState = serde_json::from_str(&body)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(state))
}

pub fn save_state(path: &Path, state: &RunState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, body)
}

pub fn delete_state(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Deterministic hash of the spec body. `DefaultHasher` is stable within one
/// Rust toolchain — fine for "did the spec change between pre and post" on the
/// same machine. Cross-toolchain-upgrade false positives are harmless (warn,
/// not block).
pub(crate) fn hash_text(text: &str) -> String {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}

pub(crate) fn strict_workflow_snapshot(
    repo_root: &Path,
    baseline_ref: &str,
    touch_files: &[String],
) -> Result<String, String> {
    if !matches!(baseline_ref.len(), 40 | 64)
        || !baseline_ref.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("strict-workflow baseline must be an exact Git object ID".into());
    }
    let mut paths = BTreeSet::new();
    for file in touch_files {
        let normalized = crate::audit_bundle::normalize_relative_path(Path::new(file))
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

    let root = repo_root
        .canonicalize()
        .map_err(|_| "strict-workflow repository root is unavailable".to_string())?;
    let mut digest = Sha256::new();
    digest.update(b"mastermind-strict-workflow-snapshot-v1\0");
    digest.update(baseline_ref.as_bytes());
    digest.update([0]);
    let mut git_args = vec![
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
    let raw = crate::diff::run_bounded_git_with_limit(
        &root,
        &git_args,
        None,
        STRICT_EVIDENCE_GIT_BYTE_LIMIT,
    )
    .map_err(|error| format!("strict-workflow git snapshot failed: {}", error.code()))?;
    if !raw.success {
        return Err("strict-workflow baseline is unavailable".into());
    }
    digest.update(b"git-raw\0");
    digest.update(raw.stdout);
    digest.update([0]);
    let mut total_bytes = 0u64;

    for relative in &paths {
        digest.update(relative.as_bytes());
        digest.update([0]);
        let mut current = root.clone();
        let parts = relative.split('/').collect::<Vec<_>>();
        let mut missing = false;
        for (index, part) in parts.iter().enumerate() {
            current.push(part);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "strict-workflow touch path `{relative}` traverses a symlink"
                    ));
                }
                Ok(metadata) if index + 1 < parts.len() && !metadata.is_dir() => {
                    return Err(format!(
                        "strict-workflow touch path `{relative}` has a non-directory parent"
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing = true;
                    break;
                }
                Err(_) => {
                    return Err(format!(
                        "strict-workflow touch path `{relative}` cannot be inspected"
                    ));
                }
            }
        }
        if missing {
            digest.update(b"missing\0");
            continue;
        }

        let metadata = std::fs::symlink_metadata(&current)
            .map_err(|_| format!("strict-workflow touch path `{relative}` cannot be inspected"))?;
        if !metadata.is_file() {
            return Err(format!(
                "strict-workflow touch path `{relative}` is not a regular file"
            ));
        }
        if metadata.len() > crate::audit_bundle::BUNDLE_INPUT_MAX as u64 {
            return Err(format!(
                "strict-workflow touch file `{relative}` exceeds the 16 MiB limit"
            ));
        }
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .filter(|total| *total <= STRICT_EVIDENCE_TOTAL_BYTE_LIMIT)
            .ok_or_else(|| {
                "strict-workflow touch files exceed the 32 MiB total limit".to_string()
            })?;
        digest.update(b"file\0");
        digest.update(metadata.len().to_le_bytes());

        let mut file = std::fs::File::open(&current)
            .map_err(|_| format!("strict-workflow touch file `{relative}` cannot be read"))?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|_| format!("strict-workflow touch file `{relative}` cannot be read"))?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        digest.update([0]);
    }
    Ok(crate::hex::encode(&digest.finalize()))
}

fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn git_head(repo_root: &Path) -> Result<String, String> {
    let out = Command::new("git")
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
    let out = Command::new("git")
        .args(["diff", "--stat", &format!("{baseline_ref}..HEAD")])
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("git diff --stat: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git diff --stat: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
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
/// beyond store queries. Missing/unindexed symbols silently contribute 0 —
/// verify_spec already surfaces the existence check as an error.
pub fn compute_risk_report(spec: &ParsedSpec, store: &Store) -> RiskReport {
    let mut total_callers: u32 = 0;
    let mut worst: Option<WorstSymbol> = None;
    let mut central: Vec<CentralEntry> = Vec::new();

    for claim in &spec.pre_edit_snapshot {
        let n = store
            .callers_of(&claim.name, None, None)
            .map(|c| c.len() as u32)
            .unwrap_or(0);
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
    let (cycles, _cycles_truncated) = store.dependency_cycles(None, 2).unwrap_or_default();
    let mut files_in_cycles: Vec<String> = Vec::new();
    for cycle in cycles {
        for f in cycle {
            if mentioned.contains(f.as_str()) && !files_in_cycles.iter().any(|x| x == &f) {
                files_in_cycles.push(f);
            }
        }
    }

    RiskReport {
        snapshot_symbols: spec.pre_edit_snapshot.len() as u32,
        total_snapshot_callers: total_callers,
        worst_callers: worst,
        mentioned_files: spec.mentioned_files.len() as u32,
        files_in_cycles,
        top_central_mentioned: central,
    }
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
    // Iteration carry-forward: when --reset drops a prior state, snapshot its
    // iteration FIRST so the next pre-flight resumes the count. Else the budget
    // is trivially bypassed by repeated --reset.
    let preserved_iter: u32 = if opts.reset {
        let prior_iter = load_state(&state_path)
            .ok()
            .flatten()
            .map(|s| s.iteration)
            .unwrap_or(0);
        if let Err(e) = delete_state(&state_path) {
            eprintln!(
                "warning: --reset failed to delete `{}`: {e}",
                state_path.display()
            );
        }
        prior_iter
    } else {
        0
    };

    let existing = match load_state(&state_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "warning: state file `{}` unreadable ({e}); treating as absent",
                state_path.display()
            );
            None
        }
    };

    // Phase select. `--pre-only` / `--post-only` are explicit overrides;
    // otherwise state-file presence decides.
    if opts.post_only {
        let Some(state) = existing else {
            eprintln!(
                "error: --post-only requested but no state file at `{}`. Run pre-flight first.",
                state_path.display()
            );
            return Outcome::PreFailed;
        };
        return run_post(spec_path, repo_root, index_path, &state, &state_path);
    }

    if opts.pre_only || existing.is_none() {
        return run_pre(
            spec_path,
            repo_root,
            index_path,
            &state_path,
            opts,
            preserved_iter,
        );
    }

    if !opts.post_only {
        if let Some(state) = existing.as_ref() {
            if state.status == "learned" {
                println!(
                    "Task already complete — state is `{}`. Use --reset to start a new iteration or --post-only to re-audit.",
                    state_path.display()
                );
                return Outcome::PostHeld;
            }
            if state.status == "history_review_required" {
                let review_path = history_review_file_path(repo_root, spec_path);
                if history_review_complete(&review_path) {
                    let mut store = match Store::open(index_path) {
                        Ok(store) => store,
                        Err(error) => {
                            eprintln!(
                                "error: opening index `{}` for semantic history refresh: {error}",
                                index_path.display()
                            );
                            return Outcome::PostBroken;
                        }
                    };
                    if let Err(error) = refresh_durable_history(&mut store, repo_root) {
                        eprintln!(
                            "error: refreshing durable history before semantic completion: {error}"
                        );
                        return Outcome::PostBroken;
                    }
                    let mut completed = state.clone();
                    completed.status = "learned".into();
                    completed.next_step = Some("close".into());
                    completed.last_artifact = Some("history-review.md".into());
                    if let Err(error) = save_state(&state_path, &completed) {
                        eprintln!(
                            "error: persisting reviewed state `{}`: {error}",
                            state_path.display()
                        );
                        return Outcome::PostBroken;
                    }
                    println!("Task complete — semantic history review is resolved.");
                } else {
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
    preserved_iter: u32,
) -> Outcome {
    // Iteration budget — refuse pre-flight once the spec has been through
    // `max_iterations` cycles without landing Held. `preserved_iter` carries
    // forward any count from a state file --reset just dropped; +1 for THIS
    // attempt.
    let iteration = preserved_iter.saturating_add(1);
    if opts.max_iterations > 0 && iteration > opts.max_iterations && !opts.force_iteration {
        eprintln!(
            "❌ iteration budget exhausted: this spec has been through {} pre-flight cycle(s) without landing `contract held` (limit: {}).",
            iteration - 1,
            opts.max_iterations
        );
        eprintln!("   Stop and re-design the spec, or re-run with --force-iteration to override.");
        eprintln!(
            "   See `defect-taxonomy.md` in the mastermind-task-planning skill, kind `iteration_budget_exhausted`."
        );
        let _ = crate::lessons::append_iteration_budget_candidate(repo_root, spec_path, iteration);
        return Outcome::PreFailed;
    }

    let spec_body = match std::fs::read_to_string(spec_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: reading spec `{}`: {e}", spec_path.display());
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
    let mut store = Store::open(index_path).ok();
    match store.as_ref() {
        None if !opts.allow_no_index => {
            eprintln!(
                "❌ No index at `{}`. Run `mastermind index .` first, or pass --allow-no-index for docs-only specs.",
                index_path.display()
            );
            return Outcome::PreFailed;
        }
        Some(index) => match index.symbol_count() {
            Ok(0) if opts.allow_no_index => {
                // An empty SQLite shell carries no repository identity and
                // contributes no graph evidence. Treat it exactly like no index
                // for an explicitly docs-only task.
                store = None;
            }
            Ok(0) => {
                eprintln!(
                    "❌ Index at `{}` is empty (0 symbols). Run `mastermind index .` to populate, or pass --allow-no-index for docs-only specs.",
                    index_path.display()
                );
                return Outcome::PreFailed;
            }
            Ok(_) => {
                if let Err(error) = validate_index_root(index, repo_root) {
                    eprintln!("❌ Index/root mismatch: {error}");
                    return Outcome::PreFailed;
                }
            }
            Err(error) => {
                eprintln!("❌ Cannot query index `{}`: {error}", index_path.display());
                return Outcome::PreFailed;
            }
        },
        None => {}
    }
    if let Some(index) = store.as_mut() {
        let refresh = match Indexer::new(repo_root).index_all(index, false) {
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
            "❌ verify-spec failed — no state written. Fix errors above and re-run `mastermind run-task`."
        );
        return Outcome::PreFailed;
    }

    // 2. risk report (needs an open store for caller counts; without one,
    //    reporting zeros would mislead).
    match &store {
        Some(s) => print!("{}", render_risk_report(&compute_risk_report(&parsed, s))),
        None => println!(
            "\nRisk Report\n  (no index at `{}` — run `mastermind index .` for blast-radius numbers)",
            index_path.display()
        ),
    }

    // 3. capture HEAD + write state.
    let head = match git_head(repo_root) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("error: capturing git HEAD as baseline ref: {e}");
            return Outcome::PreFailed;
        }
    };
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
        started_at: timestamp_now(),
        iteration,
        allow_no_index: opts.allow_no_index,
    };
    if let Err(e) = save_state(state_path, &state) {
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
    let spec_body = match std::fs::read_to_string(spec_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: reading spec `{}`: {e}", spec_path.display());
            return Outcome::PostBroken;
        }
    };
    let parsed = spec::parse_str(&spec_path.display().to_string(), &spec_body);

    println!(
        "\n=== Post-flight: {} (baseline `{}`) ===",
        spec_path.display(),
        &state.baseline_ref[..state.baseline_ref.len().min(8)]
    );

    // Spec-drift warning — informative, not a block.
    let current_hash = hash_text(&spec_body);
    if current_hash != state.spec_hash {
        eprintln!(
            "warning: spec contents changed since pre-flight (hash was {}, now {}). \
             Audit findings may be inconsistent. Use --reset to start over.",
            &state.spec_hash[..state.spec_hash.len().min(8)],
            &current_hash[..current_hash.len().min(8)],
        );
    }

    let mut store = match Store::open(index_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: opening index `{}`: {e}", index_path.display());
            return Outcome::PostBroken;
        }
    };
    let populated = match store.symbol_count() {
        Ok(count) => count > 0,
        Err(error) => {
            eprintln!("error: querying index `{}`: {error}", index_path.display());
            return Outcome::PostBroken;
        }
    };
    if populated || !state.allow_no_index {
        if let Err(error) = validate_index_root(&store, repo_root) {
            eprintln!("error: index/root mismatch: {error}");
            return Outcome::PostBroken;
        }
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
    }

    let report_path = spec_path
        .parent()
        .unwrap_or(spec_path)
        .join("executor-report.md");
    let executor_report = match crate::executor_report::parse_file(&report_path) {
        Ok(report) => report,
        Err(error) => {
            eprintln!(
                "error: post-flight requires a canonical executor report at `{}`: {error}",
                report_path.display()
            );
            let mut failed = state.clone();
            failed.status = "held".into();
            failed.risk = Some("medium".into());
            failed.next_step = Some("planner_review".into());
            failed.blocking_reason = Some("executor report missing or invalid".into());
            failed.last_artifact = Some("spec.md".into());
            let _ = save_state(state_path, &failed);
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
    let audit_body = audit.render_text();
    print!("{audit_body}");
    let audit_path = spec_path.parent().unwrap_or(spec_path).join("audit.md");
    if let Err(error) = std::fs::write(&audit_path, &audit_body) {
        eprintln!(
            "error: failed to persist `{}`: {error}",
            audit_path.display()
        );
        let mut failed = state.clone();
        failed.status = "held".into();
        failed.risk = Some("high".into());
        failed.next_step = Some("planner_review".into());
        failed.blocking_reason = Some("failed to persist audit.md".into());
        failed.last_artifact = Some("executor-report.md".into());
        let _ = save_state(state_path, &failed);
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
        let held_snapshot_sha256 = match parsed.frontmatter.as_ref() {
            Some(frontmatter)
                if frontmatter.mode.as_deref() == Some("strict")
                    && !frontmatter.touches.is_empty() =>
            {
                let touches = frontmatter
                    .touches
                    .iter()
                    .map(|touch| touch.file.clone())
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
        if let Some(parent) = release_path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "error: failed to create release-note directory `{}`: {error}",
                    parent.display()
                );
                return Outcome::PostBroken;
            }
        }
        if let Err(error) = std::fs::write(&release_path, &body) {
            eprintln!(
                "error: failed to write release notes `{}`: {error}",
                release_path.display()
            );
            return Outcome::PostBroken;
        }
        println!("Release notes saved to {}", release_path.display());
        if let Err(error) =
            ensure_history_review(repo_root, spec_path, &release_path).map(|created| {
                if created {
                    println!(
                        "History review saved to {}",
                        history_review_file_path(repo_root, spec_path).display()
                    );
                }
            })
        {
            eprintln!("error: failed to create history review: {error}");
            return Outcome::PostBroken;
        }
        if let Err(error) = refresh_durable_history(&mut store, repo_root) {
            eprintln!("error: refreshing durable post-flight history: {error}");
            return Outcome::PostBroken;
        }
        let review_path = history_review_file_path(repo_root, spec_path);
        let mut complete = state.clone();
        if history_review_complete(&review_path) {
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
        if let Err(error) = save_state(state_path, &complete) {
            eprintln!(
                "error: persisting post-flight state `{}`: {error}",
                state_path.display()
            );
            return Outcome::PostBroken;
        }
    } else {
        if let Err(error) = refresh_durable_history(&mut store, repo_root) {
            eprintln!("error: refreshing durable failed-audit history: {error}");
            return Outcome::PostBroken;
        }
        let mut failed = state.clone();
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
        if let Err(error) = save_state(state_path, &failed) {
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
        format!("  next: review the comment delta vs `{baseline_ref}` — `mastermind-comment-audit`")
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
    let status = Command::new("claude")
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
            started_at: 123456,
            iteration: 0,
            allow_no_index: true,
        };
        save_state(&path, &state).unwrap();
        let loaded = load_state(&path).unwrap().expect("present");
        assert_eq!(loaded.spec_path, state.spec_path);
        assert_eq!(loaded.spec_hash, state.spec_hash);
        assert_eq!(loaded.baseline_ref, state.baseline_ref);
        assert_eq!(loaded.held_snapshot_sha256, state.held_snapshot_sha256);
        assert_eq!(loaded.started_at, state.started_at);
        assert!(loaded.allow_no_index);
        delete_state(&path).unwrap();
        assert!(load_state(&path).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
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
        assert_eq!(a.len(), 16); // {:016x}
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
        assert!(state.spec_hash.len() == 16);
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
        write_executor_report(&spec_path, &["src/lib.py"]);

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

        // A normal re-run of a completed task is idempotent. Explicit
        // --post-only remains available when the user really wants a re-audit.
        fs::remove_file(spec_path.parent().unwrap().join("executor-report.md")).unwrap();
        assert_eq!(
            run(&spec_path, &dir, &index_path, RunOpts::default()),
            Outcome::PostHeld
        );
        assert_eq!(
            load_state(&state_path).unwrap().unwrap().status,
            "history_review_required"
        );

        fs::write(
            dir.join("CONTEXT.md"),
            "semantic review captured the zebra routing invariant\n",
        )
        .unwrap();
        fs::write(
            &review_path,
            "- **Context:** updated\n- **Lesson:** not applicable\n- **Reason:** captured zebra routing in CONTEXT.md\n",
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
        assert_eq!(
            state.blocking_reason.as_deref(),
            Some("executor report missing or invalid")
        );

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
