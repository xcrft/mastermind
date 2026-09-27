"""Local transport and evidence-accounting tests, never model-quality tests."""

from __future__ import annotations

import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

from evals import hook_intake as intake


# This executable is a protocol fixture, not an implementation of Rust prose
# admission. A real mmcg smoke run separately exercises the production seam.
NATIVE_FIXTURE = r'''
import json
from pathlib import Path
import subprocess
import sys

def unique(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("duplicate")
        result[key] = value
    return result

assert sys.argv[1:4] == ["miner", "hooks", "evaluate-refiner"]
source = json.loads(Path(sys.argv[4]).read_text())
processor = sys.argv[sys.argv.index("--processor") + 1]
arguments = [arg.split("=", 1)[1] for arg in sys.argv if arg.startswith("--processor-arg=")]
example = {"schema": 1, "intake_id": source["id"], "prompt_digest": source["prompt_digest"],
           "action": "passthrough", "workflow_intent": "ordinary", "intent_evidence": None,
           "refined_prompt": "<copy input.original exactly>", "questions": []}
request = {"schema": 1, "instructions": "Synthetic native fixture. No semantic-quality claim.",
           "input": source, "response_example": example}
process = subprocess.run([processor, *arguments], input=json.dumps(request, ensure_ascii=False).encode(),
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=25)
output = {"schema": 1, "status": "failed", "admission": False, "reason": "fixture_rejected", "elapsed_ms": 1}
try:
    response = json.loads(process.stdout, object_pairs_hook=unique)
    if process.returncode == 0 and isinstance(response, dict):
        accepted = (set(response) == set(example) and response.get("schema") == 1
                    and response.get("intake_id") == source["id"]
                    and response.get("prompt_digest") == source["prompt_digest"])
        if accepted or __FORGE_ADMISSION__:
            output = {"schema": 1, "status": "evaluated", "admission": False, "response": response, "elapsed_ms": 1}
except (ValueError, TypeError):
    pass
print(json.dumps(output, ensure_ascii=False))
'''

PROCESSOR_HEAD = r'''
import json
import os
from pathlib import Path
import sys
import time

assert os.environ.get("MASTERMIND_MINER") == "1"
assert "ANTHROPIC_API_KEY" not in os.environ
assert "CLAUDE_CODE_OAUTH_TOKEN" not in os.environ
assert "PRIVATE_TEST_VALUE" not in os.environ
request = json.load(sys.stdin)
source = request["input"]
response = {"schema": 1, "intake_id": source["id"], "prompt_digest": source["prompt_digest"],
            "action": "passthrough", "workflow_intent": "ordinary", "intent_evidence": None,
            "refined_prompt": source["original"], "questions": []}
'''


