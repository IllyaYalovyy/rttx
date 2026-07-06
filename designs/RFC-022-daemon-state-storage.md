# RFC-022: Daemon State Storage

| Field         | Value          |
|---------------|----------------|
| Status        | Implemented    |
| Author(s)     | Illya Yalovyy  |
| Supersedes    | —              |
| Superseded by | —              |

---

## Summary

The rttx daemon persists every runtime's state into a single monolithic
`state.json` file, rewritten in full on every serialization tick, stored under the
user's XDG **cache** directory. Scrollback and shell history are written as raw
per-pane files next to it. There is no schema version, no backup, no
dirty-tracking, and no separation between durable spec and transient runtime
instance data.

This works for the tens-of-runtimes case today. It does not scale, does not survive
cache eviction, and makes every future persistence-adjacent feature (encrypted
scrollback, per-host state, cross-machine sync, crash-safe recovery, selective
export) a load-bearing rewrite rather than an additive change.

This RFC proposes a clean v2 layout: a **per-runtime directory with versioned, typed
files under `$XDG_STATE_HOME/rttx/daemon/`**, a small top-level index, explicit
schema versioning, dirty-flag-driven writes, deterministic screen snapshots separate
from append-only scrollback logs, and a durable-vs-ephemeral split.

There is no migration from v1. On first v2 startup the daemon starts with a clean
state. Users get a loud log line explaining the change.

---

## Goals

- **G1** — State that survives OS/user cache cleanup (move out of `XDG_CACHE_HOME`)
- **G2** — Per-runtime files so write cost scales with churn, not with total runtime count
- **G3** — Explicit `schema_version` and a typed migration path on every persisted struct
- **G4** — A single corrupt file never takes down the whole daemon's history
- **G5** — Clean separation between *durable* state (id, name, policy, layout) and
  *transient* runtime state (revision, attached_clients, pending_replies)
- **G6** — Reconnect snapshots are reconstructed from a deterministic screen model,
  not raw byte tails that may start mid-escape-sequence
- **G7** — Pruning of removed runtimes' on-disk artefacts is automatic
- **G8** — The layout is extensible to future features (encrypted panes, per-host
  stores, sync to relay) without another flag-day migration

## Non-Goals

- **NG1** — Encryption at rest. Designed-for but not delivered in this RFC
- **NG2** — Cross-daemon replication or a cloud sync protocol (future RFC; this
  RFC makes it possible, not present)
- **NG3** — Replacing JSON with a binary format. We stay on JSON for diff-ability
  and hand-editability; binary can be swapped per-file later
- **NG4** — Backward compatibility with the v1 `state.json` layout. There is no
  migration: v2 starts clean. The old cache directory is left untouched for manual
  inspection but never read
- **NG5** — A new IPC surface for state inspection — this RFC concerns on-disk
  storage only

---

## Background & Motivation

### Current layout

```
$XDG_CACHE_HOME/rttx-server/
├── state.json                              # all runtimes, rewritten every tick
├── scrollback/<runtime_id>/<pane_id>.log   # raw bytes, tail-truncated at 10MB
└── history/<runtime_id>/<pane_id>.hist     # per-pane shell history
```

`state.json` contains a single `ServerState { runtimes, serialized_at, server_version }`.
See `services/rttx-server/src/serialization.rs` and `runtime.rs:PersistedRuntime`.

In dev mode (`RTTX_DEV_MODE=1`), the daemon uses `$XDG_CACHE_HOME/rttx-server-devel/`
instead, so development and production state are fully isolated.

### Pain points observed today

1. **Cache directory is the wrong home.** `XDG_CACHE_HOME` is explicitly defined as
   *data the user can regenerate or delete*. Distro cleaners, Flatpak refresh, and
   `systemd-tmpfiles` can all wipe it. Our runtimes are not regenerable.

2. **Monolithic rewrite.** Every serialization tick builds a full snapshot under
   the server mutex, pretty-prints it, writes to `.tmp`, renames over the old file
   (see `serialization_loop` in `server.rs` and `write_state_atomic` in
   `serialization.rs`). Cost grows linearly with total runtime count even if
   only one runtime actually changed.

