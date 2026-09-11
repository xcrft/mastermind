//! Parser for `.mastermind/tasks/<NNN>-<name>/spec.md` files.
//!
//! Canonical structure: `skills/workflow/mastermind-task-planning/references/spec-template.md`,
//! but real specs deviate (reordered/custom sections, freeform prose). Parser
//! is **lenient by design**: extracts what it can into a `ParsedSpec`, never
//! aborts on shape drift. Callers (`verify_spec` / `audit_spec`) decide what
//! missing data means semantically.
//!
//! What's parsed:
//! - Section name → body text (everything between `## Name` headers)
//! - "Pre-edit symbol snapshot" → `SymbolClaim { name, callers }` from bullets
//!   like ``- `session_count` — 8 callers (...) ``
//! - File paths — backticked path-like tokens (`*.rs`, `src/foo.ts`); audit-spec
//!   compares against `git diff --name-only`.
//! - VERIFY commands — `**VERIFY**: `cmd`` lines under phase bodies.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

const MAX_SPEC_BYTES: u64 = crate::audit_bundle::BUNDLE_INPUT_MAX as u64;

#[derive(Debug, Serialize, Clone)]
pub struct ParsedSpec {
    pub path: String,
    /// All `## Name` sections in source order. Body runs until the next `##`
    /// header; subsections (`### …`) stay inside their parent.
    pub sections: BTreeMap<String, String>,
    /// Section appearance order (BTreeMap loses it).
    pub section_order: Vec<String>,
    /// Symbols the planner declared in "Pre-edit symbol snapshot".
    pub pre_edit_snapshot: Vec<SymbolClaim>,
    /// Backticked file paths the spec mentions (deduplicated).
    pub mentioned_files: Vec<String>,
    /// VERIFY commands extracted from phase blocks.
    pub verify_commands: Vec<String>,
    /// Literal FIND preconditions; verify-spec checks the current named file.
    /// CHANGE TO payloads and replacement order are not modeled here.
    pub find_blocks: Vec<FindBlock>,
    /// YAML frontmatter (`---`-delimited at file start). When present, takes
    /// precedence over heuristic extraction in verify/audit gates; when absent,
    /// gates fall back to the heuristic fields above with an advisory
    /// "consider migrating to frontmatter" warning.
    pub frontmatter: Option<Frontmatter>,
    /// A present but invalid YAML block cannot fall back to a weaker contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontmatter_error: Option<String>,
}

impl ParsedSpec {
    /// Explicit command obligations, in declaration order with duplicates
    /// removed. Labels, empty commands and ordinary shell fences are excluded.
    /// Only outside whitespace is normalized.
    pub fn declared_verify_commands(&self) -> Vec<&str> {
        let mut seen = HashSet::new();
        self.frontmatter
            .iter()
            .flat_map(|fm| fm.verify.iter())
            .filter_map(VerifyEntry::command)
            .chain(self.verify_commands.iter().map(String::as_str))
            .map(str::trim)
            .filter(|cmd| !cmd.is_empty() && seen.insert(*cmd))
            .collect()
    }
}

