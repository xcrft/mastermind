"""Public task, manifest and adapter stream contracts."""

from __future__ import annotations

from pathlib import Path
import math
import re

from . import artifacts as artifact_io
from . import conditions as condition_contract


MAX_TURNS_LIMIT = 64

MAX_OUTPUT_TOKENS_LIMIT = 64 * 1024

OPTIONAL_LIMITS = {"timeout_seconds", "max_turns", "max_output_tokens"}

NEUTRAL_INSTRUCTION = (
    "Investigate the supplied question using only the exposed read-only source tools. "
    "The Git repository is an allowlisted projection with a synthetic single commit, "
    "not the original repository history. Cite path:line locations. Distinguish "
    "observations, inferences and unknowns. Do not execute the researched code. "
    "Use only tools listed as available; unavailable tools require source inspection."
)


def validate_limits(value: object) -> dict:
    limits = {"timeout_seconds": 300, "trace_bytes": 2 * 1024 * 1024,
              "stderr_bytes": 64 * 1024, "answer_bytes": 64 * 1024,
              "max_turns": 8, "max_output_tokens": 4096}
    if not isinstance(value, dict) or set(value) - set(limits):
        raise artifact_io.BenchmarkError("invalid_limits", "unknown benchmark limit")
    limits.update(value)
    for name, limit in limits.items():
        if name in OPTIONAL_LIMITS and limit is None:
            continue
        if type(limit) is not int or limit < 1:
            raise artifact_io.BenchmarkError("invalid_limits", f"{name} must be a positive integer" +
                (" or null" if name in OPTIONAL_LIMITS else ""))
    if (limits["timeout_seconds"] is not None and limits["timeout_seconds"] > 3600
            or max(limits["trace_bytes"], limits["stderr_bytes"]) > 16 * 1024 * 1024):
        raise artifact_io.BenchmarkError("invalid_limits", "runtime limits exceed benchmark hard caps")
    if (limits["max_turns"] is not None and limits["max_turns"] > MAX_TURNS_LIMIT
            or limits["max_output_tokens"] is not None and limits["max_output_tokens"] > MAX_OUTPUT_TOKENS_LIMIT):
        raise artifact_io.BenchmarkError("invalid_limits", "model usage limits exceed benchmark hard caps")
    if limits["answer_bytes"] > limits["trace_bytes"]:
        raise artifact_io.BenchmarkError("invalid_limits", "answer cap must fit inside trace cap")
    return limits


def validate_task(task: dict) -> dict:
    if set(task) != {"id", "revision", "source_allowlist", "question", "output_contract", "kind"}:
        raise artifact_io.BenchmarkError("invalid_task", "task must contain only the public task fields")
    if task["kind"] not in ("research", "decision"):
        raise artifact_io.BenchmarkError("invalid_task", "task kind must be research or decision")
    for field in ("id", "question", "output_contract"):
        if not isinstance(task[field], str) or not task[field] or len(task[field].encode()) > 16384:
            raise artifact_io.BenchmarkError("invalid_task", f"invalid {field}")
    artifact_io.exact_revision(task["revision"])
    paths = task["source_allowlist"]
    if not isinstance(paths, list) or not 1 <= len(paths) <= condition_contract.SOURCE_FILE_LIMIT:
        raise artifact_io.BenchmarkError("invalid_task", "source allowlist must have 1..128 files")
    normalized = [condition_contract.safe_source_path(path) for path in paths]
    if len(normalized) != len(set(normalized)):
        raise artifact_io.BenchmarkError("invalid_task", "source allowlist contains duplicates")
    return task


def validate_rubric(task: dict, rubric: dict) -> None:
    if rubric.get("task_id") != task["id"] or rubric.get("source_revision") != task["revision"]:
        raise artifact_io.BenchmarkError("rubric_mismatch", "rubric must name this task and its exact source revision")


def common_identity(manifest: dict) -> dict:
    adapter = manifest["adapter"]
    if not isinstance(adapter, dict):
        raise artifact_io.BenchmarkError("adapter_invalid", "runtime adapter must be an object")
    settings = adapter.get("settings", {})
    if not isinstance(settings, dict):
        raise artifact_io.BenchmarkError("condition_runtime_mismatch", "runtime settings must be an object")
    spec = condition_contract.condition_spec(manifest)
    varied_effort = spec is not None and "reasoning_effort" in spec
    if varied_effort and (adapter.get("kind") not in ("codex_cli", "claude_cli")
            or settings.get("reasoning_effort") != spec["reasoning_effort"]):
        raise artifact_io.BenchmarkError("condition_runtime_mismatch", "runtime effort differs from the declared condition")
    identity = {key: adapter[key] for key in ("sha256", "version", "origin")}
    if adapter.get("kind") in ("claude_cli", "codex_cli"):
        identity.update(kind=adapter["kind"], bundle=adapter["bundle"],
                        cli={k: adapter["cli"][k] for k in ("sha256", "version", "origin")},
                        python={k: adapter["python"][k] for k in ("sha256", "version", "origin")})
        if adapter.get("kind") == "codex_cli" or "settings" in adapter:
            identity["settings"] = {key: value for key, value in settings.items()
                                    if not varied_effort or key != "reasoning_effort"}
    return {key: manifest[key] for key in (
        "task", "rubric_sha256", "source_sha256", "model", "limits", "tool_revision"
    )} | {"adapter": identity,
         "neutral_instruction": NEUTRAL_INSTRUCTION}


