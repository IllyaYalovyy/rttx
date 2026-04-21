"""Shared fixture and helpers for AT-SPI2 UI tests."""

import atexit
import ctypes
import os
import shutil
import signal
import subprocess
import tempfile
import time
import uuid

import gi


def _set_pdeathsig() -> None:
    """Ask the kernel to send SIGTERM when the parent process dies.

    This prevents orphaned child processes when the test runner is
    interrupted or crashes before tearDown can run cleanup.
    """
    try:
        ctypes.CDLL("libc.so.6", use_errno=True).prctl(1, signal.SIGTERM)
    except OSError:
        pass

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402


# Path to the debug binary (faster to build than release, good enough for UI tests).
REPO_ROOT = os.path.abspath(
    os.path.join(os.path.dirname(__file__), "..", "..", "..", "..")
)
TARGET_DIR = os.environ.get("CARGO_TARGET_DIR", os.path.join(REPO_ROOT, "target"))
BINARY = os.path.join(TARGET_DIR, "debug", "rttx")
DAEMON_BINARY = os.path.join(TARGET_DIR, "debug", "rttx-server")

# Private Wayland socket name — avoids any clash with the user's compositor.
WAYLAND_SOCKET = "rttx-test"
DEV_CONFIG_DIR = "rttx-devel"

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


def click_center(node: Atspi.Accessible) -> bool:
    """Trigger a primary-button click at the center of *node*."""
    try:
        rect = node.get_extents(Atspi.CoordType.SCREEN)
        return Atspi.generate_mouse_event(
            rect.x + rect.width // 2,
            rect.y + rect.height // 2,
            "b1c",
        )
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


def is_showing(node: Atspi.Accessible) -> bool:
    """Return whether *node* is currently visible in the accessibility tree."""
    try:
        state_set = node.get_state_set()
        return state_set.contains(Atspi.StateType.SHOWING) and state_set.contains(
            Atspi.StateType.VISIBLE
        )
    except Exception:  # noqa: BLE001
        return False


def find_showing_by_role_and_name(
    root: Atspi.Accessible, role: Atspi.Role, name: str
) -> Atspi.Accessible | None:
    """Return the first visible node matching both *role* and *name*."""
    for node in find_all_by_role(root, role):
        try:
            if is_showing(node) and node.get_name().lower() == name.lower():
                return node
        except Exception:  # noqa: BLE001
            pass
    return None


def find_showing_by_name(
    root: Atspi.Accessible,
    name: str,
    roles: list[Atspi.Role] | None = None,
) -> Atspi.Accessible | None:
    """Return the first visible node matching *name* across *roles*.

    If *roles* is ``None``, searches PUSH_BUTTON, MENU_ITEM, and LABEL.
    """
    if roles is None:
        roles = [Atspi.Role.PUSH_BUTTON, Atspi.Role.MENU_ITEM, Atspi.Role.LABEL]
    for role in roles:
        found = find_showing_by_role_and_name(root, role, name)
        if found is not None:
            return found
    return None


def wait_for_showing_by_name(
    root: Atspi.Accessible,
    name: str,
    roles: list[Atspi.Role] | None = None,
    timeout: float = 10.0,
) -> Atspi.Accessible | None:
    """Poll until a visible node with *name* appears across *roles*."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        found = find_showing_by_name(root, name, roles)
        if found is not None:
            return found
        time.sleep(0.2)
    return find_showing_by_name(root, name, roles)


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


def wait_for_name(
    root: Atspi.Accessible,
    role: Atspi.Role,
    name: str,
    timeout: float = 10.0,
    showing_only: bool = False,
) -> Atspi.Accessible | None:
    """Poll until a node with *role* and *name* appears."""
    deadline = time.monotonic() + timeout
    finder = find_showing_by_role_and_name if showing_only else find_by_role_and_name
    while time.monotonic() < deadline:
        found = finder(root, role, name)
        if found is not None:
            return found
        time.sleep(0.2)
    return finder(root, role, name)


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


def wait_for_showing_role(
    root: Atspi.Accessible,
    role: Atspi.Role,
    count: int = 1,
    timeout: float = 10.0,
) -> list:
    """Poll until at least *count* visible nodes with *role* appear under *root*."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        found = [node for node in find_all_by_role(root, role) if is_showing(node)]
        if len(found) >= count:
            return found
        time.sleep(0.2)
    return [node for node in find_all_by_role(root, role) if is_showing(node)]


