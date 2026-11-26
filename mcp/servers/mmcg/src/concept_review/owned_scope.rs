use crate::store::ConceptReviewSymbol;
use rusqlite::{params, Connection};
use std::collections::{BTreeSet, HashMap};

pub(super) const OWNER_LIMIT: usize = 2;
pub(super) const PER_OWNER_LIMIT: usize = 16;

pub(super) struct Expansion {
    pub(super) indexes: Vec<usize>,
    pub(super) truncated: bool,
}

/// A nominated file owner can expose a coordinator whose own vocabulary is
/// weaker than unrelated operations competing for the global source page.
pub(super) fn expand(
    scratch: &Connection,
    rows: &[ConceptReviewSymbol],
    selected: &[(usize, Vec<String>)],
    match_query: &str,
) -> Result<Expansion, String> {
    let failure = || "concept_review_operation_search_failed".to_owned();
    let mut owners = Vec::new();
    let mut owner_paths = BTreeSet::new();
    let mut selected_ids = BTreeSet::new();
    for (index, _) in selected {
        let row = rows
            .get(*index)
            .ok_or_else(|| "concept_review_inventory_changed".to_owned())?;
        selected_ids.insert(row.symbol.id);
        if row.eligible
            && row.symbol.name == "<module>"
            && row.symbol.kind == "module"
            && row.symbol.parent_id.is_none()
            && owner_paths.insert(row.symbol.file_path.as_str())
        {
            owners.push(row.symbol.file_path.as_str());
        }
    }
    let indexes_by_id = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (row.symbol.id, index))
        .collect::<HashMap<_, _>>();
    let mut statement = scratch
        .prepare(
            "SELECT rowid FROM operations
             WHERE operations MATCH ?1 AND path = ?2
               AND kind IN ('function', 'method')
             ORDER BY bm25(operations,4.0,1.0),path COLLATE BINARY,
                      CAST(line AS INTEGER),kind COLLATE BINARY,
                      name COLLATE BINARY,rowid
             LIMIT ?3",
        )
        .map_err(|_| failure())?;
    // The source table contains only eligible inventory rows. Reserving room
    // for every prior selection lets us skip them without hiding a new match.
    let scan_limit = selected_ids.len() + PER_OWNER_LIMIT + 1;
    let mut expansion = Expansion {
        indexes: Vec::new(),
        truncated: false,
    };
    let mut added_ids = BTreeSet::new();
    for (owner_position, path) in owners.into_iter().enumerate() {
        let matches = statement
            .query_map(params![match_query, path, scan_limit as u32], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|_| failure())?;
        let mut admitted = 0usize;
        for id in matches {
            let id = id.map_err(|_| failure())?;
            let index = *indexes_by_id
                .get(&id)
                .ok_or_else(|| "concept_review_inventory_changed".to_owned())?;
            let row = &rows[index];
            if !row.eligible
                || row.symbol.file_path != path
                || !matches!(row.symbol.kind.as_str(), "function" | "method")
                || selected_ids.contains(&id)
                || !added_ids.insert(id)
            {
                continue;
            }
            if owner_position >= OWNER_LIMIT || admitted == PER_OWNER_LIMIT {
                expansion.truncated = true;
                break;
            }
            expansion.indexes.push(index);
            admitted += 1;
        }
        if owner_position >= OWNER_LIMIT && expansion.truncated {
            break;
        }
    }
    Ok(expansion)
}

#[cfg(test)]
mod tests {
    use super::super::prepare_with_contract;
    use super::{expand, OWNER_LIMIT, PER_OWNER_LIMIT};
    use crate::indexer::Indexer;
    use crate::store::Store;
    use rusqlite::{params, Connection};

