"""JSON parsing that refuses duplicate keys and NaN or Infinity literals."""

from __future__ import annotations

import json


def _unique_object(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _reject_constant(name: str):
    raise ValueError(f"nonfinite JSON number: {name}")


def strict_json(document: str | bytes):
    """Parse like json.loads, so bytes keep its encoding detection."""
    try:
        return json.loads(document, object_pairs_hook=_unique_object, parse_constant=_reject_constant)
    except RecursionError as error:
        raise ValueError("JSON nesting is too deep") from error
