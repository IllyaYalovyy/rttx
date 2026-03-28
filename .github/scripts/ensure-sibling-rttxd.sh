#!/usr/bin/env bash
set -euo pipefail

readonly proto_manifest="../rttxd/crates/rttx-proto/Cargo.toml"

if [[ ! -f "${proto_manifest}" ]]; then
    cat >&2 <<'EOF'
Missing sibling rttxd checkout at ../rttxd.

rttx depends on ../rttxd/crates/rttx-proto, so cargo-based quality jobs must
run with rttx and rttxd checked out as sibling directories.

CI should checkout the pinned ref from .github/rttxd-ref.
Local development should place rttx and rttxd next to each other.
EOF
    exit 1
fi

# Resolve the workspace early so missing or broken sibling checkouts fail with
# a precise preflight error instead of a later cargo command failure.
cargo metadata --format-version 1 --no-deps >/dev/null
