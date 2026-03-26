# rttxd

Persistent session daemon for the [rttx](https://github.com/IllyaYalovyy/rttx) tiling terminal emulator.

`rttx-server` decouples session lifetime from GUI lifetime. Sessions survive GUI crashes,
sleep/wake cycles, network drops, and local reboots.

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

The daemon owns all PTYs and terminal state. The GUI is a thin client that attaches/detaches
freely. One daemon per host, serving multiple sessions and clients.

### Crates

- **`rttx-proto`** — shared protobuf wire protocol (message types, length-prefixed framing, UUID helpers)
- **`rttx-server`** — the daemon binary (PTY management, session lifecycle, IPC, serialization)

## Building

```bash
cargo build
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

## Dev mode

Set `RTTX_DEV_MODE=1` to run a development daemon alongside a stable production instance.
Dev mode uses completely separate paths:

| | Production | Development |
|---|---|---|
| Socket | `$XDG_RUNTIME_DIR/rttx-server/v1/` | `$XDG_RUNTIME_DIR/rttxd-devel/v1/` |
| State | `$XDG_CACHE_HOME/rttx-server/` | `$XDG_CACHE_HOME/rttxd-devel/` |
| Log level | info | debug |

```bash
# Run dev daemon in foreground with debug logging
RTTX_DEV_MODE=1 cargo run -- start --foreground

# Override log level
RTTX_DEV_MODE=1 RUST_LOG=trace cargo run -- start --foreground
```

The rttx GUI in dev mode (`RTTX_DEV_MODE=1`) automatically connects to the dev daemon and
propagates the env var when auto-starting it.

## Testing

```bash
cargo test                          # All tests (56 total)
cargo test -p rttx-proto            # Protocol framing tests
cargo test -p rttx-server --lib     # Unit tests
cargo test -p rttx-server --tests   # Integration tests
```

Integration tests cover:
- PTY spawn, I/O, resize, exit status (`pty_basic`, `pty_io`)
- Session create/attach/detach/pane CRUD (`session_lifecycle`)
- Client reconnect after disconnect (`reconnect`)
- State serialization and session reconstruction after restart (`serialization`, `reconstruction`)
- Scrollback persistence to disk (`scrollback`)
- SSH stdio transport protocol (`stdio_transport`)

## Persistence model

State is written to disk continuously, not just on shutdown:

- **`state.json`** (every 1 second, atomic write) — session metadata, pane CWD/title/dimensions
- **`scrollback/<session>/<pane>.log`** (every 1 second, append-only) — raw terminal bytes

On daemon restart: metadata is loaded, scrollback logs are replayed into pane screens, fresh
shells are spawned in saved working directories. Clients attaching after restart receive a
snapshot containing the replayed scrollback plus live output from the new shell.

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
