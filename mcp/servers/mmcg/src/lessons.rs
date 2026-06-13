//! Mechanical `_lessons.md` writer.
//!
//! `mastermind-auditor` (the LLM subagent) is supposed to append a one-line
//! lesson to `.mastermind/tasks/_lessons.md` whenever its verdict is `Drift` or
//! `Broken` — but that path depends on the planner remembering to spawn the
//! auditor AND the auditor LLM remembering to call `Write` on the file. In
//! practice the file stays empty even when audits surface drift.
//!
//! This module is the deterministic fallback: `mmcg audit-spec` and
//! `mmcg run-task`'s post-phase call into here directly. Entries are prefixed
//! `[auto]` so they're distinguishable from the LLM-auditor's root-cause
//! analyses (the LLM still adds richer commentary when it runs).

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::audit_spec::{Finding, Report, Verdict};

const HEADER: &str = "# Lessons learned\n\n\
One-line lessons from auditor verdicts. Newest at the bottom. Read by the planner\n\
before drafting non-trivial specs (see `mastermind-task-planning` SKILL).\n\n\
Lines prefixed with `[auto]` are written by `mmcg audit-spec` / `run-task` and\n\
summarize the mechanical findings. The LLM auditor appends richer root-cause\n\
entries below them when it runs.\n\n";

/// Append a lesson line to `.mastermind/tasks/_lessons.md` if the audit
/// verdict is `Drift` or `Broken`. No-op on `Held`. Best-effort — IO errors
/// bubble so the caller can log without failing the audit itself.
pub fn append_if_drift_or_broken(
    repo_root: &Path,
    spec_path: &Path,
    report: &Report,
) -> std::io::Result<bool> {
    if matches!(report.verdict, Verdict::Held) {
        return Ok(false);
    }
    let lessons_path = repo_root
        .join(".mastermind")
        .join("tasks")
        .join("_lessons.md");
    if let Some(parent) = lessons_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let needs_header = !lessons_path.exists();
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&lessons_path)?;
    if needs_header {
        f.write_all(HEADER.as_bytes())?;
    }
    let line = format_lesson_line(spec_path, report);
    writeln!(f, "{line}")?;
    Ok(true)
}

fn format_lesson_line(spec_path: &Path, report: &Report) -> String {
    let date = today_ymd();
    let task_id = derive_task_id(spec_path);
    let verdict = match report.verdict {
        Verdict::Held => "held",
        Verdict::Drift => "partial drift",
        Verdict::Broken => "contract broken",
    };
    let summary = summarize_findings(&report.findings);
    format!("- {date} `{task_id}` — {verdict} — [auto] {summary}")
}

/// Derive a stable task identifier from the spec path.
///
/// Prefers the parent folder name (`.mastermind/tasks/042-name/spec.md` →
/// `042-name`). Falls back to the filename stem for legacy flat layouts so
/// pre-0.7.0 specs still produce a readable identifier.
fn derive_task_id(spec_path: &Path) -> String {
    let filename = spec_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if filename == "spec.md" {
        if let Some(parent) = spec_path.parent() {
            if let Some(name) = parent.file_name().and_then(|n| n.to_str()) {
                return name.to_string();
            }
        }
    }
    spec_path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("<unknown>")
        .to_string()
}