3. **No schema version.** The only version marker is `server_version: String` —
   the Cargo package version. There is no `schema_version`, no migration entry
   point, and `#[serde(default)]` is used ad-hoc per field (e.g., on `policy`
   and `revision` in `PersistedRuntime`). A non-trivial schema change today
   requires bespoke logic scattered across struct definitions.

4. **Scrollback truncation corrupts replay.** `Pane::flush_scrollback` appends
   pending bytes to the scrollback log, then calls `truncate_log_tail` when the
   file exceeds `DEFAULT_MAX_SCROLLBACK_LOG` (10 MB). The truncation reads the
   full file, slices off the head at a raw byte boundary, and rewrites. The kept
   tail may start mid-escape-sequence, producing visible garbage when replayed.
   `MAX_SNAPSHOT_BYTES = 256 KB` (defined in `pane.rs`) masks but does not fix
   this.

5. **Durable and transient are mixed.** `PersistedRuntime` holds `revision` and
   `last_active_at` alongside `policy`, `panes`, `command_history`, and
   `active_pane_id`, but not `attached_clients` (which is correctly ephemeral).
   The boundary is implicit and easy to get wrong on the next field.

6. **No cleanup.** When a runtime is deleted, its scrollback and history
   directories remain on disk. Over months of use the cache grows unboundedly.

7. **Corruption is fatal to the entire history.** `load_state` returns
   `Err(InvalidData)` on a bad parse, logs it, and the daemon starts empty. One
   bad byte in the monolith loses everything.

8. **No write coalescing or dirty tracking.** A runtime that hasn't changed in
   an hour is rewritten identically 3 600+ times. `Runtime` has a `revision: u64`
   field that bumps on every mutation, but there is no `persisted_revision` to
   compare against.

### Current screen model

`PaneScreen` stores raw PTY bytes in a `Vec<u8>` and tracks cursor position,
title, CWD, and terminal mode flags via a VTE parser. The code explicitly notes
that a full cell-grid model is deferred: snapshots replay raw bytes to
reconstruct state. This is the foundation for the screen-snapshot proposal in
§4.

### Why this is worth an RFC, not a patch

The fixes cut across:
- Directory layout (breaking)
- Serde schema (breaking)
- Screen-state model (new abstraction — serialized grid vs raw bytes)
- Cleanup lifecycle (new scheduler work)

…and they interlock: you cannot introduce per-runtime files without a
schema-version story; you cannot make scrollback replay correct without a
screen-model serialization; you cannot move to `$XDG_STATE_HOME` without also
deciding the directory namespace shared with RFC-023. This needs one consistent
design.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Runtimes survive cache cleanup; corruption is contained to one runtime, not the whole daemon; reconnect shows the correct screen, not garbled bytes. Existing sessions are lost on upgrade (clean start) |
| Contributors | Adding a field to a persisted struct is a one-line schema bump with a typed migration; per-runtime files mean smaller diffs and faster tests |
| Packagers    | State moves to `$XDG_STATE_HOME/rttx/daemon/`; packaging docs updated; no cache-cleaner collateral damage |

---

## Considered Options

### Option A — Keep monolithic `state.json`, add `schema_version` only

**Pros**: Smallest change. Only adds a version field and a migration hook.

**Cons**: Does nothing for cache-eviction, write scaling, corruption blast radius,
or dead-runtime pruning. We will be back here within a release.

### Option B — Per-runtime directory, versioned files, durable/transient split

**Pros**: Addresses every pain point listed. Each runtime is its own unit of
durability, corruption, and migration. Diffs are tractable.

**Cons**: Breaking layout change. More files on disk (trivial; filesystems handle
this fine — tmux/zellij already do).

### Option C — Embedded SQLite

**Pros**: Transactional. Single file. Battle-tested. Easy to query from tooling.

**Cons**: Binary file is not diff-able or hand-editable; adds a non-trivial
dependency; the concurrency model (single writer, many readers) gives us no
benefit over per-runtime JSON since the daemon is already the sole writer.
Scrollback as BLOBs in SQLite is measurably worse than append-only log files.

