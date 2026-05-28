//! `mastermind run-task` — deterministic shell around the probabilistic agents.
//!
//! Two-phase orchestrator that wraps the mastermind workflow in mechanical gates:
//!
//! 1. `verify_spec` — pre-flight checks (missing symbols, missing files,
//!    snapshot drift, FIND-block staleness, VERIFY-command resolvability).
//! 2. **Risk report** — blast radius totals, dependency-cycle membership of
//!    mentioned files, top centrality of snapshot symbols.
//! 3. Executor — hand-off message by default; with `--exec`, shells out to
//!    `claude -p` synchronously.
//! 4. `audit_spec` — post-flight drift detection (scope creep, snapshot drift,
//!    silent removals, missing planned tests).
//! 5. **Release notes draft** — H1 title + Goals + Tests Plan + `git diff --stat`
//!    of the baseline-to-HEAD range. Written to stdout AND
//!    `.mastermind/releases/<basename>.md` on the Held verdict.
//!
//! State (`{spec_hash, baseline_ref, started_at}`) persists between pre- and
//! post-flight under `.mastermind/run-state/<basename>.json`. Auto-resumes
//! based on file presence: no state → pre, state present → post. Cleared on
//! Held verdict; kept on Drift/Broken for retry after fixes.

use crate::audit_spec;
use crate::spec::{self, ParsedSpec};
use crate::store::Store;
use crate::verify_spec;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Persisted handshake between pre- and post-flight. Lives at
/// `<repo_root>/.mastermind/run-state/<spec-basename>.json`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RunState {
    /// Absolute path to the spec file pre-flight ran against.
    pub spec_path: String,
    /// Hash of the spec body at pre-flight. Re-checked at post-flight to
    /// warn if the spec was edited between phases.
    pub spec_hash: String,
    /// `git rev-parse HEAD` captured at pre-flight — the audit's `--since`.
    pub baseline_ref: String,
    /// Unix epoch seconds at pre-flight.
    pub started_at: u64,
}

/// What happened end-to-end. Mapped to exit codes by `main.rs`: any of the
/// `*Failed` / `*Broken` variants exits non-zero so CI / scripts can react.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Pre-flight passed, state written, hand-off message printed (no `--exec`).
    PreReady,
    /// `verify_spec` produced errors. State NOT written.
    PreFailed,
    /// Post-flight clean — release notes emitted, state cleared.
    PostHeld,
    /// Post-flight surfaced warnings only. State kept for retry.
    PostDrift,
    /// Post-flight surfaced contract-breaking findings. State kept.
    PostBroken,
    /// `--exec` shell-out to claude exited non-zero. State kept.
    ExecFailed,
}

/// Risk numbers surfaced after `verify_spec` in pre-flight — five-to-ten-line
/// "what's at stake" summary so the planner can spot a runaway scope before
/// inviting the executor in.
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

/// Flags from `main.rs`. Kept as a single struct so the dispatcher signature
/// stays stable as we add options (next likely: `--json`).
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOpts {
    /// Delete any existing state file before deciding which phase to run.
    pub reset: bool,
    /// Force pre-flight; never auto-resume into post-flight.
    pub pre_only: bool,
    /// Force post-flight; error if no state file exists.
    pub post_only: bool,
    /// Shell out to `claude -p` between phases. Default false — hand-off only.
    pub exec: bool,
    /// Skip the "index must exist and be non-empty" pre-check. Use for
    /// docs-only / spec-only specs that don't touch indexed source. Default
    /// false: missing-or-empty index hard-fails pre-flight, because mmcg's
    /// core claim is "grounded in the codegraph" — running gates without that
    /// grounding silently degrades them to mandatory-section + file-existence
    /// checks only.
    pub allow_no_index: bool,
}

