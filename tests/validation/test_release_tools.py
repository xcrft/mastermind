"""Release helpers: one version bump across every file and the CI gate before publishing."""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from scripts import release_version

ROOT = Path(__file__).resolve().parents[2]
GATE = ROOT / "scripts/require-green-ci.sh"


def copy_release_files(target: Path) -> None:
    for relative in {path for path, _, _ in release_version.LOCATIONS} | {"CHANGELOG.md"}:
        destination = target / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / relative, destination)


class ReleaseVersionTests(unittest.TestCase):
    def test_bump_moves_every_location_and_the_changelog(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            copy_release_files(root)
            notes = "### Fixed\n\n- A release note.\n"
            source = root / "CHANGELOG.md"
            source.write_text(
                # Replace whatever the real Unreleased section holds, so the test owns its notes.
                re.sub(
                    r"(?ms)^## \[Unreleased\]\n.*?(?=^## \[)",
                    lambda _: f"## [Unreleased]\n\n{notes}\n",
                    source.read_text(encoding="utf-8"),
                    count=1,
                ),
                encoding="utf-8",
            )
            old = release_version.source_version(root)
            release_version.bump(root, "99.0.0", "2026-01-02")
            self.assertEqual(release_version.mismatches(root), [])
            self.assertEqual(release_version.source_version(root), "99.0.0")
            changelog = (root / "CHANGELOG.md").read_text(encoding="utf-8")
            self.assertIn("## [Unreleased]\n\n## [99.0.0] - 2026-01-02\n", changelog)
            self.assertIn("compare/npm-v99.0.0...HEAD", changelog)
            self.assertIn(f"[{old}]: ", changelog)
            self.assertEqual(release_version.release_notes(changelog, "99.0.0"), notes)
            with self.assertRaisesRegex(ValueError, "no notes for 98.0.0"):
                release_version.release_notes(changelog, "98.0.0")
            for relative in {path for path, _, _ in release_version.LOCATIONS}:
                self.assertNotIn(f'"{old}"', (root / relative).read_text(encoding="utf-8"), relative)

    def test_drift_is_named_and_blocks_the_bump_without_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            copy_release_files(root)
            platform = root / "npm/platforms/linux-x64-gnu/package.json"
            platform.write_text(
                platform.read_text(encoding="utf-8").replace(
                    f'"version": "{release_version.source_version(root)}"', '"version": "0.0.1"'
                ),
                encoding="utf-8",
            )
            problems = release_version.mismatches(root)
            self.assertEqual([path for path, _ in problems], ["npm/platforms/linux-x64-gnu/package.json"])
            before = (root / release_version.SOURCE).read_bytes()
            with self.assertRaisesRegex(ValueError, "disagree before the bump"):
                release_version.bump(root, "99.0.0", "2026-01-02")
            self.assertEqual((root / release_version.SOURCE).read_bytes(), before)


class RequireGreenCiTests(unittest.TestCase):
    """The fake gh prints FAKE_<NAME> for runs of FAKE_EVENT (default push), and no run otherwise."""

    def run_gate(
        self, results: dict[str, str], *args: str, event: str = "push"
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as directory:
            fake = Path(directory) / "gh"
            fake.write_text(
                "#!/usr/bin/env bash\n"
                'for a; do case "$prev" in --workflow) wf=$a;; --event) ev=$a;; esac; prev=$a; done\n'
                'key=FAKE_${wf%.yml}; key=${key//-/_}\n'
                'if [ "$ev" = "$FAKE_EVENT" ]; then printf "%s\\n" "${!key}"; else echo "null null"; fi\n',
                encoding="utf-8",
            )
            fake.chmod(0o755)
            env = {
                "PATH": f"{directory}:{os.environ['PATH']}",
                "CI_WAIT_SECONDS": "0",
                "CI_POLL_SECONDS": "0",
                "FAKE_EVENT": event,
            }
            env.update({f"FAKE_{name.replace('-', '_')}": value for name, value in results.items()})
            return subprocess.run(
                ["bash", str(GATE), "owner/repo", "abc123", *args], env=env, capture_output=True, text=True
            )

    ALL_GREEN = {name: "completed success" for name in ("ci-mmcg", "ci-npm", "validate", "supply-chain")}

    def test_green_ci_passes(self):
        result = self.run_gate(self.ALL_GREEN)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_failed_workflow_fails_and_is_named(self):
        result = self.run_gate({**self.ALL_GREEN, "ci-mmcg": "completed failure"})
        self.assertEqual(result.returncode, 1)
        self.assertIn("ci-mmcg.yml finished as 'failure'", result.stderr)

    def test_unfinished_ci_fails_at_the_deadline(self):
        result = self.run_gate({**self.ALL_GREEN, "validate": "in_progress "})
        self.assertEqual(result.returncode, 1)
        self.assertIn("did not finish", result.stderr)
        self.assertIn("validate.yml", result.stderr)

    def test_dispatch_dry_runs_are_read_for_their_own_event_and_workflows(self):
        dry_runs = {"publish-mmcg": "completed success", "publish-npm": "completed success"}
        args = ("workflow_dispatch", "publish-mmcg.yml", "publish-npm.yml")
        self.assertEqual(self.run_gate(dry_runs, *args, event="workflow_dispatch").returncode, 0)
        result = self.run_gate(dry_runs, *args, event="push")
        self.assertEqual(result.returncode, 1)
        self.assertIn("publish-mmcg.yml publish-npm.yml", result.stderr)

    def test_an_event_without_workflows_is_a_usage_error(self):
        self.assertEqual(self.run_gate(self.ALL_GREEN, "workflow_dispatch").returncode, 2)


if __name__ == "__main__":
    unittest.main()
