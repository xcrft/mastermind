"""Useful outcomes per measured resource, preserving unknowns and failures."""

import math

from . import analysis, review_contracts


RESOURCE_COMPONENTS = {
    "context_tokens": ("input_tokens", "cache_read_tokens", "cache_write_tokens"),
    "total_tokens": (*review_contracts.TOKEN_FIELDS,),
    "run_seconds": ("run_seconds",),
    "trial_total_seconds": ("setup_seconds", "run_seconds"),
    "first_message_seconds": ("first_message_seconds",),
    "final_answer_seconds": ("final_answer_seconds",),
    "cost_usd": ("cost_usd",),
}


def distribution(values):
    """Observed sample quantiles with linear interpolation, not population estimates."""
    ordered = sorted(values)
    def percentile(fraction):
        if not ordered:
            return None
        position = (len(ordered) - 1) * fraction
        lower, upper = math.floor(position), math.ceil(position)
        try:
            return ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower)
        except OverflowError:
            review_contracts.require(False, "resource quantile exceeds numeric bounds", "review_limit")
    return {"observed_attempts": len(ordered), "p50": percentile(0.5), "p95": percentile(0.95),
            "scope": "observed_attempts_only"}


def resource_totals(slots):
    result = {}
    for name, components in RESOURCE_COMPONENTS.items():
        measured = []
        for slot in slots:
            values = [slot.get("resources", {}).get(part) for part in components]
            if all(value is not None for value in values):
                value = sum(values)
                review_contracts.require(type(value) is int or math.isfinite(value), "resource sum overflow", "review_limit")
                measured.append(value)
        complete = len(measured) == len(slots)
        total = sum(measured) if measured else None
        review_contracts.require(total is None or type(total) is int or math.isfinite(total), "resource total overflow", "review_limit")
        result[name] = {"total": total if complete else None,
            "observed_total": total,
            "observed_values": measured, "distribution": distribution(measured),
            "measured_attempts": len(measured), "unknown_attempts": len(slots) - len(measured),
            "complete": complete}
    return result


def per_success(total, success):
    if total is None or success["lower"] != success["upper"]:
        return {"value": None, "reason": "incomplete_measurement"}
    if success["lower"] == 0:
        return {"value": None, "reason": "no_successful_outcomes"}
    return {"value": total / success["lower"], "reason": None}


def outcome_yield(total, success):
    if total is None or success["lower"] != success["upper"]:
        return {"value": None, "reason": "incomplete_measurement"}
    if total == 0:
        return {"value": None, "reason": "zero_measured_resource"}
    return {"value": success["lower"] / total, "reason": None}


def outcome_yield_bounds(total, success):
    values = {bound: outcome_yield(total, {"lower": success[bound], "upper": success[bound]})
              for bound in ("lower", "upper")}
    return {bound: values[bound]["value"] for bound in values} | {"reason": values["lower"]["reason"]}


def yield_gain_bounds(baseline, candidate):
    if baseline["reason"] or candidate["reason"]:
        return {"lower": None, "upper": None, "reason": "incomplete_measurement"}
    if baseline["lower"] == 0:
        return {"lower": None, "upper": None, "reason": "possible_zero_baseline_yield"}
    return {"lower": candidate["lower"] / baseline["upper"] - 1,
            "upper": candidate["upper"] / baseline["lower"] - 1, "reason": None}


def condition_summary(attempts, success, resources):
    return {"planned_attempts": attempts, "successful_outcomes": success, "resources": resources,
        "resource_per_success": {metric: per_success(value["total"], success) for metric, value in resources.items()},
        "useful_outcomes_per_resource": {metric: outcome_yield(value["total"], success) for metric, value in resources.items()},
        "useful_outcomes_per_resource_bounds": {metric: outcome_yield_bounds(value["total"], success)
                                               for metric, value in resources.items()}}


def contrasts(conditions, baseline, candidate):
    contrasts = {}
    for metric in RESOURCE_COMPONENTS:
        left, right = (conditions[name]["resources"][metric]["total"] for name in (baseline, candidate))
        a, b = (conditions[name]["resource_per_success"][metric]["value"] for name in (baseline, candidate))
        u, v = (conditions[name]["useful_outcomes_per_resource"][metric]["value"] for name in (baseline, candidate))
        contrasts[metric] = {
            "resource_savings": 1 - right / left if left is not None and left > 0 and right is not None else None,
            "resource_per_success_savings": 1 - b / a if a is not None and a > 0 and b is not None else None,
            "useful_outcomes_per_resource_gain": v / u - 1 if u is not None and u > 0 and v is not None else None,
            "useful_outcomes_per_resource_delta": v - u if u is not None and v is not None else None,
            "useful_outcomes_per_resource_gain_bounds": yield_gain_bounds(
                conditions[baseline]["useful_outcomes_per_resource_bounds"][metric],
                conditions[candidate]["useful_outcomes_per_resource_bounds"][metric]),
        }
    return contrasts


