"""Real subprocess supervision, cleanup and bounded pipe handling."""

from pathlib import Path
from unittest.mock import patch
import os
import subprocess
import sys
import tempfile
import time
import unittest

from evals.shared import process
from evals.shared.process import run_bounded
from tests.evals.support.processes import assert_process_stopped


@unittest.skipUnless(os.name == "posix", "POSIX process supervision")
class ProcessTests(unittest.TestCase):
    def test_disabled_deadline_waits_for_completion_after_controlled_clock_jump(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ready = False
            real_clock = time.monotonic
            code = ("from pathlib import Path; import time; print('READY', flush=True); "
                    "\nwhile not Path('release').exists(): time.sleep(0.001)"
                    "\nprint('COMPLETE', flush=True)")

            def clock():
                return real_clock() + (7200 if ready else 0)

            def observe(chunk):
                nonlocal ready
                if b"READY" in chunk:
                    ready = True
                    (root / "release").touch()

            with patch.object(process.time, "monotonic", side_effect=clock):
                result = run_bounded([sys.executable, "-c", code], cwd=root, env={},
                                     timeout=None, on_stdout=observe)
            self.assertEqual(result.stdout, b"READY\nCOMPLETE\n")
            self.assertEqual(result.returncode, 0)
            self.assertIsNone(result.stop_reason)

    def test_disabled_deadline_still_stops_and_retains_output_at_byte_cap(self):
        with tempfile.TemporaryDirectory() as directory:
            result = run_bounded([sys.executable, "-c", "print('x' * 1000)"],
                cwd=Path(directory), env={}, timeout=None, stdout_limit=8)
        self.assertEqual(result.stdout, b"x" * 8)
        self.assertEqual(result.stop_reason, "output_limit")

    def test_large_input_does_not_deadlock_a_child_that_never_reads_it(self):
        with tempfile.TemporaryDirectory() as directory:
            result = run_bounded([sys.executable, "-c", "import time; time.sleep(20)"],
                cwd=Path(directory), env={}, stdin=b"x" * 1_000_000, timeout=0.3)
        self.assertEqual(result.stop_reason, "timeout")

    def test_timeout_kills_the_child_process_group(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            code = ("import subprocess, sys, time; "
                    "p = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(20)']); "
                    "print(p.pid, flush=True); time.sleep(20)")
            output = bytearray()
            real_clock = time.monotonic

            def clock():
                return real_clock() + (31 if b"\n" in output else 0)

            def observe(chunk):
                output.extend(chunk)

            with patch.object(process.time, "monotonic", side_effect=clock):
                result = run_bounded([sys.executable, "-c", code], cwd=root, env={},
                                     timeout=30, on_stdout=observe)
            self.assertEqual(result.stop_reason, "timeout")
            pid = int(result.stdout.strip())
            assert_process_stopped(self, pid)

    def test_normal_exit_does_not_wait_for_descendant_held_pipes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            code = ("import subprocess, sys; "
                    "p = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(20)']); "
                    "print(p.pid, flush=True)")
            result = run_bounded([sys.executable, "-c", code], cwd=root, env={}, timeout=30)
            pid = int(result.stdout.strip())
            self.assertEqual(result.returncode, 0)
            self.assertIsNone(result.stop_reason)
            assert_process_stopped(self, pid)

    def test_spawn_failure_has_no_fabricated_return_code(self):
        with tempfile.TemporaryDirectory() as directory:
            result = run_bounded([str(Path(directory) / "absent")], cwd=Path(directory), env={})
        self.assertEqual(result.stop_reason, "spawn_error")
        self.assertIsNone(result.returncode)

    def test_group_cleanup_keeps_the_leaders_identity_until_reaping(self):
        killpg = os.killpg
        identities = []

        def kill_owned_group(pid, sig):
            status = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            self.assertIsNotNone(status)
            self.assertEqual(status.si_pid, pid)
            identities.append(pid)
            killpg(pid, sig)

        with tempfile.TemporaryDirectory() as directory, \
                patch.object(process.os, "killpg", side_effect=kill_owned_group):
            result = run_bounded([sys.executable, "-c", "print('completed', flush=True)"],
                cwd=Path(directory), env={}, timeout=30)
        self.assertEqual(result.stdout, b"completed\n")
        self.assertEqual(result.returncode, 0)
        self.assertIsNone(result.stop_reason)
        self.assertTrue(identities)
        with self.assertRaises(ChildProcessError):
            os.waitid(os.P_PID, identities[0], os.WEXITED | os.WNOHANG | os.WNOWAIT)

    def test_exited_leader_is_unreaped_and_quiescent_permission_error_preserves_output(self):
        observed = []

        def refuse_zombie_group(pid, _signal):
            status = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            self.assertIsNotNone(status)
            self.assertEqual(status.si_pid, pid)
            observed.append(pid)
            raise PermissionError("zombie group")

        with tempfile.TemporaryDirectory() as directory, \
                patch.object(process.sys, "platform", "darwin"), \
                patch.object(process.os, "killpg", side_effect=refuse_zombie_group):
            result = run_bounded([sys.executable, "-c", "print('retained output')"],
                cwd=Path(directory), env={}, timeout=30)
        self.assertEqual(len(observed), 1)
        self.assertEqual(result.stdout, b"retained output\n")
        self.assertEqual(result.returncode, 0)
        self.assertIsNone(result.stop_reason)

    def test_unverified_group_cleanup_cannot_be_reported_as_success(self):
        for mode in ("missing", "live", "non_darwin_zombie"):
            with self.subTest(mode=mode):
                group = []

                def refused(pid, _signal):
                    group.append(pid)
                    raise PermissionError("denied")

                def observation(*args, **kwargs):
                    state = "Z" if mode == "non_darwin_zombie" else "R"
                    body = "" if mode == "missing" else f"{group[0]} {state}\n"
                    return subprocess.CompletedProcess(args, 0, stdout=body, stderr="")

                with tempfile.TemporaryDirectory() as directory, \
                        patch.object(process.sys, "platform", "linux" if mode == "non_darwin_zombie" else "darwin"), \
                        patch.object(process.os, "killpg", side_effect=refused), \
                        patch.object(process.subprocess, "run", side_effect=observation):
                    result = run_bounded([sys.executable, "-c", "print('retained output')"],
                        cwd=Path(directory), env={}, timeout=30)
                self.assertEqual(result.stdout, b"retained output\n")
                self.assertEqual(result.stop_reason, "cleanup_error")
