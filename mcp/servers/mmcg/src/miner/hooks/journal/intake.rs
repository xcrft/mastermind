//! Durable, at-most-once intake attempts. A receipt binds a model result to an
//! admitted prompt, never grants execution or certifies semantic correctness.

use super::*;
use crate::miner::hooks::refiner::{self, Config, Input, Response};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::miner::hooks) struct IntakeReceipt {
    pub schema: u32,
    pub status: String,
    pub input: Input,
    pub config_revision: i64,
    pub response: Option<Response>,
    pub reason: Option<String>,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub session_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_binding_revision: Option<String>,
}

impl Journal {
    pub fn refiner_config(&self, client: &str, root: &Path) -> Result<Option<Config>, Error> {
        // Read-only status on journals created before this table is supported.
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='hook_refiner')",
            [],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        Ok(read_config(&self.conn, client, root)?.and_then(|(_, config)| config))
    }

    pub fn configure_refiner(
        &mut self,
        client: &str,
        root: &Path,
        config: Option<&Config>,
    ) -> Result<(), Error> {
        if let Some(config) = config {
            config.validate()?;
        }
        let data = config.map(serde_json::to_string).transpose()?;
        // Keep an epoch across disable/re-enable and A -> B -> A changes.
        self.conn.execute(
            "INSERT INTO hook_refiner(client,project_root,revision,data) VALUES(?1,?2,1,?3)
             ON CONFLICT(client,project_root) DO UPDATE SET
             revision=hook_refiner.revision + CASE WHEN hook_refiner.data IS NOT excluded.data THEN 1 ELSE 0 END,
             data=excluded.data",
            params![client, root.to_string_lossy(), data],
        )?;
        Ok(())
    }

    pub fn intake(&self, id: &str) -> Result<IntakeReceipt, Error> {
        let data: String =
            self.conn
                .query_row("SELECT data FROM hook_intake WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
        Ok(serde_json::from_str(&data)?)
    }

    pub fn intake_for_episode(&self, episode: &str) -> Result<Option<Value>, Error> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='hook_intake')",
            [],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        let data: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM hook_intake WHERE episode=?1",
                [episode],
                |r| r.get(0),
            )
            .optional()?;
        data.map(|data| {
            let receipt: IntakeReceipt = serde_json::from_str(&data)?;
            Ok(json!({"id":receipt.input.id,"status":receipt.status,"reason":receipt.reason,"elapsed_ms":receipt.elapsed_ms}))
        }).transpose()
    }

    pub fn profile_task(&self, episode: &str) -> Result<Option<(String, String, String)>, Error> {
        let Some(summary) = self.intake_for_episode(episode)? else {
            return Ok(None);
        };
        let receipt = self.intake(summary["id"].as_str().ok_or("intake id unavailable")?)?;
        if receipt.status != "offered"
            || !receipt.response.as_ref().is_some_and(|response| {
                response.workflow_intent == refiner::Intent::ContinueActive
                    && response.action != refiner::Action::Ask
            })
        {
            return Ok(None);
        }
        let (epoch, binding) = super::task::active(&self.conn, &receipt.input.session_id)?;
        Ok(binding
            .filter(|binding| {
                epoch == receipt.session_epoch
                    && Some(&binding.revision) == receipt.task_binding_revision.as_ref()
                    && Some(&binding.spec_path) == receipt.input.active_task.as_ref()
            })
            .map(|binding| (binding.spec_path, binding.revision, binding.spec_sha256)))
    }

    pub fn begin_intake(
        &mut self,
        grant: &Grant,
        episode: &str,
        event_id: &str,
        original: &str,
    ) -> Result<Option<(Config, IntakeReceipt)>, Error> {
        // A failed delivery may leave only its filesystem marker, before it
        // reached SQLite. Keep bounded filesystem I/O outside the writer lock.
        let capture_clear =
            !crate::miner::hooks::fence::pending(&grant.client, Path::new(&grant.project_root))
                .unwrap_or(true);
        let ep = load_episode(&self.conn, episode)?;
        let (session_epoch, active_task) = super::task::active(&self.conn, &ep.session)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some((revision, Some(config))) =
            read_config(&tx, &grant.client, Path::new(&grant.project_root))?
        else {
            return Ok(None);
        };
        let id = hash(&json!(["hook-intake-v1", event_id]));
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM hook_intake WHERE id=?1 OR episode=?2)",
            params![id, episode],
            |r| r.get(0),
        )?;
        if exists {
            return Ok(None);
        }
        let ep = load_episode(&tx, episode)?;
        let prompt = ep
            .events
            .iter()
            .find(|e| e.id == event_id && e.kind == "UserPromptSubmit")
            .ok_or("intake prompt is absent from its capture episode")?;
        // Retain exactly the captured text. Redaction/truncation must never be
        // undone by copying raw stdin into a second store or model request.
        let input = Input {
            model: ep.model.clone(),
            id: id.clone(),
            event_id: event_id.into(),
            episode_id: episode.into(),
            session_id: ep.session.clone(),
            client: ep.client.clone(),
            project_root: ep.project_root.clone(),
            capture_generation: ep.generation,
            prompt_digest: refiner::prompt_digest(&prompt.text),
            original: prompt.text.clone(),
            active_task: active_task.as_ref().map(|task| task.spec_path.clone()),
        };
        let admissible = capture_clear
            && prompt.text == original
            && !original.trim().is_empty()
            && prompt.origin == "user_channel_unverified"
            && current_prompt(&tx, grant, &input)?
            && super::task::epoch(&tx, &input.session_id)? == session_epoch;
        let receipt = IntakeReceipt {
            schema: 1,
            status: if admissible { "pending" } else { "degraded" }.into(),
            input,
            config_revision: revision,
            response: None,
            reason: (!admissible).then(|| "capture_not_admitted".into()),
            elapsed_ms: 0,
            session_epoch,
            task_binding_revision: active_task.map(|task| task.revision),
        };
        tx.execute(
            "INSERT INTO hook_intake(id,episode,data) VALUES(?1,?2,?3)",
            params![id, episode, serde_json::to_string(&receipt)?],
        )?;
        tx.commit()?;
        Ok(Some((config, receipt)))
    }

    pub fn finish_intake(
        &mut self,
        grant: &Grant,
        receipt: &IntakeReceipt,
        result: Result<Response, Error>,
        elapsed_ms: u64,
    ) -> Result<IntakeReceipt, Error> {
        let capture_clear =
            !crate::miner::hooks::fence::pending(&grant.client, Path::new(&grant.project_root))
                .unwrap_or(true);
        let (session_epoch, active_task) =
            super::task::active(&self.conn, &receipt.input.session_id)?;
        let task_current = session_epoch == receipt.session_epoch
            && active_task.as_ref().map(|task| &task.revision)
                == receipt.task_binding_revision.as_ref();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored: String = tx.query_row(
            "SELECT data FROM hook_intake WHERE id=?1",
            [&receipt.input.id],
            |r| r.get(0),
        )?;
        let mut current: IntakeReceipt = serde_json::from_str(&stored)?;
        if current.status != "pending"
            || current.input != receipt.input
            || current.config_revision != receipt.config_revision
        {
            return Err("intake attempt changed before publication".into());
        }
        let config_current = read_config(&tx, &grant.client, Path::new(&grant.project_root))?
            .is_some_and(|(revision, config)| {
                revision == current.config_revision && config.is_some()
            });
        if !capture_clear
            || !config_current
            || !task_current
            || super::task::epoch(&tx, &current.input.session_id)? != session_epoch
            || !current_prompt(&tx, grant, &current.input)?
        {
            current.status = "withheld".into();
            current.reason = Some("admission_changed".into());
        } else {
            match result {
                Ok(response) => {
                    // Validate again at publication, even when the caller uses
                    // another adapter in future. JSON binding is not meaning.
                    refiner::parse_response(&current.input, &serde_json::to_vec(&response)?)?;
                    current.status = "offered".into();
                    current.response = Some(response);
                    mark_refiner_exposure(&tx, &current)?;
                }
                Err(_) => {
                    current.status = "degraded".into();
                    // Never copy provider stderr or potentially private output.
                    current.reason = Some("processor_failed_or_invalid_response".into());
                }
            }
        }
        current.elapsed_ms = elapsed_ms;
        tx.execute(
            "UPDATE hook_intake SET data=?2 WHERE id=?1",
            params![current.input.id, serde_json::to_string(&current)?],
        )?;
        tx.commit()?;
        Ok(current)
    }
}

