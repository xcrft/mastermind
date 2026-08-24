//! Local, read-only change-review UI backed by the shared map and impact engines.
//!
//! Lens deliberately has no HTTP framework or frontend build step. It serves a
//! small, embedded application on an ephemeral loopback port, accepts only
//! same-origin `GET`/`HEAD` requests, and opens the existing SQLite index in
//! query-only mode for every refresh.

use crate::queries::{self, ChangeImpactError, ChangeImpactResponse, ProjectMapResponse};
use crate::store::{query_budget_ms_from_env, Store, WorkBudget, DEFAULT_CLI_BUDGET_MS};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const INDEX_HTML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/lens/index.html"
));
const APP_JS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/lens/app.js"));
const STYLES_CSS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/lens/styles.css"
));
const MASTERMIND_MARK_SVG: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/lens/mastermind-mark.svg"
));

const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const LENS_CHANGED_FILE_ITEM_LIMIT: usize = 200;
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

#[derive(Debug, Clone, Serialize)]
pub struct LensOptions {
    pub since: String,
    pub path: String,
    pub depth: u8,
    pub top: u32,
    pub production_only: bool,
}

#[derive(Debug, Serialize)]
pub struct LensRepository {
    pub name: String,
    pub root_label: String,
}

#[derive(Debug)]
pub struct LensSnapshot {
    pub schema_version: u32,
    pub repository: LensRepository,
    pub options: LensOptions,
    pub map: ProjectMapResponse,
    pub impact: ChangeImpactResponse,
    pub temporal: LensTemporalSnapshot,
    pub semantic: crate::scip_overlay::SemanticOverlaySnapshot,
    pub evidence: crate::evidence::EvidenceSnapshot,
    /// Audit facts for the selected map scope.
    pub audit: LensAudit,
}

impl Serialize for LensSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LensSnapshot", 9)?;
        state.serialize_field("schema_version", &self.schema_version)?;
        state.serialize_field("repository", &self.repository)?;
        state.serialize_field("options", &self.options)?;
        state.serialize_field("map", &self.map)?;
        state.serialize_field("impact", &LensImpactPayload(&self.impact))?;
        state.serialize_field("temporal", &self.temporal)?;
        state.serialize_field("semantic", &self.semantic)?;
        state.serialize_field("evidence", &self.evidence)?;
        state.serialize_field("audit", &self.audit)?;
        state.end()
    }
}

struct LensImpactPayload<'a>(&'a ChangeImpactResponse);

impl Serialize for LensImpactPayload<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let impact = self.0;
        let mut state = serializer.serialize_struct("ChangeImpactResponse", 11)?;
        state.serialize_field("schema_version", &impact.schema_version)?;
        state.serialize_field("baseline", &impact.baseline)?;
        state.serialize_field("scope", &impact.scope)?;
        state.serialize_field("changes", &LensImpactChangesPayload(&impact.changes))?;
        state.serialize_field("affected_components", &impact.affected_components)?;
        state.serialize_field("impact", &impact.impact)?;
        state.serialize_field("api_crossings", &impact.api_crossings)?;
        state.serialize_field("tests", &impact.tests)?;
        state.serialize_field("disciplines", &impact.disciplines)?;
        state.serialize_field("limits", &impact.limits)?;
        state.serialize_field("precision_notes", &impact.precision_notes)?;
        state.end()
    }
}

struct LensImpactChangesPayload<'a>(&'a queries::ImpactChanges);

impl Serialize for LensImpactChangesPayload<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ImpactChanges", 2)?;
        state.serialize_field(
            "files",
            &LensProjectedCollection {
                source: &self.0.files,
                limit: LENS_CHANGED_FILE_ITEM_LIMIT,
            },
        )?;
        state.serialize_field("symbols", &self.0.symbols)?;
        state.end()
    }
}

struct LensProjectedCollection<'a, T> {
    source: &'a queries::Collection<T>,
    limit: usize,
}

impl<T: Serialize> Serialize for LensProjectedCollection<'_, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let projected = self.source.truncated && self.source.items.len() > self.limit;
        let items = if projected {
            &self.source.items[..self.limit]
        } else {
            &self.source.items
        };
        let returned = if projected {
            u32::try_from(items.len()).unwrap_or(u32::MAX)
        } else {
            self.source.returned
        };
        let mut state = serializer.serialize_struct("Collection", if projected { 8 } else { 5 })?;
        state.serialize_field("total", &self.source.total)?;
        state.serialize_field("returned", &returned)?;
        state.serialize_field("truncated", &self.source.truncated)?;
        state.serialize_field("truncation_reason", &self.source.truncation_reason)?;
        if projected {
            state.serialize_field("observed", &self.source.returned)?;
            state.serialize_field("projection_truncated", &true)?;
            state.serialize_field("projection_reason", "lens_payload_limit")?;
        }
        state.serialize_field("items", items)?;
        state.end()
    }
}

/// Bounded audit facts for the selected map scope.
#[derive(Debug, Serialize)]
pub struct LensAudit {
    /// Unreferenced candidates retain `Store::unreferenced` false positives.
    pub dead_code: LensDeadCode,
    pub change_hotspots: LensChangeHotspots,
    pub largest_files: LensLargestFiles,
    pub bus_factor: LensBusFactor,
    pub narrative_binding: AuditNarrativeBinding,
    /// Optional bounded sidecar interpretation; mmcg never calls a model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrative: Option<AuditNarrative>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditNarrativeBinding {
    pub repository_identity: String,
    pub baseline_oid: String,
    pub head_oid: String,
    pub snapshot_token_sha256: String,
    pub map_sha256: String,
}

/// Bounded, validated AI narrative rendered only through DOM text nodes.
#[derive(Debug, Serialize)]
pub struct AuditNarrative {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub lenses: std::collections::BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<AuditDomain>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub red_team: Vec<AuditRedTeam>,
}

#[derive(Debug, Serialize)]
pub struct AuditDomain {
    pub name: String,
    pub severity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AuditRedTeam {
    pub title: String,
    pub severity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// Non-empty component path list required at ingestion.
    pub vector: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LensLargestFiles {
    pub status: &'static str,
    pub returned: u32,
    pub truncated: bool,
    pub items: Vec<crate::store::FileSize>,
}

#[derive(Debug, Serialize)]
pub struct LensBusFactor {
    /// `"available"`, or `"unavailable"` when git history could not be read.
    pub status: &'static str,
    pub window_commits: u32,
    pub returned: u32,
    pub truncated: bool,
    pub items: Vec<LensComponentAuthors>,
}

#[derive(Debug, Serialize)]
pub struct LensComponentAuthors {
    pub component: String,
    pub authors: u32,
    pub touches: u32,
    pub top_author_pct: u32,
}

/// Change-hotspot ranking: churn (recent commits touching a file) crossed with
/// centrality (incoming edges into symbols the file declares).
#[derive(Debug, Serialize)]
pub struct LensChangeHotspots {
    /// `"available"`, or `"unavailable"` when git history could not be read.
    pub status: &'static str,
    /// Commits inspected in the churn window (0 when unavailable).
    pub window_commits: u32,
    pub returned: u32,
    pub truncated: bool,
    pub items: Vec<LensChangeHotspot>,
}

#[derive(Debug, Serialize)]
pub struct LensChangeHotspot {
    pub file: String,
    pub commits: u32,
    pub in_degree: u32,
    /// `commits * in_degree` — the ranking key, exposed so the UI can explain
    /// the ordering instead of asserting it.
    pub score: u64,
}

#[derive(Debug, Serialize)]
pub struct LensDeadCode {
    pub total: u32,
    pub returned: u32,
    pub truncated: bool,
    pub items: Vec<crate::queries::SymbolHit>,
}

#[derive(Debug, Serialize)]
pub struct LensTemporalSnapshot {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<crate::temporal::TemporalResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<LensTemporalDiagnostic>,
}

#[derive(Debug, Serialize)]
pub struct LensTemporalDiagnostic {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Debug)]
pub enum LensError {
    RootUnavailable,
    IndexUnavailable,
    IndexStale,
    SnapshotTooLarge,
    SnapshotTimeout,
    AnalysisTimeout,
    MapUnavailable(String),
    ImpactUnavailable(ChangeImpactError),
    Serialization,
    Bind(String),
    Serve(String),
}

impl LensError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::RootUnavailable => "root_unavailable",
            Self::IndexUnavailable => "index_unavailable",
            Self::IndexStale => "index_stale",
            Self::SnapshotTooLarge => "snapshot_too_large",
            Self::SnapshotTimeout => "snapshot_timeout",
            Self::AnalysisTimeout => "analysis_timeout",
            Self::MapUnavailable(_) => "map_unavailable",
            Self::ImpactUnavailable(error) => error.code(),
            Self::Serialization => "serialization_failed",
            Self::Bind(_) => "server_bind_failed",
            Self::Serve(_) => "server_io_failed",
        }
    }

    fn public_message(&self) -> String {
        match self {
            Self::RootUnavailable => "repository root is unavailable".into(),
            Self::IndexUnavailable => {
                "read-only index is unavailable; run `mastermind index .` first".into()
            }
            Self::IndexStale => {
                "the index is stale or incompatible; run `mastermind index .` and refresh".into()
            }
            Self::SnapshotTooLarge => {
                "the active index snapshot exceeds Lens's 2 GiB safety limit; stop the index writer, run `mastermind index .`, and refresh".into()
            }
            Self::SnapshotTimeout => {
                "preparing the read-only index snapshot exceeded its deadline; retry after the index writer is idle".into()
            }
            Self::AnalysisTimeout => {
                "Lens analysis exceeded its deadline; retry or narrow `--path`".into()
            }
            Self::MapUnavailable(_) => {
                "Lens could not build the project map; refresh the index or narrow `--path`".into()
            }
            Self::ImpactUnavailable(error) => match error {
                ChangeImpactError::InvalidRef => {
                    "baseline ref is invalid; pass an existing ref with `--since`".into()
                }
                ChangeImpactError::RootMismatch => {
                    "the index belongs to a different repository root".into()
                }
                ChangeImpactError::IndexStale => {
                    "the index is stale; run `mastermind index .` and refresh".into()
                }
                ChangeImpactError::SnapshotChanged => {
                    "repository or index changed during analysis; refresh Lens".into()
                }
                ChangeImpactError::GitTimeout => {
                    "git analysis exceeded its deadline; retry or narrow the repository".into()
                }
                ChangeImpactError::GitOutputLimit => {
                    "git analysis exceeded its output limit; narrow the change".into()
                }
            },
            Self::Serialization => "Lens could not serialize its bounded result".into(),
            Self::Bind(_) => "Lens could not bind its loopback server".into(),
            Self::Serve(_) => "Lens encountered a local server error".into(),
        }
    }
}

impl fmt::Display for LensError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MapUnavailable(message) => write!(formatter, "{}: {message}", self.code()),
            Self::ImpactUnavailable(error) => write!(formatter, "{}: {error}", self.code()),
            Self::Bind(message) | Self::Serve(message) => {
                write!(formatter, "{}: {message}", self.code())
            }
            _ => formatter.write_str(self.code()),
        }
    }
}

impl std::error::Error for LensError {}

