#!/usr/bin/env python3
"""Export blinded research answers and retain independently submitted reviews.

Offline artifact verification never runs a model, Git, an indexer or researched
code. Hashes bind artifacts, not their authorship or semantic correctness.
"""

from __future__ import annotations

import argparse
import re
import secrets
import sys
import uuid
from pathlib import Path, PurePosixPath

if __package__:
    from . import benchmark as bench, benchmark_corpus as corpus
    from .benchmark_review_io import Root, OUTPUT_LIMIT, encoded, sha
else:
    import benchmark as bench
    import benchmark_corpus as corpus
    from benchmark_review_io import Root, OUTPUT_LIMIT, encoded, sha


RUN_STATES = {"completed", "setup_error", "timeout", "output_limit", "protocol_error",
              "identity_mismatch", "invocation_error", "model_error", "budget_exceeded", "input_changed"}
SLOT_STATES = RUN_STATES | {"not_run", "unfinished", "missing_artifacts"}


def require(condition, message, code="review_schema"):
    if not condition:
        raise bench.BenchmarkError(code, message)


def fields(value, required, optional=()):
    require(isinstance(value, dict) and set(required) <= set(value)
            and not set(value) - set(required) - set(optional), "missing or unexpected review fields")


def text(value, maximum=16384):
    require(isinstance(value, str) and bool(value.strip()) and len(value.encode()) <= maximum,
            "expected bounded nonempty review text")
    return value


def identifier(value, prefix):
    require(isinstance(value, str) and re.fullmatch(prefix + r"[0-9a-f]{32}", value), "invalid artifact ID")
    return value


def hash_value(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value), "invalid artifact hash")
    return value


def file_records(value, task, lines=False):
    require(isinstance(value, list) and len(value) == len(task["source_allowlist"]), "invalid source inventory")
    records = {}
    for item in value:
        fields(item, ("path", "bytes", "sha256", "git_mode", *(["lines"] if lines else [])))
        path = bench.safe_source_path(item["path"])
        require(path not in records and path in task["source_allowlist"], "source scope differs from the task")
        require(type(item["bytes"]) is int and 0 <= item["bytes"] <= bench.FILE_BYTE_LIMIT, "invalid source size")
        require(item["git_mode"] in ("100644", "100755"), "invalid source mode")
        hash_value(item["sha256"])
        if lines:
            require(type(item["lines"]) is int and 0 <= item["lines"] <= item["bytes"], "invalid source line count")
        records[path] = item
    require(sum(item["bytes"] for item in value) <= bench.SOURCE_BYTE_LIMIT, "source byte cap exceeded", "review_limit")
    return records


def line_count(body):
    body.decode("utf-8")
    return body.count(b"\n") + int(bool(body) and not body.endswith(b"\n"))


def check_batch(batch, names):
    fields(batch, ("kind", "schema_version", "task_id", "repetitions", "trials", "quality_uplift", "comparison_accepted"), ("corpus_case",))
    require(batch["kind"] == "mastermind-research-batch" and type(batch["schema_version"]) is int
            and batch["schema_version"] == 1, "unsupported benchmark batch")
    repetitions = batch["repetitions"]
    require(type(repetitions) is int and 1 <= repetitions <= 20, "invalid repetition count")
    require(batch["quality_uplift"] is None and batch["comparison_accepted"] is False, "batch cannot declare accepted quality")
    require(isinstance(batch["trials"], list) and len(batch["trials"]) == 3 * repetitions,
            "batch must retain every planned condition and repetition", "review_inventory")
    seen = set()
    for position, item in enumerate(batch["trials"]):
        fields(item, ("directory", "condition", "repetition", "common_sha256", "status"))
        identifier(item["directory"], "trial-")
        require(item["directory"] not in seen, "duplicate trial directory", "review_inventory")
        seen.add(item["directory"])
        repetition, offset = divmod(position, 3)
        require(type(item["repetition"]) is int and item["repetition"] == repetition
                and item["condition"] == bench.CONDITIONS[(repetition + offset) % 3],
                "condition/repetition order differs from the planned matrix", "review_inventory")
        require(item["status"] in ("prepared", "setup_failed"), "invalid preparation status")
        if item["common_sha256"] is not None:
            hash_value(item["common_sha256"])
    require(not {name for name in names if name.startswith("trial-")} - seen,
            "batch omits trial artifacts present in its directory", "review_inventory")


