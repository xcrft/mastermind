"""Lookup accounting binds query identity, limits and native source locations."""

import copy
import unittest

from evals.benchmark import artifacts, lookup, retrieval_analysis
from evals.benchmark.retrieval import ReadLedger


SOURCES = [{"path": "src/value.rs", "lines": 4, "sha256": "a" * 64}]
FOUND = {"query": "value", "collapse_partials": True, "total": 2,
         "count": 1, "truncated": True, "results": [{"name": "value", "file": "src/value.rs", "line": 2}],
         "precision_notes": ["name_based"]}
MISSING = {"query": "absent", "collapse_partials": True, "total": 0,
           "count": 0, "truncated": False, "results": [], "precision_notes": []}


def event(identifier, args, body=None, *, kind="item.completed", failed=False):
    item = {"type": "mcp_tool_call", "server": "research", "tool": "mmcg_search",
            "id": identifier, "arguments": args, "status": "failed" if failed else "completed",
            "error": "failed" if failed else None}
    if body is not None:
        item["result"] = {"structured_content": body}
    return {"type": kind, "item": item}


def stream(*events):
    return b"".join(artifacts.canonical(item) + b"\n" for item in events)


class LookupTests(unittest.TestCase):
    def test_single_and_batch_keep_explicit_limits_missing_names_and_collisions(self):
        batch = {"query_count": 2, "truncated": True, "queries": [FOUND, MISSING]}
        completed = event("batch", {"names": ["value", "absent"], "top": 1}, batch)
        ledger = lookup.from_stream(stream(
            event("first", None, kind="item.started"),
            event("first", {"name": "value", "top": 1}, FOUND),
            completed, completed, event("last", {"name": "absent"}, MISSING)), SOURCES)
        self.assertEqual({k: ledger[k] for k in ("lookup_calls", "single_calls", "batch_calls",
            "requested_names", "returned_matches", "truncated_queries", "duplicate_completions")},
            dict(lookup_calls=3, single_calls=2, batch_calls=1, requested_names=4,
                 returned_matches=2, truncated_queries=2, duplicate_completions=1))
        self.assertTrue(ledger["lookup_accounting_complete"])
        self.assertEqual([(r["requested_top"], r["effective_top"]) for r in ledger["requests"]],
                         [(1, 1), (1, 1), (None, 100)])
        implicit_batch = lookup.from_stream(stream(event("b", {"names": ["absent"]},
            {"query_count": 1, "truncated": False, "queries": [MISSING]})), SOURCES)
        self.assertEqual(implicit_batch["requests"][0]["effective_top"], 10)

    def test_request_changes_pending_calls_and_failed_replies_remain_distinct(self):
        ledger = lookup.from_stream(stream(
            event("changed", {"name": "value"}, kind="item.started"),
            event("changed", {"name": "absent"}, kind="item.updated"),
            event("changed", {"name": "value"}, FOUND),
            event("pending", {"name": "value"}, kind="item.started"),
            event("failed", {"name": "value"}, failed=True)), SOURCES)
        self.assertEqual((ledger["lookup_calls"], ledger["failed_calls"],
                          ledger["unverifiable_calls"], ledger["pending_calls"]), (3, 1, 2, 1))
        self.assertEqual(ledger["returned_matches"], 0)
        self.assertFalse(ledger["lookup_accounting_complete"])
        missing_id = lookup.from_stream(stream(event(None, {}, kind="item.started")), SOURCES)
        self.assertFalse(missing_id["lookup_accounting_complete"])

    def test_corrupted_or_foreign_batch_metadata_never_becomes_complete_accounting(self):
        batch = {"query_count": 2, "truncated": True, "queries": [FOUND, MISSING]}
        mutations = []
        for key, value in [("query_count", True), ("truncated", False)]:
            mutations.append(dict(batch, **{key: value}))
        for key, value in [("query", "other"), ("count", True), ("total", 0),
                           ("collapse_partials", 1), ("truncated", 1),
                           ("results", [{"file": "elsewhere.rs", "line": 1}]),
                           ("results", [{"file": "src/value.rs", "line": 5}]),
                           ("results", [{"file": "src/value.rs", "line": 2, "locations": {}}]),
                           ("results", [{"file": "src/value.rs", "line": 2,
                                         "locations": [{"file": "elsewhere.rs", "line": 1}]}])]:
            changed = copy.deepcopy(batch)
            changed["queries"][0][key] = value
            mutations.append(changed)
        mutations.append(dict(batch, queries=[MISSING, FOUND]))
        for value in mutations:
            with self.subTest(value=value):
                ledger = lookup.from_stream(stream(event("b", {"names": ["value", "absent"], "top": 1}, value)), SOURCES)
                self.assertEqual((ledger["unverifiable_calls"], ledger["returned_matches"]), (1, 0))
                self.assertFalse(ledger["lookup_accounting_complete"])
        truncated_stream = stream(event("ok", {"name": "value", "top": 1}, FOUND)) + b'{"type":'
        ledger = lookup.from_stream(truncated_stream, SOURCES)
        self.assertEqual(ledger["returned_matches"], 1)
        self.assertFalse(ledger["lookup_accounting_complete"])

    def test_all_attempt_totals_keep_legacy_and_failed_observations_unknown(self):
        ledger = lookup.from_stream(stream(event("ok", {"name": "value", "top": 1}, FOUND)), SOURCES)
        rows = [dict(condition="candidate", status=status, source_integrity="verified",
            answer_sha256=None, read_ledger=ReadLedger(SOURCES).report(), lookup_ledger=observed)
            for status, observed in [("completed", ledger), ("completed", None), ("model_error", ledger)]]
        metrics = retrieval_analysis.summarize(rows, ["candidate"])["candidate"]["lookup"]
        self.assertEqual(metrics["lookup_calls"]["observed_total"], 2)
        self.assertIsNone(metrics["lookup_calls"]["total"])
        self.assertEqual(metrics["lookup_calls"]["unknown_attempts"], 2)
