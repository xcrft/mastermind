"""Advertised read ranges through the frozen source broker."""

import unittest
from unittest.mock import Mock, patch

from evals.benchmark import artifacts, tools
from tests.evals.support.benchmark import BenchmarkFixture


class SourceReadTests(unittest.TestCase):
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
