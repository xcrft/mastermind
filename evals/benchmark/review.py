"""Export blinded packets, import reviews and compare retained task outcomes."""

from __future__ import annotations

from pathlib import Path, PurePosixPath
import argparse
import secrets
import sys
import uuid

from . import analysis, efficiency
from . import assessment as review_assessment
from . import review_contracts
from .review_io import Root, OUTPUT_LIMIT, encoded, sha
from evals.benchmark import artifacts as artifact_io
from evals.benchmark import batch as batch_execution
from evals.benchmark import conditions as condition_contract
from evals.benchmark import corpus
from evals.benchmark import protocol as model_protocol


def file_records(value, task, lines=False):
    review_contracts.require(isinstance(value, list) and len(value) == len(task["source_allowlist"]), "invalid source inventory")
    records = {}
    for item in value:
        review_contracts.fields(item, ("path", "bytes", "sha256", "git_mode", *(["lines"] if lines else [])))
        path = condition_contract.safe_source_path(item["path"])
        review_contracts.require(path not in records and path in task["source_allowlist"], "source scope differs from the task")
        review_contracts.require(type(item["bytes"]) is int and 0 <= item["bytes"] <= artifact_io.FILE_BYTE_LIMIT, "invalid source size")
        review_contracts.require(item["git_mode"] in ("100644", "100755"), "invalid source mode")
        review_contracts.hash_value(item["sha256"])
        if lines:
            review_contracts.require(type(item["lines"]) is int and 0 <= item["lines"] <= item["bytes"], "invalid source line count")
        records[path] = item
    review_contracts.require(sum(item["bytes"] for item in value) <= condition_contract.SOURCE_BYTE_LIMIT, "source byte cap exceeded", "review_limit")
    return records


def line_count(body):
    body.decode("utf-8")
    return body.count(b"\n") + int(bool(body) and not body.endswith(b"\n"))


def check_batch(batch, names):
    if isinstance(batch, dict) and isinstance(batch.get("trials"), list):
        planned = {item["directory"] for item in batch["trials"]
                   if isinstance(item, dict) and isinstance(item.get("directory"), str)}
        review_contracts.require(not {name for name in names if name.startswith("trial-")} - planned,
                "batch omits trial artifacts present in its directory", "review_inventory")
    batch_execution.validate_batch_summary(batch, allow_legacy=True,
                                 code="review_inventory", identity_code="review_identity")


def check_manifest(manifest, item, batch):
    review_contracts.require(isinstance(manifest, dict), "invalid trial manifest")
    for field in ("kind", "schema_version", "trial_id", "condition", "repetition", "task", "status",
                  "rubric_sha256", "model", "tool_revision", "limits", "isolation"):
        review_contracts.require(field in manifest, "incomplete trial manifest")
    review_contracts.require(manifest["kind"] == "mastermind-research-trial" and type(manifest["schema_version"]) is int
            and manifest["schema_version"] in (1, 2, 3, 4), "unsupported trial manifest")
    spec = condition_contract.condition_spec(manifest)
    expected = next((item for item in batch.get("conditions", []) if item["id"] == manifest["condition"]), None)
    review_contracts.require(spec == expected, "trial condition differs from its batch specification", "review_identity")
    review_contracts.require(manifest.get("calibration") == batch.get("calibration"),
            "trial calibration differs from its batch specification", "review_identity")
    for key, expected in (("trial_id", item["directory"]), ("condition", item["condition"]),
                          ("repetition", item["repetition"]), ("status", item["status"]),
                          ("common_sha256", item["common_sha256"])):
        review_contracts.require(manifest.get(key) == expected, "trial differs from its batch slot", "review_identity")
    review_contracts.require(type(manifest["repetition"]) is int and isinstance(manifest["task"], dict), "invalid trial fields")
    model_protocol.validate_task(manifest["task"])
    review_contracts.require(manifest["task"]["id"] == batch["task_id"], "trial task differs from batch", "review_identity")
    if batch["schema_version"] != 1:
        position = next(index for index, candidate in enumerate(batch["trials"])
                        if candidate["directory"] == item["directory"])
        review_contracts.require(manifest.get("batch") == {"batch_id": batch["batch_id"],
                "plan_sha256": batch["plan_sha256"], "position": position},
                "trial batch binding differs from its planned slot", "review_identity")
    else:
        review_contracts.require("batch" not in manifest, "legacy batch contains a bound trial", "review_identity")
    review_contracts.hash_value(manifest["rubric_sha256"])
    review_contracts.text(manifest["model"])
    artifact_io.exact_revision(manifest["tool_revision"])
    review_contracts.require(model_protocol.validate_limits(manifest["limits"]) == manifest["limits"], "trial limits are incomplete")
    review_contracts.require(manifest["isolation"] == "host_adapter_unverified", "unsupported isolation claim")
    if manifest["status"] == "prepared":
        for field in ("adapter", "source_files", "source_sha256", "common_sha256", "condition_sha256",
                      "projection_revision", "instruction_sha256", "request_sha256"):
            review_contracts.require(field in manifest, "prepared trial is missing its identity")
    if manifest.get("source_files") is not None:
        file_records(manifest["source_files"], manifest["task"])
        review_contracts.require(artifact_io.digest({"revision": manifest["task"]["revision"], "files": manifest["source_files"]})
                == manifest.get("source_sha256"), "source identity changed", "review_identity")
    try:
        if manifest.get("common_sha256") is not None:
            review_contracts.require(artifact_io.digest(model_protocol.common_identity(manifest)) == manifest["common_sha256"],
                    "common identity changed", "review_identity")
        if manifest.get("condition_sha256") is not None:
            review_contracts.require(artifact_io.digest(model_protocol.condition_identity(manifest)) == manifest["condition_sha256"],
                    "condition identity changed", "review_identity")
    except (KeyError, TypeError) as error:
        raise artifact_io.BenchmarkError("review_identity", "trial identity fields are incomplete") from error


