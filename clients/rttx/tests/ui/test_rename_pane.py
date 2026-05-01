"""UI test: rename pane via context menu action.

Covers #819 — the rename-pane action must set a custom title on the
focused pane, and clearing it must revert to the auto-derived title.
"""

import time
import unittest

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

from common import (
    AppFixture,
    find_all_by_role,
    is_showing,
    wait_for_showing_role,
)


def pane_header_labels(app: Atspi.Accessible) -> list[str]:
    """Return the accessible names of visible LABEL nodes inside terminal pane headers."""
    labels = []
    for node in find_all_by_role(app, Atspi.Role.LABEL):
        try:
            if is_showing(node) and node.get_name():
                labels.append(node.get_name())
        except Exception:  # noqa: BLE001
            pass
    return labels


def wait_for_label_text(
    app: Atspi.Accessible, text: str, timeout: float = 5.0
) -> bool:
    """Poll until a visible LABEL with the given text appears."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if text in pane_header_labels(app):
            return True
        time.sleep(0.2)
    return text in pane_header_labels(app)


class TestRenamePaneAction(unittest.TestCase):

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_showing_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1, timeout=10.0)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_rename_pane_sets_custom_title(self) -> None:
        """Activating rename-pane with a name must update the pane header label."""
        self.fixture.activate_action("rename-pane", "My Custom Pane")
        found = wait_for_label_text(self.fixture.atspi_app, "My Custom Pane", timeout=5.0)
        self.assertTrue(
            found,
            f"Pane header should show 'My Custom Pane', got labels: {pane_header_labels(self.fixture.atspi_app)}",
        )

    def test_rename_pane_empty_clears_custom_title(self) -> None:
        """Activating rename-pane with empty string must clear the custom title."""
        self.fixture.activate_action("rename-pane", "Temporary Name")
        found = wait_for_label_text(self.fixture.atspi_app, "Temporary Name", timeout=5.0)
        self.assertTrue(found, "Custom title should be set first")

        self.fixture.activate_action("rename-pane", "")
        time.sleep(1.0)
        labels = pane_header_labels(self.fixture.atspi_app)
        self.assertNotIn(
            "Temporary Name", labels,
            "Custom title should be cleared after empty rename",
        )

    def test_renamed_pane_persists_across_restart(self) -> None:
        """A renamed pane title must survive a save/restore cycle."""
        self.fixture.activate_action("rename-pane", "Persistent Pane Title")
        found = wait_for_label_text(self.fixture.atspi_app, "Persistent Pane Title", timeout=5.0)
        self.assertTrue(found, "Rename should take effect before restart")

        self.fixture.activate_action("save-state")
        time.sleep(0.5)

        self.fixture.restart_app()
        wait_for_showing_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1, timeout=10.0)

        found = wait_for_label_text(self.fixture.atspi_app, "Persistent Pane Title", timeout=5.0)
        self.assertTrue(
            found,
            f"Renamed pane title should persist across restart, got labels: {pane_header_labels(self.fixture.atspi_app)}",
        )


if __name__ == "__main__":
    unittest.main()
