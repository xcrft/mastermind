#!/usr/bin/env python3
"""Prepare and execute bounded research trials through a trusted adapter.

This first slice separates workspaces/configuration; it is not an OS sandbox.
Generic adapter results always remain ineligible for a quality-uplift claim.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import shutil
import sqlite3
import stat
import sys
import time
import uuid
from contextlib import closing
from pathlib import Path, PurePosixPath

if __package__:
    from .benchmark_process import run_bounded
else:
    from benchmark_process import run_bounded


CONDITIONS = ("source", "portable", "portable_mmcg")
SOURCE_FILE_LIMIT = 128
SOURCE_BYTE_LIMIT = 8 * 1024 * 1024
FILE_BYTE_LIMIT = 2 * 1024 * 1024
CONTROL_BYTE_LIMIT = 1024 * 1024
BINARY_BYTE_LIMIT = 512 * 1024 * 1024
FORBIDDEN_PARTS = {".git", ".claude", ".codex", ".agents", ".mastermind", "evals"}
FORBIDDEN_NAMES = {"AGENTS.md", "CLAUDE.md", ".mcp.json"}
CREDENTIAL_NAMES = {"ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "OPENAI_API_KEY"}
NEUTRAL_INSTRUCTION = (
    "Investigate the supplied question using only the exposed read-only source tools. "
    "The Git repository is an allowlisted projection with a synthetic single commit, "
    "not the original repository history. Cite path:line locations. Distinguish "
    "observations, inferences and unknowns. Do not execute the researched code. "
    "Use only tools listed as available; unavailable tools require source inspection."
)


class BenchmarkError(Exception):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False,
                      allow_nan=False).encode("utf-8")


def digest(value: object) -> str:
    return hashlib.sha256(canonical(value)).hexdigest()


def _pairs(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def parse_json(body: bytes) -> dict:
    try:
        value = json.loads(body, object_pairs_hook=_pairs)
        if not isinstance(value, dict):
            raise ValueError("expected a JSON object")
        # Reject non-finite numbers and unpaired Unicode surrogates, including
        # inside otherwise unused trace fields, before retaining the event.
        canonical(value)
        return value
    except (RecursionError, UnicodeError) as error:
        raise ValueError("JSON nesting or string encoding is invalid") from error


def read_file(path: Path, limit: int = CONTROL_BYTE_LIMIT) -> bytes:
    """No-follow regular-file read with a byte cap and post-read identity check."""
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    try:
        with os.fdopen(os.open(path, flags), "rb") as handle:
            before = os.fstat(handle.fileno())
            if not stat.S_ISREG(before.st_mode) or before.st_size > limit:
                raise BenchmarkError("invalid_file", f"not a bounded regular file: {path}")
            body = handle.read(limit + 1)
            after = os.fstat(handle.fileno())
        current = path.lstat()
        identity = lambda item: (item.st_dev, item.st_ino, item.st_size, item.st_mtime_ns, item.st_mode)
        if len(body) > limit or identity(before) != identity(after) or identity(after) != identity(current):
            raise BenchmarkError("file_changed", f"file changed while reading: {path}")
        return body
    except OSError as error:
        raise BenchmarkError("file_unavailable", f"cannot read file: {path}") from error


def load_json(path: Path) -> dict:
    try:
        return parse_json(read_file(path))
    except (ValueError, UnicodeError) as error:
        raise BenchmarkError("invalid_json", f"invalid JSON: {path}") from error


def write_new(path: Path, value: object) -> None:
    with path.open("xb") as handle:
        handle.write(canonical(value) + b"\n")


def exact_revision(value: object) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", value):
        raise BenchmarkError("invalid_revision", "an exact lowercase Git commit ID is required")
    return value


def safe_source_path(value: object) -> str:
    if not isinstance(value, str) or not value or "\\" in value or any(ord(c) < 32 for c in value):
        raise BenchmarkError("invalid_source_path", "source paths must be relative POSIX paths")
    path = PurePosixPath(value)
    if (not path.parts or path.is_absolute() or path.as_posix() != value or ".." in path.parts
            or path.parts[0].endswith(":") or set(path.parts) & FORBIDDEN_PARTS
            or path.name in FORBIDDEN_NAMES):
        raise BenchmarkError("invalid_source_path", f"excluded source path: {value}")
    return value


def clean_environment(trial: Path, credentials: dict[str, str] | None = None) -> dict[str, str]:
    credentials = credentials or {}
    if set(credentials) - CREDENTIAL_NAMES:
        raise BenchmarkError("invalid_credentials", "only explicit credential names are accepted")
    home = trial / "home"
    temp = trial / "tmp"
    home.mkdir(exist_ok=True)
    temp.mkdir(exist_ok=True)
    return {
        "PATH": os.pathsep.join((str(Path(sys.executable).parent), os.defpath)),
        "HOME": str(home), "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_CACHE_HOME": str(home / ".cache"), "TMPDIR": str(temp),
        "LANG": "C.UTF-8", "LC_ALL": "C", "TERM": "dumb",
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0", "GIT_OPTIONAL_LOCKS": "0",
        "GIT_AUTHOR_NAME": "Benchmark", "GIT_COMMITTER_NAME": "Benchmark",
        "GIT_AUTHOR_EMAIL": "benchmark@example.invalid", "GIT_COMMITTER_EMAIL": "benchmark@example.invalid",
        "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
        **credentials,
    }


def git(repo: Path, args: list[str], env: dict[str, str], limit: int = FILE_BYTE_LIMIT) -> bytes:
    binary = shutil.which("git")
    if binary is None:
        raise BenchmarkError("git_unavailable", "Git is required")
    process = run_bounded(
        [str(Path(binary).resolve()), "--no-replace-objects", "--literal-pathspecs",
         "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
         "-c", "commit.gpgsign=false", *args], cwd=repo, env=env, stdout_limit=limit,
    )
    if process.stop_reason or process.returncode != 0:
        raise BenchmarkError("git_failed", f"Git source operation failed: {process.stop_reason or 'exit'}")
    return process.stdout


def export_source(repo: Path, revision: str, paths: list[str], destination: Path,
                  env: dict[str, str]) -> tuple[list[dict], str]:
    resolved = git(repo, ["rev-parse", "--verify", f"{revision}^{{commit}}"], env).decode().strip()
    if resolved != revision:
        raise BenchmarkError("source_revision_mismatch", "source revision is not the requested commit")
    destination.mkdir()
    files, total = [], 0
    for path in sorted(paths):
        tree = git(repo, ["ls-tree", "-z", revision, "--", path], env)
        records = tree.rstrip(b"\0").split(b"\0")
        if len(records) != 1 or b"\t" not in records[0]:
            raise BenchmarkError("source_missing", f"expected one exact file: {path}")
        header, name = records[0].split(b"\t", 1)
        mode, kind, oid = header.decode("ascii").split()
        if name.decode("utf-8") != path or kind != "blob" or mode not in {"100644", "100755"}:
            raise BenchmarkError("source_type", f"only regular source blobs are allowed: {path}")
        body = git(repo, ["cat-file", "blob", oid], env)
        total += len(body)
        if total > SOURCE_BYTE_LIMIT:
            raise BenchmarkError("source_limit", "source projection exceeds its total byte limit")
        target = destination / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(body)
        target.chmod(0o755 if mode == "100755" else 0o644)
        files.append({"path": path, "git_mode": mode, "bytes": len(body), "sha256": hashlib.sha256(body).hexdigest()})
    template = destination.parent / "empty-git-template"
    template.mkdir()
    git(destination, ["init", "-q", "-b", "benchmark", f"--template={template}"], env)
    git(destination, ["config", "core.autocrlf", "false"], env)
    # Preserve raw allowed blobs even when a supplied .gitignore or
    # .gitattributes would exclude files or transform their contents on add.
    for item in files:
        oid = git(destination, ["hash-object", "-w", "--no-filters", "--", item["path"]], env).decode().strip()
        git(destination, ["update-index", "--add", "--cacheinfo", item["git_mode"], oid, item["path"]], env)
    git(destination, ["commit", "-qm", "Allowlisted source projection"], env)
    projection_revision = git(destination, ["rev-parse", "HEAD"], env).decode().strip()
    return files, projection_revision


def verify_source(source: Path, files: list[dict]) -> None:
    if source.is_symlink() or not source.is_dir():
        raise BenchmarkError("source_changed", "source root is not the prepared directory")
    expected = {item["path"] for item in files}
    allowed_directories = {parent.as_posix() for path in expected
                           for parent in PurePosixPath(path).parents if parent != PurePosixPath(".")}
    observed = set()
    def walk_failed(error):
        raise BenchmarkError("source_changed", "cannot inspect the source inventory") from error

    for directory, names, filenames in os.walk(source, followlinks=False, onerror=walk_failed):
        directory = Path(directory)
        if directory == source:
            if (source / ".git").is_symlink() or not (source / ".git").is_dir():
                raise BenchmarkError("source_changed", "source Git directory changed")
            names[:] = [name for name in names if name != ".git"]
        for name in names:
            path = directory / name
            if path.is_symlink() or path.relative_to(source).as_posix() not in allowed_directories:
                raise BenchmarkError("source_changed", "source contains an unexpected directory")
        for name in filenames:
            observed.add((directory / name).relative_to(source).as_posix())
    if observed != expected:
        raise BenchmarkError("source_changed", "source inventory differs from the frozen projection")
    for item in files:
        path = source / item["path"]
        body = read_file(path, FILE_BYTE_LIMIT)
        mode = "100755" if path.lstat().st_mode & stat.S_IXUSR else "100644"
        if len(body) != item["bytes"] or hashlib.sha256(body).hexdigest() != item["sha256"] or mode != item["git_mode"]:
            raise BenchmarkError("source_changed", f"source no longer matches: {item['path']}")


def verify_projection(source: Path, files: list[dict], revision: str, env: dict[str, str]) -> None:
    exact_revision(revision)
    if git(source, ["rev-parse", "HEAD"], env).decode().strip() != revision:
        raise BenchmarkError("source_changed", "source Git projection changed")
    if git(source, ["rev-list", "--all", "--count"], env).strip() != b"1":
        raise BenchmarkError("source_changed", "source projection has additional Git history")
    entries = []
    for item in sorted(files, key=lambda item: item["path"]):
        body = read_file(source / item["path"], FILE_BYTE_LIMIT)
        algorithm = hashlib.sha1 if len(revision) == 40 else hashlib.sha256
        oid = algorithm(f"blob {len(body)}\0".encode() + body).hexdigest()
        entries.append(f"{item['git_mode']} blob {oid}\t{item['path']}".encode() + b"\0")
    # Compare as records because Git tree order differs for nested directories.
    actual = git(source, ["ls-tree", "-rz", "HEAD"], env, CONTROL_BYTE_LIMIT)
    if sorted(actual.split(b"\0")[:-1]) != sorted(entry[:-1] for entry in entries):
        raise BenchmarkError("source_changed", "Git tree differs from the allowed source bytes")


def runtime_pin(spec: object, role: str, revision: str | None = None) -> dict:
    if not isinstance(spec, dict):
        raise BenchmarkError(f"{role}_missing", f"an explicit {role} runtime pin is required")
    try:
        path = Path(spec["path"]).resolve(strict=True)
        expected = spec["sha256"]
        if not re.fullmatch(r"[0-9a-f]{64}", expected):
            raise ValueError("invalid hash")
        body = read_file(path, BINARY_BYTE_LIMIT)
        if hashlib.sha256(body).hexdigest() != expected:
            raise BenchmarkError(f"{role}_mismatch", f"{role} executable does not match its SHA-256 pin")
        if not os.access(path, os.X_OK):
            raise ValueError("not executable")
        if revision is not None and spec.get("source_revision") != revision:
            raise BenchmarkError(f"{role}_revision_mismatch", f"{role} declared source revision does not match instructions")
        if not isinstance(spec.get("version"), str) or not spec["version"]:
            raise ValueError("missing version")
        if not isinstance(spec.get("origin"), str) or not spec["origin"]:
            raise ValueError("missing origin")
        return {"path": str(path), "sha256": expected, "version": spec["version"],
                "origin": spec["origin"], "source_revision": spec.get("source_revision"),
                "source_provenance": "declared_not_attestation_verified"}
    except (KeyError, TypeError, ValueError, OSError) as error:
        raise BenchmarkError(f"{role}_invalid", f"invalid {role} runtime pin") from error


def validate_limits(value: object) -> dict:
    limits = {"timeout_seconds": 300, "trace_bytes": 2 * 1024 * 1024,
              "stderr_bytes": 64 * 1024, "answer_bytes": 64 * 1024,
              "max_turns": 8, "max_output_tokens": 4096}
    if not isinstance(value, dict) or set(value) - set(limits):
        raise BenchmarkError("invalid_limits", "unknown benchmark limit")
    limits.update(value)
    for name, limit in limits.items():
        if type(limit) is not int or limit < 1:
            raise BenchmarkError("invalid_limits", f"{name} must be a positive integer")
    if limits["timeout_seconds"] > 3600 or max(limits["trace_bytes"], limits["stderr_bytes"]) > 16 * 1024 * 1024:
        raise BenchmarkError("invalid_limits", "runtime limits exceed benchmark hard caps")
    if limits["answer_bytes"] > limits["trace_bytes"]:
        raise BenchmarkError("invalid_limits", "answer cap must fit inside trace cap")
    return limits


def validate_task(task: dict) -> dict:
    if set(task) != {"id", "revision", "source_allowlist", "question", "output_contract", "kind"}:
        raise BenchmarkError("invalid_task", "task must contain only the public task fields")
    if task["kind"] not in ("research", "decision"):
        raise BenchmarkError("invalid_task", "task kind must be research or decision")
    for field in ("id", "question", "output_contract"):
        if not isinstance(task[field], str) or not task[field] or len(task[field].encode()) > 16384:
            raise BenchmarkError("invalid_task", f"invalid {field}")
    exact_revision(task["revision"])
    paths = task["source_allowlist"]
    if not isinstance(paths, list) or not 1 <= len(paths) <= SOURCE_FILE_LIMIT:
        raise BenchmarkError("invalid_task", "source allowlist must have 1..128 files")
    normalized = [safe_source_path(path) for path in paths]
    if len(normalized) != len(set(normalized)):
        raise BenchmarkError("invalid_task", "source allowlist contains duplicates")
    return task


def validate_rubric(task: dict, rubric: dict) -> None:
    if rubric.get("task_id") != task["id"] or rubric.get("source_revision") != task["revision"]:
        raise BenchmarkError("rubric_mismatch", "rubric must name this task and its exact source revision")


def common_identity(manifest: dict) -> dict:
    adapter = manifest["adapter"]
    identity = {key: adapter[key] for key in ("sha256", "version", "origin")}
    if adapter.get("kind") == "claude_cli":
        identity.update(kind="claude_cli", bundle=adapter["bundle"],
                        cli={k: adapter["cli"][k] for k in ("sha256", "version", "origin")},
                        python={k: adapter["python"][k] for k in ("sha256", "version", "origin")})
    return {key: manifest[key] for key in (
        "task", "rubric_sha256", "source_sha256", "model", "limits", "tool_revision"
    )} | {"adapter": identity,
         "neutral_instruction": NEUTRAL_INSTRUCTION}


def condition_identity(manifest: dict) -> dict:
    indexer = manifest.get("indexer")
    return {key: manifest[key] for key in ("common_sha256", "condition", "instruction_sha256")} | {
        "indexer": {key: indexer[key] for key in ("sha256", "version", "source_revision", "origin")} if indexer else None,
        "index_contract": manifest.get("index_contract"), "indexed_files": manifest.get("indexed_files")}


def adapter_request(trial: Path, manifest: dict, instruction: str) -> dict:
    request = {"protocol": "mastermind-research-adapter-v1", "task": manifest["task"],
               "source_root": str(trial / "source"), "source_files": manifest["source_files"],
               "system_instruction": NEUTRAL_INSTRUCTION, "portable_instruction": instruction,
               "model": manifest["model"], "limits": manifest["limits"],
               "available_tools": ["source_read", "source_search", "source_git"], "mmcg": None}
    if manifest["condition"] == "portable_mmcg":
        request["available_tools"].append("mmcg")
        request["mmcg"] = {"binary": manifest["indexer"]["path"], "index": str(trial / "index/mmcg.db")}
    if manifest["schema_version"] >= 2:
        request["projection_revision"] = manifest["projection_revision"]
        if request["mmcg"] is not None:
            request["mmcg"].update(runtime=manifest["indexer"], index_sha256=manifest["index_sha256"],
                                   index_contract=manifest["index_contract"], indexed_files=manifest["indexed_files"])
    return request


def claude_runtime():
    if __package__:
        from . import claude_adapter
    else:
        import claude_adapter
    return claude_adapter


def validate_index(path: Path, source: Path, files: list[dict], contract: dict,
                   indexed_paths: list[str]) -> str:
    # Read-only SQLite validation, after the supervised indexer has exited.
    for suffix in ("-wal", "-journal"):
        sidecar = path.with_name(path.name + suffix)
        try:
            if sidecar.is_symlink():
                raise BenchmarkError("invalid_file", "symbolic SQLite sidecar")
            if sidecar.exists():
                read_file(sidecar, 0)
        except BenchmarkError as error:
            raise BenchmarkError("index_uncheckpointed", "index has an uncheckpointed SQLite sidecar") from error
    body = read_file(path, BINARY_BYTE_LIMIT)
    if not body.startswith(b"SQLite format 3\0"):
        raise BenchmarkError("index_invalid", "indexer did not produce a SQLite database")
    try:
        with closing(sqlite3.connect(path.as_uri() + "?mode=ro&immutable=1", uri=True)) as database:
            database.set_progress_handler(lambda: 1, 1_000_000)
            if database.execute("PRAGMA quick_check").fetchone() != ("ok",):
                raise BenchmarkError("index_invalid", "SQLite integrity check failed")
            names = {row[0] for row in database.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            if not {"symbols", "edges", "meta", "files"} <= names:
                raise BenchmarkError("index_invalid", "index is missing mmcg tables")
            metadata = dict(database.execute("SELECT key,value FROM meta"))
            if any(metadata.get(key) != value for key, value in contract.items()):
                raise BenchmarkError("index_contract_mismatch", "index schema/extractor contract differs")
            if metadata.get("index_root") != str(source.resolve()):
                raise BenchmarkError("index_root_mismatch", "index belongs to a different source projection")
            expected = {item["path"]: item["sha256"] for item in files if item["path"] in indexed_paths}
            actual = list(database.execute("SELECT path,content_sha256 FROM files ORDER BY path"))
            if actual != sorted(expected.items()):
                raise BenchmarkError("index_source_mismatch", "index does not cover the exact declared source bytes")
    except sqlite3.Error as error:
        raise BenchmarkError("index_invalid", "cannot validate the produced SQLite index") from error
    if read_file(path, BINARY_BYTE_LIMIT) != body:
        raise BenchmarkError("index_changed", "index changed during validation")
    return hashlib.sha256(body).hexdigest()


def prepare_trial(
    *, task: dict, rubric: dict, config: dict, source_repo: Path, tool_repo: Path,
    output: Path, condition: str, repetition: int = 0,
) -> Path:
    """Create one independent trial; persist setup failure without invoking a model."""
    validate_task(task)
    if condition not in CONDITIONS or type(repetition) is not int or repetition < 0:
        raise BenchmarkError("invalid_condition", "invalid condition or repetition")
    validate_rubric(task, rubric)
    limits = validate_limits(config.get("limits", {}))
    model = config.get("model")
    if not isinstance(model, str) or not model or model in {"opus", "sonnet", "haiku", "latest"}:
        raise BenchmarkError("invalid_model", "pin an exact model identity, not an alias")
    tool_revision = exact_revision(config.get("tool_revision"))
    output.mkdir(parents=True, exist_ok=True)
    trial = output.resolve() / ("trial-" + uuid.uuid4().hex)
    trial.mkdir(mode=0o700)
    env = clean_environment(trial)
    prepared = time.monotonic()
    manifest = {"kind": "mastermind-research-trial", "schema_version": 2,
                "trial_id": trial.name, "task": task, "condition": condition,
                "repetition": repetition, "rubric_sha256": digest(rubric),
                "model": model, "limits": limits, "tool_revision": tool_revision,
                "status": "setup_failed", "isolation": "host_adapter_unverified"}
    write_new(trial / "rubric.json", rubric)
    try:
        spec = config.get("adapter")
        if isinstance(spec, dict) and spec.get("kind") == "claude_cli":
            adapter = claude_runtime().prepare_runtime(trial, spec, limits)
        else:
            adapter = runtime_pin(spec, "adapter")
        files, projection_revision = export_source(
            source_repo.resolve(), task["revision"], task["source_allowlist"], trial / "source", env,
        )
        source_digest = digest({"revision": task["revision"], "files": files})
        manifest.update(adapter=adapter, source_files=files, source_sha256=source_digest,
                        projection_revision=projection_revision)
        manifest["common_sha256"] = digest(common_identity(manifest))
        instruction = ""
        if condition != "source":
            instruction_path = safe_source_path(config.get("instruction_path"))
            instruction = git(tool_repo.resolve(), ["show", f"{tool_revision}:{instruction_path}"], env,
                              CONTROL_BYTE_LIMIT).decode("utf-8")
        manifest["instruction_sha256"] = hashlib.sha256(instruction.encode()).hexdigest()
        indexer = None
        if condition == "portable_mmcg":
            indexer = runtime_pin(config.get("mmcg"), "mmcg", tool_revision)
            contract = config["mmcg"].get("index_contract")
            if (not isinstance(contract, dict) or set(contract) != {
                    "schema_version", "extractor_contract_version", "concept_normalization_version"}
                    or any(not isinstance(value, str) or not value for value in contract.values())):
                raise BenchmarkError("index_contract_missing", "pin the expected mmcg index contracts")
            indexed_paths = config["mmcg"].get("indexed_files", task["source_allowlist"])
            if (not isinstance(indexed_paths, list) or not indexed_paths
                    or any(path not in task["source_allowlist"] for path in indexed_paths)
                    or len(indexed_paths) != len(set(indexed_paths))):
                raise BenchmarkError("index_scope_invalid", "declare an exact indexed subset of allowed source files")
            manifest.update(index_contract=contract, indexed_files=sorted(indexed_paths))
            index = trial / "index"
            index.mkdir()
            index_path = index / "mmcg.db"
            process = run_bounded([indexer["path"], "--index", str(index_path), "index", str(trial / "source")],
                                  cwd=trial / "source", env=env, timeout=60)
            manifest["index_setup_seconds"] = process.elapsed_seconds
            manifest["index_process"] = {"returncode": process.returncode, "stop_reason": process.stop_reason}
            (trial / "index-stdout.txt").write_bytes(process.stdout)
            (trial / "index-stderr.txt").write_bytes(process.stderr)
            if process.stop_reason or process.returncode != 0:
                raise BenchmarkError("index_setup_failed", "indexer failed; any partial database is unusable")
            manifest["index_sha256"] = validate_index(index_path, trial / "source", files, contract, indexed_paths)
            manifest["indexer"] = indexer
        verify_source(trial / "source", files)
        verify_projection(trial / "source", files, projection_revision, env)
        manifest["condition_sha256"] = digest(condition_identity(manifest))
        request = adapter_request(trial, manifest, instruction)
        request_bytes = canonical(request)
        if len(request_bytes) > CONTROL_BYTE_LIMIT:
            raise BenchmarkError("request_limit", "adapter request exceeds the control byte cap")
        manifest["request_sha256"] = hashlib.sha256(request_bytes).hexdigest()
        write_new(trial / "request.json", request)
        for item in files:
            (trial / "source" / item["path"]).chmod(0o555 if item["git_mode"] == "100755" else 0o444)
        manifest["status"] = "prepared"
    except (BenchmarkError, UnicodeError, ValueError, OSError, TypeError, KeyError) as error:
        manifest["setup_error"] = {"code": getattr(error, "code", "invalid_setup"), "message": str(error)}
    manifest["setup_seconds"] = time.monotonic() - prepared
    write_new(trial / "manifest.json", manifest)
    return trial


def parse_stream(body: bytes, answer_limit: int) -> tuple[dict | None, dict | None, list[str], list[str]]:
    """Require one init and one terminal result, preserving a valid final answer."""
    init, result, tools, issues = None, None, [], []
    for line in body.splitlines():
        if not line.strip():
            continue
        try:
            event = parse_json(line)
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
    if turns is not None and turns > limits["max_turns"]:
        exceeded.append("max_turns")
    if values["output_tokens"] is not None and values["output_tokens"] > limits["max_output_tokens"]:
        exceeded.append("max_output_tokens")
    return {"complete": not issues, "issues": issues, "usage": values, "turns": turns,
            "cost_usd": cost, "budget_exceeded": exceeded,
            "model_budget_enforcement": "adapter_unverified"}


def verify_prepared(trial: Path, manifest: dict) -> dict:
    if manifest.get("kind") != "mastermind-research-trial" or manifest.get("schema_version") not in (1, 2):
        raise BenchmarkError("invalid_manifest", "unknown trial manifest")
    if manifest["trial_id"] != trial.name or manifest["condition"] not in CONDITIONS:
        raise BenchmarkError("invalid_manifest", "trial identity or condition changed")
    validate_task(manifest["task"])
    exact_revision(manifest["tool_revision"])
    if validate_limits(manifest["limits"]) != manifest["limits"]:
        raise BenchmarkError("invalid_manifest", "manifest limits are incomplete")
    files = manifest["source_files"]
    if not isinstance(files, list) or len(files) != len(manifest["task"]["source_allowlist"]):
        raise BenchmarkError("invalid_manifest", "invalid source manifest")
    for item in files:
        if (set(item) != {"path", "git_mode", "bytes", "sha256"}
                or item["git_mode"] not in ("100644", "100755")
                or type(item["bytes"]) is not int or not 0 <= item["bytes"] <= FILE_BYTE_LIMIT
                or not isinstance(item["sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", item["sha256"])):
            raise BenchmarkError("invalid_manifest", "invalid source file record")
        safe_source_path(item["path"])
    if {item["path"] for item in files} != set(manifest["task"]["source_allowlist"]):
        raise BenchmarkError("invalid_manifest", "source files differ from the task allowlist")
    if (digest({"revision": manifest["task"]["revision"], "files": files}) != manifest["source_sha256"]
            or digest(common_identity(manifest)) != manifest["common_sha256"]
            or digest(condition_identity(manifest)) != manifest["condition_sha256"]):
        raise BenchmarkError("manifest_changed", "trial identities do not match the manifest fields")
    rubric = load_json(trial / "rubric.json")
    if digest(rubric) != manifest["rubric_sha256"]:
        raise BenchmarkError("rubric_changed", "rubric changed after preparation")
    validate_rubric(manifest["task"], rubric)
    request = load_json(trial / "request.json")
    if hashlib.sha256(canonical(request)).hexdigest() != manifest["request_sha256"]:
        raise BenchmarkError("request_changed", "adapter request changed after preparation")
    instruction = request["portable_instruction"]
    if (not isinstance(instruction, str)
            or hashlib.sha256(instruction.encode()).hexdigest() != manifest["instruction_sha256"]
            or (manifest["condition"] == "source" and instruction)
            or request != adapter_request(trial, manifest, instruction)):
        raise BenchmarkError("request_changed", "request differs from the frozen trial inputs")
    verify_source(trial / "source", files)
    env = clean_environment(trial)
    verify_projection(trial / "source", files, manifest["projection_revision"], env)
    runtime_pin(manifest["adapter"], "adapter")
    if manifest["adapter"].get("kind") == "claude_cli":
        claude_runtime().verify_runtime(trial, manifest["adapter"])
    if manifest["condition"] == "portable_mmcg":
        runtime_pin(manifest["indexer"], "mmcg", manifest["tool_revision"])
        if validate_index(trial / "index/mmcg.db", trial / "source", manifest["source_files"],
                          manifest["index_contract"], manifest["indexed_files"]) != manifest["index_sha256"]:
            raise BenchmarkError("index_changed", "prepared index changed")
    return request


def run_trial(trial: Path, credentials: dict[str, str] | None = None) -> dict:
    trial = trial.resolve(strict=True)
    manifest_bytes = read_file(trial / "manifest.json")
    manifest = parse_json(manifest_bytes)
    # No implicit reruns: each planned condition/repetition gets one attempt.
    try:
        with (trial / "run.lock").open("x"):
            pass
    except FileExistsError as error:
        raise BenchmarkError("already_run", "trial was already attempted; prepare a new balanced batch") from error
    envelope = {"kind": "mastermind-research-result", "schema_version": 1,
                "trial_id": trial.name, "manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(),
                "common_sha256": manifest.get("common_sha256"),
                "condition_sha256": manifest.get("condition_sha256"),
                "run_status": {"state": "setup_error", "reason": None},
                "quality": {"status": "not_evaluated", "score": None},
                "diagnostics": {}, "answer": None,
                "comparability": {"eligible": False, "isolation": "host_adapter_unverified",
                    "reasons": ["adapter_isolation_unverified", "runtime_provenance_declared"]}}
    try:
        if manifest["status"] != "prepared":
            raise BenchmarkError(manifest.get("setup_error", {}).get("code", "setup_failed"), "trial setup did not complete")
        request = verify_prepared(trial, manifest)
        env = clean_environment(trial, credentials)
    except (BenchmarkError, KeyError, TypeError, ValueError, OSError) as error:
        envelope["run_status"]["reason"] = getattr(error, "code", "invalid_manifest")
        write_new(trial / "result.json", envelope)
        return envelope
    limits = manifest["limits"]
    adapter = manifest["adapter"]
    command = ([adapter["python"]["path"], "-I", "-S", "-B", adapter["path"],
                "--runtime", str(trial / "adapter-runtime.json")]
               if adapter.get("kind") == "claude_cli" else [adapter["path"]])
    process = run_bounded(command, cwd=trial / "source", env=env,
                          stdin=canonical(request) + b"\n", timeout=limits["timeout_seconds"],
                          stdout_limit=limits["trace_bytes"], stderr_limit=limits["stderr_bytes"])
    with (trial / "trace.jsonl").open("xb") as handle:
        handle.write(process.stdout)
    with (trial / "stderr.txt").open("xb") as handle:
        handle.write(process.stderr)
    init, result, tools, issues = parse_stream(process.stdout, limits["answer_bytes"])
    state, reason = "completed", None
    if process.stop_reason:
        state = process.stop_reason if process.stop_reason in {"timeout", "output_limit"} else "invocation_error"
        reason = process.stop_reason
    elif process.returncode != 0:
        state, reason = "invocation_error", "nonzero_exit"
    elif issues:
        state, reason = "protocol_error", issues[0]
    elif result and result.get("failure"):
        state, reason = result["failure"]["state"], result["failure"]["code"]
    elif result and result.get("model_error"):
        state, reason = "model_error", "adapter_reported_model_error"
    elif init and (init.get("model") != manifest["model"] or init.get("adapter_version") != manifest["adapter"]["version"]):
        state, reason = "identity_mismatch", "observed_model_or_adapter_version_mismatch"
    measured = telemetry(result, limits)
    unexpected_tools = sorted(set(tools) - set(request["available_tools"]))
    envelope["diagnostics"] = {"protocol_issues": issues, "telemetry": measured, "tools": tools,
        "unexpected_tools": unexpected_tools,
        "observed_model": init.get("model") if init else None,
        "observed_adapter_version": init.get("adapter_version") if init else None,
        "elapsed_seconds": process.elapsed_seconds, "setup_seconds": manifest["setup_seconds"],
        "returncode": process.returncode, "trace_bytes": len(process.stdout), "stderr_bytes": len(process.stderr),
        "enforced_limits": ["timeout_seconds", "trace_bytes", "stderr_bytes", "answer_bytes"]}
    if adapter.get("kind") == "claude_cli" and result:
        envelope["diagnostics"]["adapter"] = result.get("diagnostics")
    answer = result.get("answer") if result else None
    if isinstance(answer, str) and answer.strip() and len(answer.encode("utf-8")) <= limits["answer_bytes"]:
        answer_bytes = answer.encode("utf-8")
        with (trial / "answer.md").open("xb") as handle:
            handle.write(answer_bytes)
        envelope["answer"] = {"path": "answer.md", "bytes": len(answer_bytes),
                              "sha256": hashlib.sha256(answer_bytes).hexdigest()}
        envelope["quality"]["status"] = "review_pending"
    try:
        verify_prepared(trial, manifest)
        if read_file(trial / "manifest.json") != manifest_bytes:
            raise BenchmarkError("manifest_changed", "manifest changed during invocation")
    except (BenchmarkError, ValueError, KeyError, TypeError, OSError) as error:
        envelope["comparability"]["reasons"].append(getattr(error, "code", "prepared_state_changed"))
        if state == "completed":
            state, reason = "input_changed", getattr(error, "code", "prepared_state_changed")
    if not measured["complete"]:
        envelope["comparability"]["reasons"].append("telemetry_incomplete")
    if measured["budget_exceeded"]:
        envelope["comparability"]["reasons"].append("model_budget_exceeded")
    if unexpected_tools:
        envelope["comparability"]["reasons"].append("unexpected_tool")
    if state != "completed":
        envelope["comparability"]["reasons"].append(state)
    envelope["run_status"] = {"state": state, "reason": reason}
    write_new(trial / "result.json", envelope)
    return envelope


def prepare_batch(*, repetitions: int = 3, corpus_case: dict | None = None, **kwargs) -> Path:
    if type(repetitions) is not int or not 1 <= repetitions <= 20:
        raise BenchmarkError("invalid_repetitions", "use 1..20 repetitions")
    output = kwargs.pop("output").resolve()
    output.mkdir(parents=True, exist_ok=True)
    batch = output / ("batch-" + uuid.uuid4().hex)
    batch.mkdir(mode=0o700)
    trials = []
    for repetition in range(repetitions):
        offset = repetition % len(CONDITIONS)
        for condition in CONDITIONS[offset:] + CONDITIONS[:offset]:
            trial = prepare_trial(output=batch, condition=condition, repetition=repetition, **kwargs)
            manifest = load_json(trial / "manifest.json")
            trials.append({"directory": trial.name, "condition": condition, "repetition": repetition,
                           "common_sha256": manifest.get("common_sha256"), "status": manifest["status"]})
    summary = {"kind": "mastermind-research-batch", "schema_version": 1,
              "task_id": kwargs["task"]["id"], "repetitions": repetitions, "trials": trials,
              "quality_uplift": None, "comparison_accepted": False}
    if corpus_case is not None:
        summary["corpus_case"] = corpus_case
    write_new(batch / "batch.json", summary)
    return batch


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    prepare = sub.add_parser("prepare", help="freeze allowlisted trials; never invoke a model")
    for name in ("config", "source-repo", "tool-repo", "output"):
        prepare.add_argument("--" + name, type=Path, required=True)
    selection = prepare.add_mutually_exclusive_group(required=True)
    selection.add_argument("--task", type=Path)
    selection.add_argument("--case", help="select a checked calibration from the corpus")
    prepare.add_argument("--rubric", type=Path)
    prepare.add_argument("--corpus", type=Path, help="corpus registry for --case; defaults to the bundled corpus")
    prepare.add_argument("--repetitions", type=int, default=3)
    execute = sub.add_parser("run", help="invoke the pinned trusted adapter exactly once")
    execute.add_argument("trial", type=Path)
    execute.add_argument("--credential-env", action="append", choices=sorted(CREDENTIAL_NAMES), default=[])
    args = parser.parse_args(argv)
    try:
        if args.command == "prepare":
            corpus_case = None
            config = load_json(args.config)
            if args.case is not None:
                if args.rubric is not None:
                    raise BenchmarkError("invalid_selection", "--case uses its bound corpus rubric")
                if __package__:
                    from . import benchmark_corpus
                else:
                    import benchmark_corpus
                try:
                    selected = benchmark_corpus.select_case(args.corpus or benchmark_corpus.DEFAULT_CORPUS, args.case, args.source_repo)
                    config = benchmark_corpus.configure_case(selected, config)
                except benchmark_corpus.bench.BenchmarkError as error:
                    # Script entry points run as __main__; the corpus imports
                    # the named module, which has a distinct exception class.
                    raise BenchmarkError(error.code, str(error)) from error
                task, rubric, corpus_case = selected["task"], selected["rubric"], selected["summary"]
            else:
                if args.rubric is None or args.corpus is not None:
                    raise BenchmarkError("invalid_selection", "--task requires --rubric and cannot use --corpus")
                task, rubric = load_json(args.task), load_json(args.rubric)
            batch = prepare_batch(task=task, rubric=rubric, config=config, corpus_case=corpus_case,
                source_repo=args.source_repo, tool_repo=args.tool_repo, output=args.output, repetitions=args.repetitions)
            print(batch)
            return 0 if all(t["status"] == "prepared" for t in load_json(batch / "batch.json")["trials"]) else 2
        credentials = {name: os.environ[name] for name in args.credential_env if name in os.environ}
        if len(credentials) != len(set(args.credential_env)):
            raise BenchmarkError("credentials_missing", "a requested credential variable is absent")
        result = run_trial(args.trial, credentials)
        print(json.dumps({"trial": str(args.trial), "run_status": result["run_status"],
                          "quality": result["quality"], "comparison_accepted": False}))
        return 0 if result["run_status"]["state"] == "completed" else 2
    except (BenchmarkError, OSError, ValueError, KeyError, TypeError) as error:
        print(f"{getattr(error, 'code', 'benchmark_error')}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
