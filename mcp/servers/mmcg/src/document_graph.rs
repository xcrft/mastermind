//! Live, root-bound validation of portable document-evidence graph snapshots.
//!
//! The portable Python helper remains the snapshot producer. Native consumers
//! read one explicitly selected packet without following links, validate its
//! complete v1/v2 contract, and re-check endpoint and optional Markdown-corpus
//! bytes. A fresh hash never upgrades a declared relation from `unverified`.

use crate::bounded_fs::{
    read_directory_receipt_with_capability, read_regular_file_expected,
    read_regular_file_with_capability, BoundedPathKind, BoundedReadError, ReadControl,
    RootCapability, StableFileIdentity,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

const GRAPH_BYTES: u64 = 4 * 1024 * 1024;
const FILE_BYTES: u64 = 1024 * 1024;
const TOTAL_BYTES: u64 = 16 * 1024 * 1024;
const RELATION_LIMIT: usize = 256;
const ENDPOINT_FILE_LIMIT: usize = 128;
const CORPUS_DIRECTORY_LIMIT: usize = 8;
const CORPUS_FILE_LIMIT: usize = 256;
const CORPUS_ENTRY_LIMIT: usize = 8192;
const CORPUS_DEPTH_LIMIT: usize = 16;
const CORPUS_PATH_COMPONENT_LIMIT: usize = 32;
const CORPUS_TIMEOUT: Duration = Duration::from_secs(10);
const SNAPSHOT_KIND: &str = "mastermind_document_evidence_graph";
const CHECK_KIND: &str = "mastermind_native_document_evidence_check";

const RELATIONS: &[&str] = &[
    "contradicts",
    "constrains",
    "documents",
    "mentions",
    "supersedes",
    "supports",
    "verified_by",
];

#[derive(Debug)]
pub struct DocumentGraphError {
    code: &'static str,
    path: Option<String>,
}

impl DocumentGraphError {
    pub(crate) fn new(code: &'static str) -> Self {
        Self { code, path: None }
    }

    fn at(code: &'static str, path: impl Into<String>) -> Self {
        Self {
            code,
            path: Some(path.into()),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

impl std::fmt::Display for DocumentGraphError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.path {
            Some(path) => write!(formatter, "{}: {path}", self.code),
            None => formatter.write_str(self.code),
        }
    }
}

impl std::error::Error for DocumentGraphError {}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentEndpoint {
    pub path: String,
    pub line: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct SavedEdge {
    id: String,
    from: DocumentEndpoint,
    relation: String,
    to: DocumentEndpoint,
    verification: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct FileRecord {
    path: String,
    sha256: String,
    bytes: u64,
    lines: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentRevision {
    pub head: String,
    pub dirty: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct SavedCorpus {
    directories: Vec<String>,
    files: Vec<FileRecord>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotV1 {
    schema_version: u32,
    kind: String,
    root: String,
    revision: DocumentRevision,
    files: Vec<FileRecord>,
    edges: Vec<SavedEdge>,
    sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotV2 {
    schema_version: u32,
    kind: String,
    root: String,
    revision: DocumentRevision,
    files: Vec<FileRecord>,
    edges: Vec<SavedEdge>,
    corpus: SavedCorpus,
    sha256: String,
}

#[derive(Debug, Clone)]
enum Snapshot {
    V1(SnapshotV1),
    V2(SnapshotV2),
}

impl Snapshot {
    fn version(&self) -> u32 {
        match self {
            Self::V1(value) => value.schema_version,
            Self::V2(value) => value.schema_version,
        }
    }

    fn root(&self) -> &str {
        match self {
            Self::V1(value) => &value.root,
            Self::V2(value) => &value.root,
        }
    }

    fn revision(&self) -> &DocumentRevision {
        match self {
            Self::V1(value) => &value.revision,
            Self::V2(value) => &value.revision,
        }
    }

    fn files(&self) -> &[FileRecord] {
        match self {
            Self::V1(value) => &value.files,
            Self::V2(value) => &value.files,
        }
    }

    fn edges(&self) -> &[SavedEdge] {
        match self {
            Self::V1(value) => &value.edges,
            Self::V2(value) => &value.edges,
        }
    }

    fn corpus(&self) -> Option<&SavedCorpus> {
        match self {
            Self::V1(_) => None,
            Self::V2(value) => Some(&value.corpus),
        }
    }

    fn sha256(&self) -> &str {
        match self {
            Self::V1(value) => &value.sha256,
            Self::V2(value) => &value.sha256,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DocumentGraphPacket {
    pub path: String,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
    pub snapshot_sha256: String,
    pub snapshot_schema_version: u32,
}

#[derive(Debug, Serialize)]
pub struct DocumentGraphChange {
    pub path: String,
    pub reasons: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct DocumentCorpusCheck {
    pub status: &'static str,
    pub directories: Vec<String>,
    pub changed_files: Vec<DocumentGraphChange>,
}

#[derive(Debug, Serialize)]
pub struct DocumentEdgeCheck {
    pub id: String,
    pub from: DocumentEndpoint,
    pub relation: String,
    pub to: DocumentEndpoint,
    pub verification: &'static str,
    pub freshness: &'static str,
}

#[derive(Debug, Serialize)]
pub struct DocumentGraphLimits {
    pub graph_bytes: u64,
    pub relations: u32,
    pub endpoint_files: u32,
    pub file_bytes: u64,
    pub union_bytes_per_pass: u64,
    pub corpus_directories: u32,
    pub corpus_files: u32,
    pub corpus_entry_visits: u32,
    pub corpus_depth: u32,
    pub corpus_path_components: u32,
    pub corpus_capture_millis: u64,
}

#[derive(Debug, Serialize)]
pub struct DocumentGraphCheck {
    pub schema_version: u32,
    pub kind: &'static str,
    pub status: &'static str,
    pub root: String,
    pub packet: DocumentGraphPacket,
    pub snapshot_revision: DocumentRevision,
    pub changed_files: Vec<DocumentGraphChange>,
    pub corpus: DocumentCorpusCheck,
    pub edges: Vec<DocumentEdgeCheck>,
    pub limits: DocumentGraphLimits,
}

#[derive(Clone, PartialEq, Eq)]
struct CurrentFile {
    record: Option<FileRecord>,
    reason: Option<&'static str>,
    identity: Option<StableFileIdentity>,
    consumed_bytes: u64,
}

#[derive(Clone, Debug)]
struct CorpusInventory {
    files: BTreeMap<String, StableFileIdentity>,
    directories: BTreeMap<String, StableFileIdentity>,
}

fn digest_bytes(bytes: &[u8]) -> String {
    crate::hex::encode(&Sha256::digest(bytes))
}

fn canonical_digest(value: &Value) -> Result<String, DocumentGraphError> {
    serde_json::to_vec(value)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|_| DocumentGraphError::new("invalid_schema"))
}

fn lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn full_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn path_parts(value: &str) -> Result<Vec<&str>, DocumentGraphError> {
    if value.is_empty()
        || value == "."
        || value.starts_with('/')
        || value.contains('\\')
        || value.len() > 4096
        || value
            .chars()
            .any(|character| character <= '\u{1f}' || character == '\u{7f}')
    {
        return Err(DocumentGraphError::at("unsafe_path", value));
    }
    let parts = value.split('/').collect::<Vec<_>>();
    if parts
        .iter()
        .any(|part| part.is_empty() || matches!(*part, "." | ".."))
    {
        return Err(DocumentGraphError::at("unsafe_path", value));
    }
    for (index, part) in parts.iter().enumerate() {
        if !part.starts_with('.') {
            continue;
        }
        let allowed = index == 0
            && parts.len() >= 3
            && ((*part == ".mastermind"
                && matches!(parts[1], "tasks" | "releases" | "decisions" | "research"))
                || (*part == ".github" && matches!(parts[1], "workflows" | "actions")));
        if !allowed {
            return Err(DocumentGraphError::at("unsafe_path", value));
        }
    }
    Ok(parts)
}

fn validate_directory(value: &str) -> Result<(), DocumentGraphError> {
    path_parts(&format!("{value}/_corpus.md"))?;
    if value.split('/').count() >= CORPUS_PATH_COMPONENT_LIMIT {
        return Err(DocumentGraphError::at("corpus_depth_limit", value));
    }
    Ok(())
}

fn suffix(value: &str) -> String {
    value
        .rsplit_once('.')
        .map(|(_, extension)| format!(".{extension}").to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_markdown(value: &str) -> bool {
    matches!(suffix(value).as_str(), ".md" | ".markdown")
}

fn supported_endpoint(value: &str) -> bool {
    matches!(
        suffix(value).as_str(),
        ".md"
            | ".markdown"
            | ".py"
            | ".pyi"
            | ".rs"
            | ".ts"
            | ".tsx"
            | ".js"
            | ".jsx"
            | ".mjs"
            | ".cjs"
            | ".vue"
            | ".go"
            | ".java"
            | ".cs"
            | ".c"
            | ".h"
            | ".cc"
            | ".cpp"
            | ".cxx"
            | ".hpp"
            | ".hh"
            | ".hxx"
            | ".php"
            | ".sh"
            | ".bash"
            | ".zsh"
            | ".fish"
            | ".swift"
            | ".kt"
            | ".kts"
            | ".rb"
            | ".ex"
            | ".exs"
            | ".scala"
            | ".sql"
            | ".yaml"
            | ".yml"
            | ".toml"
            | ".json"
            | ".jsonl"
            | ".log"
            | ".txt"
            | ".xml"
            | ".html"
            | ".css"
            | ".scss"
            | ".ini"
            | ".cfg"
            | ".proto"
            | ".graphql"
            | ".dockerfile"
    ) || matches!(
        value
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "dockerfile" | "makefile" | "justfile"
    )
}

fn validate_endpoint(value: &DocumentEndpoint, markdown: bool) -> Result<(), DocumentGraphError> {
    path_parts(&value.path)?;
    if value.line == 0 || value.line > FILE_BYTES + 1 {
        return Err(DocumentGraphError::at("invalid_line", &value.path));
    }
    if (markdown && !is_markdown(&value.path)) || (!markdown && !supported_endpoint(&value.path)) {
        return Err(DocumentGraphError::at("unsupported_endpoint", &value.path));
    }
    Ok(())
}

fn validate_file_record(value: &FileRecord) -> Result<(), DocumentGraphError> {
    path_parts(&value.path)?;
    if !lowercase_sha256(&value.sha256) {
        return Err(DocumentGraphError::at("invalid_file_digest", &value.path));
    }
    if value.bytes > FILE_BYTES || value.lines > value.bytes {
        return Err(DocumentGraphError::at(
            "invalid_file_inventory",
            &value.path,
        ));
    }
    Ok(())
}

fn validate_snapshot(snapshot: &Snapshot, canonical_root: &str) -> Result<(), DocumentGraphError> {
    if snapshot.root() != canonical_root {
        return Err(DocumentGraphError::new("root_binding_mismatch"));
    }
    if snapshot.version() == 0
        || snapshot.version() > 2
        || !full_revision(&snapshot.revision().head)
    {
        return Err(DocumentGraphError::new("invalid_revision"));
    }
    let (kind, declared_version) = match snapshot {
        Snapshot::V1(value) => (&value.kind, 1),
        Snapshot::V2(value) => (&value.kind, 2),
    };
    if kind != SNAPSHOT_KIND || snapshot.version() != declared_version {
        return Err(DocumentGraphError::new("invalid_schema"));
    }
    if snapshot.edges().is_empty() || snapshot.edges().len() > RELATION_LIMIT {
        return Err(DocumentGraphError::new("relation_limit"));
    }
    let mut endpoint_paths = BTreeSet::new();
    let mut previous_edge = None::<&str>;
    for edge in snapshot.edges() {
        validate_endpoint(&edge.from, true)?;
        validate_endpoint(&edge.to, false)?;
        if !RELATIONS.contains(&edge.relation.as_str()) {
            return Err(DocumentGraphError::new("invalid_relation"));
        }
        if edge.verification != "unverified" {
            return Err(DocumentGraphError::new("invalid_verification"));
        }
        let identity = canonical_digest(&json!({
            "from": &edge.from,
            "relation": &edge.relation,
            "to": &edge.to,
        }))?;
        if !lowercase_sha256(&edge.id)
            || edge.id != identity
            || previous_edge.is_some_and(|previous| previous >= edge.id.as_str())
        {
            return Err(DocumentGraphError::new("invalid_edge_identity_or_order"));
        }
        previous_edge = Some(&edge.id);
        endpoint_paths.insert(edge.from.path.clone());
        endpoint_paths.insert(edge.to.path.clone());
    }
    if endpoint_paths.len() > ENDPOINT_FILE_LIMIT || snapshot.files().len() != endpoint_paths.len()
    {
        return Err(DocumentGraphError::new("invalid_file_inventory"));
    }
    let mut endpoint_files = BTreeMap::new();
    let mut endpoint_total = 0_u64;
    for record in snapshot.files() {
        validate_file_record(record)?;
        endpoint_total = endpoint_total
            .checked_add(record.bytes)
            .ok_or_else(|| DocumentGraphError::new("total_byte_limit"))?;
        if endpoint_files.insert(record.path.clone(), record).is_some() {
            return Err(DocumentGraphError::new("invalid_file_inventory"));
        }
    }
    if endpoint_total > TOTAL_BYTES
        || endpoint_files.keys().cloned().collect::<BTreeSet<_>>() != endpoint_paths
        || endpoint_files.keys().cloned().collect::<Vec<_>>()
            != snapshot
                .files()
                .iter()
                .map(|record| record.path.clone())
                .collect::<Vec<_>>()
    {
        return Err(DocumentGraphError::new("invalid_file_inventory"));
    }
    for edge in snapshot.edges() {
        for endpoint in [&edge.from, &edge.to] {
            if endpoint.line > endpoint_files[&endpoint.path].lines {
                return Err(DocumentGraphError::at("line_out_of_range", &endpoint.path));
            }
        }
    }
    let Some(corpus) = snapshot.corpus() else {
        return Ok(());
    };
    if corpus.directories.len() > CORPUS_DIRECTORY_LIMIT {
        return Err(DocumentGraphError::new("corpus_directory_limit"));
    }
    if corpus.directories.is_empty() {
        return Err(DocumentGraphError::new("invalid_corpus_directories"));
    }
    let mut previous_directory = None::<&str>;
    for directory in &corpus.directories {
        validate_directory(directory)?;
        if previous_directory.is_some_and(|previous| {
            previous >= directory.as_str() || directory.starts_with(&format!("{previous}/"))
        }) {
            return Err(DocumentGraphError::at("corpus_scope_overlap", directory));
        }
        previous_directory = Some(directory);
    }
    if corpus.files.len() > CORPUS_FILE_LIMIT {
        return Err(DocumentGraphError::new("corpus_file_limit"));
    }
    let mut corpus_files = BTreeMap::new();
    for record in &corpus.files {
        validate_file_record(record)?;
        let Some(directory) = corpus
            .directories
            .iter()
            .find(|directory| record.path.starts_with(&format!("{directory}/")))
        else {
            return Err(DocumentGraphError::at(
                "invalid_corpus_inventory",
                &record.path,
            ));
        };
        let relative_components = record.path[directory.len() + 1..].split('/').count();
        if !is_markdown(&record.path)
            || record.path.split('/').count() > CORPUS_PATH_COMPONENT_LIMIT
            || relative_components.saturating_sub(1) > CORPUS_DEPTH_LIMIT
            || corpus_files.insert(record.path.clone(), record).is_some()
        {
            return Err(DocumentGraphError::at(
                "invalid_corpus_inventory",
                &record.path,
            ));
        }
    }
    if corpus_files.keys().cloned().collect::<Vec<_>>()
        != corpus
            .files
            .iter()
            .map(|record| record.path.clone())
            .collect::<Vec<_>>()
    {
        return Err(DocumentGraphError::new("invalid_corpus_inventory"));
    }
    for (path, record) in &endpoint_files {
        let tracked_markdown = is_markdown(path)
            && corpus
                .directories
                .iter()
                .any(|directory| path.starts_with(&format!("{directory}/")));
        if tracked_markdown && corpus_files.get(path).copied() != Some(*record) {
            return Err(DocumentGraphError::at("inconsistent_corpus_endpoint", path));
        }
    }
    let mut union = endpoint_files
        .iter()
        .map(|(path, record)| (path.clone(), record.bytes))
        .collect::<BTreeMap<_, _>>();
    union.extend(
        corpus_files
            .iter()
            .map(|(path, record)| (path.clone(), record.bytes)),
    );
    let union_total = union
        .values()
        .try_fold(0_u64, |sum, bytes| sum.checked_add(*bytes))
        .ok_or_else(|| DocumentGraphError::new("total_byte_limit"))?;
    if union_total > TOTAL_BYTES {
        return Err(DocumentGraphError::new("total_byte_limit"));
    }
    Ok(())
}

fn decode_snapshot(bytes: &[u8], canonical_root: &str) -> Result<Snapshot, DocumentGraphError> {
    let mut value = crate::audit_bundle::from_json_strict::<Value>(bytes)
        .map_err(|_| DocumentGraphError::new("invalid_json"))?;
    let version = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| DocumentGraphError::new("invalid_schema"))?;
    let declared_sha = value
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| DocumentGraphError::new("invalid_schema"))?
        .to_string();
    value
        .as_object_mut()
        .ok_or_else(|| DocumentGraphError::new("invalid_schema"))?
        .remove("sha256");
    if !lowercase_sha256(&declared_sha) || canonical_digest(&value)? != declared_sha {
        return Err(DocumentGraphError::new("snapshot_digest_mismatch"));
    }
    value
        .as_object_mut()
        .expect("validated object")
        .insert("sha256".into(), Value::String(declared_sha));
    let snapshot = match version {
        1 => Snapshot::V1(
            serde_json::from_value(value).map_err(|_| DocumentGraphError::new("invalid_schema"))?,
        ),
        2 => Snapshot::V2(
            serde_json::from_value(value).map_err(|_| DocumentGraphError::new("invalid_schema"))?,
        ),
        _ => return Err(DocumentGraphError::new("invalid_schema")),
    };
    validate_snapshot(&snapshot, canonical_root)?;
    Ok(snapshot)
}

fn map_read_error(error: BoundedReadError, path: &str) -> DocumentGraphError {
    match error {
        BoundedReadError::InvalidPath | BoundedReadError::OutsideRoot => {
            DocumentGraphError::at("unsafe_path", path)
        }
        BoundedReadError::NotRegular => DocumentGraphError::at("not_regular", path),
        BoundedReadError::TooLarge { .. } => DocumentGraphError::at("file_byte_limit", path),
        BoundedReadError::SnapshotChanged => DocumentGraphError::at("snapshot_changed", path),
        BoundedReadError::Interrupted => DocumentGraphError::new("interrupted"),
        BoundedReadError::DeadlineExceeded => DocumentGraphError::new("deadline_exceeded"),
        BoundedReadError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            DocumentGraphError::at("missing", path)
        }
        BoundedReadError::Io(_error) => {
            #[cfg(unix)]
            if _error.raw_os_error() == Some(libc::ELOOP) {
                return DocumentGraphError::at("unsafe_path", path);
            }
            DocumentGraphError::at("unreadable", path)
        }
    }
}

fn repository_path(
    root: &RootCapability,
    path: &Path,
) -> Result<(PathBuf, String), DocumentGraphError> {
    let rooted;
    let candidate = if path.is_absolute() {
        path
    } else {
        rooted = root.requested_root().join(path);
        &rooted
    };
    let relative = root
        .repository_relative(candidate)
        .map_err(|error| map_read_error(error, &path.to_string_lossy()))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(value) = component else {
            return Err(DocumentGraphError::new("unsafe_graph_path"));
        };
        parts.push(
            value
                .to_str()
                .ok_or_else(|| DocumentGraphError::new("unsafe_graph_path"))?,
        );
    }
    let logical = parts.join("/");
    path_parts(&logical)?;
    if parts.len() < 3 || parts[..2] != [".mastermind", "research"] {
        return Err(DocumentGraphError::at("unsafe_graph_path", logical));
    }
    Ok((relative, logical))
}

fn current_file(
    root: &RootCapability,
    path: &str,
    control: ReadControl<'_>,
) -> Result<CurrentFile, DocumentGraphError> {
    let rooted = root.requested_root().join(path);
    match read_regular_file_with_capability(root, &rooted, FILE_BYTES, FILE_BYTES, control) {
        Ok(file) => {
            let text = match std::str::from_utf8(&file.bytes) {
                Ok(text) if !text.contains('\0') => text,
                _ => {
                    return Ok(CurrentFile {
                        record: None,
                        reason: Some("unsupported_text"),
                        identity: Some(file.identity),
                        consumed_bytes: file.declared_len,
                    })
                }
            };
            let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
            let lines = normalized.matches('\n').count()
                + usize::from(!normalized.is_empty() && !normalized.ends_with('\n'));
            Ok(CurrentFile {
                record: Some(FileRecord {
                    path: path.to_string(),
                    sha256: digest_bytes(&file.bytes),
                    bytes: file.declared_len,
                    lines: lines as u64,
                }),
                reason: None,
                identity: Some(file.identity),
                consumed_bytes: file.declared_len,
            })
        }
        Err(BoundedReadError::NotRegular) => Ok(CurrentFile {
            record: None,
            reason: Some("not_regular"),
            identity: None,
            consumed_bytes: 0,
        }),
        Err(BoundedReadError::InvalidPath | BoundedReadError::OutsideRoot) => Ok(CurrentFile {
            record: None,
            reason: Some("unsafe_path"),
            identity: None,
            consumed_bytes: 0,
        }),
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(CurrentFile {
                record: None,
                reason: Some("missing"),
                identity: None,
                consumed_bytes: 0,
            })
        }
        #[cfg(unix)]
        Err(BoundedReadError::Io(error)) if error.raw_os_error() == Some(libc::ELOOP) => {
            Ok(CurrentFile {
                record: None,
                reason: Some("unsafe_path"),
                identity: None,
                consumed_bytes: 0,
            })
        }
        Err(error) => Err(map_read_error(error, path)),
    }
}

fn capture_files(
    root: &RootCapability,
    paths: &BTreeSet<String>,
    control: ReadControl<'_>,
) -> Result<BTreeMap<String, CurrentFile>, DocumentGraphError> {
    let mut total = 0_u64;
    let mut files = BTreeMap::new();
    for path in paths {
        control
            .check()
            .map_err(|error| map_read_error(error, path))?;
        let current = current_file(root, path, control)?;
        total = total
            .checked_add(current.consumed_bytes)
            .ok_or_else(|| DocumentGraphError::new("total_byte_limit"))?;
        if total > TOTAL_BYTES {
            return Err(DocumentGraphError::new("total_byte_limit"));
        }
        files.insert(path.clone(), current);
    }
    Ok(files)
}

fn same_directory_receipts(
    left: &BTreeMap<String, StableFileIdentity>,
    right: &BTreeMap<String, StableFileIdentity>,
) -> bool {
    left.len() == right.len()
        && left.iter().all(|(path, identity)| {
            right
                .get(path)
                .is_some_and(|other| identity.same_object(*other))
        })
}

fn same_corpus(left: &CorpusInventory, right: &CorpusInventory) -> bool {
    left.files == right.files && same_directory_receipts(&left.directories, &right.directories)
}

struct CorpusScanner<'a, 'b> {
    root: &'a RootCapability,
    control: ReadControl<'b>,
    visits: &'a mut usize,
    files: BTreeMap<String, StableFileIdentity>,
    directories: BTreeMap<String, StableFileIdentity>,
    seen_directories: Vec<StableFileIdentity>,
}

impl CorpusScanner<'_, '_> {
    fn visit(&mut self) -> Result<(), DocumentGraphError> {
        self.control
            .check()
            .map_err(|error| map_read_error(error, "corpus"))?;
        *self.visits = self.visits.saturating_add(1);
        if *self.visits > CORPUS_ENTRY_LIMIT {
            return Err(DocumentGraphError::new("corpus_entry_limit"));
        }
        Ok(())
    }

    fn scan(&mut self, path: &str, depth: usize) -> Result<(), DocumentGraphError> {
        self.visit()?;
        if depth > CORPUS_DEPTH_LIMIT {
            return Err(DocumentGraphError::at("corpus_depth_limit", path));
        }
        validate_directory(path)?;
        let remaining = CORPUS_ENTRY_LIMIT.saturating_sub(*self.visits);
        let listing = read_directory_receipt_with_capability(
            self.root,
            &self.root.requested_root().join(path),
            remaining,
            self.control,
        )
        .map_err(|error| match error {
            BoundedReadError::TooLarge { .. } => DocumentGraphError::new("corpus_entry_limit"),
            error => map_read_error(error, path),
        })?;
        if self
            .seen_directories
            .iter()
            .any(|identity| identity.same_object(listing.identity))
        {
            return Err(DocumentGraphError::at("corpus_scope_overlap", path));
        }
        self.seen_directories.push(listing.identity);
        self.directories.insert(path.to_string(), listing.identity);
        for name in listing.names {
            self.visit()?;
            let name = name
                .to_str()
                .ok_or_else(|| DocumentGraphError::at("unsafe_path", path))?;
            if name.starts_with('.') {
                continue;
            }
            let child = format!("{path}/{name}");
            path_parts(&child)?;
            let receipt = crate::bounded_fs::inspect_path_receipt_with_capability(
                self.root,
                &self.root.requested_root().join(&child),
                self.control,
            )
            .map_err(|error| map_read_error(error, &child))?;
            match receipt.kind {
                BoundedPathKind::Directory => self.scan(&child, depth + 1)?,
                BoundedPathKind::RegularFile if is_markdown(&child) => {
                    if child.split('/').count() > CORPUS_PATH_COMPONENT_LIMIT {
                        return Err(DocumentGraphError::at("corpus_depth_limit", child));
                    }
                    if self.files.len() >= CORPUS_FILE_LIMIT {
                        return Err(DocumentGraphError::new("corpus_file_limit"));
                    }
                    if self.files.insert(child.clone(), receipt.identity).is_some() {
                        return Err(DocumentGraphError::at("snapshot_changed", child));
                    }
                }
                BoundedPathKind::RegularFile => {}
                BoundedPathKind::Other => {
                    return Err(DocumentGraphError::at("not_regular", child));
                }
            }
        }
        Ok(())
    }
}

fn corpus_inventory(
    root: &RootCapability,
    directories: &[String],
    control: ReadControl<'_>,
    visits: &mut usize,
) -> Result<CorpusInventory, DocumentGraphError> {
    let mut scanner = CorpusScanner {
        root,
        control,
        visits,
        files: BTreeMap::new(),
        directories: BTreeMap::new(),
        seen_directories: Vec::new(),
    };
    for directory in directories {
        scanner.scan(directory, 0)?;
    }
    root.verify()
        .map_err(|error| map_read_error(error, "repository_root"))?;
    Ok(CorpusInventory {
        files: scanner.files,
        directories: scanner.directories,
    })
}

fn capture(
    root: &RootCapability,
    endpoint_paths: &BTreeSet<String>,
    directories: &[String],
    request_control: ReadControl<'_>,
) -> Result<(BTreeMap<String, CurrentFile>, Option<CorpusInventory>), DocumentGraphError> {
    if directories.is_empty() {
        let first = capture_files(root, endpoint_paths, request_control)?;
        let second = capture_files(root, endpoint_paths, request_control)?;
        if first != second {
            return Err(DocumentGraphError::new("snapshot_changed"));
        }
        return Ok((second, None));
    }
    let corpus_deadline = Instant::now() + CORPUS_TIMEOUT;
    let deadline = request_control
        .deadline
        .map(|deadline| deadline.min(corpus_deadline))
        .or(Some(corpus_deadline));
    let control = ReadControl {
        deadline,
        interrupted: request_control.interrupted,
    };
    let mut visits = 0_usize;
    let mut previous = None;
    for _ in 0..2 {
        let before = corpus_inventory(root, directories, control, &mut visits)?;
        let paths = endpoint_paths
            .iter()
            .cloned()
            .chain(before.files.keys().cloned())
            .collect::<BTreeSet<_>>();
        let files = capture_files(root, &paths, control)?;
        let after = corpus_inventory(root, directories, control, &mut visits)?;
        if !same_corpus(&before, &after) {
            return Err(DocumentGraphError::new("snapshot_changed"));
        }
        for (path, identity) in &after.files {
            let Some(current) = files.get(path) else {
                return Err(DocumentGraphError::at("snapshot_changed", path));
            };
            if let Some(reason) = current.reason {
                return Err(DocumentGraphError::at(reason, path));
            }
            if current.identity != Some(*identity) {
                return Err(DocumentGraphError::at("snapshot_changed", path));
            }
        }
        if let Some((previous_files, previous_inventory)) = &previous {
            if previous_files != &files || !same_corpus(previous_inventory, &after) {
                return Err(DocumentGraphError::new("snapshot_changed"));
            }
        }
        previous = Some((files, after));
    }
    let (files, inventory) = previous.expect("two capture passes");
    Ok((files, Some(inventory)))
}

pub(crate) fn check(
    root: &Path,
    graph_path: &Path,
    control: ReadControl<'_>,
) -> Result<DocumentGraphCheck, DocumentGraphError> {
    let capability =
        RootCapability::open(root).map_err(|error| map_read_error(error, "repository_root"))?;
    let canonical_root = capability
        .canonical_root()
        .to_str()
        .ok_or_else(|| DocumentGraphError::new("invalid_root"))?
        .to_string();
    let (relative_graph, logical_graph) = repository_path(&capability, graph_path)?;
    let rooted_graph = capability.requested_root().join(&relative_graph);
    let graph = read_regular_file_with_capability(
        &capability,
        &rooted_graph,
        GRAPH_BYTES,
        GRAPH_BYTES,
        control,
    )
    .map_err(|error| map_read_error(error, &logical_graph))?;
    let snapshot = decode_snapshot(&graph.bytes, &canonical_root)?;
    let endpoint_paths = snapshot
        .files()
        .iter()
        .map(|record| record.path.clone())
        .collect::<BTreeSet<_>>();
    let directories = snapshot
        .corpus()
        .map(|corpus| corpus.directories.as_slice())
        .unwrap_or_default();
    let (current, inventory) = capture(&capability, &endpoint_paths, directories, control)?;
    let graph_after = read_regular_file_expected(
        &capability,
        &rooted_graph,
        GRAPH_BYTES,
        GRAPH_BYTES,
        control,
        Some(graph.identity),
    )
    .map_err(|error| map_read_error(error, &logical_graph))?;
    if graph.bytes != graph_after.bytes {
        return Err(DocumentGraphError::at("snapshot_changed", logical_graph));
    }
    capability
        .verify()
        .map_err(|error| map_read_error(error, "repository_root"))?;

    let expected = snapshot
        .files()
        .iter()
        .map(|record| (record.path.clone(), record))
        .collect::<BTreeMap<_, _>>();
    let mut changed = BTreeMap::<String, BTreeSet<&'static str>>::new();
    for path in &endpoint_paths {
        let item = &current[path];
        if let Some(reason) = item.reason {
            changed.entry(path.clone()).or_default().insert(reason);
        } else if item.record.as_ref() != expected.get(path).copied() {
            changed
                .entry(path.clone())
                .or_default()
                .insert("content_changed");
        }
    }
    for edge in snapshot.edges() {
        for endpoint in [&edge.from, &edge.to] {
            if current[&endpoint.path]
                .record
                .as_ref()
                .is_some_and(|record| endpoint.line > record.lines)
            {
                changed
                    .entry(endpoint.path.clone())
                    .or_default()
                    .insert("line_out_of_range");
            }
        }
    }
    let changed_files = changed
        .iter()
        .map(|(path, reasons)| DocumentGraphChange {
            path: path.clone(),
            reasons: reasons.iter().copied().collect(),
        })
        .collect::<Vec<_>>();

    let corpus = match (snapshot.corpus(), inventory) {
        (None, None) => DocumentCorpusCheck {
            status: "not_tracked",
            directories: Vec::new(),
            changed_files: Vec::new(),
        },
        (Some(saved), Some(live)) => {
            let old = saved
                .files
                .iter()
                .map(|record| (record.path.clone(), record))
                .collect::<BTreeMap<_, _>>();
            let paths = old
                .keys()
                .cloned()
                .chain(live.files.keys().cloned())
                .collect::<BTreeSet<_>>();
            let mut changes = Vec::new();
            for path in paths {
                let reason = if !old.contains_key(&path) {
                    Some("added")
                } else if !live.files.contains_key(&path) {
                    Some("missing")
                } else if current[&path].record.as_ref() != old.get(&path).copied() {
                    Some("content_changed")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    changes.push(DocumentGraphChange {
                        path,
                        reasons: vec![reason],
                    });
                }
            }
            DocumentCorpusCheck {
                status: if changes.is_empty() {
                    "current"
                } else {
                    "changed"
                },
                directories: saved.directories.clone(),
                changed_files: changes,
            }
        }
        _ => return Err(DocumentGraphError::new("snapshot_changed")),
    };
    let edges = snapshot
        .edges()
        .iter()
        .map(|edge| DocumentEdgeCheck {
            id: edge.id.clone(),
            from: edge.from.clone(),
            relation: edge.relation.clone(),
            to: edge.to.clone(),
            verification: "unverified",
            freshness: if changed.contains_key(&edge.from.path)
                || changed.contains_key(&edge.to.path)
            {
                "needs_review"
            } else {
                "current"
            },
        })
        .collect::<Vec<_>>();
    let status = if changed_files.is_empty() && corpus.status != "changed" {
        "current"
    } else {
        "needs_review"
    };
    Ok(DocumentGraphCheck {
        schema_version: 1,
        kind: CHECK_KIND,
        status,
        root: canonical_root,
        packet: DocumentGraphPacket {
            path: logical_graph,
            artifact_sha256: digest_bytes(&graph.bytes),
            artifact_bytes: graph.declared_len,
            snapshot_sha256: snapshot.sha256().to_string(),
            snapshot_schema_version: snapshot.version(),
        },
        snapshot_revision: snapshot.revision().clone(),
        changed_files,
        corpus,
        edges,
        limits: DocumentGraphLimits {
            graph_bytes: GRAPH_BYTES,
            relations: RELATION_LIMIT as u32,
            endpoint_files: ENDPOINT_FILE_LIMIT as u32,
            file_bytes: FILE_BYTES,
            union_bytes_per_pass: TOTAL_BYTES,
            corpus_directories: CORPUS_DIRECTORY_LIMIT as u32,
            corpus_files: CORPUS_FILE_LIMIT as u32,
            corpus_entry_visits: CORPUS_ENTRY_LIMIT as u32,
            corpus_depth: CORPUS_DEPTH_LIMIT as u32,
            corpus_path_components: CORPUS_PATH_COMPONENT_LIMIT as u32,
            corpus_capture_millis: CORPUS_TIMEOUT.as_millis() as u64,
        },
    })
}

pub fn check_for_store(
    store: &crate::store::Store,
    graph_path: &Path,
) -> Result<DocumentGraphCheck, DocumentGraphError> {
    let root = store
        .meta_value("index_root")
        .map_err(|_| DocumentGraphError::new("index_root_unavailable"))?
        .ok_or_else(|| DocumentGraphError::new("index_root_unavailable"))?;
    let interrupted = || store.work_interrupted();
    check(
        Path::new(&root),
        graph_path,
        ReadControl {
            deadline: store.request_deadline(),
            interrupted: Some(&interrupted),
        },
    )
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::fs;

    pub(crate) const GRAPH_PATH: &str = ".mastermind/research/document-graph.json";

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let destination = root.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(destination, bytes).unwrap();
    }

    fn record(root: &Path, path: &str) -> FileRecord {
        let bytes = fs::read(root.join(path)).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let lines = normalized.matches('\n').count()
            + usize::from(!normalized.is_empty() && !normalized.ends_with('\n'));
        FileRecord {
            path: path.to_string(),
            sha256: digest_bytes(&bytes),
            bytes: bytes.len() as u64,
            lines: lines as u64,
        }
    }

    fn edge() -> SavedEdge {
        let from = DocumentEndpoint {
            path: "docs/adr/0001.md".into(),
            line: 1,
        };
        let to = DocumentEndpoint {
            path: "src/handler.rs".into(),
            line: 2,
        };
        let relation = "documents".to_string();
        let id = canonical_digest(&json!({
            "from": &from,
            "relation": &relation,
            "to": &to,
        }))
        .unwrap();
        SavedEdge {
            id,
            from,
            relation,
            to,
            verification: "unverified".into(),
        }
    }

    pub(crate) fn write_snapshot(root: &Path, track_corpus: bool) -> PathBuf {
        write(root, "docs/adr/0001.md", b"# Decision\nUse handler.\n");
        write(root, "src/handler.rs", b"fn handler() {\n}\n");
        let canonical_root = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let mut files = vec![
            record(root, "docs/adr/0001.md"),
            record(root, "src/handler.rs"),
        ];
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let mut value = json!({
            "schema_version": if track_corpus { 2 } else { 1 },
            "kind": SNAPSHOT_KIND,
            "root": canonical_root,
            "revision": { "head": "a".repeat(40), "dirty": false },
            "files": files,
            "edges": [edge()],
        });
        if track_corpus {
            value.as_object_mut().unwrap().insert(
                "corpus".into(),
                serde_json::to_value(SavedCorpus {
                    directories: vec!["docs/adr".into()],
                    files: vec![record(root, "docs/adr/0001.md")],
                })
                .unwrap(),
            );
        }
        repair_digest(&mut value);
        write(
            root,
            GRAPH_PATH,
            &[serde_json::to_vec(&value).unwrap(), vec![b'\n']].concat(),
        );
        PathBuf::from(GRAPH_PATH)
    }

    pub(crate) fn read_value(root: &Path) -> Value {
        serde_json::from_slice(&fs::read(root.join(GRAPH_PATH)).unwrap()).unwrap()
    }

    pub(crate) fn repair_digest(value: &mut Value) {
        value.as_object_mut().unwrap().remove("sha256");
        let sha256 = canonical_digest(value).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("sha256".into(), Value::String(sha256));
    }

    pub(crate) fn write_value(root: &Path, value: &Value) {
        write(
            root,
            GRAPH_PATH,
            &[serde_json::to_vec(value).unwrap(), vec![b'\n']].concat(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{self, GRAPH_PATH};
    use super::*;

    fn fixture(track_corpus: bool) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let graph = test_support::write_snapshot(root.path(), track_corpus);
        (root, graph)
    }

    fn error_code(root: &Path, graph: &Path, control: ReadControl<'_>) -> &'static str {
        check(root, graph, control).unwrap_err().code()
    }

    #[test]
    fn canonical_digest_matches_portable_python_unicode_contract() {
        let value = json!({"z": "é😀", "a": {"β": 1, "line": 2}});
        assert_eq!(
            canonical_digest(&value).unwrap(),
            "ae418a65fe6684aa3a8f400dba46280a9324e9675615a188fa57873c081394ed"
        );
    }

    #[test]
    fn v1_check_is_current_without_inventing_corpus_or_verification() {
        let (root, graph) = fixture(false);
        let response = check(root.path(), &graph, ReadControl::default()).unwrap();
        assert_eq!(response.status, "current");
        assert_eq!(response.corpus.status, "not_tracked");
        assert!(response.corpus.directories.is_empty());
        assert!(response.changed_files.is_empty());
        assert_eq!(response.edges.len(), 1);
        assert_eq!(response.edges[0].verification, "unverified");
        assert_eq!(response.edges[0].freshness, "current");
        assert_eq!(response.packet.path, GRAPH_PATH);
        assert_eq!(response.packet.snapshot_schema_version, 1);
        assert_ne!(
            response.packet.artifact_sha256, response.packet.snapshot_sha256,
            "the packet digest covers the trailing newline as well as the snapshot"
        );
    }

    #[test]
    fn v2_corpus_addition_is_independent_of_endpoint_freshness() {
        let (root, graph) = fixture(true);
        std::fs::write(root.path().join("docs/adr/0002.md"), "# Later\n").unwrap();
        let response = check(root.path(), &graph, ReadControl::default()).unwrap();
        assert_eq!(response.status, "needs_review");
        assert!(response.changed_files.is_empty());
        assert_eq!(response.corpus.status, "changed");
        assert_eq!(response.corpus.changed_files.len(), 1);
        assert_eq!(response.corpus.changed_files[0].path, "docs/adr/0002.md");
        assert_eq!(response.corpus.changed_files[0].reasons, vec!["added"]);
        assert_eq!(response.edges[0].freshness, "current");
        assert_eq!(response.edges[0].verification, "unverified");
    }

    #[test]
    fn endpoint_content_and_line_drift_mark_only_affected_edges() {
        let (root, graph) = fixture(false);
        std::fs::write(root.path().join("src/handler.rs"), "fn handler() {}\n").unwrap();
        let response = check(root.path(), &graph, ReadControl::default()).unwrap();
        assert_eq!(response.status, "needs_review");
        assert_eq!(response.changed_files.len(), 1);
        assert_eq!(response.changed_files[0].path, "src/handler.rs");
        assert_eq!(
            response.changed_files[0].reasons,
            vec!["content_changed", "line_out_of_range"]
        );
        assert_eq!(response.edges[0].freshness, "needs_review");
        assert_eq!(response.edges[0].verification, "unverified");
    }

    #[test]
    fn forged_packets_fail_before_any_freshness_claim() {
        type PacketMutation = (Box<dyn Fn(&mut Value)>, bool, &'static str);
        let cases: Vec<PacketMutation> = vec![
            (
                Box::new(|value| value["root"] = Value::String("/outside".into())),
                true,
                "root_binding_mismatch",
            ),
            (
                Box::new(|value| value["edges"][0]["verification"] = json!("verified")),
                true,
                "invalid_verification",
            ),
            (
                Box::new(|value| value["authority"] = json!("approved")),
                true,
                "invalid_schema",
            ),
            (
                Box::new(|value| value["sha256"] = Value::String("0".repeat(64))),
                false,
                "snapshot_digest_mismatch",
            ),
        ];
        for (mutate, repair, expected) in cases {
            let (root, graph) = fixture(false);
            let mut value = test_support::read_value(root.path());
            mutate(&mut value);
            if repair {
                test_support::repair_digest(&mut value);
            }
            test_support::write_value(root.path(), &value);
            assert_eq!(
                error_code(root.path(), &graph, ReadControl::default()),
                expected
            );
        }
    }

    #[test]
    fn duplicate_json_and_graph_paths_outside_research_are_rejected() {
        let (root, _graph) = fixture(false);
        std::fs::write(
            root.path().join(GRAPH_PATH),
            br#"{"schema_version":1,"schema_version":1}"#,
        )
        .unwrap();
        assert_eq!(
            error_code(root.path(), Path::new(GRAPH_PATH), ReadControl::default()),
            "invalid_json"
        );
        assert_eq!(
            error_code(
                root.path(),
                Path::new("document-graph.json"),
                ReadControl::default()
            ),
            "unsafe_graph_path"
        );
        let outside = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(
            error_code(root.path(), outside.path(), ReadControl::default()),
            "unsafe_path"
        );
    }

    #[test]
    fn graph_byte_limit_and_request_control_fail_closed() {
        let (root, graph) = fixture(false);
        let interrupted = || true;
        assert_eq!(
            error_code(
                root.path(),
                &graph,
                ReadControl {
                    deadline: None,
                    interrupted: Some(&interrupted),
                },
            ),
            "interrupted"
        );
        assert_eq!(
            error_code(
                root.path(),
                &graph,
                ReadControl {
                    deadline: Some(Instant::now()),
                    interrupted: None,
                },
            ),
            "deadline_exceeded"
        );
        std::fs::write(
            root.path().join(GRAPH_PATH),
            vec![b' '; GRAPH_BYTES as usize + 1],
        )
        .unwrap();
        assert_eq!(
            error_code(root.path(), &graph, ReadControl::default()),
            "file_byte_limit"
        );
    }

    #[test]
    fn unsupported_endpoint_bytes_still_count_toward_the_union_limit() {
        let root = tempfile::tempdir().unwrap();
        let mut records = Vec::new();
        let mut edges = Vec::new();
        let bytes = vec![0xff; FILE_BYTES as usize];
        for index in 0..17 {
            let path = format!("docs/endpoint-{index:02}.md");
            let endpoint = DocumentEndpoint {
                path: path.clone(),
                line: 1,
            };
            std::fs::create_dir_all(root.path().join("docs")).unwrap();
            std::fs::write(root.path().join(&path), &bytes).unwrap();
            records.push(FileRecord {
                path,
                sha256: "0".repeat(64),
                bytes: 1,
                lines: 1,
            });
            let relation = "mentions".to_string();
            edges.push(SavedEdge {
                id: canonical_digest(&json!({
                    "from": &endpoint,
                    "relation": &relation,
                    "to": &endpoint,
                }))
                .unwrap(),
                from: endpoint.clone(),
                relation,
                to: endpoint,
                verification: "unverified".into(),
            });
        }
        records.sort_by(|left, right| left.path.cmp(&right.path));
        edges.sort_by(|left, right| left.id.cmp(&right.id));
        let mut value = json!({
            "schema_version": 1,
            "kind": SNAPSHOT_KIND,
            "root": root.path().canonicalize().unwrap().to_string_lossy(),
            "revision": { "head": "b".repeat(40), "dirty": false },
            "files": records,
            "edges": edges,
        });
        test_support::repair_digest(&mut value);
        test_support::write_value(root.path(), &value);
        assert_eq!(
            error_code(root.path(), Path::new(GRAPH_PATH), ReadControl::default()),
            "total_byte_limit"
        );
    }

    #[test]
    fn unsupported_text_in_tracked_corpus_is_an_error() {
        let (root, graph) = fixture(true);
        std::fs::write(root.path().join("docs/adr/binary.md"), [0xff, 0xfe]).unwrap();
        assert_eq!(
            error_code(root.path(), &graph, ReadControl::default()),
            "unsupported_text"
        );
    }

    #[test]
    fn directory_enumeration_reports_the_corpus_entry_limit() {
        let (root, _graph) = fixture(true);
        let capability = RootCapability::open(root.path()).unwrap();
        let mut visits = CORPUS_ENTRY_LIMIT - 1;
        let error = corpus_inventory(
            &capability,
            &["docs/adr".into()],
            ReadControl::default(),
            &mut visits,
        )
        .unwrap_err();
        assert_eq!(error.code(), "corpus_entry_limit");
    }

    #[test]
    fn relative_paths_ignore_nested_process_cwd() {
        const CHILD_ROOT: &str = "MMCG_DOCUMENT_GRAPH_CWD_TEST_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = PathBuf::from(root);
            for graph in [PathBuf::from(GRAPH_PATH), root.join(GRAPH_PATH)] {
                let response = check(&root, &graph, ReadControl::default()).unwrap();
                assert_eq!(response.status, "needs_review");
                assert_eq!(response.changed_files.len(), 1);
                assert_eq!(response.changed_files[0].path, "src/handler.rs");
            }
            std::fs::write(
                root.join(".mastermind/research/nested-cwd-child-ran"),
                b"ok",
            )
            .unwrap();
            return;
        }

        let (root, graph) = fixture(false);
        for path in ["docs/adr/0001.md", "src/handler.rs"] {
            let shadow = root.path().join("nested").join(path);
            std::fs::create_dir_all(shadow.parent().unwrap()).unwrap();
            std::fs::copy(root.path().join(path), shadow).unwrap();
        }
        std::fs::write(root.path().join("src/handler.rs"), "fn changed() {}\n").unwrap();
        assert_eq!(graph, PathBuf::from(GRAPH_PATH));
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "document_graph::tests::relative_paths_ignore_nested_process_cwd",
                "--nocapture",
            ])
            .current_dir(root.path().join("nested"))
            .env(CHILD_ROOT, root.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "nested-cwd child failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(root
            .path()
            .join(".mastermind/research/nested-cwd-child-ran")
            .is_file());
    }

    #[cfg(unix)]
    #[test]
    fn endpoint_symlink_is_reported_as_drift_without_reading_its_target() {
        use std::os::unix::fs::symlink;

        let (root, graph) = fixture(false);
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::remove_file(root.path().join("src/handler.rs")).unwrap();
        symlink(outside.path(), root.path().join("src/handler.rs")).unwrap();
        let response = check(root.path(), &graph, ReadControl::default()).unwrap();
        assert_eq!(response.status, "needs_review");
        assert_eq!(response.changed_files.len(), 1);
        assert_eq!(response.changed_files[0].path, "src/handler.rs");
        assert!(matches!(
            response.changed_files[0].reasons.as_slice(),
            ["unsafe_path"] | ["not_regular"]
        ));
        assert_eq!(response.edges[0].freshness, "needs_review");
    }
}