def check_manifest(manifest, item, batch):
    require(isinstance(manifest, dict), "invalid trial manifest")
    for field in ("kind", "schema_version", "trial_id", "condition", "repetition", "task", "status",
                  "rubric_sha256", "model", "tool_revision", "limits", "isolation"):
        require(field in manifest, "incomplete trial manifest")
    require(manifest["kind"] == "mastermind-research-trial" and type(manifest["schema_version"]) is int
            and manifest["schema_version"] in (1, 2), "unsupported trial manifest")
    for key, expected in (("trial_id", item["directory"]), ("condition", item["condition"]),
                          ("repetition", item["repetition"]), ("status", item["status"]),
                          ("common_sha256", item["common_sha256"])):
        require(manifest.get(key) == expected, "trial differs from its batch slot", "review_identity")
    require(type(manifest["repetition"]) is int and isinstance(manifest["task"], dict), "invalid trial fields")
    bench.validate_task(manifest["task"])
    require(manifest["task"]["id"] == batch["task_id"], "trial task differs from batch", "review_identity")
    hash_value(manifest["rubric_sha256"])
    text(manifest["model"])
    bench.exact_revision(manifest["tool_revision"])
    require(bench.validate_limits(manifest["limits"]) == manifest["limits"], "trial limits are incomplete")
    require(manifest["isolation"] == "host_adapter_unverified", "unsupported isolation claim")
    if manifest["status"] == "prepared":
        for field in ("adapter", "source_files", "source_sha256", "common_sha256", "condition_sha256",
                      "projection_revision", "instruction_sha256", "request_sha256"):
            require(field in manifest, "prepared trial is missing its identity")
    if manifest.get("source_files") is not None:
        file_records(manifest["source_files"], manifest["task"])
        require(bench.digest({"revision": manifest["task"]["revision"], "files": manifest["source_files"]})
                == manifest.get("source_sha256"), "source identity changed", "review_identity")
    try:
        if manifest.get("common_sha256") is not None:
            require(bench.digest(bench.common_identity(manifest)) == manifest["common_sha256"],
                    "common identity changed", "review_identity")
        if manifest.get("condition_sha256") is not None:
            require(bench.digest(bench.condition_identity(manifest)) == manifest["condition_sha256"],
                    "condition identity changed", "review_identity")
    except (KeyError, TypeError) as error:
        raise bench.BenchmarkError("review_identity", "trial identity fields are incomplete") from error


def check_request(request, manifest):
    require(isinstance(request, dict) and bench.digest(request) == manifest["request_sha256"],
            "prepared request changed", "review_identity")
    instruction = request.get("portable_instruction")
    require(isinstance(instruction, str) and sha(instruction.encode()) == manifest["instruction_sha256"]
            and (manifest["condition"] != "source" or not instruction), "instruction changed", "review_identity")
    # Derive the old location lexically. A review archive may have moved and no
    # old executable, source directory or SQLite index needs to remain installed.
    source = PurePosixPath(text(request.get("source_root")))
    require(source.is_absolute() and ".." not in source.parts and source.name == "source"
            and source.parent.name == manifest["trial_id"], "invalid original trial location")
    require(request == bench.adapter_request(Path(source.parent), manifest, instruction),
            "request differs from the frozen manifest", "review_identity")


