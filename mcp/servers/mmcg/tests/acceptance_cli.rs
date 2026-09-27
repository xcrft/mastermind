//! Acceptance requirements through the public CLI. Every repository, home,
//! executable and receipt in these tests is synthetic and local to its fixture.

#[cfg(unix)]
mod unix {
    use serde_json::{json, Value};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::{Command, Output};

    const SPEC: &str = ".mastermind/tasks/001-acceptance/spec.md";
    const REPORT: &str = ".mastermind/tasks/001-acceptance/executor-report.md";
    const STATE: &str = ".mastermind/tasks/001-acceptance/state.json";
    const VERIFICATION: &str = ".mastermind/tasks/001-acceptance/verification";
    const EXECUTABLE: &str = ".mastermind/probe.sh";
    const PROBE: &str = r##"#!/bin/sh
case "$1" in unit|integration) ;; *) exit 8 ;; esac
printf '%s\n' "$1" >> .mastermind/executions
if test -f ".mastermind/fail-$1"; then exit 7; fi
/usr/bin/grep -q 'return 2' service.py || exit 9
printf 'ACCEPTANCE_RAW_EXECUTION_OUTPUT_PRIVATE\n'
"##;

    fn criterion(id: &str, statement: &str, checks: &[&str]) -> Value {
        json!({"id": id, "statement": statement, "checks": checks})
    }

    fn metadata(checks: &[&str]) -> Value {
        let verify: Vec<Value> = checks
            .iter()
            .map(|id| {
                json!({
                    "cmd": format!("./{EXECUTABLE} {id}"),
                    "run": {
                        "id": id,
                        "argv": [format!("./{EXECUTABLE}"), id.to_string()],
                        "cwd": ".",
                        "timeout_secs": 10
                    }
                })
            })
            .collect();
        json!({
            "mode": "verified",
            "touches": [{"file": "service.py", "symbols": ["keep"]}],
            "verify": verify,
            "acceptance": [criterion("service-behavior", "The service returns two.", checks)]
        })
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        home: PathBuf,
    }

    impl Fixture {
        fn new(metadata: &Value) -> Self {
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
            for args in [
                vec!["init", "-q", "--initial-branch=main"],
                vec!["config", "user.name", "Acceptance Fixture"],
                vec!["config", "user.email", "acceptance@example.invalid"],
                vec!["config", "commit.gpgsign", "false"],
                vec!["config", "core.hooksPath", ""],
                vec!["add", "."],
                vec!["commit", "-q", "-m", "Synthetic acceptance baseline"],
                vec!["tag", "baseline"],
            ] {
                assert_success(&fixture.command("/usr/bin/git").args(args).output().unwrap());
            }
            fixture.write(EXECUTABLE, PROBE);
            fs::set_permissions(fixture.path(EXECUTABLE), fs::Permissions::from_mode(0o700))
                .unwrap();
            fixture.write_spec(metadata);
            fixture.index();
            fixture
        }

        fn path(&self, path: &str) -> PathBuf {
            self.root.join(path)
        }

        fn write(&self, path: &str, text: &str) {
            let path = self.path(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn write_spec(&self, metadata: &Value) {
            self.write(
                SPEC,
                &format!(
                    "---\n{}---\n# Acceptance fixture\n\n## Goals\nUpdate the service.\n\n## Scope\nEdit service.py.\n\n## Acceptance Criteria\n- [x] The service returns two.\n\n## Tests Plan\nRun every declared check.\n\n## Final Verification\nRun the declared commands.\n",
                    serde_norway::to_string(metadata).unwrap()
                ),
            );
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

        fn prepare(&self) {
            assert_success(&self.run(&["run-task", SPEC, "--pre-only"]));
            self.write("service.py", "def keep():\n    return 2\n");
            self.index();
        }

        fn verify(&self, id: &str) -> (Output, Value) {
            let output = self.run(&["verification", "run", SPEC, "--id", id, "--json"]);
            let receipt = parse_json(&output);
            (output, receipt)
        }

        fn acceptance(&self) -> (Output, Value) {
            let output = self.run(&["acceptance", "status", SPEC, "--json"]);
            let report = parse_json(&output);
            assert_eq!(report["schema_version"], 1);
            assert_eq!(report["semantic_accuracy"], "unknown");
            assert_eq!(report["overall_task_completion"], "not_evaluated");
            assert!(
                !String::from_utf8_lossy(&output.stdout)
                    .contains("ACCEPTANCE_RAW_EXECUTION_OUTPUT_PRIVATE"),
                "status must not disclose captured process output"
            );
            for criterion in report["criteria"].as_array().unwrap() {
                for check in criterion["checks"].as_array().unwrap() {
                    let keys: std::collections::BTreeSet<&str> = check
                        .as_object()
                        .unwrap()
                        .keys()
                        .map(String::as_str)
                        .collect();
                    assert_eq!(
                        keys,
                        ["id", "status", "receipt_revision", "run_id", "reason"]
                            .into_iter()
                            .collect(),
                        "the public observation must remain a redacted evidence reference"
                    );
                }
            }
            (output, report)
        }

        fn write_report(&self, checks: &[&str], files: &[&str]) {
            let verifications: Vec<Value> = checks
                .iter()
                .map(|id| {
                    json!({
                        "cmd": format!("./{EXECUTABLE} {id}"),
                        "result": "pass",
                        "observed": {"exit_code": 0}
                    })
                })
                .collect();
            self.write(
                REPORT,
                &json!({
                    "schema_version": 1,
                    "spec": SPEC,
                    "status": "complete",
                    "phases": [],
                    "files_modified": files,
                    "claims": [],
                    "defects": [],
                    "verifications": verifications
                })
                .to_string(),
            );
        }

        fn audit(&self) -> (Output, Value) {
            self.index();
            let output = self.run(&[
                "audit-spec",
                SPEC,
                "--since",
                "baseline",
                "--executor-report",
                REPORT,
                "--json",
            ]);
            let report = parse_json(&output);
            (output, report)
        }

        fn receipt_bytes(&self, id: &str) -> Vec<u8> {
            fs::read(self.path(&format!("{VERIFICATION}/{id}.json"))).unwrap()
        }

        fn state(&self) -> Value {
            serde_json::from_slice(&fs::read(self.path(STATE)).unwrap()).unwrap()
        }
    }

    fn assert_success(output: &Output) {
        assert!(output.status.success(), "{output:?}");
    }

    fn parse_json(output: &Output) -> Value {
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("invalid CLI JSON: {error}; {output:?}"))
    }

    fn check<'a>(report: &'a Value, criterion_id: &str, check_id: &str) -> &'a Value {
        report["criteria"]
            .as_array()
            .unwrap()
            .iter()
            .find(|criterion| criterion["id"] == criterion_id)
            .unwrap_or_else(|| panic!("missing criterion {criterion_id}: {report}"))["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == check_id)
            .unwrap_or_else(|| panic!("missing check {check_id}: {report}"))
    }

    fn assert_blocked(output: &Output, report: &Value) {
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert_eq!(report["status"], "blocked", "{report}");
    }

    fn has_finding(report: &Value, kind: &str) -> bool {
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["kind"] == kind)
    }

    #[test]
    fn legacy_absence_is_not_declared_and_adds_no_completion_gate() {
        let mut metadata = metadata(&["unit"]);
        metadata.as_object_mut().unwrap().remove("acceptance");
        metadata["verify"][0].as_object_mut().unwrap().remove("run");
        let fixture = Fixture::new(&metadata);

        let (output, report) = fixture.acceptance();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert_eq!(report["status"], "not_declared");
        assert_eq!(report["declared"], 0);
        assert_eq!(report["satisfied"], 0);
        assert_eq!(report["criteria"], json!([]));
        assert!(!fixture.path(STATE).exists());
        assert!(!fixture.path(VERIFICATION).exists());

        fixture.prepare();
        fixture.write_report(&["unit"], &["service.py"]);
        let (output, report) = fixture.audit();
        assert_success(&output);
        assert_eq!(report["verdict"], "held", "{report}");
        assert!(report.get("acceptance").is_none(), "{report}");
        assert!(!has_finding(&report, "acceptance_criterion_unmet"));
        assert_success(&fixture.run(&["run-task", SPEC, "--post-only"]));
        assert_eq!(fixture.state()["status"], "history_review_required");
        assert!(!fixture.path(".mastermind/executions").exists());
        assert!(!fixture.path(VERIFICATION).exists());
    }

    #[test]
    fn invalid_presence_or_non_observed_references_cannot_enter_preflight() {
        let valid = metadata(&["unit"]);
        let fixture = Fixture::new(&valid);
        for invalid in [
            "null",
            "empty",
            "duplicate_criterion",
            "unknown_field",
            "unknown_check",
            "duplicate_check",
            "empty_checks",
            "missing_statement",
            "placeholder",
            "label_only",
            "command_only",
        ] {
            let mut metadata = valid.clone();
            match invalid {
                "null" => metadata["acceptance"] = Value::Null,
                "empty" => metadata["acceptance"] = json!([]),
                "duplicate_criterion" => {
                    let duplicate = metadata["acceptance"][0].clone();
                    metadata["acceptance"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                }
                "unknown_field" => metadata["acceptance"][0]["satisfied"] = json!(true),
                "unknown_check" => metadata["acceptance"][0]["checks"] = json!(["unknown"]),
                "duplicate_check" => metadata["acceptance"][0]["checks"] = json!(["unit", "unit"]),
                "empty_checks" => metadata["acceptance"][0]["checks"] = json!([]),
                "missing_statement" => {
                    metadata["acceptance"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("statement");
                }
                "placeholder" => {
                    metadata["acceptance"][0]["statement"] = json!("<observable requirement>")
                }
                "label_only" => metadata["verify"] = json!(["unit"]),
                "command_only" => {
                    metadata["verify"][0].as_object_mut().unwrap().remove("run");
                }
                _ => unreachable!(),
            }
            fixture.write_spec(&metadata);
            let output = fixture.run(&["run-task", SPEC, "--pre-only"]);
            assert!(!output.status.success(), "{invalid}: {output:?}");
            assert!(!fixture.path(STATE).exists(), "{invalid}");
            assert!(!fixture.path(VERIFICATION).exists(), "{invalid}");
            assert!(
                !fixture.path(".mastermind/executions").exists(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn every_check_is_required_and_executor_pass_cannot_supply_missing_proof() {
        let fixture = Fixture::new(&metadata(&["unit", "integration"]));
        fixture.prepare();
        fixture.write_report(&["unit", "integration"], &["service.py"]);

        let (output, missing) = fixture.acceptance();
        assert_blocked(&output, &missing);
        assert_eq!(missing["declared"], 1);
        assert_eq!(missing["satisfied"], 0);
        for id in ["unit", "integration"] {
            let evidence = check(&missing, "service-behavior", id);
            assert_eq!(evidence["status"], "missing");
            assert!(evidence["receipt_revision"].is_null());
            assert!(evidence["run_id"].is_null());
        }
        assert!(!fixture.path(VERIFICATION).exists());
        let (output, audit) = fixture.audit();
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(audit["verdict"], "broken");
        assert_eq!(audit["acceptance"]["status"], "blocked");
        assert!(audit["findings"].as_array().unwrap().iter().any(|finding| {
            finding["kind"] == "acceptance_criterion_unmet"
                && finding["id"] == "service-behavior"
                && finding["reason"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty())
        }));
        assert!(!fixture.path(".mastermind/executions").exists());

        let (output, unit) = fixture.verify("unit");
        assert_success(&output);
        let (output, partial) = fixture.acceptance();
        assert_blocked(&output, &partial);
        assert_eq!(partial["satisfied"], 0);
        assert_eq!(
            check(&partial, "service-behavior", "unit")["status"],
            "current"
        );
        assert_eq!(
            check(&partial, "service-behavior", "unit")["run_id"],
            unit["run_id"]
        );
        assert_eq!(
            check(&partial, "service-behavior", "integration")["status"],
            "missing"
        );

        assert_success(&fixture.verify("integration").0);
        let (output, complete) = fixture.acceptance();
        assert_success(&output);
        assert_eq!(complete["status"], "requirements_satisfied");
        assert_eq!(complete["satisfied"], 1);
        assert_eq!(
            complete["criteria"][0]["checks"].as_array().unwrap().len(),
            2
        );
        for id in ["unit", "integration"] {
            assert_eq!(
                check(&complete, "service-behavior", id)["status"],
                "current"
            );
        }
        let (output, audit) = fixture.audit();
        assert_success(&output);
        assert_eq!(audit["verdict"], "held");
        assert_eq!(audit["acceptance"], complete);
        assert_success(&fixture.run(&["run-task", SPEC, "--post-only"]));
        assert_eq!(fixture.state()["status"], "history_review_required");
    }

    #[test]
    fn satisfied_acceptance_does_not_bypass_an_inconsistent_executor_report() {
        let fixture = Fixture::new(&metadata(&["unit"]));
        fixture.prepare();
        assert_success(&fixture.verify("unit").0);
        // All receipts are current, but this report omits the file actually changed.
        fixture.write_report(&["unit"], &[]);
        let (output, acceptance) = fixture.acceptance();
        assert_success(&output);
        assert_eq!(acceptance["status"], "requirements_satisfied");

        let (output, audit) = fixture.audit();
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(audit["verdict"], "broken");
        assert_eq!(audit["acceptance"]["status"], "requirements_satisfied");
        assert!(has_finding(&audit, "executor_report_missing_changed_file"));
        let post = fixture.run(&["run-task", SPEC, "--post-only"]);
        assert!(!post.status.success(), "{post:?}");
        assert_ne!(fixture.state()["status"], "learned");
        assert_ne!(fixture.state()["status"], "history_review_required");
    }

    #[test]
    fn shared_check_retains_the_same_receipt_identity_for_two_criteria() {
        let mut metadata = metadata(&["unit"]);
        metadata["acceptance"] = json!([
            criterion("return-value", "The service returns two.", &["unit"]),
            criterion(
                "declared-mapping",
                "The declared check supports this criterion.",
                &["unit"]
            )
        ]);
        let fixture = Fixture::new(&metadata);
        fixture.prepare();
        let (output, receipt) = fixture.verify("unit");
        assert_success(&output);
        let original = fixture.receipt_bytes("unit");

        let (output, report) = fixture.acceptance();
        assert_success(&output);
        assert_eq!(report["declared"], 2);
        assert_eq!(report["satisfied"], 2);
        let first = check(&report, "return-value", "unit");
        let second = check(&report, "declared-mapping", "unit");
        assert_eq!(first, second);
        assert_eq!(first["status"], "current");
        assert_eq!(first["run_id"], receipt["run_id"]);
        let revision = first["receipt_revision"].as_str().unwrap();
        assert_eq!(revision.len(), 64);
        assert!(revision.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(fixture.acceptance().1, report);
        assert_eq!(fixture.receipt_bytes("unit"), original);
        assert_eq!(
            fs::read_to_string(fixture.path(".mastermind/executions")).unwrap(),
            "unit\n"
        );
    }

    #[test]
    fn a_new_failed_run_revokes_satisfaction_despite_reported_pass() {
        let fixture = Fixture::new(&metadata(&["unit"]));
        fixture.prepare();
        assert_success(&fixture.verify("unit").0);
        fixture.write_report(&["unit"], &["service.py"]);
        let (output, passed) = fixture.acceptance();
        assert_success(&output);

        fixture.write(".mastermind/fail-unit", "fail\n");
        let (output, failed_receipt) = fixture.verify("unit");
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(failed_receipt["status"], "failed");
        assert_eq!(failed_receipt["exit_code"], 7);
        let (output, blocked) = fixture.acceptance();
        assert_blocked(&output, &blocked);
        let evidence = check(&blocked, "service-behavior", "unit");
        assert_eq!(evidence["status"], "failed");
        assert_eq!(evidence["run_id"], failed_receipt["run_id"]);
        assert_ne!(
            evidence["receipt_revision"],
            check(&passed, "service-behavior", "unit")["receipt_revision"]
        );
        let (output, audit) = fixture.audit();
        assert!(!output.status.success(), "{output:?}");
        assert!(has_finding(&audit, "acceptance_criterion_unmet"));
    }

    #[test]
    fn current_status_detects_source_spec_and_executable_drift_without_rerunning() {
        for change in ["source", "spec", "executable"] {
            let mut metadata = metadata(&["unit"]);
            let fixture = Fixture::new(&metadata);
            fixture.prepare();
            assert_success(&fixture.verify("unit").0);
            assert_success(&fixture.acceptance().0);
            let original = fixture.receipt_bytes("unit");
            match change {
                "source" => fixture.write("dependency.txt", "changed dependency\n"),
                "spec" => {
                    metadata["acceptance"][0]["statement"] =
                        json!("The service returns two and preserves the documented interface.");
                    fixture.write_spec(&metadata);
                }
                "executable" => fixture.write(EXECUTABLE, &format!("{PROBE}\n# changed tool\n")),
                _ => unreachable!(),
            }
            let (output, report) = fixture.acceptance();
            assert_blocked(&output, &report);
            assert_eq!(report["satisfied"], 0, "{change}: {report}");
            let evidence = check(&report, "service-behavior", "unit");
            assert_eq!(
                evidence["status"],
                if change == "spec" {
                    "unavailable"
                } else {
                    "stale"
                },
                "{change}: {report}"
            );
            assert!(evidence["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()));
            if change == "spec" {
                // Re-approving the new statement still cannot reuse the old receipt.
                assert_success(&fixture.run(&["run-task", SPEC, "--pre-only"]));
                let (output, refreshed) = fixture.acceptance();
                assert_blocked(&output, &refreshed);
                assert_eq!(
                    check(&refreshed, "service-behavior", "unit")["status"],
                    "stale"
                );
            }
            assert_eq!(fixture.receipt_bytes("unit"), original, "{change}");
            assert_eq!(
                fs::read_to_string(fixture.path(".mastermind/executions")).unwrap(),
                "unit\n",
                "{change}"
            );
        }
    }
}
