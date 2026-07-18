#!/usr/bin/env python3
"""
Validate Mastermind repo artifacts against docs/conventions.md.

Runs in CI on every PR. Catches:
  - Missing or malformed frontmatter
  - `name:` field not matching the file/folder slug (§1.2)
  - Non-kebab-case slugs (§1.1)
  - Missing `description:`
  - Missing or non-SemVer `metadata.version` (§6)
  - Domain folder not in the conventions.md whitelist (§1.3)
  - `[[slug]]` cross-references that don't resolve to any artifact

Exit code 0 if clean, 1 if any errors. Warnings do not fail the build.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

try:
    import yaml
except ImportError:
    print("error: PyYAML not installed. Run: pip install -r scripts/requirements.txt", file=sys.stderr)
    sys.exit(2)


REPO_ROOT = Path(__file__).resolve().parent.parent

# Allowed domain folders for skills/ and prompts/, per docs/conventions.md §1.3
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
            Issue(a.path, "warning", f"'description' is very short ({len(desc)} chars). See conventions §2.2 — leads with a verb, states triggers.")
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
                Issue(a.path, "warning", f"first tag ({tags[0]!r}) does not match domain folder ({a.domain!r}). See conventions §2.3.")
            )

    if a.domain is not None and a.domain not in ALLOWED_DOMAINS:
        issues.append(
            Issue(a.path, "error", f"unknown domain {a.domain!r}. Allowed: {sorted(ALLOWED_DOMAINS)}. Update docs/conventions.md if adding.")
        )

    # Subagents are loaded by the Claude Code runtime, which reads `tools`, `model`,
    # `mcpServers`, and `disallowedTools` as TOP-LEVEL frontmatter keys. Nesting them
    # under `metadata` makes Claude Code silently ignore them — the subagent then
    # inherits every tool and the parent's model (conventions §2.4). Catch the
    # regression at lint time so it can't ship again.
    if a.path.parent.name == "subagents" and isinstance(metadata, dict):
        for field in ("tools", "model", "mcpServers", "disallowedTools"):
            if field in metadata:
                issues.append(
                    Issue(
                        a.path,
                        "error",
                        f"subagent runtime field {field!r} is nested under 'metadata' — "
                        "Claude Code reads it only at the top level (conventions §2.4); move it up",
                    )
                )

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
    if rel_parts and rel_parts[0] in {"docs", "research", "scripts", "examples", "extras", "evals"}:
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
# inside the `tools_list()` JSON. Every doc file that enumerates tools — top
# READMEs and install-facing docs — must mention every tool by name, or it
# drifts and lies about the API. This validator extracts the tool names from
# the Rust source and asserts they all appear in each docfile.

MMCG_MCP_SRC = "mcp/servers/mmcg/src/mcp.rs"

# Files that must reference every mmcg_* tool name. (Each may have one
# canonical mention line; we only require *presence* of each tool name in the
# file's text. False positives possible if a name appears in unrelated prose,
# but mmcg_xxx names are sufficiently distinctive.)
MMCG_TOOL_LIST_DOCS: list[str] = [
    "mcp/servers/mmcg/README.md",
    "README.md",
]

# Files that quote a tool *count* like "16 structural query tools" / "15 tools".
# We extract counts and assert they all match the count derived from mcp.rs.
MMCG_TOOL_COUNT_DOCS: list[str] = [
    "mcp/servers/mmcg/README.md",
    "README.md",
]


def _extract_mmcg_tools_from_source(src: str) -> list[str]:
    start = src.find("static TOOLS:")
    end = src.find("];", start) + 2 if start != -1 else -1
    body = src[start:end] if start != -1 and end != -1 else src
    pattern = re.compile(
        r'name:\s*"(mmcg_[a-z_]+)"'
        r'|(?:read_only_tool|additive_tool)\s*\(\s*"(mmcg_[a-z_]+)"'
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


def validate_mmcg_tool_extractor_fixture() -> list[Issue]:
    fixture = '''
static TOOLS: &[ToolDef] = &[
    ToolDef { name: "mmcg_legacy", schema: schema_legacy, handler: handle_legacy },
    read_only_tool("mmcg_reader", schema_reader, handle_reader),
    additive_tool("mmcg_writer", schema_writer, handle_writer),
    read_only_tool("mmcg_reader", schema_reader, handle_reader),
];
'''
    expected = ["mmcg_legacy", "mmcg_reader", "mmcg_writer"]
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

# Files that get *copied* into ~/.claude/ (or a project root) by install.sh /
# mmcg init must not have relative links that escape their package — those
# links resolve fine in the repo but break after install. Use absolute
# https://github.com/xcrft/mastermind/blob/main/... URLs instead.
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
        required_inputs = {"root", "since", "bundle-dir", "expected-repository", "expected-baseline", "expected-head", "require-clean-worktree"}
        if not isinstance(action, dict) or not required_inputs.issubset(set(action.get("inputs", {}))):
            issues.append(Issue(action_path, "error", "Action metadata lacks mandatory immutable-snapshot inputs"))

    docker_path = REPO_ROOT / "Dockerfile.audit-action"
    try:
        docker_text = docker_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(docker_path, "error", f"cannot read Dockerfile: {error}"))
    else:
        expected_from = {
            "rust:1.96-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663",
            "buildpack-deps:bookworm-scm@sha256:877e9e4d949edfbcbedabc3a2d7ab593955fee5d6d0777adf3a991eb30c750d8",
        }
        actual_from = {line.split()[1] for line in docker_text.splitlines() if line.startswith("FROM ")}
        if actual_from != expected_from:
            issues.append(Issue(docker_path, "error", "Docker stages must use the two audited immutable OCI digests"))
        if "cargo +1.96.0 build" not in docker_text or "--locked" not in docker_text or "USER 65532:65532" not in docker_text:
            issues.append(Issue(docker_path, "error", "Docker Action must build with Rust 1.96 locked and run as the fixed non-root user"))
        if "RUN git --version" not in docker_text or "ENV HOME=/tmp/mastermind" not in docker_text:
            issues.append(Issue(docker_path, "error", "Docker runtime must prove Git exists and provide the non-root private HOME"))
        if re.search(r"\b(?:apt|apk|yum|dnf)(?:-get)?\b", docker_text):
            issues.append(Issue(docker_path, "error", "Docker Action must not perform an unpinned package install"))

    entrypoint_path = REPO_ROOT / "scripts/audit-action-entrypoint.sh"
    try:
        entrypoint = entrypoint_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(entrypoint_path, "error", f"cannot read Action entrypoint: {error}"))
    else:
        if "set -eu" not in entrypoint:
            issues.append(Issue(entrypoint_path, "error", "Action entrypoint must use set -eu"))
        if re.search(r"(^|\s)(eval|source|\.)\s", entrypoint, re.MULTILINE):
            issues.append(Issue(entrypoint_path, "error", "Action entrypoint must not eval or source repository data"))
        if "--expected-baseline" not in entrypoint or "--expected-head" not in entrypoint or "--expected-repository" not in entrypoint:
            issues.append(Issue(entrypoint_path, "error", "Action entrypoint must enforce exact repository/baseline/head policy"))
        if "audit prepare-output" not in entrypoint or 'test "$1" = "."' not in entrypoint:
            issues.append(Issue(entrypoint_path, "error", "Action entrypoint must accept root dot and delegate output creation to the Rust no-follow helper"))

    return issues


# ----- entry point ------------------------------------------------------


def main(argv: list[str]) -> int:
    artifacts = list(find_artifacts())
    issues: list[Issue] = []
    for a in artifacts:
        issues.extend(validate_artifact(a))

    links = collect_wikilinks()
    issues.extend(validate_wikilinks(artifacts, links))

    rel_links = collect_relative_links()
    issues.extend(validate_relative_links(rel_links))
    issues.extend(validate_installable_link_escape(rel_links))

    issues.extend(validate_mmcg_template_mirrors())
    issues.extend(validate_mmcg_tool_drift())
    issues.extend(validate_eval_fixture_clues())
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
