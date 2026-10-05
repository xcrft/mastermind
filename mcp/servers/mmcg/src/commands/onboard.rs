//! A small coordinator over scaffold, setup, hooks and the managed miner.

use mmcg::miner::hooks::{self, background, RefinerConfig};
use mmcg::onboarding::{self, Mining, Session, Settings};
use serde_json::{json, Value};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

type Error = Box<dyn std::error::Error>;

#[derive(clap::Args)]
pub struct Options {
    /// Project directory. Choices are saved in .mastermind/setup.json.
    #[arg(default_value = ".")]
    pub root: PathBuf,
    /// Client to connect. Defaults to the active client, or installed native clients.
    #[arg(long, value_parser = ["claude", "codex", "all", "none"])]
    client: Option<String>,
    /// task mines in the current agent task, on starts a separate miner, capture records locally, off disables hooks.
    #[arg(long, value_parser = ["off", "capture", "task", "on"])]
    mining: Option<String>,
    /// native uses each captured client's login and model for mining and refinement.
    #[arg(long, value_parser = ["native", "claude", "codex"])]
    provider: Option<String>,
    /// Maximum provider attempts per client run. Repeated init never renews a run.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=10_000))]
    max_calls: Option<u64>,
    /// Maximum wall time per client run in seconds, including idle time.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=86_400))]
    max_runtime: Option<u64>,
    /// Allow selected clients to read the personal profile. Enabled on first client setup.
    #[arg(long, value_parser = ["on", "off"])]
    profile_access: Option<String>,
    /// Refine each user prompt through the selected provider. Extra calls are outside the miner budget.
    #[arg(long, value_parser = ["on", "off"])]
    refiner: Option<String>,
    /// Install bundled workflows for the selected clients.
    #[arg(long, value_parser = ["on", "off"], conflicts_with = "no_global")]
    workflow: Option<String>,
    /// Show the saved choices and proposed steps without writing or running clients.
    #[arg(long)]
    dry_run: bool,
    #[arg(long, conflicts_with = "draft_with")]
    pub json: bool,
    /// Replace scaffold documents, retaining backups. Client customizations remain protected.
    #[arg(long)]
    force: bool,
    /// Skip this index refresh.
    #[arg(long)]
    no_index: bool,
    /// Explicitly draft new scaffold documents with Claude. May edit files and use provider calls.
    #[arg(long, value_parser = ["claude"], conflicts_with = "no_claude")]
    draft_with: Option<String>,
    /// Compatibility flag. Automatic drafting is already disabled by default.
    #[arg(long, hide = true)]
    no_claude: bool,
    /// Skip installing global workflow files. Saved for subsequent init runs.
    #[arg(long)]
    no_global: bool,
    /// Seed the personal profile from this repository's authored Git history.
    #[arg(long, conflicts_with = "no_seed_style")]
    seed_style: bool,
    #[arg(long, hide = true)]
    no_seed_style: bool,
}

