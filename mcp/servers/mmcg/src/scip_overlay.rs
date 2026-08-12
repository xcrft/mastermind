//! Optional compiler-resolved semantic evidence imported from a SCIP index.
//!
//! The Tree-sitter graph remains the always-available topology. SCIP facts are
//! stored in separate additive tables and surfaced with explicit provenance;
//! they never rewrite syntactic symbols or edges.

use crate::store::Store;
use protobuf::CodedInputStream;
use scip::types::{occurrence, Document, Metadata, Occurrence, SymbolInformation, TextEncoding};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_SCIP_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_DOCUMENTS: usize = 500_000;
const MAX_OCCURRENCES: usize = 10_000_000;
const MAX_SYMBOL_INFORMATION: usize = 2_000_000;
const MAX_DEFINITIONS: usize = 2_000_000;
const MAX_EDGES: usize = 5_000_000;
const MAX_SYMBOL_BYTES: usize = 16 * 1024;
const MAX_DISPLAY_BYTES: usize = 4 * 1024;
const MAX_PATH_BYTES: usize = 4 * 1024;
pub const MAX_LENS_SEMANTIC_EDGES: usize = 2_000;

#[derive(Debug)]
pub enum ScipOverlayError {
    InvalidIndex(String),
    InvalidQuery(String),
    Io(String),
    Store(String),
}

impl fmt::Display for ScipOverlayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIndex(message) => write!(formatter, "invalid SCIP index: {message}"),
            Self::InvalidQuery(message) => write!(formatter, "invalid semantic query: {message}"),
            Self::Io(message) => write!(formatter, "SCIP I/O error: {message}"),
            Self::Store(message) => write!(formatter, "SCIP store error: {message}"),
        }
    }
}

impl std::error::Error for ScipOverlayError {}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticSource {
    pub format: &'static str,
    pub tool_name: String,
    pub tool_version: String,
    pub project_root: String,
    pub artifact_path: String,
    pub artifact_sha256: String,
    pub imported_at: i64,
    pub documents: u32,
    pub definitions: u32,
    pub edges: u32,
    pub text_verified_documents: u32,
    pub repository_verified: bool,
    pub revision_verified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticDefinition {
    pub symbol: String,
    pub display_name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
    pub provenance: &'static str,
    pub confidence: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticEdge {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_display_name: Option<String>,
    pub from_file: String,
    pub from_line: u32,
    pub from_character: u32,
    pub occurrence_line: u32,
    pub occurrence_character: u32,
    pub to_symbol: String,
    pub to_display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_character: Option<u32>,
    pub kind: String,
    pub provenance: &'static str,
    pub confidence: &'static str,
}

#[derive(Debug, Serialize)]
pub struct SemanticCollection<T> {
    pub total: Option<u32>,
    pub returned: u32,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<&'static str>,
    pub items: Vec<T>,
}

#[derive(Debug, Serialize)]
pub struct SemanticResolution {
    pub default_graph: &'static str,
    pub static_precedence: [&'static str; 2],
    pub runtime_confidence: &'static str,
    pub fallback_without_scip: &'static str,
}

#[derive(Debug, Serialize)]
pub struct SemanticDiagnostic {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct SemanticOverlaySnapshot {
    pub schema_version: u32,
    pub available: bool,
    pub partial: bool,
    pub fallback_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SemanticSource>,
    pub definitions: SemanticCollection<SemanticDefinition>,
    pub edges: SemanticCollection<SemanticEdge>,
    pub diagnostics: Vec<SemanticDiagnostic>,
    pub resolution: SemanticResolution,
}

#[derive(Debug, Serialize)]
pub struct ImportSummary {
    pub schema_version: u32,
    pub replaced_previous_overlay: bool,
    pub source: SemanticSource,
    pub resolution: SemanticResolution,
}

#[derive(Debug, Clone)]
pub(crate) struct SemanticSourceInput {
    pub tool_name: String,
    pub tool_version: String,
    pub project_root: String,
    pub artifact_path: String,
    pub artifact_sha256: String,
    pub imported_at: i64,
    pub documents: u32,
    pub definitions: u32,
    pub edges: u32,
    pub text_verified_documents: u32,
    pub repository_verified: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct SemanticDocumentInput {
    pub path: String,
    pub language: String,
    pub position_encoding: String,
    pub content_sha256: String,
    pub source_text_verified: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct SemanticDefinitionInput {
    pub symbol: String,
    pub display_name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct SemanticEdgeInput {
    pub from_symbol: Option<String>,
    pub from_display_name: Option<String>,
    pub from_file: String,
    pub from_line: u32,
    pub from_character: u32,
    pub occurrence_line: u32,
    pub occurrence_character: u32,
    pub to_symbol: String,
    pub to_display_name: String,
    pub to_file: Option<String>,
    pub to_line: Option<u32>,
    pub to_character: Option<u32>,
    pub kind: String,
}

#[derive(Debug)]
pub(crate) struct SemanticImportBatch {
    pub source: SemanticSourceInput,
    pub documents: Vec<SemanticDocumentInput>,
    pub definitions: Vec<SemanticDefinitionInput>,
    pub edges: Vec<SemanticEdgeInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SourceRange {
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
}

impl SourceRange {
    fn contains(self, other: Self) -> bool {
        (self.start_line, self.start_character) <= (other.start_line, other.start_character)
            && (self.end_line, self.end_character) >= (other.end_line, other.end_character)
    }
}

#[derive(Debug, Clone)]
struct DefinitionLocator {
    definition_index: usize,
    body: SourceRange,
}

#[derive(Debug, Clone)]
struct InfoRecord {
    symbol: String,
    display_name: String,
    kind: String,
}

type SemanticEdgeKey = [u8; 32];

fn resolution() -> SemanticResolution {
    SemanticResolution {
        default_graph: "tree-sitter",
        static_precedence: ["scip", "tree-sitter"],
        runtime_confidence: "observed",
        fallback_without_scip: "tree-sitter",
    }
}

fn checked_count(value: usize, maximum: usize, label: &str) -> Result<u32, ScipOverlayError> {
    if value > maximum {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "{label} count {value} exceeds the {maximum} safety limit"
        )));
    }
    u32::try_from(value)
        .map_err(|_| ScipOverlayError::InvalidIndex(format!("{label} count is too large")))
}

fn validate_bounded(value: &str, maximum: usize, label: &str) -> Result<(), ScipOverlayError> {
    if value.len() > maximum {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "{label} exceeds {maximum} bytes"
        )));
    }
    if value.contains('\0') {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "{label} contains a NUL byte"
        )));
    }
    Ok(())
}

fn normalize_document_path(path: &str) -> Result<String, ScipOverlayError> {
    validate_bounded(path, MAX_PATH_BYTES, "document path")?;
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "document path must be a canonical repository-relative slash path: {path:?}"
        )));
    }
    let mut normalized = Vec::new();
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(ScipOverlayError::InvalidIndex(format!(
                "document path is not canonical: {path:?}"
            )));
        }
        normalized.push(component);
    }
    Ok(normalized.join("/"))
}

fn qualify_symbol(document: Option<&str>, symbol: &str) -> Result<String, ScipOverlayError> {
    validate_bounded(symbol, MAX_SYMBOL_BYTES, "symbol")?;
    if symbol.is_empty() {
        return Err(ScipOverlayError::InvalidIndex(
            "an indexed semantic fact has an empty symbol".into(),
        ));
    }
    if scip::symbol::is_local_symbol(symbol) {
        let document = document.ok_or_else(|| {
            ScipOverlayError::InvalidIndex("an external symbol cannot be local".into())
        })?;
        Ok(format!("local {document}::{}", &symbol[6..]))
    } else {
        Ok(symbol.to_string())
    }
}

