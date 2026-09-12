#!/usr/bin/env python3
"""Read-only MCP tools over a frozen benchmark source projection."""

from __future__ import annotations

import argparse
import hashlib
import os
import stat
import sys
from pathlib import Path, PurePosixPath

if not __package__:
    # -I -S disables user/site imports. This directory contains only the
    # implementation files copied and hash-checked by the trial controller.
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import benchmark as bench
    from benchmark_mcp import McpClient
else:
    from . import benchmark as bench
    from .benchmark_mcp import McpClient


SERVER_NAME = "research"
RESPONSE_LIMIT = 64 * 1024
MESSAGE_LIMIT = 64 * 1024
CALL_LIMIT = 128
LANGUAGES = ("python", "typescript", "tsx", "javascript", "vue", "rust", "csharp", "go", "java", "php", "cpp")
EDGE_KINDS = ("calls", "imports", "inherits", "references")
GRAPH_TOOLS = ("mmcg_concept", "mmcg_search", "mmcg_outline", "mmcg_files", "mmcg_callers", "mmcg_callees")
GRAPH_TOP_LIMITS = {
    "mmcg_concept": 50,
    "mmcg_search": 200,
    "mmcg_files": 500,
    "mmcg_callers": 500,
    "mmcg_callees": 500,
}


def _text(value, name, maximum=1024):
    if (not isinstance(value, str) or not value or len(value.encode()) > maximum
            or any(ord(char) < 32 for char in value)):
        raise bench.BenchmarkError("invalid_arguments", f"{name} must be bounded nonempty text")
    return value


def _integer(value, name, minimum, maximum):
    if type(value) is not int or not minimum <= value <= maximum:
        raise bench.BenchmarkError("invalid_arguments", f"{name} is outside the allowed integer range")
    return value


def _fields(args, required, optional=()):
    if not isinstance(args, dict) or set(args) - set(required) - set(optional) or set(required) - set(args):
        raise bench.BenchmarkError("invalid_arguments", "missing or unsupported tool arguments")


def _object(properties, required):
    return {"type": "object", "properties": properties, "required": required, "additionalProperties": False}


def tool_definitions(graph: bool) -> list[dict]:
    text = {"type": "string", "minLength": 1, "maxLength": 1024}
    line = {"type": "integer", "minimum": 1, "maximum": 4294967295}
    language = {"type": "string", "enum": list(LANGUAGES)}
    edge = {"type": "string", "enum": list(EDGE_KINDS)}
    definitions = [
        ("source_read", "Read a bounded line range from one allowed source file.",
         _object({"path": text, "start_line": line, "end_line": line}, ["path"])),
        ("source_search", "Literal text search over allowed source files; no regular expressions.",
         _object({"query": {**text, "maxLength": 256}, "paths": {"type": "array", "items": text, "maxItems": 128},
                  "case_sensitive": {"type": "boolean"}, "max_results": {"type": "integer", "minimum": 1, "maximum": 100}}, ["query"])),
        ("source_git", "Inspect the frozen synthetic Git projection: files, one-commit log, or show a source range. Original history is unavailable.",
         _object({"operation": {"enum": ["files", "log", "show"], "type": "string"},
                  "path": text, "start_line": line, "end_line": line}, ["operation"])),
    ]
    if graph:
        definitions += [
            ("mmcg_concept", "Search indexed name/path/signature/documentation tokens. Preserves evidence limitations; not embedding search.",
             _object({"query": {**text, "maxLength": 256}, "top": {"type": "integer", "minimum": 1, "maximum": 50}}, ["query"])),
            ("mmcg_search", "Find exact case-sensitive symbol names, preserving collisions and signatures.",
             _object({"name": text, "kind": text, "language": language, "collapse_partials": {"type": "boolean"},
                      "top": {"type": "integer", "minimum": 1, "maximum": 200}}, ["name"])),
            ("mmcg_outline", "List symbol ranges for one allowed indexed file; read bodies with source_read.",
             _object({"file": text}, ["file"])),
            ("mmcg_files", "List indexed files. Prefix is a relative literal prefix; wildcard input is not accepted.",
             _object({"prefix": text, "language": language,
                      "top": {"type": "integer", "minimum": 1, "maximum": 500}}, [])),
            ("mmcg_callers", "Find incoming extracted edges; preserve name collisions and precision notes. References are syntactic, not all textual mentions.",
             _object({"name": text, "language": language, "edge_kind": edge,
                      "top": {"type": "integer", "minimum": 1, "maximum": 500}}, ["name"])),
            ("mmcg_callees", "Find outgoing extracted edges for a selected symbol. A returned callee line is its call site, not the target definition.",
             _object({"name": text, "file": text, "line": line, "language": language, "edge_kind": edge,
                      "top": {"type": "integer", "minimum": 1, "maximum": 500}}, ["name"])),
        ]
    return [{"name": name, "description": description, "inputSchema": schema,
             "annotations": {"readOnlyHint": True, "destructiveHint": False,
                             "idempotentHint": True, "openWorldHint": False}}
            for name, description, schema in definitions]


