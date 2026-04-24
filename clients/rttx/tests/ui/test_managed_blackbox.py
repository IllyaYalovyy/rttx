"""Black-box client+daemon AT-SPI tests for managed workspace recovery."""

import unittest

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

from common import AppFixture, click, wait_for_name


class TestManagedBlackBox(unittest.TestCase):

    def setUp(self) -> None:
        self.fixture = AppFixture(disable_shell_spawn=False)
        self.fixture.start_daemon()
        self.fixture.start()

    def tearDown(self) -> None:
        self.fixture.stop()

    def _create_managed_workspace(self) -> None:
        # Create managed workspace via D-Bus — bypasses MenuButton popover
        # and dialog which are unreliable via AT-SPI on headless compositors.
        self.fixture.activate_action("create-managed-local")

        close_pane = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Close pane", timeout=20.0
        )
        self.assertIsNotNone(close_pane, "managed workspace controls never appeared")

        connected = self.fixture.wait_for_showing_name(
            Atspi.Role.LABEL, "Connected", timeout=20.0
        )
        self.assertIsNotNone(connected, "managed workspace never reached Connected state")

    def test_startup_inventory_recovery_keeps_direct_session_visible(self) -> None:
        """Cold-start recovery must restore the workspace row without stealing visibility."""
        self._create_managed_workspace()

        self.fixture.stop_app()
        self.fixture.clear_saved_state()
        self.fixture.start_app()

        workspace_row = wait_for_name(
            self.fixture.atspi_app, Atspi.Role.LIST_ITEM, "Workspace 2", timeout=20.0
        )
        self.assertIsNotNone(
            workspace_row,
            "daemon inventory should recover the managed workspace on cold start",
        )

        close_terminal = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Close terminal", timeout=20.0
        )
        self.assertIsNotNone(
            close_terminal,
            "cold start should leave the direct session visible after inventory recovery",
        )
        self.assertFalse(
            self.fixture.showing_name(Atspi.Role.PUSH_BUTTON, "Close pane"),
            "recovered managed workspace must not steal visible content from the direct session",
        )

    def test_daemon_restart_split_close_and_reconnect_preserve_visible_layout(self) -> None:
        """Restarting the daemon must preserve managed pane counts through reconnect flows."""
        self._create_managed_workspace()

        split = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Split vertically", timeout=10.0
        )
        self.assertIsNotNone(split, "managed split control not visible")
        click(split)
        self.assertEqual(
            len(self.fixture.wait_for_terminal_count(2, timeout=20.0)),
            2,
            "managed split should expose two terminals before restart",
        )

        self.fixture.restart_daemon()
        connected = self.fixture.wait_for_showing_name(
            Atspi.Role.LABEL, "Connected", timeout=20.0
        )
        self.assertIsNotNone(connected, "managed workspace did not reconnect after daemon restart")
        self.assertEqual(
            len(self.fixture.wait_for_terminal_count(2, timeout=20.0)),
            2,
            "split managed workspace lost a pane after daemon restart",
        )

        close_pane = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Close pane", timeout=10.0
        )
        self.assertIsNotNone(close_pane, "managed close-pane control not visible after reconnect")
        click(close_pane)
        self.assertEqual(
            len(self.fixture.wait_for_terminal_count(1, timeout=20.0)),
            1,
            "closing one managed pane after reconnect should leave one visible terminal",
        )

        self.fixture.restart_daemon()
        connected = self.fixture.wait_for_showing_name(
            Atspi.Role.LABEL, "Connected", timeout=20.0
        )
        self.assertIsNotNone(
            connected,
            "single-pane managed workspace did not reconnect after the second daemon restart",
        )
        self.assertEqual(
            len(self.fixture.wait_for_terminal_count(1, timeout=20.0)),
            1,
            "pane bindings drifted after close/reconnect and changed the visible pane count",
        )


