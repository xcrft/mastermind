"""Corpus membership oracles against real disposable Git repositories."""

import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest

from evals import persona_replay as replay


@unittest.skipUnless(os.name == "posix" and shutil.which("git"), "requires POSIX and Git")
class PersonaCorpusTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.home = self.root / "home"
        self.home.mkdir()
        self.repo = self.root / "source"
        self.repo.mkdir()
        self.git("init", "-q")
        self.git("config", "user.name", "Example")
        self.git("config", "user.email", "example@example.invalid")
        self.first = self.commit("first")

    def git(self, *args):
        return replay.git(self.repo, self.home, *args).decode().strip()

    def commit(self, text):
        (self.repo / "source.py").write_text(f"VALUE = {text!r}\n")
        self.git("add", "source.py")
        self.git("commit", "-qm", text)
        return self.git("rev-parse", "HEAD")

    def snapshot(self, name, frozen=None):
        output = self.root / name
        output.mkdir()
        return replay.corpus(self.repo, output, self.home, "HEAD", frozen)

    def test_single_tip_keeps_the_real_root_without_inventing_an_author(self):
        corpus, rows, manifest = self.snapshot("one")
        self.assertFalse(manifest["synthetic_merge"])
        self.assertEqual(manifest["snapshot"], self.first)
        self.assertEqual({r["sha"] for r in rows}, {self.first})
        self.assertEqual(manifest["non_merge_commits"], 1)
        self.assertEqual(manifest["raw_author_emails"], 1)
        self.assertEqual(replay.git(corpus, self.home, "rev-list", "--no-merges", "HEAD").decode().strip(), self.first)

    def test_union_covers_unmerged_commits_and_skips_only_confirmed_noncommits(self):
        self.git("branch", "side")
        second = self.commit("second")
        self.git("checkout", "-q", "side")
        side = self.commit("side")
        self.git("tag", "-a", "-m", "commit tag", "commit-tag", second)
        tree = self.git("rev-parse", "HEAD^{tree}")
        self.git("tag", "-a", "-m", "tree tag", "tree-tag", tree)
        source_refs = self.git("show-ref")
        corpus, rows, manifest = self.snapshot("union")
        self.assertTrue(manifest["synthetic_merge"])
        self.assertEqual({r["sha"] for r in rows}, {self.first, second, side})
        self.assertEqual(manifest["skipped_refs"], ["refs/tags/tree-tag"])
        selected = replay.git(corpus, self.home, "rev-list", "--no-merges", "HEAD").decode().splitlines()
        self.assertEqual(set(selected), {self.first, second, side})
        self.assertEqual(self.git("show-ref"), source_refs)

    def test_frozen_membership_survives_new_refs_and_rejects_digest_drift(self):
        _, rows, manifest = self.snapshot("initial")
        frozen = self.root / "manifest.json"
        frozen.write_bytes(replay.encoded(manifest))
        self.commit("later")
        _, repeated, repeated_manifest = self.snapshot("frozen", frozen)
        self.assertEqual(rows, repeated)
        self.assertEqual(manifest["commit_set_sha256"], repeated_manifest["commit_set_sha256"])
        manifest["commit_set_sha256"] = "0" * 64
        frozen.write_bytes(replay.encoded(manifest))
        with self.assertRaisesRegex(ValueError, "membership changed"):
            self.snapshot("mismatch", frozen)

    def test_missing_ref_object_fails_instead_of_shrinking_the_corpus(self):
        _, _, manifest = self.snapshot("initial")
        manifest["refs"].append("refs/heads/missing " + "1" * 40)
        frozen = self.root / "missing.json"
        frozen.write_text(json.dumps(manifest))
        with self.assertRaises(RuntimeError):
            self.snapshot("missing", frozen)


if __name__ == "__main__":
    unittest.main()
