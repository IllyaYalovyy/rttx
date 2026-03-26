# RFC-013: Persistent Sessions with rttxd

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Draft (v2)              |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | RFC-013 v1 (tmux-first) |
| Superseded by | ---                       |

---

## Summary

Add persistent sessions to `rttx` backed by `rttxd`, a standalone daemon that owns PTYs and
terminal state independently from the GUI. The same daemon runs on local and remote hosts. No tmux
dependency.

The core decisions:

- `rttxd` is the sole persistent session engine --- no TmuxEngine, no tmux dependency
- the same daemon binary and protocol serve both local (Unix socket) and remote (SSH tunnel) sessions
- persistence beyond host reboot is achieved through scrollback logging, state serialization, and
  session reconstruction --- not process checkpointing
- the GUI remains a thin client that attaches/detaches freely

---

## Requirements

### Disruption scenarios

| # | Scenario | Frequency | User expectation |
|---|---|---|---|
| D1 | rttx GUI closes or crashes | Common | Everything resumes exactly where I left off |
| D2 | Local machine sleeps and wakes | Daily | Nothing should change --- same as D1 |
| D3 | Network drops (SSH connection breaks) | Common | Remote work continues; I reconnect and it is all there |
| D4 | Local machine reboots | Weekly | I get back to my working context fast --- layout, folders, scrollback, reconnections |
| D5 | Remote host reboots | Occasional | I know it happened, I can re-establish context quickly, I can see what I was doing before |
| D6 | rttx is updated or reinstalled | Occasional | Same as D4 |

### State preservation matrix

| State element | D1 GUI crash | D2 Sleep/wake | D3 Network drop | D4 Local reboot | D5 Remote reboot | D6 Update |
|---|---|---|---|---|---|---|
| Layout and splits | Survives | Survives | N/A | Reconstructed | Reconstructed | Reconstructed |
| Session names and pane titles | Survives | Survives | N/A | Reconstructed | Reconstructed | Reconstructed |
| Working directories | Survives | Survives | Survives (remote) | Reconstructed | Reconstructed | Reconstructed |
| Scrollback content | Survives | Survives | Survives (remote) | Restored from logs | Restored from logs | Restored from logs |
| Running processes (local) | Survives | Survives | N/A | Lost (new shells) | N/A | Lost (new shells) |
| Running processes (remote) | Survives | Survives | Survives | Survives | Lost (new shells) | Survives |
| SSH connections | Survives | Should reconnect | Must reconnect | Must reconnect | N/A | Must reconnect |
| Per-session command history | Survives | Survives | Survives (remote) | Restored from disk | Restored from disk | Restored from disk |
| Focused pane and active session | Survives | Survives | N/A | Reconstructed | Reconstructed | Reconstructed |

### Functional requirements

- **R1** --- GUI lifecycle is decoupled from session lifecycle. GUI crash or close does not kill
  sessions.
- **R2** --- Remote processes survive anything that happens on the local machine: sleep, reboot,
  network drop, update.
- **R3** --- Scrollback is persistently logged to disk so it survives any disruption on either end.
- **R4** --- After local reboot, the user returns to a recognizable workspace: same layout, same
  sessions, same folders, same scrollback, with fresh shells spawned automatically.
- **R5** --- After remote host reboot, the user sees scrollback from before the reboot, knows the
  session was reconstructed, and has a live shell in the correct directory.
- **R6** --- Command history is per-session and persists across all disruption scenarios.
- **R7** --- Recovery is automatic on startup --- not a manual "restore session" action.
- **R8** --- Partial recovery is acceptable. Individual panes may fail while others succeed. Failures
  are visible and retryable.

---

## Goals

- **G1** --- Sessions survive GUI disconnect, local reboot, and remote reboot
- **G2** --- One daemon, one engine, no tmux dependency
- **G3** --- Same daemon binary serves local and remote sessions
- **G4** --- Preserve the current direct VTE terminal path unchanged
- **G5** --- Keep the host-side daemon portable: no GTK, no VTE, no systemd requirement
- **G6** --- Frontend owns selection, copy, search, and clipboard in persistent mode
- **G7** --- Architecture supports incremental adoption: direct and persistent sessions coexist

