"""Check corpus contracts against disposable, real Git source objects."""

from pathlib import Path
import copy
import json
import os
import shutil
import subprocess
import sys
import unittest

from evals.benchmark import artifacts as artifact_io
from evals.benchmark import conditions as condition_contract
from evals.benchmark import corpus
from evals.benchmark import review
from evals.benchmark import trials as trial_runner
from tests.evals.support.benchmark import BenchmarkFixture
from tests.evals.support.corpus import CorpusFixture


@unittest.skipUnless(os.name == "posix" and shutil.which("git"), "requires POSIX and Git")
class CorpusTests(unittest.TestCase):
    def setUp(self):
        self.case = CorpusFixture()
        self.addCleanup(self.case.close)
        self.fixture = self.case.fixture


    def test_checks_pinned_git_bytes_instead_of_current_working_tree(self):
        (self.fixture.repo / "src/service.py").write_text("Changed worktree\n")
        result = self.case.check()
        self.assertEqual(result["cases"][0]["source_revision"], self.case.task["revision"])
        self.assertEqual(result["cases"][0]["anchors_checked"], 1)
        self.assertFalse(result["comparison_accepted"])
        self.assertIsNone(result["quality_uplift"])
        self.assertNotIn("FIXTURE_PRIVATE_KEY_CANARY", json.dumps(result))

    def test_current_check_allows_unrelated_commits_but_rejects_source_drift_even_if_worktree_is_restored(self):
        source = self.fixture.repo / "src/service.py"
        original = source.read_bytes()
        (self.fixture.repo / "unrelated.md").write_text("Instructions changed.\n")
        self.fixture.git("add", "unrelated.md")
        self.fixture.git("commit", "-qm", "update unrelated instructions")
        result = corpus.check_corpus(self.case.path, self.fixture.repo, require_current=True)
        self.assertEqual(result["cases"][0]["source_revision"], self.case.task["revision"])
        self.assertNotEqual(self.fixture.git("rev-parse", "HEAD").strip(), self.case.task["revision"])
        source.write_text("def value():\n    return 9\n")
        self.fixture.git("add", "src/service.py")
        self.fixture.git("commit", "-qm", "change reviewed behavior")
        source.write_bytes(original)
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            corpus.check_corpus(self.case.path, self.fixture.repo, require_current=True)
        self.assertEqual(raised.exception.code, "corpus_revision_stale")


    def test_rejects_wrong_or_inaccessible_evidence_before_preparation(self):
        for anchor, reason in (("src/service.py:9", "corpus_anchor_range"),
                               ("src/service.py:2-1", "corpus_anchor_range"),
                               ("src/service.py:0", "corpus_anchor_invalid"),
                               ("evals/answer.json:1", "corpus_anchor_scope")):
            with self.subTest(anchor=anchor):
                self.case.key["required_knowns"][0]["anchors"] = [anchor]
                self.case.save()
                with self.assertRaises(artifact_io.BenchmarkError) as raised:
                    self.case.check()
                self.assertEqual(raised.exception.code, reason)
        self.assertFalse((self.fixture.root / "trials").exists())


    def test_rejects_bad_key_identity_structure_and_claimed_scores(self):
        original = copy.deepcopy(self.case.key)
        for field, value in (("source_revision", "0" * 40), ("task_id", "wrong"),
                             ("required_knowns", []), ("materially_false_claims", []),
                             ("automatic_quality_score", 1), ("review_dimensions", ["tool_order"]),
                             ("status", "unreviewed")):
            with self.subTest(field=field):
                self.case.key = dict(original, **{field: value})
                self.case.save()
                with self.assertRaises(artifact_io.BenchmarkError):
                    self.case.check()


    def test_rejects_unavailable_commit_without_fetching(self):
        self.case.task["revision"] = self.case.key["source_revision"] = "0" * 40
        self.case.save()
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            self.case.check()
        self.assertEqual(raised.exception.code, "corpus_source_unavailable")


    def test_rejects_duplicate_cases_bad_index_scope_and_control_paths(self):
        original = copy.deepcopy(self.case.registry)
        for mode in ("duplicate", "index_scope", "index_duplicate", "path", "held_out"):
            with self.subTest(mode=mode):
                self.case.registry = copy.deepcopy(original)
                entry = self.case.registry["cases"][0]
                if mode == "duplicate":
                    self.case.registry["cases"].append(copy.deepcopy(entry))
                elif mode == "index_scope":
                    entry["indexed_files"] = ["not-allowed.py"]
                elif mode == "index_duplicate":
                    entry["indexed_files"] *= 2
                elif mode == "path":
                    entry["rubric"] = "../rubric.json"
                else:
                    entry["role"] = "held_out"
                self.case.save()
                with self.assertRaises(artifact_io.BenchmarkError):
                    self.case.check()


    def test_selected_case_prepares_all_conditions_with_its_index_scope(self):
        config = copy.deepcopy(self.fixture.config)
        config["mmcg"].pop("indexed_files")
        result = self.case.prepare_cli(["--case", "service-01", "--corpus", str(self.case.path)], config)
        self.assertEqual(result.returncode, 0, result.stderr)
        batch = Path(result.stdout.strip())
        value = artifact_io.load_json(batch / "batch.json")
        self.assertEqual(value["corpus_case"]["role"], "calibration")
        self.assertEqual(value["corpus_case"]["id"], "service-01")
        self.assertEqual(len(value["trials"]), 3)
        self.assertFalse(list(batch.rglob("adapter-called")))
        for item in value["trials"]:
            trial = batch / item["directory"]
            manifest = artifact_io.load_json(trial / "manifest.json")
            self.assertEqual(manifest["status"], "prepared")
            request = artifact_io.load_json(trial / "request.json")
            self.assertNotIn("FIXTURE_PRIVATE_KEY_CANARY", json.dumps(request))
            self.assertNotIn("coverage", request)
            if item["condition"] == "portable_mmcg":
                self.assertEqual(manifest["indexed_files"], ["src/service.py"])
            self.assertEqual(trial_runner.run_trial(trial)["run_status"]["state"], "completed")


    def test_case_selection_rejects_conflicting_explicit_index_config(self):
        case = corpus.select_case(self.case.path, "service-01", self.fixture.repo)
        config = copy.deepcopy(self.fixture.config)
        config["mmcg"]["indexed_files"] = ["docs/guide.md"]
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            corpus.configure_case(case, config)
        self.assertEqual(raised.exception.code, "corpus_index_conflict")


    def test_cli_rejects_bad_selection_before_any_trial_or_runtime(self):
        base = ["--case", "service-01", "--corpus", str(self.case.path)]
        cases = [(["--case", "missing", "--corpus", str(self.case.path)], "corpus_case_missing"),
                 ([*base, "--rubric", str(self.case.root / self.case.entry["rubric"])], "invalid_selection"),
                 (["--task", str(self.case.root / self.case.entry["task"])], "invalid_selection"),
                 (base, "corpus_anchor_range")]
        self.case.key["required_knowns"][0]["anchors"] = ["src/service.py:9"]
        self.case.save()
        for module in (True, False):
            for selection, code in cases:
                with self.subTest(module=module, code=code):
                    result = self.case.prepare_cli(selection, module=module)
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertTrue(result.stderr.startswith(code + ":"), result.stderr)
                    self.assertNotIn("Traceback", result.stderr)
                    self.assertFalse((self.fixture.root / "selected").exists())
        self.assertFalse(list(self.fixture.root.rglob("adapter-called")))
        self.assertFalse(list(self.fixture.root.rglob("indexer-called")))


    def test_cli_rejects_conflicting_index_scope_before_any_trial(self):
        config = copy.deepcopy(self.fixture.config)
        config["mmcg"]["indexed_files"] = ["docs/guide.md"]
        result = self.case.prepare_cli(["--case", "service-01", "--corpus", str(self.case.path)], config)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertTrue(result.stderr.startswith("corpus_index_conflict:"), result.stderr)
        self.assertFalse((self.fixture.root / "selected").exists())


    def test_portable_instruction_cannot_leak_into_the_source_baseline(self):
        self.case.task["source_allowlist"].append(self.fixture.config["instruction_path"])
        self.case.save()
        result = self.case.prepare_cli(["--case", "service-01", "--corpus", str(self.case.path)])
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertTrue(result.stderr.startswith("corpus_instruction_source:"), result.stderr)
        self.assertFalse((self.fixture.root / "selected").exists())
        self.assertFalse(list(self.fixture.root.rglob("adapter-called")))
        self.assertFalse(list(self.fixture.root.rglob("indexer-called")))


    def test_custom_corpus_controls_cannot_leak_through_pinned_source(self):
        destination = self.fixture.repo / "benchmark-fixture"
        self.case.root.rename(destination)
        self.case.root, self.case.path = destination, destination / "corpus.json"
        other = dict(self.case.entry, id="other-01", task="tasks/other-01.json", rubric="rubrics/other-01.json")
        self.case.registry["cases"].append(other)
        (self.case.root / other["rubric"]).write_bytes(artifact_io.canonical(self.case.key))
        self.case.save()
        self.fixture.git("add", "benchmark-fixture")
        self.fixture.git("commit", "-qm", "historical corpus controls")
        self.case.task["revision"] = self.case.key["source_revision"] = self.fixture.git("rev-parse", "HEAD").strip()
        self.case.save()
        self.assertEqual(corpus.select_case(self.case.path, "service-01", self.fixture.repo)["task"], self.case.task)
        sources = list(self.case.task["source_allowlist"])
        for control in ("corpus.json", self.case.entry["task"], self.case.entry["rubric"], other["rubric"]):
            with self.subTest(control=control):
                self.case.task["source_allowlist"] = [*sources, "benchmark-fixture/" + control]
                self.case.save()
                result = self.case.prepare_cli(["--case", "service-01", "--corpus", str(self.case.path)])
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertTrue(result.stderr.startswith("corpus_source_control:"), result.stderr)
                self.assertFalse((self.fixture.root / "selected").exists())


    def test_corpus_files_and_case_directories_cannot_be_symlinks(self):
        for name in ("corpus.json", "tasks/service-01.json", "rubrics/service-01.json", "tasks", "rubrics"):
            with self.subTest(name=name):
                original = self.case.root / name
                target = self.fixture.root / "moved"
                original.rename(target)
                original.symlink_to(target, target_is_directory=target.is_dir())
                try:
                    with self.assertRaises(artifact_io.BenchmarkError):
                        self.case.check()
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
                self.case.task["revision"] = self.case.key["source_revision"] = self.fixture.git("rev-parse", "HEAD").strip()
                self.case.save()
                with self.assertRaises(artifact_io.BenchmarkError) as raised:
                    self.case.check()
                self.assertEqual(raised.exception.code, "corpus_source_type")



