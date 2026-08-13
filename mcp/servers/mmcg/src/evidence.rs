//! Read-only evidence overlays for Mastermind Lens.
//!
//! Evidence is deliberately ephemeral: reports and Git history are parsed into
//! the current Lens response and are never written to the codegraph database or
//! repository. Every source is bounded and failures remain visible as partial
//! diagnostics instead of being mistaken for an absence of evidence.

mod junit;
mod otel;

use crate::diff::{run_bounded_git_with_limit_until, WorkingTreeDiffError};
use crate::queries::ChangeImpactResponse;
use aho_corasick::{AhoCorasickBuilder, MatchKind};
use globset::{GlobBuilder, GlobMatcher};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Instant, SystemTime};

pub(crate) const MAX_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_CODEOWNERS_BYTES: u64 = 3 * 1024 * 1024;
pub(crate) const MAX_ARTIFACT_SOURCES: usize = 64;
const MAX_RELEVANT_FILES: usize = 1_000;
const MAX_FINDINGS: usize = 5_000;
const MAX_FINDINGS_PER_FILE: usize = 100;
const MAX_COVERAGE_LINES: usize = 500_000;
const MAX_TEST_CASES: usize = 100_000;
const MAX_TEST_FAILURES: usize = 1_000;
const MAX_TEST_FAILURES_PER_FILE: usize = 50;
const MAX_RUNTIME_SPANS: usize = 100_000;
const MAX_RUNTIME_EDGES: usize = 1_000;
const MAX_RUNTIME_NAMES_PER_EDGE: usize = 5;
const MAX_KNOWLEDGE_MATCHES: usize = 500;
const MAX_KNOWLEDGE_PER_FILE: usize = 20;
const MAX_KNOWLEDGE_ARTIFACTS: usize = 5_000;
const MAX_KNOWLEDGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_SOURCE_FACTS: usize = 1_000_000;
const MAX_CODEOWNER_RULES: usize = 50_000;
const MAX_OWNERS_PER_RULE: usize = 50;
const MAX_CONTRIBUTORS_PER_FILE: usize = 5;
const MAX_DIAGNOSTICS: usize = 100;
const GIT_HISTORY_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct EvidenceOptions {
    pub sarif: Vec<PathBuf>,
    pub coverage: Vec<PathBuf>,
    pub codeowners: Option<PathBuf>,
    pub discover_codeowners: bool,
    pub git_commits: u16,
}

#[derive(Debug, Clone, Default)]
pub struct EvidenceExtensionOptions {
    pub junit: Vec<PathBuf>,
    pub otel: Vec<PathBuf>,
    pub project_knowledge: bool,
}

#[derive(Debug, Serialize)]
pub struct EvidenceSnapshot {
    pub schema_version: u32,
    pub partial: bool,
    pub sources: EvidenceCollection<EvidenceSource>,
    pub files: EvidenceCollection<FileEvidence>,
    pub runtime_edges: EvidenceCollection<RuntimeEdgeEvidence>,
    pub fact_artifacts: EvidenceCollection<crate::facts::FactArtifact>,
    pub fact_relationships: EvidenceCollection<crate::facts::FactRelationship>,
    pub diagnostics: EvidenceCollection<EvidenceDiagnostic>,
    pub precision_notes: Vec<EvidencePrecisionNote>,
    pub limits: EvidenceLimits,
}

#[derive(Debug, Serialize)]
pub struct EvidenceCollection<T> {
    pub total: Option<u32>,
    pub returned: u32,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<&'static str>,
    pub items: Vec<T>,
}

