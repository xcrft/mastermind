//! Declarative architecture policy checks over bounded codegraph evidence.
//!
//! The evaluator is intentionally separate from evidence collection: Git,
//! SQLite, CODEOWNERS, and workflow artifacts are normalized into
//! [`PolicyInput`], then a small repository-owned YAML DSL is evaluated as a
//! pure deterministic function. This keeps the first release understandable
//! without embedding Rego or another policy runtime.

mod evidence;

use globset::{GlobBuilder, GlobMatcher};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::queries::{ImpactBaseline, ImpactEngine};
use crate::store::Store;

pub const DEFAULT_CONFIG_PATH: &str = "mastermind-policy.yml";
pub const DEFAULT_WORKFLOW_EVIDENCE_PATH: &str = ".mastermind/tasks";
const CONFIG_BYTE_LIMIT: u64 = 1024 * 1024;
const RULE_LIMIT: usize = 100;
const RULE_ID_LIMIT: usize = 128;
const PATTERN_LIMIT: usize = 512;
const POLICY_RESULT_LIMIT: usize = 1_000;

const FAMILY_IMPORT_GRAPH: &str = "import_graph";
const FAMILY_CYCLES: &str = "cycles";
const FAMILY_API: &str = "api_surface";
const FAMILY_IMPACT: &str = "impact";
const FAMILY_TESTS: &str = "tests";
const FAMILY_OWNERSHIP: &str = "ownership";
const FAMILY_WORKFLOW: &str = "workflow";

#[derive(Debug)]
pub struct PolicyError {
    code: &'static str,
    detail: String,
}

impl PolicyError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn new_for_cli(code: &'static str) -> Self {
        let detail = match code {
            "policy_root_unavailable" => "project root cannot be resolved",
            "policy_index_unavailable" => {
                "read-only codegraph index is unavailable; run `mastermind index .`"
            }
            "invalid_policy_limit" => "policy work limit is invalid",
            _ => "architecture policy check cannot continue",
        };
        Self::new(code, detail)
    }
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for PolicyError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default = "policy_version")]
    version: u32,
    rules: Vec<RawRule>,
}

fn policy_version() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    deny_imports: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    max_new_cycles: Option<u32>,
    #[serde(default)]
    when: Option<String>,
    #[serde(default)]
    require_owner: Option<String>,
    #[serde(default)]
    max_blast_radius: Option<u32>,
    #[serde(default)]
    require_tests: Option<bool>,
    #[serde(default)]
    deny_ownership_crossings: Option<bool>,
    #[serde(default)]
    critical: Option<String>,
    #[serde(default)]
    require_workflow: Option<String>,
}

impl RawRule {
    fn present_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.from.is_some() {
            fields.push("from");
        }
        if self.deny_imports.is_some() {
            fields.push("deny_imports");
        }
        if self.scope.is_some() {
            fields.push("scope");
        }
        if self.max_new_cycles.is_some() {
            fields.push("max_new_cycles");
        }
        if self.when.is_some() {
            fields.push("when");
        }
        if self.require_owner.is_some() {
            fields.push("require_owner");
        }
        if self.max_blast_radius.is_some() {
            fields.push("max_blast_radius");
        }
        if self.require_tests.is_some() {
            fields.push("require_tests");
        }
        if self.deny_ownership_crossings.is_some() {
            fields.push("deny_ownership_crossings");
        }
        if self.critical.is_some() {
            fields.push("critical");
        }
        if self.require_workflow.is_some() {
            fields.push("require_workflow");
        }
        fields
    }

    fn action_count(&self) -> usize {
        [
            self.deny_imports.is_some(),
            self.max_new_cycles.is_some(),
            self.require_owner.is_some(),
            self.max_blast_radius.is_some(),
            self.require_tests.is_some(),
            self.deny_ownership_crossings.is_some(),
            self.require_workflow.is_some(),
        ]
        .into_iter()
        .filter(|value| *value)
        .count()
    }
}

#[derive(Debug, Clone)]
pub struct PathPattern {
    raw: String,
    matcher: GlobMatcher,
}