@unittest.skipUnless(os.name == "posix" and shutil.which("git"), "requires POSIX and full corpus Git history")
class BundledCorpusTests(unittest.TestCase):
    def test_every_published_case_survives_the_prepare_run_review_workflow(self):
        fixture = BenchmarkFixture()
        self.addCleanup(fixture.close)
        source_repo = Path(__file__).resolve().parents[3]
        _, registry = corpus.load_corpus(corpus.DEFAULT_CORPUS)
        answer = "Fixture answer for artifact round-trip only."
        fixture.adapter(f"init()\nfinal({answer!r})\n")
        for entry in registry["cases"]:
            with self.subTest(case=entry["id"]):
                fixture.indexer(paths=entry["indexed_files"])
                config = copy.deepcopy(fixture.config)
                config["mmcg"].pop("indexed_files")
                config_path = fixture.root / "config.json"
                config_path.write_bytes(artifact_io.canonical(config))
                prepared = subprocess.run([sys.executable, "-m", "evals.benchmark", "prepare",
                    "--case", entry["id"], "--corpus", str(corpus.DEFAULT_CORPUS),
                    "--config", str(config_path), "--source-repo", str(source_repo),
                    "--tool-repo", str(fixture.repo), "--output", str(fixture.root / "batches"),
                    "--repetitions", "1"], cwd=source_repo, capture_output=True, text=True, timeout=30)
                self.assertEqual(prepared.returncode, 0, prepared.stderr)
                batch = Path(prepared.stdout.strip())
                manifest = artifact_io.load_json(batch / "batch.json")
                self.assertEqual(manifest["corpus_case"]["id"], entry["id"])
                self.assertEqual([slot["condition"] for slot in manifest["trials"]], list(condition_contract.CONDITIONS))
                for slot in manifest["trials"]:
                    trial = batch / slot["directory"]
                    trial_manifest = artifact_io.load_json(trial / "manifest.json")
                    self.assertEqual(trial_manifest["status"], "prepared", trial_manifest.get("setup_error"))
                    self.assertEqual(trial_runner.run_trial(trial)["run_status"]["state"], "completed")

                output = fixture.root / (entry["id"] + "-review")
                review.export_review(batch, output)
                packet = artifact_io.load_json(output / "reviewer/packet.json")
                self.assertEqual(packet["task"]["id"], entry["id"])
                self.assertEqual(packet["rubric_sha256"], manifest["corpus_case"]["rubric_sha256"])
                self.assertEqual({item["path"]: item for item in packet["source_files"]},
                                 {item["path"]: item for item in manifest["corpus_case"]["source_files"]})
                assessment = artifact_io.load_json(output / "reviewer/assessment-template.json")
                assessment["reviewer"] = "fixture-check"
                for item in assessment["reviews"]:
                    item["outcome"] = {"status": "unsatisfied", "rationale": "The transport fixture does not answer the research task."}
                    item["claims"] = [{"quote": answer, "support": "unknown", "anchors": [],
                        "material_error": False, "rationale": "Transport fixture, not a researched answer."}]
                    for known in item["knowns"]:
                        known.update(coverage="missing", rationale="The fixture contains no source findings.")
                    for unknown in item["unknowns"]:
                        unknown.update(handling="omitted", rationale="The fixture makes no research assessment.")
                    for criterion in item.get("acceptance", []):
                        criterion.update(status="unmet", rationale="The transport fixture does not satisfy this requested result.")
                assessment_path = fixture.root / "assessment.json"
                assessment_path.write_bytes(artifact_io.canonical(assessment))
                review.import_assessment(output, assessment_path)
                status = review.review_status(output)
                self.assertEqual(status["attempts"], {"planned": 3, "completed": 3, "failed": 0,
                    "not_run": 0, "unfinished": 0, "missing_artifacts": 0, "with_answer": 3})
                self.assertEqual(status["reviewed_attempts"], 3)
                self.assertFalse(status["comparison_accepted"])
                self.assertIsNone(status["quality_uplift"])


if __name__ == "__main__":
    unittest.main()
