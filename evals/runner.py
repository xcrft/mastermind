#!/usr/bin/env python3
"""
Local eval runner for mastermind subagents.

Uses the `claude` CLI in non-interactive mode (`-p`), so authentication runs
through your existing Claude Code login — no ANTHROPIC_API_KEY needed, costs
count against your Claude subscription (flat monthly, not per-token).

Auditor cases use **real git fixtures** — each case names a fixture under
`evals/fixtures/<name>/` plus `baseline_ref` and an `after_ref` tree variant.
By default both trees become tagged commits. Cases with `staged_paths` leave
the after-tree uncommitted and stage only the listed paths. The runner hands
the disposable repository to the auditor via `--add-dir`; the auditor inspects
real diffs and untracked files. No synthetic paraphrased diff strings.

Usage:
  python evals/runner.py                  # all suites
  python evals/runner.py --suite critic   # one suite
  python evals/runner.py --case c-001     # one case
  python evals/runner.py --model opus     # model alias or full name (default: opus)
  python evals/runner.py --keep-fixtures  # don't tmp-cleanup (debugging)

Prerequisites:
  - `claude` CLI on PATH (Claude Code installed, logged in)
  - `git` on PATH
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

if __package__:
    from .benchmark_process import run_bounded
    from .evidence import answer_lines, check_citations
else:
    from benchmark_process import run_bounded
    from evidence import answer_lines, check_citations

try:
    import yaml as _yaml
    _YAML_AVAILABLE = True
except ImportError:
    _YAML_AVAILABLE = False

REPO_ROOT = Path(__file__).resolve().parent.parent
EVALS_DIR = Path(__file__).resolve().parent
FIXTURES_DIR = EVALS_DIR / "fixtures"
REPORT_KIND = "mastermind-eval-report"
REPORT_SCHEMA_VERSION = 1
CRITIC_VERDICTS = frozenset(
    {"ship it", "ship with caveats", "revise", "rethink", "insufficient evidence"}
)
AUDIT_VERDICTS = frozenset({"held", "drift", "broken"})
INTAKE_ACTIONS = frozenset({"refined", "ask", "passthrough"})

# Prefer the in-tree build (matches the current SCHEMA_VERSION) over whatever
# version is installed in ~/.cargo/bin — avoids "schema mismatch — rebuilding"
# noise during eval and ensures the indexer + server speak the same SQL.
_LOCAL_MMCG = REPO_ROOT / "mcp" / "servers" / "mmcg" / "target" / "release" / "mmcg"
MMCG_BIN = str(_LOCAL_MMCG) if _LOCAL_MMCG.is_file() else "mmcg"

SUITES = {
    "critic": {
        "subagent": REPO_ROOT / "agents/subagents/mastermind-critic.md",
        "cases": EVALS_DIR / "critic.jsonl",
        "renderer": "render_critic_input",
        "uses_fixture": False,
    },
    "researcher": {
        "subagent": REPO_ROOT / "agents/subagents/mastermind-researcher.md",
        "cases": EVALS_DIR / "researcher.jsonl",
        "renderer": "render_researcher_input",
        "uses_fixture": True,
    },
    "auditor": {
        "subagent": REPO_ROOT / "agents/subagents/mastermind-auditor.md",
        "cases": EVALS_DIR / "auditor.jsonl",
        "renderer": "render_auditor_input",
        "uses_fixture": True,
    },
    "intake": {
        "subagent": REPO_ROOT / "agents/subagents/mastermind-prompt-refiner.md",
        "cases": EVALS_DIR / "intake.jsonl",
        "renderer": "render_intake_input",
        "uses_fixture": False,
    },
    "workflow": {
        "subagent": None,
        "cases": EVALS_DIR / "workflow.jsonl",
        "renderer": "render_workflow_input",
        "uses_fixture": False,
    },
}

SUITE_RUNTIME_LIMITS = {
    "critic": {"max_turns": 10, "max_output_tokens": 8192},
    "researcher": {"max_turns": 12, "max_output_tokens": 2200},
    "auditor": {"max_turns": 20, "max_output_tokens": 8192},
    "intake": {"max_turns": 4, "max_output_tokens": 4096},
    "workflow": {"max_turns": 12, "max_output_tokens": 4096},
}
SUITE_DEFAULT_EFFORT = {"workflow": "medium"}
CLAUDE_EFFORT_LEVELS = frozenset({"low", "medium", "high", "xhigh", "max"})
CLAUDE_CASE_TIMEOUT_SECONDS = 480
CLAUDE_STDOUT_LIMIT_BYTES = 8 * 1024 * 1024
CLAUDE_STDERR_LIMIT_BYTES = 64 * 1024

WORKFLOW_ARTIFACTS = frozenset(
    {
        "skills/code-review/mastermind-comment-audit/SKILL.md",
        "skills/code-review/mastermind-frontend-audit/SKILL.md",
        "skills/code-review/mastermind-test-audit/SKILL.md",
        "skills/coding/no-ai-slop-comments/SKILL.md",
        "skills/design/mastermind-design-intake/SKILL.md",
        "skills/testing/mastermind-browser-verification/SKILL.md",
        "skills/debugging/mastermind-investigation-ledger/SKILL.md",
        "skills/prompt-engineering/mastermind-prompt-refiner/SKILL.md",
        "skills/security/mastermind-agent-security-review/SKILL.md",
        "skills/security/mastermind-security-research/SKILL.md",
        "skills/workflow/mastermind-audit-attestation/SKILL.md",
        "skills/workflow/mastermind-architecture-review/SKILL.md",
        "skills/workflow/mastermind-change-impact/SKILL.md",
        "skills/workflow/mastermind-codegraph-research/SKILL.md",
        "skills/workflow/mastermind-component-research/SKILL.md",
        "skills/workflow/mastermind-critical-review/SKILL.md",
        "skills/workflow/mastermind-cross-client-setup/SKILL.md",
        "skills/workflow/mastermind-project-history/SKILL.md",
        "skills/workflow/mastermind-product-intake/SKILL.md",
        "skills/workflow/mastermind-project-map/SKILL.md",
        "skills/workflow/mastermind-runtime-research/SKILL.md",
        "skills/workflow/mastermind-structured-report-contract/SKILL.md",
        "skills/workflow/mastermind-style-deep/SKILL.md",
        "skills/workflow/mastermind-task-executor/SKILL.md",
        "skills/workflow/mastermind-task-planning/SKILL.md",
        "skills/workflow/mastermind-test-impact/SKILL.md",
    }
)

# Deterministic identity for fixture commits — avoids machine-specific git
# config noise in eval reproducibility.
GIT_ENV = {
    "GIT_AUTHOR_NAME": "mmcg-eval",
    "GIT_AUTHOR_EMAIL": "eval@mastermind.local",
    "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
    "GIT_COMMITTER_NAME": "mmcg-eval",
    "GIT_COMMITTER_EMAIL": "eval@mastermind.local",
    "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
    "GIT_ATTR_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_NOSYSTEM": "1",
    # Avoid hooks and GPG signing inside fixtures even if repository-local
    # configuration changes while a case is running.
    "GIT_CONFIG_COUNT": "2",
    "GIT_CONFIG_KEY_0": "commit.gpgsign",
    "GIT_CONFIG_VALUE_0": "false",
    "GIT_CONFIG_KEY_1": "core.hooksPath",
    "GIT_CONFIG_VALUE_1": os.devnull,
    "GIT_OPTIONAL_LOCKS": "0",
    "GIT_PAGER": "",
    "GIT_TERMINAL_PROMPT": "0",
}

# Base env for all subprocesses spawned by the runner.
# TERM=dumb prevents tput from emitting warnings in non-interactive shells
# (CI, piped output, shells where $TERM is unset). Prefer the caller's TERM
# when available so color hints still work in interactive mode.
_PROC_ENV: dict[str, str] = {**os.environ, "TERM": os.environ.get("TERM") or "dumb"}
_CLAUDE_AUTH_ENV = frozenset({"CLAUDE_CODE_OAUTH_TOKEN"})
_CLAUDE_GENERIC_RUNTIME_ENV = frozenset(
    {
        "API_FORCE_IDLE_TIMEOUT",
        "API_TIMEOUT_MS",
        "BASH_DEFAULT_TIMEOUT_MS",
        "BASH_MAX_OUTPUT_LENGTH",
        "BASH_MAX_TIMEOUT_MS",
        "BETA_TRACING_ENDPOINT",
        "DEBUG",
        "DISABLE_COMPACT",
        "ENABLE_TOOL_SEARCH",
        "FORCE_AUTOUPDATE_PLUGINS",
        "MAX_MCP_OUTPUT_TOKENS",
        "MAX_STRUCTURED_OUTPUT_RETRIES",
        "MAX_THINKING_TOKENS",
        "MCP_CONNECTION_NONBLOCKING",
        "MCP_DISCOVERY_CACHE",
        "MCP_REMOTE_SERVER_CONNECTION_BATCH_SIZE",
        "MCP_SERVER_CONNECTION_BATCH_SIZE",
        "MCP_TIMEOUT",
        "MCP_TOOL_TIMEOUT",
    }
)

AUDITOR_SAFE_ALLOWED_TOOLS = (
    "Read",
    "Glob",
    "Grep",
    "Bash(git diff)",
    "Bash(git diff *)",
    "Bash(git log)",
    "Bash(git log *)",
    "Bash(git show *)",
    "Bash(git status)",
    "Bash(git status *)",
    "Bash(git rev-parse *)",
    "Bash(git ls-files)",
    "Bash(git ls-files *)",
    "Bash(git grep *)",
    "Bash(cargo test --locked *)",
)


@dataclass
class Result:
    case_id: str
    suite: str
    passed: bool
    reasons: list[str] = field(default_factory=list)
    duration_ms: int = 0
    fixture_path: Path | None = None
    retry_used: bool = False
    retry_attempted: bool = False
    output_excerpt: str = ""
    duration_api_ms: int = 0
    num_turns: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    cache_creation_input_tokens: int = 0
    cache_read_input_tokens: int = 0
    cost_usd: float = 0.0
    telemetry_complete: bool = False
    telemetry_issues: list[str] = field(default_factory=list)
    resolved_models: list[str] = field(default_factory=list)
    tool_calls: list[str] = field(default_factory=list)
    citation_checks: dict | None = None

    @property
    def context_tokens(self) -> int:
        return (
            self.input_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
        )

    @property
    def total_tokens(self) -> int:
        return self.context_tokens + self.output_tokens

    def add_attempt(self, prior: "Result") -> None:
        """Aggregate usage from an earlier attempt into this final result."""
        self.duration_ms += prior.duration_ms
        self.duration_api_ms += prior.duration_api_ms
        self.num_turns += prior.num_turns
        self.input_tokens += prior.input_tokens
        self.output_tokens += prior.output_tokens
        self.cache_creation_input_tokens += prior.cache_creation_input_tokens
        self.cache_read_input_tokens += prior.cache_read_input_tokens
        self.cost_usd += prior.cost_usd
        self.telemetry_complete = self.telemetry_complete and prior.telemetry_complete
        self.telemetry_issues = [*prior.telemetry_issues, *self.telemetry_issues]
        self.resolved_models = list(
            dict.fromkeys([*prior.resolved_models, *self.resolved_models])
        )
        self.tool_calls = [*prior.tool_calls, *self.tool_calls]


@dataclass
class ToolExecution:
    name: str
    tool_use_id: str
    arguments: dict
    result_seen: bool = False
    succeeded: bool = False


@dataclass(frozen=True)
class StreamRuntimeContract:
    cwd: str
    tools: tuple[str, ...]
    mcp_servers: tuple[str, ...]
    claude_code_version: str


def strip_frontmatter(text: str) -> str:
    if text.startswith("---\n"):
        end = text.find("\n---\n", 4)
        if end != -1:
            return text[end + 5 :].lstrip()
    return text


def subagent_runtime_definition(
    path: Path,
    *,
    model_override: str | None = None,
    include_mmcg: bool = True,
) -> tuple[str, dict]:
    """Translate shipped YAML frontmatter into Claude's `--agents` contract."""
    if not _YAML_AVAILABLE:
        raise RuntimeError("PyYAML is required to load subagent frontmatter")
    text = path.read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        raise ValueError(f"subagent has no YAML frontmatter: {path}")
    end = text.find("\n---\n", 4)
    if end == -1:
        raise ValueError(f"subagent frontmatter is not terminated: {path}")
    loaded = _yaml.safe_load(text[4:end])
    if not isinstance(loaded, dict):
        raise ValueError(f"subagent frontmatter is not a mapping: {path}")
    name = loaded.get("name")
    description = loaded.get("description")
    if not isinstance(name, str) or not isinstance(description, str):
        raise ValueError(f"subagent name/description is invalid: {path}")

    definition: dict = {
        "description": description,
        "prompt": text[end + 5 :].lstrip(),
    }
    for key in (
        "tools",
        "disallowedTools",
        "model",
        "mcpServers",
        "permissionMode",
        "maxTurns",
        "skills",
        "hooks",
        "memory",
        "effort",
    ):
        if key in loaded:
            definition[key] = loaded[key]
    if isinstance(definition.get("tools"), str):
        definition["tools"] = [
            tool.strip()
            for tool in definition["tools"].split(",")
            if tool.strip()
        ]
    if not include_mmcg:
        tools = definition.get("tools", [])
        if not isinstance(tools, list):
            raise ValueError(f"subagent tools are invalid: {path}")
        definition["tools"] = [
            tool
            for tool in tools
            if not (isinstance(tool, str) and tool.startswith("mcp__mmcg__"))
        ]
        servers = definition.get("mcpServers", [])
        if isinstance(servers, list):
            remaining_servers: list | dict = [
                server for server in servers if server != "mmcg"
            ]
        elif isinstance(servers, dict):
            remaining_servers = {
                server: config
                for server, config in servers.items()
                if server != "mmcg"
            }
        else:
            raise ValueError(f"subagent MCP servers are invalid: {path}")
        if remaining_servers:
            definition["mcpServers"] = remaining_servers
        else:
            definition.pop("mcpServers", None)
    if model_override is not None:
        definition["model"] = model_override
    return name, definition


def subagent_cli_args(
    path: Path, *, model_override: str, include_mmcg: bool = True
) -> list[str]:
    """Run the eval through the same custom-agent boundary as production."""
    name, definition = subagent_runtime_definition(
        path,
        model_override=model_override,
        include_mmcg=include_mmcg,
    )
    return [
        "--agents",
        json.dumps({name: definition}, sort_keys=True),
        "--agent",
        name,
    ]


def subagent_mcp_tools(path: Path) -> tuple[str, ...]:
    _, definition = subagent_runtime_definition(path)
    tools = definition.get("tools", [])
    if not isinstance(tools, list):
        return ()
    return tuple(
        tool
        for tool in tools
        if isinstance(tool, str) and tool.startswith("mcp__mmcg__")
    )


def auditor_allowed_tools(
    subagent: Path | None = None, *, include_mmcg: bool = True
) -> tuple[str, ...]:
    mcp_tools = (
        subagent_mcp_tools(
            SUITES["auditor"]["subagent"] if subagent is None else subagent
        )
        if include_mmcg
        else ()
    )
    return AUDITOR_SAFE_ALLOWED_TOOLS + mcp_tools


def researcher_allowed_tools(
    subagent: Path | None = None, *, include_mmcg: bool = True
) -> tuple[str, ...]:
    mcp_tools = (
        subagent_mcp_tools(
            SUITES["researcher"]["subagent"] if subagent is None else subagent
        )
        if include_mmcg
        else ()
    )
    return ("Read", "Glob", "Grep") + mcp_tools


def case_runtime_limits(suite_name: str, expect: dict) -> dict[str, int]:
    defaults = SUITE_RUNTIME_LIMITS.get(suite_name)
    if defaults is None:
        raise ValueError(f"suite {suite_name!r} has no runtime limits")
    limits = {
        field: expect.get(field, default)
        for field, default in defaults.items()
    }
    if any(
        isinstance(value, bool) or not isinstance(value, int) or value <= 0
        for value in limits.values()
    ):
        raise ValueError("evaluation runtime limits must be positive integers")
    return limits


def evaluation_effort(suite_name: str, prompt_path: Path | None) -> str:
    effort = SUITE_DEFAULT_EFFORT.get(suite_name)
    if effort is None and prompt_path is not None:
        _, definition = subagent_runtime_definition(prompt_path)
        effort = definition.get("effort")
    if effort not in CLAUDE_EFFORT_LEVELS:
        raise ValueError(f"suite {suite_name!r} has no valid evaluation effort")
    return effort


def evaluation_environment(
    max_output_tokens: int,
    *,
    source: dict[str, str] | None = None,
    pinned_executables: tuple[str | Path, ...] = (),
) -> dict[str, str]:
    if (
        isinstance(max_output_tokens, bool)
        or not isinstance(max_output_tokens, int)
        or max_output_tokens <= 0
    ):
        raise ValueError("maximum output tokens must be a positive integer")
    inherited = _PROC_ENV if source is None else source
    environment = {
        name: value
        for name, value in inherited.items()
        if not (
            (name.startswith("ANTHROPIC_") and name not in _CLAUDE_AUTH_ENV)
            or (name.startswith("CLAUDE_") and name not in _CLAUDE_AUTH_ENV)
            or name == "CLAUDECODE"
            or name.startswith("DISABLE_")
            or name.startswith("GIT_")
            or name.startswith("MCP_")
            or name.startswith("OTEL_")
            or name in _CLAUDE_GENERIC_RUNTIME_ENV
        )
    }
    environment.update(
        {
            "API_TIMEOUT_MS": "300000",
            "BASH_DEFAULT_TIMEOUT_MS": "120000",
            "BASH_MAX_OUTPUT_LENGTH": "30000",
            "BASH_MAX_TIMEOUT_MS": "120000",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
            "CLAUDE_CODE_ENABLE_TASKS": "0",
            "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS": "0",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS": str(max_output_tokens),
            "CLAUDE_CODE_MAX_RETRIES": "2",
            "CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY": "1",
            "CLAUDE_CODE_MCP_ALLOWLIST_ENV": "1",
            "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT": "120000",
            "DISABLE_AUTOUPDATER": "1",
            "DISABLE_ERROR_REPORTING": "1",
            "DISABLE_GROWTHBOOK": "1",
            "DISABLE_TELEMETRY": "1",
            "ENABLE_TOOL_SEARCH": "false",
            "MAX_MCP_OUTPUT_TOKENS": "25000",
            "MCP_DISCOVERY_CACHE": "0",
            "MCP_TIMEOUT": "30000",
            "MCP_TOOL_TIMEOUT": "120000",
            **GIT_ENV,
        }
    )
    pinned_directories: list[str] = []
    for executable in pinned_executables:
        path = Path(executable)
        if not path.is_absolute():
            raise ValueError("pinned evaluation executables must be absolute paths")
        directory = str(path.parent)
        if directory not in pinned_directories:
            pinned_directories.append(directory)
    if pinned_directories:
        environment["PATH"] = os.pathsep.join(
            (*pinned_directories, environment.get("PATH", os.defpath))
        )
    return environment


def _git_environment(source: dict[str, str] | None = None) -> dict[str, str]:
    inherited = _PROC_ENV if source is None else source
    return {
        **{name: value for name, value in inherited.items() if not name.startswith("GIT_")},
        **GIT_ENV,
    }


