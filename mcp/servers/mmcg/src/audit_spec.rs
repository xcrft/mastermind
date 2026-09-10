//! `mastermind audit-spec` — post-execution mechanical audit.
//!
//! Handles the **deterministic** part of the audit contract: file-set
//! comparison, pre-edit snapshot drift, symbol-level diff. The LLM auditor
//! still does semantic judgment (does the test plan cover the new behavior? is
//! the doc update sufficient?); this catches the "claimed file X but git diff
//! doesn't show it" / "snapshot said 8 callers, now 5" bugs without needing
//! prompt discipline.
//!
//! Inputs: parsed spec + git ref to compare against (typically `main` or the
//! merge-base) + indexed `Store` for live symbol counts.
//!
//! The comparison runs `<ref>` → **working tree**, not `<ref>..HEAD`: the
//! documented handoff is pre-flight → executor → post-flight, with the commit
//! as a separate authorized step afterwards, so on the normal path HEAD is
//! still the baseline and the executor's work is uncommitted. Staged, unstaged,
//! and untracked changes all count as "changed" here.
//!
//! Outputs structured findings:
//! - `unexpected_file` — file changed in git but not mentioned in spec
//! - `missing_expected_file` — file mentioned in spec, not changed in git
//! - `snapshot_caller_drift` — pre-edit snapshot count != post-edit count
//! - `snapshot_signature_drift` — pre-edit signature != post-edit signature
//! - `snapshot_symbol_gone` — pre-edit symbol no longer in the index
//!
//! Phase B (test/doc/observability plan validation, non-breaking API check) is
//! deliberately out of scope for v1.

use crate::diff::{self, DiffError, SymbolDiff};
use crate::executor_claims;
pub use crate::executor_claims::{ClaimCheck, ClaimEvidence, ClaimStatus};
use crate::executor_report::{Claim, ExecutorReport};
use crate::spec::{ParsedSpec, SymbolClaim};
use crate::spec_removals;
use crate::spec_symbols::{self, Resolved, Scope, Unresolved};
use crate::store::Store;
use crate::test_scan::TestScanner;
use crate::verification::{pass_contradiction, PassContradiction};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Every check passed — no drift, no scope creep.
    Held,
    /// Finding(s) but no contract violation — usually scope creep (unexpected
    /// files) or minor signature drift. Planner reads + decides.
    Drift,
    /// Spec claimed something that didn't happen, OR pre-edit symbol now gone.
    Broken,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finding {
    /// File differs from the baseline (committed or not) but the spec didn't
    /// mention it.
    UnexpectedFile { file: String },
    /// File was mentioned in the spec but is identical to the baseline —
    /// nothing was written to it, staged or otherwise.
    MissingExpectedFile { file: String },
    /// Pre-edit snapshot count != current `mmcg_callers` count.
    SnapshotCallerDrift {
        symbol: String,
        spec_says: u32,
        index_says: u32,
    },
    /// Pre-edit snapshot signature != current signature of the same symbol.
    /// May be intentional (executor changed param shape) or a side-effect — LLM
    /// auditor decides; we flag the mechanical fact.
    SnapshotSignatureDrift {
        symbol: String,
        spec_says: String,
        index_says: Option<String>,
    },
    /// Symbol present in pre-edit snapshot has no current entry in mmcg —
    /// renamed, deleted, or moved out of the indexed tree.
    SnapshotSymbolGone { symbol: String },
    /// Identity is ambiguous, malformed, or unavailable from the index.
    SnapshotUnresolved {
        symbol: String,
        reason: String,
        matches: Option<usize>,
    },
    /// A removed baseline declaration has no matching acknowledgement.
    /// Legacy prose matching applies only to specs without frontmatter.
    RemovedSymbolNotAcknowledged { symbol: String, file: String },
    /// A structured removal acknowledgement cannot identify its baseline target.
    RemovalAcknowledgementUnresolved {
        symbol: String,
        file: Option<String>,
        reason: String,
        matches: Option<usize>,
    },
    /// Tests Plan names a test (`test_foo`, `it('bar')`, etc.) absent from
    /// `symbol_diff.added` — executor skipped it or the planned name was wrong.
    PlannedTestNotAdded { test: String },
    /// Executor claimed they added symbol X but it has no definition in the live
    /// index — the add didn't happen or indexing missed it.
    ClaimedSymbolMissing {
        symbol: String,
        file: Option<String>,
    },
    /// The selected current declaration was not introduced by this diff.
    ClaimedSymbolNotAdded {
        symbol: String,
        file: Option<String>,
    },
    /// A claim could not be checked with complete, unambiguous evidence.
    ExecutorClaimUnresolved {
        claim_index: Option<usize>,
        reason: String,
        matches: Option<usize>,
    },
    /// Executor completion, task identity or checked-report binding is invalid.
    ExecutorReportRejected { reason: String },
    /// A declared command has missing, unsuccessful or conflicting report rows.
    VerificationRequirementUnmet { cmd: String, reason: String },
    /// The integration claim has no matching target definition in its scope.
    HallucinatedSymbol {
        from_symbol: String,
        to_symbol: String,
    },
    /// Executor claimed X calls Y but no call edge from X to Y exists in the
    /// index — the integration claim is false.
    MissingCallEdge {
        from_symbol: String,
        to_symbol: String,
    },
    /// A static scan found no conventional test files for a recognized test
    /// command. This is an advisory warning, not evidence of execution.
    VacuousTestClaim { cmd: String, reason: String },
    /// Executor claimed they added symbol X with a signature that doesn't match
    /// the stored one — wrote a different signature, or copy-pasted the claim
    /// from a draft spec.
    ClaimedSignatureMismatch {
        symbol: String,
        file: Option<String>,
        claimed: String,
        actual: Option<String>,
    },
    /// Executor attached `observed: { exit_code: N }` (N != 0) but claimed the
    /// command passed — the exit code contradicts the claim; the run likely
    /// failed or was skipped.
    ObservedExitCodeNonZero { cmd: String, exit_code: i32 },
    /// Executor claimed a recognized test run passed but reported zero tests.
    /// A missing exit code does not erase this contradiction in the report.
    ObservedZeroTests { cmd: String },
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub spec: String,
    pub git_ref: String,
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
    /// Raw symbol-level diff of baseline → working tree — pasted in so the LLM
    /// auditor has full context for semantic judgment.
    pub symbol_diff: Option<SymbolDiff>,
    /// None means executor claims were not evaluated; Some([]) is an evaluated empty list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim_checks: Option<Vec<ClaimCheck>>,
    /// The complete report whose completion, task identity and claims were checked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor_report: Option<ExecutorReport>,
}

impl Report {
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        let marker = match self.verdict {
            Verdict::Held => "✅",
            Verdict::Drift => "⚠️",
            Verdict::Broken => "❌",
        };
        out.push_str(&format!(
            "{marker} {:?} — {} (vs git ref `{}`)\n  findings: {}\n\n",
            self.verdict,
            self.spec,
            self.git_ref,
            self.findings.len(),
        ));
        for f in &self.findings {
            let icon = match f {
                Finding::UnexpectedFile { .. }
                | Finding::MissingExpectedFile { .. }
                | Finding::SnapshotCallerDrift { .. }
                | Finding::SnapshotSignatureDrift { .. }
                | Finding::PlannedTestNotAdded { .. }
                | Finding::VacuousTestClaim { .. } => "⚠️ ",
                Finding::SnapshotSymbolGone { .. }
                | Finding::SnapshotUnresolved { .. }
                | Finding::RemovedSymbolNotAcknowledged { .. }
                | Finding::RemovalAcknowledgementUnresolved { .. }
                | Finding::ClaimedSymbolMissing { .. }
                | Finding::ClaimedSymbolNotAdded { .. }
                | Finding::ExecutorClaimUnresolved { .. }
                | Finding::ExecutorReportRejected { .. }
                | Finding::VerificationRequirementUnmet { .. }
                | Finding::HallucinatedSymbol { .. }
                | Finding::MissingCallEdge { .. }
                | Finding::ClaimedSignatureMismatch { .. }
                | Finding::ObservedExitCodeNonZero { .. }
                | Finding::ObservedZeroTests { .. } => "❌",
            };
            out.push_str(&format!("  {icon} {}\n", render_finding(f)));
        }
        if let Some(d) = &self.symbol_diff {
            out.push_str(&format!(
                "\n  (symbol diff: +{} -{} ~{})\n",
                d.added.len(),
                d.removed.len(),
                d.signature_changed.len(),
            ));
        }
        out
    }

    pub fn has_failures(&self) -> bool {
        matches!(self.verdict, Verdict::Broken)
    }
}

