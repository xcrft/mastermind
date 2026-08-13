use super::{
    ApiSurfaceChange, BoundaryCrossing, CheckOptions, DependencyEdge, EvidenceGap, ImpactRelation,
    PolicyChangedFile, PolicyConfig, PolicyError, PolicyInput, PolicyRuleKind, RelatedTest,
    FAMILY_API, FAMILY_CYCLES, FAMILY_IMPACT, FAMILY_IMPORT_GRAPH, FAMILY_OWNERSHIP, FAMILY_TESTS,
    FAMILY_WORKFLOW,
};
use crate::diff::{run_bounded_git_with_limit, WorkingTreeDiffError};
use crate::evidence::{EvidenceExtensionOptions, EvidenceOptions};
use crate::indexer::{extractor_for_path, parse_blob, MAX_INDEXABLE_FILE_SIZE};
use crate::queries::{ChangeImpactResponse, ImpactEngine};
use crate::run_task::RunState;
use crate::store::{PendingFile, Store};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const IMPORT_EDGE_LIMIT: usize = 50_000;
const BASELINE_CYCLE_FILE_LIMIT: usize = 500;
const BASELINE_CYCLE_BYTE_LIMIT: usize = 32 * 1024 * 1024;
const WORKFLOW_TASK_LIMIT: usize = 1_000;
const WORKFLOW_ARTIFACT_BYTE_LIMIT: u64 = 1024 * 1024;

