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
        "Use when asked to review a PR. "
        "Not for a design with no code yet (use mastermind-critical-review). "
        "Read-only defect review of a diff."
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

    def test_late_boundary_is_reported(self):
        late = (
            "Use when asked to review a PR. "
            + "x" * validate.SKILL_TRIGGER_WINDOW_CHARS
            + " Not for designs (use mastermind-critical-review)."
        )
        self.assertIn("[skill_boundary_missing]", self.codes(late))

    def test_long_description_is_reported(self):
        padded = self.GOOD + " " + "y" * validate.SKILL_DESCRIPTION_MAX_CHARS
        self.assertIn("[skill_description_too_long]", self.codes(padded))

    def test_every_shipped_skill_passes(self):
        for path in sorted(validate.REPO_ROOT.glob("skills/*/*/SKILL.md")):
            artifact = validate._load_artifact(path, slug=path.parent.name, domain=path.parent.parent.name)
            self.assertEqual(
                validate._skill_description_issues(path, artifact.frontmatter["description"]), [], path
            )


class SubagentCapabilityTests(unittest.TestCase):
    """A subagent's tools match the capability class its workflow metadata declares."""

    SUBAGENTS = validate.REPO_ROOT / "agents/subagents"

    def load(self, name):
        path = self.SUBAGENTS / f"{name}.md"
        return validate._load_artifact(path, slug=name, domain="subagents").frontmatter

    def codes(self, frontmatter, tools=None):
        tools = frontmatter.get("tools") if tools is None else tools
        return [
            message.split("]")[0] + "]"
            for message in validate._workflow_metadata_messages(
                frontmatter, required=True, runtime_tools=validate._runtime_tool_names(tools)
            )
        ]

    def test_critic_with_shell_and_web_fails(self):
        critic = self.load("mastermind-critic")
        tools = critic["tools"] + ", Bash, WebFetch"
        self.assertEqual(
            self.codes(critic, tools),
            ["[undeclared_capability]", "[undeclared_capability]", "[denied_capability]"],
        )

    def test_read_only_roles_cannot_edit_even_with_capabilities(self):
        auditor = self.load("mastermind-auditor")
        for tool in ("Edit", "Write", "NotebookEdit", "MultiEdit"):
            self.assertIn("[readonly_mutation_capability]", self.codes(auditor, auditor["tools"] + f", {tool}"))

    def test_shell_needs_a_declared_reason_for_writers_too(self):
        executor = self.load("mastermind-task-executor")
        without = {**executor, "workflow": {**executor["workflow"], "capabilities": {}}}
        self.assertEqual(self.codes(without), ["[undeclared_capability]"])
        vague = {**executor, "workflow": {**executor["workflow"], "capabilities": {"shell": "needed"}}}
        self.assertEqual(self.codes(vague), ["[workflow_capability_invalid]"])

    def test_stale_or_unknown_capabilities_are_reported(self):
        critic = self.load("mastermind-critic")
        stale = {**critic, "workflow": {**critic["workflow"], "capabilities": {"web": "reads the release notes it cites"}}}
        self.assertEqual(self.codes(stale), ["[unused_capability]"])
        unknown = {**critic, "workflow": {**critic["workflow"], "capabilities": {"network": "fetches pages it cites"}}}
        self.assertEqual(self.codes(unknown), ["[workflow_capability_invalid]"])

    def with_tools(self, name, tools, **workflow):
        frontmatter = self.load(name)
        return {**frontmatter, "tools": tools, "workflow": {**frontmatter["workflow"], **workflow}}

    def test_delegation_needs_a_declared_capability(self):
        # Reviewer counter-example: Agent and Task passed as plain tools.
        for tool in ("Agent", "Task", "Agent(mastermind-task-executor)"):
            with self.subTest(tool=tool):
                auditor = self.with_tools("mastermind-auditor", f"Read, Bash, {tool}")
                self.assertEqual(self.codes(auditor), ["[undeclared_capability]"])
                declared = self.with_tools(
                    "mastermind-auditor",
                    f"Read, Bash, {tool}",
                    capabilities={
                        "shell": "Runs git diff and the declared VERIFY commands.",
                        "delegate": "Starts the test auditor on the same baseline.",
                    },
                )
                self.assertEqual(self.codes(declared), [])

    def test_other_mcp_servers_need_a_declared_capability(self):
        auditor = self.with_tools("mastermind-auditor", "Read, Bash, mcp__github__get_issue, mcp__mmcg__mmcg_status")
        self.assertEqual(self.codes(auditor), ["[undeclared_capability]"])
        self.assertIn("mcp__github__get_issue", validate._workflow_metadata_messages(
            auditor, required=True, runtime_tools=validate._runtime_tool_names(auditor["tools"])
        )[0])

    def test_shell_output_tools_count_as_shell(self):
        for tool in ("BashOutput", "KillShell"):
            with self.subTest(tool=tool):
                critic = self.with_tools("mastermind-test-auditor", f"Read, {tool}", capabilities={})
                self.assertEqual(self.codes(critic), ["[undeclared_capability]"])

    def test_read_only_roles_cannot_ask_for_edit_or_bypass_modes(self):
        for mode in ("bypassPermissions", "acceptEdits"):
            with self.subTest(mode=mode):
                critic = {**self.load("mastermind-critic"), "permissionMode": mode}
                self.assertEqual(self.codes(critic), ["[readonly_permission_mode]"])
        plan = {**self.load("mastermind-critic"), "permissionMode": "plan"}
        self.assertEqual(self.codes(plan), [])

    def test_disallowed_tools_are_removed_before_the_class_check(self):
        auditor = {**self.load("mastermind-auditor"), "disallowedTools": "Bash"}
        self.assertEqual(self.codes(auditor), ["[unused_capability]"])
        critic = {**self.load("mastermind-critic"), "disallowedTools": ["Write"]}
        self.assertEqual(self.codes(critic, critic["tools"] + ", Write"), [])

    def test_weak_or_copied_reasons_are_rejected(self):
        for reason in ("x" * 40, "aaaa aaaa aaaa aaaa aaaa", "needs the shell for work", "shell shell shell shell shell"):
            with self.subTest(reason=reason):
                auditor = self.with_tools("mastermind-auditor", "Read, Bash", capabilities={"shell": reason})
                codes = self.codes(auditor)
                if reason == "needs the shell for work":
                    self.assertEqual(codes, [])
                else:
                    self.assertEqual(codes, ["[workflow_capability_invalid]"])
        copied = "Runs git diff and the declared VERIFY commands."
        auditor = self.with_tools("mastermind-auditor", "Read, Bash, WebFetch", capabilities={"shell": copied, "web": copied})
        self.assertEqual(self.codes(auditor), ["[workflow_capability_invalid]"])

    def test_denied_pairs_cannot_be_unlocked_by_a_reason(self):
        researcher = self.with_tools(
            "mastermind-researcher",
            self.load("mastermind-researcher")["tools"] + ", Bash",
            capabilities={"shell": "Runs git log to date the history it cites."},
        )
        self.assertEqual(self.codes(researcher), ["[denied_capability]"])
        for name in ("mastermind-comment-auditor", "mastermind-frontend-auditor"):
            with self.subTest(name=name):
                role = self.with_tools(
                    name,
                    self.load(name)["tools"] + ", Bash",
                    capabilities={"shell": "Runs git diff to scope the changed files."},
                )
                self.assertEqual(self.codes(role), ["[denied_capability]"])

    def test_writer_needs_an_edit_tool(self):
        collector = self.with_tools("mastermind-feedback-collector", "Read, Bash", mutability="writer")
        self.assertEqual(self.codes(collector), ["[writer_without_edit_tools]"])

    def test_every_shipped_subagent_matches_its_class(self):
        classes = {}
        for path in sorted(self.SUBAGENTS.glob("*.md")):
            frontmatter = self.load(path.stem)
            self.assertEqual(self.codes(frontmatter), [], path)
            classes[path.stem] = validate.workflow_capability_class(frontmatter["workflow"])
        self.assertEqual(
            classes,
            {
                "mastermind-auditor": "read-only-with-shell",
                "mastermind-comment-auditor": "read-only",
                "mastermind-critic": "read-only",
                "mastermind-feedback-collector": "read-only-with-shell",
                "mastermind-frontend-auditor": "read-only",
                "mastermind-investigator": "read-only-with-shell",
                "mastermind-prompt-refiner": "read-only",
                "mastermind-researcher": "read-only",
                "mastermind-security-auditor": "read-only-with-shell",
                "mastermind-task-executor": "writer-with-shell",
                "mastermind-test-auditor": "read-only-with-shell",
            },
        )


