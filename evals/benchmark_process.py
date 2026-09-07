"""Bounded POSIX process transport for trusted benchmark adapters and tools."""

from __future__ import annotations

import os
import selectors
import signal
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable


@dataclass
class ProcessResult:
    stdout: bytes = b""
    stderr: bytes = b""
    returncode: int | None = None
    stop_reason: str | None = None
    elapsed_seconds: float = 0.0


def run_bounded(
    command: list[str], *, cwd: Path, env: dict[str, str], stdin: bytes = b"",
    timeout: float = 60, stdout_limit: int = 2 * 1024 * 1024,
    stderr_limit: int = 64 * 1024,
    start_new_session: bool = True,
    on_stdout: Callable[[bytes], str | None] | None = None,
    on_stderr: Callable[[bytes], str | None] | None = None,
) -> ProcessResult:
    """Keep at most the supplied byte caps; kill the process group on stop.

    This is resource supervision, not an OS sandbox. The executable is trusted.
    Children must not escape the process group or inherit unrelated descriptors.
    Nested tools use start_new_session=False so the outer trial supervisor can
    kill every descendant even if the adapter itself receives SIGKILL.
    Such callers must run under that outer supervisor; local cleanup kills only
    the direct child. on_stdout may request early termination with a reason.
    """
    if os.name != "posix":
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
        while selector.get_map() or process.poll() is None:
            if not start_new_session and process.poll() is not None:
                exited_at = exited_at or time.monotonic()
                # A nested server may still hold its parent's stderr open.
                # Drain briefly; the outer trial owns descendant cleanup.
                if time.monotonic() - exited_at > 0.2:
                    break
            remaining = timeout - (time.monotonic() - started)
            if remaining <= 0:
                reason = "timeout"
                break
            for key, _ in selector.select(min(remaining, 0.05)):
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
            else:
                process.kill()
        except ProcessLookupError:
            pass
        process.wait()
        selector.close()
        for stream in streams:
            stream.close()
    return ProcessResult(
        stdout=bytes(output["stdout"]), stderr=bytes(output["stderr"]),
        returncode=process.returncode, stop_reason=reason,
        elapsed_seconds=time.monotonic() - started,
    )
