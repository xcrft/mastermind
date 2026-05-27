//! Parser for `.mastermind/tasks/XXX-*.md` spec files.
//!
//! The canonical structure is at
//! `skills/workflow/mastermind-task-planning/references/spec-template.md`,
//! but real specs deviate (section reordering, custom sections, freeform
//! prose under each header). The parser is **lenient by design**: it extracts
//! what it can, returns it as a structured `ParsedSpec`, and never aborts on
//! shape drift. Callers (`verify_spec` / `audit_spec`) decide what missing
//! data means semantically.
//!
//! What's parsed:
//! - Section name → body text (everything between `## Name` headers)
//! - "Pre-edit symbol snapshot" → list of `SymbolClaim { name, callers }`
//!   extracted from bullet lines like
//!   ``- `session_count` — 8 callers (...) ``
//! - File paths mentioned anywhere — backticked tokens matching a path-like
//!   pattern (`*.rs`, `src/foo.ts`, etc.). Used by audit-spec to compare
//!   against `git diff --name-only`.
//! - VERIFY commands — `**VERIFY**: `cmd`` lines under phase bodies.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Serialize, Clone)]
pub struct ParsedSpec {
    pub path: String,
    /// All `## Name` sections in source order. Body is everything until the
    /// next `##` header. Subsections (`### …`) stay inside their parent.
    pub sections: BTreeMap<String, String>,
    /// Order in which sections appear (BTreeMap loses that).
    pub section_order: Vec<String>,
    /// Symbols the planner declared in "Pre-edit symbol snapshot".
    pub pre_edit_snapshot: Vec<SymbolClaim>,
    /// File paths the spec mentions in backticks (deduplicated).
    pub mentioned_files: Vec<String>,
    /// VERIFY commands extracted from phase blocks.
    pub verify_commands: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct SymbolClaim {
    pub name: String,
    /// Caller count the planner recorded at snapshot time. None if the
    /// bullet line didn't say (e.g., just `- \`foo\` — added in this spec`).
    pub callers: Option<u32>,
    /// Signature the planner recorded via `mmcg_search <name>`. Extracted from
    /// `signature \`<sig>\`` after the caller count in a snapshot bullet.
    /// None if absent — that's allowed (signature claim is opt-in evidence).
    pub signature: Option<String>,
    /// Raw bullet text for hint in error messages.
    pub raw: String,
}

/// Parse a spec file from disk.
pub fn parse_file(path: &Path) -> std::io::Result<ParsedSpec> {
    let body = std::fs::read_to_string(path)?;
    Ok(parse_str(&path.display().to_string(), &body))
}

pub fn parse_str(source_path: &str, body: &str) -> ParsedSpec {
    let (sections, order) = split_sections(body);
    let pre_edit_snapshot = sections
        .iter()
        .find(|(k, _)| section_key(k) == "pre-edit symbol snapshot")
        .map(|(_, body)| extract_snapshot(body))
        .unwrap_or_default();
    let mentioned_files = extract_mentioned_files(body);
    let verify_commands = extract_verify_commands(body);

    ParsedSpec {
        path: source_path.to_string(),
        sections,
        section_order: order,
        pre_edit_snapshot,
        mentioned_files,
        verify_commands,
    }
}

/// Return the BODY (whitespace-trimmed) of a named section. Lookup is
/// case-insensitive and tolerates `*(MANDATORY ...)*` suffix annotations.
pub fn section_body<'a>(spec: &'a ParsedSpec, name: &str) -> Option<&'a str> {
    let want = section_key(name);
    spec.sections
        .iter()
        .find(|(k, _)| section_key(k) == want)
        .map(|(_, body)| body.trim())
}

/// Whitespace + annotation-stripped lowercase form, used for case-insensitive
/// section matching. `"## Tests Plan *(MANDATORY)*"` → `"tests plan"`.
fn section_key(raw: &str) -> String {
    let s = raw.trim_start_matches('#').trim();
    // Strip italic annotations like `*(MANDATORY for non-trivial work)*`.
    let s = match s.find('*') {
        Some(i) => &s[..i],
        None => s,
    };
    s.trim().to_lowercase()
}