pub fn render_finding_text(f: &Finding) -> String {
    render_finding(f)
}

fn render_finding(f: &Finding) -> String {
    match f {
        Finding::UnexpectedFile { file } => {
            format!("unexpected_file: `{file}` changed but not in spec → scope creep")
        }
        Finding::MissingExpectedFile { file } => {
            format!("missing_expected_file: spec named `{file}` but diff shows no change")
        }
        Finding::SnapshotCallerDrift {
            symbol,
            spec_says,
            index_says,
        } => {
            format!("snapshot_caller_drift: `{symbol}` pre-edit said {spec_says} callers, post-edit {index_says}")
        }
        Finding::SnapshotSignatureDrift {
            symbol,
            spec_says,
            index_says,
        } => {
            let live = index_says.as_deref().unwrap_or("<no signature stored>");
            format!("snapshot_signature_drift: `{symbol}` pre-edit signature was `{spec_says}`, post-edit `{live}` — confirm change was intentional")
        }
        Finding::SnapshotSymbolGone { symbol } => {
            format!("snapshot_symbol_gone: `{symbol}` was in pre-edit snapshot, gone from index")
        }
        Finding::SnapshotUnresolved {
            symbol,
            reason,
            matches,
        } => {
            let count = matches
                .map(|count| format!(" ({count} matching declarations)"))
                .unwrap_or_default();
            format!("snapshot_unresolved: {symbol}: {reason}{count} — verify the index and declaration scope")
        }
        Finding::RemovedSymbolNotAcknowledged { symbol, file } => {
            format!("removed_symbol_not_acknowledged: `{symbol}` deleted from `{file}` without a matching removal acknowledgement")
        }
        Finding::RemovalAcknowledgementUnresolved {
            symbol,
            file,
            reason,
            matches,
        } => {
            let file = file
                .as_deref()
                .map(|file| format!(" in {file}"))
                .unwrap_or_default();
            let count = matches
                .map(|count| format!(" ({count} matching declarations)"))
                .unwrap_or_default();
            format!("removal_acknowledgement_unresolved: {symbol}{file}: {reason}{count}")
        }
        Finding::PlannedTestNotAdded { test } => {
            format!("planned_test_not_added: Tests Plan named `{test}` but the diff doesn't show a new function with that name")
        }
        Finding::ClaimedSymbolMissing { symbol, file } => {
            let loc = file
                .as_deref()
                .map(|f| format!(" in `{f}`"))
                .unwrap_or_default();
            format!("claimed_symbol_missing: executor claimed they added `{symbol}`{loc} but it has no definition in the index")
        }
        Finding::ClaimedSymbolNotAdded { symbol, file } => {
            let location = file
                .as_deref()
                .map(|file| format!(" in {file}"))
                .unwrap_or_default();
            format!("claimed_symbol_not_added: {symbol}{location} was not introduced by this diff")
        }
        Finding::ExecutorClaimUnresolved {
            claim_index,
            reason,
            matches,
        } => {
            let claim = claim_index
                .map(|index| format!("claim {}", index + 1))
                .unwrap_or_else(|| "executor report".into());
            let count = matches
                .map(|count| format!(" ({count} matching declarations)"))
                .unwrap_or_default();
            format!("executor_claim_unresolved: {claim}: {reason}{count}")
        }
        Finding::ExecutorReportRejected { reason } => {
            format!("executor_report_rejected: {reason}")
        }
        Finding::VerificationRequirementUnmet { cmd, reason } => {
            format!("verification_requirement_unmet: `{cmd}`: {reason}")
        }
        Finding::HallucinatedSymbol {
            from_symbol,
            to_symbol,
        } => {
            format!("hallucinated_symbol: executor claimed `{from_symbol}` calls `{to_symbol}` but no target definition matches the claimed scope in the index")
        }
        Finding::MissingCallEdge {
            from_symbol,
            to_symbol,
        } => {
            format!("missing_call_edge: no compatible call candidate connects the selected `{from_symbol}` and `{to_symbol}` declarations in the index")
        }
        Finding::VacuousTestClaim { cmd, reason } => {
            format!("vacuous_test_claim: `{cmd}` claimed passed but {reason}")
        }
        Finding::ClaimedSignatureMismatch {
            symbol,
            file,
            claimed,
            actual,
        } => {
            let loc = file
                .as_deref()
                .map(|f| format!(" in `{f}`"))
                .unwrap_or_default();
            let got = actual.as_deref().unwrap_or("<no signature stored>");
            format!(
                "claimed_signature_mismatch: `{symbol}`{loc} — executor claimed `{claimed}`, index has `{got}`"
            )
        }
        Finding::ObservedExitCodeNonZero { cmd, exit_code } => {
            format!(
                "observed_exit_code_nonzero: `{cmd}` claimed passed but observed exit_code={exit_code}"
            )
        }
        Finding::ObservedZeroTests { cmd } => {
            format!(
                "observed_zero_tests: `{cmd}` claimed passed but observed tests_run=0 — vacuous pass"
            )
        }
    }
}

/// Run all Phase A checks against a parsed spec.
///
/// `git_ref` must resolve via `git rev-parse` in `repo_root` (baseline for
/// symbols-changed-since). `store` is the live index at HEAD.
pub fn run(
    spec: &ParsedSpec,
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
) -> Result<Report, DiffError> {
    run_internal(spec, store, repo_root, git_ref, false, None).map(|(report, _)| report)
}

