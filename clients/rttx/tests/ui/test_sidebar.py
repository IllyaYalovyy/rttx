"""UI test: tools sidebar is vertically oriented and toggles correctly."""

import time
import unittest

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

from common import (
    AppFixture,
    click,
    find_all_by_role,
    find_by_role_and_name,
    wait_for_role,
)


def get_extents(node: Atspi.Accessible):
    """Return (x, y, width, height) of *node*, or None on failure."""
    try:
        rect = node.get_extents(Atspi.CoordType.WINDOW)
        return rect.x, rect.y, rect.width, rect.height
    except Exception:  # noqa: BLE001
        return None


class TestSidebar(unittest.TestCase):

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1)

    def tearDown(self) -> None:
        self.fixture.stop()

    def _tools_toggle_button(self) -> Atspi.Accessible:
        btn = find_by_role_and_name(
            self.fixture.atspi_app, Atspi.Role.TOGGLE_BUTTON, "Tools"
        )
        self.assertIsNotNone(btn, "'Tools' toggle button not found in AT-SPI tree")
        return btn

    def test_sidebar_vertical_layout(self) -> None:
        """The utility sidebar content must sit below the tab bar, not beside it.

        In the correct vertical-box layout, the Places stack page (content)
        has a greater y than the Places page-tab (the switcher button).
        In the broken horizontal-box layout both would share the same y because
        the switcher and content are rendered side-by-side in the same row.
        """
        # Sidebar starts visible. Close and reopen to ensure a clean state.
        click(self._tools_toggle_button())
        time.sleep(0.4)
        click(self._tools_toggle_button())
        time.sleep(0.4)

        # The tab button (in the ViewSwitcher / StackSwitcher row).
        tab = find_by_role_and_name(
            self.fixture.atspi_app, Atspi.Role.PAGE_TAB, "Places"
        )
        if tab is None:
            self.skipTest("Places page tab not found in AT-SPI tree")

        # The content panel (the actual Places stack page below the tab bar).
        content_panels = [
            n for n in find_all_by_role(self.fixture.atspi_app, Atspi.Role.PANEL)
            if n.get_name() == "Places"
        ]
        if not content_panels:
            self.skipTest("Places content panel not found in AT-SPI tree")
        content = content_panels[0]

        tab_ext = get_extents(tab)
        content_ext = get_extents(content)
        if tab_ext is None or content_ext is None:
            self.skipTest("Could not read extents")

        _, tab_y, _, tab_h = tab_ext
        _, content_y, _, _ = content_ext

        self.assertGreater(
            content_y,
            tab_y + tab_h // 2,
            "Places content panel is not below the tab bar.\n"
            "The utility_sidebar_box must use Vertical orientation.\n"
            f"Tab y={tab_y}+h={tab_h}, Content y={content_y}",
        )

    def test_terminal_grows_when_sidebar_hidden(self) -> None:
        """Hiding the tools sidebar must increase the terminal's allocated width."""
        terminals = wait_for_role(
            self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1
        )
        if not terminals:
            self.skipTest("No terminal found")

        ext_before = get_extents(terminals[0])
        if ext_before is None:
            self.skipTest("Cannot read terminal extents")
        w_before = ext_before[2]

        click(self._tools_toggle_button())
        time.sleep(0.6)

        # Re-query — the Accessible ref may be stale after layout change.
        terminals = wait_for_role(
            self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1
        )
        ext_after = get_extents(terminals[0])
        if ext_after is None:
            self.skipTest("Cannot read terminal extents after toggle")
        w_after = ext_after[2]

        self.assertGreater(
            w_after,
            w_before,
            "Terminal width did not increase after hiding the tools sidebar.\n"
            f"Before: {w_before}px, After: {w_after}px",
        )


if __name__ == "__main__":
    unittest.main()