pub(super) fn collect(
    store: &Store,
    root: &Path,
    config: &PolicyConfig,
    options: &CheckOptions,
    impact_engine: &ImpactEngine<'_>,
) -> Result<PolicyInput, PolicyError> {
    crate::lens::validate_index_snapshot(store, root, None)
        .map_err(|error| PolicyError::new("policy_evidence_unavailable", error.code()))?;
    let impact = impact_engine(store, root, &options.since, options.depth, options.top)
        .map_err(|error| PolicyError::new("policy_evidence_unavailable", error.code()))?;
    let mut gaps = impact_gaps(&impact, config);
    let changed_files = impact
        .changes
        .files
        .items
        .iter()
        .map(|file| PolicyChangedFile {
            path: file.path.clone(),
            status: file.status.clone(),
        })
        .collect::<Vec<_>>();
    let changed_paths = changed_files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    let relevant_workflow_paths = changed_paths
        .iter()
        .filter(|path| {
            config.rules.iter().any(|rule| {
                matches!(
                    &rule.kind,
                    PolicyRuleKind::StrictWorkflow { critical } if critical.matches(path)
                )
            })
        })
        .cloned()
        .collect::<BTreeSet<_>>();

    let needs_import_graph = config.rules.iter().any(|rule| {
        matches!(
            rule.kind,
            PolicyRuleKind::DependencyDirection { .. } | PolicyRuleKind::NewCycles { .. }
        )
    });
    let mut dependency_edges = Vec::new();
    let mut new_cycles = Vec::new();
    if needs_import_graph {
        let (pairs, truncated) = store
            .map_import_edges_capped_filtered("", "root", IMPORT_EDGE_LIMIT, false)
            .map_err(|_| {
                PolicyError::new(
                    "policy_evidence_unavailable",
                    "cannot read the indexed import graph",
                )
            })?;
        if truncated {
            for family in [FAMILY_IMPORT_GRAPH, FAMILY_CYCLES] {
                gaps.push(gap(
                    family,
                    "import_graph_work_limit",
                    "The repository import graph exceeds the 50,000-edge policy work limit.",
                ));
            }
        } else {
            dependency_edges = pairs
                .iter()
                .map(|(from, to)| DependencyEdge {
                    from: from.clone(),
                    to: to.clone(),
                })
                .collect();
            if config
                .rules
                .iter()
                .any(|rule| matches!(rule.kind, PolicyRuleKind::NewCycles { .. }))
            {
                let current_cycles = crate::queries::map_cycle_components(&pairs);
                match compare_cycles_to_baseline(
                    root,
                    &impact.baseline.baseline_oid,
                    config,
                    &changed_paths,
                    current_cycles,
                ) {
                    Ok(cycles) => new_cycles = cycles,
                    Err(message) => gaps.push(gap(
                        FAMILY_CYCLES,
                        "baseline_cycle_evidence_incomplete",
                        message,
                    )),
                }
            }
        }
    }

    let api_surface_changes = api_surface_changes(&impact);
    let impact_relations = impact_relations(&impact);
    let boundary_crossings = impact
        .api_crossings
        .items
        .iter()
        .map(|crossing| BoundaryCrossing {
            seed_file: crossing.seed.file.clone(),
            seed_line: crossing.seed.line,
            seed_component: crossing.changed_component.clone(),
            impacted_file: crossing.impacted.file.clone(),
            impacted_line: crossing.impacted.line,
            impacted_component: crossing.impacted_component.clone(),
        })
        .collect();
    let related_tests = impact
        .tests
        .items
        .iter()
        .map(|candidate| {
            let mut seeds = BTreeSet::new();
            for evidence in &candidate.evidence {
                if let Some(seed) = &evidence.seed {
                    seeds.insert(seed.file.clone());
                }
            }
            RelatedTest {
                file: candidate.symbol.file.clone(),
                related_seed_files: seeds.into_iter().collect(),
            }
        })
        .collect();

    let needs_ownership = config.rules.iter().any(|rule| {
        matches!(
            rule.kind,
            PolicyRuleKind::ApiOwner { .. } | PolicyRuleKind::OwnershipBoundary { .. }
        )
    });
    let owners = if needs_ownership {
        let snapshot = crate::evidence::collect_with_store(
            root,
            &EvidenceOptions {
                codeowners: options.codeowners.clone(),
                discover_codeowners: options.codeowners.is_none(),
                ..EvidenceOptions::default()
            },
            &EvidenceExtensionOptions::default(),
            &impact,
            store,
            None,
        );
        if snapshot.partial {
            let detail = snapshot
                .diagnostics
                .items
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ");
            gaps.push(gap(
                FAMILY_OWNERSHIP,
                "codeowners_evidence_incomplete",
                if detail.is_empty() {
                    "CODEOWNERS evidence was incomplete.".to_string()
                } else {
                    format!("CODEOWNERS evidence was incomplete: {detail}.")
                },
            ));
            gaps.push(gap(
                FAMILY_API,
                "codeowners_evidence_incomplete",
                "Required-owner evaluation could not read complete CODEOWNERS evidence.",
            ));
        }
        snapshot
            .files
            .items
            .into_iter()
            .filter_map(|file| {
                file.ownership
                    .map(|ownership| (file.path, ownership.codeowners))
            })
            .collect()
    } else {
        BTreeMap::new()
    };

    let needs_workflow = config
        .rules
        .iter()
        .any(|rule| matches!(rule.kind, PolicyRuleKind::StrictWorkflow { .. }));
    let strict_workflow_files = if needs_workflow {
        let (files, workflow_gaps) = collect_workflow_evidence(
            root,
            &options.workflow_evidence_path,
            &impact.baseline.baseline_oid,
            &relevant_workflow_paths,
        );
        gaps.extend(workflow_gaps);
        files
    } else {
        BTreeMap::new()
    };

    crate::lens::validate_index_snapshot(store, root, None)
        .map_err(|error| PolicyError::new("policy_snapshot_changed", error.code()))?;
    let rechecked = impact_engine(store, root, &options.since, options.depth, options.top)
        .map_err(|error| PolicyError::new("policy_snapshot_changed", error.code()))?;
    let first = serde_json::to_value(&impact)
        .map_err(|_| PolicyError::new("policy_evidence_unavailable", "serialize impact"))?;
    let second = serde_json::to_value(&rechecked)
        .map_err(|_| PolicyError::new("policy_evidence_unavailable", "serialize impact"))?;
    if impact.snapshot_token != rechecked.snapshot_token || first != second {
        return Err(PolicyError::new(
            "policy_snapshot_changed",
            "repository or index evidence changed during evaluation",
        ));
    }

    gaps.sort();
    gaps.dedup();
    Ok(PolicyInput {
        baseline: impact.baseline,
        changed_files,
        dependency_edges,
        new_cycles,
        api_surface_changes,
        impact_relations,
        boundary_crossings,
        related_tests,
        owners,
        strict_workflow_files,
        gaps,
    })
}

