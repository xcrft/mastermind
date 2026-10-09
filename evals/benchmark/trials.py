"""Prepare and run matched research trials through a pinned adapter."""

from __future__ import annotations

from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import sys
import time
import uuid

from . import artifacts as artifact_io
from . import batch as batch_execution
from . import conditions as condition_contract
from . import protocol as model_protocol
from . import runtime as runtime_identity
from . import source as source_snapshot
from .adapters import claude, codex
from evals.shared import process as process_runner


def prepare_trial(
    *, task: dict, rubric: dict, config: dict, source_repo: Path, tool_repo: Path,
    output: Path, condition: str, repetition: int = 0, trial_id: str | None = None,
    batch_binding: dict | None = None,
) -> Path:
    """Create one independent trial; persist setup failure without invoking a model."""
    model_protocol.validate_task(task)
    conditions = condition_contract.validate_conditions(config["conditions"]) if "conditions" in config else None
    calibration = condition_contract.validate_calibration(config["calibration"], conditions) if "calibration" in config else None
    selected = next((item for item in conditions if item["id"] == condition), None) if conditions else None
    if ((conditions is not None and selected is None)
            or (conditions is None and condition not in condition_contract.CONDITIONS)
            or type(repetition) is not int or repetition < 0):
        raise artifact_io.BenchmarkError("invalid_condition", "invalid condition or repetition")
    if selected is not None and set(selected["instruction_paths"]) & set(task["source_allowlist"]):
        raise artifact_io.BenchmarkError("instruction_source_overlap", "condition instructions cannot be public source evidence")
    model_protocol.validate_rubric(task, rubric)
    limits = model_protocol.validate_limits(config.get("limits", {}))
    model = config.get("model")
    if not isinstance(model, str) or not model or model in {"opus", "sonnet", "haiku", "latest"}:
        raise artifact_io.BenchmarkError("invalid_model", "pin an exact model identity, not an alias")
    tool_revision = artifact_io.exact_revision(config.get("tool_revision"))
    if ((trial_id is None) != (batch_binding is None)
            or (trial_id is not None and not re.fullmatch(r"trial-[0-9a-f]{32}", trial_id))):
        raise artifact_io.BenchmarkError("invalid_batch_binding", "batch trials need an exact planned ID and binding")
    if batch_binding is not None:
        batch_binding = dict(batch_execution.validate_batch_binding(batch_binding))
    output.mkdir(parents=True, exist_ok=True)
    trial = output.resolve() / (trial_id or ("trial-" + uuid.uuid4().hex))
    trial.mkdir(mode=0o700)
    env = runtime_identity.clean_environment(trial)
    prepared = time.monotonic()
    manifest = {"kind": "mastermind-research-trial", "schema_version": 4 if selected else 3 if batch_binding else 2,
                "trial_id": trial.name, "task": task, "condition": condition,
                "repetition": repetition, "rubric_sha256": artifact_io.digest(rubric),
                "model": model, "limits": limits, "tool_revision": tool_revision,
                "status": "setup_failed", "isolation": "host_adapter_unverified"}
    if selected is not None:
        manifest.update(condition_spec=selected, instruction_files=[])
    if calibration is not None:
        manifest["calibration"] = calibration
    if batch_binding is not None:
        manifest["batch"] = batch_binding
    artifact_io.write_new(trial / "rubric.json", rubric)
    try:
        source_snapshot.verify_tool_commit(tool_repo.resolve(strict=True), tool_revision, env)
        spec = config.get("adapter")
        if selected is not None and "reasoning_effort" in selected:
            if not isinstance(spec, dict) or spec.get("kind") not in ("codex_cli", "claude_cli"):
                raise artifact_io.BenchmarkError("condition_runtime_mismatch", "declared effort requires a Claude or Codex adapter")
            spec = dict(spec, reasoning_effort=selected["reasoning_effort"])
        if isinstance(spec, dict) and spec.get("kind") == "claude_cli":
            adapter = claude.prepare_runtime(trial, spec, limits)
        elif isinstance(spec, dict) and spec.get("kind") == "codex_cli":
            adapter = codex.prepare_runtime(trial, spec, limits)
        else:
            adapter = runtime_identity.runtime_pin(spec, "adapter")
        files, projection_revision = source_snapshot.export_source(
            source_repo.resolve(), task["revision"], task["source_allowlist"], trial / "source", env,
        )
        source_digest = artifact_io.digest({"revision": task["revision"], "files": files})
        manifest.update(adapter=adapter, source_files=files, source_sha256=source_digest,
                        projection_revision=projection_revision)
        manifest["common_sha256"] = artifact_io.digest(model_protocol.common_identity(manifest))
        instruction = ""
        if selected is not None:
            parts = []
            for instruction_path in selected["instruction_paths"]:
                body = source_snapshot.git_regular_blob(tool_repo.resolve(), tool_revision, instruction_path, env,
                                        role="instruction", byte_limit=artifact_io.CONTROL_BYTE_LIMIT)[0]
                parts.append(body.decode("utf-8"))
                manifest["instruction_files"].append({"path": instruction_path, "bytes": len(body),
                                                       "sha256": hashlib.sha256(body).hexdigest()})
            instruction = "\n\n".join(parts)
            if len(instruction.encode()) > artifact_io.CONTROL_BYTE_LIMIT:
                raise artifact_io.BenchmarkError("instruction_limit", "combined instruction exceeds the control byte cap")
        elif condition != "source":
            instruction_path = condition_contract.safe_source_path(config.get("instruction_path"))
            instruction = source_snapshot.git_regular_blob(
                tool_repo.resolve(), tool_revision, instruction_path, env,
                role="instruction", byte_limit=artifact_io.CONTROL_BYTE_LIMIT,
            )[0].decode("utf-8")
        manifest["instruction_sha256"] = hashlib.sha256(instruction.encode()).hexdigest()
        indexer = None
        if condition_contract.uses_mmcg(manifest):
            indexer = runtime_identity.runtime_pin(config.get("mmcg"), "mmcg", tool_revision)
            contract = config["mmcg"].get("index_contract")
            if (not isinstance(contract, dict) or set(contract) != {
                    "schema_version", "extractor_contract_version", "concept_normalization_version"}
                    or any(not isinstance(value, str) or not value for value in contract.values())):
                raise artifact_io.BenchmarkError("index_contract_missing", "pin the expected mmcg index contracts")
            indexed_paths = config["mmcg"].get("indexed_files", task["source_allowlist"])
            if (not isinstance(indexed_paths, list) or not indexed_paths
                    or any(path not in task["source_allowlist"] for path in indexed_paths)
                    or len(indexed_paths) != len(set(indexed_paths))):
                raise artifact_io.BenchmarkError("index_scope_invalid", "declare an exact indexed subset of allowed source files")
            manifest.update(index_contract=contract, indexed_files=sorted(indexed_paths))
            index = trial / "index"
            index.mkdir()
            index_path = index / "mmcg.db"
            process = process_runner.run_bounded([indexer["path"], "--index", str(index_path), "index", str(trial / "source")],
                                  cwd=trial / "source", env=env, timeout=60)
            manifest["index_setup_seconds"] = process.elapsed_seconds
            manifest["index_process"] = {"returncode": process.returncode, "stop_reason": process.stop_reason}
            artifact_io.write_new_bytes(trial / "index-stdout.txt", process.stdout)
            artifact_io.write_new_bytes(trial / "index-stderr.txt", process.stderr)
            if process.stop_reason or process.returncode != 0:
                raise artifact_io.BenchmarkError("index_setup_failed", "indexer failed; any partial database is unusable")
            manifest["index_sha256"] = source_snapshot.validate_index(index_path, trial / "source", files, contract, indexed_paths)
            manifest["indexer"] = indexer
        source_snapshot.verify_source(trial / "source", files)
        source_snapshot.verify_projection(trial / "source", files, projection_revision, env)
        manifest["condition_sha256"] = artifact_io.digest(model_protocol.condition_identity(manifest))
        request = model_protocol.adapter_request(trial, manifest, instruction)
        request_bytes = artifact_io.canonical(request)
        if len(request_bytes) > artifact_io.CONTROL_BYTE_LIMIT:
            raise artifact_io.BenchmarkError("request_limit", "adapter request exceeds the control byte cap")
        manifest["request_sha256"] = hashlib.sha256(request_bytes).hexdigest()
        artifact_io.write_new(trial / "request.json", request)
        for item in files:
            (trial / "source" / item["path"]).chmod(0o555 if item["git_mode"] == "100755" else 0o444)
        manifest["status"] = "prepared"
    except (artifact_io.BenchmarkError, UnicodeError, ValueError, OSError, TypeError, KeyError) as error:
        manifest["setup_error"] = {"code": getattr(error, "code", "invalid_setup"), "message": str(error)}
    manifest["setup_seconds"] = time.monotonic() - prepared
    artifact_io.write_new(trial / "manifest.json", manifest)
    return trial


