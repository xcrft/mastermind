use mmcg::{indexer::Indexer, queries, store::Store};
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Output};

fn run(root: &Path, name: &str, language: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mmcg"));
    command
        .current_dir(root)
        .arg("--index")
        .arg(root.join("graph.db"))
        .args(["query", "explain", name])
        .env("MMCG_QUERY_BUDGET_MS", "60000");
    if let Some(language) = language {
        command.args(["--language", language]);
    }
    command.output().unwrap()
}

fn response(root: &Path, name: &str, language: Option<&str>) -> Value {
    let output = run(root, name, language);
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schema_version"], 2);
    assert_eq!(response["query"], name);
    assert_eq!(response["language"], json!(language));
    assert_eq!(response["edge_kind"], "calls");
    assert_eq!(response["caller_count_scope"], "name_or_type_candidates");
    let notes = response["precision_notes"].as_array().unwrap();
    for note in [
        "heuristic_name_resolution_without_compiler_types",
        "empty_result_does_not_prove_no_dependencies",
        "function_value_and_macro_body_usages_require_edge_kind_references",
        "caller_count_counts_distinct_source_symbols_not_call_sites",
        "caller_language_filter_applies_to_source_symbols",
        "callee_count_counts_distinct_target_name_and_line_pairs",
    ] {
        assert!(notes.iter().any(|value| value == note), "missing {note}");
    }
    response
}

fn index(files: &[(&str, &str)]) -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    for (path, source) in files {
        std::fs::write(directory.path().join(path), source).unwrap();
    }
    let mut store = Store::open(directory.path().join("graph.db")).unwrap();
    Indexer::new(directory.path())
        .index_all(&mut store, true)
        .unwrap();
    (directory, store)
}

#[test]
fn explain_does_not_choose_the_first_same_named_definition() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("graph.db")).unwrap();
    for (file, line, caller_name) in [("modules.rs", 1, "first"), ("modules.rs", 10, "second")] {
        let target = store
            .insert_symbol("process", "function", file, line, line + 5, None, None)
            .unwrap();
        let caller = store
            .insert_symbol(
                caller_name,
                "function",
                "entry.rs",
                line,
                line + 5,
                None,
                None,
            )
            .unwrap();
        for call_line in [line + 1, line + 2] {
            store
                .insert_edge(caller, Some(target), "process", "calls", call_line)
                .unwrap();
        }
        store
            .insert_edge(target, None, "leaf", "calls", line)
            .unwrap();
    }

    let result = response(directory.path(), "process", None);
    assert_eq!(result["match_status"], "ambiguous");
    assert_eq!(result["matched"].as_array().unwrap().len(), 2);
    assert_eq!(result["matched"][0]["line"], 1);
    assert_eq!(result["matched"][1]["line"], 10);
    assert_eq!(result["caller_count"], 2);
    assert_eq!(result.get("callee_count"), Some(&Value::Null));
    assert_eq!(result.get("edge_precision"), Some(&Value::Null));
}

#[test]
fn explain_filters_definitions_and_incoming_sources_by_language() {
    let (directory, _store) = index(&[
        (
            "a.py",
            "def process():\n    python_leaf()\ndef entry():\n    process()\n    process()\n",
        ),
        (
            "z.rs",
            "fn process() {\n    rust_leaf(); rust_leaf();\n    rust_leaf();\n    let callback = referenced;\n}\n\
             fn entry() { process(); process(); }\nfn referenced() {}\n",
        ),
    ]);

    let ambiguous = response(directory.path(), "process", None);
    assert_eq!(ambiguous["match_status"], "ambiguous");
    assert_eq!(ambiguous["caller_count"], 2);
    assert_eq!(ambiguous.get("callee_count"), Some(&Value::Null));
    assert_eq!(ambiguous.get("edge_precision"), Some(&Value::Null));

    for (language, file, outgoing_count, resolution) in [
        ("rust", "z.rs", 2, "syntactic"),
        ("python", "a.py", 1, "heuristic"),
    ] {
        let result = response(directory.path(), "process", Some(language));
        assert_eq!(result["match_status"], "matched");
        assert_eq!(result["matched"].as_array().unwrap().len(), 1);
        assert_eq!(result["matched"][0]["file"], file);
        assert_eq!(result["matched"][0]["language"], language);
        assert_eq!(result["caller_count"], 1);
        assert_eq!(result["callee_count"], outgoing_count);
        assert_eq!(result["edge_precision"]["resolution"], resolution);
        assert_eq!(
            result["limitations"],
            result["edge_precision"]["limitations"]
        );
    }
}

