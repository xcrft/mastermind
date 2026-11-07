//! Shared freshness admission and bounded refresh for CLI and MCP queries.

use crate::{queries, store::Store};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub(crate) enum StaleReason {
    Structural,
    Concept,
    History,
}
impl StaleReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Structural => "structural",
            Self::Concept => "concept",
            Self::History => "history",
        }
    }
}

#[derive(Debug)]
pub(crate) struct StaleIndex {
    pub status: queries::StatusResponse,
    pub reason: StaleReason,
    pub refresh_attempted: bool,
}

#[derive(Debug)]
pub(crate) enum Error {
    Stale(Box<StaleIndex>),
    SchemaIncompatible,
    SnapshotChanged,
    WorkLimitExceeded,
    /// A forced full rebuild (changed extractor or concept contract) ran out of
    /// request budget. A forced rebuild restarts from zero on every attempt, so
    /// only a manual `mastermind index .` or a larger budget converges.
    RebuildRequired,
    RefreshLimit {
        dimension: &'static str,
        cap: u64,
    },
    Internal {
        class: &'static str,
    },
    Brief(queries::BriefError),
    Concept(queries::ConceptError),
    Review(String),
}

impl Error {
    fn internal(class: &'static str, _error: impl std::fmt::Display) -> Self {
        Self::Internal { class }
    }
    pub(crate) fn into_brief(self) -> queries::BriefError {
        match self {
            Self::Brief(error) => error,
            Self::SchemaIncompatible => queries::BriefError::SchemaIncompatible,
            Self::SnapshotChanged => queries::BriefError::SnapshotChanged,
            Self::WorkLimitExceeded | Self::RebuildRequired | Self::RefreshLimit { .. } => {
                queries::BriefError::WorkLimitExceeded
            }
            _ => queries::BriefError::IndexStale,
        }
    }
    pub(crate) fn into_concept(self) -> queries::ConceptError {
        match self {
            Self::Concept(error) => error,
            Self::SchemaIncompatible => queries::ConceptError::SchemaIncompatible,
            Self::SnapshotChanged => queries::ConceptError::SnapshotChanged,
            Self::WorkLimitExceeded | Self::RebuildRequired | Self::RefreshLimit { .. } => {
                queries::ConceptError::WorkLimitExceeded
            }
            _ => queries::ConceptError::IndexStale,
        }
    }
    pub(crate) fn review_code(self) -> String {
        match self {
            Self::Review(error) => error,
            error => error.into_concept().code().to_string(),
        }
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static REFRESH_TEST_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Freshness under the auto-refresh inventory caps, as MCP refresh can only
/// repair an index within them.
pub(crate) fn index_status(store: &Store) -> Result<queries::StatusResponse, Error> {
    index_status_with_limits(store, crate::indexer::IndexLimits::AUTO_REFRESH)
}

fn index_status_with_limits(
    store: &Store,
    limits: crate::indexer::IndexLimits,
) -> Result<queries::StatusResponse, Error> {
    let db_path = queries::database_path_identity(store);
    let db_path_error = db_path.is_none().then_some("non_utf8_path");
    let symbol_count = store
        .symbol_count()
        .map_err(|error| Error::internal("symbol_count_query", error))?;
    let file_count = store
        .file_count()
        .map_err(|error| Error::internal("file_count_query", error))?;
    let extractor_contract_current = store
        .extractor_contract_current()
        .map_err(|error| Error::internal("extractor_contract_query", error))?;
    let concept_contract_current = store
        .concept_contract_current()
        .map_err(|error| Error::internal("concept_contract_query", error))?;
    let index_root = store
        .meta_value("index_root")
        .map_err(|error| Error::internal("index_root_query", error))?
        .map(PathBuf::from);
    let (stale_files, stale_files_truncated, freshness_error, unindexable_sources) =
        match index_root {
            Some(index_root) => match index_root.canonicalize() {
                Err(_) => (1, false, Some("index_root_unavailable"), Vec::new()),
                Ok(root)
                    if store
                        .serve_root()
                        .is_some_and(|authorized| authorized != root.as_path()) =>
                {
                    (1, false, Some("index_root_mismatch"), Vec::new())
                }
                Ok(root) => {
                    let interrupted = || store.work_interrupted();
                    let (source_cap, source_bytes_cap) = limits.source_caps();
                    match crate::indexer::stale_paths_controlled(
                        store,
                        &root,
                        queries::STATUS_STALE_FILE_PROBE_LIMIT,
                        source_cap,
                        source_bytes_cap,
                        crate::bounded_fs::ReadControl {
                            deadline: store.request_deadline(),
                            interrupted: Some(&interrupted),
                        },
                    ) {
                        Ok(scan) => match store.source_snapshot_unchanged() {
                            Ok(true) => (
                                scan.paths.len().min(queries::STATUS_STALE_FILE_LIMIT),
                                scan.paths.len() > queries::STATUS_STALE_FILE_LIMIT,
                                None,
                                scan.unindexable,
                            ),
                            Ok(false) => return Err(Error::SnapshotChanged),
                            Err(error) => {
                                return Err(Error::internal("freshness_snapshot_query", error))
                            }
                        },
                        Err(crate::indexer::IndexError::LimitExceeded { dimension, cap }) => {
                            return Err(Error::RefreshLimit { dimension, cap })
                        }
                        Err(crate::indexer::IndexError::SnapshotChanged) => {
                            return Err(Error::SnapshotChanged)
                        }
                        Err(crate::indexer::IndexError::Cancelled)
                        | Err(crate::indexer::IndexError::DeadlineExceeded) => {
                            return Err(Error::internal(
                                "freshness_interrupted",
                                "request interrupted",
                            ))
                        }
                        Err(_) => (1, false, Some("freshness_scan_failed"), Vec::new()),
                    }
                }
            },
            None => (1, false, Some("index_root_missing"), Vec::new()),
        };
    Ok(queries::StatusResponse {
        db_path,
        db_path_error,
        symbol_count,
        file_count,
        freshness_basis: "path_and_mtime",
        stale_files,
        stale_files_truncated,
        freshness_error,
        unindexable_sources,
        extractor_contract_current,
        concept_contract_current,
    })
}

/// The one rule for whether structural answers may be trusted: every
/// indexable path matches the index and the extractor contract is current.
/// A failed freshness scan reports `stale_files: 1`, so it counts as stale.
pub(crate) fn structurally_fresh(status: &queries::StatusResponse) -> bool {
    status.stale_files == 0 && status.freshness_error.is_none() && status.extractor_contract_current
}

enum Refresh {
    /// The store has no managed repository authority, so nothing ran.
    Unavailable,
    Completed,
    /// Indexing ran but left failed files; the index is still not fresh.
    Incomplete,
}

fn refresh(store: &mut Store) -> Result<Refresh, Error> {
    let Some(root) = store.managed_root().map(Path::to_path_buf) else {
        return Ok(Refresh::Unavailable);
    };
    #[cfg(test)]
    REFRESH_TEST_CALLS.with(|calls| calls.set(calls.get() + 1));
    let forced = !store.extractor_contract_current().unwrap_or(false)
        || !store.concept_contract_current().unwrap_or(false);
    let indexed = crate::indexer::Indexer::new(root).index_all_with_limits(
        store,
        false,
        crate::indexer::IndexLimits::AUTO_REFRESH,
    );
    // The deadline can stop indexing inside a SQLite write, which surfaces as
    // a generic error, so the budget marker decides, not the error variant.
    let out_of_budget = matches!(
        store.interrupt_source(),
        Some(crate::store::InterruptSource::Budget)
    );
    match indexed {
        Ok(stats) if stats.files_failed == 0 => Ok(Refresh::Completed),
        Err(crate::indexer::IndexError::LimitExceeded { dimension, cap }) => {
            Err(Error::RefreshLimit { dimension, cap })
        }
        _ if forced && out_of_budget => Err(Error::RebuildRequired),
        Err(crate::indexer::IndexError::Cancelled)
        | Err(crate::indexer::IndexError::DeadlineExceeded) => Err(Error::WorkLimitExceeded),
        _ => Ok(Refresh::Incomplete),
    }
}

/// Run `attempt`; when it reports a stale index, refresh a managed index once
/// and run it again. This is the only refresh-and-retry loop for CLI and MCP.
pub(crate) fn with_fresh_index<T>(
    store: &mut Store,
    mut attempt: impl FnMut(&mut Store) -> Result<T, Error>,
) -> Result<T, Error> {
    match attempt(store) {
        Err(Error::Stale(mut stale)) if !stale.refresh_attempted => match refresh(store)? {
            Refresh::Unavailable => Err(Error::Stale(stale)),
            Refresh::Incomplete => {
                stale.refresh_attempted = true;
                Err(Error::Stale(stale))
            }
            Refresh::Completed => attempt(store).map_err(attempted),
        },
        result => result,
    }
}

pub(crate) fn admission(
    store: &Store,
    concept: bool,
    limits: crate::indexer::IndexLimits,
) -> Result<queries::StatusResponse, Error> {
    match store.schema_current() {
        Ok(true) => {}
        Ok(false) => return Err(Error::SchemaIncompatible),
        Err(error) => return Err(Error::internal("schema_compatibility_query", error)),
    }
    let status = index_status_with_limits(store, limits)?;
    let reason = if !structurally_fresh(&status) {
        Some(StaleReason::Structural)
    } else if concept && !status.concept_contract_current {
        Some(StaleReason::Concept)
    } else {
        None
    };
    match reason {
        Some(reason) => Err(Error::Stale(Box::new(StaleIndex {
            status,
            reason,
            refresh_attempted: false,
        }))),
        None => Ok(status),
    }
}

/// CLI view of the same decision: `None` when structural answers are current,
/// otherwise a sentence saying why they are not. The CLI checks the whole
/// inventory, as `mastermind index .` indexes it without caps.
pub fn stale_index_reason(store: &Store) -> Option<String> {
    match admission(store, false, crate::indexer::IndexLimits::MANUAL) {
        Ok(_) => None,
        Err(Error::Stale(stale)) => Some(describe_stale(&stale.status)),
        Err(Error::SchemaIncompatible) => Some("index schema is missing or outdated".into()),
        Err(Error::SnapshotChanged) => Some("index changed while its freshness was checked".into()),
        Err(Error::RefreshLimit { dimension, cap }) => Some(format!(
            "freshness could not be checked: {dimension} exceeds {cap}"
        )),
        Err(Error::Internal { class }) => Some(format!("freshness could not be checked ({class})")),
        Err(_) => Some("freshness could not be checked".into()),
    }
}

fn describe_stale(status: &queries::StatusResponse) -> String {
    if let Some(error) = status.freshness_error {
        return format!("freshness could not be established ({error})");
    }
    if status.stale_files > 0 {
        let more = if status.stale_files_truncated {
            "+"
        } else {
            ""
        };
        let mut reason = format!(
            "{}{more} indexable files changed since the last index",
            status.stale_files
        );
        if !status.unindexable_sources.is_empty() {
            reason.push_str(&format!(
                "; not regular files, never index: {}",
                crate::terminal::escape(&status.unindexable_sources.join(", "))
            ));
        }
        return reason;
    }
    "the index was built by an older extractor".into()
}

fn attempted(error: Error) -> Error {
    match error {
        Error::Stale(mut stale) => {
            stale.refresh_attempted = true;
            Error::Stale(stale)
        }
        error => error,
    }
}

fn concept_admission(store: &Store) -> Result<u64, Error> {
    let version = store.data_version().map_err(|_| Error::SnapshotChanged)?;
    admission(store, true, crate::indexer::IndexLimits::AUTO_REFRESH)?;
    Ok(version)
}

fn current_concept_admission(store: &mut Store) -> Result<u64, Error> {
    with_fresh_index(store, |store| concept_admission(store))
}

pub(crate) fn concept_current(
    store: &mut Store,
    query: &str,
    top: u32,
    include_tests: bool,
) -> Result<queries::ConceptResponse, Error> {
    queries::validate_concept_request(query, top).map_err(Error::Concept)?;
    let version = current_concept_admission(store)?;
    queries::concept_verified_with_tests(
        store,
        query,
        top,
        include_tests,
        version,
        queries::ConceptFreshness {
            status: "fresh".into(),
            extractor_contract: crate::indexer::EXTRACTOR_CONTRACT_VERSION.into(),
            normalization_contract: crate::store::CONCEPT_NORMALIZATION_VERSION.into(),
        },
    )
    .map_err(Error::Concept)
}

pub fn build_concept_current(
    store: &mut Store,
    query: &str,
    top: u32,
) -> Result<queries::ConceptResponse, queries::ConceptError> {
    build_concept_current_with_tests(store, query, top, false)
}

pub fn build_concept_current_with_tests(
    store: &mut Store,
    query: &str,
    top: u32,
    include_tests: bool,
) -> Result<queries::ConceptResponse, queries::ConceptError> {
    concept_current(store, query, top, include_tests).map_err(Error::into_concept)
}

fn brief_attempt(
    store: &Store,
    root: &Path,
    since: &str,
    role: queries::BriefRole,
    budget_tokens: u32,
    sizer: &queries::BriefEnvelopeSizer<'_>,
) -> Result<queries::BriefPacket, Error> {
    let status = admission(store, false, crate::indexer::IndexLimits::AUTO_REFRESH)?;
    queries::brief_detailed(store, root, since, role, budget_tokens, sizer).map_err(|error| {
        match error {
            queries::BriefFailure::HistoryStale => Error::Stale(Box::new(StaleIndex {
                status,
                reason: StaleReason::History,
                refresh_attempted: false,
            })),
            queries::BriefFailure::Query(queries::BriefError::IndexStale) => {
                Error::Stale(Box::new(StaleIndex {
                    status,
                    reason: StaleReason::Structural,
                    refresh_attempted: false,
                }))
            }
            queries::BriefFailure::Query(error) => Error::Brief(error),
        }
    })
}

pub(crate) fn brief_current(
    store: &mut Store,
    root: &Path,
    since: &str,
    role: queries::BriefRole,
    budget_tokens: u32,
    sizer: &queries::BriefEnvelopeSizer<'_>,
) -> Result<queries::BriefPacket, Error> {
    queries::validate_brief_request(since, budget_tokens).map_err(Error::Brief)?;
    let root = root
        .canonicalize()
        .map_err(|_| Error::Brief(queries::BriefError::RootMismatch))?;
    let indexed_root = store
        .meta_value("index_root")
        .map_err(|_| Error::Brief(queries::BriefError::IndexStale))?
        .map(PathBuf::from)
        .ok_or(Error::Brief(queries::BriefError::IndexStale))?
        .canonicalize()
        .map_err(|_| Error::Brief(queries::BriefError::RootMismatch))?;
    if root != indexed_root {
        return Err(Error::Brief(queries::BriefError::RootMismatch));
    }
    let interrupted = || store.work_interrupted();
    crate::diff::validate_commit_ref_controlled(
        &root,
        since,
        store.request_deadline(),
        Some(&interrupted),
    )
    .map_err(|error| {
        Error::Brief(match error {
            crate::diff::WorkingTreeDiffError::InvalidRef
            | crate::diff::WorkingTreeDiffError::GitUnavailable => queries::BriefError::InvalidRef,
            crate::diff::WorkingTreeDiffError::SnapshotChanged => {
                queries::BriefError::SnapshotChanged
            }
            crate::diff::WorkingTreeDiffError::GitTimeout
            | crate::diff::WorkingTreeDiffError::GitOutputLimit => {
                queries::BriefError::WorkLimitExceeded
            }
            crate::diff::WorkingTreeDiffError::IndexStale => queries::BriefError::IndexStale,
        })
    })?;
    with_fresh_index(store, |store| {
        brief_attempt(store, &root, since, role, budget_tokens, sizer)
    })
}

pub fn build_brief_current(
    store: &mut Store,
    root: &Path,
    since: &str,
    role: queries::BriefRole,
    budget_tokens: u32,
    sizer: &queries::BriefEnvelopeSizer<'_>,
) -> Result<queries::BriefPacket, queries::BriefError> {
    brief_current(store, root, since, role, budget_tokens, sizer).map_err(Error::into_brief)
}

pub(crate) fn review_current(
    store: &mut Store,
    query: &str,
    top: u32,
    include_tests: bool,
    hints: &[String],
    contract: Option<&crate::concept_review::RequestContract>,
) -> Result<crate::concept_review::ReviewPacket, Error> {
    crate::concept_review::validate_request(query, top, hints).map_err(Error::Review)?;
    if let Some(contract) = contract {
        crate::concept_review::validate_request_contract(query, contract).map_err(Error::Review)?;
    }
    current_concept_admission(store)?;
    let root = store
        .serve_root()
        .ok_or_else(|| Error::Review("concept_review_root_unavailable".into()))?
        .to_path_buf();
    let packet = crate::concept_review::prepare_with_contract(
        store,
        &root,
        query,
        top,
        include_tests,
        hints,
        contract,
    )
    .map_err(Error::Review)?;
    review_still_current(store)?;
    Ok(packet)
}

pub(crate) fn review_finish_current(
    store: &mut Store,
    packet: &crate::concept_review::ReviewPacket,
    assessment: &crate::concept_review::Assessment,
) -> Result<crate::concept_review::ReviewResult, Error> {
    current_concept_admission(store)?;
    let root = store
        .serve_root()
        .ok_or_else(|| Error::Review("concept_review_root_unavailable".into()))?
        .to_path_buf();
    let result =
        crate::concept_review::finish(store, &root, packet, assessment).map_err(Error::Review)?;
    review_still_current(store)?;
    Ok(result)
}

fn review_still_current(store: &Store) -> Result<(), Error> {
    let status = index_status(store)
        .map_err(|_| Error::Review("concept_review_freshness_unavailable".into()))?;
    if !structurally_fresh(&status) || !status.concept_contract_current {
        return Err(Error::Review("concept_review_source_changed".into()));
    }
    Ok(())
}

pub fn build_concept_review_current(
    store: &mut Store,
    query: &str,
    top: u32,
    include_tests: bool,
) -> Result<crate::concept_review::ReviewPacket, String> {
    build_concept_review_current_with_hints(store, query, top, include_tests, &[])
}

pub fn build_concept_review_current_with_hints(
    store: &mut Store,
    query: &str,
    top: u32,
    include_tests: bool,
    hints: &[String],
) -> Result<crate::concept_review::ReviewPacket, String> {
    build_concept_review_current_with_contract(store, query, top, include_tests, hints, None)
}

pub fn build_concept_review_current_with_contract(
    store: &mut Store,
    query: &str,
    top: u32,
    include_tests: bool,
    hints: &[String],
    contract: Option<&crate::concept_review::RequestContract>,
) -> Result<crate::concept_review::ReviewPacket, String> {
    review_current(store, query, top, include_tests, hints, contract).map_err(Error::review_code)
}

pub fn finish_concept_review_current(
    store: &mut Store,
    packet: &crate::concept_review::ReviewPacket,
    assessment: &crate::concept_review::Assessment,
) -> Result<crate::concept_review::ReviewResult, String> {
    review_finish_current(store, packet, assessment).map_err(Error::review_code)
}
