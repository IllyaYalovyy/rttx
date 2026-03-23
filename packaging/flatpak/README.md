# Flatpak Packaging

This directory documents the current Flatpak strategy for `rttx`.

The repository keeps a **safe default manifest** at
[`io.github.IllyaYalovyy.rttx.json`](../../io.github.IllyaYalovyy.rttx.json).
It is intentionally conservative:

- current GNOME runtime
- no host-command permission by default
- no SSH agent socket by default
- no broad filesystem access by default

That is the right base package for a Flathub-first distribution story. Users who want deeper host
integration can opt in explicitly after install.

## Profiles

### Safe default

The default manifest is designed for broad compatibility and low review friction.

Expected behavior:

- `rttx` runs with current GNOME runtime libraries
- GTK desktop integration goes through the normal Flatpak / portal path
- shells run inside the Flatpak runtime environment
- SSH, tmux, and shell tooling come from the sandbox unless native mode is enabled later

This is the profile to ship first.

### Native mode

Native mode is an explicit user opt-in. It is intended for users who want the terminal to behave
like a true host terminal rather than a sandbox shell.

Expected behavior after the corresponding app-side support lands:

- terminal shells launch on the host via `flatpak-spawn --host`
- SSH and tmux use host binaries and host config
- host directory layout and toolchain become the primary execution context

Native mode is not a different Flatpak package. It is a documented permission and runtime mode.

## Prerequisites

Required tooling for local Flatpak work:

- `flatpak`
- `flatpak-builder`
- GNOME runtime and SDK matching the manifest branch
- Rust SDK extension for the chosen runtime

On Fedora:

```bash
sudo dnf install flatpak flatpak-builder
flatpak install flathub org.gnome.Platform//49 org.gnome.Sdk//49 org.freedesktop.Sdk.Extension.rust-stable//24.08
```

On Ubuntu:

```bash
sudo apt install flatpak flatpak-builder
flatpak install flathub org.gnome.Platform//49 org.gnome.Sdk//49 org.freedesktop.Sdk.Extension.rust-stable//24.08
```

The exact Rust extension branch may change independently of the GNOME runtime branch. Confirm the
currently available extension before scripting CI around it.

## Local build

From the repository root:

```bash
flatpak-builder --user --install --force-clean flatpak-build io.github.IllyaYalovyy.rttx.json
flatpak run io.github.IllyaYalovyy.rttx
```

The manifest bundles VTE 0.78.7 as a source module (the GNOME 49 SDK does not ship
`vte-2.91-gtk4`), so the build is self-contained.

To export a standalone `.flatpak` bundle for testing or distribution:

```bash
./packaging/flatpak/build-bundle.sh
# produces rttx.flatpak in the repository root
```

## Dependency handling

### Native libraries

`org.gnome.Sdk//49` provides `gtk4` and `libadwaita-1` but **not** `vte-2.91-gtk4`. The manifest
bundles VTE 0.78.7 as a source module (official GNOME tarball, GTK4-only Meson config).

### Rust crates (offline)

[`packaging/flatpak/cargo-sources.json`](cargo-sources.json) lists every Rust dependency for
offline builds inside the Flatpak sandbox (Cargo cannot reach `crates.io`).

Regenerate whenever `Cargo.lock` changes:

```bash
flatpak-cargo-generator Cargo.lock -o packaging/flatpak/cargo-sources.json
```

If you use the Python script from `flatpak-builder-tools` directly, the command name may differ.
Commit the regenerated file alongside the lockfile change.

The generated file is build metadata — do not hand-edit it.

## Native mode setup

The default Flatpak should stay conservative. Users who want host-shell behavior can opt in after
install.

### Step 1: allow host command launch

```bash
flatpak override --user io.github.IllyaYalovyy.rttx \
  --talk-name=org.freedesktop.Flatpak
```

This is the core permission needed for `flatpak-spawn --host`.

### Step 2: enable SSH agent access if needed

Only add this if your workflow requires direct access to the SSH agent socket:

```bash
flatpak override --user io.github.IllyaYalovyy.rttx \
  --socket=ssh-auth
```

### Step 3: enable GPG agent access if needed

Some SSH setups are mediated through GPG agent instead of a plain SSH agent socket:

```bash
flatpak override --user io.github.IllyaYalovyy.rttx \
  --socket=gpg-agent
```

### Step 4: add broader filesystem access only if necessary

Do this only for a specific, understood workflow:

```bash
flatpak override --user io.github.IllyaYalovyy.rttx \
  --filesystem=home
```

Avoid recommending `--filesystem=host` casually. It should stay a last resort.

## How to verify which mode you are using

Once native-mode support exists in the app, the quickest checks will be:

```bash
echo "$SHELL"
pwd
command -v ssh
command -v tmux
```

Practical expectations:

- in safe mode, you are checking the sandbox environment
- in native mode, you are checking the host environment

The app should also expose the current execution mode in Preferences or About:

- `Sandbox shell`
- `Host shell`

## Troubleshooting

### Themes do not match the host

This is usually a Flatpak theme-extension or portal-backend issue, not an `rttx` bug. Prefer
Adwaita-compatible setups first.

### SSH works on the host but not in Flatpak

Check whether you actually need native mode, `ssh-auth`, or `gpg-agent`. Do not widen permissions
blindly.

### A workflow only works with very broad filesystem access

Document the exact reason before keeping the override. The default package should remain narrow
unless a concrete limitation proves otherwise.

## Older distro compatibility

Flatpak decouples rttx from host library versions. The GNOME 49 runtime provides GTK 4.20 and
libadwaita 1.8 regardless of what the host distro ships. VTE is bundled in the manifest. As long as
the host can run Flatpak and install the GNOME 49 runtime, rttx will work — this covers Fedora 39+,
Ubuntu 22.04+, Debian 12+, Arch, and similar.

Users on very old distros (e.g., Ubuntu 20.04) may not be able to install a recent enough Flatpak
or the GNOME 49 runtime. This is an acceptable trade-off — those distros also lack the Wayland
stack that rttx targets.

## Current status

What exists now:

- conservative root manifest with bundled VTE 0.78.7 and offline Cargo sources
- `build-bundle.sh` script for local `.flatpak` bundle export
- RFC-011 describing the product and permission model
- this setup guide with safe-default and native-mode instructions

What still needs implementation:

- app-side Flatpak detection and dual shell-launch policy (RFC-011 Phase 2–3)
- CI build validation for the Flatpak manifest (#103)
- first-run UX that teaches users how to opt into native mode
- Flathub submission (requires `--talk-name=org.freedesktop.Flatpak` justification for native mode)
