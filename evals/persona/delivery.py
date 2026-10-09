"""Inspect retained profile delivery without inferring model adherence."""

from pathlib import Path
import argparse
import hashlib
import json


def compare(expected, packet):
    if not isinstance(expected, list) or not isinstance(packet, dict):
        raise ValueError("expected rules must be a list and the packet an object")
    wanted = {}
    for row in expected:
        if (not isinstance(row, dict) or set(row) != {"key", "statement"}
                or any(not isinstance(row[field], str) or not row[field].strip() for field in row)):
            raise ValueError("each expected rule needs a nonempty key and statement")
        if row["key"] in wanted:
            raise ValueError("duplicate expected rule")
        wanted[row["key"]] = row["statement"]
    feedback = packet.get("feedback", [])
    if not isinstance(feedback, list):
        raise ValueError("profile feedback must be a list")
    received, revisions = {}, {}
    for row in feedback:
        if (not isinstance(row, dict) or not isinstance(row.get("key"), str) or not row["key"].strip()
                or not isinstance(row.get("statement"), str) or not row["statement"].strip() or row.get("status") != "active"
                or row["key"] in received):
            raise ValueError("invalid, unaccepted or duplicate returned rule")
        received[row["key"]] = row["statement"]
        revisions[row["key"]] = row.get("review_revision")
    if received and packet.get("status") != "ok":
        raise ValueError("a non-ok profile packet cannot expose personal rules")
    unchanged = sorted(key for key in wanted if received.get(key) == wanted[key])
    return {
        "kind": "mastermind-profile-delivery-inspection", "schema_version": 1,
        "reported_status": packet.get("status"),
        "reported_source_verification": packet.get("source_verification"),
        "selection": packet.get("selection"),
        "profile_revision": packet.get("profile_revision"), "store_revision": packet.get("store_revision"),
        "expected_rules": len(wanted), "returned_rules": len(received),
        "unchanged_expected_rules": unchanged,
        "missing_expected_rules": sorted(set(wanted) - set(received)),
        "changed_expected_rules": sorted(key for key in wanted.keys() & received.keys() if wanted[key] != received[key]),
        "unexpected_rules": sorted(set(received) - set(wanted)),
        "review_revisions": revisions, "omitted": packet.get("omitted"),
        "exact_text_delivery_fraction": len(unchanged) / len(wanted) if wanted else None,
        "current_source_reverification": "not_performed",
        "model_application": None, "semantic_quality": None, "comparison_accepted": False,
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--expected", type=Path, required=True, help="selected rule keys and statements as JSON")
    parser.add_argument("--packet", type=Path, required=True, help="retained mmcg_profile response JSON")
    parser.add_argument("--output", type=Path, required=True, help="new private result path")
    args = parser.parse_args(argv)
    expected, packet = args.expected.read_bytes(), args.packet.read_bytes()
    result = compare(json.loads(expected), json.loads(packet))
    result["input_sha256"] = {"expected": hashlib.sha256(expected).hexdigest(), "packet": hashlib.sha256(packet).hexdigest()}
    with args.output.open("x", encoding="utf-8") as handle:
        json.dump(result, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
