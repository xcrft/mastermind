"""Subscription-authenticated Codex transport over the frozen research broker."""

from pathlib import Path
import argparse
import os
import sys
import time

from . import bundle
from .codex_metrics import ToolTimeline, output_breakdown
from evals.benchmark import artifacts as artifact_io
from evals.benchmark import protocol as model_protocol
from evals.benchmark import runtime as runtime_identity
from evals.benchmark.tools import SERVER_NAME, tool_definitions
from evals.benchmark.retrieval import ReadLedger
from evals.shared.process import run_bounded


VERSION = "mastermind-codex-adapter-v8"
BUNDLE = {**bundle.COMMON_BUNDLE, "evals/benchmark/adapters/codex.py": "evals/benchmark/adapters/codex.py",
          "evals/benchmark/retrieval.py": "evals/benchmark/retrieval.py",
          "evals/benchmark/adapters/codex_metrics.py": "evals/benchmark/adapters/codex_metrics.py"}
DISABLED_FEATURES = ("apps", "plugins", "memories", "hooks", "multi_agent", "shell_tool",
    "unified_exec", "browser_use", "browser_use_external", "in_app_browser", "computer_use",
    "image_generation", "workspace_dependencies", "skill_search", "view_image",
    "recommended_plugins", "sleep_tool")
DISCOVERY_FIELDS = {"list_mcp_resources": "resources", "list_mcp_resource_templates": "resourceTemplates"}
DISCOVERY_INSTRUCTION = (
    "Use the research server's source tools for evidence. Its MCP resource and template "
    "catalogs are empty; listing them provides no source coverage. Resource reads and "
    "other servers are unavailable."
)


def prepare_runtime(trial, spec, limits):
    if set(spec) != {"kind", "cli", "auth_home", "reasoning_effort"}:
        raise artifact_io.BenchmarkError("adapter_invalid", "Codex needs CLI, auth_home and reasoning_effort pins")
    auth = Path(spec["auth_home"])
    if not auth.is_absolute() or not auth.is_dir() or spec["reasoning_effort"] not in ("low", "medium", "high", "xhigh", "max"):
        raise artifact_io.BenchmarkError("adapter_invalid", "invalid Codex authentication directory or effort")
    if limits["trace_bytes"] < 6 * limits["answer_bytes"] + 16384:
        raise artifact_io.BenchmarkError("codex_trace_limit", "trace cap cannot fit the final envelope")
    return bundle.prepare_runtime(trial, spec, kind="codex_cli", version=VERSION, bundle_sources=BUNDLE,
        settings={"settings": {"auth_home": str(auth.resolve()), "reasoning_effort": spec["reasoning_effort"]}})


def verify_runtime(trial, adapter):
    sources = BUNDLE
    if adapter["version"] in ("mastermind-codex-adapter-v3", "mastermind-codex-adapter-v4", "mastermind-codex-adapter-v5"):
        excluded = {"evals/benchmark/adapters/codex_metrics.py"}
        if adapter["version"] != "mastermind-codex-adapter-v5":
            excluded.add("evals/benchmark/retrieval.py")
        sources = {name: path for name, path in BUNDLE.items() if name not in excluded}
    bundle.verify_runtime(trial, adapter, sources)


def command(runtime, request, trial):
    config = {"model_reasoning_effort": runtime["settings"]["reasoning_effort"], "web_search": "disabled",
        "suppress_unstable_features_warning": True,
        "project_doc_max_bytes": 0,
        "forced_login_method": "chatgpt",
        "mcp_servers": {},
        f"mcp_servers.{SERVER_NAME}.command": runtime["python"]["path"],
        f"mcp_servers.{SERVER_NAME}.args": ["-I", "-S", "-B", runtime["path"], "tools", "--request",
            str(trial / "request.json"), "--sha256", artifact_io.digest(request)],
        f"mcp_servers.{SERVER_NAME}.startup_timeout_sec": 15,
        f"mcp_servers.{SERVER_NAME}.tool_timeout_sec": 30}
    config.update({f"mcp_servers.{SERVER_NAME}.env.{name}": "" for name in runtime_identity.CREDENTIAL_NAMES})
    args = [runtime["cli"]["path"], "exec", "--ignore-user-config", "--ignore-rules", "--ephemeral",
        "--skip-git-repo-check", "--sandbox", "read-only", "--model", request["model"], "--json",
        "--enable", "skip_host_skill_discovery"]
    for feature in DISABLED_FEATURES:
        args.extend(("--disable", feature))
    for key, value in config.items():
        args.extend(("-c", key + "=" + artifact_io.canonical(value).decode()))
    return [*args, "-"]


