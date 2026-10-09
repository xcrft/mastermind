"""Repository documentation discovery, excluding generated local evidence."""

from pathlib import Path
from unittest.mock import patch
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
