//! Revision-bound preferences and their immutable inbox provenance.

use super::{
    collection, digest_str, feedback_key, CollectedCandidate, Feedback, NewFeedback, ProfileStore,
    SqlResult,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

fn has_table(conn: &Connection, table: &str) -> SqlResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )
}

pub(super) fn read_feedback(conn: &Connection) -> SqlResult<Vec<Feedback>> {
    read_feedback_selected(conn, None)
}

fn read_feedback_selected(conn: &Connection, key: Option<&str>) -> SqlResult<Vec<Feedback>> {
    if !has_table(conn, "feedback")? || !has_table(conn, "feedback_source")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT f.key, f.statement, f.category, f.scope, f.quote, f.first_at, f.last_at,
         (SELECT COUNT(*) FROM feedback_source s WHERE s.key=f.key), f.status
         FROM feedback f WHERE (?1 IS NULL OR f.key=?1) ORDER BY f.key",
    )?;
    let mut entries = stmt
        .query_map([key], |r| {
            Ok(Feedback {
                key: r.get(0)?,
                statement: r.get(1)?,
                category: r.get(2)?,
                scope: r.get(3)?,
                quote: r.get(4)?,
                first_at: r.get(5)?,
                last_at: r.get(6)?,
                sources: r.get(7)?,
                status: r.get(8)?,
                evidence_revision: String::new(),
                accepted_revision: None,
                relations_revision: String::new(),
                superseded_by: None,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;
    let mut hashes: BTreeMap<_, _> = entries
        .iter()
        .map(|entry| {
            let mut hash = Sha256::new();
            hash.update(b"mastermind-preference-evidence-v1\0");
            for field in [&entry.quote, &entry.first_at, &entry.last_at] {
                digest_str(&mut hash, field);
            }
            (entry.key.clone(), hash)
        })
        .collect();
    // One ordered pass per table, instead of one unbounded query per preference.
    for (table, query, fields) in [
        ("feedback_source", "SELECT key, source FROM feedback_source WHERE (?1 IS NULL OR key=?1) ORDER BY key, source", 1),
        ("feedback_evidence", "SELECT key, source, quote, observed_at, attribution FROM feedback_evidence WHERE (?1 IS NULL OR key=?1) ORDER BY key, source, quote", 4),
        ("persona_candidate_feedback", "SELECT feedback_key, candidate_id, revision, request_digest, candidate_json FROM persona_candidate_feedback WHERE (?1 IS NULL OR feedback_key=?1) ORDER BY feedback_key, rowid", 4),
        ("persona_feedback_dismissal", "SELECT feedback_key, candidate_id, reviewed_revision FROM persona_feedback_dismissal WHERE (?1 IS NULL OR feedback_key=?1) ORDER BY feedback_key, candidate_id", 2),
    ] {
        if !has_table(conn, table)? { continue; }
        let mut stmt = conn.prepare(query)?;
        let mut rows = stmt.query([key])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            if let Some(hash) = hashes.get_mut(&key) {
                digest_str(hash, table);
                for i in 1..=fields { digest_str(hash, &row.get::<_, String>(i)?); }
            }
        }
    }
    let pins: BTreeMap<String, String> = if has_table(conn, "feedback_acceptance")? {
        let mut stmt = conn.prepare(
            "SELECT key, revision FROM feedback_acceptance WHERE (?1 IS NULL OR key=?1)",
        )?;
        let rows = stmt.query_map([key], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<SqlResult<_>>()?
    } else {
        BTreeMap::new()
    };
    for entry in &mut entries {
        entry.evidence_revision =
            crate::hex::encode(&hashes.remove(&entry.key).expect("entry hash").finalize());
        entry.accepted_revision = pins.get(&entry.key).cloned();
    }
    super::supersession::load_relations(conn, key, &mut entries)?;
    Ok(entries)
}

#[derive(Debug, Serialize)]
pub(crate) struct CandidateFeedbackReceipt {
    pub candidate_id: String,
    pub revision: String,
    pub feedback_key: String,
    pub status: String,
    pub repeated: bool,
}

impl ProfileStore {
    pub(crate) fn propose_candidate_feedback(
        &mut self,
        candidate: &CollectedCandidate,
        statement: &str,
        category: &str,
        scope: &str,
        repository: &str,
        request_digest: &str,
    ) -> SqlResult<CandidateFeedbackReceipt> {
        let snapshot = serde_json::to_string(candidate)
            .map_err(|_| invalid("invalid candidate provenance"))?;
        if snapshot.len() > 16 * 1024
            || [statement, category, scope].iter().any(|s| s.len() > 800)
            || repository.len() > 128
            || !super::super::collection::valid_id(request_digest)
        {
            return Err(invalid("preference proposal metadata exceeds its bounds"));
        }
        let key = feedback_key(statement, category, scope);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = tx
            .query_row(
                "SELECT id, source, kind, quote, source_path, line_no, segment_no, record_digest,
                project_root, project, observed_at, extractor, rule_id, revision, status, present
             FROM persona_candidate WHERE id=?1",
                [&candidate.id],
                collection::candidate_row,
            )
            .optional()?;
        if stored.as_ref() != Some(candidate) || !candidate.present || candidate.status != "pending"
        {
            return Err(invalid(
                "candidate changed, removed or dismissed; inspect it again",
            ));
        }
        let bound: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM persona_collection_source WHERE source=?1 AND project=?2 AND project_root=?3 AND repository=?4)",
            params![candidate.source,candidate.project,candidate.project_root,repository], |r| r.get(0),
        )?;
        if !bound {
            return Err(invalid("candidate source binding changed"));
        }
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM persona_feedback_dismissal WHERE candidate_id=?1)",
            [&candidate.id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid(
                "preference source was dismissed; a proposal cannot restore it",
            ));
        }
        let previous: Option<(String,String,String)> = tx.query_row(
            "SELECT request_digest, feedback_key, revision FROM persona_candidate_feedback WHERE candidate_id=?1 ORDER BY rowid DESC LIMIT 1",
            [&candidate.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional()?;
        if previous.as_ref().is_some_and(|(request, previous_key, _)| {
            request != request_digest || previous_key != &key
        }) {
            return Err(invalid(
                "candidate already proposed with different preference fields",
            ));
        }
        let old_status: Option<String> = tx
            .query_row("SELECT status FROM feedback WHERE key=?1", [&key], |r| {
                r.get(0)
            })
            .optional()?;
        if matches!(old_status.as_deref(), Some("rejected" | "superseded"))
            || super::supersession::has_successor(&tx, &key)?
        {
            return Err(invalid(
                "preference was rejected or superseded; a proposal cannot restore it",
            ));
        }
        if previous
            .as_ref()
            .is_some_and(|(_, _, rev)| rev == &candidate.revision)
        {
            return Ok(CandidateFeedbackReceipt {
                candidate_id: candidate.id.clone(),
                revision: candidate.revision.clone(),
                feedback_key: key,
                status: old_status.ok_or_else(|| invalid("proposal target missing"))?,
                repeated: true,
            });
        }
        if tx.query_row("SELECT EXISTS(SELECT 1 FROM persona_candidate_feedback WHERE candidate_id=?1 AND revision=?2)",
            params![candidate.id,candidate.revision], |r| r.get::<_,bool>(0))? {
            return Err(invalid("an older proposal revision cannot replace a newer binding"));
        }
        let already_strict: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM persona_candidate_feedback WHERE feedback_key=?1)",
            [&key],
            |r| r.get(0),
        )?;
        if previous.is_none() && already_strict && old_status.as_deref() == Some("active") {
            return Err(invalid("new support cannot change an accepted preference; review a new candidate separately"));
        }
        if previous.is_none() {
            let bindings: i64 = tx.query_row("SELECT count(DISTINCT candidate_id) FROM persona_candidate_feedback WHERE feedback_key=?1
                AND candidate_id NOT IN (SELECT candidate_id FROM persona_feedback_dismissal)", [&key], |r| r.get(0))?;
            if bindings >= 32 {
                return Err(invalid(
                    "preference already has 32 source bindings; review the existing evidence",
                ));
            }
        }
        super::record_feedback_tx(
            &tx,
            &NewFeedback {
                statement,
                category,
                scope,
                quote: &candidate.quote,
                at: &candidate.observed_at,
                source: &candidate.source,
            },
        )?;
        tx.execute("INSERT INTO persona_candidate_feedback (candidate_id,revision,request_digest,feedback_key,candidate_json,proposed_at)
            VALUES (?1,?2,?3,?4,?5,unixepoch())", params![candidate.id,candidate.revision,request_digest,key,snapshot])?;
        tx.execute(
            "UPDATE feedback SET status='candidate' WHERE key=?1",
            [&key],
        )?;
        tx.execute("DELETE FROM feedback_acceptance WHERE key=?1", [&key])?;
        let reason = if previous.is_some() {
            "rebind"
        } else if old_status.is_some() && !already_strict {
            "legacy_migration"
        } else {
            "proposal"
        };
        tx.execute(
            "INSERT INTO feedback_review_event (key,status,at_epoch) VALUES (?1,?2,unixepoch())",
            params![
                key,
                format!(
                    "{reason}:{}->candidate:{}:{}",
                    old_status.as_deref().unwrap_or("absent"),
                    candidate.id,
                    candidate.revision
                )
            ],
        )?;
        let count: i64 =
            tx.query_row("SELECT count(*) FROM persona_candidate_feedback", [], |r| {
                r.get(0)
            })?;
        if count > 20_000 {
            return Err(invalid("preference proposal history exceeds 20000 rows"));
        }
        collection::check_size(&tx)?;
        tx.commit()?;
        Ok(CandidateFeedbackReceipt {
            candidate_id: candidate.id.clone(),
            revision: candidate.revision.clone(),
            feedback_key: key,
            status: "candidate".into(),
            repeated: false,
        })
    }

    /// Latest receipt per observation. Legacy quotations remain history only.
    pub(crate) fn feedback_candidate_bindings(
        &self,
        key: &str,
    ) -> SqlResult<Vec<CollectedCandidate>> {
        if !self.has_table("persona_candidate_feedback")? {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT p.candidate_id,p.revision,p.candidate_json FROM persona_candidate_feedback p
             WHERE p.feedback_key=?1 AND p.rowid=(SELECT max(q.rowid) FROM persona_candidate_feedback q WHERE q.candidate_id=p.candidate_id)
             AND p.candidate_id NOT IN (SELECT candidate_id FROM persona_feedback_dismissal)
             ORDER BY p.candidate_id LIMIT 33",
        )?;
        let mut rows = stmt.query([key])?;
        let mut candidates = Vec::new();
        while let Some(row) = rows.next()? {
            let raw: String = row.get(2)?;
            if raw.len() > 16 * 1024 {
                return Err(invalid("candidate provenance exceeds its bound"));
            }
            let candidate: CollectedCandidate =
                serde_json::from_str(&raw).map_err(|_| invalid("invalid candidate provenance"))?;
            if candidate.id != row.get::<_, String>(0)?
                || candidate.revision != row.get::<_, String>(1)?
            {
                return Err(invalid("candidate provenance does not match receipt"));
            }
            let retained: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM feedback_evidence WHERE key=?1 AND source=?2 AND quote=?3 AND attribution='human_turn')",
                params![key,candidate.source,candidate.quote], |r| r.get(0),
            )?;
            if !retained {
                return Err(invalid("preference source evidence missing"));
            }
            candidates.push(candidate);
        }
        if candidates.len() > 32 {
            return Err(invalid("preference exceeds 32 verification bindings"));
        }
        Ok(candidates)
    }

    /// Detect a writer between the definition read and the provenance read.
    pub(crate) fn feedback_revision_current(&self, entry: &Feedback) -> SqlResult<bool> {
        Ok(read_feedback_selected(&self.conn, Some(&entry.key))?
            .first()
            .is_some_and(|current| {
                current.review_revision() == entry.review_revision()
                    && current.status == entry.status
                    && current.accepted_revision == entry.accepted_revision
                    && current.superseded_by == entry.superseded_by
            }))
    }

    /// Withdraw one observation without erasing the receipt or its quotes.
    /// Dismissal is sticky; accepting the remaining sources needs a new review.
    pub(crate) fn dismiss_feedback_source(
        &mut self,
        prefix: &str,
        candidate: &str,
        revision: &str,
    ) -> SqlResult<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let entries = read_feedback(&tx)?;
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| entry.key.starts_with(prefix))
            .collect();
        let [entry] = matches.as_slice() else {
            return Err(invalid("feedback key must identify exactly one preference"));
        };
        let previous: Option<String> = tx.query_row("SELECT reviewed_revision FROM persona_feedback_dismissal WHERE candidate_id=?1 AND feedback_key=?2", params![candidate,entry.key], |r|r.get(0)).optional()?;
        if previous.as_deref() == Some(revision) {
            return Ok(());
        }
        if revision != entry.review_revision() {
            return Err(invalid(
                "review revision changed; inspect feedback show again",
            ));
        }
        if previous.is_some() {
            return Ok(());
        }
        let bound: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM persona_candidate_feedback WHERE candidate_id=?1 AND feedback_key=?2)",params![candidate,entry.key],|r|r.get(0))?;
        if !bound {
            return Err(invalid("candidate is not bound to this preference"));
        }
        tx.execute(
            "INSERT INTO persona_feedback_dismissal VALUES (?1,?2,?3,unixepoch())",
            params![candidate, entry.key, revision],
        )?;
        tx.execute(
            "UPDATE feedback SET status=CASE WHEN status='rejected' THEN 'rejected'
             WHEN status='superseded' OR EXISTS(SELECT 1 FROM feedback_supersession WHERE old_key=?1)
             THEN 'superseded' ELSE 'candidate' END WHERE key=?1",
            [&entry.key],
        )?;
        tx.execute("DELETE FROM feedback_acceptance WHERE key=?1", [&entry.key])?;
        tx.execute("INSERT INTO feedback_review_event (key,status,at_epoch,reviewed_revision) VALUES (?1,?2,unixepoch(),?3)",params![entry.key,format!("source_dismissed:{candidate}:{}",entry.status),revision])?;
        collection::check_size(&tx)?;
        tx.commit()
    }

    /// Caller verifies sources under the writer lock. SQL pins exactly the
    /// reviewed definition/evidence and fails if another writer changed it.
    pub(crate) fn review_feedback(
        &mut self,
        prefix: &str,
        status: &str,
        revision: Option<&str>,
    ) -> SqlResult<usize> {
        if !matches!(status, "active" | "rejected") {
            return Err(invalid("invalid preference review status"));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let entries = read_feedback(&tx)?;
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| entry.key.starts_with(prefix))
            .collect();
        if let [entry] = matches.as_slice() {
            if entry.status == "superseded" || entry.superseded_by.is_some() {
                return Err(invalid(
                    "a superseded preference cannot be accepted or rejected again",
                ));
            }
            if status == "active"
                && (!matches!(entry.status.as_str(), "candidate" | "active")
                    || revision != Some(entry.review_revision().as_str()))
            {
                return Err(invalid(
                    "preference rejected or review revision changed; inspect it again",
                ));
            }
            if status == "active" {
                let bound: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM persona_candidate_feedback WHERE feedback_key=?1)",
                    [&entry.key],
                    |r| r.get(0),
                )?;
                if !bound {
                    return Err(invalid(
                        "legacy preference has no verifiable source; collect and propose it first",
                    ));
                }
                if entry.status == "active" && entry.accepted_revision.as_deref() == revision {
                    return Ok(1);
                }
                tx.execute("INSERT INTO feedback_acceptance(key,revision) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET revision=excluded.revision", params![entry.key,revision])?;
            } else {
                if entry.status == "rejected" {
                    return Ok(1);
                }
                tx.execute("DELETE FROM feedback_acceptance WHERE key=?1", [&entry.key])?;
            }
            tx.execute(
                "UPDATE feedback SET status=?2 WHERE key=?1",
                params![entry.key, status],
            )?;
            tx.execute("INSERT INTO feedback_review_event (key,status,at_epoch,reviewed_revision) VALUES (?1,?2,unixepoch(),?3)", params![entry.key,status,entry.review_revision()])?;
        }
        collection::check_size(&tx)?;
        tx.commit()?;
        Ok(matches.len())
    }

    pub(crate) fn candidate_feedback_history(
        &self,
        candidate: Option<&str>,
        key: Option<&str>,
    ) -> SqlResult<serde_json::Value> {
        if !self.has_table("persona_candidate_feedback")? {
            return Ok(serde_json::json!({"items":[],"truncated":false}));
        }
        let mut stmt = self.conn.prepare("SELECT p.candidate_id,p.revision,p.feedback_key,p.proposed_at,f.status,
            EXISTS(SELECT 1 FROM persona_feedback_dismissal d WHERE d.candidate_id=p.candidate_id)
            FROM persona_candidate_feedback p JOIN feedback f ON f.key=p.feedback_key
            WHERE (?1 IS NULL OR p.candidate_id=?1) AND (?2 IS NULL OR p.feedback_key=?2) ORDER BY p.rowid DESC LIMIT 21")?;
        let mut items: Vec<serde_json::Value> = stmt.query_map(params![candidate,key], |r| Ok(serde_json::json!({
            "candidate_id":r.get::<_,String>(0)?,"revision":r.get::<_,String>(1)?,"feedback_key":r.get::<_,String>(2)?,
            "proposed_at":r.get::<_,i64>(3)?,"status":r.get::<_,String>(4)?,"source_dismissed":r.get::<_,bool>(5)?})))?.collect::<SqlResult<_>>()?;
        let truncated = items.len() > 20;
        items.truncate(20);
        let mut stmt = self.conn.prepare(
            "SELECT status,at_epoch,reviewed_revision FROM feedback_review_event
            WHERE (?1 IS NULL OR key=?1) AND (?2 IS NULL OR key IN
                (SELECT feedback_key FROM persona_candidate_feedback WHERE candidate_id=?2))
            ORDER BY id DESC LIMIT 21",
        )?;
        let mut reviews: Vec<serde_json::Value> = stmt
            .query_map(params![key, candidate], |r| {
                Ok(serde_json::json!({
            "event":r.get::<_,String>(0)?,"at_epoch":r.get::<_,i64>(1)?,"reviewed_revision":r.get::<_,Option<String>>(2)?}))
            })?
            .collect::<SqlResult<_>>()?;
        let reviews_truncated = reviews.len() > 20;
        reviews.truncate(20);
        Ok(
            serde_json::json!({"items":items,"truncated":truncated,"reviews":reviews,"reviews_truncated":reviews_truncated}),
        )
    }
}

