# rttx

[![Sponsor](https://img.shields.io/badge/Sponsor-%E2%9D%A4-ea4aaa?logo=githubsponsors)](https://github.com/sponsors/IllyaYalovyy)

A tiling terminal environment for GNOME, built with Rust, GTK4, and Libadwaita, organized around
named workspaces and split panes.

Spiritual successor to [Tilix](https://github.com/gnunn1/tilix), rewritten from scratch for the
modern GNOME desktop.

![rttx showing named workspaces, a three-pane split layout, and the Places panel](clients/rttx/data/screenshots/rttx-workspaces.png)

## Features

### Workspaces and layouts

- Create named workspaces in the left sidebar for separate work contexts
- Split a workspace into panes horizontally or vertically, up to 5 levels deep
- Drag pane headers to rearrange a workspace layout
- Broadcast keystrokes to all panes in a workspace (input sync)

### Places and commands

- Save launch targets (local folders and SSH hosts) as Places, and open them in the current pane or a new workspace
- Save and search reusable commands in the right sidebar
- Clone commands, add descriptions and labels for organization
- Use env-var parameters for dynamic command values
- Run commands in the current pane or a new split pane
- Copy raw command text to clipboard
- Assign per-command keyboard shortcuts via leader prefix (Ctrl+;)

![The Commands panel with saved, searchable commands organized by label](clients/rttx/data/screenshots/rttx-commands.png)

### Recovery and reconnect

- Workspace layouts, split sizes, and working directories persist automatically
- Periodic auto-save ensures no state loss on unexpected exit
- Per-pane shell history, scrollback, and the visible screen are preserved and restored on reconnect
- Daemon-backed workspaces reconnect explicitly instead of silently degrading

### Terminal

- Ctrl+click to open URLs, OSC 8 hyperlinks, and detected file paths
- Right-click for context menu; Shift+right-click passes mouse events to terminal apps
- Optional smart clipboard: plain Ctrl+C copies selected text, Ctrl+V pastes
- Built-in Nightfall and Daybreak themes, with Tilix color scheme compatibility
- Background process notifications via toast (foreground) or desktop notification (background)

### Configuration

- Export, import, and reset configuration from Preferences
- Customizable keyboard shortcuts via Preferences

## Install

### Fedora (COPR)

```bash
sudo dnf copr enable etf2026/rttx
sudo dnf install rttx
```

Launch from the app grid or run `rttx` in a terminal. To remove: `sudo dnf remove rttx`.

### Debian / Ubuntu (DEB)

Download the `.deb` package from the
[latest GitHub release](https://github.com/IllyaYalovyy/rttx/releases/latest):

```bash
sudo apt install ./rttx_<version>_amd64.deb
```

The package includes both the GUI (`rttx`) and the daemon (`rttx-server`). Launch from the app
grid or run `rttx` in a terminal. To remove: `sudo apt remove rttx`.

### Flatpak

Flatpak works on any Linux distribution. The bundle includes everything rttx needs.

```bash
# Add Flathub if you haven't already
flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo

# Install the GNOME 49 runtime (required, ~800 MB one-time download)
flatpak install --user flathub org.gnome.Platform//49

# Install rttx from a local bundle
flatpak install --user ./rttx.flatpak
```

Launch from the app grid, or: `flatpak run io.github.IllyaYalovyy.rttx`

#### Host shell access

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

To remove: `flatpak uninstall io.github.IllyaYalovyy.rttx`

### Build from source

Install dependencies:

**Fedora:**
```bash
sudo dnf install cargo meson pkg-config gtk4-devel libadwaita-devel vte291-gtk4-devel protobuf-compiler
```

**Ubuntu / Debian:**
```bash
sudo apt install cargo meson pkg-config libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev protobuf-compiler
```

**Arch Linux:**
```bash
sudo pacman -S rust meson pkgconf gtk4 libadwaita vte4 protobuf
```

Minimum versions: GTK4 4.14, libadwaita 1.5, VTE 0.76+ (0.78 recommended), Rust 1.85+ (edition
2024).

Quick build (no install):

```bash
cargo build --release
./target/release/rttx
```

If your system has VTE 0.76 instead of 0.78 (e.g., Fedora 40, Ubuntu 24.04):

```bash
cargo build --release --no-default-features --features vte-0_76
```

Full install with desktop integration (client + daemon):

```bash
meson setup build --prefix=/usr/local
meson compile -C build
sudo meson install -C build
cargo build --release -p rttx-server
sudo install -Dm755 target/release/rttx-server /usr/local/bin/rttx-server
sudo gtk-update-icon-cache -f -t /usr/local/share/icons/hicolor
sudo update-desktop-database /usr/local/share/applications
```

For a user-local install (no sudo), use `--prefix="$HOME/.local"` and ensure `~/.local/bin` is in
your `$PATH`.

To build and install both the GUI and `rttx-server` for your user in one step:

```bash
./install-user-local.sh
```

For VTE 0.76 systems, add `-Dvte_version=0.76` to `meson setup`.

The daemon must be on `$PATH` for the client to auto-start it. Without it, daemon-backed
workspaces will fail to connect.

## Keyboard Shortcuts

| Action | Shortcut |
|---|---|
| New workspace | Ctrl+Shift+T |
| Close pane | Ctrl+Shift+W |
| Split horizontal | Ctrl+Shift+E |
| Split vertical | Ctrl+Shift+O |
| Toggle workspace sidebar | Ctrl+Shift+N |
| Toggle tools sidebar | Ctrl+Shift+B |
| Copy / Paste | Ctrl+Shift+C / Ctrl+Shift+V |
| Search | Ctrl+Shift+F |
| Input sync toggle | Ctrl+Shift+I |
| Next / previous workspace | Ctrl+Tab / Ctrl+Shift+Tab |
| Jump to workspace 1-9 | Alt+1 through Alt+9 |
| Zoom in / out / reset | Ctrl+Plus / Ctrl+Minus / Ctrl+0 |
| Zoom pane (toggle) | Ctrl+Shift+Z |
| Rotate layout | Ctrl+Shift+R |
| Repair terminal | Ctrl+Shift+X |
| Navigate panes | Alt+Arrow (configurable) |
| Next / previous pane | Alt+] / Alt+[ (configurable) |
| Connect to existing workspace | Ctrl+Shift+A |
| New direct terminal | Ctrl+Shift+D |
| Commands leader key | Ctrl+; |
| Preferences | Ctrl+, |
| Fullscreen | F11 |

Pane navigation preserves zoom: with a pane maximized (Ctrl+Shift+Z), Alt+Arrow
and Alt+] / Alt+[ move the zoom to the target pane instead of unzooming, and the
pane header shows an `n/total` counter. Alt+Arrow is spatial and stops at the
edge of the layout; Alt+] / Alt+[ cycle through every pane in layout order and
wrap around, so they always reach the next pane even when the layout is hidden
behind a zoomed pane.

## Architecture

```
Local machine                          Remote host
--------------                         -----------
rttx (GTK GUI)                         rttx-server (daemon)
    |                                      |
    |--- Unix socket ----> rttx-server     |--- PTYs (bash, zsh, ...)
    |    (local daemon)        |           |
    |                          |           |--- runtimes/<id>/workspace.json
    |--- SSH tunnel -------->(protocol)    |--- runtimes/<id>/scrollback/*.log
```

`rttx-server` decouples runtime lifetime from GUI lifetime. rttx workspaces attach and detach
freely while runtimes continue according to their policy and endpoint availability.

The daemon owns all PTYs, runtime state, and the workspace pane tree. The GUI is a view that
renders the daemon's tree — each rendered pane is keyed directly by its durable server pane id,
with no separate binding table. One daemon per host serves multiple runtimes and clients.

### Runtime model

A workspace selects a runtime policy:

- **Ephemeral** — disposable, but still daemon-backed
- **Persistent** — survives detach, reconnect, and daemon reconstruction after restart

There is no implicit fallback to a separate execution model when the daemon is unavailable. Instead
the GUI shows explicit connection state and retries transient failures automatically.

A single rttx window may contain multiple workspaces targeting different endpoints and policies.
Multiple windows may connect to the same endpoint.

### Server-authoritative model

The daemon is the single source of truth for a workspace's pane tree and identity:

- On attach, the client adopts the daemon's pane tree wholesale and renders it; it mints no pane identity of its own.
- Splits and closes are requests the daemon applies to its tree and then broadcasts to every attached client.
- Inventory recovery: if a runtime exists without client-side metadata, rttx recovers a workspace for it. Missing client state never implicitly deletes a daemon runtime or pane.

### Persistence model

The client and daemon persist state in separate XDG-appropriate locations:

**Client** (GUI) — uses versioned JSON documents with schema envelopes (RFC-023):

| Location | Contents |
|---|---|
| `$XDG_CONFIG_HOME/rttx/` | `preferences.json`, `hosts.json`, `library.json`, `schemes/` |
| `$XDG_STATE_HOME/rttx/client/` | `workspaces.json`, `ui.json`, `migrations.json`, `backups/` |
| `$XDG_CACHE_HOME/rttx/` | `runtime-cache.json` |

Config holds durable user choices, state holds restorable application state, and cache holds
disposable runtime data that can be deleted without data loss.

**Daemon** (`rttx-server`) — writes state continuously, not just on shutdown:

- **`runtimes/<id>/workspace.json`** (dirty-flag driven) — the pane tree and per-pane metadata (CWD, title, dimensions)
- **`runtimes/<id>/scrollback/<pane>.log`** (append-only, rotated) — raw terminal bytes
- **`runtimes/<id>/screen/<pane>.snap`** — deterministic screen snapshot for reconnect
- **`runtimes/<id>/history/<pane>.hist`** — durable per-pane shell history

Daemon state lives under `$XDG_STATE_HOME/rttx/daemon/` (default `~/.local/state/rttx/daemon/`),
not in the cache directory, so it survives cache cleanup.

On daemon restart: metadata is loaded, screen snapshots restore the visible viewport, and fresh
shells are spawned in saved working directories. Clients attaching after restart receive a snapshot
containing the restored screen plus live output from the new shell.

### Wire protocol

Protobuf over 4-byte little-endian length-prefixed frames. Same protocol for Unix socket and SSH
stdio transports.

Key messages: `ClientHello`/`ServerHello` (handshake with capability negotiation),
`CreateWorkspace`/`AttachWorkspace`/`DetachWorkspace`/`TerminateWorkspace`,
`CreatePane`/`SplitPane`/`ClosePane`, `TerminalInput`/`ResizePane`/`SetFocus`,
`WorkspaceSnapshot`/`OutputDelta`/`PaneSplit`/`PaneClosed`/`FocusChanged`, `GetDiagnostics`.

The protocol version is negotiated during the handshake; a client that does not speak the current version is rejected.

### Remote access via SSH

The GUI connects to a remote daemon by running:

```
ssh <host> rttx-server attach-stdio
```

This speaks the same protocol over the SSH subprocess's stdin/stdout. No port forwarding needed —
just `rttx-server` in `$PATH` on the remote host.

## Repository Layout

```
clients/rttx/          — GTK4 + Libadwaita client (rttx)
services/rttx-server/  — daemon runtime service (rttx-server)
protocols/rttx-proto/  — shared protobuf wire protocol (rttx-proto)
packaging/rttx/        — Flatpak, RPM, and other packaging assets
designs/               — architecture RFCs and design notes
```

Package names stay unchanged even though the repository uses role-based directories.

`rttx-proto` is a shared library crate and is not installed as a standalone artifact.

## Development

### Prerequisites

See [Build from source](#build-from-source) above for the full dependency list per distro.

Check your VTE version — this is the most common build issue:

```bash
pkg-config --modversion vte-2.91-gtk4
```

### Build

Development build for the whole workspace:

```bash
cargo build --workspace
```

Individual packages:

```bash
cargo build -p rttx
cargo build -p rttx-server
cargo build -p rttx-proto
```

If your system has VTE 0.76 instead of 0.78:

```bash
cargo build -p rttx --no-default-features --features vte-0_76
```

The default feature is `vte-0_78`. If you skip the flag on a VTE 0.76 system, the build will fail
with `vte-2.91-gtk4 >= 0.78 not found`.

Run the client:

```bash
cargo run -p rttx
```

Run the daemon in foreground:

```bash
cargo run -p rttx-server -- start --foreground
```

### Dev mode

Set `RTTX_DEV_MODE=1` to run a development instance alongside a stable production instance. Dev
mode uses completely separate paths:

| | Production | Development |
|---|---|---|
| App ID | `io.github.IllyaYalovyy.rttx` | `io.github.IllyaYalovyy.rttx.Devel` |
| Socket | `$XDG_RUNTIME_DIR/rttx-server/v1/` | `$XDG_RUNTIME_DIR/rttx-server-devel/v1/` |
| Config | `$XDG_CONFIG_HOME/rttx/` | `$XDG_CONFIG_HOME/rttx-devel/` |
| Client state | `$XDG_STATE_HOME/rttx/client/` | `$XDG_STATE_HOME/rttx-devel/client/` |
| Daemon state | `$XDG_STATE_HOME/rttx/daemon/` | `$XDG_STATE_HOME/rttx-devel/daemon/` |
| Cache | `$XDG_CACHE_HOME/rttx/` | `$XDG_CACHE_HOME/rttx-devel/` |
| Log level | info | debug |

```bash
RTTX_DEV_MODE=1 cargo run -p rttx
RTTX_DEV_MODE=1 cargo run -p rttx-server -- start --foreground
```

The rttx GUI in dev mode automatically connects to the dev daemon and propagates the env var when
auto-starting it.

### Testing

```bash
bash .github/scripts/run-clippy.sh
bash .github/scripts/run-quality-tests.sh
cargo build -p rttx && ./run_ui_tests.sh
```

`cargo test --workspace` is useful for fast daemon/protocol passes, but the GTK-heavy client suite
must be driven through `.github/scripts/run-quality-tests.sh` because GTK widgets require
main-thread-aware isolation.

Server integration tests cover PTY I/O, runtime lifecycle, client reconnection, state
serialization, scrollback persistence, and SSH stdio transport:

```bash
cargo test -p rttx-proto
cargo test -p rttx-server --lib
cargo test -p rttx-server --tests
```

### Logging

Both the GUI and daemon write logs to files with daily rotation:

| Component | Log directory | Default level |
|---|---|---|
| GUI (`rttx`) | `$XDG_CACHE_HOME/rttx/` | `rttx=info,warn` |
| Daemon (`rttx-server`) | `$XDG_CACHE_HOME/rttx-server/` | `info` |

Old log files are cleaned up automatically (3 days retained).

View the daemon log directory:

```bash
rttx-server logs
```

Override the log level with `RUST_LOG`:

```bash
RUST_LOG=rttx=debug rttx                          # GUI with debug logging
RUST_LOG=debug rttx-server start --foreground      # daemon with debug logging
```

Dev mode uses separate directories (`rttx-devel/`, `rttx-server-devel/`) so development logs do
not mix with production logs. See `designs/RFC-017-logging.md` for design details.

### Meson install details

Meson installs the client binary, desktop file, icons, and AppStream metadata. It does not install
`rttx-server` — use Cargo for the daemon (see [Build from source](#build-from-source)).

If `build/` already exists, use `--reconfigure` to change options:

```bash
meson setup --reconfigure build --prefix="$HOME/.local" -Dvte_version=0.76
meson compile -C build
```

If the build is in a broken state, wipe and start fresh:

```bash
meson setup --wipe build --prefix="$HOME/.local"
meson compile -C build
```

### Resetting to factory state

To completely wipe all rttx data and simulate a first launch, stop the daemon and remove all
configuration, state, and cache directories.

**Production instance:**

```bash
# Stop the daemon (terminates all running sessions)
rttx-server stop 2>/dev/null; pkill -f "rttx-server" 2>/dev/null

# Remove configuration (preferences, hosts, saved commands, color schemes)
rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/rttx/"

# Remove client state (workspace layouts, UI state, backups)
rm -rf "${XDG_STATE_HOME:-$HOME/.local/state}/rttx/client/"

# Remove daemon state (runtimes, scrollback, screen snapshots)
rm -rf "${XDG_STATE_HOME:-$HOME/.local/state}/rttx/daemon/"

# Remove cache and logs
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/rttx/"
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/rttx-server/"

# Remove runtime files (socket, PID file)
rm -rf "${XDG_RUNTIME_DIR}/rttx-server/"
```

**Development instance** (`RTTX_DEV_MODE=1`):

```bash
pkill -f "rttx-server" 2>/dev/null
rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/rttx-devel/"
rm -rf "${XDG_STATE_HOME:-$HOME/.local/state}/rttx-devel/"
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/rttx-devel/"
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/rttx-server-devel/"
rm -rf "${XDG_RUNTIME_DIR}/rttx-server-devel/"
```

After removing these directories, the next launch of `rttx` will behave as a fresh install with
default preferences and no saved workspaces.

To reset only specific parts:
- **Preferences only:** remove `${XDG_CONFIG_HOME:-$HOME/.config}/rttx/preferences.json`
- **Workspace layouts only:** remove `${XDG_STATE_HOME:-$HOME/.local/state}/rttx/client/workspaces.json`
- **Saved commands only:** remove `${XDG_CONFIG_HOME:-$HOME/.config}/rttx/library.json`
- **Color schemes only:** remove `${XDG_CONFIG_HOME:-$HOME/.config}/rttx/schemes/`

## Flatpak Packaging

The repository keeps a conservative default manifest at
`packaging/rttx/io.github.IllyaYalovyy.rttx.json` — no host-command permission, no SSH agent
socket, no broad filesystem access by default. Users who want deeper host integration opt in
explicitly after install (see [Host shell access](#host-shell-access)).

### Local Flatpak build

Prerequisites (Fedora):

```bash
sudo dnf install flatpak flatpak-builder
flatpak install flathub org.gnome.Platform//49 org.gnome.Sdk//49 \
  org.freedesktop.Sdk.Extension.rust-stable//24.08
```

Build and run:

```bash
flatpak-builder --user --install --force-clean flatpak-build \
  packaging/rttx/io.github.IllyaYalovyy.rttx.json
flatpak run io.github.IllyaYalovyy.rttx
```

Export a standalone `.flatpak` bundle:

```bash
./packaging/rttx/flatpak/build-bundle.sh
```

### Flatpak dependency handling

The GNOME 49 SDK provides GTK4 and libadwaita but not `vte-2.91-gtk4`. The manifest bundles VTE
0.78.7 as a source module.

`packaging/rttx/flatpak/cargo-sources.json` lists every Rust dependency for offline builds inside
the Flatpak sandbox. Regenerate whenever `Cargo.lock` changes:

```bash
flatpak-cargo-generator Cargo.lock -o packaging/rttx/flatpak/cargo-sources.json
```

Commit the regenerated file alongside the lockfile change.

## Terminology

- **Window** — one rttx application window
- **Workspace** — the top-level GUI object listed in the left sidebar; contains panes, a layout, and presentation state
- **Runtime** — the live backend object owned by `rttx-server` for a daemon-backed workspace; owns PTYs, scrollback, CWD, titles, and process lifetime independently from the GUI
- **Pane** — one terminal pane inside a workspace
- **Layout** — the arrangement of panes and split ratios inside a workspace
- **Endpoint** — the local daemon or one remote host daemon that serves runtimes
- **Policy** — the runtime retention model: `ephemeral` or `persistent`; both are daemon-backed
- **Place** — a saved launch target such as a folder or SSH host, openable in a pane or a new workspace
- **Command** — a saved command snippet you can run or insert into a pane

A few connection-status names still use `Session` (e.g. `SessionMissing`). In product docs and
UI, `Workspace` and `Runtime` are the preferred terms.

## Design Documents

Architecture RFCs and design notes live in `designs/`. RFC-001 (manifesto), RFC-013 (persistent host
sessions), and RFC-031 (server-authoritative workspace tree) define the current product direction
and architecture. RFC-014 (cloud relay service) is the forward-looking design for
internet-accessible endpoints.

When reading older RFCs, note that some architecture assumptions have been superseded:

- RFC-007 (session recovery) and RFC-018 (connection state machine) — the client-owned layout and
  client↔daemon reconciliation they describe are superseded by RFC-031, which makes the daemon
  authoritative for the pane tree; per-pane history/scrollback/screen restore is delegated to the daemon
- RFC-010 (maintainability refactor) — fully implemented; `window.rs` is now a module directory
  with 8 submodules, `session/layout.rs` is split into layout, recovery, and state modules
- RFC-011 (Flatpak) — packaging and host integration model remains relevant, but daemon-related
  assumptions are superseded by RFC-013
- RFC-012 (CI/CD) — describes the live GitHub Actions workflows, not a future plan

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, code standards, testing, and the pull
request process.

## Author

Illya Yalovyy — [GitHub](https://github.com/IllyaYalovyy) · [LinkedIn](https://www.linkedin.com/in/illyayalovyy/)

## License

GPL-3.0-or-later
