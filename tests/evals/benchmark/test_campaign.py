"""Corpus-wide execution preserves inventories, bindings and failures."""

import copy
from contextlib import redirect_stderr, redirect_stdout
import io
import os
from pathlib import Path
import shutil
import unittest
from unittest.mock import patch

from evals.benchmark import artifacts, campaign, review
from tests.evals.support.corpus import CorpusFixture


class CampaignTests(unittest.TestCase):
    def setUp(self):
        self.case = CorpusFixture()
        self.addCleanup(self.case.close)
        self.fixture = self.case.fixture
        second = copy.deepcopy(self.case.entry)
        second.update(id="service-02", task="tasks/service-02.json", rubric="rubrics/service-02.json")
        self.case.registry["cases"].append(second)
        self.case.save()
        artifacts.write_new(self.case.root / second["task"], dict(self.case.task, id="service-02"))
        artifacts.write_new(self.case.root / second["rubric"], dict(self.case.key, task_id="service-02"))
        self.output = self.fixture.root / "campaign"

    def prepare(self):
        return campaign.prepare(corpus_path=self.case.path, config=self.fixture.config,
            source_repo=self.fixture.repo, tool_repo=self.fixture.repo, output=self.output, repetitions=1)

    def test_every_case_runs_once_and_failed_attempts_remain_in_comparison(self):
        self.fixture.adapter("init()\n(root.parent / 'launches').open('a').write('x')\nfinal()\n"
            "sys.exit(7 if request['task']['id'] == 'service-02' else 0)\n")
        self.assertEqual(self.prepare()["planned_attempts"], 6)
        Path(self.fixture.config["mmcg"]["path"]).write_text("changed original executable\n")
        first = campaign.run(self.output, progress=lambda *args, **kwargs: None)
        self.assertEqual([row["status"] for row in first["attempts"]].count("invocation_error"), 3)
        self.assertEqual(campaign.run(self.output, progress=lambda *args, **kwargs: None), first)
        for path in self.output.rglob("launches"):
            self.assertEqual(path.read_text(), "x")
        output = self.fixture.root / "review-set"
        campaign.export(self.output, output)
        result = campaign.compare(output, "source", "portable_mmcg")
        self.assertEqual(result["independent_task_count"], 2)
        self.assertEqual(result["planned_attempts"], 6)
        self.assertFalse(result["balanced_positions"])
        self.assertIsNone(result["quality_uplift"])
        self.assertEqual(result["cases"][1]["attempts"]["failed"], 3)
        self.assertEqual(result["efficiency"][0]["by_condition"]["source"]["planned_attempts"], 2)
        self.assertEqual(result["efficiency"][0]["by_condition"]["source"]["successful_outcomes"], {"lower": 0, "upper": 1})

    def test_campaign_cli_passes_only_explicit_credentials_and_missing_selection_stops_before_run(self):
        self.fixture.adapter("init()\nassert os.environ['ANTHROPIC_API_KEY'] == 'fixture-key'\n"
                             "assert 'OPENAI_API_KEY' not in os.environ\nfinal()\n")
        self.prepare()
        command = ["run", str(self.output), "--credential-env", "ANTHROPIC_API_KEY"]
        with patch.dict(os.environ, {}, clear=True), redirect_stderr(io.StringIO()) as error:
            self.assertEqual(campaign.main(command), 2)
        self.assertIn("credentials_missing", error.getvalue())
        self.assertFalse(list(self.output.rglob("run.lock")))
        with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "fixture-key", "OPENAI_API_KEY": "unselected-key"}), redirect_stdout(io.StringIO()) as output:
            self.assertEqual(campaign.main(command), 0)
        self.assertNotIn("fixture-key", output.getvalue())
        self.assertEqual(output.getvalue().count('"status":"completed"'), 6)

    def test_same_named_case_from_another_batch_cannot_replace_planned_export(self):
        self.prepare()
        output = self.fixture.root / "review-set"
        campaign.export(self.output, output)
        identifier = self.case.task["id"]
        second = self.fixture.root / "another-campaign"
        campaign.prepare(corpus_path=self.case.path, config=self.fixture.config,
            source_repo=self.fixture.repo, tool_repo=self.fixture.repo, output=second, repetitions=1)
        another = self.fixture.root / "another-review-set"
        campaign.export(second, another)
        shutil.rmtree(output / identifier)
        shutil.copytree(another / identifier, output / identifier)
        with self.assertRaises(artifacts.BenchmarkError) as raised:
            campaign.compare(output, "source", "portable_mmcg")
        self.assertEqual(raised.exception.code, "campaign_changed")

    def test_campaign_matrix_cannot_omit_conditions_or_relabel_the_bound_batches(self):
        self.prepare()
        original = artifacts.load_json(self.output / "campaign.json")
        exported = self.fixture.root / "review-set"
        campaign.export(self.output, exported)
        review_set = artifacts.load_json(exported / "review-set.json")
        for change in ({"conditions": ["source", "portable"]}, {"repetitions": 2},
                       {"planned_attempts": 4}, {"balanced_positions": True}):
            altered = dict(original, **change)
            (self.output / "campaign.json").write_bytes(artifacts.canonical(altered))
            with self.subTest(change=change, path="campaign"):
                with self.assertRaises(artifacts.BenchmarkError) as raised:
                    campaign.run(self.output, progress=lambda *args, **kwargs: None)
                self.assertEqual(raised.exception.code, "campaign_inventory")
            altered_set = dict(review_set, campaign=altered, campaign_sha256=artifacts.digest(altered))
            (exported / "review-set.json").write_bytes(artifacts.canonical(altered_set))
            with self.subTest(change=change, path="review"):
                with self.assertRaises(artifacts.BenchmarkError) as raised:
                    campaign.compare(exported, "source", "portable_mmcg")
                self.assertEqual(raised.exception.code, "campaign_inventory")
        self.assertFalse(list(self.output.rglob("adapter-called")))

    def test_reviewer_missing_one_case_keeps_corpus_outcomes_unresolved(self):
        self.prepare()
        campaign.run(self.output, progress=lambda *args, **kwargs: None)
        output = self.fixture.root / "review-set"
        campaign.export(self.output, output)
        first = output / self.case.task["id"]
        value = artifacts.load_json(first / "reviewer/assessment-template.json")
        value["reviewer"] = "fixture"
        for item in value["reviews"]:
            item["outcome"] = {"status": "unsatisfied", "rationale": "Required value and caller uncertainty are missing."}
            item["claims"] = [{"quote": "Observed src/service.py:2.", "support": "supported",
                "anchors": ["src/service.py:2"], "material_error": False, "rationale": "The source location exists."}]
            item["knowns"] = [{"known_index": 0, "coverage": "partial", "answer_excerpt": "Observed src/service.py:2.",
                "rationale": "Only a location is given."}]
            item["unknowns"] = [{"unknown_index": 0, "handling": "omitted", "answer_excerpt": None,
                "rationale": "No caller uncertainty is discussed."}]
        submission = self.fixture.root / "assessment.json"
        artifacts.write_new(submission, value)
        review.import_assessment(first, submission)
        total = campaign.compare(output, "source", "portable_mmcg")["efficiency"][0]
        self.assertEqual(total["reviewer"], "fixture")
        self.assertEqual(total["reviewed_cases"], 1)
        self.assertEqual(total["by_condition"]["source"]["successful_outcomes"], {"lower": 0, "upper": 1})
        self.assertIsNone(total["success_delta"]["point"])
        self.assertIsNone(total["by_condition"]["source"]["useful_outcomes_per_resource"]["total_tokens"]["value"])

    def test_source_drift_and_missing_cases_are_rejected_before_more_model_work(self):
        path = self.fixture.repo / "src/service.py"
        original = path.read_bytes()
        path.write_text("changed source\n")
        with self.assertRaises(artifacts.BenchmarkError) as raised:
            self.prepare()
        self.assertEqual(raised.exception.code, "corpus_worktree_changed")
        self.assertFalse(self.output.exists())
        path.write_bytes(original)
        self.prepare()
        plan = artifacts.load_json(self.output / "campaign.json")
        plan["cases"].pop()
        (self.output / "campaign.json").write_bytes(artifacts.canonical(plan))
        with self.assertRaises(artifacts.BenchmarkError) as raised:
            campaign.run(self.output)
        self.assertEqual(raised.exception.code, "campaign_inventory")
        self.assertFalse(list(self.output.rglob("adapter-called")))

    def test_tool_violation_preserves_failed_attempt_and_continues_only_unchanged_inputs(self):
        self.fixture.adapter('''
init()
if not request['portable_instruction'] and request['task']['id'] == 'service-01':
    if (root.parent / 'change-source').exists():
        path = root / 'src/service.py'
        path.chmod(0o644)
        path.write_text('changed source\\n')
    final(failure={'state':'identity_mismatch','code':'unavailable_tool_called'})
else:
    final()
''')
        self.prepare()
        result = campaign.run(self.output, progress=lambda *args, **kwargs: None)
        self.assertEqual(len(result["attempts"]), 6)
        self.assertEqual([row["status"] for row in result["attempts"]].count("identity_mismatch"), 1)
        second = self.fixture.root / "changed-campaign"
        campaign.prepare(corpus_path=self.case.path, config=self.fixture.config,
            source_repo=self.fixture.repo, tool_repo=self.fixture.repo, output=second, repetitions=1)
        _, batches = campaign.load(second)
        _, batch, plan = batches[0]
        (batch / plan["trials"][0]["directory"] / "change-source").write_text("yes")
        with self.assertRaises(artifacts.BenchmarkError) as raised:
            campaign.run(second, progress=lambda *args, **kwargs: None)
        self.assertEqual(raised.exception.code, "campaign_runtime_invalid")
        self.assertEqual(len(list(second.rglob("adapter-called"))), 1)
