use mmcg::{audit_spec, indexer::Indexer, run_task, spec, store::Store};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const METHODS: &str = "class A:\n    def run(self):\n        return 1\n\nclass B:\n    def run(self):\n        return 2\n";
const REMOVED_METHOD: &str =
    "class A:\n    def run(self):\n        return 1\n\nclass B:\n    pass\n";

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
        for (path, source) in files {
            let path = root.join(path);
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

    fn source(&mut self, path: &str, source: &str) {
        std::fs::write(self.root().join(path), source).unwrap();
        Indexer::new(self.root())
            .index_all(&mut self.store, false)
            .unwrap();
    }

    fn write_spec(&self, touches: &str, acknowledgements: &str, snapshot: &str) -> PathBuf {
        let path = self.root().join(".mastermind/tasks/001-removal/spec.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!(
            "---\nmode: lite\ntouches:\n{touches}breaking_changes:\n  removed_symbols:\n{acknowledgements}---\n# Remove obsolete code\n## Goals\nRemove the explicitly selected declaration.\n{snapshot}"
        )).unwrap();
        path
    }

    fn method_spec(&self, acknowledgement: &str, yaml: bool) -> PathBuf {
        let (touches, snapshot) = if yaml {
            ("  - file: service.py\n    language: python\n    symbols:\n      - name: B.run\n        signature: 'def run(self)'\n",
             "")
        } else {
            (
                "  - file: service.py\n    language: python\n    symbols: [B.run]\n",
                "## Pre-edit symbol snapshot\n- `B.run` — signature `def run(self)`\n",
            )
        };
        self.write_spec(touches, acknowledgement, snapshot)
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

    fn audit(&self, path: &Path, success: bool) -> Value {
        let output = self.command(&[
            "audit-spec",
            path.to_str().unwrap(),
            "--since",
            "baseline",
            "--json",
        ]);
        assert_eq!(output.status.success(), success, "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
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

fn has_finding(report: &Value, kind: &str, symbol: &str) -> bool {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["kind"] == kind && finding["symbol"] == symbol)
}

#[test]
fn removal_acknowledgements_keep_file_language_and_unchanged_baseline_scope() {
    for (ack, expected, reason) in [
        (
            "    - {name: run, file: a.py, language: python}\n",
            "broken",
            None,
        ),
        (
            "    - {name: run, file: ./b.py, language: python}\n",
            "held",
            None,
        ),
        (
            "    - {name: run, file: b.py, language: rust}\n",
            "broken",
            Some("missing"),
        ),
        (
            "    - {name: run, file: b.py, signature: 'def run(flag=True)'}\n",
            "broken",
            Some("signature_mismatch"),
        ),
        ("    - run\n", "broken", Some("ambiguous")),
        (
            "    - {name: run, file: ../b.py}\n",
            "broken",
            Some("invalid_file_scope"),
        ),
        (
            "    - {name: B..run, file: b.py}\n",
            "broken",
            Some("invalid_qualification"),
        ),
    ] {
        let mut fixture =
            Fixture::new(&[("a.py", "def run(): pass\n"), ("b.py", "def run(): pass\n")]);
        fixture.source("b.py", "# removed\n");
        let path = fixture.write_spec("  - file: b.py\n", ack, "");
        let report = fixture.audit(&path, expected == "held");
        assert_eq!(report["verdict"], expected, "{report}");
        if expected == "broken" {
            assert!(
                has_finding(&report, "removed_symbol_not_acknowledged", "run"),
                "{report}"
            );
        }
        if let Some(reason) = reason {
            assert!(
                report["findings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|finding| {
                        finding["kind"] == "removal_acknowledgement_unresolved"
                            && finding["reason"] == reason
                    }),
                "{report}"
            );
        }
    }
    let mut fixture = Fixture::new(&[
        ("a.py", "def keep(): pass\n"),
        ("b.py", "def old_api(): pass\n"),
    ]);
    fixture.source("b.py", "# removed\n");
    let path = fixture.write_spec("  - file: b.py\n", "    - old_api\n", "");
    assert_eq!(fixture.audit(&path, true)["verdict"], "held");
}

#[test]
fn same_line_removal_acknowledgements_keep_exact_parent_identity() {
    for (name, success) in [("A.Run", false), ("B.Run", true)] {
        let mut fixture = Fixture::new(&[(
            "service.cs",
            "class A { void Run() {} } class B { void Run() {} }\n",
        )]);
        fixture.source("service.cs", "class A { void Run() {} } class B {}\n");
        let path = fixture.write_spec(
            "  - file: service.cs\n",
            &format!("    - {{name: {name}, file: service.cs, language: csharp}}\n"),
            "",
        );
        let report = fixture.audit(&path, success);
        assert_eq!(report["verdict"], if success { "held" } else { "broken" });
        if !success {
            assert!(has_finding(
                &report,
                "removed_symbol_not_acknowledged",
                "B.Run"
            ));
        }
        let removed = &report["symbol_diff"]["removed"];
        assert_eq!(removed.as_array().unwrap().len(), 1, "{report}");
        assert_eq!(removed[0]["line"], 1);
        assert_eq!(removed[0]["name"], "Run");
        let fields: std::collections::BTreeSet<_> = removed[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fields,
            ["file", "name", "kind", "line", "signature"]
                .into_iter()
                .collect()
        );
    }
}

#[test]
fn exact_removal_acknowledgements_accept_only_their_recorded_snapshot() {
    for yaml in [false, true] {
        for (name, success) in [("A.run", false), ("B.run", true)] {
            let mut fixture = Fixture::new(&[("service.py", METHODS)]);
            let path = fixture.method_spec(
                &format!("    - {{name: {name}, file: service.py, language: python}}\n"),
                yaml,
            );
            fixture.source("service.py", REMOVED_METHOD);
            let report = fixture.audit(&path, success);
            assert_eq!(report["verdict"], if success { "held" } else { "broken" });
            if !success {
                assert!(has_finding(&report, "snapshot_symbol_gone", "B.run"));
                assert!(has_finding(
                    &report,
                    "removed_symbol_not_acknowledged",
                    "B.run"
                ));
            } else {
                assert!(report["findings"].as_array().unwrap().is_empty());
                let ci = fixture.command(&["ci", "--since", "baseline"]);
                assert!(ci.status.success(), "{ci:?}");

                let stale = std::fs::read_to_string(&path)
                    .unwrap()
                    .replace("def run(self)", "def run(self, stale)");
                std::fs::write(&path, stale).unwrap();
                let report = fixture.audit(&path, false);
                assert!(has_finding(&report, "snapshot_unresolved", "B.run"));
                let ci = fixture.command(&["ci", "--since", "baseline"]);
                assert!(!ci.status.success(), "{ci:?}");
            }
        }
    }
    // Removing a parent does not implicitly authorize removal of its children.
    for (children, success) in [
        ("", false),
        ("    - {name: B.run, file: service.py}\n", true),
    ] {
        let mut fixture = Fixture::new(&[("service.py", METHODS)]);
        let path = fixture.method_spec(
            &format!("    - {{name: B, file: service.py}}\n{children}"),
            false,
        );
        fixture.source(
            "service.py",
            "class A:\n    def run(self):\n        return 1\n",
        );
        let report = fixture.audit(&path, success);
        assert_eq!(report["verdict"], if success { "held" } else { "broken" });
        if !success {
            assert!(has_finding(
                &report,
                "removed_symbol_not_acknowledged",
                "B.run"
            ));
        }
    }
}

#[test]
fn removal_signatures_cannot_select_method_overloads_but_can_identify_impl_blocks() {
    let mut fixture = Fixture::new(&[(
        "service.cs",
        "class C { void Run() {} void Run(int value) {} }\n",
    )]);
    let signature = fixture.store.search_symbols("Run", None, None).unwrap()[0]
        .signature
        .clone()
        .unwrap();
    let ack = format!(
        "    - name: C.Run\n      file: service.cs\n      signature: '{}'\n",
        signature.replace('\'', "''")
    );
    let path = fixture.write_spec("  - file: service.cs\n", &ack, "");
    fixture.source("service.cs", "class C { void Run() {} }\n");
    let report = fixture.audit(&path, false);
    assert!(
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                finding["kind"] == "removal_acknowledgement_unresolved"
                    && finding["reason"] == "ambiguous"
                    && finding["matches"] == 2
            }),
        "{report}"
    );

    for (signature, success) in [
        (None, false),
        (Some("impl Marker for B"), true),
        (Some("impl Other for B"), false),
    ] {
        let mut fixture = Fixture::new(&[(
            "service.rs",
            "struct B;\ntrait Marker {}\nimpl Marker for B {}\n",
        )]);
        let signature = signature
            .map(|signature| format!("      signature: '{signature}'\n"))
            .unwrap_or_default();
        let ack = format!("    - name: B\n      file: service.rs\n{signature}");
        let path = fixture.write_spec("  - file: service.rs\n", &ack, "");
        fixture.source("service.rs", "struct B;\ntrait Marker {}\n");
        assert_eq!(
            fixture.audit(&path, success)["verdict"],
            if success { "held" } else { "broken" }
        );
    }
}

