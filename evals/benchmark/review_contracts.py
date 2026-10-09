"""Schema primitives shared by offline review and outcome analysis."""

from __future__ import annotations

import re

from evals.benchmark import artifacts as artifact_io


RUN_STATES = {"completed", "setup_error", "timeout", "output_limit", "protocol_error",
              "identity_mismatch", "invocation_error", "model_error", "budget_exceeded", "input_changed"}

SLOT_STATES = RUN_STATES | {"not_run", "unfinished", "missing_artifacts"}

TOKEN_FIELDS = ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens")

CORE_RESOURCE_FIELDS = ("setup_seconds", "run_seconds", "cost_usd", "turns", *TOKEN_FIELDS)
TIMING_FIELDS = ("first_message_seconds", "final_answer_seconds")
RESOURCE_FIELDS = (*CORE_RESOURCE_FIELDS, *TIMING_FIELDS)


def require(condition, message, code="review_schema"):
    if not condition:
        raise artifact_io.BenchmarkError(code, message)


def fields(value, required, optional=()):
    require(isinstance(value, dict) and set(required) <= set(value)
            and not set(value) - set(required) - set(optional), "missing or unexpected review fields")


def text(value, maximum=16384):
    require(isinstance(value, str) and bool(value.strip()) and len(value.encode()) <= maximum,
            "expected bounded nonempty review text")
    return value


def identifier(value, prefix):
    require(isinstance(value, str) and re.fullmatch(prefix + r"[0-9a-f]{32}", value), "invalid artifact ID")
    return value


def hash_value(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value), "invalid artifact hash")
    return value
