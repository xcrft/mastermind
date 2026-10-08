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
        // Fixed ceiling: only status, profile_revision and source_verification are compared, and none depend on the budget.
        let person = crate::miner::profile::view(
            &options.paths,
            repo.as_ref(),
            crate::onboarding::MAX_PROFILE_BUDGET_TOKENS,
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

    let not_enabled = omitted("not_enabled", "profile_audience_not_configured");
    let mut packet = json!({
        "schema_version":1,
        "kind":"context_preview",
        "repository_content_untrusted":true,
        "repository_identity":identity,
        "selection":options,
        "delivery":"not_recorded",
        "permission_effect":"none",
        "consistency":"independent_layer_snapshots",
        "layers":{
            "person":not_enabled.clone(), "project":project, "documentation":documentation,
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
    // Check personal sources and audience access after all other layer I/O,
    // including the separately granted review queue. A revoked queue read
    // cannot leave an earlier personal payload in the returned preview.
    let non_person_tokens = non_person_budget_tokens(&packet)?;
    let repo = crate::miner::profile::RepoContext::for_root(root);
    let queue = profile_client.map(|client| crate::miner::hooks::review_queue(root, client));
    let person_layer = |budget_tokens: usize| -> Result<Value, ContextError> {
        let Some(client) = profile_client else {
            return Ok(not_enabled.clone());
        };
        let view = |budget_tokens: usize| {
            crate::miner::profile::view(
                &options.paths,
                repo.as_ref(),
                budget_tokens,
                Some((root, client)),
                Some(options.role.as_str()),
                options.workflow.as_deref(),
            )
        };
        match view(budget_tokens) {
            Ok(mut data) => {
                if matches!(
                    data["status"].as_str(),
                    Some("ok" | "insufficient_evidence")
                ) {
                    data["review_queue"] = queue.clone().unwrap_or(Value::Null);
                }
                let status = data["status"].as_str().unwrap_or("unavailable").to_owned();
                layer(&status, data)
            }
            // A budget below the view's metadata is a shortfall, not a read failure.
            Err(_) => match view(crate::onboarding::MAX_PROFILE_BUDGET_TOKENS) {
                Ok(data) => {
                    let status = data["status"].as_str().unwrap_or("unavailable").to_owned();
                    let mut dropped = layer(&status, data)?;
                    dropped["data"] = Value::Null;
                    dropped["omitted_reason"] = json!("context_budget");
                    Ok(dropped)
                }
                Err(_) => Ok(omitted("unavailable", "profile_read_failed")),
            },
        }
    };
    let initial_budget = profile_client.map_or(0, |_| {
        person_budget(root, options.budget_tokens, non_person_tokens)
    });
    fit_person_layer(
        &mut packet,
        options.budget_tokens,
        initial_budget,
        person_layer,
    )?;
    // Re-check the root after all layer I/O, including the person reads.
    capability
        .verify()
        .map_err(|_| ContextError::RootUnavailable)?;
    bound_packet(packet, options.budget_tokens)
}

/// Fits the person layer into `budget`. On overflow it rebuilds once from the view's own
/// payload size (not `initial`, not the wrapped layer), so the retry really drops cards.
/// Overflow ignores documentation, which `bound_packet` drops before person.
fn fit_person_layer(
    packet: &mut Value,
    budget: u32,
    initial: usize,
    mut view: impl FnMut(usize) -> Result<Value, ContextError>,
) -> Result<(), ContextError> {
    let person = view(initial)?;
    let view_tokens = view_payload_tokens(&person);
    packet["layers"]["person"] = person;
    stabilize_budget(packet)?;
    let probe_estimated = non_person_budget_tokens(packet)?;
    let has_data = !packet["layers"]["person"]["data"].is_null();
    if has_data && probe_estimated > u64::from(budget) {
        let overflow = probe_estimated - u64::from(budget);
        let reduced = shrink_person_budget(view_tokens, overflow);
        // A failed retry keeps the first view; `bound_packet` decides.
        if reduced >= crate::onboarding::MIN_PROFILE_BUDGET_TOKENS {
            if let Ok(retried) = view(reduced) {
                packet["layers"]["person"] = retried;
                stabilize_budget(packet)?;
            }
        }
    }
    Ok(())
}

/// The view's own payload size: `data` without the `review_queue` its budget does not bound.
fn view_payload_tokens(person: &Value) -> u64 {
    let mut data = person["data"].clone();
    if let Some(map) = data.as_object_mut() {
        map.remove("review_queue");
    }
    serde_json::to_vec(&data)
        .map(|bytes| bytes.len().div_ceil(4) as u64)
        .unwrap_or(u64::MAX)
}

/// Drops a layer for budget exactly as `bound_packet` does, so the probe measures the real cost.
fn drop_layer_for_budget(packet: &mut Value, name: &str) {
    packet["layers"][name]["data"] = Value::Null;
    packet["layers"][name]["omitted_reason"] = json!("context_budget");
}

/// Rebuilds the top-level `omitted` list from every layer's `omitted_reason`.
fn recompute_omitted(packet: &mut Value) -> Result<(), ContextError> {
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
    Ok(())
}

/// Packet size with documentation dropped as `bound_packet` would drop it, before person:
/// documentation must not take person's share. Does not mutate `packet`.
fn non_person_budget_tokens(packet: &Value) -> Result<u64, ContextError> {
    let mut probe = packet.clone();
    if !probe["layers"]["documentation"]["data"].is_null() {
        drop_layer_for_budget(&mut probe, "documentation");
    }
    recompute_omitted(&mut probe)?;
    stabilize_budget(&mut probe)?;
    Ok(probe["budget"]["estimated_tokens"]
        .as_u64()
        .unwrap_or(u64::MAX))
}

/// `min(project budget, context budget left by everything except person)`.
fn person_budget(root: &Path, requested: u32, non_person_tokens: u64) -> usize {
    let project_budget = crate::onboarding::profile_budget_tokens(root) as u64;
    let remaining = u64::from(requested).saturating_sub(non_person_tokens);
    project_budget.min(remaining) as usize
}

/// The wrapper can grow a few bytes as the payload shrinks.
const PERSON_SHRINK_SLACK_TOKENS: u64 = 16;

/// Retry budget: the previous view's own size minus overflow and slack.
/// Shrinking the given budget instead can return the same cards.
fn shrink_person_budget(view_tokens: u64, overflow: u64) -> usize {
    view_tokens.saturating_sub(overflow + PERSON_SHRINK_SLACK_TOKENS) as usize
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
        drop_layer_for_budget(&mut packet, name);
    }
    recompute_omitted(&mut packet)?;
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
            drop_layer_for_budget(&mut packet, name);
            recompute_omitted(&mut packet)?;
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
    fn shrink_person_budget_subtracts_overflow_plus_slack_from_the_views_own_size() {
        assert_eq!(
            shrink_person_budget(2000, 100),
            2000 - 100 - PERSON_SHRINK_SLACK_TOKENS as usize
        );
        // Saturates instead of underflowing when the overflow exceeds the view's size.
        assert_eq!(shrink_person_budget(50, 1000), 0);
    }

    #[test]
    fn shrink_person_budget_shrinks_even_when_the_given_budget_had_slack() {
        // Regression: shrinking a 2327 budget by 4 still held the same 2212-token view.
        let view_tokens = 2212;
        let overflow = 4;
        let reduced = shrink_person_budget(view_tokens, overflow);
        assert!(
            reduced < view_tokens as usize,
            "the next budget must be strictly below what the view already used"
        );
        assert_eq!(reduced, 2212 - 4 - PERSON_SHRINK_SLACK_TOKENS as usize);
    }

    #[test]
    fn person_budget_is_the_smaller_of_project_budget_and_remaining_context() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        // No setup.json: fails soft to the 4000-token default project budget.
        assert_eq!(person_budget(&root, 16_000, 500), 4000);
        // The remaining context budget is the smaller side.
        assert_eq!(person_budget(&root, 2_000, 1_800), 200);
        // Already over budget before the person layer: saturates at zero.
        assert_eq!(person_budget(&root, 1_000, 5_000), 0);
    }

    #[test]
    fn non_person_budget_tokens_excludes_documentation_regardless_of_its_payload() {
        let base = json!({
            "budget": {"requested_tokens":0, "estimated_tokens":0, "serialized_bytes":0, "estimator":"ceil_utf8_bytes_div4"},
            "layers": {
                "person": Value::Null,
                "project": {"status":"ok", "data": {"some":"project data"}},
                "code": {"status":"ok", "data": {"some":"code data"}},
                "work": {"status":"ok", "data": {"some":"work data"}},
            },
        });
        // Both scenarios carry real documentation, as `run-task --exec` always sends a query.
        let mut small_payload = base.clone();
        small_payload["layers"]["documentation"] =
            layer("ok", json!({"observed": Vec::<String>::new()})).unwrap();

        let mut large_payload = base;
        large_payload["layers"]["documentation"] =
            layer("ok", json!({"observed": vec!["a document section"; 50]})).unwrap();

        assert_eq!(
            non_person_budget_tokens(&small_payload).unwrap(),
            non_person_budget_tokens(&large_payload).unwrap(),
            "a documentation payload must not change the person budget; \
             bound_packet drops documentation before person"
        );
    }

    // Fake view shaped like `person_layer`: fixed skeleton, ~105-token cards popped to fit,
    // then a `review_queue` that the view budget does not bound.
    const FAKE_SKELETON_TOKENS: usize = 434;
    const FAKE_CARD_TOKENS: usize = 105;
    const FAKE_POOL_CARDS: usize = 17;
    const FAKE_REVIEW_QUEUE_TOKENS: usize = 43;
    const FAKE_NON_PERSON_TOKENS: u64 = 900;

    fn fake_view(budget_tokens: usize) -> Result<Value, ContextError> {
        let skeleton = "s".repeat(FAKE_SKELETON_TOKENS * 4);
        let mut cards = vec!["x".repeat(FAKE_CARD_TOKENS * 4); FAKE_POOL_CARDS];
        let mut data = loop {
            let data = json!({"status": "ok", "skeleton": skeleton, "cards": cards});
            let bytes = serde_json::to_vec(&data)
                .map(|bytes| bytes.len())
                .unwrap_or(usize::MAX);
            if bytes.div_ceil(4) <= budget_tokens || cards.is_empty() {
                break data;
            }
            cards.pop();
        };
        data["review_queue"] = json!("q".repeat(FAKE_REVIEW_QUEUE_TOKENS * 4));
        layer("ok", data)
    }

    fn fake_packet(budget: u32) -> Value {
        json!({
            "budget": {
                "requested_tokens": budget,
                "estimated_tokens": 0,
                "serialized_bytes": 0,
                "estimator": "ceil_utf8_bytes_div4",
            },
            "layers": {
                "person": Value::Null,
                "filler": "x".repeat((FAKE_NON_PERSON_TOKENS * 4) as usize),
            },
        })
    }

    /// Builds a packet the way `fit_person_layer` would with a single,
    /// un-shrunk call, for the two pre-fix comparisons below.
    fn fake_packet_with_person(budget: u32, person_budget_tokens: usize) -> Value {
        let mut packet = fake_packet(budget);
        packet["layers"]["person"] = fake_view(person_budget_tokens).unwrap();
        stabilize_budget(&mut packet).unwrap();
        packet
    }

    #[test]
    fn fit_person_layer_shrinks_to_the_views_own_size_across_a_budget_range() {
        let mut previous_cards = 0usize;
        for budget in [2000u32, 2500, 3000, 3050, 3100, 3200, 3300, 3400, 4000] {
            let initial = (budget as u64).saturating_sub(FAKE_NON_PERSON_TOKENS) as usize;
            let mut packet = fake_packet(budget);
            fit_person_layer(&mut packet, budget, initial, fake_view).unwrap();
            let estimated = packet["budget"]["estimated_tokens"].as_u64().unwrap();
            assert!(
                estimated <= budget as u64,
                "budget {budget}: {estimated} tokens over"
            );
            assert!(
                !packet["layers"]["person"]["data"].is_null(),
                "budget {budget}: person data dropped whole"
            );
            let cards = packet["layers"]["person"]["data"]["cards"]
                .as_array()
                .unwrap()
                .len();
            assert!(
                cards >= previous_cards,
                "budget {budget}: card count decreased ({cards} < {previous_cards})"
            );
            previous_cards = cards;
        }
    }

    #[test]
    fn fit_person_layer_fixes_the_shrink_from_initial_and_whole_layer_regressions() {
        // Tuned to overflow on the first call; both pre-fix formulas stay over budget.
        let budget = 3220u32;
        let initial = (budget as u64).saturating_sub(FAKE_NON_PERSON_TOKENS) as usize;

        let packet = fake_packet_with_person(budget, initial);
        let estimated = packet["budget"]["estimated_tokens"].as_u64().unwrap();
        assert!(
            estimated > budget as u64,
            "fixture must reproduce an overflow to exercise the shrink"
        );
        let overflow = estimated - budget as u64;

        // Pre-fix #1: shrink from `initial`.
        let old_reduced = initial.saturating_sub((overflow + PERSON_SHRINK_SLACK_TOKENS) as usize);
        let old_packet = fake_packet_with_person(budget, old_reduced);
        let old_estimated = old_packet["budget"]["estimated_tokens"].as_u64().unwrap();
        assert!(
            old_estimated > budget as u64,
            "shrinking from `initial` must still be over budget at {budget}"
        );

        // Pre-fix #2: shrink from the whole wrapped layer.
        let whole_layer_tokens = serde_json::to_vec(&packet["layers"]["person"])
            .unwrap()
            .len()
            .div_ceil(4) as u64;
        let whole_layer_reduced =
            (whole_layer_tokens).saturating_sub(overflow + PERSON_SHRINK_SLACK_TOKENS) as usize;
        let whole_layer_packet = fake_packet_with_person(budget, whole_layer_reduced);
        let whole_layer_estimated = whole_layer_packet["budget"]["estimated_tokens"]
            .as_u64()
            .unwrap();
        assert!(
            whole_layer_estimated > budget as u64,
            "shrinking from the whole wrapped layer must still be over budget at {budget}"
        );

        // The fix: shrink from the view's own payload.
        let mut fixed_packet = fake_packet(budget);
        fit_person_layer(&mut fixed_packet, budget, initial, fake_view).unwrap();
        let fixed_estimated = fixed_packet["budget"]["estimated_tokens"].as_u64().unwrap();
        assert!(
            fixed_estimated <= budget as u64,
            "fit_person_layer (shrink from the view's own payload) must fit at {budget}"
        );
    }

    #[test]
    fn fit_person_layer_keeps_the_first_view_when_a_shrink_retry_errors() {
        // The view errors below its metadata floor; the retry budget is tuned to land there.
        let budget = 3220u32;
        let initial = (budget as u64).saturating_sub(FAKE_NON_PERSON_TOKENS) as usize;
        let first = fake_view(initial).unwrap();
        let mut packet = fake_packet(budget);
        packet["layers"]["person"] = first.clone();
        stabilize_budget(&mut packet).unwrap();
        let estimated = packet["budget"]["estimated_tokens"].as_u64().unwrap();
        assert!(
            estimated > budget as u64,
            "fixture must reproduce an overflow to exercise the shrink"
        );
        let overflow = estimated - budget as u64;
        let reduced = shrink_person_budget(view_payload_tokens(&first), overflow);
        assert!(
            reduced >= crate::onboarding::MIN_PROFILE_BUDGET_TOKENS,
            "fixture must still attempt a retry"
        );
        let erroring_view = |budget_tokens: usize| -> Result<Value, ContextError> {
            if budget_tokens <= reduced {
                return Err(ContextError::BudgetTooSmall);
            }
            fake_view(budget_tokens)
        };

        let mut test_packet = fake_packet(budget);
        fit_person_layer(&mut test_packet, budget, initial, erroring_view).unwrap();
        assert_eq!(
            test_packet["layers"]["person"], first,
            "a failed shrink retry must keep the first view unchanged, leaving \
             the decision to bound_packet's own whole-layer drop"
        );
    }

    #[test]
    fn fit_person_layer_does_not_shrink_person_to_make_room_for_documentation() {
        // Documentation is dropped before person, so it must not change the card count;
        // a fine sweep catches the boundary that a single sampled budget missed.
        let documentation_payload =
            || layer("ok", json!({"observed": vec!["a document section"; 50]})).unwrap();
        let person_cards = |bounded: &Value| {
            bounded["layers"]["person"]["data"]["cards"]
                .as_array()
                .map_or(0, Vec::len)
        };

        for budget in (2000u32..=4500).step_by(5) {
            let initial = (budget as u64).saturating_sub(FAKE_NON_PERSON_TOKENS) as usize;

            let mut without_documentation = fake_packet(budget);
            fit_person_layer(&mut without_documentation, budget, initial, fake_view).unwrap();
            let without_documentation = bound_packet(without_documentation, budget).unwrap();
            let without_documentation_estimated = without_documentation["budget"]
                ["estimated_tokens"]
                .as_u64()
                .unwrap();
            assert!(
                without_documentation_estimated <= budget as u64,
                "budget {budget}: no-documentation packet over budget"
            );
            let without_documentation_cards = person_cards(&without_documentation);

            let mut with_documentation = fake_packet(budget);
            with_documentation["layers"]["documentation"] = documentation_payload();
            fit_person_layer(&mut with_documentation, budget, initial, fake_view).unwrap();
            let with_documentation = bound_packet(with_documentation, budget).unwrap();
            let with_documentation_estimated = with_documentation["budget"]["estimated_tokens"]
                .as_u64()
                .unwrap();
            assert!(
                with_documentation_estimated <= budget as u64,
                "budget {budget}: with-documentation packet over budget"
            );
            let with_documentation_cards = person_cards(&with_documentation);

            if without_documentation_cards >= 1 {
                assert!(
                    with_documentation_cards >= 1,
                    "budget {budget}: person dropped whole with documentation present \
                     ({without_documentation_cards} card(s) without it)"
                );
            }
        }
    }

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
