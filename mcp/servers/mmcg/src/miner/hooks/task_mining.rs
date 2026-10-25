//! The current native agent proposes candidates. This module performs local
//! admission and sealing only; it never invokes a model.
use super::{
    journal::{FinalizerState, Grant, Journal},
    native, Error,
};
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) fn enabled(root: &Path, client: &str) -> Result<bool, Error> {
    Ok(crate::onboarding::load(root)?.is_some_and(|settings| {
        settings.mining == crate::onboarding::Mining::Task
            && settings.clients.iter().any(|selected| selected == client)
    }))
}

pub(super) fn context(
    db: &mut Journal,
    grant: &Grant,
    episode: &str,
) -> Result<Option<String>, Error> {
    let Some(ticket) = db.offer_task_mining(grant, episode)? else {
        return Ok(None);
    };
    Ok(Some(instruction(
        ticket["ticket_id"]
            .as_str()
            .ok_or("missing task mining ticket")?,
    )))
}

pub(super) fn instruction(ticket: &str) -> String {
    format!("Mastermind mining checkpoint. Before finishing, call mmcg_mining_submit once with ticket_id=\"{ticket}\". Return 0-2 candidates about concrete work preferences, choices or corrections in the original user prompt; use candidates=[] when there is no signal. Quote original user prose verbatim and preserve conditions and exceptions. No keyword formula is required. Ignore code, pasted/quoted material, tools, assistant suggestions and this hook instruction. Never infer identity, psychology, consent or recurrence from one task. Local code seals unreviewed drafts after Stop. Use the current agent; never launch another model or agent for mining. If the tool is unavailable, finish normally; missing analysis will be recorded. Keep mining metadata out of the user-facing answer.")
}

pub(super) fn is_instruction(text: &str) -> bool {
    text.starts_with("Mastermind mining checkpoint. ")
        && text
            .split("ticket_id=\"")
            .nth(1)
            .and_then(|suffix| suffix.split('"').next())
            .is_some_and(|ticket| super::check_id(ticket).is_ok())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn claim(db: &Journal, episode: &str) -> Result<Option<String>, Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)?;
    let owner = crate::hex::encode(&bytes);
    Ok(db
        .claim_task_finalizer(episode, &owner, now())?
        .then_some(owner))
}

