//! Context previews through the real CLI and MCP, using only synthetic homes,
//! repositories, transcripts and historical task records.

#[cfg(unix)]
use mmcg::miner::store::ProfileStore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

#[cfg(unix)]
const QUOTE: &str = "I prefer short replies with test results.";
#[cfg(unix)]
const PREFERENCE: &str = "Keep executor replies brief and include test results";
#[cfg(unix)]
const PRIVATE_NOTE: &str = "PRIVATE_CONTEXT_PROFILE_NOTE_DO_NOT_SERVE";
#[cfg(unix)]
const HOOK_QUOTE: &str =
    "Before changing a public API, inspect its callers and preserve the contract.";
#[cfg(unix)]
const HOOK_BEHAVIOR: &str = "Inspects callers before changing a public API contract";

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    codex: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let codex = home.join("codex");
        let root = temp.path().join("project");
        for dir in [&home, &codex, &root] {
            fs::create_dir_all(dir).unwrap();
        }
        let fixture = Self {
            home: home.canonicalize().unwrap(),
            codex: codex.canonicalize().unwrap(),
            root: root.canonicalize().unwrap(),
            _temp: temp,
        };
        fixture.write(".gitignore", ".mastermind/\n");
        fixture.write("src/lib.rs", "pub fn transport_ready() -> bool { false }\n");
        fixture.write(
            "CONTEXT.md",
            "# Project\n\n## Transport\nThe transport preserves the original request identifier.\n\n## Decision log\n### Transport ownership\n- **Decision:** Keep transport state in the owning service.\n- **Status:** active\n",
        );
        fixture.write(
            "docs/transport.md",
            "# Transport\n\nThe transport preserves the literal field \"request_id\" and reports failed delivery.\n",
        );
        fixture.git(&["init", "-q"]);
        fixture.git(&["add", ".gitignore", "src", "docs", "CONTEXT.md"]);
        fixture.git(&["commit", "-qm", "Synthetic context baseline"]);
        fixture.write("src/lib.rs", "pub fn transport_ready() -> bool { true }\n");
        fixture
    }

    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command.current_dir(&self.root);
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("GIT_") || name.starts_with("MMCG_") {
                command.env_remove(key);
            }
        }
        command
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", &self.codex)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        command
    }

    fn git(&self, args: &[&str]) {
        let output = self
            .command("git")
            .args([
                "-c",
                "core.hooksPath=",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Context Fixture",
                "-c",
                "user.email=context@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert_success(&output);
    }

    fn success(&self, args: &[&str]) -> Output {
        let output = self
            .command(env!("CARGO_BIN_EXE_mmcg"))
            .args(args)
            .output()
            .unwrap();
        assert_success(&output);
        output
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn index(&self) {
        self.success(&["index", "."]);
    }

    fn context_command(
        &self,
        role: &str,
        workflow: &str,
        budget: usize,
        client: Option<&str>,
    ) -> Command {
        let mut command = self.command(env!("CARGO_BIN_EXE_mmcg"));
        command
            .args(["context", "preview", "--root"])
            .arg(&self.root)
            .args([
                "--since",
                "HEAD",
                "--role",
                role,
                "--workflow",
                workflow,
                "--path",
                "./src//lib.rs",
                "--path",
                "src/lib.rs",
                "--query",
                "transport",
                "--budget-tokens",
            ])
            .arg(budget.to_string());
        if let Some(client) = client {
            command.args(["--profile-client", client]);
        }
        command
    }

    fn context(&self, role: &str, workflow: &str, budget: usize, client: Option<&str>) -> Value {
        let output = self
            .context_command(role, workflow, budget, client)
            .output()
            .unwrap();
        assert_success(&output);
        let wire = std::str::from_utf8(&output.stdout).unwrap();
        assert_eq!(
            wire.lines().count(),
            1,
            "CLI must emit one compact JSON packet"
        );
        checked_packet(wire.strip_suffix('\n').unwrap_or(wire), budget)
    }

    #[cfg(unix)]
    fn context_at(&self, root: &Path, client: &str) -> Value {
        let output = self
            .command(env!("CARGO_BIN_EXE_mmcg"))
            .current_dir(root)
            .args(["context", "preview", "--root"])
            .arg(root)
            .args([
                "--role",
                "executor",
                "--since",
                "HEAD",
                "--workflow",
                "strict",
                "--budget-tokens",
                "8000",
                "--profile-client",
                client,
            ])
            .output()
            .unwrap();
        assert_success(&output);
        let wire = std::str::from_utf8(&output.stdout).unwrap().trim_end();
        checked_packet(wire, 8000)
    }

    #[cfg(unix)]
    fn hook_event(
        &self,
        root: &Path,
        session: &str,
        turn: &str,
        kind: &str,
        extra: Value,
    ) -> Value {
        let mut event = json!({
            "session_id":session,"turn_id":turn,"hook_event_name":kind,"cwd":root,
        });
        for (key, value) in extra.as_object().unwrap() {
            event[key] = value.clone();
        }
        let mut child = self
            .command(env!("CARGO_BIN_EXE_mmcg"))
            .current_dir(root)
            .env_remove("MASTERMIND_MINER")
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
        let output = child.wait_with_output().unwrap();
        assert_success(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    #[cfg(unix)]
    fn hook_draft(&self, root: &Path, session: &str) -> Value {
        use std::os::unix::fs::PermissionsExt;

        let output = self
            .command(env!("CARGO_BIN_EXE_mmcg"))
            .current_dir(root)
            .args(["miner", "hooks", "setup", "--client", "codex", "--write"])
            .output()
            .unwrap();
        assert_success(&output);
        self.hook_event(
            root,
            session,
            "one",
            "SessionStart",
            json!({"source":"startup"}),
        );
        self.hook_event(
            root,
            session,
            "one",
            "UserPromptSubmit",
            json!({"prompt":HOOK_QUOTE}),
        );
        self.hook_event(
            root,
            session,
            "one",
            "Stop",
            json!({
                "last_assistant_message":"PRIVATE_HOOK_ASSISTANT_RESPONSE"
            }),
        );
        let output = self
            .command(env!("CARGO_BIN_EXE_mmcg"))
            .current_dir(root)
            .args(["miner", "hooks", "episodes"])
            .output()
            .unwrap();
        assert_success(&output);
        let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(listed["episodes"].as_array().unwrap().len(), 1);
        let output = self.success(&[
            "miner",
            "hooks",
            "show",
            listed["episodes"][0]["id"].as_str().unwrap(),
        ]);
        let shown: Value = serde_json::from_slice(&output.stdout).unwrap();
        let episode = &shown["episode"];
        let event = episode["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["kind"] == "UserPromptSubmit")
            .unwrap();
        let response = json!({
            "schema":1,"episode_id":episode["id"],"episode_revision":episode["revision"],
            "drafts":[{"when":"When changing a public API", "behavior":HOOK_BEHAVIOR,
                "rationale":null,"outcome":null,"exception":"No exception was observed.",
                "role":null,"workflow":null,"evidence_kind":"technical_approach",
                "supports":[{"event_id":event["id"],"quote":HOOK_QUOTE}],"contradictions":[]}]
        });
        let processor = self._temp.path().join(format!("processor-{session}"));
        fs::write(&processor, format!(
            "#!/bin/sh\n[ \"$MASTERMIND_MINER\" = 1 ] || exit 7\ncat >/dev/null\ncat <<'RESPONSE'\n{response}\nRESPONSE\n"
        )).unwrap();
        fs::set_permissions(&processor, fs::Permissions::from_mode(0o700)).unwrap();
        let output = self.success(&[
            "miner",
            "hooks",
            "analyze",
            episode["id"].as_str().unwrap(),
            "--revision",
            episode["revision"].as_str().unwrap(),
            "--processor",
            processor.to_str().unwrap(),
        ]);
        let analyzed: Value = serde_json::from_slice(&output.stdout).unwrap();
        analyzed["drafts"][0].clone()
    }

    fn mcp(&self, client: Option<&str>, arguments: &[Value]) -> Vec<Value> {
        let mut command = self.command(env!("CARGO_BIN_EXE_mmcg"));
        if let Some(client) = client {
            command.env("MMCG_PROFILE_CLIENT", client);
        }
        let mut child = command
            .args(["serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"context-fixture","version":"1"}}
        })).unwrap();
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .unwrap();
        for (index, args) in arguments.iter().enumerate() {
            writeln!(
                input,
                "{}",
                json!({"jsonrpc":"2.0","id":index+1,"method":"tools/call",
                "params":{"name":"mmcg_context","arguments":args}})
            )
            .unwrap();
        }
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert_success(&output);
        let responses: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        (1..=arguments.len())
            .map(|id| {
                responses
                    .iter()
                    .find(|response| response["id"] == id)
                    .unwrap_or_else(|| panic!("missing response {id}: {responses:?}"))
                    .clone()
            })
            .collect()
    }

    fn complete_task(&self) {
        let preview = self.context("executor", "strict", 8000, None);
        let spec = ".mastermind/tasks/001-context/spec.md";
        self.write(spec, "# Synthetic completed task\n");
        self.write(".mastermind/tasks/001-context/history-review.md",
            &format!("- **Audit snapshot:** {}\n- **Context:** not applicable\n- **Lesson:** not applicable\n- **Reason:** synthetic historical review fixture\n", "a".repeat(64)));
        self.write(
            ".mastermind/tasks/001-context/state.json",
            &json!({
                "status":"learned", "risk":"low", "next_step":"close", "last_artifact":"audit.md",
                "spec_path":spec, "repository_identity":preview["repository_identity"],
                "spec_hash":sha(b"# Synthetic completed task\n"), "baseline_ref":"0".repeat(40),
                "history_snapshot_sha256":"a".repeat(64), "started_at":1, "iteration":1
            })
            .to_string(),
        );
    }

    #[cfg(unix)]
    fn reviewed_preference(&self) -> (PathBuf, String) {
        let transcript = self.codex.join("sessions/2026/09/26/rollout-context.jsonl");
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        let records = [
            json!({"type":"session_meta","payload":{"id":"context-private-session","cwd":self.root,
                "source":"vscode","thread_source":"user"}}),
            json!({"type":"turn_context","payload":{"turn_id":"context-turn","cwd":self.root}}),
            json!({"type":"response_item","timestamp":"2026-09-26T10:00:00Z","payload":{
                "type":"message","role":"user","content":[{"type":"input_text","text":QUOTE}],
                "internal_chat_message_metadata_passthrough":{"turn_id":"context-turn","content_item_kinds":["user.text"]}}}),
        ];
        let body = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&transcript, body).unwrap();
        self.success(&[
            "miner",
            "collect",
            "--transcript",
            transcript.to_str().unwrap(),
        ]);
        let inbox: Value =
            serde_json::from_slice(&self.success(&["miner", "candidates", "list"]).stdout).unwrap();
        let candidate = &inbox["candidates"][0]["candidate"];
        self.success(&[
            "miner",
            "candidates",
            "propose-preference",
            candidate["id"].as_str().unwrap(),
            "--revision",
            candidate["revision"].as_str().unwrap(),
            "--statement",
            PREFERENCE,
            "--category",
            "communication",
            "--scope",
            "role:executor",
        ]);
        let store = ProfileStore::open_read_only(&self.home.join(".mastermind/style.db")).unwrap();
        let entry = store.feedback().unwrap().remove(0);
        drop(store);
        self.accept_in_terminal(&entry.key, &entry.review_revision());
        self.success(&["miner", "access", "grant", "--client", "context-test"]);
        fs::write(
            self.home.join(".mastermind/style.md"),
            format!("{PRIVATE_NOTE}\n{QUOTE}\n{PREFERENCE}\n"),
        )
        .unwrap();
        (transcript, entry.key)
    }

    #[cfg(unix)]
    fn accept_in_terminal(&self, key: &str, revision: &str) {
        use std::os::fd::FromRawFd;
        let (mut master, mut slave) = (-1, -1);
        // Use the real interactive gate, with all reads and writes confined to
        // this synthetic home. Keeping the master open retains the terminal.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, 0);
        let _master = unsafe { fs::File::from_raw_fd(master) };
        let slave = unsafe { fs::File::from_raw_fd(slave) };
        let output = self
            .command(env!("CARGO_BIN_EXE_mmcg"))
            .args(["miner", "feedback", "accept", key, "--revision", revision])
            .stdin(Stdio::from(slave))
            .output()
            .unwrap();
        assert_success(&output);
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn checked_packet(wire: &str, budget: usize) -> Value {
    let packet: Value = serde_json::from_str(wire).unwrap();
    // Parse for structure, but hash the producer's bytes. FTS f64 scores must
    // not pass through this test's different parse/serialize rounding first.
    let (mut quoted, mut escaped) = (false, false);
    for byte in wire.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else {
            assert!(!byte.is_ascii_whitespace(), "JSON must be compact");
        }
    }
    assert!(
        wire.len() <= budget * 4,
        "{} bytes exceeds budget {budget}",
        wire.len()
    );
    assert_eq!(packet["budget"]["serialized_bytes"], wire.len());
    assert_eq!(packet["budget"]["estimated_tokens"], wire.len().div_ceil(4));
    let revision = packet["context_revision"].as_str().unwrap();
    let member = format!("\"context_revision\":\"{revision}\",");
    assert_eq!(wire.matches(&member).count(), 1);
    assert_eq!(revision, sha(wire.replacen(&member, "", 1).as_bytes()));
    for (name, layer) in packet["layers"].as_object().unwrap() {
        if !layer["data"].is_null() {
            let prefix = format!("\"{name}\":{{\"data\":");
            assert_eq!(wire.matches(&prefix).count(), 1);
            let raw_data = object_span(wire, wire.find(&prefix).unwrap() + prefix.len());
            assert_eq!(layer["revision"], sha(raw_data.as_bytes()));
        } else if layer["omitted_reason"] == "context_budget" {
            assert_eq!(layer["revision"].as_str().unwrap().len(), 64);
            assert!(
                layer["verification"].is_object(),
                "budget omissions retain verification"
            );
        }
    }
    packet
}

/// The packet is already valid JSON. Locate an object without decoding its
/// numbers or interpreting braces and escaped quotes inside string values.
fn object_span(wire: &str, start: usize) -> &str {
    assert_eq!(wire.as_bytes()[start], b'{');
    let (mut depth, mut quoted, mut escaped) = (0, false, false);
    for (offset, byte) in wire.as_bytes()[start..].iter().copied().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &wire[start..=start + offset];
                    }
                }
                _ => {}
            }
        }
    }
    panic!("unterminated layer object")
}

