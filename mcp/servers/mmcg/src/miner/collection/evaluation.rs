//! Synthetic regression evaluation, deliberately separate from human accuracy.

use super::{detect_version, HumanTurn};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Corpus {
    provenance: String,
    items: Vec<Example>,
}

#[derive(Deserialize)]
struct Example {
    id: String,
    language: String,
    category: String,
    expected_kind: Option<String>,
    known_miss: bool,
    text: String,
}

#[derive(Default, serde::Serialize)]
struct Metrics {
    signals: usize,
    negatives: usize,
    true_positive: usize,
    false_positive: usize,
    false_negative: usize,
    wrong_kind: usize,
}

impl Metrics {
    fn count(&mut self, expected: Option<&str>, actual: Option<&str>) {
        match (expected, actual) {
            (Some(expected), Some(actual)) => {
                self.signals += 1;
                self.true_positive += 1;
                self.wrong_kind += usize::from(expected != actual);
            }
            (Some(_), None) => {
                self.signals += 1;
                self.false_negative += 1;
            }
            (None, Some(_)) => {
                self.negatives += 1;
                self.false_positive += 1;
            }
            (None, None) => self.negatives += 1,
        }
    }
}

#[test]
fn synthetic_multilingual_candidate_detection_evaluation() {
    let corpus: Corpus = serde_json::from_str(include_str!(
        "../../../tests/fixtures/persona_detector.json"
    ))
    .unwrap();
    let mut baseline = Metrics::default();
    let mut current = Metrics::default();
    let mut slices = BTreeMap::<String, Metrics>::new();
    let mut misses = Vec::new();
    let expected_misses: Vec<_> = corpus
        .items
        .iter()
        .filter(|example| example.known_miss)
        .map(|example| &example.id)
        .collect();
    for example in &corpus.items {
        let turn = HumanTurn {
            at: "2026-09-26".into(),
            text: example.text.clone(),
            line_no: 3,
            record_digest: "a".repeat(64),
            cwd: None,
            session_id: Some("synthetic".into()),
        };
        let old = detect_version(&turn, false);
        let new = detect_version(&turn, true);
        baseline.count(
            example.expected_kind.as_deref(),
            old.as_ref().map(|(_, kind, _)| *kind),
        );
        let actual = new.as_ref().map(|(_, kind, _)| *kind);
        current.count(example.expected_kind.as_deref(), actual);
        for slice in [
            format!("language:{}", example.language),
            format!("category:{}", example.category),
        ] {
            slices
                .entry(slice)
                .or_default()
                .count(example.expected_kind.as_deref(), actual);
        }
        if let Some((quote, _, _)) = new {
            assert_eq!(
                quote,
                example
                    .text
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                "{} lost condition/negation",
                example.id
            );
        }
        if example.expected_kind.is_some() && actual.is_none() {
            misses.push(&example.id);
        }
    }
    eprintln!(
        "{}",
        json!({"provenance": corpus.provenance, "baseline_v1": baseline, "current_v2": current, "slices": slices, "known_misses": misses})
    );
    assert!(current.true_positive > baseline.true_positive);
    assert_eq!(
        current.false_positive, 0,
        "negative controls must remain out of the inbox"
    );
    assert_eq!(current.wrong_kind, 0);
    assert_eq!(
        misses, expected_misses,
        "regressed supported cue or changed a declared limitation; review the labeled case"
    );
}
