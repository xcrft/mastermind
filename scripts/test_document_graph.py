"""Document relationship contracts; stdlib only, no model or indexer calls."""

from contextlib import redirect_stdout
from copy import deepcopy
import errno
import importlib.util
import io
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


HELPER = (Path(__file__).resolve().parents[1] / "skills/workflow/"
          "mastermind-project-history/scripts/document_graph.py")
SPEC = importlib.util.spec_from_file_location("document_graph", HELPER)
graph = importlib.util.module_from_spec(SPEC)
previous_bytecode_policy = sys.dont_write_bytecode
sys.dont_write_bytecode = True
try:
    SPEC.loader.exec_module(graph)
finally:
    sys.dont_write_bytecode = previous_bytecode_policy


POSIX_DESCRIPTORS = hasattr(os, "O_NOFOLLOW") and os.open in os.supports_dir_fd


class DocumentGraphPortableTests(unittest.TestCase):
    def test_unsupported_platform_returns_json_without_reading_sources(self):
        with tempfile.TemporaryDirectory() as root:
            output = io.StringIO()
            with patch.object(graph.os, "supports_dir_fd", set()), redirect_stdout(output):
                result = graph.main(["snapshot", "--root", root, "--relations", "absent.json",
                                     "--output", ".mastermind/research/snapshot.json"])
            self.assertEqual(result, 2)
            self.assertEqual(json.loads(output.getvalue())["error"]["code"], "unsupported_platform")


