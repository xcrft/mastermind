//! Cold episode payloads. The journal retains identity, deduplication and review
//! bindings; immutable private files retain the exact cited source bytes. Once a
//! session has ended, rows derived from its archived episodes share the
//! episode's lifecycle: event receipts are dropped, because the archived episode
//! already holds its trace, and intake and task mining rows shrink to a summary
//! in the episode's metadata. Otherwise they would fill the journal for ever.

use super::*;
use crate::bounded_fs::AtomicWriteExpectation;

// One hook delivery must stay short. Pressure repeats on later events, so a
// large backlog drains over several compactions.
const MAX_DROPPED_RECEIPTS: i64 = 4096;
const MAX_RETIRED_EPISODES: i64 = 64;
// Session ids hash the generation, so no event can reach a session after its
// grant moves on, and such a session never records SessionEnd.
const SUPERSEDED: &str = "json_extract(s.data,'$.generation') IS NOT (SELECT g.generation
    FROM hook_grant g WHERE g.client=json_extract(s.data,'$.client')
    AND g.project_root=json_extract(s.data,'$.project_root'))";
// A session without SessionEnd may still be running, even when it has no
// active turn right now. Its rows still pair tool results and deduplicate replays.
fn ended() -> String {
    format!(
        "({SUPERSEDED} OR (json_extract(s.data,'$.started')=0
        AND json_type(s.data,'$.active') IS NOT 'text'
        AND EXISTS(SELECT 1 FROM hook_event x WHERE x.session=s.id AND x.kind='SessionEnd')))"
    )
}

fn target(conn: &Connection, digest: &str) -> Result<PathBuf, Error> {
    if !super::super::collection::valid_id(digest) {
        return Err("invalid episode archive digest".into());
    }
    let database = conn
        .path()
        .ok_or("episode archive requires a persistent journal")?;
    let parent = Path::new(database)
        .parent()
        .ok_or("journal parent unavailable")?;
    Ok(parent
        .join("persona-archive")
        .join(format!("{digest}.json")))
}

pub(super) fn read(conn: &Connection, expected: &str) -> Result<Episode, Error> {
    let (root, path) = bounded_fs::open_file_target(&target(conn, expected)?)?;
    let file = bounded_fs::read_regular_file_with_capability(
        &root,
        &path,
        MAX_EPISODE_BYTES as u64,
        MAX_EPISODE_BYTES as u64,
        ReadControl::default(),
    )?;
    if crate::hex::sha256(&file.bytes) != expected {
        return Err("episode archive digest changed".into());
    }
    Ok(serde_json::from_slice(&file.bytes)?)
}

fn write(conn: &Connection, episode: &Episode) -> Result<String, Error> {
    let bytes = serde_json::to_vec(episode)?;
    if bytes.len() > MAX_EPISODE_BYTES {
        return Err("episode archive exceeds its bound".into());
    }
    let key = crate::hex::sha256(&bytes);
    let (root, path) = bounded_fs::prepare_file_target(&target(conn, &key)?)?;
    if let Some(identity) = bounded_fs::inspect_direct_regular_file_identity_with_capability(
        &root,
        &path,
        ReadControl::default(),
    )? {
        let file = bounded_fs::read_regular_file_expected(
            &root,
            &path,
            MAX_EPISODE_BYTES as u64,
            MAX_EPISODE_BYTES as u64,
            ReadControl::default(),
            Some(identity),
        )?;
        if file.bytes != bytes {
            return Err("existing episode archive differs".into());
        }
    } else {
        let missing = bounded_fs::inspect_absent_path(&root, &path, ReadControl::default())?
            .ok_or(BoundedReadError::SnapshotChanged)?;
        bounded_fs::write_atomic_regular_file_expected_with_capability_mode(
            &root,
            &path,
            &bytes,
            0o600,
            AtomicWriteExpectation::Missing(missing),
        )?;
    }
    Ok(key)
}