fn ask(message: &str, default: &str, choices: &[&str]) -> Result<String, Error> {
    eprint!("{message} [{default}]: ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let answer = answer.trim();
    let answer = if answer.is_empty() { default } else { answer };
    if !choices.contains(&answer) {
        return Err(format!("choose {}", choices.join(" | ")).into());
    }
    Ok(answer.into())
}

fn selection(
    options: &Options,
    root: &Path,
    previous: Option<&Settings>,
) -> Result<Settings, Error> {
    let mut settings = previous.cloned().unwrap_or_else(|| Settings::local(root));
    let mut client = options
        .client
        .clone()
        .or_else(|| previous.is_none().then(|| detected_client(root)).flatten());
    if previous.is_none()
        && client.is_none()
        && !options.dry_run
        && !options.json
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
    {
        client = Some(ask(
            "AI client: claude | codex | all | none",
            "claude",
            &["claude", "codex", "all", "none"],
        )?);
    }
    if let Some(client) = client {
        settings.clients = match client.as_str() {
            "all" => vec!["claude".into(), "codex".into()],
            "none" => Vec::new(),
            _ => vec![client],
        };
        if previous.is_none() && !settings.clients.is_empty() {
            settings.mining = if cfg!(unix) {
                Mining::Task
            } else {
                Mining::Off
            };
            settings.profile_access = true;
        }
        if settings.clients.is_empty() {
            settings.mining = Mining::Off;
            settings.refiner = false;
            settings.profile_access = false;
        }
    }
    if let Some(mining) = &options.mining {
        settings.mining = match mining.as_str() {
            "on" => Mining::On,
            "task" => Mining::Task,
            "capture" => Mining::Capture,
            _ => Mining::Off,
        };
    }
    if let Some(provider) = &options.provider {
        settings.provider = Some(provider.clone());
    }
    if (settings.mining == Mining::On || options.refiner.as_deref() == Some("on"))
        && settings.provider.is_none()
    {
        settings.provider = Some("native".into());
    }
    if let Some(value) = options.max_calls {
        settings.max_calls = value;
    }
    if let Some(value) = options.max_runtime {
        settings.max_runtime = value;
    }
    if let Some(value) = &options.profile_access {
        settings.profile_access = value == "on";
    }
    if let Some(value) = &options.refiner {
        settings.refiner = value == "on";
    } else if settings.mining == Mining::Off {
        settings.refiner = false;
    }
    if let Some(value) = &options.workflow {
        settings.workflow = value == "on";
    }
    if options.no_global {
        settings.workflow = false;
    } else if previous.is_none() && options.workflow.is_none() {
        settings.workflow = std::env::var_os("MASTERMIND_INSTALLER_JS").is_some()
            && std::env::var_os("MASTERMIND_NODE").is_some();
    }
    if let Some(previous) = previous {
        settings.pending_removals.extend(
            previous
                .clients
                .iter()
                .filter(|client| !settings.clients.contains(client))
                .cloned(),
        );
        settings
            .pending_removals
            .retain(|client| !settings.clients.contains(client));
        settings.pending_removals.sort();
        settings.pending_removals.dedup();
    }
    settings.validate(root)?;
    Ok(settings)
}

fn detected_client(root: &Path) -> Option<String> {
    // Resolve executables without running them. Active-client markers take
    // precedence over other installed clients; saved and explicit choices win.
    let available = |client| mmcg::setup::resolve_native_cli(client, root).is_ok();
    if std::env::var_os("CLAUDECODE").is_some_and(|value| !value.is_empty()) && available("claude")
    {
        return Some("claude".into());
    }
    if std::env::var_os("CODEX_THREAD_ID").is_some_and(|value| !value.is_empty())
        && available("codex")
    {
        return Some("codex".into());
    }
    match (available("claude"), available("codex")) {
        (true, true) => Some("all".into()),
        (true, false) => Some("claude".into()),
        (false, true) => Some("codex".into()),
        (false, false) => None,
    }
}

fn worker_options(settings: &Settings) -> background::StartOptions {
    background::StartOptions {
        provider: settings.provider.clone(),
        max_calls: Some(settings.max_calls),
        max_runtime: Some(settings.max_runtime),
        ..Default::default()
    }
}

fn stage(steps: &mut Vec<Value>, name: &str, result: Result<Value, Error>) -> bool {
    match result {
        Ok(detail) => {
            steps.push(json!({"component":name,"status":"ok","detail":detail}));
            true
        }
        Err(error) => {
            steps.push(json!({"component":name,"status":"failed","reason":error.to_string()}));
            false
        }
    }
}

fn mcp(root: &Path, client: &str, write: bool) -> Result<Value, Error> {
    let mut args = vec![
        "setup".into(),
        client.into(),
        "--root".into(),
        root.to_string_lossy().into_owned(),
        "--profile-client".into(),
        client.into(),
    ];
    if write {
        args.push("--write".into());
    }
    let report = mmcg::setup::run_bounded_command(
        &std::env::current_exe()?,
        &args,
        root,
        Duration::from_secs(45),
    )?;
    if !report.success || report.stdout_truncated || report.stderr_truncated {
        return Err(format!("MCP registration could not be reconciled. Run mastermind setup {client} to inspect custom configuration or a missing client").into());
    }
    let stdout = String::from_utf8(report.stdout)?;
    Ok(
        json!({"status": if stdout.contains("outcome=no_change") || (write && stdout.contains("outcome=wrote")) {"configured"} else {"missing_or_stale"},
        "profile_client":client,"activation":"restart_client_session","runtime_connection":"not_observed"}),
    )
}

fn workflow(root: &Path, client: &str, write: bool) -> Result<Value, Error> {
    let installer = std::env::var_os("MASTERMIND_INSTALLER_JS")
        .map(PathBuf::from)
        .ok_or("workflow bundle unavailable. Install the npm package or select --workflow off")?;
    let node = std::env::var_os("MASTERMIND_NODE")
        .map(PathBuf::from)
        .ok_or("npm wrapper did not supply Node.js")?;
    let mut args = vec![if write {
        installer.with_file_name("mastermind.js")
    } else {
        installer
    }
    .to_string_lossy()
    .into_owned()];
    if write {
        args.extend(["update".into(), "--workflow-only".into()]);
    } else {
        args.push("doctor".into());
    }
    args.extend(["--client".into(), client.into(), "--json".into()]);
    let result = mmcg::setup::run_bounded_command(&node, &args, root, Duration::from_secs(60))?;
    if result.stdout_truncated || result.stderr_truncated {
        return Err("workflow output exceeded its bound".into());
    }
    let detail: Value = serde_json::from_slice(&result.stdout)
        .map_err(|_| "workflow command did not return JSON")?;
    if !result.success {
        return Err(format!("workflow reconciliation failed: {detail}").into());
    }
    Ok(detail)
}

pub fn init(options: Options, index_override: Option<&Path>) -> Result<bool, Error> {
    let root = options.root.canonicalize()?;
    let previous = onboarding::load(&root)?;
    let mut settings = selection(&options, &root, previous.as_ref())?;
    if options.dry_run {
        emit(
            &json!({"schema_version":1,"status":"planned","settings":settings,
            "steps":["scaffold","index_code_and_documents","workflows","mcp","profile_access","hooks","mining"],
            "provider_calls":false,"writes":false}),
            options.json,
        )?;
        return Ok(true);
    }
    let mut session = Session::begin(&root)?;
    if session.settings() != previous.as_ref() {
        return Err("setup changed while selecting options. Run init again".into());
    }
    // Save intent first. Actual component state is inspected separately so a
    // partial failure can be retried without pretending that setup succeeded.
    session.save(&settings)?;
    let mut steps = Vec::new();
    let scaffold = super::init::scaffold(
        &root,
        super::init::InitOpts {
            index_path: index_override.map(Path::to_path_buf),
            force: options.force,
            index: !options.no_index,
            claude: options.draft_with.is_some(),
            claude_file: settings.clients.iter().any(|client| client == "claude"),
            seed_style: options.seed_style && !options.no_seed_style,
        },
    );
    match scaffold {
        Ok(report) => steps.push(json!({"component":"project", "status":if report.errors.is_empty() {"ok"} else {"failed"},
            "reason":if report.errors.is_empty() {None} else {Some(report.errors.join(". "))},"detail":report})),
        Err(error) => { stage(&mut steps, "project", Err(error)); }
    }
    // A removed client must lose its project-specific grants. Shared global
    // workflow files and MCP entries can still serve other repositories.
    for client in settings.pending_removals.clone() {
        let stopped = stage(
            &mut steps,
            &format!("{client}.stop"),
            stop_worker(&root, &client),
        );
        let revoked = stage(
            &mut steps,
            &format!("{client}.remove_hooks"),
            configure_hooks(&root, &client, Mining::Off, false, None),
        );
        let access = stage(
            &mut steps,
            &format!("{client}.revoke_profile"),
            mmcg::miner::access::configure(&root, &client, false)
                .map(|()| json!({"allowed":false})),
        );
        if stopped && revoked && access {
            settings
                .pending_removals
                .retain(|pending| pending != &client);
            session.save(&settings)?;
        }
    }
    for client in &settings.clients {
        if settings.workflow {
            stage(
                &mut steps,
                &format!("{client}.workflow"),
                workflow(&root, client, true),
            );
        }
        stage(
            &mut steps,
            &format!("{client}.mcp"),
            mcp(&root, client, true),
        );
        stage(
            &mut steps,
            &format!("{client}.profile_access"),
            mmcg::miner::access::configure(&root, client, settings.profile_access)
                .map(|()| json!({"allowed":settings.profile_access,"scope":"project_and_client"})),
        );
        if settings.mining != Mining::On {
            stage(
                &mut steps,
                &format!("{client}.mining"),
                stop_worker(&root, client),
            );
        }
        let refiner = settings.refiner.then(|| RefinerConfig {
            processor: None,
            provider: settings.provider.clone(),
            args: Vec::new(),
            timeout_secs: 8,
        });
        let hooks_ok = stage(
            &mut steps,
            &format!("{client}.hooks"),
            configure_hooks(
                &root,
                client,
                settings.mining,
                settings.profile_access,
                refiner.as_ref(),
            ),
        );
        if settings.mining == Mining::On && hooks_ok {
            stage(
                &mut steps,
                &format!("{client}.mining"),
                if settings.provider.as_deref() == Some("native") {
                    background::arm_native(client, &root)
                } else {
                    background::ensure(client, &root, worker_options(&settings))
                },
            );
        }
    }
    if settings.profile_access && !options.no_seed_style {
        if let Some(reader) = settings.clients.first() {
            let detail = mmcg::miner::profile::refresh_for_task(&root, reader).unwrap_or_else(
                |error| json!({"status":"unavailable","reason":error.to_string(),"model":false}),
            );
            steps.push(json!({"component":"profile.git","status":"ok","detail":detail}));
        }
    }
    let success = steps.iter().all(|step| step["status"] == "ok");
    let observed = status_report(&root, index_override)?;
    emit(
        &json!({"schema_version":1,"status":if success {"configured"} else {"incomplete"},
        "settings":settings,"steps":steps,"observed":observed}),
        options.json,
    )?;
    Ok(success)
}

fn stop_worker(root: &Path, client: &str) -> Result<Value, Error> {
    if !cfg!(unix) {
        return Ok(json!({"status":"unsupported","requested":"off"}));
    }
    let report = background::stop(client, root)?;
    if report["owner"] == "held" {
        return Err("worker is still stopping. Repeat init after the current attempt exits".into());
    }
    Ok(report)
}

fn configure_hooks(
    root: &Path,
    client: &str,
    mining: Mining,
    access: bool,
    refiner: Option<&RefinerConfig>,
) -> Result<Value, Error> {
    if !cfg!(unix) && mining == Mining::Off {
        return Ok(json!({"status":"disabled","platform":"unsupported"}));
    }
    hooks::setup_report(
        client,
        root,
        true,
        mining == Mining::Off,
        access.then_some(client),
        !access,
        refiner,
        refiner.is_none(),
    )
}

pub fn status_report(root: &Path, index_override: Option<&Path>) -> Result<Value, Error> {
    let (settings, settings_error) = match onboarding::load(root) {
        Ok(settings) => (settings, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let workflow_status = mmcg::workflow_status::WorkflowStatus::scan_with_index(
        root,
        &index_override
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.join(".mastermind/mmcg.db")),
    );
    let mut clients = Vec::new();
    let defaults = ["claude".to_string(), "codex".to_string()];
    let managed = settings
        .as_ref()
        .map(|settings| {
            settings
                .clients
                .iter()
                .chain(&settings.pending_removals)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| defaults.iter().collect());
    for client in managed {
        let hooks = hooks::readiness::report(client, root)?;
        let mcp = mmcg::setup::inspect_profile_registration(
            if client == "claude" {
                mmcg::setup::Client::Claude
            } else {
                mmcg::setup::Client::Codex
            },
            root,
            &std::env::current_exe()?,
            client,
        )
        .unwrap_or_else(|error| json!({"status":"unavailable","reason":error}));
        let workflow = if settings.as_ref().is_some_and(|settings| settings.workflow) {
            workflow(root, client, false)
                .unwrap_or_else(|error| json!({"status":"unavailable","reason":error.to_string()}))
        } else {
            json!({"status":"not_requested"})
        };
        let access = onboarding::profile_access(root, client)
            .map(|allowed| json!({"allowed":allowed}))
            .unwrap_or_else(|error| json!({"allowed":null,"reason":error.to_string()}));
        clients.push(json!({"client":client,"pending_removal":settings.as_ref().is_some_and(|settings| settings.pending_removals.contains(client)),"mcp":mcp,"workflow":workflow,"hooks":hooks,"profile_access":access}));
    }
    let next = workflow_status
        .next_action()
        .map(|action| json!({"description":action.description,"command":action.command}));
    Ok(
        json!({"schema_version":1,"inspection":"read_only","settings":settings,"settings_error":settings_error,"project":workflow_status,"clients":clients,"next_task_action":next}),
    )
}

pub fn status(root: &Path, index_override: Option<&Path>, json: bool) -> Result<(), Error> {
    emit(&status_report(root, index_override)?, json)
}

fn emit(report: &Value, as_json: bool) -> Result<(), Error> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!(
        "Mastermind {}",
        report["status"].as_str().unwrap_or("status")
    );
    if let Some(steps) = report["steps"].as_array() {
        for step in steps {
            let detail = step["reason"]
                .as_str()
                .or_else(|| step["detail"]["status"].as_str());
            println!(
                "  {}  {}{}",
                step["status"].as_str().unwrap_or("unknown"),
                step["component"].as_str().unwrap_or(""),
                detail
                    .map(|reason| format!(" — {}", reason.escape_debug()))
                    .unwrap_or_default()
            );
            if let Some(attempts) = step["detail"]["run"]["attempts"].as_u64() {
                println!(
                    "    Calls: {attempts}/{}, runtime limit: {} s",
                    step["detail"]["configuration"]["max_calls"],
                    step["detail"]["configuration"]["max_runtime"]
                );
            }
        }
    }
    let observed = report.get("observed").unwrap_or(report);
    if observed.get("project").is_some() {
        let index = &observed["project"]["index"];
        let state = if index["db_exists"] != true {
            "missing"
        } else if [
            "database_error",
            "root_error",
            "freshness_error",
            "history_freshness_error",
        ]
        .iter()
        .any(|key| !index[key].is_null())
        {
            "unavailable"
        } else if index["stale_count"] != 0
            || index["extractor_contract_current"] != true
            || index["concept_contract_current"] != true
            || index["history_freshness"] != "fresh"
        {
            "stale_or_incomplete"
        } else {
            "current"
        };
        println!(
            "  Index {state}: {} files, {} symbols, {} stale files",
            index["file_count"], index["symbol_count"], index["stale_count"]
        );
        if let Some(error) = observed["settings_error"].as_str() {
            println!("  Setup unavailable: {error}");
        }
        for client in observed["clients"].as_array().into_iter().flatten() {
            println!(
                "  {}: MCP {}, capture {}, session {}, miner {}",
                client["client"].as_str().unwrap_or(""),
                client["mcp"]["status"].as_str().unwrap_or("unknown"),
                client["hooks"]["capture"]["status"]
                    .as_str()
                    .unwrap_or("unknown"),
                client["hooks"]["activation"]["status"]
                    .as_str()
                    .unwrap_or("unknown"),
                client["hooks"]["mining"]["status"]
                    .as_str()
                    .unwrap_or("unknown")
            );
            if client["hooks"]["activation"]["status"] == "not_observed" {
                if let Some(activation) =
                    client["hooks"]["native_registration"]["client_activation"].as_str()
                {
                    println!("    Activate: {activation}");
                }
            }
            let evidence = &client["hooks"]["evidence"];
            let analysis = match client["hooks"]["pipeline"]["analysis_requested"].as_bool() {
                Some(true) => "requested",
                Some(false) => "not requested",
                None => "unspecified",
            };
            if evidence["status"] == "available" {
                println!(
                    "    Capture: {} current events, {} historical events; {} complete episodes, {} incomplete; semantic analysis {analysis}",
                    evidence["current"]["events"], evidence["historical"]["events"],
                    evidence["current"]["complete_episodes"], evidence["current"]["incomplete_episodes"]
                );
            } else {
                println!(
                    "    Capture evidence {}; semantic analysis {analysis}",
                    evidence["status"].as_str().unwrap_or("unavailable")
                );
            }
            for action in client["hooks"]["pipeline"]["next_actions"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(action) = action.as_str() {
                    println!("    Next: {action}");
                }
            }
        }
        for task in observed["project"]["tasks"]
            .as_array()
            .into_iter()
            .flatten()
        {
            println!(
                "  Task {}: {}",
                task["folder"].as_str().unwrap_or("").escape_debug(),
                task["phase"].as_str().unwrap_or("unknown")
            );
        }
        if let Some(action) = observed["next_task_action"]["description"].as_str() {
            println!("  Next: {action}");
        }
        if let Some(command) = observed["next_task_action"]["command"].as_str() {
            println!("    {command}");
        }
    }
    if report["status"] == "planned" {
        println!("{}", serde_json::to_string_pretty(&report["settings"])?);
    }
    println!(
        "  Use --json for component details. Restart the selected client to load MCP and hooks."
    );
    Ok(())
}

pub fn miner(
    action: &str,
    root: &Path,
    client: Option<&str>,
    as_json: bool,
) -> Result<bool, Error> {
    let _session = if action == "status" {
        None
    } else {
        Some(Session::begin(root)?)
    };
    let settings =
        onboarding::load(root)?.ok_or("run mastermind init to select a client and mining mode")?;
    if client.is_some_and(|client| !settings.clients.iter().any(|selected| selected == client)) {
        return Err("client is not selected in this project's setup".into());
    }
    if action == "start" && settings.mining != Mining::On {
        return Err("enable semantic mining with init --mining on --provider native first".into());
    }
    let mut steps = Vec::new();
    for selected in settings
        .clients
        .iter()
        .filter(|selected| client.is_none_or(|client| *selected == client))
    {
        let result = match action {
            "start" => background::start(selected, root, worker_options(&settings)),
            "stop" => background::stop(selected, root),
            _ => background::status(selected, root),
        };
        stage(&mut steps, selected, result);
    }
    let success = steps.iter().all(|step| step["status"] == "ok");
    emit(
        &json!({"schema_version":1,"status":if success {"ok"} else {"incomplete"},"steps":steps}),
        as_json,
    )?;
    Ok(success)
}
