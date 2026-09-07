#!/usr/bin/env python3
"""Pinned Claude CLI transport with a constrained read-only MCP broker.

This is a trusted host process, not an OS sandbox. The CLI's observed model and
tool inventory are checked; managed policies and runtime provenance still need
independent attestation before a trial can support a comparison claim.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
import time
from pathlib import Path

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import benchmark as bench
    from benchmark_process import run_bounded
    from benchmark_tools import SERVER_NAME, tool_definitions
else:
    from . import benchmark as bench
    from .benchmark_process import run_bounded
    from .benchmark_tools import SERVER_NAME, tool_definitions


VERSION = "mastermind-claude-adapter-v1"
BUNDLE = ("benchmark.py", "benchmark_process.py", "benchmark_mcp.py",
          "benchmark_tools.py", "claude_adapter.py")


def prepare_runtime(trial: Path, spec: dict, limits: dict) -> dict:
    if set(spec) != {"kind", "cli"}:
        raise bench.BenchmarkError("adapter_invalid", "Claude adapter requires kind and an explicit CLI pin")
    # Reserve space for an escaped final answer and a failure envelope.
    if limits["trace_bytes"] < 6 * limits["answer_bytes"] + 16384:
        raise bench.BenchmarkError("claude_trace_limit", "trace cap must fit six times the answer cap plus 16384 bytes")
    cli = bench.runtime_pin(spec["cli"], "claude_cli")
    python_path = Path(sys.executable).resolve(strict=True)
    python = bench.runtime_pin({"path": str(python_path),
        "sha256": hashlib.sha256(bench.read_file(python_path, bench.BINARY_BYTE_LIMIT)).hexdigest(),
        "version": sys.version.split()[0], "origin": "preparation interpreter; dependencies not attested"}, "python")
    runtime = trial / "runtime"
    runtime.mkdir(mode=0o700)
    bundle = {}
    for name in BUNDLE:
        body = bench.read_file(Path(__file__).resolve().parent / name)
        with (runtime / name).open("xb") as handle:
            handle.write(body)
        (runtime / name).chmod(0o555)
        bundle[name] = hashlib.sha256(body).hexdigest()
    adapter = {"kind": "claude_cli", "path": str(runtime / "claude_adapter.py"),
               "sha256": bundle["claude_adapter.py"], "version": VERSION,
               "origin": "frozen benchmark implementation bytes", "bundle": bundle,
               "cli": cli, "python": python}
    bench.write_new(trial / "adapter-runtime.json", adapter)
    adapter["runtime_sha256"] = bench.digest(adapter)
    (trial / "adapter-runtime.json").chmod(0o444)
    return adapter


def verify_runtime(trial: Path, adapter: dict) -> None:
    expected = {key: value for key, value in adapter.items() if key != "runtime_sha256"}
    if bench.load_json(trial / "adapter-runtime.json") != expected or bench.digest(expected) != adapter["runtime_sha256"]:
        raise bench.BenchmarkError("adapter_runtime_changed", "adapter runtime descriptor changed")
    runtime = trial / "runtime"
    if runtime.is_symlink() or runtime.resolve() != runtime or set(p.name for p in runtime.iterdir()) != set(BUNDLE):
        raise bench.BenchmarkError("adapter_bundle_changed", "adapter bundle inventory changed")
    if adapter["path"] != str(runtime / "claude_adapter.py") or set(adapter["bundle"]) != set(BUNDLE):
        raise bench.BenchmarkError("adapter_bundle_changed", "adapter entry point or bundle changed")
    for name in BUNDLE:
        if hashlib.sha256(bench.read_file(runtime / name)).hexdigest() != adapter["bundle"][name]:
            raise bench.BenchmarkError("adapter_bundle_changed", "adapter implementation bytes changed")
    bench.runtime_pin(adapter["cli"], "claude_cli")
    bench.runtime_pin(adapter["python"], "python")


def exposed_tools(request: dict) -> list[str]:
    return [f"mcp__{SERVER_NAME}__{tool['name']}" for tool in tool_definitions(request["mmcg"] is not None)]


def cli_environment(trial: Path, request: dict, key: str) -> dict[str, str]:
    env = bench.clean_environment(trial, {"ANTHROPIC_API_KEY": key})
    env.update({"ENABLE_TOOL_SEARCH": "false", "MCP_DISCOVERY_CACHE": "0",
                "DISABLE_AUTOUPDATER": "1", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
                "DISABLE_TELEMETRY": "1", "DISABLE_ERROR_REPORTING": "1",
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS": str(request["limits"]["max_output_tokens"])})
    return env


def cli_command(runtime: dict, request: dict, config: Path) -> list[str]:
    system = request["system_instruction"]
    if request["portable_instruction"]:
        system += "\n\n" + request["portable_instruction"]
    return [runtime["cli"]["path"], "-p", "--bare", "--model", request["model"],
            "--system-prompt", system, "--tools", "", "--allowedTools", ",".join(exposed_tools(request)),
            "--strict-mcp-config", "--mcp-config", str(config), "--setting-sources", "",
            "--disable-slash-commands", "--no-chrome", "--permission-mode", "dontAsk",
            "--no-session-persistence", "--output-format", "stream-json", "--verbose",
            "--include-partial-messages", "--max-turns", str(request["limits"]["max_turns"])]


class StreamObserver:
    """Validate live JSONL identity/tool boundaries and cumulative output usage."""

    def __init__(self, request: dict, runtime: dict, client: Path, emit):
        self.request, self.runtime, self.client, self.emit = request, runtime, client, emit
        self.buffer = bytearray()
        self.init = self.result = self.failure = None
        self.tool_ids = set()
        self.output_by_message = {}
        self.current_message = None
        self.models = set()
        self.truncated_response = False
        self.assistant_error = None

    def fail(self, state: str, code: str) -> str:
        self.failure = self.failure or {"state": state, "code": code}
        return self.failure["code"]

    def model(self, value):
        if not isinstance(value, str) or value != self.request["model"]:
            return self.fail("identity_mismatch", "observed_model_mismatch")
        self.models.add(value)

    def tool(self, block):
        name, tool_id = block.get("name"), block.get("id")
        if name not in exposed_tools(self.request):
            return self.fail("identity_mismatch", "unavailable_tool_called")
        if not isinstance(tool_id, str) or not tool_id:
            return self.fail("protocol_error", "invalid_tool_call")
        if tool_id not in self.tool_ids:
            self.tool_ids.add(tool_id)
            suffix = name.removeprefix(f"mcp__{SERVER_NAME}__")
            self.emit({"type": "trace", "tool": "mmcg" if suffix.startswith("mmcg_") else suffix,
                       "name": name, "tool_use_id": tool_id})

    def event(self, event):
        kind = event.get("type")
        if kind == "system" and event.get("subtype") == "init":
            if self.init is not None or self.result is not None:
                return self.fail("protocol_error", "duplicate_cli_init")
            self.init = event
            self.emit({"type": "init", "model": event.get("model"), "adapter_version": VERSION})
            self.model(event.get("model"))
            if event.get("claude_code_version") != self.runtime["cli"]["version"]:
                return self.fail("identity_mismatch", "observed_cli_version_mismatch")
            if event.get("cwd") != str(self.client) or event.get("permissionMode") != "dontAsk":
                return self.fail("identity_mismatch", "observed_cli_environment_mismatch")
            servers = event.get("mcp_servers")
            if (not isinstance(servers, list) or len(servers) != 1 or not isinstance(servers[0], dict)
                    or servers[0].get("name") != SERVER_NAME or servers[0].get("status") != "connected"):
                return self.fail("setup_error", "mcp_server_unavailable")
            tools = event.get("tools")
            if (not isinstance(tools, list) or any(not isinstance(t, str) for t in tools)
                    or sorted(tools) != sorted(exposed_tools(self.request))):
                return self.fail("identity_mismatch", "observed_tool_inventory_mismatch")
            if event.get("skills", []) or event.get("plugins", []):
                return self.fail("identity_mismatch", "unexpected_cli_extensions")
            return
        # Service notifications can precede init or follow result. They cannot
        # introduce model output, tools, or another terminal result.
        if kind in ("system", "rate_limit_event"):
            return
        if self.init is None:
            return self.fail("protocol_error", "cli_event_before_init")
        if self.result is not None:
            return self.fail("protocol_error", "cli_event_after_result")
        if kind == "assistant":
            message = event.get("message", {})
            if event.get("error"):
                self.assistant_error = event["error"]
                # CLI-generated error messages use a synthetic model identity.
                # Keep the terminal result/usage instead of treating it as a
                # provider model switch.
                return
            self.model(message.get("model"))
            if message.get("stop_reason") == "max_tokens":
                self.truncated_response = True
            for block in message.get("content", []):
                if block.get("type") == "tool_use":
                    self.tool(block)
        elif kind == "stream_event":
            partial = event.get("event", {})
            if partial.get("type") == "message_start":
                message = partial.get("message", {})
                self.model(message.get("model"))
                self.current_message = message.get("id")
                if not isinstance(self.current_message, str) or not self.current_message:
                    return self.fail("protocol_error", "invalid_message_identity")
            elif partial.get("type") == "content_block_start":
                block = partial.get("content_block", {})
                if block.get("type") == "tool_use":
                    self.tool(block)
            elif partial.get("type") == "message_delta":
                if partial.get("delta", {}).get("stop_reason") == "max_tokens":
                    self.truncated_response = True
                count = partial.get("usage", {}).get("output_tokens")
                if self.current_message is None or type(count) is not int or count < 0:
                    return self.fail("protocol_error", "invalid_live_usage")
                previous = self.output_by_message.get(self.current_message, 0)
                if count < previous:
                    return self.fail("protocol_error", "decreasing_live_usage")
                self.output_by_message[self.current_message] = count
                if sum(self.output_by_message.values()) > self.request["limits"]["max_output_tokens"]:
                    return self.fail("budget_exceeded", "observed_output_budget_exceeded")
        elif kind == "user":
            for block in event.get("message", {}).get("content", []):
                if block.get("type") != "tool_result" or block.get("tool_use_id") not in self.tool_ids:
                    return self.fail("protocol_error", "unmatched_tool_result")
        elif kind == "result":
            self.result = event
            if type(event.get("is_error")) is not bool:
                return self.fail("protocol_error", "invalid_cli_result")
            if event.get("permission_denials"):
                return self.fail("setup_error", "tool_permission_denied")
            subtype = event.get("subtype")
            if subtype in ("error_max_turns", "error_max_budget_usd"):
                return self.fail("budget_exceeded", subtype)
            if event["is_error"]:
                if self.assistant_error in ("authentication_failed", "billing_error"):
                    return self.fail("setup_error", "cli_" + self.assistant_error)
                return self.fail("model_error" if self.assistant_error else "invocation_error",
                                 "cli_reported_error" if self.assistant_error else "cli_execution_error")
            if self.assistant_error:
                return self.fail("protocol_error", "success_after_cli_error")
            models = event.get("modelUsage")
            if not isinstance(models, dict) or not models:
                return self.fail("protocol_error", "missing_model_usage")
            for model in models:
                self.model(model)
            if subtype != "success" or not isinstance(event.get("result"), str) or not event["result"].strip():
                return self.fail("protocol_error", "missing_final_answer")
            if self.truncated_response or event.get("stop_reason") == "max_tokens":
                return self.fail("budget_exceeded", "response_token_limit")
            if len(event["result"].encode()) > self.request["limits"]["answer_bytes"]:
                return self.fail("output_limit", "answer_limit")
        else:
            return self.fail("protocol_error", "unknown_cli_event")
        return self.failure["code"] if self.failure else None

    def feed(self, body: bytes):
        self.buffer.extend(body)
        while b"\n" in self.buffer:
            line, _, rest = self.buffer.partition(b"\n")
            self.buffer = bytearray(rest)
            if not line.strip():
                continue
            try:
                reason = self.event(bench.parse_json(bytes(line)))
            except (ValueError, TypeError, KeyError, AttributeError):
                reason = self.fail("protocol_error", "malformed_cli_event")
            if self.failure:
                reason = self.failure["code"]
            if reason:
                return reason

    def finish(self):
        if self.buffer.strip() and not self.failure:
            self.feed(b"\n")
        if self.init is None:
            self.fail("protocol_error", "missing_cli_init")
        if self.result is None:
            self.fail("protocol_error", "missing_cli_result")


def normalized_usage(result: dict) -> dict:
    usage = result.get("usage")
    if not isinstance(usage, dict):
        return {}
    return {target: usage.get(source) for source, target in (
        ("input_tokens", "input_tokens"), ("output_tokens", "output_tokens"),
        ("cache_read_input_tokens", "cache_read_tokens"), ("cache_creation_input_tokens", "cache_write_tokens"))}


def execute(request: dict, runtime: dict, trial: Path) -> None:
    limits = request["limits"]
    deadline = time.monotonic() + limits["timeout_seconds"] - 0.5
    used = 0
    emitted_init = False

    def emit(event):
        nonlocal used, emitted_init
        body = bench.canonical(event) + b"\n"
        if event["type"] == "trace" and used + len(body) > limits["trace_bytes"] - 6 * limits["answer_bytes"] - 8192:
            raise bench.BenchmarkError("normalized_trace_limit", "normalized trace exceeds its byte cap")
        sys.stdout.buffer.write(body)
        sys.stdout.buffer.flush()
        used += len(body)
        emitted_init |= event["type"] == "init"

    client = trial / "client"
    observer = StreamObserver(request, runtime, client, emit)
    process = None
    try:
        if os.getsid(0) != os.getpid():
            raise bench.BenchmarkError("outer_supervisor_required", "adapter must run under the trial process supervisor")
        if request["protocol"] != "mastermind-research-adapter-v1" or bench.load_json(trial / "request.json") != request:
            raise bench.BenchmarkError("request_changed", "adapter request does not match the frozen file")
        verify_runtime(trial, dict(runtime, runtime_sha256=bench.digest(runtime)))
        key = os.environ.get("ANTHROPIC_API_KEY")
        if not key:
            raise bench.BenchmarkError("anthropic_api_key_missing", "bare Claude CLI requires an explicit API key")
        env = cli_environment(trial, request, key)
        version = run_bounded([runtime["cli"]["path"], "--version"], cwd=trial, env=bench.clean_environment(trial),
                              timeout=min(5, max(0, deadline - time.monotonic())),
                              stdout_limit=4096, stderr_limit=4096, start_new_session=False)
        if version.stop_reason == "timeout":
            observer.fail("timeout", "cli_version_timeout")
            raise bench.BenchmarkError("cli_version_timeout", "CLI version check exceeded its deadline")
        if (version.stop_reason or version.returncode != 0
                or version.stdout.decode("utf-8").strip() != runtime["cli"]["version"] + " (Claude Code)"):
            raise bench.BenchmarkError("claude_cli_version_mismatch", "CLI does not report the pinned version")
        client.mkdir(mode=0o700)
        config = trial / "mcp-config.json"
        bench.write_new(config, {"mcpServers": {SERVER_NAME: {
            "command": runtime["python"]["path"], "args": ["-I", "-S", "-B", str(trial / "runtime/benchmark_tools.py"),
                "--request", str(trial / "request.json"), "--sha256", bench.digest(request)],
            "env": {name: "" for name in bench.CREDENTIAL_NAMES}}}})
        # Only public task fields enter the model prompt. Runtime descriptors,
        # original repository paths and the hidden rubric are controller data.
        prompt = bench.canonical({key: request["task"][key] for key in (
            "id", "revision", "kind", "source_allowlist", "question", "output_contract")}) + b"\n"
        with (trial / "claude-stream.jsonl").open("xb") as handle:
            def observe(chunk):
                handle.write(chunk)
                handle.flush()
                return observer.feed(chunk)

            def diagnostics(chunk):
                sys.stderr.buffer.write(chunk)
                sys.stderr.buffer.flush()

            process = run_bounded(cli_command(runtime, request, config), cwd=client, env=env, stdin=prompt,
                                  timeout=max(0, deadline - time.monotonic()), stdout_limit=limits["trace_bytes"],
                                  stderr_limit=limits["stderr_bytes"], start_new_session=False,
                                  on_stdout=observe, on_stderr=diagnostics)
        if process.stop_reason and not observer.failure:
            state = process.stop_reason if process.stop_reason in ("timeout", "output_limit") else "invocation_error"
            observer.fail(state, "cli_" + process.stop_reason)
        elif process.returncode != 0 and not observer.failure:
            observer.fail("invocation_error", "cli_nonzero_exit")
        observer.finish()
    except (bench.BenchmarkError, OSError, ValueError, TypeError, KeyError) as error:
        code = getattr(error, "code", "invalid_adapter_setup")
        observer.fail("output_limit" if code == "normalized_trace_limit" else "setup_error", code)
    if not emitted_init:
        emit({"type": "init", "model": None, "adapter_version": VERSION})
    result = observer.result or {}
    usage = normalized_usage(result)
    measured = bench.telemetry({"usage": usage, "turns": result.get("num_turns")}, limits)
    if not observer.failure and not measured["complete"]:
        observer.fail("protocol_error", "incomplete_cli_telemetry")
    if not observer.failure and measured["budget_exceeded"]:
        observer.fail("budget_exceeded", "reported_model_budget_exceeded")
    answer = result.get("result", "")
    if not isinstance(answer, str) or len(answer.encode()) > limits["answer_bytes"]:
        answer = ""
    final = {"type": "result", "answer": answer, "model_error": bool(observer.failure and observer.failure["state"] == "model_error"),
             "usage": usage, "turns": result.get("num_turns"), "cost_usd": result.get("total_cost_usd"),
             "diagnostics": {"cli_version": observer.init.get("claude_code_version") if observer.init else None,
                 "models": sorted(observer.models), "cli_subtype": result.get("subtype"),
                 "live_output_tokens": sum(observer.output_by_message.values()),
                 "max_turns": "cli_limit", "max_output_tokens": "per_response_cap_and_observed_aggregate_stop_may_overshoot",
                 "raw_stream": "claude-stream.jsonl" if (trial / "claude-stream.jsonl").is_file() else None}}
    if observer.failure:
        final["failure"] = observer.failure
    emit(final)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        request_bytes = sys.stdin.buffer.read(bench.CONTROL_BYTE_LIMIT + 1)
        if len(request_bytes) > bench.CONTROL_BYTE_LIMIT:
            raise ValueError("request byte limit")
        request = bench.parse_json(request_bytes)
        runtime = bench.load_json(args.runtime)
        execute(request, runtime, args.runtime.resolve().parent)
        return 0
    except (bench.BenchmarkError, OSError, ValueError, TypeError, KeyError):
        print("invalid_adapter_input", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
