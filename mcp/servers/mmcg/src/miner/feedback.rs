//! Explicit feedback the author gave coding agents — the strongest persona
//! signal, because it states a preference instead of implying one.
//!
//! Human turns are read from Claude Code sessions under `~/.claude/projects`
//! and explicitly attributed Codex rollouts under `CODEX_HOME`: never tool
//! output, harness reminders, pasted material or subagent threads. A recorded
//! statement must quote a human turn verbatim and always starts as a
//! candidate. Agents and files can propose and repeat statements; only the
//! author, in an interactive terminal, can make one active. Claude Code memory
//! files of type `feedback` or `user` import as candidates too, because an
//! agent wrote them.

use super::codex_transcript::CodexSession;
use super::profile::{profile_path, publish_profile};
use super::store::{self, NewFeedback};
use crate::bounded_fs::ReadControl;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const MAX_TRANSCRIPT_SIZE: u64 = 256 * 1024 * 1024;
const MAX_EVIDENCE_VERIFY_SIZE: u64 = 32 * 1024 * 1024;
const MAX_EVIDENCE_VERIFY_TOTAL: u64 = 64 * 1024 * 1024;
const MAX_EVIDENCE_VERIFY_FILES: usize = 16;
const MAX_EVIDENCE_VERIFY_LINES: usize = 1_000_000;
const MAX_EVIDENCE_RECORD_SIZE: usize = 64 * 1024;
const MAX_EVIDENCE_RECORD_WORK: usize = 8 * 1024 * 1024;
const MAX_MEMORY_FILE_SIZE: u64 = 64 * 1024;
const MAX_MEMORY_FILES: usize = 2000;
/// Each scanned turn is cut to this many characters; quotes must fall inside.
const SCAN_TURN_CHARS: usize = 4000;
const MIN_QUOTE_CHARS: usize = 8;
const MAX_QUOTE_CHARS: usize = 300;
const MAX_STATEMENT_CHARS: usize = 200;
pub(super) const CATEGORIES: [&str; 5] = ["code", "process", "communication", "tooling", "review"];
const SCOPE_KINDS: [&str; 6] = ["language", "repo", "path", "project", "role", "workflow"];
/// Prefixes of credentials that must never be copied into a profile.
/// ponytail: a prefix list; add an entropy check if a missed format turns up.
const SECRET_MARKERS: [&str; 20] = [
    "sk-",
    "sk_live_",
    "sk_test_",
    "rk_live_",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "xoxa-",
    "xoxb-",
    "xoxp-",
    "AKIA",
    "AIza",
    "eyJ",
    "-----BEGIN",
    "password=",
    "secret=",
];

/// One human-written turn of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanTurn {
    pub at: String,
    pub text: String,
    pub line_no: usize,
    pub record_digest: String,
    pub cwd: Option<PathBuf>,
    pub session_id: Option<String>,
}

/// Human-written turns of a Claude Code transcript (JSON Lines), in order.
/// Records marked with a human origin count. Only a transcript that carries no
/// origin markers at all falls back to plain user prompts that are not
/// harness-wrapped; otherwise an unmarked record is never taken as human.
pub fn human_turns(transcript: &str) -> Vec<HumanTurn> {
    // A key match cannot come from message text: JSON escapes quotes there.
    let marked = transcript.contains("\"origin\"");
    let mut turns: Vec<HumanTurn> = Vec::new();
    for (index, line) in transcript.lines().enumerate() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let flagged = |key: &str| record.get(key).and_then(Value::as_bool) == Some(true);
        if flagged("isSidechain") || flagged("isMeta") || flagged("isCompactSummary") {
            continue;
        }
        let raw = match record.get("type").and_then(Value::as_str) {
            Some("user") if record.get("toolUseResult").is_none() => {
                let Some(text) = record.pointer("/message/content").and_then(message_text) else {
                    continue;
                };
                let legacy_prompt = !marked && !text.trim_start().starts_with('<');
                if !(is_human(record.get("origin")) || legacy_prompt) {
                    continue;
                }
                text
            }
            Some("attachment")
                if record.pointer("/attachment/type").and_then(Value::as_str)
                    == Some("queued_command")
                    && is_human(record.pointer("/attachment/origin")) =>
            {
                let Some(prompt) = record.pointer("/attachment/prompt").and_then(Value::as_str)
                else {
                    continue;
                };
                prompt.to_string()
            }
            _ => continue,
        };
        let text = strip_harness_blocks(&raw);
        if text.is_empty() || turns.last().is_some_and(|last| last.text == text) {
            continue;
        }
        let at = record
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        turns.push(HumanTurn {
            at,
            text,
            line_no: index + 1,
            record_digest: crate::hex::encode(&Sha256::digest(line.as_bytes())),
            cwd: record
                .get("cwd")
                .and_then(Value::as_str)
                .and_then(|cwd| Path::new(cwd).canonicalize().ok()),
            session_id: record
                .get("sessionId")
                .and_then(Value::as_str)
                .and_then(valid_session_id)
                .map(str::to_string),
        });
    }
    turns
}

/// The session the transcript records, from its own records rather than its
/// file name, so a copied transcript is not a second source.
fn session_id(transcript: &str) -> Option<String> {
    transcript.lines().find_map(|line| {
        let record = serde_json::from_str::<Value>(line).ok()?;
        Some(record.get("sessionId")?.as_str()?.to_string())
    })
}

fn stable_session_id(transcript: &str) -> String {
    session_id(transcript)
        .filter(|id| valid_session_id(id).is_some())
        .unwrap_or_else(|| {
            // A renamed or copied transcript is still one source.
            crate::hex::encode(&Sha256::digest(transcript.as_bytes()))
        })
}

pub(super) fn valid_session_id(id: &str) -> Option<&str> {
    (!id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)))
    .then_some(id)
}

fn is_human(origin: Option<&Value>) -> bool {
    origin
        .and_then(|origin| origin.get("kind"))
        .and_then(Value::as_str)
        == Some("human")
}

fn message_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let texts: Vec<&str> = blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

/// Remove blocks the harness inserts into a prompt: reminders and pasted
/// material are not the author's own words.
fn strip_harness_blocks(text: &str) -> String {
    let mut out = text.to_string();
    for (open, close) in [
        ("<system-reminder>", "</system-reminder>"),
        ("<pasted_content", "</pasted_content"),
    ] {
        while let Some(start) = out.find(open) {
            let Some(offset) = out[start..].find(close) else {
                out.truncate(start);
                break;
            };
            let closing = start + offset;
            let end = out[closing..]
                .find('>')
                .map_or(out.len(), |gt| closing + gt + 1);
            out.replace_range(start..end, "");
        }
    }
    out.trim().to_string()
}