fn run_internal(
    spec: &ParsedSpec,
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
    verify_postflight: bool,
    executor_report: Option<&ExecutorReport>,
) -> Result<(Report, Option<crate::verify_spec::Report>), DiffError> {
    // Worktree-scoped, not `<ref>..HEAD` — post-flight audits work that has not
    // been committed yet. See the module header.
    let deadline = Instant::now() + diff::git_timeout();
    let index_version = store
        .data_version()
        .map_err(|_| DiffError::GitFailed("index_version_unavailable".into()))?;
    let has_claims = executor_report.is_some_and(|report| !report.claims.is_empty());
    if (has_claims
        || spec
            .frontmatter
            .as_ref()
            .is_some_and(|frontmatter| !frontmatter.breaking_changes.removed_symbols.is_empty()))
        && !store
            .extractor_contract_current()
            .map_err(|_| DiffError::GitFailed("index_stale".into()))?
    {
        return Err(DiffError::GitFailed("index_stale".into()));
    }
    let (worktree, declarations) = diff::symbols_changed_in_worktree_with_declarations(
        store,
        repo_root,
        git_ref,
        Some(deadline),
        None,
        executor_report.is_some_and(|report| {
            report
                .claims
                .iter()
                .any(|claim| matches!(claim, Claim::FunctionAdded { .. }))
        }),
    )
    .map_err(|error| diff::worktree_scope_error(git_ref, error))?;
    let symbol_diff = &worktree.diff;
    let mut findings: Vec<Finding> = Vec::new();
    let removal_plan = spec_removals::Plan::build(
        spec,
        repo_root,
        &worktree.baseline_oid,
        &declarations.removed,
        deadline,
        !worktree.files_truncated
            && worktree.skipped_non_utf8_paths == 0
            && symbol_diff.errors.is_empty(),
    );
    if let Some(plan) = &removal_plan {
        for error in &plan.errors {
            findings.push(Finding::RemovalAcknowledgementUnresolved {
                symbol: error.name.clone(),
                file: error.file.clone(),
                reason: error.error.reason.to_string(),
                matches: error.error.matches,
            });
        }
    }

    // 1. File scope check — symmetric difference of declared files vs the files
    //    that differ from the baseline on disk.
    //
    //    Frontmatter authoritative when present: `touches[].file` +
    //    `expected_docs[]` are the declared set. Heuristic mentioned_files
    //    (backticked path tokens) is too noisy — picks up prose like
    //    ``do not touch `README.md` `` and flags files the planner never claimed.
    //
    //    Filter `.mastermind/` from the diff side: local working state (index
    //    DB, specs), universally gitignored in real projects; CI fixtures commit
    //    it for test reasons.
    let spec_files_owned: Vec<String> = match spec.frontmatter.as_ref() {
        Some(fm) if fm.has_file_scope() => {
            let mut out: Vec<String> =
                Vec::with_capacity(fm.touches.len() + fm.expected_docs.len());
            for t in &fm.touches {
                out.push(t.file.clone());
            }
            for d in &fm.expected_docs {
                out.push(d.clone());
            }
            out
        }
        _ => spec.mentioned_files.clone(),
    };
    let spec_files_owned: Vec<_> = spec_files_owned
        .into_iter()
        .map(|file| spec_symbols::normalize_file(&file).unwrap_or(file))
        .collect();
    let spec_files: HashSet<&str> = spec_files_owned.iter().map(String::as_str).collect();
    let diff_files: HashSet<&str> = symbol_diff
        .files_in_diff
        .iter()
        .filter(|f| !f.starts_with(".mastermind/") && !f.starts_with(".mastermind\\"))
        .map(String::as_str)
        .collect();
    for f in &diff_files {
        if !spec_files.contains(*f) {
            findings.push(Finding::UnexpectedFile {
                file: (*f).to_string(),
            });
        }
    }
    for f in &spec_files {
        if !diff_files.contains(*f) {
            findings.push(Finding::MissingExpectedFile {
                file: (*f).to_string(),
            });
        }
    }

    // 2. Pre-edit snapshot drift — for every claim with a count, compare
    //    against live callers_of.
    for claim in &spec.pre_edit_snapshot {
        check_snapshot_claim(
            claim,
            &spec_symbols::snapshot_scopes(spec, claim),
            store,
            removal_plan.as_ref(),
            &mut findings,
        );
    }

    // Explicit frontmatter signatures/counts are snapshots too. Bare touches
    // remain pre-edit existence/scope declarations, which can name removals.
    if let Some(frontmatter) = &spec.frontmatter {
        for touch in &frontmatter.touches {
            for symbol in &touch.symbols {
                if symbol.signature().is_none() && symbol.callers().is_none() {
                    continue;
                }
                let scope = Scope {
                    name: symbol.name(),
                    file: Some(symbol.file().unwrap_or(&touch.file)),
                    language: symbol.language().or(touch.language.as_deref()),
                };
                let claim = SymbolClaim {
                    name: symbol.name().to_string(),
                    callers: symbol.callers(),
                    signature: symbol.signature().map(str::to_string),
                    raw: String::new(),
                };
                check_snapshot_claim(
                    &claim,
                    &[scope],
                    store,
                    removal_plan.as_ref(),
                    &mut findings,
                );
            }
        }
    }

    // Structured acknowledgements refer to exact baseline parser ordinals.
    // Keep the legacy prose heuristic only for specs without frontmatter.
    let spec_body_lower = spec
        .sections
        .values()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    let scoped_remaining = match removal_plan.as_ref().map(|plan| plan.unacknowledged()) {
        Some(Ok(remaining)) => Some(remaining),
        Some(Err(error)) => {
            findings.push(Finding::RemovalAcknowledgementUnresolved {
                symbol: "<baseline removals>".into(),
                file: None,
                reason: error.reason.to_string(),
                matches: error.matches,
            });
            None
        }
        None => None,
    };
    if let Some(remaining) = scoped_remaining {
        for (file, symbol) in remaining {
            findings.push(Finding::RemovedSymbolNotAcknowledged { symbol, file });
        }
    } else {
        for removed in &symbol_diff.removed {
            if removed.kind != "module"
                && (spec.frontmatter.is_some()
                    || !spec_body_lower.contains(&removed.name.to_lowercase()))
            {
                findings.push(Finding::RemovedSymbolNotAcknowledged {
                    symbol: removed.name.clone(),
                    file: removed.file.clone(),
                });
            }
        }
    }

    // 4. Test plan validation — extract test-function-name-shaped tokens from
    //    Tests Plan, cross-reference against names in symbol_diff.added.
    if let Some(tests_body) = crate::spec::section_body(spec, "Tests Plan") {
        let planned = extract_planned_test_names(tests_body);
        let added_names: HashSet<&str> = symbol_diff
            .added
            .iter()
            .filter(|s| matches!(s.kind.as_str(), "function" | "method"))
            .map(|s| s.name.as_str())
            .collect();
        for test in planned {
            if !added_names.contains(test.as_str()) {
                findings.push(Finding::PlannedTestNotAdded { test });
            }
        }
    }

    let verification = verify_postflight.then(|| {
        let deleted_files = worktree
            .files
            .iter()
            .filter(|file| file.status == "deleted")
            .map(|file| file.path.as_str())
            .collect();
        crate::verify_spec::run_with_removals(
            spec,
            Some(store),
            repo_root,
            removal_plan.as_ref(),
            &deleted_files,
        )
    });
    let claim_checks = executor_report.map(|report| {
        check_executor_completion(report, spec, repo_root, deadline, &mut findings);
        let checks = executor_claims::evaluate(
            report,
            &executor_claims::Context {
                store,
                changes: &declarations,
                repo_root,
                baseline_oid: &worktree.baseline_oid,
                complete: !worktree.files_truncated
                    && worktree.skipped_non_utf8_paths == 0
                    && symbol_diff.errors.is_empty(),
                deadline,
            },
        );
        findings.extend(checks.iter().filter_map(|check| check.finding.clone()));
        check_vacuous_tests(report, repo_root, store, deadline, &mut findings);
        checks
    });
    if removal_plan.is_some() || has_claims {
        diff::validate_working_tree_snapshot_controlled(
            repo_root,
            &worktree.baseline_oid,
            &worktree.head_oid,
            &worktree.files,
            &worktree.snapshot_token,
            Some(deadline),
            None,
        )
        .map_err(|error| diff::worktree_scope_error(git_ref, error))?;
        if store
            .data_version()
            .map_err(|_| DiffError::GitFailed("index_version_unavailable".into()))?
            != index_version
        {
            return Err(DiffError::GitFailed("index_changed".into()));
        }
    }
    let verdict = compute_verdict(&findings);
    Ok((
        Report {
            spec: spec.path.clone(),
            git_ref: git_ref.to_string(),
            verdict,
            findings,
            symbol_diff: Some(worktree.diff),
            claim_checks,
            executor_report: executor_report.cloned(),
        },
        verification,
    ))
}

/// CI checks completed work. Only baseline-proven acknowledged removals are
/// exempt from the preflight verifier's current-symbol/file requirements.
pub fn run_ci_with_report(
    spec: &ParsedSpec,
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
    executor_report: Option<&crate::executor_report::ExecutorReport>,
) -> Result<(crate::verify_spec::Report, Report), DiffError> {
    let (report, verification) =
        run_internal(spec, store, repo_root, git_ref, true, executor_report)?;
    Ok((verification.expect("CI verification requested"), report))
}

/// Run Phase A checks + executor-report mechanical checks.
///
/// `executor_report == None` is equivalent to `run()`. When present, adds:
///  - Per-claim declaration-addition and compatible-call-candidate evidence
///  - Unresolved outcomes for stale, ambiguous or unavailable source evidence
///  - Canonical report completion and repository-contained task identity
///  - Reported outcomes for every explicitly declared verification command
///  - Bounded advisory scans of conventional test files for claimed passes
pub fn run_with_report(
    spec: &ParsedSpec,
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
    executor_report: Option<&crate::executor_report::ExecutorReport>,
) -> Result<Report, DiffError> {
    run_internal(spec, store, repo_root, git_ref, false, executor_report).map(|(report, _)| report)
}

fn norm_path(p: &str) -> String {
    p.replace('\\', "/").trim_start_matches("./").to_string()
}

