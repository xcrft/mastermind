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
//! - "Pre-edit snapshot" → `SymbolClaim { name, callers }` from bullets
//!   like ``- `session_count` — 8 callers (...) ``
//! - File paths — backticked path-like tokens (`*.rs`, `src/foo.ts`); audit-spec
//!   compares against `git diff --name-only`.
//! - VERIFY commands — `**VERIFY**: `cmd`` lines under phase bodies.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

const MAX_SPEC_BYTES: u64 = crate::audit_bundle::BUNDLE_INPUT_MAX as u64;
const MAX_FRONTMATTER_DEPTH: usize = 64;

#[derive(Debug, Serialize, Clone)]
pub struct ParsedSpec {
    pub path: String,
    /// All distinct raw `## Name` sections. Body runs until the next `##`
    /// header; subsections (`### …`) stay inside their parent.
    pub sections: BTreeMap<String, String>,
    /// First source occurrence for each normalized section name.
    pub section_order: Vec<String>,
    /// Normalized section names that appear more than once outside code fences.
    pub duplicate_section_keys: Vec<String>,
    /// Symbols the planner declared in "Pre-edit snapshot".
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
    /// Lines the gates read but could not parse. A dropped line would read as
    /// nothing to check, so every gate refuses the spec instead.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unparsed_lines: Vec<UnparsedLine>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct UnparsedLine {
    pub reason: &'static str,
    pub line: String,
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
            .filter(|cmd| is_command_declaration(cmd) && seen.insert(*cmd))
            .collect()
    }
}

fn is_command_declaration(command: &str) -> bool {
    !command.is_empty() && !command.starts_with('#')
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
#[serde(deny_unknown_fields)]
pub struct Frontmatter {
    #[serde(default, deserialize_with = "deserialize_frontmatter_id")]
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
    /// Every declared criterion requires all of its observed `verify[].run` IDs.
    /// Absence preserves legacy behavior; an explicit empty/null contract fails.
    #[serde(
        default,
        deserialize_with = "crate::acceptance::deserialize_contract",
        skip_serializing_if = "Option::is_none"
    )]
    pub acceptance: Option<Vec<crate::acceptance::Criterion>>,
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
#[serde(deny_unknown_fields)]
pub struct TouchEntry {
    pub file: String,
    /// File and deletion scopes are explicit alternatives to symbol snapshots.
    #[serde(default)]
    pub scope: TouchScope,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub symbols: Vec<SymbolSpec>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchScope {
    #[default]
    Symbols,
    File,
    Delete,
}

/// Polymorphic symbol — a bare name string (`- foo`) or a detailed object
/// (`- {name: foo, signature: "...", callers: 4}`). Untagged so YAML parses
/// both forms transparently.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
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
#[serde(untagged, deny_unknown_fields)]
pub enum VerifyEntry {
    Label(String),
    Command {
        cmd: String,
    },
    Observed {
        cmd: String,
        run: crate::verification_receipts::RunDeclaration,
    },
}

impl VerifyEntry {
    /// Runnable command (`cmd:` form), or None for label-only entries.
    pub fn command(&self) -> Option<&str> {
        match self {
            VerifyEntry::Command { cmd } | VerifyEntry::Observed { cmd, .. } => Some(cmd),
            VerifyEntry::Label(_) => None,
        }
    }
    /// Human-readable label (the string for Label, the cmd for Command).
    pub fn label(&self) -> &str {
        match self {
            VerifyEntry::Label(s) => s,
            VerifyEntry::Command { cmd } | VerifyEntry::Observed { cmd, .. } => cmd,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakingChanges {
    /// Symbols intentionally removed in this spec. Audit cross-references the
    /// git diff: a symbol removed but NOT listed here is flagged
    /// `RemovedSymbolNotAcknowledged` (Broken). Replaces the older
    /// lowercase-substring heuristic, which was fooled by incidental mentions
    /// like ``Do not remove `old_api` ``.
    #[serde(default)]
    pub removed_symbols: Vec<SymbolSpec>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FrontmatterId {
    Text(String),
    Signed(i64),
    Unsigned(u64),
}

fn deserialize_frontmatter_id<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(
        Option::<FrontmatterId>::deserialize(deserializer)?.map(|id| match id {
            FrontmatterId::Text(id) => id,
            FrontmatterId::Signed(id) => id.to_string(),
            FrontmatterId::Unsigned(id) => id.to_string(),
        }),
    )
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

fn spec_read_error(error: crate::bounded_fs::BoundedReadError) -> std::io::Error {
    match error {
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
    }
}

fn parse_file_bytes(path: &Path, bytes: Vec<u8>) -> std::io::Result<ParsedSpec> {
    let source_path = path.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "spec path must have an exact UTF-8 representation",
        )
    })?;
    let body = String::from_utf8(bytes)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "spec is not UTF-8"))?;
    Ok(parse_str(source_path, &body))
}

/// Parse one explicitly selected spec file from disk.
pub fn parse_file(path: &Path) -> std::io::Result<ParsedSpec> {
    let (resolved, source) = crate::bounded_fs::read_selected_regular_file(
        path,
        MAX_SPEC_BYTES,
        MAX_SPEC_BYTES,
        crate::bounded_fs::ReadControl::default(),
    )
    .map_err(spec_read_error)?;
    if path.canonicalize()? != resolved {
        return Err(std::io::Error::other(
            "spec path changed while it was being read",
        ));
    }
    parse_file_bytes(path, source.bytes)
}

/// Parse a repository-owned spec without allowing any path component to leave
/// the selected repository through a link.
#[doc(hidden)]
pub fn parse_repository_file(repo_root: &Path, path: &Path) -> std::io::Result<ParsedSpec> {
    let relative = if path.is_absolute() {
        path.strip_prefix(repo_root).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "spec must be inside the repository",
            )
        })?
    } else {
        path
    };
    let source = crate::bounded_fs::read_repository_file(
        repo_root,
        relative,
        MAX_SPEC_BYTES,
        MAX_SPEC_BYTES,
        crate::bounded_fs::ReadControl::default(),
    )
    .map_err(spec_read_error)?;
    parse_file_bytes(path, source.bytes)
}

pub fn parse_str(source_path: &str, body: &str) -> ParsedSpec {
    // Retain the readable body for diagnostics while gates reject bad metadata.
    let (frontmatter, frontmatter_error, body_after_fm) = extract_frontmatter(body);
    let (sections, order, duplicate_section_keys) = split_sections(body_after_fm);
    let mut unparsed_lines = Vec::new();
    let pre_edit_snapshot = order
        .iter()
        .filter(|name| is_snapshot_heading(name))
        .filter_map(|name| sections.get(name))
        .flat_map(|body| extract_snapshot(body, &mut unparsed_lines))
        .collect();
    let mentioned_files = extract_mentioned_files(body_after_fm);
    unparsed_lines.extend(nested_snapshot_headings(body_after_fm));
    let verify_commands = extract_verify_commands(body_after_fm, &mut unparsed_lines);
    let find_blocks = extract_find_blocks(body_after_fm, &mut unparsed_lines);

    ParsedSpec {
        path: source_path.to_string(),
        sections,
        section_order: order,
        duplicate_section_keys,
        pre_edit_snapshot,
        mentioned_files,
        verify_commands,
        find_blocks,
        frontmatter,
        frontmatter_error,
        unparsed_lines,
    }
}