fn impact_gaps(response: &ChangeImpactResponse, config: &PolicyConfig) -> Vec<EvidenceGap> {
    let active = config
        .rules
        .iter()
        .map(|rule| rule.kind.family())
        .collect::<BTreeSet<_>>();
    let mut gaps = Vec::new();
    if response.changes.files.truncated {
        for family in [
            FAMILY_IMPORT_GRAPH,
            FAMILY_CYCLES,
            FAMILY_API,
            FAMILY_IMPACT,
            FAMILY_TESTS,
            FAMILY_OWNERSHIP,
            FAMILY_WORKFLOW,
        ] {
            if active.contains(family) {
                gaps.push(gap(
                    family,
                    "changed_file_limit",
                    "The changed-file set exceeded the change-impact work limit.",
                ));
            }
        }
    }
    if response.changes.symbols.truncated {
        for family in [FAMILY_API, FAMILY_IMPACT, FAMILY_TESTS, FAMILY_OWNERSHIP] {
            if active.contains(family) {
                gaps.push(gap(
                    family,
                    "changed_symbol_limit",
                    "The changed-symbol set exceeded the change-impact work limit.",
                ));
            }
        }
    }
    if response.impact.truncated && active.contains(FAMILY_IMPACT) {
        gaps.push(gap(
            FAMILY_IMPACT,
            response
                .impact
                .truncation_reason
                .as_deref()
                .unwrap_or("impact_limit"),
            "Static impact results were truncated before the blast radius was complete.",
        ));
    }
    if response.api_crossings.truncated {
        for family in [FAMILY_API, FAMILY_OWNERSHIP] {
            if active.contains(family) {
                gaps.push(gap(
                    family,
                    response
                        .api_crossings
                        .truncation_reason
                        .as_deref()
                        .unwrap_or("crossing_limit"),
                    "Cross-component API evidence was truncated.",
                ));
            }
        }
    }
    if response.tests.truncated && active.contains(FAMILY_TESTS) {
        gaps.push(gap(
            FAMILY_TESTS,
            response
                .tests
                .truncation_reason
                .as_deref()
                .unwrap_or("test_limit"),
            "Related-test candidates were truncated.",
        ));
    }
    gaps
}

fn gap(
    family: impl Into<String>,
    code: impl Into<String>,
    message: impl Into<String>,
) -> EvidenceGap {
    EvidenceGap {
        family: family.into(),
        code: code.into(),
        message: message.into(),
    }
}

fn api_surface_changes(response: &ChangeImpactResponse) -> Vec<ApiSurfaceChange> {
    let mut changes = BTreeMap::new();
    for crossing in &response.api_crossings.items {
        changes
            .entry((
                crossing.seed.file.clone(),
                crossing.seed.line,
                crossing.seed.name.clone(),
                crossing.seed.kind.clone(),
            ))
            .or_insert_with(|| ApiSurfaceChange {
                file: crossing.seed.file.clone(),
                line: crossing.seed.line,
                name: crossing.seed.name.clone(),
                kind: crossing.seed.kind.clone(),
                component: crossing.changed_component.clone(),
            });
    }
    changes.into_values().collect()
}

fn impact_relations(response: &ChangeImpactResponse) -> Vec<ImpactRelation> {
    let mut relations = Vec::new();
    for impacted in &response.impact.items {
        for seed in &impacted.seeds {
            relations.push(ImpactRelation {
                seed_file: seed.file.clone(),
                seed_line: seed.line,
                seed_name: seed.name.clone(),
                impacted_file: impacted.symbol.file.clone(),
                impacted_line: impacted.symbol.line,
                impacted_name: impacted.symbol.name.clone(),
                minimum_depth: impacted.minimum_depth,
            });
        }
    }
    relations.sort_by(|left, right| {
        (
            &left.seed_file,
            left.seed_line,
            &left.impacted_file,
            left.impacted_line,
            &left.impacted_name,
        )
            .cmp(&(
                &right.seed_file,
                right.seed_line,
                &right.impacted_file,
                right.impacted_line,
                &right.impacted_name,
            ))
    });
    relations
}

