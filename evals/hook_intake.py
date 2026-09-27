"""Evaluate synthetic hook-intake cases through the production refiner seam.

Example (the processor must implement the refiner JSON protocol):
  python3 -m evals.hook_intake --binary /absolute/mmcg \
    --processor /absolute/local-processor --output /private/new-report

No provider or credentials are selected automatically. The processor receives
the production request, never expected labels or explanations. Reports retain
each attempt, including failed and invalid output. Label agreement is measured
against implementation-author synthetic labels, not independent semantic truth.
"""

from __future__ import annotations

import argparse
from collections import Counter
from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import platform
import stat
import sys
import time

if __package__:
    from .benchmark_process import run_bounded
else:
    from benchmark_process import run_bounded


ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "evals/hook-intake.jsonl"
CRATE = ROOT / "mcp/servers/mmcg"
INTENTS = frozenset({"activate_mastermind", "continue_active", "ordinary", "unclear"})
ACTIONS = frozenset({"passthrough", "refined", "ask"})
CASE_FIELDS = frozenset({"id", "language", "raw_prompt", "has_active_task", "expected_intent", "why"})
RESPONSE_FIELDS = frozenset({"schema", "intake_id", "prompt_digest", "action", "workflow_intent", "intent_evidence", "refined_prompt", "questions"})
REQUEST_LIMIT = 128 * 1024
STDOUT_LIMIT = 64 * 1024
STDERR_LIMIT = 16 * 1024
FILE_LIMIT = 4 * 1024 * 1024
EXECUTABLE_LIMIT = 512 * 1024 * 1024


def digest(body: bytes) -> str:
    return hashlib.sha256(body).hexdigest()


def encoded(value: object) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False).encode("utf-8")


def strict_json(body: bytes):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("duplicate JSON key")
            result[key] = value
        return result

    def nonfinite(_):
        raise ValueError("nonfinite JSON number")

    try:
        return json.loads(body.decode("utf-8"), object_pairs_hook=pairs, parse_constant=nonfinite)
    except (UnicodeError, RecursionError) as error:
        raise ValueError("invalid bounded JSON") from error


def read_regular(path: Path, limit: int) -> bytes:
    """Read one explicit bounded file with stable descriptor and final-path checks."""
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_size > limit:
            raise ValueError("input is not a bounded regular file")
        parts = []
        remaining = limit + 1
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                break
            parts.append(chunk)
            remaining -= len(chunk)
        after = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    current = path.stat(follow_symlinks=False)
    keys = ("st_dev", "st_ino", "st_mode", "st_size", "st_mtime_ns", "st_ctime_ns")
    if any(getattr(before, key) != getattr(after, key) or getattr(before, key) != getattr(current, key) for key in keys):
        raise ValueError("input changed during read")
    body = b"".join(parts)
    if len(body) > limit or len(body) != before.st_size:
        raise ValueError("input byte count is incomplete")
    return body


def identity(path: Path, limit: int = FILE_LIMIT) -> dict:
    body = read_regular(path, limit)
    return {"path": str(path), "sha256": digest(body), "bytes": len(body)}


