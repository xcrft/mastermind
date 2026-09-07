"""Check corpus contracts against disposable, real Git source objects."""

import copy
import json
import os
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

from evals import benchmark as bench
from evals import benchmark_corpus as corpus
from evals import benchmark_review as review
from evals import test_benchmark as fixtures


@unittest.skipUnless(os.name == "posix" and shutil.which("git"), "requires POSIX and Git")
class CorpusTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.BenchmarkTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.root = self.fixture.root / "corpus"
        (self.root / "tasks").mkdir(parents=True)
        (self.root / "rubrics").mkdir()
        self.path = self.root / "corpus.json"
        self.task = copy.deepcopy(self.fixture.task)
        self.key = {"task_id": self.task["id"], "source_revision": self.task["revision"],
                    "status": "calibration_key_source_reviewed",
                    "required_knowns": [{"claim": "The return value is fixed.", "anchors": ["src/service.py:1-2"]}],
                    "expected_unknowns": ["Whether callers rely on the value."],
                    "materially_false_claims": ["The function performs network I/O."],
                    "reviewer_notes": ["FIXTURE_PRIVATE_KEY_CANARY"],
                    "review_dimensions": ["claim_to_source_support", "critical_evidence_coverage",
                                          "material_false_conclusions", "appropriate_unknowns"],
                    "decision_quality": "not_applicable_to_facts_only_research_task",
                    "automatic_quality_score": None}
        self.entry = {"id": self.task["id"], "role": "calibration", "coverage": ["fixed_return"],
                      "task": "tasks/service-01.json", "rubric": "rubrics/service-01.json",
                      "indexed_files": ["src/service.py"]}
        self.registry = {"kind": "mastermind-research-corpus", "schema_version": 1, "cases": [self.entry]}
        self.save()

    def save(self):
        for name, value in (("corpus.json", self.registry), (self.entry["task"], self.task), (self.entry["rubric"], self.key)):
            path = self.root / name
            path.write_bytes(bench.canonical(value))

    def check(self):
        return corpus.check_corpus(self.path, self.fixture.repo)

    def prepare_cli(self, selection, config=None, module=True):
        config_path = self.fixture.root / "config.json"
        config_path.write_bytes(bench.canonical(self.fixture.config if config is None else config))
        command = [sys.executable, "-m", "evals.benchmark"] if module else [sys.executable, str(Path(bench.__file__))]
        return subprocess.run([*command, "prepare", *selection, "--config", str(config_path),
            "--source-repo", str(self.fixture.repo), "--tool-repo", str(self.fixture.repo),
            "--output", str(self.fixture.root / "selected"), "--repetitions", "1"],
            capture_output=True, text=True, timeout=20)

    def test_checks_pinned_git_bytes_instead_of_current_working_tree(self):
        (self.fixture.repo / "src/service.py").write_text("Changed worktree\n")
        result = self.check()
        self.assertEqual(result["cases"][0]["source_revision"], self.task["revision"])
        self.assertEqual(result["cases"][0]["anchors_checked"], 1)
        self.assertFalse(result["comparison_accepted"])
        self.assertIsNone(result["quality_uplift"])
        self.assertNotIn("FIXTURE_PRIVATE_KEY_CANARY", json.dumps(result))

    def test_rejects_wrong_or_inaccessible_evidence_before_preparation(self):
        for anchor, reason in (("src/service.py:9", "corpus_anchor_range"),
                               ("src/service.py:2-1", "corpus_anchor_range"),
                               ("src/service.py:0", "corpus_anchor_invalid"),
                               ("evals/answer.json:1", "corpus_anchor_scope")):
            with self.subTest(anchor=anchor):
                self.key["required_knowns"][0]["anchors"] = [anchor]
                self.save()
                with self.assertRaises(bench.BenchmarkError) as raised:
                    self.check()
                self.assertEqual(raised.exception.code, reason)
        self.assertFalse((self.fixture.root / "trials").exists())

    def test_rejects_bad_key_identity_structure_and_claimed_scores(self):
        original = copy.deepcopy(self.key)
        for field, value in (("source_revision", "0" * 40), ("task_id", "wrong"),
                             ("required_knowns", []), ("materially_false_claims", []),
                             ("automatic_quality_score", 1), ("review_dimensions", ["tool_order"]),
                             ("status", "unreviewed")):
            with self.subTest(field=field):
                self.key = dict(original, **{field: value})
                self.save()
                with self.assertRaises(bench.BenchmarkError):
                    self.check()

    def test_rejects_unavailable_commit_without_fetching(self):
        self.task["revision"] = self.key["source_revision"] = "0" * 40
        self.save()
        with self.assertRaises(bench.BenchmarkError) as raised:
            self.check()
        self.assertEqual(raised.exception.code, "corpus_source_unavailable")

    def test_rejects_duplicate_cases_bad_index_scope_and_control_paths(self):
        original = copy.deepcopy(self.registry)
        for mode in ("duplicate", "index_scope", "index_duplicate", "path", "held_out"):
            with self.subTest(mode=mode):
                self.registry = copy.deepcopy(original)
                entry = self.registry["cases"][0]
                if mode == "duplicate":
                    self.registry["cases"].append(copy.deepcopy(entry))
                elif mode == "index_scope":
                    entry["indexed_files"] = ["not-allowed.py"]
                elif mode == "index_duplicate":
                    entry["indexed_files"] *= 2
                elif mode == "path":
                    entry["rubric"] = "../rubric.json"
                else:
                    entry["role"] = "held_out"
                self.save()
                with self.assertRaises(bench.BenchmarkError):
                    self.check()

    def test_selected_case_prepares_all_conditions_with_its_index_scope(self):
        config = copy.deepcopy(self.fixture.config)
        config["mmcg"].pop("indexed_files")
        result = self.prepare_cli(["--case", "service-01", "--corpus", str(self.path)], config)
        self.assertEqual(result.returncode, 0, result.stderr)
        batch = Path(result.stdout.strip())
        value = bench.load_json(batch / "batch.json")
        self.assertEqual(value["corpus_case"]["role"], "calibration")
        self.assertEqual(value["corpus_case"]["id"], "service-01")
        self.assertEqual(len(value["trials"]), 3)
        self.assertFalse(list(batch.rglob("adapter-called")))
        for item in value["trials"]:
            trial = batch / item["directory"]
            manifest = bench.load_json(trial / "manifest.json")
            self.assertEqual(manifest["status"], "prepared")
            request = bench.load_json(trial / "request.json")
            self.assertNotIn("FIXTURE_PRIVATE_KEY_CANARY", json.dumps(request))
            self.assertNotIn("coverage", request)
            if item["condition"] == "portable_mmcg":
                self.assertEqual(manifest["indexed_files"], ["src/service.py"])
            self.assertEqual(bench.run_trial(trial)["run_status"]["state"], "completed")

    def test_case_selection_rejects_conflicting_explicit_index_config(self):
        case = corpus.select_case(self.path, "service-01", self.fixture.repo)
        config = copy.deepcopy(self.fixture.config)
        config["mmcg"]["indexed_files"] = ["docs/guide.md"]
        with self.assertRaises(bench.BenchmarkError) as raised:
            corpus.configure_case(case, config)
        self.assertEqual(raised.exception.code, "corpus_index_conflict")

    def test_cli_rejects_bad_selection_before_any_trial_or_runtime(self):
        base = ["--case", "service-01", "--corpus", str(self.path)]
        cases = [(["--case", "missing", "--corpus", str(self.path)], "corpus_case_missing"),
                 ([*base, "--rubric", str(self.root / self.entry["rubric"])], "invalid_selection"),
                 (["--task", str(self.root / self.entry["task"])], "invalid_selection"),
                 (base, "corpus_anchor_range")]
        self.key["required_knowns"][0]["anchors"] = ["src/service.py:9"]
        self.save()
        for module in (True, False):
            for selection, code in cases:
                with self.subTest(module=module, code=code):
                    result = self.prepare_cli(selection, module=module)
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertTrue(result.stderr.startswith(code + ":"), result.stderr)
                    self.assertNotIn("Traceback", result.stderr)
                    self.assertFalse((self.fixture.root / "selected").exists())
        self.assertFalse(list(self.fixture.root.rglob("adapter-called")))
        self.assertFalse(list(self.fixture.root.rglob("indexer-called")))

    def test_cli_rejects_conflicting_index_scope_before_any_trial(self):
        config = copy.deepcopy(self.fixture.config)
        config["mmcg"]["indexed_files"] = ["docs/guide.md"]
        result = self.prepare_cli(["--case", "service-01", "--corpus", str(self.path)], config)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertTrue(result.stderr.startswith("corpus_index_conflict:"), result.stderr)
        self.assertFalse((self.fixture.root / "selected").exists())

    def test_portable_instruction_cannot_leak_into_the_source_baseline(self):
        self.task["source_allowlist"].append(self.fixture.config["instruction_path"])
        self.save()
        result = self.prepare_cli(["--case", "service-01", "--corpus", str(self.path)])
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertTrue(result.stderr.startswith("corpus_instruction_source:"), result.stderr)
        self.assertFalse((self.fixture.root / "selected").exists())
        self.assertFalse(list(self.fixture.root.rglob("adapter-called")))
        self.assertFalse(list(self.fixture.root.rglob("indexer-called")))

    def test_custom_corpus_controls_cannot_leak_through_pinned_source(self):
        destination = self.fixture.repo / "benchmark-fixture"
        self.root.rename(destination)
        self.root, self.path = destination, destination / "corpus.json"
        other = dict(self.entry, id="other-01", task="tasks/other-01.json", rubric="rubrics/other-01.json")
        self.registry["cases"].append(other)
        (self.root / other["rubric"]).write_bytes(bench.canonical(self.key))
        self.save()
        self.fixture.git("add", "benchmark-fixture")
        self.fixture.git("commit", "-qm", "historical corpus controls")
        self.task["revision"] = self.key["source_revision"] = self.fixture.git("rev-parse", "HEAD").strip()
        self.save()
        self.assertEqual(corpus.select_case(self.path, "service-01", self.fixture.repo)["task"], self.task)
        sources = list(self.task["source_allowlist"])
        for control in ("corpus.json", self.entry["task"], self.entry["rubric"], other["rubric"]):
            with self.subTest(control=control):
                self.task["source_allowlist"] = [*sources, "benchmark-fixture/" + control]
                self.save()
                result = self.prepare_cli(["--case", "service-01", "--corpus", str(self.path)])
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertTrue(result.stderr.startswith("corpus_source_control:"), result.stderr)
                self.assertFalse((self.fixture.root / "selected").exists())

    def test_corpus_files_and_case_directories_cannot_be_symlinks(self):
        for name in ("corpus.json", "tasks/service-01.json", "rubrics/service-01.json", "tasks", "rubrics"):
            with self.subTest(name=name):
                original = self.root / name
                target = self.fixture.root / "moved"
                original.rename(target)
                original.symlink_to(target, target_is_directory=target.is_dir())
                try:
                    with self.assertRaises(bench.BenchmarkError):
                        self.check()
                finally:
                    original.unlink()
                    target.rename(original)

    def test_git_symlinks_and_non_utf8_cannot_supply_evidence(self):
        source = self.fixture.repo / "src/service.py"
        for symlink in (True, False):
            with self.subTest(symlink=symlink):
                source.unlink()
                if symlink:
                    source.symlink_to("../docs/guide.md")
                else:
                    source.write_bytes(b"\xff\n")
                self.fixture.git("add", "src/service.py")
                self.fixture.git("commit", "-qm", "non-source fixture")
                self.task["revision"] = self.key["source_revision"] = self.fixture.git("rev-parse", "HEAD").strip()
                self.save()
                with self.assertRaises(bench.BenchmarkError) as raised:
                    self.check()
                self.assertEqual(raised.exception.code, "corpus_source_type")


