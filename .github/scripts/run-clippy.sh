#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
bash "${script_dir}/ensure-workspace-layout.sh"

VTE_VERSION=$(pkg-config --modversion vte-2.91-gtk4 2>/dev/null || echo "0.78")
if printf '%s\n' "0.78" "${VTE_VERSION}" | sort -V | head -n1 | grep -q "^0\.78"; then
    client_features=()
else
    client_features=(--no-default-features --features vte-0_76)
fi

cargo clippy --manifest-path clients/rttx/Cargo.toml "${client_features[@]}" --all-targets -- -D warnings
cargo clippy --manifest-path services/rttx-server/Cargo.toml --all-targets -- -D warnings
cargo clippy --manifest-path protocols/rttx-proto/Cargo.toml --all-targets -- -D warnings
