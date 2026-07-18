use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

const REPORT_BEGIN: &str = "mastermind:report-begin";
const REPORT_END: &str = "mastermind:report-end";
const LEGACY_BEGIN: &str = "mastermind:executor-begin";
const LEGACY_END: &str = "mastermind:executor-end";
const MAX_EXECUTOR_REPORT_BYTES: u64 = 1024 * 1024;

/// A single claim an executor made in its schema-v1 structured report tail.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Claim {
    FunctionAdded {
        symbol: String,
        #[serde(default)]
        file: Option<String>,
        #[serde(default)]
        signature: Option<String>,
    },
    Integration {
        from: String,
        #[serde(default)]
        from_file: Option<String>,
        to: String,
        #[serde(default)]
        to_file: Option<String>,
        #[serde(default)]
        relation: Option<String>,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedOutcome {
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub tests_run: Option<u32>,
}

/// Internal projection consumed by the deterministic audit checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyResult {
    pub cmd: String,
    #[serde(default)]
    pub claimed: Option<String>,
    #[serde(default)]
    pub observed: Option<ObservedOutcome>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorReport {
    #[serde(default)]
    pub claims: Vec<Claim>,
    #[serde(default)]
    pub verify: Vec<VerifyResult>,
}

impl ExecutorReport {
    pub fn is_empty(&self) -> bool {
        self.claims.is_empty() && self.verify.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReportStatus {
    Complete,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PhaseStatus {
    Done,
    Pending,
    StoppedHere,
    Skipped,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Phase {
    id: String,
    status: PhaseStatus,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Defect {
    kind: String,
    phase: String,
    details: String,
    remediation_hint: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VerificationStatus {
    Pass,
    Fail,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalVerification {
    cmd: String,
    result: VerificationStatus,
    #[serde(default)]
    output_excerpt: Option<String>,
    #[serde(default)]
    observed: Option<ObservedOutcome>,
}

/// Canonical executor-report schema v1. Keep this shape in lockstep with
/// `schemas/executor-report-v1.schema.json` and the installed skill template.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalExecutorReport {
    schema_version: u32,
    spec: String,
    status: ReportStatus,
    phases: Vec<Phase>,
    files_modified: Vec<String>,
    claims: Vec<Claim>,
    defects: Vec<Defect>,
    verifications: Vec<CanonicalVerification>,
}

impl TryFrom<CanonicalExecutorReport> for ExecutorReport {
    type Error = String;

    fn try_from(report: CanonicalExecutorReport) -> Result<Self, Self::Error> {
        if report.schema_version != 1 {
            return Err(format!(
                "unsupported executor report schema_version {}; expected 1",
                report.schema_version
            ));
        }
        if report.spec.trim().is_empty() {
            return Err("executor report spec must not be empty".into());
        }
        let mut phase_ids = HashSet::new();
        for phase in &report.phases {
            if phase.id.trim().is_empty() {
                return Err("executor report phase id must not be empty".into());
            }
            if !phase_ids.insert(phase.id.trim()) {
                return Err(format!(
                    "executor report phase id is duplicated: {}",
                    phase.id
                ));
            }
        }
        for path in &report.files_modified {
            if path.trim().is_empty() {
                return Err("executor report file path must not be empty".into());
            }
        }
        for defect in &report.defects {
            if defect.kind.trim().is_empty()
                || defect.phase.trim().is_empty()
                || defect.details.trim().is_empty()
                || defect.remediation_hint.trim().is_empty()
            {
                return Err("executor report defects require non-empty fields".into());
            }
        }
        for verification in &report.verifications {
            if verification.cmd.trim().is_empty() {
                return Err("executor report verification command must not be empty".into());
            }
        }

        match report.status {
            ReportStatus::Complete => {
                if !report.defects.is_empty() {
                    return Err("complete executor report must not contain defects".into());
                }
                if report
                    .phases
                    .iter()
                    .any(|phase| !matches!(phase.status, PhaseStatus::Done))
                {
                    return Err(
                        "complete executor report requires every phase/step to be done".into(),
                    );
                }
                if report
                    .verifications
                    .iter()
                    .any(|verification| matches!(verification.result, VerificationStatus::Fail))
                {
                    return Err(
                        "complete executor report must not contain failed verifications".into(),
                    );
                }
            }
            ReportStatus::Partial | ReportStatus::Failed if report.defects.is_empty() => {
                return Err(
                    "partial or failed executor report requires at least one defect".into(),
                );
            }
            ReportStatus::Partial | ReportStatus::Failed => {}
        }

        let verify = report
            .verifications
            .into_iter()
            .map(|verification| {
                let _ = verification.output_excerpt;
                VerifyResult {
                    cmd: verification.cmd,
                    claimed: Some(match verification.result {
                        VerificationStatus::Pass => "passed".into(),
                        VerificationStatus::Fail => "failed".into(),
                    }),
                    observed: verification.observed,
                }
            })
            .collect();

        Ok(Self {
            claims: report.claims,
            verify,
        })
    }
}

pub fn parse_file(path: &Path) -> Result<ExecutorReport, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_EXECUTOR_REPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_EXECUTOR_REPORT_BYTES {
        return Err(format!(
            "executor report exceeds {MAX_EXECUTOR_REPORT_BYTES}-byte limit"
        ));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("executor report {} is not UTF-8", path.display()))?;
    parse_str(&text)
}

pub fn parse_str(text: &str) -> Result<ExecutorReport, String> {
    if text.contains(REPORT_BEGIN) {
        let yaml = extract_sentinel_yaml(text, REPORT_BEGIN, REPORT_END)?;
        let canonical = serde_norway::from_str::<CanonicalExecutorReport>(yaml)
            .map_err(|e| format!("parse executor report schema v1 YAML: {e}"))?;
        return canonical.try_into();
    }

    if text.contains(LEGACY_BEGIN) {
        let yaml = extract_sentinel_yaml(text, LEGACY_BEGIN, LEGACY_END)?;
        return parse_legacy(yaml);
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("executor report is empty".into());
    }
    let value: serde_norway::Value =
        serde_norway::from_str(trimmed).map_err(|e| format!("parse executor report YAML: {e}"))?;
    let canonical = value
        .as_mapping()
        .is_some_and(|mapping| mapping.contains_key("schema_version"));
    if canonical {
        let report = serde_norway::from_value::<CanonicalExecutorReport>(value)
            .map_err(|e| format!("parse executor report schema v1 YAML: {e}"))?;
        report.try_into()
    } else {
        parse_legacy(trimmed)
    }
}

fn parse_legacy(yaml: &str) -> Result<ExecutorReport, String> {
    serde_norway::from_str::<ExecutorReport>(yaml)
        .map_err(|e| format!("parse legacy executor report YAML: {e}"))
}

fn extract_sentinel_yaml<'a>(text: &'a str, begin: &str, end: &str) -> Result<&'a str, String> {
    let begin_pos = text
        .find(begin)
        .ok_or_else(|| format!("executor report missing {begin} sentinel"))?;
    let after_begin = &text[begin_pos + begin.len()..];
    let fence_pos = after_begin
        .find("```yaml")
        .ok_or_else(|| "executor report sentinel is missing a yaml fence".to_string())?;
    let yaml_start = begin_pos + begin.len() + fence_pos + "```yaml".len();
    let yaml = &text[yaml_start..];
    let yaml = yaml
        .strip_prefix('\r')
        .unwrap_or(yaml)
        .strip_prefix('\n')
        .unwrap_or(yaml);
    let fence_end = yaml
        .find("```")
        .ok_or_else(|| "executor report yaml fence is not closed".to_string())?;
    let end_pos = text[yaml_start + fence_end..]
        .find(end)
        .ok_or_else(|| format!("executor report missing {end} sentinel"))?;
    if end_pos < 3 {
        return Err(format!(
            "executor report {end} sentinel precedes the yaml fence"
        ));
    }
    Ok(yaml[..fence_end].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical_yaml(extra: &str) -> String {
        format!(
            "schema_version: 1\nspec: .mastermind/tasks/001/spec.md\nstatus: complete\nphases:\n  - id: '1.1'\n    status: done\nfiles_modified:\n  - src/lib.rs\nclaims:\n  - kind: integration\n    from: A\n    to: B\ndefects: []\nverifications:\n  - cmd: cargo test\n    result: pass\n    observed:\n      exit_code: 0\n      tests_run: 12\n{extra}"
        )
    }

    #[test]
    fn parses_canonical_report_tail_used_by_executor_agent() {
        let report = parse_str(include_str!("../tests/fixtures/executor-report-v1.md")).unwrap();
        assert_eq!(report.claims.len(), 2);
        assert_eq!(report.verify.len(), 1);
        assert_eq!(report.verify[0].claimed.as_deref(), Some("passed"));
        assert_eq!(
            report.verify[0].observed.as_ref().unwrap().tests_run,
            Some(12)
        );
    }

    #[test]
    fn canonical_report_rejects_unknown_fields_and_versions() {
        let unknown = canonical_yaml("surprise: true\n");
        assert!(parse_str(&unknown).unwrap_err().contains("unknown field"));

        let unsupported = canonical_yaml("").replacen("schema_version: 1", "schema_version: 2", 1);
        assert!(parse_str(&unsupported)
            .unwrap_err()
            .contains("unsupported executor report schema_version 2"));
    }

    #[test]
    fn canonical_report_rejects_contradictory_completion_evidence() {
        let with_defect = canonical_yaml("").replace(
            "defects: []",
            "defects:\n  - kind: implementation_defect\n    phase: plan-1\n    details: failed\n    remediation_hint: retry",
        );
        assert!(parse_str(&with_defect)
            .unwrap_err()
            .contains("complete executor report must not contain defects"));

        let failed_verification = canonical_yaml("").replacen("result: pass", "result: fail", 1);
        assert!(parse_str(&failed_verification)
            .unwrap_err()
            .contains("complete executor report must not contain failed verifications"));

        let pending_step = canonical_yaml("").replacen("status: done", "status: pending", 1);
        assert!(parse_str(&pending_step)
            .unwrap_err()
            .contains("every phase/step to be done"));
    }

    #[test]
    fn canonical_report_rejects_unexplained_failure_and_duplicate_steps() {
        let failed_without_defect =
            canonical_yaml("").replacen("status: complete", "status: failed", 1);
        assert!(parse_str(&failed_without_defect)
            .unwrap_err()
            .contains("requires at least one defect"));

        let duplicate = canonical_yaml("").replace(
            "  - id: '1.1'\n    status: done",
            "  - id: '1.1'\n    status: done\n  - id: '1.1'\n    status: done",
        );
        assert!(parse_str(&duplicate)
            .unwrap_err()
            .contains("phase id is duplicated"));
    }

    #[test]
    fn malformed_canonical_sentinel_does_not_fall_back_to_prose() {
        let malformed = "<!-- mastermind:report-begin -->\nnot yaml\n";
        assert!(parse_str(malformed).unwrap_err().contains("yaml fence"));
    }

    #[test]
    fn parses_legacy_bare_yaml() {
        let yaml = "claims:\n  - kind: function_added\n    symbol: Foo\nverify:\n  - cmd: go test\n    claimed: passed\n";
        let report = parse_str(yaml).unwrap();
        assert_eq!(report.claims.len(), 1);
        assert_eq!(report.verify.len(), 1);
    }

    #[test]
    fn parses_legacy_sentinel_block() {
        let md = "Some prose.\n\n<!-- mastermind:executor-begin -->\n```yaml\nclaims:\n  - kind: integration\n    from: A\n    to: B\n    relation: calls\n```\n<!-- mastermind:executor-end -->\n";
        let report = parse_str(md).unwrap();
        assert_eq!(report.claims.len(), 1);
    }

    #[test]
    fn rejects_empty_report() {
        assert!(parse_str("").unwrap_err().contains("empty"));
    }

    #[test]
    fn parse_file_rejects_reports_over_one_mib_before_yaml_decode() {
        let path = std::env::temp_dir().join(format!(
            "mmcg-executor-report-oversize-{}.md",
            std::process::id()
        ));
        std::fs::write(&path, vec![b'a'; MAX_EXECUTOR_REPORT_BYTES as usize + 1]).unwrap();
        let error = parse_file(&path).unwrap_err();
        assert!(error.contains("1048576-byte limit"));
        std::fs::remove_file(path).ok();
    }
}
