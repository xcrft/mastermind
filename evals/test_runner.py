import json
import io
import os
import re
import shlex
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from copy import deepcopy
from pathlib import Path
from unittest.mock import patch

from evals import ablation, runner
from evals.benchmark_process import ProcessResult


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
    for field in ("duration_ms", "duration_api_ms", "turns", "cost_usd"):
        summary[field] = runner.metric_summary([case[field] for case in cases])
    for field in summary["usage"]:
        summary["usage"][field] = runner.metric_summary(
            [case["usage"][field] for case in cases]
        )
    summary["telemetry"] = {
        "complete": all(case["telemetry"]["complete"] for case in cases),
        "incomplete_cases": [
            case["id"] for case in cases if not case["telemetry"]["complete"]
        ],
    }


def bind_target_identity(report, digest="9" * 64):
    report["claude_cli_sha256"] = "8" * 64
    report["claude_cli_stable"] = True
    report["evaluation_harness"] = {
        "sha256": "7" * 64,
        "stable": True,
        "python_implementation": "CPython",
        "python_version": "3.13.0",
        "platform": "test",
        "pyyaml_version": "test",
    }
    for case in report["cases"]:
        suite_name = case["suite"]
        case["runtime_controls"] = runner.evaluation_runtime_controls(
            suite_name,
            runner.SUITES[suite_name]["subagent"],
            {},
            include_mmcg=runner.SUITES[suite_name]["uses_fixture"],
        )
    for summary in report["suites"].values():
        summary["target_definition_digest"] = digest
        summary["target_definition_stable"] = True


def bind_fixture_runtime(report, git_digest="5" * 64, mmcg_digest="4" * 64):
    report["fixture_runtime"] = {
        "git": {
            "sha256": git_digest,
            "git_mode": "100755",
            "version": "git version test",
        },
        "mmcg": {"sha256": mmcg_digest, "git_mode": "100755"},
        "stable": True,
    }


def valid_critic_report(case_id="c-1"):
    report = runner.build_report(
        [
            runner.Result(
                case_id,
                "critic",
                True,
                input_tokens=10,
                telemetry_complete=True,
                resolved_models=[RESOLVED_MODEL],
            )
        ],
        model="opus",
        suite_filter="critic",
        case_filter=None,
    )
    report["claude_cli_version"] = "test-cli"
    report["suites"]["critic"]["case_definition_digest"] = "a" * 64
    return report