#[test]
fn unresolved_removal_acknowledgements_fail_ci_bundle_and_controller() {
    for (ack, expected, failure) in [
        (
            "    - {name: B.run, file: service.py}\n",
            run_task::Outcome::PostHeld,
            None,
        ),
        (
            "    - {name: B.run, file: service.py, language: rust}\n",
            run_task::Outcome::PostBroken,
            Some("removal_acknowledgement_unresolved"),
        ),
    ] {
        let mut fixture = Fixture::new(&[("service.py", METHODS)]);
        let path = fixture.method_spec(ack, false);
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
        fixture.source("service.py", REMOVED_METHOD);
        std::fs::write(path.with_file_name("executor-report.md"),
            "schema_version: 1\nspec: .mastermind/tasks/001-removal/spec.md\nstatus: complete\nphases:\n  - id: '1'\n    status: done\nfiles_modified: [service.py]\nclaims: []\ndefects: []\nverifications: []\n"
        ).unwrap();
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
        if let Some(failure) = failure {
            assert!(!run_task::release_file_path(fixture.root(), &path).exists());
            assert_eq!(state.next_step.as_deref(), Some("planner_review"));
            assert!(state.held_snapshot_sha256.is_none());
            assert!(std::fs::read_to_string(path.with_file_name("audit.md"))
                .unwrap()
                .contains(failure));
            let parsed = spec::parse_file(&path).unwrap();
            let report =
                audit_spec::run(&parsed, &fixture.store, fixture.root(), "baseline").unwrap();
            let bundle = audit_spec::Bundle::from_report(&report, None);
            assert_eq!(bundle.verdict, "broken");
            assert!(bundle.discrepancies.iter().any(|finding| matches!(
                finding,
                audit_spec::Finding::RemovalAcknowledgementUnresolved { .. }
            )));
            assert!(
                bundle.human_summary.contains("errors"),
                "{}",
                bundle.human_summary
            );
            let ci = fixture.command(&["ci", "--since", "baseline"]);
            assert!(!ci.status.success(), "{ci:?}");
            assert!(
                String::from_utf8_lossy(&ci.stderr).contains(failure),
                "{ci:?}"
            );
        } else {
            assert_eq!(state.next_step.as_deref(), Some("review_history"));
            assert!(state.history_snapshot_sha256.is_some());
            assert!(
                state.held_snapshot_sha256.is_none(),
                "lite mode has no strict-policy snapshot"
            );
            assert!(run_task::release_file_path(fixture.root(), &path).exists());
        }
    }
}

