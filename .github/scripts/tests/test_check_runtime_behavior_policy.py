import pathlib
import sys
import unittest


SCRIPT_DIR = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

from check_runtime_behavior_policy import evaluate_policy


class RuntimeBehaviorPolicyTests(unittest.TestCase):
    def test_non_runtime_changes_do_not_require_extra_coverage(self) -> None:
        result = evaluate_policy(
            changed_files=["clients/rttx/src/bookmarks.rs"],
            added_lines_by_file={},
        )

        self.assertFalse(result.runtime_affecting)
        self.assertTrue(result.allowed)

    def test_runtime_change_without_test_evidence_fails_both_requirements(self) -> None:
        result = evaluate_policy(
            changed_files=["clients/rttx/src/window.rs"],
            added_lines_by_file={"clients/rttx/src/window.rs": ["fn load_state(&self) {"]},
        )

        self.assertTrue(result.runtime_affecting)
        self.assertFalse(result.allowed)
        self.assertFalse(result.has_pure_state_evidence)
        self.assertFalse(result.has_behavior_evidence)

    def test_runtime_change_with_only_pure_state_test_evidence_still_fails(self) -> None:
        result = evaluate_policy(
            changed_files=[
                "clients/rttx/src/window.rs",
                "clients/rttx/src/workspace_state.rs",
            ],
            added_lines_by_file={
                "clients/rttx/src/workspace_state.rs": ["#[test]", "fn reducer_handles_restart() {"],
            },
        )

        self.assertTrue(result.has_pure_state_evidence)
        self.assertFalse(result.has_behavior_evidence)
        self.assertFalse(result.allowed)

    def test_runtime_change_with_only_behavior_test_evidence_still_fails(self) -> None:
        result = evaluate_policy(
            changed_files=[
                "clients/rttx/src/window.rs",
                "clients/rttx/tests/ui/test_managed_blackbox.py",
            ],
            added_lines_by_file={
                "clients/rttx/tests/ui/test_managed_blackbox.py": [
                    "def test_daemon_restart_restores_workspace(self) -> None:"
                ],
            },
        )

        self.assertFalse(result.has_pure_state_evidence)
        self.assertTrue(result.has_behavior_evidence)
        self.assertFalse(result.allowed)

    def test_runtime_change_with_pure_state_and_black_box_evidence_passes(self) -> None:
        result = evaluate_policy(
            changed_files=[
                "clients/rttx/src/window.rs",
                "clients/rttx/src/workspace_state.rs",
                "clients/rttx/tests/ui/test_managed_blackbox.py",
            ],
            added_lines_by_file={
                "clients/rttx/src/workspace_state.rs": ["#[test]", "fn reducer_handles_restart() {"],
                "clients/rttx/tests/ui/test_managed_blackbox.py": [
                    "def test_daemon_restart_restores_workspace(self) -> None:"
                ],
            },
        )

        self.assertTrue(result.allowed)
        self.assertTrue(result.has_pure_state_evidence)
        self.assertTrue(result.has_behavior_evidence)

    def test_server_runtime_changes_can_use_server_integration_tests(self) -> None:
        result = evaluate_policy(
            changed_files=[
                "services/rttx-server/src/session.rs",
                "services/rttx-server/src/screen.rs",
                "services/rttx-server/tests/reconnect.rs",
            ],
            added_lines_by_file={
                "services/rttx-server/src/session.rs": ["#[test]", "fn session_reconnects_cleanly() {"],
                "services/rttx-server/tests/reconnect.rs": ["#[test]", "fn reconnect_after_restart() {"],
            },
        )

        self.assertTrue(result.allowed)


if __name__ == "__main__":
    unittest.main()