fn compare_cycles_to_baseline(
    root: &Path,
    baseline_oid: &str,
    config: &PolicyConfig,
    changed_paths: &BTreeSet<String>,
    current_cycles: Vec<Vec<String>>,
) -> Result<Vec<Vec<String>>, String> {
    let scopes = config
        .rules
        .iter()
        .filter_map(|rule| match &rule.kind {
            PolicyRuleKind::NewCycles { scope, .. } => Some(scope),
            _ => None,
        })
        .collect::<Vec<_>>();
    let candidates = current_cycles
        .into_iter()
        .filter(|cycle| cycle.iter().any(|path| changed_paths.contains(path)))
        .filter(|cycle| {
            scopes
                .iter()
                .any(|scope| cycle.iter().any(|path| scope.matches(path)))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let members = candidates
        .iter()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    if members.len() > BASELINE_CYCLE_FILE_LIMIT {
        return Err(format!(
            "Cycle baseline comparison needs {} files, above the {}-file limit.",
            members.len(),
            BASELINE_CYCLE_FILE_LIMIT
        ));
    }
    if members
        .iter()
        .any(|path| path.chars().any(char::is_control))
    {
        return Err("Cycle paths containing control characters cannot be compared safely.".into());
    }
    let paths = members.into_iter().collect::<Vec<_>>();
    let blobs = baseline_blobs(root, baseline_oid, &paths)
        .map_err(|error| format!("Cannot read baseline cycle members: {}.", error_code(error)))?;
    let mut files = BTreeMap::new();
    for path in &paths {
        let Some(bytes) = blobs.get(path).and_then(Option::as_ref) else {
            continue;
        };
        if bytes.len() as u64 > MAX_INDEXABLE_FILE_SIZE {
            return Err(format!(
                "Baseline cycle member `{path}` exceeds the source size limit."
            ));
        }
        let extractor = extractor_for_path(Path::new(path))
            .ok_or_else(|| format!("Baseline cycle member `{path}` has no extractor."))?;
        let pending = parse_blob(path, bytes, 0, extractor.as_ref())
            .map_err(|_| format!("Baseline cycle member `{path}` could not be parsed."))?;
        files.insert(path.clone(), pending);
    }
    let baseline_edges = import_edges_from_pending(&files);
    let baseline_cycles = crate::queries::map_cycle_components(&baseline_edges)
        .into_iter()
        .collect::<BTreeSet<_>>();
    Ok(candidates
        .into_iter()
        .filter(|cycle| !baseline_cycles.contains(cycle))
        .collect())
}

fn baseline_blobs(
    root: &Path,
    baseline_oid: &str,
    paths: &[String],
) -> Result<BTreeMap<String, Option<Vec<u8>>>, WorkingTreeDiffError> {
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(baseline_oid.as_bytes());
        input.push(b':');
        input.extend_from_slice(path.as_bytes());
        input.push(b'\n');
    }
    let output = run_bounded_git_with_limit(
        root,
        &["cat-file", "--batch"],
        Some(&input),
        BASELINE_CYCLE_BYTE_LIMIT,
    )?;
    if !output.success {
        return Err(WorkingTreeDiffError::InvalidRef);
    }
    let mut cursor = 0usize;
    let mut blobs = BTreeMap::new();
    for path in paths {
        let relative_newline = output
            .stdout
            .get(cursor..)
            .and_then(|remaining| remaining.iter().position(|byte| *byte == b'\n'))
            .ok_or(WorkingTreeDiffError::InvalidRef)?;
        let header_end = cursor + relative_newline;
        let header = &output.stdout[cursor..header_end];
        cursor = header_end + 1;
        if header.ends_with(b" missing") {
            blobs.insert(path.clone(), None);
            continue;
        }
        let header = std::str::from_utf8(header).map_err(|_| WorkingTreeDiffError::InvalidRef)?;
        let size = header
            .split_whitespace()
            .nth(2)
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or(WorkingTreeDiffError::InvalidRef)?;
        let end = cursor
            .checked_add(size)
            .filter(|end| *end < output.stdout.len())
            .ok_or(WorkingTreeDiffError::GitOutputLimit)?;
        blobs.insert(path.clone(), Some(output.stdout[cursor..end].to_vec()));
        cursor = end + 1;
    }
    Ok(blobs)
}

fn error_code(error: WorkingTreeDiffError) -> &'static str {
    error.code()
}

fn import_edges_from_pending(files: &BTreeMap<String, PendingFile>) -> Vec<(String, String)> {
    let mut targets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, file) in files {
        for symbol in &file.symbols {
            targets
                .entry(symbol.name.clone())
                .or_default()
                .push(path.clone());
        }
    }
    let paths = files.keys().cloned().collect::<BTreeSet<_>>();
    let mut pairs = BTreeSet::new();
    for (source, file) in files {
        for edge in file.edges.iter().filter(|edge| edge.kind == "imports") {
            if file.language == "cpp" {
                if let Some(encoded) = &edge.to_path {
                    for target in resolve_cpp_include(source, encoded, &paths) {
                        if &target != source {
                            pairs.insert((source.clone(), target));
                        }
                    }
                }
            } else if let Some(resolved) = targets.get(&edge.to_name) {
                for target in resolved {
                    if target != source {
                        pairs.insert((source.clone(), target.clone()));
                    }
                }
            }
        }
    }
    pairs.into_iter().collect()
}

