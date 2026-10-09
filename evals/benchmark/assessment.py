"""Evidence-backed reviewer declarations and immutable review receipts."""

from __future__ import annotations

import re

from . import review_contracts
from . import intent
from .review_io import sha
from evals.benchmark import artifacts as artifact_io
from evals.benchmark import corpus


def check_assessment(value, seal, packet, answers, sources):
    review_contracts.fields(value, ("kind", "schema_version", "export_id", "packet_sha256", "reviewer", "reviews"))
    review_contracts.require(value["kind"] == "mastermind-research-assessment" and type(value["schema_version"]) is int
            and value["schema_version"] in (1, 2, 3), "unsupported assessment")
    has_acceptance = "acceptance_criteria" in packet["rubric"]
    review_contracts.require((value["schema_version"] == 3) == has_acceptance,
                            "assessment version does not match the frozen acceptance contract", "review_outcome")
    review_contracts.require(value["export_id"] == seal["export_id"] and value["packet_sha256"] == seal["packet_sha256"],
            "assessment belongs to a different or changed export", "review_identity")
    review_contracts.require(isinstance(value["reviewer"], str) and re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", value["reviewer"]),
            "reviewer must be a stable lowercase label")
    review_contracts.require(isinstance(value["reviews"], list) and len(value["reviews"]) == len(answers),
            "assessment must account for every retained answer", "review_inventory")
    seen = set()
    for item in value["reviews"]:
        review_contracts.fields(item, ("review_id", "answer_sha256", "rubric_sha256", "claims", "knowns", "unknowns",
                      *(("outcome",) if value["schema_version"] >= 2 else ()),
                      *(("acceptance",) if has_acceptance else ())))
        review_id = item["review_id"]
        review_contracts.require(isinstance(review_id, str) and review_id in answers and review_id not in seen, "unknown or duplicate reviewed answer", "review_inventory")
        seen.add(review_id)
        answer = answers[review_id]
        review_contracts.require(item["answer_sha256"] == sha(answer.encode()) and item["rubric_sha256"] == packet["rubric_sha256"],
                "assessment answer or key changed", "review_identity")
        review_contracts.require(isinstance(item["claims"], list) and 1 <= len(item["claims"]) <= 64, "review needs bounded claim-level evidence")
        quotes = set()
        for claim in item["claims"]:
            review_contracts.fields(claim, ("quote", "support", "anchors", "material_error", "rationale"))
            quote = review_contracts.text(claim["quote"])
            review_contracts.require(quote in answer and quote not in quotes, "claim must quote a distinct passage of the retained answer", "review_quote")
            quotes.add(quote)
            review_contracts.require(claim["support"] in ("supported", "unsupported", "contradicted", "unknown")
                    and type(claim["material_error"]) is bool, "invalid claim assessment")
            review_contracts.require(not claim["material_error"] or claim["support"] in ("unsupported", "contradicted"), "material errors need unsupported or contradicted claims")
            review_contracts.text(claim["rationale"])
            review_contracts.require(isinstance(claim["anchors"], list) and len(claim["anchors"]) <= 16, "invalid claim evidence")
            for anchor in claim["anchors"]:
                review_contracts.text(anchor, 1024)
            review_contracts.require(len(set(claim["anchors"])) == len(claim["anchors"]), "duplicate claim evidence")
            review_contracts.require(claim["anchors"] or claim["support"] in ("unsupported", "unknown"), "supported or contradicted claims require source anchors")
            corpus.validate_anchors(claim["anchors"], sources)
        for name, key, expected, field, choices, absent in (
                ("knowns", "known_index", packet["rubric"]["required_knowns"], "coverage", ("covered", "partial", "missing"), "missing"),
                ("unknowns", "unknown_index", packet["rubric"]["expected_unknowns"], "handling", ("appropriate", "overclaimed", "omitted"), "omitted")):
            rows = item[name]
            review_contracts.require(isinstance(rows, list) and len(rows) == len(expected), "assessment omits required rubric dimensions", "review_inventory")
            indexes = set()
            for row in rows:
                review_contracts.fields(row, (key, field, "answer_excerpt", "rationale"))
                index = row[key]
                review_contracts.require(type(index) is int and 0 <= index < len(expected) and index not in indexes, "invalid or duplicate rubric index")
                indexes.add(index)
                review_contracts.require(row[field] in choices, "rubric assessment is unfinished or invalid")
                review_contracts.text(row["rationale"])
                if row[field] == absent:
                    review_contracts.require(row["answer_excerpt"] is None, "absent evidence cannot claim an answer excerpt")
                else:
                    review_contracts.require(review_contracts.text(row["answer_excerpt"]) in answer, "rubric excerpt is absent from the answer", "review_quote")
        if value["schema_version"] >= 2:
            review_contracts.fields(item["outcome"], ("status", "rationale"))
            review_contracts.require(item["outcome"]["status"] in ("satisfied", "unsatisfied", "unknown"), "invalid task outcome")
            review_contracts.text(item["outcome"]["rationale"])
        if has_acceptance:
            intent.validate_assessment(item["acceptance"], packet["rubric"]["acceptance_criteria"], answer)
            failed = (any(row["status"] == "unmet" for row in item["acceptance"])
                      or any(claim["material_error"] for claim in item["claims"])
                      or any(row["handling"] == "overclaimed" for row in item["unknowns"]))
            unresolved = any(row["status"] == "unknown" for row in item["acceptance"])
            expected = "unsatisfied" if failed else "unknown" if unresolved else "satisfied"
            review_contracts.require(item["outcome"]["status"] == expected,
                                     "task outcome conflicts with the acceptance evidence", "review_outcome")
        elif value["schema_version"] == 2 and item["outcome"]["status"] == "satisfied":
            review_contracts.require(all(row["coverage"] == "covered" for row in item["knowns"])
                        and all(row["handling"] == "appropriate" for row in item["unknowns"])
                        and all(claim["support"] == "supported" and not claim["material_error"] for claim in item["claims"]),
                        "satisfied task conflicts with its evidence assessment", "review_outcome")


