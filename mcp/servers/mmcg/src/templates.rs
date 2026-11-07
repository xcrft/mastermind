//! Compile-time template constants and utilities.
//!
//! Templates are embedded via `include_str!` so the published crate is
//! self-contained. `scripts/validate.py` enforces parity between the
//! crate-local copies and the repository sources.

/// Generic CONTEXT.md scaffold (stack-agnostic).
pub const CONTEXT_TEMPLATE: &str = include_str!("../templates/context.md");

/// Mastermind workflow CLAUDE.md template.
pub const WORKFLOW_TEMPLATE: &str = include_str!("../templates/workflow.md");

/// Markers around the workflow section that `init` owns in CLAUDE.md and
/// AGENTS.md. Text outside them belongs to the project.
pub const WORKFLOW_BLOCK_BEGIN: &str =
    "<!-- mastermind:workflow:begin (managed by `mastermind init`, edits inside are replaced) -->";
pub const WORKFLOW_BLOCK_END: &str = "<!-- mastermind:workflow:end -->";
const WORKFLOW_HEADING: &str = "## Mastermind workflow";

/// Template text after its frontmatter.
fn workflow_template_body() -> &'static str {
    WORKFLOW_TEMPLATE
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(WORKFLOW_TEMPLATE, |(_, body)| body)
        .trim_start()
}

fn workflow_section() -> &'static str {
    let body = workflow_template_body();
    let start = body
        .find(WORKFLOW_HEADING)
        .expect("workflow template has the workflow heading");
    body[start..].trim_end()
}

/// The managed block, markers included.
pub fn workflow_block() -> String {
    format!(
        "{WORKFLOW_BLOCK_BEGIN}\n{}\n{WORKFLOW_BLOCK_END}",
        workflow_section()
    )
}

/// A new instructions file: the project header with placeholders, then the
/// managed block.
pub fn new_instructions_file() -> String {
    let body = workflow_template_body();
    let header = body[..body.find(WORKFLOW_HEADING).expect("workflow heading")].trim_end();
    format!("{header}\n\n{}\n", workflow_block())
}

#[derive(Debug, PartialEq, Eq)]
pub enum WorkflowBlock {
    /// No markers. `legacy` is true when the text still has the workflow
    /// heading that older `init` versions wrote without markers.
    Missing {
        legacy: bool,
    },
    Current,
    Outdated,
}

fn block_span(text: &str) -> Result<Option<(usize, usize)>, String> {
    let begin = text.find(WORKFLOW_BLOCK_BEGIN);
    let end = text.find(WORKFLOW_BLOCK_END);
    let duplicate = |marker: &str, at: usize| text[at + marker.len()..].contains(marker);
    match (begin, end) {
        (None, None) => Ok(None),
        (Some(begin), Some(end))
            if begin < end
                && !duplicate(WORKFLOW_BLOCK_BEGIN, begin)
                && !duplicate(WORKFLOW_BLOCK_END, end) =>
        {
            Ok(Some((begin, end + WORKFLOW_BLOCK_END.len())))
        }
        _ => Err("the mastermind workflow markers are unpaired or repeated".into()),
    }
}

/// Block state of an instructions file. A CRLF block compares like an LF one.
pub fn workflow_block_state(text: &str) -> Result<WorkflowBlock, String> {
    Ok(match block_span(text)? {
        None => WorkflowBlock::Missing {
            legacy: text.lines().any(|line| line.trim_end() == WORKFLOW_HEADING),
        },
        Some((begin, end)) if text[begin..end].replace("\r\n", "\n") == workflow_block() => {
            WorkflowBlock::Current
        }
        Some(_) => WorkflowBlock::Outdated,
    })
}

/// Text with the current managed block, or `None` when it is already current.
/// Only the block changes: text outside it keeps its bytes, and the block uses
/// CRLF when the file does. An unmarked workflow section from an older `init`
/// is wrapped only when it is the current section verbatim, since anything
/// else may hold user edits.
pub fn with_workflow_block(text: &str) -> Result<Option<String>, String> {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let block = workflow_block().replace('\n', eol);
    match workflow_block_state(text)? {
        WorkflowBlock::Current => Ok(None),
        WorkflowBlock::Outdated => {
            let (begin, end) = block_span(text)?.expect("outdated block has markers");
            Ok(Some(format!("{}{block}{}", &text[..begin], &text[end..])))
        }
        WorkflowBlock::Missing { legacy: false } => {
            let text = text.trim_end();
            Ok(Some(if text.is_empty() {
                format!("{block}{eol}")
            } else {
                format!("{text}{eol}{eol}{block}{eol}")
            }))
        }
        WorkflowBlock::Missing { legacy: true } => {
            let section = workflow_section().replace('\n', eol);
            let wrapped = text.find(&section).and_then(|at| {
                let rest = &text[at + section.len()..];
                (rest.is_empty() || rest.starts_with(['\r', '\n']))
                    .then(|| format!("{}{block}{rest}", &text[..at]))
            });
            wrapped.map(Some).ok_or_else(|| {
                format!(
                    "it has an edited `{WORKFLOW_HEADING}` section without markers; delete that section and run init again to get the managed block"
                )
            })
        }
    }
}

