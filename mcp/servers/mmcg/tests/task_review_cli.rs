//! Semantic review through the public CLI, using deliberately judged fixtures.
//! Passing checks never fill in a semantic verdict on behalf of the reviewer.

#![cfg(unix)]

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const TASK: &str = ".mastermind/tasks/001-review";
const SPEC: &str = ".mastermind/tasks/001-review/spec.md";
const STATE: &str = ".mastermind/tasks/001-review/state.json";
const REPORT: &str = ".mastermind/tasks/001-review/executor-report.md";
const REVIEW: &str = ".mastermind/tasks/001-review/semantic-review.json";
const INPUT: &str = ".mastermind/tasks/001-review/review-input.json";
const HISTORY: &str = ".mastermind/tasks/001-review/history-review.md";
const CONTEXT: &str = "CONTEXT.md";
const LESSONS: &str = ".mastermind/tasks/_lessons.md";
const EXECUTABLE: &str = ".mastermind/check.sh";
const PROBE: &str = r##"#!/bin/sh
set -eu
printf '%s\n' "$1" >> .mastermind/check-runs
case "$1" in
  unit) /usr/bin/grep -q 'return 2' service.py || exit 9 ;;
  integration) /usr/bin/grep -q '^def keep():' service.py || exit 8 ;;
  *) exit 7 ;;
esac
printf 'PRIVATE_REVIEW_CHECK_OUTPUT\n'
"##;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::new_with_history_sources(false)
    }

    fn new_with_history_sources(present: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&home).unwrap();
        let fixture = Self {
            root: root.canonicalize().unwrap(),
            home: home.canonicalize().unwrap(),
            _temp: temp,
        };
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
            vec!["config", "user.name", "Review Fixture"],
            vec!["config", "user.email", "review@example.invalid"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "core.hooksPath", ""],
            vec!["add", "."],
            vec!["commit", "-qm", "Synthetic review baseline"],
        ] {
            fixture.git(&args);
        }
        fixture.write(EXECUTABLE, PROBE);
        fs::set_permissions(fixture.path(EXECUTABLE), fs::Permissions::from_mode(0o700)).unwrap();
        let checks: Vec<Value> = ["unit", "integration"].into_iter().map(|id| json!({
            "cmd":format!("./{EXECUTABLE} {id}"),
            "run":{"id":id, "argv":[format!("./{EXECUTABLE}"),id], "cwd":".", "timeout_secs":10}
        })).collect();
        let metadata = json!({"mode":"verified", "touches":[{"file":"service.py", "symbols":["keep"]}],
            "verify":checks, "acceptance":[{"id":"service-result", "statement":"The existing keep function returns two.", "checks":["unit","integration"]}]});
        fixture.write(SPEC, &format!("---\n{}---\n# Semantic review fixture\n\n## Goals\nReturn two from the service.\n\n## Scope\nEdit the existing service.\n\n## Acceptance Criteria\nThe existing keep function returns two.\n\n## Tests Plan\nRun both declared checks.\n\n## Final Verification\nRun the checks after the edit.\n", serde_norway::to_string(&metadata).unwrap()));
        fixture.index();
        fixture
    }

    fn held() -> Self {
        Self::new().hold()
    }

    fn hold(self) -> Self {
        let fixture = self;
        fixture.git(&["update-index", "--assume-unchanged", "dependency.txt"]);
        assert_success(&fixture.run(&["run-task", SPEC, "--pre-only"]));
        fixture.write("service.py", "def keep():\n    return 2\n");
        fixture.index();
        for id in ["unit", "integration"] {
            assert_success(&fixture.run(&["verification", "run", SPEC, "--id", id, "--json"]));
        }
        let verifications: Vec<Value> = ["unit", "integration"].into_iter().map(|id| json!({
            "cmd":format!("./{EXECUTABLE} {id}"), "result":"pass", "observed":{"exit_code":0}
        })).collect();
        fixture.write(REPORT, &json!({"schema_version":1, "spec":SPEC, "status":"complete", "phases":[],
            "files_modified":["service.py"], "claims":[], "defects":[], "verifications":verifications}).to_string());
        assert_success(&fixture.run(&["run-task", SPEC, "--post-only"]));
        assert_eq!(fixture.state()["status"], "history_review_required");
        fixture
    }

    fn path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
    fn write(&self, path: &str, body: &str) {
        let path = self.path(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CODEX_HOME", self.home.join("codex"))
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("MMCG_GIT_TIMEOUT_MS", "20000");
        command
    }
    fn git(&self, args: &[&str]) -> Output {
        let output = self.command("/usr/bin/git").args(args).output().unwrap();
        assert_success(&output);
        output
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_mmcg"))
            .args(["--index", ".mastermind/index.db"])
            .args(args)
            .output()
            .unwrap()
    }
    fn index(&self) {
        assert_success(&self.run(&["index", "."]));
    }
    fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(self.path(STATE)).unwrap()).unwrap()
    }
    fn request(&self) -> Value {
        let output = self.run(&["review-task", "prepare", SPEC, "--json"]);
        assert_success(&output);
        parse(&output)
    }
    fn follow_up(&self) -> Value {
        let output = self.run(&["review-task", "follow-up", SPEC, "--json"]);
        assert_success(&output);
        parse(&output)
    }
    fn edit_frontmatter(&self, edit: impl FnOnce(&mut Value)) {
        let body = fs::read_to_string(self.path(SPEC)).unwrap();
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
    fn status(&self) -> (Output, Value) {
        let output = self.run(&["review-task", "status", SPEC, "--json"]);
        let value = parse(&output);
        (output, value)
    }
    fn submit(&self, report: &Value) -> (Output, Value) {
        self.write(INPUT, &report.to_string());
        let output = self.run(&["review-task", "submit", SPEC, "--report", INPUT, "--json"]);
        let value = parse(&output);
        (output, value)
    }
    fn submit_positive(&self) -> Value {
        let report = positive(&self.request());
        let (output, accepted) = self.submit(&report);
        assert_success(&output);
        assert_eq!(accepted["status"], "accepted");
        accepted
    }
    fn finish_markdown(&self) {
        // Deliberately author the old Markdown contract. For a new structured
        // review this must never substitute for typed history decisions.
        self.write(HISTORY, &format!(
            "# Legacy history review\n\n- **Audit snapshot:** {}\n- **Context:** not applicable\n- **Lesson:** not applicable\n- **Reason:** the local return-value change introduces no durable project knowledge\n",
            self.state()["history_snapshot_sha256"].as_str().unwrap()
        ));
    }
    fn require_blocked_completion(&self) {
        let output = self.run(&["run-task", SPEC]);
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("Task complete —"),
            "{output:?}"
        );
        assert_ne!(self.state()["status"], "learned");
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

fn positive(request: &Value) -> Value {
    let mut report = request["draft"].clone();
    report["reviewer"] = json!({"kind":"human", "name":"Synthetic service reviewer"});
    // This deliberately authored judgment applies only to the tiny fixture.
    for item in report["criteria"].as_array_mut().unwrap() {
        item["status"] = json!("satisfied");
        item["reason"] = json!("The existing zero-argument function has one return expression, now the literal two, and both declared checks support that change.");
        item["evidence"] = json!(["check:unit", "check:integration", "spec"]);
    }
    report["verification_quality"] = json!({"status":"satisfied", "reason":"The unit check rejects the original return value and the integration check verifies that the existing function remains available.", "evidence":["check:unit","check:integration"]});
    report["scope_control"] = json!({"status":"satisfied", "reason":"The worktree changes only the declared service function; no other tracked product file is modified.", "evidence":["worktree","spec"]});
    report["proportionality"] = json!({"status":"satisfied", "reason":"Changing one return literal is sufficient for this requirement and introduces no new abstraction or dependency.", "evidence":["worktree","executor-report"]});
    report["history"] = json!({
        "context":{"decision":"no_change", "reason":"The one-line service fix does not change any durable project interface or architecture; no further context update is needed for the current canonical file.", "evidence":["knowledge:context","spec"]},
        "lessons":{"decision":"no_change", "reason":"The fixture is a routine literal correction and adds no reusable engineering lesson beyond the current canonical file.", "evidence":["knowledge:lessons","check:unit"]}
    });
    report
}

#[test]
fn green_checks_and_history_markdown_do_not_supply_a_semantic_review() {
    let fixture = Fixture::held();
    fixture.finish_markdown();
    let post = fixture.run(&["run-task", SPEC, "--post-only"]);
    assert!(!String::from_utf8_lossy(&post.stdout).contains("Task complete —"));
    assert_ne!(fixture.state()["status"], "learned");
    fixture.finish_markdown();
    fixture.require_blocked_completion();
    let (output, status) = fixture.status();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(status["status"], "missing", "{status}");
    assert!(!fixture.path(REVIEW).exists());
}

#[test]
fn prepare_is_read_only_and_unknown_until_typed_positive_review_closes_across_processes() {
    let fixture = Fixture::new();
    assert!(!fixture
        .run(&["review-task", "prepare", SPEC, "--json"])
        .status
        .success());
    assert!(!fixture.path(REVIEW).exists());

    let fixture = Fixture::held();
    let before_state = fs::read(fixture.path(STATE)).unwrap();
    let before_checks = fs::read(fixture.path(".mastermind/check-runs")).unwrap();
    let request = fixture.request();
    assert_eq!(request["target_revision"].as_str().unwrap().len(), 64);
    assert!(request["expected_review_revision"].is_null());
    assert_eq!(
        request["draft"]["target_revision"],
        request["target_revision"]
    );
    assert_eq!(
        request["draft"]["expected_review_revision"],
        request["expected_review_revision"]
    );
    assert_eq!(request["draft"]["criteria"].as_array().unwrap().len(), 1);
    assert_eq!(request["draft"]["criteria"][0]["id"], "service-result");
    for pointer in [
        "/draft/criteria/0/status",
        "/draft/verification_quality/status",
        "/draft/scope_control/status",
        "/draft/proportionality/status",
    ] {
        assert_eq!(request.pointer(pointer).unwrap(), "unknown", "{pointer}");
    }
    for kind in ["context", "lessons"] {
        assert_eq!(request["draft"]["history"][kind]["decision"], "unknown");
        assert_eq!(request["draft"]["history"][kind]["reason"], "");
        assert_eq!(request["draft"]["history"][kind]["evidence"], json!([]));
    }
    assert_eq!(
        request["target"]["project_history"],
        json!({
            "schema_version":1,
            "sources":{
                "context":{"path":CONTEXT,"sha256":null},
                "lessons":{"path":LESSONS,"sha256":null}
            }
        })
    );
    let evidence = request["target"]["evidence"].as_object().unwrap();
    for reference in [
        "spec",
        "executor-report",
        "audit",
        "release",
        "worktree",
        "check:unit",
        "check:integration",
        "knowledge:context",
        "knowledge:lessons",
    ] {
        assert!(evidence.contains_key(reference), "{reference}: {request}");
    }
    assert!(!request.to_string().contains("PRIVATE_REVIEW_CHECK_OUTPUT"));
    assert_eq!(fs::read(fixture.path(STATE)).unwrap(), before_state);
    assert_eq!(
        fs::read(fixture.path(".mastermind/check-runs")).unwrap(),
        before_checks
    );
    assert!(!fixture.path(REVIEW).exists());
    let (output, accepted) = fixture.submit(&positive(&request));
    assert_success(&output);
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["history_status"], "resolved");
    assert_eq!(accepted["reviewer_identity_verified"], false);
    assert_eq!(
        accepted["semantic_accuracy"],
        "reviewer_assertion_not_independently_verified"
    );
    assert_eq!(accepted["overall_task_completion"], "not_evaluated");
    let revision = sha(&fs::read(fixture.path(REVIEW)).unwrap());
    assert_eq!(accepted["review_revision"], revision);
    assert_eq!(fixture.state()["semantic_review_sha256"], revision);
    assert_eq!(fixture.state()["semantic_review_required"], true);
    assert_eq!(fixture.state()["status"], "history_review_required");
    assert_success(&fixture.status().0);
    assert_eq!(fixture.status().1["status"], "accepted");
    let informational_markdown = fs::read(fixture.path(HISTORY)).unwrap();
    assert!(!String::from_utf8_lossy(&informational_markdown).contains("**Context:** pending"));
    assert_success(&fixture.run(&["run-task", SPEC]));
    assert_eq!(fixture.state()["status"], "learned");
    assert_success(&fixture.status().0);
    assert_eq!(fixture.status().1["status"], "accepted");
}

