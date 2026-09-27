//! Atomic transfer from an exact inbox revision to an unreviewed habit.

use super::{collection, CollectedCandidate, NewHabit, NewHabitEvidence, ProfileStore, SqlResult};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct CandidateHabitReceipt {
    pub candidate_id: String,
    pub revision: String,
    pub habit_id: i64,
    pub evidence_id: i64,
    pub status: String,
    pub repeated: bool,
}

fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

impl ProfileStore {
    /// The caller validates source bytes under the global writer lock. Compare
    /// the complete inbox row again in SQL, then record claim, evidence and
    /// immutable provenance together. Neither a retry nor collection reviews it.
    pub(crate) fn propose_candidate_habit(
        &mut self,
        candidate: &CollectedCandidate,
        claim: &NewHabit<'_>,
        episode: &str,
        repository: &str,
        request_digest: &str,
        target: Option<i64>,
    ) -> SqlResult<CandidateHabitReceipt> {
        let snapshot = serde_json::to_string(candidate)
            .map_err(|_| invalid("cannot serialize candidate provenance"))?;
        if target.is_some_and(|id| id <= 0)
            || snapshot.len() > 16 * 1024
            || request_digest.len() != 64
            || !request_digest.bytes().all(|c| c.is_ascii_hexdigit())
            || episode.len() > 512
            || repository.len() > 128
            || [
                claim.when,
                claim.behavior,
                claim.outcome,
                claim.exception,
                claim.scope,
                claim.role,
                claim.workflow,
            ]
            .iter()
            .any(|s| s.len() > 800)
        {
            return Err(invalid("habit proposal metadata exceeds its bounds"));
        }
        let tx = self.conn.transaction()?;
        let stored = tx.query_row(
            "SELECT id, source, kind, quote, source_path, line_no, segment_no, record_digest,
                    project_root, project, observed_at, extractor, rule_id, revision, status, present
             FROM persona_candidate WHERE id=?1", [&candidate.id], collection::candidate_row,
        ).optional()?;
        if stored.as_ref() != Some(candidate) || !candidate.present || candidate.status != "pending"
        {
            return Err(invalid(
                "candidate changed, removed or dismissed; inspect it again",
            ));
        }
        let source_matches: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM persona_collection_source WHERE source=?1
                AND project=?2 AND project_root=?3 AND repository=?4)",
            params![
                candidate.source,
                candidate.project,
                candidate.project_root,
                repository
            ],
            |row| row.get(0),
        )?;
        if !source_matches {
            return Err(invalid("candidate source binding changed"));
        }
        let previous: Option<(String, i64, i64, String)> = tx.query_row(
            "SELECT request_digest, claim_id, evidence_id, revision FROM persona_candidate_habit
             WHERE candidate_id=?1 ORDER BY rowid DESC LIMIT 1", [&candidate.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).optional()?;
        if previous.as_ref().is_some_and(|(request, id, _, _)| {
            request != request_digest || target.is_some_and(|target| target != *id)
        }) {
            return Err(invalid(
                "candidate already proposed with different fields or episode",
            ));
        }
        if let Some((_, claim_id, evidence_id, old_revision)) = &previous {
            let status: String = tx.query_row(
                "SELECT status FROM persona_claim WHERE id=?1",
                [claim_id],
                |row| row.get(0),
            )?;
            let active: bool = tx.query_row(
                "SELECT status='active' FROM persona_claim_evidence WHERE id=?1 AND claim_id=?2",
                params![evidence_id, claim_id],
                |row| row.get(0),
            )?;
            if matches!(status.as_str(), "rejected" | "superseded")
                || !active
                || super::habit_supersession::has_successor(&tx, *claim_id)?
                || super::habit_generations::has_child(&tx, *claim_id)?
            {
                return Err(invalid(
                    "habit or evidence was rejected or superseded; a proposal cannot restore it",
                ));
            }
            if old_revision == &candidate.revision {
                let same_evidence: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM persona_claim_evidence WHERE id=?1 AND source=?2
                     AND source_path=?3 AND line_no=?4 AND record_digest=?5 AND quote=?6
                     AND episode=?7 AND project=?8 AND repository=?9)",
                    params![
                        evidence_id,
                        candidate.source,
                        candidate.source_path,
                        candidate.line_no as i64,
                        candidate.record_digest,
                        candidate.quote,
                        episode,
                        candidate.project,
                        repository
                    ],
                    |row| row.get(0),
                )?;
                if !same_evidence {
                    return Err(invalid(
                        "habit evidence changed after proposal; inspect it again",
                    ));
                }
                return Ok(CandidateHabitReceipt {
                    candidate_id: candidate.id.clone(),
                    revision: candidate.revision.clone(),
                    habit_id: *claim_id,
                    evidence_id: *evidence_id,
                    status,
                    repeated: true,
                });
            }
        }
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM persona_candidate_habit WHERE candidate_id=?1 AND revision=?2)",
            params![candidate.id, candidate.revision], |row| row.get::<_, bool>(0),
        )? {
            return Err(invalid("an older proposal revision cannot replace a newer binding"));
        }
        let evidence = NewHabitEvidence {
            source: &candidate.source,
            source_path: &candidate.source_path,
            line_no: candidate.line_no as i64,
            record_digest: &candidate.record_digest,
            episode,
            project: &candidate.project,
            repository,
            quote: &candidate.quote,
            at: &candidate.observed_at,
            relation: "supports",
        };
        // A new candidate cannot silently add support to an already reviewed
        // claim. A known candidate may explicitly relocate its own citation;
        // this withdraws the observation until another observe review.
        let selected = previous.as_ref().map(|(_, id, _, _)| *id).or(target);
        let id = super::record_habit_tx(&tx, claim, &evidence, previous.is_none(), selected)?;
        if previous
            .as_ref()
            .is_some_and(|(_, prior_id, _, _)| *prior_id != id)
        {
            return Err(invalid("proposal no longer identifies the original habit"));
        }
        let evidence_id: i64 = tx.query_row(
            "SELECT id FROM persona_claim_evidence WHERE claim_id=?1 AND source=?2
                AND quote=?3 AND relation='supports'",
            params![id, candidate.source, candidate.quote],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO persona_candidate_habit
                (candidate_id, revision, request_digest, claim_id, evidence_id, candidate_json, proposed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())",
            params![candidate.id, candidate.revision, request_digest, id, evidence_id, snapshot],
        )?;
        super::invalidate_habit_observation(&tx, id)?;
        tx.execute(
            "INSERT INTO persona_review_event (claim_id, status, at_epoch) VALUES (?1, ?2, unixepoch())",
            params![id, format!("proposed_from_candidate:{}:{}", candidate.id, candidate.revision)],
        )?;
        let count: i64 =
            tx.query_row("SELECT count(*) FROM persona_candidate_habit", [], |row| {
                row.get(0)
            })?;
        if count > 20_000 {
            return Err(invalid("habit proposal history exceeds 20000 rows"));
        }
        collection::check_size(&tx)?;
        let status = tx.query_row(
            "SELECT status FROM persona_claim WHERE id=?1",
            [id],
            |row| row.get(0),
        )?;
        tx.commit()?;
        Ok(CandidateHabitReceipt {
            candidate_id: candidate.id.clone(),
            revision: candidate.revision.clone(),
            habit_id: id,
            evidence_id,
            status,
            repeated: false,
        })
    }

    /// The latest explicit transfer supplies the verification mode and exact
    /// immutable locator. Later manual re-citation cannot downgrade this mode.
    pub(crate) fn habit_candidate_binding(
        &self,
        evidence_id: i64,
    ) -> SqlResult<Option<CollectedCandidate>> {
        if !self.has_table("persona_candidate_habit")? {
            return Ok(None);
        }
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT candidate_json FROM persona_candidate_habit WHERE evidence_id=?1
             ORDER BY rowid DESC LIMIT 1",
                [evidence_id],
                |row| row.get(0),
            )
            .optional()?;
        raw.map(|raw| {
            if raw.len() > 16 * 1024 {
                return Err(invalid("candidate provenance exceeds its bound"));
            }
            serde_json::from_str(&raw).map_err(|_| invalid("invalid candidate provenance"))
        })
        .transpose()
    }

    pub(crate) fn candidate_habit_history(
        &self,
        candidate: Option<&str>,
        habit: Option<i64>,
    ) -> SqlResult<serde_json::Value> {
        if !self.has_table("persona_candidate_habit")? {
            return Ok(serde_json::json!({"items":[], "truncated":false}));
        }
        let mut stmt = self.conn.prepare(
            "SELECT p.candidate_id, p.revision, p.claim_id, p.evidence_id, p.proposed_at,
                    c.status, e.status
             FROM persona_candidate_habit p JOIN persona_claim c ON c.id=p.claim_id
             JOIN persona_claim_evidence e ON e.id=p.evidence_id
             WHERE (?1 IS NULL OR p.candidate_id=?1) AND (?2 IS NULL OR p.claim_id=?2)
             ORDER BY p.rowid DESC LIMIT 21",
        )?;
        let mut items: Vec<serde_json::Value> = stmt.query_map(params![candidate, habit], |row| {
            Ok(serde_json::json!({"candidate_id":row.get::<_, String>(0)?, "revision":row.get::<_, String>(1)?,
                "habit_id":row.get::<_, i64>(2)?, "evidence_id":row.get::<_, i64>(3)?,
                "proposed_at":row.get::<_, i64>(4)?, "habit_status":row.get::<_, String>(5)?,
                "evidence_status":row.get::<_, String>(6)?}))
        })?.collect::<SqlResult<_>>()?;
        let truncated = items.len() > 20;
        items.truncate(20);
        Ok(serde_json::json!({"items":items, "truncated":truncated}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::miner::{
        profile::publish_profile,
        store::{CollectionBatch, CollectionSource},
    };

    #[test]
    fn publication_failure_retains_receipt_and_retry_does_not_duplicate_it() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let markdown = temp.path().join("style.md");
        let mut db = ProfileStore::open(&path).unwrap();
        let candidate = CollectedCandidate {
            id: "a".repeat(64),
            revision: "b".repeat(64),
            source: "session:fixture".into(),
            kind: "possible_self_report".into(),
            quote: "I usually review the contract before changing code.".into(),
            source_path: "/source".into(),
            line_no: 1,
            segment_no: 0,
            record_digest: "c".repeat(64),
            project_root: "/project".into(),
            project: "project-a".into(),
            observed_at: "2026-09-26".into(),
            extractor: "persona-explicit-v1".into(),
            rule_id: "habit.en".into(),
            status: "pending".into(),
            present: true,
        };
        db.collect_candidates(&[CollectionBatch {
            source: CollectionSource {
                source: candidate.source.clone(),
                source_path: candidate.source_path.clone(),
                project_root: candidate.project_root.clone(),
                project: candidate.project.clone(),
                repository: String::new(),
                snapshot_digest: "d".repeat(64),
                extractor: candidate.extractor.clone(),
                bytes: 100,
                lines: 1,
            },
            candidates: vec![candidate.clone()],
        }])
        .unwrap();
        let claim = NewHabit {
            when: "When changing code",
            behavior: "Reviews the contract",
            outcome: "Keeps the contract explicit",
            exception: "",
            scope: "project:project-a",
            role: "",
            workflow: "",
        };
        let request = "e".repeat(64);
        let failed = publish_profile(
            &path,
            &markdown,
            false,
            |db| {
                let receipt = db.propose_candidate_habit(
                    &candidate,
                    &claim,
                    "issue-one",
                    "",
                    &request,
                    None,
                )?;
                // Simulate a changed target after SQL commit, before atomic Markdown publication.
                std::fs::create_dir(&markdown)?;
                Ok(receipt)
            },
            |_| None,
        );
        assert!(failed.is_err());
        assert_eq!(db.habits().unwrap().len(), 1);
        assert!(db.habit_candidate_binding(1).unwrap().is_some());
        std::fs::remove_dir(&markdown).unwrap();
        let (receipt, published) = publish_profile(
            &path,
            &markdown,
            false,
            |db| {
                Ok(db.propose_candidate_habit(
                    &candidate,
                    &claim,
                    "issue-one",
                    "",
                    &request,
                    None,
                )?)
            },
            |_| None,
        )
        .unwrap();
        assert!(receipt.repeated);
        assert!(
            db.dismiss_collected_candidate(&candidate.id, &candidate.revision)
                .is_err(),
            "inbox dismissal must not silently leave a linked habit citation active"
        );
        let count: i64 = db
            .conn
            .query_row("SELECT count(*) FROM persona_review_event", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            published.revision,
            db.aggregate().unwrap().profile_revision()
        );

        let revision = db.aggregate().unwrap().profile_revision();
        let mut different = candidate;
        different.segment_no = 1;
        db.conn
            .execute(
                "UPDATE persona_candidate_habit SET candidate_json=?1",
                [serde_json::to_string(&different).unwrap()],
            )
            .unwrap();
        assert_ne!(
            db.aggregate().unwrap().profile_revision(),
            revision,
            "strict provenance must participate in the published revision"
        );
    }
}
