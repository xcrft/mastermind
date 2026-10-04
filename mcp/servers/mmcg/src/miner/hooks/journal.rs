//! Durable capture is separate from published persona state. A committed
//! pending counter fences readers before stdin is consumed; an interrupted
//! capture cannot leave its older evidence apparently current.

use super::influence::Influence;
use super::semantic::{EpisodeInput, EventInput, SemanticDraft};
use super::{hash, Error, EXTRACTOR};
use crate::bounded_fs::{self, BoundedReadError, ReadControl, RootCapability, StableFileIdentity};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod archive;
mod intake;
mod local;
pub(in crate::miner) mod task;

const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EPISODE_BYTES: usize = 512 * 1024;
const MAX_EVENTS: usize = 128;
const MAX_EPISODES: i64 = 2000;
const CAPTURE_VERSION: u32 = 3;
const INSPECTION_ATTEMPTS: usize = 3;

const SCHEMA: &str = "PRAGMA synchronous=FULL; PRAGMA journal_mode=DELETE;
                CREATE TABLE IF NOT EXISTS hook_grant (
                    client TEXT NOT NULL, project_root TEXT NOT NULL, generation INTEGER NOT NULL,
                    enabled INTEGER NOT NULL, pending INTEGER NOT NULL DEFAULT 0,
                    gap TEXT NOT NULL DEFAULT '', profile_client TEXT,
                    PRIMARY KEY(client,project_root));
                CREATE TABLE IF NOT EXISTS hook_session (id TEXT PRIMARY KEY, data TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS hook_episode (id TEXT PRIMARY KEY, session TEXT NOT NULL, data TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS hook_episode_session ON hook_episode(session,id);
                CREATE INDEX IF NOT EXISTS hook_episode_active ON hook_episode(id) WHERE json_type(data,'$.archive') IS NULL;
                CREATE TABLE IF NOT EXISTS hook_event (
                    id TEXT PRIMARY KEY, session TEXT NOT NULL, native_key TEXT NOT NULL,
                    digest TEXT NOT NULL, episode TEXT, tool_id TEXT, kind TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS hook_event_tool ON hook_event(session,tool_id,kind);
                CREATE TABLE IF NOT EXISTS hook_draft (id TEXT PRIMARY KEY, episode TEXT NOT NULL, data TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS hook_draft_episode ON hook_draft(episode,id);
                CREATE TABLE IF NOT EXISTS hook_analysis (
                    episode TEXT NOT NULL, revision TEXT NOT NULL, processor TEXT NOT NULL,
                    completed INTEGER NOT NULL DEFAULT 0, lease_until INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY(episode,revision,processor));
                CREATE TABLE IF NOT EXISTS hook_refiner (
                    client TEXT NOT NULL, project_root TEXT NOT NULL, revision INTEGER NOT NULL,
                    data TEXT, PRIMARY KEY(client,project_root));
                CREATE TABLE IF NOT EXISTS hook_profile_delivery (
                    client TEXT NOT NULL, project_root TEXT NOT NULL, disabled INTEGER NOT NULL,
                    PRIMARY KEY(client,project_root));
                CREATE TABLE IF NOT EXISTS hook_intake (
                    id TEXT PRIMARY KEY, episode TEXT NOT NULL UNIQUE, data TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS hook_task_binding (
                    revision TEXT PRIMARY KEY, intake TEXT NOT NULL UNIQUE,
                    project_root TEXT NOT NULL, spec_path TEXT NOT NULL, data TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS hook_task_binding_target ON hook_task_binding(project_root,spec_path);
                CREATE TABLE IF NOT EXISTS hook_local_queue (
                    episode TEXT PRIMARY KEY, attempts INTEGER NOT NULL DEFAULT 0,
                    next_attempt INTEGER NOT NULL DEFAULT 0);
                CREATE TABLE IF NOT EXISTS hook_archive_file (digest TEXT PRIMARY KEY, episode TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS hook_archive_episode ON hook_archive_file(episode);
                CREATE TABLE IF NOT EXISTS hook_archive_delete (digest TEXT PRIMARY KEY);";

pub(super) fn path() -> Result<PathBuf, Error> {
    Ok(std::env::home_dir()
        .ok_or("could not resolve home")?
        .join(".mastermind/persona-events.db"))
}

fn inspect_journal_file(
    root: &RootCapability,
    target: &Path,
    control: ReadControl<'_>,
) -> Result<Option<StableFileIdentity>, BoundedReadError> {
    let limit = Instant::now() + Duration::from_secs(2);
    let control = ReadControl {
        deadline: Some(
            control
                .deadline
                .map_or(limit, |deadline| deadline.min(limit)),
        ),
        ..control
    };
    let mut attempt = 0;
    loop {
        attempt += 1;
        // SQLite owns its descriptors and process-scoped POSIX locks. Inspect
        // metadata without opening another descriptor on Unix: closing one
        // would release an active connection's locks in this process.
        match bounded_fs::inspect_direct_regular_file_identity_with_capability(
            root, target, control,
        ) {
            Ok(Some(identity)) if identity.length() > MAX_BYTES => {
                return Err(BoundedReadError::TooLarge {
                    size: identity.length(),
                    limit: MAX_BYTES,
                });
            }
            Ok(identity) => return Ok(identity),
            Err(BoundedReadError::SnapshotChanged) if attempt < INSPECTION_ATTEMPTS => {
                // Commits change DB metadata and create/remove sidecars. Retry
                // only that observation, under the same root and deadline.
                root.verify()?;
            }
            Err(error) => return Err(error),
        }
    }
}

pub(super) struct Journal {
    conn: Connection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Grant {
    pub client: String,
    pub project_root: String,
    pub generation: i64,
    pub enabled: bool,
    pub pending: i64,
    pub gap: String,
    pub profile_client: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Session {
    #[serde(default)]
    capture_version: u32,
    id: String,
    native_id: String,
    client: String,
    project_root: String,
    project: String,
    repository: String,
    generation: i64,
    started: bool,
    gaps: Vec<String>,
    active: Option<String>,
    previous_assistant: Option<EventInput>,
    exposures: Vec<Value>,
    #[serde(default)]
    exposure_summaries_omitted: u64,
    #[serde(default)]
    influence: Influence,
    episode_count: usize,
    #[serde(default)]
    task_binding: Option<String>,
    #[serde(default)]
    task_epoch: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Episode {
    pub id: String,
    pub session: String,
    pub client: String,
    pub project_root: String,
    pub project: String,
    pub repository: String,
    pub generation: i64,
    pub turn_id: Option<String>,
    pub observed_at: String,
    pub events: Vec<EventInput>,
    pub gaps: Vec<String>,
    pub closed: bool,
    pub open_tools: Vec<String>,
    pub exposures: Vec<Value>,
    #[serde(default)]
    pub prior_exposure_summaries_omitted: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Draft {
    pub id: String,
    pub revision: String,
    pub episode: String,
    pub episode_revision: String,
    pub content: SemanticDraft,
    pub attested: bool,
    pub attested_episode: Option<String>,
    pub processor: Value,
}

pub(super) struct Incoming {
    pub native_session: String,
    pub native_turn: Option<String>,
    pub native_key: Option<String>,
    pub digest: String,
    pub kind: String,
    pub actor: String,
    pub origin: String,
    pub text: String,
    pub tool_id: Option<String>,
    pub gap: Option<String>,
    pub forked: bool,
    pub fresh_start: bool,
    pub profile_exposure: Option<Value>,
}

impl Journal {
    pub fn open(write: bool) -> Result<Self, Error> {
        Self::open_target(&path()?, write)
    }

    fn open_target(target: &Path, write: bool) -> Result<Self, Error> {
        let (root, target) = if write {
            bounded_fs::prepare_file_target(target)?
        } else {
            bounded_fs::open_file_target(target)?
        };
        let control = ReadControl {
            deadline: Some(Instant::now() + Duration::from_secs(2)),
            interrupted: None,
        };
        let identity = match inspect_journal_file(&root, &target, control)? {
            Some(identity) => identity,
            None if write => {
                match bounded_fs::create_regular_file_with_capability(&root, &target, true) {
                    Ok((file, identity)) => {
                        file.sync_all()?;
                        // The private creation descriptor must close before
                        // SQLite can acquire any locks on the new database.
                        drop(file);
                        identity
                    }
                    Err(BoundedReadError::Io(error))
                        if error.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        inspect_journal_file(&root, &target, control)?
                            .ok_or(BoundedReadError::SnapshotChanged)?
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            None => {
                return Err(BoundedReadError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "hook journal does not exist",
                ))
                .into())
            }
        };
        // SQLite owns the rollback journal. Reject planted special files before
        // SQLite can open them. WAL is deliberately not enabled for this store.
        for suffix in ["-journal", "-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", target.display()));
            inspect_journal_file(&root, &sidecar, control)?;
        }
        let flags = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let conn = Connection::open_with_flags(
            &target,
            flags | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        root.verify()?;
        let current = inspect_journal_file(&root, &target, control)?
            .ok_or(BoundedReadError::SnapshotChanged)?;
        if !current.same_object(identity) {
            return Err("hook journal changed while opening".into());
        }
        conn.busy_timeout(Duration::from_millis(500))?;
        if write {
            conn.execute_batch(SCHEMA)?;
            let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
            conn.pragma_update(None, "max_page_count", MAX_BYTES as i64 / page_size)?;
            archive::drain_deletes(&conn)?;
        }
        Ok(Self { conn })
    }

    pub fn grant(&self, client: &str, root: &Path) -> Result<Option<Grant>, Error> {
        read_grant(&self.conn, client, root)
    }

    pub fn configure(
        &mut self,
        client: &str,
        root: &Path,
        enabled: bool,
        profile_client: Option<&str>,
        recover: bool,
    ) -> Result<Grant, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO hook_grant(client,project_root,generation,enabled,profile_client) VALUES(?1,?2,1,?3,?4)
            ON CONFLICT(client,project_root) DO UPDATE SET
            generation=hook_grant.generation + CASE WHEN hook_grant.enabled != excluded.enabled OR ?5 THEN 1 ELSE 0 END,
            enabled=excluded.enabled, profile_client=excluded.profile_client,
            pending=CASE WHEN ?5 THEN 0 ELSE hook_grant.pending END,
            gap=CASE WHEN ?5 THEN '' ELSE hook_grant.gap END",
            params![client,root.to_string_lossy(),enabled,profile_client,recover])?;
        let grant = read_grant(&tx, client, root)?.ok_or("capture grant unavailable")?;
        tx.commit()?;
        Ok(grant)
    }

    pub fn begin_capture(&mut self, client: &str, root: &Path) -> Result<Option<Grant>, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count = tx.execute("UPDATE hook_grant SET pending=pending+1 WHERE client=?1 AND project_root=?2 AND enabled=1 AND gap=''",
            params![client,root.to_string_lossy()])?;
        if count == 0 {
            tx.commit()?;
            return Ok(None);
        }
        let grant = read_grant(&tx, client, root)?;
        tx.commit()?;
        Ok(grant)
    }

    pub fn finish_ignored(&self, grant: &Grant, gap: Option<&str>) -> Result<(), Error> {
        self.conn.execute("UPDATE hook_grant SET pending=max(pending-1,0),gap=CASE WHEN ?4 IS NULL THEN gap ELSE ?4 END WHERE client=?1 AND project_root=?2 AND generation=?3",
            params![grant.client,grant.project_root,grant.generation,gap])?;
        Ok(())
    }

    fn session(&self, id: &str) -> Result<Option<Session>, Error> {
        self.conn
            .query_row("SELECT data FROM hook_session WHERE id=?1", [id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .map(|s| Ok(serde_json::from_str(&s)?))
            .transpose()
    }

    pub fn capture_activation(
        &self,
        client: &str,
        root: &Path,
        generation: i64,
    ) -> Result<Value, Error> {
        let project = super::profile::persona_project_id(root);
        let repository = super::profile::persona_repository_id(root).unwrap_or_default();
        let mut statement = self.conn.prepare("SELECT data FROM hook_session WHERE json_extract(data,'$.client')=?1 AND json_extract(data,'$.project_root')=?2 AND json_extract(data,'$.generation')=?3 LIMIT 129")?;
        let rows = statement
            .query_map(params![client, root.to_string_lossy(), generation], |row| {
                row.get::<_, String>(0)
            })?;
        let mut scanned = 0;
        let mut active = 0;
        let mut observed = 0;
        let mut complete = project.is_some();
        for row in rows {
            scanned += 1;
            if scanned > 128 {
                complete = false;
                break;
            }
            let session: Session = serde_json::from_str(&row?)?;
            if session.capture_version == CAPTURE_VERSION
                && session.gaps.is_empty()
                && Some(&session.project) == project.as_ref()
                && session.repository == repository
            {
                let started: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM hook_event WHERE session=?1 AND kind='SessionStart')", [&session.id], |row| row.get(0))?;
                observed += usize::from(started);
                active += usize::from(started && session.started);
            }
        }
        Ok(
            json!({"status":if !complete {"incomplete"} else if observed > 0 {"session_start_observed"} else {"not_observed"},
            "capture_generation":generation,"source":"local_unverified_native_event",
            "current_sessions":if complete {Some(active)} else {None},"session_start_observations":if complete {Some(observed)} else {None},"complete":complete}),
        )
    }

    pub fn episode(&self, id: &str) -> Result<Episode, Error> {
        load_episode(&self.conn, id)
    }

    pub fn profile_delivery_disabled(&self, client: &str, root: &Path) -> Result<bool, Error> {
        profile_delivery_disabled(&self.conn, client, root)
    }

    pub fn configure_profile_delivery(
        &mut self,
        client: &str,
        root: &Path,
        disabled: bool,
    ) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO hook_profile_delivery(client,project_root,disabled) VALUES(?1,?2,?3)
             ON CONFLICT(client,project_root) DO UPDATE SET disabled=excluded.disabled",
            params![client, root.to_string_lossy(), disabled],
        )?;
        Ok(())
    }

    /// Aggregate this exact client's journal without returning source text or
    /// identities. Completeness concerns recorded capture metadata, not truth,
    /// authorship, independence or review eligibility of a semantic claim.
    pub fn capture_evidence_summary(&self, client: &str, root: &Path) -> Result<Value, Error> {
        let project = super::profile::persona_project_id(root);
        let pending = super::fence::pending(client, root).unwrap_or(true);
        let tx = self.conn.unchecked_transaction()?;
        let Some(grant) = read_grant(&tx, client, root)? else {
            return Ok(json!({"status":"not_configured","eligibility":"capture_metadata_only"}));
        };
        let episodes = {
            let mut statement = tx.prepare("SELECT id,json_extract(data,'$.generation') FROM hook_episode WHERE json_type(data,'$.archive') IS NULL AND json_extract(data,'$.client')=?1 AND json_extract(data,'$.project_root')=?2 LIMIT ?3")?;
            let rows = statement.query_map(
                params![client, root.to_string_lossy(), MAX_EPISODES + 1],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if episodes.len() as i64 > MAX_EPISODES {
            return Err("capture evidence summary exceeds the episode bound".into());
        }
        let mut current = 0;
        let mut complete = 0;
        let mut historical = 0;
        let mut gaps = BTreeMap::<String, u64>::new();
        for (id, generation) in episodes {
            if generation != grant.generation {
                historical += 1;
                continue;
            }
            current += 1;
            let snapshot = snapshot_at(&tx, &id)?;
            let mut reasons = snapshot.coverage_gaps;
            if project.as_deref() != Some(snapshot.project.as_str()) {
                reasons.push("project_identity_changed".into());
            }
            if pending {
                reasons.push("capture_delivery_pending_or_unavailable".into());
            }
            reasons.sort();
            reasons.dedup();
            complete += usize::from(reasons.is_empty());
            for reason in reasons {
                *gaps.entry(reason).or_default() += 1;
            }
        }
        let mut summary = json!({
            "status":"available","eligibility":"capture_metadata_only",
            "current":{"generation":grant.generation,"episodes":current,
                "complete_episodes":complete,"incomplete_episodes":current-complete,"coverage_gaps":gaps},
            "historical":{"episodes":historical},
            "meaning":"Complete capture metadata is not proof of human authorship, semantic truth or an accepted personal habit."
        });
        let archived: i64 = tx.query_row("SELECT count(*) FROM hook_episode WHERE json_type(data,'$.archive') IS NOT NULL AND json_extract(data,'$.client')=?1 AND json_extract(data,'$.project_root')=?2", params![client,root.to_string_lossy()], |row| row.get(0))?;
        summary["archived"] = json!({"episodes":archived,"source_verification":"on_demand"});
        for (field, sql) in [
            ("events", "SELECT count(*),coalesce(sum(json_extract(s.data,'$.generation')=?3),0) FROM hook_event e JOIN hook_session s ON s.id=e.session WHERE json_extract(s.data,'$.client')=?1 AND json_extract(s.data,'$.project_root')=?2"),
            ("drafts", "SELECT count(*),coalesce(sum(json_extract(e.data,'$.generation')=?3),0) FROM hook_draft d JOIN hook_episode e ON e.id=d.episode WHERE json_extract(e.data,'$.client')=?1 AND json_extract(e.data,'$.project_root')=?2"),
            ("completed_analyses", "SELECT count(*),coalesce(sum(json_extract(e.data,'$.generation')=?3),0) FROM hook_analysis a JOIN hook_episode e ON e.id=a.episode WHERE a.completed=1 AND json_extract(e.data,'$.client')=?1 AND json_extract(e.data,'$.project_root')=?2"),
        ] {
            let (total, count) = tx.query_row(
                sql,
                params![client, root.to_string_lossy(), grant.generation],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?;
            summary["current"][field] = json!(count);
            summary["historical"][field] = json!(total - count);
        }
        tx.commit()?;
        Ok(summary)
    }

    pub fn snapshot(&self, id: &str) -> Result<EpisodeInput, Error> {
        let tx = self.conn.unchecked_transaction()?;
        let mut snapshot = snapshot_at(&tx, id)?;
        tx.commit()?;
        if super::fence::pending(&snapshot.client, Path::new(&snapshot.project_root))
            .unwrap_or(true)
        {
            snapshot
                .coverage_gaps
                .push("capture_delivery_pending_or_unavailable".into());
        }
        // Git and filesystem checks can block. They must never hold SQLite
        // readers across native process I/O or starve a capture's writer.
        if super::super::profile::persona_project_id(Path::new(&snapshot.project_root)).as_deref()
            != Some(snapshot.project.as_str())
        {
            snapshot
                .coverage_gaps
                .push("project_identity_changed".into());
        }
        Ok(snapshot)
    }

    pub fn receive(
        &mut self,
        grant: &Grant,
        incoming: Incoming,
        project: &str,
        repository: &str,
    ) -> Result<Value, Error> {
        let sid = hash(&json!([
            "hook-session-v1",
            grant.client,
            grant.project_root,
            grant.generation,
            incoming.native_session
        ]));
        let mut session = self.session(&sid)?.unwrap_or_else(|| Session {
            capture_version: CAPTURE_VERSION,
            id: sid.clone(),
            native_id: incoming.native_session.clone(),
            client: grant.client.clone(),
            project_root: grant.project_root.clone(),
            project: project.into(),
            repository: repository.into(),
            generation: grant.generation,
            started: false,
            gaps: vec![],
            active: None,
            previous_assistant: None,
            exposures: vec![],
            exposure_summaries_omitted: 0,
            influence: if incoming.fresh_start {
                Influence::fresh()
            } else {
                Influence::default()
            },
            episode_count: 0,
            task_binding: None,
            task_epoch: 0,
        });
        let key = incoming
            .native_key
            .clone()
            .unwrap_or_else(|| format!("payload:{}", incoming.digest));
        let event_id = hash(&json!([sid, key]));
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Re-read session while holding the SQLite writer transaction. A hook
        // on another process may have committed since the preliminary read.
        if let Some(data) = tx
            .query_row("SELECT data FROM hook_session WHERE id=?1", [&sid], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
        {
            session = serde_json::from_str(&data)?;
        } else {
            // A capture generation is not a new human interaction. Reusing a
            // native session after recovery cannot erase prior context offers.
            let known: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM hook_session WHERE json_extract(data,'$.client')=?1 AND json_extract(data,'$.project_root')=?2 AND json_extract(data,'$.native_id')=?3)",
                params![grant.client,grant.project_root,incoming.native_session], |row| row.get(0))?;
            if known {
                session.influence = Influence::default();
            }
        }
        let active_generation = tx.prepare("SELECT 1 FROM hook_grant WHERE client=?1 AND project_root=?2 AND generation=?3 AND enabled=1")?
            .exists(params![grant.client,grant.project_root,grant.generation])?;
        if !active_generation {
            return Err("capture was revoked during delivery".into());
        }
        let existing: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT digest,episode FROM hook_event WHERE id=?1",
                [&event_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((digest, episode)) = existing {
            let conflict = digest != incoming.digest;
            if conflict {
                add_gap(&mut session.gaps, "event_identity_conflict");
            } else if incoming.native_key.is_none() && incoming.kind == "UserPromptSubmit" {
                add_gap(&mut session.gaps, "ambiguous_prompt_replay");
            }
            if incoming.native_key.is_none() && incoming.kind == "SessionEnd" && session.started {
                // Without a native identity this may be the resumed session ending again.
                add_gap(&mut session.gaps, "ambiguous_session_lifecycle");
                session.started = false;
                session.active = None;
                session.previous_assistant = None;
            }
            save_session(&tx, &session)?;
            finish(&tx, grant)?;
            tx.commit()?;
            return Ok(
                json!({"status":if conflict {"conflict"} else {"duplicate"},"event_id":event_id,"episode":episode}),
            );
        }
        if incoming.forked {
            add_gap(&mut session.gaps, "fork_or_delegated_session");
        }
        if session.project != project || session.repository != repository {
            add_gap(&mut session.gaps, "project_identity_changed");
        }
        if let Some(exposure) = &incoming.profile_exposure {
            session.influence.offer_profile();
            push_exposure(&mut session, exposure.clone());
        }
        let event = EventInput {
            influence: session.influence,
            id: event_id.clone(),
            kind: incoming.kind.clone(),
            actor: incoming.actor,
            origin: incoming.origin,
            text: incoming.text,
        };
        if incoming.kind == "SessionStart" {
            if !session.started {
                session.active = None;
                session.previous_assistant = None;
            }
            session.started = true;
        }
        let mut target = session.active.clone();
        let mut revised_episode = None;
        if incoming.kind == "UserPromptSubmit" {
            if let Some(turn) = &incoming.native_turn {
                if !episodes_for_turn(&tx, &sid, turn)?.is_empty() {
                    add_gap(&mut session.gaps, "ambiguous_turn_identity");
                }
            }
            if let Some(previous) = &session.active {
                let mut ep = load_episode(&tx, previous)?;
                if !ep.closed {
                    add_gap(&mut ep.gaps, "next_prompt_before_stop");
                    if ep.turn_id.is_none() || incoming.native_turn.is_none() {
                        // Without both turn identities, a later Stop cannot
                        // establish which of the overlapping requests ended.
                        add_gap(&mut session.gaps, "ambiguous_turn_overlap");
                    }
                }
                let mut correction = event.clone();
                correction.origin = "next_turn_context".into();
                if let Some(gap) = &incoming.gap {
                    // The next prompt also supplies contradiction context to
                    // the preceding episode. Missing text affects both uses.
                    add_gap(&mut ep.gaps, gap);
                }
                push_event(&mut ep, correction);
                save_episode(&tx, &ep)?;
                revised_episode = Some(ep.id.clone());
            }
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM hook_episode WHERE json_type(data,'$.archive') IS NULL",
                [],
                |r| r.get(0),
            )?;
            let pages: i64 = tx.query_row("PRAGMA page_count", [], |row| row.get(0))?;
            let free: i64 = tx.query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
            let size: i64 = tx.query_row("PRAGMA page_size", [], |row| row.get(0))?;
            if count >= MAX_EPISODES || (pages - free) * size > (MAX_BYTES * 3 / 4) as i64 {
                archive::compact(&tx, None, 32)?;
            }
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM hook_episode WHERE json_type(data,'$.archive') IS NULL",
                [],
                |r| r.get(0),
            )?;
            if count >= MAX_EPISODES {
                return Err(
                    "hook journal active episode limit reached; remaining episodes are still in use".into(),
                );
            }
            let id = hash(&json!(["hook-episode-v1", sid, event_id]));
            let mut ep = Episode {
                id: id.clone(),
                session: sid.clone(),
                client: grant.client.clone(),
                project_root: grant.project_root.clone(),
                project: project.into(),
                repository: repository.into(),
                generation: grant.generation,
                turn_id: incoming.native_turn.clone(),
                observed_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs()
                    .to_string(),
                events: vec![],
                gaps: vec![],
                closed: false,
                open_tools: vec![],
                exposures: session.exposures.clone(),
                prior_exposure_summaries_omitted: session.exposure_summaries_omitted,
            };
            if !session.started {
                add_gap(&mut ep.gaps, "missing_session_start");
            }
            if let Some(mut previous) = session.previous_assistant.clone() {
                previous.origin = "prior_turn_context".into();
                ep.events.push(previous);
            }
            save_episode(&tx, &ep)?;
            target = Some(id.clone());
            session.active = Some(id);
            session.episode_count += 1;
        } else if matches!(incoming.kind.as_str(), "Stop" | "Interrupt" | "StopFailure") {
            if let Some(turn) = &incoming.native_turn {
                let matches = episodes_for_turn(&tx, &sid, turn)?;
                target = match matches.as_slice() {
                    [episode] => Some(episode.clone()),
                    [] => {
                        add_gap(&mut session.gaps, "unmatched_turn_event");
                        None
                    }
                    _ => {
                        add_gap(&mut session.gaps, "ambiguous_turn_identity");
                        None
                    }
                };
            }
        } else if matches!(incoming.kind.as_str(), "PostToolUse" | "PostToolUseFailure") {
            if let Some(tool_id) = &incoming.tool_id {
                let found: Option<String> = tx.query_row("SELECT episode FROM hook_event WHERE session=?1 AND tool_id=?2 AND kind='PreToolUse' ORDER BY rowid DESC LIMIT 1",
                    params![sid,tool_id], |r| r.get(0)).optional()?.flatten();
                if found.is_some() {
                    target = found;
                }
            }
        }
        if let Some(id) = &target {
            let mut ep = load_episode(&tx, id)?;
            if let Some(gap) = &incoming.gap {
                // A known event belongs to this episode, including a late
                // tool result. It cannot invalidate unrelated past/future
                // episodes merely because its retained text is unavailable.
                add_gap(&mut ep.gaps, gap);
            }
            if let Some(exposure) = incoming.profile_exposure {
                ep.exposures.push(exposure);
            }
            if incoming.native_turn.is_some()
                && ep.turn_id.is_some()
                && incoming.native_turn != ep.turn_id
            {
                add_gap(&mut ep.gaps, "turn_identity_mismatch");
            }
            if incoming.kind == "PreToolUse" {
                match &incoming.tool_id {
                    Some(id) => ep.open_tools.push(id.clone()),
                    None => add_gap(&mut ep.gaps, "missing_tool_identity"),
                }
            } else if matches!(incoming.kind.as_str(), "PostToolUse" | "PostToolUseFailure") {
                match &incoming.tool_id {
                    Some(id) if ep.open_tools.contains(id) => {
                        ep.open_tools.retain(|open| open != id)
                    }
                    _ => add_gap(&mut ep.gaps, "unpaired_tool_result"),
                }
            } else if incoming.kind == "Stop" {
                ep.closed = true;
                if session.started && session.active.as_ref() == Some(id) {
                    session.previous_assistant = (!event.text.is_empty()).then(|| event.clone());
                }
            } else if matches!(
                incoming.kind.as_str(),
                "Interrupt" | "PreCompact" | "StopFailure"
            ) {
                add_gap(&mut ep.gaps, "interrupted_or_compacted_turn");
            }
            push_event(&mut ep, event);
            save_episode(&tx, &ep)?;
            if ep.closed {
                // SessionEnd and late tool/lifecycle events also change the
                // evidence revision after Stop. Retry that final source.
                revised_episode = Some(ep.id.clone());
            }
        } else {
            if let Some(gap) = &incoming.gap {
                add_gap(&mut session.gaps, gap);
            }
            if !matches!(incoming.kind.as_str(), "SessionStart" | "SessionEnd") {
                add_gap(&mut session.gaps, "event_without_user_turn");
            }
        }
        if incoming.kind == "SessionEnd" {
            session.started = false;
            session.active = None;
            session.previous_assistant = None;
        }
        tx.execute("INSERT INTO hook_event(id,session,native_key,digest,episode,tool_id,kind) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![event_id,sid,key,incoming.digest,target,incoming.tool_id,incoming.kind])?;
        save_session(&tx, &session)?;
        finish(&tx, grant)?;
        tx.commit()?;
        Ok(
            json!({"status":"recorded","event_id":event_id,"episode":target,
            "revised_episode":revised_episode}),
        )
    }

    pub fn expose(&mut self, grant: &Grant, episode: &str, packet: &Value) -> Result<(), Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut ep = load_episode(&tx, episode)?;
        let current = read_grant(&tx, &grant.client, Path::new(&grant.project_root))?
            .ok_or("hook profile delivery grant is unavailable")?;
        if !current.enabled
            || current.generation != grant.generation
            || current.profile_client.is_none()
            || current.profile_client != grant.profile_client
            || ep.client != grant.client
            || ep.project_root != grant.project_root
            || ep.generation != grant.generation
        {
            return Err("hook profile delivery is no longer authorized for this capture".into());
        }
        let mut session: Session = serde_json::from_str(&tx.query_row(
            "SELECT data FROM hook_session WHERE id=?1",
            [&ep.session],
            |r| r.get::<_, String>(0),
        )?)?;
        if !session.started
            || session.active.as_deref() != Some(episode)
            || ep.closed
            || session.capture_version != CAPTURE_VERSION
            || !session.gaps.is_empty()
            || !ep.gaps.is_empty()
            || current.pending != 0
            || !current.gap.is_empty()
        {
            return Err("hook profile delivery requires the active open prompt".into());
        }
        let claims = |field: &str, key: &str| {
            packet[field].as_array().map(|items| items.iter().take(32)
            .map(|item|json!({"id":item[key],"review_revision":item["review_revision"]})).collect::<Vec<_>>()).unwrap_or_default()
        };
        if self::profile_delivery_disabled(&tx, &grant.client, Path::new(&grant.project_root))? {
            return Err("hook profile delivery is disabled".into());
        }
        let receipt = json!({"status":"offered","packet_digest":hash(packet),"profile_revision":packet.get("profile_revision"),
            "selection":packet.get("selection"),
            "feedback":claims("feedback","key"),"habits":claims("habits","id")});
        push_exposure(&mut session, receipt.clone());
        ep.exposures.push(receipt);
        session.influence.offer_profile();
        save_session(&tx, &session)?;
        save_episode(&tx, &ep)?;
        tx.commit()?;
        Ok(())
    }

    pub fn list(&self, root: &Path, limit: usize, after: &str) -> Result<Vec<Value>, Error> {
        // Finish the SQL cursor before source verification can invoke Git.
        // Otherwise an apparently finished snapshot still holds a read lock
        // through this outer statement and stalls latency-sensitive capture.
        let rows = {
            let mut stmt = self.conn.prepare(
                "SELECT id,data FROM hook_episode WHERE id>?1
                 AND json_extract(data,'$.project_root')=?2 ORDER BY id LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(
                    params![after, root.to_string_lossy(), limit.min(101) as i64],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let mut result = vec![];
        for (id, data) in rows {
            let ep = load_episode(&self.conn, &id)?;
            let snapshot = self.snapshot(&id)?;
            result.push(json!({"id":id,"revision":snapshot.revision,"client":ep.client,"closed":ep.closed,
                "events":ep.events.len(),"archived":serde_json::from_str::<Value>(&data)?["archive"].is_string(),"coverage_gaps":snapshot.coverage_gaps,"profile_influenced":snapshot.profile_influenced}));
        }
        Ok(result)
    }

    pub fn data_version(&self) -> Result<i64, Error> {
        Ok(self
            .conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }

    pub fn local_analysis_count(
        &self,
        client: &str,
        root: &Path,
        generation: i64,
    ) -> Result<i64, Error> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM hook_analysis a JOIN hook_episode e ON e.id=a.episode
             WHERE a.completed=1 AND a.processor=?1
             AND json_extract(e.data,'$.client')=?2
             AND json_extract(e.data,'$.project_root')=?3
             AND json_extract(e.data,'$.generation')=?4",
            params![
                hash(&super::local::processor()),
                client,
                root.to_string_lossy(),
                generation
            ],
            |row| row.get(0),
        )?)
    }

    pub fn profile_offer_count(
        &self,
        client: &str,
        root: &Path,
        generation: i64,
    ) -> Result<i64, Error> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM hook_episode e, json_each(e.data,'$.exposures') x
             WHERE json_extract(e.data,'$.client')=?1
             AND json_extract(e.data,'$.project_root')=?2
             AND json_extract(e.data,'$.generation')=?3
             AND json_extract(x.value,'$.status')='offered'",
            params![client, root.to_string_lossy(), generation],
            |row| row.get(0),
        )?)
    }

    pub fn store_drafts(
        &mut self,
        input: &EpisodeInput,
        drafts: &[SemanticDraft],
        processor: Value,
    ) -> Result<Vec<Draft>, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = snapshot_at(&tx, &input.id)?;
        if current.revision != input.revision || !current.coverage_gaps.is_empty() {
            return Err("episode changed during semantic analysis".into());
        }
        let mut result = vec![];
        for content in drafts {
            // A context append changes the revision, not the identity of an
            // unchanged hypothesis. This preserves the reviewed rebind path.
            let id = hash(&json!([EXTRACTOR, input.id, content]));
            let prior: Option<String> = tx
                .query_row("SELECT data FROM hook_draft WHERE id=?1", [&id], |r| {
                    r.get(0)
                })
                .optional()?;
            let prior = prior
                .map(|data| serde_json::from_str::<Draft>(&data))
                .transpose()?;
            let draft = Draft {
                id: id.clone(),
                revision: hash(&json!([id, input.revision, processor])),
                episode: input.id.clone(),
                episode_revision: input.revision.clone(),
                content: content.clone(),
                attested: false,
                attested_episode: prior.as_ref().and_then(|old| old.attested_episode.clone()),
                processor: processor.clone(),
            };
            if prior
                .as_ref()
                .is_none_or(|old| old.episode_revision != input.revision)
            {
                tx.execute("INSERT INTO hook_draft(id,episode,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
                    params![id,input.id,serde_json::to_string(&draft)?])?;
            }
            // Preserve an existing attestation on an exact idempotent replay.
            let data: String =
                tx.query_row("SELECT data FROM hook_draft WHERE id=?1", [&id], |r| {
                    r.get(0)
                })?;
            result.push(serde_json::from_str(&data)?);
        }
        tx.execute(
            "INSERT INTO hook_analysis(episode,revision,processor,completed) VALUES(?1,?2,?3,1)
            ON CONFLICT(episode,revision,processor) DO UPDATE SET completed=1,lease_until=0",
            params![input.id, input.revision, hash(&processor)],
        )?;
        tx.commit()?;
        Ok(result)
    }

    pub fn draft(&self, id: &str) -> Result<Draft, Error> {
        let data: String =
            self.conn
                .query_row("SELECT data FROM hook_draft WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
        Ok(serde_json::from_str(&data)?)
    }

    pub fn claim_analysis(
        &mut self,
        id: &str,
        revision: &str,
        processor: &Value,
        timeout: u64,
    ) -> Result<Option<i64>, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = snapshot_at(&tx, id)?;
        if current.revision != revision || !current.coverage_gaps.is_empty() {
            return Ok(None);
        }
        let claimed=tx.execute("INSERT INTO hook_analysis(episode,revision,processor,lease_until) VALUES(?1,?2,?3,unixepoch()+?4)
            ON CONFLICT(episode,revision,processor) DO UPDATE SET lease_until=excluded.lease_until
            WHERE hook_analysis.completed=0 AND hook_analysis.lease_until<=unixepoch()",
            params![id,revision,hash(processor),(timeout+30) as i64])?;
        let lease = if claimed == 1 {
            Some(tx.query_row("SELECT lease_until FROM hook_analysis WHERE episode=?1 AND revision=?2 AND processor=?3",params![id,revision,hash(processor)],|r|r.get(0))?)
        } else {
            None
        };
        tx.commit()?;
        Ok(lease)
    }

    pub fn analysis_failed(
        &self,
        id: &str,
        revision: &str,
        processor: &Value,
        lease: i64,
    ) -> Result<(), Error> {
        self.conn.execute("UPDATE hook_analysis SET lease_until=0 WHERE episode=?1 AND revision=?2 AND processor=?3 AND completed=0 AND lease_until=?4",
            params![id,revision,hash(processor),lease])?;
        Ok(())
    }

    pub fn draft_receipts(&self, episode: &str) -> Result<Vec<Value>, Error> {
        let snapshot = self.snapshot(episode)?;
        let mut stmt = self
            .conn
            .prepare("SELECT data FROM hook_draft WHERE episode=?1 ORDER BY id LIMIT 101")?;
        let rows = stmt.query_map([episode], |r| r.get::<_, String>(0))?;
        rows.map(|row| { let draft:Draft=serde_json::from_str(&row?)?;
            Ok(json!({"id":draft.id,"revision":draft.revision,"episode_revision":draft.episode_revision,"attested":draft.attested,
                "evidence_class":super::semantic::evidence_class(&snapshot,&draft.content).ok(),
                "promotion_eligible":snapshot.revision==draft.episode_revision && super::promotion_eligible(&snapshot,&draft).is_ok()})) }).collect()
    }

    pub fn review_queue(&self, root: &Path) -> Result<Value, Error> {
        // Metadata only, bounded to the selected repository. Finish the SQL
        // cursor before verifying any source or calling Git.
        let mut statement = self.conn.prepare("SELECT d.data FROM hook_draft d JOIN hook_episode e ON d.episode=e.id WHERE json_extract(e.data,'$.project_root')=?1 AND json_extract(d.data,'$.attested')=0 ORDER BY d.id LIMIT 9")?;
        let rows = statement
            .query_map([root.to_string_lossy()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let truncated = rows.len() > 8;
        let mut items = Vec::new();
        let mut snapshots = std::collections::BTreeMap::new();
        for row in rows.iter().take(8) {
            let draft: Draft = serde_json::from_str(row)?;
            let snapshot = snapshots
                .entry(draft.episode.clone())
                .or_insert_with(|| self.snapshot(&draft.episode).ok());
            let current = snapshot.as_ref().filter(|source| {
                source.revision == draft.episode_revision && source.coverage_gaps.is_empty()
            });
            let class = current
                .and_then(|source| super::semantic::evidence_class(source, &draft.content).ok());
            let eligible =
                current.is_some_and(|source| super::promotion_eligible(source, &draft).is_ok());
            items.push(json!({"id":draft.id,"kind":"hook_habit_candidate","status":"authorship_review_required",
                "source_status":if current.is_some() {"current"} else {"stale_or_unavailable"},
                "evidence_class":class,"promotion_eligible":eligible,"source_id":draft.episode,
                "episode_id":draft.episode,"source_revision":draft.episode_revision,
                "support_count":draft.content.supports.len(),"contradiction_count":draft.content.contradictions.len()}));
        }
        Ok(
            json!({"status":"observed","scope":"unattested_hook_drafts_in_selected_repository",
            "total":if truncated {None} else {Some(items.len())},"returned":items.len(),"truncated":truncated,
            "items":items,"authority":"unreviewed_observations_only"}),
        )
    }

    pub fn attested_drafts(&self, session: &str) -> Result<Vec<Draft>, Error> {
        let mut stmt=self.conn.prepare("SELECT d.data FROM hook_draft d JOIN hook_episode e ON d.episode=e.id WHERE e.session=?1 AND json_extract(d.data,'$.attested')=1 ORDER BY d.id LIMIT 513")?;
        let rows = stmt.query_map([session], |r| r.get::<_, String>(0))?;
        let mut result = Vec::new();
        for row in rows {
            let draft: Draft = serde_json::from_str(&row?)?;
            if draft.attested {
                result.push(draft);
            }
        }
        if result.len() > 512 {
            return Err("session draft limit reached".into());
        }
        Ok(result)
    }

    pub fn attest(&mut self, id: &str, revision: &str, episode: &str) -> Result<Draft, Error> {
        let prepared = self.draft(id)?;
        let verified = self.snapshot(&prepared.episode)?;
        super::promotion_eligible(&verified, &prepared)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let data: String = tx.query_row("SELECT data FROM hook_draft WHERE id=?1", [id], |r| {
            r.get(0)
        })?;
        let mut draft: Draft = serde_json::from_str(&data)?;
        if draft.revision != revision {
            return Err("draft revision changed".into());
        }
        if draft
            .attested_episode
            .as_deref()
            .is_some_and(|old| old != episode)
        {
            return Err(
                "attested task identity cannot be relabeled as an independent episode".into(),
            );
        }
        let input = snapshot_at(&tx, &draft.episode)?;
        if input.revision != draft.episode_revision {
            return Err("episode changed; analyze and inspect again".into());
        }
        super::semantic::validate_for_promotion(&input, std::slice::from_ref(&draft.content))?;
        draft.attested = true;
        draft.attested_episode = Some(episode.to_owned());
        tx.execute(
            "UPDATE hook_draft SET data=?2 WHERE id=?1",
            params![id, serde_json::to_string(&draft)?],
        )?;
        tx.commit()?;
        Ok(draft)
    }

    pub fn forget(&mut self, id: &str, revision: &str) -> Result<(), Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if snapshot_at(&tx, id)?.revision != revision {
            return Err("episode revision changed".into());
        }
        let forgotten = load_episode(&tx, id)?;
        let event_ids: std::collections::HashSet<_> = forgotten
            .events
            .iter()
            .filter(|e| {
                !matches!(
                    e.origin.as_str(),
                    "prior_turn_context" | "next_turn_context"
                )
            })
            .map(|e| e.id.as_str())
            .collect();
        let mut session: Session = serde_json::from_str(&tx.query_row(
            "SELECT data FROM hook_session WHERE id=?1",
            [&forgotten.session],
            |r| r.get::<_, String>(0),
        )?)?;
        if session.active.as_deref() == Some(id) {
            session.active = None;
        }
        if session
            .previous_assistant
            .as_ref()
            .is_some_and(|e| event_ids.contains(e.id.as_str()))
        {
            session.previous_assistant = None;
        }
        add_gap(&mut session.gaps, "episode_forgotten");
        save_session(&tx, &session)?;
        let mut stmt = tx.prepare("SELECT id FROM hook_episode WHERE session=?1 AND id!=?2")?;
        let retained = stmt
            .query_map(params![forgotten.session, id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for retained_id in retained {
            let mut ep = load_episode(&tx, &retained_id)?;
            ep.events.retain(|e| !event_ids.contains(e.id.as_str()));
            archive::queued_delete(&tx, &retained_id)?;
            save_episode(&tx, &ep)?;
        }
        archive::queued_delete(&tx, id)?;
        // Drafts can quote cross-turn context. Drop this session's derived
        // hypotheses along with every copy of the removed raw event text.
        tx.execute("DELETE FROM hook_draft WHERE episode IN (SELECT id FROM hook_episode WHERE session=?1)",[&forgotten.session])?;
        tx.execute("DELETE FROM hook_episode WHERE id=?1", [id])?;
        tx.execute("DELETE FROM hook_event WHERE episode=?1", [id])?;
        tx.execute("DELETE FROM hook_analysis WHERE episode=?1", [id])?;
        tx.execute("DELETE FROM hook_intake WHERE episode=?1", [id])?;
        tx.execute("DELETE FROM hook_local_queue WHERE episode=?1", [id])?;
        tx.commit()?;
        archive::drain_deletes(&self.conn)?;
        Ok(())
    }
}

fn finish(conn: &Connection, grant: &Grant) -> Result<(), Error> {
    conn.execute("UPDATE hook_grant SET pending=max(pending-1,0) WHERE client=?1 AND project_root=?2 AND generation=?3",
        params![grant.client,grant.project_root,grant.generation])?;
    Ok(())
}
fn add_gap(gaps: &mut Vec<String>, gap: &str) {
    if !gaps.iter().any(|old| old == gap) && gaps.len() < 32 {
        gaps.push(gap.into());
    }
}
fn push_event(ep: &mut Episode, event: EventInput) {
    if ep.events.len() >= MAX_EVENTS {
        add_gap(&mut ep.gaps, "episode_event_limit");
    } else {
        ep.events.push(event);
    }
}
fn load_episode(conn: &Connection, id: &str) -> Result<Episode, Error> {
    let data: String = conn.query_row("SELECT data FROM hook_episode WHERE id=?1", [id], |r| {
        r.get(0)
    })?;
    let value: Value = serde_json::from_str(&data)?;
    if let Some(key) = value["archive"].as_str() {
        let archived = archive::read(conn, key)?;
        if archived.id != id
            || value["client"] != archived.client
            || value["project_root"] != archived.project_root
            || value["generation"] != archived.generation
        {
            return Err("episode archive identity differs from its journal reference".into());
        }
        Ok(archived)
    } else {
        Ok(serde_json::from_value(value)?)
    }
}

fn push_exposure(session: &mut Session, receipt: Value) {
    if session.exposures.contains(&receipt) {
        return;
    }
    // Event-level influence and each episode's delivery receipt remain durable.
    // A bounded session summary must not turn normal repeated delivery into a
    // capture gap or erase the fact that later input has prior exposure.
    if session.exposures.len() >= 32 {
        session.exposures.remove(0);
        session.exposure_summaries_omitted = session.exposure_summaries_omitted.saturating_add(1);
    }
    session.exposures.push(receipt);
}

fn profile_delivery_disabled(conn: &Connection, client: &str, root: &Path) -> Result<bool, Error> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='hook_profile_delivery')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(false);
    }
    Ok(conn
        .query_row(
            "SELECT disabled FROM hook_profile_delivery WHERE client=?1 AND project_root=?2",
            params![client, root.to_string_lossy()],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false))
}

fn episodes_for_turn(conn: &Connection, session: &str, turn: &str) -> Result<Vec<String>, Error> {
    // The session key already includes capture generation. Two matches are
    // enough to reject an ambiguous identity, never choose the latest match.
    let mut statement = conn.prepare(
        "SELECT id FROM hook_episode WHERE session=?1 AND json_extract(data,'$.turn_id')=?2 LIMIT 2",
    )?;
    let rows = statement.query_map(params![session, turn], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
fn save_session(conn: &Connection, session: &Session) -> Result<(), Error> {
    conn.execute("INSERT INTO hook_session(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![session.id,serde_json::to_string(session)?])?;
    Ok(())
}
fn save_episode(conn: &Connection, ep: &Episode) -> Result<(), Error> {
    let data = serde_json::to_string(ep)?;
    if data.len() > MAX_EPISODE_BYTES {
        return Err("hook episode exceeds 512 KiB; capture is fenced until recovery".into());
    }
    conn.execute("INSERT INTO hook_episode(id,session,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![ep.id,ep.session,data])?;
    Ok(())
}

fn read_grant(conn: &Connection, client: &str, root: &Path) -> Result<Option<Grant>, Error> {
    Ok(conn.query_row("SELECT generation,enabled,pending,gap,profile_client FROM hook_grant WHERE client=?1 AND project_root=?2",
            params![client,root.to_string_lossy()], |r| Ok(Grant { client:client.into(), project_root:root.to_string_lossy().into_owned(),
                generation:r.get(0)?,enabled:r.get(1)?,pending:r.get(2)?,gap:r.get(3)?,profile_client:r.get(4)? })).optional()?)
}

fn snapshot_at(conn: &Connection, id: &str) -> Result<EpisodeInput, Error> {
    let ep = load_episode(conn, id)?;
    let session: Session = serde_json::from_str(&conn.query_row(
        "SELECT data FROM hook_session WHERE id=?1",
        [&ep.session],
        |r| r.get::<_, String>(0),
    )?)?;
    let grant = read_grant(conn, &ep.client, Path::new(&ep.project_root))?
        .ok_or("capture grant unavailable")?;
    let mut gaps = ep.gaps.clone();
    gaps.extend(session.gaps.iter().cloned());
    if session.capture_version != CAPTURE_VERSION {
        gaps.push("legacy_capture_semantics".into());
    }
    if !grant.enabled || grant.generation != ep.generation {
        gaps.push("capture_revoked_or_restarted".into());
    }
    if grant.pending != 0 {
        gaps.push("capture_pending_or_interrupted".into());
    }
    if !grant.gap.is_empty() {
        gaps.push(grant.gap.clone());
    }
    if !ep.closed {
        gaps.push("no_stop_observed".into());
    }
    if !ep.open_tools.is_empty() {
        gaps.push("missing_tool_result".into());
    }
    gaps.sort();
    gaps.dedup();
    let revision = hash(&json!([
        EXTRACTOR,
        super::semantic::PROSE_VERSION,
        super::influence::VERSION,
        ep,
        session.capture_version,
        session.gaps,
        grant.generation,
        grant.enabled,
        grant.gap
    ]));
    Ok(EpisodeInput {
        id: ep.id,
        revision,
        client: ep.client,
        project_root: ep.project_root,
        project: ep.project,
        events: ep.events,
        coverage_gaps: gaps,
        profile_influenced: !ep.exposures.is_empty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn journal_reopen_preserves_active_sqlite_transaction_locks() {
        const CHILD_DATABASE: &str = "MMCG_HOOK_JOURNAL_LOCK_TEST_DATABASE";
        const CHILD_EXPECT_BUSY: &str = "MMCG_HOOK_JOURNAL_LOCK_TEST_EXPECT_BUSY";
        if let Some(path) = std::env::var_os(CHILD_DATABASE) {
            let connection = Connection::open_with_flags(
                Path::new(&path),
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .unwrap();
            connection.busy_timeout(Duration::ZERO).unwrap();
            let result = connection.execute_batch("BEGIN IMMEDIATE; ROLLBACK;");
            if std::env::var(CHILD_EXPECT_BUSY).unwrap() == "1" {
                assert!(
                    matches!(
                        result,
                        Err(ref error)
                            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy)
                    ),
                    "another process acquired the active transaction's lock: {result:?}"
                );
            } else {
                result.unwrap();
            }
            return;
        }

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap().join("events.db");
        let journal = Journal::open_target(&path, true).unwrap();
        let check_child = |phase: &str, expect_busy: bool| {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "miner::hooks::journal::tests::journal_reopen_preserves_active_sqlite_transaction_locks",
                    "--nocapture",
                ])
                .env(CHILD_DATABASE, &path)
                .env(CHILD_EXPECT_BUSY, if expect_busy { "1" } else { "0" })
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{phase}, busy={expect_busy}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };

        journal.conn.execute_batch("BEGIN IMMEDIATE;").unwrap();
        check_child("before second open", true);
        let observer = Journal::open_target(&path, false).unwrap();
        check_child("after second open", true);
        drop(observer);
        check_child("after observer drop", true);
        journal.conn.execute_batch("ROLLBACK;").unwrap();
        check_child("after rollback", false);
    }

    #[test]
    fn journal_inspection_resamples_metadata_with_bounded_attempts_and_deadline() {
        use std::cell::Cell;
        use std::io::Write;

        for continuous in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("changing.db");
            std::fs::write(&path, b"initial").unwrap();
            let root = RootCapability::open(directory.path()).unwrap();
            let checks = Cell::new(0);
            let changes = Cell::new(0);
            let mutate_after_metadata = || {
                checks.set(checks.get() + 1);
                if checks.get() % 3 == 2 && (continuous || changes.get() == 0) {
                    let mut file = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&path)
                        .unwrap();
                    file.write_all(b"+").unwrap();
                    changes.set(changes.get() + 1);
                }
                false
            };
            let result = inspect_journal_file(
                &root,
                &path,
                ReadControl {
                    deadline: Some(Instant::now() + Duration::from_secs(1)),
                    interrupted: Some(&mutate_after_metadata),
                },
            );
            if continuous {
                assert!(matches!(result, Err(BoundedReadError::SnapshotChanged)));
                assert_eq!(changes.get(), INSPECTION_ATTEMPTS);
            } else {
                assert_eq!(result.unwrap().unwrap().length(), 8);
                assert_eq!(checks.get(), 6, "repeat the entire metadata inspection");
                assert_eq!(changes.get(), 1);
            }
            assert!(matches!(
                inspect_journal_file(
                    &root,
                    &path,
                    ReadControl {
                        deadline: Some(Instant::now()),
                        interrupted: None,
                    }
                ),
                Err(BoundedReadError::DeadlineExceeded)
            ));
        }
    }

    #[test]
    fn journal_open_keeps_missing_private_size_and_sqlite_settings_contracts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private").join("events.db");
        assert!(Journal::open_target(&path, false).is_err());
        assert!(!path.parent().unwrap().exists());
        let journal = Journal::open_target(&path, true).unwrap();
        assert_eq!(
            journal
                .conn
                .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            500
        );
        assert_eq!(
            journal
                .conn
                .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "delete"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(journal);

        for suffix in ["", "-journal", "-wal", "-shm"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("events.db");
            drop(Journal::open_target(&path, true).unwrap());
            let oversized = PathBuf::from(format!("{}{suffix}", path.display()));
            std::fs::File::create(&oversized)
                .unwrap()
                .set_len(MAX_BYTES + 1)
                .unwrap();
            let error = Journal::open_target(&path, false)
                .err()
                .expect("oversized SQLite file must fail before open");
            assert!(
                matches!(
                    error.downcast_ref::<BoundedReadError>(),
                    Some(BoundedReadError::TooLarge { .. })
                ),
                "{suffix}: {error}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn journal_inspection_never_reopens_a_replaced_root_or_follows_symlinks() {
        use std::cell::Cell;
        use std::os::unix::fs::symlink;

        for replace_root in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let parent = directory.path().join("parent");
            std::fs::create_dir(&parent).unwrap();
            let path = parent.join("events.db");
            std::fs::write(&path, b"initial").unwrap();
            let root = RootCapability::open(&parent).unwrap();
            let checks = Cell::new(0);
            let replace_after_metadata = || {
                checks.set(checks.get() + 1);
                if checks.get() == 2 {
                    if replace_root {
                        std::fs::rename(&parent, directory.path().join("old")).unwrap();
                        std::fs::create_dir(&parent).unwrap();
                        std::fs::write(&path, b"replacement").unwrap();
                    } else {
                        let outside = directory.path().join("outside.db");
                        std::fs::write(&outside, b"outside").unwrap();
                        std::fs::remove_file(&path).unwrap();
                        symlink(&outside, &path).unwrap();
                    }
                }
                false
            };
            assert!(inspect_journal_file(
                &root,
                &path,
                ReadControl {
                    deadline: None,
                    interrupted: Some(&replace_after_metadata)
                }
            )
            .is_err());
            assert_eq!(checks.get(), 2, "unsafe paths must not be resampled");
        }

        for suffix in ["", "-journal", "-wal", "-shm"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("events.db");
            drop(Journal::open_target(&path, true).unwrap());
            let linked = PathBuf::from(format!("{}{suffix}", path.display()));
            if suffix.is_empty() {
                std::fs::remove_file(&linked).unwrap();
            }
            let outside = directory.path().join("outside.db");
            std::fs::write(&outside, b"unchanged").unwrap();
            symlink(&outside, &linked).unwrap();
            assert!(Journal::open_target(&path, true).is_err(), "{suffix}");
            assert_eq!(std::fs::read(&outside).unwrap(), b"unchanged");
        }
    }

    fn journal() -> Journal {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        Journal { conn }
    }

    fn deliver(db: &mut Journal, root: &Path, event: Value) -> Value {
        let grant = db.begin_capture("codex", root).unwrap().unwrap();
        db.receive(
            &grant,
            super::super::normalize(&event).unwrap(),
            "project",
            "repository",
        )
        .unwrap()
    }

    #[test]
    fn replay_keeps_its_original_episode_after_a_later_prompt() {
        let root = tempfile::tempdir().unwrap();
        let mut db = journal();
        db.configure("codex", root.path(), true, None, false)
            .unwrap();
        let start = json!({"session_id":"s", "hook_event_name":"SessionStart"});
        assert!(deliver(&mut db, root.path(), start.clone())["episode"].is_null());
        let first = json!({"session_id":"s", "turn_id":"one", "hook_event_name":"UserPromptSubmit", "prompt":"First request"});
        let original = deliver(&mut db, root.path(), first.clone());
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"one", "hook_event_name":"Stop"}),
        );
        let next = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"two", "hook_event_name":"UserPromptSubmit", "prompt":"Second request"}),
        );
        assert_ne!(original["episode"], next["episode"]);

        let replay = deliver(&mut db, root.path(), first.clone());
        assert_eq!(replay["status"], "duplicate");
        assert_eq!(replay["episode"], original["episode"]);
        assert!(deliver(&mut db, root.path(), start)["episode"].is_null());

        let mut changed = first;
        changed["prompt"] = json!("Different text with an old identity");
        let conflict = deliver(&mut db, root.path(), changed);
        assert_eq!(conflict["status"], "conflict");
        assert_eq!(conflict["episode"], original["episode"]);
        assert_eq!(db.grant("codex", root.path()).unwrap().unwrap().pending, 0);
        assert!(snapshot_at(&db.conn, next["episode"].as_str().unwrap())
            .unwrap()
            .coverage_gaps
            .contains(&"event_identity_conflict".to_owned()));
    }

    #[test]
    fn a_late_unidentified_stop_cannot_certify_an_overlapping_prompt() {
        let root = tempfile::tempdir().unwrap();
        let mut db = journal();
        db.configure("codex", root.path(), true, None, false)
            .unwrap();
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "hook_event_name":"SessionStart"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "hook_event_name":"UserPromptSubmit", "prompt":"First request"}),
        );
        let second = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "hook_event_name":"UserPromptSubmit", "prompt":"Second request"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "hook_event_name":"Stop", "last_assistant_message":"Result for the first request"}),
        );
        let snapshot = snapshot_at(&db.conn, second["episode"].as_str().unwrap()).unwrap();
        assert!(snapshot
            .coverage_gaps
            .contains(&"ambiguous_turn_overlap".to_owned()));

        // An unidentified Stop cannot repair the session's ordering evidence.
        let third = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "hook_event_name":"UserPromptSubmit", "prompt":"Third request"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "hook_event_name":"Stop", "last_assistant_message":"Result for another request"}),
        );
        assert!(snapshot_at(&db.conn, third["episode"].as_str().unwrap())
            .unwrap()
            .coverage_gaps
            .contains(&"ambiguous_turn_overlap".to_owned()));
    }

    #[test]
    fn profile_delivery_rechecks_capture_generation_and_audience() {
        for change in ["revoke", "recover", "audience"] {
            let root = tempfile::tempdir().unwrap();
            let mut db = journal();
            let grant = db
                .configure("codex", root.path(), true, Some("reader-one"), false)
                .unwrap();
            deliver(
                &mut db,
                root.path(),
                json!({"session_id":"s", "hook_event_name":"SessionStart"}),
            );
            let receipt = deliver(
                &mut db,
                root.path(),
                json!({"session_id":"s", "turn_id":"one", "hook_event_name":"UserPromptSubmit", "prompt":"Original request"}),
            );
            db.configure(
                "codex",
                root.path(),
                change != "revoke",
                Some(if change == "audience" {
                    "reader-two"
                } else {
                    "reader-one"
                }),
                change == "recover",
            )
            .unwrap();
            let episode = receipt["episode"].as_str().unwrap();
            assert!(
                db.expose(&grant, episode, &json!({"profile_revision":"old-view"}))
                    .is_err(),
                "{change}"
            );
            assert!(db.episode(episode).unwrap().exposures.is_empty());
        }
    }

    #[test]
    fn earlier_capture_semantics_cannot_certify_stored_evidence() {
        let root = tempfile::tempdir().unwrap();
        let mut db = journal();
        db.configure("codex", root.path(), true, None, false)
            .unwrap();
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "hook_event_name":"SessionStart"}),
        );
        let receipt = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"one", "hook_event_name":"UserPromptSubmit", "prompt":"Original request"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"one", "hook_event_name":"Stop"}),
        );
        let id = receipt["episode"].as_str().unwrap();
        let current = snapshot_at(&db.conn, id).unwrap();
        assert!(current.coverage_gaps.is_empty());
        let session = db.episode(id).unwrap().session;
        let mut legacy = serde_json::to_value(db.session(&session).unwrap().unwrap()).unwrap();
        legacy.as_object_mut().unwrap().remove("capture_version");
        db.conn
            .execute(
                "UPDATE hook_session SET data=?1 WHERE id=?2",
                params![legacy.to_string(), session],
            )
            .unwrap();
        let old = snapshot_at(&db.conn, id).unwrap();
        assert!(old
            .coverage_gaps
            .contains(&"legacy_capture_semantics".to_owned()));
        assert_ne!(old.revision, current.revision);
        assert_eq!(old.events, current.events);
    }

    #[test]
    fn session_end_requires_a_new_start_before_another_prompt() {
        let root = tempfile::tempdir().unwrap();
        let mut db = journal();
        let grant = db
            .configure("codex", root.path(), true, Some("reader"), false)
            .unwrap();
        let start = json!({"session_id":"s", "hook_event_name":"SessionStart"});
        deliver(&mut db, root.path(), start.clone());
        let first = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"one", "hook_event_name":"UserPromptSubmit", "prompt":"First request"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"one", "hook_event_name":"Stop"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "hook_event_name":"SessionEnd"}),
        );
        assert!(db
            .expose(&grant, first["episode"].as_str().unwrap(), &json!({}))
            .is_err());
        assert_eq!(deliver(&mut db, root.path(), start)["status"], "duplicate");
        let after_end = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"two", "hook_event_name":"UserPromptSubmit", "prompt":"Late request"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"two", "hook_event_name":"Stop"}),
        );
        assert!(
            snapshot_at(&db.conn, after_end["episode"].as_str().unwrap())
                .unwrap()
                .coverage_gaps
                .contains(&"missing_session_start".to_owned())
        );

        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "event_id":"fresh-start", "hook_event_name":"SessionStart"}),
        );
        let resumed = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"three", "hook_event_name":"UserPromptSubmit", "prompt":"New request after resume"}),
        );
        assert!(db
            .expose(&grant, resumed["episode"].as_str().unwrap(), &json!({}))
            .is_ok());
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"three", "hook_event_name":"Stop"}),
        );
        assert!(snapshot_at(&db.conn, resumed["episode"].as_str().unwrap())
            .unwrap()
            .coverage_gaps
            .is_empty());
    }

    #[test]
    fn a_late_identified_stop_closes_its_own_turn_without_replacing_newer_context() {
        let root = tempfile::tempdir().unwrap();
        let mut db = journal();
        db.configure("codex", root.path(), true, None, false)
            .unwrap();
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "hook_event_name":"SessionStart"}),
        );
        let first = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"one", "hook_event_name":"UserPromptSubmit", "prompt":"First request"}),
        );
        let second = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"two", "hook_event_name":"UserPromptSubmit", "prompt":"Second request"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"two", "hook_event_name":"Stop", "last_assistant_message":"Newer response"}),
        );
        let late = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"one", "hook_event_name":"Stop", "last_assistant_message":"Older response"}),
        );
        assert_eq!(late["episode"], first["episode"]);
        assert!(
            db.episode(first["episode"].as_str().unwrap())
                .unwrap()
                .closed
        );
        assert!(snapshot_at(&db.conn, second["episode"].as_str().unwrap())
            .unwrap()
            .coverage_gaps
            .is_empty());
        let next = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"three", "hook_event_name":"UserPromptSubmit", "prompt":"Third request"}),
        );
        let episode = db.episode(next["episode"].as_str().unwrap()).unwrap();
        let context: Vec<_> = episode
            .events
            .iter()
            .filter(|e| e.origin == "prior_turn_context")
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(context, ["Newer response"]);
    }

    #[test]
    fn unknown_or_ambiguous_stop_identity_never_falls_back_to_the_active_turn() {
        for ambiguous in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let mut db = journal();
            db.configure("codex", root.path(), true, None, false)
                .unwrap();
            deliver(
                &mut db,
                root.path(),
                json!({"session_id":"s", "hook_event_name":"SessionStart"}),
            );
            let first = deliver(
                &mut db,
                root.path(),
                json!({"session_id":"s", "event_id":"first", "turn_id":"same", "hook_event_name":"UserPromptSubmit", "prompt":"First request"}),
            );
            let active = if ambiguous {
                deliver(
                    &mut db,
                    root.path(),
                    json!({"session_id":"s", "event_id":"second", "turn_id":"same", "hook_event_name":"UserPromptSubmit", "prompt":"Conflicting request"}),
                )
            } else {
                first
            };
            let stop = deliver(
                &mut db,
                root.path(),
                json!({"session_id":"s", "turn_id":if ambiguous {"same"} else {"unknown"}, "hook_event_name":"Stop"}),
            );
            assert!(stop["episode"].is_null());
            assert!(
                !db.episode(active["episode"].as_str().unwrap())
                    .unwrap()
                    .closed
            );
            let gaps = snapshot_at(&db.conn, active["episode"].as_str().unwrap())
                .unwrap()
                .coverage_gaps;
            assert!(gaps.contains(
                &if ambiguous {
                    "ambiguous_turn_identity"
                } else {
                    "unmatched_turn_event"
                }
                .to_owned()
            ));
        }
    }

    #[test]
    fn repeated_session_end_without_identity_cannot_leave_resumed_admission_open() {
        for stable in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let mut db = journal();
            let grant = db
                .configure("codex", root.path(), true, Some("reader"), false)
                .unwrap();
            deliver(
                &mut db,
                root.path(),
                json!({"session_id":"s", "hook_event_name":"SessionStart"}),
            );
            let mut end = json!({"session_id":"s", "hook_event_name":"SessionEnd"});
            if stable {
                end["event_id"] = json!("first-end");
            }
            deliver(&mut db, root.path(), end.clone());
            deliver(
                &mut db,
                root.path(),
                json!({"session_id":"s", "event_id":"fresh-start", "hook_event_name":"SessionStart"}),
            );
            let prompt = deliver(
                &mut db,
                root.path(),
                json!({"session_id":"s", "turn_id":"two", "hook_event_name":"UserPromptSubmit", "prompt":"Resumed request"}),
            );
            assert_eq!(deliver(&mut db, root.path(), end)["status"], "duplicate");
            let offered = db.expose(&grant, prompt["episode"].as_str().unwrap(), &json!({}));
            assert_eq!(offered.is_ok(), stable);
        }
    }

    #[test]
    fn an_active_stop_without_text_clears_the_previous_response() {
        let root = tempfile::tempdir().unwrap();
        let mut db = journal();
        db.configure("codex", root.path(), true, None, false)
            .unwrap();
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "hook_event_name":"SessionStart"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"one", "hook_event_name":"UserPromptSubmit", "prompt":"First request"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"one", "hook_event_name":"Stop", "last_assistant_message":"Old response"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"two", "hook_event_name":"UserPromptSubmit", "prompt":"Second request"}),
        );
        deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"two", "hook_event_name":"Stop", "last_assistant_message":null}),
        );
        let next = deliver(
            &mut db,
            root.path(),
            json!({"session_id":"s", "turn_id":"three", "hook_event_name":"UserPromptSubmit", "prompt":"Third request"}),
        );
        assert!(!db
            .episode(next["episode"].as_str().unwrap())
            .unwrap()
            .events
            .iter()
            .any(|event| event.origin == "prior_turn_context"));
    }
}
