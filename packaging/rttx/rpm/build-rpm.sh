#!/usr/bin/env bash
set -euo pipefail

# Build an .rpm package for rttx (client + daemon).
# Requires: cargo-generate-rpm (`cargo install cargo-generate-rpm`)

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "${repo_root}"

echo "Building release binaries..."
cargo build --release -p rttx -p rttx-server

echo "Stripping binaries..."
strip target/release/rttx target/release/rttx-server

echo "Building RPM package..."
cargo generate-rpm -p clients/rttx

rpm_file=$(find target/generate-rpm -name '*.rpm' 2>/dev/null | head -1)
if [[ -n "${rpm_file}" ]]; then
    echo "RPM package built: ${rpm_file}"
    rpm -qip "${rpm_file}" 2>/dev/null || true
else
    echo "ERROR: No .rpm file found in target/generate-rpm/" >&2
    exit 1
fi
