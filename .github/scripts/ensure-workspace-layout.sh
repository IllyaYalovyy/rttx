#!/usr/bin/env bash
set -euo pipefail

readonly required_manifests=(
    "Cargo.toml"
    "clients/rttx/Cargo.toml"
    "services/rttx-server/Cargo.toml"
    "protocols/rttx-proto/Cargo.toml"
)

for manifest in "${required_manifests[@]}"; do
    if [[ ! -f "${manifest}" ]]; then
        cat >&2 <<EOF
Missing required workspace manifest: ${manifest}

The consolidated rttx repository must contain:
- clients/rttx
- services/rttx-server
- protocols/rttx-proto
EOF
        exit 1
    fi
done

cargo metadata --format-version 1 --no-deps >/dev/null
