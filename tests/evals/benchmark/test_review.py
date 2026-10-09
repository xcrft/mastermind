"""Offline review through real trial files, Git fixtures and CLI subprocesses."""

from pathlib import Path
from unittest.mock import patch
import copy
import json
import os
import shutil
import subprocess
import sys
import unittest

from evals.benchmark import artifacts as artifact_io
from evals.benchmark import corpus
from evals.benchmark import protocol as model_protocol
from evals.benchmark import review
from evals.benchmark import review_io
from evals.benchmark import trials as trial_runner
from evals.benchmark.review_io import Root
from tests.evals.support.corpus import CorpusFixture


@unittest.skipUnless(os.name == "posix" and shutil.which("git"), "requires POSIX and Git")
class ReviewTests(unittest.TestCase):
    def setUp(self):
        self.case = CorpusFixture()
        self.addCleanup(self.case.close)
        self.fixture = self.case.fixture
        self.root = self.fixture.root
        self.output = self.root / "review-export"
        self.fixture.config["model"] = "PRIVATE_MODEL_METADATA_CANARY"

    def batch(self, repetitions=1, run=True, corpus_case=None):
        batch = trial_runner.prepare_batch(task=self.case.task, rubric=self.case.key,
            config=self.fixture.config, source_repo=self.fixture.repo, tool_repo=self.fixture.repo,
            output=self.root / "batches", repetitions=repetitions, corpus_case=corpus_case)
        trials = [batch / item["directory"] for item in artifact_io.load_json(batch / "batch.json")["trials"]]
        if run:
            for trial in trials:
                trial_runner.run_trial(trial)
        return batch, trials

    def cli(self, *arguments):
        return subprocess.run([sys.executable, "-m", "evals.benchmark.review", *map(str, arguments)],
                              capture_output=True, text=True, timeout=20, env=dict(os.environ, PATH=""))

    def stateful_adapter(self):
        self.fixture.adapter('''
init()
marker = root.parent / 'test-mode'
mode = marker.read_text() if marker.exists() else ''
if mode == 'mutate':
    source = root / 'src/service.py'
    source.chmod(0o644)
    source.write_text('def value():\\n    return 99\\n')
final(model_error=mode == 'partial')
''')

    def assessment(self, reviewer="alice"):
        value = artifact_io.load_json(self.output / "reviewer/assessment-template.json")
        value["reviewer"] = reviewer
        for item in value["reviews"]:
            item["outcome"] = {"status": "unsatisfied", "rationale": "The required value and uncertainty are omitted."}
            item["claims"] = [{"quote": "Observed src/service.py:2.", "support": "supported",
                               "anchors": ["src/service.py:2"], "material_error": False,
                               "rationale": "The cited function returns the fixed value."}]
            item["knowns"] = [{"known_index": 0, "coverage": "partial",
                               "answer_excerpt": "Observed src/service.py:2.",
                               "rationale": "It cites the source but does not state the value."}]
            item["unknowns"] = [{"unknown_index": 0, "handling": "omitted", "answer_excerpt": None,
                                 "rationale": "Caller reliance is not discussed."}]
        return value

    def submit(self, value):
        path = self.root / "assessment.json"
        path.write_bytes(artifact_io.canonical(value))
        return review.import_assessment(self.output, path)

    def test_real_cli_exports_and_imports_without_runtime_or_batch_access(self):
        batch, trials = self.batch()
        originals = {trial.name: (trial / "result.json").read_bytes() for trial in trials}
        moved = batch.with_name("relocated-batch")
        batch.rename(moved)
        batch, trials = moved, [moved / trial.name for trial in trials]
        Path(self.fixture.config["adapter"]["path"]).unlink()
        Path(self.fixture.config["mmcg"]["path"]).unlink()
        result = self.cli("export", batch, "--output", self.output)
        self.assertEqual(result.returncode, 0, result.stderr)
        packet = artifact_io.load_json(self.output / "reviewer/packet.json")
        self.assertEqual(len(packet["items"]), 3)
        self.assertEqual(packet["task"], self.case.task)
        self.assertEqual(packet["rubric"], self.case.key)
        self.assertEqual(len({item["review_id"] for item in packet["items"]}), 3)
        for item in packet["items"]:
            self.assertEqual((self.output / "reviewer" / item["answer"]["path"]).read_text(),
                             "Observed src/service.py:2.")
        for path in (self.output / "reviewer").rglob("*"):
            if path.is_file():
                body = path.read_bytes()
                for private in [b"PRIVATE_MODEL_METADATA_CANARY", *[trial.name.encode() for trial in trials]]:
                    self.assertNotIn(private, body)
        for trial in trials:
            self.assertEqual((trial / "result.json").read_bytes(), originals[trial.name])
        batch.rename(batch.with_name("archived-batch"))
        submission = self.root / "assessment.json"
        submission.write_bytes(artifact_io.canonical(self.assessment()))
        result = self.cli("import", self.output, "--assessment", submission)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.cli("status", self.output)
        self.assertEqual(result.returncode, 0, result.stderr)
        status = json.loads(result.stdout)
        self.assertEqual(status["attempts"], {"planned": 3, "completed": 3, "failed": 0,
                         "not_run": 0, "unfinished": 0, "missing_artifacts": 0, "with_answer": 3})
        self.assertEqual(status["reviewed_attempts"], 3)
        self.assertEqual(status["reviewers"], [{"reviewer": "alice", "reviewed": 3}])
        self.assertEqual(status["execution_order_integrity"], "verified")
        summary = status["assessment_summary"]
        self.assertEqual(summary["overall"]["answer_assessments"], 3)
        self.assertEqual(summary["overall"]["reviewer_selected_claims"]["supported"], 3)
        self.assertEqual(summary["overall"]["known_coverage"], {"covered": 0, "partial": 3, "missing": 0})
        self.assertEqual(summary["overall"]["unknown_handling"], {"appropriate": 0, "overclaimed": 0, "omitted": 3})
        self.assertFalse(status["comparison_accepted"])
        self.assertIsNone(status["quality_uplift"])

    def test_two_and_four_condition_exports_keep_controls_private_and_accept_legacy_assessments(self):
        self.fixture.condition_matrix()
        matrix = copy.deepcopy(self.fixture.config["conditions"])
        for count in (2, 4):
            with self.subTest(conditions=count):
                self.fixture.config["conditions"] = matrix[:count]
                self.output = self.root / ("review-export-" + str(count))
                batch, _ = self.batch()
                self.assertEqual(self.cli("export", batch, "--output", self.output).returncode, 0)
                for path in (self.output / "reviewer").rglob("*"):
                    if path.is_file():
                        body = path.read_bytes()
                        self.assertNotIn(b"PROFILE_CONTROL_CANARY", body)
                        self.assertNotIn(b"prompts/profile.md", body)
                        self.assertNotIn(b'"condition"', body)
                pending = review.compare_review(self.output, "raw", "refined")
                self.assertEqual(pending["by_reviewer"][0]["success_delta"], {"lower": -1.0, "upper": 1.0, "point": None})
                self.assertEqual(pending["attempts"]["planned"], count)
                alice = self.assessment()
                self.submit(alice)
                legacy = self.assessment("legacy")
                legacy["schema_version"] = 1
                for item in legacy["reviews"]:
                    item.pop("outcome")
                self.submit(legacy)
                result = self.cli("compare", self.output, "--baseline", "raw", "--candidate", "refined")
                self.assertEqual(result.returncode, 0, result.stderr)
                comparison = json.loads(result.stdout)
                self.assertEqual(comparison["by_reviewer"][0]["success_delta"]["point"], 0)
                self.assertIsNone(comparison["by_reviewer"][1]["success_delta"]["point"])
                self.assertEqual(comparison["execution_order_integrity"], "verified")
                self.assertFalse(comparison["comparison_accepted"])
                self.assertIsNone(comparison["quality_uplift"])
                status = review.review_status(self.output)
                self.assertEqual(len(status["assessment_summary"]["by_condition"]), count)
                self.assertEqual(status["assessment_summary"]["overall"]["task_outcomes"],
                                 {"satisfied": 0, "unsatisfied": count, "unknown": count})

    def test_paired_comparison_retains_failed_partial_answers_unknowns_and_reviewer_disagreement(self):
        self.fixture.condition_matrix()
        self.fixture.config["conditions"] = self.fixture.config["conditions"][:2]
        self.fixture.adapter('''
init()
marker = root.parent / 'test-mode'
final('Observed src/service.py:2. value() returns 7. Caller reliance is unknown.',
      model_error=marker.exists(), cost_usd=None if marker.exists() else 0.2)
''')
        batch, trials = self.batch(repetitions=3, run=False)
        (trials[2] / "test-mode").touch()
        for trial in trials[:5]:
            trial_runner.run_trial(trial)
        review.export_review(batch, self.output)
        coordinator = artifact_io.load_json(self.output / "coordinator.json")
        slots = {slot["review_id"]: slot for slot in coordinator["slots"]}
        alice = self.assessment()
        for item in alice["reviews"]:
            slot = slots[item["review_id"]]
            if slot["repetition"] == 1 or (slot["condition"] == "refined" and slot["repetition"] == 0):
                item["outcome"] = {"status": "satisfied", "rationale": "The answer gives the return value and admits caller uncertainty."}
                item["knowns"][0].update(coverage="covered", answer_excerpt="value() returns 7.")
                item["unknowns"][0].update(handling="appropriate", answer_excerpt="Caller reliance is unknown.")
        self.submit(alice)
        bob = copy.deepcopy(alice)
        bob["reviewer"] = "bob"
        for item in bob["reviews"]:
            if slots[item["review_id"]]["condition"] == "refined" and slots[item["review_id"]]["repetition"] == 0:
                item["outcome"] = {"status": "unknown", "rationale": "I cannot judge completeness from this answer."}
        self.submit(bob)
        comparison = review.compare_review(self.output, "raw", "refined")
        first, second = comparison["by_reviewer"]
        self.assertEqual(comparison["attempts"]["failed"], 1)
        self.assertEqual(comparison["attempts"]["not_run"], 1)
        self.assertEqual(first["planned_pairs"], 3)
        self.assertEqual(first["resolved_pairs"], 2)
        self.assertEqual((first["wins"], first["ties"], first["losses"]), (1, 0, 1))
        self.assertEqual(first["success_delta"], {"lower": 0.0, "upper": 1 / 3, "point": None})
        self.assertEqual(first["pairs"][1]["candidate"]["reason"], "failed_attempt")
        self.assertEqual(first["pairs"][1]["candidate"]["success"], {"lower": 0, "upper": 0})
        self.assertEqual(second["success_delta"], {"lower": -1 / 3, "upper": 1 / 3, "point": None})
        resources = {row["condition"]: row["metrics"] for row in comparison["resources"]["by_condition"]}
        self.assertEqual(resources["refined"]["cost_usd"],
                         {"observed_total": 0.2, "measured_trials": 1, "unknown_trials": 2, "complete": False})
        self.assertEqual(resources["refined"]["input_tokens"]["observed_total"], 50)
        self.assertEqual(resources["raw"]["input_tokens"]["observed_total"], 75)
        self.assertTrue(resources["raw"]["cost_usd"]["complete"])
        self.assertEqual(review.review_status(self.output)["assessment_summary"]["disagreement"]["task_outcomes_with_disagreement"], 1)
        for baseline, candidate in (("raw", "raw"), ("missing", "refined")):
            with self.assertRaises(artifact_io.BenchmarkError) as raised:
                review.compare_review(self.output, baseline, candidate)
            self.assertEqual(raised.exception.code, "review_contrast")

    def test_old_coordinators_remain_readable_without_inventing_costs_or_runtime_checks(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        original = artifact_io.load_json(self.output / "coordinator.json")
        seal = artifact_io.load_json(self.output / "seal.json")
        (self.output / "coordinator.json").chmod(0o600)
        (self.output / "seal.json").chmod(0o600)
        for version in (1, 2):
            with self.subTest(coordinator_version=version):
                value = copy.deepcopy(original)
                value["schema_version"] = version
                if version == 1:
                    value.pop("batch")
                    value.pop("execution_order_integrity")
                for slot in value["slots"]:
                    slot.pop("resources")
                    slot.pop("runtime_contract")
                    if version == 1:
                        slot.pop("batch_execution")
                        slot.pop("execution_order")
                body = review.encoded(value)
                (self.output / "coordinator.json").write_bytes(body)
                seal["coordinator_sha256"] = review.sha(body)
                (self.output / "seal.json").write_bytes(review.encoded(seal))
                status = review.review_status(self.output)
                self.assertEqual(status["attempts"]["planned"], 3)
                self.assertEqual(status["resources"]["by_condition"][0]["metrics"]["cost_usd"],
                                 {"observed_total": None, "measured_trials": 0, "unknown_trials": 1, "complete": False})
                self.assertIsNone(review.compare_review(self.output, "source", "portable")["by_reviewer"][0]["success_delta"]["point"])

    def test_legacy_completed_budget_violation_cannot_count_as_task_success(self):
        self.fixture.adapter("init()\nfinal(turns=100)\n")
        batch, trials = self.batch()
        path = trials[-1] / "result.json"
        legacy = artifact_io.load_json(path)
        legacy["run_status"] = {"state": "completed", "reason": None}
        path.write_bytes(artifact_io.canonical(legacy))
        review.export_review(batch, self.output)
        comparison = review.compare_review(self.output, "source", "portable_mmcg")
        self.assertEqual(comparison["attempts"]["completed"], 1)
        self.assertEqual(comparison["by_reviewer"][0]["pairs"][0]["candidate"]["reason"], "failed_runtime_contract")
        self.assertEqual(comparison["by_reviewer"][0]["success_delta"], {"lower": 0.0, "upper": 0.0, "point": 0.0})

    def test_task_success_cannot_contradict_its_rubric_or_claim_evidence(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        for mutation in ("known", "unknown", "claim", "unfinished"):
            with self.subTest(mutation=mutation):
                value = self.assessment()
                item = value["reviews"][0]
                item["outcome"] = {"status": "satisfied", "rationale": "Declared success."}
                item["knowns"][0]["coverage"] = "covered"
                item["unknowns"][0].update(handling="appropriate", answer_excerpt="Observed src/service.py:2.")
                if mutation == "known":
                    item["knowns"][0]["coverage"] = "partial"
                elif mutation == "unknown":
                    item["unknowns"][0].update(handling="omitted", answer_excerpt=None)
                elif mutation == "claim":
                    item["claims"][0].update(support="contradicted", material_error=True)
                else:
                    item["outcome"]["status"] = None
                with self.assertRaises(artifact_io.BenchmarkError):
                    self.submit(value)
                self.assertFalse((self.output / "reviews/alice.json").exists())

    def test_accounts_for_failed_missing_unfinished_and_not_run_attempts(self):
        self.stateful_adapter()
        self.fixture.config["mmcg"]["path"] = str(self.root / "absent-native-runtime")
        batch, trials = self.batch(repetitions=3, run=False)
        trial_runner.run_trial(trials[0])
        (trials[1] / "test-mode").write_text("partial")
        trial_runner.run_trial(trials[1])
        trial_runner.run_trial(trials[2])
        (trials[3] / "run.lock").touch()
        (trials[3] / "answer.md").write_text("ORPHAN_ANSWER_MUST_NOT_BE_REVIEWED")
        (trials[7] / "manifest.json").unlink()
        trials[8].rename(self.root / "missing-trial")
        review.export_review(batch, self.output)
        status = review.review_status(self.output)
        self.assertEqual(status["attempts"], {"planned": 9, "completed": 1, "failed": 4,
                         "not_run": 1, "unfinished": 1, "missing_artifacts": 2, "with_answer": 2})
        self.assertEqual(status["execution_order_integrity"], "partial")
        packet = artifact_io.load_json(self.output / "reviewer/packet.json")
        self.assertEqual(len(packet["items"]), 9)
        self.assertEqual(len(artifact_io.load_json(self.output / "reviewer/assessment-template.json")["reviews"]), 2)
        self.assertFalse(any(b"ORPHAN_ANSWER" in path.read_bytes() for path in (self.output / "reviewer").rglob("*") if path.is_file()))
        self.submit(self.assessment())
        self.assertEqual(review.review_status(self.output)["reviewed_attempts"], 2)

    def test_early_setup_failures_are_counted_without_inventing_source_or_answers(self):
        self.fixture.config["adapter"]["path"] = str(self.root / "missing-adapter")
        batch, _ = self.batch(run=False)
        review.export_review(batch, self.output)
        status = review.review_status(self.output)
        self.assertEqual(status["attempts"]["failed"], 3)
        self.assertEqual(status["attempts"]["with_answer"], 0)
        self.assertEqual(status["execution_order_integrity"], "not_established")
        self.assertEqual(status["assessment_summary"]["reviewers"], 0)
        self.assertEqual(artifact_io.load_json(self.output / "reviewer/packet.json")["source_files"], [])

    def test_partial_answer_uses_intact_pinned_source_from_another_trial(self):
        self.stateful_adapter()
        batch, trials = self.batch(run=False)
        (trials[0] / "test-mode").write_text("mutate")
        for trial in trials:
            trial_runner.run_trial(trial)
        self.assertEqual(artifact_io.load_json(trials[0] / "result.json")["run_status"]["state"], "input_changed")
        review.export_review(batch, self.output)
        self.assertEqual((self.output / "reviewer/source/src/service.py").read_text(), "def value():\n    return 7\n")
        status = review.review_status(self.output)
        self.assertEqual(status["attempts"]["failed"], 1)
        self.assertEqual(status["attempts"]["with_answer"], 3)
        self.assertEqual(status["attempts_without_verified_source"], 1)

    def test_cannot_review_answers_if_every_source_snapshot_is_damaged(self):
        batch, trials = self.batch()
        for trial in trials:
            path = trial / "source/src/service.py"
            path.chmod(0o644)
            path.write_text("Untrusted replacement\n")
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            review.export_review(batch, self.output)
        self.assertEqual(raised.exception.code, "review_source")
        self.assertFalse(self.output.exists())

    def test_rejects_tampered_answer_manifest_key_and_request(self):
        batch, trials = self.batch()
        for name, replacement in (("answer.md", b"Substituted answer"), ("manifest.json", None),
                                  ("rubric.json", None), ("request.json", None)):
            with self.subTest(name=name):
                path = trials[0] / name
                original = path.read_bytes()
                if name == "manifest.json":
                    replacement = original + b" "
                elif name in ("rubric.json", "request.json"):
                    value = artifact_io.parse_json(original)
                    value["reviewer_notes" if name == "rubric.json" else "model"] = ["altered"] if name == "rubric.json" else "altered"
                    replacement = artifact_io.canonical(value)
                path.write_bytes(replacement)
                try:
                    with self.assertRaises(artifact_io.BenchmarkError):
                        review.export_review(batch, self.output)
                    self.assertFalse(self.output.exists())
                finally:
                    path.write_bytes(original)

    def test_rejects_a_tampered_batch_execution_chain(self):
        batch, trials = self.batch()
        path = trials[1] / "result.json"
        value = artifact_io.load_json(path)
        value["batch_execution"]["previous_result_sha256"] = "0" * 64
        path.write_bytes(artifact_io.canonical(value))
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            review.export_review(batch, self.output)
        self.assertEqual(raised.exception.code, "review_identity")
        self.assertFalse(self.output.exists())

    def test_result_without_its_attempt_lock_is_rejected(self):
        batch, trials = self.batch()
        (trials[0] / "run.lock").unlink()
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            review.export_review(batch, self.output)
        self.assertEqual(raised.exception.code, "review_identity")
        self.assertFalse(self.output.exists())

    def test_bound_batch_without_its_execution_lock_is_rejected(self):
        batch, _ = self.batch()
        (batch / "execution.lock").unlink()
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            review.export_review(batch, self.output)
        self.assertEqual(raised.exception.code, "review_missing")
        self.assertFalse(self.output.exists())

    def test_refuses_incomplete_duplicate_or_reordered_condition_matrix(self):
        batch, _ = self.batch(repetitions=3, run=False)
        path = batch / "batch.json"
        original = artifact_io.load_json(path)
        for mutation in ("drop", "duplicate", "reorder", "hide_repetition"):
            with self.subTest(mutation=mutation):
                value = copy.deepcopy(original)
                if mutation == "drop":
                    value["trials"].pop()
                elif mutation == "duplicate":
                    value["trials"][1]["directory"] = value["trials"][0]["directory"]
                elif mutation == "reorder":
                    value["trials"][0], value["trials"][1] = value["trials"][1], value["trials"][0]
                else:
                    value["repetitions"] = 2
                    value["trials"] = value["trials"][:6]
                path.write_bytes(artifact_io.canonical(value))
                with self.assertRaises(artifact_io.BenchmarkError) as raised:
                    review.export_review(batch, self.output)
                self.assertEqual(raised.exception.code, "review_inventory")
                self.assertFalse(self.output.exists())

    def test_version_one_generic_requests_remain_reviewable(self):
        batch, trials = self.batch(run=False)
        for trial in trials:
            manifest = artifact_io.load_json(trial / "manifest.json")
            request = artifact_io.load_json(trial / "request.json")
            manifest["schema_version"] = 1
            request.pop("projection_revision")
            if request["mmcg"] is not None:
                request["mmcg"] = {key: request["mmcg"][key] for key in ("binary", "index")}
            manifest["request_sha256"] = artifact_io.digest(request)
            (trial / "request.json").write_bytes(artifact_io.canonical(request))
            (trial / "manifest.json").write_bytes(artifact_io.canonical(manifest))
            self.assertEqual(trial_runner.run_trial(trial)["run_status"]["state"], "completed")
        review.export_review(batch, self.output)
        self.assertEqual(review.review_status(self.output)["attempts"]["completed"], 3)

    def test_legacy_unbound_batches_remain_reviewable_without_an_order_claim(self):
        batch, trials = self.batch(run=False)
        batch_value = artifact_io.load_json(batch / "batch.json")
        batch_value["schema_version"] = 1
        batch_value.pop("batch_id")
        batch_value.pop("plan_sha256")
        (batch / "batch.json").write_bytes(artifact_io.canonical(batch_value))
        for trial in trials:
            manifest = artifact_io.load_json(trial / "manifest.json")
            manifest["schema_version"] = 2
            manifest.pop("batch")
            manifest["condition_sha256"] = artifact_io.digest(model_protocol.condition_identity(manifest))
            (trial / "manifest.json").write_bytes(artifact_io.canonical(manifest))
            self.assertEqual(trial_runner.run_trial(trial)["run_status"]["state"], "completed")
        review.export_review(batch, self.output)
        self.assertEqual(review.review_status(self.output)["execution_order_integrity"], "unverified_legacy")

    def test_corpus_file_order_and_line_metadata_match_without_changing_source_identity(self):
        case = corpus.select_case(self.case.path, self.case.task["id"], self.fixture.repo)
        batch, _ = self.batch(corpus_case=case["summary"])
        review.export_review(batch, self.output)
        self.assertEqual(review.review_status(self.output)["attempts"]["completed"], 3)
        value = artifact_io.load_json(batch / "batch.json")
        value["corpus_case"]["indexed_files"] = ["docs/guide.md"]
        (batch / "batch.json").write_bytes(artifact_io.canonical(value))
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            review.export_review(batch, self.root / "conflicting-corpus")
        self.assertEqual(raised.exception.code, "review_identity")

    def test_duplicate_reviewer_cannot_replace_evidence_and_disagreement_is_retained(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        alice = self.assessment()
        receipt = self.submit(alice)
        original = receipt.read_bytes()
        changed = copy.deepcopy(alice)
        changed["reviews"][0]["knowns"][0]["coverage"] = "covered"
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            self.submit(changed)
        self.assertEqual(raised.exception.code, "review_exists")
        self.assertEqual(receipt.read_bytes(), original)
        changed["reviewer"] = "bob"
        bob = self.submit(changed)
        self.assertEqual(artifact_io.load_json(bob)["assessment"], changed)
        status = review.review_status(self.output)
        self.assertEqual(status["reviewers"], [{"reviewer": "alice", "reviewed": 3}, {"reviewer": "bob", "reviewed": 3}])
        self.assertEqual(status["reviewed_attempts"], 3)
        summary = status["assessment_summary"]
        self.assertEqual(summary["overall"]["answer_assessments"], 6)
        self.assertEqual(summary["overall"]["known_coverage"], {"covered": 1, "partial": 5, "missing": 0})
        self.assertEqual(summary["disagreement"], {"answers_compared": 3, "answers_with_disagreement": 1,
            "material_error_flags_with_disagreement": 0, "known_dimensions_with_disagreement": 1,
            "unknown_dimensions_with_disagreement": 0, "task_outcomes_with_disagreement": 0})
        self.assertIsNone(status["quality_uplift"])

    def test_rejects_stale_incomplete_fabricated_and_invalid_claim_reviews(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        original = self.assessment()
        for mutation in ("export", "packet", "answer", "rubric", "missing", "duplicate", "quote", "anchor", "unfilled", "missing_known", "missing_unknown", "material"):
            with self.subTest(mutation=mutation):
                value = copy.deepcopy(original)
                item = value["reviews"][0]
                if mutation == "export":
                    value["export_id"] = "export-" + "0" * 32
                elif mutation == "packet":
                    value["packet_sha256"] = "0" * 64
                elif mutation in ("answer", "rubric"):
                    item[mutation + "_sha256"] = "0" * 64
                elif mutation == "missing":
                    value["reviews"].pop()
                elif mutation == "duplicate":
                    value["reviews"][1] = copy.deepcopy(item)
                elif mutation == "quote":
                    item["claims"][0]["quote"] = "The model did not say this."
                elif mutation == "anchor":
                    item["claims"][0]["anchors"] = ["src/service.py:99"]
                elif mutation == "unfilled":
                    item["knowns"][0]["coverage"] = None
                elif mutation in ("missing_known", "missing_unknown"):
                    item["knowns" if mutation == "missing_known" else "unknowns"] = []
                else:
                    item["claims"][0]["material_error"] = True
                with self.assertRaises(artifact_io.BenchmarkError):
                    self.submit(value)
                self.assertFalse((self.output / "reviews/alice.json").exists())

    def test_rejects_symlinked_control_and_intermediate_directories(self):
        batch, trials = self.batch()
        for original in (batch / "batch.json", trials[0] / "manifest.json", trials[0]):
            with self.subTest(path=original.name):
                target = self.root / "moved-input"
                original.rename(target)
                original.symlink_to(target, target_is_directory=target.is_dir())
                try:
                    with self.assertRaises(artifact_io.BenchmarkError):
                        review.export_review(batch, self.output)
                    self.assertFalse(self.output.exists())
                finally:
                    original.unlink()
                    target.rename(original)
        review.export_review(batch, self.output)
        for name in ("reviewer", "reviewer/source/src", "reviewer/packet.json"):
            with self.subTest(export_path=name):
                original = self.output / name
                target = self.root / "moved-export"
                original.rename(target)
                original.symlink_to(target, target_is_directory=target.is_dir())
                try:
                    with self.assertRaises(artifact_io.BenchmarkError):
                        review.review_status(self.output)
                finally:
                    original.unlink()
                    target.rename(original)

    def test_import_cannot_follow_a_reviews_directory_symlink(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        outside = self.root / "outside"
        outside.mkdir()
        (self.output / "reviews").symlink_to(outside, target_is_directory=True)
        with self.assertRaises(artifact_io.BenchmarkError):
            self.submit(self.assessment())
        self.assertEqual(list(outside.iterdir()), [])

    def test_review_export_tampering_and_output_replacement_are_rejected(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            review.export_review(batch, self.output)
        self.assertEqual(raised.exception.code, "review_exists")
        with self.assertRaises(artifact_io.BenchmarkError):
            review.export_review(batch, batch / "review-export")
        packet = artifact_io.load_json(self.output / "reviewer/packet.json")
        for name in ("reviewer/packet.json", "coordinator.json", "reviewer/source/src/service.py",
                     "reviewer/" + packet["items"][0]["answer"]["path"]):
            with self.subTest(name=name):
                path = self.output / name
                original = path.read_bytes()
                path.chmod(0o600)
                path.write_bytes(original + b" ")
                try:
                    with self.assertRaises(artifact_io.BenchmarkError):
                        review.review_status(self.output)
                finally:
                    path.write_bytes(original)

    def test_large_answers_keep_the_producer_cap_and_aggregate_budget_is_enforced(self):
        size = artifact_io.CONTROL_BYTE_LIMIT + 32
        self.fixture.adapter(f"init()\nfinal('x' * {size})\n")
        self.fixture.config["limits"].update(answer_bytes=size, trace_bytes=2 * size)
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        packet = artifact_io.load_json(self.output / "reviewer/packet.json")
        self.assertEqual([item["answer"]["bytes"] for item in packet["items"]], [size] * 3)
        with patch.object(review, "OUTPUT_LIMIT", 3 * artifact_io.CONTROL_BYTE_LIMIT + size):
            with self.assertRaises(artifact_io.BenchmarkError) as raised:
                review.export_review(batch, self.root / "too-large")
        self.assertEqual(raised.exception.code, "review_limit")
        self.assertFalse((self.root / "too-large").exists())

    def test_cli_errors_are_bounded_and_do_not_print_tracebacks(self):
        batch, _ = self.batch()
        path = batch / "batch.json"
        value = artifact_io.load_json(path)
        value["trials"].pop()
        path.write_bytes(artifact_io.canonical(value))
        result = self.cli("export", batch, "--output", self.output)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertTrue(result.stderr.startswith("review_inventory:"), result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_import_limit_cannot_create_an_export_its_status_cannot_read(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        for index in range(64):
            self.submit(self.assessment("reviewer-" + str(index)))
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            self.submit(self.assessment("overflow"))
        self.assertEqual(raised.exception.code, "review_limit")
        self.assertEqual(len(review.review_status(self.output)["reviewers"]), 64)
        self.assertFalse((self.output / "reviews/overflow.json").exists())

    def test_reviewer_folder_cannot_acquire_unsealed_condition_metadata(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        assessment = self.assessment()
        for name in ("coordinator.json", "source/trace.jsonl", "assessment-template.json"):
            with self.subTest(name=name):
                path = self.output / "reviewer" / name
                original = path.read_bytes() if path.exists() else None
                if path.exists():
                    path.chmod(0o600)
                path.write_bytes((self.output / "coordinator.json").read_bytes())
                try:
                    with self.assertRaises(artifact_io.BenchmarkError):
                        review.review_status(self.output)
                    with self.assertRaises(artifact_io.BenchmarkError):
                        self.submit(assessment)
                finally:
                    if original is None:
                        path.unlink()
                    else:
                        path.write_bytes(original)

    def test_concurrent_import_returns_busy_until_lock_is_released(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        assessment = self.root / "assessment.json"
        assessment.write_bytes(artifact_io.canonical(self.assessment()))
        with Root(self.output) as root, root.exclusive_lock("import.lock"):
            result = self.cli("import", self.output, "--assessment", assessment)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertTrue(result.stderr.startswith("review_busy:"), result.stderr)
        self.assertFalse((self.output / "reviews/alice.json").exists())
        self.assertEqual(self.cli("import", self.output, "--assessment", assessment).returncode, 0)

    def test_unpublished_staging_file_is_not_an_assessment(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        reviews = self.output / "reviews"
        reviews.mkdir()
        (reviews / (".pending-" + "0" * 32)).write_bytes(b"interrupted write")
        self.assertEqual(review.review_status(self.output)["reviewers"], [])
        self.submit(self.assessment())
        self.assertEqual(review.review_status(self.output)["reviewers"], [{"reviewer": "alice", "reviewed": 3}])

    def test_snapshot_recheck_detects_a_result_finishing_during_export(self):
        batch, trials = self.batch(run=False)
        original_recheck = Root.recheck
        def finish_during_recheck(root):
            if root.path == batch and not (trials[0] / "result.json").exists():
                trial_runner.run_trial(trials[0])
            return original_recheck(root)
        with patch.object(Root, "recheck", finish_during_recheck):
            with self.assertRaises(artifact_io.BenchmarkError) as raised:
                review.export_review(batch, self.output)
        self.assertEqual(raised.exception.code, "review_changed")
        self.assertFalse(self.output.exists())

    def test_review_artifact_replacement_after_publication_cannot_return_success(self):
        destination = self.root / "publication-race"
        destination.mkdir()
        target = destination / "artifact.json"
        original_fsync = review_io.os.fsync
        replaced = False

        def replace_after_publication(descriptor):
            nonlocal replaced
            result = original_fsync(descriptor)
            if not replaced and target.exists():
                replaced = True
                target.unlink()
                target.write_bytes(b"external replacement")
            return result

        with Root(destination) as root, patch.object(
                review_io.os, "fsync", side_effect=replace_after_publication):
            with self.assertRaises(artifact_io.BenchmarkError) as raised:
                root.write_new("artifact.json", b"trusted artifact")
        self.assertEqual(raised.exception.code, "review_changed")
        self.assertTrue(replaced)
        self.assertEqual(target.read_bytes(), b"external replacement")

    def test_detached_review_parent_cannot_publish_a_successful_artifact(self):
        destination = self.root / "detached-publication"
        destination.mkdir()
        detached = self.root / "detached-review-parent"
        original_link = review_io.os.link
        replaced = False

        def detach_parent(*arguments, **keywords):
            nonlocal replaced
            if not replaced:
                replaced = True
                (destination / "reviews").rename(detached)
                (destination / "reviews").mkdir()
            return original_link(*arguments, **keywords)

        with Root(destination) as root, patch.object(
                review_io.os, "link", side_effect=detach_parent):
            with self.assertRaises(artifact_io.BenchmarkError) as raised:
                root.write_new("reviews/alice.json", b"trusted review")
        self.assertEqual(raised.exception.code, "review_changed")
        self.assertTrue(replaced)
        self.assertFalse((destination / "reviews/alice.json").exists())
        self.assertEqual(list(detached.iterdir()), [])

    def test_review_status_rejects_a_detached_parent_during_its_final_recheck(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        packet = artifact_io.load_json(self.output / "reviewer/packet.json")
        answer_path = packet["items"][-1]["answer"]["path"]
        answer_name = Path(answer_path).name
        reviewer = self.output / "reviewer"
        detached = self.root / "detached-reviewer"
        original_open = review_io.os.open
        opens = 0

        def replace_after_final_answer_open(path, *arguments, **keywords):
            nonlocal opens
            result = original_open(path, *arguments, **keywords)
            if path == answer_name and "dir_fd" in keywords:
                opens += 1
                if opens == 2:
                    reviewer.rename(detached)
                    shutil.copytree(detached, reviewer)
            return result

        with patch.object(review_io.os, "open", new=replace_after_final_answer_open):
            with self.assertRaises(artifact_io.BenchmarkError) as raised:
                review.review_status(self.output)
        self.assertEqual(raised.exception.code, "review_changed")
        self.assertEqual(opens, 2)

    def test_export_rechecks_earlier_artifacts_after_seal_publication(self):
        batch, _ = self.batch()
        original_write = Root.write_new

        def replace_packet_after_seal(root, path, body):
            result = original_write(root, path, body)
            if root.path == self.output and path == "seal.json":
                packet = self.output / "reviewer/packet.json"
                packet.chmod(0o600)
                packet.write_bytes(b"external replacement")
            return result

        with patch.object(Root, "write_new", replace_packet_after_seal):
            with self.assertRaises(artifact_io.BenchmarkError) as raised:
                review.export_review(batch, self.output)
        self.assertEqual(raised.exception.code, "review_changed")


if __name__ == "__main__":
    unittest.main()
