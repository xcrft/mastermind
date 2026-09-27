//! Declared acceptance criteria backed by current observed verification checks.
//!
//! A satisfied requirement reports the declared mapping, not semantic entailment
//! or overall task completion. It never consumes executor-authored pass claims.

use crate::bounded_fs::{self, ReadControl, RootCapability};
use crate::spec::{ParsedSpec, VerifyEntry};
use crate::verification_receipts::{CheckEvidence, CheckEvidenceStatus};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub id: String,
    pub statement: String,
    pub checks: Vec<String>,
}

/// Missing means legacy. Explicit null must not silently weaken the contract.
pub(crate) fn deserialize_contract<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<Criterion>>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<Criterion>::deserialize(deserializer).map(Some)
}

pub(crate) fn declared(spec: &ParsedSpec) -> Option<&[Criterion]> {
    spec.frontmatter.as_ref()?.acceptance.as_deref()
}

pub(crate) fn validate(spec: &ParsedSpec) -> Result<(), String> {
    if spec.frontmatter_error.is_some() {
        return Err("acceptance_spec_invalid".into());
    }
    let Some(criteria) = declared(spec) else {
        return Ok(());
    };
    if criteria.is_empty() || criteria.len() > 64 {
        return Err("acceptance_requires_1_to_64_criteria".into());
    }
    crate::verification_receipts::validate_declarations(spec)?;
    let known: BTreeSet<&str> = spec
        .frontmatter
        .iter()
        .flat_map(|fm| &fm.verify)
        .filter_map(|entry| match entry {
            VerifyEntry::Observed { run, .. } => Some(run.id.as_str()),
            _ => None,
        })
        .collect();
    let mut ids = BTreeSet::new();
    for criterion in criteria {
        if criterion.id.is_empty()
            || criterion.id.len() > 64
            || !criterion
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
            || !ids.insert(&criterion.id)
        {
            return Err("acceptance_ids_must_be_unique_ascii_identifiers".into());
        }
        if criterion.statement.trim().is_empty()
            || criterion.statement.len() > 2048
            || criterion.statement.chars().any(char::is_control)
            || (criterion.statement.trim().starts_with('<')
                && criterion.statement.trim().ends_with('>'))
        {
            return Err(format!(
                "{}: acceptance_statement_invalid_or_placeholder",
                criterion.id
            ));
        }
        let unique: BTreeSet<&str> = criterion.checks.iter().map(String::as_str).collect();
        if criterion.checks.is_empty()
            || criterion.checks.len() > 32
            || unique.len() != criterion.checks.len()
            || unique.iter().any(|id| !known.contains(id))
        {
            return Err(format!(
                "{}: acceptance_requires_unique_observed_check_ids",
                criterion.id
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    NotDeclared,
    RequirementsSatisfied,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CriterionResult {
    pub id: String,
    pub statement: String,
    pub status: Status,
    pub checks: Vec<CheckEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    schema_version: u32,
    repository_content_untrusted: bool,
    pub(crate) status: Status,
    pub(crate) criteria: Vec<CriterionResult>,
    declared: usize,
    satisfied: usize,
    evidence_basis: &'static str,
    semantic_accuracy: &'static str,
    overall_task_completion: &'static str,
    precision_notes: Vec<&'static str>,
}

impl Report {
    pub fn requirements_satisfied(&self) -> bool {
        self.status == Status::RequirementsSatisfied
    }

    pub fn render_text(&self) -> String {
        let mut text = format!(
            "Acceptance: {:?} ({}/{})\n",
            self.status, self.satisfied, self.declared
        );
        for criterion in &self.criteria {
            text.push_str(&format!(
                "  {}: {:?} — {}\n",
                criterion.id,
                criterion.status,
                crate::terminal::escape(&criterion.statement)
            ));
            for check in &criterion.checks {
                text.push_str(&format!(
                    "    {}: {:?}{}\n",
                    check.id,
                    check.status,
                    check
                        .reason
                        .as_ref()
                        .map(|reason| format!(" ({})", crate::terminal::escape(reason)))
                        .unwrap_or_default()
                ));
            }
        }
        text.push_str("This checks declared evidence requirements; semantic accuracy and overall task completion are not established here.\n");
        text
    }
}

pub(crate) fn evaluate(
    spec: &ParsedSpec,
    observations: &Result<Vec<CheckEvidence>, String>,
) -> Result<Report, String> {
    validate(spec)?;
    let mut report = Report {
        schema_version: 1,
        repository_content_untrusted: true,
        status: Status::NotDeclared,
        criteria: Vec::new(),
        declared: 0,
        satisfied: 0,
        evidence_basis: "current_local_execution_records",
        semantic_accuracy: "unknown",
        overall_task_completion: "not_evaluated",
        precision_notes: vec![
            "All checks listed for each criterion are required. No executor pass claim or Markdown checkbox satisfies a requirement.",
            "The criterion-to-check mapping is declared in the approved spec; semantic coverage is not mechanically established.",
            "A check shared by multiple criteria is the same observation, not independent evidence.",
            "Local owner-writable records are not independent attestations. Other task gates still apply.",
        ],
    };
    let Some(criteria) = declared(spec) else {
        return Ok(report);
    };
    let by_id: BTreeMap<&str, &CheckEvidence> = observations
        .as_ref()
        .into_iter()
        .flatten()
        .map(|check| (check.id.as_str(), check))
        .collect();
    for criterion in criteria {
        let checks: Vec<CheckEvidence> = criterion
            .checks
            .iter()
            .map(|id| {
                by_id
                    .get(id.as_str())
                    .map(|check| (*check).clone())
                    .unwrap_or_else(|| CheckEvidence {
                        id: id.clone(),
                        status: CheckEvidenceStatus::Unavailable,
                        receipt_revision: None,
                        run_id: None,
                        reason: Some("verification_unavailable".into()),
                    })
            })
            .collect();
        let status = if checks
            .iter()
            .all(|check| check.status == CheckEvidenceStatus::Current)
        {
            report.satisfied += 1;
            Status::RequirementsSatisfied
        } else {
            Status::Blocked
        };
        report.criteria.push(CriterionResult {
            id: criterion.id.clone(),
            statement: criterion.statement.clone(),
            status,
            checks,
        });
    }
    report.declared = report.criteria.len();
    report.status = if report.satisfied == report.declared {
        Status::RequirementsSatisfied
    } else {
        Status::Blocked
    };
    Ok(report)
}

/// Read-only, no index or model required. An unavailable preflight cannot pass.
pub fn inspect(spec_path: &Path, repo_root: &Path) -> Result<Report, String> {
    let root = RootCapability::open(repo_root).map_err(|_| "acceptance_root_unavailable")?;
    let spec = crate::spec::parse_repository_file(repo_root, spec_path)
        .map_err(|_| "acceptance_spec_unavailable")?;
    validate(&spec)?;
    if declared(&spec).is_none() {
        return evaluate(&spec, &Ok(Vec::new()));
    }
    let deadline = Instant::now() + crate::diff::git_timeout();
    let observations = (|| {
        let path = crate::run_task::state_file_path(repo_root, spec_path);
        let file = bounded_fs::read_regular_file_with_capability(
            &root,
            &path,
            128 * 1024,
            128 * 1024,
            ReadControl {
                deadline: Some(deadline),
                interrupted: None,
            },
        )
        .map_err(|_| "acceptance_preflight_unavailable")?;
        let state = crate::run_task::parse_run_state(&file.bytes)?;
        crate::verification_receipts::inspect_checks(
            &spec,
            repo_root,
            &state.baseline_ref,
            deadline,
        )
    })();
    root.verify().map_err(|_| "acceptance_root_changed")?;
    evaluate(&spec, &observations)
}