fn check_executor_completion(
    report: &ExecutorReport,
    spec: &ParsedSpec,
    root: &Path,
    deadline: Instant,
    findings: &mut Vec<Finding>,
) {
    let Some(metadata) = &report.canonical else {
        return;
    };
    check_declared_verifications(report, spec, findings);
    if let Some(reason) = report.completion_rejection() {
        findings.push(Finding::ExecutorReportRejected {
            reason: reason.into(),
        });
    }
    let binding = root.canonicalize().ok().and_then(|root| {
        let normalize = |text: &str| {
            let path = Path::new(text);
            let path = path.strip_prefix(".").unwrap_or(path);
            relative_binding_path(&root, path).ok()
        };
        let declared = normalize(&metadata.spec)?;
        let checked = normalize(&spec.path)?;
        Some((root, declared, checked))
    });
    let reason = match binding {
        Some((_, declared, checked)) if declared != checked => Some("task_mismatch"),
        Some(_) if metadata.schema_version != 1 => Some("schema_version_unsupported"),
        Some((root, _, checked)) => {
            let limit = crate::audit_bundle::BUNDLE_INPUT_MAX as u64;
            crate::bounded_fs::read_repository_file(
                &root,
                Path::new(&checked),
                limit,
                limit,
                crate::bounded_fs::ReadControl {
                    deadline: Some(deadline),
                    interrupted: None,
                },
            )
            .err()
            .map(|_| "task_source_unavailable")
        }
        None => Some("task_path_invalid"),
    };
    if let Some(reason) = reason {
        findings.push(Finding::ExecutorReportRejected {
            reason: reason.into(),
        });
    }
}

fn check_declared_verifications(
    report: &ExecutorReport,
    spec: &ParsedSpec,
    findings: &mut Vec<Finding>,
) {
    let mut outcomes = BTreeMap::new();
    for row in &report.verify {
        let (passed, unsuccessful) = outcomes.entry(row.cmd.trim()).or_insert((0usize, 0usize));
        if row.claimed.as_deref() == Some("passed") && pass_contradiction(row).is_none() {
            *passed += 1;
        } else {
            *unsuccessful += 1;
        }
    }
    for cmd in spec.declared_verify_commands() {
        let reason = match outcomes.get(cmd) {
            None => "missing_result",
            Some((_, 0)) => continue,
            Some((0, _)) => "not_passed",
            Some(_) => "conflicting_results",
        };
        findings.push(Finding::VerificationRequirementUnmet {
            cmd: cmd.into(),
            reason: reason.into(),
        });
    }
}

fn claim_label(index: usize, claim: &Claim) -> String {
    let scoped = |name: &str, file: Option<&str>| {
        file.map(|file| format!("{name}@{}", norm_path(file)))
            .unwrap_or_else(|| name.into())
    };
    let label = match claim {
        Claim::FunctionAdded { symbol, file, .. } => {
            format!("function_added:{}", scoped(symbol, file.as_deref()))
        }
        Claim::Integration {
            from,
            from_file,
            to,
            to_file,
            ..
        } => format!(
            "integration_candidate:{}→{}",
            scoped(from, from_file.as_deref()),
            scoped(to, to_file.as_deref()),
        ),
    };
    format!("claim[{}] {label}", index + 1)
}

fn check_vacuous_tests(
    er: &crate::executor_report::ExecutorReport,
    repo_root: &Path,
    store: &Store,
    deadline: Instant,
    findings: &mut Vec<Finding>,
) {
    let interrupted = || store.work_interrupted();
    let mut scanner = TestScanner::new(
        repo_root,
        crate::bounded_fs::ReadControl {
            deadline: Some(deadline),
            interrupted: Some(&interrupted),
        },
    );
    for v in &er.verify {
        let claimed_passed = v
            .claimed
            .as_deref()
            .is_some_and(|c| c.eq_ignore_ascii_case("passed") || c.eq_ignore_ascii_case("pass"));
        if !claimed_passed {
            continue;
        }
        if let Some(contradiction) = pass_contradiction(v) {
            findings.push(match contradiction {
                PassContradiction::NonZeroExit(code) => Finding::ObservedExitCodeNonZero {
                    cmd: v.cmd.clone(),
                    exit_code: code,
                },
                PassContradiction::ZeroTests => Finding::ObservedZeroTests { cmd: v.cmd.clone() },
            });
            continue;
        }
        if let Some(obs) = &v.observed {
            // A positive self-reported count takes precedence over the
            // advisory file scan; it does not authenticate execution.
            if matches!(obs.tests_run, Some(n) if n > 0) {
                continue;
            }
        }
        if let Some(reason) = scanner.absence_reason(&v.cmd) {
            findings.push(Finding::VacuousTestClaim {
                cmd: v.cmd.clone(),
                reason,
            });
        }
    }
}

// ----- evidence bundle ------------------------------------------------------

/// Portable proof artifact written by `audit-spec --bundle`.
///
/// `discrepancies` and `snapshot_drift` keep their pre-v2 names because the
/// signed manifest carries them under those keys.
#[derive(Debug, Serialize)]
pub struct Bundle {
    pub verdict: String,
    pub spec: String,
    /// Git ref the audit diffed against (typically `main` or a commit sha).
    pub baseline: String,
    /// Best-effort HEAD sha at the time the bundle was produced.
    pub head: String,
    /// Files the spec declared it would touch (authoritative under frontmatter,
    /// heuristic otherwise).
    pub spec_files: Vec<String>,
    /// Files changed between `baseline` and HEAD per `git diff`.
    pub changed_files: Vec<String>,
    /// Executor claims that passed all mechanical checks.
    pub verified_claims: Vec<String>,
    /// Executor claims that failed at least one mechanical check.
    pub failed_claims: Vec<String>,
    /// Query entry points for inspecting verified claims, not an execution trace.
    pub mmcg_queries: Vec<String>,
    /// Verify commands extracted from the executor report.
    pub commands: Vec<String>,
    /// One-line verdict summary suitable for a PR comment title.
    pub human_summary: String,
    /// All findings (superset of `snapshot_drift`).
    pub discrepancies: Vec<Finding>,
    /// Snapshot-drift findings only.
    pub snapshot_drift: Vec<Finding>,
    /// Legacy alias for `changed_files`.
    pub files_diff: Vec<String>,
    /// Legacy alias for `baseline`.
    pub git_ref: String,
    pub executor_report_path: Option<String>,
    #[serde(skip)]
    checked_executor_report: Option<ExecutorReport>,
}

impl Bundle {
    pub fn from_report(report: &Report, executor_report_path: Option<&str>) -> Self {
        Self::from_report_full(report, None, None, executor_report_path, None)
    }

