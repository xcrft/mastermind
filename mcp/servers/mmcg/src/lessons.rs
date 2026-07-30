//! Durable lesson candidates backed by mechanical audit evidence.
//!
//! The controller cannot infer a reusable root cause from a verdict, so it
//! records an explicit `candidate` instead of presenting finding counts as a
//! finished lesson. A planner promotes, resolves, or supersedes the candidate
//! after semantic review. Stable IDs and an exclusive file lock make repeated
//! or concurrent audit invocations idempotent.

use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::audit_spec::{Finding, Report, Verdict};

const HEADER: &str = "# Project lessons\n\n\
Reusable project knowledge, newest at the bottom. Mechanical audit events enter\n\
as `candidate`; they are not active guidance until semantic review replaces the\n\
pending lesson and changes the status to `active`, `resolved`, or `superseded`.\n\n\
A repeat of an already-recorded event refreshes that entry — newer evidence, a\n\
higher occurrence count — instead of adding a sibling. Reviewed entries are\n\
never rewritten.\n\n\
Required fields: Status, Task, Kind, Provenance, Evidence, Supersedes,\n\
Occurrences, Last seen, and Reusable lesson.\n\n";

/// Record a deduplicated lesson candidate for a `Drift`/`Broken` audit.
/// A held contract is not evidence that a reusable lesson exists.
pub fn append_audit_candidate(
    repo_root: &Path,
    spec_path: &Path,
    report: &Report,
) -> std::io::Result<bool> {
    if matches!(report.verdict, Verdict::Held) {
        return Ok(false);
    }
    let task_id = derive_task_id(spec_path);
    let verdict = verdict_label(report.verdict);
    let observation = summarize_findings(&report.findings);
    // Keyed on task + kind alone. Keying on the findings themselves gave every
    // audit round its own entry, because an iterate-until-green loop shifts the
    // finding set by an item or two each pass — one task could accumulate a
    // dozen near-identical candidates, none of them reviewed.
    let id = stable_id(&task_id, "audit_contract_failure", "");
    let spec = relative_display(repo_root, spec_path);
    let audit = spec_path
        .parent()
        .map(|parent| parent.join("audit.md"))
        .unwrap_or_else(|| spec_path.with_file_name("audit.md"));
    let audit_resolved = if audit.is_absolute() {
        audit.clone()
    } else {
        repo_root.join(&audit)
    };
    let evidence = if audit_resolved.is_file() {
        format!("`{}`; `{}`", spec, relative_display(repo_root, &audit))
    } else {
        format!("`{spec}`; standalone audit output observed in this invocation but not persisted")
    };
    append_candidate(
        repo_root,
        Candidate {
            id,
            task_id,
            kind: "audit_contract_failure",
            observed: format!("{verdict}; {observation}"),
            evidence,
        },
    )
}

/// Record a deduplicated candidate when a spec exhausts its iteration budget.
pub fn append_iteration_budget_candidate(
    repo_root: &Path,
    spec_path: &Path,
    iteration: u32,
) -> std::io::Result<bool> {
    let task_id = derive_task_id(spec_path);
    let id = stable_id(&task_id, "iteration_budget_exhausted", "preflight");
    let evidence = format!("`{}`", relative_display(repo_root, spec_path));
    append_candidate(
        repo_root,
        Candidate {
            id,
            task_id,
            kind: "iteration_budget_exhausted",
            observed: format!("pre-flight iteration {iteration} exceeded the configured budget"),
            evidence,
        },
    )
}

struct Candidate {
    id: String,
    task_id: String,
    kind: &'static str,
    observed: String,
    evidence: String,
}

fn append_candidate(repo_root: &Path, candidate: Candidate) -> std::io::Result<bool> {
    let lessons_path = repo_root.join(".mastermind/tasks/_lessons.md");
    if fs::symlink_metadata(&lessons_path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to write lessons through a symlink",
        ));
    }
    if let Some(parent) = lessons_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lessons_path)?;
    file.lock_exclusive()?;

    let result = (|| {
        let mut body = String::new();
        file.read_to_string(&mut body)?;
        let Some(merged) = merge_candidate(&body, &candidate) else {
            return Ok(false);
        };
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(merged.as_bytes())?;
        file.sync_data()?;
        Ok(true)
    })();
    FileExt::unlock(&file)?;
    result
}

