//! Deterministic, rebuildable candidates from explicit CONTEXT.md decision entries.
//! Source-declared status is never treated as a review event.

use crate::context_doctor::ProseLine;
use crate::document_sections::DocumentSection;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const EXTRACTOR_VERSION: &str = "context-decision-log-v3";
const DECISION_PREFIX: &str = "- **Decision:**";
const STATUS_PREFIX: &str = "- **Status:**";
const STATEMENT_MAX_CHARS: usize = 400;

#[derive(Debug, Clone, Serialize)]
pub struct ProjectClaimCandidate {
    pub id: String,
    pub kind: String,
    pub statement: String,
    pub status: String,
    pub source_status: String,
    pub review_status: String,
    pub evidence_id: String,
    pub section_id: String,
    pub source_path: String,
    pub source_line: u32,
    pub source_citation: String,
    pub record_digest: String,
    pub source_file_digest: String,
    pub extractor_version: String,
}

#[derive(Default)]
pub struct ExtractedProjectClaims {
    pub candidates: Vec<ProjectClaimCandidate>,
    pub omitted: u32,
}

fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    crate::hex::encode(&hasher.finalize())
}

pub fn source_file_digest(body: &str) -> String {
    crate::hex::encode(&Sha256::digest(body.as_bytes()))
}

fn declared_status(value: &str) -> Option<String> {
    let value = value.to_ascii_lowercase();
    matches!(
        value.as_str(),
        "active" | "draft" | "proposed" | "deprecated" | "superseded"
    )
    .then_some(value)
}

fn heading_depth(text: &str) -> Option<usize> {
    let depth = text.bytes().take_while(|byte| *byte == b'#').count();
    ((1..=6).contains(&depth)
        && text
            .as_bytes()
            .get(depth)
            .is_some_and(u8::is_ascii_whitespace))
    .then_some(depth)
}