fn read_config(
    conn: &Connection,
    client: &str,
    root: &Path,
) -> Result<Option<(i64, Option<Config>)>, Error> {
    let row: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT revision,data FROM hook_refiner WHERE client=?1 AND project_root=?2",
            params![client, root.to_string_lossy()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(revision, data)| {
        Ok((
            revision,
            data.map(|s| serde_json::from_str(&s)).transpose()?,
        ))
    })
    .transpose()
}

fn current_prompt(conn: &Connection, grant: &Grant, input: &Input) -> Result<bool, Error> {
    admitted_prompt(conn, grant, input, false)
}

pub(super) fn admitted_prompt(
    conn: &Connection,
    grant: &Grant,
    input: &Input,
    allow_open_tools: bool,
) -> Result<bool, Error> {
    let Some(current) = read_grant(conn, &grant.client, Path::new(&grant.project_root))? else {
        return Ok(false);
    };
    if !current.enabled
        || current.generation != grant.generation
        || current.generation != input.capture_generation
        || current.pending != 0
        || !current.gap.is_empty()
        || input.client != grant.client
        || input.project_root != grant.project_root
    {
        return Ok(false);
    }
    let ep = load_episode(conn, &input.episode_id)?;
    let session: Session = serde_json::from_str(&conn.query_row(
        "SELECT data FROM hook_session WHERE id=?1",
        [&ep.session],
        |r| r.get::<_, String>(0),
    )?)?;
    Ok(session.capture_version == CAPTURE_VERSION
        && session.started
        && session.gaps.is_empty()
        && session.active.as_deref() == Some(input.episode_id.as_str())
        && ep.session == input.session_id
        && ep.model == input.model
        && ep.gaps.is_empty()
        && !ep.closed
        && (allow_open_tools || ep.open_tools.is_empty())
        && ep.events.iter().any(|e| {
            e.id == input.event_id
                && e.kind == "UserPromptSubmit"
                && e.origin == "user_channel_unverified"
                && e.text == input.original
                && refiner::prompt_digest(&e.text) == input.prompt_digest
        }))
}

