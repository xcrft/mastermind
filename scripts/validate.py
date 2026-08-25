#!/usr/bin/env python3
"""Validate repository artifact, workflow, packaging, and CI contracts.

The detailed check inventory and boundaries live in scripts/README.md. Exit
code 0 means clean; errors return 1, while warnings remain non-blocking.
"""

from __future__ import annotations

import json
import os
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

try:
    import yaml
except ImportError:
    print(
        "error: PyYAML not installed. Run: pip install --require-hashes -r scripts/requirements.txt",
        file=sys.stderr,
    )
    sys.exit(2)


REPO_ROOT = Path(__file__).resolve().parent.parent

# Canonical top-level categories for portable skills.
ALLOWED_DOMAINS = {
    "code-review",
    "coding",
    "testing",
    "design",
    "debugging",
    "docs",
    "refactoring",
    "ops",
    "security",
    "workflow",
    "prompt-engineering",
}

SLUG_RE = re.compile(r"^[a-z][a-z0-9-]*$")
SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?$")
WIKILINK_RE = re.compile(r"\[\[([a-z][a-z0-9-]*)\]\]")
# Matches `[text](path)` and `![alt](path)`. Captures the path part only.
# Will be filtered post-match: skip if inside a fenced code block, skip if path
# is external/anchor-only.
RELATIVE_LINK_RE = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")
FRONTMATTER_RE = re.compile(r"^---\n(.*?)\n---\n", re.DOTALL)
MMCG_REFERENCE_RE = re.compile(r"\bmmcg_[a-z_]+\b")
SUBAGENT_MODELS = {"haiku", "sonnet", "opus"}
SUBAGENT_EFFORT_LEVELS = {"low", "medium", "high", "xhigh", "max"}
SUBAGENT_MAX_TURNS = 100
WORKFLOW_ACTIVATIONS = {"always", "conditional", "manual"}
WORKFLOW_MUTABILITIES = {"read-only", "writer"}
WORKFLOW_RUNTIMES = {"claude", "codex", "portable", "controller"}
WORKFLOW_MAX_RELATIONS = 512
WORKFLOW_MAX_WRITES = 64
WORKFLOW_MAX_TOOL_GRANTS = 512
WORKFLOW_MAX_SERVERS = 64
RUNTIME_TOOL_RE = re.compile(r"^[A-Za-z0-9_.:-]{1,256}$")
MCP_SERVER_RE = re.compile(r"^[a-z][a-z0-9_-]*$")


@dataclass
class Issue:
    path: Path
    level: str  # "error" | "warning"
    msg: str

    def render(self) -> str:
        rel = self.path.relative_to(REPO_ROOT)
        return f"{rel}: {self.level}: {self.msg}"


@dataclass
class Artifact:
    path: Path
    slug: str
    domain: str | None
    frontmatter: dict
    name: str | None  # may be None on malformed frontmatter


# ----- discovery ---------------------------------------------------------


def _has_template_part(p: Path) -> bool:
    return any(part == "_template" for part in p.parts)


def find_artifacts() -> Iterator[Artifact]:
    """Yield every artifact in the repo. Skips templates and category index READMEs."""
    skills = REPO_ROOT / "skills"
    agents = REPO_ROOT / "agents"
    mcp = REPO_ROOT / "mcp"

    # skills/<domain>/<slug>/SKILL.md (folder-style)
    for p in skills.glob("*/*/SKILL.md"):
        if _has_template_part(p):
            continue
        a = _load_artifact(p, slug=p.parts[-2], domain=p.parts[-3])
        if a:
            yield a

    # skills/<domain>/<slug>.md (single-file)
    for p in skills.glob("*/*.md"):
        if _has_template_part(p) or p.name == "README.md":
            continue
        a = _load_artifact(p, slug=p.stem, domain=p.parts[-2])
        if a:
            yield a

    # agents/subagents/<slug>.md and agents/claude-md/<slug>.md
    for sub in ("subagents", "claude-md"):
        for p in (agents / sub).glob("*.md"):
            if _has_template_part(p):
                continue
            a = _load_artifact(p, slug=p.stem, domain=None)
            if a:
                yield a

    # mcp/servers/<slug>/README.md and mcp/integrations/<slug>/README.md
    for sub in ("servers", "integrations"):
        for p in (mcp / sub).glob("*/README.md"):
            if _has_template_part(p):
                continue
            a = _load_artifact(p, slug=p.parts[-2], domain=None)
            if a:
                yield a


def _load_artifact(path: Path, *, slug: str, domain: str | None) -> Artifact | None:
    """Open a candidate artifact file. Returns None if the file can't be read."""
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None

    match = FRONTMATTER_RE.match(text)
    if not match:
        return Artifact(path=path, slug=slug, domain=domain, frontmatter={}, name=None)

    try:
        fm = yaml.safe_load(match.group(1)) or {}
    except yaml.YAMLError:
        fm = {}

    if not isinstance(fm, dict):
        fm = {}

    name = fm.get("name") if isinstance(fm.get("name"), str) else None
    return Artifact(path=path, slug=slug, domain=domain, frontmatter=fm, name=name)


# ----- per-artifact validation ------------------------------------------


def validate_artifact(a: Artifact) -> list[Issue]:
    issues: list[Issue] = []

    if not a.frontmatter:
        issues.append(Issue(a.path, "error", "missing or malformed YAML frontmatter (must be wrapped in `---` lines)"))
        return issues

    if a.name is None:
        issues.append(Issue(a.path, "error", "missing 'name' field in frontmatter"))
    else:
        if not SLUG_RE.match(a.name):
            issues.append(Issue(a.path, "error", f"'name' is not kebab-case: {a.name!r}"))
        if a.name != a.slug:
            issues.append(
                Issue(a.path, "error", f"'name' ({a.name!r}) does not match file/folder slug ({a.slug!r})")
            )

    desc = a.frontmatter.get("description")
    if not isinstance(desc, str) or not desc.strip():
        issues.append(Issue(a.path, "error", "missing or empty 'description'"))
    elif len(desc) < 40:
        issues.append(
            Issue(a.path, "warning", f"'description' is very short ({len(desc)} chars); state the action and its trigger.")
        )

    metadata = a.frontmatter.get("metadata", {})
    if not isinstance(metadata, dict):
        issues.append(Issue(a.path, "error", "'metadata' is not a mapping"))
    else:
        version = metadata.get("version")
        if version is None:
            issues.append(Issue(a.path, "error", "missing 'metadata.version'"))
        elif not isinstance(version, str) or not SEMVER_RE.match(version):
            issues.append(Issue(a.path, "error", f"'metadata.version' is not SemVer: {version!r}"))

        authors = metadata.get("authors")
        if authors is not None and not isinstance(authors, list):
            issues.append(Issue(a.path, "error", "'metadata.authors' must be a list"))

        tags = metadata.get("tags")
        if tags is not None and not isinstance(tags, list):
            issues.append(Issue(a.path, "error", "'metadata.tags' must be a list"))
        elif a.domain and isinstance(tags, list) and tags and tags[0] != a.domain:
            issues.append(
                Issue(a.path, "warning", f"first tag ({tags[0]!r}) does not match domain folder ({a.domain!r}).")
            )

    if a.domain is not None and a.domain not in ALLOWED_DOMAINS:
        issues.append(
            Issue(a.path, "error", f"unknown domain {a.domain!r}. Allowed: {sorted(ALLOWED_DOMAINS)}. Update ALLOWED_DOMAINS if adding a category.")
        )

    # Subagents are loaded by the Claude Code runtime, which reads `tools`, `model`,
    # `mcpServers`, and `disallowedTools` as TOP-LEVEL frontmatter keys. Nesting them
    # under `metadata` makes Claude Code silently ignore them — the subagent then
    # inherits every tool and the parent's model. Catch the
    # regression at lint time so it can't ship again.
    if a.path.parent.name == "subagents" and isinstance(metadata, dict):
        for field in (
            "tools",
            "model",
            "mcpServers",
            "disallowedTools",
            "maxTurns",
            "effort",
        ):
            if field in metadata:
                issues.append(
                    Issue(
                        a.path,
                        "error",
                        f"subagent runtime field {field!r} is nested under 'metadata' — "
                        "Claude Code reads it only at the top level; move it up",
                    )
                )

    if a.path.parent.name == "subagents":
        for message in _subagent_runtime_contract_messages(a.frontmatter):
            issues.append(Issue(a.path, "error", message))
        for message in _workflow_metadata_messages(
            a.frontmatter,
            required=True,
            runtime_tools=_runtime_tool_names(a.frontmatter.get("tools")),
        ):
            issues.append(Issue(a.path, "error", message))
    elif "workflow" in a.frontmatter or a.slug == "mastermind-task-executor":
        for message in _workflow_metadata_messages(
            a.frontmatter,
            required=a.slug == "mastermind-task-executor",
            runtime_tools=[],
        ):
            issues.append(Issue(a.path, "error", message))

    return issues


# ----- wikilink resolution ----------------------------------------------


def collect_wikilinks() -> dict[Path, set[str]]:
    """Walk all .md files and collect [[slug]] references. Skips code fences and
    inline code spans so documentation showing the syntax doesn't trigger."""
    result: dict[Path, set[str]] = {}
    for p in REPO_ROOT.rglob("*.md"):
        if _is_excluded(p):
            continue
        try:
            text = p.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        slugs = set(WIKILINK_RE.findall(_strip_code(text)))
        if slugs:
            result[p] = slugs
    return result


def _is_excluded(path: Path) -> bool:
    parts = path.parts
    # Build artifacts and version control
    if any(part in {".git", "target", "node_modules", "__pycache__"} for part in parts):
        return True

    # Templates show example references, not real ones
    if "_template" in parts:
        return True

    # Top-level `docs/` shows `[[slug]]` syntax as illustration, not as references
    # research/ is not part of the artifact tree (stray notes from past sessions)
    try:
        rel = path.relative_to(REPO_ROOT)
    except ValueError:
        return False
    rel_parts = rel.parts
    if rel_parts and rel_parts[0] in {"docs", "research", "scripts", "examples", "evals"}:
        return True
    # CLAUDE.md templates contain intentional links that resolve when copied
    # to a project root, not in the template's own location.
    if len(rel_parts) >= 2 and rel_parts[0] == "agents" and rel_parts[1] == "claude-md":
        return True
    # mmcg's embedded template mirrors are NOT artifacts — they're build-time
    # data for `include_str!`. Their links resolve in the canonical location;
    # parity with canonical is enforced by validate_mmcg_template_mirrors().
    if (
        len(rel_parts) >= 4
        and rel_parts[0] == "mcp"
        and rel_parts[1] == "servers"
        and rel_parts[2] == "mmcg"
        and rel_parts[3] == "templates"
    ):
        return True

    return False


def validate_wikilinks(artifacts: list[Artifact], links: dict[Path, set[str]]) -> list[Issue]:
    known = {a.name for a in artifacts if a.name}
    issues = []
    for path, slugs in links.items():
        for slug in slugs:
            if slug not in known:
                issues.append(
                    Issue(path, "error", f"unresolved [[{slug}]] — no artifact has 'name: {slug}'")
                )
    return issues


# ----- relative-link resolution -----------------------------------------