    pub fn from_report_full(
        report: &Report,
        executor_report: Option<&crate::executor_report::ExecutorReport>,
        spec: Option<&crate::spec::ParsedSpec>,
        executor_report_path: Option<&str>,
        root: Option<&Path>,
    ) -> Self {
        let changed_files = report
            .symbol_diff
            .as_ref()
            .map(|d| d.files_in_diff.clone())
            .unwrap_or_default();

        let snapshot_drift: Vec<Finding> = report
            .findings
            .iter()
            .filter(|f| {
                matches!(
                    f,
                    Finding::SnapshotCallerDrift { .. }
                        | Finding::SnapshotSignatureDrift { .. }
                        | Finding::SnapshotSymbolGone { .. }
                        | Finding::SnapshotUnresolved { .. }
                )
            })
            .cloned()
            .collect();

        let spec_files: Vec<String> = spec
            .map(|s| {
                s.frontmatter
                    .as_ref()
                    .filter(|fm| fm.has_file_scope())
                    .map(|fm| {
                        fm.touches
                            .iter()
                            .map(|t| t.file.clone())
                            .chain(fm.expected_docs.iter().cloned())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_else(|| s.mentioned_files.clone())
            })
            .unwrap_or_default()
            .into_iter()
            .map(|file| spec_symbols::normalize_file(&file).unwrap_or(file))
            .collect();

        let mut mmcg_queries: Vec<String> = Vec::new();
        let mut verified_claims: Vec<String> = Vec::new();
        let mut failed_claims: Vec<String> = Vec::new();

        let mut discrepancies = report.findings.clone();
        let report_binding_failed = executor_report
            .is_some_and(|executor| report.executor_report.as_ref() != Some(executor));
        if report_binding_failed {
            discrepancies.push(Finding::ExecutorReportRejected {
                reason: if report.executor_report.is_none() {
                    "report_not_evaluated"
                } else {
                    "report_checks_mismatch"
                }
                .into(),
            });
        }
        let executor_report = executor_report.or(report.executor_report.as_ref());
        let claims: Vec<_> = match executor_report {
            Some(executor) => executor.claims.iter().collect(),
            None => report
                .claim_checks
                .iter()
                .flatten()
                .map(|check| &check.claim)
                .collect(),
        };
        let checks_match = report.claim_checks.as_ref().is_some_and(|checks| {
            checks.len() == claims.len()
                && checks
                    .iter()
                    .zip(&claims)
                    .enumerate()
                    .all(|(index, (check, claim))| {
                        check.claim_index == index && check.claim == **claim
                    })
        });
        let binding_failed =
            !checks_match && (executor_report.is_some() || report.claim_checks.is_some());
        if binding_failed {
            discrepancies.push(Finding::ExecutorClaimUnresolved {
                claim_index: None,
                reason: if report.claim_checks.is_none() {
                    "claims_not_evaluated"
                } else {
                    "claim_checks_mismatch"
                }
                .into(),
                matches: None,
            });
        }
        for (index, claim) in claims.iter().enumerate() {
            let label = claim_label(index, claim);
            let checked =
                checks_match.then(|| &report.claim_checks.as_ref().expect("checked claims")[index]);
            let verified = !report_binding_failed
                && checked.is_some_and(|check| {
                    check.status == ClaimStatus::Verified
                        && check.evidence.is_some()
                        && check.finding.is_none()
                });
            if verified {
                verified_claims.push(label);
            } else {
                failed_claims.push(label);
            }
            let queries = match claim {
                Claim::FunctionAdded { symbol, .. } => vec![format!("mmcg_search {symbol}")],
                Claim::Integration { from, to, .. } => vec![
                    format!("mmcg_search {from}"),
                    format!("mmcg_search {to}"),
                    format!("mmcg_callees {from}"),
                ],
            };
            if verified {
                for query in queries {
                    if !mmcg_queries.contains(&query) {
                        mmcg_queries.push(query);
                    }
                }
            }
        }
        let verdict = if binding_failed || report_binding_failed || !failed_claims.is_empty() {
            Verdict::Broken
        } else {
            report.verdict
        };

        let commands: Vec<String> = report
            .executor_report
            .as_ref()
            .map(|er| er.verify.iter().map(|v| v.cmd.clone()).collect())
            .unwrap_or_default();

        let head = resolve_head_sha(root);

        let human_summary =
            build_human_summary(verdict, &discrepancies, &failed_claims, &verified_claims);

        Self {
            verdict: format!("{verdict:?}").to_lowercase(),
            spec: report.spec.clone(),
            baseline: report.git_ref.clone(),
            head,
            spec_files,
            files_diff: changed_files.clone(),
            changed_files,
            verified_claims,
            failed_claims,
            mmcg_queries,
            commands,
            human_summary,
            discrepancies,
            snapshot_drift,
            git_ref: report.git_ref.clone(),
            executor_report_path: executor_report_path.map(str::to_string),
            checked_executor_report: report.executor_report.clone(),
        }
    }

    pub fn into_manifest(
        self,
        root: &Path,
    ) -> Result<crate::audit_bundle::Manifest, Box<dyn std::error::Error>> {
        use crate::audit_bundle::{
            normalize_repository_identity, sha256_hex, DiffBinding, InputBinding, Manifest,
            RepositoryBinding, ToolBinding, BUNDLE_INPUT_MAX,
        };

        let root = root
            .canonicalize()
            .map_err(|e| format!("canonicalize audit root {}: {e}", root.display()))?;
        let baseline_oid = resolve_commit(&root, &self.baseline)?;
        let head_oid = resolve_commit(&root, "HEAD")?;
        if baseline_oid == head_oid {
            return Err("audit baseline and HEAD must differ".into());
        }

        let remote = git_bytes(&root, &["config", "--get", "remote.origin.url"], 4096).ok();
        let identity = remote
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|value| normalize_repository_identity(value).ok());
        let worktree_clean = git_bytes(
            &root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
            BUNDLE_INPUT_MAX,
        )?
        .is_empty();

        let spec_path = relative_binding_path(&root, Path::new(&self.spec))?;
        let spec_bytes = read_bound_input(&root.join(&spec_path))?;
        let (executor_report_path, executor_report_present, executor_report_sha256) =
            if let Some(path) = self.executor_report_path.as_deref() {
                let relative = relative_binding_path(&root, Path::new(path))?;
                let bytes = read_bound_input(&root.join(&relative))?;
                let current = crate::executor_report::parse_str(std::str::from_utf8(&bytes)?)?;
                if self.checked_executor_report.as_ref() != Some(&current) {
                    return Err("executor report changed or was not evaluated".into());
                }
                (
                    Some(relative),
                    true,
                    Some(format!("sha256:{}", sha256_hex(&bytes))),
                )
            } else {
                if self.checked_executor_report.is_some() {
                    return Err("checked executor report requires an input path".into());
                }
                (None, false, None)
            };

        let range = format!("{baseline_oid}..{head_oid}");
        let binary_diff = git_bytes(
            &root,
            &[
                "-c",
                "diff.external=",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--binary",
                &range,
                "--",
            ],
            BUNDLE_INPUT_MAX,
        )?;
        let name_status = parse_name_status(&git_bytes(
            &root,
            &[
                "-c",
                "diff.external=",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--name-status",
                "-z",
                &range,
                "--",
            ],
            BUNDLE_INPUT_MAX,
        )?)?;

        let snapshot_head = resolve_commit(&root, "HEAD")?;
        let snapshot_clean = git_bytes(
            &root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
            BUNDLE_INPUT_MAX,
        )?
        .is_empty();
        if snapshot_head != head_oid || snapshot_clean != worktree_clean {
            return Err("snapshot_changed".into());
        }

        let mut audit_configuration = BTreeMap::new();
        audit_configuration.insert("baseline_input".into(), serde_json::json!(self.baseline));
        audit_configuration.insert("require_clean_worktree".into(), serde_json::json!(true));
        let mut index_metadata = BTreeMap::new();
        index_metadata.insert("source".into(), serde_json::json!("mmcg"));

        Ok(Manifest {
            repository: RepositoryBinding {
                identity,
                baseline_oid,
                head_oid,
                worktree_clean,
            },
            inputs: InputBinding {
                spec_path,
                spec_sha256: format!("sha256:{}", sha256_hex(&spec_bytes)),
                executor_report_path,
                executor_report_present,
                executor_report_sha256,
            },
            diff: DiffBinding {
                name_status,
                binary_diff_sha256: format!("sha256:{}", sha256_hex(&binary_diff)),
            },
            tool: ToolBinding {
                name: "mastermind".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                bundle_schema: crate::audit_bundle::ENVELOPE_SCHEMA,
            },
            audit_configuration,
            index_metadata,
            verdict: self.verdict,
            declared_files: self.spec_files,
            changed_files: self.changed_files,
            verified_claims: self.verified_claims,
            failed_claims: self.failed_claims,
            discrepancies: self
                .discrepancies
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?,
            snapshot_drift: self
                .snapshot_drift
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?,
            snapshot_changed: false,
            mmcg_queries: self.mmcg_queries,
            verify_commands: self.commands,
            human_summary: self.human_summary,
        })
    }
}

fn resolve_commit(root: &Path, reference: &str) -> Result<String, Box<dyn std::error::Error>> {
    if reference.starts_with('-') || reference.contains(['\0', '\n', '\r']) {
        return Err("unsafe git reference".into());
    }
    let expression = format!("{reference}^{{commit}}");
    let bytes = git_bytes(
        root,
        &["rev-parse", "--verify", "--end-of-options", &expression],
        128,
    )?;
    let oid = std::str::from_utf8(&bytes)?.trim().to_ascii_lowercase();
    if oid.len() != 40 || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("git did not return a full object ID".into());
    }
    Ok(oid)
}

fn git_bytes(
    root: &Path,
    args: &[&str],
    limit: usize,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output = crate::diff::run_bounded_git_with_limit(root, args, None, limit)
        .map_err(|error| format!("audit_git_{}", error.code()))?;
    if !output.success {
        return Err("audit_git_failed".into());
    }
    Ok(output.stdout)
}

fn relative_binding_path(root: &Path, path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    let normalized = crate::audit_bundle::normalize_macos_system_alias(path);
    #[cfg(windows)]
    let normalized = normalize_windows_audit_input(path)?;
    #[cfg(any(target_os = "macos", windows))]
    let path = normalized.as_path();
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .map_err(|_| "audit input path is outside root")?
    } else {
        path
    };
    Ok(crate::audit_bundle::normalize_relative_path(relative)?)
}