def receipt_names(root):
    if "reviews" not in root.names():
        return set()
    entries = root.names("reviews")
    # An interrupted exclusive publication may leave its unlinked staging file.
    # It is not an admitted assessment and is never read or sent to a reviewer.
    receipts = {name for name in entries if not re.fullmatch(r"\.pending-[0-9a-f]{32}", name)}
    review_contracts.require(len(receipts) <= 64, "too many review receipts", "review_limit")
    review_contracts.require(all(re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}\.json", name) for name in receipts),
            "unexpected review receipt file")
    return receipts


def read_assessments(root, seal, packet, answers, sources):
    assessments = []
    for name in sorted(receipt_names(root)):
        receipt = root.json("reviews/" + name)
        review_contracts.fields(receipt, ("kind", "schema_version", "export_id", "packet_sha256", "coordinator_sha256", "assessment_sha256",
                         "assessment", "semantics", "comparison_accepted", "quality_uplift"))
        review_contracts.require(receipt["kind"] == "mastermind-research-assessment-receipt" and type(receipt["schema_version"]) is int
                and receipt["schema_version"] == 1 and receipt["export_id"] == seal["export_id"]
                and receipt["packet_sha256"] == seal["packet_sha256"]
                and receipt["coordinator_sha256"] == seal["coordinator_sha256"]
                and receipt["assessment_sha256"] == artifact_io.digest(receipt["assessment"])
                and receipt["comparison_accepted"] is False and receipt["quality_uplift"] is None
                and receipt["semantics"] == "reviewer_declared_not_machine_verified", "review receipt changed", "review_identity")
        check_assessment(receipt["assessment"], seal, packet, answers, sources)
        review_contracts.require(name == receipt["assessment"]["reviewer"] + ".json", "reviewer identity differs from its receipt", "review_identity")
        assessments.append(receipt["assessment"])
    return assessments