/// State file path — `<repo_root>/.mastermind/run-state/<spec-basename>.json`.
pub fn state_file_path(repo_root: &Path, spec_path: &Path) -> PathBuf {
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

fn spec_basename(spec_path: &Path) -> String {
    spec_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("spec")
        .to_string()
}

/// Read + deserialize state. Returns `Ok(None)` when the file doesn't exist —
/// "no prior pre-flight" is the dominant non-error case.
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

/// Deterministic hash of the spec body. Uses `DefaultHasher`, which is stable
/// within a single Rust toolchain — fine for "did the spec change between
/// pre and post" detection on the same machine. False positives across a
/// toolchain upgrade are harmless (we only warn, not block).
fn hash_text(text: &str) -> String {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
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
/// other than store queries. Missing or unindexed symbols silently contribute
/// 0 — verify_spec already surfaces the existence check as an error.
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
        if worst.as_ref().map_or(true, |w| n > w.callers) {
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

    // Cycle membership of mentioned files. Walk all SCCs of size ≥ 2 in any
    // language; collect mentioned files that appear inside.
    let mentioned: HashSet<&str> = spec.mentioned_files.iter().map(String::as_str).collect();
    let cycles = store.dependency_cycles(None, 2).unwrap_or_default();
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

/// Top-level dispatcher — picks pre or post based on flags + state presence,
/// then calls the corresponding phase function. Pure I/O orchestration; the
/// computational pieces above are independently testable.
pub fn run(spec_path: &Path, repo_root: &Path, index_path: &Path, opts: RunOpts) -> Outcome {
    let state_path = state_file_path(repo_root, spec_path);
    if opts.reset {
        if let Err(e) = delete_state(&state_path) {
            eprintln!(
                "warning: --reset failed to delete `{}`: {e}",
                state_path.display()
            );
        }
    }

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

    // Decide phase. `--pre-only` / `--post-only` are explicit overrides;
    // otherwise the state file's presence chooses for us.
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
        return run_pre(spec_path, repo_root, index_path, &state_path, opts);
    }

    // Default mode + state present → resume into post.
    let state = existing.unwrap();
    run_post(spec_path, repo_root, index_path, &state, &state_path)
}

fn run_pre(
    spec_path: &Path,
    repo_root: &Path,
    index_path: &Path,
    state_path: &Path,
    opts: RunOpts,
) -> Outcome {
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
    // are only as strong as the codegraph they reason from; running them
    // against an absent or empty index would silently degrade verify-spec to
    // file-existence checks and turn audit-spec into git-diff-only. The
    // escape hatch `--allow-no-index` exists for docs-only specs.
    let store = Store::open(index_path).ok();
    if !opts.allow_no_index {
        match store.as_ref() {
            None => {
                eprintln!(
                    "❌ No index at `{}`. Run `mastermind index .` first, or pass --allow-no-index for docs-only specs.",
                    index_path.display()
                );
                return Outcome::PreFailed;
            }
            Some(s) => match s.symbol_count() {
                Ok(0) => {
                    eprintln!(
                        "❌ Index at `{}` is empty (0 symbols). Run `mastermind index .` to populate, or pass --allow-no-index for docs-only specs.",
                        index_path.display()
                    );
                    return Outcome::PreFailed;
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("warning: querying index symbol count: {e}");
                    // Tolerate transient SQL errors here — verify-spec below
                    // will fail more loudly if the store is actually broken.
                }
            },
        }
    }

    // 1. verify-spec (store optional — without index, only mandatory-section +
    //    missing-file checks contribute).
    let verify = verify_spec::run(&parsed, store.as_ref(), repo_root);
    print!("{}", verify.render_text());
    if verify.has_failures() {
        eprintln!(
            "❌ verify-spec failed — no state written. Fix errors above and re-run `mastermind run-task`."
        );
        return Outcome::PreFailed;
    }

    // 2. risk report (needs an open store to compute callers; otherwise we'd
    //    be misleading by reporting zeros).
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
    let state = RunState {
        spec_path: spec_path.display().to_string(),
        spec_hash: hash_text(&spec_body),
        baseline_ref: head.clone(),
        started_at: timestamp_now(),
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
        match run_executor(spec_path) {
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
        "\nNext: invoke the executor on this spec (e.g. via the mastermind-task-executor \
         subagent, or `claude -p` directly), then re-run:\n  mastermind run-task {}\nto audit + draft release notes.",
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

    let store = match Store::open(index_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: opening index `{}`: {e}", index_path.display());
            return Outcome::PostBroken;
        }
    };

    let audit = match audit_spec::run(&parsed, &store, repo_root, &state.baseline_ref) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: audit-spec: {e:?}");
            return Outcome::PostBroken;
        }
    };
    print!("{}", audit.render_text());

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

    if matches!(outcome, Outcome::PostHeld) {
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
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&release_path, &body) {
            Ok(()) => println!("Release notes saved to {}", release_path.display()),
            Err(e) => eprintln!(
                "warning: failed to write release notes `{}`: {e}",
                release_path.display()
            ),
        }
        if let Err(e) = delete_state(state_path) {
            eprintln!("warning: clearing state `{}`: {e}", state_path.display());
        }
    } else {
        println!(
            "\nVerdict is {verdict_label} — release notes deferred. State kept at `{}` for re-run after fixes.",
            state_path.display()
        );
    }

    outcome
}

