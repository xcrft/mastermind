"""Product objectives and current matched measurements from sealed review sets."""

from pathlib import Path
import argparse
import hashlib
import sys

from . import artifacts, campaign


OBJECTIVES = {
    "request_acceptance": {"direction": "higher", "target": 0.9, "unit": "accepted_fraction"},
    "tokens_per_accepted_task": {"direction": "lower", "target": 0.1, "unit": "relative_saving"},
    "total_latency_p50": {"direction": "lower", "target": 0.1, "unit": "relative_saving"},
    "total_latency_p95": {"direction": "lower", "target": 0.1, "unit": "relative_saving"},
    "first_message_p50": {"direction": "lower", "target": 0.1, "unit": "relative_saving"},
    "user_corrections": {"direction": "lower", "target": 0.2, "unit": "relative_saving"},
    "relevant_profile_delivery": {"direction": "higher", "target": 1.0, "unit": "delivered_fraction"},
    "automatic_activation": {"direction": "higher", "target": 1.0, "unit": "successful_fraction"},
}


def saving(left, right):
    return 1 - right / left if left is not None and left > 0 and right is not None else None


def rates(condition):
    count = condition["planned_attempts"]
    return {bound: condition["successful_outcomes"][bound] / count for bound in ("lower", "upper")}


def metric_rows(measurement, baseline, candidate):
    conditions = measurement["by_condition"]
    left, right = (conditions[name] for name in (baseline, candidate))
    basis = measurement["outcome_basis"]
    user_outcomes = basis == "user_acceptance"
    useful = user_outcomes and measurement["value"]["status"] == "reviewed_outcomes_preserved"
    rows = {}
    for name, objective in OBJECTIVES.items():
        row = dict(objective, baseline=None, candidate=None, relative_saving=None,
                   status="unmeasured", target_met=None)
        if name == "request_acceptance" and user_outcomes:
            row.update(baseline=rates(left), candidate=rates(right), status="reviewer_declared")
            interval = row["candidate"]
            if interval["lower"] >= objective["target"]:
                row["target_met"] = measurement["quality_preserved_on_resolved_pairs"]
            elif interval["upper"] < objective["target"]:
                row["target_met"] = False
        elif name == "tokens_per_accepted_task" and user_outcomes:
            row.update(baseline=left["resource_per_success"]["total_tokens"]["value"],
                       candidate=right["resource_per_success"]["total_tokens"]["value"],
                       status=measurement["value"]["status"])
            row["relative_saving"] = saving(row["baseline"], row["candidate"])
        elif name in ("total_latency_p50", "total_latency_p95", "first_message_p50"):
            metric = "first_message_seconds" if name == "first_message_p50" else "trial_total_seconds"
            quantile = "p95" if name.endswith("p95") else "p50"
            resources = [condition["resources"][metric] for condition in (left, right)]
            if all(resource["complete"] for resource in resources):
                row.update(baseline=resources[0]["distribution"][quantile],
                           candidate=resources[1]["distribution"][quantile], status="observed_resource")
                row["relative_saving"] = saving(row["baseline"], row["candidate"])
        if name != "request_acceptance" and row["relative_saving"] is not None:
            row["target_met"] = False if row["relative_saving"] < objective["target"] else True if useful else None
        rows[name] = row
    return rows


def product_report(comparison):
    baseline, candidate = comparison["baseline"], comparison["candidate"]
    return {"kind": "mastermind-product-value", "schema_version": 1,
        "objectives_status": "initial_engineering_targets_not_user_qualification",
        "baseline": baseline, "candidate": candidate,
        "independent_task_count": comparison["independent_task_count"],
        "planned_attempts": comparison["planned_attempts"],
        "measurements": [{"reviewer": row["reviewer"], "outcome_basis": row["outcome_basis"],
            "metrics": metric_rows(row, baseline, candidate), "efficiency": row}
            for row in comparison["efficiency"]],
        "comparison": comparison, "product_benefit_qualified": False,
        "limitations": [
            "Source-key completeness in legacy reviews is not user acceptance.",
            "Quantiles describe observed attempts, not a population or confidence interval.",
            "First message means a completed nonempty CLI assistant message, which may be commentary; it is not TTFT.",
            "Targets cannot be met by resource reductions while request outcomes are unknown or regress.",
            "User corrections, native profile delivery and real-session activation need their own bound observations.",
            "Public calibrations and implementation-agent reviews do not qualify population or causal product benefit."]}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--review-set", type=Path, required=True)
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--output", type=Path, required=True, help="new immutable JSON report")
    args = parser.parse_args(argv)
    try:
        comparison = campaign.compare(args.review_set, args.baseline, args.candidate)
        report = product_report(comparison)
        manifest = args.review_set / "review-set.json"
        report["review_set_sha256"] = hashlib.sha256(artifacts.read_file(manifest)).hexdigest()
        artifacts.write_new(args.output, report)
    except (artifacts.BenchmarkError, OSError, ValueError) as error:
        print(f"{getattr(error, 'code', 'product_report_failed')}: {error}", file=sys.stderr)
        return 2
    print(args.output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
