//! Exact review of habit definitions, evidence and provenance.

use super::{collection, digest_str, Habit, ProfileStore, SqlResult};
use rusqlite::{params, Connection, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn has_table(conn: &Connection, table: &str) -> SqlResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )
}

pub(super) fn read_habits(conn: &Connection, id: Option<i64>) -> SqlResult<Vec<Habit>> {
    if !has_table(conn, "persona_claim")? || !has_table(conn, "persona_claim_evidence")? {
        return Ok(Vec::new());
    }
    let has_generation = conn
        .prepare("SELECT 1 FROM pragma_table_info('persona_claim') WHERE name='generation'")?
        .exists([])?;
    let generation = if has_generation { "c.generation" } else { "1" };
    let mut stmt = conn.prepare(&format!(
            "SELECT c.id, c.when_text, c.behavior, c.outcome, c.exception_text, c.scope, \
             c.role, c.workflow, c.status, \
             COUNT(DISTINCT CASE WHEN e.status = 'active' AND e.relation = 'supports' AND e.attribution = 'human_turn' \
                  THEN e.episode END), \
             COUNT(DISTINCT CASE WHEN e.status = 'active' AND e.relation = 'supports' AND e.attribution = 'human_turn' \
                  THEN e.source END), \
             COUNT(DISTINCT CASE WHEN e.status = 'active' AND e.relation = 'supports' AND e.attribution = 'human_turn' \
                  THEN e.project END), \
             COUNT(DISTINCT CASE WHEN e.status = 'active' AND e.relation = 'supports' AND e.attribution = 'human_turn' \
                  AND e.repository != '' THEN e.repository END), \
             COUNT(DISTINCT CASE WHEN e.status = 'active' AND e.relation = 'contradicts' THEN e.id END), \
             COUNT(DISTINCT CASE WHEN e.status = 'active' AND e.relation = 'limits' THEN e.id END), {generation} \
             FROM persona_claim c LEFT JOIN persona_claim_evidence e ON e.claim_id = c.id \
             WHERE c.kind = 'habit' AND (?1 IS NULL OR c.id=?1) GROUP BY c.id ORDER BY c.id",
        ))?;
    let rows = stmt.query_map([id], |row| {
        Ok(Habit {
            id: row.get(0)?,
            generation: row.get(15)?,
            generation_root: row.get(0)?,
            renewed_from: None,
            renewed_as: None,
            renewal_status: None,
            when: row.get(1)?,
            behavior: row.get(2)?,
            outcome: row.get(3)?,
            exception: row.get(4)?,
            scope: row.get(5)?,
            role: row.get(6)?,
            workflow: row.get(7)?,
            status: row.get(8)?,
            episodes: row.get(9)?,
            sources: row.get(10)?,
            projects: row.get(11)?,
            repositories: row.get(12)?,
            contradictions: row.get(13)?,
            limitations: row.get(14)?,
            evidence_revision: String::new(),
            observed_revision: None,
            relations_revision: String::new(),
            superseded_by: None,
        })
    })?;
    let mut habits: Vec<Habit> = rows.collect::<SqlResult<_>>()?;
    let mut evidence_stmt = conn.prepare(
        "SELECT claim_id, id, line_no, source, source_path, record_digest, episode, \
             project, repository, quote, observed_at, attribution, relation, status \
             FROM persona_claim_evidence WHERE (?1 IS NULL OR claim_id=?1) ORDER BY claim_id, id",
    )?;
    let evidence_rows = evidence_stmt.query_map([id], |row| {
        let fields: [String; 11] = [
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
            row.get(11)?,
            row.get(12)?,
            row.get(13)?,
        ];
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            fields,
        ))
    })?;
    let mut revisions: BTreeMap<i64, Sha256> = BTreeMap::new();
    for row in evidence_rows {
        let (claim_id, evidence_id, line_no, fields) = row?;
        let digest = revisions.entry(claim_id).or_insert_with(|| {
            let mut digest = Sha256::new();
            digest.update(b"mastermind-habit-evidence-v2\0");
            digest
        });
        digest.update(evidence_id.to_le_bytes());
        digest.update(line_no.to_le_bytes());
        for field in fields {
            digest_str(digest, &field);
        }
    }
    if has_table(conn, "persona_candidate_habit")? {
        let mut stmt = conn.prepare(
                "SELECT claim_id, evidence_id, candidate_id, revision, request_digest, candidate_json
                 FROM persona_candidate_habit WHERE (?1 IS NULL OR claim_id=?1) ORDER BY claim_id, evidence_id, rowid",
            )?;
        let rows = stmt.query_map([id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                [
                    row.get::<_, String>(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ],
            ))
        })?;
        for row in rows {
            let (claim, evidence, fields) = row?;
            let digest = revisions.entry(claim).or_default();
            digest.update(b"candidate-provenance-v1\0");
            digest.update(evidence.to_le_bytes());
            for field in fields {
                digest_str(digest, &field);
            }
        }
    }
    // A relocation round trip must not reuse an earlier reviewed digest.
    if has_table(conn, "persona_evidence_rebind")? {
        let mut stmt = conn.prepare("SELECT claim_id, CAST(id AS TEXT), CAST(evidence_id AS TEXT),
                old_source_path, CAST(old_line_no AS TEXT), old_record_digest, old_observed_at,
                new_source_path, CAST(new_line_no AS TEXT), new_record_digest, new_observed_at
                FROM persona_evidence_rebind WHERE (?1 IS NULL OR claim_id=?1) ORDER BY claim_id,id")?;
        let mut rows = stmt.query([id])?;
        while let Some(row) = rows.next()? {
            let claim: i64 = row.get(0)?;
            let hash = revisions.entry(claim).or_default();
            hash.update(b"source-relocation-v1\0");
            for i in 1..=10 {
                digest_str(hash, &row.get::<_, String>(i)?);
            }
        }
    }
    let pins: BTreeMap<i64, String> = if has_table(conn, "persona_habit_observation")? {
        let mut stmt = conn.prepare("SELECT claim_id,revision FROM persona_habit_observation WHERE (?1 IS NULL OR claim_id=?1)")?;
        let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<SqlResult<_>>()?
    } else {
        BTreeMap::new()
    };
    for habit in &mut habits {
        habit.evidence_revision = revisions
            .remove(&habit.id)
            .map(|digest| crate::hex::encode(&digest.finalize()))
            .unwrap_or_default();
        habit.observed_revision = pins.get(&habit.id).cloned();
    }
    super::habit_supersession::load_relations(conn, id, &mut habits)?;
    super::habit_generations::load_relations(conn, id, &mut habits)?;
    Ok(habits)
}

