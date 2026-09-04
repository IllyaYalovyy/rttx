# RFC-021: Client/Server Protocol v3

| Field         | Value                                                       |
|---------------|-------------------------------------------------------------|
| Status        | Review                                                      |
| Author(s)     | Illya Yalovyy                                               |
| Supersedes    | —                                                           |
| Superseded by | —                                                           |

---

## Summary

Replace the v2 daemon protocol with a clean v3 protocol built for long-term stability. v3 uses
protobuf over the same transports (Unix socket, SSH stdio) but adds version negotiation,
capability advertisement, request/response envelopes, typed errors, consolidated terminal mode
state, structured paste and focus input, chunked scrollback, and explicit wire compatibility
rules.

This is a clean-slate replacement. There is no v2 migration path — all client and server code
moves to v3 together. The v3 protocol is designed to be the foundation for 1.0 and must support
backward-compatible evolution from that point forward.

---

## Goals

- **G1** — Make protocol evolution deliberate through version negotiation and capabilities
- **G2** — Give every daemon feature a natural protocol domain: control, inventory, runtime
  lifecycle, pane lifecycle, terminal I/O, and recovery
- **G3** — Make terminal interaction state first-class via consolidated `TerminalModeState`
- **G4** — Support reconnect and reattach using explicit revisions and resync, not raw scrollback
  replay alone
- **G5** — Support host-aware creation and "connect to existing" discovery (RFC-016)
- **G6** — Preserve the transport model: local Unix socket and remote
  `ssh rttx-server attach-stdio` speak the same logical protocol
- **G7** — Make protocol behavior testable with daemon integration tests
- **G8** — Establish wire compatibility rules that hold from 1.0 onward

## Non-Goals

- **NG1** — Mixed-endpoint workspaces; one workspace lives on one endpoint
- **NG2** — Replace protobuf framing or require a network service beyond Unix sockets and SSH
- **NG3** — Full terminal emulator in the daemon (Option B, rejected)
- **NG4** — Structured key or mouse input events (raw bytes remain the primary path; the `oneof`
  allows adding these later if a concrete need arises)

---

## Background & Motivation

The v2 protocol is a length-prefixed protobuf stream with flat `ClientMessage`/`ServerMessage`
`oneof` envelopes. It uses exact `PROTOCOL_VERSION` equality during handshake. This served as a
good starting point, but has accumulated structural problems:

1. **No compatibility window** — exact version match prevents a newer GUI from connecting to
   an older daemon. This matters because daemons run on remote hosts that are painful to
   update in lockstep with the GUI.
2. **Terminal input is untyped raw bytes** — `Input { data }` cannot distinguish text, paste,
   focus, or terminal responses. The daemon sees only bytes.
3. **Request/response correlation is implicit** — the client reads "the next non-push message"
   as the response, which is fragile as the protocol grows.
4. **Errors are ad hoc** — `Error { code, message }` lacks typed variants, retryability, and
   operation context.
5. **Terminology mismatch** — wire messages use `Session*` names while the product uses
   workspace/runtime/pane terminology.
6. **Snapshots lack revisions** — no delta catch-up or chunked scrollback for large histories.
7. **Terminal modes are scattered** — five separate booleans in `PaneSnapshot` instead of a
   consolidated mode state; two tracked modes (`focus_event_mode`, `cursor_visible`) are not
   on the wire at all.
8. **Backpressure is implementation-only** — bounded push channels drop messages silently with
   no protocol-level recovery path.

