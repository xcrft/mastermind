use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd};

pub const BUNDLE_INPUT_MAX: usize = 16 * 1024 * 1024;
pub const ENVELOPE_SCHEMA: u32 = 3;
pub const CANONICALIZATION: &str = "mastermind-cjson-v1";
pub const SIGNATURE_DOMAIN: &str = "mastermind/audit-envelope-signature/v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub schema_version: u32,
    pub manifest: Manifest,
    pub integrity: Integrity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Integrity {
    pub algorithm: String,
    pub canonicalization: String,
    pub manifest_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub repository: RepositoryBinding,
    pub inputs: InputBinding,
    pub diff: DiffBinding,
    pub tool: ToolBinding,
    #[serde(default)]
    pub audit_configuration: BTreeMap<String, Value>,
    #[serde(default)]
    pub index_metadata: BTreeMap<String, Value>,
    pub verdict: String,
    #[serde(default)]
    pub declared_files: Vec<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub verified_claims: Vec<String>,
    #[serde(default)]
    pub failed_claims: Vec<String>,
    #[serde(default)]
    pub discrepancies: Vec<Value>,
    #[serde(default)]
    pub snapshot_drift: Vec<Value>,
    #[serde(default)]
    pub snapshot_changed: bool,
    #[serde(default)]
    pub mmcg_queries: Vec<String>,
    #[serde(default)]
    pub verify_commands: Vec<String>,
    pub human_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryBinding {
    pub identity: Option<String>,
    pub baseline_oid: String,
    pub head_oid: String,
    pub worktree_clean: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InputBinding {
    pub spec_path: String,
    pub spec_sha256: String,
    pub executor_report_path: Option<String>,
    pub executor_report_present: bool,
    pub executor_report_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DiffBinding {
    pub name_status: Vec<DiffEntry>,
    pub binary_diff_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiffEntry {
    pub status: String,
    pub path: String,
    pub old_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ToolBinding {
    pub name: String,
    pub version: String,
    pub bundle_schema: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DetachedSignature {
    pub schema_version: u32,
    pub algorithm: String,
    pub key_id: String,
    pub manifest_digest: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerificationReport {
    pub content_integrity: String,
    pub provenance_authenticity: String,
    pub policy_acceptance: String,
    pub reasons: Vec<String>,
    pub untrusted_diagnostic: bool,
}

impl VerificationReport {
    pub fn accepted(&self) -> bool {
        self.content_integrity == "pass" && self.policy_acceptance == "pass"
    }
}

#[derive(Debug, Clone, Default)]
pub struct VerifyPolicy {
    pub expected_repository: Option<String>,
    pub expected_baseline: Option<String>,
    pub expected_head: Option<String>,
    pub root: Option<PathBuf>,
    pub require_clean_worktree: bool,
    pub public_key: Option<PathBuf>,
    pub signature: Option<DetachedSignature>,
    pub require_signature: bool,
    pub trusted_key_ids: BTreeSet<String>,
    pub revoked_key_ids: BTreeSet<String>,
    pub integrity_only: bool,
}

#[derive(Debug)]
pub enum BundleError {
    Io(String),
    Json(String),
    Invalid(String),
    SizeLimit,
    Crypto(String),
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(s) => write!(f, "I/O error: {s}"),
            Self::Json(s) => write!(f, "invalid JSON: {s}"),
            Self::Invalid(s) => write!(f, "invalid audit bundle: {s}"),
            Self::SizeLimit => write!(f, "audit input exceeds 16 MiB limit"),
            Self::Crypto(s) => write!(f, "signature error: {s}"),
        }
    }
}

impl std::error::Error for BundleError {}

pub fn canonical_json(value: &Value) -> Result<Vec<u8>, BundleError> {
    fn normalize(value: &Value) -> Result<Value, BundleError> {
        match value {
            Value::Null | Value::Bool(_) | Value::String(_) => Ok(value.clone()),
            Value::Number(number) => {
                if number.as_i64().is_none() && number.as_u64().is_none() {
                    return Err(BundleError::Invalid("non-integer JSON number".into()));
                }
                Ok(value.clone())
            }
            Value::Array(values) => values
                .iter()
                .map(normalize)
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Value::Object(values) => {
                let mut sorted = BTreeMap::new();
                for (key, value) in values {
                    sorted.insert(key.clone(), normalize(value)?);
                }
                serde_json::to_value(sorted).map_err(|e| BundleError::Json(e.to_string()))
            }
        }
    }
    serde_json::to_vec(&normalize(value)?).map_err(|e| BundleError::Json(e.to_string()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn seal(mut manifest: Manifest) -> Result<Envelope, BundleError> {
    sort_manifest(&mut manifest);
    let value = serde_json::to_value(&manifest).map_err(|e| BundleError::Json(e.to_string()))?;
    let digest = sha256_hex(&canonical_json(&value)?);
    Ok(Envelope {
        schema_version: ENVELOPE_SCHEMA,
        manifest,
        integrity: Integrity {
            algorithm: "sha256".into(),
            canonicalization: CANONICALIZATION.into(),
            manifest_digest: format!("sha256:{digest}"),
        },
    })
}

pub fn seal_checked(manifest: Manifest, root: &Path) -> Result<Envelope, BundleError> {
    seal_checked_with_hook(manifest, root, || {})
}

fn seal_checked_with_hook<F>(
    manifest: Manifest,
    root: &Path,
    between_rechecks: F,
) -> Result<Envelope, BundleError>
where
    F: FnOnce(),
{
    let preview = seal(manifest.clone())?;
    recompute_inputs(&preview, root)
        .map_err(|_| BundleError::Invalid("snapshot_changed".into()))?;
    between_rechecks();
    recompute_inputs(&preview, root)
        .map_err(|_| BundleError::Invalid("snapshot_changed".into()))?;
    seal(manifest)
}

fn sort_manifest(manifest: &mut Manifest) {
    manifest.declared_files.sort();
    manifest.changed_files.sort();
    manifest.verified_claims.sort();
    manifest.failed_claims.sort();
    manifest
        .discrepancies
        .sort_by_cached_key(|value| canonical_json(value).unwrap_or_default());
    manifest
        .snapshot_drift
        .sort_by_cached_key(|value| canonical_json(value).unwrap_or_default());
    manifest.mmcg_queries.sort();
    manifest.verify_commands.sort();
    manifest
        .diff
        .name_status
        .sort_by(|a, b| (&a.path, &a.old_path, &a.status).cmp(&(&b.path, &b.old_path, &b.status)));
}

pub fn read_envelope(path: &Path) -> Result<Envelope, BundleError> {
    let bytes = read_bounded_regular(path, false, false)?;
    from_json_strict(&bytes)
}

pub fn read_signature(path: &Path) -> Result<DetachedSignature, BundleError> {
    let bytes = read_bounded_regular(path, false, true)?;
    from_json_strict(&bytes)
}

pub fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> Result<(), BundleError> {
    if bytes.len() > BUNDLE_INPUT_MAX {
        return Err(BundleError::SizeLimit);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| BundleError::Invalid("invalid output filename".into()))?;
    let mut created = None;
    for nonce in 0..128_u32 {
        let candidate = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), nonce));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(if private { 0o600 } else { 0o644 });
        }
        match options.open(&candidate) {
            Ok(mut file) => {
                file.write_all(bytes)
                    .and_then(|_| file.sync_all())
                    .map_err(|e| BundleError::Io(e.to_string()))?;
                created = Some(candidate);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(BundleError::Io(e.to_string())),
        }
    }
    let temp = created.ok_or_else(|| BundleError::Io("cannot allocate temp file".into()))?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(BundleError::Io(error.to_string()));
    }
    Ok(())
}

pub fn verify_envelope(envelope: &Envelope, policy: &VerifyPolicy) -> VerificationReport {
    let mut report = VerificationReport {
        content_integrity: "fail".into(),
        provenance_authenticity: "not_evaluated".into(),
        policy_acceptance: "fail".into(),
        reasons: Vec::new(),
        untrusted_diagnostic: policy.integrity_only,
    };
    if envelope.schema_version != ENVELOPE_SCHEMA {
        report.reasons.push("unsupported_envelope_schema".into());
        return report;
    }
    if envelope.integrity.algorithm != "sha256"
        || envelope.integrity.canonicalization != CANONICALIZATION
    {
        report.reasons.push("unsupported_integrity_contract".into());
        return report;
    }
    let value = match serde_json::to_value(&envelope.manifest) {
        Ok(value) => value,
        Err(_) => {
            report.reasons.push("manifest_serialization_failed".into());
            return report;
        }
    };
    let canonical = match canonical_json(&value) {
        Ok(bytes) => bytes,
        Err(_) => {
            report
                .reasons
                .push("manifest_canonicalization_failed".into());
            return report;
        }
    };
    let expected_digest = format!("sha256:{}", sha256_hex(&canonical));
    if envelope.integrity.manifest_digest != expected_digest {
        report.reasons.push("manifest_digest_mismatch".into());
        return report;
    }
    if envelope.manifest.snapshot_changed {
        report.reasons.push("snapshot_changed".into());
        return report;
    }
    report.content_integrity = "pass".into();

    if policy.integrity_only {
        report.policy_acceptance = "not_evaluated".into();
        report.reasons.push("integrity_only_untrusted".into());
        return report;
    }

    let snapshot_requested = policy.expected_repository.is_some()
        || policy.expected_baseline.is_some()
        || policy.expected_head.is_some()
        || policy.root.is_some()
        || policy.require_clean_worktree;
    let snapshot_complete = policy.expected_repository.is_some()
        && policy.expected_baseline.is_some()
        && policy.expected_head.is_some()
        && policy.root.is_some()
        && policy.require_clean_worktree;
    let signature_requested = policy.require_signature
        || policy.public_key.is_some()
        || policy.signature.is_some()
        || !policy.trusted_key_ids.is_empty()
        || !policy.revoked_key_ids.is_empty();
    let signature_complete = policy.public_key.is_some()
        && policy.signature.is_some()
        && !policy.trusted_key_ids.is_empty();
    if !snapshot_requested && !signature_requested {
        report.reasons.push("no_trust_anchor".into());
        return report;
    }

    let mut policy_ok = true;
    if snapshot_requested && !snapshot_complete {
        report.reasons.push("incomplete_snapshot_policy".into());
        policy_ok = false;
    }
    if signature_requested && !signature_complete {
        report.reasons.push("incomplete_signature_policy".into());
        policy_ok = false;
    }
    if snapshot_complete {
        let repository = policy.expected_repository.as_deref().unwrap_or_default();
        let baseline = policy.expected_baseline.as_deref().unwrap_or_default();
        let head = policy.expected_head.as_deref().unwrap_or_default();
        if !valid_repository_identity(repository)
            || envelope.manifest.repository.identity.as_deref() != Some(repository)
        {
            report.reasons.push("repository_mismatch".into());
            policy_ok = false;
        }
        if !full_oid(baseline) || envelope.manifest.repository.baseline_oid != baseline {
            report.reasons.push("baseline_mismatch".into());
            policy_ok = false;
        }
        if !full_oid(head) || envelope.manifest.repository.head_oid != head {
            report.reasons.push("head_mismatch".into());
            policy_ok = false;
        }
        if !envelope.manifest.repository.worktree_clean {
            report.reasons.push("worktree_not_clean".into());
            policy_ok = false;
        }
        if let Some(root) = policy.root.as_deref() {
            if let Err(reason) = recompute_inputs(envelope, root) {
                report.reasons.push(reason);
                policy_ok = false;
            }
        }
    }

    if signature_complete {
        let (Some(signature), Some(public_key)) =
            (policy.signature.as_ref(), policy.public_key.as_deref())
        else {
            report.reasons.push("incomplete_signature_policy".into());
            return report;
        };
        let key_id = signature.key_id.clone();
        if policy.revoked_key_ids.contains(&key_id) {
            report.reasons.push("revoked_key_id".into());
            policy_ok = false;
            report.provenance_authenticity = "fail".into();
        } else if !policy.trusted_key_ids.contains(&key_id) {
            report.reasons.push("untrusted_key_id".into());
            policy_ok = false;
            report.provenance_authenticity = "fail".into();
        } else {
            match verify_detached(envelope, signature, public_key) {
                Ok(()) => report.provenance_authenticity = "pass".into(),
                Err(_) => {
                    report.provenance_authenticity = "fail".into();
                    report.reasons.push("signature_verification_failed".into());
                    policy_ok = false;
                }
            }
        }
    }

    if policy_ok {
        report.policy_acceptance = "pass".into();
    }
    report
}

pub fn sign_detached(
    envelope: &Envelope,
    private_key_path: &Path,
) -> Result<DetachedSignature, BundleError> {
    let integrity = verify_envelope(
        envelope,
        &VerifyPolicy {
            integrity_only: true,
            ..VerifyPolicy::default()
        },
    );
    if integrity.content_integrity != "pass" {
        return Err(BundleError::Invalid("cannot sign invalid envelope".into()));
    }
    let seed = read_key(private_key_path, true)?;
    let signing = SigningKey::from_bytes(&seed);
    let public = signing.verifying_key().to_bytes();
    let key_id = format!("sha256:{}", sha256_hex(&public));
    let statement = signature_statement(&key_id, &envelope.integrity.manifest_digest)?;
    let signature = signing.sign(&statement);
    Ok(DetachedSignature {
        schema_version: 1,
        algorithm: "ed25519".into(),
        key_id,
        manifest_digest: envelope.integrity.manifest_digest.clone(),
        signature: BASE64.encode(signature.to_bytes()),
    })
}

pub fn verify_detached(
    envelope: &Envelope,
    signature: &DetachedSignature,
    public_key_path: &Path,
) -> Result<(), BundleError> {
    if envelope.schema_version != ENVELOPE_SCHEMA
        || signature.schema_version != 1
        || signature.algorithm != "ed25519"
        || envelope.integrity.algorithm != "sha256"
        || envelope.integrity.canonicalization != CANONICALIZATION
        || signature.manifest_digest != envelope.integrity.manifest_digest
    {
        return Err(BundleError::Crypto("signature contract mismatch".into()));
    }
    let public = read_key(public_key_path, false)?;
    let verifying = VerifyingKey::from_bytes(&public)
        .map_err(|_| BundleError::Crypto("invalid public key".into()))?;
    let key_id = format!("sha256:{}", sha256_hex(&public));
    if signature.key_id != key_id {
        return Err(BundleError::Crypto("key ID mismatch".into()));
    }
    let bytes = BASE64
        .decode(&signature.signature)
        .map_err(|_| BundleError::Crypto("invalid signature encoding".into()))?;
    let parsed = Signature::from_slice(&bytes)
        .map_err(|_| BundleError::Crypto("invalid signature length".into()))?;
    let statement = signature_statement(&key_id, &signature.manifest_digest)?;
    verifying
        .verify(&statement, &parsed)
        .map_err(|_| BundleError::Crypto("signature verification failed".into()))
}

fn signature_statement(key_id: &str, manifest_digest: &str) -> Result<Vec<u8>, BundleError> {
    canonical_json(&serde_json::json!({
        "canonicalization": CANONICALIZATION,
        "domain": SIGNATURE_DOMAIN,
        "envelope_schema": ENVELOPE_SCHEMA,
        "hash_algorithm": "sha256",
        "key_id": key_id,
        "manifest_digest": manifest_digest,
        "signature_algorithm": "ed25519",
        "signature_schema": 1
    }))
}

pub fn normalize_repository_identity(remote: &str) -> Result<String, BundleError> {
    let remote = remote.trim();
    if remote.contains(['?', '#']) || remote.contains('@') && remote.starts_with("http") {
        return Err(BundleError::Invalid("unsafe repository remote".into()));
    }
    let path = if let Some(rest) = remote.strip_prefix("https://github.com/") {
        rest
    } else if let Some(rest) = remote.strip_prefix("git@github.com:") {
        rest
    } else if let Some(rest) = remote.strip_prefix("ssh://git@github.com/") {
        rest
    } else {
        return Err(BundleError::Invalid(
            "repository is not GitHub owner/repo".into(),
        ));
    };
    let identity = path.strip_suffix(".git").unwrap_or(path);
    if !valid_repository_identity(identity) {
        return Err(BundleError::Invalid("invalid repository identity".into()));
    }
    Ok(identity.to_string())
}

pub fn normalize_relative_path(path: &Path) -> Result<String, BundleError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(BundleError::Invalid(
            "path must be repository-relative".into(),
        ));
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let text = part
                    .to_str()
                    .ok_or_else(|| BundleError::Invalid("path is not UTF-8".into()))?;
                if text.is_empty()
                    || text.contains(['\\', '\0'])
                    || text.chars().any(|c| c.is_control())
                {
                    return Err(BundleError::Invalid("unsafe path component".into()));
                }
                parts.push(text);
            }
            _ => return Err(BundleError::Invalid("unsafe path component".into())),
        }
    }
    if parts.is_empty() {
        return Err(BundleError::Invalid("empty relative path".into()));
    }
    Ok(parts.join("/"))
}

