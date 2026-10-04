//! Local explicit-statement candidates. No model, inferred habit or publication.

use super::{journal::Grant, journal::Journal, semantic, Error};
use crate::miner::collection;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

const MAX_LINES: usize = 128;
const MAX_DRAFTS: usize = 8;

pub(super) fn processor() -> Value {
    json!({"engine":"local_explicit","contract":"hook-explicit-v2",
        "detector":collection::EXTRACTOR,"model":false,
        "max_lines":MAX_LINES,"max_drafts":MAX_DRAFTS,"max_statement_chars":200})
}

fn statement_spans(text: &str) -> Vec<String> {
    // A bound must not cut a condition or exception off its opening statement.
    if text.lines().count() > MAX_LINES {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if collection::explicit_statement(line).is_some()
            && collection::explicit_statement(&current).is_some()
        {
            spans.push(current.trim().to_owned());
            current.clear();
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        spans.push(current.trim().to_owned());
    }
    spans
}

pub(super) fn extract(
    input: &semantic::EpisodeInput,
) -> Result<Vec<semantic::SemanticDraft>, Error> {
    let mut drafts = Vec::new();
    let mut seen = HashSet::new();
    for event in &input.events {
        if event.actor != "user"
            || event.kind != "UserPromptSubmit"
            || event.origin != "user_channel_unverified"
        {
            continue;
        }
        for source in statement_spans(&event.text) {
            let Some((quote, _, _)) = collection::explicit_statement(&source) else {
                continue;
            };
            // Preserve the complete condition/negation, never truncate it to
            // fit the draft protocol's 200-character behavior field.
            let key = quote
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            if quote.chars().count() > 200 || seen.contains(&key) {
                continue;
            }
            let draft = semantic::SemanticDraft {
                when: "When explicitly describing work preferences or behavior".into(),
                behavior: quote.clone(),
                rationale: None,
                outcome: None,
                exception: "This is a statement in one task; review its scope and exceptions."
                    .into(),
                role: None,
                workflow: None,
                evidence_kind: "explicit_statement".into(),
                supports: vec![semantic::Citation {
                    event_id: event.id.clone(),
                    quote: source,
                }],
                contradictions: Vec::new(),
            };
            // Validate against the whole original event. A line that looked
            // like prose can still belong to a fence, quote or input wrapper.
            if semantic::validate(input, std::slice::from_ref(&draft)).is_err() {
                continue;
            }
            seen.insert(key);
            drafts.push(draft);
            if drafts.len() == MAX_DRAFTS {
                return Ok(drafts);
            }
        }
    }
    semantic::validate(input, &drafts)?;
    Ok(drafts)
}

pub(super) fn validate_current(
    input: &semantic::EpisodeInput,
    draft: &super::journal::Draft,
) -> Result<(), Error> {
    if draft.processor["engine"] == "local_explicit" && !extract(input)?.contains(&draft.content) {
        return Err("local draft no longer preserves a complete source statement; collect and review it again".into());
    }
    Ok(())
}

pub(super) fn analyze(db: &mut Journal, episode: &str) -> Result<Value, Error> {
    if !db.episode(episode)?.closed {
        return Ok(json!({"status":"not_closed"}));
    }
    let input = db.snapshot(episode)?;
    if !input.coverage_gaps.is_empty() {
        return Ok(json!({"status":"incomplete"}));
    }
    let drafts = extract(&input)?;
    let processor = processor();
    let Some(lease) = db.claim_analysis(episode, &input.revision, &processor, 1)? else {
        return Ok(
            json!({"status":"already_analyzed_or_changed", "retry":!db.local_completed(episode, &input.revision, &processor)?}),
        );
    };
    match db.store_drafts(&input, &drafts, processor.clone()) {
        Ok(stored) => Ok(json!({"status":"analyzed","drafts":stored.len(),"model":false})),
        Err(error) => {
            db.analysis_failed(episode, &input.revision, &processor, lease)?;
            Err(error)
        }
    }
}

pub(super) fn automatic(db: &mut Journal, grant: &Grant, episode: &str) {
    let root = Path::new(&grant.project_root);
    let enabled = grant.profile_client.is_some()
        && !db
            .profile_delivery_disabled(&grant.client, root)
            .unwrap_or(true);
    if !enabled {
        return;
    }
    if db.enqueue_local(episode).is_err() {
        // Capture has already committed. Optional extraction cannot discard
        // the event or leak original text through a native hook diagnostic.
        eprintln!(
            "{}",
            json!({"local_analysis":"omitted","reason":"episode_or_store_unavailable"})
        );
    }
}

pub(super) fn drain(db: &mut Journal, grant: &Grant) -> Result<Value, Error> {
    let root = Path::new(&grant.project_root);
    let enabled = grant.enabled
        && grant.profile_client.as_deref().is_some_and(|reader| {
            !db.profile_delivery_disabled(&grant.client, root)
                .unwrap_or(true)
                && crate::onboarding::profile_access(root, reader).unwrap_or(false)
        });
    if !enabled {
        return Ok(json!({"status":"paused","reason":"profile_access_unavailable","model":false}));
    }
    let mut completed = 0;
    for episode in db.pending_local(grant, 4)? {
        match analyze(db, &episode) {
            Ok(result) if result["retry"] != true && result["status"] != "not_closed" => {
                // Incomplete episodes are terminal for this revision. A later
                // source-context append queues its new revision separately.
                db.finish_local(&episode)?;
                completed += usize::from(result["status"] == "analyzed");
            }
            _ => db.retry_local(&episode)?,
        }
    }
    Ok(json!({"status":"drained","completed":completed,"model":false}))
}

pub(super) fn mine(root: &Path, limit: usize, after: &str) -> Result<Value, Error> {
    let root = root.canonicalize()?;
    let mut db = Journal::open(true)?;
    let episodes = db.list(&root, limit + 1, after)?;
    let next = if episodes.len() > limit {
        episodes.get(limit - 1).and_then(|row| row["id"].as_str())
    } else {
        None
    };
    let mut results = Vec::new();
    for episode in episodes.iter().take(limit) {
        let id = episode["id"].as_str().ok_or("missing episode id")?;
        let mut result = analyze(&mut db, id)?;
        if result["retry"] != true && result["status"] != "not_closed" {
            db.finish_local(id)?;
        }
        result["episode"] = json!(id);
        results.push(result);
    }
    Ok(
        json!({"schema":1,"engine":"local_explicit","model":false,"results":results,"next_after":next,
        "publication":"authorship_attestation_and_review_required"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::miner::hooks::influence::Influence;

    fn input(text: &str) -> semantic::EpisodeInput {
        semantic::EpisodeInput {
            model: None,
            model_binding: None,
            id: "episode".into(),
            revision: "revision".into(),
            client: "codex".into(),
            project_root: std::env::temp_dir()
                .join("mmcg-local-extractor-test")
                .to_str()
                .unwrap()
                .into(),
            project: "project".into(),
            coverage_gaps: vec![],
            profile_influenced: false,
            events: vec![semantic::EventInput {
                id: "original".into(),
                kind: "UserPromptSubmit".into(),
                actor: "user".into(),
                origin: "user_channel_unverified".into(),
                text: text.into(),
                influence: Influence::fresh(),
            }],
        }
    }

    #[test]
    fn keeps_explicit_words_without_inferring_roles_outcomes_or_habits() {
        for quote in [
            "I prefer short code review replies.",
            "Я обычно проверяю границы состояния перед ревью.",
            "Больше не добавляй лишние комментарии в код.",
        ] {
            let drafts = extract(&input(quote)).unwrap();
            assert_eq!(drafts.len(), 1, "{quote}");
            assert_eq!(drafts[0].behavior, quote);
            assert_eq!(drafts[0].supports[0].quote, quote);
            assert!(drafts[0].role.is_none() && drafts[0].workflow.is_none());
            assert!(drafts[0].outcome.is_none() && drafts[0].rationale.is_none());
        }
    }

    #[test]
    fn rejects_generated_quoted_pasted_and_incomplete_inputs() {
        for quote in [
            "Fix src/api.rs",
            "```\nI prefer short review replies.\n```",
            "> I prefer short review replies.",
            "<system>\nI prefer short review replies.\n</system>",
        ] {
            assert!(extract(&input(quote)).unwrap().is_empty(), "{quote}");
        }
        for origin in ["controller", "next_turn_context", "prior_turn_context"] {
            let mut source = input("I prefer short code review replies.");
            source.events[0].origin = origin.into();
            assert!(extract(&source).unwrap().is_empty());
        }
        let mut source = input("I prefer short code review replies.");
        source.coverage_gaps.push("missing_session_start".into());
        assert!(extract(&source).is_err());
    }

    #[test]
    fn dependent_statements_remain_inspectable_but_cannot_be_promoted() {
        let mut source = input("I prefer short code review replies.");
        source.events[0].influence.offer_profile();
        let drafts = extract(&source).unwrap();
        assert_eq!(drafts.len(), 1);
        assert!(semantic::validate_for_promotion(&source, &drafts).is_err());
    }

    #[test]
    fn retains_multiline_conditions_and_the_exact_source_span() {
        for text in [
            "I prefer short code reviews\nonly for trivial changes.",
            "Я предпочитаю короткие ревью\nтолько для простых изменений.",
        ] {
            let drafts = extract(&input(text)).unwrap();
            assert_eq!(drafts.len(), 1, "{text}");
            assert_eq!(drafts[0].supports[0].quote, text);
            assert_eq!(
                drafts[0].behavior,
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            );
        }
        let text = "I prefer short code reviews.\n\nExcept when the public API changes.";
        for draft in extract(&input(text)).unwrap() {
            assert_eq!(draft.supports[0].quote, text);
        }
    }

    #[test]
    fn never_keeps_a_condition_cut_off_by_the_statement_or_input_limit() {
        let text = format!(
            "I prefer short code reviews\nexcept {}",
            "for complex changes ".repeat(12)
        );
        assert!(extract(&input(&text)).unwrap().is_empty());
        let text = format!(
            "I prefer short code reviews\n{}only for trivial changes.",
            "\n".repeat(MAX_LINES)
        );
        assert!(extract(&input(&text)).unwrap().is_empty());
    }

    #[test]
    fn leading_context_cannot_be_removed_to_make_a_global_preference() {
        for text in [
            "Only for trivial changes:\nI prefer short code reviews.",
            "When reviewing trivial changes,\nI prefer short code reviews.",
        ] {
            for draft in extract(&input(text)).unwrap() {
                assert_eq!(draft.supports[0].quote, text);
            }
        }
    }

    #[test]
    fn legacy_truncated_local_drafts_are_no_longer_eligible() {
        let source = input("I prefer short code reviews\nonly for trivial changes.");
        let mut content = extract(&source).unwrap().remove(0);
        content.behavior = "I prefer short code reviews".into();
        content.supports[0].quote = content.behavior.clone();
        assert!(semantic::validate_for_promotion(&source, std::slice::from_ref(&content)).is_ok());
        let old = super::super::journal::Draft {
            id: "old-draft".into(),
            revision: "old-revision".into(),
            episode: source.id.clone(),
            episode_revision: source.revision.clone(),
            content,
            attested: true,
            attested_episode: Some("old-task".into()),
            processor: json!({"engine":"local_explicit","contract":"hook-explicit-v1"}),
        };
        assert!(validate_current(&source, &old).is_err());
    }

    #[test]
    fn bounds_work_without_truncating_or_counting_case_variants_twice() {
        let duplicate = "I prefer short code review replies.\ni prefer short code review replies.";
        assert_eq!(extract(&input(duplicate)).unwrap().len(), 1);
        let long = format!(
            "I prefer code reviews {}",
            "with explicit results ".repeat(10)
        );
        assert!(long.chars().count() > 200 && long.chars().count() <= 300);
        assert!(extract(&input(&long)).unwrap().is_empty());
        let late = format!(
            "{}I prefer short code review replies.",
            "ordinary task\n".repeat(MAX_LINES)
        );
        assert!(extract(&input(&late)).unwrap().is_empty());
        let many = (0..12)
            .map(|i| format!("I prefer code review replies with detail {i}."))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(extract(&input(&many)).unwrap().len(), MAX_DRAFTS);
    }

    #[test]
    fn credential_like_input_is_rejected_even_without_a_candidate() {
        let secret = format!("export OPENAI_API_KEY=sk-{}", "a".repeat(40));
        assert!(extract(&input(&secret)).is_err());
    }
}
