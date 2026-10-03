//! Native client capture, semantic hypotheses and a reviewed bridge to the
//! existing profile. Hooks record observations; they do not grant tool access.

pub mod background;
mod evaluate;
mod fence;
mod influence;
mod install;
mod journal;
mod profile_context;
pub mod readiness;
mod refiner;
mod semantic;
mod worker;

pub use refiner::Config as RefinerConfig;

use super::{collection, curation, feedback, profile, store};
use journal::{Draft, Incoming, Journal};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{IsTerminal, Read};
use std::ops::ControlFlow;
use std::path::Path;

type Error = Box<dyn std::error::Error>;
pub(super) const EXTRACTOR: &str = "persona-hooks-semantic-v1";
// Native tool envelopes can be much larger than retained evidence. Parse a
// bounded envelope, then keep MAX_TEXT and the incomplete-evidence gates.
const MAX_INPUT: u64 = 4 * 1024 * 1024;
const MAX_TEXT: usize = 16 * 1024;

fn hash(value: &Value) -> String {
    crate::hex::encode(&Sha256::digest(value.to_string().as_bytes()))
}

fn print(value: &Value) -> Result<(), Error> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn client(value: &str) -> Result<(), Error> {
    if !matches!(value, "claude" | "codex") {
        return Err("client must be claude or codex".into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn setup(
    client_id: &str,
    root: &Path,
    write: bool,
    remove: bool,
    profile_client: Option<&str>,
    disable_profile: bool,
    refiner: Option<&RefinerConfig>,
    disable_refiner: bool,
) -> Result<(), Error> {
    print(&setup_report(
        client_id,
        root,
        write,
        remove,
        profile_client,
        disable_profile,
        refiner,
        disable_refiner,
    )?)
}

#[allow(clippy::too_many_arguments)]
pub fn setup_report(
    client_id: &str,
    root: &Path,
    write: bool,
    remove: bool,
    profile_client: Option<&str>,
    disable_profile: bool,
    refiner: Option<&RefinerConfig>,
    disable_refiner: bool,
) -> Result<Value, Error> {
    client(client_id)?;
    let root = root.canonicalize()?;
    if disable_profile && profile_client.is_some() {
        return Err("cannot configure and disable profile delivery together".into());
    }
    if let Some(config) = refiner {
        if remove || disable_refiner {
            return Err("cannot configure and disable a refiner together".into());
        }
        config.validate()?;
    }
    let effective_refiner = if remove || disable_refiner {
        None
    } else if let Some(config) = refiner {
        Some(config.clone())
    } else if journal::path()?.exists() {
        Journal::open(false)?.refiner_config(client_id, &root)?
    } else {
        None
    };
    if let Some(id) = profile_client {
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            return Err("invalid profile client id".into());
        }
    }
    let effective_profile = if remove || disable_profile {
        None
    } else if let Some(reader) = profile_client {
        Some(reader.to_owned())
    } else {
        configured_profile_client(&root, client_id)?
    };
    // Revoke first. Installation grants capture only after the native config
    // has been safely written, so a failed install cannot enable collection.
    if write && (remove || disable_refiner) && journal::path()?.exists() {
        let mut db = Journal::open(true)?;
        if remove {
            db.configure(client_id, &root, false, None, false)?;
        }
        db.configure_refiner(client_id, &root, None)?;
    }
    let mut receipt = install::configure(
        client_id,
        &root,
        write,
        remove,
        effective_refiner.as_ref().map(|c| c.timeout_secs),
        effective_profile.is_some(),
    )?;
    if write && !remove {
        let mut db = Journal::open(true)?;
        if disable_profile || profile_client.is_some() {
            db.configure_profile_delivery(client_id, &root, disable_profile)?;
        }
        let grant = db.configure(client_id, &root, true, effective_profile.as_deref(), false)?;
        if refiner.is_some() || disable_refiner {
            db.configure_refiner(client_id, &root, effective_refiner.as_ref())?;
        }
        receipt["capture_grant"] = serde_json::to_value(grant)?;
    }
    receipt["refiner"] = refiner_status(effective_refiner.as_ref());
    receipt["readiness"] = readiness::report(client_id, &root)?;
    receipt["profile_delivery"] = json!({"client_id":effective_profile,"requires_existing_read_grant":true,
        "trigger":"UserPromptSubmit","selection":"task_paths_role_and_workflow","requires_refiner":false,
        "note":"Each event retains prior context exposure. Dependent observations can be inspected but cannot count as unexposed habit support."});
    receipt["next"]=json!("Restart a client session after setup. Collection remains local; analyze explicitly selects a processor. User-channel citations require authorship attestation and habit review.");
    Ok(receipt)
}

/// Reuse the configured audience, or an already granted native client. This
/// never creates a store or grants a new reader access.
pub fn configured_profile_client(root: &Path, client_id: &str) -> Result<Option<String>, Error> {
    client(client_id)?;
    let root = root.canonicalize()?;
    if crate::onboarding::load(&root)?.is_some_and(|settings| {
        !settings.profile_access || !settings.clients.iter().any(|client| client == client_id)
    }) {
        return Ok(None);
    }
    if journal::path()?.exists() {
        let db = Journal::open(false)?;
        if db.profile_delivery_disabled(client_id, &root)? {
            return Ok(None);
        }
        if let Some(grant) = db.grant(client_id, &root)? {
            if grant.enabled && grant.profile_client.is_some() {
                return Ok(grant.profile_client);
            }
        }
    }
    Ok(crate::onboarding::profile_access(&root, client_id)?.then(|| client_id.to_owned()))
}

pub fn status(client_id: &str, root: &Path) -> Result<(), Error> {
    client(client_id)?;
    let root = root.canonicalize()?;
    let observed = readiness::report(client_id, &root)?;
    let legacy = (|| -> Result<Value, Error> {
        match std::fs::symlink_metadata(journal::path()?) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(json!({"status":"not_configured"}));
            }
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        let db = Journal::open(false)?;
        let config = db.refiner_config(client_id, &root)?;
        Ok(
            json!({"grant":db.grant(client_id,&root)?,"capture_pending":fence::pending(client_id,&root)?,"journal":journal::path()?,
        "coverage":"Native hook events only; no hidden reasoning, complete transcript or universal tool mediation.",
        "protection":"capture_only","source_attribution":"user_channel_requires_attestation",
        "refiner":refiner_status(config.as_ref())}),
        )
    })();
    let mut packet = legacy
        .unwrap_or_else(|_| json!({"status":"unavailable","reason":"capture_journal_unavailable"}));
    packet["readiness"] = observed;
    print(&packet)
}

fn refiner_status(config: Option<&RefinerConfig>) -> Value {
    match config {
        Some(config) => {
            json!({"status":"configured","provider":config.provider,"processor":config.processor,
            "timeout_seconds":config.timeout_secs,"native_delivery":"additional_context","execution_permission":false})
        }
        None => json!({"status":"not_configured","reason":"select_refiner_processor_or_provider"}),
    }
}

pub fn intake(id: &str) -> Result<(), Error> {
    check_id(id)?;
    print(&serde_json::to_value(Journal::open(false)?.intake(id)?)?)
}

pub fn evaluate_refiner(input: &Path, config: &RefinerConfig) -> Result<(), Error> {
    print(&evaluate::evaluate(input, config)?)
}

pub fn bind_task(id: &str, spec: &Path, root: &Path, expected: Option<&str>) -> Result<(), Error> {
    check_id(id)?;
    if let Some(revision) = expected {
        check_id(revision)?;
    }
    print(&journal::task::bind(id, spec, root, expected)?)
}

pub(crate) fn task_intake(root: &Path, spec: &Path) -> Result<Option<(String, Value)>, String> {
    journal::task::load(root, spec).map_err(|error| error.to_string())
}

pub(crate) fn review_queue(root: &Path, audience: &str) -> Value {
    let result = (|| -> Result<Value, Error> {
        if !super::access::valid_client_id(audience) {
            return Err("invalid audience".into());
        }
        let root = root.canonicalize()?;
        let path = store::ProfileStore::db_path().ok_or("profile store is unavailable")?;
        let profile = store::ProfileStore::open_optional_read_only(&path)?
            .ok_or("profile access is absent")?;
        let allowed = || {
            profile
                .reader_allowed(root.to_str().ok_or("invalid project root")?, audience)
                .map_err(Error::from)
        };
        if !allowed()? {
            return Err("profile access denied".into());
        }
        let queue = if journal::path()?.exists() {
            Journal::open(false)?.review_queue(&root)?
        } else {
            json!({"status":"not_configured","total":null,"returned":0,"truncated":false,"items":[]})
        };
        if !allowed()? {
            return Err("profile access revoked".into());
        }
        Ok(queue)
    })();
    result.unwrap_or_else(
        |_| json!({"status":"unavailable","total":null,"returned":0,"truncated":false,"items":[]}),
    )
}

pub fn recover(client_id: &str, root: &Path) -> Result<(), Error> {
    client(client_id)?;
    let root = root.canonicalize()?;
    let mut db = Journal::open(true)?;
    let old = db
        .grant(client_id, &root)?
        .ok_or("capture is not configured")?;
    let grant = db.configure(
        client_id,
        &root,
        old.enabled,
        old.profile_client.as_deref(),
        true,
    )?;
    fence::recover(client_id, &root)?;
    print(
        &json!({"grant":grant,"note":"A new capture generation invalidates earlier evidence. Restart the client session; this does not declare a missing event recovered."}),
    )
}

pub fn receive(client_id: &str, root: &Path) -> Result<(), Error> {
    client(client_id)?;
    // Controller prompts contain generated instructions and may include mined
    // preferences. Inherited native hooks must not turn that input into new
    // user-channel evidence. Refuse before opening any journal or capture fence.
    if std::env::var_os("MMCG_INPUT_ORIGIN").as_deref() == Some(std::ffi::OsStr::new("controller"))
    {
        eprintln!(
            "{}",
            json!({"status":"skipped","reason":"controller_generated_input"})
        );
        return print(&json!({}));
    }
    if std::env::var_os("MASTERMIND_MINER").is_some() || !journal::path()?.exists() {
        return print(&json!({}));
    }
    let root = root.canonicalize()?;
    let delivery = fence::begin(client_id, &root)?;
    let mut db = Journal::open(true)?;
    let Some(grant) = db.begin_capture(client_id, &root)? else {
        delivery.clear()?;
        return print(&json!({}));
    };
    if !delivery.current()? {
        return Err("capture was revoked during delivery admission".into());
    }
    // The committed pending fence survives EOF errors, size violations,
    // client timeouts, crashes and failures to append the observation.
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT + 1)
        .read_to_end(&mut bytes)?;
    let parsed = if bytes.len() as u64 > MAX_INPUT {
        Err("hook input exceeded 4 MiB".to_owned())
    } else {
        crate::setup::parse_json_unique(&bytes)
    };
    let value = match parsed {
        Ok(value) => value,
        Err(_) => {
            db.finish_ignored(&grant, Some("invalid_or_oversized_hook_input"))?;
            return Err(
                "hook input invalid; capture has a coverage gap, inspect hooks status".into(),
            );
        }
    };
    let native_cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .and_then(|p| Path::new(p).canonicalize().ok());
    let Some(native_cwd) = native_cwd else {
        db.finish_ignored(&grant, Some("missing_or_unavailable_cwd"))?;
        return Err("hook cwd unavailable; capture has a coverage gap".into());
    };
    let project = profile::persona_project_id(&root).ok_or("cannot resolve project identity")?;
    if !native_cwd.starts_with(&root)
        || profile::persona_project_id(&native_cwd).as_deref() != Some(project.as_str())
    {
        db.finish_ignored(&grant, None)?;
        delivery.clear()?;
        return print(&json!({}));
    }
    let incoming = match normalize(&value) {
        Ok(event) => event,
        Err(_) => {
            db.finish_ignored(&grant, Some("unsupported_hook_schema"))?;
            return Err("unsupported hook schema; capture has a coverage gap".into());
        }
    };
    let kind = incoming.kind.clone();
    let repository = profile::persona_repository_id(&root).unwrap_or_default();
    if !delivery.current()? {
        return Err("capture was revoked during delivery".into());
    }
    let receipt = db.receive(&grant, incoming, &project, &repository)?;
    delivery.clear()?;
    let mut contexts = Vec::new();
    if kind == "UserPromptSubmit" && receipt["status"] == "recorded" {
        if let (Some(episode), Some(event_id), Some(original)) = (
            receipt["episode"].as_str(),
            receipt["event_id"].as_str(),
            value["prompt"].as_str(),
        ) {
            if let Some((config, mut intake)) =
                db.begin_intake(&grant, episode, event_id, original)?
            {
                if intake.status == "pending" {
                    let started = std::time::Instant::now();
                    let _cancel = worker::Cancellation::install()?;
                    let result = refiner::process(&intake.input, &config);
                    intake = db.finish_intake(
                        &grant,
                        &intake,
                        result,
                        started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    )?;
                }
                if intake.status == "offered" {
                    contexts.push(refiner::context(
                        &intake.input,
                        intake
                            .response
                            .as_ref()
                            .ok_or("offered intake has no response")?,
                    )?);
                } else if intake.status == "degraded" {
                    contexts.push(format!("Mastermind refiner status (advisory metadata): {}. The original request remains authoritative. No workflow handoff was produced.",
                        json!({"intake_id":intake.input.id,"status":intake.status,"reason":intake.reason})));
                }
            }
        }
        if let (Some(reader), Some(episode)) =
            (grant.profile_client.as_deref(), receipt["episode"].as_str())
        {
            let used = contexts.iter().map(String::len).sum::<usize>() + 2 * contexts.len();
            match profile_context::deliver(&mut db, &grant, episode, reader, &root, used) {
                Ok(Some(context)) => contexts.push(context),
                Ok(None) => {}
                Err(_) => eprintln!(
                    "{}",
                    json!({"profile_delivery":"omitted","reason":"profile_or_capture_unavailable"})
                ),
            }
        }
    }
    // Successful native output is intentionally only the client hook protocol.
    // A receipt on stdout would be injected into some clients' model context.
    let output = if contexts.is_empty() {
        json!({})
    } else {
        json!({"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":contexts.join("\n\n")}})
    };
    print(&output)
}

