#!/usr/bin/env python3
"""
Sync local documentation stubs with their current online versions.

See ../SKILL.md for the workflow this script implements.
Run with --help for CLI usage.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import os
import re
import sys
import tempfile
import time
from collections import defaultdict
from pathlib import Path
from typing import Optional
from urllib.parse import urlparse

import requests
from bs4 import BeautifulSoup

DEFAULT_STUB_PATTERN = r"Fetch live documentation:\s*(https?://\S+)"
DEFAULT_TIMEOUT_SECONDS = 15
DEFAULT_RATE_LIMIT_SECONDS = 1.0
USER_AGENT = "mastermind-doc-stub-sync/0.1 (+https://github.com/aglumova/mastermind)"


@dataclasses.dataclass
class StubFile:
    path: Path
    url: str


@dataclasses.dataclass
class UpdateResult:
    path: str
    url: str
    status: str  # "updated" | "current" | "unreachable" | "error"
    old_hash: Optional[str] = None
    new_hash: Optional[str] = None
    delta_bytes: int = 0
    reason: Optional[str] = None


def find_stubs(root: Path, pattern: re.Pattern) -> list[StubFile]:
    stubs: list[StubFile] = []
    for path in root.rglob("*.md"):
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, PermissionError):
            continue
        match = pattern.search(text)
        if match:
            stubs.append(StubFile(path=path, url=match.group(1).strip()))
    return stubs


def normalize_text(text: str) -> str:
    """Strip line-end whitespace and collapse blank runs for stable hashing."""
    lines = [line.rstrip() for line in text.splitlines()]
    out: list[str] = []
    prev_blank = False
    for line in lines:
        if not line:
            if not prev_blank:
                out.append("")
            prev_blank = True
        else:
            out.append(line)
            prev_blank = False
    return "\n".join(out).strip() + "\n"


def content_hash(text: str) -> str:
    return hashlib.sha256(normalize_text(text).encode("utf-8")).hexdigest()


def local_body(text: str, pattern: re.Pattern) -> str:
    """The local file's body, with the stub line removed for fair comparison."""
    return pattern.sub("", text).strip()


def extract_main(html: str) -> tuple[str, str]:
    """Return (title, main-text) extracted from the upstream HTML."""
    soup = BeautifulSoup(html, "html.parser")
    for tag in soup(["script", "style", "noscript", "iframe", "nav", "footer"]):
        tag.decompose()
    main = soup.find("main") or soup.find("article") or soup.find("body") or soup
    text = main.get_text(separator="\n")
    title_tag = soup.find("title")
    title = (
        title_tag.get_text().split("|")[0].strip()
        if title_tag
        else urlparse(soup.find("link", rel="canonical").get("href", ""))
        .path.rstrip("/")
        .rsplit("/", 1)[-1]
        if soup.find("link", rel="canonical")
        else ""
    )
    return title, text


def fetch(url: str, timeout: float) -> requests.Response:
    return requests.get(url, timeout=timeout, headers={"User-Agent": USER_AGENT})


def fetch_with_retry(url: str, timeout: float) -> requests.Response:
    try:
        return fetch(url, timeout)
    except requests.exceptions.RequestException:
        time.sleep(2)
        return fetch(url, timeout)


def build_updated_content(title: str, body_text: str, url: str) -> str:
    header = f"# {title}\n\n" if title else ""
    return f"{header}{body_text.strip()}\n\n---\n\nFetch live documentation: {url}\n"


def write_atomic(path: Path, content: str) -> None:
    tmp_fd, tmp_path = tempfile.mkstemp(dir=path.parent, prefix=".tmp-", suffix=".md")
    try:
        with os.fdopen(tmp_fd, "w", encoding="utf-8") as f:
            f.write(content)
        os.replace(tmp_path, path)
    except Exception:
        if os.path.exists(tmp_path):
            os.unlink(tmp_path)
        raise


