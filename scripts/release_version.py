#!/usr/bin/env python3
"""Read, check and bump the release version in every file that carries it.

The crate version in mcp/servers/mmcg/Cargo.toml is the source of truth.

    python3 scripts/release_version.py check
    python3 scripts/release_version.py bump 4.0.0 [--date 2026-11-25]
    python3 scripts/release_version.py notes 4.0.0
"""

from __future__ import annotations

import argparse
import datetime
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SOURCE = "mcp/servers/mmcg/Cargo.toml"
SEMVER = r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?"
PLATFORMS = (
    "darwin-arm64",
    "darwin-x64",
    "linux-x64-gnu",
    "linux-arm64-gnu",
    "linux-x64-musl",
    "linux-arm64-musl",
    "win32-x64-msvc",
)

# Each pattern has one group for the version and must match the expected
# number of times, so a moved or duplicated line is reported instead of skipped.
LOCATIONS: tuple[tuple[str, str, int], ...] = (
    (SOURCE, rf'(?m)^version = "({SEMVER})"$', 1),
    ("mcp/servers/mmcg/Cargo.lock", rf'name = "mmcg"\nversion = "({SEMVER})"', 1),
    ("mcp/servers/mmcg/README.md", rf"(?m)^  version: ({SEMVER})$", 1),
    ("README.md", rf"badge/npm-v({SEMVER})-CB3837", 1),
    ("README.md", rf'alt="npm version ({SEMVER})"', 1),
    ("npm/mastermind/README.md", rf"badge/npm-v({SEMVER})-CB3837", 1),
    ("npm/mastermind/README.md", rf"\[!\[npm version ({SEMVER})\]", 1),
    ("npm/mastermind/package.json", rf'(?m)^  "version": "({SEMVER})",$', 1),
    ("npm/mastermind/package.json", rf'"@xcraftmind/mmcg-[a-z0-9-]+": "({SEMVER})"', len(PLATFORMS)),
    *((f"npm/platforms/{name}/package.json", rf'(?m)^  "version": "({SEMVER})",$', 1) for name in PLATFORMS),
    ("docs/examples/mastermind-review-pr.yml", rf"@xcraftmind/mastermind@({SEMVER})", 1),
    ("mcp/servers/mmcg/assets/mastermind-review-pr.yml", rf"@xcraftmind/mastermind@({SEMVER})", 1),
)


def read(root: Path, relative: str) -> str:
    return (root / relative).read_text(encoding="utf-8")


def source_version(root: Path = REPO_ROOT) -> str:
    match = re.search(LOCATIONS[0][1], read(root, SOURCE))
    if not match:
        raise ValueError(f"{SOURCE}: package version not found")
    return match.group(1)


def mismatches(root: Path = REPO_ROOT) -> list[tuple[str, str]]:
    """Return (path, message) for every location that disagrees with Cargo.toml."""
    expected = source_version(root)
    problems = []
    for relative, pattern, count in LOCATIONS:
        try:
            found = [m.group(1) for m in re.finditer(pattern, read(root, relative))]
        except OSError as error:
            problems.append((relative, f"cannot read release version: {error}"))
            continue
        if len(found) != count:
            problems.append((relative, f"expected {count} release version line(s), found {len(found)}"))
        for version in found:
            if version != expected:
                problems.append((relative, f"release version {version} differs from {SOURCE} {expected}"))
    return problems


def bump_changelog(text: str, old: str, new: str, date: str) -> str:
    heading = "## [Unreleased]\n"
    if text.count(heading) != 1:
        raise ValueError("CHANGELOG.md must have exactly one [Unreleased] heading")
    if f"## [{new}]" in text:
        raise ValueError(f"CHANGELOG.md already has a {new} section")
    text = text.replace(heading, f"{heading}\n## [{new}] - {date}\n", 1)
    link = f"[Unreleased]: https://github.com/xcrft/mastermind/compare/npm-v{old}...HEAD\n"
    if link not in text:
        raise ValueError(f"CHANGELOG.md has no [Unreleased] link from npm-v{old}")
    return text.replace(
        link,
        f"[Unreleased]: https://github.com/xcrft/mastermind/compare/npm-v{new}...HEAD\n"
        f"[{new}]: https://github.com/xcrft/mastermind/releases/tag/npm-v{new}\n",
        1,
    )


def release_notes(text: str, version: str) -> str:
    """Return the body of one CHANGELOG section, without its heading."""
    match = re.search(rf"(?ms)^## \[{re.escape(version)}\][^\n]*\n(.*?)(?=^## \[|^\[Unreleased\]: )", text)
    if not match or not match.group(1).strip():
        raise ValueError(f"CHANGELOG.md has no notes for {version}")
    return match.group(1).strip() + "\n"


def bump(root: Path, new: str, date: str) -> list[str]:
    if not re.fullmatch(SEMVER, new):
        raise ValueError(f"not a SemVer version: {new!r}")
    problems = mismatches(root)
    if problems:
        raise ValueError("versions disagree before the bump:\n" + "\n".join(f"{p}: {m}" for p, m in problems))
    old = source_version(root)
    if old == new:
        raise ValueError(f"version is already {new}")
    updated: dict[str, str] = {}
    for relative, pattern, _ in LOCATIONS:
        text = updated.get(relative, read(root, relative))
        updated[relative] = re.sub(
            pattern, lambda m: m.group(0).replace(m.group(1), new), text
        )
    updated["CHANGELOG.md"] = bump_changelog(read(root, "CHANGELOG.md"), old, new, date)
    # Write only after every file was transformed, so a bad input leaves no half bump.
    for relative, text in updated.items():
        (root / relative).write_text(text, encoding="utf-8")
    return sorted(updated)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check")
    bump_parser = commands.add_parser("bump")
    bump_parser.add_argument("version")
    bump_parser.add_argument("--date", default=datetime.date.today().isoformat())
    commands.add_parser("notes").add_argument("version")
    args = parser.parse_args(argv)
    try:
        if args.command == "notes":
            print(release_notes(read(REPO_ROOT, "CHANGELOG.md"), args.version), end="")
            return 0
        if args.command == "check":
            problems = mismatches()
            for path, message in problems:
                print(f"{path}: {message}", file=sys.stderr)
            if not problems:
                print(f"release version {source_version()} is consistent")
            return 1 if problems else 0
        for path in bump(REPO_ROOT, args.version, args.date):
            print(f"updated {path}")
        return 0
    except ValueError as error:
        print(error, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
