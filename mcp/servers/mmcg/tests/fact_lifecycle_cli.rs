#[cfg(unix)]
use base64::engine::general_purpose::STANDARD as BASE64;
#[cfg(unix)]
use base64::Engine;
#[cfg(unix)]
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
#[cfg(unix)]
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mmcg"))
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture() -> (TempDir, String) {
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/lib.rs"), "pub fn pay() {}\n").unwrap();
    fs::write(
        root.path().join("src/worker.rs"),
        "pub fn work() { crate::pay(); }\n",
    )
    .unwrap();
    let sarif = json!({
        "version": "2.1.0",
        "runs": [{
            "tool": {"driver": {"name": "Semgrep", "rules": [{"id": "payments.review"}]}},
            "results": [
                {
                    "ruleId": "payments.review",
                    "level": "warning",
                    "message": {"text": "Review payment boundary"},
                    "locations": [{"physicalLocation": {
                        "artifactLocation": {"uri": "src/lib.rs"},
                        "region": {"startLine": 1, "startColumn": 1}
                    }}]
                },
                {
                    "ruleId": "payments.review",
                    "level": "warning",
                    "message": {"text": "Review payment boundary"},
                    "locations": [{"physicalLocation": {
                        "artifactLocation": {"uri": "src/lib.rs"},
                        "region": {"startLine": 1, "startColumn": 1}
                    }}]
                }
            ]
        }]
    });
    fs::write(
        root.path().join("findings.sarif"),
        serde_json::to_vec_pretty(&sarif).unwrap(),
    )
    .unwrap();
    for args in [
        ["init", "-q"].as_slice(),
        ["config", "user.email", "facts@example.test"].as_slice(),
        ["config", "user.name", "Facts Test"].as_slice(),
        ["add", "."].as_slice(),
        ["commit", "-qm", "fixture"].as_slice(),
    ] {
        let output = Command::new("git")
            .current_dir(root.path())
            .args(args)
            .output()
            .unwrap();
        assert_success(&output);
    }
    let index = root.path().join("index.db");
    let index_text = index.to_string_lossy().into_owned();
    let root_text = root.path().to_string_lossy().into_owned();
    let output = run(root.path(), &["--index", &index_text, "index", &root_text]);
    assert_success(&output);
    (root, index_text)
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert_success(&output);
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn head(root: &Path) -> String {
    git(root, &["rev-parse", "HEAD"])
}

fn adapt_at(
    root: &Path,
    index: &str,
    format: &str,
    input: &str,
    output: &Path,
    dataset: &str,
    report_revision: &str,
) -> Output {
    let output_text = output.to_string_lossy().into_owned();
    run(
        root,
        &[
            "--index",
            index,
            "facts",
            "adapt",
            "--format",
            format,
            "--input",
            input,
            "--output",
            &output_text,
            "--producer",
            "fixture-tool",
            "--producer-version",
            "1.0.0",
            "--dataset",
            dataset,
            "--report-revision",
            report_revision,
            "--root",
            ".",
        ],
    )
}

fn adapt(
    root: &Path,
    index: &str,
    format: &str,
    input: &str,
    output: &Path,
    dataset: &str,
) -> Value {
    let result = adapt_at(root, index, format, input, output, dataset, &head(root));
    assert_success(&result);
    serde_json::from_slice(&fs::read(output).unwrap()).unwrap()
}

/// Adapts `input` and asserts that it is refused with `reason` and leaves no
/// manifest behind.
fn assert_adapt_refused(root: &Path, index: &str, format: &str, input: &str, reason: &str) {
    let output = root.join("refused-facts.json");
    let result = adapt_at(root, index, format, input, &output, "refused", &head(root));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success(), "{input} was adapted");
    assert!(stderr.contains(reason), "{input}: {stderr}");
    assert!(!output.exists());
}