/// Structured spec metadata from a YAML frontmatter block. All fields optional
/// — partial frontmatter is fine; gates use what's present and fall back to
/// heuristics for the rest.
///
/// Schema (all optional):
/// ```yaml
/// id: 042
/// title: Add billing webhook
/// risk: high
/// touches:
///   - file: src/billing/billing.controller.ts
///     language: typescript
///     symbols:
///       - name: handleWebhook
///         signature: "async handleWebhook(req, res)"
///         callers: 4
/// verify:
///   - typecheck                       # label-only (informational)
///   - cmd: "npm test -- billing"      # executable (PATH-checked)
/// expected_docs:
///   - README.md
///   - docs/billing.md
/// creates:
///   - docs/billing.md                  # new relative to the audit baseline
/// breaking_changes:
///   removed_symbols:
///     - old_api                       # bare string OR
///     - name: legacy_handler          # detailed object
///       file: src/api/legacy.ts
///       reason: "deprecated since 2025-01"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Frontmatter {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// "low" / "medium" / "high" — informational, surfaced in the risk report.
    #[serde(default)]
    pub risk: Option<String>,
    /// Workflow contract mode. `verified` and `strict` are current;
    /// `lite` / `standard` remain accepted for existing task files.
    #[serde(default)]
    pub mode: Option<String>,
    /// Files the executor is authorized to modify, with optional symbol-level
    /// snapshots scoped by file + language.
    #[serde(default)]
    pub touches: Vec<TouchEntry>,
    /// Files to add relative to the task baseline. Preflight allows absent
    /// targets or regular drafts, so revised contracts can be checked again.
    #[serde(default)]
    pub creates: Vec<String>,
    /// Verification steps. Strings are labels (informational); `cmd:` objects
    /// are command obligations checked by preflight and canonical postflight.
    #[serde(default)]
    pub verify: Vec<VerifyEntry>,
    /// Required docs, including new docs explicitly listed in `creates`.
    /// Code-removal acknowledgements never exempt these paths.
    #[serde(default)]
    pub expected_docs: Vec<String>,
    #[serde(default)]
    pub breaking_changes: BreakingChanges,
}

impl Frontmatter {
    /// True when frontmatter declares any file-scope info. When true,
    /// verify-spec / audit-spec use the frontmatter
    /// list AUTHORITATIVELY for file existence + scope checks instead of merging
    /// the noisy heuristic backticked-path extraction.
    pub fn has_file_scope(&self) -> bool {
        self.code_paths().next().is_some() || !self.expected_docs.is_empty()
    }

