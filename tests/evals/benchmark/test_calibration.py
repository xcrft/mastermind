"""Calibration rejects mixed variables and retains the declared axis."""

import copy
import unittest

from evals.benchmark import artifacts, batch, campaign, trials
from tests.evals.support.corpus import CorpusFixture


class CalibrationTests(unittest.TestCase):
    def setUp(self):
        self.case = CorpusFixture()
        self.addCleanup(self.case.close)
        self.fixture = self.case.fixture
        self.fixture.condition_matrix()
        self.fixture.config["conditions"] = self.fixture.config["conditions"][:2]
        for condition, role in zip(self.fixture.config["conditions"], ("direct", "researcher")):
            condition.update(role=role, reasoning_effort="high")
        cli = self.fixture.config["adapter"]
        self.fixture.config["adapter"] = {"kind": "claude_cli", "cli": cli}
        self.fixture.config["calibration"] = {"axis": "role_prompt"}

    def prepare(self, config, output):
        return campaign.prepare(corpus_path=self.case.path, config=config,
            source_repo=self.fixture.repo, tool_repo=self.fixture.repo,
            output=output, repetitions=2)

    def test_mixed_variables_and_unsupported_workflow_axis_stop_before_preparation(self):
        def change_lookup(config):
            for condition in config["conditions"]:
                condition.update(tools="mmcg", symbol_lookup="single")
            config["conditions"][1]["symbol_lookup"] = "batch"

        def change_delivery(config):
            for condition in config["conditions"]:
                condition.update(tools="mmcg", source_delivery="native_full")
            config["conditions"][1]["source_delivery"] = "native_reuse"

        mutations = {
            "effort": lambda cfg: cfg["conditions"][1].update(reasoning_effort="medium"),
            "tools": lambda cfg: cfg["conditions"][1].update(tools="mmcg"),
            "role": lambda cfg: cfg["conditions"][1].pop("role"),
            "partial_effort": lambda cfg: cfg["conditions"][1].pop("reasoning_effort"),
            "effort_with_changed_prompt": lambda cfg: cfg.update(calibration={"axis": "effort"}),
            "workflow": lambda cfg: cfg.update(calibration={"axis": "workflow"}),
            "lookup": change_lookup,
            "delivery": change_delivery,
        }
        for name, mutate in mutations.items():
            config = copy.deepcopy(self.fixture.config)
            mutate(config)
            output = self.fixture.root / name
            with self.subTest(name=name):
                with self.assertRaises(artifacts.BenchmarkError):
                    self.prepare(config, output)
                self.assertFalse(output.exists())
                with self.assertRaises(artifacts.BenchmarkError):
                    trials.prepare_batch(task=self.case.task, rubric=self.case.key, config=config,
                        source_repo=self.fixture.repo, tool_repo=self.fixture.repo,
                        output=output, repetitions=2)
                self.assertFalse(output.exists())
        self.assertFalse(list(self.fixture.root.rglob("adapter-called")))

    def test_axis_is_bound_through_trial_batch_campaign_and_offline_export(self):
        output = self.fixture.root / "campaign"
        self.prepare(self.fixture.config, output)
        plan, batches = campaign.load(output)
        _, directory, inventory = batches[0]
        self.assertTrue(plan["balanced_positions"])
        self.assertEqual(len(inventory["trials"]), 4)
        for item in inventory["trials"]:
            manifest = artifacts.load_json(directory / item["directory"] / "manifest.json")
            self.assertEqual(manifest["status"], "prepared", manifest)
            self.assertEqual(manifest["calibration"], {"axis": "role_prompt"})
        exported = self.fixture.root / "review"
        campaign.export(output, exported)
        packet = artifacts.load_json(exported / "review-set.json")
        self.assertEqual(packet["campaign"]["calibration"], {"axis": "role_prompt"})
        self.assertEqual(packet["cases"][0]["batch"]["calibration"], {"axis": "role_prompt"})
        result = campaign.compare(exported, "raw", "refined")
        self.assertEqual(result["planned_attempts"], 4)
        self.assertIsNone(result["quality_uplift"])
        self.assertEqual(result["efficiency"][0]["by_condition"]["raw"]["successful_outcomes"], {"lower": 0, "upper": 2})

        altered = dict(plan, calibration={"axis": "effort"})
        (output / "campaign.json").write_bytes(artifacts.canonical(altered))
        with self.assertRaises(artifacts.BenchmarkError) as raised:
            campaign.load(output)
        self.assertEqual(raised.exception.code, "campaign_inventory")
        (output / "campaign.json").write_bytes(artifacts.canonical(plan))

        # Removing the axis cannot retain the original bound plan or trial bindings.
        altered = copy.deepcopy(inventory)
        del altered["calibration"]
        (directory / "batch.json").write_bytes(artifacts.canonical(altered))
        with self.assertRaises(artifacts.BenchmarkError) as raised:
            batch.validate_batch_summary(altered)
        self.assertEqual(raised.exception.code, "batch_changed")
        altered["plan_sha256"] = artifacts.digest(batch.batch_plan_identity(altered))
        (directory / "batch.json").write_bytes(artifacts.canonical(altered))
        trial = directory / inventory["trials"][0]["directory"]
        with self.assertRaises(artifacts.BenchmarkError) as raised:
            trials.run_trial(trial)
        self.assertEqual(raised.exception.code, "batch_changed")
        self.assertFalse((trial / "run.lock").exists())
        self.assertFalse(list(self.fixture.root.rglob("adapter-called")))