#[test]
fn explain_distinguishes_missing_definitions_from_measured_empty_edges() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("graph.db")).unwrap();
    store
        .insert_symbol("empty", "function", "lib.rs", 1, 1, None, None)
        .unwrap();
    let caller = store
        .insert_symbol("entry", "function", "lib.rs", 3, 5, None, None)
        .unwrap();
    store
        .insert_edge(caller, None, "external", "calls", 4)
        .unwrap();

    for (name, caller_count) in [("external", 1), ("absent", 0)] {
        let missing = response(directory.path(), name, None);
        assert_eq!(missing["match_status"], "not_found");
        assert!(missing["matched"].as_array().unwrap().is_empty());
        assert_eq!(missing["caller_count"], caller_count);
        assert_eq!(missing.get("callee_count"), Some(&Value::Null));
        assert_eq!(missing.get("edge_precision"), Some(&Value::Null));
    }
    let empty = response(directory.path(), "empty", None);
    assert_eq!(empty["match_status"], "matched");
    assert_eq!(empty["caller_count"], 0);
    assert_eq!(empty["callee_count"], 0);
    assert_eq!(empty["edge_precision"]["resolution"], "syntactic");
}

#[test]
fn explain_preserves_outgoing_query_errors_instead_of_reporting_zero() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("graph.db");
    let store = Store::open(&path).unwrap();
    let target = store
        .insert_symbol("target", "function", "lib.rs", 1, 2, None, None)
        .unwrap();
    let caller = store
        .insert_symbol("entry", "function", "lib.rs", 4, 7, None, None)
        .unwrap();
    for line in [5, 6] {
        store
            .insert_edge(caller, Some(target), "target", "calls", line)
            .unwrap();
    }
    store.insert_edge(target, None, "leaf", "calls", 2).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE edges SET line = -1 WHERE from_id = ?1", [target])
        .unwrap();

    assert_eq!(store.search_symbols("target", None, None).unwrap().len(), 1);
    assert_eq!(store.callers_of("target", None, None).unwrap().len(), 1);
    assert!(matches!(
        store.callees_of(target, None),
        Err(rusqlite::Error::IntegralValueOutOfRange(_, -1))
    ));
    assert!(matches!(
        queries::explain(&store, "target", None),
        Err(rusqlite::Error::IntegralValueOutOfRange(_, -1))
    ));
    let output = run(directory.path(), "target", None);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn explain_language_identifiers_round_trip_through_the_filter() {
    let (directory, _store) = index(&[
        ("widget.ts", "export function Widget() { return null; }\n"),
        ("widget.tsx", "export function Widget() { return null; }\n"),
    ]);
    for language in ["typescript", "tsx"] {
        let result = response(directory.path(), "Widget", Some(language));
        assert_eq!(result["match_status"], "matched");
        assert_eq!(result["matched"][0]["language"], language);
        let returned_language = result["matched"][0]["language"].as_str().unwrap();
        let repeated = response(directory.path(), "Widget", Some(returned_language));
        assert_eq!(repeated["matched"], result["matched"]);
    }
}

#[test]
fn explain_keeps_partial_class_declarations_separate() {
    let (directory, store) = index(&[
        ("First.cs", "namespace A { partial class Processor {} }\n"),
        ("Second.cs", "namespace A { partial class Processor {} }\n"),
        ("Third.cs", "namespace B { partial class Processor {} }\n"),
    ]);
    assert_eq!(
        queries::search(&store, "Processor", None, Some("csharp"), true)
            .unwrap()
            .results
            .len(),
        2
    );
    let result = response(directory.path(), "Processor", Some("csharp"));
    assert_eq!(result["match_status"], "ambiguous");
    assert_eq!(result["matched"].as_array().unwrap().len(), 3);
    assert_eq!(result.get("callee_count"), Some(&Value::Null));
    assert!(result["collapse_note"]
        .as_str()
        .unwrap()
        .contains("listed separately"));
}
