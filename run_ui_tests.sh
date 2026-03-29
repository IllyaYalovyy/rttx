#!/usr/bin/env bash
# Run the AT-SPI2 behavioral UI test suite.
#
# The test harness launches rttx with RTTX_DEV_MODE=1, which registers it as
# io.github.IllyaYalovyy.rttx.Devel — a completely separate D-Bus name and
# config directory from the production instance. Running these tests is safe
# while rttx is open for normal work.
#
# Prerequisites:
#   - cargo build  (produces target/debug/rttx)
#   - python3 with gi.repository.Atspi (python3-atspi / typelib-1_0-Atspi-2_0)
#   - weston (headless Wayland compositor — dnf install weston)
#
# Usage:
#   ./run_ui_tests.sh              # run all UI tests
#   ./run_ui_tests.sh test_split   # run only tests matching pattern

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UI_TEST_DIR="$SCRIPT_DIR/clients/rttx/tests/ui"

# Verify the binary exists before starting AT-SPI infrastructure.
BINARY="$SCRIPT_DIR/target/debug/rttx"
if [[ ! -f "$BINARY" ]]; then
    echo "ERROR: $BINARY not found. Run 'cargo build' first." >&2
    exit 1
fi

# Verify python3 can load AT-SPI bindings.
if ! python3 -c "import gi; gi.require_version('Atspi','2.0'); from gi.repository import Atspi" 2>/dev/null; then
    echo "ERROR: python3-atspi not available. Install gi.repository.Atspi bindings." >&2
    exit 1
fi

# Verify weston is available.
if ! command -v weston &>/dev/null; then
    echo "ERROR: weston not found. Install with: dnf install weston" >&2
    exit 1
fi

PATTERN="${1:-}"

echo "=== rttx AT-SPI2 UI tests ==="
cd "$UI_TEST_DIR"
if [[ -n "$PATTERN" ]]; then
    python3 -m unittest discover -s . -p "test_*.py" -k "$PATTERN" -v
else
    python3 -m unittest discover -s . -p "test_*.py" -v
fi