def verify_prepared(trial: Path, manifest: dict) -> dict:
    if manifest.get("kind") != "mastermind-research-trial" or manifest.get("schema_version") not in (1, 2, 3, 4):
        raise artifact_io.BenchmarkError("invalid_manifest", "unknown trial manifest")
    if manifest.get("schema_version") == 3 and "batch" not in manifest:
        raise artifact_io.BenchmarkError("invalid_manifest", "batch-bound manifest is missing its binding")
    if "batch" in manifest:
        batch_execution.validate_batch_binding(manifest["batch"])
    if manifest["trial_id"] != trial.name:
        raise artifact_io.BenchmarkError("invalid_manifest", "trial identity or condition changed")
    condition_contract.condition_spec(manifest)
    model_protocol.validate_task(manifest["task"])
    artifact_io.exact_revision(manifest["tool_revision"])
    if model_protocol.validate_limits(manifest["limits"]) != manifest["limits"]:
        raise artifact_io.BenchmarkError("invalid_manifest", "manifest limits are incomplete")
    files = manifest["source_files"]
    if not isinstance(files, list) or len(files) != len(manifest["task"]["source_allowlist"]):
        raise artifact_io.BenchmarkError("invalid_manifest", "invalid source manifest")
    for item in files:
        if (set(item) != {"path", "git_mode", "bytes", "sha256"}
                or item["git_mode"] not in ("100644", "100755")
                or type(item["bytes"]) is not int or not 0 <= item["bytes"] <= artifact_io.FILE_BYTE_LIMIT
                or not isinstance(item["sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", item["sha256"])):
            raise artifact_io.BenchmarkError("invalid_manifest", "invalid source file record")
        condition_contract.safe_source_path(item["path"])
    if sum(item["bytes"] for item in files) > condition_contract.SOURCE_BYTE_LIMIT:
        raise artifact_io.BenchmarkError("source_limit", "source manifest exceeds its total byte limit")
    if {item["path"] for item in files} != set(manifest["task"]["source_allowlist"]):
        raise artifact_io.BenchmarkError("invalid_manifest", "source files differ from the task allowlist")
    if (artifact_io.digest({"revision": manifest["task"]["revision"], "files": files}) != manifest["source_sha256"]
            or artifact_io.digest(model_protocol.common_identity(manifest)) != manifest["common_sha256"]
            or artifact_io.digest(model_protocol.condition_identity(manifest)) != manifest["condition_sha256"]):
        raise artifact_io.BenchmarkError("manifest_changed", "trial identities do not match the manifest fields")
    rubric = artifact_io.load_json(trial / "rubric.json")
    if artifact_io.digest(rubric) != manifest["rubric_sha256"]:
        raise artifact_io.BenchmarkError("rubric_changed", "rubric changed after preparation")
    model_protocol.validate_rubric(manifest["task"], rubric)
    request = artifact_io.load_json(trial / "request.json")
    if hashlib.sha256(artifact_io.canonical(request)).hexdigest() != manifest["request_sha256"]:
        raise artifact_io.BenchmarkError("request_changed", "adapter request changed after preparation")
    instruction = request["portable_instruction"]
    condition_contract.verify_instruction(manifest, instruction)
    if request != model_protocol.adapter_request(trial, manifest, instruction):
        raise artifact_io.BenchmarkError("request_changed", "request differs from the frozen trial inputs")
    source_snapshot.verify_source(trial / "source", files)
    env = runtime_identity.clean_environment(trial)
    source_snapshot.verify_projection(trial / "source", files, manifest["projection_revision"], env)
    runtime_identity.runtime_pin(manifest["adapter"], "adapter")
    if manifest["adapter"].get("kind") == "claude_cli":
        claude.verify_runtime(trial, manifest["adapter"])
    elif manifest["adapter"].get("kind") == "codex_cli":
        codex.verify_runtime(trial, manifest["adapter"])
    if condition_contract.uses_mmcg(manifest):
        runtime_identity.runtime_pin(manifest["indexer"], "mmcg", manifest["tool_revision"])
        if source_snapshot.validate_index(trial / "index/mmcg.db", trial / "source", manifest["source_files"],
                          manifest["index_contract"], manifest["indexed_files"]) != manifest["index_sha256"]:
            raise artifact_io.BenchmarkError("index_changed", "prepared index changed")
    return request


def claim_attempt(trial: Path) -> None:
    if batch_execution.optional_artifact(trial / "result.json", artifact_io.CONTROL_BYTE_LIMIT) is not None:
        raise artifact_io.BenchmarkError("already_run", "trial was already attempted; prepare a new balanced batch")
    try:
        artifact_io.write_new_bytes(trial / "run.lock", b"")
    except artifact_io.BenchmarkError as error:
        if error.code != "artifact_exists":
            raise artifact_io.BenchmarkError("attempt_lock", "cannot claim the trial attempt") from error
        raise artifact_io.BenchmarkError("already_run", "trial was already attempted; prepare a new balanced batch") from error


def _run_trial_attempt(trial: Path, manifest_bytes: bytes, manifest: dict,
                       credentials: dict[str, str] | None, execution: dict | None) -> dict:
    # No implicit reruns: each planned condition/repetition gets one attempt.
    claim_attempt(trial)
    envelope = {"kind": "mastermind-research-result", "schema_version": 2 if execution else 1,
                "trial_id": trial.name, "manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(),
                "common_sha256": manifest.get("common_sha256"),
                "condition_sha256": manifest.get("condition_sha256"),
                "run_status": {"state": "setup_error", "reason": None},
                "quality": {"status": "not_evaluated", "score": None},
                "diagnostics": {}, "answer": None,
                "comparability": {"eligible": False, "isolation": "host_adapter_unverified",
                    "reasons": ["adapter_isolation_unverified", "runtime_provenance_declared"]}}
    if execution is not None:
        envelope["batch_execution"] = execution["receipt"]
    try:
        if manifest["status"] != "prepared":
            raise artifact_io.BenchmarkError(manifest.get("setup_error", {}).get("code", "setup_failed"), "trial setup did not complete")
        request = verify_prepared(trial, manifest)
        env = runtime_identity.clean_environment(trial, credentials)
    except (artifact_io.BenchmarkError, KeyError, TypeError, ValueError, OSError) as error:
        envelope["run_status"]["reason"] = getattr(error, "code", "invalid_manifest")
        artifact_io.write_new(trial / "result.json", envelope)
        return envelope
    limits = manifest["limits"]
    adapter = manifest["adapter"]
    command = ([adapter["python"]["path"], "-I", "-S", "-B", adapter["path"],
                *(("codex",) if adapter.get("kind") == "codex_cli" else ()),
                "--runtime", str(trial / "adapter-runtime.json")]
               if adapter.get("kind") in ("claude_cli", "codex_cli") else [adapter["path"]])
    process = process_runner.run_bounded(command, cwd=trial / "source", env=env,
                          stdin=artifact_io.canonical(request) + b"\n", timeout=limits["timeout_seconds"],
                          stdout_limit=limits["trace_bytes"], stderr_limit=limits["stderr_bytes"])
    artifact_io.write_new_bytes(trial / "trace.jsonl", process.stdout)
    artifact_io.write_new_bytes(trial / "stderr.txt", process.stderr)
    init, result, tools, issues = model_protocol.parse_stream(process.stdout, limits["answer_bytes"])
    state, reason = "completed", None
    reported_failure = result.get("failure") if result else None
    reported_model_error = result.get("model_error") is True if result else False
    identity_changed = bool(init) and (
        init.get("adapter_version") != manifest["adapter"]["version"]
        or (init.get("model") is not None and init.get("model") != manifest["model"])
        or (init.get("model") is None and not reported_failure and not reported_model_error)
    )
    if process.stop_reason:
        state = process.stop_reason if process.stop_reason in {"timeout", "output_limit"} else "invocation_error"
        reason = process.stop_reason
    elif process.returncode != 0:
        state, reason = "invocation_error", "nonzero_exit"
    elif issues:
        state, reason = "protocol_error", issues[0]
    elif identity_changed:
        state, reason = "identity_mismatch", "observed_model_or_adapter_version_mismatch"
    elif reported_failure:
        state, reason = reported_failure["state"], reported_failure["code"]
    elif reported_model_error:
        state, reason = "model_error", "adapter_reported_model_error"
    measured = model_protocol.telemetry(result, limits)
    if any(value is not None and value > process.elapsed_seconds for value in measured["timings"].values()):
        measured["issues"] = sorted(set(measured["issues"]) | {"invalid_timings"})
        measured["complete"] = False
        measured["timings"] = dict.fromkeys(measured["timings"])
    unexpected_tools = sorted(set(tools) - set(request["available_tools"]))
    envelope["diagnostics"] = {"protocol_issues": issues, "telemetry": measured, "tools": tools,
        "unexpected_tools": unexpected_tools,
        "observed_model": init.get("model") if init else None,
        "observed_adapter_version": init.get("adapter_version") if init else None,
        "elapsed_seconds": process.elapsed_seconds, "setup_seconds": manifest["setup_seconds"],
        "returncode": process.returncode, "trace_bytes": len(process.stdout), "stderr_bytes": len(process.stderr),
        "enforced_limits": [name for name in ("timeout_seconds", "trace_bytes", "stderr_bytes", "answer_bytes")
                            if limits[name] is not None]}
    if adapter.get("kind") in ("claude_cli", "codex_cli") and result:
        envelope["diagnostics"]["adapter"] = result.get("diagnostics")
        if adapter["kind"] == "codex_cli":
            envelope["diagnostics"]["observed_model"] = None
            envelope["diagnostics"]["requested_model"] = manifest["model"]
    answer = result.get("answer") if result else None
    if isinstance(answer, str) and answer.strip() and len(answer.encode("utf-8")) <= limits["answer_bytes"]:
        answer_bytes = answer.encode("utf-8")
        artifact_io.write_new_bytes(trial / "answer.md", answer_bytes)
        envelope["answer"] = {"path": "answer.md", "bytes": len(answer_bytes),
                              "sha256": hashlib.sha256(answer_bytes).hexdigest()}
        envelope["quality"]["status"] = "review_pending"
    try:
        verify_prepared(trial, manifest)
        batch_execution.verify_batch_snapshot(trial, manifest, manifest_bytes, execution)
        if artifact_io.read_file(trial / "manifest.json") != manifest_bytes:
            raise artifact_io.BenchmarkError("manifest_changed", "manifest changed during invocation")
    except (artifact_io.BenchmarkError, ValueError, KeyError, TypeError, OSError) as error:
        envelope["comparability"]["reasons"].append(getattr(error, "code", "prepared_state_changed"))
        if state == "completed":
            state, reason = "input_changed", getattr(error, "code", "prepared_state_changed")
    if not measured["complete"]:
        envelope["comparability"]["reasons"].append("telemetry_incomplete")
        if state == "completed":
            state, reason = "protocol_error", "telemetry_incomplete"
    if measured["budget_exceeded"]:
        envelope["comparability"]["reasons"].append("model_budget_exceeded")
        if state == "completed":
            state, reason = "budget_exceeded", "model_budget_exceeded"
    if unexpected_tools:
        envelope["comparability"]["reasons"].append("unexpected_tool")
        if state == "completed":
            state, reason = "protocol_error", "unexpected_tool"
    if state != "completed":
        envelope["comparability"]["reasons"].append(state)
    envelope["run_status"] = {"state": state, "reason": reason}
    artifact_io.write_new(trial / "result.json", envelope)
    return envelope


def run_trial(trial: Path, credentials: dict[str, str] | None = None) -> dict:
    trial = trial.resolve(strict=True)
    manifest_bytes = artifact_io.read_file(trial / "manifest.json")
    manifest = artifact_io.parse_json(manifest_bytes)
    with batch_execution.batch_execution_guard(trial, manifest, manifest_bytes) as execution:
        return _run_trial_attempt(trial, manifest_bytes, manifest, credentials, execution)


def prepare_batch(*, repetitions: int = 3, corpus_case: dict | None = None, **kwargs) -> Path:
    if type(repetitions) is not int or not 1 <= repetitions <= 20:
        raise artifact_io.BenchmarkError("invalid_repetitions", "use 1..20 repetitions")
    config = kwargs["config"]
    conditions = condition_contract.validate_conditions(config["conditions"]) if "conditions" in config else None
    calibration = condition_contract.validate_calibration(config["calibration"], conditions) if "calibration" in config else None
    names = tuple(item["id"] for item in conditions) if conditions else condition_contract.CONDITIONS
    if repetitions * len(names) > condition_contract.TRIAL_LIMIT:
        raise artifact_io.BenchmarkError("invalid_repetitions", "batch exceeds 60 planned trials")
    if conditions and any(set(item["instruction_paths"]) & set(kwargs["task"]["source_allowlist"])
                          for item in conditions):
        raise artifact_io.BenchmarkError("instruction_source_overlap", "condition instructions cannot be public source evidence")
    output = kwargs.pop("output").resolve()
    output.mkdir(parents=True, exist_ok=True)
    batch_id = "batch-" + uuid.uuid4().hex
    batch = output / batch_id
    batch.mkdir(mode=0o700)
    artifact_io.write_new_bytes(batch / "execution.lock", b"")
    planned = []
    for repetition in range(repetitions):
        offset = repetition % len(names)
        for condition in names[offset:] + names[:offset]:
            planned.append({"directory": "trial-" + uuid.uuid4().hex, "condition": condition,
                            "repetition": repetition})
    version = 3 if conditions else 2
    plan = {"schema_version": version, "batch_id": batch_id, "task_id": kwargs["task"]["id"], "repetitions": repetitions,
            "trials": planned}
    if conditions:
        plan["conditions"] = conditions
    if calibration is not None:
        plan["calibration"] = calibration
    plan_sha256 = artifact_io.digest(batch_execution.batch_plan_identity(plan))
    trials = []
    for position, item in enumerate(planned):
        binding = {"batch_id": batch_id, "plan_sha256": plan_sha256, "position": position}
        trial = prepare_trial(output=batch, condition=item["condition"], repetition=item["repetition"],
                              trial_id=item["directory"], batch_binding=binding, **kwargs)
        manifest = artifact_io.load_json(trial / "manifest.json")
        trials.append(dict(item, common_sha256=manifest.get("common_sha256"), status=manifest["status"]))
    summary = {"kind": "mastermind-research-batch", "schema_version": version,
              "batch_id": batch_id, "plan_sha256": plan_sha256,
              "task_id": kwargs["task"]["id"], "repetitions": repetitions, "trials": trials,
              "quality_uplift": None, "comparison_accepted": False}
    if conditions:
        summary["conditions"] = conditions
    if calibration is not None:
        summary["calibration"] = calibration
    if corpus_case is not None:
        summary["corpus_case"] = corpus_case
    artifact_io.write_new(batch / "batch.json", summary)
    return batch


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    prepare = sub.add_parser("prepare", help="freeze allowlisted trials; never invoke a model")
    for name in ("config", "source-repo", "tool-repo", "output"):
        prepare.add_argument("--" + name, type=Path, required=True)
    selection = prepare.add_mutually_exclusive_group(required=True)
    selection.add_argument("--task", type=Path)
    selection.add_argument("--case", help="select a checked calibration from the corpus")
    prepare.add_argument("--rubric", type=Path)
    prepare.add_argument("--corpus", type=Path, help="corpus registry for --case; defaults to the bundled corpus")
    prepare.add_argument("--repetitions", type=int, default=3)
    execute = sub.add_parser("run", help="invoke the pinned trusted adapter exactly once")
    execute.add_argument("trial", type=Path)
    execute.add_argument("--credential-env", action="append", choices=sorted(runtime_identity.CREDENTIAL_NAMES), default=[])
    args = parser.parse_args(argv)
    try:
        if args.command == "prepare":
            corpus_case = None
            config = artifact_io.load_json(args.config)
            if args.case is not None:
                if args.rubric is not None:
                    raise artifact_io.BenchmarkError("invalid_selection", "--case uses its bound corpus rubric")
                from . import corpus as benchmark_corpus
                selected = benchmark_corpus.select_case(args.corpus or benchmark_corpus.DEFAULT_CORPUS, args.case, args.source_repo)
                config = benchmark_corpus.configure_case(selected, config)
                task, rubric, corpus_case = selected["task"], selected["rubric"], selected["summary"]
            else:
                if args.rubric is None or args.corpus is not None:
                    raise artifact_io.BenchmarkError("invalid_selection", "--task requires --rubric and cannot use --corpus")
                task, rubric = artifact_io.load_json(args.task), artifact_io.load_json(args.rubric)
            batch = prepare_batch(task=task, rubric=rubric, config=config, corpus_case=corpus_case,
                source_repo=args.source_repo, tool_repo=args.tool_repo, output=args.output, repetitions=args.repetitions)
            print(batch)
            return 0 if all(t["status"] == "prepared" for t in artifact_io.load_json(batch / "batch.json")["trials"]) else 2
        credentials = {name: os.environ[name] for name in args.credential_env if name in os.environ}
        if len(credentials) != len(set(args.credential_env)):
            raise artifact_io.BenchmarkError("credentials_missing", "a requested credential variable is absent")
        result = run_trial(args.trial, credentials)
        print(json.dumps({"trial": str(args.trial), "run_status": result["run_status"],
                          "quality": result["quality"], "comparison_accepted": False}))
        return 0 if result["run_status"]["state"] == "completed" else 2
    except (artifact_io.BenchmarkError, OSError, ValueError, KeyError, TypeError) as error:
        print(f"{getattr(error, 'code', 'benchmark_error')}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
