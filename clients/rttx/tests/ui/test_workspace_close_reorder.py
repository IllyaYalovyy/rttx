"""UI test: workspace close and reorder via sidebar interactions.

Regression coverage for #315 — closing a workspace must remove its sidebar
row and switch focus to the next workspace; creating multiple workspaces
must produce distinct sidebar rows with correct names.
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
    wait_for_role,
)


def get_sidebar_rows(app: Atspi.Accessible) -> list[Atspi.Accessible]:
    """Return the top-level LIST_ITEM nodes that are direct children of the Workspaces list."""
    return [
        row
        for row in find_all_by_role(app, Atspi.Role.LIST_ITEM)
        if _is_direct_child_of_workspaces(row) and is_showing(row)
    ]


def _is_direct_child_of_workspaces(node: Atspi.Accessible) -> bool:
    try:
        parent = node.get_parent()
        return (
            parent is not None
            and parent.get_role() == Atspi.Role.LIST
            and parent.get_name() == "Workspaces"
        )
    except Exception:  # noqa: BLE001
        return False


def sidebar_row_names(app: Atspi.Accessible) -> list[str]:
    """Return the accessible names of all visible sidebar workspace rows."""
    names = []
    for row in get_sidebar_rows(app):
        try:
            # The ListBoxRow itself has empty name; the ActionRow child has the title.
            for i in range(row.get_child_count()):
                child = row.get_child_at_index(i)
                if child is not None:
                    name = child.get_name()
                    if name:
                        names.append(name)
                        break
        except Exception:  # noqa: BLE001
            pass
    return names


def wait_for_sidebar_count(
    app: Atspi.Accessible, count: int, timeout: float = 10.0
) -> list[Atspi.Accessible]:
    """Poll until exactly *count* visible sidebar rows exist."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        rows = get_sidebar_rows(app)
        if len(rows) == count:
            return rows
        time.sleep(0.3)
    return get_sidebar_rows(app)


class TestWorkspaceClose(unittest.TestCase):

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_sidebar_count(self.fixture.atspi_app, 1, timeout=10.0)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_close_workspace_removes_sidebar_row(self) -> None:
        """Closing one of two workspaces must remove its sidebar row."""
        rows_before = get_sidebar_rows(self.fixture.atspi_app)
        self.assertEqual(len(rows_before), 1, "Expected 1 sidebar row on launch")

        self.fixture.activate_action("create-direct")
        rows_after_create = wait_for_sidebar_count(self.fixture.atspi_app, 2, timeout=10.0)
        self.assertEqual(
            len(rows_after_create), 2,
            "Expected 2 sidebar rows after creating a second workspace",
        )

        self.fixture.activate_action("close-current-workspace")
        rows_after_close = wait_for_sidebar_count(self.fixture.atspi_app, 1, timeout=10.0)
        self.assertEqual(
            len(rows_after_close), 1,
            "Closing a workspace should leave exactly 1 sidebar row",
        )

    def test_close_last_workspace_exits_app(self) -> None:
        """Closing the only workspace must close the window (app exits)."""
        rows = get_sidebar_rows(self.fixture.atspi_app)
        self.assertEqual(len(rows), 1, "Expected 1 sidebar row on launch")

        self.fixture.activate_action("close-current-workspace")

        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            if self.fixture._app is not None and self.fixture._app.poll() is not None:
                break
            time.sleep(0.3)

        self.assertIsNotNone(
            self.fixture._app.poll(),
            "App process should have exited after closing the last workspace",
        )

    def test_close_workspace_switches_to_remaining(self) -> None:
        """After closing the active workspace, the remaining one must be visible."""
        self.fixture.activate_action("create-direct")
        wait_for_sidebar_count(self.fixture.atspi_app, 2, timeout=10.0)

        names_before = sidebar_row_names(self.fixture.atspi_app)
        self.assertEqual(len(names_before), 2)

        self.fixture.activate_action("close-current-workspace")
        rows = wait_for_sidebar_count(self.fixture.atspi_app, 1, timeout=10.0)
        self.assertEqual(len(rows), 1)

        remaining_names = sidebar_row_names(self.fixture.atspi_app)
        self.assertEqual(len(remaining_names), 1)
        self.assertEqual(
            remaining_names[0], names_before[0],
            "The first workspace should remain after closing the second",
        )


class TestWorkspaceReorder(unittest.TestCase):

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_sidebar_count(self.fixture.atspi_app, 1, timeout=10.0)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_multiple_workspaces_have_distinct_names(self) -> None:
        """Creating additional workspaces must produce distinct sidebar row names."""
        self.fixture.activate_action("create-direct")
        time.sleep(0.8)
        self.fixture.activate_action("create-direct")
        time.sleep(0.8)

        rows = wait_for_sidebar_count(self.fixture.atspi_app, 3, timeout=10.0)
        self.assertEqual(
            len(rows), 3,
            "Expected 3 sidebar rows after creating two additional workspaces",
        )

        names = sidebar_row_names(self.fixture.atspi_app)
        self.assertEqual(len(names), 3, f"Expected 3 workspace names, got {names}")
        self.assertEqual(
            len(set(names)), 3,
            f"All workspace names must be distinct, got {names}",
        )

    def test_close_middle_workspace_preserves_others(self) -> None:
        """Closing a workspace between two others must preserve both neighbors."""
        self.fixture.activate_action("create-direct")
        time.sleep(0.8)
        self.fixture.activate_action("create-direct")
        time.sleep(0.8)

        wait_for_sidebar_count(self.fixture.atspi_app, 3, timeout=10.0)
        names_before = sidebar_row_names(self.fixture.atspi_app)
        self.assertEqual(len(names_before), 3)

        # The last created workspace (Direct 3) is active; close it.
        self.fixture.activate_action("close-current-workspace")
        rows = wait_for_sidebar_count(self.fixture.atspi_app, 2, timeout=10.0)
        self.assertEqual(len(rows), 2)

        names_after = sidebar_row_names(self.fixture.atspi_app)
        self.assertEqual(len(names_after), 2)
        self.assertEqual(
            names_after, names_before[:2],
            "The first two workspaces should remain after closing the third",
        )


if __name__ == "__main__":
    unittest.main()
