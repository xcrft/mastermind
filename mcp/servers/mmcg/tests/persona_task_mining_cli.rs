//! Synthetic user channels exercise local storage and MCP only. The native
//! client fixture fails every invocation; no account or model is used.
#![cfg(unix)]
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const PROMPT: &str = "I prefer short reviews only for simple changes.";

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    client: &'static str,
}

impl Fixture {
    fn new() -> Self {
        Self::for_client("codex")
    }

    fn for_client(client: &'static str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        for dir in [&root, &home, &bin] {
            fs::create_dir(dir).unwrap();
        }
        let f = Self {
            _temp: temp,
            root: root.canonicalize().unwrap(),
            home,
            bin,
            client,
        };
        fs::write(f.root.join("sample.py"), "def sample():\n    return True\n").unwrap();
        for client in ["codex", "claude"] {
            let path = f.bin.join(client);
            fs::write(
                &path,
                "#!/bin/sh\nprintf called >> \"$HOME/provider-calls\"\nexit 91\n",
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(f
            .command(&["index", "."])
            .output()
            .unwrap()
            .status
            .success());
        let mut settings = mmcg::onboarding::Settings::local(&f.root);
        settings.clients = vec![client.into()];
        settings.mining = mmcg::onboarding::Mining::Task;
        settings.workflow = false;
        mmcg::onboarding::Session::begin(&f.root)
            .unwrap()
            .save(&settings)
            .unwrap();
        f.success(&["miner", "hooks", "setup", "--client", client, "--write"]);
        f.event(
            "SessionStart",
            "",
            if client == "codex" {
                json!({"model":"gpt-task-model"})
            } else {
                json!({})
            },
        );
        f
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mmcg"));
        command
            .current_dir(&self.root)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .args(args);
        command
    }

    fn success(&self, args: &[&str]) -> Value {
        let output = self.command(args).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn event(&self, kind: &str, turn: &str, extra: Value) -> Value {
        let mut value = json!({"cwd":self.root,"session_id":"task-session","hook_event_name":kind});
        if !turn.is_empty() {
            value["turn_id"] = json!(turn);
        }
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut child = self
            .command(&["miner", "hooks", "receive", "--client", self.client])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        write!(child.stdin.take().unwrap(), "{value}").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn offer(&self, turn: &str, prompt: &str) -> String {
        let result = self.event("UserPromptSubmit", turn, json!({"prompt":prompt}));
        let context = result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.len() <= 900);
        context
            .split("ticket_id=\"")
            .nth(1)
            .unwrap()
            .chars()
            .take(64)
            .collect()
    }

    fn submit(&self, ticket: &str, candidates: Value, client: &str) -> Value {
        self.submit_at(ticket, candidates, client, &self.root)
    }

    fn submit_at(
        &self,
        ticket: &str,
        candidates: Value,
        client: &str,
        root: &std::path::Path,
    ) -> Value {
        let mut child = self
            .command(&["serve"])
            .env("MMCG_PROFILE_CLIENT", client)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}});
        let ready = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"mmcg_mining_submit","arguments":{"ticket_id":ticket,"candidates":candidates}}});
        writeln!(child.stdin.take().unwrap(), "{init}\n{ready}\n{call}").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|value| value["id"] == 1)
            .unwrap()
    }

    fn episode(&self) -> Value {
        let list = self.success(&["miner", "hooks", "episodes"]);
        let id = list["episodes"][0]["id"].as_str().unwrap();
        self.success(&["miner", "hooks", "show", id])
    }

    fn candidates(&self, quote: &str) -> Value {
        json!([{"when":"Reviewing simple changes","behavior":"Keep the review concise","exception":"Only for simple changes","evidence_kind":"review_preference","quote":quote}])
    }

    fn no_inference(&self) {
        assert!(!self.home.join("provider-calls").exists());
    }
}

fn failed(response: &Value) -> bool {
    response.get("error").is_some() || response["result"]["isError"] == true
}

#[test]
fn foreign_roots_and_forgotten_sources_cannot_use_a_task_ticket() {
    let f = Fixture::new();
    let ticket = f.offer("one", PROMPT);
    let other = f._temp.path().join("other");
    fs::create_dir(&other).unwrap();
    assert!(failed(&f.submit_at(
        &ticket,
        f.candidates(PROMPT),
        "codex",
        &other
    )));
    assert!(!failed(&f.submit(&ticket, f.candidates(PROMPT), "codex")));
    let source = f.episode();
    let episode = source["episode"]["id"].as_str().unwrap().to_string();
    let revision = source["episode"]["revision"].as_str().unwrap();
    f.success(&["miner", "hooks", "forget", &episode, "--revision", revision]);
    assert!(failed(&f.submit(&ticket, f.candidates(PROMPT), "codex")));
    let db = rusqlite::Connection::open(f.home.join(".mastermind/persona-events.db")).unwrap();
    let retained: i64 = db
        .query_row(
            "SELECT count(*) FROM hook_task_mining WHERE id=?1",
            [ticket],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, 0);
    f.no_inference();
}