def _nonnegative_int(value: object) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        return 0
    return max(0, value)


def _nonnegative_float(value: object) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return 0.0
    result = float(value)
    return result if math.isfinite(result) and result >= 0 else 0.0


def telemetry_from_payload(payload: dict) -> dict[str, object]:
    issues: list[str] = []

    def required_int(container: dict, key: str, label: str) -> int:
        value = container.get(key)
        if isinstance(value, bool) or not isinstance(value, int) or value < 0:
            issues.append(f"{label} must be a non-negative integer")
            return 0
        return value

    def required_float(container: dict, key: str, label: str) -> float:
        value = container.get(key)
        if (
            isinstance(value, bool)
            or not isinstance(value, (int, float))
            or not math.isfinite(float(value))
            or float(value) < 0
        ):
            issues.append(f"{label} must be a non-negative finite number")
            return 0.0
        return float(value)

    usage = payload.get("usage")
    if not isinstance(usage, dict):
        issues.append("usage must be an object")
        usage = {}
    model_usage = payload.get("modelUsage")
    valid_model_usage = not (
        not isinstance(model_usage, dict)
        or not model_usage
        or any(not isinstance(name, str) or not name for name in model_usage)
    )
    if not valid_model_usage:
        issues.append("modelUsage must identify at least one usage model")
    resolved_model = payload.get("_resolved_model")
    if not isinstance(resolved_model, str) or not resolved_model:
        issues.append("stream init must identify the resolved primary model")
        resolved_models: list[str] = []
    else:
        resolved_models = sorted(model_usage) if valid_model_usage else []
        if valid_model_usage and resolved_model not in model_usage:
            issues.append("stream primary model is absent from modelUsage")
    return {
        "duration_ms": required_int(payload, "duration_ms", "duration_ms"),
        "duration_api_ms": required_int(
            payload, "duration_api_ms", "duration_api_ms"
        ),
        "num_turns": required_int(payload, "num_turns", "num_turns"),
        "input_tokens": required_int(usage, "input_tokens", "usage.input_tokens"),
        "output_tokens": required_int(
            usage, "output_tokens", "usage.output_tokens"
        ),
        "cache_creation_input_tokens": required_int(
            usage,
            "cache_creation_input_tokens",
            "usage.cache_creation_input_tokens",
        ),
        "cache_read_input_tokens": required_int(
            usage,
            "cache_read_input_tokens",
            "usage.cache_read_input_tokens",
        ),
        "cost_usd": required_float(payload, "total_cost_usd", "total_cost_usd"),
        "resolved_models": resolved_models,
        "complete": not issues,
        "issues": issues,
    }


def validate_stream_runtime(
    init_event: dict, contract: StreamRuntimeContract
) -> None:
    if init_event.get("claude_code_version") != contract.claude_code_version:
        raise ValueError("Claude stream CLI version does not match the pinned runtime")
    if init_event.get("cwd") != contract.cwd:
        raise ValueError("Claude stream working directory does not match the case")
    if init_event.get("permissionMode") != "dontAsk":
        raise ValueError("Claude stream permission mode is not dontAsk")

    tools = init_event.get("tools")
    if (
        not isinstance(tools, list)
        or any(not isinstance(tool, str) or not tool for tool in tools)
        or len(tools) != len(set(tools))
        or sorted(tools) != sorted(contract.tools)
    ):
        raise ValueError("Claude stream tool inventory does not match the case")

    raw_servers = init_event.get("mcp_servers", [])
    if not isinstance(raw_servers, list) or any(
        not isinstance(server, dict)
        or not isinstance(server.get("name"), str)
        or not isinstance(server.get("status"), str)
        for server in raw_servers
    ):
        raise ValueError("Claude stream has invalid MCP server state")
    server_names = [server["name"] for server in raw_servers]
    if (
        len(server_names) != len(set(server_names))
        or sorted(server_names) != sorted(contract.mcp_servers)
        or any(server["status"] != "connected" for server in raw_servers)
    ):
        raise ValueError("Claude stream MCP server state does not match the case")

    if init_event.get("skills", []) != [] or init_event.get("plugins", []) != []:
        raise ValueError("Claude stream loaded unexpected skills or plugins")


def parse_claude_output(
    stdout: str,
    *,
    streamed: bool,
    runtime_contract: StreamRuntimeContract | None = None,
) -> tuple[dict, list[str], list[ToolExecution]]:
    if not streamed:
        payload = json.loads(stdout)
        if not isinstance(payload, dict):
            raise ValueError("Claude JSON result is not an object")
        model_usage = payload.get("modelUsage")
        if isinstance(model_usage, dict) and len(model_usage) == 1:
            payload["_resolved_model"] = next(iter(model_usage))
        return payload, [], []

    events: list[dict] = []
    for line_number, line in enumerate(stdout.splitlines(), start=1):
        if not line.strip():
            continue
        event = json.loads(line)
        if not isinstance(event, dict):
            raise ValueError(f"Claude stream event {line_number} is not an object")
        events.append(event)
    init_events = [
        (index, event)
        for index, event in enumerate(events)
        if event.get("type") == "system" and event.get("subtype") == "init"
    ]
    result_events = [
        (index, event)
        for index, event in enumerate(events)
        if event.get("type") == "result"
    ]
    if len(init_events) != 1:
        raise ValueError(
            f"Claude stream must contain one init event, got {len(init_events)}"
        )
    if len(result_events) != 1:
        raise ValueError(
            f"Claude stream must contain one result event, got {len(result_events)}"
        )
    init_index, init_event = init_events[0]
    result_index, payload = result_events[0]
    resolved_model = init_event.get("model")
    if not isinstance(resolved_model, str) or not resolved_model:
        raise ValueError("Claude stream init has no primary model")
    if runtime_contract is not None:
        validate_stream_runtime(init_event, runtime_contract)
    if init_index >= result_index:
        raise ValueError("Claude stream result precedes its init event")
    notification_types = {"system", "rate_limit_event"}
    if any(event.get("type") not in notification_types for event in events[:init_index]):
        raise ValueError("Claude stream contains unsupported events before its init")
    if any(
        event.get("type") not in notification_types
        for event in events[result_index + 1 :]
    ):
        raise ValueError("Claude stream contains unsupported events after its result")
    if not isinstance(payload.get("is_error"), bool):
        raise ValueError("Claude stream result has invalid error state")
    if payload["is_error"] is True or str(payload.get("subtype", "")).startswith("error"):
        raise ValueError("Claude stream ended with an error result")
    if payload.get("subtype") != "success":
        raise ValueError("Claude stream result has an unsupported subtype")
    if payload.get("stop_reason") == "max_tokens":
        raise ValueError("Claude stream final answer reached its token limit")
    if not _nonblank_case_text(payload.get("result")):
        raise ValueError("Claude stream result has no final answer")
    permission_denials = payload.get("permission_denials", [])
    if not isinstance(permission_denials, list) or any(
        not isinstance(denial, dict) for denial in permission_denials
    ):
        raise ValueError("Claude stream result has invalid permission denials")
    payload["_resolved_model"] = resolved_model

    tool_executions: list[ToolExecution] = []
    tools_by_id: dict[str, ToolExecution] = {}
    tool_calls: list[str] = []
    for event in events[init_index + 1 : result_index]:
        event_type = event.get("type")
        message = event.get("message")
        content = message.get("content") if isinstance(message, dict) else None
        if event_type == "assistant":
            message_model = message.get("model") if isinstance(message, dict) else None
            if event.get("error") is not None:
                raise ValueError("Claude stream contains an assistant error")
            if (
                runtime_contract is not None
                and message_model != resolved_model
            ) or (
                runtime_contract is None
                and message_model is not None
                and message_model != resolved_model
            ):
                raise ValueError("Claude stream changed model during evaluation")
            if not isinstance(content, list):
                raise ValueError("Claude assistant event has invalid content")
            for block in content:
                if not isinstance(block, dict):
                    raise ValueError("Claude assistant event has invalid content block")
                if block.get("type") != "tool_use":
                    continue
                name = block.get("name")
                tool_use_id = block.get("id")
                arguments = block.get("input")
                if not isinstance(name, str) or not name:
                    raise ValueError("Claude stream has a tool call without a name")
                if not isinstance(tool_use_id, str) or not tool_use_id:
                    raise ValueError("Claude stream has a tool call without an id")
                if not isinstance(arguments, dict):
                    raise ValueError("Claude stream has a tool call with invalid input")
                if (
                    runtime_contract is not None
                    and name not in runtime_contract.tools
                ):
                    raise ValueError("Claude stream called a tool outside its inventory")
                if tool_use_id in tools_by_id:
                    raise ValueError("Claude stream repeats a tool call id")
                execution = ToolExecution(name, tool_use_id, arguments)
                tools_by_id[tool_use_id] = execution
                tool_executions.append(execution)
                tool_calls.append(name)
        elif event_type == "user":
            if not isinstance(content, list):
                raise ValueError("Claude user event has invalid content")
            for block in content:
                if not isinstance(block, dict) or block.get("type") != "tool_result":
                    raise ValueError("Claude user event has invalid content block")
                tool_use_id = block.get("tool_use_id")
                execution = tools_by_id.get(tool_use_id)
                if execution is None:
                    raise ValueError("Claude stream has an unmatched tool result")
                if execution.result_seen:
                    raise ValueError("Claude stream repeats a tool result")
                is_error = block.get("is_error", False)
                if not isinstance(is_error, bool):
                    raise ValueError("Claude stream tool result has invalid error state")
                execution.result_seen = True
                execution.succeeded = not is_error
        elif event_type not in notification_types:
            raise ValueError("Claude stream has an unsupported event type")
    if any(not execution.result_seen for execution in tool_executions):
        raise ValueError("Claude stream has a tool call without a result")
    return payload, tool_calls, tool_executions


def usage_budget_reasons(expect: dict, telemetry: dict[str, object]) -> list[str]:
    reasons: list[str] = []
    turns = int(telemetry["num_turns"])
    output_tokens = int(telemetry["output_tokens"])
    minimum_turns = expect.get("min_turns")
    maximum_turns = expect.get("max_turns")
    maximum_output = expect.get("max_output_tokens")
    if isinstance(minimum_turns, int) and turns < minimum_turns:
        reasons.append(f"used {turns} turn(s), expected at least {minimum_turns}")
    if isinstance(maximum_turns, int) and turns > maximum_turns:
        reasons.append(f"used {turns} turn(s), expected at most {maximum_turns}")
    if isinstance(maximum_output, int) and output_tokens > maximum_output:
        reasons.append(
            f"used {output_tokens} output token(s), expected at most {maximum_output}"
        )
    return reasons


def tool_usage_reasons(expect: dict, tool_calls: list[str]) -> list[str]:
    policy = expect.get("tools")
    if policy is None:
        return []
    if not isinstance(policy, dict):
        return ["tool policy must be an object"]

    reasons: list[str] = []
    first = policy.get("first")
    if first is not None:
        if not isinstance(first, str) or not first:
            reasons.append("tool policy first must be a non-empty string")
        elif not tool_calls or tool_calls[0] != first:
            observed = tool_calls[0] if tool_calls else None
            reasons.append(f"first tool {observed!r} != expected {first!r}")

    required = policy.get("contains", [])
    if not isinstance(required, list) or any(
        not isinstance(name, str) or not name for name in required
    ):
        reasons.append("tool policy contains must be a list of tool names")
    else:
        for name in required:
            if name not in tool_calls:
                reasons.append(f"required tool was not called: {name!r}")

    alternatives = policy.get("contains_any", [])
    if not isinstance(alternatives, list):
        reasons.append("tool policy contains_any must be a list")
    else:
        for group in alternatives:
            if (
                not isinstance(group, list)
                or not group
                or any(not isinstance(name, str) or not name for name in group)
            ):
                reasons.append(f"invalid tool alternative group: {group!r}")
            elif not any(name in tool_calls for name in group):
                reasons.append(f"none of the alternative tools were called: {group!r}")

    maximum = policy.get("max")
    if maximum is not None:
        if isinstance(maximum, bool) or not isinstance(maximum, int) or maximum < 0:
            reasons.append("tool policy max must be a non-negative integer")
        elif len(tool_calls) > maximum:
            reasons.append(
                f"used {len(tool_calls)} tool call(s), expected at most {maximum}"
            )
    maximum_counts = policy.get("max_counts", {})
    if not isinstance(maximum_counts, dict) or any(
        not isinstance(name, str)
        or not name
        or isinstance(limit, bool)
        or not isinstance(limit, int)
        or limit < 0
        for name, limit in maximum_counts.items()
    ):
        reasons.append("tool policy max_counts must map tool names to limits")
    else:
        for name, limit in maximum_counts.items():
            observed = tool_calls.count(name)
            if observed > limit:
                reasons.append(
                    f"called {name!r} {observed} time(s), expected at most {limit}"
                )
    return reasons


def nearest_rank(values: list[int | float], percentile: int) -> int | float:
    if not values:
        raise ValueError("cannot calculate a percentile for an empty sample")
    if not 1 <= percentile <= 100:
        raise ValueError("percentile must be between 1 and 100")
    ordered = sorted(values)
    index = max(0, math.ceil(percentile / 100 * len(ordered)) - 1)
    return ordered[index]


def metric_summary(values: list[int | float]) -> dict[str, int | float]:
    if not values:
        raise ValueError("cannot summarize an empty metric")
    return {
        "total": sum(values),
        "p50": nearest_rank(values, 50),
        "p95": nearest_rank(values, 95),
    }


def result_report(result: Result) -> dict:
    return {
        "id": result.case_id,
        "suite": result.suite,
        "passed": result.passed,
        "reasons": result.reasons,
        "retry_used": result.retry_used,
        "retry_attempted": result.retry_attempted,
        "duration_ms": result.duration_ms,
        "duration_api_ms": result.duration_api_ms,
        "turns": result.num_turns,
        "telemetry": {
            "complete": result.telemetry_complete,
            "issues": result.telemetry_issues,
        },
        "usage": {
            "input_tokens": result.input_tokens,
            "output_tokens": result.output_tokens,
            "cache_creation_input_tokens": result.cache_creation_input_tokens,
            "cache_read_input_tokens": result.cache_read_input_tokens,
            "context_tokens": result.context_tokens,
            "total_tokens": result.total_tokens,
        },
        "cost_usd": result.cost_usd,
        "resolved_models": result.resolved_models,
        "tool_calls": result.tool_calls,
        "citation_checks": result.citation_checks,
    }


def load_case_records(
    path: Path, *, suite_name: str | None = None
) -> list[dict]:
    records: list[dict] = []
    seen_ids: set[str] = set()
    for line_number, line in enumerate(
        path.read_text(encoding="utf-8").splitlines(), start=1
    ):
        if not line.strip() or line.lstrip().startswith("//"):
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError(
                f"invalid JSON at {path}:{line_number}: {error.msg}"
            ) from error
        if not isinstance(record, dict):
            raise ValueError(f"case at {path}:{line_number} must be an object")
        case_id = record.get("id")
        if (
            not isinstance(case_id, str)
            or not case_id
            or case_id.strip() != case_id
        ):
            raise ValueError(f"case at {path}:{line_number} has no id")
        if case_id in seen_ids:
            raise ValueError(f"duplicate case id {case_id!r} in {path}")
        if suite_name is not None:
            try:
                validate_case_record(suite_name, record)
            except ValueError as error:
                raise ValueError(
                    f"invalid {suite_name} case at {path}:{line_number}: {error}"
                ) from error
        seen_ids.add(case_id)
        records.append(record)
    return records


def _fixture_relative_path(value: object, label: str) -> Path:
    if not isinstance(value, str) or not value or "\\" in value:
        raise ValueError(f"{label} must be a canonical relative path")
    relative = Path(value)
    if (
        relative.is_absolute()
        or relative == Path(".")
        or ".." in relative.parts
        or relative.as_posix() != value
        or any(_is_git_metadata_component(part) for part in relative.parts)
    ):
        raise ValueError(f"{label} must be a canonical relative path")
    return relative


def _is_git_metadata_component(value: str) -> bool:
    normalized = value.rstrip(" .").casefold()
    return normalized in {".git", "git~1"}


def _fixture_tag_name(value: object, label: str) -> str:
    if not isinstance(value, str) or re.fullmatch(
        r"[A-Za-z0-9_][A-Za-z0-9._/-]*", value
    ) is None:
        raise ValueError(f"{label} must be a canonical fixture tag name")
    parts = value.split("/")
    if (
        ".." in value
        or "@{" in value
        or any(
            not part
            or part.startswith(".")
            or part.endswith(".")
            or part.casefold().endswith(".lock")
            or _is_git_metadata_component(part)
            for part in parts
        )
    ):
        raise ValueError(f"{label} must be a canonical fixture tag name")
    return value


def fixture_tag_ref(value: object, label: str = "fixture tag") -> str:
    return f"refs/tags/{_fixture_tag_name(value, label)}"


def _validate_fixture_refs(baseline_ref: object, after_ref: object) -> tuple[str, str]:
    baseline = _fixture_tag_name(baseline_ref, "baseline_ref")
    after = _fixture_tag_name(after_ref, "after_ref")
    if (
        baseline == after
        or baseline.startswith(f"{after}/")
        or after.startswith(f"{baseline}/")
    ):
        raise ValueError("baseline_ref and after_ref must be distinct fixture tags")
    return baseline, after


_CASE_INPUT_FIELDS = {
    "critic": (
        {"problem", "design", "alternatives", "constraints", "mmcg_snapshot"},
        set(),
    ),
    "researcher": ({"question", "scope"}, {"evidence"}),
    "auditor": ({"spec_summary", "executor_report"}, set()),
    "intake": ({"raw_prompt", "target_consumer"}, {"project_context"}),
    "workflow": ({"prompt"}, set()),
}
_COMMON_EXPECTATION_FIELDS = {
    "min_turns",
    "max_turns",
    "max_output_tokens",
    "tools",
    "contains",
    "contains_any",
    "not_contains",
}


def _nonblank_case_text(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip()) and "\0" not in value


def _case_string_list(
    value: object, label: str, *, allow_empty: bool = False
) -> list[str]:
    if (
        not isinstance(value, list)
        or (not allow_empty and not value)
        or any(
            not isinstance(item, str)
            or not item
            or item.strip() != item
            or "\0" in item
            for item in value
        )
    ):
        raise ValueError(f"{label} must be a list of canonical non-empty strings")
    identities = [item.casefold() for item in value]
    if len(identities) != len(set(identities)):
        raise ValueError(f"{label} must not contain duplicates")
    return value


def _case_alternative_groups(value: object, label: str) -> list[list[str]]:
    if not isinstance(value, list) or not value:
        raise ValueError(f"{label} must be a non-empty list of string lists")
    groups = [
        _case_string_list(group, f"{label} group")
        for group in value
    ]
    identities = [
        tuple(sorted(item.casefold() for item in group)) for group in groups
    ]
    if len(identities) != len(set(identities)):
        raise ValueError(f"{label} must not contain duplicate groups")
    return groups


def _case_verdicts(value: object, label: str) -> list[str]:
    values = [value] if isinstance(value, str) else value
    result = _case_string_list(values, label)
    if any(item != item.lower() for item in result):
        raise ValueError(f"{label} must use lowercase canonical values")
    return result