fn fallback_display_name(symbol: &str) -> String {
    if let Some(local) = symbol.strip_prefix("local ") {
        return local.rsplit("::").next().unwrap_or(local).to_string();
    }
    scip::symbol::parse_symbol(symbol)
        .ok()
        .and_then(|parsed| parsed.descriptors.last().map(|value| value.name.clone()))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| symbol.to_string())
}

fn info_record(
    info: &SymbolInformation,
    document: Option<&str>,
) -> Result<InfoRecord, ScipOverlayError> {
    let symbol = qualify_symbol(document, &info.symbol)?;
    let display_name = if info.display_name.is_empty() {
        fallback_display_name(&symbol)
    } else {
        validate_bounded(&info.display_name, MAX_DISPLAY_BYTES, "symbol display name")?;
        info.display_name.clone()
    };
    let kind = info
        .kind
        .enum_value()
        .map(|value| format!("{value:?}"))
        .unwrap_or_else(|_| format!("Unknown({})", info.kind.value()));
    Ok(InfoRecord {
        symbol,
        display_name,
        kind,
    })
}

fn insert_info(infos: &mut HashMap<String, InfoRecord>, record: InfoRecord) {
    infos.entry(record.symbol.clone()).or_insert(record);
}

fn is_definition_occurrence(occurrence: &Occurrence) -> bool {
    let roles = scip::types::SymbolRole::Definition as i32
        | scip::types::SymbolRole::ForwardDefinition as i32;
    occurrence.symbol_roles & roles != 0
}

fn range_from_values(values: &[i32], label: &str) -> Result<Option<SourceRange>, ScipOverlayError> {
    if values.is_empty() {
        return Ok(None);
    }
    let (start_line, start_character, end_line, end_character) = match values {
        [line, start, end] => (*line, *start, *line, *end),
        [start_line, start, end_line, end] => (*start_line, *start, *end_line, *end),
        _ => {
            return Err(ScipOverlayError::InvalidIndex(format!(
                "{label} must contain exactly three or four integers"
            )))
        }
    };
    range_from_i32(start_line, start_character, end_line, end_character, label).map(Some)
}

fn range_from_i32(
    start_line: i32,
    start_character: i32,
    end_line: i32,
    end_character: i32,
    label: &str,
) -> Result<SourceRange, ScipOverlayError> {
    let values = [start_line, start_character, end_line, end_character];
    if values.iter().any(|value| *value < 0) {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "{label} contains a negative position"
        )));
    }
    let range = SourceRange {
        start_line: start_line as u32,
        start_character: start_character as u32,
        end_line: end_line as u32,
        end_character: end_character as u32,
    };
    if (range.end_line, range.end_character) < (range.start_line, range.start_character) {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "{label} ends before it starts"
        )));
    }
    Ok(range)
}

fn occurrence_range(occurrence: &Occurrence) -> Result<SourceRange, ScipOverlayError> {
    match &occurrence.typed_range {
        Some(occurrence::Typed_range::SingleLineRange(value)) => range_from_i32(
            value.line,
            value.start_character,
            value.line,
            value.end_character,
            "occurrence range",
        ),
        Some(occurrence::Typed_range::MultiLineRange(value)) => range_from_i32(
            value.start_line,
            value.start_character,
            value.end_line,
            value.end_character,
            "occurrence range",
        ),
        None => range_from_values(&occurrence.range, "occurrence range")?.ok_or_else(|| {
            ScipOverlayError::InvalidIndex("a symbol occurrence has no source range".into())
        }),
        Some(_) => Err(ScipOverlayError::InvalidIndex(
            "occurrence uses an unsupported typed range variant".into(),
        )),
    }
}

fn enclosing_range(occurrence: &Occurrence) -> Result<Option<SourceRange>, ScipOverlayError> {
    match &occurrence.typed_enclosing_range {
        Some(occurrence::Typed_enclosing_range::SingleLineEnclosingRange(value)) => range_from_i32(
            value.line,
            value.start_character,
            value.line,
            value.end_character,
            "enclosing range",
        )
        .map(Some),
        Some(occurrence::Typed_enclosing_range::MultiLineEnclosingRange(value)) => range_from_i32(
            value.start_line,
            value.start_character,
            value.end_line,
            value.end_character,
            "enclosing range",
        )
        .map(Some),
        None => range_from_values(&occurrence.enclosing_range, "enclosing range"),
        Some(_) => Err(ScipOverlayError::InvalidIndex(
            "occurrence uses an unsupported typed enclosing-range variant".into(),
        )),
    }
}

fn hash_file(path: &Path) -> Result<(String, u64), ScipOverlayError> {
    let metadata = path.metadata().map_err(|error| {
        ScipOverlayError::Io(format!("read {} metadata: {error}", path.display()))
    })?;
    if !metadata.is_file() {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "document is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_DOCUMENT_BYTES {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "document {} exceeds the {} MiB safety limit",
            path.display(),
            MAX_DOCUMENT_BYTES / 1024 / 1024
        )));
    }
    let file = File::open(path)
        .map_err(|error| ScipOverlayError::Io(format!("open {}: {error}", path.display())))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| ScipOverlayError::Io(format!("read {}: {error}", path.display())))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > MAX_DOCUMENT_BYTES {
            return Err(ScipOverlayError::InvalidIndex(format!(
                "document {} grew beyond the {} MiB safety limit while being read",
                path.display(),
                MAX_DOCUMENT_BYTES / 1024 / 1024
            )));
        }
        hasher.update(&buffer[..read]);
    }
    Ok((crate::hex::encode(&hasher.finalize()), total))
}

fn embedded_text_matches_file(
    path: &Path,
    embedded: &str,
    encoding: TextEncoding,
) -> Result<bool, ScipOverlayError> {
    let metadata = path.metadata().map_err(|error| {
        ScipOverlayError::Io(format!("read {} metadata: {error}", path.display()))
    })?;
    if metadata.len() > MAX_DOCUMENT_BYTES {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "document {} exceeds the {} MiB safety limit",
            path.display(),
            MAX_DOCUMENT_BYTES / 1024 / 1024
        )));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    File::open(path)
        .and_then(|file| {
            file.take(MAX_DOCUMENT_BYTES.saturating_add(1))
                .read_to_end(&mut bytes)
        })
        .map_err(|error| ScipOverlayError::Io(format!("read {}: {error}", path.display())))?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "document {} grew beyond the {} MiB safety limit while being read",
            path.display(),
            MAX_DOCUMENT_BYTES / 1024 / 1024
        )));
    }
    let decoded = match encoding {
        TextEncoding::UTF16 => {
            let (little_endian, body) = match bytes.as_slice() {
                [0xff, 0xfe, rest @ ..] => (true, rest),
                [0xfe, 0xff, rest @ ..] => (false, rest),
                _ => {
                    return Err(ScipOverlayError::InvalidIndex(format!(
                        "UTF-16 SCIP document {} has no byte-order mark",
                        path.display()
                    )))
                }
            };
            if body.len() % 2 != 0 {
                return Err(ScipOverlayError::InvalidIndex(format!(
                    "UTF-16 SCIP document {} has an odd byte length",
                    path.display()
                )));
            }
            let units = body
                .chunks_exact(2)
                .map(|pair| {
                    if little_endian {
                        u16::from_le_bytes([pair[0], pair[1]])
                    } else {
                        u16::from_be_bytes([pair[0], pair[1]])
                    }
                })
                .collect::<Vec<_>>();
            String::from_utf16(&units).map_err(|_| {
                ScipOverlayError::InvalidIndex(format!(
                    "UTF-16 SCIP document {} is malformed",
                    path.display()
                ))
            })?
        }
        TextEncoding::UTF8 | TextEncoding::UnspecifiedTextEncoding => {
            let decoded = std::str::from_utf8(&bytes).map_err(|_| {
                ScipOverlayError::InvalidIndex(format!(
                    "SCIP document {} is not valid UTF-8",
                    path.display()
                ))
            })?;
            decoded
                .strip_prefix('\u{feff}')
                .unwrap_or(decoded)
                .to_string()
        }
    };
    Ok(decoded == embedded)
}

