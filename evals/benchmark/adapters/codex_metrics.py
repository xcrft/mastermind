"""Reported output accounting and client-observed tool intervals for Codex."""

import math


def output_breakdown(usage):
    output, reasoning = (usage.get(name) for name in ("output_tokens", "reasoning_output_tokens"))
    valid = (type(output) is int and type(reasoning) is int
             and 0 <= reasoning <= output)
    return {"reasoning_output_tokens": reasoning if valid else None,
            "non_reasoning_output_tokens": output - reasoning if valid else None,
            "status": "reported" if valid else "unavailable" if reasoning is None else "invalid",
            "scope": "reasoning_is_included_in_total_output_tokens"}


class ToolTimeline:
    def __init__(self):
        self.calls = {}
        self.duplicate_completions = 0

    def observe(self, kind, item, seconds):
        identifier, tool = item.get("id"), item.get("tool")
        if not isinstance(identifier, str) or not identifier or not isinstance(tool, str):
            identifier = "unidentified-" + str(len(self.calls))
        call = self.calls.setdefault(identifier, {"id": identifier, "tool": tool,
            "start_seconds": None, "end_seconds": None, "status": None, "invalid": False})
        if call["end_seconds"] is not None or call["status"] is not None:
            if kind == "item.completed":
                self.duplicate_completions += 1
            return
        if call["tool"] != tool or not _seconds(seconds):
            call["invalid"] = True
        elif kind == "item.started":
            if call["start_seconds"] is None:
                call["start_seconds"] = seconds
            else:
                call["invalid"] = True
        elif kind == "item.completed":
            call["end_seconds"] = seconds
        if kind == "item.completed":
            call["status"] = item.get("status", "unreported")

    def report(self, command_seconds):
        intervals, calls = [], []
        valid_horizon = _seconds(command_seconds)
        for call in self.calls.values():
            start, end = call["start_seconds"], call["end_seconds"]
            valid = (not call["invalid"] and valid_horizon and _seconds(start)
                     and _seconds(end) and start <= end <= command_seconds)
            duration = end - start if valid else None
            if valid:
                intervals.append((start, end))
            calls.append({key: call[key] for key in ("id", "tool", "start_seconds", "end_seconds", "status")}
                         | {"elapsed_seconds": duration})
        union, previous_end = 0, 0
        for start, end in sorted(intervals):
            union += max(0, end - max(start, previous_end))
            previous_end = max(previous_end, end)
        complete = valid_horizon and len(intervals) == len(calls)
        return {"schema_version": 1, "scope": "client_observed_tool_event_intervals",
            "command_seconds": command_seconds if valid_horizon else None,
            "calls": calls, "call_count": len(calls), "unknown_spans": len(calls) - len(intervals),
            "duplicate_completions": self.duplicate_completions, "accounting_complete": complete,
            "observed_tool_span_seconds": sum(end - start for start, end in intervals),
            "observed_tool_interval_union_seconds": union,
            "tool_interval_union_seconds": union if complete else None,
            "outside_tool_intervals_seconds": command_seconds - union if complete else None,
            "limitations": "Event receipt includes client and transport overhead. Outside intervals include "
                "startup, model work, network and queuing; neither measure isolates native execution or reasoning time."}


def _seconds(value):
    try:
        return type(value) in (int, float) and math.isfinite(value) and value >= 0
    except OverflowError:
        return False