def check_request(request, manifest):
    review_contracts.require(isinstance(request, dict) and artifact_io.digest(request) == manifest["request_sha256"],
            "prepared request changed", "review_identity")
    instruction = request.get("portable_instruction")
    try:
        condition_contract.verify_instruction(manifest, instruction)
    except (artifact_io.BenchmarkError, KeyError, TypeError, UnicodeError) as error:
        raise artifact_io.BenchmarkError("review_identity", "instruction changed") from error
    # Derive the old location lexically. A review archive may have moved and no
    # old executable, source directory or SQLite index needs to remain installed.
    source = PurePosixPath(review_contracts.text(request.get("source_root")))
    review_contracts.require(source.is_absolute() and ".." not in source.parts and source.name == "source"
            and source.parent.name == manifest["trial_id"], "invalid original trial location")
    review_contracts.require(request == model_protocol.adapter_request(Path(source.parent), manifest, instruction),
            "request differs from the frozen manifest", "review_identity")


def read_sources(root, prefix, records, check_mode=True):
    bodies, described = {}, []
    for item in records:
        path = prefix + item["path"]
        body = root.read(path, artifact_io.FILE_BYTE_LIMIT)
        review_contracts.require(len(body) == item["bytes"] and sha(body) == item["sha256"], "source bytes changed", "review_source")
        if check_mode:
            mode = "100755" if root.records[path]["identity"][2] & 0o100 else "100644"
            review_contracts.require(mode == item["git_mode"], "source mode changed", "review_source")
        bodies[item["path"]] = body
        described.append(dict(item, lines=line_count(body)))
    return bodies, described


