"""Pinned Git projections and read-only SQLite index validation."""

from __future__ import annotations

from contextlib import closing
from pathlib import Path, PurePosixPath
import hashlib
import os
import re
import shutil
import sqlite3
import stat

from . import artifacts as artifact_io
from . import conditions as condition_contract
from evals.shared import process as process_runner


def git(repo: Path, args: list[str], env: dict[str, str], limit: int = artifact_io.FILE_BYTE_LIMIT) -> bytes:
    binary = shutil.which("git")
    if binary is None:
        raise artifact_io.BenchmarkError("git_unavailable", "Git is required")
    process = process_runner.run_bounded(
        [str(Path(binary).resolve()), "--no-replace-objects", "--literal-pathspecs",
         "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
         "-c", "commit.gpgsign=false", *args], cwd=repo, env=env, stdout_limit=limit,
    )
    if process.stop_reason or process.returncode != 0:
        raise artifact_io.BenchmarkError("git_failed", f"Git source operation failed: {process.stop_reason or 'exit'}")
    return process.stdout


def verify_tool_commit(repo: Path, revision: str, env: dict[str, str]) -> None:
    try:
        resolved = git(repo, ["rev-parse", "--verify", f"{revision}^{{commit}}"], env).decode("ascii").strip()
    except (artifact_io.BenchmarkError, UnicodeError) as error:
        raise artifact_io.BenchmarkError(
            "tool_revision_unavailable",
            "tool revision must be an available exact commit in the selected tool repository",
        ) from error
    if resolved != revision:
        raise artifact_io.BenchmarkError(
            "tool_revision_mismatch",
            "tool revision does not resolve to the requested exact commit",
        )


