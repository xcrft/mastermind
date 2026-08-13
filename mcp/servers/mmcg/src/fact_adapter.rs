//! Deterministic adapters from common evidence formats into
//! `mastermind-facts/v1` manifests.
//!
//! Adapters parse inert, bounded artifacts through the same readers used by
//! Lens, bind every emitted fact to the current index and Git revision, and
//! fail closed on parser truncation. They never execute producer code and
//! never write SQLite.

use crate::evidence::{AdapterEvidenceKind, EvidenceSnapshot};
use crate::facts::{self, FactFileRecord};
use crate::store::Store;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_INDEX_PATHS: usize = 250_000;
const ADAPTER_DEADLINE: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterFormat {
    Sarif,
    Coverage,
    Junit,
    Otel,
}

impl AdapterFormat {
    fn evidence_kind(self) -> AdapterEvidenceKind {
        match self {
            Self::Sarif => AdapterEvidenceKind::Sarif,
            Self::Coverage => AdapterEvidenceKind::Coverage,
            Self::Junit => AdapterEvidenceKind::Junit,
            Self::Otel => AdapterEvidenceKind::Otel,
        }
    }

    fn provenance(self) -> &'static str {
        match self {
            Self::Sarif => "sarif",
            Self::Coverage => "coverage",
            Self::Junit => "junit",
            Self::Otel => "otel",
        }
    }
}

#[derive(Debug)]
pub struct AdaptOptions<'a> {
    pub format: AdapterFormat,
    pub input: &'a Path,
    pub output: &'a Path,
    pub producer: &'a str,
    pub producer_version: &'a str,
    pub dataset: &'a str,
    pub root: &'a Path,
}

#[derive(Debug, Serialize)]
pub struct AdaptSummary {
    pub schema_version: u32,
    pub api_version: &'static str,
    pub format: &'static str,
    pub output: String,
    pub repository_identity: String,
    pub revision: String,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
    pub files: u32,
    pub annotations: u32,
    pub relationships: u32,
}

#[derive(Debug)]
pub enum AdapterError {
    Contract(String),
    Evidence(String),
    Io(String),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(message) => write!(formatter, "fact adapter contract error: {message}"),
            Self::Evidence(message) => write!(formatter, "fact adapter evidence error: {message}"),
            Self::Io(message) => write!(formatter, "fact adapter I/O error: {message}"),
        }
    }
}

impl std::error::Error for AdapterError {}

fn contract_error(error: impl fmt::Display) -> AdapterError {
    AdapterError::Contract(error.to_string())
}

fn truncate(value: impl AsRef<str>, maximum: usize) -> String {
    let mut chars = value.as_ref().chars();
    let prefix = chars.by_ref().take(maximum).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn contained_artifact(root: &Path, input: &Path) -> Result<(PathBuf, String), AdapterError> {
    let requested = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };
    let absolute = std::path::absolute(&requested)
        .map_err(|error| AdapterError::Io(format!("resolve input path: {error}")))?;
    let relative = absolute
        .strip_prefix(root)
        .map_err(|_| contract_error("input artifact must be inside the indexed repository"))?;
    let relative = facts::normalize_fact_path(&relative.to_string_lossy().replace('\\', "/"))
        .map_err(contract_error)?;
    let mut cursor = root.to_path_buf();
    for component in Path::new(&relative).components() {
        let Component::Normal(part) = component else {
            return Err(contract_error("input artifact has an unsafe path"));
        };
        cursor.push(part);
        let metadata = std::fs::symlink_metadata(&cursor)
            .map_err(|error| AdapterError::Io(format!("read input metadata: {error}")))?;
        if metadata.file_type().is_symlink() {
            return Err(contract_error("input artifact must not traverse a symlink"));
        }
    }
    if !std::fs::metadata(&cursor)
        .map_err(|error| AdapterError::Io(format!("read input metadata: {error}")))?
        .is_file()
    {
        return Err(contract_error("input artifact must be a regular file"));
    }
    Ok((cursor, relative))
}

