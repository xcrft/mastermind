//! How the author delivers changes — the process side of the profile, next to
//! the persona side in the code-shape and commit-voice rules. Everything here is
//! measured per commit from Git metadata: pull-request squash suffixes, tracker
//! keys in subjects, and which kinds of files a commit touches. Merge settings,
//! branch protection and CI may explain part of what these measure.

use super::range;
use super::stats::{bump, cget, gate, support, Confidence, Support};
use super::store::Counts;
use std::collections::{BTreeSet, HashMap};

/// Which kinds of files one commit touched, from `git log --numstat`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CommitShape {
    /// Added lines in files the code-shape detectors read.
    pub(super) source_added: usize,
    pub(super) files: usize,
    pub(super) added: usize,
    pub(super) source_files: usize,
    pub(super) test_files: usize,
    pub(super) doc_files: usize,
    /// Range: languages and component areas of non-generated files.
    pub(super) languages: BTreeSet<&'static str>,
    pub(super) areas: BTreeSet<String>,
}

/// `git log --numstat --format=%x1e%H` rows per commit. Binary files report
/// `-` for their line counts and only count as touched files; generated and
/// vendored files are not range.
pub(super) fn parse_numstat(
    raw: &str,
    is_source: impl Fn(&str) -> bool,
    is_generated: impl Fn(&str) -> bool,
) -> HashMap<String, CommitShape> {
    let mut shapes: HashMap<String, CommitShape> = HashMap::new();
    let mut current: Option<String> = None;
    for line in raw.lines() {
        if let Some(sha) = line.strip_prefix('\u{1e}') {
            let sha = sha.trim().to_string();
            shapes.insert(sha.clone(), CommitShape::default());
            current = Some(sha);
            continue;
        }
        let Some(shape) = current.as_ref().and_then(|sha| shapes.get_mut(sha)) else {
            continue;
        };
        let mut columns = line.splitn(3, '\t');
        let (Some(added), Some(_), Some(path)) = (columns.next(), columns.next(), columns.next())
        else {
            continue;
        };
        let added = added.parse::<usize>().unwrap_or(0);
        shape.files += 1;
        shape.added += added;
        if let (Some(language), false) = (range::language(path), is_generated(path)) {
            shape.languages.insert(language);
            shape.areas.insert(range::area(path));
        }
        if is_test_path(path) {
            shape.test_files += 1;
        } else if is_doc_path(path) {
            shape.doc_files += 1;
        } else if is_source(path) {
            shape.source_files += 1;
            shape.source_added += added;
        }
    }
    shapes
}

fn is_test_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = format!("/{}", path.to_ascii_lowercase());
    let lower_name = name.to_ascii_lowercase();
    ["/test/", "/tests/", "/__tests__/", "/spec/", "/testing/"]
        .iter()
        .any(|dir| lower.contains(dir))
        || lower_name.starts_with("test_")
        || ["_test.", ".test.", ".spec.", "_spec."]
            .iter()
            .any(|marker| lower_name.contains(marker))
        || ["Test.java", "Tests.java", "Test.cs", "Tests.cs", "Test.kt"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

fn is_doc_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.starts_with("docs/")
        || path.contains("/docs/")
        || [".md", ".mdx", ".rst", ".adoc"]
            .iter()
            .any(|extension| path.ends_with(extension))
}

/// Added source lines that declare a test inside a source file, which paths
/// cannot show (Rust `#[test]` modules, for example).
pub(super) fn declares_test(line: &str) -> bool {
    let line = line.trim_start();
    [
        "#[test]",
        "#[tokio::test",
        "#[cfg(test)]",
        "def test_",
        "func Test",
        "it(",
        "test(",
        "describe(",
        "[Fact]",
        "[Test]",
        "@Test",
    ]
    .iter()
    .any(|marker| line.starts_with(marker))
}

