//! Markdown lines and sections. `prose_lines` keeps the lines a reader sees,
//! outside code, quotes, comments and raw HTML. `split` builds rebuildable
//! section search units. Source files remain authoritative.

use crate::store::ProjectHistoryEntry;
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
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
    let lines: Vec<&str> = source_lines(&entry.body)
        .into_iter()
        .map(|line| line.raw.trim_end_matches(['\r', '\n']))
        .collect();
    if lines.is_empty() {
        return (Vec::new(), false);
    }
    // Use the same full-document visibility scan as CONTEXT parsing. A hidden
    // heading must not split a section or become part of its provenance.
    let visible: HashSet<usize> = prose_lines(&entry.body)
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

/// A visible Markdown line, retaining its exact original location and bytes.
pub(crate) struct ProseLine<'a> {
    pub(crate) offset: usize,
    pub(crate) index: usize,
    pub(crate) raw: &'a str,
    pub(crate) text: &'a str,
}

/// Keep original byte ranges while recognizing LF, CRLF, and CR-only lines.
pub(crate) fn source_lines(body: &str) -> Vec<ProseLine<'_>> {
    let bytes = body.as_bytes();
    let mut lines = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let start = offset;
        while offset < bytes.len() && !matches!(bytes[offset], b'\r' | b'\n') {
            offset += 1;
        }
        if bytes.get(offset) == Some(&b'\r') && bytes.get(offset + 1) == Some(&b'\n') {
            offset += 2;
        } else if offset < bytes.len() {
            offset += 1;
        }
        let raw = &body[start..offset];
        let content = raw.trim_end_matches(['\r', '\n']);
        let indent = content.bytes().take_while(|byte| *byte == b' ').count();
        lines.push(ProseLine {
            offset: start,
            index: lines.len(),
            raw,
            text: &content[indent..],
        });
    }
    lines
}

fn hide_span(starts: &[usize], delta: &mut [i32], start: usize, end: usize) {
    if start >= end || starts.is_empty() {
        return;
    }
    let first = starts
        .partition_point(|offset| *offset <= start)
        .saturating_sub(1);
    let last = starts
        .partition_point(|offset| *offset < end)
        .saturating_sub(1);
    delta[first] += 1;
    delta[last + 1] -= 1;
}

/// Preserve byte offsets while accepting CR-only lines and a tab after a
/// closing fence. The parser currently keeps such fences open.
fn parser_input(body: &str) -> String {
    let mut bytes = body.as_bytes().to_vec();
    for source in source_lines(body) {
        let raw = source.raw;
        if raw.ends_with('\r') {
            bytes[source.offset + raw.len() - 1] = b'\n';
        }
        let content = raw.trim_end_matches(['\r', '\n']);
        let line = content.as_bytes();
        let spaces = line.iter().take_while(|byte| **byte == b' ').count();
        if spaces <= 3 {
            let marker = line.get(spaces).copied();
            if matches!(marker, Some(b'`' | b'~')) {
                let width = line[spaces..]
                    .iter()
                    .take_while(|byte| Some(**byte) == marker)
                    .count();
                if width >= 3
                    && line[spaces + width..]
                        .iter()
                        .all(|byte| matches!(byte, b' ' | b'\t'))
                {
                    for index in spaces + width..line.len() {
                        if line[index] == b'\t' {
                            bytes[source.offset + index] = b' ';
                        }
                    }
                }
            }
        }
    }
    String::from_utf8(bytes).expect("only ASCII line breaks and tabs are replaced")
}

