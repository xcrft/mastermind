"""Run every case in a source-current corpus through the same experiment."""

from pathlib import Path
import argparse
import copy
import hashlib
import os
import sys

from . import artifacts as artifact_io
from . import batch as batch_execution
from . import conditions, corpus, efficiency, effort, review, trials
from . import runtime as runtime_identity
from .review_io import Root


ATTEMPT_LIMIT = 240


def prepare(*, corpus_path, config, source_repo, tool_repo, output, repetitions=3):
    checked = corpus.check_corpus(corpus_path, source_repo, require_current=True)
    names = conditions.condition_names({"schema_version": 3, "conditions": config["conditions"]}) if "conditions" in config else conditions.CONDITIONS
    if "calibration" in config:
        conditions.validate_calibration(config["calibration"], config.get("conditions"))
    if type(repetitions) is not int or not 1 <= repetitions <= 20:
        raise artifact_io.BenchmarkError("invalid_repetitions", "use 1..20 repetitions")
    count = len(checked["cases"]) * len(names) * repetitions
    if count > ATTEMPT_LIMIT or len(names) * repetitions > conditions.TRIAL_LIMIT:
        raise artifact_io.BenchmarkError("campaign_limit", "campaign exceeds its attempt cap")
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    frozen = copy.deepcopy(config)
    if "mmcg" in frozen:
        pin = runtime_identity.runtime_pin(frozen["mmcg"], "mmcg", frozen["tool_revision"])
        directory = output / "runtimes"
        directory.mkdir(mode=0o700)
        body = artifact_io.read_file(Path(pin["path"]), artifact_io.BINARY_BYTE_LIMIT)
        if hashlib.sha256(body).hexdigest() != pin["sha256"]:
            raise artifact_io.BenchmarkError("campaign_runtime_changed", "native binary changed while being frozen")
        artifact_io.write_new_bytes(directory / "mmcg", body, mode=0o555)
        frozen["mmcg"]["path"] = str((directory / "mmcg").resolve())
    plan = {"kind": "mastermind-research-campaign", "schema_version": 1,
        "corpus": checked, "config_sha256": artifact_io.digest(config), "model": config["model"],
        "conditions": list(names), "repetitions": repetitions, "planned_attempts": count,
        "balanced_positions": repetitions % len(names) == 0, "cases": []}
    if "calibration" in config:
        plan["calibration"] = copy.deepcopy(config["calibration"])
    for entry in checked["cases"]:
        case = corpus.select_case(corpus_path, entry["id"], source_repo)
        case_config = corpus.configure_case(case, frozen)
        routing = None
        if "effort_policy" in case_config:
            case_config, routing = effort.configure(case["task"], case_config)
        batch = trials.prepare_batch(task=case["task"], rubric=case["rubric"],
            config=case_config, source_repo=source_repo, tool_repo=tool_repo,
            output=output / entry["id"], repetitions=repetitions, corpus_case=case["summary"])
        plan["cases"].append({"id": entry["id"], "batch": batch.relative_to(output.resolve()).as_posix(),
            "batch_sha256": artifact_io.digest(artifact_io.load_json(batch / "batch.json"))})
        if routing is not None:
            plan["cases"][-1]["effort_routing"] = routing
    artifact_io.write_new(output / "campaign.json", plan)
    return plan


def check_matrix(plan, batches):
    for batch in batches:
        batch_execution.validate_batch_summary(batch, allow_legacy=True, code="campaign_inventory")
    if (not batches or type(plan.get("repetitions")) is not int
            or not 1 <= plan["repetitions"] <= 20
            or type(plan.get("planned_attempts")) is not int
            or not 1 <= plan["planned_attempts"] <= ATTEMPT_LIMIT
            or not isinstance(plan.get("conditions"), list)
            or any(plan["conditions"] != list(conditions.condition_names(batch))
                   or plan["repetitions"] != batch["repetitions"]
                   or plan.get("calibration") != batch.get("calibration") for batch in batches)
            or type(plan.get("balanced_positions")) is not bool
            or plan["balanced_positions"] != (plan["repetitions"] % len(plan["conditions"]) == 0)
            or sum(len(batch["trials"]) for batch in batches) != plan["planned_attempts"]):
        raise artifact_io.BenchmarkError("campaign_inventory", "campaign matrix differs from its bound batches")


