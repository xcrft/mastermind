"""Delivery evidence preserves gaps and cannot become a quality judgment."""

import copy
import json
from pathlib import Path
import tempfile
import unittest

from evals.persona import delivery


class DeliveryTests(unittest.TestCase):
    def setUp(self):
        self.rules = [{"key": "lock", "statement": "Keep native calls outside the lock unless exclusivity is required."},
                      {"key": "tests", "statement": "Synchronize concurrent tests on observable readiness."}]
        self.packet = {"status": "ok", "source_verification": "complete", "profile_revision": "view",
                       "store_revision": "store", "selection": {"role": "executor"}, "omitted": [],
                       "feedback": [dict(row, status="active", review_revision="review-" + row["key"]) for row in self.rules]}

    def test_delivery_does_not_establish_adherence_and_retains_whole_statement_differences(self):
        result = delivery.compare(self.rules, self.packet)
        self.assertEqual(result["exact_text_delivery_fraction"], 1)
        self.assertIsNone(result["model_application"])
        self.assertIsNone(result["semantic_quality"])
        self.assertFalse(result["comparison_accepted"])
        self.assertEqual(result["review_revisions"], {"lock": "review-lock", "tests": "review-tests"})
        changed = copy.deepcopy(self.packet)
        changed["feedback"][0]["statement"] = "Keep native calls outside the lock."
        changed["feedback"].pop()
        changed["omitted"] = ["feedback"]
        result = delivery.compare(self.rules, changed)
        self.assertEqual(result["changed_expected_rules"], ["lock"])
        self.assertEqual(result["missing_expected_rules"], ["tests"])
        self.assertEqual(result["exact_text_delivery_fraction"], 0)
        self.assertEqual(result["omitted"], ["feedback"])
        denied = delivery.compare(self.rules, {"status": "access_denied"})
        self.assertEqual(denied["missing_expected_rules"], ["lock", "tests"])
        self.assertIsNone(denied["reported_source_verification"])
        self.assertEqual(delivery.compare([], {"status": "ok"})["exact_text_delivery_fraction"], None)

    def test_invalid_identity_and_unaccepted_rows_are_rejected_and_cli_output_is_immutable(self):
        for mode in ("duplicate_expected", "duplicate_received", "candidate", "invalid_statement", "denied_with_data"):
            expected, packet = copy.deepcopy(self.rules), copy.deepcopy(self.packet)
            if mode == "duplicate_expected": expected.append(expected[0])
            elif mode == "duplicate_received": packet["feedback"].append(packet["feedback"][0])
            elif mode == "candidate": packet["feedback"][0]["status"] = "candidate"
            elif mode == "invalid_statement": packet["feedback"][0]["statement"] = None
            else: packet["status"] = "access_denied"
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                delivery.compare(expected, packet)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "expected.json").write_text(json.dumps(self.rules))
            (root / "packet.json").write_text(json.dumps(self.packet))
            output = root / "result.json"
            args = ["--expected", str(root / "expected.json"), "--packet", str(root / "packet.json"), "--output", str(output)]
            self.assertEqual(delivery.main(args), 0)
            before = output.read_bytes()
            self.assertEqual(json.loads(before)["returned_rules"], 2)
            self.assertIn("packet", json.loads(before)["input_sha256"])
            with self.assertRaises(FileExistsError): delivery.main(args)
            self.assertEqual(output.read_bytes(), before)