def _observable_eval_tools(suite_name: str) -> set[str]:
    if suite_name not in {"researcher", "auditor"}:
        return set()
    try:
        mcp_tools = set(subagent_mcp_tools(SUITES[suite_name]["subagent"]))
    except (OSError, RuntimeError, ValueError) as error:
        raise ValueError(
            f"cannot inspect {suite_name!r} tool contract: {error}"
        ) from error
    if suite_name == "researcher":
        return {"Read", "Glob", "Grep", *mcp_tools}
    if suite_name == "auditor":
        return {"Read", "Glob", "Grep", "Bash", *mcp_tools}
    raise AssertionError(f"unhandled fixture suite {suite_name!r}")


def _validate_case_tool_policy(suite_name: str, value: object) -> set[str]:
    if not isinstance(value, dict) or not value:
        raise ValueError("expect.tools must be a non-empty object")
    allowed_fields = {"first", "contains", "contains_any", "max", "max_counts"}
    unexpected = set(value) - allowed_fields
    if unexpected:
        raise ValueError(f"expect.tools has unexpected fields: {sorted(unexpected)!r}")

    first = value.get("first")
    if first is not None and (
        not isinstance(first, str)
        or not first
        or first.strip() != first
    ):
        raise ValueError("expect.tools.first must be a canonical tool name")
    required = (
        _case_string_list(value["contains"], "expect.tools.contains")
        if "contains" in value
        else []
    )
    alternatives = (
        _case_alternative_groups(
            value["contains_any"], "expect.tools.contains_any"
        )
        if "contains_any" in value
        else []
    )
    maximum = value.get("max")
    if maximum is not None and (
        isinstance(maximum, bool)
        or not isinstance(maximum, int)
        or maximum < 0
    ):
        raise ValueError("expect.tools.max must be a non-negative integer")
    maximum_counts = value.get("max_counts", {})
    if (
        not isinstance(maximum_counts, dict)
        or any(
            not isinstance(name, str)
            or not name
            or name.strip() != name
            or isinstance(limit, bool)
            or not isinstance(limit, int)
            or limit < 0
            for name, limit in maximum_counts.items()
        )
    ):
        raise ValueError("expect.tools.max_counts must map tool names to limits")
    if (
        first is None
        and not required
        and not alternatives
        and maximum is None
        and not maximum_counts
    ):
        raise ValueError("expect.tools does not constrain tool use")

    named_tools = {
        *required,
        *maximum_counts,
        *(group_item for group in alternatives for group_item in group),
    }
    if first is not None:
        named_tools.add(first)
    unsupported = named_tools - _observable_eval_tools(suite_name)
    if unsupported:
        raise ValueError(
            f"expect.tools names unavailable tools: {sorted(unsupported)!r}"
        )
    mandatory = set(required)
    if first is not None:
        mandatory.add(first)
    if maximum is not None and maximum < len(mandatory):
        raise ValueError("expect.tools.max is lower than mandatory tool calls")
    if maximum == 0 and alternatives:
        raise ValueError("expect.tools.max forbids every tool alternative")
    if any(maximum_counts.get(name, 1) == 0 for name in mandatory):
        raise ValueError("expect.tools.max_counts forbids a mandatory tool")
    if any(
        all(maximum_counts.get(name, 1) == 0 for name in group)
        for group in alternatives
    ):
        raise ValueError("expect.tools.max_counts forbids every tool alternative")
    return {
        *mandatory,
        *(name for group in alternatives for name in group),
    }


def _validate_code_comment_policy(value: object) -> None:
    if not isinstance(value, dict):
        raise ValueError("expect.code_comments must be an object")
    allowed_fields = {
        "prefixes",
        "require_fenced_code",
        "min",
        "max",
        "contains_any",
        "not_contains",
    }
    unexpected = set(value) - allowed_fields
    if unexpected:
        raise ValueError(
            f"expect.code_comments has unexpected fields: {sorted(unexpected)!r}"
        )
    if "prefixes" in value:
        _case_string_list(value["prefixes"], "expect.code_comments.prefixes")
    if "require_fenced_code" in value and not isinstance(
        value["require_fenced_code"], bool
    ):
        raise ValueError("expect.code_comments.require_fenced_code must be boolean")
    minimum = value.get("min", 0)
    maximum = value.get("max")
    for label, number in (("min", minimum), ("max", maximum)):
        if number is not None and (
            isinstance(number, bool)
            or not isinstance(number, int)
            or number < 0
        ):
            raise ValueError(
                f"expect.code_comments.{label} must be a non-negative integer"
            )
    if maximum is not None and minimum > maximum:
        raise ValueError("expect.code_comments.min exceeds max")
    alternatives = (
        _case_alternative_groups(
            value["contains_any"], "expect.code_comments.contains_any"
        )
        if "contains_any" in value
        else []
    )
    if "not_contains" in value:
        forbidden = _case_string_list(
            value["not_contains"],
            "expect.code_comments.not_contains",
            allow_empty=True,
        )
    else:
        forbidden = []
    if maximum == 0 and alternatives:
        raise ValueError(
            "expect.code_comments cannot require phrases when max is zero"
        )
    if any(
        all(
            any(blocked.casefold() in phrase.casefold() for blocked in forbidden)
            for phrase in group
        )
        for group in alternatives
    ):
        raise ValueError(
            "expect.code_comments contains_any contradicts not_contains"
        )
    if (
        value.get("require_fenced_code") is False
        and minimum == 0
        and maximum is None
        and not alternatives
        and not forbidden
    ):
        raise ValueError("expect.code_comments does not constrain the output")


def _validate_citation_expectations(value: object) -> None:
    if not isinstance(value, list) or not value:
        raise ValueError("expect.citations must be a non-empty list")
    seen: set[tuple[str, str]] = set()
    for entry in value:
        if not isinstance(entry, dict) or set(entry) != {"path", "anchor"}:
            raise ValueError(
                "each expect.citations entry must contain only path and anchor"
            )
        path = _fixture_relative_path(entry["path"], "citation path").as_posix()
        anchor = entry["anchor"]
        if not _nonblank_case_text(anchor) or "\n" in anchor or "\r" in anchor:
            raise ValueError("citation anchor must be non-empty single-line text")
        identity = (path, anchor)
        if identity in seen:
            raise ValueError("expect.citations contains a duplicate expectation")
        seen.add(identity)


def _canonical_staged_paths(value: object) -> list[str] | None:
    if value is None:
        return None
    if not isinstance(value, list):
        raise ValueError(
            "staged_paths must be a list of canonical repository-relative paths"
        )
    try:
        paths = [
            _fixture_relative_path(path, "staged path").as_posix()
            for path in value
        ]
    except ValueError as error:
        raise ValueError(
            "staged_paths must be a list of canonical repository-relative paths"
        ) from error
    if len(paths) != len(set(paths)):
        raise ValueError("staged_paths must not contain duplicates")
    return paths


def validate_case_record(suite_name: str, record: object) -> None:
    if suite_name not in SUITES or suite_name not in _CASE_INPUT_FIELDS:
        raise ValueError(f"unknown suite {suite_name!r}")
    if not isinstance(record, dict):
        raise ValueError("case must be an object")
    case_id = record.get("id")
    if (
        not isinstance(case_id, str)
        or not case_id
        or case_id.strip() != case_id
        or "\0" in case_id
    ):
        raise ValueError("case id must be canonical non-empty text")

    required_fields = {"id", "why", "input", "expect"}
    optional_fields: set[str] = set()
    if SUITES[suite_name].get("uses_fixture"):
        required_fields |= {"fixture", "baseline_ref", "after_ref"}
        optional_fields |= {"staged_paths", "allow_no_mmcg"}
    if suite_name == "workflow":
        required_fields.add("artifact")
    missing = required_fields - set(record)
    unexpected = set(record) - required_fields - optional_fields
    if missing or unexpected:
        raise ValueError(
            f"case {case_id!r} has invalid fields: missing {sorted(missing)!r}, "
            f"unexpected {sorted(unexpected)!r}"
        )
    if not _nonblank_case_text(record["why"]):
        raise ValueError(f"case {case_id!r} has no rationale")

    if SUITES[suite_name].get("uses_fixture"):
        _fixture_relative_path(record["fixture"], "fixture")
        _validate_fixture_refs(record["baseline_ref"], record["after_ref"])
        if "staged_paths" in record:
            _canonical_staged_paths(record["staged_paths"])
        if "allow_no_mmcg" in record and not isinstance(
            record["allow_no_mmcg"], bool
        ):
            raise ValueError("allow_no_mmcg must be boolean")
    if suite_name == "workflow":
        artifact = record["artifact"]
        if not isinstance(artifact, str) or artifact not in WORKFLOW_ARTIFACTS:
            raise ValueError("workflow artifact is not in the shipped allowlist")

    inp = record["input"]
    required_input, optional_input = _CASE_INPUT_FIELDS[suite_name]
    if not isinstance(inp, dict):
        raise ValueError("case input must be an object")
    missing_input = required_input - set(inp)
    unexpected_input = set(inp) - required_input - optional_input
    if missing_input or unexpected_input:
        raise ValueError(
            f"case input has invalid fields: missing {sorted(missing_input)!r}, "
            f"unexpected {sorted(unexpected_input)!r}"
        )
    for field_name, value in inp.items():
        if suite_name == "critic" and field_name == "alternatives":
            if isinstance(value, list):
                _case_string_list(
                    value, "input.alternatives", allow_empty=True
                )
            elif not _nonblank_case_text(value):
                raise ValueError("input.alternatives must be text or a string list")
        elif not _nonblank_case_text(value):
            raise ValueError(f"input.{field_name} must be non-empty text")

    expect = record["expect"]
    if not isinstance(expect, dict) or not expect:
        raise ValueError("case expect must be a non-empty object")
    allowed_expectations = set(_COMMON_EXPECTATION_FIELDS)
    if suite_name in {"critic", "auditor"}:
        allowed_expectations.add("verdict")
    elif suite_name == "intake":
        allowed_expectations.add("action")
    elif suite_name == "researcher":
        allowed_expectations.add("citations")
    elif (
        suite_name == "workflow"
        and record["artifact"]
        == "skills/workflow/mastermind-critical-review/SKILL.md"
    ):
        allowed_expectations.add("verdict")
    if suite_name == "auditor":
        allowed_expectations.add("verification_rerun")
    if suite_name == "workflow":
        allowed_expectations.add("code_comments")
    unexpected_expectations = set(expect) - allowed_expectations
    if unexpected_expectations:
        raise ValueError(
            "case expect has unexpected fields: "
            f"{sorted(unexpected_expectations)!r}"
        )

    for field_name in ("min_turns", "max_turns", "max_output_tokens"):
        if field_name in expect and (
            isinstance(expect[field_name], bool)
            or not isinstance(expect[field_name], int)
            or expect[field_name] <= 0
        ):
            raise ValueError(f"expect.{field_name} must be a positive integer")
    if (
        "min_turns" in expect
        and "max_turns" in expect
        and expect["min_turns"] > expect["max_turns"]
    ):
        raise ValueError("expect.min_turns exceeds max_turns")
    expected_tools = (
        _validate_case_tool_policy(suite_name, expect["tools"])
        if "tools" in expect
        else set()
    )
    if record.get("allow_no_mmcg") is True and any(
        tool.startswith("mcp__mmcg__") for tool in expected_tools
    ):
        raise ValueError("allow_no_mmcg conflicts with a required mmcg tool")

    phrase_lists: dict[str, list[str]] = {}
    for field_name in ("contains", "not_contains"):
        if field_name in expect:
            phrase_lists[field_name] = _case_string_list(
                expect[field_name],
                f"expect.{field_name}",
                allow_empty=field_name == "not_contains",
            )
    alternative_phrases = (
        _case_alternative_groups(expect["contains_any"], "expect.contains_any")
        if "contains_any" in expect
        else []
    )
    forbidden = {
        phrase.casefold() for phrase in phrase_lists.get("not_contains", [])
    }
    required_phrases = {
        phrase.casefold() for phrase in phrase_lists.get("contains", [])
    }
    if any(
        blocked in required
        for required in required_phrases
        for blocked in forbidden
    ):
        raise ValueError("expect contains and not_contains contradict each other")
    if any(
        all(
            any(blocked in phrase.casefold() for blocked in forbidden)
            for phrase in group
        )
        for group in alternative_phrases
    ):
        raise ValueError("expect contains_any and not_contains contradict each other")

    if "action" in expect:
        action = expect["action"]
        if not isinstance(action, str) or action not in INTAKE_ACTIONS:
            raise ValueError(f"expect.action must be one of {sorted(INTAKE_ACTIONS)!r}")
        action_signal = f"action: {action}"
        if any(blocked in action_signal for blocked in forbidden):
            raise ValueError("expect.action contradicts not_contains")
    if "verdict" in expect:
        verdicts = _case_verdicts(expect["verdict"], "expect.verdict")
        allowed_verdicts = (
            AUDIT_VERDICTS if suite_name == "auditor" else CRITIC_VERDICTS
        )
        invalid_verdicts = set(verdicts) - allowed_verdicts
        if invalid_verdicts:
            raise ValueError(
                f"expect.verdict has unsupported values: {sorted(invalid_verdicts)!r}"
            )
        if all(
            any(blocked in verdict for blocked in forbidden)
            for verdict in verdicts
        ):
            raise ValueError("expect.verdict contradicts not_contains")
    if "verification_rerun" in expect:
        command = expect["verification_rerun"]
        if (
            not _nonblank_case_text(command)
            or command.strip() != command
            or "\n" in command
            or "\r" in command
        ):
            raise ValueError("expect.verification_rerun must be one command line")
    if "citations" in expect:
        _validate_citation_expectations(expect["citations"])
    if "code_comments" in expect:
        _validate_code_comment_policy(expect["code_comments"])

    if suite_name == "critic" and "verdict" not in expect:
        raise ValueError("critic case must define expect.verdict")
    if suite_name == "auditor" and "verdict" not in expect:
        raise ValueError("auditor case must define expect.verdict")
    if suite_name == "intake" and "action" not in expect:
        raise ValueError("intake case must define expect.action")
    positive_oracles = {"contains", "contains_any", "citations", "code_comments"}
    if suite_name in {"researcher", "workflow"} and not (
        positive_oracles & set(expect) or "verdict" in expect
    ):
        raise ValueError(f"{suite_name} case has no positive output oracle")


def _fixture_descendant_without_symlinks(
    root: Path, relative: Path, label: str
) -> Path:
    current = root
    for part in relative.parts:
        current /= part
        if current.is_symlink():
            raise ValueError(f"{label} contains a symbolic link: {current}")
    return current


def fixture_case_roots(
    record: dict, *, fixtures_dir: Path | None = None
) -> tuple[Path, Path, Path, Path]:
    fixtures_root = (FIXTURES_DIR if fixtures_dir is None else fixtures_dir).resolve()
    fixture_relative = _fixture_relative_path(record.get("fixture"), "fixture")
    after_relative = _fixture_relative_path(record.get("after_ref"), "after_ref")
    _validate_fixture_refs(record.get("baseline_ref"), record.get("after_ref"))
    fixture_path = _fixture_descendant_without_symlinks(
        fixtures_root, fixture_relative, "fixture path"
    )
    changes_path = _fixture_descendant_without_symlinks(
        fixture_path, Path("changes"), "fixture changes path"
    )
    baseline_path = _fixture_descendant_without_symlinks(
        fixture_path, Path("baseline"), "fixture baseline path"
    )
    after_path = _fixture_descendant_without_symlinks(
        changes_path, after_relative, "fixture after path"
    )
    fixture_root = fixture_path.resolve()
    changes_root = changes_path.resolve()
    baseline_root = baseline_path.resolve()
    after_root = after_path.resolve()
    try:
        fixture_root.relative_to(fixtures_root)
        changes_root.relative_to(fixture_root)
        baseline_root.relative_to(fixture_root)
        after_root.relative_to(changes_root)
    except ValueError as error:
        raise ValueError("fixture paths must stay inside the fixture root") from error
    return fixture_relative, after_relative, baseline_root, after_root


def _fixture_tree_paths(root: Path) -> list[Path]:
    if root.is_symlink():
        raise ValueError(f"fixture tree root is a symbolic link: {root}")
    if not root.is_dir():
        raise FileNotFoundError(f"fixture tree missing: {root}")
    paths: list[Path] = []
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        if any(_is_git_metadata_component(part) for part in relative.parts):
            raise ValueError(f"fixture tree contains Git metadata: {path}")
        if path.is_symlink():
            raise ValueError(f"fixture tree contains a symbolic link: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"fixture tree contains an unsupported artifact: {path}")
        paths.append(path)
    return paths


def _stable_regular_file_definition(path: Path) -> dict[str, str]:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode):
            raise ValueError(f"fixture tree contains a non-regular file: {path}")
        digest = hashlib.sha256()
        while chunk := os.read(descriptor, 1024 * 1024):
            digest.update(chunk)
        finished = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    current = path.stat(follow_symlinks=False)
    identity_fields = ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns")
    if any(
        getattr(opened, field) != getattr(finished, field)
        or getattr(opened, field) != getattr(current, field)
        for field in identity_fields
    ):
        raise ValueError(f"fixture file changed while it was read: {path}")
    return {
        "sha256": digest.hexdigest(),
        "git_mode": "100755" if opened.st_mode & 0o111 else "100644",
    }


def fixture_tree_definition(root: Path) -> list[dict[str, str]]:
    paths = _fixture_tree_paths(root)
    records = [
        {
            "path": path.relative_to(root).as_posix(),
            **_stable_regular_file_definition(path),
        }
        for path in paths
    ]
    if [path.relative_to(root) for path in _fixture_tree_paths(root)] != [
        path.relative_to(root) for path in paths
    ]:
        raise ValueError(f"fixture tree changed while it was read: {root}")
    return records


def _validate_case_citation_sources(record: dict, after_root: Path) -> None:
    citations = record["expect"].get("citations")
    if citations is None:
        return
    seen_locations: set[tuple[Path, int]] = set()
    for expectation in citations:
        relative = _fixture_relative_path(
            expectation["path"], "citation path"
        )
        source_path = after_root / relative
        if source_path.is_symlink():
            raise ValueError(
                f"citation source is a symbolic link: {expectation['path']!r}"
            )
        source = source_path.resolve()
        try:
            source.relative_to(after_root.resolve())
        except ValueError as error:
            raise ValueError(
                f"citation source escapes the after-tree: {expectation['path']!r}"
            ) from error
        if not source.is_file():
            raise ValueError(
                f"citation source is not a regular fixture file: "
                f"{expectation['path']!r}"
            )
        lines = source.read_text(encoding="utf-8").splitlines()
        matches = [
            line_number
            for line_number, line in enumerate(lines, start=1)
            if expectation["anchor"] in line
        ]
        if len(matches) != 1:
            raise ValueError(
                f"citation anchor must identify exactly one source line in "
                f"{expectation['path']!r}"
            )
        identity = (source, matches[0])
        if identity in seen_locations:
            raise ValueError("citation expectations identify the same source line")
        seen_locations.add(identity)