#[test]
fn the_local_finalizer_waits_for_claudes_actual_model_without_calling_it() {
    let f = Fixture::for_client("claude");
    let ticket = f.offer("one", PROMPT);
    assert!(!failed(&f.submit(&ticket, f.candidates(PROMPT), "claude")));
    let slug = f
        .root
        .to_string_lossy()
        .replace(|ch: char| !ch.is_ascii_alphanumeric(), "-");
    let dir = f.home.join(".claude/projects").join(slug);
    fs::create_dir_all(&dir).unwrap();
    let transcript = dir.join("task-session.jsonl");
    fs::write(&transcript, "").unwrap();
    f.event("Stop","one",json!({"last_assistant_message":"Reviewed the change.","prompt_id":"one","transcript_path":transcript}));
    let user = json!({"type":"user","sessionId":"task-session","cwd":f.root,"promptId":"one","message":{"role":"user","content":PROMPT}});
    let assistant = json!({"type":"assistant","sessionId":"task-session","cwd":f.root,"message":{"role":"assistant","model":"claude-task-model","content":[{"type":"text","text":"Reviewed the change."}]}});
    fs::write(transcript, format!("{user}\n{assistant}\n")).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    loop {
        let shown = f.episode();
        if let Some(analysis) = shown["analyses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|analysis| analysis["processor"]["engine"] == "current_task_agent")
        {
            assert_eq!(analysis["current"], true);
            assert_eq!(analysis["processor"]["model"], "claude-task-model");
            assert_eq!(
                analysis["processor"]["model_binding"]["source"],
                "native_session_transcript"
            );
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{shown}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    f.no_inference();
}

#[test]
fn the_current_task_stages_and_seals_source_cited_drafts_without_inference() {
    let f = Fixture::new();
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"])["readiness"].clone();
    assert_eq!(status["mining"]["execution"], "in_session", "{status}");
    assert!(!status["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning == "managed_miner_not_running"));
    let ticket = f.offer("one", PROMPT);
    let result = f.submit(&ticket, f.candidates(PROMPT), "codex");
    assert!(!failed(&result), "{result}");
    assert!(!f.episode()["drafts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|draft| draft["processor"]["engine"] == "current_task_agent"));
    assert!(!failed(&f.submit(&ticket, f.candidates(PROMPT), "codex")));
    f.event(
        "Stop",
        "one",
        json!({"last_assistant_message":"Reviewed the change."}),
    );
    let shown = f.episode();
    let current: Vec<_> = shown["analyses"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|analysis| analysis["processor"]["engine"] == "current_task_agent")
        .collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0]["current"], true);
    assert_eq!(current[0]["processor"]["model"], "gpt-task-model");
    assert_eq!(current[0]["processor"]["separate_model_invocations"], 0);
    let draft = shown["drafts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|receipt| {
            f.success(&["miner", "hooks", "draft", receipt["id"].as_str().unwrap()])["draft"]
                .clone()
        })
        .find(|draft| draft["processor"]["engine"] == "current_task_agent")
        .unwrap();
    assert_eq!(draft["attested"], false);
    assert_eq!(draft["content"]["exception"], "Only for simple changes");
    assert_eq!(draft["content"]["supports"][0]["quote"], PROMPT);
    f.event(
        "Stop",
        "one",
        json!({"last_assistant_message":"Reviewed the change."}),
    );
    assert_eq!(
        f.episode()["analyses"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|analysis| analysis["processor"]["engine"] == "current_task_agent")
            .count(),
        1
    );
    f.no_inference();
}

#[test]
fn a_conflicting_submission_and_mode_revocation_cannot_publish_a_task_result() {
    let f = Fixture::new();
    let ticket = f.offer("one", PROMPT);
    assert!(!failed(&f.submit(&ticket, f.candidates(PROMPT), "codex")));
    let mut changed = f.candidates(PROMPT);
    changed[0]["behavior"] = json!("Write an extensive review instead");
    assert!(failed(&f.submit(&ticket, changed, "codex")));
    let mut settings = mmcg::onboarding::load(&f.root).unwrap().unwrap();
    settings.mining = mmcg::onboarding::Mining::Capture;
    mmcg::onboarding::Session::begin(&f.root)
        .unwrap()
        .save(&settings)
        .unwrap();
    assert!(failed(&f.submit(&ticket, f.candidates(PROMPT), "codex")));
    f.event("Stop", "one", json!({"last_assistant_message":"Reviewed."}));
    assert!(!f.episode()["analyses"]
        .as_array()
        .unwrap()
        .iter()
        .any(|analysis| analysis["processor"]["engine"] == "current_task_agent"));
    f.no_inference();
}

#[test]
fn wrong_client_invented_quotes_and_reused_tickets_are_rejected() {
    let f = Fixture::new();
    let ticket = f.offer("one", PROMPT);
    assert!(failed(&f.submit(&ticket, f.candidates(PROMPT), "claude")));
    assert!(failed(&f.submit(
        &ticket,
        f.candidates("Invented preference that was never stated."),
        "codex"
    )));
    let second = f.offer("two", "Inspect the next change.");
    assert_ne!(ticket, second);
    assert!(failed(&f.submit(&ticket, f.candidates(PROMPT), "codex")));
    f.no_inference();
}

#[test]
fn quoted_material_and_incomplete_stops_never_become_task_analysis() {
    let f = Fixture::new();
    let ticket = f.offer("one", &format!("Quoted example:\n> {PROMPT}"));
    assert!(failed(&f.submit(&ticket, f.candidates(PROMPT), "codex")));
    f.event(
        "Stop",
        "one",
        json!({"last_assistant_message":"Read the example."}),
    );
    let second = f.offer("two", PROMPT);
    assert!(!failed(&f.submit(&second, f.candidates(PROMPT), "codex")));
    f.event(
        "PreToolUse",
        "two",
        json!({"tool_use_id":"unfinished","tool_name":"Bash","tool_input":{"command":"true"}}),
    );
    f.event("Stop", "two", json!({"last_assistant_message":"Stopped."}));
    let list = f.success(&["miner", "hooks", "episodes"]);
    for ep in list["episodes"].as_array().unwrap() {
        let shown = f.success(&["miner", "hooks", "show", ep["id"].as_str().unwrap()]);
        assert!(!shown["analyses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|analysis| analysis["processor"]["engine"] == "current_task_agent"));
    }
    f.no_inference();
}