fn source_for_store(source: &SemanticSourceInput) -> SemanticSource {
    SemanticSource {
        format: "scip",
        tool_name: source.tool_name.clone(),
        tool_version: source.tool_version.clone(),
        project_root: source.project_root.clone(),
        artifact_path: source.artifact_path.clone(),
        artifact_sha256: source.artifact_sha256.clone(),
        imported_at: source.imported_at,
        documents: source.documents,
        definitions: source.definitions,
        edges: source.edges,
        text_verified_documents: source.text_verified_documents,
        repository_verified: source.repository_verified,
        revision_verified: source.documents > 0
            && source.text_verified_documents == source.documents,
    }
}

fn artifact_metadata(path: &Path) -> Result<std::fs::Metadata, ScipOverlayError> {
    let metadata = path.metadata().map_err(|error| {
        ScipOverlayError::Io(format!("read {} metadata: {error}", path.display()))
    })?;
    if !metadata.is_file() {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "artifact is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_SCIP_BYTES {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "artifact exceeds the {} MiB safety limit",
            MAX_SCIP_BYTES / 1024 / 1024
        )));
    }
    Ok(metadata)
}

fn hash_artifact(path: &Path) -> Result<String, ScipOverlayError> {
    artifact_metadata(path)?;
    let file = File::open(path)
        .map_err(|error| ScipOverlayError::Io(format!("open {}: {error}", path.display())))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| ScipOverlayError::Io(format!("read {}: {error}", path.display())))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > MAX_SCIP_BYTES {
            return Err(ScipOverlayError::InvalidIndex(format!(
                "artifact grew beyond the {} MiB safety limit while being read",
                MAX_SCIP_BYTES / 1024 / 1024
            )));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(crate::hex::encode(&hasher.finalize()))
}

enum ScipRecord {
    Metadata(Metadata),
    Document(Document),
    ExternalSymbol(SymbolInformation),
}

/// Decode one top-level protobuf field at a time. SCIP indexes are commonly
/// produced for monorepos and the schema explicitly permits streaming; keeping
/// the full `Index` plus every document body in memory would multiply the
/// artifact's peak footprint before any useful evidence reached SQLite.
fn stream_scip(
    path: &Path,
    mut visit: impl FnMut(ScipRecord) -> Result<(), ScipOverlayError>,
) -> Result<(), ScipOverlayError> {
    artifact_metadata(path)?;
    let file = File::open(path)
        .map_err(|error| ScipOverlayError::Io(format!("open {}: {error}", path.display())))?;
    let mut reader = BufReader::new(file.take(MAX_SCIP_BYTES.saturating_add(1)));
    let mut input = CodedInputStream::from_buf_read(&mut reader);
    let mut metadata_seen = false;
    loop {
        let tag = input.read_raw_tag_or_eof().map_err(|error| {
            ScipOverlayError::InvalidIndex(format!("protobuf decode failed: {error}"))
        })?;
        let Some(tag) = tag else {
            break;
        };
        let record = match tag {
            10 => {
                if metadata_seen {
                    return Err(ScipOverlayError::InvalidIndex(
                        "the SCIP index contains duplicate metadata".into(),
                    ));
                }
                metadata_seen = true;
                Some(ScipRecord::Metadata(input.read_message().map_err(
                    |error| {
                        ScipOverlayError::InvalidIndex(format!(
                            "decode SCIP metadata failed: {error}"
                        ))
                    },
                )?))
            }
            18 if !metadata_seen => {
                return Err(ScipOverlayError::InvalidIndex(
                    "SCIP metadata must appear before documents for streaming consumption".into(),
                ))
            }
            18 => Some(ScipRecord::Document(input.read_message().map_err(
                |error| {
                    ScipOverlayError::InvalidIndex(format!("decode SCIP document failed: {error}"))
                },
            )?)),
            26 if !metadata_seen => {
                return Err(ScipOverlayError::InvalidIndex(
                    "SCIP metadata must appear before external symbols for streaming consumption"
                        .into(),
                ))
            }
            26 => Some(ScipRecord::ExternalSymbol(input.read_message().map_err(
                |error| {
                    ScipOverlayError::InvalidIndex(format!(
                        "decode external SCIP symbol failed: {error}"
                    ))
                },
            )?)),
            _ => {
                protobuf::rt::skip_field_for_tag(tag, &mut input).map_err(|error| {
                    ScipOverlayError::InvalidIndex(format!(
                        "skip unknown SCIP field failed: {error}"
                    ))
                })?;
                None
            }
        };
        if let Some(record) = record {
            visit(record)?;
        }
    }
    if input.pos() > MAX_SCIP_BYTES {
        return Err(ScipOverlayError::InvalidIndex(format!(
            "artifact grew beyond the {} MiB safety limit while being decoded",
            MAX_SCIP_BYTES / 1024 / 1024
        )));
    }
    Ok(())
}

fn decode_percent_encoded(value: &str) -> Result<String, ScipOverlayError> {
    fn nibble(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            b'A'..=b'F' => Some(value - b'A' + 10),
            _ => None,
        }
    }

    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let high = bytes.get(index + 1).and_then(|value| nibble(*value));
        let low = bytes.get(index + 2).and_then(|value| nibble(*value));
        let (Some(high), Some(low)) = (high, low) else {
            return Err(ScipOverlayError::InvalidIndex(
                "SCIP project_root contains malformed percent encoding".into(),
            ));
        };
        decoded.push((high << 4) | low);
        index += 3;
    }
    let decoded = String::from_utf8(decoded).map_err(|_| {
        ScipOverlayError::InvalidIndex("SCIP project_root is not valid UTF-8".into())
    })?;
    if decoded.contains('\0') {
        return Err(ScipOverlayError::InvalidIndex(
            "SCIP project_root contains a NUL byte".into(),
        ));
    }
    Ok(decoded)
}

fn project_root_path(value: &str) -> Result<Option<PathBuf>, ScipOverlayError> {
    if value.is_empty() {
        return Ok(None);
    }
    if value.contains(['?', '#']) {
        return Err(ScipOverlayError::InvalidIndex(
            "SCIP project_root must not contain a query or fragment".into(),
        ));
    }
    let decoded = if let Some(rest) = value.strip_prefix("file://") {
        let local = if let Some(path) = rest.strip_prefix("localhost/") {
            format!("/{path}")
        } else if rest.starts_with('/') {
            rest.to_string()
        } else {
            return Err(ScipOverlayError::InvalidIndex(
                "SCIP project_root must be a local file URI".into(),
            ));
        };
        decode_percent_encoded(&local)?
    } else if let Some(rest) = value.strip_prefix("file:") {
        decode_percent_encoded(rest)?
    } else {
        decode_percent_encoded(value)?
    };

    #[cfg(windows)]
    let decoded = decoded
        .strip_prefix('/')
        .filter(|path| path.as_bytes().get(1) == Some(&b':'))
        .unwrap_or(&decoded)
        .to_string();

    let path = PathBuf::from(decoded);
    if path.is_absolute() {
        Ok(Some(path))
    } else {
        Err(ScipOverlayError::InvalidIndex(
            "SCIP project_root must be an absolute local path".into(),
        ))
    }
}

