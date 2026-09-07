"""Deterministic source-location checks, not a semantic judge of research."""

import re
from pathlib import Path
from urllib.parse import unquote


_LOCATION_SUFFIX = r":(?P<start>[0-9]+)(?:\s*[-–—]\s*(?P<end>[0-9]+))?"
_CITATION_RE = re.compile(
    r"(?P<path>[^\s`*<>\[\]()\"']+)" + _LOCATION_SUFFIX + r"(?![\w–—-])"
)
_DELIMITED_LOCATION_RE = re.compile(r"(?P<path>.+?)" + _LOCATION_SUFFIX + r"$")
_DELIMITED_RE = re.compile(r"`(?P<code>[^`\n]+)`|\]\(<(?P<link>[^>\n]+)>\)")
MAX_CITATION_LINES = 40


def answer_lines(output: str) -> list[str]:
    """Exclude fenced, quoted, indented and commented Markdown examples."""
    lines = []
    fence = None
    quote_paragraph = False
    output = re.sub(r"<!--.*?(?:-->|$)", "", output, flags=re.S)
    for line in output.splitlines():
        quote = re.match(r"^ {0,3}>(?: ?)(.*)$", line)
        block_start = r"^ {0,3}(?:#{1,6}\s|`{3,}|~{3,}|[-+*]\s|1[.)]\s|<|(?:[-_*]\s*){3,}$)"
        if quote and fence is None:
            content = re.sub(r"^(?:> ?)+", "", quote[1])
            quote_paragraph = bool(content.strip()) and not re.match(block_start, content)
            continue
        if quote_paragraph:
            if line.strip() and not re.match(block_start, line):
                continue
            quote_paragraph = False
        marker = re.match(r"^ {0,3}(`{3,}|~{3,})(.*)$", line)
        if fence is not None:
            if (
                marker and marker[1][0] == fence[0]
                and len(marker[1]) >= fence[1] and not marker[2].strip()
            ):
                fence = None
            continue
        if marker:
            fence = (marker[1][0], len(marker[1]))
            continue
        if line.startswith(("    ", "\t")) or re.match(r"^ {0,3}>", line):
            continue
        lines.append(line.strip())
    return lines


def _citation_locations(output: str, root: Path):
    text = "\n".join(answer_lines(output))
    locations = []
    # Delimiters keep a path containing spaces intact. Mask their contents so
    # the bare-path scan cannot reinterpret only the last word as a filename.
    remaining = list(text)
    for token in _DELIMITED_RE.finditer(text):
        location = _DELIMITED_LOCATION_RE.fullmatch(token["code"] or token["link"])
        if location:
            locations.append(location)
            remaining[token.start():token.end()] = " " * (token.end() - token.start())
    bare = "".join(remaining)
    locations.extend(_CITATION_RE.finditer(bare))
    for location in locations:
        path = location["path"]
        if re.match(r"^[A-Za-z][A-Za-z0-9+.-]*://", path):
            continue
        if "/" not in path and "." not in path:
            if not re.search(r"[A-Za-z_]", path):
                continue
            if location.re is _CITATION_RE and not (root / path).is_file():
                continue
        end = int(location["end"] or location["start"])
        yield path, int(location["start"]), end


def _source_path(root: Path, raw: str) -> Path:
    path = Path(unquote(raw))
    if ".." in path.parts:
        raise ValueError("parent traversal is outside the citation contract")
    resolved = (root / path).resolve()
    resolved.relative_to(root)
    if not resolved.is_file():
        raise ValueError("source file does not exist")
    return resolved


def check_citations(output: str, expected: object, fixture: Path | None) -> dict:
    """Match unique source anchors to bounded citations in the actual fixture.

    `valid` counts existing file/line ranges, and `matched` counts required
    anchors covered by those ranges. Neither measures claim entailment.
    """
    result = {"expected": 0, "matched": 0, "total": 0, "valid": 0, "issues": []}
    issues = result["issues"]
    if not isinstance(expected, list) or not expected:
        issues.append("citations must be a non-empty list of path/anchor expectations")
        return result
    result["expected"] = len(expected)
    if fixture is None or not fixture.is_dir():
        issues.append("citation checks require a disposable source fixture")
        return result
    root = fixture.resolve()
    sources = {}

    def read(raw):
        path = _source_path(root, raw)
        if path not in sources:
            sources[path] = path.read_text(encoding="utf-8").splitlines()
        return path, sources[path]

    anchors = []
    seen = set()
    for entry in expected:
        if (
            not isinstance(entry, dict) or set(entry) != {"path", "anchor"}
            or not isinstance(entry["path"], str) or not entry["path"]
            or Path(entry["path"]).is_absolute()
            or not isinstance(entry["anchor"], str) or not entry["anchor"].strip()
        ):
            issues.append(f"invalid citation expectation: {entry!r}")
            continue
        try:
            path, lines = read(entry["path"])
            matches = [i for i, line in enumerate(lines, 1) if entry["anchor"] in line]
            if len(matches) != 1:
                raise ValueError("anchor must identify exactly one source line")
            identity = (path, matches[0])
            if identity in seen:
                raise ValueError("duplicate source anchor")
            seen.add(identity)
            anchors.append(identity)
        except (OSError, ValueError, RuntimeError) as error:
            issues.append(f"invalid citation expectation {entry['path']!r}: {error}")

    citations = set()
    valid = []
    for raw, start, end in _citation_locations(output, root):
        # Canonicalize paths before deduplication so absolute/relative aliases
        # cannot inflate location counts.
        try:
            path, lines = read(raw)
            identity = (str(path), start, end)
        except (OSError, ValueError, RuntimeError) as error:
            identity = (raw, start, end)
            if identity not in citations:
                issues.append(f"invalid citation {raw}:{start}-{end}: {error}")
                citations.add(identity)
            continue
        if identity in citations:
            continue
        citations.add(identity)
        if not 1 <= start <= end <= len(lines) or end - start + 1 > MAX_CITATION_LINES:
            issues.append(f"invalid or overlong citation range: {raw}:{start}-{end}")
        else:
            valid.append((path, start, end))
    result["total"] = len(citations)
    result["valid"] = len(valid)
    for path, line in anchors:
        if any(path == source and start <= line <= end for source, start, end in valid):
            result["matched"] += 1
        else:
            issues.append(f"missing source anchor citation: {path.relative_to(root)}:{line}")
    return result
