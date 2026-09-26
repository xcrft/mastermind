//! Explicit, reviewed replacement of a habit without reviving retired claims.

use super::{collection, digest_str, habit_review, Habit, ProfileStore, SqlResult};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

fn has_table(conn: &Connection) -> SqlResult<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='persona_habit_supersession')", [], |r| r.get(0))
}

/// Called inside the schema transaction. Preserve claim IDs and every dependent
/// evidence/receipt/pin, plus indexes and triggers on the original claim table.
pub(super) fn migrate_status(conn: &Connection) -> SqlResult<()> {
    let schema: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='persona_claim'",
        [],
        |r| r.get(0),
    )?;
    if schema.contains("'superseded'") {
        return Ok(());
    }
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('persona_claim') ORDER BY cid")?
        .query_map([], |r| r.get(0))?
        .collect::<SqlResult<_>>()?;
    if columns
        != [
            "id",
            "kind",
            "when_text",
            "behavior",
            "outcome",
            "exception_text",
            "scope",
            "role",
            "workflow",
            "status",
        ]
    {
        return Err(invalid("cannot migrate an unfamiliar habit schema"));
    }
    let objects: Vec<String> = conn.prepare("SELECT sql FROM sqlite_master WHERE tbl_name='persona_claim' AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name")?
        .query_map([], |r| r.get(0))?.collect::<SqlResult<_>>()?;
    conn.execute_batch("CREATE TABLE persona_claim_replacement (
        id INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK(kind='habit'),
        when_text TEXT NOT NULL, behavior TEXT NOT NULL, outcome TEXT NOT NULL,
        exception_text TEXT NOT NULL, scope TEXT NOT NULL, role TEXT NOT NULL, workflow TEXT NOT NULL,
        status TEXT NOT NULL CHECK(status IN ('candidate','observed','stale','rejected','superseded'))
    );
    INSERT INTO persona_claim_replacement(id,kind,when_text,behavior,outcome,exception_text,scope,role,workflow,status)
        SELECT id,kind,when_text,behavior,outcome,exception_text,scope,role,workflow,status FROM persona_claim;
    DROP TABLE persona_claim;
    ALTER TABLE persona_claim_replacement RENAME TO persona_claim;")?;
    for sql in objects {
        conn.execute_batch(&sql)?;
    }
    Ok(())
}

pub(super) fn has_successor(conn: &Connection, id: i64) -> SqlResult<bool> {
    if !has_table(conn)? {
        return Ok(false);
    }
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM persona_habit_supersession WHERE old_id=?1)",
        [id],
        |r| r.get(0),
    )
}

