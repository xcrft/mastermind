//! Private inbox retrieval. Search results are observations, never accepted claims.

use super::{sources, valid_id, Verifier};
use crate::miner::store::ProfileStore;
use serde_json::json;
use std::path::Path;

pub struct SearchOptions<'a> {
    pub query: &'a str,
    pub project_root: Option<&'a Path>,
    pub source: Option<&'a str>,
    pub status: &'a str,
    pub after: Option<&'a str>,
    pub limit: usize,
}

pub fn run(options: SearchOptions<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let SearchOptions {
        query,
        project_root,
        source,
        status,
        after,
        limit,
    } = options;
    let query = query.trim();
    if query.is_empty()
        || query.chars().count() > 128
        || query.chars().any(char::is_control)
        || !(1..=100).contains(&limit)
        || !matches!(status, "pending" | "dismissed" | "all")
        || after.is_some_and(|id| !valid_id(id))
        || source.is_some_and(|id| !sources::valid_source_id(id))
    {
        return Err(
            "invalid inbox search arguments; query must contain 1..128 printable characters".into(),
        );
    }
    let root = project_root.map(Path::canonicalize).transpose()?;
    if root.as_ref().is_some_and(|root| !root.is_dir()) {
        return Err("project root must be a directory".into());
    }
    let root = root
        .as_ref()
        .map(|root| root.to_str().ok_or("project root must be UTF-8"))
        .transpose()?;
    let path = ProfileStore::db_path().ok_or("could not resolve profile store")?;
    let db = ProfileStore::open_optional_read_only(&path)?;
    let (rows, examined, next_cursor) = if let Some(db) = &db {
        db.search_collected_candidates(
            &query.to_lowercase(),
            root,
            source,
            status,
            after.unwrap_or(""),
            limit,
        )?
    } else {
        (Vec::new(), 0, None)
    };
    let mut verifier = Verifier::new();
    let mut items = Vec::new();
    for candidate in rows {
        let links = db.as_ref().unwrap().candidate_claim_links(&candidate.id)?;
        items.push(json!({
            "freshness": verifier.freshness(&candidate), "candidate": candidate,
            "claim_links": links, "scope": "unresolved", "episode": null
        }));
    }
    let result = json!({
        "schema_version": 1, "query": query,
        "query_mode": "unicode_lowercase_literal_substring",
        "project_root": root, "source": source, "status": status,
        "coverage": "page", "search_corpus": "saved_detector_selected_inbox_quotes",
        "examined_records": examined, "record_work_limit": 10000,
        "count": items.len(), "candidates": items, "next_cursor": next_cursor,
        "source_verification": if verifier.incomplete { "incomplete" } else { "complete" },
        "note": "Local unreviewed evidence only. Does not search all human turns, Markdown, Git or claim definitions. Zero matches do not prove no relevant evidence exists. Use receipt links with feedback show / habit show; direct habit proposals are discoverable through habit list/show. Cursors are positions, not frozen snapshots."
    });
    let text = serde_json::to_string_pretty(&result)?;
    if text.len() > 1024 * 1024 {
        return Err("search response exceeds 1 MiB; reduce --limit".into());
    }
    println!("{text}");
    Ok(())
}