def condition_identity(manifest: dict) -> dict:
    indexer = manifest.get("indexer")
    identity = {key: manifest[key] for key in ("common_sha256", "condition", "instruction_sha256")} | {
        "indexer": {key: indexer[key] for key in ("sha256", "version", "source_revision", "origin")} if indexer else None,
        "index_contract": manifest.get("index_contract"), "indexed_files": manifest.get("indexed_files")}
    if "batch" in manifest:
        identity["batch"] = manifest["batch"]
    if manifest["schema_version"] == 4:
        identity.update(condition_spec=condition_contract.condition_spec(manifest), instruction_files=manifest["instruction_files"])
    if "calibration" in manifest:
        if manifest["schema_version"] != 4:
            raise artifact_io.BenchmarkError("invalid_calibration", "calibration needs explicit conditions")
        identity["calibration"] = manifest["calibration"]
    return identity


def adapter_request(trial: Path, manifest: dict, instruction: str) -> dict:
    request = {"protocol": "mastermind-research-adapter-v1", "task": manifest["task"],
               "source_root": str(trial / "source"), "source_files": manifest["source_files"],
               "system_instruction": NEUTRAL_INSTRUCTION, "portable_instruction": instruction,
               "model": manifest["model"], "limits": manifest["limits"],
               "available_tools": ["source_read", "source_search", "source_git"], "mmcg": None}
    if condition_contract.uses_mmcg(manifest):
        request["available_tools"].append("mmcg")
        request["mmcg"] = {"binary": manifest["indexer"]["path"], "index": str(trial / "index/mmcg.db")}
    if manifest["schema_version"] >= 2:
        request["projection_revision"] = manifest["projection_revision"]
        if request["mmcg"] is not None:
            request["mmcg"].update(runtime=manifest["indexer"], index_sha256=manifest["index_sha256"],
                                   index_contract=manifest["index_contract"], indexed_files=manifest["indexed_files"])
    return request


def parse_stream(body: bytes, answer_limit: int) -> tuple[dict | None, dict | None, list[str], list[str]]:
    """Require one init and one terminal result, preserving a valid final answer."""
    init, result, tools, issues = None, None, [], []
    for line in body.splitlines():
        if not line.strip():
            continue
        try:
            event = artifact_io.parse_json(line)
        except (ValueError, UnicodeError):
            issues.append("invalid_json_event")
            continue
        if result is not None:
            issues.append("event_after_result")
            continue
        kind = event.get("type")
        if kind == "init" and init is None:
            init = event
        elif kind == "trace" and init is not None:
            if isinstance(event.get("tool"), str):
                tools.append(event["tool"])
        elif kind == "result" and init is not None:
            result = event
            if type(event.get("model_error")) is not bool:
                issues.append("missing_or_invalid_model_error")
            if not isinstance(event.get("answer"), str):
                issues.append("missing_answer")
            elif not event["answer"].strip() and event.get("model_error") is not True and not event.get("failure"):
                issues.append("empty_answer")
            elif len(event["answer"].encode("utf-8")) > answer_limit:
                issues.append("answer_limit")
            failure = event.get("failure")
            if failure is not None and (
                    not isinstance(failure, dict) or set(failure) != {"state", "code"}
                    or not isinstance(failure.get("state"), str)
                    or failure.get("state") not in {"setup_error", "protocol_error", "identity_mismatch",
                        "timeout", "output_limit", "invocation_error", "model_error", "budget_exceeded"}
                    or not isinstance(failure.get("code"), str)
                    or not re.fullmatch(r"[a-z][a-z0-9_]{0,95}", failure["code"])
                    or event.get("model_error") != (failure["state"] == "model_error")):
                issues.append("invalid_failure")
        else:
            issues.append("unexpected_event")
    if init is None:
        issues.append("missing_init")
    if result is None:
        issues.append("missing_result")
    return init, result, tools, sorted(set(issues))


def telemetry(result: dict | None, limits: dict) -> dict:
    result = result or {}
    usage = result.get("usage")
    fields = ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens")
    issues, values = [], {}
    for name in fields:
        value = usage.get(name) if isinstance(usage, dict) else None
        if type(value) is not int or value < 0:
            issues.append(f"missing_or_invalid_{name}")
            value = None
        values[name] = value
    turns = result.get("turns")
    if type(turns) is not int or turns < 1:
        issues.append("missing_or_invalid_turns")
        turns = None
    cost = result.get("cost_usd")
    try:
        valid_cost = type(cost) in (int, float) and math.isfinite(cost) and cost >= 0
    except OverflowError:
        valid_cost = False
    if not valid_cost:
        cost = None
    exceeded = []
    if limits["max_turns"] is not None and turns is not None and turns > limits["max_turns"]:
        exceeded.append("max_turns")
    if (limits["max_output_tokens"] is not None and values["output_tokens"] is not None
            and values["output_tokens"] > limits["max_output_tokens"]):
        exceeded.append("max_output_tokens")
    timings = dict(first_message_seconds=None, final_answer_seconds=None)
    supplied = result.get("timings")
    if supplied is not None:
        if not isinstance(supplied, dict) or set(supplied) != set(timings):
            issues.append("invalid_timings")
        else:
            for name, value in supplied.items():
                try:
                    valid = value is None or type(value) in (int, float) and math.isfinite(value) and value >= 0
                except OverflowError:
                    valid = False
                if not valid:
                    issues.append("invalid_timings")
                else:
                    timings[name] = value
            first, final = (timings[name] for name in ("first_message_seconds", "final_answer_seconds"))
            if final is not None and (first is None or first > final):
                issues.append("invalid_timings")
        if "invalid_timings" in issues:
            timings = dict.fromkeys(timings)
    return {"complete": not issues, "issues": sorted(set(issues)), "usage": values, "turns": turns,
            "timings": timings,
            "cost_usd": cost, "budget_exceeded": exceeded,
            "model_budget_enforcement": "adapter_unverified"}