/// `Pre-edit Snapshot`, `Pre-edit symbol snapshot`, `Pre edit snapshot:` or
/// any other heading whose words start with `pre edit` and name a snapshot,
/// after an optional section number.
fn is_snapshot_heading(name: &str) -> bool {
    let lower = name.to_lowercase();
    let words: Vec<_> = lower
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .skip_while(|word| word.bytes().all(|byte| byte.is_ascii_digit()))
        .collect();
    words.starts_with(&["pre", "edit"]) && words.contains(&"snapshot")
}

/// A snapshot heading that is not a top-level `## ` section, such as `###`,
/// `##` and a tab, a setext heading or one inside a list item, whose claims
/// no gate reads.
fn nested_snapshot_headings(body: &str) -> Vec<UnparsedLine> {
    let lines = crate::context_doctor::prose_lines(body);
    let underline = |index: usize| {
        lines.get(index + 1).is_some_and(|next| {
            let text = next.text.trim();
            next.index == lines[index].index + 1
                && !text.is_empty()
                && (text.bytes().all(|byte| byte == b'-') || text.bytes().all(|byte| byte == b'='))
        })
    };
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let raw = line.raw.trim_end_matches(['\r', '\n']);
            let heading = raw.trim_start_matches([' ', '\t']);
            let level = heading.bytes().take_while(|byte| *byte == b'#').count();
            let atx = (1..=6).contains(&level)
                && heading[level..].starts_with([' ', '\t'])
                && is_snapshot_heading(&heading[level..]);
            let setext = level == 0 && is_snapshot_heading(heading) && underline(index);
            let section = level == 2 && heading_line(raw).starts_with("## ");
            ((atx && !section) || setext).then(|| UnparsedLine {
                reason: "snapshot_heading_unparsed",
                line: line.text.trim().to_string(),
            })
        })
        .collect()
}

