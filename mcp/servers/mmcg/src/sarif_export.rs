//! Deterministic SARIF 2.1 export for architectural risks observed by Mastermind.
//!
//! The exporter reports only evidence already returned by bounded map/impact
//! queries. It never upgrades a partial graph into a claim of completeness.

use crate::queries::{ChangeImpactResponse, ProjectMapResponse};
use serde_json::{json, Value};

const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";
const CYCLE_RULE: &str = "mastermind/dependency-cycle";
const CROSSING_RULE: &str = "mastermind/component-boundary-change";

pub fn project_map(map: &ProjectMapResponse) -> Value {
    let mut partial_reasons = Vec::new();
    if let Some(reason) = map.cycles.truncation_reason {
        partial_reasons.push(reason);
    }
    if map.scope.aggregation_paths_truncated {
        partial_reasons.push("path_work_limit");
    }
    partial_reasons.sort_unstable();
    partial_reasons.dedup();
    let results = map
        .cycles
        .items
        .iter()
        .filter(|cycle| !cycle.is_empty())
        .map(|cycle| {
            let locations = vec![location(&cycle[0], None)];
            let related_locations = cycle
                .iter()
                .enumerate()
                .map(|(index, path)| {
                    let mut value = location(path, None);
                    value["id"] = json!(index + 1);
                    value["message"] = json!({ "text": "Dependency-cycle member" });
                    value
                })
                .collect::<Vec<_>>();
            json!({
                "ruleId": CYCLE_RULE,
                "ruleIndex": 0,
                "level": "warning",
                "message": {
                    "text": format!(
                        "Import dependency cycle crosses {} files; review the related cycle members.",
                        cycle.len()
                    )
                },
                "locations": locations,
                "relatedLocations": related_locations,
                "properties": {
                    "cycleSize": cycle.len(),
                    "scope": map.scope.path,
                    "productionOnly": map.scope.production_only
                }
            })
        })
        .collect::<Vec<_>>();

    document(
        "mastermind/project-map/",
        vec![rule(
            CYCLE_RULE,
            "dependency-cycle",
            "Files participate in an import dependency cycle.",
            "Break or explicitly document the cyclic architecture boundary.",
        )],
        results,
        json!({
            "analysis": "project-map",
            "scope": map.scope.path,
            "productionOnly": map.scope.production_only,
            "partial": map.cycles.truncated || map.scope.aggregation_paths_truncated,
            "cyclesReturned": map.cycles.returned,
            "cyclesTotal": map.cycles.total,
            "partialReasons": partial_reasons
        }),
    )
}

pub fn change_impact(response: &ChangeImpactResponse) -> Value {
    let mut partial_reasons = [
        response.changes.files.truncation_reason.as_deref(),
        response.changes.symbols.truncation_reason.as_deref(),
        response.affected_components.truncation_reason.as_deref(),
        response.impact.truncation_reason.as_deref(),
        response.api_crossings.truncation_reason.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    partial_reasons.sort_unstable();
    partial_reasons.dedup();
    let results = response
        .api_crossings
        .items
        .iter()
        .map(|crossing| {
            json!({
                "ruleId": CROSSING_RULE,
                "ruleIndex": 0,
                "level": "warning",
                "message": {
                    "text": format!(
                        "Changed {} '{}' in component '{}' impacts {} '{}' in component '{}' at graph depth {}; review [the impacted symbol](1).",
                        safe_message_text(&crossing.seed.kind, 80),
                        safe_message_text(&crossing.seed.name, 200),
                        safe_message_text(&crossing.changed_component, 200),
                        safe_message_text(&crossing.impacted.kind, 80),
                        safe_message_text(&crossing.impacted.name, 200),
                        safe_message_text(&crossing.impacted_component, 200),
                        crossing.minimum_depth
                    )
                },
                "locations": [location(&crossing.seed.file, Some(crossing.seed.line))],
                "relatedLocations": [{
                    "id": 1,
                    "message": { "text": "Impacted symbol across the component boundary" },
                    "physicalLocation": physical_location(
                        &crossing.impacted.file,
                        Some(crossing.impacted.line)
                    )
                }],
                "properties": {
                    "changedComponent": crossing.changed_component,
                    "impactedComponent": crossing.impacted_component,
                    "minimumDepth": crossing.minimum_depth,
                    "changeKind": crossing.seed.change,
                    "baselineOid": response.baseline.baseline_oid,
                    "headOid": response.baseline.head_oid,
                    "includesWorktree": response.baseline.includes_worktree,
                    "includesUntracked": response.baseline.includes_untracked
                }
            })
        })
        .collect::<Vec<_>>();

    document(
        "mastermind/change-impact/",
        vec![rule(
            CROSSING_RULE,
            "component-boundary-change",
            "A changed symbol impacts a symbol in another inferred component.",
            "Review the downstream component contract, ownership, and test evidence.",
        )],
        results,
        json!({
            "analysis": "change-impact",
            "requestedRef": response.baseline.requested_ref,
            "baselineOid": response.baseline.baseline_oid,
            "headOid": response.baseline.head_oid,
            "partial": response.api_crossings.truncated
                || response.impact.truncated
                || response.changes.symbols.truncated
                || response.changes.files.truncated
                || response.affected_components.truncated,
            "crossingsReturned": response.api_crossings.returned,
            "crossingsTotal": response.api_crossings.total,
            "partialReasons": partial_reasons
        }),
    )
}

fn document(
    automation_id: &str,
    rules: Vec<Value>,
    results: Vec<Value>,
    properties: Value,
) -> Value {
    json!({
        "$schema": SARIF_SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "Mastermind",
                    "semanticVersion": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/xcrft/mastermind",
                    "rules": rules
                }
            },
            "automationDetails": { "id": automation_id },
            "results": results,
            "properties": properties
        }]
    })
}

