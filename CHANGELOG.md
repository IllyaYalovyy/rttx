# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Server-authoritative pane tree: the daemon now owns an immutable `PaneId` and
  a `WorkspaceTree` (leaves + splits with logical ratios + default-active pane)
  as the single source of truth for workspace structure (RFC-031 Step 1).
- Durable workspace persistence (`WorkspaceFileV2`): the daemon now persists the
  authoritative pane tree — structure, logical split ratios, and default-active
  pane — so layout survives daemon restarts (RFC-031 Step 2).
- Server-authoritative tree protocol (RFC-031 Step 3): the attach snapshot now
  carries the full workspace tree and default-active pane; new tree mutations
  `SplitPane` (server-minted, immutable pane id), `ResizeSplit`, and viewport
  messages `SetFocus`/`ReportClientSize` drive a multi-client PTY min-size
  policy so no client sees truncated output.
- Shell-correct durable history (RFC-031 Step 5): per-pane history is now
  initialized per shell, keyed on the durable `PaneId`, and robust against the
  user's rc files. bash spawns with a generated `--rcfile` that sources
  `~/.bashrc` then *appends* `history -a` to `PROMPT_COMMAND` (so a user's own
  `PROMPT_COMMAND` can no longer disable capture); zsh uses a generated
  `ZDOTDIR` with `INC_APPEND_HISTORY`; fish selects a per-pane history session;
  other shells set `HISTFILE` best-effort.
- One-time orphaned-histfile salvage utility (RFC-031 Step 7): the new
  `rttx-server salvage-history` subcommand performs a read-only scan of the
  daemon state directory for per-pane history files left unreferenced by the
  pre-RFC-031 layout and copies them into a recovery directory (`--dry-run` to
  preview, `--dest` to choose the target). It is strictly opt-in and never
  touches live runtime state, so it adds no compatibility code to normal daemon
  operation.

### Changed

- Daemon state schema bumped to version 2. This is a clean break: old-schema
  (v1) runtime state is detected, ignored, and removed on first load with no
  migration path. The obsolete `command_history` field is gone.
- Replaced the bash-only `PROMPT_COMMAND=history -a` environment hack with
  shell-aware history initialization (see Added, RFC-031 Step 5).

## [0.9.0] - 2026-05-26

## [0.6.5] - 2026-05-26

### Added

- Disconnect visualization for persistent panes — frozen indicator when daemon dies
- Remote daemon lifecycle management — start, stop, and monitor remote daemons
- Force retry for stuck "Connecting…" state with handshake timeout

### Fixed

- Chunk VTE feed_output to reduce crash risk on large snapshots
- Atomic screen snapshot writes to prevent corruption on hard kills

## [0.6.1] - 2026-05-22

### Added

- F2 keyboard shortcut for workspace rename
- Reconnecting workspace tab visually distinct from connecting state
- Block input during snapshot feed to prevent CPR garbage on restore

### Fixed

- Keep window open with fresh workspace on Close All
- Set VTE size before feeding snapshot on restore
- Feed cleanup bytes after snapshot when terminal_modes is None
- Reduce label filter chip size for better visual density

## [0.5.1] - 2026-05-18

### Added

- Daemon profiling infrastructure: ring buffer flight recorder, metrics, crash reports
- `rttx-server profile` CLI command for live performance diagnostics
- Watchdog task for daemon hang detection
- Instrumentation for PTY read, VTE parse, serialization, mutex, and channel operations
- Enhanced panic hook with crash report generation
- Optional description field for command parameters

### Fixed

- Reset VTE before sending recovery input on pane restore
- Log peer PID on Shutdown and strip CPR responses from output
- Parameter persistence with recursive ListBox search
- Handle SIGHUP and log signal number on daemon shutdown
- Expand tilde and resolve relative CWD paths before is_dir check

## [0.4.0] - 2026-04-20

### Added

- Protocol v3 with version negotiation, capabilities, and structured envelopes
- ClientEnvelope/ServerEnvelope correlation for request tracking
- TerminalModeState and TerminalModeChanged for terminal mode synchronization
- PasteInput and FocusInput as structured input types
- ProtocolError with typed ErrorKind for actionable error reporting
- RuntimeSnapshot with scrollback_tail and pane_output_seq
- OPT_CHUNKED_SCROLLBACK for incremental scrollback delivery
- OPT_RESYNC (StreamOverflow/ResyncRuntime) for overflow recovery
- OPT_RUNTIME_INVENTORY_V2 for enhanced runtime discovery
- OPT_RUNTIME_TAKEOVER for session ownership transfer
- OPT_DIAGNOSTICS for runtime health inspection
- Preserve VTE scroll position across split, rebuild, and reparent

