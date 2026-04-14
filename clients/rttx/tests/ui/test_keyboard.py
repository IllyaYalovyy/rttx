"""AT-SPI behavioral tests for keyboard shortcuts.

Verifies that keyboard-driven workflows produce the expected visible
results in the accessibility tree. Catches regressions where a shortcut
stops working or gets captured by VTE instead of the window.

Regression coverage for #317.
"""

import time
import unittest

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

from common import (
    AppFixture,
    find_all_by_role,
    find_showing_by_role_and_name,
    is_showing,
    wait_for_role,
    wait_for_showing_role,
)

# X11 keysyms for modifier keys.
_CONTROL_L = 0xFFE3
_SHIFT_L = 0xFFE1
_ALT_L = 0xFFE9


def _press(keysym: int) -> None:
    Atspi.generate_keyboard_event(keysym, None, Atspi.KeySynthType.PRESS)


def _release(keysym: int) -> None:
    Atspi.generate_keyboard_event(keysym, None, Atspi.KeySynthType.RELEASE)


def _send_ctrl_shift(key_keysym: int) -> None:
    """Send Ctrl+Shift+<key> via AT-SPI keyboard synthesis."""
    _press(_CONTROL_L)
    _press(_SHIFT_L)
    Atspi.generate_keyboard_event(key_keysym, None, Atspi.KeySynthType.PRESSRELEASE)
    _release(_SHIFT_L)
    _release(_CONTROL_L)


def _send_alt(key_keysym: int) -> None:
    """Send Alt+<key> via AT-SPI keyboard synthesis."""
    _press(_ALT_L)
    Atspi.generate_keyboard_event(key_keysym, None, Atspi.KeySynthType.PRESSRELEASE)
    _release(_ALT_L)


def _get_sidebar_rows(app: Atspi.Accessible) -> list[Atspi.Accessible]:
    """Return visible LIST_ITEM nodes that are direct children of the Workspaces list."""
    return [
        row
        for row in find_all_by_role(app, Atspi.Role.LIST_ITEM)
        if _is_workspace_row(row) and is_showing(row)
    ]


def _is_workspace_row(node: Atspi.Accessible) -> bool:
    try:
        parent = node.get_parent()
        return (
            parent is not None
            and parent.get_role() == Atspi.Role.LIST
            and parent.get_name() == "Workspaces"
        )
    except Exception:  # noqa: BLE001
        return False


def _wait_for_sidebar_count(
    app: Atspi.Accessible, count: int, timeout: float = 10.0
) -> list[Atspi.Accessible]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        rows = _get_sidebar_rows(app)
        if len(rows) == count:
            return rows
        time.sleep(0.3)
    return _get_sidebar_rows(app)


def _selected_sidebar_index(app: Atspi.Accessible) -> int | None:
    """Return the 0-based index of the selected (FOCUSED) sidebar row, or None."""
    for i, row in enumerate(_get_sidebar_rows(app)):
        try:
            state = row.get_state_set()
            if state.contains(Atspi.StateType.SELECTED):
                return i
        except Exception:  # noqa: BLE001
            pass
    return None


class TestKeyboardNewWorkspace(unittest.TestCase):
    """Ctrl+Shift+T must create a new workspace."""

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_ctrl_shift_t_creates_workspace(self) -> None:
        """Ctrl+Shift+T must add a second workspace sidebar row."""
        rows_before = _get_sidebar_rows(self.fixture.atspi_app)
        self.assertEqual(len(rows_before), 1, "Expected 1 sidebar row on launch")

        self.fixture.focus_terminal()
        time.sleep(0.3)
        _send_ctrl_shift(ord("t"))

        rows_after = _wait_for_sidebar_count(self.fixture.atspi_app, 2, timeout=10.0)
        self.assertEqual(
            len(rows_after), 2,
            "Ctrl+Shift+T did not create a second workspace sidebar row",
        )


class TestKeyboardCloseWorkspace(unittest.TestCase):
    """Ctrl+Shift+W must close the current pane/workspace."""

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_ctrl_shift_w_closes_last_workspace(self) -> None:
        """Ctrl+Shift+W on the only workspace must exit the app."""
        self.fixture.focus_terminal()
        time.sleep(0.3)
        _send_ctrl_shift(ord("w"))

        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            if self.fixture._app is not None and self.fixture._app.poll() is not None:
                break
            time.sleep(0.3)

        self.assertIsNotNone(
            self.fixture._app.poll(),
            "App should have exited after Ctrl+Shift+W on the only workspace",
        )


