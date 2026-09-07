#!/usr/bin/env python3
"""Validate published research calibrations without invoking models or indexers.

Checks structure and evidence reachability, not whether a claim follows from its
citations. Source review remains a separate, declared property of each key.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import re
import sys
import tempfile
from pathlib import Path

if __package__:
    from . import benchmark as bench
else:
    import benchmark as bench


DEFAULT_CORPUS = Path(__file__).resolve().parent / "benchmark/corpus.json"
DIMENSIONS = ("claim_to_source_support", "critical_evidence_coverage", "material_false_conclusions", "appropriate_unknowns")


def _fields(value, expected, code="corpus_schema"):
    if not isinstance(value, dict) or set(value) != set(expected):
        raise bench.BenchmarkError(code, "missing or unexpected corpus fields")


def _text(value, maximum=16384):
    if not isinstance(value, str) or not value.strip() or len(value.encode()) > maximum:
        raise bench.BenchmarkError("corpus_schema", "expected bounded nonempty text")
    return value


def _texts(value, minimum=1, maximum=32):
    if not isinstance(value, list) or not minimum <= len(value) <= maximum:
        raise bench.BenchmarkError("corpus_schema", "invalid corpus list length")
    for item in value:
        _text(item)
    if len(set(value)) != len(value):
        raise bench.BenchmarkError("corpus_schema", "duplicate corpus list item")
    return value


def load_corpus(path: Path) -> tuple[Path, dict]:
    root = path.parent.resolve(strict=True)
    value = bench.load_json(root / path.name)
    _fields(value, ("kind", "schema_version", "cases"))
    if (value["kind"] != "mastermind-research-corpus" or type(value["schema_version"]) is not int
            or value["schema_version"] != 1 or not isinstance(value["cases"], list)
            or not 1 <= len(value["cases"]) <= 32):
        raise bench.BenchmarkError("corpus_schema", "unsupported corpus or case count")
    seen = set()
    for entry in value["cases"]:
        _fields(entry, ("id", "role", "coverage", "task", "rubric", "indexed_files"))
        identifier = _text(entry["id"], 80)
        if not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", identifier) or identifier in seen:
            raise bench.BenchmarkError("corpus_case_id", "case IDs must be unique lowercase slugs")
        seen.add(identifier)
        if entry["role"] != "calibration":
            raise bench.BenchmarkError("corpus_role", "published cases are calibration, not held-out evidence")
        _texts(entry["coverage"], maximum=8)
        _texts(entry["indexed_files"], maximum=bench.SOURCE_FILE_LIMIT)
        for field, folder in (("task", "tasks"), ("rubric", "rubrics")):
            if entry[field] != f"{folder}/{identifier}.json":
                raise bench.BenchmarkError("corpus_path", "case files must match their ID within tasks/rubrics")
            parent = root / folder
            if parent.is_symlink() or parent.resolve() != parent:
                raise bench.BenchmarkError("corpus_path", "case directory cannot follow a symbolic link")
    return root, value


def validate_key(task: dict, key: dict) -> list[str]:
    _fields(key, ("task_id", "source_revision", "status", "required_knowns", "expected_unknowns",
                  "materially_false_claims", "reviewer_notes", "review_dimensions", "decision_quality",
                  "automatic_quality_score"), "corpus_key_schema")
    bench.validate_rubric(task, key)
    if (task["kind"] != "research" or key["status"] != "calibration_key_source_reviewed"
            or key["automatic_quality_score"] is not None
            or key["decision_quality"] != "not_applicable_to_facts_only_research_task"):
        raise bench.BenchmarkError("corpus_key_schema", "expected a source-reviewed research calibration without a score")
    if set(_texts(key["review_dimensions"])) != set(DIMENSIONS):
        raise bench.BenchmarkError("corpus_key_schema", "quality dimensions must assess evidence, not tool order")
    _texts(key["expected_unknowns"])
    _texts(key["materially_false_claims"])
    _texts(key["reviewer_notes"], minimum=0)
    facts = key["required_knowns"]
    if not isinstance(facts, list) or not 1 <= len(facts) <= 16:
        raise bench.BenchmarkError("corpus_key_schema", "a calibration needs explicit source-grounded facts")
    anchors = []
    for fact in facts:
        _fields(fact, ("claim", "anchors"), "corpus_key_schema")
        _text(fact["claim"])
        anchors.extend(_texts(fact["anchors"], maximum=16))
    return anchors


def source_records(repo: Path, task: dict) -> dict:
    records, total = {}, 0
    # Git is read-only and cannot fetch missing objects. A shallow checkout must
    # obtain the pinned revision explicitly before this check is run.
    with tempfile.TemporaryDirectory(prefix="mastermind-corpus-") as temporary:
        env = bench.clean_environment(Path(temporary))
        env.update(GIT_NO_LAZY_FETCH="1", GIT_ALLOW_PROTOCOL="")
        revision = task["revision"]
        try:
            commit = bench.git(repo, ["rev-parse", "--verify", revision + "^{commit}"], env).decode().strip()
            if commit != revision:
                raise bench.BenchmarkError("corpus_source_unavailable", "source revision must identify a commit")
            for path in task["source_allowlist"]:
                row = bench.git(repo, ["ls-tree", "-z", revision, "--", path], env)
                if not row.endswith(b"\0") or row.count(b"\0") != 1:
                    raise bench.BenchmarkError("corpus_source_unavailable", f"source is absent at the pinned revision: {path}")
                header, name = row[:-1].split(b"\t", 1)
                mode, kind, oid = header.decode("ascii").split()
                if name.decode() != path or mode not in ("100644", "100755") or kind != "blob":
                    raise bench.BenchmarkError("corpus_source_type", "only regular source files can supply corpus evidence")
                body = bench.git(repo, ["cat-file", "blob", oid], env)
                total += len(body)
                if total > bench.SOURCE_BYTE_LIMIT:
                    raise bench.BenchmarkError("corpus_source_limit", "case source exceeds the trial byte cap")
                text = body.decode("utf-8")
                records[path] = {"path": path, "git_mode": mode, "bytes": len(body),
                                 "sha256": hashlib.sha256(body).hexdigest(),
                                 "lines": text.count("\n") + int(bool(text) and not text.endswith("\n"))}
        except bench.BenchmarkError as error:
            if error.code == "git_failed":
                raise bench.BenchmarkError("corpus_source_unavailable", "cannot read the pinned Git source; ensure its objects are present locally") from error
            raise
        except (UnicodeError, ValueError) as error:
            raise bench.BenchmarkError("corpus_source_type", "source must be bounded UTF-8 text") from error
    return records


def validate_anchors(anchors: list[str], sources: dict) -> None:
    for anchor in anchors:
        path, separator, span = anchor.rpartition(":")
        if not separator or not re.fullmatch(r"[1-9][0-9]{0,9}(?:-[1-9][0-9]{0,9})?", span):
            raise bench.BenchmarkError("corpus_anchor_invalid", "anchors must be path:line or path:first-last")
        if path not in sources:
            raise bench.BenchmarkError("corpus_anchor_scope", "key evidence is outside the model-visible source allowlist")
        bounds = [int(value) for value in span.split("-")]
        if not 1 <= bounds[0] <= bounds[-1] <= sources[path]["lines"]:
            raise bench.BenchmarkError("corpus_anchor_range", f"anchor is outside the pinned source lines: {anchor}")


def _case(root: Path, registry: dict, entry: dict, source_repo: Path, registry_name: str) -> dict:
    task = bench.validate_task(bench.load_json(root / entry["task"]))
    if task["id"] != entry["id"]:
        raise bench.BenchmarkError("corpus_case_id", "public task ID differs from its corpus entry")
    source_repo = source_repo.resolve()
    controls = [root / registry_name]
    controls.extend(root / item[field] for item in registry["cases"] for field in ("task", "rubric"))
    for control in controls:
        try:
            relative = control.relative_to(source_repo).as_posix()
        except ValueError:
            continue
        if relative in task["source_allowlist"]:
            raise bench.BenchmarkError("corpus_source_control", "corpus registry, tasks and keys cannot be research source files")
    key = bench.load_json(root / entry["rubric"])
    anchors = validate_key(task, key)
    indexed = entry["indexed_files"]
    if any(path not in task["source_allowlist"] for path in indexed):
        raise bench.BenchmarkError("corpus_index_scope", "indexed files must be an explicit source subset")
    sources = source_records(source_repo, task)
    validate_anchors(anchors, sources)
    summary = {"id": entry["id"], "role": entry["role"], "coverage": entry["coverage"],
               "source_revision": task["revision"], "source_files": list(sources.values()),
               "indexed_files": sorted(indexed), "anchors_checked": len(anchors),
               "task_sha256": bench.digest(task), "rubric_sha256": bench.digest(key),
               "corpus_sha256": bench.digest(registry), "case_sha256": bench.digest(entry),
               "claim_semantics": "source_review_declared_not_machine_verified"}
    return {"task": task, "rubric": key, "summary": summary}


def select_case(path: Path, identifier: str, source_repo: Path) -> dict:
    root, registry = load_corpus(path)
    for entry in registry["cases"]:
        if entry["id"] == identifier:
            return _case(root, registry, entry, source_repo, path.name)
    raise bench.BenchmarkError("corpus_case_missing", f"unknown corpus case: {identifier}")


def configure_case(case: dict, config: dict) -> dict:
    result = copy.deepcopy(config)
    if not isinstance(result, dict):
        raise bench.BenchmarkError("corpus_config", "benchmark config must be an object")
    if "mmcg" in result:
        indexer = result["mmcg"]
        if not isinstance(indexer, dict):
            raise bench.BenchmarkError("corpus_config", "mmcg runtime config must be an object")
        expected = case["summary"]["indexed_files"]
        if "indexed_files" in indexer and sorted(_texts(indexer["indexed_files"], maximum=bench.SOURCE_FILE_LIMIT)) != expected:
            raise bench.BenchmarkError("corpus_index_conflict", "configured indexed_files differ from the selected case; omit that field to use the corpus subset")
        indexer["indexed_files"] = expected
    return result


def check_corpus(path: Path, source_repo: Path) -> dict:
    root, registry = load_corpus(path)
    summaries = [_case(root, registry, entry, source_repo, path.name)["summary"] for entry in registry["cases"]]
    return {"kind": "mastermind-research-corpus-check", "schema_version": 1,
            "corpus_sha256": bench.digest(registry), "cases": summaries,
            "comparison_accepted": False, "quality_uplift": None}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--source-repo", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        print(bench.canonical(check_corpus(args.corpus, args.source_repo)).decode())
        return 0
    except (bench.BenchmarkError, OSError, ValueError, TypeError, KeyError) as error:
        print(f"{getattr(error, 'code', 'corpus_error')}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
