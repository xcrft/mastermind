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
        Self::with_test_files(commands, false)
    }

    fn with_test_files(commands: &[&str], include_tests: bool) -> Self {
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
        if include_tests {
            std::fs::create_dir(root.join("src")).unwrap();
            std::fs::write(root.join("src/lib.rs"), "#[test]\nfn existing_test() {}\n").unwrap();
        }
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

    fn support_file(&self, path: &str, bytes: &[u8]) {
        let path = self.root().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
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

fn observed(cmd: &str, outcome: Value) -> Value {
    json!({"cmd": cmd, "result": "pass", "observed": outcome})
}

#[test]
fn zero_test_observations_reject_recognized_runs_with_or_without_exit_code() {
    let mut fixture = Fixture::with_test_files(&[], true);
    fixture.change();
    for cmd in [
        "cargo test",
        " \n cargo test \r\n",
        "cargo.exe test --manifest-path=Cargo.toml --locked existing_test",
        "cargo test --package app --lib --features one,two",
        "go test ./...",
        "go.exe test -v -count=1 ./...",
        "go test -count 2 -run TestService",
        "pytest",
        "pytest -v --setup-show tests/test_service.py",
        "python -m pytest -k service",
        "python3.exe -m pytest -q tests/test_service.py::test_keep",
        "jest --ci --runInBand",
        "jest.exe --testNamePattern=keep",
        "vitest run",
        "vitest --run",
        "vitest.exe --run -t keep",
        "vitest --run -t list",
    ] {
        for outcome in [
            json!({"tests_run": 0}),
            json!({"tests_run": 0, "exit_code": 0}),
        ] {
            let report = fixture.audit(Some(&executor(vec![observed(cmd, outcome)])));
            assert_eq!(report.verdict, Verdict::Broken, "{cmd}: {report:?}");
            assert!(
                matches!(report.findings.as_slice(), [Finding::ObservedZeroTests { cmd: actual }] if actual == cmd)
            );
        }
    }
}

#[test]
fn zero_test_observations_allow_compile_discovery_and_unknown_commands() {
    let mut fixture = Fixture::new(&[]);
    fixture.change();
    for cmd in [
        "cargo check",
        "cargo build --release",
        "cargo fmt --check",
        "cargo clippy --all-targets",
        "python3 -m py_compile service.py",
        "ruff check .",
        "cargo test --no-run",
        "cargo test --help",
        "cargo test -- --list",
        "cargo test -- --bench",
        "cargo test --config runner=custom",
        "cargo test --manifest-path",
        "cargo test --manifest-path=",
        "cargo test --manifest-path --no-run",
        "cargo test --unknown-flag",
        "go test -c",
        "go test -n ./...",
        "go test -list .",
        "go test -count=0",
        "go test -count -1",
        "go test -count=invalid",
        "go test -exec custom",
        "go test -args -test.list=.",
        "go test -bench .",
        "go test -fuzz FuzzService",
        "pytest --collect-only",
        "pytest --co",
        "pytest --fixtures",
        "pytest --setup-only",
        "pytest --setup-plan",
        "pytest --cache-show",
        "pytest --markers",
        "pytest --help",
        "pytest --version",
        "pytest @args.txt",
        "pytest -p custom",
        "pytest -o addopts=--collect-only",
        "jest -v",
        "jest --collectTests",
        "jest --listTests",
        "jest --showConfig",
        "jest --clearCache",
        "jest --watch",
        "jest --passWithNoTests",
        "jest --ci=false",
        "vitest",
        "vitest watch",
        "vitest list",
        "vitest bench",
        "vitest run --watch",
        "vitest run --passWithNoTests",
        "vitest --run=false",
        "vitest --run list",
        "vitest --run bench",
        "vitest --run watch",
        "vitest --run init browser",
        "vitest --run related service.ts",
        "vitest --run service.test.ts",
        "npm test",
        "yarn test",
        "npx jest",
        "uv run pytest",
        "env cargo test",
        "cargo test && echo ready",
        "cargo test; echo ready",
        "cargo test\necho ready",
        "cargo test | tee output",
        "echo cargo test",
        "echo \"cargo test\"",
        "cargo test --manifest-path \"Cargo.toml\"",
        "cargo test --package $PACKAGE",
        "cargo test --package *",
        "./cargo test",
    ] {
        for outcome in [
            json!({"tests_run": 0}),
            json!({"tests_run": 0, "exit_code": 0}),
        ] {
            std::fs::write(fixture.spec(), spec_text(json!([{"cmd": cmd}]), "")).unwrap();
            let report = fixture.audit(Some(&executor(vec![observed(cmd, outcome)])));
            assert_eq!(report.verdict, Verdict::Held, "{cmd}: {report:?}");
            assert!(report.findings.is_empty(), "{cmd}: {report:?}");
        }
    }
}

#[test]
fn zero_test_observations_preserve_nonzero_exit_precedence_for_all_commands() {
    let mut fixture = Fixture::new(&[]);
    fixture.change();
    for cmd in [
        "cargo check",
        "cargo test",
        "pytest --co",
        "env custom tests",
    ] {
        for count in [0, 3] {
            let report = fixture.audit(Some(&executor(vec![observed(
                cmd,
                json!({"exit_code": 7, "tests_run": count}),
            )])));
            assert_eq!(report.verdict, Verdict::Broken);
            assert!(
                matches!(report.findings.as_slice(), [Finding::ObservedExitCodeNonZero { cmd: actual, exit_code: 7 }] if actual == cmd)
            );
        }
    }
}

#[test]
fn zero_test_observations_preserve_optional_evidence_and_advisory_scan_boundaries() {
    let mut with_tests = Fixture::with_test_files(&["cargo test"], true);
    with_tests.change();
    for row in [
        passed("cargo test"),
        observed("cargo test", json!({})),
        observed("cargo test", json!({"exit_code": 0})),
    ] {
        let report = with_tests.audit(Some(&executor(vec![row])));
        assert_eq!(report.verdict, Verdict::Held, "{report:?}");
    }

    let mut bare = Fixture::new(&[]);
    bare.change();
    let advisory = bare.audit(Some(&executor(vec![passed("cargo test")])));
    assert_eq!(advisory.verdict, Verdict::Drift);
    assert!(matches!(
        advisory.findings.as_slice(),
        [Finding::VacuousTestClaim { .. }]
    ));
    for cmd in ["cargo test", "go test", "pytest", "jest", "vitest run"] {
        for outcome in [
            json!({"tests_run": 2}),
            json!({"tests_run": 2, "exit_code": 0}),
        ] {
            let report = bare.audit(Some(&executor(vec![observed(cmd, outcome)])));
            assert_eq!(report.verdict, Verdict::Held, "{cmd}: {report:?}");
        }
    }
}

#[test]
fn zero_test_observations_cannot_be_hidden_by_duplicate_passing_rows() {
    let cmd = "cargo test";
    let mut fixture = Fixture::with_test_files(&[cmd], true);
    fixture.change();
    for outcome in [
        json!({"tests_run": 0}),
        json!({"tests_run": 0, "exit_code": 0}),
    ] {
        let zero = observed(" \n cargo test \r\n", outcome);
        let good = observed(" cargo test ", json!({"tests_run": 2}));
        for rows in [vec![zero.clone(), good.clone()], vec![good, zero.clone()]] {
            let report = fixture.audit(Some(&executor(rows)));
            assert_eq!(report.verdict, Verdict::Broken);
            assert_eq!(unmet(&report), [(cmd, "conflicting_results")]);
            assert!(report
                .findings
                .iter()
                .any(|f| matches!(f, Finding::ObservedZeroTests { .. })));
            assert_eq!(report.executor_report.unwrap().verify.len(), 2);
        }
        let report = fixture.audit(Some(&executor(vec![zero.clone(), zero])));
        assert_eq!(unmet(&report), [(cmd, "not_passed")]);
    }
}

#[test]
fn zero_test_observations_keep_legacy_pass_aliases_without_requiring_coverage() {
    let mut fixture = Fixture::with_test_files(&["echo undeclared"], true);
    fixture.change();
    for claim in ["pass", "passed", "PASS", "Passed"] {
        let value = json!({"claims": [], "verify": [{"cmd": "cargo test", "claimed": claim, "observed": {"tests_run": 0}}]});
        let legacy = executor_report::parse_str(&value.to_string()).unwrap();
        let report = fixture.audit(Some(&legacy));
        assert_eq!(report.verdict, Verdict::Broken);
        assert!(matches!(
            report.findings.as_slice(),
            [Finding::ObservedZeroTests { .. }]
        ));
        assert!(unmet(&report).is_empty());
    }
}

#[test]
fn zero_test_observations_cli_ci_and_bundle_reject_and_recover() {
    let cmd = "cargo test";
    let mut fixture = Fixture::with_test_files(&[cmd], true);
    fixture.change();
    git(fixture.root(), &["add", "service.py"]);
    git(fixture.root(), &["commit", "-q", "-m", "change"]);
    for count in [0, 2] {
        fixture.write_report(vec![observed(cmd, json!({"tests_run": count}))]);
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
        assert_eq!(output.status.success(), count > 0, "{output:?}");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        if count == 0 {
            assert_eq!(
                report["findings"],
                json!([
                    {"kind": "verification_requirement_unmet", "cmd": cmd, "reason": "not_passed"},
                    {"kind": "observed_zero_tests", "cmd": cmd},
                ])
            );
        }
        let ci = fixture.command(&[
            "ci",
            "--since",
            "baseline",
            "--require-executor-report",
            "--bundle-dir",
            ".mastermind/output",
        ]);
        assert_eq!(ci.status.success(), count > 0, "{ci:?}");
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
            if count > 0 { "held" } else { "broken" }
        );
        let mut expected = report["findings"].as_array().unwrap().clone();
        expected.sort_by_key(Value::to_string);
        assert_eq!(envelope["manifest"]["discrepancies"], json!(expected));
        if count == 0 {
            assert!(envelope["manifest"]["human_summary"]
                .as_str()
                .unwrap()
                .contains("2 errors, 0 warnings"));
            let lessons =
                std::fs::read_to_string(fixture.root().join(".mastermind/tasks/_lessons.md"))
                    .unwrap();
            assert!(lessons.contains("observed zero tests"));
        }
    }
}

#[test]
fn zero_test_observations_controller_rejects_and_recovers_without_execution() {
    let cmd = "cargo test";
    let marker = "echo proof > .mastermind/should-not-run";
    let mut fixture = Fixture::with_test_files(&[cmd, marker], true);
    assert_eq!(fixture.run_task(true), run_task::Outcome::PreReady);
    fixture.change();
    fixture.write_report(vec![observed(cmd, json!({"tests_run": 0})), passed(marker)]);
    assert_eq!(fixture.run_task(false), run_task::Outcome::PostBroken);
    let state_path = run_task::state_file_path(fixture.root(), &fixture.spec());
    let failed = run_task::load_state(&state_path).unwrap().unwrap();
    assert_eq!(failed.next_step.as_deref(), Some("planner_review"));
    assert!(failed.history_snapshot_sha256.is_none());
    assert!(!run_task::release_file_path(fixture.root(), &fixture.spec()).exists());
    fixture.write_report(vec![observed(cmd, json!({"tests_run": 2})), passed(marker)]);
    assert_eq!(fixture.run_task(false), run_task::Outcome::PostHeld);
    let recovered = run_task::load_state(&state_path).unwrap().unwrap();
    assert_eq!(recovered.baseline_ref, failed.baseline_ref);
    assert_eq!(recovered.iteration, failed.iteration);
    assert!(recovered.history_snapshot_sha256.is_some());
    fixture.write_report(vec![observed(cmd, json!({"tests_run": 0})), passed(marker)]);
    assert_eq!(fixture.run_task(false), run_task::Outcome::PostBroken);
    let repeated = run_task::load_state(&state_path).unwrap().unwrap();
    assert_eq!(repeated.next_step.as_deref(), Some("planner_review"));
    assert!(repeated.held_snapshot_sha256.is_none() && repeated.history_snapshot_sha256.is_none());
    assert!(!fixture.root().join(".mastermind/should-not-run").exists());
}

const SCAN_MANIFEST: &str = ".mastermind/scan/Cargo.toml";
const SCAN_COMMAND: &str = "cargo test --manifest-path .mastermind/scan/Cargo.toml";

fn scan_fixture() -> Fixture {
    let mut fixture = Fixture::new(&[]);
    fixture.support_file(SCAN_MANIFEST, b"[package]\nname = \"scan\"\n");
    fixture.support_file(".mastermind/scan/src/lib.rs", b"pub fn helper() {}\n");
    fixture.change();
    fixture
}

#[test]
fn test_scan_audit_handles_contained_and_recursive_scopes() {
    let fixture = scan_fixture();
    let empty = fixture.audit(Some(&executor(vec![passed(SCAN_COMMAND)])));
    assert_eq!(empty.verdict, Verdict::Drift, "{empty:?}");
    assert!(matches!(
        empty.findings.as_slice(),
        [Finding::VacuousTestClaim { .. }]
    ));
    fixture.support_file(
        ".mastermind/scan/tests/check.rs",
        b"#[test]\nfn check() {}\n",
    );
    assert_eq!(
        fixture
            .audit(Some(&executor(vec![passed(SCAN_COMMAND)])))
            .verdict,
        Verdict::Held
    );

    let cmd = "go test -run ./decoy ./.mastermind/scan/go/...";
    fixture.support_file(".mastermind/scan/go/helper.go", b"package scan\n");
    assert_eq!(
        fixture.audit(Some(&executor(vec![passed(cmd)]))).verdict,
        Verdict::Drift
    );
    fixture.support_file(".mastermind/scan/go/pkg/scan_test.go", b"package scan\n");
    assert_eq!(
        fixture.audit(Some(&executor(vec![passed(cmd)]))).verdict,
        Verdict::Held
    );
    assert_eq!(
        fixture
            .audit(Some(&executor(vec![passed(
                "go test ./.mastermind/scan/go"
            )])))
            .verdict,
        Verdict::Drift
    );

    for name in ["test_scan.py", "scan_test.py"] {
        let cmd = format!("python3 -m pytest -k decoy .mastermind/{name}/suite");
        fixture.support_file(&format!(".mastermind/{name}/suite/helper.py"), b"pass\n");
        assert_eq!(
            fixture.audit(Some(&executor(vec![passed(&cmd)]))).verdict,
            Verdict::Drift
        );
        fixture.support_file(
            &format!(".mastermind/{name}/suite/nested/{name}"),
            b"def test_scan(): pass\n",
        );
        assert_eq!(
            fixture.audit(Some(&executor(vec![passed(&cmd)]))).verdict,
            Verdict::Held
        );
        let explicit = format!("pytest .mastermind/{name}/suite/helper.py");
        assert_eq!(
            fixture
                .audit(Some(&executor(vec![passed(&explicit)])))
                .verdict,
            Verdict::Held
        );
    }
    for cmd in ["jest", "vitest run"] {
        assert_eq!(
            fixture.audit(Some(&executor(vec![passed(cmd)]))).verdict,
            Verdict::Drift
        );
    }
    fixture.support_file(
        ".mastermind/javascript/nested/scan.spec.ts",
        b"export {};\n",
    );
    for cmd in ["jest", "vitest run"] {
        assert_eq!(
            fixture.audit(Some(&executor(vec![passed(cmd)]))).verdict,
            Verdict::Held
        );
    }
}

#[test]
fn test_scan_audit_keeps_external_and_unsupported_scopes_unknown() {
    let fixture = scan_fixture();
    let outside = tempfile::tempdir_in(fixture.root().parent().unwrap()).unwrap();
    std::fs::create_dir(outside.path().join("src")).unwrap();
    std::fs::write(
        outside.path().join("Cargo.toml"),
        b"[package]\nname = \"outside\"\n",
    )
    .unwrap();
    let sibling = outside.path().file_name().unwrap().to_str().unwrap();
    for content in [
        b"pub fn helper() {}\n".as_slice(),
        b"#[test]\nfn check() {}\n",
    ] {
        std::fs::write(outside.path().join("src/lib.rs"), content).unwrap();
        for cmd in [
            format!("cargo test --manifest-path ../{sibling}/Cargo.toml"),
            format!("pytest ../{sibling}"),
        ] {
            let report = fixture.audit(Some(&executor(vec![passed(&cmd)])));
            assert_eq!(report.verdict, Verdict::Held, "{cmd}: {report:?}");
            let zero = fixture.audit(Some(&executor(vec![observed(
                &cmd,
                json!({"tests_run": 0}),
            )])));
            assert!(
                matches!(
                    zero.findings.as_slice(),
                    [Finding::ObservedZeroTests { .. }]
                ),
                "command must reach the recognized test path: {cmd}"
            );
        }
    }
    for cmd in [
        "cargo test --workspace",
        "cargo test --doc",
        "cargo test -p other",
        "go test ./one ./two",
        "go test ./.../pkg/...",
        "go test std",
        "pytest .mastermind/one .mastermind/two",
        "pytest tests/test_app.py::test_app",
        "jest missing-filter",
        "vitest run missing-filter",
    ] {
        let report = fixture.audit(Some(&executor(vec![passed(cmd)])));
        assert_eq!(report.verdict, Verdict::Held, "{cmd}: {report:?}");
        assert!(report.findings.is_empty());
    }
}

#[test]
fn test_scan_audit_does_not_treat_incomplete_reads_as_absence() {
    for case in ["invalid_utf8", "oversized", "deep"] {
        let fixture = scan_fixture();
        assert_eq!(
            fixture
                .audit(Some(&executor(vec![passed(SCAN_COMMAND)])))
                .verdict,
            Verdict::Drift
        );
        match case {
            "invalid_utf8" => fixture.support_file(".mastermind/scan/src/bad.rs", b"\xff"),
            "oversized" => {
                let file =
                    std::fs::File::create(fixture.root().join(".mastermind/scan/src/large.rs"))
                        .unwrap();
                file.set_len(1024 * 1024 + 1).unwrap();
            }
            "deep" => fixture.support_file(
                &format!(".mastermind/scan/src/{}test.rs", "d/".repeat(20)),
                b"#[test]\nfn t() {}\n",
            ),
            _ => unreachable!(),
        }
        let report = fixture.audit(Some(&executor(vec![passed(SCAN_COMMAND)])));
        assert_eq!(report.verdict, Verdict::Held, "{case}: {report:?}");
        assert!(report.findings.is_empty());
    }
}

#[test]
fn test_scan_audit_budget_exhaustion_preserves_hard_observations() {
    let fixture = scan_fixture();
    let file = std::fs::File::create(fixture.root().join(".mastermind/scan/src/large.rs")).unwrap();
    file.set_len(1024 * 1024 + 1).unwrap();
    drop(file);
    let mut rows = vec![passed(SCAN_COMMAND); 16];
    rows.push(observed(SCAN_COMMAND, json!({"tests_run": 0})));
    rows.push(observed("cargo check", json!({"exit_code": 7})));
    let report = fixture.audit(Some(&executor(rows)));
    assert_eq!(report.verdict, Verdict::Broken);
    assert!(matches!(
        report.findings.as_slice(),
        [
            Finding::ObservedZeroTests { .. },
            Finding::ObservedExitCodeNonZero { exit_code: 7, .. }
        ]
    ));
}

#[test]
fn test_scan_cli_ci_bundle_preserve_advisory_severity() {
    let fixture = scan_fixture();
    git(fixture.root(), &["add", "service.py"]);
    git(fixture.root(), &["commit", "-q", "-m", "change"]);
    for (source, verdict) in [
        (b"pub fn helper() {}\n".as_slice(), "drift"),
        (b"\xff", "held"),
        (b"#[test]\nfn t() {}\n", "held"),
    ] {
        fixture.support_file(".mastermind/scan/src/lib.rs", source);
        fixture.write_report(vec![passed(SCAN_COMMAND)]);
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
        assert!(output.status.success(), "{output:?}");
        let audit: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(audit["verdict"], verdict);
        let ci = fixture.command(&[
            "ci",
            "--since",
            "baseline",
            "--require-executor-report",
            "--bundle-dir",
            ".mastermind/output",
        ]);
        assert!(ci.status.success(), "{ci:?}");
        let bundle: Value = serde_json::from_slice(
            &std::fs::read(
                fixture
                    .root()
                    .join(".mastermind/output/001-verification.bundle.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(bundle["schema_version"], 3);
        assert_eq!(bundle["manifest"]["verdict"], verdict);
        assert_eq!(bundle["manifest"]["discrepancies"], audit["findings"]);
        if verdict == "drift" {
            assert_eq!(audit["findings"][0]["kind"], "vacuous_test_claim");
            assert!(bundle["manifest"]["human_summary"]
                .as_str()
                .unwrap()
                .contains("0 errors, 1 warnings"));
        } else {
            assert_eq!(audit["findings"], json!([]));
        }
    }
}

#[test]
fn test_scan_controller_recovers_without_claiming_test_execution() {
    let marker = "echo proof > .mastermind/should-not-run";
    let mut fixture = Fixture::new(&[SCAN_COMMAND, marker]);
    fixture.support_file(SCAN_MANIFEST, b"[package]\nname = \"scan\"\n");
    fixture.support_file(".mastermind/scan/src/lib.rs", b"pub fn helper() {}\n");
    assert_eq!(fixture.run_task(true), run_task::Outcome::PreReady);
    fixture.change();
    fixture.write_report(vec![passed(SCAN_COMMAND), passed(marker)]);
    assert_eq!(fixture.run_task(false), run_task::Outcome::PostDrift);
    let state_path = run_task::state_file_path(fixture.root(), &fixture.spec());
    let drift = run_task::load_state(&state_path).unwrap().unwrap();
    assert_eq!(drift.next_step.as_deref(), Some("planner_review"));
    fixture.support_file(".mastermind/scan/src/lib.rs", b"\xff");
    assert_eq!(fixture.run_task(false), run_task::Outcome::PostHeld);
    let held = run_task::load_state(&state_path).unwrap().unwrap();
    assert_eq!(held.baseline_ref, drift.baseline_ref);
    assert_eq!(held.iteration, drift.iteration);
    fixture.write_report(vec![
        observed(SCAN_COMMAND, json!({"tests_run": 0})),
        passed(marker),
    ]);
    assert_eq!(fixture.run_task(false), run_task::Outcome::PostBroken);
    assert!(!fixture.root().join(".mastermind/should-not-run").exists());
}

#[test]
fn test_scan_cli_resolves_scopes_independently_of_cwd() {
    let fixture = scan_fixture();
    fixture.support_file(
        ".mastermind/nested/.mastermind/scan/src/decoy.rs",
        b"#[test]\nfn t() {}\n",
    );
    fixture.write_report(vec![passed(SCAN_COMMAND)]);
    let report_path = fixture.spec().with_file_name("executor-report.md");
    let output = Command::new(env!("CARGO_BIN_EXE_mmcg"))
        .current_dir(fixture.root().join(".mastermind/nested"))
        .arg("--index")
        .arg(fixture.root().join("graph.db"))
        .arg("audit-spec")
        .arg(fixture.spec())
        .arg("--root")
        .arg(fixture.root())
        .args(["--since", "baseline", "--executor-report"])
        .arg(report_path)
        .arg("--json")
        .env("MMCG_QUERY_BUDGET_MS", "60000")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["verdict"], "drift");
    assert_eq!(report["findings"][0]["kind"], "vacuous_test_claim");
}

#[cfg(unix)]
#[test]
fn test_scan_audit_symlinked_scope_is_unknown_unix() {
    use std::os::unix::fs::symlink;
    let fixture = scan_fixture();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(
        outside.path().join("Cargo.toml"),
        b"[package]\nname = \"outside\"\n",
    )
    .unwrap();
    symlink(outside.path(), fixture.root().join(".mastermind/link")).unwrap();
    for cmd in [
        "cargo test --manifest-path .mastermind/link/Cargo.toml",
        "pytest .mastermind/link",
    ] {
        let report = fixture.audit(Some(&executor(vec![passed(cmd)])));
        assert_eq!(report.verdict, Verdict::Held, "{cmd}: {report:?}");
    }
    symlink(
        outside.path(),
        fixture.root().join(".mastermind/scan/tests"),
    )
    .unwrap();
    let report = fixture.audit(Some(&executor(vec![passed(SCAN_COMMAND)])));
    assert_eq!(report.verdict, Verdict::Held, "{report:?}");
}
