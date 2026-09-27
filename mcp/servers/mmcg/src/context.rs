//! A bounded, read-only composition of independently checked context layers.
//!
//! This is a preview, not a delivery receipt or a permission grant. The person
//! remains behind the existing audience grant and never enters Lens exports.

use crate::queries::{self, BriefRole};
use crate::store::Store;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const DEFAULT_BUDGET: u32 = 8_000;
pub const MIN_BUDGET: u32 = 1_024;
pub const MAX_BUDGET: u32 = 16_000;

#[derive(Debug, Clone, Serialize)]
pub struct ContextOptions {
    pub since: String,
    pub paths: Vec<String>,
    pub role: BriefRole,
    pub workflow: Option<String>,
    pub query: Option<String>,
    pub budget_tokens: u32,
}

impl ContextOptions {
    pub fn normalized(&self) -> Result<Self, ContextError> {
        if !(MIN_BUDGET..=MAX_BUDGET).contains(&self.budget_tokens)
            || queries::validate_brief_request(&self.since, 2_000).is_err()
            || self.paths.len() > 64
            || self.workflow.as_deref().is_some_and(|v| !selector(v, 128))
            || self.query.as_deref().is_some_and(|v| !selector(v, 512))
        {
            return Err(ContextError::InvalidSelection);
        }
        let mut options = self.clone();
        options.paths = self
            .paths
            .iter()
            .map(|path| {
                if !selector(path, 1_024) {
                    return Err(ContextError::InvalidSelection);
                }
                queries::normalize_map_path(path)
                    .ok()
                    .filter(|path| !path.is_empty())
                    .ok_or(ContextError::InvalidSelection)
            })
            .collect::<Result<_, _>>()?;
        options.paths.sort();
        options.paths.dedup();
        Ok(options)
    }
}

fn selector(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextError {
    InvalidSelection,
    RootUnavailable,
    Serialization,
    BudgetTooSmall,
}

impl ContextError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidSelection => "invalid_context_selection",
            Self::RootUnavailable => "context_root_unavailable",
            Self::Serialization => "context_serialization_failed",
            Self::BudgetTooSmall => "context_budget_too_small",
        }
    }
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ContextError {}

fn omitted(status: &str, reason: &str) -> Value {
    json!({"status":status, "revision":null, "omitted_reason":reason, "data":null})
}

fn layer(status: &str, data: Value) -> Result<Value, ContextError> {
    // Retain epistemic status even when the whole payload does not fit. An
    // `ok` selection alone must not conceal incomplete source verification.
    let verification: serde_json::Map<String, Value> = [
        "source_verification",
        "source_verification_scope",
        "evidence_basis",
        "freshness",
        "review_status",
        "current_checkout",
    ]
    .iter()
    .filter_map(|key| {
        data.get(*key)
            .map(|value| ((*key).to_owned(), value.clone()))
    })
    .collect();
    Ok(json!({
        "status":status,
        "revision":digest(&data)?,
        "omitted_reason":null,
        "verification":verification,
        "data":data,
    }))
}

fn digest(value: &Value) -> Result<String, ContextError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ContextError::Serialization)?;
    Ok(crate::hex::encode(&Sha256::digest(bytes)))
}

/// Only CLI/server configuration may supply `profile_client`. This function
/// does not infer an audience from request parameters, repository text or SQL.
pub fn from_paths(
    root: &Path,
    index_path: &Path,
    options: &ContextOptions,
    profile_client: Option<&str>,
) -> Result<Value, ContextError> {
    options.normalized()?;
    let budget_ms = crate::store::query_budget_ms_from_env(crate::store::DEFAULT_CLI_BUDGET_MS);
    let deadline = (budget_ms > 0)
        .then(|| std::time::Instant::now() + std::time::Duration::from_millis(budget_ms));
    let store = Store::open_read_only_with_deadline(index_path, deadline).ok();
    if let Some(store) = store.as_ref() {
        store.push_work_budget(crate::store::WorkBudget::from_millis(budget_ms));
    }
    let result = build(root, store.as_ref(), options, profile_client);
    if let Some(store) = store.as_ref() {
        store.pop_work_budget();
    }
    result
}

