"""Product objectives never upgrade legacy proxies or faster incorrect answers."""

import unittest

from evals.benchmark import efficiency, value
from tests.evals.support.measurements import paired_measurements as fixture


class ValueTests(unittest.TestCase):
    def measurement(self, basis="user_acceptance"):
        coordinator, assessment = fixture()
        return coordinator, assessment, efficiency.compare(
            coordinator, assessment, "source", "portable", outcome_basis=basis)

    def test_original_request_acceptance_and_useful_token_savings_have_distinct_targets(self):
        _, _, measurement = self.measurement()
        metrics = value.metric_rows(measurement, "source", "portable")
        self.assertEqual(metrics["request_acceptance"]["candidate"], {"lower": 1, "upper": 1})
        self.assertTrue(metrics["request_acceptance"]["target_met"])
        self.assertEqual(metrics["tokens_per_accepted_task"]["relative_saving"], 0.75)
        self.assertTrue(metrics["tokens_per_accepted_task"]["target_met"])
        self.assertIsNone(metrics["first_message_p50"]["candidate"])
        self.assertIsNone(metrics["relevant_profile_delivery"]["target_met"])

    def test_legacy_source_key_and_quality_regression_do_not_meet_product_targets(self):
        coordinator, assessment, legacy = self.measurement("full_source_key")
        metrics = value.metric_rows(legacy, "source", "portable")
        self.assertEqual(metrics["request_acceptance"]["status"], "unmeasured")
        self.assertIsNone(metrics["total_latency_p50"]["target_met"])
        assessment["reviews"][1]["outcome"]["status"] = "unsatisfied"
        bad = efficiency.compare(coordinator, assessment, "source", "portable", outcome_basis="user_acceptance")
        self.assertIsNone(value.metric_rows(bad, "source", "portable")["tokens_per_accepted_task"]["target_met"])

    def test_unresolved_requests_and_missing_usage_keep_targets_unknown(self):
        coordinator, assessment, _ = self.measurement()
        assessment["reviews"][1]["outcome"]["status"] = "unknown"
        coordinator["slots"][1]["resources"]["run_seconds"] = None
        measured = efficiency.compare(coordinator, assessment, "source", "portable", outcome_basis="user_acceptance")
        metrics = value.metric_rows(measured, "source", "portable")
        self.assertIsNone(metrics["request_acceptance"]["target_met"])
        self.assertIsNone(metrics["total_latency_p95"]["candidate"])
        self.assertIsNone(metrics["tokens_per_accepted_task"]["relative_saving"])
