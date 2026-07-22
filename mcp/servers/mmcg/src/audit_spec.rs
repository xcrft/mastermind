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
use crate::spec::{ParsedSpec, SymbolClaim};
use crate::store::Store;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

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
    /// File appears in `git diff` but the spec didn't mention it.
    UnexpectedFile { file: String },
    /// File was mentioned in the spec but `git diff` doesn't show changes.
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
    /// Symbol disappeared between baseline and HEAD AND the spec text doesn't
    /// mention the name anywhere — silent breaking change. Spec should
    /// acknowledge intentional removals in Goals / Notes.
    RemovedSymbolNotAcknowledged { symbol: String, file: String },
    /// Tests Plan names a test (`test_foo`, `it('bar')`, etc.) absent from
    /// `symbol_diff.added` — executor skipped it or the planned name was wrong.
    PlannedTestNotAdded { test: String },
    /// Executor claimed they added symbol X but it has no definition in the live
    /// index — the add didn't happen or indexing missed it.
    ClaimedSymbolMissing {
        symbol: String,
        file: Option<String>,
    },
    /// Executor claimed X calls existing Y but Y has no definition anywhere in
    /// the index — Y was hallucinated.
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
    /// Executor claimed a test command passed, but no test files exist in the
    /// relevant directory — vacuous pass (zero tests ran).
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
    /// Executor attached `observed: { tests_run: 0 }` with `exit_code: 0` —
    /// exited cleanly but ran zero tests. Vacuous pass confirmed by the
    /// executor's own output rather than a static file-existence check.
    ObservedZeroTests { cmd: String },
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub spec: String,
    pub git_ref: String,
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
    /// Raw symbol-level diff from `mmcg_symbols_changed_since` — pasted in so
    /// the LLM auditor has full context for semantic judgment.
    pub symbol_diff: Option<SymbolDiff>,
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
                | Finding::RemovedSymbolNotAcknowledged { .. }
                | Finding::ClaimedSymbolMissing { .. }
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
        Finding::RemovedSymbolNotAcknowledged { symbol, file } => {
            format!("removed_symbol_not_acknowledged: `{symbol}` deleted from `{file}` but spec doesn't mention it — potential silent breaking change")
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
        Finding::HallucinatedSymbol {
            from_symbol,
            to_symbol,
        } => {
            format!("hallucinated_symbol: executor claimed `{from_symbol}` calls existing `{to_symbol}` but `{to_symbol}` has no definition in the index — it was hallucinated")
        }
        Finding::MissingCallEdge {
            from_symbol,
            to_symbol,
        } => {
            format!("missing_call_edge: executor claimed `{from_symbol}` calls `{to_symbol}` but no call edge from `{from_symbol}` to `{to_symbol}` exists in the index")
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
    let symbol_diff = diff::symbols_changed_since(store, repo_root, git_ref)?;
    let mut findings: Vec<Finding> = Vec::new();

    // 1. File scope check — symmetric difference of declared files vs
    //    git diff --name-only.
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
        check_snapshot_claim(claim, store, &mut findings);
    }

    // 3. Removed-symbol-not-acknowledged — for each symbol gone in the git diff,
    //    decide: deliberate removal or silent breaking change?
    //
    //    Resolution order:
    //    a) Frontmatter + `breaking_changes.removed_symbols` non-empty →
    //       AUTHORITATIVE. Exact-name match; anything else is flagged. No
    //       lowercase-substring fuzz, no false positives from incidental
    //       mentions like `Do not remove old_api`.
    //    b) Frontmatter but no `removed_symbols` → strict mode: ANY removed
    //       non-module symbol is flagged (forces the planner to ack removals).
    //    c) No frontmatter → legacy lowercase-substring heuristic. Imprecise;
    //       planners encouraged to migrate to frontmatter.
    let frontmatter_acks: Option<std::collections::HashSet<String>> =
        spec.frontmatter.as_ref().map(|fm| {
            fm.breaking_changes
                .removed_symbols
                .iter()
                .map(|s| s.name().to_string())
                .collect()
        });
    let spec_body_lower = spec
        .sections
        .values()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    for removed in &symbol_diff.removed {
        // Module-level synthetic symbols are an artifact of file removal, not a
        // public API delete — skip.
        if removed.kind == "module" {
            continue;
        }
        let acknowledged = match &frontmatter_acks {
            // Frontmatter → exact match against breaking_changes list. Empty list
            // flows through here and flags everything (strict mode, intended).
            Some(acks) => acks.contains(&removed.name),
            // No frontmatter → legacy lowercase-substring fallback.
            None => spec_body_lower.contains(&removed.name.to_lowercase()),
        };
        if !acknowledged {
            findings.push(Finding::RemovedSymbolNotAcknowledged {
                symbol: removed.name.clone(),
                file: removed.file.clone(),
            });
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

    let verdict = compute_verdict(&findings);
    Ok(Report {
        spec: spec.path.clone(),
        git_ref: git_ref.to_string(),
        verdict,
        findings,
        symbol_diff: Some(symbol_diff),
    })
}

/// Run Phase A checks + executor-report mechanical checks.
///
/// `executor_report == None` is equivalent to `run()`. When present, adds:
///  - Integration-claim verifier (2.2): hallucinated symbol, missing call edge
///  - Symbol-add verifier (2.1): claimed symbol not in index
///  - Vacuous test detector (2.3): test command claimed passed, no test files
pub fn run_with_report(
    spec: &ParsedSpec,
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
    executor_report: Option<&crate::executor_report::ExecutorReport>,
) -> Result<Report, DiffError> {
    let mut report = run(spec, store, repo_root, git_ref)?;

    if let Some(er) = executor_report {
        check_executor_claims(er, store, &mut report.findings);
        check_vacuous_tests(er, repo_root, &mut report.findings);
        report.verdict = compute_verdict(&report.findings);
    }

    Ok(report)
}

fn norm_path(p: &str) -> String {
    p.replace('\\', "/").trim_start_matches("./").to_string()
}

fn norm_paths_eq(stored: &str, claimed: &str) -> bool {
    norm_path(stored) == norm_path(claimed)
}

fn check_executor_claims(
    er: &crate::executor_report::ExecutorReport,
    store: &Store,
    findings: &mut Vec<Finding>,
) {
    use crate::executor_report::Claim;

    for claim in &er.claims {
        match claim {
            Claim::FunctionAdded {
                symbol,
                file,
                signature,
            } => {
                let all_hits = store.search_symbols(symbol, None, None).unwrap_or_default();
                let hits: Vec<_> = if let Some(f) = file {
                    all_hits
                        .into_iter()
                        .filter(|s| norm_paths_eq(&s.file_path, f))
                        .collect()
                } else {
                    all_hits
                };
                if hits.is_empty() {
                    findings.push(Finding::ClaimedSymbolMissing {
                        symbol: symbol.clone(),
                        file: file.clone(),
                    });
                    continue;
                }
                if let Some(claimed_sig) = signature {
                    let any_match = hits
                        .iter()
                        .any(|s| s.signature.as_deref() == Some(claimed_sig.as_str()));
                    if !any_match {
                        findings.push(Finding::ClaimedSignatureMismatch {
                            symbol: symbol.clone(),
                            file: file.clone(),
                            claimed: claimed_sig.clone(),
                            actual: hits.first().and_then(|s| s.signature.clone()),
                        });
                    }
                }
            }
            Claim::Integration {
                from,
                from_file,
                to,
                to_file,
                ..
            } => {
                let all_to_hits = store.search_symbols(to, None, None).unwrap_or_default();
                let to_hits: Vec<_> = if let Some(tf) = to_file {
                    all_to_hits
                        .into_iter()
                        .filter(|s| norm_paths_eq(&s.file_path, tf))
                        .collect()
                } else {
                    all_to_hits
                };
                if to_hits.is_empty() {
                    findings.push(Finding::HallucinatedSymbol {
                        from_symbol: from.clone(),
                        to_symbol: to.clone(),
                    });
                    continue;
                }
                let all_from = store.search_symbols(from, None, None).unwrap_or_default();
                let from_syms: Vec<_> = if let Some(ff) = from_file {
                    all_from
                        .into_iter()
                        .filter(|s| norm_paths_eq(&s.file_path, ff))
                        .collect()
                } else {
                    all_from
                };
                let call_exists = from_syms.iter().any(|s| {
                    store
                        .callees_of(s.id, None)
                        .unwrap_or_default()
                        .iter()
                        .any(|(name, _)| name == to)
                });
                if !call_exists {
                    findings.push(Finding::MissingCallEdge {
                        from_symbol: from.clone(),
                        to_symbol: to.clone(),
                    });
                }
            }
        }
    }
}

fn check_vacuous_tests(
    er: &crate::executor_report::ExecutorReport,
    repo_root: &Path,
    findings: &mut Vec<Finding>,
) {
    for v in &er.verify {
        let claimed_passed = v
            .claimed
            .as_deref()
            .is_some_and(|c| c.eq_ignore_ascii_case("passed") || c.eq_ignore_ascii_case("pass"));
        if !claimed_passed {
            continue;
        }
        if let Some(obs) = &v.observed {
            if let Some(code) = obs.exit_code {
                if code != 0 {
                    findings.push(Finding::ObservedExitCodeNonZero {
                        cmd: v.cmd.clone(),
                        exit_code: code,
                    });
                    continue;
                }
            }
            if obs.exit_code == Some(0) {
                if let Some(0) = obs.tests_run {
                    findings.push(Finding::ObservedZeroTests { cmd: v.cmd.clone() });
                    continue;
                }
            }
            // A positive observed test count is authoritative: real tests ran,
            // so the static file-scan below must not override it. Without this,
            // `cargo test` with only `tests/*.rs` integration tests would
            // false-positive as vacuous despite the executor reporting N ran.
            if matches!(obs.tests_run, Some(n) if n > 0) {
                continue;
            }
        }
        if let Some(reason) = vacuous_test_reason(&v.cmd, repo_root) {
            findings.push(Finding::VacuousTestClaim {
                cmd: v.cmd.clone(),
                reason,
            });
        }
    }
}

/// `Some(reason)` if the test run is provably vacuous (no test files in the
/// relevant scope). `None` when undeterminable — conservative: don't
/// false-positive.
fn vacuous_test_reason(cmd: &str, repo_root: &Path) -> Option<String> {
    let cmd_lower = cmd.to_lowercase();

    // `go test` — exclude `cargo test`, which contains the substring "go test"
    // ("car|go test"). Without the `cargo` guard it routes into the Go detector,
    // gets flagged for no `_test.go` files, and shadows the `cargo test` branch
    // below (unreachable for the literal command).
    if cmd_lower.contains("go test") && !cmd_lower.contains("cargo") {
        let pkg_dir = extract_go_package_dir(cmd, repo_root);
        let dir = repo_root.join(&pkg_dir);
        if dir.is_dir() && !has_files_matching(&dir, "_test.go") {
            return Some(format!("no *_test.go files in {pkg_dir}"));
        }
    } else if cmd_lower.contains("pytest")
        || cmd_lower.contains("python -m pytest")
        || cmd_lower.contains("python3 -m pytest")
    {
        let scope = extract_pytest_scope(cmd, repo_root);
        let dir = repo_root.join(&scope);
        if dir.is_dir() && !has_files_matching_pattern(&dir, "test_", ".py") {
            return Some(format!("no test_*.py files in {scope}"));
        }
    } else if cmd_lower.contains("cargo test") {
        // Unit tests live in `src/`, integration tests in `tests/`. Flag vacuous
        // only when NEITHER carries a test attribute — a crate tested entirely
        // integration-style (`tests/*.rs`, no `#[test]` in `src/`) is valid and
        // must not be flagged.
        //
        // `--manifest-path` puts the crate somewhere other than the repo root
        // (workspaces, monorepos). Scanning the root there reads a *sibling*
        // `tests/` — often CI fixtures in another language — and reports a
        // fully tested crate as vacuous.
        let (crate_root, scope) = match extract_cargo_manifest_dir(cmd) {
            Some(rel) => {
                let d = repo_root.join(&rel);
                if !d.is_dir() {
                    // Manifest points outside the tree we can see: undeterminable.
                    return None;
                }
                (d, format!("{rel}/"))
            }
            None => (repo_root.to_path_buf(), String::new()),
        };
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        for sub in ["src", "tests"] {
            let d = crate_root.join(sub);
            if d.is_dir() {
                dirs.push(d);
            }
        }
        if dirs.is_empty() {
            dirs.push(crate_root);
        }
        if !dirs.iter().any(|d| has_test_attr_in_dir(d)) {
            return Some(format!(
                "no test attribute (#[test], #[tokio::test], #[rstest], …) in \
                 {scope}src/ or {scope}tests/"
            ));
        }
    } else if (cmd_lower.contains("jest")
        || cmd_lower.contains("vitest")
        || cmd_lower.contains("npm test")
        || cmd_lower.contains("yarn test"))
        && !has_files_matching_pattern(repo_root, ".test.", "")
        && !has_files_matching_pattern(repo_root, ".spec.", "")
    {
        return Some("no *.test.* or *.spec.* files found".to_string());
    }

    None
}

fn extract_go_package_dir(cmd: &str, _root: &Path) -> String {
    for token in cmd.split_whitespace() {
        if token.starts_with("./") {
            let clean = token.trim_end_matches("/...");
            return clean.trim_start_matches("./").to_string();
        }
    }
    ".".to_string()
}

/// Crate directory named by `--manifest-path <dir>/Cargo.toml` (and the
/// `--manifest-path=…` spelling), relative to the repo root and slash-normalised.
/// `None` when the flag is absent or the manifest sits at the root — both mean
/// "the crate is the repo root", so the caller keeps its default scan.
fn extract_cargo_manifest_dir(cmd: &str) -> Option<String> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let mut raw: Option<&str> = None;
    for (i, t) in tokens.iter().enumerate() {
        if let Some(v) = t.strip_prefix("--manifest-path=") {
            raw = Some(v);
            break;
        }
        if *t == "--manifest-path" {
            raw = tokens.get(i + 1).copied().filter(|n| !n.starts_with('-'));
            break;
        }
    }
    let path = norm_path(raw?.trim_matches(|c| c == '"' || c == '\''));
    // Cargo wants the manifest file itself; tolerate a bare directory too.
    let dir = match path.rsplit_once('/') {
        Some((parent, last)) if last.eq_ignore_ascii_case("cargo.toml") => parent,
        None if path.eq_ignore_ascii_case("cargo.toml") => "",
        _ => path.as_str(),
    };
    let dir = dir.trim_end_matches('/');
    if dir.is_empty() || dir == "." {
        return None;
    }
    Some(dir.to_string())
}

fn extract_pytest_scope(cmd: &str, _root: &Path) -> String {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    for (i, t) in tokens.iter().enumerate() {
        if *t == "pytest" || t.ends_with("pytest") {
            if let Some(next) = tokens.get(i + 1) {
                if !next.starts_with('-') {
                    return next.to_string();
                }
            }
        }
    }
    ".".to_string()
}

fn has_files_matching(dir: &Path, suffix: &str) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(suffix))
        })
        .unwrap_or(false)
}

