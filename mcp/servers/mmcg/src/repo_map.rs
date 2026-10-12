//! Offline navigation prototype. Source/index bindings are checked by its caller.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};

pub const MAX_SYMBOLS: usize = 20000;

#[derive(Clone, Deserialize)]
pub struct Symbol {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub signature: Option<String>,
}

#[derive(Deserialize)]
pub struct Input {
    pub question: String,
    pub source_sha256: String,
    pub budget_bytes: usize,
    pub symbols: Vec<Symbol>,
}

#[derive(Serialize)]
pub struct Output {
    pub schema_version: u32,
    pub algorithm: &'static str,
    pub source_sha256: String,
    pub budget_bytes: usize,
    pub payload_bytes: usize,
    pub selected: usize,
    pub candidates: usize,
    pub relevant_candidates: usize,
    pub truncated: bool,
    pub payload: String,
}

fn words(text: &str) -> BTreeSet<String> {
    let mut expanded = String::new();
    let mut previous_lower = false;
    for c in text.chars() {
        if previous_lower && c.is_uppercase() {
            expanded.push(' ');
        }
        expanded.push(c);
        previous_lower = c.is_lowercase();
    }
    expanded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 4)
        .map(str::to_lowercase)
        .map(|word| {
            if word.len() > 6 && word.ends_with("ing") {
                word[..word.len() - 3].to_string()
            } else if word.len() > 5 && word.ends_with("ed") {
                word[..word.len() - 2].to_string()
            } else if word.len() > 4 && word.ends_with('s') && !word.ends_with("ss") {
                word[..word.len() - 1].to_string()
            } else {
                word
            }
        })
        .collect()
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn file_terms(file: &str) -> BTreeSet<String> {
    words(file.rsplit('/').next().unwrap_or(file))
}

fn location_order(a: &Symbol, b: &Symbol) -> std::cmp::Ordering {
    a.file
        .cmp(&b.file)
        .then(a.line.cmp(&b.line))
        .then(a.name.cmp(&b.name))
        .then(a.kind.cmp(&b.kind))
        .then(a.signature.cmp(&b.signature))
}

pub fn build(input: Input) -> Result<Output, String> {
    if !(512..=16384).contains(&input.budget_bytes)
        || input.symbols.len() > MAX_SYMBOLS
        || input.question.len() > 16384
        || input.source_sha256.len() != 64
        || !input.source_sha256.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err("invalid input bounds or source digest".into());
    }
    let terms = words(&input.question);
    let focus = words(input.question.split(['.', '?', '!']).next().unwrap_or(""));
    let mut rows = input.symbols;
    for row in &rows {
        if row.file.is_empty()
            || row.line == 0
            || row.file.starts_with('/')
            || row.file.split('/').any(|p| p == ".." || p.starts_with('.'))
            || row.file.contains('\\')
            || row.file.chars().any(char::is_control)
        {
            return Err("invalid indexed source location".into());
        }
    }
    rows.sort_by(location_order);
    rows.dedup_by(|a, b| {
        a.file == b.file
            && a.line == b.line
            && a.name == b.name
            && a.kind == b.kind
            && a.signature == b.signature
    });
    let candidates = rows.len();
    // Count files rather than definitions so large helper modules do not dominate.
    let mut vocabulary: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for row in &rows {
        if row.kind != "module"
            && row.kind != "impl"
            && !row.signature.as_deref().unwrap_or("").contains("#[test]")
        {
            let entry = vocabulary
                .entry(&row.file)
                .or_insert_with(|| file_terms(&row.file));
            entry.extend(words(&row.name));
        }
    }
    let term_weight = |term: &String| {
        let frequency = vocabulary.values().filter(|v| v.contains(term)).count();
        (1024 / (1 + frequency)) * if focus.contains(term) { 3 } else { 1 }
    };
    let score = |row: &Symbol| {
        let name = words(&row.name);
        let file = file_terms(&row.file);
        let hits: BTreeSet<_> = terms
            .intersection(&name)
            .chain(focus.intersection(&file))
            .collect();
        let weight = terms.intersection(&name).map(&term_weight).sum::<usize>()
            + focus.intersection(&file).map(&term_weight).sum::<usize>() * 3;
        // Several task terms together are stronger navigation hints than one rare adjective.
        weight * hits.len() * hits.len()
    };
    let mut files: BTreeMap<&str, Vec<&Symbol>> = BTreeMap::new();
    for row in &rows {
        if row.kind != "module"
            && row.kind != "impl"
            && !row.signature.as_deref().unwrap_or("").contains("#[test]")
            && score(row) > 0
        {
            files.entry(&row.file).or_default().push(row);
        }
    }
    let relevant_candidates = files.values().map(Vec::len).sum();
    for symbols in files.values_mut() {
        symbols.sort_by(|a, b| score(b).cmp(&score(a)).then(location_order(a, b)));
        let best = score(symbols[0]);
        symbols.retain(|row| score(row) * 3 >= best);
    }
    let mut files: Vec<_> = files.into_iter().collect();
    files.sort_by(|a, b| score(b.1[0]).cmp(&score(a.1[0])).then(a.0.cmp(b.0)));
    if let Some((_, symbols)) = files.first() {
        let best = score(symbols[0]);
        files.retain(|(_, symbols)| score(symbols[0]) * 2 >= best);
    }
    let mut payload = format!(
        "Repository map: partial definition locations from frozen source {digest}.\n\
         Read relevant locations directly, then follow calls or search for missing locations.\n\
         Track ranges already read; extend with uncovered gaps. Read source to support claims.\n\
         This shortlist is navigation metadata, not behavioral evidence or complete coverage.\n",
        digest = input.source_sha256
    );
    let mut selected = 0;
    // A shortlist leaves room for source evidence instead of filling the whole budget.
    for (file, symbols) in files.into_iter().take(3) {
        let mut section = format!("{file}\n");
        let mut section_selected = 0;
        for row in symbols.into_iter().take(3) {
            let line = format!(
                "  :{} {} {}\n",
                row.line,
                single_line(&row.kind),
                single_line(&row.name)
            );
            if payload.len() + section.len() + line.len() <= input.budget_bytes {
                section.push_str(&line);
                section_selected += 1;
            }
        }
        if section_selected > 0 {
            payload.push_str(&section);
            selected += section_selected;
        }
    }
    Ok(Output {
        schema_version: 1,
        algorithm: "focused_shortlist_v3",
        source_sha256: input.source_sha256,
        budget_bytes: input.budget_bytes,
        payload_bytes: payload.len(),
        selected,
        candidates,
        relevant_candidates,
        truncated: selected < candidates,
        payload,
    })
}

