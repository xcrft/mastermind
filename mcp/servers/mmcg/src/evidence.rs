//! Read-only evidence overlays for Mastermind Lens.
//!
//! Evidence is deliberately ephemeral: reports and Git history are parsed into
//! the current Lens response and are never written to the codegraph database or
//! repository. Every source is bounded and failures remain visible as partial
//! diagnostics instead of being mistaken for an absence of evidence.

use crate::diff::{run_bounded_git_with_limit_until, WorkingTreeDiffError};
use crate::queries::ChangeImpactResponse;
use globset::{GlobBuilder, GlobMatcher};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Instant, SystemTime};

const MAX_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_CODEOWNERS_BYTES: u64 = 3 * 1024 * 1024;
const MAX_ARTIFACT_SOURCES: usize = 64;
const MAX_RELEVANT_FILES: usize = 1_000;
const MAX_FINDINGS: usize = 5_000;
const MAX_FINDINGS_PER_FILE: usize = 100;
const MAX_COVERAGE_LINES: usize = 500_000;
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

#[derive(Debug, Serialize)]
pub struct EvidenceSnapshot {
    pub schema_version: u32,
    pub partial: bool,
    pub sources: EvidenceCollection<EvidenceSource>,
    pub files: EvidenceCollection<FileEvidence>,
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
    deadline: Option<Instant>,
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
    let (relevant, relevant_truncated) = relevant_paths(impact);
    let sources_truncated =
        options.sarif.len().saturating_add(options.coverage.len()) > MAX_ARTIFACT_SOURCES;
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
        deadline,
    };

    if !options.sarif.is_empty() || !options.coverage.is_empty() {
        collector.notes.push(EvidencePrecisionNote {
            source_id: "lens",
            code: "artifact_path_relocation",
            message: "Artifact paths match repository-relative paths exactly when possible; a unique suffix match can relocate reports produced under a different build root.".into(),
        });
        collector.notes.push(EvidencePrecisionNote {
            source_id: "lens",
            code: "artifact_revision_unverified",
            message: "Lens preserves artifact provenance labels but cannot prove that a SARIF or coverage report was produced from the current Git revision.".into(),
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
            "Only the first 64 SARIF and coverage inputs were evaluated.",
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
        self.sources.push(EvidenceSource {
            id,
            kind,
            label,
            status: "error",
            facts_total: None,
            facts_returned: 0,
            files_matched: 0,
        });
    }

    fn source_done(&mut self, id: String, kind: &'static str, label: String, stats: SourceStats) {
        self.partial |= stats.partial;
        self.sources.push(EvidenceSource {
            id,
            kind,
            label,
            status: if stats.partial { "partial" } else { "loaded" },
            facts_total: (!stats.partial).then(|| saturating_u32(stats.facts_total)),
            facts_returned: saturating_u32(stats.facts_returned),
            files_matched: saturating_u32(stats.files.len()),
        });
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

    fn load_sarif(&mut self, path: &Path, id: String) {
        let fallback = requested_label(path);
        let input = match self.read_source(path) {
            Ok(input) => input,
            Err(error) => return self.source_error(id, "sarif", fallback, error),
        };
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

    fn load_codeowners(&mut self, path: &Path, id: String) {
        let fallback = requested_label(path);
        let input = match self.read_source(path) {
            Ok(input) => input,
            Err(error) => return self.source_error(id, "codeowners", fallback, error),
        };
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
                let has_evidence = !accumulator.findings.is_empty()
                    || coverage.is_some()
                    || ownership.is_some()
                    || churn.is_some();
                has_evidence.then_some(FileEvidence {
                    path,
                    findings: accumulator.findings,
                    coverage,
                    ownership,
                    churn,
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

fn discover_codeowners(root: &Path) -> Option<PathBuf> {
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
            files: BTreeMap::new(),
            diagnostics: Vec::new(),
            notes: Vec::new(),
            diagnostics_truncated: false,
            sources_truncated: false,
            partial: false,
            finding_count: 0,
            coverage_line_count: 0,
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
}
