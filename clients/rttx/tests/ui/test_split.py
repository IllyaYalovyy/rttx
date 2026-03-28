"""UI test: splitting a pane creates a second terminal.

This is the regression test for the bug where split_terminal_in_place did not
call ensure_shell_spawned_when_ready(), leaving the new pane blank.
"""

import unittest

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

from common import AppFixture, click, find_by_role_and_name, wait_for_role


class TestSplit(unittest.TestCase):

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_split_vertical_creates_second_terminal(self) -> None:
        """'Split vertically' button must produce exactly two TERMINAL nodes."""
        btn = find_by_role_and_name(
            self.fixture.atspi_app, Atspi.Role.PUSH_BUTTON, "Split vertically"
        )
        self.assertIsNotNone(btn, "'Split vertically' button not found in AT-SPI tree")
        click(btn)

        terminals = wait_for_role(
            self.fixture.atspi_app, Atspi.Role.TERMINAL, count=2, timeout=8.0
        )
        self.assertEqual(
            len(terminals),
            2,
            "Vertical split did not produce a second TERMINAL accessible node.\n"
            "Regression: ensure_shell_spawned_when_ready() must be called in "
            "split_terminal_in_place().",
        )

    def test_split_horizontal_creates_second_terminal(self) -> None:
        """'Split horizontally' button must produce exactly two TERMINAL nodes."""
        btn = find_by_role_and_name(
            self.fixture.atspi_app, Atspi.Role.PUSH_BUTTON, "Split horizontally"
        )
        self.assertIsNotNone(btn, "'Split horizontally' button not found in AT-SPI tree")
        click(btn)

        terminals = wait_for_role(
            self.fixture.atspi_app, Atspi.Role.TERMINAL, count=2, timeout=8.0
        )
        self.assertEqual(
            len(terminals),
            2,
            "Horizontal split did not produce a second TERMINAL accessible node.",
        )


if __name__ == "__main__":
    unittest.main()
