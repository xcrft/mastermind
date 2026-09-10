use mmcg::{
    audit_spec::{self, Bundle, Finding, Report, Verdict},
    executor_report::{self, ExecutorReport, ReportStatus},
    indexer::Indexer,
    run_task, spec,
    store::Store,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPEC: &str = ".mastermind/tasks/001-report/spec.md";

fn canonical(status: &str) -> Value {
    let complete = status == "complete";
    json!({
        "schema_version": 1, "spec": SPEC, "status": status,
        "phases": [{"id": "1", "status": if complete { "done" } else { "pending" }}],
        "files_modified": ["service.py"], "claims": [],
        "defects": if complete { json!([]) } else { json!([{
            "kind": "implementation_blocked", "phase": "external-label",
            "details": "Missing dependency", "remediation_hint": "Restore the dependency"
        }]) },
        "verifications": []
    })
}

fn parsed(value: &Value) -> ExecutorReport {
    executor_report::parse_str(&value.to_string()).unwrap()
}

fn fenced(value: &Value) -> String {
    format!("Report follows.\n<!-- mastermind:report-begin -->\n```yaml\n{value}\n```\n<!-- mastermind:report-end -->\n")
}

#[test]
fn executor_report_retains_metadata_excerpts_and_empty_canonical_identity() {
    for status in ["complete", "partial", "failed"] {
        let mut value = canonical(status);
        value["verifications"] = json!([{
            "cmd": "echo checked", "result": if status == "failed" { "fail" } else { "pass" },
            "output_excerpt": "Captured output", "observed": {"exit_code": 0}
        }]);
        let report = parsed(&value);
        let metadata = serde_json::to_value(report.canonical.as_ref().unwrap()).unwrap();
        for field in [
            "schema_version",
            "spec",
            "status",
            "phases",
            "files_modified",
            "defects",
        ] {
            assert_eq!(metadata[field], value[field], "{status}: {field}");
        }
        assert_eq!(
            report.verify[0].output_excerpt.as_deref(),
            Some("Captured output")
        );
        assert_eq!(
            report.verify[0].observed.as_ref().unwrap().exit_code,
            Some(0)
        );
        let roundtrip: ExecutorReport =
            serde_json::from_value(serde_json::to_value(&report).unwrap()).unwrap();
        assert_eq!(roundtrip, report);
        assert!(!report.is_empty());
    }
    let mut empty = canonical("complete");
    empty["phases"] = json!([]);
    empty["files_modified"] = json!([]);
    let report = parsed(&empty);
    assert!(report.claims.is_empty() && report.verify.is_empty());
    assert!(!report.is_empty());
    assert_eq!(report.canonical.unwrap().status, ReportStatus::Complete);
}

#[test]
fn executor_report_canonical_schema_rejects_nulls_types_and_empty_claim_fields() {
    let mut value = canonical("partial");
    value["claims"] = json!([
        {"kind": "function_added", "symbol": "fresh", "file": "service.py", "signature": "def fresh()"},
        {"kind": "integration", "from": "keep", "from_file": "service.py", "to": "fresh", "to_file": "service.py", "relation": "calls"}
    ]);
    value["verifications"] = json!([{"cmd": "echo checked", "result": "pass", "output_excerpt": "", "observed": {"exit_code": 0, "tests_run": 1}}]);
    parsed(&value);
    for pointer in [
        "/schema_version",
        "/spec",
        "/status",
        "/phases",
        "/files_modified",
        "/claims",
        "/defects",
        "/verifications",
        "/phases/0/id",
        "/files_modified/0",
        "/claims/0/file",
        "/claims/0/signature",
        "/claims/1/from_file",
        "/claims/1/to_file",
        "/claims/1/relation",
        "/verifications/0/output_excerpt",
        "/verifications/0/observed",
        "/verifications/0/observed/exit_code",
        "/verifications/0/observed/tests_run",
    ] {
        let mut invalid = value.clone();
        *invalid.pointer_mut(pointer).unwrap() = Value::Null;
        assert!(
            executor_report::parse_str(&invalid.to_string()).is_err(),
            "null {pointer}"
        );
    }
    for pointer in [
        "/spec",
        "/phases/0/id",
        "/files_modified/0",
        "/claims/0/symbol",
        "/claims/0/file",
        "/claims/0/signature",
        "/claims/1/from",
        "/claims/1/to",
        "/claims/1/from_file",
        "/claims/1/to_file",
        "/claims/1/relation",
        "/defects/0/details",
        "/verifications/0/cmd",
    ] {
        for replacement in [json!(true), json!(7), json!("")] {
            let mut invalid = value.clone();
            *invalid.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                executor_report::parse_str(&invalid.to_string()).is_err(),
                "scalar {pointer}: {invalid}"
            );
        }
    }
    let yaml = serde_norway::to_string(&value).unwrap();
    assert!(yaml.contains("exit_code: 0"));
    for number in [".nan", ".inf", "-.inf", "true", "1.5"] {
        let invalid = yaml.replace("exit_code: 0", &format!("exit_code: {number}"));
        assert!(executor_report::parse_str(&invalid).is_err(), "{number}");
    }
    for suffix in ["schema_version: 1\n", "7: ignored\n"] {
        assert!(executor_report::parse_str(&format!("{yaml}{suffix}")).is_err());
    }
    assert!(yaml.contains("status: partial"));
    assert!(executor_report::parse_str(&yaml.replacen(
        "status: partial",
        "status: !custom partial",
        1
    ))
    .is_err());
}