@unittest.skipUnless(os.name == "posix" and shutil.which("git"), "requires POSIX and full corpus Git history")
class BundledCorpusTests(unittest.TestCase):
    def test_every_published_case_survives_the_prepare_run_review_workflow(self):
        fixture = fixtures.BenchmarkTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        source_repo = Path(__file__).resolve().parents[1]
        _, registry = corpus.load_corpus(corpus.DEFAULT_CORPUS)
        answer = "Fixture answer for artifact round-trip only."
        fixture.adapter(f"init()\nfinal({answer!r})\n")
        for entry in registry["cases"]:
            with self.subTest(case=entry["id"]):
                fixture.indexer(paths=entry["indexed_files"])
                config = copy.deepcopy(fixture.config)
                config["mmcg"].pop("indexed_files")
                config_path = fixture.root / "config.json"
                config_path.write_bytes(bench.canonical(config))
                prepared = subprocess.run([sys.executable, "-m", "evals.benchmark", "prepare",
                    "--case", entry["id"], "--corpus", str(corpus.DEFAULT_CORPUS),
                    "--config", str(config_path), "--source-repo", str(source_repo),
                    "--tool-repo", str(fixture.repo), "--output", str(fixture.root / "batches"),
                    "--repetitions", "1"], cwd=source_repo, capture_output=True, text=True, timeout=30)
                self.assertEqual(prepared.returncode, 0, prepared.stderr)
                batch = Path(prepared.stdout.strip())
                manifest = bench.load_json(batch / "batch.json")
                self.assertEqual(manifest["corpus_case"]["id"], entry["id"])
                self.assertEqual([slot["condition"] for slot in manifest["trials"]], list(bench.CONDITIONS))
                for slot in manifest["trials"]:
                    trial = batch / slot["directory"]
                    trial_manifest = bench.load_json(trial / "manifest.json")
                    self.assertEqual(trial_manifest["status"], "prepared", trial_manifest.get("setup_error"))
                    self.assertEqual(bench.run_trial(trial)["run_status"]["state"], "completed")

                output = fixture.root / (entry["id"] + "-review")
                review.export_review(batch, output)
                packet = bench.load_json(output / "reviewer/packet.json")
                self.assertEqual(packet["task"]["id"], entry["id"])
                self.assertEqual(packet["rubric_sha256"], manifest["corpus_case"]["rubric_sha256"])
                self.assertEqual({item["path"]: item for item in packet["source_files"]},
                                 {item["path"]: item for item in manifest["corpus_case"]["source_files"]})
                assessment = bench.load_json(output / "reviewer/assessment-template.json")
                assessment["reviewer"] = "fixture-check"
                for item in assessment["reviews"]:
                    item["claims"] = [{"quote": answer, "support": "unknown", "anchors": [],
                        "material_error": False, "rationale": "Transport fixture, not a researched answer."}]
                    for known in item["knowns"]:
                        known.update(coverage="missing", rationale="The fixture contains no source findings.")
                    for unknown in item["unknowns"]:
                        unknown.update(handling="omitted", rationale="The fixture makes no research assessment.")
                assessment_path = fixture.root / "assessment.json"
                assessment_path.write_bytes(bench.canonical(assessment))
                review.import_assessment(output, assessment_path)
                status = review.review_status(output)
                self.assertEqual(status["attempts"], {"planned": 3, "completed": 3, "failed": 0,
                    "not_run": 0, "unfinished": 0, "missing_artifacts": 0, "with_answer": 3})
                self.assertEqual(status["reviewed_attempts"], 3)
                self.assertFalse(status["comparison_accepted"])
                self.assertIsNone(status["quality_uplift"])


if __name__ == "__main__":
    unittest.main()
