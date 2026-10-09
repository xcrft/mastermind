"""State-based checks for subprocess cleanup, including unreaped descendants."""

import subprocess
import time


def assert_process_stopped(test, pid):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        result = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)],
                                capture_output=True, text=True, timeout=5)
        if result.returncode or result.stdout.lstrip().startswith("Z"):
            return
        time.sleep(0.02)
    test.fail(f"process {pid} survived supervisor cleanup")
