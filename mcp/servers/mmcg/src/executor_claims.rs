//! Check each executor claim once and retain its identity and evidence.

use crate::audit_spec::Finding;
use crate::bounded_fs::{read_regular_file, ReadControl};
use crate::diff::DeclarationChanges;
use crate::executor_report::{Claim, ExecutorReport};
use crate::indexer::{extractor_for_path, parse_baseline_blob, MAX_INDEXABLE_FILE_SIZE};
use crate::queries::{lang_precision, EdgePrecision};
use crate::spec_symbols::{self, Resolved, Scope, Unresolved};
use crate::store::{CallCandidateEvidence, Store, WorkBudget};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Verified,
    Failed,
    Unresolved,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClaimEvidence {
    AddedDeclaration {
        file: String,
        line: u32,
        baseline_oid: String,
    },
    CompatibleCallCandidate {
        from_file: String,
        to_file: String,
        call_line: u32,
        target_line: u32,
        to_path: Option<String>,
        to_type: Option<String>,
        target_kind: Option<String>,
        match_basis: &'static str,
        precision: EdgePrecision,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaimCheck {
    /// Zero-based position in the exact executor claim sequence.
    pub claim_index: usize,
    pub claim: Claim,
    pub status: ClaimStatus,
    pub evidence: Option<ClaimEvidence>,
    pub finding: Option<Finding>,
}

fn unresolved(index: usize, error: Unresolved) -> Finding {
    Finding::ExecutorClaimUnresolved {
        claim_index: Some(index),
        reason: error.reason.into(),
        matches: error.matches,
    }
}

fn unavailable(index: usize, reason: &'static str) -> Finding {
    unresolved(index, Unresolved::unavailable(reason))
}

fn outcome(index: usize, claim: &Claim, result: Result<ClaimEvidence, Finding>) -> ClaimCheck {
    let (status, evidence, finding) = match result {
        Ok(evidence) => (ClaimStatus::Verified, Some(evidence), None),
        Err(finding @ Finding::ExecutorClaimUnresolved { .. }) => {
            (ClaimStatus::Unresolved, None, Some(finding))
        }
        Err(finding) => (ClaimStatus::Failed, None, Some(finding)),
    };
    ClaimCheck {
        claim_index: index,
        claim: claim.clone(),
        status,
        evidence,
        finding,
    }
}

pub(crate) struct Context<'a> {
    pub store: &'a Store,
    pub changes: &'a DeclarationChanges,
    pub repo_root: &'a Path,
    pub baseline_oid: &'a str,
    pub complete: bool,
    pub deadline: Instant,
}

pub(crate) fn evaluate(report: &ExecutorReport, context: &Context<'_>) -> Vec<ClaimCheck> {
    evaluate_with_hook(report, context, || {})
}

fn evaluate_with_hook(
    report: &ExecutorReport,
    context: &Context<'_>,
    after_checks: impl FnOnce(),
) -> Vec<ClaimCheck> {
    let Context {
        store,
        changes,
        complete,
        deadline,
        ..
    } = *context;
    let fail_all = |reason| {
        report
            .claims
            .iter()
            .enumerate()
            .map(|(index, claim)| outcome(index, claim, Err(unavailable(index, reason))))
            .collect()
    };
    if report.claims.len() > 256 {
        return fail_all("claim_limit");
    }
    if !complete {
        return fail_all("diff_incomplete");
    }
    let Ok(before) = store.data_version() else {
        return fail_all("index_version_unavailable");
    };
    let checked = store.with_work_budget(
        WorkBudget {
            deadline: Some(deadline.saturating_duration_since(Instant::now())),
            op_ticks: None,
        },
        || {
            let mut freshness = BTreeMap::new();
            if !report.claims.is_empty() {
                for file in &changes.current_files {
                    if let Err(reason) = current_source(context, file, &mut freshness) {
                        return Ok(fail_all(reason));
                    }
                }
            }
            Ok(report
                .claims
                .iter()
                .enumerate()
                .map(|(index, claim)| {
                    let result = if Instant::now() >= deadline || store.work_interrupted() {
                        Err(unavailable(index, "claim_budget_exhausted"))
                    } else {
                        check(index, claim, context, &mut freshness)
                    };
                    outcome(index, claim, result)
                })
                .collect())
        },
    );
    after_checks();
    match store.data_version() {
        Ok(after) if after != before => fail_all("index_changed"),
        Err(_) => fail_all("index_version_unavailable"),
        _ if Instant::now() >= deadline => fail_all("claim_budget_exhausted"),
        _ => checked.unwrap_or_else(|_| fail_all("claim_budget_exhausted")),
    }
}

fn resolve(store: &Store, name: &str, file: Option<&str>) -> Result<Resolved, Unresolved> {
    spec_symbols::resolve(
        store,
        name,
        &[Scope {
            name,
            file,
            language: None,
        }],
    )
}

fn current_source(
    context: &Context<'_>,
    file: &str,
    checked: &mut BTreeMap<String, Result<(), &'static str>>,
) -> Result<(), &'static str> {
    if let Some(result) = checked.get(file) {
        return *result;
    }
    let result = (|| {
        if Instant::now() >= context.deadline || context.store.work_interrupted() {
            return Err("claim_budget_exhausted");
        }
        let indexed = context
            .store
            .file_content_sha256(file)
            .map_err(|_| "index_content_unavailable")?
            .filter(|hash| !hash.is_empty())
            .ok_or("index_content_unavailable")?;
        let read = read_regular_file(
            context.repo_root,
            Path::new(file),
            MAX_INDEXABLE_FILE_SIZE,
            MAX_INDEXABLE_FILE_SIZE,
            ReadControl {
                deadline: Some(context.deadline),
                interrupted: None,
            },
        )
        .map_err(|_| "current_file_unavailable")?;
        if indexed != crate::hex::encode(&Sha256::digest(&read.bytes)) {
            return Err("index_source_mismatch");
        }
        let extractor = extractor_for_path(Path::new(file)).ok_or("source_language_unavailable")?;
        parse_baseline_blob(file, &read.bytes, extractor.as_ref())
            .map_err(|_| "current_parse_failed")?;
        Ok(())
    })();
    checked.insert(file.into(), result);
    result
}

