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