impl ProfileStore {
    /// Detect concurrent definition/evidence edits or a reject during source checks.
    pub(crate) fn habit_revision_current(&self, habit: &Habit) -> SqlResult<bool> {
        Ok(read_habits(&self.conn, Some(habit.id))?.first() == Some(habit))
    }

    /// Caller verifies current source bytes under the writer lock. Pin the
    /// inspected content atomically with the status and immutable review event.
    pub fn review_habit(
        &mut self,
        id: i64,
        status: &str,
        revision: Option<&str>,
    ) -> SqlResult<Result<Habit, String>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut habit) = read_habits(&tx, Some(id))?.into_iter().next() else {
            return Ok(Err(format!("habit {id} does not exist")));
        };
        if !matches!(status, "observed" | "rejected") {
            return Ok(Err(
                "habit review status must be observed or rejected".into()
            ));
        }
        if habit.status == "superseded"
            || habit.superseded_by.is_some()
            || habit.renewed_as.is_some()
        {
            return Ok(Err(
                "a retired habit cannot be observed or rejected again".into()
            ));
        }
        if status == "observed" {
            if revision != Some(habit.review_revision().as_str()) {
                return Ok(Err(
                    "habit review revision changed; inspect habit show again".into(),
                ));
            }
            if let Some(reason) = habit.observation_problem() {
                return Ok(Err(reason.into()));
            }
            if habit.status == "observed" && habit.observed_revision.as_deref() == revision {
                return Ok(Ok(habit));
            }
            tx.execute(
                "INSERT INTO persona_habit_observation(claim_id,revision) VALUES (?1,?2)
                ON CONFLICT(claim_id) DO UPDATE SET revision=excluded.revision",
                params![id, revision],
            )?;
            habit.observed_revision = revision.map(str::to_owned);
        } else {
            if habit.status == "rejected" {
                return Ok(Ok(habit));
            }
            tx.execute(
                "DELETE FROM persona_habit_observation WHERE claim_id=?1",
                [id],
            )?;
            habit.observed_revision = None;
        }
        tx.execute(
            "UPDATE persona_claim SET status=?2 WHERE id=?1",
            params![id, status],
        )?;
        tx.execute(
            "INSERT INTO persona_review_event(claim_id,status,at_epoch,reviewed_revision)
            VALUES (?1,?2,unixepoch(),?3)",
            params![
                id,
                status,
                if status == "observed" { revision } else { None }
            ],
        )?;
        collection::check_size(&tx)?;
        tx.commit()?;
        habit.status = status.into();
        Ok(Ok(habit))
    }

    /// Local review history, including legacy events that have no reviewed digest.
    pub(crate) fn habit_review_history(&self, id: i64) -> SqlResult<serde_json::Value> {
        if !has_table(&self.conn, "persona_review_event")? {
            return Ok(serde_json::json!({"items":[],"truncated":false}));
        }
        let has_revision = self.conn.prepare("SELECT 1 FROM pragma_table_info('persona_review_event') WHERE name='reviewed_revision'")?.exists([])?;
        let revision = if has_revision {
            "reviewed_revision"
        } else {
            "NULL"
        };
        let mut stmt = self.conn.prepare(&format!("SELECT status,at_epoch,{revision} FROM persona_review_event WHERE claim_id=?1 ORDER BY id DESC LIMIT 21"))?;
        let mut items: Vec<serde_json::Value> = stmt.query_map([id], |r| Ok(serde_json::json!({
            "event":r.get::<_,String>(0)?,"at_epoch":r.get::<_,i64>(1)?,"reviewed_revision":r.get::<_,Option<String>>(2)?
        })))?.collect::<SqlResult<_>>()?;
        let truncated = items.len() > 20;
        items.truncate(20);
        Ok(serde_json::json!({"items":items,"truncated":truncated}))
    }
}

