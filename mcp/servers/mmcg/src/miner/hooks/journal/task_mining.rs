use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Offer {
    id: String,
    episode: String,
    client: String,
    project_root: String,
    generation: i64,
    event_id: String,
    event_digest: String,
    status: String,
    drafts: Option<Vec<SemanticDraft>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    when: String,
    behavior: String,
    exception: String,
    evidence_kind: String,
    quote: String,
}

fn task_mode(root: &Path, client: &str) -> Result<bool, Error> {
    Ok(crate::onboarding::load(root)?.is_some_and(|settings| {
        settings.mining == crate::onboarding::Mining::Task
            && settings.clients.iter().any(|selected| selected == client)
    }))
}

fn admission(conn: &Connection, offer: &Offer, active: bool) -> Result<EpisodeInput, Error> {
    if super::super::fence::pending(&offer.client, Path::new(&offer.project_root)).unwrap_or(true) {
        return Err("task mining capture delivery is pending".into());
    }
    if !task_mode(Path::new(&offer.project_root), &offer.client)? {
        return Err("task mining is not enabled for this client and project".into());
    }
    let ep = load_episode(conn, &offer.episode)?;
    if ep.client != offer.client
        || ep.project_root != offer.project_root
        || ep.generation != offer.generation
    {
        return Err("task mining source identity changed".into());
    }
    if active {
        let selected: Option<String> = conn.query_row(
            "SELECT json_extract(data,'$.active') FROM hook_session WHERE id=?1",
            [&ep.session],
            |row| row.get(0),
        )?;
        if ep.closed || selected.as_deref() != Some(offer.episode.as_str()) {
            return Err("task mining offer is no longer active".into());
        }
    }
    let mut input = snapshot_at(conn, &offer.episode)?;
    if input
        .events
        .iter()
        .any(|event| event.origin == "next_turn_context")
    {
        return Err("task mining source gained later user context".into());
    }
    if crate::miner::profile::persona_project_id(Path::new(&offer.project_root)).as_deref()
        != Some(input.project.as_str())
    {
        return Err("task mining project identity changed".into());
    }
    let event = input
        .events
        .iter()
        .find(|event| event.id == offer.event_id)
        .ok_or("task mining source event unavailable")?;
    if event.kind != "UserPromptSubmit"
        || event.origin != "user_channel_unverified"
        || hash(&json!(event)) != offer.event_digest
    {
        return Err("task mining source event changed".into());
    }
    // Submission occurs during the tool call; its native PostToolUse and Stop
    // have not arrived yet. These two expected gaps are not waived at sealing.
    if active {
        input
            .coverage_gaps
            .retain(|gap| gap != "no_stop_observed" && gap != "missing_tool_result");
    }
    if !input.coverage_gaps.is_empty() {
        return Err("task mining capture is incomplete".into());
    }
    Ok(input)
}