fn decision_record_headings(
    section: &DocumentSection,
    visible: &[ProseLine<'_>],
) -> Option<(String, String)> {
    if section.path != "CONTEXT.md" {
        return None;
    }
    let heading = visible
        .iter()
        .find(|line| line.index + 1 == section.start_line)?;
    if heading_depth(heading.text) != Some(3) {
        return None;
    }
    let parent = visible.iter().rfind(|line| {
        line.index < heading.index && heading_depth(line.text).is_some_and(|d| d <= 2)
    })?;
    if parent.text.trim() != "## Decision log" {
        return None;
    }
    let root_heading = visible
        .iter()
        .rfind(|line| line.index < parent.index && heading_depth(line.text) == Some(1))
        .map_or("", |line| line.text);
    Some((root_heading.into(), heading.text.into()))
}

/// Only a single explicit one-line `Decision` field per decision-log section
/// becomes a candidate. Other prose is left as source text for mmcg_docs.
pub fn extract(sections: &[DocumentSection], source_body: &str) -> ExtractedProjectClaims {
    let mut extracted = ExtractedProjectClaims::default();
    let mut seen_ids = HashSet::new();
    let visible = crate::context_doctor::prose_lines(source_body);
    let file_digest = source_file_digest(source_body);
    for section in sections {
        let Some((root_heading, record_heading)) = decision_record_headings(section, &visible)
        else {
            continue;
        };
        let mut decisions = Vec::new();
        let mut statuses = Vec::new();
        for line in visible.iter().filter(|line| {
            line.index >= section.start_line.saturating_sub(1) && line.index < section.end_line
        }) {
            if let Some(statement) = line.text.strip_prefix(DECISION_PREFIX) {
                if decisions.len() < 2 {
                    decisions.push((line, statement.trim()));
                }
            }
            if let Some(status) = line.text.strip_prefix(STATUS_PREFIX) {
                if statuses.len() < 2 {
                    statuses.push(status.trim());
                }
            }
        }
        if decisions.is_empty() {
            continue;
        }
        if decisions.len() != 1
            || decisions[0].1.is_empty()
            || decisions[0].1.chars().count() > STATEMENT_MAX_CHARS
            || statuses.len() != 1
            || declared_status(statuses[0]).is_none()
            || crate::indexer::secret_like_documentation(&section.body)
            || crate::indexer::secret_like_documentation(&section.heading)
        {
            extracted.omitted += 1;
            continue;
        }
        let (line, statement) = decisions[0];
        let source_line = (line.index + 1) as u32;
        let raw_line = line.raw.strip_suffix('\n').unwrap_or(line.raw);
        let record_digest = source_file_digest(raw_line);
        let claim_id = digest(&[
            EXTRACTOR_VERSION,
            &section.path,
            &root_heading,
            "## Decision log",
            &record_heading,
        ]);
        if !seen_ids.insert(claim_id.clone()) {
            let previous_count = extracted.candidates.len();
            extracted
                .candidates
                .retain(|candidate| candidate.id != claim_id);
            extracted.omitted += 1 + (previous_count - extracted.candidates.len()) as u32;
            continue;
        }
        let evidence_id = digest(&[
            "project-evidence-v2",
            &section.id,
            &source_line.to_string(),
            &record_digest,
            &file_digest,
        ]);
        extracted.candidates.push(ProjectClaimCandidate {
            id: claim_id,
            kind: "decision".into(),
            statement: statement.into(),
            status: "candidate".into(),
            source_status: declared_status(statuses[0]).expect("validated source status"),
            review_status: "unknown".into(),
            evidence_id,
            section_id: section.id.clone(),
            source_path: "CONTEXT.md".into(),
            source_line,
            source_citation: format!("CONTEXT.md:{source_line}"),
            record_digest,
            source_file_digest: file_digest.clone(),
            extractor_version: EXTRACTOR_VERSION.into(),
        });
    }
    extracted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ProjectHistoryEntry;

    #[test]
    fn explicit_decision_is_candidate_with_exact_line_and_separate_status() {
        let entry = ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body: "# Context\n## Decision log\n### 2026-01-01 — Storage\n- **Decision:** Keep Markdown authoritative.\n- **Status:** active\n## Other\n- **Decision:** Not a decision-log entry.\n".into(),
        };
        let sections = crate::document_sections::split(&entry).0;
        let result = extract(&sections, &entry.body);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(
            result.candidates[0].statement,
            "Keep Markdown authoritative."
        );
        assert_eq!(result.candidates[0].source_line, 4);
        assert_eq!(result.candidates[0].source_status, "active");
        assert_eq!(result.candidates[0].status, "candidate");
        assert_eq!(result.candidates[0].review_status, "unknown");
    }

    #[test]
    fn ambiguous_or_secret_like_decisions_are_omitted() {
        let entry = ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body: "## Decision log\n### One\n- **Decision:** First.\n- **Decision:** Second.\n### Two\n- **Decision:** api_key = \"sampleprivatevalue123\"\n".into(),
        };
        let sections = crate::document_sections::split(&entry).0;
        let result = extract(&sections, &entry.body);
        assert!(result.candidates.is_empty());
        assert_eq!(result.omitted, 2);
    }

    #[test]
    fn fenced_examples_do_not_create_decisions_and_crlf_digest_is_exact() {
        let entry = ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body: "## Decision log\r\n### One\r\n```md\r\n- **Decision:** Example only.\r\n```\r\n- **Decision:** Keep source bytes.\r\n- **Status:** active\r\n".into(),
        };
        let sections = crate::document_sections::split(&entry).0;
        let result = extract(&sections, &entry.body);
        assert_eq!(result.omitted, 0);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].source_line, 6);
        assert_eq!(
            result.candidates[0].record_digest,
            source_file_digest("- **Decision:** Keep source bytes.\r")
        );
    }

    #[test]
    fn duplicate_decision_heading_withholds_both_records() {
        let entry = ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body: "# Context\n## Decision log\n### Same\n- **Decision:** First.\n- **Status:** active\n### Same\n- **Decision:** Second.\n- **Status:** active\n".into(),
        };
        let sections = crate::document_sections::split(&entry).0;
        let result = extract(&sections, &entry.body);
        assert!(result.candidates.is_empty());
        assert_eq!(result.omitted, 2);
    }

    #[test]
    fn html_comments_and_indented_examples_do_not_become_candidates() {
        let entry = ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body: "# Context\n## Decision log\n<!--\n### Comment example\n- **Decision:** Hidden.\n- **Status:** active\n-->\n    ### Code example\n    - **Decision:** Hidden too.\n    - **Status:** active\n### Real\n- **Decision:** Visible.\n<!-- - **Status:** active -->\n- **Status:** proposed\n".into(),
        };
        let sections = crate::document_sections::split(&entry).0;
        let result = extract(&sections, &entry.body);
        assert_eq!(result.omitted, 0);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].statement, "Visible.");
        assert_eq!(result.candidates[0].source_status, "proposed");
        assert_eq!(result.candidates[0].source_line, 12);
    }

    #[test]
    fn hidden_heading_does_not_split_visible_decision_and_status() {
        let entry = ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body: "# Context\n## Decision log\n### Real\n- **Decision:** Visible.\n<!--\n### Fake\n-->\n- **Status:** active\n".into(),
        };
        let sections = crate::document_sections::split(&entry).0;
        let result = extract(&sections, &entry.body);
        assert_eq!(result.omitted, 0);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].statement, "Visible.");
        assert_eq!(result.candidates[0].source_status, "active");
        assert_eq!(result.candidates[0].source_line, 4);
    }

    #[test]
    fn inline_code_comment_marker_keeps_following_decision_visible() {
        let entry = ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body: "# Context\n## Decision log\nUse `<!--` in examples.\n### Real\n- **Decision:** Keep this decision.\n- **Status:** active\n".into(),
        };
        let sections = crate::document_sections::split(&entry).0;
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[2].heading, "Context > Decision log > Real");
        let result = extract(&sections, &entry.body);
        assert_eq!(result.omitted, 0);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].statement, "Keep this decision.");
        assert_eq!(result.candidates[0].source_line, 5);
    }

    #[test]
    fn multiline_code_and_html_examples_do_not_hide_or_create_claims() {
        let entry = ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body: "# Context\n## Decision log\nUse `literal\nwith <!-- marker\nend` here.\n<pre>\n### Fake\n- **Decision:** Do not use.\n- **Status:** active\n</pre>\n### Real\n- **Decision:** Keep this decision.\n- **Status:** active\n".into(),
        };
        let sections = crate::document_sections::split(&entry).0;
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[2].heading, "Context > Decision log > Real");
        let result = extract(&sections, &entry.body);
        assert_eq!(result.omitted, 0);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].statement, "Keep this decision.");
        assert_eq!(result.candidates[0].source_line, 12);
    }

    #[test]
    fn line_ending_revision_changes_evidence_id_but_not_claim_id() {
        let body = "# Context\n## Decision log\n### One\n- **Decision:** Keep bytes.\n- **Status:** active\n";
        let entry = |body: String| ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body,
        };
        let lf = entry(body.into());
        let crlf = entry(body.replace('\n', "\r\n"));
        let left = extract(&crate::document_sections::split(&lf).0, &lf.body);
        let right = extract(&crate::document_sections::split(&crlf).0, &crlf.body);
        assert_eq!(left.candidates.len(), 1);
        assert_eq!(right.candidates.len(), 1);
        assert_eq!(left.candidates[0].id, right.candidates[0].id);
        assert_ne!(
            left.candidates[0].evidence_id,
            right.candidates[0].evidence_id
        );
        assert_ne!(
            left.candidates[0].record_digest,
            right.candidates[0].record_digest
        );
    }

    #[test]
    fn cr_only_lines_keep_decision_citation_and_source_digest() {
        let entry = ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body: "# Context\r## Decision log\r### Real\r- **Decision:** Keep bytes.\r- **Status:** active\r".into(),
        };
        let sections = crate::document_sections::split(&entry).0;
        assert_eq!(sections.len(), 3);
        assert_eq!((sections[2].start_line, sections[2].end_line), (3, 5));
        let result = extract(&sections, &entry.body);
        assert_eq!(result.omitted, 0);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].source_line, 4);
        assert_eq!(
            result.candidates[0].record_digest,
            source_file_digest("- **Decision:** Keep bytes.\r")
        );
    }

    #[test]
    fn display_heading_separator_does_not_merge_record_ids() {
        let entry = ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body: "# A > Decision log\n## Decision log\n### B\n- **Decision:** First.\n- **Status:** active\n# A\n## Decision log\n### Decision log > B\n- **Decision:** Second.\n- **Status:** active\n".into(),
        };
        let sections = crate::document_sections::split(&entry).0;
        let result = extract(&sections, &entry.body);
        assert_eq!(result.omitted, 0);
        assert_eq!(result.candidates.len(), 2);
        assert_ne!(result.candidates[0].id, result.candidates[1].id);
    }
}
