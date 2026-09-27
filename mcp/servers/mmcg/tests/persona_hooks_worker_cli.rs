//! Managed worker lifecycle through the public CLI. All homes, projects,
//! processors and captured inputs are synthetic. No model is contacted.
#![cfg(unix)]

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::fs;
use std::io::{Seek, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const QUOTE: &str =
    "Before changing an API, inspect its callers and preserve the existing contract.";
const LITERAL_ARG: &str = "--fixture=keep $HOME 'quotes' `literal`";
const PROCESSOR: &str = r#"import json
import os
from pathlib import Path
import sys
import time

harness = Path(sys.argv[1])
assert sys.argv[2] == "--fixture=keep $HOME 'quotes' `literal`"
assert os.environ.get("MASTERMIND_MINER") == "1", "missing recursion guard"
request = json.load(sys.stdin)
assert request["schema"] == 1
episode = request["episode"]
call = {"episode": episode["id"], "revision": episode["revision"],
        "client": episode["client"], "pid": os.getpid(),
        "guard": os.environ["MASTERMIND_MINER"]}
name = str(time.time_ns()) + "-" + str(os.getpid())
temporary = harness / (name + ".tmp")
temporary.write_text(json.dumps(call), encoding="utf-8")
temporary.replace(harness / "calls" / (name + ".json"))
temporary = harness / (name + ".request.tmp")
temporary.write_text(json.dumps(request), encoding="utf-8")
temporary.replace(harness / "request.json")
mode = (harness / "mode").read_text(encoding="utf-8")
if mode == "blocked":
    deadline = time.monotonic() + 35
    while not (harness / "release").exists():
        if time.monotonic() >= deadline:
            sys.exit(17)
        time.sleep(0.02)
if mode == "failed":
    print("SYNTHETIC_FAILURE_DO_NOT_PERSIST", file=sys.stderr)
    sys.exit(9)
print(json.dumps({"schema": 1, "episode_id": episode["id"],
                  "episode_revision": episode["revision"], "drafts": []}))
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    harness: PathBuf,
    processor: PathBuf,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        let harness = temp.path().join("processor-fixture");
        for path in [&home, &project, &harness] {
            fs::create_dir(path).unwrap();
        }
        fs::create_dir(harness.join("calls")).unwrap();
        // Unix CI already requires Python. Bind its resolved absolute path so
        // the synthetic processor neither searches PATH nor loads user sites.
        let python = Command::new("python3")
            .args(["-I", "-c", "import sys; print(sys.executable)"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &home)
            .output()
            .expect("the Unix worker fixture requires Python 3");
        assert!(python.status.success(), "{python:?}");
        let python = PathBuf::from(String::from_utf8(python.stdout).unwrap().trim())
            .canonicalize()
            .unwrap();
        let processor = harness.join("semantic-processor");
        fs::write(
            &processor,
            format!("#!{} -I\n{PROCESSOR}", python.display()),
        )
        .unwrap();
        fs::set_permissions(&processor, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(harness.join("mode"), mode).unwrap();
        let f = Self {
            home: home.canonicalize().unwrap(),
            project: project.canonicalize().unwrap(),
            harness: harness.canonicalize().unwrap(),
            processor,
            _temp: temp,
        };
        let output = f
            .isolated("/usr/bin/git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
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

    fn cli(&self) -> Command {
        self.isolated(env!("CARGO_BIN_EXE_mmcg"))
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cli().args(args).output().unwrap()
    }

    fn success(&self, args: &[&str]) -> Value {
        parse(self.run(args))
    }

    fn setup(&self, client: &str) {
        self.success(&["miner", "hooks", "setup", "--client", client, "--write"]);
    }

    fn event(&self, client: &str, session: &str, turn: &str, kind: &str, extras: Value) {
        let mut event = json!({"session_id":session,"event_id":format!("{session}:{turn}:{kind}"),
            "hook_event_name":kind,"cwd":self.project});
        if !turn.is_empty() {
            event["turn_id"] = json!(turn);
        }
        for (key, value) in extras.as_object().unwrap() {
            event[key] = value.clone();
        }
        let mut input = tempfile::tempfile().unwrap();
        input.write_all(event.to_string().as_bytes()).unwrap();
        input.rewind().unwrap();
        let output = self
            .cli()
            .args(["miner", "hooks", "receive", "--client", client])
            .stdin(Stdio::from(input))
            .output()
            .unwrap();
        assert_eq!(
            parse(output),
            json!({}),
            "capture alone provides no context"
        );
    }

    fn capture(&self, client: &str, session: &str, turn: &str) {
        self.event(
            client,
            session,
            "",
            "SessionStart",
            json!({"source":"startup"}),
        );
        self.event(
            client,
            session,
            turn,
            "UserPromptSubmit",
            json!({"prompt":QUOTE}),
        );
        self.event(client, session, turn, "Stop", json!({"last_assistant_message":"I inspected the callers and kept the API contract unchanged."}));
    }

    fn start_command(&self, client: &str) -> Command {
        let mut command = self.cli();
        command
            .args([
                "miner",
                "hooks",
                "worker",
                "start",
                "--client",
                client,
                "--processor",
            ])
            .arg(&self.processor)
            .arg(format!("--processor-arg={}", self.harness.display()))
            .arg(format!("--processor-arg={LITERAL_ARG}"));
        command
    }

    fn start(&self, client: &str, extra: &[&str]) -> Value {
        parse(self.start_command(client).args(extra).output().unwrap())
    }

    fn restart(&self, client: &str) -> Value {
        self.success(&["miner", "hooks", "worker", "start", "--client", client])
    }

    fn status(&self, client: &str) -> Value {
        self.success(&["miner", "hooks", "worker", "status", "--client", client])
    }

    fn stop(&self, client: &str) -> Value {
        self.success(&["miner", "hooks", "worker", "stop", "--client", client])
    }

    fn until(&self, client: &str, description: &str, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let status = self.status(client);
            if predicate(&status) {
                return status;
            }
            assert!(Instant::now() < deadline, "{description}: {status}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn terminal(&self, client: &str) -> Value {
        self.until(client, "worker did not release its owner", |status| {
            status["owner"] == "available"
                && matches!(
                    status["status"].as_str(),
                    Some("stopped" | "failed" | "budget_exhausted")
                )
        })
    }

    fn calls(&self) -> Vec<Value> {
        let mut paths: Vec<_> = fs::read_dir(self.harness.join("calls"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        paths.sort();
        paths
            .iter()
            .map(|path| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
            .collect()
    }

    fn wait_calls(&self, count: usize) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let calls = self.calls();
            if calls.len() >= count {
                return calls;
            }
            assert!(
                Instant::now() < deadline,
                "processor call {count} was not observed"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn mode(&self, mode: &str) {
        fs::write(self.harness.join("mode"), mode).unwrap();
    }

    fn release(&self) {
        fs::write(self.harness.join("release"), b"").unwrap();
    }

    fn checkpoint(&self) -> (i64, i64, i64) {
        let db = Connection::open_with_flags(
            self.home.join(".mastermind/persona-events.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        db.busy_timeout(Duration::from_secs(1)).unwrap();
        db.query_row("SELECT COUNT(*),COALESCE(SUM(completed),0),COALESCE(MAX(CASE WHEN completed=0 THEN lease_until ELSE 0 END),0) FROM hook_analysis", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        }).unwrap()
    }

    fn directory(&self, client: &str) -> PathBuf {
        self.home
            .join(".mastermind/persona-workers")
            .join(self.status(client)["worker_id"].as_str().unwrap())
    }

    fn assert_drafts_only(&self) {
        assert!(!self.home.join(".mastermind/style.db").exists());
        assert!(!self.project.join(".mastermind/tasks").exists());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Stop through the production cancellation path before the temporary
        // home disappears. No PID from an external/user state is signalled.
        for client in ["codex", "claude"] {
            let _ = self.run(&["miner", "hooks", "worker", "stop", "--client", client]);
        }
        self.release();
    }
}

fn parse(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}: {output:?}"))
}

#[test]
fn status_is_read_only_and_capture_setup_does_not_start_a_worker() {
    let f = Fixture::new("ordinary");
    assert_eq!(f.status("codex")["status"], "not_configured");
    assert_eq!(fs::read_dir(&f.home).unwrap().count(), 0);
    f.setup("codex");
    f.capture("codex", "not-started", "one");
    assert_eq!(f.status("codex")["status"], "not_configured");
    assert!(!f.home.join(".mastermind/persona-workers").exists());
    assert!(f.calls().is_empty());
    for (flag, value) in [
        ("--max-calls", "0"),
        ("--max-runtime", "0"),
        ("--limit", "17"),
        ("--timeout", "121"),
    ] {
        let output = f
            .start_command("codex")
            .args([flag, value])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{flag}={value}: {output:?}");
    }
    assert!(f.calls().is_empty());
    f.assert_drafts_only();
}

#[test]
fn restart_preserves_settings_and_completed_checkpoints_but_observes_revised_episodes() {
    let f = Fixture::new("ordinary");
    f.setup("codex");
    f.capture("codex", "revised", "one");
    let first = f.start(
        "codex",
        &[
            "--max-calls",
            "12",
            "--max-runtime",
            "30",
            "--timeout",
            "8",
            "--limit",
            "1",
        ],
    );
    f.until("codex", "first empty result was not checkpointed", |s| {
        s["run"]["completed"] == 1
    });
    let duplicate = f.restart("codex");
    assert_eq!(duplicate["started"], false);
    assert_eq!(duplicate["run"]["run_id"], first["run"]["run_id"]);
    assert_eq!(duplicate["run"]["attempts"], 1);
    let changed = f
        .start_command("codex")
        .args(["--max-calls", "11"])
        .output()
        .unwrap();
    assert!(
        !changed.status.success(),
        "running settings changed: {changed:?}"
    );
    assert_eq!(f.stop("codex")["status"], "stopped");
    let restarted = f.restart("codex");
    assert_ne!(restarted["run"]["run_id"], first["run"]["run_id"]);
    assert_eq!(restarted["configuration"]["max_calls"], 12);
    assert_eq!(restarted["configuration"]["timeout"], 8);
    assert_eq!(restarted["configuration"]["limit"], 1);
    f.event("codex", "revised", "two", "UserPromptSubmit", json!({"prompt":"This applies to public APIs. Internal prototypes may use a different approach."}));
    f.until(
        "codex",
        "earlier episode revision was not reprocessed",
        |s| s["run"]["completed"] == 1,
    );
    f.stop("codex");
    let calls = f.calls();
    assert_eq!(calls.len(), 2, "completed revisions must not be repeated");
    assert_eq!(calls[0]["episode"], calls[1]["episode"]);
    assert_ne!(calls[0]["revision"], calls[1]["revision"]);
    assert!(calls.iter().all(|call| call["guard"] == "1"));
    assert_eq!(f.checkpoint().1, 2);
    f.assert_drafts_only();
}

#[test]
fn independent_client_slots_charge_only_their_own_eligible_episodes() {
    let f = Fixture::new("ordinary");
    for client in ["codex", "claude"] {
        f.setup(client);
        f.capture(client, client, "one");
    }
    let codex = f.start("codex", &["--max-calls", "1", "--max-runtime", "20"]);
    let claude = f.start("claude", &["--max-calls", "1", "--max-runtime", "20"]);
    assert_ne!(codex["worker_id"], claude["worker_id"]);
    for client in ["codex", "claude"] {
        let result = f.terminal(client);
        assert_eq!(result["status"], "budget_exhausted");
        assert_eq!(result["run"]["attempts"], 1);
        assert_eq!(result["run"]["completed"], 1);
    }
    let calls = f.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["client"] == "codex")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["client"] == "claude")
            .count(),
        1
    );
    assert_eq!(f.checkpoint().1, 2);
    f.assert_drafts_only();
}

#[test]
fn concurrent_start_has_one_owner_and_stop_cancels_the_processor_and_releases_its_lease() {
    let f = Fixture::new("blocked");
    f.setup("codex");
    f.capture("codex", "concurrent", "one");
    let spawn = || {
        f.start_command("codex")
            .args(["--max-calls", "4", "--max-runtime", "30", "--timeout", "20"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = spawn();
    let second = spawn();
    let outputs = [
        first.wait_with_output().unwrap(),
        second.wait_with_output().unwrap(),
    ];
    assert!(
        outputs.iter().any(|output| output.status.success()),
        "{outputs:?}"
    );
    for output in outputs {
        if output.status.success() {
            parse(output);
        } else {
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("worker_lifecycle_busy"),
                "{output:?}"
            );
        }
    }
    let calls = f.wait_calls(1);
    assert_eq!(calls.len(), 1);
    let pid = calls[0]["pid"].as_i64().unwrap() as libc::pid_t;
    let result = f.stop("codex");
    assert_eq!(result["status"], "stopped");
    assert_eq!(result["owner"], "available");
    assert_eq!(f.checkpoint(), (1, 0, 0));
    let deadline = Instant::now() + Duration::from_secs(3);
    // SAFETY: signal zero checks the synthetic processor's owned group only.
    while unsafe { libc::kill(-pid, 0) } == 0 {
        assert!(Instant::now() < deadline, "processor group survived stop");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(f.stop("codex")["run"], result["run"]);
    assert_eq!(f.calls().len(), 1);
}

#[test]
fn failed_and_timed_out_attempts_stop_without_retry_until_explicit_restart() {
    for (mode, timeout) in [("failed", "8"), ("blocked", "1")] {
        let f = Fixture::new(mode);
        f.setup("codex");
        f.capture("codex", mode, "one");
        f.start(
            "codex",
            &[
                "--max-calls",
                "4",
                "--max-runtime",
                "20",
                "--timeout",
                timeout,
            ],
        );
        let failed = f.terminal("codex");
        assert_eq!(failed["status"], "failed", "{failed}");
        assert_eq!(failed["run"]["attempts"], 1);
        assert_eq!(f.calls().len(), 1);
        assert_eq!(f.checkpoint(), (1, 0, 0));
        assert!(!fs::read_to_string(f.directory("codex").join("state.json"))
            .unwrap()
            .contains("SYNTHETIC_FAILURE"));
        f.mode("ordinary");
        let next = f.restart("codex");
        assert_ne!(next["run"]["run_id"], failed["run"]["run_id"]);
        f.until("codex", "failed lease was not retryable", |s| {
            s["run"]["completed"] == 1
        });
        f.stop("codex");
        assert_eq!(f.calls().len(), 2);
        assert_eq!(f.checkpoint().1, 1);
    }
    let f = Fixture::new("ordinary");
    f.setup("codex");
    f.start("codex", &["--max-calls", "4", "--max-runtime", "1"]);
    let exhausted = f.terminal("codex");
    assert_eq!(exhausted["status"], "budget_exhausted");
    assert_eq!(
        exhausted["run"]["reason"],
        "worker_runtime_budget_exhausted"
    );
    assert!(f.calls().is_empty());
}

#[test]
fn capture_revocation_and_processor_drift_withhold_inflight_results() {
    for mutation in ["revoke", "executable"] {
        let f = Fixture::new("blocked");
        f.setup("codex");
        f.capture("codex", mutation, "one");
        f.start(
            "codex",
            &["--max-calls", "4", "--max-runtime", "30", "--timeout", "20"],
        );
        f.wait_calls(1);
        if mutation == "revoke" {
            f.success(&[
                "miner", "hooks", "setup", "--client", "codex", "--write", "--remove",
            ]);
        } else {
            fs::OpenOptions::new()
                .append(true)
                .open(&f.processor)
                .unwrap()
                .write_all(b"\n# Changed direct executable identity.\n")
                .unwrap();
            f.release();
        }
        let result = f.terminal("codex");
        assert_eq!(result["status"], "failed", "{mutation}: {result}");
        assert_eq!(result["run"]["completed"], 0);
        assert_eq!(f.calls().len(), 1);
        assert_eq!(f.checkpoint(), (1, 0, 0));
        f.assert_drafts_only();
    }
}

#[test]
fn interrupted_idle_worker_can_restart_without_replaying_completed_work_or_an_old_stop() {
    let f = Fixture::new("ordinary");
    f.setup("codex");
    f.capture("codex", "before-crash", "one");
    f.start("codex", &["--max-calls", "8", "--max-runtime", "30"]);
    let old = f.until("codex", "first result was not completed", |s| {
        s["run"]["completed"] == 1
    });
    let directory = f.directory("codex");
    let pid = old["run"]["pid"].as_i64().unwrap() as libc::pid_t;
    // SAFETY: this is the synthetic worker observed after its provider exited.
    // The test deliberately exercises an abrupt idle-worker crash only.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
    f.until("codex", "owner lock survived process exit", |s| {
        s["status"] == "interrupted" && s["owner"] == "available"
    });
    fs::write(
        directory.join("stop.json"),
        json!({"schema":1,"run_id":old["run"]["run_id"]}).to_string(),
    )
    .unwrap();
    let new = f.restart("codex");
    assert_ne!(new["run"]["run_id"], old["run"]["run_id"]);
    f.capture("codex", "after-crash", "two");
    f.until("codex", "new work was not observed after restart", |s| {
        s["run"]["completed"] == 1
    });
    f.stop("codex");
    assert_eq!(f.calls().len(), 2);
    assert_eq!(f.checkpoint().1, 2);
}

#[test]
fn status_preserves_files_and_rejects_replaced_worker_storage() {
    let f = Fixture::new("ordinary");
    f.setup("codex");
    f.start("codex", &["--max-calls", "4", "--max-runtime", "20"]);
    f.stop("codex");
    let directory = f.directory("codex");
    let files = [
        "config.json",
        "state.json",
        "owner.lock",
        "heartbeat.json",
        "stop.json",
    ];
    let snapshot: Vec<_> = files
        .iter()
        .map(|name| {
            let path = directory.join(name);
            (
                fs::read(&path).unwrap(),
                fs::metadata(path).unwrap().modified().unwrap(),
            )
        })
        .collect();
    f.status("codex");
    for (name, (bytes, modified)) in files.iter().zip(snapshot) {
        let path = directory.join(name);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
    }
    let external = f.harness.join("external-state");
    fs::write(&external, b"untouched synthetic content").unwrap();
    fs::remove_file(directory.join("state.json")).unwrap();
    std::os::unix::fs::symlink(&external, directory.join("state.json")).unwrap();
    for action in ["status", "start", "stop"] {
        let output = f.run(&["miner", "hooks", "worker", action, "--client", "codex"]);
        assert!(!output.status.success(), "{action}: {output:?}");
    }
    assert_eq!(fs::read(&external).unwrap(), b"untouched synthetic content");
    assert!(f.calls().is_empty());
}