def terminal_text(node: Atspi.Accessible, max_chars: int | None = None) -> str:
    """Return the visible terminal text exposed through the AT-SPI text interface."""
    try:
        char_count = node.get_character_count()
        if char_count <= 0:
            return ""
        start = 0
        if max_chars is not None and char_count > max_chars:
            start = char_count - max_chars
        return "".join(chr(node.get_character_at_offset(i)) for i in range(start, char_count))
    except Exception:  # noqa: BLE001
        return ""


def terminal_caret_offset(node: Atspi.Accessible) -> int:
    """Return the caret offset for a terminal accessible node."""
    try:
        return node.get_caret_offset()
    except Exception:  # noqa: BLE001
        return -1


class TestEnvironment:
    """Isolated weston/XDG environment shared by one UI test."""

    def __init__(self, extra_env: dict[str, str] | None = None) -> None:
        self.root_dir = tempfile.mkdtemp(prefix="rttx-ui-test-")
        self.runtime_dir = os.path.join(self.root_dir, "run")
        self.config_home = os.path.join(self.root_dir, "config")
        self.cache_home = os.path.join(self.root_dir, "cache")
        self.state_home = os.path.join(self.root_dir, "state")
        self.extra_env = extra_env or {}
        os.makedirs(self.runtime_dir, mode=0o700, exist_ok=True)
        os.makedirs(self.config_home, exist_ok=True)
        os.makedirs(self.cache_home, exist_ok=True)
        os.makedirs(self.state_home, exist_ok=True)
        self.wayland_socket = f"{WAYLAND_SOCKET}-{uuid.uuid4().hex[:8]}"
        self._weston: subprocess.Popen | None = None
        self._daemon: subprocess.Popen | None = None
        atexit.register(self._emergency_cleanup)

    def _emergency_cleanup(self) -> None:
        """Kill managed processes by PID — last-resort handler for abnormal exits."""
        for proc in (self._daemon, self._weston):
            if proc is not None and proc.poll() is None:
                try:
                    proc.kill()
                    proc.wait(timeout=5)
                except Exception:  # noqa: BLE001
                    pass

    @property
    def weston_socket_path(self) -> str:
        return os.path.join(self.runtime_dir, self.wayland_socket)

    @property
    def daemon_socket_path(self) -> str:
        return os.path.join(
            self.runtime_dir, "rttx-server-devel", "v1", "rttx-server.sock"
        )

    @property
    def sessions_file(self) -> str:
        return os.path.join(self.config_home, DEV_CONFIG_DIR, "sessions.json")

    def process_env(self, disable_shell_spawn: bool = False) -> dict[str, str]:
        """Environment for the private weston/client/daemon processes."""
        env = os.environ.copy()
        env["WAYLAND_DISPLAY"] = self.wayland_socket
        env["GDK_DEBUG"] = "no-portals"
        env["ADW_DISABLE_PORTAL"] = "1"
        env["GDK_BACKEND"] = "wayland"
        env["RTTX_DEV_MODE"] = "1"
        env["XDG_RUNTIME_DIR"] = self.runtime_dir
        env["XDG_CONFIG_HOME"] = self.config_home
        env["XDG_CACHE_HOME"] = self.cache_home
        env["XDG_STATE_HOME"] = self.state_home
        env["NO_AT_BRIDGE"] = "0"
        env["PATH"] = os.path.join(TARGET_DIR, "debug") + os.pathsep + env["PATH"]
        env.pop("GTK_A11Y", None)
        env.pop("DISPLAY", None)
        env.update(self.extra_env)
        if disable_shell_spawn:
            env["RTTX_DISABLE_SHELL_SPAWN"] = "1"
        else:
            env.pop("RTTX_DISABLE_SHELL_SPAWN", None)
        return env

    def start_weston(self) -> None:
        """Start the private headless compositor."""
        if self._weston is not None and self._weston.poll() is None:
            return

        self._weston = subprocess.Popen(
            [
                "weston",
                "--backend=headless",
                f"--socket={self.wayland_socket}",
                "--width=1280",
                "--height=800",
            ],
            env={**os.environ, "XDG_RUNTIME_DIR": self.runtime_dir},
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            preexec_fn=_set_pdeathsig,
        )

        deadline = time.monotonic() + 10.0
        while not os.path.exists(self.weston_socket_path):
            if time.monotonic() > deadline:
                self.stop_weston()
                raise RuntimeError(
                    f"weston socket {self.weston_socket_path} did not appear within 10 s"
                )
            time.sleep(0.1)

    def stop_weston(self) -> None:
        """Terminate the private compositor."""
        if self._weston is None:
            return
        try:
            self._weston.terminate()
            self._weston.wait(timeout=5)
        except Exception:  # noqa: BLE001
            self._weston.kill()
        self._weston = None

    def start_daemon(self) -> None:
        """Start a foreground rttx-server bound to the private XDG roots."""
        if not os.path.isfile(DAEMON_BINARY):
            raise FileNotFoundError(
                f"rttx-server debug binary not found: {DAEMON_BINARY}\n"
                "Run `cargo build --workspace` or `cargo build -p rttx-server` first."
            )

        if self._daemon is not None and self._daemon.poll() is None:
            return

        self._daemon = subprocess.Popen(
            [DAEMON_BINARY, "start", "--foreground"],
            env=self.process_env(),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            preexec_fn=_set_pdeathsig,
        )

        deadline = time.monotonic() + 10.0
        while not os.path.exists(self.daemon_socket_path):
            if self._daemon.poll() is not None:
                raise RuntimeError("rttx-server exited before the daemon socket appeared")
            if time.monotonic() > deadline:
                self.stop_daemon()
                raise RuntimeError(
                    f"daemon socket {self.daemon_socket_path} did not appear within 10 s"
                )
            time.sleep(0.1)

    def stop_daemon(self) -> None:
        """Cooperatively stop the private daemon if it is running."""
        if self._daemon is None and not os.path.exists(self.daemon_socket_path):
            return

        socket_available = os.path.exists(self.daemon_socket_path)
        if socket_available:
            subprocess.run(
                [DAEMON_BINARY, "stop"],
                env=self.process_env(),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )

        if self._daemon is not None:
            if socket_available:
                try:
                    self._daemon.wait(timeout=10)
                except Exception:  # noqa: BLE001
                    self._daemon.terminate()
                    try:
                        self._daemon.wait(timeout=5)
                    except Exception:  # noqa: BLE001
                        self._daemon.kill()
            else:
                # Socket already gone — kill by PID directly.
                self._daemon.terminate()
                try:
                    self._daemon.wait(timeout=5)
                except Exception:  # noqa: BLE001
                    self._daemon.kill()
            self._daemon = None

        deadline = time.monotonic() + 5.0
        while os.path.exists(self.daemon_socket_path):
            if time.monotonic() > deadline:
                break
            time.sleep(0.1)

    def restart_daemon(self) -> None:
        """Restart the private daemon while preserving config/cache state."""
        self.stop_daemon()
        self.start_daemon()

    def clear_saved_state(self) -> None:
        """Remove the GUI sessions file and new store documents so startup uses defaults."""
        for path in [
            self.sessions_file,
            os.path.join(self.state_home, DEV_CONFIG_DIR, "client", "workspaces.json"),
            os.path.join(self.state_home, DEV_CONFIG_DIR, "client", "ui.json"),
            os.path.join(self.cache_home, DEV_CONFIG_DIR, "runtime-cache.json"),
        ]:
            try:
                os.remove(path)
            except FileNotFoundError:
                pass

    def cleanup(self) -> None:
        """Stop all managed processes and remove the private temp roots."""
        self.stop_daemon()
        self.stop_weston()
        shutil.rmtree(self.root_dir, ignore_errors=True)


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

    def __init__(
        self,
        disable_shell_spawn: bool = True,
        extra_env: dict[str, str] | None = None,
    ) -> None:
        self.environment = TestEnvironment(extra_env=extra_env)
        self.disable_shell_spawn = disable_shell_spawn
        self._app: subprocess.Popen | None = None
        self.atspi_app: Atspi.Accessible | None = None

    # ------------------------------------------------------------------
    # Setup / teardown
    # ------------------------------------------------------------------

    def start(self) -> None:
        """Start weston (headless) + rttx; wait until the AT-SPI tree is populated."""
        self.environment.start_weston()
        self.start_app()

    def start_app(self) -> None:
        """Start the GTK client inside the already-prepared private environment."""

        binary = os.path.abspath(BINARY)
        if not os.path.isfile(binary):
            raise FileNotFoundError(
                f"rttx debug binary not found: {binary}\n"
                "Run `cargo build` first."
            )

        self._app = subprocess.Popen(
            [binary],
            env=self.environment.process_env(
                disable_shell_spawn=self.disable_shell_spawn
            ),
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

    def stop_app(self) -> None:
        """Terminate the client while keeping the private environment alive."""
        if self._app is not None:
            try:
                self._app.terminate()
                self._app.wait(timeout=5)
            except Exception:  # noqa: BLE001
                self._app.kill()
            self._app = None

        self.atspi_app = None

    def restart_app(self) -> None:
        """Restart the client without resetting daemon/config state."""
        self.stop_app()
        self.start_app()

    def start_daemon(self) -> None:
        """Start the private rttx-server instance."""
        self.environment.start_daemon()

    def stop_daemon(self) -> None:
        """Stop the private rttx-server instance."""
        self.environment.stop_daemon()

    def restart_daemon(self) -> None:
        """Restart the private rttx-server instance."""
        self.environment.restart_daemon()

    def clear_saved_state(self) -> None:
        """Delete the GUI sessions file so startup recovery uses daemon inventory."""
        self.environment.clear_saved_state()

    def stop(self) -> None:
        """Terminate all managed processes and remove the private temp roots."""
        self.stop_app()
        self.environment.cleanup()

    # ------------------------------------------------------------------
    # Convenience accessors
    # ------------------------------------------------------------------

    def terminals(self) -> list:
        """Return all TERMINAL-role nodes currently in the tree."""
        return find_all_by_role(self.atspi_app, Atspi.Role.TERMINAL)

    def showing_terminals(self) -> list:
        """Return all visible TERMINAL-role nodes currently in the tree."""
        return [node for node in self.terminals() if is_showing(node)]

    def wait_for_showing_by_name(
        self, name: str, roles: list | None = None, timeout: float = 10.0
    ) -> Atspi.Accessible | None:
        """Wait until a visible node with *name* appears across *roles*."""
        return wait_for_showing_by_name(self.atspi_app, name, roles, timeout=timeout)

    def window(self) -> Atspi.Accessible | None:
        """Return the first top-level window node."""
        wins = find_all_by_role(self.atspi_app, Atspi.Role.FRAME)
        return wins[0] if wins else None

    def focus_terminal(self, terminal: Atspi.Accessible | None = None) -> bool:
        """Focus the visible terminal by clicking its center."""
        target = terminal or (self.showing_terminals()[0] if self.showing_terminals() else None)
        if target is None:
            return False
        return click_center(target)

    def send_text(self, text: str) -> bool:
        """Inject text into the focused widget through the AT-SPI keyboard bridge."""
        return Atspi.generate_keyboard_event(0, text, Atspi.KeySynthType.STRING)

    def activate_action(self, action_name: str, parameter: str | None = None) -> None:
        """Activate a GIO application action via D-Bus.

        This bypasses AT-SPI widget interaction, which is unreliable for
        MenuButton popovers on headless compositors.
        """
        from gi.repository import Gio, GLib

        bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        params = GLib.Variant("(sava{sv})", (
            action_name,
            [GLib.Variant("s", parameter)] if parameter else [],
            {},
        ))
        bus.call_sync(
            DEV_APP_ID,
            DEV_OBJECT_PATH,
            "org.freedesktop.Application",
            "ActivateAction",
            params,
            None,
            Gio.DBusCallFlags.NONE,
            5000,
            None,
        )

    def showing_name(self, role: Atspi.Role, name: str) -> bool:
        """Return whether a visible accessible node with *role* and *name* exists."""
        return find_showing_by_role_and_name(self.atspi_app, role, name) is not None

    def wait_for_showing_name(
        self, role: Atspi.Role, name: str, timeout: float = 10.0
    ) -> Atspi.Accessible | None:
        """Wait until a visible accessible node with *role* and *name* appears."""
        return wait_for_name(
            self.atspi_app, role, name, timeout=timeout, showing_only=True
        )

    def wait_for_terminal_count(self, count: int, timeout: float = 10.0) -> list:
        """Wait until exactly *count* visible terminals are exposed."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            terminals = self.showing_terminals()
            if len(terminals) == count:
                return terminals
            time.sleep(0.2)
        return self.showing_terminals()

    def wait_for_terminal_text(
        self, needle: str, timeout: float = 10.0
    ) -> Atspi.Accessible | None:
        """Wait until a visible terminal exposes *needle* in its accessible text."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for terminal in self.showing_terminals():
                if needle in terminal_text(terminal):
                    return terminal
            time.sleep(0.2)
        for terminal in self.showing_terminals():
            if needle in terminal_text(terminal):
                return terminal
        return None
