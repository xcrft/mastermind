"""Observed tool time and optional reasoning token accounting."""

import unittest

from evals.benchmark.adapters import codex_metrics


def event(identifier, *, tool="source_read", status="completed"):
    return {"id": identifier, "tool": tool, "status": status}


class CodexMetricsTests(unittest.TestCase):
    def test_overlapping_calls_count_wall_intervals_once_and_ignore_duplicate_completion(self):
        timeline = codex_metrics.ToolTimeline()
        timeline.observe("item.started", event("a"), 2)
        timeline.observe("item.started", event("b", tool="mmcg_search"), 3)
        timeline.observe("item.updated", event("a"), 4)
        timeline.observe("item.completed", event("a"), 5)
        timeline.observe("item.completed", event("b", tool="mmcg_search", status="failed"), 7)
        timeline.observe("item.completed", event("b"), 9)
        report = timeline.report(10)
        self.assertTrue(report["accounting_complete"])
        self.assertEqual(report["call_count"], 2)
        self.assertEqual(report["duplicate_completions"], 1)
        self.assertEqual(report["observed_tool_span_seconds"], 7)
        self.assertEqual(report["tool_interval_union_seconds"], 5)
        self.assertEqual(report["outside_tool_intervals_seconds"], 5)
        self.assertEqual([call["elapsed_seconds"] for call in report["calls"]], [3, 4])
        self.assertEqual(report["calls"][1]["status"], "failed")

    def test_missing_start_pending_invalid_clock_and_identity_preserve_unknown_time(self):
        timeline = codex_metrics.ToolTimeline()
        timeline.observe("item.updated", event("no-start"), 1)
        timeline.observe("item.completed", event("no-start"), 2)
        timeline.observe("item.started", event("pending"), 3)
        timeline.observe("item.started", event("reverse"), 4)
        timeline.observe("item.completed", event("reverse"), 3)
        timeline.observe("item.started", event("foreign"), 4)
        timeline.observe("item.completed", event("foreign", tool="mmcg_search"), 5)
        timeline.observe("item.started", event("invalid"), True)
        timeline.observe("item.completed", event("invalid"), 5)
        timeline.observe("item.started", event("late"), 8)
        timeline.observe("item.completed", event("late"), 11)
        timeline.observe("item.started", event("valid"), 6)
        timeline.observe("item.completed", event("valid"), 7)
        report = timeline.report(10)
        self.assertFalse(report["accounting_complete"])
        self.assertEqual(report["unknown_spans"], 6)
        self.assertEqual(report["observed_tool_interval_union_seconds"], 1)
        self.assertIsNone(report["tool_interval_union_seconds"])
        self.assertIsNone(report["outside_tool_intervals_seconds"])
        for call in report["calls"][:-1]:
            self.assertIsNone(call["elapsed_seconds"])
        for horizon in (None, False, float("inf"), 10**1000):
            with self.subTest(horizon=horizon):
                unavailable = timeline.report(horizon)
                self.assertFalse(unavailable["accounting_complete"])
                self.assertIsNone(unavailable["command_seconds"])
                self.assertIsNone(unavailable["outside_tool_intervals_seconds"])

    def test_reasoning_is_an_optional_subset_of_output_and_never_imputed(self):
        usage = {"output_tokens": 12, "reasoning_output_tokens": 9}
        measured = codex_metrics.output_breakdown(usage)
        self.assertEqual(measured["reasoning_output_tokens"], 9)
        self.assertEqual(measured["non_reasoning_output_tokens"], 3)
        self.assertEqual(usage["output_tokens"], 12)
        for supplied in ({"output_tokens": 12}, {"output_tokens": 12, "reasoning_output_tokens": -1},
                         {"output_tokens": 12, "reasoning_output_tokens": 13},
                         {"output_tokens": 12, "reasoning_output_tokens": True},
                         {"reasoning_output_tokens": 1}):
            with self.subTest(usage=supplied):
                measured = codex_metrics.output_breakdown(supplied)
                self.assertIsNone(measured["reasoning_output_tokens"])
                self.assertIsNone(measured["non_reasoning_output_tokens"])
        measured = codex_metrics.output_breakdown({"output_tokens": 12, "reasoning_output_tokens": 0})
        self.assertEqual(measured["reasoning_output_tokens"], 0)
        self.assertEqual(measured["non_reasoning_output_tokens"], 12)