/// A squash-merged pull request subject ends with `(#123)`; GitHub and GitLab
/// append the number. The title before it is the author's.
pub(super) fn is_squash_merge(subject: &str) -> bool {
    pr_title(subject).is_some()
}

/// The subject without a trailing ` (#123)` pull-request suffix.
pub(super) fn pr_title(subject: &str) -> Option<&str> {
    let inner = subject.trim_end().strip_suffix(')')?;
    let open = inner.rfind("(#")?;
    let digits = &inner[open + 2..];
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| inner[..open].trim_end())
}

/// Uppercase prefixes of standards and algorithms that read like tracker keys.
const NOT_TRACKERS: [&str; 10] = [
    "UTF", "SHA", "ISO", "RFC", "HTTP", "TLS", "SSL", "AES", "RSA", "IPV",
];

/// The project part of the first tracker key (`POL` in `POL-1303 Refresh …`).
fn tracker_key(subject: &str) -> Option<&str> {
    subject
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find_map(|token| {
            let (project, number) = token.split_once('-')?;
            let valid = (2..=10).contains(&project.len())
                && project.starts_with(|c: char| c.is_ascii_uppercase())
                && project
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
                && !number.is_empty()
                && number.bytes().all(|byte| byte.is_ascii_digit())
                && !NOT_TRACKERS.contains(&project);
            valid.then_some(project)
        })
}

fn is_revert(subject: &str) -> bool {
    let subject = subject.trim_start();
    ["Revert ", "revert:", "revert(", "fixup!", "squash!"]
        .iter()
        .any(|prefix| subject.starts_with(prefix))
}

/// Process tallies of one commit. `shape` is absent for commits beyond the
/// bounded numstat listing; `inline_tests` covers tests declared inside source.
pub(super) fn tally(
    subject: &str,
    shape: Option<&CommitShape>,
    inline_tests: Option<bool>,
    c: &mut Counts,
) {
    bump(c, "workflow.commits", 1);
    if is_squash_merge(subject) {
        bump(c, "workflow.squash", 1);
    }
    if let Some(key) = tracker_key(subject) {
        bump(c, "workflow.ticket", 1);
        bump(c, &format!("workflow.ticket_key.{key}"), 1);
    }
    if is_revert(subject) {
        bump(c, "workflow.revert", 1);
    }
    let Some(shape) = shape else {
        return;
    };
    bump(c, "workflow.files", shape.files as i64);
    bump(c, "workflow.added", shape.added as i64);
    if shape.source_files > 0 {
        bump(c, "workflow.source", 1);
        // A missing diff cannot establish absence of inline test markers.
        if shape.test_files > 0 || inline_tests.is_some() {
            bump(c, "workflow.tests_checked", 1);
        }
        if shape.test_files > 0 || inline_tests == Some(true) {
            bump(c, "workflow.tests", 1);
        }
        if shape.doc_files > 0 {
            bump(c, "workflow.docs", 1);
        }
    }
}