def read_sources(root, prefix, records, check_mode=True):
    bodies, described = {}, []
    for item in records:
        path = prefix + item["path"]
        body = root.read(path, bench.FILE_BYTE_LIMIT)
        require(len(body) == item["bytes"] and sha(body) == item["sha256"], "source bytes changed", "review_source")
        if check_mode:
            mode = "100755" if root.records[path]["identity"][2] & 0o100 else "100644"
            require(mode == item["git_mode"], "source mode changed", "review_source")
        bodies[item["path"]] = body
        described.append(dict(item, lines=line_count(body)))
    return bodies, described


def read_result(root, prefix, manifest, manifest_body):
    body = root.read(prefix + "result.json", optional=True)
    if body is None:
        lock = root.read(prefix + "run.lock", limit=0, optional=True)
        state = "setup_error" if manifest["status"] == "setup_failed" else "unfinished" if lock is not None else "not_run"
        return state, None, None
    result = bench.parse_json(body)
    fields(result, ("kind", "schema_version", "trial_id", "manifest_sha256", "common_sha256", "condition_sha256",
                    "run_status", "quality", "diagnostics", "answer", "comparability"))
    require(result["kind"] == "mastermind-research-result" and type(result["schema_version"]) is int
            and result["schema_version"] == 1, "unsupported result")
    require(result["trial_id"] == manifest["trial_id"] and result["manifest_sha256"] == sha(manifest_body)
            and result["common_sha256"] == manifest.get("common_sha256")
            and result["condition_sha256"] == manifest.get("condition_sha256"), "result identity differs from manifest", "review_identity")
    fields(result["run_status"], ("state", "reason"))
    state = result["run_status"]["state"]
    require(isinstance(state, str) and state in RUN_STATES, "unknown run state")
    if result["run_status"]["reason"] is not None:
        text(result["run_status"]["reason"], 1024)
    require(state != "completed" or result["run_status"]["reason"] is None, "completed run cannot declare a failure")
    fields(result["quality"], ("status", "score"))
    require(result["quality"]["score"] is None and isinstance(result["comparability"], dict)
            and result["comparability"].get("eligible") is False,
            "result cannot declare measured or accepted quality")
    answer = result["answer"]
    require(state != "completed" or answer is not None, "completed run needs a retained answer")
    require(result["quality"]["status"] == ("review_pending" if answer is not None else "not_evaluated"), "invalid answer review state")
    if manifest["status"] == "setup_failed":
        require(state == "setup_error" and answer is None, "failed preparation cannot produce an answer")
    answer_body = None
    if answer is not None:
        fields(answer, ("path", "bytes", "sha256"))
        require(answer["path"] == "answer.md" and type(answer["bytes"]) is int
                and 0 < answer["bytes"] <= manifest["limits"]["answer_bytes"], "invalid answer descriptor")
        hash_value(answer["sha256"])
        answer_body = root.read(prefix + "answer.md", manifest["limits"]["answer_bytes"])
        require(len(answer_body) == answer["bytes"] and sha(answer_body) == answer["sha256"]
                and bool(answer_body.decode("utf-8").strip()), "retained answer changed", "review_answer")
    return state, sha(body), answer_body