def _strip_code(text: str) -> str:
    """Remove fenced code blocks AND inline code spans so link/wikilink regex
    doesn't match documentation examples that happen to show the syntax."""
    out_lines: list[str] = []
    in_fence = False
    for line in text.splitlines():
        stripped = line.lstrip()
        if stripped.startswith("```"):
            in_fence = not in_fence
            continue
        if not in_fence:
            out_lines.append(line)
    no_fences = "\n".join(out_lines)
    # Strip inline code spans (single backticks, single line)
    return re.sub(r"`[^`\n]+`", "", no_fences)


def collect_relative_links() -> dict[Path, set[str]]:
    """Walk all .md files and collect `[text](path)` links to local files.
    Skips: code-fence content, inline code spans, external URLs, anchor-only
    links, mailto. Strips `#anchor` suffix — only the file part is validated."""
    result: dict[Path, set[str]] = {}
    for p in REPO_ROOT.rglob("*.md"):
        if _is_excluded(p):
            continue
        try:
            text = p.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        text = _strip_code(text)
        targets: set[str] = set()
        for raw in RELATIVE_LINK_RE.findall(text):
            link = raw.strip()
            # Skip external, anchor-only, mailto
            if link.startswith(("http://", "https://", "mailto:", "#")):
                continue
            # Strip any #anchor — we only check the path exists
            path_only = link.split("#", 1)[0].strip()
            if not path_only:
                continue
            targets.add(path_only)
        if targets:
            result[p] = targets
    return result


def validate_relative_links(links: dict[Path, set[str]]) -> list[Issue]:
    issues: list[Issue] = []
    for source, targets in links.items():
        source_dir = source.parent
        for target in targets:
            resolved = (source_dir / target).resolve()
            if not resolved.exists():
                rel = target
                issues.append(
                    Issue(source, "error", f"broken markdown link → {rel!r} (resolved to {resolved})")
                )
    return issues


