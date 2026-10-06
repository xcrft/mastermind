//! The current native agent proposes candidates. This module performs local
//! admission and sealing only; it never invokes a model.
use super::{
    journal::{Grant, Journal},
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

pub(super) fn on_stop(db: &mut Journal, grant: &Grant, episode: &str) {
    if !enabled(Path::new(&grant.project_root), &grant.client).unwrap_or(false) {
        return;
    }
    // Sealing a ready proposal is cheap. Delayed transcript binding and Git
    // scans run outside the native hook's three-second deadline.
    let _ = db.finish_task_mining(episode);
    spawn_finalizer(episode);
}

pub(super) fn on_source_update(db: &mut Journal, grant: &Grant, episode: &str) {
    if !enabled(Path::new(&grant.project_root), &grant.client).unwrap_or(false) {
        return;
    }
    // A matching tool result may arrive after Stop. Recheck the staged proposal
    // locally; completed drafts never rebind to changed source automatically.
    if db
        .finish_task_mining(episode)
        .is_ok_and(|result| result["status"] == "pending_model_binding")
    {
        spawn_finalizer(episode);
    }
}

fn spawn_finalizer(episode: &str) {
    let result = std::env::current_exe().and_then(|exe| {
        Command::new(exe)
            .args(["miner", "hooks", "finish-task", "--episode", episode])
            .env("MASTERMIND_MINER", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    });
    if result.is_err() {
        eprintln!("Mastermind: local task finalizer unavailable");
    }
}

pub(super) fn finish(episode: &str) -> Result<(), Error> {
    super::check_id(episode)?;
    let mut db = Journal::open(true)?;
    let captured = db.episode(episode)?;
    let root = Path::new(&captured.project_root);
    if !enabled(root, &captured.client)? {
        return Ok(());
    }
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let input = db.snapshot(episode)?;
        if !input.coverage_gaps.is_empty() {
            break;
        }
        if let Some((model, binding)) = native::pending_model(&input)? {
            if db.bind_model(&input, model, binding)? {
                if let Some(grant) = db.grant(&captured.client, root)? {
                    super::local::automatic(&mut db, &grant, episode);
                }
            }
        }
        let result = db.finish_task_mining(episode)?;
        if result["status"] != "pending_model_binding" || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if enabled(root, &captured.client)? {
        let reader = super::configured_profile_client(root, &captured.client)?;
        super::refresh_task_profile(root, reader.as_deref());
    }
    Ok(())
}

pub(super) fn status() -> Value {
    json!({"status":"configured","mode":"task","execution":"in_session",
        "separate_model_invocations":0,"agent_adherence":"not_established_by_configuration",
        "submission":"required_including_empty_result","stop_retry_limit":1,
        "output":"unreviewed_drafts_only","local_finalizer":"Stop"})
}