/// Markdown content after a leading YAML frontmatter block. Unterminated
/// frontmatter has no trustworthy Markdown body.
pub fn markdown_body(body: &str) -> &str {
    let (_, error, remainder) = extract_frontmatter(body);
    if error.as_deref() == Some("frontmatter_unterminated") {
        ""
    } else {
        remainder
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
        // A block after a BOM, a blank line or with a trailing space would
        // otherwise drop the whole contract to the legacy parser.
        let misplaced = body
            .trim_start_matches('\u{feff}')
            .lines()
            .find(|line| !line.trim().is_empty())
            .is_some_and(|line| line.trim() == "---");
        let error = misplaced.then(|| "frontmatter_not_at_start".to_string());
        return (None, error, body);
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
    // Preserve YAML scalar types before typed deserialization. Deserializing a
    // struct directly from YAML can coerce numbers and booleans into strings,
    // silently changing file, symbol, and verification declarations.
    let parsed = serde_norway::from_str::<serde_norway::Value>(yaml_src)
        .map_err(|_| ())
        .and_then(|value| strict_frontmatter_value(value, 0))
        .and_then(|value| serde_json::from_value::<Frontmatter>(value).map_err(|_| ()));
    match parsed {
        Ok(fm)
            if fm
                .mode
                .as_deref()
                .is_none_or(|mode| matches!(mode, "lite" | "standard" | "verified" | "strict"))
                && fm
                    .risk
                    .as_deref()
                    .is_none_or(|risk| matches!(risk, "low" | "medium" | "high"))
                && fm.touches.iter().all(|touch| match touch.scope {
                    TouchScope::Symbols => touch.reason.is_none(),
                    TouchScope::File | TouchScope::Delete => {
                        touch.symbols.is_empty()
                            && touch
                                .reason
                                .as_deref()
                                .is_some_and(|reason| !reason.trim().is_empty())
                    }
                }) =>
        {
            (Some(fm), None, rest)
        }
        Ok(_) => (None, Some("frontmatter_invalid".into()), rest),
        Err(_) => (None, Some("frontmatter_invalid".into()), rest),
    }
}

fn strict_frontmatter_value(
    value: serde_norway::Value,
    depth: usize,
) -> Result<serde_json::Value, ()> {
    use serde_norway::Value;

    if depth > MAX_FRONTMATTER_DEPTH {
        return Err(());
    }
    Ok(match value {
        // Optional contract fields are omitted when they are not declared.
        // Accepting an explicit null would silently erase a supplied scope,
        // signature, caller count, or workflow mode during deserialization.
        Value::Null => return Err(()),
        Value::Bool(value) => serde_json::Value::Bool(value),
        Value::Number(value) => {
            let value = serde_json::to_value(value).map_err(|_| ())?;
            if !value.is_number() {
                return Err(());
            }
            value
        }
        Value::String(value) => serde_json::Value::String(value),
        Value::Sequence(values) => serde_json::Value::Array(
            values
                .into_iter()
                .map(|value| strict_frontmatter_value(value, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Value::Mapping(values) => {
            let mut object = serde_json::Map::new();
            for (key, value) in values {
                let Value::String(key) = key else {
                    return Err(());
                };
                object.insert(key, strict_frontmatter_value(value, depth + 1)?);
            }
            serde_json::Value::Object(object)
        }
        Value::Tagged(_) => return Err(()),
    })
}

/// Whitespace-trimmed BODY of a named section. Case-insensitive lookup,
/// tolerates `*(MANDATORY ...)*` suffix annotations.
pub fn section_body<'a>(spec: &'a ParsedSpec, name: &str) -> Option<&'a str> {
    let want = section_key(name);
    spec.section_order
        .iter()
        .find(|section| section_key(section) == want)
        .and_then(|section| spec.sections.get(section))
        .map(|body| body.trim())
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

#[derive(Clone, Copy)]
struct CodeFence {
    marker: char,
    width: usize,
}

fn fence_opener(line: &str) -> Option<CodeFence> {
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let width = trimmed
        .chars()
        .take_while(|character| *character == marker)
        .count();
    (width >= 3).then_some(CodeFence { marker, width })
}

fn fence_closes(line: &str, fence: CodeFence) -> bool {
    let trimmed = line.trim_start();
    let width = trimmed
        .chars()
        .take_while(|character| *character == fence.marker)
        .count();
    width >= fence.width && trimmed.chars().skip(width).all(char::is_whitespace)
}

fn split_sections(body: &str) -> (BTreeMap<String, String>, Vec<String>, Vec<String>) {
    let mut sections: BTreeMap<String, String> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut seen_section_keys = HashSet::new();
    let mut duplicate_section_keys = Vec::new();
    let mut current: Option<(String, String)> = None;
    let prose_indexes: HashSet<usize> = crate::context_doctor::prose_lines(body)
        .into_iter()
        .map(|line| line.index)
        .collect();

    for (index, line) in body.lines().enumerate() {
        if !prose_indexes.contains(&index) {
            if let Some((_, body)) = current.as_mut() {
                body.push_str(line);
                body.push('\n');
            }
            continue;
        }
        if let Some(rest) = heading_line(line).strip_prefix("## ") {
            commit_section(
                current.take(),
                &mut sections,
                &mut order,
                &mut seen_section_keys,
                &mut duplicate_section_keys,
            );
            current = Some((rest.trim().to_string(), String::new()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
        // Lines before the first `##` ignored (frontmatter / preamble).
    }
    commit_section(
        current.take(),
        &mut sections,
        &mut order,
        &mut seen_section_keys,
        &mut duplicate_section_keys,
    );
    (sections, order, duplicate_section_keys)
}

/// `line` without the up to three spaces a heading may be indented by.
fn heading_line(line: &str) -> &str {
    let indent = line
        .bytes()
        .take(3)
        .take_while(|byte| *byte == b' ')
        .count();
    &line[indent..]
}

fn commit_section(
    section: Option<(String, String)>,
    sections: &mut BTreeMap<String, String>,
    order: &mut Vec<String>,
    seen_section_keys: &mut HashSet<String>,
    duplicate_section_keys: &mut Vec<String>,
) {
    let Some((name, body)) = section else {
        return;
    };
    let key = section_key(&name);
    if seen_section_keys.insert(key.clone()) {
        order.push(name.clone());
    } else if !duplicate_section_keys.contains(&key) {
        duplicate_section_keys.push(key);
    }
    sections.entry(name).or_insert(body);
}

/// `- \`name\` — 8 callers (...)` or `- \`name\` — added` list items.
/// Separator is em-dash (Mastermind convention) but plain `-` is tolerated.
/// The caller count is an integer up to four words before "caller", and the
/// signature is the code span right after "signature". Keywords inside code
/// spans are part of a name, not a claim.
///
/// A line that states a count or a backticked signature the parser cannot
/// read, states one without a symbol, or states claims for several symbols
/// is unparsed and yields no claim. Any other line is a planner note.
fn extract_snapshot(body: &str, unparsed: &mut Vec<UnparsedLine>) -> Vec<SymbolClaim> {
    let mut out: Vec<SymbolClaim> = Vec::new();
    for line in crate::context_doctor::prose_lines(body) {
        let trimmed = line.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        let item = strip_list_marker(trimmed);
        let text = item.unwrap_or(trimmed);
        let prose = outside_code_spans(text);
        // The symbol is a span before any `signature` keyword, so a
        // signature span is never read as the name.
        let head = &text[..prose
            .to_ascii_lowercase()
            .find("signature")
            .unwrap_or(text.len())];
        let name = item.and(first_code_span(head));
        let counts = caller_counts(&prose);
        let states_count = !counts.is_empty();
        let signatures = signatures(text, &prose);
        let states_signature = !signatures.is_empty();
        let reason = if !states_count && !states_signature {
            None
        } else if name.is_none()
            || counts.len() > 1
            || signatures.len() > 1
            || names_several_symbols(text, &prose)
        {
            Some("snapshot_line_unparsed")
        } else if counts.first().is_some_and(Option::is_none) {
            Some("snapshot_caller_count_unparsed")
        } else if signatures.first().is_some_and(Option::is_none) {
            Some("snapshot_signature_unparsed")
        } else {
            None
        };
        if let Some(reason) = reason {
            unparsed.push(UnparsedLine {
                reason,
                line: trimmed.to_string(),
            });
            continue;
        }
        let Some(name) = name else {
            continue;
        };
        out.push(SymbolClaim {
            name: name.to_string(),
            callers: counts.first().copied().flatten(),
            signature: signatures.first().copied().flatten().map(str::to_string),
            raw: trimmed.to_string(),
        });
    }
    out
}

/// One entry per `signature` keyword followed by a code span: the span when
/// only `**`, `:` or `=` separate them, None when a dash, arrow,
/// parenthetical or up to three words in the same clause do. `prose` is
/// `text` with code spans blanked.
fn signatures<'a>(text: &'a str, prose: &str) -> Vec<Option<&'a str>> {
    let lower = prose.to_ascii_lowercase();
    lower
        .match_indices("signature")
        .filter_map(|(key, word)| {
            let after = &text[key + word.len()..];
            let after = after.strip_prefix('s').unwrap_or(after);
            let canonical = after.trim_start_matches(['*', ' ', ':', '=']);
            if canonical.starts_with('`') {
                return Some(first_code_span(canonical));
            }
            let mut gap = canonical;
            if let Some(rest) = gap
                .trim_start_matches(['—', '–', '-', ' '])
                .strip_prefix('(')
            {
                gap = rest.split_once(')').map_or(gap, |(_, rest)| rest);
            }
            let (words, _) = gap.split_once('`')?;
            let clause = !words.contains([';', '.', ',']);
            let words = words
                .split_whitespace()
                .filter(|word| word.chars().any(char::is_alphanumeric))
                .count();
            (clause && words <= 3).then_some(None)
        })
        .collect()
}

/// Content of the code span `text` starts with, or of the first one in it
/// when `text` does not start with a backtick. A span closes on a run of the
/// same number of backticks.
fn first_code_span(text: &str) -> Option<&str> {
    next_code_span(text)
        .map(|(span, _)| span)
        .filter(|span| !span.is_empty())
}

/// Trimmed content of the first code span in `text` and the offset after it.
fn next_code_span(text: &str) -> Option<(&str, usize)> {
    let start = text.find('`')?;
    let rest = &text[start..];
    let width = rest.bytes().take_while(|byte| *byte == b'`').count();
    let content = &rest[width..];
    let fence = &rest[..width];
    let (end, _) = content.match_indices(fence).find(|(end, _)| {
        (*end == 0 || content.as_bytes()[end - 1] != b'`')
            && content.as_bytes().get(end + width) != Some(&b'`')
    })?;
    Some((content[..end].trim(), start + width + end + width))
}

/// `text` with every code span's content blanked byte for byte, so keyword
/// offsets still index `text`.
fn outside_code_spans(text: &str) -> String {
    let mut bytes = text.as_bytes().to_vec();
    // The opening run's offset and width.
    let mut open: Option<(usize, usize)> = None;
    let mut index = 0;
    loop {
        if index == bytes.len() {
            // A run that never closes is literal text, so scan again after it.
            let Some((start, width)) = open.take() else {
                break;
            };
            bytes[start..].copy_from_slice(&text.as_bytes()[start..]);
            index = start + width;
            continue;
        }
        if bytes[index] != b'`' {
            if open.is_some() {
                bytes[index] = b' ';
            }
            index += 1;
            continue;
        }
        let width = bytes[index..]
            .iter()
            .take_while(|byte| **byte == b'`')
            .count();
        open = match open {
            Some((_, opened)) if opened == width => None,
            Some(opened) => {
                bytes[index..index + width].fill(b' ');
                Some(opened)
            }
            None => Some((index, width)),
        };
        index += width;
    }
    String::from_utf8(bytes).expect("only ASCII bytes and whole characters change")
}

/// Claims for more than the item's first symbol: another symbol span before
/// the first `caller` or `signature` keyword, as in `` `a`, `b` — 2 callers ``,
/// or one followed by its own count, as in `` `a` — 1 caller; `b` — 2 ``.
/// File locations and spans that only name callers or parameters do not
/// count.
fn names_several_symbols(text: &str, prose: &str) -> bool {
    let lower = prose.to_ascii_lowercase();
    let claim = ["caller", "signature"]
        .into_iter()
        .filter_map(|keyword| lower.find(keyword))
        .min()
        .unwrap_or(text.len());
    let mut offset = 0;
    let mut symbols = 0;
    while let Some((span, end)) = next_code_span(&text[offset..]) {
        let start = offset + text[offset..].find('`').unwrap_or(0);
        offset += end;
        let location = is_location(span);
        let signature = prose[..start]
            .trim_end_matches([' ', ':', '=', '*'])
            .to_ascii_lowercase()
            .ends_with("signature");
        if span.is_empty() || location || signature {
            continue;
        }
        symbols += 1;
        let counted = text[offset..]
            .trim_start_matches([' ', '—', '–', '-', '(', ':', '='])
            .split_whitespace()
            .next()
            .is_some_and(|word| number_like(word.trim_end_matches([')', ',', ';', '.'])));
        if symbols > 1 && (start < claim || counted) {
            return true;
        }
    }
    // A later clause that opens with its own symbol and states a claim, as in
    // `` `a` — 3 callers; `b` — signature `fn b()` ``.
    let breaks = lower
        .match_indices(';')
        .map(|(at, _)| at + 1)
        .chain(lower.match_indices(". ").map(|(at, _)| at + 2));
    breaks.into_iter().any(|at| {
        let end = lower[at..]
            .find([';'])
            .into_iter()
            .chain(lower[at..].find(". "))
            .min()
            .map_or(lower.len(), |end| at + end);
        let clause = text[at..end].trim_start();
        let claims = lower[at..end].contains("caller") || lower[at..end].contains("signature");
        clause.starts_with('`')
            && claims
            && next_code_span(clause)
                .is_some_and(|(span, _)| !span.is_empty() && !is_location(span))
    })
}

/// A path or `file:line` span, which locates a symbol rather than naming one.
fn is_location(span: &str) -> bool {
    looks_like_path(span)
        || span
            .split_once(':')
            .is_some_and(|(_, after)| after.starts_with(|c: char| c.is_ascii_digit()))
}

/// Text after a `-`, `*`, `+`, `1.` or `1)` list marker, or None.
fn strip_list_marker(line: &str) -> Option<&str> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let rest = if digits > 0 {
        line[digits..].strip_prefix(['.', ')'])?
    } else {
        line.strip_prefix(['-', '*', '+'])?
    };
    rest.starts_with(char::is_whitespace).then(|| rest.trim())
}

/// A directive line such as `**FIND:**`, `FIND (exact):` or `- [ ] Verify: \`x\``.
struct Directive<'a> {
    word: &'a str,
    colon: bool,
    /// Written as a list item or task checkbox.
    listed: bool,
    rest: &'a str,
}

/// Read a directive after any list marker, task checkbox and bold markers.
fn directive(line: &str) -> Option<Directive<'_>> {
    let line = line.trim();
    let marked = strip_list_marker(line);
    let item = marked.unwrap_or(line);
    let checked = ["[ ]", "[x]", "[X]"]
        .into_iter()
        .find_map(|checkbox| item.strip_prefix(checkbox));
    let listed = marked.is_some() || checked.is_some();
    let item = checked.map_or(item, str::trim_start);
    let item = item.trim_start_matches('*');
    let length = item.bytes().take_while(u8::is_ascii_alphabetic).count();
    if length == 0 {
        return None;
    }
    let (word, rest) = item.split_at(length);
    let mut rest = rest.trim_start_matches('*').trim_start();
    if rest.starts_with('(') {
        // A `)` inside a code span does not close the qualifier.
        let close = outside_code_spans(rest).find(')');
        rest = close.map_or(rest, |at| &rest[at + 1..]);
        rest = rest.trim_start_matches('*').trim_start();
    }
    let colon = rest.starts_with(':');
    let rest = rest
        .strip_prefix(':')
        .unwrap_or(rest)
        .trim_start_matches('*');
    Some(Directive {
        word,
        colon,
        listed,
        rest,
    })
}

/// One entry per `caller` or `callers` word that states a count. The count
/// is the integer nearest before the word in its clause, or 0 for `no`. A
/// clause ends at `,`, `;`, `:`, a parenthesis, a dash or the previous such
/// word. A second number in that clause, an approximate, formatted or
/// spelled-out number, or any number in the three words after the word, as
/// in `callers: 4`, is None.
fn caller_counts(prose: &str) -> Vec<Option<u32>> {
    const BACK: [char; 7] = [',', ';', ':', '(', ')', '—', '–'];
    const AHEAD: [char; 4] = [',', ';', '.', ')'];
    let words = |text: &str| -> Vec<String> {
        text.split_whitespace()
            .map(|word| {
                word.trim_matches(['*', '.', ',', ';', ':', '(', ')'])
                    .to_string()
            })
            .filter(|word| !word.is_empty())
            .collect()
    };
    // `callers;`, `callers—two` and `(callers)` count. `caller-side` does not.
    let lower = prose.to_ascii_lowercase();
    let word_char = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_');
    let keywords: Vec<(usize, usize)> = lower
        .match_indices("caller")
        .filter_map(|(start, word)| {
            let end =
                start + word.len() + usize::from(lower[start + word.len()..].starts_with('s'));
            let open = !lower[..start].chars().next_back().is_some_and(word_char);
            let close = !lower[end..].chars().next().is_some_and(word_char);
            (open && close).then_some((start, end))
        })
        .collect();
    let mut previous_end = 0;
    let mut out = Vec::new();
    for (index, &(start, end)) in keywords.iter().enumerate() {
        let before = &prose[previous_end..start];
        // A comma or dash between digits, as in `1,234` or `3–5`, does not
        // end the clause.
        let digit_at = |at: usize| before[at..].starts_with(|c: char| c.is_ascii_digit());
        let clause = before
            .char_indices()
            .filter(|&(at, c)| {
                let inside = at > 0
                    && before[..at].ends_with(|c: char| c.is_ascii_digit())
                    && digit_at(at + c.len_utf8());
                BACK.contains(&c) && !inside
            })
            .map(|(at, c)| at + c.len_utf8())
            .chain(before.rfind(". ").map(|at| at + 2))
            .max()
            .unwrap_or(0);
        let before = words(&before[clause..]);
        let next = keywords
            .get(index + 1)
            .map_or(prose.len(), |&(next, _)| next);
        let after = prose[end..next].trim_start_matches([' ', '(', '—', '–', ':', '=', '-']);
        let after = words(&after[..after.find(AHEAD).unwrap_or(after.len())]);
        previous_end = end;
        let numbers = before.iter().filter(|word| number_like(word)).count();
        let read = before.iter().rev().find_map(|word| {
            if word.eq_ignore_ascii_case("no") {
                Some(0)
            } else {
                word.parse().ok()
            }
        });
        match read {
            Some(count) if numbers <= 1 => out.push(Some(count)),
            Some(_) => out.push(None),
            None if numbers > 0 || after.iter().take(3).any(|word| number_like(word)) => {
                out.push(None)
            }
            None => {}
        }
    }
    out
}

/// `40`, `~40`, `40+`, `1,234`, `12k`, `3–5` or a number word.
fn number_like(word: &str) -> bool {
    const WORDS: &str = "zero one two three four five six seven eight nine ten eleven twelve \
        thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty thirty forty fifty \
        sixty seventy eighty ninety hundred dozen none";
    if WORDS
        .split_whitespace()
        .any(|known| known.eq_ignore_ascii_case(word))
    {
        return true;
    }
    let digits = word
        .trim_start_matches(['~', '≈'])
        .trim_end_matches(['+', 'k', 'K']);
    // One range separator at most, so a date is not a count.
    digits.starts_with(|c: char| c.is_ascii_digit())
        && digits
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ',' | '–' | '-'))
        && digits.matches(['–', '-']).count() <= 1
}