/// Whitespace-collapsed text, so a quote may span a reflowed line.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The human turn that contains `quote` verbatim, ignoring whitespace runs.
pub fn quoted_turn<'a>(turns: &'a [HumanTurn], quote: &str) -> Option<&'a HumanTurn> {
    let quote = collapse(quote);
    turns
        .iter()
        .find(|turn| collapse(&turn.text).contains(&quote))
}

pub(crate) fn looks_secret(text: &str) -> bool {
    SECRET_MARKERS.iter().any(|marker| {
        text.match_indices(marker).any(|(index, _)| {
            // Credential prefixes begin tokens. In particular, "task-42"
            // and "task-oriented" must not match the embedded "sk-".
            text[..index]
                .chars()
                .next_back()
                .is_none_or(|previous| !previous.is_alphanumeric() && previous != '_')
        })
    })
}

#[test]
fn secret_prefixes_do_not_reject_ordinary_task_identifiers() {
    assert!(!looks_secret("task-42 and task-oriented work"));
    assert!(looks_secret("API_KEY=sk-privatevalue"));
    assert!(looks_secret("{\"token\":\"ghp_privatevalue\"}"));
}

/// One line of profile text: whitespace collapsed, bounded, and free of the
/// comment markup that delimits the sections of `style.md`.
pub(crate) fn single_line(
    value: &str,
    name: &str,
    min: usize,
    max: usize,
) -> Result<String, String> {
    let value = collapse(value);
    let chars = value.chars().count();
    if chars < min || chars > max {
        return Err(format!(
            "{name} must be {min} to {max} characters, got {chars}"
        ));
    }
    if value.contains("<!--") || value.contains("-->") {
        return Err(format!("{name} must not contain HTML comment markup"));
    }
    Ok(value)
}

pub(super) fn validate_scope(scope: &str) -> Result<(), String> {
    if scope == "global" {
        return Ok(());
    }
    match scope.split_once(':') {
        Some((kind, value))
            if SCOPE_KINDS.contains(&kind)
                && !value.is_empty()
                && value.len() <= 128
                && (kind != "role" || matches!(value, "planner" | "executor" | "auditor"))
                && !value.chars().any(char::is_control) =>
        {
            Ok(())
        }
        _ => Err(format!(
            "scope must be `global` or `<{}>:<value>`",
            SCOPE_KINDS.join("|")
        )),
    }
}

/// Days since 1970-01-01 as `YYYY-MM-DD` (Howard Hinnant's civil-from-days).
fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn read_bounded(path: &Path, max: u64) -> Result<String, Box<dyn std::error::Error>> {
    Ok(String::from_utf8_lossy(&read_bounded_bytes(path, max)?).into_owned())
}

fn read_bounded_bytes(path: &Path, max: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let (root, target) = crate::bounded_fs::open_file_target(path)?;
    let file = crate::bounded_fs::read_regular_file_with_capability(
        &root,
        &target,
        max,
        max,
        ReadControl::default(),
    )
    .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(file.bytes)
}

/// `~/.claude/projects`, where Claude Code keeps session transcripts and memory.
fn claude_projects() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(std::env::home_dir()
        .ok_or("could not resolve home directory")?
        .join(".claude")
        .join("projects"))
}