#[derive(Debug, Serialize)]
pub struct EvidenceSource {
    pub id: String,
    pub kind: &'static str,
    pub label: String,
    pub status: &'static str,
    pub facts_total: Option<u32>,
    pub facts_returned: u32,
    pub files_matched: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct FileEvidence {
    pub path: String,
    pub findings: Vec<EvidenceFinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<CoverageEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ownership: Option<OwnershipEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub churn: Option<ChurnEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_results: Option<TestResultsEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeFileEvidence>,
    pub knowledge: Vec<KnowledgeEvidence>,
}

#[derive(Debug, Serialize)]
pub struct KnowledgeEvidence {
    pub source_id: String,
    pub artifact_path: String,
    pub kind: String,
    pub title: String,
    pub match_kind: &'static str,
    pub excerpt: String,
}

#[derive(Debug, Serialize)]
pub struct RuntimeFileEvidence {
    pub source_ids: Vec<String>,
    pub spans: u32,
    pub traces: u32,
}

#[derive(Debug, Serialize)]
pub struct RuntimeEdgeEvidence {
    pub source_ids: Vec<String>,
    pub parent_file: String,
    pub child_file: String,
    pub spans: u32,
    pub traces: u32,
    pub span_names: Vec<String>,
    pub names_truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct TestResultsEvidence {
    pub source_ids: Vec<String>,
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub errors: u32,
    pub skipped: u32,
    pub duration_ms: u64,
    pub failures: Vec<TestFailureEvidence>,
    pub failures_truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct TestFailureEvidence {
    pub source_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct EvidenceFinding {
    pub source_id: String,
    pub tool: String,
    pub rule_id: String,
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct CoverageEvidence {
    pub source_ids: Vec<String>,
    pub lines_found: u32,
    pub lines_hit: u32,
}

#[derive(Debug, Serialize)]
pub struct OwnershipEvidence {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codeowners_source_id: Option<String>,
    pub codeowners: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contributors_source_id: Option<String>,
    pub contributors: Vec<ContributorEvidence>,
    pub contributors_truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct ContributorEvidence {
    pub name: String,
    pub commits: u32,
}

#[derive(Debug, Serialize)]
pub struct ChurnEvidence {
    pub source_id: String,
    pub commits: u32,
    pub lines_added: u64,
    pub lines_deleted: u64,
}

#[derive(Debug, Serialize)]
pub struct EvidenceDiagnostic {
    pub source_id: String,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct EvidencePrecisionNote {
    pub source_id: &'static str,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct EvidenceLimits {
    pub artifact_bytes: u64,
    pub artifact_sources: u32,
    pub relevant_files: u32,
    pub findings: u32,
    pub findings_per_file: u32,
    pub coverage_lines: u32,
    pub test_cases: u32,
    pub test_failures: u32,
    pub test_failures_per_file: u32,
    pub runtime_spans: u32,
    pub runtime_edges: u32,
    pub runtime_names_per_edge: u32,
    pub normalized_fact_sources: u32,
    pub normalized_fact_artifacts: u32,
    pub normalized_fact_relationships: u32,
    pub knowledge_matches: u32,
    pub knowledge_per_file: u32,
    pub knowledge_artifacts: u32,
    pub knowledge_bytes: u64,
    pub codeowner_rules: u32,
    pub codeowners_bytes: u64,
    pub owners_per_rule: u32,
    pub contributors_per_file: u32,
    pub diagnostics: u32,
    pub git_commits: u16,
}

#[derive(Default)]
struct FileAccumulator {
    findings: Vec<EvidenceFinding>,
    coverage_lines: BTreeMap<u32, u64>,
    coverage_sources: BTreeSet<String>,
    codeowners: Vec<String>,
    codeowners_source_id: Option<String>,
    churn: ChurnAccumulator,
    tests: TestAccumulator,
    runtime: RuntimeAccumulator,
    knowledge: Vec<KnowledgeEvidence>,
}

#[derive(Default)]
struct RuntimeAccumulator {
    source_ids: BTreeSet<String>,
    spans: u32,
    traces: BTreeSet<String>,
}

#[derive(Default)]
struct RuntimeEdgeAccumulator {
    source_ids: BTreeSet<String>,
    spans: u32,
    traces: BTreeSet<String>,
    span_names: BTreeSet<String>,
    names_truncated: bool,
}

#[derive(Default)]
struct TestAccumulator {
    source_ids: BTreeSet<String>,
    total: u32,
    passed: u32,
    failed: u32,
    errors: u32,
    skipped: u32,
    duration_ms: u64,
    failures: Vec<TestFailureEvidence>,
    failures_truncated: bool,
}

#[derive(Default)]
struct ChurnAccumulator {
    commits: u32,
    lines_added: u64,
    lines_deleted: u64,
    contributors: BTreeMap<String, u32>,
    contributors_truncated: bool,
    source_id: Option<String>,
}

struct Collector<'a> {
    root: &'a Path,
    relevant: BTreeSet<String>,
    sources: Vec<EvidenceSource>,
    files: BTreeMap<String, FileAccumulator>,
    diagnostics: Vec<EvidenceDiagnostic>,
    notes: Vec<EvidencePrecisionNote>,
    diagnostics_truncated: bool,
    sources_truncated: bool,
    partial: bool,
    finding_count: usize,
    coverage_line_count: usize,
    test_case_count: usize,
    test_failure_count: usize,
    runtime_span_count: usize,
    runtime_edges: BTreeMap<(String, String), RuntimeEdgeAccumulator>,
    runtime_edges_truncated: bool,
    fact_artifacts: Vec<crate::facts::FactArtifact>,
    fact_artifacts_truncated: bool,
    fact_relationships: Vec<crate::facts::FactRelationship>,
    fact_relationships_truncated: bool,
    knowledge_match_count: usize,
    deadline: Option<Instant>,
    artifact_identities: HashMap<String, (String, u64)>,
}

#[derive(Default)]
struct SourceStats {
    facts_total: usize,
    facts_returned: usize,
    files: BTreeSet<String>,
    partial: bool,
    invalid_records: bool,
    work_limited: bool,
    deadline_reached: bool,
}

struct SourceInput {
    label: String,
    bytes: Vec<u8>,
}

#[derive(Debug)]
enum SourceFailure {
    Unavailable,
    TooLarge,
    Changed,
    InvalidUtf8,
    InvalidFormat,
    CodeownersTooLarge,
    Deadline,
    Git(WorkingTreeDiffError),
}

impl SourceFailure {
    fn code(&self) -> &'static str {
        match self {
            Self::Unavailable => "source_unavailable",
            Self::TooLarge => "source_too_large",
            Self::Changed => "source_changed",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::InvalidFormat => "invalid_format",
            Self::CodeownersTooLarge => "codeowners_too_large",
            Self::Deadline => "deadline_exceeded",
            Self::Git(WorkingTreeDiffError::GitTimeout) => "git_timeout",
            Self::Git(WorkingTreeDiffError::GitOutputLimit) => "git_output_limit",
            Self::Git(_) => "git_unavailable",
        }
    }

    fn message(&self) -> &'static str {
        match self {
            Self::Unavailable => "The evidence source could not be read.",
            Self::TooLarge => "The evidence source exceeds the 32 MiB read limit.",
            Self::Changed => "The evidence source changed while Lens was reading it.",
            Self::InvalidUtf8 => "The evidence source is not valid UTF-8.",
            Self::InvalidFormat => "The evidence source does not match the expected format.",
            Self::CodeownersTooLarge => {
                "The CODEOWNERS file is not under GitHub's 3 MiB ingestion limit."
            }
            Self::Deadline => "Evidence loading reached the Lens request deadline.",
            Self::Git(WorkingTreeDiffError::GitTimeout) => {
                "Git history collection exceeded its deadline."
            }
            Self::Git(WorkingTreeDiffError::GitOutputLimit) => {
                "Git history collection exceeded its 8 MiB output limit."
            }
            Self::Git(_) => "Git history was unavailable.",
        }
    }
}

pub fn collect(
    root: &Path,
    options: &EvidenceOptions,
    impact: &ChangeImpactResponse,
    deadline: Option<Instant>,
) -> EvidenceSnapshot {
    collect_with_extensions(
        root,
        options,
        &EvidenceExtensionOptions::default(),
        impact,
        deadline,
    )
}

pub fn collect_with_extensions(
    root: &Path,
    options: &EvidenceOptions,
    extensions: &EvidenceExtensionOptions,
    impact: &ChangeImpactResponse,
    deadline: Option<Instant>,
) -> EvidenceSnapshot {
    collect_internal(root, options, extensions, impact, None, false, deadline)
}

pub fn collect_with_store(
    root: &Path,
    options: &EvidenceOptions,
    extensions: &EvidenceExtensionOptions,
    impact: &ChangeImpactResponse,
    store: &crate::store::Store,
    deadline: Option<Instant>,
) -> EvidenceSnapshot {
    collect_internal(
        root,
        options,
        extensions,
        impact,
        Some(store),
        false,
        deadline,
    )
}

pub(crate) fn collect_with_store_and_normalized_facts(
    root: &Path,
    options: &EvidenceOptions,
    extensions: &EvidenceExtensionOptions,
    impact: &ChangeImpactResponse,
    store: &crate::store::Store,
    deadline: Option<Instant>,
) -> EvidenceSnapshot {
    collect_internal(
        root,
        options,
        extensions,
        impact,
        Some(store),
        true,
        deadline,
    )
}

fn collect_internal(
    root: &Path,
    options: &EvidenceOptions,
    extensions: &EvidenceExtensionOptions,
    impact: &ChangeImpactResponse,
    store: Option<&crate::store::Store>,
    include_normalized_facts: bool,
    deadline: Option<Instant>,
) -> EvidenceSnapshot {
    let (relevant, relevant_truncated) = relevant_paths(impact);
    let sources_truncated = options
        .sarif
        .len()
        .saturating_add(options.coverage.len())
        .saturating_add(extensions.junit.len())
        .saturating_add(extensions.otel.len())
        > MAX_ARTIFACT_SOURCES;
    let mut collector = Collector {
        root,
        relevant,
        sources: Vec::new(),
        files: BTreeMap::new(),
        diagnostics: Vec::new(),
        notes: Vec::new(),
        diagnostics_truncated: false,
        sources_truncated,
        partial: relevant_truncated || sources_truncated,
        finding_count: 0,
        coverage_line_count: 0,
        test_case_count: 0,
        test_failure_count: 0,
        runtime_span_count: 0,
        runtime_edges: BTreeMap::new(),
        runtime_edges_truncated: false,
        fact_artifacts: Vec::new(),
        fact_artifacts_truncated: false,
        fact_relationships: Vec::new(),
        fact_relationships_truncated: false,
        knowledge_match_count: 0,
        deadline,
        artifact_identities: HashMap::new(),
    };

    if !options.sarif.is_empty()
        || !options.coverage.is_empty()
        || !extensions.junit.is_empty()
        || !extensions.otel.is_empty()
    {
        collector.notes.push(EvidencePrecisionNote {
            source_id: "lens",
            code: "artifact_path_relocation",
            message: "Artifact paths match repository-relative paths exactly when possible; a unique suffix match can relocate reports produced under a different build root.".into(),
        });
        collector.notes.push(EvidencePrecisionNote {
            source_id: "lens",
            code: "artifact_revision_unverified",
            message: "Lens preserves artifact provenance labels but cannot prove that a SARIF, coverage, JUnit, or OTLP report was produced from the current Git revision.".into(),
        });
    }
    if !options.coverage.is_empty() {
        collector.notes.push(EvidencePrecisionNote {
            source_id: "lens",
            code: "coverage_merge_max",
            message: "Duplicate file/line coverage facts are merged by their maximum hit count; coverage remains file-level evidence.".into(),
        });
    }
    if options.git_commits > 0 {
        collector.notes.push(EvidencePrecisionNote {
            source_id: "git-history",
            code: "git_history_scope",
            message: "Git churn uses a bounded no-renames log window; contributor identities are author display names and rename history is not followed.".into(),
        });
    }
    if !extensions.otel.is_empty() {
        collector.notes.push(EvidencePrecisionNote {
            source_id: "otel",
            code: "runtime_file_path_evidence",
            message: "OTLP spans match only explicit code.file.path/code.filepath attributes. Parent-child span pairs are file-level runtime evidence and never add codegraph topology.".into(),
        });
    }

    if relevant_truncated {
        collector.diagnostic(
            "lens",
            "relevant_file_limit",
            "Evidence matching was limited to the first 1,000 returned trace files.",
        );
    }
    if sources_truncated {
        collector.diagnostic(
            "lens",
            "source_limit",
            "Only the first 64 SARIF, coverage, JUnit, and OTLP inputs were evaluated.",
        );
    }

    let mut loaded_artifacts = 0;
    for (index, path) in options.sarif.iter().enumerate() {
        if loaded_artifacts >= MAX_ARTIFACT_SOURCES {
            break;
        }
        collector.load_sarif(path, format!("sarif:{index}"));
        loaded_artifacts += 1;
    }
    for (index, path) in options.coverage.iter().enumerate() {
        if loaded_artifacts >= MAX_ARTIFACT_SOURCES {
            break;
        }
        collector.load_coverage(path, format!("coverage:{index}"));
        loaded_artifacts += 1;
    }
    for (index, path) in extensions.junit.iter().enumerate() {
        if loaded_artifacts >= MAX_ARTIFACT_SOURCES {
            break;
        }
        collector.load_junit(path, format!("junit:{index}"));
        loaded_artifacts += 1;
    }
    for (index, path) in extensions.otel.iter().enumerate() {
        if loaded_artifacts >= MAX_ARTIFACT_SOURCES {
            break;
        }
        collector.load_otel(path, format!("otel:{index}"));
        loaded_artifacts += 1;
    }

    let codeowners = options.codeowners.clone().or_else(|| {
        options
            .discover_codeowners
            .then(|| discover_codeowners(root))
            .flatten()
    });
    if let Some(path) = codeowners {
        collector.notes.push(EvidencePrecisionNote {
            source_id: "codeowners",
            code: "working_tree_syntax_only",
            message: "Lens matches the working-tree CODEOWNERS syntax. It does not verify that owners exist or have write access; GitHub review assignment uses the base-branch file.".into(),
        });
        collector.load_codeowners(&path, "codeowners".into());
    }
    if options.git_commits > 0 {
        collector.load_git_history(
            options.git_commits,
            "git-history".into(),
            &impact.baseline.head_oid,
        );
    }
    if extensions.project_knowledge {
        collector.notes.push(EvidencePrecisionNote {
            source_id: "project-knowledge",
            code: "derived_history_snapshot",
            message: "Project knowledge is correlated only by exact repository-path mentions from the derived history index; Markdown remains authoritative and must be re-indexed after changes.".into(),
        });
        match store.and_then(|store| {
            let (entries, bounded_truncated) = store
                .project_history_entries_bounded(MAX_KNOWLEDGE_ARTIFACTS, MAX_KNOWLEDGE_BYTES)
                .ok()?;
            let skipped = store
                .meta_value("project_history_skipped")
                .ok()?
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            let indexed_truncated = store
                .meta_value("project_history_truncated")
                .ok()?
                .is_some_and(|value| value == "true");
            Some((entries, skipped, bounded_truncated || indexed_truncated))
        }) {
            Some((entries, skipped, truncated)) => collector.load_project_knowledge(
                &entries,
                skipped,
                truncated,
                "project-knowledge".into(),
            ),
            None => collector.source_error(
                "project-knowledge".into(),
                "project_knowledge",
                "Indexed project knowledge".into(),
                SourceFailure::Unavailable,
            ),
        }
    }

    if include_normalized_facts {
        let store = store.expect("normalized facts require a Mastermind store");
        match crate::facts::snapshot_for_paths(
            store,
            &collector.relevant,
            crate::facts::MAX_LENS_FACTS,
            deadline,
        ) {
            Ok(snapshot) => collector.load_normalized_facts(snapshot),
            Err(error) => {
                collector.diagnostic("facts", "normalized_facts_unavailable", error.to_string());
                collector.sources.push(EvidenceSource {
                    id: "facts".into(),
                    kind: "facts",
                    label: "Normalized declarative facts".into(),
                    status: "error",
                    facts_total: None,
                    facts_returned: 0,
                    files_matched: 0,
                    artifact_sha256: None,
                    artifact_bytes: None,
                });
            }
        }
    }

    collector.finish(options.git_commits)
}

fn relevant_paths(impact: &ChangeImpactResponse) -> (BTreeSet<String>, bool) {
    let mut paths = BTreeSet::new();
    paths.extend(
        impact
            .changes
            .files
            .items
            .iter()
            .map(|item| item.path.clone()),
    );
    paths.extend(
        impact
            .changes
            .symbols
            .items
            .iter()
            .map(|item| item.file.clone()),
    );
    paths.extend(
        impact
            .impact
            .items
            .iter()
            .map(|item| item.symbol.file.clone()),
    );
    paths.extend(
        impact
            .tests
            .items
            .iter()
            .map(|item| item.symbol.file.clone()),
    );
    let truncated = paths.len() > MAX_RELEVANT_FILES;
    if truncated {
        paths = paths.into_iter().take(MAX_RELEVANT_FILES).collect();
    }
    (paths, truncated)
}

impl Collector<'_> {
    fn deadline_reached(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn diagnostic(
        &mut self,
        source_id: impl Into<String>,
        code: &'static str,
        message: impl Into<String>,
    ) {
        self.partial = true;
        if self.diagnostics.len() >= MAX_DIAGNOSTICS {
            self.diagnostics_truncated = true;
            return;
        }
        self.diagnostics.push(EvidenceDiagnostic {
            source_id: source_id.into(),
            code,
            message: truncate_text(&message.into(), 300),
        });
    }

    fn source_error(
        &mut self,
        id: String,
        kind: &'static str,
        label: String,
        failure: SourceFailure,
    ) {
        self.diagnostic(id.clone(), failure.code(), failure.message());
        let artifact = self.artifact_identities.remove(&id);
        self.sources.push(EvidenceSource {
            id,
            kind,
            label,
            status: "error",
            facts_total: None,
            facts_returned: 0,
            files_matched: 0,
            artifact_sha256: artifact.as_ref().map(|value| value.0.clone()),
            artifact_bytes: artifact.map(|value| value.1),
        });
    }

    fn source_done(&mut self, id: String, kind: &'static str, label: String, stats: SourceStats) {
        self.partial |= stats.partial;
        let artifact = self.artifact_identities.remove(&id);
        self.sources.push(EvidenceSource {
            id,
            kind,
            label,
            status: if stats.partial { "partial" } else { "loaded" },
            facts_total: (!stats.partial).then(|| saturating_u32(stats.facts_total)),
            facts_returned: saturating_u32(stats.facts_returned),
            files_matched: saturating_u32(stats.files.len()),
            artifact_sha256: artifact.as_ref().map(|value| value.0.clone()),
            artifact_bytes: artifact.map(|value| value.1),
        });
    }

    fn load_normalized_facts(&mut self, snapshot: crate::facts::FactSnapshot) {
        self.partial |= snapshot.partial;
        self.sources_truncated |= snapshot.sources.truncated;
        self.fact_artifacts_truncated |= snapshot.artifacts.truncated;
        self.fact_relationships_truncated |= snapshot.relationships.truncated;
        if !snapshot.sources.items.is_empty() {
            self.notes.push(EvidencePrecisionNote {
                source_id: "facts",
                code: "normalized_fact_overlay",
                message: "Declarative facts are bound to the indexed repository, exact Git revision, and source digests. Relationship facts decorate only matching static graph endpoints and never create topology.".into(),
            });
        }
        let producers = snapshot
            .sources
            .items
            .iter()
            .map(|source| (source.id.clone(), source.producer.clone()))
            .collect::<HashMap<_, _>>();
        for source in snapshot.sources.items {
            self.sources.push(EvidenceSource {
                id: source.id,
                kind: "facts",
                label: format!(
                    "{} {} · {}",
                    source.producer, source.producer_version, source.dataset
                ),
                status: source.status,
                facts_total: Some(source.facts_total),
                facts_returned: source.facts_returned,
                files_matched: source.files_matched,
                artifact_sha256: Some(source.manifest_sha256),
                artifact_bytes: Some(source.manifest_bytes),
            });
        }
        let mut annotation_truncated = false;
        for annotation in snapshot.annotations.items {
            let tool = producers
                .get(&annotation.source_id)
                .cloned()
                .unwrap_or_else(|| "declarative-facts".into());
            let finding = EvidenceFinding {
                source_id: annotation.source_id,
                tool,
                rule_id: annotation.category,
                level: annotation.severity,
                message: format!("{}: {}", annotation.title, annotation.message),
                line: Some(annotation.line),
                column: annotation.column,
            };
            if !self.add_finding(&annotation.path, finding) {
                annotation_truncated = true;
            }
        }
        if annotation_truncated {
            self.diagnostic(
                "facts",
                "normalized_fact_limit",
                "Some normalized annotations were omitted by the Lens finding limits.",
            );
        }
        self.fact_artifacts = snapshot.artifacts.items;
        self.fact_relationships = snapshot.relationships.items;
        for diagnostic in snapshot.diagnostics.items {
            self.diagnostic(diagnostic.source_id, diagnostic.code, diagnostic.message);
        }
    }

    fn register_artifact(&mut self, id: &str, input: &SourceInput) {
        self.artifact_identities.insert(
            id.to_string(),
            (
                crate::hex::encode(&Sha256::digest(&input.bytes)),
                input.bytes.len() as u64,
            ),
        );
    }

    fn read_source(&self, path: &Path) -> Result<SourceInput, SourceFailure> {
        if self.deadline_reached() {
            return Err(SourceFailure::Deadline);
        }
        let requested = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let resolved = requested
            .canonicalize()
            .map_err(|_| SourceFailure::Unavailable)?;
        let initial = std::fs::metadata(&resolved).map_err(|_| SourceFailure::Unavailable)?;
        if !initial.is_file() {
            return Err(SourceFailure::Unavailable);
        }
        if initial.len() > MAX_ARTIFACT_BYTES {
            return Err(SourceFailure::TooLarge);
        }
        let mut file = File::open(&resolved).map_err(|_| SourceFailure::Unavailable)?;
        let before = file.metadata().map_err(|_| SourceFailure::Unavailable)?;
        if !before.is_file() {
            return Err(SourceFailure::Unavailable);
        }
        if initial.len() != before.len() || modified(&initial) != modified(&before) {
            return Err(SourceFailure::Changed);
        }
        if before.len() > MAX_ARTIFACT_BYTES {
            return Err(SourceFailure::TooLarge);
        }
        let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
        file.by_ref()
            .take(MAX_ARTIFACT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| SourceFailure::Unavailable)?;
        if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
            return Err(SourceFailure::TooLarge);
        }
        let after = file.metadata().map_err(|_| SourceFailure::Changed)?;
        if before.len() != after.len() || modified(&before) != modified(&after) {
            return Err(SourceFailure::Changed);
        }
        if self.deadline_reached() {
            return Err(SourceFailure::Deadline);
        }
        let label = resolved
            .strip_prefix(self.root)
            .ok()
            .map(display_path)
            .or_else(|| {
                resolved
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "evidence artifact".into());
        Ok(SourceInput {
            label: truncate_text(&label, 180),
            bytes,
        })
    }

    fn add_finding(&mut self, path: &str, finding: EvidenceFinding) -> bool {
        if self.finding_count >= MAX_FINDINGS {
            return false;
        }
        let entry = self.files.entry(path.to_string()).or_default();
        if entry.findings.len() >= MAX_FINDINGS_PER_FILE {
            return false;
        }
        entry.findings.push(finding);
        self.finding_count += 1;
        true
    }

    fn add_coverage(&mut self, path: &str, source_id: &str, line: u32, hits: u64) -> bool {
        let entry = self.files.entry(path.to_string()).or_default();
        let is_new = !entry.coverage_lines.contains_key(&line);
        if is_new && self.coverage_line_count >= MAX_COVERAGE_LINES {
            return false;
        }
        if is_new {
            self.coverage_line_count += 1;
        }
        entry
            .coverage_lines
            .entry(line)
            .and_modify(|value| *value = (*value).max(hits))
            .or_insert(hits);
        entry.coverage_sources.insert(source_id.to_string());
        true
    }

    fn add_test_case(
        &mut self,
        path: &str,
        source_id: &str,
        case: junit::JunitCase,
    ) -> (bool, bool) {
        if self.test_case_count >= MAX_TEST_CASES {
            return (false, false);
        }
        self.test_case_count += 1;
        let entry = self.files.entry(path.to_string()).or_default();
        let tests = &mut entry.tests;
        tests.source_ids.insert(source_id.to_string());
        tests.total = tests.total.saturating_add(1);
        tests.duration_ms = tests.duration_ms.saturating_add(case.duration_ms);
        let mut failure_truncated = false;
        let status = match case.status {
            junit::JunitStatus::Passed => {
                tests.passed = tests.passed.saturating_add(1);
                None
            }
            junit::JunitStatus::Failed => {
                tests.failed = tests.failed.saturating_add(1);
                Some("failed")
            }
            junit::JunitStatus::Error => {
                tests.errors = tests.errors.saturating_add(1);
                Some("error")
            }
            junit::JunitStatus::Skipped => {
                tests.skipped = tests.skipped.saturating_add(1);
                None
            }
        };
        if let Some(status) = status {
            if self.test_failure_count < MAX_TEST_FAILURES
                && tests.failures.len() < MAX_TEST_FAILURES_PER_FILE
            {
                self.test_failure_count += 1;
                tests.failures.push(TestFailureEvidence {
                    source_id: source_id.to_string(),
                    name: case.name,
                    class_name: case.class_name,
                    status: status.into(),
                    message: if case.message.is_empty() {
                        "No failure detail returned.".into()
                    } else {
                        case.message
                    },
                });
            } else {
                tests.failures_truncated = true;
                failure_truncated = true;
            }
        }
        (true, failure_truncated)
    }

    fn add_runtime_span(&mut self, path: &str, source_id: &str, trace_id: &str) -> bool {
        if self.runtime_span_count >= MAX_RUNTIME_SPANS {
            return false;
        }
        self.runtime_span_count += 1;
        let runtime = &mut self.files.entry(path.to_string()).or_default().runtime;
        runtime.source_ids.insert(source_id.to_string());
        runtime.spans = runtime.spans.saturating_add(1);
        runtime.traces.insert(truncate_text(trace_id, 64));
        true
    }

    fn add_runtime_edge(
        &mut self,
        parent_file: &str,
        child_file: &str,
        source_id: &str,
        trace_id: &str,
        span_name: &str,
    ) -> (bool, bool) {
        let key = (parent_file.to_string(), child_file.to_string());
        if !self.runtime_edges.contains_key(&key) && self.runtime_edges.len() >= MAX_RUNTIME_EDGES {
            self.runtime_edges_truncated = true;
            return (false, false);
        }
        let edge = self.runtime_edges.entry(key).or_default();
        edge.source_ids.insert(source_id.to_string());
        edge.spans = edge.spans.saturating_add(1);
        edge.traces.insert(truncate_text(trace_id, 64));
        let mut name_truncated = false;
        let span_name = truncate_text(span_name, 160);
        if !edge.span_names.contains(&span_name) {
            if edge.span_names.len() < MAX_RUNTIME_NAMES_PER_EDGE {
                edge.span_names.insert(span_name);
            } else {
                edge.names_truncated = true;
                name_truncated = true;
            }
        }
        (true, name_truncated)
    }

    fn load_sarif(&mut self, path: &Path, id: String) {
        let fallback = requested_label(path);
        let input = match self.read_source(path) {
            Ok(input) => input,
            Err(error) => return self.source_error(id, "sarif", fallback, error),
        };
        self.register_artifact(&id, &input);
        let label = input.label;
        let value: Value = match serde_json::from_slice(strip_utf8_bom(&input.bytes)) {
            Ok(value) => value,
            Err(_) => return self.source_error(id, "sarif", label, SourceFailure::InvalidFormat),
        };
        if self.deadline_reached() {
            return self.source_error(id, "sarif", label, SourceFailure::Deadline);
        }
        if value.get("version").and_then(Value::as_str) != Some("2.1.0") {
            return self.source_error(id, "sarif", label, SourceFailure::InvalidFormat);
        }
        let Some(runs) = value.get("runs").and_then(Value::as_array) else {
            return self.source_error(id, "sarif", label, SourceFailure::InvalidFormat);
        };
        let mut stats = SourceStats::default();
        let mut finding_limited = false;
        'runs: for run in runs {
            if self.deadline_reached() {
                stats.partial = true;
                self.diagnostic(
                    id.clone(),
                    SourceFailure::Deadline.code(),
                    SourceFailure::Deadline.message(),
                );
                break;
            }
            let tool = sarif_tool(run);
            let bases = sarif_uri_bases(run);
            let rules = sarif_rules(run);
            let results = run
                .get("results")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for result in results {
                let rule_index = result
                    .get("ruleIndex")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok());
                let rule_id = result
                    .get("ruleId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        rule_index
                            .and_then(|index| rules.get(index))
                            .map(|rule| rule.id.clone())
                    })
                    .unwrap_or_else(|| "unclassified".into());
                let rule_default = rules
                    .iter()
                    .find(|rule| rule.id == rule_id)
                    .map(|rule| rule.level.as_str());
                let level = result
                    .get("level")
                    .and_then(Value::as_str)
                    .filter(|value| matches!(*value, "error" | "warning" | "note" | "none"))
                    .or(rule_default)
                    .unwrap_or("warning");
                let message = result
                    .pointer("/message/text")
                    .and_then(Value::as_str)
                    .or_else(|| result.pointer("/message/markdown").and_then(Value::as_str))
                    .unwrap_or("No SARIF message returned.");
                let locations = result
                    .get("locations")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                if locations.is_empty() {
                    if stats.facts_total >= MAX_SOURCE_FACTS {
                        stats.partial = true;
                        self.diagnostic(
                            id.clone(),
                            "source_fact_limit",
                            "SARIF parsing reached the one-million-fact work limit.",
                        );
                        break 'runs;
                    }
                    stats.facts_total += 1;
                    continue;
                }
                for location in locations {
                    if stats.facts_total >= MAX_SOURCE_FACTS {
                        stats.partial = true;
                        self.diagnostic(
                            id.clone(),
                            "source_fact_limit",
                            "SARIF parsing reached the one-million-fact work limit.",
                        );
                        break 'runs;
                    }
                    if stats.facts_total.is_multiple_of(1_024) && self.deadline_reached() {
                        stats.partial = true;
                        self.diagnostic(
                            id.clone(),
                            SourceFailure::Deadline.code(),
                            SourceFailure::Deadline.message(),
                        );
                        break 'runs;
                    }
                    stats.facts_total += 1;
                    let artifact = location.pointer("/physicalLocation/artifactLocation");
                    let Some(uri) = artifact
                        .and_then(|value| value.get("uri"))
                        .and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let combined = artifact
                        .and_then(|value| value.get("uriBaseId"))
                        .and_then(Value::as_str)
                        .and_then(|base_id| bases.get(base_id))
                        .map_or_else(|| uri.to_string(), |base| join_uri(base, uri));
                    let Some(repo_path) =
                        normalize_evidence_path(self.root, &combined, &self.relevant)
                    else {
                        continue;
                    };
                    let line = location
                        .pointer("/physicalLocation/region/startLine")
                        .and_then(Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok());
                    let column = location
                        .pointer("/physicalLocation/region/startColumn")
                        .and_then(Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok());
                    let finding = EvidenceFinding {
                        source_id: id.clone(),
                        tool: truncate_text(&tool, 120),
                        rule_id: truncate_text(&rule_id, 160),
                        level: level.to_string(),
                        message: truncate_text(message, 500),
                        line,
                        column,
                    };
                    if self.add_finding(&repo_path, finding) {
                        stats.facts_returned += 1;
                        stats.files.insert(repo_path);
                    } else {
                        stats.partial = true;
                        finding_limited = true;
                    }
                }
            }
        }
        if finding_limited {
            self.diagnostic(
                id.clone(),
                "finding_limit",
                "Some matching SARIF findings were omitted by the bounded evidence envelope.",
            );
        }
        self.source_done(id, "sarif", label, stats);
    }

