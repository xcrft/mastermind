"""Task-local accounting of source ranges reported by completed tool calls."""

from . import artifacts
from .tools import READ_LINE_LIMIT


class ReadLedger:
    def __init__(self, source_files):
        self.sources = {item["path"]: item["sha256"] for item in source_files}
        self.ranges = {}
        self.completed = set()
        self.pending = set()
        self.read_calls = self.failed_reads = self.unknown_reads = 0
        self.returned_lines = self.unique_lines = self.duplicate_completions = 0

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
            body = self._body(result)
            path = body["path"]
            if path not in self.sources or path != args.get("path"):
                raise ValueError("unbound source")
            lines = body["lines"]
            start, end = body["start_line"], body["end_line"]
            total = body["total_lines"]
            if start is None and end is None and total == 0 and lines == []:
                return
            if (type(start) is not int or type(end) is not int or type(total) is not int
                    or not 1 <= start <= end <= total or end - start >= READ_LINE_LIMIT
                    or start != args.get("start_line", 1)
                    or ("end_line" in args and (type(args["end_line"]) is not int or end > args["end_line"]))
                    or not isinstance(lines, list) or len(lines) != end - start + 1
                    or any(not isinstance(row, dict) or type(row.get("line")) is not int
                           or row["line"] != start + index or not isinstance(row.get("text"), str)
                           for index, row in enumerate(lines))):
                raise ValueError("invalid returned range")
        except (KeyError, TypeError, ValueError, artifacts.BenchmarkError):
            self.unknown_reads += 1
            return
        previous = self.ranges.get(path, [])
        merged = []
        for left, right in sorted([*previous, (start, end)]):
            if merged and left <= merged[-1][1] + 1:
                merged[-1] = (merged[-1][0], max(merged[-1][1], right))
            else:
                merged.append((left, right))
        self.returned_lines += len(lines)
        self.unique_lines += sum(right - left + 1 for left, right in merged) - sum(
            right - left + 1 for left, right in previous)
        self.ranges[path] = merged

    @staticmethod
    def _body(result):
        if not isinstance(result, dict):
            raise ValueError("missing result")
        bodies = [result[key] for key in ("structured_content", "structuredContent") if result.get(key) is not None]
        texts = [part["text"] for part in result.get("content", [])
                 if isinstance(part, dict) and part.get("type") == "text" and isinstance(part.get("text"), str)]
        if len(texts) == 1:
            bodies.append(artifacts.parse_json(texts[0].encode()))
        if not bodies or not isinstance(bodies[0], dict) or any(body != bodies[0] for body in bodies[1:]):
            raise ValueError("inconsistent tool result")
        return bodies[0]

    def report(self):
        return {"schema_version": 1, "scope": "broker_reported_ranges_in_one_frozen_trial",
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
