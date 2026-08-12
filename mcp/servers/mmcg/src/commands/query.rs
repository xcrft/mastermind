use crate::QueryCmd;
use mmcg::{
    queries,
    store::{query_budget_ms_from_env, Store, WorkBudget, DEFAULT_CLI_BUDGET_MS},
};
use serde_json::Value;
use std::path::Path;

/// Pops the work-budget frame `execute` installs, even on an early `?`
/// return — `Store::with_work_budget` can't be used directly here since
/// `execute` returns `Result<Value, Box<dyn Error>>`, not a `SqlResult`.
struct WorkBudgetScope<'a>(&'a Store);

impl Drop for WorkBudgetScope<'_> {
    fn drop(&mut self) {
        self.0.pop_work_budget();
    }
}

pub fn dispatch(q: QueryCmd, index_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open(index_path)?;
    let is_explain = matches!(q, QueryCmd::Explain { .. });
    let result = execute(&store, q)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    if is_explain {
        if let Some(matched) = result.get("matched").and_then(|v| v.as_array()) {
            if matched.is_empty() {
                let query = result.get("query").and_then(|v| v.as_str()).unwrap_or("?");
                eprintln!("\nMatched 0 symbols for {query:?}.");
                eprintln!("Possible reasons:");
                eprintln!("  - symbol not yet indexed (run: mastermind index .)");
                eprintln!("  - generated dynamically (Python metaclass, TS decorators, macros)");
                eprintln!("  - file not in index (check extension or .gitignore)");
                eprintln!("  - language parser limitation (C++ macros, Rust proc-macros)");
                eprintln!("  - wrong name (try a prefix: mmcg query search {query})");
                eprintln!("\nTry:");
                eprintln!("  mastermind query files --prefix <dir>");
                eprintln!("  mastermind index --force .");
            }
        }
    }
    Ok(())
}

pub fn dispatch_map(
    path: &str,
    format: crate::MapFormat,
    depth: u8,
    top: u32,
    production_only: bool,
    index_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open(index_path)?;
    let budget_ms = query_budget_ms_from_env(DEFAULT_CLI_BUDGET_MS);
    store.push_work_budget(WorkBudget::from_millis(budget_ms));
    let _budget_scope = WorkBudgetScope(&store);
    let map = queries::project_map_with_options(&store, path, depth, top, production_only)?;
    match format {
        crate::MapFormat::Json => println!("{}", serde_json::to_string_pretty(&map)?),
        crate::MapFormat::Text => print!("{}", render_map_text(&map)),
        crate::MapFormat::Mermaid => print!("{}", render_map_mermaid(&map)),
        crate::MapFormat::Sarif => println!(
            "{}",
            serde_json::to_string_pretty(&mmcg::sarif_export::project_map(&map))?
        ),
    }
    Ok(())
}

pub fn dispatch_history(
    query: &str,
    kind: Option<&str>,
    top: u32,
    index_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open(index_path)?;
    let response = queries::history(&store, query, kind, top.clamp(1, 50))?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

pub fn dispatch_why(
    query: &str,
    top: u32,
    index_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open(index_path)?;
    let retrieval_query = natural_language_history_query(query);
    let response = queries::history(&store, &retrieval_query, None, top.clamp(1, 50))?;
    println!("mastermind why — {}\n", safe_line_text(query));
    println!("Observed");
    if response.observed.is_empty() {
        println!("  No matching durable history was found.");
    } else {
        for hit in &response.observed {
            println!(
                "  [{}] {} — {}\n    {}",
                safe_line_text(&hit.kind),
                safe_line_text(&hit.title),
                safe_line_text(&hit.path),
                safe_line_text(&hit.excerpt)
            );
        }
    }
    println!("\nInferred");
    println!("  None. Retrieval rank and co-occurrence do not prove rationale or correctness.");
    println!("\nUnknown");
    if response.observed.is_empty() {
        println!(
            "  Whether the rationale was never recorded, uses different terms, or was added after the last index refresh."
        );
    } else {
        println!(
            "  Whether these records fully explain the decision; verify the returned Markdown and current runtime path."
        );
    }
    println!("\nWhat would change this conclusion");
    if response.observed.is_empty() {
        println!(
            "  A matching durable record after a broader query or fresh index, Git evidence, or a current runtime fact."
        );
    } else {
        println!(
            "  Newer active records, a superseding decision, contradictory runtime evidence, or a failed verification."
        );
    }
    Ok(())
}

fn natural_language_history_query(question: &str) -> String {
    const STOP_WORDS: &[&str] = &[
        "a", "an", "are", "did", "do", "does", "how", "is", "of", "our", "project", "the", "this",
        "to", "was", "were", "what", "why",
    ];
    let mut terms = question
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .filter(|term| !STOP_WORDS.contains(&term.as_str()))
        .take(12)
        .collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    terms
        .into_iter()
        .map(|term| format!("\"{term}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn safe_text(value: &str) -> String {
    value.chars().flat_map(char::escape_default).collect()
}

fn safe_line_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            output.extend(character.escape_default());
        } else {
            output.push(character);
        }
    }
    output
}