fn mark_refiner_exposure(conn: &Connection, receipt: &IntakeReceipt) -> Result<(), Error> {
    let mut ep = load_episode(conn, &receipt.input.episode_id)?;
    let mut session: Session = serde_json::from_str(&conn.query_row(
        "SELECT data FROM hook_session WHERE id=?1",
        [&ep.session],
        |r| r.get::<_, String>(0),
    )?)?;
    // Preserve the already captured prompt's influence snapshot. Future input
    // has prior advisory exposure even if native use remains unverified.
    session.influence.offer_refiner();
    let exposure = json!({"status":"refiner_context_offered"});
    push_exposure(&mut session, exposure);
    ep.exposures
        .push(json!({"status":"refiner_context_offered","intake_id":receipt.input.id}));
    save_session(conn, &session)?;
    save_episode(conn, &ep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use refiner::{Action, Intent};

    fn event(db: &mut Journal, root: &Path, value: Value) -> Value {
        let grant = db.begin_capture("codex", root).unwrap().unwrap();
        db.receive(
            &grant,
            crate::miner::hooks::normalize(&value).unwrap(),
            "project",
            "repository",
        )
        .unwrap()
    }

    fn prepared() -> (tempfile::TempDir, Journal, Grant, Config, IntakeReceipt) {
        let root = tempfile::tempdir().unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let mut db = Journal { conn };
        let grant = db
            .configure("codex", root.path(), true, None, false)
            .unwrap();
        let config = Config {
            processor: Some(std::env::current_exe().unwrap()),
            provider: None,
            args: vec![],
            timeout_secs: 1,
        };
        db.configure_refiner("codex", root.path(), Some(&config))
            .unwrap();
        event(
            &mut db,
            root.path(),
            json!({"session_id":"s","hook_event_name":"SessionStart"}),
        );
        let captured = event(
            &mut db,
            root.path(),
            json!({"session_id":"s","turn_id":"one","hook_event_name":"UserPromptSubmit","prompt":"Explain this error."}),
        );
        let (_, receipt) = db
            .begin_intake(
                &grant,
                captured["episode"].as_str().unwrap(),
                captured["event_id"].as_str().unwrap(),
                "Explain this error.",
            )
            .unwrap()
            .unwrap();
        assert_eq!(receipt.status, "pending");
        (root, db, grant, config, receipt)
    }

    fn response(receipt: &IntakeReceipt) -> Response {
        Response {
            schema: 1,
            intake_id: receipt.input.id.clone(),
            prompt_digest: receipt.input.prompt_digest.clone(),
            action: Action::Passthrough,
            workflow_intent: Intent::Ordinary,
            intent_evidence: None,
            refined_prompt: Some(receipt.input.original.clone()),
            questions: vec![],
        }
    }

    #[test]
    fn one_durable_attempt_preserves_original_and_marks_offered_context() {
        let (_root, mut db, grant, _, receipt) = prepared();
        let original = receipt.input.original.clone();
        let result = db
            .finish_intake(&grant, &receipt, Ok(response(&receipt)), 7)
            .unwrap();
        assert_eq!(result.status, "offered");
        assert_eq!(
            db.intake(&receipt.input.id).unwrap().input.original,
            original
        );
        assert!(
            snapshot_at(&db.conn, &receipt.input.episode_id)
                .unwrap()
                .profile_influenced
        );
        assert!(db
            .begin_intake(
                &grant,
                &receipt.input.episode_id,
                &receipt.input.event_id,
                &original
            )
            .unwrap()
            .is_none());
        assert!(db
            .finish_intake(&grant, &receipt, Ok(response(&receipt)), 9)
            .is_err());
    }

    #[test]
    fn repeated_refinement_has_constant_session_exposure_storage() {
        let (root, mut db, grant, _, mut receipt) = prepared();
        for index in 0..40 {
            db.finish_intake(&grant, &receipt, Ok(response(&receipt)), 1)
                .unwrap();
            let current = db.episode(&receipt.input.episode_id).unwrap();
            assert!(current
                .exposures
                .iter()
                .any(|e| e["intake_id"] == receipt.input.id));
            assert!(
                snapshot_at(&db.conn, &current.id)
                    .unwrap()
                    .profile_influenced
            );
            if index < 39 {
                event(
                    &mut db,
                    root.path(),
                    json!({"session_id":"s","turn_id":current.turn_id,"hook_event_name":"Stop"}),
                );
                let prompt = event(
                    &mut db,
                    root.path(),
                    json!({"session_id":"s","turn_id":format!("next-{index}"),"hook_event_name":"UserPromptSubmit","prompt":"Explain this error."}),
                );
                receipt = db
                    .begin_intake(
                        &grant,
                        prompt["episode"].as_str().unwrap(),
                        prompt["event_id"].as_str().unwrap(),
                        "Explain this error.",
                    )
                    .unwrap()
                    .unwrap()
                    .1;
            }
        }
        assert_eq!(
            db.session(&receipt.input.session_id)
                .unwrap()
                .unwrap()
                .exposures
                .len(),
            1
        );
    }

    #[test]
    fn revoked_or_reconfigured_attempt_cannot_publish_even_after_an_aba_change() {
        for revoke in [false, true] {
            let (root, mut db, grant, config, receipt) = prepared();
            if revoke {
                db.configure("codex", root.path(), false, None, false)
                    .unwrap();
                db.configure("codex", root.path(), true, None, false)
                    .unwrap();
            } else {
                db.configure_refiner("codex", root.path(), None).unwrap();
                db.configure_refiner("codex", root.path(), Some(&config))
                    .unwrap();
            }
            let result = db
                .finish_intake(&grant, &receipt, Ok(response(&receipt)), 7)
                .unwrap();
            assert_eq!(result.status, "withheld");
            assert!(result.response.is_none());
        }
    }

    #[test]
    fn newer_prompt_or_stop_prevents_an_old_result_from_being_offered() {
        for kind in ["UserPromptSubmit", "Stop", "SessionEnd"] {
            let (root, mut db, grant, _, receipt) = prepared();
            event(
                &mut db,
                root.path(),
                json!({"session_id":"s","turn_id":if kind=="UserPromptSubmit" {"two"} else {"one"},"hook_event_name":kind,"prompt":"A new request"}),
            );
            let result = db
                .finish_intake(&grant, &receipt, Ok(response(&receipt)), 7)
                .unwrap();
            assert_eq!(result.status, "withheld", "{kind}");
            assert!(result.response.is_none());
        }
    }

    #[test]
    fn failures_and_unfinished_attempts_are_not_implicitly_retried() {
        let (_root, mut db, grant, _, receipt) = prepared();
        assert!(db
            .begin_intake(
                &grant,
                &receipt.input.episode_id,
                &receipt.input.event_id,
                &receipt.input.original
            )
            .unwrap()
            .is_none());
        let result = db
            .finish_intake(&grant, &receipt, Err("private provider error".into()), 1000)
            .unwrap();
        assert_eq!(result.status, "degraded");
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("private provider error"));
        assert!(db
            .begin_intake(
                &grant,
                &receipt.input.episode_id,
                &receipt.input.event_id,
                &receipt.input.original
            )
            .unwrap()
            .is_none());
    }

    #[test]
    fn forgetting_an_episode_also_forgets_its_raw_intake_copy() {
        let (_root, mut db, _, _, receipt) = prepared();
        let revision = snapshot_at(&db.conn, &receipt.input.episode_id)
            .unwrap()
            .revision;
        db.forget(&receipt.input.episode_id, &revision).unwrap();
        assert!(db.intake(&receipt.input.id).is_err());
        assert!(db
            .intake_for_episode(&receipt.input.episode_id)
            .unwrap()
            .is_none());
    }
}
