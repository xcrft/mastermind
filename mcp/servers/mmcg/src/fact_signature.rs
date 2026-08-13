//! Domain-separated Ed25519 trust for declarative fact manifests.
//!
//! The signature authenticates the canonical `mastermind-facts/v1` document,
//! which already binds repository identity, revision, sources, and provenance
//! artifacts. It proves control of an explicitly trusted key only; callers
//! remain responsible for key ownership, rotation, and revocation policy.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const SIGNATURE_DOMAIN: &str = "mastermind/fact-manifest-signature/v1";
pub const SIGNATURE_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FactSignature {
    pub schema_version: u32,
    pub domain: String,
    pub algorithm: String,
    pub canonicalization: String,
    pub key_id: String,
    pub manifest_digest: String,
    pub signature: String,
}

#[derive(Debug, Clone, Default)]
pub struct FactTrustPolicy {
    pub signature: Option<PathBuf>,
    pub public_key: Option<PathBuf>,
    pub require_signature: bool,
    pub trusted_key_ids: BTreeSet<String>,
    pub revoked_key_ids: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct VerifiedFactTrust {
    pub signature_status: &'static str,
    pub signing_key_id: Option<String>,
    pub signature_sha256: Option<String>,
    pub signature_bytes: Option<u64>,
    pub signing_public_key: Option<String>,
    pub signature_value: Option<String>,
    pub manifest_digest: String,
}

pub(crate) fn verify_stored_proof(
    manifest_digest: &str,
    key_id: &str,
    public_key: &str,
    signature: &str,
) -> Result<(), FactSignatureError> {
    if !validate_key_id(key_id) || !validate_key_id(manifest_digest) {
        return Err(FactSignatureError::Crypto(
            "invalid stored signature digest or key ID".into(),
        ));
    }
    let public = BASE64
        .decode(public_key)
        .map_err(|_| FactSignatureError::Crypto("invalid stored public key encoding".into()))?;
    let public: [u8; 32] = public
        .try_into()
        .map_err(|_| FactSignatureError::Crypto("invalid stored public key length".into()))?;
    let expected_key_id = format!(
        "sha256:{}",
        crate::audit_bundle::sha256_hex(public.as_slice())
    );
    if expected_key_id != key_id {
        return Err(FactSignatureError::Crypto(
            "stored public key ID mismatch".into(),
        ));
    }
    let signature = BASE64
        .decode(signature)
        .map_err(|_| FactSignatureError::Crypto("invalid stored signature encoding".into()))?;
    let signature = Signature::from_slice(&signature)
        .map_err(|_| FactSignatureError::Crypto("invalid stored signature length".into()))?;
    let verifying = VerifyingKey::from_bytes(&public)
        .map_err(|_| FactSignatureError::Crypto("invalid stored public key".into()))?;
    verifying
        .verify(&signature_statement(key_id, manifest_digest)?, &signature)
        .map_err(|_| FactSignatureError::Crypto("stored signature verification failed".into()))
}

#[derive(Debug, Clone, Serialize)]
pub struct FactVerificationReport {
    pub schema_version: u32,
    pub content_integrity: &'static str,
    pub provenance_authenticity: &'static str,
    pub policy_acceptance: &'static str,
    pub manifest_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FactKeygenSummary {
    pub schema_version: u32,
    pub algorithm: &'static str,
    pub private_key: String,
    pub public_key: String,
    pub key_id: String,
}

#[derive(Debug)]
pub enum FactSignatureError {
    Input(String),
    Contract(String),
    Crypto(String),
}

impl fmt::Display for FactSignatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(message) => write!(formatter, "fact signature input error: {message}"),
            Self::Contract(message) => write!(formatter, "fact signature policy error: {message}"),
            Self::Crypto(message) => {
                write!(formatter, "fact signature verification error: {message}")
            }
        }
    }
}

impl std::error::Error for FactSignatureError {}

fn input_error(error: crate::audit_bundle::BundleError) -> FactSignatureError {
    FactSignatureError::Input(error.to_string())
}

