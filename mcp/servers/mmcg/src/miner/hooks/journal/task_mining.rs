use super::*;

/// Another process may replace a finalizer whose lease has passed. A claim, a
/// renewal and each waiting record extend the lease this far from their time.
const FINALIZER_LEASE_SECONDS: i64 = 10;

pub(in crate::miner::hooks) enum FinalizerState {
    /// Keeps ownership and extends the lease from the given Unix time.
    Waiting(&'static str, i64),
    Pending(&'static str),
    Completed(Option<&'static str>),
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    continuation: Option<Continuation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    candidate_count: Option<usize>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Continuation {
    event_id: String,
    prompt_event_id: Option<String>,
    reason: String,
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

fn offer_at(conn: &Connection, episode: &str) -> Result<Option<Offer>, Error> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT data FROM hook_task_mining WHERE episode=?1",
            [episode],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| Ok(serde_json::from_str(&raw)?)).transpose()
}

pub(super) fn task_mining_receipt(conn: &Connection, episode: &str) -> Result<Value, Error> {
    if !conn
        .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='hook_task_mining'")?
        .exists([])?
    {
        return Ok(json!({"status":"not_offered"}));
    }
    let Some(offer) = offer_at(conn, episode)? else {
        return Ok(archive::retired(conn, episode, "task_mining")?
            .unwrap_or_else(|| json!({"status":"not_offered"})));
    };
    Ok(
        json!({"status":offer.status,"ticket_id":offer.id,"source_event_id":offer.event_id,
        "client":offer.client,"capture_generation":offer.generation,"publication":"none",
        "continuation_requested":offer.continuation.is_some(),"reason":offer.reason,
        "candidate_count":offer.candidate_count,"finalization":task_finalization(conn, episode)?,
        "result":if offer.status == "completed" { offer.candidate_count.map(|count| if count == 0 {"no_signal"} else {"candidates"}) } else {None}}),
    )
}

fn task_finalization(conn: &Connection, episode: &str) -> Result<Option<Value>, Error> {
    let exists:bool=conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='hook_task_finalization')",
        [], |row|row.get(0))?;
    if !exists {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            "SELECT attempts,reason,completed,owner IS NOT NULL AND lease_until>unixepoch()
        FROM hook_task_finalization WHERE episode=?1",
            [episode],
            |row| {
                let completed: bool = row.get(2)?;
                let running: bool = row.get(3)?;
                Ok(json!({"schema_version":1,"attempts":row.get::<_,i64>(0)?,
                "reason":row.get::<_,Option<String>>(1)?,
                "status":if completed {"completed"} else if running {"running"} else {"pending"}}))
            },
        )
        .optional()?)
}

fn save_offer(conn: &Connection, offer: &Offer) -> Result<(), Error> {
    conn.execute(
        "UPDATE hook_task_mining SET data=?2 WHERE id=?1",
        params![offer.id, serde_json::to_string(offer)?],
    )?;
    Ok(())
}

fn stop_output(continuation: &Continuation) -> Value {
    json!({"decision":"block","reason":continuation.reason})
}

fn ready_for_stop(conn: &Connection, ep: &Episode) -> Result<bool, Error> {
    let grant = read_grant(conn, &ep.client, Path::new(&ep.project_root))?
        .ok_or("task mining capture grant unavailable")?;
    let mut input = snapshot_at(conn, &ep.id)?;
    input.coverage_gaps.retain(|gap| {
        gap != "no_stop_observed"
            && !(gap == "capture_pending_or_interrupted" && grant.pending == 1)
    });
    Ok(!ep.closed
        && input.coverage_gaps.is_empty()
        && crate::miner::profile::persona_project_id(Path::new(&ep.project_root)).as_deref()
            == Some(input.project.as_str())
        && super::super::semantic::validate(&input, &[]).is_ok())
}

// Called under the capture writer transaction. Only this hook's one pending
// delivery is expected here; any other gap still prevents a model request.
pub(super) fn check_stop(
    conn: &Connection,
    ep: &Episode,
    session: &Session,
    event_id: &str,
    stop_hook_active: bool,
    incoming_gap: bool,
) -> Result<Option<Value>, Error> {
    let Some(mut offer) = offer_at(conn, &ep.id)? else {
        return Ok(None);
    };
    if offer.status != "offered" || offer.drafts.is_some() {
        return Ok(None);
    }
    let reason = if !super::super::task_mining::enabled(Path::new(&ep.project_root), &ep.client)? {
        Some("task_mining_disabled")
    } else if !session.started
        || !session.gaps.is_empty()
        || session.active.as_deref() != Some(ep.id.as_str())
        || incoming_gap
        || !ready_for_stop(conn, ep)?
    {
        Some("capture_incomplete")
    } else if offer.continuation.is_some() {
        Some("submission_missing_after_retry")
    } else if stop_hook_active {
        Some("native_stop_already_continued")
    } else {
        None
    };
    if let Some(reason) = reason {
        offer.status = "skipped".into();
        offer.reason = Some(reason.into());
        save_offer(conn, &offer)?;
        return Ok(None);
    }
    let continuation = Continuation {
        event_id: event_id.into(),
        prompt_event_id: None,
        reason: super::super::task_mining::instruction(&offer.id),
    };
    let output = stop_output(&continuation);
    offer.continuation = Some(continuation);
    save_offer(conn, &offer)?;
    Ok(Some(output))
}

