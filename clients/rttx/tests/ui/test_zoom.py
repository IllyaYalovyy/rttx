"""
AT-SPI behavioral test for pane zoom (Ctrl+Shift+Z).

The zoom toggle itself is tested at the unit and integration level
(session_lifecycle::zoom_* and state::tests::zoom_*). This test
verifies that the zoom code path does not regress the split layout
accessible tree.
"""

import unittest

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

from common import AppFixture, click, find_by_role_and_name, wait_for_role


class TestPaneZoom(unittest.TestCase):

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_split_layout_accessible_tree_intact_with_zoom_code(self) -> None:
        """Split must still produce two TERMINAL nodes with zoom code present."""
        btn = find_by_role_and_name(
            self.fixture.atspi_app, Atspi.Role.PUSH_BUTTON, "Split vertically"
        )
        self.assertIsNotNone(btn, "'Split vertically' button not found")
        click(btn)

        terminals = wait_for_role(
            self.fixture.atspi_app, Atspi.Role.TERMINAL, count=2, timeout=8.0
        )
        self.assertEqual(
            len(terminals), 2,
            "Vertical split did not produce two TERMINAL accessible nodes"
        )


if __name__ == "__main__":
    unittest.main()