def git_regular_blob(repo: Path, revision: str, path: str, env: dict[str, str],
                     *, role: str, byte_limit: int) -> tuple[bytes, str]:
    """Read one exact regular blob while preserving its Git path and mode."""
    tree = git(repo, ["ls-tree", "-z", revision, "--", path], env, artifact_io.CONTROL_BYTE_LIMIT)
    records = tree.rstrip(b"\0").split(b"\0")
    if len(records) != 1 or b"\t" not in records[0]:
        raise artifact_io.BenchmarkError(f"{role}_missing", f"expected one exact {role} file: {path}")
    try:
        header, name = records[0].split(b"\t", 1)
        mode, kind, oid = header.decode("ascii").split()
        exact_name = name.decode("utf-8")
    except (UnicodeError, ValueError) as error:
        raise artifact_io.BenchmarkError(f"{role}_type", f"invalid {role} tree entry: {path}") from error
    if (exact_name != path or kind != "blob" or mode not in {"100644", "100755"}
            or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", oid)):
        raise artifact_io.BenchmarkError(f"{role}_type", f"only regular {role} blobs are allowed: {path}")
    try:
        size = int(git(repo, ["cat-file", "-s", oid], env, artifact_io.CONTROL_BYTE_LIMIT).decode("ascii").strip())
    except (artifact_io.BenchmarkError, UnicodeError, ValueError) as error:
        raise artifact_io.BenchmarkError(f"{role}_type", f"cannot inspect {role} blob: {path}") from error
    if not 0 <= size <= byte_limit:
        raise artifact_io.BenchmarkError(f"{role}_limit", f"{role} blob exceeds its byte limit: {path}")
    body = git(repo, ["cat-file", "blob", oid], env, byte_limit)
    if len(body) != size:
        raise artifact_io.BenchmarkError(f"{role}_changed", f"{role} blob changed while reading: {path}")
    return body, mode


def export_source(repo: Path, revision: str, paths: list[str], destination: Path,
                  env: dict[str, str]) -> tuple[list[dict], str]:
    resolved = git(repo, ["rev-parse", "--verify", f"{revision}^{{commit}}"], env).decode().strip()
    if resolved != revision:
        raise artifact_io.BenchmarkError("source_revision_mismatch", "source revision is not the requested commit")
    destination.mkdir()
    files, total = [], 0
    for path in sorted(paths):
        body, mode = git_regular_blob(
            repo, revision, path, env, role="source", byte_limit=artifact_io.FILE_BYTE_LIMIT,
        )
        total += len(body)
        if total > condition_contract.SOURCE_BYTE_LIMIT:
            raise artifact_io.BenchmarkError("source_limit", "source projection exceeds its total byte limit")
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
        raise artifact_io.BenchmarkError("source_changed", "source root is not the prepared directory")
    expected = {item["path"] for item in files}
    allowed_directories = {parent.as_posix() for path in expected
                           for parent in PurePosixPath(path).parents if parent != PurePosixPath(".")}
    observed = set()
    def walk_failed(error):
        raise artifact_io.BenchmarkError("source_changed", "cannot inspect the source inventory") from error

    for directory, names, filenames in os.walk(source, followlinks=False, onerror=walk_failed):
        directory = Path(directory)
        if directory == source:
            if (source / ".git").is_symlink() or not (source / ".git").is_dir():
                raise artifact_io.BenchmarkError("source_changed", "source Git directory changed")
            names[:] = [name for name in names if name != ".git"]
        for name in names:
            path = directory / name
            if path.is_symlink() or path.relative_to(source).as_posix() not in allowed_directories:
                raise artifact_io.BenchmarkError("source_changed", "source contains an unexpected directory")
        for name in filenames:
            observed.add((directory / name).relative_to(source).as_posix())
    if observed != expected:
        raise artifact_io.BenchmarkError("source_changed", "source inventory differs from the frozen projection")
    for item in files:
        path = source / item["path"]
        body = artifact_io.read_file(path, artifact_io.FILE_BYTE_LIMIT)
        mode = "100755" if path.lstat().st_mode & stat.S_IXUSR else "100644"
        if len(body) != item["bytes"] or hashlib.sha256(body).hexdigest() != item["sha256"] or mode != item["git_mode"]:
            raise artifact_io.BenchmarkError("source_changed", f"source no longer matches: {item['path']}")


def verify_projection(source: Path, files: list[dict], revision: str, env: dict[str, str]) -> None:
    artifact_io.exact_revision(revision)
    if git(source, ["rev-parse", "HEAD"], env).decode().strip() != revision:
        raise artifact_io.BenchmarkError("source_changed", "source Git projection changed")
    if git(source, ["rev-list", "--all", "--count"], env).strip() != b"1":
        raise artifact_io.BenchmarkError("source_changed", "source projection has additional Git history")
    entries = []
    for item in sorted(files, key=lambda item: item["path"]):
        body = artifact_io.read_file(source / item["path"], artifact_io.FILE_BYTE_LIMIT)
        algorithm = hashlib.sha1 if len(revision) == 40 else hashlib.sha256
        oid = algorithm(f"blob {len(body)}\0".encode() + body).hexdigest()
        entries.append(f"{item['git_mode']} blob {oid}\t{item['path']}".encode() + b"\0")
    # Compare as records because Git tree order differs for nested directories.
    actual = git(source, ["ls-tree", "-rz", "HEAD"], env, artifact_io.CONTROL_BYTE_LIMIT)
    if sorted(actual.split(b"\0")[:-1]) != sorted(entry[:-1] for entry in entries):
        raise artifact_io.BenchmarkError("source_changed", "Git tree differs from the allowed source bytes")


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
                raise artifact_io.BenchmarkError(
                    "index_uncheckpointed",
                    "cannot establish an exclusive checkpointed SQLite database",
                ) from error
            raise artifact_io.BenchmarkError(
                "index_uncheckpointed", "index has an uncheckpointed SQLite sidecar"
            )

    verify_sidecars()
    before = artifact_io.hash_file(path, artifact_io.BINARY_BYTE_LIMIT, len(b"SQLite format 3\0"))
    if before["prefix"] != b"SQLite format 3\0":
        raise artifact_io.BenchmarkError("index_invalid", "indexer did not produce a SQLite database")
    try:
        with closing(sqlite3.connect(path.as_uri() + "?mode=ro&immutable=1", uri=True)) as database:
            database.set_progress_handler(lambda: 1, 1_000_000)
            if database.execute("PRAGMA quick_check").fetchone() != ("ok",):
                raise artifact_io.BenchmarkError("index_invalid", "SQLite integrity check failed")
            for name in ("symbols", "edges", "meta", "files"):
                if database.execute(
                        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=? LIMIT 1",
                        (name,)).fetchone() != (1,):
                    raise artifact_io.BenchmarkError("index_invalid", "index is missing mmcg tables")
            required_metadata = dict(contract, index_root=str(source.resolve()))
            for key, value in required_metadata.items():
                rows = database.execute("SELECT value FROM meta WHERE key=? LIMIT 2", (key,)).fetchall()
                if rows != [(value,)]:
                    raise artifact_io.BenchmarkError("index_contract_mismatch" if key != "index_root" else
                                         "index_root_mismatch", "index metadata differs from its trial contract")
            expected = {item["path"]: item["sha256"] for item in files if item["path"] in indexed_paths}
            if database.execute("SELECT COUNT(*) FROM files").fetchone() != (len(expected),):
                raise artifact_io.BenchmarkError("index_source_mismatch", "index does not cover the exact declared source bytes")
            actual = list(database.execute(
                "SELECT path,content_sha256 FROM files ORDER BY path LIMIT ?", (len(expected) + 1,)))
            if actual != sorted(expected.items()):
                raise artifact_io.BenchmarkError("index_source_mismatch", "index does not cover the exact declared source bytes")
    except sqlite3.Error as error:
        raise artifact_io.BenchmarkError("index_invalid", "cannot validate the produced SQLite index") from error
    after = artifact_io.hash_file(path, artifact_io.BINARY_BYTE_LIMIT, len(b"SQLite format 3\0"))
    verify_sidecars()
    if after != before:
        raise artifact_io.BenchmarkError("index_changed", "index changed during validation")
    return before["sha256"]
