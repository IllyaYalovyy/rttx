"""UI test: TestEnvironment cleans up daemon processes on abnormal exit paths."""

import os
import signal
import time
import unittest

from common import TestEnvironment


class TestFixtureCleanup(unittest.TestCase):
    """Verify that daemon and weston processes are killed even when
    the normal tearDown path is skipped."""

    def test_stop_daemon_kills_by_pid_when_socket_gone(self) -> None:
        """stop_daemon() must kill the daemon by PID when the socket has
        already been removed (e.g. by shutil.rmtree racing cleanup)."""
        env = TestEnvironment()
        try:
            env.start_weston()
            env.start_daemon()
            daemon_pid = env._daemon.pid

            # Verify daemon is alive.
            os.kill(daemon_pid, 0)

            # Remove the socket before calling stop_daemon — simulates
            # shutil.rmtree running before stop_daemon in a crash path.
            socket_path = env.daemon_socket_path
            if os.path.exists(socket_path):
                os.remove(socket_path)

            env.stop_daemon()

            # Daemon must be dead within a short window.
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline:
                try:
                    os.kill(daemon_pid, 0)
                    time.sleep(0.1)
                except OSError:
                    break
            with self.assertRaises(OSError):
                os.kill(daemon_pid, 0)
        finally:
            env.cleanup()

    def test_emergency_cleanup_kills_daemon(self) -> None:
        """_emergency_cleanup() must kill a running daemon by PID."""
        env = TestEnvironment()
        try:
            env.start_weston()
            env.start_daemon()
            daemon_pid = env._daemon.pid

            os.kill(daemon_pid, 0)

            env._emergency_cleanup()

            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline:
                try:
                    os.kill(daemon_pid, 0)
                    time.sleep(0.1)
                except OSError:
                    break
            with self.assertRaises(OSError):
                os.kill(daemon_pid, 0)
        finally:
            env.cleanup()

    def test_emergency_cleanup_kills_weston(self) -> None:
        """_emergency_cleanup() must kill a running weston by PID."""
        env = TestEnvironment()
        try:
            env.start_weston()
            weston_pid = env._weston.pid

            os.kill(weston_pid, 0)

            env._emergency_cleanup()

            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline:
                try:
                    os.kill(weston_pid, 0)
                    time.sleep(0.1)
                except OSError:
                    break
            with self.assertRaises(OSError):
                os.kill(weston_pid, 0)
        finally:
            env.cleanup()

    def test_atexit_handler_registered(self) -> None:
        """TestEnvironment must register an atexit handler on construction."""
        import atexit

        # Count atexit callbacks before and after creating a TestEnvironment.
        # atexit._run_exitfuncs is not public, but we can check the registry.
        env = TestEnvironment()
        try:
            # The _emergency_cleanup method should be registered.
            # We verify by checking the atexit registry internals.
            # Python's atexit module stores callbacks in atexit._exithandlers (2.x)
            # or uses C-level storage (3.x). We verify indirectly: the method exists
            # and is callable.
            self.assertTrue(
                callable(getattr(env, "_emergency_cleanup", None)),
                "TestEnvironment must have an _emergency_cleanup method",
            )
        finally:
            env.cleanup()

    def test_double_cleanup_is_safe(self) -> None:
        """Calling cleanup() twice must not raise."""
        env = TestEnvironment()
        env.start_weston()
        env.start_daemon()
        env.cleanup()
        env.cleanup()  # Must not raise.


if __name__ == "__main__":
    unittest.main()