### Option D — Event-sourced log + periodic snapshot

**Pros**: Crash-consistent by construction; trivially replicable; easy to export
recent history.

**Cons**: Operational complexity (compaction, snapshot cadence, log GC) that
materially exceeds the problem we have. Good target for a *later* RFC once we
have replication requirements.

---

## Decision

**Chosen option: Option B — per-runtime versioned files with a clean durable/transient split.**

Rationale:

- It is the minimum change that addresses every pain point simultaneously
- Stays on JSON, so diffs, review, and manual recovery all keep working
- Leaves the door open to swap specific files (screen snapshot, scrollback) to
  binary later without changing the layout
- Does not preclude Option D as a future addition: the per-runtime file becomes
  the "snapshot" in an event-sourced model

There is no v1 migration. On first v2 startup the daemon starts fresh with a
clean state directory. The old `$XDG_CACHE_HOME/rttx-server/` is left untouched
for manual inspection. A loud log line announces the change.

---

## Design

### 1. On-disk layout

```
$XDG_STATE_HOME/rttx/                       # shared root (daemon + client)
└── daemon/                                  # RFC-022 owns everything below
    ├── daemon.json                          # server-level index, schema v1
    ├── daemon.json.bak -> daemon.json.prev  # symlink to previous good copy
    ├── daemon.json.prev                     # actual previous copy
    └── runtimes/
        └── <runtime_id>/
            ├── runtime.json                 # durable spec + last-known instance
            ├── runtime.json.bak -> runtime.json.prev
            ├── runtime.json.prev
            ├── screen/<pane_id>.snap        # deterministic screen snapshot
            ├── scrollback/<pane_id>.log     # append-only, rotated not truncated
            ├── scrollback/<pane_id>.log.1   # rotated segments (keep last N)
            └── history/<pane_id>.hist       # unchanged semantics
```

The `daemon/` subdirectory isolates daemon-owned state from client-owned state
(RFC-023 owns `$XDG_STATE_HOME/rttx/client/`). This avoids naming conflicts and
makes ownership unambiguous.

The scrollback log remains under the runtime directory so deleting the runtime
directory is one `remove_dir_all` call.

The `OsInterface` trait currently exposes `cache_dir()` backed by
`$XDG_CACHE_HOME`. Implementation will add a `state_dir()` method backed by
`$XDG_STATE_HOME`, with the dev-mode variant using `rttx-devel` as it does
today.

### 2. Versioning & schema evolution

Every persisted file has a top-level `schema_version: u32` field.

```rust
#[derive(Serialize, Deserialize)]
struct DaemonIndexV1 {
    schema_version: u32,    // must be 1
    server_version: String, // informational only
    runtime_ids: Vec<Uuid>,
    created_at: SystemTime,
    last_serialized_at: SystemTime,
}
```

Loading dispatches on `schema_version`:

- Unknown future version → refuse to load, log, start fresh (rather than guess)
- Known past version → call a typed migration function that produces the current
  version's struct

Migrations live in `rttx-server/src/state/migrations.rs` as a chain of
`fn migrate_vN_to_vN_plus_1(old: VN) -> VN_plus_1`. The chain is total: any
supported version can be walked forward to current. No field defaults scattered
across the main structs — defaults are migration code, not struct attributes.

### 3. Durable vs transient split

```rust
// Durable: goes to runtime.json
struct RuntimeSpecV1 {
    schema_version: u32,
    id: Uuid,
    name: String,
    policy: RuntimePolicy,
    created_at: SystemTime,
    panes: Vec<PaneSpecV1>,
    active_pane_id: Option<Uuid>,
    command_history: Vec<HistoryEntry>,
}

// Semi-durable: also in runtime.json, but bounded & bounded-age
struct RuntimeInstanceV1 {
    revision: u64,
    last_active_at: SystemTime,
    last_snapshot_at: SystemTime, // when screen snapshots were last flushed
}

// Ephemeral: never written
// - attached_clients: HashMap<Uuid, ClientRole> (always rebuilt on attach)
// - reconstructed: bool
// - pending PTY replies
// - in-memory PaneScreen (raw_bytes, cursor, terminal mode flags)
```