#[test]
fn repeated_postflight_preserves_current_review_and_keeps_ignored_lesson_changes_reviewable() {
    for lessons_changed in [false, true] {
        let fixture = Fixture::new_with_history_sources(true).hold();
        let accepted = fixture.submit_positive();
        let pinned = accepted["review_revision"].clone();
        let prior_review = fs::read(fixture.path(REVIEW)).unwrap();
        let prior_state = fixture.state();
        let observed_checks = fs::read(fixture.path(".mastermind/check-runs")).unwrap();
        let preserved_paths = [
            "service.py",
            "dependency.txt",
            CONTEXT,
            SPEC,
            REPORT,
            ".mastermind/tasks/001-review/verification/unit.json",
            ".mastermind/tasks/001-review/verification/integration.json",
        ];
        let preserved: Vec<_> = preserved_paths
            .iter()
            .map(|path| fs::read(fixture.path(path)).unwrap())
            .collect();
        let assert_preserved = || {
            assert_eq!(
                fs::read(fixture.path(".mastermind/check-runs")).unwrap(),
                observed_checks,
                "post-flight and review completion must not execute declared checks"
            );
            for (path, expected) in preserved_paths.iter().zip(&preserved) {
                assert_eq!(fs::read(fixture.path(path)).unwrap(), *expected, "{path}");
            }
        };

        if lessons_changed {
            fixture.git(&["check-ignore", "--quiet", LESSONS]);
            fixture.write(LESSONS, "# Project lessons\nFor this service fixture, observe the failing return-value check before replacing the literal and rerun it after the change.\n");
        }
        let post = fixture.run(&["run-task", SPEC, "--post-only"]);
        assert_success(&post);
        let state = fixture.state();
        assert_eq!(state["semantic_review_sha256"], pinned);
        assert_eq!(state["iteration"], prior_state["iteration"]);
        assert_eq!(state["risk"], "low");
        assert!(state["blocking_reason"].is_null());
        assert_eq!(fs::read(fixture.path(REVIEW)).unwrap(), prior_review);
        assert_preserved();

        if lessons_changed {
            assert_eq!(state["status"], "history_review_required");
            assert_eq!(state["next_step"], "review_history");
            let (output, stale) = fixture.status();
            assert!(!output.status.success());
            assert_eq!(stale["status"], "stale");
            // A held audit with an outdated canonical-knowledge assessment
            // remains directly reviewable; another mechanical audit is not needed.
            let fresh = fixture.request();
            assert_eq!(fresh["expected_review_revision"], pinned);
            assert_eq!(
                fresh["target"]["project_history"]["sources"]["lessons"]["sha256"],
                sha(&fs::read(fixture.path(LESSONS)).unwrap())
            );
            let (output, accepted) = fixture.submit(&positive(&fresh));
            assert_success(&output);
            assert_eq!(accepted["status"], "accepted");
            assert_eq!(accepted["history_status"], "resolved");
            assert_ne!(accepted["review_revision"], pinned);
            assert_success(&fixture.run(&["run-task", SPEC]));
            assert_eq!(fixture.state()["status"], "learned");
            assert_eq!(
                fixture.state()["semantic_review_sha256"],
                accepted["review_revision"]
            );
            assert_preserved();
        } else {
            assert_eq!(state["status"], "learned");
            assert_eq!(state["next_step"], "close");
            assert_success(&fixture.status().0);
        }
    }
}

