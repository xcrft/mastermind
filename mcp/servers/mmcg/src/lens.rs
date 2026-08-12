//! Local, read-only change-review UI backed by the shared map and impact engines.
//!
//! Lens deliberately has no HTTP framework or frontend build step. It serves a
//! small, embedded application on an ephemeral loopback port, accepts only
//! same-origin `GET`/`HEAD` requests, and opens the existing SQLite index in
//! query-only mode for every refresh.

use crate::queries::{self, ChangeImpactError, ChangeImpactResponse, ProjectMapResponse};
use crate::store::{query_budget_ms_from_env, Store, WorkBudget, DEFAULT_CLI_BUDGET_MS};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
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

const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

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

#[derive(Debug, Serialize)]
pub struct LensSnapshot {
    pub schema_version: u32,
    pub repository: LensRepository,
    pub options: LensOptions,
    pub map: ProjectMapResponse,
    pub impact: ChangeImpactResponse,
    pub temporal: LensTemporalSnapshot,
    pub semantic: crate::scip_overlay::SemanticOverlaySnapshot,
    pub evidence: crate::evidence::EvidenceSnapshot,
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
    Ok(())
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
    validate_index_snapshot(store, &root, deadline)?;

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
    let evidence = crate::evidence::collect_with_store(
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

    Ok(LensSnapshot {
        schema_version: 1,
        repository: LensRepository { name, root_label },
        options: options.clone(),
        map,
        impact,
        temporal,
        semantic,
        evidence,
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
        "/api/lens" => api_response(state),
        "/favicon.ico" => HttpResponse::empty(204, "No Content"),
        _ => HttpResponse::text(404, "Not Found", "not found"),
    }
}

fn api_response(state: &ServerState) -> HttpResponse {
    let deadline = request_deadline();
    let result = index_source_state(&state.index_path).and_then(|before| {
        let snapshot = Store::open_read_only_with_deadline(&state.index_path, deadline)
            .map_err(read_only_open_error)
            .and_then(|store| {
                build_snapshot_until(
                    &store,
                    &state.root,
                    &state.options,
                    &state.evidence,
                    &state.extensions,
                    deadline,
                )
            })?;
        if index_source_state(&state.index_path)? != before {
            return Err(LensError::ImpactUnavailable(
                ChangeImpactError::SnapshotChanged,
            ));
        }
        Ok(snapshot)
    });
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
        let json = serde_json::to_value(snapshot).unwrap();

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