def collect_batch(root):
    batch = root.json("batch.json")
    check_batch(batch, root.names())
    context = None
    common = None
    indexed_subsets = []
    source_bodies, sources = {}, []
    slots, answers = [], {}
    for item in batch["trials"]:
        prefix = item["directory"] + "/"
        body = root.read(prefix + "manifest.json", optional=True)
        slot = {"review_id": "review-" + uuid.uuid4().hex, "trial_id": item["directory"],
                "condition": item["condition"], "repetition": item["repetition"], "preparation_status": item["status"],
                "status": "missing_artifacts", "manifest_sha256": None, "result_sha256": None,
                "answer_sha256": None, "source_integrity": "unavailable"}
        slots.append(slot)
        if body is None:
            continue
        manifest = bench.parse_json(body)
        check_manifest(manifest, item, batch)
        key = root.json(prefix + "rubric.json")
        corpus.validate_key(manifest["task"], key)
        require(bench.digest(key) == manifest["rubric_sha256"], "review key changed", "review_identity")
        candidate = {field: manifest[field] for field in ("task", "model", "limits", "tool_revision")}
        candidate["rubric"] = key
        require(context is None or candidate == context, "batch mixes tasks, keys or runtime settings", "review_identity")
        context = candidate
        if manifest.get("common_sha256") is not None:
            require(common is None or common == manifest["common_sha256"], "batch mixes common identities", "review_identity")
            common = manifest["common_sha256"]
        if manifest["status"] == "prepared":
            check_request(root.json(prefix + "request.json"), manifest)
        if manifest["condition"] == "portable_mmcg" and "indexed_files" in manifest:
            indexed_subsets.append(manifest["indexed_files"])
        if manifest.get("source_files") is not None:
            try:
                bodies, described = read_sources(root, prefix + "source/", manifest["source_files"])
                require(not sources or described == sources, "batch source snapshots differ", "review_identity")
                source_bodies, sources = bodies, described
                slot["source_integrity"] = "verified"
            except (bench.BenchmarkError, UnicodeError) as error:
                if getattr(error, "code", None) in ("review_limit", "review_identity"):
                    raise
                slot["source_integrity"] = "unavailable"
        state, result_hash, answer = read_result(root, prefix, manifest, body)
        slot.update(status=state, manifest_sha256=sha(body), result_sha256=result_hash,
                    answer_sha256=sha(answer) if answer is not None else None)
        if answer is not None:
            answers[slot["review_id"]] = answer
        require(sum(map(len, answers.values())) + sum(map(len, source_bodies.values())) <= OUTPUT_LIMIT - 3 * bench.CONTROL_BYTE_LIMIT,
                "retained review evidence exceeds the output cap", "review_limit")
    require(context is not None, "no intact task and key remain in the batch", "review_context")
    require(not answers or sources, "answers need an intact source snapshot from the same batch", "review_source")
    if sources:
        corpus.validate_anchors(corpus.validate_key(context["task"], context["rubric"]), {item["path"]: item for item in sources})
    if "corpus_case" in batch:
        case = batch["corpus_case"]
        require(isinstance(case, dict) and case.get("id") == context["task"]["id"]
                and case.get("role") == "calibration" and case.get("source_revision") == context["task"]["revision"]
                and case.get("task_sha256") == bench.digest(context["task"])
                and case.get("rubric_sha256") == bench.digest(context["rubric"]), "corpus binding differs from the batch", "review_identity")
        records = file_records(case.get("source_files"), context["task"], lines=True)
        require(not sources or records == {item["path"]: item for item in sources}, "corpus source differs from trial evidence", "review_identity")
        indexed = case.get("indexed_files")
        require(isinstance(indexed, list) and 1 <= len(indexed) <= len(records)
                and all(isinstance(path, str) and path in records for path in indexed)
                and len(set(indexed)) == len(indexed), "invalid corpus indexed subset")
        require(all(isinstance(subset, list) and sorted(subset) == sorted(indexed) for subset in indexed_subsets),
                "corpus indexed subset differs from the graph trials", "review_identity")
    root.recheck()
    return context, slots, answers, source_bodies, sources, batch


def attempt_counts(slots):
    counts = dict(planned=len(slots), completed=0, failed=0, not_run=0, unfinished=0, missing_artifacts=0, with_answer=0)
    for slot in slots:
        state = slot["status"]
        counts[state if state in ("completed", "not_run", "unfinished", "missing_artifacts") else "failed"] += 1
        counts["with_answer"] += slot["answer_sha256"] is not None
    return counts