def read_result(root, prefix, manifest, manifest_body):
    lock = root.read(prefix + "run.lock", limit=0, optional=True)
    body = root.read(prefix + "result.json", optional=True)
    if body is None:
        state = "setup_error" if manifest["status"] == "setup_failed" else "unfinished" if lock is not None else "not_run"
        return state, None, None, None
    review_contracts.require(lock is not None, "result exists without its one-shot attempt lock", "review_identity")
    result = artifact_io.parse_json(body)
    review_contracts.fields(result, ("kind", "schema_version", "trial_id", "manifest_sha256", "common_sha256", "condition_sha256",
                    "run_status", "quality", "diagnostics", "answer", "comparability"), ("batch_execution",))
    review_contracts.require(result["kind"] == "mastermind-research-result" and type(result["schema_version"]) is int
            and result["schema_version"] in (1, 2)
            and ((result["schema_version"] == 2) == ("batch_execution" in result)), "unsupported result")
    review_contracts.require(result["trial_id"] == manifest["trial_id"] and result["manifest_sha256"] == sha(manifest_body)
            and result["common_sha256"] == manifest.get("common_sha256")
            and result["condition_sha256"] == manifest.get("condition_sha256"), "result identity differs from manifest", "review_identity")
    review_contracts.fields(result["run_status"], ("state", "reason"))
    state = result["run_status"]["state"]
    review_contracts.require(isinstance(state, str) and state in review_contracts.RUN_STATES, "unknown run state")
    if result["run_status"]["reason"] is not None:
        review_contracts.text(result["run_status"]["reason"], 1024)
    review_contracts.require(state != "completed" or result["run_status"]["reason"] is None, "completed run cannot declare a failure")
    review_contracts.fields(result["quality"], ("status", "score"))
    review_contracts.require(result["quality"]["score"] is None and isinstance(result["comparability"], dict)
            and result["comparability"].get("eligible") is False,
            "result cannot declare measured or accepted quality")
    answer = result["answer"]
    review_contracts.require(state != "completed" or answer is not None, "completed run needs a retained answer")
    review_contracts.require(result["quality"]["status"] == ("review_pending" if answer is not None else "not_evaluated"), "invalid answer review state")
    if manifest["status"] == "setup_failed":
        review_contracts.require(state == "setup_error" and answer is None, "failed preparation cannot produce an answer")
    if manifest["adapter"].get("version") in ("mastermind-codex-adapter-v6",
            *model_protocol.CODEX_EMPTY_DISCOVERY_VERSIONS) and state == "completed":
        adapter = result["diagnostics"].get("adapter", {})
        review_contracts.require(isinstance(adapter, dict) and adapter.get("raw_stream") == "codex-stream.jsonl",
                "completed Codex run needs its bound event stream", "review_trace")
        review_contracts.hash_value(adapter.get("raw_stream_sha256"))
        trace = root.read(prefix + "codex-stream.jsonl", manifest["limits"]["trace_bytes"])
        review_contracts.require(sha(trace) == adapter["raw_stream_sha256"], "Codex event stream changed", "review_trace")
    answer_body = None
    if answer is not None:
        review_contracts.fields(answer, ("path", "bytes", "sha256"))
        review_contracts.require(answer["path"] == "answer.md" and type(answer["bytes"]) is int
                and 0 < answer["bytes"] <= manifest["limits"]["answer_bytes"], "invalid answer descriptor")
        review_contracts.hash_value(answer["sha256"])
        answer_body = root.read(prefix + "answer.md", manifest["limits"]["answer_bytes"])
        review_contracts.require(len(answer_body) == answer["bytes"] and sha(answer_body) == answer["sha256"]
                and bool(answer_body.decode("utf-8").strip()), "retained answer changed", "review_answer")
    return state, sha(body), answer_body, result


def execution_integrity(batch, slots):
    if batch["schema_version"] == 1:
        for slot in slots:
            review_contracts.require(slot.get("batch_execution") is None and slot.get("execution_order") == "unverified_legacy",
                    "legacy result contains an execution-order claim", "review_identity")
        return "unverified_legacy"
    previous_hash = None
    chain = True
    any_verified = False
    for position, slot in enumerate(slots):
        receipt = slot.get("batch_execution")
        result_hash = slot["result_sha256"]
        if result_hash is None:
            review_contracts.require(receipt is None and slot.get("execution_order") == "not_recorded",
                    "attempt without a result cannot claim execution order", "review_identity")
            chain = False
            previous_hash = None
            continue
        review_contracts.fields(receipt, ("batch_id", "plan_sha256", "position", "previous_result_sha256"))
        review_contracts.require(receipt["batch_id"] == batch["batch_id"]
                and receipt["plan_sha256"] == batch["plan_sha256"]
                and receipt["position"] == position, "result execution receipt differs from the batch", "review_identity")
        if position == 0:
            review_contracts.require(receipt["previous_result_sha256"] is None,
                    "first batch result cannot name a predecessor", "review_identity")
        else:
            review_contracts.hash_value(receipt["previous_result_sha256"])
        if previous_hash is not None:
            review_contracts.require(receipt["previous_result_sha256"] == previous_hash,
                    "result execution chain changed", "review_identity")
        expected = "verified" if chain else "unavailable"
        review_contracts.require(slot.get("execution_order") == expected, "invalid execution-order status", "review_identity")
        any_verified |= expected == "verified"
        previous_hash = result_hash
    if chain and all(slot["result_sha256"] is not None for slot in slots):
        return "verified"
    return "partial" if any_verified else "not_established"