fn summarize_findings(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return "verdict without findings — investigate".into();
    }
    let mut counts: std::collections::BTreeMap<&'static str, u32> = Default::default();
    for f in findings {
        let key = match f {
            Finding::UnexpectedFile { .. } => "scope creep",
            Finding::MissingExpectedFile { .. } => "missing expected file",
            Finding::SnapshotCallerDrift { .. } => "caller drift",
            Finding::SnapshotSignatureDrift { .. } => "signature drift",
            Finding::SnapshotSymbolGone { .. } => "snapshot symbol gone",
            Finding::RemovedSymbolNotAcknowledged { .. } => "silent symbol removal",
            Finding::PlannedTestNotAdded { .. } => "planned test missing",
            Finding::ClaimedSymbolMissing { .. } => "claimed symbol missing",
            Finding::HallucinatedSymbol { .. } => "hallucinated symbol",
            Finding::MissingCallEdge { .. } => "missing call edge",
            Finding::VacuousTestClaim { .. } => "vacuous test claim",
            Finding::ClaimedSignatureMismatch { .. } => "claimed signature mismatch",
            Finding::ObservedExitCodeNonZero { .. } => "observed exit code nonzero",
            Finding::ObservedZeroTests { .. } => "observed zero tests",
        };
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .map(|(k, n)| format!("{n}× {k}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn today_ymd() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    ymd_from_unix(secs)
}

/// Convert a unix timestamp (seconds since epoch, UTC) to `YYYY-MM-DD`.
///
/// Howard Hinnant's civil-calendar algorithm (public domain). Avoids pulling
/// in a date crate (`time` / `chrono`) for a single format call.
fn ymd_from_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_spec::{Finding, Report, Verdict};
    use std::path::PathBuf;

    fn report(verdict: Verdict, findings: Vec<Finding>) -> Report {
        Report {
            spec: "irrelevant".into(),
            git_ref: "HEAD".into(),
            verdict,
            findings,
            symbol_diff: None,
        }
    }

    #[test]
    fn ymd_known_dates() {
        // 1970-01-01
        assert_eq!(ymd_from_unix(0), "1970-01-01");
        // 2026-06-03 00:00:00 UTC = 20_607 days * 86_400.
        assert_eq!(ymd_from_unix(1_780_444_800), "2026-06-03");
        // Leap-day check: 2024-02-29
        assert_eq!(ymd_from_unix(1_709_164_800), "2024-02-29");
        // End-of-year boundary: 2023-12-31
        assert_eq!(ymd_from_unix(1_703_980_800), "2023-12-31");
    }

    #[test]
    fn task_id_from_folder_layout() {
        let p = PathBuf::from(".mastermind/tasks/042-rate-limiter/spec.md");
        assert_eq!(derive_task_id(&p), "042-rate-limiter");
    }

    #[test]
    fn task_id_falls_back_to_stem_for_legacy_flat_layout() {
        let p = PathBuf::from(".mastermind/tasks/042-rate-limiter.md");
        assert_eq!(derive_task_id(&p), "042-rate-limiter");
    }

    #[test]
    fn held_verdict_writes_nothing() {
        let dir = std::env::temp_dir().join("mmcg_lessons_held");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let spec = dir.join(".mastermind/tasks/001-x/spec.md").to_path_buf();
        let r = report(Verdict::Held, vec![]);
        let wrote = append_if_drift_or_broken(&dir, &spec, &r).unwrap();
        assert!(!wrote);
        assert!(!dir.join(".mastermind/tasks/_lessons.md").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn drift_verdict_creates_file_with_header_and_appends() {
        let dir = std::env::temp_dir().join("mmcg_lessons_drift");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let spec = PathBuf::from(".mastermind/tasks/042-name/spec.md");
        let r = report(
            Verdict::Drift,
            vec![Finding::UnexpectedFile {
                file: "src/extra.rs".into(),
            }],
        );

        let wrote = append_if_drift_or_broken(&dir, &spec, &r).unwrap();
        assert!(wrote);

        let lessons = dir.join(".mastermind/tasks/_lessons.md");
        let body = fs::read_to_string(&lessons).unwrap();
        assert!(body.starts_with("# Lessons learned"));
        assert!(body.contains("042-name"));
        assert!(body.contains("partial drift"));
        assert!(body.contains("[auto]"));
        assert!(body.contains("1× scope creep"));

        // A second drift append goes below without re-adding the header.
        let r2 = report(
            Verdict::Broken,
            vec![
                Finding::SnapshotSymbolGone {
                    symbol: "foo".into(),
                },
                Finding::SnapshotSymbolGone {
                    symbol: "bar".into(),
                },
            ],
        );
        append_if_drift_or_broken(&dir, &spec, &r2).unwrap();
        let body2 = fs::read_to_string(&lessons).unwrap();
        assert_eq!(body2.matches("# Lessons learned").count(), 1);
        assert!(body2.contains("contract broken"));
        assert!(body2.contains("2× snapshot symbol gone"));

        fs::remove_dir_all(&dir).ok();
    }
}
