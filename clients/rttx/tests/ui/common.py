"""Shared fixture and helpers for AT-SPI2 UI tests."""

import os
import subprocess
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402


# Path to the debug binary (faster to build than release, good enough for UI tests).
REPO_ROOT = os.path.abspath(
    os.path.join(os.path.dirname(__file__), "..", "..", "..", "..")
)
TARGET_DIR = os.environ.get("CARGO_TARGET_DIR", os.path.join(REPO_ROOT, "target"))
BINARY = os.path.join(TARGET_DIR, "debug", "rttx")

# Private Wayland socket name — avoids any clash with the user's compositor.
WAYLAND_SOCKET = "rttx-test"

# Dev mode: uses app ID io.github.IllyaYalovyy.rttx.Devel so the test instance
# never conflicts with the production rttx the user runs for daily work.
DEV_APP_ID = "io.github.IllyaYalovyy.rttx.Devel"
DEV_OBJECT_PATH = "/io/github/IllyaYalovyy/rttx/Devel"


def find_by_role_and_name(
    root: Atspi.Accessible, role: Atspi.Role, name: str
) -> Atspi.Accessible | None:
    """Return the first node matching both *role* and *name* (case-insensitive)."""
    for node in find_all_by_role(root, role):
        try:
            if node.get_name().lower() == name.lower():
                return node
        except Exception:  # noqa: BLE001
            pass
    return None


def click(node: Atspi.Accessible) -> bool:
    """Trigger the click action (index 0) on an accessible node."""
    try:
        return node.do_action(0)
    except Exception:  # noqa: BLE001
        return False


def find_all_by_role(root: Atspi.Accessible, role: Atspi.Role) -> list:
    """Depth-first search for all accessible nodes with the given role."""
    results = []
    _collect(root, role, results)
    return results


def _collect(node: Atspi.Accessible, role: Atspi.Role, out: list) -> None:
    try:
        if node.get_role() == role:
            out.append(node)
        count = node.get_child_count()
        for i in range(count):
            child = node.get_child_at_index(i)
            if child is not None:
                _collect(child, role, out)
    except Exception:  # noqa: BLE001 — AT-SPI calls can raise on stale refs
        pass


def find_app_by_pid(pid: int, timeout: float = 10.0) -> Atspi.Accessible | None:
    """Poll the AT-SPI desktop until the application with *pid* appears."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        desktop = Atspi.get_desktop(0)
        count = desktop.get_child_count()
        for i in range(count):
            try:
                app = desktop.get_child_at_index(i)
                if app is not None and app.get_process_id() == pid:
                    return app
            except Exception:  # noqa: BLE001
                pass
        time.sleep(0.2)
    return None


def wait_for_role(
    root: Atspi.Accessible,
    role: Atspi.Role,
    count: int = 1,
    timeout: float = 10.0,
) -> list:
    """Poll until at least *count* nodes with *role* appear under *root*."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        found = find_all_by_role(root, role)
        if len(found) >= count:
            return found
        time.sleep(0.2)
    return find_all_by_role(root, role)


class AppFixture:
    """Launches rttx on a private headless weston compositor and exposes the AT-SPI tree.

    weston --backend=headless creates a real Wayland compositor that needs no display
    hardware and no running desktop session — suitable for CI as well as local runs.
    AT-SPI2 communicates over D-Bus, independent of the display protocol, so it works
    normally on the headless compositor.

    RTTX_DEV_MODE=1 makes the app register as io.github.IllyaYalovyy.rttx.Devel,
    keeping its D-Bus name and config directory separate from the user's production
    rttx instance.
    """

    def __init__(self) -> None:
        self._weston: subprocess.Popen | None = None
        self._app: subprocess.Popen | None = None
        self.atspi_app: Atspi.Accessible | None = None

    # ------------------------------------------------------------------
    # Setup / teardown
    # ------------------------------------------------------------------

    def start(self) -> None:
        """Start weston (headless) + rttx; wait until the AT-SPI tree is populated."""
        runtime_dir = os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")

        self._weston = subprocess.Popen(
            [
                "weston",
                "--backend=headless",
                f"--socket={WAYLAND_SOCKET}",
                "--width=1280",
                "--height=800",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        # Wait for the socket to appear in XDG_RUNTIME_DIR.
        socket_path = os.path.join(runtime_dir, WAYLAND_SOCKET)
        deadline = time.monotonic() + 10.0
        while not os.path.exists(socket_path):
            if time.monotonic() > deadline:
                self.stop()
                raise RuntimeError(
                    f"weston socket {socket_path} did not appear within 10 s"
                )
            time.sleep(0.1)

        binary = os.path.abspath(BINARY)
        if not os.path.isfile(binary):
            raise FileNotFoundError(
                f"rttx debug binary not found: {binary}\n"
                "Run `cargo build` first."
            )

        # Throwaway config dir so every test run starts with no saved session state.
        tmp_config = os.path.join(
            os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"),
            "rttx-ui-test-config",
        )
        os.makedirs(tmp_config, exist_ok=True)

        env = os.environ.copy()
        env["WAYLAND_DISPLAY"] = WAYLAND_SOCKET
        env["GDK_BACKEND"] = "wayland"
        env["RTTX_DEV_MODE"] = "1"             # devel app ID — no conflict with production rttx
        env["RTTX_DISABLE_SHELL_SPAWN"] = "1"  # no real PTY; keeps tests fast
        env["XDG_CONFIG_HOME"] = tmp_config    # isolated config — no saved session to restore
        env.pop("GTK_A11Y", None)              # must NOT disable a11y — AT-SPI needs it
        env["NO_AT_BRIDGE"] = "0"              # allow the app to register with AT-SPI
        # Remove any stale display variables so GTK doesn't fall back to X11.
        env.pop("DISPLAY", None)

        self._app = subprocess.Popen(
            [binary],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        pid = self._app.pid
        app = find_app_by_pid(pid, timeout=15.0)
        if app is None:
            self.stop()
            raise RuntimeError(
                f"rttx dev instance (pid={pid}) did not appear in AT-SPI tree within 15 s"
            )
        self.atspi_app = app

    def stop(self) -> None:
        """Terminate the app and weston."""
        if self._app is not None:
            try:
                self._app.terminate()
                self._app.wait(timeout=5)
            except Exception:  # noqa: BLE001
                self._app.kill()
            self._app = None

        if self._weston is not None:
            try:
                self._weston.terminate()
                self._weston.wait(timeout=5)
            except Exception:  # noqa: BLE001
                self._weston.kill()
            self._weston = None

        self.atspi_app = None

    # ------------------------------------------------------------------
    # Convenience accessors
    # ------------------------------------------------------------------

    def terminals(self) -> list:
        """Return all TERMINAL-role nodes currently in the tree."""
        return find_all_by_role(self.atspi_app, Atspi.Role.TERMINAL)

    def window(self) -> Atspi.Accessible | None:
        """Return the first top-level window node."""
        wins = find_all_by_role(self.atspi_app, Atspi.Role.FRAME)
        return wins[0] if wins else None