    fn load_coverage(&mut self, path: &Path, id: String) {
        let fallback = requested_label(path);
        let input = match self.read_source(path) {
            Ok(input) => input,
            Err(error) => return self.source_error(id, "coverage", fallback, error),
        };
        self.register_artifact(&id, &input);
        let label = input.label;
        let coverage_bytes = strip_utf8_bom(&input.bytes);
        let first = coverage_bytes
            .iter()
            .copied()
            .find(|byte| !byte.is_ascii_whitespace());
        let result = if first == Some(b'<') {
            parse_cobertura(self.root, &self.relevant, coverage_bytes, self.deadline)
        } else {
            parse_lcov(self.root, &self.relevant, coverage_bytes, self.deadline)
        };
        let (records, mut stats) = match result {
            Ok(value) => value,
            Err(error) => return self.source_error(id, "coverage", label, error),
        };
        'records: for (repo_path, lines) in records {
            for (line, hits) in lines {
                if self.deadline_reached() {
                    stats.partial = true;
                    stats.deadline_reached = true;
                    break 'records;
                }
                if self.add_coverage(&repo_path, &id, line, hits) {
                    stats.facts_returned += 1;
                    stats.files.insert(repo_path.clone());
                } else {
                    stats.partial = true;
                    stats.work_limited = true;
                }
            }
        }
        if stats.invalid_records {
            self.diagnostic(
                id.clone(),
                "invalid_coverage_record",
                "Some coverage records were invalid and were skipped.",
            );
        }
        if stats.work_limited {
            self.diagnostic(
                id.clone(),
                "coverage_line_limit",
                "Some matching coverage lines were omitted by the bounded evidence envelope.",
            );
        }
        if stats.deadline_reached {
            self.diagnostic(
                id.clone(),
                SourceFailure::Deadline.code(),
                SourceFailure::Deadline.message(),
            );
        }
        self.source_done(id, "coverage", label, stats);
    }

