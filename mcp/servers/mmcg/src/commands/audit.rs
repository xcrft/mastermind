use mmcg::audit_bundle::{
    read_envelope, read_signature, sign_detached, verify_envelope, write_atomic, VerifyPolicy,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct VerifyOptions {
    pub bundle: PathBuf,
    pub root: Option<PathBuf>,
    pub expected_repository: Option<String>,
    pub expected_baseline: Option<String>,
    pub expected_head: Option<String>,
    pub signature: Option<PathBuf>,
    pub public_key: Option<PathBuf>,
    pub require_signature: bool,
    pub trusted_key_ids: Vec<String>,
    pub revoked_key_ids: Vec<String>,
    pub integrity_only: bool,
    pub json: bool,
}

pub fn verify(options: VerifyOptions) -> Result<(), Box<dyn std::error::Error>> {
    let envelope = read_envelope(&options.bundle)?;
    let signature = options
        .signature
        .as_deref()
        .map(read_signature)
        .transpose()?;
    let snapshot_requested = options.root.is_some()
        || options.expected_repository.is_some()
        || options.expected_baseline.is_some()
        || options.expected_head.is_some();
    let policy = VerifyPolicy {
        expected_repository: options.expected_repository,
        expected_baseline: options.expected_baseline,
        expected_head: options.expected_head,
        root: options.root,
        require_clean_worktree: snapshot_requested && !options.integrity_only,
        public_key: options.public_key,
        require_signature: options.require_signature || signature.is_some(),
        signature,
        trusted_key_ids: options.trusted_key_ids.into_iter().collect(),
        revoked_key_ids: options.revoked_key_ids.into_iter().collect(),
        integrity_only: options.integrity_only,
    };
    let report = verify_envelope(&envelope, &policy);
    if options.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if options.integrity_only {
        println!("UNTRUSTED INTEGRITY-ONLY DIAGNOSTIC");
        println!("content_integrity: {}", report.content_integrity);
        println!("provenance_authenticity: not_evaluated");
        println!("policy_acceptance: not_evaluated");
        println!("reasons: {}", report.reasons.join(","));
    } else {
        println!("content_integrity: {}", report.content_integrity);
        println!(
            "provenance_authenticity: {}",
            report.provenance_authenticity
        );
        println!("policy_acceptance: {}", report.policy_acceptance);
        println!("reasons: {}", report.reasons.join(","));
    }
    let success = if options.integrity_only {
        report.content_integrity == "pass"
    } else {
        report.accepted()
    };
    if !success {
        return Err("audit verification rejected".into());
    }
    Ok(())
}

pub fn sign(
    bundle: &Path,
    private_key: &Path,
    signature_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let envelope = read_envelope(bundle)?;
    let signature = sign_detached(&envelope, private_key)?;
    let bytes = serde_json::to_vec_pretty(&signature)?;
    write_atomic(signature_path, &bytes, true)?;
    Ok(())
}

pub fn prepare_output(root: &Path, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let output = mmcg::audit_bundle::create_contained_output_dir(root, path)?;
    println!("{}", output.display());
    Ok(())
}

pub fn validate_key_ids(values: &[String]) -> Result<BTreeSet<String>, String> {
    values
        .iter()
        .map(|value| {
            let Some(hex) = value.strip_prefix("sha256:") else {
                return Err("key IDs must use sha256:<64 lowercase hex>".into());
            };
            if hex.len() != 64
                || !hex
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err("key IDs must use sha256:<64 lowercase hex>".into());
            }
            Ok(value.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    use mmcg::audit_bundle::{
        seal, sha256_hex, DiffBinding, InputBinding, Manifest, RepositoryBinding, ToolBinding,
    };
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn envelope_file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let manifest = Manifest {
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
                name_status: vec![],
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
            declared_files: vec![],
            changed_files: vec![],
            verified_claims: vec![],
            failed_claims: vec![],
            discrepancies: vec![],
            snapshot_drift: vec![],
            snapshot_changed: false,
            mmcg_queries: vec![],
            verify_commands: vec![],
            human_summary: "held".into(),
        };
        let bundle = root.join("bundle.json");
        std::fs::write(
            &bundle,
            serde_json::to_vec(&seal(manifest).unwrap()).unwrap(),
        )
        .unwrap();
        (dir, bundle)
    }

    #[test]
    fn audit_verify_empty_policy_fails_with_no_trust_anchor() {
        let (_dir, bundle) = envelope_file();
        let envelope = read_envelope(&bundle).unwrap();
        let report = verify_envelope(&envelope, &VerifyPolicy::default());
        assert_eq!(report.policy_acceptance, "fail");
        assert_eq!(report.reasons, vec!["no_trust_anchor"]);
    }

    #[test]
    fn audit_verify_partial_policy_fails_closed() {
        let (_dir, bundle) = envelope_file();
        let envelope = read_envelope(&bundle).unwrap();
        let report = verify_envelope(
            &envelope,
            &VerifyPolicy {
                expected_repository: Some("owner/repo".into()),
                ..VerifyPolicy::default()
            },
        );
        assert_eq!(report.reasons, vec!["incomplete_snapshot_policy"]);
    }

    #[test]
    fn audit_verify_json_has_three_independent_verdicts() {
        let (_dir, bundle) = envelope_file();
        let report = verify_envelope(
            &read_envelope(&bundle).unwrap(),
            &VerifyPolicy {
                integrity_only: true,
                ..VerifyPolicy::default()
            },
        );
        let json = serde_json::to_value(report).unwrap();
        assert!(json.get("content_integrity").is_some());
        assert!(json.get("provenance_authenticity").is_some());
        assert!(json.get("policy_acceptance").is_some());
    }

    #[test]
    fn audit_verify_policy_requires_exact_full_head_and_repository() {
        let (_dir, bundle) = envelope_file();
        let report = verify_envelope(
            &read_envelope(&bundle).unwrap(),
            &VerifyPolicy {
                expected_repository: Some("wrong/repo".into()),
                expected_baseline: Some("1".repeat(40)),
                expected_head: Some("short".into()),
                root: Some(PathBuf::from(".")),
                require_clean_worktree: true,
                ..VerifyPolicy::default()
            },
        );
        assert_eq!(report.policy_acceptance, "fail");
        assert!(report.reasons.contains(&"repository_mismatch".into()));
        assert!(report.reasons.contains(&"head_mismatch".into()));
    }

    #[test]
    fn integrity_only_is_explicitly_not_reusable_as_policy() {
        let (_dir, bundle) = envelope_file();
        let report = verify_envelope(
            &read_envelope(&bundle).unwrap(),
            &VerifyPolicy {
                integrity_only: true,
                ..VerifyPolicy::default()
            },
        );
        assert_eq!(report.policy_acceptance, "not_evaluated");
        assert!(report.untrusted_diagnostic);
    }

    #[test]
    fn key_id_validation_rejects_non_lowercase_or_truncated_values() {
        assert!(validate_key_ids(&[format!("sha256:{}", "a".repeat(64))]).is_ok());
        assert!(validate_key_ids(&["sha256:ABC".into()]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn audit_sign_refuses_invalid_envelope_and_unsafe_key_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, bundle) = envelope_file();
        let key = dir.path().join("private.key");
        let output = dir.path().join("signature.json");
        std::fs::write(&key, format!("{}\n", BASE64.encode([7_u8; 32]))).unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(sign(&bundle, &key, &output).is_err());

        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut envelope = read_envelope(&bundle).unwrap();
        envelope.integrity.manifest_digest = format!("sha256:{}", "0".repeat(64));
        std::fs::write(&bundle, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(sign(&bundle, &key, &output).is_err());
    }
}