impl PathPattern {
    fn compile(raw: String, rule_id: &str, field: &str) -> Result<Self, PolicyError> {
        let normalized = raw.trim().replace('\\', "/");
        if normalized.is_empty()
            || normalized.len() > PATTERN_LIMIT
            || normalized.starts_with('/')
            || normalized.starts_with("//")
            || normalized.as_bytes().get(1) == Some(&b':')
            || normalized
                .split('/')
                .any(|part| part == ".." || part.chars().any(char::is_control))
        {
            return Err(config_error(
                rule_id,
                format!("`{field}` must be a bounded repository-relative glob"),
            ));
        }
        let glob = GlobBuilder::new(&normalized)
            .literal_separator(true)
            .backslash_escape(false)
            .build()
            .map_err(|_| config_error(rule_id, format!("`{field}` is not a valid glob")))?;
        Ok(Self {
            raw: normalized,
            matcher: glob.compile_matcher(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    fn matches(&self, path: &str) -> bool {
        self.matcher.is_match(path)
    }
}

#[derive(Debug, Clone)]
pub enum PolicyRuleKind {
    DependencyDirection {
        from: PathPattern,
        deny_imports: PathPattern,
    },
    NewCycles {
        scope: PathPattern,
        maximum: u32,
    },
    ApiOwner {
        scope: Option<PathPattern>,
        owner: String,
    },
    BlastRadius {
        scope: PathPattern,
        maximum: u32,
    },
    RelatedTests {
        scope: PathPattern,
    },
    OwnershipBoundary {
        scope: PathPattern,
    },
    StrictWorkflow {
        critical: PathPattern,
    },
}

impl PolicyRuleKind {
    fn family(&self) -> &'static str {
        match self {
            Self::DependencyDirection { .. } => FAMILY_IMPORT_GRAPH,
            Self::NewCycles { .. } => FAMILY_CYCLES,
            Self::ApiOwner { .. } => FAMILY_API,
            Self::BlastRadius { .. } => FAMILY_IMPACT,
            Self::RelatedTests { .. } => FAMILY_TESTS,
            Self::OwnershipBoundary { .. } => FAMILY_OWNERSHIP,
            Self::StrictWorkflow { .. } => FAMILY_WORKFLOW,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::DependencyDirection { .. } => "forbidden-dependency-direction",
            Self::NewCycles { .. } => "new-dependency-cycle-budget",
            Self::ApiOwner { .. } => "public-api-owner-review",
            Self::BlastRadius { .. } => "blast-radius-budget",
            Self::RelatedTests { .. } => "related-test-evidence",
            Self::OwnershipBoundary { .. } => "ownership-boundary-crossing",
            Self::StrictWorkflow { .. } => "strict-workflow-evidence",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::DependencyDirection { .. } => {
                "A changed source file imports a path forbidden by the architecture policy."
            }
            Self::NewCycles { .. } => {
                "The change adds more import dependency cycles than the configured budget."
            }
            Self::ApiOwner { .. } => {
                "An observed cross-component API change lacks the required CODEOWNER."
            }
            Self::BlastRadius { .. } => {
                "The bounded static impact of changed symbols exceeds the configured budget."
            }
            Self::RelatedTests { .. } => {
                "A changed policy scope has no related test candidate in change-impact evidence."
            }
            Self::OwnershipBoundary { .. } => {
                "A changed symbol crosses between disjoint CODEOWNER sets."
            }
            Self::StrictWorkflow { .. } => {
                "A critical changed file lacks matching held strict-workflow evidence."
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyRule {
    pub id: String,
    pub kind: PolicyRuleKind,
}

#[derive(Debug, Clone)]
pub struct PolicyConfig {
    pub version: u32,
    pub rules: Vec<PolicyRule>,
}

pub fn parse_config(source: &[u8]) -> Result<PolicyConfig, PolicyError> {
    if source.is_empty() || source.len() as u64 > CONFIG_BYTE_LIMIT {
        return Err(PolicyError::new(
            "invalid_policy_config",
            "policy config must be non-empty and no larger than 1 MiB",
        ));
    }
    let raw: RawConfig = serde_norway::from_slice(source).map_err(|error| {
        PolicyError::new(
            "invalid_policy_config",
            format!("YAML does not match the v1 policy schema: {error}"),
        )
    })?;
    if raw.version != 1 {
        return Err(PolicyError::new(
            "unsupported_policy_version",
            format!("expected version 1, found {}", raw.version),
        ));
    }
    if raw.rules.is_empty() || raw.rules.len() > RULE_LIMIT {
        return Err(PolicyError::new(
            "invalid_policy_config",
            format!("`rules` must contain between 1 and {RULE_LIMIT} entries"),
        ));
    }

    let mut ids = BTreeSet::new();
    let mut rules = Vec::with_capacity(raw.rules.len());
    for rule in raw.rules {
        validate_rule_id(&rule.id)?;
        if !ids.insert(rule.id.clone()) {
            return Err(config_error(&rule.id, "rule id is duplicated"));
        }
        if rule.action_count() != 1 {
            return Err(config_error(
                &rule.id,
                "exactly one policy action must be configured",
            ));
        }
        let kind = compile_rule(&rule)?;
        rules.push(PolicyRule { id: rule.id, kind });
    }
    rules.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(PolicyConfig { version: 1, rules })
}

fn validate_rule_id(id: &str) -> Result<(), PolicyError> {
    let valid = !id.is_empty()
        && id.len() <= RULE_ID_LIMIT
        && id.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && b"._-".contains(&byte))
        });
    if valid {
        Ok(())
    } else {
        Err(PolicyError::new(
            "invalid_policy_config",
            "rule ids must start with an alphanumeric byte and contain only alphanumerics, `.`, `_`, or `-`",
        ))
    }
}

fn compile_rule(rule: &RawRule) -> Result<PolicyRuleKind, PolicyError> {
    if let Some(target) = &rule.deny_imports {
        ensure_fields(rule, &["from", "deny_imports"])?;
        return Ok(PolicyRuleKind::DependencyDirection {
            from: PathPattern::compile(required(&rule.from, rule, "from")?, &rule.id, "from")?,
            deny_imports: PathPattern::compile(target.clone(), &rule.id, "deny_imports")?,
        });
    }
    if let Some(maximum) = rule.max_new_cycles {
        ensure_fields(rule, &["scope", "max_new_cycles"])?;
        return Ok(PolicyRuleKind::NewCycles {
            scope: PathPattern::compile(required(&rule.scope, rule, "scope")?, &rule.id, "scope")?,
            maximum,
        });
    }
    if let Some(owner) = &rule.require_owner {
        ensure_fields(rule, &["scope", "when", "require_owner"])?;
        if rule.when.as_deref() != Some("api_surface_changed") {
            return Err(config_error(
                &rule.id,
                "`require_owner` requires `when: api_surface_changed`",
            ));
        }
        let owner = normalize_required_owner(owner)
            .ok_or_else(|| config_error(&rule.id, "`require_owner` is empty or invalid"))?;
        return Ok(PolicyRuleKind::ApiOwner {
            scope: rule
                .scope
                .clone()
                .map(|scope| PathPattern::compile(scope, &rule.id, "scope"))
                .transpose()?,
            owner,
        });
    }
    if let Some(maximum) = rule.max_blast_radius {
        ensure_fields(rule, &["scope", "max_blast_radius"])?;
        return Ok(PolicyRuleKind::BlastRadius {
            scope: PathPattern::compile(required(&rule.scope, rule, "scope")?, &rule.id, "scope")?,
            maximum,
        });
    }
    if let Some(required_tests) = rule.require_tests {
        ensure_fields(rule, &["scope", "require_tests"])?;
        if !required_tests {
            return Err(config_error(
                &rule.id,
                "`require_tests` must be true; remove the rule to disable it",
            ));
        }
        return Ok(PolicyRuleKind::RelatedTests {
            scope: PathPattern::compile(required(&rule.scope, rule, "scope")?, &rule.id, "scope")?,
        });
    }
    if let Some(denied) = rule.deny_ownership_crossings {
        ensure_fields(rule, &["scope", "deny_ownership_crossings"])?;
        if !denied {
            return Err(config_error(
                &rule.id,
                "`deny_ownership_crossings` must be true; remove the rule to disable it",
            ));
        }
        return Ok(PolicyRuleKind::OwnershipBoundary {
            scope: PathPattern::compile(required(&rule.scope, rule, "scope")?, &rule.id, "scope")?,
        });
    }
    if let Some(workflow) = &rule.require_workflow {
        ensure_fields(rule, &["critical", "require_workflow"])?;
        if workflow != "strict" {
            return Err(config_error(
                &rule.id,
                "the v1 workflow evidence mode is exactly `strict`",
            ));
        }
        return Ok(PolicyRuleKind::StrictWorkflow {
            critical: PathPattern::compile(
                required(&rule.critical, rule, "critical")?,
                &rule.id,
                "critical",
            )?,
        });
    }
    Err(config_error(&rule.id, "policy action is missing"))
}

fn required(value: &Option<String>, rule: &RawRule, field: &str) -> Result<String, PolicyError> {
    value
        .clone()
        .ok_or_else(|| config_error(&rule.id, format!("`{field}` is required")))
}

fn ensure_fields(rule: &RawRule, allowed: &[&str]) -> Result<(), PolicyError> {
    if let Some(unexpected) = rule
        .present_fields()
        .into_iter()
        .find(|field| !allowed.contains(field))
    {
        Err(config_error(
            &rule.id,
            format!("`{unexpected}` does not belong to this rule type"),
        ))
    } else {
        Ok(())
    }
}

fn config_error(rule_id: &str, detail: impl AsRef<str>) -> PolicyError {
    PolicyError::new(
        "invalid_policy_config",
        format!("rule `{rule_id}`: {}", detail.as_ref()),
    )
}

fn normalize_required_owner(value: &str) -> Option<String> {
    let value = value.trim().trim_start_matches('@');
    if value.is_empty()
        || value.len() > 200
        || value.chars().any(char::is_control)
        || value.chars().any(char::is_whitespace)
    {
        None
    } else {
        Some(value.to_ascii_lowercase())
    }
}

fn owner_matches(actual: &str, required: &str) -> bool {
    let normalized = actual.trim().trim_start_matches('@').to_ascii_lowercase();
    normalized == required || normalized.rsplit('/').next() == Some(required)
}

#[derive(Debug, Clone)]
pub struct CheckOptions {
    pub since: String,
    pub config_path: PathBuf,
    pub codeowners: Option<PathBuf>,
    pub workflow_evidence_path: PathBuf,
    pub depth: u32,
    pub top: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyChangedFile {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DependencyEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiSurfaceChange {
    pub file: String,
    pub line: u32,
    pub name: String,
    pub kind: String,
    pub component: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactRelation {
    pub seed_file: String,
    pub seed_line: u32,
    pub seed_name: String,
    pub impacted_file: String,
    pub impacted_line: u32,
    pub impacted_name: String,
    pub minimum_depth: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoundaryCrossing {
    pub seed_file: String,
    pub seed_line: u32,
    pub seed_component: String,
    pub impacted_file: String,
    pub impacted_line: u32,
    pub impacted_component: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelatedTest {
    pub file: String,
    pub related_seed_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvidenceGap {
    pub family: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyInput {
    pub baseline: ImpactBaseline,
    pub changed_files: Vec<PolicyChangedFile>,
    pub dependency_edges: Vec<DependencyEdge>,
    pub new_cycles: Vec<Vec<String>>,
    pub api_surface_changes: Vec<ApiSurfaceChange>,
    pub impact_relations: Vec<ImpactRelation>,
    pub boundary_crossings: Vec<BoundaryCrossing>,
    pub related_tests: Vec<RelatedTest>,
    pub owners: BTreeMap<String, Vec<String>>,
    pub strict_workflow_files: BTreeMap<String, Vec<String>>,
    pub gaps: Vec<EvidenceGap>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyRuleSummary {
    pub id: String,
    pub kind: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyLocation {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyViolation {
    pub rule_id: String,
    pub rule_kind: &'static str,
    pub level: &'static str,
    pub message: String,
    pub location: PolicyLocation,
    pub related_locations: Vec<PolicyLocation>,
    pub properties: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct PolicyDiagnostic {
    pub rule_id: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyConfigIdentity {
    pub path: String,
    pub sha256: String,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicySummary {
    pub rules_evaluated: u32,
    pub violations: u32,
    pub diagnostics: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyReport {
    pub schema_version: u32,
    pub config: PolicyConfigIdentity,
    pub baseline: ImpactBaseline,
    pub passed: bool,
    pub complete: bool,
    pub summary: PolicySummary,
    pub rules: Vec<PolicyRuleSummary>,
    pub violations: Vec<PolicyViolation>,
    pub diagnostics: Vec<PolicyDiagnostic>,
    pub precision_notes: Vec<&'static str>,
}

struct LoadedConfig {
    config: PolicyConfig,
    identity: PolicyConfigIdentity,
}

pub fn check(
    store: &Store,
    root: &Path,
    options: &CheckOptions,
) -> Result<PolicyReport, PolicyError> {
    check_with_impact_engine(store, root, options, &crate::queries::change_impact)
}

pub fn check_with_impact_engine(
    store: &Store,
    root: &Path,
    options: &CheckOptions,
    impact_engine: &ImpactEngine<'_>,
) -> Result<PolicyReport, PolicyError> {
    let root = root.canonicalize().map_err(|_| {
        PolicyError::new("policy_root_unavailable", "project root cannot be resolved")
    })?;
    let loaded = load_config(&root, &options.config_path)?;
    let input = evidence::collect(store, &root, &loaded.config, options, impact_engine)?;
    let reloaded = load_config(&root, &options.config_path)?;
    if loaded.identity.sha256 != reloaded.identity.sha256 {
        return Err(PolicyError::new(
            "policy_snapshot_changed",
            "policy config changed during evaluation",
        ));
    }
    Ok(evaluate(&loaded.config, input, loaded.identity))
}

fn load_config(root: &Path, requested: &Path) -> Result<LoadedConfig, PolicyError> {
    let path = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let path = path.canonicalize().map_err(|_| {
        PolicyError::new(
            "policy_config_unavailable",
            format!("cannot read `{}`", display_path(root, &path)),
        )
    })?;
    if !path.starts_with(root) {
        return Err(PolicyError::new(
            "invalid_policy_config",
            "policy config must resolve inside the repository",
        ));
    }
    let metadata = std::fs::metadata(&path).map_err(|_| {
        PolicyError::new(
            "policy_config_unavailable",
            format!("cannot read `{}`", display_path(root, &path)),
        )
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > CONFIG_BYTE_LIMIT {
        return Err(PolicyError::new(
            "invalid_policy_config",
            "policy config must be a non-empty regular file no larger than 1 MiB",
        ));
    }
    let source = std::fs::read(&path).map_err(|_| {
        PolicyError::new(
            "policy_config_unavailable",
            format!("cannot read `{}`", display_path(root, &path)),
        )
    })?;
    let config = parse_config(&source)?;
    Ok(LoadedConfig {
        identity: PolicyConfigIdentity {
            path: display_path(root, &path),
            sha256: crate::hex::encode(&Sha256::digest(&source)),
            version: config.version,
        },
        config,
    })
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn evaluate(
    config: &PolicyConfig,
    input: PolicyInput,
    identity: PolicyConfigIdentity,
) -> PolicyReport {
    let changed = input
        .changed_files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut violations = Vec::new();
    let mut diagnostics = BTreeSet::new();
    let mut output_limit_rule = None;
    let mut rules_evaluated = 0u32;

    macro_rules! record_violation {
        ($rule:expr, $label:lifetime, $value:expr) => {{
            if violations.len() + diagnostics.len() >= POLICY_RESULT_LIMIT {
                output_limit_rule = Some($rule.id.clone());
                break $label;
            }
            violations.push($value);
        }};
    }

    'rules: for rule in &config.rules {
        rules_evaluated += 1;
        for gap in input
            .gaps
            .iter()
            .filter(|gap| gap.family == rule.kind.family())
        {
            if !insert_diagnostic_bounded(
                &mut diagnostics,
                PolicyDiagnostic {
                    rule_id: rule.id.clone(),
                    code: gap.code.clone(),
                    message: gap.message.clone(),
                },
                violations.len(),
            ) {
                output_limit_rule = Some(rule.id.clone());
                break 'rules;
            }
        }

        match &rule.kind {
            PolicyRuleKind::DependencyDirection { from, deny_imports } => {
                for edge in input.dependency_edges.iter().filter(|edge| {
                    changed.contains(edge.from.as_str())
                        && from.matches(&edge.from)
                        && deny_imports.matches(&edge.to)
                }) {
                    record_violation!(rule, 'rules, violation(
                        rule,
                        format!(
                            "Changed `{}` imports forbidden architecture target `{}`.",
                            edge.from, edge.to
                        ),
                        location(&edge.from, None, "Forbidden import source"),
                        vec![location(&edge.to, None, "Forbidden import target")],
                        BTreeMap::from([
                            ("fromPattern".into(), json!(from.as_str())),
                            ("denyImportsPattern".into(), json!(deny_imports.as_str())),
                        ]),
                    ));
                }
            }
            PolicyRuleKind::NewCycles { scope, maximum } => {
                let cycles = input
                    .new_cycles
                    .iter()
                    .filter(|cycle| cycle.iter().any(|path| scope.matches(path)))
                    .collect::<Vec<_>>();
                if cycles.len() as u32 > *maximum {
                    let first = cycles[0];
                    let related = first
                        .iter()
                        .skip(1)
                        .map(|path| location(path, None, "New cycle member"))
                        .collect();
                    record_violation!(rule, 'rules, violation(
                        rule,
                        format!(
                            "{} new import cycle(s) intersect `{}`, exceeding the budget of {}.",
                            cycles.len(),
                            scope.as_str(),
                            maximum
                        ),
                        location(&first[0], None, "New dependency cycle"),
                        related,
                        BTreeMap::from([
                            ("scope".into(), json!(scope.as_str())),
                            ("newCycles".into(), json!(cycles.len())),
                            ("maximum".into(), json!(maximum)),
                        ]),
                    ));
                }
            }
            PolicyRuleKind::ApiOwner { scope, owner } => {
                for change in input.api_surface_changes.iter().filter(|change| {
                    scope
                        .as_ref()
                        .is_none_or(|pattern| pattern.matches(&change.file))
                }) {
                    let actual = input
                        .owners
                        .get(&change.file)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    if !actual.iter().any(|value| owner_matches(value, owner)) {
                        record_violation!(rule, 'rules, violation(
                            rule,
                            format!(
                                "Observed API change `{} {}` in `{}` requires CODEOWNER `{}`.",
                                change.kind, change.name, change.file, owner
                            ),
                            location(&change.file, Some(change.line), "API surface change"),
                            Vec::new(),
                            BTreeMap::from([
                                ("requiredOwner".into(), json!(owner)),
                                ("observedOwners".into(), json!(actual)),
                                ("component".into(), json!(change.component)),
                            ]),
                        ));
                    }
                }
            }
            PolicyRuleKind::BlastRadius { scope, maximum } => {
                let impacted = input
                    .impact_relations
                    .iter()
                    .filter(|relation| scope.matches(&relation.seed_file))
                    .map(|relation| {
                        (
                            relation.impacted_file.as_str(),
                            relation.impacted_line,
                            relation.impacted_name.as_str(),
                        )
                    })
                    .collect::<BTreeSet<_>>();
                if impacted.len() as u32 > *maximum {
                    let seed = input
                        .impact_relations
                        .iter()
                        .find(|relation| scope.matches(&relation.seed_file))
                        .expect("non-empty impacted set has a seed");
                    record_violation!(rule, 'rules, violation(
                        rule,
                        format!(
                            "Static blast radius for `{}` is {} symbols, exceeding the budget of {}.",
                            scope.as_str(),
                            impacted.len(),
                            maximum
                        ),
                        location(&seed.seed_file, Some(seed.seed_line), "Changed blast-radius seed"),
                        Vec::new(),
                        BTreeMap::from([
                            ("scope".into(), json!(scope.as_str())),
                            ("impactedSymbols".into(), json!(impacted.len())),
                            ("maximum".into(), json!(maximum)),
                        ]),
                    ));
                }
            }
            PolicyRuleKind::RelatedTests { scope } => {
                let scoped_changes = input
                    .changed_files
                    .iter()
                    .filter(|file| scope.matches(&file.path))
                    .collect::<Vec<_>>();
                let related = input.related_tests.iter().any(|test| {
                    scope.matches(&test.file)
                        || test
                            .related_seed_files
                            .iter()
                            .any(|path| scope.matches(path))
                });
                if !scoped_changes.is_empty() && !related {
                    record_violation!(rule, 'rules, violation(
                        rule,
                        format!(
                            "Changed scope `{}` has no related test candidate.",
                            scope.as_str()
                        ),
                        location(
                            &scoped_changes[0].path,
                            None,
                            "Changed scope without related tests",
                        ),
                        Vec::new(),
                        BTreeMap::from([("scope".into(), json!(scope.as_str()))]),
                    ));
                }
            }
            PolicyRuleKind::OwnershipBoundary { scope } => {
                for crossing in input
                    .boundary_crossings
                    .iter()
                    .filter(|crossing| scope.matches(&crossing.seed_file))
                {
                    let source = input
                        .owners
                        .get(&crossing.seed_file)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    let target = input
                        .owners
                        .get(&crossing.impacted_file)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    if source.is_empty() || target.is_empty() {
                        if !insert_diagnostic_bounded(
                            &mut diagnostics,
                            PolicyDiagnostic {
                                rule_id: rule.id.clone(),
                                code: "ownership_unresolved".into(),
                                message: format!(
                                    "CODEOWNERS did not resolve both sides of `{}` -> `{}`.",
                                    crossing.seed_file, crossing.impacted_file
                                ),
                            },
                            violations.len(),
                        ) {
                            output_limit_rule = Some(rule.id.clone());
                            break 'rules;
                        }
                        continue;
                    }
                    let overlaps = source.iter().any(|left| {
                        target
                            .iter()
                            .any(|right| normalized_owner(left) == normalized_owner(right))
                    });
                    if !overlaps {
                        record_violation!(rule, 'rules, violation(
                            rule,
                            format!(
                                "Change crosses disjoint ownership sets from `{}` to `{}`.",
                                crossing.seed_component, crossing.impacted_component
                            ),
                            location(
                                &crossing.seed_file,
                                Some(crossing.seed_line),
                                "Changed owner boundary source",
                            ),
                            vec![location(
                                &crossing.impacted_file,
                                Some(crossing.impacted_line),
                                "Impacted owner boundary target",
                            )],
                            BTreeMap::from([
                                ("sourceOwners".into(), json!(source)),
                                ("targetOwners".into(), json!(target)),
                            ]),
                        ));
                    }
                }
            }
            PolicyRuleKind::StrictWorkflow { critical } => {
                for file in input.changed_files.iter().filter(|file| {
                    critical.matches(&file.path)
                        && !input.strict_workflow_files.contains_key(&file.path)
                }) {
                    record_violation!(rule, 'rules, violation(
                        rule,
                        format!(
                            "Critical change `{}` has no matching held strict-workflow evidence.",
                            file.path
                        ),
                        location(&file.path, None, "Critical changed file"),
                        Vec::new(),
                        BTreeMap::from([("criticalPattern".into(), json!(critical.as_str()))]),
                    ));
                }
            }
        }
    }

    if let Some(rule_id) = output_limit_rule {
        if violations.len() + diagnostics.len() >= POLICY_RESULT_LIMIT && violations.pop().is_none()
        {
            diagnostics.pop_last();
        }
        diagnostics.insert(PolicyDiagnostic {
            rule_id,
            code: "policy_result_limit".into(),
            message: format!(
                "Policy findings reached the bounded {POLICY_RESULT_LIMIT}-result output limit; remaining evidence was not evaluated."
            ),
        });
    }

    violations.sort_by(|left, right| {
        (
            &left.rule_id,
            &left.location.path,
            left.location.line,
            &left.message,
        )
            .cmp(&(
                &right.rule_id,
                &right.location.path,
                right.location.line,
                &right.message,
            ))
    });
    let diagnostics = diagnostics.into_iter().collect::<Vec<_>>();
    let complete = diagnostics.is_empty();
    let passed = complete && violations.is_empty();
    let rules = config
        .rules
        .iter()
        .map(|rule| PolicyRuleSummary {
            id: rule.id.clone(),
            kind: rule.kind.name(),
            description: rule.kind.description(),
        })
        .collect::<Vec<_>>();

    PolicyReport {
        schema_version: 1,
        config: identity,
        baseline: input.baseline,
        passed,
        complete,
        summary: PolicySummary {
            rules_evaluated,
            violations: violations.len() as u32,
            diagnostics: diagnostics.len() as u32,
        },
        rules,
        violations,
        diagnostics,
        precision_notes: vec![
            "Policy graph topology comes from the default syntactic index; SCIP and runtime overlays do not add or remove policy edges in v1.",
            "api_surface_changed means an observed changed seed crosses an inferred top-level component boundary; it is not a declaration-level export claim.",
            "CODEOWNERS evaluation uses working-tree syntax and does not prove that an owner exists, has access, or approved the change.",
            "Related tests are bounded graph-linked or in-scope candidates, not proof that a test executed or passed.",
            "Strict workflow evidence validates local canonical artifact consistency; it does not verify an audit signature or remote CI execution.",
        ],
    }
}

fn insert_diagnostic_bounded(
    diagnostics: &mut BTreeSet<PolicyDiagnostic>,
    diagnostic: PolicyDiagnostic,
    violation_count: usize,
) -> bool {
    if diagnostics.contains(&diagnostic) {
        return true;
    }
    if diagnostics.len() + violation_count >= POLICY_RESULT_LIMIT {
        return false;
    }
    diagnostics.insert(diagnostic);
    true
}

fn normalized_owner(value: &str) -> String {
    value.trim().trim_start_matches('@').to_ascii_lowercase()
}

fn violation(
    rule: &PolicyRule,
    message: String,
    location: PolicyLocation,
    related_locations: Vec<PolicyLocation>,
    properties: BTreeMap<String, Value>,
) -> PolicyViolation {
    PolicyViolation {
        rule_id: rule.id.clone(),
        rule_kind: rule.kind.name(),
        level: "error",
        message,
        location,
        related_locations,
        properties,
    }
}

fn location(path: &str, line: Option<u32>, message: &str) -> PolicyLocation {
    PolicyLocation {
        path: path.to_string(),
        line,
        message: message.to_string(),
    }
}

pub fn render_text(report: &PolicyReport) -> String {
    let verdict = if report.passed {
        "PASS"
    } else if report.complete {
        "FAIL"
    } else {
        "INCOMPLETE"
    };
    let mut output = format!(
        "Architecture policy: {verdict}\n  config: {} ({})\n  baseline: {}\n  rules: {} | violations: {} | diagnostics: {}\n",
        report.config.path,
        &report.config.sha256[..12],
        report.baseline.baseline_oid,
        report.summary.rules_evaluated,
        report.summary.violations,
        report.summary.diagnostics,
    );
    for violation in &report.violations {
        let line = violation
            .location
            .line
            .map(|line| format!(":{line}"))
            .unwrap_or_default();
        output.push_str(&format!(
            "  error [{}] {}{} — {}\n",
            violation.rule_id, violation.location.path, line, violation.message
        ));
    }
    for diagnostic in &report.diagnostics {
        output.push_str(&format!(
            "  incomplete [{}:{}] {}\n",
            diagnostic.rule_id, diagnostic.code, diagnostic.message
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_RULES: &[u8] = br#"
version: 1
rules:
  - id: domain-must-not-import-infrastructure
    from: src/domain/**
    deny_imports: src/infrastructure/**
  - id: no-new-payment-cycles
    scope: services/payment/**
    max_new_cycles: 0
  - id: public-api-review
    when: api_surface_changed
    require_owner: platform
  - id: payment-blast-budget
    scope: services/payment/**
    max_blast_radius: 1
  - id: payment-needs-tests
    scope: services/payment/**
    require_tests: true
  - id: payment-owner-boundary
    scope: services/payment/**
    deny_ownership_crossings: true
  - id: critical-payment-workflow
    critical: services/payment/**
    require_workflow: strict
"#;

    fn identity() -> PolicyConfigIdentity {
        PolicyConfigIdentity {
            path: "mastermind-policy.yml".into(),
            sha256: "0".repeat(64),
            version: 1,
        }
    }

    fn baseline() -> ImpactBaseline {
        ImpactBaseline {
            requested_ref: "main".into(),
            baseline_oid: "1".repeat(40),
            head_oid: "2".repeat(40),
            includes_worktree: true,
            includes_untracked: true,
        }
    }

    #[test]
    fn parses_the_developer_facing_policy_dsl() {
        let config = parse_config(ALL_RULES).unwrap();
        assert_eq!(config.version, 1);
        assert_eq!(config.rules.len(), 7);
        assert_eq!(
            config.rules[0].id, "critical-payment-workflow",
            "rules are normalized by stable id"
        );
    }

    #[test]
    fn rejects_unknown_ambiguous_and_noop_rules() {
        for source in [
            br#"rules: [{id: bad, scope: "**", require_tests: true, surprise: yes}]"#.as_slice(),
            br#"rules: [{id: bad, scope: "**", require_tests: true, max_new_cycles: 0}]"#
                .as_slice(),
            br#"rules: [{id: bad, scope: "**", require_tests: false}]"#.as_slice(),
        ] {
            let error = parse_config(source).unwrap_err();
            assert_eq!(error.code(), "invalid_policy_config");
        }
        let too_many = format!(
            "rules:\n{}",
            (0..=RULE_LIMIT)
                .map(|index| format!(
                    "  - id: rule-{index}\n    scope: src/**\n    require_tests: true\n"
                ))
                .collect::<String>()
        );
        assert_eq!(
            parse_config(too_many.as_bytes()).unwrap_err().code(),
            "invalid_policy_config"
        );
    }

    #[test]
    fn all_seven_rule_families_evaluate_from_one_structured_input() {
        let config = parse_config(ALL_RULES).unwrap();
        let input = PolicyInput {
            baseline: baseline(),
            changed_files: vec![
                PolicyChangedFile {
                    path: "src/domain/order.ts".into(),
                    status: "modified".into(),
                },
                PolicyChangedFile {
                    path: "services/payment/charge.ts".into(),
                    status: "modified".into(),
                },
            ],
            dependency_edges: vec![DependencyEdge {
                from: "src/domain/order.ts".into(),
                to: "src/infrastructure/db.ts".into(),
            }],
            new_cycles: vec![vec![
                "services/payment/charge.ts".into(),
                "services/payment/gateway.ts".into(),
            ]],
            api_surface_changes: vec![ApiSurfaceChange {
                file: "services/payment/charge.ts".into(),
                line: 7,
                name: "charge".into(),
                kind: "function".into(),
                component: "services".into(),
            }],
            impact_relations: vec![
                ImpactRelation {
                    seed_file: "services/payment/charge.ts".into(),
                    seed_line: 7,
                    seed_name: "charge".into(),
                    impacted_file: "api/checkout.ts".into(),
                    impacted_line: 20,
                    impacted_name: "checkout".into(),
                    minimum_depth: 1,
                },
                ImpactRelation {
                    seed_file: "services/payment/charge.ts".into(),
                    seed_line: 7,
                    seed_name: "charge".into(),
                    impacted_file: "worker/retry.ts".into(),
                    impacted_line: 30,
                    impacted_name: "retry".into(),
                    minimum_depth: 2,
                },
            ],
            boundary_crossings: vec![BoundaryCrossing {
                seed_file: "services/payment/charge.ts".into(),
                seed_line: 7,
                seed_component: "services".into(),
                impacted_file: "api/checkout.ts".into(),
                impacted_line: 20,
                impacted_component: "api".into(),
            }],
            related_tests: Vec::new(),
            owners: BTreeMap::from([
                (
                    "services/payment/charge.ts".into(),
                    vec!["@payments".into()],
                ),
                ("api/checkout.ts".into(), vec!["@platform".into()]),
            ]),
            strict_workflow_files: BTreeMap::new(),
            gaps: Vec::new(),
        };

        let report = evaluate(&config, input, identity());
        assert!(!report.passed);
        assert!(report.complete);
        assert_eq!(report.summary.violations, 7);
        assert_eq!(
            report
                .violations
                .iter()
                .map(|violation| violation.rule_kind)
                .collect::<BTreeSet<_>>()
                .len(),
            7
        );
        let sarif = crate::sarif_export::architecture_policy(&report);
        assert_eq!(sarif["version"], "2.1.0");
        assert_eq!(
            sarif["runs"][0]["tool"]["driver"]["rules"]
                .as_array()
                .unwrap()
                .len(),
            7
        );
        assert_eq!(sarif["runs"][0]["results"].as_array().unwrap().len(), 7);
        assert!(sarif["runs"][0]["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|result| result["level"] == "error"));
        assert!(sarif["runs"][0]["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(
                |result| result["partialFingerprints"]["primaryLocationLineHash"]
                    .as_str()
                    .is_some_and(|fingerprint| {
                        fingerprint.len() == 18 && fingerprint.ends_with(":1")
                    })
            ));
    }

    #[test]
    fn unrelated_test_in_same_top_level_component_does_not_satisfy_scope() {
        let config = parse_config(
            br#"
rules:
  - id: payment-tests
    scope: services/payment/**
    require_tests: true
"#,
        )
        .unwrap();
        let report = evaluate(
            &config,
            PolicyInput {
                baseline: baseline(),
                changed_files: vec![PolicyChangedFile {
                    path: "services/payment/charge.ts".into(),
                    status: "modified".into(),
                }],
                dependency_edges: Vec::new(),
                new_cycles: Vec::new(),
                api_surface_changes: Vec::new(),
                impact_relations: Vec::new(),
                boundary_crossings: Vec::new(),
                related_tests: vec![RelatedTest {
                    file: "services/shipping/test_rate.ts".into(),
                    related_seed_files: Vec::new(),
                }],
                owners: BTreeMap::new(),
                strict_workflow_files: BTreeMap::new(),
                gaps: Vec::new(),
            },
            identity(),
        );
        assert_eq!(report.summary.violations, 1);
        assert_eq!(report.violations[0].rule_id, "payment-tests");
    }

    #[test]
    fn sarif_fingerprints_distinguish_findings_on_the_same_source_file() {
        let config = parse_config(
            br#"
rules:
  - id: dependency-direction
    from: src/domain/**
    deny_imports: src/infrastructure/**
"#,
        )
        .unwrap();
        let report = evaluate(
            &config,
            PolicyInput {
                baseline: baseline(),
                changed_files: vec![PolicyChangedFile {
                    path: "src/domain/order.ts".into(),
                    status: "modified".into(),
                }],
                dependency_edges: vec![
                    DependencyEdge {
                        from: "src/domain/order.ts".into(),
                        to: "src/infrastructure/db.ts".into(),
                    },
                    DependencyEdge {
                        from: "src/domain/order.ts".into(),
                        to: "src/infrastructure/cache.ts".into(),
                    },
                ],
                new_cycles: Vec::new(),
                api_surface_changes: Vec::new(),
                impact_relations: Vec::new(),
                boundary_crossings: Vec::new(),
                related_tests: Vec::new(),
                owners: BTreeMap::new(),
                strict_workflow_files: BTreeMap::new(),
                gaps: Vec::new(),
            },
            identity(),
        );
        let sarif = crate::sarif_export::architecture_policy(&report);
        let fingerprints = sarif["runs"][0]["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|result| {
                result["partialFingerprints"]["primaryLocationLineHash"]
                    .as_str()
                    .unwrap()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(fingerprints.len(), 2);
    }

    #[test]
    fn policy_findings_are_bounded_and_fail_closed() {
        let config = parse_config(
            br#"
rules:
  - id: bounded-imports
    from: src/domain/**
    deny_imports: src/infrastructure/**
"#,
        )
        .unwrap();
        let mut changed_files = Vec::new();
        let mut dependency_edges = Vec::new();
        for index in 0..=POLICY_RESULT_LIMIT {
            let source = format!("src/domain/file_{index}.ts");
            changed_files.push(PolicyChangedFile {
                path: source.clone(),
                status: "modified".into(),
            });
            dependency_edges.push(DependencyEdge {
                from: source,
                to: format!("src/infrastructure/file_{index}.ts"),
            });
        }
        let report = evaluate(
            &config,
            PolicyInput {
                baseline: baseline(),
                changed_files,
                dependency_edges,
                new_cycles: Vec::new(),
                api_surface_changes: Vec::new(),
                impact_relations: Vec::new(),
                boundary_crossings: Vec::new(),
                related_tests: Vec::new(),
                owners: BTreeMap::new(),
                strict_workflow_files: BTreeMap::new(),
                gaps: vec![EvidenceGap {
                    family: FAMILY_IMPORT_GRAPH.into(),
                    code: "fixture_evidence_note".into(),
                    message: "fixture diagnostic".into(),
                }],
            },
            identity(),
        );
        assert!(!report.complete);
        assert!(!report.passed);
        assert_eq!(report.violations.len(), POLICY_RESULT_LIMIT - 2);
        assert_eq!(
            report.violations.len() + report.diagnostics.len(),
            POLICY_RESULT_LIMIT
        );
        assert_eq!(report.summary.rules_evaluated, 1);
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "policy_result_limit"));
        let sarif = crate::sarif_export::architecture_policy(&report);
        assert_eq!(
            sarif["runs"][0]["results"].as_array().unwrap().len(),
            POLICY_RESULT_LIMIT
        );
    }

    #[test]
    fn incomplete_evidence_fails_closed_without_inventing_a_violation() {
        let config =
            parse_config(br#"rules: [{id: cycles, scope: "src/**", max_new_cycles: 0}]"#).unwrap();
        let report = evaluate(
            &config,
            PolicyInput {
                baseline: baseline(),
                changed_files: Vec::new(),
                dependency_edges: Vec::new(),
                new_cycles: Vec::new(),
                api_surface_changes: Vec::new(),
                impact_relations: Vec::new(),
                boundary_crossings: Vec::new(),
                related_tests: Vec::new(),
                owners: BTreeMap::new(),
                strict_workflow_files: BTreeMap::new(),
                gaps: vec![EvidenceGap {
                    family: FAMILY_CYCLES.into(),
                    code: "import_graph_work_limit".into(),
                    message: "cycle comparison was bounded".into(),
                }],
            },
            identity(),
        );
        assert!(!report.passed);
        assert!(!report.complete);
        assert!(report.violations.is_empty());
        assert_eq!(report.diagnostics[0].code, "import_graph_work_limit");
        let sarif = crate::sarif_export::architecture_policy(&report);
        assert_eq!(
            sarif["runs"][0]["results"][0]["ruleId"],
            "mastermind/policy-evaluation-incomplete"
        );
    }
}
