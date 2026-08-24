import json
import io
import os
import shlex
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from copy import deepcopy
from pathlib import Path
from unittest.mock import patch

from evals import runner


RESOLVED_MODEL = "claude-opus-test"


def sync_gated_summaries(report, suite_name):
    cases = [case for case in report["cases"] if case["suite"] == suite_name]
    passed = sum(case["passed"] for case in cases)
    first_pass = sum(case["passed"] and not case["retry_used"] for case in cases)
    summary = report["suites"][suite_name]
    summary["quality"] = {
        "passed": passed,
        "total": len(cases),
        "pass_rate": passed / len(cases),
        "first_pass": first_pass,
        "first_pass_rate": first_pass / len(cases),
    }
    summary["usage"]["context_tokens"] = runner.metric_summary(
        [case["usage"]["context_tokens"] for case in cases]
    )
    summary["telemetry"] = {
        "complete": all(case["telemetry"]["complete"] for case in cases),
        "incomplete_cases": [
            case["id"] for case in cases if not case["telemetry"]["complete"]
        ],
    }


class StructuredOutputTests(unittest.TestCase):
    def test_audit_verdict_requires_valid_sentinel_yaml(self):
        valid = """\
<!-- mastermind:audit-begin -->
```yaml
verdict: drift
```
<!-- mastermind:audit-end -->
"""
        self.assertEqual(runner.extract_audit_verdict(valid), "drift")
        self.assertIsNone(runner.extract_audit_verdict("verdict: held"))
        self.assertIsNone(
            runner.extract_audit_verdict(
                "<!-- mastermind:audit-begin -->\n```yaml\nverdict: [\n```\n<!-- mastermind:audit-end -->"
            )
        )

    def test_audit_verification_requires_exact_passing_structured_rerun(self):
        output = """\
<!-- mastermind:audit-begin -->
```yaml
verdict: held
verifications_rerun:
  - cmd: "cargo test --locked exact_test"
    result: pass
```
<!-- mastermind:audit-end -->
"""
        self.assertTrue(
            runner.audit_verification_passed(
                output, "cargo test --locked exact_test"
            )
        )
        self.assertFalse(
            runner.audit_verification_passed(output, "cargo test --locked other_test")
        )

    def test_intake_action_requires_valid_sentinel_yaml(self):
        valid = """\
<!-- mastermind:intake-begin -->
```yaml
action: passthrough
```
<!-- mastermind:intake-end -->
"""
        self.assertEqual(runner.extract_intake_action(valid), "passthrough")
        self.assertIsNone(runner.extract_intake_action("action: refined"))

    def test_failure_diagnostics_are_bounded(self):
        result = runner.Result("case", "workflow", False, output_excerpt="x" * 4000)
        self.assertEqual(len(result.output_excerpt), 4000)

    def test_cli_usage_telemetry_is_parsed_without_guessing(self):
        telemetry = runner.telemetry_from_payload(
            {
                "duration_ms": 1500,
                "duration_api_ms": 1234,
                "num_turns": 3,
                "total_cost_usd": 0.0125,
                "usage": {
                    "input_tokens": 101,
                    "output_tokens": 202,
                    "cache_creation_input_tokens": 303,
                    "cache_read_input_tokens": 404,
                },
                "modelUsage": {RESOLVED_MODEL: {}},
                "_resolved_model": RESOLVED_MODEL,
            }
        )
        self.assertEqual(telemetry["duration_ms"], 1500)
        self.assertEqual(telemetry["duration_api_ms"], 1234)
        self.assertEqual(telemetry["num_turns"], 3)
        self.assertEqual(telemetry["input_tokens"], 101)
        self.assertEqual(telemetry["output_tokens"], 202)
        self.assertEqual(telemetry["cache_creation_input_tokens"], 303)
        self.assertEqual(telemetry["cache_read_input_tokens"], 404)
        self.assertEqual(telemetry["cost_usd"], 0.0125)
        self.assertEqual(telemetry["resolved_models"], [RESOLVED_MODEL])
        self.assertTrue(telemetry["complete"])
        self.assertEqual(telemetry["issues"], [])

    def test_cli_usage_telemetry_marks_missing_or_invalid_fields_incomplete(self):
        telemetry = runner.telemetry_from_payload(
            {
                "duration_ms": 100,
                "duration_api_ms": -1,
                "num_turns": "one",
                "total_cost_usd": float("nan"),
                "usage": {"input_tokens": 1},
            }
        )
        self.assertFalse(telemetry["complete"])
        self.assertIn(
            "duration_api_ms must be a non-negative integer", telemetry["issues"]
        )
        self.assertIn(
            "usage.output_tokens must be a non-negative integer", telemetry["issues"]
        )

    def test_usage_budgets_bound_turns_and_output_without_guessing_context(self):
        telemetry = {"num_turns": 9, "output_tokens": 1801}
        reasons = runner.usage_budget_reasons(
            {"min_turns": 2, "max_turns": 8, "max_output_tokens": 1800},
            telemetry,
        )
        self.assertEqual(
            reasons,
            [
                "used 9 turn(s), expected at most 8",
                "used 1801 output token(s), expected at most 1800",
            ],
        )

    def test_stream_parser_records_tool_identities_and_final_payload(self):
        events = [
            {
                "type": "system",
                "subtype": "init",
                "model": RESOLVED_MODEL,
            },
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "name": "mcp__mmcg__mmcg_search",
                            "input": {"query": "secret input is not persisted"},
                        }
                    ]
                },
            },
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {"type": "tool_use", "name": "Read", "input": {}}
                    ]
                },
            },
            {
                "type": "result",
                "result": "done",
                "duration_ms": 1,
                "duration_api_ms": 1,
                "num_turns": 2,
                "total_cost_usd": 0.01,
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 1,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                },
                "modelUsage": {RESOLVED_MODEL: {}},
            },
        ]
        payload, tool_calls = runner.parse_claude_output(
            "\n".join(json.dumps(event) for event in events), streamed=True
        )

        self.assertEqual(payload["result"], "done")
        self.assertEqual(payload["_resolved_model"], RESOLVED_MODEL)
        self.assertEqual(
            tool_calls, ["mcp__mmcg__mmcg_search", "Read"]
        )

    def test_tool_policy_requires_mmcg_first_and_bounded_source_read(self):
        expect = {
            "tools": {
                "first": "mcp__mmcg__mmcg_search",
                "contains": ["mcp__mmcg__mmcg_search", "Read"],
                "max": 3,
                "max_counts": {"Read": 1},
            }
        }
        self.assertEqual(
            runner.tool_usage_reasons(
                expect, ["mcp__mmcg__mmcg_search", "Read"]
            ),
            [],
        )
        reasons = runner.tool_usage_reasons(expect, ["Grep", "Read"])
        self.assertTrue(any("first tool" in reason for reason in reasons))
        self.assertTrue(any("required tool" in reason for reason in reasons))
        self.assertTrue(
            any("expected at most 0" in reason for reason in runner.tool_usage_reasons(
                {"tools": {"max": 0}}, ["Read"]
            ))
        )
        self.assertTrue(
            any("'Read' 2 time(s)" in reason for reason in runner.tool_usage_reasons(
                {"tools": {"max_counts": {"Read": 1}}}, ["Read", "Read"]
            ))
        )
    def test_retry_usage_is_aggregated_across_both_attempts(self):
        first = runner.Result(
            "case",
            "auditor",
            False,
            duration_ms=100,
            duration_api_ms=80,
            num_turns=2,
            input_tokens=10,
            output_tokens=20,
            cache_creation_input_tokens=30,
            cache_read_input_tokens=40,
            cost_usd=0.01,
            telemetry_complete=True,
            resolved_models=[RESOLVED_MODEL],
            tool_calls=["Read"],
        )
        second = runner.Result(
            "case",
            "auditor",
            True,
            duration_ms=200,
            duration_api_ms=160,
            num_turns=3,
            input_tokens=11,
            output_tokens=21,
            cache_creation_input_tokens=31,
            cache_read_input_tokens=41,
            cost_usd=0.02,
            telemetry_complete=True,
            resolved_models=[RESOLVED_MODEL],
            tool_calls=["Grep"],
        )
        second.add_attempt(first)
        self.assertEqual(second.duration_ms, 300)
        self.assertEqual(second.duration_api_ms, 240)
        self.assertEqual(second.num_turns, 5)
        self.assertEqual(second.input_tokens, 21)
        self.assertEqual(second.output_tokens, 41)
        self.assertEqual(second.cache_creation_input_tokens, 61)
        self.assertEqual(second.cache_read_input_tokens, 81)
        self.assertAlmostEqual(second.cost_usd, 0.03)
        self.assertTrue(second.telemetry_complete)
        self.assertEqual(second.resolved_models, [RESOLVED_MODEL])
        self.assertEqual(second.tool_calls, ["Read", "Grep"])

        incomplete = runner.Result(
            "case", "auditor", False, telemetry_issues=["usage missing"]
        )
        second.add_attempt(incomplete)
        self.assertFalse(second.telemetry_complete)
        self.assertEqual(second.telemetry_issues, ["usage missing"])

    def test_nearest_rank_small_samples_are_explicit(self):
        with self.assertRaisesRegex(ValueError, "empty sample"):
            runner.nearest_rank([], 95)
        self.assertEqual(runner.nearest_rank([7], 50), 7)
        self.assertEqual(runner.nearest_rank([7], 95), 7)
        self.assertEqual(runner.nearest_rank([1, 2, 3], 95), 3)
        with self.assertRaisesRegex(ValueError, "between 1 and 100"):
            runner.nearest_rank([1], 0)

    def test_report_persists_raw_telemetry_and_nearest_rank_percentiles(self):
        results = [
            runner.Result(
                f"case-{index}",
                "critic",
                True,
                duration_ms=index * 10,
                duration_api_ms=index * 8,
                num_turns=1,
                input_tokens=index,
                output_tokens=index * 2,
                cache_creation_input_tokens=index * 10,
                cache_read_input_tokens=0,
                cost_usd=index / 100,
                telemetry_complete=True,
                resolved_models=[RESOLVED_MODEL],
            )
            for index in range(1, 6)
        ]
        report = runner.build_report(
            results, model="opus", suite_filter="critic", case_filter=None
        )
        report["claude_cli_version"] = "test-cli"
        report["suites"]["critic"]["case_definition_digest"] = "c" * 64
        context = report["suites"]["critic"]["usage"]["context_tokens"]
        self.assertEqual(context, {"total": 165, "p50": 33, "p95": 55})
        self.assertEqual(report["cases"][0]["usage"]["context_tokens"], 11)
        self.assertEqual(report["cases"][0]["turns"], 1)

        with tempfile.TemporaryDirectory() as target:
            path = runner.Path(target) / "nested" / "report.json"
            runner.write_report(path, report)
            loaded = runner.load_report(path)
        self.assertEqual(loaded["kind"], runner.REPORT_KIND)
        self.assertEqual(loaded["cases"], report["cases"])

    def test_baseline_gate_requires_quality_and_lower_p50_p95_context(self):
        baseline_results = [
            runner.Result(
                f"c-{index}",
                "critic",
                True,
                input_tokens=value,
                telemetry_complete=True,
                resolved_models=[RESOLVED_MODEL],
            )
            for index, value in enumerate([100, 110, 120, 130, 140], start=1)
        ]
        current_results = [
            runner.Result(
                f"c-{index}",
                "critic",
                True,
                input_tokens=value,
                telemetry_complete=True,
                resolved_models=[RESOLVED_MODEL],
            )
            for index, value in enumerate([80, 90, 100, 110, 120], start=1)
        ]
        baseline = runner.build_report(
            baseline_results, model="opus", suite_filter="critic", case_filter=None
        )
        current = runner.build_report(
            current_results, model="opus", suite_filter="critic", case_filter=None
        )
        digest = "a" * 64
        for report in (baseline, current):
            report["claude_cli_version"] = "test-cli"
            report["suites"]["critic"]["case_definition_digest"] = digest
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(gate["passed"])
        self.assertEqual(len(gate["checks"]), 8)

        inconsistent = deepcopy(current)
        inconsistent["suites"]["critic"]["quality"]["pass_rate"] = 0.8
        inconsistent["suites"]["critic"]["usage"]["context_tokens"]["p95"] = 1
        gate = runner.compare_to_baseline(inconsistent, baseline)
        self.assertFalse(gate["passed"])
        self.assertTrue(
            any("quality summary does not match raw cases" in item for item in gate["failures"])
        )
        self.assertTrue(
            any("context summary does not match raw cases" in item for item in gate["failures"])
        )

        current["cases"][0]["passed"] = False
        last_usage = current["cases"][-1]["usage"]
        last_usage["input_tokens"] = 140
        last_usage["context_tokens"] = 140
        last_usage["total_tokens"] = 140
        sync_gated_summaries(current, "critic")
        gate = runner.compare_to_baseline(current, baseline)
        self.assertFalse(gate["passed"])
        self.assertTrue(any("pass rate regressed" in item for item in gate["failures"]))
        self.assertTrue(any("p95 context tokens" in item for item in gate["failures"]))

    def test_baseline_gate_rejects_compensating_case_regression(self):
        baseline_results = [
            runner.Result(
                "a", "critic", True, input_tokens=100,
                telemetry_complete=True, resolved_models=[RESOLVED_MODEL],
            ),
            runner.Result(
                "b", "critic", False, input_tokens=100,
                telemetry_complete=True, resolved_models=[RESOLVED_MODEL],
            ),
        ]
        current_results = [
            runner.Result(
                "a", "critic", False, input_tokens=90,
                telemetry_complete=True, resolved_models=[RESOLVED_MODEL],
            ),
            runner.Result(
                "b", "critic", True, input_tokens=90,
                telemetry_complete=True, resolved_models=[RESOLVED_MODEL],
            ),
        ]
        baseline = runner.build_report(
            baseline_results, model="opus", suite_filter="critic", case_filter=None
        )
        current = runner.build_report(
            current_results, model="opus", suite_filter="critic", case_filter=None
        )
        for report in (baseline, current):
            report["claude_cli_version"] = "test-cli"
            report["suites"]["critic"]["case_definition_digest"] = "b" * 64
        gate = runner.compare_to_baseline(current, baseline)
        self.assertFalse(gate["passed"])
        self.assertTrue(
            any("baseline-passing case regressed: a" in item for item in gate["failures"])
        )

    def test_baseline_gate_fails_closed_on_model_or_case_drift(self):
        baseline_results = [
            runner.Result(
                "c-1", "critic", True, input_tokens=10, telemetry_complete=True,
                resolved_models=[RESOLVED_MODEL],
            )
        ]
        current_results = [
            runner.Result(
                "c-2", "critic", True, input_tokens=5, telemetry_complete=True,
                resolved_models=[RESOLVED_MODEL],
            )
        ]
        baseline = runner.build_report(
            baseline_results, model="opus", suite_filter="critic", case_filter=None
        )
        current = runner.build_report(
            current_results, model="sonnet", suite_filter="critic", case_filter=None
        )
        baseline["claude_cli_version"] = "test-cli"
        current["claude_cli_version"] = "test-cli"
        baseline["suites"]["critic"]["case_definition_digest"] = "c" * 64
        current["suites"]["critic"]["case_definition_digest"] = "d" * 64
        gate = runner.compare_to_baseline(current, baseline)
        self.assertFalse(gate["passed"])
        self.assertTrue(any("model mismatch" in item for item in gate["failures"]))
        self.assertTrue(any("case set/order differs" in item for item in gate["failures"]))

    def test_baseline_gate_fails_closed_on_environment_definition_or_telemetry_drift(self):
        results = [
            runner.Result(
                "c-1", "critic", True, input_tokens=10, telemetry_complete=True,
                resolved_models=[RESOLVED_MODEL],
            )
        ]
        baseline = runner.build_report(
            results, model="opus", suite_filter="critic", case_filter=None
        )
        baseline["claude_cli_version"] = "test-cli"
        baseline["suites"]["critic"]["case_definition_digest"] = "e" * 64

        current = deepcopy(baseline)
        current["resolved_models"] = ["different-resolved-model"]
        current["cases"][0]["resolved_models"] = ["different-resolved-model"]
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(any("resolved model mismatch" in item for item in gate["failures"]))

        current = deepcopy(baseline)
        current["claude_cli_version"] = "other-cli"
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(any("CLI version mismatch" in item for item in gate["failures"]))

        current = deepcopy(baseline)
        current["filters"]["case"] = "c-1"
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(any("filters differ" in item for item in gate["failures"]))

        current = deepcopy(baseline)
        current["suites"]["critic"]["case_definition_digest"] = "f" * 64
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(
            any("case definitions differ" in item for item in gate["failures"])
        )

        current = deepcopy(baseline)
        current["cases"][0]["telemetry"] = {
            "complete": False,
            "issues": ["usage missing"],
        }
        sync_gated_summaries(current, "critic")
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(any("incomplete telemetry" in item for item in gate["failures"]))

    def test_gate_rejects_raw_context_values_that_do_not_match_usage(self):
        results = [
            runner.Result(
                "c-1",
                "critic",
                True,
                input_tokens=100,
                telemetry_complete=True,
                resolved_models=[RESOLVED_MODEL],
            )
        ]
        baseline = runner.build_report(
            results, model="opus", suite_filter="critic", case_filter=None
        )
        baseline["claude_cli_version"] = "test-cli"
        baseline["suites"]["critic"]["case_definition_digest"] = "f" * 64
        current = deepcopy(baseline)
        current["cases"][0]["usage"]["input_tokens"] = 999_999
        current["suites"]["critic"]["usage"]["context_tokens"] = {
            "total": 1,
            "p50": 1,
            "p95": 1,
        }

        gate = runner.compare_to_baseline(current, baseline)

        self.assertFalse(gate["passed"])
        self.assertTrue(
            any("context tokens do not match raw usage" in item for item in gate["failures"])
        )

    def test_report_loader_rejects_malformed_comparison_records(self):
        results = [
            runner.Result(
                "c-1", "critic", True, input_tokens=10, telemetry_complete=True,
                resolved_models=[RESOLVED_MODEL],
            )
        ]
        report = runner.build_report(
            results, model="opus", suite_filter="critic", case_filter=None
        )
        report["claude_cli_version"] = "test-cli"
        report["suites"]["critic"]["case_definition_digest"] = "a" * 64
        report["cases"] = [None]

        with tempfile.TemporaryDirectory() as target:
            path = runner.Path(target) / "malformed.json"
            runner.write_report(path, report)
            with self.assertRaisesRegex(ValueError, "case 0 must be an object"):
                runner.load_report(path)

    def test_fixture_content_changes_case_definition_digest(self):
        with tempfile.TemporaryDirectory() as target:
            fixture_root = Path(target) / "fake-session"
            baseline = fixture_root / "baseline"
            after = fixture_root / "changes" / "clean-add"
            baseline.mkdir(parents=True)
            after.mkdir(parents=True)
            (baseline / "source.rs").write_text("fn before() {}\n")
            (after / "source.rs").write_text("fn after() {}\n")
            with patch.object(runner, "FIXTURES_DIR", Path(target)):
                before = runner.case_definition_digest(
                    "researcher", ["r-001-structural-source-cross-check"]
                )
                (after / "source.rs").write_text("fn changed() {}\n")
                changed = runner.case_definition_digest(
                    "researcher", ["r-001-structural-source-cross-check"]
                )
        self.assertIsNotNone(before)
        self.assertNotEqual(before, changed)

    def test_unknown_case_filter_is_a_nonzero_cli_error(self):
        argv = [
            "runner.py",
            "--suite",
            "critic",
            "--case",
            "definitely-not-a-case",
            "--baseline-report",
            "/tmp/unused-baseline.json",
        ]
        output = io.StringIO()
        with (
            patch.object(runner.sys, "argv", argv),
            patch.object(runner.shutil, "which", return_value="/usr/bin/tool"),
            redirect_stdout(output),
        ):
            status = runner.main()
        self.assertEqual(status, 2)
        self.assertIn("no cases matched filter", output.getvalue())

    def test_alternative_phrases_accept_equivalent_wording(self):
        self.assertTrue(
            runner.contains_any_phrase("No spec file is needed.", ["no task spec", "no spec file"])
        )
        self.assertFalse(runner.contains_any_phrase("Write a strict spec.", ["no task spec"]))

    def test_code_comment_policy_rejects_slop_and_accepts_zero_comments(self):
        clean = """```ts
export const double = (value: number) => value * 2;
export const docs = "https://example.com/reference";
```"""
        noisy = "```ts\n// Double the value\nreturn value * 2; // return result\n```"
        policy = {"prefixes": ["//"], "max": 0}
        self.assertEqual(runner.code_comment_policy_reasons(clean, policy), [])
        reasons = runner.code_comment_policy_reasons(noisy, policy)
        self.assertTrue(any("expected at most 0" in reason for reason in reasons))

    def test_code_comment_policy_can_require_one_non_obvious_reason(self):
        output = """```ts
// Keep the loop constant-time: an early return leaks prefix length.
for (let index = 0; index < left.length; index += 1) mismatch |= left[index] ^ right[index];
```"""
        policy = {
            "prefixes": ["//"],
            "min": 1,
            "max": 1,
            "contains_any": [["constant-time", "timing"]],
        }
        self.assertEqual(runner.code_comment_policy_reasons(output, policy), [])