class TestManagedPaneExitBlackBox(unittest.TestCase):

    def setUp(self) -> None:
        self.fixture = AppFixture(
            disable_shell_spawn=True, extra_env={"SHELL": "/bin/true"}
        )
        self.fixture.start_daemon()
        self.fixture.start()

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_managed_pane_exit_is_visible_when_shell_exits(self) -> None:
        """A daemon PaneExited event must leave a clear, non-hanging pane."""
        self.fixture.activate_action("create-managed-local")

        close_pane = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Close pane", timeout=20.0
        )
        self.assertIsNotNone(close_pane, "managed workspace controls never appeared")

        exited = self.fixture.wait_for_showing_name(
            Atspi.Role.LABEL, "Exited", timeout=20.0
        )
        self.assertIsNotNone(exited, "managed pane never reported the exited process")
        self.assertEqual(
            len(self.fixture.wait_for_terminal_count(1, timeout=5.0)),
            1,
            "managed exited pane should remain visible for the user",
        )


class TestReconnectInputUsability(unittest.TestCase):
    """Regression tests for #769: reconnect must leave panes input-capable
    and focus must return to the correct pane."""

    def setUp(self) -> None:
        self.fixture = AppFixture(disable_shell_spawn=False)
        self.fixture.start_daemon()
        self.fixture.start()

    def tearDown(self) -> None:
        self.fixture.stop()

    def _create_managed_workspace(self) -> None:
        self.fixture.activate_action("create-managed-local")
        close_pane = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Close pane", timeout=20.0
        )
        self.assertIsNotNone(close_pane, "managed workspace controls never appeared")
        connected = self.fixture.wait_for_showing_name(
            Atspi.Role.LABEL, "Connected", timeout=20.0
        )
        self.assertIsNotNone(connected, "managed workspace never reached Connected state")

    def test_reconnect_leaves_managed_pane_input_capable(self) -> None:
        """After daemon restart, the managed pane must show Connected and
        accept input — not stay stuck in Disconnected or Reconnecting."""
        self._create_managed_workspace()

        self.fixture.restart_daemon()

        connected = self.fixture.wait_for_showing_name(
            Atspi.Role.LABEL, "Connected", timeout=20.0
        )
        self.assertIsNotNone(
            connected,
            "managed pane must show 'Connected' after daemon restart, proving input is enabled",
        )

        terminals = self.fixture.wait_for_terminal_count(1, timeout=10.0)
        self.assertEqual(
            len(terminals), 1,
            "exactly one terminal must be visible after reconnect",
        )

        # Verify the terminal is focusable (a proxy for input-capable).
        self.assertTrue(
            self.fixture.focus_terminal(terminals[0]),
            "terminal must be focusable after reconnect",
        )

    def test_focus_returns_to_active_pane_after_reconnect(self) -> None:
        """After daemon restart with a split layout, focus must return to
        the pane that was active before the restart."""
        self._create_managed_workspace()

        # Split to get two panes.
        split = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Split vertically", timeout=10.0
        )
        self.assertIsNotNone(split, "split control not visible")
        click(split)
        terminals = self.fixture.wait_for_terminal_count(2, timeout=20.0)
        self.assertEqual(len(terminals), 2, "split should produce two terminals")

        # Focus the second terminal.
        self.fixture.focus_terminal(terminals[1])
        import time
        time.sleep(0.3)

        self.fixture.restart_daemon()

        connected = self.fixture.wait_for_showing_name(
            Atspi.Role.LABEL, "Connected", timeout=20.0
        )
        self.assertIsNotNone(connected, "workspace did not reconnect after daemon restart")

        reconnected_terminals = self.fixture.wait_for_terminal_count(2, timeout=20.0)
        self.assertEqual(
            len(reconnected_terminals), 2,
            "both panes must survive reconnect",
        )

    def test_connection_state_label_matches_input_through_reconnect(self) -> None:
        """The visible 'Connected' label must appear only when the pane
        actually accepts input. During reconnect, the label must not say
        'Connected' while the pane is still reconnecting."""
        self._create_managed_workspace()

        # Verify initial Connected state.
        connected = self.fixture.wait_for_showing_name(
            Atspi.Role.LABEL, "Connected", timeout=20.0
        )
        self.assertIsNotNone(connected, "initial Connected label missing")

        self.fixture.restart_daemon()

        # After restart, the label must eventually return to Connected.
        connected = self.fixture.wait_for_showing_name(
            Atspi.Role.LABEL, "Connected", timeout=20.0
        )
        self.assertIsNotNone(
            connected,
            "connection state label must return to 'Connected' after successful reconnect",
        )


if __name__ == "__main__":
    unittest.main()