def collect_batch(root):
    batch = root.json("batch.json")
    check_batch(batch, root.names())
    if batch["schema_version"] != 1:
        root.read("execution.lock", limit=0)
    context = None
    common = None
    indexed_subsets = []
    source_bodies, sources = {}, []
    slots, answers = [], {}
    for position, item in enumerate(batch["trials"]):
        prefix = item["directory"] + "/"
        body = root.read(prefix + "manifest.json", optional=True)
        slot = {"review_id": "review-" + uuid.uuid4().hex, "trial_id": item["directory"],
                "condition": item["condition"], "repetition": item["repetition"], "preparation_status": item["status"],
                "status": "missing_artifacts", "manifest_sha256": None, "result_sha256": None,
                "answer_sha256": None, "source_integrity": "unavailable",
                "batch_execution": None,
                "resources": dict.fromkeys(review_contracts.RESOURCE_FIELDS), "runtime_contract": "unknown",
                "execution_order": "unverified_legacy" if batch["schema_version"] == 1 else "not_recorded"}
        slots.append(slot)
        if body is None:
            continue
        manifest = artifact_io.parse_json(body)
        check_manifest(manifest, item, batch)
        key = root.json(prefix + "rubric.json")
        corpus.validate_key(manifest["task"], key)
        review_contracts.require(artifact_io.digest(key) == manifest["rubric_sha256"], "review key changed", "review_identity")
        candidate = {field: manifest[field] for field in ("task", "model", "limits", "tool_revision")}
        candidate["rubric"] = key
        review_contracts.require(context is None or candidate == context, "batch mixes tasks, keys or runtime settings", "review_identity")
        context = candidate
        if manifest.get("common_sha256") is not None:
            review_contracts.require(common is None or common == manifest["common_sha256"], "batch mixes common identities", "review_identity")
            common = manifest["common_sha256"]
        if manifest["status"] == "prepared":
            check_request(root.json(prefix + "request.json"), manifest)
        if condition_contract.uses_mmcg(manifest) and "indexed_files" in manifest:
            indexed_subsets.append(manifest["indexed_files"])
        if manifest.get("source_files") is not None:
            try:
                bodies, described = read_sources(root, prefix + "source/", manifest["source_files"])
                review_contracts.require(not sources or described == sources, "batch source snapshots differ", "review_identity")
                source_bodies, sources = bodies, described
                slot["source_integrity"] = "verified"
            except (artifact_io.BenchmarkError, UnicodeError) as error:
                if getattr(error, "code", None) in ("review_limit", "review_identity"):
                    raise
                slot["source_integrity"] = "unavailable"
        state, result_hash, answer, result = read_result(root, prefix, manifest, body)
        slot["resources"], slot["runtime_contract"] = analysis.trial_measurements(manifest, result)
        if result is not None:
            slot["batch_execution"] = result.get("batch_execution")
            slot["execution_order"] = ("unverified_legacy" if batch["schema_version"] == 1
                                       else "verified" if all(previous["result_sha256"] is not None
                                                               and previous["execution_order"] == "verified"
                                                               for previous in slots[:position])
                                       else "unavailable")
        slot.update(status=state, manifest_sha256=sha(body), result_sha256=result_hash,
                    answer_sha256=sha(answer) if answer is not None else None)
        if answer is not None:
            answers[slot["review_id"]] = answer
        review_contracts.require(sum(map(len, answers.values())) + sum(map(len, source_bodies.values())) <= OUTPUT_LIMIT - 3 * artifact_io.CONTROL_BYTE_LIMIT,
                "retained review evidence exceeds the output cap", "review_limit")
    review_contracts.require(context is not None, "no intact task and key remain in the batch", "review_context")
    review_contracts.require(not answers or sources, "answers need an intact source snapshot from the same batch", "review_source")
    if sources:
        corpus.validate_anchors(corpus.validate_key(context["task"], context["rubric"]), {item["path"]: item for item in sources})
    if "corpus_case" in batch:
        case = batch["corpus_case"]
        review_contracts.require(isinstance(case, dict) and case.get("id") == context["task"]["id"]
                and case.get("role") == "calibration" and case.get("source_revision") == context["task"]["revision"]
                and case.get("task_sha256") == artifact_io.digest(context["task"])
                and case.get("rubric_sha256") == artifact_io.digest(context["rubric"]), "corpus binding differs from the batch", "review_identity")
        records = file_records(case.get("source_files"), context["task"], lines=True)
        review_contracts.require(not sources or records == {item["path"]: item for item in sources}, "corpus source differs from trial evidence", "review_identity")
        indexed = case.get("indexed_files")
        review_contracts.require(isinstance(indexed, list) and 1 <= len(indexed) <= len(records)
                and all(isinstance(path, str) and path in records for path in indexed)
                and len(set(indexed)) == len(indexed), "invalid corpus indexed subset")
        review_contracts.require(all(isinstance(subset, list) and sorted(subset) == sorted(indexed) for subset in indexed_subsets),
                "corpus indexed subset differs from the graph trials", "review_identity")
    order_integrity = execution_integrity(batch, slots)
    root.recheck()
    return context, slots, answers, source_bodies, sources, batch, order_integrity