/// The `## Workflow (process)` bullets, or nothing when no habit has evidence.
pub(super) fn render(c: &Counts, commits: &[&Counts]) -> String {
    let mut out = String::new();
    let every = |key: &'static str| {
        support(commits, move |k| {
            (cget(k, "workflow.commits") > 0).then(|| cget(k, key) > 0)
        })
    };
    let with_source = |key: &'static str, present: bool| {
        support(commits, move |k| {
            (cget(k, "workflow.source") > 0
                && (key != "workflow.tests" || cget(k, "workflow.tests_checked") > 0))
                .then(|| (cget(k, key) > 0) == present)
        })
    };

    let squash = every("workflow.squash");
    if squash.agree * 2 > squash.commits {
        push_rule(
            &mut out,
            "Observed pull-request suffixes consistent with squash merges",
            squash,
            "direct commits to the branch",
        );
    }

    let tested = with_source("workflow.tests", true);
    let source_commits = cget(c, "workflow.source");
    if source_commits > 0 {
        out.push_str(&format!("- Test-signal inspection covers {}/{} source commits; missing paths or added markers do not establish absence of tests.\n", tested.commits, source_commits));
    }
    let tests_together = tested.agree * 2 >= tested.commits;
    let tests = with_source("workflow.tests", tests_together);
    if tests_together {
        push_rule(
            &mut out,
            "Observed test paths or added test markers alongside source changes",
            tests,
            "source-only commits",
        );
    } else {
        push_rule(
            &mut out,
            "No test paths or added test markers observed in inspected source changes",
            tests,
            "detected test paths or added markers (existing inline tests may be missed)",
        );
    }

    let docs = with_source("workflow.docs", true);
    if docs.agree * 2 > docs.commits {
        push_rule(
            &mut out,
            "Updates documentation in the same commit as source code",
            docs,
            "docs left for later",
        );
    }

    let tickets = every("workflow.ticket");
    let keys = tracker_keys(c);
    if tickets.agree * 2 > tickets.commits && gate(tickets).is_some() {
        push_rule(
            &mut out,
            &format!("References a tracker key in commit subjects ({keys})"),
            tickets,
            "subjects without a ticket",
        );
    } else if tickets.agree > 0 {
        out.push_str(&format!(
            "- Tracker keys ({keys}) in the subjects of {}.\n",
            tickets.label()
        ));
    }

    let mut files: Vec<i64> = Vec::new();
    let mut added: Vec<i64> = Vec::new();
    for commit in commits {
        if cget(commit, "workflow.files") > 0 && cget(commit, "diff.bulk") == 0 {
            files.push(cget(commit, "workflow.files"));
            added.push(cget(commit, "workflow.added"));
        }
    }
    if !files.is_empty() {
        out.push_str(&format!(
            "- Typical change: median {} file(s) and {} added line(s); 75th percentile {} \
             file(s) and {} line(s) across {} non-bulk commits.\n",
            percentile(&mut files, 50),
            percentile(&mut added, 50),
            percentile(&mut files, 75),
            percentile(&mut added, 75),
            files.len()
        ));
    }

    let reverts = every("workflow.revert");
    if reverts.agree > 0 {
        out.push_str(&format!(
            "- Reverts or fixups: {} of {} commits.\n",
            reverts.agree, reverts.commits
        ));
    }
    out
}

fn push_rule(out: &mut String, statement: &str, support: Support, counter: &str) {
    if let Some(confidence) = gate(support) {
        out.push_str(&format!(
            "- **{statement}.** {}. _Alternative pattern: {counter}._ (support tier: {})\n",
            support.label(),
            Confidence::label(confidence)
        ));
    }
}