/// Claude Code's directory name for a project: its absolute path with every
/// non-alphanumeric character replaced by `-`.
pub fn claude_project_slug(project: &Path) -> String {
    project
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The most recently written session transcript of `project`.
fn latest_transcript(project: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = claude_projects()?.join(claude_project_slug(&project.canonicalize()?));
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if !metadata.is_file() || path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let modified = metadata.modified()?;
        if newest.as_ref().is_none_or(|(time, _)| modified > *time) {
            newest = Some((modified, path));
        }
    }
    newest
        .map(|(_, path)| path)
        .ok_or_else(|| format!("no session transcript under {}", dir.display()).into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TranscriptClient {
    Claude,
    Codex,
}

impl TranscriptClient {
    fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn source(self, session: &str) -> String {
        match self {
            Self::Claude => format!("session:{session}"),
            Self::Codex => format!("session:codex:{session}"),
        }
    }

    fn human_turns(self, text: &str) -> Result<Vec<HumanTurn>, Box<dyn std::error::Error>> {
        match self {
            Self::Claude => Ok(human_turns(text)),
            Self::Codex => Ok(CodexSession::parse(text)?.human_turns(text)),
        }
    }
}

struct SessionTranscript {
    path: PathBuf,
    client: TranscriptClient,
}

/// A complete bounded snapshot for automatic collection. Unlike the manual
/// legacy import, collection requires explicit human and project attribution.
pub(super) struct CollectionTranscript {
    pub path: PathBuf,
    pub source: String,
    pub digest: String,
    pub bytes: usize,
    pub lines: usize,
    pub turns: Vec<HumanTurn>,
    pub record_sizes: HashMap<usize, usize>,
}

pub(super) struct CollectionBudget {
    pub bytes: u64,
    pub lines: usize,
}

#[derive(serde::Deserialize)]
struct CollectionClaudeMetadata {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
}

pub(super) fn collection_transcript(
    path: &Path,
    project_root: &Path,
    budget: &mut CollectionBudget,
) -> Result<CollectionTranscript, Box<dyn std::error::Error>> {
    const MAX_TURNS: usize = 8192;
    let transcript = session_transcript(path)?;
    let cap = budget.bytes.min(MAX_EVIDENCE_VERIFY_SIZE);
    let text = match transcript.read(cap) {
        Ok(text) => text,
        Err(error) => {
            // A failed read may already have consumed its whole cap. Do not
            // let malformed files reset the cumulative verification budget.
            budget.bytes -= cap;
            budget.lines = 0;
            return Err(error);
        }
    };
    budget.bytes -= text.len() as u64;
    let lines = text.lines().count();
    if lines > budget.lines {
        budget.lines = 0;
        return Err("collection exceeds the remaining JSONL line budget".into());
    }
    budget.lines -= lines;
    let mut turns = Vec::new();
    let session = match transcript.client {
        TranscriptClient::Codex => {
            let session = CodexSession::parse(&text)?;
            if session.identity().1 != project_root {
                return Err("transcript does not bind to the supplied project root".into());
            }
            for (index, line) in text.lines().enumerate() {
                turns.extend(session.record_turns(line, index + 1));
                if turns.len() > MAX_TURNS {
                    return Err("collection transcript exceeds 8192 human segments".into());
                }
            }
            session.identity().0.to_string()
        }
        TranscriptClient::Claude => {
            let expected_slug = claude_project_slug(project_root);
            if transcript
                .path
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                != Some(expected_slug.as_str())
            {
                return Err("Claude transcript does not match the supplied project root".into());
            }
            let mut session = None;
            let mut project_bound = false;
            for (index, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                // Identity and cwd apply even to oversized uncited records.
                // Other fields (including tool output) are skipped by serde.
                let metadata: CollectionClaudeMetadata =
                    serde_json::from_str(line).map_err(|_| "invalid Claude JSONL record")?;
                if let Some(id) = metadata.session_id.as_deref() {
                    let id = valid_session_id(id).ok_or("invalid Claude sessionId")?;
                    if session.as_deref().is_some_and(|old| old != id) {
                        return Err("Claude transcript has conflicting session identities".into());
                    }
                    session = Some(id.to_string());
                }
                if let Some(cwd) = metadata.cwd.as_deref() {
                    if !Path::new(cwd).is_absolute() {
                        return Err("Claude collection requires an absolute cwd".into());
                    }
                    if Path::new(cwd).canonicalize()?.as_path() != project_root {
                        return Err("Claude transcript changes project cwd".into());
                    }
                    project_bound = true;
                }
                if line.len() > MAX_EVIDENCE_RECORD_SIZE {
                    continue;
                }
                let record: Value = serde_json::from_str(line)?;
                let raw = match record.get("type").and_then(Value::as_str) {
                    Some("user") if is_human(record.get("origin")) => {
                        match record.pointer("/message/content") {
                            Some(Value::String(text)) => vec![text.as_str()],
                            Some(Value::Array(blocks))
                                if blocks.iter().all(|block| block["type"] == "text") =>
                            {
                                blocks
                                    .iter()
                                    .filter_map(|block| block["text"].as_str())
                                    .collect()
                            }
                            _ => Vec::new(),
                        }
                    }
                    Some("attachment") if is_human(record.pointer("/attachment/origin")) => record
                        .pointer("/attachment/prompt")
                        .and_then(Value::as_str)
                        .into_iter()
                        .collect(),
                    _ => Vec::new(),
                };
                if raw.is_empty() {
                    continue;
                }
                // Existing filtering excludes sidechains, tools and meta turns.
                let Some(mut turn) = human_turns(line).into_iter().next() else {
                    continue;
                };
                if turn.cwd.as_deref() != Some(project_root) || turn.session_id.is_none() {
                    continue;
                }
                turn.line_no = index + 1;
                for segment in raw
                    .into_iter()
                    .flat_map(super::codex_transcript::human_segments)
                {
                    let mut segment_turn = turn.clone();
                    segment_turn.text = segment;
                    turns.push(segment_turn);
                }
                if turns.len() > MAX_TURNS {
                    return Err("collection transcript exceeds 8192 human segments".into());
                }
            }
            if !project_bound {
                return Err("Claude transcript lacks an exact project cwd".into());
            }
            session.ok_or("Claude collection needs a valid sessionId")?
        }
    };
    if turns.iter().any(|turn| {
        turn.at.len() > 64 || !turn.at.is_ascii() || turn.at.chars().any(char::is_control)
    }) {
        return Err("collection timestamp must be at most 64 printable ASCII bytes".into());
    }
    let cited_lines: std::collections::HashSet<usize> =
        turns.iter().map(|turn| turn.line_no).collect();
    let record_sizes = text
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            cited_lines
                .contains(&(index + 1))
                .then_some((index + 1, line.len()))
        })
        .collect();
    Ok(CollectionTranscript {
        path: transcript.path,
        source: transcript.client.source(&session),
        digest: crate::hex::encode(&Sha256::digest(text.as_bytes())),
        bytes: text.len(),
        lines,
        turns,
        record_sizes,
    })
}

impl SessionTranscript {
    fn read(&self, max_size: u64) -> Result<String, Box<dyn std::error::Error>> {
        // Citations must bind original bytes, never UTF-8 replacement text.
        Ok(String::from_utf8(read_bounded_bytes(
            &self.path, max_size,
        )?)?)
    }
}

fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".codex")))
}

fn codex_rollout_path(relative: &Path) -> bool {
    let Some(parts) = relative
        .iter()
        .map(|part| part.to_str())
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    let filename = match parts.as_slice() {
        ["archived_sessions", filename] => filename,
        ["sessions", year, month, day, filename]
            if year.len() == 4
                && month.len() == 2
                && day.len() == 2
                && [year, month, day]
                    .iter()
                    .all(|part| part.bytes().all(|b| b.is_ascii_digit()))
                && month
                    .parse::<u8>()
                    .is_ok_and(|month| (1..=12).contains(&month))
                && day.parse::<u8>().is_ok_and(|day| (1..=31).contains(&day)) =>
        {
            filename
        }
        _ => return false,
    };
    filename.starts_with("rollout-") && filename.ends_with(".jsonl") && filename.len() > 14
}

/// Admit explicit transcript files only within the client's local history
/// layout. Canonical containment prevents symlinks escaping that history root.
fn session_transcript(path: &Path) -> Result<SessionTranscript, Box<dyn std::error::Error>> {
    let transcript = path
        .canonicalize()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if transcript.extension().and_then(|e| e.to_str()) == Some("jsonl") && transcript.is_file() {
        if claude_projects()
            .ok()
            .and_then(|root| root.canonicalize().ok())
            .and_then(|root| {
                transcript
                    .strip_prefix(root)
                    .ok()
                    .map(|path| path.components().count())
            })
            == Some(2)
        {
            return Ok(SessionTranscript {
                path: transcript,
                client: TranscriptClient::Claude,
            });
        }
        if codex_home()
            .and_then(|root| root.canonicalize().ok())
            .is_some_and(|root| transcript.strip_prefix(root).is_ok_and(codex_rollout_path))
        {
            return Ok(SessionTranscript {
                path: transcript,
                client: TranscriptClient::Codex,
            });
        }
    }
    Err(format!(
        "{} is not a supported Claude Code or Codex session transcript",
        path.display()
    )
    .into())
}

/// A quote checked against a human turn, with a stable session and project
/// origin. The session is evidence; a caller still has to name the task episode.
pub(crate) struct VerifiedQuote {
    pub quote: String,
    pub at: String,
    pub source: String,
    /// Claude's directory slug is an additional check beside exact cwd.
    pub claude_project: Option<String>,
    pub project_cwd: Option<PathBuf>,
    pub source_path: String,
    pub line_no: usize,
    pub record_digest: String,
}

