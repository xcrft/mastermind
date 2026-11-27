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

if __package__ in {None, ""}:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from evals.behavior.paths import FIXTURES_RELATIVE_PATH
from scripts import release_version

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
# A subagent's capability class is its mutability plus the capabilities it
# declares. Each declared capability needs a reason, and every tool that needs
# a capability must have it declared.
WORKFLOW_CAPABILITY_TOOLS = {
    # Every tool that runs a command or code: Monitor runs a command in the
    # background and REPL runs JavaScript in Code Mode. BashOutput, KillShell
    # and TaskStop read and stop what the role started, so they are shell
    # tools too. From <https://code.claude.com/docs/en/tools-reference>.
    "shell": {"Bash", "PowerShell", "Monitor", "REPL", "BashOutput", "KillShell", "TaskStop"},
    "web": {"WebFetch", "WebSearch"},
    # Tools that hand work to another agent or a later prompt with its own
    # tools: subagents, workflows, messages and scheduled prompts, including
    # the next `/loop` iteration ScheduleWakeup sets up.
    "delegate": {"Agent", "Task", "Workflow", "SendMessage", "CronCreate", "RemoteTrigger", "ScheduleWakeup"},
    # Skill runs a skill in the conversation, and a skill can bring its own
    # allowed tools and hooks.
    "skills": {"Skill"},
    # Hooks in the subagent's own frontmatter. Checked from `hooks:` below,
    # not from its tools.
    "hooks": set(),
    # MCP tools of any server other than mmcg, matched by prefix below, and
    # the resource tools, which read from every connected server.
    "mcp": {"ListMcpResourcesTool", "ReadMcpResourceTool"},
    # Tools that send or publish something outside the session: a claude.ai
    # artifact or share link, a notification, a file sent to the user, a
    # feedback report or a suggestion card.
    "outbound": {
        "Artifact",
        "PushNotification",
        "SendFeedback",
        "SendUserFile",
        "ShareOnboardingGuide",
        "SuggestPluginInstall",
        "SuggestSkills",
        "propose_skills",
    },
    # Tools that change the session: its working directory, its mode, its
    # task list or its scheduled tasks.
    "session": {
        "CronDelete",
        "EndConversation",
        "EnterPlanMode",
        "EnterWorktree",
        "ExitPlanMode",
        "ExitWorktree",
        "TaskCreate",
        "TaskUpdate",
        "TodoWrite",
    },
}
# Tools a read-only role may hold with no capability: they read files, code,
# lists or the user's answers and change nothing. Any other tool in a
# read-only role is an error, so a tool Claude Code adds later is denied
# until it is classified here. From
# <https://code.claude.com/docs/en/tools-reference>.
WORKFLOW_READ_TOOLS = {
    "AskUserQuestion",
    "CronList",
    "Glob",
    "Grep",
    "LSP",
    "ListAgents",
    "ListPlugins",
    "ListSkills",
    "Read",
    "ReportFindings",
    "SearchPlugins",
    "SearchSkills",
    "SubagentHandback",
    "TaskGet",
    "TaskList",
    "TaskOutput",
    "ToolSearch",
    "WaitForMcpServers",
}
# Capabilities a read-only role can never declare, since each changes
# something outside the files it reads.
WORKFLOW_READ_ONLY_DENIED_CAPABILITIES = {"outbound", "session"}
WORKFLOW_MUTATION_TOOLS = {"Edit", "Write", "NotebookEdit", "MultiEdit"}
WORKFLOW_CAPABILITY_REASON_MIN = 20
WORKFLOW_CAPABILITY_REASON_WORDS = 4
# Permission modes that approve edits or every tool. A read-only role must not
# ask for them.
WORKFLOW_READ_ONLY_FORBIDDEN_MODES = {"bypassPermissions", "acceptEdits"}
# Role and capability pairs no declaration can unlock. These roles must work
# from Read, Grep, Glob and mmcg, or from what the caller hands them.
WORKFLOW_DENIED_CAPABILITIES = {
    "mastermind-researcher": {"shell", "delegate", "web", "mcp"},
    "mastermind-critic": {"shell", "delegate"},
    "mastermind-prompt-refiner": {"shell", "delegate", "web", "mcp"},
    "mastermind-comment-auditor": {"shell", "delegate"},
    "mastermind-frontend-auditor": {"shell", "delegate"},
}
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


# Hosts list every installed skill by its description. When the list outgrows
# Codex's budget, Codex cuts every description to an equal-length prefix, so
# the trigger and the boundary both have to start early to survive the cut.
SKILL_DESCRIPTION_MAX_CHARS = 300
SKILL_TRIGGER_WINDOW_CHARS = 200
SKILL_TRIGGER_RE = re.compile(r"\bUse (?:when|before|after|for|whenever|to)\b")
SKILL_BOUNDARY_RE = re.compile(r"\bNot for\b")


def _skill_description_issues(path: Path, desc: str) -> list[Issue]:
    issues: list[Issue] = []
    if len(desc) > SKILL_DESCRIPTION_MAX_CHARS:
        issues.append(
            Issue(path, "error", f"[skill_description_too_long] description is {len(desc)} chars; keep it within {SKILL_DESCRIPTION_MAX_CHARS}")
        )
    trigger = SKILL_TRIGGER_RE.search(desc)
    if trigger is None or trigger.start() > SKILL_TRIGGER_WINDOW_CHARS:
        issues.append(
            Issue(path, "error", f"[skill_trigger_missing] description must say 'Use when/before/after/for/to' within its first {SKILL_TRIGGER_WINDOW_CHARS} chars")
        )
    boundary = SKILL_BOUNDARY_RE.search(desc)
    if boundary is None or boundary.start() > SKILL_TRIGGER_WINDOW_CHARS:
        issues.append(
            Issue(path, "error", f"[skill_boundary_missing] description must say 'Not for ...' within its first {SKILL_TRIGGER_WINDOW_CHARS} chars and name what to use instead")
        )
    return issues


# Claude Code delegates to a subagent when a task matches its description, and
# every installed description sits in the parent's context. A loose match
# spawns a costly or writing role nobody asked for, so the description opens
# with the trigger and says what to use instead before the first 200 chars.
# Codex gets no Mastermind subagents, but the same budget logic keeps them short.
SUBAGENT_DESCRIPTION_MAX_CHARS = 300
SUBAGENT_TRIGGER_RE = re.compile(r"Use (?:only )?(?:when|before|after|for|to)\b")
SUBAGENT_MANUAL_TRIGGER = "Use only when"
# Claude's docs say wording like "use proactively" or "MUST BE USED" makes it
# delegate on its own. The workflow spawns roles by name, so these phrases only
# add delegation nobody asked for. The lookarounds keep "non-proactive" legal.
SUBAGENT_PUSH_DELEGATION_RE = re.compile(
    r"(?<![\w-])(?:must\s+be\s+used|proactive(?:ly)?|always\s+(?:use|delegate)|use\s+immediately)(?![\w-])",
    re.IGNORECASE,
)

