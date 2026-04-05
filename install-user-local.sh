#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="${script_dir}"

prefix="${RTTX_PREFIX:-$HOME/.local}"
bindir="${prefix}/bin"
build_dir="${RTTX_BUILD_DIR:-$repo_root/build-user-local}"

have_command() {
    command -v "$1" >/dev/null 2>&1
}

require_command() {
    if ! have_command "$1"; then
        printf 'error: required command not found: %s\n' "$1" >&2
        exit 1
    fi
}

select_vte_version() {
    if [[ -n "${RTTX_VTE_VERSION:-}" ]]; then
        printf '%s\n' "$RTTX_VTE_VERSION"
        return
    fi

    require_command pkg-config

    if ! pkg-config --exists vte-2.91-gtk4; then
        printf 'error: pkg-config could not find vte-2.91-gtk4\n' >&2
        exit 1
    fi

    local detected_version
    detected_version="$(pkg-config --modversion vte-2.91-gtk4)"

    if [[ "$(printf '%s\n%s\n' "$detected_version" "0.78" | sort -V | tail -n1)" == "$detected_version" ]]; then
        printf '0.78\n'
    else
        printf '0.76\n'
    fi
}

require_command cargo
require_command meson
require_command install

vte_version="$(select_vte_version)"

printf 'Installing rttx into %s\n' "$prefix"
printf 'Using Meson build directory %s\n' "$build_dir"
printf 'Using VTE compatibility mode %s\n' "$vte_version"

mkdir -p "$bindir"
cd "$repo_root"

if [[ -d "$build_dir" ]]; then
    meson setup --reconfigure "$build_dir" --prefix="$prefix" "-Dvte_version=$vte_version"
else
    meson setup "$build_dir" --prefix="$prefix" "-Dvte_version=$vte_version"
fi

meson compile -C "$build_dir"
meson install -C "$build_dir" --no-rebuild

cargo build --manifest-path "$repo_root/Cargo.toml" --release -p rttx-server
install -Dm755 "$repo_root/target/release/rttx-server" "$bindir/rttx-server"

if have_command gtk-update-icon-cache && [[ -d "$prefix/share/icons/hicolor" ]]; then
    gtk-update-icon-cache -f -t "$prefix/share/icons/hicolor"
fi

if have_command update-desktop-database && [[ -d "$prefix/share/applications" ]]; then
    update-desktop-database "$prefix/share/applications"
fi

printf '\nInstalled binaries:\n'
printf '  %s\n' "$bindir/rttx"
printf '  %s\n' "$bindir/rttx-server"

if [[ ":$PATH:" != *":$bindir:"* ]]; then
    printf '\nwarning: %s is not in PATH\n' "$bindir" >&2
    printf 'add this to your shell profile:\n  export PATH="%s:$PATH"\n' "$bindir" >&2
fi