## Non-Goals

- **NG1** --- Do not remove or weaken the current direct VTE terminal path
- **NG2** --- Do not require `systemd --user` or any Linux-only service manager
- **NG3** --- Do not require VTE or GTK on the host side
- **NG4** --- Do not attempt process checkpointing (CRIU or similar)
- **NG5** --- Do not build a TUI client for rttxd --- it is always a backend for the rttx GUI

---

## Architecture

### Overview

```
Local machine                          Remote host
--------------                         -----------
rttx (GTK GUI)                         rttxd (daemon)
    |                                      |
    |--- Unix socket ----> rttxd           |--- PTYs (bash, zsh, ...)
    |    (local daemon)        |           |
    |                          |           |--- state.json (periodic)
    |--- SSH tunnel -------->(protocol)    |--- scrollback/*.log
    |                                      |
    |--- state.json (layout metadata)
```

### Process model

**rttxd (daemon)** --- runs on any host, owns all persistent PTYs for that host:

- spawns shells, owns PTY file descriptors
- continuously drains PTY output into per-pane screen state
- persists scrollback to per-pane log files on disk (every 1 second)
- serializes session metadata to `state.json` atomically (every 1 second)
- on startup, loads persisted state, replays scrollback, spawns fresh shells
- serves clients over Unix socket (local) or stdio (remote via SSH)

**rttx (GUI)** --- remains the GTK4 frontend:

- connects to one or more rttxd instances (local + N remote)
- renders terminal content from daemon-provided snapshots and deltas
- owns selection, copy, paste, search, link detection
- sends keyboard input and resize events to the daemon
- may crash or disconnect without killing any session

### Why one server per host, not per session

RFC-013 v1 proposed per-session daemon processes (`rttx-sessiond`) for failure isolation. In
practice, Zellij has proven that one server process per host works reliably at scale. Per-session
processes add significant complexity (discovery, lifecycle management, socket-per-session) for
marginal benefit. If the daemon crashes, all sessions on that host are affected --- but the
scrollback logs and state file survive, so reconstruction is immediate on restart.

The failure domain is acceptable: one host, one daemon, periodic state to disk.

---

## Design

### 1. Session families

Two session families coexist in the same window:

**Direct sessions** --- today's model, unchanged:
- local shell in VTE
- SSH in VTE
- raw tmux in VTE (for users who explicitly want tmux)

**Persistent sessions** --- new:
- backed by rttxd (local or remote)
- session lifetime independent from GUI
- scrollback and metadata survive reboots

### 2. Daemon: rttxd