    fn load_junit(&mut self, path: &Path, id: String) {
        let fallback = requested_label(path);
        let input = match self.read_source(path) {
            Ok(input) => input,
            Err(error) => return self.source_error(id, "junit", fallback, error),
        };
        self.register_artifact(&id, &input);
        let label = input.label;
        let parsed = match junit::parse(strip_utf8_bom(&input.bytes), self.deadline) {
            Ok(parsed) => parsed,
            Err(error) => return self.source_error(id, "junit", label, error),
        };
        let mut stats = SourceStats {
            facts_total: parsed.facts_total,
            partial: parsed.partial,
            invalid_records: parsed.invalid_records,
            work_limited: parsed.work_limited,
            deadline_reached: parsed.deadline_reached,
            ..SourceStats::default()
        };
        let mut failure_limited = false;
        for case in parsed.cases {
            if self.deadline_reached() {
                stats.partial = true;
                stats.deadline_reached = true;
                break;
            }
            let Some(raw_path) = case.file.as_deref() else {
                continue;
            };
            let Some(repo_path) = normalize_evidence_path(self.root, raw_path, &self.relevant)
            else {
                continue;
            };
            let (added, failure_truncated) = self.add_test_case(&repo_path, &id, case);
            if added {
                stats.facts_returned += 1;
                stats.files.insert(repo_path);
                failure_limited |= failure_truncated;
            } else {
                stats.partial = true;
                stats.work_limited = true;
            }
        }
        if stats.invalid_records {
            self.diagnostic(
                id.clone(),
                "invalid_junit_record",
                "Some JUnit testcases were invalid and were skipped.",
            );
        }
        if stats.work_limited {
            self.diagnostic(
                id.clone(),
                "junit_case_limit",
                "Some matching JUnit testcases were omitted by the 100,000-case evidence limit.",
            );
        }
        if failure_limited {
            stats.partial = true;
            self.diagnostic(
                id.clone(),
                "junit_failure_limit",
                "JUnit totals remain complete, but some failure details were omitted by evidence limits.",
            );
        }
        if stats.deadline_reached {
            self.diagnostic(
                id.clone(),
                SourceFailure::Deadline.code(),
                SourceFailure::Deadline.message(),
            );
        }
        self.source_done(id, "junit", label, stats);
    }

    fn load_otel(&mut self, path: &Path, id: String) {
        let fallback = requested_label(path);
        let input = match self.read_source(path) {
            Ok(input) => input,
            Err(error) => return self.source_error(id, "otel", fallback, error),
        };
        self.register_artifact(&id, &input);
        let label = input.label;
        let parsed = match otel::parse(strip_utf8_bom(&input.bytes), self.deadline) {
            Ok(parsed) => parsed,
            Err(error) => return self.source_error(id, "otel", label, error),
        };
        let mut stats = SourceStats {
            facts_total: parsed.facts_total,
            partial: parsed.partial,
            invalid_records: parsed.invalid_records,
            work_limited: parsed.work_limited,
            deadline_reached: parsed.deadline_reached,
            ..SourceStats::default()
        };
        let mut mapped = HashMap::<(String, String), (String, String)>::new();
        let mut children = Vec::new();
        for span in parsed.spans {
            if self.deadline_reached() {
                stats.partial = true;
                stats.deadline_reached = true;
                break;
            }
            let Some(raw_path) = span.file.as_deref() else {
                continue;
            };
            let Some(repo_path) = normalize_evidence_path(self.root, raw_path, &self.relevant)
            else {
                continue;
            };
            if !self.add_runtime_span(&repo_path, &id, &span.trace_id) {
                stats.partial = true;
                stats.work_limited = true;
                continue;
            }
            stats.facts_returned += 1;
            stats.files.insert(repo_path.clone());
            let key = (span.trace_id.clone(), span.span_id.clone());
            if mapped
                .insert(key, (repo_path.clone(), span.name.clone()))
                .is_some()
            {
                stats.partial = true;
                stats.invalid_records = true;
            }
            if let Some(parent_span_id) = span.parent_span_id {
                children.push((span.trace_id, parent_span_id, repo_path, span.name));
            }
        }
        let mut edge_limited = false;
        let mut name_limited = false;
        for (trace_id, parent_span_id, child_file, span_name) in children {
            let Some((parent_file, _)) = mapped.get(&(trace_id.clone(), parent_span_id)) else {
                continue;
            };
            let (added, truncated) =
                self.add_runtime_edge(parent_file, &child_file, &id, &trace_id, &span_name);
            edge_limited |= !added;
            name_limited |= truncated;
        }
        if stats.invalid_records {
            self.diagnostic(
                id.clone(),
                "invalid_otel_span",
                "Some OTLP spans were invalid or duplicated and were skipped.",
            );
        }
        if stats.work_limited {
            self.diagnostic(
                id.clone(),
                "otel_span_limit",
                "Some matching OTLP spans were omitted by the 100,000-span evidence limit.",
            );
        }
        if edge_limited {
            stats.partial = true;
            self.diagnostic(
                id.clone(),
                "otel_edge_limit",
                "Some runtime file pairs were omitted by the 1,000-edge evidence limit.",
            );
        }
        if name_limited {
            stats.partial = true;
            self.diagnostic(
                id.clone(),
                "otel_span_name_limit",
                "Runtime edge counts remain complete, but some span-name samples were omitted.",
            );
        }
        if stats.deadline_reached {
            self.diagnostic(
                id.clone(),
                SourceFailure::Deadline.code(),
                SourceFailure::Deadline.message(),
            );
        }
        self.source_done(id, "otel", label, stats);
    }