`runtime.json` wraps both:
```rust
struct RuntimeFileV1 {
    schema_version: u32,
    spec: RuntimeSpecV1,
    instance: RuntimeInstanceV1,
}
```

### 4. Screen snapshot as a first-class type

The current design persists raw PTY bytes and replays them into VTE on the
client. `PaneScreen` stores a `raw_bytes: Vec<u8>` stream and tracks cursor
position, title, CWD, and terminal mode flags (bracketed paste, application
cursor keys, application keypad, mouse tracking, SGR mouse, focus events,
cursor visibility) via a VTE parser.

New contract:

- `screen/<pane_id>.snap` is a **deterministic screen snapshot** produced from
  the `PaneScreen` state on each flush
- Consumed on resurrection to restore the visible screen without replaying raw
  bytes
- Reconnect `AttachRuntime` snapshot encodes this data directly over the wire
  (proto change deferred to RFC-021; until then, render snapshot → ANSI for
  backward compatibility)
- The append-only **scrollback log** is separate: it is the history stream,
  not the reconnect seed. It need not be replayed to restore the visible
  screen — only when the user scrolls back

The snapshot schema, aligned with the current `PaneScreen` fields and the v3
`PaneSnapshot` wire format:

```rust
#[derive(Serialize, Deserialize)]
struct ScreenSnapshotV1 {
    schema_version: u32,       // must be 1
    pane_id: Uuid,
    cols: u16,
    rows: u16,
    cursor_row: u16,
    cursor_col: u16,
    cursor_visible: bool,
    title: Option<String>,
    cwd: Option<String>,
    pane_output_seq: u64,      // monotonic output counter for delta ordering
    modes: TerminalModeSnapshot,
    /// Raw bytes representing the visible screen content.
    /// This is the tail of the PTY stream sufficient to reconstruct the
    /// visible viewport. Bounded by rows × cols × max_bytes_per_cell.
    /// Future iterations may replace this with a cell-grid model.
    screen_bytes: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct TerminalModeSnapshot {
    bracketed_paste: bool,
    application_cursor_keys: bool,
    application_keypad: bool,
    mouse_tracking_mode: u16,
    sgr_mouse: bool,
    focus_reporting: bool,
}
```

The `screen_bytes` field is bounded by the visible viewport size, not the full
scrollback. This eliminates the mid-escape-sequence corruption problem: the
snapshot is produced from the screen engine's current state, not sliced from a
raw byte stream.

Consequence: scrollback becomes a rotate-not-truncate stream:

- Rotate at 10 MB: `log` → `log.1`, `log.1` → `log.2`, keep up to 3 segments
- Tail of the oldest segment may still start mid-sequence, but it is only ever
  rendered on user-initiated scrollback and is clearly labelled as history

### 5. Dirty-flag-driven writes

`Runtime` gains a `persisted_revision: u64` alongside `revision`. Serialization
loop writes only runtimes where `revision > persisted_revision`. After a
successful write, `persisted_revision = revision`.

The top-level `daemon.json` is rewritten only when `runtime_ids` changes
(runtime created or removed), not every tick.

Expected result: idle daemons with N runtimes do ~0 writes per tick, not N.

The default serialization interval is 5 seconds. With dirty-flag writes the
interval is less performance-critical, but it defines the **crash data-loss
window**: up to 5 seconds of state may be lost on an unclean daemon shutdown.

### 6. Corruption containment

On load:

1. Parse `daemon.json`. On failure (missing, corrupt, or unreadable), try
   `daemon.json.prev` (the symlink `.bak` target). On second failure, start
   fresh and log loudly (do not delete the files — let the user inspect).
2. For each `runtime_id` in the index, parse its `runtime.json` / `.prev`
   independently. A corrupt runtime is dropped from the working set and logged;
   the rest load normally.
3. Screen snapshots are best-effort: a corrupt `.snap` resurrects the pane as a
   blank screen, not a failed runtime.

On write:

1. Write to `runtime.json.tmp`
2. Rename `runtime.json.tmp` → `runtime.json.prev`
3. Update the `runtime.json.bak` symlink to point to `runtime.json.prev`
4. Rename `runtime.json.prev` → `runtime.json`

Using symlinks for the `.bak` reference instead of renaming the live file
avoids the window where neither `runtime.json` nor `.bak` exists. The symlink
update is atomic on all supported Linux filesystems. If the process crashes at
any point, at least one of `runtime.json` or `runtime.json.prev` (reachable
via the `.bak` symlink) contains a valid copy.

### 7. Cleanup

When a runtime is removed from the daemon:

- Remove `runtimes/<runtime_id>/` recursively in a background task
- Remove its entry from `daemon.json`'s index on the next tick

Orphan sweep runs **on startup only**: any `runtimes/<id>/` directory not
referenced by `daemon.json` is moved to `runtimes/.orphans/<id>/` (not deleted)
for manual recovery. `.orphans/` entries older than 30 days are pruned on the
same startup sweep. This is sufficient because the daemon restarts infrequently
enough that orphans won't accumulate between restarts.

### 8. Atomic write semantics

All writes use the tmp-write + symlink-bak + rename pattern described in §6.
The "either the old or the new file is visible" invariant is preserved across a
crash at any point.

### 9. Secrets and future encryption

Add a `no_persist: bool` hint per pane (default false). When true, scrollback
and history are not flushed to disk; the screen snapshot is still written so
reconnect works, but it is marked `confidential: true` in the file and is
excluded from any future export / sync action.

The `no_persist` flag is surfaced in the **workspace creation dialog** as an
option alongside the endpoint and policy choices. It can also be toggled
per-pane after creation via the pane context menu.

Full at-rest encryption is out of scope (NG1) but `no_persist` + file-per-pane
means the encryption boundary is already in the right place for a future RFC.

### 10. Configuration

One knob:

```toml
[daemon.state]
# Default: $XDG_STATE_HOME/rttx (or $HOME/.local/state/rttx)
dir = "/custom/path"

# Default: 5
serialize_interval_secs = 5

# Default: 3
scrollback_rotate_keep = 3
```

---

## Implementation Snapshot

**Implemented (shipped in v1.0.0).** The storage redesign landed as designed, then
evolved under RFC-031 (server-authoritative workspaces), which *extends* this RFC.
Two naming/behavioral deviations resulted:

- **`Runtime` → `Workspace`** everywhere. `RuntimeFileV1` became `WorkspaceFileV2`
  (schema version 2), `RuntimeSpecV1` → `WorkspaceSpecV2`, `RuntimeInstanceV1` →
  `WorkspaceInstanceV1`. All per-pane durable state is now keyed on an immutable
  server-assigned `PaneId` (RFC-031), not a random per-process id.
- **Orphan sweep (§7) was superseded.** RFC-031 §8 replaced the startup orphan
  sweep with explicit **close-driven cleanup** keyed on pane-tree membership: with
  a server-owned tree nothing is ever left unreferenced, so a sweep is unnecessary.
  See `services/rttx-server/src/state/cleanup.rs`.
- **`command_history` was removed**, not persisted in `WorkspaceSpecV2`. Durable
  history is now shell-init injection keyed on `PaneId` (RFC-031 §7).

The "Current source locations" table below is retained as the **pre-implementation
baseline**; those files (`serialization.rs`, `runtime.rs`, `PersistedRuntime`,
`ServerState`, `state.json`, `cache_dir()`) no longer exist. The current state code
lives under `services/rttx-server/src/state/` (`layout.rs`, `types.rs`,
`persistence.rs`, `migrations.rs`, `io.rs`, `cleanup.rs`).

### Current source locations *(pre-implementation baseline — since renamed/removed)*

