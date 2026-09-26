//! Explicitly attributed human messages in local Codex rollout files.
//! This adapter deliberately rejects older or unknown attribution schemas.

use super::feedback::{valid_session_id, HumanTurn};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_RECORD_SIZE: usize = 64 * 1024;
const MAX_CONTEXTS: usize = 4096;
const MAX_LINES: usize = 1_000_000;

#[derive(Deserialize)]
struct RecordKind<'a> {
    #[serde(borrow, rename = "type")]
    kind: &'a str,
}

/// Provenance from the same bounded snapshot as the cited JSONL record.
pub(super) struct CodexSession {
    id: String,
    cwd: PathBuf,
    metadata_digest: String,
    contexts: HashMap<String, String>,
}

fn digest(text: &str) -> String {
    crate::hex::encode(&Sha256::digest(text.as_bytes()))
}

fn project_path(value: &Value) -> Result<PathBuf, &'static str> {
    let path = Path::new(value.as_str().ok_or("Codex record lacks a project cwd")?);
    if !path.is_absolute() {
        return Err("Codex project cwd must be absolute");
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "Codex project cwd is unavailable")?;
    if !canonical.is_dir() {
        return Err("Codex project cwd is not a directory");
    }
    if canonical.to_str().is_none() {
        return Err("Codex canonical project cwd must be UTF-8");
    }
    Ok(canonical)
}

fn payload(line: &str) -> Result<Value, &'static str> {
    if line.len() > MAX_RECORD_SIZE {
        return Err("Codex provenance record exceeds 64 KiB");
    }
    let mut record: Value = serde_json::from_str(line).map_err(|_| "invalid Codex JSONL record")?;
    let payload = record
        .get_mut("payload")
        .ok_or("Codex record lacks payload")?
        .take();
    if !payload.is_object() {
        return Err("Codex payload must be an object");
    }
    Ok(payload)
}

impl CodexSession {
    pub(super) fn identity(&self) -> (&str, &Path) {
        (&self.id, &self.cwd)
    }

