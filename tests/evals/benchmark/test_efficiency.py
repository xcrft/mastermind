"""Known numerical outcomes for all-attempt efficiency and unknowns."""

import copy
import unittest

from evals.benchmark import efficiency
from evals.benchmark.artifacts import BenchmarkError
from tests.evals.support.measurements import paired_measurements as fixture


class EfficiencyTests(unittest.TestCase):
    def test_faster_failed_answers_and_all_wrong_answers_cannot_pass_the_value_gate(self):
        coordinator, assessment = fixture()
        assessment["reviews"][1]["outcome"]["status"] = "unsatisfied"
        result = efficiency.compare(coordinator, assessment, "source", "portable")
        self.assertEqual(result["contrasts"]["total_tokens"]["resource_savings"], 0.5)
        self.assertEqual(result["value"]["status"], "quality_regression")
        self.assertIsNone(result["value"]["quality_gated_resource_savings"]["total_tokens"])
        assessment["reviews"][3]["outcome"]["status"] = "unknown"
        unresolved = efficiency.compare(coordinator, assessment, "source", "portable")
        self.assertEqual(unresolved["known_quality_losses"], 1)
        self.assertEqual(unresolved["value"]["status"], "quality_regression")
        self.assertIsNone(unresolved["success_delta"]["point"])
        for row in assessment["reviews"]:
            row["outcome"]["status"] = "unsatisfied"
        empty = efficiency.compare(coordinator, assessment, "source", "portable")
        self.assertEqual(empty["value"]["status"], "no_successful_baseline")
        self.assertIsNone(empty["value"]["quality_gated_resource_savings"]["run_seconds"])

    def test_latency_distributions_pool_observations_and_keep_missing_measurements_unknown(self):
        coordinator, assessment = fixture()
        for slot, value in zip(coordinator["slots"], [1, 2, 3, 4]):
            slot["resources"].update(first_message_seconds=value, final_answer_seconds=value + 1)
        row = efficiency.compare(coordinator, assessment, "source", "portable")
        total = efficiency.aggregate([row, row], "source", "portable")
        metric = total["by_condition"]["portable"]["resources"]["first_message_seconds"]
        self.assertEqual(metric["observed_values"], [2, 4, 2, 4])
        self.assertEqual(metric["distribution"]["p50"], 3)
        self.assertEqual(metric["distribution"]["p95"], 4)
        coordinator["slots"][1]["resources"]["first_message_seconds"] = None
        missing = efficiency.compare(coordinator, assessment, "source", "portable")
        metric = missing["by_condition"]["portable"]["resources"]["first_message_seconds"]
        self.assertIsNone(metric["total"])
        self.assertEqual(metric["unknown_attempts"], 1)
        self.assertEqual(metric["distribution"]["observed_attempts"], 1)
        with self.assertRaises(BenchmarkError):
            efficiency.aggregate([row, dict(row, outcome_basis="user_acceptance")], "source", "portable")
        with self.assertRaises(BenchmarkError):
            efficiency.distribution([10**1000])

    def test_unresolved_outcomes_bound_yield_gain_without_inventing_a_point_estimate(self):
        coordinator, assessment = fixture()
        for row in assessment["reviews"]:
            row["outcome"]["status"] = "unknown" if row["review_id"].endswith("-1") else "satisfied"
        result = efficiency.aggregate([efficiency.compare(coordinator, assessment, "source", "portable")],
                                      "source", "portable")
        source, candidate = (result["by_condition"][name] for name in ("source", "portable"))
        self.assertEqual(source["successful_outcomes"], {"lower": 1, "upper": 2})
        self.assertIsNone(candidate["useful_outcomes_per_resource"]["run_seconds"]["value"])
        self.assertEqual(source["useful_outcomes_per_resource_bounds"]["run_seconds"],
                         {"lower": 0.05, "upper": 0.1, "reason": None})
        self.assertEqual(candidate["useful_outcomes_per_resource_bounds"]["run_seconds"],
                         {"lower": 0.1, "upper": 0.2, "reason": None})
        contrast = result["contrasts"]["run_seconds"]
        self.assertIsNone(contrast["useful_outcomes_per_resource_gain"])
        self.assertEqual(contrast["useful_outcomes_per_resource_gain_bounds"],
                         {"lower": 0, "upper": 3, "reason": None})
        self.assertIsNone(result["statistical_confidence_interval"])
        self.assertEqual(result["contrasts"]["cost_usd"]["useful_outcomes_per_resource_gain_bounds"],
                         {"lower": None, "upper": None, "reason": "incomplete_measurement"})
        coordinator["slots"][0]["resources"]["run_seconds"] = None
        missing = efficiency.compare(coordinator, assessment, "source", "portable")
        self.assertIsNone(missing["by_condition"]["source"]["useful_outcomes_per_resource_bounds"]["run_seconds"]["lower"])
        unknown = efficiency.compare(fixture()[0], None, "source", "portable")
        self.assertEqual(unknown["contrasts"]["run_seconds"]["useful_outcomes_per_resource_gain_bounds"]["reason"],
                         "possible_zero_baseline_yield")

    def test_useful_work_counts_all_attempts_and_all_context_tokens(self):
        coordinator, assessment = fixture()
        result = efficiency.compare(coordinator, assessment, "source", "portable")
        baseline, candidate = (result["by_condition"][name] for name in ("source", "portable"))
        self.assertEqual(baseline["planned_attempts"], 2)
        self.assertEqual(baseline["successful_outcomes"], {"lower": 1, "upper": 1})
        self.assertEqual(candidate["successful_outcomes"], {"lower": 2, "upper": 2})
        self.assertEqual(baseline["resources"]["total_tokens"]["total"], 260)
        self.assertEqual(candidate["resource_per_success"]["total_tokens"]["value"], 65)
        self.assertEqual(candidate["useful_outcomes_per_resource"]["total_tokens"]["value"], 1 / 65)
        self.assertEqual(baseline["resources"]["trial_total_seconds"]["total"], 24)
        self.assertEqual(result["contrasts"]["total_tokens"]["resource_savings"], 0.5)
        self.assertEqual(result["contrasts"]["total_tokens"]["resource_per_success_savings"], 0.75)
        self.assertEqual(result["contrasts"]["total_tokens"]["useful_outcomes_per_resource_gain"], 3)
        self.assertTrue(result["quality_preserved_on_resolved_pairs"])
        self.assertIsNone(result["contrasts"]["cost_usd"]["resource_savings"])
        self.assertIsNone(result["statistical_confidence_interval"])

    def test_aggregate_preserves_unknown_case_and_zero_success_yield(self):
        coordinator, assessment = fixture()
        first = efficiency.compare(coordinator, assessment, "source", "portable")
        for review in assessment["reviews"]:
            review["outcome"]["status"] = "unsatisfied"
        zero = efficiency.compare(coordinator, assessment, "source", "portable")
        self.assertEqual(zero["by_condition"]["source"]["useful_outcomes_per_resource"]["total_tokens"]["value"], 0)
        self.assertIsNone(zero["contrasts"]["total_tokens"]["useful_outcomes_per_resource_gain"])
        total = efficiency.aggregate([first, zero], "source", "portable")
        self.assertEqual(total["independent_task_count"], 2)
        self.assertEqual(total["by_condition"]["source"]["planned_attempts"], 4)
        self.assertEqual(total["by_condition"]["source"]["resources"]["total_tokens"]["total"], 520)
        self.assertEqual(total["by_condition"]["portable"]["resource_per_success"]["total_tokens"]["value"], 130)
        unknown = efficiency.compare(coordinator, None, "source", "portable")
        unknown["reviewer"] = "fixture"
        coordinator["slots"][1]["resources"]["output_tokens"] = None
        unknown_cost = efficiency.compare(coordinator, assessment, "source", "portable")
        total = efficiency.aggregate([first, unknown, unknown_cost], "source", "portable")
        candidate = total["by_condition"]["portable"]
        self.assertEqual(candidate["successful_outcomes"], {"lower": 2, "upper": 4})
        self.assertEqual(candidate["resources"]["total_tokens"]["unknown_attempts"], 1)
        self.assertIsNone(candidate["resources"]["total_tokens"]["total"])
        self.assertIsNone(candidate["useful_outcomes_per_resource"]["run_seconds"]["value"])

    def test_failures_unknown_resources_and_quality_losses_cannot_improve_claim(self):
        original, assessment = fixture()
        for mode in ("failed", "unknown_usage", "unreviewed", "zero_success"):
            with self.subTest(mode=mode):
                coordinator = copy.deepcopy(original)
                review = copy.deepcopy(assessment)
                if mode == "failed":
                    coordinator["slots"][1]["status"] = "timeout"
                elif mode == "unknown_usage":
                    coordinator["slots"][1]["resources"]["output_tokens"] = None
                elif mode == "unreviewed":
                    review = None
                else:
                    for item in review["reviews"]:
                        item["outcome"]["status"] = "unsatisfied"
                result = efficiency.compare(coordinator, review, "source", "portable")
                candidate = result["by_condition"]["portable"]
                if mode == "failed":
                    self.assertEqual(candidate["successful_outcomes"], {"lower": 1, "upper": 1})
                    self.assertFalse(result["quality_preserved_on_resolved_pairs"])
                    self.assertEqual(candidate["resources"]["total_tokens"]["total"], 130)
                else:
                    self.assertIsNone(candidate["resource_per_success"]["total_tokens"]["value"])
                    self.assertIsNone(result["contrasts"]["total_tokens"]["useful_outcomes_per_resource_gain"])