fn resolve_cpp_include(source: &str, encoded: &str, paths: &BTreeSet<String>) -> BTreeSet<String> {
    let include = encoded
        .strip_suffix("::*")
        .unwrap_or(encoded)
        .replace('\\', "/");
    let mut resolved = BTreeSet::new();
    if let Some(path) = normalize_relative(&include) {
        if paths.contains(&path) {
            resolved.insert(path);
        }
    }
    if let Some(parent) = source.rsplit_once('/').map(|(parent, _)| parent) {
        if let Some(path) = normalize_relative(&format!("{parent}/{include}")) {
            if paths.contains(&path) {
                resolved.insert(path);
            }
        }
    }
    if resolved.is_empty() {
        let lower = include.to_ascii_lowercase();
        let basename = lower.rsplit('/').next().unwrap_or(&lower);
        for candidate in paths {
            let candidate_lower = candidate.to_ascii_lowercase();
            if candidate_lower.rsplit('/').next() == Some(basename)
                && (!lower.contains('/')
                    || candidate_lower == lower
                    || candidate_lower.ends_with(&format!("/{lower}")))
            {
                resolved.insert(candidate.clone());
            }
        }
    }
    resolved
}

fn normalize_relative(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            value => parts.push(value),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn collect_workflow_evidence(
    root: &Path,
    requested: &Path,
    baseline_oid: &str,
    relevant_files: &BTreeSet<String>,
) -> (BTreeMap<String, Vec<String>>, Vec<EvidenceGap>) {
    if relevant_files.is_empty() {
        return (BTreeMap::new(), Vec::new());
    }
    let path = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    if !path.exists() {
        return (BTreeMap::new(), Vec::new());
    }
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => metadata,
        _ => {
            return (
                BTreeMap::new(),
                vec![gap(
                    FAMILY_WORKFLOW,
                    "workflow_evidence_invalid",
                    "Workflow evidence must be a real directory, not a symlink.",
                )],
            )
        }
    };
    let _ = metadata;
    let mut task_dirs = match std::fs::read_dir(&path) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>(),
        Err(_) => {
            return (
                BTreeMap::new(),
                vec![gap(
                    FAMILY_WORKFLOW,
                    "workflow_evidence_unreadable",
                    "Workflow evidence directory could not be read.",
                )],
            )
        }
    };
    task_dirs.sort();
    let mut gaps = Vec::new();
    if task_dirs.len() > WORKFLOW_TASK_LIMIT {
        task_dirs.truncate(WORKFLOW_TASK_LIMIT);
        gaps.push(gap(
            FAMILY_WORKFLOW,
            "workflow_task_limit",
            "Workflow evidence exceeded the 1,000-task work limit.",
        ));
    }

    let mut files: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for task_dir in task_dirs {
        if !is_real_directory(&task_dir) {
            continue;
        }
        let spec_path = task_dir.join("spec.md");
        let state_path = task_dir.join("state.json");
        let audit_path = task_dir.join("audit.md");
        let Some(spec_body) = read_small_regular_file(&spec_path) else {
            continue;
        };
        let parsed = crate::spec::parse_str(&spec_path.to_string_lossy(), &spec_body);
        let Some(frontmatter) = parsed.frontmatter else {
            continue;
        };
        if frontmatter.mode.as_deref() != Some("strict") {
            continue;
        }
        let touches = frontmatter
            .touches
            .iter()
            .filter_map(|touch| normalize_evidence_path(&touch.file))
            .collect::<BTreeSet<_>>();
        if touches.is_disjoint(relevant_files) {
            continue;
        }
        let Some(state_body) = read_small_regular_file(&state_path) else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<RunState>(&state_body) else {
            continue;
        };
        if state.baseline_ref != baseline_oid
            || !matches!(state.status.as_str(), "history_review_required" | "learned")
            || state.spec_hash != crate::run_task::hash_text(&spec_body)
        {
            continue;
        }
        if !state_spec_matches(&state.spec_path, &spec_path, &task_dir) {
            continue;
        }
        let Some(audit) = read_small_regular_file(&audit_path) else {
            continue;
        };
        if !audit
            .lines()
            .next()
            .is_some_and(|line| line.starts_with("✅ Held"))
        {
            continue;
        }
        let Some(expected_snapshot) = state.held_snapshot_sha256.as_deref() else {
            continue;
        };
        let touch_files = touches.iter().cloned().collect::<Vec<_>>();
        match crate::run_task::strict_workflow_snapshot(root, baseline_oid, &touch_files) {
            Ok(current_snapshot) if current_snapshot == expected_snapshot => {}
            Ok(_) => continue,
            Err(message) => {
                gaps.push(gap(
                    FAMILY_WORKFLOW,
                    "workflow_snapshot_unavailable",
                    message,
                ));
                continue;
            }
        }
        let evidence_path = task_dir
            .strip_prefix(root)
            .unwrap_or(&task_dir)
            .to_string_lossy()
            .replace('\\', "/");
        for path in touches.intersection(relevant_files) {
            files
                .entry(path.clone())
                .or_default()
                .push(evidence_path.clone());
        }
    }
    for paths in files.values_mut() {
        paths.sort();
        paths.dedup();
    }
    (files, gaps)
}