def write_new(path: Path, body: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(body)
        stream.flush()
        os.fsync(stream.fileno())


def write_json(path: Path, value: object) -> None:
    write_new(path, encoded(value) + b"\n")


def load_cases(path: Path) -> tuple[bytes, list[dict]]:
    body = read_regular(path, FILE_LIMIT)
    cases = []
    ids = set()
    for line in body.splitlines():
        if not line.strip():
            raise ValueError("blank corpus line")
        case = strict_json(line)
        if not isinstance(case, dict) or set(case) != CASE_FIELDS:
            raise ValueError("case fields do not match the hook-intake corpus schema")
        if any(not isinstance(case[key], str) or not case[key].strip() for key in CASE_FIELDS - {"has_active_task"}):
            raise ValueError("case strings must be nonempty")
        if len(case["id"].encode()) > 256 or case["id"] in ids:
            raise ValueError("duplicate or oversized case ID")
        if len(case["raw_prompt"].encode()) > 16 * 1024 or len(case["why"].encode()) > 2048:
            raise ValueError("case text exceeds its bound")
        if type(case["has_active_task"]) is not bool or case["expected_intent"] not in INTENTS:
            raise ValueError("case has an invalid binding or intent label")
        if case["expected_intent"] == "continue_active" and not case["has_active_task"]:
            raise ValueError("continuation label requires an active task fixture")
        ids.add(case["id"])
        cases.append(case)
    if not 1 <= len(cases) <= 256:
        raise ValueError("corpus must contain 1 to 256 cases")
    return body, cases


def clean_environment(home: Path, temporary: Path) -> dict[str, str]:
    """Do not copy credentials, native client configuration or caller HOME."""
    return {
        "PATH": os.defpath, "HOME": str(home), "USERPROFILE": str(home),
        "CODEX_HOME": str(home / ".codex"), "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_CACHE_HOME": str(home / ".cache"), "TMPDIR": str(temporary),
        "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "MASTERMIND_MINER": "1",
        "PYTHONDONTWRITEBYTECODE": "1", "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
    }


def source_identity(extra: list[Path] | None = None) -> dict:
    paths = {Path(__file__).resolve(), Path(__file__).with_name("benchmark_process.py").resolve()}
    if extra is None:
        paths.update(CRATE / name for name in ("Cargo.toml", "Cargo.lock", "build.rs"))
        def fail_walk(error):
            raise error

        for directory, directories, files in os.walk(CRATE / "src", followlinks=False, onerror=fail_walk):
            if any((Path(directory) / name).is_symlink() for name in directories):
                raise ValueError("source inventory contains a symlink directory")
            paths.update(Path(directory) / name for name in files)
    else:
        paths.update(extra)
    if len(paths) > 4096:
        raise ValueError("source inventory exceeds its bound")
    records = {str(path): identity(path) for path in sorted(paths)}
    if sum(item["bytes"] for item in records.values()) > 64 * 1024 * 1024:
        raise ValueError("source inventory byte budget exceeded")
    return {"sha256": digest(encoded(records)), "files": records}


def executable(path: Path) -> Path:
    if not path.is_absolute():
        raise ValueError("executable path must be absolute")
    resolved = path.resolve(strict=True)
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise ValueError("executable is unavailable")
    return resolved


def validate_processor_args(arguments: list[str]) -> None:
    if len(arguments) > 32 or any(not isinstance(arg, str) or len(arg.encode()) > 8192 or "\0" in arg for arg in arguments) or sum(len(arg.encode()) for arg in arguments) > 32 * 1024:
        raise ValueError("processor arguments exceed their bound")
    # The production Config sees the recorder's arguments. Guard these explicit
    # processor flags before writing the private manifest as well. This list is
    # a misuse guard, not a claim to recognize arbitrary embedded credentials.
    credential_flags = {"--api-key", "--apikey", "--access-token", "--auth-token", "--token", "--password",
                        "--secret", "--client-secret", "--authorization", "--bearer-token", "--credential", "--credentials"}
    if any(arg.split("=", 1)[0].lower().replace("_", "-") in credential_flags for arg in arguments):
        raise ValueError("processor credential arguments are not accepted")


def input_record(case: dict, attempt: int, workspace: Path) -> dict:
    binding = encoded({"raw_prompt": case["raw_prompt"], "has_active_task": case["has_active_task"], "attempt": attempt})
    return {
        "id": digest(b"hook-intake-eval:" + binding),
        "event_id": digest(b"event:" + binding),
        "episode_id": digest(b"episode:" + binding),
        "session_id": digest(b"session:" + binding),
        "client": "codex", "project_root": str(workspace),
        "prompt_digest": digest(case["raw_prompt"].encode()), "capture_generation": 1,
        "original": case["raw_prompt"],
        "active_task": ".mastermind/tasks/001-synthetic/spec.md" if case["has_active_task"] else None,
    }


def validate_request(request: object, expected: dict) -> None:
    if not isinstance(request, dict) or set(request) != {"schema", "instructions", "input", "response_example"}:
        raise ValueError("production request shape mismatch")
    if type(request["schema"]) is not int or request["schema"] != 1 or request["input"] != expected:
        raise ValueError("production request input binding mismatch")
    if not isinstance(request["instructions"], str) or not request["instructions"].strip() or len(request["instructions"].encode()) > 32 * 1024:
        raise ValueError("production instruction text is unavailable or oversized")
    if not isinstance(request["response_example"], dict) or set(request["response_example"]) != RESPONSE_FIELDS:
        raise ValueError("production response example shape mismatch")


def response_issues(response: object, source: dict) -> list[str]:
    """Independent wire checks, not a replacement for Rust prose admission."""
    if not isinstance(response, dict) or set(response) != RESPONSE_FIELDS:
        return ["response_fields"]
    issues = []
    if type(response["schema"]) is not int or response["schema"] != 1:
        issues.append("schema")
    if response["intake_id"] != source["id"] or response["prompt_digest"] != source["prompt_digest"]:
        issues.append("binding")
    action, intent = response["action"], response["workflow_intent"]
    if not isinstance(action, str) or action not in ACTIONS or not isinstance(intent, str) or intent not in INTENTS:
        return issues + ["enum"]
    text, questions, evidence = response["refined_prompt"], response["questions"], response["intent_evidence"]
    if not isinstance(questions, list) or any(not isinstance(item, str) for item in questions):
        return issues + ["questions_shape"]
    if action == "passthrough" and (text != source["original"] or questions):
        issues.append("passthrough")
    if action == "refined" and (not isinstance(text, str) or not text.strip() or len(text.encode()) > 16 * 1024 or questions):
        issues.append("refined")
    if action == "ask" and (text is not None or not 1 <= len(questions) <= 3 or len(set(questions)) != len(questions)
                            or any(not item.strip() or item.strip() != item or len(item.encode()) > 1024 or any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in item) for item in questions)):
        issues.append("ask")
    if intent == "unclear" and action != "ask":
        issues.append("unclear_without_ask")
    if intent == "continue_active" and source["active_task"] is None:
        issues.append("continuation_without_binding")
    if intent in {"activate_mastermind", "continue_active"} and evidence is None:
        issues.append("missing_intent_evidence")
    if evidence is not None and (not isinstance(evidence, str) or not evidence.strip() or evidence.strip() != evidence
                                 or len(evidence.encode()) > 2048 or evidence not in source["original"]):
        issues.append("intent_evidence_literal")
    return issues