def export_review(batch: Path, output: Path):
    destination = output.parent.resolve(strict=True) / output.name
    with Root(batch) as root:
        review_contracts.require(not destination.is_relative_to(root.path), "review output must be outside the batch", "review_path")
        context, slots, answers, bodies, sources, batch_value, order_integrity = collect_batch(root)
        export_id = "export-" + uuid.uuid4().hex
        shuffled = slots.copy()
        secrets.SystemRandom().shuffle(shuffled)
        items = [{"review_id": slot["review_id"], "answer": {
            "path": "answers/" + slot["review_id"] + ".md", "bytes": len(answers[slot["review_id"]]),
            "sha256": slot["answer_sha256"]} if slot["review_id"] in answers else None} for slot in shuffled]
        packet = {"kind": "mastermind-research-review-packet", "schema_version": 1, "export_id": export_id,
                  "task": context["task"], "rubric": context["rubric"], "rubric_sha256": artifact_io.digest(context["rubric"]),
                  "source_files": sources, "items": items, "answer_text_may_reveal_condition": True,
                  "comparison_accepted": False, "quality_uplift": None}
        packet_body = encoded(packet)
        batch_contract = {"schema_version": batch_value["schema_version"],
                          "batch_id": batch_value.get("batch_id"),
                          "plan_sha256": batch_value.get("plan_sha256")}
        if batch_value["schema_version"] == 3:
            batch_contract["conditions"] = batch_value["conditions"]
            if "calibration" in batch_value:
                batch_contract["calibration"] = batch_value["calibration"]
        coordinator = {"kind": "mastermind-research-review-coordinator", "schema_version": 3,
                       "export_id": export_id, "packet_sha256": sha(packet_body), "batch_sha256": root.records["batch.json"]["sha256"],
                       "model": context["model"], "tool_revision": context["tool_revision"],
                       "corpus_case": batch_value.get("corpus_case"), "batch": batch_contract,
                       "execution_order_integrity": order_integrity, "slots": slots}
        coordinator_body = encoded(coordinator)
        criteria = context["rubric"].get("acceptance_criteria")
        template = {"kind": "mastermind-research-assessment", "schema_version": 3 if criteria else 2, "export_id": export_id,
                    "packet_sha256": sha(packet_body), "reviewer": None, "reviews": []}
        for item in items:
            if item["answer"] is not None:
                template["reviews"].append({"review_id": item["review_id"], "answer_sha256": item["answer"]["sha256"],
                    "rubric_sha256": packet["rubric_sha256"], "claims": [],
                    "outcome": {"status": None, "rationale": None},
                    "knowns": [{"known_index": index, "coverage": None, "answer_excerpt": None, "rationale": None}
                               for index in range(len(context["rubric"]["required_knowns"]))],
                    "unknowns": [{"unknown_index": index, "handling": None, "answer_excerpt": None, "rationale": None}
                                 for index in range(len(context["rubric"]["expected_unknowns"]))]})
                if criteria:
                    template["reviews"][-1]["acceptance"] = [
                        {"criterion_id": row["id"], "status": None, "answer_excerpt": None, "rationale": None}
                        for row in criteria]
        template_body = encoded(template)
        try:
            destination.mkdir(mode=0o700)
        except FileExistsError as error:
            raise artifact_io.BenchmarkError("review_exists", "choose a new export directory; replacement is not allowed") from error
        with Root(destination) as target:
            target.write_new("reviewer/packet.json", packet_body)
            target.write_new("reviewer/assessment-template.json", template_body)
            for path, body in bodies.items():
                target.write_new("reviewer/source/" + path, body)
            for item in items:
                if item["answer"] is not None:
                    target.write_new("reviewer/" + item["answer"]["path"], answers[item["review_id"]])
            target.write_new("coordinator.json", coordinator_body)
            root.recheck()
            target.write_new("seal.json", encoded({"kind": "mastermind-research-review-seal", "schema_version": 1,
                "export_id": export_id, "packet_sha256": sha(packet_body), "coordinator_sha256": sha(coordinator_body),
                "template_sha256": sha(template_body)}))
            target.recheck()
    return destination


