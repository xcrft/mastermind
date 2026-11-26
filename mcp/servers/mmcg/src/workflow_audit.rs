//! Workflow bundle audit: the installed agent and skill manifests, their
//! digests, frontmatter safety, MCP registrations and writer conflicts. Used by
//! `mastermind workflow audit` and `doctor`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::terminal::escape as escape_terminal;

pub const WORKFLOW_AUDIT_SCHEMA_VERSION: u32 = 1;

const MAX_WORKFLOW_AGENTS: usize = 128;

const MAX_WORKFLOW_SKILLS: usize = 512;

const MAX_WORKFLOW_MARKDOWN_BYTES: usize = 256 * 1024;

const MAX_WORKFLOW_MANIFEST_BYTES: usize = 1024 * 1024;

const MAX_WORKFLOW_TEXT_BYTES: usize = 8 * 1024 * 1024;

const MAX_WORKFLOW_NODES: usize = 4_096;

const MAX_WORKFLOW_EDGES: usize = 16_384;

const MAX_WORKFLOW_YAML_DEPTH: usize = 16;

const MAX_WORKFLOW_ARTIFACT_FILES: usize = 4_096;

const MAX_WORKFLOW_DIRECTORY_ENTRIES: usize = 8_192;

const MAX_WORKFLOW_DIRECTORIES: usize = 4_096;

const MAX_WORKFLOW_RELATIONS_PER_COMPONENT: usize = 512;

const MAX_WORKFLOW_WRITES_PER_COMPONENT: usize = 64;

const MAX_WORKFLOW_TOOL_GRANTS_PER_COMPONENT: usize = 512;

const MAX_WORKFLOW_SERVERS_PER_COMPONENT: usize = 64;

const MAX_WORKFLOW_WRITERS: usize = 512;

const MAX_WORKFLOW_DIAGNOSTICS: usize = 4_096;

const MAX_WORKFLOW_CONTEXT_ESTIMATES: usize = 16_384;

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct WorkflowAuditLimits {
    pub agents: usize,
    pub skills: usize,
    pub markdown_bytes: usize,
    pub manifest_bytes: usize,
    pub total_text_bytes: usize,
    pub nodes: usize,
    pub edges: usize,
    pub yaml_depth: usize,
    pub directory_entries: usize,
    pub directories: usize,
    pub relations_per_component: usize,
    pub writes_per_component: usize,
    pub tool_grants_per_component: usize,
    pub servers_per_component: usize,
    pub writers: usize,
    pub diagnostics: usize,
    pub context_estimates: usize,
}