#[cfg(windows)]
fn normalize_windows_audit_input(
    path: &Path,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    use std::os::windows::fs::MetadataExt;
    use std::path::{Component, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    if !path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let mut cursor = PathBuf::new();
    for component in path.components() {
        cursor.push(component.as_os_str());
        match component {
            Component::Prefix(_) | Component::RootDir => {}
            Component::Normal(_) => {
                let metadata = std::fs::symlink_metadata(&cursor)?;
                if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    return Err("audit input contains a reparse point".into());
                }
            }
            _ => return Err("unsafe audit input path component".into()),
        }
    }
    // Expand short (8.3) names and the verbatim prefix like the canonical root.
    // Reparse points must be rejected before resolving the input spelling.
    Ok(path.canonicalize()?)
}

fn read_bound_input(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("audit input is not a regular non-symlink file".into());
    }
    if metadata.len() > crate::audit_bundle::BUNDLE_INPUT_MAX as u64 {
        return Err("audit input exceeds 16 MiB".into());
    }
    Ok(std::fs::read(path)?)
}

fn parse_name_status(
    bytes: &[u8],
) -> Result<Vec<crate::audit_bundle::DiffEntry>, Box<dyn std::error::Error>> {
    let fields: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .collect();
    let mut entries = Vec::new();
    let mut cursor = 0;
    while cursor < fields.len() {
        let status = std::str::from_utf8(fields[cursor])?.to_string();
        cursor += 1;
        if status.starts_with('R') || status.starts_with('C') {
            if cursor + 1 >= fields.len() {
                return Err("malformed git name-status output".into());
            }
            let old_path = crate::audit_bundle::normalize_relative_path(Path::new(
                std::str::from_utf8(fields[cursor])?,
            ))?;
            let path = crate::audit_bundle::normalize_relative_path(Path::new(
                std::str::from_utf8(fields[cursor + 1])?,
            ))?;
            cursor += 2;
            entries.push(crate::audit_bundle::DiffEntry {
                status,
                path,
                old_path: Some(old_path),
            });
        } else {
            if cursor >= fields.len() {
                return Err("malformed git name-status output".into());
            }
            let path = crate::audit_bundle::normalize_relative_path(Path::new(
                std::str::from_utf8(fields[cursor])?,
            ))?;
            cursor += 1;
            entries.push(crate::audit_bundle::DiffEntry {
                status,
                path,
                old_path: None,
            });
        }
    }
    Ok(entries)
}

fn resolve_head_sha(root: Option<&Path>) -> String {
    let mut cmd = std::process::Command::new("git");
    cmd.args(["rev-parse", "--short", "HEAD"]);
    if let Some(r) = root {
        cmd.current_dir(r);
    }
    cmd.output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn build_human_summary(
    verdict: Verdict,
    findings: &[Finding],
    failed_claims: &[String],
    verified_claims: &[String],
) -> String {
    let verdict_str = match verdict {
        Verdict::Held => "HELD",
        Verdict::Drift => "DRIFT",
        Verdict::Broken => "BROKEN",
    };
    let n_findings = findings.len();
    if !failed_claims.is_empty() {
        return format!(
            "Mastermind audit: {verdict_str} — {} claim(s) failed, {} passed",
            failed_claims.len(),
            verified_claims.len()
        );
    }
    if n_findings == 0 {
        return format!("Mastermind audit: {verdict_str} — all checks passed");
    }
    format!(
        "Mastermind audit: {verdict_str} — {n_findings} finding(s) ({} errors, {} warnings)",
        findings
            .iter()
            .filter(|f| matches!(
                f,
                Finding::SnapshotSymbolGone { .. }
                    | Finding::SnapshotUnresolved { .. }
                    | Finding::RemovedSymbolNotAcknowledged { .. }
                    | Finding::RemovalAcknowledgementUnresolved { .. }
                    | Finding::ClaimedSymbolMissing { .. }
                    | Finding::ClaimedSymbolNotAdded { .. }
                    | Finding::ExecutorClaimUnresolved { .. }
                    | Finding::ExecutorReportRejected { .. }
                    | Finding::VerificationRequirementUnmet { .. }
                    | Finding::HallucinatedSymbol { .. }
                    | Finding::MissingCallEdge { .. }
                    | Finding::ClaimedSignatureMismatch { .. }
                    | Finding::ObservedExitCodeNonZero { .. }
                    | Finding::ObservedZeroTests { .. }
            ))
            .count(),
        findings
            .iter()
            .filter(|f| !matches!(
                f,
                Finding::SnapshotSymbolGone { .. }
                    | Finding::SnapshotUnresolved { .. }
                    | Finding::RemovedSymbolNotAcknowledged { .. }
                    | Finding::RemovalAcknowledgementUnresolved { .. }
                    | Finding::ClaimedSymbolMissing { .. }
                    | Finding::ClaimedSymbolNotAdded { .. }
                    | Finding::ExecutorClaimUnresolved { .. }
                    | Finding::ExecutorReportRejected { .. }
                    | Finding::VerificationRequirementUnmet { .. }
                    | Finding::HallucinatedSymbol { .. }
                    | Finding::MissingCallEdge { .. }
                    | Finding::ClaimedSignatureMismatch { .. }
                    | Finding::ObservedExitCodeNonZero { .. }
                    | Finding::ObservedZeroTests { .. }
            ))
            .count(),
    )
}

fn check_snapshot_claim(
    claim: &SymbolClaim,
    scopes: &[Scope<'_>],
    store: &Store,
    removals: Option<&spec_removals::Plan>,
    findings: &mut Vec<Finding>,
) {
    if let Some(removals) = removals {
        match removals.accepts_snapshot(&claim.name, scopes, claim.signature.as_deref()) {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                findings.push(Finding::SnapshotUnresolved {
                    symbol: claim.name.clone(),
                    reason: format!("baseline_{}", error.reason),
                    matches: error.matches,
                });
                return;
            }
        }
    }
    check_current_snapshot(
        &claim.name,
        claim.callers,
        claim.signature.as_deref(),
        spec_symbols::resolve(store, &claim.name, scopes),
        store,
        findings,
    );
}

fn check_current_snapshot(
    name: &str,
    callers: Option<u32>,
    signature: Option<&str>,
    resolved: Result<Resolved, Unresolved>,
    store: &Store,
    findings: &mut Vec<Finding>,
) {
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(error) if error.reason == "missing" => {
            findings.push(Finding::SnapshotSymbolGone {
                symbol: name.to_string(),
            });
            return;
        }
        Err(error) => {
            findings.push(Finding::SnapshotUnresolved {
                symbol: name.to_string(),
                reason: error.reason.to_string(),
                matches: error.matches,
            });
            return;
        }
    };
    if let Some(declared) = signature {
        if resolved.symbol.signature.as_deref() != Some(declared) {
            findings.push(Finding::SnapshotSignatureDrift {
                symbol: name.to_string(),
                spec_says: declared.to_string(),
                index_says: resolved.symbol.signature.clone(),
            });
        }
    }
    if let Some(declared) = callers {
        let live = match store.callers_of(&resolved.symbol.name, resolved.language.as_deref(), None)
        {
            Ok(rows) => rows.len() as u32,
            Err(_) => {
                findings.push(Finding::SnapshotUnresolved {
                    symbol: name.to_string(),
                    reason: "caller_query_failed".to_string(),
                    matches: None,
                });
                return;
            }
        };
        if live != declared {
            findings.push(Finding::SnapshotCallerDrift {
                symbol: name.to_string(),
                spec_says: declared,
                index_says: live,
            });
        }
    }
}

fn compute_verdict(findings: &[Finding]) -> Verdict {
    if findings.iter().any(|f| {
        matches!(
            f,
            Finding::SnapshotSymbolGone { .. }
                | Finding::SnapshotUnresolved { .. }
                | Finding::RemovedSymbolNotAcknowledged { .. }
                | Finding::RemovalAcknowledgementUnresolved { .. }
                | Finding::ClaimedSymbolMissing { .. }
                | Finding::ClaimedSymbolNotAdded { .. }
                | Finding::ExecutorClaimUnresolved { .. }
                | Finding::ExecutorReportRejected { .. }
                | Finding::VerificationRequirementUnmet { .. }
                | Finding::HallucinatedSymbol { .. }
                | Finding::MissingCallEdge { .. }
                | Finding::ClaimedSignatureMismatch { .. }
                | Finding::ObservedExitCodeNonZero { .. }
                | Finding::ObservedZeroTests { .. }
        )
    }) {
        return Verdict::Broken;
    }
    if findings.is_empty() {
        Verdict::Held
    } else {
        Verdict::Drift
    }
}

