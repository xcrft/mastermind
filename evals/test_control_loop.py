"""Guard deletion and result-accounting tests for the closure evaluation."""

import unittest

from evals import control_loop as loop


class ControlLoopTests(unittest.TestCase):
    def test_model_reaches_completion_and_recovers_from_every_open_state(self):
        result = loop.explore()
        self.assertEqual(result["violations"], [])
        self.assertGreater(result["publication_edges"], 0)
        self.assertGreater(result["open_states"], 1)
        self.assertEqual(result["open_states"], result["recoverable_open_states"])

    def test_each_deleted_guard_has_a_reachable_counterexample(self):
        report = loop.model_report()
        self.assertEqual(report["status"], "passed")
        for mutant in report["mutants"]:
            with self.subTest(guard=mutant["omitted_guard"]):
                counterexample = mutant["counterexample"]
                self.assertTrue(mutant["detected"])
                self.assertEqual(counterexample["trace"][-1], "publish")
                self.assertFalse(counterexample["before"][mutant["omitted_guard"]])
                state = loop.State()
                for action in counterexample["trace"]:
                    state = dict(loop.successors(state, mutant["omitted_guard"]))[action]
                self.assertTrue(state.completed)

    def test_new_preflight_discards_old_approval_and_receipts(self):
        completed = loop.State(True, True, True, True, True, True, True)
        next_state = dict(loop.successors(completed))["new_preflight"]
        self.assertEqual(next_state, loop.State(authorized=True))
        self.assertNotIn("publish", dict(loop.successors(next_state)))

    def test_missing_duplicate_ignored_failed_or_zero_tests_cannot_pass(self):
        valid = b"test expected ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored;\n"
        self.assertEqual(loop.test_outcome(valid, ("expected",))["status"], "passed")
        for invalid in (
            b"test result: ok. 0 passed; 0 failed; 0 ignored;\n",
            valid.replace(b"expected", b"another"),
            b"test expected ... ok\n" + valid,
            valid.replace(b"... ok", b"... ignored"),
            valid.replace(b"... ok", b"... FAILED"),
            valid.replace(b"1 passed", b"2 passed"),
        ):
            self.assertEqual(loop.test_outcome(invalid, ("expected",))["status"], "failed")


if __name__ == "__main__":
    unittest.main()