#[cfg(test)]
pub(crate) fn fixture_preference(
    db: &mut ProfileStore,
    statement: &str,
    scope: &str,
) -> CollectedCandidate {
    let id = crate::hex::encode(&Sha256::digest(format!("{statement}:{scope}")));
    let candidate = CollectedCandidate {
        id: id.clone(),
        revision: "b".repeat(64),
        source: format!("session:{id}"),
        kind: "possible_stated_preference".into(),
        quote: "I prefer short replies with the test results.".into(),
        source_path: format!("/fixture/{id}.jsonl"),
        line_no: 1,
        segment_no: 0,
        record_digest: "c".repeat(64),
        project_root: "/project".into(),
        project: "project-a".into(),
        observed_at: "2026-09-26".into(),
        extractor: "persona-explicit-v1".into(),
        rule_id: "preference.en".into(),
        status: "pending".into(),
        present: true,
    };
    fixture_collect(db, &candidate);
    db.propose_candidate_feedback(
        &candidate,
        statement,
        "communication",
        scope,
        "",
        &"e".repeat(64),
    )
    .unwrap();
    candidate
}

#[cfg(test)]
fn fixture_collect(db: &mut ProfileStore, candidate: &CollectedCandidate) {
    db.collect_candidates(&[super::CollectionBatch {
        source: super::CollectionSource {
            source: candidate.source.clone(),
            source_path: candidate.source_path.clone(),
            project_root: candidate.project_root.clone(),
            project: candidate.project.clone(),
            repository: String::new(),
            snapshot_digest: candidate.record_digest.clone(),
            extractor: candidate.extractor.clone(),
            bytes: 100,
            lines: 1,
        },
        candidates: vec![candidate.clone()],
    }])
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::super::super::profile::publish_profile;
    use super::*;

    const STATEMENT: &str = "Prefer short replies with test results";

    #[test]
    fn acceptance_pins_all_evidence_and_reject_is_sticky() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let candidate = fixture_preference(&mut db, STATEMENT, "global");
        let entry = db.feedback().unwrap().remove(0);
        let revision = entry.review_revision();
        assert!(db
            .review_feedback(&entry.key, "active", Some(&"0".repeat(64)))
            .is_err());
        db.review_feedback(&entry.key, "active", Some(&revision))
            .unwrap();
        let accepted = db.aggregate().unwrap().profile_revision();
        let retry = db
            .propose_candidate_feedback(
                &candidate,
                STATEMENT,
                "communication",
                "global",
                "",
                &"e".repeat(64),
            )
            .unwrap();
        assert!(retry.repeated);
        assert_eq!(retry.status, "active");
        assert_eq!(accepted, db.aggregate().unwrap().profile_revision());
        db.record_feedback(&NewFeedback {
            statement: STATEMENT,
            category: "communication",
            scope: "global",
            quote: "A later memory quote is not reviewed",
            source: "memory:later",
            at: "2026-09-27",
        })
        .unwrap();
        let changed = db.feedback().unwrap().remove(0);
        assert_eq!(
            changed.accepted_revision.as_deref(),
            Some(revision.as_str())
        );
        assert_ne!(changed.review_revision(), revision);
        assert!(db
            .review_feedback(&entry.key, "active", Some(&revision))
            .is_err());
        db.review_feedback(&entry.key, "rejected", None).unwrap();
        let history = db
            .candidate_feedback_history(None, Some(&entry.key))
            .unwrap();
        assert!(history["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["event"] == "active" && event["reviewed_revision"] == revision));
        assert!(db
            .review_feedback(&entry.key, "active", Some(&changed.review_revision()))
            .is_err());
        assert!(db
            .propose_candidate_feedback(
                &candidate,
                STATEMENT,
                "communication",
                "global",
                "",
                &"e".repeat(64)
            )
            .is_err());
        assert!(db
            .dismiss_collected_candidate(&candidate.id, &candidate.revision)
            .is_err());
    }

    #[test]
    fn legacy_migration_keeps_history_but_never_inherits_acceptance() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let entry = db
            .record_feedback(&NewFeedback {
                statement: STATEMENT,
                category: "communication",
                scope: "global",
                quote: "Agent memory from an old import",
                source: "memory:old",
                at: "2026-09-01",
            })
            .unwrap();
        db.set_feedback_status(&entry.key, "active").unwrap();
        assert!(db
            .review_feedback(&entry.key, "active", Some(&entry.review_revision()))
            .is_err());
        let mut candidate = fixture_preference(&mut db, STATEMENT, "global");
        let entry = db.feedback().unwrap().remove(0);
        assert_eq!(entry.status, "candidate");
        assert_eq!(entry.accepted_revision, None);
        assert_eq!(db.feedback_evidence(&entry.key).unwrap().len(), 2);
        assert!(db
            .candidate_feedback_history(None, Some(&entry.key))
            .unwrap()
            .to_string()
            .contains("legacy_migration:active->candidate"));
        db.review_feedback(&entry.key, "active", Some(&entry.review_revision()))
            .unwrap();
        candidate.revision = "f".repeat(64);
        candidate.record_digest = "f".repeat(64);
        candidate.line_no = 2;
        fixture_collect(&mut db, &candidate);
        db.propose_candidate_feedback(
            &candidate,
            STATEMENT,
            "communication",
            "global",
            "",
            &"e".repeat(64),
        )
        .unwrap();
        let rebound = db.feedback().unwrap().remove(0);
        assert_eq!(rebound.status, "candidate");
        assert_eq!(rebound.accepted_revision, None);
        assert_ne!(rebound.review_revision(), entry.review_revision());
        assert!(db
            .review_feedback(&entry.key, "active", Some(&entry.review_revision()))
            .is_err());
        let revision = db.aggregate().unwrap().profile_revision();
        db.conn
            .execute(
                "UPDATE persona_candidate_feedback SET request_digest='changed'",
                [],
            )
            .unwrap();
        assert_ne!(revision, db.aggregate().unwrap().profile_revision());
    }

    #[test]
    fn statement_key_collision_cannot_attach_different_meaning() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let candidate = fixture_preference(&mut db, "Use C++ for tooling", "global");
        assert_eq!(
            feedback_key("Use C++ for tooling", "communication", "global"),
            feedback_key("Use C for tooling", "communication", "global")
        );
        let mut other = candidate.clone();
        other.id = "f".repeat(64);
        other.source = "session:other".into();
        fixture_collect(&mut db, &other);
        assert!(db
            .propose_candidate_feedback(
                &other,
                "Use C for tooling",
                "communication",
                "global",
                "",
                &"d".repeat(64)
            )
            .is_err());
        assert_eq!(db.feedback().unwrap()[0].sources, 1);
        assert!(db
            .candidate_feedback_history(Some(&other.id), None)
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn binding_limit_rejects_growth_but_allows_relocation() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let mut original = fixture_preference(&mut db, STATEMENT, "global");
        for i in 1..=32 {
            let mut candidate = original.clone();
            candidate.id = format!("{i:064x}");
            candidate.source = format!("session:fixture-{i}");
            fixture_collect(&mut db, &candidate);
            let proposed = db.propose_candidate_feedback(
                &candidate,
                STATEMENT,
                "communication",
                "global",
                "",
                &"e".repeat(64),
            );
            if i < 32 {
                assert!(proposed.is_ok());
            } else {
                assert!(proposed
                    .unwrap_err()
                    .to_string()
                    .contains("32 source bindings"));
            }
        }
        let key = feedback_key(STATEMENT, "communication", "global");
        assert_eq!(db.feedback_candidate_bindings(&key).unwrap().len(), 32);
        assert_eq!(db.feedback().unwrap()[0].sources, 32);
        original.revision = "f".repeat(64);
        original.record_digest = "f".repeat(64);
        original.line_no = 2;
        fixture_collect(&mut db, &original);
        assert!(db
            .propose_candidate_feedback(
                &original,
                STATEMENT,
                "communication",
                "global",
                "",
                &"e".repeat(64)
            )
            .is_ok());
        assert_eq!(db.feedback_candidate_bindings(&key).unwrap().len(), 32);
    }

    #[test]
    fn sql_failure_rolls_back_and_publication_failure_has_an_idempotent_retry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let markdown = temp.path().join("style.md");
        let mut db = ProfileStore::open(&path).unwrap();
        let mut candidate = fixture_preference(&mut db, STATEMENT, "global");
        candidate.id = "f".repeat(64);
        candidate.source = "session:other".into();
        fixture_collect(&mut db, &candidate);
        db.conn.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON persona_candidate_feedback BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(db
            .propose_candidate_feedback(
                &candidate,
                "Prefer a concise review",
                "review",
                "global",
                "",
                &"e".repeat(64)
            )
            .is_err());
        assert_eq!(db.feedback().unwrap().len(), 1);
        db.conn.execute_batch("DROP TRIGGER fail_receipt").unwrap();
        let failed = publish_profile(
            &path,
            &markdown,
            false,
            |db| {
                let receipt = db.propose_candidate_feedback(
                    &candidate,
                    "Prefer a concise review",
                    "review",
                    "global",
                    "",
                    &"e".repeat(64),
                )?;
                std::fs::create_dir(&markdown)?;
                Ok(receipt)
            },
            |_| None,
        );
        assert!(failed.is_err());
        let history = db
            .candidate_feedback_history(Some(&candidate.id), None)
            .unwrap();
        std::fs::remove_dir(&markdown).unwrap();
        let (receipt, _) = publish_profile(
            &path,
            &markdown,
            false,
            |db| {
                Ok(db.propose_candidate_feedback(
                    &candidate,
                    "Prefer a concise review",
                    "review",
                    "global",
                    "",
                    &"e".repeat(64),
                )?)
            },
            |_| None,
        )
        .unwrap();
        assert!(receipt.repeated);
        assert_eq!(
            history,
            db.candidate_feedback_history(Some(&candidate.id), None)
                .unwrap()
        );
        db.reset().unwrap();
        assert!(db.feedback().unwrap().is_empty());
        assert!(db
            .feedback_candidate_bindings(&receipt.feedback_key)
            .unwrap()
            .is_empty());
    }
}
