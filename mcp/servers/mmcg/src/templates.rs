//! Compile-time template constants and utilities.
//!
//! Templates are embedded via `include_str!` so the published crate is
//! self-contained. `scripts/validate.py` enforces parity between the
//! crate-local copies and the repository sources.

/// Generic CONTEXT.md scaffold (stack-agnostic).
pub const CONTEXT_TEMPLATE: &str = include_str!("../templates/context.md");

/// Mastermind workflow CLAUDE.md template.
pub const WORKFLOW_TEMPLATE: &str = include_str!("../templates/workflow.md");

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