# The one canonical hand-off form is a lowercase `(use <name>)` with full
# shipped names, joined by commas, "or" or "and". The parser reads every
# `(use` case-insensitively so a near miss like `(Use mastermind-x)` or
# `(use diff-review)` is reported instead of silently skipped.
DESCRIPTION_REFERRAL_OPEN_RE = re.compile(r"\(\s*use\b", re.IGNORECASE)
DESCRIPTION_REFERRAL_CANONICAL_OPEN = "(use "
DESCRIPTION_REFERRAL_SPLIT_RE = re.compile(r"\s*,\s*(?:or\s+|and\s+)?|\s+or\s+|\s+and\s+")
DESCRIPTION_REFERRAL_NAME_RE = re.compile(r"(?:mastermind-[a-z0-9]+(?:-[a-z0-9]+)*|no-ai-slop-comments)")
DESCRIPTION_NAME_MENTION_RE = re.compile(r"(?<![\w./-])(?:mastermind-[a-z0-9-]*[a-z0-9]|no-ai-slop-comments)(?![\w-])", re.IGNORECASE)


@dataclass(frozen=True)
class Referral:
    """One `(use ...)` clause: where it sits, the text it hands off and its targets."""

    start: int
    end: int
    clause: str
    raw: str
    targets: tuple[str, ...]

    @property
    def canonical_open(self) -> bool:
        return self.raw.startswith(DESCRIPTION_REFERRAL_CANONICAL_OPEN)


def _matching_paren(text: str, start: int) -> int:
    depth = 0
    for index in range(start, len(text)):
        if text[index] == "(":
            depth += 1
        elif text[index] == ")":
            depth -= 1
            if depth == 0:
                return index
    return len(text) - 1


def description_referral_clauses(desc: str) -> list[Referral]:
    """Every `(use ...)` clause with the boundary text it applies to.

    The clause text runs from the end of the previous `(use ...)`, or from
    `Not for`, up to this one, so "Not for a diff (use a) or a plan (use b)"
    gives ("a diff", a) and ("or a plan", b).
    """
    referrals: list[Referral] = []
    boundary = SKILL_BOUNDARY_RE.search(desc)
    cursor = boundary.end() if boundary else 0
    for match in DESCRIPTION_REFERRAL_OPEN_RE.finditer(desc):
        if referrals and match.start() <= referrals[-1].end:
            continue
        end = _matching_paren(desc, match.start())
        raw = desc[match.start(): end + 1]
        body = raw[match.end() - match.start(): -1] if raw.endswith(")") else raw[match.end() - match.start():]
        body = re.sub(r"\([^()]*\)", " ", body)
        targets = tuple(part.strip(" .`'\"") for part in DESCRIPTION_REFERRAL_SPLIT_RE.split(body.strip()) if part.strip(" .`'\""))
        clause_start = cursor if cursor <= match.start() else 0
        clause = desc[clause_start: match.start()].strip(" ,.")
        referrals.append(Referral(match.start(), end, clause, raw, targets))
        cursor = end + 1
    return referrals


def description_referrals(desc: str) -> list[str]:
    """Names a description sends the host to in its canonical `(use ...)` clauses."""
    names: list[str] = []
    for referral in description_referral_clauses(desc):
        if not referral.canonical_open:
            continue
        names.extend(target for target in referral.targets if DESCRIPTION_REFERRAL_NAME_RE.fullmatch(target))
    return names


def _subagent_description_issues(path: Path, desc: str, activation: object) -> list[Issue]:
    issues: list[Issue] = []
    if len(desc) > SUBAGENT_DESCRIPTION_MAX_CHARS:
        issues.append(
            Issue(path, "error", f"[subagent_description_too_long] description is {len(desc)} chars; keep it within {SUBAGENT_DESCRIPTION_MAX_CHARS}")
        )
    if SUBAGENT_TRIGGER_RE.match(desc) is None:
        issues.append(
            Issue(path, "error", "[subagent_trigger_missing] description must start with 'Use when/before/after/for/to' or 'Use only when'")
        )
    elif activation == "manual" and not desc.startswith(SUBAGENT_MANUAL_TRIGGER):
        issues.append(
            Issue(path, "error", f"[subagent_manual_trigger] a manual subagent's description must start with '{SUBAGENT_MANUAL_TRIGGER}' so the host does not delegate to it unasked")
        )
    push = SUBAGENT_PUSH_DELEGATION_RE.search(desc)
    if push is not None:
        issues.append(
            Issue(path, "error", f"[subagent_push_delegation] {push.group(0)!r} pushes the host to delegate on its own; the workflow spawns subagents by name")
        )
    boundary = SKILL_BOUNDARY_RE.search(desc)
    if boundary is None or boundary.start() > SKILL_TRIGGER_WINDOW_CHARS:
        issues.append(
            Issue(path, "error", f"[subagent_boundary_missing] description must say 'Not for ...' within its first {SKILL_TRIGGER_WINDOW_CHARS} chars and name what to use instead")
        )
    elif not any(r.start > boundary.start() for r in description_referral_clauses(desc)):
        issues.append(
            Issue(path, "error", "[subagent_boundary_missing] the 'Not for ...' boundary must name what to use instead with '(use <name>)'")
        )
    return issues


def _referral_target_issue(target: str, known: set[str], kind: str) -> str | None:
    if target in known:
        return None
    folded = target.casefold()
    if folded in known:
        return f"[referral_noncanonical] '(use {target})' must spell the name in lowercase: {folded}"
    prefixed = f"mastermind-{folded}"
    if prefixed in known:
        return f"[referral_noncanonical] '(use {target})' must use the full name {prefixed}"
    return f"[referral_unknown] description says 'use {target}', which is not a shipped {kind}"


def validate_description_referrals(artifacts: list[Artifact]) -> list[Issue]:
    """Every hand-off in a skill or subagent description is canonical and resolves.

    Names may appear only inside a lowercase `(use ...)` clause, every target
    in that clause must be a shipped name, and a subagent's `Not for` boundary
    must hand off to at least one name that resolves.
    """
    skills = {a.name for a in artifacts if a.path.name == "SKILL.md" and a.name}
    subagents = {a.name for a in artifacts if a.path.parent.name == "subagents" and a.name}
    issues: list[Issue] = []
    for a in artifacts:
        desc = a.frontmatter.get("description")
        if not isinstance(desc, str):
            continue
        if a.path.name == "SKILL.md":
            # A skill can only hand the host to another skill: subagents are a
            # Claude-only install and are not in Codex's skill list.
            known, kind = skills, "skill"
        elif a.path.parent.name == "subagents":
            known, kind = skills | subagents, "skill or subagent"
        else:
            continue
        referrals = description_referral_clauses(desc)
        for referral in referrals:
            if not referral.canonical_open:
                issues.append(Issue(a.path, "error", f"[referral_noncanonical] write {referral.raw!r} as a lowercase '(use <name>)'"))
            if not referral.targets:
                issues.append(Issue(a.path, "error", f"[referral_unknown] {referral.raw!r} names nothing"))
            for target in referral.targets:
                if target == a.name:
                    issues.append(Issue(a.path, "error", f"[referral_self] description sends the host to itself ({target})"))
                    continue
                message = _referral_target_issue(target, known, kind)
                if message:
                    issues.append(Issue(a.path, "error", message))
        for mention in DESCRIPTION_NAME_MENTION_RE.finditer(desc):
            if not any(r.start <= mention.start() <= r.end for r in referrals):
                issues.append(
                    Issue(a.path, "error", f"[referral_outside_clause] {mention.group(0)!r} is named outside a '(use ...)' clause; hand off only with '(use <name>)'")
                )
        if kind == "skill or subagent":
            boundary = SKILL_BOUNDARY_RE.search(desc)
            resolved = [
                target
                for r in referrals
                if boundary is not None and r.start > boundary.start() and r.canonical_open
                for target in r.targets
                if target in known and target != a.name
            ]
            if boundary is not None and not resolved:
                issues.append(Issue(a.path, "error", "[subagent_boundary_unresolved] the 'Not for ...' boundary must hand off to at least one shipped name"))
    return issues


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
    elif a.path.name == "SKILL.md":
        issues.extend(_skill_description_issues(a.path, desc))
    elif a.path.parent.name == "subagents" and not _has_template_part(a.path):
        workflow = a.frontmatter.get("workflow")
        activation = workflow.get("activation") if isinstance(workflow, dict) else None
        issues.extend(_subagent_description_issues(a.path, desc, activation))

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