def load(path):
    path = path.resolve(strict=True)
    plan = artifact_io.load_json(path / "campaign.json")
    expected = {row["id"] for row in plan["corpus"]["cases"]}
    if (plan.get("kind") != "mastermind-research-campaign" or type(plan.get("schema_version")) is not int
            or plan["schema_version"] != 1
            or len(plan["cases"]) != len(expected) or {row["id"] for row in plan["cases"]} != expected):
        raise artifact_io.BenchmarkError("campaign_inventory", "campaign must retain every corpus case")
    batches = []
    for case in plan["cases"]:
        batch = path / conditions.safe_source_path(case["batch"])
        value = artifact_io.load_json(batch / "batch.json")
        if artifact_io.digest(value) != case["batch_sha256"] or value["task_id"] != case["id"]:
            raise artifact_io.BenchmarkError("campaign_changed", "campaign batch identity changed")
        batches.append((case["id"], batch, value))
    check_matrix(plan, [value for _, _, value in batches])
    return plan, batches


def run(path, *, progress=print, credentials=None):
    plan, batches = load(path)
    if any(item["status"] != "prepared" for _, _, value in batches for item in value["trials"]):
        raise artifact_io.BenchmarkError("campaign_setup_failed", "retain this failed preparation and create a corrected complete campaign before inference")
    statuses = []
    for identifier, batch, value in batches:
        with Root(batch) as root:
            _, slots, _, _, _, _, _ = review.collect_batch(root)
        states = {slot["trial_id"]: slot["status"] for slot in slots}
        for item in value["trials"]:
            trial = batch / item["directory"]
            if states[item["directory"]] == "not_run":
                progress(f"Run {identifier} {item['condition']} repetition {item['repetition'] + 1}", flush=True)
                result = trials.run_trial(trial, credentials)
            else:
                if states[item["directory"]] in ("unfinished", "missing_artifacts"):
                    raise artifact_io.BenchmarkError("campaign_unfinished", "an incomplete attempt cannot be retried selectively")
                result = artifact_io.load_json(trial / "result.json")
            statuses.append({"case": identifier, "condition": item["condition"],
                "repetition": item["repetition"], "status": result["run_status"]["state"]})
            progress(f"Result: {statuses[-1]['status']}", flush=True)
            if result["run_status"] == {"state": "identity_mismatch", "reason": "unavailable_tool_called"}:
                try:
                    trials.verify_prepared(trial, artifact_io.load_json(trial / "manifest.json"))
                except artifact_io.BenchmarkError as error:
                    raise artifact_io.BenchmarkError("campaign_runtime_invalid", "prepared inputs changed after the tool violation") from error
                continue
            if result["run_status"]["state"] in ("input_changed", "identity_mismatch", "setup_error"):
                raise artifact_io.BenchmarkError("campaign_runtime_invalid", "runtime or prepared input changed; remaining attempts stay not_run")
    return {"planned_attempts": plan["planned_attempts"], "attempts": statuses}


def export(path, output):
    plan, batches = load(path)
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    entries = []
    for identifier, batch, value in batches:
        destination = output / identifier
        review.export_review(batch, destination)
        entries.append({"id": identifier, "directory": identifier, "batch": value})
    result = {"kind": "mastermind-campaign-review-set", "schema_version": 1,
        "campaign_sha256": artifact_io.digest(plan), "campaign": plan, "cases": entries}
    artifact_io.write_new(output / "review-set.json", result)
    return result