pub(crate) fn verified_quote(
    transcript: &Path,
    quote: &str,
) -> Result<VerifiedQuote, Box<dyn std::error::Error>> {
    verified_quote_with_limit(transcript, quote, MAX_TRANSCRIPT_SIZE, false)
}

/// Habit evidence must be re-readable under the same source limits used by
/// observation and MCP, so reject an unusable citation at insertion time.
pub(crate) fn verified_habit_quote(
    transcript: &Path,
    quote: &str,
) -> Result<VerifiedQuote, Box<dyn std::error::Error>> {
    let evidence = verified_quote_with_limit(transcript, quote, MAX_EVIDENCE_VERIFY_SIZE, true)?;
    let mut verifier = QuoteSourceVerifier::new();
    if !verifier.current(
        &evidence.source_path,
        evidence.line_no,
        &evidence.record_digest,
        &evidence.quote,
        &evidence.source,
    ) {
        return Err("habit citation exceeds verification limits or its source changed".into());
    }
    Ok(evidence)
}

fn verified_quote_with_limit(
    transcript: &Path,
    quote: &str,
    max_transcript_size: u64,
    require_turn_session_id: bool,
) -> Result<VerifiedQuote, Box<dyn std::error::Error>> {
    let quote = single_line(quote, "quote", MIN_QUOTE_CHARS, MAX_QUOTE_CHARS)?;
    if looks_secret(&quote) || crate::indexer::secret_like_documentation(&quote) {
        return Err("refusing to record text that looks like a credential".into());
    }
    let transcript = session_transcript(transcript)?;
    let text = transcript.read(max_transcript_size)?;
    let turns = transcript.client.human_turns(&text)?;
    let turn = quoted_turn(&turns, &quote).ok_or(
        "quote does not appear verbatim in a human turn of the transcript; record only what the author wrote",
    )?;
    let at = turn.at.get(..10).unwrap_or(&turn.at).to_string();
    let session = if require_turn_session_id || transcript.client == TranscriptClient::Codex {
        turn.session_id
            .clone()
            .ok_or("citation needs a valid session identity bound to the quoted human record")?
    } else {
        stable_session_id(&text)
    };
    let claude_project = match transcript.client {
        TranscriptClient::Claude => Some(
            transcript
                .path
                .strip_prefix(claude_projects()?.canonicalize()?)?
                .components()
                .next()
                .ok_or("transcript has no project directory")?
                .as_os_str()
                .to_string_lossy()
                .into_owned(),
        ),
        TranscriptClient::Codex => None,
    };
    if claude_project.as_ref().is_some_and(|project| {
        project.is_empty()
            || project == "."
            || project.len() > 128
            || project.chars().any(char::is_control)
    }) {
        return Err("transcript has no project directory".into());
    }
    Ok(VerifiedQuote {
        quote,
        at,
        source: transcript.client.source(&session),
        claude_project,
        project_cwd: turn.cwd.clone(),
        source_path: transcript.path.to_string_lossy().into_owned(),
        line_no: turn.line_no,
        record_digest: turn.record_digest.clone(),
    })
}

/// One bounded source check for a profile read or publication. Each transcript
/// is loaded once, with line offsets so many citations do not rescan it.
pub(crate) struct QuoteSourceVerifier {
    files: HashMap<PathBuf, Option<CachedTranscript>>,
    collected_files: HashMap<(String, String), Option<CollectionTranscript>>,
    remaining_bytes: u64,
    remaining_lines: usize,
    remaining_record_work: usize,
    incomplete: bool,
}

struct CachedTranscript {
    text: String,
    line_starts: Vec<usize>,
    codex: Option<CodexSession>,
}

impl CachedTranscript {
    fn new(text: String, max_lines: usize, client: TranscriptClient) -> Option<Self> {
        if max_lines == 0 {
            return None;
        }
        let mut line_starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' && index + 1 < text.len() {
                if line_starts.len() >= max_lines {
                    return None;
                }
                line_starts.push(index + 1);
            }
        }
        let codex = match client {
            TranscriptClient::Claude => None,
            TranscriptClient::Codex => Some(CodexSession::parse(&text).ok()?),
        };
        Some(Self {
            text,
            line_starts,
            codex,
        })
    }

    fn line(&self, line_no: usize) -> Option<&str> {
        let start = *self.line_starts.get(line_no.checked_sub(1)?)?;
        let end = self.line_starts.get(line_no).map_or_else(
            || self.text.len() - usize::from(self.text.ends_with('\n')),
            |next| next - 1,
        );
        self.text
            .get(start..end)
            .map(|line| line.trim_end_matches('\r'))
    }
}

impl QuoteSourceVerifier {
    pub(crate) fn new() -> Self {
        Self {
            files: HashMap::new(),
            collected_files: HashMap::new(),
            remaining_bytes: MAX_EVIDENCE_VERIFY_TOTAL,
            remaining_lines: MAX_EVIDENCE_VERIFY_LINES,
            remaining_record_work: MAX_EVIDENCE_RECORD_WORK,
            incomplete: false,
        }
    }

    pub(crate) fn incomplete(&self) -> bool {
        self.incomplete
    }

    /// Transferred inbox evidence retains full-session attribution checks.
    /// Both adapters charge the same file, byte, line and record-work budgets.
    pub(crate) fn current_collected(
        &mut self,
        candidate: &super::store::CollectedCandidate,
        evidence: &super::store::HabitEvidence,
    ) -> bool {
        if candidate.source != evidence.source
            || candidate.source_path != evidence.source_path
            || i64::try_from(candidate.line_no).ok() != Some(evidence.line_no)
            || candidate.record_digest != evidence.record_digest
            || candidate.quote != evidence.quote
            || candidate.project != evidence.project
        {
            return false;
        }
        self.current_observation(candidate)
    }