impl Default for WorkflowAuditLimits {
    fn default() -> Self {
        Self {
            agents: MAX_WORKFLOW_AGENTS,
            skills: MAX_WORKFLOW_SKILLS,
            markdown_bytes: MAX_WORKFLOW_MARKDOWN_BYTES,
            manifest_bytes: MAX_WORKFLOW_MANIFEST_BYTES,
            total_text_bytes: MAX_WORKFLOW_TEXT_BYTES,
            nodes: MAX_WORKFLOW_NODES,
            edges: MAX_WORKFLOW_EDGES,
            yaml_depth: MAX_WORKFLOW_YAML_DEPTH,
            directory_entries: MAX_WORKFLOW_DIRECTORY_ENTRIES,
            directories: MAX_WORKFLOW_DIRECTORIES,
            relations_per_component: MAX_WORKFLOW_RELATIONS_PER_COMPONENT,
            writes_per_component: MAX_WORKFLOW_WRITES_PER_COMPONENT,
            tool_grants_per_component: MAX_WORKFLOW_TOOL_GRANTS_PER_COMPONENT,
            servers_per_component: MAX_WORKFLOW_SERVERS_PER_COMPONENT,
            writers: MAX_WORKFLOW_WRITERS,
            diagnostics: MAX_WORKFLOW_DIAGNOSTICS,
            context_estimates: MAX_WORKFLOW_CONTEXT_ESTIMATES,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct WorkflowNode {
    pub id: String,
    pub kind: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct WorkflowEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub precision: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct WorkflowDiagnostic {
    pub code: String,
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_relation: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct WorkflowContextEstimate {
    pub component_id: String,
    pub scenario: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unavailable: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct WorkflowAuditReport {
    pub schema_version: u32,
    pub root: String,
    pub layout: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub limits: WorkflowAuditLimits,
    /// Whether the configured inventory, traversal, and input checks completed.
    /// This is independent of wiring diagnostics, runtime execution, and acceptance.
    pub complete: bool,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    pub diagnostics: Vec<WorkflowDiagnostic>,
    pub context_estimates: Vec<WorkflowContextEstimate>,
}

impl WorkflowAuditReport {
    pub fn has_errors(&self) -> bool {
        !self.complete
            || self
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == "error")
    }

    pub fn render_text(&self) -> String {
        let mut output = String::new();
        output.push_str("Mastermind workflow audit\n\n");
        output.push_str(&format!("  schema:   v{}\n", self.schema_version));
        output.push_str(&format!("  root:     {}\n", escape_terminal(&self.root)));
        output.push_str(&format!("  layout:   {}\n", escape_terminal(&self.layout)));
        if let Some(client) = &self.client {
            output.push_str(&format!("  client:   {}\n", escape_terminal(client)));
        }
        if let Some(profile) = &self.profile {
            output.push_str(&format!("  profile:  {}\n", escape_terminal(profile)));
        }
        let errors = self
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == "error")
            .count();
        let warnings = self
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == "warning")
            .count();
        let information = self.diagnostics.len().saturating_sub(errors + warnings);
        output.push_str(&format!(
            "  graph:    {} nodes, {} edges\n  findings: {errors} errors, {warnings} warnings, {information} info\n  collection complete: {}\n",
            self.nodes.len(),
            self.edges.len(),
            self.complete
        ));
        output.push_str(&format!(
            "  limits:   {} agents, {} skills, {} B Markdown, {} B manifest, {} B total text, {} nodes, {} edges, YAML depth {}, {} directory entries, {} directories\n",
            self.limits.agents,
            self.limits.skills,
            self.limits.markdown_bytes,
            self.limits.manifest_bytes,
            self.limits.total_text_bytes,
            self.limits.nodes,
            self.limits.edges,
            self.limits.yaml_depth,
            self.limits.directory_entries,
            self.limits.directories
        ));
        output.push_str(&format!(
            "            {} relations/component, {} writes/component, {} tool grants/component, {} servers/component, {} writers, {} diagnostics, {} context estimates\n",
            self.limits.relations_per_component,
            self.limits.writes_per_component,
            self.limits.tool_grants_per_component,
            self.limits.servers_per_component,
            self.limits.writers,
            self.limits.diagnostics,
            self.limits.context_estimates
        ));
        output.push_str("  estimate: ceil(UTF-8 bytes / 4), component scenarios only\n");

        output.push_str("\nNodes\n");
        for node in &self.nodes {
            output.push_str(&format!(
                "  {} [{}] — {}",
                escape_terminal(&node.id),
                escape_terminal(&node.kind),
                escape_terminal(&node.label)
            ));
            if let Some(path) = &node.path {
                output.push_str(&format!(" — {}", escape_terminal(path)));
            }
            output.push('\n');
        }

        output.push_str("\nEdges\n");
        for edge in &self.edges {
            output.push_str(&format!(
                "  {} -> {} [{}; precision={}]\n",
                escape_terminal(&edge.from),
                escape_terminal(&edge.to),
                escape_terminal(&edge.kind),
                escape_terminal(&edge.precision)
            ));
        }

        output.push_str("\nContext estimates\n");
        for estimate in &self.context_estimates {
            let bytes = estimate
                .bytes
                .map_or_else(|| "unknown".to_string(), |value| value.to_string());
            let tokens = estimate
                .estimated_tokens
                .map_or_else(|| "unknown".to_string(), |value| value.to_string());
            output.push_str(&format!(
                "  {} [{}] — {} bytes, {} estimated tokens\n",
                escape_terminal(&estimate.component_id),
                escape_terminal(&estimate.scenario),
                bytes,
                tokens
            ));
            if !estimate.components.is_empty() {
                output.push_str(&format!(
                    "    components: {}\n",
                    estimate
                        .components
                        .iter()
                        .map(|value| escape_terminal(value))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !estimate.unavailable.is_empty() {
                output.push_str(&format!(
                    "    unavailable: {}\n",
                    estimate
                        .unavailable
                        .iter()
                        .map(|value| escape_terminal(value))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        if self.diagnostics.is_empty() {
            output.push_str("\n  no findings\n");
        } else {
            output.push_str("\nFindings\n");
            for diagnostic in &self.diagnostics {
                let marker = match diagnostic.severity.as_str() {
                    "error" => "error",
                    "warning" => "warn",
                    _ => "info",
                };
                let mut scope = Vec::new();
                if let Some(component) = &diagnostic.component_id {
                    scope.push(format!("component={}", escape_terminal(component)));
                }
                if let Some(path) = &diagnostic.path {
                    scope.push(format!("path={}", escape_terminal(path)));
                }
                if let Some(relation) = &diagnostic.evidence_relation {
                    scope.push(format!("relation={}", escape_terminal(relation)));
                }
                let scope = if scope.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", scope.join(", "))
                };
                output.push_str(&format!(
                    "  [{marker}] {}: {}{scope}\n",
                    escape_terminal(&diagnostic.code),
                    escape_terminal(&diagnostic.message)
                ));
            }
        }
        output
    }
}

#[derive(Debug, Clone, Copy, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum WorkflowActivation {
    Always,
    Conditional,
    Manual,
}

#[derive(Debug, Clone, Copy, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum WorkflowMutability {
    ReadOnly,
    Writer,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowSkillRelation {
    id: String,
    required: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowWriteDeclaration {
    artifact: String,
    path: String,
    authority: String,
    runtime: String,
    exclusivity_group: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowMetadata {
    schema_version: u32,
    activation: WorkflowActivation,
    mutability: WorkflowMutability,
    #[serde(default)]
    skills: Vec<WorkflowSkillRelation>,
    #[serde(default)]
    writes: Vec<WorkflowWriteDeclaration>,
}

#[derive(Debug, Clone)]
struct LoadedWorkflowComponent {
    id: String,
    node_id: String,
    kind: String,
    path: String,
    text_bytes: usize,
    tools: Vec<String>,
    servers: Vec<String>,
    prompt_tools: BTreeSet<String>,
    wikilinks: BTreeSet<String>,
    model: Option<String>,
    max_turns: Option<u64>,
    effort: Option<String>,
    workflow: Option<WorkflowMetadata>,
}

#[derive(Debug, Clone)]
struct InstalledWorkflowManifest {
    client: String,
    profile: String,
    agents: Vec<String>,
    skills: Vec<String>,
    digests: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct WriterFact {
    writer_id: String,
    declaration: WorkflowWriteDeclaration,
    activation: WorkflowActivation,
}

struct WorkflowAuditBuilder {
    report: WorkflowAuditReport,
    root: PathBuf,
    canonical_root: Option<PathBuf>,
    total_text_bytes: usize,
    node_ids: BTreeSet<String>,
    edge_keys: BTreeSet<(String, String, String, String)>,
    inventory_budget: DirectoryBudget,
    graph_limit_reported: bool,
    diagnostic_limit_reported: bool,
    context_limit_reported: bool,
    writer_limit_reported: bool,
}

impl WorkflowAuditBuilder {
    fn new(root: &Path) -> Self {
        let canonical_root = root.canonicalize().ok();
        Self {
            report: WorkflowAuditReport {
                schema_version: WORKFLOW_AUDIT_SCHEMA_VERSION,
                root: root.display().to_string(),
                layout: "ambiguous".into(),
                client: None,
                profile: None,
                limits: WorkflowAuditLimits::default(),
                complete: true,
                nodes: Vec::new(),
                edges: Vec::new(),
                diagnostics: Vec::new(),
                context_estimates: Vec::new(),
            },
            root: root.to_path_buf(),
            canonical_root,
            total_text_bytes: 0,
            node_ids: BTreeSet::new(),
            edge_keys: BTreeSet::new(),
            inventory_budget: DirectoryBudget::default(),
            graph_limit_reported: false,
            diagnostic_limit_reported: false,
            context_limit_reported: false,
            writer_limit_reported: false,
        }
    }

    fn diagnostic(
        &mut self,
        code: &str,
        severity: &str,
        message: impl Into<String>,
        component_id: Option<&str>,
        path: Option<&str>,
        evidence_relation: Option<&str>,
    ) {
        if self.report.diagnostics.len() >= MAX_WORKFLOW_DIAGNOSTICS {
            self.report.complete = false;
            return;
        }
        if self.report.diagnostics.len() == MAX_WORKFLOW_DIAGNOSTICS - 1
            && code != "diagnostic_limit_exceeded"
        {
            self.report.complete = false;
            if !self.diagnostic_limit_reported {
                self.diagnostic_limit_reported = true;
                self.report.diagnostics.push(WorkflowDiagnostic {
                    code: "diagnostic_limit_exceeded".into(),
                    severity: "error".into(),
                    message: format!(
                        "workflow report reached the {MAX_WORKFLOW_DIAGNOSTICS}-diagnostic limit"
                    ),
                    component_id: None,
                    path: None,
                    evidence_relation: None,
                });
            }
            return;
        }
        if severity == "error" {
            self.report.complete &= !matches!(
                code,
                "layout_ambiguous"
                    | "manifest_invalid"
                    | "manifest_limit_exceeded"
                    | "file_limit_exceeded"
                    | "file_type_invalid"
                    | "file_outside_root"
                    | "file_identity_changed"
                    | "file_read_failed"
                    | "file_encoding_invalid"
                    | "workflow_inventory_limit_exceeded"
                    | "workflow_total_text_limit_exceeded"
                    | "unsafe_yaml"
                    | "yaml_depth_exceeded"
                    | "frontmatter_invalid"
                    | "graph_limit_exceeded"
                    | "workflow_declaration_limit_exceeded"
                    | "diagnostic_limit_exceeded"
                    | "context_estimate_limit_exceeded"
                    | "component_identity_duplicate"
            );
        }
        self.report.diagnostics.push(WorkflowDiagnostic {
            code: code.into(),
            severity: severity.into(),
            message: escape_terminal(&message.into()),
            component_id: component_id.map(escape_terminal),
            path: path.map(escape_terminal),
            evidence_relation: evidence_relation.map(escape_terminal),
        });
    }

    fn incomplete(
        &mut self,
        code: &str,
        message: impl Into<String>,
        component_id: Option<&str>,
        path: Option<&str>,
    ) {
        self.report.complete = false;
        self.diagnostic(code, "error", message, component_id, path, None);
    }

    fn add_node(&mut self, id: String, kind: &str, label: String, path: Option<String>) {
        if self.node_ids.contains(&id) {
            return;
        }
        if self.node_ids.len() >= MAX_WORKFLOW_NODES {
            if !self.graph_limit_reported {
                self.graph_limit_reported = true;
                self.incomplete(
                    "graph_limit_exceeded",
                    format!("workflow graph exceeds {MAX_WORKFLOW_NODES} nodes"),
                    None,
                    None,
                );
            }
            return;
        }
        self.node_ids.insert(id.clone());
        self.report.nodes.push(WorkflowNode {
            id,
            kind: kind.into(),
            label: escape_terminal(&label),
            path: path.map(|value| escape_terminal(&value)),
        });
    }

    fn add_edge(&mut self, from: &str, to: &str, kind: &str, precision: &str) {
        let key = (
            from.to_string(),
            to.to_string(),
            kind.to_string(),
            precision.to_string(),
        );
        if self.edge_keys.contains(&key) {
            return;
        }
        if self.edge_keys.len() >= MAX_WORKFLOW_EDGES {
            if !self.graph_limit_reported {
                self.graph_limit_reported = true;
                self.incomplete(
                    "graph_limit_exceeded",
                    format!("workflow graph exceeds {MAX_WORKFLOW_EDGES} edges"),
                    None,
                    None,
                );
            }
            return;
        }
        self.edge_keys.insert(key.clone());
        self.report.edges.push(WorkflowEdge {
            from: key.0,
            to: key.1,
            kind: key.2,
            precision: key.3,
        });
    }

    fn read_owned_markdown(&mut self, path: &Path, relative: &str) -> Option<String> {
        let canonical_root = self.canonical_root.clone()?;
        let text = read_bounded_text(path, &canonical_root, MAX_WORKFLOW_MARKDOWN_BYTES)
            .map_err(|failure| self.incomplete(failure.code, failure.message, None, Some(relative)))
            .ok()?;
        self.admit_markdown_text(text, relative)
    }

    fn admit_markdown_text(&mut self, text: String, relative: &str) -> Option<String> {
        if text.len() > MAX_WORKFLOW_MARKDOWN_BYTES {
            self.incomplete(
                "file_limit_exceeded",
                format!("workflow input exceeds {MAX_WORKFLOW_MARKDOWN_BYTES} bytes"),
                None,
                Some(relative),
            );
            return None;
        }
        let next_total = self.total_text_bytes.saturating_add(text.len());
        if next_total > MAX_WORKFLOW_TEXT_BYTES {
            self.incomplete(
                "workflow_total_text_limit_exceeded",
                format!("workflow Markdown exceeds the {MAX_WORKFLOW_TEXT_BYTES}-byte total limit"),
                None,
                Some(relative),
            );
            return None;
        }
        self.total_text_bytes = next_total;
        Some(text)
    }

    fn add_context_estimate(&mut self, estimate: WorkflowContextEstimate) {
        if self.report.context_estimates.len() >= MAX_WORKFLOW_CONTEXT_ESTIMATES {
            if !self.context_limit_reported {
                self.context_limit_reported = true;
                self.incomplete(
                    "context_estimate_limit_exceeded",
                    format!(
                        "workflow report exceeds {MAX_WORKFLOW_CONTEXT_ESTIMATES} context estimates"
                    ),
                    None,
                    None,
                );
            }
            return;
        }
        self.report.context_estimates.push(estimate);
    }

    fn read_inventory_directory(
        &mut self,
        directory: &Path,
        relative: &str,
    ) -> Option<Vec<std::ffi::OsString>> {
        let canonical_root = self.canonical_root.clone()?;
        read_bounded_directory(directory, &canonical_root, &mut self.inventory_budget)
            .map_err(|failure| {
                self.incomplete(failure.code, failure.message, None, Some(relative));
            })
            .ok()
    }

    fn finish(mut self) -> WorkflowAuditReport {
        self.report
            .nodes
            .sort_by(|left, right| left.id.cmp(&right.id));
        self.report.edges.sort_by(|left, right| {
            (&left.from, &left.to, &left.kind, &left.precision).cmp(&(
                &right.from,
                &right.to,
                &right.kind,
                &right.precision,
            ))
        });
        self.report.diagnostics.sort_by(|left, right| {
            (
                severity_rank(&left.severity),
                &left.code,
                &left.component_id,
                &left.path,
                &left.message,
            )
                .cmp(&(
                    severity_rank(&right.severity),
                    &right.code,
                    &right.component_id,
                    &right.path,
                    &right.message,
                ))
        });
        self.report.context_estimates.sort_by(|left, right| {
            (&left.component_id, &left.scenario, &left.components).cmp(&(
                &right.component_id,
                &right.scenario,
                &right.components,
            ))
        });
        self.report
    }
}

#[derive(Debug)]
struct FileReadFailure {
    code: &'static str,
    message: String,
}

#[derive(Default)]
struct DirectoryBudget {
    entries: usize,
    directories: usize,
}

pub fn audit_workflow(root: &Path) -> WorkflowAuditReport {
    let mut builder = WorkflowAuditBuilder::new(root);
    if builder.canonical_root.is_none() {
        builder.incomplete(
            "layout_ambiguous",
            format!("workflow root is unavailable: {}", root.display()),
            None,
            None,
        );
        return builder.finish();
    }
    if fs::symlink_metadata(root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        builder.incomplete(
            "file_type_invalid",
            "workflow root must not be a symbolic link",
            None,
            None,
        );
        return builder.finish();
    }

    let source_agent_root = root.join("agents");
    let source_agents = source_agent_root.join("subagents");
    let source_skills = root.join("skills");
    let manifest_path = root.join(".mastermind-workflow.json");
    for (path, label) in [
        (&source_agent_root, "agents"),
        (&source_agents, "agents/subagents"),
        (&source_skills, "skills"),
    ] {
        if fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_symlink() || !metadata.file_type().is_dir()
        }) {
            builder.incomplete(
                "file_type_invalid",
                format!("source workflow inventory path is not a regular directory: {label}"),
                None,
                Some(label),
            );
            return builder.finish();
        }
    }
    let source_layout = is_plain_directory(&source_agents) && is_plain_directory(&source_skills);
    let manifest_layout = fs::symlink_metadata(&manifest_path).is_ok();

    if source_layout == manifest_layout {
        builder.incomplete(
            "layout_ambiguous",
            if source_layout {
                "both source workflow directories and an installed ownership manifest are present"
                    .to_string()
            } else {
                "expected source agents/subagents plus skills, or an installed .mastermind-workflow.json"
                    .to_string()
            },
            None,
            None,
        );
        return builder.finish();
    }

    let (agent_paths, skill_paths, installed, mut installed_text) = if source_layout {
        builder.report.layout = "source".into();
        let agents = collect_source_agents(&mut builder, &source_agents);
        let skills = collect_source_skills(&mut builder, &source_skills);
        (agents, skills, None, BTreeMap::new())
    } else {
        builder.report.layout = "installed".into();
        let Some(manifest) = load_installed_manifest(&mut builder, &manifest_path) else {
            return builder.finish();
        };
        builder.report.client = Some(manifest.client.clone());
        builder.report.profile = Some(manifest.profile.clone());
        for (needed, directory, relative) in [
            (!manifest.agents.is_empty(), root.join("agents"), "agents"),
            (!manifest.skills.is_empty(), root.join("skills"), "skills"),
        ] {
            if needed && !validate_inventory_parent(&mut builder, &directory, relative) {
                return builder.finish();
            }
        }
        let mut agents = Vec::new();
        let mut skills = Vec::new();
        for name in &manifest.agents {
            agents.push((root.join("agents").join(name), format!("agents/{name}")));
        }
        for name in &manifest.skills {
            skills.push((
                root.join("skills").join(name).join("SKILL.md"),
                format!("skills/{name}/SKILL.md"),
            ));
        }
        let installed_text = verify_manifest_digests(&mut builder, &manifest);
        (agents, skills, Some(manifest), installed_text)
    };

    let mut components = Vec::new();
    for (path, relative) in agent_paths {
        if let Some(component) = load_workflow_component(
            &mut builder,
            &path,
            &relative,
            "agent",
            installed_text.remove(&path),
        ) {
            components.push(component);
        }
    }
    for (path, relative) in skill_paths {
        if let Some(component) = load_workflow_component(
            &mut builder,
            &path,
            &relative,
            "skill",
            installed_text.remove(&path),
        ) {
            components.push(component);
        }
    }

    let registration_required = installed
        .as_ref()
        .is_some_and(|manifest| manifest.client == "claude")
        && components.iter().any(|component| {
            component.kind == "agent" && component.servers.iter().any(|server| server == "mmcg")
        });
    let registered = if registration_required {
        registered_servers_for_installed(&mut builder)
    } else {
        BTreeSet::new()
    };

    analyze_workflow_components(&mut builder, &components, &registered, installed.as_ref());
    builder.finish()
}

fn validate_inventory_parent(
    builder: &mut WorkflowAuditBuilder,
    directory: &Path,
    relative: &str,
) -> bool {
    let Some(canonical_root) = builder.canonical_root.clone() else {
        return false;
    };
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) => {
            builder.incomplete(
                "file_read_failed",
                format!(
                    "cannot inspect installed inventory {}: {error}",
                    directory.display()
                ),
                None,
                Some(relative),
            );
            return false;
        }
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        builder.incomplete(
            "file_type_invalid",
            format!(
                "installed inventory is not a regular no-follow directory: {}",
                directory.display()
            ),
            None,
            Some(relative),
        );
        return false;
    }
    match directory.canonicalize() {
        Ok(path) if path.starts_with(canonical_root) => true,
        Ok(_) => {
            builder.incomplete(
                "file_outside_root",
                format!(
                    "installed inventory escapes its root: {}",
                    directory.display()
                ),
                None,
                Some(relative),
            );
            false
        }
        Err(error) => {
            builder.incomplete(
                "file_read_failed",
                format!(
                    "cannot canonicalize installed inventory {}: {error}",
                    directory.display()
                ),
                None,
                Some(relative),
            );
            false
        }
    }
}

pub(crate) fn audit_workflow_for_doctor(root: &Path) -> Option<WorkflowAuditReport> {
    if is_plain_directory(&root.join("agents/subagents"))
        && is_plain_directory(&root.join("skills"))
    {
        return Some(audit_workflow(root));
    }
    let project_install = root.join(".claude");
    if project_install.join(".mastermind-workflow.json").exists() {
        return Some(audit_workflow(&project_install));
    }
    std::env::home_dir()
        .map(|home| home.join(".claude"))
        .filter(|install| install.join(".mastermind-workflow.json").exists())
        .map(|install| audit_workflow(&install))
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "error" => 0,
        "warning" => 1,
        _ => 2,
    }
}

fn is_plain_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn path_is_safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !path.is_absolute()
        && !value.contains('\\')
        && !value
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        && path.components().all(|component| {
            matches!(component, Component::Normal(_))
                && component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|part| !part.is_empty() && part != "." && part != "..")
        })
}

fn safe_artifact_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && Path::new(value)
            .file_name()
            .is_some_and(|name| name == value)
}

#[cfg(unix)]
fn open_nofollow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        // A FIFO would block the open until a writer appears.
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(windows)]
fn open_nofollow(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(unix)]
fn open_directory_nofollow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
}

#[cfg(windows)]
fn open_directory_nofollow(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_directory_nofollow(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_nofollow(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

fn verify_current_file_identity(
    path: &Path,
    file: &File,
    opened_before: crate::bounded_fs::StableFileIdentity,
) -> Result<(), FileReadFailure> {
    let opened_after =
        crate::bounded_fs::stable_file_identity(file).map_err(|error| FileReadFailure {
            code: "file_identity_changed",
            message: format!(
                "cannot re-establish file identity for {}: {error}",
                path.display()
            ),
        })?;
    let current_file = open_nofollow(path).map_err(|error| FileReadFailure {
        code: "file_identity_changed",
        message: format!(
            "cannot re-open {} without following links: {error}",
            path.display()
        ),
    })?;
    let current_identity =
        crate::bounded_fs::stable_file_identity(&current_file).map_err(|error| {
            FileReadFailure {
                code: "file_identity_changed",
                message: format!(
                    "cannot verify current file identity for {}: {error}",
                    path.display()
                ),
            }
        })?;
    if opened_before != opened_after || opened_after != current_identity {
        return Err(FileReadFailure {
            code: "file_identity_changed",
            message: format!(
                "workflow input changed identity or contents during read: {}",
                path.display()
            ),
        });
    }
    Ok(())
}

fn read_bounded_directory(
    path: &Path,
    allowed_root: &Path,
    budget: &mut DirectoryBudget,
) -> Result<Vec<std::ffi::OsString>, FileReadFailure> {
    budget.directories = budget.directories.saturating_add(1);
    if budget.directories > MAX_WORKFLOW_DIRECTORIES {
        return Err(FileReadFailure {
            code: "workflow_inventory_limit_exceeded",
            message: format!("workflow inventory exceeds {MAX_WORKFLOW_DIRECTORIES} directories"),
        });
    }
    let before = fs::symlink_metadata(path).map_err(|error| FileReadFailure {
        code: "file_read_failed",
        message: format!("cannot inspect directory {}: {error}", path.display()),
    })?;
    if before.file_type().is_symlink() || !before.file_type().is_dir() {
        return Err(FileReadFailure {
            code: "file_type_invalid",
            message: format!(
                "workflow inventory path is not a regular no-follow directory: {}",
                path.display()
            ),
        });
    }
    let before_path = path.canonicalize().map_err(|error| FileReadFailure {
        code: "file_read_failed",
        message: format!("cannot canonicalize directory {}: {error}", path.display()),
    })?;
    if !before_path.starts_with(allowed_root) {
        return Err(FileReadFailure {
            code: "file_outside_root",
            message: format!(
                "workflow inventory directory escapes its allowed root: {}",
                path.display()
            ),
        });
    }
    let directory_file = open_directory_nofollow(path).map_err(|error| FileReadFailure {
        code: "file_read_failed",
        message: format!(
            "cannot open directory {} without following links: {error}",
            path.display()
        ),
    })?;
    let opened_before =
        crate::bounded_fs::stable_file_identity(&directory_file).map_err(|error| {
            FileReadFailure {
                code: "file_identity_changed",
                message: format!(
                    "cannot establish directory identity for {}: {error}",
                    path.display()
                ),
            }
        })?;
    let directory = cap_std::fs::Dir::from_std_file(directory_file);
    let iterator = directory.entries().map_err(|error| FileReadFailure {
        code: "file_read_failed",
        message: format!("cannot enumerate directory {}: {error}", path.display()),
    })?;
    let mut entries = Vec::new();
    for entry in iterator {
        let entry = entry.map_err(|error| FileReadFailure {
            code: "file_read_failed",
            message: format!("cannot enumerate directory {}: {error}", path.display()),
        })?;
        budget.entries = budget.entries.saturating_add(1);
        if budget.entries > MAX_WORKFLOW_DIRECTORY_ENTRIES {
            return Err(FileReadFailure {
                code: "workflow_inventory_limit_exceeded",
                message: format!(
                    "workflow inventory exceeds {MAX_WORKFLOW_DIRECTORY_ENTRIES} directory entries"
                ),
            });
        }
        entries.push(entry.file_name());
    }
    let directory_file = directory.into_std_file();
    let opened_after =
        crate::bounded_fs::stable_file_identity(&directory_file).map_err(|error| {
            FileReadFailure {
                code: "file_identity_changed",
                message: format!(
                    "cannot re-establish directory identity for {}: {error}",
                    path.display()
                ),
            }
        })?;
    let after_path = path.canonicalize().map_err(|error| FileReadFailure {
        code: "file_identity_changed",
        message: format!(
            "workflow inventory directory changed during read: {} ({error})",
            path.display()
        ),
    })?;
    let current_directory = open_directory_nofollow(path).map_err(|error| FileReadFailure {
        code: "file_identity_changed",
        message: format!(
            "cannot re-open directory {} without following links: {error}",
            path.display()
        ),
    })?;
    let current_identity =
        crate::bounded_fs::stable_file_identity(&current_directory).map_err(|error| {
            FileReadFailure {
                code: "file_identity_changed",
                message: format!(
                    "cannot verify current directory identity for {}: {error}",
                    path.display()
                ),
            }
        })?;
    if before_path != after_path
        || !after_path.starts_with(allowed_root)
        || opened_before != opened_after
        || opened_after != current_identity
    {
        return Err(FileReadFailure {
            code: "file_identity_changed",
            message: format!(
                "workflow inventory directory changed identity during read: {}",
                path.display()
            ),
        });
    }
    entries.sort();
    Ok(entries)
}

fn read_bounded_text(
    path: &Path,
    allowed_root: &Path,
    max_bytes: usize,
) -> Result<String, FileReadFailure> {
    let over_limit = || FileReadFailure {
        code: "file_limit_exceeded",
        message: format!(
            "workflow input exceeds {max_bytes} bytes: {}",
            path.display()
        ),
    };
    let bytes = read_bounded_bytes(path, allowed_root, max_bytes, &over_limit)?;
    String::from_utf8(bytes).map_err(|_| FileReadFailure {
        code: "file_encoding_invalid",
        message: format!("workflow input is not valid UTF-8: {}", path.display()),
    })
}

fn collect_source_agents(
    builder: &mut WorkflowAuditBuilder,
    directory: &Path,
) -> Vec<(PathBuf, String)> {
    let Some(files) = builder.read_inventory_directory(directory, "agents/subagents") else {
        return Vec::new();
    };
    let mut selected = Vec::new();
    for file_name in files {
        let path = directory.join(file_name);
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let relative = match relative_identity(&builder.root, &path) {
            Ok(relative) => relative,
            Err(()) => {
                builder.incomplete(
                    "workflow_inventory_path_invalid",
                    "workflow agent path must have an exact repository-relative UTF-8 identity",
                    None,
                    Some("agents/subagents"),
                );
                continue;
            }
        };
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            builder.incomplete(
                "file_read_failed",
                format!("cannot inspect {}", path.display()),
                None,
                Some(&relative),
            );
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            builder.incomplete(
                "file_type_invalid",
                format!(
                    "workflow agent is not a regular no-follow file: {}",
                    path.display()
                ),
                None,
                Some(&relative),
            );
            continue;
        }
        selected.push((path, relative));
        if selected.len() > MAX_WORKFLOW_AGENTS {
            builder.incomplete(
                "workflow_inventory_limit_exceeded",
                format!("workflow inventory exceeds {MAX_WORKFLOW_AGENTS} agents"),
                None,
                Some("agents/subagents"),
            );
            selected.truncate(MAX_WORKFLOW_AGENTS);
            break;
        }
    }
    selected
}

fn collect_source_skills(
    builder: &mut WorkflowAuditBuilder,
    directory: &Path,
) -> Vec<(PathBuf, String)> {
    fn visit(
        builder: &mut WorkflowAuditBuilder,
        directory: &Path,
        depth: usize,
        selected: &mut Vec<(PathBuf, String)>,
        overflowed: &mut bool,
    ) {
        if *overflowed {
            return;
        }
        let relative_directory = match relative_identity(&builder.root, directory) {
            Ok(relative) => relative,
            Err(()) => {
                builder.incomplete(
                    "workflow_inventory_path_invalid",
                    "workflow skill directory must have an exact repository-relative UTF-8 identity",
                    None,
                    Some("skills"),
                );
                return;
            }
        };
        if depth > MAX_WORKFLOW_YAML_DEPTH {
            builder.incomplete(
                "workflow_inventory_limit_exceeded",
                "workflow skill directory nesting exceeds 16 levels",
                None,
                Some(&relative_directory),
            );
            return;
        }
        let Some(entries) = builder.read_inventory_directory(directory, &relative_directory) else {
            return;
        };
        for file_name in entries {
            if *overflowed {
                return;
            }
            let path = directory.join(file_name);
            let relative = match relative_identity(&builder.root, &path) {
                Ok(relative) => relative,
                Err(()) => {
                    builder.incomplete(
                        "workflow_inventory_path_invalid",
                        "workflow skill path must have an exact repository-relative UTF-8 identity",
                        None,
                        Some(&relative_directory),
                    );
                    continue;
                }
            };
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                builder.incomplete(
                    "file_read_failed",
                    format!("cannot inspect {}", path.display()),
                    None,
                    Some(&relative),
                );
                continue;
            };
            if metadata.file_type().is_symlink() {
                builder.incomplete(
                    "file_type_invalid",
                    format!(
                        "workflow skill inventory contains a symbolic link: {}",
                        path.display()
                    ),
                    None,
                    Some(&relative),
                );
                continue;
            }
            if metadata.file_type().is_dir() {
                visit(builder, &path, depth + 1, selected, overflowed);
            } else if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                if metadata.file_type().is_file() {
                    selected.push((path, relative));
                } else {
                    builder.incomplete(
                        "file_type_invalid",
                        format!(
                            "workflow skill is not a regular no-follow file: {}",
                            path.display()
                        ),
                        None,
                        Some(&relative),
                    );
                }
                if selected.len() > MAX_WORKFLOW_SKILLS {
                    *overflowed = true;
                    builder.incomplete(
                        "workflow_inventory_limit_exceeded",
                        format!("workflow inventory exceeds {MAX_WORKFLOW_SKILLS} skills"),
                        None,
                        Some("skills"),
                    );
                    selected.truncate(MAX_WORKFLOW_SKILLS);
                    return;
                }
            }
        }
    }

    let mut selected = Vec::new();
    let mut overflowed = false;
    visit(builder, directory, 0, &mut selected, &mut overflowed);
    selected.sort_by(|left, right| left.1.cmp(&right.1));
    selected
}

fn relative_identity(root: &Path, path: &Path) -> Result<String, ()> {
    let relative = path.strip_prefix(root).map_err(|_| ())?;
    crate::bounded_fs::normalize_repository_relative_path(relative).map_err(|_| ())
}

fn load_installed_manifest(
    builder: &mut WorkflowAuditBuilder,
    path: &Path,
) -> Option<InstalledWorkflowManifest> {
    let canonical_root = builder.canonical_root.clone()?;
    let text = read_bounded_text(path, &canonical_root, MAX_WORKFLOW_MANIFEST_BYTES)
        .map_err(|failure| {
            let code = if failure.code == "file_limit_exceeded" {
                "manifest_limit_exceeded"
            } else {
                failure.code
            };
            builder.incomplete(
                code,
                failure.message,
                None,
                Some(".mastermind-workflow.json"),
            );
        })
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| {
            builder.incomplete(
                "manifest_invalid",
                format!("invalid workflow ownership manifest: {error}"),
                None,
                Some(".mastermind-workflow.json"),
            );
        })
        .ok()?;
    let object = value
        .as_object()
        .ok_or(())
        .map_err(|_| {
            builder.incomplete(
                "manifest_invalid",
                "workflow ownership manifest must be a JSON object",
                None,
                Some(".mastermind-workflow.json"),
            );
        })
        .ok()?;
    let schema = object
        .get("schema_version")
        .and_then(serde_json::Value::as_u64);
    if !matches!(schema, Some(1 | 2)) {
        builder.incomplete(
            "manifest_invalid",
            "workflow ownership manifest schema_version must be 1 or 2",
            None,
            Some(".mastermind-workflow.json"),
        );
        return None;
    }
    let expected: BTreeSet<&str> = if schema == Some(2) {
        [
            "artifacts",
            "client",
            "digests",
            "package",
            "profile",
            "schema_version",
            "version",
        ]
        .into_iter()
        .collect()
    } else {
        [
            "artifacts",
            "client",
            "digests",
            "package",
            "schema_version",
            "version",
        ]
        .into_iter()
        .collect()
    };
    let actual: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    if actual != expected
        || object.get("package").and_then(serde_json::Value::as_str)
            != Some("@xcraftmind/mastermind")
        || object
            .get("version")
            .and_then(serde_json::Value::as_str)
            .is_none()
    {
        builder.incomplete(
            "manifest_invalid",
            "workflow ownership manifest has unsupported fields or package identity",
            None,
            Some(".mastermind-workflow.json"),
        );
        return None;
    }
    let Some(client) = object
        .get("client")
        .and_then(serde_json::Value::as_str)
        .filter(|client| matches!(*client, "claude" | "codex"))
    else {
        builder.incomplete(
            "manifest_invalid",
            "workflow ownership manifest client must be claude or codex",
            None,
            Some(".mastermind-workflow.json"),
        );
        return None;
    };
    let profile = if schema == Some(1) {
        "full"
    } else {
        let Some(profile) = object
            .get("profile")
            .and_then(serde_json::Value::as_str)
            .filter(|profile| matches!(*profile, "core" | "frontend" | "security" | "full"))
        else {
            builder.incomplete(
                "manifest_invalid",
                "workflow ownership manifest profile is unsupported",
                None,
                Some(".mastermind-workflow.json"),
            );
            return None;
        };
        profile
    };
    let Some(artifacts) = object
        .get("artifacts")
        .and_then(serde_json::Value::as_object)
    else {
        builder.incomplete(
            "manifest_invalid",
            "workflow ownership manifest artifacts must be an object",
            None,
            Some(".mastermind-workflow.json"),
        );
        return None;
    };
    if artifacts
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != ["agents", "skills"].into_iter().collect()
    {
        builder.incomplete(
            "manifest_invalid",
            "workflow ownership manifest artifacts fields are unsupported",
            None,
            Some(".mastermind-workflow.json"),
        );
        return None;
    }
    let agents = manifest_string_array(artifacts.get("agents"), MAX_WORKFLOW_AGENTS);
    let skills = manifest_string_array(artifacts.get("skills"), MAX_WORKFLOW_SKILLS);
    let (Ok(agents), Ok(skills)) = (agents, skills) else {
        builder.incomplete(
            "manifest_invalid",
            "workflow ownership manifest artifact lists are invalid, duplicated, or over limit",
            None,
            Some(".mastermind-workflow.json"),
        );
        return None;
    };
    if client == "codex" && !agents.is_empty() {
        builder.incomplete(
            "manifest_invalid",
            "Codex workflow manifests cannot own Claude agent artifacts",
            None,
            Some(".mastermind-workflow.json"),
        );
        return None;
    }
    let Some(digest_object) = object.get("digests").and_then(serde_json::Value::as_object) else {
        builder.incomplete(
            "manifest_invalid",
            "workflow ownership manifest digests must be an object",
            None,
            Some(".mastermind-workflow.json"),
        );
        return None;
    };
    let expected_digest_keys: BTreeSet<String> = agents
        .iter()
        .map(|name| format!("agents/{name}"))
        .chain(skills.iter().map(|name| format!("skills/{name}")))
        .collect();
    let actual_digest_keys: BTreeSet<String> = digest_object.keys().cloned().collect();
    if expected_digest_keys != actual_digest_keys {
        builder.incomplete(
            "manifest_invalid",
            "workflow ownership manifest digest keys do not match owned artifacts",
            None,
            Some(".mastermind-workflow.json"),
        );
        return None;
    }
    let mut digests = BTreeMap::new();
    for (key, value) in digest_object {
        let Some(digest) = value.as_str().filter(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) else {
            builder.incomplete(
                "manifest_invalid",
                format!("workflow ownership manifest has an invalid digest for {key}"),
                None,
                Some(".mastermind-workflow.json"),
            );
            return None;
        };
        digests.insert(key.clone(), digest.to_string());
    }
    Some(InstalledWorkflowManifest {
        client: client.into(),
        profile: profile.into(),
        agents,
        skills,
        digests,
    })
}