/// Invoke `claude -p` synchronously on this spec. Streams stdout/stderr
/// through to the user's terminal. Returns Err on spawn failure or non-zero
/// exit so the caller can keep state for retry.
fn run_executor(spec_path: &Path) -> Result<(), String> {
    let prompt = format!(
        "Implement the mastermind spec at `{}` using the mastermind-task-executor workflow. \
         Apply edits phase-by-phase, run any VERIFY commands, mark each checklist item as you \
         complete it, and stop on the first failure. Ensure `mmcg` is available via your MCP \
         configuration so verify/audit gates have the live index.",
        spec_path.display(),
    );
    let status = Command::new("claude")
        .arg("-p")
        .arg(&prompt)
        .stdin(std::process::Stdio::null())
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
        // `.mastermind/` is already filtered by audit_spec, so no entry needed.
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

    #[test]
    fn state_file_roundtrips_through_json() {
        let dir = tmp("state_roundtrip");
        let path = dir.join("s.json");
        let state = RunState {
            spec_path: "specs/foo.md".into(),
            spec_hash: "deadbeefcafef00d".into(),
            baseline_ref: "abc1234".into(),
            started_at: 123456,
        };
        save_state(&path, &state).unwrap();
        let loaded = load_state(&path).unwrap().expect("present");
        assert_eq!(loaded.spec_path, state.spec_path);
        assert_eq!(loaded.spec_hash, state.spec_hash);
        assert_eq!(loaded.baseline_ref, state.baseline_ref);
        assert_eq!(loaded.started_at, state.started_at);
        delete_state(&path).unwrap();
        assert!(load_state(&path).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
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
        // H1 hidden behind an H2 doesn't count — would be inside-a-section.
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
            pre_only: true,       // don't even try to resume / exec
            allow_no_index: true, // fixture has no source — skip index requirement
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
                allow_no_index: true, // isolate failure to verify-spec, not index
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
    fn auto_resume_post_held_emits_release_notes_and_clears_state() {
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
        let mut store = Store::open(&index_path).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();
        drop(store);

        // Second run auto-resumes into post-flight.
        let outcome = run(&spec_path, &dir, &index_path, RunOpts::default());
        assert_eq!(outcome, Outcome::PostHeld);
        // Held → state cleared, release notes written.
        assert!(load_state(&state_path).unwrap().is_none());
        let release_path = release_file_path(&dir, &spec_path);
        assert!(release_path.exists(), "release notes file should exist");
        let body = fs::read_to_string(&release_path).unwrap();
        assert!(body.starts_with("# Clean add"));
        assert!(body.contains("Audit: ✅ Held"));
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

        // Executor added a file the spec didn't mention → scope creep / drift.
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
        // Mapped to PreFailed because the dispatcher uses that variant to
        // signal "couldn't get to post" — main.rs exits non-zero for it.
        assert_eq!(outcome, Outcome::PreFailed);
        fs::remove_dir_all(&dir).ok();
    }
}
