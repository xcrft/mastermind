import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from evals.benchmark_process import run_bounded


@unittest.skipUnless(
    os.name == "posix" and hasattr(os, "waitid") and hasattr(os, "WNOWAIT"),
    "requires waitid without reaping",
)
class BoundedProcessTests(unittest.TestCase):
    def test_group_cleanup_keeps_the_leaders_identity_until_reaping(self):
        killpg = os.killpg
        identities = []

        def kill_owned_group(pid, sig):
            try:
                status = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            except ChildProcessError as error:
                raise PermissionError("the leader PID was released before group cleanup") from error
            self.assertIsNotNone(status)
            self.assertEqual(status.si_pid, pid)
            identities.append(pid)
            killpg(pid, sig)

        with tempfile.TemporaryDirectory() as directory, mock.patch(
            "evals.benchmark_process.os.killpg", side_effect=kill_owned_group
        ):
            result = run_bounded(
                [sys.executable, "-c", "print('completed', flush=True)"],
                cwd=Path(directory), env={}, timeout=5,
            )

        self.assertEqual(result.stdout, b"completed\n")
        self.assertEqual(result.returncode, 0)
        self.assertIsNone(result.stop_reason)
        self.assertTrue(identities)

    def test_cleanup_denial_for_a_live_group_is_not_ignored(self):
        identities = []

        def deny_group_cleanup(pid, sig):
            identities.append(pid)
            raise PermissionError("live group cannot be cleaned up")

        with tempfile.TemporaryDirectory() as directory, mock.patch(
            "evals.benchmark_process.sys.platform", "darwin"
        ), mock.patch(
            "evals.benchmark_process._group_has_live_members", return_value=True
        ), mock.patch(
            "evals.benchmark_process.os.killpg", side_effect=deny_group_cleanup
        ):
            with self.assertRaisesRegex(PermissionError, "live group"):
                run_bounded(
                    [sys.executable, "-c", "print('completed', flush=True)"],
                    cwd=Path(directory), env={}, timeout=5,
                )

        self.assertTrue(identities)
        with self.assertRaises(ChildProcessError):
            os.waitid(os.P_PID, identities[0], os.WEXITED | os.WNOHANG | os.WNOWAIT)


if __name__ == "__main__":
    unittest.main()
