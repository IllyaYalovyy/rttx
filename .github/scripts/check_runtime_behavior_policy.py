#!/usr/bin/env python3
"""Enforce behavior-proof coverage for runtime-affecting pull requests."""

from __future__ import annotations

import argparse
import dataclasses
import pathlib
import re
import subprocess
import sys
from collections import defaultdict
from typing import Iterable


RUNTIME_AFFECTING_PATTERNS = (
    "clients/rttx/src/window.rs",
    "clients/rttx/src/window/**/*.rs",
    "clients/rttx/src/workspace_state.rs",
    "clients/rttx/src/runtime.rs",
    "clients/rttx/src/daemon.rs",
    "clients/rttx/src/daemon_bridge.rs",
    "clients/rttx/src/session/layout.rs",
    "clients/rttx/src/terminal/mod.rs",
    "clients/rttx/src/terminal/widget.rs",
    "clients/rttx/src/terminal/persistent_widget.rs",
    "clients/rttx/src/terminal/handle.rs",
    "services/rttx-server/src/*.rs",
    "services/rttx-server/src/**/*.rs",
    "protocols/rttx-proto/src/*.rs",
    "protocols/rttx-proto/src/**/*.rs",
)

PURE_STATE_TEST_HOST_PATTERNS = (
    "clients/rttx/src/workspace_state.rs",
    "clients/rttx/src/runtime.rs",
    "clients/rttx/src/daemon.rs",
    "clients/rttx/src/daemon_bridge.rs",
    "clients/rttx/src/session/layout.rs",
    "clients/rttx/src/terminal/mod.rs",
    "services/rttx-server/src/*.rs",
    "services/rttx-server/src/**/*.rs",
    "protocols/rttx-proto/src/*.rs",
    "protocols/rttx-proto/src/**/*.rs",
)

BEHAVIOR_TEST_PATTERNS = (
    "clients/rttx/tests/*.rs",
    "clients/rttx/tests/**/*.rs",
    "clients/rttx/tests/ui/*.py",
    "clients/rttx/tests/ui/**/*.py",
    "services/rttx-server/tests/*.rs",
    "services/rttx-server/tests/**/*.rs",
)

RUST_TEST_DECLARATION = re.compile(r"^\s*(#\[(test|should_panic|tokio::test)\]|proptest!\s*)")
PYTHON_TEST_DECLARATION = re.compile(r"^\s*def test_[A-Za-z0-9_]*\s*\(")


@dataclasses.dataclass(frozen=True)
class PolicyResult:
    runtime_affecting: bool
    allowed: bool
    runtime_files: tuple[str, ...]
    pure_state_test_files: tuple[str, ...]
    behavior_test_files: tuple[str, ...]

    @property
    def has_pure_state_evidence(self) -> bool:
        return bool(self.pure_state_test_files)

    @property
    def has_behavior_evidence(self) -> bool:
        return bool(self.behavior_test_files)


def matches_any(path: str, patterns: Iterable[str]) -> bool:
    candidate = pathlib.PurePosixPath(path)
    return any(candidate.match(pattern) for pattern in patterns)


def has_test_declaration(path: str, added_lines: Iterable[str]) -> bool:
    matcher = PYTHON_TEST_DECLARATION if path.endswith(".py") else RUST_TEST_DECLARATION
    return any(matcher.match(line) for line in added_lines)