@unittest.skipUnless(POSIX_DESCRIPTORS, "requires POSIX directory descriptors and O_NOFOLLOW")
class DocumentGraphTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="document-graph-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.write(".gitignore", ".mastermind/\n")
        self.write("docs/adr/0001.md", "# Decision\nUse handler.\nProof.\n")
        self.write("docs/notes.md", "# Independent evidence\n")
        self.write("src/handler.py", "def handler():\n    return 1\n")
        self.write("tests/test_handler.py", "def test_handler():\n    assert handler() == 1\n")
        self.write("logs/result.log", "test passed\n")
        self.manifest = {"schema_version": 1, "relations": [
            {"from": {"path": "docs/adr/0001.md", "line": 2}, "relation": "documents",
             "to": {"path": "src/handler.py", "line": 2}},
            {"from": {"path": "docs/notes.md", "line": 1}, "relation": "supports",
             "to": {"path": "logs/result.log", "line": 1}},
            {"from": {"path": "docs/adr/0001.md", "line": 3}, "relation": "verified_by",
             "to": {"path": "tests/test_handler.py", "line": 2}},
        ]}
        self.write_manifest()
        self.git("init", "-q", "--initial-branch=main")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("-c", "core.hooksPath=/dev/null", "add", "-A")
        self.git("-c", "core.hooksPath=/dev/null", "commit", "-qm", "baseline")

    def write(self, name, content):
        target = self.root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
        return target

    def write_manifest(self, manifest=None, name="relations.json"):
        return self.write(name, json.dumps(self.manifest if manifest is None else manifest))

    def git(self, *arguments):
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update({"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"})
        return subprocess.run(["git", *arguments], cwd=self.root, text=True, capture_output=True,
                              check=True, timeout=10, env=environment).stdout

    def cli(self, *arguments, expected=0, root=None):
        completed = subprocess.run(
            [sys.executable, str(HELPER), *arguments, "--root", str(root or self.root)],
            capture_output=True, text=True, timeout=20,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        )
        self.assertEqual(completed.returncode, expected, (completed.stdout, completed.stderr))
        self.assertEqual(completed.stderr, "")
        return json.loads(completed.stdout)

    def snapshot(self, name="snapshot.json", corpus_dirs=()):
        corpus_args = [argument for directory in corpus_dirs for argument in ("--corpus-dir", directory)]
        return self.cli("snapshot", "--relations", "relations.json", "--output",
                        f".mastermind/research/{name}", *corpus_args)

    def check(self, name="snapshot.json", expected=0):
        return self.cli("check", "--graph", f".mastermind/research/{name}", expected=expected)

    def main_error(self, *arguments):
        output = io.StringIO()
        with redirect_stdout(output):
            code = graph.main([*arguments, "--root", str(self.root)])
        self.assertEqual(code, 2)
        result = json.loads(output.getvalue())
        self.assertEqual(result["status"], "error")
        return result["error"]

    def test_cli_snapshot_is_deterministic_and_current_does_not_verify_claims(self):
        first = self.snapshot()
        second = self.snapshot("repeat.json")
        self.assertEqual(first, second)
        self.assertEqual(first["root"], str(self.root))
        self.assertEqual(first["revision"], {"head": self.git("rev-parse", "HEAD").strip(), "dirty": False})
        self.assertEqual(first["sha256"], graph.digest({key: value for key, value in first.items() if key != "sha256"}))
        self.assertEqual([item["path"] for item in first["files"]], sorted(item["path"] for item in first["files"]))
        self.assertNotIn("Use handler.", json.dumps(first))
        self.assertEqual({edge["verification"] for edge in first["edges"]}, {"unverified"})
        index = self.root / ".git/index"
        before = (index.read_bytes(), index.stat().st_mtime_ns)
        checked = self.check()
        self.assertEqual((index.read_bytes(), index.stat().st_mtime_ns), before)
        self.assertEqual(checked["status"], "current")
        self.assertFalse(checked["revision_changed"])
        self.assertEqual(checked["changed_files"], [])
        self.assertEqual({edge["freshness"] for edge in checked["edges"]}, {"current"})
        self.assertEqual({edge["verification"] for edge in checked["edges"]}, {"unverified"})

    def test_same_size_same_mtime_changes_invalidate_only_incident_edges(self):
        self.snapshot()
        source = self.root / "src/handler.py"
        before = source.stat()
        source.write_text(source.read_text().replace("return 1", "return 2"))
        os.utime(source, ns=(before.st_atime_ns, before.st_mtime_ns))
        self.assertEqual(source.stat().st_size, before.st_size)
        checked = self.check(expected=1)
        by_relation = {edge["relation"]: edge for edge in checked["edges"]}
        self.assertEqual(by_relation["documents"]["freshness"], "needs_review")
        self.assertEqual(by_relation["supports"]["freshness"], "current")
        self.assertEqual(by_relation["verified_by"]["freshness"], "current")
        self.assertFalse(checked["revision_changed"])
        self.assertEqual(checked["changed_files"], [{"path": "src/handler.py", "reasons": ["content_changed"]}])

        document = self.root / "docs/adr/0001.md"
        before = document.stat()
        document.write_text(document.read_text().replace("Use handler", "New handler"))
        os.utime(document, ns=(before.st_atime_ns, before.st_mtime_ns))
        self.assertEqual(document.stat().st_size, before.st_size)
        checked = self.check(expected=1)
        self.assertEqual({edge["relation"] for edge in checked["edges"] if edge["freshness"] == "needs_review"},
                         {"documents", "verified_by"})
        self.assertEqual({edge["verification"] for edge in checked["edges"]}, {"unverified"})

    def test_deletions_and_line_drift_are_review_requests(self):
        self.snapshot()
        (self.root / "src/handler.py").unlink()
        self.write("docs/adr/0001.md", "# Short\n")
        checked = self.check(expected=1)
        changed = {item["path"]: item["reasons"] for item in checked["changed_files"]}
        self.assertEqual(changed["src/handler.py"], ["missing"])
        self.assertIn("line_out_of_range", changed["docs/adr/0001.md"])
        self.assertEqual(next(edge["freshness"] for edge in checked["edges"] if edge["relation"] == "supports"), "current")

    def test_snapshot_rejects_nonexistent_lines_without_treating_control_bytes_as_lines(self):
        self.write("docs/adr/0001.md", "one\vtwo\fthree")
        result = self.cli("snapshot", "--relations", "relations.json", "--output", ".mastermind/research/no.json", expected=2)
        self.assertEqual(result["error"]["code"], "line_out_of_range")

    def test_unlisted_new_decisions_are_not_a_completeness_claim(self):
        self.snapshot()
        self.write("docs/adr/0002.md", "# Supersedes everything in 0001.md\nStatus: verified\n")
        checked = self.check()
        self.assertEqual(checked["status"], "current")
        self.assertEqual(checked["corpus"]["status"], "not_tracked")
        self.assertTrue(checked["revision"]["dirty"])
        self.git("add", "docs/adr/0002.md")
        self.git("-c", "core.hooksPath=/dev/null", "commit", "-qm", "new decision")
        checked = self.check()
        self.assertTrue(checked["revision_changed"])
        self.assertEqual(checked["status"], "current")

    def test_corpus_new_nested_decision_requires_research_with_same_dirty_head(self):
        self.write("unlisted.txt", "Keep the worktree dirty.\n")
        saved = self.snapshot(corpus_dirs=["docs/adr"])
        self.assertEqual(saved["schema_version"], 2)
        self.assertEqual(saved["corpus"]["directories"], ["docs/adr"])
        self.write("docs/adr/nested/0002.md", "# Supersedes 0001\nStatus: proposed\n")
        checked = self.check(expected=1)
        self.assertEqual(checked["revision"], saved["revision"])
        self.assertEqual(checked["status"], "needs_review")
        self.assertEqual(checked["corpus"], {
            "status": "changed", "directories": ["docs/adr"],
            "changed_files": [{"path": "docs/adr/nested/0002.md", "reasons": ["added"]}],
        })
        self.assertEqual(checked["changed_files"], [])
        self.assertEqual({edge["freshness"] for edge in checked["edges"]}, {"current"})
        self.assertEqual({edge["verification"] for edge in checked["edges"]}, {"unverified"})
        self.git("add", "docs/adr/nested/0002.md")
        self.git("-c", "core.hooksPath=/dev/null", "commit", "-qm", "new decision")
        self.assertEqual(self.check(expected=1)["corpus"], checked["corpus"])

    def test_corpus_detects_unreferenced_edits_rename_and_deletion(self):
        source = self.write("docs/adr/0002.md", "# Proposal A\n")
        self.snapshot(corpus_dirs=["docs/adr"])
        before = source.stat()
        source.write_text("# Proposal B\n")
        os.utime(source, ns=(before.st_atime_ns, before.st_mtime_ns))
        self.assertEqual(source.stat().st_size, before.st_size)
        checked = self.check(expected=1)
        self.assertEqual(checked["changed_files"], [])
        self.assertEqual(checked["corpus"]["changed_files"], [
            {"path": "docs/adr/0002.md", "reasons": ["content_changed"]},
        ])
        source.rename(source.with_name("0003.md"))
        self.assertEqual(self.check(expected=1)["corpus"]["changed_files"], [
            {"path": "docs/adr/0002.md", "reasons": ["missing"]},
            {"path": "docs/adr/0003.md", "reasons": ["added"]},
        ])
        source.with_name("0003.md").unlink()
        self.assertEqual(self.check(expected=1)["corpus"]["changed_files"], [
            {"path": "docs/adr/0002.md", "reasons": ["missing"]},
        ])

    def test_corpus_empty_scope_is_distinct_from_legacy_untracked(self):
        (self.root / "docs/empty").mkdir()
        tracked = self.snapshot("tracked.json", corpus_dirs=["docs/empty"])
        self.assertEqual(tracked["corpus"]["files"], [])
        legacy = self.snapshot("legacy.json")
        self.assertEqual(legacy["schema_version"], 1)
        self.assertNotIn("corpus", legacy)
        self.write("docs/outside.md", "# Outside the selected directory\n")
        checked = self.check("tracked.json")
        self.assertEqual(checked["schema_version"], 2)
        self.assertEqual(checked["corpus"], {
            "status": "current", "directories": ["docs/empty"], "changed_files": [],
        })
        self.assertEqual(self.check("legacy.json")["corpus"], {
            "status": "not_tracked", "directories": [], "changed_files": [],
        })

    def test_corpus_hidden_root_and_json_output_do_not_self_invalidate(self):
        self.write(".mastermind/decisions/one.MARKDOWN", "# Decision\n")
        self.write(".mastermind/research/notes.md", "# Research\n")
        self.write(".mastermind/research/.private.md", "Excluded\n")
        self.write(".mastermind/research/.cache/hidden.md", "Excluded\n")
        self.write(".mastermind/research/notes.txt", "Excluded\n")
        roots = [str(self.root / ".mastermind/research"), ".mastermind/decisions"]
        first = self.snapshot(corpus_dirs=roots)
        self.assertEqual([item["path"] for item in first["corpus"]["files"]], [
            ".mastermind/decisions/one.MARKDOWN", ".mastermind/research/notes.md",
        ])
        self.assertEqual(first, self.snapshot("repeat.json", corpus_dirs=list(reversed(roots))))
        self.assertEqual(self.check()["corpus"]["status"], "current")
        os.utime(self.root / ".mastermind/research/notes.md", (946684800, 946684800))
        self.assertEqual(self.check()["corpus"]["status"], "current")
        for suffix in ["md", "MARKDOWN"]:
            name = f".mastermind/research/rejected.{suffix}"
            result = self.cli("snapshot", "--relations", "relations.json", "--output", name,
                              "--corpus-dir", "docs/adr", expected=2)
            self.assertEqual(result["error"]["code"], "corpus_output_conflict")
            self.assertFalse((self.root / name).exists())

    def test_corpus_rejects_unsafe_duplicate_overlapping_or_excessive_roots(self):
        for directories in [["."], [""], ["../docs"], [".git"], ["docs/.secret"],
                            ["docs/adr", "docs/adr"], ["docs/adr", "docs"],
                            [f"docs/{index}" for index in range(graph.CORPUS_DIRECTORY_LIMIT + 1)]]:
            with self.subTest(directories=directories), self.assertRaises(graph.GraphError):
                graph.corpus_directories(directories)
        repository = graph.Repository(self.root)
        self.addCleanup(repository.close)
        alias = self.root / "DOCS"
        if alias.exists() and alias.samefile(self.root / "docs"):
            with self.assertRaisesRegex(graph.GraphError, "corpus_scope_overlap"):
                graph.snapshot(repository, "relations.json", ".mastermind/research/no.json", ["docs", "DOCS"])

    def test_corpus_v2_schema_rejects_forged_inventory_even_with_valid_digest(self):
        original = self.snapshot(corpus_dirs=["docs"])
        bad_corpora = [None, {}, {**original["corpus"], "status": "current"},
                       {**original["corpus"], "directories": []},
                       {**original["corpus"], "directories": "docs"},
                       {**original["corpus"], "directories": ["src", "docs"]},
                       {**original["corpus"], "directories": ["docs", "docs/adr"]},
                       {**original["corpus"], "files": list(reversed(original["corpus"]["files"]))},
                       {**original["corpus"], "files": []}]
        for key, value in [("path", "outside.md"), ("path", "docs/file.txt"),
                           ("path", "docs/" + "nested/" * (graph.CORPUS_DEPTH_LIMIT + 1) + "file.md"),
                           ("bytes", True), ("lines", -1), ("sha256", "approved"),
                           ("sha256", "0" * 64), ("verified", True)]:
            corpus = deepcopy(original["corpus"])
            corpus["files"][0][key] = value
            bad_corpora.append(corpus)
        corpus = deepcopy(original["corpus"])
        corpus["files"].append(deepcopy(corpus["files"][0]))
        bad_corpora.append(corpus)
        for corpus in bad_corpora:
            value = {**deepcopy(original), "corpus": corpus}
            value["sha256"] = graph.digest({key: item for key, item in value.items() if key != "sha256"})
            with self.subTest(corpus=corpus), self.assertRaises(graph.GraphError):
                graph.validate_snapshot(value)
        for version in [True, 0, 1, 3]:
            value = {**original, "schema_version": version}
            with self.subTest(version=version), self.assertRaises(graph.GraphError):
                graph.validate_snapshot(value)
        value = {key: item for key, item in original.items() if key != "corpus"}
        with self.assertRaisesRegex(graph.GraphError, "invalid_schema"):
            graph.validate_snapshot(value)

    def test_corpus_missing_root_and_nonregular_entries_are_errors(self):
        (self.root / "docs/watched").mkdir()
        self.snapshot(corpus_dirs=["docs/watched"])
        (self.root / "docs/watched").rmdir()
        self.assertEqual(self.check(expected=2)["error"], {"code": "missing", "path": "docs/watched"})
        (self.root / "docs/watched").symlink_to(self.root / "docs/adr", target_is_directory=True)
        self.assertEqual(self.check(expected=2)["error"]["code"], "unsafe_path")
        (self.root / "docs/watched").unlink()
        (self.root / "docs/watched").mkdir()
        entry = self.root / "docs/watched/entry.md"
        entry.symlink_to(self.root / "missing.md")
        self.assertEqual(self.check(expected=2)["error"], {"code": "unsafe_path", "path": "docs/watched/entry.md"})
        entry.unlink()
        os.mkfifo(entry)
        self.assertEqual(self.check(expected=2)["error"]["code"], "not_regular")
        entry.unlink()
        entry.write_bytes(b"\xff\x00")
        self.assertEqual(self.check(expected=2)["error"]["code"], "unsupported_text")

    def test_corpus_read_and_enumeration_errors_never_return_partial_current(self):
        self.write("docs/adr/unreferenced.md", "# Evidence\n")
        self.snapshot(corpus_dirs=["docs/adr"])
        original_read = graph.Repository.read

        def unreadable(instance, path, limit):
            if path == "docs/adr/unreferenced.md":
                raise graph.GraphError("unreadable", path)
            return original_read(instance, path, limit)

        with patch.object(graph.Repository, "read", new=unreadable):
            self.assertEqual(self.main_error("check", "--graph", ".mastermind/research/snapshot.json")["code"], "unreadable")
        original_stat = graph.os.stat

        def unreadable_entry(path, *arguments, **keywords):
            if path == "unreferenced.md" and "dir_fd" in keywords:
                raise PermissionError(errno.EACCES, "denied")
            return original_stat(path, *arguments, **keywords)

        with patch.object(graph.os, "stat", new=unreadable_entry):
            self.assertEqual(self.main_error("check", "--graph", ".mastermind/research/snapshot.json"),
                             {"code": "unreadable", "path": "docs/adr/unreferenced.md"})

    def test_corpus_file_depth_entry_and_time_limits_fail_closed(self):
        repository = graph.Repository(self.root)
        self.addCleanup(repository.close)
        self.write("docs/adr/nested/extra.md", "# Extra\n")
        self.write("docs/adr/ignored.txt", "Ignored\n")
        self.write("docs/adr/.hidden", "Ignored\n")
        for name, limit, code in [("CORPUS_FILE_LIMIT", 1, "corpus_file_limit"),
                                  ("CORPUS_DEPTH_LIMIT", 0, "corpus_depth_limit"),
                                  ("CORPUS_ENTRY_LIMIT", 9, "corpus_entry_limit"),
                                  ("CORPUS_TIMEOUT", 0, "corpus_timeout")]:
            with self.subTest(name=name), patch.object(graph, name, limit):
                with self.assertRaisesRegex(graph.GraphError, code):
                    graph.snapshot(repository, "relations.json", ".mastermind/research/no.json", ["docs/adr"])
            self.assertFalse((self.root / ".mastermind/research/no.json").exists())
        # Even excluded entries consume the shared budget, before content reads.
        with patch.object(graph, "CORPUS_ENTRY_LIMIT", 2), patch.object(repository, "read") as read:
            with self.assertRaisesRegex(graph.GraphError, "corpus_entry_limit"):
                graph.capture_files(repository, [], ["docs/adr"])
            read.assert_not_called()

    def test_corpus_mid_enumeration_failure_is_not_an_empty_or_partial_inventory(self):
        self.snapshot(corpus_dirs=["docs/adr"])
        original_scandir = graph.os.scandir

        class InterruptedScan:
            def __init__(self, descriptor):
                self.entries = original_scandir(descriptor)

            def __enter__(self):
                return self

            def __exit__(self, *_arguments):
                self.entries.close()

            def __iter__(self):
                yield next(self.entries)
                raise PermissionError(errno.EACCES, "incomplete listing")

        with patch.object(graph.os, "scandir", new=InterruptedScan), \
                patch.object(graph.os, "supports_fd", os.supports_fd | {InterruptedScan}):
            self.assertEqual(self.main_error("check", "--graph", ".mastermind/research/snapshot.json"),
                             {"code": "unreadable", "path": "docs/adr"})

    def test_corpus_deadline_is_rechecked_after_content_read(self):
        repository = graph.Repository(self.root)
        self.addCleanup(repository.close)
        original_read = repository.read
        now = 0

        def slow_read(path, limit):
            nonlocal now
            result = original_read(path, limit)
            now = graph.CORPUS_TIMEOUT + 1
            return result

        with patch.object(graph.time, "monotonic", side_effect=lambda: now), \
                patch.object(repository, "read", side_effect=slow_read):
            with self.assertRaisesRegex(graph.GraphError, "corpus_timeout"):
                graph.capture_files(repository, [], ["docs/adr"])

    def test_corpus_byte_limit_counts_the_union_once_and_validates_saved_totals(self):
        self.write("docs/adr/unreferenced.md", "# New\n")
        _edges, paths = graph.validate_manifest(self.manifest)
        all_paths = set(paths) | {"docs/adr/unreferenced.md"}
        total = sum((self.root / path).stat().st_size for path in all_paths)
        repository = graph.Repository(self.root)
        self.addCleanup(repository.close)
        with patch.object(graph, "TOTAL_BYTE_LIMIT", total):
            saved = graph.snapshot(repository, "relations.json", ".mastermind/research/ok.json", ["docs/adr"])
            graph.validate_snapshot(saved)
        with patch.object(graph, "TOTAL_BYTE_LIMIT", total - 1):
            with self.assertRaisesRegex(graph.GraphError, "total_byte_limit"):
                graph.capture_files(repository, paths, ["docs/adr"])
            with self.assertRaisesRegex(graph.GraphError, "total_byte_limit"):
                graph.validate_snapshot(saved)
        self.write("docs/adr/unreferenced.md", "x" * (graph.FILE_BYTE_LIMIT + 1))
        self.assertEqual(self.check("ok.json", expected=2)["error"]["code"], "file_byte_limit")

    def test_corpus_shared_endpoint_and_late_added_file_cannot_mix_versions(self):
        self.write("unlisted.txt", "Already dirty\n")
        repository = graph.Repository(self.root)
        self.addCleanup(repository.close)
        original_read = repository.read
        for late_addition in [False, True]:
            reads = 0

            def race(path, limit):
                nonlocal reads
                result = original_read(path, limit)
                if path == "docs/adr/0001.md":
                    reads += 1
                    if reads == 2:
                        if late_addition:
                            self.write("docs/adr/late.md", "# Newly discovered decision\n")
                        else:
                            self.write(path, "# Decision\nUse another.\nProof.\n")
                return result

            with self.subTest(late_addition=late_addition), patch.object(repository, "read", side_effect=race):
                with self.assertRaisesRegex(graph.GraphError, "corpus_changed_during_operation"):
                    graph.snapshot(repository, "relations.json", ".mastermind/research/no.json", ["docs/adr"])
            self.assertEqual(reads, 2)
            self.assertFalse((self.root / ".mastermind/research/no.json").exists())

    def test_corpus_replacing_an_empty_directory_during_capture_is_an_error(self):
        (self.root / "docs/empty").mkdir()
        repository = graph.Repository(self.root)
        self.addCleanup(repository.close)
        original_read = repository.read
        moved = False

        def replace_directory(path, limit):
            nonlocal moved
            result = original_read(path, limit)
            if path == "docs/adr/0001.md" and not moved:
                moved = True
                (self.root / "docs/empty").rename(self.root / "docs/old-empty")
                (self.root / "docs/empty").mkdir()
            return result

        with patch.object(repository, "read", side_effect=replace_directory):
            with self.assertRaisesRegex(graph.GraphError, "corpus_changed_during_operation"):
                graph.capture_files(repository, ["docs/adr/0001.md"], ["docs/empty"])

    def test_corpus_interruption_returns_json_and_rolls_back_published_output(self):
        with patch.object(graph.Repository, "corpus_inventory", side_effect=KeyboardInterrupt):
            self.assertEqual(self.main_error("snapshot", "--relations", "relations.json", "--output",
                                             ".mastermind/research/no.json", "--corpus-dir", "docs/adr"),
                             {"code": "interrupted"})
        original_fsync = graph.os.fsync

        def interrupt_after_publication(descriptor):
            if (self.root / ".mastermind/research/no.json").exists():
                raise KeyboardInterrupt
            return original_fsync(descriptor)

        with patch.object(graph.os, "fsync", new=interrupt_after_publication):
            self.assertEqual(self.main_error("snapshot", "--relations", "relations.json", "--output",
                                             ".mastermind/research/no.json", "--corpus-dir", "docs/adr"),
                             {"code": "interrupted"})
        self.assertEqual(list((self.root / ".mastermind/research").iterdir()), [])

    def test_strict_manifest_rejects_unsafe_paths_duplicates_and_false_types(self):
        for path in ["../outside.md", "/tmp/outside.md", "docs/../outside.md", "docs//file.md",
                     "docs/./file.md", "docs\\file.md", ".git/config", ".env", "docs/.secret.md",
                     ".mastermind/mmcg.db", ".github/secrets.yml", "docs/evil\n.md"]:
            value = deepcopy(self.manifest)
            value["relations"][0]["from"]["path"] = path
            with self.subTest(path=path), self.assertRaises(graph.GraphError):
                graph.validate_manifest(value)
        variations = []
        value = deepcopy(self.manifest)
        value["authority"] = "verified"
        variations.append(value)
        value = deepcopy(self.manifest)
        value["schema_version"] = True
        variations.append(value)
        value = deepcopy(self.manifest)
        value["relations"][0]["to"]["line"] = True
        variations.append(value)
        value = deepcopy(self.manifest)
        value["relations"][0]["confidence"] = 1
        variations.append(value)
        value = deepcopy(self.manifest)
        value["relations"].append(deepcopy(value["relations"][0]))
        variations.append(value)
        value = deepcopy(self.manifest)
        value["relations"][0]["from"]["path"] = "src/handler.py"
        variations.append(value)
        for value in variations:
            with self.subTest(value=value), self.assertRaises(graph.GraphError):
                graph.validate_manifest(value)
        self.write("relations.json", '{"schema_version":1,"schema_version":1,"relations":[]}')
        result = self.cli("snapshot", "--relations", "relations.json", "--output", ".mastermind/research/no.json", expected=2)
        self.assertEqual(result["error"]["code"], "duplicate_json_key")

    def test_snapshot_integrity_and_root_binding_are_strict(self):
        original = self.snapshot()
        cases = []
        value = deepcopy(original)
        value["sha256"] = "0" * 64
        cases.append((value, "snapshot_digest_mismatch"))
        value = deepcopy(original)
        value["authority"] = "approved"
        cases.append((value, "invalid_schema"))
        value = deepcopy(original)
        value["edges"][0]["verification"] = "verified"
        cases.append((value, "invalid_verification"))
        value = deepcopy(original)
        value["edges"][0]["id"] = "0" * 64
        cases.append((value, "invalid_edge_identity_or_order"))
        value = deepcopy(original)
        value["root"] = str(self.root.parent)
        cases.append((value, "root_binding_mismatch"))
        for index, (value, code) in enumerate(cases):
            if code != "snapshot_digest_mismatch":
                value["sha256"] = graph.digest({key: item for key, item in value.items() if key != "sha256"})
            self.write(f".mastermind/research/forged-{index}.json", json.dumps(value))
            result = self.check(f"forged-{index}.json", expected=2)
            self.assertEqual(result["error"]["code"], code)
        result = self.cli("check", "--graph", "../.mastermind/research/snapshot.json", expected=2, root=self.root / "docs")
        self.assertEqual(result["error"]["code"], "unsafe_path")
        result = self.cli("check", "--graph", "graph.json", expected=2, root=self.root / "docs")
        self.assertEqual(result["error"]["code"], "not_repository_root")

    def test_symlink_and_nonregular_drift_never_reads_the_target(self):
        self.snapshot()
        source = self.root / "src/handler.py"
        source.unlink()
        with tempfile.TemporaryDirectory(prefix="document-graph-outside-") as outside:
            outside = Path(outside)
            secret = outside / "private.txt"
            secret.write_text("s" * (graph.FILE_BYTE_LIMIT + 1))
            source.symlink_to(secret)
            checked = self.check(expected=1)
            self.assertEqual(checked["changed_files"], [{"path": "src/handler.py", "reasons": ["unsafe_path"]}])
            result = self.cli("snapshot", "--relations", "relations.json", "--output", ".mastermind/research/rejected.json", expected=2)
            self.assertEqual(result["error"]["code"], "unsafe_path")
            source.unlink()
            os.mkfifo(source)
            checked = self.check(expected=1)
            self.assertEqual(checked["changed_files"][0]["reasons"], ["not_regular"])
            source.unlink()
            source.mkdir()
            checked = self.check(expected=1)
            self.assertEqual(checked["changed_files"][0]["reasons"], ["not_regular"])

    def test_output_cannot_overwrite_files_or_follow_symlink_components(self):
        snapshot = self.snapshot()
        result = self.cli("snapshot", "--relations", "relations.json", "--output", ".mastermind/research/snapshot.json", expected=2)
        self.assertEqual(result["error"]["code"], "output_exists")
        self.assertEqual(json.loads((self.root / ".mastermind/research/snapshot.json").read_text()), snapshot)
        for output in ["relations.json", "src/handler.py", ".mastermind/tasks/output.json"]:
            result = self.cli("snapshot", "--relations", "relations.json", "--output", output, expected=2)
            self.assertEqual(result["error"]["code"], "unsafe_output")
        with tempfile.TemporaryDirectory(prefix="document-graph-output-") as outside:
            (self.root / ".mastermind/research/link").symlink_to(outside, target_is_directory=True)
            result = self.cli("snapshot", "--relations", "relations.json", "--output", ".mastermind/research/link/no.json", expected=2)
            self.assertEqual(result["error"]["code"], "unsafe_path")
            self.assertEqual(list(Path(outside).iterdir()), [])
        self.assertEqual(list((self.root / ".mastermind/research").glob(".document-graph-*")), [])
        (self.root / "relations-link.json").symlink_to("relations.json")
        result = self.cli("snapshot", "--relations", "relations-link.json", "--output", ".mastermind/research/no.json", expected=2)
        self.assertEqual(result["error"]["code"], "unsafe_path")

    def test_limits_are_enforced_before_unbounded_work(self):
        value = deepcopy(self.manifest)
        value["relations"] = value["relations"][:1] * (graph.RELATION_LIMIT + 1)
        with self.assertRaises(graph.GraphError):
            graph.validate_manifest(value)
        value["relations"] = [{"from": {"path": "docs/adr/0001.md", "line": 1}, "relation": "mentions",
                               "to": {"path": f"src/file-{index}.py", "line": 1}}
                              for index in range(graph.FILE_LIMIT)]
        with self.assertRaisesRegex(graph.GraphError, "file_count_limit"):
            graph.validate_manifest(value)
        self.write("oversized.json", " " * (graph.MANIFEST_LIMIT + 1))
        result = self.cli("snapshot", "--relations", "oversized.json", "--output", ".mastermind/research/no.json", expected=2)
        self.assertEqual(result["error"]["code"], "file_byte_limit")
        self.snapshot()
        self.write("src/handler.py", "x" * (graph.FILE_BYTE_LIMIT + 1))
        result = self.check(expected=2)
        self.assertEqual(result["error"]["code"], "file_byte_limit")
        repository = graph.Repository(self.root)
        self.addCleanup(repository.close)
        with patch.object(graph, "TOTAL_BYTE_LIMIT", 8):
            with self.assertRaisesRegex(graph.GraphError, "total_byte_limit"):
                graph.inspect_files(repository, ["docs/adr/0001.md"])

    def test_races_and_unreadable_files_fail_closed(self):
        self.snapshot()
        repository = graph.Repository(self.root)
        self.addCleanup(repository.close)
        original = repository.read
        changed = False

        def race(path, limit):
            nonlocal changed
            result = original(path, limit)
            if path == "src/handler.py" and not changed:
                changed = True
                self.write(path, "def handler():\n    return 2\n")
            return result

        with patch.object(repository, "read", side_effect=race):
            with self.assertRaisesRegex(graph.GraphError, "files_changed_during_snapshot"):
                graph.check(repository, ".mastermind/research/snapshot.json")
        original_read = graph.Repository.read

        def unreadable(instance, path, limit):
            if path == "src/handler.py":
                raise graph.GraphError("unreadable", path)
            return original_read(instance, path, limit)

        output = io.StringIO()
        with patch.object(graph.Repository, "read", new=unreadable), redirect_stdout(output):
            code = graph.main(["check", "--root", str(self.root), "--graph", ".mastermind/research/snapshot.json"])
        self.assertEqual(code, 2)
        self.assertEqual(json.loads(output.getvalue())["error"]["code"], "unreadable")

    def test_git_metadata_does_not_execute_configured_filters_or_monitors(self):
        self.write(".gitattributes", "src/handler.py filter=probe\n")
        self.git("add", ".gitattributes")
        self.git("-c", "core.hooksPath=/dev/null", "commit", "-qm", "attribute")
        self.git("config", "filter.probe.clean", "touch filter-ran; cat")
        self.git("config", "filter.probe.process", "touch process-ran; exit 1")
        self.git("config", "core.fsmonitor", "touch monitor-ran")
        self.git("config", "alias.status", "!touch alias-ran")
        source = self.root / "src/handler.py"
        source.write_text(source.read_text().replace("return 1", "return 2"))
        os.utime(source, (946684800, 946684800))
        for marker in ["filter-ran", "process-ran", "monitor-ran", "alias-ran"]:
            self.assertFalse((self.root / marker).exists(), marker)
        self.snapshot()
        self.check()
        for marker in ["filter-ran", "process-ran", "monitor-ran", "alias-ran"]:
            self.assertFalse((self.root / marker).exists(), marker)

    def test_git_limits_and_root_replacement_fail_closed(self):
        repository = graph.Repository(self.root)
        self.addCleanup(repository.close)
        with patch.object(graph, "GIT_OUTPUT_LIMIT", 8):
            with self.assertRaisesRegex(graph.GraphError, "git_output_limit"):
                repository.git(["rev-parse", "HEAD"])
        with patch.object(graph, "GIT_TIMEOUT", 0):
            with self.assertRaisesRegex(graph.GraphError, "git_timeout"):
                repository.git(["rev-parse", "HEAD"])
        replacement = self.root.with_name(self.root.name + "-moved")
        self.root.rename(replacement)
        self.root.mkdir()
        try:
            with self.assertRaisesRegex(graph.GraphError, "root_changed_during_operation"):
                repository.read("relations.json", graph.MANIFEST_LIMIT)
            with self.assertRaisesRegex(graph.GraphError, "root_changed_during_operation"):
                repository.revision()
        finally:
            self.root.rmdir()
            replacement.rename(self.root)

    def test_missing_promised_git_objects_do_not_trigger_fetch_or_object_writes(self):
        with tempfile.TemporaryDirectory(prefix="document-graph-promisor-") as temporary:
            remote = Path(temporary) / "remote.git"
            marker = Path(temporary) / "uploadpack-ran"
            self.git("clone", "--bare", "--quiet", str(self.root), str(remote))
            tree = self.git("rev-parse", "HEAD^{tree}").strip()
            objects = self.root / ".git/objects"
            (objects / tree[:2] / tree[2:]).unlink()
            for name, value in {
                "remote.origin.url": str(remote),
                "remote.origin.promisor": "true",
                "remote.origin.partialclonefilter": "blob:none",
                "extensions.partialClone": "origin",
                "remote.origin.uploadpack": f"touch {shlex.quote(str(marker))}; git-upload-pack",
                "protocol.file.allow": "always",
            }.items():
                self.git("config", name, value)
            before = {str(path.relative_to(objects)): path.stat().st_mtime_ns for path in objects.rglob("*")}
            result = self.cli("snapshot", "--relations", "relations.json", "--output", ".mastermind/research/no.json", expected=2)
            self.assertEqual(result["error"]["code"], "git_failed")
            self.assertFalse(marker.exists())
            self.assertEqual(list((objects / "pack").iterdir()), [])
            self.assertEqual({str(path.relative_to(objects)): path.stat().st_mtime_ns for path in objects.rglob("*")}, before)

    def test_replaced_source_parent_cannot_produce_false_current(self):
        self.write("unlisted.txt", "keep dirty metadata stable\n")
        self.snapshot()
        repository = graph.Repository(self.root)
        self.addCleanup(repository.close)
        original = graph.os.open
        opens = 0
        with tempfile.TemporaryDirectory(prefix="document-graph-detached-read-") as temporary:
            detached = Path(temporary) / "src"

            def replace_source(path, *arguments, **keywords):
                nonlocal opens
                if path == "handler.py" and "dir_fd" in keywords:
                    opens += 1
                    if opens == 2:
                        (self.root / "src").rename(detached)
                        self.write("src/handler.py", "def handler():\n    return 2\n")
                return original(path, *arguments, **keywords)

            with patch.object(graph.os, "open", new=replace_source):
                with self.assertRaisesRegex(graph.GraphError, "parent_changed_during_operation"):
                    graph.check(repository, ".mastermind/research/snapshot.json")
            self.assertEqual(opens, 2)

    def test_detached_output_parent_does_not_publish_a_successful_snapshot(self):
        repository = graph.Repository(self.root)
        self.addCleanup(repository.close)
        original = graph.os.link
        with tempfile.TemporaryDirectory(prefix="document-graph-detached-write-") as temporary:
            detached = Path(temporary) / "research"

            def replace_output(*arguments, **keywords):
                (self.root / ".mastermind/research").rename(detached)
                (self.root / ".mastermind/research").mkdir()
                return original(*arguments, **keywords)

            with patch.object(graph.os, "link", new=replace_output):
                with self.assertRaisesRegex(graph.GraphError, "parent_changed_during_operation"):
                    graph.snapshot(repository, "relations.json", ".mastermind/research/no.json")
            self.assertFalse((self.root / ".mastermind/research/no.json").exists())
            self.assertEqual(list(detached.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
