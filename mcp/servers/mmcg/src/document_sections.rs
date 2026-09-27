//! Rebuildable Markdown section search units. Source files remain authoritative.

use crate::store::ProjectHistoryEntry;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const MAX_SECTIONS_PER_DOCUMENT: usize = 128;
pub const MAX_SECTIONS_TOTAL: usize = 20_000;
pub const EXTRACTOR_VERSION: &str = "markdown-sections-v3";

pub struct DocumentSection {
    pub path: String,
    pub kind: String,
    pub id: String,
    pub heading: String,
    pub start_line: usize,
    pub end_line: usize,
    pub body: String,
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let count = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&count) || !trimmed.as_bytes().get(count)?.is_ascii_whitespace() {
        return None;
    }
    let text = trimmed[count..].trim().trim_end_matches('#').trim();
    (!text.is_empty()).then_some((count, text))
}

fn section(
    entry: &ProjectHistoryEntry,
    path: &[String],
    start: usize,
    end: usize,
    lines: &[&str],
) -> DocumentSection {
    let heading = if path.is_empty() {
        entry.title.clone()
    } else {
        path.join(" > ")
    };
    let body = lines[start - 1..end].join("\n");
    let mut digest = Sha256::new();
    digest.update(entry.path.as_bytes());
    digest.update([0]);
    digest.update(heading.as_bytes());
    digest.update([0]);
    digest.update(start.to_le_bytes());
    digest.update(body.as_bytes());
    let id = crate::hex::encode(&digest.finalize());
    DocumentSection {
        path: entry.path.clone(),
        kind: entry.kind.clone(),
        id,
        heading,
        start_line: start,
        end_line: end,
        body,
    }
}

/// Split at ATX headings outside fenced code. `truncated` means later headings
/// were folded into the final section instead of creating unbounded FTS rows.
pub fn split(entry: &ProjectHistoryEntry) -> (Vec<DocumentSection>, bool) {
    let lines: Vec<&str> = crate::context_doctor::source_lines(&entry.body)
        .into_iter()
        .map(|line| line.raw.trim_end_matches(['\r', '\n']))
        .collect();
    if lines.is_empty() {
        return (Vec::new(), false);
    }
    // Use the same full-document visibility scan as CONTEXT parsing. A hidden
    // heading must not split a section or become part of its provenance.
    let visible: HashSet<usize> = crate::context_doctor::prose_lines(&entry.body)
        .into_iter()
        .map(|line| line.index)
        .collect();
    let mut out = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut start = 1usize;
    let mut truncated = false;
    for (index, line) in lines.iter().enumerate() {
        if !visible.contains(&index) {
            continue;
        }
        let Some((depth, title)) = heading(line) else {
            continue;
        };
        let line_number = index + 1;
        if line_number > start {
            if out.len() >= MAX_SECTIONS_PER_DOCUMENT - 1 {
                truncated = true;
                break;
            }
            out.push(section(entry, &path, start, line_number - 1, &lines));
        }
        path.truncate(depth - 1);
        path.push(title.to_string());
        start = line_number;
    }
    out.push(section(entry, &path, start, lines.len(), &lines));
    (out, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_keep_exact_lines_and_ignore_code_fences() {
        let entry = ProjectHistoryEntry {
            path: "docs/guide.md".into(),
            kind: "documentation".into(),
            title: "Guide".into(),
            body: "intro\n# Setup\nfirst\n```md\n# not a heading\n```\n## Runtime\nsecond\n".into(),
        };
        let (sections, truncated) = split(&entry);
        assert!(!truncated);
        assert_eq!(sections.len(), 3);
        assert_eq!((sections[0].start_line, sections[0].end_line), (1, 1));
        assert_eq!((sections[1].start_line, sections[1].end_line), (2, 6));
        assert_eq!(sections[2].heading, "Setup > Runtime");
        assert_eq!((sections[2].start_line, sections[2].end_line), (7, 8));
        assert_eq!(sections[2].id, split(&entry).0[2].id);
    }

    #[test]
    fn comments_indented_code_and_quotes_do_not_split_sections() {
        let entry = ProjectHistoryEntry {
            path: "CONTEXT.md".into(),
            kind: "context".into(),
            title: "Context".into(),
            body: "# Root\n## Decision log\n<!--\n### Fake\n-->\n    ### Code\n> ### Quote\n### Real\n- **Decision:** Visible.\n- **Status:** active\n".into(),
        };
        let (sections, truncated) = split(&entry);
        assert!(!truncated);
        assert_eq!(sections.len(), 3);
        assert_eq!((sections[1].start_line, sections[1].end_line), (2, 7));
        assert_eq!(sections[2].heading, "Root > Decision log > Real");
        assert_eq!((sections[2].start_line, sections[2].end_line), (8, 10));
    }
}
