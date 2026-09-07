"""Actual subprocess/MCP tests; fixture CLIs never call a model or the network."""

import ast
import json
import os
import shutil
import subprocess
import sys
import textwrap
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from evals import benchmark as bench
from evals.benchmark_mcp import McpClient
from evals.benchmark_process import run_bounded
from evals.benchmark_tools import SourceBroker
from evals import test_benchmark as fixtures


FAKE_CLI = r'''
import json, os, pathlib, subprocess, sys, time
if sys.argv[1:] == ['--version']:
    if MODE == 'slow_version_hang':
        time.sleep(1.25)
    print(('0.0.0' if MODE == 'wrong_version' else '2.1.236') + ' (Claude Code)')
    sys.exit(0)
args = sys.argv[1:]
def option(name):
    return args[args.index(name) + 1]
def emit(value):
    print(json.dumps(value), flush=True)
cwd = pathlib.Path.cwd()
trial = cwd.parent
(trial / 'cli-called').write_text('yes')
prompt = json.load(sys.stdin)
assert set(prompt) == {'id', 'revision', 'kind', 'source_allowlist', 'question', 'output_contract'}
assert not list(cwd.iterdir())
assert option('--tools') == '' and option('--setting-sources') == ''
assert option('--permission-mode') == 'dontAsk' and option('--output-format') == 'stream-json'
for flag in ('--bare', '--strict-mcp-config', '--disable-slash-commands', '--no-chrome',
             '--no-session-persistence', '--verbose', '--include-partial-messages'):
    assert flag in args
assert os.environ['ANTHROPIC_API_KEY'] == 'fixture-key'
assert not os.environ.get('CLAUDE_CODE_OAUTH_TOKEN')
assert 'PYTHONPATH' not in os.environ and 'SECRET_CANARY' not in os.environ
assert os.environ['ENABLE_TOOL_SEARCH'] == 'false'
assert os.environ['CLAUDE_CODE_MAX_OUTPUT_TOKENS']
config = json.loads(pathlib.Path(option('--mcp-config')).read_text())
assert set(config['mcpServers']) == {'research'}
server = config['mcpServers']['research']
environment = dict(os.environ, **server['env'])
assert all(not environment.get(key) for key in ('ANTHROPIC_API_KEY', 'CLAUDE_CODE_OAUTH_TOKEN', 'OPENAI_API_KEY'))
process = subprocess.Popen([server['command'], *server['args']], env=environment,
                           stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
counter = 0
def rpc(method, params):
    global counter
    counter += 1
    process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': counter, 'method': method, 'params': params}) + '\n')
    process.stdin.flush()
    value = json.loads(process.stdout.readline())
    assert value['id'] == counter and 'error' not in value, value
    return value['result']
try:
    rpc('initialize', {'protocolVersion': '2025-11-25', 'capabilities': {}, 'clientInfo': {'name': 'fixture', 'version': '1'}})
    process.stdin.write(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/initialized'}) + '\n')
    process.stdin.flush()
    names = ['mcp__research__' + tool['name'] for tool in rpc('tools/list', {})['tools']]
    assert sorted(names) == sorted(option('--allowedTools').split(','))
    model = option('--model')
    emit({'type': 'system', 'subtype': 'init', 'model': model,
          'claude_code_version': '2.1.236', 'cwd': str(cwd), 'permissionMode': 'dontAsk',
          'tools': [] if MODE == 'bad_mcp' else names + (['Bash'] if MODE == 'extra_tool' else []),
          'mcp_servers': [{'name': 'research', 'status': 'failed' if MODE == 'bad_mcp' else 'connected'}],
          'skills': [], 'plugins': []})
    if MODE in ('hang', 'slow_version_hang'):
        (trial / 'child-pids.json').write_text(json.dumps([os.getpid(), process.pid]))
        emit({'type': 'system', 'subtype': 'status', 'status': 'PARTIAL_TRACE_CANARY'})
        print('PARTIAL_STDERR_CANARY', file=sys.stderr, flush=True)
        time.sleep(60)
    if MODE in ('stdout_flood', 'stderr_flood'):
        stream = sys.stdout if MODE == 'stdout_flood' else sys.stderr
        stream.write('x' * 3000000)
        stream.flush()
        time.sleep(60)
    if MODE == 'invalid_json':
        print('{broken', flush=True)
        sys.exit(0)
    if MODE == 'nonzero':
        sys.exit(7)
    emit({'type': 'stream_event', 'event': {'type': 'message_start', 'message': {'id': 'message-1', 'model': model}}})
    if MODE == 'aggregate_budget':
        emit({'type': 'stream_event', 'event': {'type': 'message_delta', 'usage': {'output_tokens': 7}, 'delta': {}}})
        emit({'type': 'stream_event', 'event': {'type': 'message_start', 'message': {'id': 'message-2', 'model': model}}})
        emit({'type': 'stream_event', 'event': {'type': 'message_delta', 'usage': {'output_tokens': 6}, 'delta': {}}})
        time.sleep(60)
    calls = [('source_read', {'path': 'src/service.py'}),
             ('source_search', {'query': 'return'}), ('source_git', {'operation': 'log'})]
    if 'mcp__research__mmcg_callees' in names:
        calls += [('mmcg_callees', {'name': 'value', 'file': 'src/service.py', 'line': 1})]
    for index, (name, arguments) in enumerate(calls):
        block = {'type': 'tool_use', 'id': 'tool-' + str(index), 'name': 'mcp__research__' + name, 'input': arguments}
        emit({'type': 'stream_event', 'event': {'type': 'content_block_start', 'content_block': block}})
        emit({'type': 'assistant', 'message': {'model': model, 'content': [block]}})
        value = rpc('tools/call', {'name': name, 'arguments': arguments})
        assert value.get('isError') is False, value
        emit({'type': 'user', 'message': {'content': [{'type': 'tool_result', 'tool_use_id': block['id'], 'content': value}]}})
    emit({'type': 'assistant', 'message': {'model': 'fallback-model' if MODE == 'fallback' else model,
                                          'content': [{'type': 'text', 'text': 'Observed source.'}]}})
    for count in (6, 12, 12):
        emit({'type': 'stream_event', 'event': {'type': 'message_delta', 'usage': {'output_tokens': count}, 'delta': {}}})
    result = {'type': 'result', 'subtype': 'success', 'is_error': False,
              'result': 'Observed src/service.py:2.\n' + 'a' * 6000, 'stop_reason': 'end_turn',
              'num_turns': 2, 'usage': {'input_tokens': 25, 'output_tokens': 12,
                    'cache_read_input_tokens': 3, 'cache_creation_input_tokens': 4},
              'total_cost_usd': 0.001, 'permission_denials': [],
              'modelUsage': {model: {'inputTokens': 25, 'outputTokens': 12}}}
    if MODE == 'model_error':
        emit({'type': 'assistant', 'error': 'server_error', 'message': {'model': '<synthetic>', 'content': []}})
        result.update(subtype='error_during_execution', is_error=True, errors=['provider unavailable'])
        result['modelUsage'] = {}
        result.pop('result')
    if MODE == 'authentication_error':
        emit({'type': 'assistant', 'error': 'authentication_failed', 'message': {'model': '<synthetic>', 'content': []}})
        result.update(subtype='error_during_execution', is_error=True, modelUsage={})
        result.pop('result')
    if MODE == 'execution_error':
        result.update(subtype='error_during_execution', is_error=True, modelUsage={})
        result.pop('result')
    if MODE == 'max_turns':
        result.update(subtype='error_max_turns', is_error=True)
    if MODE == 'max_tokens':
        result['stop_reason'] = 'max_tokens'
    if MODE == 'result_fallback':
        result['modelUsage']['fallback-model'] = {'outputTokens': 1}
    if MODE == 'permission_denied':
        result['permission_denials'] = [{'tool_name': names[0]}]
    if MODE == 'missing_usage':
        result['usage'].pop('output_tokens')
    if MODE == 'answer_limit':
        result['result'] = 'z' * 70000
    emit(result)
    emit({'type': 'system', 'subtype': 'hook_response', 'exit_code': 0})
    if MODE == 'double_result':
        emit(result)
finally:
    process.terminate()
    process.wait()
'''


