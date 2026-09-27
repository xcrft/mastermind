//! Narrow retry decisions from observed checks or an explicit semantic review.
//!
//! This does not infer the root cause of a nonzero exit or certify test coverage.
//! It authorizes another bounded attempt inside the unchanged approved contract.

use crate::audit_spec::{Finding, Report, Verdict};
use crate::executor_report::ReportStatus;
use crate::spec::{ParsedSpec, VerifyEntry};
use crate::verification_receipts::{Binding, CheckEvidence, CheckEvidenceStatus};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

pub(crate) const MAX_ITERATIONS: u32 = 20;

/// Mechanical feedback contains controller-generated identifiers and digests.
/// Semantic feedback adds explicitly untrusted, source-bound reviewer assertions;
/// free-form executor defects, excerpts and audit prose stay out of both packets.
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_review: Option<SemanticFeedback>,
}

/// Reviewer assertions stay distinct from observed command failures. These are
/// immutable source pins for one controller-owned retry, not a new approval.
#[derive(Debug, Serialize)]
pub(crate) struct SemanticFeedback {
    pub kind: &'static str,
    pub repository_content_untrusted: bool,
    pub semantic_accuracy: &'static str,
    pub working_directory: String,
    pub review_revision: String,
    pub target_revision: String,
    pub source_feedback_sha256: String,
    pub criteria: Vec<crate::task_review::CriterionAssessment>,
    pub target: crate::task_review::Target,
    pub source_files: BTreeMap<String, String>,
    pub check_executables: BTreeMap<String, String>,
    pub native_executable: crate::verification_receipts::Executable,
}

pub(crate) fn assess_semantic(
    packet: &crate::task_review::FollowUp,
) -> Result<Vec<String>, &'static str> {
    use crate::history_disposition::Decision;
    use crate::task_review::{FollowUpAction, Judgment};
    if packet.next_action != FollowUpAction::ReviseSolution
        || packet.review.criteria.is_empty()
        || packet.review.criteria.iter().any(|item| {
            item.status == Judgment::Unknown
                || (item.status == Judgment::Unsatisfied
                    && (item.reason.trim().is_empty() || item.evidence.is_empty()))
        })
        || [
            &packet.review.verification_quality,
            &packet.review.scope_control,
            &packet.review.proportionality,
        ]
        .iter()
        .any(|item| item.status != Judgment::Satisfied)
    {
        return Err("auto_follow_up_review_requires_inspection");
    }
    if packet.review.history.as_ref().is_none_or(|history| {
        history.context.decision != Decision::NoChange
            || history.lessons.decision != Decision::NoChange
    }) {
        return Err("auto_follow_up_history_requires_inspection");
    }
    let failed: Vec<_> = packet
        .review
        .criteria
        .iter()
        .filter(|item| item.status == Judgment::Unsatisfied)
        .map(|item| item.id.clone())
        .collect();
    if failed.is_empty() {
        return Err("auto_follow_up_no_concrete_criterion");
    }
    Ok(failed)
}

fn digest(bytes: &[u8]) -> String {
    crate::hex::encode(&Sha256::digest(bytes))
}