fn check(
    index: usize,
    claim: &Claim,
    context: &Context<'_>,
    freshness: &mut BTreeMap<String, Result<(), &'static str>>,
) -> Result<ClaimEvidence, Finding> {
    let Context {
        store,
        changes,
        baseline_oid,
        ..
    } = *context;
    match claim {
        Claim::FunctionAdded {
            symbol,
            file,
            signature,
        } => {
            let file = file
                .as_deref()
                .map(spec_symbols::normalize_file)
                .transpose()
                .map_err(|error| unresolved(index, error))?;
            let target = resolve(store, symbol, file.as_deref())
                .map_err(|error| {
                    if error.reason == "missing" {
                        Finding::ClaimedSymbolMissing {
                            symbol: symbol.clone(),
                            file: file.clone(),
                        }
                    } else {
                        unresolved(index, error)
                    }
                })?
                .symbol;
            current_source(context, &target.file_path, freshness)
                .map_err(|reason| unavailable(index, reason))?;
            if !matches!(target.kind.as_str(), "function" | "method" | "constructor") {
                return Err(unavailable(index, "claim_kind_mismatch"));
            }
            if let Some(claimed) = signature {
                if target.signature.as_ref() != Some(claimed) {
                    return Err(Finding::ClaimedSignatureMismatch {
                        symbol: symbol.clone(),
                        file: file.clone(),
                        claimed: claimed.clone(),
                        actual: target.signature,
                    });
                }
            }
            if let Some(reason) = changes.unavailable_additions.get(&target.file_path) {
                return Err(unavailable(index, reason));
            }
            if changes.uncertain_added.contains(&target.id) {
                return Err(unavailable(index, "addition_ambiguous"));
            }
            if !changes.added.contains(&target.id) {
                return Err(Finding::ClaimedSymbolNotAdded {
                    symbol: symbol.clone(),
                    file: file.clone(),
                });
            }
            Ok(ClaimEvidence::AddedDeclaration {
                file: target.file_path,
                line: target.line_start,
                baseline_oid: baseline_oid.into(),
            })
        }
        Claim::Integration {
            from,
            from_file,
            to,
            to_file,
            relation,
        } => {
            if relation.as_deref().unwrap_or("calls") != "calls" {
                return Err(unavailable(index, "relation_unsupported"));
            }
            let target = resolve(store, to, to_file.as_deref())
                .map_err(|error| {
                    if error.reason == "missing" {
                        Finding::HallucinatedSymbol {
                            from_symbol: from.clone(),
                            to_symbol: to.clone(),
                        }
                    } else {
                        unresolved(index, error)
                    }
                })?
                .symbol;
            let source = resolve(store, from, from_file.as_deref())
                .map_err(|error| unresolved(index, error))?
                .symbol;
            for file in [&source.file_path, &target.file_path] {
                current_source(context, file, freshness)
                    .map_err(|reason| unavailable(index, reason))?;
            }
            let source_language = store
                .symbol_language(source.id)
                .map_err(|_| unavailable(index, "symbol_language_unavailable"))?;
            let target_language = store
                .symbol_language(target.id)
                .map_err(|_| unavailable(index, "symbol_language_unavailable"))?;
            if source_language.is_empty() || target_language.is_empty() {
                return Err(unavailable(index, "symbol_language_unavailable"));
            }
            if source_language != target_language {
                return Err(unavailable(index, "cross_language_binding_unavailable"));
            }
            match store
                .call_candidate_evidence(&source, &target, &source_language)
                .map_err(|_| unavailable(index, "call_query_failed"))?
            {
                CallCandidateEvidence::Unique(witness) => {
                    Ok(ClaimEvidence::CompatibleCallCandidate {
                        precision: lang_precision(&source.file_path),
                        from_file: source.file_path,
                        to_file: target.file_path,
                        target_line: target.line_start,
                        call_line: witness.line,
                        to_path: witness.to_path,
                        to_type: witness.to_type,
                        target_kind: witness.target_kind,
                        match_basis: witness.match_basis,
                    })
                }
                CallCandidateEvidence::Unresolved(reason) => Err(unavailable(index, reason)),
                CallCandidateEvidence::Missing => Err(Finding::MissingCallEdge {
                    from_symbol: from.clone(),
                    to_symbol: to.clone(),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parse_blob;
    use std::time::Duration;

    struct Fixture {
        store: Store,
        directory: tempfile::TempDir,
        changes: DeclarationChanges,
    }

    impl Fixture {
        fn new(files: &[(&str, &str)]) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let mut store = Store::open(directory.path().join("graph.db")).unwrap();
            for (path, source) in files {
                std::fs::write(directory.path().join(path), source).unwrap();
                let extractor = extractor_for_path(Path::new(path)).unwrap();
                store
                    .commit_file(
                        parse_blob(path, source.as_bytes(), 0, extractor.as_ref()).unwrap(),
                    )
                    .unwrap();
            }
            Self {
                store,
                directory,
                changes: DeclarationChanges::default(),
            }
        }

        fn context(&self) -> Context<'_> {
            Context {
                store: &self.store,
                changes: &self.changes,
                repo_root: self.directory.path(),
                baseline_oid: "1111111111111111111111111111111111111111",
                complete: true,
                deadline: Instant::now() + Duration::from_secs(30),
            }
        }

        fn external(&self) -> rusqlite::Connection {
            rusqlite::Connection::open(self.directory.path().join("graph.db")).unwrap()
        }
    }

    fn integration(file: &str, target: &str) -> ExecutorReport {
        ExecutorReport {
            claims: vec![Claim::Integration {
                from: "caller".into(),
                from_file: Some(file.into()),
                to: target.into(),
                to_file: Some(file.into()),
                relation: Some("calls".into()),
            }],
            verify: vec![],
            canonical: None,
        }
    }

    fn assert_unresolved(checks: &[ClaimCheck], reason: &str) {
        assert!(!checks.is_empty());
        for (index, check) in checks.iter().enumerate() {
            assert_eq!(check.claim_index, index);
            assert_eq!(check.status, ClaimStatus::Unresolved, "{check:?}");
            assert!(check.evidence.is_none());
            assert!(
                matches!(&check.finding, Some(Finding::ExecutorClaimUnresolved { claim_index: Some(position), reason: actual, .. }) if actual == reason && *position == index),
                "{check:?}"
            );
        }
    }

    #[test]
    fn executor_claim_sql_failures_and_invalid_rows_are_not_missing_or_verified() {
        for (sql, reason) in [
            ("DROP TABLE edges", "call_query_failed"),
            (
                "UPDATE edges SET line = 'invalid' WHERE kind = 'calls'",
                "call_query_failed",
            ),
            ("DROP TABLE symbols", "symbol_query_failed"),
            (
                "UPDATE symbols SET language = ''",
                "symbol_language_unavailable",
            ),
            (
                "UPDATE files SET content_sha256 = ''",
                "index_content_unavailable",
            ),
        ] {
            let fixture =
                Fixture::new(&[("service.py", "def send(): pass\ndef caller(): send()\n")]);
            fixture.external().execute_batch(sql).unwrap();
            assert_unresolved(
                &evaluate(&integration("service.py", "send"), &fixture.context()),
                reason,
            );
        }
    }

    #[test]
    fn executor_index_replacement_invalidates_every_checked_claim() {
        let fixture = Fixture::new(&[("service.py", "def send(): pass\ndef caller(): send()\n")]);
        let mut report = integration("service.py", "send");
        report.claims.push(report.claims[0].clone());
        assert!(evaluate(&report, &fixture.context())
            .iter()
            .all(|check| check.status == ClaimStatus::Verified));
        let checks = evaluate_with_hook(&report, &fixture.context(), || {
            fixture
                .external()
                .execute(
                    "UPDATE symbols SET signature = 'changed' WHERE name = 'send'",
                    [],
                )
                .unwrap();
        });
        assert_unresolved(&checks, "index_changed");
    }

    #[test]
    fn executor_claim_budgets_incomplete_diff_and_call_caps_fail_closed() {
        let fixture = Fixture::new(&[("service.py", "def send(): pass\ndef caller(): send()\n")]);
        let report = integration("service.py", "send");
        let mut context = fixture.context();
        context.complete = false;
        assert_unresolved(&evaluate(&report, &context), "diff_incomplete");
        context.complete = true;
        context.deadline = Instant::now() - Duration::from_secs(1);
        assert_unresolved(&evaluate(&report, &context), "claim_budget_exhausted");
        let oversized = ExecutorReport {
            claims: vec![report.claims[0].clone(); 257],
            verify: vec![],
            canonical: None,
        };
        fixture
            .external()
            .execute_batch("DROP TABLE symbols")
            .unwrap();
        assert_unresolved(&evaluate(&oversized, &fixture.context()), "claim_limit");

        let source = format!(
            "struct B; impl B {{ fn send() {{}} }} fn caller() {{ {} B::send(); }}\n",
            "Alias::send();\n".repeat(128)
        );
        let fixture = Fixture::new(&[("service.rs", &source)]);
        assert_unresolved(
            &evaluate(&integration("service.rs", "B::send"), &fixture.context()),
            "call_candidate_limit",
        );
    }

    #[test]
    fn executor_global_candidate_limit_does_not_narrow_to_the_requested_file() {
        let mut fixture = Fixture::new(&[(
            "service.rs",
            "struct B; impl B { fn send() {} } fn caller() { B::send(); }\n",
        )]);
        for index in 0..128 {
            let path = format!("other{index}.rs");
            let source = b"struct C; impl C { fn send() {} }\n";
            std::fs::write(fixture.directory.path().join(&path), source).unwrap();
            let extractor = extractor_for_path(Path::new(&path)).unwrap();
            fixture
                .store
                .commit_file(parse_blob(&path, source, 0, extractor.as_ref()).unwrap())
                .unwrap();
        }
        assert_unresolved(
            &evaluate(&integration("service.rs", "B::send"), &fixture.context()),
            "call_target_limit",
        );
    }

    #[test]
    fn executor_candidate_parent_corruption_cannot_create_a_unique_witness() {
        for mutation in [
            "NULL",
            "id",
            "(SELECT id FROM symbols WHERE file_path = 'service.rs' AND kind = 'impl' LIMIT 1)",
        ] {
            let fixture = Fixture::new(&[
                (
                    "service.rs",
                    "struct B; impl B { fn send() {} } fn caller() { B::send(); }\n",
                ),
                ("other.rs", "struct B; impl B { fn send() {} }\n"),
            ]);
            let report = integration("service.rs", "B::send");
            assert_unresolved(
                &evaluate(&report, &fixture.context()),
                "call_target_ambiguous",
            );
            fixture.external().execute_batch(&format!(
                "UPDATE symbols SET parent_id = {mutation} WHERE file_path = 'other.rs' AND name = 'send'"
            )).unwrap();
            assert_unresolved(
                &evaluate(&report, &fixture.context()),
                "call_target_parent_invalid",
            );
        }
    }
}