fn manifest_string_array(
    value: Option<&serde_json::Value>,
    limit: usize,
) -> Result<Vec<String>, ()> {
    let values = value.and_then(serde_json::Value::as_array).ok_or(())?;
    if values.len() > limit {
        return Err(());
    }
    let strings = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| safe_artifact_name(value))
                .map(str::to_string)
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(())?;
    if strings.iter().collect::<BTreeSet<_>>().len() != strings.len() {
        return Err(());
    }
    Ok(strings)
}

fn load_workflow_component(
    builder: &mut WorkflowAuditBuilder,
    path: &Path,
    relative: &str,
    kind: &str,
    cached_text: Option<String>,
) -> Option<LoadedWorkflowComponent> {
    let text = match cached_text {
        Some(text) => builder.admit_markdown_text(text, relative)?,
        None => builder.read_owned_markdown(path, relative)?,
    };
    let (frontmatter_text, body) = split_frontmatter(&text)
        .map_err(|message| {
            builder.incomplete("frontmatter_invalid", message, None, Some(relative));
        })
        .ok()?;
    validate_yaml_safety(&frontmatter_text)
        .map_err(|message| {
            builder.incomplete("unsafe_yaml", message, None, Some(relative));
        })
        .ok()?;
    let frontmatter: serde_norway::Value = serde_norway::from_str(&frontmatter_text)
        .map_err(|error| {
            builder.incomplete(
                "frontmatter_invalid",
                format!("invalid YAML frontmatter: {error}"),
                None,
                Some(relative),
            );
        })
        .ok()?;
    if yaml_depth(&frontmatter) > MAX_WORKFLOW_YAML_DEPTH {
        builder.incomplete(
            "yaml_depth_exceeded",
            format!("YAML frontmatter exceeds depth {MAX_WORKFLOW_YAML_DEPTH}"),
            None,
            Some(relative),
        );
        return None;
    }
    let Some(_mapping) = frontmatter.as_mapping() else {
        builder.incomplete(
            "frontmatter_invalid",
            "YAML frontmatter must be a mapping",
            None,
            Some(relative),
        );
        return None;
    };
    let expected_id = if kind == "agent" {
        path.file_stem().and_then(|value| value.to_str())
    } else {
        path.parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
    };
    let id = frontmatter
        .get("name")
        .and_then(serde_norway::Value::as_str)
        .unwrap_or_default();
    if id.is_empty() || expected_id != Some(id) || !valid_slug(id) {
        builder.incomplete(
            "component_identity_invalid",
            "component name must be a canonical slug matching its artifact path",
            None,
            Some(relative),
        );
        return None;
    }
    let workflow = match frontmatter.get("workflow") {
        Some(value) => match serde_norway::from_value::<WorkflowMetadata>(value.clone()) {
            Ok(metadata) if metadata.schema_version == WORKFLOW_AUDIT_SCHEMA_VERSION => {
                Some(metadata)
            }
            Ok(_) => {
                builder.report.complete = false;
                builder.diagnostic(
                    "workflow_metadata_invalid",
                    "error",
                    "workflow metadata schema_version must be 1",
                    Some(id),
                    Some(relative),
                    None,
                );
                None
            }
            Err(error) => {
                builder.report.complete = false;
                builder.diagnostic(
                    "workflow_metadata_invalid",
                    "error",
                    format!("invalid or unknown workflow metadata field: {error}"),
                    Some(id),
                    Some(relative),
                    None,
                );
                None
            }
        },
        None if kind == "agent" => {
            builder.report.complete = false;
            builder.diagnostic(
                "workflow_metadata_missing",
                "error",
                "managed agents must declare versioned workflow metadata",
                Some(id),
                Some(relative),
                None,
            );
            None
        }
        None => None,
    };
    if let Some(metadata) = &workflow {
        if metadata.skills.len() > MAX_WORKFLOW_RELATIONS_PER_COMPONENT
            || metadata.writes.len() > MAX_WORKFLOW_WRITES_PER_COMPONENT
        {
            builder.incomplete(
                "workflow_declaration_limit_exceeded",
                format!(
                    "workflow metadata exceeds {MAX_WORKFLOW_RELATIONS_PER_COMPONENT} skill relations or {MAX_WORKFLOW_WRITES_PER_COMPONENT} write declarations"
                ),
                Some(id),
                Some(relative),
            );
            return None;
        }
        let relation_ids = metadata
            .skills
            .iter()
            .map(|relation| relation.id.as_str())
            .collect::<BTreeSet<_>>();
        if relation_ids.len() != metadata.skills.len()
            || metadata
                .skills
                .iter()
                .any(|relation| !valid_slug(&relation.id))
        {
            builder.incomplete(
                "workflow_metadata_invalid",
                "workflow skill relations require unique canonical IDs and explicit required flags",
                Some(id),
                Some(relative),
            );
            return None;
        }
    }
    let tools = match frontmatter.get("tools") {
        Some(serde_norway::Value::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
        Some(serde_norway::Value::Sequence(values)) => match values
            .iter()
            .map(|value| value.as_str().map(str::trim).map(str::to_string))
            .collect::<Option<Vec<_>>>()
        {
            Some(values) => values
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
            None => {
                builder.diagnostic(
                    "tool_allowlist_invalid",
                    "error",
                    "tools must be a scalar or string list",
                    Some(id),
                    Some(relative),
                    None,
                );
                Vec::new()
            }
        },
        None if kind == "agent" => {
            builder.diagnostic(
                "tool_allowlist_invalid",
                "error",
                "agents must declare an explicit non-empty tools allowlist",
                Some(id),
                Some(relative),
                None,
            );
            Vec::new()
        }
        None => Vec::new(),
        Some(_) => {
            builder.diagnostic(
                "tool_allowlist_invalid",
                "error",
                "tools must be a scalar or string list",
                Some(id),
                Some(relative),
                None,
            );
            Vec::new()
        }
    };
    if tools.len() > MAX_WORKFLOW_TOOL_GRANTS_PER_COMPONENT {
        builder.incomplete(
            "workflow_declaration_limit_exceeded",
            format!("tools allowlist exceeds {MAX_WORKFLOW_TOOL_GRANTS_PER_COMPONENT} entries"),
            Some(id),
            Some(relative),
        );
        return None;
    }
    let servers = match frontmatter.get("mcpServers") {
        Some(serde_norway::Value::Sequence(values)) => values
            .iter()
            .map(|value| value.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>(),
        Some(serde_norway::Value::Mapping(values)) => values
            .keys()
            .map(|value| value.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>(),
        None => Some(Vec::new()),
        Some(_) => None,
    };
    let servers = servers.unwrap_or_else(|| {
        builder.diagnostic(
            "mcp_servers_invalid",
            "error",
            "mcpServers must be a string list or mapping",
            Some(id),
            Some(relative),
            None,
        );
        Vec::new()
    });
    if servers.len() > MAX_WORKFLOW_SERVERS_PER_COMPONENT {
        builder.incomplete(
            "workflow_declaration_limit_exceeded",
            format!("mcpServers exceeds {MAX_WORKFLOW_SERVERS_PER_COMPONENT} entries"),
            Some(id),
            Some(relative),
        );
        return None;
    }
    let prompt_tools = prompt_mmcg_references(&body);
    let wikilinks = wikilinks(&body);
    if prompt_tools.len() > MAX_WORKFLOW_TOOL_GRANTS_PER_COMPONENT
        || wikilinks.len() > MAX_WORKFLOW_RELATIONS_PER_COMPONENT
    {
        builder.incomplete(
            "workflow_declaration_limit_exceeded",
            "component body exceeds bounded tool-reference or skill-link discovery",
            Some(id),
            Some(relative),
        );
        return None;
    }
    Some(LoadedWorkflowComponent {
        id: id.to_string(),
        node_id: format!("{kind}:{id}"),
        kind: kind.to_string(),
        path: relative.to_string(),
        text_bytes: body.len(),
        prompt_tools,
        wikilinks,
        tools,
        servers,
        model: frontmatter
            .get("model")
            .and_then(serde_norway::Value::as_str)
            .map(str::to_string),
        max_turns: frontmatter
            .get("maxTurns")
            .and_then(serde_norway::Value::as_u64),
        effort: frontmatter
            .get("effort")
            .and_then(serde_norway::Value::as_str)
            .map(str::to_string),
        workflow,
    })
}

fn split_frontmatter(text: &str) -> Result<(String, String), String> {
    let normalized = text.replace("\r\n", "\n");
    let Some(rest) = normalized.strip_prefix("---\n") else {
        return Err("workflow Markdown must start with YAML frontmatter".into());
    };
    let Some(end) = rest.find("\n---\n") else {
        return Err("workflow Markdown frontmatter is not terminated".into());
    };
    Ok((rest[..end].to_string(), rest[end + 5..].to_string()))
}

fn validate_yaml_safety(frontmatter: &str) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut sequence_index: BTreeMap<usize, usize> = BTreeMap::new();
    for (line_number, line) in frontmatter.lines().enumerate() {
        if line.contains('\t') && line.starts_with(char::is_whitespace) {
            return Err(format!(
                "tabs are not allowed in YAML indentation at line {}",
                line_number + 1
            ));
        }
        let visible = yaml_visible_text(line);
        let trimmed = visible.trim();
        if matches!(trimmed, "---" | "...") {
            return Err(format!(
                "multiple YAML documents are not allowed at line {}",
                line_number + 1
            ));
        }
        for token in trimmed.split(|character: char| {
            character.is_whitespace() || matches!(character, '[' | ']' | '{' | '}' | ',')
        }) {
            if yaml_directive_token(token) {
                return Err(format!(
                    "YAML anchors, aliases, and tags are not allowed at line {}",
                    line_number + 1
                ));
            }
        }
        let indent = visible
            .chars()
            .take_while(|character| *character == ' ')
            .count();
        while stack
            .last()
            .is_some_and(|(parent_indent, _)| *parent_indent >= indent)
        {
            stack.pop();
        }
        let (key_text, key_indent, sequence_item) = if let Some(rest) = trimmed.strip_prefix("- ") {
            let index = sequence_index.entry(indent).or_default();
            *index += 1;
            stack.push((indent, format!("#{}", *index)));
            (rest, indent + 2, true)
        } else {
            (trimmed, indent, false)
        };
        let Some((key, value)) = key_text.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        if key == "<<" {
            return Err(format!(
                "YAML merge keys are not allowed at line {}",
                line_number + 1
            ));
        }
        let parent = stack
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>()
            .join("/");
        if !seen.insert((parent, key.to_string())) {
            return Err(format!(
                "duplicate YAML key {key:?} at line {}",
                line_number + 1
            ));
        }
        if value.trim().is_empty() {
            stack.push((key_indent + usize::from(sequence_item), key.to_string()));
        }
    }
    Ok(())
}

fn yaml_directive_token(token: &str) -> bool {
    let Some(prefix) = token.chars().next() else {
        return false;
    };
    if !matches!(prefix, '&' | '*' | '!') {
        return false;
    }
    let value = &token[prefix.len_utf8()..];
    !value.is_empty()
        && (value.starts_with('<')
            || value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'/')))
}

