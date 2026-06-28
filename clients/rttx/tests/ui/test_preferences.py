"""UI test: the preferences dialog opens and exposes its settings (#322).

Covers the dialog-interaction half of #322 — activating the preferences
action must open a dialog whose appearance settings are visible in the
AT-SPI tree. Preference *propagation* to live terminals is covered by the
GTK widget tests in `window/tests.rs`.
"""

import unittest

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

from common import AppFixture, wait_for_showing_role


class TestPreferencesDialog(unittest.TestCase):

    def setUp(self) -> None:
        self.fixture = AppFixture()
        self.fixture.start()
        wait_for_showing_role(
            self.fixture.atspi_app, Atspi.Role.TERMINAL, count=1, timeout=10.0
        )

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_preferences_dialog_opens_with_appearance_settings(self) -> None:
        """Activating the preferences action opens a dialog exposing the Font row."""
        self.fixture.activate_action("preferences")
        font_row = self.fixture.wait_for_showing_name(
            Atspi.Role.LABEL, "Font", timeout=20.0
        )
        self.assertIsNotNone(
            font_row, "preferences dialog should expose the 'Font' appearance setting"
        )
