"""Bounded artifact reads and exclusive publication relative to directory FDs."""

from __future__ import annotations

import hashlib
import os
import stat
import uuid
from contextlib import contextmanager
from pathlib import Path, PurePosixPath

if __package__:
    from . import benchmark as bench
else:
    import benchmark as bench


READ_LIMIT = 1024 * 1024 * 1024
OUTPUT_LIMIT = 64 * 1024 * 1024


def sha(body):
    return hashlib.sha256(body).hexdigest()


def encoded(value):
    body = bench.canonical(value) + b"\n"
    if len(body) > bench.CONTROL_BYTE_LIMIT:
        raise bench.BenchmarkError("review_limit", "review JSON exceeds the control byte cap")
    return body


def relative(value):
    if (not isinstance(value, str) or not value or "\\" in value
            or any(ord(char) < 32 for char in value)):
        raise bench.BenchmarkError("review_path", "expected a relative artifact path")
    path = PurePosixPath(value)
    if path.is_absolute() or path.as_posix() != value or ".." in path.parts or value == "." or path.parts[0].endswith(":"):
        raise bench.BenchmarkError("review_path", "artifact path cannot escape its directory")
    if len(value.encode()) > 4096 or len(path.parts) > 128:
        raise bench.BenchmarkError("review_limit", "artifact path exceeds its length or depth cap")
    return path.parts


def identity(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns, info.st_ctime_ns)


