#!/usr/bin/env bash
# Fail when packaging/rttx/flatpak/cargo-sources.json is out of date with
# Cargo.lock. The Flatpak build runs offline from that list, so a crate added
# to the lockfile without regenerating the list breaks the release build of
# the bundle — after the tag is pushed.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git clone -q --depth 1 https://github.com/flatpak/flatpak-builder-tools.git "$work/tools"
python3 -m venv "$work/venv"
"$work/venv/bin/pip" install -q aiohttp tomlkit
"$work/venv/bin/python" "$work/tools/cargo/flatpak-cargo-generator.py" Cargo.lock -o "$work/cargo-sources.json"

if ! diff -q "$work/cargo-sources.json" packaging/rttx/flatpak/cargo-sources.json >/dev/null; then
    echo "packaging/rttx/flatpak/cargo-sources.json is out of date with Cargo.lock." >&2
    echo "Regenerate it with:" >&2
    echo "  flatpak-cargo-generator Cargo.lock -o packaging/rttx/flatpak/cargo-sources.json" >&2
    diff "$work/cargo-sources.json" packaging/rttx/flatpak/cargo-sources.json | head -40 >&2 || true
    exit 1
fi
echo "cargo-sources.json matches Cargo.lock"
