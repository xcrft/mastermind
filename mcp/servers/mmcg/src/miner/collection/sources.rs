//! Explicit, paged reuse of successful collection checkpoints. A checkpoint
//! records a previous selection; it is not a background-discovery permission.

use super::super::store::CollectionSourceSelection;
use super::{
    feedback, mutate_private_store, prepare_inputs, CollectionBatch, CollectionInput,
    CollectionStats, ProfileStore, EXTRACTOR, MAX_FILES,
};
use serde_json::{json, Value};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

const PAGE_NOTE: &str = "The cursor is a page position, not a mining checkpoint. Start each new pass without --after to include appended sessions and new source IDs before the cursor. Pages are not a frozen registry snapshot.";

pub(super) fn valid_source_id(source: &str) -> bool {
    source
        .strip_prefix("session:codex:")
        .or_else(|| source.strip_prefix("session:"))
        .is_some_and(|id| feedback::valid_session_id(id).is_some())
}

fn validate_page(
    root: &Path,
    after: Option<&str>,
    limit: usize,
    max_limit: usize,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if !(1..=max_limit).contains(&limit) || after.is_some_and(|id| !valid_source_id(id)) {
        return Err("invalid source page arguments; use a full source ID from sources list".into());
    }
    let root = root.canonicalize()?;
    if !root.is_dir() || root.to_str().is_none() {
        return Err("project root must be a UTF-8 directory".into());
    }
    Ok(root)
}

struct SourcePage {
    sources: Vec<CollectionSourceSelection>,
    next_cursor: Option<String>,
}

fn page(
    db: Option<&ProfileStore>,
    root: &Path,
    after: Option<&str>,
    limit: usize,
    include_excluded: bool,
) -> Result<SourcePage, Box<dyn std::error::Error>> {
    let mut sources = db
        .map(|db| {
            db.collection_sources(
                root.to_str().ok_or(rusqlite::Error::InvalidQuery)?,
                after.unwrap_or(""),
                limit + 1,
                include_excluded,
            )
        })
        .transpose()?
        .unwrap_or_default();
    let more = sources.len() > limit;
    sources.truncate(limit);
    let next_cursor = more.then(|| sources.last().unwrap().snapshot.source.clone());
    Ok(SourcePage {
        sources,
        next_cursor,
    })
}

/// Inspect stored metadata without opening transcripts or migrating the store.
pub fn list(
    root: &Path,
    after: Option<&str>,
    limit: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = validate_page(root, after, limit, 100)?;
    let path = ProfileStore::db_path().ok_or("could not resolve profile store")?;
    let db = ProfileStore::open_optional_read_only(&path)?;
    let page = page(db.as_ref(), &root, after, limit, true)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "project_root":root, "sources":page.sources,
            "next_cursor":page.next_cursor, "coverage":"page",
            "freshness":"not_checked", "note":PAGE_NOTE,
            "selection":"Previously collected sources for this exact project root. Stored metadata is not current-source verification or permission for automatic discovery."
        }))?
    );
    Ok(())
}

fn prepare_page(
    db: Option<&ProfileStore>,
    root: &Path,
    after: Option<&str>,
    limit: usize,
) -> Result<(SourcePage, Vec<CollectionBatch>), Box<dyn std::error::Error>> {
    let page = page(db, root, after, limit, false)?;
    let inputs: Vec<_> = page
        .sources
        .iter()
        .map(|source| CollectionInput::Registered(&source.snapshot))
        .collect();
    let batches = if inputs.is_empty() {
        Vec::new()
    } else {
        prepare_inputs(db, root, &inputs)?
    };
    Ok((page, batches))
}

fn report(
    root: &Path,
    page: &SourcePage,
    batches: &[CollectionBatch],
    dry_run: bool,
    stats: Option<CollectionStats>,
) -> Value {
    let mut result = json!({
        "status":"complete", "coverage":"page", "dry_run":dry_run,
        "project_root":root, "extractor":EXTRACTOR,
        "sources_selected":page.sources.len(),
        "source_ids":page.sources.iter().map(|source| &source.snapshot.source).collect::<Vec<_>>(),
        "next_cursor":page.next_cursor, "note":PAGE_NOTE,
        "scope":"unresolved", "episode":null
    });
    if dry_run {
        result["sources"] = json!(batches
            .iter()
            .map(|batch| &batch.source)
            .collect::<Vec<_>>());
        result["candidates"] = json!(batches
            .iter()
            .flat_map(|batch| &batch.candidates)
            .collect::<Vec<_>>());
        result["preview_note"] = json!("Only new or changed snapshots produce preview observations. A subsequent sync reads the sources again; this preview does not pin them.");
    }
    if let Some(stats) = stats {
        result["collection"] = json!(stats);
    }
    result
}

/// Reread one page of already selected sources. This never discovers files,
/// reviews candidates, changes curation receipts, or publishes a profile.
pub fn sync(
    root: &Path,
    after: Option<&str>,
    limit: usize,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = validate_page(root, after, limit, MAX_FILES)?;
    let path = ProfileStore::db_path().ok_or("could not resolve profile store")?;
    let result = (|| {
        let db = ProfileStore::open_optional_read_only(&path)?;
        if dry_run {
            let (page, batches) = prepare_page(db.as_ref(), &root, after, limit)?;
            return Ok(report(&root, &page, &batches, true, None));
        }
        // A read-only empty selection needs neither a store nor a lock file.
        // For nonempty work, select AGAIN under the writer lock: an explicit
        // collect could have relocated a source while sync waited for the lock.
        let initial = page(db.as_ref(), &root, after, limit, false)?;
        if initial.sources.is_empty() {
            return Ok(report(
                &root,
                &initial,
                &[],
                false,
                Some(CollectionStats::default()),
            ));
        }
        drop(db);
        mutate_private_store(
            &path,
            |db| {
                let (page, batches) = prepare_page(db, &root, after, limit)?;
                if page.sources.is_empty() {
                    return Ok(ControlFlow::Break(report(
                        &root,
                        &page,
                        &[],
                        false,
                        Some(CollectionStats::default()),
                    )));
                }
                Ok(ControlFlow::Continue((page, batches)))
            },
            |db, (page, batches)| {
                let stats = db.collect_candidates(&batches)?;
                Ok(report(&root, &page, &batches, false, Some(stats)))
            },
        )
    })()
    .map_err(|error: Box<dyn std::error::Error>| format!("sync incomplete: {error}"))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

/// Explicitly opt a registered source in or out of future sync operations.
pub fn set_sync_enabled(
    root: &Path,
    source: &str,
    enabled: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = validate_page(root, Some(source), 1, 1)?;
    let root_text = root.to_str().ok_or("project root must be UTF-8")?;
    let path = ProfileStore::db_path().ok_or("could not resolve profile store")?;
    mutate_private_store(
        &path,
        |db| {
            let selected = db
                .map(|db| db.collection_source(source))
                .transpose()?
                .flatten()
                .ok_or("source is not registered for this project root")?;
            if selected.project_root != root_text {
                return Err("source is not registered for this project root".into());
            }
            Ok(ControlFlow::Continue(()))
        },
        |db, ()| {
            db.set_source_sync_enabled(source, root_text, enabled)?;
            Ok(())
        },
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "source":source, "project_root":root, "sync_enabled":enabled,
            "note":"This controls future sync selection only. Retained observations, curation receipts and reviewed claims are unchanged. Explicit collect does not remove an exclusion."
        }))?
    );
    Ok(())
}