    fn load_project_knowledge(
        &mut self,
        entries: &[crate::store::ProjectHistoryEntry],
        skipped: u32,
        truncated: bool,
        id: String,
    ) {
        let label = "Indexed project knowledge".to_string();
        if self.relevant.is_empty() {
            let stats = SourceStats {
                partial: skipped > 0 || truncated,
                ..SourceStats::default()
            };
            return self.source_done(id, "project_knowledge", label, stats);
        }
        let mut pattern_map = BTreeMap::<String, String>::new();
        for path in &self.relevant {
            pattern_map.insert(path.clone(), path.clone());
            if path.contains('/') {
                pattern_map.insert(path.replace('/', "\\"), path.clone());
            }
        }
        let patterns = pattern_map.keys().cloned().collect::<Vec<_>>();
        let canonical_paths = patterns
            .iter()
            .map(|pattern| pattern_map[pattern].clone())
            .collect::<Vec<_>>();
        let matcher = match AhoCorasickBuilder::new()
            .match_kind(MatchKind::Standard)
            .build(&patterns)
        {
            Ok(value) => value,
            Err(_) => {
                return self.source_error(
                    id,
                    "project_knowledge",
                    label,
                    SourceFailure::InvalidFormat,
                )
            }
        };
        let mut stats = SourceStats {
            partial: skipped > 0 || truncated,
            ..SourceStats::default()
        };
        let mut sorted = entries.iter().collect::<Vec<_>>();
        sorted.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.title.cmp(&right.title))
        });
        'entries: for (entry_index, entry) in sorted.into_iter().enumerate() {
            if entry_index.is_multiple_of(64) && self.deadline_reached() {
                stats.partial = true;
                stats.deadline_reached = true;
                break;
            }
            let mut matched_paths = BTreeMap::<String, (usize, usize)>::new();
            for found in matcher.find_overlapping_iter(&entry.body) {
                if !exact_path_boundaries(&entry.body, found.start(), found.end()) {
                    continue;
                }
                let path = canonical_paths[found.pattern().as_usize()].clone();
                matched_paths
                    .entry(path)
                    .or_insert((found.start(), found.end()));
            }
            for (path, (start, end)) in matched_paths {
                stats.facts_total += 1;
                if self.knowledge_match_count >= MAX_KNOWLEDGE_MATCHES {
                    stats.partial = true;
                    stats.work_limited = true;
                    break 'entries;
                }
                let file = self.files.entry(path.clone()).or_default();
                if file.knowledge.len() >= MAX_KNOWLEDGE_PER_FILE {
                    stats.partial = true;
                    stats.work_limited = true;
                    continue;
                }
                self.knowledge_match_count += 1;
                file.knowledge.push(KnowledgeEvidence {
                    source_id: id.clone(),
                    artifact_path: truncate_text(&entry.path, 240),
                    kind: truncate_text(&entry.kind, 80),
                    title: truncate_text(&entry.title, 200),
                    match_kind: "exact_path",
                    excerpt: excerpt_around(&entry.body, start, end),
                });
                stats.facts_returned += 1;
                stats.files.insert(path);
            }
        }
        if skipped > 0 {
            self.diagnostic(
                id.clone(),
                "project_knowledge_skipped",
                format!("{skipped} project-knowledge artifacts were skipped during indexing."),
            );
        }
        if truncated {
            self.diagnostic(
                id.clone(),
                "project_knowledge_index_truncated",
                "The indexed project-knowledge corpus reached its 5,000-artifact limit.",
            );
        }
        if stats.work_limited {
            self.diagnostic(
                id.clone(),
                "project_knowledge_match_limit",
                "Some exact project-knowledge mentions were omitted by evidence limits.",
            );
        }
        if stats.deadline_reached {
            self.diagnostic(
                id.clone(),
                SourceFailure::Deadline.code(),
                SourceFailure::Deadline.message(),
            );
        }
        self.source_done(id, "project_knowledge", label, stats);
    }

    fn load_codeowners(&mut self, path: &Path, id: String) {
        let fallback = requested_label(path);
        let input = match self.read_source(path) {
            Ok(input) => input,
            Err(error) => return self.source_error(id, "codeowners", fallback, error),
        };
        self.register_artifact(&id, &input);
        let label = input.label;
        if input.bytes.len() as u64 >= MAX_CODEOWNERS_BYTES {
            return self.source_error(id, "codeowners", label, SourceFailure::CodeownersTooLarge);
        }
        let text = match std::str::from_utf8(strip_utf8_bom(&input.bytes)) {
            Ok(value) => value,
            Err(_) => {
                return self.source_error(id, "codeowners", label, SourceFailure::InvalidUtf8)
            }
        };
        let mut rules = Vec::new();
        let mut stats = SourceStats::default();
        for (line_index, line) in text.lines().enumerate() {
            if self.deadline_reached() {
                stats.partial = true;
                self.diagnostic(
                    id.clone(),
                    SourceFailure::Deadline.code(),
                    SourceFailure::Deadline.message(),
                );
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if rules.len() >= MAX_CODEOWNER_RULES {
                stats.partial = true;
                self.diagnostic(
                    id.clone(),
                    "codeowner_rule_limit",
                    "CODEOWNERS parsing reached the 50,000-rule limit.",
                );
                break;
            }
            match parse_owner_rule(trimmed) {
                Ok(rule) => {
                    if rule.owners_truncated {
                        stats.partial = true;
                        self.diagnostic(
                            id.clone(),
                            "codeowner_owner_limit",
                            format!(
                                "CODEOWNERS line {} exceeds the 50-owner limit and was truncated.",
                                line_index + 1
                            ),
                        );
                    }
                    stats.facts_total += 1;
                    rules.push(rule);
                }
                Err(()) => {
                    stats.partial = true;
                    self.diagnostic(
                        id.clone(),
                        "invalid_codeowner_pattern",
                        format!(
                            "CODEOWNERS line {} uses unsupported or invalid syntax and was skipped.",
                            line_index + 1
                        ),
                    );
                }
            }
        }
        let relevant = self.relevant.iter().cloned().collect::<Vec<_>>();
        stats.facts_total = relevant.len();
        'paths: for repo_path in &relevant {
            let mut owners = None;
            for (rule_index, rule) in rules.iter().enumerate() {
                if rule_index.is_multiple_of(1_024) && self.deadline_reached() {
                    stats.partial = true;
                    self.diagnostic(
                        id.clone(),
                        SourceFailure::Deadline.code(),
                        SourceFailure::Deadline.message(),
                    );
                    break 'paths;
                }
                if rule.matches(repo_path) {
                    owners = Some(rule.owners.clone());
                }
            }
            if let Some(owners) = owners {
                let accumulator = self.files.entry(repo_path.clone()).or_default();
                accumulator.codeowners = owners;
                accumulator.codeowners_source_id = Some(id.clone());
                stats.facts_returned += 1;
                stats.files.insert(repo_path.clone());
            }
        }
        self.source_done(id, "codeowners", label, stats);
    }

    fn load_git_history(&mut self, commits: u16, id: String, head_oid: &str) {
        let label = format!("Git · last {commits} commits");
        if self.relevant.is_empty() {
            return self.source_done(id, "git_history", label, SourceStats::default());
        }
        if self.deadline_reached() {
            return self.source_error(id, "git_history", label, SourceFailure::Deadline);
        }
        let mut args = vec![
            "--literal-pathspecs".to_string(),
            "log".to_string(),
            "--no-ext-diff".to_string(),
            "--no-renames".to_string(),
            "--format=%x1e%H%x1f%aN%x00".to_string(),
            "--numstat".to_string(),
            "-z".to_string(),
            "-n".to_string(),
            commits.to_string(),
            head_oid.to_string(),
            "--".to_string(),
        ];
        args.extend(self.relevant.iter().cloned());
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = match run_bounded_git_with_limit_until(
            self.root,
            &args,
            None,
            GIT_HISTORY_OUTPUT_LIMIT,
            self.deadline,
        ) {
            Ok(output) if output.success => output.stdout,
            Ok(_) => {
                return self.source_error(
                    id,
                    "git_history",
                    label,
                    SourceFailure::Git(WorkingTreeDiffError::GitUnavailable),
                )
            }
            Err(error) => {
                return self.source_error(id, "git_history", label, SourceFailure::Git(error))
            }
        };
        let mut stats = SourceStats::default();
        for record in output
            .split(|byte| *byte == 0x1e)
            .filter(|part| !part.is_empty())
        {
            if self.deadline_reached() {
                stats.partial = true;
                self.diagnostic(
                    id.clone(),
                    SourceFailure::Deadline.code(),
                    SourceFailure::Deadline.message(),
                );
                break;
            }
            let Some(header_end) = record.iter().position(|byte| *byte == 0) else {
                stats.partial = true;
                continue;
            };
            let header = &record[..header_end];
            let author = header
                .split(|byte| *byte == 0x1f)
                .nth(1)
                .and_then(|value| std::str::from_utf8(value).ok())
                .map(sanitize_identity)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Unknown author".into());
            let mut commit_paths = HashSet::new();
            for entry in record[header_end + 1..]
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
            {
                let entry = trim_ascii_whitespace_start(entry);
                let mut fields = entry.splitn(3, |byte| *byte == b'\t');
                let Some(added) = fields.next() else { continue };
                let Some(deleted) = fields.next() else {
                    continue;
                };
                let Some(path) = fields.next() else { continue };
                stats.facts_total += 1;
                let Some(path) = std::str::from_utf8(path).ok() else {
                    stats.partial = true;
                    continue;
                };
                let path = path.replace('\\', "/");
                if !self.relevant.contains(&path) {
                    continue;
                }
                let accumulator = self.files.entry(path.clone()).or_default();
                accumulator.churn.source_id = Some(id.clone());
                accumulator.churn.lines_added = accumulator
                    .churn
                    .lines_added
                    .saturating_add(parse_numstat(added));
                accumulator.churn.lines_deleted = accumulator
                    .churn
                    .lines_deleted
                    .saturating_add(parse_numstat(deleted));
                if commit_paths.insert(path.clone()) {
                    accumulator.churn.commits = accumulator.churn.commits.saturating_add(1);
                    if let Some(existing) = accumulator.churn.contributors.get_mut(&author) {
                        *existing = existing.saturating_add(1);
                    } else if accumulator.churn.contributors.len() < MAX_CONTRIBUTORS_PER_FILE {
                        accumulator.churn.contributors.insert(author.clone(), 1);
                    } else {
                        accumulator.churn.contributors_truncated = true;
                        stats.partial = true;
                    }
                }
                stats.facts_returned += 1;
                stats.files.insert(path);
            }
        }
        if stats.partial
            && self
                .files
                .values()
                .any(|file| file.churn.contributors_truncated)
        {
            self.diagnostic(
                id.clone(),
                "git_contributor_limit",
                "Contributor details are limited to five names per trace file; churn counts remain complete.",
            );
        }
        self.source_done(id, "git_history", label, stats);
    }

    fn finish(self, git_commits: u16) -> EvidenceSnapshot {
        let partial = self.partial || self.diagnostics_truncated;
        let runtime_edges = self
            .runtime_edges
            .into_iter()
            .map(|((parent_file, child_file), edge)| RuntimeEdgeEvidence {
                source_ids: edge.source_ids.into_iter().collect(),
                parent_file,
                child_file,
                spans: edge.spans,
                traces: saturating_u32(edge.traces.len()),
                span_names: edge.span_names.into_iter().collect(),
                names_truncated: edge.names_truncated,
            })
            .collect::<Vec<_>>();
        let runtime_edge_count = saturating_u32(runtime_edges.len());
        let fact_artifact_count = saturating_u32(self.fact_artifacts.len());
        let fact_relationship_count = saturating_u32(self.fact_relationships.len());
        let files = self
            .files
            .into_iter()
            .filter_map(|(path, accumulator)| {
                let coverage = (!accumulator.coverage_lines.is_empty()).then(|| CoverageEvidence {
                    source_ids: accumulator.coverage_sources.into_iter().collect(),
                    lines_found: saturating_u32(accumulator.coverage_lines.len()),
                    lines_hit: saturating_u32(
                        accumulator
                            .coverage_lines
                            .values()
                            .filter(|hits| **hits > 0)
                            .count(),
                    ),
                });
                let mut contributors = accumulator
                    .churn
                    .contributors
                    .into_iter()
                    .map(|(name, commits)| ContributorEvidence { name, commits })
                    .collect::<Vec<_>>();
                contributors.sort_by(|left, right| {
                    right
                        .commits
                        .cmp(&left.commits)
                        .then_with(|| left.name.cmp(&right.name))
                });
                let ownership = (accumulator.codeowners_source_id.is_some()
                    || !accumulator.codeowners.is_empty()
                    || !contributors.is_empty())
                .then(|| OwnershipEvidence {
                    codeowners_source_id: accumulator.codeowners_source_id,
                    codeowners: accumulator.codeowners,
                    contributors_source_id: (!contributors.is_empty())
                        .then(|| accumulator.churn.source_id.clone())
                        .flatten(),
                    contributors,
                    contributors_truncated: accumulator.churn.contributors_truncated,
                });
                let churn = (accumulator.churn.commits > 0).then(|| ChurnEvidence {
                    source_id: accumulator
                        .churn
                        .source_id
                        .unwrap_or_else(|| "git-history".into()),
                    commits: accumulator.churn.commits,
                    lines_added: accumulator.churn.lines_added,
                    lines_deleted: accumulator.churn.lines_deleted,
                });
                let test_results = (accumulator.tests.total > 0).then(|| TestResultsEvidence {
                    source_ids: accumulator.tests.source_ids.into_iter().collect(),
                    total: accumulator.tests.total,
                    passed: accumulator.tests.passed,
                    failed: accumulator.tests.failed,
                    errors: accumulator.tests.errors,
                    skipped: accumulator.tests.skipped,
                    duration_ms: accumulator.tests.duration_ms,
                    failures: accumulator.tests.failures,
                    failures_truncated: accumulator.tests.failures_truncated,
                });
                let runtime = (accumulator.runtime.spans > 0).then(|| RuntimeFileEvidence {
                    source_ids: accumulator.runtime.source_ids.into_iter().collect(),
                    spans: accumulator.runtime.spans,
                    traces: saturating_u32(accumulator.runtime.traces.len()),
                });
                let has_evidence = !accumulator.findings.is_empty()
                    || coverage.is_some()
                    || ownership.is_some()
                    || churn.is_some()
                    || test_results.is_some()
                    || runtime.is_some()
                    || !accumulator.knowledge.is_empty();
                has_evidence.then_some(FileEvidence {
                    path,
                    findings: accumulator.findings,
                    coverage,
                    ownership,
                    churn,
                    test_results,
                    runtime,
                    knowledge: accumulator.knowledge,
                })
            })
            .collect::<Vec<_>>();
        let file_count = saturating_u32(files.len());
        let source_count = saturating_u32(self.sources.len());
        let diagnostic_count = saturating_u32(self.diagnostics.len());
        EvidenceSnapshot {
            schema_version: 1,
            partial,
            sources: EvidenceCollection {
                total: (!self.sources_truncated).then_some(source_count),
                returned: source_count,
                truncated: self.sources_truncated,
                truncation_reason: self.sources_truncated.then_some("source_limit"),
                items: self.sources,
            },
            files: EvidenceCollection {
                total: (!partial).then_some(file_count),
                returned: file_count,
                truncated: partial,
                truncation_reason: partial.then_some("evidence_source_partial"),
                items: files,
            },
            runtime_edges: EvidenceCollection {
                total: (!self.runtime_edges_truncated).then_some(runtime_edge_count),
                returned: runtime_edge_count,
                truncated: self.runtime_edges_truncated,
                truncation_reason: self.runtime_edges_truncated.then_some("runtime_edge_limit"),
                items: runtime_edges,
            },
            fact_artifacts: EvidenceCollection {
                total: (!self.fact_artifacts_truncated).then_some(fact_artifact_count),
                returned: fact_artifact_count,
                truncated: self.fact_artifacts_truncated,
                truncation_reason: self
                    .fact_artifacts_truncated
                    .then_some("normalized_fact_artifact_limit"),
                items: self.fact_artifacts,
            },
            fact_relationships: EvidenceCollection {
                total: (!self.fact_relationships_truncated).then_some(fact_relationship_count),
                returned: fact_relationship_count,
                truncated: self.fact_relationships_truncated,
                truncation_reason: self
                    .fact_relationships_truncated
                    .then_some("normalized_fact_limit"),
                items: self.fact_relationships,
            },
            diagnostics: EvidenceCollection {
                total: (!self.diagnostics_truncated).then_some(diagnostic_count),
                returned: diagnostic_count,
                truncated: self.diagnostics_truncated,
                truncation_reason: self.diagnostics_truncated.then_some("diagnostic_limit"),
                items: self.diagnostics,
            },
            precision_notes: self.notes,
            limits: EvidenceLimits {
                artifact_bytes: MAX_ARTIFACT_BYTES,
                artifact_sources: MAX_ARTIFACT_SOURCES as u32,
                relevant_files: MAX_RELEVANT_FILES as u32,
                findings: MAX_FINDINGS as u32,
                findings_per_file: MAX_FINDINGS_PER_FILE as u32,
                coverage_lines: MAX_COVERAGE_LINES as u32,
                test_cases: MAX_TEST_CASES as u32,
                test_failures: MAX_TEST_FAILURES as u32,
                test_failures_per_file: MAX_TEST_FAILURES_PER_FILE as u32,
                runtime_spans: MAX_RUNTIME_SPANS as u32,
                runtime_edges: MAX_RUNTIME_EDGES as u32,
                runtime_names_per_edge: MAX_RUNTIME_NAMES_PER_EDGE as u32,
                normalized_fact_sources: crate::facts::MAX_LENS_SOURCES as u32,
                normalized_fact_artifacts: crate::facts::MAX_LENS_ARTIFACTS as u32,
                normalized_fact_relationships: crate::facts::MAX_LENS_FACTS as u32,
                knowledge_matches: MAX_KNOWLEDGE_MATCHES as u32,
                knowledge_per_file: MAX_KNOWLEDGE_PER_FILE as u32,
                knowledge_artifacts: MAX_KNOWLEDGE_ARTIFACTS as u32,
                knowledge_bytes: MAX_KNOWLEDGE_BYTES as u64,
                codeowner_rules: MAX_CODEOWNER_RULES as u32,
                codeowners_bytes: MAX_CODEOWNERS_BYTES,
                owners_per_rule: MAX_OWNERS_PER_RULE as u32,
                contributors_per_file: MAX_CONTRIBUTORS_PER_FILE as u32,
                diagnostics: MAX_DIAGNOSTICS as u32,
                git_commits,
            },
        }
    }
}

