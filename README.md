# rttx Monorepo

`rttx` is a tiling terminal environment for GNOME. This repository contains the GUI client, the
daemon runtime service, and the shared wire protocol in one workspace.

![rttx screenshot](clients/rttx/data/screenshots/rttx-main.png)

## Repository Layout

- `clients/rttx/` — the GTK4 + Libadwaita client package (`rttx`)
- `services/rttx-server/` — the daemon runtime service package (`rttx-server`)
- `protocols/rttx-proto/` — the shared protobuf wire protocol package (`rttx-proto`)
- `packaging/rttx/` — Flatpak, RPM, and other client packaging assets
- `designs/` — architecture RFCs and design notes

Package names stay unchanged even though the repository now uses role-based directories.

## Build

Run all commands from the repository root.

Development build for the whole workspace:

```bash
cargo build --workspace
```

Development builds for individual packages:

```bash
cargo build -p rttx
cargo build -p rttx-server
cargo build -p rttx-proto
```

Run the client in normal mode:

```bash
cargo run -p rttx
```

Run the client in development mode:

```bash
RTTX_DEV_MODE=1 cargo run -p rttx
```

Run the daemon in foreground for development:

```bash
cargo run -p rttx-server -- start --foreground
```

Run the daemon in development mode:

```bash
RTTX_DEV_MODE=1 cargo run -p rttx-server -- start --foreground
```

## Install

### Client Only

Use Meson when you want a desktop-integrated client install with the binary, desktop file, icons,
and AppStream metadata.

Production install of the client from source:

```bash
meson setup build --prefix=/usr/local
meson compile -C build
sudo meson install -C build
sudo gtk-update-icon-cache -f -t /usr/local/share/icons/hicolor
sudo update-desktop-database /usr/local/share/applications
```

If `build/` already exists, refresh it instead of creating it again:

```bash
meson setup --reconfigure build --prefix=/usr/local
meson compile -C build
```

### Daemon Only

Use Cargo for the daemon. Meson does not install `rttx-server`.

Production install of the daemon from source:

```bash
cargo build --release -p rttx-server
sudo install -Dm755 target/release/rttx-server /usr/local/bin/rttx-server
```

### Full Production Install

Install both client and daemon from source:

```bash
meson setup build --prefix=/usr/local
meson compile -C build
sudo meson install -C build
cargo build --release -p rttx-server
sudo install -Dm755 target/release/rttx-server /usr/local/bin/rttx-server
```

`rttx-proto` is a shared library crate and is not installed as a standalone artifact.

## Test

```bash
bash .github/scripts/run-clippy.sh
bash .github/scripts/run-quality-tests.sh
cargo build -p rttx && ./run_ui_tests.sh
```

`cargo test --workspace` is useful for fast daemon/protocol passes, but the GTK-heavy client suite
must be driven through `.github/scripts/run-quality-tests.sh` because GTK widgets require
main-thread-aware isolation.

## Documentation

- [Client overview](clients/rttx/README.md)
- [Daemon overview](services/rttx-server/README.md)
- [Protocol overview](protocols/rttx-proto/README.md)
- [Contributing guide](CONTRIBUTING.md)

## License

GPL-3.0-or-later