fn validate_key_id(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn manifest_digest(bytes: &[u8]) -> Result<String, FactSignatureError> {
    crate::facts::validate_manifest_document(bytes)
        .map_err(|error| FactSignatureError::Contract(error.to_string()))?;
    let value: Value = crate::audit_bundle::from_json_strict(bytes).map_err(input_error)?;
    let canonical = crate::audit_bundle::canonical_json(&value).map_err(input_error)?;
    Ok(format!(
        "sha256:{}",
        crate::audit_bundle::sha256_hex(&canonical)
    ))
}

fn signature_statement(key_id: &str, manifest_digest: &str) -> Result<Vec<u8>, FactSignatureError> {
    crate::audit_bundle::canonical_json(&serde_json::json!({
        "canonicalization": crate::audit_bundle::CANONICALIZATION,
        "domain": SIGNATURE_DOMAIN,
        "hash_algorithm": "sha256",
        "key_id": key_id,
        "manifest_api_version": crate::facts::API_VERSION,
        "manifest_digest": manifest_digest,
        "signature_algorithm": "ed25519",
        "signature_schema": SIGNATURE_SCHEMA
    }))
    .map_err(input_error)
}

fn output_identity(path: &Path) -> Result<PathBuf, FactSignatureError> {
    let absolute = std::path::absolute(path)
        .map_err(|error| FactSignatureError::Input(format!("resolve signature output: {error}")))?;
    let parent = absolute
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .map_err(|error| {
            FactSignatureError::Input(format!("resolve signature output directory: {error}"))
        })?;
    let name = absolute
        .file_name()
        .ok_or_else(|| FactSignatureError::Contract("signature output must name a file".into()))?;
    Ok(parent.join(name))
}

fn require_new_output(path: &Path, label: &str) -> Result<(), FactSignatureError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(FactSignatureError::Contract(format!(
            "{label} output already exists"
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(FactSignatureError::Input(format!(
            "inspect {label} output: {error}"
        ))),
    }
}

fn write_new_key(path: &Path, bytes: &[u8], private: bool) -> Result<(), FactSignatureError> {
    #[cfg(not(unix))]
    let _ = private;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if private { 0o600 } else { 0o644 });
    }
    let mut file = options
        .open(path)
        .map_err(|error| FactSignatureError::Input(format!("create key output: {error}")))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| FactSignatureError::Input(format!("write key output: {error}")))
}

pub fn generate_keypair(
    private_key_path: &Path,
    public_key_path: &Path,
) -> Result<FactKeygenSummary, FactSignatureError> {
    let private_key_path = output_identity(private_key_path)?;
    let public_key_path = output_identity(public_key_path)?;
    if private_key_path == public_key_path {
        return Err(FactSignatureError::Contract(
            "private-key and public-key outputs must be different files".into(),
        ));
    }
    require_new_output(&private_key_path, "private-key")?;
    require_new_output(&public_key_path, "public-key")?;

    let mut seed = [0_u8; 32];
    getrandom::fill(&mut seed).map_err(|error| {
        FactSignatureError::Crypto(format!("operating-system randomness unavailable: {error}"))
    })?;
    let signing = SigningKey::from_bytes(&seed);
    seed.fill(0);
    let public = signing.verifying_key().to_bytes();
    let private_bytes = format!("{}\n", BASE64.encode(signing.to_bytes()));
    let public_bytes = format!("{}\n", BASE64.encode(public));
    write_new_key(&private_key_path, private_bytes.as_bytes(), true)?;
    write_new_key(&public_key_path, public_bytes.as_bytes(), false)?;
    Ok(FactKeygenSummary {
        schema_version: 1,
        algorithm: "ed25519",
        private_key: private_key_path.to_string_lossy().into_owned(),
        public_key: public_key_path.to_string_lossy().into_owned(),
        key_id: format!(
            "sha256:{}",
            crate::audit_bundle::sha256_hex(public.as_slice())
        ),
    })
}