/// Every backticked token that looks like a path — directory separator OR file
/// extension. Deduplicated, stably ordered. audit-spec compares against
/// `git diff --name-only`.
fn extract_mentioned_files(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in crate::context_doctor::prose_lines(body) {
        extract_mentioned_files_from_line(line.text, &mut out, &mut seen);
    }
    out
}

fn extract_mentioned_files_from_line(
    line: &str,
    out: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '`' {
            continue;
        }
        // Matching closing backtick on the same line.
        let rest = &line[i + 1..];
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
        // `find` returns a byte offset while `char_indices` advances by Unicode
        // scalar values. Compare absolute byte positions so a non-ASCII span
        // cannot consume the opening backtick of the next token.
        let closing_index = i + 1 + end_rel;
        while chars
            .peek()
            .is_some_and(|(index, _)| *index <= closing_index)
        {
            chars.next();
        }
    }
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

/// `**VERIFY**: \`cmd\``, `VERIFY: cmd` or `Verify: \`cmd\``. An upper-case
/// marker on its own line declares the text after its colon. A list item,
/// checkbox or another case declares only a backticked command, so a
/// checklist entry or prose `Verify:` note stays prose. An upper-case marker
/// with nothing after it, or a line with several or unclosed code spans, is
/// unparsed.
fn extract_verify_commands(body: &str, unparsed: &mut Vec<UnparsedLine>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in crate::context_doctor::prose_lines(body) {
        let Some(marker) = directive(line.text) else {
            continue;
        };
        let after = marker.rest.trim();
        let bare = marker.word == "VERIFY"
            && !marker.colon
            && !marker.listed
            && after
                .trim_start_matches(['—', '–', '-', ' '])
                .starts_with('`');
        if bare {
            unparsed.push(UnparsedLine {
                reason: "verify_command_unparsed",
                line: line.text.trim().to_string(),
            });
            continue;
        }
        let declared = marker.colon
            && marker.word.eq_ignore_ascii_case("verify")
            && (after.starts_with('`') || (marker.word == "VERIFY" && !marker.listed));
        if !declared {
            continue;
        }
        let several = after.starts_with('`')
            && next_code_span(after)
                .is_some_and(|(_, end)| first_code_span(&after[end..]).is_some());
        let unclosed = after.starts_with('`') && first_code_span(after).is_none();
        if several || unclosed || (marker.word == "VERIFY" && after.is_empty()) {
            unparsed.push(UnparsedLine {
                reason: "verify_command_unparsed",
                line: line.text.trim().to_string(),
            });
            continue;
        }
        let cmd = if after.starts_with('`') {
            first_code_span(after).unwrap_or(after)
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
/// An upper-case FIND with text after its colon (outside a checklist), inline
/// code, plain words above a fence, an empty fence or no fence is unparsed. Another case counts only as a marker
/// followed by a fence, with or without a colon.
fn extract_find_blocks(body: &str, unparsed: &mut Vec<UnparsedLine>) -> Vec<FindBlock> {
    let mut out: Vec<FindBlock> = Vec::new();
    let mut current_file: Option<String> = None;
    let mut current_phase: Option<String> = None;
    let prose_indexes: HashSet<usize> = crate::context_doctor::prose_lines(body)
        .into_iter()
        .map(|line| line.index)
        .collect();
    let mut lines = body.lines().enumerate().peekable();

    while let Some((index, line)) = lines.next() {
        if !prose_indexes.contains(&index) {
            continue;
        }
        let trimmed = line.trim();
        let heading = heading_line(line);

        // Track phase headings — `## Phase N: ...` and `### N.M ...`.
        if let Some(rest) = heading.strip_prefix("## ") {
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
        if let Some(rest) = heading.strip_prefix("### ") {
            // Subphase inherits the parent phase label.
            if let Some(parent) = &current_phase {
                current_phase = Some(format!("{parent} / {}", rest.trim()));
            } else {
                current_phase = Some(rest.trim().to_string());
            }
            current_file = None;
            continue;
        }

        let Some(marker) = directive(trimmed) else {
            continue;
        };
        // An unreadable File value clears the target, so a later FIND cannot
        // be checked against the previous file.
        if marker.word.eq_ignore_ascii_case("file") && marker.colon {
            current_file = parse_backticked(marker.rest.trim());
            continue;
        }
        if !marker.word.eq_ignore_ascii_case("find") {
            continue;
        }
        let upper = marker.word == "FIND";
        let inline = marker.rest.trim();
        let fenced = lines
            .clone()
            .map(|(_, next)| next)
            .find(|next| !next.trim().is_empty())
            .is_some_and(|next| fence_opener(next).is_some());
        if !inline.is_empty() || !(upper || marker.colon || fenced) {
            // Inline code, or plain words above a fence, is a FIND the
            // parser cannot bind to its block.
            if upper
                && !inline.is_empty()
                && ((marker.colon && !marker.listed) || inline.contains('`') || fenced)
            {
                unparsed.push(UnparsedLine {
                    reason: "find_block_unparsed",
                    line: trimmed.to_string(),
                });
            }
            continue;
        }
        // Skip blanks, then expect a fence opener. Payload lines lose the
        // opener's indentation, as in a fence inside a list item.
        let mut payload = String::new();
        let mut opened = None;
        while let Some((_, next)) = lines.peek() {
            if next.trim().is_empty() && opened.is_none() {
                lines.next();
                continue;
            }
            if let Some((fence, indent, opener_column)) = opened {
                let lead = next.len() - next.trim_start_matches([' ', '\t']).len();
                // Four columns past the opener is content, not a closer.
                let column = crate::context_doctor::width(&next[..lead], 0);
                if column <= opener_column + 3 && fence_closes(next, fence) {
                    lines.next();
                    break;
                }
                payload.push_str(&next[lead.min(indent)..]);
                payload.push('\n');
                lines.next();
            } else {
                if let Some(fence) = fence_opener(next) {
                    let indent = next.len() - next.trim_start_matches([' ', '\t']).len();
                    let column = crate::context_doctor::width(&next[..indent], 0);
                    opened = Some((fence, indent, column));
                    lines.next();
                    continue;
                }
                break;
            }
        }
        let payload = payload.trim_end_matches('\n');
        if payload.trim().is_empty() {
            if upper || opened.is_some() {
                unparsed.push(UnparsedLine {
                    reason: "find_block_unparsed",
                    line: trimmed.to_string(),
                });
            }
            continue;
        }
        out.push(FindBlock {
            file: current_file.clone(),
            find_text: payload.to_string(),
            phase: current_phase.clone(),
        });
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
    fn parsed_spec_rejects_an_inexact_source_identity() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let path = std::path::PathBuf::from(OsString::from_vec(b"spec-\xff.md".to_vec()));
        let error = parse_file_bytes(&path, b"# Exact identity\n".to_vec()).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("exact UTF-8"));
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

    #[cfg(unix)]
    #[test]
    fn repository_parser_rejects_a_linked_spec() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_spec = outside.path().join("spec.md");
        std::fs::write(&outside_spec, "# Outside\n").unwrap();
        let task = root.path().join(".mastermind/tasks/001-linked");
        std::fs::create_dir_all(&task).unwrap();
        let linked_spec = task.join("spec.md");
        symlink(&outside_spec, &linked_spec).unwrap();

        let error = parse_repository_file(root.path(), &linked_spec).unwrap_err();
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
    fn extracts_path_after_unicode_code_span() {
        let s = parse_str("test.md", "## Goals\n`💥``src/after.rs`\n");
        assert_eq!(s.mentioned_files, vec![String::from("src/after.rs")]);
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
    fn ignores_shell_comments_as_verify_commands() {
        let s = parse_str(
            "test.md",
            "---\nverify:\n  - cmd: \"# deferred\"\n---\n## Goals\n- Real work\nVERIFY: # also deferred\n",
        );
        assert!(s.declared_verify_commands().is_empty());
    }

    #[test]
    fn ignores_semantic_markers_inside_fenced_code() {
        let body = "\
## Goals
- Real outcome

<!--
## Scope
- Fake scope in a comment
VERIFY: `false`
**File:** `src/comment.rs`
FIND:
```rust
fn comment_decoy() {}
```
-->

~~~markdown
## Scope
- Fake scope in an example
VERIFY: `false`
**File:** `src/decoy.rs`
FIND:
```rust
fn decoy() {}
```
~~~

> VERIFY: `false`
> `src/quoted.rs`
    VERIFY: `false`
    `src/indented.rs`

## Scope
- Edit `src/real.rs`

## Pre-edit symbol snapshot
~~~text
- `decoy` — 99 callers
~~~
<!--
- `comment_decoy` — 88 callers
-->
- `real` — 1 caller
> - `quoted_decoy` — 77 callers
    - `indented_decoy` — 66 callers
";
        let s = parse_str("test.md", body);

        assert_eq!(section_body(&s, "Scope"), Some("- Edit `src/real.rs`"));
        assert_eq!(
            s.section_order,
            vec!["Goals", "Scope", "Pre-edit symbol snapshot"]
        );
        assert_eq!(s.mentioned_files, vec!["src/real.rs"]);
        assert!(s.verify_commands.is_empty());
        assert!(s.find_blocks.is_empty());
        assert_eq!(s.pre_edit_snapshot.len(), 1);
        assert_eq!(s.pre_edit_snapshot[0].name, "real");
    }

    #[test]
    fn records_normalized_duplicate_section_headers() {
        let s = parse_str(
            "test.md",
            "## Goals\n- First outcome\n## Goals *(MANDATORY)*\n- Second outcome\n",
        );

        assert_eq!(s.duplicate_section_keys, vec!["goals"]);
        assert_eq!(section_body(&s, "Goals"), Some("- First outcome"));
    }

    #[test]
    fn markdown_body_excludes_frontmatter_comments() {
        let body =
            "---\n# Metadata comment, not a Markdown title\nmode: verified\n---\n# Actual title\n";

        assert_eq!(markdown_body(body), "# Actual title\n");
        assert_eq!(markdown_body("---\n# Unterminated metadata"), "");
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
    fn signature_offset_survives_unicode_before_keyword() {
        let parsed = parse_str(
            "test.md",
            "## Pre-edit symbol snapshot\n- `target` — İİİ SIGNATURE: `fn target()`\n",
        );
        assert_eq!(
            parsed.pre_edit_snapshot[0].signature.as_deref(),
            Some("fn target()")
        );
    }

    #[test]
    fn frontmatter_absent_when_no_yaml_block() {
        let s = parse_str("t.md", "# Title\n## Goals\n- x\n");
        assert!(s.frontmatter.is_none());
        // Body still parses through the heuristic path.
        assert!(s.section_order.iter().any(|n| n.starts_with("Goals")));
    }

    #[test]
    fn a_frontmatter_block_not_at_the_first_byte_is_an_error() {
        for body in [
            "\n---\nmode: strict\n---\n## Goals\n- x\n",
            "\u{feff}---\nmode: strict\n---\n## Goals\n- x\n",
            "--- \nmode: strict\n---\n## Goals\n- x\n",
            "\r\n---\r\nmode: strict\r\n---\r\n## Goals\r\n- x\r\n",
            "\u{feff}\n---\nmode: strict\n---\n## Goals\n- x\n",
        ] {
            let parsed = parse_str("t.md", body);
            assert!(parsed.frontmatter.is_none(), "{body:?}");
            assert_eq!(
                parsed.frontmatter_error.as_deref(),
                Some("frontmatter_not_at_start"),
                "{body:?}"
            );
        }
        let placed = parse_str("t.md", "---\nmode: strict\n---\n## Goals\n- x\n");
        assert!(placed.frontmatter_error.is_none());
        assert_eq!(placed.frontmatter.unwrap().mode.as_deref(), Some("strict"));
    }

    #[test]
    fn snapshot_lines_outside_the_template_form_are_unparsed() {
        let body = "\
## Pre-edit Snapshot (from mmcg)
* `star` — 3 callers
+ `plus` — 1 indexed caller
1) `numbered` — 39 pooled indexed callers; signature ``fn numbered(x: &[u8])``
- `get_caller` — added
- `verify_signature` — 2 callers; signature unchanged
- `added` — added in this spec
- [ ] `checked` — **4** callers
1. Step
    - `nested` — callers: 2
- plain — 3 callers
- `spaced` — callers: 9
- `doubled` — 1 caller; signature `` `
- `first`, `second` — 2 callers
-`tight` — 5 callers
- `deep` — 39 indexed direct containing callers
- `approx` — ~40 callers
- `formatted` — 1,234 callers
- `use` — 0 callers, `helper` — 39 callers
- `validate_report` (28 callers), `read_reports` (7)
- `arrow` — 2 callers; signature → `fn arrow()`
- `later` — 2 callers; signatures stay. New signature: `fn later()`
- `located` in `src/a.rs:12` — 3 callers
- `listed` — 2 callers (`evaluate`, `run`); its `now` parameter is kept
- `unused` — no callers
- No caller-count inference was made for `inferred`.
- `wordy` — 2 callers; signature is `fn wrong()`
- `kept` — signature unchanged: `fn wrong()`
- `tick` — handles the ` character, 9 callers
- `open — 3 callers
- signature `fn nameless()`
- `gamma` (line 120) — callers: 4
- `delta` — 1 file, callers: 4
- `alpha` — 3 callers, all callers in src/cli
- `zeta` — 2 callers; caller-side change needed
- `beta` — signature `fn beta()` — 3 callers
- `eps` — no direct callers
- `far` — 5 indexed direct non-test production callers
- `spread` — 3 callers in 2 files
- `omega` — 3 callers and test callers
- `first` — 3 callers; `second` — signature `fn second()`
- `sig_first` — signature `fn sig_first()`; `later` has 3 callers
- `glued` — 3 callers;signature `fn glued()`
- `slash` — 3 callers/2 callees
- `glued_count` — callers:4
- `glued_open` —callers=4
- `paren` — callers (4): `a`, `b`
- `dash` — callers — 4
- `range` — 3–5 callers
- `spelled_range` — 3 to 5 callers
- The converted functions keep their signatures.
| `table` | 3 |
Callers were read from source at 1556c27 because `mmcg_brief` was stale.
`prose` — 2 callers
<!-- planner note -->
";
        let parsed = parse_str("t.md", body);
        let claims: Vec<_> = parsed
            .pre_edit_snapshot
            .iter()
            .map(|claim| {
                (
                    claim.name.as_str(),
                    claim.callers,
                    claim.signature.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            claims,
            [
                ("star", Some(3), None),
                ("plus", Some(1), None),
                ("numbered", Some(39), Some("fn numbered(x: &[u8])")),
                ("get_caller", None, None),
                ("verify_signature", Some(2), None),
                ("added", None, None),
                ("checked", Some(4), None),
                ("deep", Some(39), None),
                ("later", Some(2), Some("fn later()")),
                ("located", Some(3), None),
                ("listed", Some(2), None),
                ("unused", Some(0), None),
                ("inferred", None, None),
                ("tick", Some(9), None),
                ("alpha", Some(3), None),
                ("zeta", Some(2), None),
                ("beta", Some(3), Some("fn beta()")),
                ("eps", Some(0), None),
                ("far", Some(5), None),
                ("spread", Some(3), None),
                ("omega", Some(3), None),
                ("glued", Some(3), Some("fn glued()")),
                ("slash", Some(3), None),
            ]
        );
        let unparsed: Vec<_> = parsed
            .unparsed_lines
            .iter()
            .map(|line| (line.reason, line.line.as_str()))
            .collect();
        assert_eq!(
            unparsed,
            [
                ("snapshot_caller_count_unparsed", "- `nested` — callers: 2"),
                ("snapshot_line_unparsed", "- plain — 3 callers"),
                ("snapshot_caller_count_unparsed", "- `spaced` — callers: 9"),
                (
                    "snapshot_signature_unparsed",
                    "- `doubled` — 1 caller; signature `` `"
                ),
                ("snapshot_line_unparsed", "- `first`, `second` — 2 callers"),
                ("snapshot_line_unparsed", "-`tight` — 5 callers"),
                ("snapshot_caller_count_unparsed", "- `approx` — ~40 callers"),
                (
                    "snapshot_caller_count_unparsed",
                    "- `formatted` — 1,234 callers"
                ),
                (
                    "snapshot_line_unparsed",
                    "- `use` — 0 callers, `helper` — 39 callers"
                ),
                (
                    "snapshot_line_unparsed",
                    "- `validate_report` (28 callers), `read_reports` (7)"
                ),
                (
                    "snapshot_signature_unparsed",
                    "- `arrow` — 2 callers; signature → `fn arrow()`"
                ),
                (
                    "snapshot_signature_unparsed",
                    "- `wordy` — 2 callers; signature is `fn wrong()`"
                ),
                (
                    "snapshot_signature_unparsed",
                    "- `kept` — signature unchanged: `fn wrong()`"
                ),
                ("snapshot_line_unparsed", "- `open — 3 callers"),
                ("snapshot_line_unparsed", "- signature `fn nameless()`"),
                (
                    "snapshot_caller_count_unparsed",
                    "- `gamma` (line 120) — callers: 4"
                ),
                (
                    "snapshot_caller_count_unparsed",
                    "- `delta` — 1 file, callers: 4"
                ),
                (
                    "snapshot_line_unparsed",
                    "- `first` — 3 callers; `second` — signature `fn second()`"
                ),
                (
                    "snapshot_line_unparsed",
                    "- `sig_first` — signature `fn sig_first()`; `later` has 3 callers"
                ),
                (
                    "snapshot_caller_count_unparsed",
                    "- `glued_count` — callers:4"
                ),
                (
                    "snapshot_caller_count_unparsed",
                    "- `glued_open` —callers=4"
                ),
                (
                    "snapshot_caller_count_unparsed",
                    "- `paren` — callers (4): `a`, `b`"
                ),
                ("snapshot_caller_count_unparsed", "- `dash` — callers — 4"),
                ("snapshot_caller_count_unparsed", "- `range` — 3–5 callers"),
                (
                    "snapshot_caller_count_unparsed",
                    "- `spelled_range` — 3 to 5 callers"
                ),
                ("snapshot_line_unparsed", "`prose` — 2 callers"),
            ]
        );
    }

    #[test]
    fn snapshot_headings_are_matched_by_their_words() {
        for heading in [
            "## Pre-edit Snapshot",
            "## Pre-edit symbol snapshot",
            "## Pre-edit snapshot *(filled by planner via mmcg)*",
            "## Pre edit snapshot:",
            "## **Pre-edit Snapshot**",
            "  ## Pre-edit Snapshot",
            "## 4. Pre-edit Snapshot",
        ] {
            let parsed = parse_str("t.md", &format!("{heading}\n- `helper` — 2 callers\n"));
            assert_eq!(parsed.pre_edit_snapshot.len(), 1, "{heading}");
        }
        let unrelated = parse_str("t.md", "## Snapshot tests\n- `helper` — 2 callers\n");
        assert!(unrelated.pre_edit_snapshot.is_empty());
        let nested = parse_str(
            "t.md",
            "## Phase 1: Edit\n### Pre-edit Snapshot\n- `helper` — 2 callers\n### Snapshot tests\n- Fix it.\n\n    ## Pre-edit Snapshot\n\n##\tPre-edit Snapshot\n\nPre-edit Snapshot\n---\n",
        );
        let unparsed: Vec<_> = nested
            .unparsed_lines
            .iter()
            .map(|line| (line.reason, line.line.as_str()))
            .collect();
        assert_eq!(
            unparsed,
            [
                ("snapshot_heading_unparsed", "### Pre-edit Snapshot"),
                ("snapshot_heading_unparsed", "## Pre-edit Snapshot"),
                ("snapshot_heading_unparsed", "##\tPre-edit Snapshot"),
                ("snapshot_heading_unparsed", "Pre-edit Snapshot"),
            ]
        );
        let split = parse_str(
            "t.md",
            "## Pre-edit Snapshot\n- `a` — 1 caller\n## Notes\n- `n` — 5 callers\n## Pre-edit snapshot (tests)\n- `b` — 2 callers\n",
        );
        let names: Vec<_> = split
            .pre_edit_snapshot
            .iter()
            .map(|claim| claim.name.as_str())
            .collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn a_heading_inside_a_list_item_keeps_the_find_target() {
        let body = "## Phase 1: Edit\n**File:** `a.py`\n- Usage notes\n    ## Usage\nFIND:\n```python\ndef a():\n```\n";
        let parsed = parse_str("t.md", body);
        assert_eq!(parsed.find_blocks[0].file.as_deref(), Some("a.py"));
        assert!(parsed.sections.contains_key("Phase 1: Edit"));
        assert_eq!(parsed.section_order, ["Phase 1: Edit"]);
    }

    #[test]
    fn find_markers_in_other_forms_are_unparsed() {
        let body = "\
## Phase 1: Edit
**File:** `a.py`
FIND:
Not a fence.
FIND: `inline`
**FIND** in `a.py`:
FIND:
```
```
FIND: blocks below are optional.
Find `helper` and rename it.
Find:
then prose
1. FIND:
   ```python
   def listed():
       return 1
   ```
2. Step

    **Find**:
    ```python
    def nested():
    ```
- [ ] **FIND (exact):**
  ```python
  def checked():
  ```
- **File**: `b.py`
FIND :
```python
def b():
```
FIND in b.py:

```python
def plain():
```
**File:** b.py
**FIND:**
```python
def c():
```
3. FIND:
\t```python
\tdef tabbed():
\t```
**Find**
```python
def bold():
```
**Find** (inside `run()`):
```markdown
1. Run:
    ```bash
    cargo test
    ```
NOT IN FILE
```
FIND:
```
fn a() {
\t```
not_in_file();
```
";
        let parsed = parse_str("t.md", body);
        let blocks: Vec<_> = parsed
            .find_blocks
            .iter()
            .map(|block| (block.file.as_deref(), block.find_text.as_str()))
            .collect();
        assert_eq!(
            blocks,
            [
                (Some("a.py"), "def listed():\n    return 1"),
                (Some("a.py"), "def nested():"),
                (Some("a.py"), "def checked():"),
                (Some("b.py"), "def b():"),
                (None, "def c():"),
                (None, "def tabbed():"),
                (None, "def bold():"),
                (
                    None,
                    "1. Run:\n    ```bash\n    cargo test\n    ```\nNOT IN FILE"
                ),
                (None, "fn a() {\n\t```\nnot_in_file();"),
            ]
        );
        let unparsed: Vec<_> = parsed
            .unparsed_lines
            .iter()
            .map(|line| (line.reason, line.line.as_str()))
            .collect();
        assert_eq!(
            unparsed,
            [
                ("find_block_unparsed", "FIND:"),
                ("find_block_unparsed", "FIND: `inline`"),
                ("find_block_unparsed", "**FIND** in `a.py`:"),
                ("find_block_unparsed", "FIND:"),
                ("find_block_unparsed", "FIND: blocks below are optional."),
                ("find_block_unparsed", "FIND in b.py:"),
            ]
        );
    }

    #[test]
    fn verify_markers_are_read_inside_list_items() {
        let body = "\
## Phase 1: Edit
- **VERIFY**: `cargo test listed`
1. **VERIFY:** `cargo test numbered`
- [ ] VERIFY: cargo test checked
- [ ] Verify `--json` output is stable
Verify: `cargo test mixed`
**VERIFY** `cargo test bare`
1. Run checks

    **VERIFY**: `cargo test nested`
- Verify: the page still renders after reload
**VERIFY**:
VERIFY: `cargo test one` and `cargo test two`
VERIFY: `cargo test unclosed
VERIFY (covers `parse()`): `cargo test qualified`
";
        let parsed = parse_str("t.md", body);
        let unparsed: Vec<_> = parsed
            .unparsed_lines
            .iter()
            .map(|line| (line.reason, line.line.as_str()))
            .collect();
        assert_eq!(
            unparsed,
            [
                ("verify_command_unparsed", "**VERIFY** `cargo test bare`"),
                ("verify_command_unparsed", "**VERIFY**:"),
                (
                    "verify_command_unparsed",
                    "VERIFY: `cargo test one` and `cargo test two`"
                ),
                ("verify_command_unparsed", "VERIFY: `cargo test unclosed"),
            ]
        );
        assert_eq!(
            parsed.verify_commands,
            [
                "cargo test listed",
                "cargo test numbered",
                "cargo test mixed",
                "cargo test nested",
                "cargo test qualified",
            ]
        );
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
    fn deletion_scope_requires_reason_and_no_symbol_snapshots() {
        let body = "---\ntouches:\n  - file: assets/old.bin\n    scope: delete\n    reason: Remove unused asset\n---\n## Goals\nRemove asset.\n";
        let parsed = parse_str("spec.md", body);
        assert!(parsed.frontmatter_error.is_none());
        let fm = parsed.frontmatter.unwrap();
        assert!(fm.has_file_scope());
        assert_eq!(fm.code_paths().collect::<Vec<_>>(), ["assets/old.bin"]);
        assert_eq!(fm.touches[0].scope, TouchScope::Delete);
        assert_eq!(
            serde_json::to_value(&fm.touches[0]).unwrap()["scope"],
            "delete"
        );
        for metadata in [
            "",
            "reason: '  '",
            "reason: null",
            "reason: 7",
            "reason: Remove API\n    symbols: [old_api]",
        ] {
            let parsed = parse_str("spec.md", &format!("---\ntouches:\n  - file: old.bin\n    scope: delete\n    {metadata}\n---\n## Goals\nRemove asset.\n"));
            assert_eq!(
                parsed.frontmatter_error.as_deref(),
                Some("frontmatter_invalid"),
                "{metadata}"
            );
        }
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
    fn frontmatter_rejects_unknown_fields_and_invalid_contract_values() {
        for field in [
            "mode: strcit",
            "risk: urgent",
            "id: null",
            "title: null",
            "mode: null",
            "title: 42",
            "id: true",
            "id: 4.2",
            "mode: true",
            "risk: 7",
            "touchess: []",
            "touches:\n  - file: 7",
            "touches:\n  - file: README.md\n    scope: whole_file\n    reason: Update documentation",
            "touches:\n  - file: README.md\n    scope: file",
            "touches:\n  - file: README.md\n    scope: file\n    reason: '  '",
            "touches:\n  - file: README.md\n    scope: file\n    reason: 42",
            "touches:\n  - file: README.md\n    scope: null",
            "touches:\n  - file: README.md\n    scope: true",
            "touches:\n  - file: src/x.rs\n    scope: file\n    reason: Change file\n    symbols: [target]",
            "touches:\n  - file: src/x.rs\n    scope: symbols\n    reason: Change file\n    symbols: [target]",
            "touches:\n  - file: src/x.rs\n    reason: Change file\n    symbols: [target]",
            "touches:\n  - file: src/x.rs\n    language: false",
            "touches:\n  - file: src/x.rs\n    language: null",
            "touches:\n  - file: src/x.rs\n    symbols:\n      - name: false",
            "touches:\n  - file: src/x.rs\n    symbols:\n      - name: target\n        signature: null",
            "touches:\n  - file: src/x.rs\n    symbols:\n      - name: target\n        callers: '4'",
            "touches:\n  - file: src/x.rs\n    symbolz: []",
            "touches:\n  - file: src/x.rs\n    symbols:\n      - name: target\n        callerz: 1",
            "verify:\n  - 7",
            "verify:\n  - cmd: true",
            "verify:\n  - cmd: echo checked\n    result: pass",
            "expected_docs: [true]",
            "breaking_changes:\n  removed_symbols: [7]",
            "breaking_changes:\n  removed_symbols:\n    - name: old_api\n      reason: null",
            "breaking_changes:\n  removed_symbolz: []",
        ] {
            let parsed = parse_str(
                "spec.md",
                &format!("---\n{field}\n---\n## Goals\nChange files.\n"),
            );
            assert_eq!(
                parsed.frontmatter_error.as_deref(),
                Some("frontmatter_invalid"),
                "{field}"
            );
            assert!(parsed.frontmatter.is_none(), "{field}");
        }
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
