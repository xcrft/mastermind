from dataclasses import replace
from pathlib import Path
import tempfile
import unittest

from evals import selection
from evals.behavior import runner

S = selection.Skill


def skill(name, trigger, boundary="Not for other work.", kind="skill"):
    return S(name, f"Use when {trigger}. {boundary} Does the thing.", Path(f"/x/{name}/SKILL.md"), kind)


def case(case_id, accept, reject, prompt, **extra):
    return selection.Case(case_id, prompt, frozenset(accept), frozenset(reject), **extra)


def shipped():
    return {c.id: c for c in selection.load_cases()}


def problems_with(changed):
    """Problems for the shipped corpus with some cases replaced or added."""
    cases = shipped()
    for item in changed:
        cases[item.id] = item
    return selection.check(cases=list(cases.values()), catalogue=None).problems


def for_case(problems, case_id):
    return [p for p in problems if p.startswith(f"{case_id}:")]


class ShippedCorpusTests(unittest.TestCase):
    def test_shipped_corpus_and_catalogue_pass(self):
        self.assertEqual(selection.check().problems, [])

    def test_every_core_skill_has_prompts_for_and_against_it(self):
        cases = selection.load_cases()
        for name in selection.core_skills():
            with self.subTest(skill=name):
                self.assertGreaterEqual(sum(name in c.accept for c in cases), selection.CORE_MIN_POSITIVE)
                self.assertGreaterEqual(sum(name in c.reject for c in cases), selection.CORE_MIN_NEGATIVE)

    def test_corpus_has_conflict_and_none_cases_and_no_open_ties(self):
        cases = selection.load_cases()
        self.assertTrue(any(c.conflict and not c.tie for c in cases))
        self.assertTrue(any(selection.NONE in c.accept for c in cases))
        self.assertEqual([c.id for c in cases if c.tie], [])

    def test_the_reviewed_copies_have_paraphrases(self):
        cases = shipped()
        for original in (
            "sel-style-deep",
            "sel-test-impact-order",
            "sel-verify-change-ready",
            "sel-conflict-scanner-over-upgrade",
            "sel-audit-attestation-verify",
            "sel-frontend-audit-tokens",
            "sel-task-executor-path",
            "sel-project-map-hotspots",
            "sel-prompt-refiner-rewrite",
        ):
            with self.subTest(case=original):
                paraphrase = cases[original + selection.PARAPHRASE_SUFFIX]
                self.assertEqual(paraphrase.accept, cases[original].accept)

    def test_session_preferences_expect_the_feedback_collector_subagent(self):
        case = shipped()["sel-agent-feedback-collector-preferences"]
        self.assertEqual(case.accept, {"agent:mastermind-feedback-collector"})
        catalogue = selection.CATALOGUE.read_text(encoding="utf-8")
        self.assertIn("## Subagents", catalogue)
        self.assertIn("- `mastermind-feedback-collector`:", catalogue)
        self.assertIn("`Agent: <name>`", catalogue)

    def test_weak_one_step_prompt_was_replaced(self):
        self.assertNotEqual(shipped()["sel-codegraph-callers"].prompt, "Who calls parse_config right now?")

    def test_selection_suite_runs_without_tools_on_the_catalogue(self):
        self.assertIn("selection", runner.PROMPT_ONLY_SUITES)
        self.assertEqual(runner.isolated_cli_args("selection")[:3], ["--safe-mode", "--tools", ""])
        cfg = runner.SUITES["selection"]
        with tempfile.TemporaryDirectory() as tmp:
            copied, root = runner.snapshot_evaluation_targets("selection", cfg, [], Path(tmp))
            self.assertIsNone(root)
            self.assertEqual(Path(copied["catalogue"]).read_text(encoding="utf-8"), selection.CATALOGUE.read_text(encoding="utf-8"))
        self.assertRegex(runner.evaluation_target_digest("selection", cfg, []), r"^[0-9a-f]{64}$")

    def test_held_out_split_is_fixed_and_about_forty_percent(self):
        self.assertEqual(runner.selection_split("sel-style-deep"), runner.selection_split("sel-style-deep"))
        report = selection.check(catalogue=None)
        total = sum(report.splits.values())
        self.assertGreaterEqual(report.splits["held-out"] / total, selection.HELD_OUT_MIN_SHARE)
        self.assertLessEqual(report.splits["held-out"] / total, selection.HELD_OUT_MAX_SHARE)
        self.assertGreater(report.splits["train"], report.splits["held-out"])


