# RFC-026: Pane State Clone on Split

| Field         | Value         |
|---------------|---------------|
| Status        | Accepted      |
| Author(s)     | Illya Yalovyy |
| Supersedes    | —             |
| Superseded by | —             |

---

## Summary

When a user splits a pane, the new pane must be a **state clone** of the parent
pane: same working directory, same shell history, same environment, same terminal
mode flags. Today the daemon has no concept of a parent pane — `CreatePane` is a
blank-slate operation that guesses the CWD from an arbitrary existing pane. This
RFC replaces `CreatePane`-as-split with an explicit `ClonePane` command that
copies the parent pane's full recoverable state into the new pane before spawning
its shell.

---

## Goals

- **G1** — A split pane starts in the parent pane's current working directory,
  always, regardless of OSC 7 reporting or client-side CWD tracking.
- **G2** — A split pane inherits the parent pane's shell history file so the user
  has the same command recall in both panes.
- **G3** — A split pane inherits the parent pane's environment variables
  (`COLORFGBG`, user-set vars) so visual and behavioral context is preserved.
- **G4** — A split pane inherits the parent pane's terminal mode state (bracketed
  paste, application cursor keys, mouse tracking, etc.) so the client can
  configure the new terminal widget correctly before the first output arrives.
- **G5** — The daemon is the single source of truth for parent pane state. The
  client does not need to track or transmit CWD, history, or modes — it sends
  the parent pane ID and the daemon does the rest.

## Non-Goals

- **NG1** — Sharing live scrollback between parent and child panes. Each pane
  gets an independent scrollback log from the moment of the split.
- **NG2** — Sharing a PTY or shell process. The child pane gets a new shell
  process; only the initial state is cloned.
- **NG3** — Cloning the screen content (visible viewport). The child pane starts
  with a fresh shell prompt.
- **NG4** — Backward compatibility with the current `CreatePane` message for
  split operations. The client must use `ClonePane` for splits after this RFC
  lands.

---

## Background & Motivation

### The split contract users expect

Every major terminal multiplexer (tmux, Tilix, Terminator, iTerm2) treats split
as "fork the current context." The new pane starts in the same directory, with
the same history, in the same visual mode. Users do not think of split as
"create a blank terminal somewhere nearby" — they think of it as "give me
another view into the same working context."

rttx has always intended this behavior. RFC-007 (Session Recovery) describes
per-pane recovery recipes that reconstruct "what the user was doing." RFC-013
(Persistent Host Sessions) established that the daemon owns all pane state. The
missing piece is that the split operation never tells the daemon *which* pane is
the parent.

### Current behavior and the bug

The `CreatePane` protocol message has an optional `cwd` field and no parent
reference:

```protobuf
message CreatePane {
  bytes runtime_id = 1;
  optional string cwd = 2;
  optional bool dark_background = 3;
  uint32 cols = 4;
  uint32 rows = 5;
  optional bool no_persist = 6;
}
```

The client attempts to read the parent pane's CWD from the widget or layout
node and pass it in `cwd`. When the CWD is unknown (OSC 7 not yet received,
shell doesn't emit it, or the widget hasn't been updated), the client sends
`cwd: None`. The daemon then falls back to `rt.any_pane_cwd()`:

```rust
let cwd = req.cwd.or_else(|| rt.any_pane_cwd());
```

`any_pane_cwd()` calls `HashMap::values().find_map(Pane::effective_cwd)` —
which returns the CWD of whichever pane the hash map iterator visits first.
With multiple panes, this is effectively random and usually wrong.

**Result**: splitting pane 2 can spawn the new pane in pane 1's directory.

Even when the client does have the CWD, the current design has structural
problems:

1. **CWD is the only state transferred.** History, environment, and terminal
   modes are not cloned. The new pane starts with an empty HISTFILE and default
   terminal modes.

2. **The client is the wrong source of truth.** The daemon owns the PTY and can
   always read `/proc/<pid>/cwd`. The client's CWD knowledge depends on OSC 7
   reporting, which is shell-dependent and asynchronous. Making the client
   responsible for transmitting the CWD is a reliability inversion.

3. **No parent identity in the protocol.** The daemon cannot look up the parent
   pane's state because `CreatePane` doesn't say which pane is being split.

### What the daemon already knows

The daemon tracks per-pane state that is sufficient for a full clone:

| State | Source | Available |
|---|---|---|
| Working directory | OSC 7 or `/proc/<pid>/cwd` | Always (proc fallback) |
| Shell history file | `$XDG_STATE_HOME/rttx/daemon/runtimes/<id>/history/<pane>.hist` | On disk |
| Terminal modes | `PaneScreen` (bracketed paste, app cursor, mouse, etc.) | In memory |
| Environment | Not tracked after spawn | ❌ — see Design §3 |
| Title | `Pane.title` | In memory |
| `no_persist` flag | `Pane.no_persist` | In memory |

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Split panes start in the correct directory with full command history and matching terminal behavior. No more "wrong directory" or "empty history" after split. |
| Contributors | `ClonePane` is a single daemon-side operation; the client no longer needs to track or transmit parent pane state for splits. |
| Packagers    | No packaging changes. |

---

## Considered Options

### Option A — Add `parent_pane_id` to `CreatePane`

Extend the existing message with an optional parent reference. When present, the
daemon reads the parent's CWD and copies its history file.

**Pros**: Minimal protocol change. No new message type.

**Cons**: `CreatePane` becomes overloaded — sometimes it's a blank-slate
creation, sometimes it's a clone. The "clone" semantics are implicit and easy to
get wrong. Terminal modes and environment are awkward to add as optional clone
behavior on a creation message.

### Option B — New `ClonePane` command

A dedicated command that explicitly means "fork this pane's state into a new
pane." The daemon handles all state cloning internally.

**Pros**: Clear semantics. The daemon is the single source of truth. Easy to
extend with additional cloned state in the future. `CreatePane` stays clean for
blank-slate creation (workspace creation, reconciliation, recovery).

**Cons**: New protocol message. Two ways to create a pane.

### Option C — Client-side clone (status quo with fixes)

Keep `CreatePane`, but have the client query the daemon for the parent pane's
full state before sending the creation request.

**Pros**: No protocol change.

**Cons**: Requires a new query message anyway. Round-trip latency. The client
becomes responsible for assembling state that the daemon already has. Race
conditions between query and creation.

---

## Decision

**Chosen option: Option B — New `ClonePane` command.**

Rationale: the daemon owns all pane state. A split is a state-cloning operation.
The protocol should express that directly rather than overloading a blank-slate
creation message. `CreatePane` remains the right tool for initial workspace
creation, recovery, and reconciliation where there is no parent pane.

---

## Design

### 1. Protocol: `ClonePane` command

```protobuf
message ClonePane {
  bytes runtime_id = 1;
  bytes parent_pane_id = 2;
  uint32 cols = 3;
  uint32 rows = 4;
}
```

Response: `PaneCreated` (same as `CreatePane`) or `ProtocolError`.

The daemon handles all state cloning. The client sends only the parent pane ID
and the desired terminal size. No CWD, no environment, no modes — the daemon
has all of it.

`dark_background` and `no_persist` are inherited from the parent pane. The
client does not need to re-specify them.

### 2. Daemon: clone behavior

When the daemon receives `ClonePane`:

1. **Resolve parent pane.** Look up `parent_pane_id` in the runtime. If not
   found, return `ProtocolError`.

2. **Read CWD.** Use `parent_pane.effective_cwd()` — this checks OSC 7 first,
   then falls back to `/proc/<pid>/cwd`. This is always accurate regardless of
   shell OSC 7 support. If the parent pane's process has exited
   (`exit_status` is `Some`), `/proc/<pid>/cwd` will not be available; use the
   last known `pane.cwd` from OSC 7. If neither is available, fall back to
   `$HOME`.

3. **Copy history file.** Copy the parent pane's `.hist` file to the new pane's
   history path. If the parent has `no_persist`, skip this step. After the copy,
   the two panes' histories diverge independently.

4. **Read terminal modes.** Snapshot the parent pane's `PaneScreen` mode flags:
   bracketed paste, application cursor keys, application keypad, mouse tracking
   mode, SGR mouse, focus events, cursor visibility. These are included in the
   `PaneCreated` response so the client can configure the new terminal widget
   before the first output arrives.

5. **Inherit `no_persist`.** The child pane inherits the parent's `no_persist`
   flag. A `no_persist` pane's split should also be `no_persist`.

6. **Inherit `COLORFGBG`.** Read from the parent pane's spawn environment (see
   §3) so the child pane matches the parent's light/dark setting.

