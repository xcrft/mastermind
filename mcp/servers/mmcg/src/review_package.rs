//! Deterministic, offline PR evidence packages built from the shared Lens snapshot.
//!
//! Export is fail-closed for stale repository/index state and for evidence that
//! changes while it is being packaged. External reports remain read-only. The
//! package manifest binds their exact bytes to the reviewed Git head; an
//! optional producer-side attestation can additionally bind those digests to
//! the same revision without changing the codegraph database.

use crate::evidence::{EvidenceExtensionOptions, EvidenceOptions, EvidencePrecisionNote};
use crate::lens::{LensError, LensOptions, LensSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

pub const REVIEW_PACKAGE_SCHEMA: u32 = 1;
pub const EVIDENCE_ATTESTATION_SCHEMA: u32 = 1;
const MAX_ATTESTATION_BYTES: u64 = 1024 * 1024;

const WORKFLOW_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/mastermind-review-pr.yml"
));

#[derive(Debug, Clone)]
pub struct ReviewExportOptions {
    pub root: PathBuf,
    pub index_path: PathBuf,
    pub out: PathBuf,
    pub lens: LensOptions,
    pub evidence: EvidenceOptions,
    pub extensions: EvidenceExtensionOptions,
    pub evidence_attestation: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct ReviewExportResult {
    pub output_dir: PathBuf,
    pub head_oid: String,
    pub partial: bool,
    pub artifacts: u32,
    pub evidence_binding: String,
}

#[derive(Debug)]
pub enum ReviewPackageError {
    Lens(LensError),
    OutputExists,
    OutputParentUnavailable,
    UnsafeOutput,
    EvidenceUnavailable(String),
    EvidenceTooLarge(String),
    EvidenceChanged(String),
    EvidenceBinding(String),
    InvalidAttestation(String),
    Serialization(String),
    Io(String),
}

impl fmt::Display for ReviewPackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lens(error) => write!(formatter, "review snapshot failed: {error}"),
            Self::OutputExists => formatter.write_str(
                "review output already exists; choose a new --out path or remove it explicitly",
            ),
            Self::OutputParentUnavailable => {
                formatter.write_str("review output parent must be an existing real directory")
            }
            Self::UnsafeOutput => formatter.write_str("review output path is unsafe"),
            Self::EvidenceUnavailable(label) => {
                write!(formatter, "review evidence is unavailable: {label}")
            }
            Self::EvidenceTooLarge(label) => {
                write!(formatter, "review evidence exceeds its read limit: {label}")
            }
            Self::EvidenceChanged(label) => {
                write!(formatter, "review evidence changed during export: {label}")
            }
            Self::EvidenceBinding(label) => {
                write!(formatter, "review evidence digest binding failed: {label}")
            }
            Self::InvalidAttestation(message) => {
                write!(formatter, "invalid review evidence attestation: {message}")
            }
            Self::Serialization(message) => {
                write!(formatter, "review package serialization failed: {message}")
            }
            Self::Io(message) => write!(formatter, "review package I/O failed: {message}"),
        }
    }
}

impl std::error::Error for ReviewPackageError {}

impl From<LensError> for ReviewPackageError {
    fn from(error: LensError) -> Self {
        Self::Lens(error)
    }
}

