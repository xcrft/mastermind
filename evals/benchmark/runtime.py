"""Pinned executables and private trial environments."""

from __future__ import annotations

from pathlib import Path
import os
import re
import sys

from . import artifacts as artifact_io


CREDENTIAL_NAMES = {"ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "OPENAI_API_KEY"}


def clean_environment(trial: Path, credentials: dict[str, str] | None = None) -> dict[str, str]:
    credentials = credentials or {}
    if set(credentials) - CREDENTIAL_NAMES:
        raise artifact_io.BenchmarkError("invalid_credentials", "only explicit credential names are accepted")
    home = trial / "home"
    temp = trial / "tmp"
    home.mkdir(exist_ok=True)
    temp.mkdir(exist_ok=True)
    return {
        "PATH": os.pathsep.join((str(Path(sys.executable).parent), os.defpath)),
        "HOME": str(home), "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_CACHE_HOME": str(home / ".cache"), "TMPDIR": str(temp),
        "LANG": "C.UTF-8", "LC_ALL": "C", "TERM": "dumb",
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0", "GIT_OPTIONAL_LOCKS": "0",
        "GIT_NO_LAZY_FETCH": "1", "GIT_ALLOW_PROTOCOL": "",
        "GIT_AUTHOR_NAME": "Benchmark", "GIT_COMMITTER_NAME": "Benchmark",
        "GIT_AUTHOR_EMAIL": "benchmark@example.invalid", "GIT_COMMITTER_EMAIL": "benchmark@example.invalid",
        "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
        **credentials,
    }


def runtime_pin(spec: object, role: str, revision: str | None = None) -> dict:
    if not isinstance(spec, dict):
        raise artifact_io.BenchmarkError(f"{role}_missing", f"an explicit {role} runtime pin is required")
    try:
        path = Path(spec["path"]).resolve(strict=True)
        expected = spec["sha256"]
        if not re.fullmatch(r"[0-9a-f]{64}", expected):
            raise ValueError("invalid hash")
        if artifact_io.hash_file(path, artifact_io.BINARY_BYTE_LIMIT)["sha256"] != expected:
            raise artifact_io.BenchmarkError(f"{role}_mismatch", f"{role} executable does not match its SHA-256 pin")
        if not os.access(path, os.X_OK):
            raise ValueError("not executable")
        if revision is not None and spec.get("source_revision") != revision:
            raise artifact_io.BenchmarkError(f"{role}_revision_mismatch", f"{role} declared source revision does not match instructions")
        if not isinstance(spec.get("version"), str) or not spec["version"]:
            raise ValueError("missing version")
        if not isinstance(spec.get("origin"), str) or not spec["origin"]:
            raise ValueError("missing origin")
        return {"path": str(path), "sha256": expected, "version": spec["version"],
                "origin": spec["origin"], "source_revision": spec.get("source_revision"),
                "source_provenance": "declared_not_attestation_verified"}
    except (KeyError, TypeError, ValueError, OSError) as error:
        raise artifact_io.BenchmarkError(f"{role}_invalid", f"invalid {role} runtime pin") from error