struct WorkBudgetScope<'a>(&'a Store);

impl Drop for WorkBudgetScope<'_> {
    fn drop(&mut self) {
        self.0.pop_work_budget();
    }
}

pub fn build_snapshot(
    store: &Store,
    root: &Path,
    options: &LensOptions,
) -> Result<LensSnapshot, LensError> {
    build_snapshot_with_evidence(
        store,
        root,
        options,
        &crate::evidence::EvidenceOptions::default(),
    )
}

pub fn build_snapshot_with_evidence(
    store: &Store,
    root: &Path,
    options: &LensOptions,
    evidence: &crate::evidence::EvidenceOptions,
) -> Result<LensSnapshot, LensError> {
    build_snapshot_with_evidence_extensions(
        store,
        root,
        options,
        evidence,
        &crate::evidence::EvidenceExtensionOptions::default(),
    )
}

pub fn build_snapshot_with_evidence_extensions(
    store: &Store,
    root: &Path,
    options: &LensOptions,
    evidence: &crate::evidence::EvidenceOptions,
    extensions: &crate::evidence::EvidenceExtensionOptions,
) -> Result<LensSnapshot, LensError> {
    build_snapshot_until(
        store,
        root,
        options,
        evidence,
        extensions,
        request_deadline(),
    )
}

/// Build one fail-closed Lens snapshot from an existing index without serving
/// HTTP. Review-package export uses this entry point so CLI, MCP, and Lens keep
/// the same freshness, WAL, and bounded-analysis semantics.
pub(crate) fn snapshot_from_paths_with_evidence_extensions(
    root: &Path,
    index_path: &Path,
    options: &LensOptions,
    evidence: &crate::evidence::EvidenceOptions,
    extensions: &crate::evidence::EvidenceExtensionOptions,
) -> Result<LensSnapshot, LensError> {
    snapshot_from_paths_until(
        root,
        index_path,
        options,
        evidence,
        extensions,
        request_deadline(),
    )
}

fn snapshot_from_paths_until(
    root: &Path,
    index_path: &Path,
    options: &LensOptions,
    evidence: &crate::evidence::EvidenceOptions,
    extensions: &crate::evidence::EvidenceExtensionOptions,
    deadline: Option<Instant>,
) -> Result<LensSnapshot, LensError> {
    let root = root
        .canonicalize()
        .map_err(|_| LensError::RootUnavailable)?;
    let index_path = index_path
        .canonicalize()
        .map_err(|_| LensError::IndexUnavailable)?;
    if !index_path.is_file() {
        return Err(LensError::IndexUnavailable);
    }
    let before = index_source_state(&index_path)?;
    let store =
        Store::open_read_only_with_deadline(&index_path, deadline).map_err(read_only_open_error)?;
    let snapshot = build_snapshot_until(&store, &root, options, evidence, extensions, deadline)?;
    if index_source_state(&index_path)? != before {
        return Err(LensError::ImpactUnavailable(
            ChangeImpactError::SnapshotChanged,
        ));
    }
    Ok(snapshot)
}

/// Render a single-file, offline Lens application. The snapshot, stylesheet,
/// and application code are embedded under content hashes; the resulting CSP
/// disables all network connections and external resources.
pub(crate) fn standalone_html(snapshot: &LensSnapshot) -> Result<Vec<u8>, LensError> {
    let snapshot_json = serde_json::to_string(snapshot).map_err(|_| LensError::Serialization)?;
    standalone_html_from_json(&snapshot_json)
}

fn standalone_html_from_json(snapshot_json: &str) -> Result<Vec<u8>, LensError> {
    standalone_html_from_template(snapshot_json, INDEX_HTML)
}

fn replace_standalone_marker(
    html: &mut String,
    served: &str,
    standalone: &str,
) -> Result<(), LensError> {
    if !html.contains(served) {
        return Err(LensError::Serialization);
    }
    *html = html.replacen(served, standalone, 1);
    Ok(())
}

fn standalone_html_from_template(
    snapshot_json: &str,
    template: &str,
) -> Result<Vec<u8>, LensError> {
    let escaped_json = snapshot_json
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    let script_hash = inline_hash(APP_JS.as_bytes());
    let snapshot_hash = inline_hash(escaped_json.as_bytes());
    let style_hash = inline_hash(STYLES_CSS.as_bytes());
    let csp = format!(
        "default-src 'none'; script-src '{script_hash}' '{snapshot_hash}'; style-src '{style_hash}'; style-src-attr 'unsafe-inline'; img-src data:; connect-src 'none'; font-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
    let served_csp = "    <meta\n      http-equiv=\"Content-Security-Policy\"\n      content=\"default-src 'self'; script-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'; connect-src 'self'; img-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'\"\n    >\n";
    let template = template.replace("\r\n", "\n");
    let mut html = template.replace(
        served_csp,
        &format!("    <meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\n"),
    );
    if html == template {
        return Err(LensError::Serialization);
    }
    html = html.replace(
        "    <link rel=\"stylesheet\" href=\"styles.css\">",
        &format!("    <style>{STYLES_CSS}</style>"),
    );
    html = html.replace("    <script src=\"app.js\" defer></script>\n", "");
    replace_standalone_marker(
        &mut html,
        "src=\"mastermind-mark.svg\"",
        &format!(
            "src=\"data:image/svg+xml;base64,{}\"",
            BASE64.encode(MASTERMIND_MARK_SVG.as_bytes())
        ),
    )?;
    replace_standalone_marker(
        &mut html,
        "<span data-lens-runtime-label><i aria-hidden=\"true\"></i>Local</span>",
        "<span data-lens-runtime-label><i aria-hidden=\"true\"></i>Offline package</span>",
    )?;
    replace_standalone_marker(
        &mut html,
        "data-lens-snapshot-action aria-label=\"Refresh Lens snapshot\"",
        "data-lens-snapshot-action aria-label=\"Static Lens snapshot\"",
    )?;
    replace_standalone_marker(
        &mut html,
        "<span data-lens-action-label>Refresh</span>",
        "<span data-lens-action-label>Static snapshot</span>",
    )?;
    let scripts = format!(
        "    <script type=\"application/json\" id=\"lens-snapshot\">{escaped_json}</script>\n    <script>{APP_JS}</script>\n  </body>"
    );
    html = html.replace("  </body>", &scripts);
    if html.contains("href=\"styles.css\"")
        || html.contains("src=\"app.js\"")
        || !html.contains("id=\"lens-snapshot\"")
    {
        return Err(LensError::Serialization);
    }
    Ok(html.into_bytes())
}

fn inline_hash(bytes: &[u8]) -> String {
    format!("sha256-{}", BASE64.encode(Sha256::digest(bytes)))
}

fn request_deadline() -> Option<Instant> {
    let budget_ms = query_budget_ms_from_env(DEFAULT_CLI_BUDGET_MS);
    (budget_ms != 0).then(|| Instant::now() + Duration::from_millis(budget_ms))
}

fn remaining_work_budget(deadline: Option<Instant>) -> WorkBudget {
    match deadline {
        Some(deadline) => WorkBudget {
            deadline: Some(deadline.saturating_duration_since(Instant::now())),
            op_ticks: None,
        },
        None => WorkBudget::UNLIMITED,
    }
}

/// Fail-closed freshness proof shared by read-only architecture consumers.
/// It checks indexed rows against source files and tracked source files against
/// indexed rows, so deletions and newly tracked files cannot hide in one
/// direction of the comparison.
pub(crate) fn validate_index_snapshot(
    store: &Store,
    root: &Path,
    deadline: Option<Instant>,
) -> Result<(), LensError> {
    validated_index_paths(store, root, deadline).map(|_| ())
}

fn validated_index_paths(
    store: &Store,
    root: &Path,
    deadline: Option<Instant>,
) -> Result<HashSet<String>, LensError> {
    if !store.schema_current().unwrap_or(false)
        || !store.extractor_contract_current().unwrap_or(false)
    {
        return Err(LensError::IndexStale);
    }
    let stored_root = store
        .meta_value("index_root")
        .map_err(|_| LensError::IndexStale)?
        .ok_or(LensError::IndexStale)?;
    let stored_root = PathBuf::from(stored_root)
        .canonicalize()
        .map_err(|_| LensError::IndexStale)?;
    if stored_root != root {
        return Err(LensError::ImpactUnavailable(
            ChangeImpactError::RootMismatch,
        ));
    }

    let indexed_files = store
        .files_under(None, None)
        .map_err(|_| LensError::IndexStale)?;
    let mut indexed_paths = HashSet::with_capacity(indexed_files.len());
    for indexed_file in indexed_files {
        if deadline.is_some_and(|value| Instant::now() >= value) {
            return Err(LensError::AnalysisTimeout);
        }
        let indexed_path = indexed_file.path;
        let relative = Path::new(&indexed_path);
        let safe_relative = !indexed_path.is_empty()
            && !relative.is_absolute()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)));
        if !safe_relative {
            return Err(LensError::IndexStale);
        }
        let source_path = root.join(relative);
        let metadata = source_path.metadata().map_err(|_| LensError::IndexStale)?;
        if !metadata.is_file() || metadata.len() > crate::indexer::MAX_INDEXABLE_FILE_SIZE {
            return Err(LensError::IndexStale);
        }
        let source_mtime = metadata
            .modified()
            .ok()
            .and_then(|mtime| mtime.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64);
        if source_mtime != Some(indexed_file.indexed_at) {
            let bytes = std::fs::read(&source_path).map_err(|_| LensError::IndexStale)?;
            let digest = crate::hex::encode(&Sha256::digest(bytes));
            let stored_digest = store
                .file_content_sha256(&indexed_path)
                .map_err(|_| LensError::IndexStale)?;
            if stored_digest.as_deref().filter(|value| !value.is_empty()) != Some(digest.as_str()) {
                return Err(LensError::IndexStale);
            }
        }
        indexed_paths.insert(indexed_path);
    }

    if deadline.is_some_and(|value| Instant::now() >= value) {
        return Err(LensError::AnalysisTimeout);
    }
    let tracked_paths =
        crate::indexer::tracked_relative_paths(root).map_err(|error| match error {
            crate::diff::WorkingTreeDiffError::GitTimeout => {
                LensError::ImpactUnavailable(ChangeImpactError::GitTimeout)
            }
            crate::diff::WorkingTreeDiffError::GitOutputLimit => {
                LensError::ImpactUnavailable(ChangeImpactError::GitOutputLimit)
            }
            _ => LensError::IndexStale,
        })?;
    for relative in tracked_paths {
        if deadline.is_some_and(|value| Instant::now() >= value) {
            return Err(LensError::AnalysisTimeout);
        }
        if crate::indexer::extractor_for_path(&relative).is_none() {
            continue;
        }
        let normalized = relative.to_string_lossy().replace('\\', "/");
        if indexed_paths.contains(&normalized) {
            continue;
        }
        match crate::indexer::source_admission(&root.join(&relative)) {
            Ok(()) => return Err(LensError::IndexStale),
            Err(crate::indexer::IndexError::Skipped(_)) => {}
            Err(_) => return Err(LensError::IndexStale),
        }
    }
    Ok(indexed_paths)
}

