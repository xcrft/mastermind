//! Local, read-only federation of multiple Mastermind indexes.
//!
//! A strict lock manifest pins each repository identity, Git revision, and the
//! exact SQLite database plus active WAL bytes. Queries reopen every index in
//! read-only snapshot mode, prove freshness in both directions, namespace all
//! nodes by repository ID, and add cross-repository topology only when the
//! manifest declares it explicitly.

use crate::store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub const API_VERSION: &str = "mastermind-team/v1";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_REPOSITORIES: usize = 16;
const MAX_RELATIONSHIPS: usize = 500;
const MAX_COMPONENTS_PER_REPOSITORY: usize = 20;
const MAX_INTERNAL_EDGE_PROBE: usize = 20_000;
const MAX_INTERNAL_EDGES_PER_REPOSITORY: usize = 200;
const MAX_INDEX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_INDEX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const DEFAULT_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamManifest {
    api_version: String,
    repositories: Vec<TeamRepositoryManifest>,
    relationships: Vec<TeamRelationshipManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamRepositoryManifest {
    id: String,
    root: String,
    index: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    index_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamRelationshipManifest {
    id: String,
    relation: String,
    from: TeamEndpointManifest,
    to: TeamEndpointManifest,
    label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamEndpointManifest {
    repository: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    component: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TeamLockSummary {
    pub schema_version: u32,
    pub api_version: &'static str,
    pub output: String,
    pub manifest_sha256: String,
    pub repositories: u32,
    pub relationships: u32,
}

#[derive(Debug, Serialize)]
pub struct TeamGraph {
    pub schema_version: u32,
    pub api_version: &'static str,
    pub partial: bool,
    pub repositories: TeamCollection<TeamRepositoryView>,
    pub nodes: TeamCollection<TeamNode>,
    pub edges: TeamCollection<TeamEdge>,
    pub diagnostics: TeamCollection<TeamDiagnostic>,
    pub limits: TeamLimits,
}

#[derive(Debug, Serialize)]
pub struct TeamCollection<T> {
    pub total: Option<u32>,
    pub returned: u32,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<&'static str>,
    pub items: Vec<T>,
}

#[derive(Debug, Serialize)]
pub struct TeamRepositoryView {
    pub id: String,
    pub repository_identity: String,
    pub revision: String,
    pub index_digest: String,
    pub files: u32,
    pub languages: Vec<String>,
    pub components_total: Option<u32>,
    pub components_returned: u32,
}

#[derive(Debug, Serialize)]
pub struct TeamNode {
    pub id: String,
    pub kind: &'static str,
    pub repository: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    pub label: String,
    pub files: u32,
    pub languages: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TeamEdge {
    pub id: String,
    pub kind: &'static str,
    pub relation: String,
    pub from: String,
    pub to: String,
    pub confidence: &'static str,
    pub provenance: &'static str,
    pub label: String,
    pub observations: u32,
}

#[derive(Debug, Serialize)]
pub struct TeamDiagnostic {
    pub repository: String,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct TeamLimits {
    pub repositories: u32,
    pub relationships: u32,
    pub components_per_repository: u32,
    pub internal_edges_per_repository: u32,
    pub index_file_bytes: u64,
    pub index_total_bytes: u64,
}

#[derive(Debug)]
pub enum TeamError {
    Contract(String),
    Stale(String),
    Io(String),
}

impl fmt::Display for TeamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(message) => write!(formatter, "team manifest error: {message}"),
            Self::Stale(message) => write!(formatter, "team index stale: {message}"),
            Self::Io(message) => write!(formatter, "team graph I/O error: {message}"),
        }
    }
}

impl std::error::Error for TeamError {}

#[derive(Clone)]
struct ResolvedRepository {
    manifest: TeamRepositoryManifest,
    root: PathBuf,
    index: PathBuf,
}

fn valid_token(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
        })
}

fn valid_repository_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validate_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn normalized_component(value: &str) -> Result<String, TeamError> {
    let normalized = crate::queries::normalize_map_path(value)
        .map_err(|error| TeamError::Contract(error.to_string()))?;
    if normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
        > 2
    {
        return Err(TeamError::Contract(
            "team component endpoints must use the depth-2 component model".into(),
        ));
    }
    let normalized = if normalized.is_empty() {
        ".".into()
    } else {
        normalized
    };
    if value != normalized {
        return Err(TeamError::Contract(
            "team component endpoints must be canonical paths".into(),
        ));
    }
    Ok(normalized)
}

fn read_manifest(
    path: &Path,
    allowed_root: Option<&Path>,
    expected_digest: Option<&str>,
) -> Result<TeamManifest, TeamError> {
    let requested = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| TeamError::Io(error.to_string()))?
            .join(path)
    };
    let canonical = requested
        .canonicalize()
        .map_err(|error| TeamError::Io(format!("resolve manifest: {error}")))?;
    if let Some(root) = allowed_root {
        let root = root
            .canonicalize()
            .map_err(|error| TeamError::Io(format!("resolve allowed root: {error}")))?;
        if !canonical.starts_with(root) {
            return Err(TeamError::Contract(
                "MCP team manifests must live inside the server repository".into(),
            ));
        }
    }
    let bytes = crate::audit_bundle::read_bounded_regular(&requested, false, true)
        .map_err(|error| TeamError::Io(error.to_string()))?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(TeamError::Contract(format!(
            "manifest exceeds {MAX_MANIFEST_BYTES} bytes"
        )));
    }
    if let Some(expected) = expected_digest {
        if !validate_digest(expected)
            || expected != format!("sha256:{}", crate::hex::encode(&Sha256::digest(&bytes)))
        {
            return Err(TeamError::Contract(
                "team manifest does not match its authorized SHA-256 digest".into(),
            ));
        }
    }
    let manifest: TeamManifest = crate::audit_bundle::from_json_strict(&bytes)
        .map_err(|error| TeamError::Contract(error.to_string()))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_manifest(manifest: &TeamManifest) -> Result<(), TeamError> {
    if manifest.api_version != API_VERSION {
        return Err(TeamError::Contract(format!(
            "unsupported api_version `{}`; expected {API_VERSION}",
            manifest.api_version
        )));
    }
    if manifest.repositories.is_empty() || manifest.repositories.len() > MAX_REPOSITORIES {
        return Err(TeamError::Contract(format!(
            "repositories must contain 1..={MAX_REPOSITORIES} entries"
        )));
    }
    if manifest.relationships.len() > MAX_RELATIONSHIPS {
        return Err(TeamError::Contract(format!(
            "relationships exceed {MAX_RELATIONSHIPS} entries"
        )));
    }
    let mut repository_ids = HashSet::new();
    for repository in &manifest.repositories {
        if !valid_repository_id(&repository.id) {
            return Err(TeamError::Contract(
                "repository IDs must use 1..=128 ASCII letters, digits, dots, underscores, or hyphens".into(),
            ));
        }
        if !repository_ids.insert(repository.id.as_str()) {
            return Err(TeamError::Contract(format!(
                "duplicate repository ID `{}`",
                repository.id
            )));
        }
        for (label, value) in [("root", &repository.root), ("index", &repository.index)] {
            if value.is_empty()
                || value.len() > 4096
                || value.chars().any(|character| character.is_control())
            {
                return Err(TeamError::Contract(format!(
                    "repository {} has an invalid {label} path",
                    repository.id
                )));
            }
        }
        if repository
            .index_digest
            .as_deref()
            .is_some_and(|value| !validate_digest(value))
        {
            return Err(TeamError::Contract(format!(
                "repository {} has an invalid index_digest",
                repository.id
            )));
        }
        if repository
            .repository_identity
            .as_deref()
            .is_some_and(|value| {
                !value
                    .strip_prefix("git-remote:sha256:")
                    .or_else(|| value.strip_prefix("git-worktree:sha256:"))
                    .is_some_and(|digest| {
                        digest.len() == 64
                            && digest
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    })
            })
        {
            return Err(TeamError::Contract(format!(
                "repository {} has an invalid repository_identity",
                repository.id
            )));
        }
        if repository.revision.as_deref().is_some_and(|value| {
            !matches!(value.len(), 40 | 64)
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Err(TeamError::Contract(format!(
                "repository {} has an invalid revision",
                repository.id
            )));
        }
    }
    let mut relationship_ids = HashSet::new();
    let mut declared_components = BTreeMap::<String, BTreeSet<String>>::new();
    for relationship in &manifest.relationships {
        if !valid_token(&relationship.id, 256)
            || relationship.id.starts_with("team:internal:")
            || !valid_token(&relationship.relation, 128)
            || relationship.label.is_empty()
            || relationship.label.len() > 512
            || relationship.label.chars().any(char::is_control)
        {
            return Err(TeamError::Contract(
                "relationship IDs, relations, and labels must be bounded inert text".into(),
            ));
        }
        if !relationship_ids.insert(relationship.id.as_str()) {
            return Err(TeamError::Contract(format!(
                "duplicate relationship ID `{}`",
                relationship.id
            )));
        }
        if relationship.from.repository == relationship.to.repository
            || !repository_ids.contains(relationship.from.repository.as_str())
            || !repository_ids.contains(relationship.to.repository.as_str())
        {
            return Err(TeamError::Contract(format!(
                "relationship {} must connect two declared, distinct repositories",
                relationship.id
            )));
        }
        for endpoint in [&relationship.from, &relationship.to] {
            if let Some(component) = &endpoint.component {
                let normalized = normalized_component(component)?;
                let components = declared_components
                    .entry(endpoint.repository.clone())
                    .or_default();
                components.insert(normalized);
                if components.len() > MAX_COMPONENTS_PER_REPOSITORY {
                    return Err(TeamError::Contract(format!(
                        "repository {} declares more than {MAX_COMPONENTS_PER_REPOSITORY} component endpoints",
                        endpoint.repository
                    )));
                }
            }
        }
    }
    Ok(())
}

fn resolve_path(base: &Path, value: &str, label: &str) -> Result<PathBuf, TeamError> {
    let requested = Path::new(value);
    let joined = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        base.join(requested)
    };
    let metadata = std::fs::symlink_metadata(&joined)
        .map_err(|error| TeamError::Io(format!("read {label} metadata: {error}")))?;
    if metadata.file_type().is_symlink() {
        return Err(TeamError::Contract(format!(
            "{label} must not be a symlink"
        )));
    }
    joined
        .canonicalize()
        .map_err(|error| TeamError::Io(format!("resolve {label}: {error}")))
}

