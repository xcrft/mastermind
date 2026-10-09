"""Experimental Codex effort routing from the public request alone."""

import copy
import re

from . import artifacts, conditions


POLICY = "bounded_readonly_v1"
RISK = re.compile(r"\b(state|pre.flight|post.flight|security|authentication|permission|"
                  r"race|deadlock|remov\w*|delet\w*|migration)\b", re.IGNORECASE)


def select(task):
    question = task.get("question", "")
    contract = task.get("output_contract", "")
    paths = task.get("source_allowlist", [])
    readonly = (task.get("kind") == "research" and isinstance(contract, str)
                and "read source only; do not execute" in contract.lower())
    bounded = isinstance(paths, list) and 1 <= len(paths) <= 3
    risk = bool(RISK.search(question)) if isinstance(question, str) else True
    effort = "high" if readonly and bounded and not risk else "max"
    return {"policy": POLICY, "task_sha256": artifacts.digest(task),
            "reasoning_effort": effort,
            "features": {"readonly_declared": readonly, "bounded_source_scope": bounded,
                         "risk_marker_present": risk},
            "scope": "request_heuristic_not_a_validated_difficulty_estimate"}


def configure(task, config):
    result = copy.deepcopy(config)
    policy = result.pop("effort_policy")
    adapter = result.get("adapter")
    if (not isinstance(policy, dict) or set(policy) != {"condition", "policy"}
            or policy["policy"] != POLICY or not isinstance(adapter, dict)
            or adapter.get("kind") != "codex_cli"):
        raise artifacts.BenchmarkError("effort_policy", "declare a supported Codex campaign effort policy")
    matrix = conditions.validate_conditions(result.get("conditions"))
    target = next((item for item in matrix if item["id"] == policy["condition"]), None)
    if target is None or not all("reasoning_effort" in item for item in matrix):
        raise artifacts.BenchmarkError("effort_policy", "effort routing needs a named condition and explicit efforts")
    decision = select(task)
    target["reasoning_effort"] = decision["reasoning_effort"]
    return result, dict(decision, condition=target["id"])
