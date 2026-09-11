use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

const REPORT_BEGIN: &str = "mastermind:report-begin";
const REPORT_END: &str = "mastermind:report-end";
const LEGACY_BEGIN: &str = "mastermind:executor-begin";
const LEGACY_END: &str = "mastermind:executor-end";
const MAX_EXECUTOR_REPORT_BYTES: u64 = 1024 * 1024;

/// A single claim an executor made in its schema-v1 structured report tail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Claim {
    FunctionAdded {
        symbol: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    Integration {
        from: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_file: Option<String>,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_file: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        relation: Option<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedOutcome {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_run: Option<u32>,
}

/// Internal projection consumed by the deterministic audit checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyResult {
    pub cmd: String,
    #[serde(default)]
    pub claimed: Option<String>,
    #[serde(default)]
    pub observed: Option<ObservedOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_excerpt: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<CanonicalMetadata>,
    #[serde(default)]
    pub claims: Vec<Claim>,
    #[serde(default)]
    pub verify: Vec<VerifyResult>,
}

impl ExecutorReport {
    pub fn is_empty(&self) -> bool {
        self.canonical.is_none() && self.claims.is_empty() && self.verify.is_empty()
    }

    pub(crate) fn completion_rejection(&self) -> Option<&'static str> {
        let metadata = self.canonical.as_ref()?;
        match metadata.status {
            ReportStatus::Partial => Some("status_partial"),
            ReportStatus::Failed => Some("status_failed"),
            ReportStatus::Complete
                if !metadata.defects.is_empty()
                    || metadata
                        .phases
                        .iter()
                        .any(|phase| phase.status != PhaseStatus::Done)
                    || self
                        .verify
                        .iter()
                        .any(|verification| verification.claimed.as_deref() != Some("passed")) =>
            {
                Some("completion_inconsistent")
            }
            ReportStatus::Complete => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalMetadata {
    pub schema_version: u32,
    pub spec: String,
    pub status: ReportStatus,
    pub phases: Vec<Phase>,
    pub files_modified: Vec<String>,
    pub defects: Vec<Defect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Complete,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    Done,
    Pending,
    StoppedHere,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phase {
    pub id: String,
    pub status: PhaseStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defect {
    pub kind: String,
    pub phase: String,
    pub details: String,
    pub remediation_hint: String,
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
        for claim in &report.claims {
            let (required, optional) = match claim {
                Claim::FunctionAdded {
                    symbol,
                    file,
                    signature,
                } => (vec![symbol], vec![file, signature]),
                Claim::Integration {
                    from,
                    from_file,
                    to,
                    to_file,
                    relation,
                } => (vec![from, to], vec![from_file, to_file, relation]),
            };
            if required
                .into_iter()
                .chain(optional.into_iter().filter_map(Option::as_ref))
                .any(|text| text.trim().is_empty())
            {
                return Err("executor report claim fields must not be empty".into());
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
            .map(|verification| VerifyResult {
                cmd: verification.cmd,
                claimed: Some(match verification.result {
                    VerificationStatus::Pass => "passed".into(),
                    VerificationStatus::Fail => "failed".into(),
                }),
                observed: verification.observed,
                output_excerpt: verification.output_excerpt,
            })
            .collect();

        Ok(Self {
            canonical: Some(CanonicalMetadata {
                schema_version: report.schema_version,
                spec: report.spec,
                status: report.status,
                phases: report.phases,
                files_modified: report.files_modified,
                defects: report.defects,
            }),
            claims: report.claims,
            verify,
        })
    }
}

fn report_read_error(path: &Path, error: crate::bounded_fs::BoundedReadError) -> String {
    match error {
        crate::bounded_fs::BoundedReadError::TooLarge { .. } => {
            format!("executor report exceeds {MAX_EXECUTOR_REPORT_BYTES}-byte limit")
        }
        crate::bounded_fs::BoundedReadError::SnapshotChanged => {
            "executor report changed while it was being read".into()
        }
        crate::bounded_fs::BoundedReadError::InvalidPath
        | crate::bounded_fs::BoundedReadError::OutsideRoot
        | crate::bounded_fs::BoundedReadError::NotRegular => {
            "executor report must be a regular file".into()
        }
        crate::bounded_fs::BoundedReadError::Interrupted
        | crate::bounded_fs::BoundedReadError::DeadlineExceeded => {
            "executor report read was interrupted".into()
        }
        crate::bounded_fs::BoundedReadError::Io(error) => {
            format!("read {}: {error}", path.display())
        }
    }
}

fn parse_file_bytes(path: &Path, bytes: Vec<u8>) -> Result<ExecutorReport, String> {
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("executor report {} is not UTF-8", path.display()))?;
    parse_str(&text)
}

pub fn parse_file(path: &Path) -> Result<ExecutorReport, String> {
    let (resolved, source) = crate::bounded_fs::read_selected_regular_file(
        path,
        MAX_EXECUTOR_REPORT_BYTES,
        MAX_EXECUTOR_REPORT_BYTES,
        crate::bounded_fs::ReadControl::default(),
    )
    .map_err(|error| report_read_error(path, error))?;
    if path
        .canonicalize()
        .map_err(|error| format!("re-resolve executor report {}: {error}", path.display()))?
        != resolved
    {
        return Err("executor report path changed while it was being read".into());
    }
    parse_file_bytes(path, source.bytes)
}

/// Parse a repository-owned executor report without following links outside
/// the selected repository.
#[doc(hidden)]
pub fn parse_repository_file(repo_root: &Path, path: &Path) -> Result<ExecutorReport, String> {
    let relative = if path.is_absolute() {
        path.strip_prefix(repo_root)
            .map_err(|_| "executor report must be inside the repository".to_string())?
    } else {
        path
    };
    let source = crate::bounded_fs::read_repository_file(
        repo_root,
        relative,
        MAX_EXECUTOR_REPORT_BYTES,
        MAX_EXECUTOR_REPORT_BYTES,
        crate::bounded_fs::ReadControl::default(),
    )
    .map_err(|error| report_read_error(path, error))?;
    parse_file_bytes(path, source.bytes)
}

pub fn parse_str(text: &str) -> Result<ExecutorReport, String> {
    if text.len() as u64 > MAX_EXECUTOR_REPORT_BYTES {
        return Err(format!(
            "executor report exceeds {MAX_EXECUTOR_REPORT_BYTES}-byte limit"
        ));
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("executor report is empty".into());
    }
    let value = serde_norway::from_str::<serde_norway::Value>(trimmed);
    let value = match value {
        Ok(value)
            if value
                .as_mapping()
                .is_some_and(|mapping| mapping.contains_key("schema_version")) =>
        {
            return parse_canonical_value(value);
        }
        Ok(value)
            if value.as_mapping().is_some_and(|mapping| {
                mapping
                    .keys()
                    .all(|key| matches!(key.as_str(), Some("claims" | "verify")))
            }) =>
        {
            return parse_legacy(trimmed);
        }
        other => other,
    };
    let has_marker = |marker: &str| {
        let marker = format!("<!-- {marker} -->");
        text.lines().any(|line| line.trim_end() == marker)
    };
    let canonical_markers = has_marker(REPORT_BEGIN) || has_marker(REPORT_END);
    let legacy_markers = has_marker(LEGACY_BEGIN) || has_marker(LEGACY_END);
    if canonical_markers && legacy_markers {
        return Err("executor report mixes canonical and legacy blocks".into());
    }
    if canonical_markers {
        let yaml = extract_sentinel_yaml(text, REPORT_BEGIN, REPORT_END)?;
        let value = serde_norway::from_str(&yaml)
            .map_err(|e| format!("parse executor report schema v1 YAML: {e}"))?;
        return parse_canonical_value(value);
    }

    if legacy_markers {
        let yaml = extract_sentinel_yaml(text, LEGACY_BEGIN, LEGACY_END)?;
        return parse_legacy(&yaml);
    }

    value.map_err(|e| format!("parse executor report YAML: {e}"))?;
    parse_legacy(trimmed)
}

pub fn parse_canonical_file(path: &Path) -> Result<ExecutorReport, String> {
    require_canonical(parse_file(path)?)
}

#[doc(hidden)]
pub fn parse_canonical_repository_file(
    repo_root: &Path,
    path: &Path,
) -> Result<ExecutorReport, String> {
    require_canonical(parse_repository_file(repo_root, path)?)
}

pub fn parse_canonical_str(text: &str) -> Result<ExecutorReport, String> {
    require_canonical(parse_str(text)?)
}

fn require_canonical(report: ExecutorReport) -> Result<ExecutorReport, String> {
    if report.canonical.is_none() {
        return Err(
            "canonical executor report schema v1 is required; legacy report supplied".into(),
        );
    }
    Ok(report)
}

fn parse_canonical_value(value: serde_norway::Value) -> Result<ExecutorReport, String> {
    let report = serde_json::from_value::<CanonicalExecutorReport>(canonical_json_value(value, 0)?)
        .map_err(|e| format!("parse executor report schema v1: {e}"))?;
    report.try_into()
}

fn canonical_json_value(
    value: serde_norway::Value,
    depth: usize,
) -> Result<serde_json::Value, String> {
    use serde_norway::Value;
    if depth > 64 {
        return Err("executor report nesting limit exceeded".into());
    }
    Ok(match value {
        Value::Null => return Err("canonical executor report does not permit null values".into()),
        Value::Bool(value) => serde_json::Value::Bool(value),
        Value::Number(value) => {
            let value = serde_json::to_value(value).map_err(|e| e.to_string())?;
            if !value.is_number() {
                return Err("canonical executor report requires finite numbers".into());
            }
            value
        }
        Value::String(value) => serde_json::Value::String(value),
        Value::Sequence(values) => serde_json::Value::Array(
            values
                .into_iter()
                .map(|value| canonical_json_value(value, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Value::Mapping(values) => {
            let mut object = serde_json::Map::new();
            for (key, value) in values {
                let Value::String(key) = key else {
                    return Err("canonical executor report requires string mapping keys".into());
                };
                object.insert(key, canonical_json_value(value, depth + 1)?);
            }
            serde_json::Value::Object(object)
        }
        Value::Tagged(_) => {
            return Err("canonical executor report does not permit YAML tags".into())
        }
    })
}

fn parse_legacy(yaml: &str) -> Result<ExecutorReport, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyReport {
        #[serde(default)]
        claims: Vec<Claim>,
        #[serde(default)]
        verify: Vec<VerifyResult>,
    }
    let report = serde_norway::from_str::<LegacyReport>(yaml)
        .map_err(|e| format!("parse legacy executor report YAML: {e}"))?;
    Ok(ExecutorReport {
        canonical: None,
        claims: report.claims,
        verify: report.verify,
    })
}

fn extract_sentinel_yaml(text: &str, begin: &str, end: &str) -> Result<String, String> {
    let lines: Vec<_> = text.lines().collect();
    let positions = |marker: &str| {
        let marker = format!("<!-- {marker} -->");
        lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| (line.trim_end() == marker).then_some(index))
            .collect::<Vec<_>>()
    };
    let (starts, ends) = (positions(begin), positions(end));
    if starts.len() != 1 || ends.len() != 1 || starts[0] >= ends[0] {
        return Err(
            "executor report requires one ordered sentinel pair enclosing a yaml fence".into(),
        );
    }
    let mut span = &lines[starts[0] + 1..ends[0]];
    while span.first().is_some_and(|line| line.trim().is_empty()) {
        span = &span[1..];
    }
    while span.last().is_some_and(|line| line.trim().is_empty()) {
        span = &span[..span.len() - 1];
    }
    if span.len() < 2 || span[0].trim() != "```yaml" || span[span.len() - 1].trim() != "```" {
        return Err("executor report sentinel span must contain one complete yaml fence".into());
    }
    Ok(span[1..span.len() - 1].join("\n"))
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

    #[cfg(unix)]
    #[test]
    fn parse_file_rejects_a_fifo_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("executor-report.md");
        let raw = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `raw` is a live, NUL-terminated path buffer.
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);

        assert!(parse_file(&path).unwrap_err().contains("regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn repository_parser_rejects_a_linked_report() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_report = outside.path().join("executor-report.md");
        std::fs::write(&outside_report, canonical_yaml("")).unwrap();
        let task = root.path().join(".mastermind/tasks/001-linked");
        std::fs::create_dir_all(&task).unwrap();
        let linked_report = task.join("executor-report.md");
        symlink(&outside_report, &linked_report).unwrap();

        let error = parse_repository_file(root.path(), &linked_report).unwrap_err();
        assert!(error.contains("regular file"));
    }
}