def export_review(batch: Path, output: Path):
    destination = output.parent.resolve(strict=True) / output.name
    with Root(batch) as root:
        require(not destination.is_relative_to(root.path), "review output must be outside the batch", "review_path")
        context, slots, answers, bodies, sources, batch_value = collect_batch(root)
        export_id = "export-" + uuid.uuid4().hex
        shuffled = slots.copy()
        secrets.SystemRandom().shuffle(shuffled)
        items = [{"review_id": slot["review_id"], "answer": {
            "path": "answers/" + slot["review_id"] + ".md", "bytes": len(answers[slot["review_id"]]),
            "sha256": slot["answer_sha256"]} if slot["review_id"] in answers else None} for slot in shuffled]
        packet = {"kind": "mastermind-research-review-packet", "schema_version": 1, "export_id": export_id,
                  "task": context["task"], "rubric": context["rubric"], "rubric_sha256": bench.digest(context["rubric"]),
                  "source_files": sources, "items": items, "answer_text_may_reveal_condition": True,
                  "comparison_accepted": False, "quality_uplift": None}
        packet_body = encoded(packet)
        coordinator = {"kind": "mastermind-research-review-coordinator", "schema_version": 1,
                       "export_id": export_id, "packet_sha256": sha(packet_body), "batch_sha256": root.records["batch.json"]["sha256"],
                       "model": context["model"], "tool_revision": context["tool_revision"],
                       "corpus_case": batch_value.get("corpus_case"), "slots": slots}
        coordinator_body = encoded(coordinator)
        template = {"kind": "mastermind-research-assessment", "schema_version": 1, "export_id": export_id,
                    "packet_sha256": sha(packet_body), "reviewer": None, "reviews": []}
        for item in items:
            if item["answer"] is not None:
                template["reviews"].append({"review_id": item["review_id"], "answer_sha256": item["answer"]["sha256"],
                    "rubric_sha256": packet["rubric_sha256"], "claims": [],
                    "knowns": [{"known_index": index, "coverage": None, "answer_excerpt": None, "rationale": None}
                               for index in range(len(context["rubric"]["required_knowns"]))],
                    "unknowns": [{"unknown_index": index, "handling": None, "answer_excerpt": None, "rationale": None}
                                 for index in range(len(context["rubric"]["expected_unknowns"]))]})
        template_body = encoded(template)
        try:
            destination.mkdir(mode=0o700)
        except FileExistsError as error:
            raise bench.BenchmarkError("review_exists", "choose a new export directory; replacement is not allowed") from error
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
    return destination