#[cfg(test)]
pub(crate) fn fixture_habit(db: &mut ProfileStore, behavior: &str) -> Habit {
    let claim = super::NewHabit {
        when: "When reviewing changes",
        behavior,
        outcome: "Reports the evidence",
        exception: "",
        scope: "project:fixture",
        role: "",
        workflow: "",
    };
    let first = db
        .record_habit(&claim, &fixture_quote("session:a", "task-a", "supports"))
        .unwrap();
    db.add_habit_evidence(first.id, &fixture_quote("session:b", "task-b", "supports"))
        .unwrap()
        .unwrap()
}

#[cfg(test)]
fn fixture_quote<'a>(
    source: &'a str,
    episode: &'a str,
    relation: &'a str,
) -> super::NewHabitEvidence<'a> {
    super::NewHabitEvidence {
        source,
        source_path: "fixture.jsonl",
        line_no: 1,
        record_digest: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        episode,
        project: "fixture",
        repository: "fixture-origin",
        quote: "Checks the contract before changing code",
        at: "2026-09-26",
        relation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::miner::profile::publish_profile;

    fn observe(db: &mut ProfileStore, id: i64) -> Habit {
        let revision = db.habit(id).unwrap().unwrap().review_revision();
        db.review_habit(id, "observed", Some(&revision))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn exact_observation_is_pinned_idempotent_and_rejection_is_terminal() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let habit = fixture_habit(&mut db, "Checks contracts before delivery");
        let before = db.aggregate().unwrap().profile_revision();
        assert!(db
            .review_habit(habit.id, "observed", None)
            .unwrap()
            .is_err());
        assert!(db
            .review_habit(habit.id, "observed", Some(&"f".repeat(64)))
            .unwrap()
            .is_err());
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        let accepted = observe(&mut db, habit.id);
        assert_eq!(accepted.observed_revision, Some(habit.review_revision()));
        assert_eq!(accepted.review_revision(), habit.review_revision());
        assert_ne!(before, db.aggregate().unwrap().profile_revision());
        let after = db.aggregate().unwrap().profile_revision();
        let history = db.habit_review_history(habit.id).unwrap();
        assert_eq!(
            history["items"][0]["reviewed_revision"],
            habit.review_revision()
        );
        observe(&mut db, habit.id);
        assert_eq!(history, db.habit_review_history(habit.id).unwrap());
        assert_eq!(after, db.aggregate().unwrap().profile_revision());
        db.review_habit(habit.id, "rejected", None)
            .unwrap()
            .unwrap();
        assert_eq!(db.habit(habit.id).unwrap().unwrap().observed_revision, None);
        assert!(db
            .review_habit(habit.id, "observed", Some(&habit.review_revision()))
            .unwrap()
            .is_err());
        let rejected = db.aggregate().unwrap().profile_revision();
        db.review_habit(habit.id, "rejected", None)
            .unwrap()
            .unwrap();
        assert_eq!(rejected, db.aggregate().unwrap().profile_revision());
        assert_eq!(
            db.habit_review_history(habit.id).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn every_definition_field_is_bound_and_concurrent_rejection_fails_snapshot_check() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let mut db = ProfileStore::open(&path).unwrap();
        let habit = fixture_habit(&mut db, "Checks contracts before delivery");
        let other = Connection::open(&path).unwrap();
        for field in [
            "when_text",
            "behavior",
            "outcome",
            "exception_text",
            "scope",
            "role",
            "workflow",
        ] {
            let original: String = other
                .query_row(
                    &format!("SELECT {field} FROM persona_claim WHERE id=?1"),
                    [habit.id],
                    |r| r.get(0),
                )
                .unwrap();
            other
                .execute(
                    &format!("UPDATE persona_claim SET {field}=?2 WHERE id=?1"),
                    params![habit.id, format!("{original} changed")],
                )
                .unwrap();
            assert!(!db.habit_revision_current(&habit).unwrap());
            assert!(
                db.review_habit(habit.id, "observed", Some(&habit.review_revision()))
                    .unwrap()
                    .is_err(),
                "{field}"
            );
            other
                .execute(
                    &format!("UPDATE persona_claim SET {field}=?2 WHERE id=?1"),
                    params![habit.id, original],
                )
                .unwrap();
        }
        other
            .execute(
                "UPDATE persona_claim_evidence SET attribution='agent' WHERE claim_id=?1",
                [habit.id],
            )
            .unwrap();
        assert!(db
            .review_habit(habit.id, "observed", Some(&habit.review_revision()))
            .unwrap()
            .is_err());
        other
            .execute(
                "UPDATE persona_claim_evidence SET attribution='human_turn' WHERE claim_id=?1",
                [habit.id],
            )
            .unwrap();
        assert!(db.habit_revision_current(&habit).unwrap());
        other
            .execute(
                "UPDATE persona_claim SET status='rejected' WHERE id=?1",
                [habit.id],
            )
            .unwrap();
        assert_eq!(
            habit.review_revision(),
            db.habit(habit.id).unwrap().unwrap().review_revision()
        );
        assert!(!db.habit_revision_current(&habit).unwrap());
        assert!(db
            .review_habit(habit.id, "observed", Some(&habit.review_revision()))
            .unwrap()
            .is_err());
    }

    #[test]
    fn new_support_and_withdrawal_invalidate_pin_but_exact_citation_does_not() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let habit = fixture_habit(&mut db, "Checks contracts before delivery");
        let accepted = observe(&mut db, habit.id);
        let before = db.aggregate().unwrap().profile_revision();
        db.add_habit_evidence(habit.id, &fixture_quote("session:a", "task-a", "supports"))
            .unwrap();
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        let extra = fixture_quote("session:c", "task-c", "supports");
        let changed = db.add_habit_evidence(habit.id, &extra).unwrap().unwrap();
        assert_eq!(changed.status, "stale");
        assert_eq!(changed.observed_revision, None);
        assert!(db
            .review_habit(habit.id, "observed", Some(&accepted.review_revision()))
            .unwrap()
            .is_err());
        observe(&mut db, habit.id);
        let id = db
            .habit_evidence(habit.id)
            .unwrap()
            .iter()
            .find(|e| e.source == "session:c")
            .unwrap()
            .id;
        db.dismiss_habit_evidence(habit.id, id).unwrap();
        let changed = db.habit(habit.id).unwrap().unwrap();
        assert_eq!(changed.status, "stale");
        assert_eq!(changed.observed_revision, None);
        assert!(db.add_habit_evidence(habit.id, &extra).is_err());
        observe(&mut db, habit.id);
        db.review_habit(habit.id, "rejected", None)
            .unwrap()
            .unwrap();
        let id = db.habit_evidence(habit.id).unwrap()[0].id;
        db.dismiss_habit_evidence(habit.id, id).unwrap();
        assert_eq!(db.habit(habit.id).unwrap().unwrap().status, "rejected");
    }

    #[test]
    fn relocation_round_trip_retains_history_without_reusing_observed_revision() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let habit = fixture_habit(&mut db, "Checks contracts before delivery");
        let accepted = observe(&mut db, habit.id);
        let original = fixture_quote("session:a", "task-a", "supports");
        let mut moved = fixture_quote("session:a", "task-a", "supports");
        moved.source_path = "archive.jsonl";
        db.add_habit_evidence(habit.id, &moved).unwrap();
        assert_eq!(db.habit(habit.id).unwrap().unwrap().observed_revision, None);
        observe(&mut db, habit.id);
        db.add_habit_evidence(habit.id, &original).unwrap();
        let back = db.habit(habit.id).unwrap().unwrap();
        assert_eq!(back.status, "stale");
        assert_eq!(back.observed_revision, None);
        assert_ne!(back.review_revision(), accepted.review_revision());
        assert!(db
            .review_habit(habit.id, "observed", Some(&accepted.review_revision()))
            .unwrap()
            .is_err());
        assert_eq!(db.habit_rebinds(habit.id).unwrap().len(), 2);
    }

    #[test]
    fn sql_event_failure_and_size_limit_roll_back_status_and_pin() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let habit = fixture_habit(&mut db, "Checks contracts before delivery");
        let before = db.aggregate().unwrap().profile_revision();
        db.conn.execute_batch("CREATE TRIGGER fail_review BEFORE INSERT ON persona_review_event BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(db
            .review_habit(habit.id, "observed", Some(&habit.review_revision()))
            .is_err());
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        assert!(db.habit_review_history(habit.id).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty());
        db.conn.execute_batch("DROP TRIGGER fail_review; CREATE TABLE fixture_padding(bytes BLOB);
            CREATE TRIGGER grow_review AFTER INSERT ON persona_habit_observation BEGIN INSERT INTO fixture_padding VALUES(zeroblob(67108864)); END;").unwrap();
        assert!(db
            .review_habit(habit.id, "observed", Some(&habit.review_revision()))
            .unwrap_err()
            .to_string()
            .contains("64 MiB"));
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        assert!(db.habit_review_history(habit.id).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn publication_failure_can_refresh_without_repeating_the_review() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let markdown = temp.path().join("style.md");
        let mut db = ProfileStore::open(&path).unwrap();
        let habit = fixture_habit(&mut db, "Checks contracts before delivery");
        assert!(publish_profile(
            &path,
            &markdown,
            false,
            |db| {
                let observed =
                    db.review_habit(habit.id, "observed", Some(&habit.review_revision()))??;
                std::fs::create_dir(&markdown)?;
                Ok(observed)
            },
            |_| None
        )
        .is_err());
        assert_eq!(
            db.habit(habit.id).unwrap().unwrap().observed_revision,
            Some(habit.review_revision())
        );
        let history = db.habit_review_history(habit.id).unwrap();
        std::fs::remove_dir(&markdown).unwrap();
        publish_profile(&path, &markdown, false, |_| Ok(()), |_| None).unwrap();
        assert_eq!(history, db.habit_review_history(habit.id).unwrap());
        // Fixture sources are unavailable: recovery never fabricates publication.
        assert!(!std::fs::read_to_string(markdown)
            .unwrap()
            .contains(&habit.behavior));
        db.review_habit(habit.id, "rejected", None)
            .unwrap()
            .unwrap();
        assert!(db
            .review_habit(habit.id, "observed", Some(&habit.review_revision()))
            .unwrap()
            .is_err());
    }

    #[test]
    fn old_schema_readonly_and_migration_preserve_unpinned_history_and_reset_clears_pins() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let mut db = ProfileStore::open(&path).unwrap();
        let habit = fixture_habit(&mut db, "Checks contracts before delivery");
        observe(&mut db, habit.id);
        db.conn.execute_batch("DROP TABLE persona_habit_observation; ALTER TABLE persona_review_event DROP COLUMN reviewed_revision;").unwrap();
        drop(db);
        let db = ProfileStore::open_read_only(&path).unwrap();
        assert_eq!(db.habit(habit.id).unwrap().unwrap().observed_revision, None);
        assert!(
            db.habit_review_history(habit.id).unwrap()["items"][0]["reviewed_revision"].is_null()
        );
        drop(db);
        let mut db = ProfileStore::open(&path).unwrap();
        assert_eq!(db.habit(habit.id).unwrap().unwrap().observed_revision, None);
        assert!(
            db.habit_review_history(habit.id).unwrap()["items"][0]["reviewed_revision"].is_null()
        );
        observe(&mut db, habit.id);
        db.reset().unwrap();
        let new = fixture_habit(&mut db, "Checks contracts before delivery");
        assert_eq!(new.id, habit.id);
        assert_eq!(new.observed_revision, None);
        assert!(db.habit_review_history(new.id).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}
