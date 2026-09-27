//! New claim identities for an explicit return to a retired description.

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
    conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='persona_habit_generation')", [], |r| r.get(0))
}

/// Called after the status migration, inside the same immediate transaction.
pub(super) fn ensure_schema(conn: &Connection) -> SqlResult<()> {
    let has_generation = conn
        .prepare("SELECT 1 FROM pragma_table_info('persona_claim') WHERE name='generation'")?
        .exists([])?;
    if !has_generation {
        conn.execute_batch("ALTER TABLE persona_claim ADD COLUMN generation INTEGER NOT NULL DEFAULT 1 CHECK(generation>=1)")?;
    }
    let fields: Vec<String> = conn
        .prepare("SELECT name FROM pragma_index_info('persona_claim_unique') ORDER BY seqno")?
        .query_map([], |r| r.get(0))?
        .collect::<SqlResult<_>>()?;
    let original = [
        "kind",
        "when_text",
        "behavior",
        "outcome",
        "exception_text",
        "scope",
        "role",
        "workflow",
    ];
    let expected: Vec<_> = original.iter().copied().chain(["generation"]).collect();
    if fields != expected {
        if !fields.is_empty() && fields != original {
            return Err(invalid("cannot migrate an unfamiliar habit identity index"));
        }
        conn.execute_batch(
            "DROP INDEX IF EXISTS persona_claim_unique;
            CREATE UNIQUE INDEX persona_claim_unique ON persona_claim
            (kind,when_text,behavior,outcome,exception_text,scope,role,workflow,generation);",
        )?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS persona_habit_generation (
        parent_id INTEGER PRIMARY KEY, child_id INTEGER NOT NULL UNIQUE,
        root_id INTEGER NOT NULL, parent_revision TEXT NOT NULL,
        parent_status TEXT NOT NULL CHECK(parent_status IN ('rejected','superseded')),
        created_at INTEGER NOT NULL, CHECK(parent_id != child_id)
    );
    CREATE INDEX IF NOT EXISTS persona_habit_generation_root ON persona_habit_generation(root_id);",
    )
}

pub(super) fn has_child(conn: &Connection, id: i64) -> SqlResult<bool> {
    if !has_table(conn)? {
        return Ok(false);
    }
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM persona_habit_generation WHERE parent_id=?1)",
        [id],
        |r| r.get(0),
    )
}