fn resolve_repositories(
    manifest_path: &Path,
    manifest: &TeamManifest,
) -> Result<Vec<ResolvedRepository>, TeamError> {
    let canonical_manifest = manifest_path
        .canonicalize()
        .map_err(|error| TeamError::Io(format!("resolve manifest: {error}")))?;
    let base = canonical_manifest
        .parent()
        .unwrap_or_else(|| Path::new("/"));
    let mut roots = HashSet::new();
    let mut indexes = HashSet::new();
    let mut output = Vec::new();
    for repository in &manifest.repositories {
        let root = resolve_path(base, &repository.root, "repository root")?;
        let index = resolve_path(base, &repository.index, "repository index")?;
        if !root.is_dir() || !index.is_file() {
            return Err(TeamError::Contract(format!(
                "repository {} root/index types are invalid",
                repository.id
            )));
        }
        if !roots.insert(root.clone()) || !indexes.insert(index.clone()) {
            return Err(TeamError::Contract(
                "repository roots and indexes must be unique after canonicalization".into(),
            ));
        }
        output.push(ResolvedRepository {
            manifest: repository.clone(),
            root,
            index,
        });
    }
    Ok(output)
}

fn modified(metadata: &std::fs::Metadata) -> Option<SystemTime> {
    metadata.modified().ok()
}

