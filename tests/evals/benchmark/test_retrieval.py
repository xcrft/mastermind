"""Returned-range accounting with overlaps, continuations and unknown results."""

import unittest
import copy

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
    def test_native_receipts_count_only_new_text_and_reject_unobserved_reuse(self):
        ledger = ReadLedger([{"path":"src/value.py", "sha256":"a"*64}])

        def native(identifier, start, end, spans, reused, receipt_ranges, receipt, previous=None):
            segments = [{"start_line":a,"end_line":b, "lines":[{"line":n,"text":f"line {n}"} for n in range(a,b+1)]} for a,b in spans]
            ranges = lambda values: [{"start_line":a,"end_line":b} for a,b in values]
            body = {"path":"src/value.py", "source_sha256":"a"*64, "total_lines":500,
                "requested_start_line":start,"requested_end_line":end, "segments":segments,
                "reused_ranges":ranges(reused), "receipt_ranges":ranges(receipt_ranges), "receipt":receipt,
                "reuse_status":"applied" if previous else "not_requested", "range_truncated":False,"next_line":None}
            args = {"path":"src/value.py", "start_line":start,"end_line":end}
            if previous: args["previous_receipt"] = previous
            return {"tool":"source_read", "id":identifier,"status":"completed", "arguments":args,
                "result":{"structuredContent":body}}

        first = native("first",1,3,[(1,3)],[],[(1,3)],"1"*32)
        second = native("second",3,6,[(4,6)],[(3,3)],[(1,6)],"2"*32,"1"*32)
        repeat = native("repeat",1,6,[],[(1,6)],[(1,6)],"3"*32,"2"*32)
        for item in (first,second,repeat): ledger.observe(item)
        report = ledger.report()
        self.assertEqual((report["returned_lines"],report["unique_returned_lines"],report["repeated_lines"]),(6,6,0))
        self.assertEqual(report["native_delivery"],{"read_calls":3,"reused_lines":7})
        self.assertTrue(report["range_accounting_complete"])
        for mutate in (lambda body: body.update(source_sha256="b"*64),
                       lambda body: body["segments"][0]["lines"].pop(),
                       lambda body: body.update(receipt_ranges=[{"start_line":1,"end_line":500}])):
            other = ReadLedger([{"path":"src/value.py", "sha256":"a"*64}])
            bad = copy.deepcopy(first)
            mutate(bad["result"]["structuredContent"])
            other.observe(bad)
            self.assertEqual(other.report()["unique_returned_lines"],0)
            self.assertFalse(other.report()["range_accounting_complete"])
        unknown = ReadLedger([{"path":"src/value.py", "sha256":"a"*64}])
        unknown.observe(repeat)
        self.assertEqual(unknown.report()["unverifiable_reads"],1)
        self.assertEqual(unknown.report()["native_delivery"]["reused_lines"],0)
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