pub(super) fn load_relations(
    conn: &Connection,
    id: Option<i64>,
    habits: &mut [Habit],
) -> SqlResult<()> {
    if !has_table(conn)? {
        return Ok(());
    }
    let mut hashes = BTreeMap::new();
    let mut parents = BTreeMap::new();
    let mut children = BTreeMap::new();
    let mut stmt = conn.prepare("SELECT parent_id,child_id,root_id,parent_revision,parent_status,created_at
        FROM persona_habit_generation WHERE (?1 IS NULL OR parent_id=?1 OR child_id=?1) ORDER BY parent_id")?;
    let mut rows = stmt.query([id])?;
    while let Some(row) = rows.next()? {
        let (parent, child, root, revision, status, at): (i64, i64, i64, String, String, i64) = (
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        );
        for endpoint in [parent, child] {
            let hash = hashes.entry(endpoint).or_insert_with(Sha256::new);
            hash.update(parent.to_le_bytes());
            hash.update(child.to_le_bytes());
            hash.update(root.to_le_bytes());
            digest_str(hash, &revision);
            digest_str(hash, &status);
            hash.update(at.to_le_bytes());
        }
        parents.insert(child, (parent, root));
        children.insert(parent, (child, status));
    }
    for habit in habits {
        if let Some((parent, root)) = parents.remove(&habit.id) {
            habit.renewed_from = Some(parent);
            habit.generation_root = root;
        }
        if let Some((child, status)) = children.remove(&habit.id) {
            habit.renewed_as = Some(child);
            habit.renewal_status = Some(status);
        }
        if let Some(mut hash) = hashes.remove(&habit.id) {
            hash.update(b"mastermind-habit-generations-v1\0");
            digest_str(&mut hash, &habit.relations_revision);
            habit.relations_revision = crate::hex::encode(&hash.finalize());
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub(crate) struct HabitRenewalReceipt {
    pub parent_id: i64,
    pub child_id: i64,
    pub root_id: i64,
    pub parent_revision: String,
    pub created_at: i64,
    pub generation: i64,
    pub child_status: String,
    pub repeated: bool,
}

fn committed_receipt(
    conn: &Connection,
    parent: i64,
    revision: &str,
) -> SqlResult<Option<HabitRenewalReceipt>> {
    if !has_table(conn)? {
        return Ok(None);
    }
    let receipt = conn.query_row("SELECT r.child_id,r.root_id,r.parent_revision,r.created_at,c.generation,c.status
        FROM persona_habit_generation r JOIN persona_claim c ON c.id=r.child_id WHERE r.parent_id=?1", [parent], |r| {
        Ok(HabitRenewalReceipt {parent_id:parent, child_id:r.get(0)?,root_id:r.get(1)?,parent_revision:r.get(2)?,
            created_at:r.get(3)?,generation:r.get(4)?,child_status:r.get(5)?,repeated:true})
    }).optional()?;
    if receipt
        .as_ref()
        .is_some_and(|item| item.parent_revision != revision)
    {
        return Err(invalid(
            "habit already renewed from a different reviewed revision; inspect its generations",
        ));
    }
    Ok(receipt)
}

impl ProfileStore {
    /// No source check: renewal copies only the description, never support or a pin.
    pub(crate) fn renew_habit(
        &mut self,
        parent: i64,
        revision: &str,
    ) -> SqlResult<HabitRenewalReceipt> {
        if parent <= 0 || !super::super::collection::valid_id(revision) {
            return Err(invalid(
                "renewal requires a positive habit ID and full review revision",
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(receipt) = committed_receipt(&tx, parent, revision)? {
            return Ok(receipt);
        }
        let old = habit_review::read_habits(&tx, Some(parent))?
            .into_iter()
            .next()
            .ok_or_else(|| invalid("parent habit missing"))?;
        if old.review_revision() != revision {
            return Err(invalid(
                "parent review revision changed; inspect habit show again",
            ));
        }
        let status = old
            .retired_status()
            .ok_or_else(|| invalid("only rejected or superseded habits can be renewed"))?;
        if [
            &old.when,
            &old.behavior,
            &old.outcome,
            &old.exception,
            &old.scope,
            &old.role,
            &old.workflow,
        ]
        .iter()
        .any(|s| s.len() > 800)
        {
            return Err(invalid("habit definition exceeds renewal metadata limits"));
        }
        let generation = old
            .generation
            .checked_add(1)
            .filter(|n| *n > 1)
            .ok_or_else(|| invalid("habit generation overflow"))?;
        tx.execute("INSERT INTO persona_claim(kind,when_text,behavior,outcome,exception_text,scope,role,workflow,status,generation)
            VALUES('habit',?1,?2,?3,?4,?5,?6,?7,'candidate',?8)", params![old.when,old.behavior,old.outcome,old.exception,old.scope,old.role,old.workflow,generation])?;
        let child = tx.last_insert_rowid();
        let at: i64 = tx.query_row("SELECT unixepoch()", [], |r| r.get(0))?;
        tx.execute("INSERT INTO persona_habit_generation(parent_id,child_id,root_id,parent_revision,parent_status,created_at)
            VALUES(?1,?2,?3,?4,?5,?6)",params![parent,child,old.generation_root,revision,status,at])?;
        tx.execute(
            "UPDATE persona_claim SET status=?2 WHERE id=?1",
            params![parent, status],
        )?;
        tx.execute(
            "DELETE FROM persona_habit_observation WHERE claim_id=?1",
            [parent],
        )?;
        tx.execute("INSERT INTO persona_review_event(claim_id,status,at_epoch,reviewed_revision) VALUES(?1,?2,?3,?4)",params![parent,format!("renewed_as:{child}"),at,revision])?;
        tx.execute("INSERT INTO persona_review_event(claim_id,status,at_epoch,reviewed_revision) VALUES(?1,?2,?3,NULL)",params![child,format!("renewed_from:{parent}"),at])?;
        let count: i64 =
            tx.query_row("SELECT count(*) FROM persona_habit_generation", [], |r| {
                r.get(0)
            })?;
        if count > 20_000 {
            return Err(invalid("habit generation history exceeds 20000 rows"));
        }
        collection::check_size(&tx)?;
        tx.commit()?;
        Ok(HabitRenewalReceipt {
            parent_id: parent,
            child_id: child,
            root_id: old.generation_root,
            parent_revision: revision.into(),
            created_at: at,
            generation,
            child_status: "candidate".into(),
            repeated: false,
        })
    }

    pub(crate) fn habit_generations(&self, id: i64) -> SqlResult<serde_json::Value> {
        if !has_table(&self.conn)? {
            return Ok(serde_json::json!({"items":[],"truncated":false}));
        }
        let root: i64 = self
            .conn
            .query_row(
                "SELECT root_id FROM persona_habit_generation WHERE child_id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(id);
        let mut stmt = self.conn.prepare("SELECT r.parent_id,r.child_id,r.parent_revision,r.parent_status,r.created_at,c.generation,c.status
            FROM persona_habit_generation r JOIN persona_claim c ON c.id=r.child_id
            WHERE r.root_id=?1 ORDER BY c.generation DESC,r.child_id DESC LIMIT 21")?;
        let mut items:Vec<serde_json::Value> = stmt.query_map([root],|r| Ok(serde_json::json!({
            "parent_id":r.get::<_,i64>(0)?,"child_id":r.get::<_,i64>(1)?,"parent_revision":r.get::<_,String>(2)?,
            "retained_parent_status":r.get::<_,String>(3)?,"created_at":r.get::<_,i64>(4)?,
            "generation":r.get::<_,i64>(5)?,"child_status":r.get::<_,String>(6)?
        })))?.collect::<SqlResult<_>>()?;
        let truncated = items.len() > 20;
        items.truncate(20);
        Ok(serde_json::json!({"root_id":root,"items":items,"truncated":truncated}))
    }
}
