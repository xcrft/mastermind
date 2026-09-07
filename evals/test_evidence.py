import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from evals.evidence import check_citations
from evals import runner


class CitationEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "src").mkdir()
        (self.root / "src/store.py").write_text(
            "# Storage\ndef load():\n    return 7\n\ndef save():\n    pass\n"
        )
        self.expected = [{"path": "src/store.py", "anchor": "return 7"}]

    def check(self, output, expected=None):
        return check_citations(
            output, self.expected if expected is None else expected, self.root
        )

    def test_real_anchor_must_be_inside_the_cited_range(self):
        for citation in (
            "`src/store.py:3`", "src/store.py:2-3",
            "**src/store.py:3**", "src/store.py:2 – 3",
            "src/store.py:3 — load returns 7.", "src/store.py:3 - load returns 7.",
            "[implementation](src/store.py:3)",
            f"[load]({self.root}/src/store.py:2-3)",
        ):
            with self.subTest(citation=citation):
                result = self.check(f"load returns seven. {citation}")
                self.assertEqual(result["issues"], [])
                self.assertEqual(result["matched"], 1)
                self.assertEqual(result["valid"], 1)

    def test_keywords_or_a_valid_but_wrong_line_do_not_satisfy_evidence(self):
        for output in ("src/store.py returns 7", "return 7 at src/store.py:5"):
            with self.subTest(output=output):
                result = self.check(output)
                self.assertEqual(result["matched"], 0)
                self.assertTrue(result["issues"])

    def test_missing_files_invalid_ranges_and_extra_invented_citations_fail(self):
        for bad in (
            "src/missing.py:3", "src/store.py:0", "src/store.py:90",
            "src/store.py:5-2", "src/store.py:2-200",
            "src/store.py:3 - 9999", "src/store.py:3—9999",
            "`Dockerfile:10`",
        ):
            with self.subTest(bad=bad):
                result = self.check(f"src/store.py:3 and {bad}")
                self.assertEqual(result["matched"], 1)
                self.assertTrue(result["issues"])
                self.assertLess(result["valid"], result["total"])

    def test_code_fences_quotes_and_comments_are_not_answer_citations(self):
        for output in (
            "```markdown\nsrc/store.py:3\n```",
            "~~~~\nsrc/store.py:3\n~~~~",
            "> src/store.py:3", "    src/store.py:3",
            "> Quoted example:\nsrc/store.py:3",
            "<!-- src/store.py:3 -->",
        ):
            with self.subTest(output=output):
                self.assertEqual(self.check(output)["matched"], 0)
        self.assertEqual(
            self.check("```\nsrc/missing.py:1\n```\nsrc/store.py:3")["issues"], []
        )
        self.assertEqual(self.check(
            "src/store.py:3\n\n> Quoted example:\nsrc/missing.py:1"
        )["issues"], [])

    def test_timestamps_and_web_urls_are_not_source_citations(self):
        result = self.check(
            "Checked at 12:34 via http://localhost:8080. TTL:900. `src/store.py:3`"
        )
        self.assertEqual(result["issues"], [])
        self.assertEqual(result["total"], 1)

    def test_delimited_paths_with_spaces_are_supported(self):
        (self.root / "src/store copy.py").write_text("return 7\n")
        expected = [{"path": "src/store copy.py", "anchor": "return 7"}]
        for citation in (
            "`src/store copy.py:1`",
            f"[source](<{self.root}/src/store copy.py:1>)",
        ):
            with self.subTest(citation=citation):
                self.assertEqual(self.check(citation, expected)["issues"], [])

    def test_citations_cannot_escape_the_fixture_or_follow_an_outside_symlink(self):
        with tempfile.TemporaryDirectory() as outside:
            external = Path(outside) / "private.py"
            external.write_text("return 7\n")
            (self.root / "src/link.py").symlink_to(external)
            for bad in (str(external) + ":1", "../private.py:1", "src/link.py:1"):
                with self.subTest(bad=bad):
                    result = self.check(f"src/store.py:3 {bad}")
                    self.assertTrue(result["issues"])
                    self.assertEqual(result["valid"], 1)

    def test_bad_or_ambiguous_expectations_fail_instead_of_passing_vacuously(self):
        for expected in (
            [], {}, [None], [{"path": "src/store.py", "anchor": ""}],
            [{"path": "src/store.py", "anchor": "absent"}],
            [{"path": "src/store.py", "anchor": "def "}],
            [{"path": "../outside.py", "anchor": "return 7"}],
            [{"path": str(self.root / "src/store.py"), "anchor": "return 7"}],
            self.expected * 2,
        ):
            with self.subTest(expected=expected):
                self.assertTrue(self.check("src/store.py:3", expected)["issues"])
        self.assertTrue(check_citations("src/store.py:3", self.expected, None)["issues"])

    def test_whole_file_citations_cannot_game_anchor_recall(self):
        (self.root / "long.md").write_text("\n".join(f"fact {i}" for i in range(80)))
        expected = [{"path": "long.md", "anchor": "fact 79"}]
        self.assertTrue(self.check("long.md:1-80", expected)["issues"])
        self.assertEqual(self.check("long.md:80", expected)["issues"], [])

    def test_duplicate_citations_do_not_inflate_the_score(self):
        result = self.check("src/store.py:3 and `src/store.py:3`")
        self.assertEqual(result["matched"], 1)
        self.assertEqual(result["total"], 1)

    def test_shipped_research_cases_have_unique_resolvable_source_anchors(self):
        cases = [json.loads(line) for line in runner.SUITES["researcher"]["cases"].read_text().splitlines()]
        ids = [case["id"] for case in cases]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertIsNotNone(runner.case_definition_digest("researcher", ids))
        for case in cases:
            expected = case["expect"].get("citations")
            if expected is None:
                continue
            root = runner.FIXTURES_DIR / case["fixture"] / "changes" / case["after_ref"]
            citations = []
            for entry in expected:
                lines = (root / entry["path"]).read_text().splitlines()
                matches = [i for i, line in enumerate(lines, 1) if entry["anchor"] in line]
                self.assertEqual(len(matches), 1, (case["id"], entry))
                citations.append(f"`{entry['path']}:{matches[0]}`")
            with self.subTest(case=case["id"]):
                self.assertEqual(check_citations("\n".join(citations), expected, root)["issues"], [])

    def test_public_evaluator_rejects_correct_keywords_with_wrong_evidence(self):
        case = {
            "id": "source-evidence", "fixture": "unused", "baseline_ref": "baseline",
            "after_ref": "current", "allow_no_mmcg": True, "input": {},
            "expect": {"contains": ["returns 7"], "citations": self.expected},
        }
        for line, passed in ((3, True), (5, False)):
            payload = {
                "type": "result", "result": f"load returns 7. `src/store.py:{line}`",
                "duration_ms": 1, "duration_api_ms": 1, "num_turns": 1,
                "total_cost_usd": 0, "modelUsage": {"test-model": {}},
                "usage": {
                    "input_tokens": 1, "output_tokens": 1,
                    "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0,
                },
            }
            events = [
                {"type": "system", "subtype": "init", "model": "test-model"}, payload,
            ]
            process = subprocess.CompletedProcess(
                [], 0, "\n".join(json.dumps(event) for event in events), ""
            )
            with (
                self.subTest(line=line),
                patch.object(runner, "setup_fixture", return_value=self.root),
                patch.object(runner.subprocess, "run", return_value=process),
            ):
                result = runner.evaluate_case(
                    "test-model", "researcher", runner.SUITES["researcher"],
                    case, keep_fixtures=True,
                )
                self.assertEqual(result.passed, passed, result.reasons)
                checks = runner.result_report(result)["citation_checks"]
                self.assertEqual(checks["expected"], 1)
                self.assertEqual(checks["matched"], int(passed))


if __name__ == "__main__":
    unittest.main()