def compare(path, baseline, candidate):
    value = artifact_io.load_json(path / "review-set.json")
    plan = value["campaign"]
    if (value.get("kind") != "mastermind-campaign-review-set" or value.get("schema_version") != 1
            or artifact_io.digest(plan) != value["campaign_sha256"]
            or len(value["cases"]) != len(plan["cases"])
            or {row["id"] for row in value["cases"]} != {row["id"] for row in plan["cases"]}):
        raise artifact_io.BenchmarkError("campaign_inventory", "review set omits or replaces a planned case")
    check_matrix(plan, [case["batch"] for case in value["cases"]])
    reports, unreviewed = [], []
    expected = {row["id"]: row for row in plan["cases"]}
    keys = {row["id"]: row for row in plan["corpus"]["cases"]}
    for case in value["cases"]:
        if case["directory"] != case["id"]:
            raise artifact_io.BenchmarkError("campaign_inventory", "review directory must match the case ID")
        directory = path / conditions.safe_source_path(case["directory"])
        with Root(directory) as root:
            _, packet, coordinator, _, _ = review.load_export(root)
            if (artifact_io.digest(case["batch"]) != expected[case["id"]]["batch_sha256"]
                    or coordinator["batch_sha256"] != hashlib.sha256(artifact_io.canonical(case["batch"]) + b"\n").hexdigest()
                    or packet["rubric_sha256"] != keys[case["id"]]["rubric_sha256"]
                    or artifact_io.digest(packet["task"]) != keys[case["id"]]["task_sha256"]):
                raise artifact_io.BenchmarkError("campaign_changed", "review batch, task or key differs from the planned case")
            report = review.compare_review(directory, baseline, candidate)
            basis = "user_acceptance" if "acceptance_criteria" in packet["rubric"] else "full_source_key"
            unreviewed.append(efficiency.compare(coordinator, None, baseline, candidate, outcome_basis=basis))
            root.recheck()
        if report["task_id"] != case["id"] or report["model"] != plan["model"]:
            raise artifact_io.BenchmarkError("campaign_changed", "review belongs to another task or model")
        reports.append(report)
    reviewers = {row["reviewer"] for report in reports for row in report["efficiency"]} - {None}
    totals = []
    for reviewer in sorted(reviewers) if reviewers else [None]:
        rows = [next((row for row in report["efficiency"] if row["reviewer"] == reviewer),
            dict(unknown, reviewer=reviewer)) for report, unknown in zip(reports, unreviewed)]
        total = efficiency.aggregate(rows, baseline, candidate)
        total["reviewed_cases"] = sum(any(row["reviewer"] == reviewer for row in report["efficiency"])
            for report in reports) if reviewer is not None else 0
        totals.append(total)
    return {"kind": "mastermind-campaign-comparison", "schema_version": 1,
        "corpus_sha256": plan["corpus"]["corpus_sha256"], "model": plan["model"],
        "baseline": baseline, "candidate": candidate, "efficiency": totals,
        "independent_task_count": len(reports), "planned_attempts": plan["planned_attempts"],
        "balanced_positions": plan["balanced_positions"], "cases": reports,
        "comparison_accepted": False, "quality_uplift": None,
        "scope": "public_current_source_calibrations", "population_generalization": "not_established"}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    make = commands.add_parser("prepare")
    make.add_argument("--corpus", type=Path, default=corpus.DEFAULT_CORPUS)
    make.add_argument("--config", type=Path, required=True)
    make.add_argument("--source-repo", type=Path, required=True)
    make.add_argument("--tool-repo", type=Path, required=True)
    make.add_argument("--output", type=Path, required=True)
    make.add_argument("--repetitions", type=int, default=3)
    execute = commands.add_parser("run")
    execute.add_argument("campaign", type=Path)
    execute.add_argument("--credential-env", action="append", choices=sorted(runtime_identity.CREDENTIAL_NAMES), default=[])
    packet = commands.add_parser("export")
    packet.add_argument("campaign", type=Path)
    packet.add_argument("--output", type=Path, required=True)
    report = commands.add_parser("compare")
    report.add_argument("review_set", type=Path)
    report.add_argument("--baseline", required=True)
    report.add_argument("--candidate", required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "prepare":
            result = prepare(corpus_path=args.corpus, config=artifact_io.load_json(args.config),
                source_repo=args.source_repo.resolve(), tool_repo=args.tool_repo.resolve(),
                output=args.output.resolve(), repetitions=args.repetitions)
        elif args.command == "run":
            credentials = {name: os.environ[name] for name in args.credential_env if name in os.environ}
            if len(credentials) != len(set(args.credential_env)):
                raise artifact_io.BenchmarkError("credentials_missing", "a requested credential variable is absent")
            result = run(args.campaign, credentials=credentials)
        elif args.command == "export":
            result = export(args.campaign, args.output.resolve())
        else:
            result = compare(args.review_set.resolve(), args.baseline, args.candidate)
        print(artifact_io.canonical(result).decode())
        return 0
    except (artifact_io.BenchmarkError, OSError, ValueError, TypeError, KeyError) as error:
        print(f"{getattr(error, 'code', 'campaign_error')}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