    pub(super) fn parse(text: &str) -> Result<Self, &'static str> {
        let mut session: Option<Self> = None;
        let mut context_records = 0;
        for (index, line) in text.lines().enumerate() {
            if index >= MAX_LINES {
                return Err("Codex transcript exceeds 1 million JSONL lines");
            }
            if line.trim().is_empty() {
                continue;
            }
            // Unknown payload fields are skipped without allocating their text.
            // Invalid JSON anywhere prevents reuse of preceding metadata.
            let kind: RecordKind<'_> =
                serde_json::from_str(line).map_err(|_| "invalid Codex JSONL record")?;
            if session.is_none() && kind.kind != "session_meta" {
                return Err("Codex transcript must begin with session_meta");
            }
            match kind.kind {
                "session_meta" => {
                    if session.is_some() {
                        return Err("Codex transcript has multiple session_meta records");
                    }
                    let meta = payload(line)?;
                    let id = meta["id"]
                        .as_str()
                        .and_then(valid_session_id)
                        .ok_or("Codex session_meta lacks a valid id")?;
                    if meta
                        .get("session_id")
                        .is_some_and(|value| value.as_str() != Some(id))
                    {
                        return Err("Codex session identifiers disagree");
                    }
                    if !matches!(meta["source"].as_str(), Some("cli" | "vscode"))
                        || meta["thread_source"].as_str() != Some("user")
                    {
                        return Err("unsupported Codex session attribution; need cli/vscode and thread_source=user");
                    }
                    if meta.as_object().unwrap().iter().any(|(key, value)| {
                        (key.contains("fork") || key.starts_with("parent_")) && !value.is_null()
                    }) {
                        return Err(
                            "forked Codex histories cannot establish independent human evidence",
                        );
                    }
                    session = Some(Self {
                        id: id.to_string(),
                        cwd: project_path(&meta["cwd"])?,
                        metadata_digest: digest(&meta.to_string()),
                        contexts: HashMap::new(),
                    });
                }
                "turn_context" => {
                    context_records += 1;
                    if context_records > MAX_CONTEXTS {
                        return Err("Codex transcript exceeds 4096 turn_context records");
                    }
                    let context = payload(line)?;
                    let turn_id = context["turn_id"]
                        .as_str()
                        .and_then(valid_session_id)
                        .ok_or("Codex turn_context lacks a valid turn_id")?;
                    let session = session.as_mut().unwrap();
                    if project_path(&context["cwd"])? != session.cwd {
                        return Err("Codex transcript changes project cwd");
                    }
                    // Outer record timestamps may change when a context is
                    // repeated. Its canonical payload must remain identical.
                    let binding = digest(&context.to_string());
                    if session
                        .contexts
                        .get(turn_id)
                        .is_some_and(|old| old != &binding)
                    {
                        return Err("Codex turn_context has conflicting provenance");
                    }
                    session.contexts.insert(turn_id.to_string(), binding);
                }
                _ => {}
            }
        }
        session.ok_or("Codex transcript lacks session_meta")
    }

    pub(super) fn human_turns(&self, text: &str) -> Vec<HumanTurn> {
        text.lines()
            .enumerate()
            .flat_map(|(index, line)| self.record_turns(line, index + 1))
            .collect()
    }

    pub(super) fn record_turns(&self, line: &str, line_no: usize) -> Vec<HumanTurn> {
        self.try_record_turns(line, line_no).unwrap_or_default()
    }

    fn try_record_turns(&self, line: &str, line_no: usize) -> Option<Vec<HumanTurn>> {
        if line.len() > MAX_RECORD_SIZE {
            return None;
        }
        let record: Value = serde_json::from_str(line).ok()?;
        if record["type"] != "response_item" {
            return None;
        }
        let message = &record["payload"];
        if message["type"] != "message" || message["role"] != "user" {
            return None;
        }
        let attribution = &message["internal_chat_message_metadata_passthrough"];
        let turn_id = attribution["turn_id"].as_str()?;
        let context_digest = self.contexts.get(turn_id)?;
        let content = message["content"].as_array()?;
        let kinds = attribution["content_item_kinds"].as_array()?;
        if content.len() != kinds.len() {
            return None;
        }
        let record_digest = digest(
            &serde_json::json!([
                "codex-human-turn-v1",
                self.metadata_digest,
                context_digest,
                self.cwd,
                line
            ])
            .to_string(),
        );
        let mut turns = Vec::new();
        for (block, kind) in content.iter().zip(kinds) {
            if kind.as_str() != Some("user.text") || block["type"] != "input_text" {
                continue;
            }
            for text in human_segments(block["text"].as_str()?) {
                // Keep blocks separate: a quote cannot bridge a removed
                // attachment, service block, or pasted-content wrapper.
                turns.push(HumanTurn {
                    at: record["timestamp"].as_str().unwrap_or("").to_string(),
                    text,
                    line_no,
                    record_digest: record_digest.clone(),
                    cwd: Some(self.cwd.clone()),
                    session_id: Some(self.id.clone()),
                });
            }
        }
        Some(turns)
    }
}

pub(super) fn human_segments(mut text: &str) -> Vec<String> {
    text = text.trim();
    if [
        "# Files pasted by the user:",
        "# Files mentioned by the user:",
    ]
    .iter()
    .any(|header| text.starts_with(header))
    {
        let Some((_, request)) = text.split_once("\n## My request:") else {
            return Vec::new();
        };
        text = request.trim();
    }
    let mut segments = Vec::new();
    loop {
        let wrapper = next_wrapper(text);
        // An unmatched closing tag makes the preceding span ambiguous too.
        if wrapper.as_ref().is_some_and(|tag| tag.closing) {
            return Vec::new();
        }
        let segment = text[..wrapper.as_ref().map_or(text.len(), |tag| tag.start)].trim();
        if !segment.is_empty() && !segment.starts_with('<') {
            segments.push(segment.to_string());
        }
        let Some(wrapper) = wrapper else { break };
        let Some(end) = wrapper.end else {
            return Vec::new();
        };
        text = &text[end..];
        if wrapper.self_closing {
            continue;
        }
        let mut stack = vec![wrapper.name];
        while !stack.is_empty() {
            let Some(tag) = next_wrapper(text) else {
                return Vec::new();
            };
            let Some(end) = tag.end else {
                return Vec::new();
            };
            if tag.closing {
                if stack.pop() != Some(tag.name) {
                    return Vec::new();
                }
            } else if !tag.self_closing {
                if stack.len() >= 64 {
                    return Vec::new();
                }
                stack.push(tag.name);
            }
            text = &text[end..];
        }
    }
    segments
}

