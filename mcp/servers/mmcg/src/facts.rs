//! Declarative, revision-bound fact ingestion for community extensions.
//!
//! Producers submit a strict JSON manifest. Mastermind validates the complete
//! document, repository/revision binding, source files, and provenance
//! artifacts before atomically replacing one producer dataset. Imported data
//! never selects SQL, registers MCP handlers, or changes the Tree-sitter graph.

use crate::store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs::File;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const API_VERSION: &str = "mastermind-facts/v1";
pub const SUPPORTED_CAPABILITIES: [&str; 2] = ["annotations", "relationships"];

const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILES: usize = 10_000;
const MAX_FACTS: usize = 100_000;
const MAX_ARTIFACTS: usize = 64;
const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ARTIFACT_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_LENS_SOURCES: usize = 64;
const MAX_SOURCES_RETURNED: usize = MAX_LENS_SOURCES;
pub const MAX_LENS_ARTIFACTS: usize = 64;
const MAX_ARTIFACTS_RETURNED: usize = MAX_LENS_ARTIFACTS;
// Even when every path/message byte needs JSON escaping, a standalone MCP
// response remains below the server's 8 MiB serialized-result limit.
const MAX_FACTS_RETURNED: usize = 400;
pub const MAX_LENS_FACTS: usize = 200;
const MAX_DIAGNOSTICS: usize = 100;
const MAX_ID_BYTES: usize = 256;
const MAX_NAME_BYTES: usize = 128;
const MAX_VERSION_BYTES: usize = 128;
const MAX_PATH_BYTES: usize = 4 * 1024;
const MAX_TITLE_BYTES: usize = 256;
const MAX_MESSAGE_BYTES: usize = 4 * 1024;
const MAX_LABEL_BYTES: usize = 512;

#[derive(Debug)]
pub enum FactError {
    InvalidManifest(String),
    InvalidQuery(String),
    Io(String),
    Store(String),
    Git(String),
}

impl fmt::Display for FactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidManifest(message) => write!(formatter, "invalid fact manifest: {message}"),
            Self::InvalidQuery(message) => write!(formatter, "invalid fact query: {message}"),
            Self::Io(message) => write!(formatter, "fact ingestion I/O error: {message}"),
            Self::Store(message) => write!(formatter, "fact store error: {message}"),
            Self::Git(message) => write!(formatter, "fact repository error: {message}"),
        }
    }
}

