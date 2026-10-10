"""Exercise the real trial transport with disposable Git/SQLite and fake adapters."""

from pathlib import Path
from unittest.mock import patch
import copy
import hashlib
import os
import shutil
import sqlite3
import subprocess
import time
import unittest

from evals.benchmark import artifacts as artifact_io
from evals.benchmark import batch as batch_execution
from evals.benchmark import conditions as condition_contract
from evals.benchmark import protocol as model_protocol
from evals.benchmark import source as source_snapshot
from evals.benchmark import trials as trial_runner
from evals.shared import process as process_runner
from tests.evals.support.benchmark import BenchmarkFixture


@unittest.skipUnless(os.name == "posix" and shutil.which("git"), "requires POSIX and Git")
class BenchmarkTests(unittest.TestCase):
    def setUp(self):
        self.fixture = BenchmarkFixture()
        self.addCleanup(self.fixture.close)


    def test_stale_or_unbound_rubric_is_rejected_before_preparation(self):
        for revision in ("0" * 40, None):
            with self.subTest(revision=revision):
                if revision is None:
                    self.fixture.rubric.pop("source_revision")
                else:
                    self.fixture.rubric["source_revision"] = revision
                with self.assertRaises(artifact_io.BenchmarkError) as raised:
                    self.fixture.prepare()
                self.assertEqual(raised.exception.code, "rubric_mismatch")
                self.assertFalse((self.fixture.root / "trials").exists())


    def manifest(self, trial):
        return artifact_io.load_json(trial / "manifest.json")


    def assert_setup_failure(self, trial, reason):
        self.assertEqual(self.manifest(trial)["status"], "setup_failed")
        result = trial_runner.run_trial(trial)
        self.assertEqual(result["run_status"], {"state": "setup_error", "reason": reason})
        self.assertEqual(result["quality"]["status"], "not_evaluated")
        self.assertFalse((trial / "adapter-called").exists())


    def test_conditions_share_source_task_limits_and_rubric_but_only_third_indexes(self):
        trials = [self.fixture.prepare(condition) for condition in condition_contract.CONDITIONS]
        manifests = [self.manifest(trial) for trial in trials]
        requests = [artifact_io.load_json(trial / "request.json") for trial in trials]
        self.assertEqual({m["status"] for m in manifests}, {"prepared"})
        for field in ("common_sha256", "source_sha256", "rubric_sha256", "projection_revision"):
            self.assertEqual(len({m[field] for m in manifests}), 1, field)
        self.assertEqual(len({m["condition_sha256"] for m in manifests}), 3)
        self.assertEqual(len({m["request_sha256"] for m in manifests}), 3)
        self.assertEqual(requests[0]["portable_instruction"], "")
        self.assertEqual(requests[1]["portable_instruction"], self.fixture.instruction)
        self.assertEqual(requests[2]["portable_instruction"], self.fixture.instruction)
        for index, (trial, request) in enumerate(zip(trials, requests)):
            self.assertEqual(request["task"], self.fixture.task)
            self.assertEqual(request["model"], self.fixture.config["model"])
            self.assertEqual((trial / "indexer-called").exists(), index == 2)
            self.assertEqual("mmcg" in request["available_tools"], index == 2)
            self.assertEqual((trial / "index").exists(), index == 2)
            self.assertEqual(trial_runner.run_trial(trial)["run_status"]["state"], "completed")


    def test_model_limits_can_be_disabled_but_finite_limits_and_byte_caps_are_validated(self):
        accepted = model_protocol.validate_limits({
            "max_turns": model_protocol.MAX_TURNS_LIMIT,
            "max_output_tokens": model_protocol.MAX_OUTPUT_TOKENS_LIMIT,
        })
        self.assertEqual(accepted["max_turns"], model_protocol.MAX_TURNS_LIMIT)
        self.assertEqual(accepted["max_output_tokens"], model_protocol.MAX_OUTPUT_TOKENS_LIMIT)
        for field, limit in (
                ("max_turns", model_protocol.MAX_TURNS_LIMIT),
                ("max_output_tokens", model_protocol.MAX_OUTPUT_TOKENS_LIMIT)):
            with self.subTest(field=field), self.assertRaises(artifact_io.BenchmarkError) as raised:
                model_protocol.validate_limits({field: limit + 1})
            self.assertEqual(raised.exception.code, "invalid_limits")
        limits = model_protocol.validate_limits({name: None for name in model_protocol.OPTIONAL_LIMITS})
        result = {"usage": {"input_tokens": 5, "output_tokens": 100000,
                           "cache_read_tokens": 3, "cache_write_tokens": 0}, "turns": 100}
        measured = model_protocol.telemetry(result, limits)
        self.assertTrue(measured["complete"])
        self.assertEqual(measured["usage"]["output_tokens"], 100000)
        self.assertEqual(measured["turns"], 100)
        self.assertEqual(measured["budget_exceeded"], [])
        del result["usage"]["cache_write_tokens"]
        self.assertFalse(model_protocol.telemetry(result, limits)["complete"])
        for field, value in (("answer_bytes", None), ("timeout_seconds", False),
                             ("timeout_seconds", 0), ("max_turns", -1), ("max_output_tokens", "none")):
            with self.subTest(field=field, value=value), self.assertRaises(artifact_io.BenchmarkError):
                model_protocol.validate_limits({field: value})


    def test_git_projection_has_no_original_history_or_omitted_objects(self):
        trial = self.fixture.prepare()
        source = trial / "source"
        self.assertEqual(self.fixture.git("rev-list", "--all", "--count", cwd=source).strip(), "1")
        self.assertEqual(set(self.fixture.git("ls-files", cwd=source).splitlines()), set(self.fixture.task["source_allowlist"]))
        hidden_blob = self.fixture.git("rev-parse", "HEAD:evals/answer.json").strip()
        result = subprocess.run([shutil.which("git"), "cat-file", "-e", hidden_blob],
                                cwd=source, env=self.fixture.env, capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        for path in source.rglob("*"):
            if path.is_file():
                body = path.read_bytes()
                for canary in (b"ANSWER_KEY_CANARY", b"SETTINGS_CANARY", b"AGENT_CONFIG_CANARY"):
                    self.assertNotIn(canary, body)


    def test_prepared_version_one_generic_requests_remain_runnable(self):
        for condition in ("source", "portable_mmcg"):
            with self.subTest(condition=condition):
                trial = self.fixture.prepare(condition)
                manifest = self.manifest(trial)
                request = artifact_io.load_json(trial / "request.json")
                manifest["schema_version"] = 1
                request.pop("projection_revision")
                if request["mmcg"] is not None:
                    request["mmcg"] = {key: request["mmcg"][key] for key in ("binary", "index")}
                manifest["request_sha256"] = artifact_io.digest(request)
                (trial / "request.json").write_bytes(artifact_io.canonical(request))
                (trial / "manifest.json").write_bytes(artifact_io.canonical(manifest))
                self.assertEqual(trial_runner.run_trial(trial)["run_status"]["state"], "completed")


    def test_git_projection_keeps_ignored_files_and_attribute_sensitive_bytes(self):
        (self.fixture.repo / ".gitignore").write_text("*.py\n")
        (self.fixture.repo / ".gitattributes").write_text("*.py text eol=lf\n")
        raw = b"def value():\r\n    return 7\r\n"
        (self.fixture.repo / "src/service.py").write_bytes(raw)
        # Store raw fixture bytes rather than letting its attributes normalize them.
        oid = self.fixture.git("hash-object", "-w", "--no-filters", "src/service.py").strip()
        self.fixture.git("update-index", "--cacheinfo", "100644", oid, "src/service.py")
        self.fixture.git("add", ".gitignore", ".gitattributes")
        self.fixture.git("commit", "-qm", "raw source with ignore and attributes")
        task = dict(self.fixture.task, revision=self.fixture.git("rev-parse", "HEAD").strip(),
                    source_allowlist=["src/service.py", ".gitignore", ".gitattributes"])
        self.fixture.rubric["source_revision"] = task["revision"]
        trial = self.fixture.prepare(task=task)
        self.assertEqual(self.manifest(trial)["status"], "prepared")
        self.assertEqual((trial / "source/src/service.py").read_bytes(), raw)
        self.assertEqual(set(self.fixture.git("ls-files", cwd=trial / "source").splitlines()), set(task["source_allowlist"]))
        stored = subprocess.run([shutil.which("git"), "show", "HEAD:src/service.py"],
                                 cwd=trial / "source", env=self.fixture.env, check=True,
                                 capture_output=True, timeout=10).stdout
        self.assertEqual(stored, raw)
        self.assertEqual(trial_runner.run_trial(trial)["run_status"]["state"], "completed")


    def test_request_and_environment_do_not_inherit_hidden_inputs(self):
        self.fixture.adapter("""
            init()
            (root.parent / 'observed-env.json').write_text(json.dumps(dict(os.environ)))
            final(json.dumps(request))
        """)
        with patch.dict(os.environ, {"SECRET_CANARY": "secret", "GIT_DIR": "/wrong/git",
                                     "PYTHONPATH": "/wrong/python", "NODE_OPTIONS": "--inspect",
                                     "OPENAI_API_KEY": "not-implicitly-passed"}):
            trial = self.fixture.prepare()
            result = trial_runner.run_trial(trial)
        self.assertEqual(result["run_status"]["state"], "completed")
        observed = artifact_io.load_json(trial / "observed-env.json")
        for name in ("SECRET_CANARY", "GIT_DIR", "PYTHONPATH", "NODE_OPTIONS", "OPENAI_API_KEY"):
            self.assertNotIn(name, observed)
        self.assertEqual(observed["HOME"], str(trial / "home"))
        self.assertNotIn("HIDDEN_RUBRIC_CANARY", (trial / "answer.md").read_text())
        self.assertNotIn("rubric", artifact_io.load_json(trial / "request.json"))


    def test_full_final_answer_is_retained_without_a_quality_score(self):
        answer = "Observed src/service.py:2.\n" + "полный ответ " * 700
        self.fixture.adapter(f"init()\nemit({{'type': 'trace', 'tool': 'source_read', 'path': 'src/service.py'}})\nfinal({answer!r})\n")
        trial = self.fixture.prepare()
        result = trial_runner.run_trial(trial)
        self.assertEqual(result["run_status"]["state"], "completed")
        self.assertEqual((trial / "answer.md").read_text(), answer)
        self.assertEqual(result["answer"]["sha256"], hashlib.sha256(answer.encode()).hexdigest())
        self.assertEqual(result["answer"]["bytes"], len(answer.encode()))
        self.assertEqual(result["quality"], {"status": "review_pending", "score": None})
        self.assertFalse(result["comparability"]["eligible"])
        self.assertEqual(result["diagnostics"]["tools"], ["source_read"])
        self.assertEqual(len((trial / "trace.jsonl").read_bytes()), result["diagnostics"]["trace_bytes"])
        self.assertEqual(artifact_io.load_json(trial / "result.json"), result)


    def test_runtime_pin_and_index_failures_prevent_model_invocation(self):
        for role in ("adapter", "mmcg"):
            with self.subTest(role=role):
                config = copy.deepcopy(self.fixture.config)
                config[role]["sha256"] = "0" * 64
                trial = self.fixture.prepare("portable_mmcg", config=config)
                self.assert_setup_failure(trial, f"{role}_mismatch")
                self.assertFalse((trial / "indexer-called").exists())
        config = copy.deepcopy(self.fixture.config)
        config["mmcg"]["source_revision"] = "1" * 40
        self.assert_setup_failure(self.fixture.prepare("portable_mmcg", config=config), "mmcg_revision_mismatch")
        config.pop("mmcg")
        self.assert_setup_failure(self.fixture.prepare("portable_mmcg", config=config), "mmcg_missing")


    def test_every_condition_requires_an_available_exact_tool_commit(self):
        tree = self.fixture.git("rev-parse", "HEAD^{tree}").strip()
        for revision, reason in (("0" * 40, "tool_revision_unavailable"),
                                 (tree, "tool_revision_unavailable")):
            for condition in condition_contract.CONDITIONS:
                with self.subTest(revision=revision, condition=condition):
                    config = copy.deepcopy(self.fixture.config)
                    config["tool_revision"] = revision
                    self.assert_setup_failure(self.fixture.prepare(condition, config=config), reason)


    def test_portable_instruction_must_be_a_regular_blob(self):
        (self.fixture.repo / "skills/research/link.md").symlink_to("SKILL.md")
        self.fixture.git("add", "skills/research/link.md")
        self.fixture.git("commit", "-qm", "symlink instruction fixture")
        config = copy.deepcopy(self.fixture.config)
        config["tool_revision"] = self.fixture.git("rev-parse", "HEAD").strip()
        config["instruction_path"] = "skills/research/link.md"
        self.assert_setup_failure(self.fixture.prepare("portable", config=config), "instruction_type")


    def test_failed_partial_stale_or_uncheckpointed_indexes_never_run(self):
        expected = {"exit_failure": "index_setup_failed", "partial": "index_source_mismatch",
                    "wrong_root": "index_root_mismatch", "wrong_hash": "index_source_mismatch",
                    "wrong_contract": "index_contract_mismatch", "wal": "index_uncheckpointed"}
        for mode, reason in expected.items():
            with self.subTest(mode=mode):
                self.fixture.indexer(mode)
                trial = self.fixture.prepare("portable_mmcg")
                self.assertTrue((trial / "index/mmcg.db").is_file())
                self.assert_setup_failure(trial, reason)


    def test_index_validation_rejects_empty_sqlite_sidecars(self):
        trial = self.fixture.prepare("portable_mmcg")
        manifest = self.manifest(trial)
        index = trial / "index/mmcg.db"
        for suffix in ("-wal", "-journal", "-shm"):
            with self.subTest(suffix=suffix):
                sidecar = index.with_name(index.name + suffix)
                sidecar.touch()
                try:
                    with self.assertRaises(artifact_io.BenchmarkError) as raised:
                        source_snapshot.validate_index(
                            index, trial / "source", manifest["source_files"],
                            manifest["index_contract"], manifest["indexed_files"],
                        )
                    self.assertEqual(raised.exception.code, "index_uncheckpointed")
                finally:
                    sidecar.unlink(missing_ok=True)


    def test_index_validation_rejects_a_same_byte_database_replacement(self):
        trial = self.fixture.prepare("portable_mmcg")
        manifest = self.manifest(trial)
        index = trial / "index/mmcg.db"
        real_connect = sqlite3.connect
        replaced = False

        def replace_before_open(*args, **kwargs):
            nonlocal replaced
            if not replaced:
                replacement = index.with_name("replacement.db")
                replacement.write_bytes(index.read_bytes())
                replacement.replace(index)
                replaced = True
            return real_connect(*args, **kwargs)

        with patch.object(source_snapshot.sqlite3, "connect", side_effect=replace_before_open):
            with self.assertRaises(artifact_io.BenchmarkError) as raised:
                source_snapshot.validate_index(index, trial / "source", manifest["source_files"],
                                     manifest["index_contract"], manifest["indexed_files"])
        self.assertEqual(raised.exception.code, "index_changed")


    def test_unknown_indexed_paths_are_rejected_before_indexing(self):
        for paths in ([], ["omitted.py"], ["src/service.py"] * 2):
            with self.subTest(paths=paths):
                self.fixture.config["mmcg"]["indexed_files"] = paths
                trial = self.fixture.prepare("portable_mmcg")
                self.assert_setup_failure(trial, "index_scope_invalid")
                self.assertFalse((trial / "indexer-called").exists())


    def test_changed_runtime_after_preparation_prevents_invocation(self):
        trial = self.fixture.prepare()
        with Path(self.fixture.config["adapter"]["path"]).open("a") as handle:
            handle.write("# changed executable\n")
        result = trial_runner.run_trial(trial)
        self.assertEqual(result["run_status"]["reason"], "adapter_mismatch")
        self.assertFalse((trial / "adapter-called").exists())


    def test_changed_source_or_private_key_prevents_invocation(self):
        for mutation, reason in (("bytes", "source_changed"), ("mode", "source_changed"),
                                 ("inventory", "source_changed"), ("rubric", "rubric_changed"),
                                 ("request", "request_changed")):
            with self.subTest(mutation=mutation):
                trial = self.fixture.prepare()
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
                    value = artifact_io.load_json(path)
                    value["added"] = "changed"
                    path.write_bytes(artifact_io.canonical(value))
                result = trial_runner.run_trial(trial)
                self.assertEqual(result["run_status"]["reason"], reason)
                self.assertFalse((trial / "adapter-called").exists())


    def test_mutation_during_adapter_retains_answer_and_marks_input_change(self):
        self.fixture.adapter("""
            init()
            target = root / 'src/service.py'
            target.chmod(0o644)
            target.write_text('changed source')
            final()
        """)
        trial = self.fixture.prepare()
        result = trial_runner.run_trial(trial)
        self.assertEqual(result["run_status"], {"state": "input_changed", "reason": "source_changed"})
        self.assertEqual(result["quality"]["status"], "review_pending")
        self.assertIn("source_changed", result["comparability"]["reasons"])


    def test_changed_manifest_cannot_reuse_original_digests_and_request(self):
        for field, replacement in (("condition", "portable"), ("limits", {"timeout_seconds": 999}),
                                   ("source_files", None), ("adapter", [])):
            with self.subTest(field=field):
                trial = self.fixture.prepare()
                manifest = self.manifest(trial)
                if field == "limits":
                    manifest[field].update(replacement)
                else:
                    manifest[field] = replacement
                (trial / "manifest.json").write_bytes(artifact_io.canonical(manifest))
                result = trial_runner.run_trial(trial)
                self.assertEqual(result["run_status"]["state"], "setup_error")
                self.assertFalse((trial / "adapter-called").exists())


    def test_prepared_manifest_rechecks_the_total_source_limit(self):
        trial = self.fixture.prepare()
        manifest = self.manifest(trial)
        prepared_bytes = sum(item["bytes"] for item in manifest["source_files"])
        with patch.object(condition_contract, "SOURCE_BYTE_LIMIT", prepared_bytes - 1):
            result = trial_runner.run_trial(trial)
        self.assertEqual(result["run_status"], {"state": "setup_error", "reason": "source_limit"})
        self.assertFalse((trial / "adapter-called").exists())


    def test_unexpected_empty_or_unreadable_directory_fails_inventory(self):
        for mode in (0o700, 0):
            with self.subTest(mode=mode):
                trial = self.fixture.prepare()
                unexpected = trial / "source/unexpected"
                unexpected.mkdir()
                unexpected.chmod(mode)
                try:
                    result = trial_runner.run_trial(trial)
                    self.assertEqual(result["run_status"]["reason"], "source_changed")
                    self.assertFalse((trial / "adapter-called").exists())
                finally:
                    unexpected.chmod(0o700)


    def test_attempt_cannot_be_silently_retried(self):
        trial = self.fixture.prepare()
        first = trial_runner.run_trial(trial)
        (trial / "run.lock").unlink()
        with self.assertRaisesRegex(artifact_io.BenchmarkError, "already attempted"):
            trial_runner.run_trial(trial)
        self.assertEqual(artifact_io.load_json(trial / "result.json"), first)


    def test_artifact_replacement_after_publication_cannot_return_success(self):
        parent = self.fixture.root / "artifact-publication"
        parent.mkdir()
        target = parent / "result.json"
        body = b"trusted artifact"
        original_fsync = artifact_io.os.fsync
        replaced = False

        def replace_after_publication(descriptor):
            nonlocal replaced
            result = original_fsync(descriptor)
            if not replaced and target.exists():
                replaced = True
                target.unlink()
                target.write_bytes(body)
            return result

        with patch.object(artifact_io.os, "fsync", side_effect=replace_after_publication):
            with self.assertRaises(artifact_io.BenchmarkError) as raised:
                artifact_io.write_new_bytes(target, body)
        self.assertEqual(raised.exception.code, "artifact_changed")
        self.assertTrue(replaced)
        self.assertEqual(target.read_bytes(), body)


    def test_detached_artifact_parent_cannot_return_publication_success(self):
        parent = self.fixture.root / "artifact-parent"
        parent.mkdir()
        target = parent / "result.json"
        detached = self.fixture.root / "detached-artifact-parent"
        original_fsync = artifact_io.os.fsync
        replaced = False

        def detach_after_write(descriptor):
            nonlocal replaced
            result = original_fsync(descriptor)
            if not replaced and target.exists():
                replaced = True
                parent.rename(detached)
                parent.mkdir()
            return result

        with patch.object(artifact_io.os, "fsync", side_effect=detach_after_write):
            with self.assertRaises(artifact_io.BenchmarkError) as raised:
                artifact_io.write_new_bytes(target, b"trusted artifact")
        self.assertEqual(raised.exception.code, "artifact_changed")
        self.assertTrue(replaced)
        self.assertFalse(target.exists())
        self.assertEqual(list(detached.iterdir()), [])


    def test_verified_artifact_writer_never_replaces_an_existing_file(self):
        parent = self.fixture.root / "artifact-no-clobber"
        parent.mkdir()
        target = parent / "result.json"
        target.write_bytes(b"existing")
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            artifact_io.write_new_bytes(target, b"replacement")
        self.assertEqual(raised.exception.code, "artifact_exists")
        self.assertEqual(target.read_bytes(), b"existing")


    def test_declared_latency_cannot_exceed_the_observed_process_span(self):
        trial = self.fixture.prepare()
        events = [{"type": "init", "model": self.fixture.config["model"], "adapter_version": "fake-v1"},
                  {"type": "result", "answer": "Observed src/service.py:2.", "model_error": False,
                   "usage": {"input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 0, "cache_write_tokens": 0},
                   "turns": 1, "timings": {"first_message_seconds": 1, "final_answer_seconds": 3}}]
        child = process_runner.ProcessResult(stdout=b"\n".join(artifact_io.canonical(row) for row in events),
            stderr=b"", returncode=0, stop_reason=None, elapsed_seconds=2)
        execute = process_runner.run_bounded
        def observe(command, **kwargs):
            if command[0] == self.fixture.config["adapter"]["path"]:
                return child
            return execute(command, **kwargs)
        with patch.object(process_runner, "run_bounded", side_effect=observe):
            result = trial_runner.run_trial(trial)
        self.assertEqual(result["run_status"]["state"], "protocol_error")
        telemetry = result["diagnostics"]["telemetry"]
        self.assertIn("invalid_timings", telemetry["issues"])
        self.assertFalse(telemetry["complete"])
        self.assertTrue(all(value is None for value in telemetry["timings"].values()))

    def test_missing_telemetry_and_exceeded_budget_fail_without_discarding_answers(self):
        for extra, state in (("usage=None, turns=None", "protocol_error"),
                ("turns=100, usage={'input_tokens': 2, 'output_tokens': 9000, 'cache_read_tokens': 0, 'cache_write_tokens': 0}", "budget_exceeded")):
            with self.subTest(extra=extra):
                self.fixture.adapter(f"init()\nfinal({extra})\n")
                result = trial_runner.run_trial(self.fixture.prepare())
                self.assertEqual(result["run_status"]["state"], state)
                self.assertEqual(result["quality"], {"status": "review_pending", "score": None})
                diagnostic = result["diagnostics"]["telemetry"]
                self.assertTrue(not diagnostic["complete"] or diagnostic["budget_exceeded"])


    def test_unavailable_reported_tool_fails_without_discarding_the_answer(self):
        self.fixture.adapter("init()\nemit({'type': 'trace', 'tool': 'mmcg'})\nfinal()\n")
        result = trial_runner.run_trial(self.fixture.prepare("source"))
        self.assertEqual(result["run_status"], {"state": "protocol_error", "reason": "unexpected_tool"})
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
        run_bounded = process_runner.run_bounded
        for state, body in cases.items():
            with self.subTest(state=state):
                self.fixture.adapter(body)
                trial = self.fixture.prepare()
                ready = False
                real_clock = time.monotonic

                def clock():
                    return real_clock() + (31 if ready else 0)

                def observe(chunk):
                    nonlocal ready
                    ready = state == "timeout" and bool(chunk)

                def supervise(*args, **kwargs):
                    if args[0][0] == self.fixture.config["adapter"]["path"]:
                        kwargs["on_stdout"] = observe
                    return run_bounded(*args, **kwargs)

                with patch.object(process_runner.time, "monotonic", side_effect=clock), \
                        patch.object(process_runner, "run_bounded", side_effect=supervise):
                    result = trial_runner.run_trial(trial)
                self.assertEqual(result["run_status"]["state"], state)
                self.assertFalse(result["comparability"]["eligible"])
                self.assertTrue((trial / "trace.jsonl").exists())
                self.assertTrue((trial / "stderr.txt").exists())


    def test_runtime_identity_mismatch_cannot_hide_behind_a_model_failure(self):
        self.fixture.adapter("""
            init('unexpected-model')
            final('', model_error=True,
                  failure={'state': 'model_error', 'code': 'provider_failed'})
        """)
        result = trial_runner.run_trial(self.fixture.prepare())
        self.assertEqual(result["run_status"], {
            "state": "identity_mismatch",
            "reason": "observed_model_or_adapter_version_mismatch",
        })
        self.assertIn("identity_mismatch", result["comparability"]["reasons"])


    def test_missing_or_duplicate_terminal_event_is_not_success(self):
        for body, issue in (("init()\n", "missing_result"), ("final()\n", "missing_init"),
                            ("init()\nfinal()\nfinal()\n", "event_after_result")):
            with self.subTest(issue=issue):
                self.fixture.adapter(body)
                result = trial_runner.run_trial(self.fixture.prepare())
                self.assertEqual(result["run_status"]["state"], "protocol_error")
                self.assertIn(issue, result["diagnostics"]["protocol_issues"])


    def test_empty_success_or_ambiguous_error_flag_is_not_success(self):
        for body, issue in (("init()\nfinal('   ')\n", "empty_answer"),
                            ("init()\nfinal(model_error='false')\n", "missing_or_invalid_model_error")):
            with self.subTest(issue=issue):
                self.fixture.adapter(body)
                result = trial_runner.run_trial(self.fixture.prepare())
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
                self.fixture.adapter(body)
                trial = self.fixture.prepare()
                result = trial_runner.run_trial(trial)
                self.assertEqual(result["run_status"]["state"], "protocol_error")
                self.assertIn("invalid_json_event", result["diagnostics"]["protocol_issues"])
                self.assertEqual(artifact_io.load_json(trial / "result.json"), result)
        self.fixture.adapter("init()\nfinal(cost_usd=10**1000)\n")
        trial = self.fixture.prepare()
        result = trial_runner.run_trial(trial)
        self.assertEqual(result["run_status"]["state"], "completed")
        self.assertIsNone(result["diagnostics"]["telemetry"]["cost_usd"])
        self.assertEqual(result["quality"]["status"], "review_pending")


    def test_bounded_stdout_stderr_and_answer_output(self):
        self.fixture.config["limits"].update(trace_bytes=1024, stderr_bytes=512, answer_bytes=100)
        for stream, size in (("stdout", 1024), ("stderr", 512)):
            with self.subTest(stream=stream):
                self.fixture.adapter(f"init()\nsys.{stream}.write('x' * 100000)\nsys.{stream}.flush()\n")
                trial = self.fixture.prepare()
                result = trial_runner.run_trial(trial)
                self.assertEqual(result["run_status"]["state"], "output_limit")
                artifact = "trace.jsonl" if stream == "stdout" else "stderr.txt"
                self.assertEqual((trial / artifact).stat().st_size, size)
        self.fixture.adapter("init()\nfinal('x' * 101)\n")
        trial = self.fixture.prepare()
        result = trial_runner.run_trial(trial)
        self.assertEqual(result["run_status"], {"state": "protocol_error", "reason": "answer_limit"})
        self.assertIsNone(result["answer"])
        self.assertFalse((trial / "answer.md").exists())


    def test_batch_counterbalances_independent_trials_without_running_models(self):
        batch = trial_runner.prepare_batch(task=self.fixture.task, rubric=self.fixture.rubric, config=self.fixture.config,
            source_repo=self.fixture.repo, tool_repo=self.fixture.repo, output=self.fixture.root / "batches", repetitions=3)
        value = artifact_io.load_json(batch / "batch.json")
        self.assertEqual(value["schema_version"], 2)
        self.assertEqual(value["batch_id"], batch.name)
        self.assertEqual(value["plan_sha256"], artifact_io.digest(batch_execution.batch_plan_identity(value)))
        self.assertEqual([t["condition"] for t in value["trials"]], [
            "source", "portable", "portable_mmcg", "portable", "portable_mmcg", "source",
            "portable_mmcg", "source", "portable"])
        self.assertEqual(len({t["directory"] for t in value["trials"]}), 9)
        self.assertEqual(len({t["common_sha256"] for t in value["trials"]}), 1)
        self.assertIsNone(value["quality_uplift"])
        self.assertFalse(value["comparison_accepted"])
        self.assertFalse(list(batch.rglob("adapter-called")))
        for position, item in enumerate(value["trials"]):
            manifest = self.manifest(batch / item["directory"])
            self.assertEqual(manifest["schema_version"], 3)
            self.assertEqual(manifest["batch"], {"batch_id": value["batch_id"],
                "plan_sha256": value["plan_sha256"], "position": position})


    def test_batch_enforces_recorded_order_without_spending_a_later_attempt(self):
        batch = trial_runner.prepare_batch(task=self.fixture.task, rubric=self.fixture.rubric, config=self.fixture.config,
            source_repo=self.fixture.repo, tool_repo=self.fixture.repo, output=self.fixture.root / "batches", repetitions=1)
        value = artifact_io.load_json(batch / "batch.json")
        trials = [batch / item["directory"] for item in value["trials"]]
        with self.assertRaises(artifact_io.BenchmarkError) as raised:
            trial_runner.run_trial(trials[1])
        self.assertEqual(raised.exception.code, "batch_order")
        self.assertFalse((trials[1] / "run.lock").exists())
        self.assertFalse((trials[1] / "adapter-called").exists())

        first = trial_runner.run_trial(trials[0])
        second = trial_runner.run_trial(trials[1])
        self.assertEqual(first["batch_execution"]["position"], 0)
        self.assertIsNone(first["batch_execution"]["previous_result_sha256"])
        self.assertEqual(second["batch_execution"], {"batch_id": value["batch_id"],
            "plan_sha256": value["plan_sha256"], "position": 1,
            "previous_result_sha256": hashlib.sha256((trials[0] / "result.json").read_bytes()).hexdigest()})


    def test_explicit_matrix_freezes_composed_instructions_and_tool_capabilities(self):
        instructions = self.fixture.condition_matrix()
        self.fixture.config["conditions"][-1]["tools"] = "mmcg"
        self.fixture.config["conditions"][-1]["symbol_lookup"] = "single"
        self.fixture.config["conditions"][-1]["source_delivery"] = "native_reuse"
        batch = trial_runner.prepare_batch(task=self.fixture.task, rubric=self.fixture.rubric, config=self.fixture.config,
            source_repo=self.fixture.repo, tool_repo=self.fixture.repo, output=self.fixture.root / "batches", repetitions=2)
        value = artifact_io.load_json(batch / "batch.json")
        self.assertEqual(value["schema_version"], 3)
        self.assertEqual(value["plan_sha256"], artifact_io.digest(batch_execution.batch_plan_identity(value)))
        self.assertEqual([item["condition"] for item in value["trials"]],
                         ["raw", "refined", "profile", "refined_profile", "refined", "profile", "refined_profile", "raw"])
        self.assertEqual(len({item["common_sha256"] for item in value["trials"]}), 1)
        for position, item in enumerate(value["trials"]):
            trial = batch / item["directory"]
            manifest = self.manifest(trial)
            spec = next(spec for spec in value["conditions"] if spec["id"] == item["condition"])
            request = artifact_io.load_json(trial / "request.json")
            self.assertEqual(manifest["schema_version"], 4)
            self.assertEqual(manifest["condition_spec"], spec)
            self.assertEqual(request["portable_instruction"], "\n\n".join(instructions[path] for path in spec["instruction_paths"]))
            self.assertEqual(manifest["instruction_files"], [
                {"path": path, "bytes": len(instructions[path].encode()),
                 "sha256": hashlib.sha256(instructions[path].encode()).hexdigest()} for path in spec["instruction_paths"]])
            self.assertEqual(request["task"], self.fixture.task)
            self.assertNotIn("conditions", request)
            self.assertEqual("mmcg" in request["available_tools"], spec["tools"] == "mmcg")
            if spec["tools"] == "mmcg":
                self.assertEqual(request["mmcg"]["symbol_lookup"], spec["symbol_lookup"])
                self.assertEqual(request["mmcg"]["source_delivery"], spec["source_delivery"])
            self.assertEqual((trial / "indexer-called").exists(), spec["tools"] == "mmcg")
            result = trial_runner.run_trial(trial)
            self.assertEqual(result["run_status"]["state"], "completed")
            self.assertEqual(result["batch_execution"]["position"], position)


    def test_rejects_invalid_matrix_before_creating_a_batch(self):
        self.fixture.condition_matrix()
        original = copy.deepcopy(self.fixture.config)
        for mutation in ("duplicate", "tools", "path", "overlap", "lookup_without_graph", "invalid_lookup", "cap"):
            with self.subTest(mutation=mutation):
                config = copy.deepcopy(original)
                repetitions = 1
                if mutation == "duplicate":
                    config["conditions"][1]["id"] = "raw"
                elif mutation == "tools":
                    config["conditions"][1]["tools"] = "shell"
                elif mutation == "path":
                    config["conditions"][1]["instruction_paths"] = ["../private.md"]
                elif mutation == "overlap":
                    config["conditions"][1]["instruction_paths"] = ["src/service.py"]
                elif mutation == "lookup_without_graph":
                    config["conditions"][1]["symbol_lookup"] = "single"
                elif mutation == "invalid_lookup":
                    config["conditions"][1].update(tools="mmcg", symbol_lookup="unknown")
                else:
                    repetitions = 16
                with self.assertRaises(artifact_io.BenchmarkError):
                    trial_runner.prepare_batch(task=self.fixture.task, rubric=self.fixture.rubric, config=config,
                        source_repo=self.fixture.repo, tool_repo=self.fixture.repo, output=self.fixture.root / "batches", repetitions=repetitions)
                self.assertFalse((self.fixture.root / "batches").exists())


    def test_matrix_spec_and_instruction_inventory_are_checked_before_invocation(self):
        self.fixture.condition_matrix()
        for mutation in ("spec", "file", "lookup"):
            with self.subTest(mutation=mutation):
                if mutation == "lookup":
                    self.fixture.config["conditions"][0].update(tools="mmcg", symbol_lookup="single")
                batch = trial_runner.prepare_batch(task=self.fixture.task, rubric=self.fixture.rubric, config=self.fixture.config,
                    source_repo=self.fixture.repo, tool_repo=self.fixture.repo, output=self.fixture.root / "batches", repetitions=1)
                trials = [batch / item["directory"] for item in artifact_io.load_json(batch / "batch.json")["trials"]]
                trial = trials[0]
                if mutation == "file":
                    trial_runner.run_trial(trials[0])
                    trial = trials[1]
                manifest = self.manifest(trial)
                if mutation == "spec":
                    manifest["condition_spec"]["tools"] = "mmcg"
                elif mutation == "lookup":
                    manifest["condition_spec"]["symbol_lookup"] = "batch"
                else:
                    manifest["instruction_files"][0]["sha256"] = "0" * 64
                manifest["condition_sha256"] = artifact_io.digest(model_protocol.condition_identity(manifest))
                (trial / "manifest.json").write_bytes(artifact_io.canonical(manifest))
                if mutation in ("spec", "lookup"):
                    with self.assertRaises(artifact_io.BenchmarkError) as raised:
                        trial_runner.run_trial(trial)
                    self.assertEqual(raised.exception.code, "batch_changed")
                    self.assertFalse((trial / "run.lock").exists())
                else:
                    result = trial_runner.run_trial(trial)
                    self.assertEqual(result["run_status"]["state"], "setup_error")
                    self.assertEqual(result["run_status"]["reason"], "request_changed")
                self.assertFalse((trial / "adapter-called").exists())


    def test_batch_lock_rejects_a_detached_root_before_spending_an_attempt(self):
        import fcntl

        batch = trial_runner.prepare_batch(task=self.fixture.task, rubric=self.fixture.rubric, config=self.fixture.config,
            source_repo=self.fixture.repo, tool_repo=self.fixture.repo, output=self.fixture.root / "batches", repetitions=1)
        trial = batch / artifact_io.load_json(batch / "batch.json")["trials"][0]["directory"]
        detached = self.fixture.root / "detached-batch"
        original_flock = fcntl.flock
        swapped = False

        def swap_after_lock(descriptor, operation):
            nonlocal swapped
            result = original_flock(descriptor, operation)
            if operation & fcntl.LOCK_EX and not swapped:
                swapped = True
                batch.rename(detached)
                shutil.copytree(detached, batch)
            return result

        with patch.object(fcntl, "flock", side_effect=swap_after_lock):
            with self.assertRaises(artifact_io.BenchmarkError) as raised:
                trial_runner.run_trial(trial)
        self.assertEqual(raised.exception.code, "batch_lock")
        self.assertTrue(swapped)
        self.assertFalse((trial / "run.lock").exists())
        self.assertFalse((trial / "adapter-called").exists())


    def test_source_allowlist_rejects_control_files_and_path_escapes(self):
        for path in (".", "../private", "/private", "src//service.py", "src/./service.py",
                     "src/../service.py", "src\\service.py", "evals/answer.json", "AGENTS.md",
                     "nested/CLAUDE.md", ".claude/settings.json", ".git/config", "C:/source"):
            with self.subTest(path=path), self.assertRaises(artifact_io.BenchmarkError):
                condition_contract.safe_source_path(path)


    def test_source_symlink_is_rejected_instead_of_followed(self):
        (self.fixture.repo / "src/link.py").symlink_to("service.py")
        self.fixture.git("add", "src/link.py")
        self.fixture.git("commit", "-qm", "symlink fixture")
        task = dict(self.fixture.task, revision=self.fixture.git("rev-parse", "HEAD").strip(),
                    source_allowlist=["src/link.py"])
        self.fixture.rubric["source_revision"] = task["revision"]
        self.assert_setup_failure(self.fixture.prepare(task=task), "source_type")



if __name__ == "__main__":
    unittest.main()
