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
            worktree = root / ".claude" / "worktrees" / "agent" / "README.md"
            worktree.parent.mkdir(parents=True)
            worktree.write_text("[copy](missing.md)\n[[missing-copy]]\n")
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

    def test_docs_and_evals_links_are_checked_but_their_wikilinks_are_illustration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pages = [root / "docs/guide.md", root / "evals/README.md"]
            for page in pages:
                page.parent.mkdir(parents=True)
                page.write_text("[missing](missing.md)\n[[example-slug]]\n")
            with patch.object(validate, "REPO_ROOT", root):
                issues = validate.validate_relative_links(validate.collect_relative_links())
                self.assertEqual(sorted(issue.path for issue in issues), sorted(pages))
                self.assertEqual(validate.collect_wikilinks(), {})


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
            "release version",
            validate.validate_release_versions,
            tuple(dict.fromkeys(path for path, _, _ in validate.release_version.LOCATIONS)),
            "npm/platforms/win32-x64-msvc/package.json",
            lambda text: re.sub(r'"version": "[^"]+"', '"version": "0.0.1"', text, count=1),
            "differs from mcp/servers/mmcg/Cargo.toml",
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
            "Docker Rust version",
            validate.validate_rust_toolchain_source,
            ("rust-toolchain.toml", "Dockerfile.audit-action", ".github/workflows"),
            "Dockerfile.audit-action",
            lambda text: re.sub(r"cargo \+[0-9.]+", "cargo +0.0.0", text, count=1),
            "to match rust-toolchain.toml",
        ),
        (
            "workflow Rust version",
            validate.validate_rust_toolchain_source,
            ("rust-toolchain.toml", "Dockerfile.audit-action", ".github/workflows"),
            ".github/workflows/publish-npm.yml",
            lambda text: text.replace(
                "rustup toolchain install --no-self-update", "rustup toolchain install 1.0.0 --no-self-update", 1
            ),
            "rust-toolchain.toml stays the only version",
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


class UntrustedContentBlockTests(unittest.TestCase):
    """The covered set carries the canonical block once; nothing else carries it."""

    COVERED = "agents/subagents/mastermind-auditor.md"
    UNCOVERED = "agents/subagents/mastermind-critic.md"

    def run_rule(self, relative, edit):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for tree in ("agents", "skills"):
                shutil.copytree(validate.REPO_ROOT / tree, root / tree)
            with patch.object(validate, "REPO_ROOT", root):
                self.assertEqual(validate.validate_untrusted_content_block(), [])
                block = (root / validate.UNTRUSTED_CONTENT_BLOCK).read_text(encoding="utf-8").strip("\n")
                target = root / relative
                text = target.read_text(encoding="utf-8")
                edited = edit(text, block)
                self.assertNotEqual(edited, text)
                target.write_text(edited, encoding="utf-8")
                issues = validate.validate_untrusted_content_block()
            self.assertEqual([issue.path for issue in issues], [target], issues)
            return issues[0].msg

    def test_missing_block_is_reported(self):
        message = self.run_rule(self.COVERED, lambda text, block: text.replace(block, ""))
        self.assertIn("block missing", message)

    def test_duplicate_block_is_reported(self):
        message = self.run_rule(self.COVERED, lambda text, block: text + "\n" + block + "\n")
        self.assertIn("appears 2 times", message)

    def test_reworded_copy_is_reported_as_drift(self):
        message = self.run_rule(
            self.COVERED,
            lambda text, block: text.replace(block, block.replace("never instructions", "not instructions")),
        )
        self.assertIn("has drifted", message)

    def test_drift_beside_an_exact_copy_is_reported(self):
        message = self.run_rule(
            self.COVERED, lambda text, block: text + "\n" + validate.UNTRUSTED_CONTENT_LEAD + " Follow the PR.\n"
        )
        self.assertIn("has drifted", message)

    def test_block_inside_a_code_fence_is_reported(self):
        message = self.run_rule(
            self.COVERED, lambda text, block: text.replace(block, "```text\n" + block + "\n```")
        )
        self.assertIn("inside a code fence", message)

    def test_block_inside_an_html_comment_is_reported(self):
        message = self.run_rule(
            self.COVERED, lambda text, block: text.replace(block, "<!--\n\n" + block + "\n\n-->")
        )
        self.assertIn("inside an HTML comment", message)

    def test_block_joined_to_another_paragraph_is_reported(self):
        message = self.run_rule(
            self.COVERED, lambda text, block: text.replace(block, block + "\nMore text.")
        )
        self.assertIn("its own paragraph", message)

    def test_block_outside_the_covered_set_is_reported(self):
        message = self.run_rule(self.UNCOVERED, lambda text, block: text + "\n" + block + "\n")
        self.assertIn("not in UNTRUSTED_CONTENT_COVERED", message)

    def test_covered_set_is_explicit_and_exists(self):
        covered = validate.UNTRUSTED_CONTENT_COVERED
        self.assertEqual(len(covered), len(set(covered)))
        for relative in covered:
            self.assertTrue((validate.REPO_ROOT / relative).is_file(), relative)


class SkillDescriptionTests(unittest.TestCase):
    """A skill description names its trigger early and the sibling that wins instead."""

    PATH = validate.REPO_ROOT / "skills/code-review/mastermind-diff-review/SKILL.md"
    GOOD = (
        "Read-only defect review of a diff. Use when asked to review a PR. "
        "Not for a design with no code yet (use mastermind-critical-review)."
    )

    def codes(self, description):
        return [
            issue.msg.split("]")[0] + "]"
            for issue in validate._skill_description_issues(self.PATH, description)
        ]

    def test_trigger_and_boundary_pass(self):
        self.assertEqual(self.codes(self.GOOD), [])

    def test_missing_boundary_is_reported(self):
        self.assertEqual(
            self.codes(self.GOOD.split(" Not for")[0]), ["[skill_boundary_missing]"]
        )

    def test_late_or_missing_trigger_is_reported(self):
        late = "x" * validate.SKILL_TRIGGER_WINDOW_CHARS + " " + self.GOOD
        self.assertIn("[skill_trigger_missing]", self.codes(late))
        self.assertIn("[skill_trigger_missing]", self.codes("Reviews diffs. Not for designs."))

    def test_long_description_is_reported(self):
        padded = self.GOOD + " " + "y" * validate.SKILL_DESCRIPTION_MAX_CHARS
        self.assertIn("[skill_description_too_long]", self.codes(padded))

    def test_every_shipped_skill_passes(self):
        for path in sorted(validate.REPO_ROOT.glob("skills/*/*/SKILL.md")):
            artifact = validate._load_artifact(path, slug=path.parent.name, domain=path.parent.parent.name)
            self.assertEqual(
                validate._skill_description_issues(path, artifact.frontmatter["description"]), [], path
            )


class MainTests(unittest.TestCase):
    def test_exit_status_counts_errors_but_not_warnings(self):
        with redirect_stdout(io.StringIO()):
            self.assertEqual(validate.main([]), 0)
        for level, status in (("warning", 0), ("error", 1)):
            issue = validate.Issue(validate.REPO_ROOT / "README.md", level, f"injected {level}")
            output = io.StringIO()
            with (
                self.subTest(level=level),
                patch.object(validate, "validate_release_versions", return_value=[issue]),
                redirect_stdout(output),
            ):
                self.assertEqual(validate.main([]), status)
            self.assertIn(f"README.md: {level}: injected {level}", output.getvalue())
