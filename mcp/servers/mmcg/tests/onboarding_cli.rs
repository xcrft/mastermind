//! Public onboarding in disposable projects, homes and native client fixtures.
//! The fake client only implements MCP management. No model is contacted.
#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const CLIENT: &str = r#"#!/bin/sh
if [ "$1" != mcp ]; then printf 'unexpected provider call\n' >> "$HOME/provider-calls"; exit 91; fi
if [ "$FIXTURE_FORBID_MCP" = 1 ]; then printf 'unexpected native call\n' >> "$HOME/native-calls"; exit 92; fi
state=removed
if [ -f "$0.state" ]; then read -r state < "$0.state" || :; fi
case "$2" in
  get)
    [ "$state" != removed ] || exit 1
    printf '  Scope: User config\n  Type: stdio\n  Command: %s\n  Args: serve\n  Environment:\n    MMCG_PROFILE_CLIENT=%s\n' "$FIXTURE_MMCG" "$state"
    ;;
  list)
    if [ "$state" = removed ]; then printf '[]\n'; exit 0; fi
    printf '[{"name":"mmcg","enabled":true,"transport":{"type":"stdio","command":"%s","args":["serve"],"env":{"MMCG_PROFILE_CLIENT":"%s"},"env_vars":[],"cwd":null}}]\n' "$FIXTURE_MMCG" "$state"
    ;;
  remove) printf 'removed\n' > "$0.state";;
  add)
    for arg in "$@"; do
      [ "$arg" != -- ] || break
      case "$arg" in MMCG_PROFILE_CLIENT=*) audience=${arg#MMCG_PROFILE_CLIENT=}; printf '%s\n' "$audience" > "$0.state";; esac
    done
    if [ "$audience" = claude ]; then
      printf '{"mcpServers":{"mmcg":{"command":"%s","args":["serve"],"env":{"MMCG_PROFILE_CLIENT":"claude"}}}}\n' "$FIXTURE_MMCG" > "$HOME/.claude.json"
    else
      mkdir -p "$HOME/.codex"
      printf '[mcp_servers.mmcg]\ncommand = "%s"\nargs = ["serve"]\n[mcp_servers.mmcg.env]\nMMCG_PROFILE_CLIENT = "codex"\n' "$FIXTURE_MMCG" > "$HOME/.codex/config.toml"
    fi
    ;;
  *) exit 2;;
