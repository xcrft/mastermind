//! Explicit, reviewed replacement of a preference in the same scope/category.
//! Relations describe a review decision, not a similarity or a source timestamp.

use super::{collection, digest_str, preferences, Feedback, ProfileStore, SqlResult};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

fn has_table(conn: &Connection) -> SqlResult<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='feedback_supersession')", [], |r| r.get(0))
}

pub(super) fn has_successor(conn: &Connection, key: &str) -> SqlResult<bool> {
    if !has_table(conn)? {
        return Ok(false);
    }
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM feedback_supersession WHERE old_key=?1)",
        [key],
        |r| r.get(0),
    )
}

/// Relations affect the published snapshot, independently of source review pins.
pub(super) fn load_relations(
    conn: &Connection,
    key: Option<&str>,
    entries: &mut [Feedback],
) -> SqlResult<()> {
    let mut hashes: BTreeMap<_, _> = entries
        .iter()
        .map(|entry| {
            let mut hash = Sha256::new();
            hash.update(b"mastermind-preference-relations-v1\0");
            (entry.key.clone(), hash)
        })
        .collect();
    let mut successors = BTreeMap::new();
    if has_table(conn)? {
        let mut stmt = conn.prepare("SELECT old_key,new_key,old_revision,new_revision,CAST(reviewed_at AS TEXT)
            FROM feedback_supersession WHERE (?1 IS NULL OR old_key=?1 OR new_key=?1) ORDER BY old_key")?;
        let mut rows = stmt.query([key])?;
        while let Some(row) = rows.next()? {
            let fields = (0..5)
                .map(|i| row.get::<_, String>(i))
                .collect::<SqlResult<Vec<_>>>()?;
            for endpoint in [&fields[0], &fields[1]] {
                if let Some(hash) = hashes.get_mut(endpoint) {
                    for field in &fields {
                        digest_str(hash, field);
                    }
                }
            }
            successors.insert(fields[0].clone(), fields[1].clone());
        }
    }
    for entry in entries {
        entry.relations_revision =
            crate::hex::encode(&hashes.remove(&entry.key).expect("entry hash").finalize());
        entry.superseded_by = successors.remove(&entry.key);
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub(crate) struct SupersessionReceipt {
    pub old_key: String,
    pub new_key: String,
    pub old_revision: String,
    pub new_revision: String,
    pub reviewed_at: i64,
    pub old_status: String,
    pub new_status: String,
    pub repeated: bool,
}

fn committed_receipt(
    conn: &Connection,
    old: &str,
    new: &str,
    old_revision: &str,
    new_revision: &str,
) -> SqlResult<Option<SupersessionReceipt>> {
    if !has_table(conn)? {
        return Ok(None);
    }
    let previous: Option<(String,String,String,i64)> = conn.query_row(
        "SELECT new_key,old_revision,new_revision,reviewed_at FROM feedback_supersession WHERE old_key=?1", [old],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
    ).optional()?;
    let Some((target, old_pin, new_pin, at)) = previous else {
        return Ok(None);
    };
    if target != new || old_pin != old_revision || new_pin != new_revision {
        return Err(invalid(
            "preference already superseded by a different reviewed request; inspect its relations",
        ));
    }
    let status = |key| {
        conn.query_row("SELECT status FROM feedback WHERE key=?1", [key], |r| {
            r.get::<_, String>(0)
        })
    };
    Ok(Some(SupersessionReceipt {
        old_key: old.into(),
        new_key: target,
        old_revision: old_pin,
        new_revision: new_pin,
        reviewed_at: at,
        old_status: status(old)?,
        new_status: status(new)?,
        repeated: true,
    }))
}

impl ProfileStore {
    /// Check a committed request before source verification or current status
    /// gates. A retry republishes the current state and never re-accepts it.
    pub(crate) fn feedback_supersession_retry(
        &self,
        old: &str,
        new: &str,
        old_revision: &str,
        new_revision: &str,
    ) -> SqlResult<Option<SupersessionReceipt>> {
        committed_receipt(&self.conn, old, new, old_revision, new_revision)
    }

    /// The caller verifies the successor's sources under the publication lock.
    /// Both definitions are compared again under an immediate SQL transaction.
    pub(crate) fn supersede_feedback(
        &mut self,
        old: &str,
        new: &str,
        old_revision: &str,
        new_revision: &str,
    ) -> SqlResult<SupersessionReceipt> {
        if old == new {
            return Err(invalid("a preference cannot supersede itself"));
        }
        if !super::super::collection::valid_id(old_revision)
            || !super::super::collection::valid_id(new_revision)
        {
            return Err(invalid(
                "both review revisions must be full 64-character keys",
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(receipt) = committed_receipt(&tx, old, new, old_revision, new_revision)? {
            return Ok(receipt);
        }
        let entries = preferences::read_feedback(&tx)?;
        let old_entry = entries
            .iter()
            .find(|entry| entry.key == old)
            .ok_or_else(|| invalid("old preference missing"))?;
        let new_entry = entries
            .iter()
            .find(|entry| entry.key == new)
            .ok_or_else(|| invalid("new preference missing"))?;
        if old_entry.review_revision() != old_revision
            || new_entry.review_revision() != new_revision
        {
            return Err(invalid(
                "review revision changed; inspect both preferences again",
            ));
        }
        if old_entry.category != new_entry.category || old_entry.scope != new_entry.scope {
            return Err(invalid("replacement requires the same category and scope"));
        }
        for entry in [old_entry, new_entry] {
            if !matches!(entry.status.as_str(), "candidate" | "active")
                || entry.superseded_by.is_some()
            {
                return Err(invalid(
                    "rejected or superseded preferences cannot take part in a new replacement",
                ));
            }
        }
        // Each old node has at most one outgoing edge. Both endpoints must
        // currently have none, and self-links are forbidden, so an insertion
        // cannot close a cycle. There is no unbounded ancestry traversal.
        let bound: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM persona_candidate_feedback p WHERE feedback_key=?1
            AND NOT EXISTS(SELECT 1 FROM persona_feedback_dismissal d WHERE d.candidate_id=p.candidate_id))", [new], |r|r.get(0))?;
        if !bound {
            return Err(invalid(
                "replacement needs a currently verified strict source",
            ));
        }
        let at: i64 = tx.query_row("SELECT unixepoch()", [], |r| r.get(0))?;
        tx.execute("INSERT INTO feedback_supersession (old_key,new_key,old_revision,new_revision,reviewed_at)
            VALUES (?1,?2,?3,?4,?5)", params![old,new,old_revision,new_revision,at])?;
        tx.execute(
            "UPDATE feedback SET status='superseded' WHERE key=?1",
            [old],
        )?;
        tx.execute("DELETE FROM feedback_acceptance WHERE key=?1", [old])?;
        tx.execute("UPDATE feedback SET status='active' WHERE key=?1", [new])?;
        tx.execute("INSERT INTO feedback_acceptance(key,revision) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET revision=excluded.revision", params![new,new_revision])?;
        for (key, event, revision) in [
            (
                old,
                format!("superseded:{}->{new}", old_entry.status),
                old_revision,
            ),
            (
                new,
                format!("supersedes:{old}:{}->active", new_entry.status),
                new_revision,
            ),
        ] {
            tx.execute("INSERT INTO feedback_review_event(key,status,at_epoch,reviewed_revision) VALUES (?1,?2,?3,?4)", params![key,event,at,revision])?;
        }
        let count: i64 = tx.query_row("SELECT count(*) FROM feedback_supersession", [], |r| {
            r.get(0)
        })?;
        if count > 20_000 {
            return Err(invalid("preference relation history exceeds 20000 rows"));
        }
        collection::check_size(&tx)?;
        tx.commit()?;
        Ok(SupersessionReceipt {
            old_key: old.into(),
            new_key: new.into(),
            old_revision: old_revision.into(),
            new_revision: new_revision.into(),
            reviewed_at: at,
            old_status: "superseded".into(),
            new_status: "active".into(),
            repeated: false,
        })
    }

    /// Local, bounded adjacency list. Retired wording is never sent to MCP.
    pub(crate) fn feedback_relations(&self, key: &str) -> SqlResult<serde_json::Value> {
        if !has_table(&self.conn)? {
            return Ok(serde_json::json!({"items":[],"truncated":false}));
        }
        let mut stmt = self.conn.prepare("SELECT r.old_key,r.new_key,r.old_revision,r.new_revision,r.reviewed_at,a.status,b.status
            FROM feedback_supersession r JOIN feedback a ON a.key=r.old_key JOIN feedback b ON b.key=r.new_key
            WHERE r.old_key=?1 OR r.new_key=?1 ORDER BY r.reviewed_at DESC,r.old_key LIMIT 21")?;
        let mut items: Vec<serde_json::Value> = stmt.query_map([key], |r|Ok(serde_json::json!({
            "relation":"supersedes","old_key":r.get::<_,String>(0)?,"new_key":r.get::<_,String>(1)?,
            "old_revision":r.get::<_,String>(2)?,"new_revision":r.get::<_,String>(3)?,"reviewed_at":r.get::<_,i64>(4)?,
            "old_status":r.get::<_,String>(5)?,"new_status":r.get::<_,String>(6)?})))?.collect::<SqlResult<_>>()?;
        let truncated = items.len() > 20;
        items.truncate(20);
        Ok(serde_json::json!({"items":items,"truncated":truncated}))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{preferences::fixture_preference, NewFeedback};
    use super::*;
    use crate::miner::profile::publish_profile;

    fn entry(db: &ProfileStore, statement: &str) -> Feedback {
        db.feedback()
            .unwrap()
            .into_iter()
            .find(|e| e.statement == statement)
            .unwrap()
    }

    fn replace(
        db: &mut ProfileStore,
        old: &Feedback,
        new: &Feedback,
    ) -> SqlResult<SupersessionReceipt> {
        db.supersede_feedback(
            &old.key,
            &new.key,
            &old.review_revision(),
            &new.review_revision(),
        )
    }

    fn pair(db: &mut ProfileStore) -> (Feedback, Feedback) {
        fixture_preference(db, "Prefer short replies", "global");
        fixture_preference(db, "Prefer detailed replies", "global");
        (
            entry(db, "Prefer short replies"),
            entry(db, "Prefer detailed replies"),
        )
    }

    #[test]
    fn replacement_pins_successor_without_changing_source_review_revision() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let (old, new) = pair(&mut db);
        db.review_feedback(&old.key, "active", Some(&old.review_revision()))
            .unwrap();
        let before = db.aggregate().unwrap().profile_revision();
        let receipt = replace(&mut db, &old, &new).unwrap();
        assert!(!receipt.repeated);
        let retired = entry(&db, &old.statement);
        let accepted = entry(&db, &new.statement);
        assert_eq!(retired.status, "superseded");
        assert_eq!(retired.accepted_revision, None);
        assert_eq!(retired.superseded_by.as_deref(), Some(new.key.as_str()));
        assert_eq!(accepted.status, "active");
        assert_eq!(accepted.review_revision(), new.review_revision());
        assert_eq!(accepted.accepted_revision, Some(new.review_revision()));
        assert_ne!(accepted.relations_revision, new.relations_revision);
        assert_ne!(before, db.aggregate().unwrap().profile_revision());
        assert_eq!(db.feedback_evidence(&old.key).unwrap().len(), 1);
        let history = db.candidate_feedback_history(None, Some(&old.key)).unwrap();
        assert!(history["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["event"].as_str().unwrap().starts_with("superseded:")
                && r["reviewed_revision"] == old.review_revision()));
        // Relation metadata belongs to the aggregate, not to the source pin.
        let snapshot = db.aggregate().unwrap().profile_revision();
        db.conn
            .execute(
                "UPDATE feedback_supersession SET reviewed_at=reviewed_at+1",
                [],
            )
            .unwrap();
        assert_ne!(snapshot, db.aggregate().unwrap().profile_revision());
        assert_eq!(
            entry(&db, &new.statement).review_revision(),
            new.review_revision()
        );
    }

    #[test]
    fn chain_retry_never_reactivates_an_intermediate_or_rejected_successor() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let (a, b) = pair(&mut db);
        fixture_preference(&mut db, "Prefer replies with a summary", "global");
        let c = entry(&db, "Prefer replies with a summary");
        replace(&mut db, &a, &b).unwrap();
        replace(&mut db, &b, &c).unwrap();
        let before = db.aggregate().unwrap().profile_revision();
        let history = db.candidate_feedback_history(None, None).unwrap();
        let retry = replace(&mut db, &a, &b).unwrap();
        assert!(retry.repeated);
        assert_eq!(retry.new_status, "superseded");
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        assert_eq!(history, db.candidate_feedback_history(None, None).unwrap());
        assert_eq!(entry(&db, &c.statement).status, "active");
        assert!(
            replace(&mut db, &c, &a).is_err(),
            "backward link must not create a cycle"
        );
        assert!(
            replace(&mut db, &a, &c).is_err(),
            "one predecessor cannot have two successors"
        );
        db.review_feedback(&c.key, "rejected", None).unwrap();
        assert_eq!(replace(&mut db, &b, &c).unwrap().new_status, "rejected");
        assert_eq!(entry(&db, &c.statement).accepted_revision, None);
        assert!(db.feedback().unwrap().iter().all(|e| e.status != "active"));
    }

    #[test]
    fn fan_in_preserves_acceptance_and_local_history_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        fixture_preference(&mut db, "Prefer detailed replies", "global");
        let new = entry(&db, "Prefer detailed replies");
        for i in 0..22 {
            let statement = format!("Prefer reply format {i}");
            fixture_preference(&mut db, &statement, "global");
            let old = entry(&db, &statement);
            replace(&mut db, &old, &new).unwrap();
        }
        let accepted = entry(&db, &new.statement);
        assert_eq!(accepted.accepted_revision, Some(accepted.review_revision()));
        let history = db.feedback_relations(&new.key).unwrap();
        assert_eq!(history["items"].as_array().unwrap().len(), 20);
        assert_eq!(history["truncated"], true);
        assert!(
            db.feedback()
                .unwrap()
                .iter()
                .filter(|e| e.status == "active")
                .count()
                == 1
        );
    }

    #[test]
    fn terminals_and_source_withdrawal_cannot_revive_a_predecessor() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let mut db = ProfileStore::open(&path).unwrap();
        let candidate = fixture_preference(&mut db, "Prefer short replies", "global");
        fixture_preference(&mut db, "Prefer detailed replies", "global");
        let old = entry(&db, "Prefer short replies");
        let new = entry(&db, "Prefer detailed replies");
        replace(&mut db, &old, &new).unwrap();
        // Even an older writer that resets the raw status cannot erase the edge.
        db.set_feedback_status(&old.key, "active").unwrap();
        assert!(db
            .review_feedback(&old.key, "active", Some(&old.review_revision()))
            .is_err());
        assert!(db.review_feedback(&old.key, "rejected", None).is_err());
        assert!(db
            .propose_candidate_feedback(
                &candidate,
                &old.statement,
                "communication",
                "global",
                "",
                &"e".repeat(64)
            )
            .is_err());
        let markdown = temp.path().join("style.md");
        publish_profile(&path, &markdown, false, |_| Ok(()), |_| None).unwrap();
        assert!(!std::fs::read_to_string(&markdown)
            .unwrap()
            .contains(&old.statement));
        db.dismiss_feedback_source(&old.key, &candidate.id, &old.review_revision())
            .unwrap();
        assert_eq!(entry(&db, &old.statement).status, "superseded");
        assert_eq!(entry(&db, &old.statement).accepted_revision, None);
        let before = db.aggregate().unwrap().profile_revision();
        assert!(replace(&mut db, &old, &new).unwrap().repeated);
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
    }

    #[test]
    fn revision_scope_category_and_strict_successor_gates_leave_both_sides_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let (old, new) = pair(&mut db);
        fixture_preference(&mut db, "Prefer replies for reviewers", "role:auditor");
        let other_scope = entry(&db, "Prefer replies for reviewers");
        let legacy = db
            .record_feedback(&NewFeedback {
                statement: "Use short summaries",
                category: "communication",
                scope: "global",
                quote: "A retained legacy quote",
                source: "memory:old",
                at: "2026-09-01",
            })
            .unwrap();
        let before = db.aggregate().unwrap().profile_revision();
        assert!(replace(&mut db, &old, &old).is_err());
        assert!(replace(&mut db, &old, &other_scope).is_err());
        assert!(replace(&mut db, &old, &legacy).is_err());
        assert!(db
            .supersede_feedback(&old.key, &new.key, &"f".repeat(64), &new.review_revision())
            .is_err());
        assert!(db
            .supersede_feedback(&old.key, &new.key, &old.review_revision(), &"f".repeat(64))
            .is_err());
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        db.conn
            .execute(
                "UPDATE feedback SET category='process' WHERE key=?1",
                [&new.key],
            )
            .unwrap();
        let changed = entry(&db, &new.statement);
        let before = db.aggregate().unwrap().profile_revision();
        assert!(replace(&mut db, &old, &changed).is_err());
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        db.conn
            .execute(
                "UPDATE feedback SET category='communication' WHERE key=?1",
                [&new.key],
            )
            .unwrap();
        // Withdrawing a legacy predecessor requires no surviving source.
        assert!(replace(&mut db, &legacy, &new).is_ok());
    }

    #[test]
    fn concurrent_evidence_change_is_caught_again_inside_sql_transaction() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let mut db = ProfileStore::open(&path).unwrap();
        let (old, new) = pair(&mut db);
        let other = Connection::open(&path).unwrap();
        for key in [&old.key, &new.key] {
            other
                .execute(
                    "UPDATE feedback_evidence SET quote=quote||' changed' WHERE key=?1",
                    [key],
                )
                .unwrap();
            let before = db.aggregate().unwrap().profile_revision();
            assert!(replace(&mut db, &old, &new).is_err());
            assert_eq!(before, db.aggregate().unwrap().profile_revision());
            assert_eq!(
                db.feedback_relations(key).unwrap()["items"],
                serde_json::json!([])
            );
            other.execute("UPDATE feedback_evidence SET quote=substr(quote,1,length(quote)-8) WHERE key=?1", [key]).unwrap();
        }
    }

    #[test]
    fn second_review_event_failure_rolls_back_relation_statuses_and_pins() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let (old, new) = pair(&mut db);
        db.review_feedback(&old.key, "active", Some(&old.review_revision()))
            .unwrap();
        let before = db.aggregate().unwrap().profile_revision();
        let history = db.candidate_feedback_history(None, None).unwrap();
        db.conn.execute_batch("CREATE TRIGGER fail_new_review BEFORE INSERT ON feedback_review_event WHEN NEW.status LIKE 'supersedes:%' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(replace(&mut db, &old, &new).is_err());
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        assert_eq!(history, db.candidate_feedback_history(None, None).unwrap());
        assert_eq!(
            db.feedback_relations(&old.key).unwrap()["items"],
            serde_json::json!([])
        );
    }

    #[test]
    fn publication_failure_receipt_survives_a_later_rejection() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let markdown = temp.path().join("style.md");
        let mut db = ProfileStore::open(&path).unwrap();
        let (old, new) = pair(&mut db);
        assert!(publish_profile(
            &path,
            &markdown,
            false,
            |db| {
                let receipt = replace(db, &old, &new)?;
                std::fs::create_dir(&markdown)?;
                Ok(receipt)
            },
            |_| None
        )
        .is_err());
        assert_eq!(entry(&db, &old.statement).status, "superseded");
        assert_eq!(entry(&db, &new.statement).status, "active");
        db.review_feedback(&new.key, "rejected", None).unwrap();
        let before = db.aggregate().unwrap().profile_revision();
        let history = db.candidate_feedback_history(None, None).unwrap();
        std::fs::remove_dir(&markdown).unwrap();
        let (receipt, _) = publish_profile(
            &path,
            &markdown,
            false,
            |db| Ok(replace(db, &old, &new)?),
            |_| None,
        )
        .unwrap();
        assert!(receipt.repeated);
        assert_eq!(receipt.new_status, "rejected");
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        assert_eq!(history, db.candidate_feedback_history(None, None).unwrap());
        let text = std::fs::read_to_string(markdown).unwrap();
        assert!(!text.contains(&old.statement));
        assert!(!text.contains(&new.statement));
    }

    #[test]
    fn size_and_relation_count_gates_roll_back_the_complete_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = ProfileStore::open(&temp.path().join("style.db")).unwrap();
        let (old, new) = pair(&mut db);
        let before = db.aggregate().unwrap().profile_revision();
        db.conn.execute_batch("CREATE TABLE fixture_padding(data BLOB);
            CREATE TRIGGER grow_relation AFTER INSERT ON feedback_supersession BEGIN INSERT INTO fixture_padding VALUES(zeroblob(67108864)); END;").unwrap();
        assert!(replace(&mut db, &old, &new)
            .unwrap_err()
            .to_string()
            .contains("64 MiB"));
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        assert_eq!(
            db.conn
                .query_row("SELECT count(*) FROM fixture_padding", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        db.conn.execute_batch("DROP TRIGGER grow_relation;
            WITH RECURSIVE numbers(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM numbers WHERE n<20000)
            INSERT INTO feedback_supersession SELECT 'fixture-old-'||n,'fixture-new', 'old','new',0 FROM numbers;").unwrap();
        assert!(replace(&mut db, &old, &new)
            .unwrap_err()
            .to_string()
            .contains("20000"));
        assert_eq!(before, db.aggregate().unwrap().profile_revision());
        assert_eq!(
            db.conn
                .query_row("SELECT count(*) FROM feedback_supersession", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            20000
        );
    }

    #[test]
    fn rekey_moves_both_endpoints_preserves_review_digests_and_reset_clears_relations() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let mut db = ProfileStore::open(&path).unwrap();
        let (old, new) = pair(&mut db);
        replace(&mut db, &old, &new).unwrap();
        db.conn
            .execute("UPDATE feedback SET statement=statement||' please'", [])
            .unwrap();
        drop(db);
        let mut db = ProfileStore::open(&path).unwrap();
        let renamed_old = entry(&db, &format!("{} please", old.statement));
        let renamed_new = entry(&db, &format!("{} please", new.statement));
        assert_ne!(renamed_old.key, old.key);
        assert_ne!(renamed_new.key, new.key);
        assert_eq!(renamed_old.superseded_by, Some(renamed_new.key.clone()));
        let relations = db.feedback_relations(&renamed_new.key).unwrap();
        assert_eq!(relations["items"][0]["old_revision"], old.review_revision());
        assert_eq!(relations["items"][0]["new_revision"], new.review_revision());
        assert_ne!(
            renamed_new.accepted_revision,
            Some(renamed_new.review_revision())
        );
        assert!(db
            .feedback_supersession_retry(
                &renamed_old.key,
                &renamed_new.key,
                &old.review_revision(),
                &new.review_revision()
            )
            .unwrap()
            .is_some());
        db.reset().unwrap();
        assert_eq!(
            db.conn
                .query_row("SELECT count(*) FROM feedback_supersession", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(db.feedback().unwrap().is_empty());
    }

    #[test]
    fn readonly_store_without_relation_schema_remains_readable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("style.db");
        let mut db = ProfileStore::open(&path).unwrap();
        let (old, _) = pair(&mut db);
        db.conn
            .execute_batch("DROP TABLE feedback_supersession")
            .unwrap();
        drop(db);
        let db = ProfileStore::open_read_only(&path).unwrap();
        assert_eq!(entry(&db, &old.statement).superseded_by, None);
        assert_eq!(
            db.feedback_relations(&old.key).unwrap()["items"],
            serde_json::json!([])
        );
    }
}