/// Archives at most `limit` episodes. With a deadline it stops once the
/// deadline has passed, but only after one archive, so the caller always
/// frees at least one active slot.
pub(super) fn compact(
    conn: &Connection,
    root: Option<&Path>,
    limit: usize,
    deadline: Option<Instant>,
) -> Result<usize, Error> {
    // The active episode can still receive Stop, tools or next-turn context.
    // Queued work and reviewed citations resolve through the same archive reader.
    // Dropping first frees pages, because a full journal refuses the writes below.
    drop_event_receipts(conn)?;
    retire_derived(conn)?;
    let ids = {
        let mut stmt = conn.prepare(&format!(
            "SELECT e.id FROM hook_episode e JOIN hook_session s ON s.id=e.session
            WHERE json_type(e.data,'$.archive') IS NULL
            AND (coalesce(json_extract(s.data,'$.active'),'')!=e.id OR {SUPERSEDED})
            AND (?2 IS NULL OR json_extract(e.data,'$.project_root')=?2)
            ORDER BY json_extract(e.data,'$.observed_at'),e.id LIMIT ?1"
        ))?;
        let rows = stmt.query_map(
            params![
                limit.min(64) as i64,
                root.map(|path| path.to_string_lossy())
            ],
            |row| row.get::<_, String>(0),
        )?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut archived = 0;
    for id in &ids {
        if archived > 0 && deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        archived += 1;
        store(conn, &load_episode(conn, id)?)?;
    }
    drop_event_receipts(conn)?;
    retire_derived(conn)?;
    Ok(archived)
}

/// Moves the episode's text into a new archive file and keeps its metadata.
pub(super) fn store(conn: &Connection, episode: &Episode) -> Result<(), Error> {
    let key = write(conn, episode)?;
    let mut metadata = serde_json::to_value(episode)?;
    metadata["events"] = json!([]);
    metadata["exposures"] = json!([]);
    metadata["archive"] = json!(key);
    let retired: Option<String> = conn
        .query_row(
            "SELECT json_extract(data,'$.retired') FROM hook_episode WHERE id=?1",
            [&episode.id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if let Some(retired) = retired {
        metadata["retired"] = serde_json::from_str(&retired)?;
    }
    conn.execute(
        "UPDATE hook_episode SET data=?2 WHERE id=?1",
        params![episode.id, metadata.to_string()],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO hook_archive_file(digest,episode) VALUES(?1,?2)",
        params![key, episode.id],
    )?;
    Ok(())
}

// SessionStart and SessionEnd stay: they establish that a session ended, and a
// resumed session checks them. Receipts carry no payload and pressure comes
// from the shared journal, so this is not limited to one project.
fn drop_event_receipts(conn: &Connection) -> Result<(), Error> {
    conn.execute(
        &format!(
            "DELETE FROM hook_event WHERE rowid IN (
            SELECT e.rowid FROM hook_session s CROSS JOIN hook_event e ON e.session=s.id
            WHERE e.kind NOT IN ('SessionStart','SessionEnd') AND {}
            AND NOT EXISTS(SELECT 1 FROM hook_episode x WHERE x.id=e.episode
                AND json_type(x.data,'$.archive') IS NULL)
            LIMIT ?1)",
            ended()
        ),
        [MAX_DROPPED_RECEIPTS],
    )?;
    Ok(())
}

// An intake that a task binding cites stays, because the binding verifies its
// response. Unfinished task mining keeps its offer until the finalizer is done,
// and a pending intake until its hook can no longer publish it.
fn retire_derived(conn: &Connection) -> Result<(), Error> {
    let ids = {
        let mut stmt = conn.prepare(&format!(
            "SELECT e.id FROM hook_episode e JOIN hook_session s ON s.id=e.session
            WHERE json_type(e.data,'$.archive') IS NOT NULL AND json_type(e.data,'$.retired') IS NULL
            AND {}
            AND (EXISTS(SELECT 1 FROM hook_intake i WHERE i.episode=e.id)
                OR EXISTS(SELECT 1 FROM hook_task_mining t WHERE t.episode=e.id))
            AND NOT EXISTS(SELECT 1 FROM hook_task_mining t WHERE t.episode=e.id
                AND json_extract(t.data,'$.status')='submitted')
            AND NOT EXISTS(SELECT 1 FROM hook_task_finalization f WHERE f.episode=e.id AND f.completed=0)
            AND NOT EXISTS(SELECT 1 FROM hook_intake i WHERE i.episode=e.id
                AND json_extract(i.data,'$.status')='pending'
                AND coalesce(json_extract(i.data,'$.pending_until'),0)>=unixepoch())
            LIMIT ?1",
            ended()
        ))?;
        let rows = stmt.query_map([MAX_RETIRED_EPISODES], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for id in ids {
        let summary = json!({
            "intake": super::intake::intake_summary(conn, &id)?,
            "task_mining": super::task_mining::task_mining_receipt(conn, &id)?,
        });
        // Delete before the metadata grows, so a full journal can still retire.
        conn.execute(
            "DELETE FROM hook_intake WHERE episode=?1 AND id NOT IN (SELECT intake FROM hook_task_binding)",
            [&id],
        )?;
        conn.execute("DELETE FROM hook_task_mining WHERE episode=?1", [&id])?;
        conn.execute("DELETE FROM hook_task_finalization WHERE episode=?1", [&id])?;
        conn.execute(
            "UPDATE hook_episode SET data=json_set(data,'$.retired',json(?2)) WHERE id=?1",
            params![id, summary.to_string()],
        )?;
    }
    Ok(())
}

/// The summary that `retire_derived` kept for a derived row it deleted.
pub(super) fn retired(conn: &Connection, episode: &str, key: &str) -> Result<Option<Value>, Error> {
    let summary: Option<String> = conn
        .query_row(
            "SELECT json_extract(data,'$.retired') FROM hook_episode WHERE id=?1",
            [episode],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    Ok(summary
        .map(|summary| serde_json::from_str::<Value>(&summary))
        .transpose()?
        .map(|mut summary| summary[key].take())
        .filter(|value| !value.is_null()))
}

// Freed pages are reused, so they do not count toward the journal byte limit.
pub(super) fn used_bytes(conn: &Connection) -> Result<u64, Error> {
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let free: i64 = conn.query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
    let size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    Ok(((pages - free) * size) as u64)
}

// The limit SQLite enforces on this connection, which `open_target` sets.
pub(super) fn limit_bytes(conn: &Connection) -> Result<u64, Error> {
    let pages: i64 = conn.query_row("PRAGMA max_page_count", [], |row| row.get(0))?;
    let size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    Ok(pages.saturating_mul(size) as u64)
}

pub(super) fn queued_delete(conn: &Connection, id: &str) -> Result<(), Error> {
    conn.execute("INSERT OR IGNORE INTO hook_archive_delete(digest) SELECT digest FROM hook_archive_file WHERE episode=?1", [id])?;
    conn.execute("DELETE FROM hook_archive_file WHERE episode=?1", [id])?;
    Ok(())
}

pub(super) fn drain_deletes(conn: &Connection) -> Result<(), Error> {
    let keys = {
        let mut stmt = conn.prepare("SELECT digest FROM hook_archive_delete LIMIT 2001")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for key in keys {
        let path = target(conn, &key)?;
        let (root, path) = bounded_fs::open_file_target(&path)?;
        if let Some(identity) = bounded_fs::inspect_direct_regular_file_identity_with_capability(
            &root,
            &path,
            ReadControl::default(),
        )? {
            bounded_fs::remove_regular_file_expected_with_capability(&root, &path, identity)?;
        }
        conn.execute("DELETE FROM hook_archive_delete WHERE digest=?1", [key])?;
    }
    Ok(())
}

impl Journal {
    /// Each transaction keeps the delivery compaction budget, so hook
    /// deliveries waiting for the writer lock still fit their own budget.
    pub fn compact(&mut self, root: Option<&Path>, limit: usize) -> Result<usize, Error> {
        let mut total = 0;
        loop {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let count = compact(
                &tx,
                root,
                limit - total,
                Some(Instant::now() + COMPACTION_BUDGET),
            )?;
            tx.commit()?;
            total += count;
            if count == 0 || total >= limit {
                return Ok(total);
            }
        }
    }

    pub fn retention(&self) -> Result<Value, Error> {
        let (active, archived): (i64, i64) = self.conn.query_row(
            "SELECT
            coalesce(sum(json_type(data,'$.archive') IS NULL),0),
            coalesce(sum(json_type(data,'$.archive') IS NOT NULL),0) FROM hook_episode",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(
            json!({"active_episodes":active,"archived_episodes":archived,
            "active_limit":MAX_EPISODES,"automatic":true,
            "source_storage":"private_digest_verified_files","deletion":"explicit_forget_only",
            "pinned":"current_session_episode",
            "database_bytes":used_bytes(&self.conn)?,"database_byte_limit":MAX_BYTES}),
        )
    }
}