pub fn create_contained_output_dir(root: &Path, relative: &Path) -> Result<PathBuf, BundleError> {
    #[cfg(unix)]
    {
        create_contained_output_dir_unix(root, relative, || {})
    }
    #[cfg(not(unix))]
    {
        create_contained_output_dir_portable(root, relative)
    }
}

#[cfg(unix)]
fn path_component_cstring(component: &std::ffi::OsStr) -> Result<CString, BundleError> {
    CString::new(component.as_bytes())
        .map_err(|_| BundleError::Invalid("unsafe path component".into()))
}

#[cfg(unix)]
fn open_dir_at(parent: &File, component: &std::ffi::OsStr) -> Result<File, BundleError> {
    let component = path_component_cstring(component)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            component.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(BundleError::Io(std::io::Error::last_os_error().to_string()));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn open_file_at(parent: &File, component: &std::ffi::OsStr) -> Result<File, BundleError> {
    let component = path_component_cstring(component)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            component.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(BundleError::Io(std::io::Error::last_os_error().to_string()));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn open_directory_chain(path: &Path) -> Result<File, BundleError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| BundleError::Io(e.to_string()))?
            .join(path)
    };
    let mut options = OpenOptions::new();
    options.read(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut directory = options
        .open(Path::new("/"))
        .map_err(|e| BundleError::Io(e.to_string()))?;
    for component in absolute.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(part) => directory = open_dir_at(&directory, part)?,
            _ => return Err(BundleError::Invalid("unsafe path component".into())),
        }
    }
    Ok(directory)
}

