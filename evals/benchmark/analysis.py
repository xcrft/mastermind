"""Paired task outcomes and resource accounting across every planned attempt."""

from __future__ import annotations

import math

from . import review_contracts
from evals.benchmark import conditions as condition_contract


def validate_resources(value, *, complete=False):
    review_contracts.fields(value, review_contracts.CORE_RESOURCE_FIELDS, review_contracts.TIMING_FIELDS)
    for name, amount in value.items():
        if amount is not None:
            review_contracts.require(type(amount) is int if name in (*review_contracts.TOKEN_FIELDS, "turns") else type(amount) in (int, float),
                    "invalid resource measurement")
            review_contracts.require(amount >= 0 and (type(amount) is int or math.isfinite(amount)), "invalid resource measurement")
    if complete:
        review_contracts.require(all(value[name] is not None for name in (*review_contracts.TOKEN_FIELDS, "turns")) and value["turns"] >= 1,
                "passed runtime contract lacks complete telemetry")


def trial_measurements(manifest, result):
    resources = dict.fromkeys(review_contracts.RESOURCE_FIELDS)
    resources["setup_seconds"] = manifest.get("setup_seconds")
    contract = "unknown"
    if result is not None:
        diagnostic = result["diagnostics"]
        review_contracts.require(isinstance(diagnostic, dict), "invalid result diagnostics")
        resources["run_seconds"] = diagnostic.get("elapsed_seconds")
        measured = diagnostic.get("telemetry")
        if isinstance(measured, dict):
            usage = measured.get("usage")
            review_contracts.require(isinstance(usage, dict), "invalid token telemetry")
            resources.update({name: usage.get(name) for name in review_contracts.TOKEN_FIELDS})
            resources.update({name: measured.get(name) for name in ("turns", "cost_usd")})
            resources.update({name: measured.get("timings", {}).get(name) for name in review_contracts.TIMING_FIELDS})
            exceeded = measured.get("budget_exceeded")
            unexpected = diagnostic.get("unexpected_tools")
            review_contracts.require(type(measured.get("complete")) is bool and isinstance(exceeded, list)
                    and isinstance(unexpected, list), "invalid runtime contract diagnostics")
            if exceeded or unexpected:
                contract = "failed"
            elif measured["complete"]:
                contract = "passed"
        if result["run_status"]["state"] != "completed":
            contract = "failed"
    validate_resources(resources, complete=contract == "passed")
    return resources, contract


def resource_summary(coordinator):
    names = condition_contract.condition_names(coordinator.get("batch", {"schema_version": 1}))
    summary = []
    for condition in names:
        slots = [slot for slot in coordinator["slots"] if slot["condition"] == condition]
        metrics = {}
        for name in review_contracts.RESOURCE_FIELDS:
            values = [slot.get("resources", {}).get(name) for slot in slots]
            observed = [value for value in values if value is not None]
            total = sum(observed)
            review_contracts.require(type(total) is int or math.isfinite(total), "resource sum exceeds numeric bounds", "review_limit")
            metrics[name] = {"observed_total": total if observed else None, "measured_trials": len(observed),
                             "unknown_trials": len(values) - len(observed), "complete": len(observed) == len(values)}
        summary.append({"condition": condition, "planned": len(slots), "metrics": metrics})
    return {"scope": "trial_setup_and_adapter_attempts", "by_condition": summary}


def attempt_counts(slots):
    counts = dict(planned=len(slots), completed=0, failed=0, not_run=0, unfinished=0, missing_artifacts=0, with_answer=0)
    for slot in slots:
        state = slot["status"]
        counts[state if state in ("completed", "not_run", "unfinished", "missing_artifacts") else "failed"] += 1
        counts["with_answer"] += slot["answer_sha256"] is not None
    return counts


