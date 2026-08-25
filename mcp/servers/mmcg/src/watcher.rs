//! Filesystem watcher — keeps the index fresh as files change.
//!
//! Architecture:
//!   1. Initial full index of `root`
//!   2. Subscribe to recursive filesystem events under `root`
//!   3. Coalesce rapid-fire events per path with a 500ms debounce
//!   4. Re-index files on Modify/Create, purge on Remove
//!
//! Runs in the foreground until stdin closes or Ctrl-C.

use crate::indexer::{extractor_for_path, IndexError, Indexer, SourceMatcher};
use crate::store::Store;
use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(500);
const RX_TIMEOUT: Duration = Duration::from_millis(100);

pub fn run(root: PathBuf, mut store: Store) -> Result<(), Box<dyn std::error::Error>> {
    let root = root.canonicalize()?;
    let indexer = Indexer::new(&root);
    let mut source_matcher = SourceMatcher::new(&root);

    // Initial pass — bring the index current before watching. Incremental:
    // only re-parses files whose mtime is newer than the stored index.
    let stats = indexer.index_all(&mut store, false)?;
    eprintln!(
        "[mastermind watch] initial: indexed {} (unchanged {}, purged {}) in {} ms",
        stats.files_indexed, stats.files_unchanged, stats.files_purged, stats.duration_ms
    );
    eprintln!("[mastermind watch] watching {}", root.display());

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(tx)?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    let mut pending_changes: HashMap<PathBuf, Instant> = HashMap::new();

    loop {
        // Short timeout so we can also flush pending changes between events.
        match rx.recv_timeout(RX_TIMEOUT) {
            Ok(Ok(event)) => handle_event(
                event,
                &root,
                &indexer,
                &mut source_matcher,
                &mut pending_changes,
                &mut store,
            ),
            Ok(Err(e)) => eprintln!("[mastermind watch] notify error: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("[mastermind watch] watcher channel closed");
                break;
            }
        }

        flush_settled(&indexer, &mut store, &mut pending_changes);
    }

    Ok(())
}