fn has_files_matching_pattern(dir: &Path, contains: &str, ends: &str) -> bool {
    walkdir::WalkDir::new(dir)
        .max_depth(5)
        .into_iter()
        .filter_map(|e| e.ok())
        .any(|e| {
            if !e.file_type().is_file() {
                return false;
            }
            let name = e.file_name().to_string_lossy();
            name.contains(contains) && (ends.is_empty() || name.ends_with(ends))
        })
}

fn has_test_attr_in_dir(dir: &Path) -> bool {
    walkdir::WalkDir::new(dir)
        .max_depth(6)
        .into_iter()
        .filter_map(|e| e.ok())
        .any(|e| {
            if !e.file_type().is_file() {
                return false;
            }
            if e.path().extension().and_then(|s| s.to_str()) != Some("rs") {
                return false;
            }
            std::fs::read_to_string(e.path())
                .map(|text| file_has_test_attr(&text))
                .unwrap_or(false)
        })
}

/// Recognises common Rust test-attribute spellings: built-in `#[test]`, async
/// variants like `#[tokio::test]` / `#[async_std::test]` (end in `::test]`), and
/// `#[rstest]` / `#[rstest(...)]`. Matching only literal `#[test]` used to
/// false-flag crates testing exclusively via these.
fn file_has_test_attr(text: &str) -> bool {
    text.contains("#[test]") || text.contains("::test]") || text.contains("#[rstest")
}

