#!/usr/bin/env bash
# Generate test coverage metrics for the rttx workspace.
#
# Covers rttx-server and rttx-proto (non-GTK packages that run reliably
# in headless CI). The GTK client is excluded because its tests require
# a display server and GTK global state isolation that conflict with
# coverage instrumentation.
#
# Requires: cargo-llvm-cov (installed automatically if missing)
#
# Outputs:
#   coverage/lcov.info   — LCOV report for downstream tools
#   coverage/summary.txt — human-readable summary
#   stdout               — GitHub Actions job summary (markdown table)
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "${script_dir}/../.." && pwd)
coverage_dir="${repo_root}/coverage"

mkdir -p "${coverage_dir}"

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
    echo "Installing cargo-llvm-cov…"
    cargo install cargo-llvm-cov --locked
fi

# Ensure the llvm-tools component is available.
rustup component add llvm-tools-preview 2>/dev/null || true

# Clean previous coverage data.
cargo llvm-cov clean --workspace

# Run tests with coverage for non-GTK packages.
# --lib and --tests cover unit + integration tests.
cargo llvm-cov \
    --manifest-path "${repo_root}/services/rttx-server/Cargo.toml" \
    --no-report \
    --lib --tests

cargo llvm-cov \
    --manifest-path "${repo_root}/protocols/rttx-proto/Cargo.toml" \
    --no-report \
    --lib --tests

# Generate LCOV report.
cargo llvm-cov report \
    --manifest-path "${repo_root}/Cargo.toml" \
    --lcov \
    --output-path "${coverage_dir}/lcov.info"

# Generate human-readable summary.
cargo llvm-cov report \
    --manifest-path "${repo_root}/Cargo.toml" \
    | tee "${coverage_dir}/summary.txt"

# Emit GitHub Actions job summary when running in CI.
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
        echo "## Test Coverage"
        echo ""
        echo "Packages measured: \`rttx-server\`, \`rttx-proto\`"
        echo ""
        echo '```'
        cat "${coverage_dir}/summary.txt"
        echo '```'
        echo ""
        echo "Full LCOV report uploaded as workflow artifact."
    } >> "${GITHUB_STEP_SUMMARY}"
fi