#[cfg(unix)]
fn write_keys(root: &Path) -> (String, String, String) {
    use std::os::unix::fs::PermissionsExt;

    let signing = SigningKey::from_bytes(&[7_u8; 32]);
    let public = signing.verifying_key().to_bytes();
    let private_path = root.join("facts.seed");
    let public_path = root.join("facts.pub");
    fs::write(&private_path, format!("{}\n", BASE64.encode([7_u8; 32]))).unwrap();
    fs::write(&public_path, format!("{}\n", BASE64.encode(public))).unwrap();
    fs::set_permissions(&private_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&public_path, fs::Permissions::from_mode(0o644)).unwrap();
    let key_id = format!(
        "sha256:{}",
        Sha256::digest(public)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    (
        private_path.to_string_lossy().into_owned(),
        public_path.to_string_lossy().into_owned(),
        key_id,
    )
}

#[cfg(unix)]
#[test]
fn keygen_cli_creates_a_non_overwriting_signing_pair() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempDir::new().unwrap();
    let private = root.path().join("producer.seed");
    let public = root.path().join("producer.pub");
    let private_text = private.to_string_lossy().into_owned();
    let public_text = public.to_string_lossy().into_owned();
    let generated = run(
        root.path(),
        &[
            "facts",
            "keygen",
            "--private-key",
            &private_text,
            "--public-key",
            &public_text,
        ],
    );
    assert_success(&generated);
    let summary: Value = serde_json::from_slice(&generated.stdout).unwrap();
    assert!(summary["key_id"]
        .as_str()
        .is_some_and(|value| value.starts_with("sha256:") && value.len() == 71));
    assert_eq!(
        fs::metadata(&private).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        BASE64
            .decode(fs::read_to_string(&private).unwrap().trim())
            .unwrap()
            .len(),
        32
    );
    assert_eq!(
        BASE64
            .decode(fs::read_to_string(&public).unwrap().trim())
            .unwrap()
            .len(),
        32
    );

    let private_before = fs::read(&private).unwrap();
    let repeated = run(
        root.path(),
        &[
            "facts",
            "keygen",
            "--private-key",
            &private_text,
            "--public-key",
            &public_text,
        ],
    );
    assert!(!repeated.status.success());
    assert_eq!(fs::read(private).unwrap(), private_before);
}