fn identifier<'a>(v: &'a Value, name: &str) -> Result<&'a str, Error> {
    let value = v
        .get(name)
        .and_then(Value::as_str)
        .ok_or("missing hook identifier")?;
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err("invalid hook identifier".into());
    }
    Ok(value)
}

fn normalize(v: &Value) -> Result<Incoming, Error> {
    let session = identifier(v, "session_id")?;
    let kind = identifier(v, "hook_event_name")?;
    let supported = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "Stop",
        "SessionEnd",
        "PreCompact",
        "SubagentStart",
        "SubagentStop",
        "Interrupt",
        "StopFailure",
    ];
    if !supported.contains(&kind) {
        return Err("unsupported hook event".into());
    }
    let turn = v
        .get("turn_id")
        .filter(|value| !value.is_null())
        .map(|_| identifier(v, "turn_id"))
        .transpose()?
        .map(str::to_owned);
    let tool = v
        .get("tool_use_id")
        .filter(|value| !value.is_null())
        .map(|_| identifier(v, "tool_use_id"))
        .transpose()?
        .map(str::to_owned);
    let native_key = if v.get("event_id").is_some() {
        Some(format!("{kind}:event:{}", identifier(v, "event_id")?))
    } else if let Some(tool) = &tool {
        Some(format!("{kind}:tool:{tool}"))
    } else if matches!(kind, "UserPromptSubmit" | "Stop") {
        turn.as_ref().map(|id| format!("{kind}:turn:{id}"))
    } else {
        None
    };
    let (actor, mut origin, mut text) = match kind {
        "UserPromptSubmit" => (
            "user",
            "user_channel_unverified",
            v.get("prompt")
                .and_then(Value::as_str)
                .ok_or("missing prompt")?
                .to_owned(),
        ),
        "Stop" | "SubagentStop" => (
            "assistant",
            "agent",
            v.get("last_assistant_message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        ),
        "PreToolUse" => (
            "assistant",
            "agent",
            serde_json::to_string(
                &json!({"tool_name":v.get("tool_name"),"tool_input":v.get("tool_input")}),
            )?,
        ),
        "PostToolUse" | "PostToolUseFailure" => (
            "tool",
            "tool",
            serde_json::to_string(
                &json!({"tool_name":v.get("tool_name"),"tool_response":v.get("tool_response"),"error":v.get("error")}),
            )?,
        ),
        _ => ("system", "client", String::new()),
    };
    if kind == "UserPromptSubmit"
        && ([
            "agent_id",
            "parent_session_id",
            "parent_thread_id",
            "forked_from_id",
        ]
        .iter()
        .any(|key| v.get(key).is_some_and(|v| !v.is_null()))
            || v.get("is_automated").and_then(Value::as_bool) == Some(true)
            || v.get("prompt_origin")
                .and_then(Value::as_str)
                .is_some_and(|s| s != "human"))
    {
        origin = "automation_or_agent";
    }
    let mut gap = None;
    if text.len() > MAX_TEXT {
        text.clear();
        gap = Some("oversized_event_content".into());
    } else if feedback::looks_secret(&text) || crate::indexer::secret_like_documentation(&text) {
        text.clear();
        gap = Some("redacted_event_content".into());
    }
    let forked = ["parent_session_id", "parent_thread_id", "forked_from_id"]
        .iter()
        .any(|key| v.get(key).is_some_and(|v| !v.is_null()));
    let tool_name = v.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let tool_input = v
        .get("tool_input")
        .map(Value::to_string)
        .unwrap_or_default();
    let profile_read = matches!(kind, "PreToolUse" | "PostToolUse" | "PostToolUseFailure")
        && (tool_name.ends_with("mmcg_profile")
            || tool_name.ends_with("mmcg_context")
            || (matches!(tool_name, "Read" | "Bash" | "read_file" | "exec_command")
                && (tool_input.contains("style.md")
                    || tool_input.contains("mmcg context")
                    || tool_input.contains("mastermind context"))));
    let profile_exposure = profile_read.then(|| {
        json!({"status":"possible_tool_exposure","tool":tool_name,
        "native_event_digest":hash(v),"profile_revision":"unknown"})
    });
    Ok(Incoming {
        native_session: session.into(),
        native_turn: turn,
        native_key,
        digest: hash(v),
        kind: kind.into(),
        actor: actor.into(),
        origin: origin.into(),
        text,
        tool_id: tool,
        gap,
        forked,
        fresh_start: kind == "SessionStart"
            && v.get("source").and_then(Value::as_str) == Some("startup")
            && !forked,
        profile_exposure,
    })
}

pub fn episodes(root: &Path, limit: usize, after: Option<&str>) -> Result<(), Error> {
    let root = root.canonicalize()?;
    if !journal::path()?.exists() {
        return print(&json!({"episodes":[]}));
    }
    let list = Journal::open(false)?.list(&root, limit + 1, after.unwrap_or(""))?;
    let next = if list.len() > limit {
        list.get(limit - 1).and_then(|e| e["id"].as_str())
    } else {
        None
    };
    print(&json!({"episodes":list.iter().take(limit).collect::<Vec<_>>(),"next_after":next}))
}

pub fn show(episode: &str) -> Result<(), Error> {
    check_id(episode)?;
    let db = Journal::open(false)?;
    print(
        &json!({"episode":db.snapshot(episode)?,"capture":db.episode(episode)?,"drafts":db.draft_receipts(episode)?,"intake":db.intake_for_episode(episode)?,
        "note":"User-channel text is unverified authorship. Stop is an observed boundary, not task completion. Tool output is not proof of a human preference."}),
    )
}

pub fn analyze(
    episode: &str,
    revision: &str,
    processor: Option<&Path>,
    provider: Option<&str>,
    args: &[String],
    timeout: u64,
) -> Result<(), Error> {
    check_id(episode)?;
    check_id(revision)?;
    let input = Journal::open(false)?.snapshot(episode)?;
    if input.revision != revision {
        return Err("episode revision changed; inspect again".into());
    }
    let drafts = match (processor, provider) {
        (Some(processor), None) => semantic::analyze(&input, processor, args, timeout)?,
        (None, Some("claude")) if args.is_empty() => semantic::analyze_claude(&input, timeout)?,
        _ => return Err("select either --processor with arguments or --provider claude".into()),
    };
    let processor_receipt = json!({"path":processor,"provider":provider,"arguments_digest":hash(&json!(args)),"protocol":EXTRACTOR});
    let mut db = Journal::open(true)?;
    let current = db.snapshot(episode)?;
    if current.revision != revision || !current.coverage_gaps.is_empty() {
        return Err("episode changed or capture became incomplete during analysis".into());
    }
    print(
        &json!({"drafts":db.store_drafts(&input,&drafts,processor_receipt)?,
        "note":"Unreviewed semantic hypotheses. Inspect exact citations and attest authorship before proposing a habit; no draft is active in LLM context."}),
    )
}

/// An explicitly invoked worker, separate from the latency-sensitive hook.
/// Completed checkpoints include empty results. Short leases avoid duplicate
/// provider requests from simultaneous workers for one episode revision.
pub fn mine(
    root: &Path,
    processor: Option<&Path>,
    provider: Option<&str>,
    args: &[String],
    timeout: u64,
    limit: usize,
    after: Option<&str>,
) -> Result<(), Error> {
    let report = mine_page(root, processor, provider, args, timeout, limit, after)?;
    print(&report)?;
    if report["failed"] == true {
        return Err(
            "semantic worker stopped after a processor failure; the failed episode can be retried"
                .into(),
        );
    }
    Ok(())
}

/// Watch the local journal in one explicitly started foreground process.
/// A separate read connection makes data_version meaningful across batches.
pub fn follow(
    root: &Path,
    processor: Option<&Path>,
    provider: Option<&str>,
    args: &[String],
    timeout: u64,
    limit: usize,
) -> Result<(), Error> {
    use std::time::{Duration, Instant};

    let root = root.canonicalize()?;
    let _cancellation = worker::Cancellation::install()?;
    let observer = Journal::open(false)?;
    let mut last_version = None;
    let mut last_pass = Instant::now();
    print(
        &json!({"status":"following","project_root":root,"note":"New eligible episodes are sent to the selected processor while this foreground worker runs. Ctrl-C stops it. Drafts require review."}),
    )?;
    while !worker::interrupted() {
        let version = observer.data_version()?;
        // A periodic pass also retries expired leases left by crashed workers.
        if last_version != Some(version) || last_pass.elapsed() >= Duration::from_secs(30) {
            let mut after = None;
            loop {
                let report = mine_page(
                    &root,
                    processor,
                    provider,
                    args,
                    timeout,
                    limit,
                    after.as_deref(),
                )?;
                if worker::interrupted() {
                    break;
                }
                if report["results"]
                    .as_array()
                    .is_some_and(|rows| !rows.is_empty())
                {
                    print(&report)?;
                }
                if report["failed"] == true {
                    return Err(
                        "semantic worker stopped after a processor failure; restart to retry"
                            .into(),
                    );
                }
                after = report["next_after"].as_str().map(str::to_owned);
                if after.is_none() {
                    break;
                }
            }
            last_version = Some(version);
            last_pass = Instant::now();
        }
        for _ in 0..20 {
            if worker::interrupted() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    print(&json!({"status":"stopped"}))
}

fn mine_page(
    root: &Path,
    processor: Option<&Path>,
    provider: Option<&str>,
    args: &[String],
    timeout: u64,
    limit: usize,
    after: Option<&str>,
) -> Result<Value, Error> {
    mine_page_controlled(root, processor, provider, args, timeout, limit, after, None)
}

#[allow(clippy::too_many_arguments)]
fn mine_page_controlled(
    root: &Path,
    processor: Option<&Path>,
    provider: Option<&str>,
    args: &[String],
    timeout: u64,
    limit: usize,
    after: Option<&str>,
    mut control: Option<&mut dyn background::BatchControl>,
) -> Result<Value, Error> {
    let root = root.canonicalize()?;
    if let Some(after) = after {
        check_id(after)?;
    }
    if !journal::path()?.exists() {
        return Ok(json!({"results":[],"next_after":null,"failed":false}));
    }
    let processor_receipt = background::processor_fingerprint(&root, processor, provider, args)?;
    let mut db = Journal::open(true)?;
    let rows = db.list(&root, 33, after.unwrap_or(""))?;
    let mut results = vec![];
    let mut skipped = vec![];
    let mut next = None;
    let mut visited = 0;
    let mut attempts = 0;
    let mut failure = false;
    for row in rows.iter().take(32) {
        if worker::interrupted() || attempts >= limit {
            break;
        }
        let id = row["id"].as_str().ok_or("episode id unavailable")?;
        next = Some(id.to_owned());
        visited += 1;
        let input = db.snapshot(id)?;
        if control
            .as_ref()
            .is_some_and(|control| control.client() != input.client)
        {
            continue;
        }
        if !input.coverage_gaps.is_empty() {
            skipped.push(json!({"episode":id,"reason":"incomplete"}));
            continue;
        }
        let Some(lease) = db.claim_analysis(id, &input.revision, &processor_receipt, timeout)?
        else {
            skipped.push(json!({"episode":id,"reason":"completed_or_leased_or_changed"}));
            continue;
        };
        attempts += 1;
        let result = (|| -> Result<Vec<journal::Draft>, Error> {
            if let Some(control) = control.as_mut() {
                control.before_attempt()?;
            }
            let drafts = match (processor, provider) {
                (Some(processor), None) => semantic::analyze(&input, processor, args, timeout)?,
                (None, Some("claude")) if args.is_empty() => {
                    semantic::analyze_claude(&input, timeout)?
                }
                _ => {
                    return Err(
                        "select either --processor with arguments or --provider claude".into(),
                    )
                }
            };
            let current = db.snapshot(id)?;
            if current.revision != input.revision || !current.coverage_gaps.is_empty() {
                return Err("episode changed during analysis".into());
            }
            if let Some(control) = control.as_mut() {
                control.before_publish()?;
            }
            db.store_drafts(&input, &drafts, processor_receipt.clone())
        })();
        match result {
            Ok(drafts) => {
                results.push(json!({"episode":id,"revision":input.revision,"drafts":drafts}))
            }
            Err(error) => {
                db.analysis_failed(id, &input.revision, &processor_receipt, lease)?;
                if worker::interrupted() {
                    break;
                }
                if db.snapshot(id).is_ok_and(|current| {
                    current.revision != input.revision || !current.coverage_gaps.is_empty()
                }) {
                    // A new turn can supersede a draft while a provider runs.
                    // Retry its new revision on the next pass without forcing
                    // the user to restart a healthy continuous worker.
                    skipped.push(json!({"episode":id,"reason":"changed_during_analysis"}));
                    continue;
                }
                results.push(json!({"episode":id,"error":error.to_string()}));
                failure = true;
                break;
            }
        }
    }
    let next = if visited < rows.len() { next } else { None };
    Ok(
        json!({"results":results,"skipped":skipped,"next_after":next,"failed":failure,
        "note":"Only unreviewed hypotheses are stored. No provider requests are scheduled after this worker exits."}),
    )
}

pub fn draft(id: &str) -> Result<(), Error> {
    check_id(id)?;
    let db = Journal::open(false)?;
    let draft = db.draft(id)?;
    let snapshot = db.snapshot(&draft.episode)?;
    print(
        &json!({"draft":draft,"current":snapshot.revision==draft.episode_revision && snapshot.coverage_gaps.is_empty(),
        "evidence_class":semantic::evidence_class(&snapshot,&draft.content).ok(),
        "promotion_eligible":snapshot.revision==draft.episode_revision && semantic::validate_for_promotion(&snapshot,std::slice::from_ref(&draft.content)).is_ok(),
        "episode":snapshot}),
    )
}

pub fn forget(id: &str, revision: &str) -> Result<(), Error> {
    check_id(id)?;
    check_id(revision)?;
    Journal::open(true)?.forget(id, revision)?;
    print(
        &json!({"forgotten":id,"note":"Dependent profile citations are now unavailable and withheld on verified reads."}),
    )
}

fn check_id(id: &str) -> Result<(), Error> {
    if !collection::valid_id(id) {
        return Err("expected a full 64-character id or revision".into());
    }
    Ok(())
}

fn candidate(db: &Journal, draft: &Draft) -> Result<store::CollectedCandidate, Error> {
    let input = db.snapshot(&draft.episode)?;
    if !draft.attested || input.revision != draft.episode_revision {
        return Err("draft needs current human authorship attestation".into());
    }
    semantic::validate_for_promotion(&input, std::slice::from_ref(&draft.content))?;
    let citation = draft
        .content
        .supports
        .first()
        .ok_or("draft has no human citation")?;
    let (index, event) = input
        .events
        .iter()
        .enumerate()
        .find(|(_, e)| e.id == citation.event_id)
        .ok_or("citation unavailable")?;
    let ep = db.episode(&draft.episode)?;
    Ok(store::CollectedCandidate {
        id: draft.id.clone(),
        source: format!("hook:{}", ep.session),
        kind: "possible_hook_habit".into(),
        quote: citation.quote.clone(),
        source_path: journal::path()?.to_string_lossy().into_owned(),
        line_no: index + 1,
        segment_no: 0,
        record_digest: hash(&serde_json::to_value(event)?),
        project_root: input.project_root,
        project: input.project,
        observed_at: ep.observed_at,
        extractor: EXTRACTOR.into(),
        rule_id: draft.id.clone(),
        revision: hash(&json!([
            draft.revision,
            draft.episode_revision,
            draft.attested_episode,
            true
        ])),
        status: "pending".into(),
        present: true,
    })
}

/// All public profile projections revalidate the complete episode receipt,
/// including coverage, revision and explicit attribution, not just a quote.
pub(super) fn current_candidate(c: &store::CollectedCandidate) -> bool {
    if c.extractor != EXTRACTOR || c.id != c.rule_id {
        return false;
    }
    (|| -> Result<bool, Error> {
        let db = Journal::open(false)?;
        let draft = db.draft(&c.id)?;
        let expected = candidate(&db, &draft)?;
        let mut actual = c.clone();
        actual.status = "pending".into();
        actual.present = true;
        Ok(expected == actual)
    })()
    .unwrap_or(false)
}

pub(super) fn require_episode(c: &store::CollectedCandidate, episode: &str) -> Result<(), Error> {
    if c.extractor == EXTRACTOR
        && Journal::open(false)?
            .draft(&c.id)?
            .attested_episode
            .as_deref()
            != Some(episode)
    {
        return Err(
            "hook evidence must retain the task identity confirmed during authorship attestation"
                .into(),
        );
    }
    Ok(())
}

pub fn propose(
    id: &str,
    revision: &str,
    episode: &str,
    attest_human: bool,
    target: Option<i64>,
) -> Result<(), Error> {
    check_id(id)?;
    check_id(revision)?;
    if !attest_human {
        return Err("inspect the draft and use --attest-human only after confirming that the cited words are the person's own, not pasted, delegated or automatically generated text".into());
    }
    if !std::io::stdin().is_terminal() {
        return Err("authorship attestation requires the author's interactive terminal".into());
    }
    let task = super::habit::episode(episode)?;
    let mut journal = Journal::open(true)?;
    let draft = journal.draft(id)?;
    if !draft.content.contradictions.is_empty() {
        return Err(
            "draft has counterevidence; resolve and qualify the hypothesis before proposing it"
                .into(),
        );
    }
    let draft = journal.attest(id, revision, &task)?;
    let selected = candidate(&journal, &draft)?;
    let ep = journal.episode(&draft.episode)?;
    let source_id = selected.source.clone();
    let candidates = journal
        .attested_drafts(&ep.session)?
        .iter()
        .filter_map(|d| candidate(&journal, d).ok())
        .collect::<Vec<_>>();
    if !candidates
        .iter()
        .any(|candidate| candidate.id == selected.id)
    {
        return Err("selected draft is no longer current; inspect its episode again".into());
    }
    let source = store::CollectionSource {
        source: source_id.clone(),
        source_path: selected.source_path.clone(),
        project_root: ep.project_root,
        project: ep.project,
        repository: ep.repository,
        snapshot_digest: hash(&serde_json::to_value(&candidates)?),
        extractor: EXTRACTOR.into(),
        bytes: serde_json::to_vec(&candidates)?.len(),
        lines: candidates.len(),
    };
    let path = store::ProfileStore::db_path().ok_or("could not resolve profile store")?;
    profile::mutate_private_store(
        &path,
        |_| Ok(ControlFlow::Continue(())),
        |db, ()| {
            if !current_candidate(&selected) {
                return Err("hook evidence changed before transfer".into());
            }
            db.collect_candidates(&[store::CollectionBatch { source, candidates }])?;
            db.set_source_sync_enabled(&source_id, &selected.project_root, false)?;
            Ok(())
        },
    )?;
    curation::propose_habit(
        &selected.id,
        &selected.revision,
        curation::HabitDraft {
            episode: task,
            when: draft.content.when,
            behavior: draft.content.behavior,
            outcome: "Unknown; not established by captured evidence.".into(),
            exception: draft.content.exception,
            global: false,
            role: draft.content.role.unwrap_or_default(),
            workflow: draft.content.workflow.unwrap_or_default(),
        },
        target,
    )
}
