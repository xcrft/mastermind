"""Codex stream and frozen runtime contracts, with no model calls."""

from pathlib import Path
from unittest.mock import patch
import os
import unittest

from evals.benchmark import artifacts, protocol, review, trials
from evals.benchmark.review_io import Root
from evals.benchmark.adapters import codex
from tests.evals.support.benchmark import BenchmarkFixture


@unittest.skipUnless(os.name == "posix", "POSIX process supervision")
class CodexTests(unittest.TestCase):
    def test_invalid_or_unavailable_timing_cannot_become_a_zero_latency(self):
        base = {"usage": {"input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 0,
                          "cache_write_tokens": 0}, "turns": 1}
        limits = protocol.validate_limits({})
        legacy = protocol.telemetry(base, limits)
        self.assertTrue(legacy["complete"])
        self.assertEqual(legacy["timings"], {"first_message_seconds": None, "final_answer_seconds": None})
        for timing in ({"first_message_seconds": -1, "final_answer_seconds": 2},
                       {"first_message_seconds": True, "final_answer_seconds": 2},
                       {"first_message_seconds": 3, "final_answer_seconds": 2},
                       {"first_message_seconds": None, "final_answer_seconds": 2},
                       {"first_message_seconds": 10**1000, "final_answer_seconds": None},
                       {"first_message_seconds": float("inf"), "final_answer_seconds": None}):
            with self.subTest(timing=timing):
                measured = protocol.telemetry(dict(base, timings=timing), limits)
                self.assertFalse(measured["complete"])
                self.assertIn("invalid_timings", measured["issues"])
                self.assertTrue(all(value is None for value in measured["timings"].values()))

    def test_latency_starts_at_native_launch_and_does_not_count_reasoning_tools_or_empty_messages(self):
        moment = [100.0]
        observer = codex.StreamObserver({"model": "fixture", "mmcg": None}, lambda _: None,
                                        clock=lambda: moment[0])
        observer.start_timing()
        observer.event({"type": "thread.started"})
        moment[0] = 102.0
        for item in ({"type": "reasoning"}, {"type": "agent_message", "text": " "},
                     {"type": "mcp_tool_call", "id": "1", "server": "research", "tool": "source_read"}):
            observer.event({"type": "item.completed", "item": item})
        self.assertIsNone(observer.timings["first_message_seconds"])
        moment[0] = 107.0
        observer.event({"type": "item.completed", "item": {"type": "agent_message", "text": "Reading source."}})
        moment[0] = 112.0
        observer.event({"type": "item.completed", "item": {"type": "agent_message", "text": "Final answer."}})
        moment[0] = 113.0
        observer.event({"type": "turn.completed", "usage": {
            "input_tokens": 10, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "output_tokens": 2}})
        self.assertEqual(observer.timings, {"first_message_seconds": 7.0, "final_answer_seconds": 13.0})
        self.assertEqual(observer.answer, "Final answer.")
        self.assertFalse(observer.tool_timeline.report(13)["accounting_complete"])
        self.assertIsNone(observer.tool_timeline.report(13)["tool_interval_union_seconds"])

    def test_frozen_subscription_runtime_and_cache_accounting(self):
        for uncapped in (False, True):
            with self.subTest(uncapped=uncapped):
                fixture = BenchmarkFixture()
                self.addCleanup(fixture.close)
                auth = fixture.root / "auth"
                auth.mkdir()
                cli = fixture.executable("fake-codex", f'''
        import json, os, sys
        if sys.argv[1:] == ['--version']:
            print('codex-cli 0.160.1')
            sys.exit(0)
        assert os.environ['CODEX_HOME'] == {str(auth)!r}
        assert not any(os.environ.get(key) for key in ('OPENAI_API_KEY', 'ANTHROPIC_API_KEY'))
        assert '--ignore-user-config' in sys.argv and '--ignore-rules' in sys.argv
        assert '--ephemeral' in sys.argv and '--sandbox' in sys.argv
        assert os.path.basename(os.getcwd()) == 'source'
        assert os.path.isfile('src/service.py')
        assert not os.path.exists('AGENTS.md')
        assert 'project_doc_max_bytes=0' in sys.argv
        assert 'forced_login_method="chatgpt"' in sys.argv
        assert 'skip_host_skill_discovery' in sys.argv
        assert 'mcp_servers={{}}' in sys.argv
        assert '--disable' in sys.argv and 'multi_agent' in sys.argv
        assert 'HIDDEN_RUBRIC_CANARY' not in sys.stdin.read()
        for event in [{{'type':'thread.started'}}, {{'type':'turn.started'}},
            {{'type':'item.completed','item':{{'type':'agent_message','text':'Observed src/service.py:2.'}}}},
            {{'type':'turn.completed','usage':{{'input_tokens':100,'cached_input_tokens':30,'cache_write_input_tokens':10,'output_tokens':{70000 if uncapped else 4},'reasoning_output_tokens':2}}}}]:
            print(json.dumps(event), flush=True)
        ''')
                cli["version"] = "0.160.1"
                fixture.config["adapter"] = {"kind": "codex_cli", "cli": cli, "auth_home": str(auth), "reasoning_effort": "max"}
                if uncapped:
                    fixture.config["limits"].update(timeout_seconds=None, max_turns=None, max_output_tokens=None)
                trial = fixture.prepare()
                with patch.dict(os.environ, {"OPENAI_API_KEY": "must-not-forward"}):
                    result = trials.run_trial(trial)
                self.assertEqual(result["run_status"]["state"], "completed")
                self.assertEqual(result["diagnostics"]["telemetry"]["usage"], {
                    "input_tokens": 60, "cache_read_tokens": 30, "cache_write_tokens": 10, "output_tokens": 70000 if uncapped else 4})
                self.assertIsNone(result["diagnostics"]["telemetry"]["cost_usd"])
                self.assertEqual(result["diagnostics"]["adapter"]["model_identity"], "requested_not_observed")
                timing = result["diagnostics"]["telemetry"]["timings"]
                self.assertGreaterEqual(timing["first_message_seconds"], 0)
                self.assertGreaterEqual(timing["final_answer_seconds"], timing["first_message_seconds"])
                self.assertTrue((trial / "codex-stream.jsonl").is_file())
                diagnostics = result["diagnostics"]["adapter"]
                self.assertEqual(diagnostics["raw_stream_sha256"],
                    artifacts.hash_file(trial / "codex-stream.jsonl",
                        protocol.validate_limits(fixture.config["limits"])["trace_bytes"])["sha256"])
                self.assertEqual(diagnostics["output_breakdown"]["reasoning_output_tokens"], 2)
                self.assertEqual(diagnostics["output_breakdown"]["non_reasoning_output_tokens"],
                    (70000 if uncapped else 4) - 2)
                self.assertTrue(diagnostics["tool_timeline"]["accounting_complete"])
                self.assertEqual(diagnostics["tool_timeline"]["call_count"], 0)
                self.assertFalse(result["comparability"]["eligible"])
                runtime = artifacts.load_json(trial / "adapter-runtime.json")
                self.assertEqual(protocol.common_identity(artifacts.load_json(trial / "manifest.json"))["adapter"]["settings"], runtime["settings"])

                self.assertEqual("timeout_seconds" in result["diagnostics"]["enforced_limits"], not uncapped)
                self.assertEqual(result["diagnostics"]["adapter"]["model_budget_enforcement"],
                    "disabled" if uncapped else "reported_after_turn")
                manifest_body = artifacts.read_file(trial / "manifest.json")
                manifest = artifacts.parse_json(manifest_body)
                with Root(trial) as root:
                    self.assertEqual(review.read_result(root, "", manifest, manifest_body)[0], "completed")
                (trial / "codex-stream.jsonl").write_bytes(b'{"type":"changed"}\n')
                with Root(trial) as root, self.assertRaises(artifacts.BenchmarkError) as caught:
                    review.read_result(root, "", manifest, manifest_body)
                self.assertEqual(caught.exception.code, "review_trace")

    def test_stream_rejects_unavailable_tools_context_errors_and_ambiguous_usage(self):
        request = {"model": "fixture-model", "mmcg": None}
        cases = [
            ({"type": "item.started", "item": {"type": "mcp_tool_call", "id": "1", "server": "codex", "tool": "list_mcp_resources", "arguments": {}}}, "identity_mismatch"),
            ({"type": "item.started", "item": {"type": "mcp_tool_call", "id": "1", "server": "research", "tool": "mmcg_callees"}}, "identity_mismatch"),
            ({"type": "item.completed", "item": {"type": "command_execution"}}, "identity_mismatch"),
            ({"type": "item.completed", "item": {"type": "error", "message": "context unavailable"}}, "protocol_error"),
            ({"type": "error", "message": None}, "protocol_error"),
            ({"type": "turn.completed", "usage": {"input_tokens": 2, "cached_input_tokens": 3, "cache_write_input_tokens": 0}}, "protocol_error"),
            ({"type": "turn.completed", "usage": {"input_tokens": 2, "cached_input_tokens": 0}}, "protocol_error"),
        ]
        for event, state in cases:
            with self.subTest(event=event):
                observer = codex.StreamObserver(request, lambda _: None)
                observer.event({"type": "thread.started"})
                self.assertIsNotNone(observer.feed(artifacts.canonical(event) + b"\n"))
                self.assertEqual(observer.failure["state"], state)

    def test_reconnect_notice_waits_for_completion_failure_or_stream_end(self):
        endings = {
            "recovered": [
                {"type": "item.completed", "item": {"type": "agent_message", "text": "Final src/service.py:2."}},
                {"type": "turn.completed", "usage": {"input_tokens": 30, "cached_input_tokens": 10,
                    "cache_write_input_tokens": 0, "output_tokens": 4}}],
            "failed": [{"type": "turn.failed", "error": {"message": "connection exhausted"}}],
            "ended": [],
        }
        for ending, tail in endings.items():
            with self.subTest(ending=ending):
                fixture = BenchmarkFixture()
                self.addCleanup(fixture.close)
                auth = fixture.root / "auth"
                auth.mkdir()
                events = [{"type": "thread.started"}, {"type": "turn.started"},
                    {"type": "item.completed", "item": {"type": "agent_message", "text": "Reading source."}},
                    {"type": "error", "message": "Reconnecting... 2/5 (stream disconnected before completion)"},
                    *tail]
                cli = fixture.executable("fake-codex", f'''
        import json, sys
        if sys.argv[1:] == ['--version']:
            print('codex-cli 0.162.0-alpha.2')
            sys.exit(0)
        sys.stdin.read()
        for event in {events!r}:
            print(json.dumps(event), flush=True)
        ''')
                cli["version"] = "0.162.0-alpha.2"
                fixture.config["adapter"] = {"kind": "codex_cli", "cli": cli,
                    "auth_home": str(auth), "reasoning_effort": "max"}
                result = trials.run_trial(fixture.prepare())
                self.assertEqual(result["diagnostics"]["adapter"]["stream_error_events"], 1)
                if ending == "recovered":
                    self.assertEqual(result["run_status"], {"state": "completed", "reason": None})
                    self.assertEqual(result["diagnostics"]["telemetry"]["usage"]["input_tokens"], 20)
                    self.assertEqual(result["diagnostics"]["telemetry"]["usage"]["output_tokens"], 4)
                    self.assertIsNotNone(result["diagnostics"]["telemetry"]["timings"]["final_answer_seconds"])
                else:
                    self.assertEqual(result["run_status"], {"state": "model_error", "reason":
                        "codex_turn_failed" if ending == "failed" else "codex_stream_ended_after_error"})
                    self.assertIsNone(result["diagnostics"]["telemetry"]["usage"]["input_tokens"])
                    self.assertIsNone(result["diagnostics"]["telemetry"]["timings"]["final_answer_seconds"])

    def test_empty_discovery_uses_the_frozen_broker_and_keeps_source_coverage_separate(self):
        for unexpected_resource in (False, True):
            with self.subTest(unexpected_resource=unexpected_resource):
                fixture = BenchmarkFixture()
                self.addCleanup(fixture.close)
                auth = fixture.root / "auth"
                auth.mkdir()
                cli = fixture.executable("fake-codex", f'''
        import json, os, subprocess, sys
        if sys.argv[1:] == ['--version']:
            print('codex-cli 0.162.0-alpha.2')
            sys.exit(0)
        config = {{}}
        for i, arg in enumerate(sys.argv[:-1]):
            if arg == '-c':
                key, value = sys.argv[i + 1].split('=', 1)
                config[key] = json.loads(value)
        assert config['mcp_servers'] == {{}}
        server = 'mcp_servers.research.'
        env = dict(os.environ, **{{key[len(server + 'env.'):]: value for key, value in config.items()
                                 if key.startswith(server + 'env.')}})
        process = subprocess.Popen([config[server + 'command'], *config[server + 'args']],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
        def rpc(method, params):
            process.stdin.write(json.dumps({{'jsonrpc':'2.0','id':1,'method':method,'params':params}}) + '\\n')
            process.stdin.flush()
            reply = json.loads(process.stdout.readline())
            assert 'error' not in reply, reply
            return reply['result']
        def emit(event):
            print(json.dumps(event), flush=True)
        rpc('initialize', {{'protocolVersion':'2025-11-25'}})
        process.stdin.write(json.dumps({{'jsonrpc':'2.0','method':'notifications/initialized'}}) + '\\n')
        process.stdin.flush()
        emit({{'type':'thread.started'}})
        emit({{'type':'turn.started'}})
        for identifier, name, args, method in (
            ('resources', 'list_mcp_resources', {{}}, 'resources/list'),
            ('templates', 'list_mcp_resource_templates', {{'server':'research'}}, 'resources/templates/list')):
            item = {{'type':'mcp_tool_call','id':identifier,'server':args.get('server', 'codex'),'tool':name,
                    'arguments':args,'status':'in_progress'}}
            emit({{'type':'item.started','item':item}})
            body = rpc(method, {{}})
            if args:
                body['server'] = args['server']
            if {unexpected_resource!r}:
                body['resources'] = [{{'uri':'private://unavailable'}}]
            item.update(status='completed', result={{'content':[{{'type':'text','text':json.dumps(body)}}],
                'structured_content':None}}, error=None)
            emit({{'type':'item.completed','item':item}})
        item = {{'type':'mcp_tool_call','id':'read','server':'research','tool':'source_read',
                'arguments':{{'path':'src/service.py'}},'status':'in_progress'}}
        emit({{'type':'item.started','item':item}})
        item.update(status='completed', result=rpc('tools/call', {{'name':'source_read',
            'arguments':item['arguments']}}), error=None)
        emit({{'type':'item.completed','item':item}})
        process.stdin.close()
        process.wait()
        emit({{'type':'item.completed','item':{{'type':'agent_message','text':'Observed src/service.py:2.'}}}})
        emit({{'type':'turn.completed','usage':{{'input_tokens':30,'cached_input_tokens':0,
            'cache_write_input_tokens':0,'output_tokens':4}}}})
        ''')
                cli["version"] = "0.162.0-alpha.2"
                fixture.config["adapter"] = {"kind": "codex_cli", "cli": cli,
                    "auth_home": str(auth), "reasoning_effort": "max"}
                trial = fixture.prepare()
                result = trials.run_trial(trial)
                if unexpected_resource:
                    self.assertEqual(result["run_status"], {"state": "identity_mismatch",
                        "reason": "resource_discovery_not_empty"})
                    self.assertIsNone(result["diagnostics"]["telemetry"]["usage"]["input_tokens"])
                else:
                    self.assertEqual(result["run_status"]["state"], "completed")
                    diagnostics = result["diagnostics"]["adapter"]
                    self.assertEqual(diagnostics["resource_discovery"], {
                        "contract": "empty_only", "calls": 2, "completed_empty": 2})
                    self.assertEqual(diagnostics["mcp_calls"], 3)
                    self.assertEqual(diagnostics["read_ledger"]["read_calls"], 1)
                    self.assertEqual(diagnostics["read_ledger"]["unique_returned_lines"], 2)
                    self.assertEqual(result["diagnostics"]["unexpected_tools"], [])

    def test_discovery_denies_resource_reads_other_servers_pagination_and_unbound_results(self):
        request = {"model": "fixture", "mmcg": None, "resource_discovery": "empty_only"}
        base = {"type": "mcp_tool_call", "id": "catalog", "server": "codex",
                "tool": "list_mcp_resources", "arguments": {}, "status": "in_progress"}
        for change in ({"tool": "read_mcp_resource"}, {"arguments": {"server": "other"}},
                       {"arguments": {"cursor": "next"}}, {"arguments": {"uri": "private://x"}},
                       {"arguments": None}, {"id": None}):
            with self.subTest(change=change):
                observer = codex.StreamObserver(request, lambda _: None)
                self.assertIsNotNone(observer.event({"type": "item.started", "item": base | change}))
                self.assertEqual(observer.failure["code"], "unavailable_tool_called")
        for body in ({"resources": [], "nextCursor": "next"}, {"resources": [], "server": "other"},
                     {"resources": [], "extra": "outside evidence"}, {"resources": [{"uri": "x"}]}):
            with self.subTest(body=body):
                observer = codex.StreamObserver(request, lambda _: None)
                observer.event({"type": "item.started", "item": base})
                item = base | {"status": "completed", "result": {
                    "content": [{"type": "text", "text": artifacts.canonical(body).decode()}]}}
                self.assertIsNotNone(observer.event({"type": "item.completed", "item": item}))
                self.assertEqual(observer.failure["code"], "resource_discovery_not_empty")
        observer = codex.StreamObserver(request, lambda _: None)
        observer.event({"type": "item.started", "item": base})
        self.assertIsNotNone(observer.event({"type": "item.completed", "item": base | {
            "server": "research", "arguments": {"server": "research"}}}))
        self.assertEqual(observer.failure["code"], "resource_discovery_changed")

    def test_unfinished_discovery_cannot_complete_a_trial(self):
        observer = codex.StreamObserver({"model": "fixture", "mmcg": None,
            "resource_discovery": "empty_only"}, lambda _: None)
        observer.event({"type": "item.started", "item": {"type": "mcp_tool_call", "id": "catalog",
            "server": "codex", "tool": "list_mcp_resources", "arguments": {}}})
        observer.event({"type": "turn.completed", "usage": {"input_tokens": 1,
            "cached_input_tokens": 0, "cache_write_input_tokens": 0, "output_tokens": 1}})
        self.assertEqual(observer.failure, {"state": "protocol_error", "code": "incomplete_resource_discovery"})
        self.assertEqual(observer.usage["input_tokens"], 1)