    pub(crate) fn current_observation(
        &mut self,
        candidate: &super::store::CollectedCandidate,
    ) -> bool {
        if candidate.extractor == super::hooks::EXTRACTOR {
            if self.remaining_record_work < 512 * 1024 {
                self.incomplete = true;
                return false;
            }
            self.remaining_record_work -= 512 * 1024;
            return super::hooks::current_candidate(candidate);
        }
        if super::profile::persona_project_id(Path::new(&candidate.project_root)).as_deref()
            != Some(candidate.project.as_str())
        {
            return false;
        }
        let key = (
            candidate.source_path.clone(),
            candidate.project_root.clone(),
        );
        if !self.collected_files.contains_key(&key) {
            if self.files.len() + self.collected_files.len() >= MAX_EVIDENCE_VERIFY_FILES {
                self.incomplete = true;
                return false;
            }
            let mut budget = CollectionBudget {
                bytes: self.remaining_bytes,
                lines: self.remaining_lines,
            };
            let snapshot =
                collection_transcript(Path::new(&key.0), Path::new(&key.1), &mut budget).ok();
            self.remaining_bytes = budget.bytes;
            self.remaining_lines = budget.lines;
            if snapshot.is_none() {
                self.incomplete = true;
            }
            self.collected_files.insert(key.clone(), snapshot);
        }
        let Some(snapshot) = self.collected_files.get(&key).and_then(Option::as_ref) else {
            return false;
        };
        let Some(size) = snapshot.record_sizes.get(&candidate.line_no) else {
            return false;
        };
        if *size > self.remaining_record_work {
            self.incomplete = true;
            return false;
        }
        self.remaining_record_work -= size;
        super::collection::snapshot_contains_candidate(snapshot, candidate)
    }

    /// Revalidate the exact human JSONL record before serving an observation.
    /// Appends preserve an earlier record; rewrites or missing sources fail closed.
    pub(crate) fn current(
        &mut self,
        source_path: &str,
        line_no: usize,
        record_digest: &str,
        quote: &str,
        source: &str,
    ) -> bool {
        if line_no == 0
            || record_digest.len() != 64
            || quote.chars().take(MAX_QUOTE_CHARS + 1).count() > MAX_QUOTE_CHARS
        {
            return false;
        }
        let Ok(SessionTranscript { path, client }) = session_transcript(Path::new(source_path))
        else {
            return false;
        };
        if !self.files.contains_key(&path) {
            if self.files.len() + self.collected_files.len() >= MAX_EVIDENCE_VERIFY_FILES {
                self.incomplete = true;
                return false;
            }
            let Ok(size) = std::fs::metadata(&path).map(|metadata| metadata.len()) else {
                return false;
            };
            if size > MAX_EVIDENCE_VERIFY_SIZE || size > self.remaining_bytes {
                self.incomplete = true;
                return false;
            }
            // The read uses the measured size as its cap. A concurrent append
            // fails verification rather than exceeding the remaining budget.
            self.remaining_bytes -= size;
            let Ok(text) =
                read_bounded_bytes(&path, size).and_then(|bytes| Ok(String::from_utf8(bytes)?))
            else {
                self.incomplete = true;
                self.files.insert(path, None);
                return false;
            };
            let cached = CachedTranscript::new(text, self.remaining_lines, client);
            if let Some(ref transcript) = cached {
                self.remaining_lines -= transcript.line_starts.len();
            } else {
                self.incomplete = true;
            }
            self.files.insert(path.clone(), cached);
        }
        let Some(transcript) = self.files.get(&path).and_then(Option::as_ref) else {
            return false;
        };
        let Some(line) = transcript.line(line_no) else {
            return false;
        };
        if line.len() > MAX_EVIDENCE_RECORD_SIZE || line.len() > self.remaining_record_work {
            self.incomplete = true;
            return false;
        }
        self.remaining_record_work -= line.len();
        match &transcript.codex {
            Some(codex) => {
                quoted_turn(&codex.record_turns(line, line_no), quote).is_some_and(|turn| {
                    turn.record_digest == record_digest
                        && turn
                            .session_id
                            .as_deref()
                            .is_some_and(|id| source == TranscriptClient::Codex.source(id))
                })
            }
            None => quote_line_current(line, record_digest, quote, Some(source)),
        }
    }
}

#[cfg(test)]
fn quote_record_current(text: &str, line_no: usize, record_digest: &str, quote: &str) -> bool {
    let Some(line) = text.lines().nth(line_no - 1) else {
        return false;
    };
    quote_line_current(line, record_digest, quote, None)
}

fn quote_line_current(
    line: &str,
    record_digest: &str,
    quote: &str,
    expected_source: Option<&str>,
) -> bool {
    if crate::hex::encode(&Sha256::digest(line.as_bytes())) != record_digest {
        return false;
    }
    quoted_turn(&human_turns(line), quote).is_some_and(|turn| {
        expected_source.is_none_or(|source| {
            turn.session_id
                .as_deref()
                .is_some_and(|id| source.strip_prefix("session:") == Some(id))
        })
    })
}

/// `mastermind miner feedback scan`: the human turns of one transcript as JSON.
pub fn scan(transcript: Option<PathBuf>, project: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let path = match transcript {
        Some(path) => path,
        None => latest_transcript(project)?,
    };
    let transcript = session_transcript(&path)?;
    let turns = transcript
        .client
        .human_turns(&transcript.read(MAX_TRANSCRIPT_SIZE)?)?;
    let turns: Vec<Value> = turns
        .iter()
        .map(|turn| {
            let text: String = turn.text.chars().take(SCAN_TURN_CHARS).collect();
            json!({ "at": turn.at, "text": text, "line_no": turn.line_no })
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "transcript": transcript.path.to_string_lossy(),
            "client": transcript.client.name(),
            "turns": turns,
        }))?
    );
    Ok(())
}

/// `mastermind miner feedback add`: record one candidate preference quoted
/// from a human turn of a session transcript.
pub fn add(
    transcript: &Path,
    quote: &str,
    statement: &str,
    category: &str,
    scope: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let statement = single_line(statement, "statement", MIN_QUOTE_CHARS, MAX_STATEMENT_CHARS)?;
    let source_quote = verified_quote(transcript, quote)?;
    if !CATEGORIES.contains(&category) {
        return Err(format!("category must be one of {}", CATEGORIES.join(", ")).into());
    }
    validate_scope(scope)?;
    if looks_secret(&statement) || crate::indexer::secret_like_documentation(&statement) {
        return Err("refusing to record text that looks like a credential".into());
    }
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let (recorded, _) = publish_profile(
        &db_path,
        &profile_path()?,
        false,
        |db| {
            Ok(db.record_feedback(&NewFeedback {
                statement: &statement,
                category,
                scope,
                quote: &source_quote.quote,
                at: &source_quote.at,
                source: &source_quote.source,
            })?)
        },
        |_| None,
    )?;
    println!(
        "Recorded `{}` as {} ({} source(s)); key {}.",
        recorded.statement, recorded.status, recorded.sources, recorded.key
    );
    Ok(())
}

/// One Claude Code memory file worth importing.
#[derive(Debug, PartialEq, Eq)]
struct MemoryEntry {
    statement: String,
    quote: String,
    scope: String,
    source: String,
}

