"""Tiny module used by the CI smoke fixture.

This file is indexed by `mmcg index .` during CI verify/audit smokes. The
test exercises the Python extractor (functions, decorators, module-level
constants) without depending on any external imports — keeps CI runtime
identical across all 7 build-matrix targets.
"""

MAX_RETRIES = 3
TIMEOUT_SECS = 30.0


def greet(name: str) -> str:
    """Trivial public function — referenced from caller below."""
    return f"hello {name}"


def caller():
    """Calls `greet` once so the index records an edge from caller→greet."""
    return greet("world")