/// Invalid, unsupported, or oversized sidecars degrade to facts-only output.
fn read_audit_narrative(
    root: &Path,
    expected_binding: &AuditNarrativeBinding,
    valid_components: &HashSet<String>,
) -> Option<AuditNarrative> {
    const MAX_BYTES: u64 = 256 * 1024;
    const CAP_SUMMARY: usize = 2000;
    const CAP_LENS: usize = 600;
    const CAP_LENSES: usize = 12;
    const CAP_DOMAINS: usize = 16;
    const CAP_DOMAIN_NAME: usize = 80;
    const CAP_DOMAIN_NOTE: usize = 400;
    const CAP_DOMAIN_COMPONENTS: usize = 24;
    const CAP_COMPONENT: usize = 200;
    const CAP_RED_TEAM: usize = 16;
    const CAP_TITLE: usize = 140;
    const CAP_SCENARIO: usize = 600;
    const CAP_EVIDENCE: usize = 500;

    let path = std::env::var("MMCG_AUDIT_NARRATIVE")
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".mastermind").join("audit-narrative.json"));
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let raw: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if raw
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return None;
    }
    let binding: AuditNarrativeBinding =
        serde_json::from_value(raw.get("binding")?.clone()).ok()?;
    if &binding != expected_binding {
        return None;
    }

    // Truncate on a char boundary so a byte cap never splits a multibyte glyph.
    let clip = |value: &str, cap: usize| -> String {
        if value.chars().count() <= cap {
            value.trim().to_string()
        } else {
            value
                .chars()
                .take(cap)
                .collect::<String>()
                .trim()
                .to_string()
        }
    };
    let sev = |value: Option<&str>| -> String {
        let value = value.unwrap_or("info");
        match value {
            "healthy" | "attention" | "risk" | "info" => value.to_string(),
            _ => "info".to_string(),
        }
    };
    let opt_str = |value: Option<&serde_json::Value>, cap: usize| -> Option<String> {
        value
            .and_then(serde_json::Value::as_str)
            .map(|text| clip(text, cap))
            .filter(|text| !text.is_empty())
    };
    let component_vector = |value: Option<&serde_json::Value>| -> Option<Vec<String>> {
        let values = value?.as_array()?;
        if values.is_empty() || values.len() > CAP_DOMAIN_COMPONENTS {
            return None;
        }
        let mut seen = HashSet::new();
        let mut components = Vec::with_capacity(values.len());
        for value in values {
            let raw_component = value.as_str()?;
            if raw_component.is_empty()
                || raw_component.trim() != raw_component
                || raw_component.chars().count() > CAP_COMPONENT
                || !valid_components.contains(raw_component)
            {
                return None;
            }
            let component = raw_component.to_string();
            if seen.insert(component.clone()) {
                components.push(component);
            }
        }
        (!components.is_empty()).then_some(components)
    };

    let summary = opt_str(raw.get("summary"), CAP_SUMMARY);

    let mut lenses = std::collections::BTreeMap::new();
    if let Some(map) = raw.get("lenses").and_then(serde_json::Value::as_object) {
        const KNOWN: &[&str] = &[
            "bugs",
            "bus",
            "structural",
            "change",
            "health",
            "security",
            "domain",
            "explain",
        ];
        for (key, value) in map.iter().take(CAP_LENSES) {
            if KNOWN.contains(&key.as_str()) {
                if let Some(text) = value
                    .as_str()
                    .map(|t| clip(t, CAP_LENS))
                    .filter(|t| !t.is_empty())
                {
                    lenses.insert(key.clone(), text);
                }
            }
        }
    }

    let mut domains = Vec::new();
    if let Some(list) = raw.get("domains").and_then(serde_json::Value::as_array) {
        for item in list.iter().take(CAP_DOMAINS) {
            let name = opt_str(item.get("name"), CAP_DOMAIN_NAME);
            let Some(name) = name else { continue };
            let Some(components) = component_vector(item.get("components")) else {
                continue;
            };
            domains.push(AuditDomain {
                name,
                severity: sev(item.get("severity").and_then(serde_json::Value::as_str)),
                note: opt_str(item.get("note"), CAP_DOMAIN_NOTE),
                components,
            });
        }
    }

    let mut red_team = Vec::new();
    if let Some(list) = raw.get("red_team").and_then(serde_json::Value::as_array) {
        for item in list.iter().take(CAP_RED_TEAM) {
            let Some(title) = opt_str(item.get("title"), CAP_TITLE) else {
                continue;
            };
            let Some(vector) = component_vector(item.get("vector")) else {
                continue;
            };
            red_team.push(AuditRedTeam {
                title,
                severity: sev(item.get("severity").and_then(serde_json::Value::as_str)),
                scenario: opt_str(item.get("scenario"), CAP_SCENARIO),
                evidence: opt_str(item.get("evidence"), CAP_EVIDENCE),
                vector,
            });
        }
    }

    if summary.is_none() && lenses.is_empty() && domains.is_empty() && red_team.is_empty() {
        return None;
    }
    Some(AuditNarrative {
        summary,
        lenses,
        domains,
        red_team,
    })
}

fn audit_narrative_binding(
    root: &Path,
    impact: &ChangeImpactResponse,
    map: &ProjectMapResponse,
    deadline: Option<Instant>,
) -> Result<AuditNarrativeBinding, LensError> {
    let repository_identity = crate::facts::repository_identity_until(root, deadline)
        .unwrap_or_else(|_| {
            format!(
                "git-worktree:sha256:{}",
                crate::hex::encode(&Sha256::digest(root.to_string_lossy().as_bytes()))
            )
        });
    let map_bytes = serde_json::to_vec(map).map_err(|_| LensError::Serialization)?;
    Ok(AuditNarrativeBinding {
        repository_identity,
        baseline_oid: impact.baseline.baseline_oid.clone(),
        head_oid: impact.baseline.head_oid.clone(),
        snapshot_token_sha256: impact.snapshot_token.clone(),
        map_sha256: crate::hex::encode(&Sha256::digest(map_bytes)),
    })
}

fn audit_scope(map: &ProjectMapResponse) -> &str {
    if map.scope.path == "." {
        ""
    } else {
        &map.scope.path
    }
}

fn audit_path_in_scope(path: &str, map: &ProjectMapResponse) -> bool {
    let scope = audit_scope(map);
    match map.scope.kind.as_str() {
        "root" => true,
        "file" => path == scope,
        "directory" => path
            .strip_prefix(scope)
            .is_some_and(|suffix| suffix.starts_with('/')),
        _ => false,
    }
}

/// Distinct authors by selected map component over bounded git history.
/// Renames are not followed; git failure makes this advisory section unavailable.
fn authors_by_component(
    root: &Path,
    window_commits: u32,
    map: &ProjectMapResponse,
    production_only: bool,
    indexed_paths: &HashSet<String>,
    deadline: Option<Instant>,
) -> Option<Vec<LensComponentAuthors>> {
    const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
    let max_count = window_commits.to_string();
    let output = crate::diff::run_bounded_git_with_limit_until(
        root,
        &[
            "log",
            "--no-merges",
            "--format=%x00%an",
            "--name-only",
            "-n",
            &max_count,
        ],
        None,
        OUTPUT_LIMIT,
        deadline,
    )
    .ok()?;
    if !output.success {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut components: HashMap<String, HashMap<String, u32>> = HashMap::new();
    let mut author = String::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix('\u{0}') {
            author = name.to_string();
            continue;
        }
        let path = line.trim();
        if path.is_empty()
            || author.is_empty()
            || !audit_path_in_scope(path, map)
            || !indexed_paths.contains(path)
            || production_only && !crate::store::is_production_path(path)
        {
            continue;
        }
        let component =
            queries::component_for_file(audit_scope(map), &map.scope.kind, path, map.scope.depth);
        *components
            .entry(component)
            .or_default()
            .entry(author.clone())
            .or_insert(0) += 1;
    }
    let mut rows: Vec<LensComponentAuthors> = components
        .into_iter()
        .map(|(component, authors)| {
            let touches: u32 = authors.values().sum();
            let top = authors.values().copied().max().unwrap_or(0);
            let top_author_pct = if touches > 0 {
                ((u64::from(top) * 100) / u64::from(touches)) as u32
            } else {
                0
            };
            LensComponentAuthors {
                component,
                authors: authors.len() as u32,
                touches,
                top_author_pct,
            }
        })
        .collect();
    // Prefer concentrated ownership, then use activity as the tie-breaker.
    rows.sort_by(|a, b| {
        let a_solo = a.authors == 1;
        let b_solo = b.authors == 1;
        b_solo
            .cmp(&a_solo)
            .then_with(|| b.top_author_pct.cmp(&a.top_author_pct))
            .then_with(|| b.touches.cmp(&a.touches))
            .then_with(|| a.component.cmp(&b.component))
    });
    Some(rows)
}

