//! Read-only setup diagnostics in an isolated home. Configuration and locally
//! observed native events never stand in for a provider call or client trust.
#![cfg(unix)]

use serde_json::{json, Value};
use std::fs;
use std::io::{Seek, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&project).unwrap();
        let f = Self {
            home: home.canonicalize().unwrap(),
            project: project.canonicalize().unwrap(),
            _temp: temp,
        };
        let init = f
            .isolated("/usr/bin/git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .output()
            .unwrap();
        assert!(init.status.success(), "{init:?}");
        f
    }

    fn isolated(&self, executable: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(executable);
        command
            .current_dir(&self.project)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.isolated(env!("CARGO_BIN_EXE_mmcg"))
            .args(args)
            .output()
            .unwrap()
    }

    fn success(&self, args: &[&str]) -> Value {
        parse(self.run(args))
    }

    fn setup(&self, client: &str) -> Value {
        self.success(&["miner", "hooks", "setup", "--client", client, "--write"])
    }

    fn report(&self, client: &str) -> Value {
        self.success(&["miner", "hooks", "status", "--client", client])["readiness"].clone()
    }

    fn doctor(&self, client: &str) -> Value {
        // Unrelated index/MCP checks may fail in this intentionally empty repo.
        let output = self.run(&["doctor", "--json"]);
        let report: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {output:?}"));
        let name = if client == "codex" {
            "Codex hook readiness"
        } else {
            "Claude hook readiness"
        };
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == name)
            .unwrap()
            .clone()
    }

    fn session_start(&self, client: &str, session: &str) {
        let event = json!({"hook_event_name":"SessionStart","event_id":format!("{session}:start"),
            "session_id":session,"source":"startup","cwd":self.project});
        self.event(client, event);
    }

    fn event(&self, client: &str, mut event: Value) {
        event["cwd"] = json!(self.project);
        let mut input = tempfile::tempfile().unwrap();
        input.write_all(event.to_string().as_bytes()).unwrap();
        input.rewind().unwrap();
        let output = self
            .isolated(env!("CARGO_BIN_EXE_mmcg"))
            .args(["miner", "hooks", "receive", "--client", client])
            .stdin(Stdio::from(input))
            .output()
            .unwrap();
        assert_eq!(parse(output), json!({}));
    }

    fn config(&self, client: &str) -> PathBuf {
        self.project.join(if client == "codex" {
            ".codex/hooks.json"
        } else {
            ".claude/settings.local.json"
        })
    }
}

#[test]
fn session_end_does_not_erase_observed_native_activation() {
    let f = Fixture::new();
    f.setup("codex");
    f.session_start("codex", "ended");
    f.event(
        "codex",
        json!({"hook_event_name":"SessionEnd","session_id":"ended"}),
    );
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(
        status["readiness"]["activation"]["status"],
        "session_start_observed"
    );
    assert_eq!(status["readiness"]["activation"]["current_sessions"], 0);
    assert_eq!(
        status["readiness"]["activation"]["session_start_observations"],
        1
    );
}

#[test]
fn evidence_summary_separates_clients_generations_and_incomplete_episodes() {
    let f = Fixture::new();
    f.setup("codex");
    f.setup("claude");
    f.session_start("codex", "current");
    f.event("codex", json!({"hook_event_name":"UserPromptSubmit","session_id":"current","turn_id":"one","prompt":"Inspect the callers before changing the API."}));
    f.event(
        "codex",
        json!({"hook_event_name":"Stop","session_id":"current","turn_id":"one"}),
    );
    f.event("codex", json!({"hook_event_name":"UserPromptSubmit","session_id":"current","turn_id":"two","prompt":"Check the test results."}));
    f.event("codex", json!({"hook_event_name":"UserPromptSubmit","session_id":"missing-start","turn_id":"three","prompt":"Inspect the API."}));
    f.event(
        "codex",
        json!({"hook_event_name":"Stop","session_id":"missing-start","turn_id":"three"}),
    );
    f.session_start("claude", "other-client");
    f.event("claude", json!({"hook_event_name":"UserPromptSubmit","session_id":"other-client","turn_id":"four","prompt":"Inspect the API."}));
    let database = f.home.join(".mastermind/persona-events.db");
    let before = fs::read(&database).unwrap();
    let evidence = f.report("codex")["evidence"].clone();
    assert_eq!(evidence["status"], "available");
    assert_eq!(evidence["eligibility"], "capture_metadata_only");
    assert_eq!(evidence["current"]["events"], 6);
    assert_eq!(evidence["current"]["episodes"], 3);
    assert_eq!(evidence["current"]["complete_episodes"], 1);
    assert_eq!(evidence["current"]["incomplete_episodes"], 2);
    assert_eq!(
        evidence["current"]["coverage_gaps"]["missing_session_start"],
        1
    );
    assert_eq!(evidence["current"]["coverage_gaps"]["no_stop_observed"], 1);
    assert_eq!(evidence["historical"]["episodes"], 0);
    assert_eq!(fs::read(&database).unwrap(), before);
    assert!(!evidence.to_string().contains("Inspect the"));
    f.success(&["miner", "hooks", "recover", "--client", "codex"]);
    let recovered = f.report("codex")["evidence"].clone();
    assert_eq!(recovered["current"]["episodes"], 0);
    assert_eq!(recovered["current"]["events"], 0);
    assert_eq!(recovered["historical"]["episodes"], 3);
    assert_eq!(recovered["historical"]["events"], 6);
    assert!(!f.home.join(".mastermind/style.db").exists());
}