fn output_identity(path: &Path) -> Result<PathBuf, AdapterError> {
    let absolute = std::path::absolute(path)
        .map_err(|error| AdapterError::Io(format!("resolve output path: {error}")))?;
    let parent = absolute
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .map_err(|error| AdapterError::Io(format!("resolve output directory: {error}")))?;
    let name = absolute
        .file_name()
        .ok_or_else(|| contract_error("output must name a file"))?;
    Ok(parent.join(name))
}

fn stable_fact_id(prefix: &str, fact: &Value) -> Result<String, AdapterError> {
    let canonical = crate::audit_bundle::canonical_json(fact)
        .map_err(|error| AdapterError::Contract(error.to_string()))?;
    Ok(format!(
        "adapter:{prefix}:sha256:{}",
        crate::hex::encode(&Sha256::digest(canonical))
    ))
}

fn with_id(prefix: &str, mut fact: Value) -> Result<Value, AdapterError> {
    let id = stable_fact_id(prefix, &fact)?;
    let object = fact
        .as_object_mut()
        .ok_or_else(|| contract_error("generated fact is not an object"))?;
    object.insert("id".into(), Value::String(id));
    Ok(fact)
}

fn severity(level: &str) -> &'static str {
    match level {
        "error" => "error",
        "warning" => "warning",
        _ => "info",
    }
}

