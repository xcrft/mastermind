//! Durable capture is separate from published persona state. A committed
//! pending counter fences readers before stdin is consumed; an interrupted
//! capture cannot leave its older evidence apparently current.

use super::semantic::{EpisodeInput, EventInput, SemanticDraft};
use super::{hash, Error, EXTRACTOR};
use crate::bounded_fs::{self, BoundedReadError, ReadControl};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EPISODE_BYTES: usize = 512 * 1024;
const MAX_EVENTS: usize = 128;
const MAX_EPISODES: i64 = 2000;

pub(super) fn path() -> Result<PathBuf, Error> {
    Ok(std::env::home_dir()
        .ok_or("could not resolve home")?
        .join(".mastermind/persona-events.db"))
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
    episode_count: usize,
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
    pub profile_exposure: Option<Value>,
}

impl Journal {
    pub fn open(write: bool) -> Result<Self, Error> {
        let target = path()?;
        let (root, target) = if write {
            bounded_fs::prepare_file_target(&target)?
        } else {
            bounded_fs::open_file_target(&target)?
        };
        let identity = match bounded_fs::read_regular_file_with_capability(
            &root,
            &target,
            MAX_BYTES,
            0,
            ReadControl::default(),
        ) {
            Ok(file) => file.identity,
            Err(BoundedReadError::Io(error))
                if write && error.kind() == std::io::ErrorKind::NotFound =>
            {
                match bounded_fs::create_regular_file_with_capability(&root, &target, true) {
                    Ok((file, identity)) => {
                        file.sync_all()?;
                        identity
                    }
                    Err(BoundedReadError::Io(error))
                        if error.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        bounded_fs::read_regular_file_with_capability(
                            &root,
                            &target,
                            MAX_BYTES,
                            0,
                            ReadControl::default(),
                        )?
                        .identity
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        };
        // SQLite owns the rollback journal. Reject planted special files before
        // SQLite can open them. WAL is deliberately not enabled for this store.
        for suffix in ["-journal", "-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", target.display()));
            match bounded_fs::read_regular_file_with_capability(
                &root,
                &sidecar,
                MAX_BYTES,
                0,
                ReadControl::default(),
            ) {
                Ok(_) => {}
                Err(BoundedReadError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
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
        if !bounded_fs::read_regular_file_with_capability(
            &root,
            &target,
            MAX_BYTES,
            0,
            ReadControl::default(),
        )?
        .identity
        .same_object(identity)
        {
            return Err("hook journal changed while opening".into());
        }
        conn.busy_timeout(Duration::from_millis(500))?;
        if write {
            conn.execute_batch("PRAGMA synchronous=FULL; PRAGMA journal_mode=DELETE;
                CREATE TABLE IF NOT EXISTS hook_grant (
                    client TEXT NOT NULL, project_root TEXT NOT NULL, generation INTEGER NOT NULL,
                    enabled INTEGER NOT NULL, pending INTEGER NOT NULL DEFAULT 0,
                    gap TEXT NOT NULL DEFAULT '', profile_client TEXT,
                    PRIMARY KEY(client,project_root));
                CREATE TABLE IF NOT EXISTS hook_session (id TEXT PRIMARY KEY, data TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS hook_episode (id TEXT PRIMARY KEY, session TEXT NOT NULL, data TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS hook_episode_session ON hook_episode(session,id);
                CREATE TABLE IF NOT EXISTS hook_event (
                    id TEXT PRIMARY KEY, session TEXT NOT NULL, native_key TEXT NOT NULL,
                    digest TEXT NOT NULL, episode TEXT, tool_id TEXT, kind TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS hook_event_tool ON hook_event(session,tool_id,kind);
                CREATE TABLE IF NOT EXISTS hook_draft (id TEXT PRIMARY KEY, episode TEXT NOT NULL, data TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS hook_draft_episode ON hook_draft(episode,id);
                CREATE TABLE IF NOT EXISTS hook_analysis (
                    episode TEXT NOT NULL, revision TEXT NOT NULL, processor TEXT NOT NULL,
                    completed INTEGER NOT NULL DEFAULT 0, lease_until INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY(episode,revision,processor));")?;
            let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
            conn.pragma_update(None, "max_page_count", MAX_BYTES as i64 / page_size)?;
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

    pub fn episode(&self, id: &str) -> Result<Episode, Error> {
        let data: String =
            self.conn
                .query_row("SELECT data FROM hook_episode WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
        Ok(serde_json::from_str(&data)?)
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
            episode_count: 0,
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
        }
        let active_generation = tx.prepare("SELECT 1 FROM hook_grant WHERE client=?1 AND project_root=?2 AND generation=?3 AND enabled=1")?
            .exists(params![grant.client,grant.project_root,grant.generation])?;
        if !active_generation {
            return Err("capture was revoked during delivery".into());
        }
        let existing: Option<String> = tx
            .query_row(
                "SELECT digest FROM hook_event WHERE id=?1",
                [&event_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(digest) = existing {
            if digest != incoming.digest {
                add_gap(&mut session.gaps, "event_identity_conflict");
            } else if incoming.native_key.is_none() && incoming.kind == "UserPromptSubmit" {
                add_gap(&mut session.gaps, "ambiguous_prompt_replay");
            }
            save_session(&tx, &session)?;
            finish(&tx, grant)?;
            tx.commit()?;
            return Ok(json!({"status":"duplicate","event_id":event_id,"episode":session.active}));
        }
        if incoming.forked {
            add_gap(&mut session.gaps, "fork_or_delegated_session");
        }
        if session.project != project || session.repository != repository {
            add_gap(&mut session.gaps, "project_identity_changed");
        }
        if let Some(gap) = &incoming.gap {
            add_gap(&mut session.gaps, gap);
        }
        if let Some(exposure) = &incoming.profile_exposure {
            if !session.exposures.contains(exposure) {
                if session.exposures.len() >= 32 {
                    add_gap(&mut session.gaps, "profile_exposure_limit");
                } else {
                    session.exposures.push(exposure.clone());
                }
            }
        }
        let event = EventInput {
            id: event_id.clone(),
            kind: incoming.kind.clone(),
            actor: incoming.actor,
            origin: incoming.origin,
            text: incoming.text,
        };
        if incoming.kind == "SessionStart" {
            session.started = true;
        }
        let mut target = session.active.clone();
        if incoming.kind == "UserPromptSubmit" {
            if let Some(previous) = &session.active {
                let mut ep = load_episode(&tx, previous)?;
                if !ep.closed {
                    add_gap(&mut ep.gaps, "next_prompt_before_stop");
                }
                let mut correction = event.clone();
                correction.origin = "next_turn_context".into();
                push_event(&mut ep, correction);
                save_episode(&tx, &ep)?;
            }
            let count: i64 = tx.query_row("SELECT count(*) FROM hook_episode", [], |r| r.get(0))?;
            if count >= MAX_EPISODES {
                return Err(
                    "hook journal episode limit reached; export or forget reviewed sessions".into(),
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
                if !event.text.is_empty() {
                    session.previous_assistant = Some(event.clone());
                }
            } else if matches!(
                incoming.kind.as_str(),
                "Interrupt" | "PreCompact" | "StopFailure"
            ) {
                add_gap(&mut ep.gaps, "interrupted_or_compacted_turn");
            }
            push_event(&mut ep, event);
            save_episode(&tx, &ep)?;
        } else if !matches!(incoming.kind.as_str(), "SessionStart" | "SessionEnd") {
            add_gap(&mut session.gaps, "event_without_user_turn");
        }
        tx.execute("INSERT INTO hook_event(id,session,native_key,digest,episode,tool_id,kind) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![event_id,sid,key,incoming.digest,target,incoming.tool_id,incoming.kind])?;
        save_session(&tx, &session)?;
        finish(&tx, grant)?;
        tx.commit()?;
        Ok(json!({"status":"recorded","event_id":event_id,"episode":target}))
    }

    pub fn expose(&mut self, episode: &str, packet: &Value) -> Result<(), Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut ep = load_episode(&tx, episode)?;
        let mut session: Session = serde_json::from_str(&tx.query_row(
            "SELECT data FROM hook_session WHERE id=?1",
            [&ep.session],
            |r| r.get::<_, String>(0),
        )?)?;
        let claims = |field: &str, key: &str| {
            packet[field].as_array().map(|items| items.iter().take(32)
            .map(|item|json!({"id":item[key],"review_revision":item["review_revision"]})).collect::<Vec<_>>()).unwrap_or_default()
        };
        let receipt = json!({"status":"offered","packet_digest":hash(packet),"profile_revision":packet.get("profile_revision"),
            "feedback":claims("feedback","key"),"habits":claims("habits","id")});
        if !session.exposures.contains(&receipt) {
            if session.exposures.len() >= 32 {
                add_gap(&mut session.gaps, "profile_exposure_limit");
            } else {
                session.exposures.push(receipt.clone());
            }
        }
        ep.exposures.push(receipt);
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
            let ep: Episode = serde_json::from_str(&data)?;
            let snapshot = self.snapshot(&id)?;
            result.push(json!({"id":id,"revision":snapshot.revision,"client":ep.client,"closed":ep.closed,
                "events":ep.events.len(),"coverage_gaps":snapshot.coverage_gaps,"profile_influenced":snapshot.profile_influenced}));
        }
        Ok(result)
    }

    pub fn data_version(&self) -> Result<i64, Error> {
        Ok(self
            .conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))?)
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
        if current.revision != revision
            || !current.coverage_gaps.is_empty()
            || current.profile_influenced
        {
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
        let mut stmt = self
            .conn
            .prepare("SELECT data FROM hook_draft WHERE episode=?1 ORDER BY id LIMIT 101")?;
        let rows = stmt.query_map([episode], |r| r.get::<_, String>(0))?;
        rows.map(|row| { let draft:Draft=serde_json::from_str(&row?)?;
            Ok(json!({"id":draft.id,"revision":draft.revision,"episode_revision":draft.episode_revision,"attested":draft.attested})) }).collect()
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
        super::semantic::validate(&verified, std::slice::from_ref(&prepared.content))?;
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
        super::semantic::validate(&input, std::slice::from_ref(&draft.content))?;
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
        let mut stmt = tx.prepare("SELECT data FROM hook_episode WHERE session=?1 AND id!=?2")?;
        let retained = stmt
            .query_map(params![forgotten.session, id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for data in retained {
            let mut ep: Episode = serde_json::from_str(&data)?;
            ep.events.retain(|e| !event_ids.contains(e.id.as_str()));
            save_episode(&tx, &ep)?;
        }
        // Drafts can quote cross-turn context. Drop this session's derived
        // hypotheses along with every copy of the removed raw event text.
        tx.execute("DELETE FROM hook_draft WHERE episode IN (SELECT id FROM hook_episode WHERE session=?1)",[&forgotten.session])?;
        tx.execute("DELETE FROM hook_episode WHERE id=?1", [id])?;
        tx.execute("DELETE FROM hook_event WHERE episode=?1", [id])?;
        tx.execute("DELETE FROM hook_analysis WHERE episode=?1", [id])?;
        tx.commit()?;
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
    Ok(serde_json::from_str(&data)?)
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
        ep,
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