pub fn from_reader(reader: impl Read) -> Result<Output, Box<dyn std::error::Error>> {
    let mut body = String::new();
    reader
        .take(16 * 1024 * 1024 + 1)
        .read_to_string(&mut body)?;
    if body.len() > 16 * 1024 * 1024 {
        return Err("input exceeds byte limit".into());
    }
    let result = build(serde_json::from_str(&body)?).map_err(io::Error::other)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(symbols: Vec<Symbol>, budget_bytes: usize) -> Input {
        Input {
            question: "How are cache receipts checked?".into(),
            source_sha256: "a".repeat(64),
            budget_bytes,
            symbols,
        }
    }

    #[test]
    fn selection_is_query_relevant_stable_and_charged_in_utf8_bytes() {
        let symbols = vec![
            Symbol {
                file: "src/a.rs".into(),
                name: "unrelated".into(),
                kind: "fn".into(),
                line: 2,
                signature: None,
            },
            Symbol {
                file: "src/z.rs".into(),
                name: "cache_receipts_ключ".into(),
                kind: "fn".into(),
                line: 3,
                signature: Some("fn cache_receipts(ключ: String)".into()),
            },
        ];
        let result = build(input(symbols.clone(), 512)).unwrap();
        let reversed = build(input(symbols.into_iter().rev().collect(), 512)).unwrap();
        assert_eq!(result.payload, reversed.payload);
        assert!(result.payload.contains("src/z.rs"));
        assert!(!result.payload.contains("src/a.rs"));
        assert_eq!(result.selected, 1);
        assert_eq!(result.payload_bytes, result.payload.len());
        assert!(result.payload_bytes > result.payload.chars().count());
        assert!(result.payload_bytes <= 512);
        assert!(build(input(vec![], 511)).is_err());
    }

    #[test]
    fn omitted_symbols_and_invalid_locations_are_not_silent() {
        let row = Symbol {
            file: "src/a.rs".into(),
            name: "cache".repeat(200),
            kind: "fn".into(),
            line: 1,
            signature: None,
        };
        let result = build(input(vec![row.clone()], 512)).unwrap();
        assert_eq!(result.selected, 0);
        assert!(result.truncated);
        let invalid = Symbol {
            file: "../secret.rs".into(),
            ..row
        };
        assert!(build(input(vec![invalid], 512)).is_err());
    }

    #[test]
    fn task_subject_beats_generic_signature_hints_and_does_not_fill_spare_budget() {
        let symbols = vec![
            Symbol {
                file: "src/delivery_cache.rs".into(),
                name: "deliver".into(),
                kind: "method".into(),
                line: 123,
                signature: None,
            },
            Symbol {
                file: "src/helpers.rs".into(),
                name: "read_generic".into(),
                kind: "function".into(),
                line: 30,
                signature: Some("fn read_generic(cache: Cache, receipts: Receipt)".into()),
            },
            Symbol {
                file: "src/tests.rs".into(),
                name: "cache_receipts".into(),
                kind: "function".into(),
                line: 40,
                signature: Some("#[test] fn cache_receipts()".into()),
            },
        ];
        let result = build(input(symbols, 16000)).unwrap();
        assert!(result
            .payload
            .contains("src/delivery_cache.rs\n  :123 method deliver"));
        assert!(!result.payload.contains("helpers.rs"));
        assert!(!result.payload.contains("tests.rs"));
        assert_eq!(result.selected, 1);
        assert!(result.truncated);
    }
}
