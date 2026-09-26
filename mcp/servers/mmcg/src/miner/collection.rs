//! Bounded, local collection of possible persona signals. The inbox is
//! separate from both accepted preferences and reviewed habit claims.

use super::feedback::{self, CollectionBudget, CollectionTranscript, HumanTurn};
use super::profile::{mutate_private_store, persona_project_id, persona_repository_id};
use super::store::{
    CollectedCandidate, CollectionBatch, CollectionSource, CollectionStats, ProfileStore,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

pub mod search;
pub mod sources;

// Includes detector, attribution adapters and segmentation. Bump whenever an
// input can produce a different set of candidates or provenance bindings.
const EXTRACTOR: &str = "persona-explicit-v2";
const MAX_FILES: usize = 16;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LINES: usize = 1_000_000;
const MAX_CANDIDATES: usize = 512;

fn hash(value: Value) -> String {
    crate::hex::encode(&Sha256::digest(value.to_string().as_bytes()))
}

/// Match only short, complete, explicitly worded human segments. This is a
/// recall-limited cue detector, not a semantic assertion about the author.
fn detect(turn: &HumanTurn) -> Option<(String, &'static str, &'static str)> {
    detect_version(turn, true)
}

fn detect_version(
    turn: &HumanTurn,
    expanded: bool,
) -> Option<(String, &'static str, &'static str)> {
    let quote = feedback::single_line(&turn.text, "candidate quote", 8, 300).ok()?;
    if quote.contains('?')
        || quote.chars().any(char::is_control)
        || feedback::looks_secret(&quote)
        || crate::indexer::secret_like_documentation(&quote)
    {
        return None;
    }
    // Never combine visible fragments across quoted/code/HTML material.
    let visible: HashSet<usize> = crate::context_doctor::prose_lines(&turn.text)
        .into_iter()
        .map(|line| line.index)
        .collect();
    if crate::context_doctor::source_lines(&turn.text)
        .iter()
        .any(|line| !line.text.trim().is_empty() && !visible.contains(&line.index))
    {
        return None;
    }
    let lower = quote.to_lowercase();
    let wording = lower
        .strip_prefix("- ")
        .or_else(|| lower.strip_prefix("* "))
        .unwrap_or(&lower);
    let rules = [
        (
            "possible_stated_preference",
            "preference.en",
            &["i prefer ", "my preference is ", "i find it easier to "][..],
        ),
        (
            "possible_stated_preference",
            "preference.ru",
            &[
                "я предпочитаю ",
                "мне удобнее ",
                "в ответах мне важно ",
                "для меня важно ",
            ][..],
        ),
        (
            "possible_self_report",
            "habit.en",
            &["i usually ", "i tend to "][..],
        ),
        (
            "possible_self_report",
            "habit.ru",
            &["я обычно ", "я привык ", "я привыкла ", "у меня привычка "][..],
        ),
        (
            "possible_correction",
            "correction.en",
            &["from now on, ", "from now on ", "going forward, "][..],
        ),
        (
            "possible_correction",
            "correction.ru",
            &["впредь ", "больше не "][..],
        ),
    ];
    let cue = rules
        .into_iter()
        .find(|(_, _, prefixes)| prefixes.iter().any(|prefix| wording.starts_with(prefix)))
        .map(|(kind, rule, _)| (kind, rule));
    let (kind, rule) = cue.or_else(|| {
        if expanded {
            expanded_cue(wording)
        } else {
            None
        }
    })?;
    // Limit the first detector to work-related vocabulary. This is a filter,
    // not a claim that these keywords establish relevance or identity.
    let work = [
        "code",
        "coding",
        "review",
        "test",
        "commit",
        "repo",
        "task",
        "plan",
        "chang",
        "output",
        "document",
        "workflow",
        "reply",
        "repli",
        "answer",
        "debug",
        "clarif",
        "код",
        "ревью",
        "тест",
        "коммит",
        "репозитор",
        "задач",
        "план",
        "изменен",
        "изменён",
        "ответ",
        "документац",
        "провер",
        "работ",
        "формат",
    ];
    let engineering = [
        "state",
        "boundar",
        "effect",
        "failure",
        "rollout",
        "storage",
        "lifecycle",
        "состояни",
        "границ",
        "эффект",
        "отказ",
        "жизненн",
        "причин",
    ];
    if !wording.split(|c: char| !c.is_alphanumeric()).any(|word| {
        work.iter().any(|prefix| word.starts_with(prefix))
            || (expanded && engineering.iter().any(|prefix| word.starts_with(prefix)))
    }) {
        return None;
    }
    Some((quote, kind, rule))
}

fn expanded_cue(wording: &str) -> Option<(&'static str, &'static str)> {
    let cues = [
        (
            "possible_stated_preference",
            "preference.expanded.en",
            &["my default is ", "i would rather ", "i value "][..],
        ),
        (
            "possible_stated_preference",
            "preference.expanded.ru",
            &[
                "мне важно ",
                "мне проще ",
                "предпочитаю ",
                "в ответах ценю ",
            ][..],
        ),
        (
            "possible_self_report",
            "habit.expanded.en",
            &[
                "i generally ",
                "i normally ",
                "i always ",
                "i avoid ",
                "my habit is to ",
            ][..],
        ),
        (
            "possible_self_report",
            "habit.expanded.ru",
            &[
                "обычно я ",
                "как правило, я ",
                "я всегда ",
                "я стараюсь ",
                "я избегаю ",
                "я больше не ",
            ][..],
        ),
        (
            "possible_correction",
            "correction.expanded.en",
            &["in future replies, ", "for future reviews, "][..],
        ),
        (
            "possible_correction",
            "correction.expanded.ru",
            &["в следующих ответах ", "на будущее, ", "дальше в ответах "][..],
        ),
    ];
    if let Some((kind, rule, _)) = cues
        .into_iter()
        .find(|(_, _, prefixes)| prefixes.iter().any(|prefix| wording.starts_with(prefix)))
    {
        return Some((kind, rule));
    }
    // Keep the complete condition and any exception. These cues nominate a
    // self-report only; they do not establish recurrence or independent episodes.
    if !wording
        .chars()
        .any(|ch| matches!(ch, ':' | '"' | '“' | '”' | '«' | '»' | '‘' | '’'))
    {
        if wording.split_once(", ").is_some_and(|(condition, action)| {
            (condition.starts_with("when ") || condition.starts_with("before "))
                && action.starts_with("i ")
        }) {
            return Some(("possible_self_report", "habit.conditional.en"));
        }
        let personal_condition = [
            "когда проверяю ",
            "когда читаю ",
            "когда меняю ",
            "когда пишу ",
            "когда планирую ",
        ];
        let direct_condition = [
            "при ревью я ",
            "при проверке кода я ",
            "при изменении кода я ",
            "при планировании я ",
        ];
        if (personal_condition
            .iter()
            .any(|prefix| wording.starts_with(prefix))
            && wording
                .split_once(", ")
                .is_some_and(|(_, action)| action.starts_with("я ")))
            || direct_condition
                .iter()
                .any(|prefix| wording.starts_with(prefix))
        {
            return Some(("possible_self_report", "habit.conditional.ru"));
        }
    }
    None
}

#[cfg(test)]
mod evaluation;

fn extract(snapshot: &CollectionTranscript, source: &CollectionSource) -> Vec<CollectedCandidate> {
    let mut segments: HashMap<usize, usize> = HashMap::new();
    let mut seen = HashSet::new();
    snapshot
        .turns
        .iter()
        .filter_map(|turn| {
            let segment = segments.entry(turn.line_no).or_default();
            let segment_no = *segment;
            *segment += 1;
            let (quote, kind, rule) = detect(turn)?;
            // Repetition within a session cannot become independent support.
            let id = hash(json!(["persona-candidate-v1", source.source, kind, quote]));
            if !seen.insert(id.clone()) {
                return None;
            }
            let revision = hash(json!([
                EXTRACTOR,
                id,
                source.source_path,
                source.project_root,
                turn.line_no,
                segment_no,
                turn.record_digest,
                rule
            ]));
            Some(CollectedCandidate {
                id,
                source: source.source.clone(),
                kind: kind.into(),
                quote,
                source_path: source.source_path.clone(),
                line_no: turn.line_no,
                segment_no,
                record_digest: turn.record_digest.clone(),
                project_root: source.project_root.clone(),
                project: source.project.clone(),
                observed_at: turn.at.clone(),
                extractor: EXTRACTOR.into(),
                rule_id: rule.into(),
                revision,
                status: "pending".into(),
                present: true,
            })
        })
        .collect()
}

fn prepare(
    db: Option<&ProfileStore>,
    root: &Path,
    paths: &[PathBuf],
) -> Result<Vec<CollectionBatch>, Box<dyn std::error::Error>> {
    if paths.is_empty() || paths.len() > MAX_FILES {
        return Err("collection requires 1 to 16 explicit transcript paths".into());
    }
    let inputs: Vec<_> = paths
        .iter()
        .map(|path| CollectionInput::Explicit(path))
        .collect();
    prepare_inputs(db, root, &inputs)
}

enum CollectionInput<'a> {
    Explicit(&'a Path),
    Registered(&'a CollectionSource),
}

fn prepare_inputs(
    db: Option<&ProfileStore>,
    root: &Path,
    inputs: &[CollectionInput<'_>],
) -> Result<Vec<CollectionBatch>, Box<dyn std::error::Error>> {
    if inputs.len() > MAX_FILES {
        return Err("collection exceeds 16 transcript paths".into());
    }
    let root_text = root.to_str().ok_or("project root must be UTF-8")?;
    let project = persona_project_id(root).ok_or("cannot identify project root")?;
    let repository = persona_repository_id(root).unwrap_or_default();
    let mut budget = CollectionBudget {
        bytes: MAX_BYTES,
        lines: MAX_LINES,
    };
    let mut candidates = 0;
    let mut seen = HashMap::new();
    let mut batches = Vec::new();
    for input in inputs {
        let (path, checkpoint) = match input {
            CollectionInput::Explicit(path) => (*path, None),
            CollectionInput::Registered(source) => {
                if !sources::valid_source_id(&source.source) {
                    return Err("invalid registered source identity; inspect sources list".into());
                }
                if source.project_root != root_text || source.project != project {
                    return Err(format!("source {} cannot change project", source.source).into());
                }
                (Path::new(&source.source_path), Some(*source))
            }
        };
        let snapshot = feedback::collection_transcript(path, root, &mut budget).map_err(|error| {
            match checkpoint {
                Some(source) => format!("source {}: {error}; inspect sources list and use collect with an explicit path to register a relocation", source.source).into(),
                None => error,
            }
        })?;
        // Check the selected checkpoint before session-level deduplication.
        // A renamed path or a different session requires an explicit collect.
        if let Some(source) = checkpoint {
            if snapshot.source != source.source || snapshot.path != path {
                return Err(format!("source {} identity or canonical path changed; use collect with an explicit path after inspecting the source", source.source).into());
            }
        }
        if let Some(old) = seen.insert(snapshot.source.clone(), snapshot.digest.clone()) {
            if old != snapshot.digest {
                return Err("two selected files disagree about the same session snapshot".into());
            }
            continue;
        }
        let source = CollectionSource {
            source: snapshot.source.clone(),
            source_path: snapshot
                .path
                .to_str()
                .ok_or("transcript path must be UTF-8")?
                .into(),
            project_root: root_text.into(),
            project: project.clone(),
            repository: repository.clone(),
            snapshot_digest: snapshot.digest.clone(),
            extractor: EXTRACTOR.into(),
            bytes: snapshot.bytes,
            lines: snapshot.lines,
        };
        let old = db
            .map(|db| db.collection_source(&source.source))
            .transpose()?
            .flatten();
        if old.as_ref().is_some_and(|old| {
            old.project != source.project || old.project_root != source.project_root
        }) {
            return Err("collection source cannot change project".into());
        }
        let mut detected = if old.as_ref() == Some(&source) {
            Vec::new()
        } else {
            extract(&snapshot, &source)
        };
        candidates += detected.len();
        if candidates > MAX_CANDIDATES {
            return Err("collection exceeds 512 candidates; select fewer transcripts".into());
        }
        if let Some(db) = db {
            for candidate in &mut detected {
                if let Some(old) = db.collected_candidate(&candidate.id)? {
                    candidate.status = old.status;
                }
            }
        }
        batches.push(CollectionBatch {
            source,
            candidates: detected,
        });
    }
    Ok(batches)
}

pub fn collect(
    root: &Path,
    paths: &[PathBuf],
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err("project root must be a directory".into());
    }
    let db_path = ProfileStore::db_path().ok_or("could not resolve profile store")?;
    let result = if dry_run {
        let db = ProfileStore::open_optional_read_only(&db_path)?;
        prepare(db.as_ref(), &root, paths).map(|batches| {
            json!({
                "status":"complete", "dry_run":true, "extractor":EXTRACTOR,
                "sources":batches.iter().map(|b| &b.source).collect::<Vec<_>>(),
                "candidates":batches.iter().flat_map(|b| &b.candidates).collect::<Vec<_>>(),
                "scope":"unresolved", "episode":null,
                "note":"Preview includes observations from new or changed sources only; the existing inbox is unchanged.",
            })
        })
    } else {
        mutate_private_store(
            &db_path,
            |db| prepare(db, &root, paths).map(ControlFlow::Continue),
            |db, batches| {
                let stats: CollectionStats = db.collect_candidates(&batches)?;
                Ok(
                    json!({"status":"complete", "dry_run":false, "extractor":EXTRACTOR, "collection":stats}),
                )
            },
        )
    };
    let result = result.map_err(|error| {
        format!("collection incomplete; successful checkpoints unchanged: {error}")
    })?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub(super) struct Verifier {
    sources: HashMap<(String, String), Option<CollectionTranscript>>,
    budget: CollectionBudget,
    incomplete: bool,
}

impl Verifier {
    pub(super) fn new() -> Self {
        Self {
            sources: HashMap::new(),
            budget: CollectionBudget {
                bytes: MAX_BYTES,
                lines: MAX_LINES,
            },
            incomplete: false,
        }
    }

    pub(super) fn freshness(&mut self, c: &CollectedCandidate) -> &'static str {
        if !c.present {
            return "removed";
        }
        if c.extractor == super::hooks::EXTRACTOR {
            if self.budget.bytes < 512 * 1024 {
                self.incomplete = true;
                return "unchecked";
            }
            self.budget.bytes -= 512 * 1024;
            return if super::hooks::current_candidate(c) {
                "current"
            } else {
                "unavailable"
            };
        }
        if c.extractor != EXTRACTOR {
            return "extractor_changed";
        }
        let key = (c.source_path.clone(), c.project_root.clone());
        if !self.sources.contains_key(&key) {
            if self.sources.len() >= MAX_FILES {
                self.incomplete = true;
                return "unchecked";
            }
            let source = feedback::collection_transcript(
                Path::new(&c.source_path),
                Path::new(&c.project_root),
                &mut self.budget,
            );
            let source = source.ok();
            if source.is_none() {
                self.incomplete = true;
            }
            self.sources.insert(key.clone(), source);
        }
        let Some(source) = self.sources.get(&key).and_then(Option::as_ref) else {
            return "unavailable";
        };
        if snapshot_contains_candidate(source, c) {
            "current"
        } else {
            "changed"
        }
    }
}

pub(super) fn snapshot_contains_candidate(
    source: &CollectionTranscript,
    c: &CollectedCandidate,
) -> bool {
    if source.source != c.source || c.extractor != EXTRACTOR {
        return false;
    }
    let turn = source
        .turns
        .iter()
        .filter(|turn| turn.line_no == c.line_no)
        .nth(c.segment_no);
    turn.is_some_and(|turn| {
        turn.record_digest == c.record_digest
            && detect(turn).is_some_and(|(quote, kind, rule)| {
                quote == c.quote && kind == c.kind && rule == c.rule_id
            })
    })
}

pub(super) fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn list(
    after: Option<&str>,
    status: &str,
    limit: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if !(1..=100).contains(&limit)
        || !matches!(status, "all" | "pending" | "dismissed")
        || after.is_some_and(|id| !valid_id(id))
    {
        return Err("invalid candidate page arguments".into());
    }
    let path = ProfileStore::db_path().ok_or("could not resolve profile store")?;
    let db = ProfileStore::open_optional_read_only(&path)?;
    let mut rows = db
        .map(|db| db.collected_candidates(after.unwrap_or(""), status, limit + 1))
        .transpose()?
        .unwrap_or_default();
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next = more.then(|| rows.last().unwrap().id.clone());
    let mut verifier = Verifier::new();
    let items: Vec<Value> = rows.iter().map(|c| json!({
        "candidate":c, "freshness":verifier.freshness(c), "scope":"unresolved", "episode":null
    })).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "candidates":items, "next_cursor":next,
            "source_verification":if verifier.incomplete {"incomplete"} else {"complete"},
            "note":"Unreviewed local observations. No habits, scope, or independent episodes are inferred."
        }))?
    );
    Ok(())
}