| Component | File |
| --- | --- |
| State serialization | `services/rttx-server/src/serialization.rs` — `write_state_atomic()`, `load_state()`, `ServerState`, path helpers |
| Serialization loop | `services/rttx-server/src/server.rs` — `serialization_loop()` (1-second tick, unconditional full rewrite) |
| Runtime persistence | `services/rttx-server/src/runtime.rs` — `PersistedRuntime`, `Runtime::persist()`, `Runtime::resurrect()` |
| Scrollback flush | `services/rttx-server/src/pane.rs` — `flush_scrollback()`, `truncate_log_tail()` |
| Screen model | `services/rttx-server/src/screen.rs` — `PaneScreen`, `ScreenPerformer` (raw-bytes model, no cell grid) |
| Snapshot constant | `services/rttx-server/src/pane.rs` — `MAX_SNAPSHOT_BYTES = 256 KB` |
| OS path abstraction | `services/rttx-server/src/os/unix.rs` — `UnixOs::cache_dir()` (XDG_CACHE_HOME) |
| Dev-mode isolation | `services/rttx-server/src/os/unix.rs` — `rttx-server` vs `rttx-server-devel` directory names |

### Current test coverage for persistence

| Test | Layer | Location |
| --- | --- | --- |
| `round_trip_write_and_load` | Integration | `tests/serialization.rs` |
| `missing_state_file_returns_none` | Integration | `tests/serialization.rs` |
| `default_policy_is_ephemeral` | Integration | `tests/serialization.rs` |
| `load_state_rejects_corrupt_json` | Integration | `tests/persistence_failures.rs` |
| `corrupt_pane_does_not_crash_daemon` | Integration | `tests/persistence_failures.rs` |
| `missing_pane_fields_use_defaults` | Integration | `tests/persistence_failures.rs` |
| Backward-compat round-trips | Integration | `tests/persistence_compat.rs` |
| `scrollback_flushed_to_disk_after_serialization_tick` | Integration | `tests/scrollback.rs` |

### What RFC-022 proposed vs what shipped (v1.0.0)

Implemented via this RFC and its RFC-031 evolution (`Runtime` → `Workspace`).

| RFC Feature | Status |
| --- | --- |
| XDG_STATE_HOME | ✅ `OsInterface::state_dir()` → `$XDG_STATE_HOME/rttx/daemon/` (`os/unix.rs`) |
| Per-workspace files | ✅ `workspaces/<id>/workspace.json` (`state/layout.rs`, `state/persistence.rs`) |
| `schema_version` field | ✅ `DAEMON_INDEX_SCHEMA_VERSION`, `RUNTIME_FILE_SCHEMA_VERSION = 2`, `SCREEN_SNAPSHOT_SCHEMA_VERSION` (`state/types.rs`) |
| Schema migration module | ✅ `state/migrations.rs` — total forward migration chain |
| Dirty-flag writes | ✅ `persisted_revision` on `Workspace`; write gated on `revision > persisted_revision` (`workspace.rs`, `server.rs`) |
| Screen snapshot type | ✅ `ScreenSnapshotV1` (`state/types.rs`) |
| Scrollback rotation | ✅ `rotate_scrollback_log`, `SCROLLBACK_ROTATE_KEEP = 3` (`pane.rs`) |
| Corruption containment | ✅ Per-file primary/backup fallback; a corrupt file is logged and dropped, not fatal (`state/persistence.rs`, `state/io.rs`) |
| `.bak` backup | ✅ `write_with_backup` / `read_backup` (`state/io.rs`) |
| Dead workspace cleanup | ✅ Close-driven, keyed on pane-tree membership (`state/cleanup.rs`, `server.rs`) |
| Durable/transient split | ✅ `WorkspaceSpecV2` (durable) + `WorkspaceInstanceV1` (semi-durable) (`state/types.rs`) |
| Orphan sweep | ⚠️ **Superseded** by RFC-031 §8 — close-driven cleanup keys on tree membership, so nothing is left unreferenced and a startup sweep is unnecessary (`state/cleanup.rs`) |
| `no_persist` pane flag | ✅ Per-pane "Confidential mode" toggle action (`window/actions.rs`); ⚠️ surfaced as a per-pane toggle only, **not** in the workspace-creation dialog |
| `command_history` field | ⚠️ **Removed** under RFC-031 — durable history is now shell-init keyed on `PaneId`, not a persisted field |

### Deviations from original text