/// Parse the whole document before slicing into records. Byte ranges from the
/// Markdown parser keep comments, raw HTML, code, and quotes from creating
/// headings or decision fields. A comment anywhere on a line hides that line.
pub(crate) fn prose_lines(body: &str) -> Vec<ProseLine<'_>> {
    let lines = source_lines(body);
    let starts: Vec<usize> = lines.iter().map(|line| line.offset).collect();
    let mut delta = vec![0i32; lines.len() + 1];
    let mut quotes = Vec::new();
    let mut code_blocks = Vec::new();
    let mut html_blocks = Vec::new();
    let mut open_items = Vec::new();
    let mut items = Vec::new();
    let parsed = parser_input(body);
    for (event, range) in Parser::new(&parsed).into_offset_iter() {
        match event {
            Event::Start(Tag::BlockQuote(_)) => quotes.push(range.start),
            Event::End(TagEnd::BlockQuote(_)) => {
                if let Some(start) = quotes.pop() {
                    hide_span(&starts, &mut delta, start, range.end);
                }
            }
            Event::Start(Tag::CodeBlock(_)) => code_blocks.push(range.start),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(start) = code_blocks.pop() {
                    hide_span(&starts, &mut delta, start, range.end);
                }
            }
            Event::Start(Tag::Item) => open_items.push(range.start),
            Event::End(TagEnd::Item) => {
                if let Some(start) = open_items.pop() {
                    // The range can start at the previous line's ending or
                    // inside the marker's indentation.
                    let start = start + body[start..].len()
                        - body[start..]
                            .trim_start_matches(['\r', '\n', ' ', '\t'])
                            .len();
                    let line_start = body[..start].rfind(['\n', '\r']).map_or(0, |at| at + 1);
                    items.push((line_start, range.end, item_content_column(body, start)));
                }
            }
            Event::Start(Tag::HtmlBlock) => html_blocks.push(range.start),
            Event::End(TagEnd::HtmlBlock) => {
                if let Some(start) = html_blocks.pop() {
                    hide_span(&starts, &mut delta, start, range.end);
                }
            }
            Event::InlineHtml(html) if html.trim_start().starts_with("<!--") => {
                hide_span(&starts, &mut delta, range.start, range.end);
            }
            _ => {}
        }
    }
    let mut hidden = 0i32;
    lines
        .into_iter()
        .enumerate()
        .filter_map(|(index, line)| {
            hidden += delta[index];
            // Indentation counts from the innermost list item's content
            // column. More than three spaces past it reads as an example, as an
            // indented code block would.
            let column = items
                .iter()
                .filter(|(start, end, _)| *start <= line.offset && line.offset < *end)
                .max_by_key(|(start, _, _)| *start)
                .map(|(_, _, column)| *column);
            let content = line.raw.trim_end_matches(['\r', '\n']);
            let lead = content.len() - content.trim_start_matches([' ', '\t']).len();
            let code_indent = width(&content[..lead], 0) > column.unwrap_or(0) + 3;
            (hidden == 0 && !code_indent && !line.text.starts_with('>')).then_some(line)
        })
        .collect()
}