fn mermaid_label(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_alphanumeric() || matches!(ch, ' ' | '/' | '.' | '_' | '-') {
            out.push(ch);
        } else {
            out.push_str(&format!("&#{};", ch as u32));
        }
    }
    out
}

fn render_map_text(map: &queries::ProjectMapResponse) -> String {
    let mut out = format!(
        "mastermind map — {} ({}, {} files)\n\nLanguages\n",
        safe_text(&map.scope.path),
        map.scope.kind,
        map.files.total.unwrap_or(map.files.returned)
    );
    for language in &map.languages.items {
        out.push_str(&format!(
            "  {}: {}\n",
            safe_text(&language.language),
            language.file_count
        ));
    }
    out.push_str("\nComponents\n");
    for component in &map.components.items {
        out.push_str(&format!(
            "  {} ({} files, {} boundaries)\n",
            safe_text(&component.path),
            component.file_count,
            component.boundaries.returned
        ));
    }
    out.push_str("\nLikely entry points (heuristic)\n");
    for entry in &map.entry_points.items {
        out.push_str(&format!("  {}\n", safe_text(&entry.file)));
    }
    out.push_str("\nHotspots\n");
    for hotspot in &map.hotspots.items {
        out.push_str(&format!(
            "  {} — {}:{} (in-degree {}, collisions {})\n",
            safe_text(&hotspot.name),
            safe_text(&hotspot.file),
            hotspot.line,
            hotspot.in_degree,
            hotspot.name_collision
        ));
    }
    out.push_str("\nPrecision notes\n");
    for note in &map.precision_notes {
        out.push_str(&format!("  {}: {}\n", note.code, safe_text(note.message)));
    }
    out
}

fn render_map_mermaid(map: &queries::ProjectMapResponse) -> String {
    let mut out = String::from("flowchart TD\n");
    out.push_str("  classDef boundary fill:#e8f1ff,stroke:#4b75b8\n");
    out.push_str("  classDef hotspot fill:#fff0e6,stroke:#c35b22\n");
    out.push_str("  classDef cycle fill:#ffe8e8,stroke:#b84b4b\n");
    let file_count = map.files.total.unwrap_or(map.files.returned);
    let language_summary = map
        .languages
        .items
        .iter()
        .take(4)
        .map(|item| format!("{} {}", item.language, item.file_count))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!(
        "  n0[\"{} | {} files | {}\"]\n",
        mermaid_label(&map.scope.path),
        file_count,
        mermaid_label(&language_summary)
    ));
    for (index, component) in map.components.items.iter().enumerate() {
        let id = index + 1;
        let languages = component
            .languages
            .iter()
            .take(3)
            .map(|item| format!("{} {}", item.language, item.file_count))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "  n{id}[\"{} | {} files | {}\"]\n  n0 --> n{id}\n",
            mermaid_label(&component.path),
            component.file_count,
            mermaid_label(&languages)
        ));
        for (boundary_index, boundary) in component.boundaries.items.iter().take(5).enumerate() {
            let boundary_id = format!("b{id}_{boundary_index}");
            out.push_str(&format!(
                "  {boundary_id}[\"boundary {} | {}:{}\"]:::boundary\n  n{id} -.-> {boundary_id}\n",
                mermaid_label(&boundary.name),
                mermaid_label(&boundary.file),
                boundary.line
            ));
        }
    }
    for (index, hotspot) in map.hotspots.items.iter().take(10).enumerate() {
        out.push_str(&format!(
            "  h{index}[\"hotspot {} | degree {} | {}:{}\"]:::hotspot\n  n0 --> h{index}\n",
            mermaid_label(&hotspot.name),
            hotspot.in_degree,
            mermaid_label(&hotspot.file),
            hotspot.line
        ));
    }
    for (cycle_index, cycle) in map.cycles.items.iter().take(5).enumerate() {
        let display_index = cycle_index + 1;
        out.push_str(&format!(
            "  cy{cycle_index}[\"cycle {display_index} | {} files\"]:::cycle\n",
            cycle.len()
        ));
        for (member_index, member) in cycle.iter().enumerate() {
            let member_id = format!("cy{cycle_index}_{member_index}");
            out.push_str(&format!(
                "  {member_id}[\"{}\"]:::cycle\n  cy{cycle_index} --> {member_id}\n",
                mermaid_label(member)
            ));
            let next = (member_index + 1) % cycle.len();
            out.push_str(&format!("  {member_id} -.-> cy{cycle_index}_{next}\n"));
        }
    }
    out
}

