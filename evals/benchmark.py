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
from contextlib import closing, contextmanager
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


def file_identity(info: os.stat_result) -> tuple[int, int, int, int, int, int]:
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns,
            info.st_ctime_ns, info.st_mode)


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
        if (len(body) > limit or file_identity(before) != file_identity(after)
                or file_identity(after) != file_identity(current)):
            raise BenchmarkError("file_changed", f"file changed while reading: {path}")
        return body
    except OSError as error:
        raise BenchmarkError("file_unavailable", f"cannot read file: {path}") from error


def hash_file(path: Path, limit: int, prefix_limit: int = 0) -> dict:
    """Stream a stable regular-file digest without retaining the whole file."""
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    try:
        with os.fdopen(os.open(path, flags), "rb") as handle:
            before = os.fstat(handle.fileno())
            if not stat.S_ISREG(before.st_mode) or before.st_size > limit:
                raise BenchmarkError("invalid_file", f"not a bounded regular file: {path}")
            hasher = hashlib.sha256()
            prefix = bytearray()
            size = 0
            while True:
                chunk = handle.read(min(1024 * 1024, limit - size + 1))
                if not chunk:
                    break
                size += len(chunk)
                if size > limit:
                    raise BenchmarkError("invalid_file", f"not a bounded regular file: {path}")
                hasher.update(chunk)
                if len(prefix) < prefix_limit:
                    prefix.extend(chunk[:prefix_limit - len(prefix)])
            after = os.fstat(handle.fileno())
        current = path.lstat()
        if (file_identity(before) != file_identity(after)
                or file_identity(after) != file_identity(current)):
            raise BenchmarkError("file_changed", f"file changed while reading: {path}")
        return {"bytes": size, "sha256": hasher.hexdigest(),
                "identity": file_identity(after), "prefix": bytes(prefix)}
    except OSError as error:
        raise BenchmarkError("file_unavailable", f"cannot read file: {path}") from error


def load_json(path: Path) -> dict:
    try:
        return parse_json(read_file(path))
    except (ValueError, UnicodeError) as error:
        raise BenchmarkError("invalid_json", f"invalid JSON: {path}") from error


def _same_node(left: os.stat_result, right: os.stat_result) -> bool:
    return (left.st_dev, left.st_ino) == (right.st_dev, right.st_ino)


def _check_output_parent(parent: Path, descriptor: int, expected: os.stat_result) -> None:
    try:
        observed = parent.stat(follow_symlinks=False)
        canonical_parent = parent.resolve(strict=True)
    except (OSError, RuntimeError) as error:
        raise BenchmarkError("artifact_changed", "artifact parent changed during publication") from error
    if (not stat.S_ISDIR(observed.st_mode) or not _same_node(observed, expected)
            or not _same_node(os.fstat(descriptor), expected) or canonical_parent != parent):
        raise BenchmarkError("artifact_changed", "artifact parent changed during publication")