fn safe_message_text(value: &str, maximum: usize) -> String {
    let mut sanitized = String::new();
    let mut previous_space = false;
    for character in value.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if character.is_whitespace() {
            if previous_space {
                continue;
            }
            sanitized.push(' ');
            previous_space = true;
        } else {
            sanitized.push(character);
            previous_space = false;
        }
    }
    let sanitized = sanitized.trim();
    let mut characters = sanitized.chars();
    let prefix = characters.by_ref().take(maximum).collect::<String>();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn rule(id: &str, name: &str, description: &str, help: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "shortDescription": { "text": description },
        "fullDescription": { "text": description },
        "help": { "text": help },
        "defaultConfiguration": { "level": "warning" },
        "properties": {
            "precision": "high",
            "problem.severity": "warning",
            "tags": ["architecture", "mastermind"]
        }
    })
}

fn location(path: &str, line: Option<u32>) -> Value {
    json!({ "physicalLocation": physical_location(path, line) })
}

fn physical_location(path: &str, line: Option<u32>) -> Value {
    let mut value = json!({
        "artifactLocation": {
            "uri": artifact_uri(path)
        }
    });
    if let Some(line) = line.filter(|line| *line > 0) {
        value["region"] = json!({ "startLine": line });
    }
    value
}

fn artifact_uri(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let mut encoded = String::with_capacity(normalized.len());
    for byte in normalized.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(nibble(byte >> 4));
            encoded.push(nibble(byte & 0x0f));
        }
    }
    encoded
}

fn nibble(value: u8) -> char {
    char::from(if value < 10 {
        b'0' + value
    } else {
        b'A' + (value - 10)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::{
        ApiCrossing, Collection, ImpactBaseline, ImpactChanges, ImpactDisciplines, ImpactLimits,
        ImpactScope, SeedEvidence, SymbolEvidence,
    };

    fn empty_collection<T>() -> Collection<T> {
        Collection {
            total: Some(0),
            returned: 0,
            truncated: false,
            truncation_reason: None,
            items: Vec::new(),
        }
    }

    #[test]
    fn impact_export_links_changed_and_impacted_locations() {
        let crossing = ApiCrossing {
            seed: SeedEvidence {
                file: "core/pay.rs".into(),
                name: "charge".into(),
                kind: "function".into(),
                line: 7,
                change: "body_changed".into(),
            },
            changed_component: "core".into(),
            impacted: SymbolEvidence {
                file: "api/http.rs".into(),
                name: "checkout".into(),
                kind: "function".into(),
                line: 23,
            },
            impacted_component: "api".into(),
            minimum_depth: 2,
        };
        let response = ChangeImpactResponse {
            schema_version: 1,
            baseline: ImpactBaseline {
                requested_ref: "main".into(),
                baseline_oid: "111".into(),
                head_oid: "222".into(),
                includes_worktree: true,
                includes_untracked: true,
            },
            scope: ImpactScope {
                repository_relative_root: ".".into(),
            },
            changes: ImpactChanges {
                files: empty_collection(),
                symbols: empty_collection(),
            },
            affected_components: empty_collection(),
            impact: empty_collection(),
            api_crossings: Collection {
                total: Some(1),
                returned: 1,
                truncated: false,
                truncation_reason: None,
                items: vec![crossing],
            },
            tests: empty_collection(),
            disciplines: ImpactDisciplines {
                detected: Vec::new(),
                unclassified: Vec::new(),
                note: String::new(),
            },
            limits: ImpactLimits {
                changed_files: 100,
                changed_seeds: 200,
                graph_rows: 5_001,
                impact: 100,
                tests: 100,
                crossings: 100,
                heuristic_paths: 100,
                max_depth: 3,
            },
            precision_notes: Vec::new(),
        };

        let sarif = change_impact(&response);
        let result = &sarif["runs"][0]["results"][0];
        assert_eq!(result["ruleId"], CROSSING_RULE);
        assert_eq!(
            result["locations"][0]["physicalLocation"]["region"]["startLine"],
            7
        );
        assert_eq!(
            result["relatedLocations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "api/http.rs"
        );
        assert_eq!(result["properties"]["baselineOid"], "111");
        assert_eq!(result["properties"]["headOid"], "222");
        assert_eq!(
            sarif["runs"][0]["automationDetails"]["id"],
            "mastermind/change-impact/"
        );
    }

    #[test]
    fn artifact_uris_are_utf8_byte_encoded_and_never_platform_paths() {
        assert_eq!(artifact_uri("src/a file.rs"), "src/a%20file.rs");
        assert_eq!(artifact_uri("src\\nested\\é.rs"), "src/nested/%C3%A9.rs");
    }

    #[test]
    fn messages_remove_controls_and_are_bounded() {
        assert_eq!(safe_message_text("auth\n\tboundary", 100), "auth boundary");
        assert_eq!(safe_message_text("abcdef", 3), "abc…");
    }
}