def load_export(root):
    seal = root.json("seal.json")
    fields(seal, ("kind", "schema_version", "export_id", "packet_sha256", "coordinator_sha256", "template_sha256"))
    require(seal["kind"] == "mastermind-research-review-seal" and type(seal["schema_version"]) is int
            and seal["schema_version"] == 1, "unsupported review export")
    identifier(seal["export_id"], "export-")
    packet = root.json("reviewer/packet.json")
    coordinator = root.json("coordinator.json")
    template = root.read("reviewer/assessment-template.json")
    require(root.records["reviewer/packet.json"]["sha256"] == seal["packet_sha256"]
            and root.records["coordinator.json"]["sha256"] == seal["coordinator_sha256"]
            and sha(template) == seal["template_sha256"], "review export changed", "review_identity")
    fields(packet, ("kind", "schema_version", "export_id", "task", "rubric", "rubric_sha256", "source_files", "items",
                    "answer_text_may_reveal_condition", "comparison_accepted", "quality_uplift"))
    require(packet["kind"] == "mastermind-research-review-packet" and type(packet["schema_version"]) is int
            and packet["schema_version"] == 1 and packet["export_id"] == seal["export_id"]
            and packet["comparison_accepted"] is False and packet["quality_uplift"] is None
            and packet["answer_text_may_reveal_condition"] is True, "invalid review packet")
    bench.validate_task(packet["task"])
    anchors = corpus.validate_key(packet["task"], packet["rubric"])
    require(bench.digest(packet["rubric"]) == packet["rubric_sha256"], "packet key changed", "review_identity")
    sources = {}
    if packet["source_files"]:
        sources = file_records(packet["source_files"], packet["task"], lines=True)
        _, described = read_sources(root, "reviewer/source/", list(sources.values()), check_mode=False)
        require(described == packet["source_files"], "source line counts changed", "review_source")
        corpus.validate_anchors(anchors, sources)
    require(isinstance(packet["items"], list) and 3 <= len(packet["items"]) <= 60, "invalid review item count")
    answers, items = {}, {}
    retained_bytes = sum(item["bytes"] for item in sources.values())
    for item in packet["items"]:
        fields(item, ("review_id", "answer"))
        identifier(item["review_id"], "review-")
        require(item["review_id"] not in items, "duplicate review item")
        items[item["review_id"]] = item
        answer = item["answer"]
        if answer is not None:
            fields(answer, ("path", "bytes", "sha256"))
            require(answer["path"] == "answers/" + item["review_id"] + ".md" and type(answer["bytes"]) is int
                    and 0 < answer["bytes"] <= 16 * 1024 * 1024, "invalid review answer")
            body = root.read("reviewer/" + answer["path"], 16 * 1024 * 1024)
            retained_bytes += len(body)
            require(retained_bytes <= OUTPUT_LIMIT, "review export exceeds its byte cap", "review_limit")
            require(len(body) == answer["bytes"] and sha(body) == answer["sha256"], "review answer changed", "review_answer")
            answers[item["review_id"]] = body.decode("utf-8")
    require(not answers or sources, "review answers lack source evidence", "review_source")
    root.inventory({"reviewer/packet.json", "reviewer/assessment-template.json"}
                   | {"reviewer/source/" + path for path in sources}
                   | {"reviewer/" + item["answer"]["path"] for item in items.values() if item["answer"] is not None})
    fields(coordinator, ("kind", "schema_version", "export_id", "packet_sha256", "batch_sha256", "model", "tool_revision", "corpus_case", "slots"))
    require(coordinator["kind"] == "mastermind-research-review-coordinator" and type(coordinator["schema_version"]) is int
            and coordinator["schema_version"] == 1 and coordinator["export_id"] == seal["export_id"]
            and coordinator["packet_sha256"] == seal["packet_sha256"], "coordinator identity changed", "review_identity")
    require(isinstance(coordinator["slots"], list) and len(coordinator["slots"]) == len(items), "coordinator omitted attempts", "review_inventory")
    require(len(items) % 3 == 0, "coordinator has an incomplete condition matrix", "review_inventory")
    seen = set()
    for slot in coordinator["slots"]:
        fields(slot, ("review_id", "trial_id", "condition", "repetition", "preparation_status", "status", "manifest_sha256",
                      "result_sha256", "answer_sha256", "source_integrity"))
        require(isinstance(slot["review_id"], str) and slot["review_id"] in items and slot["review_id"] not in seen
                and isinstance(slot["status"], str) and slot["status"] in SLOT_STATES
                and slot["source_integrity"] in ("verified", "unavailable"), "invalid coordinator slot")
        seen.add(slot["review_id"])
        for field in ("manifest_sha256", "result_sha256", "answer_sha256"):
            if slot[field] is not None:
                hash_value(slot[field])
        answer = items[slot["review_id"]]["answer"]
        require(slot["answer_sha256"] == (answer["sha256"] if answer is not None else None), "answer mapping changed", "review_identity")
        if slot["status"] == "missing_artifacts":
            require(slot["manifest_sha256"] is None and slot["result_sha256"] is None and answer is None
                    and slot["source_integrity"] == "unavailable", "missing attempt cannot declare retained evidence")
        else:
            hash_value(slot["manifest_sha256"])
        if slot["status"] in ("not_run", "unfinished"):
            require(slot["result_sha256"] is None and answer is None and slot["preparation_status"] == "prepared",
                    "unfinished attempt cannot declare a result")
        if slot["status"] in RUN_STATES - {"setup_error"}:
            hash_value(slot["result_sha256"])
        if slot["preparation_status"] == "setup_failed":
            require(slot["status"] in ("setup_error", "missing_artifacts") and answer is None,
                    "failed preparation cannot declare an answer")
        require(slot["status"] != "completed" or answer is not None, "completed attempt lacks an answer")
    check_batch({"kind": "mastermind-research-batch", "schema_version": 1, "task_id": packet["task"]["id"],
                 "repetitions": len(items) // 3, "quality_uplift": None, "comparison_accepted": False,
                 "trials": [{"directory": slot["trial_id"], "condition": slot["condition"], "repetition": slot["repetition"],
                             "status": slot["preparation_status"], "common_sha256": None} for slot in coordinator["slots"]]}, set())
    return seal, packet, coordinator, answers, sources


