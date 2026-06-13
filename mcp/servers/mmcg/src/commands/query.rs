use crate::QueryCmd;
use mmcg::{queries, store::Store};
use serde_json::Value;
use std::path::Path;

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

fn execute(store: &Store, q: QueryCmd) -> Result<Value, Box<dyn std::error::Error>> {
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
    })
}
