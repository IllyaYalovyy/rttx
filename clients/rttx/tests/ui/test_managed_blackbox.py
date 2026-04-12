"""Black-box client+daemon AT-SPI tests for managed workspace recovery."""

import unittest

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

from common import AppFixture, click, click_center, wait_for_name


class TestManagedBlackBox(unittest.TestCase):

    def setUp(self) -> None:
        self.fixture = AppFixture(disable_shell_spawn=False)
        self.fixture.start_daemon()
        self.fixture.start()

    def tearDown(self) -> None:
        self.fixture.stop()

    def _create_managed_workspace(self) -> None:
        button = self.fixture.wait_for_showing_name(
            Atspi.Role.TOGGLE_BUTTON, "New workspace"
        )
        self.assertIsNotNone(button, "New workspace button not visible")
        click_center(button)
        import time
        time.sleep(1.0)
        # gio::Menu items in a PopoverMenu appear as PUSH_BUTTON in AT-SPI.
        local_item = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Local", timeout=10.0
        )
        self.assertIsNotNone(local_item, "Local host item not visible in New menu")
        click(local_item)

        # The New Workspace dialog opens — select "Home" to create the workspace.
        home_button = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Home", timeout=10.0
        )
        self.assertIsNotNone(home_button, "Home place button not visible in dialog")
        click(home_button)

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
        button = self.fixture.wait_for_showing_name(
            Atspi.Role.TOGGLE_BUTTON, "New workspace"
        )
        self.assertIsNotNone(button, "New workspace button not visible")
        click_center(button)
        import time
        time.sleep(1.0)
        local_item = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Local", timeout=10.0
        )
        self.assertIsNotNone(local_item, "Local host item not visible in New menu")
        click(local_item)

        # The New Workspace dialog opens — select "Home" to create the workspace.
        home_button = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Home", timeout=10.0
        )
        self.assertIsNotNone(home_button, "Home place button not visible in dialog")
        click(home_button)

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


if __name__ == "__main__":
    unittest.main()