def check_assessment(value, seal, packet, answers, sources):
    fields(value, ("kind", "schema_version", "export_id", "packet_sha256", "reviewer", "reviews"))
    require(value["kind"] == "mastermind-research-assessment" and type(value["schema_version"]) is int
            and value["schema_version"] == 1, "unsupported assessment")
    require(value["export_id"] == seal["export_id"] and value["packet_sha256"] == seal["packet_sha256"],
            "assessment belongs to a different or changed export", "review_identity")
    require(isinstance(value["reviewer"], str) and re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", value["reviewer"]),
            "reviewer must be a stable lowercase label")
    require(isinstance(value["reviews"], list) and len(value["reviews"]) == len(answers),
            "assessment must account for every retained answer", "review_inventory")
    seen = set()
    for item in value["reviews"]:
        fields(item, ("review_id", "answer_sha256", "rubric_sha256", "claims", "knowns", "unknowns"))
        review_id = item["review_id"]
        require(isinstance(review_id, str) and review_id in answers and review_id not in seen, "unknown or duplicate reviewed answer", "review_inventory")
        seen.add(review_id)
        answer = answers[review_id]
        require(item["answer_sha256"] == sha(answer.encode()) and item["rubric_sha256"] == packet["rubric_sha256"],
                "assessment answer or key changed", "review_identity")
        require(isinstance(item["claims"], list) and 1 <= len(item["claims"]) <= 64, "review needs bounded claim-level evidence")
        quotes = set()
        for claim in item["claims"]:
            fields(claim, ("quote", "support", "anchors", "material_error", "rationale"))
            quote = text(claim["quote"])
            require(quote in answer and quote not in quotes, "claim must quote a distinct passage of the retained answer", "review_quote")
            quotes.add(quote)
            require(claim["support"] in ("supported", "unsupported", "contradicted", "unknown")
                    and type(claim["material_error"]) is bool, "invalid claim assessment")
            require(not claim["material_error"] or claim["support"] in ("unsupported", "contradicted"), "material errors need unsupported or contradicted claims")
            text(claim["rationale"])
            require(isinstance(claim["anchors"], list) and len(claim["anchors"]) <= 16, "invalid claim evidence")
            for anchor in claim["anchors"]:
                text(anchor, 1024)
            require(len(set(claim["anchors"])) == len(claim["anchors"]), "duplicate claim evidence")
            require(claim["anchors"] or claim["support"] in ("unsupported", "unknown"), "supported or contradicted claims require source anchors")
            corpus.validate_anchors(claim["anchors"], sources)
        for name, key, expected, field, choices, absent in (
                ("knowns", "known_index", packet["rubric"]["required_knowns"], "coverage", ("covered", "partial", "missing"), "missing"),
                ("unknowns", "unknown_index", packet["rubric"]["expected_unknowns"], "handling", ("appropriate", "overclaimed", "omitted"), "omitted")):
            rows = item[name]
            require(isinstance(rows, list) and len(rows) == len(expected), "assessment omits required rubric dimensions", "review_inventory")
            indexes = set()
            for row in rows:
                fields(row, (key, field, "answer_excerpt", "rationale"))
                index = row[key]
                require(type(index) is int and 0 <= index < len(expected) and index not in indexes, "invalid or duplicate rubric index")
                indexes.add(index)
                require(row[field] in choices, "rubric assessment is unfinished or invalid")
                text(row["rationale"])
                if row[field] == absent:
                    require(row["answer_excerpt"] is None, "absent evidence cannot claim an answer excerpt")
                else:
                    require(text(row["answer_excerpt"]) in answer, "rubric excerpt is absent from the answer", "review_quote")


