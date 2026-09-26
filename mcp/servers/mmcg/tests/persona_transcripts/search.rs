use super::*;

fn search(f: &Fixture, query: &str, options: &[&str]) -> Value {
    let mut args = vec!["miner", "candidates", "search", query];
    args.extend_from_slice(options);
    serde_json::from_slice(&f.success(&args).stdout).unwrap()
}

#[test]
fn inbox_search_is_unicode_literal_scoped_paginated_and_read_only() {
    let f = Fixture::new();
    let path = f.write(
        "sessions/2026/09/26/rollout-search-a.jsonl",
        &f.preferences(
            "search-a",
            &[
                "Я предпочитаю короткие ответы.",
                "Я предпочитаю короткие планы.",
                "I prefer replies with 100% literal_test coverage.",
            ],
        ),
    );
    assert!(f.collect(&[&path], false).status.success());
    let unrelated = f.write(
        "sessions/2026/09/26/rollout-search-b.jsonl",
        &f.preferences("search-b", &["I prefer brief replies."]),
    );
    assert!(f.collect(&[&unrelated], false).status.success());
    fs::remove_file(&unrelated).unwrap();
    let current = f.inbox()["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["candidate"]["quote"] == "Я предпочитаю короткие ответы.")
        .unwrap()["candidate"]
        .clone();
    let proposal = f.propose_preference(&current, "Prefer short replies", None);
    assert!(proposal.status.success(), "{proposal:?}");
    let proposal: Value = serde_json::from_slice(&proposal.stdout).unwrap();
    let before = fs::read(f.db_path()).unwrap();
    let style = f.style();
    let result = search(
        &f,
        "КОРОТКИЕ",
        &["--project-root", ".", "--source", "session:codex:search-a"],
    );
    assert_eq!(result["count"], 2);
    assert_eq!(result["source_verification"], "complete");
    assert_eq!(
        result["search_corpus"],
        "saved_detector_selected_inbox_quotes"
    );
    let linked = result["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["candidate"]["id"] == current["id"])
        .unwrap();
    assert_eq!(linked["freshness"], "current");
    assert_eq!(linked["candidate"]["line_no"], current["line_no"]);
    assert_eq!(
        linked["candidate"]["record_digest"],
        current["record_digest"]
    );
    assert_eq!(
        linked["claim_links"]["items"][0]["id"],
        proposal["proposal"]["feedback_key"]
    );
    assert_eq!(
        linked["claim_links"]["items"][0]["candidate_revision"],
        current["revision"]
    );
    let first = search(&f, "КОРОТКИЕ", &["--limit", "1"]);
    let second = search(
        &f,
        "КОРОТКИЕ",
        &[
            "--limit",
            "1",
            "--after",
            first["next_cursor"].as_str().unwrap(),
        ],
    );
    assert!(second["next_cursor"].is_null());
    assert_ne!(
        first["candidates"][0]["candidate"]["id"],
        second["candidates"][0]["candidate"]["id"]
    );
    assert_eq!(search(&f, "100% LITERAL_TEST", &[])["count"], 1);
    assert_eq!(search(&f, "%' OR 1=1 --", &[])["count"], 0);
    assert_eq!(
        search(&f, "КОРОТКИЕ", &["--status", "dismissed"])["count"],
        0
    );
    let elsewhere = f._temp.path().join("other");
    fs::create_dir(&elsewhere).unwrap();
    assert_eq!(
        search(
            &f,
            "КОРОТКИЕ",
            &["--project-root", elsewhere.to_str().unwrap()]
        )["count"],
        0
    );
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
    assert_eq!(f.style(), style);
}

#[test]
fn inbox_search_empty_invalid_and_legacy_queries_never_create_or_migrate_a_store() {
    let f = Fixture::new();
    assert_eq!(search(&f, "review", &[])["count"], 0);
    assert!(!f.db_path().exists());
    for query in ["", " ", "line\nbreak"] {
        assert!(!f
            .run(&["miner", "candidates", "search", query])
            .status
            .success());
    }
    for options in [
        vec!["--limit", "0"],
        vec!["--after", "partial"],
        vec!["--source", "partial"],
        vec!["--status", "active"],
    ] {
        let mut args = vec!["miner", "candidates", "search", "review"];
        args.extend(options);
        assert!(!f.run(&args).status.success());
    }
    assert!(!f.db_path().exists());
    fs::create_dir_all(f.db_path().parent().unwrap()).unwrap();
    Connection::open(f.db_path())
        .unwrap()
        .execute_batch("CREATE TABLE legacy (id INTEGER)")
        .unwrap();
    let before = fs::read(f.db_path()).unwrap();
    assert_eq!(search(&f, "review", &[])["count"], 0);
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
}

#[test]
fn inbox_search_reports_stale_evidence_and_source_verification_caps() {
    let f = Fixture::new();
    let mut paths = Vec::new();
    for i in 0..17 {
        let id = format!("search-{i}");
        let path = f.write(
            &format!("sessions/2026/09/26/rollout-{id}.jsonl"),
            &f.preferences(&id, &["I prefer concise test reports."]),
        );
        assert!(f.collect(&[&path], false).status.success());
        paths.push(path);
    }
    let before = fs::read(f.db_path()).unwrap();
    let result = search(&f, "concise", &[]);
    assert_eq!(result["count"], 17);
    assert_eq!(result["source_verification"], "incomplete");
    assert_eq!(
        result["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["freshness"] == "unchecked")
            .count(),
        1
    );
    assert_eq!(fs::read(f.db_path()).unwrap(), before);
    fs::write(
        &paths[0],
        f.preferences("search-0", &["A one-off unrelated task."]),
    )
    .unwrap();
    let changed = search(&f, "concise", &["--source", "session:codex:search-0"]);
    assert_eq!(changed["candidates"][0]["freshness"], "changed");
    assert!(f.collect(&[&paths[0]], false).status.success());
    let removed = search(&f, "concise", &["--source", "session:codex:search-0"]);
    assert_eq!(removed["candidates"][0]["freshness"], "removed");
    assert_eq!(removed["candidates"][0]["candidate"]["present"], false);
}
