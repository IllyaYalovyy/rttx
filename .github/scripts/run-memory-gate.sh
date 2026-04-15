#!/usr/bin/env bash
# Memory profiling CI gate for rttx-server.
#
# Runs a scripted lifecycle scenario and asserts that:
# 1. No sessions or panes leak after the scenario completes.
# 2. The diagnostics report shows zero residual state.
#
# This script is designed to run in CI but can also be used locally.
# It does NOT require valgrind or heaptrack — it uses the built-in
# diagnostics protocol to verify cleanup at the application level.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "${script_dir}/../.." && pwd)

echo "Building rttx-server..."
cargo build -p rttx-server --manifest-path "${repo_root}/services/rttx-server/Cargo.toml"

echo "Running memory cleanup integration tests..."
cargo test -p rttx-server --test memory_cleanup -- --nocapture

echo "Running diagnostics integration tests..."
cargo test -p rttx-server --test diagnostics -- --nocapture

echo "Running lifecycle leak tests..."
cargo test -p rttx-server --test lifecycle_leaks -- --nocapture

echo "Running bounded channel tests..."
cargo test -p rttx-server --test bounded_channels -- --nocapture

echo ""
echo "Memory profiling gate: PASSED"