/// Heuristic test-name extractor over the Tests Plan section.
///
/// **Best-effort signal, not a gate.** `PlannedTestNotAdded` is Drift (warning,
/// not Broken) for this reason — the detector covers a slice of naming
/// conventions and misses several. Don't rely on it for "did the executor write
/// the tests"; use frontmatter `verify[].cmd` to run the actual suite.
///
/// Recognises:
/// - backticked tokens shaped like `test_*`, `*_test`, `it_*`, `should_*`
/// - bare-word `test_*` even outside backticks (plain bullets)
///
/// Does NOT recognise (planner: document these explicitly):
/// - Jest / Vitest `it("does x", ...)` / `describe(...)` — name is a string
///   literal in the test file, not a function symbol
/// - Playwright `test("logs in", ...)` — same shape, not a function symbol
/// - Table-driven cases (one Rust `#[test] fn cases() { for case in ... }`)
/// - Golden / snapshot tests where the "name" is a fixture filename
/// - Modifications to EXISTING tests — only new function symbols appear in
///   `symbol_diff.added`
///
/// Returns deduplicated names in source order.
fn extract_planned_test_names(body: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();

    // Pass 1: backticked tokens.
    let mut chars = body.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '`' {
            continue;
        }
        let rest = &body[i + 1..];
        let Some(end) = rest.find('`') else { continue };
        let token = &rest[..end];
        if is_test_name(token) && seen.insert(token.to_string()) {
            out.push(token.to_string());
        }
        for _ in 0..end + 1 {
            chars.next();
        }
    }

    // Pass 2: bare `test_*` words (often in unbacked bullets).
    for word in body.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if is_test_name(word) && seen.insert(word.to_string()) {
            out.push(word.to_string());
        }
    }
    out
}

