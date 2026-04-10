#!/usr/bin/env bash
# Verify that meson.build version matches the workspace Cargo.toml version.
set -euo pipefail

cargo_version=$(grep -A5 '^\[workspace\.package\]' Cargo.toml \
    | grep '^version' | head -1 | sed 's/.*"\(.*\)".*/\1/')
meson_version=$(grep "version:" meson.build | head -1 | sed "s/.*'\(.*\)'.*/\1/")

if [[ "${cargo_version}" != "${meson_version}" ]]; then
    echo >&2 "Version mismatch: Cargo.toml has ${cargo_version}, meson.build has ${meson_version}"
    echo >&2 "Update meson.build to match the workspace version in Cargo.toml."
    exit 1
fi

echo "Version consistent: ${cargo_version}"