/// Strip the HTML-comment "instructions to the user" block from a template so
/// the written file contains only what the adopter uses. Looks for
/// `<!-- ─── COPY FROM HERE ─── -->` … `<!-- ─── COPY TO HERE ─── -->`; returns
/// the text unchanged if absent.
pub fn strip_comment(text: &str) -> String {
    const MARKER_OPEN: &str = "<!-- ─── COPY FROM HERE ─── -->";
    const MARKER_CLOSE: &str = "<!-- ─── COPY TO HERE ─── -->";
    if let Some(start) = text.find(MARKER_OPEN) {
        let body_start = start + MARKER_OPEN.len();
        let body_end = text[body_start..]
            .find(MARKER_CLOSE)
            .map(|i| body_start + i)
            .unwrap_or(text.len());
        text[body_start..body_end].trim().to_string() + "\n"
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_block_is_added_refreshed_and_left_alone_when_current() {
        let user = "# My project\n\nOwn rules.\n";
        let added = with_workflow_block(user).unwrap().unwrap();
        assert!(added.starts_with(user));
        assert_eq!(
            workflow_block_state(&added).unwrap(),
            WorkflowBlock::Current
        );
        assert_eq!(with_workflow_block(&added).unwrap(), None);

        let stale = added.replace("Use the lightest mode", "Use the old mode");
        assert_eq!(
            workflow_block_state(&stale).unwrap(),
            WorkflowBlock::Outdated
        );
        let edited = format!("{stale}\n## After\nkept\n");
        let refreshed = with_workflow_block(&edited).unwrap().unwrap();
        assert!(refreshed.starts_with(user));
        assert!(refreshed.ends_with("\n## After\nkept\n"));
        assert_eq!(
            workflow_block_state(&refreshed).unwrap(),
            WorkflowBlock::Current
        );
    }

    #[test]
    fn legacy_workflow_section_is_wrapped_only_when_unedited() {
        let legacy = strip_comment(WORKFLOW_TEMPLATE);
        let wrapped = with_workflow_block(&legacy).unwrap().unwrap();
        assert_eq!(
            workflow_block_state(&wrapped).unwrap(),
            WorkflowBlock::Current
        );
        assert_eq!(wrapped.matches(WORKFLOW_HEADING).count(), 1);

        let followed = format!("{legacy}\n## My rules\nkept\n");
        let wrapped = with_workflow_block(&followed).unwrap().unwrap();
        assert!(wrapped.ends_with(&format!("{WORKFLOW_BLOCK_END}\n\n## My rules\nkept\n")));

        let edited = legacy.replace("Use the lightest mode", "Use my mode");
        assert!(with_workflow_block(&edited).is_err());
    }

    #[test]
    fn crlf_blocks_compare_like_lf_and_text_outside_the_block_keeps_its_bytes() {
        let crlf = |text: &str| text.replace('\n', "\r\n");
        let current = crlf(&new_instructions_file());
        assert_eq!(
            workflow_block_state(&current).unwrap(),
            WorkflowBlock::Current
        );
        assert_eq!(with_workflow_block(&current).unwrap(), None);

        let stale = current.replace("Use the lightest mode", "Use the old mode");
        let refreshed = with_workflow_block(&stale).unwrap().unwrap();
        assert_eq!(refreshed, current);

        let legacy = crlf(&strip_comment(WORKFLOW_TEMPLATE));
        let wrapped = with_workflow_block(&legacy).unwrap().unwrap();
        assert!(!wrapped.replace("\r\n", "").contains('\n'));
        assert_eq!(
            workflow_block_state(&wrapped).unwrap(),
            WorkflowBlock::Current
        );

        let mixed = "a\nb\r\n";
        let added = with_workflow_block(mixed).unwrap().unwrap();
        assert!(added.starts_with("a\nb\r\n\r\n"), "{added:?}");
    }

    #[test]
    fn unpaired_workflow_markers_are_rejected() {
        let block = workflow_block();
        for text in [
            WORKFLOW_BLOCK_BEGIN.to_string(),
            format!("{WORKFLOW_BLOCK_END}\n{WORKFLOW_BLOCK_BEGIN}"),
            format!("{block}\n{block}"),
        ] {
            assert!(with_workflow_block(&text).is_err(), "{text}");
        }
    }

    #[test]
    fn new_instructions_file_keeps_placeholders_outside_the_block() {
        let file = new_instructions_file();
        assert!(!file.starts_with("---"));
        let begin = file.find(WORKFLOW_BLOCK_BEGIN).unwrap();
        assert!(file[..begin].contains("<PROJECT_NAME>"));
        assert!(!file[begin..].contains("<PROJECT_NAME>"));
        assert_eq!(workflow_block_state(&file).unwrap(), WorkflowBlock::Current);
    }

    /// CONTEXT stays stack-agnostic: one scaffold for every detected stack.
    /// Stack facts and commands are derivable and belong in CLAUDE.md;
    /// per-stack templates caused unverified assumptions and schema drift in
    /// durable project memory.
    #[test]
    fn context_template_carries_the_lean_contract() {
        for heading in ["## Identity", "## Active goals", "## Decision log"] {
            assert!(CONTEXT_TEMPLATE.contains(heading), "missing {heading}");
        }
        for field in [
            "Status",
            "Supersedes",
            "Provenance",
            "Evidence",
            "Reusable lesson",
        ] {
            assert!(CONTEXT_TEMPLATE.contains(field), "missing {field}");
        }
        assert!(
            !CONTEXT_TEMPLATE.contains("Pre-seeded with"),
            "generic gotchas"
        );
    }
}