def bridge(manifest_path: Path) -> int:
    """Trusted stream recorder, nested inside the production-owned process group."""
    manifest = strict_json(read_regular(manifest_path, REQUEST_LIMIT))
    directory = manifest_path.parent
    streams = {}
    try:
        for name in ("processor.stdout", "processor.stderr"):
            fd = os.open(directory / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            streams[name] = os.fdopen(fd, "wb", buffering=0)
        body = sys.stdin.buffer.read(REQUEST_LIMIT + 1)
        write_new(directory / "request.json", body[:REQUEST_LIMIT])
        if len(body) > REQUEST_LIMIT:
            raise ValueError("production request exceeds byte limit")
        request = strict_json(body)
        expected = strict_json(read_regular(directory / "input.json", REQUEST_LIMIT))
        if digest(encoded(expected)) != manifest["input_sha256"]:
            raise ValueError("input changed before bridge invocation")
        validate_request(request, expected)

        def record(name):
            def receive(chunk):
                streams[name].write(chunk)
                return None
            return receive

        result = run_bounded(
            manifest["command"], cwd=Path(manifest["cwd"]),
            env=clean_environment(Path(manifest["home"]), Path(manifest["temporary"])),
            stdin=body, timeout=manifest["processor_timeout_seconds"],
            stdout_limit=STDOUT_LIMIT, stderr_limit=STDERR_LIMIT, start_new_session=False,
            on_stdout=record("processor.stdout"), on_stderr=record("processor.stderr"),
        )
        status = asdict(result)
        status.pop("stdout")
        status.pop("stderr")
        status.update(status="finished", request_sha256=digest(body))
        write_json(directory / "process.json", status)
        sys.stdout.buffer.write(result.stdout)
        sys.stdout.buffer.flush()
        sys.stderr.buffer.write(result.stderr)
        sys.stderr.buffer.flush()
        return 0 if result.returncode == 0 and result.stop_reason is None else 1
    except (OSError, ValueError, KeyError, TypeError, RecursionError):
        if not (directory / "process.json").exists():
            write_json(directory / "process.json", {"status": "bridge_error"})
        return 1
    finally:
        for stream in streams.values():
            stream.close()


def retained(path: Path, limit: int) -> dict | None:
    if not path.exists():
        return None
    item = identity(path, limit)
    item["path"] = path.name
    return item


def summarize(attempts: list[dict]) -> dict:
    admitted = [row for row in attempts if row.get("protocol_status") == "admitted"]
    counts = Counter(row.get("status", "not_run") for row in attempts)
    matched = sum(row.get("label_match") is True for row in admitted)
    required = [row for row in admitted if row["response"]["workflow_intent"] in {"activate_mastermind", "continue_active"}]
    confusion = {expected: {actual: 0 for actual in sorted(INTENTS)} for expected in sorted(INTENTS)}
    for row in admitted:
        confusion[row["expected_intent"]][row["response"]["workflow_intent"]] += 1
    return {
        "planned_attempts": len(attempts), "attempt_statuses": dict(counts),
        "protocol": {"admitted": len(admitted), "not_admitted": len(attempts) - len(admitted)},
        "wire_contract": {"valid": sum(row.get("wire_contract_valid") is True for row in attempts),
                          "invalid": sum(row.get("wire_contract_valid") is False for row in attempts),
                          "not_observed": sum("wire_contract_valid" not in row for row in attempts)},
        "label_agreement": {"compared": len(admitted), "matched": matched, "mismatched": len(admitted) - matched,
                            "not_comparable": len(attempts) - len(admitted), "confusion": confusion},
        "action_distribution": dict(Counter(row["response"]["action"] for row in admitted)),
        "intent_evidence": {"required_admitted_attempts": len(required), "native_validated_attempts": len(required)},
        "independent_label_review": "missing", "real_world_semantic_quality": "not_established",
    }


def evaluate_attempt(directory: Path, source: dict, case: dict, binary: Path,
                     command: list[str], runtime: Path, timeout: int, processor_identity: dict) -> dict:
    home, temporary, workspace = directory / "home", directory / "tmp", directory / "workspace"
    for path in (home, temporary, workspace):
        path.mkdir(mode=0o700)
    write_json(directory / "input.json", source)
    # Reserve a quarter second for writing diagnostics before Rust's deadline.
    # Report the actual processor budget rather than claiming the larger value.
    write_json(directory / "bridge.json", {
        "command": command, "cwd": str(workspace), "home": str(home), "temporary": str(temporary),
        "input_sha256": digest(encoded(source)), "processor_timeout_seconds": timeout - 0.25,
    })
    native_command = [str(binary), "miner", "hooks", "evaluate-refiner", str(directory / "input.json"),
                      "--processor", sys.executable, f"--processor-arg={runtime / 'hook_intake.py'}",
                      "--processor-arg=--bridge", f"--processor-arg={directory / 'bridge.json'}", "--timeout", str(timeout)]
    row = {"case_id": case["id"], "language": case["language"], "expected_intent": case["expected_intent"],
           "input_sha256": digest(encoded(source)), "prompt_sha256": source["prompt_digest"],
           "status": "pending", "protocol_status": "not_admitted", "processor": processor_identity,
           "native_argv": native_command, "admission": False}
    write_json(directory / "started.json", row)
    result = run_bounded(native_command, cwd=workspace, env=clean_environment(home, temporary),
                         timeout=timeout + 15, stdout_limit=REQUEST_LIMIT, stderr_limit=STDERR_LIMIT)
    write_new(directory / "native.stdout", result.stdout)
    write_new(directory / "native.stderr", result.stderr)
    row["native_process"] = {key: value for key, value in asdict(result).items() if key not in {"stdout", "stderr"}}
    row["status"] = "native_process_failed"
    process_result = None
    if (directory / "process.json").exists():
        try:
            process_result = strict_json(read_regular(directory / "process.json", FILE_LIMIT))
            if not isinstance(process_result, dict):
                raise ValueError("invalid processor record")
            row["processor_process"] = process_result
        except (OSError, ValueError, TypeError, RecursionError):
            row["processor_process"] = {"status": "invalid_record"}
    raw_response = None
    if (directory / "processor.stdout").exists():
        try:
            raw_response = strict_json(read_regular(directory / "processor.stdout", STDOUT_LIMIT))
            row["wire_invariant_issues"] = response_issues(raw_response, source)
            row["wire_contract_valid"] = not row["wire_invariant_issues"]
        except (OSError, ValueError, KeyError, TypeError, RecursionError):
            row["wire_contract_valid"] = False
            row["wire_invariant_issues"] = ["invalid_response_json"]
    try:
        native = strict_json(result.stdout)
        if not isinstance(native, dict) or type(native.get("schema")) is not int or native.get("schema") != 1 or native.get("admission") is not False:
            raise ValueError("evaluation envelope mismatch")
        elapsed = native.get("elapsed_ms")
        if type(elapsed) is not int or not 0 <= elapsed < 2 ** 63:
            raise ValueError("invalid evaluation duration")
        row["evaluation"] = native
        if result.returncode != 0 or result.stop_reason is not None:
            return row
        if native.get("status") == "failed" and set(native) == {"schema", "status", "admission", "reason", "elapsed_ms"}:
            if not isinstance(native["reason"], str) or not native["reason"] or len(native["reason"]) > 256:
                raise ValueError("invalid failure reason")
            row["status"] = "production_rejected"
            return row
        if native.get("status") != "evaluated" or set(native) != {"schema", "status", "admission", "response", "elapsed_ms"}:
            raise ValueError("evaluation envelope mismatch")
        request = strict_json(read_regular(directory / "request.json", REQUEST_LIMIT))
        validate_request(request, source)
        if not isinstance(process_result, dict) or process_result.get("status") != "finished" or process_result.get("returncode") != 0 or process_result.get("stop_reason") is not None:
            raise ValueError("processor did not finish successfully")
        response = raw_response
        issues = response_issues(response, source)
        if issues or native["response"] != response:
            row["invariant_issues"] = issues or ["native_response_differs_from_raw_output"]
            raise ValueError("response invariant mismatch")
        row.update(status="matched" if response["workflow_intent"] == case["expected_intent"] else "label_mismatch",
                   protocol_status="admitted", response=response,
                   label_match=response["workflow_intent"] == case["expected_intent"],
                   instructions_sha256=digest(request["instructions"].encode()))
    except (OSError, ValueError, KeyError, TypeError, RecursionError):
        row["status"] = "evidence_invalid"
    return row


def run_evaluation(*, binary: Path, processor: Path, processor_args: list[str], output: Path,
                   cases_path: Path = CORPUS, case_ids: list[str] | None = None, repetitions: int = 1,
                   timeout: int = 8, processor_sources: list[Path] | None = None,
                   source_files: list[Path] | None = None) -> dict:
    if os.name != "posix":
        raise ValueError("hook intake evaluation requires POSIX process supervision")
    if type(timeout) is not int or not 1 <= timeout <= 20 or not 1 <= repetitions <= 5:
        raise ValueError("timeout must be 1..20 and repetitions 1..5")
    validate_processor_args(processor_args)
    binary, processor = executable(binary), executable(processor)
    cases_path = cases_path.absolute()
    corpus_bytes, cases = load_cases(cases_path)
    if case_ids:
        if len(set(case_ids)) != len(case_ids) or set(case_ids) - {case["id"] for case in cases}:
            raise ValueError("case selection contains duplicate or unknown IDs")
        cases = [case for case in cases if case["id"] in set(case_ids)]
    sources = processor_sources or []
    if len(sources) > 16:
        raise ValueError("at most 16 explicit processor source files are supported")
    output = output.absolute()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    output = output.resolve()
    runtime = output / "runtime"
    runtime.mkdir(mode=0o700)
    for name in ("hook_intake.py", "benchmark_process.py"):
        write_new(runtime / name, read_regular(Path(__file__).with_name(name), FILE_LIMIT))
    write_new(output / "corpus.jsonl", corpus_bytes)
    command = [str(processor), *processor_args]
    planned = [(case, repeat) for repeat in range(1, repetitions + 1) for case in cases]
    report = {
        "schema_version": 1, "kind": "mastermind-hook-intake-eval", "status": "failed",
        "runtime": {"python": platform.python_version(), "platform": platform.platform()},
        "corpus": {"sha256": digest(corpus_bytes), "selected_case_ids": [case["id"] for case in cases],
                   "provenance": "synthetic implementation-author labels", "independent_review": "missing"},
        "budgets": {"native_timeout_seconds": timeout, "processor_timeout_seconds": timeout - 0.25,
                    "stdout_bytes": STDOUT_LIMIT, "stderr_bytes": STDERR_LIMIT, "repetitions": repetitions},
        "attempts": [{"case_id": case["id"], "expected_intent": case["expected_intent"], "repetition": repeat, "status": "not_run"} for case, repeat in planned],
        "limitations": [
            "Label agreement uses author-written synthetic labels, not independent semantic accuracy.",
            "Fixture processors test transport and accounting only, not model quality.",
            "Evaluation inputs simulate task bindings and never admit a task or publish a journal.",
            "Rust validates the response protocol and citation provenance, not semantic truth or preserved intent.",
            "One external processor invocation is not a measured provider API-call count, token count or cost.",
            "Before/after hashes do not detect a temporary mutation that was restored.",
            "Source and binary digests are recorded separately, build correspondence is not established.",
            "Executable and explicitly listed processor sources are pinned, other dependencies remain unverified.",
            "Local records are owner-writable, not independent attestation or an OS sandbox.",
            "Output-limit or interrupted attempts retain bounded stream prefixes, not claimed complete output.",
        ],
    }
    started = time.monotonic()
    try:
        report["source"] = source_identity(source_files)
        processor_binding = {"executable": identity(processor, EXECUTABLE_LIMIT), "argv_sha256": digest(encoded(command)),
                             "declared_sources": [identity(path.absolute()) for path in sources]}
        report["processor"] = processor_binding
        report["binary"] = identity(binary, EXECUTABLE_LIMIT)
        report["python"] = identity(Path(sys.executable).resolve(), EXECUTABLE_LIMIT)
        write_json(output / "manifest.json", report)
        for index, (case, repeat) in enumerate(planned):
            directory = output / f"attempt-{index + 1:04d}"
            directory.mkdir(mode=0o700)
            row = report["attempts"][index]
            row["directory"] = directory.name
            try:
                current_processor = {"executable": identity(processor, EXECUTABLE_LIMIT), "argv_sha256": digest(encoded(command)),
                                     "declared_sources": [identity(path.absolute()) for path in sources]}
                if current_processor != processor_binding or identity(binary, EXECUTABLE_LIMIT) != report["binary"]:
                    raise ValueError("runtime_input_changed")
                source = input_record(case, repeat, directory / "workspace")
                result = evaluate_attempt(directory, source, case, binary, command, runtime, timeout, processor_binding)
                row.update(result)
                current_processor = {"executable": identity(processor, EXECUTABLE_LIMIT), "argv_sha256": digest(encoded(command)),
                                     "declared_sources": [identity(path.absolute()) for path in sources]}
                if current_processor != processor_binding or identity(binary, EXECUTABLE_LIMIT) != report["binary"]:
                    row.update(status="input_changed", protocol_status="not_admitted", label_match=None)
            except KeyboardInterrupt:
                row.update(status="interrupted", protocol_status="not_admitted", label_match=None)
                raise
            except (OSError, ValueError, KeyError, TypeError, RecursionError):
                row.update(status="input_or_setup_error", protocol_status="not_admitted", label_match=None)
            finally:
                row["retained"] = {}
                for name in ("input.json", "request.json", "process.json", "processor.stdout", "processor.stderr", "native.stdout", "native.stderr"):
                    try:
                        row["retained"][name] = retained(directory / name, REQUEST_LIMIT)
                    except (OSError, ValueError):
                        row["retained"][name] = {"status": "unsafe_or_changed"}
                        row.update(status="evidence_invalid", protocol_status="not_admitted", label_match=None)
                write_json(directory / "result.json", row)
        report["source_unchanged"] = source_identity(source_files) == report["source"]
        report["corpus_unchanged"] = read_regular(cases_path, FILE_LIMIT) == corpus_bytes
        report["status"] = "passed" if report["source_unchanged"] and report["corpus_unchanged"] and all(row["status"] == "matched" for row in report["attempts"]) else "failed"
    except KeyboardInterrupt:
        report["error"] = "interrupted"
    except (OSError, ValueError, KeyError, TypeError, RecursionError):
        report["error"] = "evaluation_setup_or_evidence_failure"
    report["elapsed_seconds"] = time.monotonic() - started
    report["metrics"] = summarize(report["attempts"])
    write_json(output / "report.json", report)
    return report


def main(argv=None) -> int:
    arguments = list(sys.argv[1:] if argv is None else argv)
    if arguments[:1] == ["--bridge"]:
        if len(arguments) != 2:
            return 2
        return bridge(Path(arguments[1]))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True, help="explicit mmcg binary with evaluate-refiner")
    parser.add_argument("--processor", type=Path, required=True, help="explicit external JSON processor")
    parser.add_argument("--processor-arg", action="append", default=[])
    parser.add_argument("--processor-source", type=Path, action="append", default=[], help="additional code/config bytes to pin")
    parser.add_argument("--output", type=Path, required=True, help="new private output directory")
    parser.add_argument("--cases", type=Path, default=CORPUS)
    parser.add_argument("--case", action="append", dest="case_ids")
    parser.add_argument("--repetitions", type=int, default=1)
    parser.add_argument("--timeout", type=int, default=8)
    args = parser.parse_args(arguments)
    try:
        report = run_evaluation(binary=args.binary, processor=args.processor, processor_args=args.processor_arg,
                                output=args.output, cases_path=args.cases, case_ids=args.case_ids,
                                repetitions=args.repetitions, timeout=args.timeout, processor_sources=args.processor_source)
    except (OSError, ValueError, TypeError):
        print("Hook intake evaluation setup failed. No result is claimed.", file=sys.stderr)
        return 2
    print(f"Hook intake evaluation: {report['status']}. Report: {args.output / 'report.json'}")
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