fn yaml_visible_text(line: &str) -> String {
    let mut visible = String::with_capacity(line.len());
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            visible.push(' ');
            escaped = false;
            continue;
        }
        if double && character == '\\' {
            visible.push(' ');
            escaped = true;
            continue;
        }
        if !double && character == '\'' {
            single = !single;
            visible.push(' ');
            continue;
        }
        if !single && character == '"' {
            double = !double;
            visible.push(' ');
            continue;
        }
        if !single && !double && character == '#' {
            break;
        }
        visible.push(if single || double { ' ' } else { character });
    }
    visible
}

fn yaml_depth(value: &serde_norway::Value) -> usize {
    match value {
        serde_norway::Value::Sequence(values) => {
            1 + values.iter().map(yaml_depth).max().unwrap_or(0)
        }
        serde_norway::Value::Mapping(values) => {
            1 + values
                .iter()
                .map(|(key, value)| yaml_depth(key).max(yaml_depth(value)))
                .max()
                .unwrap_or(0)
        }
        serde_norway::Value::Tagged(_) => MAX_WORKFLOW_YAML_DEPTH + 1,
        _ => 1,
    }
}

fn valid_slug(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn prompt_mmcg_references(body: &str) -> BTreeSet<String> {
    body.split(|character: char| !(character.is_ascii_lowercase() || character == '_'))
        .filter_map(|token| {
            token
                .strip_prefix("mcp__mmcg__")
                .filter(|name| name.starts_with("mmcg_"))
                .or_else(|| token.starts_with("mmcg_").then_some(token))
        })
        .map(str::to_string)
        .collect()
}

fn wikilinks(body: &str) -> BTreeSet<String> {
    let mut links = BTreeSet::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find("]]") else {
            break;
        };
        let candidate = &rest[..end];
        if valid_slug(candidate) {
            links.insert(candidate.to_string());
        }
        rest = &rest[end + 2..];
    }
    links
}

struct DigestBudget {
    files: usize,
    bytes: usize,
    directories: DirectoryBudget,
}

fn verify_manifest_digests(
    builder: &mut WorkflowAuditBuilder,
    manifest: &InstalledWorkflowManifest,
) -> BTreeMap<PathBuf, String> {
    let mut text_cache = BTreeMap::new();
    let Some(canonical_root) = builder.canonical_root.clone() else {
        return text_cache;
    };
    let mut budget = DigestBudget {
        files: 0,
        bytes: 0,
        directories: DirectoryBudget::default(),
    };
    for (key, expected) in &manifest.digests {
        let path = builder.root.join(key);
        match digest_artifact(&path, &canonical_root, &mut budget, &mut text_cache) {
            Ok(actual) if actual == *expected => {}
            Ok(_) => builder.diagnostic(
                "manifest_digest_mismatch",
                "error",
                format!("installed workflow artifact differs from its ownership digest: {key}"),
                None,
                Some(key),
                Some("manifest_owns"),
            ),
            Err(failure) => builder.incomplete(failure.code, failure.message, None, Some(key)),
        }
    }
    text_cache
}

fn digest_artifact(
    path: &Path,
    canonical_root: &Path,
    budget: &mut DigestBudget,
    text_cache: &mut BTreeMap<PathBuf, String>,
) -> Result<String, FileReadFailure> {
    let metadata = fs::symlink_metadata(path).map_err(|error| FileReadFailure {
        code: "file_read_failed",
        message: format!(
            "cannot inspect installed artifact {}: {error}",
            path.display()
        ),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(FileReadFailure {
            code: "file_type_invalid",
            message: format!(
                "installed workflow artifact is a symbolic link: {}",
                path.display()
            ),
        });
    }
    let mut hasher = Sha256::new();
    if metadata.file_type().is_file() {
        let relative = path.file_name().ok_or_else(|| FileReadFailure {
            code: "manifest_invalid",
            message: format!("installed artifact has no file name: {}", path.display()),
        })?;
        let relative = crate::bounded_fs::normalize_repository_relative_path(Path::new(relative))
            .map_err(|_| FileReadFailure {
            code: "manifest_invalid",
            message: format!(
                "installed artifact name is not a canonical UTF-8 path: {}",
                path.display()
            ),
        })?;
        hash_artifact_file(
            path,
            &relative,
            canonical_root,
            budget,
            text_cache,
            &mut hasher,
        )?;
    } else if metadata.file_type().is_dir() {
        hash_artifact_directory(
            path,
            Path::new(""),
            0,
            canonical_root,
            budget,
            text_cache,
            &mut hasher,
        )?;
    } else {
        return Err(FileReadFailure {
            code: "file_type_invalid",
            message: format!(
                "installed workflow artifact is not a regular file or directory: {}",
                path.display()
            ),
        });
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn hash_artifact_directory(
    directory: &Path,
    relative: &Path,
    depth: usize,
    canonical_root: &Path,
    budget: &mut DigestBudget,
    text_cache: &mut BTreeMap<PathBuf, String>,
    hasher: &mut Sha256,
) -> Result<(), FileReadFailure> {
    if depth > MAX_WORKFLOW_YAML_DEPTH {
        return Err(FileReadFailure {
            code: "workflow_inventory_limit_exceeded",
            message: format!(
                "installed artifact directory nesting exceeds {MAX_WORKFLOW_YAML_DEPTH} levels"
            ),
        });
    }
    let entries = read_bounded_directory(directory, canonical_root, &mut budget.directories)?;
    for file_name in entries {
        let path = directory.join(&file_name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| FileReadFailure {
            code: "file_read_failed",
            message: format!(
                "cannot inspect installed artifact {}: {error}",
                path.display()
            ),
        })?;
        if metadata.file_type().is_symlink() {
            return Err(FileReadFailure {
                code: "file_type_invalid",
                message: format!(
                    "installed workflow artifact contains a symbolic link: {}",
                    path.display()
                ),
            });
        }
        let child_relative = relative.join(file_name);
        let relative_text = crate::bounded_fs::normalize_repository_relative_path(&child_relative)
            .map_err(|_| FileReadFailure {
                code: "manifest_invalid",
                message: format!(
                    "installed workflow artifact has an ambiguous path: {}",
                    path.display()
                ),
            })?;
        if metadata.file_type().is_dir() {
            hash_artifact_directory(
                &path,
                Path::new(&relative_text),
                depth + 1,
                canonical_root,
                budget,
                text_cache,
                hasher,
            )?;
        } else if metadata.file_type().is_file() {
            hash_artifact_file(
                &path,
                &relative_text,
                canonical_root,
                budget,
                text_cache,
                hasher,
            )?;
        } else {
            return Err(FileReadFailure {
                code: "file_type_invalid",
                message: format!(
                    "installed workflow artifact contains a non-regular file: {}",
                    path.display()
                ),
            });
        }
    }
    Ok(())
}

fn hash_artifact_file(
    path: &Path,
    relative: &str,
    canonical_root: &Path,
    budget: &mut DigestBudget,
    text_cache: &mut BTreeMap<PathBuf, String>,
    hasher: &mut Sha256,
) -> Result<(), FileReadFailure> {
    budget.files = budget.files.saturating_add(1);
    if budget.files > MAX_WORKFLOW_ARTIFACT_FILES {
        return Err(FileReadFailure {
            code: "workflow_inventory_limit_exceeded",
            message: format!("installed artifacts exceed {MAX_WORKFLOW_ARTIFACT_FILES} files"),
        });
    }
    let remaining = MAX_WORKFLOW_TEXT_BYTES.saturating_sub(budget.bytes);
    let over_limit = || FileReadFailure {
        code: "workflow_total_text_limit_exceeded",
        message: format!(
            "installed artifacts exceed the {MAX_WORKFLOW_TEXT_BYTES}-byte read limit"
        ),
    };
    let bytes = read_bounded_bytes(path, canonical_root, remaining, &over_limit)?;
    budget.bytes = budget.bytes.saturating_add(bytes.len());
    if path.extension().and_then(|value| value.to_str()) == Some("md") {
        if bytes.len() > MAX_WORKFLOW_MARKDOWN_BYTES {
            return Err(FileReadFailure {
                code: "file_limit_exceeded",
                message: format!(
                    "workflow Markdown exceeds {MAX_WORKFLOW_MARKDOWN_BYTES} bytes: {}",
                    path.display()
                ),
            });
        }
        let text = String::from_utf8(bytes.clone()).map_err(|_| FileReadFailure {
            code: "file_encoding_invalid",
            message: format!("workflow Markdown is not valid UTF-8: {}", path.display()),
        })?;
        text_cache.insert(path.to_path_buf(), text);
    }
    hasher.update(b"file\0");
    hasher.update(relative.as_bytes());
    hasher.update(b"\0");
    hasher.update(bytes);
    hasher.update(b"\0");
    Ok(())
}

/// Read a regular no-follow file under `allowed_root`, failing with
/// `over_limit` when it holds more than `max_bytes`.
fn read_bounded_bytes(
    path: &Path,
    allowed_root: &Path,
    max_bytes: usize,
    over_limit: &dyn Fn() -> FileReadFailure,
) -> Result<Vec<u8>, FileReadFailure> {
    let before = fs::symlink_metadata(path).map_err(|error| FileReadFailure {
        code: "file_read_failed",
        message: format!("cannot inspect {}: {error}", path.display()),
    })?;
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err(FileReadFailure {
            code: "file_type_invalid",
            message: format!(
                "workflow input is not a regular no-follow file: {}",
                path.display()
            ),
        });
    }
    if before.len() > max_bytes as u64 {
        return Err(over_limit());
    }
    let before_path = path.canonicalize().map_err(|error| FileReadFailure {
        code: "file_read_failed",
        message: format!("cannot canonicalize {}: {error}", path.display()),
    })?;
    if !before_path.starts_with(allowed_root) {
        return Err(FileReadFailure {
            code: "file_outside_root",
            message: format!(
                "workflow input escapes its allowed root: {}",
                path.display()
            ),
        });
    }
    let mut file = open_nofollow(path).map_err(|error| FileReadFailure {
        code: "file_read_failed",
        message: format!(
            "cannot open {} without following links: {error}",
            path.display()
        ),
    })?;
    let opened_before =
        crate::bounded_fs::stable_file_identity(&file).map_err(|error| FileReadFailure {
            code: "file_identity_changed",
            message: format!(
                "cannot establish file identity for {}: {error}",
                path.display()
            ),
        })?;
    let mut bytes = Vec::with_capacity(before.len() as usize);
    file.by_ref()
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| FileReadFailure {
            code: "file_read_failed",
            message: format!("cannot read {}: {error}", path.display()),
        })?;
    if bytes.len() > max_bytes {
        return Err(over_limit());
    }
    verify_current_file_identity(path, &file, opened_before)?;
    let after_path = path.canonicalize().map_err(|error| FileReadFailure {
        code: "file_identity_changed",
        message: format!(
            "workflow input changed during read: {} ({error})",
            path.display()
        ),
    })?;
    if before_path != after_path || !after_path.starts_with(allowed_root) {
        return Err(FileReadFailure {
            code: "file_identity_changed",
            message: format!(
                "workflow input changed identity during read: {}",
                path.display()
            ),
        });
    }
    Ok(bytes)
}

fn registered_servers_for_installed(builder: &mut WorkflowAuditBuilder) -> BTreeSet<String> {
    let mut servers = BTreeSet::new();
    let mut invalid_mmcg_entries = Vec::new();
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    for path in registration_config_candidates(
        &builder.root,
        std::env::home_dir().as_deref(),
        config_dir.as_deref(),
    ) {
        if fs::symlink_metadata(&path).is_err() {
            continue;
        }
        let allowed_root = path
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .or_else(|| builder.canonical_root.clone());
        let Some(allowed_root) = allowed_root else {
            builder.incomplete(
                "mcp_registration_config_invalid",
                format!(
                    "cannot resolve MCP registration config root for {}",
                    path.display()
                ),
                None,
                None,
            );
            continue;
        };
        let text = match read_bounded_text(&path, &allowed_root, MAX_WORKFLOW_MANIFEST_BYTES) {
            Ok(text) => text,
            Err(failure) => {
                builder.incomplete(
                    "mcp_registration_config_invalid",
                    failure.message,
                    None,
                    Some(&path.display().to_string()),
                );
                continue;
            }
        };
        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                builder.incomplete(
                    "mcp_registration_config_invalid",
                    format!(
                        "invalid MCP registration JSON at {}: {error}",
                        path.display()
                    ),
                    None,
                    Some(&path.display().to_string()),
                );
                continue;
            }
        };
        let Some(mapping) = value
            .get("mcpServers")
            .and_then(serde_json::Value::as_object)
        else {
            continue;
        };
        if let Some(entry) = mapping.get("mmcg") {
            if valid_mmcg_registration_entry(entry) {
                servers.insert("mmcg".into());
            } else {
                invalid_mmcg_entries.push(path);
            }
        }
    }
    if !servers.contains("mmcg") {
        for path in invalid_mmcg_entries {
            builder.diagnostic(
                "mcp_registration_entry_invalid",
                "error",
                "Claude mmcg registration must match a supported Mastermind stdio launcher",
                None,
                Some(&path.display().to_string()),
                Some("server_registered_for_client"),
            );
        }
    }
    servers
}

