#!/usr/bin/env python3
"""Local, explicit document relationships with content-based freshness checks.

Relation labels are supplied by the caller. Neither a label nor an unchanged
file establishes that a claim is true. This module never upgrades verification.
"""

import argparse
from contextlib import contextmanager
import errno
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import selectors
import stat
import subprocess
import sys
import time
import uuid


MANIFEST_LIMIT = 1024 * 1024
GRAPH_LIMIT = 4 * 1024 * 1024
RELATION_LIMIT = 256
FILE_LIMIT = 128
FILE_BYTE_LIMIT = 1024 * 1024
TOTAL_BYTE_LIMIT = 16 * 1024 * 1024
GIT_OUTPUT_LIMIT = 1024 * 1024
GIT_TIMEOUT = 5
RELATIONS = {
    "supersedes", "documents", "constrains", "supports", "contradicts", "verified_by", "mentions"
}
MARKDOWN = {".md", ".markdown"}
TEXT_SUFFIXES = MARKDOWN | {
    ".py", ".pyi", ".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".vue",
    ".go", ".java", ".cs", ".c", ".h", ".cc", ".cpp", ".cxx", ".hpp", ".hh",
    ".hxx", ".php", ".sh", ".bash", ".zsh", ".fish", ".swift", ".kt", ".kts",
    ".rb", ".ex", ".exs", ".scala", ".sql", ".yaml", ".yml", ".toml", ".json",
    ".jsonl", ".log", ".txt", ".xml", ".html", ".css", ".scss", ".ini", ".cfg",
    ".proto", ".graphql", ".dockerfile",
}
SNAPSHOT_KIND = "mastermind_document_evidence_graph"
CHECK_KIND = "mastermind_document_evidence_check"
HEX = re.compile(r"[0-9a-f]{64}\Z")


class GraphError(Exception):
    def __init__(self, code, path=None):
        super().__init__(code)
        self.code = code
        self.path = path


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False,
                      allow_nan=False).encode("utf-8")


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def exact_fields(value, fields):
    if not isinstance(value, dict) or set(value) != set(fields):
        raise GraphError("invalid_schema")


def integer(value, minimum, maximum):
    if type(value) is not int or not minimum <= value <= maximum:
        raise GraphError("invalid_integer")


def relative_path(value):
    if not isinstance(value, str) or not value or "\\" in value:
        raise GraphError("unsafe_path")
    try:
        encoded = value.encode("utf-8")
    except UnicodeError as error:
        raise GraphError("unsafe_path") from error
    path = PurePosixPath(value)
    if (len(encoded) > 4096 or path.is_absolute() or path.as_posix() != value
            or value == "." or any(part in {".", ".."} for part in path.parts)
            or any(ord(character) < 32 or ord(character) == 127 for character in value)):
        raise GraphError("unsafe_path", value)
    for index, part in enumerate(path.parts):
        if not part.startswith("."):
            continue
        allowed = index == 0 and len(path.parts) >= 3 and (
            (part == ".mastermind" and path.parts[1] in {"tasks", "releases", "decisions", "research"})
            or (part == ".github" and path.parts[1] in {"workflows", "actions"})
        )
        if not allowed:
            raise GraphError("unsafe_path", value)
    return value


def endpoint(value, markdown=False):
    exact_fields(value, {"path", "line"})
    path = relative_path(value["path"])
    integer(value["line"], 1, FILE_BYTE_LIMIT + 1)
    suffix = PurePosixPath(path).suffix.lower()
    if markdown:
        accepted = suffix in MARKDOWN
    else:
        accepted = suffix in TEXT_SUFFIXES or PurePosixPath(path).name.lower() in {
            "dockerfile", "makefile", "justfile"
        }
    if not accepted:
        raise GraphError("unsupported_endpoint", path)
    return {"path": path, "line": value["line"]}


def relation(value):
    exact_fields(value, {"from", "relation", "to"})
    if not isinstance(value["relation"], str) or value["relation"] not in RELATIONS:
        raise GraphError("invalid_relation")
    return {"from": endpoint(value["from"], markdown=True), "relation": value["relation"],
            "to": endpoint(value["to"])}