**Current layout path.** The original Background section listed the current
layout as `$XDG_CACHE_HOME/rttx/`. The actual production path is
`$XDG_CACHE_HOME/rttx-server/` (with `rttx-server-devel/` in dev mode). The
proposed v2 path `$XDG_STATE_HOME/rttx/daemon/` uses a `daemon/` subdirectory
to cleanly separate daemon state from client state (RFC-023).

**PersistedRuntime fields.** The original text listed `revision`,
`last_active_at`, `policy`, and `panes` as the persisted fields. The actual
struct also includes `command_history: Vec<HistoryEntry>` and
`active_pane_id: Option<Uuid>`, both of which are durable and belong in the
proposed `RuntimeSpecV1`.

**Screen model.** `PaneScreen` wraps a VTE parser (`vte::Parser`) and a
`ScreenPerformer` that tracks `raw_bytes`, cursor position, title, CWD, and
terminal mode flags (bracketed paste, application cursor keys, application
keypad, mouse tracking, SGR mouse, focus events, cursor visibility). The code
contains an explicit comment noting that a full cell-grid model is deferred to
a later iteration. The `ScreenSnapshotV1` struct defined in §4 captures these
fields as a serializable type.

**Serialization interval.** The current implementation uses 1 second. This RFC
sets the default to 5 seconds. The dirty-flag optimization (§5) makes the
interval less performance-critical; the tradeoff is a wider crash data-loss
window (up to 5 seconds vs 1 second).

### Relationship to RFC-023

RFC-023 (Client Configuration and State Store) proposes a parallel redesign for
the client side, also targeting `$XDG_STATE_HOME/rttx/`. The two RFCs share the
same XDG base directory but govern different subdirectories:

- `$XDG_STATE_HOME/rttx/daemon/` — RFC-022 (this RFC)
- `$XDG_STATE_HOME/rttx/client/` — RFC-023

Implementation must coordinate the directory layout before either RFC lands
code. This is Step 0 in the development plan.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 — Survive cache cleanup | §1 moves to `$XDG_STATE_HOME` |
| G2 — Scale with churn | §5 dirty-flag writes; §1 per-runtime files |
| G3 — Schema version & migration | §2 explicit `schema_version` + migration chain |
| G4 — Corruption isolation | §6 per-file load, symlink `.bak` fallback, orphan quarantine |
| G5 — Durable vs transient split | §3 Spec/Instance/Ephemeral tiers |
| G6 — Deterministic screen replay | §4 `ScreenSnapshotV1` separate from scrollback |
| G7 — Dead-runtime pruning | §7 directory removal + orphan sweep |
| G8 — Extensibility | §2 + §9 leave encryption, sync, export as additive changes |

---

## Development Plan

- [x] **Step 0** — Coordinate with RFC-023 on shared `$XDG_STATE_HOME/rttx/`
  directory layout: `daemon/` vs `client/` subdirectories *(prerequisite: —)*
- [x] **Step 1** — Introduce `state::layout` module: path helpers for the new
  directory tree *(prerequisite: Step 0)* — `state/layout.rs`
- [x] **Step 2** — Define `DaemonIndexV1`, `WorkspaceFileV2` (was `RuntimeFileV1`),
  `PaneSpecV2`, `ScreenSnapshotV1` structs with `schema_version` *(prerequisite: Step 1)*
  — `state/types.rs`
- [x] **Step 3** — Swap load/serialization to per-workspace files with `.bak`
  backup; on first startup with no v2 state, start clean and log the change
  *(prerequisite: Step 2)* — `state/persistence.rs`, `state/io.rs`; #1008, #1028
- [x] **Step 4** — Add `persisted_revision` to `Workspace` and skip clean-workspace
  writes *(prerequisite: Step 3)* — `workspace.rs`, `server.rs`
- [x] **Step 5** — Implement `ScreenSnapshotV1` serialization from `PaneScreen`
  and replace the raw-bytes reconstruction path; switch scrollback truncation to
  rotation *(prerequisite: Step 3)* — `pane.rs` (`rotate_scrollback_log`)