def receipt_names(root):
    if "reviews" not in root.names():
        return set()
    entries = root.names("reviews")
    # An interrupted exclusive publication may leave its unlinked staging file.
    # It is not an admitted assessment and is never read or sent to a reviewer.
    receipts = {name for name in entries if not re.fullmatch(r"\.pending-[0-9a-f]{32}", name)}
    require(len(receipts) <= 64, "too many review receipts", "review_limit")
    require(all(re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}\.json", name) for name in receipts),
            "unexpected review receipt file")
    return receipts


def import_assessment(export: Path, assessment: Path):
    with Root(export) as root, Root(assessment.absolute().parent) as submitted, root.exclusive_lock("import.lock"):
        seal, packet, _, answers, sources = load_export(root)
        value = submitted.json(assessment.name)
        check_assessment(value, seal, packet, answers, sources)
        existing = receipt_names(root)
        require(value["reviewer"] + ".json" not in existing, "reviewer already submitted an assessment", "review_exists")
        require(len(existing) < 64, "reviewer admission limit reached", "review_limit")
        receipt = {"kind": "mastermind-research-assessment-receipt", "schema_version": 1,
                   "export_id": seal["export_id"], "packet_sha256": seal["packet_sha256"],
                   "coordinator_sha256": seal["coordinator_sha256"], "assessment_sha256": bench.digest(value),
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
        reviewers = []
        for name in sorted(receipt_names(root)):
            receipt = root.json("reviews/" + name)
            fields(receipt, ("kind", "schema_version", "export_id", "packet_sha256", "coordinator_sha256", "assessment_sha256",
                             "assessment", "semantics", "comparison_accepted", "quality_uplift"))
            require(receipt["kind"] == "mastermind-research-assessment-receipt" and type(receipt["schema_version"]) is int
                    and receipt["schema_version"] == 1 and receipt["export_id"] == seal["export_id"]
                    and receipt["packet_sha256"] == seal["packet_sha256"]
                    and receipt["coordinator_sha256"] == seal["coordinator_sha256"]
                    and receipt["assessment_sha256"] == bench.digest(receipt["assessment"])
                    and receipt["comparison_accepted"] is False and receipt["quality_uplift"] is None
                    and receipt["semantics"] == "reviewer_declared_not_machine_verified", "review receipt changed", "review_identity")
            check_assessment(receipt["assessment"], seal, packet, answers, sources)
            require(name == receipt["assessment"]["reviewer"] + ".json", "reviewer identity differs from its receipt", "review_identity")
            reviewers.append({"reviewer": receipt["assessment"]["reviewer"], "reviewed": len(receipt["assessment"]["reviews"])})
        root.recheck()
        return {"kind": "mastermind-research-review-status", "schema_version": 1, "export_id": seal["export_id"],
                "attempts": attempt_counts(coordinator["slots"]), "reviewers": reviewers,
                "reviewed_attempts": len(answers) if reviewers else 0,
                "attempts_without_verified_source": sum(slot["source_integrity"] != "verified" for slot in coordinator["slots"]),
                "assessment_semantics": "reviewer_declared_not_machine_verified", "comparison_accepted": False, "quality_uplift": None}


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
    args = parser.parse_args(argv)
    try:
        if args.command == "export":
            print(export_review(args.batch, args.output))
        elif args.command == "import":
            print(import_assessment(args.export, args.assessment))
        else:
            print(bench.canonical(review_status(args.export)).decode())
        return 0
    except (bench.BenchmarkError, OSError, ValueError, TypeError, KeyError, AttributeError) as error:
        print(f"{getattr(error, 'code', 'review_error')}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