NATIVE_SERVER = r'''
import json, os, pathlib, sys, time
assert sys.argv[1] == '--index' and sys.argv[3:] == ['serve']
root = pathlib.Path.cwd()
assert pathlib.Path(sys.argv[2]) == root.parent / 'index/mmcg.db'
assert not any(os.environ.get(key) for key in ('ANTHROPIC_API_KEY', 'CLAUDE_CODE_OAUTH_TOKEN', 'OPENAI_API_KEY'))
assert os.environ['MMCG_QUERY_BUDGET_MS'] == '2000'
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    if request['method'] == 'initialize':
        result = {'protocolVersion': request['params']['protocolVersion'], 'capabilities': {'tools': {}},
                  'serverInfo': {'name': 'fixture-native', 'version': '1'}}
    else:
        assert request['method'] == 'tools/call'
        args = request['params']['arguments']
        if args.get('name') == 'slow':
            time.sleep(3)
        result = {'content': [{'type': 'text', 'text': 'x' * 200000 if args.get('name') == 'huge' else 'native evidence'}],
                  'structuredContent': {'match_status': 'ambiguous', 'candidates': ['src/service.py:1', 'src/service.py:2'],
                    'precision_notes': ['syntactic edge only'], 'truncated': True, 'freshness': 'fresh'},
                  'isError': args.get('name') == 'error'}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
'''