def write_new_bytes(path: Path, body: bytes, *, mode: int = 0o600) -> None:
    """Publish one owned regular file and verify its final name and bytes."""
    if (os.name != "posix" or not hasattr(os, "O_NOFOLLOW")
            or not isinstance(body, bytes) or mode not in {0o400, 0o444, 0o555, 0o600}):
        raise BenchmarkError("artifact_platform", "verified artifact publication requires POSIX")
    target = path.absolute()
    parent = target.parent
    if not target.name or target.name in {".", ".."}:
        raise BenchmarkError("artifact_path", "invalid artifact path")
    try:
        if parent.resolve(strict=True) != parent:
            raise BenchmarkError("artifact_path", "artifact parent must be canonical without links")
        parent_descriptor = os.open(
            parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except BenchmarkError:
        raise
    except OSError as error:
        raise BenchmarkError("artifact_path", "cannot open artifact parent without links") from error
    descriptor = None
    owned = None
    published = False
    expected_parent = os.fstat(parent_descriptor)
    try:
        if not stat.S_ISDIR(expected_parent.st_mode):
            raise BenchmarkError("artifact_path", "artifact parent is not a directory")
        try:
            descriptor = os.open(
                target.name,
                os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_NONBLOCK,
                mode,
                dir_fd=parent_descriptor,
            )
            published = True
        except FileExistsError as error:
            raise BenchmarkError("artifact_exists", f"artifact already exists: {target}") from error
        opened = os.fstat(descriptor)
        owned = (opened.st_dev, opened.st_ino)
        os.fchmod(descriptor, mode)
        with os.fdopen(os.dup(descriptor), "wb") as handle:
            handle.write(body)
            handle.flush()
            os.fsync(handle.fileno())
        written = os.fstat(descriptor)
        named = os.stat(target.name, dir_fd=parent_descriptor, follow_symlinks=False)
        if (not stat.S_ISREG(written.st_mode) or stat.S_IMODE(written.st_mode) != mode
                or written.st_size != len(body) or not _same_node(written, named)):
            raise BenchmarkError("artifact_changed", "artifact changed during publication")
        os.fsync(parent_descriptor)
        _check_output_parent(parent, parent_descriptor, expected_parent)

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
        try:
            observed = read_file(target, len(body))
            current = target.lstat()
        except (BenchmarkError, OSError) as error:
            raise BenchmarkError("artifact_changed", "artifact changed during verification") from error
        if (bytes(retained) != body or observed != body
                or (written.st_dev, written.st_ino) != owned
                or not _same_node(written, current)
                or file_identity(written) != file_identity(current)):
            raise BenchmarkError("artifact_changed", "artifact changed during verification")
        _check_output_parent(parent, parent_descriptor, expected_parent)
    except (BenchmarkError, OSError, KeyboardInterrupt):
        if published:
            try:
                current = os.stat(target.name, dir_fd=parent_descriptor, follow_symlinks=False)
                if (current.st_dev, current.st_ino) == owned:
                    os.unlink(target.name, dir_fd=parent_descriptor)
                    os.fsync(parent_descriptor)
            except FileNotFoundError:
                pass
        raise
    finally:
        if descriptor is not None:
            os.close(descriptor)
        os.close(parent_descriptor)


def write_new(path: Path, value: object, *, mode: int = 0o600) -> None:
    write_new_bytes(path, canonical(value) + b"\n", mode=mode)


def exact_revision(value: object) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", value):
        raise BenchmarkError("invalid_revision", "an exact lowercase Git commit ID is required")
    return value


def validate_batch_binding(value: object) -> dict:
    if (not isinstance(value, dict)
            or set(value) != {"batch_id", "plan_sha256", "position"}
            or not isinstance(value.get("batch_id"), str)
            or not re.fullmatch(r"batch-[0-9a-f]{32}", value["batch_id"])
            or not isinstance(value.get("plan_sha256"), str)
            or not re.fullmatch(r"[0-9a-f]{64}", value["plan_sha256"])
            or type(value.get("position")) is not int or value["position"] < 0):
        raise BenchmarkError("invalid_batch_binding", "invalid trial batch binding")
    return value


def batch_plan_identity(batch: dict) -> dict:
    return {"batch_id": batch["batch_id"], "task_id": batch["task_id"],
            "repetitions": batch["repetitions"],
            "trials": [{key: item[key] for key in ("directory", "condition", "repetition")}
                       for item in batch["trials"]]}


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
        "GIT_NO_LAZY_FETCH": "1", "GIT_ALLOW_PROTOCOL": "",
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


def verify_tool_commit(repo: Path, revision: str, env: dict[str, str]) -> None:
    try:
        resolved = git(repo, ["rev-parse", "--verify", f"{revision}^{{commit}}"], env).decode("ascii").strip()
    except (BenchmarkError, UnicodeError) as error:
        raise BenchmarkError(
            "tool_revision_unavailable",
            "tool revision must be an available exact commit in the selected tool repository",
        ) from error
    if resolved != revision:
        raise BenchmarkError(
            "tool_revision_mismatch",
            "tool revision does not resolve to the requested exact commit",
        )


def git_regular_blob(repo: Path, revision: str, path: str, env: dict[str, str],
                     *, role: str, byte_limit: int) -> tuple[bytes, str]:
    """Read one exact regular blob while preserving its Git path and mode."""
    tree = git(repo, ["ls-tree", "-z", revision, "--", path], env, CONTROL_BYTE_LIMIT)
    records = tree.rstrip(b"\0").split(b"\0")
    if len(records) != 1 or b"\t" not in records[0]:
        raise BenchmarkError(f"{role}_missing", f"expected one exact {role} file: {path}")
    try:
        header, name = records[0].split(b"\t", 1)
        mode, kind, oid = header.decode("ascii").split()
        exact_name = name.decode("utf-8")
    except (UnicodeError, ValueError) as error:
        raise BenchmarkError(f"{role}_type", f"invalid {role} tree entry: {path}") from error
    if (exact_name != path or kind != "blob" or mode not in {"100644", "100755"}
            or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", oid)):
        raise BenchmarkError(f"{role}_type", f"only regular {role} blobs are allowed: {path}")
    try:
        size = int(git(repo, ["cat-file", "-s", oid], env, CONTROL_BYTE_LIMIT).decode("ascii").strip())
    except (BenchmarkError, UnicodeError, ValueError) as error:
        raise BenchmarkError(f"{role}_type", f"cannot inspect {role} blob: {path}") from error
    if not 0 <= size <= byte_limit:
        raise BenchmarkError(f"{role}_limit", f"{role} blob exceeds its byte limit: {path}")
    body = git(repo, ["cat-file", "blob", oid], env, byte_limit)
    if len(body) != size:
        raise BenchmarkError(f"{role}_changed", f"{role} blob changed while reading: {path}")
    return body, mode


def export_source(repo: Path, revision: str, paths: list[str], destination: Path,
                  env: dict[str, str]) -> tuple[list[dict], str]:
    resolved = git(repo, ["rev-parse", "--verify", f"{revision}^{{commit}}"], env).decode().strip()
    if resolved != revision:
        raise BenchmarkError("source_revision_mismatch", "source revision is not the requested commit")
    destination.mkdir()
    files, total = [], 0
    for path in sorted(paths):
        body, mode = git_regular_blob(
            repo, revision, path, env, role="source", byte_limit=FILE_BYTE_LIMIT,
        )
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
        if hash_file(path, BINARY_BYTE_LIMIT)["sha256"] != expected:
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
    identity = {key: manifest[key] for key in ("common_sha256", "condition", "instruction_sha256")} | {
        "indexer": {key: indexer[key] for key in ("sha256", "version", "source_revision", "origin")} if indexer else None,
        "index_contract": manifest.get("index_contract"), "indexed_files": manifest.get("indexed_files")}
    if "batch" in manifest:
        identity["batch"] = manifest["batch"]
    return identity


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
    def verify_sidecars() -> None:
        for suffix in ("-wal", "-journal", "-shm"):
            sidecar = path.with_name(path.name + suffix)
            try:
                sidecar.lstat()
            except FileNotFoundError:
                continue
            except OSError as error:
                raise BenchmarkError(
                    "index_uncheckpointed",
                    "cannot establish an exclusive checkpointed SQLite database",
                ) from error
            raise BenchmarkError(
                "index_uncheckpointed", "index has an uncheckpointed SQLite sidecar"
            )

    verify_sidecars()
    before = hash_file(path, BINARY_BYTE_LIMIT, len(b"SQLite format 3\0"))
    if before["prefix"] != b"SQLite format 3\0":
        raise BenchmarkError("index_invalid", "indexer did not produce a SQLite database")
    try:
        with closing(sqlite3.connect(path.as_uri() + "?mode=ro&immutable=1", uri=True)) as database:
            database.set_progress_handler(lambda: 1, 1_000_000)
            if database.execute("PRAGMA quick_check").fetchone() != ("ok",):
                raise BenchmarkError("index_invalid", "SQLite integrity check failed")
            for name in ("symbols", "edges", "meta", "files"):
                if database.execute(
                        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=? LIMIT 1",
                        (name,)).fetchone() != (1,):
                    raise BenchmarkError("index_invalid", "index is missing mmcg tables")
            required_metadata = dict(contract, index_root=str(source.resolve()))
            for key, value in required_metadata.items():
                rows = database.execute("SELECT value FROM meta WHERE key=? LIMIT 2", (key,)).fetchall()
                if rows != [(value,)]:
                    raise BenchmarkError("index_contract_mismatch" if key != "index_root" else
                                         "index_root_mismatch", "index metadata differs from its trial contract")
            expected = {item["path"]: item["sha256"] for item in files if item["path"] in indexed_paths}
            if database.execute("SELECT COUNT(*) FROM files").fetchone() != (len(expected),):
                raise BenchmarkError("index_source_mismatch", "index does not cover the exact declared source bytes")
            actual = list(database.execute(
                "SELECT path,content_sha256 FROM files ORDER BY path LIMIT ?", (len(expected) + 1,)))
            if actual != sorted(expected.items()):
                raise BenchmarkError("index_source_mismatch", "index does not cover the exact declared source bytes")
    except sqlite3.Error as error:
        raise BenchmarkError("index_invalid", "cannot validate the produced SQLite index") from error
    after = hash_file(path, BINARY_BYTE_LIMIT, len(b"SQLite format 3\0"))
    verify_sidecars()
    if after != before:
        raise BenchmarkError("index_changed", "index changed during validation")
    return before["sha256"]


def prepare_trial(
    *, task: dict, rubric: dict, config: dict, source_repo: Path, tool_repo: Path,
    output: Path, condition: str, repetition: int = 0, trial_id: str | None = None,
    batch_binding: dict | None = None,
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
    if ((trial_id is None) != (batch_binding is None)
            or (trial_id is not None and not re.fullmatch(r"trial-[0-9a-f]{32}", trial_id))):
        raise BenchmarkError("invalid_batch_binding", "batch trials need an exact planned ID and binding")
    if batch_binding is not None:
        batch_binding = dict(validate_batch_binding(batch_binding))
    output.mkdir(parents=True, exist_ok=True)
    trial = output.resolve() / (trial_id or ("trial-" + uuid.uuid4().hex))
    trial.mkdir(mode=0o700)
    env = clean_environment(trial)
    prepared = time.monotonic()
    manifest = {"kind": "mastermind-research-trial", "schema_version": 3 if batch_binding else 2,
                "trial_id": trial.name, "task": task, "condition": condition,
                "repetition": repetition, "rubric_sha256": digest(rubric),
                "model": model, "limits": limits, "tool_revision": tool_revision,
                "status": "setup_failed", "isolation": "host_adapter_unverified"}
    if batch_binding is not None:
        manifest["batch"] = batch_binding
    write_new(trial / "rubric.json", rubric)
    try:
        verify_tool_commit(tool_repo.resolve(strict=True), tool_revision, env)
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
            instruction = git_regular_blob(
                tool_repo.resolve(), tool_revision, instruction_path, env,
                role="instruction", byte_limit=CONTROL_BYTE_LIMIT,
            )[0].decode("utf-8")
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
            write_new_bytes(trial / "index-stdout.txt", process.stdout)
            write_new_bytes(trial / "index-stderr.txt", process.stderr)
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
    if manifest.get("kind") != "mastermind-research-trial" or manifest.get("schema_version") not in (1, 2, 3):
        raise BenchmarkError("invalid_manifest", "unknown trial manifest")
    if manifest.get("schema_version") == 3 and "batch" not in manifest:
        raise BenchmarkError("invalid_manifest", "batch-bound manifest is missing its binding")
    if "batch" in manifest:
        validate_batch_binding(manifest["batch"])
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
    if sum(item["bytes"] for item in files) > SOURCE_BYTE_LIMIT:
        raise BenchmarkError("source_limit", "source manifest exceeds its total byte limit")
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


def validate_batch_summary(batch: dict) -> list[dict]:
    required = {"kind", "schema_version", "batch_id", "plan_sha256", "task_id",
                "repetitions", "trials", "quality_uplift", "comparison_accepted"}
    if (not isinstance(batch, dict) or not required <= set(batch)
            or set(batch) - required - {"corpus_case"}
            or batch.get("kind") != "mastermind-research-batch"
            or batch.get("schema_version") != 2
            or not isinstance(batch.get("batch_id"), str)
            or not re.fullmatch(r"batch-[0-9a-f]{32}", batch["batch_id"])
            or not isinstance(batch.get("task_id"), str) or not batch["task_id"]
            or type(batch.get("repetitions")) is not int or not 1 <= batch["repetitions"] <= 20
            or batch.get("quality_uplift") is not None or batch.get("comparison_accepted") is not False
            or not isinstance(batch.get("trials"), list)
            or len(batch["trials"]) != 3 * batch["repetitions"]):
        raise BenchmarkError("batch_changed", "invalid bound batch plan")
    seen = set()
    for position, item in enumerate(batch["trials"]):
        common = item.get("common_sha256") if isinstance(item, dict) else None
        invalid_common = (common is not None
                          and (not isinstance(common, str) or not re.fullmatch(r"[0-9a-f]{64}", common)))
        if (not isinstance(item, dict)
                or set(item) != {"directory", "condition", "repetition", "common_sha256", "status"}
                or not isinstance(item.get("directory"), str)
                or not re.fullmatch(r"trial-[0-9a-f]{32}", item["directory"])
                or item["directory"] in seen
                or type(item.get("repetition")) is not int
                or item.get("status") not in {"prepared", "setup_failed"}
                or invalid_common):
            raise BenchmarkError("batch_changed", "invalid bound batch slot")
        seen.add(item["directory"])
        repetition, offset = divmod(position, len(CONDITIONS))
        if (item.get("repetition") != repetition
                or item.get("condition") != CONDITIONS[(repetition + offset) % len(CONDITIONS)]):
            raise BenchmarkError("batch_changed", "bound batch order differs from its planned matrix")
    if (not isinstance(batch.get("plan_sha256"), str)
            or digest(batch_plan_identity(batch)) != batch["plan_sha256"]):
        raise BenchmarkError("batch_changed", "bound batch plan identity changed")
    return batch["trials"]


def optional_artifact(path: Path, limit: int) -> tuple[bytes, tuple[int, ...]] | None:
    try:
        path.lstat()
    except FileNotFoundError:
        return None
    body = read_file(path, limit)
    return body, file_identity(path.lstat())


def load_bound_batch(trial: Path, manifest: dict, manifest_bytes: bytes) -> dict:
    binding = validate_batch_binding(manifest.get("batch"))
    batch_root = trial.parent
    try:
        root_before = file_identity(batch_root.lstat())
        if not stat.S_ISDIR(root_before[-1]):
            raise BenchmarkError("batch_changed", "batch root is not a directory")
        batch_path = batch_root / "batch.json"
        batch_bytes = read_file(batch_path)
        batch_identity = file_identity(batch_path.lstat())
        batch = parse_json(batch_bytes)
    except (OSError, ValueError) as error:
        raise BenchmarkError("batch_changed", "cannot read the bound batch plan") from error
    items = validate_batch_summary(batch)
    task = manifest.get("task")
    if (batch["batch_id"] != binding["batch_id"]
            or batch["plan_sha256"] != binding["plan_sha256"]
            or not isinstance(task, dict) or task.get("id") != batch["task_id"]
            or binding["position"] >= len(items)):
        raise BenchmarkError("batch_changed", "trial binding differs from its batch plan")
    manifest_records = []
    for position, item in enumerate(items):
        directory = batch_root / item["directory"]
        try:
            if directory.is_symlink() or not directory.is_dir() or directory.resolve(strict=True) != directory.absolute():
                raise BenchmarkError("batch_changed", "batch trial directory changed")
            body = read_file(directory / "manifest.json")
            identity = file_identity((directory / "manifest.json").lstat())
            value = parse_json(body)
        except (OSError, ValueError) as error:
            raise BenchmarkError("batch_changed", "batch trial manifest is unavailable") from error
        expected_binding = {"batch_id": batch["batch_id"], "plan_sha256": batch["plan_sha256"],
                            "position": position}
        if (value.get("kind") != "mastermind-research-trial"
                or value.get("schema_version") not in (1, 2, 3)
                or value.get("batch") != expected_binding
                or value.get("trial_id") != item["directory"]
                or value.get("condition") != item["condition"]
                or value.get("repetition") != item["repetition"]
                or value.get("status") != item["status"]
                or value.get("common_sha256") != item["common_sha256"]):
            raise BenchmarkError("batch_changed", "trial manifest differs from its batch slot")
        if position == binding["position"] and (body != manifest_bytes or value != manifest):
            raise BenchmarkError("batch_changed", "selected trial manifest changed during batch validation")
        manifest_records.append({"bytes": body, "identity": identity,
                                 "sha256": hashlib.sha256(body).hexdigest()})
    if file_identity(batch_root.lstat()) != root_before:
        raise BenchmarkError("batch_changed", "batch root changed during validation")
    return {"root": batch_root, "root_identity": root_before, "batch_bytes": batch_bytes,
            "batch_identity": batch_identity, "batch": batch, "items": items,
            "manifests": manifest_records, "position": binding["position"]}


def batch_attempt_state(snapshot: dict, *, claimed: bool) -> tuple[dict, dict[str, tuple[int, ...] | None]]:
    previous_hash = None
    fingerprints = {}
    current = snapshot["position"]
    for position, item in enumerate(snapshot["items"]):
        directory = snapshot["root"] / item["directory"]
        lock = optional_artifact(directory / "run.lock", 0)
        result = optional_artifact(directory / "result.json", CONTROL_BYTE_LIMIT)
        if position < current:
            if lock is None or result is None:
                raise BenchmarkError("batch_order", "every earlier batch attempt must finish first")
            try:
                value = parse_json(result[0])
            except ValueError as error:
                raise BenchmarkError("batch_order", "an earlier batch result is invalid") from error
            expected = {"batch_id": snapshot["batch"]["batch_id"],
                        "plan_sha256": snapshot["batch"]["plan_sha256"], "position": position,
                        "previous_result_sha256": previous_hash}
            if (value.get("kind") != "mastermind-research-result"
                    or value.get("schema_version") != 2
                    or value.get("trial_id") != item["directory"]
                    or value.get("manifest_sha256") != snapshot["manifests"][position]["sha256"]
                    or value.get("batch_execution") != expected):
                raise BenchmarkError("batch_order", "an earlier result breaks the batch execution chain")
            previous_hash = hashlib.sha256(result[0]).hexdigest()
        elif position == current:
            if result is not None or (lock is None) == claimed:
                raise BenchmarkError("already_run" if not claimed else "batch_changed",
                                     "trial was already attempted; prepare a new balanced batch")
        elif lock is not None or result is not None:
            raise BenchmarkError("batch_order", "a later batch attempt was started out of order")
        if position != current:
            fingerprints[f"{item['directory']}/run.lock"] = lock[1] if lock else None
            fingerprints[f"{item['directory']}/result.json"] = result[1] if result else None
    receipt = {"batch_id": snapshot["batch"]["batch_id"],
               "plan_sha256": snapshot["batch"]["plan_sha256"], "position": current,
               "previous_result_sha256": previous_hash}
    return receipt, fingerprints


@contextmanager
def batch_execution_guard(trial: Path, manifest: dict, manifest_bytes: bytes):
    if "batch" not in manifest:
        yield None
        return
    if os.name != "posix" or not hasattr(os, "O_NOFOLLOW"):
        raise BenchmarkError("batch_platform", "batch execution requires POSIX no-follow file locking")
    import fcntl
    path = trial.parent / "execution.lock"
    try:
        descriptor = os.open(path, os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError as error:
        raise BenchmarkError("batch_lock", "cannot acquire the batch execution lock") from error
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_size != 0:
            raise BenchmarkError("batch_lock", "batch execution lock is invalid")
        current = path.lstat()
        if file_identity(before) != file_identity(current):
            raise BenchmarkError("batch_lock", "batch execution lock changed while opening")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise BenchmarkError("batch_busy", "another batch attempt is still running") from error
    except OSError as error:
        os.close(descriptor)
        raise BenchmarkError("batch_lock", "cannot acquire the batch execution lock") from error
    except Exception:
        os.close(descriptor)
        raise
    try:
        snapshot = load_bound_batch(trial, manifest, manifest_bytes)
        receipt, fingerprints = batch_attempt_state(snapshot, claimed=False)
        snapshot.update(receipt=receipt, fingerprints=fingerprints,
                        lock_identity=file_identity(before))
        yield snapshot
    finally:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
        finally:
            os.close(descriptor)


def verify_batch_snapshot(trial: Path, manifest: dict, manifest_bytes: bytes,
                          execution: dict | None) -> None:
    if execution is None:
        return
    current = load_bound_batch(trial, manifest, manifest_bytes)
    receipt, fingerprints = batch_attempt_state(current, claimed=True)
    if (receipt != execution["receipt"] or fingerprints != execution["fingerprints"]
            or current["root_identity"] != execution["root_identity"]
            or current["batch_bytes"] != execution["batch_bytes"]
            or current["batch_identity"] != execution["batch_identity"]
            or current["manifests"] != execution["manifests"]
            or file_identity((trial.parent / "execution.lock").lstat()) != execution["lock_identity"]):
        raise BenchmarkError("batch_changed", "batch inputs changed during the attempt")


def claim_attempt(trial: Path) -> None:
    if optional_artifact(trial / "result.json", CONTROL_BYTE_LIMIT) is not None:
        raise BenchmarkError("already_run", "trial was already attempted; prepare a new balanced batch")
    try:
        write_new_bytes(trial / "run.lock", b"")
    except BenchmarkError as error:
        if error.code != "artifact_exists":
            raise BenchmarkError("attempt_lock", "cannot claim the trial attempt") from error
        raise BenchmarkError("already_run", "trial was already attempted; prepare a new balanced batch") from error


def _run_trial_attempt(trial: Path, manifest_bytes: bytes, manifest: dict,
                       credentials: dict[str, str] | None, execution: dict | None) -> dict:
    # No implicit reruns: each planned condition/repetition gets one attempt.
    claim_attempt(trial)
    envelope = {"kind": "mastermind-research-result", "schema_version": 2 if execution else 1,
                "trial_id": trial.name, "manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(),
                "common_sha256": manifest.get("common_sha256"),
                "condition_sha256": manifest.get("condition_sha256"),
                "run_status": {"state": "setup_error", "reason": None},
                "quality": {"status": "not_evaluated", "score": None},
                "diagnostics": {}, "answer": None,
                "comparability": {"eligible": False, "isolation": "host_adapter_unverified",
                    "reasons": ["adapter_isolation_unverified", "runtime_provenance_declared"]}}
    if execution is not None:
        envelope["batch_execution"] = execution["receipt"]
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
    write_new_bytes(trial / "trace.jsonl", process.stdout)
    write_new_bytes(trial / "stderr.txt", process.stderr)
    init, result, tools, issues = parse_stream(process.stdout, limits["answer_bytes"])
    state, reason = "completed", None
    reported_failure = result.get("failure") if result else None
    reported_model_error = result.get("model_error") is True if result else False
    identity_changed = bool(init) and (
        init.get("adapter_version") != manifest["adapter"]["version"]
        or (init.get("model") is not None and init.get("model") != manifest["model"])
        or (init.get("model") is None and not reported_failure and not reported_model_error)
    )
    if process.stop_reason:
        state = process.stop_reason if process.stop_reason in {"timeout", "output_limit"} else "invocation_error"
        reason = process.stop_reason
    elif process.returncode != 0:
        state, reason = "invocation_error", "nonzero_exit"
    elif issues:
        state, reason = "protocol_error", issues[0]
    elif identity_changed:
        state, reason = "identity_mismatch", "observed_model_or_adapter_version_mismatch"
    elif reported_failure:
        state, reason = reported_failure["state"], reported_failure["code"]
    elif reported_model_error:
        state, reason = "model_error", "adapter_reported_model_error"
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
        write_new_bytes(trial / "answer.md", answer_bytes)
        envelope["answer"] = {"path": "answer.md", "bytes": len(answer_bytes),
                              "sha256": hashlib.sha256(answer_bytes).hexdigest()}
        envelope["quality"]["status"] = "review_pending"
    try:
        verify_prepared(trial, manifest)
        verify_batch_snapshot(trial, manifest, manifest_bytes, execution)
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


def run_trial(trial: Path, credentials: dict[str, str] | None = None) -> dict:
    trial = trial.resolve(strict=True)
    manifest_bytes = read_file(trial / "manifest.json")
    manifest = parse_json(manifest_bytes)
    with batch_execution_guard(trial, manifest, manifest_bytes) as execution:
        return _run_trial_attempt(trial, manifest_bytes, manifest, credentials, execution)


def prepare_batch(*, repetitions: int = 3, corpus_case: dict | None = None, **kwargs) -> Path:
    if type(repetitions) is not int or not 1 <= repetitions <= 20:
        raise BenchmarkError("invalid_repetitions", "use 1..20 repetitions")
    output = kwargs.pop("output").resolve()
    output.mkdir(parents=True, exist_ok=True)
    batch_id = "batch-" + uuid.uuid4().hex
    batch = output / batch_id
    batch.mkdir(mode=0o700)
    write_new_bytes(batch / "execution.lock", b"")
    planned = []
    for repetition in range(repetitions):
        offset = repetition % len(CONDITIONS)
        for condition in CONDITIONS[offset:] + CONDITIONS[:offset]:
            planned.append({"directory": "trial-" + uuid.uuid4().hex, "condition": condition,
                            "repetition": repetition})
    plan = {"batch_id": batch_id, "task_id": kwargs["task"]["id"], "repetitions": repetitions,
            "trials": planned}
    plan_sha256 = digest(plan)
    trials = []
    for position, item in enumerate(planned):
        binding = {"batch_id": batch_id, "plan_sha256": plan_sha256, "position": position}
        trial = prepare_trial(output=batch, condition=item["condition"], repetition=item["repetition"],
                              trial_id=item["directory"], batch_binding=binding, **kwargs)
        manifest = load_json(trial / "manifest.json")
        trials.append(dict(item, common_sha256=manifest.get("common_sha256"), status=manifest["status"]))
    summary = {"kind": "mastermind-research-batch", "schema_version": 2,
              "batch_id": batch_id, "plan_sha256": plan_sha256,
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