fn verify_repository_identity(
    project_root: &str,
    indexed_root: &Path,
    all_documents_text_verified: bool,
) -> Result<bool, ScipOverlayError> {
    let reported = project_root_path(project_root)?;
    if let Some(reported) = reported {
        match reported.canonicalize() {
            Ok(reported) if reported == indexed_root => return Ok(true),
            Ok(reported) if !all_documents_text_verified => {
                return Err(ScipOverlayError::InvalidIndex(format!(
                    "SCIP project_root {} does not match the indexed repository {}; regenerate the artifact here or embed every Document.text",
                    reported.display(),
                    indexed_root.display()
                )));
            }
            Err(error) if !all_documents_text_verified => {
                return Err(ScipOverlayError::InvalidIndex(format!(
                    "SCIP project_root cannot be verified ({error}); regenerate the artifact here or embed every Document.text"
                )));
            }
            _ => {}
        }
    } else if !all_documents_text_verified {
        return Err(ScipOverlayError::InvalidIndex(
            "SCIP metadata has no project_root and not every document embeds matching text".into(),
        ));
    }
    // Exact embedded text binds a portable artifact to this repository even
    // when its producer used a container-only or since-moved project root.
    Ok(all_documents_text_verified)
}

fn artifact_label(artifact: &Path, root: &Path) -> String {
    artifact
        .strip_prefix(root)
        .ok()
        .and_then(|relative| relative.to_str())
        .filter(|relative| !relative.is_empty())
        .map(|relative| relative.replace('\\', "/"))
        .or_else(|| {
            artifact
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "index.scip".into())
}

fn definition_for_symbol<'a>(
    definitions: &'a HashMap<String, Vec<DefinitionLocator>>,
    symbol: &str,
) -> Option<&'a DefinitionLocator> {
    definitions.get(symbol).and_then(|values| values.first())
}

fn owner_for_reference(
    definitions: &[DefinitionLocator],
    range: SourceRange,
) -> Option<&DefinitionLocator> {
    definitions
        .iter()
        .filter(|definition| definition.body.contains(range))
        .max_by(|left, right| {
            (left.body.start_line, left.body.start_character)
                .cmp(&(right.body.start_line, right.body.start_character))
                .then_with(|| {
                    (right.body.end_line, right.body.end_character)
                        .cmp(&(left.body.end_line, left.body.end_character))
                })
        })
}

fn edge_key(edge: &SemanticEdgeInput) -> SemanticEdgeKey {
    fn string(hasher: &mut Sha256, value: Option<&str>) {
        match value {
            Some(value) => {
                hasher.update([1]);
                hasher.update((value.len() as u64).to_le_bytes());
                hasher.update(value.as_bytes());
            }
            None => hasher.update([0]),
        }
    }

    let mut hasher = Sha256::new();
    string(&mut hasher, edge.from_symbol.as_deref());
    string(&mut hasher, Some(&edge.from_file));
    for value in [
        edge.from_line,
        edge.from_character,
        edge.occurrence_line,
        edge.occurrence_character,
    ] {
        hasher.update(value.to_le_bytes());
    }
    string(&mut hasher, Some(&edge.to_symbol));
    string(&mut hasher, edge.to_file.as_deref());
    string(&mut hasher, Some(&edge.kind));
    hasher.finalize().into()
}

fn push_edge(
    edges: &mut Vec<SemanticEdgeInput>,
    seen_edges: &mut HashSet<SemanticEdgeKey>,
    edge: SemanticEdgeInput,
) -> Result<(), ScipOverlayError> {
    if seen_edges.insert(edge_key(&edge)) {
        edges.push(edge);
        checked_count(edges.len(), MAX_EDGES, "semantic edge")?;
    }
    Ok(())
}

fn append_relationship_edges(
    info: &SymbolInformation,
    document: Option<&str>,
    definitions: &[SemanticDefinitionInput],
    locators_by_symbol: &HashMap<String, Vec<DefinitionLocator>>,
    infos: &HashMap<String, InfoRecord>,
    edges: &mut Vec<SemanticEdgeInput>,
    seen_edges: &mut HashSet<SemanticEdgeKey>,
) -> Result<(), ScipOverlayError> {
    let source_symbol = qualify_symbol(document, &info.symbol)?;
    let Some(source_locator) = definition_for_symbol(locators_by_symbol, &source_symbol) else {
        return Ok(());
    };
    let source = &definitions[source_locator.definition_index];
    for relationship in &info.relationships {
        let target_symbol = qualify_symbol(document, &relationship.symbol)?;
        let target = definition_for_symbol(locators_by_symbol, &target_symbol)
            .map(|locator| &definitions[locator.definition_index]);
        let target_display = infos
            .get(&target_symbol)
            .map(|value| value.display_name.clone())
            .unwrap_or_else(|| fallback_display_name(&target_symbol));
        for (enabled, kind) in [
            (relationship.is_reference, "reference"),
            (relationship.is_implementation, "implementation"),
            (relationship.is_type_definition, "type_definition"),
            (relationship.is_definition, "definition"),
        ] {
            if !enabled {
                continue;
            }
            push_edge(
                edges,
                seen_edges,
                SemanticEdgeInput {
                    from_symbol: Some(source.symbol.clone()),
                    from_display_name: Some(source.display_name.clone()),
                    from_file: source.file.clone(),
                    from_line: source.line,
                    from_character: source.character,
                    occurrence_line: source.line,
                    occurrence_character: source.character,
                    to_symbol: target_symbol.clone(),
                    to_display_name: target_display.clone(),
                    to_file: target.map(|value| value.file.clone()),
                    to_line: target.map(|value| value.line),
                    to_character: target.map(|value| value.character),
                    kind: kind.into(),
                },
            )?;
        }
    }
    Ok(())
}