    /// Explicit implementation scope shared by controller and policy evidence.
    pub(crate) fn code_paths(&self) -> impl Iterator<Item = &str> {
        self.touches
            .iter()
            .map(|touch| touch.file.as_str())
            .chain(self.creates.iter().map(String::as_str))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TouchEntry {
    pub file: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub symbols: Vec<SymbolSpec>,
}

/// Polymorphic symbol — a bare name string (`- foo`) or a detailed object
/// (`- {name: foo, signature: "...", callers: 4}`). Untagged so YAML parses
/// both forms transparently.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SymbolSpec {
    Name(String),
    Detailed {
        name: String,
        #[serde(default)]
        signature: Option<String>,
        #[serde(default)]
        callers: Option<u32>,
        #[serde(default)]
        file: Option<String>,
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
}

impl SymbolSpec {
    pub fn name(&self) -> &str {
        match self {
            SymbolSpec::Name(s) => s,
            SymbolSpec::Detailed { name, .. } => name,
        }
    }
    pub fn signature(&self) -> Option<&str> {
        match self {
            SymbolSpec::Name(_) => None,
            SymbolSpec::Detailed { signature, .. } => signature.as_deref(),
        }
    }
    pub fn callers(&self) -> Option<u32> {
        match self {
            SymbolSpec::Name(_) => None,
            SymbolSpec::Detailed { callers, .. } => *callers,
        }
    }
    pub fn file(&self) -> Option<&str> {
        match self {
            SymbolSpec::Name(_) => None,
            SymbolSpec::Detailed { file, .. } => file.as_deref(),
        }
    }
    pub fn language(&self) -> Option<&str> {
        match self {
            SymbolSpec::Name(_) => None,
            SymbolSpec::Detailed { language, .. } => language.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VerifyEntry {
    Label(String),
    Command { cmd: String },
}

impl VerifyEntry {
    /// Runnable command (`cmd:` form), or None for label-only entries.
    pub fn command(&self) -> Option<&str> {
        match self {
            VerifyEntry::Command { cmd } => Some(cmd),
            VerifyEntry::Label(_) => None,
        }
    }
    /// Human-readable label (the string for Label, the cmd for Command).
    pub fn label(&self) -> &str {
        match self {
            VerifyEntry::Label(s) => s,
            VerifyEntry::Command { cmd } => cmd,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BreakingChanges {
    /// Symbols intentionally removed in this spec. Audit cross-references the
    /// git diff: a symbol removed but NOT listed here is flagged
    /// `RemovedSymbolNotAcknowledged` (Broken). Replaces the older
    /// lowercase-substring heuristic, which was fooled by incidental mentions
    /// like ``Do not remove `old_api` ``.
    #[serde(default)]
    pub removed_symbols: Vec<SymbolSpec>,
}

#[derive(Debug, Serialize, Clone)]
pub struct FindBlock {
    /// File path declared above this FIND (`**File:** \`<path>\``). None when no
    /// File marker — FIND text still parses, but preflight rejects it as
    /// unavailable without a target.
    pub file: Option<String>,
    /// Raw FIND payload (between the triple backticks).
    pub find_text: String,
    /// Phase label for diagnostic output (`Phase 1.2`, etc.).
    pub phase: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct SymbolClaim {
    /// Declared name, including lexical qualification such as B.run or B::run.
    pub name: String,
    /// Caller count recorded at snapshot time. None if the bullet didn't say
    /// (e.g., just `- \`foo\` — added in this spec`).
    pub callers: Option<u32>,
    /// Signature recorded via `mmcg_search <name>`, from `signature \`<sig>\``
    /// after the caller count. None if absent — allowed (opt-in evidence).
    pub signature: Option<String>,
    /// Raw bullet text for hint in error messages.
    pub raw: String,
}

/// Parse a spec file from disk.
pub fn parse_file(path: &Path) -> std::io::Result<ParsedSpec> {
    let (resolved, source) = crate::bounded_fs::read_selected_regular_file(
        path,
        MAX_SPEC_BYTES,
        MAX_SPEC_BYTES,
        crate::bounded_fs::ReadControl::default(),
    )
    .map_err(|error| match error {
        crate::bounded_fs::BoundedReadError::TooLarge { .. } => std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("spec exceeds the {MAX_SPEC_BYTES}-byte limit"),
        ),
        crate::bounded_fs::BoundedReadError::SnapshotChanged => {
            std::io::Error::other("spec changed while it was being read")
        }
        crate::bounded_fs::BoundedReadError::InvalidPath
        | crate::bounded_fs::BoundedReadError::OutsideRoot
        | crate::bounded_fs::BoundedReadError::NotRegular => std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "spec must be a regular file",
        ),
        crate::bounded_fs::BoundedReadError::Interrupted
        | crate::bounded_fs::BoundedReadError::DeadlineExceeded => {
            std::io::Error::other("spec read was interrupted")
        }
        crate::bounded_fs::BoundedReadError::Io(error) => error,
    })?;
    if path.canonicalize()? != resolved {
        return Err(std::io::Error::other(
            "spec path changed while it was being read",
        ));
    }
    let body = String::from_utf8(source.bytes)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "spec is not UTF-8"))?;
    Ok(parse_str(&path.display().to_string(), &body))
}

pub fn parse_str(source_path: &str, body: &str) -> ParsedSpec {
    // Retain the readable body for diagnostics while gates reject bad metadata.
    let (frontmatter, frontmatter_error, body_after_fm) = extract_frontmatter(body);
    let (sections, order) = split_sections(body_after_fm);
    let pre_edit_snapshot = sections
        .iter()
        .find(|(k, _)| section_key(k) == "pre-edit symbol snapshot")
        .map(|(_, body)| extract_snapshot(body))
        .unwrap_or_default();
    let mentioned_files = extract_mentioned_files(body_after_fm);
    let verify_commands = extract_verify_commands(body_after_fm);
    let find_blocks = extract_find_blocks(body_after_fm);

    ParsedSpec {
        path: source_path.to_string(),
        sections,
        section_order: order,
        pre_edit_snapshot,
        mentioned_files,
        verify_commands,
        find_blocks,
        frontmatter,
        frontmatter_error,
    }
}

/// Split a `---\n...\n---\n` block off the top. Returns parsed frontmatter
/// (None if absent or unparseable), any contract error and the body remainder.
///
/// Leading `---` MUST be the very first line (no blank lines before it), per
/// Jekyll / MkDocs / Hugo convention; a trailing `---` closes the block. On
/// deserialize failure the body stays readable, but is not a fallback contract.
fn extract_frontmatter(body: &str) -> (Option<Frontmatter>, Option<String>, &str) {
    if !body.starts_with("---\n") && !body.starts_with("---\r\n") {
        return (None, None, body);
    }
    // Skip opening fence.
    let after_open = body
        .strip_prefix("---\n")
        .or_else(|| body.strip_prefix("---\r\n"))
        .unwrap_or(body);
    // Find closing `---` on its own line.
    let mut yaml_end = None;
    let mut rest_start = 0;
    let mut offset = 0;
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" {
            yaml_end = Some(offset);
            rest_start = offset + line.len();
            break;
        }
        offset += line.len();
    }
    let Some(yaml_end) = yaml_end else {
        return (None, Some("frontmatter_unterminated".into()), body);
    };
    let yaml_src = &after_open[..yaml_end];
    let rest = &after_open[rest_start..];
    // Inspect YAML types before typed deserialization can coerce path scalars.
    // Keep the existing parser for older metadata, including numeric task IDs.
    let valid_shape = serde_norway::from_str::<serde_norway::Value>(yaml_src)
        .ok()
        .is_some_and(|value| {
            let serde_norway::Value::Mapping(fields) = value else {
                return false;
            };
            fields.get("creates").is_none_or(|creates| {
                matches!(creates, serde_norway::Value::Sequence(paths)
                    if paths.iter().all(|path| matches!(path, serde_norway::Value::String(_))))
            })
        });
    if !valid_shape {
        return (None, Some("frontmatter_invalid".into()), rest);
    }
    match serde_norway::from_str::<Frontmatter>(yaml_src) {
        Ok(fm) => (Some(fm), None, rest),
        Err(_) => (None, Some("frontmatter_invalid".into()), rest),
    }
}

/// Whitespace-trimmed BODY of a named section. Case-insensitive lookup,
/// tolerates `*(MANDATORY ...)*` suffix annotations.
pub fn section_body<'a>(spec: &'a ParsedSpec, name: &str) -> Option<&'a str> {
    let want = section_key(name);
    spec.sections
        .iter()
        .find(|(k, _)| section_key(k) == want)
        .map(|(_, body)| body.trim())
}

/// Whitespace + annotation-stripped lowercase form for case-insensitive
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
            // Commit previous section.
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
        // Lines before the first `##` ignored (frontmatter / preamble).
    }
    if let Some((name, body)) = current.take() {
        if !sections.contains_key(&name) {
            order.push(name.clone());
        }
        sections.insert(name, body);
    }
    (sections, order)
}

