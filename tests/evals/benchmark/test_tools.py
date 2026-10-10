"""Advertised read ranges through the frozen source broker."""

import io
import hashlib
import unittest
from unittest.mock import Mock, patch

from evals.benchmark import artifacts, tools
from tests.evals.support.benchmark import BenchmarkFixture


class SourceReadTests(unittest.TestCase):
    def test_native_delivery_forwards_task_binding_and_rejects_changed_source(self):
        fixture = BenchmarkFixture()
        self.addCleanup(fixture.close)
        request = artifacts.load_json(fixture.prepare() / "request.json")
        request["mmcg"] = {"source_delivery":"native_reuse"}
        request["available_tools"].append("mmcg")
        broker = tools.SourceBroker(request)
        self.addCleanup(broker.close)
        client = Mock()
        broker.native = client
        path = next(iter(broker.bodies))
        lines = broker.lines(path)
        body = {"path":path, "source_sha256":hashlib.sha256(broker.bodies[path]).hexdigest(),
            "segments":[{"start_line":2,"end_line":len(lines),
                "lines":[{"line":n,"text":lines[n-1]} for n in range(2,len(lines)+1)]}],
            "reuse_status":"receipt_unavailable", "reused_ranges":[], "receipt":"b"*32}
        response = {"structuredContent":body, "content":[{"type":"text", "text":artifacts.canonical(body).decode()}], "isError":False}
        client.call.return_value = response
        with patch.object(broker, "ensure_native"):
            actual = broker.call("source_read", {"path":path, "start_line":2, "previous_receipt":"a"*32})
        self.assertEqual(actual, response)
        client.call.assert_called_once_with("mmcg_read", {"file":path, "task":request["task"]["id"], "start_line":2,"previous_receipt":"a"*32})
        changed = {**body, "source_sha256":"0"*64}
        client.call.return_value = {"structuredContent":changed, "isError":False}
        with patch.object(broker, "ensure_native"), self.assertRaises(artifacts.BenchmarkError) as caught:
            broker.call("source_read", {"path":path})
        self.assertEqual(caught.exception.code, "input_changed")
        client.close.assert_called_once()
        self.assertIsNone(broker.native)

    def test_native_full_denies_receipts_before_start_and_reuse_requires_capability(self):
        fixture = BenchmarkFixture()
        self.addCleanup(fixture.close)
        request = artifacts.load_json(fixture.prepare() / "request.json")
        request["mmcg"] = {"source_delivery":"native_full"}
        request["available_tools"].append("mmcg")
        broker = tools.SourceBroker(request)
        self.addCleanup(broker.close)
        path = next(iter(broker.bodies))
        with patch.object(broker, "ensure_native") as start:
            with self.assertRaises(artifacts.BenchmarkError):
                broker.call("source_read", {"path":path, "previous_receipt":"a"*32})
            start.assert_not_called()
        client = Mock()
        client.request.return_value = {"tools":tools.tool_definitions(True)}
        broker.native = client
        with patch.object(broker, "ensure_native"), self.assertRaises(artifacts.BenchmarkError) as caught:
            broker.definitions()
        self.assertEqual(caught.exception.code, "tool_denied")
        client.call.assert_not_called()
        client.close.assert_called_once()
        schema = tools.tool_definitions(True, source_delivery="native_reuse")[0]["inputSchema"]
        self.assertIn("previous_receipt", schema["properties"])
        self.assertNotIn("previous_receipt", tools.tool_definitions(True, source_delivery="native_full")[0]["inputSchema"]["properties"])
    def test_empty_resource_catalog_does_not_grant_reads_or_pagination(self):
        fixture = BenchmarkFixture()
        self.addCleanup(fixture.close)
        trial = fixture.prepare()
        request = artifacts.load_json(trial / "request.json")
        request["resource_discovery"] = "empty_only"
        request["available_tools"].append("resource_discovery")
        broker = tools.SourceBroker(request)
        self.addCleanup(broker.close)
        messages = [
            {"id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25"}},
            {"method": "notifications/initialized"},
            {"id": 2, "method": "resources/list", "params": {"_meta": {"progressToken": 1}}},
            {"id": 3, "method": "resources/templates/list"},
            {"id": 4, "method": "resources/read", "params": {"uri": "file:///etc/passwd"}},
            {"id": 5, "method": "resources/list", "params": {"cursor": "next"}},
        ]
        stream = io.BytesIO(b"".join(artifacts.canonical({"jsonrpc": "2.0", **m}) + b"\n" for m in messages))
        output = io.BytesIO()
        tools.serve(broker, stream, output)
        replies = {m["id"]: m for m in (artifacts.parse_json(line) for line in output.getvalue().splitlines())}
        self.assertEqual(replies[2]["result"], {"resources": []})
        self.assertEqual(replies[3]["result"], {"resourceTemplates": []})
        self.assertEqual(replies[4]["error"]["code"], -32601)
        self.assertEqual(replies[5]["error"]["code"], -32600)
        self.assertEqual(broker.calls, 0)

    def test_advertised_ranges_cover_a_body_without_missing_or_repeated_lines(self):
        fixture = BenchmarkFixture()
        self.addCleanup(fixture.close)
        body = [f"# source line {number}" for number in range(1, 202)]
        (fixture.repo / "src/service.py").write_text("\n".join(body) + "\n")
        fixture.git("add", "src/service.py")
        fixture.git("commit", "-qm", "long source body")
        revision = fixture.git("rev-parse", "HEAD").strip()
        fixture.task["revision"] = fixture.rubric["source_revision"] = revision
        trial = fixture.prepare()
        broker = tools.SourceBroker(artifacts.load_json(trial / "request.json"))
        self.addCleanup(broker.close)

        description = next(row["description"] for row in tools.tool_definitions(False)
                           if row["name"] == "source_read")
        self.assertIn("at most 200 lines", description)
        self.assertIn("80 by default", description)
        self.assertIn("inclusive", description)
        self.assertIn("next_line", description)
        default = broker.call("source_read", {"path": "src/service.py"})["structuredContent"]
        self.assertEqual(default["end_line"], 80)
        first = broker.call("source_read", {"path": "src/service.py", "end_line": 201})["structuredContent"]
        self.assertEqual((first["requested_end_line"], first["end_line"], first["range_truncated"], first["next_line"]),
                         (201, 200, True, 201))
        remaining = broker.call("source_git", {"operation": "show", "path": "src/service.py",
                                               "start_line": first["next_line"], "end_line": 400})["structuredContent"]
        self.assertEqual((remaining["start_line"], remaining["end_line"], remaining["total_lines"]), (201, 201, 201))
        self.assertFalse(remaining["range_truncated"])
        self.assertIsNone(remaining["next_line"])
        self.assertEqual([row["text"] for row in first["lines"] + remaining["lines"]], body)
        with self.assertRaises(artifacts.BenchmarkError):
            broker.call("source_read", {"path": "src/service.py", "start_line": 202})