/// Commits per file over bounded git history. Renames are intentionally not followed.
fn churn_by_file(
    root: &Path,
    window_commits: u32,
    map: &ProjectMapResponse,
    production_only: bool,
    indexed_paths: &HashSet<String>,
    deadline: Option<Instant>,
) -> Option<HashMap<String, u32>> {
    const CHURN_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
    let max_count = window_commits.to_string();
    let output = crate::diff::run_bounded_git_with_limit_until(
        root,
        &[
            "log",
            "--no-renames",
            "--no-merges",
            "--pretty=format:",
            "--name-only",
            "-n",
            &max_count,
        ],
        None,
        CHURN_OUTPUT_LIMIT,
        deadline,
    )
    .ok()?;
    if !output.success {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut counts: HashMap<String, u32> = HashMap::new();
    for line in text.lines() {
        let path = line.trim();
        if path.is_empty()
            || !audit_path_in_scope(path, map)
            || !indexed_paths.contains(path)
            || production_only && !crate::store::is_production_path(path)
        {
            continue;
        }
        *counts.entry(path.to_string()).or_insert(0) += 1;
    }
    Some(counts)
}

fn build_snapshot_until(
    store: &Store,
    root: &Path,
    options: &LensOptions,
    evidence_options: &crate::evidence::EvidenceOptions,
    evidence_extensions: &crate::evidence::EvidenceExtensionOptions,
    deadline: Option<Instant>,
) -> Result<LensSnapshot, LensError> {
    let root = root
        .canonicalize()
        .map_err(|_| LensError::RootUnavailable)?;
    let exhausted = store.push_work_budget(remaining_work_budget(deadline));
    let _budget_scope = WorkBudgetScope(store);
    if exhausted {
        return Err(LensError::AnalysisTimeout);
    }

    let index_version = store
        .data_version()
        .map_err(|_| LensError::ImpactUnavailable(ChangeImpactError::SnapshotChanged))?;
    let source_index_state = store
        .source_index_state()
        .map_err(|_| LensError::ImpactUnavailable(ChangeImpactError::SnapshotChanged))?;
    let indexed_paths = validated_index_paths(store, &root, deadline)?;

    let impact = queries::change_impact(
        store,
        &root,
        &options.since,
        u32::from(options.depth),
        options.top as usize,
    )
    .map_err(LensError::ImpactUnavailable)?;
    let map = match queries::project_map_with_options(
        store,
        &options.path,
        options.depth,
        options.top,
        options.production_only,
    ) {
        Ok(map) => map,
        Err(error)
            if error.contains("scope has no indexed files")
                && crate::temporal::scope_has_deleted_file(&impact, &options.path)
                    .map_err(LensError::MapUnavailable)? =>
        {
            queries::empty_project_map(&options.path, options.depth, options.production_only)
                .map_err(LensError::MapUnavailable)?
        }
        Err(error) => return Err(LensError::MapUnavailable(error)),
    };
    let temporal_options = crate::temporal::TemporalOptions {
        since: options.since.clone(),
        path: options.path.clone(),
        depth: options.depth,
        top: options.top,
        production_only: options.production_only,
        codeowners: evidence_options.codeowners.clone(),
    };
    let temporal = match crate::temporal::analyze_with_impact(
        store,
        &root,
        &temporal_options,
        &impact,
        Some(&map),
    ) {
        Ok(data) => LensTemporalSnapshot {
            status: "available",
            data: Some(data),
            diagnostic: None,
        },
        Err(crate::temporal::TemporalError::SnapshotChanged) => {
            return Err(LensError::ImpactUnavailable(
                ChangeImpactError::SnapshotChanged,
            ));
        }
        Err(crate::temporal::TemporalError::Impact(error)) => {
            return Err(LensError::ImpactUnavailable(error));
        }
        Err(error) => LensTemporalSnapshot {
            status: "unavailable",
            data: None,
            diagnostic: Some(LensTemporalDiagnostic {
                code: error.code(),
                message: "Temporal architecture could not be completed within its bounded snapshot contract.",
            }),
        },
    };
    let semantic_paths = impact
        .changes
        .files
        .items
        .iter()
        .map(|item| item.path.clone())
        .chain(
            impact
                .changes
                .symbols
                .items
                .iter()
                .map(|item| item.file.clone()),
        )
        .chain(
            impact
                .impact
                .items
                .iter()
                .map(|item| item.symbol.file.clone()),
        )
        .chain(
            impact
                .tests
                .items
                .iter()
                .map(|item| item.symbol.file.clone()),
        );
    let semantic = crate::scip_overlay::for_lens(store, &root, semantic_paths)
        .unwrap_or_else(|_| crate::scip_overlay::unavailable_with_diagnostic());
    let evidence = crate::evidence::collect_with_store_and_normalized_facts(
        &root,
        evidence_options,
        evidence_extensions,
        &impact,
        store,
        deadline,
    );

    let name = root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("repository")
        .to_string();
    let root_label = impact.scope.repository_relative_root.clone();
    if store
        .data_version()
        .map_err(|_| LensError::ImpactUnavailable(ChangeImpactError::SnapshotChanged))?
        != index_version
        || store
            .source_index_state()
            .map_err(|_| LensError::ImpactUnavailable(ChangeImpactError::SnapshotChanged))?
            != source_index_state
    {
        return Err(LensError::ImpactUnavailable(
            ChangeImpactError::SnapshotChanged,
        ));
    }

    const AUDIT_DEAD_CODE_CAP: usize = 100;
    let (dead_total, dead_symbols) = store
        .unreferenced_bounded(
            None,
            None,
            audit_scope(&map),
            &map.scope.kind,
            options.production_only,
            AUDIT_DEAD_CODE_CAP,
        )
        .map_err(|_| LensError::ImpactUnavailable(ChangeImpactError::SnapshotChanged))?;
    let dead_items = dead_symbols
        .into_iter()
        .map(crate::queries::SymbolHit::from)
        .collect::<Vec<_>>();
    let dead_truncated = dead_total > dead_items.len() as u32;
    const CHURN_WINDOW_COMMITS: u32 = 500;
    const CENTRALITY_ROWS: usize = 2000;
    const CHANGE_HOTSPOT_CAP: usize = 20;
    let change_hotspots = match churn_by_file(
        &root,
        CHURN_WINDOW_COMMITS,
        &map,
        options.production_only,
        &indexed_paths,
        deadline,
    ) {
        None => LensChangeHotspots {
            status: "unavailable",
            window_commits: 0,
            returned: 0,
            truncated: false,
            items: Vec::new(),
        },
        Some(churn) => match store.file_in_degrees_scoped(
            audit_scope(&map),
            &map.scope.kind,
            options.production_only,
            CENTRALITY_ROWS.saturating_add(1),
        ) {
            Err(_) => LensChangeHotspots {
                status: "unavailable",
                window_commits: 0,
                returned: 0,
                truncated: false,
                items: Vec::new(),
            },
            Ok(mut degrees) => {
                let centrality_truncated = degrees.len() > CENTRALITY_ROWS;
                degrees.truncate(CENTRALITY_ROWS);
                let mut ranked: Vec<LensChangeHotspot> = degrees
                    .into_iter()
                    .filter_map(|row| {
                        let commits = *churn.get(&row.file)?;
                        (commits > 0 && row.in_degree > 0).then(|| LensChangeHotspot {
                            score: u64::from(commits) * u64::from(row.in_degree),
                            file: row.file,
                            commits,
                            in_degree: row.in_degree,
                        })
                    })
                    .collect();
                ranked.sort_by(|left, right| {
                    right
                        .score
                        .cmp(&left.score)
                        .then_with(|| left.file.cmp(&right.file))
                });
                let result_truncated = ranked.len() > CHANGE_HOTSPOT_CAP;
                ranked.truncate(CHANGE_HOTSPOT_CAP);
                LensChangeHotspots {
                    status: "available",
                    window_commits: CHURN_WINDOW_COMMITS,
                    returned: ranked.len() as u32,
                    truncated: centrality_truncated || result_truncated,
                    items: ranked,
                }
            }
        },
    };

    const LARGEST_FILES_CAP: usize = 20;
    let largest_files = match store.largest_files_scoped(
        audit_scope(&map),
        &map.scope.kind,
        options.production_only,
        LARGEST_FILES_CAP.saturating_add(1),
    ) {
        Ok(mut items) => {
            let truncated = items.len() > LARGEST_FILES_CAP;
            items.truncate(LARGEST_FILES_CAP);
            LensLargestFiles {
                status: "available",
                returned: items.len() as u32,
                truncated,
                items,
            }
        }
        Err(_) => LensLargestFiles {
            status: "unavailable",
            returned: 0,
            truncated: false,
            items: Vec::new(),
        },
    };

    const BUS_FACTOR_WINDOW_COMMITS: u32 = 2000;
    const BUS_FACTOR_CAP: usize = 20;
    let bus_factor = match authors_by_component(
        &root,
        BUS_FACTOR_WINDOW_COMMITS,
        &map,
        options.production_only,
        &indexed_paths,
        deadline,
    ) {
        None => LensBusFactor {
            status: "unavailable",
            window_commits: 0,
            returned: 0,
            truncated: false,
            items: Vec::new(),
        },
        Some(mut rows) => {
            let truncated = rows.len() > BUS_FACTOR_CAP;
            rows.truncate(BUS_FACTOR_CAP);
            LensBusFactor {
                status: "available",
                window_commits: BUS_FACTOR_WINDOW_COMMITS,
                returned: rows.len() as u32,
                truncated,
                items: rows,
            }
        }
    };

    let narrative_binding = audit_narrative_binding(&root, &impact, &map, deadline)?;
    let valid_components = map
        .components
        .items
        .iter()
        .map(|component| component.path.clone())
        .collect::<HashSet<_>>();
    let narrative = read_audit_narrative(&root, &narrative_binding, &valid_components);

    let audit = LensAudit {
        dead_code: LensDeadCode {
            total: dead_total,
            returned: dead_items.len() as u32,
            truncated: dead_truncated,
            items: dead_items,
        },
        change_hotspots,
        largest_files,
        bus_factor,
        narrative_binding,
        narrative,
    };

    if store
        .data_version()
        .map_err(|_| LensError::ImpactUnavailable(ChangeImpactError::SnapshotChanged))?
        != index_version
        || store
            .source_index_state()
            .map_err(|_| LensError::ImpactUnavailable(ChangeImpactError::SnapshotChanged))?
            != source_index_state
    {
        return Err(LensError::ImpactUnavailable(
            ChangeImpactError::SnapshotChanged,
        ));
    }

    Ok(LensSnapshot {
        schema_version: 1,
        repository: LensRepository { name, root_label },
        options: options.clone(),
        map,
        impact,
        temporal,
        semantic,
        evidence,
        audit,
    })
}

pub fn run(
    root: PathBuf,
    index_path: PathBuf,
    options: LensOptions,
    port: u16,
) -> Result<(), LensError> {
    run_with_evidence(
        root,
        index_path,
        options,
        crate::evidence::EvidenceOptions::default(),
        port,
    )
}

pub fn run_with_evidence(
    root: PathBuf,
    index_path: PathBuf,
    options: LensOptions,
    evidence: crate::evidence::EvidenceOptions,
    port: u16,
) -> Result<(), LensError> {
    run_with_evidence_extensions(
        root,
        index_path,
        options,
        evidence,
        crate::evidence::EvidenceExtensionOptions::default(),
        port,
    )
}

pub fn run_with_evidence_extensions(
    root: PathBuf,
    index_path: PathBuf,
    options: LensOptions,
    evidence: crate::evidence::EvidenceOptions,
    extensions: crate::evidence::EvidenceExtensionOptions,
    port: u16,
) -> Result<(), LensError> {
    let root = root
        .canonicalize()
        .map_err(|_| LensError::RootUnavailable)?;
    let index_path = index_path
        .canonicalize()
        .map_err(|_| LensError::IndexUnavailable)?;
    if !index_path.is_file() {
        return Err(LensError::IndexUnavailable);
    }

    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| LensError::Bind(error.to_string()))?;
    let address = listener
        .local_addr()
        .map_err(|error| LensError::Bind(error.to_string()))?;
    let authority = format!("127.0.0.1:{}", address.port());
    println!("Mastermind Lens: http://{authority}/");
    println!("Local and read-only. Press Ctrl-C to stop.");

    let state = ServerState {
        root,
        index_path,
        options,
        evidence,
        extensions,
        authority,
    };
    serve(listener, &state, None)
}

struct ServerState {
    root: PathBuf,
    index_path: PathBuf,
    options: LensOptions,
    evidence: crate::evidence::EvidenceOptions,
    extensions: crate::evidence::EvidenceExtensionOptions,
    authority: String,
}

#[derive(Debug, PartialEq, Eq)]
struct IndexSourceState {
    database: FileState,
    wal: Option<FileState>,
}

#[derive(Debug, PartialEq, Eq)]
struct FileState {
    len: u64,
    modified: Option<SystemTime>,
}