#[test]
fn unknown_or_negative_judgments_are_stored_and_block_completion_despite_green_checks() {
    for unknown in [true, false] {
        let fixture = Fixture::held();
        let mut report = positive(&fixture.request());
        if unknown {
            report["criteria"][0]["status"] = json!("unknown");
            report["criteria"][0]["reason"] = json!("The reviewer has not evaluated whether these checks establish the required behavior.");
            report["criteria"][0]["evidence"] = json!([]);
        } else {
            report["verification_quality"]["status"] = json!("unsatisfied");
            report["verification_quality"]["reason"] = json!("Lexical matching alone is insufficient evidence for the behavior this reviewer requires.");
        }
        let (output, blocked) = fixture.submit(&report);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert_eq!(blocked["status"], "blocked");
        assert!(fixture.path(REVIEW).exists());
        assert_eq!(
            fixture.state()["semantic_review_sha256"],
            blocked["review_revision"]
        );
        assert_eq!(fixture.status().1["status"], "blocked");
        fixture.finish_markdown();
        fixture.require_blocked_completion();
    }
}

#[test]
fn target_changes_revoke_review_including_git_assume_unchanged_content() {
    for change in [
        "code",
        "hidden_dependency",
        "spec",
        "receipt",
        "executable",
        "executor_report",
    ] {
        let fixture = Fixture::held();
        fixture.submit_positive();
        fixture.finish_markdown();
        let reviewed_record = fs::read(fixture.path(REVIEW)).unwrap();
        match change {
            "code" => fixture.write("service.py", "def keep():\n    return 3\n"),
            "hidden_dependency" => {
                let before = fixture
                    .command("/usr/bin/git")
                    .env("GIT_OPTIONAL_LOCKS", "0")
                    .args(["status", "--porcelain", "--", "dependency.txt"])
                    .output()
                    .unwrap();
                assert_success(&before);
                assert!(before.stdout.is_empty());
                let index = fs::read(fixture.path(".git/index")).unwrap();
                fixture.write("dependency.txt", "a hidden dependency change\n");
                let after = fixture
                    .command("/usr/bin/git")
                    .env("GIT_OPTIONAL_LOCKS", "0")
                    .args(["status", "--porcelain", "--", "dependency.txt"])
                    .output()
                    .unwrap();
                assert_success(&after);
                assert!(after.stdout.is_empty());
                assert_eq!(fs::read(fixture.path(".git/index")).unwrap(), index);
            }
            "spec" => {
                let body = fs::read_to_string(fixture.path(SPEC)).unwrap();
                fixture.write(SPEC, &(body + "\nA newly added requirement.\n"));
            }
            "receipt" => {
                let path = format!("{TASK}/verification/unit.json");
                let mut receipt: Value =
                    serde_json::from_slice(&fs::read(fixture.path(&path)).unwrap()).unwrap();
                receipt["run_id"] = json!("f".repeat(32));
                fixture.write(&path, &receipt.to_string());
            }
            "executable" => fixture.write(
                EXECUTABLE,
                &(PROBE.to_owned() + "\n# A different executable revision.\n"),
            ),
            "executor_report" => {
                let mut report: Value =
                    serde_json::from_slice(&fs::read(fixture.path(REPORT)).unwrap()).unwrap();
                report["verifications"][0]["output_excerpt"] =
                    json!("The report was changed after review.");
                fixture.write(REPORT, &report.to_string());
            }
            _ => unreachable!(),
        }
        let state = fs::read(fixture.path(STATE)).unwrap();
        let (output, status) = fixture.status();
        assert_eq!(output.status.code(), Some(1), "{change}: {output:?}");
        assert!(
            matches!(status["status"].as_str(), Some("stale" | "unavailable")),
            "{change}: {status}"
        );
        assert_eq!(
            fs::read(fixture.path(STATE)).unwrap(),
            state,
            "status must remain read-only"
        );
        fixture.require_blocked_completion();
        assert_eq!(
            fixture.state()["semantic_review_required"],
            true,
            "{change}"
        );
        assert_eq!(
            fs::read(fixture.path(REVIEW)).unwrap(),
            reviewed_record,
            "{change}"
        );
    }
}

#[test]
fn compare_and_swap_rejects_a_queued_positive_after_a_newer_negative() {
    let fixture = Fixture::held();
    let accepted = fixture.submit_positive();
    let request = fixture.request();
    assert_eq!(
        request["expected_review_revision"],
        accepted["review_revision"]
    );
    let queued_positive = positive(&request);
    let mut negative = queued_positive.clone();
    negative["criteria"][0]["status"] = json!("unsatisfied");
    negative["criteria"][0]["reason"] = json!("The reviewer rejects the declared check mapping as insufficient support for the requirement.");
    let (output, blocked) = fixture.submit(&negative);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(blocked["status"], "blocked");
    assert_ne!(blocked["review_revision"], accepted["review_revision"]);
    let before = fs::read(fixture.path(REVIEW)).unwrap();
    fixture.write(INPUT, &queued_positive.to_string());
    let stale = fixture.run(&["review-task", "submit", SPEC, "--report", INPUT, "--json"]);
    assert!(!stale.status.success(), "{stale:?}");
    assert_eq!(fs::read(fixture.path(REVIEW)).unwrap(), before);
    assert_eq!(
        fixture.state()["semantic_review_sha256"],
        blocked["review_revision"]
    );
    assert_eq!(fixture.status().1["status"], "blocked");
    fixture.finish_markdown();
    fixture.require_blocked_completion();
}

