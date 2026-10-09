"""Acceptance criteria bound to the original request, separate from source coverage."""

import re

from . import review_contracts as contract


def validate_criteria(task, criteria):
    contract.require(isinstance(criteria, list) and 1 <= len(criteria) <= 12,
                     "acceptance needs 1..12 criteria", "corpus_acceptance")
    seen = set()
    for row in criteria:
        contract.fields(row, ("id", "request_excerpt", "criterion"))
        identifier = row["id"]
        contract.require(isinstance(identifier, str) and re.fullmatch(r"[a-z][a-z0-9_]{0,47}", identifier)
                         and identifier not in seen, "invalid or duplicate acceptance criterion", "corpus_acceptance")
        seen.add(identifier)
        excerpt = contract.text(row["request_excerpt"])
        contract.require(any(excerpt in task[field] for field in ("question", "output_contract")),
                         "acceptance criterion is not bound to the original request", "corpus_acceptance")
        contract.text(row["criterion"])


def validate_assessment(rows, criteria, answer):
    contract.require(isinstance(rows, list) and len(rows) == len(criteria),
                     "assessment omits user acceptance criteria", "review_inventory")
    expected = {row["id"] for row in criteria}
    seen = set()
    for row in rows:
        contract.fields(row, ("criterion_id", "status", "answer_excerpt", "rationale"))
        identifier = row["criterion_id"]
        contract.require(isinstance(identifier, str) and identifier in expected and identifier not in seen,
                         "unknown or duplicate acceptance criterion", "review_inventory")
        seen.add(identifier)
        contract.require(row["status"] in ("met", "unmet", "unknown"), "invalid acceptance status")
        contract.text(row["rationale"])
        excerpt = row["answer_excerpt"]
        contract.require(row["status"] != "met" or excerpt is not None,
                         "met acceptance criterion needs answer evidence", "review_quote")
        if excerpt is not None:
            contract.require(contract.text(excerpt) in answer,
                             "acceptance excerpt is absent from the retained answer", "review_quote")
