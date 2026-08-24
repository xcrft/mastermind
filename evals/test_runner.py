import json
import os
import shlex
import subprocess
import tempfile
import unittest

from evals import runner


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
                "duration_api_ms": 1234,
                "num_turns": 3,
                "total_cost_usd": 0.0125,
                "usage": {
                    "input_tokens": 101,
                    "output_tokens": 202,
                    "cache_creation_input_tokens": 303,
                    "cache_read_input_tokens": 404,
                },
            }
        )
        self.assertEqual(telemetry["duration_api_ms"], 1234)
        self.assertEqual(telemetry["num_turns"], 3)
        self.assertEqual(telemetry["input_tokens"], 101)
        self.assertEqual(telemetry["output_tokens"], 202)
        self.assertEqual(telemetry["cache_creation_input_tokens"], 303)
        self.assertEqual(telemetry["cache_read_input_tokens"], 404)
        self.assertEqual(telemetry["cost_usd"], 0.0125)

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

    def test_critic_prompt_states_the_no_tools_evaluation_boundary(self):
        rendered = runner.render_critic_input(
            {"problem": "p", "design": "d", "alternatives": [], "constraints": "c"}
        )
        self.assertIn("no repository checkout or tools are available", rendered)
        self.assertIn("do not invoke tools", rendered)

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
