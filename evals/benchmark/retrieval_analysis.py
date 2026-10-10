"""Summarize bound read ledgers without turning missing observations into zero."""

from pathlib import Path
import argparse
import sys

from . import analysis, artifacts, campaign, review
from .review_io import Root
from .tools import READ_LINE_LIMIT


COUNTERS = ("read_calls", "failed_reads", "unverifiable_reads", "pending_read_calls",
            "duplicate_completions", "returned_lines", "unique_returned_lines", "repeated_lines")
RANGE_COUNTERS = ("returned_lines", "unique_returned_lines", "repeated_lines")


def validate_ledger(value, sources):
    def require(condition):
        if not condition:
            raise artifacts.BenchmarkError("retrieval_ledger_invalid", "invalid or unbound returned-range ledger")

    require(isinstance(value, dict) and type(value.get("schema_version")) is int and value["schema_version"] == 1
            and value.get("scope") == "broker_reported_ranges_in_one_frozen_trial")
    require(all(type(value.get(name)) is int and value[name] >= 0 for name in COUNTERS))
    require(value["failed_reads"] + value["unverifiable_reads"] <= value["read_calls"]
            and value["pending_read_calls"] <= value["unverifiable_reads"]
            and value["unique_returned_lines"] <= value["returned_lines"]
            and value["returned_lines"] <= READ_LINE_LIMIT * (value["read_calls"] - value["failed_reads"] - value["unverifiable_reads"])
            and value["repeated_lines"] == value["returned_lines"] - value["unique_returned_lines"])
    require(type(value.get("range_accounting_complete")) is bool
            and value["range_accounting_complete"] == (value["unverifiable_reads"] == 0))
    bound = {item["path"]: item for item in sources}
    require(isinstance(value.get("sources"), list))
    seen, unique = set(), 0
    for item in value["sources"]:
        require(isinstance(item, dict) and isinstance(item.get("path"), str))
        path = item["path"]
        require(path in bound and path not in seen and item.get("source_sha256") == bound[path]["sha256"])
        seen.add(path)
        ranges = item.get("ranges")
        require(isinstance(ranges, list) and bool(ranges))
        previous = -1
        for span in ranges:
            require(isinstance(span, list) and len(span) == 2 and all(type(line) is int for line in span))
            first, last = span
            require(1 <= first <= last <= bound[path]["lines"] and first > previous + 1)
            unique += last - first + 1
            previous = last
    require(unique == value["unique_returned_lines"])
    native = value.get("native_delivery")
    if native is not None:
        require(isinstance(native, dict) and set(native) == {"read_calls", "reused_lines"}
                and type(native["read_calls"]) is int and 1 <= native["read_calls"] <= value["read_calls"]
                and type(native["reused_lines"]) is int and native["reused_lines"] >= 0
                and native["reused_lines"] <= sum(item["lines"] for item in sources) * native["read_calls"])
    return value


def summarize(rows, conditions):
    result = {}
    for name in conditions:
        slots = [row for row in rows if row["condition"] == name]
        metrics = {}
        for metric in COUNTERS:
            observed = [row["read_ledger"][metric] for row in slots if row["read_ledger"] is not None]
            complete = sum(row["read_ledger"] is not None and row["status"] == "completed"
                and row["source_integrity"] == "verified"
                and (metric not in RANGE_COUNTERS or row["read_ledger"]["range_accounting_complete"])
                for row in slots)
            total = sum(observed) if observed else None
            metrics[metric] = {"observed_total": total, "total": total if complete == len(slots) else None,
                "reported_attempts": len(observed), "complete_attempts": complete,
                "unknown_attempts": len(slots) - complete}
        native_metrics = {}
        for metric in ("read_calls", "reused_lines"):
            observed = [row["read_ledger"]["native_delivery"][metric] for row in slots
                        if row["read_ledger"] is not None and "native_delivery" in row["read_ledger"]]
            complete = sum(row["read_ledger"] is not None and "native_delivery" in row["read_ledger"]
                and row["status"] == "completed" and row["source_integrity"] == "verified"
                and row["read_ledger"]["range_accounting_complete"] for row in slots)
            total = sum(observed) if observed else None
            native_metrics[metric] = {"observed_total": total, "total": total if complete == len(slots) else None,
                "reported_attempts": len(observed), "unknown_attempts": len(slots) - complete}
        result[name] = {"attempts": analysis.attempt_counts(slots), "metrics": metrics, "native_delivery":native_metrics}
    return result


def summarize_campaign(path):
    plan, batches = campaign.load(path)
    rows = []
    for identifier, batch, _ in batches:
        with Root(batch) as root:
            _, slots, _, _, sources, _, _ = review.collect_batch(root)
            for slot in slots:
                result = root.json(slot["trial_id"] + "/result.json", optional=True)
                adapter = result["diagnostics"].get("adapter", {}) if result else {}
                ledger = adapter.get("read_ledger") if isinstance(adapter, dict) else None
                # A readable result with unavailable source bytes cannot bind range coverage.
                if ledger is not None and slot["source_integrity"] == "verified":
                    validate_ledger(ledger, sources)
                else:
                    ledger = None
                rows.append(dict(slot, case=identifier, read_ledger=ledger))
            root.recheck()
    return {"kind": "mastermind-retrieval-accounting", "schema_version": 1,
        "campaign_sha256": artifacts.digest(plan), "planned_attempts": plan["planned_attempts"],
        "denominator": "all_planned_attempts", "by_condition": summarize(rows, plan["conditions"]),
        "attempts": [{key: row[key] for key in ("case", "condition", "repetition", "trial_id",
                    "status", "source_integrity", "manifest_sha256", "result_sha256", "read_ledger")}
                     for row in rows],
        "quality_uplift": None,
        "limitations": ["Counts broker-reported delivery, not semantic relevance or source truth.",
            "Missing ledgers and incomplete attempts retain unknown totals and observed partial counts.",
            "Fewer returned lines do not establish a better answer or lower model token use."]}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("campaign", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    try:
        value = summarize_campaign(args.campaign)
        if args.output:
            artifacts.write_new(args.output, value)
        else:
            print(artifacts.canonical(value).decode())
        return 0
    except (artifacts.BenchmarkError, OSError, ValueError, TypeError, KeyError) as error:
        print(f"{getattr(error, 'code', 'retrieval_accounting_error')}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