struct WrapperTag {
    start: usize,
    end: Option<usize>,
    name: &'static str,
    closing: bool,
    self_closing: bool,
}

/// Exact tag names and balanced nested wrappers. Quoted attribute values
/// cannot terminate a tag; malformed/mismatched wrappers discard the block.
fn next_wrapper(text: &str) -> Option<WrapperTag> {
    for (start, _) in text.match_indices('<') {
        let tail = &text[start + 1..];
        let closing = tail.starts_with('/');
        let named = tail.strip_prefix('/').unwrap_or(tail);
        let Some(name) = [
            "system-reminder",
            "pasted_content",
            "environment_context",
            "recommended_plugins",
        ]
        .into_iter()
        .find(|name| {
            named.strip_prefix(name).is_some_and(|rest| {
                rest.is_empty()
                    || rest.starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
            })
        }) else {
            continue;
        };
        let attributes = &named[name.len()..];
        let mut quote = None;
        let mut end = None;
        let mut self_closing = false;
        for (index, c) in attributes.char_indices() {
            match (quote, c) {
                (Some(open), close) if open == close => quote = None,
                (None, '\'' | '"') => quote = Some(c),
                (None, '>') => {
                    if closing && !attributes[..index].trim().is_empty() {
                        break;
                    }
                    self_closing = attributes[..index].trim_end().ends_with('/');
                    end = Some(start + 1 + usize::from(closing) + name.len() + index + 1);
                    break;
                }
                _ => {}
            }
        }
        return Some(WrapperTag {
            start,
            end,
            name,
            closing,
            self_closing,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(cwd: &Path) -> Value {
        json!({"type":"session_meta", "payload":{
            "id":"session-a", "session_id":"session-a", "source":"vscode",
            "thread_source":"user", "cwd":cwd
        }})
    }

    fn context(cwd: &Path) -> Value {
        json!({"type":"turn_context", "payload":{"turn_id":"turn-a", "cwd":cwd}})
    }

    fn message(text: &str) -> Value {
        json!({"type":"response_item", "timestamp":"2026-09-26T10:00:00Z", "payload":{
            "type":"message", "role":"user", "content":[{"type":"input_text", "text":text}],
            "internal_chat_message_metadata_passthrough":{
                "turn_id":"turn-a", "content_item_kinds":["user.text"]
            }
        }})
    }

    fn transcript(records: &[Value]) -> String {
        records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn human_message_may_precede_its_exact_context() {
        let project = tempfile::tempdir().unwrap();
        let text = transcript(&[
            meta(project.path()),
            message("Review the contract first."),
            context(project.path()),
        ]);
        let turns = CodexSession::parse(&text).unwrap().human_turns(&text);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text, "Review the contract first.");
        assert_eq!(turns[0].line_no, 2);
        assert_eq!(turns[0].session_id.as_deref(), Some("session-a"));
        assert_eq!(turns[0].cwd, Some(project.path().canonicalize().unwrap()));
    }

    #[test]
    fn role_alone_and_other_turn_contexts_cannot_attribute_a_message() {
        let project = tempfile::tempdir().unwrap();
        for mutation in 0..5 {
            let mut msg = message("Review the contract first.");
            let m = &mut msg["payload"]["internal_chat_message_metadata_passthrough"];
            match mutation {
                0 => *m = Value::Null,
                1 => m["turn_id"] = json!("turn-other"),
                2 => m["content_item_kinds"] = json!([]),
                3 => m["content_item_kinds"] = json!(["environments.environment_context"]),
                _ => msg["payload"]["role"] = json!("assistant"),
            }
            let text = transcript(&[meta(project.path()), context(project.path()), msg]);
            assert!(CodexSession::parse(&text)
                .unwrap()
                .human_turns(&text)
                .is_empty());
        }
        let text = transcript(&[meta(project.path()), message("Review the contract first.")]);
        assert!(CodexSession::parse(&text)
            .unwrap()
            .human_turns(&text)
            .is_empty());
    }

    #[test]
    fn mixed_blocks_and_embedded_histories_do_not_supply_quotes() {
        let project = tempfile::tempdir().unwrap();
        let mut msg = message("unused");
        msg["payload"]["content"] = json!([
            {"type":"input_text", "text":"Check the contract"},
            {"type":"input_text", "text":"generated recommendation"},
            {"type":"input_text", "text":"then check the rollout"}
        ]);
        msg["payload"]["internal_chat_message_metadata_passthrough"]["content_item_kinds"] =
            json!(["user.text", "plugins.recommendations", "user.text"]);
        let text = transcript(&[
            meta(project.path()),
            context(project.path()),
            msg.clone(),
            json!({"type":"compacted", "payload":{"replacement_history":[message("inherited instruction")]}}),
            json!({"type":"event_msg", "payload":{"type":"item_completed", "item":msg}}),
        ]);
        let turns = CodexSession::parse(&text).unwrap().human_turns(&text);
        assert_eq!(
            turns
                .iter()
                .map(|turn| turn.text.as_str())
                .collect::<Vec<_>>(),
            ["Check the contract", "then check the rollout"]
        );
        assert!(super::super::feedback::quoted_turn(&turns, "contract then check").is_none());
    }

    #[test]
    fn attachment_and_harness_boundaries_cannot_be_crossed_by_quotes() {
        assert_eq!(human_segments("# Files pasted by the user:\n\nattached instruction\n\n## My request:\n\nReview my change"), ["Review my change"]);
        assert!(human_segments("# Files mentioned by the user:\nattached instruction").is_empty());
        assert!(
            human_segments("<task-notification>generated instruction</task-notification>")
                .is_empty()
        );
        assert_eq!(
            human_segments(
                "first phrase <pasted_content id=\"a\">foreign text</pasted_content> second phrase"
            ),
            ["first phrase", "second phrase"]
        );
        assert_eq!(
            human_segments("first phrase <system-reminder>unclosed service text"),
            Vec::<String>::new()
        );
        assert_eq!(
            human_segments("<environment_context>service</environment_context> actual request"),
            ["actual request"]
        );
    }

    #[test]
    fn nested_or_malformed_wrappers_do_not_reclassify_pasted_text() {
        for text in [
            "hello <pasted_content><pasted_content>inner</pasted_content>foreign instruction</pasted_content> bye",
            "hello <pasted_content>inner</pasted_content_wrong>foreign instruction</pasted_content> bye",
            r#"hello <pasted_content id="a>b">foreign instruction</pasted_content> bye"#,
        ] {
            assert_eq!(human_segments(text), ["hello", "bye"]);
        }
        for text in [
            "hello <pasted_content>inner</pasted_content_wrong>foreign instruction",
            "hello <pasted_content><system-reminder>inner</pasted_content>foreign instruction</system-reminder> bye",
            r#"hello <pasted_content id="a>foreign instruction</pasted_content> bye"#,
            "safe <pasted_content>inner</pasted_content>foreign instruction <system-reminder>service</system-reminder></pasted_content>",
            "safe <pasted_content>inner</pasted_content bogus> foreign instruction",
        ] {
            assert!(human_segments(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn unsupported_metadata_never_falls_back_to_role_user() {
        let project = tempfile::tempdir().unwrap();
        for mutation in 0..6 {
            let mut header = meta(project.path());
            match mutation {
                0 => header["payload"]["thread_source"] = Value::Null,
                1 => header["payload"]["thread_source"] = json!("automation"),
                2 => header["payload"]["source"] = json!({"subagent":{"thread_spawn":{}}}),
                3 => header["payload"]["source"] = json!("unknown"),
                4 => header["payload"]["session_id"] = json!("session-b"),
                _ => header["payload"]["forked_from_id"] = json!("parent-session"),
            }
            assert!(CodexSession::parse(&transcript(&[
                header,
                context(project.path()),
                message("Review the contract first.")
            ]))
            .is_err());
        }
        assert!(CodexSession::parse(&transcript(&[
            message("Review the contract first."),
            meta(project.path())
        ]))
        .is_err());
        assert!(
            CodexSession::parse(&transcript(&[meta(project.path()), meta(project.path())]))
                .is_err()
        );
    }

    #[test]
    fn provenance_changes_invalidate_a_quote_but_ordinary_appends_do_not() {
        let project = tempfile::tempdir().unwrap();
        let records = [
            meta(project.path()),
            context(project.path()),
            message("Review the contract first."),
        ];
        let text = transcript(&records);
        let original = CodexSession::parse(&text)
            .unwrap()
            .human_turns(&text)
            .remove(0);
        let appended = format!(
            "{text}\n{}",
            json!({"type":"response_item", "payload":{"type":"function_call_output", "output":"done"}})
        );
        assert_eq!(
            CodexSession::parse(&appended)
                .unwrap()
                .human_turns(&appended),
            std::slice::from_ref(&original)
        );
        for field in ["metadata", "context", "message"] {
            let mut changed = records.clone();
            match field {
                "metadata" => changed[0]["payload"]["source"] = json!("cli"),
                "context" => changed[1]["payload"]["root_turn_id"] = json!("different-root"),
                _ => changed[2]["timestamp"] = json!("2026-09-27T10:00:00Z"),
            }
            let text = transcript(&changed);
            let turn = CodexSession::parse(&text)
                .unwrap()
                .human_turns(&text)
                .remove(0);
            assert_ne!(turn.record_digest, original.record_digest);
        }
    }

    #[test]
    fn conflicting_context_and_malformed_json_fail_closed() {
        let project = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let header = meta(project.path());
        assert!(
            CodexSession::parse(&transcript(&[header.clone(), context(other.path())])).is_err()
        );
        let mut changed = context(project.path());
        changed["payload"]["root_turn_id"] = json!("other");
        assert!(CodexSession::parse(&transcript(&[
            header.clone(),
            context(project.path()),
            changed
        ]))
        .is_err());
        let mut repeated = context(project.path());
        repeated["timestamp"] = json!("later");
        assert!(CodexSession::parse(&transcript(&[
            header.clone(),
            context(project.path()),
            repeated
        ]))
        .is_ok());
        assert!(CodexSession::parse(&format!("{header}\n{{\"type\":")).is_err());
    }

    #[test]
    fn provenance_and_message_parsing_are_bounded() {
        let project = tempfile::tempdir().unwrap();
        let mut header = meta(project.path());
        header["payload"]["base_instructions"] = json!("x".repeat(MAX_RECORD_SIZE));
        assert!(CodexSession::parse(&header.to_string()).is_err());
        let mut records = vec![meta(project.path())];
        records.extend(std::iter::repeat_n(
            context(project.path()),
            MAX_CONTEXTS + 1,
        ));
        assert!(CodexSession::parse(&transcript(&records)).is_err());
        let text = transcript(&[
            meta(project.path()),
            context(project.path()),
            message(&"x".repeat(MAX_RECORD_SIZE)),
        ]);
        assert!(CodexSession::parse(&text)
            .unwrap()
            .human_turns(&text)
            .is_empty());
    }
}