#[test]
fn executor_report_sentinels_select_one_complete_span_without_reading_literal_tokens() {
    let mut value = canonical("complete");
    value["verifications"] = json!([{"cmd": "echo mastermind:report-end", "result": "pass", "output_excerpt": "literal ``` and <!-- mastermind:executor-begin -->"}]);
    let expected = parsed(&value);
    for text in [
        fenced(&value),
        fenced(&value).replace('\n', "\r\n"),
        serde_norway::to_string(&value).unwrap(),
    ] {
        assert_eq!(
            executor_report::parse_canonical_str(&text).unwrap(),
            expected
        );
    }
    let excerpt = "<!-- mastermind:report-begin -->\n<!-- mastermind:report-end -->\n<!-- mastermind:executor-begin -->\n<!-- mastermind:executor-end -->";
    let mut literal = canonical("complete");
    literal["verifications"] =
        json!([{"cmd": "echo checked", "result": "pass", "output_excerpt": excerpt}]);
    let yaml = serde_norway::to_string(&canonical("complete")).unwrap();
    assert!(yaml.contains("verifications: []"));
    let yaml = yaml.replace(
        "verifications: []",
        &format!(
            "verifications:\n  - cmd: echo checked\n    result: pass\n    output_excerpt: |-\n{}",
            excerpt
                .lines()
                .map(|line| format!("      {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    );
    let markdown = format!("Report follows.\n<!-- mastermind:report-begin -->\n```yaml\n{yaml}\n```\n<!-- mastermind:report-end -->\n");
    for text in [yaml, markdown] {
        assert_eq!(
            executor_report::parse_canonical_str(&text).unwrap(),
            parsed(&literal)
        );
    }
    let good = fenced(&canonical("complete"));
    let failed = fenced(&canonical("failed"));
    let legacy = "<!-- mastermind:executor-begin -->\n```yaml\nclaims: []\n```\n<!-- mastermind:executor-end -->\n";
    for invalid in [
        format!("{good}{failed}"), format!("{good}{legacy}"),
        format!("<!-- mastermind:report-end -->\n{good}"),
        good.replace("<!-- mastermind:report-begin -->", "<!-- mastermind:report-begin -->\n<!-- mastermind:report-begin -->"),
        good.replace("<!-- mastermind:report-begin -->", "<!-- mastermind:report-begin -->\n<!-- mastermind:report-end -->"),
        "<!-- mastermind:report-begin -->\n<!-- mastermind:report-end -->\n```yaml\nclaims: []\n```\n".into(),
        good.replace("\n```\n<!-- mastermind:report-end -->", "\n<!-- mastermind:report-end -->"),
    ] {
        assert!(executor_report::parse_str(&invalid).is_err(), "accepted ambiguous report: {invalid}");
    }
}

#[test]
fn executor_report_required_parser_rejects_legacy_spoofing_and_preserves_input_bounds() {
    for text in [
        "{}",
        "claims: []\nverify: []\n",
        "verify:\n - cmd: check\n   observed: null\n",
    ] {
        assert!(executor_report::parse_str(text)
            .unwrap()
            .canonical
            .is_none());
        assert!(executor_report::parse_canonical_str(text)
            .unwrap_err()
            .contains("legacy"));
    }
    let mut spoof = serde_json::to_value(parsed(&canonical("complete"))).unwrap();
    assert!(executor_report::parse_str(&spoof.to_string()).is_err());
    spoof["canonical"] = Value::Null;
    assert!(executor_report::parse_str(&spoof.to_string()).is_err());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("report.md");
    let oversized = "x".repeat(1024 * 1024 + 1);
    std::fs::write(&path, &oversized).unwrap();
    assert!(executor_report::parse_str(&oversized)
        .unwrap_err()
        .contains("1048576-byte limit"));
    assert!(executor_report::parse_canonical_file(&path)
        .unwrap_err()
        .contains("1048576-byte limit"));
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(executor_report::parse_canonical_file(&path)
        .unwrap_err()
        .contains("not UTF-8"));
}

struct Fixture {
    store: Store,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
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
        let path = root.join(SPEC);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "---\nmode: lite\ntouches:\n  - file: service.py\n---\n# Update service\n## Goals\nApply the service change.\n").unwrap();
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
    fn report_path(&self) -> PathBuf {
        self.spec().with_file_name("executor-report.md")
    }

    fn change(&mut self) {
        std::fs::write(
            self.root().join("service.py"),
            "def keep():\n    return 2\ndef fresh(): pass\n",
        )
        .unwrap();
        Indexer::new(self.root())
            .index_all(&mut self.store, false)
            .unwrap();
    }

    fn write_report(&self, value: &Value) {
        std::fs::write(self.report_path(), value.to_string()).unwrap();
    }

    fn audit(&self, executor: &ExecutorReport) -> Report {
        audit_spec::run_with_report(
            &spec::parse_file(&self.spec()).unwrap(),
            &self.store,
            self.root(),
            "baseline",
            Some(executor),
        )
        .unwrap()
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

fn rejected(report: &Report, reason: &str) -> bool {
    report.findings.iter().any(|finding| matches!(finding, Finding::ExecutorReportRejected { reason: actual } if actual == reason))
}

#[test]
fn executor_report_completion_and_task_identity_reach_the_shared_audit() {
    let mut fixture = Fixture::new();
    fixture.change();
    for status in ["complete", "partial", "failed"] {
        let executor = parsed(&canonical(status));
        let report = fixture.audit(&executor);
        assert_eq!(
            report.verdict,
            if status == "complete" {
                Verdict::Held
            } else {
                Verdict::Broken
            }
        );
        assert_eq!(report.executor_report.as_ref(), Some(&executor));
        assert!(report.claim_checks.as_ref().unwrap().is_empty());
        if status != "complete" {
            assert!(rejected(&report, &format!("status_{status}")));
            let bundle = Bundle::from_report(&report, None);
            assert_eq!(bundle.verdict, "broken");
            assert!(bundle.human_summary.contains("1 errors, 0 warnings"));
        }
    }
    for path in [
        SPEC.to_string(),
        format!("./{SPEC}"),
        fixture.spec().display().to_string(),
        fixture.spec().canonicalize().unwrap().display().to_string(),
    ] {
        let mut value = canonical("complete");
        value["spec"] = json!(path);
        assert_eq!(
            fixture.audit(&parsed(&value)).verdict,
            Verdict::Held,
            "{path}"
        );
    }
    for (path, reason) in [
        (".mastermind/tasks/002-other/spec.md", "task_mismatch"),
        ("../spec.md", "task_path_invalid"),
    ] {
        let mut value = canonical("complete");
        value["spec"] = json!(path);
        let report = fixture.audit(&parsed(&value));
        assert_eq!(report.verdict, Verdict::Broken);
        assert!(rejected(&report, reason), "{report:?}");
    }
    let mut fabricated = parsed(&canonical("partial"));
    fabricated.canonical.as_mut().unwrap().status = ReportStatus::Complete;
    assert!(rejected(
        &fixture.audit(&fabricated),
        "completion_inconsistent"
    ));
}

#[test]
fn executor_report_bundle_binds_metadata_verification_and_current_disk_input() {
    let mut fixture = Fixture::new();
    fixture.change();
    git(fixture.root(), &["add", "service.py"]);
    git(fixture.root(), &["commit", "-q", "-m", "change"]);
    let mut value = canonical("complete");
    value["claims"] = json!([{"kind": "function_added", "symbol": "fresh", "file": "service.py"}]);
    value["verifications"] = json!([{"cmd": "echo checked", "result": "pass", "output_excerpt": "original", "observed": {"exit_code": 0}}]);
    fixture.write_report(&value);
    let executor = parsed(&value);
    let report = fixture.audit(&executor);
    assert_eq!(report.verdict, Verdict::Held);
    let bundle = Bundle::from_report(&report, None);
    assert_eq!(bundle.commands, ["echo checked"]);
    assert_eq!(bundle.verified_claims.len(), 1);
    assert!(bundle
        .into_manifest(fixture.root())
        .unwrap_err()
        .to_string()
        .contains("requires an input path"));
    for (pointer, replacement) in [
        ("/canonical/spec", json!("other.md")),
        ("/canonical/status", json!("partial")),
        ("/canonical/phases", json!([])),
        ("/canonical/files_modified", json!([])),
        (
            "/canonical/defects",
            json!([{"kind": "blocked", "phase": "1", "details": "blocked", "remediation_hint": "retry"}]),
        ),
        ("/verify/0/observed/exit_code", json!(9)),
        ("/verify/0/output_excerpt", json!("substituted")),
        ("/canonical", Value::Null),
    ] {
        let mut substituted = serde_json::to_value(&executor).unwrap();
        *substituted.pointer_mut(pointer).unwrap() = replacement;
        let substituted: ExecutorReport = serde_json::from_value(substituted).unwrap();
        assert_eq!(substituted.claims, executor.claims);
        let bundle = Bundle::from_report_full(&report, Some(&substituted), None, None, None);
        assert_eq!(bundle.verdict, "broken", "{pointer}");
        assert!(bundle.verified_claims.is_empty());
        assert!(bundle.discrepancies.iter().any(|finding| matches!(finding, Finding::ExecutorReportRejected { reason } if reason == "report_checks_mismatch")));
    }
    let path = fixture.report_path();
    let valid = Bundle::from_report(&report, path.to_str());
    let manifest = valid.into_manifest(fixture.root()).unwrap();
    assert_eq!(manifest.verdict, "held");
    assert!(manifest.inputs.executor_report_present);
    let pending = Bundle::from_report(&report, path.to_str());
    value["verifications"][0]["output_excerpt"] = json!("disk substitution");
    fixture.write_report(&value);
    assert!(pending
        .into_manifest(fixture.root())
        .unwrap_err()
        .to_string()
        .contains("executor report changed"));
}

#[test]
fn executor_report_cli_and_required_ci_emit_broken_completion_evidence() {
    for mode in ["complete", "partial", "failed", "wrong_task"] {
        let mut fixture = Fixture::new();
        fixture.change();
        git(fixture.root(), &["add", "service.py"]);
        git(fixture.root(), &["commit", "-q", "-m", "change"]);
        let mut value = canonical(if mode == "wrong_task" {
            "complete"
        } else {
            mode
        });
        if mode == "wrong_task" {
            value["spec"] = json!(".mastermind/tasks/other/spec.md");
        }
        fixture.write_report(&value);
        let path = fixture.report_path();
        let output = fixture.command(&[
            "audit-spec",
            SPEC,
            "--since",
            "baseline",
            "--executor-report",
            path.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(
            output.status.success(),
            mode == "complete",
            "{mode}: {output:?}"
        );
        let audit: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(audit["executor_report"]["canonical"]["spec"], value["spec"]);
        assert_eq!(
            audit["executor_report"]["canonical"]["status"],
            value["status"]
        );
        let ci = fixture.command(&[
            "ci",
            "--since",
            "baseline",
            "--require-executor-report",
            "--bundle-dir",
            ".mastermind/output",
        ]);
        assert_eq!(ci.status.success(), mode == "complete", "{mode}: {ci:?}");
        let envelope_path = fixture
            .root()
            .join(".mastermind/output/001-report.bundle.json");
        assert!(envelope_path.is_file(), "{mode}: {ci:?}");
        let envelope: Value =
            serde_json::from_slice(&std::fs::read(envelope_path).unwrap()).unwrap();
        assert_eq!(envelope["schema_version"], 3);
        assert_eq!(
            envelope["manifest"]["verdict"],
            if mode == "complete" { "held" } else { "broken" }
        );
        if mode != "complete" {
            assert!(envelope["manifest"]["discrepancies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|finding| finding["kind"] == "executor_report_rejected"));
        }
    }
}

#[test]
fn executor_report_legacy_compatibility_does_not_bypass_canonical_ci_requirements() {
    let mut fixture = Fixture::new();
    fixture.change();
    git(fixture.root(), &["add", "service.py"]);
    git(fixture.root(), &["commit", "-q", "-m", "change"]);
    std::fs::write(fixture.report_path(), "claims: []\nverify: []\n").unwrap();
    let path = fixture.report_path();
    let audit = fixture.command(&[
        "audit-spec",
        SPEC,
        "--since",
        "baseline",
        "--executor-report",
        path.to_str().unwrap(),
        "--json",
    ]);
    assert!(audit.status.success(), "{audit:?}");
    let ci = fixture.command(&["ci", "--since", "baseline"]);
    assert!(ci.status.success(), "{ci:?}");
    for extra in [
        vec!["--require-executor-report"],
        vec!["--bundle-dir", ".mastermind/output"],
    ] {
        let mut args = vec!["ci", "--since", "baseline"];
        args.extend(extra);
        let rejected = fixture.command(&args);
        assert!(!rejected.status.success(), "{rejected:?}");
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("legacy report supplied"));
        assert!(!fixture
            .root()
            .join(".mastermind/output/001-report.bundle.json")
            .exists());
    }
}

#[test]
fn executor_report_controller_rejects_incomplete_wrong_and_legacy_reports_then_recovers() {
    for mode in ["partial", "failed", "wrong_task", "legacy", "ambiguous"] {
        let mut fixture = Fixture::new();
        assert_eq!(fixture.run_task(true), run_task::Outcome::PreReady);
        fixture.change();
        let body = match mode {
            "legacy" => "claims: []\nverify: []\n".into(),
            "ambiguous" => format!(
                "{}{}",
                fenced(&canonical("complete")),
                fenced(&canonical("failed"))
            ),
            "wrong_task" => {
                let mut value = canonical("complete");
                value["spec"] = json!(".mastermind/tasks/other/spec.md");
                value.to_string()
            }
            status => canonical(status).to_string(),
        };
        std::fs::write(fixture.report_path(), body).unwrap();
        assert_eq!(
            fixture.run_task(false),
            run_task::Outcome::PostBroken,
            "{mode}"
        );
        let state_path = run_task::state_file_path(fixture.root(), &fixture.spec());
        let failed = run_task::load_state(&state_path).unwrap().unwrap();
        assert_eq!(failed.next_step.as_deref(), Some("planner_review"));
        assert!(failed.blocking_reason.is_some());
        assert!(failed.held_snapshot_sha256.is_none() && failed.history_snapshot_sha256.is_none());
        assert!(!run_task::release_file_path(fixture.root(), &fixture.spec()).exists());
        if matches!(mode, "legacy" | "ambiguous") {
            assert_eq!(failed.last_artifact.as_deref(), Some("executor-report.md"));
        }
        fixture.write_report(&canonical("complete"));
        assert_eq!(
            fixture.run_task(false),
            run_task::Outcome::PostHeld,
            "recovery: {mode}"
        );
        let recovered = run_task::load_state(&state_path).unwrap().unwrap();
        assert_eq!(recovered.baseline_ref, failed.baseline_ref);
        assert_eq!(recovered.iteration, failed.iteration);
        assert_eq!(recovered.next_step.as_deref(), Some("review_history"));
        assert!(recovered.history_snapshot_sha256.is_some());
        fixture.write_report(&canonical("partial"));
        assert_eq!(fixture.run_task(false), run_task::Outcome::PostBroken);
        let repeated = run_task::load_state(&state_path).unwrap().unwrap();
        assert_eq!(repeated.next_step.as_deref(), Some("planner_review"));
        assert!(repeated.history_snapshot_sha256.is_none());
    }
}

#[test]
fn executor_report_cli_uses_the_actual_spec_path_when_cwd_differs_from_root() {
    let mut fixture = Fixture::new();
    fixture.change();
    fixture.write_report(&canonical("complete"));
    let outside = tempfile::tempdir().unwrap();
    let external_spec = outside.path().join(SPEC);
    std::fs::create_dir_all(external_spec.parent().unwrap()).unwrap();
    std::fs::copy(fixture.spec(), &external_spec).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mmcg"))
        .current_dir(outside.path())
        .arg("--index")
        .arg(fixture.root().join("graph.db"))
        .args(["audit-spec", SPEC, "--root"])
        .arg(fixture.root())
        .args(["--since", "baseline", "--executor-report"])
        .arg(fixture.report_path())
        .arg("--json")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["verdict"], "broken");
    assert!(Path::new(report["spec"].as_str().unwrap()).is_absolute());
    assert!(report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["kind"] == "executor_report_rejected"
            && finding["reason"] == "task_path_invalid"));
}

#[cfg(unix)]
#[test]
fn executor_report_audit_rejects_task_sources_through_symlink_ancestors() {
    let mut fixture = Fixture::new();
    fixture.change();
    let outside = tempfile::tempdir().unwrap();
    std::fs::copy(fixture.spec(), outside.path().join("spec.md")).unwrap();
    let link = fixture.root().join(".mastermind/linked");
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    let mut value = canonical("complete");
    value["spec"] = json!(".mastermind/linked/spec.md");
    let checked = spec::parse_file(&link.join("spec.md")).unwrap();
    let report = audit_spec::run_with_report(
        &checked,
        &fixture.store,
        fixture.root(),
        "baseline",
        Some(&parsed(&value)),
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Broken);
    assert!(rejected(&report, "task_source_unavailable"));
}
