//! Compile-time template constants and utilities.
//!
//! Templates are embedded via `include_str!` so the published crate is
//! self-contained. `scripts/validate.py` enforces parity between the
//! crate-local copies and the repository sources.

/// Generic CONTEXT.md scaffold (stack-agnostic).
pub const CONTEXT_TEMPLATE: &str = include_str!("../templates/context.md");

/// Mastermind workflow CLAUDE.md template.
pub const WORKFLOW_TEMPLATE: &str = include_str!("../templates/workflow.md");

/// CONTEXT is deliberately stack-agnostic. Stack facts and commands are
/// derivable and belong in CLAUDE.md; duplicating profile templates caused
/// unverified assumptions and schema drift in durable project memory.
pub fn for_profile(_profile: crate::Profile) -> &'static str {
    CONTEXT_TEMPLATE
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
    fn every_detected_stack_uses_the_same_lean_context_contract() {
        for profile in [
            crate::Profile::Generic,
            crate::Profile::TypescriptApi,
            crate::Profile::ReactNative,
            crate::Profile::PythonFastapi,
            crate::Profile::Rust,
            crate::Profile::Monorepo,
        ] {
            let template = for_profile(profile);
            for heading in ["## Identity", "## Active goals", "## Decision log"] {
                assert!(
                    template.contains(heading),
                    "missing {heading} in {profile:?}"
                );
            }
            for field in [
                "Status",
                "Supersedes",
                "Provenance",
                "Evidence",
                "Reusable lesson",
            ] {
                assert!(template.contains(field), "missing {field} in {profile:?}");
            }
            assert!(
                !template.contains("Pre-seeded with"),
                "generic gotchas in {profile:?}"
            );
        }
    }
}