fn modified(metadata: &std::fs::Metadata) -> Option<SystemTime> {
    metadata.modified().ok()
}

fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)
}

fn requested_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "evidence artifact".into())
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn truncate_text(value: &str, maximum: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(maximum).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn exact_path_boundaries(body: &str, start: usize, end: usize) -> bool {
    let bytes = body.as_bytes();
    let before = start.checked_sub(1).and_then(|index| bytes.get(index));
    let after = bytes.get(end);
    let after_is_boundary = match after {
        Some(b'.') => bytes
            .get(end.saturating_add(1))
            .is_none_or(|byte| !is_path_byte(*byte)),
        Some(byte) => !is_path_byte(*byte),
        None => true,
    };
    before.is_none_or(|byte| !is_path_byte(*byte)) && after_is_boundary
}

fn is_path_byte(byte: u8) -> bool {
    byte >= 0x80
        || byte.is_ascii_alphanumeric()
        || matches!(byte, b'_' | b'-' | b'.' | b'/' | b'\\')
}

fn excerpt_around(body: &str, start: usize, end: usize) -> String {
    let mut excerpt_start = start.saturating_sub(100);
    while excerpt_start < start && !body.is_char_boundary(excerpt_start) {
        excerpt_start += 1;
    }
    let mut excerpt_end = end.saturating_add(100).min(body.len());
    while excerpt_end > end && !body.is_char_boundary(excerpt_end) {
        excerpt_end -= 1;
    }
    let mut output = String::new();
    if excerpt_start > 0 {
        output.push('…');
    }
    let mut previous_space = false;
    for character in body[excerpt_start..excerpt_end].chars() {
        if character.is_whitespace() || character.is_control() {
            if !previous_space {
                output.push(' ');
                previous_space = true;
            }
        } else {
            output.push(character);
            previous_space = false;
        }
    }
    if excerpt_end < body.len() {
        output.push('…');
    }
    truncate_text(output.trim(), 280)
}

pub(crate) fn discover_codeowners(root: &Path) -> Option<PathBuf> {
    exact_directory(root, ".github")
        .and_then(|directory| exact_regular_file(&directory, "CODEOWNERS"))
        .or_else(|| exact_regular_file(root, "CODEOWNERS"))
        .or_else(|| {
            exact_directory(root, "docs")
                .and_then(|directory| exact_regular_file(&directory, "CODEOWNERS"))
        })
}

fn exact_directory(parent: &Path, name: &str) -> Option<PathBuf> {
    exact_child(parent, name).filter(|path| {
        std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
    })
}

fn exact_regular_file(parent: &Path, name: &str) -> Option<PathBuf> {
    exact_child(parent, name).filter(|path| {
        std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
    })
}

fn exact_child(parent: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .find(|entry| entry.file_name() == OsStr::new(name))
        .map(|entry| entry.path())
}

fn sarif_tool(run: &Value) -> String {
    run.pointer("/tool/driver/name")
        .and_then(Value::as_str)
        .unwrap_or("SARIF")
        .to_string()
}

fn sarif_uri_bases(run: &Value) -> HashMap<String, String> {
    run.get("originalUriBaseIds")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| {
            value
                .get("uri")
                .and_then(Value::as_str)
                .map(|uri| (key.clone(), uri.to_string()))
        })
        .collect()
}

struct SarifRule {
    id: String,
    level: String,
}

fn sarif_rules(run: &Value) -> Vec<SarifRule> {
    run.pointer("/tool/driver/rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|rule| {
            let level = rule
                .pointer("/defaultConfiguration/level")
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "error" | "warning" | "note"))
                .unwrap_or("warning");
            SarifRule {
                id: rule
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("unclassified")
                    .to_string(),
                level: level.to_string(),
            }
        })
        .collect()
}

fn join_uri(base: &str, relative: &str) -> String {
    if relative.starts_with("file:") || Path::new(relative).is_absolute() {
        return relative.to_string();
    }
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        relative.trim_start_matches('/')
    )
}

fn normalize_evidence_path(root: &Path, raw: &str, relevant: &BTreeSet<String>) -> Option<String> {
    let without_fragment = raw.split(['?', '#']).next().unwrap_or(raw);
    if without_fragment.contains("://")
        && !without_fragment.starts_with("file:///")
        && !without_fragment.starts_with("file://localhost/")
    {
        return None;
    }
    let decoded = percent_decode(without_fragment)?;
    let decoded = decoded
        .strip_prefix("file://localhost")
        .or_else(|| decoded.strip_prefix("file://"))
        .unwrap_or(&decoded)
        .replace('\\', "/");
    if decoded
        .chars()
        .any(|character| character == '\0' || character.is_control())
    {
        return None;
    }
    if Path::new(&decoded)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return None;
    }

    let candidate = PathBuf::from(&decoded);
    if candidate.is_absolute() {
        if let Ok(relative) = candidate.strip_prefix(root) {
            let normalized = normalize_relative(relative)?;
            if relevant.contains(&normalized) {
                return Some(normalized);
            }
        }
        if let Ok(canonical) = candidate.canonicalize() {
            if let Ok(relative) = canonical.strip_prefix(root) {
                let normalized = normalize_relative(relative)?;
                if relevant.contains(&normalized) {
                    return Some(normalized);
                }
            }
        }
    }

    let root_text = display_path(root);
    if let Some(relative) = decoded
        .strip_prefix(&root_text)
        .and_then(|value| value.strip_prefix('/').or(Some(value)))
    {
        let normalized = normalize_relative(Path::new(relative))?;
        if relevant.contains(&normalized) {
            return Some(normalized);
        }
    }

    if let Some(normalized) = normalize_relative(Path::new(decoded.trim_start_matches('/'))) {
        if relevant.contains(&normalized) {
            return Some(normalized);
        }
    }

    let suffix = decoded.trim_matches('/');
    let mut matches = relevant
        .iter()
        .filter(|path| suffix == path.as_str() || suffix.ends_with(&format!("/{path}")));
    let first = matches.next()?.clone();
    matches.next().is_none().then_some(first)
}

fn normalize_relative(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => {
                let value = value.to_str()?;
                if value.is_empty() || value.chars().any(char::is_control) {
                    return None;
                }
                parts.push(value);
            }
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push(hex_value(high)? * 16 + hex_value(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

type CoverageRecords = BTreeMap<String, BTreeMap<u32, u64>>;

fn parse_lcov(
    root: &Path,
    relevant: &BTreeSet<String>,
    bytes: &[u8],
    deadline: Option<Instant>,
) -> Result<(CoverageRecords, SourceStats), SourceFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| SourceFailure::InvalidUtf8)?;
    let mut records = CoverageRecords::new();
    let mut current = None;
    let mut stats = SourceStats::default();
    let mut saw_format_marker = false;
    for (line_index, line) in text.lines().enumerate() {
        if line_index.is_multiple_of(4_096) && deadline.is_some_and(|value| Instant::now() >= value)
        {
            stats.partial = true;
            stats.deadline_reached = true;
            break;
        }
        if let Some(path) = line.strip_prefix("SF:") {
            saw_format_marker = true;
            current = normalize_evidence_path(root, path.trim(), relevant);
            continue;
        }
        if line == "end_of_record" {
            saw_format_marker = true;
            current = None;
            continue;
        }
        let Some(data) = line.strip_prefix("DA:") else {
            saw_format_marker |= line.starts_with("TN:");
            continue;
        };
        saw_format_marker = true;
        if stats.facts_total >= MAX_SOURCE_FACTS {
            stats.partial = true;
            stats.work_limited = true;
            break;
        }
        stats.facts_total += 1;
        let mut fields = data.split(',');
        let Some(line_number) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            stats.partial = true;
            stats.invalid_records = true;
            continue;
        };
        let Some(hits) = fields.next().and_then(|value| value.parse::<u64>().ok()) else {
            stats.partial = true;
            stats.invalid_records = true;
            continue;
        };
        if let Some(path) = &current {
            records
                .entry(path.clone())
                .or_default()
                .entry(line_number)
                .and_modify(|value| *value = (*value).max(hits))
                .or_insert(hits);
        }
    }
    if !saw_format_marker {
        return Err(SourceFailure::InvalidFormat);
    }
    Ok((records, stats))
}

fn parse_cobertura(
    root: &Path,
    relevant: &BTreeSet<String>,
    bytes: &[u8],
    deadline: Option<Instant>,
) -> Result<(CoverageRecords, SourceStats), SourceFailure> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut sources = Vec::new();
    let mut reading_source = false;
    let mut current_filename = None;
    let mut records = CoverageRecords::new();
    let mut stats = SourceStats::default();
    let mut events = 0usize;
    let mut saw_coverage = false;
    loop {
        if events.is_multiple_of(4_096) && deadline.is_some_and(|value| Instant::now() >= value) {
            stats.partial = true;
            stats.deadline_reached = true;
            break;
        }
        events += 1;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|_| SourceFailure::InvalidFormat)?
        {
            Event::Start(event) => match event.local_name().as_ref() {
                b"coverage" => saw_coverage = true,
                b"source" => reading_source = true,
                b"class" => current_filename = xml_attribute(&event, b"filename"),
                b"line" => add_cobertura_line(
                    root,
                    relevant,
                    &sources,
                    current_filename.as_deref(),
                    &event,
                    &mut records,
                    &mut stats,
                ),
                _ => {}
            },
            Event::Empty(event) => {
                if event.local_name().as_ref() == b"coverage" {
                    saw_coverage = true;
                } else if event.local_name().as_ref() == b"line" {
                    add_cobertura_line(
                        root,
                        relevant,
                        &sources,
                        current_filename.as_deref(),
                        &event,
                        &mut records,
                        &mut stats,
                    );
                }
            }
            Event::Text(event) if reading_source => {
                let value =
                    std::str::from_utf8(event.as_ref()).map_err(|_| SourceFailure::InvalidUtf8)?;
                sources.push(xml_unescape(value)?);
            }
            Event::End(event) => match event.local_name().as_ref() {
                b"source" => reading_source = false,
                b"class" => current_filename = None,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        if stats.facts_total >= MAX_SOURCE_FACTS {
            stats.partial = true;
            stats.work_limited = true;
            break;
        }
        buffer.clear();
    }
    if !saw_coverage {
        return Err(SourceFailure::InvalidFormat);
    }
    Ok((records, stats))
}

fn add_cobertura_line(
    root: &Path,
    relevant: &BTreeSet<String>,
    sources: &[String],
    filename: Option<&str>,
    event: &quick_xml::events::BytesStart<'_>,
    records: &mut CoverageRecords,
    stats: &mut SourceStats,
) {
    stats.facts_total += 1;
    let Some(filename) = filename else { return };
    let Some(line) = xml_attribute(event, b"number").and_then(|value| value.parse::<u32>().ok())
    else {
        stats.partial = true;
        stats.invalid_records = true;
        return;
    };
    let Some(hits) = xml_attribute(event, b"hits").and_then(|value| value.parse::<u64>().ok())
    else {
        stats.partial = true;
        stats.invalid_records = true;
        return;
    };
    let path = normalize_evidence_path(root, filename, relevant).or_else(|| {
        sources.iter().find_map(|source| {
            normalize_evidence_path(
                root,
                &format!("{}/{filename}", source.trim_end_matches('/')),
                relevant,
            )
        })
    });
    if let Some(path) = path {
        records
            .entry(path)
            .or_default()
            .entry(line)
            .and_modify(|value| *value = (*value).max(hits))
            .or_insert(hits);
    }
}

fn xml_attribute(event: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Option<String> {
    event.attributes().flatten().find_map(|attribute| {
        (attribute.key.local_name().as_ref() == name).then(|| {
            std::str::from_utf8(attribute.value.as_ref())
                .ok()
                .and_then(|value| xml_unescape(value).ok())
        })?
    })
}

fn xml_unescape(value: &str) -> Result<String, SourceFailure> {
    quick_xml::escape::unescape(value)
        .map(|value| value.into_owned())
        .map_err(|_| SourceFailure::InvalidFormat)
}

struct OwnerRule {
    matchers: Vec<GlobMatcher>,
    owners: Vec<String>,
    owners_truncated: bool,
}

#[derive(Debug)]
pub(crate) struct CodeownersResolution {
    pub owners_by_path: BTreeMap<String, Vec<String>>,
    pub partial: bool,
    pub diagnostics_truncated: bool,
    pub diagnostics: Vec<String>,
}

/// Resolve CODEOWNERS text for a bounded path set using the same last-match
/// semantics as Lens evidence. This pure projection is shared by temporal
/// analysis so base and head ownership are compared with identical syntax.
#[cfg(test)]
pub(crate) fn resolve_codeowners_bytes(
    bytes: &[u8],
    paths: &[String],
) -> Result<CodeownersResolution, &'static str> {
    resolve_codeowners_bytes_controlled(bytes, paths, &|| false)
}

