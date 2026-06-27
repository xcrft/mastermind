//! Compile-time template constants and utilities.
//!
//! Templates are embedded via `include_str!` so the published crate is
//! self-contained. `scripts/validate.py` enforces parity between the
//! crate-local copies and the repository sources.

/// Generic CONTEXT.md scaffold (stack-agnostic).
pub const CONTEXT_TEMPLATE: &str = include_str!("../templates/context.md");

/// Mastermind workflow CLAUDE.md template.
pub const WORKFLOW_TEMPLATE: &str = include_str!("../templates/workflow.md");

const PROFILE_TYPESCRIPT_API: &str = include_str!("../templates/profiles/typescript-api.md");
const PROFILE_REACT_NATIVE: &str = include_str!("../templates/profiles/react-native.md");
const PROFILE_PYTHON_FASTAPI: &str = include_str!("../templates/profiles/python-fastapi.md");
const PROFILE_RUST_CLI: &str = include_str!("../templates/profiles/rust-cli.md");
const PROFILE_MONOREPO: &str = include_str!("../templates/profiles/monorepo.md");

/// Return the raw template text for a given profile.
pub fn for_profile(profile: crate::Profile) -> &'static str {
    match profile {
        crate::Profile::Generic => CONTEXT_TEMPLATE,
        crate::Profile::TypescriptApi => PROFILE_TYPESCRIPT_API,
        crate::Profile::ReactNative => PROFILE_REACT_NATIVE,
        crate::Profile::PythonFastapi => PROFILE_PYTHON_FASTAPI,
        crate::Profile::RustCli => PROFILE_RUST_CLI,
        crate::Profile::Monorepo => PROFILE_MONOREPO,
    }
}

/// Return the kebab-case label for a profile (used in user-facing messages).
pub fn profile_label(profile: crate::Profile) -> &'static str {
    match profile {
        crate::Profile::Generic => "generic",
        crate::Profile::TypescriptApi => "typescript-api",
        crate::Profile::ReactNative => "react-native",
        crate::Profile::PythonFastapi => "python-fastapi",
        crate::Profile::RustCli => "rust-cli",
        crate::Profile::Monorepo => "monorepo",
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