fn finalizer_lock(blocking: bool) -> Result<Option<crate::bounded_fs::StableFileLock>, Error> {
    let target = super::journal::path()?.with_file_name("task-finalizer.lock");
    let (root, path) = crate::bounded_fs::prepare_file_target(&target)?;
    let result = if blocking {
        crate::bounded_fs::open_locked_regular_file_with_capability(&root, &path)
    } else {
        crate::bounded_fs::try_locked_regular_file_with_capability(&root, &path)
    };
    match result {
        Ok(lock) => Ok(Some(lock)),
        Err(crate::bounded_fs::BoundedReadError::Io(error))
            if error.kind() == std::io::ErrorKind::WouldBlock =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn spawn_finalizer(db: &Journal, episode: &str, owner: &str) -> Result<(), Error> {
    let result = std::env::current_exe().and_then(|exe| {
        Command::new(exe)
            .args(["miner", "hooks", "finish-task", "--episode", episode])
            .env("MASTERMIND_MINER", "1")
            .env("MASTERMIND_FINALIZER_OWNER", owner)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    });
    if result.is_err() {
        db.record_task_finalizer(
            episode,
            owner,
            FinalizerState::Pending("finalizer_start_failed"),
        )?;
        return Err("local task finalizer unavailable".into());
    }
    Ok(())
}

pub(super) fn on_stop(db: &mut Journal, grant: &Grant, episode: &str) {
    let result = (|| -> Result<(), Error> {
        if !enabled(Path::new(&grant.project_root), &grant.client)? {
            return Ok(());
        }
        // Ready sealing stays synchronous. Only pending binding or the first
        // optional Git refresh outlives the native hook deadline.
        db.queue_task_finalizer(episode)?;
        let result = db.finish_task_mining(episode);
        if result.as_ref().is_ok_and(|result| {
            !result["drafts"].is_number() && result["status"] != "pending_model_binding"
        }) {
            return Ok(());
        }
        let Some(_lock) = finalizer_lock(false)? else {
            return Ok(());
        };
        let Some(owner) = claim(db, episode)? else {
            return Ok(());
        };
        if result.is_err() {
            db.record_task_finalizer(
                episode,
                &owner,
                FinalizerState::Pending("capture_or_source_unavailable"),
            )?;
            return Ok(());
        }
        spawn_finalizer(db, episode, &owner)
    })();
    if result.is_err() {
        eprintln!("Mastermind: local task finalizer remains pending");
    }
}

pub(super) fn on_source_update(db: &mut Journal, grant: &Grant, episode: &str) {
    on_stop(db, grant, episode);
}

fn finalize(db: &mut Journal, episode: &str, owner: &str, wait: bool) -> Result<bool, Error> {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if !db.owns_task_finalizer(episode, owner, now())? {
            return Err("task finalizer ownership expired".into());
        }
        let captured = db.episode(episode)?;
        if !enabled(Path::new(&captured.project_root), &captured.client)? {
            db.record_task_finalizer(
                episode,
                owner,
                FinalizerState::Pending("task_mining_disabled"),
            )?;
            return Ok(false);
        }
        let input = db.snapshot(episode)?;
        if !input.coverage_gaps.is_empty() {
            db.record_task_finalizer(
                episode,
                owner,
                FinalizerState::Pending("capture_incomplete"),
            )?;
            return Ok(false);
        }
        if let Some((model, binding)) = native::pending_model(&input)? {
            if db.bind_model(&input, model, binding)? {
                if let Some(grant) =
                    db.grant(&captured.client, Path::new(&captured.project_root))?
                {
                    super::local::automatic(db, &grant, episode);
                }
            }
        }
        let result = db.finish_task_mining(episode)?;
        if result["status"] == "completed" {
            return Ok(true);
        }
        if result["status"] != "pending_model_binding" {
            return Err("task mining proposal unavailable".into());
        }
        if !wait || Instant::now() >= deadline {
            db.record_task_finalizer(
                episode,
                owner,
                FinalizerState::Pending("transcript_pending"),
            )?;
            return Ok(false);
        }
        db.record_task_finalizer(
            episode,
            owner,
            FinalizerState::Waiting("transcript_pending"),
        )?;
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn run_owned(db: &mut Journal, episode: &str, owner: &str, wait: bool) -> Result<bool, Error> {
    let result = finalize(db, episode, owner, wait);
    if result.is_err() {
        let _ =
            db.record_task_finalizer(episode, owner, FinalizerState::Pending("finalizer_failed"));
    }
    result
}

pub(super) fn finish(episode: &str) -> Result<(), Error> {
    super::check_id(episode)?;
    let mut db = Journal::open(true)?;
    // Hook admission never waits for this lock. The worker holds it through
    // optional profile I/O, whose duration can exceed the binding lease.
    let lock = finalizer_lock(true)?.ok_or("task finalizer lock unavailable")?;
    let owner = match std::env::var("MASTERMIND_FINALIZER_OWNER") {
        Ok(owner) => {
            super::check_id(&owner)?;
            db.renew_task_finalizer(episode, &owner, now())?;
            owner
        }
        Err(std::env::VarError::NotPresent) => {
            let Some(owner) = claim(&db, episode)? else {
                return Ok(());
            };
            owner
        }
        Err(error) => return Err(error.into()),
    };
    if run_owned(&mut db, episode, &owner, true)? {
        let captured = db.episode(episode)?;
        let root = Path::new(&captured.project_root);
        let result = (|| -> Result<Option<serde_json::Value>, Error> {
            if !enabled(root, &captured.client)? {
                return Ok(None);
            }
            let reader = super::configured_profile_client(root, &captured.client)?;
            Ok(super::refresh_task_profile(root, reader.as_deref()))
        })();
        match result {
            Ok(Some(result)) if crate::miner::profile::task_refresh_completed(&result) => {
                db.record_task_finalizer(episode, &owner, FinalizerState::Completed(None))?
            }
            Ok(None) => db.record_task_finalizer(
                episode,
                &owner,
                FinalizerState::Completed(Some("profile_access_unavailable")),
            )?,
            _ => {
                db.record_task_finalizer(
                    episode,
                    &owner,
                    FinalizerState::Pending("profile_refresh_failed"),
                )?;
                return Ok(());
            }
        }
        drop(lock);
        if let Some(grant) = db.grant(&captured.client, root)? {
            drain(&mut db, &grant)?;
        }
    }
    Ok(())
}

pub(super) fn drain(db: &mut Journal, grant: &Grant) -> Result<(), Error> {
    if !enabled(Path::new(&grant.project_root), &grant.client)? {
        return Ok(());
    }
    let Some(_lock) = finalizer_lock(false)? else {
        return Ok(());
    };
    for episode in db.pending_task_finalizers(grant)? {
        let Some(owner) = claim(db, &episode)? else {
            continue;
        };
        if run_owned(db, &episode, &owner, false)? {
            spawn_finalizer(db, &episode, &owner)?;
            break;
        }
    }
    Ok(())
}

pub(super) fn status() -> Value {
    json!({"status":"configured","mode":"task","execution":"in_session",
        "separate_model_invocations":0,"agent_adherence":"not_established_by_configuration",
        "submission":"required_including_empty_result","stop_retry_limit":1,
        "output":"unreviewed_drafts_only","local_finalizer":"Stop"})
}
