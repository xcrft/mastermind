//! Bounded architecture deltas between a Git baseline and the indexed worktree.
//!
//! The current codegraph remains the source of truth. Temporal analysis clones
//! that exact SQLite snapshot into a private temporary database, rewinds only
//! the Git-changed source files to the resolved baseline blobs, and runs the
//! existing project-map engine over both stores. No checkout, worktree, or
//! repository-index write is required.

use crate::diff::{self, WorkingTreeChangedFile, WorkingTreeDiffError};
use crate::indexer::{extractor_for_path, is_binary_content, parse_blob, MAX_INDEXABLE_FILE_SIZE};
use crate::queries::{
    self, ChangeImpactError, ChangeImpactResponse, MapComponent, MapHotspot, ProjectMapResponse,
    SymbolHit,
};
use crate::store::Store;
use aho_corasick::AhoCorasick;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(test)]
use std::cell::RefCell;

const COMPONENT_DELTA_LIMIT: usize = 100;
const BOUNDARY_DELTA_LIMIT: usize = 500;
const CYCLE_DELTA_LIMIT: usize = 100;
const CENTRALITY_DELTA_LIMIT: usize = 200;
const OWNERSHIP_PATH_LIMIT: usize = 500;
const HISTORY_CANDIDATE_LIMIT: usize = 500;
const HISTORY_ARTIFACT_LIMIT: usize = 5_000;
const HISTORY_BYTE_LIMIT: usize = 32 * 1024 * 1024;
const DIAGNOSTIC_LIMIT: usize = 100;

