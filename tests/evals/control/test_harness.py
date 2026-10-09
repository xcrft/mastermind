"""Guard deletion and result-accounting tests for the closure evaluation."""

from contextlib import redirect_stdout
from itertools import product
from pathlib import Path
from unittest.mock import patch
import io
import tempfile
import unittest

from evals.control import harness as loop
from evals.shared.process import ProcessResult


UI_PASSED = (
    f"TAP version 13\n# {loop.UI_COMPLETION}\n"
    f"# Subtest: {loop.UI_SUITE}\nok 1 - {loop.UI_SUITE}\n"
    "  ---\n  duration_ms: 10.0\n  ...\n1..1\n"
    "# tests 1\n# suites 0\n# pass 1\n# fail 0\n"
    "# cancelled 0\n# skipped 0\n# todo 0\n# duration_ms 20.0\n"
).encode()


class ControlLoopTests(unittest.TestCase):
    def test_cli_selection_uses_the_requested_checkout_and_includes_profile_delivery_guards(self):
        observed = []
        def execute(command, **kwargs):
            observed.append((command, kwargs["cwd"]))
            selectors = command[command.index("--test-threads=2") + 1:]
            body = "".join(f"test {name} ... ok\n" for name in selectors)
            body += f"test result: ok. {len(selectors)} passed; 0 failed; 0 ignored;\n"
            return ProcessResult(stdout=body.encode(), stderr=b"", returncode=0, stop_reason=None)
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(loop.shutil, "which", return_value="/fixture/cargo"), \
                patch.object(loop, "CASES", {"profile_budget_cli": loop.CASES["profile_budget_cli"]}), \
                patch.object(loop, "run_bounded", side_effect=execute), \
                redirect_stdout(io.StringIO()):
            result = loop.run_cli(Path(directory), Path(directory) / "current-main")
        self.assertTrue(all(row["status"] == "passed" for row in result))
        self.assertEqual(len(observed), 2)
        self.assertTrue(all(cwd == Path(directory) / "current-main/mcp/servers/mmcg" for _, cwd in observed))
        self.assertIn("--lib", observed[1][0])
        self.assertIn("miner::profile::tests::feedback_delivers_a_ranked_prefix_instead_of_all_or_nothing", observed[1][0])
        self.assertIn("mcp::tests::resolve_profile_budget_uses_argument_then_project_setting_then_default", observed[1][0])

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

    def test_ui_requires_completed_unskipped_exact_aggregate_suite(self):
        for label in (loop.UI_SUITE, str(loop.ROOT / loop.UI_SUITE)):
            with self.subTest(label=label):
                output = UI_PASSED.replace(loop.UI_SUITE.encode(), label.encode())
                result = loop.ui_outcome(output)
                self.assertEqual(result["status"], "passed")
                self.assertEqual(result["aggregation"], "one_file_suite")
                self.assertEqual(result["passed_suites"], 1)
        for invalid in (
            UI_PASSED.replace(loop.UI_COMPLETION.encode(), b"helper stopped early"),
            UI_PASSED.replace(loop.UI_SUITE.encode(), b"another.test.cjs"),
            UI_PASSED + f"ok 1 - {loop.UI_SUITE}\n".encode(),
            UI_PASSED.replace(b"ok 1 -", b"not ok 1 -"),
            UI_PASSED.replace(b"# skipped 0", b"# skipped 1"),
            UI_PASSED.replace(b"# cancelled 0", b"# cancelled 1"),
            UI_PASSED.replace(b"# todo 0", b"# todo 1"),
            UI_PASSED.replace(b"1..1", b"1..0"),
            UI_PASSED.replace(b"# tests 1", b"# tests 0"),
            UI_PASSED.replace(b"# fail 0", b"# fail 1"),
            UI_PASSED.replace(b"# pass 1\n", b""),
            UI_PASSED + b"# tests 1\n",
        ):
            with self.subTest(output=invalid):
                self.assertEqual(loop.ui_outcome(invalid)["status"], "failed")

    def test_native_process_failure_cannot_be_hidden_by_passing_ui_output(self):
        for code, stop_reason in ((1, None), (0, "timeout"), (None, "stdout_limit")):
            with self.subTest(code=code, stop_reason=stop_reason):
                child = ProcessResult(stdout=UI_PASSED, stderr=b"child diagnostics\n",
                                      returncode=code, stop_reason=stop_reason)
                with tempfile.TemporaryDirectory() as directory, \
                        patch.object(loop.shutil, "which", return_value="/fixture/node"), \
                        patch.object(loop, "run_bounded", return_value=child), \
                        redirect_stdout(io.StringIO()):
                    output = Path(directory)
                    result = loop.run_ui(output)
                    self.assertEqual(result["status"], "failed")
                    self.assertEqual(result["exit_code"], code)
                    self.assertEqual(result["stop_reason"], stop_reason)
                    self.assertEqual((output / "lens.stdout").read_bytes(), UI_PASSED)
                    self.assertEqual((output / "lens.stderr").read_bytes(), child.stderr)

    def test_cli_process_failure_cannot_be_hidden_by_passing_selected_tests(self):
        child = ProcessResult(
            stdout=b"test expected ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;\n",
            stderr=b"bounded process stopped\n", returncode=0, stop_reason="timeout")
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(loop.shutil, "which", return_value="/fixture/cargo"), \
                patch.object(loop, "CASES", {"fixture": ("expected",)}), \
                patch.object(loop, "run_bounded", return_value=child), \
                redirect_stdout(io.StringIO()):
            output = Path(directory)
            result = loop.run_cli(output)
            self.assertEqual(result[0]["status"], "failed")
            self.assertEqual((output / "fixture.stdout").read_bytes(), child.stdout)
            self.assertEqual((output / "fixture.stderr").read_bytes(), child.stderr)

    def test_missing_runtimes_fail_before_attempting_execution(self):
        for runner in (loop.run_cli, loop.run_ui):
            with self.subTest(runner=runner.__name__), \
                    patch.object(loop.shutil, "which", return_value=None), \
                    patch.object(loop, "run_bounded") as child:
                with self.assertRaises(ValueError):
                    runner(Path("unused"))
                child.assert_not_called()

    def test_overall_pass_requires_model_cli_ui_and_unchanged_sources(self):
        for model, cli, ui, unchanged in product(
                ("passed", "failed"), ("passed", "failed", "not_run"),
                ("passed", "failed", "not_run"), (True, False)):
            with self.subTest(model=model, cli=cli, ui=ui, unchanged=unchanged):
                result = loop.report_status({
                    "bounded_model_safety": {"status": model},
                    "sampled_cli_conformance": cli, "sampled_ui_conformance": ui,
                    "source_unchanged": unchanged,
                })
                if "failed" in (model, cli, ui) or not unchanged:
                    self.assertEqual(result, "failed")
                elif cli == "not_run" or ui == "not_run":
                    self.assertEqual(result, "incomplete")
                else:
                    self.assertEqual(result, "passed")


if __name__ == "__main__":
    unittest.main()