- [x] **Step 6** — Workspace-directory removal on delete. *(prerequisite: Step 3)*
  — ⚠️ the **startup orphan sweep was superseded** by RFC-031 §8 close-driven
  cleanup (`state/cleanup.rs`); nothing is left unreferenced, so no sweep runs.
- [x] **Step 7** — `no_persist` pane flag plumbed to flush/snapshot, surfaced as a
  per-pane "Confidential mode" toggle *(prerequisite: Step 5)* — `window/actions.rs`,
  `shell_init.rs`. ⚠️ Not surfaced in the workspace-creation dialog (per-pane only).
- [x] **Step 8** — Tests: corrupt file does not kill the daemon, dirty-flag skips
  writes, rotation keeps N segments, `ScreenSnapshotV1` round-trip
  *(prerequisite: Steps 3–6)* — `state/persistence.rs`, `state/cleanup.rs`
- [x] **Step 9** — Packaging/dev docs updated with the new `$XDG_STATE_HOME/rttx/`
  location; first-run clean-start log line *(prerequisite: Step 3)* — `CONTRIBUTING.md`, #1054

Shipped in v1.0.0. Implemented in coordination with RFC-031, which extended the
schema (`Runtime` → `Workspace`) and replaced the orphan sweep with close-driven
cleanup.

---

## Open Questions

- [x] **Q1** — Should we keep a legacy `state.json` importer? **Resolved:**
  No. Clean start on v2. Users get a loud log line. The old cache directory is
  left untouched for manual inspection.
- [x] **Q2** — Should `daemon.json` hold any per-runtime summary (name, pane
  count) to render a sidebar without reading every `runtime.json`, or keep it
  minimal (ids only)? **Resolved:** minimal — the daemon always has runtimes
  in-memory after startup, so the index is only consulted once.
- [x] **Q3** — Scrollback rotation default of 3×10 MB = 30 MB per pane may be
  higher than current effective limit. Acceptable? **Resolved:** yes, the old
  cap was partly driven by the truncation approach, not a user-facing promise.
- [x] **Q4** — Is `$XDG_STATE_HOME` correct on macOS / sandboxed Flatpak?
  **Resolved:** rttx targets GNOME/Linux only (RFC-001 principle 1); macOS is
  permanently out of scope. Flatpak sandboxes map `$XDG_STATE_HOME` correctly
  via the portal filesystem, so no special handling is needed.
- [x] **Q5** — Should `.bak` files use renames or symlinks? **Resolved:**
  symlinks. A symlink update is atomic on supported Linux filesystems and avoids
  the window where neither the live file nor the backup exists.

---

## References

- [RFC-007: Session Recovery](./RFC-007-session-recovery.md) — recipe-based recovery; daemon-side
  persistence delegated to this RFC
- [RFC-010: Maintainability Refactor](./RFC-010-maintainability-refactor.md) (Implemented)
- [RFC-013: Persistent Host Sessions](./RFC-013-persistent-host-sessions.md) (Implemented) —
  established the daemon-backed runtime model that this RFC's storage redesign serves
- [RFC-018: Workspace Connection State Machine](./RFC-018-workspace-connection-state-machine.md)
  (Implemented)
- [RFC-019: Missing Daemon Session Handling](./RFC-019-missing-session-handling.md) (Implemented)
- [RFC-021: Client/Server Protocol v3](./RFC-021-client-server-protocol-v3.md) (Review) — grid
  snapshot wire encoding deferred to protocol v3
- [RFC-023: Client Configuration and State Store](./RFC-023-client-configuration-state-store.md)
  (Review) — parallel client-side storage redesign sharing `$XDG_STATE_HOME/rttx/`
- `services/rttx-server/src/serialization.rs` — current monolith writer
- `services/rttx-server/src/runtime.rs` — `PersistedRuntime` current schema
- `services/rttx-server/src/pane.rs` — `flush_scrollback()`, `truncate_log_tail()`
- `services/rttx-server/src/screen.rs` — `PaneScreen` raw-bytes model
- `services/rttx-server/src/os/unix.rs` — `OsInterface::cache_dir()` path abstraction
- [#622](https://github.com/IllyaYalovyy/rttx/issues/622) — tracking issue for this review
