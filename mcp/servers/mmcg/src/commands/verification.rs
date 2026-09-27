//! Explicit foreground verification execution. Read-only commands never call this.

use std::path::{Path, PathBuf};

pub fn dispatch(
    spec: &Path,
    root: PathBuf,
    id: &str,
    json: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    let root = std::path::absolute(root)?;
    let spec = if spec.is_absolute() {
        spec.to_path_buf()
    } else {
        root.join(spec)
    };
    let receipt = mmcg::verification_receipts::run(&spec, &root, id)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        println!(
            "Verification {}: {:?} (exit {:?}, {} ms). Local runner record; not independent attestation.",
            receipt.check_id, receipt.status, receipt.exit_code, receipt.duration_ms
        );
    }
    Ok(receipt.passed())
}