/// `- \`name\` — 8 callers (...)` or `- \`name\` — added` bullet lines.
/// Separator is em-dash (Mastermind convention) but plain `-` is tolerated.
/// Caller count is the first integer preceding literal "caller"
/// (case-insensitive).
fn extract_snapshot(body: &str) -> Vec<SymbolClaim> {
    let mut out: Vec<SymbolClaim> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('-') {
            continue;
        }
        let after_dash = trimmed.trim_start_matches('-').trim();
        // First backticked identifier — `name` (may be `mod.name` or `Type::method`).
        let Some(start) = after_dash.find('`') else {
            continue;
        };
        let Some(end_rel) = after_dash[start + 1..].find('`') else {
            continue;
        };
        let full = &after_dash[start + 1..start + 1 + end_rel];
        let callers = extract_caller_count(after_dash);
        let signature = extract_signature(after_dash);
        out.push(SymbolClaim {
            name: full.to_string(),
            callers,
            signature,
            raw: trimmed.to_string(),
        });
    }
    out
}

/// Extract the bullet's `signature \`<sig>\`` claim, or None. Tolerates the
/// word "signature" followed by either a backticked code span (preferred) or
/// bare text up to a trailing parenthetical / comma.
fn extract_signature(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let key = lower.find("signature")?;
    // Same offset in the original (single-byte word).
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
    // Walk words looking for "<int> caller(s)".
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

/// Every backticked token that looks like a path — directory separator OR file
/// extension. Deduplicated, stably ordered. audit-spec compares against
/// `git diff --name-only`.
fn extract_mentioned_files(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut chars = body.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '`' {
            continue;
        }
        // Matching closing backtick on the same line.
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
        // Skip past closing backtick.
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

/// `**VERIFY**: `cmd`` or `**VERIFY**: command-without-backticks`. Strips the
/// leading bold marker, returns the command text.
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
        let fence_len = after.bytes().take_while(|byte| *byte == b'`').count();
        let cmd = if fence_len == 0 {
            after
        } else {
            let fence = &after[..fence_len];
            let content = &after[fence_len..];
            content
                .match_indices(fence)
                .find(|(end, _)| {
                    (*end == 0 || content.as_bytes()[end - 1] != b'`')
                        && content.as_bytes().get(end + fence_len) != Some(&b'`')
                })
                .map(|(end, _)| &content[..end])
                .unwrap_or(after)
        };
        let cmd = cmd.trim();
        if !cmd.is_empty() {
            out.push(cmd.to_string());
        }
    }
    out
}

