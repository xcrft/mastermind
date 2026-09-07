"""Document relationship contracts; stdlib only, no model or indexer calls."""

from contextlib import redirect_stdout
from copy import deepcopy
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

    def snapshot(self, name="snapshot.json"):
        return self.cli("snapshot", "--relations", "relations.json", "--output",
                        f".mastermind/research/{name}")

    def check(self, name="snapshot.json", expected=0):
        return self.cli("check", "--graph", f".mastermind/research/{name}", expected=expected)

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
        self.assertTrue(checked["revision"]["dirty"])
        self.git("add", "docs/adr/0002.md")
        self.git("-c", "core.hooksPath=/dev/null", "commit", "-qm", "new decision")
        checked = self.check()
        self.assertTrue(checked["revision_changed"])
        self.assertEqual(checked["status"], "current")

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