# These trees show `[[slug]]` syntax as illustration, not as references to
# artifacts. Their relative links are still checked.
WIKILINK_ILLUSTRATION_ROOTS = {"docs", "research", "scripts", "examples", "evals"}


def collect_wikilinks() -> dict[Path, set[str]]:
    """Walk all .md files and collect [[slug]] references. Skips code fences and
    inline code spans so documentation showing the syntax doesn't trigger."""
    result: dict[Path, set[str]] = {}
    for p in REPO_ROOT.rglob("*.md"):
        if _is_excluded(p) or p.relative_to(REPO_ROOT).parts[0] in WIKILINK_ILLUSTRATION_ROOTS:
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
    try:
        rel = path.relative_to(REPO_ROOT)
    except ValueError:
        return False
    parts = rel.parts
    # Build artifacts, local state and version control inside this repository.
    if any(part in {".git", ".mastermind", "target", "node_modules", "__pycache__"} for part in parts):
        return True
    # Agent worktrees are other checkouts of this repository.
    if parts[:2] == (".claude", "worktrees"):
        return True

    # Templates show example references, not real ones
    if "_template" in parts:
        return True

    rel_parts = rel.parts
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


# Retained reports whose exact bytes are bound by SHA-256 in other records.
# Editing a dead link would break that binding, so their links are not checked.
HASH_BOUND_REPORTS = {
    "evals/baselines/statistical-reanalysis-20261015.md",
}


def validate_relative_links(links: dict[Path, set[str]]) -> list[Issue]:
    issues: list[Issue] = []
    for source, targets in links.items():
        if source.relative_to(REPO_ROOT).as_posix() in HASH_BOUND_REPORTS:
            continue
        source_dir = source.parent
        for target in targets:
            resolved = (source_dir / target).resolve()
            if not resolved.exists():
                rel = target
                issues.append(
                    Issue(source, "error", f"broken markdown link → {rel!r} (resolved to {resolved})")
                )
    return issues


def validate_release_versions() -> list[Issue]:
    """Keep every shipped version string equal to the crate version."""
    try:
        problems = release_version.mismatches(REPO_ROOT)
    except (OSError, ValueError) as error:
        return [Issue(REPO_ROOT / release_version.SOURCE, "error", str(error))]
    return [Issue(REPO_ROOT / path, "error", message) for path, message in problems]


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
MMCG_EXPECTED_LAST_TOOL = "mmcg_mining_submit"
MMCG_EXPECTED_BEHAVIOR_COUNTS = {
    "refreshable_tool": 22,
    "read_only_tool": 14,
    "additive_tool": 1,
}

# The reference owns the complete catalog; integration guides link to it.
MMCG_TOOL_LIST_DOCS: list[str] = [
    "docs/reference/mcp.md",
]

# Files that quote a tool *count* like "16 structural query tools" / "15 tools".
# We extract counts and assert they all match the count derived from mcp.rs.
MMCG_TOOL_COUNT_DOCS: list[str] = [
    "docs/reference/mmcg.md",
    "docs/reference/mcp.md",
    "docs/integrations/generic-mcp.md",
    "mcp/servers/mmcg/README.md",
    "npm/mastermind/README.md",
    "README.md",
]

# Role-entry instructions must keep the bounded packet as the first discovery
# step. Otherwise a prompt edit can silently regress back to several broad MCP
# calls even while the tool catalog itself stays correct.
MMCG_BRIEF_ROLE_DOCS: dict[str, str] = {
    "agents/subagents/mastermind-auditor.md": "auditor",
    "agents/subagents/mastermind-task-executor.md": "executor",
    "skills/workflow/mastermind-task-planning/SKILL.md": "planner",
}


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
    allowed = {"schema_version", "activation", "mutability", "capabilities", "skills", "writes"}
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

    messages.extend(_capability_messages(workflow, runtime_tools, frontmatter))
    return messages


def workflow_capability_class(workflow: dict) -> str:
    """Name a role's capability class, such as `read-only-with-shell`."""
    capabilities = workflow.get("capabilities")
    declared = sorted(capabilities) if isinstance(capabilities, dict) else []
    name = str(workflow.get("mutability"))
    return name + "".join(f"-with-{capability}" for capability in declared)


def _tool_base(tool: str) -> str:
    """The tool name without a `(specifier)`, such as `Agent` for `Agent(x)`."""
    return tool.split("(", 1)[0].strip()


def _tool_capability(tool: str) -> str | None:
    base = _tool_base(tool)
    if base.startswith("mcp__"):
        return None if base.startswith("mcp__mmcg__") else "mcp"
    for capability, tools in WORKFLOW_CAPABILITY_TOOLS.items():
        if base in tools:
            return capability
    return None


def _reason_problem(reason: object) -> str | None:
    """Why a capability reason is too weak to explain what the role runs."""
    if not isinstance(reason, str) or len(reason.strip()) < WORKFLOW_CAPABILITY_REASON_MIN:
        return f"needs a reason of at least {WORKFLOW_CAPABILITY_REASON_MIN} characters"
    words = re.findall(r"[A-Za-z][A-Za-z'-]+", reason)
    if len(set(word.lower() for word in words)) < WORKFLOW_CAPABILITY_REASON_WORDS:
        return f"needs a reason of at least {WORKFLOW_CAPABILITY_REASON_WORDS} different words"
    letters = [c.lower() for c in reason if c.isalnum()]
    if len(set(letters)) < 6:
        return "needs a reason that is not one repeated character"
    return None