/// A memory file of type `feedback` or `user`: its description is the
/// statement and the first body line the quote. `project` is the Claude Code
/// project directory name, which `mmcg_profile` matches exactly.
fn memory_entry(project: &str, file: &str, text: &str) -> Option<MemoryEntry> {
    let rest = text.strip_prefix("---\n")?;
    let (frontmatter, body) = rest.split_once("\n---\n")?;
    let frontmatter: serde_norway::Value = serde_norway::from_str(frontmatter).ok()?;
    let kind = frontmatter
        .get("metadata")
        .and_then(|metadata| metadata.get("type"))
        .or_else(|| frontmatter.get("type"))
        .and_then(serde_norway::Value::as_str)?;
    if !matches!(kind, "feedback" | "user") {
        return None;
    }
    let statement = frontmatter
        .get("description")
        .or_else(|| frontmatter.get("name"))
        .and_then(serde_norway::Value::as_str)?;
    let statement: String = collapse(statement)
        .chars()
        .take(MAX_STATEMENT_CHARS)
        .collect();
    let quote: String = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| collapse(line).chars().take(MAX_QUOTE_CHARS).collect())
        .unwrap_or_else(|| statement.clone());
    let clean = |text: &str| {
        single_line(text, "memory text", MIN_QUOTE_CHARS, MAX_QUOTE_CHARS).is_ok()
            && !looks_secret(text)
    };
    if !clean(&statement) || !clean(&quote) {
        return None;
    }
    // A user memory describes the person; feedback may belong to its project.
    let scope = if kind == "user" {
        "global".to_string()
    } else {
        format!("project:{project}")
    };
    Some(MemoryEntry {
        statement,
        quote,
        scope,
        source: format!("memory:{project}/{file}"),
    })
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// `mastermind miner feedback import-memory`: import Claude Code memory files
/// of type `feedback` or `user` from every project under `dir` as candidates.
pub fn import_memory(dir: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let dir = match dir {
        Some(dir) => dir,
        None => claude_projects()?,
    };
    let mut entries: Vec<(MemoryEntry, String)> = Vec::new();
    let mut read = 0;
    for project in std::fs::read_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))? {
        let project = project?.path();
        let memory = project.join("memory");
        if !is_real_dir(&project) || !is_real_dir(&memory) {
            continue;
        }
        let slug = project
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for file in std::fs::read_dir(&memory)? {
            let path = file?.path();
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            let Some(name) = name.filter(|name| name.ends_with(".md") && name != "MEMORY.md")
            else {
                continue;
            };
            read += 1;
            if read > MAX_MEMORY_FILES {
                return Err(format!(
                    "more than {MAX_MEMORY_FILES} memory files under {}",
                    dir.display()
                )
                .into());
            }
            let Ok(text) = read_bounded(&path, MAX_MEMORY_FILE_SIZE) else {
                continue;
            };
            let modified = std::fs::metadata(&path)?
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs() as i64);
            if let Some(entry) = memory_entry(&slug, &name, &text) {
                entries.push((entry, civil_date(modified.div_euclid(86_400))));
            }
        }
    }
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let (imported, _) = publish_profile(
        &db_path,
        &profile_path()?,
        false,
        |db| {
            for (entry, at) in &entries {
                db.record_feedback(&NewFeedback {
                    statement: &entry.statement,
                    // Memory files carry no category; the agent that wrote them did not classify.
                    category: "memory",
                    scope: &entry.scope,
                    quote: &entry.quote,
                    at,
                    source: &entry.source,
                })?;
            }
            Ok(entries.len())
        },
        |_| None,
    )?;
    println!(
        "Imported {imported} memory statement(s) as candidates; accept the ones that are yours."
    );
    Ok(())
}

/// `mastermind miner feedback list`.
pub fn list() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let Some(db) = store::ProfileStore::open_optional_read_only(&db_path)? else {
        println!("No feedback recorded.");
        return Ok(());
    };
    let feedback = db.feedback()?;
    if feedback.is_empty() {
        println!("No feedback recorded.");
    }
    for entry in feedback {
        println!(
            "{}\t{}\t{} retained source(s)\t{}\t{}\t{}",
            entry.key,
            entry.status,
            entry.sources,
            entry.scope,
            entry.statement,
            if db
                .feedback_candidate_bindings(&entry.key)
                .is_ok_and(|items| !items.is_empty())
            {
                "source-bound; use show for freshness and review revision"
            } else {
                "legacy-unverifiable or invalid provenance"
            }
        );
    }
    Ok(())
}

/// Show one statement with its individual retained quotes. Older stores only
/// kept a single quote and source count, so their missing spans stay explicit.
pub fn show(prefix: &str) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let db = store::ProfileStore::open_read_only(&db_path)?;
    let matches: Vec<_> = db
        .feedback()?
        .into_iter()
        .filter(|entry| entry.key.starts_with(prefix))
        .collect();
    let [entry] = matches.as_slice() else {
        return Err(if matches.is_empty() {
            format!("no feedback key starts with `{prefix}`").into()
        } else {
            format!(
                "`{prefix}` matches {} keys; use a longer prefix",
                matches.len()
            )
            .into()
        });
    };
    println!(
        "{}\t{}\t{}\t{}",
        entry.key, entry.status, entry.category, entry.scope
    );
    println!("{}", entry.statement);
    println!("Review revision: {}", entry.review_revision());
    println!(
        "Accepted revision: {}",
        entry.accepted_revision.as_deref().unwrap_or("none")
    );
    let verified =
        super::profile::feedback_sources_current(&db, entry, &mut QuoteSourceVerifier::new());
    println!(
        "Source verification: {}",
        match &verified {
            Ok(_) => "current",
            Err(reason) => reason,
        }
    );
    if let Ok(bindings) = db.feedback_candidate_bindings(&entry.key) {
        for item in bindings {
            println!(
                "{}",
                serde_json::to_string(&json!({"candidate_id":item.id,"revision":item.revision,
                "source":item.source,"source_path":item.source_path,"line_no":item.line_no,
                "segment_no":item.segment_no,"record_digest":item.record_digest,"quote":item.quote}))?
            );
        }
    }
    println!(
        "Proposal history: {}",
        db.candidate_feedback_history(None, Some(&entry.key))?
    );
    println!("Relations: {}", db.feedback_relations(&entry.key)?);
    let evidence = db.feedback_evidence(&entry.key)?;
    if evidence.is_empty() {
        println!("No per-source quote retained for this legacy entry; re-import its source before review.");
    }
    for item in evidence {
        println!(
            "{}\t{}\t{}\t{}",
            item.at, item.attribution, item.source, item.quote
        );
    }
    Ok(())
}