#[cfg(unix)]
fn create_contained_output_dir_unix<F>(
    root: &Path,
    relative: &Path,
    before_final: F,
) -> Result<PathBuf, BundleError>
where
    F: FnOnce(),
{
    use std::os::unix::fs::MetadataExt;
    let normalized = normalize_relative_path(relative)?;
    let parts: Vec<_> = Path::new(&normalized)
        .components()
        .map(|component| match component {
            Component::Normal(part) => Ok(part.to_os_string()),
            _ => Err(BundleError::Invalid("unsafe path component".into())),
        })
        .collect::<Result<_, _>>()?;
    let (final_component, parents) = parts
        .split_last()
        .ok_or_else(|| BundleError::Invalid("empty relative path".into()))?;
    let root_directory = open_directory_chain(root)?;
    let root_metadata = root_directory
        .metadata()
        .map_err(|e| BundleError::Io(e.to_string()))?;
    let mut directory = root_directory
        .try_clone()
        .map_err(|e| BundleError::Io(e.to_string()))?;
    for parent in parents {
        match open_dir_at(&directory, parent) {
            Ok(next) => directory = next,
            Err(BundleError::Io(_)) => {
                let component = path_component_cstring(parent)?;
                let result =
                    unsafe { libc::mkdirat(directory.as_raw_fd(), component.as_ptr(), 0o700) };
                if result != 0 {
                    return Err(BundleError::Io(std::io::Error::last_os_error().to_string()));
                }
                directory = open_dir_at(&directory, parent)?;
            }
            Err(error) => return Err(error),
        }
    }
    before_final();
    let final_name = path_component_cstring(final_component)?;
    if unsafe { libc::mkdirat(directory.as_raw_fd(), final_name.as_ptr(), 0o700) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(BundleError::Invalid(
                "output final component already exists".into(),
            ));
        }
        return Err(BundleError::Io(error.to_string()));
    }
    let created = open_dir_at(&directory, final_component)?;
    let created_metadata = created
        .metadata()
        .map_err(|e| BundleError::Io(e.to_string()))?;
    let root_recheck = open_directory_chain(root)?;
    let root_recheck_metadata = root_recheck
        .metadata()
        .map_err(|e| BundleError::Io(e.to_string()))?;
    if root_metadata.dev() != root_recheck_metadata.dev()
        || root_metadata.ino() != root_recheck_metadata.ino()
    {
        return Err(BundleError::Invalid("output root identity changed".into()));
    }
    let mut current = root_recheck;
    for part in &parts {
        current = open_dir_at(&current, part)?;
    }
    let current_metadata = current
        .metadata()
        .map_err(|e| BundleError::Io(e.to_string()))?;
    if created_metadata.dev() != current_metadata.dev()
        || created_metadata.ino() != current_metadata.ino()
    {
        return Err(BundleError::Invalid(
            "output directory identity changed".into(),
        ));
    }
    let absolute_root = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| BundleError::Io(e.to_string()))?
            .join(root)
    };
    Ok(absolute_root.join(normalized))
}