fn state_spec_matches(state_spec: &str, current_spec: &Path, task_dir: &Path) -> bool {
    if Path::new(state_spec)
        .canonicalize()
        .ok()
        .zip(current_spec.canonicalize().ok())
        .is_some_and(|(state, current)| state == current)
    {
        return true;
    }
    let Some(task_id) = task_dir.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let normalized = state_spec.replace('\\', "/");
    let suffix = format!(".mastermind/tasks/{task_id}/spec.md");
    normalized == suffix || normalized.ends_with(&format!("/{suffix}"))
}

fn is_real_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

fn read_small_regular_file(path: &Path) -> Option<String> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
        || metadata.len() > WORKFLOW_ARTIFACT_BYTE_LIMIT
    {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn normalize_evidence_path(path: &str) -> Option<String> {
    let path = path.trim().replace('\\', "/");
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with("//")
        || path.as_bytes().get(1) == Some(&b':')
        || path
            .split('/')
            .any(|part| part == ".." || part.chars().any(char::is_control))
    {
        None
    } else {
        Some(
            path.split('/')
                .filter(|part| !part.is_empty() && *part != ".")
                .collect::<Vec<_>>()
                .join("/"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs;

    #[test]
    fn held_strict_workflow_evidence_is_bound_to_baseline_and_touched_file() {
        let root = tempfile::tempdir().unwrap();
        run(root.path(), &["init", "-b", "main"]);
        run(root.path(), &["config", "user.email", "policy@example.com"]);
        run(root.path(), &["config", "user.name", "Policy Test"]);
        let task = root.path().join(".mastermind/tasks/001-payment");
        let source = root.path().join("services/payment/charge.ts");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "export function charge() { return 0; }\n").unwrap();
        run(root.path(), &["add", "."]);
        run(root.path(), &["commit", "-m", "baseline"]);
        let baseline = output(root.path(), &["rev-parse", "HEAD"]);
        fs::write(&source, "export function charge() { return 1; }\n").unwrap();
        fs::create_dir_all(&task).unwrap();
        let spec_body =
            "---\nmode: strict\ntouches:\n  - file: services/payment/charge.ts\n---\n# Payment\n";
        fs::write(task.join("spec.md"), spec_body).unwrap();
        let touch_files = vec!["services/payment/charge.ts".to_string()];
        let held_snapshot =
            crate::run_task::strict_workflow_snapshot(root.path(), &baseline, &touch_files)
                .unwrap();
        fs::write(
            task.join("state.json"),
            serde_json::to_vec(&RunState {
                status: "learned".into(),
                risk: Some("low".into()),
                next_step: Some("close".into()),
                blocking_reason: None,
                last_artifact: Some("audit.md".into()),
                spec_path: "/original/checkout/.mastermind/tasks/001-payment/spec.md".into(),
                spec_hash: crate::run_task::hash_text(spec_body),
                baseline_ref: baseline.clone(),
                held_snapshot_sha256: Some(held_snapshot),
                started_at: 1,
                iteration: 1,
                allow_no_index: false,
            })
            .unwrap(),
        )
        .unwrap();
        fs::write(task.join("audit.md"), "✅ Held — spec.md\n").unwrap();
        let relevant = BTreeSet::from(["services/payment/charge.ts".to_string()]);

        let (files, gaps) = collect_workflow_evidence(
            root.path(),
            Path::new(".mastermind/tasks"),
            &baseline,
            &relevant,
        );
        assert!(gaps.is_empty());
        assert_eq!(
            files["services/payment/charge.ts"],
            [".mastermind/tasks/001-payment"]
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let original_mode = fs::metadata(&source).unwrap().permissions().mode();
            let mut executable = fs::metadata(&source).unwrap().permissions();
            executable.set_mode(original_mode | 0o111);
            fs::set_permissions(&source, executable).unwrap();
            let (mode_stale, _) = collect_workflow_evidence(
                root.path(),
                Path::new(".mastermind/tasks"),
                &baseline,
                &relevant,
            );
            assert!(mode_stale.is_empty());
            let mut restored = fs::metadata(&source).unwrap().permissions();
            restored.set_mode(original_mode);
            fs::set_permissions(&source, restored).unwrap();
        }

        let (wrong_baseline, _) = collect_workflow_evidence(
            root.path(),
            Path::new(".mastermind/tasks"),
            &"2".repeat(40),
            &relevant,
        );
        assert!(wrong_baseline.is_empty());

        fs::write(&source, "export function charge() { return 2; }\n").unwrap();
        let (stale, gaps) = collect_workflow_evidence(
            root.path(),
            Path::new(".mastermind/tasks"),
            &baseline,
            &relevant,
        );
        assert!(stale.is_empty());
        assert!(
            gaps.is_empty(),
            "a stale held artifact is not current evidence"
        );
    }

    #[test]
    fn baseline_graph_detects_a_new_cycle_without_checkout_mutation() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path();
        run(repo, &["init"]);
        run(repo, &["config", "user.email", "policy@example.com"]);
        run(repo, &["config", "user.name", "Policy Test"]);
        fs::write(
            repo.join("a.py"),
            "from b import b\ndef a():\n    return b()\n",
        )
        .unwrap();
        fs::write(repo.join("b.py"), "def b():\n    return 1\n").unwrap();
        run(repo, &["add", "."]);
        run(repo, &["commit", "-m", "baseline"]);
        let baseline = output(repo, &["rev-parse", "HEAD"]);
        fs::write(
            repo.join("b.py"),
            "from a import a\ndef b():\n    return a()\n",
        )
        .unwrap();
        let config =
            super::super::parse_config(br#"rules: [{id: cycles, scope: "**", max_new_cycles: 0}]"#)
                .unwrap();
        let cycles = compare_cycles_to_baseline(
            repo,
            baseline.trim(),
            &config,
            &BTreeSet::from(["b.py".into()]),
            vec![vec!["a.py".into(), "b.py".into()]],
        )
        .unwrap();
        assert_eq!(cycles, [vec!["a.py", "b.py"]]);
    }

    #[test]
    fn full_check_connects_all_policy_families_to_real_repository_evidence() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path();
        run(repo, &["init", "-b", "main"]);
        run(repo, &["config", "user.email", "policy@example.com"]);
        run(repo, &["config", "user.name", "Policy Test"]);
        for directory in [
            "src/domain",
            "src/infrastructure",
            "services/payment",
            "api",
            ".mastermind",
        ] {
            fs::create_dir_all(repo.join(directory)).unwrap();
        }
        fs::write(
            repo.join("src/infrastructure/db.py"),
            "def save():\n    return True\n",
        )
        .unwrap();
        fs::write(
            repo.join("src/domain/order.py"),
            "from src.infrastructure.db import save\ndef order():\n    return save()\n",
        )
        .unwrap();
        fs::write(
            repo.join("services/payment/a.py"),
            "from services.payment.b import b\ndef a():\n    return b()\n",
        )
        .unwrap();
        fs::write(
            repo.join("services/payment/b.py"),
            "def b():\n    return 1\n",
        )
        .unwrap();
        fs::write(
            repo.join("services/payment/charge.py"),
            "def charge():\n    return 1\n",
        )
        .unwrap();
        fs::write(
            repo.join("api/checkout.py"),
            "from services.payment.charge import charge\ndef checkout():\n    return charge()\n",
        )
        .unwrap();
        fs::write(
            repo.join("CODEOWNERS"),
            "/services/payment/ @payments\n/api/ @platform\n",
        )
        .unwrap();
        fs::write(
            repo.join("mastermind-policy.yml"),
            r#"version: 1
rules:
  - id: direction
    from: src/domain/**
    deny_imports: src/infrastructure/**
  - id: cycles
    scope: services/payment/**
    max_new_cycles: 0
  - id: api-owner
    when: api_surface_changed
    require_owner: platform
  - id: blast
    scope: services/payment/**
    max_blast_radius: 0
  - id: tests
    scope: services/payment/**
    require_tests: true
  - id: owner-boundary
    scope: services/payment/**
    deny_ownership_crossings: true
  - id: workflow
    critical: services/payment/**
    require_workflow: strict
"#,
        )
        .unwrap();
        run(repo, &["add", "."]);
        run(repo, &["commit", "-m", "baseline"]);

        let index = repo.join(".mastermind/mmcg.db");
        let mut store = Store::open(&index).unwrap();
        crate::indexer::Indexer::new(repo)
            .index_all(&mut store, true)
            .unwrap();
        let bound_path = repo.join("services/payment/charge.py");
        let bound_source = fs::read(&bound_path).unwrap();
        let fact_contract = crate::facts::contract(&store).unwrap();
        let fact_manifest = repo.join(".mastermind/policy-facts.json");
        fs::write(
            &fact_manifest,
            serde_json::to_vec(&serde_json::json!({
                "api_version": crate::facts::API_VERSION,
                "capabilities": ["annotations"],
                "repository": {
                    "identity": fact_contract.repository.identity,
                    "revision": fact_contract.repository.revision
                },
                "producer": {"name": "com.example.policy-test", "version": "1.0.0"},
                "dataset": "stale-overlay",
                "provenance": {"kind": "test", "artifacts": []},
                "files": [{
                    "path": "services/payment/charge.py",
                    "sha256": crate::hex::encode(&Sha256::digest(&bound_source)),
                    "bytes": bound_source.len()
                }],
                "artifacts": [],
                "facts": []
            }))
            .unwrap(),
        )
        .unwrap();
        crate::facts::import(&store, &fact_manifest).unwrap();
        fs::write(
            repo.join("services/payment/b.py"),
            "from services.payment.a import a\ndef b():\n    return a()\n",
        )
        .unwrap();
        fs::write(
            repo.join("services/payment/charge.py"),
            "def charge():\n    return 2\n",
        )
        .unwrap();
        fs::write(
            repo.join("src/domain/order.py"),
            "from src.infrastructure.db import save\ndef order():\n    return bool(save())\n",
        )
        .unwrap();
        crate::indexer::Indexer::new(repo)
            .index_all(&mut store, true)
            .unwrap();

        let report = super::super::check(
            &store,
            repo,
            &super::super::CheckOptions {
                since: "main".into(),
                config_path: "mastermind-policy.yml".into(),
                codeowners: None,
                workflow_evidence_path: ".mastermind/tasks".into(),
                depth: 3,
                top: 500,
            },
        )
        .unwrap();
        let violated = report
            .violations
            .iter()
            .map(|violation| violation.rule_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            violated,
            BTreeSet::from([
                "api-owner",
                "blast",
                "cycles",
                "direction",
                "owner-boundary",
                "tests",
                "workflow",
            ]),
            "violations: {:#?}; diagnostics: {:#?}",
            report.violations,
            report.diagnostics
        );
        assert!(
            report.complete,
            "a stale declarative overlay must not extend or block the closed policy DSL: {:#?}",
            report.diagnostics
        );
    }

    #[test]
    fn deletion_only_stale_index_is_rejected_before_policy_evaluation() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path();
        run(repo, &["init", "-b", "main"]);
        run(repo, &["config", "user.email", "policy@example.com"]);
        run(repo, &["config", "user.name", "Policy Test"]);
        fs::write(repo.join("a.py"), "def a():\n    return 1\n").unwrap();
        fs::write(
            repo.join("mastermind-policy.yml"),
            "rules:\n  - id: direction\n    from: src/**\n    deny_imports: vendor/**\n",
        )
        .unwrap();
        run(repo, &["add", "."]);
        run(repo, &["commit", "-m", "baseline"]);
        fs::create_dir_all(repo.join(".mastermind")).unwrap();
        let mut store = Store::open(repo.join(".mastermind/mmcg.db")).unwrap();
        crate::indexer::Indexer::new(repo)
            .index_all(&mut store, true)
            .unwrap();
        fs::remove_file(repo.join("a.py")).unwrap();

        let error = super::super::check(
            &store,
            repo,
            &super::super::CheckOptions {
                since: "main".into(),
                config_path: "mastermind-policy.yml".into(),
                codeowners: None,
                workflow_evidence_path: ".mastermind/tasks".into(),
                depth: 3,
                top: 500,
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), "policy_evidence_unavailable");
        assert!(error.to_string().contains("index_stale"));
    }

    fn run(root: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn output(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
