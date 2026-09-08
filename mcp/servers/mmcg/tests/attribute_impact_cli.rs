use mmcg::{indexer::Indexer, store::Store};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::{Command, Output};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mmcg"))
        .current_dir(root)
        .arg("--index")
        .arg(root.join("graph.db"))
        .args(args)
        .env("MMCG_QUERY_BUDGET_MS", "60000")
        .output()
        .unwrap()
}

fn success(root: &Path, args: &[&str]) -> Value {
    let output = run(root, args);
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cli_rejects_old_attribute_signatures_until_index_refresh() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    git(root, &["init", "-q", "--initial-branch=main"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join(".gitignore"), "graph.db*\n").unwrap();
    std::fs::create_dir(root.join("src")).unwrap();
    let baseline = "fn checks_value() {}\n#[cfg(feature = \"fixed\")]\nfn stable() {}\n";
    std::fs::write(root.join("src/lib.rs"), baseline).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "baseline"]);
    git(root, &["tag", "baseline"]);
    let current = format!("#[test]\n{baseline}");
    std::fs::write(root.join("src/lib.rs"), &current).unwrap();
    git(root, &["add", "src/lib.rs"]);
    git(root, &["commit", "-q", "-m", "test attribute"]);

    let db = root.join("graph.db");
    let mut store = Store::open(&db).unwrap();
    let indexer = Indexer::new(root);
    indexer.index_all(&mut store, false).unwrap();
    assert_eq!(
        store.file_content_sha256("src/lib.rs").unwrap(),
        Some(mmcg::hex::encode(&Sha256::digest(current.as_bytes())))
    );
    let connection = rusqlite::Connection::open(&db).unwrap();
    for name in ["checks_value", "stable"] {
        connection
            .execute(
                "UPDATE symbols SET signature = ?1 WHERE name = ?2",
                rusqlite::params![format!("fn {name}()"), name],
            )
            .unwrap();
    }
    drop(connection);
    store
        .set_meta(
            mmcg::indexer::EXTRACTOR_CONTRACT_META_KEY,
            "mmcg-extractors-v6",
        )
        .unwrap();

    let impact_args = ["impact", "--since", "baseline", "--format", "json"];
    let diff_args = ["query", "symbols-changed-since", "baseline"];
    for args in [impact_args.as_slice(), diff_args.as_slice()] {
        let output = run(root, args);
        assert!(!output.status.success(), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("index_stale"),
            "{output:?}"
        );
    }
    assert!(!store.extractor_contract_current().unwrap());
    assert_eq!(
        store
            .symbols_in_file("src/lib.rs")
            .unwrap()
            .iter()
            .find(|s| s.name == "checks_value")
            .unwrap()
            .signature
            .as_deref(),
        Some("fn checks_value()")
    );

    let stats = indexer.index_all(&mut store, false).unwrap();
    assert!(stats.extractor_contract_rebuilt);
    assert_eq!(stats.files_indexed, 1);
    let impact = success(root, &impact_args);
    assert_eq!(
        impact["changes"]["symbols"]["items"],
        json!([{
            "file": "src/lib.rs", "name": "checks_value", "kind": "function",
            "line": 2, "change": "signature_changed"
        }])
    );
    assert_eq!(impact["tests"]["total"], 1);
    assert_eq!(impact["tests"]["items"][0]["classification"], "direct");
    assert_eq!(impact["tests"]["items"][0]["minimum_depth"], 0);
    let diff = success(root, &diff_args);
    assert!(diff["added"].as_array().unwrap().is_empty());
    assert!(diff["removed"].as_array().unwrap().is_empty());
    assert_eq!(
        diff["signature_changed"],
        json!([{
            "file": "src/lib.rs", "name": "checks_value", "kind": "function",
            "old_signature": "fn checks_value()", "new_signature": "#[test] fn checks_value()",
            "new_line": 2
        }])
    );
    drop(store);
}

#[test]
fn cli_rejects_old_python_decorator_signatures_until_index_refresh() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    git(root, &["init", "-q", "--initial-branch=main"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join(".gitignore"), "graph.db*\n").unwrap();
    std::fs::create_dir(root.join("tests")).unwrap();
    let baseline = "import pytest\n@pytest.mark.parametrize(\"value\", [1])\ndef test_value(value):\n    assert value > 0\n";
    std::fs::write(root.join("tests/test_values.py"), baseline).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "baseline"]);
    git(root, &["tag", "baseline"]);
    let current = baseline.replace("[1]", "[2]");
    std::fs::write(root.join("tests/test_values.py"), &current).unwrap();
    git(root, &["add", "tests/test_values.py"]);
    git(root, &["commit", "-q", "-m", "decorator arguments"]);

    let db = root.join("graph.db");
    let mut store = Store::open(&db).unwrap();
    let indexer = Indexer::new(root);
    indexer.index_all(&mut store, false).unwrap();
    assert_eq!(
        store.file_content_sha256("tests/test_values.py").unwrap(),
        Some(mmcg::hex::encode(&Sha256::digest(current.as_bytes())))
    );
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute(
            "UPDATE symbols SET signature = 'def test_value(value)' WHERE name = 'test_value'",
            [],
        )
        .unwrap();
    drop(connection);
    store
        .set_meta(
            mmcg::indexer::EXTRACTOR_CONTRACT_META_KEY,
            "mmcg-extractors-v7",
        )
        .unwrap();
    let impact_args = ["impact", "--since", "baseline", "--format", "json"];
    let diff_args = ["query", "symbols-changed-since", "baseline"];
    for args in [impact_args.as_slice(), diff_args.as_slice()] {
        let output = run(root, args);
        assert!(!output.status.success(), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("index_stale"),
            "{output:?}"
        );
    }
    assert!(!store.extractor_contract_current().unwrap());
    let stats = indexer.index_all(&mut store, false).unwrap();
    assert!(stats.extractor_contract_rebuilt);
    assert_eq!(stats.files_indexed, 1);
    let impact = success(root, &impact_args);
    assert_eq!(
        impact["changes"]["symbols"]["items"],
        json!([{
            "file": "tests/test_values.py", "name": "test_value", "kind": "function",
            "line": 3, "change": "signature_changed"
        }])
    );
    assert_eq!(impact["tests"]["total"], 1);
    assert_eq!(impact["tests"]["items"][0]["classification"], "direct");
    assert_eq!(impact["tests"]["items"][0]["minimum_depth"], 0);
    assert!(impact["tests"]["items"][0]["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "changed_test_symbol" && e["seed"]["change"] == "signature_changed"));
    let diff = success(root, &diff_args);
    assert!(diff["added"].as_array().unwrap().is_empty());
    assert!(diff["removed"].as_array().unwrap().is_empty());
    assert_eq!(
        diff["signature_changed"],
        json!([{
            "file": "tests/test_values.py", "name": "test_value", "kind": "function",
            "old_signature": "@pytest.mark.parametrize(\"value\", [1]) def test_value(value)",
            "new_signature": "@pytest.mark.parametrize(\"value\", [2]) def test_value(value)",
            "new_line": 3
        }])
    );
    drop(store);
}
