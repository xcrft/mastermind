"""Explicit instruction and tool conditions for matched research trials."""

from __future__ import annotations

from pathlib import PurePosixPath
import hashlib
import re

from . import artifacts as artifact_io


CONDITIONS = ("source", "portable", "portable_mmcg")

REASONING_EFFORTS = ("low", "medium", "high", "xhigh", "max")

CONDITION_LIMIT = 8

TRIAL_LIMIT = 60

SOURCE_FILE_LIMIT = 128

SOURCE_BYTE_LIMIT = 8 * 1024 * 1024

FORBIDDEN_PARTS = {".git", ".claude", ".codex", ".agents", ".mastermind", "evals"}

FORBIDDEN_NAMES = {"AGENTS.md", "CLAUDE.md", ".mcp.json"}


def safe_source_path(value: object) -> str:
    if not isinstance(value, str) or not value or "\\" in value or any(ord(c) < 32 for c in value):
        raise artifact_io.BenchmarkError("invalid_source_path", "source paths must be relative POSIX paths")
    path = PurePosixPath(value)
    if (not path.parts or path.is_absolute() or path.as_posix() != value or ".." in path.parts
            or path.parts[0].endswith(":") or set(path.parts) & FORBIDDEN_PARTS
            or path.name in FORBIDDEN_NAMES):
        raise artifact_io.BenchmarkError("invalid_source_path", f"excluded source path: {value}")
    return value


def validate_condition(item: object) -> dict:
    required = {"id", "tools", "instruction_paths"}
    if (not isinstance(item, dict) or not required <= set(item)
            or set(item) - required - {"reasoning_effort", "role", "symbol_lookup", "source_delivery"}
            or not isinstance(item["id"], str)
            or not re.fullmatch(r"[a-z][a-z0-9_]{0,47}", item["id"])
            or item["tools"] not in ("source", "mmcg")
            or not isinstance(item["instruction_paths"], list)
            or len(item["instruction_paths"]) > CONDITION_LIMIT):
        raise artifact_io.BenchmarkError("invalid_conditions", "invalid condition specification")
    if "symbol_lookup" in item and (item["tools"] != "mmcg"
            or item["symbol_lookup"] not in ("single", "batch")):
        raise artifact_io.BenchmarkError("invalid_conditions", "symbol_lookup requires mmcg and single or batch")
    if "source_delivery" in item and (item["tools"] != "mmcg"
            or item["source_delivery"] not in ("native_full", "native_reuse")):
        raise artifact_io.BenchmarkError("invalid_conditions", "source_delivery requires mmcg and native_full or native_reuse")
    if "reasoning_effort" in item and item["reasoning_effort"] not in REASONING_EFFORTS:
        raise artifact_io.BenchmarkError("invalid_conditions", "invalid declared reasoning effort")
    if "role" in item and (not isinstance(item["role"], str)
            or not re.fullmatch(r"[a-z][a-z0-9_]{0,47}", item["role"])):
        raise artifact_io.BenchmarkError("invalid_conditions", "invalid portable role label")
    paths = [safe_source_path(path) for path in item["instruction_paths"]]
    if len(paths) != len(set(paths)):
        raise artifact_io.BenchmarkError("invalid_conditions", "duplicate condition instruction path")
    return item


def validate_conditions(value: object) -> list[dict]:
    if not isinstance(value, list) or not 2 <= len(value) <= CONDITION_LIMIT:
        raise artifact_io.BenchmarkError("invalid_conditions", "declare 2..8 conditions")
    ids = [validate_condition(item)["id"] for item in value]
    if len(ids) != len(set(ids)):
        raise artifact_io.BenchmarkError("invalid_conditions", "duplicate condition ID")
    if any("reasoning_effort" in item for item in value) and not all("reasoning_effort" in item for item in value):
        raise artifact_io.BenchmarkError("invalid_conditions", "declare effort for every condition or keep it common")
    return value