/// The project `.mcp.json` and the user file that `setup` writes for Claude,
/// so the audit checks the same registration the installer made.
fn registration_config_candidates(
    installed_root: &Path,
    home: Option<&Path>,
    config_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let installed = installed_root.canonicalize().ok();
    let user_install = config_dir
        .map(Path::to_path_buf)
        .or_else(|| home.map(|home| home.join(".claude")))
        .and_then(|user_root| user_root.canonicalize().ok())
        .is_some_and(|candidate| Some(candidate) == installed);
    let mut candidates = Vec::new();
    if !user_install {
        if let Some(project) = installed_root.parent() {
            candidates.push(project.join(".mcp.json"));
        }
    }
    if let Some(path) = home.and_then(|home| {
        crate::setup::user_registration_path(crate::setup::Client::Claude, home, config_dir).ok()
    }) {
        candidates.push(path);
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

pub(crate) fn valid_mmcg_registration_entry(value: &serde_json::Value) -> bool {
    let Some(entry) = value.as_object() else {
        return false;
    };
    if entry
        .keys()
        .any(|key| !matches!(key.as_str(), "command" | "args" | "type" | "env"))
    {
        return false;
    }
    let Some(command) = entry
        .get("command")
        .and_then(serde_json::Value::as_str)
        .filter(|command| bounded_registration_string(command))
    else {
        return false;
    };
    let Some(args) = entry
        .get("args")
        .and_then(serde_json::Value::as_array)
        .filter(|args| !args.is_empty() && args.len() <= 8)
        .and_then(|args| {
            args.iter()
                .map(serde_json::Value::as_str)
                .collect::<Option<Vec<_>>>()
        })
        .filter(|args| {
            args.iter()
                .all(|argument| bounded_registration_string(argument))
        })
    else {
        return false;
    };
    let type_valid = entry
        .get("type")
        .is_none_or(|value| value.as_str() == Some("stdio"));
    let env_valid = entry.get("env").is_none_or(|value| {
        value.as_object().is_some_and(|env| {
            env.is_empty()
                || (env.len() == 1
                    && env
                        .get("MMCG_PROFILE_CLIENT")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(crate::miner::access::valid_client_id))
        })
    });
    type_valid && env_valid && canonical_mmcg_launcher(command, &args)
}

fn bounded_registration_string(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4_096 && !value.chars().any(char::is_control)
}

fn canonical_mmcg_launcher(command: &str, args: &[&str]) -> bool {
    if let [launcher, "serve"] = args {
        if crate::setup::npm_project_launcher(Path::new(command), Path::new(launcher)) {
            return true;
        }
    }
    if cfg!(windows) {
        if args == ["serve"] && canonical_mmcg_command(command) {
            return true;
        }
        if !command.eq_ignore_ascii_case("cmd.exe") || !args.starts_with(&["/d", "/s", "/c"]) {
            return false;
        }
        let wrapped = &args[3..];
        return (wrapped == [r".\node_modules\.bin\mastermind.cmd", "serve"])
            || (wrapped == ["mastermind.cmd", "serve"])
            || (wrapped.len() == 4
                && wrapped[0] == "npx.cmd"
                && wrapped[1] == "-y"
                && canonical_mastermind_package(wrapped[2])
                && wrapped[3] == "serve");
    }
    (args == ["serve"] && canonical_mmcg_command(command))
        || (command == "npx"
            && args.len() == 3
            && args[0] == "-y"
            && canonical_mastermind_package(args[1])
            && args[2] == "serve")
}

fn canonical_mmcg_command(command: &str) -> bool {
    if !cfg!(windows)
        && matches!(
            command,
            "mastermind" | "mmcg" | "./node_modules/.bin/mastermind"
        )
    {
        return true;
    }
    let command_path = Path::new(command);
    if !command_path.is_absolute() {
        return false;
    }
    let basename = command.rsplit(['/', '\\']).next().unwrap_or_default();
    let basename_valid = if cfg!(windows) {
        matches!(basename, "mastermind.exe" | "mmcg.exe")
    } else {
        matches!(basename, "mastermind" | "mmcg")
    };
    if !basename_valid {
        return false;
    }
    std::env::current_exe()
        .ok()
        .and_then(|current| current.canonicalize().ok())
        .zip(command_path.canonicalize().ok())
        .is_some_and(|(current, registered)| current == registered)
}

fn canonical_mastermind_package(package: &str) -> bool {
    if package == "@xcraftmind/mastermind" {
        return true;
    }
    package
        .strip_prefix("@xcraftmind/mastermind@")
        .is_some_and(|version| {
            !version.is_empty()
                && version.len() <= 128
                && version.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_')
                })
        })
}

fn valid_runtime_tool_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.'))
}

fn valid_server_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

fn mutation_capable_tool(value: &str) -> bool {
    matches!(value, "Bash" | "Edit" | "Write")
}

fn analyze_workflow_components(
    builder: &mut WorkflowAuditBuilder,
    components: &[LoadedWorkflowComponent],
    registered_servers: &BTreeSet<String>,
    installed: Option<&InstalledWorkflowManifest>,
) {
    let schema_bytes: BTreeMap<String, usize> = crate::mcp::workflow_tool_schemas()
        .into_iter()
        .map(|(name, schema)| {
            (
                name.to_string(),
                serde_json::to_vec(&schema)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0),
            )
        })
        .collect();
    builder.add_node("server:mmcg".into(), "mcp_server", "mmcg".into(), None);
    for tool in schema_bytes.keys() {
        let tool_node = format!("tool:{tool}");
        builder.add_node(tool_node.clone(), "tool", tool.clone(), None);
        builder.add_edge(
            "server:mmcg",
            &tool_node,
            "provides",
            "authoritative_registry",
        );
    }
    builder.add_node(
        "capability:filesystem-mutation".into(),
        "capability",
        "filesystem mutation".into(),
        None,
    );

    let mut component_paths = BTreeMap::new();
    let mut duplicate_identity = false;
    for component in components {
        let key = (component.kind.as_str(), component.id.as_str());
        if let Some(previous) = component_paths.insert(key, component.path.as_str()) {
            duplicate_identity = true;
            builder.incomplete(
                "component_identity_duplicate",
                format!(
                    "{} ID {} is declared by both {previous} and {}",
                    component.kind, component.id, component.path
                ),
                Some(&component.id),
                Some(&component.path),
            );
        }
    }
    for component in components {
        builder.add_node(
            component.node_id.clone(),
            &component.kind,
            component.id.clone(),
            Some(component.path.clone()),
        );
    }
    if duplicate_identity {
        return;
    }
    let skill_by_id: BTreeMap<&str, &LoadedWorkflowComponent> = components
        .iter()
        .filter(|component| component.kind == "skill")
        .map(|component| (component.id.as_str(), component))
        .collect();

    let mut writers = built_in_writer_facts(builder);
    let mut granted_by_tool: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for component in components {
        if component.kind == "agent" {
            validate_agent_runtime(builder, component);
        }
        let mut known_grants = BTreeSet::new();
        if component.kind == "agent" {
            let unique_tools: BTreeSet<&str> = component.tools.iter().map(String::as_str).collect();
            if unique_tools.len() != component.tools.len() {
                builder.diagnostic(
                    "tool_allowlist_duplicate",
                    "error",
                    "tools allowlist contains duplicate entries",
                    Some(&component.id),
                    Some(&component.path),
                    Some("agent_grants_tool"),
                );
            }

            for tool in &component.tools {
                if !valid_runtime_tool_name(tool) {
                    if !(tool.starts_with("mcp__mmcg__") && tool.contains('*')) {
                        builder.diagnostic(
                            "tool_allowlist_invalid",
                            "error",
                            format!("tools allowlist contains an invalid tool name {tool:?}"),
                            Some(&component.id),
                            Some(&component.path),
                            Some("agent_grants_tool"),
                        );
                    }
                    continue;
                }
                if !tool.starts_with("mcp__mmcg__") {
                    let tool_node = format!("tool:{tool}");
                    builder.add_node(tool_node.clone(), "tool", tool.clone(), None);
                    builder.add_edge(&component.node_id, &tool_node, "grants_tool", "declared");
                }
                if mutation_capable_tool(tool) {
                    builder.add_edge(
                        &component.node_id,
                        "capability:filesystem-mutation",
                        "grants_capability",
                        "declared",
                    );
                }
            }

            let scopes_mmcg = component.servers.iter().any(|server| server == "mmcg");
            for server in &component.servers {
                if !valid_server_name(server) {
                    builder.diagnostic(
                        "mcp_servers_invalid",
                        "error",
                        format!("mcpServers contains invalid server name {server:?}"),
                        Some(&component.id),
                        Some(&component.path),
                        Some("agent_scopes_server"),
                    );
                    continue;
                }
                let server_node = format!("server:{server}");
                builder.add_node(server_node.clone(), "mcp_server", server.clone(), None);
                builder.add_edge(
                    &component.node_id,
                    &server_node,
                    "scopes_server",
                    "declared",
                );
            }
            if component.servers.iter().collect::<BTreeSet<_>>().len() != component.servers.len() {
                builder.diagnostic(
                    "mcp_servers_invalid",
                    "error",
                    "mcpServers contains duplicate entries",
                    Some(&component.id),
                    Some(&component.path),
                    Some("agent_scopes_server"),
                );
            }

            let wildcard = component
                .tools
                .iter()
                .any(|tool| tool.starts_with("mcp__mmcg__") && tool.contains('*'));
            if wildcard {
                builder.diagnostic(
                    "mmcg_wildcard_grant",
                    "error",
                    "mmcg tools must be granted by exact name, never by wildcard",
                    Some(&component.id),
                    Some(&component.path),
                    Some("agent_grants_tool"),
                );
            }
            let exact_grants: BTreeSet<String> = component
                .tools
                .iter()
                .filter_map(|tool| tool.strip_prefix("mcp__mmcg__"))
                .filter(|tool| !tool.contains('*'))
                .map(str::to_string)
                .collect();
            known_grants = exact_grants
                .iter()
                .filter(|tool| crate::mcp::is_known_tool(tool) && schema_bytes.contains_key(*tool))
                .cloned()
                .collect();
            for tool in exact_grants.difference(&known_grants) {
                builder.diagnostic(
                    "mmcg_tool_unknown",
                    "error",
                    format!("tools allowlist grants unknown mmcg tool {tool}"),
                    Some(&component.id),
                    Some(&component.path),
                    Some("agent_grants_tool"),
                );
            }
            for tool in &known_grants {
                let tool_node = format!("tool:{tool}");
                builder.add_edge(&component.node_id, &tool_node, "grants_tool", "declared");
                granted_by_tool
                    .entry(tool.clone())
                    .or_default()
                    .insert(component.node_id.clone());
                if !component.prompt_tools.contains(tool) {
                    builder.diagnostic(
                        "tool_grant_unreferenced",
                        "info",
                        format!("exact grant {tool} is not named in the component body"),
                        Some(&component.id),
                        Some(&component.path),
                        Some("agent_grants_tool"),
                    );
                }
            }
            if (!exact_grants.is_empty() || !component.prompt_tools.is_empty() || wildcard)
                && !scopes_mmcg
            {
                builder.diagnostic(
                    "mmcg_server_scope_missing",
                    "error",
                    "component uses mmcg but does not declare mcpServers: [mmcg]",
                    Some(&component.id),
                    Some(&component.path),
                    Some("agent_scopes_server"),
                );
            }
            if scopes_mmcg && known_grants.is_empty() {
                builder.diagnostic(
                    "mmcg_scope_without_grant",
                    "error",
                    "component scopes mmcg without an exact known tool grant",
                    Some(&component.id),
                    Some(&component.path),
                    Some("agent_grants_tool"),
                );
            }
            for referenced in &component.prompt_tools {
                if !schema_bytes.contains_key(referenced) {
                    builder.diagnostic(
                        "mmcg_prompt_tool_unknown",
                        "error",
                        format!("component body references unknown mmcg tool {referenced}"),
                        Some(&component.id),
                        Some(&component.path),
                        Some("body_references_tool"),
                    );
                } else {
                    let tool_node = format!("tool:{referenced}");
                    builder.add_edge(
                        &component.node_id,
                        &tool_node,
                        "references_tool",
                        "sound_text_reference",
                    );
                    if !known_grants.contains(referenced) {
                        builder.diagnostic(
                        "mmcg_prompt_grant_missing",
                        "error",
                        format!("component body references {referenced} but tools does not grant it"),
                        Some(&component.id),
                        Some(&component.path),
                        Some("body_references_tool"),
                    );
                    }
                }
            }
            if installed.is_some_and(|manifest| manifest.client == "claude")
                && scopes_mmcg
                && builder.report.complete
                && !registered_servers.contains("mmcg")
            {
                builder.diagnostic(
                    "mmcg_registration_missing",
                    "error",
                    "installed Claude workflow scopes mmcg, but mmcg is not registered",
                    Some(&component.id),
                    Some(&component.path),
                    Some("server_registered_for_client"),
                );
            }
        }

        if let Some(workflow) = &component.workflow {
            if workflow.activation == WorkflowActivation::Always {
                builder.diagnostic(
                    "role_unconditional",
                    "info",
                    "role activation is unconditional",
                    Some(&component.id),
                    Some(&component.path),
                    Some("role_activation"),
                );
            }
            if workflow.mutability == WorkflowMutability::ReadOnly {
                for capability in ["Edit", "Write"] {
                    if component.tools.iter().any(|tool| tool == capability) {
                        builder.add_edge(
                            &component.node_id,
                            "capability:filesystem-mutation",
                            "grants_capability",
                            "declared",
                        );
                        builder.diagnostic(
                            "readonly_mutation_capability",
                            "error",
                            format!("read-only role grants mutation tool {capability}"),
                            Some(&component.id),
                            Some(&component.path),
                            Some("role_grants_capability"),
                        );
                    }
                }
                if component.tools.iter().any(|tool| tool == "Bash") {
                    builder.diagnostic(
                        "readonly_bash_capability",
                        "warning",
                        "read-only role grants broad Bash capability",
                        Some(&component.id),
                        Some(&component.path),
                        Some("role_grants_capability"),
                    );
                }
            }
            for relation in &workflow.skills {
                let target_node = format!("skill:{}", relation.id);
                if skill_by_id.contains_key(relation.id.as_str()) {
                    builder.add_edge(
                        &component.node_id,
                        &target_node,
                        if relation.required {
                            "requires_skill"
                        } else {
                            "advises_skill"
                        },
                        "declared",
                    );
                } else if relation.required && builder.report.complete {
                    builder.diagnostic(
                        "required_skill_missing",
                        "error",
                        format!(
                            "required skill {} is not owned by this workflow layout",
                            relation.id
                        ),
                        Some(&component.id),
                        Some(&component.path),
                        Some("component_requires_skill"),
                    );
                }
            }
            for declaration in &workflow.writes {
                if validate_write_declaration(builder, component, declaration) {
                    if writers.len() >= MAX_WORKFLOW_WRITERS {
                        if !builder.writer_limit_reported {
                            builder.writer_limit_reported = true;
                            builder.incomplete(
                                "workflow_declaration_limit_exceeded",
                                format!("workflow exceeds {MAX_WORKFLOW_WRITERS} admitted writers"),
                                Some(&component.id),
                                Some(&component.path),
                            );
                        }
                        break;
                    }
                    let writer_id = format!("writer:{}", component.node_id);
                    let artifact_id = format!("artifact:{}", declaration.artifact);
                    builder.add_node(
                        writer_id.clone(),
                        "writer",
                        component.id.clone(),
                        Some(component.path.clone()),
                    );
                    builder.add_node(
                        artifact_id.clone(),
                        "artifact",
                        declaration.artifact.clone(),
                        Some(declaration.path.clone()),
                    );
                    builder.add_edge(&component.node_id, &writer_id, "acts_as_writer", "declared");
                    builder.add_edge(&writer_id, &artifact_id, "writes_artifact", "declared");
                    writers.push(WriterFact {
                        writer_id,
                        declaration: declaration.clone(),
                        activation: workflow.activation,
                    });
                }
            }
        }

        let explicit_relations: BTreeSet<&str> = component
            .workflow
            .as_ref()
            .map(|workflow| {
                workflow
                    .skills
                    .iter()
                    .map(|relation| relation.id.as_str())
                    .collect()
            })
            .unwrap_or_default();
        for link in &component.wikilinks {
            if explicit_relations.contains(link.as_str()) {
                continue;
            }
            if skill_by_id.contains_key(link.as_str()) {
                builder.add_edge(
                    &component.node_id,
                    &format!("skill:{link}"),
                    if component.kind == "skill" {
                        "depends_on_skill"
                    } else {
                        "mentions_skill"
                    },
                    if component.kind == "skill" && installed.is_some() {
                        "manifest_owned_reference"
                    } else {
                        "advisory_text_reference"
                    },
                );
            } else if component.kind == "skill" && installed.is_some() && builder.report.complete {
                builder.diagnostic(
                    "manifest_skill_dependency_missing",
                    "error",
                    format!("manifest-owned skill dependency {link} is absent"),
                    Some(&component.id),
                    Some(&component.path),
                    Some("skill_depends_on_skill"),
                );
            }
        }
        if component.kind == "agent" {
            add_context_estimates(
                builder,
                component,
                &skill_by_id,
                &schema_bytes,
                &known_grants,
            );
        }
    }

    let unreachable = schema_bytes
        .keys()
        .filter(|tool| !granted_by_tool.contains_key(*tool))
        .cloned()
        .collect::<Vec<_>>();
    if !unreachable.is_empty() && builder.report.complete {
        builder.diagnostic(
            "tools_unreachable",
            "info",
            format!("no managed role grants {}", unreachable.join(", ")),
            None,
            None,
            Some("agent_grants_tool"),
        );
    }
    detect_writer_conflicts(builder, &writers);
}

