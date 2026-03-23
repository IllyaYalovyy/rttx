#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

rm -rf b repo rttx.flatpak
flatpak-builder --force-clean --disable-rofiles-fuse --repo=repo b io.github.IllyaYalovyy.rttx.json
flatpak build-bundle repo rttx.flatpak io.github.IllyaYalovyy.rttx
