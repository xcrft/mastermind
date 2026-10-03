//! Native semantic review through the public controller with a synthetic client.
//! No provider, real credentials or global client configuration are used.

#![cfg(unix)]

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const TASK: &str = ".mastermind/tasks/001-native-review";
const SPEC: &str = ".mastermind/tasks/001-native-review/spec.md";
const STATE: &str = ".mastermind/tasks/001-native-review/state.json";
const REPORT: &str = ".mastermind/tasks/001-native-review/executor-report.md";
const EXECUTOR_RECEIPT: &str = ".mastermind/tasks/001-native-review/invocation.json";
const NATIVE_RECEIPT: &str = ".mastermind/tasks/001-native-review/review-invocation.json";
const REVIEW: &str = ".mastermind/tasks/001-native-review/semantic-review.json";
const INPUT: &str = ".mastermind/tasks/001-native-review/review-input.json";
const HISTORY: &str = ".mastermind/tasks/001-native-review/history-review.md";
const CONTEXT: &str = "CONTEXT.md";
const LESSONS: &str = ".mastermind/tasks/_lessons.md";
const MODEL: &str = "synthetic-review-model";
const PROBE: &str = "#!/bin/sh\nprintf 'run\\n' >> .mastermind/check-runs\n/usr/bin/grep -q 'return 2' service.py || exit 9\nprintf 'PRIVATE_REVIEW_CHECK_OUTPUT\\n'\n";
const FAKE: &str = r##"#!/bin/sh
set -eu
mode=$(/bin/cat "$HARNESS/mode")
case "$1" in
  --version)
    if test -f .mastermind/tasks/001-native-review/state.json; then
      /bin/cp .mastermind/tasks/001-native-review/state.json "$HARNESS/probe-state.json"
    fi
    if test "$mode" = unsupported_probe; then printf '2.1.100 (Claude Code)\n'; else printf '2.1.267 (Claude Code)\n'; fi
    exit 0 ;;
  --help)
    printf '%s\n' '--input-format text --output-format stream-json --verbose --tools --permission-mode acceptEdits dontAsk --permission-prompts none --no-session-persistence --no-chrome --max-turns --restricted --strict-mcp-config --mcp-config --disable-slash-commands'
    if test "$mode" != missing_safe_mode; then printf '%s\n' '--safe-mode'; fi
    if test -f "$HARNESS/reviewer-calls" && test "$(/bin/cat "$HARNESS/reviewer-calls")" = 1; then
      case "$mode" in
        follow_up_stale_dependency) printf 'changed after the retry preflight\n' > dependency.txt ;;
        follow_up_stale_review) printf '{}\n' > .mastermind/tasks/001-native-review/semantic-review.json ;;
        follow_up_stale_lessons) printf '# Changed during the next native probe\n' > .mastermind/tasks/_lessons.md ;;
        follow_up_stale_check) printf '\n# Changed after the retry preflight\n' >> .mastermind/check.sh ;;
      esac
    fi
    exit 0 ;;
esac
kind=unknown
previous=
for arg in "$@"; do
  if test "$previous" = --permission-mode; then
    case "$arg" in acceptEdits) kind=executor ;; dontAsk) kind=reviewer ;; esac
  fi
  previous=$arg
done
test "$kind" != unknown
calls=0
if test -f "$HARNESS/$kind-calls"; then calls=$(/bin/cat "$HARNESS/$kind-calls"); fi
calls=$((calls + 1))
printf '%s' "$calls" > "$HARNESS/$kind-calls"
printf '%s\n' "$@" > "$HARNESS/$kind-argv-$calls"
printf '%s' "$HOME" > "$HARNESS/$kind-home"
printf '%s' "$SYNTHETIC_AUTH" > "$HARNESS/$kind-auth"
printf '%s' "$MMCG_INPUT_ORIGIN" > "$HARNESS/$kind-origin"
/bin/cat > "$HARNESS/$kind-stdin-$calls"
if test "$kind" = reviewer; then
  /bin/cp .mastermind/tasks/001-native-review/state.json "$HARNESS/review-start-state.json"
  printf started > "$HARNESS/review-started"
  case "$mode" in
    wait) while ! test -f "$HARNESS/release"; do /bin/sleep 0.02; done ;;
    timeout)
      /bin/sleep 30 & child=$!
      printf '%s' "$child" > "$HARNESS/child"
      wait "$child" ;;
  esac