#[test]
fn removal_audit_reads_literal_baseline_objects_despite_replacement_refs() {
    let mut fixture = Fixture::new(&[("service.py", "def old_api(): pass\n")]);
    let baseline = git(fixture.root(), &["rev-parse", "baseline"]);
    fixture.source("service.py", "# removed\n");
    git(fixture.root(), &["add", "service.py"]);
    git(fixture.root(), &["commit", "-q", "-m", "remove"]);
    let tree = git(fixture.root(), &["rev-parse", "HEAD^{tree}"]);
    let replacement = git(fixture.root(), &["commit-tree", &tree, "-m", "replacement"]);
    git(fixture.root(), &["replace", &baseline, &replacement]);
    let path = fixture.write_spec("  - file: service.py\n", "    - old_api\n", "");
    let report = fixture.audit(&path, true);
    assert_eq!(report["verdict"], "held", "{report}");
    assert_eq!(
        report["symbol_diff"]["removed"].as_array().unwrap().len(),
        1
    );
    assert_eq!(report["symbol_diff"]["removed"][0]["name"], "old_api");
}

#[test]
fn malformed_unchanged_baseline_cannot_certify_an_unqualified_removal() {
    for (path, source) in [
        ("malformed.py", "def other(\n"),
        ("Broken.vue", "<script>function other(</script>"),
        (
            "Multiple.vue",
            "<script>function other() {}</script><script setup>function run() {}</script>",
        ),
        ("External.vue", "<script SRC=\"./missing.js\"></script>"),
        (
            "Unsupported.vue",
            "<script lang=\"coffee\">run = -> 1</script>",
        ),
    ] {
        for (ack, success) in [
            ("    - run\n", false),
            ("    - {name: run, file: service.py}\n", true),
        ] {
            let mut fixture = Fixture::new(&[("service.py", "def run(): pass\n"), (path, source)]);
            fixture.source("service.py", "# removed\n");
            let path = fixture.write_spec("  - file: service.py\n", ack, "");
            let report = fixture.audit(&path, success);
            if !success {
                assert!(
                    report["findings"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|finding| {
                            finding["kind"] == "removal_acknowledgement_unresolved"
                                && finding["reason"] == "baseline_parse_failed"
                        }),
                    "{report}"
                );
            }
        }
    }
    let mut fixture = Fixture::new(&[
        ("service.py", "def run(): pass\n"),
        (
            "Valid.vue",
            "<script setup lang=\"ts\">function other(value: number) { return value; }</script>",
        ),
    ]);
    fixture.source("service.py", "# removed\n");
    let path = fixture.write_spec("  - file: service.py\n", "    - run\n", "");
    assert_eq!(fixture.audit(&path, true)["verdict"], "held");
}