/// Fold `candidate` into `body`, returning the new file contents or `None` when
/// nothing should change.
///
/// A re-occurrence refreshes the existing entry rather than appending a sibling:
/// same lesson, newer evidence, one more occurrence. An entry that semantic
/// review has already moved off `candidate` is left exactly as it is — the
/// reviewer's text is the durable artifact, and later mechanical noise must not
/// reopen or overwrite it.
fn merge_candidate(body: &str, candidate: &Candidate) -> Option<String> {
    let heading = format!("## {}", candidate.id);
    let lines: Vec<&str> = body.lines().collect();
    let Some(start) = lines.iter().position(|line| line.trim() == heading) else {
        let mut merged = if body.is_empty() {
            HEADER.to_string()
        } else {
            let mut existing = body.to_string();
            if !existing.ends_with('\n') {
                existing.push('\n');
            }
            existing
        };
        merged.push_str(&render_entry(candidate, &today_ymd(), 1));
        return Some(merged);
    };

    let end = lines[start + 1..]
        .iter()
        .position(|line| line.starts_with("## "))
        .map_or(lines.len(), |offset| start + 1 + offset);
    let section = &lines[start..end];
    if field(section, "Status") != Some("candidate") {
        return None;
    }
    let today = today_ymd();
    let created = field(section, "Created").unwrap_or(&today).to_string();
    let occurrences = field(section, "Occurrences")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(1)
        .saturating_add(1);

    let mut merged = String::new();
    for line in &lines[..start] {
        merged.push_str(line);
        merged.push('\n');
    }
    // `render_entry` opens with a blank line, so drop the one already sitting
    // between the previous entry and this heading to avoid doubling it.
    while merged.ends_with("\n\n") {
        merged.pop();
    }
    merged.push_str(&render_entry(candidate, &created, occurrences));
    for line in &lines[end..] {
        merged.push_str(line);
        merged.push('\n');
    }
    Some(merged)
}

fn render_entry(candidate: &Candidate, created: &str, occurrences: u32) -> String {
    format!(
        "\n## {}\n\n- **Created:** {created}\n- **Last seen:** {}\n- **Occurrences:** {occurrences}\n- **Status:** candidate\n- **Task:** `{}`\n- **Kind:** `{}`\n- **Observed:** {}\n- **Provenance:** mastermind controller\n- **Evidence:** {}\n- **Supersedes:** none\n- **Reusable lesson:** pending semantic review\n",
        candidate.id,
        today_ymd(),
        candidate.task_id,
        candidate.kind,
        candidate.observed,
        candidate.evidence,
    )
}

fn field<'a>(section: &[&'a str], name: &str) -> Option<&'a str> {
    let prefix = format!("- **{name}:** ");
    section
        .iter()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .map(str::trim)
}

fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Held => "held",
        Verdict::Drift => "partial drift",
        Verdict::Broken => "contract broken",
    }
}

/// Derive a stable task identifier from the spec path.
///
/// Prefers the parent folder name (`.mastermind/tasks/042-name/spec.md` →
/// `042-name`). Falls back to the filename stem for legacy flat layouts, so
/// pre-0.7.0 specs still get a readable identifier.
fn derive_task_id(spec_path: &Path) -> String {
    let filename = spec_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if filename == "spec.md" {
        if let Some(parent) = spec_path.parent() {
            if let Some(name) = parent.file_name().and_then(|n| n.to_str()) {
                return sanitize_task_id(name);
            }
        }
    }
    spec_path
        .file_stem()
        .and_then(|n| n.to_str())
        .map(sanitize_task_id)
        .unwrap_or_else(|| "unknown".to_string())
}

fn sanitize_task_id(raw: &str) -> String {
    let mut sanitized = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let safe = if ch.is_ascii_alphanumeric() || ch == '_' {
            ch
        } else {
            '-'
        };
        if safe != '-' || !sanitized.ends_with('-') {
            sanitized.push(safe);
        }
    }
    let sanitized = sanitized.trim_matches('-');
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized.to_string()
    }
}

fn stable_id(task_id: &str, kind: &str, observation: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(task_id.as_bytes());
    hash.update([0]);
    hash.update(kind.as_bytes());
    hash.update([0]);
    hash.update(observation.as_bytes());
    let digest = crate::hex::encode(&hash.finalize());
    format!("lesson-{}", &digest[..16])
}