def valid_critic_case_definition():
    return {
        "id": "case",
        "why": "Exercise the runner lifecycle with a valid critic case.",
        "input": {
            "problem": "Choose a safe change.",
            "design": "Keep the current contract.",
            "alternatives": "None.",
            "constraints": "Preserve behavior.",
            "mmcg_snapshot": "No repository evidence is required.",
        },
        "expect": {"verdict": "ship it"},
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
        execution = runner.ToolExecution(
            "Bash",
            "tool-1",
            {"command": "cargo test --locked exact_test"},
            result_seen=True,
            succeeded=True,
        )
        self.assertTrue(
            runner.audit_verification_passed(
                output, "cargo test --locked exact_test", [execution]
            )
        )
        self.assertFalse(
            runner.audit_verification_passed(
                output, "cargo test --locked other_test", [execution]
            )
        )
        self.assertFalse(
            runner.audit_verification_passed(
                output, "cargo test --locked exact_test", []
            )
        )
        execution.succeeded = False
        self.assertFalse(
            runner.audit_verification_passed(
                output, "cargo test --locked exact_test", [execution]
            )
        )
        execution.succeeded = True
        failed_later = runner.ToolExecution(
            "Bash",
            "tool-2",
            {"command": "cargo test --locked exact_test"},
            result_seen=True,
            succeeded=False,
        )
        self.assertFalse(
            runner.audit_verification_passed(
                output,
                "cargo test --locked exact_test",
                [execution, failed_later],
            )
        )
        duplicate = output.replace(
            "    result: pass\n",
            "    result: pass\n"
            "  - cmd: \"cargo test --locked exact_test\"\n"
            "    result: pass\n",
        )
        self.assertFalse(
            runner.audit_verification_passed(
                duplicate, "cargo test --locked exact_test", [execution]
            )
        )
        self.assertFalse(
            runner.audit_verification_passed(
                output,
                "cargo test --locked exact_test && echo unsafe",
                [execution],
            )
        )

    def test_verification_rerun_is_one_canonical_focused_cargo_test(self):
        command = "cargo test --locked module::exact_test"
        self.assertEqual(runner.verification_rerun_command(command), command)
        for invalid in (
            "cargo test exact_test",
            "cargo test --locked --all",
            "cargo  test --locked exact_test",
            "cargo test --locked exact/test",
            "cargo test --locked 'exact test'",
            "cargo test --locked exact_test && echo unsafe",
            "cargo test --locked exact_test\nwhoami",
        ):
            with self.subTest(command=invalid), self.assertRaisesRegex(
                ValueError, "must be exactly"
            ):
                runner.verification_rerun_command(invalid)

        case = {
            "input": {
                "executor_report": (
                    "VERIFY: cargo test --locked module::exact_test — PASSED\n"
                    "VERIFY: cargo test --locked other_test && echo unsafe — PASSED\n"
                    "VERIFY: python -m pytest — PASSED"
                )
            }
        }
        self.assertEqual(
            runner.reported_cargo_verification_commands(case),
            ("cargo test --locked module::exact_test",),
        )

        auditor = deepcopy(
            runner.load_case_records(
                runner.SUITES["auditor"]["cases"], suite_name="auditor"
            )[2]
        )
        auditor["expect"]["verification_rerun"] = (
            "cargo test --locked different_test"
        )
        with self.assertRaisesRegex(ValueError, "must match a safe VERIFY"):
            runner.validate_case_record("auditor", auditor)

    def test_auditor_case_requires_the_attested_bash_execution(self):
        case = runner.load_case_records(
            runner.SUITES["auditor"]["cases"], suite_name="auditor"
        )[2]
        case = {**case, "allow_no_mmcg": True}
        expected_command = case["expect"]["verification_rerun"]
        output = f"""\
session_count matches the requested implementation.
<!-- mastermind:audit-begin -->
```yaml
verdict: held
verifications_rerun:
  - cmd: "{expected_command}"
    result: pass
```
<!-- mastermind:audit-end -->
"""

        def process(command, cwd):
            events = [
                {
                    "type": "system",
                    "subtype": "init",
                    "model": RESOLVED_MODEL,
                    "claude_code_version": "2.1.236",
                    "cwd": str(cwd),
                    "permissionMode": "dontAsk",
                    "tools": [
                        "Read",
                        "Grep",
                        "Glob",
                        "Bash",
                        "EndConversation",
                    ],
                    "mcp_servers": [],
                    "skills": [],
                    "plugins": [],
                },
                {
                    "type": "assistant",
                    "message": {
                        "model": RESOLVED_MODEL,
                        "content": [
                            {
                                "type": "tool_use",
                                "id": "tool-1",
                                "name": "Bash",
                                "input": {"command": command},
                            }
                        ]
                    },
                },
                {
                    "type": "user",
                    "message": {
                        "content": [
                            {"type": "tool_result", "tool_use_id": "tool-1"}
                        ]
                    },
                },
                {
                    "type": "result",
                    "subtype": "success",
                    "is_error": False,
                    "result": output,
                    "duration_ms": 1,
                    "duration_api_ms": 1,
                    "num_turns": 1,
                    "total_cost_usd": 0,
                    "usage": {
                        "input_tokens": 1,
                        "output_tokens": 1,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0,
                    },
                    "modelUsage": {RESOLVED_MODEL: {}},
                },
            ]
            return ProcessResult(
                stdout="\n".join(json.dumps(event) for event in events).encode(),
                stderr=b"",
                returncode=0,
            )

        with tempfile.TemporaryDirectory() as target:
            fixture = Path(target)
            for command, passed in (
                (expected_command, True),
                ("cargo test --locked another_test", False),
            ):
                invocations = []

                def invoke(cli_command, **kwargs):
                    invocations.append((cli_command, kwargs))
                    return process(command, fixture)

                with (
                    self.subTest(command=command),
                    patch.object(runner, "setup_fixture", return_value=fixture),
                    patch.object(runner, "teardown_fixture"),
                    patch.object(
                        runner,
                        "run_bounded",
                        side_effect=invoke,
                    ),
                ):
                    result = runner.evaluate_case(
                        "opus",
                        "auditor",
                        runner.SUITES["auditor"],
                        case,
                        keep_fixtures=False,
                        mmcg_binary=None,
                        claude_version="2.1.236 (Claude Code)",
                        git_binary=Path("/runtime/git/bin/git"),
                        cargo_binary=Path("/runtime/cargo/bin/cargo"),
                    )
                    self.assertEqual(result.passed, passed, result.reasons)
                self.assertEqual(len(invocations), 1)
                cli_command, invocation = invocations[0]
                self.assertEqual(
                    cli_command[cli_command.index("--max-turns") + 1], "20"
                )
                self.assertEqual(
                    cli_command[cli_command.index("--effort") + 1], "high"
                )
                self.assertEqual(
                    invocation["env"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"],
                    "8192",
                )
                self.assertEqual(
                    invocation["env"]["PATH"].split(os.pathsep)[:2],
                    ["/runtime/git/bin", "/runtime/cargo/bin"],
                )
                self.assertIsInstance(invocation["stdin"], bytes)
                self.assertEqual(
                    invocation["timeout"], runner.CLAUDE_CASE_TIMEOUT_SECONDS
                )
                self.assertEqual(
                    invocation["stdout_limit"], runner.CLAUDE_STDOUT_LIMIT_BYTES
                )
                self.assertEqual(
                    invocation["stderr_limit"], runner.CLAUDE_STDERR_LIMIT_BYTES
                )
                self.assertTrue(invocation["start_new_session"])

    def test_auditor_verification_requires_a_pinned_cargo_runtime(self):
        case = runner.load_case_records(
            runner.SUITES["auditor"]["cases"], suite_name="auditor"
        )[2]
        case = {**case, "allow_no_mmcg": True}
        fixture = Path("/unused-disposable-fixture")
        with (
            patch.object(runner, "setup_fixture", return_value=fixture),
            patch.object(runner, "teardown_fixture"),
            patch.object(runner, "run_bounded") as invoke,
        ):
            result = runner.evaluate_case(
                "opus",
                "auditor",
                runner.SUITES["auditor"],
                case,
                keep_fixtures=False,
                mmcg_binary=None,
                cargo_binary=None,
            )
        self.assertFalse(result.passed)
        self.assertEqual(
            result.reasons,
            ["pinned Cargo runtime unavailable for required verification"],
        )
        invoke.assert_not_called()

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

    def test_cli_usage_telemetry_retains_every_observed_model(self):
        payload = {
            "duration_ms": 1,
            "duration_api_ms": 1,
            "num_turns": 1,
            "total_cost_usd": 0,
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
            },
            "modelUsage": {RESOLVED_MODEL: {}, "fallback-model": {}},
            "_resolved_model": RESOLVED_MODEL,
        }
        telemetry = runner.telemetry_from_payload(payload)
        self.assertTrue(telemetry["complete"])
        self.assertEqual(
            telemetry["resolved_models"], [RESOLVED_MODEL, "fallback-model"]
        )

        payload["modelUsage"] = {"fallback-model": {}}
        telemetry = runner.telemetry_from_payload(payload)
        self.assertFalse(telemetry["complete"])
        self.assertIn(
            "stream primary model is absent from modelUsage", telemetry["issues"]
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

    def test_runtime_limits_enforce_case_caps_and_suite_defaults(self):
        self.assertEqual(set(runner.SUITE_RUNTIME_LIMITS), set(runner.SUITES))
        self.assertEqual(
            runner.case_runtime_limits(
                "researcher", {"max_turns": 3, "max_output_tokens": 600}
            ),
            {"max_turns": 3, "max_output_tokens": 600},
        )
        self.assertEqual(
            runner.case_runtime_limits("auditor", {}),
            {"max_turns": 20, "max_output_tokens": 8192},
        )
        self.assertEqual(
            runner.case_runtime_limits("intake", {}),
            {"max_turns": 4, "max_output_tokens": 4096},
        )
        with self.assertRaisesRegex(ValueError, "positive integers"):
            runner.case_runtime_limits("critic", {"max_turns": 0})

        self.assertEqual(
            runner.evaluation_effort(
                "critic", runner.SUITES["critic"]["subagent"]
            ),
            "high",
        )
        self.assertEqual(runner.evaluation_effort("workflow", None), "medium")
        for suite_name, suite in runner.SUITES.items():
            prompt_path = None if suite_name == "workflow" else suite["subagent"]
            with self.subTest(suite=suite_name):
                self.assertIn(
                    runner.evaluation_effort(suite_name, prompt_path),
                    runner.CLAUDE_EFFORT_LEVELS,
                )

    def test_evaluation_environment_removes_external_runtime_overrides(self):
        environment = runner.evaluation_environment(
            1400,
            source={
                "PATH": "/usr/bin",
                "HOME": "/tmp/home",
                "ANTHROPIC_API_KEY": "paid-key",
                "ANTHROPIC_MODEL": "external-model",
                "CLAUDE_CODE_EFFORT_LEVEL": "max",
                "CLAUDE_CODE_OAUTH_TOKEN": "subscription-token",
                "DISABLE_COMPACT": "1",
                "MAX_THINKING_TOKENS": "0",
                "MCP_TOOL_TIMEOUT": "999999999",
                "DEBUG": "1",
                "GIT_DIR": "/private/other-repository",
                "GIT_EXTERNAL_DIFF": "/private/untrusted-diff",
                "GIT_CONFIG_GLOBAL": "/private/untrusted-gitconfig",
            },
            pinned_executables=(Path("/runtime/git/bin/git"),),
        )
        self.assertEqual(
            environment["PATH"],
            f"/runtime/git/bin{os.pathsep}/usr/bin",
        )
        self.assertEqual(environment["HOME"], "/tmp/home")
        self.assertEqual(
            environment["CLAUDE_CODE_OAUTH_TOKEN"], "subscription-token"
        )
        for name in (
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_MODEL",
            "CLAUDE_CODE_EFFORT_LEVEL",
            "DISABLE_COMPACT",
            "MAX_THINKING_TOKENS",
            "DEBUG",
            "GIT_DIR",
            "GIT_EXTERNAL_DIFF",
        ):
            self.assertNotIn(name, environment)
        self.assertEqual(environment["CLAUDE_CODE_MAX_OUTPUT_TOKENS"], "1400")
        self.assertEqual(environment["CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY"], "1")
        self.assertEqual(environment["CLAUDE_CODE_DISABLE_AUTO_MEMORY"], "1")
        self.assertEqual(environment["MCP_TOOL_TIMEOUT"], "120000")
        self.assertEqual(environment["ENABLE_TOOL_SEARCH"], "false")
        self.assertEqual(environment["GIT_CONFIG_GLOBAL"], os.devnull)
        self.assertEqual(environment["GIT_CONFIG_NOSYSTEM"], "1")
        self.assertEqual(environment["GIT_CONFIG_VALUE_1"], os.devnull)
        self.assertEqual(environment["GIT_AUTHOR_DATE"], "2000-01-01T00:00:00Z")

        git_environment = runner._git_environment(
            {"PATH": "/usr/bin", "GIT_DIR": "/private/other-repository"}
        )
        self.assertEqual(git_environment["PATH"], "/usr/bin")
        self.assertNotIn("GIT_DIR", git_environment)
        self.assertEqual(git_environment["GIT_CONFIG_GLOBAL"], os.devnull)

        with self.assertRaisesRegex(ValueError, "must be absolute"):
            runner.evaluation_environment(
                100,
                source={"PATH": "/usr/bin"},
                pinned_executables=(Path("relative/git"),),
            )
        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            git_directory = root / "git-bin"
            cargo_directory = root / "cargo-bin"
            git_directory.mkdir()
            cargo_directory.mkdir()
            (git_directory / "cargo").write_text("shadow")
            (cargo_directory / "cargo").write_text("pinned")
            with self.assertRaisesRegex(ValueError, "shadowed"):
                runner.evaluation_environment(
                    100,
                    source={"PATH": "/usr/bin"},
                    pinned_executables=(
                        git_directory / "git",
                        cargo_directory / "cargo",
                    ),
                )

    def test_effective_runtime_controls_expose_case_policy(self):
        controls = runner.evaluation_runtime_controls(
            "auditor",
            runner.SUITES["auditor"]["subagent"],
            {"max_turns": 7, "max_output_tokens": 1300},
            include_mmcg=False,
        )
        self.assertEqual(controls["effort"], "high")
        self.assertEqual(controls["max_turns"], 7)
        self.assertEqual(controls["max_output_tokens"], 1300)
        self.assertEqual(
            controls["transport"],
            {
                "timeout_seconds": runner.CLAUDE_CASE_TIMEOUT_SECONDS,
                "stdout_limit_bytes": runner.CLAUDE_STDOUT_LIMIT_BYTES,
                "stderr_limit_bytes": runner.CLAUDE_STDERR_LIMIT_BYTES,
            },
        )
        self.assertFalse(controls["isolation"]["safe_mode"])
        self.assertFalse(controls["isolation"]["auto_memory"])
        self.assertEqual(
            controls["tool_policy"]["builtins"],
            ["Read", "Glob", "Grep", "Bash"],
        )
        self.assertEqual(controls["tool_policy"]["mcp_servers"], [])
        self.assertEqual(
            controls["environment"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"],
            "1300",
        )
        self.assertTrue(runner._valid_evaluation_runtime_controls(controls))
        unsafe = deepcopy(controls)
        unsafe["tool_policy"]["allowed"].append(
            "Bash(cargo test --locked *)"
        )
        self.assertFalse(runner._valid_evaluation_runtime_controls(unsafe))

        prompt_only = runner.evaluation_runtime_controls(
            "workflow", None, {}, include_mmcg=False
        )
        self.assertTrue(prompt_only["isolation"]["safe_mode"])
        self.assertEqual(prompt_only["tool_policy"]["builtins"], [])
        self.assertEqual(prompt_only["tool_policy"]["expected_stream"], [])

    def test_harness_identity_includes_bounded_process_transport(self):
        definitions = {
            "runner.py": {"sha256": "1" * 64, "git_mode": "100755"},
            "benchmark_process.py": {
                "sha256": "2" * 64,
                "git_mode": "100644",
            },
            "evidence.py": {"sha256": "3" * 64, "git_mode": "100644"},
        }
        seen = []

        def definition(path):
            seen.append(Path(path).name)
            return definitions[Path(path).name]

        with patch.object(
            runner, "_stable_regular_file_definition", side_effect=definition
        ):
            before = runner.evaluation_harness_definition()["sha256"]
            first_sources = list(seen)
            definitions["benchmark_process.py"] = {
                "sha256": "4" * 64,
                "git_mode": "100644",
            }
            after = runner.evaluation_harness_definition()["sha256"]

        self.assertEqual(
            first_sources, ["runner.py", "benchmark_process.py", "evidence.py"]
        )
        self.assertNotEqual(before, after)

    def test_metadata_probes_use_bounded_process_transport(self):
        revision = "a" * 40
        completed = ProcessResult(
            stdout=(revision + "\n").encode(), stderr=b"", returncode=0
        )
        with patch.object(runner, "run_bounded", return_value=completed) as invoke:
            self.assertEqual(runner.git_revision(Path("/runtime/git")), revision)
            git_call = invoke.call_args
            self.assertEqual(git_call.args[0][0], "/runtime/git")
            self.assertEqual(
                git_call.kwargs["timeout"], runner.METADATA_PROCESS_TIMEOUT_SECONDS
            )
            self.assertEqual(
                git_call.kwargs["stdout_limit"],
                runner.METADATA_OUTPUT_LIMIT_BYTES,
            )

            invoke.return_value = ProcessResult(
                stdout=b"2.1.236 (Claude Code)\n", stderr=b"", returncode=0
            )
            self.assertEqual(
                runner.claude_cli_version(Path("/runtime/claude")),
                "2.1.236 (Claude Code)",
            )
            self.assertEqual(invoke.call_args.args[0][0], "/runtime/claude")

        invalid_revision = ProcessResult(
            stdout=b"HEAD\n", stderr=b"", returncode=0
        )
        with patch.object(runner, "run_bounded", return_value=invalid_revision):
            self.assertIsNone(runner.git_revision())

        stopped = ProcessResult(
            stdout=b"/private/secret", returncode=-9, stop_reason="output_limit"
        )
        with patch.object(runner, "run_bounded", return_value=stopped):
            self.assertIsNone(runner.git_revision())
            self.assertIsNone(runner.claude_cli_version())

    def test_fixture_tools_fail_safely_when_bounded_transport_stops(self):
        stopped = ProcessResult(
            stdout=b"/private/stdout-secret",
            stderr=b"/private/stderr-secret",
            returncode=-9,
            stop_reason="output_limit",
        )
        with tempfile.TemporaryDirectory() as target, patch.object(
            runner, "run_bounded", return_value=stopped
        ) as invoke:
            root = Path(target)
            with self.assertRaisesRegex(RuntimeError, "output_limit") as git_error:
                runner._run_git(["status"], root)
            self.assertNotIn("private", str(git_error.exception))

            with self.assertRaisesRegex(RuntimeError, "output_limit") as mmcg_error:
                runner._build_mmcg_index(root, mmcg_binary=Path("/runtime/mmcg"))
            self.assertNotIn("private", str(mmcg_error.exception))
            self.assertEqual(
                invoke.call_args.kwargs["timeout"], runner.MMCG_INDEX_TIMEOUT_SECONDS
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
                            "id": "tool-1",
                            "name": "mcp__mmcg__mmcg_search",
                            "input": {"query": "secret input is not persisted"},
                        }
                    ]
                },
            },
            {
                "type": "user",
                "message": {
                    "content": [
                        {"type": "tool_result", "tool_use_id": "tool-1"}
                    ]
                },
            },
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "tool-2",
                            "name": "Read",
                            "input": {},
                        }
                    ]
                },
            },
            {
                "type": "user",
                "message": {
                    "content": [
                        {"type": "tool_result", "tool_use_id": "tool-2"}
                    ]
                },
            },
            {
                "type": "result",
                "subtype": "success",
                "is_error": False,
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
        payload, tool_calls, tool_executions = runner.parse_claude_output(
            "\n".join(json.dumps(event) for event in events), streamed=True
        )

        self.assertEqual(payload["result"], "done")
        self.assertEqual(payload["_resolved_model"], RESOLVED_MODEL)
        self.assertEqual(
            tool_calls, ["mcp__mmcg__mmcg_search", "Read"]
        )
        self.assertEqual(
            [execution.tool_use_id for execution in tool_executions],
            ["tool-1", "tool-2"],
        )
        self.assertTrue(all(execution.succeeded for execution in tool_executions))

    def test_stream_parser_rejects_unverified_tool_protocol(self):
        base = [
            {"type": "system", "subtype": "init", "model": RESOLVED_MODEL},
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "tool-1",
                            "name": "Bash",
                            "input": {"command": "true"},
                        }
                    ]
                },
            },
            {
                "type": "result",
                "subtype": "success",
                "is_error": False,
                "result": "done",
                "modelUsage": {RESOLVED_MODEL: {}},
            },
        ]
        with self.assertRaisesRegex(ValueError, "without a result"):
            runner.parse_claude_output(
                "\n".join(json.dumps(event) for event in base), streamed=True
            )

        unmatched = deepcopy(base)
        unmatched.insert(
            2,
            {
                "type": "user",
                "message": {
                    "content": [
                        {"type": "tool_result", "tool_use_id": "other"}
                    ]
                },
            },
        )
        with self.assertRaisesRegex(ValueError, "unmatched tool result"):
            runner.parse_claude_output(
                "\n".join(json.dumps(event) for event in unmatched), streamed=True
            )

        before_init = deepcopy(base)
        before_init.insert(
            0, {"type": "assistant", "message": {"content": []}}
        )
        with self.assertRaisesRegex(ValueError, "before its init"):
            runner.parse_claude_output(
                "\n".join(json.dumps(event) for event in before_init), streamed=True
            )

        missing_error_state = deepcopy(base)
        del missing_error_state[-1]["is_error"]
        with self.assertRaisesRegex(ValueError, "invalid error state"):
            runner.parse_claude_output(
                "\n".join(json.dumps(event) for event in missing_error_state),
                streamed=True,
            )

        unsupported = deepcopy(base)
        unsupported.insert(1, {"type": "opaque"})
        with self.assertRaisesRegex(ValueError, "unsupported event type"):
            runner.parse_claude_output(
                "\n".join(json.dumps(event) for event in unsupported),
                streamed=True,
            )

    def test_stream_parser_validates_the_observed_cli_runtime(self):
        contract = runner.StreamRuntimeContract(
            cwd="/tmp/eval-case",
            tools=("Read", "mcp__mmcg__mmcg_search"),
            mcp_servers=("mmcg",),
            claude_code_version="2.1.236",
        )
        init = {
            "type": "system",
            "subtype": "init",
            "model": RESOLVED_MODEL,
            "claude_code_version": "2.1.236",
            "cwd": "/tmp/eval-case",
            "permissionMode": "dontAsk",
            "tools": ["mcp__mmcg__mmcg_search", "Read"],
            "mcp_servers": [{"name": "mmcg", "status": "connected"}],
            "skills": [],
            "plugins": [],
        }
        result = {
            "type": "result",
            "subtype": "success",
            "is_error": False,
            "result": "done",
            "modelUsage": {RESOLVED_MODEL: {}},
        }

        payload, tool_calls, executions = runner.parse_claude_output(
            "\n".join(json.dumps(event) for event in (init, result)),
            streamed=True,
            runtime_contract=contract,
        )
        self.assertEqual(payload["result"], "done")
        self.assertEqual(tool_calls, [])
        self.assertEqual(executions, [])

        mutations = (
            ("claude_code_version", "2.1.235", "CLI version"),
            ("cwd", "/tmp/other", "working directory"),
            ("permissionMode", "acceptEdits", "permission mode"),
            ("tools", ["Read", "Bash"], "tool inventory"),
            (
                "mcp_servers",
                [{"name": "mmcg", "status": "failed"}],
                "MCP server state",
            ),
            ("skills", ["unexpected"], "unexpected skills or plugins"),
        )
        for field, value, message in mutations:
            with self.subTest(field=field):
                changed = deepcopy(init)
                changed[field] = value
                with self.assertRaisesRegex(ValueError, message):
                    runner.parse_claude_output(
                        "\n".join(
                            json.dumps(event) for event in (changed, result)
                        ),
                        streamed=True,
                        runtime_contract=contract,
                    )

        extra_tool_events = [
            init,
            {
                "type": "assistant",
                "message": {
                    "model": RESOLVED_MODEL,
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "tool-1",
                            "name": "Bash",
                            "input": {"command": "true"},
                        }
                    ],
                },
            },
            {
                "type": "user",
                "message": {
                    "content": [
                        {"type": "tool_result", "tool_use_id": "tool-1"}
                    ]
                },
            },
            result,
        ]
        with self.assertRaisesRegex(ValueError, "outside its inventory"):
            runner.parse_claude_output(
                "\n".join(json.dumps(event) for event in extra_tool_events),
                streamed=True,
                runtime_contract=contract,
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

    def test_report_writer_replaces_an_existing_report(self):
        report = {"kind": runner.REPORT_KIND, "schema_version": 1}
        with tempfile.TemporaryDirectory() as target:
            path = Path(target) / "report.json"
            path.write_text("old report\n")
            runner.write_report(path, report)
            self.assertEqual(json.loads(path.read_text()), report)

    def test_report_writer_detects_final_path_replacement(self):
        if os.name != "posix" or not hasattr(os, "O_NOFOLLOW"):
            self.skipTest("verified publication uses POSIX directory descriptors")
        report = {"kind": runner.REPORT_KIND, "schema_version": 1}
        with tempfile.TemporaryDirectory() as target:
            path = Path(target) / "report.json"
            external = (
                json.dumps(report, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
            ).encode("utf-8")
            original_fsync = runner.os.fsync
            replaced = False

            def replace_after_publication(descriptor):
                nonlocal replaced
                result = original_fsync(descriptor)
                if (
                    not replaced
                    and runner.stat.S_ISDIR(os.fstat(descriptor).st_mode)
                    and path.exists()
                ):
                    path.unlink()
                    path.write_bytes(external)
                    replaced = True
                return result

            with (
                patch.object(runner.os, "fsync", side_effect=replace_after_publication),
                self.assertRaisesRegex(OSError, "changed during verification"),
            ):
                runner.write_report(path, report)
            self.assertEqual(path.read_bytes(), external)

    def test_report_writer_rejects_detached_parent(self):
        if os.name != "posix" or not hasattr(os, "O_NOFOLLOW"):
            self.skipTest("verified publication uses POSIX directory descriptors")
        report = {"kind": runner.REPORT_KIND, "schema_version": 1}
        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            parent = root / "reports"
            parent.mkdir()
            path = parent / "report.json"
            detached = root / "detached-reports"
            original_fsync = runner.os.fsync
            moved = False

            def detach_after_publication(descriptor):
                nonlocal moved
                result = original_fsync(descriptor)
                if (
                    not moved
                    and runner.stat.S_ISDIR(os.fstat(descriptor).st_mode)
                    and path.exists()
                ):
                    parent.rename(detached)
                    parent.mkdir()
                    moved = True
                return result

            with (
                patch.object(runner.os, "fsync", side_effect=detach_after_publication),
                self.assertRaisesRegex(OSError, "parent changed"),
            ):
                runner.write_report(path, report)
            self.assertFalse(path.exists())
            self.assertEqual(list(detached.iterdir()), [])

    def test_report_gate_rejects_inconsistent_citation_scores(self):
        result = runner.Result(
            "source-case", "researcher", True, telemetry_complete=True,
            resolved_models=[RESOLVED_MODEL],
            citation_checks={"expected": 1, "matched": 1, "total": 1, "valid": 1, "issues": []},
        )
        report = runner.build_report(
            [result], model="opus", suite_filter="researcher", case_filter=None
        )
        report["claude_cli_version"] = "test-cli"
        report["suites"]["researcher"]["case_definition_digest"] = "e" * 64
        self.assertEqual(runner.report_comparison_issues(report, "test"), [])
        for update in (
            {"matched": 0}, {"matched": 2}, {"valid": 2}, {"valid": False},
            {"expected": 0}, {"issues": ["missing citation"]},
        ):
            with self.subTest(update=update):
                malformed = deepcopy(report)
                malformed["cases"][0]["citation_checks"].update(update)
                self.assertTrue(runner.report_comparison_issues(malformed, "test"))
        legacy = deepcopy(report)
        del legacy["cases"][0]["citation_checks"]
        self.assertEqual(runner.report_comparison_issues(legacy, "test"), [])

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
            bind_target_identity(report)
        baseline["suites"]["critic"]["target_definition_digest"] = "1" * 64
        current["suites"]["critic"]["target_definition_digest"] = "2" * 64
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(gate["passed"])
        self.assertEqual(len(gate["checks"]), 9)

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
        current["cases"][0]["reasons"] = ["quality regression"]
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
                "b", "critic", False, reasons=["baseline failure"], input_tokens=100,
                telemetry_complete=True, resolved_models=[RESOLVED_MODEL],
            ),
        ]
        current_results = [
            runner.Result(
                "a", "critic", False, reasons=["current failure"], input_tokens=90,
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
            bind_target_identity(report)
        gate = runner.compare_to_baseline(current, baseline)
        self.assertFalse(gate["passed"])
        self.assertTrue(
            any("baseline-passing case regressed: a" in item for item in gate["failures"])
        )

    def test_fixture_gate_requires_and_compares_runtime_identity(self):
        baseline = runner.build_report(
            [
                runner.Result(
                    "research-case",
                    "researcher",
                    True,
                    input_tokens=100,
                    telemetry_complete=True,
                    resolved_models=[RESOLVED_MODEL],
                )
            ],
            model="opus",
            suite_filter="researcher",
            case_filter=None,
        )
        current = runner.build_report(
            [
                runner.Result(
                    "research-case",
                    "researcher",
                    True,
                    input_tokens=90,
                    telemetry_complete=True,
                    resolved_models=[RESOLVED_MODEL],
                )
            ],
            model="opus",
            suite_filter="researcher",
            case_filter=None,
        )
        for report in (baseline, current):
            report["claude_cli_version"] = "test-cli"
            report["suites"]["researcher"]["case_definition_digest"] = "a" * 64
            bind_target_identity(report)

        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(
            any("no fixture runtime identity" in item for item in gate["failures"])
        )

        bind_fixture_runtime(current)
        self.assertTrue(runner.compare_to_baseline(current, baseline)["passed"])

        bind_fixture_runtime(baseline)
        current["fixture_runtime"]["mmcg"]["sha256"] = "3" * 64
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(
            any("fixture runtime differs" in item for item in gate["failures"])
        )

    def test_verification_gate_requires_and_compares_cargo_identity(self):
        command = "cargo test --locked exact_test"
        result = runner.Result(
            "audit-case",
            "auditor",
            True,
            input_tokens=100,
            telemetry_complete=True,
            resolved_models=[RESOLVED_MODEL],
        )
        baseline = runner.build_report(
            [result], model="opus", suite_filter="auditor", case_filter=None
        )
        baseline["claude_cli_version"] = "test-cli"
        baseline["suites"]["auditor"]["case_definition_digest"] = "a" * 64
        bind_target_identity(baseline)
        bind_fixture_runtime(baseline)
        baseline["cases"][0]["runtime_controls"] = (
            runner.evaluation_runtime_controls(
                "auditor",
                runner.SUITES["auditor"]["subagent"],
                {"verification_rerun": command},
                include_mmcg=True,
            )
        )
        current = deepcopy(baseline)
        current["cases"][0]["usage"]["input_tokens"] = 90
        current["cases"][0]["usage"]["context_tokens"] = 90
        current["cases"][0]["usage"]["total_tokens"] = 90
        sync_gated_summaries(current, "auditor")

        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(
            any(
                "no verification runtime identity" in item
                for item in gate["failures"]
            )
        )

        for report in (baseline, current):
            report["verification_runtime"] = {
                "cargo": {"sha256": "3" * 64, "git_mode": "100755"},
                "stable": True,
            }
        self.assertTrue(runner.compare_to_baseline(current, baseline)["passed"])

        current["verification_runtime"]["cargo"]["sha256"] = "2" * 64
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(
            any(
                "verification runtime differs" in item
                for item in gate["failures"]
            )
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
        bind_target_identity(baseline)
        bind_target_identity(current)
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
        bind_target_identity(baseline)

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
        current["cases"][0]["runtime_controls"]["max_turns"] += 1
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(
            any(
                "effective runtime controls differ" in item
                for item in gate["failures"]
            )
        )

        current = deepcopy(baseline)
        current["claude_cli_sha256"] = "7" * 64
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(any("CLI binary mismatch" in item for item in gate["failures"]))

        current = deepcopy(baseline)
        del current["claude_cli_sha256"]
        del current["claude_cli_stable"]
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(
            any("no Claude CLI identity" in item for item in gate["failures"])
        )

        current = deepcopy(baseline)
        current["evaluation_harness"]["sha256"] = "6" * 64
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(
            any("evaluation harness differs" in item for item in gate["failures"])
        )

        current = deepcopy(baseline)
        del current["evaluation_harness"]
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(
            any("no evaluation harness identity" in item for item in gate["failures"])
        )

        current = deepcopy(baseline)
        current["filters"]["case"] = "c-1"
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(any("filters differ" in item for item in gate["failures"]))

        current = deepcopy(baseline)
        del current["suites"]["critic"]["target_definition_digest"]
        del current["suites"]["critic"]["target_definition_stable"]
        gate = runner.compare_to_baseline(current, baseline)
        self.assertTrue(any("no target identity" in item for item in gate["failures"]))

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
        bind_target_identity(baseline)
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

    def test_report_schema_rejects_inconsistent_records(self):
        report = valid_critic_report()
        malformed_reports = []

        malformed = deepcopy(report)
        malformed["unexpected"] = True
        malformed_reports.append((malformed, "unexpected fields"))

        malformed = deepcopy(report)
        malformed["generated_at"] = "not-a-timestamp"
        malformed_reports.append((malformed, "invalid generation time"))

        malformed = deepcopy(report)
        malformed["git_revision"] = "HEAD"
        malformed_reports.append((malformed, "invalid Git revision"))

        malformed = deepcopy(report)
        malformed["cases"][0]["suite"] = "unknown"
        malformed_reports.append((malformed, "unknown suite"))

        malformed = deepcopy(report)
        malformed["suites"]["critic"]["duration_ms"]["total"] = 1
        malformed_reports.append((malformed, "duration_ms summary"))

        malformed = deepcopy(report)
        malformed["cases"][0]["retry_used"] = True
        malformed_reports.append((malformed, "inconsistent retry state"))

        malformed = deepcopy(report)
        malformed["cases"][0]["telemetry"]["issues"] = ["missing usage"]
        malformed_reports.append((malformed, "inconsistent telemetry status"))

        malformed = deepcopy(report)
        malformed["filters"]["suite"] = "researcher"
        malformed_reports.append((malformed, "suite filter does not match"))

        malformed = deepcopy(report)
        malformed["filters"]["case"] = "another-case"
        malformed_reports.append((malformed, "case filter does not match"))

        malformed = deepcopy(report)
        malformed["filters"] = {"suite": None, "case": None}
        malformed_reports.append((malformed, "does not contain every suite"))

        for malformed, expected_issue in malformed_reports:
            with self.subTest(expected_issue=expected_issue):
                issues = runner.report_comparison_issues(malformed, "test")
                self.assertTrue(
                    any(expected_issue in issue for issue in issues), issues
                )

    def test_current_report_requires_definition_stability(self):
        baseline = valid_critic_report()
        current = deepcopy(baseline)
        for report in (baseline, current):
            bind_target_identity(report)
        del current["suites"]["critic"]["definition_stable"]

        gate = runner.compare_to_baseline(current, baseline)

        self.assertTrue(
            any("no definition stability" in issue for issue in gate["failures"])
        )

    def test_legacy_capture_is_limited_to_baseline_evidence(self):
        baseline_path = runner.EVALS_DIR / "baselines" / "critic-opus-pre-lean.json"
        legacy = runner.load_report(baseline_path)
        self.assertEqual(legacy["capture"]["mode"], "pre-report-console")

        current = valid_critic_report()
        current["capture"] = deepcopy(legacy["capture"])
        current["suites"]["critic"]["duration_api_ms"]["total"] = 1
        issues = runner.report_comparison_issues(current, "current")

        self.assertTrue(any("cannot use legacy capture" in issue for issue in issues))
        self.assertTrue(any("duration_api_ms summary" in issue for issue in issues))

    def test_shipped_case_definitions_match_the_fail_closed_schema(self):
        for suite_name, suite in runner.SUITES.items():
            with self.subTest(suite=suite_name):
                records = runner.load_case_records(
                    suite["cases"], suite_name=suite_name
                )
                self.assertTrue(records)
                self.assertRegex(
                    runner.case_definition_digest_from_records(
                        suite_name, records
                    ),
                    r"^[0-9a-f]{64}$",
                )

    def test_case_schema_rejects_typos_and_impossible_expectations(self):
        base = valid_critic_case_definition()
        malformed_cases = []

        malformed = deepcopy(base)
        malformed["extra"] = True
        malformed_cases.append((malformed, "invalid fields"))

        malformed = deepcopy(base)
        malformed["input"]["problemm"] = malformed["input"].pop("problem")
        malformed_cases.append((malformed, "input has invalid fields"))

        malformed = deepcopy(base)
        malformed["expect"]["contain"] = ["ship"]
        malformed_cases.append((malformed, "expect has unexpected fields"))

        malformed = deepcopy(base)
        malformed["expect"].update({"min_turns": 3, "max_turns": 2})
        malformed_cases.append((malformed, "min_turns exceeds"))

        malformed = deepcopy(base)
        malformed["expect"].update(
            {"contains": ["same"], "not_contains": ["same"]}
        )
        malformed_cases.append((malformed, "contradict"))

        malformed = deepcopy(base)
        malformed["expect"]["verdict"] = "looks good"
        malformed_cases.append((malformed, "unsupported values"))

        malformed = deepcopy(base)
        del malformed["expect"]["verdict"]
        malformed["expect"]["max_turns"] = 1
        malformed_cases.append((malformed, "must define expect.verdict"))

        for malformed, expected_error in malformed_cases:
            with self.subTest(expected_error=expected_error), self.assertRaisesRegex(
                ValueError, expected_error
            ):
                runner.validate_case_record("critic", malformed)

    def test_case_schema_rejects_unavailable_tools_and_invalid_citations(self):
        researcher = runner.load_case_records(
            runner.SUITES["researcher"]["cases"], suite_name="researcher"
        )[0]

        unavailable = deepcopy(researcher)
        unavailable["expect"]["tools"]["contains"].append("mmcg_serch")
        with self.assertRaisesRegex(ValueError, "unavailable tools"):
            runner.validate_case_record("researcher", unavailable)

        impossible_max = deepcopy(researcher)
        impossible_max["expect"]["tools"]["max"] = 0
        with self.assertRaisesRegex(ValueError, "lower than mandatory"):
            runner.validate_case_record("researcher", impossible_max)

        optional_index = deepcopy(researcher)
        optional_index["allow_no_mmcg"] = True
        with self.assertRaisesRegex(ValueError, "conflicts with a required mmcg"):
            runner.validate_case_record("researcher", optional_index)

        unsafe_citation = deepcopy(researcher)
        unsafe_citation["expect"]["citations"][0]["path"] = "../outside.rs"
        with self.assertRaisesRegex(ValueError, "canonical relative path"):
            runner.validate_case_record("researcher", unsafe_citation)

        missing_anchor = deepcopy(researcher)
        missing_anchor["expect"]["citations"][0]["anchor"] = "not in source"
        with self.assertRaisesRegex(ValueError, "exactly one source line"):
            runner.case_definition_digest_from_records(
                "researcher", [missing_anchor]
            )

        duplicate_line = deepcopy(researcher)
        duplicate_line["expect"]["citations"].append(
            {
                "path": "src/session.rs",
                "anchor": "session_count(&self)",
            }
        )
        with self.assertRaisesRegex(ValueError, "same source line"):
            runner.case_definition_digest_from_records(
                "researcher", [duplicate_line]
            )

        no_output_oracle = deepcopy(researcher)
        for field_name in ("contains", "contains_any", "citations"):
            no_output_oracle["expect"].pop(field_name, None)
        with self.assertRaisesRegex(ValueError, "no positive output oracle"):
            runner.validate_case_record("researcher", no_output_oracle)

        intake = runner.load_case_records(
            runner.SUITES["intake"]["cases"], suite_name="intake"
        )[0]
        intake["expect"]["action"] = ["refined"]
        with self.assertRaisesRegex(ValueError, "expect.action"):
            runner.validate_case_record("intake", intake)

    def test_case_loader_rejects_unknown_expectation_fields_before_execution(self):
        malformed = valid_critic_case_definition()
        malformed["expect"]["contain"] = ["ship"]
        with tempfile.TemporaryDirectory() as target:
            path = Path(target) / "critic.jsonl"
            path.write_text(json.dumps(malformed) + "\n")
            with self.assertRaisesRegex(
                ValueError, "critic.jsonl:1:.*unexpected fields"
            ):
                runner.load_case_records(path, suite_name="critic")

    def test_fixture_content_changes_case_definition_digest(self):
        with tempfile.TemporaryDirectory() as target:
            fixture_root = Path(target) / "fake-session"
            baseline = fixture_root / "baseline"
            after = fixture_root / "changes" / "clean-add"
            baseline.mkdir(parents=True)
            after.mkdir(parents=True)
            baseline_source = baseline / "src" / "session.rs"
            after_source = after / "src" / "session.rs"
            baseline_source.parent.mkdir()
            after_source.parent.mkdir()
            baseline_source.write_text("fn before() {}\n")
            after_source.write_text(
                "pub fn session_count(&self) -> usize {\n"
                "    self.sessions.read().unwrap().len()\n"
                "}\n"
            )
            with patch.object(runner, "FIXTURES_DIR", Path(target)):
                before = runner.case_definition_digest(
                    "researcher", ["r-001-structural-source-cross-check"]
                )
                after_source.write_text(after_source.read_text() + "// changed\n")
                changed = runner.case_definition_digest(
                    "researcher", ["r-001-structural-source-cross-check"]
                )
        self.assertIsNotNone(before)
        self.assertNotEqual(before, changed)

    def test_fixture_mode_changes_case_definition_digest(self):
        with tempfile.TemporaryDirectory() as target:
            fixture_root = Path(target) / "fake-session"
            baseline = fixture_root / "baseline"
            after = fixture_root / "changes" / "clean-add"
            baseline.mkdir(parents=True)
            after.mkdir(parents=True)
            baseline_source = baseline / "src" / "session.rs"
            changed_file = after / "src" / "session.rs"
            baseline_source.parent.mkdir()
            changed_file.parent.mkdir()
            baseline_source.write_text("fn before() {}\n")
            changed_file.write_text(
                "pub fn session_count(&self) -> usize {\n"
                "    self.sessions.read().unwrap().len()\n"
                "}\n"
            )
            with patch.object(runner, "FIXTURES_DIR", Path(target)):
                before = runner.case_definition_digest(
                    "researcher", ["r-001-structural-source-cross-check"]
                )
                os.chmod(changed_file, changed_file.stat().st_mode ^ 0o100)
                changed = runner.case_definition_digest(
                    "researcher", ["r-001-structural-source-cross-check"]
                )
        self.assertIsNotNone(before)
        self.assertNotEqual(before, changed)

    def test_frozen_fixture_snapshot_isolated_from_later_source_changes(self):
        case = {
            "id": "case",
            "why": "Verify that fixture snapshots remain immutable.",
            "fixture": "sample",
            "baseline_ref": "baseline",
            "after_ref": "current",
            "input": {"question": "What changed?", "scope": "source.py"},
            "expect": {"contains": ["after"]},
        }
        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            source = root / "source"
            baseline = source / "sample" / "baseline"
            after = source / "sample" / "changes" / "current"
            baseline.mkdir(parents=True)
            after.mkdir(parents=True)
            (baseline / "source.py").write_text("before = True\n")
            changed_file = after / "source.py"
            changed_file.write_text("after = True\n")
            frozen = root / "frozen"
            frozen.mkdir()
            with patch.object(runner, "FIXTURES_DIR", source):
                source_digest = runner.case_definition_digest_from_records(
                    "researcher", [case]
                )
                runner.snapshot_fixture_definitions([case], frozen)
                frozen_digest = runner.case_definition_digest_from_records(
                    "researcher", [case], fixtures_dir=frozen
                )
                changed_file.write_text("after = False\n")
                changed_source_digest = runner.case_definition_digest_from_records(
                    "researcher", [case]
                )
                unchanged_frozen_digest = runner.case_definition_digest_from_records(
                    "researcher", [case], fixtures_dir=frozen
                )
        self.assertEqual(source_digest, frozen_digest)
        self.assertNotEqual(source_digest, changed_source_digest)
        self.assertEqual(frozen_digest, unchanged_frozen_digest)

    def test_fixture_definition_rejects_parent_traversal(self):
        with self.assertRaisesRegex(ValueError, "canonical relative path"):
            runner.fixture_case_roots(
                {
                    "fixture": "fake-session",
                    "baseline_ref": "baseline",
                    "after_ref": "../outside",
                }
            )

    def test_fixture_definition_rejects_git_metadata_and_symlinks(self):
        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            metadata = root / ".GIT"
            metadata.mkdir()
            (metadata / "config").write_text("unsafe = true\n")
            with self.assertRaisesRegex(ValueError, "Git metadata"):
                runner.fixture_tree_definition(root)

        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            source = root / "source"
            source.mkdir()
            outside = root / "outside.txt"
            outside.write_text("outside\n")
            try:
                (source / "linked.txt").symlink_to(outside)
            except OSError as error:
                self.skipTest(f"symbolic links unavailable: {error}")
            destination = root / "destination"
            with self.assertRaisesRegex(ValueError, "symbolic link"):
                runner._copy_tree_into(source, destination)
            self.assertFalse(destination.exists())

        with tempfile.TemporaryDirectory() as target:
            fixtures = Path(target)
            real_baseline = fixtures / "real-baseline"
            real_baseline.mkdir()
            sample = fixtures / "sample"
            (sample / "changes" / "after").mkdir(parents=True)
            try:
                (sample / "baseline").symlink_to(
                    real_baseline, target_is_directory=True
                )
            except OSError as error:
                self.skipTest(f"symbolic links unavailable: {error}")
            with self.assertRaisesRegex(ValueError, "symbolic link"):
                runner.fixture_case_roots(
                    {
                        "fixture": "sample",
                        "baseline_ref": "baseline",
                        "after_ref": "after",
                    },
                    fixtures_dir=fixtures,
                )

    def test_fixture_refs_and_staged_paths_are_canonical(self):
        for baseline_ref, after_ref in (
            ("--force", "after"),
            ("baseline", "after ref"),
            ("same", "same"),
            ("topic", "topic/after"),
        ):
            with self.subTest(
                baseline_ref=baseline_ref, after_ref=after_ref
            ), self.assertRaisesRegex(ValueError, "fixture tag"):
                runner._validate_fixture_refs(baseline_ref, after_ref)

        with tempfile.TemporaryDirectory() as target:
            fixtures = Path(target)
            baseline = fixtures / "sample" / "baseline"
            after = fixtures / "sample" / "changes" / "after"
            baseline.mkdir(parents=True)
            after.mkdir(parents=True)
            (baseline / "source.py").write_text("before = True\n")
            (after / "source.py").write_text("after = True\n")
            for staged_paths in (
                ["../outside"],
                [".git/config"],
                ["src\\file.py"],
                ["src/file.py", "src/file.py"],
            ):
                with self.subTest(
                    staged_paths=staged_paths
                ), self.assertRaisesRegex(ValueError, "staged_paths"):
                    runner.setup_fixture(
                        "sample",
                        "baseline",
                        "after",
                        staged_paths=staged_paths,
                        fixtures_dir=fixtures,
                    )

    def test_main_fails_report_when_case_definition_changes_during_run(self):
        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            cases = root / "critic.jsonl"
            case_definition = valid_critic_case_definition()
            cases.write_text(json.dumps(case_definition) + "\n")
            subagent = root / "agent.md"
            subagent.write_text("---\nname: test\ndescription: test\n---\nPrompt.\n")
            report_path = root / "report.json"
            suites = {
                "critic": {
                    "subagent": subagent,
                    "cases": cases,
                    "renderer": "render_critic_input",
                    "uses_fixture": False,
                }
            }

            def evaluate(*_args, **_kwargs):
                changed_definition = {**case_definition, "changed": True}
                cases.write_text(
                    json.dumps(changed_definition) + "\n"
                )
                return runner.Result(
                    "case",
                    "critic",
                    True,
                    telemetry_complete=True,
                    resolved_models=[RESOLVED_MODEL],
                )

            argv = [
                "runner.py",
                "--suite",
                "critic",
                "--report",
                str(report_path),
            ]
            output = io.StringIO()
            with (
                patch.object(runner, "SUITES", suites),
                patch.object(runner.sys, "argv", argv),
                patch.object(
                    runner.shutil,
                    "which",
                    side_effect=lambda name: (
                        runner.sys.executable if name == "claude" else None
                    ),
                ),
                patch.object(runner, "evaluate_case", side_effect=evaluate),
                patch.object(runner, "git_revision", return_value="revision"),
                patch.object(runner, "claude_cli_version", return_value="test-cli"),
                redirect_stdout(output),
            ):
                status = runner.main()
            report = json.loads(report_path.read_text())

        self.assertEqual(status, 1)
        self.assertFalse(report["cases"][0]["passed"])
        self.assertIn(
            "case or fixture definition changed during evaluation",
            report["cases"][0]["reasons"],
        )
        self.assertFalse(report["suites"]["critic"]["definition_stable"])
        self.assertTrue(runner.report_comparison_issues(report, "test"))
        self.assertIn("definition changed", output.getvalue())

    def test_frozen_evaluation_target_isolated_from_later_source_changes(self):
        case = {"id": "case"}
        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            source = root / "agent.md"
            source.write_text("first prompt\n")
            suite = {"subagent": source}
            frozen = root / "frozen"
            frozen.mkdir()

            source_digest = runner.evaluation_target_digest(
                "critic", suite, [case]
            )
            frozen_suite, workflow_root = runner.snapshot_evaluation_targets(
                "critic", suite, [case], frozen
            )
            frozen_digest = runner.evaluation_target_digest(
                "critic", frozen_suite, [case], workflow_root=workflow_root
            )
            source.write_text("second prompt\n")
            changed_source_digest = runner.evaluation_target_digest(
                "critic", suite, [case]
            )
            unchanged_frozen_digest = runner.evaluation_target_digest(
                "critic", frozen_suite, [case], workflow_root=workflow_root
            )

        self.assertEqual(source_digest, frozen_digest)
        self.assertNotEqual(source_digest, changed_source_digest)
        self.assertEqual(frozen_digest, unchanged_frozen_digest)

    def test_main_fails_report_when_evaluation_target_changes_during_run(self):
        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            cases = root / "critic.jsonl"
            cases.write_text(json.dumps(valid_critic_case_definition()) + "\n")
            subagent = root / "agent.md"
            subagent.write_text("---\nname: test\ndescription: test\n---\nPrompt.\n")
            report_path = root / "report.json"
            suites = {
                "critic": {
                    "subagent": subagent,
                    "cases": cases,
                    "renderer": "render_critic_input",
                    "uses_fixture": False,
                }
            }

            def evaluate(*_args, **_kwargs):
                subagent.write_text(
                    "---\nname: test\ndescription: test\n---\nChanged prompt.\n"
                )
                return runner.Result(
                    "case",
                    "critic",
                    True,
                    telemetry_complete=True,
                    resolved_models=[RESOLVED_MODEL],
                )

            argv = [
                "runner.py",
                "--suite",
                "critic",
                "--report",
                str(report_path),
            ]
            output = io.StringIO()
            with (
                patch.object(runner, "SUITES", suites),
                patch.object(runner.sys, "argv", argv),
                patch.object(
                    runner.shutil, "which", return_value=runner.sys.executable
                ),
                patch.object(runner, "evaluate_case", side_effect=evaluate),
                patch.object(runner, "git_revision", return_value="revision"),
                patch.object(runner, "claude_cli_version", return_value="test-cli"),
                redirect_stdout(output),
            ):
                status = runner.main()
            report = json.loads(report_path.read_text())

        self.assertEqual(status, 1)
        self.assertFalse(report["cases"][0]["passed"])
        self.assertIn(
            "evaluated agent or skill changed during evaluation",
            report["cases"][0]["reasons"],
        )
        self.assertFalse(
            report["suites"]["critic"]["target_definition_stable"]
        )
        self.assertTrue(runner.report_comparison_issues(report, "test"))
        self.assertIn("agent or skill changed", output.getvalue())

    def test_main_fails_report_when_claude_runtime_changes_during_run(self):
        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            cases = root / "critic.jsonl"
            cases.write_text(json.dumps(valid_critic_case_definition()) + "\n")
            subagent = root / "agent.md"
            subagent.write_text("---\nname: test\ndescription: test\n---\nPrompt.\n")
            report_path = root / "report.json"
            suites = {
                "critic": {
                    "subagent": subagent,
                    "cases": cases,
                    "renderer": "render_critic_input",
                    "uses_fixture": False,
                }
            }
            invoked_with = []

            def evaluate(*_args, **kwargs):
                invoked_with.append(
                    (kwargs["claude_binary"], kwargs["claude_version"])
                )
                return runner.Result(
                    "case",
                    "critic",
                    True,
                    telemetry_complete=True,
                    resolved_models=[RESOLVED_MODEL],
                )

            argv = [
                "runner.py",
                "--suite",
                "critic",
                "--report",
                str(report_path),
            ]
            output = io.StringIO()
            with (
                patch.object(runner, "SUITES", suites),
                patch.object(runner.sys, "argv", argv),
                patch.object(
                    runner.shutil, "which", return_value=runner.sys.executable
                ),
                patch.object(runner, "evaluate_case", side_effect=evaluate),
                patch.object(runner, "git_revision", return_value="revision"),
                patch.object(
                    runner,
                    "claude_cli_version",
                    side_effect=["test-cli-before", "test-cli-after"],
                ),
                redirect_stdout(output),
            ):
                status = runner.main()
            report = json.loads(report_path.read_text())

        self.assertEqual(status, 1)
        self.assertEqual(
            invoked_with,
            [(Path(runner.sys.executable).resolve(), "test-cli-before")],
        )
        self.assertFalse(report["cases"][0]["passed"])
        self.assertIn(
            "Claude CLI changed during evaluation", report["cases"][0]["reasons"]
        )
        self.assertFalse(report["claude_cli_stable"])
        self.assertTrue(runner.report_comparison_issues(report, "test"))
        self.assertIn("Claude CLI changed", output.getvalue())

    def test_main_fails_report_when_repository_head_changes_during_run(self):
        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            cases = root / "critic.jsonl"
            cases.write_text(json.dumps(valid_critic_case_definition()) + "\n")
            subagent = root / "agent.md"
            subagent.write_text(
                "---\nname: test\ndescription: test\n---\nPrompt.\n"
            )
            report_path = root / "report.json"
            suites = {
                "critic": {
                    "subagent": subagent,
                    "cases": cases,
                    "renderer": "render_critic_input",
                    "uses_fixture": False,
                }
            }
            argv = [
                "runner.py",
                "--suite",
                "critic",
                "--report",
                str(report_path),
            ]
            output = io.StringIO()
            with (
                patch.object(runner, "SUITES", suites),
                patch.object(runner.sys, "argv", argv),
                patch.object(
                    runner.shutil, "which", return_value=runner.sys.executable
                ),
                patch.object(
                    runner,
                    "evaluate_case",
                    return_value=runner.Result(
                        "case",
                        "critic",
                        True,
                        telemetry_complete=True,
                        resolved_models=[RESOLVED_MODEL],
                    ),
                ),
                patch.object(
                    runner,
                    "git_revision",
                    side_effect=["1" * 40, "2" * 40],
                ),
                patch.object(
                    runner, "claude_cli_version", return_value="test-cli"
                ),
                redirect_stdout(output),
            ):
                status = runner.main()
            report = json.loads(report_path.read_text())

        self.assertEqual(status, 1)
        self.assertEqual(report["git_revision"], "1" * 40)
        self.assertFalse(report["cases"][0]["passed"])
        self.assertIn(
            "repository HEAD changed during evaluation",
            report["cases"][0]["reasons"],
        )
        self.assertIn("repository HEAD changed", output.getvalue())

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
            patch.object(
                runner.shutil, "which", return_value=runner.sys.executable
            ),
            patch.object(runner, "claude_cli_version", return_value="test-cli"),
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


class CriticGraderTests(unittest.TestCase):
    def evaluate(self, output, case=None, permission_denials=None):
        if case is None:
            case = json.loads(runner.SUITES["critic"]["cases"].read_text().splitlines()[0])
        events = [
            {"type": "system", "subtype": "init", "model": RESOLVED_MODEL},
            {
                "type": "result",
                "subtype": "success",
                "is_error": False,
                "result": output,
                "duration_ms": 1000,
                "duration_api_ms": 800,
                "num_turns": 1,
                "total_cost_usd": 0,
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 50,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                },
                "modelUsage": {RESOLVED_MODEL: {}},
                "permission_denials": permission_denials or [],
            },
        ]
        process = ProcessResult(
            stdout="\n".join(json.dumps(event) for event in events).encode(),
            stderr=b"",
            returncode=0,
        )
        with patch.object(runner, "run_bounded", return_value=process):
            return runner.evaluate_case(
                "opus", "critic", runner.SUITES["critic"], case, keep_fixtures=False
            )

    def test_critic_grades_the_final_verdict_instead_of_mentions(self):
        result = self.evaluate(
            "The proposal contains fabricated targets and AI slop. I considered rethink.\n\n"
            "## Verdict\nship with caveats — proceed with this design."
        )
        self.assertFalse(result.passed)
        self.assertIn("ship with caveats", " ".join(result.reasons))

    def test_permission_denial_fails_without_persisting_denial_input(self):
        result = self.evaluate(
            "The design fabricates a target.\n\n## Verdict\nrethink — unsafe.",
            permission_denials=[
                {
                    "tool_name": "Read",
                    "tool_input": {"file_path": "/private/secret"},
                }
            ],
        )
        self.assertFalse(result.passed)
        self.assertEqual(result.reasons, ["permission denied for tools: ['Read']"])
        self.assertEqual(result.output_excerpt, "")
        self.assertTrue(result.telemetry_complete)
        self.assertNotIn("/private/secret", " ".join(result.reasons))

    def test_invalid_stream_does_not_copy_tool_input_into_diagnostics(self):
        events = [
            {"type": "system", "subtype": "init", "model": RESOLVED_MODEL},
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "name": "Read",
                            "input": {"file_path": "/private/secret"},
                        }
                    ]
                },
            },
            {
                "type": "result",
                "subtype": "success",
                "is_error": False,
                "result": (
                    "The design fabricates a target.\n\n"
                    "## Verdict\nrethink — unsafe."
                ),
                "duration_ms": 1,
                "duration_api_ms": 1,
                "num_turns": 1,
                "total_cost_usd": 0,
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 1,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                },
                "modelUsage": {RESOLVED_MODEL: {}},
            },
        ]
        process = ProcessResult(
            stdout="\n".join(json.dumps(event) for event in events).encode(),
            stderr=b"",
            returncode=0,
        )
        with patch.object(runner, "run_bounded", return_value=process):
            result = runner.evaluate_case(
                "opus",
                "critic",
                runner.SUITES["critic"],
                json.loads(
                    runner.SUITES["critic"]["cases"].read_text().splitlines()[0]
                ),
                keep_fixtures=False,
            )
        self.assertFalse(result.passed)
        self.assertEqual(result.output_excerpt, "")
        self.assertEqual(len(result.reasons), 1)
        self.assertTrue(result.reasons[0].startswith("invalid Claude stream:"))
        self.assertEqual(result.telemetry_issues, ["Claude stream was invalid"])
        self.assertNotIn("/private/secret", " ".join(result.reasons))

    def test_nonzero_cli_exit_does_not_copy_process_output_into_diagnostics(self):
        process = ProcessResult(
            stdout=b'{"tool_input":{"file_path":"/private/stdout-secret"}}',
            stderr=b"failed near /private/stderr-secret",
            returncode=1,
        )
        with patch.object(runner, "run_bounded", return_value=process):
            result = runner.evaluate_case(
                "opus",
                "critic",
                runner.SUITES["critic"],
                json.loads(
                    runner.SUITES["critic"]["cases"].read_text().splitlines()[0]
                ),
                keep_fixtures=False,
            )
        self.assertFalse(result.passed)
        self.assertEqual(result.reasons, ["claude exit 1"])
        self.assertEqual(result.output_excerpt, "")
        self.assertEqual(
            result.telemetry_issues, ["Claude exited before valid telemetry"]
        )

    def test_transport_failures_do_not_copy_process_output_into_diagnostics(self):
        expected_reasons = {
            "timeout": f"timeout after {runner.CLAUDE_CASE_TIMEOUT_SECONDS}s",
            "output_limit": "Claude process output exceeded transport limits",
            "spawn_error": "cannot start Claude CLI",
            "unsupported_platform": (
                "bounded Claude process transport requires POSIX"
            ),
        }
        case = json.loads(
            runner.SUITES["critic"]["cases"].read_text().splitlines()[0]
        )
        for stop_reason, expected in expected_reasons.items():
            process = ProcessResult(
                stdout=b"/private/stdout-secret",
                stderr=b"/private/stderr-secret",
                returncode=-9,
                stop_reason=stop_reason,
            )
            with self.subTest(stop_reason=stop_reason), patch.object(
                runner, "run_bounded", return_value=process
            ):
                result = runner.evaluate_case(
                    "opus",
                    "critic",
                    runner.SUITES["critic"],
                    case,
                    keep_fixtures=False,
                )
            self.assertFalse(result.passed)
            self.assertEqual(result.reasons, [expected])
            self.assertEqual(result.output_excerpt, "")
            self.assertEqual(result.telemetry_issues, [expected])
            report = json.dumps(runner.result_report(result))
            self.assertNotIn("stdout-secret", report)
            self.assertNotIn("stderr-secret", report)

    def test_non_utf8_process_output_fails_without_copying_bytes(self):
        process = ProcessResult(
            stdout=b"\xff/private/stream-secret",
            stderr=b"",
            returncode=0,
        )
        with patch.object(runner, "run_bounded", return_value=process):
            result = runner.evaluate_case(
                "opus",
                "critic",
                runner.SUITES["critic"],
                json.loads(
                    runner.SUITES["critic"]["cases"].read_text().splitlines()[0]
                ),
                keep_fixtures=False,
            )
        self.assertFalse(result.passed)
        self.assertEqual(result.reasons, ["invalid Claude stream encoding"])
        self.assertEqual(result.output_excerpt, "")
        self.assertEqual(
            result.telemetry_issues, ["Claude stream encoding was invalid"]
        )

    def test_pre_runtime_failure_produces_a_structurally_valid_report(self):
        case = json.loads(
            runner.SUITES["critic"]["cases"].read_text().splitlines()[0]
        )
        result = runner.failed_evaluation_result(
            case["id"],
            "critic",
            "cannot start Claude CLI",
            fixture_path=None,
            telemetry_issue="cannot start Claude CLI",
        )
        with patch.object(runner, "git_revision", return_value="1" * 40):
            report = runner.build_report(
                [result],
                model="opus",
                suite_filter="critic",
                case_filter=case["id"],
                claude_version="test-cli",
            )

        self.assertEqual(report["resolved_models"], [])
        self.assertEqual(
            runner.report_comparison_issues(report, "failure"), []
        )

    def test_incomplete_telemetry_skips_secondary_grading(self):
        events = [
            {"type": "system", "subtype": "init", "model": RESOLVED_MODEL},
            {
                "type": "result",
                "subtype": "success",
                "is_error": False,
                "result": "fabricated slop\n\n## Verdict\nrethink — unsafe.",
                "duration_ms": 1,
                "duration_api_ms": 1,
                "num_turns": 1,
                "total_cost_usd": 0,
                "modelUsage": {RESOLVED_MODEL: {}},
                "usage": {
                    "input_tokens": 1,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                },
            },
        ]
        process = ProcessResult(
            stdout="\n".join(json.dumps(event) for event in events).encode(),
            stderr=b"",
            returncode=0,
        )
        with patch.object(runner, "run_bounded", return_value=process):
            result = runner.evaluate_case(
                "opus",
                "critic",
                runner.SUITES["critic"],
                json.loads(
                    runner.SUITES["critic"]["cases"].read_text().splitlines()[0]
                ),
                keep_fixtures=False,
            )

        self.assertFalse(result.passed)
        self.assertEqual(len(result.reasons), 1)
        self.assertTrue(result.reasons[0].startswith("incomplete Claude telemetry:"))
        self.assertEqual(
            result.telemetry_issues,
            ["usage.output_tokens must be a non-negative integer"],
        )
        self.assertEqual(result.resolved_models, [RESOLVED_MODEL])
        self.assertEqual(result.output_excerpt, "")

    def test_critic_rejects_missing_quoted_or_conflicting_verdicts(self):
        outputs = [
            "The verdict should be rethink.",
            "```markdown\n## Verdict\nrethink — invalid approach.\n```",
            "~~~~markdown\n## Verdict\nrethink — invalid approach.\n~~~~",
            "> ## Verdict\n> rethink — invalid approach.",
            "    ## Verdict\n    rethink — invalid approach.",
            "## Verdict\nrethink — invalid approach.\n## Verdict\nship with caveats — proceed.",
            "## Verdict\nrethink — invalid approach.\nship with caveats — proceed.",
            "## Verdict\nrethink — invalid approach.\n- **ship with caveats** — proceed.",
            "## Verdict\nrethink — invalid approach.\n## Verdict\nrethink — repeated.",
            "## Verdict\nrethink or revise — uncertain.",
            "## Verdict\nrethink — invalid approach.\n## Final answer\nship it.",
            "## Verdict\ninsufficient evidence — unknown.\n\n**Verdict:** ship it",
            "**Verdict:** rethink\n\n## Verdict\nship it — proceed.",
        ]
        for output in outputs:
            with self.subTest(output=output):
                result = self.evaluate("fabricated slop\n\n" + output)
                self.assertFalse(result.passed, result.reasons)

    def test_critic_accepts_exact_contract_verdicts_and_markdown_emphasis(self):
        for verdict in ("ship it", "ship with caveats", "revise", "rethink", "insufficient evidence"):
            for label in (verdict, f"**{verdict}**", f"`{verdict}`"):
                with self.subTest(label=label):
                    case = {
                        "id": "critic-verdict-contract",
                        "input": {},
                        "expect": {"verdict": verdict},
                    }
                    result = self.evaluate(
                        "```markdown\n## Verdict\nrethink — quoted example.\n```\n\n"
                        f"## Verdict\n{label} — evidence determines the outcome.\n",
                        case,
                    )
                    self.assertTrue(result.passed, result.reasons)

    def test_critic_missing_evidence_is_not_a_design_failure(self):
        case = {
            "id": "critic-missing-evidence",
            "input": {"mmcg_snapshot": "Unavailable"},
            "expect": {"verdict": "insufficient evidence"},
        }
        result = self.evaluate(
            "The cache invalidation contract has not been inspected.\n\n"
            "## Verdict\ninsufficient evidence — read the mutation paths first.",
            case,
        )
        self.assertTrue(result.passed, result.reasons)
        for wrong in ("revise", "rethink", "ship it"):
            with self.subTest(wrong=wrong):
                result = self.evaluate(f"## Verdict\n{wrong} — no evidence.", case)
                self.assertFalse(result.passed)

    def test_shipped_critic_cases_expect_aggregate_verdicts(self):
        for line in runner.SUITES["critic"]["cases"].read_text().splitlines():
            case = json.loads(line)
            verdicts = case["expect"]["verdict"]
            if isinstance(verdicts, str):
                verdicts = [verdicts]
            self.assertTrue(set(verdicts) <= runner.CRITIC_VERDICTS, case["id"])

    def test_portable_critic_uses_the_same_final_verdict_grader(self):
        case = {
            "id": "portable-critic-evidence",
            "artifact": "skills/workflow/mastermind-critical-review/SKILL.md",
            "input": {"prompt": "Only the file name is known."},
            "expect": {"verdict": "insufficient evidence"},
        }
        suite = runner.SUITES["workflow"]
        for verdict, passed in (("insufficient evidence", True), ("ship it", False)):
            events = [
                {"type": "system", "subtype": "init", "model": RESOLVED_MODEL},
                {
                    "type": "result", "subtype": "success", "is_error": False,
                    "result": (
                        "I considered insufficient evidence.\n\n"
                        f"## Verdict\n{verdict} — assessment of supplied evidence."
                    ),
                    "duration_ms": 1, "duration_api_ms": 1, "num_turns": 1,
                    "total_cost_usd": 0, "modelUsage": {RESOLVED_MODEL: {}},
                    "usage": {
                        "input_tokens": 1, "output_tokens": 1,
                        "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0,
                    },
                },
            ]
            process = ProcessResult(
                stdout="\n".join(json.dumps(event) for event in events).encode(),
                stderr=b"",
                returncode=0,
            )
            with self.subTest(verdict=verdict), patch.object(
                runner, "run_bounded", return_value=process
            ):
                result = runner.evaluate_case("opus", "workflow", suite, case, keep_fixtures=False)
                self.assertEqual(result.passed, passed, result.reasons)