#[test]
fn ci_accepts_acknowledged_file_deletion_without_weakening_preflight() {
    for file in ["service.py", "./service.py"] {
        let mut fixture = Fixture::new(&[
            ("service.py", "def old_api(): pass\n"),
            ("keep.py", "def keep(): return 1\n"),
        ]);
        let path = fixture.write_spec(
        &format!("  - file: {file}\n    symbols:\n      - name: old_api\n        signature: 'def old_api()'\n"),
        "    - {name: old_api, file: service.py}\n",
        &format!("## Phase 1: remove\n**File:** `{file}`\nFIND:\n```python\ndef old_api(): pass\n```\n"),
    );
        let verify = fixture.command(&[
            "verify-spec",
            path.to_str().unwrap(),
            "--require-index",
            "--json",
        ]);
        assert!(verify.status.success(), "{verify:?}");
        std::fs::remove_file(fixture.root().join("service.py")).unwrap();
        Indexer::new(fixture.root())
            .index_all(&mut fixture.store, false)
            .unwrap();
        assert_eq!(fixture.audit(&path, true)["verdict"], "held");
        let parsed = spec::parse_file(&path).unwrap();
        let report = audit_spec::run(&parsed, &fixture.store, fixture.root(), "baseline").unwrap();
        let bundle = audit_spec::Bundle::from_report_full(
            &report,
            None,
            Some(&parsed),
            None,
            Some(fixture.root()),
        );
        assert_eq!(bundle.spec_files, vec!["service.py"]);
        let verify = fixture.command(&[
            "verify-spec",
            path.to_str().unwrap(),
            "--require-index",
            "--json",
        ]);
        assert!(!verify.status.success(), "{verify:?}");
        let preflight: Value = serde_json::from_slice(&verify.stdout).unwrap();
        assert!(preflight["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["kind"] == "find_block_unavailable"));
        let ci = fixture.command(&["ci", "--since", "baseline"]);
        assert!(ci.status.success(), "{ci:?}");

        // A surviving declaration keeps its ordinary scoped existence check.
        fixture.source("keep.py", "def keep(): return 2\n");
        let surviving_touch = std::fs::read_to_string(&path).unwrap().replace(
            "breaking_changes:\n",
            "  - file: ./keep.py\n    symbols: [keep]\nbreaking_changes:\n",
        );
        std::fs::write(&path, surviving_touch).unwrap();
        assert_eq!(fixture.audit(&path, true)["verdict"], "held");
        let ci = fixture.command(&["ci", "--since", "baseline"]);
        assert!(ci.status.success(), "{ci:?}");

        // A required document never gains the code-removal exception, even through an alias.
        let required_doc = std::fs::read_to_string(&path).unwrap().replacen(
            "touches:\n",
            "expected_docs: [./service.py]\ntouches:\n",
            1,
        );
        std::fs::write(&path, required_doc).unwrap();
        let ci = fixture.command(&["ci", "--since", "baseline"]);
        assert!(!ci.status.success(), "{ci:?}");

        // An unrelated missing file remains a real error in the combined CI gate.
        let path = fixture.write_spec(
            "  - file: service.py\n    symbols: [old_api]\n  - file: absent.py\n",
            "    - {name: old_api, file: service.py}\n",
            "",
        );
        let ci = fixture.command(&["ci", "--since", "baseline"]);
        assert!(!ci.status.success(), "{ci:?}");
        assert!(
            String::from_utf8_lossy(&ci.stderr).contains("absent.py"),
            "{ci:?}"
        );
        assert!(path.is_file());
    }
}

#[test]
fn vue_quoted_typescript_contract_refreshes_declarations() {
    for language in ["ts", "\"ts\"", "'ts'"] {
        let source = format!(
            "<script setup lang={language}>\ninterface Model {{ run(): number; }}\nfunction keep(model: Model) {{ return model.run(); }}\n</script>\n"
        );
        let mut fixture = Fixture::new(&[("Typed.vue", &source)]);
        let symbols = fixture.store.symbols_in_file("Typed.vue").unwrap();
        assert!(symbols
            .iter()
            .any(|symbol| symbol.name == "Model" && symbol.kind == "interface"));
        assert!(symbols.iter().any(|symbol| symbol.name == "keep"
            && symbol.signature.as_deref() == Some("function keep(model: Model)")));
        assert_eq!(
            Indexer::new(fixture.root())
                .index_all(&mut fixture.store, false)
                .unwrap()
                .files_indexed,
            0
        );
        fixture
            .store
            .set_meta(
                mmcg::indexer::EXTRACTOR_CONTRACT_META_KEY,
                "mmcg-extractors-v9",
            )
            .unwrap();
        assert!(!fixture.store.extractor_contract_current().unwrap());
        let stats = Indexer::new(fixture.root())
            .index_all(&mut fixture.store, false)
            .unwrap();
        assert!(stats.extractor_contract_rebuilt);
        assert_eq!(stats.files_indexed, 1);
        assert!(fixture.store.extractor_contract_current().unwrap());
    }
}