class SubagentDescriptionTests(unittest.TestCase):
    """A subagent description opens with its trigger and names the role to use instead."""

    PATH = validate.REPO_ROOT / "agents/subagents/mastermind-critic.md"
    GOOD = (
        "Use before a spec is written, to challenge a design. "
        "Not for a finished change (use mastermind-auditor). "
        "Read-only design critique."
    )

    def codes(self, description, activation="conditional"):
        return [
            issue.msg.split("]")[0] + "]"
            for issue in validate._subagent_description_issues(self.PATH, description, activation)
        ]

    def test_trigger_and_boundary_pass(self):
        self.assertEqual(self.codes(self.GOOD), [])

    def test_trigger_must_open_the_description(self):
        self.assertEqual(self.codes("Design critic. " + self.GOOD), ["[subagent_trigger_missing]"])
        self.assertEqual(self.codes("Independent critic. Not for code (use mastermind-auditor)."), ["[subagent_trigger_missing]"])

    def test_missing_or_late_boundary_is_reported(self):
        self.assertEqual(self.codes(self.GOOD.split(" Not for")[0]), ["[subagent_boundary_missing]"])
        late = "Use before a spec. " + "x" * validate.SKILL_TRIGGER_WINDOW_CHARS + " Not for code (use mastermind-auditor)."
        self.assertLessEqual(len(late), validate.SUBAGENT_DESCRIPTION_MAX_CHARS)
        self.assertEqual(self.codes(late), ["[subagent_boundary_missing]"])

    def test_long_description_is_reported(self):
        self.assertIn("[subagent_description_too_long]", self.codes(self.GOOD + " " + "y" * 300))

    def test_manual_role_must_say_only_when(self):
        self.assertEqual(self.codes(self.GOOD, activation="manual"), ["[subagent_manual_trigger]"])
        manual = "Use only when asked to rewrite a prompt. Not for handoffs (use mastermind-agent-handoff). Rewrites."
        self.assertEqual(self.codes(manual, activation="manual"), [])

    def test_proactive_wording_is_reported(self):
        self.assertEqual(
            self.codes("Use proactively after code changes. Not for designs (use mastermind-critic). Reviews."),
            ["[subagent_trigger_missing]", "[subagent_push_delegation]"],
        )

    def test_push_delegation_phrases_are_reported(self):
        for phrase in ("MUST BE USED for every change", "use PROACTIVELY on diffs", "always use it", "Use immediately after edits"):
            with self.subTest(phrase=phrase):
                description = f"Use when a change lands, {phrase}. Not for designs (use mastermind-critic). Reviews."
                self.assertEqual(self.codes(description), ["[subagent_push_delegation]"])

    def test_non_proactive_is_not_push_delegation(self):
        self.assertEqual(self.codes("Use when a change is non-proactive. Not for designs (use mastermind-critic). Reviews."), [])

    def test_boundary_must_name_what_to_use_instead(self):
        self.assertEqual(self.codes("Use when a change lands. Not for designs. Reviews."), ["[subagent_boundary_missing]"])
        before = "Use when a change lands (use mastermind-critic). Not for designs. Reviews."
        self.assertEqual(self.codes(before), ["[subagent_boundary_missing]"])

    def test_every_shipped_subagent_passes(self):
        for path in sorted(validate.REPO_ROOT.glob("agents/subagents/*.md")):
            artifact = validate._load_artifact(path, slug=path.stem, domain=None)
            activation = artifact.frontmatter["workflow"]["activation"]
            self.assertEqual(
                validate._subagent_description_issues(path, artifact.frontmatter["description"], activation), [], path
            )


