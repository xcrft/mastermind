from pathlib import Path
import tempfile
import unittest

from evals import selection
from evals.behavior import runner

S = selection.Skill


def skill(name, trigger, boundary="Not for other work."):
    return S(name, f"Use when {trigger}. {boundary} Does the thing.", Path(f"/x/{name}/SKILL.md"))


def case(case_id, accept, reject, prompt="Do something realistic."):
    return selection.Case(case_id, prompt, frozenset(accept), frozenset(reject))


class ShippedCorpusTests(unittest.TestCase):
    def test_shipped_corpus_and_catalogue_pass(self):
        report = selection.check()
        self.assertEqual(report.problems, [])

    def test_every_core_skill_has_prompts_for_and_against_it(self):
        cases = selection.load_cases()
        for name in selection.core_skills():
            with self.subTest(skill=name):
                self.assertGreaterEqual(sum(name in c.accept for c in cases), selection.CORE_MIN_POSITIVE)
                self.assertGreaterEqual(sum(name in c.reject for c in cases), selection.CORE_MIN_NEGATIVE)

    def test_corpus_has_conflict_tie_and_none_cases(self):
        cases = selection.load_cases()
        self.assertTrue(any(c.conflict and not c.tie for c in cases))
        self.assertTrue(any(c.tie for c in cases))
        self.assertTrue(any(selection.NONE in c.accept for c in cases))

    def test_selection_suite_runs_without_tools_on_the_catalogue(self):
        self.assertIn("selection", runner.PROMPT_ONLY_SUITES)
        self.assertEqual(runner.isolated_cli_args("selection")[:3], ["--safe-mode", "--tools", ""])
        cfg = runner.SUITES["selection"]
        with tempfile.TemporaryDirectory() as tmp:
            copied, root = runner.snapshot_evaluation_targets("selection", cfg, [], Path(tmp))
            self.assertIsNone(root)
            self.assertEqual(Path(copied["catalogue"]).read_text(encoding="utf-8"), selection.CATALOGUE.read_text(encoding="utf-8"))
        self.assertRegex(runner.evaluation_target_digest("selection", cfg, []), r"^[0-9a-f]{64}$")