pub fn sign(
    manifest_path: &Path,
    private_key_path: &Path,
    signature_path: &Path,
) -> Result<FactSignature, FactSignatureError> {
    let signature_identity = output_identity(signature_path)?;
    for (label, input) in [
        ("fact manifest", manifest_path),
        ("private key", private_key_path),
    ] {
        let input = input
            .canonicalize()
            .map_err(|error| FactSignatureError::Input(format!("resolve {label}: {error}")))?;
        if signature_identity == input {
            return Err(FactSignatureError::Contract(format!(
                "signature output must not overwrite the {label}"
            )));
        }
    }
    let bytes = crate::audit_bundle::read_bounded_regular(manifest_path, false, true)
        .map_err(input_error)?;
    let manifest_digest = manifest_digest(&bytes)?;
    let seed = crate::audit_bundle::read_key(private_key_path, true).map_err(input_error)?;
    let signing = SigningKey::from_bytes(&seed);
    let public = signing.verifying_key().to_bytes();
    let key_id = format!(
        "sha256:{}",
        crate::audit_bundle::sha256_hex(public.as_slice())
    );
    let statement = signature_statement(&key_id, &manifest_digest)?;
    let signature = FactSignature {
        schema_version: SIGNATURE_SCHEMA,
        domain: SIGNATURE_DOMAIN.into(),
        algorithm: "ed25519".into(),
        canonicalization: crate::audit_bundle::CANONICALIZATION.into(),
        key_id,
        manifest_digest,
        signature: BASE64.encode(signing.sign(&statement).to_bytes()),
    };
    let output = serde_json::to_vec_pretty(&signature)
        .map_err(|error| FactSignatureError::Input(error.to_string()))?;
    crate::audit_bundle::write_atomic(signature_path, &output, true).map_err(input_error)?;
    Ok(signature)
}

pub(crate) fn verify_bytes(
    manifest_bytes: &[u8],
    policy: &FactTrustPolicy,
) -> Result<VerifiedFactTrust, FactSignatureError> {
    let digest = manifest_digest(manifest_bytes)?;
    let signature_requested = policy.signature.is_some()
        || policy.public_key.is_some()
        || policy.require_signature
        || !policy.trusted_key_ids.is_empty()
        || !policy.revoked_key_ids.is_empty();
    if !signature_requested {
        return Ok(VerifiedFactTrust {
            signature_status: "unsigned",
            signing_key_id: None,
            signature_sha256: None,
            signature_bytes: None,
            signing_public_key: None,
            signature_value: None,
            manifest_digest: digest,
        });
    }
    let (Some(signature_path), Some(public_key_path)) =
        (policy.signature.as_deref(), policy.public_key.as_deref())
    else {
        return Err(FactSignatureError::Contract(
            "signature, public key, and trusted key ID are required together".into(),
        ));
    };
    if policy.trusted_key_ids.is_empty() {
        return Err(FactSignatureError::Contract(
            "at least one trusted key ID is required".into(),
        ));
    }
    if policy
        .trusted_key_ids
        .iter()
        .chain(policy.revoked_key_ids.iter())
        .any(|value| !validate_key_id(value))
    {
        return Err(FactSignatureError::Contract(
            "key IDs must use sha256:<64 lowercase hex>".into(),
        ));
    }
    let signature_bytes = crate::audit_bundle::read_bounded_regular(signature_path, false, true)
        .map_err(input_error)?;
    let signature: FactSignature =
        crate::audit_bundle::from_json_strict(&signature_bytes).map_err(input_error)?;
    if signature.schema_version != SIGNATURE_SCHEMA
        || signature.domain != SIGNATURE_DOMAIN
        || signature.algorithm != "ed25519"
        || signature.canonicalization != crate::audit_bundle::CANONICALIZATION
        || signature.manifest_digest != digest
        || !validate_key_id(&signature.key_id)
    {
        return Err(FactSignatureError::Crypto(
            "signature contract or manifest digest mismatch".into(),
        ));
    }
    if policy.revoked_key_ids.contains(&signature.key_id) {
        return Err(FactSignatureError::Crypto("signing key is revoked".into()));
    }
    if !policy.trusted_key_ids.contains(&signature.key_id) {
        return Err(FactSignatureError::Crypto(
            "signing key is not trusted".into(),
        ));
    }
    let public = crate::audit_bundle::read_key(public_key_path, false).map_err(input_error)?;
    let key_id = format!(
        "sha256:{}",
        crate::audit_bundle::sha256_hex(public.as_slice())
    );
    if signature.key_id != key_id {
        return Err(FactSignatureError::Crypto("public key ID mismatch".into()));
    }
    let verifying = VerifyingKey::from_bytes(&public)
        .map_err(|_| FactSignatureError::Crypto("invalid public key".into()))?;
    let signature_value = BASE64
        .decode(&signature.signature)
        .map_err(|_| FactSignatureError::Crypto("invalid signature encoding".into()))?;
    let parsed = Signature::from_slice(&signature_value)
        .map_err(|_| FactSignatureError::Crypto("invalid signature length".into()))?;
    let statement = signature_statement(&key_id, &digest)?;
    verifying
        .verify(&statement, &parsed)
        .map_err(|_| FactSignatureError::Crypto("signature verification failed".into()))?;
    Ok(VerifiedFactTrust {
        signature_status: "verified",
        signing_key_id: Some(key_id),
        signature_sha256: Some(crate::audit_bundle::sha256_hex(&signature_bytes)),
        signature_bytes: Some(signature_bytes.len() as u64),
        signing_public_key: Some(BASE64.encode(public)),
        signature_value: Some(signature.signature),
        manifest_digest: digest,
    })
}

