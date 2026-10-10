"""Account for exact-name lookup use in a retained, hash-bound Codex stream."""

from . import artifacts
from .mcp import result_body


COUNTERS = ("lookup_calls", "single_calls", "batch_calls", "requested_names",
            "returned_matches", "truncated_queries", "failed_calls",
            "unverifiable_calls", "pending_calls", "duplicate_completions")


def from_stream(body, sources):
    allowed = {item["path"]: item["lines"] for item in sources}
    pending, changed, completed, requests = {}, set(), {}, []
    counts = dict.fromkeys(COUNTERS, 0)
    stream_complete = not body or body.endswith(b"\n")
    for line in body.splitlines():
        try:
            event = artifacts.parse_json(line)
            if not isinstance(event, dict):
                raise ValueError("invalid event")
        except (ValueError, TypeError, UnicodeError):
            stream_complete = False
            break
        if event.get("type") not in ("item.started", "item.updated", "item.completed"):
            continue
        item = event.get("item")
        if (not isinstance(item, dict) or item.get("type") != "mcp_tool_call"
                or item.get("server") != "research" or item.get("tool") != "mmcg_search"):
            continue
        identifier, args = item.get("id"), item.get("arguments")
        if not isinstance(identifier, str) or not identifier:
            stream_complete = False
            if event["type"] == "item.completed":
                counts["lookup_calls"] += 1
                counts["unverifiable_calls"] += 1
            continue
        if event["type"] != "item.completed":
            if identifier not in completed:
                previous = pending.get(identifier)
                if previous is not None and args is not None and previous != args:
                    changed.add(identifier)
                pending[identifier] = previous if previous is not None else args
            continue
        if identifier in completed:
            counts["duplicate_completions"] += 1
            if completed[identifier] != artifacts.digest(item):
                stream_complete = False
            continue
        completed[identifier] = artifacts.digest(item)
        prior = pending.pop(identifier, args)
        counts["lookup_calls"] += 1
        try:
            if (identifier in changed or prior is not None and prior != args
                    or not isinstance(args, dict) or ("name" in args) == ("names" in args)):
                raise ValueError("changed or ambiguous request")
            batch = "names" in args
            names = args["names"] if batch else [args["name"]]
            if (not isinstance(names, list) or not 1 <= len(names) <= (8 if batch else 1)
                    or any(not isinstance(name, str) or not name.strip()
                           or len(name.encode()) > 1024 for name in names)
                    or len(set(names)) != len(names)):
                raise ValueError("invalid names")
            top = args.get("top", 10 if batch else 100)
            if type(top) is not int or not 1 <= top <= (25 if batch else 200):
                raise ValueError("invalid limit")
            record = {"id": identifier, "names": names, "batch": batch,
                      "requested_top": args.get("top"), "effective_top": top,
                      "selectors": {key: args[key] for key in ("kind", "language", "collapse_partials")
                                    if key in args}}
            requests.append(record)
            counts["batch_calls" if batch else "single_calls"] += 1
            counts["requested_names"] += len(names)
            result = item.get("result")
            if (item.get("status") == "failed" or item.get("error") is not None
                    or isinstance(result, dict) and result.get("isError") is True):
                record["status"] = "failed"
                counts["failed_calls"] += 1
                continue
            if item.get("status") != "completed":
                raise ValueError("incomplete result")
            value = result_body(result)
            queries = value.get("queries") if batch else [value]
            if (not isinstance(queries, list) or len(queries) != len(names)
                    or batch and (type(value.get("query_count")) is not int
                                  or value["query_count"] != len(names))):
                raise ValueError("invalid batch")
            returned, truncated = 0, 0
            for name, query in zip(names, queries):
                if (not isinstance(query, dict) or query.get("query") != name
                        or type(query.get("count")) is not int
                        or not isinstance(query.get("results"), list)
                        or query["count"] != len(query["results"])
                        or not 0 <= query["count"] <= top
                        or type(query.get("truncated")) is not bool or "total" not in query
                        or type(query.get("collapse_partials")) is not bool
                        or query.get("collapse_partials") != args.get("collapse_partials", True)
                        or any(query.get(key) != args.get(key) for key in ("kind", "language"))
                        or not isinstance(query.get("precision_notes"), list)
                        or any(not isinstance(note, str) for note in query["precision_notes"])
                        or query.get("total") is not None and (
                            type(query["total"]) is not int or query["total"] < query["count"])):
                    raise ValueError("invalid query metadata")
                for hit in query["results"]:
                    if (not isinstance(hit, dict) or hit.get("locations") is not None
                            and not isinstance(hit["locations"], list)):
                        raise ValueError("invalid source locations")
                    locations = [hit, *(hit.get("locations") or [])]
                    if not locations or any(not isinstance(location, dict)
                            or location.get("file") not in allowed
                            or type(location.get("line")) is not int
                            or not 1 <= location["line"] <= allowed[location["file"]]
                            for location in locations):
                        raise ValueError("foreign source location")
                returned += query["count"]
                truncated += query["truncated"]
            if batch and (type(value.get("truncated")) is not bool
                          or value["truncated"] != bool(truncated)):
                raise ValueError("invalid batch truncation")
            counts["returned_matches"] += returned
            counts["truncated_queries"] += truncated
            record["status"] = "completed"
        except (ValueError, TypeError, KeyError, AttributeError):
            counts["unverifiable_calls"] += 1
            if requests and requests[-1]["id"] == identifier:
                requests[-1]["status"] = "unverifiable"
    counts["pending_calls"] = len(pending)
    counts["lookup_calls"] += len(pending)
    counts["unverifiable_calls"] += len(pending)
    return {"schema_version": 1, **counts, "requests": requests,
            "lookup_accounting_complete": stream_complete and counts["unverifiable_calls"] == 0,
            "stream_complete": stream_complete,
            "scope": "native_lookup_calls_in_one_hash_bound_trial",
            "limitations": "Counts lookup use and returned matches, not avoided model rounds or semantic quality."}