class StreamObserver:
    def __init__(self, request, emit, clock=time.monotonic):
        self.request, self.emit = request, emit
        self.buffer = bytearray()
        self.answer, self.usage, self.failure = "", None, None
        self.started = self.completed = False
        self.turns = self.calls = 0
        self.stream_error_events = 0
        self.seen_calls = set()
        self.discovery = {}
        self.read_ledger = ReadLedger(request.get("source_files", []))
        self.tool_timeline = ToolTimeline()
        self.output_breakdown = output_breakdown({})
        self.clock = clock
        self.command_started = None
        self.timings = dict(first_message_seconds=None, final_answer_seconds=None)

    def start_timing(self):
        self.command_started = self.clock()

    def record_timing(self, name):
        if self.command_started is not None and self.timings[name] is None:
            self.timings[name] = self.clock() - self.command_started

    def fail(self, state, code):
        self.failure = self.failure or {"state": state, "code": code}
        return self.failure["code"]

    def observe_discovery(self, kind, item):
        args, identifier = item.get("arguments"), item.get("id")
        if (self.request.get("resource_discovery") != "empty_only"
                or not isinstance(args, dict) or set(args) - {"server", "cursor"}
                or args.get("server") not in (None, SERVER_NAME) or args.get("cursor") is not None
                or item.get("server") != (args.get("server") or "codex")
                or not isinstance(identifier, str) or not identifier):
            return self.fail("identity_mismatch", "unavailable_tool_called")
        identity = (item["tool"], args)
        call = self.discovery.setdefault(identifier, {"identity": identity, "completed": False})
        if call["identity"] != identity:
            return self.fail("identity_mismatch", "resource_discovery_changed")
        if kind == "item.completed":
            try:
                result = item["result"]
                if (item.get("status") != "completed" or item.get("error") is not None
                        or not isinstance(result, dict)
                        or set(result) - {"content", "structured_content", "structuredContent", "isError"}
                        or result.get("isError") is not None and result["isError"] is not False):
                    raise ValueError("invalid catalog result")
                content = result.get("content")
                if (not isinstance(content, list) or len(content) != 1
                        or set(content[0]) != {"type", "text"} or content[0]["type"] != "text"):
                    raise ValueError("catalog must contain one JSON text block")
                body = artifact_io.parse_json(content[0]["text"].encode())
                expected = {DISCOVERY_FIELDS[item["tool"]]: []}
                if args.get("server") is not None:
                    expected["server"] = SERVER_NAME
                if body != expected or any(result.get(key) not in (None, expected)
                        for key in ("structured_content", "structuredContent")):
                    raise ValueError("catalog is not the declared empty result")
            except (ValueError, TypeError, KeyError, AttributeError):
                return self.fail("identity_mismatch", "resource_discovery_not_empty")
            call["completed"] = True

    def event(self, event):
        kind = event.get("type")
        if kind == "thread.started":
            if self.started:
                return self.fail("protocol_error", "duplicate_codex_thread")
            self.started = True
            self.emit({"type": "init", "model": self.request["model"], "adapter_version": VERSION})
        elif kind == "turn.started":
            self.turns += 1
        elif kind in ("item.started", "item.updated", "item.completed"):
            item = event.get("item", {})
            item_type = item.get("type")
            if item_type == "mcp_tool_call":
                names = {tool["name"] for tool in tool_definitions(self.request["mmcg"] is not None)}
                discovery = item.get("server") in ("codex", SERVER_NAME) and item.get("tool") in DISCOVERY_FIELDS
                if discovery:
                    failure = self.observe_discovery(kind, item)
                    if failure:
                        return failure
                elif item.get("server") != SERVER_NAME or item.get("tool") not in names:
                    return self.fail("identity_mismatch", "unavailable_tool_called")
                if item.get("id") not in self.seen_calls:
                    self.seen_calls.add(item.get("id"))
                    self.calls += 1
                    name = item["tool"]
                    self.emit({"type": "trace", "tool": "resource_discovery" if discovery else
                        "mmcg" if name.startswith("mmcg_") else name})
                self.tool_timeline.observe(kind, item,
                    self.clock() - self.command_started if self.command_started is not None else None)
                if kind != "item.completed":
                    self.read_ledger.started(item)
                else:
                    self.read_ledger.observe(item)
            elif item_type == "agent_message" and kind == "item.completed":
                self.answer = item.get("text", "")
                if isinstance(self.answer, str) and self.answer.strip():
                    self.record_timing("first_message_seconds")
            elif item_type == "error":
                return self.fail("protocol_error", "codex_context_error")
            elif item_type not in ("reasoning", "agent_message", "todo_list"):
                return self.fail("identity_mismatch", "unexpected_codex_item")
        elif kind == "turn.completed":
            if self.completed:
                return self.fail("protocol_error", "duplicate_codex_result")
            self.completed = True
            if any(not call["completed"] for call in self.discovery.values()):
                self.fail("protocol_error", "incomplete_resource_discovery")
            if isinstance(self.answer, str) and self.answer.strip() and self.failure is None:
                self.record_timing("final_answer_seconds")
            raw = event.get("usage", {})
            self.output_breakdown = output_breakdown(raw)
            total, cached, written = (raw.get(name) for name in ("input_tokens", "cached_input_tokens", "cache_write_input_tokens"))
            if all(type(value) is int and value >= 0 for value in (total, cached, written)) and total >= cached + written:
                self.usage = {"input_tokens": total - cached - written, "cache_read_tokens": cached,
                    "cache_write_tokens": written, "output_tokens": raw.get("output_tokens")}
            else:
                return self.fail("protocol_error", "invalid_codex_usage")
        elif kind == "error":
            if self.completed:
                return self.fail("protocol_error", "codex_error_after_result")
            if not isinstance(event.get("message"), str) or not event["message"].strip():
                return self.fail("protocol_error", "invalid_codex_error")
            # CLI reconnect notices are not terminal turn failures. The raw
            # stream retains their messages; completion still requires usage.
            self.stream_error_events += 1
        elif kind == "turn.failed":
            return self.fail("model_error", "codex_turn_failed")
        else:
            return self.fail("protocol_error", "unexpected_codex_event")

    def feed(self, chunk):
        self.buffer.extend(chunk)
        while b"\n" in self.buffer:
            line, _, rest = self.buffer.partition(b"\n")
            self.buffer = bytearray(rest)
            try:
                stop = self.event(artifact_io.parse_json(line))
            except (ValueError, TypeError, KeyError, AttributeError):
                stop = self.fail("protocol_error", "invalid_codex_event")
            if stop:
                return stop