/// The three most frequent tracker projects, most frequent first.
fn tracker_keys(c: &Counts) -> String {
    let mut keys: Vec<(i64, &str)> = c
        .iter()
        .filter_map(|(key, value)| Some((*value, key.strip_prefix("workflow.ticket_key.")?)))
        .collect();
    keys.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    keys.iter()
        .take(3)
        .map(|(_, key)| *key)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Nearest-rank percentile of `values`, which it sorts.
fn percentile(values: &mut [i64], rank: usize) -> i64 {
    values.sort_unstable();
    let index = (values.len() * rank).div_ceil(100).max(1) - 1;
    values[index.min(values.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_yield_pr_titles_tracker_keys_and_reverts() {
        assert_eq!(
            pr_title("POL-1303 Refresh bundle (#812)"),
            Some("POL-1303 Refresh bundle")
        );
        assert_eq!(pr_title("note (#12) mid-subject"), None);
        assert_eq!(pr_title("fix: handle (#) empty"), None);
        assert_eq!(
            tracker_key("POL-1288: prepare benchmark (#787)"),
            Some("POL")
        );
        assert_eq!(tracker_key("SEC-000: remediate Snyk findings"), Some("SEC"));
        assert_eq!(tracker_key("Update to 1.3.540914 (#337)"), None);
        assert_eq!(tracker_key("utf-8 decoding"), None);
        assert_eq!(tracker_key("Hash with SHA-256 and UTF-8"), None);
        assert!(is_test_path("src/FooTest.java"));
        assert!(!is_test_path("src/latest.java"));
        assert!(is_revert("Revert \"Add cache\""));
        assert!(is_revert("fixup! tighten guard"));
        assert!(!is_revert("Reverting a decision is documented"));
    }

    #[test]
    fn numstat_separates_source_tests_and_docs() {
        let raw = "\u{1e}aaa\n\n10\t2\tsrc/a.rs\n4\t0\ttests/a_test.rs\n3\t1\tdocs/guide.md\n\
                   -\t-\tlogo.png\n\u{1e}bbb\n\n7\t0\tweb/app.spec.ts\n";
        let shapes = parse_numstat(
            raw,
            |path| path.ends_with(".rs") || path.ends_with(".ts"),
            |path| path.starts_with("dist/"),
        );
        assert_eq!(
            shapes["aaa"],
            CommitShape {
                source_added: 10,
                files: 4,
                added: 17,
                source_files: 1,
                test_files: 1,
                doc_files: 1,
                languages: BTreeSet::from(["Rust"]),
                areas: BTreeSet::from(["src".to_string(), "tests".to_string()]),
            }
        );
        assert_eq!(shapes["bbb"].test_files, 1);
        assert_eq!(shapes["bbb"].source_files, 0);
    }

    #[test]
    fn unread_diffs_are_unknown_instead_of_negative_test_evidence() {
        let shape = CommitShape {
            source_files: 1,
            ..CommitShape::default()
        };
        let commits: Vec<_> = (0..2000)
            .map(|i| {
                let mut c = Counts::new();
                tally("change", Some(&shape), (i < 400).then_some(true), &mut c);
                c
            })
            .collect();
        let mut total = Counts::new();
        for c in &commits {
            for (key, value) in c {
                bump(&mut total, key, *value);
            }
        }
        let rendered = render(&total, &commits.iter().collect::<Vec<_>>());
        assert!(rendered.contains("inspection covers 400/2000"));
        assert!(rendered.contains("alongside source changes.** 400/400"));
        assert!(!rendered.contains("**No test paths"));
    }

    #[test]
    fn workflow_rules_need_commit_level_agreement() {
        let shape = CommitShape {
            source_added: 20,
            files: 3,
            added: 25,
            source_files: 2,
            test_files: 1,
            doc_files: 0,
            ..CommitShape::default()
        };
        let per_commit: Vec<Counts> = (0..12)
            .map(|i| {
                let mut c = Counts::new();
                tally(
                    &format!("POL-{i} Add thing (#{i})"),
                    Some(&shape),
                    Some(false),
                    &mut c,
                );
                c
            })
            .collect();
        let mut total = Counts::new();
        for commit in &per_commit {
            for (key, value) in commit {
                bump(&mut total, key, *value);
            }
        }
        let refs: Vec<&Counts> = per_commit.iter().collect();
        let rendered = render(&total, &refs);
        assert!(rendered.contains(
            "**Observed pull-request suffixes consistent with squash merges.** 12/12 commits"
        ));
        assert!(rendered.contains(
            "**Observed test paths or added test markers alongside source changes.** 12/12"
        ));
        assert!(rendered.contains("tracker key in commit subjects (POL)"));
        assert!(rendered.contains("median 3 file(s) and 25 added line(s)"));
        assert!(!rendered.contains("documentation"));

        let few: Vec<&Counts> = refs.iter().take(3).copied().collect();
        let rendered = render(&total, &few);
        assert!(
            !rendered.contains("**Observed pull-request suffixes"),
            "three commits are not a habit"
        );
        assert!(rendered.contains("Tracker keys (POL) in the subjects of 3/3 commits."));
    }
}