def validate_release_badges() -> list[Issue]:
    """Keep release-bound badges aligned with the packaged npm version."""
    issues: list[Issue] = []
    package_path = REPO_ROOT / "npm/mastermind/package.json"
    try:
        package = json.loads(package_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return [Issue(package_path, "error", f"cannot read npm package metadata: {error}")]
    version = package.get("version")
    if not isinstance(version, str) or not SEMVER_RE.match(version):
        return [Issue(package_path, "error", "npm package version is not valid semver")]
    badge = f"https://img.shields.io/badge/npm-v{version}-CB3837?logo=npm"
    for relative in ("README.md", "npm/mastermind/README.md"):
        path = REPO_ROOT / relative
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as error:
            issues.append(Issue(path, "error", f"cannot read README badge: {error}"))
            continue
        if badge not in text:
            issues.append(Issue(path, "error", f"npm badge must match package version {version}"))
    return issues


# These are the metadata and README surfaces that actually ship in the npm
# tarball or crates.io archive. Keep new language support visible in every
# distributed landing surface instead of only the repository README.
DISTRIBUTED_VUE_MARKERS: dict[str, str] = {
    "npm/mastermind/package.json": '"vue"',
    "npm/mastermind/README.md": "Vue SFC",
    "mcp/servers/mmcg/Cargo.toml": "Vue SFC",
    "mcp/servers/mmcg/README.md": "Vue SFC",
}


def distributed_vue_metadata_contents() -> dict[str, str]:
    contents: dict[str, str] = {}
    for relative in DISTRIBUTED_VUE_MARKERS:
        path = REPO_ROOT / relative
        try:
            contents[relative] = path.read_text(encoding="utf-8")
        except OSError:
            contents[relative] = ""
    return contents


def distributed_vue_metadata_errors(contents: dict[str, str] | None = None) -> list[str]:
    values = distributed_vue_metadata_contents() if contents is None else contents
    return [
        relative
        for relative, marker in DISTRIBUTED_VUE_MARKERS.items()
        if marker not in values.get(relative, "")
    ]


def validate_distributed_language_metadata() -> list[Issue]:
    return [
        Issue(REPO_ROOT / relative, "error", "distributed package metadata must advertise Vue SFC support")
        for relative in distributed_vue_metadata_errors()
    ]


# ----- mmcg template-mirror sync ---------------------------------------

# The mmcg crate embeds two templates at build time via `include_str!` (CONTEXT.md
# and CLAUDE.md/workflow). They live at `mcp/servers/mmcg/templates/` so
# `cargo publish` can ship them (the canonical originals are outside the crate
# root and would be unreachable from the published tarball). When the canonical
# files change, the mirror must be refreshed — otherwise `mmcg init` from
# `cargo install`ed binaries scaffolds stale templates. This check enforces parity.
#
# The spec template is no longer mirrored / embedded — `mmcg init` does not drop
# it into projects anymore (the planner skill carries its own copy and uses that
# directly, so the project-level copy was dead weight).
MMCG_TEMPLATE_MIRRORS: list[tuple[str, str]] = [
    ("agents/claude-md/mastermind-context.md", "mcp/servers/mmcg/templates/context.md"),
    ("agents/claude-md/mastermind-workflow.md", "mcp/servers/mmcg/templates/workflow.md"),
]


# ----- mmcg tool-list drift check ----------------------------------------

# The authoritative list of MCP tools lives in `mcp/servers/mmcg/src/mcp.rs`
# inside the `tools_list()` JSON. The exhaustive technical reference must
# mention every tool by name; landing-page READMEs only carry the derived count
# and link to that canonical reference. This keeps the API contract checked
# without forcing the same long inventory into every user-facing README.

MMCG_MCP_SRC = "mcp/servers/mmcg/src/mcp.rs"

# Files that must reference every mmcg_* tool name. (Each may have one
# canonical mention line; we only require *presence* of each tool name in the
# file's text. False positives possible if a name appears in unrelated prose,
# but mmcg_xxx names are sufficiently distinctive.)
MMCG_TOOL_LIST_DOCS: list[str] = [
    "docs/reference/mmcg.md",
    "docs/integrations/generic-mcp.md",
]

# Files that quote a tool *count* like "16 structural query tools" / "15 tools".
# We extract counts and assert they all match the count derived from mcp.rs.
MMCG_TOOL_COUNT_DOCS: list[str] = [
    "docs/reference/mmcg.md",
    "docs/integrations/generic-mcp.md",
    "mcp/README.md",
    "mcp/servers/mmcg/README.md",
    "npm/mastermind/README.md",
    "README.md",
]


def _extract_mmcg_tools_from_source(src: str) -> list[str]:
    start = src.find("static TOOLS:")
    end = src.find("];", start) + 2 if start != -1 else -1
    body = src[start:end] if start != -1 and end != -1 else src
    pattern = re.compile(
        r'name:\s*"(mmcg_[a-z_]+)"'
        r'|(?:read_only_tool|refreshable_tool|additive_tool)\s*\(\s*"(mmcg_[a-z_]+)"'
    )
    tools: list[str] = []
    seen: set[str] = set()
    for match in pattern.finditer(body):
        name = match.group(1) or match.group(2)
        if name not in seen:
            seen.add(name)
            tools.append(name)
    return tools


def extract_mmcg_tools() -> list[str]:
    """Pull every mmcg_xxx tool name from mcp.rs in declaration order."""
    src = (REPO_ROOT / MMCG_MCP_SRC).read_text()
    return _extract_mmcg_tools_from_source(src)


def _runtime_tool_names(value: object) -> list[str]:
    """Normalize Claude Code's scalar or YAML-list `tools` frontmatter."""
    if isinstance(value, str):
        return [entry.strip() for entry in value.split(",") if entry.strip()]
    if isinstance(value, list) and all(isinstance(entry, str) for entry in value):
        return [entry.strip() for entry in value if entry.strip()]
    return []


def _subagent_runtime_contract_messages(frontmatter: dict) -> list[str]:
    messages: list[str] = []

    model = frontmatter.get("model")
    if not isinstance(model, str) or model not in SUBAGENT_MODELS:
        messages.append(
            f"[model_unsupported] subagent 'model' must be one of {sorted(SUBAGENT_MODELS)}, got {model!r}"
        )

    tools_value = frontmatter.get("tools")
    valid_tools_shape = isinstance(tools_value, str) or (
        isinstance(tools_value, list)
        and all(isinstance(entry, str) for entry in tools_value)
    )
    runtime_tools = _runtime_tool_names(tools_value)
    if not valid_tools_shape or not runtime_tools:
        messages.append("[tool_allowlist_invalid] subagent 'tools' must be an explicit non-empty allowlist")
    elif len(runtime_tools) > WORKFLOW_MAX_TOOL_GRANTS:
        messages.append(
            f"[workflow_declaration_limit_exceeded] subagent 'tools' exceeds {WORKFLOW_MAX_TOOL_GRANTS} entries"
        )
    elif len(runtime_tools) != len(set(runtime_tools)):
        messages.append("[tool_allowlist_duplicate] subagent 'tools' allowlist contains duplicate entries")
    elif any(
        not RUNTIME_TOOL_RE.fullmatch(tool)
        and not (tool.startswith("mcp__mmcg__") and "*" in tool)
        for tool in runtime_tools
    ):
        messages.append("[tool_allowlist_invalid] subagent 'tools' contains an invalid tool name")

    max_turns = frontmatter.get("maxTurns")
    if (
        isinstance(max_turns, bool)
        or not isinstance(max_turns, int)
        or not 1 <= max_turns <= SUBAGENT_MAX_TURNS
    ):
        messages.append(
            f"[max_turns_invalid] subagent 'maxTurns' must be an integer from 1 to {SUBAGENT_MAX_TURNS}"
        )

    effort = frontmatter.get("effort")
    if not isinstance(effort, str) or effort not in SUBAGENT_EFFORT_LEVELS:
        messages.append(
            f"[effort_invalid] subagent 'effort' must be one of {sorted(SUBAGENT_EFFORT_LEVELS)}, got {effort!r}"
        )

    servers = frontmatter.get("mcpServers")
    valid_servers_shape = (
        isinstance(servers, list)
        and all(isinstance(server, str) for server in servers)
    ) or (
        isinstance(servers, dict)
        and all(isinstance(server, str) for server in servers)
    )
    if servers is not None and not valid_servers_shape:
        messages.append("[mcp_servers_invalid] subagent 'mcpServers' must be a list or mapping when present")
    elif len(_mcp_server_names(servers)) > WORKFLOW_MAX_SERVERS:
        messages.append(
            f"[workflow_declaration_limit_exceeded] subagent 'mcpServers' exceeds {WORKFLOW_MAX_SERVERS} entries"
        )
    elif any(not MCP_SERVER_RE.fullmatch(server) for server in _mcp_server_names(servers)):
        messages.append("[mcp_servers_invalid] subagent 'mcpServers' contains an invalid server name")

    return messages


def _safe_workflow_write_path(value: object) -> bool:
    if not isinstance(value, str) or value.count("{task}") > 1:
        return False
    expanded = value.replace("{task}", "task")
    if "{" in expanded or "}" in expanded or "\\" in expanded:
        return False
    if any(part in {"", ".", ".."} for part in expanded.split("/")):
        return False
    path = Path(expanded)
    return not path.is_absolute() and all(part not in {"", ".", ".."} for part in path.parts)


def _workflow_metadata_messages(
    frontmatter: dict, *, required: bool, runtime_tools: list[str]
) -> list[str]:
    workflow = frontmatter.get("workflow")
    if workflow is None:
        return ["[workflow_metadata_missing] managed component must declare workflow metadata"] if required else []
    if not isinstance(workflow, dict):
        return ["[workflow_metadata_invalid] workflow metadata must be a mapping"]

    messages: list[str] = []
    allowed = {"schema_version", "activation", "mutability", "skills", "writes"}
    unknown = sorted(set(workflow) - allowed)
    if unknown:
        messages.append(
            f"[workflow_metadata_invalid] unknown workflow field(s): {', '.join(unknown)}"
        )
    if type(workflow.get("schema_version")) is not int or workflow.get("schema_version") != 1:
        messages.append("[workflow_metadata_invalid] workflow schema_version must be 1")
    if workflow.get("activation") not in WORKFLOW_ACTIVATIONS:
        messages.append("[workflow_metadata_invalid] workflow activation is unsupported")
    mutability = workflow.get("mutability")
    if mutability not in WORKFLOW_MUTABILITIES:
        messages.append("[workflow_metadata_invalid] workflow mutability is unsupported")

    skills = workflow.get("skills", [])
    if not isinstance(skills, list):
        messages.append("[workflow_metadata_invalid] workflow skills must be a list")
    elif len(skills) > WORKFLOW_MAX_RELATIONS:
        messages.append(
            f"[workflow_declaration_limit_exceeded] workflow skills exceeds {WORKFLOW_MAX_RELATIONS} entries"
        )
    else:
        relation_ids: list[str] = []
        for relation in skills:
            if (
                not isinstance(relation, dict)
                or set(relation) != {"id", "required"}
                or not isinstance(relation.get("id"), str)
                or not SLUG_RE.fullmatch(relation["id"])
                or not isinstance(relation.get("required"), bool)
            ):
                messages.append(
                    "[workflow_metadata_invalid] each skill relation needs only canonical id and boolean required"
                )
                break
            relation_ids.append(relation["id"])
        if len(relation_ids) != len(set(relation_ids)):
            messages.append(
                "[workflow_metadata_invalid] workflow skill relation IDs must be unique"
            )

    writes = workflow.get("writes", [])
    write_fields = {"artifact", "path", "authority", "runtime", "exclusivity_group"}
    if not isinstance(writes, list):
        messages.append("[writer_declaration_invalid] workflow writes must be a list")
    elif len(writes) > WORKFLOW_MAX_WRITES:
        messages.append(
            f"[workflow_declaration_limit_exceeded] workflow writes exceeds {WORKFLOW_MAX_WRITES} entries"
        )
    else:
        for declaration in writes:
            valid = (
                isinstance(declaration, dict)
                and set(declaration) == write_fields
                and isinstance(declaration.get("artifact"), str)
                and re.fullmatch(r"[a-z][a-z0-9.-]*", declaration["artifact"])
                and _safe_workflow_write_path(declaration.get("path"))
                and declaration.get("authority") in {"canonical", "advisory"}
                and declaration.get("runtime") in WORKFLOW_RUNTIMES
                and isinstance(declaration.get("exclusivity_group"), str)
                and SLUG_RE.fullmatch(declaration["exclusivity_group"])
            )
            if not valid:
                messages.append(
                    "[writer_declaration_invalid] writer needs canonical artifact, safe path, authority, runtime, and exclusivity group"
                )
                break

    if mutability == "read-only" and {"Edit", "Write"}.intersection(runtime_tools):
        messages.append(
            "[readonly_mutation_capability] read-only workflow role grants Edit or Write"
        )
    return messages


def validate_workflow_metadata_fixture() -> list[Issue]:
    valid = {
        "workflow": {
            "schema_version": 1,
            "activation": "conditional",
            "mutability": "read-only",
            "skills": [{"id": "mastermind-example", "required": False}],
            "writes": [],
        }
    }
    checks = (
        not _workflow_metadata_messages(valid, required=True, runtime_tools=["Read"]),
        any(
            "workflow_metadata_missing" in message
            for message in _workflow_metadata_messages({}, required=True, runtime_tools=["Read"])
        ),
        any(
            "unknown workflow field" in message
            for message in _workflow_metadata_messages(
                {"workflow": {**valid["workflow"], "surprise": True}},
                required=True,
                runtime_tools=["Read"],
            )
        ),
        any(
            "readonly_mutation_capability" in message
            for message in _workflow_metadata_messages(
                valid, required=True, runtime_tools=["Read", "Write"]
            )
        ),
        any(
            "workflow_metadata_invalid" in message
            for message in _workflow_metadata_messages(
                {"workflow": {**valid["workflow"], "schema_version": True}},
                required=True,
                runtime_tools=["Read"],
            )
        ),
        any(
            "workflow_metadata_invalid" in message
            for message in _workflow_metadata_messages(
                {
                    "workflow": {
                        **valid["workflow"],
                        "skills": [{"id": "mastermind-example"}],
                    }
                },
                required=True,
                runtime_tools=["Read"],
            )
        ),
        any(
            "workflow_declaration_limit_exceeded" in message
            for message in _workflow_metadata_messages(
                {
                    "workflow": {
                        **valid["workflow"],
                        "writes": [{}] * (WORKFLOW_MAX_WRITES + 1),
                    }
                },
                required=True,
                runtime_tools=["Read"],
            )
        ),
    )
    if all(checks):
        return []
    return [
        Issue(
            REPO_ROOT / "scripts/validate.py",
            "error",
            "workflow metadata validator fixture failed",
        )
    ]


def _mcp_server_names(value: object) -> list[str]:
    if isinstance(value, list):
        return [entry for entry in value if isinstance(entry, str)]
    if isinstance(value, dict):
        return [entry for entry in value if isinstance(entry, str)]
    return []


def _subagent_mmcg_contract_messages(
    frontmatter: dict, body: str, known_tools: set[str]
) -> list[str]:
    """Return deterministic Claude Code MCP allowlist violations."""
    servers = _mcp_server_names(frontmatter.get("mcpServers"))
    runtime_tools = _runtime_tool_names(frontmatter.get("tools"))
    grants = sorted(
        tool for tool in runtime_tools if tool.startswith("mcp__mmcg__")
    )
    messages: list[str] = []
    all_referenced = set(MMCG_REFERENCE_RE.findall(body))

    if "mmcg" not in servers:
        if grants or all_referenced:
            messages.append(
                "[mmcg_server_scope_missing] uses mmcg tools but does not declare `mcpServers: [mmcg]`"
            )

    if "mmcg" in servers and not grants:
        messages.append(
            "[mmcg_scope_without_grant] declares `mcpServers: [mmcg]` but its `tools` allowlist grants no "
            "`mcp__mmcg__mmcg_*` tools"
        )

    wildcards = {grant for grant in grants if "*" in grant}
    for wildcard in sorted(wildcards):
        messages.append(
            f"[mmcg_wildcard_grant] uses broad `{wildcard}`; grant only the exact mmcg tools this role needs"
        )

    granted_mmcg = {
        grant.removeprefix("mcp__mmcg__")
        for grant in grants
        if grant not in wildcards
    }
    for granted in sorted(granted_mmcg - known_tools):
        messages.append(f"[mmcg_tool_unknown] grants unknown mmcg tool `{granted}`")

    for unknown in sorted(all_referenced - known_tools):
        messages.append(f"[mmcg_prompt_tool_unknown] prompt references unknown mmcg tool `{unknown}`")
    referenced = all_referenced & known_tools
    for required in sorted(referenced - granted_mmcg):
        messages.append(
            f"[mmcg_prompt_grant_missing] prompt references `{required}` but `tools` omits "
            f"`mcp__mmcg__{required}`"
        )
    return messages


def validate_subagent_mmcg_fixture(known_tools: set[str]) -> list[Issue]:
    bounded = {
        "model": "haiku",
        "tools": "Read, mcp__mmcg__mmcg_status, mcp__mmcg__mmcg_search",
        "mcpServers": ["mmcg"],
        "maxTurns": 12,
        "effort": "low",
    }
    broken = {
        **bounded,
        "mcpServers": ["mmcg"],
        "tools": "Read, Grep",
    }
    missing_reference = {
        **bounded,
        "mcpServers": ["mmcg"],
        "tools": "Read, mcp__mmcg__mmcg_status",
    }
    malformed_enums = {**bounded, "model": [], "effort": {}}
    missing_server = {**bounded, "mcpServers": []}
    wildcard = {**bounded, "tools": "Read, mcp__mmcg__*"}
    checks = (
        not _subagent_runtime_contract_messages(bounded),
        all(
            field in " ".join(_subagent_runtime_contract_messages({}))
            for field in ("model", "tools", "maxTurns", "effort")
        ),
        all(
            field in " ".join(
                _subagent_runtime_contract_messages(malformed_enums)
            )
            for field in ("model", "effort")
        ),
        any(
            "mcpServers" in message
            for message in _subagent_runtime_contract_messages(
                {**bounded, "mcpServers": "mmcg"}
            )
        ),
        bool(_subagent_mmcg_contract_messages(broken, "mmcg_search", known_tools)),
        any(
            "prompt references `mmcg_search`" in message
            for message in _subagent_mmcg_contract_messages(
                missing_reference, "Use mmcg_search.", known_tools
            )
        ),
        any(
            "does not declare `mcpServers: [mmcg]`" in message
            for message in _subagent_mmcg_contract_messages(
                missing_server, "Use mmcg_search.", known_tools
            )
        ),
        any(
            "uses broad `mcp__mmcg__*`" in message
            for message in _subagent_mmcg_contract_messages(
                wildcard, "Use mmcg_search.", known_tools
            )
        ),
        not _subagent_mmcg_contract_messages(
            bounded, "Use mmcg_search.", known_tools
        ),
    )
    if all(checks):
        return []
    return [
        Issue(
            REPO_ROOT / "scripts/validate.py",
            "error",
            "subagent mmcg allowlist validator fixture failed",
        )
    ]


def validate_subagent_mmcg_access(artifacts: list[Artifact]) -> list[Issue]:
    source = REPO_ROOT / MMCG_MCP_SRC
    if not source.is_file():
        return [Issue(source, "error", "mcp.rs missing — cannot validate agent tools")]
    known_tools = set(extract_mmcg_tools())
    issues = validate_subagent_mmcg_fixture(known_tools)
    if issues:
        return issues
    for artifact in artifacts:
        if artifact.path.parent.name != "subagents":
            continue
        try:
            text = artifact.path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        match = FRONTMATTER_RE.match(text)
        body = text[match.end() :] if match else text
        for message in _subagent_mmcg_contract_messages(
            artifact.frontmatter, body, known_tools
        ):
            issues.append(Issue(artifact.path, "error", message))
    return issues


def validate_mmcg_tool_extractor_fixture() -> list[Issue]:
    fixture = '''
static TOOLS: &[ToolDef] = &[
    ToolDef { name: "mmcg_legacy", schema: schema_legacy, handler: handle_legacy },
    read_only_tool("mmcg_reader", schema_reader, handle_reader),
    refreshable_tool("mmcg_refresher", schema_refresher, handle_refresher),
    additive_tool("mmcg_writer", schema_writer, handle_writer),
    read_only_tool("mmcg_reader", schema_reader, handle_reader),
];
'''
    expected = ["mmcg_legacy", "mmcg_reader", "mmcg_refresher", "mmcg_writer"]
    actual = _extract_mmcg_tools_from_source(fixture)
    if actual == expected:
        return []
    return [
        Issue(
            REPO_ROOT / "scripts/validate.py",
            "error",
            f"mmcg tool extractor fixture failed — expected {expected}, got {actual}",
        )
    ]


def validate_mmcg_tool_drift() -> list[Issue]:
    issues = validate_mmcg_tool_extractor_fixture()
    if issues:
        return issues
    src_path = REPO_ROOT / MMCG_MCP_SRC
    if not src_path.is_file():
        return [Issue(src_path, "error", "mcp.rs missing — cannot derive tool list")]

    tools = extract_mmcg_tools()
    if not tools:
        issues.append(
            Issue(src_path, "error", "could not extract any mmcg_* tool names from tools_list()")
        )
        return issues
    canonical_count = len(tools)

    # 1. Presence check — every tool name must appear in each docfile.
    for rel in MMCG_TOOL_LIST_DOCS:
        path = REPO_ROOT / rel
        if not path.is_file():
            issues.append(Issue(path, "error", f"missing doc file (referenced by tool-drift check)"))
            continue
        text = path.read_text()
        for tool in tools:
            # READMEs sometimes drop the `mmcg_` prefix (e.g. "search" / "callers"
            # in inline lists). Accept either spelling.
            short = tool.removeprefix("mmcg_")
            if tool not in text and short not in text:
                issues.append(
                    Issue(
                        path,
                        "error",
                        f"tool `{tool}` missing — declared in {MMCG_MCP_SRC} "
                        f"but absent from this file",
                    )
                )

    # 2. Count check — every "N structural query tools" / "N query tools" /
    # "N tool" claim must equal canonical_count.
    count_re = re.compile(r"(\d+)\s+(?:structural\s+)?(?:query\s+)?tools?\b")
    for rel in MMCG_TOOL_COUNT_DOCS:
        path = REPO_ROOT / rel
        if not path.is_file():
            continue
        for m in count_re.finditer(path.read_text()):
            stated = int(m.group(1))
            if stated != canonical_count:
                issues.append(
                    Issue(
                        path,
                        "error",
                        f"tool count drift — file says `{stated} tools` but "
                        f"{MMCG_MCP_SRC} declares {canonical_count}",
                    )
                )

    return issues


def validate_mmcg_template_mirrors() -> list[Issue]:
    issues: list[Issue] = []
    for canonical_rel, mirror_rel in MMCG_TEMPLATE_MIRRORS:
        canonical = REPO_ROOT / canonical_rel
        mirror = REPO_ROOT / mirror_rel
        if not canonical.exists():
            issues.append(Issue(canonical, "error", f"canonical template missing (referenced by mmcg mirror at {mirror_rel})"))
            continue
        if not mirror.exists():
            issues.append(
                Issue(
                    mirror,
                    "error",
                    f"mmcg template mirror missing — copy from canonical: `cp {canonical_rel} {mirror_rel}`",
                )
            )
            continue
        if canonical.read_bytes() != mirror.read_bytes():
            issues.append(
                Issue(
                    mirror,
                    "error",
                    f"out of sync with canonical {canonical_rel}. Resync: `cp {canonical_rel} {mirror_rel}`",
                )
            )
    return issues


# ----- installable-file link-escape check --------------------------------

# Files that get *copied* into a client workflow home (or a project root) by
# the npm installer / mastermind init must not have relative links that escape
# their package — those links resolve fine in the repo but break after install.
# Use absolute https://github.com/xcrft/mastermind/blob/main/... URLs instead.
#
# Map: regex matching repo-relative path → max `../` levels permitted in any
# relative markdown link from that file.
INSTALLABLE_MAX_PARENT_TRAVERSALS: list[tuple[re.Pattern, int]] = [
    # Subagents are copied flat to ~/.claude/agents/ — no `../` permitted at all.
    (re.compile(r"^agents/subagents/[^/]+\.md$"), 0),
    # CLAUDE.md templates copied to ~/.claude/templates/ or to a project's CLAUDE.md.
    (re.compile(r"^agents/claude-md/[^/]+\.md$"), 0),
    # Skills installed as ~/.claude/skills/<name>/SKILL.md (flat domain). One
    # `../` is OK — sibling skill in same install layout still resolves.
    (re.compile(r"^skills/[^/]+/[^/]+/SKILL\.md$"), 1),
    # Skill references — one `../` to the parent SKILL.md or sibling scripts/ is fine.
    (re.compile(r"^skills/[^/]+/[^/]+/references/.+\.md$"), 1),
    # MCP-server READMEs ship as the crate's published README — links escaping
    # the server folder don't resolve on crates.io.
    (re.compile(r"^mcp/servers/[^/]+/README\.md$"), 0),
]


def _count_parent_traversals(link_path: str) -> int:
    n = 0
    for part in link_path.split("/"):
        if part == "..":
            n += 1
        else:
            break
    return n


def validate_installable_link_escape(links: dict[Path, set[str]]) -> list[Issue]:
    issues: list[Issue] = []
    for source, targets in links.items():
        try:
            rel = source.relative_to(REPO_ROOT)
        except ValueError:
            continue
        rel_str = str(rel)
        max_allowed = None
        for pat, n in INSTALLABLE_MAX_PARENT_TRAVERSALS:
            if pat.match(rel_str):
                max_allowed = n
                break
        if max_allowed is None:
            continue
        for target in targets:
            if target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            path_part = target.split("#", 1)[0]
            traversals = _count_parent_traversals(path_part)
            if traversals > max_allowed:
                issues.append(
                    Issue(
                        source,
                        "warning",
                        f"installable file escapes package — link `{target}` goes {traversals} levels up (max {max_allowed} for this file class). Reference the artifact by name instead (e.g. `mastermind-critic` for a subagent, `/mastermind-task-planning` for a skill) — the agent has it loaded; no path needed.",
                    )
                )
    return issues


# ----- eval fixture clue guard ------------------------------------------

BANNED_FIXTURE_CLUES: list[str] = [
    "auditor must",
    "auditor should",
    "auditor catches",
    "scope creep",
    "unrelated to spec",
    "hallucinated",
    "mmcg_search",
    "mmcg_callers",
    "tsc would reject",
    "tsc rejects",
    "spec scoped",
    "executor added",
    "executor refactored",
    "executor changed",
    "not in the spec",
    "never defined",
]


def validate_eval_fixture_clues() -> list[Issue]:
    """Scan eval fixture source trees for embedded answer clues.

    Only `baseline/` and `changes/` trees are scanned; READMEs are
    excluded (they are the correct place for scenario explanations).
    """
    issues: list[Issue] = []
    fixtures_dir = REPO_ROOT / "evals" / "fixtures"
    if not fixtures_dir.is_dir():
        return issues
    for fixture_dir in sorted(fixtures_dir.iterdir()):
        if not fixture_dir.is_dir():
            continue
        for tree_name in ("baseline", "changes"):
            tree = fixture_dir / tree_name
            if not tree.is_dir():
                continue
            for src_file in tree.rglob("*"):
                if not src_file.is_file() or src_file.name == "README.md":
                    continue
                try:
                    text = src_file.read_text(encoding="utf-8", errors="ignore")
                except OSError:
                    continue
                text_lower = text.lower()
                for clue in BANNED_FIXTURE_CLUES:
                    if clue.lower() in text_lower:
                        issues.append(Issue(
                            src_file,
                            "error",
                            f"eval fixture contains banned clue {clue!r} — move explanation to the fixture README.md",
                        ))
                        break
    return issues


def validate_workflow_eval_contract() -> list[Issue]:
    """Keep planner/executor and product-skill regression coverage loadable."""
    path = REPO_ROOT / "evals/workflow.jsonl"
    issues: list[Issue] = []
    required_artifacts = {
        path.relative_to(REPO_ROOT).as_posix()
        for path in (REPO_ROOT / "skills").rglob("SKILL.md")
    }
    found_artifacts: set[str] = set()
    found_ids: set[str] = set()
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        return [Issue(path, "error", f"cannot read workflow eval cases: {error}")]
    for number, line in enumerate(lines, 1):
        if not line.strip() or line.lstrip().startswith("//"):
            continue
        try:
            case = json.loads(line)
        except json.JSONDecodeError as error:
            issues.append(Issue(path, "error", f"workflow eval line {number} is invalid JSON: {error}"))
            continue
        case_id = case.get("id")
        artifact = case.get("artifact")
        prompt = case.get("input", {}).get("prompt") if isinstance(case.get("input"), dict) else None
        expect = case.get("expect")
        if not isinstance(case_id, str) or not case_id.startswith("w-"):
            issues.append(Issue(path, "error", f"workflow eval line {number} needs a w-* id"))
        elif case_id in found_ids:
            issues.append(Issue(path, "error", f"duplicate workflow eval id {case_id}"))
        else:
            found_ids.add(case_id)
        if not isinstance(artifact, str):
            issues.append(Issue(path, "error", f"workflow eval line {number} needs an artifact"))
        else:
            found_artifacts.add(artifact)
            resolved = (REPO_ROOT / artifact).resolve()
            if REPO_ROOT.resolve() not in resolved.parents or not resolved.is_file():
                issues.append(Issue(path, "error", f"workflow eval artifact is missing or escapes the repository: {artifact}"))
        if not isinstance(prompt, str) or not prompt.strip():
            issues.append(Issue(path, "error", f"workflow eval line {number} needs a non-empty prompt"))
        if not isinstance(expect, dict) or not expect.get("contains"):
            issues.append(Issue(path, "error", f"workflow eval line {number} needs contains assertions"))
        elif "contains_any" in expect:
            groups = expect["contains_any"]
            valid_groups = (
                isinstance(groups, list)
                and all(
                    isinstance(group, list)
                    and len(group) >= 2
                    and all(isinstance(phrase, str) and phrase for phrase in group)
                    for group in groups
                )
            )
            if not valid_groups:
                issues.append(
                    Issue(
                        path,
                        "error",
                        f"workflow eval line {number} has invalid contains_any assertions",
                    )
                )
        if isinstance(expect, dict) and "code_comments" in expect:
            policy = expect["code_comments"]
            valid_policy = isinstance(policy, dict)
            if valid_policy:
                prefixes = policy.get("prefixes", ["//", "/*"])
                minimum = policy.get("min", 0)
                maximum = policy.get("max")
                forbidden = policy.get("not_contains", [])
                required_groups = policy.get("contains_any", [])
                valid_policy = (
                    isinstance(prefixes, list)
                    and bool(prefixes)
                    and all(isinstance(prefix, str) and prefix for prefix in prefixes)
                    and isinstance(minimum, int)
                    and not isinstance(minimum, bool)
                    and minimum >= 0
                    and (
                        maximum is None
                        or (
                            isinstance(maximum, int)
                            and not isinstance(maximum, bool)
                            and maximum >= minimum
                        )
                    )
                    and isinstance(policy.get("require_fenced_code", True), bool)
                    and isinstance(forbidden, list)
                    and all(
                        isinstance(phrase, str) and phrase
                        for phrase in forbidden
                    )
                    and isinstance(required_groups, list)
                    and all(
                        isinstance(group, list)
                        and len(group) >= 2
                        and all(isinstance(phrase, str) and phrase for phrase in group)
                        for group in required_groups
                    )
                )
            if not valid_policy:
                issues.append(
                    Issue(
                        path,
                        "error",
                        f"workflow eval line {number} has invalid code_comments policy",
                    )
                )
    if found_artifacts != required_artifacts:
        missing = required_artifacts - found_artifacts
        extra = found_artifacts - required_artifacts
        details = []
        if missing:
            details.append(f"missing {', '.join(sorted(missing))}")
        if extra:
            details.append(f"not allowlisted {', '.join(sorted(extra))}")
        issues.append(Issue(path, "error", f"workflow eval artifact set drifted: {'; '.join(details)}"))
    runner_path = REPO_ROOT / "evals/runner.py"
    try:
        runner = runner_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(runner_path, "error", f"cannot read workflow eval runner: {error}"))
    else:
        for token in (
            "WORKFLOW_ARTIFACTS",
            '"--safe-mode"',
            '"--tools", ""',
            "TemporaryDirectory",
            "requires_prompt_sandbox",
        ):
            if token not in runner:
                issues.append(Issue(runner_path, "error", f"workflow eval sandbox missing {token!r}"))
        if '"--permission-mode", "default"' in runner:
            issues.append(Issue(runner_path, "error", "workflow eval runner uses an invalid permission mode"))
    return issues


