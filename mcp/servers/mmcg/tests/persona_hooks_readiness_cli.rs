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
