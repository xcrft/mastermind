"""Bounded POSIX process transport for trusted benchmark adapters and tools."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Callable
import os
import selectors
import signal
import subprocess
import time


@dataclass
class ProcessResult:
    stdout: bytes = b""
    stderr: bytes = b""
    returncode: int | None = None
    stop_reason: str | None = None
    elapsed_seconds: float = 0.0


def _exited(pid):
    status = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
    return status is not None and status.si_pid == pid


def _group_quiescent(pgid):
    try:
        result = subprocess.run(["ps", "-axo", "pgid=,stat="], env={"PATH": os.defpath},
            capture_output=True, text=True, timeout=1, check=True)
        rows = [line.split() for line in result.stdout.splitlines()]
        if any(len(row) != 2 for row in rows):
            return False
        states = [state for group, state in rows if group == str(pgid)]
        return bool(states) and all(state.startswith("Z") for state in states)
    except (OSError, subprocess.SubprocessError, UnicodeError):
        return False


def run_bounded(
    command: list[str], *, cwd: Path, env: dict[str, str], stdin: bytes = b"",
    timeout: float | None = 60, stdout_limit: int = 2 * 1024 * 1024,
    stderr_limit: int = 64 * 1024,
    start_new_session: bool = True,
    on_stdout: Callable[[bytes], str | None] | None = None,
    on_stderr: Callable[[bytes], str | None] | None = None,
) -> ProcessResult:
    """Keep the supplied byte caps; timeout=None waits for exit without a deadline.

    This is resource supervision, not an OS sandbox. The executable is trusted.
    Children must not escape the process group or inherit unrelated descriptors.
    Nested tools use start_new_session=False so the outer trial supervisor can
    kill every descendant even if the adapter itself receives SIGKILL.
    Such callers must run under that outer supervisor; local cleanup kills only
    the direct child. on_stdout may request early termination with a reason.
    """
    if os.name != "posix" or not all(hasattr(os, name) for name in ("waitid", "WNOWAIT")):
        return ProcessResult(stop_reason="unsupported_platform")
    started = time.monotonic()
    try:
        process = subprocess.Popen(
            command, cwd=cwd, env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            start_new_session=start_new_session, close_fds=True,
        )
    except OSError:
        return ProcessResult(stop_reason="spawn_error", elapsed_seconds=time.monotonic() - started)
    output = {"stdout": bytearray(), "stderr": bytearray()}
    caps = {"stdout": stdout_limit, "stderr": stderr_limit}
    reason = None
    sent = 0
    exited_at = None
    selector = selectors.DefaultSelector()
    assert process.stdin is not None and process.stdout is not None and process.stderr is not None
    streams = (process.stdin, process.stdout, process.stderr)
    try:
        for stream, kind in ((process.stdout, "stdout"), (process.stderr, "stderr")):
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, kind)
        if stdin:
            os.set_blocking(process.stdin.fileno(), False)
            selector.register(process.stdin, selectors.EVENT_WRITE, "stdin")
        else:
            process.stdin.close()
        # Keep the leader unreaped until group cleanup so its PID cannot be
        # reused by an unrelated process between observing exit and signalling.
        while selector.get_map() or not _exited(process.pid):
            if _exited(process.pid):
                exited_at = exited_at or time.monotonic()
                # A descendant may still hold the exited process's pipes open.
                # Drain briefly, then let the finally block kill the owned
                # process group or direct nested child.
                if time.monotonic() - exited_at > 0.2:
                    break
            remaining = None if timeout is None else timeout - (time.monotonic() - started)
            if remaining is not None and remaining <= 0:
                reason = "timeout"
                break
            for key, _ in selector.select(0.05 if remaining is None else min(remaining, 0.05)):
                stream, kind = key.fileobj, key.data
                if kind == "stdin":
                    try:
                        sent += os.write(stream.fileno(), stdin[sent:sent + 65536])
                    except BrokenPipeError:
                        sent = len(stdin)
                    except BlockingIOError:
                        continue
                    if sent == len(stdin):
                        selector.unregister(stream)
                        stream.close()
                    continue
                try:
                    chunk = os.read(stream.fileno(), 65536)
                except BlockingIOError:
                    continue
                if not chunk:
                    selector.unregister(stream)
                    stream.close()
                    continue
                room = caps[kind] - len(output[kind])
                output[kind].extend(chunk[:room])
                callback = on_stdout if kind == "stdout" else on_stderr
                if callback is not None and room:
                    reason = callback(chunk[:room])
                if len(chunk) > room:
                    reason = "output_limit"
                if reason:
                    break
            if reason:
                break
    finally:
        # Also terminate descendants after a normally exiting adapter. A trial
        # must not leave a tool server running into the next condition.
        try:
            if start_new_session:
                os.killpg(process.pid, signal.SIGKILL)
            elif not _exited(process.pid):
                os.kill(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except PermissionError:
            # Darwin reports EPERM for a group containing only zombies. Any
            # live or unobservable member remains a cleanup failure.
            if not start_new_session or not _exited(process.pid) or not _group_quiescent(process.pid):
                reason = "cleanup_error"
                try:
                    os.kill(process.pid, signal.SIGKILL)
                except OSError:
                    pass
        try:
            process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            reason = "cleanup_error"
        selector.close()
        for stream in streams:
            stream.close()
    return ProcessResult(
        stdout=bytes(output["stdout"]), stderr=bytes(output["stderr"]),
        returncode=process.returncode, stop_reason=reason,
        elapsed_seconds=time.monotonic() - started,
    )