fn handle_event(
    event: notify::Event,
    root: &Path,
    indexer: &Indexer,
    source_matcher: &mut SourceMatcher,
    pending: &mut HashMap<PathBuf, Instant>,
    store: &mut Store,
) {
    if event
        .paths
        .iter()
        .any(|path| is_project_history_path(path, root))
    {
        match indexer.index_project_history(store) {
            Ok(stats) => eprintln!(
                "[mastermind watch] refreshed {} history entries (skipped {}, truncated {})",
                stats.indexed, stats.skipped, stats.truncated
            ),
            Err(error) => eprintln!("[mastermind watch] history refresh failed: {error}"),
        }
    }

    if event.paths.iter().any(|path| is_ignore_config(path, root)) {
        *source_matcher = SourceMatcher::new(root);
        pending.clear();
        match indexer.index_all(store, false) {
            Ok(stats) => eprintln!(
                "[mastermind watch] ignore rules changed: indexed {} (unchanged {}, purged {})",
                stats.files_indexed, stats.files_unchanged, stats.files_purged
            ),
            Err(error) => eprintln!("[mastermind watch] ignore-rule refresh failed: {error}"),
        }
        return;
    }

    match event.kind {
        EventKind::Modify(ModifyKind::Name(rename_mode)) => {
            let paths = event.paths;
            let indexed_paths = match store.indexed_paths() {
                Ok(paths) => paths,
                Err(error) => {
                    eprintln!("[mastermind watch] rename inventory failed: {error}");
                    Vec::new()
                }
            };
            let mut purge_paths = Vec::new();
            let mut full_refresh = indexed_paths.is_empty()
                || matches!(
                    rename_mode,
                    RenameMode::To | RenameMode::Any | RenameMode::Other
                );

            for (position, path) in paths.iter().enumerate() {
                pending.remove(path);
                if path.is_dir() {
                    full_refresh = true;
                }
                if path.exists() && !rename_event_path_is_old(&rename_mode, position) {
                    continue;
                }
                if let Some(rel) = relative_path(path, root) {
                    let descendant_prefix = format!("{rel}/");
                    let mut matched = false;
                    for indexed in &indexed_paths {
                        if indexed == &rel || indexed.starts_with(&descendant_prefix) {
                            if indexed.starts_with(&descendant_prefix) {
                                full_refresh = true;
                            }
                            purge_paths.push(indexed.clone());
                            matched = true;
                        }
                    }
                    if !matched {
                        purge_paths.push(rel);
                    }
                }
            }
            purge_paths.sort();
            purge_paths.dedup();
            for rel in purge_paths {
                if let Err(error) = store.purge_file(&rel) {
                    eprintln!("[mastermind watch] rename purge failed {rel}: {error}");
                    full_refresh = true;
                } else {
                    eprintln!("[mastermind watch] rename purged {rel}");
                }
            }

            if full_refresh {
                pending.clear();
                match indexer.index_all(store, false) {
                    Ok(stats) => eprintln!(
                        "[mastermind watch] rename refresh: indexed {} (unchanged {}, purged {})",
                        stats.files_indexed, stats.files_unchanged, stats.files_purged
                    ),
                    Err(error) => eprintln!("[mastermind watch] rename refresh failed: {error}"),
                }
                return;
            }

            for (position, path) in paths.into_iter().enumerate() {
                if rename_event_path_is_old(&rename_mode, position) {
                    continue;
                }
                if path.is_file()
                    && extractor_for_path(&path).is_some()
                    && !source_matcher.is_ignored(&path, false)
                {
                    pending.insert(path, Instant::now());
                }
            }
        }
        EventKind::Modify(_) | EventKind::Create(_) => {
            for path in event.paths {
                if path.is_file()
                    && extractor_for_path(&path).is_some()
                    && !source_matcher.is_ignored(&path, false)
                {
                    pending.insert(path, Instant::now());
                }
            }
        }
        EventKind::Remove(_) => {
            let indexed_paths = match store.indexed_paths() {
                Ok(paths) => paths,
                Err(error) => {
                    eprintln!("[mastermind watch] removal inventory failed: {error}");
                    match indexer.index_all(store, false) {
                        Ok(_) => {}
                        Err(refresh_error) => {
                            eprintln!("[mastermind watch] removal refresh failed: {refresh_error}");
                        }
                    }
                    return;
                }
            };
            let mut purge_paths = Vec::new();
            for path in event.paths {
                // Drop descendants too — a directory remove otherwise leaves
                // debounced child paths able to re-enter the index.
                pending.retain(|candidate, _| !candidate.starts_with(&path));
                if let Some(rel) = relative_path(&path, root) {
                    let descendant_prefix = format!("{rel}/");
                    let mut matched = false;
                    for indexed in &indexed_paths {
                        if indexed == &rel || indexed.starts_with(&descendant_prefix) {
                            purge_paths.push(indexed.clone());
                            matched = true;
                        }
                    }
                    if !matched {
                        purge_paths.push(rel);
                    }
                }
            }
            purge_paths.sort();
            purge_paths.dedup();
            for rel in purge_paths {
                if let Err(error) = store.purge_file(&rel) {
                    eprintln!("[mastermind watch] purge failed {rel}: {error}");
                } else {
                    eprintln!("[mastermind watch] purged {rel}");
                }
            }
        }
        _ => {}
    }
}

fn flush_settled(indexer: &Indexer, store: &mut Store, pending: &mut HashMap<PathBuf, Instant>) {
    let now = Instant::now();
    let ready: Vec<PathBuf> = pending
        .iter()
        .filter(|(_, t)| now.duration_since(**t) >= DEBOUNCE)
        .map(|(p, _)| p.clone())
        .collect();

    for path in ready {
        pending.remove(&path);
        if !path.is_file() {
            continue;
        }
        match indexer.index_one(store, &path) {
            Ok(()) => eprintln!("[mastermind watch] reindexed {}", path.display()),
            Err(IndexError::Skipped(reason)) => {
                eprintln!("[mastermind watch] skipped {}: {reason:?}", path.display())
            }
            Err(e) => eprintln!("[mastermind watch] failed {}: {e}", path.display()),
        }
    }
}