### Changed

- Renamed Session types to Runtime/Workspace across codebase for terminology alignment
- Complete v3 protocol integration in both client and daemon

### Fixed

- Prevent Blocked status override during transient reconnect
- Panes show bottom of scrollback instead of top after reconnect

## [0.3.0] - 2026-04-17

### Added

- Customizable keyboard shortcuts via Preferences (RFC-024)
- Client logging migrated from `log` to `tracing` with structured output
- Trim trailing whitespace on copy option
- Configurable pane navigation with Alt+Arrow
- Zoom pane toggle (Ctrl+Shift+Z)
- Zoom button in pane title bar
- Smart workspace auto-naming from CWD and hostname
- Layout rotation to toggle split orientations (Ctrl+Shift+R)
- Workspace context menu on sidebar 3-dot button
- Open Link and Copy Link in right-click context menu
- Connection status icon in workspace sidebar row
- Active pane command/path shown in sidebar subtitle
- Pane count in workspace row subtitle
- Propagate light/dark mode to daemon panes via COLORFGBG
- Forward mouse events to daemon for persistent panes
- `rttx-server clean` command to remove unused daemon sessions
- Daemon heartbeat for connection health monitoring
- Memory diagnostics command and periodic metrics logging
- File logging with daily rotation for GUI and daemon
- Per-pane shell history via HISTFILE
- New Remote Workspace UI action
- Edit Connection dialog in sidebar context menu
- Attach to Remote Runtime action
- Indicate remote vs local in bookmark sidebar
- Adwaita modernization: notifications and color coding (RFC-002)

### Fixed

- Darken Daybreak bright ANSI colors for WCAG AA contrast
- Apply paned ratios on realize to prevent split jump
- Strip bell characters from snapshot scrollback
- Handle VTE spawn_async failures with error display
- Set COLORTERM=truecolor in daemon-spawned PTYs
- Forward F-keys and modifier+navigation key combos to PTY in managed panes
- Pass source pane CWD to daemon when splitting managed panes
- Treat ERR_PANE_NOT_FOUND as successful close
- Retry connection falls back to new runtime on stale id
- Propagate workspace rename from client to daemon
- Apply bell preferences to daemon-managed panes
- Broadcast CwdChanged and TitleChanged from daemon on OSC events
- Add SSH timeout and BatchMode to prevent reconnect hang
- Always schedule reconnect regardless of error type
- Bypass stuck EndpointActor on explicit retry
- Close window when closing the last workspace
- Break reconnect reattach loop on first transient failure
- Prevent heartbeat timeouts from write-blocking-read deadlock
- Prevent workspace layout growth on repeated reconnect cycles
- Break GObject reference cycles between Window and pane widgets
- Guard signal handler and timer stacking on reconnect
- Remove stale HashMap entries after workspace reconciliation
- Replace unbounded channels with bounded channels for backpressure
- Release scrollback buffer when pane process exits
- Strip DSR/DA query sequences from client-bound output
- Filter VTE CPR responses from client-to-daemon input path
- Strip DSR queries from scrollback before writing to disk
- Limit event poller batch size to prevent UI hang during output bursts
- Pass VTE terminal size in CreatePane instead of zeros
- Resolve PTY CWD to home directory when unspecified
- Fall back to layout node CWD when splitting panes
- Swap right-click and Shift+right-click to match GNOME conventions
- Prevent unbounded daemon process spawning on reconnect
- Cap on-disk scrollback log at 10 MB per pane
- Replace PID-based single-instance check with flock
- Silently drop Input and Resize for missing panes
- Cap snapshot bytes and scroll to bottom on reconnect
- Handle DSR escape sequences in daemon-backed panes
- Improve light palette contrast for CLI apps

### Performance

- Use Bytes for Delta, Input, and Snapshot data fields
- Coalesce PTY reads into batched Delta messages
- Prioritize control messages over data in client_writer
- Release mutex before broadcasting in PTY read loop
- Handle Ping without acquiring the server mutex