pub(crate) fn resolve_codeowners_bytes_controlled(
    bytes: &[u8],
    paths: &[String],
    interrupted: &dyn Fn() -> bool,
) -> Result<CodeownersResolution, &'static str> {
    const MATCH_WORK_LIMIT: usize = 5_000_000;
    if bytes.len() as u64 >= MAX_CODEOWNERS_BYTES {
        return Err("codeowners_too_large");
    }
    let text = std::str::from_utf8(strip_utf8_bom(bytes)).map_err(|_| "invalid_utf8")?;
    let mut rules = Vec::new();
    let mut partial = false;
    let mut diagnostics = Vec::new();
    let mut diagnostics_truncated = false;
    for (line_index, line) in text.lines().enumerate() {
        if interrupted() {
            return Err("work_interrupted");
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if rules.len() >= MAX_CODEOWNER_RULES {
            partial = true;
            diagnostics.push("codeowner_rule_limit".to_string());
            break;
        }
        match parse_owner_rule(trimmed) {
            Ok(rule) => {
                if rule.owners_truncated {
                    partial = true;
                    if diagnostics.len() < MAX_DIAGNOSTICS.saturating_sub(1) {
                        diagnostics.push(format!("codeowner_owner_limit:{}", line_index + 1));
                    } else {
                        diagnostics_truncated = true;
                    }
                }
                rules.push(rule);
            }
            Err(()) => {
                partial = true;
                if diagnostics.len() < MAX_DIAGNOSTICS.saturating_sub(1) {
                    diagnostics.push(format!("invalid_codeowner_pattern:{}", line_index + 1));
                } else {
                    diagnostics_truncated = true;
                }
            }
        }
    }
    let mut owners_by_path = BTreeMap::new();
    let mut operations = 0usize;
    'paths: for path in paths {
        let mut owners = None;
        for rule in &rules {
            operations = operations.saturating_add(1);
            if operations.is_multiple_of(1_024) && interrupted() {
                return Err("work_interrupted");
            }
            if operations > MATCH_WORK_LIMIT {
                partial = true;
                if diagnostics.len() < MAX_DIAGNOSTICS.saturating_sub(1) {
                    diagnostics.push("codeowner_match_work_limit".to_string());
                } else {
                    diagnostics_truncated = true;
                }
                break 'paths;
            }
            if rule.matches(path) {
                owners = Some(rule.owners.clone());
            }
        }
        if let Some(owners) = owners {
            owners_by_path.insert(path.clone(), owners);
        }
    }
    if diagnostics_truncated {
        diagnostics.truncate(MAX_DIAGNOSTICS.saturating_sub(1));
        diagnostics.push("codeowner_diagnostic_limit".to_string());
    }
    diagnostics.sort();
    diagnostics.dedup();
    Ok(CodeownersResolution {
        owners_by_path,
        partial,
        diagnostics_truncated,
        diagnostics,
    })
}

impl OwnerRule {
    fn matches(&self, path: &str) -> bool {
        self.matchers.iter().any(|matcher| matcher.is_match(path))
    }
}

fn parse_owner_rule(line: &str) -> Result<OwnerRule, ()> {
    let mut fields = line.split_whitespace();
    let pattern = fields.next().ok_or(())?;
    if pattern.starts_with('!')
        || pattern.contains('[')
        || pattern.contains('{')
        || pattern.contains('}')
        || pattern.contains("\\#")
    {
        return Err(());
    }
    let owner_fields = fields
        .take_while(|value| !value.starts_with('#'))
        .collect::<Vec<_>>();
    if owner_fields
        .iter()
        .any(|value| !(value.starts_with('@') || value.contains('@')))
    {
        return Err(());
    }
    let mut owners = owner_fields
        .into_iter()
        .map(|value| truncate_text(value, 160))
        .collect::<Vec<_>>();
    let owners_truncated = owners.len() > MAX_OWNERS_PER_RULE;
    owners.truncate(MAX_OWNERS_PER_RULE);
    let pattern = pattern.trim_start_matches('/');
    if pattern.is_empty() {
        return Err(());
    }
    let directory = pattern.ends_with('/');
    let pattern = pattern.trim_end_matches('/');
    let final_segment_has_magic = pattern
        .rsplit('/')
        .next()
        .is_some_and(|segment| segment.contains('*') || segment.contains('?'));
    let matches_directory_contents = directory || !final_segment_has_magic;
    let mut candidates = BTreeSet::new();
    if pattern.contains('/') {
        candidates.insert(pattern.to_string());
        if matches_directory_contents {
            candidates.insert(format!("{pattern}/**"));
        }
    } else {
        candidates.insert(pattern.to_string());
        candidates.insert(format!("**/{pattern}"));
        if matches_directory_contents {
            candidates.insert(format!("{pattern}/**"));
            candidates.insert(format!("**/{pattern}/**"));
        }
    }
    let mut matchers = Vec::new();
    for candidate in candidates {
        let mut builder = GlobBuilder::new(&candidate);
        builder.literal_separator(true).backslash_escape(false);
        matchers.push(builder.build().map_err(|_| ())?.compile_matcher());
    }
    Ok(OwnerRule {
        matchers,
        owners,
        owners_truncated,
    })
}

fn trim_ascii_whitespace_start(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    value
}