// ----- evidence bundle ------------------------------------------------------

/// Portable proof artifact written by `audit-spec --bundle`.
///
/// v2 adds: `head`, `spec_files`, `changed_files`, `verified_claims`,
/// `failed_claims`, `mmcg_queries`, `commands`, `human_summary`. Legacy fields
/// (`files_diff`, `discrepancies`, `snapshot_drift`) kept for back-compat.
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
    /// Logical mmcg queries issued during the audit (human inspection).
    pub mmcg_queries: Vec<String>,
    /// Verify commands extracted from the executor report.
    pub commands: Vec<String>,
    /// One-line verdict summary suitable for a PR comment title.
    pub human_summary: String,
    /// All findings (superset of `snapshot_drift`). Legacy field name kept.
    pub discrepancies: Vec<Finding>,
    /// Snapshot-drift findings only. Legacy field kept.
    pub snapshot_drift: Vec<Finding>,
    /// Legacy alias for `changed_files`, kept for back-compat.
    pub files_diff: Vec<String>,
    /// Legacy alias for `baseline`, kept for consumers parsing the pre-v2
    /// bundle format.
    pub git_ref: String,
    pub executor_report_path: Option<String>,
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
            .unwrap_or_default();

        let mut mmcg_queries: Vec<String> = Vec::new();
        let mut verified_claims: Vec<String> = Vec::new();
        let mut failed_claims: Vec<String> = Vec::new();

        if let Some(er) = executor_report {
            use crate::executor_report::Claim;
            for claim in &er.claims {
                match claim {
                    Claim::FunctionAdded { symbol, file, .. } => {
                        let q = format!("mmcg_search {symbol}");
                        if !mmcg_queries.contains(&q) {
                            mmcg_queries.push(q);
                        }
                        let norm_file = file.as_deref().map(norm_path);
                        let label = norm_file
                            .as_deref()
                            .map(|f| format!("function_added:{symbol}@{f}"))
                            .unwrap_or_else(|| format!("function_added:{symbol}"));
                        let failed = report.findings.iter().any(|f| match f {
                            Finding::ClaimedSymbolMissing {
                                symbol: s,
                                file: ff,
                            } => s == symbol && norm_file == ff.as_deref().map(norm_path),
                            Finding::ClaimedSignatureMismatch {
                                symbol: s,
                                file: ff,
                                ..
                            } => s == symbol && norm_file == ff.as_deref().map(norm_path),
                            _ => false,
                        });
                        if failed {
                            failed_claims.push(label);
                        } else {
                            verified_claims.push(label);
                        }
                    }
                    Claim::Integration {
                        from,
                        from_file,
                        to,
                        to_file,
                        ..
                    } => {
                        for name in [to.as_str(), from.as_str()] {
                            let q = format!("mmcg_search {name}");
                            if !mmcg_queries.contains(&q) {
                                mmcg_queries.push(q);
                            }
                        }
                        let callees_q = format!("mmcg_callees {from}");
                        if !mmcg_queries.contains(&callees_q) {
                            mmcg_queries.push(callees_q);
                        }
                        let norm_ff = from_file.as_deref().map(norm_path);
                        let norm_tf = to_file.as_deref().map(norm_path);
                        let label = match (norm_ff.as_deref(), norm_tf.as_deref()) {
                            (Some(ff), Some(tf)) => {
                                format!("integration:{from}@{ff}→{to}@{tf}")
                            }
                            (Some(ff), None) => format!("integration:{from}@{ff}→{to}"),
                            (None, Some(tf)) => format!("integration:{from}→{to}@{tf}"),
                            (None, None) => format!("integration:{from}→{to}"),
                        };
                        let failed = report.findings.iter().any(|f| match f {
                            Finding::HallucinatedSymbol {
                                from_symbol: fs,
                                to_symbol: ts,
                            } => fs == from && ts == to,
                            Finding::MissingCallEdge {
                                from_symbol: fs,
                                to_symbol: ts,
                            } => fs == from && ts == to,
                            _ => false,
                        });
                        if failed {
                            failed_claims.push(label);
                        } else {
                            verified_claims.push(label);
                        }
                    }
                }
            }
        }

        let commands: Vec<String> = executor_report
            .map(|er| er.verify.iter().map(|v| v.cmd.clone()).collect())
            .unwrap_or_default();

        let head = resolve_head_sha(root);

        let human_summary = build_human_summary(report, &failed_claims, &verified_claims);

        Self {
            verdict: format!("{:?}", report.verdict).to_lowercase(),
            spec: report.spec.clone(),
            baseline: report.git_ref.clone(),
            git_ref: report.git_ref.clone(),
            head,
            spec_files,
            changed_files: changed_files.clone(),
            verified_claims,
            failed_claims,
            mmcg_queries,
            commands,
            human_summary,
            discrepancies: report.findings.clone(),
            snapshot_drift,
            files_diff: changed_files,
            executor_report_path: executor_report_path.map(str::to_string),
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
                (
                    Some(relative),
                    true,
                    Some(format!("sha256:{}", sha256_hex(&bytes))),
                )
            } else {
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
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .map_err(|_| "audit input path is outside root")?
    } else {
        path
    };
    Ok(crate::audit_bundle::normalize_relative_path(relative)?)
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
    report: &Report,
    failed_claims: &[String],
    verified_claims: &[String],
) -> String {
    let verdict_str = match report.verdict {
        Verdict::Held => "HELD",
        Verdict::Drift => "DRIFT",
        Verdict::Broken => "BROKEN",
    };
    let n_findings = report.findings.len();
    if n_findings == 0 {
        return format!("Mastermind audit: {verdict_str} — all checks passed");
    }
    if !failed_claims.is_empty() {
        return format!(
            "Mastermind audit: {verdict_str} — {} claim(s) failed, {} passed",
            failed_claims.len(),
            verified_claims.len()
        );
    }
    format!(
        "Mastermind audit: {verdict_str} — {n_findings} finding(s) ({} errors, {} warnings)",
        report
            .findings
            .iter()
            .filter(|f| matches!(
                f,
                Finding::SnapshotSymbolGone { .. }
                    | Finding::RemovedSymbolNotAcknowledged { .. }
                    | Finding::ClaimedSymbolMissing { .. }
                    | Finding::HallucinatedSymbol { .. }
                    | Finding::MissingCallEdge { .. }
                    | Finding::ClaimedSignatureMismatch { .. }
                    | Finding::ObservedExitCodeNonZero { .. }
                    | Finding::ObservedZeroTests { .. }
            ))
            .count(),
        report
            .findings
            .iter()
            .filter(|f| !matches!(
                f,
                Finding::SnapshotSymbolGone { .. }
                    | Finding::RemovedSymbolNotAcknowledged { .. }
                    | Finding::ClaimedSymbolMissing { .. }
                    | Finding::HallucinatedSymbol { .. }
                    | Finding::MissingCallEdge { .. }
                    | Finding::ClaimedSignatureMismatch { .. }
                    | Finding::ObservedExitCodeNonZero { .. }
                    | Finding::ObservedZeroTests { .. }
            ))
            .count(),
    )
}