pub(super) fn load_relations(
    conn: &Connection,
    id: Option<i64>,
    habits: &mut [Habit],
) -> SqlResult<()> {
    let mut hashes: BTreeMap<_, _> = habits
        .iter()
        .map(|habit| {
            let mut hash = Sha256::new();
            hash.update(b"mastermind-habit-relations-v1\0");
            (habit.id, hash)
        })
        .collect();
    let mut successors = BTreeMap::new();
    if has_table(conn)? {
        let mut stmt = conn.prepare("SELECT old_id,new_id,old_revision,new_revision,reviewed_at
            FROM persona_habit_supersession WHERE (?1 IS NULL OR old_id=?1 OR new_id=?1) ORDER BY old_id")?;
        let mut rows = stmt.query([id])?;
        while let Some(row) = rows.next()? {
            let (old, new, old_pin, new_pin, at): (i64, i64, String, String, i64) = (
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            );
            for endpoint in [old, new] {
                if let Some(hash) = hashes.get_mut(&endpoint) {
                    hash.update(old.to_le_bytes());
                    hash.update(new.to_le_bytes());
                    digest_str(hash, &old_pin);
                    digest_str(hash, &new_pin);
                    hash.update(at.to_le_bytes());
                }
            }
            successors.insert(old, new);
        }
    }
    for habit in habits {
        habit.relations_revision =
            crate::hex::encode(&hashes.remove(&habit.id).expect("habit hash").finalize());
        habit.superseded_by = successors.remove(&habit.id);
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub(crate) struct HabitSupersessionReceipt {
    pub old_id: i64,
    pub new_id: i64,
    pub old_revision: String,
    pub new_revision: String,
    pub reviewed_at: i64,
    pub old_status: String,
    pub new_status: String,
    pub repeated: bool,
}

fn committed_receipt(
    conn: &Connection,
    old: i64,
    new: i64,
    old_revision: &str,
    new_revision: &str,
) -> SqlResult<Option<HabitSupersessionReceipt>> {
    if !has_table(conn)? {
        return Ok(None);
    }
    let previous: Option<(i64,String,String,i64)>=conn.query_row("SELECT new_id,old_revision,new_revision,reviewed_at FROM persona_habit_supersession WHERE old_id=?1", [old], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((target, old_pin, new_pin, at)) = previous else {
        return Ok(None);
    };
    if target != new || old_pin != old_revision || new_pin != new_revision {
        return Err(invalid(
            "habit already superseded by a different reviewed request; inspect its relations",
        ));
    }
    let status = |id| {
        conn.query_row("SELECT status FROM persona_claim WHERE id=?1", [id], |r| {
            r.get::<_, String>(0)
        })
    };
    Ok(Some(HabitSupersessionReceipt {
        old_id: old,
        new_id: new,
        old_revision: old_pin,
        new_revision: new_pin,
        reviewed_at: at,
        old_status: status(old)?,
        new_status: status(new)?,
        repeated: true,
    }))
}

impl ProfileStore {
    /// A committed retry precedes current revision/status/source checks and
    /// republishes current state without ever observing the successor again.
    pub(crate) fn habit_supersession_retry(
        &self,
        old: i64,
        new: i64,
        old_revision: &str,
        new_revision: &str,
    ) -> SqlResult<Option<HabitSupersessionReceipt>> {
        committed_receipt(&self.conn, old, new, old_revision, new_revision)
    }

    /// The caller verifies successor sources under the profile writer lock.
    pub(crate) fn supersede_habit(
        &mut self,
        old: i64,
        new: i64,
        old_revision: &str,
        new_revision: &str,
    ) -> SqlResult<HabitSupersessionReceipt> {
        if old <= 0 || new <= 0 || old == new {
            return Err(invalid(
                "replacement requires two different positive habit IDs",
            ));
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
        let Some(old_habit) = habit_review::read_habits(&tx, Some(old))?
            .into_iter()
            .next()
        else {
            return Err(invalid("old habit missing"));
        };
        let Some(new_habit) = habit_review::read_habits(&tx, Some(new))?
            .into_iter()
            .next()
        else {
            return Err(invalid("new habit missing"));
        };
        if old_habit.review_revision() != old_revision
            || new_habit.review_revision() != new_revision
        {
            return Err(invalid(
                "review revision changed; inspect both habits again",
            ));
        }
        if old_habit.scope != new_habit.scope
            || old_habit.role != new_habit.role
            || old_habit.workflow != new_habit.workflow
        {
            return Err(invalid(
                "replacement requires the same scope, role and workflow",
            ));
        }
        for habit in [&old_habit, &new_habit] {
            if !matches!(habit.status.as_str(), "candidate" | "stale" | "observed")
                || habit.retired_status().is_some()
            {
                return Err(invalid(
                    "rejected or superseded habits cannot take part in a new replacement",
                ));
            }
        }
        if let Some(reason) = new_habit.observation_problem() {
            return Err(invalid(reason));
        }
        // Both endpoints have no outgoing edge and the predecessor is unique.
        // Together with no self-link, this prevents cycles without graph walks.
        let at: i64 = tx.query_row("SELECT unixepoch()", [], |r| r.get(0))?;
        tx.execute("INSERT INTO persona_habit_supersession(old_id,new_id,old_revision,new_revision,reviewed_at) VALUES (?1,?2,?3,?4,?5)",params![old,new,old_revision,new_revision,at])?;
        tx.execute(
            "UPDATE persona_claim SET status='superseded' WHERE id=?1",
            [old],
        )?;
        tx.execute(
            "DELETE FROM persona_habit_observation WHERE claim_id=?1",
            [old],
        )?;
        tx.execute(
            "UPDATE persona_claim SET status='observed' WHERE id=?1",
            [new],
        )?;
        tx.execute("INSERT INTO persona_habit_observation(claim_id,revision) VALUES (?1,?2) ON CONFLICT(claim_id) DO UPDATE SET revision=excluded.revision",params![new,new_revision])?;
        for (id, event, revision) in [
            (
                old,
                format!("superseded:{}->{new}", old_habit.status),
                old_revision,
            ),
            (
                new,
                format!("supersedes:{old}:{}->observed", new_habit.status),
                new_revision,
            ),
        ] {
            tx.execute("INSERT INTO persona_review_event(claim_id,status,at_epoch,reviewed_revision) VALUES (?1,?2,?3,?4)", params![id,event,at,revision])?;
        }
        let count: i64 =
            tx.query_row("SELECT count(*) FROM persona_habit_supersession", [], |r| {
                r.get(0)
            })?;
        if count > 20_000 {
            return Err(invalid("habit relation history exceeds 20000 rows"));
        }
        collection::check_size(&tx)?;
        tx.commit()?;
        Ok(HabitSupersessionReceipt {
            old_id: old,
            new_id: new,
            old_revision: old_revision.into(),
            new_revision: new_revision.into(),
            reviewed_at: at,
            old_status: "superseded".into(),
            new_status: "observed".into(),
            repeated: false,
        })
    }

    pub(crate) fn habit_relations(&self, id: i64) -> SqlResult<serde_json::Value> {
        if !has_table(&self.conn)? {
            return Ok(serde_json::json!({"items":[],"truncated":false}));
        }
        let mut stmt=self.conn.prepare("SELECT r.old_id,r.new_id,r.old_revision,r.new_revision,r.reviewed_at,a.status,b.status
            FROM persona_habit_supersession r JOIN persona_claim a ON a.id=r.old_id JOIN persona_claim b ON b.id=r.new_id
            WHERE r.old_id=?1 OR r.new_id=?1 ORDER BY r.reviewed_at DESC,r.old_id LIMIT 21")?;
        let mut items:Vec<serde_json::Value>=stmt.query_map([id],|r|Ok(serde_json::json!({
            "relation":"supersedes","old_id":r.get::<_,i64>(0)?,"new_id":r.get::<_,i64>(1)?,
            "old_revision":r.get::<_,String>(2)?,"new_revision":r.get::<_,String>(3)?,"reviewed_at":r.get::<_,i64>(4)?,
            "old_status":r.get::<_,String>(5)?,"new_status":r.get::<_,String>(6)?
        })))?.collect::<SqlResult<_>>()?;
        let truncated = items.len() > 20;
        items.truncate(20);
        Ok(serde_json::json!({"items":items,"truncated":truncated}))
    }
}
