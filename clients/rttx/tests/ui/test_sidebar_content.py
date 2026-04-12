"""UI test: sidebar row content compliance per RFC-015.

Verifies that sidebar rows never contain garbage text (VTE prompt titles,
generic shell names, runtime metadata) and that every row has a connection
icon. These tests catch the class of regressions from PRs #332–#360.
"""

import re
import unittest

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

from common import AppFixture, click, find_all_by_role, wait_for_role

# Patterns that must never appear in a sidebar subtitle (RFC-015 exclusion list).
FORBIDDEN_SUBTITLE_PATTERNS = [
    re.compile(r"@.*:"),  # user@host:path prompt titles
    re.compile(r"^bash$", re.IGNORECASE),
    re.compile(r"^zsh$", re.IGNORECASE),
    re.compile(r"^sh$", re.IGNORECASE),
    re.compile(r"^fish$", re.IGNORECASE),
    re.compile(r"^nu$", re.IGNORECASE),
    re.compile(r"Terminal", re.IGNORECASE),
    re.compile(r"persistent", re.IGNORECASE),
    re.compile(r"ephemeral", re.IGNORECASE),
    re.compile(r"^Session\b", re.IGNORECASE),
    re.compile(r"^Workspace\b", re.IGNORECASE),
]


def get_sidebar_rows(app: Atspi.Accessible) -> list[Atspi.Accessible]:
    """Return all LIST_ITEM nodes (sidebar workspace rows)."""
    return [
        row
        for row in find_all_by_role(app, Atspi.Role.LIST_ITEM)
        if has_ancestor_named(row, Atspi.Role.LIST, "Workspaces")
    ]


def has_ancestor_named(node: Atspi.Accessible, role: Atspi.Role, name: str) -> bool:
    """Return whether *node* belongs to the named accessible container."""
    try:
        parent = node.get_parent()
        while parent is not None:
            if parent.get_role() == role and parent.get_name() == name:
                return True
            parent = parent.get_parent()
    except Exception:  # noqa: BLE001
        return False
    return False


def get_row_subtitle(row: Atspi.Accessible) -> str:
    """Return the subtitle text of a sidebar row (exposed as accessible description)."""
    try:
        return row.get_description() or ""
    except Exception:  # noqa: BLE001
        return ""


def get_row_images(row: Atspi.Accessible) -> list[Atspi.Accessible]:
    """Return all IMAGE-role children of a sidebar row (connection icons)."""
    results = []
    try:
        for i in range(row.get_child_count()):
            child = row.get_child_at_index(i)
            if child is not None:
                results.extend(find_all_by_role(child, Atspi.Role.IMAGE))
    except Exception:  # noqa: BLE001
        pass
    return results


class TestSidebarContentDirect(unittest.TestCase):
    """Sidebar content compliance for direct (no-daemon) workspaces."""

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1)

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_direct_workspace_subtitle_no_forbidden_patterns(self) -> None:
        """Direct workspace subtitle must not contain VTE title garbage."""
        rows = get_sidebar_rows(self.fixture.atspi_app)
        self.assertGreaterEqual(len(rows), 1, "No sidebar rows found")

        for row in rows:
            subtitle = get_row_subtitle(row)
            name = row.get_name()
            for pattern in FORBIDDEN_SUBTITLE_PATTERNS:
                self.assertIsNone(
                    pattern.search(subtitle),
                    f"Row '{name}' subtitle '{subtitle}' matches forbidden "
                    f"pattern /{pattern.pattern}/",
                )

    def test_direct_workspace_subtitle_is_path_or_empty(self) -> None:
        """When present, the subtitle must look like a file path (~ or / prefix)."""
        rows = get_sidebar_rows(self.fixture.atspi_app)
        self.assertGreaterEqual(len(rows), 1, "No sidebar rows found")

        for row in rows:
            subtitle = get_row_subtitle(row)
            if subtitle:
                self.assertTrue(
                    subtitle.startswith("~") or subtitle.startswith("/"),
                    f"Row '{row.get_name()}' subtitle '{subtitle}' does not "
                    f"look like a path (expected ~ or / prefix)",
                )

    def test_direct_workspace_has_icon(self) -> None:
        """Every sidebar row must have at least one IMAGE child (connection icon)."""
        rows = get_sidebar_rows(self.fixture.atspi_app)
        self.assertGreaterEqual(len(rows), 1, "No sidebar rows found")

        for row in rows:
            images = get_row_images(row)
            self.assertGreaterEqual(
                len(images),
                1,
                f"Row '{row.get_name()}' has no IMAGE child (missing connection icon)",
            )


class TestSidebarContentManaged(unittest.TestCase):
    """Sidebar content compliance for managed (daemon-backed) workspaces."""

    def setUp(self) -> None:
        self.fixture = AppFixture(disable_shell_spawn=False)
        self.fixture.start_daemon()
        self.fixture.start()
        wait_for_role(self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1)

    def tearDown(self) -> None:
        self.fixture.stop()

    def _create_managed_workspace(self) -> None:
        button = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "New persistent workspace"
        )
        self.assertIsNotNone(button, "persistent workspace button not visible")
        click(button)

        # The New Workspace dialog opens — select "Home" to create the workspace.
        home_button = self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Home", timeout=10.0
        )
        self.assertIsNotNone(home_button, "Home place button not visible in dialog")
        click(home_button)

        self.fixture.wait_for_showing_name(
            Atspi.Role.PUSH_BUTTON, "Close pane", timeout=20.0
        )

    def test_managed_workspace_subtitle_no_forbidden_patterns(self) -> None:
        """Managed workspace subtitle must not contain VTE title garbage."""
        self._create_managed_workspace()

        rows = get_sidebar_rows(self.fixture.atspi_app)
        self.assertGreaterEqual(len(rows), 2, "Expected at least 2 rows (direct + managed)")

        for row in rows:
            subtitle = get_row_subtitle(row)
            name = row.get_name()
            for pattern in FORBIDDEN_SUBTITLE_PATTERNS:
                self.assertIsNone(
                    pattern.search(subtitle),
                    f"Row '{name}' subtitle '{subtitle}' matches forbidden "
                    f"pattern /{pattern.pattern}/",
                )

    def test_all_rows_have_icons_after_managed_creation(self) -> None:
        """Both direct and managed rows must have connection icons."""
        self._create_managed_workspace()

        rows = get_sidebar_rows(self.fixture.atspi_app)
        self.assertGreaterEqual(len(rows), 2, "Expected at least 2 rows")

        for row in rows:
            images = get_row_images(row)
            self.assertGreaterEqual(
                len(images),
                1,
                f"Row '{row.get_name()}' has no IMAGE child (missing connection icon)",
            )


if __name__ == "__main__":
    unittest.main()