impl std::error::Error for FactError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FactManifest {
    api_version: String,
    capabilities: Vec<String>,
    repository: ManifestRepository,
    producer: ManifestProducer,
    dataset: String,
    provenance: ManifestProvenance,
    files: Vec<ManifestFile>,
    artifacts: Vec<ManifestArtifact>,
    facts: Vec<ManifestFact>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestRepository {
    identity: String,
    revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestProducer {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestProvenance {
    kind: String,
    artifacts: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestArtifact {
    id: String,
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestLocation {
    path: String,
    line: u32,
    #[serde(default)]
    column: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ManifestFact {
    Annotation {
        id: String,
        path: String,
        line: u32,
        #[serde(default)]
        column: Option<u32>,
        #[serde(default)]
        end_line: Option<u32>,
        #[serde(default)]
        end_column: Option<u32>,
        severity: String,
        category: String,
        title: String,
        message: String,
    },
    Relationship {
        id: String,
        relation: String,
        from: ManifestLocation,
        to: ManifestLocation,
        confidence: String,
        label: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct FactSourceRecord {
    pub id: i64,
    pub api_version: String,
    pub producer_name: String,
    pub producer_version: String,
    pub dataset: String,
    pub provenance_kind: String,
    pub capabilities: String,
    pub repository_identity: String,
    pub revision: String,
    pub manifest_sha256: String,
    pub manifest_bytes: u64,
    pub imported_at: i64,
    pub file_count: u32,
    pub annotation_count: u32,
    pub relationship_count: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct FactFileRecord {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct FactArtifactRecord {
    pub id: String,
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FactAnnotation {
    pub source_id: String,
    pub fact_id: String,
    pub path: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
    pub severity: String,
    pub category: String,
    pub title: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FactRelationship {
    pub source_id: String,
    pub fact_id: String,
    pub relation: String,
    pub from_path: String,
    pub from_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_column: Option<u32>,
    pub to_path: String,
    pub to_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_column: Option<u32>,
    pub confidence: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FactArtifact {
    pub source_id: String,
    pub id: String,
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct FactImportBatch {
    pub source: FactSourceRecord,
    pub files: Vec<FactFileRecord>,
    pub artifacts: Vec<FactArtifactRecord>,
    pub annotations: Vec<FactAnnotation>,
    pub relationships: Vec<FactRelationship>,
}

#[derive(Debug, Clone)]
pub(crate) enum FactQueryFilter {
    Scope(String),
    Paths(Vec<String>),
}

#[derive(Debug, Serialize)]
pub struct FactContract {
    pub api_version: &'static str,
    pub supported_capabilities: [&'static str; 2],
    pub repository: FactRepository,
}

#[derive(Debug, Clone, Serialize)]
pub struct FactRepository {
    pub identity: String,
    pub revision: String,
}

#[derive(Debug, Serialize)]
pub struct FactImportSummary {
    pub schema_version: u32,
    pub replaced_previous_dataset: bool,
    pub contract: FactContract,
    pub source: FactSourceView,
}

#[derive(Debug, Clone, Serialize)]
pub struct FactSourceView {
    pub id: String,
    pub producer: String,
    pub producer_version: String,
    pub dataset: String,
    pub provenance: String,
    pub capabilities: Vec<String>,
    pub repository_identity: String,
    pub revision: String,
    pub manifest_sha256: String,
    pub manifest_bytes: u64,
    pub imported_at: i64,
    pub status: &'static str,
    pub facts_total: u32,
    pub facts_returned: u32,
    pub files_matched: u32,
}

#[derive(Debug, Serialize)]
pub struct FactCollection<T> {
    pub total: Option<u32>,
    pub returned: u32,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<&'static str>,
    pub items: Vec<T>,
}

#[derive(Debug, Serialize)]
pub struct FactDiagnostic {
    pub source_id: String,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct FactLimits {
    pub sources: u32,
    pub facts: u32,
    pub diagnostics: u32,
    pub provenance_artifacts: u32,
    pub manifest_bytes: u64,
    pub files_per_manifest: u32,
    pub source_bytes_per_manifest: u64,
    pub artifacts_per_manifest: u32,
    pub artifact_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct FactSnapshot {
    pub schema_version: u32,
    pub available: bool,
    pub partial: bool,
    pub contract: FactContract,
    pub sources: FactCollection<FactSourceView>,
    pub artifacts: FactCollection<FactArtifact>,
    pub annotations: FactCollection<FactAnnotation>,
    pub relationships: FactCollection<FactRelationship>,
    pub diagnostics: FactCollection<FactDiagnostic>,
    pub limits: FactLimits,
}

pub(crate) fn source_public_id(producer: &str, dataset: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(producer.as_bytes());
    hasher.update([0]);
    hasher.update(dataset.as_bytes());
    format!("facts:sha256:{}", crate::hex::encode(&hasher.finalize()))
}

fn invalid(message: impl Into<String>) -> FactError {
    FactError::InvalidManifest(message.into())
}

fn validate_text(value: &str, maximum: usize, label: &str) -> Result<(), FactError> {
    if value.is_empty() || value.len() > maximum {
        return Err(invalid(format!(
            "{label} must contain 1..={maximum} UTF-8 bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid(format!("{label} contains control characters")));
    }
    Ok(())
}

fn validate_token(value: &str, maximum: usize, label: &str) -> Result<(), FactError> {
    validate_text(value, maximum, label)?;
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
    }) {
        return Err(invalid(format!("{label} contains unsupported characters")));
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<(), FactError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(format!(
            "{label} must be 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

pub(crate) fn normalize_fact_path(value: &str) -> Result<String, FactError> {
    if value.is_empty()
        || value == "."
        || value.len() > MAX_PATH_BYTES
        || value.starts_with("./")
        || value.ends_with('/')
        || value.contains('\\')
        || value.contains(':')
        || value.contains("//")
        || value
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
        || value.chars().any(char::is_control)
    {
        return Err(invalid(
            "fact paths must be canonical repository-relative paths",
        ));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(invalid(
            "fact paths must not contain traversal or absolute roots",
        ));
    }
    Ok(value.to_string())
}

pub fn normalize_query_path(value: &str) -> Result<String, FactError> {
    if value == "." {
        return Ok(".".into());
    }
    normalize_fact_path(value).map_err(|_| {
        FactError::InvalidQuery("path must be `.` or a canonical repository-relative path".into())
    })
}

fn read_regular(path: &Path, maximum: u64, label: &str) -> Result<Vec<u8>, FactError> {
    read_regular_until(path, maximum, label, None)
}

fn read_regular_until(
    path: &Path,
    maximum: u64,
    label: &str,
    deadline: Option<Instant>,
) -> Result<Vec<u8>, FactError> {
    let initial = std::fs::symlink_metadata(path)
        .map_err(|error| FactError::Io(format!("read {label} metadata: {error}")))?;
    if !initial.file_type().is_file() {
        return Err(invalid(format!(
            "{label} must be a regular non-symlink file"
        )));
    }
    if initial.len() > maximum {
        return Err(invalid(format!("{label} exceeds the {maximum}-byte limit")));
    }
    let file = File::open(path).map_err(|error| FactError::Io(format!("open {label}: {error}")))?;
    let before = file
        .metadata()
        .map_err(|error| FactError::Io(format!("read {label} metadata: {error}")))?;
    if !before.is_file() || before.len() != initial.len() || modified(&before) != modified(&initial)
    {
        return Err(invalid(format!("{label} changed before it could be read")));
    }
    read_opened_regular_until(file, before, maximum, label, deadline)
}

fn read_opened_regular_until(
    mut file: File,
    before: std::fs::Metadata,
    maximum: u64,
    label: &str,
    deadline: Option<Instant>,
) -> Result<Vec<u8>, FactError> {
    if !before.is_file() {
        return Err(invalid(format!(
            "{label} must be a regular non-symlink file"
        )));
    }
    if before.len() > maximum {
        return Err(invalid(format!("{label} exceeds the {maximum}-byte limit")));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
    let mut remaining = maximum.saturating_add(1);
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(FactError::Io(format!(
                "verification deadline exceeded while reading {label}"
            )));
        }
        let chunk = buffer.len().min(remaining as usize);
        let count = file
            .read(&mut buffer[..chunk])
            .map_err(|error| FactError::Io(format!("read {label}: {error}")))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        remaining = remaining.saturating_sub(count as u64);
    }
    if bytes.len() as u64 > maximum {
        return Err(invalid(format!("{label} exceeds the {maximum}-byte limit")));
    }
    let after = file
        .metadata()
        .map_err(|error| FactError::Io(format!("re-read {label} metadata: {error}")))?;
    let identity_changed = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            before.dev() != after.dev() || before.ino() != after.ino()
        }
        #[cfg(not(unix))]
        {
            false
        }
    };
    if identity_changed || before.len() != after.len() || modified(&before) != modified(&after) {
        return Err(invalid(format!("{label} changed while it was being read")));
    }
    Ok(bytes)
}

fn modified(metadata: &std::fs::Metadata) -> Option<SystemTime> {
    metadata.modified().ok()
}

#[cfg(not(unix))]
fn resolve_contained_regular(
    root: &Path,
    relative: &str,
    label: &str,
) -> Result<PathBuf, FactError> {
    let mut candidate = root.to_path_buf();
    let components = Path::new(relative).components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(part) = component else {
            return Err(invalid(format!("{label} has an unsafe path component")));
        };
        candidate.push(part);
        let metadata = std::fs::symlink_metadata(&candidate)
            .map_err(|error| FactError::Io(format!("read {label} path metadata: {error}")))?;
        if metadata.file_type().is_symlink() {
            return Err(invalid(format!("{label} must not traverse a symlink")));
        }
        if index + 1 < components.len() && !metadata.is_dir() {
            return Err(invalid(format!("{label} has a non-directory parent")));
        }
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|error| FactError::Io(format!("resolve {label}: {error}")))?;
    if !canonical.starts_with(root) {
        return Err(invalid(format!("{label} escapes the indexed repository")));
    }
    Ok(canonical)
}

#[cfg(unix)]
fn path_component_cstring(component: &std::ffi::OsStr) -> Result<std::ffi::CString, FactError> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(component.as_bytes())
        .map_err(|_| invalid("fact path contains an unsafe component"))
}

#[cfg(unix)]
fn open_contained_at(
    parent: &File,
    component: &std::ffi::OsStr,
    directory: bool,
) -> Result<File, FactError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let component = path_component_cstring(component)?;
    let flags = libc::O_RDONLY
        | libc::O_NOFOLLOW
        | libc::O_CLOEXEC
        | if directory { libc::O_DIRECTORY } else { 0 };
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), component.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(invalid(
            "fact inputs must use regular non-symlink repository paths",
        ));
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn read_contained_regular(
    root: &Path,
    relative: &str,
    maximum: u64,
    label: &str,
    deadline: Option<Instant>,
) -> Result<Vec<u8>, FactError> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut directory = options
        .open(root)
        .map_err(|error| FactError::Io(format!("open indexed repository: {error}")))?;
    let parts = Path::new(relative)
        .components()
        .map(|component| match component {
            Component::Normal(part) => Ok(part.to_os_string()),
            _ => Err(invalid("fact path contains an unsafe component")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (leaf, parents) = parts
        .split_last()
        .ok_or_else(|| invalid("fact path is empty"))?;
    for parent in parents {
        directory = open_contained_at(&directory, parent, true)?;
    }
    let file = open_contained_at(&directory, leaf, false)?;
    let metadata = file
        .metadata()
        .map_err(|error| FactError::Io(format!("read {label} metadata: {error}")))?;
    read_opened_regular_until(file, metadata, maximum, label, deadline)
}

#[cfg(not(unix))]
fn read_contained_regular(
    root: &Path,
    relative: &str,
    maximum: u64,
    label: &str,
    deadline: Option<Instant>,
) -> Result<Vec<u8>, FactError> {
    let resolved = resolve_contained_regular(root, relative, label)?;
    read_regular_until(&resolved, maximum, label, deadline)
}

fn canonical_remote(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 4 * 1024 || value.chars().any(char::is_control) {
        return None;
    }
    if let Some((scheme, rest)) = value.split_once("://") {
        if !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "ssh" | "git"
        ) {
            return None;
        }
        let rest = rest.split(['?', '#']).next()?;
        let (authority, raw_path) = rest.split_once('/')?;
        let authority = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        if authority.is_empty() {
            return None;
        }
        return canonical_remote_host_path(authority, raw_path);
    }
    let scp = value.rsplit_once('@').map_or(value, |(_, rest)| rest);
    let (host, path) = scp.split_once(':')?;
    if host.contains('/') || host.is_empty() {
        return None;
    }
    canonical_remote_host_path(host, path)
}

fn canonical_remote_host_path(host: &str, path: &str) -> Option<String> {
    let path = path
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_matches('/'));
    if path.is_empty()
        || path
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return None;
    }
    Some(format!("{}/{}", host.to_ascii_lowercase(), path))
}

fn repository_identity(root: &Path) -> Result<String, FactError> {
    let output = crate::diff::run_bounded_git_with_limit(
        root,
        &["config", "--get", "remote.origin.url"],
        None,
        4 * 1024,
    )
    .map_err(|error| FactError::Git(format!("read bounded origin identity: {error}")))?;
    let canonical = if output.success {
        std::str::from_utf8(&output.stdout)
            .ok()
            .and_then(canonical_remote)
            .map(|value| ("git-remote", value))
    } else {
        None
    };
    let (kind, identity) =
        canonical.unwrap_or_else(|| ("git-worktree", root.to_string_lossy().replace('\\', "/")));
    Ok(format!(
        "{kind}:sha256:{}",
        crate::hex::encode(&Sha256::digest(identity.as_bytes()))
    ))
}

fn repository_contract(store: &Store) -> Result<FactRepository, FactError> {
    if !store
        .schema_current()
        .map_err(|error| FactError::Store(error.to_string()))?
    {
        return Err(FactError::Store(
            "the codegraph schema is stale; run `mastermind index .` first".into(),
        ));
    }
    let root = store
        .meta_value("index_root")
        .map_err(|error| FactError::Store(error.to_string()))?
        .ok_or_else(|| {
            FactError::Store(
                "the index has no repository identity; run `mastermind index .` first".into(),
            )
        })?;
    let root = PathBuf::from(root)
        .canonicalize()
        .map_err(|error| FactError::Store(format!("resolve index root: {error}")))?;
    crate::indexer::validate_index_root(store, &root).map_err(FactError::Store)?;
    let revision =
        crate::diff::current_head_oid(&root).map_err(|error| FactError::Git(error.to_string()))?;
    Ok(FactRepository {
        identity: repository_identity(&root)?,
        revision,
    })
}

pub fn contract(store: &Store) -> Result<FactContract, FactError> {
    Ok(FactContract {
        api_version: API_VERSION,
        supported_capabilities: SUPPORTED_CAPABILITIES,
        repository: repository_contract(store)?,
    })
}

fn indexed_root(store: &Store) -> Result<PathBuf, FactError> {
    store
        .meta_value("index_root")
        .map_err(|error| FactError::Store(error.to_string()))?
        .ok_or_else(|| FactError::Store("the index has no repository identity".into()))
        .and_then(|value| {
            PathBuf::from(value)
                .canonicalize()
                .map_err(|error| FactError::Store(format!("resolve index root: {error}")))
        })
}

fn validate_capabilities(values: &[String]) -> Result<Vec<String>, FactError> {
    if values.is_empty() || values.len() > SUPPORTED_CAPABILITIES.len() {
        return Err(invalid(
            "capabilities must declare at least one supported capability",
        ));
    }
    let supported = SUPPORTED_CAPABILITIES.into_iter().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for capability in values {
        if !supported.contains(capability.as_str()) {
            return Err(invalid(format!(
                "unsupported capability `{capability}`; supported capabilities: {}",
                SUPPORTED_CAPABILITIES.join(", ")
            )));
        }
        if !seen.insert(capability.as_str()) {
            return Err(invalid(format!("duplicate capability `{capability}`")));
        }
        normalized.push(capability.clone());
    }
    normalized.sort();
    Ok(normalized)
}

fn validate_location(
    location: &ManifestLocation,
    files: &HashMap<String, FactFileRecord>,
    label: &str,
) -> Result<(), FactError> {
    let path = normalize_fact_path(&location.path)?;
    if !files.contains_key(&path) {
        return Err(invalid(format!(
            "{label} path `{path}` is absent from files"
        )));
    }
    if location.line == 0 || location.column == Some(0) {
        return Err(invalid(format!("{label} positions are one-based")));
    }
    Ok(())
}

fn verified_file(
    root: &Path,
    store: &Store,
    file: &ManifestFile,
) -> Result<FactFileRecord, FactError> {
    let path = normalize_fact_path(&file.path)?;
    validate_sha256(&file.sha256, "file sha256")?;
    if file.bytes > crate::indexer::MAX_INDEXABLE_FILE_SIZE {
        return Err(invalid(format!(
            "file `{path}` exceeds the indexable source limit"
        )));
    }
    let bytes = read_contained_regular(
        root,
        &path,
        crate::indexer::MAX_INDEXABLE_FILE_SIZE,
        &format!("source file `{path}`"),
        None,
    )?;
    if bytes.len() as u64 != file.bytes {
        return Err(invalid(format!(
            "file `{path}` size does not match manifest"
        )));
    }
    let digest = crate::hex::encode(&Sha256::digest(&bytes));
    if digest != file.sha256 {
        return Err(invalid(format!(
            "file `{path}` digest does not match manifest"
        )));
    }
    let indexed = store
        .file_content_sha256(&path)
        .map_err(|error| FactError::Store(error.to_string()))?;
    if indexed.as_deref().filter(|value| !value.is_empty()) != Some(digest.as_str()) {
        return Err(invalid(format!(
            "file `{path}` does not match the current codegraph index"
        )));
    }
    Ok(FactFileRecord {
        path,
        sha256: digest,
        bytes: file.bytes,
    })
}

fn verified_artifact(
    root: &Path,
    artifact: &ManifestArtifact,
) -> Result<FactArtifactRecord, FactError> {
    validate_token(&artifact.id, MAX_ID_BYTES, "artifact id")?;
    let path = normalize_fact_path(&artifact.path)?;
    validate_sha256(&artifact.sha256, "artifact sha256")?;
    if artifact.bytes > MAX_ARTIFACT_BYTES {
        return Err(invalid(format!(
            "artifact `{}` exceeds the size limit",
            artifact.id
        )));
    }
    let label = format!("provenance artifact `{}`", artifact.id);
    let bytes = read_contained_regular(root, &path, MAX_ARTIFACT_BYTES, &label, None)?;
    if bytes.len() as u64 != artifact.bytes {
        return Err(invalid(format!(
            "artifact `{}` size does not match manifest",
            artifact.id
        )));
    }
    let digest = crate::hex::encode(&Sha256::digest(&bytes));
    if digest != artifact.sha256 {
        return Err(invalid(format!(
            "artifact `{}` digest does not match manifest",
            artifact.id
        )));
    }
    Ok(FactArtifactRecord {
        id: artifact.id.clone(),
        path,
        sha256: digest,
        bytes: artifact.bytes,
    })
}

fn build_batch(
    store: &Store,
    manifest_path: &Path,
) -> Result<(FactImportBatch, FactContract), FactError> {
    let bytes = read_regular(manifest_path, MAX_MANIFEST_BYTES, "fact manifest")?;
    let manifest: FactManifest = crate::audit_bundle::from_json_strict(&bytes)
        .map_err(|error| invalid(error.to_string()))?;
    if manifest.api_version != API_VERSION {
        return Err(invalid(format!(
            "unsupported api_version `{}`; expected `{API_VERSION}`",
            manifest.api_version
        )));
    }
    let capabilities = validate_capabilities(&manifest.capabilities)?;
    validate_token(&manifest.producer.name, MAX_NAME_BYTES, "producer name")?;
    validate_text(
        &manifest.producer.version,
        MAX_VERSION_BYTES,
        "producer version",
    )?;
    validate_token(&manifest.dataset, MAX_NAME_BYTES, "dataset")?;
    validate_token(&manifest.provenance.kind, MAX_NAME_BYTES, "provenance kind")?;
    if manifest.files.len() > MAX_FILES {
        return Err(invalid(format!("files exceed the {MAX_FILES}-entry limit")));
    }
    if manifest.artifacts.len() > MAX_ARTIFACTS {
        return Err(invalid(format!(
            "artifacts exceed the {MAX_ARTIFACTS}-entry limit"
        )));
    }
    if manifest.facts.len() > MAX_FACTS {
        return Err(invalid(format!("facts exceed the {MAX_FACTS}-entry limit")));
    }

    let contract = contract(store)?;
    if manifest.repository.identity != contract.repository.identity {
        return Err(invalid(
            "repository identity does not match the indexed repository",
        ));
    }
    if manifest.repository.revision != contract.repository.revision {
        return Err(invalid(
            "repository revision does not match the current Git HEAD",
        ));
    }
    let root = indexed_root(store)?;

    let mut files = HashMap::new();
    let mut source_bytes = 0_u64;
    for file in &manifest.files {
        let verified = verified_file(&root, store, file)?;
        source_bytes = source_bytes
            .checked_add(verified.bytes)
            .ok_or_else(|| invalid("source byte total overflow"))?;
        if source_bytes > MAX_SOURCE_BYTES {
            return Err(invalid(format!(
                "referenced sources exceed the {MAX_SOURCE_BYTES}-byte total limit"
            )));
        }
        let path = verified.path.clone();
        if files.insert(path.clone(), verified).is_some() {
            return Err(invalid(format!("duplicate file path `{path}`")));
        }
    }

    let mut artifacts = Vec::new();
    let mut artifact_ids = HashSet::new();
    let mut artifact_total = 0_u64;
    for artifact in &manifest.artifacts {
        let verified = verified_artifact(&root, artifact)?;
        if !artifact_ids.insert(verified.id.clone()) {
            return Err(invalid(format!("duplicate artifact id `{}`", verified.id)));
        }
        artifact_total = artifact_total
            .checked_add(verified.bytes)
            .ok_or_else(|| invalid("artifact byte total overflow"))?;
        if artifact_total > MAX_ARTIFACT_TOTAL_BYTES {
            return Err(invalid(format!(
                "provenance artifacts exceed the {MAX_ARTIFACT_TOTAL_BYTES}-byte total limit"
            )));
        }
        artifacts.push(verified);
    }
    let mut referenced_artifacts = HashSet::new();
    for id in &manifest.provenance.artifacts {
        validate_token(id, MAX_ID_BYTES, "provenance artifact id")?;
        if !artifact_ids.contains(id) {
            return Err(invalid(format!(
                "provenance references unknown artifact `{id}`"
            )));
        }
        if !referenced_artifacts.insert(id) {
            return Err(invalid(format!("duplicate provenance artifact `{id}`")));
        }
    }
    if artifact_ids.len() != referenced_artifacts.len() {
        return Err(invalid(
            "every declared artifact must be referenced by provenance",
        ));
    }

    let mut fact_ids = HashSet::new();
    let mut annotations = Vec::new();
    let mut relationships = Vec::new();
    let source_id = source_public_id(&manifest.producer.name, &manifest.dataset);
    for fact in manifest.facts {
        match fact {
            ManifestFact::Annotation {
                id,
                path,
                line,
                column,
                end_line,
                end_column,
                severity,
                category,
                title,
                message,
            } => {
                if !capabilities.iter().any(|value| value == "annotations") {
                    return Err(invalid(
                        "annotation fact requires the `annotations` capability",
                    ));
                }
                validate_token(&id, MAX_ID_BYTES, "fact id")?;
                if !fact_ids.insert(id.clone()) {
                    return Err(invalid(format!("duplicate fact id `{id}`")));
                }
                let path = normalize_fact_path(&path)?;
                if !files.contains_key(&path) {
                    return Err(invalid(format!(
                        "annotation path `{path}` is absent from files"
                    )));
                }
                if line == 0 || column == Some(0) || end_line == Some(0) || end_column == Some(0) {
                    return Err(invalid("annotation positions are one-based"));
                }
                if let Some(end) = end_line {
                    if end < line {
                        return Err(invalid("annotation end_line precedes line"));
                    }
                    if end == line
                        && end_column
                            .zip(column)
                            .is_some_and(|(end, start)| end < start)
                    {
                        return Err(invalid("annotation end_column precedes column"));
                    }
                }
                if !matches!(severity.as_str(), "info" | "warning" | "error") {
                    return Err(invalid(
                        "annotation severity must be info, warning, or error",
                    ));
                }
                validate_token(&category, MAX_NAME_BYTES, "annotation category")?;
                validate_text(&title, MAX_TITLE_BYTES, "annotation title")?;
                validate_text(&message, MAX_MESSAGE_BYTES, "annotation message")?;
                annotations.push(FactAnnotation {
                    source_id: source_id.clone(),
                    fact_id: id,
                    path,
                    line,
                    column,
                    end_line,
                    end_column,
                    severity,
                    category,
                    title,
                    message,
                });
            }
            ManifestFact::Relationship {
                id,
                relation,
                from,
                to,
                confidence,
                label,
            } => {
                if !capabilities.iter().any(|value| value == "relationships") {
                    return Err(invalid(
                        "relationship fact requires the `relationships` capability",
                    ));
                }
                validate_token(&id, MAX_ID_BYTES, "fact id")?;
                if !fact_ids.insert(id.clone()) {
                    return Err(invalid(format!("duplicate fact id `{id}`")));
                }
                validate_token(&relation, MAX_NAME_BYTES, "relationship relation")?;
                validate_location(&from, &files, "relationship from")?;
                validate_location(&to, &files, "relationship to")?;
                if !matches!(confidence.as_str(), "low" | "medium" | "high" | "observed") {
                    return Err(invalid(
                        "relationship confidence must be low, medium, high, or observed",
                    ));
                }
                validate_text(&label, MAX_LABEL_BYTES, "relationship label")?;
                relationships.push(FactRelationship {
                    source_id: source_id.clone(),
                    fact_id: id,
                    relation,
                    from_path: from.path,
                    from_line: from.line,
                    from_column: from.column,
                    to_path: to.path,
                    to_line: to.line,
                    to_column: to.column,
                    confidence,
                    label,
                });
            }
        }
    }

    let file_count = u32::try_from(files.len()).map_err(|_| invalid("file count overflow"))?;
    let annotation_count =
        u32::try_from(annotations.len()).map_err(|_| invalid("annotation count overflow"))?;
    let relationship_count =
        u32::try_from(relationships.len()).map_err(|_| invalid("relationship count overflow"))?;
    let imported_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let source = FactSourceRecord {
        id: 0,
        api_version: API_VERSION.into(),
        producer_name: manifest.producer.name,
        producer_version: manifest.producer.version,
        dataset: manifest.dataset,
        provenance_kind: manifest.provenance.kind,
        capabilities: capabilities.join(","),
        repository_identity: contract.repository.identity.clone(),
        revision: contract.repository.revision.clone(),
        manifest_sha256: crate::hex::encode(&Sha256::digest(&bytes)),
        manifest_bytes: bytes.len() as u64,
        imported_at,
        file_count,
        annotation_count,
        relationship_count,
    };
    let mut files = files.into_values().collect::<Vec<_>>();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok((
        FactImportBatch {
            source,
            files,
            artifacts,
            annotations,
            relationships,
        },
        contract,
    ))
}

fn revalidate_batch_snapshot(
    store: &Store,
    batch: &FactImportBatch,
    expected: &FactContract,
) -> Result<(), FactError> {
    let current = contract(store)?;
    if current.repository.identity != expected.repository.identity
        || current.repository.revision != expected.repository.revision
    {
        return Err(invalid(
            "repository identity or revision changed while the manifest was being validated",
        ));
    }
    let root = indexed_root(store)?;
    for file in &batch.files {
        let binding = ManifestFile {
            path: file.path.clone(),
            sha256: file.sha256.clone(),
            bytes: file.bytes,
        };
        verified_file(&root, store, &binding)?;
    }
    for artifact in &batch.artifacts {
        verify_artifact_record(&root, artifact, None)?;
    }
    Ok(())
}

pub fn import(store: &Store, manifest_path: &Path) -> Result<FactImportSummary, FactError> {
    let (batch, contract) = build_batch(store, manifest_path)?;
    let replaced_previous_dataset = store
        .fact_source_exists(&batch.source.producer_name, &batch.source.dataset)
        .map_err(|error| FactError::Store(error.to_string()))?;
    revalidate_batch_snapshot(store, &batch, &contract)?;
    store
        .replace_fact_dataset(&batch)
        .map_err(|error| FactError::Store(error.to_string()))?;
    let source = source_view(
        &batch.source,
        "loaded",
        batch
            .source
            .annotation_count
            .saturating_add(batch.source.relationship_count),
        batch.source.file_count,
    );
    Ok(FactImportSummary {
        schema_version: 1,
        replaced_previous_dataset,
        contract,
        source,
    })
}

fn source_view(
    source: &FactSourceRecord,
    status: &'static str,
    facts_returned: u32,
    files_matched: u32,
) -> FactSourceView {
    FactSourceView {
        id: source_public_id(&source.producer_name, &source.dataset),
        producer: source.producer_name.clone(),
        producer_version: source.producer_version.clone(),
        dataset: source.dataset.clone(),
        provenance: source.provenance_kind.clone(),
        capabilities: source
            .capabilities
            .split(',')
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
        repository_identity: source.repository_identity.clone(),
        revision: source.revision.clone(),
        manifest_sha256: source.manifest_sha256.clone(),
        manifest_bytes: source.manifest_bytes,
        imported_at: source.imported_at,
        status,
        facts_total: source
            .annotation_count
            .saturating_add(source.relationship_count),
        facts_returned,
        files_matched,
    }
}

fn validate_stored_files(
    store: &Store,
    root: &Path,
    source: &FactSourceRecord,
    verify_filesystem: bool,
) -> Result<(), FactError> {
    let files = store
        .fact_files(source.id, MAX_FILES.saturating_add(1))
        .map_err(|error| FactError::Store(error.to_string()))?;
    if files.len() != source.file_count as usize || files.len() > MAX_FILES {
        return Err(FactError::Store(
            "stored fact file inventory is inconsistent".into(),
        ));
    }
    for file in files {
        let indexed = store
            .file_content_sha256(&file.path)
            .map_err(|error| FactError::Store(error.to_string()))?;
        if indexed.as_deref().filter(|value| !value.is_empty()) != Some(file.sha256.as_str()) {
            return Err(FactError::InvalidQuery(format!(
                "source file `{}` no longer matches the codegraph",
                file.path
            )));
        }
        if verify_filesystem {
            let bytes = read_contained_regular(
                root,
                &file.path,
                crate::indexer::MAX_INDEXABLE_FILE_SIZE,
                &format!("source file `{}`", file.path),
                None,
            )?;
            if bytes.len() as u64 != file.bytes
                || crate::hex::encode(&Sha256::digest(&bytes)) != file.sha256
            {
                return Err(FactError::InvalidQuery(format!(
                    "source file `{}` changed after fact ingestion",
                    file.path
                )));
            }
        }
    }
    Ok(())
}

fn verify_artifact_record(
    root: &Path,
    artifact: &FactArtifactRecord,
    deadline: Option<Instant>,
) -> Result<(), FactError> {
    let label = format!("provenance artifact `{}`", artifact.id);
    let bytes = read_contained_regular(root, &artifact.path, MAX_ARTIFACT_BYTES, &label, deadline)?;
    if bytes.len() as u64 != artifact.bytes
        || crate::hex::encode(&Sha256::digest(&bytes)) != artifact.sha256
    {
        return Err(FactError::InvalidQuery(format!(
            "provenance artifact `{}` changed after fact ingestion",
            artifact.path
        )));
    }
    Ok(())
}

fn validate_stored_artifacts(
    store: &Store,
    root: &Path,
    source: &FactSourceRecord,
    deadline: Option<Instant>,
) -> Result<(), FactError> {
    let (artifacts, truncated) = store
        .fact_artifacts(&[source.id], MAX_ARTIFACTS.saturating_add(1))
        .map_err(|error| FactError::Store(error.to_string()))?;
    if truncated || artifacts.len() > MAX_ARTIFACTS {
        return Err(FactError::Store(
            "stored fact provenance inventory is inconsistent".into(),
        ));
    }
    for artifact in artifacts {
        verify_artifact_record(
            root,
            &FactArtifactRecord {
                id: artifact.id,
                path: artifact.path,
                sha256: artifact.sha256,
                bytes: artifact.bytes,
            },
            deadline,
        )?;
    }
    Ok(())
}

fn push_diagnostic(
    diagnostics: &mut Vec<FactDiagnostic>,
    truncated: &mut bool,
    source_id: String,
    code: &'static str,
    message: impl Into<String>,
) {
    if diagnostics.len() >= MAX_DIAGNOSTICS {
        *truncated = true;
        return;
    }
    diagnostics.push(FactDiagnostic {
        source_id,
        code,
        message: message.into().chars().take(300).collect(),
    });
}

pub fn snapshot(store: &Store, path: &str, top: usize) -> Result<FactSnapshot, FactError> {
    let path = normalize_query_path(path)?;
    snapshot_filtered(store, FactQueryFilter::Scope(path), top, true, None)
}

pub(crate) fn snapshot_for_paths(
    store: &Store,
    paths: &BTreeSet<String>,
    top: usize,
    deadline: Option<Instant>,
) -> Result<FactSnapshot, FactError> {
    let mut normalized = Vec::new();
    for path in paths.iter().take(1_000) {
        normalized.push(normalize_fact_path(path)?);
    }
    snapshot_filtered(
        store,
        FactQueryFilter::Paths(normalized),
        top,
        false,
        deadline,
    )
}

fn snapshot_filtered(
    store: &Store,
    filter: FactQueryFilter,
    top: usize,
    verify_filesystem: bool,
    deadline: Option<Instant>,
) -> Result<FactSnapshot, FactError> {
    if !(1..=MAX_FACTS_RETURNED).contains(&top) {
        return Err(FactError::InvalidQuery(format!(
            "top must be between 1 and {MAX_FACTS_RETURNED}"
        )));
    }
    let index_version = store
        .data_version()
        .map_err(|error| FactError::Store(error.to_string()))?;
    let contract = contract(store)?;
    let root = indexed_root(store)?;
    let mut source_rows = store
        .fact_sources(MAX_SOURCES_RETURNED.saturating_add(1))
        .map_err(|error| FactError::Store(error.to_string()))?;
    let sources_truncated = source_rows.len() > MAX_SOURCES_RETURNED;
    source_rows.truncate(MAX_SOURCES_RETURNED);
    let mut diagnostics = Vec::new();
    let mut diagnostics_truncated = false;
    let mut current_source_ids = Vec::new();
    let mut source_status = HashMap::new();
    for source in &source_rows {
        let public_id = source_public_id(&source.producer_name, &source.dataset);
        if source.api_version != API_VERSION
            || source.repository_identity != contract.repository.identity
            || source.revision != contract.repository.revision
        {
            source_status.insert(source.id, "stale");
            push_diagnostic(
                &mut diagnostics,
                &mut diagnostics_truncated,
                public_id,
                "fact_source_stale",
                "Imported facts were omitted because their repository or revision binding is stale.",
            );
            continue;
        }
        match validate_stored_files(store, &root, source, verify_filesystem)
            .and_then(|()| validate_stored_artifacts(store, &root, source, deadline))
        {
            Ok(()) => {
                source_status.insert(source.id, "loaded");
                current_source_ids.push(source.id);
            }
            Err(error) => {
                source_status.insert(source.id, "stale");
                push_diagnostic(
                    &mut diagnostics,
                    &mut diagnostics_truncated,
                    public_id,
                    "fact_source_stale",
                    error.to_string(),
                );
            }
        }
    }

    let (annotations, annotations_truncated) = store
        .fact_annotations(&current_source_ids, &filter, top)
        .map_err(|error| FactError::Store(error.to_string()))?;
    let (artifacts, artifacts_truncated) = store
        .fact_artifacts(&current_source_ids, MAX_ARTIFACTS_RETURNED)
        .map_err(|error| FactError::Store(error.to_string()))?;
    let remaining = top.saturating_sub(annotations.len());
    let (relationships, relationships_truncated) = if remaining == 0 {
        let (probe, _) = store
            .fact_relationships(&current_source_ids, &filter, 1)
            .map_err(|error| FactError::Store(error.to_string()))?;
        (Vec::new(), !probe.is_empty())
    } else {
        store
            .fact_relationships(&current_source_ids, &filter, remaining)
            .map_err(|error| FactError::Store(error.to_string()))?
    };
    let fact_limit_reached = annotations_truncated || relationships_truncated;

    let mut returned_by_source = HashMap::<String, (u32, BTreeSet<String>)>::new();
    for annotation in &annotations {
        let entry = returned_by_source
            .entry(annotation.source_id.clone())
            .or_default();
        entry.0 = entry.0.saturating_add(1);
        entry.1.insert(annotation.path.clone());
    }
    for relationship in &relationships {
        let entry = returned_by_source
            .entry(relationship.source_id.clone())
            .or_default();
        entry.0 = entry.0.saturating_add(1);
        entry.1.insert(relationship.from_path.clone());
        entry.1.insert(relationship.to_path.clone());
    }
    let sources = source_rows
        .iter()
        .map(|source| {
            let public_id = source_public_id(&source.producer_name, &source.dataset);
            let (returned, files) = returned_by_source
                .get(&public_id)
                .map(|(count, files)| (*count, files.len() as u32))
                .unwrap_or((0, 0));
            source_view(
                source,
                source_status.get(&source.id).copied().unwrap_or("stale"),
                returned,
                files,
            )
        })
        .collect::<Vec<_>>();

    let stale_sources = sources.iter().any(|source| source.status == "stale");
    let source_count = sources.len() as u32;
    let artifact_count = artifacts.len() as u32;
    let diagnostic_count = diagnostics.len() as u32;
    let after_contract = self::contract(store)?;
    if store
        .data_version()
        .map_err(|error| FactError::Store(error.to_string()))?
        != index_version
        || after_contract.repository.identity != contract.repository.identity
        || after_contract.repository.revision != contract.repository.revision
    {
        return Err(FactError::InvalidQuery(
            "repository or fact-store snapshot changed during the query; retry".into(),
        ));
    }
    let partial = sources_truncated
        || stale_sources
        || artifacts_truncated
        || fact_limit_reached
        || diagnostics_truncated;
    Ok(FactSnapshot {
        schema_version: 1,
        available: sources.iter().any(|source| source.status == "loaded"),
        partial,
        contract,
        sources: FactCollection {
            total: (!sources_truncated).then_some(source_count),
            returned: source_count,
            truncated: sources_truncated,
            truncation_reason: sources_truncated.then_some("source_limit"),
            items: sources,
        },
        artifacts: FactCollection {
            total: (!artifacts_truncated).then_some(artifact_count),
            returned: artifact_count,
            truncated: artifacts_truncated,
            truncation_reason: artifacts_truncated.then_some("provenance_artifact_limit"),
            items: artifacts,
        },
        annotations: FactCollection {
            total: (!annotations_truncated).then_some(annotations.len() as u32),
            returned: annotations.len() as u32,
            truncated: annotations_truncated,
            truncation_reason: annotations_truncated.then_some("fact_limit"),
            items: annotations,
        },
        relationships: FactCollection {
            total: (!relationships_truncated).then_some(relationships.len() as u32),
            returned: relationships.len() as u32,
            truncated: relationships_truncated,
            truncation_reason: relationships_truncated.then_some("fact_limit"),
            items: relationships,
        },
        diagnostics: FactCollection {
            total: (!diagnostics_truncated).then_some(diagnostic_count),
            returned: diagnostic_count,
            truncated: diagnostics_truncated,
            truncation_reason: diagnostics_truncated.then_some("diagnostic_limit"),
            items: diagnostics,
        },
        limits: FactLimits {
            sources: MAX_SOURCES_RETURNED as u32,
            facts: MAX_FACTS_RETURNED as u32,
            diagnostics: MAX_DIAGNOSTICS as u32,
            provenance_artifacts: MAX_ARTIFACTS_RETURNED as u32,
            manifest_bytes: MAX_MANIFEST_BYTES,
            files_per_manifest: MAX_FILES as u32,
            source_bytes_per_manifest: MAX_SOURCE_BYTES,
            artifacts_per_manifest: MAX_ARTIFACTS as u32,
            artifact_bytes: MAX_ARTIFACT_BYTES,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::Indexer;
    use serde_json::{json, Value};
    use std::fs;
    use std::process::Command;

    struct Fixture {
        root: tempfile::TempDir,
        store: Store,
        contract: FactContract,
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::create_dir_all(root.path().join("reports")).unwrap();
        fs::write(
            root.path().join("src/lib.rs"),
            "pub fn charge() { helper(); }\nfn helper() {}\n",
        )
        .unwrap();
        fs::write(
            root.path().join("src/worker.rs"),
            "pub fn run() { crate::charge(); }\n",
        )
        .unwrap();
        fs::write(
            root.path().join("reports/analyzer.json"),
            b"{\"ok\":true}\n",
        )
        .unwrap();
        git(root.path(), &["init", "-q"]);
        git(
            root.path(),
            &["config", "user.email", "facts@example.invalid"],
        );
        git(root.path(), &["config", "user.name", "Facts Test"]);
        git(
            root.path(),
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:example/fact-fixture.git",
            ],
        );
        git(root.path(), &["add", "."]);
        git(root.path(), &["commit", "-qm", "fixture"]);
        let db = root.path().join(".mastermind/mmcg.db");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        let mut store = Store::open(&db).unwrap();
        Indexer::new(root.path())
            .index_all(&mut store, true)
            .unwrap();
        let contract = contract(&store).unwrap();
        Fixture {
            root,
            store,
            contract,
        }
    }

    fn digest(path: &Path) -> String {
        crate::hex::encode(&Sha256::digest(fs::read(path).unwrap()))
    }

    fn file_binding(root: &Path, path: &str) -> Value {
        let absolute = root.join(path);
        json!({
            "path": path,
            "sha256": digest(&absolute),
            "bytes": fs::metadata(absolute).unwrap().len(),
        })
    }

    fn manifest(fixture: &Fixture) -> Value {
        json!({
            "api_version": API_VERSION,
            "capabilities": ["annotations", "relationships"],
            "repository": {
                "identity": fixture.contract.repository.identity,
                "revision": fixture.contract.repository.revision,
            },
            "producer": {
                "name": "com.example.arch-lint",
                "version": "1.4.0"
            },
            "dataset": "default",
            "provenance": {
                "kind": "static-analysis",
                "artifacts": ["analyzer-output"]
            },
            "files": [
                file_binding(fixture.root.path(), "src/lib.rs"),
                file_binding(fixture.root.path(), "src/worker.rs")
            ],
            "artifacts": [{
                "id": "analyzer-output",
                "path": "reports/analyzer.json",
                "sha256": digest(&fixture.root.path().join("reports/analyzer.json")),
                "bytes": fs::metadata(fixture.root.path().join("reports/analyzer.json")).unwrap().len()
            }],
            "facts": [
                {
                    "kind": "annotation",
                    "id": "architecture.payment-boundary",
                    "path": "src/lib.rs",
                    "line": 1,
                    "column": 8,
                    "severity": "warning",
                    "category": "architecture.boundary",
                    "title": "Payment boundary crossed",
                    "message": "The changed function crosses the payment ownership boundary."
                },
                {
                    "kind": "relationship",
                    "id": "architecture.worker-to-payment",
                    "relation": "calls",
                    "from": {"path": "src/worker.rs", "line": 1, "column": 16},
                    "to": {"path": "src/lib.rs", "line": 1, "column": 8},
                    "confidence": "high",
                    "label": "Compiler-resolved worker to payment call"
                }
            ]
        })
    }

    fn write_manifest(fixture: &Fixture, value: &Value, name: &str) -> PathBuf {
        let path = fixture.root.path().join(name);
        fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        path
    }

    #[test]
    fn imports_and_queries_revision_bound_normalized_facts() {
        let fixture = fixture();
        let path = write_manifest(&fixture, &manifest(&fixture), "facts.json");
        let summary = import(&fixture.store, &path).unwrap();
        assert!(!summary.replaced_previous_dataset);
        assert_eq!(summary.contract.api_version, API_VERSION);
        assert_eq!(summary.source.producer, "com.example.arch-lint");
        assert_eq!(summary.source.facts_returned, 2);
        assert_eq!(summary.source.files_matched, 2);

        let snapshot = snapshot(&fixture.store, ".", 100).unwrap();
        assert!(snapshot.available);
        assert!(!snapshot.partial);
        assert_eq!(snapshot.sources.items.len(), 1);
        assert_eq!(snapshot.sources.items[0].status, "loaded");
        assert_eq!(snapshot.sources.items[0].facts_total, 2);
        assert_eq!(snapshot.artifacts.items.len(), 1);
        assert_eq!(snapshot.artifacts.items[0].id, "analyzer-output");
        assert_eq!(snapshot.artifacts.items[0].path, "reports/analyzer.json");
        assert_eq!(snapshot.artifacts.items[0].sha256.len(), 64);
        assert_eq!(snapshot.annotations.items.len(), 1);
        assert_eq!(snapshot.annotations.items[0].path, "src/lib.rs");
        assert_eq!(snapshot.relationships.items.len(), 1);
        assert_eq!(snapshot.relationships.items[0].confidence, "high");
        assert!(snapshot.relationships.items[0]
            .source_id
            .starts_with("facts:sha256:"));
    }

    #[test]
    fn invalid_replacement_is_rejected_before_the_previous_dataset_is_touched() {
        let fixture = fixture();
        let valid = manifest(&fixture);
        let valid_path = write_manifest(&fixture, &valid, "facts.json");
        import(&fixture.store, &valid_path).unwrap();

        let mut invalid = valid;
        invalid["facts"][0]["path"] = json!("../src/lib.rs");
        let invalid_path = write_manifest(&fixture, &invalid, "invalid-facts.json");
        assert!(import(&fixture.store, &invalid_path)
            .unwrap_err()
            .to_string()
            .contains("fact paths"));

        let snapshot = snapshot(&fixture.store, ".", 100).unwrap();
        assert_eq!(snapshot.annotations.items.len(), 1);
        assert_eq!(
            snapshot.annotations.items[0].fact_id,
            "architecture.payment-boundary"
        );
    }

    #[test]
    fn strict_manifest_rejects_duplicate_unknown_and_unsupported_contract_fields() {
        let fixture = fixture();
        let value = manifest(&fixture);
        let mut unknown = value.clone();
        unknown["command"] = json!("./plugin");
        let path = write_manifest(&fixture, &unknown, "unknown.json");
        assert!(import(&fixture.store, &path).is_err());

        let raw = serde_json::to_string(&value).unwrap();
        let duplicate = raw.replacen("{", "{\"api_version\":\"mastermind-facts/v1\",", 1);
        let duplicate_path = fixture.root.path().join("duplicate.json");
        fs::write(&duplicate_path, duplicate).unwrap();
        assert!(import(&fixture.store, &duplicate_path)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));

        let mut unsupported = value;
        unsupported["capabilities"] = json!(["native-code"]);
        let path = write_manifest(&fixture, &unsupported, "unsupported.json");
        assert!(import(&fixture.store, &path)
            .unwrap_err()
            .to_string()
            .contains("unsupported capability"));

        for noncanonical in ["./src/lib.rs", "src/./lib.rs", "src/lib.rs/"] {
            let mut invalid_path = manifest(&fixture);
            invalid_path["files"][0]["path"] = json!(noncanonical);
            let path = write_manifest(&fixture, &invalid_path, "noncanonical.json");
            assert!(
                import(&fixture.store, &path).is_err(),
                "accepted {noncanonical}"
            );
        }
    }

    #[test]
    fn revision_and_file_drift_fail_closed() {
        let fixture = fixture();
        let mut wrong_revision = manifest(&fixture);
        wrong_revision["repository"]["revision"] =
            json!("0000000000000000000000000000000000000000");
        let path = write_manifest(&fixture, &wrong_revision, "wrong-revision.json");
        assert!(import(&fixture.store, &path)
            .unwrap_err()
            .to_string()
            .contains("revision"));

        let path = write_manifest(&fixture, &manifest(&fixture), "facts.json");
        import(&fixture.store, &path).unwrap();
        fs::write(
            fixture.root.path().join("src/lib.rs"),
            "pub fn charge() { changed(); }\nfn changed() {}\n",
        )
        .unwrap();
        let snapshot = snapshot(&fixture.store, ".", 100).unwrap();
        assert!(!snapshot.available);
        assert!(snapshot.partial);
        assert!(snapshot.annotations.items.is_empty());
        assert_eq!(snapshot.sources.items[0].status, "stale");
        assert_eq!(snapshot.diagnostics.items[0].code, "fact_source_stale");
    }

    #[test]
    fn provenance_drift_suppresses_normalized_facts() {
        let fixture = fixture();
        let path = write_manifest(&fixture, &manifest(&fixture), "facts.json");
        import(&fixture.store, &path).unwrap();
        fs::write(
            fixture.root.path().join("reports/analyzer.json"),
            b"{\"changed\":true}\n",
        )
        .unwrap();

        let snapshot = snapshot(&fixture.store, ".", 100).unwrap();
        assert!(!snapshot.available);
        assert!(snapshot.partial);
        assert!(snapshot.annotations.items.is_empty());
        assert!(snapshot.relationships.items.is_empty());
        assert!(snapshot.artifacts.items.is_empty());
        assert!(snapshot.diagnostics.items[0]
            .message
            .contains("provenance artifact"));
    }

    #[cfg(unix)]
    #[test]
    fn provenance_artifact_rejects_a_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let fixture = fixture();
        symlink(
            fixture.root.path().join("reports"),
            fixture.root.path().join("linked-reports"),
        )
        .unwrap();
        let mut value = manifest(&fixture);
        value["artifacts"][0]["path"] = json!("linked-reports/analyzer.json");
        let path = write_manifest(&fixture, &value, "symlinked-artifact.json");
        assert!(import(&fixture.store, &path)
            .unwrap_err()
            .to_string()
            .contains("symlink"));
    }

    #[test]
    fn repository_identity_sanitizes_remote_credentials_and_transport() {
        assert_eq!(
            canonical_remote("https://token@example.com/Owner/repo.git?secret=yes"),
            Some("example.com/Owner/repo".into())
        );
        assert_eq!(
            canonical_remote("git@example.com:Owner/repo.git"),
            Some("example.com/Owner/repo".into())
        );
        assert!(canonical_remote("/tmp/local-repo").is_none());
        assert_ne!(source_public_id("a/b", "c"), source_public_id("a", "b/c"));
    }

    #[test]
    fn public_schema_matches_runtime_contract() {
        let schema_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../schemas/mastermind-facts-v1.schema.json");
        if !schema_path.is_file() {
            // The root contract is repository-owned rather than duplicated in
            // the crates.io tarball. Repository validation covers it in CI.
            return;
        }
        let schema: Value = serde_json::from_slice(&fs::read(schema_path).unwrap()).unwrap();
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["properties"]["api_version"]["const"], API_VERSION);
        assert_eq!(
            schema["properties"]["capabilities"]["items"]["enum"],
            json!(SUPPORTED_CAPABILITIES)
        );
        assert_eq!(
            schema["required"],
            json!([
                "api_version",
                "capabilities",
                "repository",
                "producer",
                "dataset",
                "provenance",
                "files",
                "artifacts",
                "facts"
            ])
        );
        for definition in [
            "repository",
            "producer",
            "provenance",
            "file",
            "artifact",
            "location",
            "annotation",
            "relationship",
        ] {
            assert_eq!(
                schema["$defs"][definition]["additionalProperties"],
                json!(false)
            );
        }
    }
}
