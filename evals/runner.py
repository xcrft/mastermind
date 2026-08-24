#!/usr/bin/env python3
"""
Local eval runner for mastermind subagents.

Uses the `claude` CLI in non-interactive mode (`-p`), so authentication runs
through your existing Claude Code login — no ANTHROPIC_API_KEY needed, costs
count against your Claude subscription (flat monthly, not per-token).

Auditor cases use **real git fixtures** — each case names a fixture under
`evals/fixtures/<name>/` plus two refs (`baseline_ref` + `after_ref`). The
runner builds a real tmp git repo with those two commits/tags and hands the
path to the auditor via `--add-dir`. The auditor runs `git diff`, `git log`,
etc. itself against actual hunks. No synthetic paraphrased diff strings.

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
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from datetime import datetime, timezone
from functools import lru_cache
from pathlib import Path

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
    "GIT_COMMITTER_NAME": "mmcg-eval",
    "GIT_COMMITTER_EMAIL": "eval@mastermind.local",
    # Avoid GPG signing inside fixtures (some dev machines force it globally).
    "GIT_CONFIG_COUNT": "1",
    "GIT_CONFIG_KEY_0": "commit.gpgsign",
    "GIT_CONFIG_VALUE_0": "false",
}

# Base env for all subprocesses spawned by the runner.
# TERM=dumb prevents tput from emitting warnings in non-interactive shells
# (CI, piped output, shells where $TERM is unset). Prefer the caller's TERM
# when available so color hints still work in interactive mode.
_PROC_ENV: dict[str, str] = {**os.environ, "TERM": os.environ.get("TERM") or "dumb"}

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


def strip_frontmatter(text: str) -> str:
    if text.startswith("---\n"):
        end = text.find("\n---\n", 4)
        if end != -1:
            return text[end + 5 :].lstrip()
    return text


def subagent_runtime_definition(
    path: Path, *, model_override: str | None = None
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
    if model_override is not None:
        definition["model"] = model_override
    return name, definition


def subagent_cli_args(path: Path, *, model_override: str) -> list[str]:
    """Run the eval through the same custom-agent boundary as production."""
    name, definition = subagent_runtime_definition(
        path, model_override=model_override
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


def auditor_allowed_tools() -> tuple[str, ...]:
    return AUDITOR_SAFE_ALLOWED_TOOLS + subagent_mcp_tools(
        SUITES["auditor"]["subagent"]
    )


def researcher_allowed_tools() -> tuple[str, ...]:
    return ("Read", "Glob", "Grep") + subagent_mcp_tools(
        SUITES["researcher"]["subagent"]
    )


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
    if (
        not isinstance(model_usage, dict)
        or not model_usage
        or any(not isinstance(name, str) or not name for name in model_usage)
    ):
        issues.append("modelUsage must identify at least one usage model")
    resolved_model = payload.get("_resolved_model")
    if not isinstance(resolved_model, str) or not resolved_model:
        issues.append("stream init must identify the resolved primary model")
        resolved_models: list[str] = []
    else:
        resolved_models = [resolved_model]
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


def parse_claude_output(stdout: str, *, streamed: bool) -> tuple[dict, list[str]]:
    if not streamed:
        payload = json.loads(stdout)
        if not isinstance(payload, dict):
            raise ValueError("Claude JSON result is not an object")
        model_usage = payload.get("modelUsage")
        if isinstance(model_usage, dict) and len(model_usage) == 1:
            payload["_resolved_model"] = next(iter(model_usage))
        return payload, []

    events: list[dict] = []
    for line_number, line in enumerate(stdout.splitlines(), start=1):
        if not line.strip():
            continue
        event = json.loads(line)
        if not isinstance(event, dict):
            raise ValueError(f"Claude stream event {line_number} is not an object")
        events.append(event)
    payload = next(
        (event for event in reversed(events) if event.get("type") == "result"),
        None,
    )
    if payload is None:
        raise ValueError("Claude stream has no final result event")
    init_models = {
        event.get("model")
        for event in events
        if event.get("type") == "system"
        and event.get("subtype") == "init"
        and isinstance(event.get("model"), str)
        and event.get("model")
    }
    if len(init_models) != 1:
        raise ValueError(
            f"Claude stream must identify one primary model, got {sorted(init_models)!r}"
        )
    payload["_resolved_model"] = next(iter(init_models))

    tool_calls: list[str] = []
    for event in events:
        if event.get("type") != "assistant":
            continue
        message = event.get("message")
        content = message.get("content") if isinstance(message, dict) else None
        if not isinstance(content, list):
            continue
        for block in content:
            if not isinstance(block, dict) or block.get("type") != "tool_use":
                continue
            name = block.get("name")
            if isinstance(name, str) and name:
                tool_calls.append(name)
    return payload, tool_calls


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
    }


def fixture_tree_definition(root: Path) -> list[dict[str, str]]:
    if not root.is_dir():
        raise FileNotFoundError(f"fixture tree missing: {root}")
    records: list[dict[str, str]] = []
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"fixture tree contains a symbolic link: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"fixture tree contains an unsupported artifact: {path}")
        records.append(
            {
                "path": path.relative_to(root).as_posix(),
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
        )
    return records


def case_definition_digest(suite_name: str, case_ids: list[str]) -> str | None:
    suite = SUITES.get(suite_name)
    if not suite:
        return None
    try:
        records = [
            json.loads(line)
            for line in suite["cases"].read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.lstrip().startswith("//")
        ]
    except (OSError, json.JSONDecodeError):
        return None
    by_id = {record.get("id"): record for record in records if isinstance(record, dict)}
    if len(by_id) != len(records) or any(case_id not in by_id for case_id in case_ids):
        return None
    selected = [by_id[case_id] for case_id in case_ids]
    canonical = json.dumps(
        selected,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    if suite.get("uses_fixture"):
        fixtures: list[dict[str, object]] = []
        try:
            for record in selected:
                fixture_name = record["fixture"]
                after_ref = record["after_ref"]
                if not isinstance(fixture_name, str) or not isinstance(after_ref, str):
                    raise ValueError("fixture names and refs must be strings")
                fixture_root = (FIXTURES_DIR / fixture_name).resolve()
                fixture_root.relative_to(FIXTURES_DIR.resolve())
                fixtures.append(
                    {
                        "case_id": record["id"],
                        "baseline": fixture_tree_definition(fixture_root / "baseline"),
                        "after": fixture_tree_definition(
                            fixture_root / "changes" / after_ref
                        ),
                    }
                )
        except (KeyError, OSError, ValueError):
            return None
        canonical += b"\0fixture-trees\0" + json.dumps(
            fixtures,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode("utf-8")
    return hashlib.sha256(canonical).hexdigest()


def suite_report(results: list[Result]) -> dict:
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
    return {
        "case_ids": case_ids,
        "case_definition_digest": case_definition_digest(results[0].suite, case_ids),
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


def git_revision() -> str | None:
    proc = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=REPO_ROOT,
        env=_PROC_ENV,
        text=True,
        capture_output=True,
        check=False,
    )
    revision = proc.stdout.strip()
    return revision if proc.returncode == 0 and revision else None


@lru_cache(maxsize=1)
def claude_cli_version() -> str | None:
    try:
        proc = subprocess.run(
            ["claude", "--version"],
            env=_PROC_ENV,
            text=True,
            capture_output=True,
            check=False,
        )
    except OSError:
        return None
    version = proc.stdout.strip()
    return version if proc.returncode == 0 and version else None


def build_report(
    results: list[Result],
    *,
    model: str,
    suite_filter: str | None,
    case_filter: str | None,
) -> dict:
    suites: dict[str, list[Result]] = {}
    for result in results:
        suites.setdefault(result.suite, []).append(result)
    return {
        "kind": REPORT_KIND,
        "schema_version": REPORT_SCHEMA_VERSION,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "git_revision": git_revision(),
        "model": model,
        "resolved_models": sorted(
            {name for result in results for name in result.resolved_models}
        ),
        "claude_cli_version": claude_cli_version(),
        "filters": {"suite": suite_filter, "case": case_filter},
        "suites": {
            name: suite_report(suite_results)
            for name, suite_results in suites.items()
        },
        "cases": [result_report(result) for result in results],
    }


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
        and all(isinstance(item, str) and item for item in value)
        and (not unique or len(value) == len(set(value)))
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


def report_comparison_issues(report: object, label: str) -> list[str]:
    issues: list[str] = []
    if not isinstance(report, dict):
        return [f"{label} report must be a JSON object"]
    if report.get("kind") != REPORT_KIND:
        issues.append(f"{label} report has an unsupported kind")
    if report.get("schema_version") != REPORT_SCHEMA_VERSION:
        issues.append(f"{label} report has an unsupported schema version")
    if not isinstance(report.get("model"), str) or not report["model"]:
        issues.append(f"{label} report has no model")
    report_models = report.get("resolved_models")
    if (
        not _string_list(report_models, allow_empty=False)
        or report_models != sorted(report_models)
    ):
        issues.append(f"{label} report has invalid resolved model ids")
    if (
        not isinstance(report.get("claude_cli_version"), str)
        or not report["claude_cli_version"]
    ):
        issues.append(f"{label} report has no Claude CLI version")

    filters = report.get("filters")
    if (
        not isinstance(filters, dict)
        or set(filters) != {"suite", "case"}
        or any(value is not None and not isinstance(value, str) for value in filters.values())
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
    for index, case in enumerate(cases):
        if not isinstance(case, dict):
            issues.append(f"{label} case {index} must be an object")
            continue
        case_id = case.get("id")
        suite_name = case.get("suite")
        if not isinstance(case_id, str) or not case_id:
            issues.append(f"{label} case {index} has no id")
            continue
        if case_id in seen_case_ids:
            issues.append(f"{label} report repeats case id {case_id!r}")
        seen_case_ids.add(case_id)
        if not isinstance(suite_name, str) or not suite_name:
            issues.append(f"{label} case {case_id!r} has no suite")
            continue
        case_ids_by_suite.setdefault(suite_name, []).append(case_id)
        cases_by_suite.setdefault(suite_name, []).append(case)

        for field_name in ("passed", "retry_used", "retry_attempted"):
            if not isinstance(case.get(field_name), bool):
                issues.append(
                    f"{label} case {case_id!r} has invalid {field_name}"
                )
        if not isinstance(case.get("reasons"), list) or any(
            not isinstance(reason, str) for reason in case.get("reasons", [])
        ):
            issues.append(f"{label} case {case_id!r} has invalid reasons")
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
            or not isinstance(telemetry.get("complete"), bool)
            or not isinstance(telemetry.get("issues"), list)
            or any(not isinstance(issue, str) for issue in telemetry.get("issues", []))
        ):
            issues.append(f"{label} case {case_id!r} has invalid telemetry status")

        case_models = case.get("resolved_models")
        if not _string_list(case_models, allow_empty=False):
            issues.append(f"{label} case {case_id!r} has invalid resolved model ids")
        else:
            raw_models.update(case_models)
        if not _string_list(
            case.get("tool_calls"), allow_empty=True, unique=False
        ):
            issues.append(f"{label} case {case_id!r} has invalid tool calls")

        usage = case.get("usage")
        if not isinstance(usage, dict):
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

    if _string_list(report_models, allow_empty=False) and sorted(raw_models) != report_models:
        issues.append(f"{label} resolved model ids do not match raw cases")

    for suite_name, summary in suites.items():
        if not isinstance(suite_name, str) or not isinstance(summary, dict):
            issues.append(f"{label} report has an invalid suite summary")
            continue
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

        suite_cases = cases_by_suite.get(suite_name, [])
        can_recompute = (
            valid_case_ids
            and case_ids == case_ids_by_suite.get(suite_name, [])
            and suite_cases
            and all(
                isinstance(case.get("passed"), bool)
                and isinstance(case.get("retry_used"), bool)
                and isinstance(case.get("telemetry"), dict)
                and isinstance(case["telemetry"].get("complete"), bool)
                and isinstance(case.get("usage"), dict)
                and all(
                    _non_negative_integer(case["usage"].get(field_name))
                    for field_name in usage_fields
                )
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
        usage = summary.get("usage")
        context_summary = usage.get("context_tokens") if isinstance(usage, dict) else None
        expected_context_summary = metric_summary(
            [case["usage"]["context_tokens"] for case in suite_cases]
        )
        if context_summary != expected_context_summary:
            issues.append(
                f"{label} suite {suite_name!r} context summary does not match raw cases"
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
    return issues


def load_report(path: Path) -> dict:
    try:
        report = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read eval report {path}: {error}") from error
    issues = report_comparison_issues(report, f"eval report {path}")
    if issues:
        raise ValueError("; ".join(issues))
    return report


def compare_to_baseline(current: dict, baseline: dict) -> dict:
    checks: list[dict] = []
    failures = [
        *report_comparison_issues(current, "current"),
        *report_comparison_issues(baseline, "baseline"),
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


def write_report(path: Path, report: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=path.parent,
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            temporary = Path(handle.name)
            json.dump(report, handle, indent=2, sort_keys=True, ensure_ascii=False)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        temporary.replace(path)
    except Exception:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
        raise


# ----- fixture lifecycle ----------------------------------------------------


def _run_git(args: list[str], cwd: Path) -> None:
    """Run git with our scrubbed environment. Raises on non-zero exit."""
    proc = subprocess.run(
        ["git", *args],
        cwd=cwd,
        env={**_PROC_ENV, **GIT_ENV},
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"git {' '.join(args)} failed in {cwd}:\n"
            f"  stdout: {proc.stdout.strip()}\n  stderr: {proc.stderr.strip()}"
        )


def setup_fixture(fixture_name: str, baseline_ref: str, after_ref: str) -> Path:
    """Build a real tmp git repo with baseline → after as two tagged commits.

    Layout expected at `evals/fixtures/<name>/`:
        baseline/             — files at the baseline tag
        changes/<after_ref>/  — files at the after tag (full tree, replaces baseline)

    Returns the tmp repo path. Caller is responsible for cleanup via
    `teardown_fixture`.
    """
    src = FIXTURES_DIR / fixture_name
    if not src.is_dir():
        raise FileNotFoundError(f"fixture not found: {src}")
    baseline_src = src / "baseline"
    after_src = src / "changes" / after_ref
    if not baseline_src.is_dir():
        raise FileNotFoundError(f"fixture baseline missing: {baseline_src}")
    if not after_src.is_dir():
        raise FileNotFoundError(f"fixture variant missing: {after_src}")

    tmp = Path(tempfile.mkdtemp(prefix=f"mmcg-eval-{fixture_name}-"))

    # Phase 1: baseline tree.
    _copy_tree_into(baseline_src, tmp)
    _run_git(["init", "-q", "--initial-branch=main"], tmp)
    _run_git(["add", "-A"], tmp)
    _run_git(["commit", "-q", "-m", "baseline"], tmp)
    _run_git(["tag", baseline_ref], tmp)

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
    _run_git(["add", "-A"], tmp)
    _run_git(["commit", "-q", "-m", f"executor change ({after_ref})", "--allow-empty"], tmp)
    _run_git(["tag", after_ref], tmp)

    # Phase 3: build an mmcg index of the after-tree so the auditor can run
    # real `mmcg_callers` / `mmcg_search` against the working state and
    # compare against the spec's pre-edit snapshot. Failure here is non-fatal
    # — the auditor can still operate on `git diff` alone.
    try:
        _build_mmcg_index(tmp)
    except (RuntimeError, FileNotFoundError) as e:
        sys.stderr.write(f"  [fixture] mmcg index skipped: {e}\n")

    return tmp


def _build_mmcg_index(repo: Path) -> None:
    """Run `mmcg index .` in `repo`, leaving `.mastermind/mmcg.db` behind.

    Uses the in-tree binary when available so the SQL schema matches the MCP
    server's expectations. Quiet on success — index summary is suppressed.
    """
    db_path = repo / ".mastermind" / "mmcg.db"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    proc = subprocess.run(
        [MMCG_BIN, "--index", str(db_path), "index", str(repo)],
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
    """Recursive copy src/* into dst/, creating dst if needed."""
    dst.mkdir(parents=True, exist_ok=True)
    for entry in src.iterdir():
        target = dst / entry.name
        if entry.is_dir():
            shutil.copytree(entry, target, dirs_exist_ok=True)
        else:
            shutil.copy2(entry, target)


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


def isolated_cli_args(suite_name: str) -> list[str]:
    """Deny repository tools to suites whose fixtures exist only in prompts."""
    if suite_name in {"critic", "intake", "workflow"}:
        return ["--safe-mode", "--tools", ""]
    if suite_name == "researcher":
        return [
            "--tools",
            "Read,Glob,Grep",
            "--allowedTools",
            ",".join(researcher_allowed_tools()),
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
            ",".join(auditor_allowed_tools()),
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--no-chrome",
        ]
    return []


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


def workflow_prompt_path(case: dict) -> Path:
    artifact = case.get("artifact")
    if artifact not in WORKFLOW_ARTIFACTS:
        raise ValueError(f"workflow artifact is not allowlisted: {artifact!r}")
    path = (REPO_ROOT / artifact).resolve()
    try:
        path.relative_to(REPO_ROOT.resolve())
    except ValueError as error:
        raise ValueError("workflow artifact must stay inside the repository") from error
    if not path.is_file():
        raise FileNotFoundError(f"workflow artifact not found: {path}")
    return path


def render_auditor_input(
    inp: dict,
    *,
    fixture_path: Path,
    baseline_ref: str,
    after_ref: str,
    has_mmcg: bool,
) -> str:
    """Build the auditor's user message.

    Crucially: NO synthetic git_diff. The auditor is told the working
    directory and the two tag names and is expected to run `git diff`,
    `git log`, `git show --stat` etc. itself via Bash. When `has_mmcg` is
    true, the auditor also has live `mmcg_callers` / `mmcg_search` MCP
    tools pointed at an index of the after-tree state.
    """
    mmcg_note = (
        "\n\n**mmcg available:** the working dir has a fresh `.mastermind/mmcg.db` "
        "indexed at the after-commit state. You can call `mmcg_callers`, "
        "`mmcg_search`, `mmcg_outline` etc. via the MCP tools — use them to "
        "verify the spec's pre-edit symbol snapshot against the current state.\n"
    ) if has_mmcg else "\n"
    return (
        f"Audit the executor's work against the spec.\n\n"
        f"**Working directory:** `{fixture_path}` — a real git repo with two commits.\n"
        f"**Baseline tag:** `{baseline_ref}` (state before the executor ran)\n"
        f"**Executor commit tag:** `{after_ref}` (state after the executor ran)\n\n"
        f"Use real `git diff {baseline_ref}..{after_ref}`, `git log`, "
        f"`git show --stat` against this repo. Do NOT trust the executor's "
        f"narrative — verify each claim against the diff."
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


def audit_verification_passed(output: str, expected_command: str) -> bool:
    data = extract_audit_data(output)
    if not data:
        return False
    reruns = data.get("verifications_rerun")
    if not isinstance(reruns, list):
        return False
    return any(
        isinstance(entry, dict)
        and entry.get("cmd") == expected_command
        and str(entry.get("result", "")).lower() == "pass"
        for entry in reruns
    )


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


def evaluate_case(
    model: str,
    suite_name: str,
    suite_cfg: dict,
    case: dict,
    *,
    keep_fixtures: bool,
) -> Result:
    case_id = case["id"]
    prompt_path = (
        workflow_prompt_path(case)
        if suite_name == "workflow"
        else suite_cfg["subagent"]
    )
    system_prompt = strip_frontmatter(prompt_path.read_text())

    fixture_path: Path | None = None
    prompt_sandbox: tempfile.TemporaryDirectory[str] | None = None
    extra_cmd: list[str] = []
    try:
        if suite_cfg["uses_fixture"]:
            fixture_name = case["fixture"]
            baseline_ref = case["baseline_ref"]
            after_ref = case["after_ref"]
            fixture_path = setup_fixture(fixture_name, baseline_ref, after_ref)

            db_path = fixture_path / ".mastermind" / "mmcg.db"
            has_mmcg = db_path.is_file()
            if suite_name in {"auditor", "researcher"} and not has_mmcg and not case.get("allow_no_mmcg"):
                return Result(
                    case_id=case_id,
                    suite=suite_name,
                    passed=False,
                    reasons=[
                        "mmcg index unavailable — build the mmcg binary first "
                        "(`cargo build --release`)"
                    ],
                    fixture_path=fixture_path,
                )
            if suite_name == "auditor":
                user_message = render_auditor_input(
                    case["input"],
                    fixture_path=fixture_path,
                    baseline_ref=baseline_ref,
                    after_ref=after_ref,
                    has_mmcg=has_mmcg,
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
                            "command": MMCG_BIN,
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
            agent_args = subagent_cli_args(prompt_path, model_override=model)
        else:
            prompt_flag = (
                "--system-prompt"
                if suite_name == "workflow"
                else "--append-system-prompt"
            )
            prompt_args = [prompt_flag, system_prompt]
            agent_args = []
        workflow_safety = isolated_cli_args(suite_name)
        if requires_prompt_sandbox(suite_name):
            prompt_sandbox = tempfile.TemporaryDirectory(prefix="mastermind-eval-")
        case_cwd = evaluation_cwd(
            suite_name,
            fixture_path=fixture_path,
            prompt_sandbox=prompt_sandbox,
        )
        streamed_output = True
        cmd = [
            "claude",
            "-p",
            "--model", model,
            *prompt_args,
            *agent_args,
            "--output-format", "stream-json" if streamed_output else "json",
            *(["--verbose"] if streamed_output else []),
            "--no-session-persistence",
            "--permission-mode", "dontAsk",
            *workflow_safety,
            *extra_cmd,
        ]

        # Auditor with real-git fixtures runs many Bash + Read tool calls per
        # case; observed range is ~120–280s on Sonnet, can blow past 300s. Set
        # generously at 480s to absorb tail-latency variance.
        try:
            proc = subprocess.run(
                cmd, input=user_message, capture_output=True, text=True,
                env=_PROC_ENV,
                cwd=case_cwd,
                timeout=480,
            )
        except subprocess.TimeoutExpired:
            return Result(
                case_id=case_id,
                suite=suite_name,
                passed=False,
                reasons=["timeout after 480s"],
                fixture_path=fixture_path,
            )

        if proc.returncode != 0:
            err = (proc.stderr or proc.stdout or "").strip()[:300]
            return Result(
                case_id=case_id,
                suite=suite_name,
                passed=False,
                reasons=[f"claude exit {proc.returncode}: {err}"],
                fixture_path=fixture_path,
            )

        permission_denials: list[dict] = []
        tool_calls: list[str] = []
        telemetry: dict[str, object] = telemetry_from_payload({})
        try:
            payload, tool_calls = parse_claude_output(
                proc.stdout, streamed=streamed_output
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
        except (json.JSONDecodeError, TypeError, ValueError):
            output = proc.stdout
            duration_ms = 0

        expect = case.get("expect", {})
        reasons: list[str] = []
        passed = True

        if telemetry["complete"] is not True:
            passed = False
            reasons.append(
                "incomplete Claude telemetry: " + "; ".join(telemetry["issues"])
            )

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
            elif not any(
                re.search(rf"\b{re.escape(v)}\b", output, re.IGNORECASE)
                for v in candidates
            ):
                passed = False
                reasons.append(f"none of expected verdicts {candidates} found")

        expected_rerun = expect.get("verification_rerun")
        if expected_rerun and suite_name == "auditor":
            if not audit_verification_passed(output, expected_rerun):
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

        if not passed and permission_denials:
            reasons.append(
                "permission denials: "
                + json.dumps(permission_denials, sort_keys=True, ensure_ascii=False)
            )

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

    if not shutil.which("claude"):
        print(
            "error: `claude` CLI not on PATH. Install Claude Code: https://claude.com/claude-code",
            file=sys.stderr,
        )
        return 2
    if not shutil.which("git"):
        print("error: `git` not on PATH (required for fixture suites).", file=sys.stderr)
        return 2

    suites_to_run = [args.suite] if args.suite else list(SUITES.keys())
    results: list[Result] = []

    for suite_name in suites_to_run:
        suite_cfg = SUITES[suite_name]
        if not suite_cfg["cases"].exists():
            print(f"  skip suite {suite_name} — no cases file at {suite_cfg['cases']}")
            continue
        print(f"\n=== {suite_name} ===")
        with suite_cfg["cases"].open() as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("//"):
                    continue
                case = json.loads(line)
                if args.case and case["id"] != args.case:
                    continue
                print(f"  [{case['id']}] running ...", end=" ", flush=True)
                r = evaluate_case(
                    args.model, suite_name, suite_cfg, case,
                    keep_fixtures=args.keep_fixtures,
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
                        args.model, suite_name, suite_cfg, case,
                        keep_fixtures=args.keep_fixtures,
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

    if not results:
        print("\nno cases matched filter")
        return 2

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
