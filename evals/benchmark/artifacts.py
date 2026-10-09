"""Bounded JSON, hashing and immutable artifact publication."""

from __future__ import annotations

from pathlib import Path
import hashlib
import json
import os
import re
import stat


FILE_BYTE_LIMIT = 2 * 1024 * 1024

CONTROL_BYTE_LIMIT = 1024 * 1024

BINARY_BYTE_LIMIT = 512 * 1024 * 1024


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