class CheckTests(unittest.TestCase):
    def setUp(self):
        self.skills = {
            "mastermind-alpha": skill("mastermind-alpha", "asked to review a diff", "Not for a design (use mastermind-beta)."),
            "mastermind-beta": skill("mastermind-beta", "a design needs stress testing"),
        }
        self.good = [
            case("sel-a1", {"mastermind-alpha"}, {"mastermind-beta"}),
            case("sel-b1", {"mastermind-beta"}, {"mastermind-alpha"}),
        ]

    def problems(self, cases=None, skills=None, core=frozenset()):
        return selection.check(skills or self.skills, self.good if cases is None else cases, core, catalogue=None).problems

    def test_clean_corpus_passes(self):
        self.assertEqual(self.problems(), [])

    def test_stale_catalogue_is_reported(self):
        with tempfile.NamedTemporaryFile("w", suffix=".md", dir=selection.REPO_ROOT, delete=False) as handle:
            handle.write("old")
        path = Path(handle.name)
        try:
            report = selection.check(self.skills, self.good, frozenset(), catalogue=path)
            self.assertTrue(any("is stale" in p for p in report.problems))
            path.write_text(selection.render_catalogue(self.skills), encoding="utf-8")
            self.assertEqual(selection.check(self.skills, self.good, frozenset(), catalogue=path).problems, [])
        finally:
            path.unlink()

    def test_unknown_leaked_and_unrejected_cases_are_reported(self):
        cases = self.good + [
            case("sel-x", {"mastermind-gone"}, {"mastermind-alpha"}),
            case("sel-y", {"mastermind-alpha"}, {"mastermind-beta"}, prompt="Use mastermind-alpha on this."),
            case("sel-z", {"mastermind-alpha"}, set()),
            case("sel-w", {"mastermind-alpha"}, {"mastermind-alpha"}),
        ]
        problems = "\n".join(self.problems(cases))
        self.assertIn("sel-x: mastermind-gone is not a shipped skill", problems)
        self.assertIn("sel-y: the prompt names mastermind-alpha", problems)
        self.assertIn("sel-z: name at least one near miss", problems)
        self.assertIn("sel-w: ['mastermind-alpha'] both expected and rejected", problems)

    def test_core_coverage_needs_two_prompts_each_way(self):
        problems = self.problems(core=frozenset({"mastermind-alpha"}))
        self.assertIn("mastermind-alpha: 1 prompt(s) expect it; need 2", problems)
        self.assertIn("mastermind-alpha: 1 prompt(s) reject it; need 2", problems)

    def test_conflict_needs_a_written_tie_break_unless_declared_a_tie(self):
        settled = case("sel-conflict-ok", {"mastermind-beta"}, {"mastermind-alpha"})
        unsettled = case("sel-conflict-bad", {"mastermind-alpha"}, {"mastermind-beta"})
        tie = case("sel-conflict-tie", {"mastermind-alpha", "mastermind-beta"}, {"mastermind-gamma"})
        skills = dict(self.skills, **{"mastermind-gamma": skill("mastermind-gamma", "something else")})
        cases = self.good + [settled, unsettled, tie, case("sel-g", {"mastermind-gamma"}, {"mastermind-alpha"})]
        report = selection.check(skills, cases, frozenset(), catalogue=None)
        self.assertEqual(
            [p for p in report.problems if p.startswith("sel-conflict")],
            ["sel-conflict-bad: no rejected skill's description says 'Not for ... (use mastermind-alpha)'; "
             "write the tie-break into a description or declare the case a tie"],
        )
        self.assertEqual([c.id for c in report.ties], ["sel-conflict-tie"])

    def test_three_skill_referral_loop_is_reported_but_a_pair_is_not(self):
        def ref(name, target, trigger):
            return skill(name, trigger, f"Not for x (use {target}).")
        skills = {
            "mastermind-a": ref("mastermind-a", "mastermind-b", "one"),
            "mastermind-b": ref("mastermind-b", "mastermind-c", "two"),
            "mastermind-c": ref("mastermind-c", "mastermind-a", "three"),
        }
        cases = [case(f"sel-{n}", {n}, {m}) for n, m in zip(skills, list(skills)[1:] + list(skills)[:1])]
        problems = selection.check(skills, cases, frozenset(), catalogue=None).problems
        self.assertIn("referral loop with no owner: mastermind-a -> mastermind-b -> mastermind-c -> mastermind-a", problems)
        self.assertEqual(self.problems(), [])

    def test_overlapping_triggers_need_a_separating_prompt_or_hand_off(self):
        skills = {
            "mastermind-tests-pick": skill("mastermind-tests-pick", "deciding which tests to run for a change"),
            "mastermind-tests-run": skill("mastermind-tests-run", "asked to run tests for a change"),
        }
        apart = [case("sel-p", {"mastermind-tests-pick"}, {"mastermind-alpha"}), case("sel-r", {"mastermind-tests-run"}, {"mastermind-alpha"})]
        skills_all = dict(self.skills, **skills)
        problems = selection.check(skills_all, self.good + apart, frozenset(), catalogue=None).problems
        self.assertTrue(any("mastermind-tests-pick and mastermind-tests-run share trigger words" in p for p in problems))
        together = apart + [case("sel-pr", {"mastermind-tests-pick"}, {"mastermind-tests-run"})]
        problems = selection.check(skills_all, self.good + together, frozenset(), catalogue=None).problems
        self.assertFalse(any("share trigger words" in p for p in problems))

    def test_name_prefixes_would_make_answers_ambiguous(self):
        skills = dict(self.skills, **{"mastermind-alpha-two": skill("mastermind-alpha-two", "other")})
        cases = self.good + [case("sel-a2", {"mastermind-alpha-two"}, {"mastermind-beta"})]
        problems = selection.check(skills, cases, frozenset(), catalogue=None).problems
        self.assertIn("skill names mastermind-alpha and mastermind-alpha-two share a prefix, so answer phrases cannot tell them apart", problems)

    def test_answers_must_use_the_fixed_prefix(self):
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl", dir=selection.REPO_ROOT, delete=False) as handle:
            handle.write('{"id":"sel-q","why":"w","input":{"prompt":"p"},"expect":{"contains":["mastermind-alpha"],"not_contains":["Skill: mastermind-beta"]}}\n')
        path = Path(handle.name)
        try:
            problems = []
            self.assertEqual(selection.load_cases(path, problems), [selection.Case("sel-q", "p", frozenset(), frozenset({"mastermind-beta"}))])
            self.assertEqual(problems, ["sel-q: expected phrase 'mastermind-alpha' must start with 'Skill: '"])
        finally:
            path.unlink()


if __name__ == "__main__":
    unittest.main()