# ----- executor report schema parity ------------------------------------

def validate_executor_report_schema_contract() -> list[Issue]:
    """Keep the agent tail, strict Rust parser, fixture, and JSON schema aligned."""
    issues: list[Issue] = []
    schema_path = REPO_ROOT / "schemas/executor-report-v1.schema.json"
    try:
        schema = json.loads(schema_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return [Issue(schema_path, "error", f"invalid executor report schema: {error}")]

    expected = {
        "schema_version",
        "spec",
        "status",
        "phases",
        "files_modified",
        "claims",
        "defects",
        "verifications",
    }
    if set(schema.get("required", [])) != expected:
        issues.append(Issue(schema_path, "error", "executor report schema required fields drifted"))
    if schema.get("additionalProperties") is not False:
        issues.append(Issue(schema_path, "error", "executor report schema must fail closed on unknown fields"))

    contract_paths = [
        REPO_ROOT / "agents/subagents/mastermind-task-executor.md",
        REPO_ROOT / "skills/workflow/mastermind-structured-report-contract/SKILL.md",
        REPO_ROOT / "skills/workflow/mastermind-task-planning/references/structured-report-schema.md",
        REPO_ROOT / "mcp/servers/mmcg/tests/fixtures/executor-report-v1.md",
    ]
    for path in contract_paths:
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as error:
            issues.append(Issue(path, "error", f"cannot read executor report contract: {error}"))
            continue
        for token in ("mastermind:report-begin", "schema_version: 1", "claims:", "verifications:"):
            if token not in text:
                issues.append(Issue(path, "error", f"executor report contract missing {token!r}"))

    rust_path = REPO_ROOT / "mcp/servers/mmcg/src/executor_report.rs"
    try:
        rust = rust_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(rust_path, "error", f"cannot read executor report parser: {error}"))
    else:
        for token in (
            "mastermind:report-begin",
            "CanonicalExecutorReport",
            "deny_unknown_fields",
            "MAX_EXECUTOR_REPORT_BYTES",
            "complete executor report must not contain defects",
            "partial or failed executor report requires at least one defect",
        ):
            if token not in rust:
                issues.append(Issue(rust_path, "error", f"strict executor report parser missing {token!r}"))

    return issues


