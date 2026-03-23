# Install rttx

## Fedora

The easiest path on Fedora — installs rttx and all dependencies from a COPR repository:

```bash
sudo dnf copr enable illya/rttx
sudo dnf install rttx
```

Launch from the app grid or run `rttx` in a terminal.

To remove:

```bash
sudo dnf remove rttx
```

## Flatpak

Flatpak works on any Linux distribution. The bundle includes everything rttx needs — no host
library requirements.

### Install

```bash
# Add Flathub if you haven't already
flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo

# Install the GNOME 49 runtime (required, ~800 MB one-time download)
flatpak install --user flathub org.gnome.Platform//49

# Install rttx from a local bundle
flatpak install --user ./rttx.flatpak
```

Launch from the app grid, or:

```bash
flatpak run io.github.IllyaYalovyy.rttx
```

If rttx doesn't appear in the app grid, log out and back in.

### Host shell access

By default, the Flatpak runs shells inside the sandbox. Most users will want host shell access so
that rttx behaves like a normal terminal with access to your tools, SSH config, and files:

```bash
# Required — enables host shell access
flatpak override --user io.github.IllyaYalovyy.rttx \
  --talk-name=org.freedesktop.Flatpak

# Recommended — access to your home directory
flatpak override --user io.github.IllyaYalovyy.rttx \
  --filesystem=home

# Optional — if you use SSH
flatpak override --user io.github.IllyaYalovyy.rttx \
  --socket=ssh-auth

# Optional — if your SSH keys are managed by GPG agent
flatpak override --user io.github.IllyaYalovyy.rttx \
  --socket=gpg-agent
```

### Remove

```bash
flatpak uninstall io.github.IllyaYalovyy.rttx
```

## Build from source

### Install dependencies

**Fedora:**

```bash
sudo dnf install cargo meson pkg-config gtk4-devel libadwaita-devel vte291-gtk4-devel
```

**Ubuntu / Debian:**

```bash
sudo apt install cargo meson pkg-config libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev
```

**Arch Linux:**

```bash
sudo pacman -S rust meson pkgconf gtk4 libadwaita vte4
```

Minimum versions: GTK4 4.14, libadwaita 1.5, VTE 0.78 (GTK4 variant). Rust edition 2024 (Rust
1.85+).

### Quick build (no install)

```bash
cargo build --release
./target/release/rttx
```

### Full install (desktop integration)

This installs the binary, desktop file, icons, and AppStream metadata so rttx appears in the app
grid:

```bash
meson setup build --prefix="$HOME/.local"
meson install -C build
```

Refresh the icon and app caches:

```bash
gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor"
update-desktop-database "$HOME/.local/share/applications"
```

For a system-wide install:

```bash
meson setup build --prefix=/usr/local
sudo meson install -C build
```

If the GNOME app grid still shows a generic icon after a user-local install, log out and back in.