/// `mastermind miner feedback accept|reject`: review one entry by key prefix.
/// Accepting makes a statement outrank mined rules for every future agent, so
/// it needs the author at an interactive terminal; rejecting is always safe.
pub fn set_status(
    prefix: &str,
    status: &str,
    revision: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    if status == "active" && !std::io::stdin().is_terminal() {
        return Err(
            "accept needs an interactive terminal: run `mastermind miner feedback accept` yourself"
                .into(),
        );
    }
    let db_path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let (matches, _) = publish_profile(
        &db_path,
        &profile_path()?,
        false,
        |db| {
            if status == "active" {
                let entries = db.feedback()?;
                let matches: Vec<_> = entries
                    .iter()
                    .filter(|entry| entry.key.starts_with(prefix))
                    .collect();
                let [entry] = matches.as_slice() else {
                    return Ok(matches.len());
                };
                if revision != Some(entry.review_revision().as_str()) {
                    return Err("review revision changed; inspect feedback show again".into());
                }
                super::profile::feedback_sources_current(
                    db,
                    entry,
                    &mut QuoteSourceVerifier::new(),
                )?;
            }
            Ok(db.review_feedback(prefix, status, revision)?)
        },
        |_| None,
    )?;
    match matches {
        1 => println!("Marked `{prefix}` {status}."),
        0 => return Err(format!("no feedback key starts with `{prefix}`").into()),
        n => return Err(format!("`{prefix}` matches {n} keys; use a longer prefix").into()),
    }
    Ok(())
}

pub fn dismiss_source(
    prefix: &str,
    candidate: &str,
    revision: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if !std::io::stdin().is_terminal() {
        return Err("source dismissal requires the author's interactive terminal".into());
    }
    if !super::collection::valid_id(candidate) || !super::collection::valid_id(revision) {
        return Err("candidate and review revision must be full 64-character keys".into());
    }
    let path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    publish_profile(
        &path,
        &profile_path()?,
        false,
        |db| Ok(db.dismiss_feedback_source(prefix, candidate, revision)?),
        |_| None,
    )?;
    println!("Dismissed source {candidate}; inspect feedback show before any further review.");
    Ok(())
}