#[cfg(not(unix))]
fn create_contained_output_dir_portable(
    root: &Path,
    relative: &Path,
) -> Result<PathBuf, BundleError> {
    if std::fs::symlink_metadata(root)
        .map_err(|e| BundleError::Io(e.to_string()))?
        .file_type()
        .is_symlink()
    {
        return Err(BundleError::Invalid(
            "symlinked output root rejected".into(),
        ));
    }
    let root = root
        .canonicalize()
        .map_err(|e| BundleError::Io(e.to_string()))?;
    let normalized = normalize_relative_path(relative)?;
    let mut cursor = root.clone();
    let components: Vec<_> = Path::new(&normalized).components().collect();
    for (index, component) in components.iter().enumerate() {
        cursor.push(component.as_os_str());
        let final_component = index + 1 == components.len();
        match std::fs::symlink_metadata(&cursor) {
            Ok(metadata) => {
                if final_component {
                    return Err(BundleError::Invalid(
                        "output final component already exists".into(),
                    ));
                }
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(BundleError::Invalid("unsafe output path component".into()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = std::fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder
                    .create(&cursor)
                    .map_err(|e| BundleError::Io(e.to_string()))?;
            }
            Err(error) => return Err(BundleError::Io(error.to_string())),
        }
        let canonical = cursor
            .canonicalize()
            .map_err(|e| BundleError::Io(e.to_string()))?;
        if !canonical.starts_with(&root) {
            return Err(BundleError::Invalid(
                "output path escapes trusted root".into(),
            ));
        }
    }
    let before = std::fs::symlink_metadata(&cursor).map_err(|e| BundleError::Io(e.to_string()))?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
    }
    let directory = options
        .open(&cursor)
        .map_err(|e| BundleError::Io(e.to_string()))?;
    let opened = directory
        .metadata()
        .map_err(|e| BundleError::Io(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != opened.dev() || before.ino() != opened.ino() {
            return Err(BundleError::Invalid(
                "output directory identity changed".into(),
            ));
        }
    }
    Ok(cursor)
}

fn recompute_inputs(envelope: &Envelope, root: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(root)
        .map_err(|_| "root_invalid".to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("root_symlink".into());
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "root_invalid".to_string())?;
    if let Some(reference) = envelope
        .manifest
        .audit_configuration
        .get("baseline_input")
        .and_then(Value::as_str)
    {
        if reference.starts_with('-') || reference.contains(['\0', '\n', '\r']) {
            return Err("baseline_ref_invalid".into());
        }
        let expression = format!("{reference}^{{commit}}");
        let baseline = git_output(
            &canonical_root,
            &["rev-parse", "--verify", "--end-of-options", &expression],
        )
        .map_err(|_| "baseline_recompute_failed".to_string())?;
        if std::str::from_utf8(&baseline).map(str::trim).ok()
            != Some(envelope.manifest.repository.baseline_oid.as_str())
        {
            return Err("baseline_snapshot_mismatch".into());
        }
    }
    let current_head = git_output(&canonical_root, &["rev-parse", "--verify", "HEAD^{commit}"])
        .map_err(|_| "head_recompute_failed".to_string())?;
    if std::str::from_utf8(&current_head).map(str::trim).ok()
        != Some(envelope.manifest.repository.head_oid.as_str())
    {
        return Err("head_snapshot_mismatch".into());
    }
    let status = git_output(
        &canonical_root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
    )
    .map_err(|_| "worktree_recompute_failed".to_string())?;
    if !status.is_empty() {
        return Err("worktree_not_clean".into());
    }
    if let Some(expected_identity) = envelope.manifest.repository.identity.as_deref() {
        let remote = git_output(&canonical_root, &["config", "--get", "remote.origin.url"])
            .map_err(|_| "repository_recompute_failed".to_string())?;
        let actual_identity = std::str::from_utf8(&remote)
            .ok()
            .and_then(|value| normalize_repository_identity(value).ok());
        if actual_identity.as_deref() != Some(expected_identity) {
            return Err("repository_root_mismatch".into());
        }
    }
    let spec_bytes = read_bounded_relative(
        &canonical_root,
        &envelope.manifest.inputs.spec_path,
        false,
        false,
    )
    .map_err(|_| "spec_read_failed".to_string())?;
    if format!("sha256:{}", sha256_hex(&spec_bytes)) != envelope.manifest.inputs.spec_sha256 {
        return Err("spec_digest_mismatch".into());
    }
    match (
        envelope.manifest.inputs.executor_report_present,
        envelope.manifest.inputs.executor_report_path.as_deref(),
        envelope.manifest.inputs.executor_report_sha256.as_deref(),
    ) {
        (true, Some(path), Some(digest)) => {
            let bytes = read_bounded_relative(&canonical_root, path, false, false)
                .map_err(|_| "executor_report_read_failed".to_string())?;
            if format!("sha256:{}", sha256_hex(&bytes)) != digest {
                return Err("executor_report_digest_mismatch".into());
            }
        }
        (false, None, None) => {}
        _ => return Err("executor_report_binding_invalid".into()),
    }
    let diff = git_output(
        &canonical_root,
        &[
            "-c",
            "diff.external=",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--binary",
            &format!(
                "{}..{}",
                envelope.manifest.repository.baseline_oid, envelope.manifest.repository.head_oid
            ),
            "--",
        ],
    )
    .map_err(|_| "diff_recompute_failed".to_string())?;
    if format!("sha256:{}", sha256_hex(&diff)) != envelope.manifest.diff.binary_diff_sha256 {
        return Err("diff_digest_mismatch".into());
    }
    let range = format!(
        "{}..{}",
        envelope.manifest.repository.baseline_oid, envelope.manifest.repository.head_oid
    );
    let name_status = git_output(
        &canonical_root,
        &[
            "-c",
            "diff.external=",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--name-status",
            "-z",
            &range,
            "--",
        ],
    )
    .map_err(|_| "name_status_recompute_failed".to_string())?;
    let mut parsed = parse_name_status(&name_status).map_err(|_| "name_status_invalid")?;
    parsed
        .sort_by(|a, b| (&a.path, &a.old_path, &a.status).cmp(&(&b.path, &b.old_path, &b.status)));
    if parsed != envelope.manifest.diff.name_status {
        return Err("name_status_mismatch".into());
    }
    Ok(())
}

fn parse_name_status(bytes: &[u8]) -> Result<Vec<DiffEntry>, BundleError> {
    let fields: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect();
    let mut entries = Vec::new();
    let mut cursor = 0;
    while cursor < fields.len() {
        let status = std::str::from_utf8(fields[cursor])
            .map_err(|_| BundleError::Invalid("non-UTF8 name-status".into()))?
            .to_string();
        cursor += 1;
        if cursor >= fields.len() {
            return Err(BundleError::Invalid("malformed name-status".into()));
        }
        let path = normalize_relative_path(Path::new(
            std::str::from_utf8(fields[cursor])
                .map_err(|_| BundleError::Invalid("non-UTF8 diff path".into()))?,
        ))?;
        cursor += 1;
        entries.push(DiffEntry {
            status,
            path,
            old_path: None,
        });
    }
    Ok(entries)
}

#[cfg(not(unix))]
fn contained_file(root: &Path, relative: &str) -> Result<PathBuf, BundleError> {
    let normalized = normalize_relative_path(Path::new(relative))?;
    let joined = root.join(normalized);
    let mut cursor = root.to_path_buf();
    for component in joined
        .strip_prefix(root)
        .map_err(|_| BundleError::Invalid("path escapes trusted root".into()))?
        .components()
    {
        cursor.push(component.as_os_str());
        if std::fs::symlink_metadata(&cursor)
            .map_err(|e| BundleError::Io(e.to_string()))?
            .file_type()
            .is_symlink()
        {
            return Err(BundleError::Invalid("symlink input rejected".into()));
        }
    }
    let canonical = joined
        .canonicalize()
        .map_err(|e| BundleError::Io(e.to_string()))?;
    if !canonical.starts_with(root) {
        return Err(BundleError::Invalid("path escapes trusted root".into()));
    }
    Ok(canonical)
}

fn read_bounded_regular(
    path: &Path,
    private: bool,
    forbid_shared_write: bool,
) -> Result<Vec<u8>, BundleError> {
    #[cfg(unix)]
    {
        read_bounded_regular_unix(path, private, forbid_shared_write, || {})
    }
    #[cfg(not(unix))]
    {
        read_bounded_regular_portable(path, private, forbid_shared_write)
    }
}

fn read_bounded_relative(
    root: &Path,
    relative: &str,
    private: bool,
    forbid_shared_write: bool,
) -> Result<Vec<u8>, BundleError> {
    let normalized = normalize_relative_path(Path::new(relative))?;
    #[cfg(unix)]
    {
        read_bounded_relative_unix(
            root,
            Path::new(&normalized),
            private,
            forbid_shared_write,
            || {},
        )
    }
    #[cfg(not(unix))]
    {
        let path = contained_file(root, &normalized)?;
        read_bounded_regular_portable(&path, private, forbid_shared_write)
    }
}

#[cfg(unix)]
fn read_opened_file(
    mut file: File,
    private: bool,
    forbid_shared_write: bool,
) -> Result<Vec<u8>, BundleError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let opened = file
        .metadata()
        .map_err(|e| BundleError::Io(e.to_string()))?;
    if !opened.is_file() {
        return Err(BundleError::Invalid("input must be a regular file".into()));
    }
    if opened.len() > BUNDLE_INPUT_MAX as u64 {
        return Err(BundleError::SizeLimit);
    }
    if private || forbid_shared_write {
        let forbidden = if private { 0o077 } else { 0o022 };
        if opened.permissions().mode() & forbidden != 0 {
            return Err(BundleError::Invalid("unsafe input file permissions".into()));
        }
    }
    if private && opened.uid() != unsafe { libc::geteuid() } {
        return Err(BundleError::Invalid(
            "private key must be owned by the current user".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    std::io::Read::by_ref(&mut file)
        .take((BUNDLE_INPUT_MAX + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| BundleError::Io(e.to_string()))?;
    if bytes.len() > BUNDLE_INPUT_MAX {
        return Err(BundleError::SizeLimit);
    }
    let after = file
        .metadata()
        .map_err(|e| BundleError::Io(e.to_string()))?;
    if opened.dev() != after.dev() || opened.ino() != after.ino() || opened.len() != after.len() {
        return Err(BundleError::Invalid(
            "input identity changed while reading".into(),
        ));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn read_bounded_regular_unix<F>(
    path: &Path,
    private: bool,
    forbid_shared_write: bool,
    before_leaf: F,
) -> Result<Vec<u8>, BundleError>
where
    F: FnOnce(),
{
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| BundleError::Io(e.to_string()))?
            .join(path)
    };
    let mut options = OpenOptions::new();
    options.read(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut directory = options
        .open(Path::new("/"))
        .map_err(|e| BundleError::Io(e.to_string()))?;
    let mut parts = Vec::new();
    for component in absolute.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(part) => parts.push(part.to_os_string()),
            _ => return Err(BundleError::Invalid("unsafe path component".into())),
        }
    }
    let (leaf, parents) = parts
        .split_last()
        .ok_or_else(|| BundleError::Invalid("input path has no leaf".into()))?;
    for parent in parents {
        directory = open_dir_at(&directory, parent)?;
    }
    before_leaf();
    let file = open_file_at(&directory, leaf)?;
    read_opened_file(file, private, forbid_shared_write)
}

#[cfg(unix)]
fn read_bounded_relative_unix<F>(
    root: &Path,
    relative: &Path,
    private: bool,
    forbid_shared_write: bool,
    before_leaf: F,
) -> Result<Vec<u8>, BundleError>
where
    F: FnOnce(),
{
    let normalized = normalize_relative_path(relative)?;
    let parts: Vec<_> = Path::new(&normalized)
        .components()
        .map(|component| match component {
            Component::Normal(part) => Ok(part.to_os_string()),
            _ => Err(BundleError::Invalid("unsafe path component".into())),
        })
        .collect::<Result<_, _>>()?;
    let (leaf, parents) = parts
        .split_last()
        .ok_or_else(|| BundleError::Invalid("input path has no leaf".into()))?;
    let mut directory = open_directory_chain(root)?;
    for parent in parents {
        directory = open_dir_at(&directory, parent)?;
    }
    before_leaf();
    let file = open_file_at(&directory, leaf)?;
    read_opened_file(file, private, forbid_shared_write)
}

#[cfg(not(unix))]
fn read_bounded_regular_portable(
    path: &Path,
    _private: bool,
    _forbid_shared_write: bool,
) -> Result<Vec<u8>, BundleError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| BundleError::Io(e.to_string()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(BundleError::Invalid(
            "input must be a regular non-symlink file".into(),
        ));
    }
    if metadata.len() > BUNDLE_INPUT_MAX as u64 {
        return Err(BundleError::SizeLimit);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| BundleError::Io(e.to_string()))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::io::Read::by_ref(&mut file)
        .take((BUNDLE_INPUT_MAX + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| BundleError::Io(e.to_string()))?;
    if bytes.len() > BUNDLE_INPUT_MAX {
        return Err(BundleError::SizeLimit);
    }
    let after = std::fs::symlink_metadata(path).map_err(|e| BundleError::Io(e.to_string()))?;
    if !after.is_file() || after.len() != metadata.len() {
        return Err(BundleError::Invalid("input changed while reading".into()));
    }
    Ok(bytes)
}

fn read_key(path: &Path, private: bool) -> Result<[u8; 32], BundleError> {
    let bytes = read_bounded_regular(path, private, true)?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| BundleError::Crypto("key is not UTF-8".into()))?;
    let mut lines = text.lines();
    let line = lines
        .next()
        .ok_or_else(|| BundleError::Crypto("empty key file".into()))?;
    if line.trim() != line || lines.any(|line| !line.trim().is_empty()) {
        return Err(BundleError::Crypto(
            "key file must contain exactly one base64 line".into(),
        ));
    }
    let decoded = BASE64
        .decode(line)
        .map_err(|_| BundleError::Crypto("invalid key encoding".into()))?;
    decoded
        .try_into()
        .map_err(|_| BundleError::Crypto("Ed25519 key must be 32 bytes".into()))
}

fn from_json_strict<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, BundleError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = NoDuplicates
        .deserialize(&mut deserializer)
        .map_err(|e| BundleError::Json(e.to_string()))?;
    deserializer
        .end()
        .map_err(|e| BundleError::Json(e.to_string()))?;
    serde_json::from_value(value).map_err(|e| BundleError::Json(e.to_string()))
}

struct NoDuplicates;

impl<'de> DeserializeSeed<'de> for NoDuplicates {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(NoDuplicateVisitor)
    }
}

struct NoDuplicateVisitor;

impl<'de> Visitor<'de> for NoDuplicateVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("strict JSON")
    }

    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: serde::de::Error>(self, _value: f64) -> Result<Value, E> {
        Err(E::custom("floating-point numbers are forbidden"))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_string()))
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        NoDuplicates.deserialize(d)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(NoDuplicates)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(A::Error::custom(format!("duplicate key: {key}")));
            }
            values.insert(key, map.next_value_seed(NoDuplicates)?);
        }
        Ok(Value::Object(values))
    }
}