def evaluate_policy(
    *,
    changed_files: Iterable[str],
    added_lines_by_file: dict[str, list[str]],
) -> PolicyResult:
    changed_files = sorted(set(changed_files))
    runtime_files = tuple(
        path for path in changed_files if matches_any(path, RUNTIME_AFFECTING_PATTERNS)
    )
    if not runtime_files:
        return PolicyResult(
            runtime_affecting=False,
            allowed=True,
            runtime_files=(),
            pure_state_test_files=(),
            behavior_test_files=(),
        )

    pure_state_test_files = tuple(
        sorted(
            path
            for path in changed_files
            if matches_any(path, PURE_STATE_TEST_HOST_PATTERNS)
            and has_test_declaration(path, added_lines_by_file.get(path, ()))
        )
    )
    behavior_test_files = tuple(
        sorted(
            path
            for path in changed_files
            if matches_any(path, BEHAVIOR_TEST_PATTERNS)
            and has_test_declaration(path, added_lines_by_file.get(path, ()))
        )
    )

    return PolicyResult(
        runtime_affecting=True,
        allowed=bool(pure_state_test_files) and bool(behavior_test_files),
        runtime_files=runtime_files,
        pure_state_test_files=pure_state_test_files,
        behavior_test_files=behavior_test_files,
    )


def git_output(repo_root: pathlib.Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", *args],
        check=True,
        cwd=repo_root,
        capture_output=True,
        text=True,
    )
    return completed.stdout


def changed_files(repo_root: pathlib.Path, base: str, head: str) -> list[str]:
    output = git_output(repo_root, "diff", "--name-only", "--diff-filter=ACMR", f"{base}..{head}")
    return [line for line in output.splitlines() if line]


def added_lines_by_file(repo_root: pathlib.Path, base: str, head: str) -> dict[str, list[str]]:
    output = git_output(
        repo_root,
        "diff",
        "--unified=0",
        "--no-color",
        "--diff-filter=ACMR",
        f"{base}..{head}",
    )
    current_file: str | None = None
    added: dict[str, list[str]] = defaultdict(list)

    for raw_line in output.splitlines():
        if raw_line.startswith("+++ b/"):
            current_file = raw_line[6:]
            continue
        if raw_line.startswith("+++ "):
            current_file = None
            continue
        if current_file is None:
            continue
        if raw_line.startswith("+") and not raw_line.startswith("+++"):
            added[current_file].append(raw_line[1:])

    return dict(added)


def format_failure(result: PolicyResult) -> str:
    lines = [
        "Runtime-affecting changes were detected without the required proof layers.",
        "",
        "Changed runtime-affecting files:",
        *[f"- {path}" for path in result.runtime_files],
    ]

    if result.has_pure_state_evidence:
        lines.extend(
            [
                "",
                "Pure-state evidence detected:",
                *[f"- {path}" for path in result.pure_state_test_files],
            ]
        )
    else:
        lines.extend(
            [
                "",
                "Missing pure-state evidence.",
                "Add at least one new unit-style regression test in a pure-state test host such as:",
                "- clients/rttx/src/workspace_state.rs",
                "- clients/rttx/src/runtime.rs",
                "- clients/rttx/src/session/layout.rs",
                "- services/rttx-server/src/**",
                "- protocols/rttx-proto/src/**",
            ]
        )

    if result.has_behavior_evidence:
        lines.extend(
            [
                "",
                "Behavior-layer evidence detected:",
                *[f"- {path}" for path in result.behavior_test_files],
            ]
        )
    else:
        lines.extend(
            [
                "",
                "Missing integration or black-box evidence.",
                "Add at least one new regression test in:",
                "- clients/rttx/tests/*.rs",
                "- clients/rttx/tests/ui/*.py",
                "- services/rttx-server/tests/*.rs",
            ]
        )

    return "\n".join(lines)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", required=True, help="Base commit or ref for the PR diff")
    parser.add_argument("--head", required=True, help="Head commit or ref for the PR diff")
    parser.add_argument(
        "--repo-root",
        default=str(pathlib.Path(__file__).resolve().parents[2]),
        help="Repository root containing the git checkout",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo_root = pathlib.Path(args.repo_root).resolve()

    result = evaluate_policy(
        changed_files=changed_files(repo_root, args.base, args.head),
        added_lines_by_file=added_lines_by_file(repo_root, args.base, args.head),
    )
    if result.allowed:
        if result.runtime_affecting:
            print("Runtime-affecting changes detected with both required coverage layers.")
        else:
            print("No runtime-affecting changes detected; behavior-proof gate skipped.")
        return 0

    print(format_failure(result), file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
