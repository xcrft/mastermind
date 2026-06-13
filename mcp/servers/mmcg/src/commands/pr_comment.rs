use std::path::Path;

pub fn run(bundle_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string(bundle_path)
        .map_err(|e| format!("read {}: {e}", bundle_path.display()))?;
    let bundle: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse bundle JSON: {e}"))?;

    let verdict = bundle["verdict"]
        .as_str()
        .unwrap_or("unknown")
        .to_uppercase();
    let spec = bundle["spec"].as_str().unwrap_or("unknown");
    let baseline = bundle["baseline"]
        .as_str()
        .or_else(|| bundle["git_ref"].as_str())
        .unwrap_or("unknown");
    let head = bundle["head"].as_str().unwrap_or("unknown");
    let human_summary = bundle["human_summary"].as_str().unwrap_or("");

    let icon = match verdict.as_str() {
        "HELD" => "✅",
        "DRIFT" => "⚠️",
        _ => "❌",
    };

    println!("## {icon} Mastermind Audit — {verdict}");
    println!();
    if !human_summary.is_empty() {
        println!("> {human_summary}");
        println!();
    }
    println!("**Spec:** `{spec}`  ");
    println!("**Baseline:** `{baseline}` → **HEAD:** `{head}`");
    println!();

    let failed_claims = bundle["failed_claims"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    let verified_claims = bundle["verified_claims"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();

    if !failed_claims.is_empty() {
        println!("### Failed claims");
        println!();
        for c in &failed_claims {
            println!("- ❌ `{c}`");
        }
        println!();
    }

    if !verified_claims.is_empty() {
        println!("### Verified claims");
        println!();
        for c in &verified_claims {
            println!("- ✅ `{c}`");
        }
        println!();
    }

    let discrepancies = bundle["discrepancies"].as_array();
    if let Some(disc) = discrepancies {
        if !disc.is_empty() {
            println!("### Findings ({} total)", disc.len());
            println!();
            for f in disc {
                let kind = f["kind"].as_str().unwrap_or("unknown");
                let is_error = matches!(
                    kind,
                    "snapshot_symbol_gone"
                        | "removed_symbol_not_acknowledged"
                        | "claimed_symbol_missing"
                        | "hallucinated_symbol"
                        | "missing_call_edge"
                        | "claimed_signature_mismatch"
                        | "observed_exit_code_nonzero"
                        | "observed_zero_tests"
                );
                let bullet_icon = if is_error { "❌" } else { "⚠️" };
                let detail = render_finding_line(f);
                println!("- {bullet_icon} {detail}");
            }
            println!();
        }
    }

    let spec_files = bundle["spec_files"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    let changed_files = bundle["changed_files"]
        .as_array()
        .or_else(|| bundle["files_diff"].as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();

    if !spec_files.is_empty() || !changed_files.is_empty() {
        println!("<details>");
        println!("<summary>File scope</summary>");
        println!();
        if !spec_files.is_empty() {
            println!("**Declared by spec:**");
            for f in &spec_files {
                println!("- `{f}`");
            }
        }
        if !changed_files.is_empty() {
            println!();
            println!("**Changed (git diff):**");
            for f in &changed_files {
                println!("- `{f}`");
            }
        }
        println!();
        println!("</details>");
        println!();
    }

    let mmcg_queries = bundle["mmcg_queries"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    let commands = bundle["commands"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();

    if !mmcg_queries.is_empty() || !commands.is_empty() {
        println!("<details>");
        println!("<summary>Mechanical verification log</summary>");
        println!();
        if !mmcg_queries.is_empty() {
            println!("**mmcg queries:**");
            for q in &mmcg_queries {
                println!("- `{q}`");
            }
        }
        if !commands.is_empty() {
            println!();
            println!("**Verify commands:**");
            for c in &commands {
                println!("- `{c}`");
            }
        }
        println!();
        println!("</details>");
        println!();
    }

    println!("---");
    println!(
        "*Generated by [Mastermind](https://github.com/xcrft/mastermind) — mechanical spec audit*"
    );

    Ok(())
}

fn render_finding_line(f: &serde_json::Value) -> String {
    let kind = f["kind"].as_str().unwrap_or("unknown");
    match kind {
        "unexpected_file" => {
            let file = f["file"].as_str().unwrap_or("?");
            format!("`{file}` changed but not in spec — scope creep")
        }
        "missing_expected_file" => {
            let file = f["file"].as_str().unwrap_or("?");
            format!("`{file}` declared in spec but not in git diff")
        }
        "snapshot_caller_drift" => {
            let sym = f["symbol"].as_str().unwrap_or("?");
            let spec = f["spec_says"].as_u64().unwrap_or(0);
            let live = f["index_says"].as_u64().unwrap_or(0);
            format!("`{sym}` — spec said {spec} callers, index has {live}")
        }
        "snapshot_signature_drift" => {
            let sym = f["symbol"].as_str().unwrap_or("?");
            let spec = f["spec_says"].as_str().unwrap_or("?");
            let live = f["index_says"].as_str().unwrap_or("<unknown>");
            format!("`{sym}` — signature changed from `{spec}` to `{live}`")
        }
        "snapshot_symbol_gone" => {
            let sym = f["symbol"].as_str().unwrap_or("?");
            format!("`{sym}` — in pre-edit snapshot but gone from index")
        }
        "removed_symbol_not_acknowledged" => {
            let sym = f["symbol"].as_str().unwrap_or("?");
            let file = f["file"].as_str().unwrap_or("?");
            format!("`{sym}` deleted from `{file}` — not acknowledged in spec")
        }
        "planned_test_not_added" => {
            let test = f["test"].as_str().unwrap_or("?");
            format!("`{test}` — in Tests Plan but not in symbol diff")
        }
        "claimed_symbol_missing" => {
            let sym = f["symbol"].as_str().unwrap_or("?");
            let file = f["file"].as_str();
            if let Some(loc) = file {
                format!("`{sym}` claimed added in `{loc}` but not in index")
            } else {
                format!("`{sym}` claimed added but not in index")
            }
        }
        "hallucinated_symbol" => {
            let from = f["from_symbol"].as_str().unwrap_or("?");
            let to = f["to_symbol"].as_str().unwrap_or("?");
            format!("`{from}` claims to call `{to}` — `{to}` does not exist in index")
        }
        "missing_call_edge" => {
            let from = f["from_symbol"].as_str().unwrap_or("?");
            let to = f["to_symbol"].as_str().unwrap_or("?");
            format!("`{from}` claims to call `{to}` — no call edge found in index")
        }
        "vacuous_test_claim" => {
            let cmd = f["cmd"].as_str().unwrap_or("?");
            let reason = f["reason"].as_str().unwrap_or("?");
            format!("`{cmd}` claimed passed — {reason}")
        }
        "claimed_signature_mismatch" => {
            let sym = f["symbol"].as_str().unwrap_or("?");
            let claimed = f["claimed"].as_str().unwrap_or("?");
            let actual = f["actual"].as_str().unwrap_or("<none>");
            format!("`{sym}` — claimed signature `{claimed}`, index has `{actual}`")
        }
        "observed_exit_code_nonzero" => {
            let cmd = f["cmd"].as_str().unwrap_or("?");
            let code = f["exit_code"].as_i64().unwrap_or(-1);
            format!("`{cmd}` claimed passed but exit_code={code}")
        }
        "observed_zero_tests" => {
            let cmd = f["cmd"].as_str().unwrap_or("?");
            format!("`{cmd}` claimed passed but tests_run=0 — vacuous pass")
        }
        other => format!("({other}) {f}"),
    }
}