@unittest.skipUnless(os.name == "posix" and shutil.which("git"), "requires POSIX and Git")
class ClaudeAdapterTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.BenchmarkTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.cli()

    def cli(self, mode="ok"):
        pin = self.fixture.executable("fake-claude", f"MODE = {mode!r}\n" + FAKE_CLI)
        pin["version"] = "2.1.236"
        self.fixture.config["adapter"] = {"kind": "claude_cli", "cli": pin}

    def native(self):
        body = ("import sys\nif sys.argv[3:] == ['serve']:\n" + textwrap.indent(NATIVE_SERVER, "    ")
                + "    sys.exit(0)\n" + f"CONTRACT = {fixtures.CONTRACT!r}\nMODE = 'ok'\n" + fixtures.INDEXER_BODY)
        pin = self.fixture.executable("fake-mmcg", body)
        self.fixture.config["mmcg"] = dict(pin, source_revision=self.fixture.revision,
                                           index_contract=fixtures.CONTRACT, indexed_files=["src/service.py"])

    def prepare(self, condition="source"):
        trial = self.fixture.prepare(condition)
        self.assertEqual(bench.load_json(trial / "manifest.json")["status"], "prepared",
                         bench.load_json(trial / "manifest.json"))
        return trial

    def run_trial(self, trial):
        return bench.run_trial(trial, {"ANTHROPIC_API_KEY": "fixture-key"})

    def test_three_conditions_use_real_broker_with_public_prompt_and_complete_answer(self):
        self.native()
        common = set()
        for condition in bench.CONDITIONS:
            with self.subTest(condition=condition), patch.dict(os.environ, {"SECRET_CANARY": "private", "PYTHONPATH": "/wrong"}):
                trial = self.prepare(condition)
                common.add(bench.load_json(trial / "manifest.json")["common_sha256"])
                result = self.run_trial(trial)
                self.assertEqual(result["run_status"], {"state": "completed", "reason": None},
                                 (result, (trial / "stderr.txt").read_text()))
                self.assertGreater(result["answer"]["bytes"], 6000)
                self.assertFalse(result["comparability"]["eligible"])
                self.assertEqual(result["quality"]["status"], "review_pending")
                self.assertEqual(result["diagnostics"]["telemetry"]["usage"],
                                 {"input_tokens": 25, "output_tokens": 12, "cache_read_tokens": 3, "cache_write_tokens": 4})
                self.assertEqual(result["diagnostics"]["adapter"]["live_output_tokens"], 12)
                self.assertEqual(result["diagnostics"]["tools"], ["source_read", "source_search", "source_git"] +
                                 (["mmcg"] if condition == "portable_mmcg" else []))
                raw = (trial / "claude-stream.jsonl").read_text()
                self.assertIn('"type": "tool_result"', raw)
                for secret in ("HIDDEN_RUBRIC_CANARY", "ANSWER_KEY_CANARY", "SETTINGS_CANARY", "fixture-key"):
                    self.assertNotIn(secret, raw)
        self.assertEqual(len(common), 1)

    def test_missing_auth_has_setup_envelope_without_cli_invocation(self):
        trial = self.prepare()
        result = bench.run_trial(trial)
        self.assertEqual(result["run_status"], {"state": "setup_error", "reason": "anthropic_api_key_missing"})
        self.assertIsNone(result["diagnostics"]["observed_model"])
        self.assertEqual(result["quality"]["status"], "not_evaluated")
        self.assertFalse((trial / "cli-called").exists())

    def test_cli_protocol_model_permissions_and_limits_fail_distinctly(self):
        cases = {"extra_tool": ("identity_mismatch", "observed_tool_inventory_mismatch"),
                 "bad_mcp": ("setup_error", "mcp_server_unavailable"),
                 "fallback": ("identity_mismatch", "observed_model_mismatch"),
                 "result_fallback": ("identity_mismatch", "observed_model_mismatch"),
                 "invalid_json": ("protocol_error", "malformed_cli_event"),
                 "nonzero": ("invocation_error", "cli_nonzero_exit"),
                 "model_error": ("model_error", "cli_reported_error"),
                 "authentication_error": ("setup_error", "cli_authentication_failed"),
                 "execution_error": ("invocation_error", "cli_execution_error"),
                 "max_turns": ("budget_exceeded", "error_max_turns"),
                 "max_tokens": ("budget_exceeded", "response_token_limit"),
                 "permission_denied": ("setup_error", "tool_permission_denied"),
                 "missing_usage": ("protocol_error", "incomplete_cli_telemetry"),
                 "double_result": ("protocol_error", "cli_event_after_result"),
                 "answer_limit": ("output_limit", "answer_limit"),
                 "wrong_version": ("setup_error", "claude_cli_version_mismatch")}
        for mode, (state, reason) in cases.items():
            with self.subTest(mode=mode):
                self.cli(mode)
                trial = self.prepare()
                result = self.run_trial(trial)
                self.assertEqual(result["run_status"], {"state": state, "reason": reason},
                                 (result, (trial / "stderr.txt").read_text()))
                self.assertFalse(result["comparability"]["eligible"])

    def test_live_output_budget_counts_messages_without_double_counting_updates(self):
        self.cli("aggregate_budget")
        self.fixture.config["limits"]["max_output_tokens"] = 12
        trial = self.prepare()
        result = self.run_trial(trial)
        self.assertEqual(result["run_status"], {"state": "budget_exceeded", "reason": "observed_output_budget_exceeded"})
        self.assertEqual(result["diagnostics"]["adapter"]["live_output_tokens"], 13)
        self.assertLess(result["diagnostics"]["elapsed_seconds"], 2)

    def test_runtime_bundle_and_cli_tampering_stop_before_inference(self):
        for target in ("bundle", "cli", "descriptor", "extra_file"):
            with self.subTest(target=target):
                self.cli()
                trial = self.prepare()
                if target == "extra_file":
                    (trial / "runtime/injected.py").write_text("raise RuntimeError('injection')")
                else:
                    path = {"bundle": trial / "runtime/benchmark_tools.py", "descriptor": trial / "adapter-runtime.json",
                            "cli": Path(self.fixture.config["adapter"]["cli"]["path"])}[target]
                    path.chmod(0o755)
                    path.write_bytes(path.read_bytes() + b"\n# changed\n")
                result = self.run_trial(trial)
                self.assertEqual(result["run_status"]["state"], "setup_error")
                self.assertFalse((trial / "cli-called").exists())

    def test_outer_timeout_kills_nested_cli_and_mcp(self):
        self.cli("hang")
        trial = self.prepare()
        manifest = bench.load_json(trial / "manifest.json")
        adapter = manifest["adapter"]
        command = [adapter["python"]["path"], "-I", "-S", "-B", adapter["path"],
                   "--runtime", str(trial / "adapter-runtime.json")]
        result = run_bounded(command, cwd=trial / "source", env=bench.clean_environment(trial, {"ANTHROPIC_API_KEY": "fixture-key"}),
                             stdin=bench.canonical(bench.load_json(trial / "request.json")), timeout=1)
        self.assertEqual(result.stop_reason, "timeout")
        self.assertIn(b"PARTIAL_TRACE_CANARY", (trial / "claude-stream.jsonl").read_bytes())
        self.assertIn(b"PARTIAL_STDERR_CANARY", result.stderr)
        pids = json.loads((trial / "child-pids.json").read_text())
        for pid in pids:
            self.assert_process_stopped(pid)

    def test_cli_timeout_shares_deadline_with_version_check_and_preserves_stream(self):
        self.cli("slow_version_hang")
        trial = self.prepare()
        result = self.run_trial(trial)
        self.assertEqual(result["run_status"], {"state": "timeout", "reason": "cli_timeout"})
        self.assertIn(b"PARTIAL_TRACE_CANARY", (trial / "claude-stream.jsonl").read_bytes())
        self.assertIn("PARTIAL_STDERR_CANARY", (trial / "stderr.txt").read_text())

    def test_cli_output_and_diagnostic_floods_are_bounded(self):
        for mode in ("stdout_flood", "stderr_flood"):
            with self.subTest(mode=mode):
                self.cli(mode)
                trial = self.prepare()
                result = self.run_trial(trial)
                limits = bench.load_json(trial / "manifest.json")["limits"]
                self.assertEqual(result["run_status"]["state"], "output_limit")
                self.assertLessEqual((trial / "claude-stream.jsonl").stat().st_size, limits["trace_bytes"])
                self.assertLessEqual((trial / "stderr.txt").stat().st_size, limits["stderr_bytes"])

    def assert_process_stopped(self, pid):
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            probe = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True, timeout=2)
            if probe.returncode or probe.stdout.lstrip().startswith("Z"):
                return
            time.sleep(0.03)
        self.fail(f"process {pid} survived outer supervisor cleanup")

    def broker(self, condition="source", timeout=1):
        trial = self.prepare(condition)
        broker = SourceBroker(bench.load_json(trial / "request.json"), native_timeout=timeout)
        self.addCleanup(broker.close)
        return trial, broker

    def test_source_tools_deny_paths_and_unsupported_arguments(self):
        _, broker = self.broker()
        for path in ("../rubric.json", "/etc/passwd", "evals/answer.json", "AGENTS.md", "missing.py", "src//service.py"):
            with self.subTest(path=path), self.assertRaises(bench.BenchmarkError):
                broker.call("source_read", {"path": path})
        for tool, args in (("source_git", {"operation": "shell"}), ("source_read", {"path": "src/service.py", "command": "cat"}),
                           ("source_search", {"query": "x", "case_sensitive": 1}), ("mmcg_callees", {"name": "value"})):
            with self.subTest(tool=tool, args=args), self.assertRaises(bench.BenchmarkError):
                broker.call(tool, args)
        self.assertEqual(broker.call("source_search", {"query": ".*"})["structuredContent"]["matches"], [])
        data = broker.call("source_read", {"path": "src/service.py", "start_line": 2, "end_line": 2})["structuredContent"]
        self.assertEqual(data["lines"], [{"line": 2, "text": "    return 7"}])

    def test_source_mcp_accepts_metadata_and_returns_tool_errors_without_exiting(self):
        trial = self.prepare()
        request = bench.load_json(trial / "request.json")
        with McpClient([sys.executable, "-I", "-S", "-B", str(trial / "runtime/benchmark_tools.py"),
                        "--request", str(trial / "request.json"), "--sha256", bench.digest(request)],
                       cwd=trial, env=bench.clean_environment(trial)) as client:
            tools = client.request("tools/list", {"_meta": {"progressToken": "fixture"}})["tools"]
            self.assertEqual({item["name"] for item in tools}, {"source_read", "source_search", "source_git"})
            denied = client.call("source_read", {"path": "../rubric.json"})
            self.assertTrue(denied["isError"])
            allowed = client.request("tools/call", {"name": "source_read", "arguments": {"path": "src/service.py"},
                                                   "_meta": {"progressToken": 1}})
            self.assertFalse(allowed["isError"])
            self.assertEqual(allowed["structuredContent"]["total_lines"], 2)

    def test_source_symlinks_and_changed_bytes_fail_snapshot_creation(self):
        for mode in ("parent", "leaf", "bytes"):
            with self.subTest(mode=mode):
                trial = self.prepare()
                source = trial / "source/src/service.py"
                if mode == "parent":
                    source.unlink()
                    source.parent.rmdir()
                    source.parent.symlink_to(self.fixture.repo / "src", target_is_directory=True)
                elif mode == "leaf":
                    source.unlink()
                    source.symlink_to(self.fixture.repo / "src/service.py")
                else:
                    source.chmod(0o644)
                    source.write_text("CHANGED_CANARY")
                with self.assertRaises(bench.BenchmarkError):
                    SourceBroker(bench.load_json(trial / "request.json"))

    def test_source_file_lines_match_ast_and_empty_files_are_readable(self):
        trial, broker = self.broker()
        body = "x = 1\n\fdef target():\n    return 1\n"
        broker.bodies["src/service.py"] = body.encode()
        line = ast.parse(body).body[1].lineno
        match = broker.call("source_search", {"query": "def target"})["structuredContent"]["matches"][0]
        self.assertEqual(match["line"], line)
        data = broker.call("source_read", {"path": "src/service.py", "start_line": line, "end_line": line})["structuredContent"]
        self.assertEqual(data["lines"][0]["text"], "\fdef target():")
        broker.bodies["src/service.py"] = b""
        self.assertEqual(broker.call("source_read", {"path": "src/service.py"})["structuredContent"]["lines"], [])

    def test_native_metadata_errors_and_recovery_survive_proxy(self):
        self.native()
        _, broker = self.broker("portable_mmcg", timeout=0.3)
        for name, expected in (("huge", "mcp_output_limit"), ("slow", "mcp_timeout")):
            with self.subTest(name=name):
                with self.assertRaises(bench.BenchmarkError) as raised:
                    broker.call("mmcg_callees", {"name": name})
                self.assertEqual(raised.exception.code, expected)
                recovered = broker.call("mmcg_callees", {"name": "value"})
                self.assertFalse(recovered["isError"])
                self.assertEqual(recovered["structuredContent"]["match_status"], "ambiguous")
                self.assertEqual(recovered["structuredContent"]["precision_notes"], ["syntactic edge only"])
                self.assertTrue(recovered["structuredContent"]["truncated"])
        self.assertTrue(broker.call("mmcg_callees", {"name": "error"})["isError"])
        self.assertFalse(broker.call("mmcg_callees", {"name": "value"})["isError"])
        for args in ({"name": "value", "file": "../secret"}, {"name": "value", "line": 1},
                     {"name": "value", "language": "shell"}, {"name": "value", "sql": "select *"}):
            with self.subTest(args=args), self.assertRaises(bench.BenchmarkError):
                broker.call("mmcg_callees", args)


if __name__ == "__main__":
    unittest.main()