# ----- review evidence package -----------------------------------------

def validate_review_package_contract() -> list[Issue]:
    """Keep review export schemas, Rust implementation, and CI example aligned."""
    issues: list[Issue] = []
    manifest_path = REPO_ROOT / "schemas/mastermind-review-manifest-v1.schema.json"
    attestation_path = REPO_ROOT / "schemas/mastermind-evidence-attestation-v1.schema.json"
    for path, label, required in (
        (
            manifest_path,
            "review manifest",
            {
                "schema_version",
                "package_format",
                "generator",
                "repository",
                "scope",
                "analysis",
                "evidence_binding",
                "artifacts",
                "content_sha256",
            },
        ),
        (
            attestation_path,
            "evidence attestation",
            {"schema_version", "head_oid", "artifacts"},
        ),
    ):
        try:
            schema = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            issues.append(Issue(path, "error", f"invalid {label} schema: {error}"))
            continue
        if schema.get("additionalProperties") is not False:
            issues.append(Issue(path, "error", f"{label} schema must fail closed on unknown fields"))
        if set(schema.get("required", [])) != required:
            issues.append(Issue(path, "error", f"{label} schema required fields drifted"))
        if schema.get("properties", {}).get("schema_version", {}).get("const") != 1:
            issues.append(Issue(path, "error", f"{label} schema must pin version 1"))

    rust_path = REPO_ROOT / "mcp/servers/mmcg/src/review_package.rs"
    try:
        rust = rust_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(rust_path, "error", f"cannot read review exporter: {error}"))
    else:
        for token in (
            "REVIEW_PACKAGE_SCHEMA: u32 = 1",
            "EVIDENCE_ATTESTATION_SCHEMA: u32 = 1",
            "standalone_html",
            "mastermind.sarif",
            "summary.md",
            "manifest.json",
            "mastermind-review.yml",
            "digest-bound-at-export",
            "producer-attested",
            "from_json_strict",
        ):
            if token not in rust:
                issues.append(Issue(rust_path, "error", f"review exporter missing {token!r}"))

    workflow_path = REPO_ROOT / "docs/examples/mastermind-review-pr.yml"
    try:
        workflow_text = workflow_path.read_text(encoding="utf-8")
        workflow = yaml.safe_load(workflow_text)
    except (OSError, yaml.YAMLError) as error:
        issues.append(Issue(workflow_path, "error", f"invalid review workflow: {error}"))
    else:
        package_path = REPO_ROOT / "npm/mastermind/package.json"
        try:
            package_version = json.loads(package_path.read_text(encoding="utf-8"))["version"]
        except (OSError, json.JSONDecodeError, KeyError) as error:
            issues.append(Issue(package_path, "error", f"cannot resolve review workflow version: {error}"))
        else:
            if f"@xcraftmind/mastermind@{package_version}" not in workflow_text:
                issues.append(Issue(workflow_path, "error", "review workflow must pin the current npm version"))
        for token in (
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "actions/setup-node@820762786026740c76f36085b0efc47a31fe5020",
            "dtolnay/rust-toolchain@4cda84d5c5c54efe2404f9d843567869ab1699d4",
            "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
            "actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c",
            "github/codeql-action/upload-sarif@c54b30b7df092240050e69945842bc67aee0f0f4",
            "github.event.pull_request.head.sha",
            "github.event.pull_request.base.sha",
            "refs/pull/${{ github.event.pull_request.number }}/head",
            "github.repository == 'xcrft/mastermind'",
            "persist-credentials: false",
            "review export --since",
            "review export --help",
            "sarif_file: mastermind-review/mastermind.sarif",
            "github.event.pull_request.head.repo.full_name == github.repository",
            "github.actor != 'dependabot[bot]'",
        ):
            if token not in workflow_text:
                issues.append(Issue(workflow_path, "error", f"review workflow missing {token!r}"))
        if "pull_request_target" in workflow_text:
            issues.append(Issue(workflow_path, "error", "review workflow must never run on pull_request_target"))
        jobs = workflow.get("jobs", {}) if isinstance(workflow, dict) else {}
        review = jobs.get("review", {}) if isinstance(jobs, dict) else {}
        sarif_job = jobs.get("sarif", {}) if isinstance(jobs, dict) else {}
        review_permissions = review.get("permissions", {}) if isinstance(review, dict) else {}
        sarif_permissions = sarif_job.get("permissions", {}) if isinstance(sarif_job, dict) else {}
        if review_permissions != {"contents": "read"}:
            issues.append(Issue(workflow_path, "error", "review build job permissions must be read-only"))
        if sarif_permissions != {"actions": "read", "contents": "read", "security-events": "write"}:
            issues.append(Issue(workflow_path, "error", "SARIF upload job permissions must be exact and minimal"))
        if sarif_job.get("needs") != "review" or any("run" in step for step in sarif_job.get("steps", [])):
            issues.append(Issue(workflow_path, "error", "SARIF upload job must consume the review artifact without running pull-request code"))
        embedded_path = REPO_ROOT / "mcp/servers/mmcg/assets/mastermind-review-pr.yml"
        try:
            embedded_text = embedded_path.read_text(encoding="utf-8")
        except OSError as error:
            issues.append(Issue(embedded_path, "error", f"cannot read embedded review workflow: {error}"))
        else:
            if embedded_text != workflow_text:
                issues.append(Issue(embedded_path, "error", "embedded review workflow must match the documented example byte-for-byte"))
    return issues


# ----- declarative fact-ingestion SDK -----------------------------------

