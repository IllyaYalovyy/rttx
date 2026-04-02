# rttx-server

Daemon runtime service for the [rttx](https://github.com/IllyaYalovyy/rttx) tiling terminal environment.

`rttx-server` decouples runtime lifetime from GUI lifetime. rttx workspaces attach and detach
freely while runtimes continue according to their policy and endpoint availability.

## Terminology

- **Workspace** — the top-level GUI object in rttx
- **Runtime** — the daemon-owned live backend object attached to one workspace
- **Pane** — one terminal tile inside a workspace/runtime
- **Endpoint** — the local daemon or one remote host daemon
- **Policy** — `ephemeral` or `persistent`; both are daemon-backed

Current Rust code in rttx still uses `Session*` names in some places. Product docs use
`Workspace` and `Runtime`.

## Architecture

```
Local machine                          Remote host
--------------                         -----------
rttx (GTK GUI)                         rttx-server (daemon)
    |                                      |
    |--- Unix socket ----> rttx-server     |--- PTYs (bash, zsh, ...)
    |    (local daemon)        |           |
    |                          |           |--- state.json (periodic)
    |--- SSH tunnel -------->(protocol)    |--- scrollback/*.log
```

The daemon owns all PTYs and runtime state. The GUI owns workspace layout and presentation.
Bindings map workspace panes to daemon pane ids. One daemon per host serves multiple runtimes and
clients.

rttx is converging on one managed execution model for both local and remote endpoints. A workspace
selects a runtime policy:

- **Ephemeral** — disposable, but still daemon-backed
- **Persistent** — survives detach, reconnect, and daemon reconstruction after restart

There is no implicit fallback to a separate direct-terminal model when the daemon is unavailable.
Instead the GUI shows explicit connection state and retries transient failures automatically.

### Crates

- **`rttx-proto`** — shared protobuf wire protocol (message types, length-prefixed framing, UUID helpers)
- **`rttx-server`** — the daemon binary (PTY management, runtime lifecycle, IPC, serialization)

## Building

Development build from the repository root:

```bash
cargo build -p rttx-server
```

Requires `protoc` (protobuf compiler) for code generation:

```bash
# Fedora
sudo dnf install protobuf-compiler

# Ubuntu/Debian
sudo apt install protobuf-compiler
```

## Running

```bash
# Start as background daemon
rttx-server start

# Start in foreground (for development)
rttx-server start --foreground

# Stop the daemon
rttx-server stop

# Serve one client over stdin/stdout (for SSH tunneling)
rttx-server attach-stdio
```

### Remote access via SSH

The GUI connects to a remote daemon by running:

```
ssh <host> rttx-server attach-stdio
```

This speaks the same protocol over the SSH subprocess's stdin/stdout. No port forwarding needed —
just `rttx-server` in `$PATH` on the remote host.

## Runtime model

- One workspace binds to one endpoint and one policy.
- A single rttx window may contain multiple workspaces that target different endpoints and
  policies.
- Multiple windows may connect to the same endpoint.
- A specific runtime has one writer by default; multi-attach should be explicit if supported.

## Reconciliation contract

The daemon owns runtimes, panes, PTYs, scrollback, CWD, runtime titles, and process lifetime.
rttx owns workspace layout, pane arrangement, selection, focus, and presentation state.

Reconciliation between the GUI and daemon must be non-destructive:

- If a runtime exists without GUI metadata, rttx should recover a workspace for it.
- If GUI metadata exists without a live runtime, rttx should keep a disconnected placeholder.
- Missing GUI state must never implicitly delete a daemon runtime or pane.

## Dev mode

Set `RTTX_DEV_MODE=1` to run a development daemon alongside a stable production instance.
Dev mode uses completely separate paths:

| | Production | Development |
|---|---|---|
| Socket | `$XDG_RUNTIME_DIR/rttx-server/v1/` | `$XDG_RUNTIME_DIR/rttx-server-devel/v1/` |
| State | `$XDG_CACHE_HOME/rttx-server/` | `$XDG_CACHE_HOME/rttx-server-devel/` |
| Log level | info | debug |

```bash
# Run dev daemon in foreground with debug logging
RTTX_DEV_MODE=1 cargo run -p rttx-server -- start --foreground

# Override log level
RTTX_DEV_MODE=1 RUST_LOG=trace cargo run -p rttx-server -- start --foreground
```

The rttx GUI in dev mode (`RTTX_DEV_MODE=1`) automatically connects to the dev daemon and
propagates the env var when auto-starting it.

## Install

The daemon must be on `$PATH` for the rttx client to auto-start it. Without it, daemon-backed
workspaces (both ephemeral and persistent) will fail to connect.

User-local install (no sudo needed):

```bash
cargo build --release -p rttx-server
install -Dm755 target/release/rttx-server "$HOME/.local/bin/rttx-server"
```

Make sure `~/.local/bin` is in your `$PATH`. Most distributions include it by default; if not:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
```

System-wide install:

```bash
cargo build --release -p rttx-server
sudo install -Dm755 target/release/rttx-server /usr/local/bin/rttx-server
```

Combined source install with the client:

```bash
# Run from the repository root. Meson installs the client only.
meson setup build --prefix=/usr/local
meson compile -C build
sudo meson install -C build
cargo build --release -p rttx-server
sudo install -Dm755 target/release/rttx-server /usr/local/bin/rttx-server
```

Verify the daemon is reachable:

```bash
rttx-server --help
```

## Testing

From the repository root:

```bash
cargo test -p rttx-proto
cargo test -p rttx-server --lib
cargo test -p rttx-server --tests
```

Integration tests cover:
- PTY spawn, I/O, resize, exit status (`pty_basic`, `pty_io`)
- Runtime create/attach/detach/pane CRUD (`session_lifecycle`)
- Client reconnect after disconnect (`reconnect`)
- State serialization and runtime reconstruction after restart (`serialization`, `reconstruction`)
- Scrollback persistence to disk (`scrollback`)
- SSH stdio transport protocol (`stdio_transport`)

## Persistence model

State is written to disk continuously, not just on shutdown:

- **`state.json`** (every 1 second, atomic write) — runtime metadata, pane CWD/title/dimensions
- **`scrollback/<session>/<pane>.log`** (every 1 second, append-only) — raw terminal bytes

On daemon restart: metadata is loaded, scrollback logs are replayed into pane screens, fresh
shells are spawned in saved working directories. Clients attaching after restart receive a
snapshot containing the replayed scrollback plus live output from the new shell.

Ephemeral and persistent runtimes share the same backend mechanics. The policy decides retention
semantics, not protocol or transport.

## Wire protocol

Protobuf over 4-byte little-endian length-prefixed frames. Same protocol for Unix socket and
SSH stdio transports.

Key messages: `Hello`/`HelloAck` (handshake), `CreateSession`/`AttachSession`/`DetachSession`,
`CreatePane`/`ClosePane`, `Input`/`Resize`, `Snapshot`/`Delta`/`PaneExited`.

Protocol version is checked during handshake. Incompatible versions disconnect cleanly.

## Code quality

- `unsafe_code = "deny"` — no unsafe code
- Clippy at pedantic + nursery level, all warnings as errors
- rustfmt enforced
- All public items have doc comments

## License

GPL-3.0-or-later