fn build_batch(store: &Store, scip_path: &Path) -> Result<SemanticImportBatch, ScipOverlayError> {
    if !store
        .schema_current()
        .map_err(|error| ScipOverlayError::Store(error.to_string()))?
    {
        return Err(ScipOverlayError::Store(
            "the codegraph schema is stale; run `mastermind index .` first".into(),
        ));
    }
    let root = store
        .meta_value("index_root")
        .map_err(|error| ScipOverlayError::Store(error.to_string()))?
        .ok_or_else(|| {
            ScipOverlayError::Store(
                "the index has no repository identity; run `mastermind index .` first".into(),
            )
        })?;
    let root = PathBuf::from(root)
        .canonicalize()
        .map_err(|error| ScipOverlayError::Store(format!("resolve index root: {error}")))?;
    crate::indexer::validate_index_root(store, &root).map_err(ScipOverlayError::Store)?;

    let artifact = scip_path.canonicalize().map_err(|error| {
        ScipOverlayError::Io(format!("resolve {}: {error}", scip_path.display()))
    })?;
    let artifact_sha256 = hash_artifact(&artifact)?;

    let mut scip_metadata = None::<Metadata>;
    let mut documents = Vec::new();
    let mut document_paths = HashSet::new();
    let mut total_source_bytes = 0_u64;
    let mut text_verified_documents = 0_usize;
    let mut occurrence_count = 0_usize;
    let mut symbol_information_count = 0_usize;
    let mut infos = HashMap::<String, InfoRecord>::new();
    let mut locators_by_document = HashMap::<String, Vec<DefinitionLocator>>::new();
    let mut locators_by_symbol = HashMap::<String, Vec<DefinitionLocator>>::new();
    let mut definitions = Vec::new();

    // Pass one validates source identity and retains only compact definition
    // and relationship records. Each potentially large Document is released
    // before the next top-level protobuf field is decoded.
    stream_scip(&artifact, |record| {
        match record {
            ScipRecord::Metadata(value) => scip_metadata = Some(value),
            ScipRecord::ExternalSymbol(info) => {
                symbol_information_count = symbol_information_count.saturating_add(1);
                checked_count(
                    symbol_information_count,
                    MAX_SYMBOL_INFORMATION,
                    "symbol information",
                )?;
                let record = info_record(&info, None)?;
                insert_info(&mut infos, record);
            }
            ScipRecord::Document(document) => {
                checked_count(documents.len().saturating_add(1), MAX_DOCUMENTS, "document")?;
                occurrence_count = occurrence_count.saturating_add(document.occurrences.len());
                checked_count(occurrence_count, MAX_OCCURRENCES, "occurrence")?;
                symbol_information_count =
                    symbol_information_count.saturating_add(document.symbols.len());
                checked_count(
                    symbol_information_count,
                    MAX_SYMBOL_INFORMATION,
                    "symbol information",
                )?;

                let relative = normalize_document_path(&document.relative_path)?;
                if !document_paths.insert(relative.clone()) {
                    return Err(ScipOverlayError::InvalidIndex(format!(
                        "duplicate document path: {relative}"
                    )));
                }
                validate_bounded(&document.language, 256, "document language")?;
                let resolved = root.join(&relative).canonicalize().map_err(|error| {
                    ScipOverlayError::InvalidIndex(format!(
                        "document {relative:?} is unavailable under the indexed repository: {error}"
                    ))
                })?;
                if !resolved.starts_with(&root) {
                    return Err(ScipOverlayError::InvalidIndex(format!(
                        "document escapes the indexed repository through a symlink: {relative:?}"
                    )));
                }
                let (content_sha256, bytes) = hash_file(&resolved)?;
                total_source_bytes = total_source_bytes.saturating_add(bytes);
                if total_source_bytes > MAX_SOURCE_BYTES {
                    return Err(ScipOverlayError::InvalidIndex(format!(
                        "documents exceed the {} GiB source verification limit",
                        MAX_SOURCE_BYTES / 1024 / 1024 / 1024
                    )));
                }
                let source_text_verified = if document.text.is_empty() {
                    false
                } else {
                    let encoding = scip_metadata
                        .as_ref()
                        .expect("stream_scip guarantees metadata before documents")
                        .text_document_encoding
                        .enum_value()
                        .map_err(|_| {
                            ScipOverlayError::InvalidIndex(
                                "SCIP metadata uses an unknown document text encoding".into(),
                            )
                        })?;
                    if !embedded_text_matches_file(&resolved, &document.text, encoding)? {
                        return Err(ScipOverlayError::InvalidIndex(format!(
                            "embedded SCIP text does not match the current repository file {relative:?}"
                        )));
                    }
                    if hash_file(&resolved)?.0 != content_sha256 {
                        return Err(ScipOverlayError::InvalidIndex(format!(
                            "repository file changed while SCIP evidence was being verified: {relative:?}"
                        )));
                    }
                    text_verified_documents += 1;
                    true
                };
                let position_encoding = document
                    .position_encoding
                    .enum_value()
                    .map(|value| format!("{value:?}"))
                    .unwrap_or_else(|_| format!("Unknown({})", document.position_encoding.value()));
                documents.push(SemanticDocumentInput {
                    path: relative.clone(),
                    language: document.language.clone(),
                    position_encoding,
                    content_sha256,
                    source_text_verified,
                });

                for info in &document.symbols {
                    let record = info_record(info, Some(&relative))?;
                    insert_info(&mut infos, record);
                }
                for occurrence in &document.occurrences {
                    if occurrence.symbol.is_empty() || !is_definition_occurrence(occurrence) {
                        continue;
                    }
                    let symbol = qualify_symbol(Some(&relative), &occurrence.symbol)?;
                    let range = occurrence_range(occurrence)?;
                    let body = enclosing_range(occurrence)?.unwrap_or(range);
                    if !body.contains(range) {
                        return Err(ScipOverlayError::InvalidIndex(format!(
                            "definition enclosing range does not enclose its occurrence in {relative:?}"
                        )));
                    }
                    let info = infos.get(&symbol);
                    let definition = SemanticDefinitionInput {
                        symbol: symbol.clone(),
                        display_name: info
                            .map(|value| value.display_name.clone())
                            .unwrap_or_else(|| fallback_display_name(&symbol)),
                        kind: info
                            .map(|value| value.kind.clone())
                            .unwrap_or_else(|| "UnspecifiedKind".into()),
                        file: relative.clone(),
                        line: range.start_line.saturating_add(1),
                        character: range.start_character.saturating_add(1),
                        end_line: range.end_line.saturating_add(1),
                        end_character: range.end_character.saturating_add(1),
                    };
                    let definition_index = definitions.len();
                    let locator = DefinitionLocator {
                        definition_index,
                        body,
                    };
                    definitions.push(definition);
                    locators_by_document
                        .entry(relative.clone())
                        .or_default()
                        .push(locator.clone());
                    locators_by_symbol.entry(symbol).or_default().push(locator);
                    checked_count(definitions.len(), MAX_DEFINITIONS, "definition")?;
                }
            }
        }
        Ok(())
    })?;

    let metadata = scip_metadata
        .as_ref()
        .ok_or_else(|| ScipOverlayError::InvalidIndex("the SCIP index has no metadata".into()))?;
    let tool = metadata.tool_info.as_ref();
    let tool_name = tool.map(|value| value.name.clone()).unwrap_or_default();
    let tool_version = tool.map(|value| value.version.clone()).unwrap_or_default();
    let project_root = metadata.project_root.clone();
    validate_bounded(&tool_name, 256, "tool name")?;
    validate_bounded(&tool_version, 256, "tool version")?;
    validate_bounded(&project_root, MAX_PATH_BYTES * 2, "project root")?;
    let all_documents_text_verified =
        !documents.is_empty() && text_verified_documents == documents.len();
    let repository_verified =
        verify_repository_identity(&project_root, &root, all_documents_text_verified)?;

    for values in locators_by_symbol.values_mut() {
        values.sort_by(|left, right| {
            definitions[left.definition_index]
                .file
                .cmp(&definitions[right.definition_index].file)
                .then_with(|| {
                    definitions[left.definition_index]
                        .line
                        .cmp(&definitions[right.definition_index].line)
                })
                .then_with(|| {
                    definitions[left.definition_index]
                        .character
                        .cmp(&definitions[right.definition_index].character)
                })
        });
    }

    let mut edges = Vec::new();
    let mut seen_edges = HashSet::new();
    let mut second_pass_paths = HashSet::with_capacity(document_paths.len());
    let mut second_pass_occurrences = 0_usize;
    // Pass two resolves references now that every global definition and symbol
    // relationship is known. It still holds only one Document at a time.
    stream_scip(&artifact, |record| {
        let ScipRecord::Document(document) = record else {
            return Ok(());
        };
        let relative = normalize_document_path(&document.relative_path)?;
        if !document_paths.contains(&relative) || !second_pass_paths.insert(relative.clone()) {
            return Err(ScipOverlayError::InvalidIndex(
                "SCIP artifact changed while it was being imported".into(),
            ));
        }
        second_pass_occurrences =
            second_pass_occurrences.saturating_add(document.occurrences.len());
        checked_count(second_pass_occurrences, MAX_OCCURRENCES, "occurrence")?;
        let document_definitions = locators_by_document
            .get(&relative)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for occurrence in &document.occurrences {
            if occurrence.symbol.is_empty() || is_definition_occurrence(occurrence) {
                continue;
            }
            let symbol = qualify_symbol(Some(&relative), &occurrence.symbol)?;
            let range = occurrence_range(occurrence)?;
            let owner = owner_for_reference(document_definitions, range)
                .map(|locator| &definitions[locator.definition_index]);
            let target = definition_for_symbol(&locators_by_symbol, &symbol)
                .map(|locator| &definitions[locator.definition_index]);
            let display = infos
                .get(&symbol)
                .map(|value| value.display_name.clone())
                .unwrap_or_else(|| fallback_display_name(&symbol));
            let kind = if occurrence.symbol_roles & scip::types::SymbolRole::Import as i32 != 0 {
                "import"
            } else {
                "reference"
            };
            let edge = SemanticEdgeInput {
                from_symbol: owner.map(|value| value.symbol.clone()),
                from_display_name: owner.map(|value| value.display_name.clone()),
                from_file: relative.clone(),
                from_line: owner
                    .map(|value| value.line)
                    .unwrap_or_else(|| range.start_line.saturating_add(1)),
                from_character: owner
                    .map(|value| value.character)
                    .unwrap_or_else(|| range.start_character.saturating_add(1)),
                occurrence_line: range.start_line.saturating_add(1),
                occurrence_character: range.start_character.saturating_add(1),
                to_symbol: symbol,
                to_display_name: display,
                to_file: target.map(|value| value.file.clone()),
                to_line: target.map(|value| value.line),
                to_character: target.map(|value| value.character),
                kind: kind.into(),
            };
            push_edge(&mut edges, &mut seen_edges, edge)?;
        }
        for info in &document.symbols {
            append_relationship_edges(
                info,
                Some(&relative),
                &definitions,
                &locators_by_symbol,
                &infos,
                &mut edges,
                &mut seen_edges,
            )?;
        }
        Ok(())
    })?;
    if second_pass_paths != document_paths || second_pass_occurrences != occurrence_count {
        return Err(ScipOverlayError::InvalidIndex(
            "SCIP artifact changed while it was being imported".into(),
        ));
    }

    // External symbol information can also carry relationships. A third
    // streaming pass avoids retaining those relationship vectors in memory.
    stream_scip(&artifact, |record| {
        let ScipRecord::ExternalSymbol(info) = record else {
            return Ok(());
        };
        append_relationship_edges(
            &info,
            None,
            &definitions,
            &locators_by_symbol,
            &infos,
            &mut edges,
            &mut seen_edges,
        )
    })?;

    if hash_artifact(&artifact)? != artifact_sha256 {
        return Err(ScipOverlayError::InvalidIndex(
            "SCIP artifact changed while it was being imported".into(),
        ));
    }

    let source = SemanticSourceInput {
        tool_name,
        tool_version,
        project_root: ".".into(),
        artifact_path: artifact_label(&artifact, &root),
        artifact_sha256,
        imported_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .try_into()
            .unwrap_or(i64::MAX),
        documents: checked_count(documents.len(), MAX_DOCUMENTS, "document")?,
        definitions: checked_count(definitions.len(), MAX_DEFINITIONS, "definition")?,
        edges: checked_count(edges.len(), MAX_EDGES, "semantic edge")?,
        text_verified_documents: checked_count(
            text_verified_documents,
            MAX_DOCUMENTS,
            "text-verified document",
        )?,
        repository_verified,
    };
    Ok(SemanticImportBatch {
        source,
        documents,
        definitions,
        edges,
    })
}

