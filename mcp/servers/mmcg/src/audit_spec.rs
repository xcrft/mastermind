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
                Finding::UnexpectedFile { .. } | Finding::MissingExpectedFile { .. } => "⚠️ ",
                Finding::SnapshotCallerDrift { .. } | Finding::SnapshotSignatureDrift { .. } => {
                    "⚠️ "
                }
                Finding::SnapshotSymbolGone { .. } => "❌",
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
    // SymbolGone is the only "contract broken" finding — a pre-edit symbol
    // disappearing means the executor likely renamed/deleted it without
    // saying so. Other findings are scope drift, not broken contracts.
    if findings
        .iter()
        .any(|f| matches!(f, Finding::SnapshotSymbolGone { .. }))
    {
        return Verdict::Broken;
    }
    if findings.is_empty() {
        Verdict::Held
    } else {
        Verdict::Drift
    }
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