def compare(coordinator, assessment, baseline, candidate, *, outcome_basis=None):
    paired = analysis.paired_comparison(coordinator, assessment, baseline, candidate, outcome_basis=outcome_basis)
    count = paired["planned_pairs"]
    conditions = {}
    for name in (baseline, candidate):
        slots = [slot for slot in coordinator["slots"] if slot["condition"] == name]
        label = "baseline" if name == baseline else "candidate"
        success = {bound: sum(pair[label]["success"][bound] for pair in paired["pairs"]) for bound in ("lower", "upper")}
        conditions[name] = condition_summary(len(slots), success, resource_totals(slots))
    quality_preserved = (paired["resolved_pairs"] == count and paired["losses"] == 0
        and paired["success_delta"]["point"] is not None and paired["success_delta"]["point"] >= 0)
    return {"reviewer": paired["reviewer"], "denominator": "all_planned_attempts",
        "outcome_basis": paired["outcome_basis"],
        "known_quality_losses": paired["losses"],
        "quality_preserved_on_resolved_pairs": quality_preserved,
        "success_delta": paired["success_delta"], "by_condition": conditions, "contrasts": contrasts(conditions, baseline, candidate),
        "value": value_status(conditions, paired["success_delta"], quality_preserved, baseline, candidate, paired["losses"]),
        "semantics": "descriptive_efficiency_on_this_task", "population_generalization": "not_established",
        "statistical_confidence_interval": None, "independent_task_count": 1,
        "limitations": ["Repeated attempts of one task are not independent task samples.",
            "Unknown resources and outcomes prevent a point efficiency estimate.",
            "Yield bounds describe possible unresolved outcomes, not statistical confidence or independent review.",
            "Subscription billing cost is unknown unless reported by the runtime.",
            "Reviewer declarations do not establish reviewer independence or causal benefit."]}


def value_status(conditions, success_delta, quality_preserved, baseline, candidate, known_losses):
    """Savings can support useful-work observations only after the outcome gate."""
    success = [conditions[name]["successful_outcomes"] for name in (baseline, candidate)]
    if known_losses:
        status = "quality_regression"
    elif any(row["lower"] != row["upper"] for row in success):
        status = "unresolved_quality"
    elif not quality_preserved:
        status = "quality_regression"
    elif any(row["lower"] == 0 for row in success):
        status = "no_successful_baseline" if success[0]["lower"] == 0 else "no_successful_candidate"
    else:
        status = "reviewed_outcomes_preserved"
    measured = contrasts(conditions, baseline, candidate)
    savings = {metric: row["resource_savings"] if status == "reviewed_outcomes_preserved" else None
               for metric, row in measured.items()}
    return {"status": status, "success_delta": success_delta,
            "quality_gated_resource_savings": savings,
            "semantics": "descriptive_reviewed_sample_not_product_qualification"}


def aggregate(measurements, baseline, candidate):
    """Sum one reviewer's case records without converting missing costs to zero."""
    review_contracts.require(bool(measurements) and len({row["reviewer"] for row in measurements}) == 1,
        "aggregate needs the same reviewer across every case", "review_identity")
    bases = {row.get("outcome_basis", "full_source_key") for row in measurements}
    review_contracts.require(len(bases) == 1, "cannot pool different outcome contracts", "review_outcome")
    conditions = {}
    for name in (baseline, candidate):
        rows = [row["by_condition"][name] for row in measurements]
        resources = {}
        for metric in RESOURCE_COMPONENTS:
            values = [row["resources"][metric] for row in rows]
            observed = [value["observed_total"] for value in values if value["observed_total"] is not None]
            total = sum(observed) if observed else None
            review_contracts.require(total is None or type(total) is int or math.isfinite(total),
                "resource total overflow", "review_limit")
            complete = all(value["complete"] for value in values)
            observations = [item for value in values for item in value.get("observed_values", [])]
            resources[metric] = {"total": total if complete else None, "observed_total": total,
                "observed_values": observations, "distribution": distribution(observations),
                "complete": complete, "measured_attempts": sum(value["measured_attempts"] for value in values),
                "unknown_attempts": sum(value["unknown_attempts"] for value in values)}
        success = {bound: sum(row["successful_outcomes"][bound] for row in rows) for bound in ("lower", "upper")}
        conditions[name] = condition_summary(sum(row["planned_attempts"] for row in rows), success, resources)
    a, b = (conditions[name]["successful_outcomes"] for name in (baseline, candidate))
    count = conditions[baseline]["planned_attempts"]
    lower, upper = (b["lower"] - a["upper"]) / count, (b["upper"] - a["lower"]) / count
    delta = {"lower": lower, "upper": upper, "point": lower if lower == upper else None}
    preserved = all(row["quality_preserved_on_resolved_pairs"] for row in measurements)
    known_losses = sum(row["known_quality_losses"] for row in measurements)
    return {"reviewer": measurements[0]["reviewer"], "denominator": "all_planned_attempts",
        "outcome_basis": next(iter(bases)),
        "known_quality_losses": known_losses,
        "by_condition": conditions, "contrasts": contrasts(conditions, baseline, candidate),
        "success_delta": delta,
        "quality_preserved_on_resolved_pairs": preserved,
        "value": value_status(conditions, delta, preserved, baseline, candidate, known_losses),
        "independent_task_count": len(measurements), "statistical_confidence_interval": None,
        "semantics": "descriptive_efficiency_on_this_corpus", "population_generalization": "not_established",
        "limitations": measurements[0]["limitations"]}