def read_source_snapshot(source: Path, files: list[dict]) -> dict[str, bytes]:
    """Open every component relative to one directory capability, without links."""
    if source != source.resolve(strict=True):
        raise bench.BenchmarkError("source_changed", "source root must be canonical")
    root_fd = os.open(source, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    bodies, total = {}, 0
    try:
        root_identity = os.fstat(root_fd)
        for item in files:
            path = bench.safe_source_path(item["path"])
            parents = [os.dup(root_fd)]
            try:
                parts = PurePosixPath(path).parts
                for part in parts[:-1]:
                    parents.append(os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                           dir_fd=parents[-1]))
                fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parents[-1])
                with os.fdopen(fd, "rb") as handle:
                    before = os.fstat(handle.fileno())
                    if not stat.S_ISREG(before.st_mode) or before.st_size > bench.FILE_BYTE_LIMIT:
                        raise bench.BenchmarkError("source_changed", "source is not a bounded regular file")
                    body = handle.read(bench.FILE_BYTE_LIMIT + 1)
                    after = os.fstat(handle.fileno())
                current = os.stat(parts[-1], dir_fd=parents[-1], follow_symlinks=False)
                identity = lambda info: (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_mode)
                mode = "100755" if current.st_mode & stat.S_IXUSR else "100644"
                if (identity(before) != identity(after) or identity(after) != identity(current)
                        or len(body) != item["bytes"] or hashlib.sha256(body).hexdigest() != item["sha256"]
                        or mode != item["git_mode"]):
                    raise bench.BenchmarkError("source_changed", "source no longer matches the trial")
                total += len(body)
                if total > bench.SOURCE_BYTE_LIMIT:
                    raise bench.BenchmarkError("source_limit", "source snapshot exceeds its total byte cap")
                bodies[path] = body
            finally:
                for fd in reversed(parents):
                    os.close(fd)
        current_root = source.stat(follow_symlinks=False)
        if (root_identity.st_dev, root_identity.st_ino) != (current_root.st_dev, current_root.st_ino):
            raise bench.BenchmarkError("source_changed", "source directory changed while reading")
        return bodies
    except OSError as error:
        raise bench.BenchmarkError("source_changed", "cannot safely read the source snapshot") from error
    finally:
        os.close(root_fd)


