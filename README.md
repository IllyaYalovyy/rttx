# rttx-server

Persistent session daemon for the [rttx](https://github.com/IllyaYalovyy/rttx) tiling terminal emulator.

`rttx-server` decouples session lifetime from GUI lifetime. Sessions survive GUI crashes, sleep/wake cycles, network drops, and local reboots.

## Architecture

```
rttx (GTK GUI)  ←── Unix socket ──→  rttx-server (daemon)
                                          │
                               ┌──────────┼──────────┐
                            Session A  Session B  Session C
                               │
                         ┌─────┼─────┐
                      Pane 1  Pane 2  Pane 3
                         │
                      PTY + screen state + scrollback log
```

## Building

```bash
cargo build
```

## Running

```bash
# Start in foreground (for development)
cargo run -- start --foreground

# Start as daemon
cargo run -- start

# Stop
cargo run -- stop
```

## License

GPL-3.0-or-later