pub fn import(store: &Store, scip_path: &Path) -> Result<ImportSummary, ScipOverlayError> {
    let batch = build_batch(store, scip_path)?;
    let previous = store
        .semantic_source()
        .map_err(|error| ScipOverlayError::Store(error.to_string()))?
        .is_some();
    let source = source_for_store(&batch.source);
    store
        .replace_semantic_overlay(&batch)
        .map_err(|error| ScipOverlayError::Store(error.to_string()))?;
    Ok(ImportSummary {
        schema_version: 1,
        replaced_previous_overlay: previous,
        source,
        resolution: resolution(),
    })
}

fn empty_snapshot() -> SemanticOverlaySnapshot {
    SemanticOverlaySnapshot {
        schema_version: 1,
        available: false,
        partial: false,
        fallback_active: true,
        source: None,
        definitions: SemanticCollection {
            total: Some(0),
            returned: 0,
            truncated: false,
            truncation_reason: None,
            items: Vec::new(),
        },
        edges: SemanticCollection {
            total: Some(0),
            returned: 0,
            truncated: false,
            truncation_reason: None,
            items: Vec::new(),
        },
        diagnostics: Vec::new(),
        resolution: resolution(),
    }
}

pub fn unavailable_with_diagnostic() -> SemanticOverlaySnapshot {
    let mut snapshot = empty_snapshot();
    snapshot.partial = true;
    snapshot.diagnostics.push(SemanticDiagnostic {
        code: "semantic_overlay_unavailable",
        message: "The SCIP overlay could not be read safely. Tree-sitter remains available; re-run `mastermind enrich --scip <index.scip>` to replace it.".into(),
    });
    snapshot
}

pub fn query(
    store: &Store,
    symbol: &str,
    top: u32,
) -> Result<SemanticOverlaySnapshot, ScipOverlayError> {
    if symbol.len() > MAX_SYMBOL_BYTES || symbol.contains('\0') {
        return Err(ScipOverlayError::InvalidQuery(format!(
            "query exceeds {MAX_SYMBOL_BYTES} bytes or contains a NUL byte"
        )));
    }
    if symbol.trim().is_empty() {
        return Err(ScipOverlayError::InvalidQuery(
            "query must not be empty".into(),
        ));
    }
    let source = store
        .semantic_source()
        .map_err(|error| ScipOverlayError::Store(error.to_string()))?;
    let Some(source) = source else {
        return Ok(empty_snapshot());
    };
    if !source.repository_verified {
        return Ok(unverified_repository_snapshot(source));
    }
    let limit = usize::try_from(top.clamp(1, 500)).unwrap_or(500);
    let (mut definitions, definitions_truncated) = store
        .semantic_definitions(Some(symbol), limit)
        .map_err(|error| ScipOverlayError::Store(error.to_string()))?;
    let (mut edges, edges_truncated) = store
        .semantic_edges(Some(symbol), &[], limit)
        .map_err(|error| ScipOverlayError::Store(error.to_string()))?;
    let root = store
        .meta_value("index_root")
        .map_err(|error| ScipOverlayError::Store(error.to_string()))?
        .ok_or_else(|| ScipOverlayError::Store("the index has no repository identity".into()))?;
    let root = PathBuf::from(root)
        .canonicalize()
        .map_err(|error| ScipOverlayError::Store(format!("resolve index root: {error}")))?;
    let semantic_paths =
        definitions
            .iter()
            .map(|definition| definition.file.clone())
            .chain(edges.iter().flat_map(|edge| {
                std::iter::once(edge.from_file.clone()).chain(edge.to_file.clone())
            }))
            .collect::<Vec<_>>();
    let stale = stale_semantic_paths(store, &root, semantic_paths)?;
    definitions.retain(|definition| !stale.contains(&definition.file));
    edges.retain(|edge| {
        !stale.contains(&edge.from_file)
            && edge
                .to_file
                .as_ref()
                .is_none_or(|path| !stale.contains(path))
    });
    let mut diagnostics = revision_diagnostic(&source).into_iter().collect::<Vec<_>>();
    diagnostics.extend(stale_diagnostic(&stale));
    Ok(SemanticOverlaySnapshot {
        schema_version: 1,
        available: true,
        partial: definitions_truncated || edges_truncated || !stale.is_empty(),
        fallback_active: false,
        source: Some(source),
        definitions: collection(definitions, definitions_truncated, "definition_limit"),
        edges: collection(edges, edges_truncated, "semantic_edge_limit"),
        diagnostics,
        resolution: resolution(),
    })
}