fn args(role: &str, workflow: &str, budget: usize) -> Value {
    json!({"since":"HEAD","role":role,"workflow":workflow,
        "paths":["./src//lib.rs","src/lib.rs"],"query":"transport","budget_tokens":budget})
}

fn mcp_packet(response: &Value, budget: usize) -> Value {
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["isError"], true, "{response}");
    checked_packet(
        response["result"]["content"][0]["text"].as_str().unwrap(),
        budget,
    )
}

#[cfg(unix)]
fn assert_private(packet: &Value, transcript: &Path) {
    let wire = packet.to_string();
    for private in [
        QUOTE,
        PRIVATE_NOTE,
        "context-private-session",
        transcript.to_str().unwrap(),
    ] {
        assert!(
            !wire.contains(private),
            "private source data exposed: {private}"
        );
    }
}

#[test]
fn context_cli_and_mcp_repeat_exact_hashes_and_bound_escaped_json() {
    let f = Fixture::new();
    f.complete_task();
    f.index();
    let first = f.context("executor", "strict", 8000, None);
    assert_eq!(first["kind"], "context_preview");
    assert_eq!(first["delivery"], "not_recorded");
    assert_eq!(first["permission_effect"], "none");
    assert_eq!(first["selection"]["paths"], json!(["src/lib.rs"]));
    assert_eq!(first["layers"]["code"]["status"], "ok");
    assert_eq!(first["layers"]["project"]["data"]["freshness"], "fresh");
    assert_eq!(
        first["layers"]["documentation"]["data"]["freshness"],
        "fresh"
    );
    assert_eq!(first["layers"]["person"]["status"], "not_enabled");
    let work = &first["layers"]["work"]["data"];
    assert_eq!(work["tasks"][0]["phase"], "complete");
    assert_eq!(work["tasks"][0]["completion_basis"], "historical_record");
    assert_eq!(work["tasks"][0]["current_checkout"], "not_verified");
    assert_eq!(work["current_checkout"], "not_verified");
    assert_eq!(first, f.context("executor", "strict", 8000, None));
    let requests = [
        args("executor", "strict", 8000),
        args("executor", "strict", 8000),
    ];
    let responses = f.mcp(None, &requests);
    for response in responses {
        assert_eq!(first, mcp_packet(&response, 8000));
    }
    for workflow in ["\\\"".repeat(64), "🦀".repeat(32)] {
        let bounded = f.context("executor", &workflow, 1024, None);
        assert!(!bounded["omitted"].as_array().unwrap().is_empty());
        for (name, layer) in bounded["layers"].as_object().unwrap() {
            if layer["omitted_reason"] == "context_budget" {
                assert_eq!(
                    layer["verification"], first["layers"][name]["verification"],
                    "omitting {name} must retain its original verification"
                );
            }
        }
        let from_mcp = f.mcp(None, &[args("executor", &workflow, 1024)]);
        assert_eq!(bounded, mcp_packet(&from_mcp[0], 1024));
    }
    f.write(
        "src/lib.rs",
        "pub fn transport_ready() -> bool { panic!(\"changed after completion\") }\n",
    );
    let changed = f.context("executor", "strict", 8000, None);
    assert_ne!(changed["layers"]["code"]["status"], "ok");
    assert_eq!(
        changed["layers"]["work"]["data"]["tasks"][0]["phase"],
        "complete"
    );
    assert_eq!(
        changed["layers"]["work"]["data"]["current_checkout"],
        "not_verified"
    );
}

