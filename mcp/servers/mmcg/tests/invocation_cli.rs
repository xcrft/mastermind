//! Native invocation through the real controller, with an external fake client.
//! All homes, profiles, executable probes and model streams are synthetic.

#![cfg(unix)]

use mmcg::miner::store::ProfileStore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const SPEC: &str = ".mastermind/tasks/001-invocation/spec.md";
const REPORT: &str = ".mastermind/tasks/001-invocation/executor-report.md";
const STATE: &str = ".mastermind/tasks/001-invocation/state.json";
const RECEIPT: &str = ".mastermind/tasks/001-invocation/invocation.json";
const QUOTE: &str = "I prefer short replies with test results.";
const PREFERENCE: &str =
    "PRIVATE_INVOCATION_PREFERENCE: keep executor replies brief and include test results";
const PROBE: &str = "#!/bin/sh\n/usr/bin/grep -q 'return 2' service.py || exit 9\nprintf 'PRIVATE_VERIFICATION_OUTPUT\\n'\n";
const FAKE: &str = r##"#!/bin/sh
set -eu
mode=$(/bin/cat "$HARNESS/mode")
case "${1-}" in
  --version)
    if test "$mode" = old_version; then printf '2.1.100 (Claude Code)\n'; else printf '2.1.267 (Claude Code)\n'; fi
    exit 0 ;;
  --help)
    if test "$mode" = auto_hidden_dependency && test -f "$HARNESS/calls"; then
      printf 'changed during second preparation\n' > hidden-input.txt
    fi
    if test "$mode" = missing_capability; then printf 'old help\n'; else
      printf '%s\n' '--input-format text --output-format stream-json --verbose --tools --permission-mode acceptEdits --permission-prompts none --no-session-persistence --no-chrome'
    fi
    exit 0 ;;
esac
attempt=0
if test -f "$HARNESS/calls"; then attempt=$(/bin/cat "$HARNESS/calls"); fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$HARNESS/calls"
printf '%s\n' "$@" > "$HARNESS/argv"
printf '%s' "$PWD" > "$HARNESS/cwd"
printf '%s' "$HOME" > "$HARNESS/home"
printf '%s' "${SYNTHETIC_AUTH-}" > "$HARNESS/auth"
printf '%s' "${MMCG_INPUT_ORIGIN-}" > "$HARNESS/input-origin"
/bin/cat > "$HARNESS/stdin"
/bin/cp "$HARNESS/stdin" "$HARNESS/stdin-$attempt"
/bin/cp "$HARNESS/argv" "$HARNESS/argv-$attempt"
/bin/cp .mastermind/tasks/001-invocation/invocation.json "$HARNESS/invocation-$attempt.json"
printf started > "$HARNESS/started"
case "$mode" in
  wait|timeout)
    /bin/sleep 30 & child=$!
    printf '%s' "$child" > "$HARNESS/child"
    if test "$mode" = wait; then
      while ! test -f "$HARNESS/release"; do /bin/sleep 0.02; done
      kill "$child" || true
    else wait "$child"; fi ;;
esac
if test "$mode" != missing_init; then /bin/cat "$HARNESS/init.json"; fi
if test "$mode" = duplicate_init; then /bin/cat "$HARNESS/init.json"; fi
if test "$mode" = bad_json; then printf '{broken\n'; exit 0; fi
printf 'PRIVATE_NATIVE_STDERR\n' >&2
if test "$mode" = repair; then /bin/cat "$HARNESS/tool-error.json"; fi
value=2
case "$mode" in
  auto_once|auto_hidden_dependency)
    if test "$attempt" -eq 1; then value=3; else
      /usr/bin/grep -q '<mastermind-repair-json>' "$HARNESS/stdin" || exit 88
    fi ;;
  auto_forever|auto_denial|auto_scope|auto_missing_receipt|auto_stale_receipt|auto_mutate_before|auto_mutate_after|auto_weaken_check|auto_report_conflict) value=3 ;;
esac
printf 'def keep():\n    return %s\n' "$value" > service.py
if test "$mode" = auto_scope; then printf 'value = 1\n' > unauthorized.py; fi
if test "$mode" = auto_mutate_before; then printf '\n# Changed before the check.\n' >> .mastermind/check.sh; fi
if test "$mode" = auto_weaken_check; then printf '#!/bin/sh\nexit 0\n' > .mastermind/check.sh; fi
"$MMCG_TEST_BIN" --index .mastermind/index.db index . > "$HARNESS/index-log" 2>&1
check_status=0
"$MMCG_TEST_BIN" --index .mastermind/index.db verification run .mastermind/tasks/001-invocation/spec.md --id unit --json > "$HARNESS/verify-log" 2>&1 || check_status=$?
/bin/cp "$HARNESS/verify-log" "$HARNESS/verify-$attempt.json"
if test "$mode" = auto_report_conflict; then
  "$MMCG_TEST_BIN" --index .mastermind/index.db verification run .mastermind/tasks/001-invocation/spec.md --id other --json > "$HARNESS/other-check-log" 2>&1
fi
if test "$value" -eq 2 || test "$mode" = auto_weaken_check; then
  test "$check_status" -eq 0
  /bin/cp "$HARNESS/report.json" .mastermind/tasks/001-invocation/executor-report.md
else
  test "$check_status" -ne 0
  /bin/cp "$HARNESS/partial-report.json" .mastermind/tasks/001-invocation/executor-report.md
fi
if test "$mode" = auto_scope; then /bin/cp "$HARNESS/scope-report.json" .mastermind/tasks/001-invocation/executor-report.md; fi
if test "$mode" = auto_missing_receipt; then /bin/rm .mastermind/tasks/001-invocation/verification/unit.json; fi
if test "$mode" = auto_stale_receipt; then printf 'def keep():\n    return 4\n' > service.py; fi
if test "$mode" = auto_mutate_after; then printf '\n# Changed after the check.\n' >> .mastermind/check.sh; fi
if test "$mode" = changed_spec; then printf '\nChanged during invocation.\n' >> .mastermind/tasks/001-invocation/spec.md; fi
case "$mode" in
  missing_result) exit 0 ;;
  denial|auto_denial) /bin/cat "$HARNESS/denied.json" ;;
  terminal_error) /bin/cat "$HARNESS/error.json" ;;
  *) /bin/cat "$HARNESS/result.json" ;;