fn check_snapshot_claim(claim: &SymbolClaim, store: &Store, findings: &mut Vec<Finding>) {
    let hits = match store.search_symbols(&claim.name, None, None) {
        Ok(rows) => rows,
        Err(_) => return,
    };
    if hits.is_empty() {
        findings.push(Finding::SnapshotSymbolGone {
            symbol: claim.name.clone(),
        });
        return;
    }
    if let Some(spec_count) = claim.callers {
        let live = match store.callers_of(&claim.name, None, None) {
            Ok(callers) => callers.len() as u32,
            Err(_) => return,
        };
        if live != spec_count {
            findings.push(Finding::SnapshotCallerDrift {
                symbol: claim.name.clone(),
                spec_says: spec_count,
                index_says: live,
            });
        }
    }
    // Signature comparison — same any-match rule as verify_spec (accept if at
    // least one of multiple matches still matches the claim).
    if let Some(spec_sig) = &claim.signature {
        let live_sigs: Vec<Option<String>> = hits.iter().map(|s| s.signature.clone()).collect();
        let any_match = live_sigs
            .iter()
            .any(|s| s.as_deref() == Some(spec_sig.as_str()));
        if !any_match {
            findings.push(Finding::SnapshotSignatureDrift {
                symbol: claim.name.clone(),
                spec_says: spec_sig.clone(),
                index_says: live_sigs.into_iter().flatten().next(),
            });
        }
    }
}