pub fn verify(
    manifest_path: &Path,
    policy: &FactTrustPolicy,
) -> Result<FactVerificationReport, FactSignatureError> {
    let bytes = crate::audit_bundle::read_bounded_regular(manifest_path, false, true)
        .map_err(input_error)?;
    let trust = verify_bytes(&bytes, policy)?;
    Ok(FactVerificationReport {
        schema_version: 1,
        content_integrity: "pass",
        provenance_authenticity: if trust.signature_status == "verified" {
            "pass"
        } else {
            "not_evaluated"
        },
        policy_acceptance: if trust.signature_status == "verified" {
            "pass"
        } else {
            "not_evaluated"
        },
        manifest_digest: trust.manifest_digest,
        key_id: trust.signing_key_id,
        signature_sha256: trust.signature_sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn manifest() -> serde_json::Value {
        json!({
            "api_version": crate::facts::API_VERSION,
            "capabilities": ["annotations"],
            "repository": {
                "identity": format!("git-worktree:sha256:{}", "a".repeat(64)),
                "revision": "b".repeat(40),
            },
            "producer": {"name": "test", "version": "1"},
            "dataset": "default",
            "provenance": {"kind": "test", "artifacts": []},
            "files": [],
            "artifacts": [],
            "facts": [],
        })
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, String) {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let manifest_path = root.path().join("facts.json");
        let private_path = root.path().join("seed");
        let public_path = root.path().join("public");
        let seed = [13_u8; 32];
        let signing = SigningKey::from_bytes(&seed);
        let public = signing.verifying_key().to_bytes();
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest()).unwrap(),
        )
        .unwrap();
        fs::write(&private_path, format!("{}\n", BASE64.encode(seed))).unwrap();
        fs::write(&public_path, format!("{}\n", BASE64.encode(public))).unwrap();
        #[cfg(unix)]
        {
            fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o644)).unwrap();
            fs::set_permissions(&private_path, fs::Permissions::from_mode(0o600)).unwrap();
            fs::set_permissions(&public_path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        let key_id = format!(
            "sha256:{}",
            crate::audit_bundle::sha256_hex(public.as_slice())
        );
        (root, manifest_path, private_path, public_path, key_id)
    }

    fn policy(signature: PathBuf, public: PathBuf, key_id: String) -> FactTrustPolicy {
        FactTrustPolicy {
            signature: Some(signature),
            public_key: Some(public),
            require_signature: true,
            trusted_key_ids: [key_id].into_iter().collect(),
            revoked_key_ids: BTreeSet::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn generated_keypair_is_private_usable_and_never_overwritten() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("producer.seed");
        let public = root.path().join("producer.pub");
        let generated = generate_keypair(&private, &public).unwrap();
        assert_eq!(generated.algorithm, "ed25519");
        assert!(generated.key_id.starts_with("sha256:"));
        assert_eq!(
            fs::metadata(&private).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&public).unwrap().permissions().mode() & 0o022,
            0
        );
        assert_eq!(
            crate::audit_bundle::read_key(&private, true).unwrap().len(),
            32
        );
        assert_eq!(
            crate::audit_bundle::read_key(&public, false).unwrap().len(),
            32
        );

        let private_before = fs::read(&private).unwrap();
        let public_before = fs::read(&public).unwrap();
        assert!(generate_keypair(&private, &public).is_err());
        assert_eq!(fs::read(&private).unwrap(), private_before);
        assert_eq!(fs::read(&public).unwrap(), public_before);
    }

    #[test]
    fn trusted_signature_is_domain_separated_and_tamper_evident() {
        let (root, manifest_path, private_path, public_path, key_id) = fixture();
        let signature_path = root.path().join("facts.sig.json");
        let signed = sign(&manifest_path, &private_path, &signature_path).unwrap();
        assert_eq!(signed.domain, SIGNATURE_DOMAIN);
        assert_eq!(signed.key_id, key_id);
        assert_eq!(
            verify(
                &manifest_path,
                &policy(signature_path.clone(), public_path.clone(), key_id.clone())
            )
            .unwrap()
            .policy_acceptance,
            "pass"
        );

        let mut changed = manifest();
        changed["dataset"] = json!("tampered");
        fs::write(&manifest_path, serde_json::to_vec_pretty(&changed).unwrap()).unwrap();
        assert!(
            verify(&manifest_path, &policy(signature_path, public_path, key_id))
                .unwrap_err()
                .to_string()
                .contains("digest")
        );
    }

    #[test]
    fn signature_output_cannot_overwrite_the_manifest_or_private_key() {
        let (_root, manifest_path, private_path, _public_path, _key_id) = fixture();
        let manifest_before = fs::read(&manifest_path).unwrap();
        let private_before = fs::read(&private_path).unwrap();

        assert!(sign(&manifest_path, &private_path, &manifest_path)
            .unwrap_err()
            .to_string()
            .contains("manifest"));
        assert!(sign(&manifest_path, &private_path, &private_path)
            .unwrap_err()
            .to_string()
            .contains("private key"));
        assert_eq!(fs::read(&manifest_path).unwrap(), manifest_before);
        assert_eq!(fs::read(&private_path).unwrap(), private_before);
    }

    #[test]
    fn revoked_untrusted_and_cross_domain_signatures_fail_closed() {
        let (root, manifest_path, private_path, public_path, key_id) = fixture();
        let signature_path = root.path().join("facts.sig.json");
        sign(&manifest_path, &private_path, &signature_path).unwrap();

        let mut revoked = policy(signature_path.clone(), public_path.clone(), key_id.clone());
        revoked.revoked_key_ids.insert(key_id.clone());
        assert!(verify(&manifest_path, &revoked)
            .unwrap_err()
            .to_string()
            .contains("revoked"));

        let mut untrusted = policy(signature_path.clone(), public_path.clone(), key_id);
        untrusted.trusted_key_ids = [format!("sha256:{}", "c".repeat(64))].into_iter().collect();
        assert!(verify(&manifest_path, &untrusted)
            .unwrap_err()
            .to_string()
            .contains("not trusted"));

        let audit_signature = json!({
            "schema_version": 1,
            "algorithm": "ed25519",
            "key_id": format!("sha256:{}", "d".repeat(64)),
            "manifest_digest": format!("sha256:{}", "e".repeat(64)),
            "signature": BASE64.encode([0_u8; 64]),
        });
        fs::write(
            &signature_path,
            serde_json::to_vec_pretty(&audit_signature).unwrap(),
        )
        .unwrap();
        assert!(verify(&manifest_path, &untrusted).is_err());
    }

    #[test]
    fn signing_rejects_unknown_or_duplicate_manifest_fields() {
        let (root, manifest_path, private_path, _, _) = fixture();
        let signature_path = root.path().join("facts.sig.json");
        let mut unknown = manifest();
        unknown["command"] = json!("./plugin");
        fs::write(&manifest_path, serde_json::to_vec_pretty(&unknown).unwrap()).unwrap();
        assert!(sign(&manifest_path, &private_path, &signature_path).is_err());

        fs::write(
            &manifest_path,
            r#"{"api_version":"mastermind-facts/v1","api_version":"mastermind-facts/v1"}"#,
        )
        .unwrap();
        assert!(sign(&manifest_path, &private_path, &signature_path)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
    }
}
