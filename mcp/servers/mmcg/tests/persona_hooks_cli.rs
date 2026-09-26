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
        let list = self.success(&["miner", "hooks", "episodes"]);
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
fn offered_profile_is_recorded_before_native_context_and_blocks_own_echo_mining() {
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
    let p = f.processor(&ep, QUOTE);
    assert!(!f
        .run(&[
            "miner",
            "hooks",
            "analyze",
            ep["id"].as_str().unwrap(),
            "--revision",
            ep["revision"].as_str().unwrap(),
            "--processor",
            p.to_str().unwrap()
        ])
        .status
        .success());
}

#[test]
fn ordinary_mcp_profile_tool_read_marks_following_echo_as_influenced() {
    let f = Fixture::new();
    f.event("s", "", "SessionStart", json!({"source":"startup"}));
    f.event("s", "t1", "UserPromptSubmit", json!({"prompt":QUOTE}));
    f.event(
        "s",
        "t1",
        "PreToolUse",
        json!({"tool_name":"mcp__mmcg__mmcg_profile","tool_use_id":"profile-1","tool_input":{}}),
    );
    f.event("s","t1","PostToolUse",json!({"tool_name":"mcp__mmcg__mmcg_profile","tool_use_id":"profile-1","tool_response":{"status":"ok","profile_revision":"some-revision"}}));
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
    let p = f.processor(&input, QUOTE);
    assert!(!f
        .run(&[
            "miner",
            "hooks",
            "analyze",
            input["id"].as_str().unwrap(),
            "--revision",
            input["revision"].as_str().unwrap(),
            "--processor",
            p.to_str().unwrap()
        ])
        .status
        .success());
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
        if f.success(&["miner", "hooks", "status", "--client", "codex"])["grant"]["pending"] == 1 {
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

struct ForegroundWorker(std::process::Child);

impl ForegroundWorker {
    fn start(f: &Fixture, processor: &std::path::Path) -> Self {
        Self(
            f.command()
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
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn stop(&mut self, signal: libc::c_int) {
        // SAFETY: this test owns the live worker child.
        assert_eq!(unsafe { libc::kill(self.0.id() as libc::pid_t, signal) }, 0);
        wait_until("foreground worker cancellation", || {
            self.0.try_wait().unwrap().is_some()
        });
        assert!(self.0.wait().unwrap().success());
    }
}

impl Drop for ForegroundWorker {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            // SAFETY: the un-reaped child is owned by this fixture. Let its
            // normal cancellation path clean up its processor process group.
            unsafe {
                libc::kill(self.0.id() as libc::pid_t, libc::SIGTERM);
            }
            for _ in 0..100 {
                if self.0.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
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
fn foreground_worker_cancellation_terminates_provider_and_releases_its_lease() {
    let f = Fixture::new();
    f.capture("cancel-session", "cancel-turn");
    let processor = f._temp.path().join("slow-processor");
    let pid_file = f._temp.path().join("processor.pid");
    fs::write(
        &processor,
        format!(
            "#!/bin/sh\ncat >/dev/null\necho $$ > '{}'\nsleep 30\n",
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