def case_definition_digest_from_records(
    suite_name: str,
    selected: list[dict],
    *,
    fixtures_dir: Path | None = None,
) -> str:
    suite = SUITES.get(suite_name)
    if not suite:
        raise ValueError(f"unknown suite: {suite_name}")
    for record in selected:
        validate_case_record(suite_name, record)
    case_ids = [record.get("id") for record in selected]
    if (
        not case_ids
        or any(not isinstance(case_id, str) or not case_id for case_id in case_ids)
        or len(case_ids) != len(set(case_ids))
    ):
        raise ValueError("selected cases must have unique non-empty ids")
    canonical = json.dumps(
        selected,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    if suite.get("uses_fixture"):
        fixtures: list[dict[str, object]] = []
        for record in selected:
            _, _, baseline_root, after_root = fixture_case_roots(
                record, fixtures_dir=fixtures_dir
            )
            baseline_definition = fixture_tree_definition(baseline_root)
            after_definition = fixture_tree_definition(after_root)
            _validate_case_citation_sources(record, after_root)
            fixtures.append(
                {
                    "case_id": record["id"],
                    "baseline": baseline_definition,
                    "after": after_definition,
                }
            )
        canonical += b"\0fixture-trees\0" + json.dumps(
            fixtures,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode("utf-8")
    return hashlib.sha256(canonical).hexdigest()


def snapshot_fixture_definitions(
    selected: list[dict], destination: Path, *, fixtures_dir: Path | None = None
) -> None:
    copied: set[Path] = set()
    for record in selected:
        fixture_relative, after_relative, baseline_root, after_root = fixture_case_roots(
            record, fixtures_dir=fixtures_dir
        )
        sources = (
            (baseline_root, destination / fixture_relative / "baseline"),
            (after_root, destination / fixture_relative / "changes" / after_relative),
        )
        for source, target in sources:
            if target in copied:
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            _copy_tree_into(source, target)
            copied.add(target)


def case_definition_digest(suite_name: str, case_ids: list[str]) -> str | None:
    suite = SUITES.get(suite_name)
    if not suite:
        return None
    try:
        records = load_case_records(suite["cases"], suite_name=suite_name)
        by_id = {record["id"]: record for record in records}
        if any(case_id not in by_id for case_id in case_ids):
            return None
        return case_definition_digest_from_records(
            suite_name, [by_id[case_id] for case_id in case_ids]
        )
    except (KeyError, OSError, ValueError):
        return None


def suite_report(
    results: list[Result],
    *,
    definition_digest: str | None = None,
    definition_stable: bool = True,
    target_digest: str | None = None,
    target_stable: bool = True,
) -> dict:
    if not results:
        raise ValueError("cannot build a suite report without cases")
    passed = sum(result.passed for result in results)
    first_pass = sum(result.passed and not result.retry_used for result in results)
    usage_fields = (
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "context_tokens",
        "total_tokens",
    )
    case_ids = [result.case_id for result in results]
    report = {
        "case_ids": case_ids,
        "case_definition_digest": (
            definition_digest
            if definition_digest is not None
            else case_definition_digest(results[0].suite, case_ids)
        ),
        "definition_stable": definition_stable,
        "quality": {
            "passed": passed,
            "total": len(results),
            "pass_rate": passed / len(results),
            "first_pass": first_pass,
            "first_pass_rate": first_pass / len(results),
        },
        "telemetry": {
            "complete": all(result.telemetry_complete for result in results),
            "incomplete_cases": [
                result.case_id for result in results if not result.telemetry_complete
            ],
        },
        "duration_ms": metric_summary([result.duration_ms for result in results]),
        "duration_api_ms": metric_summary(
            [result.duration_api_ms for result in results]
        ),
        "turns": metric_summary([result.num_turns for result in results]),
        "usage": {
            field: metric_summary([getattr(result, field) for result in results])
            for field in usage_fields
        },
        "cost_usd": metric_summary([result.cost_usd for result in results]),
    }
    if target_digest is not None:
        report["target_definition_digest"] = target_digest
        report["target_definition_stable"] = target_stable
    return report


def git_revision() -> str | None:
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=REPO_ROOT,
            env=_git_environment(),
            text=True,
            capture_output=True,
            check=False,
        )
    except OSError:
        return None
    revision = proc.stdout.strip()
    return revision if proc.returncode == 0 and revision else None


def claude_cli_version(binary: str | Path = "claude") -> str | None:
    try:
        proc = subprocess.run(
            [str(binary), "--version"],
            env=_PROC_ENV,
            text=True,
            capture_output=True,
            check=False,
        )
    except OSError:
        return None
    version = proc.stdout.strip()
    return version if proc.returncode == 0 and version else None


def claude_stream_version(cli_version: str) -> str:
    """Return the version token emitted by Claude's stream init event."""
    version = cli_version.strip().split(maxsplit=1)[0] if cli_version.strip() else ""
    if not version:
        raise ValueError("Claude CLI version is empty")
    return version


def evaluation_harness_definition() -> dict[str, str]:
    sources = [
        {
            "path": path.relative_to(REPO_ROOT).as_posix(),
            **_stable_regular_file_definition(path),
        }
        for path in (
            Path(__file__).resolve(),
            EVALS_DIR / "benchmark_process.py",
            EVALS_DIR / "evidence.py",
        )
    ]
    environment = {
        "python_implementation": platform.python_implementation(),
        "python_version": platform.python_version(),
        "platform": sys.platform,
        "pyyaml_version": (
            str(getattr(_yaml, "__version__", "unknown"))
            if _YAML_AVAILABLE
            else "unavailable"
        ),
    }
    digest = hashlib.sha256(
        json.dumps(
            {"sources": sources, "environment": environment},
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode("utf-8")
    ).hexdigest()
    return {"sha256": digest, **environment}


def fixture_runtime_definition(
    git_binary: Path, mmcg_binary: Path | None
) -> dict[str, object]:
    try:
        process = subprocess.run(
            [str(git_binary), "--version"],
            env=_git_environment(),
            text=True,
            capture_output=True,
            check=False,
        )
    except OSError as error:
        raise ValueError("cannot execute pinned Git runtime") from error
    git_version = process.stdout.strip()
    if process.returncode != 0 or not git_version:
        raise ValueError("cannot read pinned Git runtime version")
    git_definition = _stable_regular_file_definition(git_binary)
    mmcg_definition = (
        None
        if mmcg_binary is None
        else _stable_regular_file_definition(mmcg_binary)
    )
    return {
        "git": {
            "sha256": git_definition["sha256"],
            "git_mode": git_definition["git_mode"],
            "version": git_version,
        },
        "mmcg": mmcg_definition,
    }


def build_report(
    results: list[Result],
    *,
    model: str,
    suite_filter: str | None,
    case_filter: str | None,
    case_definition_digests: dict[str, str] | None = None,
    case_definition_stability: dict[str, bool] | None = None,
    target_definition_digests: dict[str, str] | None = None,
    target_definition_stability: dict[str, bool] | None = None,
    claude_version: str | None = None,
    claude_sha256: str | None = None,
    claude_stable: bool = True,
    harness_definition: dict[str, str] | None = None,
    harness_stable: bool = True,
    fixture_runtime: dict[str, object] | None = None,
    fixture_runtime_stable: bool = True,
) -> dict:
    suites: dict[str, list[Result]] = {}
    for result in results:
        suites.setdefault(result.suite, []).append(result)
    report = {
        "kind": REPORT_KIND,
        "schema_version": REPORT_SCHEMA_VERSION,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "git_revision": git_revision(),
        "model": model,
        "resolved_models": sorted(
            {name for result in results for name in result.resolved_models}
        ),
        "claude_cli_version": (
            claude_version if claude_version is not None else claude_cli_version()
        ),
        "filters": {"suite": suite_filter, "case": case_filter},
        "suites": {
            name: suite_report(
                suite_results,
                definition_digest=(
                    None
                    if case_definition_digests is None
                    else case_definition_digests[name]
                ),
                definition_stable=(
                    True
                    if case_definition_stability is None
                    else case_definition_stability[name]
                ),
                target_digest=(
                    None
                    if target_definition_digests is None
                    else target_definition_digests[name]
                ),
                target_stable=(
                    True
                    if target_definition_stability is None
                    else target_definition_stability[name]
                ),
            )
            for name, suite_results in suites.items()
        },
        "cases": [result_report(result) for result in results],
    }
    if claude_sha256 is not None:
        report["claude_cli_sha256"] = claude_sha256
        report["claude_cli_stable"] = claude_stable
    if harness_definition is not None:
        report["evaluation_harness"] = {
            **harness_definition,
            "stable": harness_stable,
        }
    if fixture_runtime is not None:
        report["fixture_runtime"] = {
            **fixture_runtime,
            "stable": fixture_runtime_stable,
        }
    return report


def _non_negative_integer(value: object) -> bool:
    return not isinstance(value, bool) and isinstance(value, int) and value >= 0


def _non_negative_number(value: object) -> bool:
    return (
        not isinstance(value, bool)
        and isinstance(value, (int, float))
        and math.isfinite(float(value))
        and float(value) >= 0
    )


def _string_list(
    value: object, *, allow_empty: bool, unique: bool = True
) -> bool:
    return (
        isinstance(value, list)
        and (allow_empty or bool(value))
        and all(
            isinstance(item, str) and bool(item) and item.strip() == item
            for item in value
        )
        and (not unique or len(value) == len(set(value)))
    )


def _valid_generated_at(value: object) -> bool:
    if not isinstance(value, str) or not value:
        return False
    try:
        parsed = datetime.fromisoformat(value)
    except ValueError:
        return False
    return parsed.tzinfo is not None and parsed.utcoffset() is not None


def _valid_legacy_console_capture(value: object) -> bool:
    return (
        isinstance(value, dict)
        and set(value) == {"mode", "limitations"}
        and value["mode"] == "pre-report-console"
        and _string_list(value["limitations"], allow_empty=False)
    )


def raw_suite_gate_metrics(report: dict, suite_name: str) -> dict[str, object]:
    cases = [case for case in report["cases"] if case["suite"] == suite_name]
    context_values = [case["usage"]["context_tokens"] for case in cases]
    return {
        "pass_rate": sum(case["passed"] for case in cases) / len(cases),
        "passed": {case["id"]: case["passed"] for case in cases},
        "context_tokens": {
            "p50": nearest_rank(context_values, 50),
            "p95": nearest_rank(context_values, 95),
        },
        "incomplete_cases": [
            case["id"]
            for case in cases
            if case["telemetry"]["complete"] is not True
        ],
    }


def report_comparison_issues(
    report: object,
    label: str,
    *,
    require_target_identity: bool = False,
    require_runtime_identity: bool = False,
    require_harness_identity: bool = False,
    require_fixture_runtime: bool = False,
    require_definition_stability: bool = False,
    allow_legacy_capture: bool = False,
) -> list[str]:
    issues: list[str] = []
    if not isinstance(report, dict):
        return [f"{label} report must be a JSON object"]
    allowed_top_level = {
        "kind",
        "schema_version",
        "generated_at",
        "git_revision",
        "model",
        "resolved_models",
        "claude_cli_version",
        "claude_cli_sha256",
        "claude_cli_stable",
        "evaluation_harness",
        "fixture_runtime",
        "filters",
        "suites",
        "cases",
        "capture",
        "baseline_gate",
    }
    unexpected_top_level = set(report) - allowed_top_level
    if unexpected_top_level:
        issues.append(
            f"{label} report has unexpected fields: {sorted(unexpected_top_level)!r}"
        )
    if report.get("kind") != REPORT_KIND:
        issues.append(f"{label} report has an unsupported kind")
    if report.get("schema_version") != REPORT_SCHEMA_VERSION:
        issues.append(f"{label} report has an unsupported schema version")
    if (
        not isinstance(report.get("model"), str)
        or not report["model"]
        or report["model"].strip() != report["model"]
    ):
        issues.append(f"{label} report has no model")
    if not _valid_generated_at(report.get("generated_at")):
        issues.append(f"{label} report has invalid generation time")
    revision = report.get("git_revision")
    if revision is not None and (
        not isinstance(revision, str)
        or re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", revision) is None
    ):
        issues.append(f"{label} report has invalid Git revision")
    valid_legacy_console_capture = _valid_legacy_console_capture(
        report.get("capture")
    )
    legacy_console_capture = allow_legacy_capture and valid_legacy_console_capture
    if "capture" in report:
        if not valid_legacy_console_capture:
            issues.append(f"{label} report has invalid capture metadata")
        elif not allow_legacy_capture:
            issues.append(f"{label} report cannot use legacy capture metadata")
    if "baseline_gate" in report and not isinstance(report["baseline_gate"], dict):
        issues.append(f"{label} report has invalid baseline gate metadata")
    report_models = report.get("resolved_models")
    if (
        not _string_list(report_models, allow_empty=True)
        or report_models != sorted(report_models)
    ):
        issues.append(f"{label} report has invalid resolved model ids")
    if (
        not isinstance(report.get("claude_cli_version"), str)
        or not report["claude_cli_version"]
    ):
        issues.append(f"{label} report has no Claude CLI version")
    has_cli_digest = "claude_cli_sha256" in report
    has_cli_stability = "claude_cli_stable" in report
    if require_runtime_identity and not has_cli_digest and not has_cli_stability:
        issues.append(f"{label} report has no Claude CLI identity")
    elif has_cli_digest != has_cli_stability:
        issues.append(f"{label} report has incomplete Claude CLI identity")
    elif has_cli_digest:
        cli_digest = report["claude_cli_sha256"]
        cli_stable = report["claude_cli_stable"]
        if (
            not isinstance(cli_digest, str)
            or re.fullmatch(r"[0-9a-f]{64}", cli_digest) is None
        ):
            issues.append(f"{label} report has invalid Claude CLI digest")
        if not isinstance(cli_stable, bool):
            issues.append(f"{label} report has invalid Claude CLI stability")
        elif not cli_stable:
            issues.append(f"{label} report changed Claude CLI during evaluation")
    harness = report.get("evaluation_harness")
    if harness is None:
        if require_harness_identity:
            issues.append(f"{label} report has no evaluation harness identity")
    elif not isinstance(harness, dict) or set(harness) != {
        "sha256",
        "stable",
        "python_implementation",
        "python_version",
        "platform",
        "pyyaml_version",
    }:
        issues.append(f"{label} report has invalid evaluation harness identity")
    else:
        harness_digest = harness["sha256"]
        harness_strings = (
            harness["python_implementation"],
            harness["python_version"],
            harness["platform"],
            harness["pyyaml_version"],
        )
        if (
            not isinstance(harness_digest, str)
            or re.fullmatch(r"[0-9a-f]{64}", harness_digest) is None
            or any(not isinstance(value, str) or not value for value in harness_strings)
            or not isinstance(harness["stable"], bool)
        ):
            issues.append(f"{label} report has invalid evaluation harness identity")
        elif not harness["stable"]:
            issues.append(f"{label} report changed evaluation harness during run")
    fixture_runtime = report.get("fixture_runtime")
    if fixture_runtime is None:
        if require_fixture_runtime:
            issues.append(f"{label} report has no fixture runtime identity")
    elif not isinstance(fixture_runtime, dict) or set(fixture_runtime) != {
        "git",
        "mmcg",
        "stable",
    }:
        issues.append(f"{label} report has invalid fixture runtime identity")
    else:
        git_runtime = fixture_runtime["git"]
        mmcg_runtime = fixture_runtime["mmcg"]
        valid_git = (
            isinstance(git_runtime, dict)
            and set(git_runtime) == {"sha256", "git_mode", "version"}
            and isinstance(git_runtime["sha256"], str)
            and re.fullmatch(r"[0-9a-f]{64}", git_runtime["sha256"]) is not None
            and git_runtime["git_mode"] in {"100644", "100755"}
            and isinstance(git_runtime["version"], str)
            and bool(git_runtime["version"])
        )
        valid_mmcg = mmcg_runtime is None or (
            isinstance(mmcg_runtime, dict)
            and set(mmcg_runtime) == {"sha256", "git_mode"}
            and isinstance(mmcg_runtime["sha256"], str)
            and re.fullmatch(r"[0-9a-f]{64}", mmcg_runtime["sha256"]) is not None
            and mmcg_runtime["git_mode"] in {"100644", "100755"}
        )
        if (
            not valid_git
            or not valid_mmcg
            or not isinstance(fixture_runtime["stable"], bool)
        ):
            issues.append(f"{label} report has invalid fixture runtime identity")
        elif not fixture_runtime["stable"]:
            issues.append(f"{label} report changed fixture runtime during run")

    filters = report.get("filters")
    if (
        not isinstance(filters, dict)
        or set(filters) != {"suite", "case"}
        or any(
            value is not None
            and (
                not isinstance(value, str)
                or not value
                or value.strip() != value
            )
            for value in filters.values()
        )
        or (
            isinstance(filters, dict)
            and isinstance(filters.get("suite"), str)
            and filters["suite"] not in SUITES
        )
    ):
        issues.append(f"{label} report has invalid filters")

    suites = report.get("suites")
    cases = report.get("cases")
    if not isinstance(suites, dict) or not suites:
        issues.append(f"{label} report has no suite summaries")
    if not isinstance(cases, list) or not cases:
        issues.append(f"{label} report has no cases")
    if not isinstance(suites, dict) or not suites or not isinstance(cases, list):
        return issues

    case_ids_by_suite: dict[str, list[str]] = {}
    cases_by_suite: dict[str, list[dict]] = {}
    seen_case_ids: set[str] = set()
    raw_models: set[str] = set()
    usage_fields = (
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "context_tokens",
        "total_tokens",
    )
    required_case_fields = {
        "id",
        "suite",
        "passed",
        "reasons",
        "retry_used",
        "retry_attempted",
        "duration_ms",
        "duration_api_ms",
        "turns",
        "telemetry",
        "usage",
        "cost_usd",
        "resolved_models",
        "tool_calls",
    }
    allowed_case_fields = required_case_fields | {"citation_checks"}
    for index, case in enumerate(cases):
        if not isinstance(case, dict):
            issues.append(f"{label} case {index} must be an object")
            continue
        missing_case_fields = required_case_fields - set(case)
        unexpected_case_fields = set(case) - allowed_case_fields
        if missing_case_fields or unexpected_case_fields:
            issues.append(
                f"{label} case {index} has invalid fields: "
                f"missing {sorted(missing_case_fields)!r}, "
                f"unexpected {sorted(unexpected_case_fields)!r}"
            )
        case_id = case.get("id")
        suite_name = case.get("suite")
        if (
            not isinstance(case_id, str)
            or not case_id
            or case_id.strip() != case_id
        ):
            issues.append(f"{label} case {index} has no id")
            continue
        if case_id in seen_case_ids:
            issues.append(f"{label} report repeats case id {case_id!r}")
        seen_case_ids.add(case_id)
        if (
            not isinstance(suite_name, str)
            or not suite_name
            or suite_name.strip() != suite_name
        ):
            issues.append(f"{label} case {case_id!r} has no suite")
            continue
        if suite_name not in SUITES:
            issues.append(f"{label} case {case_id!r} has unknown suite {suite_name!r}")
        case_ids_by_suite.setdefault(suite_name, []).append(case_id)
        cases_by_suite.setdefault(suite_name, []).append(case)

        for field_name in ("passed", "retry_used", "retry_attempted"):
            if not isinstance(case.get(field_name), bool):
                issues.append(
                    f"{label} case {case_id!r} has invalid {field_name}"
                )
        if not _string_list(
            case.get("reasons"), allow_empty=True, unique=False
        ):
            issues.append(f"{label} case {case_id!r} has invalid reasons")
        elif (
            isinstance(case.get("passed"), bool)
            and bool(case["reasons"]) == case["passed"]
        ):
            issues.append(f"{label} case {case_id!r} has inconsistent pass reasons")
        if all(
            isinstance(case.get(field), bool)
            for field in ("passed", "retry_used", "retry_attempted")
        ):
            if case["retry_used"] and (
                not case["retry_attempted"] or not case["passed"]
            ):
                issues.append(f"{label} case {case_id!r} has inconsistent retry state")
            if case["retry_attempted"] and case["passed"] and not case["retry_used"]:
                issues.append(f"{label} case {case_id!r} has inconsistent retry state")
        for field_name in ("duration_ms", "duration_api_ms", "turns"):
            if not _non_negative_integer(case.get(field_name)):
                issues.append(
                    f"{label} case {case_id!r} has invalid {field_name}"
                )
        if not _non_negative_number(case.get("cost_usd")):
            issues.append(f"{label} case {case_id!r} has invalid cost_usd")

        telemetry = case.get("telemetry")
        if (
            not isinstance(telemetry, dict)
            or set(telemetry) != {"complete", "issues"}
            or not isinstance(telemetry.get("complete"), bool)
            or not isinstance(telemetry.get("issues"), list)
            or any(
                not isinstance(issue, str) or not issue
                for issue in telemetry.get("issues", [])
            )
        ):
            issues.append(f"{label} case {case_id!r} has invalid telemetry status")
        elif telemetry["complete"] == bool(telemetry["issues"]):
            issues.append(f"{label} case {case_id!r} has inconsistent telemetry status")

        case_models = case.get("resolved_models")
        telemetry_complete = (
            isinstance(telemetry, dict) and telemetry.get("complete") is True
        )
        if not _string_list(
            case_models, allow_empty=not telemetry_complete
        ):
            issues.append(f"{label} case {case_id!r} has invalid resolved model ids")
        else:
            raw_models.update(case_models)
        if not _string_list(
            case.get("tool_calls"), allow_empty=True, unique=False
        ):
            issues.append(f"{label} case {case_id!r} has invalid tool calls")

        citations = case.get("citation_checks")
        if citations is not None:
            counters = ("expected", "matched", "total", "valid")
            if (
                not isinstance(citations, dict)
                or set(citations) != {*counters, "issues"}
                or any(not _non_negative_integer(citations.get(key)) for key in counters)
                or not _string_list(
                    citations.get("issues"), allow_empty=True, unique=False
                )
            ):
                issues.append(f"{label} case {case_id!r} has invalid citation checks")
            elif (
                citations["matched"] > citations["expected"]
                or citations["valid"] > citations["total"]
                or (not citations["issues"] and (
                    citations["expected"] == 0
                    or citations["matched"] != citations["expected"]
                    or citations["valid"] != citations["total"]
                ))
                or (case.get("passed") is True and citations["issues"])
            ):
                issues.append(f"{label} case {case_id!r} has inconsistent citation checks")

        usage = case.get("usage")
        if not isinstance(usage, dict) or set(usage) != set(usage_fields):
            issues.append(f"{label} case {case_id!r} has invalid usage")
            continue
        invalid_usage = [
            field_name
            for field_name in usage_fields
            if not _non_negative_integer(usage.get(field_name))
        ]
        if invalid_usage:
            issues.append(
                f"{label} case {case_id!r} has invalid usage fields {invalid_usage!r}"
            )
            continue
        expected_context = (
            usage["input_tokens"]
            + usage["cache_creation_input_tokens"]
            + usage["cache_read_input_tokens"]
        )
        if usage["context_tokens"] != expected_context:
            issues.append(
                f"{label} case {case_id!r} context tokens do not match raw usage"
            )
        if usage["total_tokens"] != expected_context + usage["output_tokens"]:
            issues.append(
                f"{label} case {case_id!r} total tokens do not match raw usage"
            )

    if _string_list(report_models, allow_empty=True) and sorted(raw_models) != report_models:
        issues.append(f"{label} resolved model ids do not match raw cases")

    required_suite_fields = {
        "case_ids",
        "case_definition_digest",
        "quality",
        "telemetry",
        "duration_ms",
        "duration_api_ms",
        "turns",
        "usage",
        "cost_usd",
    }
    allowed_suite_fields = required_suite_fields | {
        "definition_stable",
        "target_definition_digest",
        "target_definition_stable",
    }
    for suite_name, summary in suites.items():
        if not isinstance(suite_name, str) or not isinstance(summary, dict):
            issues.append(f"{label} report has an invalid suite summary")
            continue
        if suite_name not in SUITES:
            issues.append(f"{label} report has unknown suite {suite_name!r}")
        missing_suite_fields = required_suite_fields - set(summary)
        unexpected_suite_fields = set(summary) - allowed_suite_fields
        if missing_suite_fields or unexpected_suite_fields:
            issues.append(
                f"{label} suite {suite_name!r} has invalid fields: "
                f"missing {sorted(missing_suite_fields)!r}, "
                f"unexpected {sorted(unexpected_suite_fields)!r}"
            )
        case_ids = summary.get("case_ids")
        valid_case_ids = (
            isinstance(case_ids, list)
            and bool(case_ids)
            and all(isinstance(case_id, str) and case_id for case_id in case_ids)
            and len(case_ids) == len(set(case_ids))
        )
        if not valid_case_ids:
            issues.append(f"{label} suite {suite_name!r} has invalid case ids")
        elif case_ids != case_ids_by_suite.get(suite_name, []):
            issues.append(
                f"{label} suite {suite_name!r} case ids do not match its case records"
            )
        digest = summary.get("case_definition_digest")
        if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            issues.append(
                f"{label} suite {suite_name!r} has no valid case definition digest"
            )
        has_definition_stability = "definition_stable" in summary
        definition_stable = summary.get("definition_stable", True)
        if require_definition_stability and not has_definition_stability:
            issues.append(
                f"{label} suite {suite_name!r} has no definition stability"
            )
        elif not isinstance(definition_stable, bool):
            issues.append(
                f"{label} suite {suite_name!r} has invalid definition stability"
            )
        elif not definition_stable:
            issues.append(
                f"{label} suite {suite_name!r} changed definition during evaluation"
            )
        has_target_digest = "target_definition_digest" in summary
        has_target_stability = "target_definition_stable" in summary
        if require_target_identity and not has_target_digest and not has_target_stability:
            issues.append(
                f"{label} suite {suite_name!r} has no target identity"
            )
        elif has_target_digest != has_target_stability:
            issues.append(
                f"{label} suite {suite_name!r} has incomplete target identity"
            )
        elif has_target_digest:
            target_digest = summary["target_definition_digest"]
            target_stable = summary["target_definition_stable"]
            if (
                not isinstance(target_digest, str)
                or re.fullmatch(r"[0-9a-f]{64}", target_digest) is None
            ):
                issues.append(
                    f"{label} suite {suite_name!r} has invalid target definition digest"
                )
            if not isinstance(target_stable, bool):
                issues.append(
                    f"{label} suite {suite_name!r} has invalid target stability"
                )
            elif not target_stable:
                issues.append(
                    f"{label} suite {suite_name!r} changed target during evaluation"
                )

        suite_cases = cases_by_suite.get(suite_name, [])
        can_recompute = (
            valid_case_ids
            and case_ids == case_ids_by_suite.get(suite_name, [])
            and suite_cases
            and all(
                isinstance(case.get("passed"), bool)
                and isinstance(case.get("retry_used"), bool)
                and isinstance(case.get("retry_attempted"), bool)
                and isinstance(case.get("telemetry"), dict)
                and isinstance(case["telemetry"].get("complete"), bool)
                and isinstance(case.get("usage"), dict)
                and all(
                    _non_negative_integer(case["usage"].get(field_name))
                    for field_name in usage_fields
                )
                and all(
                    _non_negative_integer(case.get(field_name))
                    for field_name in ("duration_ms", "duration_api_ms", "turns")
                )
                and _non_negative_number(case.get("cost_usd"))
                for case in suite_cases
            )
        )
        if not can_recompute:
            continue
        passed = sum(case["passed"] for case in suite_cases)
        first_pass = sum(
            case["passed"] and not case["retry_used"] for case in suite_cases
        )
        expected_quality = {
            "passed": passed,
            "total": len(suite_cases),
            "pass_rate": passed / len(suite_cases),
            "first_pass": first_pass,
            "first_pass_rate": first_pass / len(suite_cases),
        }
        if summary.get("quality") != expected_quality:
            issues.append(
                f"{label} suite {suite_name!r} quality summary does not match raw cases"
            )
        for field_name in ("duration_ms", "duration_api_ms", "turns", "cost_usd"):
            expected_summary = metric_summary(
                [case[field_name] for case in suite_cases]
            )
            observed_summary = summary.get(field_name)
            if legacy_console_capture and field_name == "duration_api_ms":
                continue
            if legacy_console_capture and field_name == "cost_usd":
                legacy_cost_matches = (
                    isinstance(observed_summary, dict)
                    and set(observed_summary) == {"total", "p50", "p95"}
                    and observed_summary["p50"] == expected_summary["p50"]
                    and observed_summary["p95"] == expected_summary["p95"]
                    and _non_negative_number(observed_summary["total"])
                    and math.isclose(
                        float(observed_summary["total"]),
                        float(expected_summary["total"]),
                        rel_tol=0,
                        abs_tol=0.0001,
                    )
                )
                if legacy_cost_matches:
                    continue
            if observed_summary != expected_summary:
                issues.append(
                    f"{label} suite {suite_name!r} {field_name} summary "
                    "does not match raw cases"
                )
        usage = summary.get("usage")
        if not isinstance(usage, dict) or set(usage) != set(usage_fields):
            issues.append(f"{label} suite {suite_name!r} has invalid usage summary")
        else:
            for field_name in usage_fields:
                expected_usage_summary = metric_summary(
                    [case["usage"][field_name] for case in suite_cases]
                )
                if usage.get(field_name) != expected_usage_summary:
                    metric_label = (
                        "context summary"
                        if field_name == "context_tokens"
                        else f"{field_name} summary"
                    )
                    issues.append(
                        f"{label} suite {suite_name!r} {metric_label} "
                        "does not match raw cases"
                    )
        expected_telemetry = {
            "complete": all(
                case["telemetry"]["complete"] for case in suite_cases
            ),
            "incomplete_cases": [
                case["id"]
                for case in suite_cases
                if not case["telemetry"]["complete"]
            ],
        }
        if summary.get("telemetry") != expected_telemetry:
            issues.append(
                f"{label} suite {suite_name!r} telemetry summary does not match raw cases"
            )
    extra_case_suites = set(case_ids_by_suite) - set(suites)
    if extra_case_suites:
        issues.append(
            f"{label} cases reference missing suites: {sorted(extra_case_suites)!r}"
        )
    extra_summary_suites = set(suites) - set(case_ids_by_suite)
    if extra_summary_suites:
        issues.append(
            f"{label} suite summaries have no cases: {sorted(extra_summary_suites)!r}"
        )
    if isinstance(filters, dict) and set(filters) == {"suite", "case"}:
        suite_filter = filters.get("suite")
        case_filter = filters.get("case")
        if isinstance(suite_filter, str) and set(suites) != {suite_filter}:
            issues.append(f"{label} suite filter does not match report suites")
        if suite_filter is None and case_filter is None and set(suites) != set(SUITES):
            issues.append(f"{label} unfiltered report does not contain every suite")
        if isinstance(case_filter, str) and seen_case_ids != {case_filter}:
            issues.append(f"{label} case filter does not match report cases")
    return issues


def load_report(path: Path) -> dict:
    try:
        report = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read eval report {path}: {error}") from error
    issues = report_comparison_issues(
        report, f"eval report {path}", allow_legacy_capture=True
    )
    if issues:
        raise ValueError("; ".join(issues))
    return report


def compare_to_baseline(current: dict, baseline: dict) -> dict:
    checks: list[dict] = []
    current_summaries = current.get("suites") if isinstance(current, dict) else None
    current_uses_fixture = isinstance(current_summaries, dict) and any(
        SUITES.get(name, {}).get("uses_fixture") is True
        for name in current_summaries
    )
    failures = [
        *report_comparison_issues(
            current,
            "current",
            require_target_identity=True,
            require_runtime_identity=True,
            require_harness_identity=True,
            require_fixture_runtime=current_uses_fixture,
            require_definition_stability=True,
        ),
        *report_comparison_issues(
            baseline, "baseline", allow_legacy_capture=True
        ),
    ]
    if failures:
        return {"passed": False, "checks": checks, "failures": failures}
    if current["model"] != baseline["model"]:
        failures.append(
            f"model mismatch: current {current['model']!r}, "
            f"baseline {baseline['model']!r}"
        )
    if current["resolved_models"] != baseline["resolved_models"]:
        failures.append(
            "resolved model mismatch: "
            f"current {current['resolved_models']!r}, "
            f"baseline {baseline['resolved_models']!r}"
        )
    if current.get("claude_cli_version") != baseline.get("claude_cli_version"):
        failures.append(
            "Claude CLI version mismatch: "
            f"current {current.get('claude_cli_version')!r}, "
            f"baseline {baseline.get('claude_cli_version')!r}"
        )
    current_cli_digest = current.get("claude_cli_sha256")
    baseline_cli_digest = baseline.get("claude_cli_sha256")
    if (
        isinstance(current_cli_digest, str)
        and isinstance(baseline_cli_digest, str)
        and current_cli_digest != baseline_cli_digest
    ):
        failures.append(
            "Claude CLI binary mismatch: "
            f"current {current_cli_digest!r}, baseline {baseline_cli_digest!r}"
        )
    current_harness = current.get("evaluation_harness")
    baseline_harness = baseline.get("evaluation_harness")
    if isinstance(current_harness, dict) and isinstance(baseline_harness, dict):
        current_harness_identity = {
            key: value for key, value in current_harness.items() if key != "stable"
        }
        baseline_harness_identity = {
            key: value for key, value in baseline_harness.items() if key != "stable"
        }
        if current_harness_identity != baseline_harness_identity:
            failures.append("evaluation harness differs from baseline")
    current_fixture_runtime = current.get("fixture_runtime")
    baseline_fixture_runtime = baseline.get("fixture_runtime")
    if isinstance(current_fixture_runtime, dict) and isinstance(
        baseline_fixture_runtime, dict
    ):
        current_fixture_identity = {
            key: value
            for key, value in current_fixture_runtime.items()
            if key != "stable"
        }
        baseline_fixture_identity = {
            key: value
            for key, value in baseline_fixture_runtime.items()
            if key != "stable"
        }
        if current_fixture_identity != baseline_fixture_identity:
            failures.append("fixture runtime differs from baseline")

    if current.get("filters") != baseline.get("filters"):
        failures.append("current filters differ from baseline filters")

    current_suites = set(current["suites"])
    baseline_suites = set(baseline["suites"])
    if current_suites != baseline_suites:
        failures.append(
            "suite set differs from baseline: "
            f"current {sorted(current_suites)!r}, baseline {sorted(baseline_suites)!r}"
        )

    for suite_name, current_suite in current["suites"].items():
        baseline_suite = baseline["suites"].get(suite_name)
        if not isinstance(baseline_suite, dict):
            failures.append(f"suite {suite_name!r} is missing from baseline")
            continue
        current_ids = current_suite.get("case_ids")
        baseline_ids = baseline_suite.get("case_ids")
        if current_ids != baseline_ids:
            failures.append(
                f"suite {suite_name!r} case set/order differs from baseline"
            )
            continue
        current_digest = current_suite.get("case_definition_digest")
        baseline_digest = baseline_suite.get("case_definition_digest")
        if (
            not isinstance(current_digest, str)
            or not isinstance(baseline_digest, str)
            or current_digest != baseline_digest
        ):
            failures.append(
                f"suite {suite_name!r} case definitions differ from baseline"
            )
            continue
        current_raw = raw_suite_gate_metrics(current, suite_name)
        baseline_raw = raw_suite_gate_metrics(baseline, suite_name)
        incomplete = current_raw["incomplete_cases"]
        if incomplete:
            failures.append(
                f"suite {suite_name!r} has incomplete telemetry for {incomplete!r}"
            )
            continue
        try:
            current_quality = current_raw["pass_rate"]
            baseline_quality = baseline_raw["pass_rate"]
            quality_passed = current_quality >= baseline_quality
            checks.append(
                {
                    "suite": suite_name,
                    "metric": "pass_rate",
                    "statistic": "value",
                    "operator": ">=",
                    "baseline": baseline_quality,
                    "current": current_quality,
                    "passed": quality_passed,
                }
            )
            if not quality_passed:
                failures.append(
                    f"{suite_name} pass rate regressed: "
                    f"{current_quality:.3f} < {baseline_quality:.3f}"
                )
            for case_id in current_ids:
                baseline_passed = baseline_raw["passed"][case_id]
                current_passed = current_raw["passed"][case_id]
                case_passed = not baseline_passed or current_passed
                checks.append(
                    {
                        "suite": suite_name,
                        "case": case_id,
                        "metric": "quality",
                        "statistic": "baseline_pass_preserved",
                        "operator": "implies",
                        "baseline": baseline_passed,
                        "current": current_passed,
                        "passed": case_passed,
                    }
                )
                if not case_passed:
                    failures.append(
                        f"{suite_name} baseline-passing case regressed: {case_id}"
                    )
            for statistic in ("p50", "p95"):
                current_tokens = current_raw["context_tokens"][statistic]
                baseline_tokens = baseline_raw["context_tokens"][statistic]
                tokens_passed = current_tokens < baseline_tokens
                checks.append(
                    {
                        "suite": suite_name,
                        "metric": "context_tokens",
                        "statistic": statistic,
                        "operator": "<",
                        "baseline": baseline_tokens,
                        "current": current_tokens,
                        "passed": tokens_passed,
                    }
                )
                if not tokens_passed:
                    failures.append(
                        f"{suite_name} {statistic} context tokens did not decrease: "
                        f"{current_tokens} >= {baseline_tokens}"
                    )
        except (AttributeError, IndexError, KeyError, TypeError, ValueError) as error:
            failures.append(
                f"suite {suite_name!r} has malformed comparison data: {error}"
            )

    return {"passed": not failures, "checks": checks, "failures": failures}


def _same_report_node(left: os.stat_result, right: os.stat_result) -> bool:
    return (left.st_dev, left.st_ino) == (right.st_dev, right.st_ino)


def _report_file_identity(value: os.stat_result) -> tuple[int, ...]:
    return (
        value.st_dev,
        value.st_ino,
        value.st_mode,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
    )


def _check_report_parent(
    requested: Path,
    canonical: Path,
    descriptor: int,
    expected: os.stat_result,
) -> None:
    try:
        current = canonical.stat(follow_symlinks=False)
        resolved = requested.resolve(strict=True)
        opened = os.fstat(descriptor)
    except (OSError, RuntimeError) as error:
        raise OSError("eval report parent changed during publication") from error
    if (
        resolved != canonical
        or not stat.S_ISDIR(current.st_mode)
        or not _same_report_node(current, expected)
        or not _same_report_node(opened, expected)
    ):
        raise OSError("eval report parent changed during publication")


def _read_report_file_at(parent_descriptor: int, name: str, limit: int) -> bytes:
    descriptor = os.open(
        name,
        os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
        dir_fd=parent_descriptor,
    )
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_size > limit:
            raise OSError("published eval report is not the expected regular file")
        body = bytearray()
        remaining = limit + 1
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                break
            body.extend(chunk)
            remaining -= len(chunk)
        after = os.fstat(descriptor)
        current = os.stat(name, dir_fd=parent_descriptor, follow_symlinks=False)
    finally:
        os.close(descriptor)
    if (
        len(body) > limit
        or _report_file_identity(before) != _report_file_identity(after)
        or _report_file_identity(after) != _report_file_identity(current)
    ):
        raise OSError("published eval report changed during verification")
    return bytes(body)


def _write_report_portable(path: Path, body: bytes) -> None:
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb",
            dir=path.parent,
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            temporary = Path(handle.name)
            handle.write(body)
            handle.flush()
            os.fsync(handle.fileno())
        temporary.replace(path)
        temporary = None
        if path.read_bytes() != body:
            raise OSError("published eval report changed during verification")
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def write_report(path: Path, report: dict) -> None:
    body = (
        json.dumps(report, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    ).encode("utf-8")
    target = path.absolute()
    if not target.name or target.name in {".", ".."}:
        raise OSError("invalid eval report path")
    target.parent.mkdir(parents=True, exist_ok=True)
    if os.name != "posix" or not hasattr(os, "O_NOFOLLOW"):
        _write_report_portable(target, body)
        return

    requested_parent = target.parent
    try:
        canonical_parent = requested_parent.resolve(strict=True)
        parent_descriptor = os.open(
            canonical_parent,
            os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_NONBLOCK,
        )
    except (OSError, RuntimeError) as error:
        raise OSError("cannot open eval report parent safely") from error

    temporary_name = f".{target.name}.{uuid.uuid4().hex}.tmp"
    descriptor: int | None = None
    owned: tuple[int, int] | None = None
    staged = False
    published = False
    try:
        expected_parent = os.fstat(parent_descriptor)
        if not stat.S_ISDIR(expected_parent.st_mode):
            raise OSError("eval report parent is not a directory")
        descriptor = os.open(
            temporary_name,
            os.O_RDWR
            | os.O_CREAT
            | os.O_EXCL
            | os.O_NOFOLLOW
            | os.O_NONBLOCK,
            0o600,
            dir_fd=parent_descriptor,
        )
        staged = True
        opened = os.fstat(descriptor)
        owned = (opened.st_dev, opened.st_ino)
        os.fchmod(descriptor, 0o600)
        with os.fdopen(os.dup(descriptor), "wb") as handle:
            handle.write(body)
            handle.flush()
            os.fsync(handle.fileno())
        written = os.fstat(descriptor)
        staged_stat = os.stat(
            temporary_name, dir_fd=parent_descriptor, follow_symlinks=False
        )
        if (
            not stat.S_ISREG(written.st_mode)
            or stat.S_IMODE(written.st_mode) != 0o600
            or written.st_size != len(body)
            or not _same_report_node(written, staged_stat)
        ):
            raise OSError("staged eval report changed during publication")
        _check_report_parent(
            requested_parent, canonical_parent, parent_descriptor, expected_parent
        )
        os.replace(
            temporary_name,
            target.name,
            src_dir_fd=parent_descriptor,
            dst_dir_fd=parent_descriptor,
        )
        staged = False
        published = True
        named = os.stat(target.name, dir_fd=parent_descriptor, follow_symlinks=False)
        if not _same_report_node(written, named):
            raise OSError("published eval report changed during publication")
        os.fsync(parent_descriptor)
        _check_report_parent(
            requested_parent, canonical_parent, parent_descriptor, expected_parent
        )

        os.lseek(descriptor, 0, os.SEEK_SET)
        retained = bytearray()
        remaining = len(body) + 1
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                break
            retained.extend(chunk)
            remaining -= len(chunk)
        written = os.fstat(descriptor)
        current = os.stat(target.name, dir_fd=parent_descriptor, follow_symlinks=False)
        observed = _read_report_file_at(parent_descriptor, target.name, len(body))
        if (
            bytes(retained) != body
            or observed != body
            or owned != (written.st_dev, written.st_ino)
            or not _same_report_node(written, current)
            or _report_file_identity(written) != _report_file_identity(current)
        ):
            raise OSError("published eval report changed during verification")
        _check_report_parent(
            requested_parent, canonical_parent, parent_descriptor, expected_parent
        )
    except (OSError, KeyboardInterrupt):
        cleanup_name = target.name if published else temporary_name
        if (published or staged) and owned is not None:
            try:
                current = os.stat(
                    cleanup_name,
                    dir_fd=parent_descriptor,
                    follow_symlinks=False,
                )
                if (current.st_dev, current.st_ino) == owned:
                    os.unlink(cleanup_name, dir_fd=parent_descriptor)
                    os.fsync(parent_descriptor)
            except FileNotFoundError:
                pass
        raise
    finally:
        if descriptor is not None:
            os.close(descriptor)
        os.close(parent_descriptor)


# ----- fixture lifecycle ----------------------------------------------------


def _run_git(
    args: list[str], cwd: Path, *, git_binary: str | Path = "git"
) -> None:
    """Run git with our scrubbed environment. Raises on non-zero exit."""
    proc = subprocess.run(
        [str(git_binary), *args],
        cwd=cwd,
        env=_git_environment(),
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"git {' '.join(args)} failed in {cwd}:\n"
            f"  stdout: {proc.stdout.strip()}\n  stderr: {proc.stderr.strip()}"
        )


def setup_fixture(
    fixture_name: str,
    baseline_ref: str,
    after_ref: str,
    *,
    staged_paths: list[str] | None = None,
    fixtures_dir: Path | None = None,
    git_binary: str | Path = "git",
    mmcg_binary: str | Path | None = MMCG_BIN,
) -> Path:
    """Build a real tmp git repo with a tagged baseline and an after-tree.

    Layout expected at `evals/fixtures/<name>/`:
        baseline/             — files at the baseline tag
        changes/<after_ref>/  — full after-tree, replaces baseline

    By default the after-tree is committed and tagged. Passing staged_paths
    leaves HEAD at baseline, stages those paths, and leaves all other changes
    unstaged or untracked. An empty list creates an entirely unstaged tree.

    Returns the tmp repo path. Caller is responsible for cleanup via
    `teardown_fixture`.
    """
    baseline_ref, after_ref = _validate_fixture_refs(baseline_ref, after_ref)
    _, _, baseline_src, after_src = fixture_case_roots(
        {
            "fixture": fixture_name,
            "baseline_ref": baseline_ref,
            "after_ref": after_ref,
        },
        fixtures_dir=fixtures_dir,
    )
    if not baseline_src.is_dir():
        raise FileNotFoundError(f"fixture baseline missing: {baseline_src}")
    if not after_src.is_dir():
        raise FileNotFoundError(f"fixture variant missing: {after_src}")
    staged_paths = _canonical_staged_paths(staged_paths)

    tmp = Path(tempfile.mkdtemp(prefix=f"mmcg-eval-{fixture_name}-"))
    complete = False
    try:
        # Phase 1: baseline tree.
        _copy_tree_into(baseline_src, tmp)
        _run_git(
            ["init", "-q", "--initial-branch=main"],
            tmp,
            git_binary=git_binary,
        )
        _run_git(["add", "-A"], tmp, git_binary=git_binary)
        _run_git(["commit", "-q", "-m", "baseline"], tmp, git_binary=git_binary)
        _run_git(["tag", baseline_ref], tmp, git_binary=git_binary)

        # Phase 2: replace working tree with `after` variant content.
        # We wipe everything except `.git/` then re-overlay so deletions are
        # reflected too (e.g. a variant that removes a file present in baseline).
        for entry in tmp.iterdir():
            if entry.name == ".git":
                continue
            if entry.is_dir():
                shutil.rmtree(entry)
            else:
                entry.unlink()
        _copy_tree_into(after_src, tmp)
        if staged_paths is None:
            _run_git(["add", "-A"], tmp, git_binary=git_binary)
            _run_git(
                [
                    "commit",
                    "-q",
                    "-m",
                    f"executor change ({after_ref})",
                    "--allow-empty",
                ],
                tmp,
                git_binary=git_binary,
            )
            _run_git(["tag", after_ref], tmp, git_binary=git_binary)
        elif staged_paths:
            _run_git(["add", "--", *staged_paths], tmp, git_binary=git_binary)

        # Phase 3: build an mmcg index of the after-tree so the auditor can run
        # real `mmcg_callers` / `mmcg_search` against the working state and
        # compare against the spec's pre-edit snapshot. Failure here is non-fatal
        # — the auditor can still operate on `git diff` alone.
        if mmcg_binary is not None:
            try:
                _build_mmcg_index(tmp, mmcg_binary=mmcg_binary)
            except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
                sys.stderr.write(f"  [fixture] mmcg index skipped: {error}\n")

        complete = True
        return tmp
    finally:
        if not complete:
            teardown_fixture(tmp)


def _build_mmcg_index(
    repo: Path, *, mmcg_binary: str | Path | None = MMCG_BIN
) -> None:
    """Run `mmcg index .` in `repo`, leaving `.mastermind/mmcg.db` behind.

    Uses the in-tree binary when available so the SQL schema matches the MCP
    server's expectations. Quiet on success — index summary is suppressed.
    """
    if mmcg_binary is None:
        raise FileNotFoundError("mmcg executable is unavailable")
    db_path = repo / ".mastermind" / "mmcg.db"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    proc = subprocess.run(
        [str(mmcg_binary), "--index", str(db_path), "index", str(repo)],
        cwd=repo,
        env=_PROC_ENV,
        capture_output=True,
        text=True,
        timeout=60,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"mmcg index failed: {proc.stderr.strip() or proc.stdout.strip()}"
        )


def _copy_tree_into(src: Path, dst: Path) -> None:
    """Copy content and modes, leaving fresh mtimes for Git change detection."""
    source_definition = fixture_tree_definition(src)
    source_entries = [(entry, entry.is_dir()) for entry in sorted(src.iterdir())]
    if dst.is_symlink():
        raise ValueError(f"fixture destination is a symbolic link: {dst}")
    dst.mkdir(parents=True, exist_ok=True)
    destination_root = dst.resolve()
    for entry, entry_is_directory in source_entries:
        target = dst / entry.name
        if target.is_symlink():
            raise ValueError(
                f"fixture destination contains a symbolic link: {target}"
            )
        if target.is_dir():
            fixture_tree_definition(target)
        if entry_is_directory:
            shutil.copytree(
                entry,
                target,
                dirs_exist_ok=True,
                copy_function=shutil.copy,
            )
        else:
            shutil.copy(entry, target)
    if fixture_tree_definition(src) != source_definition:
        raise ValueError(f"fixture tree changed while it was copied: {src}")
    for entry, entry_is_directory in source_entries:
        target = dst / entry.name
        try:
            target.resolve(strict=True).relative_to(destination_root)
        except (FileNotFoundError, ValueError) as error:
            raise ValueError(
                f"copied fixture path escaped its destination: {target}"
            ) from error
        if entry_is_directory:
            prefix = f"{entry.name}/"
            expected = [
                {**record, "path": record["path"][len(prefix):]}
                for record in source_definition
                if record["path"].startswith(prefix)
            ]
            observed = fixture_tree_definition(target)
        else:
            record = next(
                (
                    record
                    for record in source_definition
                    if record["path"] == entry.name
                ),
                None,
            )
            if record is None:
                raise ValueError(f"fixture tree changed while it was copied: {src}")
            expected = {
                "sha256": record["sha256"],
                "git_mode": record["git_mode"],
            }
            observed = _stable_regular_file_definition(target)
        if observed != expected:
            raise ValueError(
                f"copied fixture content does not match its source: {target}"
            )


def teardown_fixture(path: Path) -> None:
    shutil.rmtree(path, ignore_errors=True)


# ----- renderers ------------------------------------------------------------


def render_intake_input(inp: dict) -> str:
    target = inp.get("target_consumer", "planner")
    ctx = inp.get("project_context", "")
    ctx_block = f"\n\n**Project context:** {ctx}" if ctx else ""
    return (
        f"**Target consumer:** {target}{ctx_block}\n\n"
        f"**Raw prompt to refine:**\n\n{inp.get('raw_prompt', '')}"
    )


def render_critic_input(inp: dict) -> str:
    alternatives = inp.get("alternatives", "")
    if isinstance(alternatives, list):
        alternatives = "\n".join(f"- {a}" for a in alternatives)
    return (
        "**Evaluation boundary:** no repository checkout or tools are available. "
        "Assess only the supplied design and mmcg snapshot; do not invoke tools.\n\n"
        f"**Problem:** {inp.get('problem', '')}\n\n"
        f"**Proposed design:** {inp.get('design', '')}\n\n"
        f"**Alternatives considered:**\n{alternatives}\n\n"
        f"**Constraints:** {inp.get('constraints', '')}\n\n"
        f"**mmcg snapshot:** {inp.get('mmcg_snapshot', '')}"
    )


def render_researcher_input(
    inp: dict,
    *,
    fixture_path: Path | None = None,
    has_mmcg: bool = False,
) -> str:
    if fixture_path is None:
        boundary = (
            "no repository checkout or tools are available. Use only the quoted "
            "evidence and keep missing facts explicit."
        )
    else:
        boundary = (
            f"work only in the disposable repository at `{fixture_path}`. "
            f"mmcg is {'available from a freshly built index' if has_mmcg else 'unavailable'}; "
            "start with the requested query, not a status probe, and cite the "
            "source you verify."
        )
    evidence = inp.get("evidence", "")
    evidence_block = f"\n\n**Quoted evidence:**\n{evidence}" if evidence else ""
    return (
        f"**Evaluation boundary:** {boundary}\n\n"
        f"**Research question:** {inp.get('question', '')}\n\n"
        f"**Scope:** {inp.get('scope', '')}{evidence_block}"
    )


def isolated_cli_args(
    suite_name: str,
    *,
    subagent: Path | None = None,
    include_mmcg: bool = True,
) -> list[str]:
    """Deny repository tools to suites whose fixtures exist only in prompts."""
    if suite_name in {"critic", "intake", "workflow"}:
        return [
            "--safe-mode",
            "--tools",
            "",
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--no-chrome",
        ]
    if suite_name == "researcher":
        return [
            "--tools",
            "Read,Glob,Grep",
            "--allowedTools",
            ",".join(
                researcher_allowed_tools(
                    subagent, include_mmcg=include_mmcg
                )
            ),
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--no-chrome",
        ]
    if suite_name == "auditor":
        return [
            "--tools",
            "Read,Glob,Grep,Bash",
            "--allowedTools",
            ",".join(
                auditor_allowed_tools(subagent, include_mmcg=include_mmcg)
            ),
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--no-chrome",
        ]
    return []


def expected_stream_tools(
    suite_name: str,
    *,
    subagent: Path | None,
    include_mmcg: bool,
) -> tuple[str, ...]:
    if suite_name in {"critic", "intake", "workflow"}:
        return ()
    if subagent is None:
        raise ValueError(f"suite {suite_name!r} has no subagent definition")
    _, definition = subagent_runtime_definition(
        subagent, include_mmcg=include_mmcg
    )
    tools = definition.get("tools")
    if (
        not isinstance(tools, list)
        or any(not isinstance(tool, str) or not tool for tool in tools)
        or len(tools) != len(set(tools))
    ):
        raise ValueError(f"suite {suite_name!r} has an invalid tool inventory")
    # Claude Code keeps EndConversation when any other built-in or MCP tool is
    # available, even when it is omitted from --tools.
    return (*tools, "EndConversation") if tools else ()


def requires_prompt_sandbox(suite_name: str) -> bool:
    """Return whether a prompt-only suite needs a fresh, empty working tree."""
    return suite_name in {"critic", "intake", "workflow"}


def evaluation_cwd(
    suite_name: str,
    *,
    fixture_path: Path | None,
    prompt_sandbox: tempfile.TemporaryDirectory[str] | None,
) -> Path | None:
    if suite_name in {"auditor", "researcher"}:
        return fixture_path
    if prompt_sandbox is not None:
        return Path(prompt_sandbox.name)
    return None


def render_workflow_input(inp: dict) -> str:
    return str(inp.get("prompt", ""))


def workflow_prompt_path(
    case: dict, *, repository_root: Path | None = None
) -> Path:
    artifact = case.get("artifact")
    if artifact not in WORKFLOW_ARTIFACTS:
        raise ValueError(f"workflow artifact is not allowlisted: {artifact!r}")
    root = (REPO_ROOT if repository_root is None else repository_root).resolve()
    path = (root / artifact).resolve()
    try:
        path.relative_to(root)
    except ValueError as error:
        raise ValueError("workflow artifact must stay inside the repository") from error
    if not path.is_file():
        raise FileNotFoundError(f"workflow artifact not found: {path}")
    return path


def evaluation_target_digest(
    suite_name: str,
    suite_cfg: dict,
    selected: list[dict],
    *,
    workflow_root: Path | None = None,
) -> str:
    if suite_name == "workflow":
        records = [
            {
                "case_id": case["id"],
                "artifact": case["artifact"],
                **_stable_regular_file_definition(
                    workflow_prompt_path(case, repository_root=workflow_root)
                ),
            }
            for case in selected
        ]
    else:
        subagent = suite_cfg.get("subagent")
        if not isinstance(subagent, (str, os.PathLike)):
            raise ValueError(f"suite {suite_name!r} has no subagent definition")
        records = [
            {
                "artifact": "subagent",
                **_stable_regular_file_definition(Path(subagent)),
            }
        ]
    return hashlib.sha256(
        json.dumps(
            records,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode("utf-8")
    ).hexdigest()


def snapshot_evaluation_targets(
    suite_name: str,
    suite_cfg: dict,
    selected: list[dict],
    destination: Path,
) -> tuple[dict, Path | None]:
    if suite_name == "workflow":
        copied: set[str] = set()
        for case in selected:
            artifact = case["artifact"]
            if artifact in copied:
                continue
            source = workflow_prompt_path(case)
            _stable_regular_file_definition(source)
            target = destination / artifact
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(source, target)
            copied.add(artifact)
        return dict(suite_cfg), destination

    subagent = suite_cfg.get("subagent")
    if not isinstance(subagent, (str, os.PathLike)):
        raise ValueError(f"suite {suite_name!r} has no subagent definition")
    source = Path(subagent)
    _stable_regular_file_definition(source)
    target = destination / "subagent.md"
    shutil.copy(source, target)
    return {**suite_cfg, "subagent": target}, None


def render_auditor_input(
    inp: dict,
    *,
    fixture_path: Path,
    baseline_ref: str,
    after_ref: str,
    has_mmcg: bool,
    uncommitted: bool = False,
) -> str:
    """Build the auditor's user message.

    Crucially: NO synthetic git_diff. The auditor is told the working
    directory and baseline and is expected to inspect the current working
    tree, including untracked files, itself via Bash and Read. When `has_mmcg` is
    true, the auditor also has live `mmcg_callers` / `mmcg_search` MCP
    tools pointed at an index of the after-tree state.
    """
    mmcg_note = (
        "\n\n**mmcg available:** the working dir has a fresh `.mastermind/mmcg.db` "
        "indexed at the current working-tree state. You can call `mmcg_callers`, "
        "`mmcg_search`, `mmcg_outline` etc. via the MCP tools — use them to "
        "verify the spec's pre-edit symbol snapshot against the current state.\n"
    ) if has_mmcg else "\n"
    baseline_tag = fixture_tag_ref(baseline_ref, "baseline_ref")
    after_tag = fixture_tag_ref(after_ref, "after_ref")
    executor_state = (
        "**Executor state:** changes are uncommitted; HEAD remains at the baseline.\n\n"
        if uncommitted
        else f"**Executor commit tag:** `{after_tag}` (also inspect the current working tree)\n\n"
    )
    return (
        f"Audit the executor's work against the spec.\n\n"
        f"**Working directory:** `{fixture_path}` — a real git repo.\n"
        f"**Baseline tag:** `{baseline_tag}` (state before the executor ran)\n"
        f"{executor_state}"
        f"Use real `git diff {baseline_tag} --` for tracked changes, "
        f"`git status --porcelain=v1 --untracked-files=all` for staging state, and "
        f"`git ls-files --others --exclude-standard` for untracked paths. Read "
        f"untracked file contents directly; they are absent from `git diff`. "
        f"Do NOT trust the executor's narrative — verify each claim against "
        f"the current code."
        f"{mmcg_note}\n"
        f"**Spec summary:**\n{inp.get('spec_summary', '')}\n\n"
        f"**Executor report (what they claim they did):**\n```\n{inp.get('executor_report', '')}\n```"
    )


# ----- verdict extraction ---------------------------------------------------

_AUDIT_BLOCK_RE = re.compile(
    r"<!--\s*mastermind:audit-begin\s*-->.*?```ya?ml(.*?)```.*?<!--\s*mastermind:audit-end\s*-->",
    re.S,
)

_INTAKE_BLOCK_RE = re.compile(
    r"<!--\s*mastermind:intake-begin\s*-->.*?```ya?ml(.*?)```.*?<!--\s*mastermind:intake-end\s*-->",
    re.S,
)


_CRITIC_VERDICT_PATTERN = "|".join(
    re.escape(verdict) for verdict in sorted(CRITIC_VERDICTS, key=len, reverse=True)
)
_CRITIC_VERDICT_RE = re.compile(
    rf"^({_CRITIC_VERDICT_PATTERN})(?=\s*(?:$|[—–:-](?:\s|$)))",
    re.I,
)


def extract_critic_verdict(output: str) -> str | None:
    """Read the sole final Verdict section, excluding quoted Markdown examples."""
    lines = answer_lines(output)

    if any(
        re.match(r"^(?:[-+*]\s+)?(?:final\s+)?verdict\s*:", re.sub(r"[*_`]", "", line), re.I)
        for line in lines
    ):
        return None

    headings = [
        index for index, line in enumerate(lines)
        if re.fullmatch(r"##\s+Verdict(?:\s+#+)?", line, re.I)
    ]
    if len(headings) != 1:
        return None
    body = [line for line in lines[headings[0] + 1:] if line]
    if not body or any(re.match(r"^#{1,6}\s", line) for line in body):
        return None

    verdicts = []
    for index, line in enumerate(body):
        if index:
            line = re.sub(r"^(?:[-+*]|\d+[.)])\s+", "", line)
        line = re.sub(
            rf"^(\*\*|__|`|\*|_)({_CRITIC_VERDICT_PATTERN})\1(?=\s|$)",
            r"\2", line, flags=re.I,
        )
        verdicts.append(_CRITIC_VERDICT_RE.match(line))
    if verdicts[0] is None or sum(match is not None for match in verdicts) != 1:
        return None
    return verdicts[0][1].lower()


def extract_audit_data(output: str) -> dict | None:
    """Return the validated mapping from a structured audit tail, or None."""
    m = _AUDIT_BLOCK_RE.search(output)
    if not m:
        return None
    if not _YAML_AVAILABLE:
        return None
    try:
        data = _yaml.safe_load(m.group(1))
        if isinstance(data, dict):
            return data
    except Exception:
        pass
    return None


def extract_audit_verdict(output: str) -> str | None:
    """Return the `verdict` field from a structured audit tail, or None."""
    data = extract_audit_data(output)
    if data and data.get("verdict"):
        return str(data["verdict"]).lower().strip()
    return None


def audit_verification_passed(
    output: str,
    expected_command: str,
    tool_executions: list[ToolExecution],
) -> bool:
    data = extract_audit_data(output)
    if not data:
        return False
    reruns = data.get("verifications_rerun")
    if not isinstance(reruns, list):
        return False
    if any(
        not isinstance(entry, dict)
        or set(entry) != {"cmd", "result"}
        or not isinstance(entry["cmd"], str)
        or str(entry["result"]).lower() not in {"pass", "fail"}
        for entry in reruns
    ):
        return False
    attestations = [entry for entry in reruns if entry["cmd"] == expected_command]
    attested = (
        len(attestations) == 1
        and str(attestations[0]["result"]).lower() == "pass"
    )
    executions = [
        execution
        for execution in tool_executions
        if (
            execution.name == "Bash"
            and execution.arguments.get("command") == expected_command
            and execution.result_seen
        )
    ]
    executed = bool(executions) and executions[-1].succeeded
    return attested and executed


def extract_intake_action(output: str) -> str | None:
    """Return the `action` field from a structured intake metadata block, or None."""
    m = _INTAKE_BLOCK_RE.search(output)
    if not m:
        return None
    if not _YAML_AVAILABLE:
        return None
    try:
        data = _yaml.safe_load(m.group(1))
        if isinstance(data, dict) and data.get("action"):
            return str(data["action"]).lower().strip()
    except Exception:
        pass
    return None


def contains_any_phrase(output: str, phrases: list[str]) -> bool:
    """Accept one of several semantically equivalent case-insensitive phrases."""
    lowered = output.lower()
    return bool(phrases) and any(phrase.lower() in lowered for phrase in phrases)


_FENCED_CODE_RE = re.compile(r"```[^\n]*\n(.*?)```", re.DOTALL)


def _comment_start(line: str, prefixes: list[str]) -> int | None:
    """Find a comment marker outside simple quoted strings."""
    quote: str | None = None
    escaped = False
    for index, char in enumerate(line):
        if quote is not None:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in {"'", '"', "`"}:
            quote = char
            continue
        if any(line.startswith(prefix, index) for prefix in prefixes):
            return index
    return None


def extract_code_comments(output: str, prefixes: list[str]) -> tuple[list[str], bool]:
    """Return comment fragments from fenced code and whether code was fenced."""
    blocks = _FENCED_CODE_RE.findall(output)
    if not blocks:
        return [], False
    comments: list[str] = []
    for block in blocks:
        for line in block.splitlines():
            position = _comment_start(line, prefixes)
            if position is not None:
                comments.append(line[position:].strip())
    return comments, True


def code_comment_policy_reasons(output: str, policy: dict) -> list[str]:
    """Deterministically score comments in model-generated fenced code."""
    prefixes = policy.get("prefixes", ["//", "/*"])
    comments, fenced = extract_code_comments(output, prefixes)
    reasons: list[str] = []
    if policy.get("require_fenced_code", True) and not fenced:
        reasons.append("comment policy requires a fenced code block")
        return reasons
    minimum = policy.get("min", 0)
    maximum = policy.get("max")
    if len(comments) < minimum:
        reasons.append(f"found {len(comments)} code comment(s), expected at least {minimum}")
    if maximum is not None and len(comments) > maximum:
        reasons.append(
            f"found {len(comments)} code comment(s), expected at most {maximum}: {comments!r}"
        )
    comment_text = "\n".join(comments)
    for alternatives in policy.get("contains_any", []):
        if not contains_any_phrase(comment_text, alternatives):
            reasons.append(f"comments lack required alternatives: {alternatives!r}")
    for phrase in policy.get("not_contains", []):
        if phrase.lower() in comment_text.lower():
            reasons.append(f"forbidden comment phrase present: {phrase!r}")
    return reasons


# ----- per-case evaluator ---------------------------------------------------


def failed_evaluation_result(
    case_id: str,
    suite_name: str,
    reason: str,
    *,
    fixture_path: Path | None,
    telemetry_issue: str | None = None,
    telemetry: dict[str, object] | None = None,
    tool_calls: list[str] | None = None,
) -> Result:
    """Create a structurally valid failure without persisting model output."""
    if telemetry is None:
        telemetry = {
            "duration_ms": 0,
            "duration_api_ms": 0,
            "num_turns": 0,
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0,
            "cost_usd": 0.0,
            "resolved_models": [],
            "complete": False,
            "issues": [telemetry_issue or "evaluation ended before telemetry"],
        }
    return Result(
        case_id=case_id,
        suite=suite_name,
        passed=False,
        reasons=[reason],
        duration_ms=int(telemetry["duration_ms"]),
        fixture_path=fixture_path,
        duration_api_ms=int(telemetry["duration_api_ms"]),
        num_turns=int(telemetry["num_turns"]),
        input_tokens=int(telemetry["input_tokens"]),
        output_tokens=int(telemetry["output_tokens"]),
        cache_creation_input_tokens=int(
            telemetry["cache_creation_input_tokens"]
        ),
        cache_read_input_tokens=int(telemetry["cache_read_input_tokens"]),
        cost_usd=float(telemetry["cost_usd"]),
        telemetry_complete=bool(telemetry["complete"]),
        telemetry_issues=list(telemetry["issues"]),
        resolved_models=list(telemetry["resolved_models"]),
        tool_calls=[] if tool_calls is None else tool_calls,
    )


def evaluate_case(
    model: str,
    suite_name: str,
    suite_cfg: dict,
    case: dict,
    *,
    keep_fixtures: bool,
    fixtures_dir: Path | None = None,
    workflow_root: Path | None = None,
    claude_binary: str | Path = "claude",
    claude_version: str | None = None,
    git_binary: str | Path = "git",
    mmcg_binary: str | Path | None = MMCG_BIN,
) -> Result:
    case_id = case["id"]
    prompt_path = (
        workflow_prompt_path(case, repository_root=workflow_root)
        if suite_name == "workflow"
        else suite_cfg["subagent"]
    )
    system_prompt = strip_frontmatter(prompt_path.read_text())

    fixture_path: Path | None = None
    prompt_sandbox: tempfile.TemporaryDirectory[str] | None = None
    extra_cmd: list[str] = []
    has_mmcg = False
    try:
        if suite_cfg["uses_fixture"]:
            fixture_name = case["fixture"]
            baseline_ref = case["baseline_ref"]
            after_ref = case["after_ref"]
            staged_paths = case.get("staged_paths")
            fixture_path = setup_fixture(
                fixture_name,
                baseline_ref,
                after_ref,
                staged_paths=staged_paths,
                fixtures_dir=fixtures_dir,
                git_binary=git_binary,
                mmcg_binary=mmcg_binary,
            )

            db_path = fixture_path / ".mastermind" / "mmcg.db"
            has_mmcg = mmcg_binary is not None and db_path.is_file()
            if suite_name in {"auditor", "researcher"} and not has_mmcg and not case.get("allow_no_mmcg"):
                return failed_evaluation_result(
                    case_id,
                    suite_name,
                    "mmcg index unavailable — build the mmcg binary first "
                    "(`cargo build --release`)",
                    fixture_path=fixture_path,
                    telemetry_issue="evaluation did not start because mmcg was unavailable",
                )
            if suite_name == "auditor":
                user_message = render_auditor_input(
                    case["input"],
                    fixture_path=fixture_path,
                    baseline_ref=baseline_ref,
                    after_ref=after_ref,
                    has_mmcg=has_mmcg,
                    uncommitted=staged_paths is not None,
                )
            else:
                user_message = render_researcher_input(
                    case["input"],
                    fixture_path=fixture_path,
                    has_mmcg=has_mmcg,
                )
            extra_cmd = ["--add-dir", str(fixture_path)]
            if has_mmcg:
                # Spawn an mmcg MCP stdio server pointed at the fixture's index.
                # This matches the production custom-agent wiring.
                mcp_cfg = json.dumps({
                    "mcpServers": {
                        "mmcg": {
                            "command": str(mmcg_binary),
                            "args": ["--index", str(db_path), "serve"],
                        }
                    }
                })
                extra_cmd += ["--mcp-config", mcp_cfg]
        elif suite_name == "intake":
            user_message = render_intake_input(case["input"])
        elif suite_name == "workflow":
            user_message = render_workflow_input(case["input"])
        else:
            user_message = render_critic_input(case["input"])

        # Pass the user message via stdin — passing it as a positional arg
        # collides with `--add-dir <directories...>` (variadic), which would
        # swallow the message as another directory.
        if suite_name in {"auditor", "researcher"}:
            prompt_args: list[str] = []
            agent_args = subagent_cli_args(
                prompt_path,
                model_override=model,
                include_mmcg=has_mmcg,
            )
        else:
            prompt_flag = (
                "--system-prompt"
                if suite_name == "workflow"
                else "--append-system-prompt"
            )
            prompt_args = [prompt_flag, system_prompt]
            agent_args = []
        workflow_safety = isolated_cli_args(
            suite_name,
            subagent=(
                prompt_path if suite_name in {"auditor", "researcher"} else None
            ),
            include_mmcg=has_mmcg,
        )
        if requires_prompt_sandbox(suite_name):
            prompt_sandbox = tempfile.TemporaryDirectory(prefix="mastermind-eval-")
        case_cwd = evaluation_cwd(
            suite_name,
            fixture_path=fixture_path,
            prompt_sandbox=prompt_sandbox,
        )
        if case_cwd is None:
            raise ValueError(f"suite {suite_name!r} has no evaluation directory")
        runtime_contract = (
            None
            if claude_version is None
            else StreamRuntimeContract(
                cwd=str(case_cwd),
                tools=expected_stream_tools(
                    suite_name,
                    subagent=(
                        prompt_path
                        if suite_name in {"auditor", "researcher"}
                        else None
                    ),
                    include_mmcg=has_mmcg,
                ),
                mcp_servers=("mmcg",) if has_mmcg else (),
                claude_code_version=claude_stream_version(claude_version),
            )
        )
        runtime_limits = case_runtime_limits(suite_name, case.get("expect", {}))
        effort = evaluation_effort(suite_name, prompt_path)
        streamed_output = True
        cmd = [
            str(claude_binary),
            "-p",
            "--model", model,
            "--effort", effort,
            *prompt_args,
            *agent_args,
            "--output-format", "stream-json" if streamed_output else "json",
            *(["--verbose"] if streamed_output else []),
            "--no-session-persistence",
            "--permission-mode", "dontAsk",
            "--max-turns", str(runtime_limits["max_turns"]),
            *workflow_safety,
            *extra_cmd,
        ]

        # Auditor with real-git fixtures runs many Bash + Read tool calls per
        # case; observed range is ~120–280s on Sonnet, so the wall-clock cap
        # absorbs tail latency. Byte caps keep a broken or noisy CLI from
        # retaining an unbounded stream in memory. The process-group supervisor
        # also removes MCP descendants after every outcome.
        pinned_executables = (
            (git_binary,)
            if suite_cfg["uses_fixture"] and Path(git_binary).is_absolute()
            else ()
        )
        proc = run_bounded(
            cmd,
            cwd=case_cwd,
            env=evaluation_environment(
                runtime_limits["max_output_tokens"],
                pinned_executables=pinned_executables,
            ),
            stdin=user_message.encode("utf-8"),
            timeout=CLAUDE_CASE_TIMEOUT_SECONDS,
            stdout_limit=CLAUDE_STDOUT_LIMIT_BYTES,
            stderr_limit=CLAUDE_STDERR_LIMIT_BYTES,
            start_new_session=True,
        )
        if proc.stop_reason is not None:
            reason = {
                "timeout": f"timeout after {CLAUDE_CASE_TIMEOUT_SECONDS}s",
                "output_limit": "Claude process output exceeded transport limits",
                "spawn_error": "cannot start Claude CLI",
                "unsupported_platform": (
                    "bounded Claude process transport requires POSIX"
                ),
            }.get(proc.stop_reason, "Claude process stopped by transport")
            return failed_evaluation_result(
                case_id,
                suite_name,
                reason,
                fixture_path=fixture_path,
                telemetry_issue=reason,
            )

        if proc.returncode != 0:
            reason = f"claude exit {proc.returncode}"
            return failed_evaluation_result(
                case_id,
                suite_name,
                reason,
                fixture_path=fixture_path,
                telemetry_issue="Claude exited before valid telemetry",
            )

        try:
            stdout = proc.stdout.decode("utf-8")
        except UnicodeDecodeError:
            return failed_evaluation_result(
                case_id,
                suite_name,
                "invalid Claude stream encoding",
                fixture_path=fixture_path,
                telemetry_issue="Claude stream encoding was invalid",
            )

        permission_denials: list[dict] = []
        tool_calls: list[str] = []
        tool_executions: list[ToolExecution] = []
        telemetry: dict[str, object] = telemetry_from_payload({})
        try:
            payload, tool_calls, tool_executions = parse_claude_output(
                stdout,
                streamed=streamed_output,
                runtime_contract=runtime_contract,
            )
            output = payload.get("result", "")
            if not isinstance(output, str):
                output = json.dumps(output, sort_keys=True, ensure_ascii=False)
            telemetry = telemetry_from_payload(payload)
            duration_ms = int(telemetry["duration_ms"])
            raw_denials = payload.get("permission_denials", [])
            if isinstance(raw_denials, list):
                permission_denials = [
                    denial for denial in raw_denials if isinstance(denial, dict)
                ]
        except (json.JSONDecodeError, TypeError, ValueError) as error:
            return failed_evaluation_result(
                case_id,
                suite_name,
                f"invalid Claude stream: {error}",
                fixture_path=fixture_path,
                telemetry_issue="Claude stream was invalid",
            )

        if telemetry["complete"] is not True:
            return failed_evaluation_result(
                case_id,
                suite_name,
                "incomplete Claude telemetry: " + "; ".join(telemetry["issues"]),
                fixture_path=fixture_path,
                telemetry=telemetry,
                tool_calls=tool_calls,
            )
        if permission_denials:
            denied_tools = sorted(
                {
                    denial.get("tool_name", "unknown")
                    if isinstance(denial.get("tool_name"), str)
                    else "unknown"
                    for denial in permission_denials
                }
            )
            return failed_evaluation_result(
                case_id,
                suite_name,
                f"permission denied for tools: {denied_tools!r}",
                fixture_path=fixture_path,
                telemetry=telemetry,
                tool_calls=tool_calls,
            )

        expect = case.get("expect", {})
        reasons: list[str] = []
        passed = True

        usage_reasons = usage_budget_reasons(expect, telemetry)
        if usage_reasons:
            passed = False
            reasons.extend(usage_reasons)

        tool_reasons = tool_usage_reasons(expect, tool_calls)
        if tool_reasons:
            passed = False
            reasons.extend(tool_reasons)

        expected_action = expect.get("action")
        if expected_action and suite_name == "intake":
            structured = extract_intake_action(output)
            if structured is not None:
                if structured != expected_action.lower():
                    passed = False
                    reasons.append(
                        f"intake action {structured!r} != expected {expected_action!r}"
                    )
            else:
                passed = False
                reasons.append(
                    "no structured intake metadata block found "
                    "(<!-- mastermind:intake-begin --> ... <!-- mastermind:intake-end -->)"
                )

        expected_verdict = expect.get("verdict")
        if expected_verdict:
            candidates = (
                [expected_verdict]
                if isinstance(expected_verdict, str)
                else list(expected_verdict)
            )
            if suite_name == "auditor":
                structured = extract_audit_verdict(output)
                if structured is not None:
                    if not any(v.lower() == structured for v in candidates):
                        passed = False
                        reasons.append(
                            f"structured verdict {structured!r} not in expected {candidates}"
                        )
                else:
                    passed = False
                    reasons.append(
                        "no structured audit verdict block found "
                        "(<!-- mastermind:audit-begin --> ... <!-- mastermind:audit-end -->)"
                    )
            elif suite_name == "critic" or (
                suite_name == "workflow"
                and case.get("artifact") == "skills/workflow/mastermind-critical-review/SKILL.md"
            ):
                invalid_candidates = [
                    candidate for candidate in candidates
                    if candidate.lower() not in CRITIC_VERDICTS
                ]
                if invalid_candidates:
                    passed = False
                    reasons.append(f"invalid expected critic verdicts: {invalid_candidates}")
                verdict = extract_critic_verdict(output)
                if verdict is None:
                    passed = False
                    reasons.append("no single valid final critic Verdict section found")
                elif verdict not in {candidate.lower() for candidate in candidates}:
                    passed = False
                    reasons.append(f"critic verdict {verdict!r} not in expected {candidates}")
            elif not any(
                re.search(rf"\b{re.escape(v)}\b", output, re.IGNORECASE)
                for v in candidates
            ):
                passed = False
                reasons.append(f"none of expected verdicts {candidates} found")

        expected_rerun = expect.get("verification_rerun")
        if expected_rerun and suite_name == "auditor":
            if not audit_verification_passed(
                output, expected_rerun, tool_executions
            ):
                passed = False
                reasons.append(
                    f"structured audit did not record a passing rerun of {expected_rerun!r}"
                )

        for phrase in expect.get("contains", []):
            if phrase.lower() not in output.lower():
                passed = False
                reasons.append(f"missing phrase: {phrase!r}")

        for alternatives in expect.get("contains_any", []):
            if not isinstance(alternatives, list) or not all(
                isinstance(phrase, str) for phrase in alternatives
            ):
                passed = False
                reasons.append(f"invalid contains_any group: {alternatives!r}")
            elif not contains_any_phrase(output, alternatives):
                passed = False
                reasons.append(f"none of alternative phrases found: {alternatives!r}")

        for phrase in expect.get("not_contains", []):
            if phrase.lower() in output.lower():
                passed = False
                reasons.append(f"forbidden phrase present: {phrase!r}")

        comment_policy = expect.get("code_comments")
        if comment_policy is not None:
            comment_reasons = code_comment_policy_reasons(output, comment_policy)
            if comment_reasons:
                passed = False
                reasons.extend(comment_reasons)

        citation_checks = None
        if "citations" in expect:
            citation_checks = check_citations(output, expect["citations"], fixture_path)
            if citation_checks["issues"]:
                passed = False
                reasons.extend(citation_checks["issues"])

        return Result(
            case_id=case_id,
            suite=suite_name,
            passed=passed,
            reasons=reasons,
            duration_ms=duration_ms,
            fixture_path=fixture_path,
            output_excerpt=output[:4000],
            duration_api_ms=int(telemetry["duration_api_ms"]),
            num_turns=int(telemetry["num_turns"]),
            input_tokens=int(telemetry["input_tokens"]),
            output_tokens=int(telemetry["output_tokens"]),
            cache_creation_input_tokens=int(
                telemetry["cache_creation_input_tokens"]
            ),
            cache_read_input_tokens=int(telemetry["cache_read_input_tokens"]),
            cost_usd=float(telemetry["cost_usd"]),
            telemetry_complete=bool(telemetry["complete"]),
            telemetry_issues=list(telemetry["issues"]),
            resolved_models=list(telemetry["resolved_models"]),
            tool_calls=tool_calls,
            citation_checks=citation_checks,
        )
    finally:
        if prompt_sandbox is not None:
            prompt_sandbox.cleanup()
        if fixture_path is not None and not keep_fixtures:
            teardown_fixture(fixture_path)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--suite", choices=list(SUITES.keys()), help="run one suite only")
    parser.add_argument("--case", help="run one case by id")
    parser.add_argument(
        "--model", default="opus",
        help="model alias (opus/sonnet/haiku) or full name; default opus",
    )
    parser.add_argument(
        "--keep-fixtures", action="store_true",
        help="don't delete fixture tmp dirs after each case (debugging)",
    )
    parser.add_argument(
        "--verbose-failures", action="store_true",
        help="print up to 4000 characters of model output for failed cases",
    )
    parser.add_argument(
        "--report",
        type=Path,
        help="atomically write a schema-versioned JSON telemetry report",
    )
    parser.add_argument(
        "--baseline-report",
        type=Path,
        help=(
            "compare the same model and case set; require non-regressed quality "
            "and lower p50/p95 context tokens"
        ),
    )
    args = parser.parse_args()

    try:
        frozen_harness_definition = evaluation_harness_definition()
    except (OSError, ValueError) as error:
        print(f"error: cannot identify evaluation harness: {error}", file=sys.stderr)
        return 2

    claude_location = shutil.which("claude")
    if not claude_location:
        print(
            "error: `claude` CLI not on PATH. Install Claude Code: https://claude.com/claude-code",
            file=sys.stderr,
        )
        return 2
    try:
        claude_binary = Path(claude_location).resolve(strict=True)
        claude_definition = _stable_regular_file_definition(claude_binary)
        frozen_claude_version = claude_cli_version(claude_binary)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"error: cannot freeze `claude` CLI: {error}", file=sys.stderr)
        return 2
    if frozen_claude_version is None:
        print("error: cannot read `claude` CLI version", file=sys.stderr)
        return 2
    suites_to_run = [args.suite] if args.suite else list(SUITES.keys())
    results: list[Result] = []
    case_definition_digests: dict[str, str] = {}
    case_definition_stability: dict[str, bool] = {}
    target_definition_digests: dict[str, str] = {}
    target_definition_stability: dict[str, bool] = {}
    git_binary: Path | None = None
    mmcg_binary: Path | None = None
    frozen_fixture_runtime: dict[str, object] | None = None

    for suite_name in suites_to_run:
        suite_cfg = SUITES[suite_name]
        if not suite_cfg["cases"].exists():
            print(f"  skip suite {suite_name} — no cases file at {suite_cfg['cases']}")
            continue
        try:
            suite_cases = [
                case
                for case in load_case_records(
                    suite_cfg["cases"], suite_name=suite_name
                )
                if not args.case or case["id"] == args.case
            ]
        except (OSError, ValueError) as error:
            print(f"error: loading suite {suite_name}: {error}", file=sys.stderr)
            return 2
        if not suite_cases:
            continue

        if suite_cfg["uses_fixture"] and frozen_fixture_runtime is None:
            git_location = shutil.which("git")
            if not git_location:
                print(
                    "error: `git` not on PATH (required for fixture suites).",
                    file=sys.stderr,
                )
                return 2
            try:
                git_binary = Path(git_location).resolve(strict=True)
                mmcg_location = shutil.which(str(MMCG_BIN))
                mmcg_binary = (
                    None
                    if mmcg_location is None
                    else Path(mmcg_location).resolve(strict=True)
                )
                frozen_fixture_runtime = fixture_runtime_definition(
                    git_binary, mmcg_binary
                )
            except (OSError, RuntimeError, ValueError) as error:
                print(
                    f"error: cannot freeze fixture runtime: {error}",
                    file=sys.stderr,
                )
                return 2

        print(f"\n=== {suite_name} ===")
        try:
            live_digest = case_definition_digest_from_records(
                suite_name, suite_cases
            )
        except (KeyError, OSError, ValueError) as error:
            print(
                f"error: freezing suite {suite_name} definition: {error}",
                file=sys.stderr,
            )
            return 2

        fixture_snapshot: tempfile.TemporaryDirectory[str] | None = None
        fixtures_dir_for_run: Path | None = None
        if suite_cfg["uses_fixture"]:
            fixture_snapshot = tempfile.TemporaryDirectory(
                prefix=f"mastermind-eval-{suite_name}-definition-"
            )
            fixtures_dir_for_run = Path(fixture_snapshot.name)
            try:
                snapshot_fixture_definitions(suite_cases, fixtures_dir_for_run)
                frozen_digest = case_definition_digest_from_records(
                    suite_name,
                    suite_cases,
                    fixtures_dir=fixtures_dir_for_run,
                )
                live_digest_after_snapshot = case_definition_digest_from_records(
                    suite_name, suite_cases
                )
            except (KeyError, OSError, ValueError) as error:
                fixture_snapshot.cleanup()
                print(
                    f"error: freezing suite {suite_name} fixtures: {error}",
                    file=sys.stderr,
                )
                return 2
            if not (
                live_digest == frozen_digest == live_digest_after_snapshot
            ):
                fixture_snapshot.cleanup()
                print(
                    f"error: suite {suite_name} fixtures changed while being frozen",
                    file=sys.stderr,
                )
                return 2
        else:
            frozen_digest = live_digest

        target_snapshot = tempfile.TemporaryDirectory(
            prefix=f"mastermind-eval-{suite_name}-target-"
        )
        target_snapshot_root = Path(target_snapshot.name)
        try:
            live_target_digest = evaluation_target_digest(
                suite_name, suite_cfg, suite_cases
            )
            suite_cfg_for_run, workflow_root_for_run = snapshot_evaluation_targets(
                suite_name,
                suite_cfg,
                suite_cases,
                target_snapshot_root,
            )
            frozen_target_digest = evaluation_target_digest(
                suite_name,
                suite_cfg_for_run,
                suite_cases,
                workflow_root=workflow_root_for_run,
            )
            live_target_digest_after_snapshot = evaluation_target_digest(
                suite_name, suite_cfg, suite_cases
            )
        except (KeyError, OSError, ValueError) as error:
            target_snapshot.cleanup()
            if fixture_snapshot is not None:
                fixture_snapshot.cleanup()
            print(
                f"error: freezing suite {suite_name} target: {error}",
                file=sys.stderr,
            )
            return 2
        if not (
            live_target_digest
            == frozen_target_digest
            == live_target_digest_after_snapshot
        ):
            target_snapshot.cleanup()
            if fixture_snapshot is not None:
                fixture_snapshot.cleanup()
            print(
                f"error: suite {suite_name} target changed while being frozen",
                file=sys.stderr,
            )
            return 2

        case_definition_digests[suite_name] = frozen_digest
        case_definition_stability[suite_name] = True
        target_definition_digests[suite_name] = frozen_target_digest
        target_definition_stability[suite_name] = True
        suite_result_start = len(results)
        try:
            for case in suite_cases:
                print(f"  [{case['id']}] running ...", end=" ", flush=True)
                r = evaluate_case(
                    args.model, suite_name, suite_cfg_for_run, case,
                    keep_fixtures=args.keep_fixtures,
                    fixtures_dir=fixtures_dir_for_run,
                    workflow_root=workflow_root_for_run,
                    claude_binary=claude_binary,
                    claude_version=frozen_claude_version,
                    git_binary=git_binary or "git",
                    mmcg_binary=mmcg_binary,
                )
                _SENTINEL_MISSING = "no structured audit verdict block found"
                if (
                    not r.passed
                    and suite_name == "auditor"
                    and len(r.reasons) == 1
                    and _SENTINEL_MISSING in r.reasons[0]
                ):
                    print(f"retry (sentinel missing) ...", end=" ", flush=True)
                    r2 = evaluate_case(
                        args.model, suite_name, suite_cfg_for_run, case,
                        keep_fixtures=args.keep_fixtures,
                        fixtures_dir=fixtures_dir_for_run,
                        workflow_root=workflow_root_for_run,
                        claude_binary=claude_binary,
                        claude_version=frozen_claude_version,
                        git_binary=git_binary or "git",
                        mmcg_binary=mmcg_binary,
                    )
                    r2.retry_attempted = True
                    r2.add_attempt(r)
                    if r2.passed:
                        r2.retry_used = True
                    r = r2
                results.append(r)
                status = "✓ pass" if r.passed else "✗ FAIL"
                retry_tag = " [retry]" if r.retry_used else ""
                print(
                    f"{status}{retry_tag}  ({r.duration_ms}ms, {r.num_turns} turns, "
                    f"tokens {r.input_tokens} in/{r.output_tokens} out, "
                    f"cache {r.cache_creation_input_tokens} write/"
                    f"{r.cache_read_input_tokens} read, ${r.cost_usd:.4f})"
                )
                if args.keep_fixtures and r.fixture_path is not None:
                    print(f"      fixture: {r.fixture_path}")
                for reason in r.reasons:
                    print(f"      → {reason}")
                if args.verbose_failures and not r.passed and r.output_excerpt:
                    print("      --- model output ---")
                    for line in r.output_excerpt.splitlines():
                        print(f"      {line}")
        finally:
            try:
                frozen_case_digest_after_run = (
                    frozen_digest
                    if fixtures_dir_for_run is None
                    else case_definition_digest_from_records(
                        suite_name,
                        suite_cases,
                        fixtures_dir=fixtures_dir_for_run,
                    )
                )
            except (KeyError, OSError, ValueError):
                frozen_case_digest_after_run = None
            try:
                frozen_target_digest_after_run = evaluation_target_digest(
                    suite_name,
                    suite_cfg_for_run,
                    suite_cases,
                    workflow_root=workflow_root_for_run,
                )
            except (KeyError, OSError, ValueError):
                frozen_target_digest_after_run = None
            target_snapshot.cleanup()
            if fixture_snapshot is not None:
                fixture_snapshot.cleanup()

        suite_case_ids = [case["id"] for case in suite_cases]
        definition_stable = (
            case_definition_digest(suite_name, suite_case_ids)
            == frozen_case_digest_after_run
            == frozen_digest
        )
        case_definition_stability[suite_name] = definition_stable
        if not definition_stable:
            reason = "case or fixture definition changed during evaluation"
            for result in results[suite_result_start:]:
                result.passed = False
                if reason not in result.reasons:
                    result.reasons.append(reason)
            print(f"  ✗ FAIL  {reason}")
        try:
            live_target_digest_after_run = evaluation_target_digest(
                suite_name, suite_cfg, suite_cases
            )
        except (KeyError, OSError, ValueError):
            live_target_digest_after_run = None
        target_stable = (
            live_target_digest_after_run
            == frozen_target_digest_after_run
            == frozen_target_digest
        )
        target_definition_stability[suite_name] = target_stable
        if not target_stable:
            reason = "evaluated agent or skill changed during evaluation"
            for result in results[suite_result_start:]:
                result.passed = False
                if reason not in result.reasons:
                    result.reasons.append(reason)
            print(f"  ✗ FAIL  {reason}")

    if not results:
        print("\nno cases matched filter")
        return 2

    try:
        claude_definition_after_run = _stable_regular_file_definition(
            claude_binary
        )
        claude_version_after_run = claude_cli_version(claude_binary)
    except (OSError, ValueError):
        claude_definition_after_run = None
        claude_version_after_run = None
    claude_stable = (
        claude_definition_after_run == claude_definition
        and claude_version_after_run == frozen_claude_version
    )
    if not claude_stable:
        reason = "Claude CLI changed during evaluation"
        for result in results:
            result.passed = False
            if reason not in result.reasons:
                result.reasons.append(reason)
        print(f"  ✗ FAIL  {reason}")
    try:
        harness_definition_after_run = evaluation_harness_definition()
    except (OSError, ValueError):
        harness_definition_after_run = None
    harness_stable = harness_definition_after_run == frozen_harness_definition
    if not harness_stable:
        reason = "evaluation harness changed during evaluation"
        for result in results:
            result.passed = False
            if reason not in result.reasons:
                result.reasons.append(reason)
        print(f"  ✗ FAIL  {reason}")
    fixture_runtime_stable = True
    if frozen_fixture_runtime is not None:
        try:
            fixture_runtime_after_run = fixture_runtime_definition(
                git_binary, mmcg_binary
            )
        except (OSError, ValueError):
            fixture_runtime_after_run = None
        fixture_runtime_stable = (
            fixture_runtime_after_run == frozen_fixture_runtime
        )
        if not fixture_runtime_stable:
            reason = "fixture runtime changed during evaluation"
            for result in results:
                if not SUITES[result.suite]["uses_fixture"]:
                    continue
                result.passed = False
                if reason not in result.reasons:
                    result.reasons.append(reason)
            print(f"  ✗ FAIL  {reason}")

    n_pass = sum(r.passed for r in results)
    n_fail = len(results) - n_pass
    n_first_pass = sum(r.passed and not r.retry_used for r in results)
    n_retry_attempted = sum(r.retry_attempted for r in results)
    n_retry_pass = sum(r.passed and r.retry_used for r in results)
    total_ms = sum(r.duration_ms for r in results)
    total_api_ms = sum(r.duration_api_ms for r in results)
    total_turns = sum(r.num_turns for r in results)
    total_input = sum(r.input_tokens for r in results)
    total_output = sum(r.output_tokens for r in results)
    total_cache_creation = sum(r.cache_creation_input_tokens for r in results)
    total_cache_read = sum(r.cache_read_input_tokens for r in results)
    total_cost = sum(r.cost_usd for r in results)
    print(f"\n=== summary ===")
    print(f"  passed: {n_pass}/{len(results)}")
    print(f"  first_pass: {n_first_pass}/{len(results)}")
    if n_retry_attempted:
        print(f"  retry_attempted: {n_retry_attempted}/{len(results)}")
    if n_retry_pass:
        print(f"  after_retry: {n_first_pass + n_retry_pass}/{len(results)} ({n_retry_pass} case(s) passed on retry)")
    print(f"  total time: {total_ms / 1000:.1f}s")
    print(f"  API time: {total_api_ms / 1000:.1f}s across {total_turns} turns")
    print(
        "  tokens: "
        f"{total_input} input, {total_output} output, "
        f"{total_cache_creation} cache write, {total_cache_read} cache read"
    )
    print(f"  reported cost: ${total_cost:.4f}")

    report = build_report(
        results,
        model=args.model,
        suite_filter=args.suite,
        case_filter=args.case,
        case_definition_digests=case_definition_digests,
        case_definition_stability=case_definition_stability,
        target_definition_digests=target_definition_digests,
        target_definition_stability=target_definition_stability,
        claude_version=frozen_claude_version,
        claude_sha256=claude_definition["sha256"],
        claude_stable=claude_stable,
        harness_definition=frozen_harness_definition,
        harness_stable=harness_stable,
        fixture_runtime=frozen_fixture_runtime,
        fixture_runtime_stable=fixture_runtime_stable,
    )
    for suite_name, summary in report["suites"].items():
        context = summary["usage"]["context_tokens"]
        print(
            f"  {suite_name} context tokens: "
            f"p50 {context['p50']}, p95 {context['p95']}"
        )

    gate_failed = False
    if args.baseline_report is not None:
        try:
            baseline = load_report(args.baseline_report)
            gate = compare_to_baseline(report, baseline)
        except ValueError as error:
            gate = {"passed": False, "checks": [], "failures": [str(error)]}
        report["baseline_gate"] = {
            "baseline": str(args.baseline_report),
            **gate,
        }
        gate_failed = not gate["passed"]
        print(f"  baseline gate: {'pass' if gate['passed'] else 'FAIL'}")
        for failure in gate["failures"]:
            print(f"      → {failure}")

    if args.report is not None:
        try:
            write_report(args.report, report)
        except OSError as error:
            print(f"error: writing eval report {args.report}: {error}", file=sys.stderr)
            return 2
        print(f"  report: {args.report}")

    return 0 if n_fail == 0 and not gate_failed else 1


if __name__ == "__main__":
    sys.exit(main())