class ReviewerCounterExampleTests(unittest.TestCase):
    """Each test replays a counter-example from the review of the first version."""

    def test_wrong_label_on_a_none_case_fails(self):
        convert = shipped()["sel-none-convert"]
        wrong = replace(convert, accept=frozenset({"mastermind-style-deep"}))
        self.assertIn("shares no description words with the expected choice", "\n".join(for_case(problems_with([wrong]), wrong.id)))

    def test_prompt_swapped_for_unrelated_text_fails(self):
        review = shipped()["sel-diff-review-pr"]
        wrong = replace(review, prompt=shipped()["sel-none-convert"].prompt)
        self.assertTrue(for_case(problems_with([wrong]), wrong.id))

    def test_gibberish_prompt_fails(self):
        ledger = shipped()["sel-conflict-ledger-over-verify"]
        wrong = replace(ledger, prompt="asdf qwer")
        self.assertIn("has 2 word(s)", "\n".join(for_case(problems_with([wrong]), wrong.id)))

    def test_flipping_critical_over_diff_fails(self):
        original = shipped()["sel-conflict-critical-over-diff"]
        flipped = replace(
            original,
            accept=frozenset({"mastermind-diff-review"}),
            reject=frozenset({"mastermind-critical-review", "agent:mastermind-critic"}),
        )
        found = "\n".join(for_case(problems_with([flipped]), flipped.id))
        self.assertIn("tie_break 'a design with no code yet' is neither", found)

    def test_every_conflict_case_fails_when_flipped(self):
        choices = selection.load_choices()
        for original in shipped().values():
            if not original.conflict or original.tie:
                continue
            (winner,) = selection.jobs(original.accept)
            for loser in sorted(original.reject):
                if choices[loser].kind != "skill":
                    continue
                with self.subTest(case=original.id, loser=loser):
                    companions = {k for k, c in choices.items() if c.kind == "agent" and c.job == loser}
                    flipped = replace(
                        original,
                        accept=frozenset({loser} | companions),
                        reject=frozenset({winner} | {k for k in original.accept if k != winner}),
                    )
                    self.assertIsNotNone(selection.tie_break_problem(flipped, choices))

    def test_short_and_cased_skill_names_leak(self):
        review = shipped()["sel-diff-review-pr"]
        for prompt in (
            "Run diff-review on PR 212",
            "Use Mastermind-Diff-Review on PR 212",
            "Can you do a diff review of PR 212 please",
        ):
            with self.subTest(prompt=prompt):
                found = "\n".join(for_case(problems_with([replace(review, prompt=prompt)]), review.id))
                self.assertIn("the prompt names", found)

    def test_adjacent_pair_needs_its_own_conflict_case(self):
        pair = {"mastermind-critical-review", "mastermind-diff-review"}
        cases = {
            k: c for k, c in shipped().items()
            if not (c.conflict and selection.jobs(c.accept) & pair and c.reject & pair)
        }
        self.assertIn("sel-conflict-critical-over-diff", shipped())
        self.assertNotIn("sel-conflict-critical-over-diff", cases)
        problems = selection.check(cases=list(cases.values()), catalogue=None).problems
        self.assertIn(
            "mastermind-critical-review and mastermind-diff-review are known neighbours; "
            "add a `sel-conflict-` case that expects one and rejects the other",
            problems,
        )

    def test_reviewed_adjacent_pairs_are_listed(self):
        for left, right in (
            ("investigation-ledger", "verify-change"),
            ("critical-review", "diff-review"),
            ("codegraph-research", "project-history"),
            ("project-map", "codegraph-research"),
            ("task-planning", "task-executor"),
            ("architecture-review", "critical-review"),
            ("dependency-upgrade", "scanner-alert-response"),
            ("diff-review", "agent-security-review"),
        ):
            self.assertIn(frozenset((f"mastermind-{left}", f"mastermind-{right}")), selection.ADJACENT_PAIRS)
        self.assertIn(frozenset(("mastermind-comment-audit", "no-ai-slop-comments")), selection.ADJACENT_PAIRS)