Repository: [IllyaYalovyy/rttxd](https://github.com/IllyaYalovyy/rttxd)

#### Existing implementation

The daemon skeleton exists with:
- Protobuf wire protocol (`rttx-proto` crate)
- Unix socket IPC with length-prefixed framing
- Session/Pane data model with attach/detach semantics
- NativeEngine with PTY spawning via `pty-process`
- Periodic state serialization to disk (1-second tick, atomic write)
- State loading and session resurrection on startup

#### What is missing

| Gap | Issue |
|---|---|
| PTY output is not read --- no Delta messages | [rttxd#1](https://github.com/IllyaYalovyy/rttxd/issues/1) |
| Input/Resize not routed to PTY | [rttxd#2](https://github.com/IllyaYalovyy/rttxd/issues/2) |
| Scrollback not persisted to disk | [rttxd#3](https://github.com/IllyaYalovyy/rttxd/issues/3) |
| Sessions not reconstructed on restart (no shell re-spawn, no scrollback replay) | [rttxd#4](https://github.com/IllyaYalovyy/rttxd/issues/4) |
| No daemon mode (fork, PID file, signals) | [rttxd#5](https://github.com/IllyaYalovyy/rttxd/issues/5) |
| No SSH stdio transport for remote access | [rttxd#6](https://github.com/IllyaYalovyy/rttxd/issues/6) |
| TmuxEngine stub should be removed | [rttxd#7](https://github.com/IllyaYalovyy/rttxd/issues/7) |

#### Persistence model

State is written to disk continuously, not just on shutdown.

**Metadata** (`<cache_dir>/state.json`, every 1 second):
```json
{
  "sessions": [{
    "id": "...",
    "name": "dev",
    "panes": [{
      "id": "...",
      "cwd": "/home/user/project",
      "title": "bash",
      "cols": 120, "rows": 40,
      "scrollback_log_path": "scrollback/<session>/<pane>.log",
      "exit_status": null
    }],
    "active_pane_id": "...",
    "command_history": [...]
  }],
  "server_version": "0.1.0"
}
```

**Scrollback** (`<cache_dir>/scrollback/<session_id>/<pane_id>.log`, append-only, every 1 second):
- Raw terminal bytes, appended incrementally
- Pane tracks `flushed_offset` to avoid rewriting
- Capped at configurable size (default 10 MB per pane) with rotation

**Reconstruction on daemon restart:**
1. Load `state.json`
2. For each non-exited pane: load scrollback log into `PaneScreen`
3. Spawn fresh shell in saved CWD via `NativeEngine`
4. Wire PTY output loop
5. On client attach: send Snapshot with replayed scrollback + live output

After reboot, the user sees their previous scrollback and a fresh shell prompt in the same
directory. Processes are new, but context is preserved.

#### Protocol

Protobuf over length-prefixed frames. Same protocol for both transports.

Key message types (already defined in `rttx.proto`):

| Direction | Message | Purpose |
|---|---|---|
| C->S | Hello | Protocol version handshake |
| C->S | CreateSession / AttachSession / DetachSession | Session lifecycle |
| C->S | CreatePane / ClosePane | Pane lifecycle |
| C->S | Input | Keyboard bytes to pane PTY |
| C->S | Resize | Terminal dimensions change |
| S->C | Snapshot | Full pane state on attach |
| S->C | Delta | Incremental PTY output |
| S->C | PaneExited / PaneClosed | Pane lifecycle events |
| S->C | Bell / TitleChanged / CwdChanged | Terminal events |
| S->C | Error | Error responses |

#### Transports

**Local:** Unix domain socket at `<runtime_dir>/rttxd.sock`

**Remote:** SSH stdio tunneling. The GUI runs `ssh <host> rttxd attach-stdio` and speaks the
protocol over the subprocess's stdin/stdout. This requires only that `rttxd` is installed on the
remote host and accessible in `$PATH`. No port forwarding, no extra sockets.

### 3. GUI: rttx client changes

#### PersistentPaneView ([#122](https://github.com/IllyaYalovyy/rttx/issues/122))

New widget for rendering daemon-backed panes. First implementation: `vte4::Terminal` in feed mode
(no PTY). The daemon sends raw bytes via Delta; the widget feeds them into VTE for rendering.

- Keyboard input captured and sent as `Input` messages
- Resize events sent as `Resize` messages
- Selection, copy, paste owned by the GUI (paste sends bytes as Input)
- Search operates on locally rendered VTE content
- Connection status indicator (connected / reconnecting / disconnected)

#### Daemon connection manager ([#123](https://github.com/IllyaYalovyy/rttx/issues/123))

`DaemonConnection` struct managing the protocol lifecycle:

- Local transport: connect to Unix socket
- Remote transport: spawn SSH subprocess, protocol over stdin/stdout
- Async message send/receive, routed to GTK main loop via channels
- Reconnect with exponential backoff on disconnect
- One connection per rttxd instance (multiplexes sessions/panes)

#### Local daemon auto-start ([#124](https://github.com/IllyaYalovyy/rttx/issues/124))

On GUI startup:
1. Check if rttxd is running (probe socket or PID file)
2. If not, spawn `rttxd start` and wait for socket
3. If rttxd binary not found, persistent sessions unavailable (direct mode only)
4. Version check via Hello/HelloAck

#### Session creation UI ([#125](https://github.com/IllyaYalovyy/rttx/issues/125))

Extend session/bookmark model:
- `execution_mode: direct | persistent`
- `host: Option<String>` for remote persistent sessions
- Visual distinction in sidebar for persistent sessions
- Close behavior: "detach" (keep running) vs "terminate" (kill session)
- On GUI restart: re-attach to running daemon sessions

#### Remote connection ([#126](https://github.com/IllyaYalovyy/rttx/issues/126))

- Bookmark with `host` field triggers SSH transport
- GUI spawns `ssh <host> rttxd attach-stdio`
- Same protocol, same PersistentPaneView, different transport
- SSH auth relies on ssh-agent / SSH config / key files
- SSH drop detection with reconnect

### 4. Selection and clipboard

**Direct sessions:** no change --- VTE handles everything.

**Persistent sessions:** GUI-local semantics:
- Selection performed on locally rendered VTE feed-mode widget
- Copy copies from rttx
- Paste sends bytes to the daemon as Input
- No tmux copy-mode conflicts

### 5. Host compatibility

rttxd requires:
- Rust standard library (statically linked binary is fine)
- PTY support (POSIX, available on any Linux/macOS)
- No GTK, no VTE, no systemd, no root
- User-space installable, launchable through `ssh`

### 6. Failure model

| Failure | Impact | Recovery |
|---|---|---|
| GUI crash | Sessions continue in daemon | Re-attach on restart |
| Local daemon crash | Local sessions lost in-flight | Reconstruct from state.json + scrollback logs |
| Local reboot | Local daemon stops | Daemon auto-starts, reconstructs from disk |
| SSH connection drop | Remote sessions continue | GUI reconnects automatically |
| Remote daemon crash | Remote sessions lost in-flight | Reconstruct from state.json + scrollback logs on remote |
| Remote reboot | Remote daemon stops | Remote daemon restarts (systemd/cron/manual), reconstructs |

After any crash or reboot, the reconstruction model is identical: load metadata, replay scrollback,
spawn fresh shells. Running processes are lost but working context is preserved.

### 7. Relationship to current recovery (RFC-007)

RFC-007's `PaneRecovery` / `PaneTarget` system remains valid for direct sessions. Persistent
sessions do not use it --- the daemon owns session state directly.

The two systems coexist:
- Direct sessions: RFC-007 recovery (reconstruct from bookmarks/commands/CWD)
- Persistent sessions: daemon state (scrollback + metadata on disk)
- If a persistent session's daemon is unavailable, the GUI can fall back to RFC-007 recovery as a
  degraded path

---

## Development Plan

### Phase 1: Daemon functional (rttxd)

Make rttxd a working terminal multiplexer that a client can connect to and use interactively.

| Step | Description | Issue | Depends on |
|---|---|---|---|
| 1.0 | Remove TmuxEngine stub | [rttxd#7](https://github.com/IllyaYalovyy/rttxd/issues/7) | --- |
| 1.1 | Wire PTY output loop and Delta streaming | [rttxd#1](https://github.com/IllyaYalovyy/rttxd/issues/1) | --- |
| 1.2 | Route Input and Resize to PTY | [rttxd#2](https://github.com/IllyaYalovyy/rttxd/issues/2) | --- |
| 1.3 | Persist scrollback to disk | [rttxd#3](https://github.com/IllyaYalovyy/rttxd/issues/3) | 1.1 |
| 1.4 | Reconstruct sessions on daemon restart | [rttxd#4](https://github.com/IllyaYalovyy/rttxd/issues/4) | 1.1, 1.3 |
| 1.5 | Daemon lifecycle (fork, PID, signals) | [rttxd#5](https://github.com/IllyaYalovyy/rttxd/issues/5) | --- |

**Milestone:** connect to rttxd with a test client, type commands, see output, restart daemon,
see scrollback restored.

### Phase 2: GUI integration (rttx + local rttxd)

Connect the rttx GUI to a local rttxd instance.

| Step | Description | Issue | Depends on |
|---|---|---|---|
| 2.1 | PersistentPaneView widget | [rttx#122](https://github.com/IllyaYalovyy/rttx/issues/122) | Phase 1 |
| 2.2 | Daemon connection manager | [rttx#123](https://github.com/IllyaYalovyy/rttx/issues/123) | Phase 1 |
| 2.3 | Local daemon auto-start | [rttx#124](https://github.com/IllyaYalovyy/rttx/issues/124) | 2.2 |
| 2.4 | Persistent session creation UI | [rttx#125](https://github.com/IllyaYalovyy/rttx/issues/125) | 2.1, 2.2, 2.3 |

**Milestone:** create a persistent session in rttx, close the GUI, reopen, session is still there
with scrollback. Reboot, daemon auto-starts, sessions reconstruct.

### Phase 3: Remote sessions (rttx + remote rttxd)

Connect the rttx GUI to rttxd on a remote host over SSH.

| Step | Description | Issue | Depends on |
|---|---|---|---|
| 3.1 | SSH stdio transport in rttxd | [rttxd#6](https://github.com/IllyaYalovyy/rttxd/issues/6) | Phase 1 |
| 3.2 | Remote daemon connection in rttx | [rttx#126](https://github.com/IllyaYalovyy/rttx/issues/126) | 2.2, 3.1 |

**Milestone:** SSH to remote host from rttx, work in persistent pane, close laptop, reopen, remote
session is still running. Remote host reboots, daemon reconstructs, user sees previous scrollback
and fresh shell.

---

## Open Questions

- **Q1** --- Should `PersistentPaneView` use VTE in feed mode (fast, reuses selection/search) or a
  custom GTK widget (more control, much more work)? Recommendation: VTE feed mode first.
- **Q2** --- What scrollback log size cap should be the default? 10 MB per pane seems reasonable.
- **Q3** --- Should the daemon auto-start on login (systemd user unit) or only when rttx launches?
- **Q4** --- How should the GUI handle multiple remote hosts with different rttxd versions?
- **Q5** --- Should the reconstruction banner (showing the session was rebuilt after reboot) be
  injected as terminal output or as a GUI overlay?

---

## Prior Art: Zellij

[Zellij](https://github.com/zellij-org/zellij) (MIT, Rust, 30k+ stars) validates the core
architecture of this RFC:

- A Rust-native PTY-owning server works reliably in production at scale
- Periodic serialization (every 1 second) of layout + commands + scrollback to disk is proven
- Session resurrection from serialized state after reboot works well
- The "don't auto-run resurrected commands" safety pattern is important
- Versioned protocol contracts allow binary upgrades without breaking sessions

Key differences from rttx:
- Zellij is a TUI multiplexer running inside a terminal; rttx IS the terminal (GTK4)
- Zellij uses one server for all sessions (same as this RFC's updated process model)
- Zellij cannot run on remote hosts without SSH + tmux; rttxd can serve remote sessions natively

---

## References

- [RFC-007: Per-Pane Recovery Recipes & Smart Session Restoration](./RFC-007-session-recovery.md)
- [rttxd repository](https://github.com/IllyaYalovyy/rttxd)
- [Zellij terminal workspace](https://github.com/zellij-org/zellij)
- [Zellij session resurrection docs](https://zellij.dev/documentation/session-resurrection)
- [`pty-process` crate](https://docs.rs/pty-process/latest/pty_process/)
- [`prost` crate](https://docs.rs/prost/latest/prost/) --- protocol buffer serialization
- [`vte` crate](https://docs.rs/vte/latest/vte/) --- terminal escape sequence parser (no GTK)