class DescriptionReferralTests(unittest.TestCase):
    """`(use X)` must name something the install actually ships."""

    def artifact(self, path, name, description):
        return validate.Artifact(path=path, slug=name, domain=None, frontmatter={"description": description}, name=name)

    def test_referrals_are_read_from_use_clauses_only(self):
        self.assertEqual(
            validate.description_referrals(
                "Use when x. Not for y (use mastermind-a, mastermind-b or no-ai-slop-comments). Mentions mastermind-c."
            ),
            ["mastermind-a", "mastermind-b", "no-ai-slop-comments"],
        )

    def test_unknown_and_self_referrals_are_errors(self):
        root = validate.REPO_ROOT
        skill = self.artifact(root / "skills/x/mastermind-a/SKILL.md", "mastermind-a", "Not for y (use mastermind-a).")
        other = self.artifact(root / "skills/x/mastermind-b/SKILL.md", "mastermind-b", "Not for y (use mastermind-gone).")
        agent = self.artifact(root / "agents/subagents/mastermind-r.md", "mastermind-r", "Not for y (use mastermind-a).")
        to_agent = self.artifact(root / "skills/x/mastermind-c/SKILL.md", "mastermind-c", "Not for y (use mastermind-r).")
        codes = [i.msg.split("]")[0] + "]" for i in validate.validate_description_referrals([skill, other, agent, to_agent])]
        self.assertEqual(sorted(codes), ["[referral_self]", "[referral_unknown]", "[referral_unknown]"])

    def referral_codes(self, description, kind="skill"):
        root = validate.REPO_ROOT
        known = [
            self.artifact(root / "skills/x/mastermind-a/SKILL.md", "mastermind-a", "Use when a. Not for b (use mastermind-diff-review)."),
            self.artifact(root / "skills/x/mastermind-diff-review/SKILL.md", "mastermind-diff-review", "Use when d. Not for e (use mastermind-a)."),
        ]
        path = root / ("skills/x/mastermind-t/SKILL.md" if kind == "skill" else "agents/subagents/mastermind-t.md")
        issues = validate.validate_description_referrals(known + [self.artifact(path, "mastermind-t", description)])
        return sorted(i.msg.split("]")[0] + "]" for i in issues)

    def test_non_canonical_and_unknown_targets_are_reported(self):
        cases = {
            "Use when x. Not for y (Use mastermind-zzz).": ["[referral_noncanonical]", "[referral_unknown]"],
            "Use when x. Not for y (use Mastermind-Diff-Review).": ["[referral_noncanonical]"],
            "Use when x. Not for y (use diff-review).": ["[referral_noncanonical]"],
            "Use when x. Not for y, use mastermind-zzz instead.": ["[referral_outside_clause]"],
            "Use when x. Not for y (use mastermind-a (core) or mastermind-zzz).": ["[referral_unknown]"],
            "Use when x. Not for y (use mastermind-a or mastermind-diff-review).": [],
        }
        for description, expected in cases.items():
            with self.subTest(description=description):
                self.assertEqual(self.referral_codes(description), expected)

    def test_every_target_in_a_clause_is_read(self):
        self.assertEqual(
            validate.description_referrals("Use when x. Not for y (use mastermind-a (core) or mastermind-zzz), z (use mastermind-b)."),
            ["mastermind-a", "mastermind-zzz", "mastermind-b"],
        )
        clauses = validate.description_referral_clauses(
            "Use when q. Not for the live page (use mastermind-b) or the spec contract (use mastermind-a). x"
        )
        self.assertEqual([(c.clause, c.targets) for c in clauses], [("the live page", ("mastermind-b",)), ("or the spec contract", ("mastermind-a",))])

    def test_subagent_boundary_must_resolve(self):
        self.assertEqual(self.referral_codes("Use when x. Not for y (use mastermind-zzz). z", kind="subagent"), ["[referral_unknown]", "[subagent_boundary_unresolved]"])
        self.assertEqual(self.referral_codes("Use when x. Not for y (use mastermind-a). z", kind="subagent"), [])

    def test_shipped_referrals_resolve(self):
        self.assertEqual(validate.validate_description_referrals(list(validate.find_artifacts())), [])


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