fn sidecar_path(index_path: &Path, suffix: &str) -> PathBuf {
    let mut value = index_path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn file_state(path: &Path) -> Result<FileState, LensError> {
    let metadata = std::fs::metadata(path).map_err(|_| LensError::IndexUnavailable)?;
    Ok(FileState {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn index_source_state(index_path: &Path) -> Result<IndexSourceState, LensError> {
    let index_path = index_path
        .canonicalize()
        .map_err(|_| LensError::IndexUnavailable)?;
    let wal_path = sidecar_path(&index_path, "-wal");
    let wal = match std::fs::metadata(&wal_path) {
        Ok(metadata) => Some(FileState {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(LensError::IndexUnavailable),
    };
    Ok(IndexSourceState {
        database: file_state(&index_path)?,
        wal,
    })
}

fn read_only_open_error(error: rusqlite::Error) -> LensError {
    match error.sqlite_error_code() {
        Some(rusqlite::ffi::ErrorCode::DatabaseBusy) => {
            LensError::ImpactUnavailable(ChangeImpactError::SnapshotChanged)
        }
        Some(rusqlite::ffi::ErrorCode::OperationInterrupted) => LensError::SnapshotTimeout,
        Some(rusqlite::ffi::ErrorCode::TooBig) => LensError::SnapshotTooLarge,
        _ => LensError::IndexUnavailable,
    }
}

fn serve(
    listener: TcpListener,
    state: &ServerState,
    request_limit: Option<usize>,
) -> Result<(), LensError> {
    for (handled, incoming) in listener.incoming().enumerate() {
        let mut stream = incoming.map_err(|error| LensError::Serve(error.to_string()))?;
        if let Err(error) = handle_connection(&mut stream, state) {
            eprintln!("mastermind lens: {error}");
        }
        if request_limit.is_some_and(|limit| handled.saturating_add(1) >= limit) {
            break;
        }
    }
    Ok(())
}

fn handle_connection(stream: &mut TcpStream, state: &ServerState) -> Result<(), LensError> {
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|error| LensError::Serve(error.to_string()))?;
    stream
        .set_write_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|error| LensError::Serve(error.to_string()))?;

    let raw = read_request(stream)?;
    let request = match HttpRequest::parse(&raw) {
        Ok(request) => request,
        Err(error) => {
            return write_response(
                stream,
                &HttpResponse::text(400, "Bad Request", error),
                false,
            )
        }
    };
    if let Err(error) = request.validate(&state.authority) {
        return write_response(
            stream,
            &HttpResponse::text(error.status, error.reason, error.message),
            request.method == "HEAD",
        );
    }

    let response = route(&request.path, state);
    write_response(stream, &response, request.method == "HEAD")
}

fn read_request(stream: &mut TcpStream) -> Result<Vec<u8>, LensError> {
    let mut request = Vec::with_capacity(1024);
    let mut buffer = [0u8; 1024];
    loop {
        let read = stream
            .read(&mut buffer)
            .map_err(|error| LensError::Serve(error.to_string()))?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() > MAX_REQUEST_HEADER_BYTES {
            return Err(LensError::Serve("request headers exceed 16 KiB".into()));
        }
    }
    if request.len() > MAX_REQUEST_HEADER_BYTES {
        return Err(LensError::Serve("request headers exceed 16 KiB".into()));
    }
    Ok(request)
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl HttpRequest {
    fn parse(raw: &[u8]) -> Result<Self, &'static str> {
        let text = std::str::from_utf8(raw).map_err(|_| "request headers must be UTF-8")?;
        let headers_end = text
            .find("\r\n\r\n")
            .ok_or("request headers are incomplete")?;
        let mut lines = text[..headers_end].split("\r\n");
        let mut start = lines
            .next()
            .ok_or("request line is missing")?
            .split_whitespace();
        let method = start.next().ok_or("method is missing")?;
        let target = start.next().ok_or("target is missing")?;
        let version = start.next().ok_or("HTTP version is missing")?;
        if start.next().is_some() || version != "HTTP/1.1" {
            return Err("only HTTP/1.1 requests are accepted");
        }
        if !target.starts_with('/') || target.starts_with("//") {
            return Err("request target must be origin-form");
        }
        let path = target.split_once('?').map_or(target, |(path, _)| path);
        let mut headers = Vec::new();
        for line in lines {
            let (name, value) = line.split_once(':').ok_or("malformed request header")?;
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err("malformed request header name");
            }
            if value.chars().any(char::is_control) {
                return Err("request header contains control characters");
            }
            headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
        }
        Ok(Self {
            method: method.to_string(),
            path: path.to_string(),
            headers,
        })
    }

    fn validate(&self, authority: &str) -> Result<(), RequestRejection> {
        if self.method != "GET" && self.method != "HEAD" {
            return Err(RequestRejection::new(
                405,
                "Method Not Allowed",
                "Lens is read-only; only GET and HEAD are accepted",
            ));
        }
        let hosts = self.header_values("host");
        if hosts.len() != 1 || hosts[0] != authority {
            return Err(RequestRejection::new(
                421,
                "Misdirected Request",
                "request Host does not match the Lens loopback origin",
            ));
        }
        let origins = self.header_values("origin");
        let expected_origin = format!("http://{authority}");
        if origins.len() > 1
            || origins
                .first()
                .is_some_and(|origin| *origin != expected_origin)
        {
            return Err(RequestRejection::new(
                403,
                "Forbidden",
                "cross-origin requests are not accepted",
            ));
        }
        let fetch_sites = self.header_values("sec-fetch-site");
        if fetch_sites.len() > 1
            || fetch_sites
                .first()
                .is_some_and(|site| *site != "same-origin" && *site != "none")
        {
            return Err(RequestRejection::new(
                403,
                "Forbidden",
                "cross-site requests are not accepted",
            ));
        }
        Ok(())
    }

    fn header_values(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }
}

struct RequestRejection {
    status: u16,
    reason: &'static str,
    message: &'static str,
}

impl RequestRejection {
    fn new(status: u16, reason: &'static str, message: &'static str) -> Self {
        Self {
            status,
            reason,
            message,
        }
    }
}

struct HttpResponse {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
    extra_headers: Vec<(&'static str, &'static str)>,
}

impl HttpResponse {
    fn static_asset(content_type: &'static str, body: &'static str) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type,
            body: body.as_bytes().to_vec(),
            extra_headers: Vec::new(),
        }
    }

    fn text(status: u16, reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            reason,
            content_type: "text/plain; charset=utf-8",
            body: message.into().into_bytes(),
            extra_headers: Vec::new(),
        }
    }

    fn json(status: u16, reason: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            reason,
            content_type: "application/json; charset=utf-8",
            body,
            extra_headers: Vec::new(),
        }
    }

    fn empty(status: u16, reason: &'static str) -> Self {
        Self {
            status,
            reason,
            content_type: "text/plain; charset=utf-8",
            body: Vec::new(),
            extra_headers: Vec::new(),
        }
    }
}

fn route(path: &str, state: &ServerState) -> HttpResponse {
    match path {
        "/" | "/index.html" => HttpResponse::static_asset("text/html; charset=utf-8", INDEX_HTML),
        "/app.js" => HttpResponse::static_asset("text/javascript; charset=utf-8", APP_JS),
        "/styles.css" => HttpResponse::static_asset("text/css; charset=utf-8", STYLES_CSS),
        "/mastermind-mark.svg" => {
            HttpResponse::static_asset("image/svg+xml; charset=utf-8", MASTERMIND_MARK_SVG)
        }
        "/api/lens" => api_response(state),
        "/favicon.ico" => HttpResponse::empty(204, "No Content"),
        _ => HttpResponse::text(404, "Not Found", "not found"),
    }
}

fn api_response(state: &ServerState) -> HttpResponse {
    let deadline = request_deadline();
    let result = snapshot_from_paths_until(
        &state.root,
        &state.index_path,
        &state.options,
        &state.evidence,
        &state.extensions,
        deadline,
    );
    match result {
        Ok(snapshot) => match serde_json::to_vec(&snapshot) {
            Ok(body) => HttpResponse::json(200, "OK", body),
            Err(_) => error_response(&LensError::Serialization),
        },
        Err(error) => error_response(&error),
    }
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: String,
}

fn error_response(error: &LensError) -> HttpResponse {
    let status = match error {
        LensError::IndexUnavailable | LensError::RootUnavailable => 409,
        LensError::IndexStale
        | LensError::SnapshotTooLarge
        | LensError::MapUnavailable(_)
        | LensError::ImpactUnavailable(_) => 422,
        LensError::SnapshotTimeout | LensError::AnalysisTimeout => 503,
        LensError::Serialization | LensError::Bind(_) | LensError::Serve(_) => 500,
    };
    let envelope = ErrorEnvelope {
        error: ErrorBody {
            code: error.code(),
            message: error.public_message(),
        },
    };
    match serde_json::to_vec(&envelope) {
        Ok(body) => HttpResponse::json(status, "Lens Error", body),
        Err(_) => HttpResponse::text(500, "Internal Server Error", "Lens error"),
    }
}