/// Column where the content of the list item whose marker starts at
/// `marker_at` begins: the marker, then one to four columns of space. An item
/// with nothing after its marker on that line starts one column past it.
fn item_content_column(body: &str, marker_at: usize) -> usize {
    let line_start = body[..marker_at].rfind(['\n', '\r']).map_or(0, |at| at + 1);
    let rest = &body[marker_at..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let marker_end = marker_at + (digits + 1).min(rest.len());
    let after = &body[marker_end..];
    let content = after.trim_start_matches([' ', '\t']);
    let space = &after[..after.len() - content.len()];
    let column = width(&body[line_start..marker_end], 0);
    let spaces = width(space, column) - column;
    let blank = content.is_empty() || content.starts_with(['\r', '\n']);
    column
        + if (1..=4).contains(&spaces) && !blank {
            spaces
        } else {
            1
        }
}

/// Column reached after `text` from `column`, with a tab to the next
/// multiple of four as in CommonMark.
pub(crate) fn width(text: &str, column: usize) -> usize {
    text.chars().fold(column, |column, character| {
        if character == '\t' {
            column + 4 - column % 4
        } else {
            column + 1
        }
    })
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

    #[test]
    fn prose_lines_preserve_offsets_and_reject_indented_fence_closers() {
        let body = "intro λ\r\n```markdown\r\n    ```\r\n## hidden-indented-close\r\n\t```\r\n## hidden-tab-close\r\n``\r\n## hidden-short-close\r\n``` trailing\r\n## hidden-suffixed-close\r\n   ```` \t\r\n<!--\r\n## hidden-comment\r\n--> <!--\r\n## hidden-second-comment\r\n-->\r\n> ## hidden-quote\r\n    ## hidden-code\r\n \t## hidden-mixed-code\r\n  ## live\r\n- **Status:** candidate\r\n~~~markdown\r\n## hidden-tilde\r\n~~~\r\n```html <!--\r\n## hidden-comment-in-info\r\n```\r\nlast";
        let visible = prose_lines(body);
        assert_eq!(
            visible.iter().map(|line| line.text).collect::<Vec<_>>(),
            [
                "intro λ",
                "## hidden-second-comment",
                "-->",
                "## live",
                "- **Status:** candidate",
                "last"
            ]
        );
        let heading = &visible[3];
        assert_eq!(heading.raw, "  ## live\r\n");
        assert_eq!(heading.offset, body.find("  ## live").unwrap());
        assert_eq!(heading.index, 19);
        for line in visible {
            assert_eq!(&body[line.offset..line.offset + line.raw.len()], line.raw);
        }
    }

    #[test]
    fn list_item_content_is_indented_from_its_content_column() {
        let body = "- item\n  continued\n      ## hidden-past-item-column\n\t\t## hidden-tab-past-item-column\n10. numbered\n    continued\n        ## hidden-past-numbered-column\n  - indented\n      - nested-under-indented\n-\ttabbed\n      - nested-under-tabbed\n- Group\n\t- Sub\n\t\t- Subsub\n\t\t\t- deepest\n\t\t\t\t\t## hidden-past-deepest\n\t\t\t\tdeepest-continued\n-   \n  blank-item\n       hidden-past-blank-item\n## live\n";
        let visible = prose_lines(body);
        assert_eq!(
            visible.iter().map(|line| line.text).collect::<Vec<_>>(),
            [
                "- item",
                "continued",
                "10. numbered",
                "continued",
                "- indented",
                "- nested-under-indented",
                "-\ttabbed",
                "- nested-under-tabbed",
                "- Group",
                "\t- Sub",
                "\t\t- Subsub",
                "\t\t\t- deepest",
                "\t\t\t\tdeepest-continued",
                "-   ",
                "blank-item",
                "## live"
            ]
        );
    }

    #[test]
    fn literal_comment_marker_in_inline_code_does_not_hide_following_lines() {
        let body = "## Decision log\nUse `<!--` in examples.\nUse ``<!--`` too.\nUse `<!--\\` too.\nUse \\<!-- as text.\n<!-- real comment --> `<!--`\n### Real\n- **Decision:** Keep the visible entry.\n- **Status:** active\n";
        let visible = prose_lines(body);
        assert_eq!(
            visible.iter().map(|line| line.text).collect::<Vec<_>>(),
            [
                "## Decision log",
                "Use `<!--` in examples.",
                "Use ``<!--`` too.",
                "Use `<!--\\` too.",
                "Use \\<!-- as text.",
                "### Real",
                "- **Decision:** Keep the visible entry.",
                "- **Status:** active"
            ]
        );
    }

    #[test]
    fn multiline_inline_code_and_raw_html_do_not_hide_or_create_decisions() {
        let body = "## Decision log\nUse `literal\nwith <!-- marker\nend` here.\n<pre>\n### Fake\n- **Decision:** Do not use.\n- **Status:** active\n</pre>\n### Real\n- **Decision:** Keep the visible entry.\n- **Status:** active\n";
        let visible = prose_lines(body);
        assert_eq!(
            visible.iter().map(|line| line.text).collect::<Vec<_>>(),
            [
                "## Decision log",
                "Use `literal",
                "with <!-- marker",
                "end` here.",
                "### Real",
                "- **Decision:** Keep the visible entry.",
                "- **Status:** active"
            ]
        );
    }

    #[test]
    fn many_comments_on_one_line_leave_next_heading_visible() {
        let body = format!("{}\n### Real\n", "<!--x-->".repeat(8_192));
        let visible = prose_lines(&body);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].text, "### Real");
        assert_eq!(visible[0].index, 1);
    }
}