impl Journal {
    pub fn task_mining_receipt(&self, episode: &str) -> Result<Value, Error> {
        if !self
            .conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='hook_task_mining'")?
            .exists([])?
        {
            return Ok(json!({"status":"not_offered"}));
        }
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM hook_task_mining WHERE episode=?1",
                [episode],
                |row| row.get(0),
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(json!({"status":"not_offered"}));
        };
        let offer: Offer = serde_json::from_str(&raw)?;
        Ok(
            json!({"status":offer.status,"ticket_id":offer.id,"source_event_id":offer.event_id,
            "client":offer.client,"capture_generation":offer.generation,"publication":"none"}),
        )
    }

    pub fn offer_task_mining(
        &mut self,
        grant: &Grant,
        episode: &str,
    ) -> Result<Option<Value>, Error> {
        if !task_mode(Path::new(&grant.project_root), &grant.client)? {
            return Ok(None);
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ep = load_episode(&tx, episode)?;
        let Some(event) = ep.events.iter().find(|event| {
            event.kind == "UserPromptSubmit" && event.origin == "user_channel_unverified"
        }) else {
            return Ok(None);
        };
        let offer = Offer {
            id: hash(&json!([
                "persona-task-mining-v1",
                episode,
                event.id,
                grant.generation
            ])),
            episode: episode.into(),
            client: grant.client.clone(),
            project_root: grant.project_root.clone(),
            generation: grant.generation,
            event_id: event.id.clone(),
            event_digest: hash(&json!(event)),
            status: "offered".into(),
            drafts: None,
        };
        admission(&tx, &offer, true)?;
        tx.execute(
            "INSERT INTO hook_task_mining(id,episode,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO NOTHING",
            params![offer.id, episode, serde_json::to_string(&offer)?],
        )?;
        tx.commit()?;
        Ok(Some(
            json!({"ticket_id":offer.id,"source_event_id":offer.event_id}),
        ))
    }

    pub fn submit_task_mining(
        &mut self,
        root: &Path,
        client: &str,
        ticket: &str,
        candidates: &Value,
    ) -> Result<Value, Error> {
        super::super::check_id(ticket)?;
        if serde_json::to_vec(candidates)?.len() > 8 * 1024 {
            return Err("task mining submission exceeds 8 KiB".into());
        }
        let candidates: Vec<Candidate> = serde_json::from_value(candidates.clone())?;
        if candidates.len() > 2 {
            return Err("task mining accepts at most two candidates".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let raw: String = tx.query_row(
            "SELECT data FROM hook_task_mining WHERE id=?1",
            [ticket],
            |row| row.get(0),
        )?;
        let mut offer: Offer = serde_json::from_str(&raw)?;
        if offer.client != client || Path::new(&offer.project_root) != root {
            return Err("task mining ticket belongs to another client or project".into());
        }
        let input = admission(&tx, &offer, true)?;
        let drafts: Vec<_> = candidates
            .into_iter()
            .map(|candidate| SemanticDraft {
                when: candidate.when,
                behavior: candidate.behavior,
                exception: candidate.exception,
                evidence_kind: candidate.evidence_kind,
                rationale: None,
                outcome: None,
                role: None,
                workflow: None,
                supports: vec![super::super::semantic::Citation {
                    event_id: offer.event_id.clone(),
                    quote: candidate.quote,
                }],
                contradictions: vec![],
            })
            .collect();
        super::super::semantic::validate(&input, &drafts)?;
        if let Some(previous) = &offer.drafts {
            if previous != &drafts {
                return Err("task mining ticket already has a different submission".into());
            }
        } else {
            offer.drafts = Some(drafts);
            offer.status = "submitted".into();
            tx.execute(
                "UPDATE hook_task_mining SET data=?2 WHERE id=?1",
                params![ticket, serde_json::to_string(&offer)?],
            )?;
        }
        tx.commit()?;
        Ok(
            json!({"status":"submitted","ticket_id":ticket,"publication":"none","finalization":"after_complete_stop","separate_model_invocations":0}),
        )
    }

    pub fn finish_task_mining(&mut self, episode: &str) -> Result<Value, Error> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM hook_task_mining WHERE episode=?1",
                [episode],
                |row| row.get(0),
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(json!({"status":"not_offered"}));
        };
        let mut offer: Offer = serde_json::from_str(&raw)?;
        let Some(drafts) = &offer.drafts else {
            return Ok(json!({"status":"not_submitted"}));
        };
        if offer.status == "completed" {
            return Ok(json!({"status":"completed"}));
        }
        let input = admission(&self.conn, &offer, false)?;
        if input.model.is_none()
            && input
                .model_binding
                .as_ref()
                .is_some_and(|binding| binding["source"] == "native_session_transcript_pending")
        {
            return Ok(json!({"status":"pending_model_binding"}));
        }
        super::super::semantic::validate(&input, drafts)?;
        let processor = json!({"engine":"current_task_agent","task_adapter":"persona-task-mining-v1",
            "source_client":input.client,"model":input.model,"model_binding":input.model_binding,
            "execution":"in_session","proposer_identity":"unverified","separate_model_invocations":0});
        let stored = self.store_drafts(&input, drafts, processor)?;
        offer.status = "completed".into();
        offer.drafts = Some(vec![]);
        self.conn.execute(
            "UPDATE hook_task_mining SET data=?2 WHERE id=?1",
            params![offer.id, serde_json::to_string(&offer)?],
        )?;
        Ok(
            json!({"status":"completed","drafts":stored.len(),"publication":"none","separate_model_invocations":0}),
        )
    }
}
