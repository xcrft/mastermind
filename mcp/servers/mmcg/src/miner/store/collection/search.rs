//! Bounded metadata search. Transcript reads happen only after page selection.

use super::{candidate_row, CollectedCandidate, ProfileStore, SqlResult};
use rusqlite::params;
use serde_json::{json, Value};

type SearchPage = (Vec<CollectedCandidate>, usize, Option<String>);

impl ProfileStore {
    pub(crate) fn search_collected_candidates(
        &self,
        query: &str,
        root: Option<&str>,
        source: Option<&str>,
        status: &str,
        after: &str,
        limit: usize,
    ) -> SqlResult<SearchPage> {
        if !self.has_table("persona_candidate")? {
            return Ok((Vec::new(), 0, None));
        }
        let mut stmt = self.conn.prepare(
            "SELECT id, source, kind, quote, source_path, line_no, segment_no, record_digest,
                    project_root, project, observed_at, extractor, rule_id, revision, status, present
             FROM persona_candidate WHERE id > ?1 AND (?2 = 'all' OR status = ?2)
               AND (?3 IS NULL OR project_root = ?3) AND (?4 IS NULL OR source = ?4)
             ORDER BY id LIMIT 10001",
        )?;
        let rows = stmt.query_map(params![after, status, root, source], candidate_row)?;
        let mut found = Vec::new();
        let mut examined = 0;
        let mut last_examined = None;
        for row in rows {
            let row = row?;
            if examined == 10_000 {
                return Ok((found, examined, last_examined));
            }
            examined += 1;
            last_examined = Some(row.id.clone());
            // SQLite's built-in lower() is ASCII-only. Apply Unicode lowercasing
            // to complete bounded quotes; %, _, quotes and FTS syntax are literal.
            if row.quote.to_lowercase().contains(query) {
                if found.len() == limit {
                    let cursor = found.last().map(|c: &CollectedCandidate| c.id.clone());
                    return Ok((found, examined, cursor));
                }
                found.push(row);
            }
        }
        Ok((found, examined, None))
    }

    pub(crate) fn candidate_claim_links(&self, id: &str) -> SqlResult<Value> {
        let mut selects = Vec::new();
        if self.has_table("persona_candidate_feedback")? {
            selects.push("SELECT 'preference' AS kind, feedback_key AS id, revision FROM persona_candidate_feedback WHERE candidate_id=?1");
        }
        if self.has_table("persona_candidate_habit")? {
            selects.push("SELECT 'habit' AS kind, CAST(claim_id AS TEXT) AS id, revision FROM persona_candidate_habit WHERE candidate_id=?1");
        }
        if selects.is_empty() {
            return Ok(json!({"items": [], "truncated": false}));
        }
        let sql = format!(
            "{} ORDER BY kind, id, revision LIMIT 9",
            selects.join(" UNION ALL ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([id], |row| {
            Ok(json!({
                "kind": row.get::<_, String>(0)?, "id": row.get::<_, String>(1)?,
                "candidate_revision": row.get::<_, String>(2)?
            }))
        })?;
        let mut items = rows.collect::<SqlResult<Vec<_>>>()?;
        let truncated = items.len() > 8;
        items.truncate(8);
        Ok(
            json!({"items": items, "truncated": truncated, "note": "Retained proposal receipts; links do not imply current review or source verification."}),
        )
    }
}