pub fn render_change_impact(
    response: &queries::ChangeImpactResponse,
    format: crate::ImpactFormat,
) -> Result<String, serde_json::Error> {
    match format {
        crate::ImpactFormat::Json => {
            let mut output = serde_json::to_string_pretty(response)?;
            output.push('\n');
            return Ok(output);
        }
        crate::ImpactFormat::Sarif => {
            let mut output =
                serde_json::to_string_pretty(&mmcg::sarif_export::change_impact(response))?;
            output.push('\n');
            return Ok(output);
        }
        crate::ImpactFormat::Text => {}
    }
    let mut output = format!(
        "mastermind impact — {}..{}\n\nChanged symbols\n",
        safe_text(&response.baseline.requested_ref),
        safe_text(&response.baseline.head_oid)
    );
    for symbol in &response.changes.symbols.items {
        output.push_str(&format!(
            "  {} {} — {}:{} ({})\n",
            safe_text(&symbol.kind),
            safe_text(&symbol.name),
            safe_text(&symbol.file),
            symbol.line,
            safe_text(&symbol.change)
        ));
    }
    output.push_str("\nImpacted callers\n");
    for impact in &response.impact.items {
        output.push_str(&format!(
            "  {} — {}:{} (depth {}, {} seeds)\n",
            safe_text(&impact.symbol.name),
            safe_text(&impact.symbol.file),
            impact.symbol.line,
            impact.minimum_depth,
            impact.seeds.len()
        ));
    }
    output.push_str("\nCandidate tests\n");
    for test in &response.tests.items {
        output.push_str(&format!(
            "  {} — {}:{} ({}, {})\n",
            safe_text(&test.symbol.name),
            safe_text(&test.symbol.file),
            test.symbol.line,
            safe_text(&test.classification),
            safe_text(&test.confidence)
        ));
    }
    output.push_str("\nPrecision notes\n");
    for note in &response.precision_notes {
        output.push_str(&format!("  {}\n", safe_text(note)));
    }
    Ok(output)
}

fn execute(store: &Store, q: QueryCmd) -> Result<Value, Box<dyn std::error::Error>> {
    let budget_ms = query_budget_ms_from_env(DEFAULT_CLI_BUDGET_MS);
    store.push_work_budget(WorkBudget::from_millis(budget_ms));
    let _budget_scope = WorkBudgetScope(store);
    Ok(match q {
        QueryCmd::Search {
            name,
            kind,
            language,
            no_collapse_partials,
        } => serde_json::to_value(queries::search(
            store,
            &name,
            kind.as_deref(),
            language.as_deref(),
            !no_collapse_partials,
        )?)?,
        QueryCmd::Callers {
            name,
            language,
            edge_kind,
        } => serde_json::to_value(queries::callers(
            store,
            &name,
            language.as_deref(),
            edge_kind.as_deref(),
        )?)?,
        QueryCmd::Callees {
            name,
            language,
            edge_kind,
        } => serde_json::to_value(queries::callees(
            store,
            &name,
            language.as_deref(),
            edge_kind.as_deref(),
        )?)?,
        QueryCmd::Impact {
            name,
            depth,
            language,
        } => serde_json::to_value(queries::impact(store, &name, depth, language.as_deref())?)?,
        QueryCmd::Files { prefix, language } => serde_json::to_value(queries::files(
            store,
            prefix.as_deref(),
            language.as_deref(),
        )?)?,
        QueryCmd::SymbolsInFile { file } => {
            serde_json::to_value(queries::symbols_in_file(store, &file)?)?
        }
        QueryCmd::Outline { file } => serde_json::to_value(queries::outline(store, &file)?)?,
        QueryCmd::Recent { since } => serde_json::to_value(
            queries::recent_changes(store, &since).map_err(|e| format!("recent_changes: {e}"))?,
        )?,
        QueryCmd::Unreferenced { kind, language } => serde_json::to_value(queries::unreferenced(
            store,
            kind.as_deref(),
            language.as_deref(),
        )?)?,
        QueryCmd::ApiSurface { prefix, language } => {
            serde_json::to_value(queries::api_surface(store, &prefix, language.as_deref())?)?
        }
        QueryCmd::DependencyCycles { language, min_size } => serde_json::to_value(
            queries::dependency_cycles(store, language.as_deref(), min_size)?,
        )?,
        QueryCmd::Centrality {
            prefix,
            language,
            kind,
            top,
        } => serde_json::to_value(queries::centrality(
            store,
            prefix.as_deref(),
            language.as_deref(),
            kind.as_deref(),
            top,
        )?)?,
        QueryCmd::Tasks { query, top } => {
            serde_json::to_value(queries::tasks(store, &query, top)?)?
        }
        QueryCmd::History { query, kind, top } => serde_json::to_value(queries::history(
            store,
            &query,
            kind.as_deref(),
            top.clamp(1, 50),
        )?)?,
        QueryCmd::SymbolsChangedSince { git_ref, root } => {
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize {}: {e}", root.display()))?;
            let diff = queries::symbols_changed_since(store, &root, &git_ref)?;
            serde_json::to_value(diff)?
        }
        QueryCmd::ImportedBy {
            query,
            match_kind,
            language,
        } => serde_json::to_value(queries::imported_by(
            store,
            &query,
            &match_kind,
            language.as_deref(),
        )?)?,
        QueryCmd::Explain { name, language } => {
            serde_json::to_value(queries::explain(store, &name, language.as_deref())?)?
        }
        QueryCmd::Semantic { symbol, top } => {
            serde_json::to_value(mmcg::scip_overlay::query(store, &symbol, top)?)?
        }
    })
}