def validate_fact_ingestion_sdk_contract() -> list[Issue]:
    """Keep the public fact schema and its non-executable ingestion boundary aligned."""
    issues: list[Issue] = []
    schema_path = REPO_ROOT / "schemas/mastermind-facts-v1.schema.json"
    try:
        schema = json.loads(schema_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return [Issue(schema_path, "error", f"invalid fact manifest schema: {error}")]

    required = {
        "api_version",
        "capabilities",
        "repository",
        "producer",
        "dataset",
        "provenance",
        "files",
        "artifacts",
        "facts",
    }
    if schema.get("additionalProperties") is not False:
        issues.append(Issue(schema_path, "error", "fact schema must reject unknown root fields"))
    if set(schema.get("required", [])) != required:
        issues.append(Issue(schema_path, "error", "fact schema required fields drifted"))
    properties = schema.get("properties", {})
    if properties.get("api_version", {}).get("const") != "mastermind-facts/v1":
        issues.append(Issue(schema_path, "error", "fact schema must pin mastermind-facts/v1"))
    capabilities = properties.get("capabilities", {}).get("items", {}).get("enum", [])
    if capabilities != ["annotations", "relationships"]:
        issues.append(Issue(schema_path, "error", "fact capability allowlist drifted"))
    definitions = schema.get("$defs", {})
    for name in (
        "repository",
        "producer",
        "provenance",
        "file",
        "artifact",
        "location",
        "annotation",
        "relationship",
    ):
        if definitions.get(name, {}).get("additionalProperties") is not False:
            issues.append(
                Issue(schema_path, "error", f"fact schema definition {name!r} must reject unknown fields")
            )

    facts_path = REPO_ROOT / "mcp/servers/mmcg/src/facts.rs"
    try:
        rust = facts_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(facts_path, "error", f"cannot read fact ingestion module: {error}"))
    else:
        for token in (
            'API_VERSION: &str = "mastermind-facts/v1"',
            'SUPPORTED_CAPABILITIES: [&str; 2] = ["annotations", "relationships"]',
            "deny_unknown_fields",
            "from_json_strict",
            "run_bounded_git_with_limit",
            "validate_index_root",
            "current_head_oid",
            "MAX_MANIFEST_BYTES",
            "replace_fact_dataset",
            "snapshot_for_paths",
            "fact_source_stale",
        ):
            if token not in rust:
                issues.append(Issue(facts_path, "error", f"fact ingestion contract missing {token!r}"))
        production = rust.split("#[cfg(test)]", 1)[0]
        for forbidden in ("Command::new(", "CREATE TABLE", "libloading", "dlopen"):
            if forbidden in production:
                issues.append(
                    Issue(facts_path, "error", f"fact ingestion boundary contains forbidden {forbidden!r}")
                )

    store_path = REPO_ROOT / "mcp/servers/mmcg/src/store.rs"
    try:
        store = store_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(store_path, "error", f"cannot read normalized fact store: {error}"))
    else:
        for token in (
            "fact_sources",
            "fact_files",
            "fact_artifacts",
            "fact_annotations",
            "fact_relationships",
            "replace_fact_dataset",
            "transaction()",
        ):
            if token not in store:
                issues.append(Issue(store_path, "error", f"normalized fact store missing {token!r}"))

    surface_paths = {
        "mcp/servers/mmcg/src/main.rs": ("facts: Option<PathBuf>", "QueryCmd::Facts"),
        "mcp/servers/mmcg/src/mcp.rs": (
            'read_only_tool("mmcg_facts"',
            "crate::facts::snapshot",
        ),
        "mcp/servers/mmcg/src/evidence.rs": (
            "snapshot_for_paths",
            "fact_artifacts",
            "fact_relationships",
        ),
        "mcp/servers/mmcg/src/review_package.rs": (
            "normalized_fact_revision_binding",
            "fact_artifacts",
            'kind: "facts".into()',
        ),
        "mcp/servers/mmcg/assets/lens/app.js": (
            "factMatchesGraphEdge",
            "factEvidence",
        ),
        "docs/fact-ingestion-sdk.md": (
            "mastermind-facts/v1",
            "mastermind enrich --facts",
            "no direct SQLite access",
        ),
    }
    for relative, tokens in surface_paths.items():
        path = REPO_ROOT / relative
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as error:
            issues.append(Issue(path, "error", f"cannot read fact SDK surface: {error}"))
            continue
        for token in tokens:
            if token not in text:
                issues.append(Issue(path, "error", f"fact SDK surface missing {token!r}"))

    return issues


def validate_extension_lifecycle_contract() -> list[Issue]:
    """Keep adapters, signed provenance, team federation, and registry smoke closed."""
    issues: list[Issue] = []
    schemas = (
        (
            REPO_ROOT / "schemas/mastermind-fact-signature-v1.schema.json",
            "fact signature",
            {
                "schema_version",
                "domain",
                "algorithm",
                "canonicalization",
                "key_id",
                "manifest_digest",
                "signature",
            },
        ),
        (
            REPO_ROOT / "schemas/mastermind-team-v1.schema.json",
            "team graph",
            {"api_version", "repositories", "relationships"},
        ),
    )
    for path, label, required in schemas:
        try:
            schema = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            issues.append(Issue(path, "error", f"invalid {label} schema: {error}"))
            continue
        if schema.get("additionalProperties") is not False:
            issues.append(Issue(path, "error", f"{label} schema must reject unknown root fields"))
        if set(schema.get("required", [])) != required:
            issues.append(Issue(path, "error", f"{label} schema required fields drifted"))

    surfaces = {
        "mcp/servers/mmcg/src/fact_adapter.rs": (
            "collect_for_fact_adapter",
            "facts_total != Some(source.facts_returned)",
            "validate_generated_manifest",
            "indexed_paths_bounded",
        ),
        "mcp/servers/mmcg/src/fact_signature.rs": (
            'SIGNATURE_DOMAIN: &str = "mastermind/fact-manifest-signature/v1"',
            "generate_keypair",
            "verify_stored_proof",
            "trusted_key_ids",
            "revoked_key_ids",
        ),
        "mcp/servers/mmcg/src/team.rs": (
            'API_VERSION: &str = "mastermind-team/v1"',
            "open_read_only_with_deadline",
            "validate_index_snapshot",
            'provenance: "team-manifest"',
            "manifest_sha256",
            "MAX_REPOSITORIES",
            "MAX_INDEX_TOTAL_BYTES",
        ),
        "mcp/servers/mmcg/src/mcp.rs": (
            'read_only_tool("mmcg_team_map"',
            "schema_team_map",
            "handle_team_map",
            "MMCG_TEAM_MANIFEST",
            "MMCG_TEAM_MANIFEST_SHA256",
        ),
        "mcp/servers/mmcg/src/main.rs": (
            "Facts(FactCmd)",
            "Team(TeamCmd)",
            "FactCmd::Keygen",
            "FactTrustPolicy",
        ),
        "mcp/servers/mmcg/src/review_package.rs": (
            "producer-signed",
            "signing_public_key",
            "signed_manifest_digest",
        ),
        ".github/workflows/publish-npm.yml": (
            "smoke-installed-npm-release.sh",
            "Publish or verify all 8 packages",
        ),
        ".github/workflows/publish-mmcg.yml": (
            "smoke-installed-crate-release.sh",
            "Publish exact verified crate bytes",
        ),
    }
    for relative, tokens in surfaces.items():
        path = REPO_ROOT / relative
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as error:
            issues.append(Issue(path, "error", f"cannot read extension lifecycle surface: {error}"))
            continue
        for token in tokens:
            if token not in text:
                issues.append(Issue(path, "error", f"extension lifecycle surface missing {token!r}"))

    for relative in (
        "scripts/smoke-installed-npm-release.sh",
        "scripts/smoke-installed-crate-release.sh",
    ):
        path = REPO_ROOT / relative
        if not os.access(path, os.X_OK):
            issues.append(Issue(path, "error", "registry smoke helper must be executable"))
    return issues


# ----- repository workflow supply-chain contract -----------------------

def validate_repository_workflow_pins() -> list[Issue]:
    issues: list[Issue] = []
    workflow_dir = REPO_ROOT / ".github/workflows"
    use_re = re.compile(r"^\s*(?:-\s*)?uses:\s*([^\s#]+)", re.MULTILINE)
    full_sha = re.compile(r"^[0-9a-f]{40}$")
    workflow_paths = sorted(
        path
        for path in workflow_dir.iterdir()
        if path.is_file() and path.suffix in {".yml", ".yaml"}
    )
    for path in workflow_paths:
        text = path.read_text(encoding="utf-8")
        for target in use_re.findall(text):
            if target.startswith("./"):
                continue
            if "@" not in target:
                issues.append(Issue(path, "error", f"Action reference lacks an immutable ref: {target}"))
                continue
            action, ref = target.rsplit("@", 1)
            if not full_sha.fullmatch(ref):
                issues.append(Issue(path, "error", f"Action {action} must use a full 40-character commit SHA"))

    for name in ("publish-npm.yml", "publish-mmcg.yml"):
        path = workflow_dir / name
        text = path.read_text(encoding="utf-8")
        if "github.event.inputs" in text:
            issues.append(Issue(path, "error", "manual workflow inputs must never authorize publishing"))
        if "github.repository == 'xcrft/mastermind' && github.event_name == 'push'" not in text:
            issues.append(Issue(path, "error", "publish job must require a tag push in the canonical repository"))
        if "fetch-depth: 0" not in text or "merge-base --is-ancestor" not in text:
            issues.append(Issue(path, "error", "tag publication must verify that the release commit is reachable from main"))

    for name in ("ci-mmcg.yml", "ci-npm.yml", "supply-chain.yml", "validate.yml"):
        path = workflow_dir / name
        try:
            workflow = yaml.safe_load(path.read_text(encoding="utf-8"))
        except (OSError, yaml.YAMLError) as error:
            issues.append(Issue(path, "error", f"invalid required workflow YAML: {error}"))
            continue
        if isinstance(workflow, dict) and required_workflow_has_path_filter(workflow):
            issues.append(Issue(path, "error", "required workflow must not use pull_request path filters"))
    return issues


# ----- cross-client skill adapter contract ------------------------------

def validate_openai_skill_adapters(artifacts: list[Artifact]) -> list[Issue]:
    issues: list[Issue] = []
    for artifact in artifacts:
        if artifact.path.name != "SKILL.md" or "skills" not in artifact.path.parts:
            continue
        adapter = artifact.path.parent / "agents/openai.yaml"
        try:
            value = yaml.safe_load(adapter.read_text(encoding="utf-8"))
        except (OSError, yaml.YAMLError) as error:
            issues.append(Issue(adapter, "error", f"missing or invalid OpenAI skill adapter: {error}"))
            continue
        interface = value.get("interface", {}) if isinstance(value, dict) else {}
        required = ("display_name", "short_description", "default_prompt")
        if not all(isinstance(interface.get(key), str) and interface[key].strip() for key in required):
            issues.append(Issue(adapter, "error", "OpenAI adapter requires display_name, short_description, and default_prompt"))
        elif f"${artifact.slug}" not in interface["default_prompt"]:
            issues.append(Issue(adapter, "error", f"OpenAI default_prompt must invoke ${artifact.slug}"))

        metadata = artifact.frontmatter.get("metadata", {})
        if isinstance(metadata, dict) and "model" in metadata:
            issues.append(Issue(artifact.path, "error", "portable skill metadata must not hard-code a model vendor tier"))
    return issues


def validate_workflow_role_contracts() -> list[Issue]:
    """Prevent planner/executor/auditor ownership from drifting back together."""
    issues: list[Issue] = []
    auditor_path = REPO_ROOT / "agents/subagents/mastermind-auditor.md"
    planner_path = REPO_ROOT / "skills/workflow/mastermind-task-planning/SKILL.md"
    executor_path = REPO_ROOT / "agents/subagents/mastermind-task-executor.md"
    try:
        auditor = auditor_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(auditor_path, "error", f"cannot read auditor contract: {error}"))
    else:
        for token in ("repository-read-only", "must not mutate", "mastermind:audit-begin"):
            if token not in auditor:
                issues.append(Issue(auditor_path, "error", f"auditor read-only contract missing {token!r}"))
        for forbidden in ("### Write state.json", "### Capture lesson", "auditor appends"):
            if forbidden in auditor:
                issues.append(Issue(auditor_path, "error", f"auditor must not own persistence: {forbidden!r}"))
    try:
        planner = planner_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(planner_path, "error", f"cannot read planner contract: {error}"))
    else:
        for token in (
            "Persist the reviewed result (planner/controller only)",
            "The auditor is repository-read-only",
            "advisory evidence",
            "controller's\n`audit.md` and `state.json` remain the persisted machine record",
        ):
            if token not in planner:
                issues.append(Issue(planner_path, "error", f"planner persistence contract missing {token!r}"))
        step_headings = re.findall(r"^### Step (9[a-z])\b", planner, re.MULTILINE)
        if len(step_headings) != len(set(step_headings)):
            issues.append(Issue(planner_path, "error", "planner has duplicate Step 9 sub-step headings"))
        if len(planner.encode("utf-8")) > 16_000:
            issues.append(Issue(planner_path, "error", "planner contract exceeds the 16 KB anti-ceremony budget"))
    try:
        executor = executor_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(executor_path, "error", f"cannot read executor contract: {error}"))
    else:
        for token in ("<task>/executor-report.md", "Never write `state.json`", "controller owns lifecycle state"):
            if token not in executor:
                issues.append(Issue(executor_path, "error", f"executor ownership contract missing {token!r}"))
        if "### Write state.json" in executor:
            issues.append(Issue(executor_path, "error", "executor must not own lifecycle state"))
    workflow_path = REPO_ROOT / "agents/claude-md/mastermind-workflow.md"
    try:
        workflow = workflow_path.read_bytes()
    except OSError as error:
        issues.append(Issue(workflow_path, "error", f"cannot read project workflow contract: {error}"))
    else:
        if len(workflow) > 10_000:
            issues.append(Issue(workflow_path, "error", "project workflow exceeds the 10 KB anti-ceremony budget"))
        text = workflow.decode("utf-8")
        ordered = (
            text.find("mastermind verify-spec"),
            text.find("The user approves Scope and Acceptance Criteria"),
            text.find("mastermind run-task .mastermind/tasks/<task>/spec.md --pre-only"),
        )
        if min(ordered) < 0 or ordered != tuple(sorted(ordered)):
            issues.append(Issue(workflow_path, "error", "workflow must validate read-only, obtain approval, then write pre-flight state"))
    return issues