fi
export MMCG_REVIEW_FIXTURE_KIND=$kind
export MMCG_REVIEW_FIXTURE_ATTEMPT=$calls
"$TEST_HELPER" --ignored --exact native_fixture_helper --nocapture > "$HARNESS/$kind-helper-$calls.log" 2>&1
/bin/cat "$HARNESS/$kind-stream-$calls.jsonl"
printf 'PRIVATE_NATIVE_REVIEW_STDERR\n' >&2
"##;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    harness: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::new_with_history_sources(false)
    }

    fn new_with_history_sources(present: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let home = temp.path().join("home");
        let harness = temp.path().join("harness");
        let bin = temp.path().join("native-bin");
        for path in [&root, &home, &harness, &bin] {
            fs::create_dir_all(path).unwrap();
        }
        let fixture = Self {
            root: root.canonicalize().unwrap(),
            home: home.canonicalize().unwrap(),
            harness: harness.canonicalize().unwrap(),
            bin: bin.canonicalize().unwrap(),
            _temp: temp,
        };
        fixture.mode("positive");
        fixture.write(".gitignore", ".mastermind/\n");
        fixture.write("service.py", "def keep():\n    return 1\n");
        fixture.write("dependency.txt", "stable dependency\n");
        if present {
            fixture.write(
                CONTEXT,
                "# Project context\nThe service has one existing function.\n",
            );
            fixture.write(
                LESSONS,
                "# Project lessons\nUse a failing check before changing behavior.\n",
            );
        }
        for args in [
            vec!["init", "-q", "--initial-branch=main"],
            vec!["config", "user.name", "Native Review Fixture"],
            vec!["config", "user.email", "native-review@example.invalid"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "core.hooksPath", ""],
            vec!["add", "."],
            vec!["commit", "-qm", "Synthetic native review baseline"],
            // Set before preflight, so later hidden-byte changes do not change
            // the Git index and cannot be detected from a new flag alone.
            vec!["update-index", "--assume-unchanged", "dependency.txt"],
        ] {
            fixture.git(&args);
        }
        fixture.write(".mastermind/check.sh", PROBE);
        fs::set_permissions(
            fixture.root.join(".mastermind/check.sh"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let metadata = json!({
            "mode":"verified",
            "touches":[{"file":"service.py", "symbols":["keep"]}],
            "verify":[{"cmd":"./.mastermind/check.sh", "run":{
                "id":"unit", "argv":["./.mastermind/check.sh"], "cwd":".", "timeout_secs":10
            }}],
            "acceptance":[{"id":"service-result",
                "statement":"The existing keep function returns two.", "checks":["unit"]}]
        });
        fixture.write(SPEC, &format!(
            "---\n{}---\n# Native semantic review fixture\n\n## Goals\nReturn two from the service.\n\n## Scope\nEdit the existing service function.\n\n## Acceptance Criteria\nThe existing keep function returns two.\n\n## Tests Plan\nRun the declared unit check.\n\n## Final Verification\nRun the declared check after the edit.\n",
            serde_norway::to_string(&metadata).unwrap()
        ));
        fs::write(fixture.bin.join("claude"), FAKE).unwrap();
        fs::set_permissions(
            fixture.bin.join("claude"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::create_dir_all(fixture.home.join(".claude")).unwrap();
        fs::write(
            fixture.home.join(".claude/settings.json"),
            r#"{"fixture":"preserve-config"}"#,
        )
        .unwrap();
        fs::create_dir_all(fixture.home.join(".mastermind")).unwrap();
        fs::write(
            fixture.home.join(".mastermind/style.md"),
            "PRIVATE_REVIEW_PROFILE_NOT_AUTHORIZED",
        )
        .unwrap();
        fixture.index();
        fixture
    }

    fn held() -> Self {
        Self::new().hold()
    }

    fn hold(self) -> Self {
        let fixture = self;
        assert_success(&fixture.run(&["run-task", SPEC, "--exec"]));
        assert_eq!(fixture.state()["status"], "history_review_required");
        fixture
    }

    fn hold_external(self) -> Self {
        assert_success(&self.run(&["run-task", SPEC, "--pre-only"]));
        self.write("service.py", "def keep():\n    return 2\n");
        self.index();
        assert_success(&self.run(&["verification", "run", SPEC, "--id=unit", "--json"]));
        self.write(REPORT, &json!({
            "schema_version":1,"spec":SPEC,"status":"complete","phases":[],
            "files_modified":["service.py"],"claims":[],"defects":[],
            "verifications":[{"cmd":"./.mastermind/check.sh","result":"pass","observed":{"exit_code":0}}]
        }).to_string());
        assert_success(&self.run(&["run-task", SPEC, "--post-only"]));
        assert_eq!(self.state()["status"], "history_review_required");
        self
    }

    fn mode(&self, mode: &str) {
        fs::write(self.harness.join("mode"), mode).unwrap();
    }
    fn write(&self, relative: &str, body: &str) {
        let path = self.root.join(relative);
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
            .env("FAKE_CLAUDE", self.bin.join("claude"))
            .env("TEST_HELPER", std::env::current_exe().unwrap())
            .env("MMCG_TEST_BIN", env!("CARGO_BIN_EXE_mmcg"))
            .env("SYNTHETIC_AUTH", "PRIVATE_NATIVE_REVIEW_AUTH");
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
    fn git(&self, args: &[&str]) -> Output {
        let output = self.command("/usr/bin/git").args(args).output().unwrap();
        assert_success(&output);
        output
    }
    fn index(&self) {
        assert_success(&self.run(&["index", "."]));
    }
    fn native_review(&self, options: &[&str]) -> Output {
        self.cli(&["review-task", "run", SPEC, "--json"])
            .args(options)
            .output()
            .unwrap()
    }
    fn review_resume(&self, options: &[&str]) -> Output {
        self.cli(&["run-task", SPEC, "--auto-review"])
            .args(options)
            .output()
            .unwrap()
    }
    fn value(&self, relative: &str) -> Value {
        serde_json::from_slice(&fs::read(self.root.join(relative)).unwrap()).unwrap()
    }
    fn state(&self) -> Value {
        self.value(STATE)
    }
    fn calls(&self, kind: &str) -> u32 {
        fs::read_to_string(self.harness.join(format!("{kind}-calls")))
            .map(|text| text.parse().unwrap())
            .unwrap_or(0)
    }
    fn status(&self) -> (Output, Value) {
        let output = self.run(&["review-task", "status", SPEC, "--json"]);
        let value = parse(&output);
        (output, value)
    }
    fn manual_positive(&self) -> Value {
        let output = self.run(&["review-task", "prepare", SPEC, "--json"]);
        assert_success(&output);
        let request = parse(&output);
        self.write(INPUT, &judged(&request["draft"], true).to_string());
        let output = self.run(&["review-task", "submit", SPEC, "--report", INPUT, "--json"]);
        assert_success(&output);
        let accepted = parse(&output);
        assert_eq!(accepted["status"], "accepted");
        accepted
    }
    fn finish_markdown(&self) {
        self.write(HISTORY, &format!(
            "# Legacy history review\n\n- **Audit snapshot:** {}\n- **Context:** not applicable\n- **Lesson:** not applicable\n- **Reason:** the local return-value change introduces no durable project knowledge\n",
            self.state()["history_snapshot_sha256"].as_str().unwrap()
        ));
    }
    fn spawn_review(&self) -> Child {
        self.cli(&["review-task", "run", SPEC, "--timeout", "30", "--json"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }
    fn await_started(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !self.harness.join("review-started").exists() {
            assert!(Instant::now() < deadline, "fake reviewer did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
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

fn parse(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid JSON: {error}: {output:?}"))
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn judged(template: &Value, human: bool) -> Value {
    let mut report = template.clone();
    if human {
        report["reviewer"] = json!({"kind":"human", "name":"Synthetic prior reviewer"});
    }
    // Deliberately authored assessment of this tiny fixture, never a verdict
    // inferred by the controller from a successful verification command.
    for item in report["criteria"].as_array_mut().unwrap() {
        item["status"] = json!("satisfied");
        item["reason"] = json!("The existing zero-argument function has one return expression, now the literal two, and the declared check supports that behavior.");
        item["evidence"] = json!(["spec", "check:unit"]);
    }
    report["verification_quality"] = json!({
        "status":"satisfied",
        "reason":"The declared check rejects the original return value and runs after the implementation edit.",
        "evidence":["check:unit"]
    });
    report["scope_control"] = json!({
        "status":"satisfied",
        "reason":"The worktree changes only the declared service function and no other tracked product file.",
        "evidence":["worktree", "spec"]
    });
    report["proportionality"] = json!({
        "status":"satisfied",
        "reason":"Replacing one return literal is sufficient and introduces no new abstraction or dependency.",
        "evidence":["worktree", "executor-report"]
    });
    report["history"] = json!({
        "context":{"decision":"no_change", "reason":"The fixture changes one return literal without changing a durable interface or architecture, so the current canonical context needs no further update.", "evidence":["knowledge:context","spec"]},
        "lessons":{"decision":"no_change", "reason":"The routine literal correction introduces no reusable lesson beyond the current canonical lessons file.", "evidence":["knowledge:lessons","check:unit"]}
    });
    report
}

fn review_packet(input: &str) -> (&str, Value) {
    let wire = input
        .rsplit_once("<mastermind-review-json>\n")
        .unwrap()
        .1
        .strip_suffix("\n</mastermind-review-json>\n")
        .unwrap();
    (wire, serde_json::from_str(wire).unwrap())
}

fn no_running_process(pid: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let output = Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        let status = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || status.trim().is_empty() || status.trim().starts_with('Z') {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native descendant remains active: {pid} {status}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

// The fake shell calls only this ignored entrypoint in a child test process,
// avoiding a runtime Python/jq dependency. Its harness output is redirected,
// while native stdout contains only the generated JSONL file.
#[test]
#[ignore = "child fixture entrypoint, launched by the fake native client"]
fn native_fixture_helper() {
    let harness = PathBuf::from(std::env::var_os("HARNESS").unwrap());
    let mode = fs::read_to_string(harness.join("mode")).unwrap();
    let kind = std::env::var("MMCG_REVIEW_FIXTURE_KIND").unwrap();
    let attempt: u32 = std::env::var("MMCG_REVIEW_FIXTURE_ATTEMPT")
        .unwrap()
        .parse()
        .unwrap();
    let root = std::env::current_dir().unwrap().canonicalize().unwrap();
    let input = fs::read_to_string(harness.join(format!("{kind}-stdin-{attempt}"))).unwrap();
    let reviewer = kind == "reviewer";
    let session = format!("synthetic-{kind}-session-{attempt}");
    let mut init = json!({
        "type":"system", "subtype":"init", "cwd":root,
        "session_id":session, "model":MODEL, "claude_code_version":"2.1.267",
        "permissionMode": if reviewer {"dontAsk"} else {"acceptEdits"},
        "tools": if reviewer {json!(["Read","Grep","Glob"])}
            else {json!(["Read","Edit","Write","Grep","Glob","Bash"])},
        "mcp_servers":[]
    });
    let mut events = Vec::new();
    let response = if reviewer {
        let (_, packet) = review_packet(&input);
        let mut assessment = judged(&packet["output_template"], false);
        assert!(assessment.get("reviewer").is_none());
        if mode.starts_with("follow_up_") && (attempt == 1 || mode == "follow_up_negative") {
            assessment["criteria"][0]["status"] = json!("unsatisfied");
            assessment["criteria"][0]["reason"] = json!("The conditional return expression selects three because its condition is false. The declared criterion requires the existing function to return two.");
            assessment["criteria"][0]["evidence"] = json!(["spec", "worktree", "check:unit"]);
        }
        match mode.as_str() {
            "follow_up_unknown" => {
                assessment["criteria"][0]["status"] = json!("unknown");
                assessment["criteria"][0]["reason"] = json!(
                    "The reviewer has insufficient evidence to determine the runtime behavior."
                );
            }
            "follow_up_verification_unknown" => {
                assessment["verification_quality"]["status"] = json!("unknown");
            }
            "follow_up_scope" => {
                assessment["scope_control"]["status"] = json!("unsatisfied");
                assessment["scope_control"]["reason"] = json!("The proposed remedy would require changing an undeclared interface, so a planner must revise the contract first.");
            }
            "follow_up_history" => {
                assessment["history"]["lessons"]["decision"] = json!("update_required");
                assessment["history"]["lessons"]["reason"] = json!("The reviewer requests a durable lesson that requires separate inspection before changing the knowledge file.");
            }
            "negative" | "repair_then_negative" => {
                assessment["criteria"][0]["status"] = json!("unsatisfied");
                assessment["criteria"][0]["reason"] = json!("The declared check inspects source text only, so this fixture reviewer rejects its support for the intended runtime behavior.");
            }
            "unknown" => {
                assessment["verification_quality"] = json!({
                    "status":"unknown",
                    "reason":"The available evidence does not establish runtime coverage for the requested behavior.",
                    "evidence":[]
                });
            }
            "history_unknown" => {
                assessment["history"]["context"] = json!({
                    "decision":"unknown", "reason":"The fixture reviewer has not inspected whether the canonical context needs a further update.", "evidence":[]
                });
            }
            "history_update" => {
                assessment["history"]["lessons"] = json!({
                    "decision":"update_required", "reason":"The canonical lessons file still needs the concrete service lesson identified by this fixture reviewer.", "evidence":["knowledge:lessons"]
                });
            }
            "missing_history" => {
                assessment.as_object_mut().unwrap().remove("history");
            }
            "changed_code" => {
                fs::write(root.join("service.py"), "def keep():\n    return 3\n").unwrap();
            }
            "changed_hidden_dependency" => {
                fs::write(
                    root.join("dependency.txt"),
                    "changed behind the Git assume-unchanged flag\n",
                )
                .unwrap();
            }
            "changed_context" | "created_context" => {
                fs::write(
                    root.join(CONTEXT),
                    "# Context changed during native review\n",
                )
                .unwrap();
            }
            "deleted_context" => {
                fs::remove_file(root.join(CONTEXT)).unwrap();
            }
            "changed_lessons" | "created_lessons" => {
                fs::write(
                    root.join(LESSONS),
                    "# Lessons changed during native review\n",
                )
                .unwrap();
            }
            "deleted_lessons" => {
                fs::remove_file(root.join(LESSONS)).unwrap();
            }
            "changed_native_executable" => {
                writeln!(
                    fs::OpenOptions::new()
                        .append(true)
                        .open(std::env::var_os("FAKE_CLAUDE").unwrap())
                        .unwrap(),
                    "\n# Native executable changed during review."
                )
                .unwrap();
            }
            "write_init" => init["tools"].as_array_mut().unwrap().push(json!("Write")),
            "toolsearch_init" => init["tools"]
                .as_array_mut()
                .unwrap()
                .push(json!("ToolSearch")),
            "mcp_init" => {
                init["mcp_servers"] = json!([{"name":"unexpected", "status":"connected"}])
            }
            _ => {}
        }
        events.push(init);
        if mode == "forbidden_tool" {
            events.push(json!({"type":"assistant", "session_id":session,
            "message":{"role":"assistant", "content":[{
                "type":"tool_use", "id":"synthetic-forbidden-tool", "name":"Bash",
                "input":{"command":"PRIVATE_REVIEW_TOOL_COMMAND"}
            }]}}));
        }
        if mode == "assistant_error" {
            events.push(json!({"type":"assistant", "session_id":session,
                "error":"authentication_failed", "message":{"role":"assistant", "content":[]}}));
        }
        if matches!(mode.as_str(), "invalid_json" | "missing_result") {
            // A plausible answer in assistant text and a file must never replace
            // the protocol's required terminal result.
            events.push(json!({"type":"assistant", "session_id":session,
                "message":{"role":"assistant", "content":[{"type":"text","text":assessment.to_string()}]}}));
            fs::write(
                root.join(format!("{TASK}/review-output.json")),
                assessment.to_string(),
            )
            .unwrap();
        }
        if mode == "invalid_json" {
            "{not valid JSON".into()
        } else {
            // Preserve significant whitespace to test exact terminal-string
            // hashing independently of canonical report serialization.
            serde_json::to_string_pretty(&assessment).unwrap()
        }
    } else {
        let fail = (mode == "repair_then_negative" && attempt == 1)
            || (mode == "follow_up_failed_check" && attempt == 2);
        if mode == "repair_then_negative" && attempt > 1 {
            assert!(input.contains("<mastermind-repair-json>"));
        }
        if mode.starts_with("follow_up_") && attempt == 2 {
            let feedback = input
                .split_once("<mastermind-semantic-follow-up-json>\n")
                .unwrap()
                .1
                .split_once("\n</mastermind-semantic-follow-up-json>")
                .unwrap()
                .0;
            let feedback: Value = serde_json::from_str(feedback).unwrap();
            assert_eq!(
                feedback["failed_checks"],
                json!([]),
                "semantic rejection is not a failed command"
            );
            assert_eq!(feedback["source_iteration"], 1);
            assert_eq!(feedback["source_binding"]["spec_path"], SPEC);
            let review = fs::read(root.join(REVIEW)).unwrap();
            assert_eq!(feedback["semantic_review"]["review_revision"], sha(&review));
            fs::write(harness.join("source-semantic-review.json"), review).unwrap();
            let state: Value =
                serde_json::from_slice(&fs::read(root.join(STATE)).unwrap()).unwrap();
            assert!(
                state["semantic_review_sha256"].is_null(),
                "new preflight must revoke the old review pin"
            );
        }
        // The text check deliberately passes this wrong branch. The synthetic
        // reviewer authors a separate source-level objection; this is a control
        // flow regression, not an estimate of real reviewer accuracy.
        let implementation = if mode.starts_with("follow_up_") && attempt == 1 {
            "def keep():\n    return 2 if False else 3\n".to_string()
        } else {
            format!("def keep():\n    return {}\n", if fail { 3 } else { 2 })
        };
        fs::write(root.join("service.py"), implementation).unwrap();
        if mode == "packet_overflow" {
            fs::OpenOptions::new()
                .append(true)
                .open(root.join("service.py"))
                .unwrap()
                .write_all(
                    "    # synthetic source evidence line\n"
                        .repeat(2500)
                        .as_bytes(),
                )
                .unwrap();
        }
        let command = |args: &[&str]| {
            Command::new(std::env::var_os("MMCG_TEST_BIN").unwrap())
                .args(["--index", ".mastermind/index.db"])
                .args(args)
                .output()
                .unwrap()
        };
        assert_success(&command(&["index", "."]));
        let check = command(&["verification", "run", SPEC, "--id", "unit", "--json"]);
        assert_eq!(check.status.success(), !fail, "{check:?}");
        let defects = if fail {
            json!([{
                "kind":"implementation_defect", "phase":"implementation",
                "details":"The service returns three instead of two.",
                "remediation_hint":"Correct the return literal."
            }])
        } else {
            json!([])
        };
        let report = json!({
            "schema_version":1, "spec":SPEC,
            "status": if fail {"partial"} else {"complete"}, "phases":[],
            "files_modified":["service.py"], "claims":[], "defects":defects,
            "verifications":[{"cmd":"./.mastermind/check.sh", "result":if fail {"fail"} else {"pass"},
                "observed":{"exit_code":if fail {9} else {0}}}]
        });
        fs::write(root.join(REPORT), report.to_string()).unwrap();
        events.push(init);
        "Synthetic execution complete".into()
    };
    fs::write(
        harness.join(format!("{kind}-response-{attempt}.json")),
        &response,
    )
    .unwrap();
    let mut result = json!({
        "type":"result", "subtype":"success", "is_error":false,
        "session_id":session, "result":response, "permission_denials":[]
    });
    if reviewer && mode == "denial" {
        result["permission_denials"] =
            json!([{"tool_name":"Read", "tool_input":{"file_path":"PRIVATE_DENIED_PATH"}}]);
    }
    if !(reviewer && mode == "missing_result") {
        events.push(result.clone());
    }
    if reviewer && mode == "duplicate_result" {
        events.push(result);
    }
    let stream = events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(
        harness.join(format!("{kind}-stream-{attempt}.jsonl")),
        stream,
    )
    .unwrap();
}

#[test]
fn reviewed_completion_refreshes_only_committed_profile_evidence_without_a_model() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.home.join(".mastermind/style.md")).unwrap();
    fixture.write(".git/info/exclude", ".claude/settings.local.json\n");
    assert_success(&fixture.run(&["miner", "access", "grant", "--client", "claude"]));
    assert_success(&fixture.run(&["miner", "hooks", "setup", "--client", "claude", "--write"]));
    let fixture = fixture.hold_external();
    fixture.manual_positive();
    let head = String::from_utf8(fixture.git(&["rev-parse", "HEAD"]).stdout).unwrap();
    assert_success(&fixture.run(&["run-task", SPEC]));
    let state = fixture.state();
    assert_eq!(state["status"], "learned");
    let refresh = &state["profile_refresh"];
    assert_eq!(refresh["status"], "refreshed");
    assert_eq!(refresh["source"], "committed_git");
    assert_eq!(refresh["source_snapshot"], head.trim());
    assert_eq!(refresh["repo_commits"], 1);
    assert_eq!(refresh["model"], false);
    assert_eq!(refresh["personal_claims_accepted"], 0);
    assert_eq!(fixture.calls("reviewer"), 0);
    let conn = rusqlite::Connection::open(fixture.home.join(".mastermind/style.db")).unwrap();
    for table in [
        "feedback_acceptance",
        "persona_habit_observation",
        "persona_review_event",
    ] {
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    let before = fs::read(fixture.root.join(STATE)).unwrap();
    assert_success(&fixture.run(&["run-task", SPEC]));
    assert_eq!(fs::read(fixture.root.join(STATE)).unwrap(), before);
}

#[test]
fn standalone_native_review_binds_exact_input_and_result_and_preserves_executor_evidence() {
    let fixture = Fixture::held();
    let executor = fs::read(fixture.root.join(EXECUTOR_RECEIPT)).unwrap();
    let markdown = fs::read(fixture.root.join(HISTORY)).unwrap();
    let config = fs::read(fixture.home.join(".claude/settings.json")).unwrap();
    let output = fixture.native_review(&["--timeout", "20", "--max-turns", "3"]);
    assert_success(&output);
    let accepted = parse(&output);
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["history_status"], "resolved");
    assert_eq!(accepted["overall_task_completion"], "not_evaluated");
    let follow_up = fixture.run(&["review-task", "follow-up", SPEC, "--json"]);
    assert_success(&follow_up);
    let follow_up = parse(&follow_up);
    assert_eq!(follow_up["next_action"], "complete");
    assert_eq!(follow_up["review_revision"], accepted["review_revision"]);
    assert_eq!(follow_up["review"]["reviewer"]["kind"], "llm");
    assert_eq!(fixture.calls("reviewer"), 1);
    let wrapper_bytes = fs::read(fixture.root.join(NATIVE_RECEIPT)).unwrap();
    let wrapper: Value = serde_json::from_slice(&wrapper_bytes).unwrap();
    assert_eq!(wrapper["status"], "passed", "{wrapper}");
    let native = &wrapper["invocation"];
    assert_eq!(native["status"], "passed");
    assert_eq!(native["agent"]["role"], "reviewer");
    assert_eq!(
        native["policy"]["builtin_tools"],
        json!(["Read", "Grep", "Glob"])
    );
    assert_eq!(native["native"]["init"]["permission_mode"], "dontAsk");
    assert_eq!(native["native"]["init"]["model"], MODEL);
    assert_eq!(native["options"]["max_turns"], 3);
    assert_eq!(native["options"]["wall_timeout_secs"], 20);
    assert!(native["options"]["profile_client"].is_null());

    let input = fs::read_to_string(fixture.harness.join("reviewer-stdin-1")).unwrap();
    let (wire, packet) = review_packet(&input);
    assert_eq!(
        native["context_delivery"]["prompt_sha256"],
        sha(input.as_bytes())
    );
    assert_eq!(native["context_delivery"]["prompt_bytes"], input.len());
    assert_eq!(native["context_delivery"]["bytes_offered"], input.len());
    assert_eq!(
        native["context_delivery"]["context_wire_sha256"],
        sha(wire.as_bytes())
    );
    assert_eq!(native["context_delivery"]["context_bytes"], wire.len());
    assert_eq!(
        native["context_delivery"]["context_revision"],
        packet["target_revision"]
    );
    assert_eq!(
        native["context_delivery"]["input_origin"],
        "controller_generated"
    );
    assert_eq!(native["context_delivery"]["model_use"], "unknown");
    assert_eq!(wrapper["target_revision"], packet["target_revision"]);
    assert_eq!(
        wrapper["expected_review_revision"],
        packet["expected_review_revision"]
    );
    assert_eq!(packet["repository_content_untrusted"], true);
    assert!(packet["output_template"].get("reviewer").is_none());
    assert_eq!(
        packet["output_template"]["criteria"][0]["status"],
        "unknown"
    );
    for kind in ["context", "lessons"] {
        assert_eq!(
            packet["output_template"]["history"][kind]["decision"],
            "unknown"
        );
        assert!(packet["target"]["evidence"]
            .get(format!("knowledge:{kind}"))
            .is_some());
    }
    assert!(packet["changes"]["diff"]
        .as_str()
        .unwrap()
        .contains("return 2"));
    assert!(packet["changes"]["paths"]
        .as_array()
        .unwrap()
        .contains(&json!("service.py")));
    assert!(!input.contains("PRIVATE_REVIEW_PROFILE_NOT_AUTHORIZED"));
    assert!(!input.contains("PRIVATE_NATIVE_REVIEW_AUTH"));

    let response_bytes = fs::read(fixture.harness.join("reviewer-response-1.json")).unwrap();
    assert_eq!(wrapper["response_sha256"], sha(&response_bytes));
    let response: Value = serde_json::from_slice(&response_bytes).unwrap();
    let record_bytes = fs::read(fixture.root.join(REVIEW)).unwrap();
    let record: Value = serde_json::from_slice(&record_bytes).unwrap();
    assert_eq!(record["report"]["reviewer"]["kind"], "llm");
    assert_eq!(record["report"]["reviewer"]["name"], MODEL);
    let mut without_reviewer = record["report"].clone();
    without_reviewer.as_object_mut().unwrap().remove("reviewer");
    assert_eq!(without_reviewer, response);
    let submission: mmcg::task_review::Submission =
        serde_json::from_value(record["report"].clone()).unwrap();
    let submission_hash = sha(&serde_json::to_vec(&submission).unwrap());
    assert_eq!(wrapper["submission_sha256"], submission_hash);
    assert_eq!(
        record["native_invocation"]["submission_sha256"],
        submission_hash
    );
    assert_eq!(
        record["native_invocation"]["receipt_sha256"],
        sha(&wrapper_bytes)
    );
    assert_eq!(
        record["native_invocation"]["invocation_id"],
        native["invocation_id"]
    );
    assert_ne!(wrapper["attempt_id"], native["invocation_id"]);
    assert_eq!(
        fixture.state()["semantic_review_sha256"],
        sha(&record_bytes)
    );
    assert_eq!(fixture.state()["status"], "history_review_required");
    assert_eq!(fixture.status().1["status"], "accepted");
    assert_eq!(
        fs::read(fixture.root.join(EXECUTOR_RECEIPT)).unwrap(),
        executor
    );
    assert_eq!(fs::read(fixture.root.join(HISTORY)).unwrap(), markdown);
    assert_eq!(
        fs::read(fixture.home.join(".claude/settings.json")).unwrap(),
        config
    );

    let argv = fs::read_to_string(fixture.harness.join("reviewer-argv-1")).unwrap();
    let args: Vec<&str> = argv.lines().collect();
    for flag in [
        "--safe-mode",
        "--restricted",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--no-chrome",
    ] {
        assert!(args.contains(&flag), "{flag}: {args:?}");
    }
    for (flag, value) in [
        ("--tools", "Read,Grep,Glob"),
        ("--permission-mode", "dontAsk"),
        ("--permission-prompts", "none"),
        ("--max-turns", "3"),
    ] {
        assert!(
            args.windows(2).any(|pair| pair == [flag, value]),
            "{flag}: {args:?}"
        );
    }
    let mcp = args.iter().position(|arg| *arg == "--mcp-config").unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(args[mcp + 1]).unwrap(),
        json!({"mcpServers":{}})
    );
    assert!(!args.contains(&"--bare"));
    assert!(!args.contains(&"--dangerously-skip-permissions"));
    assert_eq!(
        fs::read_to_string(fixture.harness.join("reviewer-home")).unwrap(),
        fixture.home.to_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(fixture.harness.join("reviewer-auth")).unwrap(),
        "PRIVATE_NATIVE_REVIEW_AUTH"
    );
    assert_eq!(
        fs::read_to_string(fixture.harness.join("reviewer-origin")).unwrap(),
        "controller"
    );
    let persisted = String::from_utf8(wrapper_bytes).unwrap();
    for private in [
        "PRIVATE_NATIVE_REVIEW_STDERR",
        "PRIVATE_NATIVE_REVIEW_AUTH",
        "PRIVATE_REVIEW_PROFILE_NOT_AUTHORIZED",
    ] {
        assert!(!persisted.contains(private), "{private}");
    }
    assert_eq!(fixture.calls("reviewer"), 1);
}

#[test]
fn a_new_native_request_revokes_old_approval_before_probe_and_on_protocol_failures() {
    let fixture = Fixture::held();
    for mode in [
        "unsupported_probe",
        "missing_safe_mode",
        "denial",
        "timeout",
        "invalid_json",
        "missing_result",
        "duplicate_result",
        "write_init",
        "toolsearch_init",
        "mcp_init",
        "forbidden_tool",
        "assistant_error",
        "missing_history",
    ] {
        fixture.mode("positive");
        fixture.manual_positive();
        let prior_record = fs::read(fixture.root.join(REVIEW)).unwrap();
        let calls = fixture.calls("reviewer");
        fixture.mode(mode);
        let timeout = if mode == "timeout" { "1" } else { "20" };
        let output = fixture.native_review(&["--timeout", timeout]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(parse(&output)["status"], "failed", "{mode}: {output:?}");
        assert!(
            fixture.state()["semantic_review_sha256"].is_null(),
            "{mode}"
        );
        let probe: Value =
            serde_json::from_slice(&fs::read(fixture.harness.join("probe-state.json")).unwrap())
                .unwrap();
        assert!(
            probe["semantic_review_sha256"].is_null(),
            "{mode}: approval still present during native probe"
        );
        assert_eq!(fixture.value(NATIVE_RECEIPT)["status"], "failed", "{mode}");
        assert_eq!(
            fs::read(fixture.root.join(REVIEW)).unwrap(),
            prior_record,
            "{mode}: old record is historical, not active approval"
        );
        assert!(!fixture.status().0.status.success(), "{mode}");
        if matches!(mode, "unsupported_probe" | "missing_safe_mode") {
            assert_eq!(
                fixture.calls("reviewer"),
                calls,
                "{mode}: unsupported native capability must stop before model process"
            );
        } else {
            assert_eq!(fixture.calls("reviewer"), calls + 1, "{mode}");
        }
        if mode == "timeout" {
            let pid = fs::read_to_string(fixture.harness.join("child")).unwrap();
            no_running_process(&pid);
        }
    }
}

#[test]
fn native_judgments_and_history_decisions_remain_separate_without_completing_the_task() {
    let fixture = Fixture::held();
    let markdown = fs::read(fixture.root.join(HISTORY)).unwrap();
    for (mode, semantic, history) in [
        ("negative", "blocked", "resolved"),
        ("unknown", "blocked", "resolved"),
        ("history_unknown", "accepted", "unknown"),
        ("history_update", "accepted", "update_required"),
    ] {
        fixture.mode("positive");
        let prior = fixture.manual_positive();
        fixture.mode(mode);
        let output = fixture.native_review(&[]);
        assert_eq!(
            output.status.code(),
            Some(if semantic == "accepted" { 0 } else { 1 }),
            "{mode}: {output:?}"
        );
        let assessment = parse(&output);
        assert_eq!(assessment["status"], semantic, "{mode}: {assessment}");
        assert_eq!(
            assessment["history_status"], history,
            "{mode}: {assessment}"
        );
        assert_ne!(
            assessment["review_revision"], prior["review_revision"],
            "{mode}"
        );
        assert_eq!(
            fixture.state()["semantic_review_sha256"],
            assessment["review_revision"]
        );
        assert_eq!(fixture.status().1["status"], semantic);
        assert_eq!(fixture.status().1["history_status"], history);
        assert_eq!(fixture.state()["status"], "history_review_required");
        assert_eq!(fs::read(fixture.root.join(HISTORY)).unwrap(), markdown);
    }
}

#[test]
fn review_cannot_approve_inputs_or_executable_changed_during_the_native_run() {
    for mode in [
        "changed_code",
        "changed_hidden_dependency",
        "changed_native_executable",
        "changed_context",
        "created_context",
        "deleted_context",
        "changed_lessons",
        "created_lessons",
        "deleted_lessons",
    ] {
        let with_sources = matches!(
            mode,
            "changed_context" | "deleted_context" | "changed_lessons" | "deleted_lessons"
        );
        let fixture = Fixture::new_with_history_sources(with_sources).hold();
        fixture.manual_positive();
        let index = fs::read(fixture.root.join(".git/index")).unwrap();
        let executor = fs::read(fixture.root.join(EXECUTOR_RECEIPT)).unwrap();
        fixture.mode(mode);
        let output = fixture.native_review(&[]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(parse(&output)["status"], "failed", "{mode}: {output:?}");
        assert!(
            fixture.state()["semantic_review_sha256"].is_null(),
            "{mode}"
        );
        assert_eq!(fixture.value(NATIVE_RECEIPT)["status"], "failed", "{mode}");
        assert_eq!(
            fs::read(fixture.root.join(EXECUTOR_RECEIPT)).unwrap(),
            executor
        );
        assert_eq!(fixture.calls("reviewer"), 1, "{mode}");
        if mode == "changed_hidden_dependency" {
            let status = fixture
                .command("/usr/bin/git")
                .env("GIT_OPTIONAL_LOCKS", "0")
                .args(["status", "--porcelain", "--", "dependency.txt"])
                .output()
                .unwrap();
            assert_success(&status);
            assert!(status.stdout.is_empty(), "{status:?}");
            assert_eq!(
                fs::read(fixture.root.join(".git/index")).unwrap(),
                index,
                "hidden bytes changed while the index and its assume-unchanged flag stayed fixed"
            );
        }
    }
}

struct ReleaseOnDrop(PathBuf);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        let _ = fs::write(&self.0, "release");
    }
}

#[test]
fn review_holds_the_controller_lock_until_native_completion() {
    for resume in [false, true] {
        let fixture = Fixture::held();
        fixture.mode("wait");
        let release = ReleaseOnDrop(fixture.harness.join("release"));
        let mut active = if resume {
            fixture
                .cli(&["run-task", SPEC, "--auto-review", "--review-timeout", "30"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        } else {
            fixture.spawn_review()
        };
        fixture.await_started();
        let state = fs::read(fixture.root.join(STATE)).unwrap();
        for args in [
            vec!["review-task", "run", SPEC, "--json"],
            vec!["run-task", SPEC, "--pre-only"],
            vec!["run-task", SPEC, "--auto-review"],
        ] {
            let output = fixture.run(&args);
            assert!(!output.status.success(), "resume={resume}: {output:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("busy"),
                "resume={resume}: {output:?}"
            );
            assert_eq!(fs::read(fixture.root.join(STATE)).unwrap(), state);
        }
        drop(release);
        assert!(active.wait().unwrap().success());
        assert_eq!(fixture.calls("reviewer"), 1);
        assert_eq!(fixture.calls("executor"), 1);
        assert_eq!(fixture.status().1["status"], "accepted");
        assert_eq!(
            fixture.state()["status"],
            if resume {
                "learned"
            } else {
                "history_review_required"
            }
        );
    }
}

#[test]
fn missing_or_corrupt_native_receipt_revokes_a_learned_review_in_completion_consumers() {
    for missing in [true, false] {
        let fixture = Fixture::held();
        assert_success(&fixture.native_review(&[]));
        assert_success(&fixture.run(&["run-task", SPEC]));
        assert_eq!(fixture.state()["status"], "learned");
        if missing {
            fs::remove_file(fixture.root.join(NATIVE_RECEIPT)).unwrap();
        } else {
            fixture.write(NATIVE_RECEIPT, "{corrupt");
        }
        let (output, status) = fixture.status();
        assert_eq!(output.status.code(), Some(1), "{status}");
        assert_ne!(status["status"], "accepted");
        assert_ne!(status["status"], "not_required");
        fixture.index();
        let next = fixture.run(&["next"]);
        assert_success(&next);
        let text = String::from_utf8_lossy(&next.stdout);
        assert!(text.contains("review-task"), "{text}");
        assert!(!text.contains("All tasks complete"), "{text}");
        let completion = fixture.run(&["run-task", SPEC]);
        assert!(!String::from_utf8_lossy(&completion.stdout).contains("Task complete —"));
        assert_ne!(fixture.state()["status"], "learned");
        let follow_up = fixture.run(&["review-task", "follow-up", SPEC, "--json"]);
        assert!(!follow_up.status.success(), "{follow_up:?}");
        assert!(follow_up.stdout.is_empty());
    }
}

#[test]
fn auto_review_runs_once_after_held_and_never_turns_negative_review_into_another_repair() {
    let fixture = Fixture::new();
    let output = fixture.run(&[
        "run-task",
        SPEC,
        "--exec",
        "--auto-review",
        "--review-timeout",
        "20",
        "--review-max-turns",
        "4",
    ]);
    assert_success(&output);
    assert_eq!(fixture.calls("executor"), 1);
    assert_eq!(fixture.calls("reviewer"), 1);
    assert_eq!(fixture.state()["status"], "learned");
    assert_eq!(fixture.status().1["status"], "accepted");
    assert_eq!(fixture.status().1["history_status"], "resolved");
    assert_eq!(
        fixture.value(NATIVE_RECEIPT)["invocation"]["options"]["max_turns"],
        4
    );
    assert!(!fs::read_to_string(fixture.root.join(HISTORY))
        .unwrap()
        .contains("**Lesson:** pending"));
    assert!(!fixture.root.join(CONTEXT).exists());
    assert!(!fixture.root.join(LESSONS).exists());

    let fixture = Fixture::new();
    fixture.mode("repair_then_negative");
    let output = fixture.run(&[
        "run-task",
        SPEC,
        "--exec",
        "--auto-repair",
        "--max-iterations",
        "3",
        "--auto-review",
    ]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fixture.calls("executor"), 2);
    assert_eq!(fixture.calls("reviewer"), 1);
    assert_eq!(fixture.state()["iteration"], 2);
    assert_eq!(fixture.state()["status"], "history_review_required");
    assert_eq!(fixture.status().1["status"], "blocked");

    for (mode, history) in [
        ("history_unknown", "unknown"),
        ("history_update", "update_required"),
    ] {
        let fixture = Fixture::new();
        fixture.mode(mode);
        let output = fixture.run(&["run-task", SPEC, "--exec", "--auto-repair", "--auto-review"]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(fixture.calls("executor"), 1, "{mode}");
        assert_eq!(fixture.calls("reviewer"), 1, "{mode}");
        assert_eq!(fixture.state()["status"], "history_review_required");
        let (output, status) = fixture.status();
        assert_success(&output);
        assert_eq!(status["status"], "accepted");
        assert_eq!(status["history_status"], history);
        fixture.finish_markdown();
        let completion = fixture.run(&["run-task", SPEC]);
        assert!(!String::from_utf8_lossy(&completion.stdout).contains("Task complete —"));
        assert_eq!(fixture.state()["status"], "history_review_required");
        assert_eq!(fixture.calls("reviewer"), 1);
    }

    let fixture = Fixture::new();
    fixture.mode("invalid_json");
    let output = fixture.run(&["run-task", SPEC, "--exec", "--auto-review"]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fixture.calls("executor"), 1);
    assert_eq!(fixture.calls("reviewer"), 1);
    assert_eq!(fixture.state()["status"], "history_review_required");
    assert_eq!(fixture.state()["last_artifact"], "review-invocation.json");
    assert_eq!(
        fixture.state()["blocking_reason"],
        fixture.value(NATIVE_RECEIPT)["reason"]
    );
    assert!(fixture.state()["semantic_review_sha256"].is_null());
    assert!(!fixture.root.join(REVIEW).exists());
}

#[test]
fn auto_follow_up_retries_one_current_criterion_with_bound_feedback_and_fresh_review() {
    let fixture = Fixture::new();
    fixture.mode("follow_up_positive");
    let output = fixture.run(&[
        "run-task",
        SPEC,
        "--exec",
        "--auto-review",
        "--auto-follow-up",
        "--max-iterations",
        "3",
    ]);
    assert_success(&output);
    assert_eq!(fixture.calls("executor"), 2);
    assert_eq!(fixture.calls("reviewer"), 2);
    assert_eq!(fixture.state()["iteration"], 2);
    assert_eq!(fixture.state()["status"], "learned");
    assert_eq!(fixture.status().1["status"], "accepted");
    assert_eq!(
        fs::read_to_string(fixture.root.join(".mastermind/check-runs")).unwrap(),
        "run\nrun\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("service.py")).unwrap(),
        "def keep():\n    return 2\n"
    );
    let first = fs::read_to_string(fixture.harness.join("executor-stdin-1")).unwrap();
    let second = fs::read_to_string(fixture.harness.join("executor-stdin-2")).unwrap();
    assert!(!first.contains("<mastermind-semantic-follow-up-json>"));
    assert!(!second.contains("<mastermind-repair-json>"));
    let wire = second
        .split_once("<mastermind-semantic-follow-up-json>\n")
        .unwrap()
        .1
        .split_once("\n</mastermind-semantic-follow-up-json>")
        .unwrap()
        .0;
    let feedback: Value = serde_json::from_str(wire).unwrap();
    assert_eq!(feedback["failed_checks"], json!([]));
    assert_eq!(feedback["unmet_criteria"], json!(["service-result"]));
    assert_eq!(feedback["source_iteration"], 1);
    assert_eq!(feedback["source_binding"]["iteration"], 1);
    let source: Value = serde_json::from_slice(
        &fs::read(fixture.harness.join("source-semantic-review.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        feedback["semantic_review"]["target_revision"],
        source["report"]["target_revision"]
    );
    assert_eq!(
        feedback["semantic_review"]["target"]["binding"],
        source["target"]["binding"]
    );
    assert_eq!(feedback["semantic_review"]["kind"], "semantic_follow_up");
    assert_eq!(
        feedback["semantic_review"]["repository_content_untrusted"],
        true
    );
    assert_eq!(
        feedback["semantic_review"]["semantic_accuracy"],
        "reviewer_assertion_not_independently_verified"
    );
    assert_eq!(
        feedback["semantic_review"]["working_directory"],
        fixture.root.to_str().unwrap()
    );
    assert_eq!(
        feedback["semantic_review"]["source_feedback_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_ne!(
        feedback["semantic_review"]["review_revision"],
        fixture.state()["semantic_review_sha256"]
    );
    assert_eq!(
        fixture.value(EXECUTOR_RECEIPT)["context_delivery"]["prompt_sha256"],
        sha(second.as_bytes())
    );
    assert_eq!(
        fixture.value(NATIVE_RECEIPT)["invocation"]["binding"]["iteration"],
        2
    );
    for secret in [
        "PRIVATE_REVIEW_PROFILE_NOT_AUTHORIZED",
        "PRIVATE_NATIVE_REVIEW_AUTH",
        "PRIVATE_REVIEW_CHECK_OUTPUT",
        "PRIVATE_NATIVE_REVIEW_STDERR",
    ] {
        assert!(!second.contains(secret), "{secret}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.matches("Task complete —").count(), 1);
}

#[test]
fn auto_follow_up_stops_on_unknown_history_scope_and_native_permission_failures() {
    for mode in [
        "follow_up_unknown",
        "follow_up_verification_unknown",
        "follow_up_scope",
        "follow_up_history",
        "denial",
    ] {
        let fixture = Fixture::new();
        fixture.mode(mode);
        let output = fixture.run(&[
            "run-task",
            SPEC,
            "--exec",
            "--auto-review",
            "--auto-follow-up",
        ]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(fixture.calls("executor"), 1, "{mode}");
        assert_eq!(fixture.calls("reviewer"), 1, "{mode}");
        assert_eq!(fixture.state()["iteration"], 1, "{mode}");
        assert_eq!(
            fixture.state()["status"],
            "history_review_required",
            "{mode}"
        );
        assert!(!fixture.harness.join("executor-stdin-2").exists());
    }
}

#[test]
fn auto_follow_up_shares_the_iteration_budget_and_never_repeats_semantic_rejection() {
    for (mode, budget, repair, executors, reviewers) in [
        ("follow_up_positive", "1", false, 1, 1),
        ("repair_then_negative", "2", true, 2, 1),
        ("follow_up_negative", "5", false, 2, 2),
    ] {
        let fixture = Fixture::new();
        fixture.mode(mode);
        let mut args = vec![
            "run-task",
            SPEC,
            "--exec",
            "--auto-review",
            "--auto-follow-up",
            "--max-iterations",
            budget,
        ];
        if repair {
            args.push("--auto-repair");
        }
        let output = fixture.run(&args);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(fixture.calls("executor"), executors, "{mode}");
        assert_eq!(fixture.calls("reviewer"), reviewers, "{mode}");
        assert_eq!(fixture.state()["iteration"], executors, "{mode}");
        assert_eq!(
            fixture.state()["status"],
            "history_review_required",
            "{mode}"
        );
        assert!(!fixture
            .harness
            .join(format!("executor-stdin-{}", executors + 1))
            .exists());
    }
}

#[test]
fn auto_follow_up_revalidates_review_and_hidden_sources_after_iteration_advance() {
    for mode in [
        "follow_up_stale_dependency",
        "follow_up_stale_review",
        "follow_up_stale_lessons",
        "follow_up_stale_check",
    ] {
        let fixture = Fixture::new();
        fixture.mode(mode);
        let output = fixture.run(&[
            "run-task",
            SPEC,
            "--exec",
            "--auto-review",
            "--auto-follow-up",
        ]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(
            fixture.calls("executor"),
            1,
            "stale source must stop before the second model call: {mode}"
        );
        assert_eq!(fixture.calls("reviewer"), 1, "{mode}");
        assert_eq!(
            fixture.state()["iteration"],
            2,
            "the mutation occurs after new preflight: {mode}"
        );
        assert_eq!(fixture.state()["next_step"], "run_preflight", "{mode}");
        assert!(
            fixture.state()["semantic_review_sha256"].is_null(),
            "{mode}"
        );
        assert_ne!(
            fixture.value(EXECUTOR_RECEIPT)["status"],
            "passed",
            "{mode}"
        );
        assert!(!fixture.harness.join("executor-stdin-2").exists());
    }
}

#[test]
fn auto_follow_up_requires_exec_review_and_a_finite_unforced_budget() {
    let fixture = Fixture::new();
    for options in [
        vec!["--auto-follow-up"],
        vec!["--exec", "--auto-follow-up"],
        vec!["--auto-review", "--auto-follow-up"],
        vec![
            "--exec",
            "--auto-review",
            "--auto-follow-up",
            "--force-iteration",
        ],
        vec![
            "--exec",
            "--auto-review",
            "--auto-follow-up",
            "--max-iterations",
            "0",
        ],
        vec![
            "--exec",
            "--auto-review",
            "--auto-follow-up",
            "--max-iterations",
            "21",
        ],
    ] {
        let output = fixture
            .cli(&["run-task", SPEC])
            .args(&options)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{options:?}: {output:?}");
        assert_eq!(fixture.calls("executor"), 0);
        assert_eq!(fixture.calls("reviewer"), 0);
        assert!(!fixture.root.join(STATE).exists());
    }
}

#[test]
fn auto_follow_up_does_not_approve_or_retry_new_failed_checks() {
    let fixture = Fixture::new();
    fixture.mode("follow_up_failed_check");
    let output = fixture.run(&[
        "run-task",
        SPEC,
        "--exec",
        "--auto-review",
        "--auto-follow-up",
    ]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fixture.calls("executor"), 2);
    assert_eq!(fixture.calls("reviewer"), 1);
    assert_eq!(fixture.state()["iteration"], 2);
    assert_ne!(fixture.state()["status"], "learned");
    let check = fixture.value(&format!("{TASK}/verification/unit.json"));
    assert_eq!(check["status"], "failed");
    assert!(!fixture.harness.join("executor-stdin-3").exists());
}

#[test]
fn invalid_review_limits_or_auto_review_flags_never_spawn_a_native_client() {
    let fixture = Fixture::new();
    for options in [
        vec!["--auto-review"],
        vec!["--exec", "--auto-review", "--pre-only"],
        vec!["--exec", "--auto-review", "--post-only"],
        vec!["--exec", "--auto-review", "--review-timeout", "0"],
        vec!["--exec", "--auto-review", "--review-timeout", "7201"],
        vec!["--exec", "--auto-review", "--review-max-turns", "0"],
        vec!["--exec", "--auto-review", "--review-max-turns", "101"],
    ] {
        let output = fixture
            .cli(&["run-task", SPEC])
            .args(&options)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{options:?}: {output:?}");
        assert_eq!(fixture.calls("executor"), 0, "{options:?}");
        assert_eq!(fixture.calls("reviewer"), 0, "{options:?}");
    }
    let fixture = Fixture::held();
    for options in [
        ["--timeout", "0"],
        ["--timeout", "7201"],
        ["--max-turns", "0"],
        ["--max-turns", "101"],
    ] {
        let output = fixture.native_review(&options);
        assert!(!output.status.success(), "{options:?}: {output:?}");
        assert_eq!(fixture.calls("reviewer"), 0);
    }
}

#[test]
fn an_oversized_source_packet_revokes_prior_approval_before_the_reviewer_starts() {
    let fixture = Fixture::new();
    fixture.mode("packet_overflow");
    assert_success(&fixture.run(&["run-task", SPEC, "--exec"]));
    assert_eq!(fixture.state()["status"], "history_review_required");
    fixture.manual_positive();
    let old_record = fs::read(fixture.root.join(REVIEW)).unwrap();
    let executor = fs::read(fixture.root.join(EXECUTOR_RECEIPT)).unwrap();
    let output = fixture.native_review(&[]);
    assert!(!output.status.success(), "{output:?}");
    let failed = parse(&output);
    assert_eq!(failed["status"], "failed", "{failed}");
    assert!(
        failed["reason"].as_str().unwrap().contains("limit"),
        "{failed}"
    );
    assert_eq!(fixture.calls("reviewer"), 0);
    assert!(fixture.state()["semantic_review_sha256"].is_null());
    assert_eq!(fixture.value(NATIVE_RECEIPT)["status"], "failed");
    assert_eq!(fs::read(fixture.root.join(REVIEW)).unwrap(), old_record);
    assert_eq!(
        fs::read(fixture.root.join(EXECUTOR_RECEIPT)).unwrap(),
        executor
    );
}

fn assert_same_preflight(before: &Value, after: &Value) {
    for field in [
        "repository_identity",
        "spec_path",
        "spec_hash",
        "baseline_ref",
        "iteration",
        "started_at",
        "allow_no_index",
        "strict",
        "invocation_required",
        "held_snapshot_sha256",
        "held_snapshot_version",
        "history_snapshot_sha256",
    ] {
        assert_eq!(before[field], after[field], "review resume changed {field}");
    }
}

#[test]
fn review_resume_completes_manual_and_native_held_tasks_without_execution() {
    for native in [false, true] {
        let fixture = if native {
            Fixture::held()
        } else {
            Fixture::new().hold_external()
        };
        let before = fixture.state();
        let executor = fs::read(fixture.root.join(EXECUTOR_RECEIPT)).ok();
        let client = fs::read(fixture.bin.join("claude")).unwrap();
        let checks = fs::read(fixture.root.join(".mastermind/check-runs")).unwrap();
        let watched = [
            REPORT,
            ".mastermind/tasks/001-native-review/audit.md",
            ".mastermind/tasks/001-native-review/verification/unit.json",
        ];
        let mechanical: Vec<_> = watched
            .iter()
            .map(|path| fs::read(fixture.root.join(path)).unwrap())
            .collect();
        let output = fixture.review_resume(&[
            "--review-timeout",
            "20",
            "--review-max-turns",
            "2",
            "--strict",
            "--allow-no-index",
        ]);
        assert_success(&output);
        assert_eq!(fixture.state()["status"], "learned");
        assert_same_preflight(&before, &fixture.state());
        assert_eq!(fixture.calls("executor"), if native { 1 } else { 0 });
        assert_eq!(fixture.calls("reviewer"), 1);
        assert_eq!(
            fs::read(fixture.root.join(".mastermind/check-runs")).unwrap(),
            checks
        );
        assert_eq!(fs::read(fixture.root.join(EXECUTOR_RECEIPT)).ok(), executor);
        assert_eq!(fs::read(fixture.bin.join("claude")).unwrap(), client);
        for (path, bytes) in watched.iter().zip(mechanical) {
            assert_eq!(fs::read(fixture.root.join(path)).unwrap(), bytes, "{path}");
        }
        let receipt = fixture.value(NATIVE_RECEIPT);
        assert_eq!(receipt["status"], "passed");
        assert_eq!(receipt["invocation"]["agent"]["role"], "reviewer");
        assert_eq!(receipt["invocation"]["options"]["wall_timeout_secs"], 20);
        assert_eq!(receipt["invocation"]["options"]["max_turns"], 2);
        assert_eq!(
            receipt["invocation"]["binding"]["iteration"],
            before["iteration"]
        );
        assert_eq!(fixture.status().1["history_status"], "resolved");
    }
}

#[test]
fn review_resume_stops_after_one_unresolved_review_and_can_recheck_updated_lessons() {
    for mode in [
        "negative",
        "unknown",
        "history_unknown",
        "history_update",
        "invalid_json",
    ] {
        let fixture = Fixture::new().hold_external();
        let prior = fixture.manual_positive();
        let before = fixture.state();
        let checks = fs::read(fixture.root.join(".mastermind/check-runs")).unwrap();
        fixture.mode(mode);
        let output = fixture.review_resume(&[]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(
            fixture.state()["status"],
            "history_review_required",
            "{mode}"
        );
        assert_same_preflight(&before, &fixture.state());
        assert_eq!(fixture.calls("reviewer"), 1, "{mode}");
        assert_eq!(fixture.calls("executor"), 0, "{mode}");
        assert_eq!(
            fs::read(fixture.root.join(".mastermind/check-runs")).unwrap(),
            checks
        );
        assert!(!fixture.root.join(EXECUTOR_RECEIPT).exists());
        if mode == "invalid_json" {
            assert!(fixture.state()["semantic_review_sha256"].is_null());
            assert_eq!(fixture.value(NATIVE_RECEIPT)["status"], "failed");
        } else {
            assert_ne!(
                fixture.state()["semantic_review_sha256"],
                prior["review_revision"]
            );
            assert!(fixture.state()["semantic_review_sha256"].is_string());
            assert_eq!(fixture.value(NATIVE_RECEIPT)["status"], "passed");
        }
        fixture.finish_markdown();
        let completion = fixture.run(&["run-task", SPEC]);
        assert!(!String::from_utf8_lossy(&completion.stdout).contains("Task complete —"));
        assert_eq!(fixture.state()["status"], "history_review_required");
        assert_eq!(fixture.calls("reviewer"), 1);
        if mode == "history_update" {
            assert_eq!(fixture.status().1["history_status"], "update_required");
            let lesson = "# Project lessons\nThe existing service must return two; check it after changing the return expression.\n";
            fixture.write(LESSONS, lesson);
            fixture.mode("positive");
            assert_success(&fixture.review_resume(&[]));
            assert_eq!(fixture.state()["status"], "learned");
            assert_same_preflight(&before, &fixture.state());
            assert_eq!(fixture.calls("reviewer"), 2);
            assert_eq!(fixture.calls("executor"), 0);
            assert_eq!(
                fs::read(fixture.root.join(".mastermind/check-runs")).unwrap(),
                checks
            );
            assert_eq!(
                fixture.value(REVIEW)["target"]["project_history"]["sources"]["lessons"]["sha256"],
                sha(lesson.as_bytes())
            );
            assert_eq!(fixture.status().1["history_status"], "resolved");
        }
    }
}

#[test]
fn review_resume_rejects_nonpending_states_and_conflicting_flags_without_mutation() {
    for phase in ["missing", "approved", "learned", "needs_preflight"] {
        let fixture = match phase {
            "missing" => Fixture::new(),
            "approved" => {
                let fixture = Fixture::new();
                assert_success(&fixture.run(&["run-task", SPEC, "--pre-only"]));
                fixture
            }
            _ => Fixture::new().hold_external(),
        };
        if phase == "learned" {
            fixture.manual_positive();
            assert_success(&fixture.run(&["run-task", SPEC]));
            assert_eq!(fixture.state()["status"], "learned");
        } else if phase == "needs_preflight" {
            let mut state = fixture.state();
            state["status"] = json!("held");
            state["next_step"] = json!("run_preflight");
            fixture.write(STATE, &state.to_string());
        }
        let state = fs::read(fixture.root.join(STATE)).ok();
        let checks = fs::read(fixture.root.join(".mastermind/check-runs")).ok();
        let output = fixture.review_resume(&[]);
        assert!(!output.status.success(), "{phase}: {output:?}");
        assert_eq!(fs::read(fixture.root.join(STATE)).ok(), state, "{phase}");
        assert_eq!(
            fs::read(fixture.root.join(".mastermind/check-runs")).ok(),
            checks
        );
        assert_eq!(fixture.calls("reviewer"), 0, "{phase}");
        assert_eq!(fixture.calls("executor"), 0, "{phase}");
        assert!(!fixture.harness.join("probe-state.json").exists());
        assert!(!fixture.root.join(NATIVE_RECEIPT).exists());
        if phase == "learned" {
            assert_success(&fixture.run(&["run-task", SPEC]));
            assert_eq!(fs::read(fixture.root.join(STATE)).ok(), state);
        }
    }
    let fixture = Fixture::new().hold_external();
    fixture.manual_positive();
    let state = fs::read(fixture.root.join(STATE)).unwrap();
    let checks = fs::read(fixture.root.join(".mastermind/check-runs")).unwrap();
    for flag in [
        "--reset",
        "--force-iteration",
        "--pre-only",
        "--post-only",
        "--auto-repair",
    ] {
        let output = fixture.review_resume(&[flag]);
        assert!(!output.status.success(), "{flag}: {output:?}");
        assert_eq!(fs::read(fixture.root.join(STATE)).unwrap(), state, "{flag}");
        assert_eq!(
            fs::read(fixture.root.join(".mastermind/check-runs")).unwrap(),
            checks
        );
        assert_eq!(fixture.calls("reviewer"), 0, "{flag}");
        assert_eq!(fixture.calls("executor"), 0, "{flag}");
        assert!(!fixture.harness.join("probe-state.json").exists());
        assert!(!fixture.root.join(NATIVE_RECEIPT).exists());
    }
}

#[test]
fn review_resume_revokes_stale_pending_approval_before_any_native_or_check_launch() {
    for changed in ["missing_receipt", "hidden_dependency", "audit"] {
        let fixture = Fixture::new().hold_external();
        fixture.manual_positive();
        let before = fixture.state();
        let old_review = fs::read(fixture.root.join(REVIEW)).unwrap();
        let checks = fs::read(fixture.root.join(".mastermind/check-runs")).unwrap();
        match changed {
            "missing_receipt" => {
                fs::remove_file(fixture.root.join(format!("{TASK}/verification/unit.json")))
                    .unwrap()
            }
            "hidden_dependency" => fixture.write("dependency.txt", "changed hidden dependency\n"),
            "audit" => writeln!(
                fs::OpenOptions::new()
                    .append(true)
                    .open(fixture.root.join(format!("{TASK}/audit.md")))
                    .unwrap(),
                "\nChanged audit after review."
            )
            .unwrap(),
            _ => unreachable!(),
        }
        let output = fixture.review_resume(&[]);
        assert!(!output.status.success(), "{changed}: {output:?}");
        assert_eq!(
            fixture.state()["status"],
            "history_review_required",
            "{changed}"
        );
        assert!(
            fixture.state()["semantic_review_sha256"].is_null(),
            "{changed}"
        );
        assert_same_preflight(&before, &fixture.state());
        assert_eq!(
            fixture.value(NATIVE_RECEIPT)["status"],
            "failed",
            "{changed}"
        );
        assert_eq!(fixture.calls("reviewer"), 0, "{changed}");
        assert_eq!(fixture.calls("executor"), 0, "{changed}");
        assert!(!fixture.harness.join("probe-state.json").exists());
        assert!(!fixture.root.join(EXECUTOR_RECEIPT).exists());
        assert_eq!(
            fs::read(fixture.root.join(".mastermind/check-runs")).unwrap(),
            checks
        );
        assert_eq!(fs::read(fixture.root.join(REVIEW)).unwrap(), old_review);
    }
}