esac
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        for name in ["project", "home", "bin"] {
            fs::create_dir(temp.path().join(name)).unwrap();
        }
        let fixture = Self {
            root: temp.path().join("project").canonicalize().unwrap(),
            home: temp.path().join("home").canonicalize().unwrap(),
            bin: temp.path().join("bin").canonicalize().unwrap(),
            _temp: temp,
        };
        fs::write(
            fixture.root.join("module.py"),
            "def welcome(name):\n    return name\n",
        )
        .unwrap();
        fs::write(
            fixture.root.join("CONTEXT.md"),
            "# Context\n\nDecision: keep the user's project context.\n",
        )
        .unwrap();
        let output = fixture
            .command("/usr/bin/git")
            .args(["init", "-q", "--initial-branch=main"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        fixture
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("FIXTURE_MMCG", env!("CARGO_BIN_EXE_mmcg"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_mmcg"))
            .args(args)
            .output()
            .unwrap()
    }

    fn success(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}: {output:?}"))
    }

    fn client(&self, name: &str) {
        let path = self.bin.join(name);
        fs::write(&path, CLIENT).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn no_provider_calls(&self) {
        assert!(!self.home.join("provider-calls").exists());
    }
}

#[test]
fn dry_run_and_unconfigured_status_leave_project_and_home_unchanged() {
    let f = Fixture::new();
    f.client("claude");
    let planned = f.success(&[
        "init",
        "--client",
        "claude",
        "--mining",
        "on",
        "--provider",
        "claude",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(planned["status"], "planned");
    assert_eq!(planned["provider_calls"], false);
    let status = f.success(&["status", "--json"]);
    assert!(status["settings"].is_null());
    assert!(!f.root.join(".mastermind").exists());
    assert_eq!(fs::read_dir(&f.home).unwrap().count(), 0);
    f.no_provider_calls();
}

#[test]
fn local_init_indexes_source_and_context_and_preserves_existing_documents() {
    let f = Fixture::new();
    let context = fs::read(f.root.join("CONTEXT.md")).unwrap();
    fs::write(f.root.join("CLAUDE.md"), "user-owned instructions\n").unwrap();
    let first = f.success(&["init", "--json"]);
    assert_eq!(first["status"], "configured");
    assert_eq!(first["settings"]["mining"], "off");
    assert!(first["settings"]["clients"].as_array().unwrap().is_empty());
    assert!(
        first["observed"]["project"]["index"]["symbol_count"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(
        first["observed"]["project"]["index"]["history_freshness"],
        "fresh"
    );
    let settings = fs::read(f.root.join(".mastermind/setup.json")).unwrap();
    f.success(&["init", "--json"]);
    assert_eq!(
        fs::read(f.root.join(".mastermind/setup.json")).unwrap(),
        settings
    );
    assert_eq!(fs::read(f.root.join("CONTEXT.md")).unwrap(), context);
    assert_eq!(
        fs::read_to_string(f.root.join("CLAUDE.md")).unwrap(),
        "user-owned instructions\n"
    );
    for args in [vec!["init"], vec!["status"]] {
        let output = f.run(&args);
        assert!(output.status.success(), "{output:?}");
        assert!(
            !String::from_utf8(output.stdout)
                .unwrap()
                .contains("Restart"),
            "local setup and read-only status must not request a client restart"
        );
    }
    assert_eq!(fs::read_dir(&f.home).unwrap().count(), 0);
}

#[test]
fn explicit_capture_registers_audience_without_claiming_activation_or_profile_access() {
    let f = Fixture::new();
    f.client("codex");
    let first = f.success(&[
        "init",
        "--client",
        "codex",
        "--mining",
        "capture",
        "--profile-access",
        "off",
        "--no-global",
        "--json",
    ]);
    let client = &first["observed"]["clients"][0];
    assert_eq!(client["mcp"]["status"], "configured");
    assert_eq!(client["hooks"]["capture"]["status"], "enabled");
    assert_eq!(client["hooks"]["activation"]["status"], "not_observed");
    assert_eq!(client["profile_access"]["allowed"], false);
    assert!(!f.root.join("CLAUDE.md").exists());
    let hooks = fs::read(f.root.join(".codex/hooks.json")).unwrap();
    let second = f.success(&["init", "--json"]);
    assert_eq!(first["settings"], second["settings"]);
    assert_eq!(
        first["observed"]["clients"][0]["hooks"]["capture"]["generation"],
        second["observed"]["clients"][0]["hooks"]["capture"]["generation"]
    );
    assert_eq!(fs::read(f.root.join(".codex/hooks.json")).unwrap(), hooks);
    for (args, restart) in [(vec!["init"], true), (vec!["status"], false)] {
        let output = f.run(&args);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("Restart"),
            restart,
            "{args:?}"
        );
    }
    f.success(&["init", "--refiner", "on", "--provider", "claude", "--json"]);
    let disabled = f.success(&["init", "--mining", "off", "--json"]);
    assert_eq!(disabled["settings"]["refiner"], false);
    assert_eq!(
        disabled["observed"]["clients"][0]["hooks"]["capture"]["enabled"],
        false
    );
    f.no_provider_calls();
}

#[test]
fn first_init_uses_the_active_client_and_arms_personalization_without_inference() {
    let f = Fixture::new();
    f.client("claude");
    f.client("codex");
    let output = f
        .command(env!("CARGO_BIN_EXE_mmcg"))
        .env("CODEX_THREAD_ID", "native-thread")
        .args(["init", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let first: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(first["settings"]["clients"], serde_json::json!(["codex"]));
    assert_eq!(first["settings"]["mining"], "task");
    assert!(first["settings"]["provider"].is_null());
    assert_eq!(first["settings"]["profile_access"], true);
    assert_eq!(
        first["observed"]["clients"][0]["profile_access"]["allowed"],
        true
    );
    assert_eq!(
        first["observed"]["clients"][0]["hooks"]["activation"]["status"],
        "not_observed"
    );
    let settings = fs::read(f.root.join(".mastermind/setup.json")).unwrap();
    f.success(&["init", "--json"]);
    assert_eq!(
        fs::read(f.root.join(".mastermind/setup.json")).unwrap(),
        settings
    );
    f.no_provider_calls();
}

#[test]
fn first_init_detects_an_installed_client_and_preserves_explicit_opt_outs() {
    let f = Fixture::new();
    f.client("codex");
    let first = f.success(&[
        "init",
        "--mining",
        "capture",
        "--profile-access",
        "off",
        "--json",
    ]);
    assert_eq!(first["settings"]["clients"], serde_json::json!(["codex"]));
    assert_eq!(first["settings"]["mining"], "capture");
    assert_eq!(first["settings"]["profile_access"], false);
    assert_eq!(
        first["settings"],
        f.success(&["init", "--json"])["settings"]
    );
    f.no_provider_calls();
}

#[test]
fn partial_setup_retries_saved_client_selection_after_client_installation() {
    let f = Fixture::new();
    let output = f.run(&["init", "--client", "claude", "--no-global", "--json"]);
    assert!(!output.status.success());
    let failed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(failed["status"], "incomplete");
    assert!(f.root.join(".mastermind/setup.json").exists());
    f.client("claude");
    let retried = f.success(&["init", "--json"]);
    assert_eq!(retried["settings"]["clients"][0], "claude");
    assert_eq!(
        retried["observed"]["clients"][0]["mcp"]["status"],
        "configured"
    );
    f.no_provider_calls();
}

#[test]
fn repeated_init_preserves_a_stopped_worker_budget_and_explicit_start_renews_it() {
    let f = Fixture::new();
    f.client("claude");
    let first = f.success(&[
        "init",
        "--client",
        "claude",
        "--mining",
        "on",
        "--provider",
        "claude",
        "--max-calls",
        "2",
        "--max-runtime",
        "60",
        "--no-global",
        "--json",
    ]);
    let initial = &first["observed"]["clients"][0]["hooks"]["mining"]["run"];
    assert!(initial["run_id"].is_string());
    f.success(&["miner", "stop", "--json"]);
    let repeated = f.success(&["init", "--json"]);
    let after = &repeated["observed"]["clients"][0]["hooks"]["mining"]["run"];
    assert_eq!(initial["run_id"], after["run_id"]);
    assert_eq!(initial["attempts"], after["attempts"]);
    let restart = f.success(&["miner", "start", "--json"]);
    assert_ne!(
        restart["steps"][0]["detail"]["run"]["run_id"],
        initial["run_id"]
    );
    f.success(&["miner", "stop", "--json"]);
    let capture = f.success(&["init", "--mining", "capture", "--json"]);
    let hooks = &capture["observed"]["clients"][0]["hooks"];
    assert_eq!(hooks["pipeline"]["requested_mode"], "capture");
    assert_eq!(hooks["pipeline"]["analysis_requested"], false);
    assert_eq!(
        hooks["mining"]["run"]["run_id"],
        restart["steps"][0]["detail"]["run"]["run_id"]
    );
    assert!(!hooks["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning == "managed_miner_not_running"));
    let status = f.run(&["status"]);
    assert!(status.status.success(), "{status:?}");
    let text = String::from_utf8(status.stdout).unwrap();
    assert!(text.contains("semantic analysis not requested"), "{text}");
    assert!(text.contains("events"), "{text}");
    f.no_provider_calls();
}

#[test]
fn failed_client_removal_stays_pending_and_reader_access_is_revoked_independently() {
    let f = Fixture::new();
    f.client("claude");
    f.success(&[
        "init",
        "--client",
        "claude",
        "--profile-access",
        "on",
        "--no-global",
        "--json",
    ]);
    let hooks = f.root.join(".claude/settings.local.json");
    let original = fs::read(&hooks).unwrap();
    fs::write(&hooks, "invalid JSON").unwrap();
    let removed = f.run(&["init", "--client", "none", "--json"]);
    assert!(!removed.status.success());
    let report: Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(report["settings"]["pending_removals"][0], "claude");
    assert_eq!(
        report["observed"]["clients"][0]["profile_access"]["allowed"],
        false
    );
    fs::write(hooks, original).unwrap();
    let retried = f.success(&["init", "--json"]);
    assert!(retried["settings"]["pending_removals"]
        .as_array()
        .unwrap()
        .is_empty());
    let readiness = f.success(&["miner", "hooks", "status", "--client", "claude"]);
    assert_eq!(readiness["readiness"]["capture"]["enabled"], false);
    f.no_provider_calls();
}

#[test]
fn status_reads_registrations_without_executing_a_native_client_or_server() {
    let f = Fixture::new();
    f.client("claude");
    f.success(&["init", "--client", "claude", "--no-global", "--json"]);
    let config = fs::read(f.home.join(".claude.json")).unwrap();
    let output = f
        .command(env!("CARGO_BIN_EXE_mmcg"))
        .env("FIXTURE_FORBID_MCP", "1")
        .args(["status", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["clients"][0]["mcp"]["status"], "configured");
    assert!(!f.home.join("native-calls").exists());
    assert_eq!(fs::read(f.home.join(".claude.json")).unwrap(), config);
    f.no_provider_calls();
}

#[test]
fn failed_file_commits_do_not_report_a_successful_index_build() {
    let f = Fixture::new();
    f.success(&["init", "--json"]);
    let connection = rusqlite::Connection::open(f.root.join(".mastermind/mmcg.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER fixture_block_symbols BEFORE INSERT ON symbols BEGIN SELECT RAISE(FAIL, 'fixture persistence failure'); END").unwrap();
    fs::write(
        f.root.join("module.py"),
        "def changed_source(value):\n    return value + 1\n",
    )
    .unwrap();
    let output = f.run(&["init", "--json"]);
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "incomplete");
    assert_eq!(report["steps"][0]["status"], "failed");
    assert!(
        report["steps"][0]["detail"]["index_stats"]["failed"]
            .as_u64()
            .unwrap()
            > 0
    );
    connection
        .execute_batch("DROP TRIGGER fixture_block_symbols")
        .unwrap();
    f.success(&["init", "--json"]);
}
