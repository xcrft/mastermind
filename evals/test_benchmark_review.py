"""Offline review through real trial files, Git fixtures and CLI subprocesses."""

import copy
import json
import os
import shutil
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

from evals import benchmark as bench
from evals import benchmark_review as review
from evals.benchmark_review_io import Root
from evals import benchmark_corpus as corpus
from evals import test_benchmark_corpus as fixtures


@unittest.skipUnless(os.name == "posix" and shutil.which("git"), "requires POSIX and Git")
class ReviewTests(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.CorpusTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)
        self.fixture = self.case.fixture
        self.root = self.fixture.root
        self.output = self.root / "review-export"
        self.fixture.config["model"] = "PRIVATE_MODEL_METADATA_CANARY"

    def batch(self, repetitions=1, run=True, corpus_case=None):
        batch = bench.prepare_batch(task=self.case.task, rubric=self.case.key,
            config=self.fixture.config, source_repo=self.fixture.repo, tool_repo=self.fixture.repo,
            output=self.root / "batches", repetitions=repetitions, corpus_case=corpus_case)
        trials = [batch / item["directory"] for item in bench.load_json(batch / "batch.json")["trials"]]
        if run:
            for trial in trials:
                bench.run_trial(trial)
        return batch, trials

    def cli(self, *arguments):
        return subprocess.run([sys.executable, "-m", "evals.benchmark_review", *map(str, arguments)],
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
        value = bench.load_json(self.output / "reviewer/assessment-template.json")
        value["reviewer"] = reviewer
        for item in value["reviews"]:
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
        path.write_bytes(bench.canonical(value))
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
        packet = bench.load_json(self.output / "reviewer/packet.json")
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
        submission.write_bytes(bench.canonical(self.assessment()))
        result = self.cli("import", self.output, "--assessment", submission)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.cli("status", self.output)
        self.assertEqual(result.returncode, 0, result.stderr)
        status = json.loads(result.stdout)
        self.assertEqual(status["attempts"], {"planned": 3, "completed": 3, "failed": 0,
                         "not_run": 0, "unfinished": 0, "missing_artifacts": 0, "with_answer": 3})
        self.assertEqual(status["reviewed_attempts"], 3)
        self.assertEqual(status["reviewers"], [{"reviewer": "alice", "reviewed": 3}])
        self.assertFalse(status["comparison_accepted"])
        self.assertIsNone(status["quality_uplift"])

    def test_accounts_for_failed_missing_unfinished_and_not_run_attempts(self):
        self.stateful_adapter()
        self.fixture.config["mmcg"]["path"] = str(self.root / "absent-native-runtime")
        batch, trials = self.batch(repetitions=3, run=False)
        bench.run_trial(trials[0])
        (trials[1] / "test-mode").write_text("partial")
        bench.run_trial(trials[1])
        bench.run_trial(trials[2])
        (trials[3] / "run.lock").touch()
        (trials[3] / "answer.md").write_text("ORPHAN_ANSWER_MUST_NOT_BE_REVIEWED")
        (trials[7] / "manifest.json").unlink()
        trials[8].rename(self.root / "missing-trial")
        review.export_review(batch, self.output)
        status = review.review_status(self.output)
        self.assertEqual(status["attempts"], {"planned": 9, "completed": 1, "failed": 4,
                         "not_run": 1, "unfinished": 1, "missing_artifacts": 2, "with_answer": 2})
        packet = bench.load_json(self.output / "reviewer/packet.json")
        self.assertEqual(len(packet["items"]), 9)
        self.assertEqual(len(bench.load_json(self.output / "reviewer/assessment-template.json")["reviews"]), 2)
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
        self.assertEqual(bench.load_json(self.output / "reviewer/packet.json")["source_files"], [])

    def test_partial_answer_uses_intact_pinned_source_from_another_trial(self):
        self.stateful_adapter()
        batch, trials = self.batch(run=False)
        (trials[0] / "test-mode").write_text("mutate")
        for trial in trials:
            bench.run_trial(trial)
        self.assertEqual(bench.load_json(trials[0] / "result.json")["run_status"]["state"], "input_changed")
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
        with self.assertRaises(bench.BenchmarkError) as raised:
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
                    value = bench.parse_json(original)
                    value["reviewer_notes" if name == "rubric.json" else "model"] = ["altered"] if name == "rubric.json" else "altered"
                    replacement = bench.canonical(value)
                path.write_bytes(replacement)
                try:
                    with self.assertRaises(bench.BenchmarkError):
                        review.export_review(batch, self.output)
                    self.assertFalse(self.output.exists())
                finally:
                    path.write_bytes(original)

    def test_refuses_incomplete_duplicate_or_reordered_condition_matrix(self):
        batch, _ = self.batch(repetitions=3, run=False)
        path = batch / "batch.json"
        original = bench.load_json(path)
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
                path.write_bytes(bench.canonical(value))
                with self.assertRaises(bench.BenchmarkError) as raised:
                    review.export_review(batch, self.output)
                self.assertEqual(raised.exception.code, "review_inventory")
                self.assertFalse(self.output.exists())

    def test_version_one_generic_requests_remain_reviewable(self):
        batch, trials = self.batch(run=False)
        for trial in trials:
            manifest = bench.load_json(trial / "manifest.json")
            request = bench.load_json(trial / "request.json")
            manifest["schema_version"] = 1
            request.pop("projection_revision")
            if request["mmcg"] is not None:
                request["mmcg"] = {key: request["mmcg"][key] for key in ("binary", "index")}
            manifest["request_sha256"] = bench.digest(request)
            (trial / "request.json").write_bytes(bench.canonical(request))
            (trial / "manifest.json").write_bytes(bench.canonical(manifest))
            self.assertEqual(bench.run_trial(trial)["run_status"]["state"], "completed")
        review.export_review(batch, self.output)
        self.assertEqual(review.review_status(self.output)["attempts"]["completed"], 3)

    def test_corpus_file_order_and_line_metadata_match_without_changing_source_identity(self):
        case = corpus.select_case(self.case.path, self.case.task["id"], self.fixture.repo)
        batch, _ = self.batch(corpus_case=case["summary"])
        review.export_review(batch, self.output)
        self.assertEqual(review.review_status(self.output)["attempts"]["completed"], 3)
        value = bench.load_json(batch / "batch.json")
        value["corpus_case"]["indexed_files"] = ["docs/guide.md"]
        (batch / "batch.json").write_bytes(bench.canonical(value))
        with self.assertRaises(bench.BenchmarkError) as raised:
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
        with self.assertRaises(bench.BenchmarkError) as raised:
            self.submit(changed)
        self.assertEqual(raised.exception.code, "review_exists")
        self.assertEqual(receipt.read_bytes(), original)
        changed["reviewer"] = "bob"
        bob = self.submit(changed)
        self.assertEqual(bench.load_json(bob)["assessment"], changed)
        status = review.review_status(self.output)
        self.assertEqual(status["reviewers"], [{"reviewer": "alice", "reviewed": 3}, {"reviewer": "bob", "reviewed": 3}])
        self.assertEqual(status["reviewed_attempts"], 3)
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
                with self.assertRaises(bench.BenchmarkError):
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
                    with self.assertRaises(bench.BenchmarkError):
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
                    with self.assertRaises(bench.BenchmarkError):
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
        with self.assertRaises(bench.BenchmarkError):
            self.submit(self.assessment())
        self.assertEqual(list(outside.iterdir()), [])

    def test_review_export_tampering_and_output_replacement_are_rejected(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        with self.assertRaises(bench.BenchmarkError) as raised:
            review.export_review(batch, self.output)
        self.assertEqual(raised.exception.code, "review_exists")
        with self.assertRaises(bench.BenchmarkError):
            review.export_review(batch, batch / "review-export")
        packet = bench.load_json(self.output / "reviewer/packet.json")
        for name in ("reviewer/packet.json", "coordinator.json", "reviewer/source/src/service.py",
                     "reviewer/" + packet["items"][0]["answer"]["path"]):
            with self.subTest(name=name):
                path = self.output / name
                original = path.read_bytes()
                path.chmod(0o600)
                path.write_bytes(original + b" ")
                try:
                    with self.assertRaises(bench.BenchmarkError):
                        review.review_status(self.output)
                finally:
                    path.write_bytes(original)

    def test_large_answers_keep_the_producer_cap_and_aggregate_budget_is_enforced(self):
        size = bench.CONTROL_BYTE_LIMIT + 32
        self.fixture.adapter(f"init()\nfinal('x' * {size})\n")
        self.fixture.config["limits"].update(answer_bytes=size, trace_bytes=2 * size)
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        packet = bench.load_json(self.output / "reviewer/packet.json")
        self.assertEqual([item["answer"]["bytes"] for item in packet["items"]], [size] * 3)
        with patch.object(review, "OUTPUT_LIMIT", 3 * bench.CONTROL_BYTE_LIMIT + size):
            with self.assertRaises(bench.BenchmarkError) as raised:
                review.export_review(batch, self.root / "too-large")
        self.assertEqual(raised.exception.code, "review_limit")
        self.assertFalse((self.root / "too-large").exists())

    def test_cli_errors_are_bounded_and_do_not_print_tracebacks(self):
        batch, _ = self.batch()
        path = batch / "batch.json"
        value = bench.load_json(path)
        value["trials"].pop()
        path.write_bytes(bench.canonical(value))
        result = self.cli("export", batch, "--output", self.output)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertTrue(result.stderr.startswith("review_inventory:"), result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_import_limit_cannot_create_an_export_its_status_cannot_read(self):
        batch, _ = self.batch()
        review.export_review(batch, self.output)
        for index in range(64):
            self.submit(self.assessment("reviewer-" + str(index)))
        with self.assertRaises(bench.BenchmarkError) as raised:
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
                    with self.assertRaises(bench.BenchmarkError):
                        review.review_status(self.output)
                    with self.assertRaises(bench.BenchmarkError):
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
        assessment.write_bytes(bench.canonical(self.assessment()))
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
                bench.run_trial(trials[0])
            return original_recheck(root)
        with patch.object(Root, "recheck", finish_during_recheck):
            with self.assertRaises(bench.BenchmarkError) as raised:
                review.export_review(batch, self.output)
        self.assertEqual(raised.exception.code, "review_changed")
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