fn validate_agent_runtime(builder: &mut WorkflowAuditBuilder, component: &LoadedWorkflowComponent) {
    match component.model.as_deref() {
        Some(model @ ("haiku" | "sonnet" | "opus")) => {
            let model_node = format!("model:{model}");
            builder.add_node(model_node.clone(), "model", model.into(), None);
            builder.add_edge(&component.node_id, &model_node, "uses_model", "declared");
        }
        _ => builder.diagnostic(
            "model_unsupported",
            "error",
            "agent model must be haiku, sonnet, or opus",
            Some(&component.id),
            Some(&component.path),
            Some("agent_uses_model"),
        ),
    }
    if !component
        .max_turns
        .is_some_and(|turns| (1..=100).contains(&turns))
    {
        builder.diagnostic(
            "max_turns_invalid",
            "error",
            "agent maxTurns must be an integer from 1 to 100",
            Some(&component.id),
            Some(&component.path),
            Some("agent_runtime_limit"),
        );
    }
    if !matches!(
        component.effort.as_deref(),
        Some("low" | "medium" | "high" | "xhigh" | "max")
    ) {
        builder.diagnostic(
            "effort_invalid",
            "error",
            "agent effort is missing or unsupported",
            Some(&component.id),
            Some(&component.path),
            Some("agent_runtime_limit"),
        );
    }
    if component.tools.is_empty() {
        builder.diagnostic(
            "tool_allowlist_invalid",
            "error",
            "agent tools allowlist must be explicit and non-empty",
            Some(&component.id),
            Some(&component.path),
            Some("agent_grants_tool"),
        );
    }
}

fn add_context_estimates(
    builder: &mut WorkflowAuditBuilder,
    component: &LoadedWorkflowComponent,
    component_by_id: &BTreeMap<&str, &LoadedWorkflowComponent>,
    schema_bytes: &BTreeMap<String, usize>,
    known_grants: &BTreeSet<String>,
) {
    builder.add_context_estimate(WorkflowContextEstimate {
        component_id: component.node_id.clone(),
        scenario: "agent_body".into(),
        bytes: Some(component.text_bytes),
        estimated_tokens: Some(estimated_tokens(component.text_bytes)),
        components: vec![component.path.clone()],
        unavailable: Vec::new(),
    });
    let mut skill_ids = BTreeSet::new();
    if let Some(workflow) = &component.workflow {
        skill_ids.extend(workflow.skills.iter().map(|relation| relation.id.clone()));
    }
    if component.kind == "agent" {
        skill_ids.extend(component.wikilinks.iter().cloned());
    }
    for skill_id in skill_ids {
        if let Some(skill) = component_by_id
            .get(skill_id.as_str())
            .filter(|target| target.kind == "skill")
        {
            builder.add_context_estimate(WorkflowContextEstimate {
                component_id: component.node_id.clone(),
                scenario: "advisory_skill_if_loaded".into(),
                bytes: Some(skill.text_bytes),
                estimated_tokens: Some(estimated_tokens(skill.text_bytes)),
                components: vec![format!("skill:{skill_id}")],
                unavailable: Vec::new(),
            });
        } else {
            builder.add_context_estimate(WorkflowContextEstimate {
                component_id: component.node_id.clone(),
                scenario: "advisory_skill_if_loaded".into(),
                bytes: None,
                estimated_tokens: None,
                components: Vec::new(),
                unavailable: vec![format!("skill:{skill_id}")],
            });
        }
    }
    let known_schema_total = known_grants
        .iter()
        .filter_map(|tool| schema_bytes.get(tool))
        .sum::<usize>();
    builder.add_context_estimate(WorkflowContextEstimate {
        component_id: component.node_id.clone(),
        scenario: "known_mmcg_schema".into(),
        bytes: Some(known_schema_total),
        estimated_tokens: Some(estimated_tokens(known_schema_total)),
        components: known_grants
            .iter()
            .map(|tool| format!("tool:{tool}"))
            .collect(),
        unavailable: Vec::new(),
    });
    let built_in = component
        .tools
        .iter()
        .filter(|tool| !tool.starts_with("mcp__"))
        .cloned()
        .collect::<Vec<_>>();
    if !built_in.is_empty() {
        builder.add_context_estimate(WorkflowContextEstimate {
            component_id: component.node_id.clone(),
            scenario: "built_in_schema_unknown".into(),
            bytes: None,
            estimated_tokens: None,
            components: Vec::new(),
            unavailable: built_in,
        });
    }
}

fn estimated_tokens(bytes: usize) -> usize {
    bytes.saturating_add(3) / 4
}

fn valid_artifact_id(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
}

fn valid_write_path(value: &str) -> bool {
    if value.matches("{task}").count() > 1 {
        return false;
    }
    let expanded = value.replace("{task}", "task");
    !expanded.contains('{') && !expanded.contains('}') && path_is_safe_relative(&expanded)
}

fn validate_write_declaration(
    builder: &mut WorkflowAuditBuilder,
    component: &LoadedWorkflowComponent,
    declaration: &WorkflowWriteDeclaration,
) -> bool {
    let valid = valid_artifact_id(&declaration.artifact)
        && valid_write_path(&declaration.path)
        && matches!(declaration.authority.as_str(), "canonical" | "advisory")
        && matches!(
            declaration.runtime.as_str(),
            "claude" | "codex" | "portable" | "controller"
        )
        && valid_slug(&declaration.exclusivity_group);
    if !valid {
        builder.diagnostic(
            "writer_declaration_invalid",
            "error",
            "writer declarations require a canonical artifact ID, safe {task}-only path template, authority, runtime, and exclusivity group",
            Some(&component.id),
            Some(&component.path),
            Some("writer_writes_artifact"),
        );
    }
    valid
}

fn built_in_writer_facts(builder: &mut WorkflowAuditBuilder) -> Vec<WriterFact> {
    let declarations = [
        (
            "controller-state",
            "task.state",
            ".mastermind/tasks/{task}/state.json",
        ),
        (
            "controller-audit",
            "task.audit",
            ".mastermind/tasks/{task}/audit.md",
        ),
        (
            "controller-invocation",
            "task.invocation",
            ".mastermind/tasks/{task}/invocation.json",
        ),
        (
            "controller-review-invocation",
            "task.review-invocation",
            ".mastermind/tasks/{task}/review-invocation.json",
        ),
        (
            "controller-semantic-review",
            "task.semantic-review",
            ".mastermind/tasks/{task}/semantic-review.json",
        ),
        (
            "controller-history-review",
            "task.history-review",
            ".mastermind/tasks/{task}/history-review.md",
        ),
    ];
    declarations
        .into_iter()
        .map(|(name, artifact, path)| {
            let writer_id = format!("writer:{name}");
            let artifact_node = format!("artifact:{artifact}");
            builder.add_node(writer_id.clone(), "writer", name.into(), None);
            builder.add_node(
                artifact_node.clone(),
                "artifact",
                artifact.into(),
                Some(path.into()),
            );
            builder.add_edge(
                &writer_id,
                &artifact_node,
                "writes_artifact",
                "built_in_contract",
            );
            WriterFact {
                writer_id,
                declaration: WorkflowWriteDeclaration {
                    artifact: artifact.into(),
                    path: path.into(),
                    authority: "canonical".into(),
                    runtime: "controller".into(),
                    exclusivity_group: name.into(),
                },
                activation: WorkflowActivation::Always,
            }
        })
        .collect()
}

fn detect_writer_conflicts(builder: &mut WorkflowAuditBuilder, writers: &[WriterFact]) {
    let mut paths_by_artifact: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut artifacts_by_path: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut declarations_by_writer_target: BTreeMap<(&str, &str, &str), usize> = BTreeMap::new();
    let mut target_groups: BTreeMap<(&str, &str), Vec<&WriterFact>> = BTreeMap::new();
    for writer in writers {
        paths_by_artifact
            .entry(&writer.declaration.artifact)
            .or_default()
            .insert(&writer.declaration.path);
        artifacts_by_path
            .entry(&writer.declaration.path)
            .or_default()
            .insert(&writer.declaration.artifact);
        *declarations_by_writer_target
            .entry((
                &writer.writer_id,
                &writer.declaration.artifact,
                &writer.declaration.path,
            ))
            .or_default() += 1;
        if writer.declaration.authority == "canonical" {
            target_groups
                .entry(("artifact", &writer.declaration.artifact))
                .or_default()
                .push(writer);
            target_groups
                .entry(("path", &writer.declaration.path))
                .or_default()
                .push(writer);
        }
    }
    for (artifact, paths) in paths_by_artifact {
        if paths.len() > 1 {
            builder.diagnostic(
                "artifact_definition_conflict",
                "error",
                format!(
                    "artifact {artifact} maps to multiple paths: {}",
                    paths.into_iter().collect::<Vec<_>>().join(", ")
                ),
                None,
                None,
                Some("writer_writes_artifact"),
            );
        }
    }
    for (path, artifacts) in artifacts_by_path {
        if artifacts.len() > 1 {
            builder.diagnostic(
                "artifact_definition_conflict",
                "error",
                format!(
                    "path {path} has multiple artifact IDs: {}",
                    artifacts.into_iter().collect::<Vec<_>>().join(", ")
                ),
                None,
                Some(path),
                Some("writer_writes_artifact"),
            );
        }
    }
    for ((writer, artifact, path), count) in declarations_by_writer_target {
        if count > 1 {
            builder.diagnostic(
                "writer_declaration_duplicate",
                "error",
                format!("writer {writer} declares {artifact} at {path} {count} times"),
                None,
                Some(path),
                Some("writer_writes_artifact"),
            );
        }
    }

    let mut seen_groups = BTreeSet::new();
    for ((target_kind, target), group) in target_groups {
        let fingerprint = group
            .iter()
            .map(|writer| {
                (
                    writer.writer_id.as_str(),
                    writer.declaration.runtime.as_str(),
                    writer.declaration.exclusivity_group.as_str(),
                )
            })
            .collect::<BTreeSet<_>>();
        if fingerprint.len() < 2 || !seen_groups.insert(fingerprint) {
            continue;
        }
        let Some((left, right)) = first_coactivatable_pair(&group) else {
            continue;
        };
        builder.diagnostic(
            "writer_conflict",
            "error",
            format!(
                "authoritative writers {} and {} can both target {target_kind} {target}",
                left.writer_id, right.writer_id
            ),
            None,
            (target_kind == "path")
                .then_some(target)
                .or(Some(&left.declaration.path)),
            Some("writer_writes_artifact"),
        );
    }
}