esac
if test "$mode" = exit_failure; then exit 7; fi
"##;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    harness: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let home = temp.path().join("home");
        let harness = temp.path().join("harness");
        let bin = temp.path().join("native-bin");
        for directory in [&root, &home, &harness, &bin] {
            fs::create_dir_all(directory).unwrap();
        }
        let fixture = Self {
            root: root.canonicalize().unwrap(),
            home: home.canonicalize().unwrap(),
            harness: harness.canonicalize().unwrap(),
            bin: bin.canonicalize().unwrap(),
            _temp: temp,
        };
        fixture.write(".gitignore", ".mastermind/\n");
        fixture.write("service.py", "def keep():\n    return 1\n");
        fixture.write(
            "CONTEXT.md",
            "# Project\n\n## Runtime\nThe project keeps service results explicit.\n",
        );
        for args in [
            vec!["init", "-q", "--initial-branch=main"],
            vec!["config", "user.name", "Invocation Fixture"],
            vec!["config", "user.email", "invocation@example.invalid"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "core.hooksPath", ""],
            vec!["add", "."],
            vec!["commit", "-q", "-m", "Synthetic invocation baseline"],
            vec!["tag", "baseline"],
        ] {
            assert_success(&fixture.command("/usr/bin/git").args(args).output().unwrap());
        }
        let metadata = json!({
            "mode": "verified", "touches": [{"file": "service.py", "symbols": ["keep"]}],
            "verify": [{"cmd": "./.mastermind/check.sh", "run": {
                "id": "unit", "argv": ["./.mastermind/check.sh"], "cwd": ".", "timeout_secs": 10
            }}],
            "acceptance": [{"id": "service-result", "statement": "The service returns two.", "checks": ["unit"]}]
        });
        fixture.write(SPEC, &format!("---\n{}---\n# Invocation fixture\n\n## Goals\nUpdate the service.\n\n## Scope\nEdit service.py.\n\n## Acceptance Criteria\nThe service returns two.\n\n## Tests Plan\nRun the declared check.\n\n## Final Verification\nRun the declared command.\n\n## Alternatives Considered\nKeep one service.\n\n## Risk Register\nLocal behavior only.\n\n## Evidence Ledger\nUse the declared check.\n\n## Documentation Plan\nNo public documentation change.\n\n## Observability Plan\nNo logging change.\n\n## Performance Considerations\nConstant work.\n\n## Rollback / Migration\nRestore the previous return value.\n", serde_norway::to_string(&metadata).unwrap()));
        fixture.write(".mastermind/check.sh", PROBE);
        fs::set_permissions(
            fixture.root.join(".mastermind/check.sh"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::write(fixture.bin.join("claude"), FAKE).unwrap();
        fs::set_permissions(
            fixture.bin.join("claude"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fixture.mode(mode);
        fixture.stream("init.json", json!({"type":"system", "subtype":"init", "cwd":fixture.root,
            "session_id":"synthetic-native-session", "model":"synthetic-model", "claude_code_version":"2.1.267",
            "permissionMode":"acceptEdits", "tools":["Read","Edit","Write","Grep","Glob","Bash","mcp__existing__reader"],
            "mcp_servers":[{"name":"existing", "status":"connected"}], "plugins":[], "skills":[]}));
        let result = json!({"type":"result", "subtype":"success", "is_error":false,
            "session_id":"synthetic-native-session", "result":"PRIVATE_NATIVE_FINAL_MESSAGE", "permission_denials":[],
            "num_turns":2, "duration_ms":12});
        fixture.stream("result.json", result.clone());
        let mut denied = result.clone();
        denied["permission_denials"] = json!([{"tool_name":"Bash", "tool_use_id":"private-denied-id", "tool_input":{"command":"PRIVATE_DENIED_COMMAND"}}]);
        fixture.stream("denied.json", denied);
        let mut error = result;
        error["subtype"] = json!("error_max_turns");
        error["is_error"] = json!(true);
        fixture.stream("error.json", error);
        fixture.stream("tool-error.json", json!({"type":"user", "session_id":"synthetic-native-session",
            "message":{"role":"user", "content":[{"type":"tool_result", "tool_use_id":"synthetic-check", "is_error":true, "content":"A test failed before repair."}]}}));
        fs::write(
            fixture.harness.join("report.json"),
            canonical_report().to_string(),
        )
        .unwrap();
        let mut partial = canonical_report();
        partial["status"] = json!("partial");
        partial["defects"] = json!([{"kind":"implementation_defect", "phase":"implementation",
            "details":"The service returns three instead of two.", "remediation_hint":"Correct the return value."}]);
        partial["verifications"][0]["result"] = json!("fail");
        partial["verifications"][0]["observed"]["exit_code"] = json!(9);
        fixture.stream("partial-report.json", partial.clone());
        partial["files_modified"] = json!(["service.py", "unauthorized.py"]);
        fixture.stream("scope-report.json", partial);
        fixture.index();
        fixture
    }

    fn mode(&self, mode: &str) {
        fs::write(self.harness.join("mode"), mode).unwrap();
    }
    fn stream(&self, name: &str, value: Value) {
        fs::write(self.harness.join(name), format!("{value}\n")).unwrap();
    }
    fn write(&self, path: &str, body: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.bin.display()),
            )
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", self.home.join("codex"))
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("MMCG_GIT_TIMEOUT_MS", "20000")
            .env("HARNESS", &self.harness)
            .env("MMCG_TEST_BIN", env!("CARGO_BIN_EXE_mmcg"))
            .env("SYNTHETIC_AUTH", "PRIVATE_SYNTHETIC_AUTH");
        command
    }
    fn cli(&self, args: &[&str]) -> Command {
        let mut command = self.command(env!("CARGO_BIN_EXE_mmcg"));
        command.args(["--index", ".mastermind/index.db"]).args(args);
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cli(args).output().unwrap()
    }
    fn index(&self) {
        assert_success(&self.run(&["index", "."]));
    }
    fn execute(&self, options: &[&str]) -> Output {
        self.cli(&["run-task", SPEC, "--exec"])
            .args(options)
            .output()
            .unwrap()
    }
    fn receipt(&self) -> Value {
        self.json(RECEIPT)
    }
    fn context_preview(&self) -> Value {
        let output = self.run(&[
            "context",
            "preview",
            "--since",
            "baseline",
            "--role",
            "executor",
            "--workflow",
            "verified",
            "--path",
            "service.py",
            "--budget-tokens",
            "8000",
        ]);
        assert_success(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn calls(&self) -> u32 {
        fs::read_to_string(self.harness.join("calls"))
            .map(|value| value.parse().unwrap())
            .unwrap_or(0)
    }
    fn attempt_input(&self, attempt: u32) -> String {
        fs::read_to_string(self.harness.join(format!("stdin-{attempt}"))).unwrap()
    }
    fn attempt_receipt(&self, attempt: u32) -> Value {
        serde_json::from_slice(
            &fs::read(self.harness.join(format!("invocation-{attempt}.json"))).unwrap(),
        )
        .unwrap()
    }
    fn edit_frontmatter(&self, edit: impl FnOnce(&mut Value)) {
        let body = fs::read_to_string(self.root.join(SPEC)).unwrap();
        let (frontmatter, content) = body
            .strip_prefix("---\n")
            .unwrap()
            .split_once("---\n")
            .unwrap();
        let mut metadata: Value = serde_norway::from_str(frontmatter).unwrap();
        edit(&mut metadata);
        self.write(
            SPEC,
            &format!(
                "---\n{}---\n{content}",
                serde_norway::to_string(&metadata).unwrap()
            ),
        );
    }
    fn state(&self) -> Value {
        self.json(STATE)
    }
    fn json(&self, path: &str) -> Value {
        serde_json::from_slice(&fs::read(self.root.join(path)).unwrap()).unwrap()
    }
    fn packet(&self) -> (Vec<u8>, Value) {
        let input = fs::read_to_string(self.harness.join("stdin")).unwrap();
        let wire = input
            .split_once("<mastermind-context-json>\n")
            .unwrap()
            .1
            .strip_suffix("\n</mastermind-context-json>\n")
            .unwrap();
        (
            wire.as_bytes().to_vec(),
            serde_json::from_str(wire).unwrap(),
        )
    }
    fn finish_artifacts(&self) {
        self.write("service.py", "def keep():\n    return 2\n");
        self.index();
        assert_success(&self.run(&["verification", "run", SPEC, "--id", "unit", "--json"]));
        self.write(REPORT, &canonical_report().to_string());
    }
    fn resolve_history_review(&self) {
        let request = self.run(&["review-task", "prepare", SPEC, "--json"]);
        assert_success(&request);
        let request: Value = serde_json::from_slice(&request.stdout).unwrap();
        let mut report = request["draft"].clone();
        report["reviewer"] = json!({"kind":"human", "name":"Synthetic invocation reviewer"});
        for criterion in report["criteria"].as_array_mut().unwrap() {
            criterion["status"] = json!("satisfied");
            criterion["reason"] = json!("The sole return expression now returns two, satisfying this synthetic service requirement.");
            criterion["evidence"] = json!(["check:unit", "spec"]);
        }
        report["verification_quality"] = json!({"status":"satisfied", "reason":"The declared check runs after the edit and rejects the original return value.", "evidence":["check:unit"]});
        report["scope_control"] = json!({"status":"satisfied", "reason":"Only the declared service file has a product change in the worktree.", "evidence":["worktree", "spec"]});
        report["proportionality"] = json!({"status":"satisfied", "reason":"A single literal replacement implements the requested behavior without a new abstraction.", "evidence":["worktree", "executor-report"]});
        report["history"] = json!({
            "context": {"decision":"no_change", "reason":"The existing project description needs no durable change for this return-value fixture.", "evidence":["knowledge:context", "spec", "worktree"]},
            "lessons": {"decision":"no_change", "reason":"This synthetic literal replacement introduces no reusable lesson beyond the task evidence.", "evidence":["knowledge:lessons", "worktree"]}
        });
        let input = ".mastermind/tasks/001-invocation/review-input.json";
        self.write(input, &report.to_string());
        assert_success(&self.run(&["review-task", "submit", SPEC, "--report", input, "--json"]));
    }
    fn await_started(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !self.harness.join("started").exists() {
            assert!(
                Instant::now() < deadline,
                "fake native client did not start"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn spawn(&self, options: &[&str]) -> Child {
        self.cli(&["run-task", SPEC, "--exec"])
            .args(options)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }
    fn seed_private_preference(&self) {
        let transcript = self
            .home
            .join("codex/sessions/2026/09/26/rollout-invocation.jsonl");
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        let records = [
            json!({"type":"session_meta", "payload":{"id":"invocation-private-session", "cwd":self.root, "source":"vscode", "thread_source":"user"}}),
            json!({"type":"turn_context", "payload":{"turn_id":"invocation-turn", "cwd":self.root}}),
            json!({"type":"response_item", "timestamp":"2026-09-26T10:00:00Z", "payload":{"type":"message", "role":"user",
                "content":[{"type":"input_text", "text":QUOTE}], "internal_chat_message_metadata_passthrough":{
                    "turn_id":"invocation-turn", "content_item_kinds":["user.text"]}}}),
        ];
        fs::write(
            &transcript,
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        assert_success(&self.run(&[
            "miner",
            "collect",
            "--transcript",
            transcript.to_str().unwrap(),
        ]));
        let output = self.run(&["miner", "candidates", "list"]);
        assert_success(&output);
        let inbox: Value = serde_json::from_slice(&output.stdout).unwrap();
        let candidate = &inbox["candidates"][0]["candidate"];
        assert_success(&self.run(&[
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
        ]));
        let store = ProfileStore::open_read_only(&self.home.join(".mastermind/style.db")).unwrap();
        let feedback = store.feedback().unwrap().remove(0);
        drop(store);
        use std::os::fd::FromRawFd;
        let (mut master, mut slave) = (-1, -1);
        // The real interactive review gate gets an isolated pseudo-terminal.
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
        assert_success(
            &self
                .cli(&[
                    "miner",
                    "feedback",
                    "accept",
                    &feedback.key,
                    "--revision",
                    &feedback.review_revision(),
                ])
                .stdin(Stdio::from(slave))
                .output()
                .unwrap(),
        );
        assert_success(&self.run(&["miner", "access", "grant", "--client", "invocation-test"]));
        fs::write(
            self.home.join(".mastermind/style.md"),
            "PRIVATE_STATIC_PROFILE_NOT_AUTHORITATIVE\n",
        )
        .unwrap();
    }
}

fn canonical_report() -> Value {
    json!({"schema_version":1, "spec":SPEC, "status":"complete", "phases":[], "files_modified":["service.py"],
        "claims":[], "defects":[], "verifications":[{"cmd":"./.mastermind/check.sh", "result":"pass", "observed":{"exit_code":0}}]})
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
fn no_running_process(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let output = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "stat="])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || state.trim().is_empty() || state.trim().starts_with('Z') {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "invocation descendant is still running: {pid}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn intake_binding_reaches_native_checks_review_and_historical_completion() {
    let fixture = Fixture::new("repair");
    // Local hook installation is not a product change in this task.
    fixture.write(".git/info/exclude", ".claude/\n");
    let interpreter = Command::new("python3")
        .args(["-I", "-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert_success(&interpreter);
    let python = PathBuf::from(String::from_utf8(interpreter.stdout).unwrap().trim())
        .canonicalize()
        .unwrap();
    let processor = fixture.harness.join("refiner.py");
    fs::write(&processor, "import json,sys\ni=json.load(sys.stdin)['input']\nprint(json.dumps(dict(schema=1,intake_id=i['id'],prompt_digest=i['prompt_digest'],action='passthrough',workflow_intent='activate_mastermind',intent_evidence=i['original'],refined_prompt=i['original'],questions=[])))\n").unwrap();
    assert_success(
        &fixture
            .cli(&[
                "miner",
                "hooks",
                "setup",
                "--client",
                "claude",
                "--write",
                "--refiner-processor",
            ])
            .arg(python)
            .arg("--refiner-arg=-I")
            .arg(format!("--refiner-arg={}", processor.display()))
            .output()
            .unwrap(),
    );
    let original = "Use Mastermind to make service.py return two, without expanding the scope.";
    for (kind, prompt) in [("SessionStart", ""), ("UserPromptSubmit", original)] {
        let input = json!({"session_id":"source-session", "event_id":kind, "turn_id":"one", "hook_event_name":kind,
            "source":"startup","cwd":fixture.root,"prompt":prompt});
        let mut child = fixture
            .cli(&["miner", "hooks", "receive", "--client", "claude"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.to_string().as_bytes())
            .unwrap();
        assert_success(&child.wait_with_output().unwrap());
    }
    let db =
        rusqlite::Connection::open(fixture.home.join(".mastermind/persona-events.db")).unwrap();
    let data: String = db
        .query_row("SELECT data FROM hook_intake", [], |row| row.get(0))
        .unwrap();
    let source: Value = serde_json::from_str(&data).unwrap();
    let id = source["input"]["id"].as_str().unwrap();
    let output = fixture.run(&["miner", "hooks", "bind-task", id, "--spec", SPEC]);
    assert_success(&output);
    let binding: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_success(&fixture.execute(&[]));
    let revision = &binding["binding"]["revision"];
    assert_eq!(&fixture.state()["intake_revision"], revision);
    assert_eq!(&fixture.receipt()["binding"]["intake_revision"], revision);
    assert_eq!(
        &fixture.json(".mastermind/tasks/001-invocation/verification/unit.json")["binding"]
            ["intake_revision"],
        revision
    );
    let input = fixture.attempt_input(1);
    assert!(input.contains("<mastermind-intake-json>"));
    assert!(input.contains(original));
    assert!(!fs::read_to_string(fixture.root.join(RECEIPT))
        .unwrap()
        .contains(original));
    let preview = fixture.context_preview();
    let invocation = &preview["layers"]["work"]["data"]["tasks"][0]["invocation"];
    assert_eq!(&invocation["binding"]["intake_revision"], revision);
    assert_eq!(
        invocation["invocation_id"],
        fixture.receipt()["invocation_id"]
    );
    assert_eq!(invocation["validation"], "matches_recorded_iteration");
    assert!(
        !preview.to_string().contains(original),
        "the source link contains only its revision"
    );
    fixture.resolve_history_review();
    assert_success(&fixture.run(&["run-task", SPEC]));
    assert_eq!(fixture.state()["status"], "learned");
    let episode = source["input"]["episode_id"].as_str().unwrap();
    let shown = fixture.run(&["miner", "hooks", "show", episode]);
    assert_success(&shown);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    let forgotten = fixture.run(&[
        "miner",
        "hooks",
        "forget",
        episode,
        "--revision",
        shown["episode"]["revision"].as_str().unwrap(),
    ]);
    assert_success(&forgotten);
    assert_success(&fixture.run(&["run-task", SPEC]));
    assert_eq!(fixture.state()["status"], "learned");
    assert!(!fixture.execute(&[]).status.success());
    assert_eq!(
        fixture.calls(),
        1,
        "forgotten source cannot start another executor"
    );
}

#[test]
fn native_executor_automatically_uses_the_configured_hook_audience_and_its_own_scope() {
    let fixture = Fixture::new("repair");
    fixture.write(".git/info/exclude", ".claude/settings.local.json\n");
    fixture.seed_private_preference();
    fs::remove_file(fixture.home.join(".mastermind/style.md")).unwrap();
    assert_success(&fixture.run(&["miner", "profile", "--author", "invocation@example.invalid"]));
    let db_path = fixture.home.join(".mastermind/style.db");
    let repo_key = fixture.root.join(".git").canonicalize().unwrap();
    let prior = ProfileStore::open_read_only(&db_path)
        .unwrap()
        .repo_meta(repo_key.to_str().unwrap())
        .unwrap()
        .unwrap();
    fixture.write(
        "service.py",
        "# Second committed observation\ndef keep():\n    return 1\n",
    );
    for args in [
        vec!["add", "service.py"],
        vec!["commit", "-qm", "Add a committed observation"],
        vec!["config", "user.name", "Unrelated current Git name"],
    ] {
        assert_success(&fixture.command("/usr/bin/git").args(args).output().unwrap());
    }
    let head = fixture
        .command("/usr/bin/git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert_success(&head);
    let head = String::from_utf8(head.stdout).unwrap().trim().to_owned();
    assert_ne!(prior.1.as_deref(), Some(head.as_str()));
    fixture.index();
    assert_success(&fixture.run(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "claude",
        "--profile-client",
        "invocation-test",
        "--write",
    ]));
    assert_success(&fixture.execute(&[]));
    assert_eq!(
        fixture.receipt()["options"]["profile_client"],
        "invocation-test"
    );
    let (_, packet) = fixture.packet();
    assert_eq!(
        packet["layers"]["person"]["data"]["selection"]["role"],
        "executor"
    );
    assert_eq!(
        packet["layers"]["person"]["data"]["selection"]["paths"],
        json!(["service.py"])
    );
    assert!(packet.to_string().contains(PREFERENCE));
    let store = ProfileStore::open_read_only(&db_path).unwrap();
    let current = store
        .repo_meta(repo_key.to_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(current.0, "invocation@example.invalid");
    assert_eq!(current.1.as_deref(), Some(head.as_str()));
    assert_eq!(store.feedback().unwrap().len(), 1);
    assert!(store.habits().unwrap().is_empty());
}

#[test]
fn native_executor_preserves_profile_delivery_opt_out_without_revoking_mcp_access() {
    let fixture = Fixture::new("repair");
    fixture.write(".git/info/exclude", ".claude/settings.local.json\n");
    fixture.seed_private_preference();
    assert_success(&fixture.run(&["miner", "access", "grant", "--client", "claude"]));
    assert_success(&fixture.run(&[
        "miner",
        "hooks",
        "setup",
        "--client",
        "claude",
        "--disable-profile",
        "--write",
    ]));
    assert_success(&fixture.run(&["miner", "hooks", "setup", "--client", "claude", "--write"]));
    assert_success(&fixture.execute(&[]));
    assert!(fixture.receipt()["options"]["profile_client"].is_null());
    let (_, packet) = fixture.packet();
    assert_eq!(packet["layers"]["person"]["status"], "not_enabled");
    assert!(!packet.to_string().contains(PREFERENCE));
    let store = ProfileStore::open_read_only(&fixture.home.join(".mastermind/style.db")).unwrap();
    assert!(store
        .reader_allowed(fixture.root.to_str().unwrap(), "claude")
        .unwrap());
}

#[test]
fn successful_invocation_binds_exact_stdin_context_and_keeps_private_payloads_out_of_receipt() {
    let fixture = Fixture::new("repair");
    fixture.seed_private_preference();
    assert_success(&fixture.execute(&[
        "--profile-client",
        "invocation-test",
        "--exec-max-turns",
        "7",
    ]));
    let receipt = fixture.receipt();
    assert_eq!(receipt["status"], "passed", "{receipt}");
    assert_eq!(receipt["native"]["tool_errors"], 1);
    assert_eq!(receipt["options"]["max_turns"], 7);
    assert_eq!(fixture.state()["invocation_required"], true);
    assert_eq!(fixture.state()["status"], "history_review_required");
    let parsed: mmcg::invocation::InvocationReceipt =
        serde_json::from_value(receipt.clone()).unwrap();
    assert!(parsed.success());
    let input = fs::read(fixture.harness.join("stdin")).unwrap();
    let (wire, packet) = fixture.packet();
    assert_eq!(receipt["context_delivery"]["prompt_sha256"], sha(&input));
    assert_eq!(receipt["context_delivery"]["prompt_bytes"], input.len());
    assert_eq!(receipt["context_delivery"]["bytes_offered"], input.len());
    assert_eq!(
        receipt["context_delivery"]["context_wire_sha256"],
        sha(&wire)
    );
    assert_eq!(receipt["context_delivery"]["context_bytes"], wire.len());
    assert_eq!(
        receipt["context_delivery"]["context_revision"],
        packet["context_revision"]
    );
    assert_eq!(receipt["context_delivery"]["status"], "offered_to_process");
    assert_eq!(receipt["context_delivery"]["model_use"], "unknown");
    assert_eq!(
        receipt["context_delivery"]["input_origin"],
        "controller_generated"
    );
    assert_eq!(
        fs::read_to_string(fixture.harness.join("input-origin")).unwrap(),
        "controller"
    );
    assert_eq!(packet["selection"]["role"], "executor");
    assert_eq!(packet["selection"]["paths"], json!(["service.py"]));
    assert_eq!(packet["selection"]["workflow"], "verified");
    assert!(String::from_utf8(wire).unwrap().contains(PREFERENCE));
    assert!(!String::from_utf8_lossy(&input).contains("PRIVATE_STATIC_PROFILE_NOT_AUTHORITATIVE"));
    let disk = fs::read_to_string(fixture.root.join(RECEIPT)).unwrap();
    for private in [
        PREFERENCE,
        QUOTE,
        "PRIVATE_NATIVE_FINAL_MESSAGE",
        "PRIVATE_NATIVE_STDERR",
        "PRIVATE_SYNTHETIC_AUTH",
    ] {
        assert!(
            !disk.contains(private),
            "receipt persisted private content: {private}"
        );
    }
    let preview = fixture.context_preview();
    let task = &preview["layers"]["work"]["data"]["tasks"][0];
    assert_eq!(task["phase"], "awaiting_history_review");
    assert_eq!(task["completion_basis"], "in_progress");
    assert_eq!(task["current_checkout"], "not_verified");
    let projected = &task["invocation"];
    assert_eq!(projected["invocation_id"], receipt["invocation_id"]);
    assert_eq!(projected["role"], "executor");
    assert_eq!(projected["status"], "passed");
    assert_eq!(projected["binding"], receipt["binding"]);
    assert_eq!(projected["context_delivery"], receipt["context_delivery"]);
    assert_eq!(projected["validation"], "matches_recorded_iteration");
    assert_eq!(projected["current_inputs"], "not_rechecked");
    assert_eq!(projected["provenance"], "local_unsigned_record");
    let keys = projected
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            "binding",
            "context_delivery",
            "current_inputs",
            "invocation_id",
            "provenance",
            "role",
            "status",
            "validation"
        ]
    );
    for private in [
        PREFERENCE,
        QUOTE,
        "PRIVATE_NATIVE_FINAL_MESSAGE",
        "PRIVATE_NATIVE_STDERR",
        "PRIVATE_SYNTHETIC_AUTH",
        "PRIVATE_VERIFICATION_OUTPUT",
        "<mastermind-context-json>",
        fixture.bin.to_str().unwrap(),
        fixture.home.to_str().unwrap(),
    ] {
        assert!(
            !preview.to_string().contains(private),
            "context preview exposed private invocation data: {private}"
        );
    }
    assert_eq!(
        fixture.calls(),
        1,
        "preview is read-only and does not execute a client"
    );
    let args = fs::read_to_string(fixture.harness.join("argv")).unwrap();
    assert!(args.contains("--permission-mode\nacceptEdits\n--permission-prompts\nnone\n"));
    assert!(args.contains("--tools\nRead,Edit,Write,Grep,Glob,Bash\n"));
    assert!(
        !args.contains("--bare") && !args.contains("--allowedTools") && !args.contains(PREFERENCE)
    );
    assert_eq!(
        fs::read_to_string(fixture.harness.join("home")).unwrap(),
        fixture.home.to_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(fixture.harness.join("auth")).unwrap(),
        "PRIVATE_SYNTHETIC_AUTH"
    );
    assert_eq!(receipt["policy"]["mcp"], "inherited_unverified");
    assert_eq!(
        receipt["policy"]["filesystem"],
        "not_enforced_by_mastermind"
    );
    assert_eq!(
        fs::metadata(fixture.root.join(RECEIPT))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(!fixture.home.join(".claude").exists());
    assert!(String::from_utf8_lossy(&input).contains("mastermind verification run"));
    assert!(String::from_utf8_lossy(&input).contains("\"schema_version\":1"));
    assert!(!fixture
        .root
        .join("schemas/executor-report-v1.schema.json")
        .exists());
    fixture.resolve_history_review();
    assert_success(&fixture.run(&["run-task", SPEC]));
    assert_eq!(fixture.state()["status"], "learned");
}

#[test]
fn native_context_retrieves_documents_from_literal_task_title_terms() {
    let fixture = Fixture::new("success");
    let spec = fs::read_to_string(fixture.root.join(SPEC)).unwrap();
    fixture.write(
        SPEC,
        &spec.replacen(
            "---\n",
            "---\ntitle: 'Runtime OR \"symbolicneedle\" NOT absentterm'\n",
            1,
        ),
    );
    assert_success(&fixture.execute(&[]));
    let (_, packet) = fixture.packet();
    assert_eq!(
        packet["selection"]["query"],
        "\"Runtime\" OR \"symbolicneedle\" OR \"NOT\" OR \"absentterm\""
    );
    let documents = &packet["layers"]["documentation"];
    assert_eq!(documents["status"], "ok");
    assert_eq!(documents["data"]["freshness"], "fresh");
    assert!(documents["data"]["observed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|hit| hit["path"] == "CONTEXT.md"));
    assert_eq!(
        fixture.receipt()["context_delivery"]["context_revision"],
        packet["context_revision"]
    );
}

#[test]
fn context_preview_withholds_mismatched_or_malformed_invocation_metadata() {
    let fixture = Fixture::new("success");
    assert_success(&fixture.execute(&[]));
    let original = fs::read_to_string(fixture.root.join(RECEIPT)).unwrap();
    let receipt: Value = serde_json::from_str(&original).unwrap();
    let state = fs::read(fixture.root.join(STATE)).unwrap();
    assert_eq!(
        fixture.context_preview()["layers"]["work"]["data"]["tasks"][0]["invocation"]["status"],
        "passed"
    );

    for (field, value) in [
        ("spec_sha256", json!("0".repeat(64))),
        (
            "repository_identity",
            json!("git-worktree:sha256:".to_owned() + &"0".repeat(64)),
        ),
        ("baseline_oid", json!("0".repeat(40))),
        (
            "iteration",
            json!(receipt["binding"]["iteration"].as_u64().unwrap() + 1),
        ),
        ("intake_revision", json!("a".repeat(64))),
    ] {
        let mut altered = receipt.clone();
        altered["binding"][field] = value;
        fixture.write(RECEIPT, &altered.to_string());
        let preview = fixture.context_preview();
        let invocation = &preview["layers"]["work"]["data"]["tasks"][0]["invocation"];
        assert_eq!(
            *invocation,
            json!({"status":"unavailable","delivery":"not_verified"}),
            "binding mismatch: {field}"
        );
        assert!(!preview
            .to_string()
            .contains(receipt["invocation_id"].as_str().unwrap()));
        assert_eq!(
            fs::read(fixture.root.join(STATE)).unwrap(),
            state,
            "preview does not repair task state"
        );
    }
    fixture.write(
        RECEIPT,
        "{\"private\":\"PRIVATE_MALFORMED_INVOCATION_PAYLOAD\"",
    );
    let preview = fixture.context_preview();
    assert_eq!(
        preview["layers"]["work"]["data"]["tasks"][0]["invocation"],
        json!({"status":"unavailable","delivery":"not_verified"})
    );
    assert!(!preview
        .to_string()
        .contains("PRIVATE_MALFORMED_INVOCATION_PAYLOAD"));
    fixture.write(RECEIPT, &original);
    assert_eq!(
        fixture.context_preview()["layers"]["work"]["data"]["tasks"][0]["invocation"]
            ["invocation_id"],
        receipt["invocation_id"]
    );
    assert_eq!(
        fixture.calls(),
        1,
        "reading receipt metadata must not invoke the client again"
    );
}

#[test]
fn strict_controller_selection_overrides_frontmatter_workflow() {
    let fixture = Fixture::new("success");
    let output = fixture.execute(&["--strict"]);
    assert_eq!(fixture.receipt()["status"], "passed", "{output:?}");
    assert_eq!(fixture.packet().1["selection"]["workflow"], "strict");
}

#[test]
fn unsupported_runtime_refuses_before_model_spawn_and_retains_pending_replacement() {
    for mode in ["old_version", "missing_capability"] {
        let fixture = Fixture::new(mode);
        let output = fixture.execute(&[]);
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(fixture.receipt()["status"], "runtime_unsupported");
        assert!(!fixture.harness.join("started").exists());
        assert_eq!(
            fixture.receipt()["context_delivery"]["status"],
            "not_prepared"
        );
        assert_eq!(fixture.state()["invocation_required"], true);
    }
}

#[test]
fn native_denial_and_terminal_failures_cannot_enter_postflight_with_complete_report() {
    for mode in ["denial", "terminal_error", "exit_failure"] {
        let fixture = Fixture::new(mode);
        let output = fixture.execute(&[]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        let receipt = fixture.receipt();
        assert_eq!(receipt["status"], "failed", "{mode}: {receipt}");
        assert!(fixture.root.join(REPORT).exists());
        assert!(!fixture
            .run(&["run-task", SPEC, "--post-only"])
            .status
            .success());
        assert_ne!(fixture.state()["status"], "history_review_required");
        assert_eq!(fixture.state()["next_step"], "run_executor");
        fixture.index();
        let next = fixture.run(&["next"]);
        assert_success(&next);
        let next = String::from_utf8_lossy(&next.stdout);
        assert!(
            next.contains("--exec") && next.contains("invocation.json"),
            "{next}"
        );
        if mode == "denial" {
            assert_eq!(receipt["reason"], "invocation_native_permission_denied");
            assert_eq!(receipt["native"]["result"]["permission_denials"], 1);
            assert!(!receipt.to_string().contains("PRIVATE_DENIED_COMMAND"));
        }
    }
}

#[test]
fn malformed_missing_or_duplicate_native_protocol_never_counts_as_success() {
    for mode in [
        "missing_init",
        "missing_result",
        "duplicate_init",
        "bad_json",
    ] {
        let fixture = Fixture::new(mode);
        let output = fixture.execute(&[]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(fixture.receipt()["status"], "protocol_error", "{mode}");
    }
}

#[test]
fn spec_change_during_native_execution_invalidates_otherwise_successful_result() {
    let fixture = Fixture::new("changed_spec");
    let output = fixture.execute(&[]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fixture.receipt()["status"], "input_changed");
    assert_eq!(fixture.receipt()["native"]["result"]["subtype"], "success");
    assert!(!fixture
        .run(&["run-task", SPEC, "--post-only"])
        .status
        .success());
}

#[test]
fn timeout_kills_child_group_and_does_not_allow_manually_completed_artifacts() {
    let fixture = Fixture::new("timeout");
    let started = Instant::now();
    let output = fixture.execute(&["--exec-timeout", "1"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(fixture.receipt()["status"], "timeout");
    let pid: i32 = fs::read_to_string(fixture.harness.join("child"))
        .unwrap()
        .parse()
        .unwrap();
    no_running_process(pid);
    fixture.finish_artifacts();
    assert!(!fixture
        .run(&["run-task", SPEC, "--post-only"])
        .status
        .success());
}

#[test]
fn pending_or_deleted_receipt_cannot_close_a_completed_native_attempt() {
    for pending in [false, true] {
        let fixture = Fixture::new("success");
        assert_success(&fixture.execute(&[]));
        fixture.resolve_history_review();
        if pending {
            let mut receipt = fixture.receipt();
            receipt["status"] = json!("pending");
            fixture.write(RECEIPT, &receipt.to_string());
        } else {
            fs::remove_file(fixture.root.join(RECEIPT)).unwrap();
        }
        assert!(!fixture.run(&["run-task", SPEC]).status.success());
        let output = fixture.run(&["run-task", SPEC, "--post-only"]);
        assert!(!output.status.success(), "{pending}: {output:?}");
        assert_eq!(fixture.state()["invocation_required"], true);
    }
}

#[test]
fn new_explicit_execution_changes_iteration_and_cannot_reuse_old_success() {
    let fixture = Fixture::new("success");
    assert_success(&fixture.execute(&[]));
    let old = fixture.receipt();
    let iteration = fixture.state()["iteration"].as_u64().unwrap();
    fixture.mode("old_version");
    assert!(!fixture.execute(&[]).status.success());
    assert_eq!(fixture.state()["iteration"], iteration + 1);
    assert_eq!(fixture.receipt()["status"], "runtime_unsupported");
    assert_ne!(fixture.receipt()["invocation_id"], old["invocation_id"]);
    fixture.write(RECEIPT, &old.to_string());
    assert!(!fixture
        .run(&["run-task", SPEC, "--post-only"])
        .status
        .success());
}

#[test]
fn concurrent_controller_attempt_does_not_change_the_active_invocation_binding() {
    let fixture = Fixture::new("wait");
    let mut first = fixture.spawn(&["--exec-timeout", "15"]);
    fixture.await_started();
    let state = fixture.state();
    for option in ["--exec", "--pre-only"] {
        let output = fixture.run(&["run-task", SPEC, option]);
        assert!(!output.status.success(), "{option}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("busy"),
            "{output:?}"
        );
        assert_eq!(fixture.state(), state);
    }
    fs::write(fixture.harness.join("release"), "go").unwrap();
    assert!(first.wait().unwrap().success());
    assert_eq!(fixture.receipt()["status"], "passed");
    assert_eq!(
        fixture.receipt()["binding"]["iteration"],
        state["iteration"]
    );
}

#[test]
fn auto_repair_uses_bound_feedback_then_stops_for_semantic_history_review() {
    let fixture = Fixture::new("auto_once");
    let original_spec = fs::read(fixture.root.join(SPEC)).unwrap();
    let output = fixture.execute(&[
        "--auto-repair",
        "--max-iterations",
        "2",
        "--exec-timeout",
        "20",
        "--exec-max-turns",
        "5",
    ]);
    assert_success(&output);
    assert_eq!(fixture.calls(), 2);
    let first = fixture.attempt_receipt(1);
    let second = fixture.attempt_receipt(2);
    let completed = fixture.receipt();
    assert_eq!(first["binding"]["iteration"], 1);
    assert_eq!(second["binding"]["iteration"], 2);
    assert_ne!(first["invocation_id"], second["invocation_id"]);
    for key in [
        "repository_identity",
        "spec_path",
        "spec_sha256",
        "baseline_oid",
    ] {
        assert_eq!(first["binding"][key], second["binding"][key], "{key}");
        assert_eq!(second["binding"][key], completed["binding"][key], "{key}");
    }
    assert_eq!(first["options"], second["options"]);
    assert_eq!(completed["options"], second["options"]);
    assert_eq!(
        fs::read(fixture.harness.join("argv-1")).unwrap(),
        fs::read(fixture.harness.join("argv-2")).unwrap()
    );
    assert_eq!(original_spec, fs::read(fixture.root.join(SPEC)).unwrap());
    assert!(!fixture
        .attempt_input(1)
        .contains("<mastermind-repair-json>"));
    let second_input = fixture.attempt_input(2);
    let (before_feedback, rest) = second_input.split_once("<mastermind-repair-json>").unwrap();
    assert!(!before_feedback.contains("<mastermind-context-json>"));
    let (feedback, after_feedback) = rest.split_once("</mastermind-repair-json>").unwrap();
    assert!(after_feedback.contains("<mastermind-context-json>"));
    let feedback: Value = serde_json::from_str(feedback.trim()).unwrap();
    assert_eq!(feedback["source_iteration"], 1);
    assert_eq!(feedback["source_invocation_id"], first["invocation_id"]);
    assert_eq!(feedback["failed_checks"], json!(["unit"]));
    assert_eq!(feedback["unmet_criteria"], json!(["service-result"]));
    let failed: Value =
        serde_json::from_slice(&fs::read(fixture.harness.join("verify-1.json")).unwrap()).unwrap();
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["exit_code"], 9);
    assert_eq!(completed["status"], "passed");
    assert_eq!(fixture.state()["iteration"], 2);
    assert_eq!(fixture.state()["status"], "history_review_required");
    assert_eq!(fixture.state()["next_step"], "review_history");
    assert_eq!(fixture.json(REPORT)["status"], "complete");
    assert_eq!(
        fixture.json(".mastermind/tasks/001-invocation/verification/unit.json")["status"],
        "passed"
    );
    for path in [STATE, RECEIPT] {
        let saved = fs::read_to_string(fixture.root.join(path)).unwrap();
        assert!(
            !saved.contains("<mastermind-repair-json>") && !saved.contains("\"failed_checks\"")
        );
    }
}

#[test]
fn auto_repair_respects_iteration_budget_and_plain_exec_does_not_retry() {
    let bounded = Fixture::new("auto_forever");
    let output = bounded.execute(&["--auto-repair", "--max-iterations", "2"]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(bounded.calls(), 2);
    assert_eq!(bounded.state()["iteration"], 2);
    assert_eq!(bounded.receipt()["status"], "passed");
    assert_eq!(bounded.json(REPORT)["status"], "partial");
    assert!(bounded.state()["blocking_reason"]
        .as_str()
        .unwrap()
        .starts_with("auto_repair_"));
    assert_ne!(bounded.state()["status"], "learned");
    let exhausted = bounded.execute(&["--auto-repair", "--max-iterations", "2"]);
    assert!(!exhausted.status.success());
    assert_eq!(bounded.calls(), 2);
    assert_eq!(bounded.state()["iteration"], 2);

    let plain = Fixture::new("auto_once");
    let output = plain.execute(&[]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(plain.calls(), 1);
    assert_eq!(plain.state()["iteration"], 1);
    assert_eq!(plain.receipt()["status"], "passed");
    assert!(!plain.attempt_input(1).contains("<mastermind-repair-json>"));
}

#[test]
fn auto_repair_does_not_retry_native_denial_runtime_scope_or_missing_current_evidence() {
    for (mode, calls) in [
        ("auto_denial", 1),
        ("auto_scope", 1),
        ("auto_missing_receipt", 1),
        ("auto_stale_receipt", 1),
        ("old_version", 0),
        ("changed_spec", 1),
    ] {
        let fixture = Fixture::new(mode);
        let output = fixture.execute(&["--auto-repair", "--max-iterations", "2"]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(fixture.calls(), calls, "{mode}: {output:?}");
        assert_eq!(fixture.state()["iteration"], 1, "{mode}");
        assert_ne!(
            fixture.state()["status"],
            "history_review_required",
            "{mode}"
        );
        assert!(
            fixture.state()["blocking_reason"]
                .as_str()
                .unwrap()
                .starts_with("auto_repair_"),
            "{mode}: {}",
            fixture.state()
        );
    }
}

#[test]
fn auto_repair_requires_all_declared_checks_to_have_current_receipts() {
    let fixture = Fixture::new("auto_once");
    fixture.write(".mastermind/extra-check.sh", "#!/bin/sh\nexit 0\n");
    fs::set_permissions(
        fixture.root.join(".mastermind/extra-check.sh"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fixture.edit_frontmatter(|metadata| {
        metadata["verify"].as_array_mut().unwrap().push(json!({
            "cmd":"./.mastermind/extra-check.sh", "run":{
                "id":"other", "argv":["./.mastermind/extra-check.sh"], "cwd":".", "timeout_secs":10
            }
        }));
    });
    let output = fixture.execute(&["--auto-repair", "--max-iterations", "2"]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fixture.calls(), 1);
    assert_eq!(fixture.receipt()["status"], "passed");
    assert_eq!(
        fixture.json(".mastermind/tasks/001-invocation/verification/unit.json")["exit_code"],
        9
    );
    assert!(!fixture
        .root
        .join(".mastermind/tasks/001-invocation/verification/other.json")
        .exists());
    assert_eq!(fixture.state()["iteration"], 1);
}

#[test]
fn auto_repair_rejects_invalid_flags_and_budgets_before_native_spawn() {
    let fixture = Fixture::new("auto_once");
    for flags in [
        vec!["--auto-repair"],
        vec!["--exec", "--auto-repair", "--pre-only"],
        vec!["--exec", "--auto-repair", "--post-only"],
        vec!["--exec", "--auto-repair", "--force-iteration"],
        vec!["--exec", "--auto-repair", "--max-iterations", "0"],
        vec!["--exec", "--auto-repair", "--max-iterations", "21"],
    ] {
        let output = fixture
            .cli(&["run-task", SPEC])
            .args(&flags)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{flags:?}: {output:?}");
        assert_eq!(fixture.calls(), 0, "{flags:?}");
        assert!(!fixture.harness.join("started").exists());
    }
}

#[test]
fn auto_repair_requires_acceptance_and_observed_verification_contracts() {
    for legacy in [false, true] {
        let fixture = Fixture::new("auto_once");
        fixture.edit_frontmatter(|metadata| {
            if legacy {
                metadata["verify"][0].as_object_mut().unwrap().remove("run");
            } else {
                metadata.as_object_mut().unwrap().remove("acceptance");
            }
        });
        let output = fixture.execute(&["--auto-repair", "--max-iterations", "2"]);
        assert!(!output.status.success(), "{legacy}: {output:?}");
        assert_eq!(fixture.calls(), 0, "{legacy}");
        assert!(!fixture.harness.join("started").exists());
    }
}

#[test]
fn auto_repair_stops_when_check_executable_changes_during_native_run() {
    for mode in [
        "auto_mutate_before",
        "auto_mutate_after",
        "auto_weaken_check",
    ] {
        let fixture = Fixture::new(mode);
        let original_check = fs::read(fixture.root.join(".mastermind/check.sh")).unwrap();
        let output = fixture.execute(&["--auto-repair", "--max-iterations", "2"]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(fixture.calls(), 1, "{mode}");
        assert_eq!(fixture.state()["iteration"], 1, "{mode}");
        assert_eq!(fixture.receipt()["status"], "passed", "{mode}");
        assert_ne!(
            original_check,
            fs::read(fixture.root.join(".mastermind/check.sh")).unwrap()
        );
        assert_ne!(fixture.state()["status"], "history_review_required");
        assert!(fixture.state()["blocking_reason"]
            .as_str()
            .unwrap()
            .starts_with("auto_repair_"));
        let stopped = fixture.state();
        assert_eq!(stopped["next_step"], "run_preflight");
        if mode == "auto_weaken_check" {
            assert_eq!(fixture.json(REPORT)["status"], "complete");
            assert_eq!(
                fixture.json(".mastermind/tasks/001-invocation/verification/unit.json")["status"],
                "passed"
            );
        }
        for args in [
            vec!["run-task", SPEC],
            vec!["run-task", SPEC, "--post-only"],
        ] {
            let resumed = fixture.run(&args);
            assert!(!resumed.status.success(), "{mode}: {resumed:?}");
            assert_eq!(fixture.state(), stopped);
            assert_eq!(fixture.calls(), 1);
        }
    }
}

#[test]
fn auto_repair_rechecks_hidden_dependencies_before_delivering_feedback() {
    let fixture = Fixture::new("auto_hidden_dependency");
    fixture.write("hidden-input.txt", "original\n");
    for args in [
        vec!["add", "hidden-input.txt"],
        vec!["commit", "-q", "-m", "Track hidden verification input"],
        vec!["update-index", "--assume-unchanged", "hidden-input.txt"],
    ] {
        assert_success(&fixture.command("/usr/bin/git").args(args).output().unwrap());
    }
    let output = fixture.execute(&["--auto-repair", "--max-iterations", "2"]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fixture.calls(), 1);
    assert_eq!(fixture.state()["iteration"], 2);
    assert_eq!(fixture.state()["next_step"], "run_preflight");
    assert_eq!(fixture.receipt()["status"], "input_changed");
    assert!(fs::read_to_string(fixture.root.join("hidden-input.txt"))
        .unwrap()
        .contains("changed"));
    let diff = fixture
        .command("/usr/bin/git")
        .args(["diff", "--", "hidden-input.txt"])
        .output()
        .unwrap();
    assert_success(&diff);
    assert!(diff.stdout.is_empty());
    assert!(!fixture.harness.join("stdin-2").exists());
}

#[test]
fn auto_repair_rejects_contradictory_duplicate_and_undeclared_report_rows() {
    for scenario in ["contradictory", "duplicate", "undeclared"] {
        let fixture = Fixture::new("auto_report_conflict");
        fixture.write(".mastermind/other.sh", "#!/bin/sh\nexit 0\n");
        fs::set_permissions(
            fixture.root.join(".mastermind/other.sh"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fixture.edit_frontmatter(|metadata| {
            metadata["verify"].as_array_mut().unwrap().push(json!({
                "cmd":"./.mastermind/other.sh", "run":{
                    "id":"other", "argv":["./.mastermind/other.sh"], "cwd":".", "timeout_secs":10
                }
            }));
        });
        let path = fixture.harness.join("partial-report.json");
        let mut report: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let rows = report["verifications"].as_array_mut().unwrap();
        let passed =
            json!({"cmd":"./.mastermind/other.sh", "result":"pass", "observed":{"exit_code":0}});
        match scenario {
            "contradictory" => rows.push(json!({"cmd":"./.mastermind/other.sh", "result":"fail", "observed":{"exit_code":7}})),
            "duplicate" => rows.extend([passed.clone(), passed]),
            _ => {
                rows.push(passed);
                rows.push(json!({"cmd":"undeclared-command", "result":"pass", "observed":{"exit_code":0}}));
            }
        }
        fs::write(path, serde_json::to_vec(&report).unwrap()).unwrap();
        let output = fixture.execute(&["--auto-repair", "--max-iterations", "2"]);
        assert!(!output.status.success(), "{scenario}: {output:?}");
        assert_eq!(fixture.calls(), 1, "{scenario}: {output:?}");
        assert_eq!(
            fixture.json(".mastermind/tasks/001-invocation/verification/other.json")["status"],
            "passed"
        );
        assert_eq!(fixture.state()["next_step"], "run_preflight");
    }
}