def assessment_summary(coordinator, assessments):
    def counts(condition=None):
        retained = sum(slot["answer_sha256"] is not None
                       and (condition is None or slot["condition"] == condition)
                       for slot in coordinator["slots"])
        return {"retained_answers": retained, "answer_assessments": 0,
                "task_outcomes": {name: 0 for name in ("satisfied", "unsatisfied", "unknown")},
                "reviewer_selected_claims": {name: 0 for name in (
                    "supported", "unsupported", "contradicted", "unknown")},
                "material_error_claims": 0, "answer_assessments_with_material_error": 0,
                "known_coverage": {name: 0 for name in ("covered", "partial", "missing")},
                "request_criteria": {name: 0 for name in ("met", "unmet", "unknown")},
                "unknown_handling": {name: 0 for name in ("appropriate", "overclaimed", "omitted")}}

    overall = counts()
    names = condition_contract.condition_names(coordinator.get("batch", {"schema_version": 1}))
    conditions = {condition: counts(condition) for condition in names}
    slots = {slot["review_id"]: slot for slot in coordinator["slots"] if slot["answer_sha256"] is not None}
    aligned = {review_id: [] for review_id in slots}
    for assessment in assessments:
        for item in assessment["reviews"]:
            slot = slots[item["review_id"]]
            targets = (overall, conditions[slot["condition"]])
            for target in targets:
                target["answer_assessments"] += 1
                target["task_outcomes"][item.get("outcome", {"status": "unknown"})["status"]] += 1
                for claim in item["claims"]:
                    target["reviewer_selected_claims"][claim["support"]] += 1
                    target["material_error_claims"] += claim["material_error"]
                target["answer_assessments_with_material_error"] += any(
                    claim["material_error"] for claim in item["claims"])
                for known in item["knowns"]:
                    target["known_coverage"][known["coverage"]] += 1
                for unknown in item["unknowns"]:
                    target["unknown_handling"][unknown["handling"]] += 1
                for criterion in item.get("acceptance", []):
                    target["request_criteria"][criterion["status"]] += 1
            aligned[item["review_id"]].append({
                "material_error": any(claim["material_error"] for claim in item["claims"]),
                "outcome": item.get("outcome", {"status": "unknown"})["status"],
                "knowns": {row["known_index"]: row["coverage"] for row in item["knowns"]},
                "unknowns": {row["unknown_index"]: row["handling"] for row in item["unknowns"]},
            })
    disagreement = {"answers_compared": 0, "answers_with_disagreement": 0,
                    "material_error_flags_with_disagreement": 0,
                    "task_outcomes_with_disagreement": 0,
                    "known_dimensions_with_disagreement": 0,
                    "unknown_dimensions_with_disagreement": 0}
    if len(assessments) >= 2:
        for records in aligned.values():
            disagreement["answers_compared"] += 1
            material = len({record["material_error"] for record in records}) > 1
            outcome = len({record["outcome"] for record in records}) > 1
            known = sum(len({record["knowns"][index] for record in records}) > 1
                        for index in records[0]["knowns"])
            unknown = sum(len({record["unknowns"][index] for record in records}) > 1
                          for index in records[0]["unknowns"])
            disagreement["material_error_flags_with_disagreement"] += material
            disagreement["task_outcomes_with_disagreement"] += outcome
            disagreement["known_dimensions_with_disagreement"] += known
            disagreement["unknown_dimensions_with_disagreement"] += unknown
            disagreement["answers_with_disagreement"] += material or outcome or known > 0 or unknown > 0
    return {"semantics": "descriptive_reviewer_declarations", "reviewers": len(assessments),
            "overall": overall, "by_condition": [dict(condition=name, **conditions[name])
                                                   for name in names],
            "disagreement": disagreement}


def outcome_interval(slot, assessment):
    """Task success includes failed attempts; unobserved outcomes remain unknown."""
    if slot["status"] in review_contracts.RUN_STATES - {"completed"}:
        return (0, 0), "failed_attempt"
    if slot.get("runtime_contract") == "failed":
        return (0, 0), "failed_runtime_contract"
    if slot.get("runtime_contract") != "passed":
        return (0, 1), "unverified_runtime_contract"
    if slot["status"] != "completed" or slot["source_integrity"] != "verified":
        return (0, 1), "unverified_attempt"
    outcome = assessment.get("outcome", {"status": "unknown"}) if assessment else {"status": "unknown"}
    if outcome["status"] == "unknown":
        return (0, 1), "unreviewed_or_unknown"
    success = int(outcome["status"] == "satisfied")
    return (success, success), "reviewer_declared_" + outcome["status"]


def paired_comparison(coordinator, assessment, baseline, candidate, *, outcome_basis=None):
    reviews = {item["review_id"]: item for item in assessment["reviews"]} if assessment else {}
    slots = {(slot["repetition"], slot["condition"]): slot for slot in coordinator["slots"]}
    repetitions = sorted({slot["repetition"] for slot in coordinator["slots"]})
    pairs = []
    totals = {condition: [0, 0] for condition in (baseline, candidate)}
    resolved, wins, ties, losses = 0, 0, 0, 0
    for repetition in repetitions:
        pair = {"repetition": repetition}
        intervals = {}
        for condition, label in ((baseline, "baseline"), (candidate, "candidate")):
            slot = slots[(repetition, condition)]
            interval, reason = outcome_interval(slot, reviews.get(slot["review_id"]))
            intervals[condition] = interval
            for index in (0, 1):
                totals[condition][index] += interval[index]
            pair[label] = {"review_id": slot["review_id"], "run_status": slot["status"],
                           "success": {"lower": interval[0], "upper": interval[1]}, "reason": reason}
        lower = intervals[candidate][0] - intervals[baseline][1]
        upper = intervals[candidate][1] - intervals[baseline][0]
        pair["success_delta"] = {"lower": lower, "upper": upper}
        if lower == upper:
            resolved += 1
            wins += lower > 0
            ties += lower == 0
            losses += lower < 0
        pairs.append(pair)
    count = len(pairs)
    lower = sum(pair["success_delta"]["lower"] for pair in pairs) / count
    upper = sum(pair["success_delta"]["upper"] for pair in pairs) / count
    return {"reviewer": assessment["reviewer"] if assessment else None,
            "outcome_basis": outcome_basis or (
                "user_acceptance" if assessment and assessment.get("schema_version") == 3 else "full_source_key"),
            "planned_pairs": count, "resolved_pairs": resolved, "wins": wins, "ties": ties, "losses": losses,
            "success_rate": {condition: {"lower": values[0] / count, "upper": values[1] / count}
                             for condition, values in totals.items()},
            "success_delta": {"lower": lower, "upper": upper, "point": lower if lower == upper else None},
            "pairs": pairs}
