#!/usr/bin/env bash
# Build and install only the rttx-server daemon binary.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="${script_dir}"

prefix="${RTTX_PREFIX:-$HOME/.local}"
bindir="${prefix}/bin"

if ! command -v cargo >/dev/null 2>&1; then
    printf 'error: cargo not found\n' >&2
    exit 1
fi

printf 'Building rttx-server...\n'
cargo build --manifest-path "$repo_root/Cargo.toml" --release -p rttx-server

mkdir -p "$bindir"
install -Dm755 "$repo_root/target/release/rttx-server" "$bindir/rttx-server"

printf 'Installed: %s\n' "$bindir/rttx-server"
"$bindir/rttx-server" --version
