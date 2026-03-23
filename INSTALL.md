# Install rttx

## Fedora

```bash
sudo dnf copr enable illya/rttx
sudo dnf install rttx
```

## Flatpak

If you have `rttx.flatpak`:

```bash
flatpak install --user ./rttx.flatpak
```

Run it:

```bash
flatpak run io.github.IllyaYalovyy.rttx
```

## Flatpak Host Integration

Default Flatpak install is sandboxed.

Enable host shell access:

```bash
flatpak override --user io.github.IllyaYalovyy.rttx --talk-name=org.freedesktop.Flatpak
```

Optional access:

SSH agent:

```bash
flatpak override --user io.github.IllyaYalovyy.rttx --socket=ssh-auth
```

GPG agent:

```bash
flatpak override --user io.github.IllyaYalovyy.rttx --socket=gpg-agent
```

Home directory:

```bash
flatpak override --user io.github.IllyaYalovyy.rttx --filesystem=home
```

## Build From Source

```bash
cargo build --release
./target/release/rttx
```

## Install From Source

User install:

```bash
meson setup build --prefix="$HOME/.local"
meson install -C build
gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor"
update-desktop-database "$HOME/.local/share/applications"
```

System install:

```bash
meson setup build --prefix=/usr/local
sudo meson install -C build
```

## Remove

Fedora package:

```bash
sudo dnf remove rttx
```

Flatpak:

```bash
flatpak uninstall io.github.IllyaYalovyy.rttx
```