#[test]
fn submit_rejects_a_prepared_assessment_after_its_evidence_changes() {
    let fixture = Fixture::held();
    let prepared = positive(&fixture.request());
    fixture.write("service.py", "def keep():\n    return 3\n");
    fixture.write(INPUT, &prepared.to_string());
    let output = fixture.run(&["review-task", "submit", SPEC, "--report", INPUT, "--json"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(!fixture.path(REVIEW).exists());
    assert!(fixture.state()["semantic_review_sha256"].is_null());
    fixture.finish_markdown();
    fixture.require_blocked_completion();
}

#[test]
fn malformed_incomplete_or_unbound_assessments_cannot_overwrite_an_accepted_record() {
    let fixture = Fixture::held();
    fixture.submit_positive();
    let valid = positive(&fixture.request());
    let original = fs::read(fixture.path(REVIEW)).unwrap();
    for variant in [
        "malformed_json",
        "duplicate_key",
        "omitted_criterion",
        "duplicate_criterion",
        "unknown_reference",
        "missing_mapped_check",
        "placeholder_reason",
        "omitted_dimension",
        "empty_reviewer",
        "missing_history",
        "missing_history_context",
        "wrong_history_source",
        "placeholder_history_reason",
        "invalid_history_decision",
        "empty_history_evidence",
    ] {
        let mut invalid = valid.clone();
        match variant {
            "omitted_criterion" => invalid["criteria"] = json!([]),
            "duplicate_criterion" => {
                let duplicate = invalid["criteria"][0].clone();
                invalid["criteria"].as_array_mut().unwrap().push(duplicate);
            }
            "unknown_reference" => {
                invalid["criteria"][0]["evidence"] =
                    json!(["check:unit", "check:integration", "check:missing"])
            }
            "missing_mapped_check" => invalid["criteria"][0]["evidence"] = json!(["check:unit"]),
            "placeholder_reason" => invalid["criteria"][0]["reason"] = json!("pending"),
            "omitted_dimension" => {
                invalid.as_object_mut().unwrap().remove("proportionality");
            }
            "empty_reviewer" => invalid["reviewer"]["name"] = json!(""),
            "missing_history" => {
                invalid.as_object_mut().unwrap().remove("history");
            }
            "missing_history_context" => {
                invalid["history"]
                    .as_object_mut()
                    .unwrap()
                    .remove("context");
            }
            "wrong_history_source" => {
                invalid["history"]["context"]["evidence"] = json!(["knowledge:lessons"])
            }
            "placeholder_history_reason" => {
                invalid["history"]["context"]["reason"] = json!("pending")
            }
            "invalid_history_decision" => {
                invalid["history"]["context"]["decision"] = json!("updated")
            }
            "empty_history_evidence" => invalid["history"]["lessons"]["evidence"] = json!([]),
            "malformed_json" | "duplicate_key" => {}
            _ => unreachable!(),
        }
        let body = match variant {
            "malformed_json" => "{broken".into(),
            "duplicate_key" => invalid.to_string().replace(
                "\"schema_version\":1",
                "\"schema_version\":1,\"schema_version\":1",
            ),
            _ => invalid.to_string(),
        };
        fixture.write(INPUT, &body);
        let output = fixture.run(&["review-task", "submit", SPEC, "--report", INPUT, "--json"]);
        assert!(!output.status.success(), "{variant}: {output:?}");
        assert_eq!(
            fs::read(fixture.path(REVIEW)).unwrap(),
            original,
            "{variant}"
        );
    }
    let outside = fixture.home.join("outside-review.json");
    fs::write(&outside, valid.to_string()).unwrap();
    let output = fixture.run(&[
        "review-task",
        "submit",
        SPEC,
        "--report",
        outside.to_str().unwrap(),
        "--json",
    ]);
    assert!(!output.status.success());
    assert_eq!(fs::read(fixture.path(REVIEW)).unwrap(), original);
    assert_eq!(fixture.status().1["status"], "accepted");
}

#[test]
fn missing_or_corrupted_pinned_record_cannot_fall_back_to_markdown_completion() {
    for missing in [true, false] {
        let fixture = Fixture::held();
        let accepted = fixture.submit_positive();
        fixture.finish_markdown();
        if missing {
            fs::remove_file(fixture.path(REVIEW)).unwrap();
        } else {
            fixture.write(REVIEW, "{corrupt");
        }
        let (output, status) = fixture.status();
        assert_eq!(output.status.code(), Some(1));
        assert!(
            matches!(
                status["status"].as_str(),
                Some("missing" | "stale" | "unavailable")
            ),
            "{status}"
        );
        fixture.require_blocked_completion();
        assert_eq!(fixture.state()["semantic_review_required"], true);
        assert_eq!(
            fixture.state()["semantic_review_sha256"],
            accepted["review_revision"]
        );
    }
}

#[test]
fn damaging_the_review_of_a_learned_task_revokes_complete_in_status_and_next() {
    let fixture = Fixture::held();
    fixture.submit_positive();
    fixture.finish_markdown();
    assert_success(&fixture.run(&["run-task", SPEC]));
    assert_eq!(fixture.state()["status"], "learned");
    fixture.write(REVIEW, "{corrupt");
    let (output, status) = fixture.status();
    assert_eq!(output.status.code(), Some(1));
    assert_ne!(status["status"], "accepted");
    assert_ne!(status["status"], "not_required");
    fixture.index();
    let state = fs::read(fixture.path(STATE)).unwrap();
    let next = fixture.run(&["next"]);
    assert_success(&next);
    let next = String::from_utf8_lossy(&next.stdout);
    assert!(next.contains("review-task prepare"), "{next}");
    assert!(!next.contains("All tasks complete"), "{next}");
    assert_eq!(fs::read(fixture.path(STATE)).unwrap(), state);
    fixture.require_blocked_completion();
}

#[test]
fn migration_requires_review_for_legacy_pending_but_preserves_historical_learned_state() {
    for learned in [false, true] {
        let fixture = Fixture::held();
        fixture.finish_markdown();
        let mut state = fixture.state();
        state
            .as_object_mut()
            .unwrap()
            .remove("semantic_review_required");
        state
            .as_object_mut()
            .unwrap()
            .remove("semantic_review_sha256");
        if learned {
            // A persisted pre-upgrade completed record is historical evidence.
            state["status"] = json!("learned");
            state["next_step"] = json!("close");
        }
        fixture.write(STATE, &state.to_string());
        let (output, status) = fixture.status();
        if learned {
            assert_success(&output);
            assert_eq!(status["status"], "not_required");
            assert_success(&fixture.run(&["run-task", SPEC]));
            assert_eq!(fixture.state()["status"], "learned");
        } else {
            assert_eq!(output.status.code(), Some(1));
            assert_ne!(status["status"], "not_required");
            assert_ne!(status["status"], "accepted");
            fixture.require_blocked_completion();
        }
        assert!(!fixture.path(REVIEW).exists());
    }
}

#[test]
fn accepted_semantics_with_unknown_or_required_history_updates_cannot_close_via_markdown() {
    for kind in ["context", "lessons"] {
        for decision in ["unknown", "update_required"] {
            let fixture = Fixture::held();
            let mut report = positive(&fixture.request());
            report["history"][kind] = json!({
                "decision":decision,
                "reason": if decision == "unknown" {
                    "This fixture reviewer has not determined whether the current canonical knowledge file needs a further update."
                } else {
                    "The current canonical knowledge file still needs an explicit description of the service behavior."
                },
                "evidence":[format!("knowledge:{kind}")]
            });
            let (output, accepted) = fixture.submit(&report);
            assert_success(&output);
            assert_eq!(
                accepted["status"], "accepted",
                "{kind} {decision}: {accepted}"
            );
            assert_eq!(accepted["history_status"], decision, "{accepted}");
            assert_eq!(fixture.state()["status"], "history_review_required");
            let (_, status) = fixture.status();
            assert_eq!(status["status"], "accepted");
            assert_eq!(status["history_status"], decision);
            fixture.finish_markdown();
            fixture.require_blocked_completion();
        }
    }
}

fn mutate_history_source(fixture: &Fixture, path: &str, operation: &str) {
    match operation {
        "create" | "mutate" => fixture.write(
            path,
            "# Canonical project knowledge\nThis file now contains a different reviewed input.\n",
        ),
        "delete" => fs::remove_file(fixture.path(path)).unwrap(),
        _ => unreachable!(),
    }
}

#[test]
fn canonical_history_mutation_creation_or_deletion_invalidates_submit_and_first_completion() {
    for phase in ["prepare_submit", "first_completion"] {
        for (kind, path) in [("context", CONTEXT), ("lessons", LESSONS)] {
            for operation in ["create", "mutate", "delete"] {
                let fixture = Fixture::new_with_history_sources(operation != "create").hold();
                let request = fixture.request();
                let source = &request["target"]["project_history"]["sources"][kind];
                assert_eq!(source["path"], path);
                if operation == "create" {
                    assert!(source["sha256"].is_null());
                } else {
                    assert_eq!(
                        source["sha256"],
                        sha(&fs::read(fixture.path(path)).unwrap())
                    );
                }
                let report = positive(&request);
                if phase == "first_completion" {
                    assert_success(&fixture.submit(&report).0);
                }
                mutate_history_source(&fixture, path, operation);
                if phase == "prepare_submit" {
                    fixture.write(INPUT, &report.to_string());
                    let output =
                        fixture.run(&["review-task", "submit", SPEC, "--report", INPUT, "--json"]);
                    assert!(!output.status.success(), "{kind} {operation}: {output:?}");
                    assert!(fixture.state()["semantic_review_sha256"].is_null());
                } else {
                    let (output, status) = fixture.status();
                    assert!(!output.status.success(), "{kind} {operation}: {status}");
                    assert_ne!(status["status"], "accepted", "{kind} {operation}: {status}");
                }
                fixture.finish_markdown();
                fixture.require_blocked_completion();
            }
        }
    }
}

#[test]
fn later_canonical_knowledge_edits_do_not_rewrite_an_already_completed_iteration() {
    for operation in ["create", "mutate", "delete"] {
        let fixture = Fixture::new_with_history_sources(operation != "create").hold();
        fixture.submit_positive();
        assert_success(&fixture.run(&["run-task", SPEC]));
        assert_eq!(fixture.state()["status"], "learned");
        let historical_state = fs::read(fixture.path(STATE)).unwrap();
        let historical_record = fs::read(fixture.path(REVIEW)).unwrap();
        for path in [CONTEXT, LESSONS] {
            mutate_history_source(&fixture, path, operation);
        }
        let (output, status) = fixture.status();
        assert_success(&output);
        assert_eq!(status["status"], "accepted", "{operation}: {status}");
        assert_eq!(status["history_status"], "resolved");
        assert_success(&fixture.run(&["run-task", SPEC]));
        assert_eq!(fixture.state()["status"], "learned");
        assert_eq!(fs::read(fixture.path(STATE)).unwrap(), historical_state);
        assert_eq!(fs::read(fixture.path(REVIEW)).unwrap(), historical_record);
    }
}

#[test]
fn learned_legacy_semantic_records_keep_their_original_markdown_history_gate() {
    let fixture = Fixture::held();
    fixture.submit_positive();
    assert_success(&fixture.run(&["run-task", SPEC]));
    assert_eq!(fixture.state()["status"], "learned");

    // Synthesize the preceding on-disk schema with an accepted, pinned report.
    // No new CLI submission is allowed to downgrade to this legacy shape.
    let mut record: Value =
        serde_json::from_slice(&fs::read(fixture.path(REVIEW)).unwrap()).unwrap();
    record["target"]
        .as_object_mut()
        .unwrap()
        .remove("project_history");
    record["target"]["evidence"]
        .as_object_mut()
        .unwrap()
        .remove("knowledge:context");
    record["target"]["evidence"]
        .as_object_mut()
        .unwrap()
        .remove("knowledge:lessons");
    record["report"].as_object_mut().unwrap().remove("history");
    let legacy_target: mmcg::task_review::Target =
        serde_json::from_value(record["target"].clone()).unwrap();
    assert!(serde_json::to_value(&legacy_target)
        .unwrap()
        .get("project_history")
        .is_none());
    record["report"]["target_revision"] = json!(sha(&serde_json::to_vec(&legacy_target).unwrap()));
    let legacy_report: mmcg::task_review::Submission =
        serde_json::from_value(record["report"].clone()).unwrap();
    assert!(serde_json::to_value(&legacy_report)
        .unwrap()
        .get("history")
        .is_none());
    fixture.write(REVIEW, &record.to_string());
    let mut state = fixture.state();
    state["semantic_review_sha256"] = json!(sha(&fs::read(fixture.path(REVIEW)).unwrap()));
    fixture.write(STATE, &state.to_string());
    fixture.finish_markdown();

    let (output, status) = fixture.status();
    assert_success(&output);
    assert_eq!(status["status"], "accepted");
    assert_eq!(status["history_status"], "legacy_markdown");
    assert_success(&fixture.run(&["run-task", SPEC]));
    assert_eq!(fixture.state()["status"], "learned");
    let markdown = fs::read_to_string(fixture.path(HISTORY)).unwrap();
    fixture.write(
        HISTORY,
        &markdown.replace("**Context:** not applicable", "**Context:** pending"),
    );
    fixture.require_blocked_completion();
}

#[test]
fn a_late_lessons_change_cannot_cross_the_final_guarded_completion_check() {
    let fixture = Fixture::new_with_history_sources(true).hold();
    fixture.submit_positive();
    let before_lessons = fs::read(fixture.path(LESSONS)).unwrap();
    let before_record = fs::read(fixture.path(REVIEW)).unwrap();
    let harness = fixture._temp.path().join("late-history-race");
    fs::create_dir_all(&harness).unwrap();
    let wrapper = harness.join("git");
    fs::write(
        &wrapper,
        r#"#!/bin/sh
set -eu
if test "$#" -eq 8 &&
   test "$1" = --literal-pathspecs &&
   test "$2" = -c &&
   test "$3" = core.fsmonitor=false &&
   test "$4" = -c &&
   test "$5" = diff.external= &&
   test "$6" = ls-files &&
   test "$7" = --stage &&
   test "$8" = -z; then
  count=0
  if test -f "$MMCG_LATE_HISTORY_COUNT"; then count=$(/bin/cat "$MMCG_LATE_HISTORY_COUNT"); fi
  count=$((count + 1))
  printf '%s' "$count" > "$MMCG_LATE_HISTORY_COUNT"
  if test "$count" -eq "$MMCG_LATE_HISTORY_INJECT_AT"; then
    printf '# Lessons changed after early review validation\n' > .mastermind/tasks/_lessons.md
    printf '%s' "$count" > "$MMCG_LATE_HISTORY_MARKER"
  fi
fi
exec /usr/bin/git "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let count_path = harness.join("stage-inventories");
    let marker_path = harness.join("injected");
    let wrapped = |args: &[&str], inject_at: usize| {
        fixture
            .command(env!("CARGO_BIN_EXE_mmcg"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", harness.display()),
            )
            .env("MMCG_LATE_HISTORY_COUNT", &count_path)
            .env("MMCG_LATE_HISTORY_MARKER", &marker_path)
            .env("MMCG_LATE_HISTORY_INJECT_AT", inject_at.to_string())
            .args(["--index", ".mastermind/index.db"])
            .args(args)
            .output()
            .unwrap()
    };

    // Status and early completion both collect the current review target.
    // Calibrate that collection instead of assuming a fixed total Git count.
    // A verification snapshot currently reads the stage inventory twice, and
    // one collect_target uses three snapshots. The next inventory belongs to
    // the outer inspect_checks, after the early validate_current has passed.
    let status = wrapped(&["review-task", "status", SPEC, "--json"], 0);
    assert_success(&status);
    assert_eq!(parse(&status)["status"], "accepted");
    let early_inventories: usize = fs::read_to_string(&count_path).unwrap().parse().unwrap();
    assert!(early_inventories >= 2 && early_inventories.is_multiple_of(2));
    assert!(!marker_path.exists());
    assert_eq!(fs::read(fixture.path(LESSONS)).unwrap(), before_lessons);
    fs::write(&count_path, "0").unwrap();
    let injection_at = early_inventories + 1;

    let output = wrapped(&["run-task", SPEC], injection_at);
    assert_eq!(
        fs::read_to_string(&marker_path).unwrap(),
        injection_at.to_string(),
        "the race must actually occur after the calibrated early validation"
    );
    let final_inventories: usize = fs::read_to_string(&count_path).unwrap().parse().unwrap();
    assert!(final_inventories > injection_at);
    assert_ne!(fs::read(fixture.path(LESSONS)).unwrap(), before_lessons);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("audited inputs or semantic review changed during history refresh"),
        "{output:?}"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Task complete —"));
    assert_ne!(fixture.state()["status"], "learned");
    assert_eq!(fs::read(fixture.path(REVIEW)).unwrap(), before_record);
    let (output, status) = fixture.status();
    assert!(!output.status.success(), "{status}");
    assert_ne!(status["status"], "accepted");
}

fn request_history_update(request: &Value, kind: &str) -> Value {
    let mut report = positive(request);
    report["history"][kind] = json!({
        "decision":"update_required",
        "reason":format!("Record the concrete service behavior in the current canonical {kind} file before completing this fixture."),
        "evidence":[format!("knowledge:{kind}"),"check:unit"]
    });
    report
}

#[test]
fn follow_up_returns_exact_bound_history_work_and_conditional_steps_without_side_effects() {
    let fixture = Fixture::new_with_history_sources(true);
    // integration remains a declared observed check, but is deliberately not
    // mapped to a criterion. The recovery plan must still include it.
    fixture.edit_frontmatter(|metadata| metadata["acceptance"][0]["checks"] = json!(["unit"]));
    let fixture = fixture.hold();
    let request = fixture.request();
    let report = request_history_update(&request, "lessons");
    let (output, accepted) = fixture.submit(&report);
    assert_success(&output);

    let watched = [
        STATE,
        REVIEW,
        SPEC,
        REPORT,
        HISTORY,
        CONTEXT,
        LESSONS,
        ".mastermind/check-runs",
        ".mastermind/index.db",
        ".mastermind/tasks/001-review/verification/unit.json",
        ".mastermind/tasks/001-review/verification/integration.json",
    ];
    let before: Vec<_> = watched
        .iter()
        .map(|path| fs::read(fixture.path(path)).unwrap())
        .collect();
    let guard = fixture._temp.path().join("follow-up-provider-guard");
    fs::create_dir_all(&guard).unwrap();
    let marker = guard.join("called");
    for client in ["claude", "codex"] {
        let path = guard.join(client);
        fs::write(
            &path,
            "#!/bin/sh\nprintf invoked > \"$MMCG_FOLLOW_UP_PROVIDER_MARKER\"\nexit 91\n",
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    // The command promises JSON even without --json.
    let output = fixture
        .command(env!("CARGO_BIN_EXE_mmcg"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", guard.display()),
        )
        .env("MMCG_FOLLOW_UP_PROVIDER_MARKER", &marker)
        .args([
            "--index",
            ".mastermind/index.db",
            "review-task",
            "follow-up",
            SPEC,
        ])
        .output()
        .unwrap();
    assert_success(&output);
    let packet = parse(&output);
    assert_eq!(packet["schema_version"], 1);
    assert_eq!(packet["repository_content_untrusted"], true);
    assert_eq!(packet["review_revision"], accepted["review_revision"]);
    assert_eq!(
        packet["review_revision"],
        fixture.state()["semantic_review_sha256"]
    );
    assert_eq!(packet["target_revision"], report["target_revision"]);
    assert_eq!(packet["target"], request["target"]);
    assert_eq!(packet["review"], report);
    assert_eq!(packet["next_action"], "update_project_history");
    assert_eq!(packet["role"], "planner");
    assert_eq!(packet["focus"], json!(["history.lessons"]));
    assert_eq!(packet["overall_task_completion"], "not_evaluated");
    assert_eq!(
        packet["completion"],
        json!({
            "when":"all_review_decisions_resolved",
            "argv":["mastermind","run-task",format!("./{SPEC}")]
        })
    );
    assert_eq!(packet["working_directory"], fixture.root.to_str().unwrap());
    assert!(!packet["instructions"].as_str().unwrap().trim().is_empty());
    assert_eq!(
        packet["target"]["project_history"]["sources"]["lessons"]["path"],
        LESSONS
    );
    assert_eq!(
        packet["target"]["project_history"]["sources"]["lessons"]["sha256"],
        sha(&fs::read(fixture.path(LESSONS)).unwrap())
    );
    assert_eq!(
        packet["review"]["history"]["lessons"]["reason"],
        report["history"]["lessons"]["reason"]
    );
    assert_eq!(
        packet["review"]["history"]["lessons"]["evidence"],
        json!(["knowledge:lessons", "check:unit"])
    );
    assert_eq!(
        packet["target"]["evidence"]["knowledge:lessons"],
        request["target"]["evidence"]["knowledge:lessons"]
    );

    let steps = packet["after_changes"].as_array().unwrap();
    assert_eq!(steps.len(), 4);
    let ids: std::collections::BTreeSet<_> = steps[..2]
        .iter()
        .map(|step| {
            assert_eq!(step["when"], "verification_inputs_changed");
            let argv = step["argv"].as_array().unwrap();
            assert_eq!(
                &argv[..4],
                &json!(["mastermind", "verification", "run", format!("./{SPEC}")])
                    .as_array()
                    .unwrap()[..]
            );
            assert_eq!(argv.len(), 6);
            assert_eq!(argv[5], "--json");
            argv[4]
                .as_str()
                .unwrap()
                .strip_prefix("--id=")
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        ids,
        ["unit".to_string(), "integration".to_string()]
            .into_iter()
            .collect()
    );
    assert_eq!(
        steps[2],
        json!({
            "when":"audited_inputs_or_check_receipts_changed",
            "argv":["mastermind","run-task",format!("./{SPEC}"),"--post-only"]
        })
    );
    assert_eq!(
        steps[3],
        json!({
            "when":"after_any_change_or_additional_inspection",
            "argv":["mastermind","review-task","prepare",format!("./{SPEC}"),"--json"]
        })
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE_REVIEW_CHECK_OUTPUT"));
    assert!(
        !marker.exists(),
        "read-only follow-up invoked a native provider"
    );
    for (path, bytes) in watched.iter().zip(before) {
        assert_eq!(fs::read(fixture.path(path)).unwrap(), bytes, "{path}");
    }
    assert_eq!(fixture.state()["status"], "history_review_required");
}

#[test]
fn follow_up_prioritizes_semantic_findings_before_history_and_focuses_only_that_class() {
    let fixture = Fixture::held();
    for (case, action, role, focus) in [
        (
            "semantic_failure",
            "revise_solution",
            "planner",
            json!(["criterion:service-result"]),
        ),
        (
            "dimension_failure",
            "revise_solution",
            "planner",
            json!(["scope_control"]),
        ),
        (
            "semantic_unknown",
            "inspect_review",
            "reviewer",
            json!(["verification_quality"]),
        ),
        (
            "history_update",
            "update_project_history",
            "planner",
            json!(["history.lessons"]),
        ),
        (
            "history_unknown",
            "inspect_project_history",
            "reviewer",
            json!(["history.context"]),
        ),
        ("complete", "complete", "controller", json!([])),
    ] {
        let mut report = positive(&fixture.request());
        if case != "complete" {
            report["history"]["context"] = json!({
                "decision":"unknown", "reason":"The reviewer has not inspected whether the canonical context needs a further update.", "evidence":[]
            });
        }
        if matches!(
            case,
            "semantic_failure" | "dimension_failure" | "semantic_unknown" | "history_update"
        ) {
            report["history"]["lessons"]["decision"] = json!("update_required");
            report["history"]["lessons"]["reason"] =
                json!("The current canonical lessons file needs the reviewed service lesson.");
        }
        match case {
            "semantic_failure" => {
                report["criteria"][0]["status"] = json!("unsatisfied");
                report["criteria"][0]["reason"] =
                    json!("The reviewer rejects the evidence for the intended runtime behavior.");
                report["verification_quality"]["status"] = json!("unknown");
            }
            "dimension_failure" => {
                report["scope_control"]["status"] = json!("unsatisfied");
                report["scope_control"]["reason"] = json!("The reviewer has identified a scope concern requiring an explicit solution change.");
                report["proportionality"]["status"] = json!("unknown");
            }
            "semantic_unknown" => {
                report["verification_quality"]["status"] = json!("unknown");
                report["verification_quality"]["reason"] = json!("The reviewer still needs to inspect whether the declared check covers the intended behavior.");
            }
            _ => {}
        }
        let (_, submitted) = fixture.submit(&report);
        let packet = fixture.follow_up();
        assert_eq!(
            packet["review_revision"], submitted["review_revision"],
            "{case}"
        );
        assert_eq!(packet["next_action"], action, "{case}: {packet}");
        assert_eq!(packet["role"], role, "{case}");
        assert_eq!(packet["focus"], focus, "{case}");
        assert!(packet["target"]["project_history"]["sources"]["context"]["sha256"].is_null());
        assert!(packet["target"]["project_history"]["sources"]["lessons"]["sha256"].is_null());
        assert_eq!(
            fixture.state()["status"],
            "history_review_required",
            "read-only complete action does not complete the task"
        );
    }
}

#[test]
fn follow_up_withholds_actionable_packets_for_missing_revoked_stale_or_historical_reviews() {
    const PRIVATE_REASON: &str = "FOLLOWUP_WITHHELD_PRIVATE_REASON";
    for invalid in [
        "missing",
        "deleted",
        "corrupt",
        "revoked",
        "stale_lessons",
        "stale_code",
        "stale_executable",
        "learned",
        "legacy",
    ] {
        let fixture = Fixture::new_with_history_sources(true).hold();
        if invalid != "missing" {
            let request = fixture.request();
            let mut report = if invalid == "learned" {
                positive(&request)
            } else {
                request_history_update(&request, "lessons")
            };
            report["history"]["lessons"]["reason"] = json!(PRIVATE_REASON);
            assert_success(&fixture.submit(&report).0);
        }
        match invalid {
            "deleted" => fs::remove_file(fixture.path(REVIEW)).unwrap(),
            "corrupt" => fixture.write(REVIEW, "{corrupt"),
            "revoked" => {
                let mut state = fixture.state();
                state["semantic_review_sha256"] = Value::Null;
                fixture.write(STATE, &state.to_string());
            }
            "stale_lessons" => fixture.write(LESSONS, "# A different canonical lesson\n"),
            "stale_code" => fixture.write("service.py", "def keep():\n    return 3\n"),
            "stale_executable" => fixture.write(
                EXECUTABLE,
                &(PROBE.to_owned() + "\n# Changed executable.\n"),
            ),
            "learned" => {
                assert_success(&fixture.run(&["run-task", SPEC]));
                assert_eq!(fixture.state()["status"], "learned");
            }
            "legacy" => {
                let mut record: Value =
                    serde_json::from_slice(&fs::read(fixture.path(REVIEW)).unwrap()).unwrap();
                record["target"]
                    .as_object_mut()
                    .unwrap()
                    .remove("project_history");
                record["target"]["evidence"]
                    .as_object_mut()
                    .unwrap()
                    .remove("knowledge:context");
                record["target"]["evidence"]
                    .as_object_mut()
                    .unwrap()
                    .remove("knowledge:lessons");
                record["report"].as_object_mut().unwrap().remove("history");
                let target: mmcg::task_review::Target =
                    serde_json::from_value(record["target"].clone()).unwrap();
                record["report"]["target_revision"] =
                    json!(sha(&serde_json::to_vec(&target).unwrap()));
                fixture.write(REVIEW, &record.to_string());
                let mut state = fixture.state();
                state["semantic_review_sha256"] =
                    json!(sha(&fs::read(fixture.path(REVIEW)).unwrap()));
                fixture.write(STATE, &state.to_string());
            }
            _ => {}
        }
        let state = fs::read(fixture.path(STATE)).unwrap();
        let checks = fs::read(fixture.path(".mastermind/check-runs")).unwrap();
        let output = fixture.run(&["review-task", "follow-up", SPEC, "--json"]);
        assert!(!output.status.success(), "{invalid}: {output:?}");
        assert!(
            output.stdout.is_empty(),
            "{invalid}: no partial actionable packet may escape: {output:?}"
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains(PRIVATE_REASON),
            "{invalid}"
        );
        assert_eq!(fs::read(fixture.path(STATE)).unwrap(), state, "{invalid}");
        assert_eq!(
            fs::read(fixture.path(".mastermind/check-runs")).unwrap(),
            checks,
            "{invalid}"
        );
    }
}

#[test]
fn next_and_resume_route_valid_unresolved_pins_to_follow_up() {
    let fixture = Fixture::held();
    let check_route = |expected: &str, absent: Option<&str>| {
        fixture.index();
        let state = fs::read(fixture.path(STATE)).unwrap();
        let checks = fs::read(fixture.path(".mastermind/check-runs")).unwrap();
        for args in [vec!["next"], vec!["resume", "--task", "001-review"]] {
            let output = fixture.run(&args);
            assert_success(&output);
            let text = String::from_utf8_lossy(&output.stdout);
            assert!(text.contains(expected), "{args:?}: {text}");
            if let Some(absent) = absent {
                assert!(!text.contains(absent), "{args:?}: {text}");
            }
        }
        assert_eq!(fs::read(fixture.path(STATE)).unwrap(), state);
        assert_eq!(
            fs::read(fixture.path(".mastermind/check-runs")).unwrap(),
            checks
        );
    };
    check_route("review-task prepare", Some("review-task follow-up"));
    assert_success(
        &fixture
            .submit(&request_history_update(&fixture.request(), "lessons"))
            .0,
    );
    check_route("review-task follow-up", None);

    let mut negative = positive(&fixture.request());
    negative["criteria"][0]["status"] = json!("unsatisfied");
    negative["criteria"][0]["reason"] = json!("The reviewer rejects the current evidence of runtime behavior and requires a solution revision.");
    let (output, blocked) = fixture.submit(&negative);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(blocked["status"], "blocked");
    check_route("review-task follow-up", None);

    let pinned = fs::read(fixture.path(REVIEW)).unwrap();
    fixture.write(REVIEW, "{corrupt");
    check_route("review-task prepare", Some("review-task follow-up"));
    fs::write(fixture.path(REVIEW), pinned).unwrap();
    fixture.submit_positive();
    check_route("mastermind run-task", Some("review-task follow-up"));
}

#[test]
fn an_ignored_lesson_update_requires_a_fresh_typed_review_then_closes_without_rerunning_checks() {
    let fixture = Fixture::new_with_history_sources(true).hold();
    let request = fixture.request();
    let report = request_history_update(&request, "lessons");
    assert_success(&fixture.submit(&report).0);
    let follow_up = fixture.follow_up();
    assert_eq!(follow_up["next_action"], "update_project_history");
    let observed_checks = fs::read(fixture.path(".mastermind/check-runs")).unwrap();
    let prior_review = fs::read(fixture.path(REVIEW)).unwrap();

    fixture.write(LESSONS, "# Project lessons\nFor this service fixture, observe the failing return-value check before replacing the literal and rerun it after the change.\n");
    let stale = fixture.run(&["review-task", "follow-up", SPEC, "--json"]);
    assert!(!stale.status.success());
    assert!(stale.stdout.is_empty());
    assert_eq!(fs::read(fixture.path(REVIEW)).unwrap(), prior_review);
    let fresh = fixture.request();
    assert_ne!(fresh["target_revision"], follow_up["target_revision"]);
    assert_eq!(
        fresh["expected_review_revision"],
        follow_up["review_revision"]
    );
    assert_eq!(
        fresh["target"]["project_history"]["sources"]["lessons"]["sha256"],
        sha(&fs::read(fixture.path(LESSONS)).unwrap())
    );
    let mut reviewed = positive(&fresh);
    reviewed["history"]["lessons"]["reason"] = json!("The current canonical lessons file now records the concrete failing-check sequence, so no further update is needed.");
    assert_success(&fixture.submit(&reviewed).0);
    assert_eq!(fixture.follow_up()["next_action"], "complete");
    assert_eq!(fixture.state()["status"], "history_review_required");
    assert_success(&fixture.run(&["run-task", SPEC]));
    assert_eq!(fixture.state()["status"], "learned");
    assert_eq!(
        fs::read(fixture.path(".mastermind/check-runs")).unwrap(),
        observed_checks,
        "the ignored canonical lesson alone does not change observed verification inputs"
    );
}

#[test]
fn a_tracked_context_follow_up_requires_approved_scope_checks_and_a_fresh_audit() {
    let fixture = Fixture::new_with_history_sources(true).hold();
    assert_success(
        &fixture
            .submit(&request_history_update(&fixture.request(), "context"))
            .0,
    );
    let previous = fixture.follow_up();
    assert_eq!(previous["next_action"], "update_project_history");
    assert_eq!(previous["focus"], json!(["history.context"]));
    fixture.write(CONTEXT, "# Project context\nThe existing keep function returns the literal two. Its declared unit and integration checks support this bounded behavior.\n");
    assert!(!fixture
        .run(&["review-task", "follow-up", SPEC, "--json"])
        .status
        .success());
    assert!(!fixture
        .run(&["acceptance", "status", SPEC, "--json"])
        .status
        .success());
    assert!(
        !fixture
            .run(&["run-task", SPEC, "--post-only"])
            .status
            .success(),
        "a follow-up packet does not authorize undeclared tracked file scope"
    );
    assert_ne!(fixture.state()["status"], "learned");

    // Explicitly revise the approved fixture contract before a new preflight;
    // a history recommendation never expands file scope by itself.
    fixture.edit_frontmatter(|metadata| {
        metadata["touches"]
            .as_array_mut()
            .unwrap()
            .push(json!({"file":CONTEXT,"symbols":[]}));
    });
    fixture.index();
    assert_success(&fixture.run(&["run-task", SPEC, "--pre-only"]));
    for id in ["unit", "integration"] {
        assert_success(&fixture.run(&["verification", "run", SPEC, "--id", id, "--json"]));
    }
    let mut executor: Value =
        serde_json::from_slice(&fs::read(fixture.path(REPORT)).unwrap()).unwrap();
    executor["files_modified"] = json!(["service.py", CONTEXT]);
    fixture.write(REPORT, &executor.to_string());
    assert_success(&fixture.run(&["run-task", SPEC, "--post-only"]));
    assert_eq!(fixture.state()["status"], "history_review_required");
    let request = fixture.request();
    assert_ne!(request["target_revision"], previous["target_revision"]);
    let mut reviewed = positive(&request);
    reviewed["scope_control"]["reason"] = json!("The changed service and canonical context are both included in the explicitly revised file scope.");
    reviewed["proportionality"]["reason"] = json!("A single return literal and a short description in the existing canonical context implement the revised bounded contract.");
    reviewed["history"]["context"]["reason"] = json!("The current canonical context now states the service behavior, so no further update is needed.");
    assert_success(&fixture.submit(&reviewed).0);
    assert_eq!(fixture.follow_up()["next_action"], "complete");
    assert_success(&fixture.run(&["run-task", SPEC]));
    assert_eq!(fixture.state()["status"], "learned");
    assert_eq!(
        fs::read_to_string(fixture.path(".mastermind/check-runs"))
            .unwrap()
            .lines()
            .count(),
        4
    );
}

#[test]
fn generated_follow_up_argv_executes_with_leading_dash_spec_and_check_id() {
    const DASH_SPEC: &str = "-task.md";
    const SPEC_ARG: &str = "./-task.md";
    let fixture = Fixture::new();
    fixture.edit_frontmatter(|metadata| {
        metadata["verify"][0]["run"]["id"] = json!("-unit");
        metadata["acceptance"][0]["checks"] = json!(["-unit", "integration"]);
    });
    fs::copy(fixture.path(SPEC), fixture.path(DASH_SPEC)).unwrap();
    // Standalone controller artifacts live beside this root spec. Keep them
    // out of product verification inputs, as canonical .mastermind artifacts
    // are in the other fixtures. Their dedicated controller hashes still bind them.
    fixture.write(
        ".gitignore",
        ".mastermind/\n-task.md\nexecutor-report.md\naudit.md\n",
    );
    fixture.git(&["add", ".gitignore"]);
    fixture.git(&[
        "commit",
        "-qm",
        "Declare standalone fixture controller artifacts",
    ]);
    fixture.index();
    assert_success(&fixture.run(&["run-task", SPEC_ARG, "--pre-only"]));
    fixture.write("service.py", "def keep():\n    return 2\n");
    fixture.index();
    for id in ["--id=-unit", "--id=integration"] {
        assert_success(&fixture.run(&["verification", "run", SPEC_ARG, id, "--json"]));
    }
    let verifications: Vec<_> = ["unit", "integration"]
        .into_iter()
        .map(|id| {
            json!({
                "cmd":format!("./{EXECUTABLE} {id}"), "result":"pass", "observed":{"exit_code":0}
            })
        })
        .collect();
    fixture.write("executor-report.md", &json!({
        "schema_version":1, "spec":DASH_SPEC, "status":"complete", "phases":[],
        "files_modified":["service.py"], "claims":[], "defects":[], "verifications":verifications
    }).to_string());
    assert_success(&fixture.run(&["run-task", SPEC_ARG, "--post-only"]));
    let state_path = mmcg::run_task::state_file_path(&fixture.root, &fixture.path(DASH_SPEC));
    let read_state =
        || -> Value { serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap() };
    assert_eq!(read_state()["spec_path"], DASH_SPEC);
    assert_eq!(read_state()["status"], "history_review_required");
    assert!(
        !fixture.path(STATE).exists(),
        "the noncanonical task must own independent state"
    );

    let assessment = |request: &Value, update: bool| {
        let mut report = if update {
            request_history_update(request, "lessons")
        } else {
            positive(request)
        };
        for item in report["criteria"].as_array_mut().unwrap() {
            item["evidence"] = json!(["spec", "check:-unit", "check:integration"]);
        }
        report["verification_quality"]["evidence"] = json!(["check:-unit", "check:integration"]);
        report["history"]["lessons"]["evidence"] = json!(["knowledge:lessons", "check:-unit"]);
        report
    };
    let prepare = fixture.run(&["review-task", "prepare", SPEC_ARG, "--json"]);
    assert_success(&prepare);
    fixture.write(INPUT, &assessment(&parse(&prepare), true).to_string());
    assert_success(&fixture.run(&[
        "review-task",
        "submit",
        SPEC_ARG,
        "--report",
        INPUT,
        "--json",
    ]));
    let follow_up = fixture.run(&["review-task", "follow-up", SPEC_ARG, "--json"]);
    assert_success(&follow_up);
    let packet = parse(&follow_up);
    assert_eq!(packet["next_action"], "update_project_history");
    assert_eq!(packet["working_directory"], fixture.root.to_str().unwrap());

    let execute_argv = |step: &Value| {
        let argv: Vec<_> = step["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert_eq!(argv[0], "mastermind");
        // Only substitute the installed program for the test binary. Every
        // generated argument is sent to the real CLI parser unchanged.
        fixture
            .command(env!("CARGO_BIN_EXE_mmcg"))
            .current_dir(packet["working_directory"].as_str().unwrap())
            .args(["--index", ".mastermind/index.db"])
            .args(&argv[1..])
            .output()
            .unwrap()
    };
    let steps = packet["after_changes"].as_array().unwrap();
    assert!(steps.iter().any(|step| step["argv"]
        == json!([
            "mastermind",
            "verification",
            "run",
            SPEC_ARG,
            "--id=-unit",
            "--json"
        ])));
    let mut fresh_request = None;
    for step in steps {
        let output = execute_argv(step);
        assert_success(&output);
        if step["argv"][1] == "review-task" {
            fresh_request = Some(parse(&output));
        }
    }
    let fresh = fresh_request.expect("the emitted plan must prepare a new review");
    assert_ne!(fresh["target_revision"], packet["target_revision"]);
    fixture.write(INPUT, &assessment(&fresh, false).to_string());
    assert_success(&fixture.run(&[
        "review-task",
        "submit",
        SPEC_ARG,
        "--report",
        INPUT,
        "--json",
    ]));
    let follow_up = fixture.run(&["review-task", "follow-up", SPEC_ARG, "--json"]);
    assert_success(&follow_up);
    let completion = parse(&follow_up);
    assert_eq!(completion["next_action"], "complete");
    assert_eq!(
        completion["completion"]["argv"],
        json!(["mastermind", "run-task", SPEC_ARG])
    );
    assert_success(&execute_argv(&completion["completion"]));
    assert_eq!(read_state()["status"], "learned");
    assert_eq!(
        fs::read_to_string(fixture.path(".mastermind/check-runs"))
            .unwrap()
            .lines()
            .count(),
        4
    );
}
