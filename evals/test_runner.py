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
    def test_synthetic_prompt_suites_cannot_inspect_the_maintainer_checkout(self):
        for suite in ("critic", "intake", "workflow"):
            self.assertEqual(runner.isolated_cli_args(suite), ["--safe-mode", "--tools", ""])
            self.assertTrue(runner.requires_prompt_sandbox(suite))
        self.assertEqual(runner.isolated_cli_args("auditor"), [])
        self.assertFalse(runner.requires_prompt_sandbox("auditor"))

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