fn convert_snapshot(
    format: AdapterFormat,
    snapshot: &EvidenceSnapshot,
) -> Result<(Vec<Value>, BTreeSet<String>), AdapterError> {
    if snapshot.partial
        || snapshot.sources.truncated
        || snapshot.files.truncated
        || snapshot.runtime_edges.truncated
        || snapshot.diagnostics.truncated
    {
        let codes = snapshot
            .diagnostics
            .items
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if codes.is_empty() {
            String::new()
        } else {
            format!(": {codes}")
        };
        return Err(AdapterError::Evidence(format!(
            "artifact parsing was partial or truncated{suffix}"
        )));
    }
    let [source] = snapshot.sources.items.as_slice() else {
        return Err(AdapterError::Evidence(
            "adapter expected exactly one evidence source".into(),
        ));
    };
    if source.status != "loaded" {
        return Err(AdapterError::Evidence(
            "evidence source could not be loaded completely".into(),
        ));
    }
    if source.facts_total != Some(source.facts_returned) {
        return Err(AdapterError::Evidence(format!(
            "{} parsed facts were not all mapped to indexed repository files",
            source.facts_total.unwrap_or(source.facts_returned)
        )));
    }

    let mut facts = Vec::new();
    let mut paths = BTreeSet::new();
    for file in &snapshot.files.items {
        match format {
            AdapterFormat::Sarif => {
                for finding in &file.findings {
                    let title = truncate(format!("{} · {}", finding.tool, finding.rule_id), 256);
                    let fact = json!({
                        "kind": "annotation",
                        "path": file.path,
                        "line": finding.line.unwrap_or(1),
                        "column": finding.column,
                        "severity": severity(&finding.level),
                        "category": "sarif.finding",
                        "title": title,
                        "message": truncate(&finding.message, 4096),
                    });
                    facts.push(with_id("sarif", fact)?);
                    paths.insert(file.path.clone());
                }
            }
            AdapterFormat::Coverage => {
                if let Some(coverage) = &file.coverage {
                    let missing = coverage.lines_found.saturating_sub(coverage.lines_hit);
                    let fact = json!({
                        "kind": "annotation",
                        "path": file.path,
                        "line": 1,
                        "severity": if missing == 0 { "info" } else { "warning" },
                        "category": "coverage.file",
                        "title": "Coverage summary",
                        "message": format!(
                            "{} of {} executable lines covered; {} uncovered.",
                            coverage.lines_hit, coverage.lines_found, missing
                        ),
                    });
                    facts.push(with_id("coverage", fact)?);
                    paths.insert(file.path.clone());
                }
            }
            AdapterFormat::Junit => {
                if let Some(results) = &file.test_results {
                    let unsuccessful = results.failed.saturating_add(results.errors);
                    let fact = json!({
                        "kind": "annotation",
                        "path": file.path,
                        "line": 1,
                        "severity": if unsuccessful == 0 { "info" } else { "error" },
                        "category": "test.junit",
                        "title": "JUnit test summary",
                        "message": format!(
                            "{} tests: {} passed, {} failed, {} errors, {} skipped ({} ms).",
                            results.total,
                            results.passed,
                            results.failed,
                            results.errors,
                            results.skipped,
                            results.duration_ms
                        ),
                    });
                    facts.push(with_id("junit-summary", fact)?);
                    for failure in &results.failures {
                        let fact = json!({
                            "kind": "annotation",
                            "path": file.path,
                            "line": 1,
                            "severity": "error",
                            "category": "test.junit.failure",
                            "title": truncate(
                                failure.class_name.as_ref().map_or_else(
                                    || failure.name.clone(),
                                    |class_name| format!("{class_name} · {}", failure.name),
                                ),
                                256,
                            ),
                            "message": truncate(
                                format!("{}: {}", failure.status, failure.message),
                                4096,
                            ),
                        });
                        facts.push(with_id("junit-failure", fact)?);
                    }
                    paths.insert(file.path.clone());
                }
            }
            AdapterFormat::Otel => {
                if let Some(runtime) = &file.runtime {
                    let fact = json!({
                        "kind": "annotation",
                        "path": file.path,
                        "line": 1,
                        "severity": "info",
                        "category": "runtime.otel",
                        "title": "Observed runtime activity",
                        "message": format!(
                            "Observed {} spans across {} traces.",
                            runtime.spans, runtime.traces
                        ),
                    });
                    facts.push(with_id("otel-file", fact)?);
                    paths.insert(file.path.clone());
                }
            }
        }
    }
    if format == AdapterFormat::Otel {
        for edge in &snapshot.runtime_edges.items {
            let label = if edge.span_names.is_empty() {
                format!(
                    "Observed {} spans across {} traces",
                    edge.spans, edge.traces
                )
            } else {
                format!(
                    "Observed {} spans across {} traces: {}",
                    edge.spans,
                    edge.traces,
                    edge.span_names.join(", ")
                )
            };
            let fact = json!({
                "kind": "relationship",
                "relation": "runtime_parent_child",
                "from": {"path": edge.parent_file, "line": 1},
                "to": {"path": edge.child_file, "line": 1},
                "confidence": "observed",
                "label": truncate(label, 512),
            });
            facts.push(with_id("otel-edge", fact)?);
            paths.insert(edge.parent_file.clone());
            paths.insert(edge.child_file.clone());
        }
    }
    // Third-party reports may repeat an identical result across runs or test
    // suites. The fact contract requires unique IDs, so collapse exact
    // duplicates by their content-derived ID instead of rejecting an otherwise
    // valid artifact. A hash collision with different content fails closed.
    let mut unique = BTreeMap::<String, Value>::new();
    for fact in facts {
        let id = fact
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| contract_error("generated fact is missing its stable ID"))?
            .to_string();
        if let Some(previous) = unique.get(&id) {
            if previous != &fact {
                return Err(contract_error("generated fact ID collision"));
            }
            continue;
        }
        unique.insert(id, fact);
    }
    Ok((unique.into_values().collect(), paths))
}

fn file_json(binding: FactFileRecord) -> Value {
    json!({
        "path": binding.path,
        "sha256": binding.sha256,
        "bytes": binding.bytes,
    })
}

