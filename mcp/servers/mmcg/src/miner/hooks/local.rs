//! Local explicit-statement candidates. No model, inferred habit or publication.

use super::{journal::Grant, journal::Journal, semantic, Error};
use crate::miner::collection;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

const MAX_LINES: usize = 128;
const MAX_DRAFTS: usize = 8;

pub(super) fn processor() -> Value {
    json!({"engine":"local_explicit","contract":"hook-explicit-v1",
        "detector":collection::EXTRACTOR,"model":false,
        "max_lines":MAX_LINES,"max_drafts":MAX_DRAFTS,"max_statement_chars":200})
}

fn extract(input: &semantic::EpisodeInput) -> Result<Vec<semantic::SemanticDraft>, Error> {
    let mut drafts = Vec::new();
    let mut seen = HashSet::new();
    for event in &input.events {
        if event.actor != "user"
            || event.kind != "UserPromptSubmit"
            || event.origin != "user_channel_unverified"
        {
            continue;
        }
        for line in event.text.lines().take(MAX_LINES) {
            let Some((quote, _, _)) = collection::explicit_statement(line) else {
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
                    quote: quote.clone(),
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
        return Ok(json!({"status":"already_analyzed_or_changed"}));
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
    let enabled = grant.profile_client.as_deref().is_some_and(|reader| {
        !db.profile_delivery_disabled(&grant.client, root)
            .unwrap_or(true)
            && crate::onboarding::profile_access(root, reader).unwrap_or(false)
    });
    if !enabled {
        return;
    }
    if analyze(db, episode).is_err() {
        // Capture has already committed. Optional extraction cannot discard
        // the event or leak original text through a native hook diagnostic.
        eprintln!(
            "{}",
            json!({"local_analysis":"omitted","reason":"episode_or_store_unavailable"})
        );
    }
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
            id: "episode".into(),
            revision: "revision".into(),
            client: "codex".into(),
            project_root: "/project".into(),
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