#[test]
fn legacy_capture_gaps_are_retained_without_reinterpreting_old_sources() {
    let f = Fixture::new();
    f.setup("codex");
    f.session_start("codex", "legacy");
    f.event("codex", json!({"hook_event_name":"UserPromptSubmit","session_id":"legacy","turn_id":"one","prompt":"Inspect the API."}));
    f.event(
        "codex",
        json!({"hook_event_name":"Stop","session_id":"legacy","turn_id":"one"}),
    );
    let path = f.home.join(".mastermind/persona-events.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("UPDATE hook_session SET data=json_set(data,'$.capture_version',2,'$.gaps',json('[\"oversized_event_content\"]'))", []).unwrap();
    drop(conn);
    let before = fs::read(&path).unwrap();
    let report = f.report("codex");
    assert_eq!(report["evidence"]["current"]["complete_episodes"], 0);
    assert_eq!(
        report["evidence"]["current"]["coverage_gaps"]["legacy_capture_semantics"],
        1
    );
    assert_eq!(
        report["evidence"]["current"]["coverage_gaps"]["oversized_event_content"],
        1
    );
    assert_eq!(report["activation"]["status"], "not_observed");
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn capture_mode_reports_disabled_analysis_and_actionable_activation() {
    let f = Fixture::new();
    let mut settings = mmcg::onboarding::Settings::local(&f.project);
    settings.clients = vec!["codex".into()];
    settings.mining = mmcg::onboarding::Mining::Capture;
    mmcg::onboarding::Session::begin(&f.project)
        .unwrap()
        .save(&settings)
        .unwrap();
    f.setup("codex");
    let report = f.report("codex");
    assert_eq!(report["pipeline"]["requested_mode"], "capture");
    assert_eq!(report["pipeline"]["analysis_requested"], false);
    assert!(report["pipeline"]["next_actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action.as_str().unwrap().contains("/hooks")));
    assert_eq!(report["evidence"]["current"]["events"], 0);
    assert!(!f.home.join(".mastermind/persona-workers").exists());
    assert!(!f.home.join(".mastermind/style.db").exists());
    settings.mining = mmcg::onboarding::Mining::On;
    settings.provider = Some("claude".into());
    mmcg::onboarding::Session::begin(&f.project)
        .unwrap()
        .save(&settings)
        .unwrap();
    let requested = f.report("codex");
    assert_eq!(requested["pipeline"]["analysis_requested"], true);
    assert!(requested["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning == "managed_miner_not_running"));
    assert!(!f.home.join(".mastermind/persona-workers").exists());
    let path = f.project.join(".mastermind/setup.json");
    fs::write(&path, b"{invalid setup settings").unwrap();
    let unavailable = f.report("codex");
    assert!(unavailable["pipeline"]["requested_mode"].is_null());
    assert!(unavailable["pipeline"]["analysis_requested"].is_null());
    assert!(unavailable["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning == "setup_settings_unavailable"));
    assert_eq!(fs::read(&path).unwrap(), b"{invalid setup settings");
}

fn parse(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}: {output:?}"))
}

#[test]
fn absent_optional_hooks_are_not_failures_and_status_does_not_install_anything() {
    let f = Fixture::new();
    for client in ["codex", "claude"] {
        let report = f.report(client);
        assert_eq!(report["configured"], false);
        assert_eq!(report["inspection"], "read_only");
        assert_eq!(report["capture"]["status"], "not_configured");
        assert_eq!(report["native_registration"]["status"], "missing_or_stale");
        assert_eq!(report["mining"]["status"], "not_configured");
        assert_eq!(f.doctor(client)["status"], "ok");
        assert!(!f.config(client).exists());
    }
    assert_eq!(fs::read_dir(&f.home).unwrap().count(), 0);
}

#[test]
fn unavailable_capture_journal_preserves_other_readiness_observations() {
    let f = Fixture::new();
    f.setup("claude");
    let config = fs::read(f.config("claude")).unwrap();
    let database = f.home.join(".mastermind/persona-events.db");
    fs::write(&database, b"invalid journal fixture").unwrap();
    let status = f.success(&["miner", "hooks", "status", "--client", "claude"]);
    assert_eq!(status["status"], "unavailable");
    assert_eq!(status["readiness"]["capture"]["status"], "unavailable");
    assert_eq!(
        status["readiness"]["pipeline"]["local_analysis"]["status"],
        "unavailable"
    );
    assert_eq!(
        status["readiness"]["native_registration"]["status"],
        "current"
    );
    assert_eq!(status["readiness"]["mining"]["status"], "not_configured");
    assert_eq!(f.doctor("claude")["status"], "warn");
    assert_eq!(fs::read(f.config("claude")).unwrap(), config);
    assert_eq!(fs::read(database).unwrap(), b"invalid journal fixture");
}

#[test]
fn installed_capture_and_observed_current_sessions_are_separate_boundaries() {
    let f = Fixture::new();
    let setup = f.setup("codex");
    let installed = &setup["readiness"];
    assert_eq!(installed["native_registration"]["status"], "current");
    assert_eq!(installed["capture"]["status"], "enabled");
    assert_eq!(installed["activation"]["status"], "not_observed");
    assert_eq!(installed["refiner"]["status"], "not_configured");
    assert_eq!(installed["mining"]["status"], "not_configured");
    assert_eq!(f.doctor("codex")["status"], "warn");
    f.session_start("codex", "synthetic-private-session-id");
    let observed = f.report("codex");
    assert_eq!(observed["activation"]["status"], "session_start_observed");
    assert_eq!(observed["activation"]["current_sessions"], 1);
    assert_eq!(
        observed["activation"]["client_trust"],
        "not_independently_verified"
    );
    assert!(!observed
        .to_string()
        .contains("synthetic-private-session-id"));
    assert_eq!(f.doctor("codex")["status"], "ok");
    let before = observed["capture"]["generation"].as_i64().unwrap();
    f.success(&["miner", "hooks", "recover", "--client", "codex"]);
    let recovered = f.report("codex");
    assert!(recovered["capture"]["generation"].as_i64().unwrap() > before);
    assert_eq!(recovered["activation"]["status"], "not_observed");
    assert_eq!(recovered["activation"]["current_sessions"], 0);
    assert!(!f.home.join(".mastermind/persona-workers").exists());
    assert!(!f.home.join(".mastermind/style.db").exists());
}

#[test]
fn native_drift_disabled_hooks_and_malformed_config_do_not_hide_the_capture_grant() {
    let f = Fixture::new();
    f.setup("claude");
    f.session_start("claude", "local-observation");
    let path = f.config("claude");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["disableAllHooks"] = json!(true);
    fs::write(&path, config.to_string()).unwrap();
    let disabled = f.report("claude");
    assert_eq!(disabled["native_registration"]["status"], "current");
    assert_eq!(
        disabled["native_registration"]["local_hooks_disabled"],
        true
    );
    assert_eq!(f.doctor("claude")["status"], "warn");
    config["hooks"]["UserPromptSubmit"][0]["hooks"][0]["timeout"] = json!(99);
    let bytes = config.to_string();
    fs::write(&path, &bytes).unwrap();
    let drift = f.report("claude");
    assert_eq!(drift["native_registration"]["status"], "missing_or_stale");
    assert_eq!(drift["capture"]["status"], "enabled");
    assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
    fs::write(&path, b"{malformed private native config").unwrap();
    let malformed = f.report("claude");
    assert_eq!(malformed["native_registration"]["status"], "unavailable");
    assert_eq!(malformed["capture"]["status"], "enabled");
    assert_eq!(malformed["mining"]["status"], "not_configured");
    assert!(!malformed
        .to_string()
        .contains("malformed private native config"));
    assert_eq!(f.doctor("claude")["status"], "warn");
    assert_eq!(
        fs::read(&path).unwrap(),
        b"{malformed private native config"
    );
}

#[test]
fn configured_refiner_is_not_a_provider_test_or_a_background_miner() {
    let f = Fixture::new();
    let processor = f._temp.path().join("must-not-execute");
    let marker = f._temp.path().join("unexpected-provider-call");
    fs::write(
        &processor,
        format!("#!/bin/sh\ntouch '{}'\nexit 9\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&processor, fs::Permissions::from_mode(0o700)).unwrap();
    let setup = f.success(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "codex",
        "--write",
        "--refiner-processor",
        processor.to_str().unwrap(),
        "--refiner-timeout",
        "7",
    ]);
    assert_eq!(setup["readiness"]["refiner"]["status"], "configured");
    assert_eq!(
        setup["readiness"]["native_registration"]["status"],
        "current"
    );
    let report = f.report("codex");
    assert_eq!(report["refiner"]["execution"], "not_tested");
    assert_eq!(report["refiner"]["timeout_seconds"], 7);
    assert_eq!(report["mining"]["status"], "not_configured");
    assert_eq!(report["mining"]["autostart"], false);
    f.doctor("codex");
    assert!(!marker.exists());
    assert!(!f.home.join(".mastermind/persona-workers").exists());
    fs::remove_file(processor).unwrap();
    assert_eq!(
        f.report("codex")["refiner"]["status"],
        "configuration_invalid"
    );
    assert_eq!(f.doctor("codex")["status"], "warn");
    assert!(!marker.exists());
}
