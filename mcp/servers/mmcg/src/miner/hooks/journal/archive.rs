//! Cold episode payloads. The journal retains identity, deduplication and review
//! bindings; immutable private files retain the exact cited source bytes.

use super::*;
use crate::bounded_fs::AtomicWriteExpectation;
use sha2::{Digest, Sha256};

fn digest(bytes: &[u8]) -> String {
    crate::hex::encode(&Sha256::digest(bytes))
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
    if digest(&file.bytes) != expected {
        return Err("episode archive digest changed".into());
    }
    Ok(serde_json::from_slice(&file.bytes)?)
}

fn write(conn: &Connection, episode: &Episode) -> Result<String, Error> {
    let bytes = serde_json::to_vec(episode)?;
    if bytes.len() > MAX_EPISODE_BYTES {
        return Err("episode archive exceeds its bound".into());
    }
    let key = digest(&bytes);
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

pub(super) fn compact(
    conn: &Connection,
    root: Option<&Path>,
    limit: usize,
) -> Result<usize, Error> {
    // The active episode can still receive Stop, tools or next-turn context.
    // Queued work and reviewed citations resolve through the same archive reader.
    let ids = {
        let mut stmt = conn.prepare(
            "SELECT e.id FROM hook_episode e JOIN hook_session s ON s.id=e.session
            WHERE json_type(e.data,'$.archive') IS NULL
            AND coalesce(json_extract(s.data,'$.active'),'')!=e.id
            AND (?2 IS NULL OR json_extract(e.data,'$.project_root')=?2)
            ORDER BY json_extract(e.data,'$.observed_at'),e.id LIMIT ?1",
        )?;
        let rows = stmt.query_map(
            params![
                limit.min(64) as i64,
                root.map(|path| path.to_string_lossy())
            ],
            |row| row.get::<_, String>(0),
        )?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for id in &ids {
        let episode = load_episode(conn, id)?;
        let key = write(conn, &episode)?;
        conn.execute(
            "INSERT OR IGNORE INTO hook_archive_file(digest,episode) VALUES(?1,?2)",
            params![key, id],
        )?;
        let mut metadata = serde_json::to_value(&episode)?;
        metadata["events"] = json!([]);
        metadata["exposures"] = json!([]);
        metadata["archive"] = json!(key);
        conn.execute(
            "UPDATE hook_episode SET data=?2 WHERE id=?1",
            params![id, metadata.to_string()],
        )?;
    }
    Ok(ids.len())
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
    pub fn compact(&mut self, root: Option<&Path>, limit: usize) -> Result<usize, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count = compact(&tx, root, limit)?;
        tx.commit()?;
        Ok(count)
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
            "database_byte_limit":MAX_BYTES}),
        )
    }
}
