"""Exercise the real trial transport with disposable Git/SQLite and fake adapters."""

import copy
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest.mock import patch

from evals import benchmark as bench
from evals.benchmark_process import run_bounded


CONTRACT = {"schema_version": "8", "extractor_contract_version": "mmcg-extractors-v6",
            "concept_normalization_version": "mmcg-concepts-v2"}
ADAPTER_HEADER = """import hashlib, json, os, pathlib, sys, time
request = json.load(sys.stdin)
root = pathlib.Path(request['source_root'])
(root.parent / 'adapter-called').write_text('yes')
def emit(event):
    print(json.dumps(event), flush=True)
def init(model=None):
    emit({'type': 'init', 'model': model or request['model'], 'adapter_version': 'fake-v1'})
def final(answer='Observed src/service.py:2.', **extra):
    result = {'type': 'result', 'answer': answer, 'model_error': False, 'turns': 1,
              'usage': {'input_tokens': 25, 'output_tokens': 12, 'cache_read_tokens': 0,
                        'cache_write_tokens': 0}, 'cost_usd': 0}
    result.update(extra)
    emit(result)
"""
INDEXER_BODY = """import hashlib, json, pathlib, sqlite3, sys
assert len(sys.argv) == 5 and sys.argv[1] == '--index' and sys.argv[3] == 'index'
index = pathlib.Path(sys.argv[2])
root = pathlib.Path(sys.argv[4]).resolve()
(root.parent / 'indexer-called').write_text('yes')
with sqlite3.connect(index) as db:
    db.executescript('CREATE TABLE symbols(id); CREATE TABLE edges(id); '
                     'CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT); '
                     'CREATE TABLE files(path TEXT PRIMARY KEY, content_sha256 TEXT);')
    meta = dict(CONTRACT, index_root=str(root))
    if MODE == 'wrong_root':
        meta['index_root'] = str(root.parent)
    if MODE == 'wrong_contract':
        meta['schema_version'] = '7'
    db.executemany('INSERT INTO meta VALUES (?, ?)', meta.items())
    if MODE != 'partial':
        sha = hashlib.sha256((root / 'src/service.py').read_bytes()).hexdigest()
        db.execute('INSERT INTO files VALUES (?, ?)',
                   ('src/service.py', '0' * 64 if MODE == 'wrong_hash' else sha))
if MODE == 'wal':
    index.with_name(index.name + '-wal').write_bytes(b'uncheckpointed')
if MODE == 'exit_failure':
    sys.exit(7)
"""