class SourceBroker:
    def __init__(self, request: dict, *, native_timeout=10):
        self.request = request
        bench.validate_task(request["task"])
        bench.exact_revision(request["projection_revision"])
        self.source = Path(request["source_root"])
        files = request["source_files"]
        if (not isinstance(files, list) or len(files) != len(request["task"]["source_allowlist"])
                or {item["path"] for item in files} != set(request["task"]["source_allowlist"])):
            raise bench.BenchmarkError("invalid_request", "source inventory differs from the task")
        self.bodies = read_source_snapshot(self.source, files)
        expected_tools = ["source_read", "source_search", "source_git"]
        self.graph = request.get("mmcg") is not None
        if self.graph:
            expected_tools.append("mmcg")
        if request["available_tools"] != expected_tools:
            raise bench.BenchmarkError("invalid_request", "unexpected exposed tool contract")
        self.native_timeout = native_timeout
        self.native = None
        self.calls = 0

    def path(self, value):
        path = bench.safe_source_path(_text(value, "path"))
        if path not in self.bodies:
            raise bench.BenchmarkError("path_denied", "path is outside the source allowlist")
        return path

    def lines(self, path):
        try:
            text = self.bodies[path].decode("utf-8")
            if not text:
                return []
            # Match file/AST line numbers. str.splitlines also splits form-feed,
            # vertical tab and Unicode separators that are source characters.
            lines = text.split("\n")
            lines[:-1] = [line.removesuffix("\r") for line in lines[:-1]]
            if not lines[-1]:
                lines.pop()
            return lines
        except UnicodeError as error:
            raise bench.BenchmarkError("source_encoding", "source is not UTF-8 text") from error

    def read(self, args):
        _fields(args, ("path",), ("start_line", "end_line"))
        path = self.path(args["path"])
        lines = self.lines(path)
        if not lines and set(args) == {"path"}:
            return {"path": path, "start_line": None, "end_line": None, "total_lines": 0, "lines": []}
        start = _integer(args.get("start_line", 1), "start_line", 1, max(1, len(lines)))
        end = _integer(args.get("end_line", min(len(lines), start + 79)), "end_line", start, len(lines))
        if end - start >= 200:
            raise bench.BenchmarkError("range_limit", "read at most 200 source lines per call")
        return {"path": path, "start_line": start, "end_line": end, "total_lines": len(lines),
                "lines": [{"line": index + 1, "text": lines[index]} for index in range(start - 1, end)]}

    def search(self, args):
        _fields(args, ("query",), ("paths", "case_sensitive", "max_results"))
        query = _text(args["query"], "query", 256)
        case_sensitive = args.get("case_sensitive", False)
        if type(case_sensitive) is not bool:
            raise bench.BenchmarkError("invalid_arguments", "case_sensitive must be boolean")
        paths = args.get("paths", sorted(self.bodies))
        if not isinstance(paths, list) or not 1 <= len(paths) <= bench.SOURCE_FILE_LIMIT:
            raise bench.BenchmarkError("invalid_arguments", "paths must be a nonempty bounded list")
        paths = [self.path(path) for path in paths]
        if len(paths) != len(set(paths)):
            raise bench.BenchmarkError("invalid_arguments", "duplicate search paths")
        maximum = _integer(args.get("max_results", 30), "max_results", 1, 100)
        needle = query if case_sensitive else query.casefold()
        matches = []
        for path in sorted(paths):
            for line, body in enumerate(self.lines(path), 1):
                if needle in (body if case_sensitive else body.casefold()):
                    if len(matches) == maximum:
                        return {"matches": matches, "truncated": True}
                    matches.append({"path": path, "line": line, "text": body})
        return {"matches": matches, "truncated": False}

    def git(self, args):
        _fields(args, ("operation",), ("path", "start_line", "end_line"))
        operation = args["operation"]
        if operation == "show":
            return self.read({key: value for key, value in args.items() if key != "operation"})
        if set(args) != {"operation"}:
            raise bench.BenchmarkError("invalid_arguments", "only show accepts a source path/range")
        if operation == "files":
            return {"files": sorted(self.bodies), "view": "frozen_git_projection"}
        if operation == "log":
            return {"commits": [{"revision": self.request["projection_revision"],
                                  "subject": "Allowlisted source projection"}], "original_history_available": False}
        raise bench.BenchmarkError("operation_denied", "Git operation is not available")

    def graph_arguments(self, name, args):
        signatures = {
            "mmcg_concept": (("query",), ("top",)),
            "mmcg_search": (("name",), ("kind", "language", "collapse_partials", "top")),
            "mmcg_outline": (("file",), ()), "mmcg_files": ((), ("prefix", "language", "top")),
            "mmcg_callers": (("name",), ("language", "edge_kind", "top")),
            "mmcg_callees": (("name",), ("file", "line", "language", "edge_kind", "top")),
        }
        _fields(args, *signatures[name])
        for key, value in args.items():
            if key == "file":
                self.path(value)
            elif key == "top":
                _integer(value, key, 1, GRAPH_TOP_LIMITS[name])
            elif key == "line":
                _integer(value, key, 1, 4294967295)
                if "file" not in args:
                    raise bench.BenchmarkError("invalid_arguments", "line requires file")
            elif key == "collapse_partials":
                if type(value) is not bool:
                    raise bench.BenchmarkError("invalid_arguments", "collapse_partials must be boolean")
            elif key in ("language", "edge_kind"):
                if value not in (LANGUAGES if key == "language" else EDGE_KINDS):
                    raise bench.BenchmarkError("invalid_arguments", f"unsupported {key}")
            else:
                _text(value, key, 256 if key == "query" else 1024)
                if key == "prefix" and (value.startswith("/") or ".." in value.split("/")
                                         or any(char in value for char in ("%", "_", "\\"))):
                    raise bench.BenchmarkError("invalid_arguments", "prefix must be relative and contain no wildcards")
        return args

    def query_graph(self, name, args):
        args = self.graph_arguments(name, args)
        if self.native is None:
            graph = self.request["mmcg"]
            index = Path(graph["index"])
            bench.validate_index(index, self.source, self.request["source_files"],
                                 graph["index_contract"], graph["indexed_files"])
            if hashlib.sha256(bench.read_file(index, bench.BINARY_BYTE_LIMIT)).hexdigest() != graph["index_sha256"]:
                raise bench.BenchmarkError("index_changed", "index changed after preparation")
            bench.runtime_pin(graph["runtime"], "mmcg")
            env = bench.clean_environment(self.source.parent)
            env["MMCG_QUERY_BUDGET_MS"] = "2000"
            self.native = McpClient([graph["runtime"]["path"], "--index", str(index), "serve"],
                                    cwd=self.source, env=env, timeout=self.native_timeout)
        try:
            result = self.native.call(name, args)
            if (not isinstance(result.get("content"), list) or type(result.get("isError", False)) is not bool):
                raise bench.BenchmarkError("mcp_protocol", "invalid native tool result")
        except bench.BenchmarkError:
            # A timed-out/oversized reply can still be pending. The next query
            # needs a fresh session, not a response from the previous request.
            self.close()
            raise
        # Preserve the complete MCP result: errors, ambiguity, freshness,
        # precision notes and truncation are part of the evidence.
        return result

    def call(self, name, args):
        self.calls += 1
        if self.calls > CALL_LIMIT:
            raise bench.BenchmarkError("tool_call_limit", "tool server call budget exhausted")
        if name == "source_read":
            body = self.read(args)
        elif name == "source_search":
            body = self.search(args)
        elif name == "source_git":
            body = self.git(args)
        elif self.graph and name in GRAPH_TOOLS:
            return self.query_graph(name, args)
        else:
            raise bench.BenchmarkError("tool_denied", "tool is not available in this condition")
        return {"content": [{"type": "text", "text": bench.canonical(body).decode()}],
                "structuredContent": body, "isError": False}

    def close(self):
        if self.native is not None:
            self.native.close()
            self.native = None