# A small Python version of the `mastermind doctor` hook rules, so this check
# runs without building mmcg. It covers the patterns a shipped subagent could
# plausibly carry; doctor reads the full set at run time.
HOOK_SHELLS = {"sh", "bash", "zsh", "dash", "ksh", "fish", "busybox", "pwsh", "powershell", "cmd"}
HOOK_INTERPRETERS = {"python", "python3", "node", "deno", "bun", "perl", "ruby", "php", "lua", "osascript"}
HOOK_FETCHERS = {"curl", "wget", "http", "https", "xh", "nc", "ncat", "socat", "ssh", "scp", "rsync", "aria2c", "iwr", "irm", "invoke-webrequest", "invoke-restmethod", "git"}
HOOK_DECODERS = {"base64", "xxd", "openssl", "gunzip", "zcat"}


def _command_name(word: str) -> str:
    """A program's base name, without `.exe` or a version such as `3.12`."""
    name = word.strip("'\"").replace("\\", "/").rsplit("/", 1)[-1].lower()
    name = name.removesuffix(".exe")
    return name if name in HOOK_DECODERS else re.sub(r"[0-9.]+$", "", name) or name


# Wrappers that run the command after them, with the options that take a
# value and the leading words that are not the command, as in doctor.
HOOK_WRAPPERS = {
    "builtin": (set(), 0),
    "busybox": (set(), 0),
    "caffeinate": ({"-w"}, 0),
    "chrt": (set(), 1),
    "command": (set(), 0),
    "doas": ({"-u", "-c"}, 0),
    "env": ({"-u", "-c", "-s", "--unset", "--chdir"}, 0),
    "exec": ({"-a"}, 0),
    "flock": ({"-w", "-e", "-s", "-x", "-n", "-c"}, 1),
    "ionice": ({"-c", "-n", "-p"}, 0),
    "nice": ({"-n"}, 0),
    "nohup": (set(), 0),
    "setsid": (set(), 0),
    "stdbuf": ({"-i", "-o", "-e"}, 0),
    "sudo": ({"-u", "-g", "-h", "-p", "-r", "-t", "-c", "-d"}, 0),
    "time": ({"-f", "-o"}, 0),
    "timeout": ({"-s", "-k", "--signal", "--kill-after"}, 1),
    "unbuffer": (set(), 0),
    "xargs": ({"-a", "-d", "-e", "-l", "-n", "-p", "-s", "-i"}, 0),
}


def _strip_comments(text: str) -> str:
    """Drop shell comments: an unquoted `#` at the start of a word, to the end of the line."""
    out: list[str] = []
    quote = None
    start = True
    index = 0
    while index < len(text):
        c = text[index]
        if quote is None and c == "#" and start:
            end = text.find("\n", index)
            if end < 0:
                break
            index = end
            continue
        if c == "\\" and quote != "'":
            out.append(text[index : index + 2])
            index += 2
            start = False
            continue
        if quote is None and c in "'\"":
            quote = c
        elif quote == c:
            quote = None
        out.append(c)
        start = quote is None and (c.isspace() or c in ";&|()")
        index += 1
    return "".join(out)


def _normalise_hook(text: str) -> str:
    """Hook text as doctor reads it: no comments, quotes, backslashes or `$IFS`,
    simple `NAME=value` variables substituted, lower case."""
    text = _strip_comments(text).replace("\\\n", "")
    text = re.sub(r"[\\'\"]", "", text).replace("\n", ";").replace("\r", ";")
    text = text.replace("${IFS}", " ").replace("$IFS", " ")
    values: dict[str, str] = {}
    for name, value in re.findall(r"(?:^|[\s;&|()`])([A-Za-z_][A-Za-z0-9_]*)=([^\s;&|()`]+)", text):
        values.setdefault(name, value)
    for _ in range(2):
        for name, value in values.items():
            text = text.replace("${" + name + "}", value)
            text = re.sub(r"\$" + name + r"(?![A-Za-z0-9_])", lambda _match, value=value: value, text)
    return " ".join(text.split()).lower()


def _hook_script(text: str) -> list[list[list[str]]]:
    """Commands, each a pipeline of stages, each stage its words."""
    text = text.replace("${", "$")
    for old, new in ((">&", " > "), ("&>", " > "), ("<&", " < "), ("|&", "|"), ("&&", ";"), ("||", ";")):
        text = text.replace(old, new)
    text = text.replace("&", ";").replace(">", " > ").replace("<", " < ")
    script = []
    for command in text.split(";"):
        stages = [[word for word in re.split(r"[\s(){}]+", stage) if word] for stage in command.split("|")]
        stages = [stage for stage in stages if stage]
        if stages:
            script.append(stages)
    return script


def _hook_call(words: list[str]) -> tuple[str, str, list[str]] | None:
    """The program word, its name and its arguments after wrappers and assignments."""
    assignment = re.compile(r"[a-z0-9_]+=.*")
    index = 0
    for _ in range(8):
        while index < len(words) and assignment.fullmatch(words[index]):
            index += 1
        if index >= len(words):
            return None
        name = _command_name(words[index])
        if name not in HOOK_WRAPPERS:
            return words[index], name, words[index + 1 :]
        values, skip = HOOK_WRAPPERS[name]
        index += 1
        skipped = 0
        while index < len(words):
            arg = words[index]
            if assignment.fullmatch(arg):
                index += 1
            elif arg.startswith("-") and len(arg) > 1:
                index += 2 if arg in values else 1
            elif skipped < skip:
                skipped += 1
                index += 1
            else:
                break
    return None


def _base(path: str) -> str:
    return path.rstrip("/").rsplit("/", 1)[-1]


def _written_files(name: str, args: list[str]) -> set[str]:
    """Files a call writes: `>` targets, `-o` and `--output` values, `tee` files,
    the `scp` and `rsync` target, and the file plain `wget` saves."""
    files: set[str] = set()
    explicit = False
    for index, arg in enumerate(args):
        following = args[index + 1] if index + 1 < len(args) else None
        short_output = name in {"curl", "wget"} and re.fullmatch(r"-[a-z]*o", arg)
        if following and (arg in {">", "--output", "--output-document"} or short_output):
            files.add(_base(following))
            explicit = True
        elif arg.startswith(("--output=", "--output-document=")):
            files.add(_base(arg.split("=", 1)[1]))
            explicit = True
    positional = [arg for arg in args if not arg.startswith("-")]
    if name == "tee":
        files.update(_base(arg) for arg in positional)
    if name in {"scp", "rsync"} and positional:
        files.add(_base(positional[-1]))
    if name == "wget" and not explicit:
        files.update(_base(arg.split("?", 1)[0]) for arg in positional if "://" in arg)
    return {file for file in files if file and file not in {"-", ".", "/dev/null"}}


def _runs_written_file(word: str, name: str, args: list[str], files: set[str]) -> bool:
    if _base(word) in files:
        return True
    if name not in HOOK_SHELLS | HOOK_INTERPRETERS | {"source", "."}:
        return False
    positional = [arg for arg in args if not arg.startswith("-")]
    redirected = any(args[index] == "<" and _base(args[index + 1]) in files for index in range(len(args) - 1))
    return bool(positional and _base(positional[0]) in files) or redirected