fn write_response(
    stream: &mut TcpStream,
    response: &HttpResponse,
    head_only: bool,
) -> Result<(), LensError> {
    let mut headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nContent-Security-Policy: {CSP}\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Resource-Policy: same-origin\r\nX-Frame-Options: DENY\r\n",
        response.status,
        response.reason,
        response.content_type,
        response.body.len(),
    );
    for (name, value) in &response.extra_headers {
        headers.push_str(name);
        headers.push_str(": ");
        headers.push_str(value);
        headers.push_str("\r\n");
    }
    headers.push_str("\r\n");
    stream
        .write_all(headers.as_bytes())
        .map_err(|error| LensError::Serve(error.to_string()))?;
    if !head_only {
        stream
            .write_all(&response.body)
            .map_err(|error| LensError::Serve(error.to_string()))?;
    }
    stream
        .flush()
        .map_err(|error| LensError::Serve(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::Indexer;
    use std::fs;
    use std::process::Command;

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

    fn fixture() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
        let repo = tempfile::tempdir().unwrap();
        let index_dir = tempfile::tempdir().unwrap();
        fs::create_dir(repo.path().join("src")).unwrap();
        fs::write(
            repo.path().join("src/lib.rs"),
            "pub fn seed() -> i32 { 1 }\npub fn caller() -> i32 { seed() }\n",
        )
        .unwrap();
        git(repo.path(), &["init", "-q"]);
        git(repo.path(), &["config", "user.email", "lens@example.test"]);
        git(repo.path(), &["config", "user.name", "Lens Test"]);
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "baseline"]);

        fs::write(
            repo.path().join("src/lib.rs"),
            "pub fn seed() -> i32 { 2 }\npub fn caller() -> i32 { seed() }\n",
        )
        .unwrap();
        let index_path = index_dir.path().join("mmcg.db");
        let mut store = Store::open(&index_path).unwrap();
        Indexer::new(repo.path())
            .index_all(&mut store, false)
            .unwrap();
        drop(store);
        (repo, index_dir, index_path)
    }

    fn options() -> LensOptions {
        LensOptions {
            since: "HEAD".into(),
            path: ".".into(),
            depth: 3,
            top: 100,
            production_only: false,
        }
    }

    #[test]
    fn standalone_html_is_single_file_offline_and_script_safe() {
        let html = standalone_html_from_json(
            r#"{"repository":{"name":"</script><img src=x onerror=alert(1)>"}}"#,
        )
        .unwrap();
        let html = String::from_utf8(html).unwrap();

        assert!(html.contains("id=\"lens-snapshot\""));
        assert!(html.contains("\\u003c/script\\u003e"));
        assert!(!html.contains("</script><img"));
        assert!(!html.contains("href=\"styles.css\""));
        assert!(!html.contains("src=\"app.js\""));
        assert!(!html.contains("src=\"mastermind-mark.svg\""));
        assert!(html.contains("src=\"data:image/svg+xml;base64,"));
        assert!(html.contains("connect-src 'none'"));
        assert!(html.contains("style-src-attr 'unsafe-inline'"));
        assert!(html.contains("sha256-"));
        assert!(html.contains(
            "<span data-lens-runtime-label><i aria-hidden=\"true\"></i>Offline package</span>"
        ));
        assert!(html.contains(
            "<button class=\"refresh-button\" id=\"refresh-button\" type=\"button\" data-lens-snapshot-action aria-label=\"Static Lens snapshot\""
        ));
        assert!(html.contains("<span data-lens-action-label>Static snapshot</span>"));
        assert!(!html.contains("data-lens-runtime-label><i aria-hidden=\"true\"></i>Local<"));
        assert!(!html.contains("data-lens-action-label>Refresh<"));
        assert!(!html.contains("content: \"Local\""));
    }

    #[test]
    fn standalone_html_accepts_crlf_checkout_assets() {
        let crlf_template = INDEX_HTML.replace("\r\n", "\n").replace('\n', "\r\n");

        let html = standalone_html_from_template("{}", &crlf_template).unwrap();
        let html = String::from_utf8(html).unwrap();

        assert!(html.contains("id=\"lens-snapshot\""));
        assert!(html.contains("connect-src 'none'"));
        assert!(!html.contains("href=\"styles.css\""));
        assert!(!html.contains("src=\"app.js\""));
    }

    fn directory_snapshot(path: &Path) -> Vec<(String, Vec<u8>)> {
        let mut files = fs::read_dir(path)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files
    }

    fn directory_names(path: &Path) -> Vec<String> {
        let mut names = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn snapshot_wraps_the_shared_map_and_impact_schemas() {
        let (repo, _index_dir, index_path) = fixture();
        let store = Store::open_read_only(&index_path).unwrap();
        let snapshot = build_snapshot(&store, repo.path(), &options()).unwrap();
        let raw_impact = serde_json::to_value(&snapshot.impact).unwrap();
        let json = serde_json::to_value(&snapshot).unwrap();

        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["evidence"]["schema_version"], 1);
        assert_eq!(json["evidence"]["sources"]["returned"], 0);
        assert_eq!(json["evidence"]["files"]["returned"], 0);
        assert_eq!(json["map"]["schema_version"], 1);
        assert_eq!(json["impact"]["schema_version"], 1);
        assert_eq!(json["temporal"]["status"], "available");
        assert_eq!(json["temporal"]["data"]["schema_version"], 1);
        assert_eq!(
            json["temporal"]["data"]["provenance"]["baseline_graph"],
            "git_blob_rewind_private_sqlite_snapshot"
        );
        assert_eq!(json["options"]["since"], "HEAD");
        assert_eq!(json["impact"]["changes"]["files"]["returned"], 1);
        assert_eq!(
            json["impact"]["changes"]["files"]["items"][0]["path"],
            "src/lib.rs"
        );
        assert_eq!(json["impact"], raw_impact);
    }

    #[test]
    fn serialized_snapshot_projects_an_already_truncated_changed_file_collection() {
        let (repo, _index_dir, index_path) = fixture();
        let store = Store::open_read_only(&index_path).unwrap();
        let mut snapshot = build_snapshot(&store, repo.path(), &options()).unwrap();
        let sample = snapshot.impact.changes.files.items[0].clone();
        snapshot.impact.changes.files = queries::Collection {
            total: None,
            returned: 10_000,
            truncated: true,
            truncation_reason: Some("file_limit".into()),
            items: vec![sample; 10_000],
        };

        let bytes = serde_json::to_vec(&snapshot).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let files = &json["impact"]["changes"]["files"];

        assert_eq!(
            files["items"].as_array().unwrap().len(),
            LENS_CHANGED_FILE_ITEM_LIMIT
        );
        assert_eq!(files["returned"], LENS_CHANGED_FILE_ITEM_LIMIT as u64);
        assert_eq!(files["observed"], 10_000);
        assert_eq!(files["total"], serde_json::Value::Null);
        assert_eq!(files["truncated"], true);
        assert_eq!(files["truncation_reason"], "file_limit");
        assert_eq!(files["projection_truncated"], true);
        assert_eq!(files["projection_reason"], "lens_payload_limit");
        assert_eq!(snapshot.impact.changes.files.items.len(), 10_000);
        assert!(
            bytes.len() < 100_000,
            "projected payload was {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn snapshot_carries_selected_scope_dead_code_candidates() {
        let (repo, _index_dir, index_path) = fixture();
        let store = Store::open_read_only(&index_path).unwrap();
        let snapshot = build_snapshot(&store, repo.path(), &options()).unwrap();
        let json = serde_json::to_value(snapshot).unwrap();

        let dead = &json["audit"]["dead_code"];
        assert!(
            dead["total"].as_u64().unwrap() >= 1,
            "expected a dead-code candidate"
        );
        assert_eq!(
            dead["returned"],
            dead["items"].as_array().unwrap().len() as u64
        );
        assert_eq!(dead["truncated"], false);
        let names: Vec<&str> = dead["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"caller"),
            "unreferenced caller must be listed, got {names:?}"
        );
        assert!(
            !names.contains(&"seed"),
            "a referenced symbol must not be listed as dead"
        );
    }

    #[test]
    fn snapshot_applies_scope_and_production_policy_before_audit_caps() {
        let (repo, _index_dir, index_path) = fixture();
        fs::create_dir(repo.path().join("tests")).unwrap();
        fs::write(
            repo.path().join("tests/dead.rs"),
            format!(
                "{}pub fn test_only_helper() -> i32 {{ 7 }}\n",
                "\n".repeat(50)
            ),
        )
        .unwrap();
        for index in 0..21 {
            fs::write(
                repo.path().join(format!("src/extra_{index:02}.rs")),
                format!("pub fn extra_{index:02}() -> i32 {{ {index} }}\n"),
            )
            .unwrap();
        }
        let mut writable = Store::open(&index_path).unwrap();
        Indexer::new(repo.path())
            .index_all(&mut writable, false)
            .unwrap();
        drop(writable);
        let store = Store::open_read_only(&index_path).unwrap();

        let all =
            serde_json::to_value(build_snapshot(&store, repo.path(), &options()).unwrap()).unwrap();
        assert_eq!(all["audit"]["largest_files"]["returned"], 20);
        assert_eq!(all["audit"]["largest_files"]["truncated"], true);
        assert!(all["audit"]["largest_files"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["file"] == "tests/dead.rs"));

        let mut production = options();
        production.production_only = true;
        let production =
            serde_json::to_value(build_snapshot(&store, repo.path(), &production).unwrap())
                .unwrap();
        for section in ["dead_code", "largest_files"] {
            assert!(production["audit"][section]["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| !item["file"].as_str().unwrap().starts_with("tests/")));
        }
        assert!(production["map"]["components"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["path"] != "tests"));

        let mut selected = options();
        selected.path = "tests".to_string();
        let selected =
            serde_json::to_value(build_snapshot(&store, repo.path(), &selected).unwrap()).unwrap();
        for section in ["dead_code", "largest_files"] {
            assert!(selected["audit"][section]["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["file"].as_str().unwrap().starts_with("tests/")));
        }
    }

    #[test]
    fn snapshot_ranks_change_hotspots_by_churn_times_centrality() {
        let (repo, _index_dir, index_path) = fixture();
        let store = Store::open_read_only(&index_path).unwrap();
        let snapshot = build_snapshot(&store, repo.path(), &options()).unwrap();
        let json = serde_json::to_value(snapshot).unwrap();

        let hotspots = &json["audit"]["change_hotspots"];
        assert_eq!(
            hotspots["status"], "available",
            "the fixture has git history"
        );
        let items = hotspots["items"].as_array().unwrap();
        assert_eq!(hotspots["returned"], items.len() as u64);
        let entry = items
            .iter()
            .find(|item| item["file"] == "src/lib.rs")
            .expect("src/lib.rs is both changed and depended on");
        let commits = entry["commits"].as_u64().unwrap();
        let in_degree = entry["in_degree"].as_u64().unwrap();
        assert!(commits >= 1 && in_degree >= 1, "both axes must be non-zero");
        assert_eq!(entry["score"].as_u64().unwrap(), commits * in_degree);
        let scores: Vec<u64> = items
            .iter()
            .map(|item| item["score"].as_u64().unwrap())
            .collect();
        assert!(
            scores.windows(2).all(|pair| pair[0] >= pair[1]),
            "change hotspots must be ranked by score, got {scores:?}"
        );
    }

    #[test]
    fn snapshot_requires_a_bound_and_grounded_ai_narrative_sidecar() {
        let (repo, _index_dir, index_path) = fixture();
        let mastermind = repo.path().join(".mastermind");
        fs::create_dir_all(&mastermind).unwrap();
        let store = Store::open_read_only(&index_path).unwrap();
        let initial =
            serde_json::to_value(build_snapshot(&store, repo.path(), &options()).unwrap()).unwrap();
        let binding = initial["audit"]["narrative_binding"].clone();
        let component = initial["map"]["components"]["items"][0]["path"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            binding["map_sha256"].as_str().unwrap().len(),
            64,
            "the sidecar contract binds the exact returned map"
        );
        let long = "x".repeat(9000);
        let sidecar = serde_json::json!({
            "schema_version": 1,
            "binding": binding,
            "summary": long,
            "lenses": { "bugs": "Review the largest indexed files.", "bogus": "ignored key" },
            "domains": [
                { "name": "Auth & tenancy", "note": "compliance-critical", "components": [component] },
                { "name": "Unknown domain", "severity": "risk", "components": ["ghost"] }
            ],
            "red_team": [
                { "title": "AI tool reaches the DB", "scenario": "s", "evidence": "MultiAgentBot", "vector": [component] },
                { "title": "Ungrounded guess", "severity": "risk", "scenario": "unknown component", "vector": ["ghost"] }
            ]
        });
        fs::write(
            mastermind.join("audit-narrative.json"),
            serde_json::to_vec(&sidecar).unwrap(),
        )
        .unwrap();

        let json =
            serde_json::to_value(build_snapshot(&store, repo.path(), &options()).unwrap()).unwrap();
        let n = &json["audit"]["narrative"];
        assert!(!n.is_null(), "a valid sidecar must be ingested");
        assert_eq!(
            n["summary"].as_str().unwrap().chars().count(),
            2000,
            "summary is capped"
        );
        assert_eq!(n["lenses"]["bugs"], "Review the largest indexed files.");
        assert!(
            n["lenses"].get("bogus").is_none(),
            "unknown lens keys are dropped"
        );
        assert_eq!(n["domains"].as_array().unwrap().len(), 1);
        assert_eq!(n["domains"][0]["name"], "Auth & tenancy");
        assert_eq!(
            n["domains"][0]["severity"], "info",
            "missing severity must safely default to info"
        );
        assert_eq!(
            n["red_team"].as_array().unwrap().len(),
            1,
            "a hypothesis with an unknown component is dropped"
        );
        assert_eq!(n["red_team"][0]["title"], "AI tool reaches the DB");
        assert_eq!(n["red_team"][0]["severity"], "info");
        assert_eq!(n["red_team"][0]["evidence"], "MultiAgentBot");
        assert_eq!(
            n["red_team"][0]["vector"][0], initial["map"]["components"]["items"][0]["path"],
            "only an exact returned component path is traceable"
        );

        let mut stale_binding = initial["audit"]["narrative_binding"].clone();
        stale_binding["head_oid"] = serde_json::Value::String("0".repeat(40));
        fs::write(
            mastermind.join("audit-narrative.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "binding": stale_binding,
                "summary": "stale"
            }))
            .unwrap(),
        )
        .unwrap();
        let stale =
            serde_json::to_value(build_snapshot(&store, repo.path(), &options()).unwrap()).unwrap();
        assert!(
            stale["audit"].get("narrative").is_none(),
            "a sidecar for another snapshot must be rejected"
        );

        fs::write(
            mastermind.join("audit-narrative.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 7,
                "binding": initial["audit"]["narrative_binding"],
                "summary": "nope"
            }))
            .unwrap(),
        )
        .unwrap();
        let json2 =
            serde_json::to_value(build_snapshot(&store, repo.path(), &options()).unwrap()).unwrap();
        assert!(
            json2["audit"].get("narrative").is_none(),
            "unknown schema yields no narrative"
        );
    }

    #[test]
    fn audit_narrative_schema_requires_the_runtime_binding() {
        let schema_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../schemas/mastermind-audit-narrative-v1.schema.json");
        if !schema_path.is_file() {
            return;
        }
        let schema: serde_json::Value =
            serde_json::from_slice(&fs::read(schema_path).unwrap()).unwrap();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["schema_version"]["const"], 1);
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "binding"));
        assert_eq!(schema["$defs"]["binding"]["additionalProperties"], false);
    }

    #[test]
    fn snapshot_reports_largest_files_and_bus_factor() {
        let (repo, _index_dir, index_path) = fixture();
        let store = Store::open_read_only(&index_path).unwrap();
        let snapshot = build_snapshot(&store, repo.path(), &options()).unwrap();
        let json = serde_json::to_value(snapshot).unwrap();

        assert_eq!(json["audit"]["largest_files"]["status"], "available");
        let files: Vec<&str> = json["audit"]["largest_files"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["file"].as_str().unwrap())
            .collect();
        assert!(
            files.contains(&"src/lib.rs"),
            "largest files must list the indexed file, got {files:?}"
        );

        let bus = &json["audit"]["bus_factor"];
        assert_eq!(bus["status"], "available", "the fixture has git history");
        let src = bus["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["component"] == "src")
            .expect("the src component must appear");
        assert_eq!(src["authors"].as_u64().unwrap(), 1);
        assert_eq!(src["top_author_pct"].as_u64().unwrap(), 100);
    }

    #[test]
    fn snapshot_explains_a_fully_deleted_selected_scope() {
        let repo = tempfile::tempdir().unwrap();
        let index_dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(repo.path().join("legacy")).unwrap();
        fs::write(
            repo.path().join("legacy/api.py"),
            "def legacy_api():\n    return 1\n",
        )
        .unwrap();
        git(repo.path(), &["init", "-q"]);
        git(repo.path(), &["config", "user.email", "lens@example.test"]);
        git(repo.path(), &["config", "user.name", "Lens Test"]);
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "baseline"]);
        fs::remove_file(repo.path().join("legacy/api.py")).unwrap();

        let index_path = index_dir.path().join("mmcg.db");
        let mut writable = Store::open(&index_path).unwrap();
        Indexer::new(repo.path())
            .index_all(&mut writable, true)
            .unwrap();
        drop(writable);
        let store = Store::open_read_only(&index_path).unwrap();
        let mut lens_options = options();
        lens_options.path = "legacy".to_string();

        let snapshot = build_snapshot(&store, repo.path(), &lens_options).unwrap();
        let json = serde_json::to_value(snapshot).unwrap();

        assert_eq!(json["map"]["files"]["total"], 0);
        assert_eq!(json["temporal"]["status"], "available");
        assert_eq!(
            json["temporal"]["data"]["components"]["removed"]["items"][0]["path"],
            "."
        );

        lens_options.path = "typo/never/existed".to_string();
        let error = build_snapshot(&store, repo.path(), &lens_options)
            .expect_err("a scope absent from both snapshots must not look clean");
        assert!(matches!(error, LensError::MapUnavailable(_)));

        fs::create_dir_all(repo.path().join("docs-only")).unwrap();
        fs::write(repo.path().join("docs-only/README.md"), "temporary docs\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "docs baseline"]);
        fs::remove_file(repo.path().join("docs-only/README.md")).unwrap();
        drop(store);
        let mut writable = Store::open(&index_path).unwrap();
        Indexer::new(repo.path())
            .index_all(&mut writable, true)
            .unwrap();
        drop(writable);
        let store = Store::open_read_only(&index_path).unwrap();
        lens_options.since = "HEAD".to_string();
        lens_options.path = "docs-only".to_string();
        let error = build_snapshot(&store, repo.path(), &lens_options)
            .expect_err("a deleted non-source must not prove a deleted architecture scope");
        assert!(matches!(error, LensError::MapUnavailable(_)));
    }

    #[test]
    fn snapshot_adds_read_only_evidence_for_returned_trace_files() {
        let (repo, _index_dir, index_path) = fixture();
        fs::create_dir_all(repo.path().join(".github")).unwrap();
        fs::write(
            repo.path().join("semgrep.sarif"),
            serde_json::to_vec(&serde_json::json!({
                "version": "2.1.0",
                "runs": [{
                    "tool": {"driver": {"name": "Semgrep"}},
                    "results": [{
                        "ruleId": "rust.changed-seed",
                        "level": "warning",
                        "message": {"text": "Changed seed requires review"},
                        "locations": [{"physicalLocation": {
                            "artifactLocation": {"uri": "src/lib.rs"},
                            "region": {"startLine": 1}
                        }}]
                    }]
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            repo.path().join("lcov.info"),
            "SF:src/lib.rs\nDA:1,1\nDA:2,0\nend_of_record\n",
        )
        .unwrap();
        fs::write(
            repo.path().join(".github/CODEOWNERS"),
            "/src/** @rust-team\n",
        )
        .unwrap();
        let source_paths = [
            repo.path().join("semgrep.sarif"),
            repo.path().join("lcov.info"),
            repo.path().join(".github/CODEOWNERS"),
        ];
        let source_bytes = source_paths
            .iter()
            .map(fs::read)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let root_entries = directory_names(repo.path());
        let github_entries = directory_names(&repo.path().join(".github"));
        let lens_options = options();
        let evidence_options = crate::evidence::EvidenceOptions {
            sarif: vec![PathBuf::from("semgrep.sarif")],
            coverage: vec![PathBuf::from("lcov.info")],
            codeowners: None,
            discover_codeowners: true,
            git_commits: 10,
        };

        let store = Store::open_read_only(index_path).unwrap();
        let snapshot =
            build_snapshot_with_evidence(&store, repo.path(), &lens_options, &evidence_options)
                .unwrap();
        let json = serde_json::to_value(snapshot).unwrap();

        assert_eq!(json["evidence"]["sources"]["returned"], 4);
        let file = json["evidence"]["files"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["path"] == "src/lib.rs")
            .unwrap();
        assert_eq!(file["findings"][0]["tool"], "Semgrep");
        assert_eq!(file["coverage"]["lines_found"], 2);
        assert_eq!(file["coverage"]["lines_hit"], 1);
        assert_eq!(file["ownership"]["codeowners"][0], "@rust-team");
        assert!(file["churn"]["commits"].as_u64().unwrap() >= 1);
        assert_eq!(directory_names(repo.path()), root_entries);
        assert_eq!(
            directory_names(&repo.path().join(".github")),
            github_entries
        );
        assert_eq!(
            source_paths
                .iter()
                .map(fs::read)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            source_bytes
        );
    }

    #[test]
    fn snapshot_reads_revision_bound_facts_without_adding_graph_topology() {
        let (repo, _index_dir, index_path) = fixture();
        let writable = Store::open(&index_path).unwrap();
        let contract = crate::facts::contract(&writable).unwrap();
        let source = fs::read(repo.path().join("src/lib.rs")).unwrap();
        let source_sha256 = crate::hex::encode(&Sha256::digest(&source));
        let manifest_path = repo.path().join("facts.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "api_version": crate::facts::API_VERSION,
                "capabilities": ["annotations", "relationships"],
                "repository": {
                    "identity": contract.repository.identity,
                    "revision": contract.repository.revision
                },
                "producer": {"name": "com.example.lens", "version": "1.0.0"},
                "dataset": "review",
                "provenance": {"kind": "static-analysis", "artifacts": []},
                "files": [{
                    "path": "src/lib.rs",
                    "sha256": source_sha256,
                    "bytes": source.len()
                }],
                "artifacts": [],
                "facts": [
                    {
                        "kind": "annotation",
                        "id": "review.seed",
                        "path": "src/lib.rs",
                        "line": 1,
                        "severity": "warning",
                        "category": "architecture.review",
                        "title": "Seed changed",
                        "message": "Review the changed seed contract."
                    },
                    {
                        "kind": "relationship",
                        "id": "review.seed-caller",
                        "relation": "calls",
                        "from": {"path": "src/lib.rs", "line": 2},
                        "to": {"path": "src/lib.rs", "line": 1},
                        "confidence": "high",
                        "label": "Caller reaches the changed seed"
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        crate::facts::import(&writable, &manifest_path).unwrap();
        drop(writable);

        let store = Store::open_read_only(&index_path).unwrap();
        let snapshot = build_snapshot(&store, repo.path(), &options()).unwrap();
        let json = serde_json::to_value(snapshot).unwrap();
        let source = json["evidence"]["sources"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["kind"] == "facts")
            .unwrap();
        assert_eq!(source["status"], "loaded");
        assert!(source["id"].as_str().unwrap().starts_with("facts:sha256:"));
        let source_id = source["id"].clone();
        let file = json["evidence"]["files"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["path"] == "src/lib.rs")
            .unwrap();
        assert!(file["findings"].as_array().unwrap().iter().any(|finding| {
            finding["source_id"] == source_id && finding["rule_id"] == "architecture.review"
        }));
        assert_eq!(
            json["evidence"]["fact_relationships"]["items"][0]["fact_id"],
            "review.seed-caller"
        );
        assert_eq!(json["impact"]["impact"]["returned"], 1);
    }

    #[test]
    fn snapshot_exposes_junit_runtime_and_exact_project_knowledge() {
        let (repo, _index_dir, index_path) = fixture();
        fs::write(
            repo.path().join("junit.xml"),
            r#"<testsuite><testcase name="seed fails" file="src/lib.rs" time="0.01"><failure message="expected two"/></testcase></testsuite>"#,
        )
        .unwrap();
        fs::write(
            repo.path().join("traces.json"),
            serde_json::to_vec(&serde_json::json!({
                "resourceSpans": [{"scopeSpans": [{"spans": [{
                    "traceId": "trace-1",
                    "spanId": "span-1",
                    "name": "seed",
                    "attributes": [{
                        "key": "code.file.path",
                        "value": {"stringValue": "src/lib.rs"}
                    }]
                }]}]}]
            }))
            .unwrap(),
        )
        .unwrap();
        let mut writable = Store::open(&index_path).unwrap();
        writable
            .replace_project_history(&[crate::store::ProjectHistoryEntry {
                path: "docs/adr/001-seed.md".into(),
                kind: "architecture_decision".into(),
                title: "Keep seed deterministic".into(),
                body: "The contract is implemented in src/lib.rs.".into(),
            }])
            .unwrap();
        drop(writable);

        let store = Store::open_read_only(index_path).unwrap();
        let snapshot = build_snapshot_with_evidence_extensions(
            &store,
            repo.path(),
            &options(),
            &crate::evidence::EvidenceOptions::default(),
            &crate::evidence::EvidenceExtensionOptions {
                junit: vec![PathBuf::from("junit.xml")],
                otel: vec![PathBuf::from("traces.json")],
                project_knowledge: true,
            },
        )
        .unwrap();
        let json = serde_json::to_value(snapshot).unwrap();
        let file = json["evidence"]["files"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["path"] == "src/lib.rs")
            .unwrap();
        assert_eq!(file["test_results"]["failed"], 1);
        assert_eq!(file["runtime"]["spans"], 1);
        assert_eq!(
            file["knowledge"][0]["kind"], "architecture_decision",
            "{json}"
        );
        assert_eq!(json["evidence"]["sources"]["returned"], 3);
    }

    #[test]
    fn request_policy_accepts_only_same_origin_reads() {
        let authority = "127.0.0.1:43123";
        let same_origin = HttpRequest::parse(
            b"GET /api/lens HTTP/1.1\r\nHost: 127.0.0.1:43123\r\nOrigin: http://127.0.0.1:43123\r\nSec-Fetch-Site: same-origin\r\n\r\n",
        )
        .unwrap();
        assert!(same_origin.validate(authority).is_ok());

        let mutation =
            HttpRequest::parse(b"POST /api/lens HTTP/1.1\r\nHost: 127.0.0.1:43123\r\n\r\n")
                .unwrap();
        assert_eq!(mutation.validate(authority).unwrap_err().status, 405);

        let foreign_origin = HttpRequest::parse(
            b"GET /api/lens HTTP/1.1\r\nHost: 127.0.0.1:43123\r\nOrigin: https://attacker.example\r\n\r\n",
        )
        .unwrap();
        assert_eq!(foreign_origin.validate(authority).unwrap_err().status, 403);

        let rebound_host =
            HttpRequest::parse(b"GET /api/lens HTTP/1.1\r\nHost: attacker.example\r\n\r\n")
                .unwrap();
        assert_eq!(rebound_host.validate(authority).unwrap_err().status, 421);
    }

    #[test]
    fn embedded_assets_are_offline_and_csp_compatible() {
        assert!(INDEX_HTML.contains("src=\"app.js\""));
        assert!(INDEX_HTML.contains("href=\"styles.css\""));
        assert!(INDEX_HTML.contains("src=\"mastermind-mark.svg\""));
        assert!(INDEX_HTML.contains("style-src-attr 'unsafe-inline'"));
        assert!(!INDEX_HTML.contains("<script>"));
        assert!(!INDEX_HTML.contains("<style>"));
        for asset in [INDEX_HTML, STYLES_CSS] {
            assert!(!asset.contains("https://"));
            assert!(!asset.contains("http://"));
        }
        assert!(!APP_JS.contains("https://"));
        assert!(!APP_JS
            .replace("http://www.w3.org/2000/svg", "")
            .contains("http://"));
        assert!(MASTERMIND_MARK_SVG.contains("<svg"));
        assert!(!MASTERMIND_MARK_SVG.contains("https://"));
    }

    #[test]
    fn server_listener_is_loopback_only() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        assert!(listener.local_addr().unwrap().ip().is_loopback());
    }

    #[test]
    fn loopback_api_serves_shared_snapshot_without_touching_the_index() {
        let (repo, index_dir, index_path) = fixture();
        let writer = Store::open(&index_path).unwrap();
        writer
            .insert_symbol(
                "active_wal_evidence",
                "function",
                "src/lib.rs",
                3,
                3,
                None,
                None,
            )
            .unwrap();
        let index_before = directory_snapshot(index_dir.path());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let authority = format!("127.0.0.1:{}", address.port());
        let state = ServerState {
            root: repo.path().to_path_buf(),
            index_path,
            options: options(),
            evidence: crate::evidence::EvidenceOptions::default(),
            extensions: crate::evidence::EvidenceExtensionOptions::default(),
            authority: authority.clone(),
        };
        let server = std::thread::spawn(move || serve(listener, &state, Some(1)).unwrap());

        let mut client = TcpStream::connect(address).unwrap();
        client
            .write_all(
                format!(
                    "GET /api/lens HTTP/1.1\r\nHost: {authority}\r\nOrigin: http://{authority}\r\nSec-Fetch-Site: same-origin\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        let mut transcript = String::new();
        client.read_to_string(&mut transcript).unwrap();
        server.join().unwrap();

        assert!(transcript.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(transcript.contains("Content-Security-Policy: default-src 'none'"));
        assert!(transcript.contains("Cross-Origin-Resource-Policy: same-origin"));
        let (_, body) = transcript.split_once("\r\n\r\n").unwrap();
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["map"]["schema_version"], 1);
        assert_eq!(json["temporal"]["status"], "available");
        assert_eq!(json["impact"]["changes"]["files"]["returned"], 1);
        assert_eq!(directory_snapshot(index_dir.path()), index_before);
        drop(writer);
    }

    #[test]
    fn deletion_only_staleness_is_rejected_before_rendering_a_map() {
        let (repo, _index_dir, index_path) = fixture();
        fs::remove_file(repo.path().join("src/lib.rs")).unwrap();
        let state = ServerState {
            root: repo.path().to_path_buf(),
            index_path,
            options: options(),
            evidence: crate::evidence::EvidenceOptions::default(),
            extensions: crate::evidence::EvidenceExtensionOptions::default(),
            authority: "127.0.0.1:43123".into(),
        };

        let response = api_response(&state);
        let body = String::from_utf8(response.body).unwrap();
        assert_eq!(response.status, 422);
        assert!(body.contains("\"code\":\"index_stale\""), "{body}");
        assert!(!body.contains("\"map\""), "{body}");
    }

    #[test]
    fn clean_worktree_with_outdated_index_is_rejected() {
        let (repo, _index_dir, index_path) = fixture();
        let source_path = repo.path().join("src/lib.rs");
        fs::write(
            &source_path,
            "pub fn seed() -> i32 { 3 }\npub fn caller() -> i32 { seed() }\n",
        )
        .unwrap();
        let indexed_at = Store::open_read_only(&index_path)
            .unwrap()
            .file_mtime("src/lib.rs")
            .unwrap()
            .unwrap();
        let modified = SystemTime::UNIX_EPOCH
            + Duration::from_millis(u64::try_from(indexed_at + 10_000).unwrap());
        fs::OpenOptions::new()
            .write(true)
            .open(&source_path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        git(repo.path(), &["add", "src/lib.rs"]);
        git(
            repo.path(),
            &["commit", "-qm", "advance head without reindex"],
        );
        let state = ServerState {
            root: repo.path().to_path_buf(),
            index_path,
            options: options(),
            evidence: crate::evidence::EvidenceOptions::default(),
            extensions: crate::evidence::EvidenceExtensionOptions::default(),
            authority: "127.0.0.1:43123".into(),
        };

        let response = api_response(&state);
        let body = String::from_utf8(response.body).unwrap();
        assert_eq!(response.status, 422);
        assert!(body.contains("\"code\":\"index_stale\""), "{body}");
    }

    #[test]
    fn clean_worktree_with_unindexed_committed_source_is_rejected() {
        let (repo, _index_dir, index_path) = fixture();
        fs::write(
            repo.path().join("src/new.rs"),
            "pub fn newly_committed() -> i32 { 7 }\n",
        )
        .unwrap();
        git(repo.path(), &["add", "src/new.rs"]);
        git(
            repo.path(),
            &["commit", "-qm", "add source without reindex"],
        );
        let state = ServerState {
            root: repo.path().to_path_buf(),
            index_path,
            options: options(),
            evidence: crate::evidence::EvidenceOptions::default(),
            extensions: crate::evidence::EvidenceExtensionOptions::default(),
            authority: "127.0.0.1:43123".into(),
        };

        let response = api_response(&state);
        let body = String::from_utf8(response.body).unwrap();
        assert_eq!(response.status, 422);
        assert!(body.contains("\"code\":\"index_stale\""), "{body}");
    }

    #[test]
    fn incompatible_schema_returns_a_sanitized_actionable_error() {
        let repo = tempfile::tempdir().unwrap();
        let index_dir = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q"]);
        let index_path = index_dir.path().join("old.db");
        let connection = rusqlite::Connection::open(&index_path).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);\
                 INSERT INTO meta VALUES ('schema_version', '6');\
                 INSERT INTO meta VALUES ('index_root', '{}');\
                 CREATE TABLE files(path TEXT PRIMARY KEY, indexed_at INTEGER, symbol_count INTEGER);",
                repo.path().display()
            ))
            .unwrap();
        drop(connection);
        let state = ServerState {
            root: repo.path().to_path_buf(),
            index_path,
            options: options(),
            evidence: crate::evidence::EvidenceOptions::default(),
            extensions: crate::evidence::EvidenceExtensionOptions::default(),
            authority: "127.0.0.1:43123".into(),
        };

        let response = api_response(&state);
        let body = String::from_utf8(response.body).unwrap();
        assert_eq!(response.status, 422);
        assert!(body.contains("\"code\":\"index_stale\""), "{body}");
        assert!(body.contains("mastermind index ."), "{body}");
        for leaked in ["SELECT", "no such", "column", "SQL"] {
            assert!(!body.contains(leaked), "leaked {leaked}: {body}");
        }
    }

    #[test]
    fn map_failures_do_not_expose_sql_details() {
        let response = error_response(&LensError::MapUnavailable(
            "SELECT secret FROM internal; no such column: hidden".into(),
        ));
        let body = String::from_utf8(response.body).unwrap();
        assert_eq!(response.status, 422);
        assert!(body.contains("\"code\":\"map_unavailable\""), "{body}");
        assert!(body.contains("refresh the index"), "{body}");
        for leaked in ["SELECT", "secret", "no such", "hidden"] {
            assert!(!body.contains(leaked), "leaked {leaked}: {body}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn index_state_follows_a_symlink_to_the_target_wal() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("real.db");
        let alias = directory.path().join("alias.db");
        let writer = Store::open(&path).unwrap();
        writer
            .insert_symbol("wal_only", "function", "src/lib.rs", 1, 2, None, None)
            .unwrap();
        symlink(&path, &alias).unwrap();

        let state = index_source_state(&alias).unwrap();
        assert!(state.wal.as_ref().is_some_and(|wal| wal.len > 0));
        assert_eq!(state, index_source_state(&path).unwrap());
        drop(writer);
    }
}