class SearchBatchTests(unittest.TestCase):
    def setUp(self):
        self.fixture = BenchmarkFixture()
        self.addCleanup(self.fixture.close)
        trial = self.fixture.prepare("portable_mmcg")
        self.request = artifacts.load_json(trial / "request.json")

    def test_single_condition_hides_and_denies_batch_even_when_native_supports_it(self):
        request = {**self.request, "mmcg": {**self.request["mmcg"], "symbol_lookup": "single"}}
        broker = tools.SourceBroker(request)
        self.addCleanup(broker.close)
        client = Mock()
        client.request.return_value = {"tools": tools.tool_definitions(True, batch_search=True)}
        response = {"content": [], "isError": False}
        client.call.return_value = response
        with patch.object(tools, "McpClient", return_value=client):
            schema = next(row["inputSchema"] for row in broker.definitions() if row["name"] == "mmcg_search")
            self.assertNotIn("names", schema["properties"])
            with self.assertRaises(artifacts.BenchmarkError) as denied:
                broker.call("mmcg_search", {"names": ["value"]})
            self.assertEqual(denied.exception.code, "tool_denied")
            client.call.assert_not_called()
            self.assertIs(broker.call("mmcg_search", {"name": "value"}), response)
            client.call.assert_called_once_with("mmcg_search", {"name": "value"})

    def test_batch_condition_rejects_unsupported_native_without_a_query_or_silent_fallback(self):
        request = {**self.request, "mmcg": {**self.request["mmcg"], "symbol_lookup": "batch"}}
        broker = tools.SourceBroker(request)
        self.addCleanup(broker.close)
        client = Mock()
        client.request.return_value = {"tools": tools.tool_definitions(True)}
        with patch.object(tools, "McpClient", return_value=client):
            for operation in (broker.definitions, lambda: broker.call("mmcg_search", {"name": "value"})):
                with self.assertRaises(artifacts.BenchmarkError) as denied:
                    operation()
                self.assertEqual(denied.exception.code, "tool_denied")
        client.call.assert_not_called()

    def test_catalog_gates_batch_support_and_preserves_native_results(self):
        for supported in (False, True):
            with self.subTest(supported=supported):
                broker = tools.SourceBroker(self.request)
                self.addCleanup(broker.close)
                client = Mock()
                catalog = [row for row in tools.tool_definitions(True, batch_search=supported)
                           if row["name"] in tools.GRAPH_TOOLS]
                client.request.return_value = {"tools": catalog}
                response = {"content": [], "structuredContent": {"queries": [
                    {"query": "value", "truncated": True, "total": None,
                     "precision_notes": ["name_based"], "raw_work_limit": 500}]}, "isError": False}
                client.call.return_value = response
                with patch.object(tools, "McpClient", return_value=client):
                    definitions = broker.definitions()
                    schema = next(row["inputSchema"] for row in definitions if row["name"] == "mmcg_search")
                    self.assertEqual("names" in schema["properties"], supported)
                    broker.definitions()
                    client.request.assert_called_once_with("tools/list", {})
                    args = {"names": ["value", "missing"], "top": 1}
                    if supported:
                        self.assertIs(broker.call("mmcg_search", args), response)
                        client.call.assert_called_once_with("mmcg_search", args)
                    else:
                        with self.assertRaises(artifacts.BenchmarkError) as denied:
                            broker.call("mmcg_search", args)
                        self.assertEqual(denied.exception.code, "tool_denied")
                        client.call.assert_not_called()
                        self.assertIs(broker.call("mmcg_search", {"name": "value"}), response)

    def test_invalid_batch_arguments_do_not_start_or_refresh_native_runtime(self):
        broker = tools.SourceBroker(self.request)
        self.addCleanup(broker.close)
        for args in ({"names": None}, {"names": []}, {"names": ["value", "value"]},
                     {"name": "value", "names": ["value"]}, {"names": [" "]},
                     {"names": ["value", True]}, {"names": [str(n) for n in range(9)]},
                     {"names": ["x" * 1025]}, {"names": ["x\x7f"]},
                     {"names": ["value"], "top": 26}, {"names": ["value"], "top": True}):
            with self.subTest(args=args), patch.object(tools, "McpClient") as native:
                with self.assertRaises(artifacts.BenchmarkError) as invalid:
                    broker.call("mmcg_search", args)
                self.assertEqual(invalid.exception.code, "invalid_arguments")
                native.assert_not_called()

    def test_catalog_failure_keeps_failure_class_and_restarts_the_session(self):
        broker = tools.SourceBroker(self.request)
        self.addCleanup(broker.close)
        client = Mock()
        client.request.side_effect = artifacts.BenchmarkError("mcp_timeout", "catalog timeout")
        with patch.object(tools, "McpClient", return_value=client):
            with self.assertRaises(artifacts.BenchmarkError) as error:
                broker.definitions()
        self.assertEqual(error.exception.code, "mcp_timeout")
        client.close.assert_called_once()
        self.assertIsNone(broker.native)
        self.assertIsNone(broker.native_batch_search)