def validate_calibration(value: object, matrix: object) -> dict:
    """Separate portable role-prompt and effort experiments before inference."""
    if (not isinstance(value, dict) or set(value) != {"axis"}
            or value["axis"] not in ("role_prompt", "effort")):
        raise artifact_io.BenchmarkError("invalid_calibration", "declare a role_prompt or effort axis")
    matrix = validate_conditions(matrix)
    if (any("role" not in item or "reasoning_effort" not in item for item in matrix)
            or len({(item["tools"], item.get("symbol_lookup"), item.get("source_delivery")) for item in matrix}) != 1):
        raise artifact_io.BenchmarkError("confounded_calibration", "calibration needs explicit roles, efforts and identical tools")
    if value["axis"] == "effort":
        if (len({item["role"] for item in matrix}) != 1
                or len({tuple(item["instruction_paths"]) for item in matrix}) != 1):
            raise artifact_io.BenchmarkError("confounded_calibration", "an effort comparison must keep the role and instruction paths fixed")
    elif len({item["reasoning_effort"] for item in matrix}) != 1:
        raise artifact_io.BenchmarkError("confounded_calibration", "a role-prompt comparison must keep effort fixed")
    return value


def condition_names(batch: dict) -> tuple[str, ...]:
    if batch["schema_version"] == 3:
        return tuple(item["id"] for item in validate_conditions(batch["conditions"]))
    if "conditions" in batch:
        raise artifact_io.BenchmarkError("invalid_conditions", "legacy batch cannot redefine its conditions")
    return CONDITIONS


def condition_spec(manifest: dict) -> dict | None:
    if manifest["schema_version"] < 4:
        if "condition_spec" in manifest or "instruction_files" in manifest:
            raise artifact_io.BenchmarkError("invalid_condition", "legacy trial cannot redefine its condition")
        if manifest["condition"] not in CONDITIONS:
            raise artifact_io.BenchmarkError("invalid_condition", "unknown legacy condition")
        return None
    spec = validate_condition(manifest["condition_spec"])
    if spec["id"] != manifest["condition"]:
        raise artifact_io.BenchmarkError("invalid_condition", "trial differs from its condition specification")
    return spec


def uses_mmcg(manifest: dict) -> bool:
    spec = condition_spec(manifest)
    return spec["tools"] == "mmcg" if spec is not None else manifest["condition"] == "portable_mmcg"


def verify_instruction(manifest: dict, instruction: str) -> None:
    if (not isinstance(instruction, str)
            or hashlib.sha256(instruction.encode()).hexdigest() != manifest["instruction_sha256"]):
        raise artifact_io.BenchmarkError("request_changed", "instruction changed")
    spec = condition_spec(manifest)
    if spec is None:
        if manifest["condition"] == "source" and instruction:
            raise artifact_io.BenchmarkError("request_changed", "source condition contains an instruction")
        return
    records = manifest["instruction_files"]
    if not isinstance(records, list) or len(records) != len(spec["instruction_paths"]):
        raise artifact_io.BenchmarkError("request_changed", "instruction inventory changed")
    body, offset = instruction.encode(), 0
    for position, (record, path) in enumerate(zip(records, spec["instruction_paths"])):
        if (not isinstance(record, dict) or set(record) != {"path", "bytes", "sha256"}
                or record["path"] != path or type(record["bytes"]) is not int
                or not 0 <= record["bytes"] <= artifact_io.CONTROL_BYTE_LIMIT):
            raise artifact_io.BenchmarkError("request_changed", "invalid instruction record")
        if position:
            if body[offset:offset + 2] != b"\n\n":
                raise artifact_io.BenchmarkError("request_changed", "instruction separator changed")
            offset += 2
        part = body[offset:offset + record["bytes"]]
        part.decode("utf-8")
        if len(part) != record["bytes"] or hashlib.sha256(part).hexdigest() != record["sha256"]:
            raise artifact_io.BenchmarkError("request_changed", "instruction file bytes changed")
        offset += record["bytes"]
    if offset != len(body) or len(body) > artifact_io.CONTROL_BYTE_LIMIT:
        raise artifact_io.BenchmarkError("request_changed", "instruction bytes exceed their inventory")