def validate_portable_skill_semantics() -> list[Issue]:
    """Reject known cross-client and lifecycle claims that the runtime cannot honor."""
    issues: list[Issue] = []
    checks = {
        "skills/workflow/mastermind-task-planning/SKILL.md": {
            "required": ("mastermind verify-spec <task>/spec.md", "After the user approves Scope and Acceptance Criteria"),
            "forbidden": ("planner-persisted auditor verdict",),
        },
        "skills/workflow/mastermind-task-executor/SKILL.md": {
            "required": ("bounded repair loop", "Acceptance Criteria define success"),
            "forbidden": ("Do not retry with modifications", "Execute it anyway"),
        },
        "skills/workflow/mastermind-structured-report-contract/SKILL.md": {
            "required": ("<task>/executor-report.md", "not a Rust-parsed\nlifecycle input"),
            "forbidden": ("planner extracts it with one regex", "planner applies the taxonomy fix and re-spawns"),
        },
        "skills/workflow/mastermind-codegraph-research/SKILL.md": {
            "required": ("syntactic evidence", "read the\nsource"),
            "forbidden": ("Do NOT re-verify mmcg results", "always cheaper"),
        },
        "skills/prompt-engineering/mastermind-prompt-refiner/SKILL.md": {
            "required": ("## Original request", "Do not activate just because"),
            "forbidden": ("The planner sees the refined version, not the user's brain dump", "Mounted as the intake gate"),
        },
        "skills/security/mastermind-agent-security-review/SKILL.md": {
            "required": ("self-contained for Codex", "## Review protocol", "## Output"),
            "forbidden": ("The review *protocol* lives in that subagent",),
        },
    }
    for relative, contract in checks.items():
        path = REPO_ROOT / relative
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as error:
            issues.append(Issue(path, "error", f"cannot read portable skill contract: {error}"))
            continue
        for token in contract["required"]:
            if token not in text:
                issues.append(Issue(path, "error", f"portable skill contract missing {token!r}"))
        for token in contract["forbidden"]:
            if token in text:
                issues.append(Issue(path, "error", f"portable skill retains stale claim {token!r}"))

    readme_path = REPO_ROOT / "skills/README.md"
    readme = readme_path.read_text(encoding="utf-8")
    for skill_path in sorted((REPO_ROOT / "skills").rglob("SKILL.md")):
        slug = skill_path.parent.name
        if f"[`{slug}`]" not in readme:
            issues.append(Issue(readme_path, "error", f"installed skill is missing from index: {slug}"))

    agent_readme_path = REPO_ROOT / "agents/README.md"
    agent_readme = agent_readme_path.read_text(encoding="utf-8")
    for agent_path in sorted((REPO_ROOT / "agents/subagents").glob("*.md")):
        slug = agent_path.stem
        if f"[`{slug}`]" not in agent_readme:
            issues.append(
                Issue(agent_readme_path, "error", f"installed agent is missing from index: {slug}")
            )
    return issues


# ----- verifiable audit Action security contract -----------------------

AUDIT_ACTION_PINS = {
    "actions/checkout": "9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0",
    "actions/upload-artifact": "043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
    "actions/download-artifact": "3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c",
    "actions/attest": "f7c74d28b9d84cb8768d0b8ca14a4bac6ef463e6",
}
AUDIT_EXAMPLES = (
    "docs/examples/mastermind-audit-pr.yml",
    "docs/examples/mastermind-audit-publish.yml",
)


def _workflow_trigger(workflow: dict) -> object:
    return workflow.get("on", workflow.get(True))


def required_workflow_has_path_filter(workflow: dict) -> bool:
    """Return whether a required PR workflow can be suppressed by file paths."""
    trigger = _workflow_trigger(workflow)
    pull_request = trigger.get("pull_request") if isinstance(trigger, dict) else None
    return isinstance(pull_request, dict) and any(
        key in pull_request for key in ("paths", "paths-ignore")
    )


def audit_pr_contract_errors(text: str, workflow: dict) -> list[str]:
    errors: list[str] = []
    expected_name = "mastermind-pr-audit-attempt-${{ github.run_attempt }}"
    jobs = workflow.get("jobs", {}) if isinstance(workflow, dict) else {}
    steps = jobs.get("audit", {}).get("steps", []) if isinstance(jobs, dict) else []
    upload_names = []
    if isinstance(steps, list):
        for step in steps:
            if isinstance(step, dict) and step.get("uses") == f"actions/upload-artifact@{AUDIT_ACTION_PINS['actions/upload-artifact']}":
                upload_names.append(step.get("with", {}).get("name"))
    if upload_names != [expected_name]:
        errors.append("PR producer must upload exactly one attempt-specific artifact name from github.run_attempt")
    if "name: mastermind-pr-audit\n" in text:
        errors.append("PR producer must not use a run-wide fixed artifact name")
    return errors


def audit_publication_contract_errors(text: str, workflow: dict) -> list[str]:
    errors: list[str] = []
    required_counts = {
        "exact run-attempt API binding": ("?attempt_number=$PUBLICATION_RUN_ATTEMPT", 2),
        "exact raw artifact ID download": ("actions/artifacts/$ARTIFACT_ID/zip", 3),
        "raw artifact digest comparison": ('test "$actual" = "$ARTIFACT_DIGEST"', 3),
        "raw artifact size comparison": ('wc -c <', 3),
        "regular-file archive preflight": ("not stat.S_ISREG(mode)", 3),
        "special archive metadata preflight": ("info.extra", 3),
        "duplicate archive preflight": ("name in seen", 3),
        "per-file archive size cap": ("16 * 1024 * 1024", 3),
        "total archive size cap": ("64 * 1024 * 1024", 3),
    }
    for label, (fragment, minimum) in required_counts.items():
        if text.count(fragment) < minimum:
            errors.append(f"publication workflow lacks {label} in every consumer")
    if "printf 'artifact_size=%s\\n' \"$artifact_size\"" not in text:
        errors.append("source artifact size is not exported from independent evidence")
    if "?attempt_number=$SOURCE_RUN_ATTEMPT" not in text:
        errors.append("source run evidence is not bound to the exact run attempt")
    attempt_binding = {
        'source_run_attempt=$(jq -er \'.run_attempt | select(type == "number" and . >= 1)\' <<<"$run")': "API-confirmed source run attempt",
        'expected_artifact_name="mastermind-pr-audit-attempt-$source_run_attempt"': "attempt-specific expected artifact name",
        'artifacts?name=$expected_artifact_name': "exact attempt-specific artifact query",
        '.name == $expected': "exact artifact-name filter",
        '[.artifacts[] | select(.expired == false and .name == $expected)] | if length == 1 then .[0] else error("exactly one attempt-specific artifact required") end': "single non-expired exact-name artifact requirement",
        "printf 'artifact_name=%s\\n' \"$expected_artifact_name\"": "artifact-name evidence output",
    }
    for fragment, label in attempt_binding.items():
        if fragment not in text:
            errors.append(f"publication workflow lacks {label}")
    if "artifact_created" in text or "run_started" in text:
        errors.append("publication workflow must not use timestamps as attempt identity")
    if "source_artifact_size: ${{ steps.evidence.outputs.artifact_size }}" not in text:
        errors.append("source artifact size is not bound as a verify output")
    if "verified_artifact_size: ${{ steps.verified-artifact.outputs.artifact_size }}" not in text:
        errors.append("verified artifact size is not bound as a verify output")
    if text.count('test "$size" = "$ARTIFACT_SIZE"') < 2:
        errors.append("privileged consumers do not bind server size to verified evidence")
    if text.count('test "$(jq -r .id <<<"$server")" = "$ARTIFACT_ID"') < 1:
        errors.append("verified artifact metadata is not bound to the exact artifact ID")
    if "REPLACE_WITH_FULL_" in text or "REPLACE_WITH_ALLOWED_VERIFIER" in text:
        errors.append("executable publication workflow contains an unresolved verifier reference")

    jobs = workflow.get("jobs", {}) if isinstance(workflow, dict) else {}
    if not isinstance(jobs, dict):
        return errors + ["publication jobs must be a mapping"]
    publish_needs = jobs.get("publish", {}).get("needs", [])
    if not isinstance(publish_needs, list) or set(publish_needs) != {"verify", "attest"}:
        errors.append("publish must be blocked on both verify and attest")
    attest_steps = jobs.get("attest", {}).get("steps", [])
    attest_subjects = []
    if isinstance(attest_steps, list):
        for step in attest_steps:
            if isinstance(step, dict) and step.get("uses") == f"actions/attest@{AUDIT_ACTION_PINS['actions/attest']}":
                subject = step.get("with", {}).get("subject-path", "")
                attest_subjects = [line.strip() for line in subject.splitlines() if line.strip()]
    if attest_subjects != ["verified-subject/verified.tar", "verified-subject/verified-statement.json"]:
        errors.append("attestation subjects do not exactly match the digest-verified extracted files")
    return errors


def dockerfile_from_images(text: str) -> list[str]:
    """Return every Dockerfile FROM image in stage order.

    Docker instruction names are case-insensitive and may have leading horizontal
    whitespace. Keep an empty sentinel for a FROM line without a parseable image so
    malformed or continuation-based instructions fail the exact allowlist check.
    """
    images: list[str] = []
    for line in text.splitlines():
        match = re.match(r"^[ \t]*from(?:[ \t]+(.*))?$", line, flags=re.IGNORECASE)
        if match is None:
            continue
        tokens = (match.group(1) or "").split()
        image_index = 0
        while image_index < len(tokens) and tokens[image_index].startswith("--"):
            image_index += 1
        images.append(tokens[image_index] if image_index < len(tokens) else "")
    return images