@unittest.skipUnless(os.name == "posix" and shutil.which("git"), "requires POSIX and Git")
class BenchmarkTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.repo = self.root / "original"
        self.repo.mkdir()
        self.env = bench.clean_environment(self.root)
        self.git("init", "-q", "-b", "fixture")
        self.git("config", "core.autocrlf", "false")
        self.instruction = "# Research\nRead source; cite evidence and unknowns.\n"
        for name, body in {
            "src/service.py": "def value():\n    return 7\n",
            "docs/guide.md": "The service returns a fixed value.\n",
            "evals/answer.json": '{"hidden": "ANSWER_KEY_CANARY"}\n',
            ".claude/settings.json": '{"private": "SETTINGS_CANARY"}\n',
            "AGENTS.md": "AGENT_CONFIG_CANARY\n",
            "skills/research/SKILL.md": self.instruction,
        }.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        self.git("add", "--all")
        self.git("commit", "-qm", "fixture with private evaluation data")
        self.revision = self.git("rev-parse", "HEAD").strip()
        self.task = {"id": "service-01", "revision": self.revision, "kind": "research",
                     "source_allowlist": ["src/service.py", "docs/guide.md"],
                     "question": "What does value return?", "output_contract": "Cite path:line."}
        self.rubric = {"task_id": self.task["id"], "required_fact": "HIDDEN_RUBRIC_CANARY"}
        self.config = {"model": "fixed-test-model-20260907", "tool_revision": self.revision,
                       "instruction_path": "skills/research/SKILL.md",
                       "limits": {"timeout_seconds": 3}}
        self.adapter()
        self.indexer()

    def git(self, *args, cwd=None):
        result = subprocess.run(
            [shutil.which("git"), "--no-replace-objects", "-c", "core.hooksPath=/dev/null",
             "-c", "commit.gpgsign=false", *args], cwd=cwd or self.repo, env=self.env,
            check=True, capture_output=True, text=True, timeout=10,
        )
        return result.stdout

    def executable(self, name, body):
        path = self.root / name
        path.write_text(f"#!{sys.executable}\n" + textwrap.dedent(body))
        path.chmod(0o755)
        return {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                "version": "fake-v1", "origin": "deterministic test fixture"}

    def adapter(self, body="init()\nfinal()\n"):
        self.config["adapter"] = self.executable("fake-adapter", ADAPTER_HEADER + textwrap.dedent(body))

    def indexer(self, mode="ok"):
        body = f"CONTRACT = {CONTRACT!r}\nMODE = {mode!r}\n" + INDEXER_BODY
        pin = self.executable("fake-mmcg", body)
        self.config["mmcg"] = dict(pin, source_revision=self.revision,
                                   index_contract=CONTRACT, indexed_files=["src/service.py"])

    def prepare(self, condition="source", **kwargs):
        return bench.prepare_trial(task=kwargs.get("task", self.task), rubric=self.rubric,
            config=kwargs.get("config", self.config), source_repo=self.repo, tool_repo=self.repo,
            output=self.root / "trials", condition=condition)

    def manifest(self, trial):
        return bench.load_json(trial / "manifest.json")

    def assert_setup_failure(self, trial, reason):
        self.assertEqual(self.manifest(trial)["status"], "setup_failed")
        result = bench.run_trial(trial)
        self.assertEqual(result["run_status"], {"state": "setup_error", "reason": reason})
        self.assertEqual(result["quality"]["status"], "not_evaluated")
        self.assertFalse((trial / "adapter-called").exists())

    def test_conditions_share_source_task_limits_and_rubric_but_only_third_indexes(self):
        trials = [self.prepare(condition) for condition in bench.CONDITIONS]
        manifests = [self.manifest(trial) for trial in trials]
        requests = [bench.load_json(trial / "request.json") for trial in trials]
        self.assertEqual({m["status"] for m in manifests}, {"prepared"})
        for field in ("common_sha256", "source_sha256", "rubric_sha256", "projection_revision"):
            self.assertEqual(len({m[field] for m in manifests}), 1, field)
        self.assertEqual(len({m["condition_sha256"] for m in manifests}), 3)
        self.assertEqual(len({m["request_sha256"] for m in manifests}), 3)
        self.assertEqual(requests[0]["portable_instruction"], "")
        self.assertEqual(requests[1]["portable_instruction"], self.instruction)
        self.assertEqual(requests[2]["portable_instruction"], self.instruction)
        for index, (trial, request) in enumerate(zip(trials, requests)):
            self.assertEqual(request["task"], self.task)
            self.assertEqual(request["model"], self.config["model"])
            self.assertEqual((trial / "indexer-called").exists(), index == 2)
            self.assertEqual("mmcg" in request["available_tools"], index == 2)
            self.assertEqual((trial / "index").exists(), index == 2)
            self.assertEqual(bench.run_trial(trial)["run_status"]["state"], "completed")

    def test_git_projection_has_no_original_history_or_omitted_objects(self):
        trial = self.prepare()
        source = trial / "source"
        self.assertEqual(self.git("rev-list", "--all", "--count", cwd=source).strip(), "1")
        self.assertEqual(set(self.git("ls-files", cwd=source).splitlines()), set(self.task["source_allowlist"]))
        hidden_blob = self.git("rev-parse", "HEAD:evals/answer.json").strip()
        result = subprocess.run([shutil.which("git"), "cat-file", "-e", hidden_blob],
                                cwd=source, env=self.env, capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        for path in source.rglob("*"):
            if path.is_file():
                body = path.read_bytes()
                for canary in (b"ANSWER_KEY_CANARY", b"SETTINGS_CANARY", b"AGENT_CONFIG_CANARY"):
                    self.assertNotIn(canary, body)

    def test_git_projection_keeps_ignored_files_and_attribute_sensitive_bytes(self):
        (self.repo / ".gitignore").write_text("*.py\n")
        (self.repo / ".gitattributes").write_text("*.py text eol=lf\n")
        raw = b"def value():\r\n    return 7\r\n"
        (self.repo / "src/service.py").write_bytes(raw)
        # Store raw fixture bytes rather than letting its attributes normalize them.
        oid = self.git("hash-object", "-w", "--no-filters", "src/service.py").strip()
        self.git("update-index", "--cacheinfo", "100644", oid, "src/service.py")
        self.git("add", ".gitignore", ".gitattributes")
        self.git("commit", "-qm", "raw source with ignore and attributes")
        task = dict(self.task, revision=self.git("rev-parse", "HEAD").strip(),
                    source_allowlist=["src/service.py", ".gitignore", ".gitattributes"])
        trial = self.prepare(task=task)
        self.assertEqual(self.manifest(trial)["status"], "prepared")
        self.assertEqual((trial / "source/src/service.py").read_bytes(), raw)
        self.assertEqual(set(self.git("ls-files", cwd=trial / "source").splitlines()), set(task["source_allowlist"]))
        stored = subprocess.run([shutil.which("git"), "show", "HEAD:src/service.py"],
                                 cwd=trial / "source", env=self.env, check=True,
                                 capture_output=True, timeout=10).stdout
        self.assertEqual(stored, raw)
        self.assertEqual(bench.run_trial(trial)["run_status"]["state"], "completed")

    def test_request_and_environment_do_not_inherit_hidden_inputs(self):
        self.adapter("""
            init()
            (root.parent / 'observed-env.json').write_text(json.dumps(dict(os.environ)))
            final(json.dumps(request))
        """)
        with patch.dict(os.environ, {"SECRET_CANARY": "secret", "GIT_DIR": "/wrong/git",
                                     "PYTHONPATH": "/wrong/python", "NODE_OPTIONS": "--inspect",
                                     "OPENAI_API_KEY": "not-implicitly-passed"}):
            trial = self.prepare()
            result = bench.run_trial(trial)
        self.assertEqual(result["run_status"]["state"], "completed")
        observed = bench.load_json(trial / "observed-env.json")
        for name in ("SECRET_CANARY", "GIT_DIR", "PYTHONPATH", "NODE_OPTIONS", "OPENAI_API_KEY"):
            self.assertNotIn(name, observed)
        self.assertEqual(observed["HOME"], str(trial / "home"))
        self.assertNotIn("HIDDEN_RUBRIC_CANARY", (trial / "answer.md").read_text())
        self.assertNotIn("rubric", bench.load_json(trial / "request.json"))

    def test_full_final_answer_is_retained_without_a_quality_score(self):
        answer = "Observed src/service.py:2.\n" + "полный ответ " * 700
        self.adapter(f"init()\nemit({{'type': 'trace', 'tool': 'source_read', 'path': 'src/service.py'}})\nfinal({answer!r})\n")
        trial = self.prepare()
        result = bench.run_trial(trial)
        self.assertEqual(result["run_status"]["state"], "completed")
        self.assertEqual((trial / "answer.md").read_text(), answer)
        self.assertEqual(result["answer"]["sha256"], hashlib.sha256(answer.encode()).hexdigest())
        self.assertEqual(result["answer"]["bytes"], len(answer.encode()))
        self.assertEqual(result["quality"], {"status": "review_pending", "score": None})
        self.assertFalse(result["comparability"]["eligible"])
        self.assertEqual(result["diagnostics"]["tools"], ["source_read"])
        self.assertEqual(len((trial / "trace.jsonl").read_bytes()), result["diagnostics"]["trace_bytes"])
        self.assertEqual(bench.load_json(trial / "result.json"), result)

    def test_runtime_pin_and_index_failures_prevent_model_invocation(self):
        for role in ("adapter", "mmcg"):
            with self.subTest(role=role):
                config = copy.deepcopy(self.config)
                config[role]["sha256"] = "0" * 64
                trial = self.prepare("portable_mmcg", config=config)
                self.assert_setup_failure(trial, f"{role}_mismatch")
                self.assertFalse((trial / "indexer-called").exists())
        config = copy.deepcopy(self.config)
        config["mmcg"]["source_revision"] = "1" * 40
        self.assert_setup_failure(self.prepare("portable_mmcg", config=config), "mmcg_revision_mismatch")
        config.pop("mmcg")
        self.assert_setup_failure(self.prepare("portable_mmcg", config=config), "mmcg_missing")

    def test_failed_partial_stale_or_uncheckpointed_indexes_never_run(self):
        expected = {"exit_failure": "index_setup_failed", "partial": "index_source_mismatch",
                    "wrong_root": "index_root_mismatch", "wrong_hash": "index_source_mismatch",
                    "wrong_contract": "index_contract_mismatch", "wal": "index_uncheckpointed"}
        for mode, reason in expected.items():
            with self.subTest(mode=mode):
                self.indexer(mode)
                trial = self.prepare("portable_mmcg")
                self.assertTrue((trial / "index/mmcg.db").is_file())
                self.assert_setup_failure(trial, reason)

    def test_unknown_indexed_paths_are_rejected_before_indexing(self):
        for paths in ([], ["omitted.py"], ["src/service.py"] * 2):
            with self.subTest(paths=paths):
                self.config["mmcg"]["indexed_files"] = paths
                trial = self.prepare("portable_mmcg")
                self.assert_setup_failure(trial, "index_scope_invalid")
                self.assertFalse((trial / "indexer-called").exists())

    def test_changed_runtime_after_preparation_prevents_invocation(self):
        trial = self.prepare()
        with Path(self.config["adapter"]["path"]).open("a") as handle:
            handle.write("# changed executable\n")
        result = bench.run_trial(trial)
        self.assertEqual(result["run_status"]["reason"], "adapter_mismatch")
        self.assertFalse((trial / "adapter-called").exists())

    def test_changed_source_or_private_key_prevents_invocation(self):
        for mutation, reason in (("bytes", "source_changed"), ("mode", "source_changed"),
                                 ("inventory", "source_changed"), ("rubric", "rubric_changed"),
                                 ("request", "request_changed")):
            with self.subTest(mutation=mutation):
                trial = self.prepare()
                if mutation == "bytes":
                    source = trial / "source/src/service.py"
                    source.chmod(0o644)
                    source.write_text("def value():\n    return 9\n")
                elif mutation == "mode":
                    (trial / "source/src/service.py").chmod(0o555)
                elif mutation == "inventory":
                    (trial / "source/unexpected.txt").write_text("new")
                else:
                    path = trial / (mutation + ".json")
                    value = bench.load_json(path)
                    value["added"] = "changed"
                    path.write_bytes(bench.canonical(value))
                result = bench.run_trial(trial)
                self.assertEqual(result["run_status"]["reason"], reason)
                self.assertFalse((trial / "adapter-called").exists())

    def test_mutation_during_adapter_retains_answer_and_marks_input_change(self):
        self.adapter("""
            init()
            target = root / 'src/service.py'
            target.chmod(0o644)
            target.write_text('changed source')
            final()
        """)
        trial = self.prepare()
        result = bench.run_trial(trial)
        self.assertEqual(result["run_status"], {"state": "input_changed", "reason": "source_changed"})
        self.assertEqual(result["quality"]["status"], "review_pending")
        self.assertIn("source_changed", result["comparability"]["reasons"])

    def test_changed_manifest_cannot_reuse_original_digests_and_request(self):
        for field, replacement in (("condition", "portable"), ("limits", {"timeout_seconds": 999}),
                                   ("source_files", None), ("adapter", [])):
            with self.subTest(field=field):
                trial = self.prepare()
                manifest = self.manifest(trial)
                if field == "limits":
                    manifest[field].update(replacement)
                else:
                    manifest[field] = replacement
                (trial / "manifest.json").write_bytes(bench.canonical(manifest))
                result = bench.run_trial(trial)
                self.assertEqual(result["run_status"]["state"], "setup_error")
                self.assertFalse((trial / "adapter-called").exists())

    def test_unexpected_empty_or_unreadable_directory_fails_inventory(self):
        for mode in (0o700, 0):
            with self.subTest(mode=mode):
                trial = self.prepare()
                unexpected = trial / "source/unexpected"
                unexpected.mkdir()
                unexpected.chmod(mode)
                try:
                    result = bench.run_trial(trial)
                    self.assertEqual(result["run_status"]["reason"], "source_changed")
                    self.assertFalse((trial / "adapter-called").exists())
                finally:
                    unexpected.chmod(0o700)

    def test_attempt_cannot_be_silently_retried(self):
        trial = self.prepare()
        first = bench.run_trial(trial)
        with self.assertRaisesRegex(bench.BenchmarkError, "already attempted"):
            bench.run_trial(trial)
        self.assertEqual(bench.load_json(trial / "result.json"), first)

    def test_missing_telemetry_and_exceeded_budget_do_not_grade_correctness(self):
        for extra in ("usage=None, turns=None", "turns=100, usage={'input_tokens': 2, 'output_tokens': 9000, 'cache_read_tokens': 0, 'cache_write_tokens': 0}"):
            with self.subTest(extra=extra):
                self.adapter(f"init()\nfinal({extra})\n")
                result = bench.run_trial(self.prepare())
                self.assertEqual(result["run_status"]["state"], "completed")
                self.assertEqual(result["quality"], {"status": "review_pending", "score": None})
                diagnostic = result["diagnostics"]["telemetry"]
                self.assertTrue(not diagnostic["complete"] or diagnostic["budget_exceeded"])

    def test_unavailable_reported_tool_is_diagnostic_not_a_quality_score(self):
        self.adapter("init()\nemit({'type': 'trace', 'tool': 'mmcg'})\nfinal()\n")
        result = bench.run_trial(self.prepare("source"))
        self.assertEqual(result["run_status"]["state"], "completed")
        self.assertEqual(result["quality"], {"status": "review_pending", "score": None})
        self.assertEqual(result["diagnostics"]["unexpected_tools"], ["mmcg"])
        self.assertIn("unexpected_tool", result["comparability"]["reasons"])

    def test_transport_protocol_model_and_identity_failures_are_distinct(self):
        cases = {
            "timeout": "init()\ntime.sleep(20)\n",
            "invocation_error": "init()\nfinal()\nsys.exit(8)\n",
            "model_error": "init()\nfinal(model_error=True)\n",
            "identity_mismatch": "init('unexpected-model')\nfinal()\n",
            "protocol_error": "init()\nprint('not-json')\nfinal()\n",
        }
        self.config["limits"]["timeout_seconds"] = 1
        for state, body in cases.items():
            with self.subTest(state=state):
                self.adapter(body)
                trial = self.prepare()
                result = bench.run_trial(trial)
                self.assertEqual(result["run_status"]["state"], state)
                self.assertFalse(result["comparability"]["eligible"])
                self.assertTrue((trial / "trace.jsonl").exists())
                self.assertTrue((trial / "stderr.txt").exists())

    def test_missing_or_duplicate_terminal_event_is_not_success(self):
        for body, issue in (("init()\n", "missing_result"), ("final()\n", "missing_init"),
                            ("init()\nfinal()\nfinal()\n", "event_after_result")):
            with self.subTest(issue=issue):
                self.adapter(body)
                result = bench.run_trial(self.prepare())
                self.assertEqual(result["run_status"]["state"], "protocol_error")
                self.assertIn(issue, result["diagnostics"]["protocol_issues"])

    def test_empty_success_or_ambiguous_error_flag_is_not_success(self):
        for body, issue in (("init()\nfinal('   ')\n", "empty_answer"),
                            ("init()\nfinal(model_error='false')\n", "missing_or_invalid_model_error")):
            with self.subTest(issue=issue):
                self.adapter(body)
                result = bench.run_trial(self.prepare())
                self.assertEqual(result["run_status"]["state"], "protocol_error")
                self.assertIn(issue, result["diagnostics"]["protocol_issues"])

    def test_malformed_unicode_nested_json_and_numbers_still_produce_envelopes(self):
        cases = (
            "init()\nfinal(chr(0xd800))\n",
            "init()\nprint('{\"type\":\"trace\",\"body\":' + '[' * 10000 + '0' + ']' * 10000 + '}')\nfinal()\n",
            "init()\nfinal(cost_usd=float('inf'))\n",
            "init()\nprint('{\"type\":\"result\",\"answer\":\"ok\",\"cost_usd\":1e999}')\n",
        )
        for body in cases:
            with self.subTest(body=body[:60]):
                self.adapter(body)
                trial = self.prepare()
                result = bench.run_trial(trial)
                self.assertEqual(result["run_status"]["state"], "protocol_error")
                self.assertIn("invalid_json_event", result["diagnostics"]["protocol_issues"])
                self.assertEqual(bench.load_json(trial / "result.json"), result)
        self.adapter("init()\nfinal(cost_usd=10**1000)\n")
        trial = self.prepare()
        result = bench.run_trial(trial)
        self.assertEqual(result["run_status"]["state"], "completed")
        self.assertIsNone(result["diagnostics"]["telemetry"]["cost_usd"])
        self.assertEqual(result["quality"]["status"], "review_pending")

    def test_bounded_stdout_stderr_and_answer_output(self):
        self.config["limits"].update(trace_bytes=1024, stderr_bytes=512, answer_bytes=100)
        for stream, size in (("stdout", 1024), ("stderr", 512)):
            with self.subTest(stream=stream):
                self.adapter(f"init()\nsys.{stream}.write('x' * 100000)\nsys.{stream}.flush()\n")
                trial = self.prepare()
                result = bench.run_trial(trial)
                self.assertEqual(result["run_status"]["state"], "output_limit")
                artifact = "trace.jsonl" if stream == "stdout" else "stderr.txt"
                self.assertEqual((trial / artifact).stat().st_size, size)
        self.adapter("init()\nfinal('x' * 101)\n")
        trial = self.prepare()
        result = bench.run_trial(trial)
        self.assertEqual(result["run_status"], {"state": "protocol_error", "reason": "answer_limit"})
        self.assertIsNone(result["answer"])
        self.assertFalse((trial / "answer.md").exists())

    def test_batch_counterbalances_independent_trials_without_running_models(self):
        batch = bench.prepare_batch(task=self.task, rubric=self.rubric, config=self.config,
            source_repo=self.repo, tool_repo=self.repo, output=self.root / "batches", repetitions=3)
        value = bench.load_json(batch / "batch.json")
        self.assertEqual([t["condition"] for t in value["trials"]], [
            "source", "portable", "portable_mmcg", "portable", "portable_mmcg", "source",
            "portable_mmcg", "source", "portable"])
        self.assertEqual(len({t["directory"] for t in value["trials"]}), 9)
        self.assertEqual(len({t["common_sha256"] for t in value["trials"]}), 1)
        self.assertIsNone(value["quality_uplift"])
        self.assertFalse(value["comparison_accepted"])
        self.assertFalse(list(batch.rglob("adapter-called")))

    def test_source_allowlist_rejects_control_files_and_path_escapes(self):
        for path in (".", "../private", "/private", "src//service.py", "src/./service.py",
                     "src/../service.py", "src\\service.py", "evals/answer.json", "AGENTS.md",
                     "nested/CLAUDE.md", ".claude/settings.json", ".git/config", "C:/source"):
            with self.subTest(path=path), self.assertRaises(bench.BenchmarkError):
                bench.safe_source_path(path)

    def test_source_symlink_is_rejected_instead_of_followed(self):
        (self.repo / "src/link.py").symlink_to("service.py")
        self.git("add", "src/link.py")
        self.git("commit", "-qm", "symlink fixture")
        task = dict(self.task, revision=self.git("rev-parse", "HEAD").strip(),
                    source_allowlist=["src/link.py"])
        self.assert_setup_failure(self.prepare(task=task), "source_type")


@unittest.skipUnless(os.name == "posix", "POSIX process supervision")
class ProcessTests(unittest.TestCase):
    def test_large_input_does_not_deadlock_a_child_that_never_reads_it(self):
        with tempfile.TemporaryDirectory() as directory:
            result = run_bounded([sys.executable, "-c", "import time; time.sleep(20)"],
                cwd=Path(directory), env={}, stdin=b"x" * 1_000_000, timeout=0.3)
        self.assertEqual(result.stop_reason, "timeout")
        self.assertLess(result.elapsed_seconds, 5)

    def test_timeout_kills_the_child_process_group(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            code = ("import subprocess, sys, time; "
                    "p = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(20)']); "
                    "print(p.pid, flush=True); time.sleep(20)")
            result = run_bounded([sys.executable, "-c", code], cwd=root, env={}, timeout=0.5)
            self.assertEqual(result.stop_reason, "timeout")
            pid = int(result.stdout.strip())
            # A dead descendant can remain a zombie until init reaps it on Linux.
            check = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True,
                                   text=True, timeout=5)
            self.assertTrue(check.returncode != 0 or check.stdout.strip().startswith("Z"), check.stdout)

    def test_spawn_failure_has_no_fabricated_return_code(self):
        with tempfile.TemporaryDirectory() as directory:
            result = run_bounded([str(Path(directory) / "absent")], cwd=Path(directory), env={})
        self.assertEqual(result.stop_reason, "spawn_error")
        self.assertIsNone(result.returncode)


if __name__ == "__main__":
    unittest.main()