class PromptIsolationTests(unittest.TestCase):
    def test_vanilla_comparison_preserves_uncommitted_fixture_state(self):
        for staged in ([], ["src/staged.py"]):
            case = {
                "fixture": "uncommitted-audit", "baseline_ref": "baseline",
                "after_ref": "executor-added", "staged_paths": staged,
                "input": {}, "expect": {"contains": ["changed"]},
            }
            fixture = Path("/unused-disposable-fixture")
            events = [
                {
                    "type": "system",
                    "subtype": "init",
                    "model": RESOLVED_MODEL,
                    "claude_code_version": "2.1.236",
                    "cwd": str(fixture),
                    "permissionMode": "dontAsk",
                    "tools": list(ablation.VANILLA_STREAM_TOOLS),
                    "mcp_servers": [],
                    "skills": [],
                    "plugins": [],
                },
                {
                    "type": "assistant",
                    "message": {
                        "model": RESOLVED_MODEL,
                        "content": [
                            {
                                "type": "tool_use",
                                "id": "git-inspection",
                                "name": "Bash",
                                "input": {
                                    "command": "git diff refs/tags/baseline --"
                                },
                            }
                        ],
                    },
                },
                {
                    "type": "user",
                    "message": {
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": "git-inspection",
                            }
                        ]
                    },
                },
                {
                    "type": "result",
                    "subtype": "success",
                    "is_error": False,
                    "result": "changed",
                    "duration_ms": 1,
                    "duration_api_ms": 1,
                    "num_turns": 1,
                    "total_cost_usd": 0,
                    "modelUsage": {RESOLVED_MODEL: {}},
                    "usage": {
                        "input_tokens": 1,
                        "output_tokens": 1,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0,
                    },
                },
            ]
            process = ProcessResult(
                stdout="\n".join(json.dumps(event) for event in events).encode(),
                stderr=b"",
                returncode=0,
            )
            with (
                self.subTest(staged=staged),
                patch.object(runner, "setup_fixture", return_value=fixture) as setup,
                patch.object(runner, "teardown_fixture"),
                patch.object(runner, "run_bounded", return_value=process) as invoke,
            ):
                outcome = ablation.run_vanilla(
                    "opus",
                    case,
                    claude_binary=Path("/runtime/claude"),
                    claude_version="2.1.236 (Claude Code)",
                    git_binary=Path("/runtime/bin/git"),
                )
                self.assertIsNotNone(outcome)
                self.assertTrue(outcome)
                self.assertEqual(outcome.resolved_models, (RESOLVED_MODEL,))
                setup.assert_called_once_with(
                    "uncommitted-audit",
                    "baseline",
                    "executor-added",
                    staged_paths=staged,
                    fixtures_dir=None,
                    git_binary=Path("/runtime/bin/git"),
                    mmcg_binary=None,
                )
                prompt = invoke.call_args.kwargs["stdin"].decode()
                self.assertIn("git diff refs/tags/baseline --", prompt)
                self.assertIn("git ls-files --others --exclude-standard --", prompt)
                self.assertNotIn("baseline..executor-added", prompt)
                command = invoke.call_args.args[0]
                for argument in (
                    "--effort",
                    "--max-turns",
                    "--strict-mcp-config",
                    "--setting-sources",
                    "--disable-slash-commands",
                    "--no-chrome",
                ):
                    self.assertIn(argument, command)
                self.assertEqual(command[0], "/runtime/claude")
                self.assertEqual(
                    invoke.call_args.kwargs["env"]["PATH"].split(os.pathsep)[0],
                    "/runtime/bin",
                )
                self.assertEqual(
                    invoke.call_args.kwargs["stdout_limit"],
                    runner.CLAUDE_STDOUT_LIMIT_BYTES,
                )
                self.assertEqual(
                    invoke.call_args.kwargs["stderr_limit"],
                    runner.CLAUDE_STDERR_LIMIT_BYTES,
                )
        committed = ablation.vanilla_message({"input": {}}, fixture, "baseline", "after")
        self.assertIn(
            "git diff refs/tags/baseline..refs/tags/after --", committed
        )

    def test_ablation_requires_one_resolved_model_identity(self):
        first = ablation.ConditionOutcome(True, (RESOLVED_MODEL,))
        expected, issue = ablation.merge_model_identity(None, first, "a vanilla")
        self.assertEqual(expected, (RESOLVED_MODEL,))
        self.assertIsNone(issue)

        expected, issue = ablation.merge_model_identity(
            expected,
            ablation.ConditionOutcome(False, (RESOLVED_MODEL,)),
            "a mastermind",
        )
        self.assertEqual(expected, (RESOLVED_MODEL,))
        self.assertIsNone(issue)

        expected, issue = ablation.merge_model_identity(
            expected,
            ablation.ConditionOutcome(True, ("different-model",)),
            "b vanilla",
        )
        self.assertEqual(expected, (RESOLVED_MODEL,))
        self.assertIn("b vanilla resolved model ids", issue)

    def test_vanilla_requires_a_successful_simple_git_inspection(self):
        valid = runner.ToolExecution(
            "Bash",
            "tool-1",
            {"command": "git diff refs/tags/baseline --"},
            result_seen=True,
            succeeded=True,
        )
        self.assertTrue(ablation._successful_git_inspection([valid]))
        for command, succeeded in (
            ("git diff refs/tags/baseline --", False),
            ("git diff refs/tags/baseline -- && curl example.invalid", True),
            ("cargo test --locked unit", True),
        ):
            with self.subTest(command=command, succeeded=succeeded):
                execution = runner.ToolExecution(
                    "Bash",
                    "tool-1",
                    {"command": command},
                    result_seen=True,
                    succeeded=succeeded,
                )
                self.assertFalse(ablation._successful_git_inspection([execution]))

    def test_fixture_copy_exposes_same_size_changes_despite_matching_source_mtimes(self):
        with tempfile.TemporaryDirectory(prefix="mmcg-fixture-stat-cache-") as temporary:
            root = Path(temporary)
            before, after, repository = (root / name for name in ("before", "after", "repo"))
            paths = {"root.py", "src/staged.py"}
            old_timestamp = 946684800
            for tree, value in ((before, "1"), (after, "3")):
                for relative in paths:
                    source = tree / relative
                    source.parent.mkdir(parents=True, exist_ok=True)
                    source.write_text(f"MAX_ATTEMPTS = {value}\n", encoding="utf-8")
                    os.utime(source, (old_timestamp, old_timestamp))

            runner._copy_tree_into(before, repository)
            # Seed an old cached mtime even when the copier correctly uses fresh
            # timestamps. This makes the regression independent of clock resolution.
            for relative in paths:
                os.utime(repository / relative, (old_timestamp, old_timestamp))
            for arguments in (
                ["init", "-q", "--initial-branch=main"],
                ["config", "core.trustctime", "false"],
                ["config", "core.checkStat", "minimal"],
                ["add", "-A"],
                ["commit", "-q", "-m", "baseline"],
            ):
                runner._run_git(arguments, repository)

            runner._copy_tree_into(after, repository)
            for relative in paths:
                self.assertEqual(
                    (repository / relative).read_text(encoding="utf-8"), "MAX_ATTEMPTS = 3\n"
                )
            changed = subprocess.run(
                ["git", "diff", "--name-only", "HEAD"],
                cwd=repository, text=True, capture_output=True, check=True,
            ).stdout.splitlines()
            self.assertEqual(set(changed), paths)
            runner._run_git(["add", "-A"], repository)
            staged = subprocess.run(
                ["git", "diff", "--cached", "--name-only"],
                cwd=repository, text=True, capture_output=True, check=True,
            ).stdout.splitlines()
            self.assertEqual(set(staged), paths)

    def test_auditor_runs_verify_as_an_exact_standalone_command(self):
        auditor = runner.SUITES["auditor"]["subagent"].read_text(encoding="utf-8")
        self.assertIn("Run each reported\n   `VERIFY` command exactly as written", auditor)
        self.assertIn("do not prepend `cd`", auditor)
        self.assertIn("do not append pipes", auditor)

    def test_fixture_setup_failure_removes_its_temporary_repository(self):
        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            fixtures = root / "fixtures"
            baseline = fixtures / "sample" / "baseline"
            after = fixtures / "sample" / "changes" / "after"
            baseline.mkdir(parents=True)
            after.mkdir(parents=True)
            (baseline / "source.py").write_text("before = True\n")
            (after / "source.py").write_text("after = True\n")
            temporary_repo = root / "temporary-repo"

            def make_temporary_repo(*_args, **_kwargs):
                temporary_repo.mkdir()
                return str(temporary_repo)

            with (
                patch.object(runner, "FIXTURES_DIR", fixtures),
                patch.object(runner.tempfile, "mkdtemp", side_effect=make_temporary_repo),
                patch.object(runner, "_run_git", side_effect=RuntimeError("git failed")),
                self.assertRaisesRegex(RuntimeError, "git failed"),
            ):
                runner.setup_fixture("sample", "baseline", "after")

            self.assertFalse(temporary_repo.exists())

    def test_fixture_index_timeout_is_a_nonfatal_missing_index(self):
        with tempfile.TemporaryDirectory() as target:
            root = Path(target)
            fixtures = root / "fixtures"
            baseline = fixtures / "sample" / "baseline"
            after = fixtures / "sample" / "changes" / "after"
            baseline.mkdir(parents=True)
            after.mkdir(parents=True)
            (baseline / "source.py").write_text("before = True\n")
            (after / "source.py").write_text("after = True\n")
            with (
                patch.object(runner, "FIXTURES_DIR", fixtures),
                patch.object(runner, "_run_git"),
                patch.object(
                    runner,
                    "_build_mmcg_index",
                    side_effect=RuntimeError("mmcg index stopped: timeout"),
                ),
                patch.object(runner.sys, "stderr", io.StringIO()),
            ):
                fixture = runner.setup_fixture("sample", "baseline", "after")
            self.addCleanup(runner.teardown_fixture, fixture)
            self.assertTrue(fixture.is_dir())
            self.assertFalse((fixture / ".mastermind" / "mmcg.db").exists())

    def test_auditor_file_inventory_covers_staged_unstaged_and_untracked_changes(self):
        case = next(
            case for case in map(json.loads, runner.SUITES["auditor"]["cases"].read_text().splitlines())
            if case["id"] == "a-010-uncommitted-execution-held"
        )
        with patch.object(runner, "_build_mmcg_index"):
            fixture = runner.setup_fixture(
                case["fixture"], case["baseline_ref"], case["after_ref"],
                staged_paths=case["staged_paths"],
            )
        self.addCleanup(runner.teardown_fixture, fixture)
        status = subprocess.run(
            ["git", "status", "--porcelain=v1", "--untracked-files=all"],
            cwd=fixture, text=True, capture_output=True, check=True,
        ).stdout.splitlines()
        self.assertEqual(set(status), {"M  src/staged.py", " M src/unstaged.py", "?? src/added.py"})
        committed_diff = subprocess.run(
            ["git", "diff", "--name-status", "baseline...HEAD"],
            cwd=fixture, text=True, capture_output=True, check=True,
        ).stdout
        self.assertEqual(committed_diff, "")
        auditor = runner.SUITES["auditor"]["subagent"].read_text(encoding="utf-8")
        commands = [
            command.replace("<baseline>", "baseline")
            for command in re.findall(r"`(git [^`]+)`", auditor)
            if command.startswith("git diff --name-status ")
            or command.startswith("git ls-files --others ")
        ]
        changed_files = set()
        for command in commands:
            result = subprocess.run(
                shlex.split(command), cwd=fixture, text=True, capture_output=True, check=True
            )
            changed_files.update(line.split("\t")[-1] for line in result.stdout.splitlines())
        self.assertEqual(changed_files, {"src/staged.py", "src/unstaged.py", "src/added.py"})

    def test_auditor_fixture_keeps_committed_mode_as_default(self):
        with patch.object(runner, "_build_mmcg_index"):
            fixture = runner.setup_fixture("uncommitted-audit", "baseline", "executor-added")
        self.addCleanup(runner.teardown_fixture, fixture)
        status = subprocess.run(
            ["git", "status", "--porcelain=v1", "--untracked-files=all"],
            cwd=fixture, text=True, capture_output=True, check=True,
        ).stdout
        self.assertEqual(status, "")
        changed = subprocess.run(
            ["git", "diff", "--name-only", "baseline..executor-added"],
            cwd=fixture, text=True, capture_output=True, check=True,
        ).stdout.splitlines()
        self.assertEqual(set(changed), {"src/staged.py", "src/unstaged.py", "src/added.py"})

    def test_synthetic_prompt_suites_cannot_inspect_the_maintainer_checkout(self):
        for suite in ("critic", "intake", "workflow"):
            arguments = runner.isolated_cli_args(suite)
            self.assertEqual(arguments[:3], ["--safe-mode", "--tools", ""])
            for flag in (
                "--setting-sources",
                "--strict-mcp-config",
                "--disable-slash-commands",
                "--no-chrome",
            ):
                self.assertIn(flag, arguments)
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
            "mcp__mmcg__mmcg_status",
            "mcp__mmcg__mmcg_search",
            "mcp__mmcg__mmcg_callers",
            "mcp__mmcg__mmcg_impact",
        ):
            self.assertIn(tool, allowed)
        self.assertNotIn("scratchpad_append", allowed)
        self.assertNotIn("Bash(cargo", allowed)
        self.assertNotIn("Bash(cargo *)", allowed)
        self.assertNotIn("Bash(cargo test *)", allowed)
        verification = "cargo test --locked exact_test"
        auditor_args = runner.isolated_cli_args(
            "auditor", verification_commands=(verification,)
        )
        allowed = auditor_args[auditor_args.index("--allowedTools") + 1]
        self.assertIn(f"Bash({verification})", allowed)
        self.assertNotIn("Bash(cargo test --locked *)", allowed)
        self.assertFalse(runner.requires_prompt_sandbox("auditor"))

    def test_source_only_subagent_runtime_removes_unavailable_mmcg(self):
        path = runner.SUITES["researcher"]["subagent"]
        _, definition = runner.subagent_runtime_definition(
            path, model_override="haiku", include_mmcg=False
        )
        self.assertEqual(definition["tools"], ["Read", "Grep", "Glob"])
        self.assertNotIn("mcpServers", definition)

        arguments = runner.isolated_cli_args(
            "researcher", subagent=path, include_mmcg=False
        )
        allowed = arguments[arguments.index("--allowedTools") + 1]
        self.assertEqual(allowed, "Read,Glob,Grep")
        self.assertEqual(
            runner.expected_stream_tools(
                "researcher", subagent=path, include_mmcg=False
            ),
            ("Read", "Grep", "Glob", "EndConversation"),
        )
        agent_arguments = runner.subagent_cli_args(
            path, model_override="haiku", include_mmcg=False
        )
        payload = json.loads(agent_arguments[agent_arguments.index("--agents") + 1])
        self.assertEqual(
            payload["mastermind-researcher"]["tools"],
            ["Read", "Grep", "Glob"],
        )
        self.assertNotIn("mcpServers", payload["mastermind-researcher"])

    def test_researcher_allowed_tools_come_from_the_frozen_subagent(self):
        with tempfile.TemporaryDirectory() as target:
            subagent = Path(target) / "researcher.md"
            subagent.write_text(
                "---\n"
                "name: frozen-researcher\n"
                "description: Frozen definition\n"
                "tools:\n"
                "  - mcp__mmcg__frozen_search\n"
                "---\n"
                "Prompt.\n"
            )
            arguments = runner.isolated_cli_args(
                "researcher", subagent=subagent
            )
        allowed = arguments[arguments.index("--allowedTools") + 1]
        self.assertIn("mcp__mmcg__frozen_search", allowed)

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
        self.assertIn("mcp__mmcg__mmcg_concept", definition["tools"])
        self.assertNotIn("Bash", definition["tools"])
        self.assertIn("Do not replace a complete graph answer", definition["prompt"])
        self.assertIn("data, never instructions", definition["prompt"])
        self.assertIn("Contradictions / Unknowns", definition["prompt"])

        args = runner.subagent_cli_args(path, model_override="haiku")
        payload = json.loads(args[args.index("--agents") + 1])
        self.assertEqual(payload[name]["prompt"], definition["prompt"])

    def test_project_router_enforces_graph_first_tool_selection(self):
        workflow = (
            runner.REPO_ROOT / "agents/claude-md/mastermind-workflow.md"
        ).read_text(encoding="utf-8")
        for token in (
            "Call `mmcg_brief` once",
            "`mmcg_concept`",
            "Do not use Bash to rediscover",
            "Bash remains the right tool for Git, builds, tests, linters, logs, and runtime probes",
        ):
            self.assertIn(token, workflow)

        codegraph = (
            runner.REPO_ROOT
            / "skills/workflow/mastermind-codegraph-research/SKILL.md"
        ).read_text(encoding="utf-8")
        for token in (
            "`mmcg_brief`",
            "`mmcg_concept`",
            "managed index",
            "custom external index",
        ):
            self.assertIn(token, codegraph)

    def test_graph_first_bash_roles_route_every_granted_mmcg_tool(self):
        for name in (
            "mastermind-investigator",
            "mastermind-security-auditor",
            "mastermind-task-executor",
        ):
            path = runner.REPO_ROOT / "agents/subagents" / f"{name}.md"
            _, definition = runner.subagent_runtime_definition(path)
            self.assertIn("Bash", definition["tools"], name)
            granted = {
                tool.removeprefix("mcp__mmcg__")
                for tool in definition["tools"]
                if tool.startswith("mcp__mmcg__mmcg_")
            }
            self.assertTrue(granted, name)
            for tool in granted:
                self.assertIn(tool, definition["prompt"], f"{name}: {tool}")

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

        _, investigator = runner.subagent_runtime_definition(
            runner.REPO_ROOT / "agents/subagents/mastermind-investigator.md"
        )
        self.assertLessEqual(len(investigator["prompt"]), 3_500, "investigator")
        self.assertNotIn("## Examples", investigator["prompt"])
        self.assertNotIn("## Companion pieces", investigator["prompt"])

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
        allowed = runner.isolated_cli_args(
            "auditor", verification_commands=(command,)
        )
        allowed = allowed[allowed.index("--allowedTools") + 1]
        self.assertIn(f"Bash({command})", allowed)
        self.assertNotIn("Bash(cargo test --locked *)", allowed)

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
        self.assertIn("git diff refs/tags/baseline --", rendered)
        self.assertIn("git ls-files --others --exclude-standard", rendered)
        self.assertIn("Read untracked file contents directly", rendered)

    def test_uncommitted_auditor_input_does_not_invent_an_executor_commit(self):
        rendered = runner.render_auditor_input(
            {"spec_summary": "three files", "executor_report": "complete"},
            fixture_path=runner.REPO_ROOT,
            baseline_ref="baseline",
            after_ref="after",
            has_mmcg=True,
            uncommitted=True,
        )
        self.assertIn("HEAD remains at the baseline", rendered)
        self.assertNotIn("Executor commit tag", rendered)
        self.assertNotIn("`after`", rendered)
        self.assertIn("current working-tree state", rendered)

    def test_frontmatter_is_removed_without_dropping_prompt_body(self):
        text = "---\nname: demo\n---\n\n# Contract\nBody\n"
        self.assertEqual(runner.strip_frontmatter(text), "# Contract\nBody\n")


if __name__ == "__main__":
    unittest.main()
