//! `mmcg audit-spec` — post-execution mechanical audit.
//!
//! Complements the LLM auditor by handling the **deterministic** part of the
//! audit contract: file-set comparison, pre-edit snapshot drift, symbol-level
//! diff. The LLM still does semantic judgment (does the test plan actually
//! cover the new behavior? is the doc update sufficient?) — this catches the
//! "you claimed file X but git diff doesn't show it" / "snapshot said 8
//! callers, now 5" class of bugs without prompt discipline.
//!
//! Inputs: a parsed spec + the git ref to compare against (typically
//! `main` or the merge-base) + the indexed `Store` for live symbol counts.
//!
//! Outputs structured findings:
//! - `unexpected_file` — file changed in git but not mentioned in spec
//! - `missing_expected_file` — file mentioned in spec, not changed in git
//! - `snapshot_caller_drift` — pre-edit snapshot count != post-edit count
//! - `snapshot_signature_drift` — pre-edit signature != post-edit signature
//! - `snapshot_symbol_gone` — pre-edit symbol no longer in the index
//!
//! Phase B (test/doc/observability plan validation, non-breaking API check)
//! is deliberately out of scope for v1.

use crate::diff::{self, DiffError, SymbolDiff};
use crate::spec::{ParsedSpec, SymbolClaim};
use crate::store::Store;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Every check passed — no drift, no scope creep.
    Held,
    /// At least one finding but no contract violation — usually scope creep
    /// (unexpected files) or minor signature drift. Planner reads + decides.
    Drift,
    /// Spec claimed something that didn't happen, OR pre-edit symbol now
    /// missing entirely.
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
    /// Could be intentional (executor changed param shape) or a side-effect.
    /// LLM auditor decides; we just flag the mechanical fact.
    SnapshotSignatureDrift {
        symbol: String,
        spec_says: String,
        index_says: Option<String>,
    },
    /// Symbol present in pre-edit snapshot has no current entry in mmcg —
    /// renamed, deleted, or moved out of the indexed tree.
    SnapshotSymbolGone { symbol: String },
    /// A symbol disappeared between baseline and HEAD AND the spec text
    /// doesn't mention the name anywhere — silent breaking change. Spec
    /// should acknowledge intentional removals in Goals / Notes.
    RemovedSymbolNotAcknowledged { symbol: String, file: String },
    /// The Tests Plan section names a test (`test_foo`, `it('bar')`, etc.)
    /// that doesn't appear in `symbol_diff.added`. Either the executor
    /// skipped it or the test name in the plan was wrong.
    PlannedTestNotAdded { test: String },
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub spec: String,
    pub git_ref: String,
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
    /// Raw symbol-level diff from `mmcg_symbols_changed_since` — pasted in
    /// so the LLM auditor has the full context for semantic judgment.
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
                | Finding::PlannedTestNotAdded { .. } => "⚠️ ",
                Finding::SnapshotSymbolGone { .. }
                | Finding::RemovedSymbolNotAcknowledged { .. } => "❌",
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
    }
}

/// Run all Phase A checks against a parsed spec.
///
/// `git_ref` must resolve via `git rev-parse` in `repo_root` — symbols-changed-
/// since uses it as the baseline. `store` is the live index at HEAD.
pub fn run(
    spec: &ParsedSpec,
    store: &Store,
    repo_root: &Path,
    git_ref: &str,
) -> Result<Report, DiffError> {
    let symbol_diff = diff::symbols_changed_since(store, repo_root, git_ref)?;
    let mut findings: Vec<Finding> = Vec::new();

    // 1. File scope check — symmetric difference of spec.mentioned_files vs
    //    git diff --name-only. Mentioned files that are doc-only (README.md
    //    etc.) are still counted; the LLM auditor decides if scope makes sense.
    //
    //    Filter out `.mastermind/` from the diff side — that directory is
    //    local working state (the index DB, the specs themselves) and is
    //    universally gitignored in real projects. When a fixture eval commits
    //    it for test reasons, we'd otherwise drown the report in noise.
    let spec_files: HashSet<&str> = spec.mentioned_files.iter().map(String::as_str).collect();
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

    // 3. Removed-symbol-not-acknowledged — for every symbol that disappeared
    //    in the git diff, decide if it's a deliberate removal or a silent
    //    breaking change.
    //
    //    Resolution order:
    //    a) Frontmatter present + `breaking_changes.removed_symbols` non-empty
    //       → AUTHORITATIVE. Exact-name match against that list. Anything not
    //         in the list is flagged. No lowercase-substring fuzz, no false
    //         positives from incidental mentions like `Do not remove old_api`.
    //    b) Frontmatter present but no `removed_symbols` → strict mode. ANY
    //         removed non-module symbol is flagged (frontmatter forces the
    //         planner to explicitly ack removals).
    //    c) No frontmatter → fall back to the legacy lowercase-substring
    //         heuristic. Documented as imprecise; planners are encouraged to
    //         migrate to frontmatter.
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
        // Module-level synthetic symbols shouldn't trigger this — they're an
        // artifact of file removal, not a public API delete.
        if removed.kind == "module" {
            continue;
        }
        let acknowledged = match &frontmatter_acks {
            // Frontmatter present → exact match against breaking_changes list.
            // The empty-list case still goes through here and flags everything
            // (strict mode, intended).
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
    //    Tests Plan, cross-reference against names added in symbol_diff.
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
    // Signature comparison — same any-match rule as verify_spec (multiple
    // matches accepted if at least one signature still matches the claim).
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
    // "Contract broken" = silent deletion of something the spec didn't warn
    // about. Both SnapshotSymbolGone (pre-tracked symbol vanished) and
    // RemovedSymbolNotAcknowledged (any symbol removed without spec mention)
    // qualify — they're the strongest invariant violations we can detect
    // mechanically.
    if findings.iter().any(|f| {
        matches!(
            f,
            Finding::SnapshotSymbolGone { .. } | Finding::RemovedSymbolNotAcknowledged { .. }
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
/// **This is a best-effort signal, not a gate.** `PlannedTestNotAdded` lives
/// in the Drift bucket (warning, not Broken) for exactly this reason — the
/// detector below covers a slice of test-naming conventions and misses several
/// common ones. Don't rely on it for "did the executor write the tests" — use
/// frontmatter `verify[].cmd` to run the actual test suite for that.
///
/// Recognises:
/// - backticked tokens shaped like `test_*`, `*_test`, `it_*`, `should_*`
/// - bare-word `test_*` patterns even outside backticks (for plain bullets)
///
/// Does NOT recognise (planner: document these explicitly, don't rely on the
/// heuristic for them):
/// - Jest / Vitest `it("does x", ...)` / `describe(...)` — the test name is
///   a string literal in the test file, not a function symbol
/// - Playwright `test("logs in", ...)` — same shape, not a function symbol
/// - Table-driven test cases (single Rust `#[test] fn cases() { for case in ... }`)
/// - Golden / snapshot tests where the test "name" is a fixture filename
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

    // Pass 2: bare `test_*` words (often appear in unbacked bullets).
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
        // The legacy heuristic was fooled by `Do not remove `old_api`` (mention
        // ≠ acknowledgement). With frontmatter, the audit ONLY trusts
        // `breaking_changes.removed_symbols` — a prose mention is no longer
        // sufficient ack.
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

        // Spec MENTIONS old_api in prose (`Do not remove`) but doesn't list it
        // in breaking_changes. Under legacy heuristic, this passed silently.
        // Under frontmatter strict mode, it's a Broken verdict.
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