class TestKeyboardSwitchWorkspace(unittest.TestCase):
    """Alt+1..9 must switch to the workspace at that position."""

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1)
        # Create a second workspace so we can switch between them.
        self.fixture.activate_action("create-direct")
        _wait_for_sidebar_count(self.fixture.atspi_app, 2, timeout=10.0)
        time.sleep(0.5)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_alt_1_switches_to_first_workspace(self) -> None:
        """Alt+1 must select the first workspace."""
        # We're on workspace 2 after creation; switch to 1.
        _send_alt(ord("1"))
        time.sleep(0.5)

        idx = _selected_sidebar_index(self.fixture.atspi_app)
        self.assertEqual(idx, 0, f"Alt+1 should select workspace index 0, got {idx}")

    def test_alt_2_switches_to_second_workspace(self) -> None:
        """Alt+2 must select the second workspace."""
        # Switch to first, then back to second.
        _send_alt(ord("1"))
        time.sleep(0.5)
        _send_alt(ord("2"))
        time.sleep(0.5)

        idx = _selected_sidebar_index(self.fixture.atspi_app)
        self.assertEqual(idx, 1, f"Alt+2 should select workspace index 1, got {idx}")


class TestKeyboardToggleUtilitySidebar(unittest.TestCase):
    """Ctrl+Shift+B must toggle the utility (tools) sidebar."""

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_ctrl_shift_b_toggles_sidebar(self) -> None:
        """Ctrl+Shift+B must hide the tools sidebar, then show it again."""
        # The sidebar starts visible — look for the Commands page tab.
        tab_before = find_showing_by_role_and_name(
            self.fixture.atspi_app, Atspi.Role.PAGE_TAB, "Commands"
        )
        sidebar_initially_visible = tab_before is not None

        self.fixture.focus_terminal()
        time.sleep(0.3)
        _send_ctrl_shift(ord("b"))
        time.sleep(0.5)

        tab_after_toggle = find_showing_by_role_and_name(
            self.fixture.atspi_app, Atspi.Role.PAGE_TAB, "Commands"
        )

        if sidebar_initially_visible:
            self.assertIsNone(
                tab_after_toggle,
                "Ctrl+Shift+B should have hidden the tools sidebar",
            )
        else:
            self.assertIsNotNone(
                tab_after_toggle,
                "Ctrl+Shift+B should have shown the tools sidebar",
            )

        # Toggle back.
        _send_ctrl_shift(ord("b"))
        time.sleep(0.5)

        tab_restored = find_showing_by_role_and_name(
            self.fixture.atspi_app, Atspi.Role.PAGE_TAB, "Commands"
        )
        if sidebar_initially_visible:
            self.assertIsNotNone(
                tab_restored,
                "Second Ctrl+Shift+B should have restored the tools sidebar",
            )
        else:
            self.assertIsNone(
                tab_restored,
                "Second Ctrl+Shift+B should have hidden the tools sidebar again",
            )


class TestKeyboardInputSync(unittest.TestCase):
    """Ctrl+Shift+I must toggle input sync."""

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_ctrl_shift_i_toggles_input_sync(self) -> None:
        """Ctrl+Shift+I must activate the input-sync toggle action.

        After toggling, the toast 'Input sync enabled' should appear.
        """
        self.fixture.focus_terminal()
        time.sleep(0.3)
        _send_ctrl_shift(ord("i"))
        time.sleep(0.5)

        # Input sync shows a toast or changes the header bar state.
        # Look for the toast or the toggle button state change.
        toast = self.fixture.wait_for_showing_by_name(
            "Input sync enabled",
            roles=[Atspi.Role.LABEL, Atspi.Role.NOTIFICATION, Atspi.Role.STATUS_BAR],
            timeout=3.0,
        )
        # Also check for a toggle button named "Input sync" that is now pressed.
        toggle = find_showing_by_role_and_name(
            self.fixture.atspi_app, Atspi.Role.TOGGLE_BUTTON, "Input sync"
        )
        toggled = False
        if toggle is not None:
            try:
                toggled = toggle.get_state_set().contains(Atspi.StateType.CHECKED)
            except Exception:  # noqa: BLE001
                pass

        self.assertTrue(
            toast is not None or toggled,
            "Ctrl+Shift+I did not produce a visible input-sync indicator "
            "(no toast and no checked toggle button found)",
        )


class TestKeyboardSplit(unittest.TestCase):
    """Ctrl+Shift+E/O must split the focused pane."""

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_ctrl_shift_e_splits_horizontal(self) -> None:
        """Ctrl+Shift+E must create a second terminal (horizontal split)."""
        self.fixture.focus_terminal()
        time.sleep(0.3)
        _send_ctrl_shift(ord("e"))

        terminals = wait_for_showing_role(
            self.fixture.atspi_app, Atspi.Role.TERMINAL, count=2, timeout=8.0
        )
        self.assertEqual(
            len(terminals), 2,
            "Ctrl+Shift+E did not produce a second terminal (horizontal split)",
        )

    def test_ctrl_shift_o_splits_vertical(self) -> None:
        """Ctrl+Shift+O must create a second terminal (vertical split)."""
        self.fixture.focus_terminal()
        time.sleep(0.3)
        _send_ctrl_shift(ord("o"))

        terminals = wait_for_showing_role(
            self.fixture.atspi_app, Atspi.Role.TERMINAL, count=2, timeout=8.0
        )
        self.assertEqual(
            len(terminals), 2,
            "Ctrl+Shift+O did not produce a second terminal (vertical split)",
        )


if __name__ == "__main__":
    unittest.main()