def _fetch_and_exec(command: str) -> str | None:
    """Why a hook command runs code it fetched, wrote or decoded, if it does."""
    command = _normalise_hook(command)
    words = {_command_name(word) for word in re.split(r"[\s;&|()`$<>]+", command) if word}
    written: set[str] = set()
    for pipeline in _hook_script(command):
        calls = [call for call in (_hook_call(stage) for stage in pipeline) if call]
        for index, (word, name, args) in enumerate(calls):
            if index > 0 and name in HOOK_SHELLS | HOOK_INTERPRETERS:
                return f"pipes {calls[index - 1][1]} into {name}"
            if written and _runs_written_file(word, name, args, written):
                return f"runs {name}, which a command before it wrote"
            written |= _written_files(name, args)
    if re.search(r"<\(|<<<", command) and words & (HOOK_SHELLS | HOOK_INTERPRETERS | {"source", "."}):
        return "runs a process substitution or here-string as code"
    if words & {"eval", "iex", "invoke-expression"} and words & (HOOK_FETCHERS | HOOK_DECODERS):
        return "evaluates fetched or decoded text"
    if re.search(r"(-c|-e|--eval)\s+\S*\$\(", command) and words & HOOK_FETCHERS:
        return "runs a command substitution as code"
    if re.search(r"(exec|eval|function)\s*\(", command) and re.search(r"b64decode|base64|atob\(|frombase64string|urlopen|fetch\(", command, re.I):
        return "runs decoded or downloaded code inline"
    return None


def _frontmatter_hooks(hooks: object) -> tuple[list[str], list[str], int]:
    """Command texts in a frontmatter `hooks:` mapping, the MCP servers other
    than mmcg its `mcp_tool` hooks call, and the malformed entry count."""
    if not isinstance(hooks, dict):
        return [], [], 1
    commands: list[str] = []
    servers: list[str] = []
    malformed = 0
    for groups in hooks.values():
        if not isinstance(groups, list):
            malformed += 1
            continue
        for group in groups:
            handlers = group.get("hooks") if isinstance(group, dict) else None
            if not isinstance(handlers, list):
                malformed += 1
                continue
            for handler in handlers:
                if not isinstance(handler, dict) or handler.get("type") not in {"command", "http", "mcp_tool", "prompt", "agent"}:
                    malformed += 1
                    continue
                if handler.get("type") == "mcp_tool":
                    # An MCP tool hook names a server and a tool:
                    # <https://code.claude.com/docs/en/hooks#mcp-tool-hook-fields>.
                    server, tool = handler.get("server"), handler.get("tool")
                    if not isinstance(server, str) or not isinstance(tool, str):
                        malformed += 1
                    elif server != "mmcg":
                        servers.append(server)
                    continue
                if handler.get("type") != "command":
                    continue
                command = handler.get("command")
                args = handler.get("args", [])
                if not isinstance(command, str) or not isinstance(args, list) or not all(isinstance(arg, str) for arg in args):
                    malformed += 1
                    continue
                commands.append(" ".join([command, *args]))
    return commands, servers, malformed


