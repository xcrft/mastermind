use mmcg::{indexer::Indexer, store::Store};

fn index(source: &str) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), source).unwrap();
    let mut store = Store::open(dir.path().join("graph.db")).unwrap();
    Indexer::new(dir.path())
        .index_all(&mut store, true)
        .unwrap();
    (dir, store)
}

#[test]
fn receiver_calls_do_not_reference_a_free_function_with_the_same_name() {
    let (_dir, store) = index(
        "fn map() -> u32 { 999 }\nfn render_values() { (0..3).map(|v| v + 1).collect::<Vec<_>>(); }",
    );
    assert!(store.callers_of("map", None, None).unwrap().is_empty());
    assert!(store
        .impact_of_many(&["map".into()], 3, 100, None)
        .unwrap()
        .is_empty());
    assert!(store
        .unreferenced(Some("function"), None)
        .unwrap()
        .iter()
        .any(|s| s.name == "map"));
    assert!(!store
        .centrality(None, None, None, 100)
        .unwrap()
        .iter()
        .any(|(s, _, _)| s.name == "map"));
    for (scope, kind) in [(".", "root"), ("src", "directory"), ("src/lib.rs", "file")] {
        assert!(!store
            .map_centrality(scope, kind, 100)
            .unwrap()
            .iter()
            .any(|r| r.symbol.name == "map"));
    }
    assert!(store.file_in_degrees(false, 100).unwrap().is_empty());
}

#[test]
fn receiver_and_free_calls_keep_separate_degrees_even_with_the_same_name() {
    let (_dir, store) = index(
        "fn run() {}\nstruct Worker;\nimpl Worker { fn run(&self) {} }\nfn free_entry() { run(); }\nfn method_entry(w: Worker) { w.run(); }",
    );
    let degrees = store.centrality(None, None, None, 100).unwrap();
    let runs: Vec<_> = degrees.iter().filter(|(s, _, _)| s.name == "run").collect();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|(_, degree, _)| *degree == 1));
    for (scope, kind) in [(".", "root"), ("src", "directory")] {
        let rows = store.map_centrality(scope, kind, 100).unwrap();
        let runs: Vec<_> = rows.iter().filter(|r| r.symbol.name == "run").collect();
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|r| r.in_degree == 1));
    }
}

#[test]
fn transitive_impact_does_not_jump_from_a_free_function_to_a_same_named_method() {
    let (_dir, store) = index(
        "fn changed() {}\nfn run() { changed(); }\nstruct Worker;\nimpl Worker { fn run(&self) {} }\nfn free_entry() { run(); }\nfn method_entry(w: Worker) { w.run(); }",
    );
    let impact = store
        .impact_of_many(&["changed".into()], 3, 100, None)
        .unwrap();
    assert!(impact
        .iter()
        .any(|r| r.symbol.name == "free_entry" && r.depth == 2));
    assert!(!impact.iter().any(|r| r.symbol.name == "method_entry"));
}

#[test]
fn scoped_alias_and_trait_calls_remain_candidates_without_compiler_resolution() {
    let (_dir, store) = index(
        "struct Worker;\nimpl Worker { fn execute() {} }\ntype Alias = Worker;\nfn via_alias() { Alias::execute(); }\ntrait Run { fn operate(&self); }\nimpl Run for Worker { fn operate(&self) {} }\nfn via_trait(w: &Worker) { Run::operate(w); }",
    );
    for (name, caller) in [("execute", "via_alias"), ("operate", "via_trait")] {
        assert!(store
            .callers_of(name, None, None)
            .unwrap()
            .iter()
            .any(|s| s.name == caller));
        assert!(!store
            .unreferenced(Some("method"), None)
            .unwrap()
            .iter()
            .any(|s| s.name == name));
    }
}

#[test]
fn external_receiver_calls_do_not_create_a_false_api_boundary() {
    let (dir, mut store) = index("pub fn map() -> u32 { 999 }");
    std::fs::write(
        dir.path().join("entry.rs"),
        "fn entry() { (0..3).map(|x| x + 1).collect::<Vec<_>>(); }",
    )
    .unwrap();
    Indexer::new(dir.path())
        .index_all(&mut store, true)
        .unwrap();
    assert!(store.api_surface("src/", None).unwrap().is_empty());
    let components = [mmcg::store::MapBoundaryScope {
        label: "src".into(),
        path: "src".into(),
        match_mode: mmcg::store::MapBoundaryMatch::Recursive,
    }];
    assert!(store
        .map_boundaries(&components, 10, 10)
        .unwrap()
        .is_empty());
}

#[test]
fn callback_and_macro_references_participate_in_impact_and_liveness() {
    let (_dir, store) = index(
        r#"fn callback_target() -> u32 { 7 }
fn invoke(callback: fn() -> u32) -> u32 { callback() }
fn registered_entry() -> u32 { invoke(callback_target) }
fn macro_target() -> u32 { 11 }
fn macro_entry() { println!("{}", macro_target()); }
fn main() { registered_entry(); macro_entry(); }
"#,
    );
    for (target, entry) in [
        ("callback_target", "registered_entry"),
        ("macro_target", "macro_entry"),
    ] {
        let refs = store.callers_of(target, None, Some("references")).unwrap();
        assert!(
            refs.iter().any(|s| s.name == entry),
            "missing reference to {target}"
        );
        assert!(
            store.callers_of(target, None, None).unwrap().is_empty(),
            "a reference must not claim a proven call"
        );
        let impact = store
            .impact_of_many(&[target.into()], 3, 100, None)
            .unwrap();
        assert!(impact
            .iter()
            .any(|r| r.symbol.name == entry && r.depth == 1));
        assert!(impact
            .iter()
            .any(|r| r.symbol.name == "main" && r.depth == 2));
        assert!(!store
            .unreferenced(Some("function"), None)
            .unwrap()
            .iter()
            .any(|s| s.name == target));
    }
}