#[test]
fn context_withholds_changed_markdown_until_explicit_reindex() {
    let f = Fixture::new();
    f.index();
    let first = f.context("executor", "strict", 8000, None);
    assert!(first["layers"]["project"]["data"]
        .to_string()
        .contains("original request identifier"));
    let index = f.root.join(".mastermind/mmcg.db");
    let before = fs::read(&index).unwrap();
    f.write(
        "CONTEXT.md",
        "# Project\n\n## Transport\nThe transport requires a revised version identifier.\n",
    );
    let stale = f.context("executor", "strict", 8000, None);
    for layer in ["project", "documentation"] {
        assert_eq!(stale["layers"][layer]["status"], "index_not_fresh");
        assert!(stale["layers"][layer]["data"]["observed"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    assert!(stale["layers"]["project"]["data"]["claim_candidates"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!stale.to_string().contains("original request identifier"));
    assert!(!stale.to_string().contains("revised version identifier"));
    assert_eq!(
        fs::read(&index).unwrap(),
        before,
        "a preview never reindexes"
    );
    let responses = f.mcp(None, &[args("executor", "strict", 8000)]);
    let via_mcp = mcp_packet(&responses[0], 8000);
    assert_eq!(via_mcp["layers"]["project"]["status"], "index_not_fresh");
    assert!(!via_mcp.to_string().contains("original request identifier"));
    f.index();
    let fresh = f.context("executor", "strict", 8000, None);
    assert_eq!(fresh["layers"]["project"]["data"]["freshness"], "fresh");
    assert!(fresh["layers"]["project"]["data"]
        .to_string()
        .contains("revised version identifier"));
    assert_ne!(first["context_revision"], fresh["context_revision"]);
}

#[test]
fn context_without_profile_access_creates_no_personal_store_or_index() {
    let f = Fixture::new();
    for client in [None, Some("context-test")] {
        let packet = f.context("executor", "strict", 8000, client);
        assert_eq!(
            packet["layers"]["person"]["status"],
            if client.is_some() {
                "access_denied"
            } else {
                "not_enabled"
            }
        );
        assert!(!f.home.join(".mastermind").exists());
        assert!(!f.root.join(".mastermind/mmcg.db").exists());
    }
    f.index();
    let responses = f.mcp(Some("context-test"), &[args("executor", "strict", 8000)]);
    assert_eq!(
        mcp_packet(&responses[0], 8000)["layers"]["person"]["status"],
        "access_denied"
    );
    assert!(!f.home.join(".mastermind").exists());
}

#[test]
#[cfg(unix)]
fn context_person_layer_requires_configured_audience_scope_and_current_source() {
    let f = Fixture::new();
    f.index();
    let (transcript, key) = f.reviewed_preference();
    let granted = f.context("executor", "strict", 8000, Some("context-test"));
    assert_eq!(
        granted["layers"]["person"]["data"]["feedback"][0]["key"],
        key
    );
    assert_private(&granted, &transcript);
    let auditor = f.context("auditor", "strict", 8000, Some("context-test"));
    assert!(auditor["layers"]["person"]["data"]["feedback"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!auditor.to_string().contains(PREFERENCE));
    let disabled = f
        .context_command("executor", "strict", 8000, None)
        .env("MMCG_PROFILE_CLIENT", "context-test")
        .output()
        .unwrap();
    assert_success(&disabled);
    let disabled: Value = serde_json::from_slice(&disabled.stdout).unwrap();
    assert_eq!(disabled["layers"]["person"]["status"], "not_enabled");
    assert!(!disabled.to_string().contains(PREFERENCE));
    assert_private(&disabled, &transcript);
    let denied = f.context("executor", "strict", 8000, Some("ungranted-client"));
    assert_eq!(denied["layers"]["person"]["status"], "access_denied");
    assert!(!denied.to_string().contains(PREFERENCE));
    assert_private(&denied, &transcript);

    let responses = f.mcp(Some("context-test"), &[args("executor", "strict", 8000)]);
    let served = mcp_packet(&responses[0], 8000);
    assert_eq!(
        served["layers"]["person"]["data"]["feedback"][0]["key"],
        key
    );
    assert_private(&served, &transcript);
    for (field, value) in [
        ("root", json!(f.root)),
        ("profile_client", json!("context-test")),
    ] {
        let mut forged = args("executor", "strict", 8000);
        forged[field] = value;
        let response = f.mcp(None, &[forged]).remove(0);
        assert!(
            response.get("error").is_some() || response["result"]["isError"] == true,
            "{response}"
        );
        assert!(!response.to_string().contains(PREFERENCE));
    }
    fs::remove_file(&transcript).unwrap();
    let stale = f.context("executor", "strict", 8000, Some("context-test"));
    assert!(stale["layers"]["person"]["data"]["feedback"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        stale["layers"]["person"]["data"]["source_verification"],
        "incomplete"
    );
    assert_eq!(
        stale["layers"]["person"]["verification"]["source_verification"],
        "incomplete"
    );
    assert_private(&stale, &transcript);
    assert!(
        !stale.to_string().contains(PREFERENCE),
        "no static style.md fallback"
    );
}

#[test]
#[cfg(unix)]
fn context_review_queue_requires_exact_root_access_and_withholds_candidate_content() {
    let f = Fixture::new();
    let draft = f.hook_draft(&f.root, "private-queue-session");
    let other = f._temp.path().join("other-project");
    fs::create_dir(&other).unwrap();
    assert_success(
        &f.command("git")
            .current_dir(&other)
            .args(["init", "-q"])
            .output()
            .unwrap(),
    );
    let other = other.canonicalize().unwrap();
    let other_draft = f.hook_draft(&other, "private-other-session");

    let denied = f.context_at(&f.root, "context-test");
    assert_eq!(denied["layers"]["person"]["status"], "access_denied");
    assert!(!denied.to_string().contains(draft["id"].as_str().unwrap()));
    f.success(&["miner", "access", "grant", "--client", "context-test"]);
    let granted = f.context_at(&f.root, "context-test");
    let person = &granted["layers"]["person"]["data"];
    assert_eq!(
        person["status"], "insufficient_evidence",
        "a draft is not a reviewed profile claim"
    );
    let queue = &person["review_queue"];
    assert_eq!(
        queue["status"], "observed",
        "the first draft must be visible before profile publication"
    );
    assert_eq!(queue["total"], 1);
    assert_eq!(queue["returned"], 1);
    assert_eq!(queue["truncated"], false);
    assert_eq!(queue["authority"], "unreviewed_observations_only");
    let item = &queue["items"][0];
    assert_eq!(item["id"], draft["id"]);
    assert_eq!(item["source_id"], draft["episode"]);
    assert_eq!(item["episode_id"], draft["episode"]);
    assert_eq!(item["source_revision"], draft["episode_revision"]);
    assert_eq!(item["source_status"], "current");
    assert_eq!(item["evidence_class"], "no_recorded_prior_exposure");
    assert_eq!(item["promotion_eligible"], true);
    assert_eq!(item["status"], "authorship_review_required");
    assert_eq!(item["support_count"], 1);
    assert_eq!(item["contradiction_count"], 0);
    let keys = item
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            "contradiction_count",
            "episode_id",
            "evidence_class",
            "id",
            "kind",
            "promotion_eligible",
            "source_id",
            "source_revision",
            "source_status",
            "status",
            "support_count"
        ]
    );
    for private in [
        HOOK_QUOTE,
        HOOK_BEHAVIOR,
        "PRIVATE_HOOK_ASSISTANT_RESPONSE",
        "private-queue-session",
        "private-other-session",
        other_draft["id"].as_str().unwrap(),
    ] {
        assert!(
            !granted.to_string().contains(private),
            "preview leaked {private}"
        );
    }
    let via_mcp = f.mcp(Some("context-test"), &[args("executor", "strict", 8000)]);
    assert_eq!(
        mcp_packet(&via_mcp[0], 8000)["layers"]["person"]["data"]["review_queue"],
        *queue
    );

    // The same audience has no implicit access to a second repository, even
    // though both capture journals live in this synthetic user's global DB.
    assert_eq!(
        f.context_at(&other, "context-test")["layers"]["person"]["status"],
        "access_denied"
    );
    f.success(&[
        "miner",
        "access",
        "grant",
        other.to_str().unwrap(),
        "--client",
        "context-test",
    ]);
    let other_packet = f.context_at(&other, "context-test");
    assert_eq!(
        other_packet["layers"]["person"]["data"]["review_queue"]["items"][0]["id"],
        other_draft["id"]
    );
    assert!(!other_packet
        .to_string()
        .contains(draft["id"].as_str().unwrap()));
    f.success(&["miner", "access", "revoke", "--client", "context-test"]);
    let revoked = f.context_at(&f.root, "context-test");
    assert_eq!(revoked["layers"]["person"]["status"], "access_denied");
    assert!(revoked["layers"]["person"]["data"]
        .get("review_queue")
        .is_none());
    assert!(!revoked.to_string().contains(draft["id"].as_str().unwrap()));
    let revoked_mcp = f.mcp(Some("context-test"), &[args("executor", "strict", 8000)]);
    assert_eq!(
        mcp_packet(&revoked_mcp[0], 8000)["layers"]["person"]["status"],
        "access_denied"
    );
    assert_eq!(
        f.context_at(&other, "context-test")["layers"]["person"]["data"]["review_queue"]
            ["returned"],
        1
    );
}

#[test]
#[cfg(unix)]
fn context_review_queue_preserves_stale_and_malformed_source_boundaries() {
    let f = Fixture::new();
    let draft = f.hook_draft(&f.root, "queue-source-session");
    f.success(&["miner", "access", "grant", "--client", "context-test"]);
    let initial = f.context_at(&f.root, "context-test");
    assert_eq!(
        initial["layers"]["person"]["data"]["review_queue"]["items"][0]["source_status"],
        "current"
    );
    f.hook_event(
        &f.root,
        "queue-source-session",
        "two",
        "UserPromptSubmit",
        json!({
            "prompt":"The previous request applies only to public API changes."
        }),
    );
    let stale = f.context_at(&f.root, "context-test");
    let item = &stale["layers"]["person"]["data"]["review_queue"]["items"][0];
    assert_eq!(item["id"], draft["id"]);
    assert_eq!(item["source_status"], "stale_or_unavailable");
    assert_eq!(item["promotion_eligible"], false);
    assert!(item["evidence_class"].is_null());

    // Corrupt an owner-writable source fixture after its real admission. Keep
    // the repository discriminator intact so the read must reject the source,
    // rather than hiding it through a different SQL selection.
    let db_path = f.home.join(".mastermind/persona-events.db");
    let db = rusqlite::Connection::open(&db_path).unwrap();
    assert_eq!(
        db.execute(
            "UPDATE hook_episode SET data=json_set(data,'$.events',?2) WHERE id=?1",
            rusqlite::params![
                draft["episode"].as_str().unwrap(),
                "PRIVATE_MALFORMED_EVENT_BODY"
            ],
        )
        .unwrap(),
        1
    );
    drop(db);
    let before = fs::read(&db_path).unwrap();
    let malformed = f.context_at(&f.root, "context-test");
    let queue = &malformed["layers"]["person"]["data"]["review_queue"];
    assert_eq!(queue["status"], "observed");
    assert_eq!(queue["items"][0]["id"], draft["id"]);
    assert_eq!(queue["items"][0]["source_status"], "stale_or_unavailable");
    assert_eq!(queue["items"][0]["promotion_eligible"], false);
    assert!(queue["items"][0]["evidence_class"].is_null());
    for private in [HOOK_QUOTE, HOOK_BEHAVIOR, "PRIVATE_MALFORMED_EVENT_BODY"] {
        assert!(!malformed.to_string().contains(private));
    }
    assert_eq!(
        fs::read(db_path).unwrap(),
        before,
        "preview must not repair or publish journal data"
    );
}

#[test]
fn context_delivery_revalidates_selected_document_evidence_before_native_use() {
    use mmcg::context::{from_paths, validate_delivery, ContextOptions};

    let f = Fixture::new();
    f.index();
    let index = f.root.join(".mastermind/mmcg.db");
    let options = ContextOptions {
        since: "HEAD".into(),
        paths: vec!["src/lib.rs".into()],
        role: mmcg::queries::BriefRole::Executor,
        workflow: Some("strict".into()),
        query: Some("transport".into()),
        budget_tokens: 8000,
    };
    let offered = from_paths(&f.root, &index, &options, None).unwrap();
    assert_eq!(
        offered["layers"]["documentation"]["data"]["freshness"],
        "fresh"
    );
    assert!(!offered["layers"]["documentation"]["data"]["observed"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        validate_delivery(&f.root, &index, &options, None, &offered),
        Ok(())
    );

    f.write(
        "docs/transport.md",
        "# Transport\n\nThe transport now also checks the delivery revision.\n",
    );
    assert_eq!(
        validate_delivery(&f.root, &index, &options, None, &offered),
        Err("invocation_context_sources_changed")
    );
    f.index();
    assert_eq!(
        validate_delivery(&f.root, &index, &options, None, &offered),
        Err("invocation_context_sources_changed"),
        "reindexing cannot make an already selected old document current"
    );
    let refreshed = from_paths(&f.root, &index, &options, None).unwrap();
    assert_ne!(
        refreshed["layers"]["documentation"]["revision"],
        offered["layers"]["documentation"]["revision"]
    );
    assert_eq!(
        validate_delivery(&f.root, &index, &options, None, &refreshed),
        Ok(())
    );

    // Successful validation of a preview without source payloads must retain
    // that absence. It is not evidence that unselected documents are current.
    let absent_index = f.root.join(".mastermind/not-created.db");
    let omitted = from_paths(&f.root, &absent_index, &options, None).unwrap();
    for layer in ["project", "documentation", "code"] {
        assert_eq!(omitted["layers"][layer]["status"], "unavailable");
        assert!(omitted["layers"][layer]["data"].is_null());
        assert!(omitted["layers"][layer]["revision"].is_null());
    }
    f.write(
        "docs/transport.md",
        "# Transport\n\nA change outside the omitted source selection.\n",
    );
    assert_eq!(
        validate_delivery(&f.root, &absent_index, &options, None, &omitted),
        Ok(())
    );
    assert_eq!(omitted["delivery"], "not_recorded");
    assert_eq!(omitted["permission_effect"], "none");
    assert!(
        !absent_index.exists(),
        "delivery validation must not create an index"
    );
}