#[cfg(unix)]
#[test]
fn adapter_signature_import_and_query_are_one_revision_bound_flow() {
    let (root, index) = fixture();
    let manifest = root.path().join("facts.json");
    let signature = root.path().join("facts.sig.json");
    let manifest_text = manifest.to_string_lossy().into_owned();
    let signature_text = signature.to_string_lossy().into_owned();
    let adapted = adapt(
        root.path(),
        &index,
        "sarif",
        "findings.sarif",
        &manifest,
        "pr-security",
    );
    assert_eq!(adapted["api_version"], "mastermind-facts/v1");
    assert_eq!(adapted["provenance"]["kind"], "sarif");
    assert_eq!(adapted["facts"].as_array().unwrap().len(), 1);
    assert_eq!(adapted["facts"][0]["kind"], "annotation");

    let (private_key, public_key, key_id) = write_keys(root.path());
    assert_success(&run(
        root.path(),
        &[
            "facts",
            "sign",
            &manifest_text,
            "--private-key",
            &private_key,
            "--signature",
            &signature_text,
        ],
    ));
    assert_success(&run(
        root.path(),
        &[
            "facts",
            "verify",
            &manifest_text,
            "--signature",
            &signature_text,
            "--public-key",
            &public_key,
            "--trusted-key-id",
            &key_id,
            "--json",
        ],
    ));
    assert_success(&run(
        root.path(),
        &[
            "--index",
            &index,
            "enrich",
            "--facts",
            &manifest_text,
            "--signature",
            &signature_text,
            "--public-key",
            &public_key,
            "--trusted-key-id",
            &key_id,
            "--require-signature",
        ],
    ));
    let query = run(
        root.path(),
        &["--index", &index, "query", "facts", "--top", "10"],
    );
    assert_success(&query);
    let query: Value = serde_json::from_slice(&query.stdout).unwrap();
    assert_eq!(query["annotations"]["returned"], 1);
    assert_eq!(query["sources"]["items"][0]["signature_status"], "verified");
    assert_eq!(query["sources"]["items"][0]["signing_key_id"], key_id);
    assert_eq!(
        query["sources"]["items"][0]["signing_public_key"],
        BASE64.encode(
            SigningKey::from_bytes(&[7_u8; 32])
                .verifying_key()
                .to_bytes()
        )
    );
    assert!(query["sources"]["items"][0]["signature"]
        .as_str()
        .is_some_and(|value| value.len() == 88));
    assert!(query["sources"]["items"][0]["signed_manifest_digest"]
        .as_str()
        .is_some_and(|value| value.starts_with("sha256:") && value.len() == 71));

    let mut tampered = adapted;
    tampered["facts"][0]["message"] = Value::String("tampered".into());
    fs::write(&manifest, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();
    let rejected = run(
        root.path(),
        &[
            "--index",
            &index,
            "enrich",
            "--facts",
            &manifest_text,
            "--signature",
            &signature_text,
            "--public-key",
            &public_key,
            "--trusted-key-id",
            &key_id,
            "--require-signature",
        ],
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("signature"));
}

#[test]
fn adapter_rejects_external_facts_that_cannot_bind_to_the_index() {
    let (root, index) = fixture();
    let sarif = json!({
        "version": "2.1.0",
        "runs": [{
            "tool": {"driver": {"name": "Semgrep"}},
            "results": [{
                "ruleId": "outside.index",
                "message": {"text": "This path is not indexed"},
                "locations": [{"physicalLocation": {
                    "artifactLocation": {"uri": "src/not-indexed.rs"},
                    "region": {"startLine": 1}
                }}]
            }]
        }]
    });
    fs::write(
        root.path().join("unmapped.sarif"),
        serde_json::to_vec_pretty(&sarif).unwrap(),
    )
    .unwrap();
    assert_adapt_refused(
        root.path(),
        &index,
        "sarif",
        "unmapped.sarif",
        "report_entry_outside_index",
    );
}

#[test]
fn adapter_output_cannot_overwrite_its_input_through_a_path_alias() {
    let (root, index) = fixture();
    let input = root.path().join("findings.sarif");
    let before = fs::read(&input).unwrap();
    let result = adapt_at(
        root.path(),
        &index,
        "sarif",
        "findings.sarif",
        Path::new("./findings.sarif"),
        "overwrite",
        &head(root.path()),
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("must not overwrite"));
    assert_eq!(fs::read(input).unwrap(), before);
}

#[test]
fn coverage_junit_and_otel_adapters_emit_importable_revision_bound_facts() {
    let (root, index) = fixture();
    fs::write(
        root.path().join("coverage.info"),
        "TN:\nSF:src/lib.rs\nDA:1,0\nend_of_record\n",
    )
    .unwrap();
    let coverage = adapt(
        root.path(),
        &index,
        "coverage",
        "coverage.info",
        &root.path().join("coverage-facts.json"),
        "coverage",
    );
    assert_eq!(coverage["provenance"]["kind"], "coverage");
    assert_eq!(coverage["facts"][0]["category"], "coverage.file");

    fs::write(
        root.path().join("coverage.xml"),
        r#"<coverage><sources><source>.</source></sources><packages><package><classes><class filename="src/lib.rs"><lines><line number="1" hits="1" /></lines></class></classes></package></packages></coverage>"#,
    )
    .unwrap();
    let cobertura = adapt(
        root.path(),
        &index,
        "coverage",
        "coverage.xml",
        &root.path().join("cobertura-facts.json"),
        "cobertura",
    );
    assert_eq!(cobertura["facts"][0]["severity"], "info");

    fs::write(
        root.path().join("junit.xml"),
        r#"<testsuite name="payments"><testcase name="pays" classname="PayTest" file="src/lib.rs" time="0.1"><failure message="declined">expected approval</failure></testcase></testsuite>"#,
    )
    .unwrap();
    let junit = adapt(
        root.path(),
        &index,
        "junit",
        "junit.xml",
        &root.path().join("junit-facts.json"),
        "junit",
    );
    assert!(junit["facts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|fact| fact["category"] == "test.junit.failure"));

    let traces = json!({
        "resourceSpans": [{"scopeSpans": [{"spans": [
            {
                "traceId": "trace-1",
                "spanId": "span-parent",
                "name": "worker",
                "attributes": [{"key": "code.file.path", "value": {"stringValue": "src/worker.rs"}}]
            },
            {
                "traceId": "trace-1",
                "spanId": "span-child",
                "parentSpanId": "span-parent",
                "name": "pay",
                "attributes": [{"key": "code.file.path", "value": {"stringValue": "src/lib.rs"}}]
            }
        ]}]}]
    });
    fs::write(
        root.path().join("traces.json"),
        serde_json::to_vec_pretty(&traces).unwrap(),
    )
    .unwrap();
    let otel = adapt(
        root.path(),
        &index,
        "otel",
        "traces.json",
        &root.path().join("otel-facts.json"),
        "otel",
    );
    assert!(otel["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "relationships"));
    assert!(otel["facts"].as_array().unwrap().iter().any(|fact| {
        fact["kind"] == "relationship"
            && fact["confidence"] == "observed"
            && fact["relation"] == "runtime_parent_child"
    }));

    for manifest in [
        "coverage-facts.json",
        "cobertura-facts.json",
        "junit-facts.json",
        "otel-facts.json",
    ] {
        let result = run(
            root.path(),
            &["--index", &index, "enrich", "--facts", manifest],
        );
        assert_success(&result);
    }
}

#[test]
fn adapter_binds_only_a_full_report_revision_that_is_the_indexed_head() {
    let (root, index) = fixture();
    let output = root.path().join("facts.json");
    let short = head(root.path())[..12].to_string();
    for (revision, reason) in [
        (
            "0000000000000000000000000000000000000000",
            "does not match the indexed Git HEAD",
        ),
        ("HEAD", "full lowercase commit OID"),
        (short.as_str(), "full lowercase commit OID"),
    ] {
        let result = adapt_at(
            root.path(),
            &index,
            "sarif",
            "findings.sarif",
            &output,
            "revision",
            revision,
        );
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{revision} was accepted");
        assert!(stderr.contains(reason), "{revision}: {stderr}");
        assert!(!output.exists());
    }
}

#[test]
fn sarif_adapter_checks_the_provenance_the_report_declares() {
    let (root, index) = fixture();
    git(
        root.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://example.test/owner/this.git",
        ],
    );
    let head = head(root.path());
    let report = |provenance: Value| {
        json!({
            "version": "2.1.0",
            "runs": [{
                "tool": {"driver": {"name": "Semgrep"}},
                "versionControlProvenance": provenance,
                "results": [{
                    "ruleId": "payments.review",
                    "message": {"text": "Review payment boundary"},
                    "locations": [{"physicalLocation": {
                        "artifactLocation": {"uri": "src/lib.rs"},
                        "region": {"startLine": 1}
                    }}]
                }]
            }]
        })
    };
    let older = "0000000000000000000000000000000000000000";
    for (name, provenance) in [
        (
            "older.sarif",
            json!([{"repositoryUri": "https://example.test/owner/this", "revisionId": older}]),
        ),
        (
            "other-repo.sarif",
            json!([{"repositoryUri": "https://example.test/owner/other", "revisionId": head}]),
        ),
    ] {
        fs::write(
            root.path().join(name),
            serde_json::to_vec(&report(provenance)).unwrap(),
        )
        .unwrap();
        assert_adapt_refused(
            root.path(),
            &index,
            "sarif",
            name,
            "artifact_revision_mismatch",
        );
    }

    // A submodule entry at another revision does not contradict this
    // repository's own entry.
    fs::write(
        root.path().join("with-submodule.sarif"),
        serde_json::to_vec(&report(json!([
            {"repositoryUri": "git@example.test:owner/this.git", "revisionId": head},
            {"repositoryUri": "https://example.test/owner/vendored", "revisionId": older}
        ])))
        .unwrap(),
    )
    .unwrap();
    let adapted = adapt(
        root.path(),
        &index,
        "sarif",
        "with-submodule.sarif",
        &root.path().join("facts.json"),
        "provenance",
    );
    assert_eq!(adapted["repository"]["revision"], head);
}

#[test]
fn adapters_accept_entries_that_carry_no_file_fact_or_repeat_a_line() {
    let (root, index) = fixture();
    let traces = json!({
        "resourceSpans": [{"scopeSpans": [{"spans": [
            {
                "traceId": "trace-1",
                "spanId": "span-parent",
                "name": "pay",
                "attributes": [{"key": "code.file.path", "value": {"stringValue": "src/lib.rs"}}]
            },
            {"traceId": "trace-1", "spanId": "span-http", "parentSpanId": "span-parent", "name": "GET /charge"}
        ]}]}]
    });
    fs::write(
        root.path().join("traces.json"),
        serde_json::to_vec(&traces).unwrap(),
    )
    .unwrap();
    let otel = adapt(
        root.path(),
        &index,
        "otel",
        "traces.json",
        &root.path().join("otel-facts.json"),
        "otel",
    );
    assert_eq!(
        otel["facts"][0]["message"],
        "Observed 1 spans across 1 traces."
    );

    for (name, report) in [
        (
            "methods.xml",
            r#"<coverage><packages><package><classes><class filename="src/lib.rs"><methods><method name="pay"><lines><line number="1" hits="1"/></lines></method></methods><lines><line number="1" hits="1"/></lines></class></classes></package></packages></coverage>"#,
        ),
        (
            "tests.info",
            "TN:unit\nSF:src/lib.rs\nDA:1,1\nend_of_record\nTN:integration\nSF:src/lib.rs\nDA:1,0\nend_of_record\n",
        ),
    ] {
        fs::write(root.path().join(name), report).unwrap();
        let coverage = adapt(
            root.path(),
            &index,
            "coverage",
            name,
            &root.path().join("coverage-facts.json"),
            "coverage",
        );
        assert_eq!(
            coverage["facts"][0]["message"],
            "1 of 1 executable lines covered; 0 uncovered.",
            "{name}"
        );
    }
}

#[test]
fn adapters_name_entries_that_cannot_bind_to_an_indexed_file() {
    let (root, index) = fixture();
    for (name, report, reason) in [
        (
            "no-file.xml",
            r#"<testsuite><testcase name="pays" file="src/lib.rs"/><testcase name="refunds" classname="RefundTest"/></testsuite>"#,
            "report_entry_without_path",
        ),
        (
            "unindexed.xml",
            r#"<testsuite><testcase name="pays" file="src/nope.rs"/></testsuite>"#,
            "report_entry_outside_index",
        ),
    ] {
        fs::write(root.path().join(name), report).unwrap();
        assert_adapt_refused(root.path(), &index, "junit", name, reason);
    }
}

#[test]
fn adapter_fits_multi_line_and_long_report_text_to_the_manifest() {
    let (root, index) = fixture();
    let name = "я".repeat(150);
    fs::write(
        root.path().join("junit.xml"),
        format!(
            "<testsuite><testcase name=\"{name}\" file=\"src/lib.rs\"><failure message=\"declined\">at pay\n\tat charge\r\nat main</failure></testcase></testsuite>"
        ),
    )
    .unwrap();
    let junit = adapt(
        root.path(),
        &index,
        "junit",
        "junit.xml",
        &root.path().join("junit-facts.json"),
        "junit",
    );
    let failure = junit["facts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|fact| fact["category"] == "test.junit.failure")
        .unwrap();
    let title = failure["title"].as_str().unwrap();
    assert!(title.len() <= 256 && title.ends_with('…'), "{title}");
    assert_eq!(
        failure["message"],
        "failed: declined · at pay at charge at main"
    );

    let long = "x".repeat(170);
    let spans = std::iter::once(json!({
        "traceId": "trace-1",
        "spanId": "parent",
        "name": "worker",
        "attributes": [{"key": "code.file.path", "value": {"stringValue": "src/worker.rs"}}]
    }))
    .chain((0..5).map(|index| {
        json!({
            "traceId": "trace-1",
            "spanId": format!("child-{index}"),
            "parentSpanId": "parent",
            "name": format!("{index}{long}\nline"),
            "attributes": [{"key": "code.file.path", "value": {"stringValue": "src/lib.rs"}}]
        })
    }))
    .collect::<Vec<_>>();
    fs::write(
        root.path().join("traces.json"),
        serde_json::to_vec(&json!({"resourceSpans": [{"scopeSpans": [{"spans": spans}]}]}))
            .unwrap(),
    )
    .unwrap();
    let otel = adapt(
        root.path(),
        &index,
        "otel",
        "traces.json",
        &root.path().join("otel-facts.json"),
        "otel",
    );
    let label = otel["facts"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|fact| fact["label"].as_str())
        .unwrap();
    assert!(label.len() <= 512 && label.ends_with('…'), "{label}");
    assert!(!label.chars().any(char::is_control));

    fs::write(
        root.path().join("multi-line.sarif"),
        serde_json::to_vec(&json!({
            "version": "2.1.0",
            "runs": [{
                "tool": {"driver": {"name": "Semgrep"}},
                "results": [{
                    "ruleId": "payments.review",
                    "message": {"text": "Review payment boundary\nCalled from checkout"},
                    "locations": [{"physicalLocation": {
                        "artifactLocation": {"uri": "src/lib.rs"},
                        "region": {"startLine": 1}
                    }}]
                }]
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let sarif = adapt(
        root.path(),
        &index,
        "sarif",
        "multi-line.sarif",
        &root.path().join("sarif-facts.json"),
        "sarif",
    );
    assert_eq!(
        sarif["facts"][0]["message"],
        "Review payment boundary Called from checkout"
    );

    for manifest in ["junit-facts.json", "otel-facts.json", "sarif-facts.json"] {
        assert_success(&run(
            root.path(),
            &["--index", &index, "enrich", "--facts", manifest],
        ));
    }
}

#[test]
fn adapter_refuses_cut_or_empty_reports_and_keeps_the_previous_dataset() {
    let (root, index) = fixture();
    fs::write(
        root.path().join("junit.xml"),
        r#"<testsuite tests="1" failures="1"><testcase name="pays" file="src/lib.rs"><failure message="declined"/></testcase></testsuite>"#,
    )
    .unwrap();
    let manifest = root.path().join("junit-facts.json");
    let manifest_text = manifest.to_string_lossy().into_owned();
    adapt(
        root.path(),
        &index,
        "junit",
        "junit.xml",
        &manifest,
        "refused",
    );
    assert_success(&run(
        root.path(),
        &["--index", &index, "enrich", "--facts", &manifest_text],
    ));

    for (format, name, report, reason) in [
        (
            "junit",
            "cut.xml",
            r#"<testsuites><testsuite tests="3" failures="1"><testcase name="t1" file="src/lib.rs"/>"#,
            "invalid_format",
        ),
        (
            "junit",
            "lost-failure.xml",
            r#"<testsuite tests="1" errors="1"><testcase name="t1" file="src/lib.rs"/></testsuite>"#,
            "junit_totals_mismatch",
        ),
        ("junit", "empty.xml", "<testsuites/>", "produced no facts"),
        (
            "coverage",
            "cut-cobertura.xml",
            r#"<coverage><packages><package><classes><class filename="src/lib.rs"><lines><line number="1" hits="1"/></lines></class>"#,
            "invalid_format",
        ),
        (
            "coverage",
            "cut.info",
            "TN:\nSF:src/lib.rs\nDA:1,1\n",
            "invalid_format",
        ),
    ] {
        fs::write(root.path().join(name), report).unwrap();
        assert_adapt_refused(root.path(), &index, format, name, reason);
    }

    let query = run(root.path(), &["--index", &index, "query", "facts"]);
    assert_success(&query);
    let query: Value = serde_json::from_slice(&query.stdout).unwrap();
    assert_eq!(query["sources"]["items"][0]["dataset"], "refused");
    assert_eq!(query["annotations"]["returned"], 2);

    // A clean SARIF scan has no results and still clears its dataset.
    fs::write(
        root.path().join("clean.sarif"),
        r#"{"version": "2.1.0", "runs": [{"tool": {"driver": {"name": "Semgrep"}}, "results": []}]}"#,
    )
    .unwrap();
    let clean = adapt(
        root.path(),
        &index,
        "sarif",
        "clean.sarif",
        &root.path().join("clean-facts.json"),
        "clean",
    );
    assert_eq!(clean["facts"], json!([]));
}
