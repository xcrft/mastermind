use mmcg::{
    audit_spec::{self, Bundle, ClaimStatus, Report},
    executor_report::{Claim, ExecutorReport},
    indexer::Indexer,
    run_task, spec,
    store::Store,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    store: Store,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
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
        for (file, source) in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, source).unwrap();
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "baseline"]);
        git(root, &["tag", "baseline"]);
        let mut store = Store::open(root.join("graph.db")).unwrap();
        Indexer::new(root).index_all(&mut store, false).unwrap();
        Self { store, directory }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn source(&mut self, file: &str, source: &str) {
        std::fs::write(self.root().join(file), source).unwrap();
        Indexer::new(self.root())
            .index_all(&mut self.store, false)
            .unwrap();
    }

    fn spec(&self, files: &[&str]) -> PathBuf {
        let path = self.root().join(".mastermind/tasks/001-claims/spec.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let touches = if files.is_empty() {
            "touches: []\n".into()
        } else {
            format!(
                "touches:\n{}",
                files
                    .iter()
                    .map(|file| format!("  - file: {file}\n"))
                    .collect::<String>()
            )
        };
        std::fs::write(&path, format!("---\nmode: lite\n{touches}---\n# Update service\n## Goals\nApply the declared service change.\n")).unwrap();
        path
    }

    fn audit(&self, path: &Path, executor: Option<&ExecutorReport>) -> Report {
        audit_spec::run_with_report(
            &spec::parse_file(path).unwrap(),
            &self.store,
            self.root(),
            "baseline",
            executor,
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

    fn executor_file(&self, path: &Path, claims: &[Claim]) -> PathBuf {
        let report = path.with_file_name("executor-report.md");
        let mut claims = serde_json::to_value(claims).unwrap();
        for claim in claims.as_array_mut().unwrap() {
            claim
                .as_object_mut()
                .unwrap()
                .retain(|_, value| !value.is_null());
        }
        std::fs::write(
            &report,
            serde_json::to_vec(&json!({
                "schema_version": 1,
                "spec": ".mastermind/tasks/001-claims/spec.md",
                "status": "complete",
                "phases": [{"id": "1", "status": "done"}],
                "files_modified": ["service.py"],
                "claims": claims,
                "defects": [],
                "verifications": []
            }))
            .unwrap(),
        )
        .unwrap();
        report
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn added(symbol: &str, file: &str, signature: Option<&str>) -> Claim {
    Claim::FunctionAdded {
        symbol: symbol.into(),
        file: Some(file.into()),
        signature: signature.map(str::to_string),
    }
}

fn calls(from: &str, from_file: &str, to: &str, to_file: &str) -> Claim {
    Claim::Integration {
        from: from.into(),
        from_file: Some(from_file.into()),
        to: to.into(),
        to_file: Some(to_file.into()),
        relation: Some("calls".into()),
    }
}

fn executor(claims: Vec<Claim>) -> ExecutorReport {
    ExecutorReport {
        claims,
        verify: vec![],
    }
}

fn checks(report: &Report) -> Value {
    serde_json::to_value(report.claim_checks.as_ref().unwrap()).unwrap()
}

#[test]
fn executor_additions_require_diff_identity_before_signature_matching() {
    let mut fixture = Fixture::new(&[("service.py", "def keep():\n    return 1\n")]);
    let path = fixture.spec(&["service.py"]);
    for source in [
        "def keep():\n    return 2\n",
        "def keep(flag=False):\n    return 2\n",
    ] {
        fixture.source("service.py", source);
        let report = fixture.audit(
            &path,
            Some(&executor(vec![added("keep", "./service.py", None)])),
        );
        assert_eq!(
            checks(&report)[0]["finding"]["kind"],
            "claimed_symbol_not_added"
        );
        assert_eq!(checks(&report)[0]["status"], "failed");
    }
    fixture.source("service.py", "def keep():\n    return 1\n\nclass A:\n    def run(self): pass\n\nclass B:\n    def run(self, flag=False): pass\n");
    let report = fixture.audit(
        &path,
        Some(&executor(vec![
            added("B.run", "service.py", Some("def run(self, flag=False)")),
            added("A.run", "service.py", Some("def run(self, flag=False)")),
            added("run", "service.py", Some("def run(self, flag=False)")),
        ])),
    );
    let checked = checks(&report);
    assert_eq!(checked[0]["status"], "verified");
    assert_eq!(checked[0]["evidence"]["kind"], "added_declaration");
    assert_eq!(
        checked[0]["evidence"]["baseline_oid"],
        git(fixture.root(), &["rev-parse", "baseline"])
    );
    assert_eq!(checked[1]["finding"]["kind"], "claimed_signature_mismatch");
    assert_eq!(checked[2]["finding"]["reason"], "ambiguous");
    assert_eq!(checked[2]["finding"]["matches"], 2);
}

#[test]
fn executor_same_line_declarations_cannot_borrow_an_addition() {
    let mut fixture = Fixture::new(&[("Service.cs", "class A { void Run() {} }\n")]);
    fixture.source(
        "Service.cs",
        "class A { void Run() {} } class B { void Run() {} }\n",
    );
    let path = fixture.spec(&["Service.cs"]);
    let report = fixture.audit(
        &path,
        Some(&executor(vec![
            added("A.Run", "Service.cs", None),
            added("B.Run", "Service.cs", None),
        ])),
    );
    let checked = checks(&report);
    assert_eq!(checked[0]["finding"]["kind"], "claimed_symbol_not_added");
    assert_eq!(checked[1]["status"], "verified");
    assert_eq!(checked[1]["evidence"]["line"], 1);
    let projected = serde_json::to_value(report.symbol_diff.unwrap()).unwrap();
    for declaration in projected["added"].as_array().unwrap() {
        assert_eq!(
            declaration
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["file", "kind", "line", "name", "signature"]
        );
    }
}

#[test]
fn executor_ambiguous_overloads_and_duplicate_impl_parents_are_unresolved() {
    for (file, before, after, symbol) in [
        (
            "Service.cs",
            "class C { void Run() {} void Run(int n) {} }\n",
            "class C { void Run(string n) {} }\n",
            "C.Run",
        ),
        (
            "service.rs",
            "struct B; impl B { fn old() {} } impl B { fn keep() {} }\n",
            "struct B; impl B { fn keep() {} }\n",
            "B::keep",
        ),
        (
            "service.rs",
            "struct B; impl B { fn keep() {} }\n",
            "struct B; impl B { fn fresh() {} } impl B { fn keep() {} }\n",
            "B::keep",
        ),
    ] {
        let mut fixture = Fixture::new(&[(file, before)]);
        fixture.source(file, after);
        let report = fixture.audit(
            &fixture.spec(&[file]),
            Some(&executor(vec![added(symbol, file, None)])),
        );
        assert_eq!(checks(&report)[0]["status"], "unresolved", "{report:?}");
        assert_eq!(
            checks(&report)[0]["finding"]["reason"],
            "addition_ambiguous",
            "{report:?}"
        );
    }
}

#[test]
fn executor_recovered_baseline_and_current_syntax_cannot_prove_additions() {
    for (before, after, reason) in [
        (
            "def keep(): pass\n\ndef broken(\n",
            "def keep(): pass\n\ndef added(): pass\n",
            "baseline_parse_failed",
        ),
        (
            "def keep(): pass\n",
            "def keep(): pass\n\ndef added(): pass\n\ndef broken(\n",
            "current_parse_failed",
        ),
    ] {
        let mut fixture = Fixture::new(&[("service.py", before)]);
        fixture.source("service.py", after);
        let report = fixture.audit(
            &fixture.spec(&["service.py"]),
            Some(&executor(vec![added("added", "service.py", None)])),
        );
        assert_eq!(checks(&report)[0]["status"], "unresolved", "{report:?}");
        assert_eq!(
            checks(&report)[0]["finding"]["reason"],
            reason,
            "{report:?}"
        );
    }
}

#[test]
fn executor_call_candidates_preserve_target_kind_and_global_ambiguity() {
    for (body, duplicate, expected) in [
        ("client.send();", false, "missing_call_edge"),
        ("send();", false, "verified"),
        ("send();", true, "call_target_ambiguous"),
    ] {
        let source = format!("fn caller() {{ {body} }}\n");
        let mut files = vec![
            ("caller.rs", source.as_str()),
            ("target.rs", "fn send() {}\n"),
        ];
        if duplicate {
            files.push(("other.rs", "fn send() {}\n"));
        }
        let fixture = Fixture::new(&files);
        let report = fixture.audit(
            &fixture.spec(&[]),
            Some(&executor(vec![calls(
                "caller",
                "caller.rs",
                "send",
                "target.rs",
            )])),
        );
        let checked = checks(&report);
        if expected == "verified" {
            assert_eq!(checked[0]["status"], "verified", "{report:?}");
            assert_eq!(checked[0]["evidence"]["kind"], "compatible_call_candidate");
            assert_eq!(
                checked[0]["evidence"]["precision"]["target_resolution"],
                "name_based_candidates"
            );
            assert_eq!(checked[0]["evidence"]["target_kind"], "function");
        } else if expected == "missing_call_edge" {
            assert_eq!(checked[0]["finding"]["kind"], expected, "{report:?}");
        } else {
            assert_eq!(checked[0]["finding"]["reason"], expected, "{report:?}");
            assert!(checked[0]["finding"]["matches"].is_null());
        }
    }
}

#[test]
fn executor_scoped_calls_do_not_infer_aliases_or_confuse_types_with_methods() {
    for (body, target, expected, basis) in [
        ("B::run();", "A::run", "call_target_scope_unresolved", None),
        ("B::run();", "B::run", "verified", Some("name")),
        (
            "Alias::run();",
            "B::run",
            "call_target_scope_unresolved",
            None,
        ),
        ("B::new();", "B", "verified", Some("type")),
    ] {
        let source = format!("struct A; struct B; impl A {{ fn run() {{}} }} impl B {{ fn run() {{}} fn new() -> Self {{ B }} }} fn caller() {{ {body} }}\n");
        let fixture = Fixture::new(&[("service.rs", &source)]);
        let report = fixture.audit(
            &fixture.spec(&[]),
            Some(&executor(vec![calls(
                "caller",
                "service.rs",
                target,
                "service.rs",
            )])),
        );
        let checked = checks(&report);
        if let Some(basis) = basis {
            assert_eq!(checked[0]["status"], expected, "{report:?}");
            assert_eq!(checked[0]["evidence"]["match_basis"], basis);
        } else {
            assert_eq!(checked[0]["status"], "unresolved", "{report:?}");
            assert_eq!(checked[0]["finding"]["reason"], expected, "{report:?}");
        }
    }
}

#[test]
fn executor_relation_callback_and_cross_language_claims_do_not_become_calls() {
    let fixture = Fixture::new(&[
        (
            "service.rs",
            "fn send() {} fn caller() { let callback = send; }\n",
        ),
        ("target.py", "def send(): pass\n"),
    ]);
    let mut unsupported = calls("caller", "service.rs", "send", "service.rs");
    if let Claim::Integration { relation, .. } = &mut unsupported {
        *relation = Some("references".into());
    }
    let report = fixture.audit(
        &fixture.spec(&[]),
        Some(&executor(vec![
            unsupported,
            calls("caller", "service.rs", "send", "service.rs"),
            calls("caller", "service.rs", "send", "target.py"),
        ])),
    );
    let checked = checks(&report);
    assert_eq!(checked[0]["finding"]["reason"], "relation_unsupported");
    assert_eq!(checked[1]["finding"]["kind"], "missing_call_edge");
    assert_eq!(
        checked[2]["finding"]["reason"],
        "cross_language_binding_unavailable"
    );
}

#[test]
fn executor_claim_outcomes_keep_file_signature_and_order_identity_in_bundles() {
    let mut fixture = Fixture::new(&[
        ("good.py", "def caller(): send()\n"),
        ("bad.py", "def caller(): pass\n"),
        ("target.py", "def send(): pass\n"),
    ]);
    let path = fixture.spec(&[]);
    let good = calls("caller", "good.py", "send", "target.py");
    let bad = calls("caller", "bad.py", "send", "target.py");
    for claims in [
        vec![good.clone(), bad.clone()],
        vec![bad.clone(), good.clone()],
    ] {
        let er = executor(claims);
        let report = fixture.audit(&path, Some(&er));
        let bundle = Bundle::from_report_full(&report, Some(&er), None, None, None);
        assert_eq!(bundle.verified_claims.len(), 1, "{bundle:?}");
        assert_eq!(bundle.failed_claims.len(), 1);
        assert!(bundle.verified_claims[0].contains("caller@good.py"));
        assert!(bundle.failed_claims[0].contains("caller@bad.py"));
        let derived = Bundle::from_report(&report, None);
        assert_eq!(derived.verified_claims, bundle.verified_claims);
        let reordered = executor(er.claims.iter().cloned().rev().collect());
        let mismatch = Bundle::from_report_full(&report, Some(&reordered), None, None, None);
        assert!(mismatch.verified_claims.is_empty());
        assert_eq!(mismatch.verdict, "broken");
        assert!(mismatch.discrepancies.iter().any(|finding| matches!(finding, audit_spec::Finding::ExecutorClaimUnresolved { claim_index: None, reason, .. } if reason == "claim_checks_mismatch")));
    }
    fixture.source(
        "target.py",
        "def send(): pass\ndef fresh(flag=False): pass\n",
    );
    let path = fixture.spec(&["target.py"]);
    let er = executor(vec![
        added("fresh", "target.py", Some("def fresh(flag=False)")),
        added("fresh", "target.py", Some("def fresh()")),
    ]);
    let report = fixture.audit(&path, Some(&er));
    let bundle = Bundle::from_report_full(&report, Some(&er), None, None, None);
    assert_eq!(
        bundle.verified_claims,
        ["claim[1] function_added:fresh@target.py"]
    );
    assert_eq!(
        bundle.failed_claims,
        ["claim[2] function_added:fresh@target.py"]
    );
    let mut substituted = er.clone();
    substituted.claims[1] = substituted.claims[0].clone();
    assert!(
        Bundle::from_report_full(&report, Some(&substituted), None, None, None)
            .verified_claims
            .is_empty()
    );
}

#[test]
fn executor_unchecked_and_empty_reports_have_distinct_bundle_states() {
    let fixture = Fixture::new(&[("service.py", "def caller(): send()\ndef send(): pass\n")]);
    let path = fixture.spec(&[]);
    let er = executor(vec![calls("caller", "service.py", "send", "service.py")]);
    let unchecked = fixture.audit(&path, None);
    assert!(unchecked.claim_checks.is_none());
    assert!(serde_json::to_value(&unchecked)
        .unwrap()
        .get("claim_checks")
        .is_none());
    for er in [er, ExecutorReport::default()] {
        let bundle = Bundle::from_report_full(&unchecked, Some(&er), None, None, None);
        assert_eq!(bundle.verdict, "broken");
        assert!(bundle.verified_claims.is_empty());
        assert!(bundle.discrepancies.iter().any(|finding| matches!(finding, audit_spec::Finding::ExecutorClaimUnresolved { reason, .. } if reason == "claims_not_evaluated")));
    }
    let empty = ExecutorReport::default();
    let checked = fixture.audit(&path, Some(&empty));
    assert_eq!(checks(&checked), json!([]));
    assert_eq!(
        Bundle::from_report_full(&checked, Some(&empty), None, None, None).verdict,
        "held"
    );
}

#[test]
fn executor_public_audit_rejects_stale_additions_calls_and_unindexed_competitors() {
    for mode in ["addition", "call", "competitor"] {
        let mut fixture = Fixture::new(&[("service.py", "def caller(): pass\ndef send(): pass\n")]);
        fixture.source(
            "service.py",
            "def caller(): send()\ndef send(): pass\ndef added(): pass\n",
        );
        let (claim, touches) = match mode {
            "addition" => {
                std::fs::write(
                    fixture.root().join("service.py"),
                    "def caller():\n    return 2\ndef send(): pass\n",
                )
                .unwrap();
                (added("added", "service.py", None), vec!["service.py"])
            }
            "call" => {
                std::fs::write(
                    fixture.root().join("service.py"),
                    "def caller(): pass\ndef send(): pass\ndef added(): pass\n",
                )
                .unwrap();
                (
                    calls("caller", "service.py", "send", "service.py"),
                    vec!["service.py"],
                )
            }
            _ => {
                std::fs::write(fixture.root().join("unindexed.py"), "def send(): pass\n").unwrap();
                (
                    calls("caller", "service.py", "send", "service.py"),
                    vec!["service.py", "unindexed.py"],
                )
            }
        };
        let path = fixture.spec(&touches);
        let er_path = fixture.executor_file(&path, &[claim]);
        let output = fixture.command(&[
            "audit-spec",
            path.to_str().unwrap(),
            "--since",
            "baseline",
            "--executor-report",
            er_path.to_str().unwrap(),
            "--json",
        ]);
        assert!(!output.status.success(), "{output:?}");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["claim_checks"][0]["status"], "unresolved",
            "{report}"
        );
        let expected = if mode == "competitor" {
            "index_content_unavailable"
        } else {
            "index_source_mismatch"
        };
        assert_eq!(report["claim_checks"][0]["finding"]["reason"], expected);
        assert_eq!(report["verdict"], "broken");
    }
}

#[test]
fn executor_claim_failures_reach_cli_ci_envelopes_and_controller_state() {
    for actual_addition in [false, true] {
        let mut fixture = Fixture::new(&[("service.py", "def keep():\n    return 1\n")]);
        let path = fixture.spec(&["service.py"]);
        let index = fixture.root().join("graph.db");
        assert_eq!(
            run_task::run(
                &path,
                fixture.root(),
                &index,
                run_task::RunOpts {
                    pre_only: true,
                    ..Default::default()
                }
            ),
            run_task::Outcome::PreReady
        );
        let (source, symbol) = if actual_addition {
            ("def keep():\n    return 1\ndef fresh(): pass\n", "fresh")
        } else {
            ("def keep():\n    return 2\n", "keep")
        };
        fixture.source("service.py", source);
        let er = executor(vec![added(symbol, "service.py", None)]);
        let er_path = fixture.executor_file(&path, &er.claims);
        let expected = if actual_addition {
            run_task::Outcome::PostHeld
        } else {
            run_task::Outcome::PostBroken
        };
        assert_eq!(
            run_task::run(
                &path,
                fixture.root(),
                &index,
                run_task::RunOpts {
                    post_only: true,
                    ..Default::default()
                }
            ),
            expected
        );
        let state = run_task::load_state(&run_task::state_file_path(fixture.root(), &path))
            .unwrap()
            .unwrap();
        assert_eq!(
            run_task::release_file_path(fixture.root(), &path).exists(),
            actual_addition
        );
        assert!(
            state.held_snapshot_sha256.is_none(),
            "lite mode has no strict snapshot"
        );
        if actual_addition {
            assert_eq!(state.next_step.as_deref(), Some("review_history"));
            assert!(state.history_snapshot_sha256.is_some());
        } else {
            assert_eq!(state.next_step.as_deref(), Some("planner_review"));
            assert!(std::fs::read_to_string(path.with_file_name("audit.md"))
                .unwrap()
                .contains("claimed_symbol_not_added"));
            let ci = fixture.command(&["ci", "--since", "baseline"]);
            assert!(!ci.status.success(), "{ci:?}");
            assert!(
                String::from_utf8_lossy(&ci.stderr).contains("claimed_symbol_not_added"),
                "{ci:?}"
            );
        }
        git(fixture.root(), &["add", "service.py"]);
        git(fixture.root(), &["commit", "-q", "-m", "change"]);
        let envelope_path = path.with_file_name("claim.bundle.json");
        let output = fixture.command(&[
            "audit-spec",
            path.to_str().unwrap(),
            "--since",
            "baseline",
            "--executor-report",
            er_path.to_str().unwrap(),
            "--bundle",
            envelope_path.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(output.status.success(), actual_addition, "{output:?}");
        assert!(
            envelope_path.is_file(),
            "bundle was not written: {output:?}"
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        let envelope: Value =
            serde_json::from_slice(&std::fs::read(envelope_path).unwrap()).unwrap();
        assert_eq!(envelope["schema_version"], 3);
        assert_eq!(
            envelope["manifest"]["verified_claims"]
                .as_array()
                .unwrap()
                .len(),
            usize::from(actual_addition)
        );
        assert_eq!(
            envelope["manifest"]["failed_claims"]
                .as_array()
                .unwrap()
                .len(),
            usize::from(!actual_addition)
        );
        assert_eq!(
            report["claim_checks"][0]["status"],
            if actual_addition {
                "verified"
            } else {
                "failed"
            }
        );
        assert!(fixture
            .audit(&path, Some(&er))
            .claim_checks
            .unwrap()
            .iter()
            .all(|check| (check.status == ClaimStatus::Verified) == actual_addition));
    }
}
