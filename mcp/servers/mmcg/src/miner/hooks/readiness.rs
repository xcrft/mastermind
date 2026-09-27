//! Read-only observations of separate hook boundaries. Configuration, capture,
//! native delivery, refinement and mining are deliberately reported separately.

use super::{background, fence, install, journal, journal::Journal, Error};
use serde_json::{json, Value};
use std::path::Path;

fn open_journal() -> Result<Option<Journal>, Error> {
    match std::fs::symlink_metadata(journal::path()?) {
        Ok(_) => Ok(Some(Journal::open(false)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Inspect existing local state only. This never installs hooks, starts a
/// worker, probes a provider or grants client trust. Components are independent
/// snapshots, so a concurrent hook may change state between these observations.
pub fn report(client: &str, root: &Path) -> Result<Value, Error> {
    super::client(client)?;
    let root = root.canonicalize()?;
    if !root.is_dir() || root.to_str().is_none() {
        return Err("hook_readiness_project_root_invalid".into());
    }
    let database = open_journal();
    let grant = match &database {
        Ok(Some(db)) => db.grant(client, &root),
        Ok(None) => Ok(None),
        Err(_) => Err("capture_journal_unavailable".into()),
    };
    let refiner = match &database {
        Ok(Some(db)) => db.refiner_config(client, &root),
        Ok(None) => Ok(None),
        Err(_) => Err("capture_journal_unavailable".into()),
    };
    let pending = fence::pending(client, &root);
    let capture = match &grant {
        Ok(Some(grant)) => {
            let blocked = grant.pending != 0
                || !grant.gap.is_empty()
                || !pending.as_ref().is_ok_and(|pending| !*pending);
            json!({
                "status":if !grant.enabled {"revoked"} else if blocked {"incomplete"} else {"enabled"},
                "enabled":grant.enabled,"generation":grant.generation,
                "pending":grant.pending,"gap_present":!grant.gap.is_empty(),
                "filesystem_pending":pending.as_ref().ok(),
                "scope":"local_native_event_capture","human_authorship":"unverified"
            })
        }
        Ok(None) => json!({"status":"not_configured","enabled":false,"generation":null,
            "pending":null,"filesystem_pending":pending.as_ref().ok()}),
        Err(_) => json!({"status":"unavailable","enabled":null,"generation":null,
            "pending":null,"filesystem_pending":pending.as_ref().ok(),"reason":"capture_journal_unavailable"}),
    };
    let mut activation = match (&database, &grant) {
        (Ok(Some(db)), Ok(Some(grant))) if grant.enabled => db
            .capture_activation(client, &root, grant.generation)
            .unwrap_or_else(|_| json!({"status":"unavailable","capture_generation":grant.generation,
                "current_sessions":null,"complete":false,"reason":"session_observation_unavailable"})),
        (_, Ok(_)) => json!({"status":"not_observed","capture_generation":null,
            "current_sessions":null,"complete":false,"reason":"capture_not_enabled"}),
        _ => json!({"status":"unavailable","capture_generation":null,
            "current_sessions":null,"complete":false,"reason":"capture_journal_unavailable"}),
    };
    activation["source"] = json!("local_unverified_native_event");
    activation["client_trust"] = json!("not_independently_verified");
    activation["meaning"] = json!("A SessionStart observation belongs to the current capture generation. It does not prove that the current native configuration is trusted or loaded.");

    let valid_refiner = refiner
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .filter(|config| config.validate().is_ok());
    let refiner_report = match &refiner {
        Ok(Some(config)) => json!({
            "status":if valid_refiner.is_some() {"configured"} else {"configuration_invalid"},
            "selection":if config.provider.as_deref()==Some("claude") {"claude"} else {"custom_processor"},
            "timeout_seconds":config.timeout_secs,"trigger":"UserPromptSubmit",
            "native_delivery":"additional_context","execution":"not_tested",
            "authority":"advisory","permission_effect":"none"
        }),
        Ok(None) => json!({"status":"not_configured","optional":true,"execution":"not_tested"}),
        Err(_) => {
            json!({"status":"unavailable","execution":"not_tested","reason":"refiner_configuration_unavailable"})
        }
    };
    let native = if cfg!(unix) {
        match install::configure(
            client,
            &root,
            false,
            false,
            valid_refiner.map(|config| config.timeout_secs),
        ) {
            Ok(preview) => json!({
                "status":if preview["changed"]==false {"current"} else {"missing_or_stale"},
                "config_path":preview["config_path"],
                "local_hooks_disabled":preview["local_hooks_disabled"],
                "requires_client_trust":preview["requires_client_trust"],
                "client_activation":preview["client_activation"],
                "coverage":"native_events_only","permission_effect":"none"
            }),
            Err(_) => json!({"status":"unavailable","reason":"native_configuration_unavailable",
                "local_hooks_disabled":null,"requires_client_trust":client=="codex"}),
        }
    } else {
        json!({"status":"unsupported","reason":"native_hooks_require_unix","local_hooks_disabled":null})
    };
    let mut mining = if cfg!(unix) {
        background::status(client, &root)
            .unwrap_or_else(|_| json!({"status":"unavailable","reason":"worker_state_unavailable"}))
    } else {
        json!({"status":"unsupported","reason":"managed_worker_requires_unix"})
    };
    mining["autostart"] = json!(false);
    mining["foreground_workers"] = json!("not_observed");
    mining["output"] = json!("unreviewed_drafts_only");

    let configured = if capture["enabled"] == true
        || refiner.as_ref().is_ok_and(Option::is_some)
        || native["status"] == "current"
        || !matches!(
            mining["status"].as_str(),
            Some("not_configured" | "unsupported" | "unavailable")
        ) {
        Some(true)
    } else if grant.is_err()
        || refiner.is_err()
        || native["status"] == "unavailable"
        || mining["status"] == "unavailable"
    {
        None
    } else {
        Some(false)
    };
    let mut warnings = Vec::new();
    if configured != Some(false) {
        match capture["status"].as_str() {
            Some("enabled") => {}
            Some("incomplete") => warnings.push("capture_incomplete"),
            Some("unavailable") => warnings.push("capture_unavailable"),
            _ => warnings.push("capture_not_enabled"),
        }
        match native["status"].as_str() {
            Some("current") => {}
            Some("missing_or_stale") => warnings.push("native_registration_missing_or_stale"),
            Some("unsupported") => warnings.push("native_platform_unsupported"),
            _ => warnings.push("native_registration_unavailable"),
        }
        if native["local_hooks_disabled"] == true {
            warnings.push("native_hooks_disabled");
        }
        if capture["enabled"] == true && activation["status"] != "session_start_observed" {
            warnings.push("current_session_start_not_observed");
        }
        if matches!(
            refiner_report["status"].as_str(),
            Some("configuration_invalid" | "unavailable")
        ) {
            warnings.push("refiner_configuration_unavailable");
        }
        if !matches!(
            mining["status"].as_str(),
            Some("running" | "not_configured" | "unsupported")
        ) {
            warnings.push("managed_miner_not_running");
        }
    }
    Ok(json!({
        "schema":1,"client":client,"project_root":root,"inspection":"read_only",
        "consistency":"independent_snapshots","configured":configured,
        "platform":if cfg!(unix) {"unix"} else {"unsupported"},
        "native_registration":native,"capture":capture,"activation":activation,
        "refiner":refiner_report,"mining":mining,"warnings":warnings,
        "boundaries":[
            "Native configuration, capture permission, observed delivery, refinement and mining are separate states.",
            "Status does not call a provider, start mining or establish model quality or semantic truth.",
            "Hook coverage is incomplete. Observations are not a permission system or proof of human authorship."
        ]
    }))
}