#[derive(Debug, Clone)]
pub struct TemporalOptions {
    pub since: String,
    pub path: String,
    pub depth: u8,
    pub top: u32,
    pub production_only: bool,
    pub codeowners: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalCollection<T> {
    pub total: u32,
    pub returned: u32,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<&'static str>,
    pub items: Vec<T>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalBaseline {
    pub requested_ref: String,
    pub baseline_oid: String,
    pub head_oid: String,
    pub includes_worktree: bool,
    pub includes_untracked: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalScope {
    pub path: String,
    pub depth: u8,
    pub production_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalComponent {
    pub path: String,
    pub file_count: u32,
    pub languages: Vec<TemporalLanguage>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TemporalLanguage {
    pub language: String,
    pub file_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalComponentDrift {
    pub path: String,
    pub base_file_count: u32,
    pub head_file_count: u32,
    pub base_languages: Vec<TemporalLanguage>,
    pub head_languages: Vec<TemporalLanguage>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct TemporalBoundary {
    pub component: String,
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalBoundaryDrift {
    pub component: String,
    pub file: String,
    pub name: String,
    pub kind: String,
    pub base_line: u32,
    pub head_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalBoundaryDelta {
    pub added: TemporalCollection<TemporalBoundary>,
    pub removed: TemporalCollection<TemporalBoundary>,
    pub changed: TemporalCollection<TemporalBoundaryDrift>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct TemporalHotspot {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub rank: u32,
    pub in_degree: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalCentralityDrift {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub base_rank: u32,
    pub head_rank: u32,
    pub base_in_degree: u32,
    pub head_in_degree: u32,
    pub in_degree_delta: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalHotspotDrift {
    pub entered: TemporalCollection<TemporalHotspot>,
    pub exited: TemporalCollection<TemporalHotspot>,
    pub moved: TemporalCollection<TemporalCentralityDrift>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalCycleDrift {
    pub base_cycles: Vec<Vec<String>>,
    pub head_cycles: Vec<Vec<String>>,
    pub added_members: Vec<String>,
    pub removed_members: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalCycleDelta {
    pub added: TemporalCollection<Vec<String>>,
    pub removed: TemporalCollection<Vec<String>>,
    pub changed: TemporalCollection<TemporalCycleDrift>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalOwnershipChange {
    pub path: String,
    pub base_owners: Vec<String>,
    pub head_owners: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalOwnership {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_source: Option<String>,
    pub changes: TemporalCollection<TemporalOwnershipChange>,
    #[serde(skip)]
    diagnostics_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalHistoryCandidate {
    pub artifact_path: String,
    pub kind: String,
    pub title: String,
    pub referenced_path: String,
    pub trigger: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalComponents {
    pub added: TemporalCollection<TemporalComponent>,
    pub removed: TemporalCollection<TemporalComponent>,
    pub changed: TemporalCollection<TemporalComponentDrift>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalSummary {
    pub architecture_changed: bool,
    pub components_added: u32,
    pub components_removed: u32,
    pub boundaries_added: u32,
    pub boundaries_removed: u32,
    pub boundaries_changed: u32,
    pub cycles_introduced: u32,
    pub cycles_resolved: u32,
    pub cycles_changed: u32,
    pub centrality_increases: u32,
    pub hotspot_entries: u32,
    pub hotspot_exits: u32,
    pub ownership_changes: u32,
    pub history_review_candidates: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalProvenance {
    pub baseline_graph: &'static str,
    pub head_graph: &'static str,
    pub graph_edges: &'static str,
    pub ownership: &'static str,
    pub history: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalLimits {
    pub changed_files: u32,
    pub components_per_direction: u32,
    pub boundaries_per_direction: u32,
    pub cycles_per_direction: u32,
    pub centrality_rows: u32,
    pub ownership_paths: u32,
    pub history_candidates: u32,
    pub history_artifacts: u32,
    pub history_bytes: u64,
    pub diagnostics: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalDiagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporalResponse {
    pub schema_version: u32,
    pub baseline: TemporalBaseline,
    pub scope: TemporalScope,
    pub summary: TemporalSummary,
    pub components: TemporalComponents,
    pub boundaries: TemporalBoundaryDelta,
    pub cycles: TemporalCycleDelta,
    pub centrality: TemporalCollection<TemporalCentralityDrift>,
    pub hotspots: TemporalHotspotDrift,
    /// Cross-component boundary symbols are Mastermind's observable public API.
    pub public_api: TemporalBoundaryDelta,
    pub ownership: TemporalOwnership,
    pub history_review_candidates: TemporalCollection<TemporalHistoryCandidate>,
    pub provenance: TemporalProvenance,
    pub limits: TemporalLimits,
    pub partial: bool,
    pub diagnostics_truncated: bool,
    pub diagnostics: Vec<TemporalDiagnostic>,
}

#[derive(Debug, Clone)]
pub enum TemporalError {
    Impact(ChangeImpactError),
    MapUnavailable(String),
    SnapshotUnavailable,
    BaselineRewind(String),
    ChangeSetTruncated,
    SnapshotChanged,
}

impl TemporalError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Impact(error) => error.code(),
            Self::MapUnavailable(_) => "map_unavailable",
            Self::SnapshotUnavailable => "snapshot_unavailable",
            Self::BaselineRewind(_) => "baseline_rewind_failed",
            Self::ChangeSetTruncated => "change_set_too_large",
            Self::SnapshotChanged => "snapshot_changed",
        }
    }
}

impl std::fmt::Display for TemporalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Impact(error) => write!(formatter, "{}: {error}", self.code()),
            Self::MapUnavailable(message) | Self::BaselineRewind(message) => {
                write!(formatter, "{}: {message}", self.code())
            }
            _ => formatter.write_str(self.code()),
        }
    }
}

impl std::error::Error for TemporalError {}

impl From<ChangeImpactError> for TemporalError {
    fn from(error: ChangeImpactError) -> Self {
        Self::Impact(error)
    }
}

impl From<WorkingTreeDiffError> for TemporalError {
    fn from(error: WorkingTreeDiffError) -> Self {
        match error {
            WorkingTreeDiffError::SnapshotChanged => Self::SnapshotChanged,
            WorkingTreeDiffError::InvalidRef
            | WorkingTreeDiffError::GitUnavailable
            | WorkingTreeDiffError::GitTimeout
            | WorkingTreeDiffError::GitOutputLimit
            | WorkingTreeDiffError::IndexStale => Self::Impact(error.into()),
        }
    }
}

#[derive(Default)]
struct ArchitectureProjection {
    components: BTreeMap<String, TemporalComponent>,
    boundaries: BTreeMap<BoundaryKey, TemporalBoundary>,
    cycles: BTreeSet<Vec<String>>,
    hotspots: BTreeMap<HotspotKey, TemporalHotspot>,
    partial: bool,
}

type BoundaryKey = (String, String, String, String);
type HotspotKey = (String, String, String);

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemporalTestStage {
    AfterImpact,
}

#[cfg(test)]
type TemporalTestHook = (TemporalTestStage, Box<dyn FnOnce()>);

#[cfg(test)]
thread_local! {
    static TEMPORAL_TEST_HOOK: RefCell<Option<TemporalTestHook>> = RefCell::new(None);
}

#[cfg(test)]
pub(crate) struct TemporalTestHookGuard;

#[cfg(test)]
impl Drop for TemporalTestHookGuard {
    fn drop(&mut self) {
        TEMPORAL_TEST_HOOK.with(|hook| hook.borrow_mut().take());
    }
}

#[cfg(test)]
pub(crate) fn install_temporal_test_hook(
    stage: TemporalTestStage,
    hook: impl FnOnce() + 'static,
) -> TemporalTestHookGuard {
    TEMPORAL_TEST_HOOK.with(|slot| *slot.borrow_mut() = Some((stage, Box::new(hook))));
    TemporalTestHookGuard
}

#[cfg(test)]
fn run_temporal_test_hook(stage: TemporalTestStage) {
    let hook = TEMPORAL_TEST_HOOK.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|(candidate, _)| *candidate == stage)
        {
            slot.take().map(|(_, hook)| hook)
        } else {
            None
        }
    });
    if let Some(hook) = hook {
        hook();
    }
}

/// Full temporal analysis for CLI and MCP surfaces.
pub fn analyze(
    store: &Store,
    root: &Path,
    options: &TemporalOptions,
) -> Result<TemporalResponse, TemporalError> {
    let root = root
        .canonicalize()
        .map_err(|_| TemporalError::Impact(ChangeImpactError::RootMismatch))?;
    crate::lens::validate_index_snapshot(store, &root, None).map_err(|error| match error {
        crate::lens::LensError::ImpactUnavailable(error) => TemporalError::Impact(error),
        crate::lens::LensError::AnalysisTimeout
        | crate::lens::LensError::SnapshotTimeout
        | crate::lens::LensError::SnapshotTooLarge => TemporalError::SnapshotUnavailable,
        _ => TemporalError::Impact(ChangeImpactError::IndexStale),
    })?;
    let source_index_state = store
        .source_index_state()
        .map_err(|_| TemporalError::SnapshotChanged)?;
    let index_version = store
        .data_version()
        .map_err(|_| TemporalError::SnapshotChanged)?;
    let impact = queries::change_impact(
        store,
        &root,
        &options.since,
        u32::from(options.depth.clamp(1, 5)),
        options.top.clamp(1, 100) as usize,
    )?;
    #[cfg(test)]
    run_temporal_test_hook(TemporalTestStage::AfterImpact);
    let head_map = match queries::project_map_with_options(
        store,
        &options.path,
        options.depth,
        options.top,
        options.production_only,
    ) {
        Ok(map) => Some(map),
        Err(error)
            if error.contains("scope has no indexed files")
                && scope_has_deleted_file(&impact, &options.path)
                    .map_err(TemporalError::MapUnavailable)? =>
        {
            None
        }
        Err(error) => return Err(TemporalError::MapUnavailable(error)),
    };
    let response = analyze_with_impact(store, &root, options, &impact, head_map.as_ref())?;
    if store
        .data_version()
        .map_err(|_| TemporalError::SnapshotChanged)?
        != index_version
        || store
            .source_index_state()
            .map_err(|_| TemporalError::SnapshotChanged)?
            != source_index_state
    {
        return Err(TemporalError::SnapshotChanged);
    }
    Ok(response)
}

/// Reuse Lens's already-validated impact and map snapshots.
pub(crate) fn analyze_with_impact(
    store: &Store,
    root: &Path,
    options: &TemporalOptions,
    impact: &ChangeImpactResponse,
    head_map: Option<&ProjectMapResponse>,
) -> Result<TemporalResponse, TemporalError> {
    if impact.changes.files.truncated {
        return Err(TemporalError::ChangeSetTruncated);
    }
    let root = root
        .canonicalize()
        .map_err(|_| TemporalError::Impact(ChangeImpactError::RootMismatch))?;
    let changed_files = impact
        .changes
        .files
        .items
        .iter()
        .map(|file| WorkingTreeChangedFile {
            path: file.path.clone(),
            status: file.status.clone(),
        })
        .collect::<Vec<_>>();
    let changed_paths = changed_files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let work_deadline = store
        .remaining_work_budget()
        .deadline
        .map(|remaining| Instant::now() + remaining);
    let interrupted = || store.work_interrupted();
    let baseline_blobs = diff::baseline_blobs_for_paths_controlled(
        &root,
        &impact.baseline.baseline_oid,
        &changed_paths,
        work_deadline,
        Some(&interrupted),
    )?;
    let mut baseline_store = store
        .private_writable_snapshot()
        .map_err(|_| TemporalError::SnapshotUnavailable)?;
    let exhausted = baseline_store.push_work_budget(store.remaining_work_budget());
    if exhausted {
        baseline_store.pop_work_budget();
        return Err(TemporalError::SnapshotUnavailable);
    }
    let mut diagnostics = Vec::new();
    let head_ownership_paths = ownership_scope_paths(store, options)?;
    let baseline_projection = (|| {
        rewind_changed_files(
            &mut baseline_store,
            &changed_paths,
            &baseline_blobs,
            &mut diagnostics,
        )?;
        let map = match queries::project_map_with_options(
            &baseline_store,
            &options.path,
            options.depth,
            options.top,
            options.production_only,
        ) {
            Ok(map) => Some(map),
            Err(error) if error.contains("scope has no indexed files") => {
                diagnostics.push(diagnostic(
                    "baseline_scope_empty",
                    "The selected scope did not contain indexed source files at the baseline.",
                ));
                None
            }
            Err(error) => return Err(TemporalError::MapUnavailable(error)),
        };
        let ownership_paths = ownership_scope_paths(&baseline_store, options)?;
        Ok((map, ownership_paths))
    })();
    baseline_store.pop_work_budget();
    let (baseline_map, baseline_ownership_paths) = baseline_projection?;
    let head_scope_empty = head_map.is_none_or(|map| map.files.total == Some(0));
    if head_scope_empty && baseline_map.is_none() {
        return Err(TemporalError::MapUnavailable(if options.production_only {
            "map production scope has no indexed files".to_string()
        } else {
            "map scope has no indexed files".to_string()
        }));
    }

    let base = baseline_map
        .as_ref()
        .map(ArchitectureProjection::from_map)
        .unwrap_or_default();
    let head = head_map
        .map(ArchitectureProjection::from_map)
        .unwrap_or_default();
    let components = component_delta(&base, &head);
    let boundaries = boundary_delta(&base, &head);
    let cycles = cycle_delta(&base, &head);
    let (centrality, hotspots) = centrality_delta(&base, &head);
    let public_api = boundaries.clone();
    let ownership = ownership_delta(
        OwnershipInput {
            store,
            root: &root,
            override_path: options.codeowners.as_deref(),
            baseline_oid: &impact.baseline.baseline_oid,
            changed_files: &changed_files,
            baseline_blobs: &baseline_blobs,
            scope_paths: baseline_ownership_paths
                .into_iter()
                .chain(head_ownership_paths)
                .collect(),
            boundaries: &boundaries,
        },
        &mut diagnostics,
    )?;
    let history_review_candidates = history_review_candidates(
        store,
        &changed_files,
        &components,
        &boundaries,
        &mut diagnostics,
    )?;

    let validation_deadline = store
        .remaining_work_budget()
        .deadline
        .map(|remaining| Instant::now() + remaining);
    let interrupted = || store.work_interrupted();
    diff::validate_working_tree_snapshot_controlled(
        &root,
        &impact.baseline.baseline_oid,
        &impact.baseline.head_oid,
        &changed_files,
        &impact.snapshot_token,
        validation_deadline,
        Some(&interrupted),
    )?;

    let centrality_increases = centrality
        .items
        .iter()
        .filter(|item| item.in_degree_delta > 0)
        .count() as u32;
    let summary = TemporalSummary {
        architecture_changed: architecture_changed(
            &components,
            &boundaries,
            &cycles,
            &centrality,
            &hotspots,
            &ownership,
        ),
        components_added: components.added.total,
        components_removed: components.removed.total,
        boundaries_added: boundaries.added.total,
        boundaries_removed: boundaries.removed.total,
        boundaries_changed: boundaries.changed.total,
        cycles_introduced: cycles.added.total,
        cycles_resolved: cycles.removed.total,
        cycles_changed: cycles.changed.total,
        centrality_increases,
        hotspot_entries: hotspots.entered.total,
        hotspot_exits: hotspots.exited.total,
        ownership_changes: ownership.changes.total,
        history_review_candidates: history_review_candidates.total,
    };
    let mut partial = base.partial
        || head.partial
        || components.added.truncated
        || components.removed.truncated
        || components.changed.truncated
        || boundaries.added.truncated
        || boundaries.removed.truncated
        || boundaries.changed.truncated
        || cycles.added.truncated
        || cycles.removed.truncated
        || cycles.changed.truncated
        || centrality.truncated
        || hotspots.entered.truncated
        || hotspots.exited.truncated
        || hotspots.moved.truncated
        || ownership.changes.truncated
        || history_review_candidates.truncated;
    if base.partial || head.partial {
        diagnostics.push(diagnostic(
            "bounded_map_projection",
            "At least one base/head map section was truncated; deltas describe only the returned deterministic projection.",
        ));
    }
    diagnostics
        .sort_by(|left, right| (&left.code, &left.message).cmp(&(&right.code, &right.message)));
    diagnostics.dedup_by(|left, right| left.code == right.code && left.message == right.message);
    let mut diagnostics_truncated = ownership.diagnostics_truncated;
    if diagnostics.len() > DIAGNOSTIC_LIMIT {
        diagnostics.truncate(DIAGNOSTIC_LIMIT.saturating_sub(1));
        diagnostics.push(diagnostic(
            "diagnostic_limit",
            "Additional temporal diagnostics were omitted by the response limit.",
        ));
        diagnostics_truncated = true;
    }
    if diagnostics_truncated {
        partial = true;
    }

    Ok(TemporalResponse {
        schema_version: 1,
        baseline: TemporalBaseline {
            requested_ref: impact.baseline.requested_ref.clone(),
            baseline_oid: impact.baseline.baseline_oid.clone(),
            head_oid: impact.baseline.head_oid.clone(),
            includes_worktree: impact.baseline.includes_worktree,
            includes_untracked: impact.baseline.includes_untracked,
        },
        scope: TemporalScope {
            path: if options.path.is_empty() || options.path == "." {
                ".".to_string()
            } else {
                queries::normalize_map_path(&options.path).map_err(TemporalError::MapUnavailable)?
            },
            depth: options.depth.clamp(1, 5),
            production_only: options.production_only,
        },
        summary,
        components,
        boundaries,
        cycles,
        centrality,
        hotspots,
        public_api,
        ownership,
        history_review_candidates,
        provenance: TemporalProvenance {
            baseline_graph: "git_blob_rewind_private_sqlite_snapshot",
            head_graph: "indexed_worktree_snapshot",
            graph_edges: "tree_sitter_syntactic_medium_confidence",
            ownership: "base_and_head_codeowners_last_match",
            history: "exact_path_mentions_review_candidates_only",
        },
        limits: TemporalLimits {
            changed_files: diff::CHANGE_FILE_LIMIT as u32,
            components_per_direction: COMPONENT_DELTA_LIMIT as u32,
            boundaries_per_direction: BOUNDARY_DELTA_LIMIT as u32,
            cycles_per_direction: CYCLE_DELTA_LIMIT as u32,
            centrality_rows: CENTRALITY_DELTA_LIMIT as u32,
            ownership_paths: OWNERSHIP_PATH_LIMIT as u32,
            history_candidates: HISTORY_CANDIDATE_LIMIT as u32,
            history_artifacts: HISTORY_ARTIFACT_LIMIT as u32,
            history_bytes: HISTORY_BYTE_LIMIT as u64,
            diagnostics: DIAGNOSTIC_LIMIT as u32,
        },
        partial,
        diagnostics_truncated,
        diagnostics,
    })
}

pub(crate) fn scope_has_deleted_file(
    impact: &ChangeImpactResponse,
    path: &str,
) -> Result<bool, String> {
    let normalized = queries::normalize_map_path(path)?;
    let directory_prefix = (!normalized.is_empty()).then(|| format!("{normalized}/"));
    Ok(impact.changes.files.items.iter().any(|file| {
        file.status == "deleted"
            && crate::indexer::extractor_for_path(Path::new(&file.path)).is_some()
            && (normalized.is_empty()
                || file.path == normalized
                || directory_prefix
                    .as_deref()
                    .is_some_and(|prefix| file.path.starts_with(prefix)))
    }))
}

fn architecture_changed(
    components: &TemporalComponents,
    boundaries: &TemporalBoundaryDelta,
    cycles: &TemporalCycleDelta,
    centrality: &TemporalCollection<TemporalCentralityDrift>,
    hotspots: &TemporalHotspotDrift,
    ownership: &TemporalOwnership,
) -> bool {
    components.added.total > 0
        || components.removed.total > 0
        || components.changed.total > 0
        || boundaries.added.total > 0
        || boundaries.removed.total > 0
        || boundaries.changed.total > 0
        || cycles.added.total > 0
        || cycles.removed.total > 0
        || cycles.changed.total > 0
        || centrality.total > 0
        || hotspots.entered.total > 0
        || hotspots.exited.total > 0
        || ownership.changes.total > 0
}

fn ownership_scope_paths(
    store: &Store,
    options: &TemporalOptions,
) -> Result<Vec<String>, TemporalError> {
    let normalized =
        queries::normalize_map_path(&options.path).map_err(TemporalError::MapUnavailable)?;
    let kind = if normalized.is_empty() {
        "root"
    } else if store
        .file_mtime(&normalized)
        .map_err(|error| TemporalError::BaselineRewind(error.to_string()))?
        .is_some()
    {
        "file"
    } else {
        "directory"
    };
    store
        .map_paths_filtered(
            &normalized,
            kind,
            OWNERSHIP_PATH_LIMIT.saturating_add(1),
            options.production_only,
        )
        .map_err(|error| TemporalError::BaselineRewind(error.to_string()))
}

fn rewind_changed_files(
    store: &mut Store,
    paths: &[String],
    blobs: &BTreeMap<String, Option<Vec<u8>>>,
    diagnostics: &mut Vec<TemporalDiagnostic>,
) -> Result<(), TemporalError> {
    for path in paths {
        if store.work_interrupted() {
            return Err(TemporalError::SnapshotUnavailable);
        }
        let Some(extractor) = extractor_for_path(Path::new(path)) else {
            continue;
        };
        let Some(bytes) = blobs.get(path).and_then(|value| value.as_deref()) else {
            store
                .purge_file(path)
                .map_err(|error| TemporalError::BaselineRewind(error.to_string()))?;
            continue;
        };
        if bytes.len() as u64 > MAX_INDEXABLE_FILE_SIZE || is_binary_content(bytes) {
            store
                .purge_file(path)
                .map_err(|error| TemporalError::BaselineRewind(error.to_string()))?;
            diagnostics.push(diagnostic(
                "baseline_source_skipped",
                format!("Baseline source `{path}` exceeded source admission limits."),
            ));
            continue;
        }
        let pending = parse_blob(path, bytes, 0, extractor.as_ref())
            .map_err(|error| TemporalError::BaselineRewind(format!("{path}: {error:?}")))?;
        store
            .commit_file(pending)
            .map_err(|error| TemporalError::BaselineRewind(format!("{path}: {error}")))?;
    }
    Ok(())
}

impl ArchitectureProjection {
    fn from_map(map: &ProjectMapResponse) -> Self {
        let mut components = BTreeMap::new();
        let mut boundaries = BTreeMap::new();
        for component in &map.components.items {
            let item = temporal_component(component);
            for boundary in &component.boundaries.items {
                let boundary = temporal_boundary(&component.path, boundary);
                boundaries.insert(boundary_key(&boundary), boundary);
            }
            components.insert(item.path.clone(), item);
        }
        let cycles = map
            .cycles
            .items
            .iter()
            .map(|cycle| {
                let mut cycle = cycle.clone();
                cycle.sort();
                cycle
            })
            .collect();
        let hotspots = map
            .hotspots
            .items
            .iter()
            .enumerate()
            .map(|(index, hotspot)| {
                let hotspot = temporal_hotspot(hotspot, index as u32 + 1);
                (hotspot_key(&hotspot), hotspot)
            })
            .collect();
        let partial = map.files.truncated
            || map.languages.truncated
            || map.components.truncated
            || map
                .components
                .items
                .iter()
                .any(|item| item.boundaries.truncated)
            || map.entry_points.truncated
            || map.hotspots.truncated
            || map.cycles.truncated
            || map.scope.aggregation_paths_truncated;
        Self {
            components,
            boundaries,
            cycles,
            hotspots,
            partial,
        }
    }
}

fn temporal_component(component: &MapComponent) -> TemporalComponent {
    TemporalComponent {
        path: component.path.clone(),
        file_count: component.file_count,
        languages: component
            .languages
            .iter()
            .map(|language| TemporalLanguage {
                language: language.language.clone(),
                file_count: language.file_count,
            })
            .collect(),
    }
}

fn temporal_boundary(component: &str, symbol: &SymbolHit) -> TemporalBoundary {
    TemporalBoundary {
        component: component.to_string(),
        file: symbol.file.clone(),
        name: symbol.name.clone(),
        kind: symbol.kind.clone(),
        line: symbol.line,
        signature: symbol.signature.clone(),
    }
}

fn temporal_hotspot(hotspot: &MapHotspot, rank: u32) -> TemporalHotspot {
    TemporalHotspot {
        file: hotspot.file.clone(),
        name: hotspot.name.clone(),
        kind: hotspot.kind.clone(),
        line: hotspot.line,
        rank,
        in_degree: hotspot.in_degree,
    }
}

fn component_delta(
    base: &ArchitectureProjection,
    head: &ArchitectureProjection,
) -> TemporalComponents {
    let base_keys = base.components.keys().cloned().collect::<BTreeSet<_>>();
    let head_keys = head.components.keys().cloned().collect::<BTreeSet<_>>();
    let added = head_keys
        .difference(&base_keys)
        .filter_map(|key| head.components.get(key).cloned())
        .collect();
    let removed = base_keys
        .difference(&head_keys)
        .filter_map(|key| base.components.get(key).cloned())
        .collect();
    let changed = base_keys
        .intersection(&head_keys)
        .filter_map(|key| {
            let base = base.components.get(key)?;
            let head = head.components.get(key)?;
            (base.file_count != head.file_count || base.languages != head.languages).then(|| {
                TemporalComponentDrift {
                    path: key.clone(),
                    base_file_count: base.file_count,
                    head_file_count: head.file_count,
                    base_languages: base.languages.clone(),
                    head_languages: head.languages.clone(),
                }
            })
        })
        .collect();
    TemporalComponents {
        added: bounded(added, COMPONENT_DELTA_LIMIT),
        removed: bounded(removed, COMPONENT_DELTA_LIMIT),
        changed: bounded(changed, COMPONENT_DELTA_LIMIT),
    }
}

fn boundary_delta(
    base: &ArchitectureProjection,
    head: &ArchitectureProjection,
) -> TemporalBoundaryDelta {
    let base_keys = base.boundaries.keys().cloned().collect::<BTreeSet<_>>();
    let head_keys = head.boundaries.keys().cloned().collect::<BTreeSet<_>>();
    let added = head_keys
        .difference(&base_keys)
        .filter_map(|key| head.boundaries.get(key).cloned())
        .collect();
    let removed = base_keys
        .difference(&head_keys)
        .filter_map(|key| base.boundaries.get(key).cloned())
        .collect();
    let changed = base_keys
        .intersection(&head_keys)
        .filter_map(|key| {
            let base = base.boundaries.get(key)?;
            let head = head.boundaries.get(key)?;
            (base.signature != head.signature).then(|| TemporalBoundaryDrift {
                component: key.0.clone(),
                file: key.1.clone(),
                name: key.2.clone(),
                kind: key.3.clone(),
                base_line: base.line,
                head_line: head.line,
                base_signature: base.signature.clone(),
                head_signature: head.signature.clone(),
            })
        })
        .collect();
    TemporalBoundaryDelta {
        added: bounded(added, BOUNDARY_DELTA_LIMIT),
        removed: bounded(removed, BOUNDARY_DELTA_LIMIT),
        changed: bounded(changed, BOUNDARY_DELTA_LIMIT),
    }
}

fn cycle_delta(base: &ArchitectureProjection, head: &ArchitectureProjection) -> TemporalCycleDelta {
    let exact = base
        .cycles
        .intersection(&head.cycles)
        .cloned()
        .collect::<BTreeSet<_>>();
    let base_remaining = base.cycles.difference(&exact).cloned().collect::<Vec<_>>();
    let head_remaining = head.cycles.difference(&exact).cloned().collect::<Vec<_>>();
    let mut base_edges = vec![Vec::new(); base_remaining.len()];
    let mut head_edges = vec![Vec::new(); head_remaining.len()];
    for (base_index, base_cycle) in base_remaining.iter().enumerate() {
        let base_members = base_cycle.iter().collect::<BTreeSet<_>>();
        for (head_index, head_cycle) in head_remaining.iter().enumerate() {
            if head_cycle
                .iter()
                .any(|member| base_members.contains(member))
            {
                base_edges[base_index].push(head_index);
                head_edges[head_index].push(base_index);
            }
        }
    }
    let mut used_base = vec![false; base_remaining.len()];
    let mut used_head = vec![false; head_remaining.len()];
    let mut changed = Vec::new();
    for start in 0..base_remaining.len() {
        if used_base[start] || base_edges[start].is_empty() {
            continue;
        }
        let mut queue = VecDeque::from([(true, start)]);
        let mut base_indices = BTreeSet::new();
        let mut head_indices = BTreeSet::new();
        while let Some((is_base, index)) = queue.pop_front() {
            if is_base {
                if used_base[index] {
                    continue;
                }
                used_base[index] = true;
                base_indices.insert(index);
                queue.extend(base_edges[index].iter().map(|next| (false, *next)));
            } else {
                if used_head[index] {
                    continue;
                }
                used_head[index] = true;
                head_indices.insert(index);
                queue.extend(head_edges[index].iter().map(|next| (true, *next)));
            }
        }
        let base_cycles = base_indices
            .iter()
            .map(|index| base_remaining[*index].clone())
            .collect::<Vec<_>>();
        let head_cycles = head_indices
            .iter()
            .map(|index| head_remaining[*index].clone())
            .collect::<Vec<_>>();
        let base_members = base_cycles
            .iter()
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>();
        let head_members = head_cycles
            .iter()
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>();
        changed.push(TemporalCycleDrift {
            base_cycles,
            head_cycles,
            added_members: head_members.difference(&base_members).cloned().collect(),
            removed_members: base_members.difference(&head_members).cloned().collect(),
        });
    }
    changed.sort_by(|left, right| {
        left.head_cycles
            .cmp(&right.head_cycles)
            .then_with(|| left.base_cycles.cmp(&right.base_cycles))
    });
    let added = head_remaining
        .into_iter()
        .enumerate()
        .filter_map(|(index, cycle)| (!used_head[index]).then_some(cycle))
        .collect();
    let removed = base_remaining
        .into_iter()
        .enumerate()
        .filter_map(|(index, cycle)| (!used_base[index]).then_some(cycle))
        .collect();
    TemporalCycleDelta {
        added: bounded(added, CYCLE_DELTA_LIMIT),
        removed: bounded(removed, CYCLE_DELTA_LIMIT),
        changed: bounded(changed, CYCLE_DELTA_LIMIT),
    }
}

fn centrality_delta(
    base: &ArchitectureProjection,
    head: &ArchitectureProjection,
) -> (
    TemporalCollection<TemporalCentralityDrift>,
    TemporalHotspotDrift,
) {
    let base_keys = base.hotspots.keys().cloned().collect::<BTreeSet<_>>();
    let head_keys = head.hotspots.keys().cloned().collect::<BTreeSet<_>>();
    let entered = head_keys
        .difference(&base_keys)
        .filter_map(|key| head.hotspots.get(key).cloned())
        .collect();
    let exited = base_keys
        .difference(&head_keys)
        .filter_map(|key| base.hotspots.get(key).cloned())
        .collect();
    let mut drift = base_keys
        .intersection(&head_keys)
        .filter_map(|key| {
            let base = base.hotspots.get(key)?;
            let head = head.hotspots.get(key)?;
            (base.in_degree != head.in_degree || base.rank != head.rank).then(|| {
                TemporalCentralityDrift {
                    file: key.0.clone(),
                    name: key.1.clone(),
                    kind: key.2.clone(),
                    base_rank: base.rank,
                    head_rank: head.rank,
                    base_in_degree: base.in_degree,
                    head_in_degree: head.in_degree,
                    in_degree_delta: i64::from(head.in_degree) - i64::from(base.in_degree),
                }
            })
        })
        .collect::<Vec<_>>();
    drift.sort_by(|left, right| {
        right
            .in_degree_delta
            .cmp(&left.in_degree_delta)
            .then_with(|| {
                (&left.file, &left.name, &left.kind).cmp(&(&right.file, &right.name, &right.kind))
            })
    });
    let centrality = bounded(drift.clone(), CENTRALITY_DELTA_LIMIT);
    let moved = bounded(drift, CENTRALITY_DELTA_LIMIT);
    (
        centrality,
        TemporalHotspotDrift {
            entered: bounded(entered, CENTRALITY_DELTA_LIMIT),
            exited: bounded(exited, CENTRALITY_DELTA_LIMIT),
            moved,
        },
    )
}

struct OwnershipInput<'a> {
    store: &'a Store,
    root: &'a Path,
    override_path: Option<&'a Path>,
    baseline_oid: &'a str,
    changed_files: &'a [WorkingTreeChangedFile],
    baseline_blobs: &'a BTreeMap<String, Option<Vec<u8>>>,
    scope_paths: Vec<String>,
    boundaries: &'a TemporalBoundaryDelta,
}

fn ownership_delta(
    input: OwnershipInput<'_>,
    diagnostics: &mut Vec<TemporalDiagnostic>,
) -> Result<TemporalOwnership, TemporalError> {
    let OwnershipInput {
        store,
        root,
        override_path,
        baseline_oid,
        changed_files,
        baseline_blobs,
        scope_paths,
        boundaries,
    } = input;
    let mut candidates = scope_paths
        .into_iter()
        .chain(changed_files.iter().map(|file| file.path.clone()))
        .chain(boundaries.added.items.iter().map(|item| item.file.clone()))
        .chain(
            boundaries
                .removed
                .items
                .iter()
                .map(|item| item.file.clone()),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let paths_truncated = candidates.len() > OWNERSHIP_PATH_LIMIT;
    candidates.truncate(OWNERSHIP_PATH_LIMIT);

    let requested_head_path = override_path
        .map(|path| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            }
        })
        .or_else(|| crate::evidence::discover_codeowners(root));
    let head_path = requested_head_path
        .as_deref()
        .and_then(|path| path.canonicalize().ok());
    let head_source = head_path
        .as_deref()
        .and_then(|path| path.strip_prefix(root).ok())
        .map(|path| path.to_string_lossy().replace('\\', "/"));
    if override_path.is_some() && head_source.is_none() {
        diagnostics.push(diagnostic(
            "external_codeowners_baseline_unavailable",
            "An unavailable or external CODEOWNERS override has no Git-baseline counterpart; ownership drift was omitted.",
        ));
        return Ok(TemporalOwnership {
            base_source: None,
            head_source: requested_head_path.map(|path| path.to_string_lossy().to_string()),
            changes: TemporalCollection {
                total: 0,
                returned: 0,
                truncated: true,
                truncation_reason: Some("baseline_unavailable"),
                items: Vec::new(),
            },
            diagnostics_truncated: false,
        });
    }

    let baseline_candidates = if override_path.is_some() {
        head_source.iter().cloned().collect()
    } else {
        vec![
            ".github/CODEOWNERS".to_string(),
            "CODEOWNERS".to_string(),
            "docs/CODEOWNERS".to_string(),
        ]
    };
    let deadline = store
        .remaining_work_budget()
        .deadline
        .map(|remaining| Instant::now() + remaining);
    let interrupted = || store.work_interrupted();
    let baseline_owner_blobs = diff::baseline_blobs_for_paths_controlled(
        root,
        baseline_oid,
        &baseline_candidates,
        deadline,
        Some(&interrupted),
    )?;
    let (base_source, base_bytes) = baseline_candidates
        .iter()
        .find_map(|path| {
            baseline_owner_blobs
                .get(path)
                .and_then(|bytes| bytes.as_ref())
                .map(|bytes| (Some(path.clone()), Some(bytes.as_slice())))
        })
        .unwrap_or((None, None));
    let head_bytes = match head_path.as_deref() {
        Some(path) => match read_stable_codeowners(path, store) {
            Ok(bytes) => Some(bytes),
            Err("work_interrupted") => return Err(TemporalError::SnapshotUnavailable),
            Err(code) => {
                diagnostics.push(diagnostic(
                    code,
                    "Head CODEOWNERS could not be read safely.",
                ));
                None
            }
        },
        None => None,
    };

    let base_resolution = base_bytes.and_then(|bytes| {
        match crate::evidence::resolve_codeowners_bytes_controlled(bytes, &candidates, &interrupted)
        {
            Ok(resolution) => Some(resolution),
            Err("work_interrupted") => None,
            Err(code) => {
                diagnostics.push(diagnostic(
                    code,
                    "Baseline CODEOWNERS could not be resolved.",
                ));
                None
            }
        }
    });
    let head_resolution = head_bytes.as_deref().and_then(|bytes| {
        match crate::evidence::resolve_codeowners_bytes_controlled(bytes, &candidates, &interrupted)
        {
            Ok(resolution) => Some(resolution),
            Err("work_interrupted") => None,
            Err(code) => {
                diagnostics.push(diagnostic(code, "Head CODEOWNERS could not be resolved."));
                None
            }
        }
    });
    if store.work_interrupted() {
        return Err(TemporalError::SnapshotUnavailable);
    }
    for resolution in [base_resolution.as_ref(), head_resolution.as_ref()]
        .into_iter()
        .flatten()
    {
        for code in &resolution.diagnostics {
            diagnostics.push(diagnostic(code, "CODEOWNERS resolution was partial."));
        }
    }
    let parser_diagnostics_truncated = base_resolution
        .as_ref()
        .is_some_and(|resolution| resolution.diagnostics_truncated)
        || head_resolution
            .as_ref()
            .is_some_and(|resolution| resolution.diagnostics_truncated);
    if (base_bytes.is_some() && base_resolution.is_none())
        || (head_path.is_some() && head_resolution.is_none())
    {
        return Ok(TemporalOwnership {
            base_source,
            head_source,
            changes: TemporalCollection {
                total: 0,
                returned: 0,
                truncated: true,
                truncation_reason: Some("source_unavailable"),
                items: Vec::new(),
            },
            diagnostics_truncated: parser_diagnostics_truncated,
        });
    }
    let changed_by_path = changed_files
        .iter()
        .map(|file| (file.path.as_str(), file.status.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut changes = Vec::new();
    for path in &candidates {
        if store.work_interrupted() {
            return Err(TemporalError::SnapshotUnavailable);
        }
        let existed_at_base = baseline_blobs.get(path).is_none_or(|value| value.is_some());
        let exists_at_head = changed_by_path.get(path.as_str()) != Some(&"deleted");
        let base_owners = if existed_at_base {
            base_resolution
                .as_ref()
                .and_then(|value| value.owners_by_path.get(path))
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let head_owners = if exists_at_head {
            head_resolution
                .as_ref()
                .and_then(|value| value.owners_by_path.get(path))
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if base_owners != head_owners {
            changes.push(TemporalOwnershipChange {
                path: path.clone(),
                base_owners,
                head_owners,
            });
        }
    }
    let parser_partial = base_resolution.as_ref().is_some_and(|value| value.partial)
        || head_resolution.as_ref().is_some_and(|value| value.partial);
    let mut changes = bounded(changes, OWNERSHIP_PATH_LIMIT);
    if paths_truncated || parser_partial {
        changes.truncated = true;
        changes.truncation_reason = Some(if paths_truncated {
            "path_limit"
        } else {
            "codeowners_work_limit"
        });
    }
    Ok(TemporalOwnership {
        base_source,
        head_source,
        changes,
        diagnostics_truncated: parser_diagnostics_truncated,
    })
}

fn read_stable_codeowners(path: &Path, store: &Store) -> Result<Vec<u8>, &'static str> {
    const MAX_BYTES: u64 = 3 * 1024 * 1024;
    let mut file = std::fs::File::open(path).map_err(|_| "codeowners_unavailable")?;
    let before = file.metadata().map_err(|_| "codeowners_unavailable")?;
    if !before.is_file() {
        return Err("codeowners_unavailable");
    }
    if before.len() >= MAX_BYTES {
        return Err("codeowners_too_large");
    }
    let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
    let mut input = file.by_ref().take(MAX_BYTES + 1);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if store.work_interrupted() {
            return Err("work_interrupted");
        }
        let count = input
            .read(&mut buffer)
            .map_err(|_| "codeowners_unavailable")?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    if bytes.len() as u64 >= MAX_BYTES {
        return Err("codeowners_too_large");
    }
    let after = file.metadata().map_err(|_| "codeowners_changed")?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("codeowners_changed");
    }
    Ok(bytes)
}

fn history_review_candidates(
    store: &Store,
    changed_files: &[WorkingTreeChangedFile],
    components: &TemporalComponents,
    boundaries: &TemporalBoundaryDelta,
    diagnostics: &mut Vec<TemporalDiagnostic>,
) -> Result<TemporalCollection<TemporalHistoryCandidate>, TemporalError> {
    let mut triggers = BTreeMap::new();
    for file in changed_files.iter().filter(|file| file.status == "deleted") {
        triggers.insert(file.path.clone(), "path_deleted".to_string());
    }
    for component in &components.removed.items {
        if component.path != "." && !component.path.is_empty() {
            triggers
                .entry(component.path.clone())
                .or_insert_with(|| "component_removed".to_string());
        }
    }
    for boundary in &boundaries.removed.items {
        triggers
            .entry(boundary.file.clone())
            .or_insert_with(|| "public_api_removed".to_string());
    }
    for boundary in &boundaries.changed.items {
        triggers
            .entry(boundary.file.clone())
            .or_insert_with(|| "public_api_changed".to_string());
    }
    if triggers.is_empty() {
        return Ok(bounded(Vec::new(), HISTORY_CANDIDATE_LIMIT));
    }
    let paths = triggers.keys().cloned().collect::<Vec<_>>();
    let matcher = AhoCorasick::new(&paths)
        .map_err(|error| TemporalError::BaselineRewind(error.to_string()))?;
    let (entries, history_truncated) = store
        .project_history_entries_bounded(HISTORY_ARTIFACT_LIMIT, HISTORY_BYTE_LIMIT)
        .map_err(|error| TemporalError::BaselineRewind(error.to_string()))?;
    let mut candidates = BTreeSet::new();
    let mut overflow = false;
    'entries: for entry in entries {
        if store.work_interrupted() {
            return Err(TemporalError::SnapshotUnavailable);
        }
        for matched in matcher.find_overlapping_iter(entry.body.as_bytes()) {
            if store.work_interrupted() {
                return Err(TemporalError::SnapshotUnavailable);
            }
            if !exact_path_boundaries(&entry.body, matched.start(), matched.end()) {
                continue;
            }
            let referenced_path = paths[matched.pattern().as_usize()].clone();
            candidates.insert((
                entry.path.clone(),
                entry.kind.clone(),
                entry.title.clone(),
                referenced_path.clone(),
                triggers
                    .get(&referenced_path)
                    .cloned()
                    .unwrap_or_else(|| "architecture_changed".to_string()),
            ));
            if candidates.len() > HISTORY_CANDIDATE_LIMIT {
                overflow = true;
                break 'entries;
            }
        }
    }
    if history_truncated {
        diagnostics.push(diagnostic(
            "history_snapshot_truncated",
            "The derived history corpus was truncated before temporal correlation.",
        ));
    }
    let mut response = bounded(
        candidates
            .into_iter()
            .map(|(artifact_path, kind, title, referenced_path, trigger)| {
                TemporalHistoryCandidate {
                    artifact_path,
                    kind,
                    title,
                    referenced_path,
                    trigger,
                }
            })
            .collect(),
        HISTORY_CANDIDATE_LIMIT,
    );
    if history_truncated || overflow {
        response.truncated = true;
        response.truncation_reason = Some(if overflow {
            "candidate_limit"
        } else {
            "history_snapshot_limit"
        });
    }
    Ok(response)
}

fn exact_path_boundaries(body: &str, start: usize, end: usize) -> bool {
    let bytes = body.as_bytes();
    let before = start.checked_sub(1).and_then(|index| bytes.get(index));
    let after = bytes.get(end);
    let is_path_byte = |byte: u8| {
        byte >= 0x80
            || byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'-' | b'.' | b'/' | b'\\')
    };
    let after_is_boundary = match after {
        Some(b'.') => bytes
            .get(end.saturating_add(1))
            .is_none_or(|byte| !is_path_byte(*byte)),
        Some(byte) => !is_path_byte(*byte),
        None => true,
    };
    before.is_none_or(|byte| !is_path_byte(*byte)) && after_is_boundary
}

fn boundary_key(boundary: &TemporalBoundary) -> BoundaryKey {
    (
        boundary.component.clone(),
        boundary.file.clone(),
        boundary.name.clone(),
        boundary.kind.clone(),
    )
}

fn hotspot_key(hotspot: &TemporalHotspot) -> HotspotKey {
    (
        hotspot.file.clone(),
        hotspot.name.clone(),
        hotspot.kind.clone(),
    )
}

fn bounded<T>(mut items: Vec<T>, limit: usize) -> TemporalCollection<T> {
    let total = items.len();
    let truncated = total > limit;
    items.truncate(limit);
    TemporalCollection {
        total: total as u32,
        returned: items.len() as u32,
        truncated,
        truncation_reason: truncated.then_some("output_limit"),
        items,
    }
}

fn diagnostic(code: impl Into<String>, message: impl Into<String>) -> TemporalDiagnostic {
    TemporalDiagnostic {
        code: code.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::Indexer;
    use std::fs;
    use std::process::Command;

    fn set_delta(base: BTreeSet<String>, head: BTreeSet<String>) -> (Vec<String>, Vec<String>) {
        (
            head.difference(&base).cloned().collect(),
            base.difference(&head).cloned().collect(),
        )
    }

    #[test]
    fn temporal_projection_reports_additions_and_removals() {
        let base = BTreeSet::from(["services/legacy".to_string(), "services/shared".to_string()]);
        let head = BTreeSet::from([
            "services/payment".to_string(),
            "services/shared".to_string(),
        ]);

        let (added, removed) = set_delta(base, head);

        assert_eq!(added, ["services/payment"]);
        assert_eq!(removed, ["services/legacy"]);
    }

    #[test]
    fn temporal_projection_covers_cycles_centrality_and_public_api() {
        let boundary = |signature: &str| TemporalBoundary {
            component: "payment".to_string(),
            file: "payment/api.py".to_string(),
            name: "charge".to_string(),
            kind: "function".to_string(),
            line: 1,
            signature: Some(signature.to_string()),
        };
        let hotspot = |rank, in_degree| TemporalHotspot {
            file: "payment/api.py".to_string(),
            name: "charge".to_string(),
            kind: "function".to_string(),
            line: 1,
            rank,
            in_degree,
        };
        let mut base = ArchitectureProjection::default();
        base.boundaries.insert(
            boundary_key(&boundary("charge(total)")),
            boundary("charge(total)"),
        );
        base.cycles
            .insert(vec!["a.py".to_string(), "b.py".to_string()]);
        base.hotspots
            .insert(hotspot_key(&hotspot(2, 3)), hotspot(2, 3));
        let mut head = ArchitectureProjection::default();
        head.boundaries.insert(
            boundary_key(&boundary("charge(total, currency)")),
            boundary("charge(total, currency)"),
        );
        head.cycles
            .insert(vec!["a.py".to_string(), "b.py".to_string()]);
        head.cycles
            .insert(vec!["c.py".to_string(), "d.py".to_string()]);
        head.hotspots
            .insert(hotspot_key(&hotspot(1, 8)), hotspot(1, 8));

        let api = boundary_delta(&base, &head);
        let cycles = cycle_delta(&base, &head);
        let (centrality, hotspots) = centrality_delta(&base, &head);

        assert_eq!(api.changed.total, 1);
        assert_eq!(cycles.added.items, [vec!["c.py", "d.py"]]);
        assert_eq!(centrality.items[0].in_degree_delta, 5);
        assert_eq!(hotspots.moved.items[0].base_rank, 2);
        assert_eq!(hotspots.moved.items[0].head_rank, 1);
    }

    #[test]
    fn temporal_projection_distinguishes_cycle_growth_and_line_movement() {
        let mut base = ArchitectureProjection::default();
        let mut head = ArchitectureProjection::default();
        base.cycles
            .insert(vec!["a.py".to_string(), "b.py".to_string()]);
        head.cycles.insert(vec![
            "a.py".to_string(),
            "b.py".to_string(),
            "c.py".to_string(),
        ]);
        let base_boundary = TemporalBoundary {
            component: "api".to_string(),
            file: "api.py".to_string(),
            name: "serve".to_string(),
            kind: "function".to_string(),
            line: 5,
            signature: Some("serve(request)".to_string()),
        };
        let mut moved_boundary = base_boundary.clone();
        moved_boundary.line = 50;
        base.boundaries
            .insert(boundary_key(&base_boundary), base_boundary);
        head.boundaries
            .insert(boundary_key(&moved_boundary), moved_boundary);

        let cycles = cycle_delta(&base, &head);
        let api = boundary_delta(&base, &head);

        assert_eq!(cycles.added.total, 0);
        assert_eq!(cycles.removed.total, 0);
        assert_eq!(cycles.changed.total, 1);
        assert_eq!(cycles.changed.items[0].added_members, ["c.py"]);
        assert!(cycles.changed.items[0].removed_members.is_empty());
        assert_eq!(api.changed.total, 0, "line motion is not API drift");
    }

    #[test]
    fn hotspot_window_entry_alone_marks_architecture_changed() {
        let empty = ArchitectureProjection::default();
        let components = component_delta(&empty, &empty);
        let boundaries = boundary_delta(&empty, &empty);
        let cycles = cycle_delta(&empty, &empty);
        let centrality = bounded(Vec::new(), CENTRALITY_DELTA_LIMIT);
        let hotspots = TemporalHotspotDrift {
            entered: bounded(
                vec![TemporalHotspot {
                    file: "src/gravity.rs".to_string(),
                    name: "gravity".to_string(),
                    kind: "function".to_string(),
                    line: 1,
                    rank: 1,
                    in_degree: 8,
                }],
                CENTRALITY_DELTA_LIMIT,
            ),
            exited: bounded(Vec::new(), CENTRALITY_DELTA_LIMIT),
            moved: bounded(Vec::new(), CENTRALITY_DELTA_LIMIT),
        };
        let ownership = TemporalOwnership {
            base_source: None,
            head_source: None,
            changes: bounded(Vec::new(), OWNERSHIP_PATH_LIMIT),
            diagnostics_truncated: false,
        };

        assert!(architecture_changed(
            &components,
            &boundaries,
            &cycles,
            &centrality,
            &hotspots,
            &ownership,
        ));
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

    fn write(root: &Path, path: &str, body: &str) {
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(target, body).unwrap();
    }

    #[test]
    fn temporal_analysis_rewinds_components_ownership_and_history() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-q"]);
        git(
            root.path(),
            &["config", "user.email", "temporal@example.com"],
        );
        git(root.path(), &["config", "user.name", "Temporal Test"]);
        write(
            root.path(),
            ".github/CODEOWNERS",
            "* @platform\n/legacy/ @legacy\n",
        );
        write(
            root.path(),
            "legacy/old.py",
            "def legacy_api():\n    return 1\n",
        );
        write(
            root.path(),
            "shared/api.py",
            "def shared_api():\n    return 1\n",
        );
        write(
            root.path(),
            ".mastermind/tasks/001-history/spec.md",
            "---\nid: 001-history\ntitle: Legacy boundary\n---\nKeep legacy/old.py behind the compatibility boundary.\n",
        );
        git(root.path(), &["add", "."]);
        git(root.path(), &["commit", "-qm", "baseline"]);

        fs::remove_file(root.path().join("legacy/old.py")).unwrap();
        write(
            root.path(),
            "payment/new.py",
            "def payment_api():\n    return 2\n",
        );
        write(
            root.path(),
            ".github/CODEOWNERS",
            "* @platform\n/payment/ @payments\n",
        );

        let db = root.path().join(".mastermind/mmcg.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(root.path())
            .index_all(&mut store, true)
            .unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        let freshness = crate::lens::validate_index_snapshot(&store, &canonical_root, None);
        assert!(freshness.is_ok(), "freshness: {freshness:?}");
        assert!(store.project_history_count().unwrap() > 0);
        let (history_entries, _) = store
            .project_history_entries_bounded(HISTORY_ARTIFACT_LIMIT, HISTORY_BYTE_LIMIT)
            .unwrap();
        assert!(
            history_entries
                .iter()
                .any(|entry| entry.body.contains("legacy/old.py")),
            "history entries: {history_entries:?}"
        );
        let response = analyze(
            &store,
            root.path(),
            &TemporalOptions {
                since: "HEAD".to_string(),
                path: ".".to_string(),
                depth: 1,
                top: 20,
                production_only: false,
                codeowners: None,
            },
        )
        .unwrap();

        assert!(response
            .components
            .added
            .items
            .iter()
            .any(|component| component.path == "payment"));
        assert!(response
            .components
            .removed
            .items
            .iter()
            .any(|component| component.path == "legacy"));
        assert!(response.ownership.changes.items.iter().any(|change| {
            change.path == "legacy/old.py"
                && change.base_owners == ["@legacy"]
                && change.head_owners.is_empty()
        }));
        assert!(response.ownership.changes.items.iter().any(|change| {
            change.path == "payment/new.py"
                && change.base_owners.is_empty()
                && change.head_owners == ["@payments"]
        }));
        assert!(
            response
                .history_review_candidates
                .items
                .iter()
                .any(|candidate| {
                    candidate.referenced_path == "legacy/old.py"
                        && candidate.trigger == "path_deleted"
                }),
            "history candidates: {:?}",
            response.history_review_candidates.items
        );
        assert!(response.summary.architecture_changed);

        let deleted_scope = analyze(
            &store,
            root.path(),
            &TemporalOptions {
                since: "HEAD".to_string(),
                path: "legacy".to_string(),
                depth: 1,
                top: 20,
                production_only: false,
                codeowners: None,
            },
        )
        .expect("a fully deleted scope must still be reconstructed from the baseline");
        assert_eq!(deleted_scope.components.removed.total, 1);
        assert_eq!(deleted_scope.components.removed.items[0].path, ".");

        let error = analyze(
            &store,
            root.path(),
            &TemporalOptions {
                since: "HEAD".to_string(),
                path: "typo/never/existed".to_string(),
                depth: 1,
                top: 20,
                production_only: false,
                codeowners: None,
            },
        )
        .expect_err("a scope absent from both snapshots must not look clean");
        assert!(matches!(error, TemporalError::MapUnavailable(_)));
    }

    #[test]
    fn temporal_analysis_detects_codeowners_only_drift_for_stable_sources() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-q"]);
        git(
            root.path(),
            &["config", "user.email", "temporal@example.com"],
        );
        git(root.path(), &["config", "user.name", "Temporal Test"]);
        write(root.path(), "CODEOWNERS", "* @platform\n");
        write(root.path(), "src/a.py", "def a():\n    return 1\n");
        write(root.path(), "src/b.py", "def b():\n    return 2\n");
        git(root.path(), &["add", "."]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        write(root.path(), "CODEOWNERS", "* @architecture\n");

        let db = root.path().join(".mastermind/mmcg.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(root.path())
            .index_all(&mut store, true)
            .unwrap();
        let response = analyze(
            &store,
            root.path(),
            &TemporalOptions {
                since: "HEAD".to_string(),
                path: ".".to_string(),
                depth: 1,
                top: 20,
                production_only: false,
                codeowners: None,
            },
        )
        .unwrap();

        assert!(response.ownership.changes.items.iter().any(|change| {
            change.path == "src/a.py"
                && change.base_owners == ["@platform"]
                && change.head_owners == ["@architecture"]
        }));
        assert!(response.ownership.changes.items.iter().any(|change| {
            change.path == "src/b.py"
                && change.base_owners == ["@platform"]
                && change.head_owners == ["@architecture"]
        }));
    }

    #[test]
    fn temporal_analysis_reports_truncated_codeowners_diagnostics() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-q"]);
        git(
            root.path(),
            &["config", "user.email", "temporal@example.com"],
        );
        git(root.path(), &["config", "user.name", "Temporal Test"]);
        write(root.path(), "CODEOWNERS", "* @platform\n");
        write(root.path(), "src/a.py", "def a():\n    return 1\n");
        git(root.path(), &["add", "."]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        write(root.path(), "CODEOWNERS", &"!\n".repeat(60_000));

        let db = root.path().join(".mastermind/mmcg.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(root.path())
            .index_all(&mut store, true)
            .unwrap();
        let response = analyze(
            &store,
            root.path(),
            &TemporalOptions {
                since: "HEAD".to_string(),
                path: ".".to_string(),
                depth: 1,
                top: 20,
                production_only: false,
                codeowners: None,
            },
        )
        .unwrap();

        assert!(response.partial);
        assert!(response.diagnostics_truncated);
        assert!(response
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "codeowner_diagnostic_limit"));
        assert!(response.diagnostics.len() <= DIAGNOSTIC_LIMIT);
    }

    #[cfg(unix)]
    #[test]
    fn temporal_analysis_rewinds_a_git_path_with_a_newline() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-q"]);
        git(
            root.path(),
            &["config", "user.email", "temporal@example.com"],
        );
        git(root.path(), &["config", "user.name", "Temporal Test"]);
        let path = "src/evil\nname.py";
        write(root.path(), path, "def value():\n    return 1\n");
        git(root.path(), &["add", "."]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        write(root.path(), path, "def value():\n    return 2\n");

        let db = root.path().join(".mastermind/mmcg.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(root.path())
            .index_all(&mut store, true)
            .unwrap();
        let response = analyze(
            &store,
            root.path(),
            &TemporalOptions {
                since: "HEAD".to_string(),
                path: "src".to_string(),
                depth: 1,
                top: 20,
                production_only: false,
                codeowners: None,
            },
        )
        .unwrap();

        assert_eq!(response.components.added.total, 0);
        assert_eq!(response.components.removed.total, 0);
        assert!(!response
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "baseline_scope_empty"));
    }

    #[test]
    fn temporal_analysis_rejects_an_index_revision_race_after_impact() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-q"]);
        git(
            root.path(),
            &["config", "user.email", "temporal@example.com"],
        );
        git(root.path(), &["config", "user.name", "Temporal Test"]);
        write(root.path(), "src/a.py", "def a():\n    return 1\n");
        git(root.path(), &["add", "."]);
        git(root.path(), &["commit", "-qm", "baseline"]);
        write(root.path(), "src/a.py", "def a():\n    return 2\n");

        let db = root.path().join(".mastermind/mmcg.db");
        let mut store = Store::open(&db).unwrap();
        Indexer::new(root.path())
            .index_all(&mut store, true)
            .unwrap();
        let race_db = db.clone();
        let _hook = install_temporal_test_hook(TemporalTestStage::AfterImpact, move || {
            let connection = rusqlite::Connection::open(race_db).unwrap();
            connection
                .execute(
                    "INSERT INTO meta(key, value) VALUES ('temporal_race', '1')",
                    [],
                )
                .unwrap();
        });

        let error = analyze(
            &store,
            root.path(),
            &TemporalOptions {
                since: "HEAD".to_string(),
                path: ".".to_string(),
                depth: 1,
                top: 20,
                production_only: false,
                codeowners: None,
            },
        )
        .unwrap_err();
        assert!(matches!(error, TemporalError::SnapshotChanged));
    }

    #[test]
    fn signature_drift_marks_exact_history_mentions_for_review() {
        let path = tempfile::tempdir().unwrap().path().join("history.db");
        let mut store = Store::open(path).unwrap();
        store
            .replace_project_history(&[crate::store::ProjectHistoryEntry {
                path: ".mastermind/architecture/adr.md".to_string(),
                kind: "architecture_decision".to_string(),
                title: "Public payment API".to_string(),
                body: "The stable boundary is services/payment/api.py.".to_string(),
            }])
            .unwrap();
        let empty = ArchitectureProjection::default();
        let boundary = TemporalBoundaryDrift {
            component: "services/payment".to_string(),
            file: "services/payment/api.py".to_string(),
            name: "charge".to_string(),
            kind: "function".to_string(),
            base_line: 1,
            head_line: 1,
            base_signature: Some("charge(total)".to_string()),
            head_signature: Some("charge(total, currency)".to_string()),
        };
        let boundaries = TemporalBoundaryDelta {
            added: bounded(Vec::new(), BOUNDARY_DELTA_LIMIT),
            removed: bounded(Vec::new(), BOUNDARY_DELTA_LIMIT),
            changed: bounded(vec![boundary], BOUNDARY_DELTA_LIMIT),
        };
        let mut diagnostics = Vec::new();

        let candidates = history_review_candidates(
            &store,
            &[],
            &component_delta(&empty, &empty),
            &boundaries,
            &mut diagnostics,
        )
        .unwrap();

        assert_eq!(candidates.total, 1);
        assert_eq!(candidates.items[0].trigger, "public_api_changed");
    }
}