### Removed

- Tmux-related data model and UI paths
- Template-related UI and data paths

## [0.2.0] - 2026-04-06

### Added

- Daemon-backed persistent sessions via `rttx-server` (RFC-013)
- PTY I/O loop with Delta streaming, Input/Resize routing
- Scrollback persistence to disk per pane
- Session reconstruction on daemon restart
- DaemonConnection for client-daemon communication
- PersistentPaneView widget for daemon-backed panes
- Daemon lifecycle with daemonize and PID file
- Persistent session creation with daemon auto-start
- `attach-stdio` command for SSH remote sessions
- SSH transport for remote daemon connections
- VTE version feature flags for 0.76 and 0.78 support
- Debug logging in dev mode
- Dev mode for side-by-side development and production instances
- Managed workspace runtime flows with connection controls
- Endpoint event reducer for runtime state management
- Runtime inventory metadata exposure
- Runtime revision acknowledgements
- Runtime retention and ownership semantics
- Git commit hash in version strings
- `rttx-server status` command

### Fixed

- Duplicate restore entries and clipboard for persistent panes
- Smart clipboard shared state and duplicate session restore
- Daemon auto-start with stale socket, session restore dedup
- Deduplicate persistent sessions by name across restarts
- Daemon bridge lifecycle and widget replacement visibility
- Rebind daemon-backed panes to runtime UUIDs
- Keep managed pane size aligned with viewport
- Preserve managed clipboard shortcuts
- Preserve pane CWD across daemon restart
- Compact managed recovery status
- Normalize smart clipboard shortcut modifiers
- Recover daemon inventory into managed workspaces
- Keep selected session visible during recovery

## [0.1.0] - 2026-03-19

Initial release — a tiling terminal emulator for GNOME built with Rust, GTK4, and Libadwaita.

### Added

- Named workspaces in a left sidebar for separate work contexts
- Split panes horizontally or vertically, up to 5 levels deep
- Drag pane headers to rearrange workspace layout
- Broadcast keystrokes to all panes in a workspace (input sync)
- Folder and SSH bookmarks for quick access
- Saved and searchable commands in the right sidebar
- Pane recovery recipes that reconstruct what the user was doing
- Ctrl+click to open URLs, OSC 8 hyperlinks, and detected file paths
- Right-click context menu; Shift+right-click passes mouse events to terminal apps
- Smart clipboard: plain Ctrl+C copies selected text, Ctrl+V pastes
- Built-in Nightfall (dark) and Daybreak (light) themes
- Tilix color scheme compatibility
- Background process notifications via toast or desktop notification
- Configurable default folder for new sessions
- New split panes inherit current working directory
- Confirm before closing sessions with multiple terminals
- Session row activity indicator
- Session number display for Alt+Number access
- Ctrl+Tab session switching
- Drag-and-drop reordering of bookmarks, commands, and session tabs
- Visual bell with option to mute audible bell
- Internal padding for terminal breathing room
- Framed terminal panes with scrollbar and focus state
- Resizable session and tools sidebars via GtkPaned
- Isolated development mode with separate config paths
- AT-SPI2 behavioral UI test suite
- Flatpak packaging with host shell access support
- RPM packaging for Fedora COPR
- CI quality gate and release pipeline

### Known Limitations

- No daemon support in this release (added in 0.2.0)
- Workspace state persists locally only — no remote sync
- Single window only — multi-window support planned for a future release

[Unreleased]: https://github.com/IllyaYalovyy/rttx/compare/v0.9.0...HEAD
[0.9.0]: https://github.com/IllyaYalovyy/rttx/compare/v0.6.5...v0.9.0
[0.6.5]: https://github.com/IllyaYalovyy/rttx/compare/v0.6.1...v0.6.5
[0.6.1]: https://github.com/IllyaYalovyy/rttx/compare/v0.5.1...v0.6.1
[0.5.1]: https://github.com/IllyaYalovyy/rttx/compare/v0.4.0...v0.5.1
[0.4.0]: https://github.com/IllyaYalovyy/rttx/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/IllyaYalovyy/rttx/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/IllyaYalovyy/rttx/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/IllyaYalovyy/rttx/releases/tag/v0.1.0
