"""Task-local accounting of source ranges reported by completed tool calls."""

from . import artifacts
from .mcp import result_body
from .tools import READ_LINE_LIMIT, READ_DEFAULT_LINES


class ReadLedger:
    def __init__(self, source_files):
        self.sources = {item["path"]: item["sha256"] for item in source_files}
        self.ranges = {}
        self.completed = set()
        self.pending = set()
        self.read_calls = self.failed_reads = self.unknown_reads = 0
        self.returned_lines = self.unique_lines = self.duplicate_completions = 0
        self.receipts = {}
        self.native_calls = self.reused_lines = 0

    @staticmethod
    def _is_read(item):
        args = item.get("arguments")
        return item.get("tool") == "source_read" or (item.get("tool") == "source_git"
            and isinstance(args, dict) and args.get("operation") == "show")

    def started(self, item):
        identifier = item.get("id")
        if self._is_read(item) and isinstance(identifier, str) and identifier and identifier not in self.completed:
            self.pending.add(identifier)

    def observe(self, item):
        if not self._is_read(item):
            return
        identifier = item.get("id")
        if not isinstance(identifier, str) or not identifier:
            self.read_calls += 1
            self.unknown_reads += 1
            return
        if identifier in self.completed:
            self.duplicate_completions += 1
            return
        self.completed.add(identifier)
        self.pending.discard(identifier)
        self.read_calls += 1
        args = item.get("arguments")
        if not isinstance(args, dict):
            self.unknown_reads += 1
            return
        result = item.get("result")
        if item.get("status") == "failed" or item.get("error") is not None or (
                isinstance(result, dict) and result.get("isError") is True):
            self.failed_reads += 1
            return
        try:
            body = result_body(result)
            path = body["path"]
            if path not in self.sources or path != args.get("path"):
                raise ValueError("unbound source")
            if "segments" in body:
                self.native_calls += 1
                spans, reused = self._native_spans(body, args, path)
            else:
                spans, reused = self._legacy_spans(body, args), 0
        except (KeyError, TypeError, ValueError, artifacts.BenchmarkError):
            self.unknown_reads += 1
            return
        self.reused_lines += reused
        for start, end in spans:
            previous = self.ranges.get(path, [])
            merged = self._merge([*previous, (start, end)])
            self.returned_lines += end - start + 1
            self.unique_lines += self._size(merged) - self._size(previous)
            self.ranges[path] = merged

    @staticmethod
    def _legacy_spans(body, args):
        lines = body["lines"]
        start, end = body["start_line"], body["end_line"]
        total = body["total_lines"]
        if start is None and end is None and total == 0 and lines == []:
            return []
        if (type(start) is not int or type(end) is not int or type(total) is not int
                or not 1 <= start <= end <= total or end - start >= READ_LINE_LIMIT
                or start != args.get("start_line", 1)
                or ("end_line" in args and (type(args["end_line"]) is not int or end > args["end_line"]))
                or not isinstance(lines, list) or len(lines) != end - start + 1
                or any(not isinstance(row, dict) or type(row.get("line")) is not int
                       or row["line"] != start + index or not isinstance(row.get("text"), str)
                       for index, row in enumerate(lines))):
            raise ValueError("invalid returned range")
        return [(start, end)]

    @staticmethod
    def _merge(spans):
        merged = []
        for left, right in sorted(spans):
            if merged and left <= merged[-1][1] + 1:
                merged[-1] = (merged[-1][0], max(merged[-1][1], right))
            else:
                merged.append((left, right))
        return merged

    @staticmethod
    def _size(spans):
        return sum(right - left + 1 for left, right in spans)

    def _native_spans(self, body, args, path):
        total = body["total_lines"]
        start, requested_end = body["requested_start_line"], body["requested_end_line"]
        if (type(total) is not int or total < 0 or type(start) is not int or start < 1 or type(requested_end) is not int
                or start != args.get("start_line", 1)
                or requested_end != args.get("end_line", min(total, start + READ_DEFAULT_LINES - 1))
                or body["source_sha256"] != self.sources[path]):
            raise ValueError("unbound native source read")
        end = min(total, requested_end)

        def ranges(rows, lower=1, upper=total):
            if not isinstance(rows, list) or len(rows) > 128:
                raise ValueError("invalid ranges")
            result, last = [], 0
            for row in rows:
                a, b = row["start_line"], row["end_line"]
                if type(a) is not int or type(b) is not int or not lower <= a <= b <= upper or a <= last:
                    raise ValueError("invalid native span")
                result.append((a, b))
                last = b
            return result

        segments = body["segments"]
        spans = ranges(segments, start, end)
        for segment, (a, b) in zip(segments, spans):
            lines = segment["lines"]
            if (not isinstance(lines, list) or len(lines) != b-a+1
                    or any(not isinstance(row, dict) or type(row.get("line")) is not int
                           or row["line"] != a+i or not isinstance(row.get("text"), str)
                           for i,row in enumerate(lines))):
                raise ValueError("invalid delivered lines")
        if self._size(spans) > READ_LINE_LIMIT:
            raise ValueError("native delivery exceeds line limit")
        reused = ranges(body["reused_ranges"], start, end)
        prior = self.receipts.get(args.get("previous_receipt"))
        if reused and (body["reuse_status"] != "applied" or prior is None or prior[0] != path
                or any(not any(c <= a <= b <= d for c,d in prior[1]) for a,b in reused)):
            raise ValueError("unverified reuse")
        complete = self._merge([*spans, *reused])
        if self._size(complete) != self._size(spans) + self._size(reused):
            raise ValueError("overlapping delivery and reuse")
        next_line = body["next_line"]
        truncated = body["range_truncated"]
        if type(truncated) is not bool:
            raise ValueError("invalid completion")
        if truncated:
            if type(next_line) is not int or not start <= next_line <= end:
                raise ValueError("invalid continuation")
            expected = [(start, next_line-1)] if next_line > start else []
        else:
            if next_line is not None:
                raise ValueError("unexpected continuation")
            expected = [(start, end)] if total else []
        if complete != expected:
            raise ValueError("unaccounted requested range")
        receipt_ranges = ranges(body["receipt_ranges"])
        observed = self._merge([*(prior[1] if prior is not None and prior[0] == path else []), *spans])
        if any(not any(c <= a <= b <= d for c,d in observed) for a,b in receipt_ranges):
            raise ValueError("receipt claims unread text")
        receipt = body["receipt"]
        if not isinstance(receipt, str) or len(receipt) != 32 or any(c not in "0123456789abcdef" for c in receipt):
            raise ValueError("invalid receipt")
        self.receipts[receipt] = (path, receipt_ranges)
        return spans, self._size(reused)

    def report(self):
        report = {"schema_version": 1, "scope": "broker_reported_ranges_in_one_frozen_trial",
            "read_calls": self.read_calls + len(self.pending), "failed_reads": self.failed_reads,
            "unverifiable_reads": self.unknown_reads + len(self.pending), "pending_read_calls": len(self.pending),
            "duplicate_completions": self.duplicate_completions,
            "returned_lines": self.returned_lines, "unique_returned_lines": self.unique_lines,
            "repeated_lines": self.returned_lines - self.unique_lines,
            "range_accounting_complete": self.unknown_reads == 0 and not self.pending,
            "sources": [{"path": path, "source_sha256": self.sources[path],
                         "ranges": [[left, right] for left, right in ranges]}
                        for path, ranges in sorted(self.ranges.items())],
            "limitations": "Counts reported delivery, not semantic relevance or source truth. Trial integrity remains a separate gate."}
        if self.native_calls:
            report["native_delivery"] = {"read_calls":self.native_calls, "reused_lines":self.reused_lines}
        return report