pub(super) fn replay_stop(
    conn: &Connection,
    episode: &str,
    event_id: &str,
) -> Result<Option<Value>, Error> {
    let ep = load_episode(conn, episode)?;
    if !super::super::task_mining::enabled(Path::new(&ep.project_root), &ep.client)?
        || !ready_for_stop(conn, &ep)?
    {
        return Ok(None);
    }
    Ok(offer_at(conn, episode)?
        .filter(|offer| {
            offer.status == "offered"
                && offer
                    .continuation
                    .as_ref()
                    .is_some_and(|continuation| continuation.event_id == event_id)
        })
        .and_then(|offer| offer.continuation)
        .map(|continuation| stop_output(&continuation)))
}

pub(super) fn continuation_prompt(
    conn: &Connection,
    session: &Session,
    incoming: &Incoming,
    event_id: &str,
) -> Result<bool, Error> {
    if incoming.kind != "UserPromptSubmit"
        || incoming.gap.is_some()
        || incoming.forked
        || !session.started
    {
        return Ok(false);
    }
    let Some(episode) = &session.active else {
        return Ok(false);
    };
    let Some(mut offer) = offer_at(conn, episode)? else {
        return Ok(false);
    };
    let ep = load_episode(conn, episode)?;
    if ep.closed
        || offer.continuation.as_ref().is_none_or(|continuation| {
            continuation.prompt_event_id.is_some() || incoming.text != continuation.reason
        })
    {
        return Ok(false);
    }
    if let Some(turn) = &incoming.native_turn {
        if episodes_for_turn(conn, &session.id, turn)?
            .iter()
            .any(|id| id != episode)
        {
            return Ok(false);
        }
    }
    offer
        .continuation
        .as_mut()
        .expect("checked continuation")
        .prompt_event_id = Some(event_id.into());
    save_offer(conn, &offer)?;
    Ok(true)
}

