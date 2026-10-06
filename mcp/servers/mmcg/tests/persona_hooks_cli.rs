//! Public hook→journal→semantic process→review→MCP replay. Every source, home,
//! processor and profile is synthetic and isolated from the developer's data.
#![cfg(unix)]

use mmcg::miner::store::{Habit, ProfileStore};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const QUOTE: &str = "Before changing an API, check its callers and preserve the contract.";
const BEHAVIOR: &str = "Checks callers before changing an API contract";

#[test]
fn archived_sources_keep_reviewed_claims_current_and_detect_tampering_and_forgetting() {
    let f = Fixture::new();
    f.capture("archive-one", "one");
    f.event("archive-one", "", "SessionEnd", json!({}));
    let first = f.episode("one");
    f.propose(&f.analyze(&first));
    f.capture("archive-two", "two");
    f.event("archive-two", "", "SessionEnd", json!({}));
    let second = f.episode("two");
    f.propose(&f.analyze(&second));
    assert!(f.observe().status.success());
    assert_eq!(f.profile()["habits"][0]["behavior"], BEHAVIOR);
    let archived = f.success(&["miner", "hooks", "archive"]);
    assert_eq!(archived["archived"], 2);
    assert_eq!(
        f.success(&["miner", "hooks", "show", first["id"].as_str().unwrap()])["episode"],
        first
    );
    assert_eq!(f.profile()["habits"][0]["behavior"], BEHAVIOR);
    let files: Vec<_> = fs::read_dir(f.home.join(".mastermind/persona-archive"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 2);
    for file in &files {
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let bytes = fs::read(&files[0]).unwrap();
    fs::write(&files[0], b"{}").unwrap();
    assert!(f.profile()["habits"].as_array().unwrap().is_empty());
    fs::write(&files[0], bytes).unwrap();
    assert_eq!(f.profile()["habits"][0]["behavior"], BEHAVIOR);
    f.success(&[
        "miner",
        "hooks",
        "forget",
        first["id"].as_str().unwrap(),
        "--revision",
        first["revision"].as_str().unwrap(),
    ]);
    assert_eq!(
        fs::read_dir(f.home.join(".mastermind/persona-archive"))
            .unwrap()
            .count(),
        1
    );
    assert!(f.profile()["habits"].as_array().unwrap().is_empty());
}

#[test]
fn episode_capacity_automatically_archives_closed_sources_before_new_capture() {
    let f = Fixture::new();
    let first = f.capture("old", "old-turn");
    f.event("old", "", "SessionEnd", json!({}));
    {
        let conn =
            rusqlite::Connection::open(f.home.join(".mastermind/persona-events.db")).unwrap();
        let text: String = conn
            .query_row(
                "SELECT data FROM hook_episode WHERE id=?1",
                [first["id"].as_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        let mut episode: Value = serde_json::from_str(&text).unwrap();
        for i in 0..1999 {
            let id = format!("{i:064x}");
            episode["id"] = json!(id);
            conn.execute(
                "INSERT INTO hook_episode(id,session,data) VALUES(?1,?2,?3)",
                rusqlite::params![
                    id,
                    episode["session"].as_str().unwrap(),
                    episode.to_string()
                ],
            )
            .unwrap();
        }
    }
    f.event("fresh", "", "SessionStart", json!({"source":"startup"}));
    f.event(
        "fresh",
        "new-turn",
        "UserPromptSubmit",
        json!({"prompt":"Inspect the service contract."}),
    );
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(
        status["readiness"]["pipeline"]["retention"]["archived_episodes"],
        32
    );
    assert_eq!(
        status["readiness"]["pipeline"]["retention"]["active_episodes"],
        1969
    );
}

#[test]
fn session_end_preserves_completed_local_candidates() {
    let f = Fixture::new();
    assert!(f
        .run(&["miner", "access", "grant", "--client", "codex"])
        .status
        .success());
    f.success(&["miner", "hooks", "setup", "--client", "codex", "--write"]);
    f.event("ended", "", "SessionStart", json!({"source":"startup"}));
    f.event(
        "ended",
        "one",
        "UserPromptSubmit",
        json!({"prompt":"I prefer short code reviews\nonly for trivial changes."}),
    );
    f.event(
        "ended",
        "one",
        "Stop",
        json!({"last_assistant_message":"Reviewed."}),
    );
    let stopped = f.episode("one");
    f.event("ended", "", "SessionEnd", json!({}));
    let shown = f.success(&["miner", "hooks", "show", stopped["id"].as_str().unwrap()]);
    assert_eq!(shown["episode"]["revision"], stopped["revision"]);
    assert!(shown["episode"]["coverage_gaps"]
        .as_array()
        .unwrap()
        .is_empty());
    let current = shown["drafts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|draft| draft["episode_revision"] == shown["episode"]["revision"])
        .expect("SessionEnd must preserve the completed source revision");
    let draft = f.success(&["miner", "hooks", "draft", current["id"].as_str().unwrap()]);
    assert_eq!(
        draft["draft"]["content"]["behavior"],
        "I prefer short code reviews only for trivial changes."
    );
    assert_eq!(draft["current"], true);
    assert_eq!(draft["draft"]["attested"], false);
}

#[test]
fn local_analysis_retries_on_the_next_native_event_after_profile_store_outage() {
    let f = Fixture::new();
    assert!(f
        .run(&["miner", "access", "grant", "--client", "codex"])
        .status
        .success());
    f.success(&["miner", "hooks", "setup", "--client", "codex", "--write"]);
    f.event("retry", "", "SessionStart", json!({"source":"startup"}));
    f.event(
        "retry",
        "one",
        "UserPromptSubmit",
        json!({"prompt":"I prefer short code review replies."}),
    );
    let store = f.home.join(".mastermind/style.db");
    let backup = f.home.join(".mastermind/style.unavailable");
    fs::rename(&store, &backup).unwrap();
    f.event(
        "retry",
        "one",
        "Stop",
        json!({"last_assistant_message":"The review is complete."}),
    );
    fs::rename(&backup, &store).unwrap();
    let episode = f.episode("one");
    assert!(
        f.success(&["miner", "hooks", "show", episode["id"].as_str().unwrap()])["drafts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.event(
        "next-session",
        "",
        "SessionStart",
        json!({"source":"startup"}),
    );
    let shown = f.success(&["miner", "hooks", "show", episode["id"].as_str().unwrap()]);
    assert_eq!(shown["drafts"].as_array().unwrap().len(), 1);
    assert_eq!(
        f.success(&["miner", "hooks", "status", "--client", "codex"])["readiness"]["pipeline"]
            ["local_analysis"]["retry_queue"]["pending"],
        0
    );
}

#[test]
fn local_hook_candidates_are_automatic_idempotent_and_never_accept_a_claim() {
    let f = Fixture::new();
    for args in [
        vec!["config", "user.name", "Hook Fixture"],
        vec!["config", "user.email", "hooks@example.invalid"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "core.hooksPath", ""],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&f.project)
            .env("HOME", &f.home)
            .output()
            .unwrap()
            .status
            .success());
    }
    for value in 0..8 {
        fs::write(
            f.project.join("service.py"),
            format!("def keep():\n    if True:\n        return {value}\n"),
        )
        .unwrap();
        for args in [
            vec!["add", "service.py"],
            vec!["commit", "-qm", "Local observation"],
        ] {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&f.project)
                .env("HOME", &f.home)
                .output()
                .unwrap()
                .status
                .success());
        }
    }
    let mined = f.run(&["miner", "profile"]);
    assert!(mined.status.success(), "{mined:?}");
    assert!(f
        .run(&["miner", "access", "grant", "--client", "codex"])
        .status
        .success());
    let configured = f.success(&["miner", "hooks", "setup", "--client", "codex", "--write"]);
    assert_eq!(
        configured["hook_config"]["hooks"]["Stop"][0]["hooks"][0]["statusMessage"],
        "Mastermind: Capture response and collect local candidates"
    );
    f.event("local", "", "SessionStart", json!({"source":"startup"}));
    let quote = "I prefer short code review replies.";
    let output = f.native(json!({"session_id":"local","turn_id":"first","hook_event_name":"UserPromptSubmit","prompt":quote}));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("additionalContext"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let offered = parse(output);
    assert!(offered["hookSpecificOutput"]["additionalContext"].is_string());
    let stop = json!({"last_assistant_message":"The code review is complete."});
    f.event("local", "first", "Stop", stop.clone());
    let episode = f.episode("first");
    let shown = f.success(&["miner", "hooks", "show", episode["id"].as_str().unwrap()]);
    assert_eq!(shown["drafts"].as_array().unwrap().len(), 1);
    let id = shown["drafts"][0]["id"].as_str().unwrap();
    let draft = f.success(&["miner", "hooks", "draft", id]);
    assert_eq!(draft["draft"]["content"]["behavior"], quote);
    assert_eq!(draft["draft"]["processor"]["engine"], "local_explicit");
    assert_eq!(draft["draft"]["processor"]["model"], false);
    assert_eq!(draft["evidence_class"], "no_recorded_prior_exposure");
    assert_eq!(draft["promotion_eligible"], true);
    assert_eq!(draft["draft"]["attested"], false);
    f.event("local", "first", "Stop", stop);
    let replay = f.success(&["miner", "hooks", "mine-local", "--limit", "16"]);
    assert_eq!(replay["model"], false);
    assert_eq!(
        replay["results"][0]["status"],
        "already_analyzed_or_changed"
    );
    let replayed = f.success(&["miner", "hooks", "draft", id]);
    assert_eq!(replayed["draft"]["revision"], draft["draft"]["revision"]);
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(
        status["readiness"]["pipeline"]["local_analysis"]["completed_revisions"],
        1
    );

    f.event(
        "local",
        "second",
        "UserPromptSubmit",
        json!({"prompt":"I prefer explicit code review results."}),
    );
    f.event(
        "local",
        "second",
        "Stop",
        json!({"last_assistant_message":"Reviewed."}),
    );
    let second = f.episode("second");
    let shown = f.success(&["miner", "hooks", "show", second["id"].as_str().unwrap()]);
    let dependent = f.success(&[
        "miner",
        "hooks",
        "draft",
        shown["drafts"][0]["id"].as_str().unwrap(),
    ]);
    assert_eq!(dependent["evidence_class"], "dependent_observation");
    assert_eq!(dependent["promotion_eligible"], false);
    let revised = f.success(&["miner", "hooks", "draft", id]);
    assert_eq!(revised["current"], true);
    assert_ne!(revised["draft"]["revision"], draft["draft"]["revision"]);
    let store = ProfileStore::open_read_only(&f.home.join(".mastermind/style.db")).unwrap();
    assert!(store.habits().unwrap().is_empty());
    assert!(store.feedback().unwrap().is_empty());
}

#[test]
fn local_hook_candidates_require_complete_capture_and_respect_delivery_opt_out() {
    let f = Fixture::new();
    assert!(f
        .run(&["miner", "access", "grant", "--client", "codex"])
        .status
        .success());
    f.success(&["miner", "hooks", "setup", "--client", "codex", "--write"]);
    let prompt = json!({"prompt":"I prefer short code review replies."});
    f.event(
        "missing-start",
        "incomplete",
        "UserPromptSubmit",
        prompt.clone(),
    );
    f.event(
        "missing-start",
        "incomplete",
        "Stop",
        json!({"last_assistant_message":"Done."}),
    );
    let incomplete = f.episode("incomplete");
    let shown = f.success(&["miner", "hooks", "show", incomplete["id"].as_str().unwrap()]);
    assert!(shown["drafts"].as_array().unwrap().is_empty());
    let replay = f.success(&["miner", "hooks", "mine-local"]);
    assert_eq!(replay["results"][0]["status"], "incomplete");
    f.success(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "codex",
        "--disable-profile",
        "--write",
    ]);
    f.event("disabled", "", "SessionStart", json!({"source":"startup"}));
    f.event("disabled", "opt-out", "UserPromptSubmit", prompt);
    f.event(
        "disabled",
        "opt-out",
        "Stop",
        json!({"last_assistant_message":"Done."}),
    );
    let capture = f.episode("opt-out");
    assert!(capture["coverage_gaps"].as_array().unwrap().is_empty());
    let shown = f.success(&["miner", "hooks", "show", capture["id"].as_str().unwrap()]);
    assert!(shown["drafts"].as_array().unwrap().is_empty());
}

#[test]
fn large_tool_output_keeps_a_receipt_without_poisoning_capture() {
    let f = Fixture::new();
    f.capture("large-output", "before");
    f.event(
        "large-output",
        "one",
        "UserPromptSubmit",
        json!({"prompt":QUOTE}),
    );
    let previous = f.episode("before");
    f.event(
        "large-output",
        "one",
        "PreToolUse",
        json!({"tool_use_id":"tool-one","tool_name":"exec_command","tool_input":{"cmd":"inspect"}}),
    );
    let mut native = json!({"session_id":"large-output","turn_id":"one","hook_event_name":"PostToolUse","tool_use_id":"tool-one","tool_name":"exec_command","tool_response":"x".repeat(350*1024)});
    let output = f.native(native.clone());
    assert!(output.status.success(), "{output:?}");
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(status["capture_pending"], false);
    assert_eq!(status["grant"]["gap"], "");
    let episode = f.episode("one");
    assert_eq!(episode["coverage_gaps"], json!(["no_stop_observed"]));
    let result = episode["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "PostToolUse")
        .unwrap();
    let receipt: Value = serde_json::from_str(result["text"].as_str().unwrap()).unwrap();
    assert_eq!(receipt["capture"], "metadata_only");
    native["cwd"] = json!(f.project.canonicalize().unwrap());
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let mut expected = String::new();
    for byte in Sha256::digest(serde_json::to_vec(&native).unwrap()) {
        write!(expected, "{byte:02x}").unwrap();
    }
    assert_eq!(receipt["native_event_digest"], expected);
    assert!(!episode.to_string().contains(&"x".repeat(1024)));
    f.event(
        "large-output",
        "one",
        "Stop",
        json!({"last_assistant_message":"Done"}),
    );
    f.event(
        "large-output",
        "two",
        "UserPromptSubmit",
        json!({"prompt":QUOTE}),
    );
    f.event(
        "large-output",
        "two",
        "Stop",
        json!({"last_assistant_message":"Done"}),
    );
    let fresh = f.episode("two");
    assert!(fresh["coverage_gaps"].as_array().unwrap().is_empty());
    assert_eq!(f.episode("before")["revision"], previous["revision"]);
    assert!(f.episode("one")["coverage_gaps"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(f.capture("fresh-session", "three")["coverage_gaps"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn missing_next_prompt_text_marks_its_two_uses_without_poisoning_later_turns() {
    let f = Fixture::new();
    f.capture("s", "one");
    f.event(
        "s",
        "two",
        "UserPromptSubmit",
        json!({"prompt":"API_KEY=sk-abcdefghijklmnopqrstuvwxyz0123456789"}),
    );
    f.event("s", "two", "Stop", json!({"last_assistant_message":"Done"}));
    for turn in ["one", "two"] {
        let episode = f.episode(turn);
        assert!(episode["coverage_gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap == "redacted_event_content"));
        assert!(!episode.to_string().contains("abcdefghijklmnopqrstuvwxyz"));
    }
    f.event("s", "three", "UserPromptSubmit", json!({"prompt":QUOTE}));
    f.event(
        "s",
        "three",
        "Stop",
        json!({"last_assistant_message":"Done"}),
    );
    assert!(f.episode("three")["coverage_gaps"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn private_late_tool_result_keeps_its_receipt_without_invalidating_the_next_episode() {
    let f = Fixture::new();
    f.event("s", "", "SessionStart", json!({"source":"startup"}));
    f.event("s", "one", "UserPromptSubmit", json!({"prompt":QUOTE}));
    f.event(
        "s",
        "one",
        "PreToolUse",
        json!({"tool_use_id":"tool-one","tool_name":"exec_command","tool_input":{"cmd":"inspect"}}),
    );
    f.event("s", "one", "Stop", json!({"last_assistant_message":"Done"}));
    f.event("s", "two", "UserPromptSubmit", json!({"prompt":QUOTE}));
    f.event("s", "two", "Stop", json!({"last_assistant_message":"Done"}));
    let clean = f.episode("two");
    f.event("s", "one", "PostToolUse", json!({"tool_use_id":"tool-one","tool_name":"exec_command","tool_response":"API_KEY=sk-abcdefghijklmnopqrstuvwxyz0123456789"}));
    let completed = f.episode("one");
    assert!(completed["coverage_gaps"].as_array().unwrap().is_empty());
    assert!(!completed.to_string().contains("abcdefghijklmnopqrstuvwxyz"));
    assert_eq!(f.episode("two")["revision"], clean["revision"]);
    assert!(f.episode("two")["coverage_gaps"]
        .as_array()
        .unwrap()
        .is_empty());
}

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
        assert!(Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&project)
            .env("HOME", &home)
            .output()
            .unwrap()
            .status
            .success());
        let f = Self {
            _temp: temp,
            home,
            project,
        };
        f.success(&["miner", "hooks", "setup", "--client", "codex", "--write"]);
        f
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mmcg"));
        c.current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env_remove("MASTERMIND_MINER")
            .env_remove("MMCG_PROFILE_CLIENT")
            .env_remove("MMCG_INDEX_PATH");
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn success(&self, args: &[&str]) -> Value {
        parse(self.run(args))
    }
    fn tty(&self, args: &[&str]) -> Output {
        let (mut master, mut slave) = (-1, -1);
        // Exercise the real interactive boundary in a synthetic home only.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let _master = unsafe { fs::File::from_raw_fd(master) };
        let slave = unsafe { fs::File::from_raw_fd(slave) };
        self.command().stdin(slave).args(args).output().unwrap()
    }
    fn native(&self, mut event: Value) -> Output {
        event["cwd"] = json!(self.project.canonicalize().unwrap());
        let mut child = self
            .command()
            .args(["miner", "hooks", "receive", "--client", "codex"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(event.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
    fn event(&self, session: &str, turn: &str, kind: &str, extras: Value) -> Value {
        let mut v = json!({"session_id":session,"hook_event_name":kind});
        if !turn.is_empty() {
            v["turn_id"] = json!(turn);
        }
        for (k, val) in extras.as_object().unwrap() {
            v[k] = val.clone();
        }
        parse(self.native(v))
    }
    fn capture(&self, session: &str, turn: &str) -> Value {
        self.event(session, "", "SessionStart", json!({"source":"startup"}));
        self.event(session, turn, "UserPromptSubmit", json!({"prompt":QUOTE}));
        self.event(session,turn,"Stop",json!({"last_assistant_message":"I inspected the callers. The change is still a proposal."}));
        self.episode(turn)
    }
    fn episode(&self, turn: &str) -> Value {
        let list = self.success(&["miner", "hooks", "episodes", "--limit", "100"]);
        list["episodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| self.success(&["miner", "hooks", "show", row["id"].as_str().unwrap()]))
            .find(|item| item["capture"]["turn_id"] == turn)
            .unwrap()["episode"]
            .clone()
    }
    fn processor(&self, episode: &Value, quote: &str) -> PathBuf {
        let support = episode["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["origin"] == "user_channel_unverified")
            .unwrap();
        let response = json!({"schema":1,"episode_id":episode["id"],"episode_revision":episode["revision"],"drafts":[{
            "when":"When changing a public API contract","behavior":BEHAVIOR,"rationale":null,"outcome":null,
            "exception":"No exception was observed.","role":null,"workflow":null,"evidence_kind":"technical_approach",
            "supports":[{"event_id":support["id"],"quote":quote}],"contradictions":[]}]});
        let path = self._temp.path().join("processor");
        // Single-quoted heredoc keeps the fixture bytes literal. It also
        // consumes the real stdin protocol and checks recursion protection.
        fs::write(&path,format!("#!/bin/sh\n[ \"$MASTERMIND_MINER\" = 1 ] || exit 7\ncat >/dev/null\ncat <<'RESPONSE'\n{response}\nRESPONSE\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn analyze(&self, episode: &Value) -> Value {
        let processor = self.processor(episode, QUOTE);
        self.success(&[
            "miner",
            "hooks",
            "analyze",
            episode["id"].as_str().unwrap(),
            "--revision",
            episode["revision"].as_str().unwrap(),
            "--processor",
            processor.to_str().unwrap(),
        ])["drafts"][0]
            .clone()
    }
    fn propose(&self, draft: &Value) -> Value {
        self.propose_task(
            draft,
            &format!("fixture-task-{}", &draft["episode"].as_str().unwrap()[..8]),
        )
    }
    fn propose_task(&self, draft: &Value, task: &str) -> Value {
        parse(self.tty(&[
            "miner",
            "hooks",
            "propose",
            draft["id"].as_str().unwrap(),
            "--revision",
            draft["revision"].as_str().unwrap(),
            "--episode",
            task,
            "--attest-human",
        ]))
    }
    fn habit(&self) -> Habit {
        ProfileStore::open_read_only(&self.home.join(".mastermind/style.db"))
            .unwrap()
            .habits()
            .unwrap()
            .remove(0)
    }
    fn observe(&self) -> Output {
        let h = self.habit();
        self.tty(&[
            "miner",
            "habit",
            "observe",
            &h.id.to_string(),
            "--revision",
            &h.review_revision(),
        ])
    }
    fn profile(&self) -> Value {
        let out = self.run(&["miner", "access", "grant", "--client", "fixture"]);
        assert!(out.status.success(), "{out:?}");
        let mut child = self
            .command()
            .env("MMCG_PROFILE_CLIENT", "fixture")
            .args(["serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}});
        let ready = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"mmcg_profile","arguments":{"paths":["src/api.rs"]}}});
        writeln!(child.stdin.take().unwrap(), "{init}\n{ready}\n{request}").unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
        let response = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|s| serde_json::from_str::<Value>(s).unwrap())
            .find(|v| v["id"] == 1)
            .unwrap();
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }
}
fn parse(out: Output) -> Value {
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {out:?}"))
}

#[test]
fn semantic_hook_habit_requires_attestation_independent_sources_review_and_current_mcp_receipts() {
    let f = Fixture::new();
    let first = f.capture("session-one", "turn-one");
    let draft = f.analyze(&first);
    assert!(!f.home.join(".mastermind/style.db").exists());
    assert!(!f
        .run(&[
            "miner",
            "hooks",
            "propose",
            draft["id"].as_str().unwrap(),
            "--revision",
            draft["revision"].as_str().unwrap(),
            "--episode",
            "fixture-task",
            "--attest-human"
        ])
        .status
        .success());
    f.propose(&draft);
    assert_eq!(f.habit().status, "candidate");
    assert!(!f.observe().status.success());
    let second = f.capture("session-two", "turn-two");
    f.propose(&f.analyze(&second));
    assert_eq!(f.habit().episodes, 2);
    assert_eq!(f.habit().sources, 2);
    assert!(f.profile()["habits"].as_array().unwrap().is_empty());
    let observed = f.observe();
    assert!(observed.status.success(), "{observed:?}");
    assert_eq!(f.profile()["habits"][0]["behavior"], BEHAVIOR);
    f.event(
        "session-one",
        "turn-three",
        "UserPromptSubmit",
        json!({"prompt":"For this task only, change the API without migrating callers."}),
    );
    assert!(
        !f.success(&["miner", "hooks", "draft", draft["id"].as_str().unwrap()])["current"]
            .as_bool()
            .unwrap()
    );
    assert!(f.profile()["habits"].as_array().unwrap().is_empty());
    assert!(!f.observe().status.success());
}

#[test]
fn retries_conflicts_gaps_and_recovery_never_manufacture_clean_evidence() {
    let f = Fixture::new();
    let episode = f.capture("session-a", "turn-a");
    f.event(
        "session-a",
        "turn-a",
        "UserPromptSubmit",
        json!({"prompt":QUOTE}),
    );
    assert_eq!(f.episode("turn-a")["revision"], episode["revision"]);
    f.event(
        "session-a",
        "turn-a",
        "UserPromptSubmit",
        json!({"prompt":"Conflicting payload using the same stable turn id."}),
    );
    let conflicted = f.episode("turn-a");
    assert!(conflicted["coverage_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "event_identity_conflict"));
    let p = f.processor(&conflicted, QUOTE);
    assert!(!f
        .run(&[
            "miner",
            "hooks",
            "analyze",
            conflicted["id"].as_str().unwrap(),
            "--revision",
            conflicted["revision"].as_str().unwrap(),
            "--processor",
            p.to_str().unwrap()
        ])
        .status
        .success());
    let failed = f
        .native(json!({"session_id":"session-a","hook_event_name":"UserPromptSubmit","prompt":42}));
    assert!(!failed.status.success());
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(status["grant"]["gap"], "unsupported_hook_schema");
    f.success(&["miner", "hooks", "recover", "--client", "codex"]);
    assert!(f.episode("turn-a")["coverage_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "capture_revoked_or_restarted"));
}

#[test]
fn incomplete_tool_pairs_forked_sessions_and_redacted_content_are_not_mineable() {
    let f = Fixture::new();
    f.event("s", "", "SessionStart", json!({"source":"startup"}));
    f.event("s", "t", "UserPromptSubmit", json!({"prompt":QUOTE}));
    f.event(
        "s",
        "t",
        "PreToolUse",
        json!({"tool_use_id":"tool-1","tool_name":"Bash","tool_input":{"command":"cargo test"}}),
    );
    f.event("s", "t", "Stop", json!({"last_assistant_message":"Done"}));
    assert!(f.episode("t")["coverage_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "missing_tool_result"));
    f.event(
        "s",
        "t",
        "PostToolUse",
        json!({"tool_use_id":"tool-1","tool_name":"Bash","tool_response":{"exit_code":0}}),
    );
    assert!(f.episode("t")["coverage_gaps"]
        .as_array()
        .unwrap()
        .is_empty());
    f.event(
        "fork",
        "",
        "SessionStart",
        json!({"parent_session_id":"s","source":"resume"}),
    );
    f.event(
        "fork",
        "fork-t",
        "UserPromptSubmit",
        json!({"prompt":QUOTE}),
    );
    f.event(
        "fork",
        "fork-t",
        "Stop",
        json!({"last_assistant_message":"Done"}),
    );
    assert!(f.episode("fork-t")["coverage_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "fork_or_delegated_session"));
    f.event("secret", "", "SessionStart", json!({"source":"startup"}));
    f.event(
        "secret",
        "secret-t",
        "UserPromptSubmit",
        json!({"prompt":"API_KEY=sk-abcdefghijklmnopqrstuvwxyz0123456789"}),
    );
    f.event(
        "secret",
        "secret-t",
        "Stop",
        json!({"last_assistant_message":"Done"}),
    );
    let secret = f.episode("secret-t");
    assert!(!secret.to_string().contains("abcdefghijklmnopqrstuvwxyz"));
    assert!(!secret["coverage_gaps"].as_array().unwrap().is_empty());
}

#[test]
fn fabricated_semantic_citation_is_rejected_without_profile_side_effects() {
    let f = Fixture::new();
    let episode = f.capture("session-a", "turn-a");
    let p = f.processor(
        &episode,
        "This sentence never appeared in the human prompt.",
    );
    assert!(!f
        .run(&[
            "miner",
            "hooks",
            "analyze",
            episode["id"].as_str().unwrap(),
            "--revision",
            episode["revision"].as_str().unwrap(),
            "--processor",
            p.to_str().unwrap()
        ])
        .status
        .success());
    assert!(!f.home.join(".mastermind/style.db").exists());
}

#[test]
fn missing_session_start_omits_profile_without_failing_local_capture() {
    let f = Fixture::new();
    let one = f.capture("s1", "t1");
    f.propose(&f.analyze(&one));
    let two = f.capture("s2", "t2");
    f.propose(&f.analyze(&two));
    assert!(f.observe().status.success());
    f.profile();
    f.success(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "codex",
        "--write",
        "--profile-client",
        "fixture",
    ]);
    let output = f.native(json!({"session_id":"already-open","turn_id":"missing-start","hook_event_name":"UserPromptSubmit","prompt":QUOTE}));
    assert!(output.status.success(), "{output:?}");
    assert_eq!(parse(output), json!({}));
    f.event(
        "already-open",
        "missing-start",
        "Stop",
        json!({"last_assistant_message":"Done"}),
    );
    let episode = f.episode("missing-start");
    assert!(episode["coverage_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|gap| gap == "missing_session_start"));
    assert_eq!(episode["profile_influenced"], false);
    assert!(f.capture("new-session", "clean")["profile_influenced"]
        .as_bool()
        .unwrap());
}

#[test]
fn unavailable_optional_profile_does_not_fail_local_capture() {
    let f = Fixture::new();
    f.success(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "codex",
        "--write",
        "--profile-client",
        "fixture",
    ]);
    fs::write(
        f.home.join(".mastermind/style.db"),
        b"invalid synthetic profile database",
    )
    .unwrap();
    f.event("s", "", "SessionStart", json!({"source":"startup"}));
    let output = f.native(json!({"session_id":"s","turn_id":"local","hook_event_name":"UserPromptSubmit","prompt":QUOTE}));
    assert!(output.status.success(), "{output:?}");
    assert_eq!(parse(output), json!({}));
    f.event(
        "s",
        "local",
        "Stop",
        json!({"last_assistant_message":"Done"}),
    );
    let episode = f.episode("local");
    assert!(episode["coverage_gaps"].as_array().unwrap().is_empty());
    assert_eq!(episode["profile_influenced"], false);
    assert_eq!(
        fs::read(f.home.join(".mastermind/style.db")).unwrap(),
        b"invalid synthetic profile database"
    );
}

#[test]
fn offered_profile_preserves_original_observation_and_restricts_later_echo_promotion() {
    let f = Fixture::new();
    let one = f.capture("s1", "t1");
    f.propose(&f.analyze(&one));
    let two = f.capture("s2", "t2");
    f.propose(&f.analyze(&two));
    assert!(f.observe().status.success());
    f.profile();
    f.success(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "codex",
        "--write",
        "--profile-client",
        "fixture",
    ]);
    f.event("s3", "", "SessionStart", json!({"source":"startup"}));
    let context = f.event("s3", "t3", "UserPromptSubmit", json!({"prompt":QUOTE}));
    assert!(context["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .contains(BEHAVIOR));
    f.event("s3", "t3", "Stop", json!({"last_assistant_message":"Done"}));
    let ep = f.episode("t3");
    assert_eq!(ep["profile_influenced"], true);
    let show = f.success(&["miner", "hooks", "show", ep["id"].as_str().unwrap()]);
    assert!(show["capture"]["exposures"][0]["habits"][0]["review_revision"].is_string());
    let first = f.analyze(&ep);
    let inspected = f.success(&["miner", "hooks", "draft", first["id"].as_str().unwrap()]);
    assert_eq!(inspected["evidence_class"], "no_recorded_prior_exposure");
    assert_eq!(inspected["promotion_eligible"], true);
    f.event("s3", "t4", "UserPromptSubmit", json!({"prompt":QUOTE,"influence":{"prior_unknown":false,"prior_profile_context":false,"prior_refiner_context":false}}));
    f.event("s3", "t4", "Stop", json!({"last_assistant_message":"Done"}));
    let later = f.analyze(&f.episode("t4"));
    let inspected = f.success(&["miner", "hooks", "draft", later["id"].as_str().unwrap()]);
    assert_eq!(inspected["evidence_class"], "dependent_observation");
    assert_eq!(inspected["promotion_eligible"], false);
    let count = f.habit().episodes;
    let result = f.tty(&[
        "miner",
        "hooks",
        "propose",
        later["id"].as_str().unwrap(),
        "--revision",
        later["revision"].as_str().unwrap(),
        "--episode",
        "echo-task",
        "--attest-human",
    ]);
    assert!(!result.status.success());
    assert_eq!(f.habit().episodes, count);
}

#[test]
fn every_prompt_selects_its_own_profile_without_a_refiner_and_setup_preserves_the_reader() {
    let f = Fixture::new();
    for (session, turn) in [("one", "first"), ("two", "second")] {
        let captured = f.capture(session, turn);
        f.propose(&f.analyze(&captured));
    }
    assert!(f.observe().status.success());
    f.profile();
    f.success(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "codex",
        "--profile-client",
        "fixture",
        "--write",
    ]);
    let refreshed = f.success(&["miner", "hooks", "setup", "--client", "codex", "--write"]);
    assert_eq!(refreshed["capture_grant"]["profile_client"], "fixture");
    assert_eq!(refreshed["refiner"]["status"], "not_configured");
    f.event("automatic", "", "SessionStart", json!({"source":"startup"}));
    let prompts = [
        (
            "rust".to_owned(),
            "Fix `src/api.rs`".to_owned(),
            json!(["src/api.rs"]),
        ),
        (
            "python".to_owned(),
            "Check src/api.py".to_owned(),
            json!(["src/api.py"]),
        ),
        (
            "unknown".to_owned(),
            "Исправь следующую задачу".to_owned(),
            json!([]),
        ),
    ]
    .into_iter()
    .chain((0..37).map(|index| {
        let path = format!("src/task-{index}.rs");
        (
            format!("task-{index}"),
            format!("Inspect {path}"),
            json!([path]),
        )
    }));
    for (turn, prompt, paths) in prompts {
        let output = f.event(
            "automatic",
            &turn,
            "UserPromptSubmit",
            json!({"prompt":prompt}),
        );
        let context = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        let packet: Value = serde_json::from_str(context.split_once('\n').unwrap().1).unwrap();
        assert_eq!(packet["selection"]["paths"], paths);
        assert_eq!(packet["selection"]["role"], "planner");
        assert!(packet["selection"]["workflow"].is_null());
        assert_eq!(
            packet["task_context"]["source"],
            "original_prompt_path_literals"
        );
        assert_eq!(packet["habits"][0]["behavior"], BEHAVIOR);
        f.event(
            "automatic",
            &turn,
            "Stop",
            json!({"last_assistant_message":"Done"}),
        );
        let episode = f.episode(&turn);
        let retained = f.success(&["miner", "hooks", "show", episode["id"].as_str().unwrap()]);
        assert_eq!(
            retained["capture"]["exposures"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["selection"]["paths"],
            paths
        );
        assert!(retained["intake"].is_null());
        assert!(episode["coverage_gaps"].as_array().unwrap().is_empty());
        let original = episode["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["kind"] == "UserPromptSubmit")
            .unwrap();
        assert_eq!(
            original["influence"]["prior_profile_context"],
            turn != "rust"
        );
        if turn == "task-36" {
            assert!(
                retained["capture"]["prior_exposure_summaries_omitted"]
                    .as_u64()
                    .unwrap()
                    > 0
            );
        }
    }
}

#[test]
fn existing_client_grant_enables_delivery_and_explicit_opt_out_survives_setup() {
    let f = Fixture::new();
    assert!(f
        .run(&["miner", "access", "grant", "--client", "codex"])
        .status
        .success());
    let automatic = f.success(&["miner", "hooks", "setup", "--client", "codex", "--write"]);
    assert_eq!(automatic["capture_grant"]["profile_client"], "codex");
    assert_eq!(automatic["readiness"]["profile"]["status"], "configured");
    let disabled = f.success(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "codex",
        "--disable-profile",
        "--write",
    ]);
    assert!(disabled["capture_grant"]["profile_client"].is_null());
    let repeated = f.success(&["miner", "hooks", "setup", "--client", "codex", "--write"]);
    assert!(repeated["capture_grant"]["profile_client"].is_null());
    f.success(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "codex",
        "--profile-client",
        "codex",
        "--write",
    ]);
    let enabled = f.success(&["miner", "hooks", "setup", "--client", "codex", "--write"]);
    assert_eq!(enabled["capture_grant"]["profile_client"], "codex");
    assert!(f
        .run(&["miner", "access", "revoke", "--client", "codex"])
        .status
        .success());
    let readiness = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(readiness["readiness"]["profile"]["status"], "access_denied");
}

#[test]
fn ordinary_mcp_profile_tool_read_marks_following_echo_as_influenced() {
    for tool_name in ["mcp__mmcg__mmcg_profile", "mcp__mmcg__mmcg_context"] {
        let f = Fixture::new();
        f.event("s", "", "SessionStart", json!({"source":"startup"}));
        f.event("s", "t1", "UserPromptSubmit", json!({"prompt":QUOTE}));
        f.event(
            "s",
            "t1",
            "PreToolUse",
            json!({"tool_name":tool_name,"tool_use_id":"profile-1","tool_input":{}}),
        );
        f.event("s","t1","PostToolUse",json!({"tool_name":tool_name,"tool_use_id":"profile-1","tool_response":{"status":"ok","profile_revision":"some-revision"}}));
        f.event(
            "s",
            "t1",
            "Stop",
            json!({"last_assistant_message":"I will follow the profile."}),
        );
        f.event("s", "t2", "UserPromptSubmit", json!({"prompt":QUOTE}));
        f.event("s", "t2", "Stop", json!({"last_assistant_message":"Done"}));
        let input = f.episode("t2");
        assert_eq!(input["profile_influenced"], true);
        let draft = f.analyze(&input);
        let inspected = f.success(&["miner", "hooks", "draft", draft["id"].as_str().unwrap()]);
        assert_eq!(inspected["evidence_class"], "dependent_observation");
        assert_eq!(inspected["promotion_eligible"], false);
    }
}

#[test]
fn the_same_task_across_sessions_is_not_independent_and_cannot_be_relabeled() {
    let f = Fixture::new();
    let a = f.analyze(&f.capture("s1", "t1"));
    f.propose_task(&a, "task-one");
    let b = f.analyze(&f.capture("s2", "t2"));
    f.propose_task(&b, "task-one");
    assert_eq!(f.habit().episodes, 1);
    assert_eq!(f.habit().sources, 2);
    assert!(!f.observe().status.success());
    let result = f.tty(&[
        "miner",
        "hooks",
        "propose",
        b["id"].as_str().unwrap(),
        "--revision",
        b["revision"].as_str().unwrap(),
        "--episode",
        "task-two",
        "--attest-human",
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("cannot be relabeled"));
}

#[test]
fn reanalysis_after_context_append_repairs_binding_and_requires_review_again() {
    let f = Fixture::new();
    let a = f.analyze(&f.capture("s1", "t1"));
    f.propose_task(&a, "task-one");
    let b = f.analyze(&f.capture("s2", "t2"));
    f.propose_task(&b, "task-two");
    assert!(f.observe().status.success());
    f.event(
        "s1",
        "t3",
        "UserPromptSubmit",
        json!({"prompt":"The first task is done. Apply the same approach to the next change."}),
    );
    let updated = f.episode("t1");
    assert!(f.profile()["habits"].as_array().unwrap().is_empty());
    let repaired = f.analyze(&updated);
    assert_eq!(repaired["id"], a["id"]);
    assert_ne!(repaired["revision"], a["revision"]);
    f.propose_task(&repaired, "task-one");
    assert_eq!(f.habit().status, "stale");
    assert!(f.profile()["habits"].as_array().unwrap().is_empty());
    let out = f.observe();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(f.profile()["habits"][0]["behavior"], BEHAVIOR);
}

#[test]
fn forget_removes_cross_turn_text_and_next_capture_has_a_gap_without_getting_stuck() {
    let f = Fixture::new();
    let first = f.capture("s", "t1");
    let sensitive =
        "For this task, please use a dedicated isolated queue and retain this exact choice.";
    f.event("s", "t2", "UserPromptSubmit", json!({"prompt":sensitive}));
    f.event(
        "s",
        "t2",
        "Stop",
        json!({"last_assistant_message":"An assistant response to forget."}),
    );
    let second = f.episode("t2");
    assert!(f.episode("t1").to_string().contains(sensitive));
    f.success(&[
        "miner",
        "hooks",
        "forget",
        second["id"].as_str().unwrap(),
        "--revision",
        second["revision"].as_str().unwrap(),
    ]);
    let retained = f.success(&["miner", "hooks", "show", first["id"].as_str().unwrap()]);
    assert!(!retained.to_string().contains(sensitive));
    f.event("s", "t3", "UserPromptSubmit", json!({"prompt":QUOTE}));
    f.event("s", "t3", "Stop", json!({"last_assistant_message":"Done"}));
    let next = f.episode("t3");
    assert!(!next
        .to_string()
        .contains("An assistant response to forget."));
    assert!(next["coverage_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "episode_forgotten"));
    assert_eq!(
        f.success(&["miner", "hooks", "status", "--client", "codex"])["grant"]["pending"],
        0
    );
}

#[test]
fn pending_capture_fences_mcp_and_a_recovery_cannot_reassign_its_delivery() {
    let f = Fixture::new();
    let a = f.analyze(&f.capture("s1", "t1"));
    f.propose(&a);
    let b = f.analyze(&f.capture("s2", "t2"));
    f.propose(&b);
    assert!(f.observe().status.success());
    assert_eq!(f.profile()["habits"][0]["behavior"], BEHAVIOR);
    let mut child = f
        .command()
        .args(["miner", "hooks", "receive", "--client", "codex"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let output = f.run(&["miner", "hooks", "status", "--client", "codex"]);
        let pending = if output.status.success() {
            parse(output)["grant"]["pending"] == 1
        } else {
            // The receiver may update SQLite while status opens its bounded
            // file snapshot. That refusal is safe and remains deadline-bound.
            assert_eq!(
                String::from_utf8_lossy(&output.stderr).trim(),
                "mmcg error: file identity changed during read",
                "{output:?}"
            );
            false
        };
        if pending {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "capture did not commit its fence before stdin"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(f.profile()["habits"].as_array().unwrap().is_empty());
    let generation = f.success(&["miner", "hooks", "recover", "--client", "codex"])["grant"]
        ["generation"]
        .clone();
    let old_delivery = json!({"session_id":"s1","turn_id":"old-delivery","hook_event_name":"UserPromptSubmit","cwd":f.project.canonicalize().unwrap(),"prompt":QUOTE});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(old_delivery.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("revoked"));
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(status["grant"]["generation"], generation);
    assert_eq!(status["grant"]["pending"], 0);
}

#[test]
fn batch_worker_checkpoints_empty_results_and_retries_failed_processors() {
    let f = Fixture::new();
    let episode = f.capture("session-one", "turn-one");
    let processor = f._temp.path().join("empty-processor");
    let counter = f._temp.path().join("processor-invocations");
    let response = json!({"schema":1,"episode_id":episode["id"],"episode_revision":episode["revision"],"drafts":[]});
    fs::write(
        &processor,
        format!(
            "#!/bin/sh\ncat >/dev/null\nprintf x >> '{}'\ncat <<'RESPONSE'\n{response}\nRESPONSE\n",
            counter.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&processor, fs::Permissions::from_mode(0o700)).unwrap();
    let args = [
        "miner",
        "hooks",
        "mine",
        "--processor",
        processor.to_str().unwrap(),
    ];
    let mined = f.success(&args);
    assert_eq!(mined["results"].as_array().unwrap().len(), 1);
    assert!(mined["results"][0]["drafts"].as_array().unwrap().is_empty());
    let again = f.success(&args);
    assert!(again["results"].as_array().unwrap().is_empty());
    assert_eq!(fs::read_to_string(&counter).unwrap(), "x");
    assert!(!f.home.join(".mastermind/style.db").exists());

    let failed = f._temp.path().join("failed-processor");
    fs::write(&failed, "#!/bin/sh\ncat >/dev/null\nexit 1\n").unwrap();
    fs::set_permissions(&failed, fs::Permissions::from_mode(0o700)).unwrap();
    let args = [
        "miner",
        "hooks",
        "mine",
        "--processor",
        failed.to_str().unwrap(),
    ];
    let result = f.run(&args);
    assert!(!result.status.success());
    fs::write(
        &failed,
        format!("#!/bin/sh\ncat >/dev/null\ncat <<'RESPONSE'\n{response}\nRESPONSE\n"),
    )
    .unwrap();
    assert_eq!(f.success(&args)["results"].as_array().unwrap().len(), 1);
}

struct ForegroundWorker {
    child: std::process::Child,
    log: PathBuf,
}

impl ForegroundWorker {
    fn start(f: &Fixture, processor: &std::path::Path) -> Self {
        let log = f._temp.path().join("foreground-worker.log");
        let output = fs::File::create(&log).unwrap();
        Self {
            child: f
                .command()
                .args([
                    "miner",
                    "hooks",
                    "mine",
                    "--follow",
                    "--limit",
                    "1",
                    "--processor",
                ])
                .arg(processor)
                .stdout(output.try_clone().unwrap())
                .stderr(output)
                .spawn()
                .unwrap(),
            log,
        }
    }

    fn stop(&mut self, signal: libc::c_int) {
        // SAFETY: this test owns the live worker child.
        assert_eq!(
            unsafe { libc::kill(self.child.id() as libc::pid_t, signal) },
            0
        );
        wait_until("foreground worker cancellation", || {
            self.child.try_wait().unwrap().is_some()
        });
        let status = self.child.wait().unwrap();
        assert!(
            status.success(),
            "foreground worker exited with {status}:\n{}",
            fs::read_to_string(&self.log).unwrap()
        );
    }
}

impl Drop for ForegroundWorker {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // SAFETY: the un-reaped child is owned by this fixture. Let its
            // normal cancellation path clean up its processor process group.
            unsafe {
                libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
            }
            for _ in 0..100 {
                if self.child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn wait_until(description: &str, mut predicate: impl FnMut() -> bool) {
    let started = std::time::Instant::now();
    while !predicate() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "timed out: {description}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn foreground_worker_observes_new_and_revised_episodes_without_repeating_completed_requests() {
    let f = Fixture::new();
    let first = f.capture("follow-one", "turn-one");
    let root = f._temp.path();
    let processor = root.join("continuous-processor");
    let request = root.join("request.json");
    let response = root.join("response.json");
    // The test answers the actual protocol requests, atomically, without a
    // provider dependency or assumptions about JSON serialization ordering.
    fs::write(&processor, format!(
        "#!/bin/sh\ncat > '{0}/request.tmp'\nmv '{0}/request.tmp' '{0}/request.json'\nwhile [ ! -f '{0}/response.json' ]; do sleep 0.02; done\ncat '{0}/response.json'\nrm '{0}/response.json'\n", root.display())).unwrap();
    fs::set_permissions(&processor, fs::Permissions::from_mode(0o700)).unwrap();
    let mut worker = ForegroundWorker::start(&f, &processor);
    let db = rusqlite::Connection::open(f.home.join(".mastermind/persona-events.db")).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(1)).unwrap();
    let answer = |expected_count: i64| -> Value {
        wait_until("semantic request", || request.exists());
        let input: Value = serde_json::from_slice(&fs::read(&request).unwrap()).unwrap();
        fs::remove_file(&request).unwrap();
        let result = json!({"schema":1,"episode_id":input["episode"]["id"],
            "episode_revision":input["episode"]["revision"],"drafts":[]});
        let temporary = response.with_extension("tmp");
        fs::write(&temporary, result.to_string()).unwrap();
        fs::rename(temporary, &response).unwrap();
        wait_until("completed checkpoint", || {
            db.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM hook_analysis WHERE completed=1",
                [],
                |r| r.get(0),
            )
            .unwrap()
                == expected_count
        });
        input["episode"].clone()
    };
    assert_eq!(answer(1)["id"], first["id"]);
    std::thread::sleep(std::time::Duration::from_millis(2300));
    assert!(
        !request.exists(),
        "completed empty result was analyzed again"
    );

    let second = f.capture("follow-two", "turn-two");
    assert_eq!(answer(2)["id"], second["id"]);

    // Appending context changes an earlier ID, so following only newly added
    // IDs would silently miss this correction. The new turn remains open.
    f.event(
        "follow-one",
        "turn-three",
        "UserPromptSubmit",
        json!({"prompt":"This applies only to public APIs, not a private prototype."}),
    );
    let revised = answer(3);
    assert_eq!(revised["id"], first["id"]);
    assert_ne!(revised["revision"], first["revision"]);
    worker.stop(libc::SIGINT);
    assert!(!f.home.join(".mastermind/style.db").exists());
}

#[test]
fn foreground_worker_cancellation_during_fingerprinting_never_starts_the_provider() {
    let f = Fixture::new();
    f.capture("fingerprint-session", "fingerprint-turn");
    let processor = f._temp.path().join("large-processor");
    let marker = f._temp.path().join("provider-started");
    fs::write(
        &processor,
        format!("#!/bin/sh\nprintf x > '{}'\nexit 1\n#", marker.display()),
    )
    .unwrap();
    let file = fs::OpenOptions::new().write(true).open(&processor).unwrap();
    // A sparse executable keeps hashing in flight long enough to interrupt it.
    file.set_len(128 * 1024 * 1024).unwrap();
    file.set_times(fs::FileTimes::new().set_accessed(std::time::UNIX_EPOCH))
        .unwrap();
    drop(file);
    fs::set_permissions(&processor, fs::Permissions::from_mode(0o700)).unwrap();
    let mut worker = ForegroundWorker::start(&f, &processor);
    wait_until("processor fingerprint read", || {
        fs::metadata(&processor).unwrap().accessed().unwrap() > std::time::UNIX_EPOCH
    });
    worker.stop(libc::SIGINT);
    assert!(!marker.exists(), "provider started after cancellation");
    let db = rusqlite::Connection::open(f.home.join(".mastermind/persona-events.db")).unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>("SELECT COUNT(*) FROM hook_analysis", [], |r| r.get(0))
            .unwrap(),
        0,
        "cancellation before analysis must not claim an episode"
    );
}

#[test]
fn foreground_worker_cancellation_terminates_provider_and_releases_its_lease() {
    let f = Fixture::new();
    f.capture("cancel-session", "cancel-turn");
    let processor = f._temp.path().join("slow-processor");
    let pid_file = f._temp.path().join("processor.pid");
    fs::write(
        &processor,
        format!(
            "#!/bin/sh\ncat >/dev/null\necho $$ > '{0}.tmp'\nmv '{0}.tmp' '{0}'\nsleep 30\n",
            pid_file.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&processor, fs::Permissions::from_mode(0o700)).unwrap();
    let mut worker = ForegroundWorker::start(&f, &processor);
    wait_until("provider started", || pid_file.exists());
    let pid: libc::pid_t = fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    worker.stop(libc::SIGTERM);
    wait_until("provider process group exited", || {
        // SAFETY: signal 0 only checks existence of this fixture's group.
        unsafe { libc::kill(-pid, 0) != 0 }
    });
    let db = rusqlite::Connection::open(f.home.join(".mastermind/persona-events.db")).unwrap();
    let lease: (i64, i64) = db
        .query_row("SELECT completed,lease_until FROM hook_analysis", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(lease, (0, 0));
}

#[test]
fn sqlite_writer_contention_leaves_a_durable_delivery_fence_before_sql_admission() {
    let f = Fixture::new();
    let input = f.capture("s", "t1");
    let journal = f.home.join(".mastermind/persona-events.db");
    let connection = rusqlite::Connection::open(&journal).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    let rejected=f.native(json!({"session_id":"s","turn_id":"t2","hook_event_name":"UserPromptSubmit","prompt":"A correction that could not enter the busy journal."}));
    assert!(!rejected.status.success());
    connection.execute_batch("ROLLBACK").unwrap();
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(status["grant"]["pending"], 0);
    assert_eq!(status["capture_pending"], true);
    let source = f.success(&["miner", "hooks", "show", input["id"].as_str().unwrap()]);
    assert!(source["episode"]["coverage_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|gap| gap == "capture_delivery_pending_or_unavailable"));
    f.success(&["miner", "hooks", "recover", "--client", "codex"]);
    assert_eq!(
        f.success(&["miner", "hooks", "status", "--client", "codex"])["capture_pending"],
        false
    );
    assert!(f.episode("t1")["coverage_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|gap| gap == "capture_revoked_or_restarted"));
}

#[test]
fn unreviewed_draft_history_does_not_hide_a_later_selected_attestation() {
    let f = Fixture::new();
    let input = f.capture("s", "t1");
    let draft = f.analyze(&input);
    let connection =
        rusqlite::Connection::open(f.home.join(".mastermind/persona-events.db")).unwrap();
    // Exercise the persisted migration boundary with a legal backlog of
    // unreviewed hypotheses, without paying for hundreds of fake processes.
    for index in 0..520 {
        let id = format!("{index:064x}");
        let mut pending = draft.clone();
        pending["id"] = json!(id);
        connection
            .execute(
                "INSERT OR IGNORE INTO hook_draft(id,episode,data) VALUES(?1,?2,?3)",
                rusqlite::params![id, input["id"].as_str().unwrap(), pending.to_string()],
            )
            .unwrap();
    }
    f.propose_task(&draft, "one-real-task");
    assert_eq!(f.habit().episodes, 1);
}