fn valid_repository_identity(value: &str) -> bool {
    let mut parts = value.split('/');
    matches!((parts.next(), parts.next(), parts.next()), (Some(owner), Some(repo), None)
        if !owner.is_empty() && !repo.is_empty()
            && owner.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && repo.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
}

fn full_oid(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum TestGitObservationTarget {
    BinaryDiff,
    NameStatus,
}

#[cfg(test)]
struct TestGitObservation {
    target: TestGitObservationTarget,
    replacement: Vec<u8>,
    observations: usize,
    executed: bool,
}

#[cfg(test)]
thread_local! {
    static TEST_GIT_OBSERVATION: std::cell::RefCell<Option<TestGitObservation>> = const { std::cell::RefCell::new(None) };
}

fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>, BundleError> {
    let output = crate::diff::run_bounded_git_with_limit(root, args, None, BUNDLE_INPUT_MAX)
        .map_err(|error| BundleError::Invalid(format!("audit_git_{}", error.code())))?;
    if !output.success {
        return Err(BundleError::Invalid("audit_git_failed".into()));
    }
    #[cfg(test)]
    {
        let target = if args.contains(&"--binary") {
            Some(TestGitObservationTarget::BinaryDiff)
        } else if args.contains(&"--name-status") {
            Some(TestGitObservationTarget::NameStatus)
        } else {
            None
        };
        if let Some(target) = target {
            if let Some(replacement) = TEST_GIT_OBSERVATION.with(|slot| {
                let mut slot = slot.borrow_mut();
                let observation = slot.as_mut()?;
                if observation.target != target {
                    return None;
                }
                observation.observations += 1;
                if observation.observations == 2 {
                    observation.executed = true;
                    return Some(observation.replacement.clone());
                }
                None
            }) {
                return Ok(replacement);
            }
        }
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::{tempdir, TempDir};

    fn run_git(root: &Path, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .args(["-C", root.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn snapshot_fixture() -> (TempDir, PathBuf, Manifest) {
        let directory = tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        run_git(&root, &["init", "-q"]);
        run_git(&root, &["config", "user.name", "Audit Test"]);
        run_git(&root, &["config", "user.email", "audit@example.invalid"]);
        run_git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        );
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("spec.md"), "spec-v1").unwrap();
        std::fs::write(root.join("report.md"), "report-v1").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n").unwrap();
        run_git(&root, &["add", "."]);
        run_git(&root, &["commit", "-qm", "baseline"]);
        let baseline = String::from_utf8(run_git(&root, &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_string();
        run_git(&root, &["branch", "baseline-ref", &baseline]);

        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 2 }\n").unwrap();
        run_git(&root, &["add", "src/lib.rs"]);
        run_git(&root, &["commit", "-qm", "alternate diff baseline"]);
        let alternate_diff = String::from_utf8(run_git(&root, &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_string();
        run_git(&root, &["branch", "alternate-diff", &alternate_diff]);

        std::fs::write(root.join("transient.txt"), "transient").unwrap();
        run_git(&root, &["add", "transient.txt"]);
        run_git(&root, &["commit", "-qm", "alternate name status baseline"]);
        let alternate_status = String::from_utf8(run_git(&root, &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_string();
        run_git(&root, &["branch", "alternate-status", &alternate_status]);

        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 3 }\n").unwrap();
        std::fs::remove_file(root.join("transient.txt")).unwrap();
        run_git(&root, &["add", "-A"]);
        run_git(&root, &["commit", "-qm", "head"]);
        let head = String::from_utf8(run_git(&root, &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_string();
        let range = format!("{baseline}..{head}");
        let binary_diff = git_output(
            &root,
            &[
                "-c",
                "diff.external=",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--binary",
                &range,
                "--",
            ],
        )
        .unwrap();
        let mut name_status = parse_name_status(
            &git_output(
                &root,
                &[
                    "-c",
                    "diff.external=",
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                    "--name-status",
                    "-z",
                    &range,
                    "--",
                ],
            )
            .unwrap(),
        )
        .unwrap();
        name_status.sort_by(|a, b| {
            (&a.path, &a.old_path, &a.status).cmp(&(&b.path, &b.old_path, &b.status))
        });
        let mut value = manifest();
        value.repository = RepositoryBinding {
            identity: Some("owner/repo".into()),
            baseline_oid: baseline,
            head_oid: head,
            worktree_clean: true,
        };
        value.inputs = InputBinding {
            spec_path: "spec.md".into(),
            spec_sha256: format!("sha256:{}", sha256_hex(b"spec-v1")),
            executor_report_path: Some("report.md".into()),
            executor_report_present: true,
            executor_report_sha256: Some(format!("sha256:{}", sha256_hex(b"report-v1"))),
        };
        value.diff = DiffBinding {
            name_status,
            binary_diff_sha256: format!("sha256:{}", sha256_hex(&binary_diff)),
        };
        value
            .audit_configuration
            .insert("baseline_input".into(), serde_json::json!("baseline-ref"));
        value
            .audit_configuration
            .insert("require_clean_worktree".into(), serde_json::json!(true));
        value
            .index_metadata
            .insert("source".into(), serde_json::json!("mmcg"));
        (directory, root, value)
    }

    fn snapshot_policy(root: &Path, manifest: &Manifest) -> VerifyPolicy {
        VerifyPolicy {
            expected_repository: manifest.repository.identity.clone(),
            expected_baseline: Some(manifest.repository.baseline_oid.clone()),
            expected_head: Some(manifest.repository.head_oid.clone()),
            root: Some(root.to_path_buf()),
            require_clean_worktree: true,
            ..VerifyPolicy::default()
        }
    }

    fn with_second_git_observation<R>(
        target: TestGitObservationTarget,
        replacement: Vec<u8>,
        action: impl FnOnce() -> R,
    ) -> (R, bool, usize) {
        TEST_GIT_OBSERVATION.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(TestGitObservation {
                target,
                replacement,
                observations: 0,
                executed: false,
            });
        });
        let result = action();
        let observation = TEST_GIT_OBSERVATION
            .with(|slot| slot.borrow_mut().take())
            .unwrap();
        (result, observation.executed, observation.observations)
    }

    fn manifest() -> Manifest {
        Manifest {
            repository: RepositoryBinding {
                identity: Some("owner/repo".into()),
                baseline_oid: "1".repeat(40),
                head_oid: "2".repeat(40),
                worktree_clean: true,
            },
            inputs: InputBinding {
                spec_path: "spec.md".into(),
                spec_sha256: format!("sha256:{}", sha256_hex(b"spec")),
                executor_report_path: None,
                executor_report_present: false,
                executor_report_sha256: None,
            },
            diff: DiffBinding {
                name_status: Vec::new(),
                binary_diff_sha256: format!("sha256:{}", sha256_hex(b"")),
            },
            tool: ToolBinding {
                name: "mastermind".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                bundle_schema: 3,
            },
            audit_configuration: BTreeMap::new(),
            index_metadata: BTreeMap::new(),
            verdict: "held".into(),
            declared_files: vec!["src/lib.rs".into()],
            changed_files: vec![],
            verified_claims: vec![],
            failed_claims: vec![],
            discrepancies: vec![],
            snapshot_drift: vec![],
            snapshot_changed: false,
            mmcg_queries: vec![],
            verify_commands: vec![],
            human_summary: "held".into(),
        }
    }

    #[test]
    fn canonical_json_is_stable_and_rejects_non_integer_numbers() {
        assert_eq!(
            canonical_json(&serde_json::json!({"z":1,"é":2,"a":"\n"})).unwrap(),
            r#"{"a":"\n","z":1,"é":2}"#.as_bytes()
        );
        assert!(canonical_json(&serde_json::json!(1.5)).is_err());
    }

    #[test]
    fn every_manifest_field_is_bound_by_integrity_digest() {
        let envelope = seal(manifest()).unwrap();
        let mut altered = envelope.clone();
        altered.manifest.human_summary.push('!');
        assert_eq!(
            verify_envelope(
                &altered,
                &VerifyPolicy {
                    integrity_only: true,
                    ..Default::default()
                }
            )
            .content_integrity,
            "fail"
        );
    }

    #[test]
    fn verify_detects_reordered_but_equivalent_json_as_valid() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("bundle.json");
        let envelope = seal(manifest()).unwrap();
        let value = serde_json::to_value(&envelope).unwrap();
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        let read = read_envelope(&path).unwrap();
        assert_eq!(
            verify_envelope(
                &read,
                &VerifyPolicy {
                    integrity_only: true,
                    ..Default::default()
                }
            )
            .content_integrity,
            "pass"
        );
    }

    #[test]
    fn verify_detects_tampered_manifest_or_digest() {
        let mut envelope = seal(manifest()).unwrap();
        envelope.integrity.manifest_digest = format!("sha256:{}", "0".repeat(64));
        assert_eq!(
            verify_envelope(
                &envelope,
                &VerifyPolicy {
                    integrity_only: true,
                    ..Default::default()
                }
            )
            .content_integrity,
            "fail"
        );
    }

    #[test]
    fn verify_recomputes_spec_report_diff_and_full_object_ids() {
        let (_directory, root, value) = snapshot_fixture();
        let envelope = seal_checked(value.clone(), &root).unwrap();
        let report = verify_envelope(&envelope, &snapshot_policy(&root, &value));
        assert!(report.accepted(), "{:?}", report.reasons);
        assert_eq!(
            value.inputs.spec_sha256,
            format!(
                "sha256:{}",
                sha256_hex(&std::fs::read(root.join("spec.md")).unwrap())
            )
        );
        let report_digest = format!(
            "sha256:{}",
            sha256_hex(&std::fs::read(root.join("report.md")).unwrap())
        );
        assert_eq!(
            value.inputs.executor_report_sha256.as_deref(),
            Some(report_digest.as_str())
        );

        type ManifestMutation = Box<dyn Fn(&mut Manifest)>;
        let mutations: Vec<(&str, ManifestMutation)> = vec![
            (
                "baseline",
                Box::new(|manifest| manifest.repository.baseline_oid = "0".repeat(40)),
            ),
            (
                "head",
                Box::new(|manifest| manifest.repository.head_oid = "f".repeat(40)),
            ),
            (
                "clean",
                Box::new(|manifest| manifest.repository.worktree_clean = false),
            ),
            (
                "spec",
                Box::new(|manifest| {
                    manifest.inputs.spec_sha256 = format!("sha256:{}", "0".repeat(64))
                }),
            ),
            (
                "report",
                Box::new(|manifest| {
                    manifest.inputs.executor_report_sha256 =
                        Some(format!("sha256:{}", "0".repeat(64)));
                }),
            ),
        ];
        for (label, mutate) in mutations {
            let mut changed = value.clone();
            mutate(&mut changed);
            let changed_envelope = seal(changed.clone()).unwrap();
            let report = verify_envelope(&changed_envelope, &snapshot_policy(&root, &changed));
            assert!(!report.accepted(), "mutation accepted: {label}");
        }

        let mut changed = value.clone();
        changed.diff.binary_diff_sha256 = format!("sha256:{}", "0".repeat(64));
        let report = verify_envelope(
            &seal(changed.clone()).unwrap(),
            &snapshot_policy(&root, &changed),
        );
        assert_eq!(report.reasons, vec!["diff_digest_mismatch"]);

        let mut changed = value.clone();
        changed.diff.name_status.push(DiffEntry {
            status: "A".into(),
            path: "forged.txt".into(),
            old_path: None,
        });
        let report = verify_envelope(
            &seal(changed.clone()).unwrap(),
            &snapshot_policy(&root, &changed),
        );
        assert_eq!(report.reasons, vec!["name_status_mismatch"]);
    }

    #[test]
    fn producer_rechecks_binary_diff_and_name_status_independently() {
        let (_directory, root, value) = snapshot_fixture();
        let (result, executed, observations) = with_second_git_observation(
            TestGitObservationTarget::BinaryDiff,
            b"forged binary diff".to_vec(),
            || seal_checked(value, &root),
        );
        assert!(executed);
        assert_eq!(observations, 2);
        assert!(matches!(
            result,
            Err(BundleError::Invalid(reason)) if reason == "snapshot_changed"
        ));

        let (_directory, root, value) = snapshot_fixture();
        let (result, executed, observations) = with_second_git_observation(
            TestGitObservationTarget::NameStatus,
            b"M\0src/lib.rs\0A\0forged.txt\0".to_vec(),
            || seal_checked(value, &root),
        );
        assert!(executed);
        assert_eq!(observations, 2);
        assert!(matches!(
            result,
            Err(BundleError::Invalid(reason)) if reason == "snapshot_changed"
        ));
    }

    #[test]
    fn producer_rechecks_every_coherent_snapshot_input_before_seal() {
        let (_directory, root, value) = snapshot_fixture();
        assert!(seal_checked_with_hook(value, &root, || {
            run_git(&root, &["branch", "-f", "baseline-ref", "alternate-diff"]);
        })
        .is_err());

        let (_directory, root, value) = snapshot_fixture();
        assert!(seal_checked_with_hook(value, &root, || {
            std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 4 }\n").unwrap();
            run_git(&root, &["add", "src/lib.rs"]);
            run_git(&root, &["commit", "-qm", "moved head"]);
        })
        .is_err());

        let (_directory, root, value) = snapshot_fixture();
        assert!(seal_checked_with_hook(value, &root, || {
            std::fs::write(root.join("dirty.txt"), "dirty").unwrap();
        })
        .is_err());

        let (_directory, root, value) = snapshot_fixture();
        run_git(&root, &["update-index", "--assume-unchanged", "spec.md"]);
        assert!(seal_checked_with_hook(value, &root, || {
            std::fs::write(root.join("spec.md"), "spec-v2").unwrap();
        })
        .is_err());

        let (_directory, root, value) = snapshot_fixture();
        run_git(&root, &["update-index", "--assume-unchanged", "report.md"]);
        assert!(seal_checked_with_hook(value, &root, || {
            std::fs::write(root.join("report.md"), "report-v2").unwrap();
        })
        .is_err());

        let (_directory, root, value) = snapshot_fixture();
        assert!(seal_checked_with_hook(value, &root, || {
            run_git(&root, &["branch", "-f", "baseline-ref", "alternate-diff"]);
        })
        .is_err());

        let (_directory, root, value) = snapshot_fixture();
        assert!(seal_checked_with_hook(value, &root, || {
            run_git(&root, &["branch", "-f", "baseline-ref", "alternate-status"]);
        })
        .is_err());
    }

    #[test]
    fn repository_identity_rejects_credentials_query_and_fragment() {
        assert_eq!(
            normalize_repository_identity("https://github.com/owner/repo.git").unwrap(),
            "owner/repo"
        );
        assert!(normalize_repository_identity("https://token@github.com/owner/repo").is_err());
        assert!(normalize_repository_identity("https://github.com/owner/repo?token=x").is_err());
    }

    #[test]
    fn detached_ed25519_signature_round_trip_and_wrong_key_failure() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let private = root.join("private.key");
        let public = root.join("public.key");
        let wrong = root.join("wrong.key");
        let seed = [7_u8; 32];
        let signing = SigningKey::from_bytes(&seed);
        std::fs::write(&private, format!("{}\n", BASE64.encode(seed))).unwrap();
        std::fs::write(
            &public,
            format!("{}\n", BASE64.encode(signing.verifying_key().to_bytes())),
        )
        .unwrap();
        std::fs::write(&wrong, format!("{}\n", BASE64.encode([9_u8; 32]))).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let envelope = seal(manifest()).unwrap();
        let signature = sign_detached(&envelope, &private).unwrap();
        verify_detached(&envelope, &signature, &public).unwrap();
        assert!(verify_detached(&envelope, &signature, &wrong).is_err());
    }

    #[test]
    fn verification_report_separates_integrity_authenticity_and_policy() {
        let report = verify_envelope(&seal(manifest()).unwrap(), &VerifyPolicy::default());
        assert_eq!(report.content_integrity, "pass");
        assert_eq!(report.provenance_authenticity, "not_evaluated");
        assert_eq!(report.policy_acceptance, "fail");
        assert_eq!(report.reasons, vec!["no_trust_anchor"]);
    }

    #[test]
    fn bundle_inputs_are_size_bounded() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("large");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len((BUNDLE_INPUT_MAX + 1) as u64).unwrap();
        assert!(matches!(
            read_bounded_regular(&path, false, false),
            Err(BundleError::SizeLimit)
        ));
    }

    #[test]
    fn strict_json_rejects_duplicate_unknown_and_float_fields() {
        let envelope = seal(manifest()).unwrap();
        let raw = serde_json::to_string(&envelope).unwrap();
        let duplicate = raw.replacen("{", "{\"schema_version\":3,", 1);
        assert!(from_json_strict::<Envelope>(duplicate.as_bytes()).is_err());
        let unknown = raw.replacen("{", "{\"unknown\":true,", 1);
        assert!(from_json_strict::<Envelope>(unknown.as_bytes()).is_err());
    }

    #[test]
    fn signature_replay_across_protocol_fails() {
        let mut envelope = seal(manifest()).unwrap();
        envelope.integrity.canonicalization = "other".into();
        let dir = tempdir().unwrap();
        let public = dir.path().join("public.key");
        std::fs::write(&public, format!("{}\n", BASE64.encode([1_u8; 32]))).unwrap();
        let signature = DetachedSignature {
            schema_version: 1,
            algorithm: "ed25519".into(),
            key_id: format!("sha256:{}", sha256_hex(&[1_u8; 32])),
            manifest_digest: envelope.integrity.manifest_digest.clone(),
            signature: BASE64.encode([0_u8; 64]),
        };
        assert!(verify_detached(&envelope, &signature, &public).is_err());
    }

    #[test]
    fn every_envelope_section_mutation_is_rejected() {
        let envelope = seal(manifest()).unwrap();
        type EnvelopeMutation = Box<dyn Fn(&mut Envelope)>;
        let mutations: Vec<EnvelopeMutation> = vec![
            Box::new(|e| e.manifest.repository.worktree_clean = false),
            Box::new(|e| e.manifest.inputs.spec_path = "other.md".into()),
            Box::new(|e| e.manifest.diff.binary_diff_sha256 = "sha256:bad".into()),
            Box::new(|e| e.manifest.tool.version = "other".into()),
            Box::new(|e| {
                e.manifest
                    .audit_configuration
                    .insert("x".into(), serde_json::json!(1));
            }),
            Box::new(|e| {
                e.manifest
                    .index_metadata
                    .insert("x".into(), serde_json::json!(1));
            }),
            Box::new(|e| e.manifest.verdict = "broken".into()),
            Box::new(|e| e.manifest.declared_files.push("other".into())),
            Box::new(|e| e.manifest.changed_files.push("other".into())),
            Box::new(|e| e.manifest.verified_claims.push("other".into())),
            Box::new(|e| e.manifest.failed_claims.push("other".into())),
            Box::new(|e| e.manifest.discrepancies.push(serde_json::json!({"x":1}))),
            Box::new(|e| e.manifest.snapshot_drift.push(serde_json::json!({"x":1}))),
            Box::new(|e| e.manifest.snapshot_changed = true),
            Box::new(|e| e.manifest.mmcg_queries.push("other".into())),
            Box::new(|e| e.manifest.verify_commands.push("other".into())),
            Box::new(|e| e.manifest.human_summary = "other".into()),
        ];
        for mutate in mutations {
            let mut changed = envelope.clone();
            mutate(&mut changed);
            assert_eq!(
                verify_envelope(
                    &changed,
                    &VerifyPolicy {
                        integrity_only: true,
                        ..VerifyPolicy::default()
                    }
                )
                .content_integrity,
                "fail"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_symlinks_and_unsafe_private_key_modes() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        std::fs::write(&target, "value").unwrap();
        symlink(&target, &link).unwrap();
        assert!(read_bounded_regular(&link, false, false).is_err());
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_bounded_regular(&target, true, true).is_err());
    }

    #[test]
    fn snapshot_anchor_never_masks_partial_signature_policy() {
        let envelope = seal(manifest()).unwrap();
        let signature = DetachedSignature {
            schema_version: 1,
            algorithm: "ed25519".into(),
            key_id: format!("sha256:{}", "a".repeat(64)),
            manifest_digest: envelope.integrity.manifest_digest.clone(),
            signature: BASE64.encode([0_u8; 64]),
        };
        let snapshot = || VerifyPolicy {
            expected_repository: Some("owner/repo".into()),
            expected_baseline: Some("1".repeat(40)),
            expected_head: Some("2".repeat(40)),
            root: Some(PathBuf::from(".")),
            require_clean_worktree: true,
            ..VerifyPolicy::default()
        };
        let mut policies = Vec::new();
        let mut value = snapshot();
        value.signature = Some(signature.clone());
        policies.push(value);
        let mut value = snapshot();
        value.public_key = Some(PathBuf::from("public.key"));
        policies.push(value);
        let mut value = snapshot();
        value.trusted_key_ids.insert(signature.key_id.clone());
        policies.push(value);
        let mut value = snapshot();
        value.revoked_key_ids.insert(signature.key_id.clone());
        policies.push(value);
        let mut value = snapshot();
        value.require_signature = true;
        policies.push(value);
        let mut value = snapshot();
        value.signature = Some(signature.clone());
        value.public_key = Some(PathBuf::from("public.key"));
        policies.push(value);
        for policy in policies {
            let report = verify_envelope(&envelope, &policy);
            assert_eq!(report.policy_acceptance, "fail");
            assert!(report
                .reasons
                .contains(&"incomplete_signature_policy".into()));
        }
    }

    #[test]
    fn integrity_valid_snapshot_changed_envelope_is_rejected() {
        let mut changed = manifest();
        changed.snapshot_changed = true;
        let report = verify_envelope(
            &seal(changed).unwrap(),
            &VerifyPolicy {
                integrity_only: true,
                ..VerifyPolicy::default()
            },
        );
        assert_eq!(report.content_integrity, "fail");
        assert_eq!(report.reasons, vec!["snapshot_changed"]);
    }

    #[test]
    fn output_helper_accepts_nested_path_and_rejects_dot_escape_and_symlink_parent() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let output = create_contained_output_dir(&root, Path::new("nested/bundles")).unwrap();
        assert!(output.is_dir());
        assert!(create_contained_output_dir(&root, Path::new("nested/bundles")).is_err());
        assert!(create_contained_output_dir(&root, Path::new(".")).is_err());
        assert!(create_contained_output_dir(&root, Path::new("../escape")).is_err());
        std::fs::create_dir(root.join("empty")).unwrap();
        assert!(create_contained_output_dir(&root, Path::new("empty")).is_err());
        std::fs::create_dir(root.join("prepopulated")).unwrap();
        std::fs::write(root.join("prepopulated/stale.bundle.json"), "stale").unwrap();
        assert!(create_contained_output_dir(&root, Path::new("prepopulated")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&root, root.join("link")).unwrap();
            assert!(create_contained_output_dir(&root, Path::new("link/out")).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_relative_reads_and_outputs_resist_parent_swaps() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("root");
        let parent = root.join("parent");
        let moved = root.join("moved");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::write(parent.join("value"), "inside").unwrap();
        let bytes =
            read_bounded_relative_unix(&root, Path::new("parent/value"), false, false, || {
                std::fs::rename(&parent, &moved).unwrap();
                std::fs::create_dir(&parent).unwrap();
                std::fs::write(parent.join("value"), "outside").unwrap();
            })
            .unwrap();
        assert_eq!(bytes, b"inside");

        let absolute_parent = root.join("absolute-parent");
        let moved_absolute_parent = root.join("moved-absolute-parent");
        std::fs::create_dir(&absolute_parent).unwrap();
        let absolute_leaf = absolute_parent.join("value");
        std::fs::write(&absolute_leaf, "absolute-inside").unwrap();
        let bytes = read_bounded_regular_unix(&absolute_leaf, false, false, || {
            std::fs::rename(&absolute_parent, &moved_absolute_parent).unwrap();
            std::fs::create_dir(&absolute_parent).unwrap();
            std::fs::write(absolute_parent.join("value"), "absolute-outside").unwrap();
        })
        .unwrap();
        assert_eq!(bytes, b"absolute-inside");

        let output_parent = root.join("output-parent");
        let moved_output_parent = root.join("moved-output-parent");
        std::fs::create_dir(&output_parent).unwrap();
        let result =
            create_contained_output_dir_unix(&root, Path::new("output-parent/final"), || {
                std::fs::rename(&output_parent, &moved_output_parent).unwrap();
                std::fs::create_dir(&output_parent).unwrap();
            });
        assert!(result.is_err());
        assert!(!output_parent.join("final").exists());
        assert!(moved_output_parent.join("final").is_dir());
    }
}
