use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
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

fn adapt(
    root: &Path,
    index: &str,
    format: &str,
    input: &str,
    output: &Path,
    dataset: &str,
) -> Value {
    let output_text = output.to_string_lossy().into_owned();
    let result = run(
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
            "--root",
            ".",
        ],
    );
    assert_success(&result);
    serde_json::from_slice(&fs::read(output).unwrap()).unwrap()
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
    let output = run(
        root.path(),
        &[
            "--index",
            &index,
            "facts",
            "adapt",
            "--format",
            "sarif",
            "--input",
            "findings.sarif",
            "--output",
            &manifest_text,
            "--producer",
            "semgrep",
            "--producer-version",
            "1.0.0",
            "--dataset",
            "pr-security",
            "--root",
            ".",
        ],
    );
    assert_success(&output);
    let adapted: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
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
    let output_path = root.path().join("must-not-exist.json");
    let output_text = output_path.to_string_lossy().into_owned();
    let result = run(
        root.path(),
        &[
            "--index",
            &index,
            "facts",
            "adapt",
            "--format",
            "sarif",
            "--input",
            "unmapped.sarif",
            "--output",
            &output_text,
            "--producer",
            "semgrep",
            "--producer-version",
            "1.0.0",
            "--dataset",
            "unmapped",
            "--root",
            ".",
        ],
    );
    assert!(!result.status.success());
    assert!(!output_path.exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("not all mapped"));
}

#[test]
fn adapter_output_cannot_overwrite_its_input_through_a_path_alias() {
    let (root, index) = fixture();
    let input = root.path().join("findings.sarif");
    let before = fs::read(&input).unwrap();
    let result = run(
        root.path(),
        &[
            "--index",
            &index,
            "facts",
            "adapt",
            "--format",
            "sarif",
            "--input",
            "findings.sarif",
            "--output",
            "./findings.sarif",
            "--producer",
            "semgrep",
            "--producer-version",
            "1.0.0",
            "--dataset",
            "overwrite",
            "--root",
            ".",
        ],
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
