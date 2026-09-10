use mmcg::{
    audit_spec::{self, Bundle, Finding, Report, Verdict},
    executor_report::{self, ExecutorReport},
    indexer::Indexer,
    run_task, spec,
    store::Store,
    verify_spec,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPEC: &str = ".mastermind/tasks/001-verification/spec.md";
const FIRST: &str = "echo first --mode focused";
const SECOND: &str = "echo second && echo ready";

fn report_value(rows: Vec<Value>) -> Value {
    json!({
        "schema_version": 1, "spec": SPEC, "status": "complete",
        "phases": [], "files_modified": ["service.py"], "claims": [],
        "defects": [], "verifications": rows,
    })
}

fn passed(cmd: &str) -> Value {
    json!({"cmd": cmd, "result": "pass"})
}

fn executor(rows: Vec<Value>) -> ExecutorReport {
    executor_report::parse_canonical_str(&report_value(rows).to_string()).unwrap()
}

fn spec_text(verify: Value, body: &str) -> String {
    let metadata = json!({
        "mode": "lite", "touches": [{"file": "service.py", "symbols": ["keep"]}],
        "verify": verify,
    });
    format!(
        "---\n{}---\n# Service verification\n## Goals\nUpdate service behavior.\n{body}\n",
        serde_norway::to_string(&metadata).unwrap()
    )
}

struct Fixture {
    store: Store,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new(commands: &[&str]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for args in [
            vec!["init", "-q", "--initial-branch=main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            git(root, &args);
        }
        std::fs::write(root.join(".gitignore"), "graph.db*\n.mastermind/\n").unwrap();
        std::fs::write(root.join("service.py"), "def keep():\n    return 1\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "baseline"]);
        git(root, &["tag", "baseline"]);
        std::fs::create_dir_all(root.join(SPEC).parent().unwrap()).unwrap();
        let declarations: Vec<_> = commands.iter().map(|cmd| json!({"cmd": cmd})).collect();
        std::fs::write(root.join(SPEC), spec_text(json!(declarations), "")).unwrap();
        let mut store = Store::open(root.join("graph.db")).unwrap();
        Indexer::new(root).index_all(&mut store, false).unwrap();
        Self { store, directory }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn spec(&self) -> PathBuf {
        self.root().join(SPEC)
    }

    fn change(&mut self) {
        std::fs::write(
            self.root().join("service.py"),
            "def keep():\n    return 2\n",
        )
        .unwrap();
        Indexer::new(self.root())
            .index_all(&mut self.store, false)
            .unwrap();
    }

    fn audit(&self, report: Option<&ExecutorReport>) -> Report {
        audit_spec::run_with_report(
            &spec::parse_file(&self.spec()).unwrap(),
            &self.store,
            self.root(),
            "baseline",
            report,
        )
        .unwrap()
    }

    fn write_report(&self, rows: Vec<Value>) {
        std::fs::write(
            self.spec().with_file_name("executor-report.md"),
            report_value(rows).to_string(),
        )
        .unwrap();
    }

    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mmcg"))
            .current_dir(self.root())
            .args(["--index", "graph.db"])
            .args(args)
            .env("MMCG_QUERY_BUDGET_MS", "60000")
            .output()
            .unwrap()
    }

    fn run_task(&self, pre_only: bool) -> run_task::Outcome {
        run_task::run(
            &self.spec(),
            self.root(),
            &self.root().join("graph.db"),
            run_task::RunOpts {
                pre_only,
                post_only: !pre_only,
                ..Default::default()
            },
        )
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
}

fn unmet(report: &Report) -> Vec<(&str, &str)> {
    report
        .findings
        .iter()
        .filter_map(|finding| match finding {
            Finding::VerificationRequirementUnmet { cmd, reason } => {
                Some((cmd.as_str(), reason.as_str()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn verification_coverage_collects_explicit_commands_without_shell_equivalence() {
    let parsed = spec::parse_str(SPEC, &spec_text(
        json!(["typecheck", {"cmd": ""}, {"cmd": "  \t"}, {"cmd": format!("  {FIRST}  ")},
               {"cmd": FIRST}, {"cmd": "echo first  --mode focused"}, {"cmd": SECOND}]),
        &format!("**VERIFY**: `{FIRST}`\n**VERIFY:** `echo legacy`\nVERIFY: echo bare\n\n## Final Verification\n```sh\necho fenced-only\n```"),
    ));
    assert_eq!(
        parsed.declared_verify_commands(),
        [
            FIRST,
            "echo first  --mode focused",
            SECOND,
            "echo legacy",
            "echo bare"
        ]
    );
    assert_eq!(parsed.frontmatter.unwrap().verify[0].label(), "typecheck");
}

#[test]
fn verification_coverage_legacy_spans_preserve_embedded_backticks() {
    let first = "echo `probe`";
    let second = "echo \"`` проверка\"";
    let mut fixture = Fixture::new(&[]);
    let body = format!("VERIFY: `` {first} ``\n**VERIFY**: ``` {second} ``` — check output");
    std::fs::write(fixture.spec(), spec_text(json!([]), &body)).unwrap();
    let parsed = spec::parse_file(&fixture.spec()).unwrap();
    assert_eq!(parsed.declared_verify_commands(), [first, second]);
    fixture.change();
    let missing = fixture.audit(Some(&executor(vec![])));
    assert_eq!(missing.verdict, Verdict::Broken);
    assert_eq!(
        unmet(&missing),
        [(first, "missing_result"), (second, "missing_result")]
    );
    assert_eq!(
        fixture
            .audit(Some(&executor(vec![passed(first), passed(second)])))
            .verdict,
        Verdict::Held
    );

    let unmatched = spec::parse_str(SPEC, &spec_text(json!([]), "VERIFY: ``echo incomplete`"));
    assert_eq!(unmatched.declared_verify_commands(), ["``echo incomplete`"]);
    let longer = spec::parse_str(SPEC, &spec_text(json!([]), "VERIFY: `` echo '```' ``"));
    assert_eq!(longer.declared_verify_commands(), ["echo '```'"]);
}

#[test]
fn verification_coverage_strict_preflight_requires_a_nonempty_command() {
    for verify in [
        json!([]),
        json!(["typecheck"]),
        json!([{"cmd": ""}]),
        json!(["tests", {"cmd": " \t\n "}]),
    ] {
        let parsed = spec::parse_str(
            SPEC,
            &spec_text(verify.clone(), "```sh\necho fenced-only\n```"),
        );
        let findings = verify_spec::strict_check(&parsed);
        assert_eq!(findings.len(), 1, "{verify}: {findings:?}");
        assert!(
            matches!(&findings[0], verify_spec::Finding::StrictViolation { reason } if reason.contains("no verify command"))
        );
        let legacy = spec::parse_str(SPEC, &spec_text(verify, "VERIFY: `echo checked`"));
        assert!(verify_spec::strict_check(&legacy).is_empty());
    }
    let parsed = spec::parse_str(
        SPEC,
        &spec_text(json!(["tests", {"cmd": "  echo checked  "}]), ""),
    );
    assert!(verify_spec::strict_check(&parsed).is_empty());
}

#[test]
fn verification_coverage_audit_rejects_missing_partial_and_different_commands() {
    let mut fixture = Fixture::new(&[FIRST, SECOND]);
    fixture.change();
    for (rows, missing) in [
        (vec![], vec![FIRST, SECOND]),
        (vec![passed("echo unrelated")], vec![FIRST, SECOND]),
        (vec![passed(FIRST)], vec![SECOND]),
        (
            vec![passed("echo first --mode broad"), passed(SECOND)],
            vec![FIRST],
        ),
        (
            vec![passed("echo first  --mode focused"), passed(SECOND)],
            vec![FIRST],
        ),
        (
            vec![passed("env echo first --mode focused"), passed(SECOND)],
            vec![FIRST],
        ),
        (
            vec![passed("ECHO first --mode focused"), passed(SECOND)],
            vec![FIRST],
        ),
        (
            vec![passed(FIRST), passed("echo second"), passed("echo ready")],
            vec![SECOND],
        ),
    ] {
        let report = fixture.audit(Some(&executor(rows)));
        assert_eq!(report.verdict, Verdict::Broken, "{report:?}");
        let expected: Vec<_> = missing
            .into_iter()
            .map(|cmd| (cmd, "missing_result"))
            .collect();
        assert_eq!(unmet(&report), expected);
        assert!(report
            .render_text()
            .contains("verification_requirement_unmet"));
        let bundle = Bundle::from_report(&report, None);
        assert_eq!(bundle.verdict, "broken");
        assert!(bundle
            .human_summary
            .contains(&format!("{} errors, 0 warnings", expected.len())));
    }
    let report = fixture.audit(Some(&executor(vec![
        passed(&format!(" \t{FIRST}\n")),
        passed(SECOND),
        passed("echo extra"),
    ])));
    assert_eq!(report.verdict, Verdict::Held, "{report:?}");
    assert!(report.findings.is_empty());
}

#[test]
fn verification_coverage_checks_all_duplicate_rows_and_observed_failures() {
    let mut fixture = Fixture::new(&[FIRST]);
    fixture.change();
    let bad = json!({"cmd": FIRST, "result": "pass", "observed": {"exit_code": 7}});
    for rows in [
        vec![passed(FIRST), bad.clone()],
        vec![bad.clone(), passed(FIRST)],
    ] {
        let report = fixture.audit(Some(&executor(rows)));
        assert_eq!(report.verdict, Verdict::Broken);
        assert_eq!(unmet(&report), [(FIRST, "conflicting_results")]);
        assert!(report
            .findings
            .iter()
            .any(|f| matches!(f, Finding::ObservedExitCodeNonZero { exit_code: 7, .. })));
        assert_eq!(report.executor_report.unwrap().verify.len(), 2);
    }
    let report = fixture.audit(Some(&executor(vec![bad])));
    assert_eq!(unmet(&report), [(FIRST, "not_passed")]);
    let report = fixture.audit(Some(&executor(vec![
        json!({"cmd": FIRST, "result": "pass", "output_excerpt": "first run", "observed": {"tests_run": 1}}),
        json!({"cmd": format!(" {FIRST} "), "result": "pass", "output_excerpt": "second run", "observed": {"exit_code": 0, "tests_run": 2}}),
    ])));
    assert_eq!(report.verdict, Verdict::Held, "{report:?}");
    assert_eq!(report.executor_report.unwrap().verify.len(), 2);
    let report = fixture.audit(Some(&executor(vec![
        json!({"cmd": FIRST, "result": "pass", "observed": {}}),
    ])));
    assert_eq!(report.verdict, Verdict::Held);

    let mut partial = report_value(vec![json!({"cmd": FIRST, "result": "fail"})]);
    partial["status"] = json!("partial");
    partial["defects"] = json!([{"kind": "verification_failed", "phase": "final", "details": "check failed", "remediation_hint": "fix and rerun"}]);
    let failed = executor_report::parse_canonical_str(&partial.to_string()).unwrap();
    let report = fixture.audit(Some(&failed));
    assert_eq!(report.verdict, Verdict::Broken);
    assert_eq!(unmet(&report), [(FIRST, "not_passed")]);
    partial["verifications"]
        .as_array_mut()
        .unwrap()
        .push(passed(FIRST));
    let conflicting = executor_report::parse_canonical_str(&partial.to_string()).unwrap();
    assert_eq!(
        unmet(&fixture.audit(Some(&conflicting))),
        [(FIRST, "conflicting_results")]
    );
    let mut unknown = executor(vec![passed(FIRST)]);
    unknown.verify[0].claimed = None;
    assert_eq!(
        unmet(&fixture.audit(Some(&unknown))),
        [(FIRST, "not_passed")]
    );
}

#[test]
fn verification_coverage_preserves_no_requirement_and_legacy_audits() {
    let mut fixture = Fixture::new(&[]);
    fixture.change();
    std::fs::write(
        fixture.spec(),
        spec_text(
            json!(["typecheck", {"cmd": " "}]),
            "```sh\necho prose-only\n```",
        ),
    )
    .unwrap();
    assert_eq!(
        fixture.audit(Some(&executor(vec![]))).verdict,
        Verdict::Held
    );
    std::fs::write(fixture.spec(), spec_text(json!([{"cmd": FIRST}]), "")).unwrap();
    assert_eq!(fixture.audit(None).verdict, Verdict::Held);
    let legacy = executor_report::parse_str("claims: []\nverify: []\n").unwrap();
    assert_eq!(fixture.audit(Some(&legacy)).verdict, Verdict::Held);
    assert_eq!(
        fixture.audit(Some(&executor(vec![]))).verdict,
        Verdict::Broken
    );
}

#[test]
fn verification_coverage_cli_strict_preflight_rejects_labels_and_empty_commands() {
    let fixture = Fixture::new(&[]);
    for (verify, valid) in [
        (json!(["typecheck"]), false),
        (json!([{"cmd": ""}]), false),
        (json!([{"cmd": " \t "}]), false),
        (json!(["tests", {"cmd": "echo checked"}]), true),
    ] {
        std::fs::write(fixture.spec(), spec_text(verify.clone(), "")).unwrap();
        let output = fixture.command(&["verify-spec", SPEC, "--strict", "--json"]);
        assert_eq!(output.status.success(), valid, "{verify}: {output:?}");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["kind"] == "strict_violation"
                    && f["reason"].as_str().unwrap().contains("no verify command")),
            !valid
        );
    }
}

#[test]
fn verification_coverage_cli_ci_and_bundle_preserve_missing_command_errors() {
    for legacy_declaration in [false, true] {
        let mut fixture = Fixture::new(&[FIRST, SECOND]);
        if legacy_declaration {
            std::fs::write(
                fixture.spec(),
                spec_text(
                    json!([]),
                    &format!("VERIFY: `{FIRST}`\n**VERIFY**: `{SECOND}`"),
                ),
            )
            .unwrap();
        }
        fixture.change();
        git(fixture.root(), &["add", "service.py"]);
        git(fixture.root(), &["commit", "-q", "-m", "change"]);
        for complete in [false, true] {
            let rows = if complete {
                vec![passed(FIRST), passed(SECOND)]
            } else {
                vec![passed(FIRST)]
            };
            fixture.write_report(rows);
            let report_path = fixture.spec().with_file_name("executor-report.md");
            let output = fixture.command(&[
                "audit-spec",
                SPEC,
                "--since",
                "baseline",
                "--executor-report",
                report_path.to_str().unwrap(),
                "--json",
            ]);
            assert_eq!(output.status.success(), complete, "{output:?}");
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            if !complete {
                assert_eq!(
                    report["findings"],
                    json!([{"kind": "verification_requirement_unmet", "cmd": SECOND, "reason": "missing_result"}])
                );
                let lessons =
                    std::fs::read_to_string(fixture.root().join(".mastermind/tasks/_lessons.md"))
                        .unwrap();
                assert!(lessons.contains("verification requirement unmet"));
            }
            let ci = fixture.command(&[
                "ci",
                "--since",
                "baseline",
                "--require-executor-report",
                "--bundle-dir",
                ".mastermind/output",
            ]);
            assert_eq!(ci.status.success(), complete, "{ci:?}");
            let envelope: Value = serde_json::from_slice(
                &std::fs::read(
                    fixture
                        .root()
                        .join(".mastermind/output/001-verification.bundle.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(envelope["schema_version"], 3);
            assert_eq!(
                envelope["manifest"]["verdict"],
                if complete { "held" } else { "broken" }
            );
            assert_eq!(envelope["manifest"]["discrepancies"], report["findings"]);
            if !complete {
                assert!(envelope["manifest"]["human_summary"]
                    .as_str()
                    .unwrap()
                    .contains("1 errors, 0 warnings"));
            }
        }
    }
}

#[test]
fn verification_coverage_controller_rejects_recovers_and_never_executes_commands() {
    let command = "echo proof > .mastermind/should-not-run";
    let mut fixture = Fixture::new(&[FIRST, command]);
    assert_eq!(fixture.run_task(true), run_task::Outcome::PreReady);
    fixture.change();
    fixture.write_report(vec![passed(FIRST)]);
    assert_eq!(fixture.run_task(false), run_task::Outcome::PostBroken);
    let state_path = run_task::state_file_path(fixture.root(), &fixture.spec());
    let failed = run_task::load_state(&state_path).unwrap().unwrap();
    assert_eq!(failed.next_step.as_deref(), Some("planner_review"));
    assert!(failed.held_snapshot_sha256.is_none() && failed.history_snapshot_sha256.is_none());
    assert!(!run_task::release_file_path(fixture.root(), &fixture.spec()).exists());
    fixture.write_report(vec![passed(FIRST), passed(command)]);
    assert_eq!(fixture.run_task(false), run_task::Outcome::PostHeld);
    let recovered = run_task::load_state(&state_path).unwrap().unwrap();
    assert_eq!(recovered.baseline_ref, failed.baseline_ref);
    assert_eq!(recovered.iteration, failed.iteration);
    assert_eq!(recovered.next_step.as_deref(), Some("review_history"));
    assert!(recovered.history_snapshot_sha256.is_some());
    fixture.write_report(vec![passed(command)]);
    assert_eq!(fixture.run_task(false), run_task::Outcome::PostBroken);
    let repeated = run_task::load_state(&state_path).unwrap().unwrap();
    assert_eq!(repeated.next_step.as_deref(), Some("planner_review"));
    assert!(repeated.history_snapshot_sha256.is_none());
    assert!(!fixture.root().join(".mastermind/should-not-run").exists());
}