fn relative_display(repo_root: &Path, path: &Path) -> String {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    };
    let display = resolved
        .strip_prefix(repo_root)
        .unwrap_or(&resolved)
        .to_string_lossy()
        .replace('\\', "/");
    display
        .chars()
        .map(|ch| {
            if ch == '`' || ch.is_control() {
                '-'
            } else {
                ch
            }
        })
        .collect()
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
/// Howard Hinnant's civil-calendar algorithm (public domain). Avoids a date
/// crate (`time` / `chrono`) for a single format call.
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
        // Leap day: 2024-02-29
        assert_eq!(ymd_from_unix(1_709_164_800), "2024-02-29");
        // Year-end boundary: 2023-12-31
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
        let wrote = append_audit_candidate(&dir, &spec, &r).unwrap();
        assert!(!wrote);
        assert!(!dir.join(".mastermind/tasks/_lessons.md").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_audit_failure_for_a_task_folds_into_one_candidate() {
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

        let wrote = append_audit_candidate(&dir, &spec, &r).unwrap();
        assert!(wrote);

        let lessons = dir.join(".mastermind/tasks/_lessons.md");
        let body = fs::read_to_string(&lessons).unwrap();
        assert!(body.starts_with("# Project lessons"));
        assert!(body.contains("042-name"));
        assert!(body.contains("partial drift"));
        assert!(body.contains("**Status:** candidate"));
        assert!(body.contains("**Provenance:** mastermind controller"));
        assert!(body.contains("**Reusable lesson:** pending semantic review"));
        assert!(body.contains("not persisted"));
        assert!(body.contains("1× scope creep"));

        assert!(body.contains("**Occurrences:** 1"));

        // Different evidence, then a different verdict entirely: the audit loop
        // reshapes its finding set every pass, and each pass used to mint its
        // own candidate. All of it now folds into the one entry for this task.
        let different_evidence = report(
            Verdict::Drift,
            vec![Finding::UnexpectedFile {
                file: "src/other.rs".into(),
            }],
        );
        assert!(append_audit_candidate(&dir, &spec, &different_evidence).unwrap());

        let broken = report(
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
        assert!(append_audit_candidate(&dir, &spec, &broken).unwrap());

        let body2 = fs::read_to_string(&lessons).unwrap();
        assert_eq!(body2.matches("# Project lessons").count(), 1);
        assert_eq!(body2.matches("**Status:** candidate").count(), 1);
        assert_eq!(body2.matches("## lesson-").count(), 1);
        assert!(body2.contains("**Occurrences:** 3"));
        // Refreshed to the newest observation, not frozen at the first.
        assert!(body2.contains("contract broken"));
        assert!(body2.contains("2× snapshot symbol gone"));
        assert!(!body2.contains("1× scope creep"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn semantic_review_survives_later_mechanical_noise() {
        let dir = tempfile::tempdir().unwrap();
        let spec = PathBuf::from(".mastermind/tasks/042-name/spec.md");
        let first = report(
            Verdict::Drift,
            vec![Finding::UnexpectedFile {
                file: "src/extra.rs".into(),
            }],
        );
        assert!(append_audit_candidate(dir.path(), &spec, &first).unwrap());

        let lessons = dir.path().join(".mastermind/tasks/_lessons.md");
        let reviewed = fs::read_to_string(&lessons)
            .unwrap()
            .replace("**Status:** candidate", "**Status:** active")
            .replace(
                "**Reusable lesson:** pending semantic review",
                "**Reusable lesson:** scope the spec before handing off.",
            );
        fs::write(&lessons, &reviewed).unwrap();

        let later = report(
            Verdict::Broken,
            vec![Finding::SnapshotSymbolGone {
                symbol: "foo".into(),
            }],
        );
        assert!(!append_audit_candidate(dir.path(), &spec, &later).unwrap());
        assert_eq!(fs::read_to_string(&lessons).unwrap(), reviewed);
    }

    #[test]
    fn iteration_budget_candidate_stays_a_single_entry() {
        let dir = tempfile::tempdir().unwrap();
        let spec = PathBuf::from(".mastermind/tasks/007-retry/spec.md");
        assert!(append_iteration_budget_candidate(dir.path(), &spec, 4).unwrap());
        assert!(append_iteration_budget_candidate(dir.path(), &spec, 5).unwrap());
        let body = fs::read_to_string(dir.path().join(".mastermind/tasks/_lessons.md")).unwrap();
        assert_eq!(body.matches("iteration_budget_exhausted").count(), 1);
        assert_eq!(body.matches("## lesson-").count(), 1);
        assert!(body.contains("**Occurrences:** 2"));
        assert!(body.contains("iteration 5"));
    }

    #[test]
    fn concurrent_candidate_writes_produce_one_entry() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::sync::Arc::new(dir.path().to_path_buf());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let mut threads = Vec::new();
        for _ in 0..8 {
            let root = root.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                append_iteration_budget_candidate(
                    &root,
                    Path::new(".mastermind/tasks/007-retry/spec.md"),
                    4,
                )
                .unwrap()
            }));
        }
        for thread in threads {
            assert!(thread.join().unwrap());
        }
        // The lock has to serialize a full read-modify-write now, not just an
        // append: eight racing writers must still leave one entry, and the
        // occurrence count must not have lost an update.
        let body = fs::read_to_string(root.join(".mastermind/tasks/_lessons.md")).unwrap();
        assert_eq!(body.matches("iteration_budget_exhausted").count(), 1);
        assert_eq!(body.matches("## lesson-").count(), 1);
        assert!(body.contains("**Occurrences:** 8"));
    }

    #[test]
    fn task_id_is_markdown_safe() {
        let path = PathBuf::from(".mastermind/tasks/042-name`\n- injected/spec.md");
        assert_eq!(derive_task_id(&path), "042-name-injected");
    }

    #[cfg(unix)]
    #[test]
    fn lesson_writer_rejects_symlink_target() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join(".mastermind/tasks");
        fs::create_dir_all(&tasks).unwrap();
        let outside = dir.path().join("outside.md");
        fs::write(&outside, "unchanged\n").unwrap();
        symlink(&outside, tasks.join("_lessons.md")).unwrap();

        let spec = PathBuf::from(".mastermind/tasks/007-retry/spec.md");
        assert!(append_iteration_budget_candidate(dir.path(), &spec, 4).is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "unchanged\n");
    }
}
