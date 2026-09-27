//! A narrow retry decision from observed checks and one completed audit.
//!
//! This does not infer the root cause of a nonzero exit or certify test coverage.
//! It authorizes another bounded attempt inside the unchanged approved contract.

use crate::audit_spec::{Finding, Report, Verdict};
use crate::executor_report::ReportStatus;
use crate::spec::{ParsedSpec, VerifyEntry};
use crate::verification_receipts::{Binding, CheckEvidence, CheckEvidenceStatus};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(crate) const MAX_ITERATIONS: u32 = 20;

/// Only controller-generated identifiers and digests enter the next prompt.
/// Free-form executor defects, excerpts and audit prose stay out of this packet.
#[derive(Debug, Serialize)]
pub(crate) struct Feedback {
    pub schema_version: u32,
    pub source_iteration: u32,
    pub source_binding: Binding,
    pub source_invocation_id: String,
    pub source_inputs_sha256: String,
    pub source_verification_inputs_sha256: String,
    pub source_audit_sha256: String,
    pub max_iterations: u32,
    pub failed_checks: Vec<String>,
    pub unmet_criteria: Vec<String>,
}

pub(crate) fn validate_contract(spec: &ParsedSpec) -> Result<(), &'static str> {
    crate::acceptance::validate(spec).map_err(|_| "auto_repair_contract_invalid")?;
    if crate::acceptance::declared(spec).is_none() {
        return Err("auto_repair_requires_acceptance");
    }
    if spec.frontmatter.as_ref().is_none_or(|fm| {
        fm.verify.is_empty()
            || fm
                .verify
                .iter()
                .any(|entry| !matches!(entry, VerifyEntry::Observed { .. }))
    }) {
        return Err("auto_repair_requires_observed_checks");
    }
    Ok(())
}

pub(crate) fn assess(
    spec: &ParsedSpec,
    audit: &Report,
    checks: &[CheckEvidence],
) -> Result<(Vec<String>, Vec<String>), &'static str> {
    validate_contract(spec)?;
    if audit.verdict == Verdict::Held
        || audit.findings.is_empty()
        || audit
            .symbol_diff
            .as_ref()
            .is_none_or(|diff| diff.truncated || !diff.errors.is_empty())
    {
        return Err("auto_repair_audit_incomplete");
    }
    // inspect_repair_checks validates current snapshots, normal nonzero exits,
    // and executable identity for failures as well as passes.
    if checks.is_empty()
        || checks.iter().any(|check| {
            check.status != CheckEvidenceStatus::Current
                && !(check.status == CheckEvidenceStatus::Failed
                    && check.reason.as_deref() == Some("receipt_failed"))
        })
    {
        return Err("auto_repair_check_requires_review");
    }
    let failed: BTreeSet<_> = checks
        .iter()
        .filter(|check| check.status == CheckEvidenceStatus::Failed)
        .map(|check| check.id.clone())
        .collect();
    if failed.is_empty() {
        return Err("auto_repair_no_observed_failure");
    }
    let report = audit
        .executor_report
        .as_ref()
        .ok_or("auto_repair_report_requires_review")?;
    let metadata = report
        .canonical
        .as_ref()
        .ok_or("auto_repair_report_requires_review")?;
    if metadata.status != ReportStatus::Partial
        || metadata.defects.is_empty()
        || metadata
            .defects
            .iter()
            .any(|defect| defect.kind != "implementation_defect")
    {
        return Err("auto_repair_report_requires_review");
    }
    let mut commands = BTreeSet::new();
    for entry in spec.frontmatter.iter().flat_map(|fm| &fm.verify) {
        let VerifyEntry::Observed { cmd, run } = entry else {
            return Err("auto_repair_contract_invalid");
        };
        let check = checks
            .iter()
            .find(|check| check.id == run.id)
            .ok_or("auto_repair_check_requires_review")?;
        let mut rows = report
            .verify
            .iter()
            .filter(|row| row.cmd.trim() == cmd.trim());
        let row = rows.next();
        if rows.next().is_some() {
            return Err("auto_repair_report_requires_review");
        }
        if let Some(row) = row {
            let exit = row
                .observed
                .as_ref()
                .and_then(|observed| observed.exit_code);
            let agrees = match check.status {
                CheckEvidenceStatus::Current => {
                    row.claimed.as_deref() == Some("passed") && exit == Some(0)
                }
                CheckEvidenceStatus::Failed => {
                    row.claimed.as_deref() == Some("failed")
                        && exit.is_some_and(|code| (1..=255).contains(&code))
                }
                _ => false,
            };
            if !agrees {
                return Err("auto_repair_report_requires_review");
            }
        } else if check.status == CheckEvidenceStatus::Failed {
            return Err("auto_repair_report_requires_review");
        }
        commands.insert(cmd.trim());
    }
    if commands.len() != checks.len()
        || report
            .verify
            .iter()
            .any(|row| !commands.contains(row.cmd.trim()))
    {
        return Err("auto_repair_report_requires_review");
    }
    let acceptance = audit
        .acceptance
        .as_ref()
        .ok_or("auto_repair_contract_invalid")?;
    let unmet: BTreeSet<_> = acceptance
        .criteria
        .iter()
        .filter(|criterion| criterion.status != crate::acceptance::Status::RequirementsSatisfied)
        .map(|criterion| criterion.id.clone())
        .collect();
    let receipt_reason = crate::verification_receipts::current_digests(checks)
        .err()
        .ok_or("auto_repair_no_observed_failure")?;
    let allowed = audit.findings.iter().all(|finding| match finding {
        Finding::AcceptanceCriterionUnmet { id, .. } => unmet.contains(id),
        Finding::VerificationRequirementUnmet { cmd, reason } => {
            (cmd == "observed verification receipts" && reason == &receipt_reason)
                || (commands.contains(cmd.trim())
                    && matches!(reason.as_str(), "not_passed" | "missing_result"))
        }
        Finding::ExecutorReportRejected { reason } => reason == "status_partial",
        Finding::MissingExpectedFile { file } => crate::declared_files::paths(spec)
            .any(|path| crate::declared_files::normalize(path).as_deref() == Ok(file)),
        _ => false,
    });
    if !allowed {
        return Err("auto_repair_audit_requires_review");
    }
    Ok((failed.into_iter().collect(), unmet.into_iter().collect()))
}

impl Feedback {
    pub(crate) fn new(
        audit: &Report,
        invocation: &crate::invocation::InvocationReceipt,
        source_inputs_sha256: String,
        source_verification_inputs_sha256: String,
        max_iterations: u32,
        failed_checks: Vec<String>,
        unmet_criteria: Vec<String>,
    ) -> Result<Self, &'static str> {
        let bytes = serde_json::to_vec(audit).map_err(|_| "auto_repair_audit_unavailable")?;
        Ok(Self {
            schema_version: 1,
            source_iteration: invocation.binding.iteration,
            source_binding: invocation.binding.clone(),
            source_invocation_id: invocation.invocation_id.clone(),
            source_inputs_sha256,
            source_verification_inputs_sha256,
            source_audit_sha256: crate::hex::encode(&Sha256::digest(bytes)),
            max_iterations,
            failed_checks,
            unmet_criteria,
        })
    }
}