/// Parse phase FIND blocks.
///
/// Format from the planner skill's bundled `spec-template.md`:
/// - `## Phase 1: <name>` opens a phase
/// - `### 1.2 <action>` opens a sub-step
/// - `**File:** \`src/path.ext\`` sets the active target file
/// - `FIND:` opens a fenced block whose payload is the literal pattern to replace
///
/// Tracks the most recent phase heading + `**File:**` line, then on `FIND:`
/// followed by a fenced block emits a FindBlock with whatever context is active.
fn extract_find_blocks(body: &str) -> Vec<FindBlock> {
    let mut out: Vec<FindBlock> = Vec::new();
    let mut current_file: Option<String> = None;
    let mut current_phase: Option<String> = None;
    let mut lines = body.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();

        // Track phase headings — `## Phase N: ...` and `### N.M ...`.
        if let Some(rest) = trimmed.strip_prefix("## ") {
            if rest.to_lowercase().starts_with("phase ") {
                current_phase = Some(rest.trim().to_string());
                current_file = None; // file marker is scoped to a subsection
            } else {
                // Left phase territory — clear the trail.
                current_phase = None;
                current_file = None;
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            // Subphase inherits the parent phase label.
            if let Some(parent) = &current_phase {
                current_phase = Some(format!("{parent} / {}", rest.trim()));
            } else {
                current_phase = Some(rest.trim().to_string());
            }
            current_file = None;
            continue;
        }

        // `**File:** \`<path>\`` markers.
        if let Some(rest) = trimmed.strip_prefix("**File:**") {
            current_file = parse_backticked(rest.trim());
            continue;
        }

        // FIND: marker — next line opens a code fence; consume until closing
        // fence, payload is everything between.
        if trimmed == "FIND:" || trimmed == "**FIND:**" || trimmed == "**FIND**:" {
            let mut payload = String::new();
            // Skip blanks, then expect fence opener.
            let mut opened = false;
            while let Some(next) = lines.peek() {
                let nt = next.trim();
                if nt.is_empty() && !opened {
                    lines.next();
                    continue;
                }
                if !opened {
                    if nt.starts_with("```") {
                        opened = true;
                        lines.next();
                        continue;
                    }
                    break; // No fence after FIND: — abandon this block.
                }
                if nt.starts_with("```") {
                    lines.next();
                    break;
                }
                payload.push_str(next);
                payload.push('\n');
                lines.next();
            }
            let payload = payload.trim_end_matches('\n').to_string();
            if !payload.is_empty() {
                out.push(FindBlock {
                    file: current_file.clone(),
                    find_text: payload,
                    phase: current_phase.clone(),
                });
            }
        }
    }
    out
}