fn is_test_name(s: &str) -> bool {
    if s.len() < 3 || s.len() > 100 {
        return false;
    }
    if !s.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return false;
    }
    s.starts_with("test_")
        || s.ends_with("_test")
        || s.starts_with("it_")
        || s.starts_with("should_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::Indexer;
    use crate::spec;
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    fn init_repo(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "mmcg-audit-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for args in [
            ["init", "-q", "--initial-branch=main"].as_slice(),
            ["config", "user.email", "t@t"].as_slice(),
            ["config", "user.name", "t"].as_slice(),
            ["config", "commit.gpgsign", "false"].as_slice(),
        ] {
            let out = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {:?} failed", args);
        }
        // Tests keep the index DB inside the repo; real projects gitignore it.
        // Without this it lands in the audit's file scope as an untracked file.
        fs::write(dir.join(".gitignore"), "idx.db*\n").unwrap();
        dir
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

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn bundle_preserves_legacy_aliases_and_constructor() {
        let report = Report {
            spec: "spec.md".into(),
            git_ref: "main".into(),
            verdict: Verdict::Held,
            findings: Vec::new(),
            symbol_diff: None,
            claim_checks: None,
            executor_report: None,
        };

        let value = serde_json::to_value(Bundle::from_report(&report, None)).unwrap();
        assert_eq!(value["git_ref"], value["baseline"]);
        assert_eq!(value["files_diff"], value["changed_files"]);
    }

    #[test]
    fn flags_unexpected_files_and_drift() {
        let dir = init_repo("scope_creep");
        // Baseline: foo() with 1 caller.
        write(
            &dir,
            "src/lib.py",
            "def helper(): pass\ndef caller():\n    helper()\n",
        );
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);
        git(&dir, &["tag", "baseline"]);

        // Executor edited lib.py AND created scope_creep.py (not in spec).
        write(
            &dir,
            "src/lib.py",
            "def helper(): pass\ndef caller():\n    helper()\ndef caller2():\n    helper()\n",
        );
        write(&dir, "src/scope_creep.py", "def extra(): pass\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "head"]);

        // Index HEAD.
        let db = dir.join("idx.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();

        // Spec: claims to touch only src/lib.py, says helper had 1 caller.
        let spec_body = "\
## Goals
Add caller2() in `src/lib.py`
## Alternatives Considered
- A — rejected
## Pre-edit symbol snapshot
- `helper` — 1 callers
## Tests Plan
- n/a
## Documentation Plan
- n/a
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("spec.md", spec_body);
        let r = run(&s, &store, &dir, "baseline").unwrap();

        // helper now has 2 callers — drift; scope_creep.py wasn't in spec.
        assert!(r.findings.iter().any(
            |f| matches!(f, Finding::UnexpectedFile { file } if file == "src/scope_creep.py")
        ));
        assert!(r.findings.iter().any(|f| matches!(f, Finding::SnapshotCallerDrift { symbol, spec_says, index_says } if symbol == "helper" && *spec_says == 1 && *index_says == 2)));
        assert_eq!(r.verdict, Verdict::Drift);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn flags_signature_drift() {
        let dir = init_repo("sig_drift");
        // Baseline: refresh() takes no params.
        write(&dir, "src/lib.py", "def refresh():\n    return 1\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);
        git(&dir, &["tag", "baseline"]);

        // Executor changed signature: added a `force` param.
        write(
            &dir,
            "src/lib.py",
            "def refresh(force=False):\n    return 1\n",
        );
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "head"]);

        let db = dir.join("idx.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();

        // Spec recorded the OLD signature in the snapshot.
        let spec_body = "\
## Goals
- Refactor `refresh`
## Alternatives Considered
- A
## Pre-edit symbol snapshot
- `refresh` — 0 callers, signature `def refresh()`
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("spec.md", spec_body);
        let r = run(&s, &store, &dir, "baseline").unwrap();
        assert!(r.findings.iter().any(|f| matches!(
            f,
            Finding::SnapshotSignatureDrift { symbol, spec_says, .. }
                if symbol == "refresh" && spec_says == "def refresh()"
        )));
        // Signature drift alone is a Drift verdict, not Broken.
        assert_eq!(r.verdict, Verdict::Drift);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn flags_removed_symbol_not_acknowledged() {
        let dir = init_repo("removed_silent");
        write(
            &dir,
            "src/lib.py",
            "def will_be_removed(): pass\ndef stays(): pass\n",
        );
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);
        git(&dir, &["tag", "baseline"]);

        // Executor silently removed `will_be_removed`.
        write(&dir, "src/lib.py", "def stays(): pass\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "head"]);

        let db = dir.join("idx.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();

        // Spec only mentions `stays` — `will_be_removed` is silent.
        let spec_body = "\
## Goals
- Keep `stays`
## Alternatives Considered
- A
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("spec.md", spec_body);
        let r = run(&s, &store, &dir, "baseline").unwrap();
        assert!(r.findings.iter().any(|f| matches!(
            f,
            Finding::RemovedSymbolNotAcknowledged { symbol, .. } if symbol == "will_be_removed"
        )));
        assert_eq!(r.verdict, Verdict::Broken);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn does_not_flag_removed_symbol_acknowledged_in_spec() {
        let dir = init_repo("removed_ack");
        write(&dir, "src/lib.py", "def old_api(): pass\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);
        git(&dir, &["tag", "baseline"]);
        write(&dir, "src/lib.py", "# replaced\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "head"]);

        let db = dir.join("idx.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();

        // Spec mentions old_api in Goals — intentional removal.
        let spec_body = "\
## Goals
- Remove deprecated `old_api`
## Alternatives Considered
- A
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("spec.md", spec_body);
        let r = run(&s, &store, &dir, "baseline").unwrap();
        assert!(!r.findings.iter().any(|f| matches!(
            f,
            Finding::RemovedSymbolNotAcknowledged { symbol, .. } if symbol == "old_api"
        )));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn flags_planned_test_not_added() {
        let dir = init_repo("test_not_added");
        write(&dir, "src/lib.py", "def existing(): pass\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);
        git(&dir, &["tag", "baseline"]);

        // Executor added `test_foo` but NOT `test_missing`.
        write(
            &dir,
            "src/lib.py",
            "def existing(): pass\ndef test_foo(): pass\n",
        );
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "head"]);

        let db = dir.join("idx.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();

        let spec_body = "\
## Goals
- Add tests
## Alternatives Considered
- A
## Tests Plan
- `test_foo` — covers happy path
- `test_missing` — covers edge case
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("spec.md", spec_body);
        let r = run(&s, &store, &dir, "baseline").unwrap();
        assert!(r.findings.iter().any(|f| matches!(
            f,
            Finding::PlannedTestNotAdded { test } if test == "test_missing"
        )));
        // test_foo WAS added — should not be flagged.
        assert!(!r.findings.iter().any(|f| matches!(
            f,
            Finding::PlannedTestNotAdded { test } if test == "test_foo"
        )));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn frontmatter_breaking_changes_replaces_lowercase_substring_match() {
        // Legacy heuristic was fooled by `Do not remove `old_api`` (mention !=
        // ack). With frontmatter the audit ONLY trusts
        // `breaking_changes.removed_symbols`; prose mention no longer suffices.
        let dir = init_repo("frontmatter_breaking_strict");
        write(
            &dir,
            "src/lib.py",
            "def old_api(): pass\ndef stays(): pass\n",
        );
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);
        git(&dir, &["tag", "baseline"]);
        // Executor silently removed old_api.
        write(&dir, "src/lib.py", "def stays(): pass\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "head"]);

        let db = dir.join("idx.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();

        // Spec mentions old_api in prose (`Do not remove`) but doesn't list it
        // in breaking_changes. Legacy heuristic: passed silently. Frontmatter
        // strict mode: Broken verdict.
        let spec_body = "---
id: \"1\"
breaking_changes:
  removed_symbols: []
---

## Goals
- Keep `stays`. Do not remove `old_api`.
## Alternatives Considered
- a
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("spec.md", spec_body);
        let r = run(&s, &store, &dir, "baseline").unwrap();
        assert!(
            r.findings.iter().any(|f| matches!(
                f,
                Finding::RemovedSymbolNotAcknowledged { symbol, .. } if symbol == "old_api"
            )),
            "expected `old_api` flagged despite the `Do not remove` prose mention; \
             frontmatter strict mode requires structured ack"
        );
        assert_eq!(r.verdict, Verdict::Broken);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn frontmatter_breaking_changes_accepts_explicit_acknowledgement() {
        let dir = init_repo("frontmatter_breaking_acked");
        write(&dir, "src/lib.py", "def old_api(): pass\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);
        git(&dir, &["tag", "baseline"]);
        write(&dir, "src/lib.py", "# replaced\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "head"]);

        let db = dir.join("idx.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();

        // Frontmatter lists old_api in removed_symbols → audit accepts.
        let spec_body = "---
id: \"2\"
breaking_changes:
  removed_symbols:
    - old_api
---

## Goals
- Drop deprecated API
## Alternatives Considered
- A
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("spec.md", spec_body);
        let r = run(&s, &store, &dir, "baseline").unwrap();
        assert!(
            !r.findings.iter().any(|f| matches!(
                f,
                Finding::RemovedSymbolNotAcknowledged { symbol, .. } if symbol == "old_api"
            )),
            "old_api explicitly acked in frontmatter — should not flag"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn norm_path_strips_dotslash_and_backslash() {
        assert_eq!(norm_path("./src/foo.ts"), "src/foo.ts");
        assert_eq!(norm_path("src/foo.ts"), "src/foo.ts");
        assert_eq!(norm_path(r"src\foo.ts"), "src/foo.ts");
        assert_eq!(norm_path(r".\src\foo.ts"), "src/foo.ts");
        assert_eq!(norm_path("./src/foo.ts"), norm_path("src/foo.ts"));
        assert_eq!(norm_path(r"src\foo.ts"), norm_path("src/foo.ts"));
        assert_ne!(norm_path("src/foo.ts"), norm_path("src/bar.ts"));
    }

    #[test]
    fn file_scoped_claim_dotslash_prefix_matches() {
        let dir = init_repo("claim_norm_path");
        write(
            &dir,
            "src/checkout.go",
            "package main\nfunc CancelOrder() {}\n",
        );
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);
        git(&dir, &["tag", "baseline"]);
        git(&dir, &["commit", "--allow-empty", "-q", "-m", "head"]);

        let db = dir.join("idx.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();

        use crate::executor_report::{Claim, ExecutorReport};
        let er = ExecutorReport {
            claims: vec![Claim::FunctionAdded {
                symbol: "CancelOrder".to_string(),
                file: Some("./src/checkout.go".to_string()),
                signature: None,
            }],
            verify: vec![],
            canonical: None,
        };

        let checks = executor_claims::evaluate(
            &er,
            &executor_claims::Context {
                store: &store,
                changes: &diff::DeclarationChanges::default(),
                repo_root: &dir,
                baseline_oid: "baseline",
                complete: true,
                deadline: Instant::now() + std::time::Duration::from_secs(10),
            },
        );
        let findings: Vec<_> = checks
            .into_iter()
            .filter_map(|check| check.finding)
            .collect();
        assert!(
            !findings
                .iter()
                .any(|f| matches!(f, Finding::ClaimedSymbolMissing { .. })),
            "./src/checkout.go claim should match stored src/checkout.go"
        );
        assert!(findings
            .iter()
            .any(|finding| matches!(finding, Finding::ClaimedSymbolNotAdded { .. })));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn broken_when_snapshot_symbol_gone() {
        let dir = init_repo("symbol_gone");
        write(&dir, "src/lib.py", "def will_be_removed(): pass\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);
        git(&dir, &["tag", "baseline"]);
        // Executor removed the symbol.
        write(&dir, "src/lib.py", "# now empty\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "head"]);

        let db = dir.join("idx.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();

        let spec_body = "\
## Goals
- Rename `will_be_removed` to `renamed`
## Alternatives Considered
- A
## Pre-edit symbol snapshot
- `will_be_removed` — 0 callers
## Tests Plan
- t
## Documentation Plan
- d
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("spec.md", spec_body);
        let r = run(&s, &store, &dir, "baseline").unwrap();
        assert!(r.findings.iter().any(
            |f| matches!(f, Finding::SnapshotSymbolGone { symbol } if symbol == "will_be_removed")
        ));
        assert_eq!(r.verdict, Verdict::Broken);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn audits_complete_but_uncommitted_work() {
        // Regression: post-flight runs before the commit step, so HEAD is still
        // the baseline. With a `<baseline>..HEAD` file scope the diff was empty
        // by construction — finished work produced one `missing_expected_file`
        // per scoped file, a `+0 -0 ~0` symbol diff, and a Drift verdict.
        let dir = init_repo("uncommitted_work");
        write(&dir, "src/lib.py", "def helper(): pass\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "baseline"]);
        git(&dir, &["tag", "baseline"]);

        // Executor's work: a tracked file edited, a new file created. Nothing
        // committed — exactly what `git status` shows at hand-off.
        write(
            &dir,
            "src/lib.py",
            "def helper(): pass\ndef added_by_executor(): pass\n",
        );
        write(&dir, "src/new_module.py", "def brand_new(): pass\n");

        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&dir)
            .output()
            .unwrap();
        let baseline = Command::new("git")
            .args(["rev-parse", "baseline"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert_eq!(
            head.stdout, baseline.stdout,
            "the regression only reproduces while HEAD is still the baseline"
        );

        let db = dir.join("idx.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(&dir).index_all(&mut store, false).unwrap();

        let spec_body = "---
id: \"1\"
touches:
  - file: src/lib.py
  - file: src/new_module.py
---

## Goals
- Add `added_by_executor` and `src/new_module.py`
## Alternatives Considered
- A
## Tests Plan
- n/a
## Documentation Plan
- n/a
## Observability Plan
- n/a
## Performance Considerations
- O(1)
";
        let s = spec::parse_str("spec.md", spec_body);
        let r = run(&s, &store, &dir, "baseline").unwrap();

        let missing: Vec<&str> = r
            .findings
            .iter()
            .filter_map(|f| match f {
                Finding::MissingExpectedFile { file } => Some(file.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            missing.is_empty(),
            "both scoped files were written; nothing is missing: {missing:?}"
        );
        assert_eq!(r.verdict, Verdict::Held, "findings: {:?}", r.findings);

        // ... and the symbol diff reflects the work rather than reporting +0 -0 ~0.
        let added: Vec<&str> = r
            .symbol_diff
            .as_ref()
            .expect("symbol diff")
            .added
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(added.contains(&"added_by_executor"), "added: {added:?}");
        assert!(added.contains(&"brand_new"), "added: {added:?}");
        fs::remove_dir_all(&dir).ok();
    }
}
