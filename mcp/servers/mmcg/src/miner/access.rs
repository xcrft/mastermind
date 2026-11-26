//! Explicit, local grants for exposing the user-global profile to one project
//! through one configured MCP client. Grants never come from tool arguments.

use super::store::ProfileStore;
use std::path::Path;

pub fn valid_client_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub fn set(root: &Path, client_id: &str, allowed: bool) -> Result<(), Box<dyn std::error::Error>> {
    configure(root, client_id, allowed)?;
    let root = root.display();
    println!(
        "{} profile access for client `{client_id}` in `{root}`.",
        if allowed { "Granted" } else { "Revoked" }
    );
    Ok(())
}

/// Grant or revoke profile read access for one repository root and MCP client.
///
/// Validates the client identifier and canonicalizes a UTF-8 directory root
/// before updating the global profile store. Revoking access is a no-op when
/// no store exists. Filesystem and store failures propagate; nothing is printed.
pub fn configure(
    root: &Path,
    client_id: &str,
    allowed: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !valid_client_id(client_id) {
        return Err(
            "client must be 1-64 ASCII letters, digits, dots, dashes or underscores".into(),
        );
    }
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err("profile access root must be a directory".into());
    }
    let root = root.to_str().ok_or("profile access root is not UTF-8")?;
    let path = ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let store = if allowed {
        Some(ProfileStore::open(&path)?)
    } else {
        ProfileStore::open_existing(&path)?
    };
    if let Some(mut store) = store {
        store.set_reader_grant(root, client_id, allowed)?;
    }
    Ok(())
}

pub fn list() -> Result<(), Box<dyn std::error::Error>> {
    let path = ProfileStore::db_path().ok_or("could not resolve home directory")?;
    let Some(store) = ProfileStore::open_optional_read_only(&path)? else {
        println!("No profile access grants.");
        return Ok(());
    };
    let grants = store.reader_grants()?;
    if grants.is_empty() {
        println!("No profile access grants.");
    }
    for (root, client) in grants {
        println!("{root}\t{client}");
    }
    Ok(())
}