def load_export(root):
    seal = root.json("seal.json")
    review_contracts.fields(seal, ("kind", "schema_version", "export_id", "packet_sha256", "coordinator_sha256", "template_sha256"))
    review_contracts.require(seal["kind"] == "mastermind-research-review-seal" and type(seal["schema_version"]) is int
            and seal["schema_version"] == 1, "unsupported review export")
    review_contracts.identifier(seal["export_id"], "export-")
    packet = root.json("reviewer/packet.json")
    coordinator = root.json("coordinator.json")
    template = root.read("reviewer/assessment-template.json")
    review_contracts.require(root.records["reviewer/packet.json"]["sha256"] == seal["packet_sha256"]
            and root.records["coordinator.json"]["sha256"] == seal["coordinator_sha256"]
            and sha(template) == seal["template_sha256"], "review export changed", "review_identity")
    review_contracts.fields(packet, ("kind", "schema_version", "export_id", "task", "rubric", "rubric_sha256", "source_files", "items",
                    "answer_text_may_reveal_condition", "comparison_accepted", "quality_uplift"))
    review_contracts.require(packet["kind"] == "mastermind-research-review-packet" and type(packet["schema_version"]) is int
            and packet["schema_version"] == 1 and packet["export_id"] == seal["export_id"]
            and packet["comparison_accepted"] is False and packet["quality_uplift"] is None
            and packet["answer_text_may_reveal_condition"] is True, "invalid review packet")
    model_protocol.validate_task(packet["task"])
    anchors = corpus.validate_key(packet["task"], packet["rubric"])
    review_contracts.require(artifact_io.digest(packet["rubric"]) == packet["rubric_sha256"], "packet key changed", "review_identity")
    sources = {}
    if packet["source_files"]:
        sources = file_records(packet["source_files"], packet["task"], lines=True)
        _, described = read_sources(root, "reviewer/source/", list(sources.values()), check_mode=False)
        review_contracts.require(described == packet["source_files"], "source line counts changed", "review_source")
        corpus.validate_anchors(anchors, sources)
    review_contracts.require(isinstance(packet["items"], list) and 2 <= len(packet["items"]) <= condition_contract.TRIAL_LIMIT, "invalid review item count")
    answers, items = {}, {}
    retained_bytes = sum(item["bytes"] for item in sources.values())
    for item in packet["items"]:
        review_contracts.fields(item, ("review_id", "answer"))
        review_contracts.identifier(item["review_id"], "review-")
        review_contracts.require(item["review_id"] not in items, "duplicate review item")
        items[item["review_id"]] = item
        answer = item["answer"]
        if answer is not None:
            review_contracts.fields(answer, ("path", "bytes", "sha256"))
            review_contracts.require(answer["path"] == "answers/" + item["review_id"] + ".md" and type(answer["bytes"]) is int
                    and 0 < answer["bytes"] <= 16 * 1024 * 1024, "invalid review answer")
            body = root.read("reviewer/" + answer["path"], 16 * 1024 * 1024)
            retained_bytes += len(body)
            review_contracts.require(retained_bytes <= OUTPUT_LIMIT, "review export exceeds its byte cap", "review_limit")
            review_contracts.require(len(body) == answer["bytes"] and sha(body) == answer["sha256"], "review answer changed", "review_answer")
            answers[item["review_id"]] = body.decode("utf-8")
    review_contracts.require(not answers or sources, "review answers lack source evidence", "review_source")
    root.inventory({"reviewer/packet.json", "reviewer/assessment-template.json"}
                   | {"reviewer/source/" + path for path in sources}
                   | {"reviewer/" + item["answer"]["path"] for item in items.values() if item["answer"] is not None})
    coordinator_version = coordinator.get("schema_version") if isinstance(coordinator, dict) else None
    extra = ("batch", "execution_order_integrity") if coordinator_version in (2, 3) else ()
    review_contracts.fields(coordinator, ("kind", "schema_version", "export_id", "packet_sha256", "batch_sha256", "model",
                         "tool_revision", "corpus_case", "slots", *extra))
    review_contracts.require(coordinator["kind"] == "mastermind-research-review-coordinator" and type(coordinator["schema_version"]) is int
            and coordinator["schema_version"] in (1, 2, 3) and coordinator["export_id"] == seal["export_id"]
            and coordinator["packet_sha256"] == seal["packet_sha256"], "coordinator identity changed", "review_identity")
    if coordinator_version in (2, 3):
        batch_contract = coordinator["batch"]
        review_contracts.fields(batch_contract, ("schema_version", "batch_id", "plan_sha256"),
               ("conditions", "calibration") if batch_contract.get("schema_version") == 3 else ())
        review_contracts.require(type(batch_contract["schema_version"]) is int and batch_contract["schema_version"] in (1, 2, 3),
                "invalid coordinator batch contract", "review_identity")
        if batch_contract["schema_version"] != 1:
            review_contracts.identifier(batch_contract["batch_id"], "batch-")
            review_contracts.hash_value(batch_contract["plan_sha256"])
        else:
            review_contracts.require(batch_contract["batch_id"] is None and batch_contract["plan_sha256"] is None,
                    "legacy coordinator cannot claim a bound plan", "review_identity")
        review_contracts.require(coordinator["execution_order_integrity"] in {
            "verified", "partial", "not_established", "unverified_legacy"},
            "invalid execution-order integrity", "review_identity")
    else:
        batch_contract = {"schema_version": 1, "batch_id": None, "plan_sha256": None}
    review_contracts.require(isinstance(coordinator["slots"], list) and len(coordinator["slots"]) == len(items), "coordinator omitted attempts", "review_inventory")
    names = condition_contract.condition_names(batch_contract)
    review_contracts.require(len(items) % len(names) == 0, "coordinator has an incomplete condition matrix", "review_inventory")
    seen = set()
    for slot in coordinator["slots"]:
        extra = ("batch_execution", "execution_order") if coordinator_version in (2, 3) else ()
        if coordinator_version == 3:
            extra += ("resources", "runtime_contract")
        review_contracts.fields(slot, ("review_id", "trial_id", "condition", "repetition", "preparation_status", "status", "manifest_sha256",
                      "result_sha256", "answer_sha256", "source_integrity", *extra))
        review_contracts.require(isinstance(slot["review_id"], str) and slot["review_id"] in items and slot["review_id"] not in seen
                and isinstance(slot["status"], str) and slot["status"] in review_contracts.SLOT_STATES
                and slot["source_integrity"] in ("verified", "unavailable"), "invalid coordinator slot")
        seen.add(slot["review_id"])
        if coordinator_version == 3:
            review_contracts.require(slot["runtime_contract"] in ("passed", "failed", "unknown"), "invalid runtime contract status")
            analysis.validate_resources(slot["resources"], complete=slot["runtime_contract"] == "passed")
            if slot["runtime_contract"] == "passed":
                review_contracts.require(slot["status"] == "completed", "failed attempt cannot declare a passed runtime contract")
            if slot["status"] in ("not_run", "unfinished", "missing_artifacts"):
                review_contracts.require(slot["runtime_contract"] == "unknown", "unobserved attempt cannot declare a runtime contract")
        for field in ("manifest_sha256", "result_sha256", "answer_sha256"):
            if slot[field] is not None:
                review_contracts.hash_value(slot[field])
        answer = items[slot["review_id"]]["answer"]
        review_contracts.require(slot["answer_sha256"] == (answer["sha256"] if answer is not None else None), "answer mapping changed", "review_identity")
        if slot["status"] == "missing_artifacts":
            review_contracts.require(slot["manifest_sha256"] is None and slot["result_sha256"] is None and answer is None
                    and slot["source_integrity"] == "unavailable", "missing attempt cannot declare retained evidence")
        else:
            review_contracts.hash_value(slot["manifest_sha256"])
        if slot["status"] in ("not_run", "unfinished"):
            review_contracts.require(slot["result_sha256"] is None and answer is None and slot["preparation_status"] == "prepared",
                    "unfinished attempt cannot declare a result")
        if slot["status"] in review_contracts.RUN_STATES - {"setup_error"}:
            review_contracts.hash_value(slot["result_sha256"])
        if slot["preparation_status"] == "setup_failed":
            review_contracts.require(slot["status"] in ("setup_error", "missing_artifacts") and answer is None,
                    "failed preparation cannot declare an answer")
        review_contracts.require(slot["status"] != "completed" or answer is not None, "completed attempt lacks an answer")
    reconstructed = {"kind": "mastermind-research-batch", "schema_version": batch_contract["schema_version"],
                     "task_id": packet["task"]["id"], "repetitions": len(items) // len(names),
                     "quality_uplift": None, "comparison_accepted": False,
                     "trials": [{"directory": slot["trial_id"], "condition": slot["condition"],
                                 "repetition": slot["repetition"], "status": slot["preparation_status"],
                                 "common_sha256": None} for slot in coordinator["slots"]]}
    if batch_contract["schema_version"] != 1:
        reconstructed.update(batch_id=batch_contract["batch_id"], plan_sha256=batch_contract["plan_sha256"])
    if batch_contract["schema_version"] == 3:
        reconstructed["conditions"] = batch_contract["conditions"]
        if "calibration" in batch_contract:
            reconstructed["calibration"] = batch_contract["calibration"]
    check_batch(reconstructed, set())
    if coordinator_version in (2, 3):
        review_contracts.require(execution_integrity(reconstructed, coordinator["slots"])
                == coordinator["execution_order_integrity"],
                "coordinator execution-order status changed", "review_identity")
    return seal, packet, coordinator, answers, sources