fn admission(conn: &Connection, offer: &Offer, active: bool) -> Result<EpisodeInput, Error> {
    if super::super::fence::pending(&offer.client, Path::new(&offer.project_root)).unwrap_or(true) {
        return Err("task mining capture delivery is pending".into());
    }
    if !super::super::task_mining::enabled(Path::new(&offer.project_root), &offer.client)? {
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
    pub fn task_mining_summary(&self, grant: &Grant) -> Result<Value, Error> {
        let mut query = self.conn.prepare("SELECT t.data,json_extract(e.data,'$.closed'),coalesce(json_extract(s.data,'$.started') AND json_extract(s.data,'$.active')=e.id,0) FROM hook_task_mining t JOIN hook_episode e ON e.id=t.episode LEFT JOIN hook_session s ON s.id=e.session WHERE json_type(e.data,'$.archive') IS NULL AND json_extract(t.data,'$.client')=?1 AND json_extract(t.data,'$.project_root')=?2 AND json_extract(t.data,'$.generation')=?3 LIMIT ?4")?;
        let mut rows = query.query(params![
            grant.client,
            grant.project_root,
            grant.generation,
            MAX_EPISODES + 1
        ])?;
        let mut tickets = 0;
        let mut completed = 0;
        let mut with_candidates = 0;
        let mut no_signal = 0;
        let mut legacy_completed = 0;
        let mut skipped = 0;
        let mut pending = 0;
        let mut unreported = 0;
        let mut reasons = BTreeMap::<String, u64>::new();
        while let Some(row) = rows.next()? {
            tickets += 1;
            if tickets > MAX_EPISODES {
                return Err("task mining summary exceeds the episode bound".into());
            }
            let offer: Offer = serde_json::from_str(&row.get::<_, String>(0)?)?;
            match offer.status.as_str() {
                "completed" => {
                    completed += 1;
                    match offer.candidate_count {
                        Some(0) => no_signal += 1,
                        Some(_) => with_candidates += 1,
                        None => legacy_completed += 1,
                    }
                }
                "skipped" => {
                    skipped += 1;
                    if let Some(reason) = offer.reason {
                        *reasons.entry(reason).or_default() += 1
                    }
                }
                "offered" if row.get::<_, bool>(1)? || !row.get::<_, bool>(2)? => unreported += 1,
                _ => pending += 1,
            }
        }
        Ok(
            json!({"status":if tickets == 0 {"not_observed"} else {"recorded"},
            "scope":"retained_active_episodes","capture_generation":grant.generation,"tickets":tickets,"completed":completed,
            "with_candidates":with_candidates,"no_signal":no_signal,"legacy_completed":legacy_completed,
            "skipped":skipped,"pending":pending,"unreported":unreported,"skip_reasons":reasons,
            "meaning":"Recorded task-agent reports. Counts do not establish source freshness, authorship, interpretation quality or active profile rules."}),
        )
    }

    pub fn task_mining_receipt(&self, episode: &str) -> Result<Value, Error> {
        task_mining_receipt(&self.conn, episode)
    }

    pub fn queue_task_finalizer(&self, episode: &str) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO hook_task_finalization(episode,reason)
            SELECT episode,'awaiting_finalizer' FROM hook_task_mining
            WHERE episode=?1 AND json_extract(data,'$.status')='submitted'
            ON CONFLICT(episode) DO NOTHING",
            [episode],
        )?;
        Ok(())
    }

    pub fn claim_task_finalizer(
        &self,
        episode: &str,
        owner: &str,
        now: i64,
    ) -> Result<bool, Error> {
        Ok(self.conn.execute(
            "INSERT INTO hook_task_finalization(episode,owner,lease_until,attempts)
            SELECT t.episode,?2,?4,1 FROM hook_task_mining t
            JOIN hook_episode e ON e.id=t.episode
            JOIN hook_grant g ON g.client=json_extract(e.data,'$.client')
                AND g.project_root=json_extract(e.data,'$.project_root')
                AND g.generation=json_extract(e.data,'$.generation')
            WHERE t.episode=?1 AND (json_extract(t.data,'$.status')='submitted'
                OR (json_extract(t.data,'$.status')='completed' AND EXISTS(
                    SELECT 1 FROM hook_task_finalization f WHERE f.episode=t.episode AND f.completed=0)))
                AND json_extract(e.data,'$.closed') AND g.enabled=1 AND g.pending=0 AND g.gap=''
            ON CONFLICT(episode) DO UPDATE SET owner=excluded.owner,
                lease_until=excluded.lease_until,attempts=min(hook_task_finalization.attempts+1,2147483647),reason=NULL
            WHERE hook_task_finalization.completed=0 AND hook_task_finalization.lease_until<=?3",
            params![episode,owner,now,now + FINALIZER_LEASE_SECONDS],
        )? == 1)
    }

    pub fn owns_task_finalizer(&self, episode: &str, owner: &str, now: i64) -> Result<bool, Error> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM hook_task_finalization WHERE episode=?1
                AND owner=?2 AND lease_until>?3 AND completed=0)",
            params![episode, owner, now],
            |row| row.get(0),
        )?)
    }

    pub fn renew_task_finalizer(&self, episode: &str, owner: &str, now: i64) -> Result<(), Error> {
        if self.conn.execute(
            "UPDATE hook_task_finalization SET lease_until=?3
            WHERE episode=?1 AND owner=?2 AND completed=0",
            params![episode, owner, now + FINALIZER_LEASE_SECONDS],
        )? != 1
        {
            return Err("task finalizer ownership changed".into());
        }
        Ok(())
    }

    pub fn record_task_finalizer(
        &self,
        episode: &str,
        owner: &str,
        state: FinalizerState,
    ) -> Result<(), Error> {
        let (reason, completed, lease_until) = match state {
            FinalizerState::Waiting(reason, now) => {
                (Some(reason), false, Some(now + FINALIZER_LEASE_SECONDS))
            }
            FinalizerState::Pending(reason) => (Some(reason), false, None),
            FinalizerState::Completed(reason) => (reason, true, None),
        };
        if self.conn.execute(
            "UPDATE hook_task_finalization SET reason=?3,completed=?4,
                owner=CASE WHEN ?5 IS NULL THEN NULL ELSE owner END,
                lease_until=coalesce(?5,0)
            WHERE episode=?1 AND owner=?2 AND completed=0",
            params![episode, owner, reason, completed, lease_until],
        )? != 1
        {
            return Err("task finalizer ownership changed".into());
        }
        Ok(())
    }

    pub fn pending_task_finalizers(&self, grant: &Grant) -> Result<Vec<String>, Error> {
        if !grant.enabled || grant.pending != 0 || !grant.gap.is_empty() {
            return Ok(vec![]);
        }
        let mut statement = self.conn.prepare(
            "SELECT t.episode FROM hook_task_mining t JOIN hook_episode e ON e.id=t.episode
            LEFT JOIN hook_task_finalization f ON f.episode=t.episode
            WHERE (json_extract(t.data,'$.status')='submitted'
                OR (json_extract(t.data,'$.status')='completed' AND f.completed=0))
                AND json_extract(e.data,'$.closed')
                AND json_extract(e.data,'$.client')=?1
                AND json_extract(e.data,'$.project_root')=?2
                AND json_extract(e.data,'$.generation')=?3
                AND coalesce(f.completed,0)=0 AND coalesce(f.lease_until,0)<=unixepoch()
            ORDER BY coalesce(f.attempts,0),t.episode LIMIT 4",
        )?;
        let rows = statement.query_map(
            params![grant.client, grant.project_root, grant.generation],
            |row| row.get(0),
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn offer_task_mining(
        &mut self,
        grant: &Grant,
        episode: &str,
    ) -> Result<Option<Value>, Error> {
        if !super::super::task_mining::enabled(Path::new(&grant.project_root), &grant.client)? {
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
            continuation: None,
            reason: None,
            candidate_count: None,
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
            offer.candidate_count = Some(drafts.len());
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
        let Some(mut offer) = offer_at(&self.conn, episode)? else {
            return Ok(json!({"status":"not_offered"}));
        };
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
        offer.candidate_count = Some(stored.len());
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

#[cfg(test)]
mod finalizer_tests {
    use super::*;

    fn submitted_episode() -> (tempfile::TempDir, Journal) {
        let directory = tempfile::tempdir().unwrap();
        let db = Journal::open_target(&directory.path().join("journal.db"), true).unwrap();
        let root = directory.path().to_string_lossy();
        db.conn.execute("INSERT INTO hook_grant(client,project_root,generation,enabled) VALUES('claude',?1,1,1)", [&root]).unwrap();
        db.conn
            .execute(
                "INSERT INTO hook_episode(id,session,data) VALUES('episode','session',?1)",
                [
                    json!({"client":"claude","project_root":root,"generation":1,"closed":true})
                        .to_string(),
                ],
            )
            .unwrap();
        db.conn.execute("INSERT INTO hook_task_mining(id,episode,data) VALUES('ticket','episode','{\"status\":\"submitted\"}')", []).unwrap();
        db.queue_task_finalizer("episode").unwrap();
        (directory, db)
    }

    #[test]
    fn finalizer_claims_fence_stale_owners_and_revoked_capture() {
        let (_directory, db) = submitted_episode();
        assert!(db.claim_task_finalizer("episode", "first", 100).unwrap());
        assert!(!db
            .claim_task_finalizer("episode", "duplicate", 100)
            .unwrap());
        assert!(db.claim_task_finalizer("episode", "recovery", 111).unwrap());
        assert!(db
            .record_task_finalizer("episode", "first", FinalizerState::Completed(None))
            .is_err());
        db.record_task_finalizer(
            "episode",
            "recovery",
            FinalizerState::Pending("transcript_pending"),
        )
        .unwrap();
        db.conn
            .execute("UPDATE hook_grant SET enabled=0", [])
            .unwrap();
        assert!(!db.claim_task_finalizer("episode", "revoked", 112).unwrap());
        db.conn
            .execute("UPDATE hook_grant SET enabled=1,generation=2", [])
            .unwrap();
        assert!(!db
            .claim_task_finalizer("episode", "new_generation", 112)
            .unwrap());
        db.conn
            .execute("UPDATE hook_grant SET generation=1", [])
            .unwrap();
        assert!(db.claim_task_finalizer("episode", "last", 112).unwrap());
        db.record_task_finalizer("episode", "last", FinalizerState::Completed(None))
            .unwrap();
        assert!(!db.claim_task_finalizer("episode", "replayed", 200).unwrap());
    }

    #[test]
    fn a_waiting_record_extends_the_lease_from_when_it_was_written() {
        let (_directory, db) = submitted_episode();
        assert!(db.claim_task_finalizer("episode", "slow", 100).unwrap());
        db.record_task_finalizer(
            "episode",
            "slow",
            FinalizerState::Waiting("transcript_pending", 109),
        )
        .unwrap();
        assert!(!db.claim_task_finalizer("episode", "other", 111).unwrap());
        assert!(db.claim_task_finalizer("episode", "other", 119).unwrap());
    }
}
