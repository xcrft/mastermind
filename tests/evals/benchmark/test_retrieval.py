"""Returned-range accounting with overlaps, continuations and unknown results."""

import unittest

from evals.benchmark import artifacts
from evals.benchmark.retrieval import ReadLedger
from evals.benchmark.adapters.codex import StreamObserver


def read(identifier, start, end, *, tool="source_read", path="src/value.py"):
    body = {"path": path, "start_line": start, "end_line": end, "total_lines": 500,
            "lines": [{"line": line, "text": f"line {line}"} for line in range(start, end + 1)]}
    args = {"path": path, "start_line": start, "end_line": end}
    if tool == "source_git":
        args["operation"] = "show"
    return {"type": "mcp_tool_call", "server": "research", "tool": tool,
            "id": identifier, "arguments": args, "status": "completed", "error": None,
            "result": {"structured_content": body,
                       "content": [{"type": "text", "text": artifacts.canonical(body).decode()}]}}


class ReadLedgerTests(unittest.TestCase):
    def test_observer_counts_only_completed_calls_and_merges_overlap_across_read_tools(self):
        source = {"path": "src/value.py", "sha256": "a" * 64}
        observer = StreamObserver({"model": "fixture", "mmcg": None, "source_files": [source]}, lambda _: None)
        for item in (read("first", 1, 200), read("overlap", 150, 250, tool="source_git"),
                     read("last", 251, 300), read("separate", 400, 410)):
            observer.event({"type": "item.started", "item": item})
            observer.event({"type": "item.completed", "item": item})
        observer.event({"type": "item.completed", "item": read("first", 1, 200)})
        report = observer.read_ledger.report()
        self.assertEqual((report["read_calls"], report["returned_lines"], report["unique_returned_lines"],
                          report["repeated_lines"], report["duplicate_completions"]), (4, 362, 311, 51, 1))
        self.assertEqual(report["sources"], [{"path": source["path"], "source_sha256": source["sha256"],
                                             "ranges": [[1, 300], [400, 410]]}])

    def test_failed_unbound_inconsistent_and_missing_results_do_not_claim_coverage(self):
        ledger = ReadLedger([{"path": "src/value.py", "sha256": "a" * 64}])
        failed = read("failed", 1, 10)
        failed["result"]["isError"] = True
        missing = read("missing", 1, 10)
        missing["result"] = None
        inconsistent = read("inconsistent", 1, 10)
        inconsistent["result"]["structured_content"]["end_line"] = 20
        gaps = read("gaps", 1, 10)
        gaps["result"].pop("content")
        gaps["result"]["structured_content"]["lines"].pop(3)
        for item in (failed, missing, inconsistent, gaps, read("unbound", 1, 10, path="private.py")):
            ledger.started(item)
            ledger.observe(item)
        ledger.started(read("unfinished", 11, 20))
        report = ledger.report()
        self.assertEqual((report["read_calls"], report["failed_reads"], report["unverifiable_reads"]), (6, 1, 5))
        self.assertEqual(report["pending_read_calls"], 1)
        self.assertEqual(report["unique_returned_lines"], 0)
        self.assertEqual(report["sources"], [])
        self.assertFalse(report["range_accounting_complete"])

    def test_partial_reply_counts_only_returned_lines_and_accepts_text_only_transport(self):
        ledger = ReadLedger([{"path": "src/value.py", "sha256": "a" * 64}])
        first = read("first", 1, 200)
        first["arguments"]["end_line"] = 400
        first["result"]["structured_content"] = None
        ledger.observe(first)
        ledger.observe(read("next", 201, 400))
        report = ledger.report()
        self.assertEqual(report["unique_returned_lines"], 400)
        self.assertEqual(report["repeated_lines"], 0)
        self.assertTrue(report["range_accounting_complete"])
        self.assertEqual(report["sources"][0]["ranges"], [[1, 400]])