fn split_sections(body: &str) -> (BTreeMap<String, String>, Vec<String>) {
    let mut sections: BTreeMap<String, String> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut current: Option<(String, String)> = None;

    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            // Commit the previous section.
            if let Some((name, body)) = current.take() {
                if !sections.contains_key(&name) {
                    order.push(name.clone());
                }
                sections.insert(name, body);
            }
            current = Some((rest.trim().to_string(), String::new()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
        // Lines before the first `##` are ignored (frontmatter / preamble).
    }
    if let Some((name, body)) = current.take() {
        if !sections.contains_key(&name) {
            order.push(name.clone());
        }
        sections.insert(name, body);
    }
    (sections, order)
}

/// `- \`name\` — 8 callers (...)` or  `- \`name\` — added` style bullet lines.
/// The dash separator is em-dash (Mastermind convention) but plain `-` is
/// tolerated. Caller count is the first integer that precedes the literal
/// "caller" (case-insensitive).
fn extract_snapshot(body: &str) -> Vec<SymbolClaim> {
    let mut out: Vec<SymbolClaim> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('-') {
            continue;
        }
        let after_dash = trimmed.trim_start_matches('-').trim();
        // Pull the first backticked identifier — `name` (may be `mod.name` or
        // `Type::method`).
        let Some(start) = after_dash.find('`') else {
            continue;
        };
        let Some(end_rel) = after_dash[start + 1..].find('`') else {
            continue;
        };
        let full = &after_dash[start + 1..start + 1 + end_rel];
        // Take the leaf for matching against mmcg_search (mmcg indexes the
        // leaf name regardless of how the planner spelled it).
        let leaf = full
            .rsplit(&['.', ':'][..])
            .next()
            .unwrap_or(full)
            .to_string();
        // Parse caller count from "N callers" pattern.
        let callers = extract_caller_count(after_dash);
        let signature = extract_signature(after_dash);
        out.push(SymbolClaim {
            name: leaf,
            callers,
            signature,
            raw: trimmed.to_string(),
        });
    }
    out
}

/// Extract the bullet's `signature \`<sig>\`` claim. Returns None when the
/// bullet doesn't include one. Tolerates the literal word "signature" being
/// followed by either a backticked code span (preferred) or bare text up to a
/// trailing parenthetical / comma.
fn extract_signature(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let key = lower.find("signature")?;
    // Position in the original string is the same offset (single-byte word).
    let after = text[key + "signature".len()..].trim_start_matches(['*', ' ', ':', '=']);
    // Preferred: backticked.
    if let Some(stripped) = after.strip_prefix('`') {
        if let Some(end) = stripped.find('`') {
            let sig = stripped[..end].trim();
            if !sig.is_empty() {
                return Some(sig.to_string());
            }
        }
    }
    None
}

fn extract_caller_count(text: &str) -> Option<u32> {
    // Walk word-by-word looking for "<int> caller(s)".
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if w.to_lowercase().starts_with("caller") && i > 0 {
            if let Ok(n) = words[i - 1].parse::<u32>() {
                return Some(n);
            }
        }
    }
    None
}

/// Every backticked token that looks like a path — has a directory separator
/// OR a file extension. Deduplicated and stably ordered. Used by audit-spec
/// to compare against `git diff --name-only`.
fn extract_mentioned_files(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut chars = body.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '`' {
            continue;
        }
        // Find the matching closing backtick on the same line.
        let rest = &body[i + 1..];
        let Some(end_rel) = rest.find('`') else {
            continue;
        };
        let token = &rest[..end_rel];
        if token.is_empty() || token.len() > 200 || token.contains('\n') {
            continue;
        }
        if looks_like_path(token) && seen.insert(token.to_string()) {
            out.push(token.to_string());
        }
        // Skip past the closing backtick.
        for _ in 0..end_rel + 1 {
            chars.next();
        }
    }
    out
}

fn looks_like_path(s: &str) -> bool {
    let has_slash = s.contains('/');
    let has_known_ext = [
        ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".cs", ".go", ".java", ".php",
        ".c", ".cc", ".cpp", ".cxx", ".h", ".hpp", ".md", ".toml", ".json", ".yml", ".yaml",
        ".sql", ".sh", ".html", ".css", ".scss",
    ]
    .iter()
    .any(|ext| s.ends_with(ext));
    has_slash || has_known_ext
}