/// Revalidate the evidence actually selected for delivery immediately before
/// native execution. Work rows are historical previews and are deliberately
/// excluded: publishing this invocation changes its own work row. This is an
/// optimistic boundary check, not an atomic snapshot across Git, SQL and files.
pub fn validate_delivery(
    root: &Path,
    index_path: &Path,
    options: &ContextOptions,
    profile_client: Option<&str>,
    offered: &Value,
) -> Result<(), &'static str> {
    let normalized = options
        .normalized()
        .map_err(|_| "invocation_context_selection_invalid")?;
    let options = &normalized;
    let mut selection = options.clone();
    // Rechecking a selected layer must not omit it merely because another
    // optional layer has grown since the initial preview.
    selection.budget_tokens = MAX_BUDGET;
    // The personal selection is checked separately, after project source I/O.
    let current = from_paths(root, index_path, &selection, None)
        .map_err(|_| "invocation_context_revalidation_unavailable")?;
    if offered["repository_identity"] != current["repository_identity"] {
        return Err("invocation_context_repository_changed");
    }
    for name in ["project", "documentation", "code"] {
        let previous = &offered["layers"][name];
        if !previous["data"].is_null()
            && (previous["status"] != current["layers"][name]["status"]
                || previous["revision"] != current["layers"][name]["revision"])
        {
            return Err("invocation_context_sources_changed");
        }
    }
    let previous = &offered["layers"]["person"]["data"];
    if !previous.is_null() && !previous["profile_revision"].is_null() {
        // Do this last, after all other source I/O. Compare selected evidence,
        // not the global store revision or the advisory draft review queue.
        let client = profile_client.ok_or("invocation_profile_audience_missing")?;
        let repo = crate::miner::profile::RepoContext::for_root(root);
        let person = crate::miner::profile::view(
            &options.paths,
            repo.as_ref(),
            2_000,
            Some((root, client)),
            Some(options.role.as_str()),
            options.workflow.as_deref(),
        )
        .map_err(|_| "invocation_profile_revalidation_unavailable")?;
        if previous["status"] != person["status"]
            || previous["profile_revision"] != person["profile_revision"]
            || previous["source_verification"] != person["source_verification"]
        {
            return Err("invocation_profile_sources_or_grant_changed");
        }
    }
    Ok(())
}

