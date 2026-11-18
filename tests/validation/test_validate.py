"""validate.py rules and exit status, run against temporary copies of their inputs."""

from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch
import io
import json
import re
import shutil
import tempfile
import unittest

from scripts import validate


class DocumentationDiscoveryTests(unittest.TestCase):
    def test_local_state_is_excluded_without_hiding_source_under_an_ignored_ancestor(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "target" / "checkout"
            private = root / ".mastermind" / "research" / "copy" / "README.md"
            private.parent.mkdir(parents=True)
            private.write_text("[private](missing.md)\n[[missing-private]]\n")
            source = root / "README.md"
            source.write_text("[source](missing.md)\n[[missing-source]]\n")
            nearby = root / "mastermind-notes" / "README.md"
            nearby.parent.mkdir()
            nearby.write_text("[source](../README.md)\n")
            with patch.object(validate, "REPO_ROOT", root):
                links = validate.collect_relative_links()
                self.assertEqual(set(links), {source, nearby})
                issues = validate.validate_relative_links(links)
                self.assertEqual([issue.path for issue in issues], [source])
                wiki = validate.collect_wikilinks()
                self.assertEqual(wiki, {source: {"missing-source"}})
                issues = validate.validate_wikilinks([], wiki)
                self.assertEqual([issue.path for issue in issues], [source])


class EvalFixtureClueTests(unittest.TestCase):
    def test_behavior_fixture_sources_are_scanned_but_readmes_are_excluded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / "evals/behavior/fixtures/sample"
            source = fixture / "changes/after/source.py"
            source.parent.mkdir(parents=True)
            source.write_text("# auditor must catch this\n")
            baseline = fixture / "baseline"
            baseline.mkdir()
            (baseline / "README.md").write_text("auditor must catch this\n")
            (fixture / "README.md").write_text("auditor must catch this\n")
            with patch.object(validate, "REPO_ROOT", root):
                issues = validate.validate_eval_fixture_clues()
            self.assertEqual([issue.path for issue in issues], [source])
            self.assertEqual(issues[0].level, "error")
            self.assertIn("banned clue", issues[0].msg)

    def test_missing_or_empty_fixture_inventory_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixtures = root / "evals/behavior/fixtures"
            for state in ("missing", "empty", "empty-trees"):
                with self.subTest(state=state):
                    if state == "empty":
                        fixtures.mkdir(parents=True)
                    elif state == "empty-trees":
                        (fixtures / "sample/baseline").mkdir(parents=True)
                        (fixtures / "sample/changes").mkdir()
                    with patch.object(validate, "REPO_ROOT", root):
                        issues = validate.validate_eval_fixture_clues()
                    self.assertTrue(issues)
                    self.assertTrue(all(issue.level == "error" for issue in issues))


class ContractRuleTests(unittest.TestCase):
    """Each rule passes on copies of its real inputs and names one broken file."""

    CASES = (
        (
            "release badge",
            validate.validate_release_badges,
            ("npm/mastermind/package.json", "README.md", "npm/mastermind/README.md"),
            "README.md",
            lambda text: text.replace("badge/npm-v", "badge/npm-v0."),
            "npm badge must match package version",
        ),
        (
            "template mirror",
            validate.validate_mmcg_template_mirrors,
            tuple(path for pair in validate.MMCG_TEMPLATE_MIRRORS for path in pair),
            validate.MMCG_TEMPLATE_MIRRORS[0][1],
            lambda text: text + "drift\n",
            "out of sync with canonical",
        ),
        (
            "installable link escape",
            lambda: validate.validate_installable_link_escape(
                validate.collect_relative_links()
            ),
            ("agents/subagents",),
            "agents/subagents/mastermind-critic.md",
            lambda text: text + "\n[parent](../README.md)\n",
            "installable file escapes package",
        ),
        (
            "workflow pin",
            validate.validate_repository_workflow_pins,
            (".github/workflows",),
            ".github/workflows/ci-mmcg.yml",
            lambda text: re.sub(r"@[0-9a-f]{40}", "@v4", text, count=1),
            "must use a full 40-character commit SHA",
        ),
        (
            "executor report schema",
            validate.validate_executor_report_schema_contract,
            (
                "schemas/executor-report-v1.schema.json",
                "agents/subagents/mastermind-task-executor.md",
                "skills/workflow/mastermind-structured-report-contract/SKILL.md",
                "skills/workflow/mastermind-task-planning/references/structured-report-schema.md",
                "mcp/servers/mmcg/tests/fixtures/executor-report-v1.md",
                "mcp/servers/mmcg/src/executor_report.rs",
            ),
            "schemas/executor-report-v1.schema.json",
            lambda text: json.dumps({**json.loads(text), "additionalProperties": True}),
            "must fail closed on unknown fields",
        ),
    )

    def test_rule_reports_the_broken_file_and_nothing_on_the_real_inputs(self):
        for label, rule, inputs, broken, edit, message in self.CASES:
            with self.subTest(rule=label), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                for relative in inputs:
                    source = validate.REPO_ROOT / relative
                    target = root / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    if source.is_dir():
                        shutil.copytree(source, target)
                    else:
                        shutil.copyfile(source, target)
                with patch.object(validate, "REPO_ROOT", root):
                    self.assertEqual(rule(), [])
                    original = (root / broken).read_text(encoding="utf-8")
                    edited = edit(original)
                    self.assertNotEqual(edited, original)
                    (root / broken).write_text(edited, encoding="utf-8")
                    issues = rule()
                self.assertEqual([issue.path for issue in issues], [root / broken], issues)
                self.assertIn(message, issues[0].msg)


class MainTests(unittest.TestCase):
    def test_exit_status_counts_errors_but_not_warnings(self):
        with redirect_stdout(io.StringIO()):
            self.assertEqual(validate.main([]), 0)
        for level, status in (("warning", 0), ("error", 1)):
            issue = validate.Issue(validate.REPO_ROOT / "README.md", level, f"injected {level}")
            output = io.StringIO()
            with (
                self.subTest(level=level),
                patch.object(validate, "validate_release_badges", return_value=[issue]),
                redirect_stdout(output),
            ):
                self.assertEqual(validate.main([]), status)
            self.assertIn(f"README.md: {level}: injected {level}", output.getvalue())