def execute(request, runtime, trial):
    observer = StreamObserver(request, lambda event: print(artifact_io.canonical(event).decode(), flush=True))
    timeout = request["limits"]["timeout_seconds"]
    deadline = time.monotonic() + timeout - 0.5 if timeout is not None else None
    try:
        if os.getsid(0) != os.getpid():
            raise artifact_io.BenchmarkError("outer_supervisor_required", "Codex must run under the trial supervisor")
        if artifact_io.load_json(trial / "request.json") != request:
            raise artifact_io.BenchmarkError("request_changed", "request differs from frozen input")
        verify_runtime(trial, dict(runtime, runtime_sha256=artifact_io.digest(runtime)))
        env = runtime_identity.clean_environment(trial)
        env.update(CODEX_HOME=runtime["settings"]["auth_home"], RUST_LOG="error")
        version = run_bounded([runtime["cli"]["path"], "--version"], cwd=trial, env=env,
            timeout=5 if deadline is None else min(5, max(0, deadline - time.monotonic())), start_new_session=False)
        if version.stop_reason or version.returncode != 0 or version.stdout.decode().strip() != "codex-cli " + runtime["cli"]["version"]:
            raise artifact_io.BenchmarkError("codex_version_mismatch", "Codex CLI pin does not match")
        prompt = "\n\n".join((request["system_instruction"], DISCOVERY_INSTRUCTION,
            request["portable_instruction"], artifact_io.canonical(request["task"]).decode()))
        with (trial / "codex-stream.jsonl").open("xb") as stream:
            def receive(chunk):
                stream.write(chunk)
                stream.flush()
                return observer.feed(chunk)

            def diagnostics(chunk):
                sys.stderr.buffer.write(chunk)
                sys.stderr.buffer.flush()

            observer.start_timing()
            result = run_bounded(command(runtime, request, trial), cwd=Path(request["source_root"]), env=env, stdin=prompt.encode(),
                timeout=None if deadline is None else max(0, deadline - time.monotonic()), stdout_limit=request["limits"]["trace_bytes"],
                stderr_limit=request["limits"]["stderr_bytes"], start_new_session=False, on_stdout=receive,
                on_stderr=diagnostics)
        if result.stop_reason:
            observer.fail(result.stop_reason if result.stop_reason in ("timeout", "output_limit") else "invocation_error", "codex_process_stopped")
        elif result.returncode != 0:
            observer.fail("invocation_error", "codex_nonzero_exit")
        if observer.buffer:
            observer.fail("protocol_error", "incomplete_codex_stream")
        if not observer.completed:
            observer.fail("model_error" if observer.stream_error_events else "protocol_error",
                "codex_stream_ended_after_error" if observer.stream_error_events else "incomplete_codex_stream")
    except (artifact_io.BenchmarkError, OSError, ValueError, TypeError, KeyError) as error:
        observer.fail("setup_error", getattr(error, "code", "invalid_codex_setup"))
    if not observer.started:
        observer.emit({"type": "init", "model": None, "adapter_version": VERSION})
    measured = model_protocol.telemetry({"usage": observer.usage, "turns": observer.turns}, request["limits"])
    if not measured["complete"]:
        observer.fail("protocol_error", "incomplete_codex_telemetry")
    if measured["budget_exceeded"]:
        observer.fail("budget_exceeded", "reported_model_budget_exceeded")
    final = {"type": "result", "answer": observer.answer, "usage": observer.usage, "turns": observer.turns,
        "timings": observer.timings,
        "cost_usd": None, "model_error": bool(observer.failure and observer.failure["state"] == "model_error"),
        "diagnostics": {"model_identity": "requested_not_observed", "authentication": "existing_codex_subscription",
            "mcp_calls": observer.calls, "raw_stream": "codex-stream.jsonl", "billing_cost": "not_reported",
            "stream_error_events": observer.stream_error_events,
            "resource_discovery": {"contract": request.get("resource_discovery"),
                "calls": len(observer.discovery),
                "completed_empty": sum(call["completed"] for call in observer.discovery.values())},
            "raw_stream_sha256": artifact_io.hash_file(trial / "codex-stream.jsonl",
                request["limits"]["trace_bytes"])["sha256"] if (trial / "codex-stream.jsonl").exists() else None,
            "read_ledger": observer.read_ledger.report(),
            "output_breakdown": observer.output_breakdown,
            "tool_timeline": observer.tool_timeline.report(
                observer.clock() - observer.command_started if observer.command_started is not None else None),
            "model_budget_enforcement": "disabled" if all(request["limits"][name] is None
                for name in ("max_turns", "max_output_tokens")) else "reported_after_turn",
            "host_isolation": "unverified"}}
    if observer.failure:
        final["failure"] = observer.failure
    observer.emit(final)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", type=Path, required=True)
    args = parser.parse_args()
    request = artifact_io.parse_json(sys.stdin.buffer.read(artifact_io.CONTROL_BYTE_LIMIT + 1))
    execute(request, artifact_io.load_json(args.runtime), args.runtime.parent)


if __name__ == "__main__":
    main()