fn writers_can_coactivate(left: &WriterFact, right: &WriterFact) -> bool {
    if left.writer_id == right.writer_id {
        return false;
    }
    if left.declaration.exclusivity_group == right.declaration.exclusivity_group {
        return false;
    }
    let runtimes = [
        left.declaration.runtime.as_str(),
        right.declaration.runtime.as_str(),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if runtimes == ["claude", "codex"].into_iter().collect() {
        return false;
    }
    matches!(
        (left.activation, right.activation),
        (
            WorkflowActivation::Always
                | WorkflowActivation::Conditional
                | WorkflowActivation::Manual,
            WorkflowActivation::Always
                | WorkflowActivation::Conditional
                | WorkflowActivation::Manual
        )
    )
}

fn first_coactivatable_pair<'a>(
    writers: &[&'a WriterFact],
) -> Option<(&'a WriterFact, &'a WriterFact)> {
    let mut by_runtime: BTreeMap<&str, BTreeMap<&str, &'a WriterFact>> = BTreeMap::new();
    for writer in writers {
        by_runtime
            .entry(&writer.declaration.runtime)
            .or_default()
            .entry(&writer.declaration.exclusivity_group)
            .or_insert(writer);
    }
    for groups in by_runtime.values() {
        let representatives = groups.values().copied().collect::<Vec<_>>();
        for pair in representatives.windows(2) {
            if writers_can_coactivate(pair[0], pair[1]) {
                return Some((pair[0], pair[1]));
            }
        }
    }
    let runtimes = by_runtime.iter().collect::<Vec<_>>();
    for (index, (left_runtime, left_groups)) in runtimes.iter().enumerate() {
        for (right_runtime, right_groups) in runtimes.iter().skip(index + 1) {
            if [**left_runtime, **right_runtime]
                .into_iter()
                .collect::<BTreeSet<_>>()
                == ["claude", "codex"].into_iter().collect()
            {
                continue;
            }
            for left in left_groups.values().take(2) {
                for right in right_groups.values().take(2) {
                    if writers_can_coactivate(left, right) {
                        return Some((left, right));
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn source_fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("agents/subagents")).unwrap();
        fs::create_dir_all(root.path().join("skills")).unwrap();
        root
    }

    fn bounded_agent(name: &str, tools: &str, server: &str, body: &str) -> String {
        let server = if server.is_empty() {
            String::new()
        } else {
            format!("mcpServers: [{server}]\n")
        };
        format!(
            "---\nname: {name}\ndescription: fixture\ntools: {tools}\nmodel: haiku\n{server}maxTurns: 4\neffort: low\nworkflow:\n  schema_version: 1\n  activation: conditional\n  mutability: read-only\n---\n{body}"
        )
    }

    fn write_agent(root: &Path, name: &str, text: &str) {
        fs::write(
            root.join("agents/subagents").join(format!("{name}.md")),
            text,
        )
        .unwrap();
    }

    fn installed_manifest(install: &Path, profile: &str, agents: &[&str], skills: &[&str]) {
        let canonical_root = install.canonicalize().unwrap();
        let mut budget = DigestBudget {
            files: 0,
            bytes: 0,
            directories: DirectoryBudget::default(),
        };
        let mut text_cache = BTreeMap::new();
        let mut digests = serde_json::Map::new();
        for name in agents {
            let key = format!("agents/{name}");
            let digest = digest_artifact(
                &install.join(&key),
                &canonical_root,
                &mut budget,
                &mut text_cache,
            )
            .unwrap();
            digests.insert(key, serde_json::Value::String(digest));
        }
        for name in skills {
            let key = format!("skills/{name}");
            let digest = digest_artifact(
                &install.join(&key),
                &canonical_root,
                &mut budget,
                &mut text_cache,
            )
            .unwrap();
            digests.insert(key, serde_json::Value::String(digest));
        }
        let manifest = serde_json::json!({
            "schema_version": 2,
            "package": "@xcraftmind/mastermind",
            "version": "9.9.9",
            "client": "claude",
            "profile": profile,
            "artifacts": { "agents": agents, "skills": skills },
            "digests": digests,
        });
        fs::write(
            install.join(".mastermind-workflow.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn workflow_audit_missing_mcp_server_scope() {
        let root = source_fixture();
        write_agent(
            root.path(),
            "mastermind-broken",
            &bounded_agent(
                "mastermind-broken",
                "Read, mcp__mmcg__mmcg_search",
                "",
                "Use mmcg_search.",
            ),
        );

        let report = audit_workflow(root.path());
        assert!(report.complete);
        assert!(report.has_errors());
        assert!(report.render_text().contains("collection complete: true"));
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "mmcg_server_scope_missing"
                && diagnostic.component_id.as_deref() == Some("mastermind-broken")
        }));
    }

    #[test]
    fn workflow_audit_profiles_are_optional_skill_aware() {
        for profile in ["core", "frontend", "security", "full"] {
            let home = tempfile::tempdir().unwrap();
            let install = home.path().join(".claude");
            fs::create_dir_all(install.join("agents")).unwrap();
            fs::create_dir_all(install.join("skills")).unwrap();
            let agent = bounded_agent("mastermind-role", "Read", "", "fixture").replace(
                "  mutability: read-only\n",
                "  mutability: read-only\n  skills:\n    - id: mastermind-optional\n      required: false\n",
            );
            fs::write(install.join("agents/mastermind-role.md"), agent).unwrap();
            installed_manifest(&install, profile, &["mastermind-role.md"], &[]);

            let report = audit_workflow(&install);
            assert!(report.complete, "{profile}: {:#?}", report.diagnostics);
            assert!(
                !report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.severity == "error"),
                "{profile}: {:#?}",
                report.diagnostics
            );
            assert!(report.context_estimates.iter().any(|estimate| {
                estimate.scenario == "advisory_skill_if_loaded"
                    && estimate.unavailable == ["skill:mastermind-optional"]
            }));
        }
    }

    #[test]
    fn workflow_audit_rejects_unsafe_yaml_and_files() {
        let root = source_fixture();
        let duplicate = "---\nname: mastermind-duplicate\ndescription: fixture\ntools: Read\nmodel: haiku\nmodel: opus\nmaxTurns: 4\neffort: low\nworkflow:\n  schema_version: 1\n  activation: conditional\n  mutability: read-only\n---\nfixture";
        write_agent(root.path(), "mastermind-duplicate", duplicate);
        write_agent(
            root.path(),
            "mastermind-required",
            &bounded_agent("mastermind-required", "Read", "", "fixture").replace(
                "  mutability: read-only\n",
                "  mutability: read-only\n  skills:\n    - id: mastermind-example\n",
            ),
        );
        write_agent(
            root.path(),
            "mastermind-schema-bool",
            &bounded_agent("mastermind-schema-bool", "Read", "", "fixture")
                .replace("  schema_version: 1\n", "  schema_version: true\n"),
        );
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;
            use std::os::unix::fs::symlink;
            let target = root.path().join("target.md");
            fs::write(&target, "target").unwrap();
            symlink(
                &target,
                root.path().join("agents/subagents/mastermind-link.md"),
            )
            .unwrap();
            let fifo = root.path().join("agents/subagents/mastermind-fifo.md");
            let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
            let skill_dir = root.path().join("skills/mastermind-fifo");
            fs::create_dir_all(&skill_dir).unwrap();
            let skill_fifo = skill_dir.join("SKILL.md");
            let skill_fifo_path = CString::new(skill_fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(skill_fifo_path.as_ptr(), 0o600) }, 0);
        }

        let report = audit_workflow(root.path());
        assert!(!report.complete);
        assert!(report.has_errors());
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "unsafe_yaml"));
        assert!(
            report
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "workflow_metadata_invalid")
                .count()
                >= 2
        );
        #[cfg(unix)]
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "file_type_invalid"));
        for unsafe_yaml in [
            "value: &anchor text",
            "value: *anchor",
            "value: !custom text",
            "value:\n  <<: other",
            "value: 1\nvalue: 2",
            "value: 1\n---\nother: 2",
        ] {
            assert!(validate_yaml_safety(unsafe_yaml).is_err(), "{unsafe_yaml}");
        }
        assert!(!path_is_safe_relative("../escape"));
        assert!(!path_is_safe_relative("/absolute"));
        assert!(!path_is_safe_relative("nested//alias"));
        assert!(manifest_string_array(
            Some(&serde_json::json!(["../escape"])),
            MAX_WORKFLOW_AGENTS
        )
        .is_err());
    }

    // APFS rejects invalid UTF-8 names before inventory can observe them.
    #[cfg(target_os = "linux")]
    #[test]
    fn workflow_audit_rejects_symlinked_source_inventory_parent() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().join("project");
        let inventory = workspace.path().join("inventory");
        fs::create_dir_all(inventory.join("subagents")).unwrap();
        fs::create_dir_all(root.join("skills")).unwrap();
        symlink(&inventory, root.join("agents")).unwrap();

        let report = audit_workflow(&root);
        assert!(!report.complete);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "file_type_invalid" && diagnostic.path.as_deref() == Some("agents")
        }));
    }

    #[test]
    fn workflow_audit_enforces_file_and_total_text_limits() {
        let oversized = source_fixture();
        fs::write(
            oversized
                .path()
                .join("agents/subagents/mastermind-oversized.md"),
            vec![b'a'; MAX_WORKFLOW_MARKDOWN_BYTES + 1],
        )
        .unwrap();
        let oversized_report = audit_workflow(oversized.path());
        assert!(!oversized_report.complete);
        assert!(oversized_report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "file_limit_exceeded"));

        let aggregate = source_fixture();
        let body = "a".repeat(MAX_WORKFLOW_MARKDOWN_BYTES - 512);
        for index in 0..33 {
            let name = format!("mastermind-large-{index:02}");
            write_agent(
                aggregate.path(),
                &name,
                &bounded_agent(&name, "Read", "", &body),
            );
        }
        let aggregate_report = audit_workflow(aggregate.path());
        assert!(!aggregate_report.complete);
        assert!(aggregate_report
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "workflow_total_text_limit_exceeded" }));

        let skills = source_fixture();
        for index in 0..=MAX_WORKFLOW_SKILLS {
            let name = format!("mastermind-skill-{index:03}");
            let directory = skills.path().join("skills").join(&name);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: fixture\n---\nfixture"),
            )
            .unwrap();
        }
        let skills_report = audit_workflow(skills.path());
        assert!(!skills_report.complete);
        assert!(skills_report
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "workflow_inventory_limit_exceeded" }));

        let duplicate_skills = source_fixture();
        for parent in ["one", "two"] {
            let directory = duplicate_skills
                .path()
                .join("skills")
                .join(parent)
                .join("mastermind-shared");
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("SKILL.md"),
                "---\nname: mastermind-shared\ndescription: fixture\n---\nfixture",
            )
            .unwrap();
        }
        let duplicate_report = audit_workflow(duplicate_skills.path());
        assert!(!duplicate_report.complete);
        assert!(duplicate_report
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "component_identity_duplicate" }));

        let installed = tempfile::tempdir().unwrap();
        let artifact = installed.path().join("artifact");
        let mut nested = artifact.clone();
        for index in 0..=MAX_WORKFLOW_YAML_DEPTH {
            nested = nested.join(format!("level-{index}"));
        }
        fs::create_dir_all(&nested).unwrap();
        let mut budget = DigestBudget {
            files: 0,
            bytes: 0,
            directories: DirectoryBudget::default(),
        };
        let depth_failure = digest_artifact(
            &artifact,
            &installed.path().canonicalize().unwrap(),
            &mut budget,
            &mut BTreeMap::new(),
        )
        .unwrap_err();
        assert_eq!(depth_failure.code, "workflow_inventory_limit_exceeded");

        let bounded_directory = tempfile::tempdir().unwrap();
        fs::write(bounded_directory.path().join("entry"), "fixture").unwrap();
        let mut directory_budget = DirectoryBudget {
            entries: MAX_WORKFLOW_DIRECTORY_ENTRIES,
            directories: 0,
        };
        let entry_failure = read_bounded_directory(
            bounded_directory.path(),
            &bounded_directory.path().canonicalize().unwrap(),
            &mut directory_budget,
        )
        .unwrap_err();
        assert_eq!(entry_failure.code, "workflow_inventory_limit_exceeded");
    }

    #[cfg(unix)]
    #[test]
    fn source_workflow_inventory_rejects_backslash_path_aliases() {
        let root = source_fixture();
        fs::write(
            root.path()
                .join("agents/subagents")
                .join("mastermind\\alias.md"),
            "ignored",
        )
        .unwrap();
        let skill = root.path().join("skills").join("mastermind\\alias");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "ignored").unwrap();

        let report = audit_workflow(root.path());

        assert!(!report.complete);
        let invalid = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "workflow_inventory_path_invalid")
            .collect::<Vec<_>>();
        assert_eq!(invalid.len(), 2);
        assert!(invalid.iter().all(|diagnostic| {
            diagnostic
                .path
                .as_deref()
                .is_some_and(|path| !path.contains('\\'))
        }));
    }

    // APFS rejects invalid UTF-8 names before the inventory can observe them.
    #[cfg(target_os = "linux")]
    #[test]
    fn source_workflow_inventory_does_not_publish_lossy_paths() {
        use std::os::unix::ffi::OsStringExt;

        let root = source_fixture();
        let agent = std::ffi::OsString::from_vec(b"mastermind-\xff.md".to_vec());
        fs::write(root.path().join("agents/subagents").join(agent), "ignored").unwrap();
        let skill = root
            .path()
            .join("skills")
            .join(std::ffi::OsString::from_vec(b"mastermind-\xfe".to_vec()));
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "ignored").unwrap();

        let report = audit_workflow(root.path());

        assert!(!report.complete);
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "workflow_inventory_path_invalid")
                .count(),
            2
        );
        assert!(!serde_json::to_string(&report).unwrap().contains('\u{fffd}'));
    }

    #[cfg(unix)]
    #[test]
    fn installed_artifact_digest_rejects_backslash_path_aliases() {
        let installed = tempfile::tempdir().unwrap();
        let artifact = installed.path().join("artifact");
        fs::create_dir_all(artifact.join("skill")).unwrap();
        fs::write(artifact.join("skill/file.md"), "canonical").unwrap();
        fs::write(artifact.join("skill\\file.md"), "alias").unwrap();
        let mut budget = DigestBudget {
            files: 0,
            bytes: 0,
            directories: DirectoryBudget::default(),
        };

        let failure = digest_artifact(
            &artifact,
            &installed.path().canonicalize().unwrap(),
            &mut budget,
            &mut BTreeMap::new(),
        )
        .unwrap_err();

        assert_eq!(failure.code, "manifest_invalid");
    }

    #[test]
    fn workflow_audit_writer_conflicts_require_coactivation() {
        let root = source_fixture();
        let writer = |name: &str, group: &str, runtime: &str, artifact: &str, path: &str| {
            format!(
                "---\nname: {name}\ndescription: fixture\ntools: Read, Write\nmodel: sonnet\nmaxTurns: 5\neffort: medium\nworkflow:\n  schema_version: 1\n  activation: conditional\n  mutability: writer\n  writes:\n    - artifact: {artifact}\n      path: \"{path}\"\n      authority: canonical\n      runtime: {runtime}\n      exclusivity_group: {group}\n---\nfixture"
            )
        };
        let shared_path = ".mastermind/tasks/{task}/shared.md";
        write_agent(
            root.path(),
            "mastermind-left",
            &writer(
                "mastermind-left",
                "left",
                "claude",
                "task.shared",
                shared_path,
            ),
        );
        write_agent(
            root.path(),
            "mastermind-right",
            &writer(
                "mastermind-right",
                "right",
                "claude",
                "task.shared",
                shared_path,
            ),
        );
        let conflict = audit_workflow(root.path());
        assert!(conflict
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "writer_conflict"));

        fs::write(
            root.path().join("agents/subagents/mastermind-right.md"),
            writer(
                "mastermind-right",
                "left",
                "claude",
                "task.shared",
                shared_path,
            ),
        )
        .unwrap();
        let exclusive = audit_workflow(root.path());
        assert!(!exclusive
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "writer_conflict"));

        fs::write(
            root.path().join("agents/subagents/mastermind-right.md"),
            writer(
                "mastermind-right",
                "right",
                "codex",
                "task.shared",
                shared_path,
            ),
        )
        .unwrap();
        let alternate_client = audit_workflow(root.path());
        assert!(!alternate_client
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "writer_conflict"));

        fs::write(
            root.path().join("agents/subagents/mastermind-right.md"),
            writer(
                "mastermind-right",
                "right",
                "claude",
                "task.alias",
                shared_path,
            ),
        )
        .unwrap();
        let path_alias = audit_workflow(root.path());
        assert!(path_alias
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "artifact_definition_conflict"));
        assert!(path_alias
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "writer_conflict"));

        fs::write(
            root.path().join("agents/subagents/mastermind-right.md"),
            writer(
                "mastermind-right",
                "right",
                "claude",
                "task.shared",
                ".mastermind/tasks/{task}/other.md",
            ),
        )
        .unwrap();
        let id_alias = audit_workflow(root.path());
        assert!(id_alias
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "artifact_definition_conflict"));

        fs::write(
            root.path().join("agents/subagents/mastermind-right.md"),
            writer(
                "mastermind-right",
                "right",
                "claude",
                "task.shared",
                ".mastermind/tasks/{task}/advisory.md",
            )
            .replace("authority: canonical", "authority: advisory"),
        )
        .unwrap();
        let advisory_alias = audit_workflow(root.path());
        assert!(advisory_alias
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "artifact_definition_conflict"));
        assert!(!advisory_alias
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "writer_conflict"));
    }

    #[test]
    fn workflow_audit_bounds_declarations_and_output() {
        let declarations = source_fixture();
        let writes = (0..=MAX_WORKFLOW_WRITES_PER_COMPONENT)
            .map(|index| {
                format!(
                    "    - artifact: task.output-{index}\n      path: \".mastermind/tasks/{{task}}/output-{index}.md\"\n      authority: canonical\n      runtime: claude\n      exclusivity_group: output-{index}\n"
                )
            })
            .collect::<String>();
        write_agent(
            declarations.path(),
            "mastermind-many-writes",
            &format!(
                "---\nname: mastermind-many-writes\ndescription: fixture\ntools: Read, Write\nmodel: sonnet\nmaxTurns: 5\neffort: medium\nworkflow:\n  schema_version: 1\n  activation: conditional\n  mutability: writer\n  writes:\n{writes}---\nfixture"
            ),
        );
        let declaration_report = audit_workflow(declarations.path());
        assert!(!declaration_report.complete);
        assert!(declaration_report
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "workflow_declaration_limit_exceeded" }));

        let writers = source_fixture();
        for agent_index in 0..8 {
            let name = format!("mastermind-writer-{agent_index}");
            let writes = (0..MAX_WORKFLOW_WRITES_PER_COMPONENT)
                .map(|write_index| {
                    format!(
                        "    - artifact: task.output-{agent_index}-{write_index}\n      path: \".mastermind/tasks/{{task}}/output-{agent_index}-{write_index}.md\"\n      authority: canonical\n      runtime: claude\n      exclusivity_group: writer-{agent_index}-{write_index}\n"
                    )
                })
                .collect::<String>();
            write_agent(
                writers.path(),
                &name,
                &format!(
                    "---\nname: {name}\ndescription: fixture\ntools: Read, Write\nmodel: sonnet\nmaxTurns: 5\neffort: medium\nworkflow:\n  schema_version: 1\n  activation: conditional\n  mutability: writer\n  writes:\n{writes}---\nfixture"
                ),
            );
        }
        let writer_report = audit_workflow(writers.path());
        assert!(!writer_report.complete);
        assert!(writer_report
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "workflow_declaration_limit_exceeded" }));

        let mut diagnostic_builder = WorkflowAuditBuilder::new(declarations.path());
        for index in 0..MAX_WORKFLOW_DIAGNOSTICS + 100 {
            diagnostic_builder.diagnostic(
                "fixture",
                "warning",
                format!("fixture {index}"),
                None,
                None,
                None,
            );
        }
        let diagnostic_report = diagnostic_builder.finish();
        assert_eq!(
            diagnostic_report.diagnostics.len(),
            MAX_WORKFLOW_DIAGNOSTICS
        );
        assert!(!diagnostic_report.complete);
        assert!(diagnostic_report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "diagnostic_limit_exceeded"));

        let mut context_builder = WorkflowAuditBuilder::new(declarations.path());
        for index in 0..MAX_WORKFLOW_CONTEXT_ESTIMATES + 1 {
            context_builder.add_context_estimate(WorkflowContextEstimate {
                component_id: format!("fixture:{index}"),
                scenario: "fixture".into(),
                bytes: None,
                estimated_tokens: None,
                components: Vec::new(),
                unavailable: Vec::new(),
            });
        }
        let context_report = context_builder.finish();
        assert_eq!(
            context_report.context_estimates.len(),
            MAX_WORKFLOW_CONTEXT_ESTIMATES
        );
        assert!(!context_report.complete);
        assert!(context_report
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "context_estimate_limit_exceeded" }));

        let mut graph_builder = WorkflowAuditBuilder::new(declarations.path());
        for index in 0..MAX_WORKFLOW_NODES + 100 {
            graph_builder.add_node(
                format!("fixture:{index}"),
                "fixture",
                index.to_string(),
                None,
            );
        }
        let graph_report = graph_builder.finish();
        assert_eq!(graph_report.nodes.len(), MAX_WORKFLOW_NODES);
        assert!(!graph_report.complete);
        assert_eq!(
            graph_report
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "graph_limit_exceeded")
                .count(),
            1
        );
    }

    #[test]
    fn workflow_audit_graphs_builtin_and_mutation_capabilities() {
        let root = source_fixture();
        let mut agent = bounded_agent("mastermind-writer", "Read, Write", "", "fixture");
        agent = agent.replace("  mutability: read-only\n", "  mutability: writer\n");
        write_agent(root.path(), "mastermind-writer", &agent);
        let report = audit_workflow(root.path());
        assert!(report.edges.iter().any(|edge| {
            edge.from == "agent:mastermind-writer"
                && edge.to == "tool:Read"
                && edge.kind == "grants_tool"
        }));
        assert!(report.edges.iter().any(|edge| {
            edge.from == "agent:mastermind-writer"
                && edge.to == "tool:Write"
                && edge.kind == "grants_tool"
        }));
        assert!(report.edges.iter().any(|edge| {
            edge.from == "agent:mastermind-writer"
                && edge.to == "capability:filesystem-mutation"
                && edge.kind == "grants_capability"
        }));
    }

    #[test]
    fn workflow_audit_uses_real_claude_registration_scopes() {
        let workspace = tempfile::tempdir().unwrap();
        let home = workspace.path().join("home");
        let project = workspace.path().join("project");
        let install = project.join(".claude");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(install.join("agents")).unwrap();
        fs::create_dir_all(install.join("skills")).unwrap();
        let candidates = registration_config_candidates(&install, Some(&home), None);
        assert_eq!(
            candidates,
            vec![home.join(".claude.json"), project.join(".mcp.json")]
        );
        assert!(!candidates.contains(&install.join(".mcp.json")));

        let user_install = home.join(".claude");
        fs::create_dir_all(&user_install).unwrap();
        assert_eq!(
            registration_config_candidates(&user_install, Some(&home), None),
            vec![home.join(".claude.json")]
        );
        let config_dir = workspace.path().join("claude-config");
        fs::create_dir_all(&config_dir).unwrap();
        assert_eq!(
            registration_config_candidates(&config_dir, Some(&home), Some(&config_dir)),
            vec![config_dir.join(".claude.json")]
        );
        fs::write(config_dir.join(".config.json"), "{}").unwrap();
        assert_eq!(
            registration_config_candidates(&install, Some(&home), Some(&config_dir)),
            vec![config_dir.join(".config.json"), project.join(".mcp.json")]
        );
        let non_windows_global = serde_json::json!({
            "command": "mastermind",
            "args": ["serve"],
        });
        let non_windows_npx = serde_json::json!({
            "command": "npx",
            "args": ["-y", "@xcraftmind/mastermind@2.0.1", "serve"],
            "type": "stdio",
            "env": {},
        });
        assert_eq!(
            valid_mmcg_registration_entry(&non_windows_global),
            !cfg!(windows)
        );
        assert_eq!(
            valid_mmcg_registration_entry(&non_windows_npx),
            !cfg!(windows)
        );
        let windows_project = serde_json::json!({
            "command": "cmd.exe",
            "args": ["/d", "/s", "/c", ".\\node_modules\\.bin\\mastermind.cmd", "serve"],
        });
        let windows_npx = serde_json::json!({
            "command": "cmd.exe",
            "args": ["/d", "/s", "/c", "npx.cmd", "-y", "@xcraftmind/mastermind@2.0.1", "serve"],
        });
        assert_eq!(
            valid_mmcg_registration_entry(&windows_project),
            cfg!(windows)
        );
        assert_eq!(valid_mmcg_registration_entry(&windows_npx), cfg!(windows));
        assert!(!valid_mmcg_registration_entry(&serde_json::json!({})));
        assert!(!valid_mmcg_registration_entry(&serde_json::json!({
            "command": "mastermind",
            "args": ["doctor"],
        })));
        assert!(!valid_mmcg_registration_entry(&serde_json::json!({
            "command": "evil",
            "args": ["serve"],
        })));
        assert!(!valid_mmcg_registration_entry(&serde_json::json!({
            "command": "npx",
            "args": ["-y", "attacker-package", "serve"],
        })));
        assert!(!valid_mmcg_registration_entry(&serde_json::json!({
            "command": "mastermind",
            "args": ["serve"],
            "transport": "stdio",
        })));
        let profile_env_entry = serde_json::json!({
            "command": "mastermind",
            "args": ["serve"],
            "env": {"MMCG_PROFILE_CLIENT": "claude"},
        });
        assert_eq!(
            valid_mmcg_registration_entry(&profile_env_entry),
            !cfg!(windows)
        );
        for environment in [
            serde_json::json!({"PATH": "/attacker"}),
            serde_json::json!({"NODE_OPTIONS": "--require=/attacker.js"}),
            serde_json::json!({"npm_config_registry": "https://attacker.invalid"}),
            serde_json::json!({"MMCG_PROFILE_CLIENT": "claude", "PATH": "/attacker"}),
            serde_json::json!({"MMCG_PROFILE_CLIENT": 1}),
            serde_json::json!({"MMCG_PROFILE_CLIENT": ""}),
            serde_json::json!({"MMCG_PROFILE_CLIENT": "a b"}),
        ] {
            assert!(!valid_mmcg_registration_entry(&serde_json::json!({
                "command": "mastermind",
                "args": ["serve"],
                "env": environment,
            })));
        }
        if !cfg!(windows) {
            assert!(!valid_mmcg_registration_entry(&serde_json::json!({
                "command": "mastermind.exe",
                "args": ["serve"],
            })));
        }

        fs::write(
            install.join("agents/mastermind-role.md"),
            bounded_agent(
                "mastermind-role",
                "Read, mcp__mmcg__mmcg_status",
                "mmcg",
                "Use mmcg_status.",
            ),
        )
        .unwrap();
        installed_manifest(&install, "core", &["mastermind-role.md"], &[]);
        let registered_entry = if cfg!(windows) {
            serde_json::json!({
                "command": "cmd.exe",
                "args": ["/d", "/s", "/c", ".\\node_modules\\.bin\\mastermind.cmd", "serve"],
                "env": {"MMCG_PROFILE_CLIENT": "claude"},
            })
        } else {
            profile_env_entry
        };
        fs::write(
            project.join(".mcp.json"),
            serde_json::to_vec(&serde_json::json!({
                "mcpServers": {
                    "mmcg": registered_entry
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let report = audit_workflow(&install);
        assert!(!report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "mmcg_registration_missing"));
        let doctor_report = audit_workflow_for_doctor(&project).unwrap();
        assert!(!doctor_report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "mmcg_registration_missing"));
    }

    #[test]
    fn workflow_audit_context_estimates_are_componentized() {
        let root = source_fixture();
        write_agent(
            root.path(),
            "mastermind-context",
            &bounded_agent(
                "mastermind-context",
                "Read, mcp__mmcg__mmcg_status",
                "mmcg",
                "é",
            ),
        );
        let report = audit_workflow(root.path());
        let body = report
            .context_estimates
            .iter()
            .find(|estimate| {
                estimate.component_id == "agent:mastermind-context"
                    && estimate.scenario == "agent_body"
            })
            .unwrap();
        assert_eq!((body.bytes, body.estimated_tokens), (Some(2), Some(1)));
        let schemas = report
            .context_estimates
            .iter()
            .find(|estimate| estimate.scenario == "known_mmcg_schema")
            .unwrap();
        assert!(schemas.bytes.is_some_and(|bytes| bytes > 0));
        assert!(report.context_estimates.iter().any(|estimate| {
            estimate.scenario == "built_in_schema_unknown" && estimate.unavailable == ["Read"]
        }));
    }

    #[test]
    fn workflow_audit_schema_v1_golden_and_renderer_parity() {
        fn object_keys(value: &serde_json::Value) -> BTreeSet<&str> {
            value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect()
        }

        let root = source_fixture();
        write_agent(
            root.path(),
            "mastermind-stable",
            &bounded_agent("mastermind-stable", "Read", "", "fixture"),
        );
        let report = audit_workflow(root.path());
        let json = serde_json::to_value(&report).unwrap();
        assert!(!report.has_errors());
        assert_eq!(json["schema_version"], 1);
        assert_eq!(
            object_keys(&json),
            [
                "complete",
                "context_estimates",
                "diagnostics",
                "edges",
                "layout",
                "limits",
                "nodes",
                "root",
                "schema_version",
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(
            object_keys(&json["limits"]),
            [
                "agents",
                "context_estimates",
                "diagnostics",
                "directories",
                "directory_entries",
                "edges",
                "manifest_bytes",
                "markdown_bytes",
                "nodes",
                "relations_per_component",
                "servers_per_component",
                "skills",
                "tool_grants_per_component",
                "total_text_bytes",
                "writers",
                "writes_per_component",
                "yaml_depth",
            ]
            .into_iter()
            .collect()
        );
        assert!(json["nodes"].as_array().unwrap().iter().all(|node| {
            let keys = object_keys(node);
            keys == ["id", "kind", "label"].into_iter().collect()
                || keys == ["id", "kind", "label", "path"].into_iter().collect()
        }));
        assert!(json["edges"].as_array().unwrap().iter().all(|edge| {
            object_keys(edge) == ["from", "kind", "precision", "to"].into_iter().collect()
        }));
        assert!(json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .all(|diagnostic| {
                let keys = object_keys(diagnostic);
                ["code", "severity", "message"]
                    .into_iter()
                    .all(|key| keys.contains(key))
                    && keys.iter().all(|key| {
                        [
                            "code",
                            "component_id",
                            "evidence_relation",
                            "message",
                            "path",
                            "severity",
                        ]
                        .contains(key)
                    })
            }));
        assert!(json["context_estimates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|estimate| {
                let keys = object_keys(estimate);
                ["component_id", "scenario"]
                    .into_iter()
                    .all(|key| keys.contains(key))
                    && keys.iter().all(|key| {
                        [
                            "bytes",
                            "component_id",
                            "components",
                            "estimated_tokens",
                            "scenario",
                            "unavailable",
                        ]
                        .contains(key)
                    })
            }));
        assert!(report
            .nodes
            .windows(2)
            .all(|nodes| nodes[0].id <= nodes[1].id));
        assert!(report.edges.windows(2).all(|edges| {
            (
                &edges[0].from,
                &edges[0].to,
                &edges[0].kind,
                &edges[0].precision,
            ) <= (
                &edges[1].from,
                &edges[1].to,
                &edges[1].kind,
                &edges[1].precision,
            )
        }));
        assert_eq!(report, audit_workflow(root.path()));
        let text = report.render_text();
        assert!(text.contains(&format!(
            "{} nodes, {} edges",
            report.nodes.len(),
            report.edges.len()
        )));
        assert!(text.contains("schema:   v1"));
        assert!(text.contains("B manifest"));
        let node = report.nodes.first().expect("fixture should produce nodes");
        assert!(text.contains(&format!("{} [{}]", node.id, node.kind)));
        let edge = report.edges.first().expect("fixture should produce edges");
        assert!(text.contains(&format!(
            "{} -> {} [{}; precision={}]",
            edge.from, edge.to, edge.kind, edge.precision
        )));
        let estimate = report
            .context_estimates
            .first()
            .expect("fixture should produce context estimates");
        assert!(text.contains(&format!(
            "{} [{}]",
            estimate.component_id, estimate.scenario
        )));
        assert_eq!(
            escape_terminal("x\u{1b}]8;;bad\u{7}\u{202e}"),
            "x\\u{1b}]8;;bad\\u{7}\\u{202e}"
        );
    }
}