/// Explicit replacement, after reviewing both exact definitions and their
/// retained evidence. Only the successor needs sources that are still current.
pub fn supersede(
    old_prefix: &str,
    new_prefix: &str,
    old_revision: &str,
    new_revision: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if !std::io::stdin().is_terminal() {
        return Err("supersede requires the author's interactive terminal".into());
    }
    if !super::collection::valid_id(old_revision) || !super::collection::valid_id(new_revision) {
        return Err("both review revisions must be full 64-character keys".into());
    }
    let path = store::ProfileStore::db_path().ok_or("could not resolve home directory")?;
    // An invalid request must not initialize a new empty profile.
    store::ProfileStore::open_read_only(&path)?;
    let (receipt, _) = publish_profile(
        &path,
        &profile_path()?,
        false,
        |db| {
            let entries = db.feedback()?;
            let resolve = |prefix: &str| -> Result<&store::Feedback, String> {
                let mut matches = entries.iter().filter(|entry| entry.key.starts_with(prefix));
                let entry = matches
                    .next()
                    .ok_or_else(|| format!("no feedback key starts with `{prefix}`"))?;
                if matches.next().is_some() {
                    return Err(format!(
                        "`{prefix}` matches multiple keys; use a longer prefix"
                    ));
                }
                Ok(entry)
            };
            let old = resolve(old_prefix)?;
            let new = resolve(new_prefix)?;
            if old.key == new.key {
                return Err("a preference cannot supersede itself".into());
            }
            // SQL may have committed while a previous Markdown write failed.
            // This receipt precedes current revision/status/source gates: a
            // retry publishes today's state without reactivating the successor.
            if let Some(receipt) =
                db.feedback_supersession_retry(&old.key, &new.key, old_revision, new_revision)?
            {
                return Ok(receipt);
            }
            if old.review_revision() != old_revision || new.review_revision() != new_revision {
                return Err("review revision changed; inspect both preferences again".into());
            }
            super::profile::feedback_sources_current(db, new, &mut QuoteSourceVerifier::new())?;
            Ok(db.supersede_feedback(&old.key, &new.key, old_revision, new_revision)?)
        },
        |_| None,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "relation": "supersedes", "receipt": receipt,
            "note": "Stored statuses shown. Publication separately revalidates current sources. Retrying a committed replacement does not accept it again."
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(value: Value) -> String {
        serde_json::to_string(&value).unwrap()
    }

    #[test]
    fn copied_transcript_without_valid_session_id_is_one_source() {
        let transcript = record(json!({"type": "user", "origin": {"kind": "human"},
            "message": {"content": "Check the rollout before saying it is live."}}));
        assert_eq!(stable_session_id(&transcript).len(), 64);
        let malformed = record(json!({"sessionId": "bad\nidentifier"}));
        assert_eq!(stable_session_id(&malformed).len(), 64);
        let identified = record(json!({"sessionId": "session-123"}));
        assert_eq!(stable_session_id(&identified), "session-123");
    }

    #[test]
    fn habit_session_identity_comes_from_quoted_human_record() {
        let transcript = [
            record(json!({"type": "user", "origin": {"kind": "human"},
                "sessionId": "session-a", "message": {"content": "Check the contract first."}})),
            record(json!({"type": "user", "origin": {"kind": "human"},
                "sessionId": "bad\nidentifier", "message": {"content": "Review the rollout too."}})),
        ]
        .join("\n");
        let turns = human_turns(&transcript);
        assert_eq!(turns[0].session_id.as_deref(), Some("session-a"));
        assert_eq!(turns[1].session_id, None);
        let first_line = transcript.lines().next().unwrap();
        let digest = crate::hex::encode(&Sha256::digest(first_line.as_bytes()));
        assert!(quote_line_current(
            first_line,
            &digest,
            "contract first",
            Some("session:session-a"),
        ));
        assert!(!quote_line_current(
            first_line,
            &digest,
            "contract first",
            Some("session:another-session"),
        ));
    }

    #[test]
    fn cited_human_record_survives_append_but_not_rewrite() {
        let line = record(json!({"type": "user", "origin": {"kind": "human"},
            "message": {"content": "Check contract and rollout separately."}}));
        let digest = crate::hex::encode(&Sha256::digest(line.as_bytes()));
        assert!(quote_record_current(
            &format!("{line}\n{}", record(json!({"type": "assistant"}))),
            1,
            &digest,
            "contract and rollout"
        ));
        assert!(!quote_record_current(
            &line.replace("separately", "together"),
            1,
            &digest,
            "contract and rollout"
        ));
        assert!(!quote_record_current(
            &line,
            2,
            &digest,
            "contract and rollout"
        ));
    }

    #[test]
    fn cached_transcript_addresses_exact_jsonl_lines() {
        let first = record(json!({"type": "assistant"}));
        let second = record(json!({"type": "user", "origin": {"kind": "human"},
            "message": {"content": "Check contract and rollout separately."}}));
        let digest = crate::hex::encode(&Sha256::digest(second.as_bytes()));
        let cache = CachedTranscript::new(
            format!("{first}\r\n{second}\n"),
            2,
            TranscriptClient::Claude,
        )
        .unwrap();
        assert_eq!(cache.line(1), Some(first.as_str()));
        assert_eq!(cache.line(2), Some(second.as_str()));
        assert_eq!(cache.line(3), None);
        assert!(cache.line(2).is_some_and(|line| quote_line_current(
            line,
            &digest,
            "contract and rollout",
            None,
        )));
        assert!(!cache.line(1).is_some_and(|line| quote_line_current(
            line,
            &digest,
            "contract and rollout",
            None,
        )));
        assert!(CachedTranscript::new("a\nb\nc".into(), 2, TranscriptClient::Claude).is_none());
    }

    #[test]
    fn human_turns_keep_prompts_and_queued_messages_only() {
        let transcript = [
            record(json!({"type": "user", "origin": {"kind": "human"}, "timestamp": "2026-09-24T10:00:00Z",
                "sessionId": "s-1",
                "message": {"content": "<pasted_content id=\"1\">pasted skills</pasted_content id=\"1\">\n\nnever pad commits"}})),
            record(json!({"type": "user", "timestamp": "2026-09-24T10:01:00Z", "toolUseResult": {},
                "message": {"content": [{"type": "tool_result", "content": "tool output"}]}})),
            record(json!({"type": "attachment", "timestamp": "2026-09-24T10:02:00Z",
                "attachment": {"type": "queued_command", "origin": {"kind": "human"}, "prompt": "each person mines their own profile"}})),
            record(json!({"type": "attachment", "attachment": {"type": "queued_command", "prompt": "<task-notification>done</task-notification>"}})),
            record(json!({"type": "user", "isSidechain": true, "origin": {"kind": "human"}, "message": {"content": "subagent prompt"}})),
            record(json!({"type": "user", "origin": {"kind": "human"},
                "message": {"content": "<system-reminder>harness</system-reminder>"}})),
            record(json!({"type": "user", "message": {"content": "Base directory for this skill: an unmarked record"}})),
            "not json".to_string(),
        ]
        .join("\n");
        let turns = human_turns(&transcript);
        let texts: Vec<&str> = turns.iter().map(|turn| turn.text.as_str()).collect();
        assert_eq!(
            texts,
            ["never pad commits", "each person mines their own profile"],
            "an unmarked record is not human once the transcript carries origins"
        );
        let legacy =
            record(json!({"type": "user", "message": {"content": "legacy prompt without origin"}}));
        assert_eq!(human_turns(&legacy)[0].text, "legacy prompt without origin");
        let quoted_origin = record(json!({"type": "user",
            "message": {"content": "a message that mentions \"origin\" in quotes"}}));
        assert_eq!(
            human_turns(&quoted_origin).len(),
            1,
            "escaped text is not a marker"
        );
        assert_eq!(turns[0].at, "2026-09-24T10:00:00Z");
        assert_eq!(session_id(&transcript).as_deref(), Some("s-1"));
        assert!(quoted_turn(&turns, "each  person mines\ntheir own").is_some());
        assert!(
            quoted_turn(&turns, "pasted skills").is_none(),
            "pasted text is not the author's"
        );
        assert!(quoted_turn(&turns, "tool output").is_none());
    }

    #[test]
    fn statements_scopes_and_secrets_are_validated() {
        assert!(single_line("ok", "statement", 8, 200).is_err());
        assert_eq!(
            single_line("  Prefer   small\ncommits ", "statement", 8, 200).unwrap(),
            "Prefer small commits"
        );
        assert!(single_line("text <!-- mastermind-style:managed:end -->", "s", 8, 200).is_err());
        assert!(validate_scope("global").is_ok());
        assert!(validate_scope("language:rust").is_ok());
        assert!(validate_scope("role:auditor").is_ok());
        assert!(validate_scope("workflow:release").is_ok());
        assert!(validate_scope("role:release").is_err());
        assert!(validate_scope("team:x").is_err());
        for secret in [
            "token ghp_abc123",
            "key AIzaSyA",
            "glpat-x",
            "password=hunter2",
        ] {
            assert!(looks_secret(secret), "{secret}");
        }
        assert!(!looks_secret("prefer typed errors"));
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_720), "2026-09-24");
        assert_eq!(
            claude_project_slug(Path::new("/Users/a/Documents/edge-ai")),
            "-Users-a-Documents-edge-ai"
        );
    }

    #[test]
    fn transcripts_outside_claude_projects_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let forged = dir.path().join("forged.jsonl");
        std::fs::write(&forged, "{}\n").unwrap();
        assert!(session_transcript(&forged).is_err());
    }

    #[test]
    fn memory_files_of_type_feedback_or_user_import() {
        let feedback = "---\nname: verbatim-signatures\ndescription: Executor-report signatures are compared verbatim\nmetadata:\n  type: feedback\n---\n\nReflowing a signature reads as a mismatch.\n\n**Why:** audits compare bytes.\n";
        assert_eq!(
            memory_entry("-Users-a-mastermind", "sig.md", feedback),
            Some(MemoryEntry {
                statement: "Executor-report signatures are compared verbatim".to_string(),
                quote: "Reflowing a signature reads as a mismatch.".to_string(),
                scope: "project:-Users-a-mastermind".to_string(),
                source: "memory:-Users-a-mastermind/sig.md".to_string(),
            })
        );
        let user = "---\nname: role\ndescription: Staff engineer who reviews for rollback safety\ntype: user\n---\nBody of the memory\n";
        assert_eq!(memory_entry("p", "role.md", user).unwrap().scope, "global");
        let project = "---\nname: x\ndescription: A project fact that is not a preference\nmetadata:\n  type: project\n---\nBody\n";
        assert_eq!(memory_entry("p", "x.md", project), None);
        let secret = "---\nname: k\ndescription: Deploy key is sk-live-123 for staging\ntype: user\n---\nBody of the memory\n";
        assert_eq!(memory_entry("p", "k.md", secret), None);
    }
}