fn unverified_repository_snapshot(source: SemanticSource) -> SemanticOverlaySnapshot {
    let mut snapshot = empty_snapshot();
    snapshot.available = true;
    snapshot.partial = true;
    snapshot.source = Some(source);
    snapshot.diagnostics.push(SemanticDiagnostic {
        code: "semantic_repository_unverified",
        message: "SCIP evidence was omitted because its repository identity is not verified. Re-import it with `mastermind enrich --scip <index.scip>`.".into(),
    });
    snapshot
}

fn collection<T>(items: Vec<T>, truncated: bool, reason: &'static str) -> SemanticCollection<T> {
    let returned = u32::try_from(items.len()).unwrap_or(u32::MAX);
    SemanticCollection {
        total: (!truncated).then_some(returned),
        returned,
        truncated,
        truncation_reason: truncated.then_some(reason),
        items,
    }
}

fn stale_semantic_paths(
    store: &Store,
    root: &Path,
    paths: impl IntoIterator<Item = String>,
) -> Result<BTreeSet<String>, ScipOverlayError> {
    let paths = paths.into_iter().collect::<BTreeSet<_>>();
    let path_vec = paths.iter().cloned().collect::<Vec<_>>();
    let hashes = store
        .semantic_document_hashes(&path_vec)
        .map_err(|error| ScipOverlayError::Store(error.to_string()))?;
    let mut stale = paths;
    for (path, expected) in hashes {
        stale.remove(&path);
        let actual = root.join(&path).canonicalize().ok().and_then(|resolved| {
            resolved
                .starts_with(root)
                .then(|| hash_file(&resolved).ok().map(|value| value.0))
                .flatten()
        });
        if actual.as_deref() != Some(expected.as_str()) {
            stale.insert(path);
        }
    }
    Ok(stale)
}

fn revision_diagnostic(source: &SemanticSource) -> Option<SemanticDiagnostic> {
    (!source.revision_verified).then(|| SemanticDiagnostic {
        code: "semantic_artifact_revision_unverified",
        message: "The SCIP producer omitted embedded text for at least one document. Mastermind can detect changes after import, but cannot prove that every imported semantic fact was generated from the current revision.".into(),
    })
}

fn stale_diagnostic(stale: &BTreeSet<String>) -> Option<SemanticDiagnostic> {
    if stale.is_empty() {
        return None;
    }
    let sample = stale.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    Some(SemanticDiagnostic {
        code: "semantic_overlay_stale",
        message: format!(
            "SCIP evidence was omitted for {} changed or unavailable document(s): {sample}. Re-run the language indexer and `mastermind enrich --scip <index.scip>`.",
            stale.len()
        ),
    })
}