#[derive(Debug, Clone)]
struct SourceRequest {
    id: String,
    kind: &'static str,
    path: PathBuf,
    maximum_bytes: u64,
    retain_body: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceIdentity {
    id: String,
    kind: String,
    label: String,
    resolved: PathBuf,
    repository_relative: bool,
    sha256: String,
    bytes: u64,
    modified: Option<SystemTime>,
    body: Vec<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceAttestation {
    schema_version: u32,
    head_oid: String,
    artifacts: Vec<AttestedArtifact>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttestedArtifact {
    kind: String,
    path: String,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct ReviewManifest {
    schema_version: u32,
    package_format: &'static str,
    generator: GeneratorBinding,
    repository: RepositoryBinding,
    scope: ScopeBinding,
    analysis: AnalysisBinding,
    evidence_binding: EvidenceBinding,
    artifacts: Vec<ArtifactBinding>,
    content_sha256: String,
}

#[derive(Debug, Serialize)]
struct GeneratorBinding {
    name: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct RepositoryBinding {
    name: String,
    root_label: String,
    requested_ref: String,
    baseline_oid: String,
    head_oid: String,
    includes_worktree: bool,
    includes_untracked: bool,
    snapshot_token_sha256: String,
}

#[derive(Debug, Serialize)]
struct ScopeBinding {
    path: String,
    depth: u8,
    top: u32,
    production_only: bool,
}

#[derive(Debug, Serialize)]
struct AnalysisBinding {
    partial: bool,
    temporal_status: &'static str,
    semantic_available: bool,
    evidence_partial: bool,
    states: Vec<AnalysisState>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct AnalysisState {
    path: String,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct EvidenceBinding {
    status: String,
    head_oid: String,
    sources: Vec<EvidenceSourceBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attestation: Option<AttestationBinding>,
}

#[derive(Debug, Serialize)]
struct EvidenceSourceBinding {
    id: String,
    kind: String,
    label: String,
    repository_relative: bool,
    sha256: String,
    bytes: u64,
    analysis_status: &'static str,
    revision_binding: &'static str,
}

#[derive(Debug, Serialize)]
struct AttestationBinding {
    label: String,
    sha256: String,
    head_oid: String,
    artifacts: u32,
    trust: &'static str,
}

#[derive(Default)]
struct AttestationValidation {
    artifacts: BTreeSet<(String, String, String)>,
    binding: Option<AttestationBinding>,
}

#[derive(Debug, Clone, Serialize)]
struct ArtifactBinding {
    path: &'static str,
    media_type: &'static str,
    sha256: String,
    bytes: u64,
}

struct PackageDocument {
    path: &'static str,
    media_type: &'static str,
    body: Vec<u8>,
}

pub fn export(options: &ReviewExportOptions) -> Result<ReviewExportResult, ReviewPackageError> {
    let root = options
        .root
        .canonicalize()
        .map_err(|_| ReviewPackageError::Lens(LensError::RootUnavailable))?;
    let output_dir = output_target(&options.out)?;
    let requests = evidence_requests(&root, &options.evidence, &options.extensions);
    let before_sources = read_sources(&root, &requests)?;
    let before_attestation = options
        .evidence_attestation
        .as_ref()
        .map(|path| {
            read_source(
                &root,
                &SourceRequest {
                    id: "attestation".into(),
                    kind: "attestation",
                    path: path.clone(),
                    maximum_bytes: MAX_ATTESTATION_BYTES,
                    retain_body: true,
                },
            )
        })
        .transpose()?;

    let mut snapshot = crate::lens::snapshot_from_paths_with_evidence_extensions(
        &root,
        &options.index_path,
        &options.lens,
        &options.evidence,
        &options.extensions,
    )?;

    let after_sources = read_sources(&root, &requests)?;
    ensure_sources_unchanged(&before_sources, &after_sources)?;
    let after_attestation = options
        .evidence_attestation
        .as_ref()
        .map(|path| {
            read_source(
                &root,
                &SourceRequest {
                    id: "attestation".into(),
                    kind: "attestation",
                    path: path.clone(),
                    maximum_bytes: MAX_ATTESTATION_BYTES,
                    retain_body: true,
                },
            )
        })
        .transpose()?;
    match (&before_attestation, &after_attestation) {
        (Some(before), Some(after)) if before != after => {
            return Err(ReviewPackageError::EvidenceChanged(before.label.clone()));
        }
        (None, None) | (Some(_), Some(_)) => {}
        _ => return Err(ReviewPackageError::EvidenceChanged("attestation".into())),
    }

    let head_oid = snapshot.impact.baseline.head_oid.clone();
    let attestation = validate_attestation(after_attestation.as_ref(), &after_sources, &head_oid)?;
    if !after_sources.is_empty() {
        snapshot.evidence.precision_notes.push(EvidencePrecisionNote {
            source_id: "review-package",
            code: "artifact_digest_revision_binding",
            message: format!(
                "manifest.json binds the exact SHA-256 digest of every packaged external evidence source to Git head {head_oid}; this proves which bytes were reviewed, not when or by whom the report was produced."
            ),
        });
    }
    if attestation.binding.is_some() {
        snapshot.evidence.precision_notes.push(EvidencePrecisionNote {
            source_id: "review-package",
            code: "producer_revision_attested",
            message: "A producer-side evidence attestation matched the package Git head and exact artifact digests. The attestation is an unsigned CI fact unless the surrounding workflow supplies a stronger trust anchor.".into(),
        });
    }
    snapshot
        .evidence
        .precision_notes
        .sort_by(|left, right| (left.code, &left.message).cmp(&(right.code, &right.message)));

    let snapshot_value = serde_json::to_value(&snapshot)
        .map_err(|error| ReviewPackageError::Serialization(error.to_string()))?;
    let analysis = analysis_binding(&snapshot, &snapshot_value);
    let evidence_binding = evidence_binding(
        &snapshot,
        &after_sources,
        &attestation.artifacts,
        attestation.binding,
        &head_oid,
    )?;
    let html = crate::lens::standalone_html(&snapshot)?;
    let sarif = pretty_json(&crate::sarif_export::review_package(
        &snapshot.map,
        &snapshot.impact,
    ))?;
    let workflow = WORKFLOW_TEMPLATE.as_bytes().to_vec();
    let summary = summary_markdown(&snapshot, &analysis, &evidence_binding).into_bytes();
    let documents = vec![
        PackageDocument {
            path: "index.html",
            media_type: "text/html; charset=utf-8",
            body: html,
        },
        PackageDocument {
            path: "mastermind.sarif",
            media_type: "application/sarif+json",
            body: sarif,
        },
        PackageDocument {
            path: "summary.md",
            media_type: "text/markdown; charset=utf-8",
            body: summary,
        },
        PackageDocument {
            path: "mastermind-review.yml",
            media_type: "application/yaml",
            body: workflow,
        },
    ];
    let artifacts = documents
        .iter()
        .map(|document| ArtifactBinding {
            path: document.path,
            media_type: document.media_type,
            sha256: sha256_hex(&document.body),
            bytes: document.body.len() as u64,
        })
        .collect::<Vec<_>>();
    let content_sha256 = sha256_hex(
        &serde_json::to_vec(&artifacts)
            .map_err(|error| ReviewPackageError::Serialization(error.to_string()))?,
    );
    let manifest = ReviewManifest {
        schema_version: REVIEW_PACKAGE_SCHEMA,
        package_format: "mastermind-review",
        generator: GeneratorBinding {
            name: "Mastermind",
            version: env!("CARGO_PKG_VERSION"),
        },
        repository: RepositoryBinding {
            name: snapshot.repository.name.clone(),
            root_label: snapshot.repository.root_label.clone(),
            requested_ref: snapshot.impact.baseline.requested_ref.clone(),
            baseline_oid: snapshot.impact.baseline.baseline_oid.clone(),
            head_oid: head_oid.clone(),
            includes_worktree: snapshot.impact.baseline.includes_worktree,
            includes_untracked: snapshot.impact.baseline.includes_untracked,
            snapshot_token_sha256: sha256_hex(snapshot.impact.snapshot_token.as_bytes()),
        },
        scope: ScopeBinding {
            path: snapshot.options.path.clone(),
            depth: snapshot.options.depth,
            top: snapshot.options.top,
            production_only: snapshot.options.production_only,
        },
        analysis,
        evidence_binding,
        artifacts,
        content_sha256,
    };
    let manifest_body = pretty_json(
        &serde_json::to_value(&manifest)
            .map_err(|error| ReviewPackageError::Serialization(error.to_string()))?,
    )?;
    write_package(&output_dir, documents, &manifest_body)?;

    Ok(ReviewExportResult {
        output_dir,
        head_oid,
        partial: manifest.analysis.partial,
        artifacts: manifest.artifacts.len() as u32 + 1,
        evidence_binding: manifest.evidence_binding.status,
    })
}

fn evidence_requests(
    root: &Path,
    evidence: &EvidenceOptions,
    extensions: &EvidenceExtensionOptions,
) -> Vec<SourceRequest> {
    let mut requests = Vec::new();
    let mut artifact_count = 0_usize;
    for (kind, paths) in [
        ("sarif", evidence.sarif.as_slice()),
        ("coverage", evidence.coverage.as_slice()),
        ("junit", extensions.junit.as_slice()),
        ("otel", extensions.otel.as_slice()),
    ] {
        for (index, path) in paths.iter().enumerate() {
            if artifact_count >= crate::evidence::MAX_ARTIFACT_SOURCES {
                break;
            }
            requests.push(SourceRequest {
                id: format!("{kind}:{index}"),
                kind,
                path: path.clone(),
                maximum_bytes: crate::evidence::MAX_ARTIFACT_BYTES,
                retain_body: false,
            });
            artifact_count += 1;
        }
    }
    let codeowners = evidence.codeowners.clone().or_else(|| {
        evidence
            .discover_codeowners
            .then(|| crate::evidence::discover_codeowners(root))
            .flatten()
    });
    if let Some(path) = codeowners {
        requests.push(SourceRequest {
            id: "codeowners".into(),
            kind: "codeowners",
            path,
            maximum_bytes: crate::evidence::MAX_CODEOWNERS_BYTES,
            retain_body: false,
        });
    }
    requests
}

fn read_sources(
    root: &Path,
    requests: &[SourceRequest],
) -> Result<Vec<SourceIdentity>, ReviewPackageError> {
    requests
        .iter()
        .map(|request| read_source(root, request))
        .collect()
}

fn read_source(root: &Path, request: &SourceRequest) -> Result<SourceIdentity, ReviewPackageError> {
    let requested = if request.path.is_absolute() {
        request.path.clone()
    } else {
        root.join(&request.path)
    };
    let fallback_label = display_path(&request.path);
    let resolved = requested
        .canonicalize()
        .map_err(|_| ReviewPackageError::EvidenceUnavailable(fallback_label.clone()))?;
    let initial = std::fs::metadata(&resolved)
        .map_err(|_| ReviewPackageError::EvidenceUnavailable(fallback_label.clone()))?;
    if !initial.is_file() {
        return Err(ReviewPackageError::EvidenceUnavailable(fallback_label));
    }
    if initial.len() > request.maximum_bytes {
        return Err(ReviewPackageError::EvidenceTooLarge(fallback_label));
    }
    let mut file = File::open(&resolved)
        .map_err(|_| ReviewPackageError::EvidenceUnavailable(fallback_label.clone()))?;
    let before = file
        .metadata()
        .map_err(|_| ReviewPackageError::EvidenceUnavailable(fallback_label.clone()))?;
    if !before.is_file() || before.len() != initial.len() || modified(&before) != modified(&initial)
    {
        return Err(ReviewPackageError::EvidenceChanged(fallback_label));
    }
    let mut body = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
    Read::by_ref(&mut file)
        .take(request.maximum_bytes + 1)
        .read_to_end(&mut body)
        .map_err(|_| ReviewPackageError::EvidenceUnavailable(fallback_label.clone()))?;
    if body.len() as u64 > request.maximum_bytes {
        return Err(ReviewPackageError::EvidenceTooLarge(fallback_label));
    }
    let after = file
        .metadata()
        .map_err(|_| ReviewPackageError::EvidenceChanged(fallback_label.clone()))?;
    if after.len() != before.len() || modified(&after) != modified(&before) {
        return Err(ReviewPackageError::EvidenceChanged(fallback_label));
    }
    let (label, repository_relative) = match resolved.strip_prefix(root) {
        Ok(relative) => (display_path(relative), true),
        Err(_) => (
            resolved
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "evidence-artifact".into()),
            false,
        ),
    };
    let sha256 = sha256_hex(&body);
    let bytes = body.len() as u64;
    Ok(SourceIdentity {
        id: request.id.clone(),
        kind: request.kind.into(),
        label,
        resolved,
        repository_relative,
        sha256,
        bytes,
        modified: modified(&after),
        body: if request.retain_body {
            body
        } else {
            Vec::new()
        },
    })
}

fn ensure_sources_unchanged(
    before: &[SourceIdentity],
    after: &[SourceIdentity],
) -> Result<(), ReviewPackageError> {
    if before.len() != after.len() {
        return Err(ReviewPackageError::EvidenceChanged(
            "evidence source set".into(),
        ));
    }
    for (left, right) in before.iter().zip(after) {
        if left != right {
            return Err(ReviewPackageError::EvidenceChanged(left.label.clone()));
        }
    }
    Ok(())
}

fn validate_attestation(
    input: Option<&SourceIdentity>,
    sources: &[SourceIdentity],
    head_oid: &str,
) -> Result<AttestationValidation, ReviewPackageError> {
    let Some(input) = input else {
        return Ok(AttestationValidation::default());
    };
    let attestation: EvidenceAttestation = crate::audit_bundle::from_json_strict(&input.body)
        .map_err(|error| ReviewPackageError::InvalidAttestation(error.to_string()))?;
    if attestation.schema_version != EVIDENCE_ATTESTATION_SCHEMA {
        return Err(ReviewPackageError::InvalidAttestation(
            "unsupported schema_version".into(),
        ));
    }
    if !full_oid(&attestation.head_oid) || attestation.head_oid != head_oid {
        return Err(ReviewPackageError::InvalidAttestation(
            "head_oid does not match the review snapshot".into(),
        ));
    }
    if attestation.artifacts.len() > crate::evidence::MAX_ARTIFACT_SOURCES + 1 {
        return Err(ReviewPackageError::InvalidAttestation(
            "artifact list exceeds the review source limit".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut attested = BTreeSet::new();
    for artifact in &attestation.artifacts {
        if !matches!(
            artifact.kind.as_str(),
            "sarif" | "coverage" | "junit" | "otel" | "codeowners"
        ) {
            return Err(ReviewPackageError::InvalidAttestation(format!(
                "unsupported artifact kind {}",
                artifact.kind
            )));
        }
        let normalized = normalize_relative(&artifact.path).ok_or_else(|| {
            ReviewPackageError::InvalidAttestation(
                "artifact path must be canonical and repository-relative".into(),
            )
        })?;
        if normalized != artifact.path || !sha256_digest(&artifact.sha256) {
            return Err(ReviewPackageError::InvalidAttestation(
                "artifact path or sha256 is not canonical".into(),
            ));
        }
        if !seen.insert((artifact.kind.clone(), artifact.path.clone())) {
            return Err(ReviewPackageError::InvalidAttestation(
                "duplicate artifact identity".into(),
            ));
        }
        let matches = sources
            .iter()
            .filter(|source| {
                source.repository_relative
                    && source.kind == artifact.kind
                    && source.label == artifact.path
            })
            .collect::<Vec<_>>();
        if matches.is_empty()
            || matches
                .iter()
                .any(|source| source.sha256 != artifact.sha256)
        {
            return Err(ReviewPackageError::InvalidAttestation(format!(
                "artifact digest does not match {}:{}",
                artifact.kind, artifact.path
            )));
        }
        attested.insert((
            artifact.kind.clone(),
            artifact.path.clone(),
            artifact.sha256.clone(),
        ));
    }
    Ok(AttestationValidation {
        artifacts: attested,
        binding: Some(AttestationBinding {
            label: input.label.clone(),
            sha256: input.sha256.clone(),
            head_oid: attestation.head_oid,
            artifacts: attestation.artifacts.len() as u32,
            trust: "unsigned-digest-attestation",
        }),
    })
}

fn evidence_binding(
    snapshot: &LensSnapshot,
    sources: &[SourceIdentity],
    attested: &BTreeSet<(String, String, String)>,
    attestation: Option<AttestationBinding>,
    head_oid: &str,
) -> Result<EvidenceBinding, ReviewPackageError> {
    let bound_sources = sources
        .iter()
        .map(|source| {
            let observed = snapshot
                .evidence
                .sources
                .items
                .iter()
                .find(|candidate| candidate.id == source.id)
                .ok_or_else(|| ReviewPackageError::EvidenceBinding(source.id.clone()))?;
            if observed.kind != source.kind {
                return Err(ReviewPackageError::EvidenceBinding(source.id.clone()));
            }
            match observed.artifact_sha256.as_deref() {
                Some(digest)
                    if digest == source.sha256 && observed.artifact_bytes == Some(source.bytes) => {
                }
                None if observed.status == "error" => {}
                _ => return Err(ReviewPackageError::EvidenceBinding(source.id.clone())),
            }
            let producer_attested = attested.contains(&(
                source.kind.clone(),
                source.label.clone(),
                source.sha256.clone(),
            ));
            Ok(EvidenceSourceBinding {
                id: source.id.clone(),
                kind: source.kind.clone(),
                label: source.label.clone(),
                repository_relative: source.repository_relative,
                sha256: source.sha256.clone(),
                bytes: source.bytes,
                analysis_status: observed.status,
                revision_binding: if producer_attested {
                    "producer-attested"
                } else {
                    "digest-bound-at-export"
                },
            })
        })
        .collect::<Result<Vec<_>, ReviewPackageError>>()?;
    let attested_count = bound_sources
        .iter()
        .filter(|source| source.revision_binding == "producer-attested")
        .count();
    let status = if bound_sources.is_empty() {
        "not-applicable"
    } else if attested_count == bound_sources.len() {
        "producer-attested"
    } else if attested_count > 0 {
        "partially-producer-attested"
    } else {
        "digest-bound-at-export"
    };
    Ok(EvidenceBinding {
        status: status.into(),
        head_oid: head_oid.into(),
        sources: bound_sources,
        attestation,
    })
}

fn analysis_binding(snapshot: &LensSnapshot, value: &Value) -> AnalysisBinding {
    let mut states = BTreeSet::new();
    collect_analysis_states(value, "$", &mut states);
    if snapshot.temporal.status != "available" {
        states.insert(AnalysisState {
            path: "$.temporal".into(),
            state: "unavailable",
            reason: snapshot
                .temporal
                .diagnostic
                .as_ref()
                .map(|diagnostic| diagnostic.code.into()),
        });
    }
    let states = states.into_iter().collect::<Vec<_>>();
    AnalysisBinding {
        partial: !states.is_empty(),
        temporal_status: snapshot.temporal.status,
        semantic_available: snapshot.semantic.available,
        evidence_partial: snapshot.evidence.partial,
        states,
    }
}

fn collect_analysis_states(value: &Value, path: &str, states: &mut BTreeSet<AnalysisState>) {
    match value {
        Value::Object(object) => {
            let reason = object
                .get("truncation_reason")
                .and_then(Value::as_str)
                .map(str::to_string);
            for (key, child) in object {
                let state = if key == "partial" {
                    Some("partial")
                } else if key == "truncated" || key.ends_with("_truncated") {
                    Some("truncated")
                } else {
                    None
                };
                if child.as_bool() == Some(true) {
                    let Some(state) = state else {
                        continue;
                    };
                    states.insert(AnalysisState {
                        path: path.into(),
                        state,
                        reason: match key.as_str() {
                            "partial" => None,
                            "truncated" => reason.clone().or_else(|| Some("truncated".to_string())),
                            _ => Some(key.to_string()),
                        },
                    });
                }
            }
            for (key, child) in object {
                collect_analysis_states(child, &format!("{path}.{key}"), states);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_analysis_states(child, &format!("{path}[{index}]"), states);
            }
        }
        _ => {}
    }
}

fn summary_markdown(
    snapshot: &LensSnapshot,
    analysis: &AnalysisBinding,
    evidence: &EvidenceBinding,
) -> String {
    let impact = &snapshot.impact;
    let map = &snapshot.map;
    let revision_kind = if impact.baseline.includes_worktree || impact.baseline.includes_untracked {
        "commit plus working-tree state"
    } else {
        "clean commit"
    };
    let mut output = format!(
        "# Mastermind review\n\n[Open the autonomous Lens report](index.html) · [SARIF results](mastermind.sarif)\n\n- Repository: {}\n- Baseline: `{}` (`{}`)\n- Head: `{}` ({revision_kind})\n- Scope: `{}` · depth {} · top {}\n- Analysis: **{}**\n- Evidence binding: `{}`\n\n## Change summary\n\n- Changed files: {}\n- Changed symbols: {}\n- Impacted symbols: {}\n- Cross-component impacts: {}\n- Candidate tests: {}\n\n## Architecture snapshot\n\n- Indexed files in scope: {}\n- Components: {}\n- Dependency cycles: {}\n- Hotspots: {}\n",
        markdown_text(&snapshot.repository.name),
        markdown_text(&impact.baseline.requested_ref),
        short_oid(&impact.baseline.baseline_oid),
        short_oid(&impact.baseline.head_oid),
        markdown_text(&snapshot.options.path),
        snapshot.options.depth,
        snapshot.options.top,
        if analysis.partial { "partial" } else { "complete" },
        evidence.status,
        count_label(impact.changes.files.total, impact.changes.files.returned),
        count_label(impact.changes.symbols.total, impact.changes.symbols.returned),
        count_label(impact.impact.total, impact.impact.returned),
        count_label(impact.api_crossings.total, impact.api_crossings.returned),
        count_label(impact.tests.total, impact.tests.returned),
        count_label(map.files.total, map.files.returned),
        count_label(map.components.total, map.components.returned),
        count_label(map.cycles.total, map.cycles.returned),
        count_label(map.hotspots.total, map.hotspots.returned),
    );
    if let Some(temporal) = snapshot.temporal.data.as_ref() {
        let summary = &temporal.summary;
        output.push_str(&format!(
            "\n## Architecture drift\n\n- Components: +{} / -{}\n- Public boundaries: +{} / -{} / ~{}\n- Cycles: +{} / -{} / ~{}\n- Ownership changes: {}\n- History review candidates: {}\n",
            summary.components_added,
            summary.components_removed,
            summary.boundaries_added,
            summary.boundaries_removed,
            summary.boundaries_changed,
            summary.cycles_introduced,
            summary.cycles_resolved,
            summary.cycles_changed,
            summary.ownership_changes,
            summary.history_review_candidates,
        ));
    }
    output.push_str(&format!(
        "\n## Evidence and limits\n\n- External evidence inputs: {}\n- Lens evidence status: {}\n- Temporal status: `{}`\n- Bounded/partial states: {}\n",
        evidence.sources.len(),
        if snapshot.evidence.partial { "partial" } else { "complete" },
        snapshot.temporal.status,
        analysis.states.len(),
    ));
    for state in analysis.states.iter().take(8) {
        output.push_str(&format!(
            "  - `{}`: {}{}\n",
            markdown_text(&state.path),
            state.state,
            state
                .reason
                .as_ref()
                .map(|reason| format!(" ({})", markdown_text(reason)))
                .unwrap_or_default(),
        ));
    }
    if analysis.states.len() > 8 {
        output.push_str(&format!(
            "  - {} additional states are recorded in `manifest.json`.\n",
            analysis.states.len() - 8
        ));
    }
    output.push_str(
        "\n`manifest.json` records the exact Git OIDs, package payload digests, external evidence digests, and every returned partial/truncation state.\n",
    );
    output
}

fn output_target(path: &Path) -> Result<PathBuf, ReviewPackageError> {
    let name = path.file_name().ok_or(ReviewPackageError::UnsafeOutput)?;
    if name.is_empty() || matches!(path.components().next_back(), Some(Component::ParentDir)) {
        return Err(ReviewPackageError::UnsafeOutput);
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = std::fs::symlink_metadata(parent)
        .map_err(|_| ReviewPackageError::OutputParentUnavailable)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ReviewPackageError::OutputParentUnavailable);
    }
    let parent = parent
        .canonicalize()
        .map_err(|_| ReviewPackageError::OutputParentUnavailable)?;
    let target = parent.join(name);
    match std::fs::symlink_metadata(&target) {
        Ok(_) => Err(ReviewPackageError::OutputExists),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(target),
        Err(error) => Err(ReviewPackageError::Io(error.to_string())),
    }
}

fn write_package(
    target: &Path,
    documents: Vec<PackageDocument>,
    manifest: &[u8],
) -> Result<(), ReviewPackageError> {
    let parent = target
        .parent()
        .ok_or(ReviewPackageError::OutputParentUnavailable)?;
    let temporary = tempfile::Builder::new()
        .prefix(".mastermind-review-")
        .tempdir_in(parent)
        .map_err(|error| ReviewPackageError::Io(error.to_string()))?;
    for document in documents {
        write_new_file(&temporary.path().join(document.path), &document.body)?;
    }
    write_new_file(&temporary.path().join("manifest.json"), manifest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o755))
            .map_err(|error| ReviewPackageError::Io(error.to_string()))?;
        File::open(temporary.path())
            .and_then(|directory| directory.sync_all())
            .map_err(|error| ReviewPackageError::Io(error.to_string()))?;
    }
    match rename_package_noclobber(temporary.path(), target) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(ReviewPackageError::OutputExists);
        }
        Err(error) => return Err(ReviewPackageError::Io(error.to_string())),
    }
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ReviewPackageError::Io(error.to_string()))?;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn rename_package_noclobber(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in source path"))?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in target path"))?;
    // SAFETY: both arguments are live NUL-terminated path buffers. RENAME_EXCL
    // gives the directory publication the same no-clobber contract as create_new.
    let status = unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_package_noclobber(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in source path"))?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in target path"))?;
    // SAFETY: both path pointers remain valid for the syscall. renameat2 with
    // RENAME_NOREPLACE publishes atomically and never replaces another entry.
    let status = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn rename_package_noclobber(source: &Path, target: &Path) -> std::io::Result<()> {
    // MoveFileEx without MOVEFILE_REPLACE_EXISTING is the behavior used by
    // std::fs::rename on Windows, so an existing destination fails.
    std::fs::rename(source, target)
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", windows)))]
fn rename_package_noclobber(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-clobber directory publication is unavailable on this platform",
    ))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), ReviewPackageError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    let mut file = options
        .open(path)
        .map_err(|error| ReviewPackageError::Io(error.to_string()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| ReviewPackageError::Io(error.to_string()))
}

fn pretty_json(value: &Value) -> Result<Vec<u8>, ReviewPackageError> {
    let mut body = serde_json::to_vec_pretty(value)
        .map_err(|error| ReviewPackageError::Serialization(error.to_string()))?;
    body.push(b'\n');
    Ok(body)
}

fn modified(metadata: &std::fs::Metadata) -> Option<SystemTime> {
    metadata.modified().ok()
}

fn sha256_hex(bytes: &[u8]) -> String {
    crate::hex::encode(&Sha256::digest(bytes))
}

fn sha256_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn full_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn normalize_relative(value: &str) -> Option<String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || value.as_bytes().get(1) == Some(&b':')
    {
        return None;
    }
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(part) = component else {
            return None;
        };
        let part = part.to_str()?;
        if part.is_empty() || part.chars().any(|character| character.is_control()) {
            return None;
        }
        parts.push(part);
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn short_oid(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}

fn count_label(total: Option<u32>, returned: u32) -> String {
    match total {
        Some(total) if total == returned => total.to_string(),
        Some(total) => format!("{returned} returned / {total} total"),
        None => format!("{returned} returned / total unknown"),
    }
}

fn markdown_text(value: &str) -> String {
    let mut output = String::new();
    let mut previous_space = false;
    for character in value.chars() {
        if character.is_control() || character.is_whitespace() {
            if !previous_space {
                output.push(' ');
                previous_space = true;
            }
        } else {
            if matches!(character, '`' | '*' | '_' | '[' | ']' | '<' | '>') {
                output.push('\\');
            }
            output.push(character);
            previous_space = false;
        }
    }
    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::Indexer;
    use crate::store::Store;
    use std::process::Command;

    fn git(root: &Path, args: &[&str]) -> String {
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
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    fn fixture() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
        let repository = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        std::fs::create_dir(repository.path().join("src")).unwrap();
        std::fs::write(
            repository.path().join("src/lib.rs"),
            "pub fn charge() -> i32 { 1 }\npub fn checkout() -> i32 { charge() }\n",
        )
        .unwrap();
        git(repository.path(), &["init", "-q"]);
        git(
            repository.path(),
            &["config", "user.email", "review@example.test"],
        );
        git(repository.path(), &["config", "user.name", "Review Export"]);
        git(repository.path(), &["add", "."]);
        git(repository.path(), &["commit", "-qm", "baseline"]);
        std::fs::write(
            repository.path().join("src/lib.rs"),
            "pub fn charge() -> i32 { 2 }\npub fn checkout() -> i32 { charge() }\n",
        )
        .unwrap();
        let index_path = state.path().join("mmcg.db");
        let mut store = Store::open(&index_path).unwrap();
        Indexer::new(repository.path())
            .index_all(&mut store, false)
            .unwrap();
        drop(store);
        (repository, state, index_path)
    }

    fn export_options(
        repository: &Path,
        index_path: PathBuf,
        output: PathBuf,
    ) -> ReviewExportOptions {
        ReviewExportOptions {
            root: repository.into(),
            index_path,
            out: output,
            lens: LensOptions {
                since: "HEAD".into(),
                path: ".".into(),
                depth: 3,
                top: 100,
                production_only: false,
            },
            evidence: EvidenceOptions {
                git_commits: 0,
                ..EvidenceOptions::default()
            },
            extensions: EvidenceExtensionOptions::default(),
            evidence_attestation: None,
        }
    }

    #[test]
    fn export_writes_one_atomic_offline_evidence_package() {
        let (repository, state, index_path) = fixture();
        let sarif = state.path().join("semgrep.sarif");
        std::fs::write(
            &sarif,
            r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Semgrep"}},"results":[]}]}"#,
        )
        .unwrap();
        let output = repository.path().join("mastermind-review");
        let mut options = export_options(repository.path(), index_path, output.clone());
        options.evidence.sarif.push(sarif);

        let result = export(&options).unwrap();

        assert_eq!(result.output_dir, output.canonicalize().unwrap());
        assert_eq!(result.artifacts, 5);
        for name in [
            "index.html",
            "mastermind.sarif",
            "summary.md",
            "manifest.json",
            "mastermind-review.yml",
        ] {
            assert!(output.join(name).is_file(), "missing {name}");
        }
        let html = std::fs::read_to_string(output.join("index.html")).unwrap();
        assert!(html.contains("id=\"lens-snapshot\""));
        assert!(html.contains("connect-src 'none'"));
        assert!(!html.contains("href=\"styles.css\""));
        assert!(!html.contains("src=\"app.js\""));
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(output.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["schema_version"], 1);
        assert_eq!(manifest["package_format"], "mastermind-review");
        assert_eq!(
            manifest["repository"]["head_oid"].as_str().unwrap().len(),
            40
        );
        assert_eq!(
            manifest["evidence_binding"]["status"],
            "digest-bound-at-export"
        );
        assert_eq!(
            manifest["evidence_binding"]["sources"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let source = &manifest["evidence_binding"]["sources"][0];
        assert_eq!(source["id"], "sarif:0");
        assert_eq!(source["analysis_status"], "loaded");
        assert_eq!(
            source["sha256"],
            sha256_hex(&std::fs::read(state.path().join("semgrep.sarif")).unwrap())
        );
        assert_eq!(manifest["artifacts"].as_array().unwrap().len(), 4);
        let package_sarif: Value =
            serde_json::from_slice(&std::fs::read(output.join("mastermind.sarif")).unwrap())
                .unwrap();
        assert_eq!(package_sarif["runs"].as_array().unwrap().len(), 2);
        let workflow = std::fs::read_to_string(output.join("mastermind-review.yml")).unwrap();
        assert!(workflow.contains("github/codeql-action/upload-sarif@c54b30b7"));
        assert!(workflow.contains("actions/upload-artifact@043fb46d"));
        assert!(matches!(
            export(&options),
            Err(ReviewPackageError::OutputExists)
        ));
    }

    #[test]
    fn invalid_external_report_is_digest_bound_and_explicitly_partial() {
        let (repository, state, index_path) = fixture();
        let report = state.path().join("invalid.sarif");
        std::fs::write(&report, b"not SARIF").unwrap();
        let output = state.path().join("partial-review");
        let mut options = export_options(repository.path(), index_path, output.clone());
        options.evidence.sarif.push(report.clone());

        let result = export(&options).unwrap();

        assert!(result.partial);
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(output.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(
            manifest["evidence_binding"]["sources"][0]["analysis_status"],
            "error"
        );
        assert_eq!(
            manifest["evidence_binding"]["sources"][0]["sha256"],
            sha256_hex(&std::fs::read(report).unwrap())
        );
        assert!(manifest["analysis"]["states"]
            .as_array()
            .unwrap()
            .iter()
            .any(|state| state["path"] == "$.evidence" && state["state"] == "partial"));
    }

    #[test]
    fn producer_attestation_requires_exact_head_path_and_digest() {
        let repository = tempfile::tempdir().unwrap();
        std::fs::create_dir(repository.path().join("reports")).unwrap();
        let report = repository.path().join("reports/semgrep.sarif");
        std::fs::write(&report, b"sarif bytes").unwrap();
        let root = repository.path().canonicalize().unwrap();
        let source = read_source(
            &root,
            &SourceRequest {
                id: "sarif:0".into(),
                kind: "sarif",
                path: PathBuf::from("reports/semgrep.sarif"),
                maximum_bytes: crate::evidence::MAX_ARTIFACT_BYTES,
                retain_body: false,
            },
        )
        .unwrap();
        assert!(source.body.is_empty());
        let head = "a".repeat(40);
        let body = format!(
            "{{\"schema_version\":1,\"head_oid\":\"{head}\",\"artifacts\":[{{\"kind\":\"sarif\",\"path\":\"reports/semgrep.sarif\",\"sha256\":\"{}\"}}]}}",
            source.sha256
        )
        .into_bytes();
        let input = SourceIdentity {
            id: "attestation".into(),
            kind: "attestation".into(),
            label: "evidence-attestation.json".into(),
            resolved: root.join("evidence-attestation.json"),
            repository_relative: true,
            sha256: sha256_hex(&body),
            bytes: body.len() as u64,
            modified: None,
            body,
        };
        let parsed: EvidenceAttestation =
            crate::audit_bundle::from_json_strict(&input.body).unwrap();
        assert_eq!(parsed.artifacts[0].sha256, source.sha256);

        let validation =
            validate_attestation(Some(&input), std::slice::from_ref(&source), &head).unwrap();
        assert_eq!(validation.artifacts.len(), 1);
        assert_eq!(validation.binding.unwrap().artifacts, 1);

        let mut mismatched = input.clone();
        let text = String::from_utf8(mismatched.body)
            .unwrap()
            .replace(&source.sha256, &"0".repeat(64));
        mismatched.body = text.into_bytes();
        assert!(matches!(
            validate_attestation(Some(&mismatched), &[source], &head),
            Err(ReviewPackageError::InvalidAttestation(_))
        ));
    }

    #[test]
    fn export_records_matching_producer_attestation_end_to_end() {
        let (repository, state, index_path) = fixture();
        std::fs::create_dir(repository.path().join("reports")).unwrap();
        let report = repository.path().join("reports/semgrep.sarif");
        std::fs::write(
            &report,
            r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Semgrep"}},"results":[]}]}"#,
        )
        .unwrap();
        let digest = sha256_hex(&std::fs::read(&report).unwrap());
        let head = git(repository.path(), &["rev-parse", "HEAD"]);
        let attestation = repository.path().join("reports/evidence-attestation.json");
        std::fs::write(
            &attestation,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "head_oid": head,
                "artifacts": [{
                    "kind": "sarif",
                    "path": "reports/semgrep.sarif",
                    "sha256": digest
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let output = state.path().join("attested-review");
        let mut options = export_options(repository.path(), index_path, output.clone());
        options
            .evidence
            .sarif
            .push(PathBuf::from("reports/semgrep.sarif"));
        options.evidence_attestation = Some(PathBuf::from("reports/evidence-attestation.json"));

        let result = export(&options).unwrap();

        assert_eq!(result.evidence_binding, "producer-attested");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(output.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["evidence_binding"]["status"], "producer-attested");
        assert_eq!(
            manifest["evidence_binding"]["sources"][0]["revision_binding"],
            "producer-attested"
        );
        assert_eq!(
            manifest["evidence_binding"]["attestation"]["head_oid"],
            head
        );
    }

    #[test]
    fn strict_attestation_rejects_duplicate_json_keys() {
        let body = br#"{"schema_version":1,"schema_version":1,"head_oid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","artifacts":[]}"#.to_vec();
        let input = SourceIdentity {
            id: "attestation".into(),
            kind: "attestation".into(),
            label: "attestation.json".into(),
            resolved: PathBuf::from("attestation.json"),
            repository_relative: true,
            sha256: sha256_hex(&body),
            bytes: body.len() as u64,
            modified: None,
            body,
        };
        assert!(matches!(
            validate_attestation(Some(&input), &[], &"a".repeat(40)),
            Err(ReviewPackageError::InvalidAttestation(_))
        ));
    }

    #[test]
    fn nested_partial_and_truncation_states_are_explicit() {
        let mut states = BTreeSet::new();
        collect_analysis_states(
            &serde_json::json!({
                "map": {"partial": true},
                "impact": {"rows": {
                    "truncated": true,
                    "truncation_reason": "row_limit",
                    "names_truncated": true,
                    "failures_truncated": true,
                    "contributors_truncated": true
                }}
            }),
            "$",
            &mut states,
        );
        assert!(states
            .iter()
            .any(|state| state.path == "$.map" && state.state == "partial"));
        assert!(states.iter().any(|state| {
            state.path == "$.impact.rows"
                && state.state == "truncated"
                && state.reason.as_deref() == Some("row_limit")
        }));
        for reason in [
            "names_truncated",
            "failures_truncated",
            "contributors_truncated",
        ] {
            assert!(states.iter().any(|state| {
                state.path == "$.impact.rows"
                    && state.state == "truncated"
                    && state.reason.as_deref() == Some(reason)
            }));
        }
    }

    #[test]
    fn atomic_publication_never_replaces_an_existing_directory() {
        let parent = tempfile::tempdir().unwrap();
        let source = tempfile::Builder::new()
            .prefix("source-")
            .tempdir_in(parent.path())
            .unwrap();
        std::fs::write(source.path().join("payload"), b"new").unwrap();
        let target = parent.path().join("review");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("owner"), b"existing").unwrap();

        let error = rename_package_noclobber(source.path(), &target).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(target.join("owner")).unwrap(), b"existing");
        assert!(source.path().join("payload").is_file());
    }
}