pub fn adapt(store: &Store, options: &AdaptOptions<'_>) -> Result<AdaptSummary, AdapterError> {
    let root = options
        .root
        .canonicalize()
        .map_err(|error| AdapterError::Io(format!("resolve repository root: {error}")))?;
    let indexed_root = facts::indexed_root(store).map_err(contract_error)?;
    if root != indexed_root {
        return Err(contract_error(
            "--root must identify the repository bound to the selected index",
        ));
    }
    let contract = facts::contract(store).map_err(contract_error)?;
    let (input, input_relative) = contained_artifact(&root, options.input)?;
    let output = output_identity(options.output)?;
    if output == input {
        return Err(contract_error(
            "output must not overwrite the input artifact",
        ));
    }

    let (indexed_paths, paths_truncated) = store
        .indexed_paths_bounded(MAX_INDEX_PATHS)
        .map_err(|error| AdapterError::Contract(error.to_string()))?;
    if paths_truncated {
        return Err(contract_error(format!(
            "indexed path inventory exceeds the {MAX_INDEX_PATHS}-file adapter limit"
        )));
    }
    let snapshot = crate::evidence::collect_for_fact_adapter(
        &root,
        options.format.evidence_kind(),
        &input,
        indexed_paths.into_iter().collect(),
        Some(Instant::now() + ADAPTER_DEADLINE),
    );
    let (facts, fact_paths) = convert_snapshot(options.format, &snapshot)?;
    let mut bindings = BTreeMap::new();
    for path in fact_paths {
        let binding = facts::indexed_file_binding(store, &root, &path).map_err(contract_error)?;
        bindings.insert(path, file_json(binding));
    }
    let source = snapshot
        .sources
        .items
        .first()
        .ok_or_else(|| AdapterError::Evidence("missing evidence source".into()))?;
    let artifact_sha256 = source
        .artifact_sha256
        .clone()
        .ok_or_else(|| AdapterError::Evidence("missing artifact digest".into()))?;
    let artifact_bytes = source
        .artifact_bytes
        .ok_or_else(|| AdapterError::Evidence("missing artifact size".into()))?;
    let annotations = facts
        .iter()
        .filter(|fact| fact.get("kind").and_then(Value::as_str) == Some("annotation"))
        .count();
    let relationships = facts.len().saturating_sub(annotations);
    let mut capabilities = vec!["annotations"];
    if relationships > 0 {
        capabilities.push("relationships");
    }
    let manifest = json!({
        "api_version": facts::API_VERSION,
        "capabilities": capabilities,
        "repository": {
            "identity": contract.repository.identity,
            "revision": contract.repository.revision,
        },
        "producer": {
            "name": options.producer,
            "version": options.producer_version,
        },
        "dataset": options.dataset,
        "provenance": {
            "kind": options.format.provenance(),
            "artifacts": ["input"],
        },
        "files": bindings.into_values().collect::<Vec<_>>(),
        "artifacts": [{
            "id": "input",
            "path": input_relative,
            "sha256": artifact_sha256,
            "bytes": artifact_bytes,
        }],
        "facts": facts,
    });
    let mut bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| AdapterError::Io(error.to_string()))?;
    bytes.push(b'\n');
    facts::validate_generated_manifest(store, &bytes).map_err(contract_error)?;
    crate::audit_bundle::write_atomic(&output, &bytes, false)
        .map_err(|error| AdapterError::Io(error.to_string()))?;
    Ok(AdaptSummary {
        schema_version: 1,
        api_version: facts::API_VERSION,
        format: options.format.provenance(),
        output: output.to_string_lossy().into_owned(),
        repository_identity: contract.repository.identity,
        revision: contract.repository.revision,
        artifact_sha256,
        artifact_bytes,
        files: u32::try_from(manifest["files"].as_array().map_or(0, Vec::len)).unwrap_or(u32::MAX),
        annotations: u32::try_from(annotations).unwrap_or(u32::MAX),
        relationships: u32::try_from(relationships).unwrap_or(u32::MAX),
    })
}
