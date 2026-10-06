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
    let pending = f.episode();
    assert_eq!(pending["task_mining"]["status"], "submitted");
    assert!(
        pending["drafts"].as_array().unwrap().is_empty(),
        "{pending}"
    );
    assert!(
        pending["analyses"].as_array().unwrap().is_empty(),
        "{pending}"
    );
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
    f.event("SessionEnd", "", json!({}));
    let ended = f.success(&[
        "miner",
        "hooks",
        "show",
        shown["episode"]["id"].as_str().unwrap(),
    ]);
    assert_eq!(ended["episode"]["revision"], shown["episode"]["revision"]);
    let analysis = ended["analyses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|analysis| analysis["processor"]["engine"] == "current_task_agent")
        .unwrap();
    assert_eq!(analysis["current"], true);
    let retained = f.success(&["miner", "hooks", "draft", draft["id"].as_str().unwrap()]);
    assert_eq!(retained["current"], true);
    assert_eq!(retained["draft"]["revision"], draft["revision"]);
    assert_eq!(retained["draft"]["attested"], false);
    // Older captures retained the empty closing event inside the episode.
    // Reading that representation must preserve the original source binding.
    let db = rusqlite::Connection::open(f.home.join(".mastermind/persona-events.db")).unwrap();
    let end_id: String = db
        .query_row(
            "SELECT id FROM hook_event WHERE kind='SessionEnd'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut legacy = ended["capture"].clone();
    let influence = legacy["events"][0]["influence"].clone();
    legacy["events"].as_array_mut().unwrap().push(json!({
        "id":end_id,"kind":"SessionEnd","actor":"system","origin":"client","text":"",
        "influence":influence
    }));
    db.execute(
        "UPDATE hook_episode SET data=?1 WHERE id=?2",
        rusqlite::params![legacy.to_string(), legacy["id"].as_str().unwrap()],
    )
    .unwrap();
    let historical = f.success(&["miner", "hooks", "draft", draft["id"].as_str().unwrap()]);
    assert_eq!(historical["current"], true);
    assert_eq!(historical["draft"]["revision"], draft["revision"]);
    f.no_inference();
}

#[test]
fn long_task_capture_preserves_user_evidence_and_seals_after_a_late_tool_result() {
    let f = Fixture::new();
    let ticket = f.offer("one", PROMPT);
    for number in 0..100 {
        let id = format!("tool-{number}");
        f.event(
            "PreToolUse",
            "one",
            json!({"tool_use_id":id,"tool_name":"Bash","tool_input":{"command":"inspect"}}),
        );
        if number != 99 {
            let body = if number == 0 {
                "API_KEY=sk-synthetic-private-value".to_string() + &"x".repeat(350 * 1024)
            } else {
                "Inspected.".into()
            };
            f.event(
                "PostToolUse",
                "one",
                json!({"tool_use_id":id,"tool_name":"Bash","tool_response":body}),
            );
        }
    }
    let staged = f.submit(&ticket, f.candidates(PROMPT), "codex");
    assert!(!failed(&staged), "{staged}");
    f.event("Stop", "one", json!({"last_assistant_message":"Reviewed."}));
    let pending = f.episode();
    assert_eq!(
        pending["episode"]["coverage_gaps"],
        json!(["missing_tool_result"])
    );
    assert!(pending["analyses"].as_array().unwrap().is_empty());
    f.event(
        "PostToolUse",
        "one",
        json!({"tool_use_id":"tool-99","tool_name":"Bash","tool_response":"Inspected."}),
    );
    let completed = f.episode();
    assert!(completed["episode"]["coverage_gaps"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(completed["task_mining"]["status"], "completed");
    let analysis = completed["analyses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|analysis| analysis["processor"]["engine"] == "current_task_agent")
        .unwrap();
    assert_eq!(analysis["current"], true);
    assert_eq!(analysis["processor"]["model"], "gpt-task-model");
    assert_eq!(analysis["processor"]["separate_model_invocations"], 0);
    let events = completed["episode"]["events"].as_array().unwrap();
    assert!(events.len() < 40, "{} retained events", events.len());
    let original = events
        .iter()
        .find(|event| event["origin"] == "user_channel_unverified")
        .unwrap();
    assert_eq!(original["text"], PROMPT);
    let trace = events
        .iter()
        .find(|event| event["kind"] == "ToolTrace")
        .unwrap();
    let trace: Value = serde_json::from_str(trace["text"].as_str().unwrap()).unwrap();
    assert_eq!(trace["events"], 200);
    assert_eq!(trace["receipts_retained"], 32);
    assert!(!completed.to_string().contains("synthetic-private-value"));
    let db = rusqlite::Connection::open(f.home.join(".mastermind/persona-events.db")).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM hook_event WHERE kind IN ('PreToolUse','PostToolUse')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 200);
    let revision = completed["episode"]["revision"].clone();
    f.event(
        "PostToolUse",
        "one",
        json!({"tool_use_id":"tool-99","tool_name":"Bash","tool_response":"Inspected."}),
    );
    assert_eq!(f.episode()["episode"]["revision"], revision);
    f.event("PreToolUse", "one", json!({"tool_use_id":"changed-source","tool_name":"Bash","tool_input":{"command":"inspect again"}}));
    f.event("PostToolUse", "one", json!({"tool_use_id":"changed-source","tool_name":"Bash","tool_response":"Different source context."}));
    let changed = f.episode();
    assert_ne!(changed["episode"]["revision"], revision);
    assert!(changed["episode"]["coverage_gaps"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(changed["analyses"]
        .as_array()
        .unwrap()
        .iter()
        .all(|analysis| analysis["current"] == false));
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

#[test]
fn stop_requires_a_result_and_keeps_the_original_source_in_both_clients() {
    for client in ["codex", "claude"] {
        let f = Fixture::for_client(client);
        let ticket = f.offer("one", PROMPT);
        let stop = json!({"last_assistant_message":"Reviewed.","stop_hook_active":false});
        let checkpoint = f.event("Stop", "one", stop.clone());
        assert_eq!(checkpoint["decision"], "block");
        let reason = checkpoint["reason"].as_str().unwrap();
        assert!(reason.contains(&ticket));
        assert_eq!(f.event("Stop", "one", stop), checkpoint);
        let pending = f.episode();
        assert_eq!(pending["capture"]["closed"], false);
        assert_eq!(pending["task_mining"]["continuation_requested"], true);
        assert!(pending["drafts"].as_array().unwrap().is_empty());

        // Codex re-enters UserPromptSubmit with the hook's reason. This is system
        // continuation, not another human statement or another mining ticket.
        let turn = if client == "codex" {
            assert_eq!(
                f.event("UserPromptSubmit", "continued", json!({"prompt":reason})),
                json!({})
            );
            "continued"
        } else {
            "one"
        };
        let bound = f.episode();
        assert_eq!(bound["episode"]["id"], pending["episode"]["id"]);
        let events = bound["episode"]["events"].as_array().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event["actor"] == "user")
                .count(),
            1
        );
        assert!(events
            .iter()
            .any(|event| event["kind"] == "MiningCheckpoint"
                && event["actor"] == "assistant"
                && event["text"] == "Reviewed."));
        if client == "codex" {
            assert!(events
                .iter()
                .any(|event| event["kind"] == "MiningContinuation"
                    && event["origin"] == "automation_or_agent"));
        }
        let generated = "Keep mining metadata out of the user-facing answer.";
        assert!(reason.contains(generated));
        assert!(failed(&f.submit(&ticket, f.candidates(generated), client)));

        f.event("PreToolUse", turn, json!({"tool_use_id":"mine","tool_name":"mcp__mmcg__mmcg_mining_submit","tool_input":{"ticket_id":ticket}}));
        assert!(!failed(&f.submit(&ticket, f.candidates(PROMPT), client)));
        f.event(
            "PostToolUse",
            turn,
            json!({"tool_use_id":"mine","tool_name":"mcp__mmcg__mmcg_mining_submit"}),
        );
        assert_eq!(
        f.event(
            "Stop",
            turn,
            json!({"last_assistant_message":"Reviewed.","stop_hook_active":true,"model":format!("{client}-task-model")})
        ),
        json!({})
    );
        let completed = f.episode();
        assert!(completed["episode"]["coverage_gaps"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(completed["task_mining"]["status"], "completed");
        assert_eq!(completed["task_mining"]["result"], "candidates");
        assert_eq!(completed["task_mining"]["candidate_count"], 1);
        let id = completed["drafts"][0]["id"].as_str().unwrap();
        let draft = f.success(&["miner", "hooks", "draft", id]);
        assert_eq!(draft["draft"]["content"]["supports"][0]["quote"], PROMPT);
        assert_eq!(draft["draft"]["attested"], false);
        assert_eq!(draft["current"], true);
        assert_eq!(draft["draft"]["processor"]["source_client"], client);
        assert_eq!(
            draft["draft"]["processor"]["model"],
            format!("{client}-task-model")
        );
        let list = f.success(&["miner", "hooks", "episodes"]);
        assert_eq!(list["episodes"].as_array().unwrap().len(), 1);
        let status = f.success(&["miner", "hooks", "status", "--client", client]);
        assert_eq!(
            status["readiness"]["mining"]["outcomes"]["with_candidates"],
            1
        );
        assert_eq!(status["readiness"]["mining"]["outcomes"]["unreported"], 0);
        f.no_inference();
    }
}

#[test]
fn empty_submission_is_an_auditable_no_signal_result_without_continuation() {
    let f = Fixture::new();
    let ticket = f.offer("one", "Inspect src/api.rs for a state bug.");
    assert!(!failed(&f.submit(&ticket, json!([]), "codex")));
    assert_eq!(
        f.event(
            "Stop",
            "one",
            json!({"last_assistant_message":"Inspected."})
        ),
        json!({})
    );
    let completed = f.episode();
    assert_eq!(completed["task_mining"]["result"], "no_signal");
    assert_eq!(completed["task_mining"]["candidate_count"], 0);
    assert_eq!(completed["task_mining"]["continuation_requested"], false);
    assert!(completed["drafts"].as_array().unwrap().is_empty());
    assert!(completed["analyses"]
        .as_array()
        .unwrap()
        .iter()
        .any(|analysis| analysis["current"] == true
            && analysis["processor"]["engine"] == "current_task_agent"));
    let status = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(status["readiness"]["mining"]["outcomes"]["no_signal"], 1);
    // Old completed tickets cleared their draft list without storing its
    // count. Reading them must not invent an empty model result.
    let db = rusqlite::Connection::open(f.home.join(".mastermind/persona-events.db")).unwrap();
    db.execute(
        "UPDATE hook_task_mining SET data=json_remove(data,'$.candidate_count') WHERE id=?1",
        [&ticket],
    )
    .unwrap();
    let legacy = f.success(&["miner", "hooks", "status", "--client", "codex"]);
    assert_eq!(
        legacy["readiness"]["mining"]["outcomes"]["legacy_completed"],
        1
    );
    assert_eq!(legacy["readiness"]["mining"]["outcomes"]["no_signal"], 0);
    f.no_inference();
}

#[test]
fn the_stop_retry_is_bounded_and_missing_submission_is_never_no_signal() {
    for client in ["codex", "claude"] {
        let f = Fixture::for_client(client);
        let ticket = f.offer("one", PROMPT);
        let first = f.event(
            "Stop",
            "one",
            json!({"last_assistant_message":"Reviewed.","stop_hook_active":false}),
        );
        assert_eq!(first["decision"], "block", "{client}");
        // Claude continues without a new UserPromptSubmit and can retain the
        // same turn identity. Native stop_hook_active prevents another retry.
        let turn = if client == "codex" {
            f.event(
                "UserPromptSubmit",
                "continued",
                json!({"prompt":first["reason"]}),
            );
            "continued"
        } else {
            "one"
        };
        let last = json!({"last_assistant_message":"Reviewed again.","stop_hook_active":client == "claude"});
        assert_eq!(f.event("Stop", turn, last.clone()), json!({}));
        assert_eq!(f.event("Stop", turn, last), json!({}));
        let skipped = f.episode();
        assert_eq!(skipped["task_mining"]["status"], "skipped");
        assert_eq!(
            skipped["task_mining"]["reason"],
            "submission_missing_after_retry"
        );
        assert!(skipped["task_mining"]["result"].is_null());
        assert!(skipped["episode"]["coverage_gaps"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(!skipped["analyses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|analysis| analysis["processor"]["engine"] == "current_task_agent"));
        assert!(failed(&f.submit(&ticket, f.candidates(PROMPT), client)));
        let status = f.success(&["miner", "hooks", "status", "--client", client]);
        assert_eq!(status["readiness"]["mining"]["outcomes"]["skipped"], 1);
        assert_eq!(status["readiness"]["mining"]["outcomes"]["no_signal"], 0);
        assert!(status["readiness"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning == "task_mining_has_missing_reports"));
        f.no_inference();
    }
}

#[test]
fn stop_never_requests_mining_from_incomplete_or_revoked_capture() {
    // A gap first observed in this Stop has not yet been saved in the session
    // snapshot. The guard must also check the in-flight capture state.
    let fork = Fixture::new();
    let fork_ticket = fork.offer("one", PROMPT);
    assert_eq!(
        fork.event(
            "Stop",
            "one",
            json!({"parent_session_id":"parent","last_assistant_message":"Reviewed."})
        ),
        json!({})
    );
    assert_eq!(
        fork.episode()["task_mining"]["reason"],
        "capture_incomplete"
    );
    assert!(failed(&fork.submit(&fork_ticket, json!([]), "codex")));
    fork.no_inference();

    let f = Fixture::new();
    let ticket = f.offer("one", PROMPT);
    f.event("PreCompact", "one", json!({}));
    assert_eq!(
        f.event("Stop", "one", json!({"last_assistant_message":"Reviewed."})),
        json!({})
    );
    let skipped = f.episode();
    assert_eq!(skipped["task_mining"]["status"], "skipped");
    assert_eq!(skipped["task_mining"]["reason"], "capture_incomplete");
    assert!(failed(&f.submit(&ticket, json!([]), "codex")));
    let next = f.offer("two", PROMPT);
    let mut settings = mmcg::onboarding::load(&f.root).unwrap().unwrap();
    settings.mining = mmcg::onboarding::Mining::Capture;
    mmcg::onboarding::Session::begin(&f.root)
        .unwrap()
        .save(&settings)
        .unwrap();
    assert_eq!(
        f.event("Stop", "two", json!({"last_assistant_message":"Reviewed."})),
        json!({})
    );
    assert!(failed(&f.submit(&next, json!([]), "codex")));
    f.no_inference();
}
