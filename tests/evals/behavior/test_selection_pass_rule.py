import unittest

from evals.behavior import runner


class SelectionPassRuleTests(unittest.TestCase):
    EXPECT = {"contains": ["Skill: mastermind-diff-review"], "not_contains": ["Skill: mastermind-agent-security-review"]}

    def test_two_answer_lines_fail(self):
        pick, reasons = runner.selection_reasons(
            "Skill: mastermind-diff-review\nSkill: mastermind-agent-security-review", self.EXPECT
        )
        self.assertIsNone(pick)
        self.assertEqual(reasons, ["expected exactly one 'Skill:' or 'Agent:' line, found 2"])

    def test_backticks_quotes_and_spaces_are_stripped(self):
        for output in (
            "`Skill: mastermind-diff-review`",
            "Skill: `mastermind-diff-review`",
            '  "Skill: mastermind-diff-review"  \n',
            "**Skill:** mastermind-diff-review",
        ):
            with self.subTest(output=output):
                self.assertEqual(runner.selection_reasons(output, self.EXPECT), ("Skill: mastermind-diff-review", []))

    def test_names_must_match_exactly(self):
        for output in ("Skill: mastermind-diff-review-extra", "Skill: diff-review", "Skill: mastermind-diff-review, maybe"):
            with self.subTest(output=output):
                self.assertTrue(runner.selection_reasons(output, self.EXPECT)[1])

    def test_no_answer_line_fails(self):
        self.assertEqual(
            runner.selection_reasons("I would use the diff review skill.", self.EXPECT)[1],
            ["expected exactly one 'Skill:' or 'Agent:' line, found 0"],
        )

    def test_subagent_answers_count_when_expected(self):
        expect = {"contains_any": [["Skill: mastermind-test-audit", "Agent: mastermind-test-auditor"]]}
        self.assertEqual(runner.selection_reasons("Agent: mastermind-test-auditor", expect), ("Agent: mastermind-test-auditor", []))

    def test_repeated_runs_pass_on_a_majority(self):
        def run(passed, pick):
            return runner.Result("sel-x", "selection", passed, [] if passed else ["miss"], duration_ms=10, selection_pick=pick, telemetry_complete=True)

        combined = runner.combine_selection_runs([run(True, "Skill: a"), run(False, "Skill: b"), run(True, "Skill: a")])
        self.assertTrue(combined.passed)
        self.assertEqual(combined.reasons, [])
        self.assertEqual(combined.duration_ms, 30)
        failed = runner.combine_selection_runs([run(False, "Skill: b"), run(False, None), run(True, "Skill: a")])
        self.assertFalse(failed.passed)
        self.assertEqual(failed.reasons, ["passed 1 of 3 runs; picks: Skill: b, no answer, Skill: a"])

    def test_summary_reports_each_split_and_misses_by_expected_choice(self):
        cases = [
            {"id": "sel-a", "expect": {"contains": ["Skill: mastermind-a"]}},
            {"id": "sel-b", "expect": {"contains": ["Skill: mastermind-b"]}},
        ]
        runs = {
            "sel-a": [runner.Result("sel-a", "selection", True, selection_pick="Skill: mastermind-a")],
            "sel-b": [runner.Result("sel-b", "selection", False, ["miss"], selection_pick="Skill: mastermind-a")],
        }
        results = [runs["sel-a"][0], runs["sel-b"][0]]
        report = runner.summarize_selection(cases, runs, results, 1)
        self.assertEqual(sum(t["cases"] for t in report["splits"].values()), 2)
        self.assertEqual(sum(t["cases_passed"] for t in report["splits"].values()), 1)
        self.assertEqual([m["id"] for m in report["misses"]["Skill: mastermind-b"]], ["sel-b"])

    def test_extra_case_fields_never_reach_the_model(self):
        record = {
            "id": "sel-x",
            "why": "w",
            "lexical_exception": "reviewed",
            "tie_break": "a clause",
            "input": {"prompt": "Review PR 212 please."},
            "expect": {"contains": ["Skill: mastermind-diff-review"], "not_contains": ["Skill: mastermind-test-audit"]},
        }
        runner.validate_case_record("selection", record)
        with self.assertRaises(ValueError):
            runner.validate_case_record("selection", {**record, "lexical_exception": " "})
        self.assertEqual(runner._CASE_INPUT_FIELDS["selection"], ({"prompt"}, set()))


if __name__ == "__main__":
    unittest.main()