class Root:
    def __init__(self, path: Path):
        if os.name != "posix" or not hasattr(os, "O_NOFOLLOW"):
            raise bench.BenchmarkError("review_platform", "review artifacts require POSIX no-follow directory operations")
        self.path = path.absolute()
        if self.path != self.path.resolve(strict=True):
            raise bench.BenchmarkError("review_path", "artifact directory must be canonical, without symbolic links")
        self.fd = os.open(self.path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        self.start = os.fstat(self.fd)
        self.records, self.listings = {}, {}
        self.read_bytes = self.written_bytes = 0

    def __enter__(self):
        return self

    def __exit__(self, *_):
        os.close(self.fd)

    def check_root(self):
        current = self.path.stat(follow_symlinks=False)
        if ((current.st_dev, current.st_ino) != (self.start.st_dev, self.start.st_ino)
                or self.path != self.path.resolve(strict=True)):
            raise bench.BenchmarkError("review_changed", "artifact directory changed during access")

    def check_parent(self, path, expected):
        """Confirm that an opened parent is still reachable from this root."""
        with self.parent(path) as (current, _):
            observed = os.fstat(current)
        if ((observed.st_dev, observed.st_ino)
                != (expected.st_dev, expected.st_ino)):
            raise bench.BenchmarkError(
                "review_changed", "artifact parent changed during access")
        self.check_root()

    @contextmanager
    def parent(self, path, create=False):
        parts = relative(path)
        descriptors = [os.dup(self.fd)]
        try:
            for part in parts[:-1]:
                if create:
                    try:
                        os.mkdir(part, 0o700, dir_fd=descriptors[-1])
                    except FileExistsError:
                        pass
                descriptors.append(os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                           dir_fd=descriptors[-1]))
            yield descriptors[-1], parts[-1]
        finally:
            for fd in reversed(descriptors):
                os.close(fd)

    def read(self, path, limit=bench.CONTROL_BYTE_LIMIT, optional=False):
        try:
            with self.parent(path) as (parent, name):
                fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
                with os.fdopen(fd, "rb") as stream:
                    before = os.fstat(stream.fileno())
                    if not stat.S_ISREG(before.st_mode) or before.st_size > limit:
                        raise bench.BenchmarkError("review_file", "artifact must be a bounded regular file")
                    body = stream.read(limit + 1)
                    after = os.fstat(stream.fileno())
                current = os.stat(name, dir_fd=parent, follow_symlinks=False)
                if len(body) > limit or identity(before) != identity(after) or identity(after) != identity(current):
                    raise bench.BenchmarkError("review_changed", "artifact changed while reading")
            self.read_bytes += len(body)
            if self.read_bytes > READ_LIMIT:
                raise bench.BenchmarkError("review_limit", "review input exceeds its aggregate read cap")
            self.check_root()
            self.records[path] = {"sha256": sha(body), "limit": limit, "identity": identity(before)}
            return body
        except FileNotFoundError:
            if not optional:
                raise bench.BenchmarkError("review_missing", f"required artifact is absent: {path}")
            self.records[path] = {"sha256": None, "limit": limit, "identity": None}
            return None
        except OSError as error:
            raise bench.BenchmarkError("review_path", f"cannot read artifact without following links: {path}") from error

    def json(self, path, optional=False):
        body = self.read(path, optional=optional)
        return None if body is None else bench.parse_json(body)

    def names(self, directory=None):
        descriptor = os.dup(self.fd)
        try:
            if directory is not None:
                with self.parent(directory) as (parent, name):
                    nested = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
                    os.close(descriptor)
                    descriptor = nested
            values = set()
            with os.scandir(descriptor) as entries:
                for entry in entries:
                    if len(values) >= 512:
                        raise bench.BenchmarkError("review_limit", "artifact directory exceeds its entry cap")
                    values.add(entry.name)
            self.check_root()
            self.listings[directory] = values
            return values
        except OSError as error:
            raise bench.BenchmarkError("review_path", "cannot enumerate artifact directory without links") from error
        finally:
            os.close(descriptor)

    def recheck(self):
        for path, expected in list(self.records.items()):
            self.read(path, expected["limit"], optional=expected["sha256"] is None)
            if self.records[path] != expected:
                raise bench.BenchmarkError("review_changed", "input artifacts changed during review processing")
        for directory, expected in list(self.listings.items()):
            if self.names(directory) != expected:
                raise bench.BenchmarkError("review_changed", "artifact inventory changed during review processing")
        self.check_root()

    def inventory(self, paths):
        expected = {}
        for path in paths:
            parts = relative(path)
            for index in range(1, len(parts)):
                expected.setdefault("/".join(parts[:index]), set()).add(parts[index])
        for directory, names in expected.items():
            if self.names(directory) != names:
                raise bench.BenchmarkError("review_inventory", "reviewer directory contains missing or unexpected artifacts")

    @contextmanager
    def exclusive_lock(self, path):
        import fcntl
        try:
            with self.parent(path) as (parent, name):
                fd = os.open(name, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600, dir_fd=parent)
                try:
                    current = os.fstat(fd)
                    if not stat.S_ISREG(current.st_mode) or current.st_size != 0:
                        raise bench.BenchmarkError("review_file", "import lock must be an empty regular file")
                    try:
                        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError as error:
                        raise bench.BenchmarkError("review_busy", "another assessment import is in progress") from error
                    if identity(current) != identity(os.stat(name, dir_fd=parent, follow_symlinks=False)):
                        raise bench.BenchmarkError("review_changed", "import lock changed during acquisition")
                    yield
                finally:
                    os.close(fd)
        except OSError as error:
            raise bench.BenchmarkError("review_path", "cannot acquire import lock without following links") from error

    def write_new(self, path, body):
        self.written_bytes += len(body)
        if self.written_bytes > OUTPUT_LIMIT:
            raise bench.BenchmarkError("review_limit", "review output exceeds its aggregate byte cap")
        temporary = ".pending-" + uuid.uuid4().hex
        try:
            with self.parent(path, create=True) as (parent, name):
                expected_parent = os.fstat(parent)
                descriptor = None
                owned = None
                staged = False
                published = False
                try:
                    descriptor = os.open(
                        temporary,
                        os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                        0o400,
                        dir_fd=parent,
                    )
                    staged = True
                    opened = os.fstat(descriptor)
                    owned = (opened.st_dev, opened.st_ino)
                    os.fchmod(descriptor, 0o400)
                    with os.fdopen(os.dup(descriptor), "wb") as stream:
                        stream.write(body)
                        stream.flush()
                        os.fsync(stream.fileno())
                    written = os.fstat(descriptor)
                    if (not stat.S_ISREG(written.st_mode)
                            or stat.S_IMODE(written.st_mode) != 0o400
                            or written.st_size != len(body)):
                        raise bench.BenchmarkError(
                            "review_changed", "staged review artifact changed during publication")
                    self.check_parent(path, expected_parent)
                    os.link(temporary, name, src_dir_fd=parent, dst_dir_fd=parent, follow_symlinks=False)
                    published = True
                    linked = os.stat(name, dir_fd=parent, follow_symlinks=False)
                    if (linked.st_dev, linked.st_ino) != owned:
                        raise bench.BenchmarkError(
                            "review_changed", "published review artifact changed during publication")
                    current_staging = os.stat(temporary, dir_fd=parent, follow_symlinks=False)
                    if (current_staging.st_dev, current_staging.st_ino) != owned:
                        raise bench.BenchmarkError(
                            "review_changed", "staged review artifact changed during publication")
                    os.unlink(temporary, dir_fd=parent)
                    staged = False
                    os.fsync(parent)
                    self.check_parent(path, expected_parent)

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
                        observed = self.read(path, len(body))
                    except bench.BenchmarkError as error:
                        raise bench.BenchmarkError(
                            "review_changed", "published review artifact changed during verification") from error
                    current = self.records[path]["identity"]
                    if (bytes(retained) != body or observed != body
                            or (written.st_dev, written.st_ino) != owned
                            or (current[0], current[1]) != owned
                            or identity(written) != current):
                        raise bench.BenchmarkError(
                            "review_changed", "published review artifact changed during verification")
                    self.check_parent(path, expected_parent)
                except (bench.BenchmarkError, OSError, KeyboardInterrupt):
                    if published:
                        try:
                            current = os.stat(name, dir_fd=parent, follow_symlinks=False)
                            if (current.st_dev, current.st_ino) == owned:
                                os.unlink(name, dir_fd=parent)
                                os.fsync(parent)
                        except FileNotFoundError:
                            pass
                    raise
                finally:
                    try:
                        if staged:
                            try:
                                current = os.stat(temporary, dir_fd=parent, follow_symlinks=False)
                                if (current.st_dev, current.st_ino) == owned:
                                    os.unlink(temporary, dir_fd=parent)
                            except FileNotFoundError:
                                pass
                    finally:
                        if descriptor is not None:
                            os.close(descriptor)
            self.check_root()
        except FileExistsError as error:
            raise bench.BenchmarkError("review_exists", "review artifact already exists; replacement is not allowed") from error
        except OSError as error:
            raise bench.BenchmarkError("review_path", "cannot publish artifact without following links") from error