    #[test]
    fn owner_and_candidate_caps_disclose_omissions_without_counting_prior_selections() {
        let dir = tempfile::tempdir().unwrap();
        for owner in ["a", "b", "c"] {
            let source = (0..20)
                .map(|index| format!("fn {owner}_{index:02}() {{}}\n"))
                .collect::<String>();
            std::fs::write(dir.path().join(format!("{owner}.rs")), source).unwrap();
        }
        let mut store = Store::open(dir.path().join("graph.db")).unwrap();
        Indexer::new(dir.path())
            .index_all(&mut store, true)
            .unwrap();
        let (_, rows) = store.concept_review_symbols(false, 20_000).unwrap();
        let scratch = Connection::open_in_memory().unwrap();
        scratch
            .execute_batch(
                "CREATE VIRTUAL TABLE operations USING fts5(
                     metadata,body,path UNINDEXED,line UNINDEXED,
                     kind UNINDEXED,name UNINDEXED)",
            )
            .unwrap();
        for row in rows.iter().filter(|row| row.eligible) {
            scratch
                .execute(
                    "INSERT INTO operations(rowid,metadata,body,path,line,kind,name)
                     VALUES(?1,'needle','',?2,?3,?4,?5)",
                    params![
                        row.symbol.id,
                        row.symbol.file_path,
                        row.symbol.line_start,
                        row.symbol.kind,
                        row.symbol.name
                    ],
                )
                .unwrap();
        }
        let mut selected = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.symbol.name == "<module>")
            .map(|(index, _)| (index, vec!["lexical".to_owned()]))
            .collect::<Vec<_>>();
        let prior = rows
            .iter()
            .position(|row| row.symbol.name == "a_00")
            .unwrap();
        selected.push((prior, vec!["operation_source".to_owned()]));
        selected.push(selected[0].clone());
        let expansion = expand(&scratch, &rows, &selected, "needle").unwrap();
        assert_eq!(expansion.indexes.len(), OWNER_LIMIT * PER_OWNER_LIMIT);
        assert!(expansion.truncated);
        assert!(!expansion.indexes.contains(&prior));
        assert!(expansion
            .indexes
            .iter()
            .all(|index| { matches!(rows[*index].symbol.file_path.as_str(), "a.rs" | "b.rs") }));
        selected.extend(
            rows.iter()
                .enumerate()
                .filter(|(_, row)| row.symbol.kind == "function")
                .map(|(index, _)| (index, vec!["operation_source".to_owned()])),
        );
        let complete = expand(&scratch, &rows, &selected, "needle").unwrap();
        assert!(complete.indexes.is_empty());
        assert!(!complete.truncated);
    }

    #[test]
    fn nominated_owner_recovers_a_coordinator_beyond_the_global_source_page() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("owner.rs"),
            "//! Assemble a blank current evidence review packet for the held task.\n\
             pub fn coordinate() -> &'static str {\n\
                 let state = \"held\";\n\
                 state\n\
             }\n\
             #[cfg(test)]\n\
             mod tests {\n\
                 #[test]\n\
                 fn test_coordinator() { let _ = \"held\"; }\n\
             }\n",
        )
        .unwrap();
        let mut competitors = String::new();
        for index in 0..70 {
            competitors.push_str(&format!(
                "fn competing_{index:02}() {{ let _ = \"blank current evidence review packet held task\"; }}\n"
            ));
        }
        std::fs::write(dir.path().join("competitors.rs"), competitors).unwrap();
        std::fs::write(
            dir.path().join("elsewhere.rs"),
            "pub fn unrelated_coordinator() -> &'static str { \"held\" }\n",
        )
        .unwrap();
        let mut store = Store::open(dir.path().join("graph.db")).unwrap();
        Indexer::new(dir.path())
            .index_all(&mut store, true)
            .unwrap();
        let packet = prepare_with_contract(
            &store,
            dir.path(),
            "blank current evidence review packet held task",
            5,
            false,
            &[],
            None,
        )
        .unwrap();
        let owner = packet
            .candidates
            .iter()
            .find(|candidate| candidate.path == "owner.rs" && candidate.name == "<module>")
            .unwrap();
        assert!(owner
            .retrieval_sources
            .iter()
            .any(|source| source == "lexical"));
        let coordinator = packet
            .candidates
            .iter()
            .find(|candidate| candidate.path == "owner.rs" && candidate.name == "coordinate")
            .unwrap();
        assert_eq!(coordinator.retrieval_sources, vec!["owned_scope"]);
        assert_eq!(
            packet
                .candidates
                .iter()
                .filter(|candidate| candidate
                    .retrieval_sources
                    .iter()
                    .any(|source| source == "operation_source"))
                .count(),
            64
        );
        for index in 0..63 {
            assert!(packet.candidates.iter().any(|candidate| {
                candidate.path == "competitors.rs"
                    && candidate.name == format!("competing_{index:02}")
                    && candidate
                        .retrieval_sources
                        .iter()
                        .any(|source| source == "operation_source")
            }));
        }
        assert!(!packet.candidates.iter().any(|candidate| {
            matches!(
                candidate.name.as_str(),
                "test_coordinator" | "unrelated_coordinator"
            )
        }));
    }
}