def validate_manifest(value):
    exact_fields(value, {"schema_version", "relations"})
    integer(value["schema_version"], 1, 1)
    if not isinstance(value["relations"], list):
        raise GraphError("invalid_schema")
    integer(len(value["relations"]), 1, RELATION_LIMIT)
    edges = []
    seen = set()
    for item in value["relations"]:
        edge = relation(item)
        identity = digest(edge)
        if identity in seen:
            raise GraphError("duplicate_relation")
        seen.add(identity)
        edges.append({"id": identity, **edge, "verification": "unverified"})
    paths = {edge[side]["path"] for edge in edges for side in ("from", "to")}
    if len(paths) > FILE_LIMIT:
        raise GraphError("file_count_limit")
    return sorted(edges, key=lambda edge: edge["id"]), sorted(paths)


def decode_json(data):
    def unique_pairs(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise GraphError("duplicate_json_key")
            result[key] = value
        return result

    def reject_constant(_value):
        raise GraphError("invalid_json")

    try:
        return json.loads(data.decode("utf-8"), object_pairs_hook=unique_pairs,
                          parse_constant=reject_constant)
    except (ValueError, UnicodeError, RecursionError) as error:
        raise GraphError("invalid_json") from error


def stat_identity(value):
    return (value.st_dev, value.st_ino, value.st_mode, value.st_size,
            value.st_mtime_ns, value.st_ctime_ns)


def path_error(error, path):
    if error.errno == errno.ENOENT:
        return GraphError("missing", path)
    if error.errno in {errno.ELOOP, errno.ENOTDIR}:
        return GraphError("unsafe_path", path)
    return GraphError("unreadable", path)


class Repository:
    def __init__(self, root):
        self.requested_root = Path(os.path.abspath(root))
        try:
            self.root = self.requested_root.resolve(strict=True)
            if not self.root.is_dir():
                raise GraphError("invalid_root")
            if not hasattr(os, "O_NOFOLLOW") or os.open not in os.supports_dir_fd:
                raise GraphError("unsupported_platform")
            self.fd = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            identity = os.fstat(self.fd)
            self.root_identity = (identity.st_dev, identity.st_ino)
        except OSError as error:
            raise GraphError("invalid_root") from error

    def close(self):
        os.close(self.fd)

    def assert_root_binding(self):
        try:
            current = os.stat(self.root, follow_symlinks=False)
        except OSError as error:
            raise GraphError("root_changed_during_operation") from error
        if not stat.S_ISDIR(current.st_mode) or (current.st_dev, current.st_ino) != self.root_identity:
            raise GraphError("root_changed_during_operation")

    def local_argument(self, argument):
        candidate = Path(argument)
        if not candidate.is_absolute():
            return relative_path(candidate.as_posix())
        for base in (self.root, self.requested_root):
            try:
                return relative_path(candidate.relative_to(base).as_posix())
            except ValueError:
                continue
        raise GraphError("path_outside_root")

    @contextmanager
    def parent(self, path, create=False):
        self.assert_root_binding()
        descriptor = os.dup(self.fd)
        try:
            for component in PurePosixPath(path).parts[:-1]:
                if create:
                    try:
                        os.mkdir(component, mode=0o700, dir_fd=descriptor)
                    except FileExistsError:
                        pass
                next_descriptor = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                          dir_fd=descriptor)
                os.close(descriptor)
                descriptor = next_descriptor
            yield descriptor, PurePosixPath(path).name
        except OSError as error:
            raise path_error(error, path) from error
        finally:
            os.close(descriptor)

    def assert_parent_binding(self, path, descriptor):
        try:
            with self.parent(path) as (current, _name):
                previous_stat, current_stat = os.fstat(descriptor), os.fstat(current)
                if (previous_stat.st_dev, previous_stat.st_ino) != (current_stat.st_dev, current_stat.st_ino):
                    raise GraphError("parent_changed_during_operation", path)
        except GraphError as error:
            raise GraphError("parent_changed_during_operation", path) from error

    def read(self, path, limit):
        with self.parent(path) as (directory, name):
            try:
                descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                                     dir_fd=directory)
                try:
                    before = os.fstat(descriptor)
                    if not stat.S_ISREG(before.st_mode):
                        raise GraphError("not_regular", path)
                    if before.st_size > limit:
                        raise GraphError("file_byte_limit", path)
                    chunks = []
                    remaining = limit + 1
                    while remaining:
                        chunk = os.read(descriptor, min(65536, remaining))
                        if not chunk:
                            break
                        chunks.append(chunk)
                        remaining -= len(chunk)
                    data = b"".join(chunks)
                    if len(data) > limit:
                        raise GraphError("file_byte_limit", path)
                    after = os.fstat(descriptor)
                    named = os.stat(name, dir_fd=directory, follow_symlinks=False)
                    if stat_identity(before) != stat_identity(after) or stat_identity(after) != stat_identity(named):
                        raise GraphError("file_changed_during_read", path)
                    self.assert_parent_binding(path, directory)
                    self.assert_root_binding()
                    return data, stat_identity(after)
                finally:
                    os.close(descriptor)
            except OSError as error:
                raise path_error(error, path) from error

    def write_new(self, path, data):
        parts = PurePosixPath(path).parts
        if len(parts) < 3 or parts[:2] != (".mastermind", "research"):
            raise GraphError("unsafe_output", path)
        with self.parent(path, create=True) as (directory, name):
            temporary = f".document-graph-{uuid.uuid4().hex}.tmp"
            created = False
            published = False
            owned_identity = None
            try:
                descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                                     0o600, dir_fd=directory)
                created = True
                with os.fdopen(descriptor, "wb") as stream:
                    stream.write(data)
                    stream.flush()
                    os.fsync(stream.fileno())
                    owned = os.fstat(stream.fileno())
                    owned_identity = (owned.st_dev, owned.st_ino)
                try:
                    self.assert_parent_binding(path, directory)
                    # Hard-link publication is atomic and, unlike rename(), cannot
                    # overwrite a file created between the existence check and write.
                    os.link(temporary, name, src_dir_fd=directory, dst_dir_fd=directory,
                            follow_symlinks=False)
                    published = True
                    linked = os.stat(name, dir_fd=directory, follow_symlinks=False)
                    if (linked.st_dev, linked.st_ino) != owned_identity:
                        raise GraphError("output_changed_during_operation", path)
                except FileExistsError as error:
                    raise GraphError("output_exists", path) from error
                os.fsync(directory)
                self.assert_parent_binding(path, directory)
            except (GraphError, OSError):
                if published:
                    try:
                        owned = os.stat(name, dir_fd=directory, follow_symlinks=False)
                        if (owned.st_dev, owned.st_ino) == owned_identity:
                            os.unlink(name, dir_fd=directory)
                    except FileNotFoundError:
                        pass
                raise
            finally:
                if created:
                    os.unlink(temporary, dir_fd=directory)

    def git(self, arguments):
        self.assert_root_binding()
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update({"GIT_OPTIONAL_LOCKS": "0", "GIT_CONFIG_NOSYSTEM": "1",
                            "GIT_CONFIG_GLOBAL": os.devnull, "GIT_TERMINAL_PROMPT": "0",
                            "GIT_NO_LAZY_FETCH": "1", "GIT_ALLOW_PROTOCOL": ""})
        command = ["git", "--no-optional-locks", "-c", "core.fsmonitor=false",
                   "-c", "core.untrackedCache=false", "-c", f"core.hooksPath={os.devnull}",
                   "-c", "protocol.allow=never", *arguments]
        try:
            process = subprocess.Popen(command, cwd=self.root, env=environment,
                                       stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        except OSError as error:
            raise GraphError("git_unavailable") from error
        output = bytearray()
        deadline = time.monotonic() + GIT_TIMEOUT
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                while selector.get_map():
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise GraphError("git_timeout")
                    for key, _ in selector.select(remaining):
                        chunk = os.read(key.fileobj.fileno(), 65536)
                        if not chunk:
                            selector.unregister(key.fileobj)
                        else:
                            output.extend(chunk)
                            if len(output) > GIT_OUTPUT_LIMIT:
                                raise GraphError("git_output_limit")
            if process.wait(timeout=max(0.001, deadline - time.monotonic())) != 0:
                raise GraphError("git_failed")
        except subprocess.TimeoutExpired as error:
            raise GraphError("git_timeout") from error
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            process.stdout.close()
        self.assert_root_binding()
        return bytes(output)

    def revision(self):
        try:
            top = self.git(["rev-parse", "--show-toplevel"]).decode("utf-8").rstrip("\n")
            if Path(top).resolve() != self.root:
                raise GraphError("not_repository_root")
            head = self.git(["rev-parse", "--verify", "HEAD"]).decode("ascii").strip()
        except UnicodeError as error:
            raise GraphError("invalid_git_output") from error
        if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", head):
            raise GraphError("invalid_git_revision")
        # `git status` may run clean filters while refreshing same-size files.
        # Raw plumbing avoids that refresh. Dirty is deliberately conservative:
        # a metadata-only touch can count as dirty without hashing unlisted files.
        diff_options = ["--raw", "-z", "--no-ext-diff", "--no-textconv", "--no-renames", "--ignore-submodules=all"]
        dirty = any((
            bool(self.git(["diff-files", *diff_options])),
            bool(self.git(["diff-index", "--cached", *diff_options, "HEAD"])),
            bool(self.git(["ls-files", "--others", "--exclude-standard", "-z"])),
        ))
        return {"head": head, "dirty": dirty}


def inspect_files(repository, paths, checking=False):
    records = {}
    states = {}
    total = 0
    for path in paths:
        try:
            data, identity = repository.read(path, FILE_BYTE_LIMIT)
            total += len(data)
            if total > TOTAL_BYTE_LIMIT:
                raise GraphError("total_byte_limit")
            try:
                text = data.decode("utf-8")
                if "\x00" in text:
                    raise ValueError("binary")
            except (UnicodeError, ValueError) as error:
                raise GraphError("unsupported_text", path) from error
            text = text.replace("\r\n", "\n").replace("\r", "\n")
            lines = text.count("\n") + int(bool(text) and not text.endswith("\n"))
            records[path] = {"path": path, "sha256": hashlib.sha256(data).hexdigest(),
                             "bytes": len(data), "lines": lines}
            states[path] = (identity, records[path]["sha256"])
        except GraphError as error:
            if not checking or error.code not in {"missing", "unsafe_path", "not_regular", "unsupported_text"}:
                raise
            records[path] = {"path": path, "reason": error.code}
            states[path] = error.code
    return records, states


def stable_files(repository, paths, checking=False):
    records, states = inspect_files(repository, paths, checking)
    _second_records, second_states = inspect_files(repository, paths, checking)
    if states != second_states:
        raise GraphError("files_changed_during_snapshot")
    return records


def validate_lines(edges, files):
    for edge in edges:
        for side in ("from", "to"):
            target = edge[side]
            if target["line"] > files[target["path"]]["lines"]:
                raise GraphError("line_out_of_range", target["path"])


def validate_snapshot(value):
    exact_fields(value, {"schema_version", "kind", "root", "revision", "files", "edges", "sha256"})
    integer(value["schema_version"], 1, 1)
    if value["kind"] != SNAPSHOT_KIND or not isinstance(value["root"], str) or not Path(value["root"]).is_absolute():
        raise GraphError("invalid_schema")
    exact_fields(value["revision"], {"head", "dirty"})
    head = value["revision"]["head"]
    if not isinstance(head, str) or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", head) or type(value["revision"]["dirty"]) is not bool:
        raise GraphError("invalid_revision")
    if not isinstance(value["edges"], list):
        raise GraphError("invalid_schema")
    declared = []
    for edge in value["edges"]:
        exact_fields(edge, {"id", "from", "relation", "to", "verification"})
        if edge["verification"] != "unverified":
            raise GraphError("invalid_verification")
        declared.append({key: edge[key] for key in ("from", "relation", "to")})
    edges, paths = validate_manifest({"schema_version": 1, "relations": declared})
    if edges != value["edges"]:
        raise GraphError("invalid_edge_identity_or_order")
    if not isinstance(value["files"], list) or len(value["files"]) != len(paths):
        raise GraphError("invalid_file_inventory")
    files = {}
    total = 0
    for item in value["files"]:
        exact_fields(item, {"path", "sha256", "bytes", "lines"})
        path = relative_path(item["path"])
        if path in files:
            raise GraphError("duplicate_file")
        integer(item["bytes"], 0, FILE_BYTE_LIMIT)
        integer(item["lines"], 0, item["bytes"])
        if not isinstance(item["sha256"], str) or not HEX.fullmatch(item["sha256"]):
            raise GraphError("invalid_file_digest")
        total += item["bytes"]
        files[path] = item
    if total > TOTAL_BYTE_LIMIT:
        raise GraphError("total_byte_limit")
    if list(files) != paths:
        raise GraphError("invalid_file_inventory")
    validate_lines(edges, files)
    if not isinstance(value["sha256"], str) or value["sha256"] != digest({key: item for key, item in value.items() if key != "sha256"}):
        raise GraphError("snapshot_digest_mismatch")
    return edges, paths, files


def snapshot(repository, relations_path, output_path):
    before = repository.revision()
    raw, identity = repository.read(relations_path, MANIFEST_LIMIT)
    edges, paths = validate_manifest(decode_json(raw))
    files = stable_files(repository, paths)
    validate_lines(edges, files)
    if repository.read(relations_path, MANIFEST_LIMIT) != (raw, identity):
        raise GraphError("input_changed_during_snapshot")
    if repository.revision() != before:
        raise GraphError("repository_changed_during_snapshot")
    result = {"schema_version": 1, "kind": SNAPSHOT_KIND, "root": str(repository.root),
              "revision": before, "files": [files[path] for path in paths], "edges": edges}
    result["sha256"] = digest(result)
    encoded = canonical(result) + b"\n"
    if len(encoded) > GRAPH_LIMIT:
        raise GraphError("graph_byte_limit")
    repository.write_new(output_path, encoded)
    return result


def check(repository, graph_path):
    before = repository.revision()
    raw, identity = repository.read(graph_path, GRAPH_LIMIT)
    saved = decode_json(raw)
    edges, paths, expected = validate_snapshot(saved)
    if saved["root"] != str(repository.root):
        raise GraphError("root_binding_mismatch")
    current = stable_files(repository, paths, checking=True)
    changed = {}
    for path in paths:
        item = current[path]
        if "reason" in item:
            changed[path] = {item["reason"]}
        elif item != expected[path]:
            changed[path] = {"content_changed"}
    for edge in edges:
        for side in ("from", "to"):
            target = edge[side]
            item = current[target["path"]]
            if "lines" in item and target["line"] > item["lines"]:
                changed.setdefault(target["path"], set()).add("line_out_of_range")
    if repository.read(graph_path, GRAPH_LIMIT) != (raw, identity):
        raise GraphError("input_changed_during_check")
    if repository.revision() != before:
        raise GraphError("repository_changed_during_check")
    return {
        "schema_version": 1, "kind": CHECK_KIND,
        "status": "needs_review" if changed else "current",
        "root": str(repository.root), "snapshot_sha256": saved["sha256"],
        "revision": before, "snapshot_revision": saved["revision"],
        "revision_changed": before["head"] != saved["revision"]["head"],
        "changed_files": [{"path": path, "reasons": sorted(changed[path])} for path in sorted(changed)],
        "edges": [{**edge, "freshness": "needs_review" if any(edge[side]["path"] in changed for side in ("from", "to")) else "current"}
                  for edge in edges],
    }


class JsonArgumentParser(argparse.ArgumentParser):
    def error(self, _message):
        raise GraphError("invalid_arguments")


def main(argv=None):
    repository = None
    try:
        parser = JsonArgumentParser(description=__doc__)
        commands = parser.add_subparsers(dest="command", required=True, parser_class=JsonArgumentParser)
        make = commands.add_parser("snapshot")
        make.add_argument("--root", required=True)
        make.add_argument("--relations", required=True)
        make.add_argument("--output", required=True)
        inspect = commands.add_parser("check")
        inspect.add_argument("--root", required=True)
        inspect.add_argument("--graph", required=True)
        arguments = parser.parse_args(argv)
        repository = Repository(arguments.root)
        if arguments.command == "snapshot":
            result = snapshot(repository, repository.local_argument(arguments.relations),
                              repository.local_argument(arguments.output))
        else:
            result = check(repository, repository.local_argument(arguments.graph))
        print(canonical(result).decode("utf-8"))
        return 1 if result.get("status") == "needs_review" else 0
    except GraphError as error:
        detail = {"code": error.code}
        if error.path is not None:
            detail["path"] = error.path
        print(json.dumps({"schema_version": 1, "status": "error", "error": detail}))
        return 2
    except (OSError, ValueError, RecursionError):
        print(json.dumps({"schema_version": 1, "status": "error", "error": {"code": "operation_failed"}}))
        return 2
    finally:
        if repository is not None:
            repository.close()


if __name__ == "__main__":
    sys.exit(main())