def import_assessment(export: Path, assessment: Path):
    with Root(export) as root, Root(assessment.absolute().parent) as submitted, root.exclusive_lock("import.lock"):
        seal, packet, _, answers, sources = load_export(root)
        value = submitted.json(assessment.name)
        review_assessment.check_assessment(value, seal, packet, answers, sources)
        existing = review_assessment.receipt_names(root)
        review_contracts.require(value["reviewer"] + ".json" not in existing, "reviewer already submitted an assessment", "review_exists")
        review_contracts.require(len(existing) < 64, "reviewer admission limit reached", "review_limit")
        receipt = {"kind": "mastermind-research-assessment-receipt", "schema_version": 1,
                   "export_id": seal["export_id"], "packet_sha256": seal["packet_sha256"],
                   "coordinator_sha256": seal["coordinator_sha256"], "assessment_sha256": artifact_io.digest(value),
                   "assessment": value, "semantics": "reviewer_declared_not_machine_verified",
                   "comparison_accepted": False, "quality_uplift": None}
        body = encoded(receipt)
        submitted.recheck()
        root.recheck()
        destination = "reviews/" + value["reviewer"] + ".json"
        root.write_new(destination, body)
        return root.path / destination


def review_status(export: Path):
    with Root(export) as root:
        seal, packet, coordinator, answers, sources = load_export(root)
        assessments = review_assessment.read_assessments(root, seal, packet, answers, sources)
        reviewers = [{"reviewer": value["reviewer"], "reviewed": len(value["reviews"])} for value in assessments]
        root.recheck()
        return {"kind": "mastermind-research-review-status", "schema_version": 3, "export_id": seal["export_id"],
                "attempts": analysis.attempt_counts(coordinator["slots"]), "reviewers": reviewers,
                "reviewed_attempts": len(answers) if reviewers else 0,
                "attempts_without_verified_source": sum(slot["source_integrity"] != "verified" for slot in coordinator["slots"]),
                "execution_order_integrity": coordinator.get("execution_order_integrity", "unverified_legacy"),
                "assessment_summary": analysis.assessment_summary(coordinator, assessments),
                "resources": analysis.resource_summary(coordinator),
                "assessment_semantics": "reviewer_declared_not_machine_verified", "comparison_accepted": False, "quality_uplift": None}