def serve(broker: SourceBroker, input_stream, output_stream) -> None:
    state = "new"
    for _ in range(512):
        line = input_stream.readline(MESSAGE_LIMIT + 1)
        if not line:
            return
        if len(line) > MESSAGE_LIMIT:
            raise bench.BenchmarkError("message_limit", "MCP message exceeds its byte cap")
        identifier = None
        try:
            event = bench.parse_json(line.decode("utf-8").encode("utf-8"))
            if event.get("jsonrpc") != "2.0" or not isinstance(event.get("method"), str):
                raise ValueError("invalid request")
            identifier = event.get("id")
            if identifier is not None and (type(identifier) not in (int, str) or isinstance(identifier, str) and len(identifier) > 128):
                identifier = None
                raise ValueError("invalid id")
            method, params = event["method"], event.get("params", {})
            if method == "initialize" and state == "new" and identifier is not None:
                if not isinstance(params, dict) or not isinstance(params.get("protocolVersion"), str):
                    raise ValueError("invalid initialize")
                protocol = params["protocolVersion"]
                if protocol not in {"2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"}:
                    protocol = "2025-11-25"
                result = {"protocolVersion": protocol, "capabilities": {"tools": {}},
                          "serverInfo": {"name": "mastermind-benchmark-source", "version": "1"}}
                state = "initializing"
            elif method == "notifications/initialized" and state == "initializing" and identifier is None:
                state = "ready"
                continue
            elif identifier is None:
                continue
            elif method == "ping":
                result = {}
            elif state != "ready":
                raise ValueError("MCP initialization required")
            elif method == "tools/list":
                _fields(params, (), ("_meta",))
                if not isinstance(params.get("_meta", {}), dict):
                    raise ValueError("invalid request metadata")
                result = {"tools": tool_definitions(broker.graph)}
            elif method == "tools/call":
                _fields(params, ("name",), ("arguments", "_meta"))
                if not isinstance(params.get("_meta", {}), dict):
                    raise ValueError("invalid request metadata")
                try:
                    result = broker.call(_text(params["name"], "name", 128), params.get("arguments", {}))
                    if len(bench.canonical(result)) > RESPONSE_LIMIT:
                        raise bench.BenchmarkError("tool_output_limit", "tool response is too large; narrow the query")
                except (bench.BenchmarkError, ValueError, TypeError, KeyError, OSError) as error:
                    code = getattr(error, "code", "tool_error")
                    result = {"content": [{"type": "text", "text": code + ": " + str(error)[:256]}], "isError": True}
            else:
                output = {"jsonrpc": "2.0", "id": identifier, "error": {"code": -32601, "message": "Method unavailable"}}
                output_stream.write(bench.canonical(output) + b"\n")
                output_stream.flush()
                continue
            output = {"jsonrpc": "2.0", "id": identifier, "result": result}
        except (ValueError, UnicodeError, TypeError, KeyError, bench.BenchmarkError):
            output = {"jsonrpc": "2.0", "id": identifier, "error": {"code": -32600, "message": "Invalid MCP request"}}
        output_stream.write(bench.canonical(output) + b"\n")
        output_stream.flush()
    raise bench.BenchmarkError("message_limit", "MCP message budget exhausted")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", required=True, type=Path)
    parser.add_argument("--sha256", required=True)
    args = parser.parse_args(argv)
    broker = None
    try:
        request = bench.load_json(args.request)
        if bench.digest(request) != args.sha256:
            raise bench.BenchmarkError("request_changed", "tool request changed after preparation")
        broker = SourceBroker(request)
        serve(broker, sys.stdin.buffer, sys.stdout.buffer)
        return 0
    except (bench.BenchmarkError, OSError, ValueError, KeyError, TypeError) as error:
        print(getattr(error, "code", "tool_server_error"), file=sys.stderr)
        return 2
    finally:
        if broker is not None:
            broker.close()


if __name__ == "__main__":
    raise SystemExit(main())