def validate_audit_action_security() -> list[Issue]:
    issues: list[Issue] = []
    parsed: dict[str, dict] = {}
    for relative in AUDIT_EXAMPLES:
        path = REPO_ROOT / relative
        if not path.is_file():
            issues.append(Issue(path, "error", "required audit workflow example is missing"))
            continue
        text = path.read_text(encoding="utf-8")
        if "pull_request_target" in text:
            issues.append(Issue(path, "error", "pull_request_target is forbidden"))
        try:
            value = yaml.safe_load(text)
        except yaml.YAMLError as error:
            issues.append(Issue(path, "error", f"invalid workflow YAML: {error}"))
            continue
        if not isinstance(value, dict):
            issues.append(Issue(path, "error", "workflow root must be a mapping"))
            continue
        parsed[relative] = value
        for match in re.finditer(r"uses:\s*([^\s#]+)", text):
            reference = match.group(1)
            if reference == "./":
                if relative != "docs/examples/mastermind-audit-pr.yml" or not (REPO_ROOT / "action.yml").is_file():
                    issues.append(Issue(path, "error", "local Action reference is absent or outside the unprivileged PR workflow"))
                continue
            if "@" not in reference:
                issues.append(Issue(path, "error", f"Action reference lacks immutable revision: {reference}"))
                continue
            action, revision = reference.rsplit("@", 1)
            if action in AUDIT_ACTION_PINS:
                if revision != AUDIT_ACTION_PINS[action]:
                    issues.append(Issue(path, "error", f"{action} must use audited commit {AUDIT_ACTION_PINS[action]}"))
            elif action == "xcrft/mastermind":
                if not re.fullmatch(r"[0-9a-f]{40}", revision):
                    issues.append(Issue(path, "error", "Mastermind Action must use a full commit SHA"))
            elif action == "xcrft/mastermind/.github/actions/verify-only":
                issues.append(Issue(path, "error", "missing external verify-only Action is forbidden; use the workflow-bound verifier"))
            else:
                issues.append(Issue(path, "error", f"Action is not in the audit allowlist: {action}"))
            if revision in {"main", "master"} or re.fullmatch(r"v\d+(?:\.\d+)*", revision):
                issues.append(Issue(path, "error", f"mutable Action reference is forbidden: {reference}"))

    pr_path = REPO_ROOT / AUDIT_EXAMPLES[0]
    pr = parsed.get(AUDIT_EXAMPLES[0])
    if pr:
        trigger = _workflow_trigger(pr)
        if not isinstance(trigger, dict) or set(trigger) != {"pull_request"}:
            issues.append(Issue(pr_path, "error", "PR audit must trigger only on pull_request"))
        permissions = pr.get("permissions")
        if permissions != {"contents": "read"}:
            issues.append(Issue(pr_path, "error", "PR audit top-level permissions must be exactly contents: read"))
        text = pr_path.read_text(encoding="utf-8")
        if "id-token: write" in text or "secrets:" in text:
            issues.append(Issue(pr_path, "error", "PR audit must not receive OIDC or secrets"))
        if "persist-credentials: false" not in text:
            issues.append(Issue(pr_path, "error", "PR checkout must disable credential persistence"))
        if "uses: ./" not in text:
            issues.append(Issue(pr_path, "error", "unprivileged PR workflow must use the present repository Action"))
        for error in audit_pr_contract_errors(text, pr):
            issues.append(Issue(pr_path, "error", error))

    publish_path = REPO_ROOT / AUDIT_EXAMPLES[1]
    publish = parsed.get(AUDIT_EXAMPLES[1])
    if publish:
        trigger = _workflow_trigger(publish)
        if not isinstance(trigger, dict) or set(trigger) != {"workflow_run"}:
            issues.append(Issue(publish_path, "error", "publication must trigger only on workflow_run"))
        text = publish_path.read_text(encoding="utf-8")
        if "actions/checkout@" in text:
            issues.append(Issue(publish_path, "error", "privileged publication must never checkout source"))
        if "id-token: write" not in text or "attestations: write" not in text:
            issues.append(Issue(publish_path, "error", "attestation job must receive OIDC and attestation authority"))
        jobs = publish.get("jobs", {})
        publish_permissions = jobs.get("publish", {}).get("permissions", {}) if isinstance(jobs, dict) else {}
        if publish_permissions.get("pull-requests") != "write" or "id-token" in publish_permissions:
            issues.append(Issue(publish_path, "error", "publication job must have PR write but no OIDC"))
        if "REPLACE_WITH_FULL_" in text or "REPLACE_WITH_ALLOWED_VERIFIER" in text:
            issues.append(Issue(publish_path, "error", "executable publication workflow must not contain unresolved verifier references"))
        if "mastermind-inline-schema-v3-verifier-v1" not in text:
            issues.append(Issue(publish_path, "error", "publication workflow lacks the allowlisted workflow-bound verifier identity"))
        for error in audit_publication_contract_errors(text, publish):
            issues.append(Issue(publish_path, "error", error))

    action_path = REPO_ROOT / "action.yml"
    try:
        action = yaml.safe_load(action_path.read_text(encoding="utf-8"))
    except (OSError, yaml.YAMLError) as error:
        issues.append(Issue(action_path, "error", f"invalid Action metadata: {error}"))
    else:
        required_inputs = ["root", "since", "bundle-dir", "expected-repository", "expected-baseline", "expected-head", "require-clean-worktree"]
        if not isinstance(action, dict) or not set(required_inputs).issubset(set(action.get("inputs", {}))):
            issues.append(Issue(action_path, "error", "Action metadata lacks mandatory immutable-snapshot inputs"))
        elif action.get("runs", {}).get("args") != [f"${{{{ inputs.{name} }}}}" for name in required_inputs]:
            issues.append(Issue(action_path, "error", "Docker Action inputs must cross the container boundary through ordered runs.args"))

    docker_path = REPO_ROOT / "Dockerfile.audit-action"
    try:
        docker_text = docker_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(docker_path, "error", f"cannot read Dockerfile: {error}"))
    else:
        expected_from = [
            "rust:1.98-bookworm@sha256:e70e2eec3d495fd5c8e0be74adda86507dfac7f51a724fbf9813ff59b2b247c7",
            "buildpack-deps:bookworm-scm@sha256:de4e518f98c6533eceeee6f8b14a77a918856fa8282a1b711c0292d089157c0c",
        ]
        actual_from = dockerfile_from_images(docker_text)
        if actual_from != expected_from:
            issues.append(Issue(docker_path, "error", "Docker stages must use the two audited immutable OCI digests"))
        if "cargo +1.96.0 build" not in docker_text or "--locked" not in docker_text:
            issues.append(Issue(docker_path, "error", "Docker Action must build with Rust 1.96 and the Cargo lockfile"))
        if "COPY mcp/servers/mmcg/benches ./mcp/servers/mmcg/benches" not in docker_text:
            issues.append(Issue(docker_path, "error", "Docker Action must include Cargo's declared benchmark target"))
        if "COPY mcp/servers/mmcg/build.rs ./mcp/servers/mmcg/build.rs" not in docker_text:
            issues.append(Issue(docker_path, "error", "Docker Action must include Cargo's declared build script"))
        if "COPY --chmod=0755 scripts/audit-action-entrypoint.sh" not in docker_text:
            issues.append(Issue(docker_path, "error", "Docker Action must install an executable entrypoint"))
        if re.search(r"^USER\s+", docker_text, re.MULTILINE):
            issues.append(Issue(docker_path, "error", "GitHub Docker Action must use the default root user for GITHUB_WORKSPACE access"))
        if "RUN git --version" not in docker_text or "ENV HOME=/tmp/mastermind" not in docker_text:
            issues.append(Issue(docker_path, "error", "Docker runtime must prove Git exists and provide a private HOME"))
        if re.search(r"\b(?:apt|apk|yum|dnf)(?:-get)?\b", docker_text):
            issues.append(Issue(docker_path, "error", "Docker Action must not perform an unpinned package install"))

    entrypoint_path = REPO_ROOT / "scripts/audit-action-entrypoint.sh"
    try:
        entrypoint = entrypoint_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(entrypoint_path, "error", f"cannot read Action entrypoint: {error}"))
    else:
        if entrypoint_path.stat().st_mode & 0o100 == 0:
            issues.append(Issue(entrypoint_path, "error", "Action entrypoint must be executable in the Git tree"))
        if "set -eu" not in entrypoint:
            issues.append(Issue(entrypoint_path, "error", "Action entrypoint must use set -eu"))
        if re.search(r"(^|\s)(eval|source|\.)\s", entrypoint, re.MULTILINE):
            issues.append(Issue(entrypoint_path, "error", "Action entrypoint must not eval or source repository data"))
        if "--expected-baseline" not in entrypoint or "--expected-head" not in entrypoint or "--expected-repository" not in entrypoint:
            issues.append(Issue(entrypoint_path, "error", "Action entrypoint must enforce exact repository/baseline/head policy"))
        if "audit prepare-output" not in entrypoint or 'test "$1" = "."' not in entrypoint:
            issues.append(Issue(entrypoint_path, "error", "Action entrypoint must accept root dot and delegate output creation to the Rust no-follow helper"))
        if "--changed-only" not in entrypoint or "--require-executor-report" not in entrypoint:
            issues.append(Issue(entrypoint_path, "error", "Action entrypoint must audit only changed tasks and require executor evidence"))
        if "handoff_output_to_workspace_owner" not in entrypoint or "chown -R -P --no-dereference --preserve-root" not in entrypoint:
            issues.append(Issue(entrypoint_path, "error", "root-owned Docker Action outputs must be handed back to the GITHUB_WORKSPACE owner"))

    return issues


# ----- entry point ------------------------------------------------------


def main(argv: list[str]) -> int:
    artifacts = list(find_artifacts())
    issues: list[Issue] = []
    for a in artifacts:
        issues.extend(validate_artifact(a))
    issues.extend(validate_workflow_metadata_fixture())
    issues.extend(validate_subagent_mmcg_access(artifacts))
    issues.extend(validate_openai_skill_adapters(artifacts))
    issues.extend(validate_workflow_role_contracts())
    issues.extend(validate_portable_skill_semantics())

    links = collect_wikilinks()
    issues.extend(validate_wikilinks(artifacts, links))

    rel_links = collect_relative_links()
    issues.extend(validate_relative_links(rel_links))
    issues.extend(validate_installable_link_escape(rel_links))
    issues.extend(validate_release_badges())
    issues.extend(validate_distributed_language_metadata())

    issues.extend(validate_mmcg_template_mirrors())
    issues.extend(validate_mmcg_tool_drift())
    issues.extend(validate_eval_fixture_clues())
    issues.extend(validate_workflow_eval_contract())
    issues.extend(validate_executor_report_schema_contract())
    issues.extend(validate_review_package_contract())
    issues.extend(validate_fact_ingestion_sdk_contract())
    issues.extend(validate_extension_lifecycle_contract())
    issues.extend(validate_repository_workflow_pins())
    issues.extend(validate_audit_action_security())

    issues.sort(key=lambda i: (str(i.path), 0 if i.level == "error" else 1, i.msg))
    for i in issues:
        print(i.render())

    errors = sum(1 for i in issues if i.level == "error")
    warnings = sum(1 for i in issues if i.level == "warning")

    print(
        f"\nChecked {len(artifacts)} artifacts. "
        f"{errors} error(s), {warnings} warning(s)."
    )
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