fn parse_numstat(value: &[u8]) -> u64 {
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

fn sanitize_identity(value: &str) -> String {
    truncate_text(
        &value
            .chars()
            .filter(|character| !character.is_control())
            .collect::<String>(),
        120,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn collector<'a>(root: &'a Path, paths: &[&str]) -> Collector<'a> {
        Collector {
            root,
            relevant: paths.iter().map(|path| (*path).to_string()).collect(),
            sources: Vec::new(),
            artifact_identities: HashMap::new(),
            files: BTreeMap::new(),
            diagnostics: Vec::new(),
            notes: Vec::new(),
            diagnostics_truncated: false,
            sources_truncated: false,
            partial: false,
            finding_count: 0,
            coverage_line_count: 0,
            test_case_count: 0,
            test_failure_count: 0,
            runtime_span_count: 0,
            runtime_edges: BTreeMap::new(),
            runtime_edges_truncated: false,
            fact_artifacts: Vec::new(),
            fact_artifacts_truncated: false,
            fact_relationships: Vec::new(),
            fact_relationships_truncated: false,
            knowledge_match_count: 0,
            deadline: None,
        }
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn sarif_keeps_only_exact_trace_files_and_preserves_provenance() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/pay.rs"), "fn pay() {}\n").unwrap();
        let sarif = serde_json::json!({
            "version": "2.1.0",
            "runs": [{
                "tool": {"driver": {
                    "name": "Semgrep",
                    "rules": [{
                        "id": "payments.open-redirect",
                        "defaultConfiguration": {"level": "note"}
                    }]
                }},
                "results": [
                    {
                        "ruleIndex": 0,
                        "message": {"text": "Unsafe redirect"},
                        "locations": [{"physicalLocation": {
                            "artifactLocation": {"uri": "src/pay.rs"},
                            "region": {"startLine": 7, "startColumn": 4}
                        }}]
                    },
                    {
                        "ruleId": "unrelated",
                        "message": {"text": "Outside the returned trace"},
                        "locations": [{"physicalLocation": {
                            "artifactLocation": {"uri": "src/other.rs"}
                        }}]
                    }
                ]
            }]
        });
        fs::write(
            root.path().join("findings.sarif"),
            serde_json::to_vec(&sarif).unwrap(),
        )
        .unwrap();

        let mut collector = collector(root.path(), &["src/pay.rs"]);
        collector.load_sarif(Path::new("findings.sarif"), "sarif:0".into());
        let snapshot = collector.finish(0);

        assert!(!snapshot.partial);
        let artifact = fs::read(root.path().join("findings.sarif")).unwrap();
        let artifact_digest = crate::hex::encode(&Sha256::digest(&artifact));
        assert_eq!(
            snapshot.sources.items[0].artifact_sha256.as_deref(),
            Some(artifact_digest.as_str())
        );
        assert_eq!(
            snapshot.sources.items[0].artifact_bytes,
            Some(artifact.len() as u64)
        );
        assert_eq!(snapshot.sources.items[0].facts_total, Some(2));
        assert_eq!(snapshot.sources.items[0].facts_returned, 1);
        assert_eq!(snapshot.files.items.len(), 1);
        let finding = &snapshot.files.items[0].findings[0];
        assert_eq!(finding.tool, "Semgrep");
        assert_eq!(finding.rule_id, "payments.open-redirect");
        assert_eq!(finding.level, "note");
        assert_eq!(finding.line, Some(7));
        assert_eq!(finding.column, Some(4));
    }

    #[test]
    fn lcov_and_cobertura_merge_line_hits_without_inventing_files() {
        let root = tempfile::tempdir().unwrap();
        let relevant = BTreeSet::from(["src/pay.rs".to_string()]);
        let lcov = b"TN:\nSF:src/pay.rs\nDA:1,0\nDA:2,3\nDA:2,4\nend_of_record\nDA:3,9\nSF:src/other.rs\nDA:1,9\n";
        let (lcov_records, lcov_stats) = parse_lcov(root.path(), &relevant, lcov, None).unwrap();
        assert_eq!(lcov_stats.facts_total, 5);
        assert_eq!(lcov_records["src/pay.rs"].len(), 2);
        assert_eq!(lcov_records["src/pay.rs"][&2], 4);

        let cobertura = br#"<?xml version="1.0"?>
          <coverage><sources><source>.</source></sources><packages><package><classes>
            <class filename="src/pay.rs"><lines>
              <line number="1" hits="2"/><line number="3" hits="0"/>
            </lines></class>
            <class filename="src/other.rs"><lines><line number="1" hits="1"/></lines></class>
          </classes></package></packages></coverage>"#;
        let (xml_records, xml_stats) =
            parse_cobertura(root.path(), &relevant, cobertura, None).unwrap();
        assert_eq!(xml_stats.facts_total, 3);
        assert_eq!(xml_records["src/pay.rs"].len(), 2);
        assert_eq!(xml_records["src/pay.rs"][&1], 2);
        assert!(!xml_records.contains_key("src/other.rs"));
    }

    #[test]
    fn junit_matches_only_explicit_trace_files_and_preserves_failures() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("junit.xml"),
            r#"<?xml version="1.0"?>
            <testsuites><testsuite name="payments">
              <testcase name="accepts_card" classname="payments.CardTests" file="tests/pay_test.rs" time="0.125" />
              <testcase name="rejects_expired" classname="payments.CardTests" file="tests/pay_test.rs" time="0.250">
                <failure message="expected decline"><![CDATA[stack <trace>]]></failure>
              </testcase>
              <testcase name="unmapped" classname="payments.GuessedTests" time="0.010" />
            </testsuite></testsuites>"#,
        )
        .unwrap();

        let mut collector = collector(root.path(), &["tests/pay_test.rs"]);
        collector.load_junit(Path::new("junit.xml"), "junit:0".into());
        let snapshot = collector.finish(0);

        assert_eq!(snapshot.sources.items[0].facts_total, Some(3));
        assert_eq!(snapshot.sources.items[0].facts_returned, 2);
        let tests = snapshot.files.items[0].test_results.as_ref().unwrap();
        assert_eq!(tests.total, 2);
        assert_eq!(tests.passed, 1);
        assert_eq!(tests.failed, 1);
        assert_eq!(tests.errors, 0);
        assert_eq!(tests.skipped, 0);
        assert_eq!(tests.duration_ms, 375);
        assert_eq!(tests.failures[0].name, "rejects_expired");
        assert_eq!(
            tests.failures[0].message,
            "expected decline · stack <trace>"
        );
    }

    #[test]
    fn otlp_json_maps_code_paths_and_preserves_parent_child_runtime_evidence() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("traces.json"),
            serde_json::to_vec(&serde_json::json!({
                "resourceSpans": [{"scopeSpans": [{"spans": [
                    {
                        "traceId": "00112233445566778899aabbccddeeff",
                        "spanId": "0011223344556677",
                        "name": "checkout",
                        "attributes": [{"key": "code.file.path", "value": {"stringValue": "src/caller.rs"}}]
                    },
                    {
                        "traceId": "00112233445566778899aabbccddeeff",
                        "spanId": "8899aabbccddeeff",
                        "parentSpanId": "0011223344556677",
                        "name": "charge",
                        "attributes": [{"key": "code.file.path", "value": {"stringValue": "src/pay.rs"}}]
                    },
                    {
                        "traceId": "ffeeddccbbaa99887766554433221100",
                        "spanId": "ffeeddccbbaa9988",
                        "name": "unmapped",
                        "attributes": []
                    }
                ]}]}]
            }))
            .unwrap(),
        )
        .unwrap();

        let mut collector = collector(root.path(), &["src/caller.rs", "src/pay.rs"]);
        collector.load_otel(Path::new("traces.json"), "otel:0".into());
        let snapshot = collector.finish(0);

        assert_eq!(snapshot.sources.items[0].facts_total, Some(3));
        assert_eq!(snapshot.sources.items[0].facts_returned, 2);
        assert_eq!(snapshot.runtime_edges.returned, 1);
        let runtime = &snapshot.runtime_edges.items[0];
        assert_eq!(runtime.parent_file, "src/caller.rs");
        assert_eq!(runtime.child_file, "src/pay.rs");
        assert_eq!(runtime.spans, 1);
        assert_eq!(runtime.traces, 1);
        let pay = snapshot
            .files
            .items
            .iter()
            .find(|item| item.path == "src/pay.rs")
            .unwrap();
        assert_eq!(pay.runtime.as_ref().unwrap().spans, 1);
        assert_eq!(pay.runtime.as_ref().unwrap().traces, 1);
    }

    #[test]
    fn project_knowledge_requires_an_exact_trace_path_mention() {
        let root = tempfile::tempdir().unwrap();
        let entries = vec![
            crate::store::ProjectHistoryEntry {
                path: ".mastermind/tasks/101-payment/spec.md".into(),
                kind: "task_spec".into(),
                title: "Payment boundary".into(),
                body: "Route changes in `src/pay.rs` through the payment boundary.".into(),
            },
            crate::store::ProjectHistoryEntry {
                path: ".mastermind/tasks/_lessons.md".into(),
                kind: "lesson".into(),
                title: "Lessons".into(),
                body: "Do not treat src/pay.rs.bak as production evidence.".into(),
            },
            crate::store::ProjectHistoryEntry {
                path: ".mastermind/tasks/101-payment/audit.md".into(),
                kind: "audit".into(),
                title: "Payment audit".into(),
                body: "Verified `src\\pay.rs` against the runtime contract.".into(),
            },
            crate::store::ProjectHistoryEntry {
                path: "docs/adr/004-storage.md".into(),
                kind: "architecture_decision".into(),
                title: "Storage boundary".into(),
                body: "This decision covers src/storage.rs only.".into(),
            },
        ];

        let mut collector = collector(root.path(), &["src/pay.rs"]);
        collector.load_project_knowledge(&entries, 0, false, "project-knowledge".into());
        let snapshot = collector.finish(0);

        assert_eq!(snapshot.sources.items[0].facts_total, Some(2));
        assert_eq!(snapshot.sources.items[0].facts_returned, 2);
        assert!(
            snapshot.sources.items[0].facts_returned
                <= snapshot.sources.items[0].facts_total.unwrap()
        );
        let knowledge = &snapshot.files.items[0].knowledge;
        assert_eq!(knowledge.len(), 2);
        assert_eq!(knowledge[0].kind, "audit");
        assert_eq!(knowledge[1].kind, "task_spec");
        assert!(knowledge.iter().all(|item| item.match_kind == "exact_path"));
        assert!(!knowledge
            .iter()
            .any(|item| item.artifact_path == ".mastermind/tasks/_lessons.md"));
    }

    #[test]
    fn codeowners_uses_last_match_and_reports_unsupported_rules() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".github")).unwrap();
        fs::write(
            root.path().join(".github/CODEOWNERS"),
            "* @platform\n/src/** @payments\n/src/pay.rs @security @payments\n**/logs @operations\n/src/open.rs\n!docs/ @invalid\n",
        )
        .unwrap();

        let mut collector = collector(
            root.path(),
            &[
                "src/pay.rs",
                "src/open.rs",
                "tests/pay.rs",
                "deep/logs/errors.txt",
            ],
        );
        collector.load_codeowners(Path::new(".github/CODEOWNERS"), "codeowners".into());
        let snapshot = collector.finish(0);

        assert!(snapshot.partial);
        assert!(snapshot
            .diagnostics
            .items
            .iter()
            .any(|item| item.code == "invalid_codeowner_pattern"));
        let pay = snapshot
            .files
            .items
            .iter()
            .find(|item| item.path == "src/pay.rs")
            .unwrap();
        assert_eq!(
            pay.ownership.as_ref().unwrap().codeowners,
            ["@security", "@payments"]
        );
        let test = snapshot
            .files
            .items
            .iter()
            .find(|item| item.path == "tests/pay.rs")
            .unwrap();
        assert_eq!(test.ownership.as_ref().unwrap().codeowners, ["@platform"]);
        let logs = snapshot
            .files
            .items
            .iter()
            .find(|item| item.path == "deep/logs/errors.txt")
            .unwrap();
        assert_eq!(logs.ownership.as_ref().unwrap().codeowners, ["@operations"]);
        let explicitly_unowned = snapshot
            .files
            .items
            .iter()
            .find(|item| item.path == "src/open.rs")
            .unwrap();
        assert_eq!(
            explicitly_unowned
                .ownership
                .as_ref()
                .unwrap()
                .codeowners_source_id
                .as_deref(),
            Some("codeowners")
        );
        assert!(explicitly_unowned
            .ownership
            .as_ref()
            .unwrap()
            .codeowners
            .is_empty());
    }

    #[test]
    fn git_history_is_bounded_to_trace_paths_and_exposes_names_not_emails() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/pay.rs"), "one\n").unwrap();
        fs::write(root.path().join("src/other.rs"), "other\n").unwrap();
        git(root.path(), &["init", "-q"]);
        git(root.path(), &["config", "user.email", "one@example.test"]);
        git(root.path(), &["config", "user.name", "First Author"]);
        git(root.path(), &["add", "."]);
        git(root.path(), &["commit", "-qm", "first"]);
        fs::write(root.path().join("src/pay.rs"), "one\ntwo\nthree\n").unwrap();
        git(root.path(), &["config", "user.email", "two@example.test"]);
        git(root.path(), &["config", "user.name", "Second Author"]);
        git(root.path(), &["add", "src/pay.rs"]);
        git(root.path(), &["commit", "-qm", "second"]);

        let mut collector = collector(root.path(), &["src/pay.rs"]);
        let head_oid = crate::diff::current_head_oid(root.path()).unwrap();
        fs::write(root.path().join("src/pay.rs"), "one\ntwo\nthree\nfour\n").unwrap();
        git(root.path(), &["config", "user.name", "Later Author"]);
        git(root.path(), &["add", "src/pay.rs"]);
        git(root.path(), &["commit", "-qm", "later"]);

        collector.load_git_history(10, "git-history".into(), &head_oid);
        let snapshot = collector.finish(10);
        assert_eq!(snapshot.sources.items[0].facts_total, Some(2));
        let file = &snapshot.files.items[0];
        let churn = file.churn.as_ref().unwrap();
        assert_eq!(churn.commits, 2);
        assert_eq!(churn.lines_added, 3);
        let contributors = &file.ownership.as_ref().unwrap().contributors;
        assert_eq!(contributors.len(), 2);
        assert!(contributors.iter().any(|item| item.name == "First Author"));
        assert!(contributors.iter().any(|item| item.name == "Second Author"));
        assert!(!contributors.iter().any(|item| item.name == "Later Author"));
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("example.test"));
        assert!(!json.contains("src/other.rs"));
    }

    #[test]
    fn evidence_loaders_do_not_modify_sources_or_create_sidecars() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("coverage.info"), "SF:src/pay.rs\nDA:1,1\n").unwrap();
        let before = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect::<Vec<_>>();

        let mut collector = collector(root.path(), &["src/pay.rs"]);
        collector.load_coverage(Path::new("coverage.info"), "coverage:0".into());
        let _ = collector.finish(0);

        let after = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect::<Vec<_>>();
        assert_eq!(after, before);
    }

    #[test]
    fn evidence_paths_decode_repo_paths_but_reject_traversal_and_remote_uris() {
        let root = tempfile::tempdir().unwrap();
        let relevant = BTreeSet::from(["src/pay me.rs".to_string()]);
        assert_eq!(
            normalize_evidence_path(root.path(), "src/pay%20me.rs", &relevant),
            Some("src/pay me.rs".into())
        );
        assert_eq!(
            normalize_evidence_path(root.path(), "../src/pay%20me.rs", &relevant),
            None
        );
        assert_eq!(
            normalize_evidence_path(
                root.path(),
                "https://reports.example/src/pay%20me.rs",
                &relevant,
            ),
            None
        );
        #[cfg(unix)]
        assert_eq!(
            normalize_evidence_path(
                root.path(),
                &format!("file://{}", root.path().join("src/pay me.rs").display()),
                &relevant,
            ),
            Some("src/pay me.rs".into())
        );
    }

    #[test]
    fn missing_and_wrong_version_sources_are_explicit_partial_errors() {
        let root = tempfile::tempdir().unwrap();
        let mut collector = collector(root.path(), &["src/pay.rs"]);
        collector.load_sarif(Path::new("missing.sarif"), "sarif:0".into());
        fs::write(
            root.path().join("old.sarif"),
            r#"{"version":"1.0.0","runs":[]}"#,
        )
        .unwrap();
        collector.load_sarif(Path::new("old.sarif"), "sarif:1".into());
        let snapshot = collector.finish(0);

        assert!(snapshot.partial);
        assert_eq!(snapshot.sources.items.len(), 2);
        assert!(snapshot
            .sources
            .items
            .iter()
            .all(|source| source.status == "error"));
        assert_eq!(snapshot.diagnostics.items.len(), 2);
        assert_eq!(snapshot.diagnostics.items[0].code, "source_unavailable");
        assert_eq!(snapshot.diagnostics.items[1].code, "invalid_format");
    }

    #[test]
    fn codeowners_discovery_uses_github_priority_and_exact_case() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".GitHub")).unwrap();
        fs::write(root.path().join(".GitHub/CODEOWNERS"), "* @wrong-case\n").unwrap();
        fs::write(root.path().join("CODEOWNERS"), "* @root\n").unwrap();
        assert_eq!(
            discover_codeowners(root.path()).unwrap(),
            root.path().join("CODEOWNERS")
        );

        fs::remove_file(root.path().join(".GitHub/CODEOWNERS")).unwrap();
        fs::remove_dir(root.path().join(".GitHub")).unwrap();
        fs::create_dir(root.path().join(".github")).unwrap();
        fs::write(root.path().join(".github/CODEOWNERS"), "* @github\n").unwrap();
        assert_eq!(
            discover_codeowners(root.path()).unwrap(),
            root.path().join(".github/CODEOWNERS")
        );
    }

    #[test]
    fn codeowners_at_githubs_three_megabyte_limit_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let pattern = b"* @owner\n";
        let mut bytes = Vec::with_capacity(3 * 1024 * 1024);
        while bytes.len() < 3 * 1024 * 1024 {
            bytes.extend_from_slice(pattern);
        }
        bytes.truncate(3 * 1024 * 1024);
        fs::write(root.path().join("CODEOWNERS"), bytes).unwrap();

        let mut collector = collector(root.path(), &["src/pay.rs"]);
        collector.load_codeowners(Path::new("CODEOWNERS"), "codeowners".into());
        let snapshot = collector.finish(0);

        assert!(snapshot.partial);
        assert_eq!(snapshot.sources.items[0].status, "error");
        assert_eq!(snapshot.diagnostics.items[0].code, "codeowners_too_large");
    }

    #[test]
    fn pure_codeowners_resolution_bounds_invalid_line_diagnostics() {
        let bytes = "!\n".repeat(MAX_CODEOWNER_RULES + 10_000);
        let resolution =
            resolve_codeowners_bytes(bytes.as_bytes(), &["src/pay.rs".to_string()]).unwrap();

        assert!(resolution.partial);
        assert!(resolution.diagnostics.len() <= MAX_DIAGNOSTICS);
        assert!(resolution
            .diagnostics
            .iter()
            .any(|value| value == "codeowner_diagnostic_limit"));
    }
}
