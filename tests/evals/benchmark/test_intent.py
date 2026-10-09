"""Original-request acceptance through frozen trials and immutable review imports."""

import copy
import unittest

from evals.benchmark import artifacts, corpus, review, trials
from tests.evals.support.corpus import CorpusFixture


class IntentTests(unittest.TestCase):
    def setUp(self):
        self.case = CorpusFixture()
        self.addCleanup(self.case.close)
        self.case.key["acceptance_criteria"] = [{
            "id": "return_value", "request_excerpt": "What does value return?",
            "criterion": "State the value returned by the function with a source citation."}]
        self.case.key["required_knowns"].append({
            "claim": "The separate guide also documents the fixed service value.",
            "anchors": ["docs/guide.md:1"]})
        self.case.save()

    def test_criteria_must_name_the_original_request_and_have_unique_ids(self):
        self.case.check()
        original = copy.deepcopy(self.case.key)
        for mode in ("invented_request", "duplicate", "empty", "unexpected_field"):
            with self.subTest(mode=mode):
                key = copy.deepcopy(original)
                if mode == "invented_request":
                    key["acceptance_criteria"][0]["request_excerpt"] = "Optimize network throughput."
                elif mode == "duplicate":
                    key["acceptance_criteria"] *= 2
                elif mode == "empty":
                    key["acceptance_criteria"] = []
                else:
                    key["acceptance_criteria"][0]["hidden_answer"] = "7"
                with self.assertRaises(artifacts.BenchmarkError):
                    corpus.validate_key(self.case.task, key)

    def export(self):
        fixture = self.case.fixture
        fixture.adapter("init()\nfinal(answer='value returns 7, src/service.py:2.')\n")
        batch = trials.prepare_batch(task=self.case.task, rubric=self.case.key,
            config=fixture.config, source_repo=fixture.repo, tool_repo=fixture.repo,
            output=fixture.root / "batches", repetitions=1)
        for slot in artifacts.load_json(batch / "batch.json")["trials"]:
            trials.run_trial(batch / slot["directory"])
        output = fixture.root / "review"
        review.export_review(batch, output)
        assessment = artifacts.load_json(output / "reviewer/assessment-template.json")
        assessment["reviewer"] = "fixture"
        for item in assessment["reviews"]:
            item.update(
                claims=[{"quote": "value returns 7", "support": "supported", "anchors": ["src/service.py:2"],
                         "material_error": False, "rationale": "The function returns this value."}],
                knowns=[{"known_index": 0, "coverage": "covered", "answer_excerpt": "value returns 7",
                         "rationale": "The returned value is correctly stated."},
                        {"known_index": 1, "coverage": "missing", "answer_excerpt": None,
                         "rationale": "The separate guide was not requested or discussed."}],
                unknowns=[{"unknown_index": 0, "handling": "omitted", "answer_excerpt": None,
                           "rationale": "Caller behavior was not requested."}],
                acceptance=[{"criterion_id": "return_value", "status": "met", "answer_excerpt": "value returns 7",
                             "rationale": "The original request is answered with a citation."}],
                outcome={"status": "satisfied", "rationale": "The request is fulfilled."})
        return output, assessment

    def submit(self, output, assessment):
        path = self.case.fixture.root / "assessment.json"
        path.write_bytes(artifacts.canonical(assessment))
        return review.import_assessment(output, path)

    def test_user_acceptance_is_distinct_from_source_coverage_and_bound_into_comparison(self):
        output, assessment = self.export()
        self.assertEqual(assessment["schema_version"], 3)
        self.submit(output, assessment)
        result = review.compare_review(output, "source", "portable")
        measured = result["efficiency"][0]
        self.assertEqual(measured["outcome_basis"], "user_acceptance")
        self.assertEqual(measured["by_condition"]["portable"]["successful_outcomes"], {"lower": 1, "upper": 1})
        self.assertTrue(measured["quality_preserved_on_resolved_pairs"])
        self.assertFalse(result["comparison_accepted"])
        summary = review.review_status(output)["assessment_summary"]["overall"]
        self.assertEqual(summary["request_criteria"], {"met": 3, "unmet": 0, "unknown": 0})

    def test_missing_evidence_material_errors_overclaims_and_schema_downgrade_cannot_pass(self):
        output, original = self.export()
        for mode in ("unmet", "unknown", "missing", "absent_excerpt", "invented_excerpt",
                     "material_error", "overclaim", "downgrade"):
            with self.subTest(mode=mode):
                assessment = copy.deepcopy(original)
                row = assessment["reviews"][0]
                if mode in ("unmet", "unknown"):
                    row["acceptance"][0]["status"] = mode
                elif mode == "missing":
                    row["acceptance"] = []
                elif mode == "absent_excerpt":
                    row["acceptance"][0]["answer_excerpt"] = None
                elif mode == "invented_excerpt":
                    row["acceptance"][0]["answer_excerpt"] = "answer that was never emitted"
                elif mode == "material_error":
                    row["claims"][0].update(support="unsupported", material_error=True, anchors=[])
                elif mode == "overclaim":
                    row["unknowns"][0].update(handling="overclaimed", answer_excerpt="value returns 7")
                else:
                    assessment["schema_version"] = 2
                    for item in assessment["reviews"]:
                        item.pop("acceptance")
                with self.assertRaises(artifacts.BenchmarkError):
                    self.submit(output, assessment)
        self.assertFalse((output / "reviews").exists())

    def test_unknown_acceptance_cannot_be_converted_into_known_success_or_failure(self):
        output, assessment = self.export()
        for row in assessment["reviews"]:
            row["acceptance"][0].update(status="unknown", answer_excerpt=None)
            row["outcome"]["status"] = "unknown"
        self.submit(output, assessment)
        measured = review.compare_review(output, "source", "portable")["efficiency"][0]
        self.assertEqual(measured["by_condition"]["source"]["successful_outcomes"], {"lower": 0, "upper": 1})
        self.assertEqual(measured["value"]["status"], "unresolved_quality")
        self.assertIsNone(measured["value"]["quality_gated_resource_savings"]["total_tokens"])