fn relative_path(absolute: &Path, root: &Path) -> Option<String> {
    absolute
        .strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

fn rename_event_path_is_old(mode: &RenameMode, position: usize) -> bool {
    matches!(mode, &RenameMode::From) || matches!(mode, &RenameMode::Both) && position == 0
}

fn is_ignore_config(path: &Path, root: &Path) -> bool {
    if path
        .file_name()
        .is_some_and(|name| name == ".gitignore" || name == ".ignore")
    {
        return path.starts_with(root);
    }
    path == root.join(".git/info/exclude")
}

fn is_project_history_path(path: &Path, root: &Path) -> bool {
    let is_context = path == root.join("CONTEXT.md")
        || (path.parent() == Some(root)
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("CONTEXT-archive-") && name.ends_with(".md")));
    is_context
        || path.starts_with(root.join(".mastermind").join("tasks"))
        || path.starts_with(root.join(".mastermind").join("releases"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_ignore_rules_rebuilds_matcher_and_purges_newly_ignored_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("ignored.rs");
        std::fs::write(&source, "pub fn indexed() {}\n").unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        assert_eq!(store.indexed_paths().unwrap(), vec!["ignored.rs"]);

        let mut matcher = SourceMatcher::new(&root);
        let mut pending = HashMap::new();
        let ignore_file = root.join(".gitignore");
        std::fs::write(&ignore_file, "ignored.rs\n").unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Any)).add_path(ignore_file);

        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        );

        assert!(pending.is_empty());
        assert!(matcher.is_ignored(&source, false));
        assert!(store.indexed_paths().unwrap().is_empty());
    }

    #[test]
    fn history_markdown_events_refresh_the_derived_corpus() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".mastermind/tasks")).unwrap();
        let context = root.join("CONTEXT.md");
        std::fs::write(&context, "# Context\n\nInitial decision.\n").unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();

        std::fs::write(&context, "# Context\n\nUse durable idempotency keys.\n").unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Any)).add_path(context);
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = HashMap::new();
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        );

        assert_eq!(
            store
                .search_project_history("idempotency", Some("context"), 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn rename_event_purges_old_graph_and_concept_path_before_reindexing_new_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        let old_path = root.join("old_handler.rs");
        let new_path = root.join("new_handler.rs");
        std::fs::write(&old_path, "pub fn renamed_handler() {}\n").unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        assert_eq!(store.search_concepts("\"old\"", 10).unwrap().len(), 2);

        std::fs::rename(&old_path, &new_path).unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Name(
            notify::event::RenameMode::Both,
        )))
        .add_path(old_path.clone())
        .add_path(new_path.clone());
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = HashMap::new();
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        );

        assert!(store.symbols_in_file("old_handler.rs").unwrap().is_empty());
        assert!(store.search_concepts("\"old\"", 10).unwrap().is_empty());
        assert!(pending.contains_key(&new_path));
        pending.insert(new_path, Instant::now() - DEBOUNCE);
        flush_settled(&indexer, &mut store, &mut pending);

        assert_eq!(store.symbols_in_file("new_handler.rs").unwrap().len(), 2);
        assert_eq!(store.search_concepts("\"new\"", 10).unwrap().len(), 2);
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn directory_rename_purges_all_old_paths_and_indexes_the_new_tree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("old/nested")).unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        std::fs::write(root.join("old/first.rs"), "pub fn first_handler() {}\n").unwrap();
        std::fs::write(
            root.join("old/nested/second.py"),
            "def second_handler(): pass\n",
        )
        .unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();

        let old_path = root.join("old");
        let new_path = root.join("new");
        std::fs::rename(&old_path, &new_path).unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Name(
            notify::event::RenameMode::Both,
        )))
        .add_path(old_path)
        .add_path(new_path);
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = HashMap::new();
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        );

        assert!(store.symbols_in_file("old/first.rs").unwrap().is_empty());
        assert!(store
            .symbols_in_file("old/nested/second.py")
            .unwrap()
            .is_empty());
        assert_eq!(store.symbols_in_file("new/first.rs").unwrap().len(), 2);
        assert_eq!(
            store.symbols_in_file("new/nested/second.py").unwrap().len(),
            2
        );
        assert!(pending.is_empty());
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn rename_to_event_runs_inventory_refresh_when_the_old_path_is_absent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        let old_path = root.join("old.rs");
        let new_path = root.join("new.rs");
        std::fs::write(&old_path, "pub fn moved_handler() {}\n").unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();

        std::fs::rename(&old_path, &new_path).unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::To)))
            .add_path(new_path);
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = HashMap::new();
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        );

        assert!(store.symbols_in_file("old.rs").unwrap().is_empty());
        assert_eq!(store.symbols_in_file("new.rs").unwrap().len(), 2);
        assert!(pending.is_empty());
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn rename_mode_positions_identify_case_only_old_paths_without_stat() {
        assert!(rename_event_path_is_old(&RenameMode::From, 0));
        assert!(rename_event_path_is_old(&RenameMode::Both, 0));
        assert!(!rename_event_path_is_old(&RenameMode::Both, 1));
        assert!(!rename_event_path_is_old(&RenameMode::To, 0));
    }

    #[test]
    fn case_only_rename_never_requeues_the_old_spelling_when_it_still_stats() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        let old_path = root.join("Handler.rs");
        let new_path = root.join("handler.rs");
        std::fs::write(&old_path, "pub fn renamed_handler() {}\n").unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();

        // Keep both spellings present to reproduce the observable stat result
        // of a case-insensitive filesystem without making the test platform-specific.
        std::fs::write(&new_path, "pub fn renamed_handler() {}\n").unwrap();
        let event = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(old_path.clone())
            .add_path(new_path.clone());
        let mut matcher = SourceMatcher::new(&root);
        let mut pending = HashMap::new();
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        );

        assert!(old_path.is_file());
        assert!(!pending.contains_key(&old_path));
        assert!(pending.contains_key(&new_path));
        pending.insert(new_path, Instant::now() - DEBOUNCE);
        flush_settled(&indexer, &mut store, &mut pending);
        assert!(store.symbols_in_file("Handler.rs").unwrap().is_empty());
        assert_eq!(store.symbols_in_file("handler.rs").unwrap().len(), 2);
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn directory_remove_event_purges_descendant_graph_and_concept_rows() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let removed = root.join("removed");
        std::fs::create_dir_all(removed.join("nested")).unwrap();
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        std::fs::write(removed.join("first.rs"), "pub fn first_handler() {}\n").unwrap();
        std::fs::write(
            removed.join("nested/second.py"),
            "def second_handler(): pass\n",
        )
        .unwrap();
        let mut store = Store::open(root.join(".mastermind/mmcg.db")).unwrap();
        let indexer = Indexer::new(&root);
        indexer.index_all(&mut store, false).unwrap();
        assert_eq!(store.concept_count().unwrap(), 4);

        std::fs::remove_dir_all(&removed).unwrap();
        let event = notify::Event::new(EventKind::Remove(notify::event::RemoveKind::Folder))
            .add_path(removed.clone());
        let mut matcher = SourceMatcher::new(&root);
        let mut pending =
            HashMap::from([(removed.join("nested/second.py"), Instant::now() - DEBOUNCE)]);
        handle_event(
            event,
            &root,
            &indexer,
            &mut matcher,
            &mut pending,
            &mut store,
        );

        assert!(store.indexed_paths().unwrap().is_empty());
        assert_eq!(store.concept_count().unwrap(), 0);
        assert!(pending.is_empty());
        assert!(store.concept_contract_current().unwrap());
    }

    #[test]
    fn release_and_context_archive_are_history_paths() {
        let root = Path::new("/repo");
        assert!(is_project_history_path(
            Path::new("/repo/CONTEXT-archive-2025.md"),
            root
        ));
        assert!(is_project_history_path(
            Path::new("/repo/.mastermind/releases/001-task.md"),
            root
        ));
        assert!(!is_project_history_path(
            Path::new("/repo/notes/CONTEXT-archive-2025.md"),
            root
        ));
    }
}
