//! Offline, source-bound evaluation. Labels and outcomes are supplied by the
//! reviewer; this command neither manufactures independent labels nor runs tasks.

use super::{influence::Influence, local, semantic, Error};
use crate::bounded_fs::{self, ReadControl};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    schema: u32,
    provenance: String,
    labeler: String,
    cases: Vec<Case>,
    task_pairs: Vec<Pair>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    session: String,
    partition: String,
    text: String,
    expected_quotes: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    task_revision: String,
    baseline: Trial,
    with_profile: Trial,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Trial {
    session: String,
    profile_digest: Option<String>,
    correct: bool,
    iterations: u32,
    user_corrections: u32,
    wall_ms: u64,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_:.".contains(&byte))
}

fn evaluate(input: Input) -> Result<Value, Error> {
    if input.schema != 1
        || !matches!(input.provenance.as_str(), "synthetic" | "selected_history")
        || !identifier(&input.labeler)
        || input.cases.len() > 128
        || input.task_pairs.len() > 128
        || input.cases.is_empty() && input.task_pairs.is_empty()
    {
        return Err("invalid or empty local evaluation corpus".into());
    }
    let mut ids = HashSet::new();
    let mut sessions = BTreeMap::new();
    let (mut tp, mut fp, mut fn_count, mut exclusions) = (0usize, 0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    for case in &input.cases {
        if !identifier(&case.id)
            || !identifier(&case.session)
            || !ids.insert(&case.id)
            || !matches!(case.partition.as_str(), "development" | "held_out")
            || case.text.len() > 16 * 1024
            || case.expected_quotes.len() > 8
        {
            return Err("evaluation case identity, partition or bound is invalid".into());
        }
        if sessions
            .insert(&case.session, &case.partition)
            .is_some_and(|old| old != &case.partition)
        {
            return Err(
                "evaluation session occurs in both development and held-out partitions".into(),
            );
        }
        let expected: HashSet<_> = case.expected_quotes.iter().cloned().collect();
        if expected.len() != case.expected_quotes.len()
            || expected.iter().any(|quote| {
                !(8..=16 * 1024).contains(&quote.chars().count())
                    || !semantic::quote_is_user_prose(&case.text, quote)
            })
        {
            return Err("evaluation label is duplicated or absent from exact user prose".into());
        }
        let source = semantic::EpisodeInput {
            model: None,
            model_binding: None,
            id: case.id.clone(),
            revision: "offline-evaluation".into(),
            client: "codex".into(),
            project_root: std::env::temp_dir()
                .join("mastermind-evaluation")
                .to_string_lossy()
                .into_owned(),
            project: "offline".into(),
            coverage_gaps: vec![],
            profile_influenced: false,
            events: vec![semantic::EventInput {
                id: "source".into(),
                kind: "UserPromptSubmit".into(),
                actor: "user".into(),
                origin: "user_channel_unverified".into(),
                text: case.text.clone(),
                influence: Influence::fresh(),
            }],
        };
        let actual: HashSet<_> = match local::extract(&source) {
            Ok(drafts) => drafts
                .into_iter()
                .map(|draft| draft.supports[0].quote.clone())
                .collect(),
            Err(_) => {
                exclusions += 1;
                HashSet::new()
            }
        };
        let hit = expected.intersection(&actual).count();
        tp += hit;
        fp += actual.len() - hit;
        fn_count += expected.len() - hit;
        if actual != expected {
            failures.push(&case.id);
        }
    }
    let mut trial_sessions = HashSet::new();
    let mut revisions = HashSet::new();
    let (mut correctness, mut iterations, mut corrections, mut wall) = (0i64, 0i64, 0i64, 0i64);
    for pair in &input.task_pairs {
        if !crate::miner::collection::valid_id(&pair.task_revision)
            || !revisions.insert(&pair.task_revision)
            || pair.baseline.profile_digest.is_some()
            || !pair
                .with_profile
                .profile_digest
                .as_deref()
                .is_some_and(crate::miner::collection::valid_id)
        {
            return Err(
                "task pair must bind one task revision and opposite profile conditions".into(),
            );
        }
        for trial in [&pair.baseline, &pair.with_profile] {
            if !identifier(&trial.session)
                || !trial_sessions.insert(&trial.session)
                || !(1..=20).contains(&trial.iterations)
                || trial.user_corrections > 128
                || !(1..=7_200_000).contains(&trial.wall_ms)
            {
                return Err("task trial has a repeated session or invalid observations".into());
            }
        }
        correctness += i64::from(pair.with_profile.correct) - i64::from(pair.baseline.correct);
        iterations += i64::from(pair.with_profile.iterations) - i64::from(pair.baseline.iterations);
        corrections += i64::from(pair.with_profile.user_corrections)
            - i64::from(pair.baseline.user_corrections);
        wall += pair.with_profile.wall_ms as i64 - pair.baseline.wall_ms as i64;
    }
    let ratio = |a: usize, b: usize| (b > 0).then(|| a as f64 / b as f64);
    let mean = |value: i64| {
        (!input.task_pairs.is_empty()).then(|| value as f64 / input.task_pairs.len() as f64)
    };
    Ok(
        json!({"schema":1,"processor":local::processor(),"provenance":input.provenance,
        "labels":{"labeler":input.labeler,"independence":"not_verified"},
        "mining":{"cases":input.cases.len(),"sessions":sessions.len(),
            "held_out_cases":input.cases.iter().filter(|case| case.partition=="held_out").count(),
            "true_positive":tp,"false_positive":fp,"false_negative":fn_count,"excluded_inputs":exclusions,
            "precision":ratio(tp,tp+fp),"recall":ratio(tp,tp+fn_count),"failed_case_ids":failures,
            "meaning":"Exact complete source-span agreement with supplied labels, not proof of authorship or habit truth."},
        "task_benefit":{"status":if input.task_pairs.is_empty(){"unmeasured"}else{"paired_observations"},
            "pairs":input.task_pairs.len(),"profile_minus_baseline":{"correctness":mean(correctness),
                "iterations":mean(iterations),"user_corrections":mean(corrections),"wall_ms":mean(wall)},
            "causal_effect":"not_established","meaning":"Supplied paired outcomes. Runtime conditions and reviewer independence are not verified."},
        "publication":false,"model":false}),
    )
}

pub(super) fn run(path: &Path) -> Result<Value, Error> {
    let (root, path) = bounded_fs::open_file_target(path)?;
    let file = bounded_fs::read_regular_file_with_capability(
        &root,
        &path,
        2 * 1024 * 1024,
        2 * 1024 * 1024,
        ReadControl::default(),
    )?;
    let input: Input = serde_json::from_value(crate::setup::parse_json_unique(&file.bytes)?)?;
    let mut result = evaluate(input)?;
    result["corpus_sha256"] = json!(crate::hex::encode(&Sha256::digest(&file.bytes)));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Value {
        json!({"schema":1,"provenance":"synthetic","labeler":"fixture-author",
            "cases":[{"id":"scope","session":"one","partition":"held_out", "text":"I prefer short code reviews\nonly for trivial changes.","expected_quotes":["I prefer short code reviews\nonly for trivial changes."]},
                {"id":"control","session":"two","partition":"held_out","text":"Review this change.","expected_quotes":[]}],"task_pairs":[]})
    }

    #[test]
    fn evaluates_source_scope_and_keeps_empty_benefit_unknown() {
        let result = evaluate(serde_json::from_value(corpus()).unwrap()).unwrap();
        assert_eq!(result["mining"]["true_positive"], 1);
        assert_eq!(result["mining"]["false_positive"], 0);
        assert_eq!(result["mining"]["recall"], 1.0);
        assert_eq!(result["task_benefit"]["status"], "unmeasured");
        assert!(result["task_benefit"]["profile_minus_baseline"]["correctness"].is_null());
    }

    #[test]
    fn rejects_leaked_sessions_and_invented_source_labels() {
        let mut value = corpus();
        value["cases"][1]["session"] = json!("one");
        value["cases"][1]["partition"] = json!("development");
        assert!(evaluate(serde_json::from_value(value).unwrap()).is_err());
        let mut value = corpus();
        value["cases"][0]["expected_quotes"] = json!(["I prefer invented evidence."]);
        assert!(evaluate(serde_json::from_value(value).unwrap()).is_err());
    }

    #[test]
    fn pairs_preserve_direction_and_reject_reused_sessions() {
        let mut value = corpus();
        value["task_pairs"] = json!([{"task_revision":"a".repeat(64),
            "baseline":{"session":"before","profile_digest":null,"correct":false,"iterations":3,"user_corrections":2,"wall_ms":3000},
            "with_profile":{"session":"after","profile_digest":"b".repeat(64),"correct":true,"iterations":1,"user_corrections":0,"wall_ms":1000}}]);
        let result = evaluate(serde_json::from_value(value.clone()).unwrap()).unwrap();
        assert_eq!(
            result["task_benefit"]["profile_minus_baseline"]["iterations"],
            -2.0
        );
        assert_eq!(
            result["task_benefit"]["profile_minus_baseline"]["correctness"],
            1.0
        );
        value["task_pairs"][0]["with_profile"]["session"] = json!("before");
        assert!(evaluate(serde_json::from_value(value).unwrap()).is_err());
    }
}
