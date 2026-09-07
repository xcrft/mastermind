"""Bounded, synchronous stdio client for the benchmark's pinned mmcg server."""

from __future__ import annotations

import os
import selectors
import subprocess
import time
from pathlib import Path

if __package__:
    from .benchmark import BenchmarkError, canonical, parse_json
else:
    from benchmark import BenchmarkError, canonical, parse_json


class McpClient:
    """One sequential connection, with no sampling, roots or elicitation access.

    The server inherits the outer trial's process group. The caller must be
    supervised by benchmark.run_trial or an equivalent group-owning harness.
    """

    def __init__(self, command: list[str], *, cwd: Path, env: dict[str, str],
                 timeout: float = 10, output_limit: int = 128 * 1024):
        self.timeout, self.output_limit = timeout, output_limit
        self.next_id = 0
        self.pending = bytearray()
        self.selector = selectors.DefaultSelector()
        self.process = None
        try:
            self.process = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.PIPE,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, close_fds=True)
            for stream, label in ((self.process.stdout, "stdout"), (self.process.stderr, "stderr")):
                os.set_blocking(stream.fileno(), False)
                self.selector.register(stream, selectors.EVENT_READ, label)
            os.set_blocking(self.process.stdin.fileno(), False)
            response = self.request("initialize", {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": "mastermind-benchmark", "version": "1"},
            })
            if response.get("protocolVersion") not in {"2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"}:
                raise BenchmarkError("mcp_protocol", "unsupported MCP protocol version")
            self._write({"jsonrpc": "2.0", "method": "notifications/initialized"},
                        time.monotonic() + timeout)
        except (OSError, ValueError) as error:
            self.close()
            raise BenchmarkError("mcp_unavailable", "cannot start the pinned MCP server") from error
        except BaseException:
            self.close()
            raise

    def _write(self, value: dict, deadline: float) -> None:
        body = canonical(value) + b"\n"
        if len(body) > 16 * 1024:
            raise BenchmarkError("mcp_request_limit", "MCP request exceeds its byte cap")
        sent = 0
        while sent < len(body):
            if time.monotonic() >= deadline:
                raise BenchmarkError("mcp_timeout", "MCP server did not consume the request")
            try:
                sent += os.write(self.process.stdin.fileno(), body[sent:])
            except BlockingIOError:
                time.sleep(0.005)
            except (BrokenPipeError, OSError) as error:
                raise BenchmarkError("mcp_unavailable", "MCP server closed its input") from error

    def request(self, method: str, params: dict) -> dict:
        self.next_id += 1
        expected_id = self.next_id
        deadline = time.monotonic() + self.timeout
        self._write({"jsonrpc": "2.0", "id": expected_id, "method": method, "params": params}, deadline)
        received = len(self.pending)
        stderr_bytes = 0
        while True:
            while b"\n" in self.pending:
                line, _, rest = self.pending.partition(b"\n")
                self.pending = bytearray(rest)
                try:
                    event = parse_json(bytes(line).decode("utf-8").encode("utf-8"))
                except (ValueError, UnicodeError) as error:
                    raise BenchmarkError("mcp_protocol", "MCP server emitted invalid JSON") from error
                if event.get("jsonrpc") != "2.0":
                    raise BenchmarkError("mcp_protocol", "invalid MCP envelope")
                if "method" in event:
                    if "id" in event:
                        # A research query cannot grant sampling, roots or
                        # other server-to-client capabilities.
                        self._write({"jsonrpc": "2.0", "id": event["id"],
                                     "error": {"code": -32601, "message": "Client method unavailable"}}, deadline)
                    continue
                if type(event.get("id")) is not int or event["id"] != expected_id:
                    raise BenchmarkError("mcp_protocol", "unexpected MCP response ID")
                if "error" in event:
                    raise BenchmarkError("mcp_remote_error", "MCP query returned a protocol error")
                if not isinstance(event.get("result"), dict):
                    raise BenchmarkError("mcp_protocol", "MCP response lacks a result object")
                return event["result"]
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise BenchmarkError("mcp_timeout", "MCP query exceeded its deadline")
            if not self.selector.get_map():
                raise BenchmarkError("mcp_unavailable", "MCP server exited before responding")
            for key, _ in self.selector.select(min(remaining, 0.05)):
                try:
                    chunk = os.read(key.fileobj.fileno(), 65536)
                except BlockingIOError:
                    continue
                if not chunk:
                    self.selector.unregister(key.fileobj)
                    continue
                if key.data == "stderr":
                    stderr_bytes += len(chunk)
                    if stderr_bytes > 32 * 1024:
                        raise BenchmarkError("mcp_output_limit", "MCP diagnostics exceeded their byte cap")
                else:
                    received += len(chunk)
                    if received > self.output_limit:
                        raise BenchmarkError("mcp_output_limit", "MCP query output exceeded its byte cap")
                    self.pending.extend(chunk)

    def call(self, name: str, arguments: dict) -> dict:
        return self.request("tools/call", {"name": name, "arguments": arguments})

    def close(self) -> None:
        self.selector.close()
        if self.process is not None:
            self.process.kill()
            self.process.wait()
            for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
                stream.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