pub fn show(id: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !valid_id(id) {
        return Err("candidate id must be a full 64-character key".into());
    }
    let path = ProfileStore::db_path().ok_or("could not resolve profile store")?;
    let db = ProfileStore::open_read_only(&path)?;
    let candidate = db.collected_candidate(id)?.ok_or("candidate not found")?;
    let mut verifier = Verifier::new();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "freshness":verifier.freshness(&candidate), "candidate":candidate,
            "scope":"unresolved", "episode":null, "history":db.candidate_history(id)?,
            "source_verification":if verifier.incomplete {"incomplete"} else {"complete"},
            "note":"Use candidates propose-habit or propose-preference with this exact revision to retain provenance; semantic support still needs review."
        }))?
    );
    Ok(())
}

pub fn dismiss(id: &str, revision: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !std::io::stdin().is_terminal() {
        return Err("candidate dismissal requires the author's interactive terminal".into());
    }
    if !valid_id(id) || !valid_id(revision) {
        return Err("candidate id and revision must be full 64-character keys".into());
    }
    let path = ProfileStore::db_path().ok_or("could not resolve profile store")?;
    mutate_private_store(
        &path,
        |db| {
            db.map(|_| ControlFlow::Continue(()))
                .ok_or_else(|| "candidate not found".into())
        },
        |db, ()| {
            if !db.dismiss_collected_candidate(id, revision)? {
                return Err("candidate missing or revision changed; inspect it again".into());
            }
            Ok(())
        },
    )?;
    println!(
        "{}",
        json!({"id":id, "status":"dismissed", "revision":revision})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(text: &str) -> HumanTurn {
        HumanTurn {
            at: "2026-09-26".into(),
            text: text.into(),
            line_no: 3,
            record_digest: "a".repeat(64),
            cwd: None,
            session_id: Some("session-a".into()),
        }
    }

    #[test]
    fn detector_preserves_negations_scope_and_self_reports_in_both_languages() {
        for (text, kind) in [
            (
                "I prefer short replies with the test results.",
                "possible_stated_preference",
            ),
            (
                "I prefer not to add tests only for this PR.",
                "possible_stated_preference",
            ),
            (
                "Я предпочитаю короткие ответы без лишних деталей.",
                "possible_stated_preference",
            ),
            (
                "Мне удобнее сначала читать план, потом смотреть код.",
                "possible_stated_preference",
            ),
            (
                "I usually review the contract before changing code.",
                "possible_self_report",
            ),
            (
                "Я обычно читаю документацию перед изменением кода.",
                "possible_self_report",
            ),
            (
                "From now on, show test results in the final reply.",
                "possible_correction",
            ),
            (
                "Больше не добавляй лишние детали в ответы.",
                "possible_correction",
            ),
            (
                "- I prefer short replies\n  only when the change is small.",
                "possible_stated_preference",
            ),
        ] {
            let (quote, actual, _) = detect(&turn(text)).unwrap_or_else(|| panic!("missed {text}"));
            assert_eq!(actual, kind);
            assert_eq!(quote, text.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }

    #[test]
    fn detector_abstains_from_commands_quoted_material_and_unrelated_details() {
        for text in [
            "Не коммить.",
            "Проверь тесты.",
            "Сделай короче.",
            "Always run tests.",
            "Review this code.",
            "I prefer coffee.",
            "We prefer short replies.",
            "Anton said: I prefer short replies.",
            "Anton said:\nI prefer short replies.",
            "> I prefer short replies.",
            "```\nI prefer short replies.\n```",
            "    I prefer short replies.",
            "\"I prefer short replies.\"",
            "I prefer short replies?",
            "I prefer short replies.\n> third party quote",
            "I prefer short replies.\n```\nagent content\n```",
            "I prefer short replies <!-- foreign text -->",
            "I prefer commits signed using ghp_sensitivecredential",
        ] {
            assert!(
                detect(&turn(text)).is_none(),
                "unexpected candidate: {text}"
            );
        }
        assert!(detect(&turn(&format!(
            "I prefer short replies {}",
            "x".repeat(300)
        )))
        .is_none());
    }

    #[test]
    fn exact_repeats_share_candidate_id_without_merging_other_sessions() {
        let first = turn("I prefer short replies.");
        let mut repeated = first.clone();
        repeated.line_no = 4;
        let snapshot = CollectionTranscript {
            path: PathBuf::from("/source"),
            source: "session:codex:a".into(),
            digest: "d".repeat(64),
            bytes: 100,
            lines: 4,
            turns: vec![first, repeated],
            record_sizes: HashMap::new(),
        };
        let mut source = CollectionSource {
            source: snapshot.source.clone(),
            source_path: "/source".into(),
            project_root: "/project".into(),
            project: "p".into(),
            repository: String::new(),
            snapshot_digest: snapshot.digest.clone(),
            extractor: EXTRACTOR.into(),
            bytes: 100,
            lines: 4,
        };
        let candidates = extract(&snapshot, &source);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].line_no, 3);
        source.source_path = "/archive".into();
        let archived = extract(&snapshot, &source);
        assert_eq!(archived[0].id, candidates[0].id);
        assert_ne!(archived[0].revision, candidates[0].revision);
        source.source = "session:codex:b".into();
        assert_ne!(extract(&snapshot, &source)[0].id, candidates[0].id);
    }
}