/// `\`<value>\`` → Some("value"); else None. For `**File:**` markers and
/// similar single-backticked-value patterns.
fn parse_backticked(s: &str) -> Option<String> {
    let s = s.trim();
    s.strip_prefix('`')
        .and_then(|r| r.strip_suffix('`'))
        .map(|v| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_file_rejects_specs_over_the_bundle_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("spec.md");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_SPEC_BYTES + 1).unwrap();
        drop(file);

        let error = parse_file(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("16777216-byte limit"));
    }

    #[cfg(unix)]
    #[test]
    fn parse_file_rejects_a_fifo_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("spec.md");
        let raw = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `raw` is a live, NUL-terminated path buffer.
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);

        let error = parse_file(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("regular file"));
    }

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
        // Backticked identifiers without path shape are NOT files.
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
    fn extracts_find_blocks_with_file_and_phase_context() {
        let body = "\
## Phase 1: Add accessor

### 1.1 Add session_count

**File:** `src/session.rs`

FIND:
```rust
pub fn refresh(&self) -> Result<Session> {
```

CHANGE TO:
```rust
pub fn session_count(&self) -> usize { ... }
```

### 1.2 Update tests

**File:** `tests/session_test.rs`

FIND:
```rust
fn old_test() {}
```
";
        let s = parse_str("t.md", body);
        assert_eq!(s.find_blocks.len(), 2);
        let first = &s.find_blocks[0];
        assert_eq!(first.file.as_deref(), Some("src/session.rs"));
        assert!(first.phase.as_deref().unwrap().contains("Phase 1"));
        assert!(first.find_text.contains("pub fn refresh"));
        let second = &s.find_blocks[1];
        assert_eq!(second.file.as_deref(), Some("tests/session_test.rs"));
        assert!(second.phase.as_deref().unwrap().contains("1.2"));
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
    fn frontmatter_absent_when_no_yaml_block() {
        let s = parse_str("t.md", "# Title\n## Goals\n- x\n");
        assert!(s.frontmatter.is_none());
        // Body still parses through the heuristic path.
        assert!(s.section_order.iter().any(|n| n.starts_with("Goals")));
    }

    #[test]
    fn frontmatter_parses_full_schema() {
        let body = "---
id: \"42\"
title: Add billing webhook
risk: high
touches:
  - file: src/billing/billing.controller.ts
    language: typescript
    symbols:
      - name: handleWebhook
        signature: \"async handleWebhook(req, res)\"
        callers: 4
verify:
  - typecheck
  - cmd: \"npm test -- billing\"
expected_docs:
  - README.md
  - docs/billing.md
breaking_changes:
  removed_symbols:
    - old_api
    - name: legacy_handler
      file: src/api/legacy.ts
      reason: \"deprecated since 2025-01\"
---

# Add billing webhook

## Goals
- Wire the new endpoint
";
        let s = parse_str("t.md", body);
        let fm = s.frontmatter.expect("frontmatter parsed");
        assert_eq!(fm.id.as_deref(), Some("42"));
        assert_eq!(fm.title.as_deref(), Some("Add billing webhook"));
        assert_eq!(fm.risk.as_deref(), Some("high"));
        assert_eq!(fm.touches.len(), 1);
        let t = &fm.touches[0];
        assert_eq!(t.file, "src/billing/billing.controller.ts");
        assert_eq!(t.language.as_deref(), Some("typescript"));
        assert_eq!(t.symbols.len(), 1);
        assert_eq!(t.symbols[0].name(), "handleWebhook");
        assert_eq!(t.symbols[0].callers(), Some(4));
        // Verify list is mixed: label + cmd object.
        assert_eq!(fm.verify.len(), 2);
        assert_eq!(fm.verify[0].label(), "typecheck");
        assert!(fm.verify[0].command().is_none());
        assert_eq!(fm.verify[1].command(), Some("npm test -- billing"));
        // Expected docs.
        assert_eq!(fm.expected_docs, vec!["README.md", "docs/billing.md"]);
        // Removed symbols — mixed string + object.
        assert_eq!(fm.breaking_changes.removed_symbols.len(), 2);
        assert_eq!(fm.breaking_changes.removed_symbols[0].name(), "old_api");
        assert_eq!(
            fm.breaking_changes.removed_symbols[1].name(),
            "legacy_handler"
        );
        assert_eq!(
            fm.breaking_changes.removed_symbols[1].file(),
            Some("src/api/legacy.ts")
        );
        // Body after frontmatter still parses.
        assert!(s.section_order.iter().any(|n| n.starts_with("Goals")));
    }

    #[test]
    fn frontmatter_with_partial_fields_uses_defaults() {
        let body = "---
id: \"7\"
touches:
  - file: src/x.rs
---

## Goals
- x
";
        let s = parse_str("t.md", body);
        let fm = s.frontmatter.expect("present");
        assert_eq!(fm.id.as_deref(), Some("7"));
        assert!(fm.title.is_none());
        assert!(fm.risk.is_none());
        assert!(fm.verify.is_empty());
        assert!(fm.expected_docs.is_empty());
        assert!(fm.breaking_changes.removed_symbols.is_empty());
        assert_eq!(fm.touches.len(), 1);
        assert!(fm.touches[0].symbols.is_empty());
    }

    #[test]
    fn malformed_frontmatter_retains_body_and_marks_contract_invalid() {
        let body = "---\nid: 42\ntitle: \"Unterminated\n\n## Goals\n- x\n";
        let s = parse_str("t.md", body);
        assert!(s.frontmatter.is_none());
        assert_eq!(
            s.frontmatter_error.as_deref(),
            Some("frontmatter_unterminated")
        );
        // No close fence → parser returns the original body, so heuristic
        // parsing still finds Goals.
        assert!(
            s.section_order.iter().any(|n| n.starts_with("Goals")),
            "diagnostics should retain sections on malformed frontmatter"
        );
    }

    #[test]
    fn creates_frontmatter_preserves_scope_and_invalid_metadata() {
        let parsed = parse_str("spec.md", "---\ncreates: [src/new.py, docs/new.md]\nexpected_docs: [docs/new.md]\n---\n## Goals\nCreate files.\n");
        assert!(parsed.frontmatter_error.is_none());
        let fm = parsed.frontmatter.unwrap();
        assert!(fm.has_file_scope());
        assert!(fm.touches.is_empty());
        assert_eq!(
            fm.code_paths().collect::<Vec<_>>(),
            ["src/new.py", "docs/new.md"]
        );
        assert_eq!(fm.expected_docs, ["docs/new.md"]);
        let legacy = parse_str("legacy.md", "# Task\n## Goals\nChange files.\n");
        assert!(legacy.frontmatter_error.is_none());
        for field in [
            "",
            "null",
            "creates: new.py",
            "creates: null",
            "creates: [null]",
            "creates: [true]",
            "creates: [7]",
            "creates: [{file: new.py}]",
            "creates: [new.py",
            "creates: [one.py]\ncreates: [two.py]",
            "verify: [\ncreates: [new.py]",
        ] {
            let parsed = parse_str(
                "spec.md",
                &format!("---\n{field}\n---\n## Goals\nCreate files.\n"),
            );
            assert_eq!(
                parsed.frontmatter_error.as_deref(),
                Some("frontmatter_invalid"),
                "{field}"
            );
            assert!(parsed.frontmatter.is_none());
            assert!(section_body(&parsed, "Goals").is_some());
        }
        let numeric_names = parse_str(
            "spec.md",
            "---\nid: 42\ncreates: ['7', 'true', 'null']\n---\n## Goals\nCreate files.\n",
        );
        assert!(numeric_names.frontmatter_error.is_none());
        let fm = numeric_names.frontmatter.unwrap();
        assert_eq!(fm.id.as_deref(), Some("42"));
        assert_eq!(fm.creates, ["7", "true", "null"]);
    }

    #[test]
    fn frontmatter_does_not_swallow_body_sections() {
        let body = "---
id: \"1\"
---

## Goals
- a real goal

## Tests Plan
- t
";
        let s = parse_str("t.md", body);
        assert!(s.frontmatter.is_some());
        assert!(s.section_order.iter().any(|n| n.starts_with("Goals")));
        assert!(s.section_order.iter().any(|n| n.starts_with("Tests Plan")));
    }

    #[test]
    fn snapshot_preserves_qualified_names() {
        let body = "## Pre-edit symbol snapshot\n\
                    - `pkg.module.foo` — 4 callers\n\
                    - `Type::method` — 2 callers\n";
        let s = parse_str("t.md", body);
        let names: Vec<&str> = s
            .pre_edit_snapshot
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["pkg.module.foo", "Type::method"]);
    }
}
