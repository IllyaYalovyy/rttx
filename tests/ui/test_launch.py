"""UI test: app launches and the terminal pane is accessible."""

import unittest

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

from common import AppFixture, wait_for_role


class TestLaunch(unittest.TestCase):

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_window_is_accessible(self) -> None:
        """The main window must appear in the AT-SPI tree."""
        win = self.fixture.window()
        self.assertIsNotNone(win, "No FRAME role found — window not accessible")

    def test_terminal_pane_exists(self) -> None:
        """At least one TERMINAL role must be present after launch."""
        terminals = wait_for_role(
            self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1
        )
        self.assertGreaterEqual(
            len(terminals),
            1,
            "No TERMINAL accessible node found — initial pane not rendered",
        )

    def test_exactly_one_terminal_on_launch(self) -> None:
        """A fresh session must start with exactly one terminal pane."""
        terminals = wait_for_role(
            self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1
        )
        self.assertEqual(
            len(terminals),
            1,
            f"Expected 1 terminal on launch, found {len(terminals)}",
        )


if __name__ == "__main__":
    unittest.main()