pub fn for_lens(
    store: &Store,
    root: &Path,
    relevant_paths: impl IntoIterator<Item = String>,
) -> Result<SemanticOverlaySnapshot, ScipOverlayError> {
    let source = store
        .semantic_source()
        .map_err(|error| ScipOverlayError::Store(error.to_string()))?;
    let Some(source) = source else {
        return Ok(empty_snapshot());
    };
    if !source.repository_verified {
        return Ok(unverified_repository_snapshot(source));
    }
    let paths = relevant_paths.into_iter().collect::<BTreeSet<_>>();
    let path_vec = paths.into_iter().collect::<Vec<_>>();
    let (mut edges, edges_truncated) = if path_vec.is_empty() {
        (Vec::new(), false)
    } else {
        store
            .semantic_edges(None, &path_vec, MAX_LENS_SEMANTIC_EDGES)
            .map_err(|error| ScipOverlayError::Store(error.to_string()))?
    };
    let edge_paths = edges
        .iter()
        .flat_map(|edge| std::iter::once(edge.from_file.clone()).chain(edge.to_file.clone()))
        .collect::<Vec<_>>();
    let stale = stale_semantic_paths(store, root, edge_paths)?;
    edges.retain(|edge| {
        !stale.contains(&edge.from_file)
            && edge
                .to_file
                .as_ref()
                .is_none_or(|path| !stale.contains(path))
    });
    let mut diagnostics = revision_diagnostic(&source).into_iter().collect::<Vec<_>>();
    diagnostics.extend(stale_diagnostic(&stale));
    let partial = edges_truncated || !stale.is_empty();
    Ok(SemanticOverlaySnapshot {
        schema_version: 1,
        available: true,
        partial,
        fallback_active: false,
        source: Some(source),
        definitions: collection(Vec::new(), false, "definition_limit"),
        edges: collection(edges, edges_truncated, "semantic_edge_limit"),
        diagnostics,
        resolution: resolution(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::{EnumOrUnknown, Message, MessageField};
    use scip::types::{
        occurrence, Document, Index, Metadata, MultiLineRange, Occurrence, Relationship,
        SingleLineRange, SymbolInformation, SymbolRole, ToolInfo,
    };
    use tempfile::TempDir;

    fn definition(symbol: &str, line: i32, body_start: i32, body_end: i32) -> Occurrence {
        let mut occurrence = Occurrence::new();
        occurrence.symbol = symbol.into();
        occurrence.symbol_roles = SymbolRole::Definition as i32;
        occurrence.typed_range = Some(occurrence::Typed_range::SingleLineRange(SingleLineRange {
            line,
            start_character: 3,
            end_character: 8,
            ..Default::default()
        }));
        occurrence.typed_enclosing_range = Some(
            occurrence::Typed_enclosing_range::MultiLineEnclosingRange(MultiLineRange {
                start_line: body_start,
                start_character: 0,
                end_line: body_end,
                end_character: 100,
                ..Default::default()
            }),
        );
        occurrence
    }

    fn reference(symbol: &str, line: i32) -> Occurrence {
        let mut occurrence = Occurrence::new();
        occurrence.symbol = symbol.into();
        occurrence.typed_range = Some(occurrence::Typed_range::SingleLineRange(SingleLineRange {
            line,
            start_character: 4,
            end_character: 9,
            ..Default::default()
        }));
        occurrence
    }

    fn symbol(symbol: &str, display: &str) -> SymbolInformation {
        let mut info = SymbolInformation::new();
        info.symbol = symbol.into();
        info.display_name = display.into();
        info.kind = EnumOrUnknown::from_i32(17);
        info
    }

    fn local_file_uri(path: &Path) -> String {
        let normalized = path.to_string_lossy().replace('\\', "/");
        if normalized.starts_with('/') {
            format!("file://{normalized}")
        } else {
            format!("file:///{normalized}")
        }
    }

    fn fixture() -> (TempDir, Store, PathBuf) {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let a_text = "int target() { return 1; }\n";
        let b_text = "int caller() {\n  return target();\n}\n";
        std::fs::write(root.join("a.cpp"), a_text).unwrap();
        std::fs::write(root.join("b.cpp"), b_text).unwrap();
        let db = temp.path().join("index.db");
        let store = Store::open(&db).unwrap();
        store
            .set_meta("index_root", root.canonicalize().unwrap().to_str().unwrap())
            .unwrap();

        let target = "scip-clang . demo . target().";
        let caller = "scip-clang . demo . caller().";
        let mut target_info = symbol(target, "target");
        target_info.relationships.push(Relationship {
            symbol: caller.into(),
            is_definition: true,
            ..Default::default()
        });
        let mut a = Document::new();
        a.relative_path = "a.cpp".into();
        a.language = "cpp".into();
        a.text = a_text.into();
        a.occurrences.push(definition(target, 0, 0, 0));
        a.symbols.push(target_info);

        let mut b = Document::new();
        b.relative_path = "b.cpp".into();
        b.language = "cpp".into();
        b.text = b_text.into();
        b.occurrences.push(definition(caller, 0, 0, 2));
        b.occurrences.push(reference(target, 1));
        b.symbols.push(symbol(caller, "caller"));

        let mut index = Index::new();
        index.metadata = MessageField::some(Metadata {
            project_root: local_file_uri(&root),
            tool_info: MessageField::some(ToolInfo {
                name: "scip-clang".into(),
                version: "test".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        index.documents = vec![a, b];
        let scip_path = temp.path().join("index.scip");
        scip::write_message_to_file(&scip_path, index).unwrap();
        (temp, store, scip_path)
    }

    #[test]
    fn import_keeps_semantic_facts_separate_and_resolves_reference_owner() {
        let (_temp, store, path) = fixture();
        let summary = import(&store, &path).unwrap();
        assert_eq!(summary.source.documents, 2);
        assert_eq!(summary.source.text_verified_documents, 2);
        assert!(summary.source.repository_verified);
        assert!(summary.source.revision_verified);

        let snapshot = query(&store, "target", 20).unwrap();
        assert!(snapshot.available);
        assert_eq!(
            snapshot.resolution.static_precedence,
            ["scip", "tree-sitter"]
        );
        let reference = snapshot
            .edges
            .items
            .iter()
            .find(|edge| edge.kind == "reference")
            .unwrap();
        assert_eq!(reference.from_display_name.as_deref(), Some("caller"));
        assert_eq!(reference.from_file, "b.cpp");
        assert_eq!(reference.from_line, 1);
        assert_eq!(reference.occurrence_line, 2);
        assert_eq!(reference.to_file.as_deref(), Some("a.cpp"));
        assert_eq!(reference.to_line, Some(1));
        assert_eq!(reference.provenance, "scip");
        assert_eq!(reference.confidence, "high");

        assert_eq!(store.callers_of("target", None, None).unwrap().len(), 0);
    }

    #[test]
    fn malformed_path_does_not_replace_previous_overlay() {
        let (_temp, store, path) = fixture();
        import(&store, &path).unwrap();
        let previous = store.semantic_source().unwrap().unwrap().artifact_sha256;

        let mut bad = Index::new();
        let mut document = Document::new();
        document.relative_path = "../outside.cpp".into();
        bad.documents.push(document);
        scip::write_message_to_file(&path, bad).unwrap();
        assert!(import(&store, &path).is_err());
        assert_eq!(
            store.semantic_source().unwrap().unwrap().artifact_sha256,
            previous
        );
    }

    #[test]
    fn foreign_project_root_without_embedded_text_is_rejected_atomically() {
        let (temp, store, path) = fixture();
        import(&store, &path).unwrap();
        let previous = store.semantic_source().unwrap().unwrap().artifact_sha256;

        let bytes = std::fs::read(&path).unwrap();
        let mut index = Index::parse_from_bytes(&bytes).unwrap();
        for document in &mut index.documents {
            document.text.clear();
        }
        let foreign = temp.path().join("foreign-repo");
        std::fs::create_dir(&foreign).unwrap();
        index.metadata.as_mut().unwrap().project_root = local_file_uri(&foreign);
        scip::write_message_to_file(&path, index).unwrap();

        let error = import(&store, &path).unwrap_err().to_string();
        assert!(error.contains("does not match the indexed repository"));
        assert_eq!(
            store.semantic_source().unwrap().unwrap().artifact_sha256,
            previous
        );
    }

    #[test]
    fn lens_omits_stale_semantic_edges_without_breaking_tree_sitter_fallback() {
        let (temp, store, path) = fixture();
        import(&store, &path).unwrap();
        let root = temp.path().join("repo").canonicalize().unwrap();
        std::fs::write(root.join("b.cpp"), "int caller() { return 2; }\n").unwrap();
        let snapshot = for_lens(&store, &root, ["a.cpp".into(), "b.cpp".into()]).unwrap();
        assert!(snapshot.available);
        assert!(snapshot.partial);
        assert!(snapshot.edges.items.is_empty());
        assert_eq!(snapshot.diagnostics[0].code, "semantic_overlay_stale");
        assert_eq!(snapshot.resolution.default_graph, "tree-sitter");

        let queried = query(&store, "target", 20).unwrap();
        assert!(queried.edges.items.is_empty());
        assert!(queried
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "semantic_overlay_stale"));
    }

    #[test]
    fn no_overlay_is_an_explicit_non_error_fallback() {
        let temp = TempDir::new().unwrap();
        let store = Store::open(temp.path().join("index.db")).unwrap();
        let snapshot = query(&store, "anything", 10).unwrap();
        assert!(!snapshot.available);
        assert!(snapshot.fallback_active);
        assert_eq!(snapshot.edges.returned, 0);
    }

    #[test]
    fn lens_does_not_leak_the_whole_overlay_without_relevant_paths() {
        let (temp, store, path) = fixture();
        import(&store, &path).unwrap();
        let root = temp.path().join("repo").canonicalize().unwrap();
        let snapshot = for_lens(&store, &root, Vec::<String>::new()).unwrap();
        assert!(snapshot.available);
        assert!(snapshot.edges.items.is_empty());
    }

    #[test]
    fn typed_ranges_and_forward_definitions_follow_the_modern_scip_fields() {
        let mut occurrence = definition("local 0", 4, 4, 4);
        occurrence.range = vec![99, 1, 2];
        occurrence.symbol_roles = SymbolRole::ForwardDefinition as i32;
        assert!(is_definition_occurrence(&occurrence));
        assert_eq!(occurrence_range(&occurrence).unwrap().start_line, 4);
    }

    #[test]
    fn nested_reference_owner_prefers_the_innermost_multiline_definition() {
        let outer = DefinitionLocator {
            definition_index: 0,
            body: SourceRange {
                start_line: 1,
                start_character: 20,
                end_line: 8,
                end_character: 10,
            },
        };
        let inner = DefinitionLocator {
            definition_index: 1,
            body: SourceRange {
                start_line: 1,
                start_character: 30,
                end_line: 8,
                end_character: 5,
            },
        };
        let reference = SourceRange {
            start_line: 4,
            start_character: 1,
            end_line: 4,
            end_character: 2,
        };
        assert_eq!(
            owner_for_reference(&[outer, inner], reference)
                .unwrap()
                .definition_index,
            1
        );
    }

    #[test]
    fn embedded_text_verification_honors_scip_utf16_metadata() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("utf16.cpp");
        let source = "int target() { return 1; }\n";
        let mut bytes = vec![0xff, 0xfe];
        for unit in source.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        assert!(embedded_text_matches_file(&path, source, TextEncoding::UTF16).unwrap());
        assert!(!embedded_text_matches_file(&path, "different", TextEncoding::UTF16).unwrap());
    }
}