fn compute_verdict(findings: &[Finding]) -> Verdict {
    if findings.iter().any(|f| {
        matches!(
            f,
            Finding::SnapshotSymbolGone { .. }
                | Finding::RemovedSymbolNotAcknowledged { .. }
                | Finding::ClaimedSymbolMissing { .. }
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
        assert!(norm_paths_eq("./src/foo.ts", "src/foo.ts"));
        assert!(norm_paths_eq(r"src\foo.ts", "src/foo.ts"));
        assert!(!norm_paths_eq("src/foo.ts", "src/bar.ts"));
    }

    #[test]
    fn file_has_test_attr_recognises_common_spellings() {
        assert!(file_has_test_attr("#[test]\nfn t() {}"));
        assert!(file_has_test_attr("#[tokio::test]\nasync fn t() {}"));
        assert!(file_has_test_attr("#[async_std::test]\nasync fn t() {}"));
        assert!(file_has_test_attr("#[rstest]\nfn t() {}"));
        assert!(file_has_test_attr(
            "#[rstest(input, case(1))]\nfn t(input: u8) {}"
        ));
        assert!(!file_has_test_attr("fn helper() {}\n// no tests here"));
    }

    #[test]
    fn cargo_test_not_vacuous_with_only_integration_tests() {
        // Regression: a crate with only integration-style tests (`tests/*.rs`,
        // no `#[test]` in `src/`) used to be flagged vacuous because the scan
        // looked only at `src/`.
        let dir = init_repo("cargo_integration_only");
        write(
            &dir,
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        );
        write(
            &dir,
            "tests/it.rs",
            "#[test]\nfn adds() { assert_eq!(2, 2); }\n",
        );
        assert!(
            vacuous_test_reason("cargo test", dir.as_path()).is_none(),
            "integration tests in tests/*.rs must not be flagged vacuous"
        );

        // A crate with no test attribute anywhere is still flagged.
        let bare = init_repo("cargo_no_tests");
        write(&bare, "src/lib.rs", "pub fn add() {}\n");
        assert!(vacuous_test_reason("cargo test", bare.as_path()).is_some());

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&bare).ok();
    }

    #[test]
    fn cargo_test_manifest_path_scopes_scan_to_the_crate() {
        // Regression: `cargo test --manifest-path <sub>/Cargo.toml` used to be
        // scanned at the repo root, so a monorepo whose root `tests/` holds
        // non-Rust fixtures reported a fully tested nested crate as vacuous.
        let dir = init_repo("cargo_manifest_path");
        write(
            &dir,
            "tests/ci_fixture.py",
            "def test_nothing():\n    pass\n",
        );
        write(
            &dir,
            "mcp/servers/mmcg/Cargo.toml",
            "[package]\nname = \"c\"\n",
        );
        write(
            &dir,
            "mcp/servers/mmcg/src/store.rs",
            "pub fn budget() {}\n#[test]\nfn work_budget() {}\n",
        );
        assert!(
            vacuous_test_reason(
                "cargo test --manifest-path mcp/servers/mmcg/Cargo.toml --locked work_budget",
                dir.as_path()
            )
            .is_none(),
            "tests in the crate named by --manifest-path must not be flagged vacuous"
        );
        // Without the flag the crate really is the repo root, and there the
        // only `tests/` carries no Rust test attribute.
        assert!(vacuous_test_reason("cargo test", dir.as_path()).is_some());
        // An unresolvable manifest is undeterminable, not vacuous.
        assert!(vacuous_test_reason(
            "cargo test --manifest-path absent/Cargo.toml",
            dir.as_path()
        )
        .is_none());

        // A nested crate genuinely without tests is still flagged — a decoy
        // `tests/` at the root must not vouch for it.
        let bare = init_repo("cargo_manifest_path_bare");
        write(&bare, "tests/it.rs", "#[test]\nfn decoy() {}\n");
        write(
            &bare,
            "crates/thing/Cargo.toml",
            "[package]\nname = \"t\"\n",
        );
        write(&bare, "crates/thing/src/lib.rs", "pub fn add() {}\n");
        let reason = vacuous_test_reason(
            "cargo test --manifest-path=crates/thing/Cargo.toml",
            bare.as_path(),
        )
        .expect("untested nested crate must still be flagged");
        assert!(
            reason.contains("crates/thing/src/"),
            "reason should name the scanned crate, got: {reason}"
        );

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&bare).ok();
    }

    #[test]
    fn extract_cargo_manifest_dir_handles_flag_spellings() {
        let d = |cmd: &str| extract_cargo_manifest_dir(cmd);
        assert_eq!(
            d("cargo test --manifest-path mcp/servers/mmcg/Cargo.toml --locked"),
            Some("mcp/servers/mmcg".to_string())
        );
        assert_eq!(
            d("cargo test --manifest-path=./crates/a/Cargo.toml"),
            Some("crates/a".to_string())
        );
        assert_eq!(
            d(r#"cargo test --manifest-path "crates/a/Cargo.toml""#),
            Some("crates/a".to_string())
        );
        // Bare directory instead of the manifest file.
        assert_eq!(
            d("cargo test --manifest-path crates/a/"),
            Some("crates/a".to_string())
        );
        // No relocation: absent flag, root manifest, or a missing value.
        assert_eq!(d("cargo test --locked --all"), None);
        assert_eq!(d("cargo test --manifest-path Cargo.toml"), None);
        assert_eq!(d("cargo test --manifest-path ./Cargo.toml"), None);
        assert_eq!(d("cargo test --manifest-path --locked"), None);
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
        };

        let mut findings = Vec::new();
        check_executor_claims(&er, &store, &mut findings);
        assert!(
            !findings
                .iter()
                .any(|f| matches!(f, Finding::ClaimedSymbolMissing { .. })),
            "./src/checkout.go claim should match stored src/checkout.go"
        );
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
}