def compare_review(export: Path, baseline: str, candidate: str):
    with Root(export) as root:
        seal, packet, coordinator, answers, sources = load_export(root)
        names = condition_contract.condition_names(coordinator.get("batch", {"schema_version": 1}))
        review_contracts.require(baseline in names and candidate in names and baseline != candidate,
                "choose two distinct planned conditions", "review_contrast")
        assessments = review_assessment.read_assessments(root, seal, packet, answers, sources)
        basis = "user_acceptance" if "acceptance_criteria" in packet["rubric"] else "full_source_key"
        comparisons = [analysis.paired_comparison(coordinator, value, baseline, candidate, outcome_basis=basis)
                       for value in assessments or [None]]
        root.recheck()
        return {"kind": "mastermind-research-comparison", "schema_version": 1, "export_id": seal["export_id"],
                "task_id": packet["task"]["id"], "rubric_sha256": packet["rubric_sha256"],
                "model": coordinator["model"], "tool_revision": coordinator["tool_revision"],
                "baseline": baseline, "candidate": candidate, "attempts": analysis.attempt_counts(coordinator["slots"]),
                "execution_order_integrity": coordinator.get("execution_order_integrity", "unverified_legacy"),
                "resources": analysis.resource_summary(coordinator),
                "semantics": "descriptive_reviewer_declared_task_success",
                "scope": "one_task_repeated_attempts", "by_reviewer": comparisons,
                "efficiency": [efficiency.compare(coordinator, value, baseline, candidate, outcome_basis=basis)
                    for value in assessments or [None]],
                "comparison_accepted": False, "quality_uplift": None}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    export = sub.add_parser("export", help="freeze every planned attempt into a new review package")
    export.add_argument("batch", type=Path)
    export.add_argument("--output", type=Path, required=True)
    submit = sub.add_parser("import", help="retain one complete independent assessment without replacing prior reviews")
    submit.add_argument("export", type=Path)
    submit.add_argument("--assessment", type=Path, required=True)
    status = sub.add_parser("status", help="verify review evidence and account for all attempts")
    status.add_argument("export", type=Path)
    compare = sub.add_parser("compare", help="pair task outcomes under the shared original rubric, retaining unknowns")
    compare.add_argument("export", type=Path)
    compare.add_argument("--baseline", required=True)
    compare.add_argument("--candidate", required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "export":
            print(export_review(args.batch, args.output))
        elif args.command == "import":
            print(import_assessment(args.export, args.assessment))
        elif args.command == "compare":
            print(artifact_io.canonical(compare_review(args.export, args.baseline, args.candidate)).decode())
        else:
            print(artifact_io.canonical(review_status(args.export)).decode())
        return 0
    except (artifact_io.BenchmarkError, OSError, ValueError, TypeError, KeyError, AttributeError) as error:
        print(f"{getattr(error, 'code', 'review_error')}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