class CheckTests(unittest.TestCase):
    def setUp(self):
        self.skills = {
            "mastermind-alpha": skill("mastermind-alpha", "asked to review a diff for broken callers", "Not for a design with no code yet (use mastermind-beta)."),
            "mastermind-beta": skill("mastermind-beta", "a design needs stress testing before approval", "Not for a code diff (use mastermind-alpha)."),
            "mastermind-gamma": skill("mastermind-gamma", "a dashboard chart renders blank"),
        }
        self.good = [
            case("sel-a1", {"mastermind-alpha"}, {"mastermind-beta"}, "Please review this diff for broken callers."),
            case("sel-b1", {"mastermind-beta"}, {"mastermind-alpha"}, "Stress-test the design before my approval."),
            case("sel-g1", {"mastermind-gamma"}, {"mastermind-alpha"}, "The dashboard chart renders blank for me."),
            case("sel-g2", {"mastermind-alpha"}, {"mastermind-gamma"}, "Can you review the diff of this branch?"),
        ]

    def check(self, cases=None, skills=None, core=frozenset(), adjacent=frozenset(), corpus_gates=False):
        return selection.check(
            skills or self.skills, self.good if cases is None else cases, core,
            catalogue=None, adjacent=adjacent, corpus_gates=corpus_gates,
        )

    def problems(self, cases=None, **kwargs):
        return self.check(cases, **kwargs).problems

    def test_clean_corpus_passes(self):
        self.assertEqual(self.problems(), [])

    def test_stale_catalogue_is_reported(self):
        with tempfile.NamedTemporaryFile("w", suffix=".md", dir=selection.REPO_ROOT, delete=False) as handle:
            handle.write("old")
        path = Path(handle.name)
        try:
            report = selection.check(self.skills, self.good, frozenset(), catalogue=path, adjacent=frozenset(), corpus_gates=False)
            self.assertTrue(any("is stale" in p for p in report.problems))
            path.write_text(selection.render_catalogue(self.skills), encoding="utf-8")
            self.assertEqual(selection.check(self.skills, self.good, frozenset(), catalogue=path, adjacent=frozenset(), corpus_gates=False).problems, [])
        finally:
            path.unlink()

    def test_unknown_leaked_and_unrejected_cases_are_reported(self):
        cases = self.good + [
            case("sel-x", {"mastermind-gone"}, {"mastermind-alpha"}, "Please review this diff for broken callers."),
            case("sel-y", {"mastermind-alpha"}, {"mastermind-beta"}, "Use Mastermind-Alpha on this diff please."),
            case("sel-z", {"mastermind-alpha"}, set(), "Please review this diff for broken callers."),
            case("sel-w", {"mastermind-alpha"}, {"mastermind-alpha"}, "Please review this diff for broken callers."),
        ]
        problems = "\n".join(self.problems(cases))
        self.assertIn("sel-x: mastermind-gone is not a shipped skill", problems)
        self.assertIn("sel-y: the prompt names 'mastermind-alpha'", problems)
        self.assertIn("sel-z: name at least one near miss", problems)
        self.assertIn("sel-w: ['mastermind-alpha'] both expected and rejected", problems)

    def test_name_leak_matches_short_names_and_spaces_but_not_other_words(self):
        self.assertEqual(selection.leaked_name("Run diff-review on PR 212", "mastermind-diff-review"), "diff-review")
        self.assertEqual(selection.leaked_name("Use Mastermind-Diff-Review here", "mastermind-diff-review"), "mastermind-diff-review")
        self.assertEqual(selection.leaked_name("a quick Diff Review please", "mastermind-diff-review"), "diff review")
        self.assertEqual(selection.leaked_name("ask the Feedback-Collector", "agent:mastermind-feedback-collector"), "feedback-collector")
        self.assertIsNone(selection.leaked_name("review the diffs of PR 212", "mastermind-diff-review"))
        self.assertIsNone(selection.leaked_name("pre-diff-review notes", "mastermind-diff-review"))

    def test_lexical_gate_needs_the_label_to_win_on_words_or_a_reviewed_exception(self):
        wrong = case("sel-a2", {"mastermind-gamma"}, {"mastermind-alpha"}, "Please review this diff for broken callers on the dashboard.")
        self.assertIn("not above a rejected one", "\n".join(self.problems(self.good + [wrong])))
        excused = replace(wrong, lexical_exception="Reviewed: the chart breaks only in diffs.")
        self.assertFalse(any(p.startswith("sel-a2") for p in self.problems(self.good + [excused])))
        needless = replace(self.good[0], lexical_exception="Not needed.")
        self.assertIn("sel-a1: `lexical_exception` is not needed", "\n".join(self.problems([needless] + self.good[1:])))

    def test_none_case_fails_when_a_choice_fits_its_words(self):
        none = case("sel-n1", {"none"}, {"mastermind-gamma"}, "Please review this diff for broken callers.")
        self.assertIn("expects none, but it shares words with mastermind-alpha", "\n".join(self.problems(self.good + [none])))
        fine = case("sel-n2", {"none"}, {"mastermind-gamma"}, "Convert this JSON snippet to YAML.")
        self.assertEqual(self.problems(self.good + [fine]), [])

    def test_nonsense_prompts_fail(self):
        self.assertIsNotNone(selection.nonsense_problem("asdf qwer"))
        self.assertIsNotNone(selection.nonsense_problem("zxcv bnmq wert yuio pqrs"))
        self.assertIsNone(selection.nonsense_problem("Convert this JSON snippet to YAML."))

    def test_core_coverage_needs_two_prompts_each_way(self):
        problems = self.problems(core=frozenset({"mastermind-gamma"}))
        self.assertIn("mastermind-gamma: 1 prompt(s) expect it; need 2", problems)
        self.assertIn("mastermind-gamma: 1 prompt(s) reject it; need 2", problems)

    def test_core_skill_may_not_hand_off_outside_core(self):
        problems = self.problems(core=frozenset({"mastermind-alpha"}))
        self.assertIn("mastermind-alpha is in core but its description sends the host to mastermind-beta, which core does not install", problems)

    def conflict(self, accept, reject, prompt, tie_break):
        return case("sel-conflict-x", accept, reject, prompt, tie_break=tie_break)

    def test_conflict_cites_the_losers_clause_for_the_winner(self):
        settled = self.conflict({"mastermind-beta"}, {"mastermind-alpha"}, "Stress-test my design. No code is written yet.", "a design with no code yet")
        self.assertEqual(self.problems(self.good + [settled]), [])
        missing = replace(settled, tie_break=None)
        self.assertIn("needs a `tie_break`", "\n".join(self.problems(self.good + [missing])))

    def test_flipped_conflict_fails_even_when_both_descriptions_name_each_other(self):
        flipped = self.conflict({"mastermind-alpha"}, {"mastermind-beta"}, "Stress-test my design. No code is written yet.", "a design with no code yet")
        found = "\n".join(self.problems(self.good + [flipped]))
        self.assertIn("is neither a rejected choice's 'Not for ... (use mastermind-alpha)' clause", found)

    def test_winner_must_not_send_the_cited_trigger_back(self):
        skills = dict(self.skills)
        skills["mastermind-beta"] = skill("mastermind-beta", "a design needs stress testing", "Not for a design with no code yet (use mastermind-alpha).")
        settled = self.conflict({"mastermind-beta"}, {"mastermind-alpha"}, "Stress-test my design. No code is written yet.", "a design with no code yet")
        self.assertIn("sends the cited trigger back", "\n".join(self.problems(self.good + [settled], skills=skills)))

    def test_prompt_must_match_the_cited_clause_better_than_the_reverse_one(self):
        both = self.conflict({"mastermind-beta"}, {"mastermind-alpha"}, "Stress-test this code diff design.", "a design with no code yet")
        self.assertIn("as well as the cited clause", "\n".join(self.problems(self.good + [both])))

    def test_winner_trigger_can_settle_when_the_loser_cannot_name_it(self):
        skills = dict(self.skills)
        skills["mastermind-delta"] = skill("mastermind-delta", "a chart mockup arrives, even with a ticket")
        cases = self.good + [
            case("sel-d1", {"mastermind-delta"}, {"mastermind-gamma"}, "Here is a chart mockup to build."),
            case("sel-d2", {"mastermind-gamma"}, {"mastermind-delta"}, "The dashboard chart renders blank today."),
            self.conflict({"mastermind-delta"}, {"mastermind-gamma"}, "The chart mockup came with a ticket.", "even with a ticket"),
        ]
        self.assertEqual(self.problems(cases, skills=skills), [])

    def test_three_skill_referral_loop_is_reported_but_a_pair_is_not(self):
        def ref(name, target, trigger):
            return skill(name, trigger, f"Not for x (use {target}).")
        skills = {
            "mastermind-a": ref("mastermind-a", "mastermind-b", "apples need peeling"),
            "mastermind-b": ref("mastermind-b", "mastermind-c", "bananas need slicing"),
            "mastermind-c": ref("mastermind-c", "mastermind-a", "cherries need pitting"),
        }
        cases = [
            case("sel-a", {"mastermind-a"}, {"mastermind-b"}, "Can you peel the apples for me?"),
            case("sel-b", {"mastermind-b"}, {"mastermind-c"}, "Can you slice the bananas for me?"),
            case("sel-c", {"mastermind-c"}, {"mastermind-a"}, "Can you pit the cherries for me?"),
        ]
        problems = selection.check(skills, cases, frozenset(), catalogue=None, adjacent=frozenset(), corpus_gates=False).problems
        self.assertIn("referral loop with no owner: mastermind-a -> mastermind-b -> mastermind-c -> mastermind-a", problems)
        self.assertEqual(self.problems(), [])

    def test_overlap_needs_a_separating_prompt_even_when_the_descriptions_link(self):
        skills = {
            "mastermind-tests-pick": skill("mastermind-tests-pick", "deciding which tests to run for a change", "Not for x (use mastermind-tests-run)."),
            "mastermind-tests-run": skill("mastermind-tests-run", "asked to run tests for a change"),
        }
        skills_all = dict(self.skills, **skills)
        apart = [
            case("sel-p", {"mastermind-tests-pick"}, {"mastermind-alpha"}, "Which tests should I be deciding to run?"),
            case("sel-r", {"mastermind-tests-run"}, {"mastermind-alpha"}, "Please run the tests that I asked for."),
        ]
        problems = self.problems(self.good + apart, skills=skills_all)
        self.assertTrue(any("mastermind-tests-pick and mastermind-tests-run share trigger words" in p for p in problems))
        together = apart + [case("sel-pr", {"mastermind-tests-pick"}, {"mastermind-tests-run"}, "Which tests should I be deciding to run?")]
        self.assertFalse(any("share trigger words" in p for p in self.problems(self.good + together, skills=skills_all)))

    def test_adjacent_pair_with_no_shared_words_needs_a_conflict_case(self):
        pair = frozenset({"mastermind-alpha", "mastermind-gamma"})
        problems = self.problems(adjacent=frozenset({pair}))
        self.assertIn("mastermind-alpha and mastermind-gamma are known neighbours; add a `sel-conflict-` case that expects one and rejects the other", problems)

    def test_subagent_companion_must_be_accepted_with_its_skill(self):
        skills = dict(self.skills)
        agent = S("mastermind-critic", "Use when a design needs a critic. Not for code (use mastermind-alpha). Critiques.", Path("/x/mastermind-critic.md"), "agent")
        skills[agent.key] = agent
        original = selection.COMPANIONS.copy()
        selection.COMPANIONS["mastermind-critic"] = "mastermind-beta"
        try:
            found = "\n".join(self.problems(skills=skills))
            self.assertIn("sel-b1: mastermind-critic does mastermind-beta's job, so expect Agent: mastermind-critic as well", found)
            fixed = [replace(c, accept=c.accept | {agent.key}) if c.id == "sel-b1" else c for c in self.good]
            self.assertEqual(self.problems(fixed, skills=skills), [])
            self.assertFalse(selection.Case("sel-t", "p", frozenset({"mastermind-beta", agent.key}), frozenset()).tie)
        finally:
            selection.COMPANIONS.clear()
            selection.COMPANIONS.update(original)

    def test_paraphrase_may_not_share_rare_words_with_its_label(self):
        para = case("sel-g1" + selection.PARAPHRASE_SUFFIX, {"mastermind-gamma"}, {"mastermind-alpha"}, "The dashboard graph shows nothing at all for me.", lexical_exception="x")
        self.assertIn("a paraphrase must share no rare words", "\n".join(self.problems(self.good + [para])))

    def test_leak_rate_above_the_limit_fails(self):
        leaky = [
            case(f"sel-l{i}", {"mastermind-gamma"}, {"mastermind-alpha"}, "The dashboard chart renders blank again.")
            for i in range(6)
        ]
        found = "\n".join(self.problems(self.good + leaky, corpus_gates=True))
        self.assertIn("share 2+ rare words only with the expected description", found)

    def test_answers_must_use_a_fixed_prefix(self):
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl", dir=selection.REPO_ROOT, delete=False) as handle:
            handle.write('{"id":"sel-q","why":"w","input":{"prompt":"p"},"expect":{"contains":["mastermind-alpha"],"not_contains":["Skill: mastermind-beta","Agent: mastermind-critic"]}}\n')
        path = Path(handle.name)
        try:
            problems = []
            self.assertEqual(
                selection.load_cases(path, problems),
                [selection.Case("sel-q", "p", frozenset(), frozenset({"mastermind-beta", "agent:mastermind-critic"}))],
            )
            self.assertEqual(problems, ["sel-q: expected phrase 'mastermind-alpha' must start with 'Skill: ' or 'Agent: '"])
        finally:
            path.unlink()


if __name__ == "__main__":
    unittest.main()