def _capability_messages(
    workflow: dict, runtime_tools: list[str], frontmatter: dict | None = None
) -> list[str]:
    capabilities = workflow.get("capabilities", {})
    if not isinstance(capabilities, dict):
        return ["[workflow_capability_invalid] workflow capabilities must be a mapping of capability to reason"]
    frontmatter = frontmatter or {}
    messages: list[str] = []
    for capability, reason in capabilities.items():
        if capability not in WORKFLOW_CAPABILITY_TOOLS:
            messages.append(
                f"[workflow_capability_invalid] unknown capability {capability!r}, "
                f"use {', '.join(sorted(WORKFLOW_CAPABILITY_TOOLS))}"
            )
        elif problem := _reason_problem(reason):
            messages.append(
                f"[workflow_capability_invalid] capability {capability!r} {problem} that says what the role runs"
            )
    reasons = [
        " ".join(reason.lower().split())
        for reason in capabilities.values()
        if isinstance(reason, str)
    ]
    if len(reasons) != len(set(reasons)):
        messages.append(
            "[workflow_capability_invalid] each capability needs its own reason, not one copied across capabilities"
        )
    # Claude Code removes `disallowedTools` from the granted set:
    # <https://code.claude.com/docs/en/sub-agents#supported-frontmatter-fields>.
    disallowed = {_tool_base(tool) for tool in _runtime_tool_names(frontmatter.get("disallowedTools"))}
    tools = {tool for tool in runtime_tools if _tool_base(tool) not in disallowed}
    bases = {_tool_base(tool) for tool in tools}
    role = workflow_capability_class(workflow)
    read_only = workflow.get("mutability") == "read-only"
    if read_only:
        mutation = sorted(WORKFLOW_MUTATION_TOOLS & bases)
        if mutation:
            messages.append(
                f"[readonly_mutation_capability] {role} role grants {', '.join(mutation)}"
            )
        side_effects = sorted(
            base
            for base in bases
            if _tool_capability(base) in WORKFLOW_READ_ONLY_DENIED_CAPABILITIES
        )
        if side_effects:
            messages.append(
                f"[readonly_side_effect_tool] {role} role grants {', '.join(side_effects)}, "
                "which publish, send or change the session"
            )
        unknown = sorted(
            base
            for base in bases
            if base not in WORKFLOW_READ_TOOLS
            and base not in WORKFLOW_MUTATION_TOOLS
            and _tool_capability(base) is None
            and not base.startswith("mcp__mmcg__")
        )
        if unknown:
            messages.append(
                f"[readonly_unknown_tool] {role} role grants {', '.join(unknown)}, "
                "which is not a known read tool; classify it in scripts/validate.py first"
            )
        for capability in sorted(WORKFLOW_READ_ONLY_DENIED_CAPABILITIES & set(capabilities)):
            messages.append(
                f"[readonly_side_effect_tool] {role} role declares {capability!r}, "
                "which a read-only role can never hold"
            )
        mode = frontmatter.get("permissionMode")
        if mode in WORKFLOW_READ_ONLY_FORBIDDEN_MODES:
            messages.append(
                f"[readonly_permission_mode] {role} role asks for permissionMode {mode!r}, "
                "which approves edits or every tool"
            )
    elif workflow.get("mutability") == "writer" and runtime_tools and not WORKFLOW_MUTATION_TOOLS & bases:
        messages.append(
            f"[writer_without_edit_tools] {role} role grants no Edit, Write, NotebookEdit or MultiEdit; "
            "declare it read-only and say in the shell reason what its commands write"
        )
    granted_by_capability: dict[str, list[str]] = {}
    for tool in sorted(tools):
        capability = _tool_capability(tool)
        if capability:
            granted_by_capability.setdefault(capability, []).append(tool)
    if "hooks" in frontmatter:
        hook_commands, hook_servers, malformed = _frontmatter_hooks(frontmatter.get("hooks"))
        granted_by_capability["hooks"] = ["frontmatter hooks"]
        for server in hook_servers:
            granted_by_capability.setdefault("mcp", []).append(f"an mcp_tool hook on {server}")
        if malformed:
            messages.append(
                f"[subagent_hook_invalid] {malformed} frontmatter hook entries are not command or http handlers"
            )
        for command in hook_commands:
            if reason := _fetch_and_exec(command):
                messages.append(f"[subagent_hook_fetch_exec] frontmatter hook {reason}")
    for capability in sorted(WORKFLOW_CAPABILITY_TOOLS):
        granted = granted_by_capability.get(capability, [])
        if granted and capability not in capabilities:
            messages.append(
                f"[undeclared_capability] {role} role grants {', '.join(granted)} without "
                f"declaring workflow capability {capability!r} and its reason"
            )
        # A role that declares a capability it no longer uses keeps a stale
        # class. Skills pass no runtime tools, so they cannot declare one.
        if capability in capabilities and not granted:
            messages.append(
                f"[unused_capability] workflow capability {capability!r} is declared but no tool needs it"
            )
    denied = WORKFLOW_DENIED_CAPABILITIES.get(str(frontmatter.get("name")), set())
    for capability in sorted(denied & (set(granted_by_capability) | set(capabilities))):
        messages.append(
            f"[denied_capability] {frontmatter.get('name')} must never get {capability!r}, "
            "even with a declared reason"
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
            "readonly_mutation_capability" in message
            for message in _workflow_metadata_messages(
                valid, required=True, runtime_tools=["Read", "NotebookEdit"]
            )
        ),
        any(
            "undeclared_capability" in message and "'shell'" in message
            for message in _workflow_metadata_messages(
                valid, required=True, runtime_tools=["Read", "Bash"]
            )
        ),
        not _workflow_metadata_messages(
            {"workflow": {**valid["workflow"], "capabilities": {"shell": "runs git diff to scope the review"}}},
            required=True,
            runtime_tools=["Read", "Bash"],
        ),
        any(
            "unused_capability" in message
            for message in _workflow_metadata_messages(
                {"workflow": {**valid["workflow"], "capabilities": {"web": "reads the release notes it cites"}}},
                required=True,
                runtime_tools=["Read"],
            )
        ),
        any(
            "workflow_capability_invalid" in message
            for message in _workflow_metadata_messages(
                {"workflow": {**valid["workflow"], "capabilities": {"shell": "git"}}},
                required=True,
                runtime_tools=["Read", "Bash"],
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
    guide = REPO_ROOT / "docs/integrations/generic-mcp.md"
    try:
        guide_text = guide.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(guide, "error", f"cannot read MCP integration guide: {error}"))
    else:
        if not re.search(r"\]\(\.\./reference/mcp\.md#mcp-tools\)", guide_text):
            issues.append(Issue(guide, "error", "MCP integration guide must link to the canonical tool catalog"))
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
    if tools[-1] != MMCG_EXPECTED_LAST_TOOL:
        issues.append(
            Issue(
                src_path,
                "error",
                f"authoritative tool order drift — expected final tool "
                f"`{MMCG_EXPECTED_LAST_TOOL}`, got `{tools[-1]}`",
            )
        )
    source = src_path.read_text()
    start = source.find("static TOOLS:")
    end = source.find("];", start) + 2 if start != -1 else -1
    registry = source[start:end] if start != -1 and end != -1 else ""
    for constructor, expected in MMCG_EXPECTED_BEHAVIOR_COUNTS.items():
        actual = len(re.findall(rf"\b{constructor}\s*\(", registry))
        if actual != expected:
            issues.append(
                Issue(
                    src_path,
                    "error",
                    f"tool behavior drift — expected {expected} `{constructor}` entries, got {actual}",
                )
            )

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

    for rel, role in MMCG_BRIEF_ROLE_DOCS.items():
        path = REPO_ROOT / rel
        if not path.is_file():
            issues.append(Issue(path, "error", "missing role-entry brief instruction"))
            continue
        text = path.read_text()
        required = ["mmcg_brief", f"role: {role}", "budget_tokens"]
        missing = [value for value in required if value not in text]
        if missing:
            issues.append(
                Issue(
                    path,
                    "error",
                    "bounded role-entry brief contract missing " + ", ".join(missing),
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
    fixtures_dir = REPO_ROOT / FIXTURES_RELATIVE_PATH
    if not fixtures_dir.is_dir():
        return [Issue(fixtures_dir, "error", "eval fixture inventory is missing")]
    scanned_files = 0
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
                except OSError as error:
                    issues.append(Issue(src_file, "error", f"cannot read eval fixture: {error}"))
                    continue
                scanned_files += 1
                text_lower = text.lower()
                for clue in BANNED_FIXTURE_CLUES:
                    if clue.lower() in text_lower:
                        issues.append(Issue(
                            src_file,
                            "error",
                            f"eval fixture contains banned clue {clue!r} — move explanation to the fixture README.md",
                        ))
                        break
    if not scanned_files:
        issues.append(Issue(
            fixtures_dir, "error", "eval fixture inventory has no baseline/changes source files",
        ))
    return issues


def validate_workflow_eval_contract() -> list[Issue]:
    """Keep planner/executor and product-skill regression coverage loadable."""
    path = REPO_ROOT / "evals/behavior/cases/workflow.jsonl"
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
    runner_path = REPO_ROOT / "evals/behavior/runner.py"
    try:
        runner = runner_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(runner_path, "error", f"cannot read workflow eval runner: {error}"))
    else:
        for token in (
            "WORKFLOW_ARTIFACTS",
            '"--safe-mode"',
            "TemporaryDirectory",
            "requires_prompt_sandbox",
        ):
            if token not in runner:
                issues.append(Issue(runner_path, "error", f"workflow eval sandbox missing {token!r}"))
        if re.search(r'["\']--tools["\']\s*,\s*["\']["\']', runner) is None:
            issues.append(
                Issue(
                    runner_path,
                    "error",
                    "workflow eval sandbox does not disable tools",
                )
            )
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
        if path == manifest_path:
            properties = schema.get("properties", {})
            graph = schema.get("$defs", {}).get("document_graph", {})
            graph_change = schema.get("$defs", {}).get("document_graph_change", {})
            analysis = properties.get("analysis", {}).get("properties", {})
            states = analysis.get("states", {}).get("items", {}).get("properties", {})
            if properties.get("document_graph", {}).get("$ref") != "#/$defs/document_graph":
                issues.append(Issue(path, "error", "review manifest must define optional document_graph binding"))
            if graph.get("additionalProperties") is not False or graph.get("properties", {}).get("relation_verification", {}).get("const") != "unverified":
                issues.append(Issue(path, "error", "document graph binding must stay strict and semantically unverified"))
            graph_fields = graph.get("properties", {})
            if not {"corpus_directories", "endpoint_changed_files", "corpus_changed_files"} <= set(graph.get("required", [])):
                issues.append(Issue(path, "error", "document graph binding must preserve scope and changed paths"))
            if any(graph_fields.get(name, {}).get("items", {}).get("$ref") != "#/$defs/document_graph_change"
                   for name in ("endpoint_changed_files", "corpus_changed_files")):
                issues.append(Issue(path, "error", "document graph changed paths must use the strict change contract"))
            if graph_change.get("additionalProperties") is not False or set(graph_change.get("required", [])) != {"path", "reasons"}:
                issues.append(Issue(path, "error", "document graph change records must be strict"))
            if set(analysis.get("document_graph_status", {}).get("enum", [])) != {"current", "needs_review"}:
                issues.append(Issue(path, "error", "review analysis must expose document graph freshness"))
            if "needs_review" not in states.get("state", {}).get("enum", []):
                issues.append(Issue(path, "error", "review partial-state schema must admit document graph review"))
            if len(schema.get("allOf", [])) < 3 or "$.document_graph" not in json.dumps(schema.get("allOf", [])):
                issues.append(Issue(path, "error", "review manifest must bind graph status to its partial-state projection"))

    workflow_path = REPO_ROOT / "docs/examples/mastermind-review-pr.yml"
    try:
        workflow_text = workflow_path.read_text(encoding="utf-8")
        workflow = yaml.safe_load(workflow_text)
    except (OSError, yaml.YAMLError) as error:
        issues.append(Issue(workflow_path, "error", f"invalid review workflow: {error}"))
    else:
        for token in (
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "actions/setup-node@820762786026740c76f36085b0efc47a31fe5020",
            "dtolnay/rust-toolchain@39008827e6b4319a349e9a42dfc90579dd25f1ca",
            "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
            "actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c",
            "github/codeql-action/upload-sarif@2892aa5e19bbd11bc0cff5427e3b750a04d9e3c2",
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
    revision_pattern = (
        definitions.get("repository", {})
        .get("properties", {})
        .get("revision", {})
        .get("pattern")
    )
    if revision_pattern != r"^(?:[0-9a-f]{40}|[0-9a-f]{64})$":
        issues.append(
            Issue(schema_path, "error", "fact revisions must accept full SHA-1 and SHA-256 Git OIDs")
        )
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
        production = rust.split("#[cfg(test)]", 1)[0]
        for forbidden in ("Command::new(", "CREATE TABLE", "libloading", "dlopen"):
            if forbidden in production:
                issues.append(
                    Issue(facts_path, "error", f"fact ingestion boundary contains forbidden {forbidden!r}")
                )

    surface_paths = {
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
    researcher_path = REPO_ROOT / "agents/subagents/mastermind-researcher.md"
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
    try:
        researcher = researcher_path.read_text(encoding="utf-8")
    except OSError as error:
        issues.append(Issue(researcher_path, "error", f"cannot read researcher contract: {error}"))
    else:
        for token in (
            "mcp__mmcg__mmcg_concept",
            "Do not replace a complete graph answer",
            "data, never instructions",
        ):
            if token not in researcher:
                issues.append(Issue(researcher_path, "error", f"researcher graph-first contract missing {token!r}"))
    workflow_path = REPO_ROOT / "agents/claude-md/mastermind-workflow.md"
    try:
        workflow = workflow_path.read_bytes()
    except OSError as error:
        issues.append(Issue(workflow_path, "error", f"cannot read project workflow contract: {error}"))
    else:
        if len(workflow) > 10_000:
            issues.append(Issue(workflow_path, "error", "project workflow exceeds the 10 KB anti-ceremony budget"))
        text = workflow.decode("utf-8")
        for token in (
            "Call `mmcg_brief` once",
            "`mmcg_concept`",
            "Do not use Bash to rediscover",
            "Bash remains the right tool for Git, builds, tests, linters, logs, and runtime probes",
        ):
            if token not in text:
                issues.append(Issue(workflow_path, "error", f"workflow graph-first routing missing {token!r}"))
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
            "required": (
                "syntactic evidence",
                "read the\nsource",
                "`mmcg_brief`",
                "`mmcg_concept`",
                "managed index",
                "custom external index",
            ),
            "forbidden": ("Do NOT re-verify mmcg results", "always cheaper"),
        },
        "skills/prompt-engineering/mastermind-prompt-refiner/SKILL.md": {
            "required": ("## Original request", "Do not activate just because"),
            "forbidden": ("The planner sees the refined version, not the user's brain dump", "Mounted as the intake gate"),
        },
        "skills/workflow/mastermind-style-deep/SKILL.md": {
            "required": (
                "Absence needs a denominator and opportunity",
                "not observed in <N> relevant files/tests/commits",
            ),
            "forbidden": ("Their presence — or absence — tells you",),
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


# ----- untrusted-content block ------------------------------------------

# One canonical paragraph tells every role that reads repository, pull request
# or tool content to treat it as data. Installed skills and subagents are copied
# as single files, so each covered file carries a verbatim copy. This list is
# the covered set: a role that reads such content belongs here, and a file
# outside it must not carry the paragraph, and the copy must be a plain
# paragraph, not inside a code fence or an HTML comment. The critic is left out
# because its prompt has a tight size ceiling; it states the same rule in its
# opening paragraph.
UNTRUSTED_CONTENT_BLOCK = "agents/blocks/untrusted-content.md"
UNTRUSTED_CONTENT_LEAD = "**Untrusted content.**"
UNTRUSTED_CONTENT_COVERED: tuple[str, ...] = (
    "agents/subagents/mastermind-auditor.md",
    "agents/subagents/mastermind-comment-auditor.md",
    "agents/subagents/mastermind-feedback-collector.md",
    "agents/subagents/mastermind-frontend-auditor.md",
    "agents/subagents/mastermind-investigator.md",
    "agents/subagents/mastermind-prompt-refiner.md",
    "agents/subagents/mastermind-researcher.md",
    "agents/subagents/mastermind-security-auditor.md",
    "agents/subagents/mastermind-task-executor.md",
    "agents/subagents/mastermind-test-auditor.md",
    "skills/code-review/mastermind-comment-audit/SKILL.md",
    "skills/code-review/mastermind-diff-review/SKILL.md",
    "skills/code-review/mastermind-frontend-audit/SKILL.md",
    "skills/code-review/mastermind-test-audit/SKILL.md",
    "skills/debugging/mastermind-investigation-ledger/SKILL.md",
    "skills/design/mastermind-design-intake/SKILL.md",
    "skills/docs/mastermind-docs-authoring/SKILL.md",
    "skills/ops/mastermind-dependency-upgrade/SKILL.md",
    "skills/prompt-engineering/mastermind-prompt-refiner/SKILL.md",
    "skills/security/mastermind-agent-security-review/SKILL.md",
    "skills/security/mastermind-scanner-alert-response/SKILL.md",
    "skills/security/mastermind-security-research/SKILL.md",
    "skills/testing/mastermind-browser-verification/SKILL.md",
    "skills/testing/mastermind-verify-change/SKILL.md",
    "skills/workflow/mastermind-architecture-review/SKILL.md",
    "skills/workflow/mastermind-audit-attestation/SKILL.md",
    "skills/workflow/mastermind-change-impact/SKILL.md",
    "skills/workflow/mastermind-codegraph-research/SKILL.md",
    "skills/workflow/mastermind-component-research/SKILL.md",
    "skills/workflow/mastermind-critical-review/SKILL.md",
    "skills/workflow/mastermind-product-intake/SKILL.md",
    "skills/workflow/mastermind-project-history/SKILL.md",
    "skills/workflow/mastermind-project-map/SKILL.md",
    "skills/workflow/mastermind-runtime-research/SKILL.md",
    "skills/workflow/mastermind-style-deep/SKILL.md",
    "skills/workflow/mastermind-task-executor/SKILL.md",
    "skills/workflow/mastermind-task-planning/SKILL.md",
    "skills/workflow/mastermind-test-impact/SKILL.md",
)


def _untrusted_block_placement(text: str, block: str) -> str | None:
    """Say why the block would not be read as a plain paragraph, if it would not."""
    index = text.index(block)
    before, after = text[:index], text[index + len(block):]
    fences = sum(
        1 for line in before.splitlines() if line.lstrip().startswith(("```", "~~~"))
    )
    if fences % 2:
        return "sits inside a code fence"
    if before.rfind("<!--") > before.rfind("-->"):
        return "sits inside an HTML comment"
    if not before.endswith("\n\n") or not (after == "\n" or after.startswith("\n\n")):
        return "must be its own paragraph, with a blank line before and after"
    return None


def validate_untrusted_content_block() -> list[Issue]:
    issues: list[Issue] = []
    source = REPO_ROOT / UNTRUSTED_CONTENT_BLOCK
    try:
        block = source.read_text(encoding="utf-8").strip("\n")
    except OSError as error:
        return [Issue(source, "error", f"cannot read the canonical untrusted-content block: {error}")]
    if not block.startswith(UNTRUSTED_CONTENT_LEAD) or block.count(UNTRUSTED_CONTENT_LEAD) != 1:
        return [Issue(source, "error", f"canonical block must start with {UNTRUSTED_CONTENT_LEAD!r} once")]
    fix = f"copy it verbatim from {UNTRUSTED_CONTENT_BLOCK}"
    for relative in UNTRUSTED_CONTENT_COVERED:
        path = REPO_ROOT / relative
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as error:
            issues.append(Issue(path, "error", f"cannot read covered file: {error}"))
            continue
        exact = text.count(block)
        leads = text.count(UNTRUSTED_CONTENT_LEAD)
        if leads > exact:
            issues.append(Issue(path, "error", f"untrusted-content block has drifted; {fix}"))
        elif exact == 0:
            issues.append(Issue(path, "error", f"untrusted-content block missing; {fix}"))
        elif exact > 1:
            issues.append(Issue(path, "error", f"untrusted-content block appears {exact} times; keep one"))
        elif problem := _untrusted_block_placement(text, block):
            issues.append(Issue(path, "error", f"untrusted-content block {problem}"))
    covered = {REPO_ROOT / relative for relative in UNTRUSTED_CONTENT_COVERED}
    candidates = [
        *(REPO_ROOT / "skills").rglob("*.md"),
        *(REPO_ROOT / "agents/subagents").glob("*.md"),
        *(REPO_ROOT / "agents/claude-md").glob("*.md"),
    ]
    for path in sorted(candidates):
        if path in covered:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        if UNTRUSTED_CONTENT_LEAD in text:
            issues.append(
                Issue(path, "error", "carries the untrusted-content block but is not in UNTRUSTED_CONTENT_COVERED")
            )
    return issues


# ----- verifiable audit Action security contract -----------------------

AUDIT_ACTION_PINS = {
    "actions/checkout": "3d3c42e5aac5ba805825da76410c181273ba90b1",
    "actions/upload-artifact": "043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
    "actions/download-artifact": "3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c",
    "actions/attest": "1e69f48acb82d1966a394da916b4c1698aa569d6",
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


# A workflow that names its own toolchain drifts from rust-toolchain.toml on the next bump.
WORKFLOW_TOOLCHAIN_PIN = re.compile(
    r"dtolnay/rust-toolchain|rustup (?:toolchain install|default|override set)[ \t]+\w|cargo \+\w"
)


def validate_rust_toolchain_source() -> list[Issue]:
    """Keep rust-toolchain.toml the only place that chooses the Rust version."""
    toolchain_path = REPO_ROOT / "rust-toolchain.toml"
    docker_path = REPO_ROOT / "Dockerfile.audit-action"
    try:
        toolchain_text = toolchain_path.read_text(encoding="utf-8")
        docker_text = docker_path.read_text(encoding="utf-8")
    except OSError as error:
        return [Issue(toolchain_path, "error", f"cannot read Rust toolchain sources: {error}")]
    match = re.search(r'(?m)^channel = "((\d+\.\d+)\.\d+)"$', toolchain_text)
    if match is None:
        return [Issue(toolchain_path, "error", 'rust-toolchain.toml must pin channel = "X.Y.Z"')]
    channel, minor = match.groups()

    issues: list[Issue] = []
    images = dockerfile_from_images(docker_text)
    if not images or not images[0].startswith(f"rust:{minor}-") or f"cargo +{channel} build" not in docker_text:
        issues.append(Issue(
            docker_path, "error",
            f"Docker build must use rust:{minor} and `cargo +{channel} build` to match rust-toolchain.toml",
        ))
    for path in sorted((REPO_ROOT / ".github/workflows").glob("*.y*ml")):
        if WORKFLOW_TOOLCHAIN_PIN.search(path.read_text(encoding="utf-8")):
            issues.append(Issue(
                path, "error",
                "install Rust with `rustup toolchain install` so rust-toolchain.toml stays the only version",
            ))
    return issues


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
                issues.append(Issue(path, "error", "examples are copied into other repositories, so they must pin xcrft/mastermind instead of `uses: ./`"))
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
        if not re.search(r"uses:\s*xcrft/mastermind@[0-9a-f]{40}\b", text):
            issues.append(Issue(pr_path, "error", "PR audit must run the Mastermind Action pinned to a full commit SHA"))
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
            "rust:1.100-bookworm@sha256:ade39a710ef85f3506cffcfec3e8a24ba090badef1dff2fb798f71d878833fea",
            "buildpack-deps:bookworm-scm@sha256:5bac6b3020afcf0358de87990605d5dc4c3f060629381912266b9b9278375af3",
        ]
        actual_from = dockerfile_from_images(docker_text)
        if actual_from != expected_from:
            issues.append(Issue(docker_path, "error", "Docker stages must use the two audited immutable OCI digests"))
        if "--locked" not in docker_text:
            issues.append(Issue(docker_path, "error", "Docker Action must build with the Cargo lockfile"))
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
        if 'case "${#1}" in 40|64)' not in entrypoint:
            issues.append(
                Issue(
                    entrypoint_path,
                    "error",
                    "Action entrypoint must accept full SHA-1 and SHA-256 Git OIDs",
                )
            )
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
    issues.extend(validate_untrusted_content_block())
    issues.extend(validate_description_referrals(artifacts))

    links = collect_wikilinks()
    issues.extend(validate_wikilinks(artifacts, links))

    rel_links = collect_relative_links()
    issues.extend(validate_relative_links(rel_links))
    issues.extend(validate_installable_link_escape(rel_links))
    issues.extend(validate_release_versions())
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
    issues.extend(validate_rust_toolchain_source())
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
