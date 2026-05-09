#!/usr/bin/env bash
set -euo pipefail

# Dependency security audit using cargo-deny.
# Checks: known vulnerabilities, license compliance, banned crates, source origins.

if ! command -v cargo-deny &>/dev/null; then
    echo "Installing cargo-deny..."
    cargo install cargo-deny --locked
fi

echo "=== Running cargo deny check ==="
cargo deny check
echo ""
echo "=== Dependency audit passed ==="
