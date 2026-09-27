//! One real MCP composition with synthetic project and personal evidence.

use super::*;
use std::io::Write;
use std::process::Stdio;

fn calls(f: &Fixture, requests: &[(&str, Value)]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mmcg"))
        .current_dir(&f.project)
        .env("HOME", &f.home)
        .env("USERPROFILE", &f.home)
        .env("CODEX_HOME", &f.codex)
        .env("MMCG_PROFILE_CLIENT", "fixture")
        .env_remove("MMCG_INDEX_PATH")
        .args(["serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}})).unwrap();
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    for (index, (name, args)) in requests.iter().enumerate() {
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":index+1,"method":"tools/call","params":{"name":name,"arguments":args}})).unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let responses: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (1..=requests.len())
        .map(|id| {
            let response = responses
                .iter()
                .find(|response| response["id"] == id)
                .unwrap();
            assert!(response.get("error").is_none(), "{response}");
            assert_ne!(response["result"]["isError"], true, "{response}");
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap()
        })
        .collect()
}

fn git(f: &Fixture, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(&f.project)
        .env("HOME", &f.home)
        .env("USERPROFILE", &f.home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "0")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn context_composition_keeps_role_scope_revisions_budget_and_freshness_separate() {
    let f = Fixture::new();
    git(&f, &["init", "-q"]);
    fs::create_dir(f.project.join("src")).unwrap();
    fs::create_dir(f.project.join("docs")).unwrap();
    fs::write(
        f.project.join("src/service.rs"),
        "pub fn route() -> bool { false }\n",
    )
    .unwrap();
    fs::write(
        f.project.join("CONTEXT.md"),
        "# Project\n\n## Contract\nKeep request identifiers stable.\n",
    )
    .unwrap();
    fs::write(
        f.project.join("docs/transport.md"),
        "# Transport\n\nThe transport validates request identifiers.\n",
    )
    .unwrap();
    git(&f, &["add", "src", "docs", "CONTEXT.md"]);
    git(&f, &["commit", "-qm", "Synthetic baseline"]);
    fs::write(
        f.project.join("src/service.rs"),
        "pub fn route() -> bool { true }\n",
    )
    .unwrap();
    f.success(&["index", "."]);
    let transcript = f.write(
        "sessions/2026/09/26/rollout-composition.jsonl",
        &f.preferences(
            "composition",
            &["I prefer short replies with test results."],
        ),
    );
    assert!(f.collect(&[&transcript], false).status.success());
    let candidate = f.inbox()["candidates"][0]["candidate"].clone();
    let statement = "Keep executor replies brief and include test results";
    assert!(f
        .propose_preference(&candidate, statement, Some("role:executor"))
        .status
        .success());
    let entry = ProfileStore::open_read_only(&f.db_path())
        .unwrap()
        .feedback()
        .unwrap()
        .remove(0);
    let accepted = f.in_fixture_terminal(&[
        "miner",
        "feedback",
        "accept",
        &entry.key,
        "--revision",
        &entry.review_revision(),
    ]);
    assert!(accepted.status.success(), "{accepted:?}");
    f.success(&["miner", "access", "grant", "--client", "fixture"]);
    let profile_args = |role: &str| json!({"paths":["src/service.rs"],"role":role,"workflow":"strict","budget_tokens":1500});
    let mut store_revision = Value::Null;
    let mut executor_view_revision = Value::Null;
    for role in ["planner", "executor", "auditor"] {
        let parts = calls(
            &f,
            &[
                (
                    "mmcg_brief",
                    json!({"role":role,"since":"HEAD","budget_tokens":2000}),
                ),
                ("mmcg_project_profile", json!({"top":2})),
                ("mmcg_docs", json!({"query":"transport","top":2})),
                ("mmcg_profile", profile_args(role)),
            ],
        );
        assert_eq!(parts[0]["role"], role);
        assert_eq!(parts[0]["freshness"]["structural"]["status"], "fresh");
        assert!(parts[0]["freshness"]["structural"]["checked_token"]
            .as_str()
            .is_some());
        assert_eq!(parts[1]["freshness"], "fresh");
        assert!(parts[1]["review_status"]
            .as_str()
            .unwrap()
            .starts_with("unknown;"));
        assert!(parts[1]["count"].as_u64().unwrap() > 0);
        assert_eq!(parts[2]["freshness"], "fresh");
        assert!(parts[2].to_string().contains("docs/transport.md"));
        let profile = &parts[3];
        assert_eq!(profile["selection"]["role"], role);
        assert_eq!(profile["selection"]["workflow"], "strict");
        assert_eq!(
            profile["feedback"].as_array().unwrap().len(),
            usize::from(role == "executor")
        );
        if role == "executor" {
            assert_eq!(profile["feedback"][0]["key"], entry.key);
            executor_view_revision = profile["profile_revision"].clone();
        }
        if store_revision.is_null() {
            store_revision = profile["store_revision"].clone();
        }
        assert_eq!(profile["store_revision"], store_revision);
        let handoff = json!({"role":role,"workflow":"strict","paths":["src/service.rs"],
            "budget_tokens":8000,"code":parts[0],"project":parts[1],"docs":parts[2],"person":parts[3]});
        assert!(serde_json::to_vec(&handoff).unwrap().len() <= 32000);
    }
    // Personal source drift cannot rewrite project knowledge or SQL review history.
    fs::remove_file(&transcript).unwrap();
    let after_source = calls(
        &f,
        &[
            ("mmcg_project_profile", json!({})),
            ("mmcg_profile", profile_args("executor")),
        ],
    );
    assert_eq!(after_source[0]["freshness"], "fresh");
    assert_eq!(after_source[1]["store_revision"], store_revision);
    assert_ne!(after_source[1]["profile_revision"], executor_view_revision);
    assert!(after_source[1]["feedback"].as_array().unwrap().is_empty());
    // Static Markdown still contains its old publication. Revocation never uses it.
    assert!(f.style().contains(statement));
    f.success(&["miner", "access", "revoke", "--client", "fixture"]);
    let denied = calls(&f, &[("mmcg_profile", profile_args("executor"))]);
    assert_eq!(denied[0]["status"], "access_denied");
    assert!(!denied[0].to_string().contains(statement));
    fs::write(
        f.project.join("CONTEXT.md"),
        "# Project\n\n## Contract\nKeep identifiers stable and validate the version.\n",
    )
    .unwrap();
    let stale = calls(&f, &[("mmcg_project_profile", json!({}))]);
    assert_ne!(stale[0]["freshness"], "fresh");
    assert!(stale[0]["observed"].as_array().unwrap().is_empty());
    f.success(&["index", "."]);
    let fresh = calls(&f, &[("mmcg_project_profile", json!({}))]);
    assert_eq!(fresh[0]["freshness"], "fresh");
    assert!(fresh[0].to_string().contains("validate the version"));
    assert_eq!(
        ProfileStore::open_read_only(&f.db_path())
            .unwrap()
            .aggregate()
            .unwrap()
            .profile_revision(),
        store_revision.as_str().unwrap()
    );
}