/// `**VERIFY**: `cmd``  or  `**VERIFY**: command-without-backticks`.
/// Strips the leading bold marker and returns the command text.
fn extract_verify_commands(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        let after = trimmed
            .strip_prefix("**VERIFY**:")
            .or_else(|| trimmed.strip_prefix("**VERIFY:**"))
            .or_else(|| trimmed.strip_prefix("VERIFY:"));
        let Some(after) = after else { continue };
        let after = after.trim();
        // Prefer backticked content, else the whole line.
        let cmd = if let Some(s) = after.strip_prefix('`') {
            s.find('`').map(|e| &s[..e]).unwrap_or(after)
        } else {
            after
        };
        let cmd = cmd.trim();
        if !cmd.is_empty() {
            out.push(cmd.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Add session_count accessor

## Goals
1. Add `SessionStore::session_count()` returning current size
2. Unit-test the accessor

## Alternatives Considered
- **In-memory atomic counter** — rejected: extra invariant to maintain
- **Picked: read from internal map** — chosen: zero-extra-state

## Pre-edit symbol snapshot
- `SessionStore` — 12 callers, signature `pub struct SessionStore`
- `refresh` — 8 callers, signature `pub fn refresh(&self) -> Result<Session>`
- `new_helper` — added in this spec

## Phase 1: Add accessor
- [ ] Add accessor
**FIND** in `src/session.rs`:
```rust
pub fn refresh(&self) -> Result<Session> {
```
**CHANGE TO**:
```rust
pub fn session_count(&self) -> usize {
    self.sessions.read().unwrap().len()
}

pub fn refresh(&self) -> Result<Session> {
```
**VERIFY**: `cargo test session_count_returns_current_size`

## Tests Plan
- `session_count_returns_current_size` in `src/session.rs`

## Documentation Plan
- Update `README.md` § Session API

## Observability Plan
- N/A — pure accessor, no side effects

## Performance Considerations
- O(1) — RwLock read + HashMap::len
";

    #[test]
    fn extracts_sections_in_order() {
        let s = parse_str("test.md", SAMPLE);
        assert!(s.section_order.iter().any(|n| n.starts_with("Goals")));
        assert!(s.section_order.iter().any(|n| n.starts_with("Tests Plan")));
        assert!(s
            .section_order
            .iter()
            .any(|n| n.contains("Pre-edit symbol snapshot")));
        // section_body() finds despite annotation suffixes.
        assert!(section_body(&s, "Goals").is_some());
        assert!(section_body(&s, "Tests Plan").is_some());
    }

    #[test]
    fn extracts_snapshot_with_caller_counts() {
        let s = parse_str("test.md", SAMPLE);
        let by_name: std::collections::HashMap<&str, &SymbolClaim> = s
            .pre_edit_snapshot
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();
        assert_eq!(by_name["SessionStore"].callers, Some(12));
        assert_eq!(by_name["refresh"].callers, Some(8));
        // `new_helper` has no caller count — None, not skipped.
        assert_eq!(by_name["new_helper"].callers, None);
    }

    #[test]
    fn extracts_mentioned_files() {
        let s = parse_str("test.md", SAMPLE);
        assert!(s.mentioned_files.contains(&"src/session.rs".to_string()));
        assert!(s.mentioned_files.contains(&"README.md".to_string()));
        // Backticked identifiers without path-like shape are NOT files.
        assert!(!s.mentioned_files.contains(&"SessionStore".to_string()));
        assert!(!s.mentioned_files.contains(&"refresh".to_string()));
    }

    #[test]
    fn extracts_verify_commands() {
        let s = parse_str("test.md", SAMPLE);
        assert_eq!(
            s.verify_commands,
            vec!["cargo test session_count_returns_current_size".to_string()]
        );
    }

    #[test]
    fn extracts_signature_from_snapshot_bullets() {
        let s = parse_str("test.md", SAMPLE);
        let by_name: std::collections::HashMap<&str, &SymbolClaim> = s
            .pre_edit_snapshot
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();
        // SAMPLE has signatures on the first two bullets.
        assert_eq!(
            by_name["SessionStore"].signature.as_deref(),
            Some("pub struct SessionStore")
        );
        assert_eq!(
            by_name["refresh"].signature.as_deref(),
            Some("pub fn refresh(&self) -> Result<Session>")
        );
        // `new_helper` bullet has no signature clause.
        assert_eq!(by_name["new_helper"].signature, None);
    }

    #[test]
    fn snapshot_handles_qualified_names_taking_leaf() {
        let body = "## Pre-edit symbol snapshot\n\
                    - `pkg.module.foo` — 4 callers\n\
                    - `Type::method` — 2 callers\n";
        let s = parse_str("t.md", body);
        let names: Vec<&str> = s
            .pre_edit_snapshot
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"foo"), "dotted name → leaf");
        assert!(names.contains(&"method"), "::-qualified → leaf");
    }
}
