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

Build the whole workspace:

```bash
cargo build --workspace
```

Build individual artifacts:

```bash
cargo build -p rttx
cargo build -p rttx-server
cargo build -p rttx-proto
```

Run the client:

```bash
cargo run -p rttx
```

Run the daemon:

```bash
cargo run -p rttx-server -- start --foreground
```

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
- [Contributing guide](CONTRIBUTING.md)

## License

GPL-3.0-or-later