#[cfg(test)]
mod map_tests {
    use super::*;

    #[test]
    fn map_renderers_escape_repository_control_syntax() {
        let hostile = "x\n\u{1b}]0;owned\u{7}%% click [\"`";
        let text = safe_text(hostile);
        assert!(!text.contains('\n'));
        assert!(!text.contains('\u{1b}'));
        let label = mermaid_label(hostile);
        assert!(!label.contains("%%"));
        assert!(!label.contains('['));
        assert!(!label.contains('"'));
    }

    #[test]
    fn why_questions_become_safe_bounded_fts_queries() {
        assert_eq!(
            natural_language_history_query("Why is source-of-truth important?"),
            "\"important\" OR \"source\" OR \"truth\""
        );
        assert_eq!(
            natural_language_history_query("Почему выбрали Redis?"),
            "\"redis\" OR \"выбрали\" OR \"почему\""
        );
        assert_eq!(natural_language_history_query("why is this?"), "");
        assert_eq!(safe_line_text("почему\nтак"), "почему\\nтак");
    }

    #[test]
    fn mermaid_map_includes_boundaries_hotspots_and_cycles() {
        let path = std::env::temp_dir().join(format!("mmcg-mermaid-map-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path).unwrap();
        for file in ["src/a.py", "src/b.py", "outside.py"] {
            store.upsert_file(file, 1, 1).unwrap();
        }
        let a = store
            .insert_symbol("alpha", "function", "src/a.py", 1, 3, None, None)
            .unwrap();
        let b = store
            .insert_symbol("beta", "function", "src/b.py", 1, 3, None, None)
            .unwrap();
        let outside = store
            .insert_symbol("outside", "function", "outside.py", 1, 3, None, None)
            .unwrap();
        store
            .insert_edge(outside, Some(a), "alpha", "calls", 2)
            .unwrap();
        store.insert_edge(a, Some(b), "beta", "imports", 2).unwrap();
        store
            .insert_edge(b, Some(a), "alpha", "imports", 2)
            .unwrap();

        let map = queries::project_map(&store, ".", 2, 20).unwrap();
        let rendered = render_map_mermaid(&map);
        assert!(rendered.contains("files"));
        assert!(rendered.contains("boundary alpha"));
        assert!(rendered.contains("hotspot alpha"));
        assert!(rendered.contains("cycle 1"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn sarif_map_exports_cycles_with_stable_rules_and_relative_locations() {
        let path = std::env::temp_dir().join(format!("mmcg-sarif-map-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path).unwrap();
        for file in ["src/a file.py", "src/b.py"] {
            store.upsert_file(file, 1, 1).unwrap();
        }
        let a = store
            .insert_symbol("alpha", "function", "src/a file.py", 1, 3, None, None)
            .unwrap();
        let b = store
            .insert_symbol("beta", "function", "src/b.py", 1, 3, None, None)
            .unwrap();
        store.insert_edge(a, Some(b), "beta", "imports", 2).unwrap();
        store
            .insert_edge(b, Some(a), "alpha", "imports", 2)
            .unwrap();

        let map = queries::project_map(&store, ".", 2, 20).unwrap();
        let sarif = mmcg::sarif_export::project_map(&map);
        assert_eq!(sarif["version"], "2.1.0");
        assert_eq!(
            sarif["runs"][0]["tool"]["driver"]["rules"][0]["id"],
            "mastermind/dependency-cycle"
        );
        assert_eq!(
            sarif["runs"][0]["results"][0]["ruleId"],
            "mastermind/dependency-cycle"
        );
        assert_eq!(
            sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]
                ["uri"],
            "src/a%20file.py"
        );
        assert_eq!(
            sarif["runs"][0]["results"][0]["relatedLocations"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        std::fs::remove_file(path).ok();
    }
}