7. **Spawn shell.** Create a new PTY with the cloned CWD, copied HISTFILE,
   inherited environment, and the requested terminal size. The shell is a new
   process — no PTY sharing.

8. **Apply terminal modes to the child PTY.** After the shell starts, send
   escape sequences to the new PTY to restore the parent's terminal mode state
   (bracketed paste enable, application cursor keys, etc.). This ensures the
   shell sees the correct terminal capabilities from the start. The
   `alternate_screen` flag is **not** applied — it is a property of the running
   application (e.g., vim), not the terminal configuration, and a fresh shell
   should never start in alternate screen mode. The client independently applies
   `inherited_modes` from the `PaneCreated` response to configure its VTE widget
   (see §4). Both sides act: the daemon sets PTY state so the shell is correct;
   the client sets widget state so rendering is correct.

### 3. Environment tracking

The daemon currently does not track the full environment after spawn. For
`ClonePane`, the daemon needs to reproduce the parent pane's spawn-time
environment variables (`COLORFGBG`, `HISTFILE`, and any future per-pane vars).

Add a `spawn_env: Vec<(String, String)>` field to `Pane` that records the
environment variables set at spawn time. `ClonePane` copies this list (with
`HISTFILE` updated to the new pane's path).

This is not a full `/proc/<pid>/environ` clone — it only covers variables the
daemon explicitly set. Shell-internal variables (`PS1`, aliases, functions) are
not cloned because they live inside the shell process, not the PTY environment.

`spawn_env` must be persisted in `PaneSpecV1` (RFC-022) so that reconstructed
panes after a daemon restart can still be cloned correctly.

### 4. `PaneCreated` response extension

The `PaneCreated` response gains a new field for the parent's terminal mode
state so the client can configure the new widget before output arrives:

```protobuf
message PaneCreated {
  bytes runtime_id = 1;
  bytes pane_id = 2;
  uint64 runtime_revision = 3;
  optional TerminalModeState inherited_modes = 4;  // new — present only for ClonePane
}
```

`TerminalModeState` already exists in the v3 proto for `PaneSnapshot`. Reuse it.

When `inherited_modes` is present (i.e., the pane was created via `ClonePane`),
the client applies the modes to the new `PersistentPaneView` immediately, before
any `OutputDelta` arrives. This prevents a flash of incorrect behavior (e.g.,
bracketed paste disabled for a moment in a pane that should have it enabled).

For `CreatePane` responses, `inherited_modes` is absent.

### 5. Client changes

The client split path (`window/terminal.rs:split_terminal`) changes from:

```rust
// Before: client guesses CWD, sends CreatePane
let source_cwd = terminal_cwd.or_else(|| layout.terminal_cwd(terminal_uuid));
manager.create_pane(..., source_cwd, ...);
```

To:

```rust
// After: client sends ClonePane with parent pane ID
let parent_pane_id = session_state.runtime.pane_bindings.get(terminal_uuid);
manager.clone_pane(..., parent_pane_id, cols, rows);
```

The client no longer needs to read, track, or transmit the parent pane's CWD
for split operations. The daemon is the single source of truth.

`CreatePane` remains for:
- Initial pane creation when a workspace is first created (no parent)
- Reconciliation when the daemon has panes the GUI doesn't know about
- Recovery when restoring a workspace from persisted state

### 6. `CreatePane` cleanup

Remove the `any_pane_cwd()` fallback from `CreatePane`. When `CreatePane`
arrives without a CWD, the daemon spawns the shell in the user's home directory
(the default). The "guess a CWD from some other pane" behavior was a workaround
for the missing parent reference and is no longer needed.

After this change:
- `CreatePane` with `cwd` → spawn in that directory
- `CreatePane` without `cwd` → spawn in `$HOME`
- `ClonePane` → spawn in parent pane's directory (always accurate)

### 7. Remote endpoints

`ClonePane` works identically for local and remote daemons — the daemon on the
remote host performs all cloning locally. The `/proc/<pid>/cwd` fallback is
Linux-specific; on remote hosts running a different OS, the daemon relies on
OSC 7 for CWD (with `$HOME` as the final fallback). This is acceptable because
rttx targets Linux (RFC-001 principle 1).

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 — Correct CWD | §2 step 2: daemon reads parent's `effective_cwd()` which always works via `/proc` fallback |
| G2 — History clone | §2 step 3: daemon copies parent's `.hist` file before spawning the child shell |
| G3 — Environment | §3: daemon records spawn-time env vars per pane; `ClonePane` copies them |
| G4 — Terminal modes | §2 steps 4 and 8: daemon snapshots parent modes and applies them to the child PTY; §4: modes sent to client in `PaneCreated` |
| G5 — Daemon as source of truth | §5: client sends only parent pane ID; daemon handles all state cloning |

---

## Development Plan

- [ ] **Step 1** — Add `spawn_env` field to `Pane` and `PaneSpecV1`; record
  environment at spawn time; persist and restore on daemon restart. Tests:
  round-trip serialization, reconstructed pane retains `spawn_env`.
  *(prerequisite: this RFC accepted)*
- [ ] **Step 2** — Add `ClonePane` message to `rttx-v3.proto` and extend
  `PaneCreated` with `inherited_modes`. Tests: proto round-trip, field presence
  for `ClonePane` vs `CreatePane` responses. *(prerequisite: Step 1)*
- [ ] **Step 3** — Implement `ClonePane` handler in the daemon: CWD resolution
  (including exited parent fallback), history copy, env clone, mode snapshot,
  shell spawn, mode application (excluding `alternate_screen`). Tests: CWD
  clone with multiple panes, history file copy, mode inheritance, `no_persist`
  inheritance, exited parent pane fallback, parent not found error.
  *(prerequisite: Step 2)*
- [ ] **Step 4** — Update client split path to use `ClonePane` instead of
  `CreatePane`; apply `inherited_modes` on the new widget. Tests: GTK split
  test verifying `ClonePane` is sent with correct parent pane ID.
  *(prerequisite: Step 3)*
- [ ] **Step 5** — Remove `any_pane_cwd()` fallback from `CreatePane` handler;
  default to `$HOME` when no CWD is provided. Tests: `CreatePane` without CWD
  defaults to `$HOME`, end-to-end split with multiple panes verifying correct
  CWD. *(prerequisite: Step 4)*

---

## Open Questions

- [x] **Q1** — Should `ClonePane` support an optional CWD override?
  **Resolved:** no. Use `CreatePane` with an explicit CWD for that case.
  `ClonePane` means "exact state clone."
- [x] **Q2** — Should the daemon apply terminal modes via escape sequences to
  the child PTY (§2 step 8), or should the client be responsible for sending
  mode-setting sequences after receiving `inherited_modes`? **Resolved:** both.
  The daemon applies modes to the PTY so the shell sees correct capabilities
  from the start. The client applies `inherited_modes` from `PaneCreated` to
  configure its VTE widget so rendering is correct before the first output.
  `alternate_screen` is excluded from PTY-side application (see §2 step 8).

---

## References

- [#687](https://github.com/IllyaYalovyy/rttx/issues/687) — feat: copy parent
  pane HISTFILE on split (subsumed by this RFC)
- [#773](https://github.com/IllyaYalovyy/rttx/issues/773) — fix: new pane must
  inherit parent pane's CWD (the bug that motivated this RFC)
- [#774](https://github.com/IllyaYalovyy/rttx/issues/774) — fix: `any_pane_cwd`
  workaround (insufficient — picks arbitrary pane, not parent)
- [RFC-007: Session Recovery](./RFC-007-session-recovery.md) — per-pane recovery
  recipes; split clone is the runtime-time equivalent
- [RFC-013: Persistent Host Sessions](./RFC-013-persistent-host-sessions.md) —
  daemon owns all pane state; this RFC makes split honor that ownership
- [RFC-021: Client/Server Protocol v3](./RFC-021-client-server-protocol-v3.md) —
  `CreatePane` and `PaneCreated` message definitions
- [RFC-022: Daemon State Storage](./RFC-022-daemon-state-storage.md) — per-pane
  history files at `runtimes/<id>/history/<pane>.hist`
- `services/rttx-server/src/server.rs` — `CreatePane` handler, `any_pane_cwd()`
- `services/rttx-server/src/pane.rs` — `Pane` struct, `effective_cwd()`,
  `read_proc_cwd()`
- `services/rttx-server/src/screen.rs` — `PaneScreen`, terminal mode tracking
- `clients/rttx/src/window/terminal.rs` — `split_terminal()` client-side split
- `protocols/rttx-proto/proto/rttx-v3.proto` — `CreatePane`, `PaneCreated`