class PromptIsolationTests(unittest.TestCase):
    def test_auditor_runs_verify_as_an_exact_standalone_command(self):
        auditor = runner.SUITES["auditor"]["subagent"].read_text(encoding="utf-8")
        self.assertIn("Run each reported\n   `VERIFY` command exactly as written", auditor)
        self.assertIn("do not prepend `cd`", auditor)
        self.assertIn("do not append pipes", auditor)

    def test_synthetic_prompt_suites_cannot_inspect_the_maintainer_checkout(self):
        for suite in ("critic", "intake", "workflow"):
            self.assertEqual(runner.isolated_cli_args(suite), ["--safe-mode", "--tools", ""])
            self.assertTrue(runner.requires_prompt_sandbox(suite))
        researcher_args = runner.isolated_cli_args("researcher")
        self.assertIn("--strict-mcp-config", researcher_args)
        researcher_allowed = researcher_args[
            researcher_args.index("--allowedTools") + 1
        ]
        self.assertIn("mcp__mmcg__mmcg_search", researcher_allowed)
        self.assertIn("mcp__mmcg__mmcg_callers", researcher_allowed)
        self.assertNotIn("mcp__mmcg__*", researcher_allowed)
        self.assertFalse(runner.requires_prompt_sandbox("researcher"))
        auditor_args = runner.isolated_cli_args("auditor")
        self.assertIn("--strict-mcp-config", auditor_args)
        self.assertIn("--setting-sources", auditor_args)
        self.assertIn("--allowedTools", auditor_args)
        allowed = auditor_args[auditor_args.index("--allowedTools") + 1]
        for tool in (
            "Read",
            "Glob",
            "Grep",
            "Bash(git diff *)",
            "Bash(git status *)",
            "Bash(cargo test --locked *)",
            "mcp__mmcg__mmcg_status",
            "mcp__mmcg__mmcg_search",
            "mcp__mmcg__mmcg_callers",
            "mcp__mmcg__mmcg_impact",
        ):
            self.assertIn(tool, allowed)
        self.assertNotIn("scratchpad_append", allowed)
        self.assertNotIn("Bash(cargo *)", allowed)
        self.assertNotIn("Bash(cargo test *)", allowed)
        self.assertFalse(runner.requires_prompt_sandbox("auditor"))

    def test_auditor_eval_uses_the_shipped_agent_runtime_contract(self):
        path = runner.SUITES["auditor"]["subagent"]
        name, definition = runner.subagent_runtime_definition(
            path, model_override="sonnet"
        )
        self.assertEqual(name, "mastermind-auditor")
        self.assertEqual(definition["model"], "sonnet")
        self.assertEqual(definition["mcpServers"], ["mmcg"])
        self.assertEqual(definition["maxTurns"], 20)
        self.assertEqual(definition["effort"], "high")
        self.assertIn("mcp__mmcg__mmcg_search", definition["tools"])
        self.assertNotIn("mcp__mmcg__*", definition["tools"])
        self.assertIn("# Mastermind auditor", definition["prompt"])

        args = runner.subagent_cli_args(path, model_override="sonnet")
        self.assertEqual(args[-2:], ["--agent", "mastermind-auditor"])
        payload = json.loads(args[args.index("--agents") + 1])
        self.assertEqual(payload["mastermind-auditor"]["maxTurns"], 20)

    def test_researcher_eval_uses_the_shipped_agent_runtime_contract(self):
        path = runner.SUITES["researcher"]["subagent"]
        name, definition = runner.subagent_runtime_definition(
            path, model_override="haiku"
        )
        self.assertEqual(name, "mastermind-researcher")
        self.assertEqual(definition["model"], "haiku")
        self.assertEqual(definition["maxTurns"], 12)
        self.assertEqual(definition["effort"], "low")
        self.assertIn("mcp__mmcg__mmcg_search", definition["tools"])
        self.assertIn("Contradictions / Unknowns", definition["prompt"])

        args = runner.subagent_cli_args(path, model_override="haiku")
        payload = json.loads(args[args.index("--agents") + 1])
        self.assertEqual(payload[name]["prompt"], definition["prompt"])

    def test_lean_runtime_prompts_have_no_examples_or_companion_sections(self):
        ceilings = {
            "researcher": 2_000,
            "critic": 2_700,
        }
        for suite, ceiling in ceilings.items():
            _, definition = runner.subagent_runtime_definition(
                runner.SUITES[suite]["subagent"]
            )
            prompt = definition["prompt"]
            self.assertLessEqual(len(prompt), ceiling, suite)
            self.assertNotIn("## Examples", prompt, suite)
            self.assertNotIn("## Companion pieces", prompt, suite)

    def test_every_scoped_shipped_agent_grants_exact_mmcg_tools(self):
        for path in (runner.REPO_ROOT / "agents/subagents").glob("*.md"):
            _, definition = runner.subagent_runtime_definition(path)
            if "mmcg" not in definition.get("mcpServers", []):
                continue
            tools = definition.get("tools", [])
            self.assertTrue(
                any(tool.startswith("mcp__mmcg__mmcg_") for tool in tools),
                path.name,
            )
            self.assertNotIn("mcp__mmcg__*", tools, path.name)

    def test_auditor_runs_inside_its_disposable_fixture(self):
        fixture = runner.REPO_ROOT / "evals/fixtures/fake-session"
        self.assertEqual(
            runner.evaluation_cwd("auditor", fixture_path=fixture, prompt_sandbox=None),
            fixture,
        )
        self.assertEqual(
            runner.evaluation_cwd(
                "researcher", fixture_path=fixture, prompt_sandbox=None
            ),
            fixture,
        )

    def test_critic_prompt_states_the_no_tools_evaluation_boundary(self):
        rendered = runner.render_critic_input(
            {"problem": "p", "design": "d", "alternatives": [], "constraints": "c"}
        )
        self.assertIn("no repository checkout or tools are available", rendered)
        self.assertIn("do not invoke tools", rendered)

    def test_researcher_prompt_is_bounded_to_quoted_evidence(self):
        rendered = runner.render_researcher_input(
            {"question": "q", "scope": "s", "evidence": "e"}
        )
        self.assertIn("no repository checkout or tools are available", rendered)
        self.assertIn("Use only the quoted evidence", rendered)
        self.assertIn("**Research question:** q", rendered)

    def test_clean_auditor_fixture_has_an_executable_cargo_test_contract(self):
        fixture = runner.FIXTURES_DIR / "fake-session"
        baseline = fixture / "baseline"
        clean = fixture / "changes/clean-add"
        for relative in ("Cargo.toml", "Cargo.lock", "src/lib.rs"):
            self.assertEqual(
                (baseline / relative).read_text(),
                (clean / relative).read_text(),
                f"{relative} must stay unchanged so the eval remains a one-file diff",
            )
        cases = [
            json.loads(line)
            for line in (runner.EVALS_DIR / "auditor.jsonl").read_text().splitlines()
            if line.strip()
        ]
        case = next(case for case in cases if case["id"] == "a-003-clean-execution-held")
        verify = next(
            line for line in case["input"]["executor_report"].splitlines()
            if line.startswith("VERIFY: ")
        )
        command = verify.removeprefix("VERIFY: ").removesuffix(" — PASSED")
        self.assertTrue(command.startswith("cargo test --locked "))
        allowed = runner.isolated_cli_args("auditor")
        allowed = allowed[allowed.index("--allowedTools") + 1]
        self.assertIn("Bash(cargo test --locked *)", allowed)

        with tempfile.TemporaryDirectory() as target:
            result = subprocess.run(
                shlex.split(command),
                cwd=clean,
                env={**os.environ, "CARGO_TARGET_DIR": target},
                text=True,
                capture_output=True,
                check=False,
            )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_workflow_allowlist_matches_shipped_skills(self):
        shipped = {
            path.relative_to(runner.REPO_ROOT).as_posix()
            for path in (runner.REPO_ROOT / "skills").rglob("SKILL.md")
        }
        self.assertEqual(runner.WORKFLOW_ARTIFACTS, shipped)
        for artifact in shipped:
            self.assertEqual(
                runner.workflow_prompt_path({"artifact": artifact}),
                (runner.REPO_ROOT / artifact).resolve(),
            )

    def test_workflow_prompt_rejects_unallowlisted_paths(self):
        with self.assertRaises(ValueError):
            runner.workflow_prompt_path({"artifact": "../../etc/passwd"})
        with self.assertRaises(ValueError):
            runner.workflow_prompt_path({"artifact": "README.md"})

    def test_auditor_input_never_accepts_a_synthetic_diff(self):
        rendered = runner.render_auditor_input(
            {
                "spec_summary": "one file only",
                "executor_report": "complete",
                "git_diff": "ANSWER LEAK",
            },
            fixture_path=runner.REPO_ROOT,
            baseline_ref="baseline",
            after_ref="after",
            has_mmcg=False,
        )
        self.assertNotIn("ANSWER LEAK", rendered)
        self.assertIn("git diff baseline..after", rendered)

    def test_frontmatter_is_removed_without_dropping_prompt_body(self):
        text = "---\nname: demo\n---\n\n# Contract\nBody\n"
        self.assertEqual(runner.strip_frontmatter(text), "# Contract\nBody\n")


if __name__ == "__main__":
    unittest.main()