@unittest.skipUnless(os.name == "posix", "POSIX process supervision is required")
class HookIntakeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.binary = self.root / "synthetic-mmcg"
        self.processor = self.root / "synthetic-processor"
        self.cases = self.root / "cases.jsonl"
        self.source = self.root / "source.rs"
        self.source.write_text("// synthetic source binding\n")
        self.script(self.binary, NATIVE_FIXTURE.replace("__FORGE_ADMISSION__", "False"))
        self.script(self.processor, PROCESSOR_HEAD + '\nprint(json.dumps(response, ensure_ascii=False))\n')
        self.write_cases([self.case()])

    def script(self, path, source):
        path.write_text(f"#!{sys.executable}\n" + source)
        path.chmod(0o700)

    def case(self, identifier="synthetic-test-1", text="Explain this setting.", expected="ordinary", active=False):
        return {"id": identifier, "language": "en", "raw_prompt": text,
                "has_active_task": active, "expected_intent": expected,
                "why": "SYNTHETIC_LABEL_EXPLANATION_MUST_NOT_REACH_PROCESSOR"}

    def write_cases(self, cases):
        self.cases.write_bytes(b"".join(intake.encoded(case) + b"\n" for case in cases))

    def run_eval(self, **overrides):
        options = dict(binary=self.binary, processor=self.processor, processor_args=[],
                       output=self.root / "report", cases_path=self.cases,
                       timeout=3, source_files=[self.source])
        options.update(overrides)
        return intake.run_evaluation(**options)

    def artifact(self, name, attempt=1):
        return self.root / "report" / f"attempt-{attempt:04d}" / name

    def test_checked_in_corpus_is_strict_and_multilingual(self):
        body, cases = intake.load_cases(intake.CORPUS)
        self.assertEqual(len(cases), 40)
        self.assertEqual({row["language"] for row in cases}, {"ru", "en", "ru-en", "es", "fr", "de", "zh"})
        self.assertEqual({row["expected_intent"] for row in cases}, intake.INTENTS)
        self.assertTrue(body.endswith(b"\n"))

    def test_corpus_rejects_ambiguous_schema_and_impossible_binding(self):
        for mutation in (
            lambda case: dict(case, extra=True),
            lambda case: dict(case, has_active_task=1),
            lambda case: dict(case, expected_intent="continue_active"),
            lambda case: dict(case, raw_prompt="x" * (16 * 1024 + 1)),
        ):
            self.write_cases([mutation(self.case())])
            with self.assertRaises(ValueError):
                intake.load_cases(self.cases)
        self.write_cases([self.case(), self.case()])
        with self.assertRaises(ValueError):
            intake.load_cases(self.cases)
        self.cases.write_bytes(b'{"id":"one","id":"two"}\n')
        with self.assertRaises(ValueError):
            intake.load_cases(self.cases)
        for body in (b'{"x":NaN}', b'{"x":Infinity}', b'{} {}', b'"\xff"'):
            with self.assertRaises(ValueError):
                intake.strict_json(body)

    def test_real_external_process_retains_requests_labels_and_all_repetitions(self):
        self.write_cases([
            self.case(text="  Сохрани café и 条件.\r\n"),
            self.case("synthetic-test-2", "Explain the new request only.", active=True),
        ])
        literal = "--literal=$HOME 'quoted' `not-executed`"
        self.script(self.processor, PROCESSOR_HEAD + f"\nassert sys.argv[1] == {literal!r}\n" +
                    'print("fixture diagnostic", file=sys.stderr)\nprint(json.dumps(response, ensure_ascii=False))\n')
        with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "synthetic-do-not-forward", "CLAUDE_CODE_OAUTH_TOKEN": "synthetic-do-not-forward", "PRIVATE_TEST_VALUE": "not-for-children"}):
            report = self.run_eval(repetitions=2, processor_args=[literal])
        self.assertEqual(report["status"], "passed")
        self.assertEqual(report["metrics"]["planned_attempts"], 4)
        self.assertEqual(report["metrics"]["label_agreement"]["matched"], 4)
        self.assertEqual(report["metrics"]["independent_label_review"], "missing")
        self.assertEqual(report["metrics"]["real_world_semantic_quality"], "not_established")
        for index, row in enumerate(report["attempts"], 1):
            request_bytes = self.artifact("request.json", index).read_bytes()
            request = intake.strict_json(request_bytes)
            self.assertNotIn(b"SYNTHETIC_LABEL_EXPLANATION", request_bytes)
            self.assertNotIn("expected_intent", request)
            self.assertNotIn("why", request)
            self.assertNotIn("synthetic-test", request["input"]["id"])
            self.assertEqual(request["input"]["prompt_digest"], intake.digest(request["input"]["original"].encode()))
            self.assertEqual(row["retained"]["request.json"]["sha256"], intake.digest(request_bytes))
            self.assertEqual(self.artifact("processor.stderr", index).read_text(), "fixture diagnostic\n")
            self.assertFalse(row["admission"])
            self.assertTrue(self.artifact("started.json", index).is_file())
            self.assertTrue(self.artifact("result.json", index).is_file())
        self.assertTrue(intake.strict_json(self.artifact("request.json", 2).read_bytes())["input"]["active_task"])
        self.assertIsNone(intake.strict_json(self.artifact("request.json", 1).read_bytes())["input"]["active_task"])
        self.assertEqual((self.root / "report").stat().st_mode & 0o777, 0o700)
        self.assertEqual(self.artifact("processor.stdout").stat().st_mode & 0o777, 0o600)

    def test_label_disagreement_is_retained_without_becoming_protocol_failure(self):
        self.write_cases([self.case(text="Use Mastermind for this task.", expected="activate_mastermind")])
        report = self.run_eval()
        self.assertEqual(report["status"], "failed")
        row = report["attempts"][0]
        self.assertEqual(row["status"], "label_mismatch")
        self.assertEqual(row["protocol_status"], "admitted")
        self.assertEqual(report["metrics"]["label_agreement"]["mismatched"], 1)
        self.assertEqual(report["metrics"]["label_agreement"]["confusion"]["activate_mastermind"]["ordinary"], 1)
        self.assertEqual(report["metrics"]["action_distribution"], {"passthrough": 1})

    def test_production_rejection_keeps_malformed_raw_output_and_every_attempt(self):
        self.script(self.processor, PROCESSOR_HEAD + '\nprint("{invalid", flush=True)\n')
        report = self.run_eval(repetitions=2)
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["metrics"]["label_agreement"]["compared"], 0)
        self.assertEqual(report["metrics"]["wire_contract"]["invalid"], 2)
        for index, row in enumerate(report["attempts"], 1):
            self.assertEqual(row["status"], "production_rejected")
            self.assertEqual(self.artifact("processor.stdout", index).read_bytes(), b"{invalid\n")
            self.assertTrue(self.artifact("native.stdout", index).is_file())
            self.assertTrue(self.artifact("result.json", index).is_file())

    def test_duplicate_keys_are_not_silently_normalized(self):
        self.script(self.processor, PROCESSOR_HEAD + '\nbody=json.dumps(response)\nprint("{\\"schema\\":1," + body[1:])\n')
        report = self.run_eval()
        self.assertEqual(report["attempts"][0]["status"], "production_rejected")
        self.assertEqual(report["metrics"]["protocol"]["admitted"], 0)
        self.assertIn(b'"schema":1', self.artifact("processor.stdout").read_bytes())

    def test_forged_admission_cannot_hide_wrong_binding_or_extra_fields(self):
        self.script(self.binary, NATIVE_FIXTURE.replace("__FORGE_ADMISSION__", "True"))
        self.script(self.processor, PROCESSOR_HEAD + '\nresponse["intake_id"]="wrong"\nresponse["execute_now"]=True\nprint(json.dumps(response))\n')
        report = self.run_eval()
        self.assertEqual(report["attempts"][0]["status"], "evidence_invalid")
        self.assertEqual(report["metrics"]["protocol"]["admitted"], 0)
        self.assertEqual(report["metrics"]["label_agreement"]["not_comparable"], 1)

    def test_nonzero_exit_does_not_count_an_otherwise_valid_answer(self):
        self.script(self.processor, PROCESSOR_HEAD + '\nprint(json.dumps(response), flush=True)\nsys.exit(7)\n')
        report = self.run_eval()
        self.assertEqual(report["attempts"][0]["status"], "production_rejected")
        self.assertTrue(report["attempts"][0]["wire_contract_valid"])
        self.assertEqual(intake.strict_json(self.artifact("process.json").read_bytes())["returncode"], 7)
        self.assertEqual(report["metrics"]["label_agreement"]["compared"], 0)

    def test_timeout_retains_partial_stream_and_stop_reason(self):
        self.script(self.processor, PROCESSOR_HEAD + '\nprint("partial", flush=True)\ntime.sleep(30)\n')
        report = self.run_eval(timeout=1)
        process = intake.strict_json(self.artifact("process.json").read_bytes())
        self.assertEqual(process["stop_reason"], "timeout")
        self.assertEqual(report["attempts"][0]["processor_process"]["stop_reason"], "timeout")
        self.assertEqual(self.artifact("processor.stdout").read_bytes(), b"partial\n")
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["budgets"]["processor_timeout_seconds"], 0.75)

    def test_output_limit_retains_bounded_prefix_and_no_label_comparison(self):
        self.script(self.processor, PROCESSOR_HEAD + f'\nsys.stdout.write("x" * {intake.STDOUT_LIMIT + 1})\nsys.stdout.flush()\n')
        report = self.run_eval()
        self.assertEqual(self.artifact("processor.stdout").stat().st_size, intake.STDOUT_LIMIT)
        self.assertEqual(intake.strict_json(self.artifact("process.json").read_bytes())["stop_reason"], "output_limit")
        self.assertEqual(report["metrics"]["label_agreement"]["compared"], 0)

    def test_processor_change_revokes_the_attempt_and_prevents_another_launch(self):
        launches = self.root / "launches"
        self.script(self.processor, PROCESSOR_HEAD + f'''\nPath({str(launches)!r}).open("a").write("launch\\n")
Path(__file__).write_text(Path(__file__).read_text() + "\\n# changed\\n")
print(json.dumps(response))
''')
        report = self.run_eval(repetitions=2)
        self.assertEqual(report["attempts"][0]["status"], "input_changed")
        self.assertEqual(report["attempts"][1]["status"], "input_or_setup_error")
        self.assertEqual(launches.read_text(), "launch\n")
        self.assertEqual(report["metrics"]["protocol"]["admitted"], 0)

    def test_changed_declared_dependency_is_detected_even_if_executable_is_unchanged(self):
        dependency = self.root / "processor-data.json"
        dependency.write_text("{}\n")
        self.script(self.processor, PROCESSOR_HEAD + f'\nPath({str(dependency)!r}).write_text("changed")\nprint(json.dumps(response))\n')
        report = self.run_eval(processor_sources=[dependency])
        self.assertEqual(report["attempts"][0]["status"], "input_changed")
        self.assertEqual(report["metrics"]["label_agreement"]["compared"], 0)

    def test_corpus_and_source_changes_cannot_produce_a_pass(self):
        self.script(self.processor, PROCESSOR_HEAD + f'''\nPath({str(self.cases)!r}).write_bytes(Path({str(self.cases)!r}).read_bytes() + b"\\n")
Path({str(self.source)!r}).write_text("// changed source\\n")
print(json.dumps(response))
''')
        report = self.run_eval()
        self.assertFalse(report["source_unchanged"])
        self.assertFalse(report["corpus_unchanged"])
        self.assertEqual(report["status"], "failed")
        self.assertNotEqual((self.root / "report" / "corpus.jsonl").read_bytes(), self.cases.read_bytes())

    def test_new_directory_required_and_unsafe_evidence_reads_fail(self):
        existing = self.root / "existing"
        existing.mkdir()
        sentinel = existing / "keep"
        sentinel.write_text("original")
        with self.assertRaises(FileExistsError):
            self.run_eval(output=existing)
        self.assertEqual(sentinel.read_text(), "original")
        link = self.root / "linked.jsonl"
        link.symlink_to(self.cases)
        with self.assertRaises(OSError):
            intake.load_cases(link)
        fifo = self.root / "fifo"
        os.mkfifo(fifo)
        with self.assertRaises(ValueError):
            intake.read_regular(fifo, 1024)

    def test_credential_arguments_rejected_before_any_output_or_launch(self):
        for arguments in (["--api-key=synthetic"], ["--AUTH_TOKEN", "synthetic"], ["--password", "synthetic"]):
            with self.assertRaises(ValueError):
                self.run_eval(processor_args=arguments)
        self.assertFalse((self.root / "report").exists())

    def test_wire_invariants_cover_ask_passthrough_and_continuation(self):
        source = intake.input_record(self.case(), 1, self.root)
        valid = {"schema": 1, "intake_id": source["id"], "prompt_digest": source["prompt_digest"],
                 "action": "passthrough", "workflow_intent": "ordinary", "intent_evidence": None,
                 "refined_prompt": source["original"], "questions": []}
        self.assertEqual(intake.response_issues(valid, source), [])
        for override, issue in (
            ({"refined_prompt": "Changed original."}, "passthrough"),
            ({"workflow_intent": "unclear"}, "unclear_without_ask"),
            ({"workflow_intent": "continue_active", "intent_evidence": source["original"]}, "continuation_without_binding"),
            ({"workflow_intent": "activate_mastermind"}, "missing_intent_evidence"),
            ({"action": "ask", "refined_prompt": None, "questions": []}, "ask"),
        ):
            self.assertIn(issue, intake.response_issues(dict(valid, **override), source))
        asked = dict(valid, action="ask", workflow_intent="unclear", refined_prompt=None, questions=["Which task?"])
        self.assertEqual(intake.response_issues(asked, source), [])


if __name__ == "__main__":
    unittest.main()