Most terminal correctness issues (#457–#464) were resolved within v2. The remaining gaps are
structural: version negotiation, correlation, typed errors, consolidated terminal state, and
resync semantics.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | More reliable reconnect, clearer session discovery, fewer terminal regressions |
| Contributors | Clear protocol domains and evolution rules; consistent terminology |
| Packagers    | Client/server version mismatches produce clear errors instead of silent failures |

---

## Decision

**Clean-slate v3 protocol** using protobuf over the existing transports. No v2 compatibility
layer — all code migrates together. The v3 wire format is designed to be the 1.0 foundation
with additive-only evolution within major versions.

---

## Design

### 1. Version and Capability Negotiation

#### Handshake

```protobuf
message ClientHello {
  uint32 min_protocol_version = 1;
  uint32 max_protocol_version = 2;
  bytes client_id = 3;            // 16-byte UUID
  string client_name = 4;         // e.g. "rttx"
  string client_version = 5;      // e.g. "0.4.0"
  repeated Capability capabilities = 6;
}

message ServerHello {
  uint32 negotiated_protocol_version = 1;
  bytes server_id = 2;            // 16-byte UUID
  string server_version = 3;
  repeated Capability capabilities = 4;
}
```

#### Capability type

```protobuf
enum Capability {
  CAPABILITY_UNSPECIFIED = 0;
  // Core (required) — values 1–99
  CORE_RUNTIME_LIFECYCLE = 1;
  CORE_PANE_LIFECYCLE = 2;
  CORE_TERMINAL_IO = 3;
  CORE_TERMINAL_MODES = 4;
  CORE_PASTE_INTENT = 5;
  CORE_FOCUS_EVENTS = 6;
  // Optional — values 100+
  OPT_RUNTIME_INVENTORY_V2 = 100;
  OPT_RUNTIME_TAKEOVER = 101;
  OPT_RESYNC = 102;
  OPT_CHUNKED_SCROLLBACK = 103;
  OPT_DIAGNOSTICS = 104;
}
```

Core values occupy 1–99, optional values start at 100. This leaves room for future core
additions in a new major protocol version.

#### Negotiation rules

- Client sends the range of protocol versions it supports.
- Server selects the highest mutually supported version.
- If no overlap, server returns `ProtocolError` with kind `PROTOCOL_MISMATCH` and includes its
  own supported range in the error message so the client can display actionable guidance
  (e.g. "daemon supports v3, client requires v4 — please update the daemon").
- After version negotiation, the effective capability set is the intersection of client and
  daemon capabilities.
- The client must never send a command that requires a capability the daemon did not advertise.

#### Compatibility model

rttx supports asymmetric compatibility. The GUI is expected to be updated first and may drop
support for older GUI behavior at any time — if the GUI is old, update it. The daemon is
harder to update: it may be remote, embedded in long-running sessions, or protecting active
work. A current GUI must connect to older supported daemons through version negotiation and
optional capability fallback. Daemons must not require a newer GUI than the negotiated
protocol/capability set.

This means:
- Protocol evolution optimizes for **new GUI → older daemon**, not old GUI → newer daemon.
- The effective capability set is the intersection, but in practice the GUI is the newer side.
- The daemon must never send optional events, fields, or commands that were not negotiated.
- The GUI must never send commands requiring capabilities the daemon did not advertise.
- The testing matrix is: latest GUI against each supported daemon capability profile, not
  arbitrary old/new combinations.

#### Core capabilities

Required by all v3 implementations. If a daemon does not advertise all core capabilities, the
client rejects the connection.

| Capability | Covers |
|---|---|
| `CORE_RUNTIME_LIFECYCLE` | Create, attach, detach, terminate, rename runtime |
| `CORE_PANE_LIFECYCLE` | Create, close, resize pane; all pane events |
| `CORE_TERMINAL_IO` | Raw byte input/output, snapshot on attach |
| `CORE_TERMINAL_MODES` | Full `TerminalModeState` in snapshots and mode-change events |
| `CORE_PASTE_INTENT` | Structured `PasteInput` with daemon-side bracketed paste wrapping |
| `CORE_FOCUS_EVENTS` | Structured `FocusInput`; daemon generates focus in/out sequences |

#### Optional capabilities

Client degrades gracefully when absent.

| Capability | Enables | Fallback when absent |
|---|---|---|
| `OPT_RUNTIME_INVENTORY_V2` | Rich inventory with takeover eligibility, disabled reasons | Basic inventory (name, pane count, attached status) |
| `OPT_RUNTIME_TAKEOVER` | Explicit takeover command and lease events | "Session busy" without takeover option |
| `OPT_RESYNC` | `StreamOverflow` event + `ResyncRuntime` command | Full detach/reattach on suspected data loss |
| `OPT_CHUNKED_SCROLLBACK` | Paginated `GetScrollback` for large histories | Truncated snapshot tail only |
| `OPT_DIAGNOSTICS` | `GetDiagnostics` / `DiagnosticsReport` | Diagnostics UI disabled |

#### Evolution rules for capabilities

- New capabilities are always optional within a major protocol version.
- A capability may be promoted from optional to core only in a new major protocol version.
- Within a major version, all capabilities that were optional at release remain optional forever.

### 2. Command/Event Envelopes

```protobuf
message ClientEnvelope {
  uint64 request_id = 1;       // non-zero for commands expecting a response
  oneof command {
    // Terminal I/O (field 2: single-byte tag for highest-frequency path)
    TerminalInput terminal_input = 2;

    // Control
    Ping ping = 10;
    Shutdown shutdown = 11;

    // Runtime lifecycle
    CreateRuntime create_runtime = 20;
    AttachRuntime attach_runtime = 21;
    DetachRuntime detach_runtime = 22;
    TerminateRuntime terminate_runtime = 23;
    RenameRuntime rename_runtime = 24;
    ListRuntimes list_runtimes = 25;

    // Pane lifecycle
    CreatePane create_pane = 30;
    ClosePane close_pane = 31;
    ResizePane resize_pane = 32;
    SetPaneTitle set_pane_title = 33;

    // Recovery (OPT_RESYNC)
    ResyncRuntime resync_runtime = 50;

    // Scrollback (OPT_CHUNKED_SCROLLBACK)
    GetScrollback get_scrollback = 51;

    // Diagnostics (OPT_DIAGNOSTICS)
    GetDiagnostics get_diagnostics = 60;
  }
}

message ServerEnvelope {
  uint64 request_id = 1;       // echoed from client; zero for push events
  oneof payload {
    // Terminal I/O (fields 2–3: single-byte tags for highest-frequency paths)
    OutputDelta output_delta = 2;
    TerminalModeChanged terminal_mode_changed = 3;

    // Control
    Pong pong = 10;

    // Runtime lifecycle events
    RuntimeCreated runtime_created = 20;
    RuntimeSnapshot runtime_snapshot = 21;
    RuntimeDetached runtime_detached = 22;
    RuntimeTerminated runtime_terminated = 23;
    RuntimeRenamed runtime_renamed = 24;
    RuntimeList runtime_list = 25;
    AttachBlocked attach_blocked = 26;

    // Pane lifecycle events
    PaneCreated pane_created = 30;
    PaneClosed pane_closed = 31;
    PaneResized pane_resized = 32;
    PaneExited pane_exited = 33;
    TitleChanged title_changed = 34;
    CwdChanged cwd_changed = 35;
    Bell bell = 36;

    // Recovery events (OPT_RESYNC)
    StreamOverflow stream_overflow = 50;

    // Scrollback response (OPT_CHUNKED_SCROLLBACK)
    ScrollbackChunk scrollback_chunk = 51;

    // Diagnostics response (OPT_DIAGNOSTICS)
    DiagnosticsReport diagnostics_report = 60;

    // Errors
    ProtocolError error = 100;
  }
}
```

#### Correlation rules

- The handshake (`ClientHello`/`ServerHello`) is exchanged as bare length-prefixed protobuf
  messages before the envelope protocol begins. All subsequent messages use
  `ClientEnvelope`/`ServerEnvelope`.
- If the handshake fails (e.g. version mismatch), the server sends a bare length-prefixed
  `ProtocolError` message and closes the connection. `ProtocolError` is the only message type
  that may appear both inside a `ServerEnvelope` and as a bare handshake-phase message.
- Every client command that expects a response carries a non-zero `request_id`.
- Request IDs are assigned by the client, must be unique within a connection, and must be
  non-zero. The server does not validate ordering or monotonicity.
- The server echoes `request_id` in the response.
- Push events (output deltas, pane events, mode changes) use `request_id = 0`.
- Fire-and-forget commands (`TerminalInput`, `ResizePane`, `SetPaneTitle`, `Shutdown`) do not
  receive responses. If the target runtime or pane is invalid, the server silently drops the
  command. This matches real terminal behavior where typing into a dead shell produces nothing.
- The transport (TCP over Unix socket or SSH) guarantees ordered delivery. No sequence numbers
  are needed on the envelope.

#### Command response table

| Command | Response | Notes |
|---|---|---|
| `Ping` | `Pong` | |
| `Shutdown` | none | fire-and-forget |
| `CreateRuntime` | `RuntimeCreated` or `ProtocolError` | |
| `AttachRuntime` | `RuntimeSnapshot` or `AttachBlocked` or `ProtocolError` | |
| `DetachRuntime` | `RuntimeDetached` or `ProtocolError` | |
| `TerminateRuntime` | `RuntimeTerminated` or `ProtocolError` | |
| `RenameRuntime` | `RuntimeRenamed` or `ProtocolError` | |
| `ListRuntimes` | `RuntimeList` | |
| `CreatePane` | `PaneCreated` or `ProtocolError` | |
| `ClosePane` | `PaneClosed` or `ProtocolError` | |
| `ResizePane` | none | fire-and-forget; `PaneResized` is a push event |
| `SetPaneTitle` | none | fire-and-forget; `TitleChanged` is a push event |
| `TerminalInput` | none | fire-and-forget |
| `ResyncRuntime` | `RuntimeSnapshot` or `ProtocolError` | `OPT_RESYNC` |
| `GetScrollback` | `ScrollbackChunk` or `ProtocolError` | `OPT_CHUNKED_SCROLLBACK` |
| `GetDiagnostics` | `DiagnosticsReport` | `OPT_DIAGNOSTICS` |

### 3. Protocol Domains

#### Control domain

Handshake (`ClientHello`/`ServerHello`), `Ping`/`Pong`, `Shutdown`, `ProtocolError`.

#### Runtime lifecycle domain

`CreateRuntime`, `AttachRuntime`, `DetachRuntime`, `TerminateRuntime`, `RenameRuntime`,
`ListRuntimes`. Events: `RuntimeCreated`, `RuntimeSnapshot`, `RuntimeDetached`,
`RuntimeTerminated`, `RuntimeRenamed`, `AttachBlocked`.

#### Pane lifecycle domain

`CreatePane`, `ClosePane`, `ResizePane`, `SetPaneTitle`. Events: `PaneCreated`, `PaneClosed`,
`PaneResized`, `PaneExited`, `TitleChanged`, `CwdChanged`, `Bell`.

#### Terminal I/O domain

`TerminalInput` (client → server). Events: `OutputDelta`, `TerminalModeChanged`
(server → client).

#### Recovery domain (OPT_RESYNC)

`ResyncRuntime`. Events: `StreamOverflow`. Server may respond with `RuntimeSnapshot` (full
resync) or a delta catch-up.

#### Scrollback domain (OPT_CHUNKED_SCROLLBACK)

`GetScrollback`. Response: `ScrollbackChunk`.

### 4. Terminal Input Model

```protobuf
message TerminalInput {
  bytes runtime_id = 1;
  bytes pane_id = 2;
  // Fields 3–19 reserved for oneof kind variants.
  // Top-level fields (e.g. timestamp) should use 20+.
  oneof kind {
    RawInput raw = 3;           // always available (core)
    PasteInput paste = 4;       // CORE_PASTE_INTENT
    FocusInput focus = 5;       // CORE_FOCUS_EVENTS
  }
}

message RawInput {
  bytes data = 1;
  // Wrapped in a message so zero-length data is distinguishable from
  // "no kind set" in proto3 oneof semantics.
}

message PasteInput {
  bytes text = 1;
  // Daemon wraps with bracketed paste sequences when the mode is active.
  // bytes rather than string: paste content may include non-UTF-8 data.
}

message FocusInput {
  bool focused = 1;
  // Daemon generates focus in/out escape sequences when focus reporting is active.
}
```

`raw` remains for keyboard input, mouse input, and any other byte-level terminal interaction.
The `oneof` is forward-compatible — `KeyInput`, `MouseInput`, and other structured variants can
be added in future versions without any wire break.

### 5. Terminal Interaction State

A single consolidated message replaces the scattered booleans in v2 `PaneSnapshot`:

```protobuf
enum MouseMode {
  MOUSE_MODE_NONE = 0;       // default: no mouse tracking (also the unspecified state)
  MOUSE_MODE_X10 = 1;
  MOUSE_MODE_NORMAL = 2;
  MOUSE_MODE_BUTTON = 3;
  MOUSE_MODE_ANY = 4;
}

message TerminalModeState {
  bool bracketed_paste = 1;
  bool focus_reporting = 2;
  bool application_cursor_keys = 3;
  bool application_keypad = 4;
  bool alternate_screen = 5;
  bool cursor_hidden = 6;         // default false = cursor visible (correct terminal default)
  MouseMode mouse_mode = 7;
  bool sgr_mouse = 8;
}
```

- `PaneSnapshot` carries the full `TerminalModeState`.
- Live mode changes are emitted as `TerminalModeChanged` events with runtime ID, pane ID,
  `runtime_revision`, and the updated `TerminalModeState`.
- New modes can be added as new fields — missing fields default to false/zero per protobuf
  rules, which is the correct "mode not active" semantic.

```protobuf
message TerminalModeChanged {
  bytes runtime_id = 1;
  bytes pane_id = 2;
  uint64 runtime_revision = 3;
  TerminalModeState modes = 4;    // full state, not a diff
}
```

### 6. Terminal Output

```protobuf
message OutputDelta {
  bytes runtime_id = 1;
  bytes pane_id = 2;
  bytes data = 3;
  uint64 pane_output_seq = 4;
}
```

- Raw PTY bytes remain the primary output stream. VTE renders the terminal.
- `pane_output_seq` is a per-pane monotonic counter incremented by every delta. Used for
  output continuity detection (see Section 8).
- The daemon continues to parse output for mode state, title, CWD, and terminal responses
  (RFC-020). Parsed state changes are emitted as typed events (`TerminalModeChanged`,
  `TitleChanged`, `CwdChanged`).
- Terminal-generated replies (DA1, DA2, DSR, DECRQM) are synthesized by the daemon and
  stripped from client-bound output.

### 7. Revision Semantics

Two distinct revision spaces, clearly named:

#### Runtime revision (`runtime_revision: uint64`)

Bumped by structural changes to the runtime:
- Pane created, closed, exited
- Runtime renamed
- Terminal mode changed
- Client attached or detached

Carried on all runtime-level and pane-level events. Used for inventory freshness and resync
decisions.

#### Pane output sequence (`pane_output_seq: uint64`)

Per-pane monotonic counter, incremented by 1 for every `OutputDelta` message. Carried on
`OutputDelta` messages. Used only for output continuity detection. A gap in sequence numbers
(e.g. expected 5, received 7) indicates dropped messages.

On attach, the snapshot includes the current `pane_output_seq` for each pane. Subsequent
deltas increment from there. The server-sent `StreamOverflow` event is the primary signal
that messages were dropped. Client-side gap detection on `pane_output_seq` is a defensive
fallback for edge cases where the server drops a message but fails to send `StreamOverflow`
(e.g. the overflow event itself is dropped). If the client detects a gap and has `OPT_RESYNC`,
it requests resync.

No other revision spaces exist. Inventory and diagnostics are request/response and do not
need revisions.

### 8. Snapshots and Resync

#### Attach snapshot

On successful attach, the server sends a `RuntimeSnapshot`:

```protobuf
message RuntimeSnapshot {
  bytes runtime_id = 1;
  uint64 runtime_revision = 2;
  RuntimeClientRole client_role = 3;
  repeated PaneSnapshot panes = 4;
}

message PaneSnapshot {
  bytes pane_id = 1;
  uint64 pane_output_seq = 2;
  string title = 3;
  string cwd = 4;
  uint32 cols = 5;
  uint32 rows = 6;
  optional int32 exit_status = 7;
  TerminalModeState terminal_modes = 8;
  bytes scrollback_tail = 9;           // last N bytes (e.g. 256 KB)
  uint64 total_scrollback_bytes = 10;  // total available on daemon
  bool scrollback_complete = 11;       // true if scrollback_tail is everything
}
```

The client renders immediately from `scrollback_tail`. If the user scrolls up and
`scrollback_complete` is false, the client requests pages via `GetScrollback`
(requires `OPT_CHUNKED_SCROLLBACK`).

#### Chunked scrollback (OPT_CHUNKED_SCROLLBACK)

```protobuf
message GetScrollback {
  bytes runtime_id = 1;
  bytes pane_id = 2;
  uint64 offset = 3;       // byte offset from start of scrollback
  uint32 limit = 4;        // max bytes to return (server may cap)
}

message ScrollbackChunk {
  bytes runtime_id = 1;
  bytes pane_id = 2;
  uint64 offset = 3;       // byte offset this chunk starts at
  bytes data = 4;
  bool is_last = 5;        // true if this chunk reaches the current end
}
```

Rules:
- `GetScrollback` is a request/response command (uses `request_id`). Not a push stream.
- Each request returns a single `ScrollbackChunk` response. The client pages by adjusting
  `offset`.
- `limit` is capped by the server (e.g. 256 KB per chunk).
- Scrollback is append-only. Already-fetched pages remain valid as new output arrives.
- Without `OPT_CHUNKED_SCROLLBACK`, the client only has `scrollback_tail` from the snapshot.

#### Resync (OPT_RESYNC)

```protobuf
message ResyncRuntime {
  bytes runtime_id = 1;
}

message StreamOverflow {
  bytes runtime_id = 1;
  optional bytes pane_id = 2;  // absent if runtime-level overflow
  uint64 dropped_count = 3;   // approximate number of dropped messages
}
```

When the server's bounded push channel drops messages for a client, it sends `StreamOverflow`.
The server detects overflow at the point of `try_send` failure, not through client feedback.
The client requests `ResyncRuntime`, and the server responds with a fresh `RuntimeSnapshot`.

Without `OPT_RESYNC`, the server must not silently drop push messages. If the push channel is
full and the client does not support `OPT_RESYNC`, the server forcibly disconnects the client.
The client's connection state machine will reconnect and receive a fresh snapshot on reattach.
This is safe because the client already handles unexpected disconnections.

### 9. Endpoint Inventory

#### Inventory messages

```protobuf
message ListRuntimes {}

message RuntimeList {
  repeated RuntimeInfo runtimes = 1;
}

message RuntimeInfo {
  // Core fields (always populated):
  bytes id = 1;
  string name = 2;
  RuntimePolicy policy = 3;
  uint32 pane_count = 4;
  bool has_write_owner = 5;
  uint32 read_only_client_count = 6;
  RuntimeClientRole current_client_role = 7;
  uint64 runtime_revision = 8;
  bool reconstructed = 9;
  // OPT_RUNTIME_INVENTORY_V2 fields (empty/default when capability absent):
  string active_pane_summary = 10;
  bool takeover_eligible = 11;
  string disabled_reason = 12;    // empty when selectable
  repeated PaneInfo panes = 13;
}

message PaneInfo {
  bytes id = 1;
  string title = 2;
  string cwd = 3;
  uint32 cols = 4;
  uint32 rows = 5;
  optional int32 exit_status = 6;
  bool reconstructed = 7;
}
```

Busy runtimes are visible but disabled in the UI. The `disabled_reason` field explains why a
runtime cannot be selected (e.g. "owned by another client").

### 10. Ownership and Multi-Client Semantics

```protobuf
message AttachRuntime {
  bytes runtime_id = 1;
  RuntimeAttachMode attach_mode = 2;
}

enum RuntimeTerminationReason {
  RUNTIME_TERMINATION_REASON_UNSPECIFIED = 0;
  RUNTIME_TERMINATION_REASON_EXPLICIT = 1;          // client requested termination
  RUNTIME_TERMINATION_REASON_EPHEMERAL_DETACH = 2;  // last client detached from ephemeral runtime
}

message RuntimeTerminated {
  bytes runtime_id = 1;
  uint64 final_revision = 2;
  RuntimeTerminationReason reason = 3;
}

enum RuntimeAttachMode {
  RUNTIME_ATTACH_MODE_UNSPECIFIED = 0;
  RUNTIME_ATTACH_MODE_READ_WRITE = 1;
  RUNTIME_ATTACH_MODE_READ_ONLY = 2;
}

enum RuntimeClientRole {
  RUNTIME_CLIENT_ROLE_UNSPECIFIED = 0;
  RUNTIME_CLIENT_ROLE_UNATTACHED = 1;
  RUNTIME_CLIENT_ROLE_WRITER = 2;
  RUNTIME_CLIENT_ROLE_READER = 3;
}

enum RuntimePolicy {
  RUNTIME_POLICY_UNSPECIFIED = 0;
  RUNTIME_POLICY_PERSISTENT = 1;
  RUNTIME_POLICY_EPHEMERAL = 2;
}
```

```protobuf
message TakeoverWorkspace {
  bytes runtime_id = 1;
}

message TakeoverCompleted {
  bytes runtime_id = 1;
  uint64 workspace_revision = 2;
}

message LeaseLost {
  bytes runtime_id = 1;
  uint64 workspace_revision = 2;
  bytes new_owner_id = 3;
}

message OwnerDisconnected {
  bytes runtime_id = 1;
  uint64 workspace_revision = 2;
}
```

Rules:
- One writer lease per runtime, zero or more readers.
- Takeover is an explicit command (requires `OPT_RUNTIME_TAKEOVER`), not a side effect of
  attach.
- `AttachBlocked` is returned when a read-write attach fails due to an existing writer.
- Lease loss, owner disconnect, and forced takeover are typed events when
  `OPT_RUNTIME_TAKEOVER` is active.

#### Takeover semantics (as built)

Takeover shipped in #1090. The wire capability is spelled `OPT_WORKSPACE_TAKEOVER` in
`rttx-v3.proto` — the same value 101 as `OPT_RUNTIME_TAKEOVER` above, renamed by the
`Session`/`Runtime` → `Workspace` terminology pass. This section records the behavior the
daemon and client actually implement.

**Challenger flow.** Takeover is never implicit and never a retry of a failed attach:

1. The connect-existing dialog offers a "Take over" button for a runtime only when the
   daemon reports it as busy elsewhere *and* `takeover_eligible` is true
   (`OPT_RUNTIME_INVENTORY_V2`, Section 9). Without the takeover capability the row is
   simply disabled with its `disabled_reason`.
2. The button opens an `adw::AlertDialog` whose confirm response is styled
   `Destructive` and whose **default and close response is Cancel** — a stray Enter or
   Escape never steals someone else's workspace.
3. On confirm the client sends `TakeoverWorkspace { runtime_id }`, and only after
   `TakeoverCompleted` does it send `AttachWorkspace` in read-write mode. Seizing the
   lease first means the subsequent attach cannot lose a race against the previous owner.

**The previous owner is demoted, not disconnected.** This is the central semantic:

- The daemon moves the previous writer from `RUNTIME_CLIENT_ROLE_WRITER` to
  `RUNTIME_CLIENT_ROLE_READER`. It stays attached. Its pane widgets, scrollback, and
  window layout are untouched.
- It keeps receiving `OutputDelta`, `CwdChanged`, `TitleChanged`, and tree deltas, because
  push fan-out targets every attached client regardless of role. The demoted client
  therefore becomes a **live read-only mirror** of the workspace it just lost, which is
  what makes the handoff observable rather than merely destructive.
- The client maps `LeaseLost` to `ConnectionStatus::Blocked(ConnectionProblem::TakenOver)`
  — header label *"Action Required"*, detail *"Another client took over this
  workspace"* — and raises a toast so the demotion is not silent.
- Input is refused on **both** sides. Client-side, `ConnectionStatus::accepts_input()` is
  true only for `Connected` and `Recovered`, so a `Blocked` workspace sends nothing.
  Server-side, `handle_v3_terminal_input` drops input unless the sending client has write
  access, as do all structural commands (`SplitPane`, `ClosePane`, `ResizeSplit`,
  `RenameWorkspace`, …), which return `ERROR_KIND_OWNERSHIP_CONFLICT`. The single-writer
  invariant therefore holds even against a stale or racing client that has not yet
  processed its `LeaseLost`.

**Ordering.** The daemon pushes `LeaseLost` to the demoted writer **before** it returns
`TakeoverCompleted` to the challenger. The client that is losing input always learns why
first; the client that gains it is told only once the demotion is on the wire.

**Non-transience.** `ConnectionProblem::TakenOver` is not transient: `is_transient()`
matches only `DaemonUnavailable`, so a taken-over workspace is never auto-retried. Without
this rule two clients would trade the lease back and forth indefinitely. Recovery is
manual and symmetric — the demoted user either keeps watching the mirror, retries the
connection explicitly, or takes the workspace back through the same confirmed takeover
flow.

**Guards.** `TakeoverWorkspace` is rejected with `ERROR_KIND_OWNERSHIP_CONFLICT` when:

| Condition | Why |
|---|---|
| The runtime has no current write owner | An ordinary read-write attach already succeeds; takeover would be a needlessly destructive way to ask. |
| The requester already holds the lease | There is nothing to seize, and a bumped revision would report a handoff that did not happen. |
| The runtime policy is `EPHEMERAL` | An ephemeral runtime lives only as long as its clients stay attached, so handing it over would transfer a runtime already on its way out. |

The same three conditions drive `takeover_eligible` in `RuntimeInfo`, so an ineligible
runtime never shows the button in the first place; the server-side check exists because a
client's inventory can be stale.

`OwnerDisconnected` is reserved for the unforced case — the writer leaving on its own, so
readers know the lease is free. The message and its builders exist on the wire, but the
daemon does not emit it yet: a reader currently discovers the free lease by retrying its
attach. Emitting it is a follow-up, not a protocol change.

### 11. Error Model

```protobuf
enum ErrorKind {
  ERROR_KIND_UNSPECIFIED = 0;
  ERROR_KIND_PROTOCOL_MISMATCH = 1;
  ERROR_KIND_UNSUPPORTED_CAPABILITY = 2;
  ERROR_KIND_INVALID_ARGUMENT = 3;
  ERROR_KIND_RUNTIME_NOT_FOUND = 4;
  ERROR_KIND_PANE_NOT_FOUND = 5;
  ERROR_KIND_OWNERSHIP_CONFLICT = 6;
  ERROR_KIND_TAKEOVER_REQUIRED = 7;
  ERROR_KIND_STREAM_OVERFLOW = 8;
  ERROR_KIND_INTERNAL = 9;
}

message ProtocolError {
  ErrorKind kind = 1;
  string message = 2;
  string operation = 3;
  bool retryable = 4;
  bool user_action_required = 5;
  uint32 retry_after_seconds = 6; // suggested backoff for retryable errors; 0 = no hint
}
```

Rules:
- Error kinds are append-only after 1.0. New kinds can be added; existing kinds cannot be
  removed or have their semantics changed.
- Clients must handle unknown error kinds by falling back to a generic error display.
- The client maps typed errors to `ConnectionProblem` and UI policy without string matching.

### 12. Naming

All v3 messages use product terminology:

| Concept | Wire name | Notes |
|---|---|---|
| Daemon-owned backend | `Runtime` | Not "Session" |
| Terminal tile | `Pane` | |
| Local or remote daemon | Endpoint | Client-side concept; not a wire identity |
| GUI tab | Workspace | Client-side only; never appears in daemon protocol |

### 13. Wire Compatibility Rules

These rules are binding after the initial v3 release and govern all subsequent protocol changes.

**Additive changes (no version bump required):**
- New fields added to existing messages (missing fields decode as zero/empty/false per protobuf
  defaults)
- New variants added to `oneof` fields (unknown variants are ignored by older implementations)
- New enum values added (unknown values decode as 0/unspecified; receivers must handle unknown
  values gracefully)
- New optional capabilities defined
- New message types added to the envelope `oneof`

**Prohibited changes (require a new major protocol version):**
- Removing or renumbering existing fields (use `reserved` for deprecated fields)
- Removing `oneof` variants or enum values
- Changing the semantic meaning of an existing field
- Changing a core capability's behavior
- Changing the framing format (4-byte little-endian length prefix)

**Send discipline:**
- The daemon must not send messages, events, or populate fields that require a capability the
  connected client did not negotiate. Receivers should still handle unknown variants and fields
  gracefully (ignore unknown `oneof` variants, treat unknown enum values as zero) as a
  defensive measure, but the sender is responsible for not producing them.
- The client must not send commands that require a capability the daemon did not advertise.

**Capability promotion:**
- A capability may be promoted from optional to core only in a new major protocol version.
- Within a major version, all capabilities that were optional at release remain optional.

**Error kind stability:**
- Error kinds are append-only. New kinds can be added at any time. Existing kinds cannot be
  removed or have their semantics changed.
- Clients must handle unknown error kinds gracefully.

**Enum zero values:**
- Every enum must have a zero value that represents the default or absent state.
- For most enums this is `_UNSPECIFIED = 0` (meaning "not set / unknown").
- For enums where the default state is semantically meaningful (e.g. `MouseMode` where the
  default is "no tracking"), the zero value may carry that meaning directly
  (e.g. `MOUSE_MODE_NONE = 0`).
- Receivers must handle unknown enum values gracefully by treating them as the zero value.

### 14. Protocol Versioning

- **Major version** (the negotiated `protocol_version`): bumped only when a change cannot be
  expressed as an additive protobuf extension, or when an optional capability is promoted to
  core.
- **No minor version on the wire.** Within a major version, all changes are additive and handled
  by protobuf's native forward/backward compatibility. Capabilities serve as the
  feature-detection mechanism.
- **Version negotiation**: Client sends `min_protocol_version` and `max_protocol_version`.
  Server picks the highest it supports. If no overlap, `ProtocolError` with kind
  `PROTOCOL_MISMATCH`.
- **Lifetime commitment**: The lifetime commitment protects daemon operators. A current GUI
  must connect to any daemon within the support window. Once a major version is released as
  stable, the server must support it for at least 2 major versions forward. When v4 ships, v3
  is still supported. When v5 ships, v3 can be dropped. Old GUIs connecting to new daemons
  are not a supported configuration — update the GUI.

### 15. Transport

No changes to the transport layer. v3 uses the same framing as v2:

- 4-byte little-endian length prefix followed by a protobuf-encoded `ClientEnvelope` or
  `ServerEnvelope`.
- `MAX_MESSAGE_SIZE` remains 16 MB.
- Local: Unix socket at `$XDG_RUNTIME_DIR/rttx-server/rttx.sock`.
- Remote: `ssh <host> rttx-server attach-stdio` over stdin/stdout.

The socket path does not contain a protocol version — version negotiation happens inside the
handshake, not at the transport level.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 | Version range negotiation + core/optional capabilities |
| G2 | Six protocol domains with clear command/event boundaries |
| G3 | Consolidated `TerminalModeState` + `TerminalModeChanged` events |
| G4 | `runtime_revision` + `pane_output_seq` + `ResyncRuntime` + chunked scrollback |
| G5 | `RuntimeInfo` with ownership and takeover metadata |
| G6 | Same transports: Unix socket and SSH stdio |
| G7 | Request IDs enable deterministic test assertions; typed events are directly matchable |
| G8 | Wire compatibility rules section (Section 13) |

---

## Development Plan

- [ ] **Step 1** — Rename all `Session*` types to `Runtime*` across codebase (do this first so
  all new v3 code uses the correct terminology from the start)
- [ ] **Step 2** — Define the complete `.proto` file for v3 (the envelope references many
  messages — `CreateRuntime`, `AttachRuntime`, `PaneCreated`, `Ping`, `Pong`, etc. — that this
  RFC defines structurally but not field-by-field; the `.proto` file is the authoritative
  definition)
- [ ] **Step 3** — Implement v3 handshake with version negotiation and capability advertisement
- [ ] **Step 4** — Implement `ClientEnvelope`/`ServerEnvelope` with request/response correlation
- [ ] **Step 5** — Implement `TerminalModeState` consolidation and `TerminalModeChanged` events
- [ ] **Step 6** — Implement `PasteInput` and `FocusInput` as core structured input
- [ ] **Step 7** — Implement `ProtocolError` with typed error kinds
- [ ] **Step 8** — Implement `RuntimeSnapshot` with `scrollback_tail` and `pane_output_seq`
- [ ] **Step 9** — Implement `OPT_CHUNKED_SCROLLBACK` (`GetScrollback`/`ScrollbackChunk`)
- [ ] **Step 10** — Implement `OPT_RESYNC` (`StreamOverflow`/`ResyncRuntime`)
- [ ] **Step 11** — Implement `OPT_RUNTIME_INVENTORY_V2` (rich inventory fields)
- [x] **Step 12** — Implement `OPT_RUNTIME_TAKEOVER` (explicit takeover and lease events)
  — shipped as `OPT_WORKSPACE_TAKEOVER` with `TakeoverWorkspace`, `TakeoverCompleted`, and
  `LeaseLost`; semantics recorded in Section 10. `OwnerDisconnected` is defined but not
  yet emitted.
- [ ] **Step 13** — Implement `OPT_DIAGNOSTICS`
- [ ] **Step 14** — Remove all v2 protocol code
- [ ] **Step 15** — Full integration test suite for v3 protocol. Test matrix: latest GUI
  against each supported daemon capability profile (all-core-only, core+individual-optional,
  core+all-optional), not arbitrary version combinations.

---

## Open Questions

- [ ] **Q1** — How much keyboard encoding should live in the daemon versus the client? The
  current implementation keeps keyboard encoding in the client (VTE commit path). The `oneof`
  in `TerminalInput` allows adding `KeyInput` later if a concrete need arises.
- [ ] **Q2** — Should terminal mode tracking grow beyond the current `ScreenPerformer` scope?
  `alternate_screen` and Kitty keyboard protocol are not yet tracked. They can be added as new
  fields on `TerminalModeState` without a wire break.
- [ ] **Q3** — Should remote daemons expose stable host identity for inventory and
  troubleshooting, or should endpoint metadata remain entirely client-owned?

---

## References

- [RFC-013: Persistent Host Sessions](RFC-013-persistent-host-sessions.md) *(Implemented)*
- [RFC-016: Workspace Management v2](RFC-016-workspace-management-v2.md) *(Implemented)*
- [RFC-018: Workspace Connection State Machine](RFC-018-workspace-connection-state-machine.md) *(Implemented)*
- [RFC-019: Missing Session Handling](RFC-019-missing-session-handling.md) *(Accepted, partially implemented)*
- [RFC-020: Terminal Response Ownership](RFC-020-terminal-response-ownership.md) *(Implemented)*
- [Issue #362: bounded push channel resync](https://github.com/IllyaYalovyy/rttx/issues/362)
- [Issue #493: RFC tracking issue for protocol v3](https://github.com/IllyaYalovyy/rttx/issues/493)
