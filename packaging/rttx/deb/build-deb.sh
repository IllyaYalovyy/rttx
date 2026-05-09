#!/usr/bin/env bash
set -euo pipefail

# Build a .deb package for rttx (client + daemon).
# Requires: cargo-deb (`cargo install cargo-deb`)

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "${repo_root}"

echo "Building release binaries..."
cargo build --release -p rttx -p rttx-server

echo "Stripping binaries..."
strip target/release/rttx target/release/rttx-server

echo "Building DEB package..."
cargo deb -p rttx --no-build

deb_file=$(ls -1 target/debian/*.deb 2>/dev/null | head -1)
if [[ -n "${deb_file}" ]]; then
    echo "DEB package built: ${deb_file}"
    dpkg-deb --info "${deb_file}"
else
    echo "ERROR: No .deb file found in target/debian/" >&2
    exit 1
fi