fn open_index_file(path: &Path) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        File::open(path)
    }
}

fn hash_regular(
    path: &Path,
    deadline: Instant,
    total_bytes: &mut u64,
) -> Result<Option<(String, u64)>, TeamError> {
    let initial = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(TeamError::Io(format!("read index metadata: {error}"))),
    };
    if !initial.file_type().is_file() || initial.len() > MAX_INDEX_FILE_BYTES {
        return Err(TeamError::Contract(
            "index database/WAL must be bounded regular files".into(),
        ));
    }
    let mut file = open_index_file(path).map_err(|error| TeamError::Io(error.to_string()))?;
    let before = file
        .metadata()
        .map_err(|error| TeamError::Io(error.to_string()))?;
    if !before.is_file()
        || before.len() > MAX_INDEX_FILE_BYTES
        || initial.len() != before.len()
        || modified(&initial) != modified(&before)
    {
        return Err(TeamError::Stale(
            "index database or WAL changed before it was hashed".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if initial.dev() != before.dev() || initial.ino() != before.ino() {
            return Err(TeamError::Stale(
                "index database or WAL identity changed before it was hashed".into(),
            ));
        }
    }
    *total_bytes = total_bytes
        .checked_add(before.len())
        .ok_or_else(|| TeamError::Contract("index byte total overflow".into()))?;
    if *total_bytes > MAX_INDEX_TOTAL_BYTES {
        return Err(TeamError::Contract(format!(
            "team indexes exceed the {MAX_INDEX_TOTAL_BYTES}-byte total limit"
        )));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    let mut read_bytes = 0_u64;
    loop {
        if Instant::now() >= deadline {
            return Err(TeamError::Io("index digest deadline exceeded".into()));
        }
        let count = file
            .read(&mut buffer)
            .map_err(|error| TeamError::Io(error.to_string()))?;
        if count == 0 {
            break;
        }
        read_bytes = read_bytes
            .checked_add(count as u64)
            .ok_or_else(|| TeamError::Contract("index byte count overflow".into()))?;
        if read_bytes > before.len() || read_bytes > MAX_INDEX_FILE_BYTES {
            return Err(TeamError::Stale(
                "index database or WAL grew while it was hashed".into(),
            ));
        }
        hasher.update(&buffer[..count]);
    }
    let after = file
        .metadata()
        .map_err(|error| TeamError::Io(error.to_string()))?;
    if before.len() != after.len()
        || read_bytes != after.len()
        || modified(&before) != modified(&after)
    {
        return Err(TeamError::Stale(
            "index database or WAL changed while it was hashed".into(),
        ));
    }
    Ok(Some((crate::hex::encode(&hasher.finalize()), after.len())))
}

fn wal_path(index: &Path) -> PathBuf {
    let mut value = index.as_os_str().to_os_string();
    value.push("-wal");
    PathBuf::from(value)
}

fn index_digest(
    index: &Path,
    deadline: Instant,
    total_bytes: &mut u64,
) -> Result<String, TeamError> {
    let database = hash_regular(index, deadline, total_bytes)?
        .ok_or_else(|| TeamError::Io("index database disappeared".into()))?;
    let wal = hash_regular(&wal_path(index), deadline, total_bytes)?;
    let statement = serde_json::json!({
        "database": {"sha256": database.0, "bytes": database.1},
        "domain": "mastermind/team-index-snapshot/v1",
        "wal": wal.as_ref().map(|(sha256, bytes)| serde_json::json!({
            "sha256": sha256,
            "bytes": bytes,
        })),
    });
    let canonical = crate::audit_bundle::canonical_json(&statement)
        .map_err(|error| TeamError::Contract(error.to_string()))?;
    Ok(format!(
        "sha256:{}",
        crate::hex::encode(&Sha256::digest(canonical))
    ))
}

fn inspect_repository(
    repository: &ResolvedRepository,
    deadline: Instant,
    total_bytes: &mut u64,
) -> Result<(Store, String, String, String), TeamError> {
    let digest_before = index_digest(&repository.index, deadline, total_bytes)?;
    let store = Store::open_read_only_with_deadline(&repository.index, Some(deadline))
        .map_err(|error| TeamError::Io(error.to_string()))?;
    crate::lens::validate_index_snapshot(&store, &repository.root, Some(deadline))
        .map_err(|error| TeamError::Stale(format!("{}: {error}", repository.manifest.id)))?;
    let contract = crate::facts::contract(&store)
        .map_err(|error| TeamError::Stale(format!("{}: {error}", repository.manifest.id)))?;
    let mut verification_bytes = 0_u64;
    let digest_after = index_digest(&repository.index, deadline, &mut verification_bytes)?;
    if digest_before != digest_after {
        return Err(TeamError::Stale(format!(
            "{} index changed during inspection",
            repository.manifest.id
        )));
    }
    Ok((
        store,
        contract.repository.identity,
        contract.repository.revision,
        digest_after,
    ))
}

pub fn lock(manifest_path: &Path, output: &Path) -> Result<TeamLockSummary, TeamError> {
    let mut manifest = read_manifest(manifest_path, None, None)?;
    let resolved = resolve_repositories(manifest_path, &manifest)?;
    let deadline = Instant::now() + DEFAULT_DEADLINE;
    let mut total_bytes = 0_u64;
    for (entry, repository) in manifest.repositories.iter_mut().zip(&resolved) {
        let (_, identity, revision, digest) =
            inspect_repository(repository, deadline, &mut total_bytes)?;
        entry.root = repository.root.to_string_lossy().into_owned();
        entry.index = repository.index.to_string_lossy().into_owned();
        entry.repository_identity = Some(identity);
        entry.revision = Some(revision);
        entry.index_digest = Some(digest);
    }
    let mut bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|error| TeamError::Io(error.to_string()))?;
    bytes.push(b'\n');
    let manifest_sha256 = format!("sha256:{}", crate::hex::encode(&Sha256::digest(&bytes)));
    crate::audit_bundle::write_atomic(output, &bytes, true)
        .map_err(|error| TeamError::Io(error.to_string()))?;
    Ok(TeamLockSummary {
        schema_version: 1,
        api_version: API_VERSION,
        output: output.to_string_lossy().into_owned(),
        manifest_sha256,
        repositories: manifest.repositories.len() as u32,
        relationships: manifest.relationships.len() as u32,
    })
}

fn component_for_file(path: &str) -> String {
    let parent = Path::new(path)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or("");
    if parent.is_empty() {
        return ".".into();
    }
    parent.split('/').take(2).collect::<Vec<_>>().join("/")
}

fn component_node_id(repository: &str, component: Option<&str>) -> String {
    match component {
        Some(component) => format!("repo:{repository}/component:{component}"),
        None => format!("repo:{repository}"),
    }
}

fn internal_edge_id(repository: &str, from: &str, to: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(repository.as_bytes());
    hasher.update([0]);
    hasher.update(from.as_bytes());
    hasher.update([0]);
    hasher.update(to.as_bytes());
    format!(
        "team:internal:sha256:{}",
        crate::hex::encode(&hasher.finalize())
    )
}

fn declared_components(manifest: &TeamManifest, repository: &str) -> BTreeSet<String> {
    manifest
        .relationships
        .iter()
        .flat_map(|relationship| [&relationship.from, &relationship.to])
        .filter(|endpoint| endpoint.repository == repository)
        .filter_map(|endpoint| endpoint.component.as_deref())
        .map(|component| normalized_component(component).expect("validated team component"))
        .collect()
}

fn with_repository_budget<T, E: fmt::Display>(
    store: &Store,
    deadline: Instant,
    operation: impl FnOnce() -> Result<T, E>,
) -> Result<T, TeamError> {
    let budget = crate::store::WorkBudget {
        deadline: Some(deadline.saturating_duration_since(Instant::now())),
        op_ticks: Some(250_000),
    };
    if store.push_work_budget(budget) {
        store.pop_work_budget();
        return Err(TeamError::Io("team query deadline exceeded".into()));
    }
    let result = operation();
    let interrupted = store.take_interrupt_source().is_some();
    store.pop_work_budget();
    if interrupted {
        return Err(TeamError::Io("team query work limit exceeded".into()));
    }
    result.map_err(|error| TeamError::Stale(error.to_string()))
}

pub fn map(
    manifest_path: &Path,
    allowed_manifest_root: Option<&Path>,
) -> Result<TeamGraph, TeamError> {
    map_with_policy(manifest_path, allowed_manifest_root, None)
}

pub fn map_authorized(
    manifest_path: &Path,
    allowed_manifest_root: &Path,
    expected_digest: &str,
) -> Result<TeamGraph, TeamError> {
    map_with_policy(
        manifest_path,
        Some(allowed_manifest_root),
        Some(expected_digest),
    )
}

fn map_with_policy(
    manifest_path: &Path,
    allowed_manifest_root: Option<&Path>,
    expected_digest: Option<&str>,
) -> Result<TeamGraph, TeamError> {
    let manifest = read_manifest(manifest_path, allowed_manifest_root, expected_digest)?;
    let resolved = resolve_repositories(manifest_path, &manifest)?;
    let deadline = Instant::now() + DEFAULT_DEADLINE;
    let mut total_bytes = 0_u64;
    let mut repositories = Vec::new();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut diagnostics = Vec::new();
    let mut partial = false;
    let mut nodes_truncated = false;
    let mut edges_truncated = false;
    let mut node_ids = BTreeSet::new();

    for repository in &resolved {
        let expected_identity = repository
            .manifest
            .repository_identity
            .as_deref()
            .ok_or_else(|| TeamError::Contract("run `mastermind team lock` first".into()))?;
        let expected_revision = repository
            .manifest
            .revision
            .as_deref()
            .ok_or_else(|| TeamError::Contract("run `mastermind team lock` first".into()))?;
        let expected_digest = repository
            .manifest
            .index_digest
            .as_deref()
            .ok_or_else(|| TeamError::Contract("run `mastermind team lock` first".into()))?;
        let (store, identity, revision, digest) =
            inspect_repository(repository, deadline, &mut total_bytes)?;
        if identity != expected_identity
            || revision != expected_revision
            || digest != expected_digest
        {
            return Err(TeamError::Stale(format!(
                "{} no longer matches its locked identity, revision, or index digest",
                repository.manifest.id
            )));
        }
        let project = with_repository_budget(&store, deadline, || {
            crate::queries::project_map_with_options(
                &store,
                ".",
                2,
                MAX_COMPONENTS_PER_REPOSITORY as u32,
                false,
            )
        })
        .map_err(|error| TeamError::Stale(format!("{}: {error}", repository.manifest.id)))?;
        let repository_node = component_node_id(&repository.manifest.id, None);
        node_ids.insert(repository_node.clone());
        nodes.push(TeamNode {
            id: repository_node,
            kind: "repository",
            repository: repository.manifest.id.clone(),
            component: None,
            label: repository.manifest.id.clone(),
            files: project.files.returned,
            languages: project
                .languages
                .items
                .iter()
                .map(|language| language.language.clone())
                .collect(),
        });
        let required_components = declared_components(&manifest, &repository.manifest.id);
        let mut retained = BTreeMap::<String, (u32, Vec<String>)>::new();
        for component in &project.components.items {
            if required_components.contains(&component.path) {
                retained.insert(
                    component.path.clone(),
                    (
                        component.file_count,
                        component
                            .languages
                            .iter()
                            .map(|language| language.language.clone())
                            .collect(),
                    ),
                );
            }
        }
        for component in &required_components {
            if retained.contains_key(component) {
                continue;
            }
            let scoped = with_repository_budget(&store, deadline, || {
                crate::queries::project_map_with_options(&store, component, 1, 1, false)
            })
            .map_err(|error| {
                TeamError::Contract(format!(
                    "relationship references unavailable component {} / {component}: {error}",
                    repository.manifest.id
                ))
            })?;
            retained.insert(
                component.clone(),
                (
                    scoped.files.returned,
                    scoped
                        .languages
                        .items
                        .into_iter()
                        .map(|language| language.language)
                        .collect(),
                ),
            );
        }
        for component in &project.components.items {
            if retained.len() >= MAX_COMPONENTS_PER_REPOSITORY {
                break;
            }
            retained.entry(component.path.clone()).or_insert_with(|| {
                (
                    component.file_count,
                    component
                        .languages
                        .iter()
                        .map(|language| language.language.clone())
                        .collect(),
                )
            });
        }
        let retained_components = retained.keys().cloned().collect::<BTreeSet<_>>();
        for (component, (file_count, languages)) in &retained {
            let id = component_node_id(&repository.manifest.id, Some(component));
            node_ids.insert(id.clone());
            nodes.push(TeamNode {
                id,
                kind: "component",
                repository: repository.manifest.id.clone(),
                component: Some(component.clone()),
                label: format!("{} / {component}", repository.manifest.id),
                files: *file_count,
                languages: languages.clone(),
            });
        }
        let component_projection_truncated = project.components.truncated
            || project
                .components
                .total
                .is_none_or(|total| total > retained.len() as u32);
        if component_projection_truncated {
            partial = true;
            nodes_truncated = true;
            edges_truncated = true;
            diagnostics.push(TeamDiagnostic {
                repository: repository.manifest.id.clone(),
                code: "component_limit",
                message: "Only the largest 20 components are present in the team graph.".into(),
            });
        }
        let (imports, imports_truncated) = with_repository_budget(&store, deadline, || {
            store.map_import_edges_capped_filtered("", "root", MAX_INTERNAL_EDGE_PROBE, false)
        })?;
        let mut aggregated = BTreeMap::<(String, String), u32>::new();
        for (from_file, to_file) in imports {
            let from = component_for_file(&from_file);
            let to = component_for_file(&to_file);
            if from == to
                || !retained_components.contains(&from)
                || !retained_components.contains(&to)
            {
                continue;
            }
            let count = aggregated.entry((from, to)).or_default();
            *count = count.saturating_add(1);
        }
        let internal_total = aggregated.len();
        for ((from, to), observations) in aggregated
            .into_iter()
            .take(MAX_INTERNAL_EDGES_PER_REPOSITORY)
        {
            edges.push(TeamEdge {
                id: internal_edge_id(&repository.manifest.id, &from, &to),
                kind: "internal",
                relation: "imports".into(),
                from: component_node_id(&repository.manifest.id, Some(&from)),
                to: component_node_id(&repository.manifest.id, Some(&to)),
                confidence: "medium",
                provenance: "tree-sitter",
                label: format!("{observations} syntactic import edge(s)"),
                observations,
            });
        }
        if imports_truncated || internal_total > MAX_INTERNAL_EDGES_PER_REPOSITORY {
            partial = true;
            edges_truncated = true;
            diagnostics.push(TeamDiagnostic {
                repository: repository.manifest.id.clone(),
                code: "internal_edge_limit",
                message: "Internal component edges were bounded; narrow the team manifest or inspect the repository map.".into(),
            });
        }
        repositories.push(TeamRepositoryView {
            id: repository.manifest.id.clone(),
            repository_identity: identity,
            revision,
            index_digest: digest,
            files: project.files.returned,
            languages: project
                .languages
                .items
                .into_iter()
                .map(|language| language.language)
                .collect(),
            components_total: project.components.total,
            components_returned: retained.len() as u32,
        });
    }

    for relationship in &manifest.relationships {
        let endpoint = |value: &TeamEndpointManifest| -> Result<String, TeamError> {
            let component = value.component.as_deref().map(|component| {
                normalized_component(component).expect("validated team component")
            });
            let id = component_node_id(&value.repository, component.as_deref());
            if !node_ids.contains(&id) {
                return Err(TeamError::Contract(format!(
                    "relationship {} references an unavailable component node `{id}`",
                    relationship.id
                )));
            }
            Ok(id)
        };
        edges.push(TeamEdge {
            id: relationship.id.clone(),
            kind: "cross_repository",
            relation: relationship.relation.clone(),
            from: endpoint(&relationship.from)?,
            to: endpoint(&relationship.to)?,
            confidence: "declared",
            provenance: "team-manifest",
            label: relationship.label.clone(),
            observations: 1,
        });
    }
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    edges.sort_by(|left, right| left.id.cmp(&right.id));
    repositories.sort_by(|left, right| left.id.cmp(&right.id));
    diagnostics
        .sort_by(|left, right| (&left.repository, left.code).cmp(&(&right.repository, right.code)));
    let repository_count = repositories.len() as u32;
    let node_count = nodes.len() as u32;
    let edge_count = edges.len() as u32;
    let diagnostic_count = diagnostics.len() as u32;
    Ok(TeamGraph {
        schema_version: 1,
        api_version: API_VERSION,
        partial,
        repositories: TeamCollection {
            total: Some(repository_count),
            returned: repository_count,
            truncated: false,
            truncation_reason: None,
            items: repositories,
        },
        nodes: TeamCollection {
            total: (!nodes_truncated).then_some(node_count),
            returned: node_count,
            truncated: nodes_truncated,
            truncation_reason: nodes_truncated.then_some("component_limit"),
            items: nodes,
        },
        edges: TeamCollection {
            total: (!edges_truncated).then_some(edge_count),
            returned: edge_count,
            truncated: edges_truncated,
            truncation_reason: edges_truncated.then_some("component_or_edge_limit"),
            items: edges,
        },
        diagnostics: TeamCollection {
            total: Some(diagnostic_count),
            returned: diagnostic_count,
            truncated: false,
            truncation_reason: None,
            items: diagnostics,
        },
        limits: TeamLimits {
            repositories: MAX_REPOSITORIES as u32,
            relationships: MAX_RELATIONSHIPS as u32,
            components_per_repository: MAX_COMPONENTS_PER_REPOSITORY as u32,
            internal_edges_per_repository: MAX_INTERNAL_EDGES_PER_REPOSITORY as u32,
            index_file_bytes: MAX_INDEX_FILE_BYTES,
            index_total_bytes: MAX_INDEX_TOTAL_BYTES,
        },
    })
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
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn directory_snapshot(path: &Path) -> Vec<(String, Vec<u8>)> {
        let mut entries = fs::read_dir(path)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }

    fn git_status(root: &Path) -> Vec<u8> {
        let output = Command::new("git")
            .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(output.status.success());
        output.stdout
    }

    fn repository(parent: &Path, name: &str, import: &str) -> (PathBuf, PathBuf) {
        let root = parent.join(name);
        fs::create_dir_all(root.join("src/api")).unwrap();
        fs::create_dir_all(root.join("src/core")).unwrap();
        fs::write(root.join("src/api/mod.rs"), import).unwrap();
        fs::write(root.join("src/core/lib.rs"), "pub fn core() {}\n").unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "team@example.test"]);
        git(&root, &["config", "user.name", "Team Test"]);
        git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                &format!("git@example.com:team/{name}.git"),
            ],
        );
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "fixture"]);
        let index = root.join(".mastermind/mmcg.db");
        fs::create_dir_all(index.parent().unwrap()).unwrap();
        let mut store = Store::open(&index).unwrap();
        Indexer::new(&root).index_all(&mut store, true).unwrap();
        (root, index)
    }

    #[test]
    fn lock_and_map_pin_two_fresh_read_only_indexes() {
        let parent = tempfile::tempdir().unwrap();
        let (one_root, one_index) = repository(
            parent.path(),
            "one",
            "use crate::core::core;\npub fn api() { core(); }\n",
        );
        let (two_root, two_index) = repository(
            parent.path(),
            "two",
            "use crate::core::core;\npub fn api() { core(); }\n",
        );
        let draft_path = parent.path().join("team.json");
        let lock_path = parent.path().join("team.lock.json");
        fs::write(
            &draft_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "api_version": API_VERSION,
                "repositories": [
                    {"id": "one", "root": one_root, "index": one_index},
                    {"id": "two", "root": two_root, "index": two_index}
                ],
                "relationships": [{
                    "id": "one-to-two",
                    "relation": "calls_service",
                    "from": {"repository": "one"},
                    "to": {"repository": "two"},
                    "label": "Declared API dependency"
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let one_index_before = directory_snapshot(one_index.parent().unwrap());
        let two_index_before = directory_snapshot(two_index.parent().unwrap());
        let one_worktree_before = git_status(&one_root);
        let two_worktree_before = git_status(&two_root);
        let locked = lock(&draft_path, &lock_path).unwrap();
        assert_eq!(locked.repositories, 2);
        assert!(map_authorized(
            &lock_path,
            parent.path(),
            &format!("sha256:{}", "0".repeat(64)),
        )
        .unwrap_err()
        .to_string()
        .contains("authorized SHA-256"));
        let lock_bytes = fs::read(&lock_path).unwrap();
        let lock_digest = format!(
            "sha256:{}",
            crate::hex::encode(&Sha256::digest(&lock_bytes))
        );
        assert_eq!(locked.manifest_sha256, lock_digest);
        let graph = map_authorized(&lock_path, parent.path(), &lock_digest).unwrap();
        assert!(!graph.partial);
        assert_eq!(graph.repositories.returned, 2);
        assert!(graph.edges.items.iter().any(|edge| {
            edge.kind == "cross_repository"
                && edge.from == "repo:one"
                && edge.to == "repo:two"
                && edge.confidence == "declared"
        }));
        assert!(graph
            .nodes
            .items
            .iter()
            .all(|node| node.id.starts_with("repo:")));
        assert_eq!(
            directory_snapshot(one_index.parent().unwrap()),
            one_index_before
        );
        assert_eq!(
            directory_snapshot(two_index.parent().unwrap()),
            two_index_before
        );
        assert_eq!(git_status(&one_root), one_worktree_before);
        assert_eq!(git_status(&two_root), two_worktree_before);
    }

    #[test]
    fn revision_drift_and_duplicate_canonical_indexes_fail_closed() {
        let parent = tempfile::tempdir().unwrap();
        let (root, index) = repository(parent.path(), "one", "pub fn api() {}\n");
        let draft_path = parent.path().join("team.json");
        let lock_path = parent.path().join("team.lock.json");
        fs::write(
            &draft_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "api_version": API_VERSION,
                "repositories": [{"id": "one", "root": root, "index": index}],
                "relationships": []
            }))
            .unwrap(),
        )
        .unwrap();
        lock(&draft_path, &lock_path).unwrap();
        fs::write(root.join("src/new.rs"), "pub fn changed() {}\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "drift"]);
        assert!(map(&lock_path, None)
            .unwrap_err()
            .to_string()
            .contains("stale"));

        let duplicate = parent.path().join("duplicate.json");
        fs::write(
            &duplicate,
            serde_json::to_vec_pretty(&serde_json::json!({
                "api_version": API_VERSION,
                "repositories": [
                    {"id": "a", "root": root, "index": index},
                    {"id": "b", "root": root, "index": index}
                ],
                "relationships": []
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(lock(&duplicate, &parent.path().join("out.json"))
            .unwrap_err()
            .to_string()
            .contains("unique"));
    }

    #[test]
    fn repository_namespace_and_internal_edge_ids_are_unambiguous() {
        let repository = |id: &str| TeamRepositoryManifest {
            id: id.into(),
            root: "/tmp/repository".into(),
            index: "/tmp/repository/index.db".into(),
            repository_identity: None,
            revision: None,
            index_digest: None,
        };
        let ambiguous = TeamManifest {
            api_version: API_VERSION.into(),
            repositories: vec![repository("one"), repository("one/component:api")],
            relationships: Vec::new(),
        };
        assert!(validate_manifest(&ambiguous)
            .unwrap_err()
            .to_string()
            .contains("repository IDs"));

        let reserved = TeamManifest {
            api_version: API_VERSION.into(),
            repositories: vec![repository("one"), repository("two")],
            relationships: vec![TeamRelationshipManifest {
                id: format!("team:internal:sha256:{}", "a".repeat(64)),
                relation: "calls".into(),
                from: TeamEndpointManifest {
                    repository: "one".into(),
                    component: None,
                },
                to: TeamEndpointManifest {
                    repository: "two".into(),
                    component: None,
                },
                label: "Reserved edge namespace".into(),
            }],
        };
        assert!(validate_manifest(&reserved).is_err());
    }

    #[test]
    fn declared_component_is_retained_even_when_it_is_outside_the_top_twenty() {
        let parent = tempfile::tempdir().unwrap();
        let (one_root, one_index) = repository(parent.path(), "one", "pub fn api() {}\n");
        for number in 0..24 {
            let directory = one_root.join(format!("packages/c{number:02}"));
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("lib.rs"), "pub fn component() {}\n").unwrap();
        }
        fs::create_dir_all(one_root.join("zz/declared")).unwrap();
        fs::write(
            one_root.join("zz/declared/lib.rs"),
            "pub fn declared_target() {}\n",
        )
        .unwrap();
        git(&one_root, &["add", "."]);
        git(&one_root, &["commit", "-qm", "many components"]);
        let mut store = Store::open(&one_index).unwrap();
        Indexer::new(&one_root).index_all(&mut store, true).unwrap();
        assert!(store.file_mtime("zz/declared/lib.rs").unwrap().is_some());
        drop(store);
        let (two_root, two_index) = repository(parent.path(), "two", "pub fn api() {}\n");
        let draft = parent.path().join("team.json");
        let locked = parent.path().join("team.lock.json");
        fs::write(
            &draft,
            serde_json::to_vec_pretty(&serde_json::json!({
                "api_version": API_VERSION,
                "repositories": [
                    {"id": "one", "root": one_root, "index": one_index},
                    {"id": "two", "root": two_root, "index": two_index}
                ],
                "relationships": [{
                    "id": "target-to-two",
                    "relation": "calls_service",
                    "from": {"repository": "one", "component": "zz/declared"},
                    "to": {"repository": "two"},
                    "label": "Declared low-ranked component dependency"
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        lock(&draft, &locked).unwrap();
        let graph = map(&locked, None).unwrap();
        assert!(graph.partial);
        assert!(graph.nodes.truncated);
        assert!(graph.edges.truncated);
        assert!(graph
            .nodes
            .items
            .iter()
            .any(|node| node.id == "repo:one/component:zz/declared"));
        assert!(graph.edges.items.iter().any(|edge| {
            edge.id == "target-to-two"
                && edge.from == "repo:one/component:zz/declared"
                && edge.to == "repo:two"
        }));
    }
}