fn source_bytes(
    root: &crate::bounded_fs::RootCapability,
    path: &Path,
    remaining: &mut u64,
) -> Result<Vec<u8>, &'static str> {
    let file = crate::bounded_fs::read_regular_file_with_capability(
        root,
        path,
        4 * 1024 * 1024,
        (*remaining).min(4 * 1024 * 1024),
        crate::bounded_fs::ReadControl {
            deadline: Some(Instant::now() + crate::diff::git_timeout()),
            interrupted: None,
        },
    )
    .map_err(|_| "auto_follow_up_source_unavailable")?;
    if file.bytes.len() as u64 != file.declared_len {
        return Err("auto_follow_up_source_incomplete");
    }
    *remaining = remaining
        .checked_sub(file.bytes.len() as u64)
        .ok_or("auto_follow_up_source_limit")?;
    Ok(file.bytes)
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
            semantic_review: None,
        })
    }

    pub(crate) fn new_semantic(
        audit: &Report,
        invocation: &crate::invocation::InvocationReceipt,
        source_inputs_sha256: String,
        packet: crate::task_review::FollowUp,
        check_executables: BTreeMap<String, String>,
        max_iterations: u32,
    ) -> Result<Self, &'static str> {
        let unmet = assess_semantic(&packet)?;
        if packet.target.binding != invocation.binding
            || packet.working_directory != invocation.root
        {
            return Err("auto_follow_up_binding_mismatch");
        }
        let root = crate::bounded_fs::RootCapability::open(Path::new(&packet.working_directory))
            .map_err(|_| "auto_follow_up_root_unavailable")?;
        let spec = Path::new(&packet.target.binding.spec_path);
        let mut source_files = BTreeMap::new();
        for evidence in packet.target.evidence.values() {
            if let Some(path) = &evidence.path {
                if source_files
                    .insert(path.clone(), evidence.sha256.clone())
                    .is_some_and(|old| old != evidence.sha256)
                {
                    return Err("auto_follow_up_evidence_ambiguous");
                }
            }
        }
        let review_path = crate::task_review::record_path(root.canonical_root(), spec);
        let mut remaining = 64 * 1024 * 1024;
        let review = source_bytes(&root, &review_path, &mut remaining)?;
        if digest(&review) != packet.review_revision {
            return Err("auto_follow_up_review_changed");
        }
        // follow_up has validated the native receipt's content and binding.
        // Retain its exact pin before run_pre clears the active review pin.
        let value = crate::setup::parse_json_unique(&review)
            .map_err(|_| "auto_follow_up_review_unavailable")?;
        let native_pin = value["native_invocation"]["receipt_sha256"]
            .as_str()
            .filter(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("auto_follow_up_native_review_required")?;
        for (path, pin) in [
            (review_path, packet.review_revision.as_str()),
            (
                crate::review_invocation::receipt_path(root.canonical_root(), spec),
                native_pin,
            ),
        ] {
            let relative = root
                .repository_relative(&path)
                .and_then(|path| crate::bounded_fs::normalize_repository_relative_path(&path))
                .map_err(|_| "auto_follow_up_path_invalid")?;
            source_files.insert(relative, pin.to_owned());
        }
        let mut feedback = Self::new(
            audit,
            invocation,
            source_inputs_sha256,
            packet.target.verification_inputs_sha256.clone(),
            max_iterations,
            Vec::new(),
            unmet,
        )?;
        let mut semantic = SemanticFeedback {
            kind: "semantic_follow_up",
            repository_content_untrusted: true,
            semantic_accuracy: "reviewer_assertion_not_independently_verified",
            working_directory: packet.working_directory,
            review_revision: packet.review_revision,
            target_revision: packet.target_revision,
            source_feedback_sha256: String::new(),
            criteria: packet
                .review
                .criteria
                .into_iter()
                .filter(|item| item.status == crate::task_review::Judgment::Unsatisfied)
                .collect(),
            target: packet.target,
            source_files,
            check_executables,
            native_executable: invocation
                .executable
                .clone()
                .ok_or("auto_follow_up_native_executable_unavailable")?,
        };
        semantic.source_feedback_sha256 = digest(
            &serde_json::to_vec(&("mastermind-semantic-follow-up-v1", &semantic))
                .map_err(|_| "auto_follow_up_serialization_failed")?,
        );
        feedback.semantic_review = Some(semantic);
        if serde_json::to_vec(&feedback)
            .map_err(|_| "auto_follow_up_serialization_failed")?
            .len()
            > 32 * 1024
        {
            return Err("auto_follow_up_feedback_limit");
        }
        feedback.semantic_sources_current(root.canonical_root())?;
        Ok(feedback)
    }

    /// Independent of the new iteration's review pin: the source record and its
    /// references must stay unchanged until the next executor receives them.
    pub(crate) fn semantic_sources_current(&self, repo: &Path) -> Result<(), &'static str> {
        let Some(semantic) = &self.semantic_review else {
            return Ok(());
        };
        let root = crate::bounded_fs::RootCapability::open(repo)
            .map_err(|_| "auto_follow_up_root_unavailable")?;
        if root.canonical_root() != Path::new(&semantic.working_directory) {
            return Err("auto_follow_up_root_changed");
        }
        let mut remaining = 64 * 1024 * 1024;
        for (path, expected) in &semantic.source_files {
            if digest(&source_bytes(&root, Path::new(path), &mut remaining)?) != *expected {
                return Err("auto_follow_up_evidence_changed");
            }
        }
        if semantic.target.project_history.as_ref()
            != Some(
                &crate::history_disposition::capture(&root)
                    .map_err(|_| "auto_follow_up_history_unavailable")?,
            )
        {
            return Err("auto_follow_up_history_changed");
        }
        let spec_path = Path::new(&self.source_binding.spec_path);
        let body = source_bytes(&root, spec_path, &mut remaining)?;
        let text = std::str::from_utf8(&body).map_err(|_| "auto_follow_up_spec_invalid")?;
        let parsed = crate::spec::parse_str(&self.source_binding.spec_path, text);
        let current = crate::verification_receipts::repair_executable_revisions(
            &parsed,
            root.canonical_root(),
            &self.source_binding.baseline_oid,
            Instant::now() + crate::diff::git_timeout(),
        )
        .map_err(|_| "auto_follow_up_check_executable_unavailable")?;
        if current != semantic.check_executables {
            return Err("auto_follow_up_check_executable_changed");
        }
        root.verify().map_err(|_| "auto_follow_up_root_changed")
    }
}