/// The approved title is a bounded search selector, never an instruction or
/// query language. The document reader treats the resulting words literally.
#[cfg(any(unix, test))]
pub(crate) fn task_query(title: Option<&str>) -> Option<String> {
    let words: Vec<String> = title?
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .take(12)
        .map(|word| {
            let mut bytes = 0;
            let literal: String = word
                .chars()
                .take_while(|ch| {
                    bytes += ch.len_utf8();
                    bytes <= 32
                })
                .collect();
            format!("\"{literal}\"")
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

/// Reuses the existing readers without refreshing an index, mining a profile,
/// creating a store, invoking a model or recording delivery. Each layer owns
/// its freshness check. There is no cross-store atomic snapshot claim.
pub fn build(
    root: &Path,
    store: Option<&Store>,
    options: &ContextOptions,
    profile_client: Option<&str>,
) -> Result<Value, ContextError> {
    let options = options.normalized()?;
    let capability =
        crate::bounded_fs::RootCapability::open(root).map_err(|_| ContextError::RootUnavailable)?;
    let root = capability.canonical_root();
    let identity =
        crate::facts::repository_identity(root).map_err(|_| ContextError::RootUnavailable)?;

    let mut project = omitted("unavailable", "index_unavailable");
    let mut code = project.clone();
    let mut documentation = if options.query.is_some() {
        project.clone()
    } else {
        omitted("not_requested", "documentation_query_not_selected")
    };
    if let Some(store) = store {
        let valid = store.schema_current().unwrap_or(false)
            && crate::indexer::validate_index_root(store, root).is_ok()
            && store.ensure_source_snapshot_current().is_ok();
        if valid {
            project = match queries::project_profile(store, options.query.as_deref(), 4) {
                Ok(data) => {
                    let status = data.status;
                    layer(
                        status,
                        serde_json::to_value(data).map_err(|_| ContextError::Serialization)?,
                    )?
                }
                Err(_) => omitted("unavailable", "project_query_failed"),
            };
            if let Some(query) = options.query.as_deref() {
                documentation = match queries::documents(store, query, 4) {
                    Ok(mut data) => {
                        // A text match in a stale derived index is not context.
                        if data.freshness != "fresh" || !data.section_extractor_current {
                            data.observed.clear();
                            data.count = 0;
                            let mut result = layer(
                                "index_not_fresh",
                                serde_json::to_value(data)
                                    .map_err(|_| ContextError::Serialization)?,
                            )?;
                            result["omitted_reason"] = json!("documentation_index_not_fresh");
                            result
                        } else {
                            layer(
                                "ok",
                                serde_json::to_value(data)
                                    .map_err(|_| ContextError::Serialization)?,
                            )?
                        }
                    }
                    Err(_) => omitted("unavailable", "documentation_query_failed"),
                };
            }
            code = match crate::lens::validate_index_snapshot(store, root, store.request_deadline())
            {
                Ok(()) => match queries::brief(
                    store,
                    root,
                    &options.since,
                    options.role,
                    2_000,
                    &|packet| {
                        serde_json::to_vec(packet)
                            .map(|v| v.len())
                            .map_err(|_| queries::BriefError::Serialization)
                    },
                ) {
                    Ok(data) => layer(
                        "ok",
                        serde_json::to_value(data).map_err(|_| ContextError::Serialization)?,
                    )?,
                    Err(error) => omitted("unavailable", error.code()),
                },
                Err(_) => omitted("index_not_fresh", "code_index_not_fresh"),
            };
            if store.ensure_source_snapshot_current().is_err() {
                project = omitted("unavailable", "index_snapshot_changed");
                code = project.clone();
                if options.query.is_some() {
                    documentation = project.clone();
                }
            }
        } else {
            project = omitted("unavailable", "index_root_schema_or_snapshot_mismatch");
            code = project.clone();
            if options.query.is_some() {
                documentation = project.clone();
            }
        }
    }
    let work = crate::workflow_status::task_overview(root, 20);
    let work_status = work["status"].as_str().unwrap_or("unavailable").to_owned();
    // Check personal sources and audience access after all other layer I/O,
    // including the separately granted review queue. A revoked queue read
    // cannot leave an earlier personal payload in the returned preview.
    let person = match profile_client {
        None => omitted("not_enabled", "profile_audience_not_configured"),
        Some(client) => {
            let queue = crate::miner::hooks::review_queue(root, client);
            let repo = crate::miner::profile::RepoContext::for_root(root);
            match crate::miner::profile::view(
                &options.paths,
                repo.as_ref(),
                2_000,
                Some((root, client)),
                Some(options.role.as_str()),
                options.workflow.as_deref(),
            ) {
                Ok(mut data) => {
                    if matches!(
                        data["status"].as_str(),
                        Some("ok" | "insufficient_evidence")
                    ) {
                        data["review_queue"] = queue;
                    }
                    let status = data["status"].as_str().unwrap_or("unavailable").to_owned();
                    layer(&status, data)?
                }
                Err(_) => omitted("unavailable", "profile_read_failed"),
            }
        }
    };

    capability
        .verify()
        .map_err(|_| ContextError::RootUnavailable)?;
    let packet = json!({
        "schema_version":1,
        "kind":"context_preview",
        "repository_content_untrusted":true,
        "repository_identity":identity,
        "selection":options,
        "delivery":"not_recorded",
        "permission_effect":"none",
        "consistency":"independent_layer_snapshots",
        "layers":{
            "person":person, "project":project, "documentation":documentation,
            "code":code, "work":layer(&work_status, work)?,
        },
        "budget":{
            "requested_tokens":options.budget_tokens,
            "estimated_tokens":0,
            "serialized_bytes":0,
            "estimator":"ceil_utf8_bytes_div4",
        },
        "context_revision":"0".repeat(64),
        "omitted":[],
        "precision_notes":[
            "Source binding, human review, semantic accuracy and current execution are separate evidence.",
            "Revisions hash exact compact UTF-8 JSON bytes: layer data before omission; the packet with only its top-level context_revision member removed, preserving other bytes and key order. Neither establishes truth or an atomic cross-store snapshot.",
            "Person data is advisory and does not enlarge task, role or environment permissions.",
            "Task completion is historical; this preview does not re-run checks or prove current checkout correctness.",
            "Counts describe this selection and its declared limits, not completeness of a person or project.",
            "Code covers the selected repository diff; paths filter the person layer. Project and documentation use the query.",
            "Size units estimate UTF-8 JSON bytes, not model-specific tokens. MCP framing is additional.",
        ],
    });
    bound_packet(packet, options.budget_tokens)
}

fn bound_packet(mut packet: Value, budget: u32) -> Result<Value, ContextError> {
    // Whole optional components are removed with their provenance intact.
    // Never cut a statement, its exception, a citation, or a JSON value.
    for name in ["documentation", "person", "project", "code", "work"] {
        stabilize_budget(&mut packet)?;
        if packet["budget"]["estimated_tokens"]
            .as_u64()
            .unwrap_or(u64::MAX)
            <= u64::from(budget)
        {
            break;
        }
        if packet["layers"][name]["data"].is_null() {
            continue;
        }
        packet["layers"][name]["data"] = Value::Null;
        packet["layers"][name]["omitted_reason"] = json!("context_budget");
    }
    let omissions: Vec<Value> = packet["layers"]
        .as_object()
        .ok_or(ContextError::Serialization)?
        .iter()
        .filter_map(|(name, value)| {
            value["omitted_reason"]
                .as_str()
                .map(|reason| json!({"layer":name,"reason":reason}))
        })
        .collect();
    packet["omitted"] = json!(omissions);
    stabilize_budget(&mut packet)?;
    // Omission metadata can add bytes at the boundary. Repeat admission on the
    // complete packet, retaining only fixed-size metadata when necessary.
    for name in ["documentation", "person", "project", "code", "work"] {
        if packet["budget"]["estimated_tokens"]
            .as_u64()
            .unwrap_or(u64::MAX)
            <= u64::from(budget)
        {
            break;
        }
        if !packet["layers"][name]["data"].is_null() {
            packet["layers"][name]["data"] = Value::Null;
            packet["layers"][name]["omitted_reason"] = json!("context_budget");
            packet["omitted"]
                .as_array_mut()
                .ok_or(ContextError::Serialization)?
                .push(json!({"layer":name,"reason":"context_budget"}));
            stabilize_budget(&mut packet)?;
        }
    }
    if packet["budget"]["estimated_tokens"]
        .as_u64()
        .unwrap_or(u64::MAX)
        > u64::from(budget)
    {
        return Err(ContextError::BudgetTooSmall);
    }
    let mut revision_input = packet.clone();
    revision_input
        .as_object_mut()
        .ok_or(ContextError::Serialization)?
        .remove("context_revision");
    packet["context_revision"] = json!(digest(&revision_input)?);
    Ok(packet)
}

fn stabilize_budget(packet: &mut Value) -> Result<(), ContextError> {
    for _ in 0..16 {
        let bytes = serde_json::to_vec(packet)
            .map_err(|_| ContextError::Serialization)?
            .len();
        let units = bytes.div_ceil(4);
        if packet["budget"]["serialized_bytes"] == json!(bytes)
            && packet["budget"]["estimated_tokens"] == json!(units)
        {
            return Ok(());
        }
        packet["budget"]["serialized_bytes"] = json!(bytes);
        packet["budget"]["estimated_tokens"] = json!(units);
    }
    Err(ContextError::Serialization)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multilingual_task_title_search_cannot_exceed_selection_limits() {
        let title = ("Ж".repeat(500) + "\n").repeat(100);
        let options = ContextOptions {
            since: "HEAD".into(),
            paths: vec![],
            role: BriefRole::Executor,
            workflow: None,
            query: task_query(Some(&title)),
            budget_tokens: DEFAULT_BUDGET,
        };
        assert!(options.normalized().is_ok());
        assert!(options.query.as_ref().unwrap().len() <= 512);
        assert_eq!(task_query(Some("\n\t---***\"")), None);
    }
}