def process_stub(
    stub: StubFile,
    pattern: re.Pattern,
    timeout: float,
    dry_run: bool,
) -> UpdateResult:
    try:
        response = fetch_with_retry(stub.url, timeout)
    except requests.exceptions.RequestException as exc:
        return UpdateResult(
            path=str(stub.path),
            url=stub.url,
            status="unreachable",
            reason=f"request failed: {exc}",
        )

    if response.status_code >= 400:
        return UpdateResult(
            path=str(stub.path),
            url=stub.url,
            status="unreachable",
            reason=f"{response.status_code} {response.reason}",
        )

    title, upstream_text = extract_main(response.text)
    new_content = build_updated_content(title, upstream_text, stub.url)
    new_hash = content_hash(new_content)

    try:
        old_text = stub.path.read_text(encoding="utf-8")
    except Exception as exc:
        return UpdateResult(
            path=str(stub.path), url=stub.url, status="error", reason=str(exc)
        )

    old_hash = content_hash(local_body(old_text, pattern))
    cmp_hash = content_hash(local_body(new_content, pattern))

    if old_hash == cmp_hash:
        return UpdateResult(
            path=str(stub.path),
            url=stub.url,
            status="current",
            old_hash=old_hash,
            new_hash=cmp_hash,
        )

    delta = len(new_content.encode("utf-8")) - len(old_text.encode("utf-8"))
    if not dry_run:
        try:
            write_atomic(stub.path, new_content)
        except Exception as exc:
            return UpdateResult(
                path=str(stub.path), url=stub.url, status="error", reason=str(exc)
            )
    return UpdateResult(
        path=str(stub.path),
        url=stub.url,
        status="updated",
        old_hash=old_hash,
        new_hash=cmp_hash,
        delta_bytes=delta,
    )


def run(
    root: Path,
    stub_pattern: str,
    rate_limit_seconds: float,
    timeout_seconds: float,
    dry_run: bool,
) -> dict:
    pattern = re.compile(stub_pattern)
    stubs = find_stubs(root, pattern)
    if not stubs:
        return {
            "summary": {"scanned": 0, "updated": 0, "skipped_current": 0, "unreachable": 0, "errors": 0, "duration_seconds": 0},
            "updated": [],
            "unreachable": [],
            "errors": [],
        }

    started = time.monotonic()
    last_request_per_host: dict[str, float] = defaultdict(float)
    results: list[UpdateResult] = []

    for stub in stubs:
        host = urlparse(stub.url).netloc
        wait = (last_request_per_host[host] + rate_limit_seconds) - time.monotonic()
        if wait > 0:
            time.sleep(wait)
        last_request_per_host[host] = time.monotonic()
        results.append(process_stub(stub, pattern, timeout_seconds, dry_run))

    summary = {
        "scanned": len(results),
        "updated": sum(1 for r in results if r.status == "updated"),
        "skipped_current": sum(1 for r in results if r.status == "current"),
        "unreachable": sum(1 for r in results if r.status == "unreachable"),
        "errors": sum(1 for r in results if r.status == "error"),
        "duration_seconds": round(time.monotonic() - started, 1),
        "dry_run": dry_run,
    }
    return {
        "summary": summary,
        "updated": [dataclasses.asdict(r) for r in results if r.status == "updated"],
        "unreachable": [dataclasses.asdict(r) for r in results if r.status == "unreachable"],
        "errors": [dataclasses.asdict(r) for r in results if r.status == "error"],
    }


def cli(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Sync local doc stubs with online sources.")
    parser.add_argument("target_dir", type=Path, help="Root directory containing stub markdown files.")
    parser.add_argument("--stub-pattern", default=DEFAULT_STUB_PATTERN,
                        help="Regex with one capture group for the URL. Default: %(default)r")
    parser.add_argument("--rate-limit", type=float, default=DEFAULT_RATE_LIMIT_SECONDS,
                        help="Minimum seconds between requests to the same host. Default: %(default)s")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT_SECONDS,
                        help="Per-request timeout in seconds. Default: %(default)s")
    parser.add_argument("--dry-run", action="store_true",
                        help="Detect changes but do not write files. Report what would change.")
    parser.add_argument("--report-file", type=Path,
                        help="If set, write the JSON report to this path.")
    args = parser.parse_args(argv)

    if not args.target_dir.is_dir():
        print(f"error: target_dir is not a directory: {args.target_dir}", file=sys.stderr)
        return 2

    report = run(
        root=args.target_dir,
        stub_pattern=args.stub_pattern,
        rate_limit_seconds=args.rate_limit,
        timeout_seconds=args.timeout,
        dry_run=args.dry_run,
    )

    output_json = json.dumps(report, indent=2)
    print(output_json)
    if args.report_file:
        args.report_file.write_text(output_json + "\n", encoding="utf-8")

    return 0 if report["summary"]["errors"] == 0 else 1


if __name__ == "__main__":
    raise SystemExit(cli(sys.argv[1:]))
