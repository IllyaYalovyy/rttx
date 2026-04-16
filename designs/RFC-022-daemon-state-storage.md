# RFC-022: Daemon State Storage

| Field         | Value          |
|---------------|----------------|
| Status        | Draft          |
| Author(s)     | Illya Yalovyy  |
| Supersedes    | —              |
| Superseded by | —              |

---

## Summary

The rttx daemon persists every session's runtime state into a single monolithic
`state.json` file, rewritten in full on every serialization tick, stored under the
user's XDG **cache** directory. Scrollback and shell history are written as raw
per-pane files next to it. There is no schema version, no migration hook, no backup,
no dirty-tracking, and no separation between durable spec and transient runtime
instance data.

This works for the tens-of-sessions case today. It does not scale, does not survive
cache eviction, and makes every future persistence-adjacent feature (encrypted
scrollback, per-host state, cross-machine sync, crash-safe recovery, selective
export) a load-bearing rewrite rather than an additive change.

Because we have explicit permission to break backward compatibility now, this RFC
proposes a clean v2 layout: a **per-session directory with versioned, typed files
under `$XDG_STATE_HOME/rttx`**, a small top-level index, explicit schema versioning
with a migration module, dirty-flag-driven writes, deterministic screen snapshots
separate from append-only scrollback logs, and a durable-vs-ephemeral split.

---

## Goals

- **G1** — State that survives OS/user cache cleanup (move out of `XDG_CACHE_HOME`)
- **G2** — Per-session files so write cost scales with churn, not with total session count
- **G3** — Explicit `schema_version` and a typed migration path on every persisted struct
- **G4** — A single corrupt file never takes down the whole daemon's history
- **G5** — Clean separation between *durable* state (id, name, policy, layout) and
  *transient* runtime state (revision, attached_clients, pending_replies)
- **G6** — Reconnect snapshots are reconstructed from a deterministic screen model,
  not raw byte tails that may start mid-escape-sequence
- **G7** — Pruning of removed sessions' on-disk artefacts is automatic
- **G8** — The layout is extensible to future features (encrypted panes, per-host
  stores, sync to relay) without another flag-day migration

## Non-Goals

- **NG1** — Encryption at rest. Designed-for but not delivered in this RFC
- **NG2** — Cross-daemon replication or a cloud sync protocol (future RFC; this
  RFC makes it possible, not present)
- **NG3** — Replacing JSON with a binary format. We stay on JSON for diff-ability
  and hand-editability; binary can be swapped per-file later
- **NG4** — Backward compatibility with the v1 `state.json` layout. Migration is
  one-way: a v1 file loads once, is upgraded in place, and v1 is no longer written
- **NG5** — A new IPC surface for state inspection — this RFC concerns on-disk
  storage only

---

## Background & Motivation

### Current layout

```
$XDG_CACHE_HOME/rttx-server/
├── state.json                              # all sessions, rewritten every tick
├── scrollback/<session_id>/<pane_id>.log   # raw bytes, tail-truncated at 10MB
└── history/<session_id>/<pane_id>.hist     # per-pane shell history
```

`state.json` contains a single `ServerState { sessions, serialized_at, server_version }`.
See `services/rttx-server/src/serialization.rs` and `session.rs:PersistedSession`.

In dev mode (`RTTX_DEV_MODE=1`), the daemon uses `$XDG_CACHE_HOME/rttx-server-devel/`
instead, so development and production state are fully isolated.

### Pain points observed today

1. **Cache directory is the wrong home.** `XDG_CACHE_HOME` is explicitly defined as
   *data the user can regenerate or delete*. Distro cleaners, Flatpak refresh, and
   `systemd-tmpfiles` can all wipe it. Our sessions are not regenerable.

2. **Monolithic rewrite.** Every serialization tick builds a full snapshot under
   the server mutex, pretty-prints it, writes to `.tmp`, renames over the old file
   (see `serialization_loop` in `server.rs` and `write_state_atomic` in
   `serialization.rs`). Cost grows linearly with total session count even if
   only one session actually changed.

3. **No schema version.** The only version marker is `server_version: String` —
   the Cargo package version. There is no `schema_version`, no migration entry
   point, and `#[serde(default)]` is used ad-hoc per field (e.g., on `policy`
   and `revision` in `PersistedSession`). A non-trivial schema change today
   requires bespoke logic scattered across struct definitions.

4. **Scrollback truncation corrupts replay.** `Pane::flush_scrollback` appends
   pending bytes to the scrollback log, then calls `truncate_log_tail` when the
   file exceeds `DEFAULT_MAX_SCROLLBACK_LOG` (10 MB). The truncation reads the
   full file, slices off the head at a raw byte boundary, and rewrites. The kept
   tail may start mid-escape-sequence, producing visible garbage when replayed.
   `MAX_SNAPSHOT_BYTES = 256 KB` (defined in `pane.rs`) masks but does not fix
   this.

5. **Durable and transient are mixed.** `PersistedSession` holds `revision` and
   `last_active_at` alongside `policy`, `panes`, `command_history`, and
   `active_pane_id`, but not `attached_clients` (which is correctly ephemeral).
   The boundary is implicit and easy to get wrong on the next field.

6. **No cleanup.** When a session is deleted, its scrollback and history
   directories remain on disk. Over months of use the cache grows unboundedly.

7. **Corruption is fatal to the entire history.** `load_state` returns
   `Err(InvalidData)` on a bad parse, logs it, and the daemon starts empty. One
   bad byte in the monolith loses everything.

8. **No write coalescing or dirty tracking.** A session that hasn't changed in
   an hour is rewritten identically 3 600+ times. `Session` has a `revision: u64`
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
- Migration story (new subsystem)
- Screen-state model (new abstraction — serialized grid vs raw bytes)
- Cleanup lifecycle (new scheduler work)

…and they interlock: you cannot introduce per-session files without a
schema-version story; you cannot make scrollback replay correct without a
screen-model serialization; you cannot move to `$XDG_STATE_HOME` without also
deciding the migration-from-v1 behaviour. This needs one consistent design.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Sessions survive cache cleanup; corruption is contained to one session, not the whole daemon; reconnect shows the correct screen, not garbled bytes |
| Contributors | Adding a field to a persisted struct is a one-line schema bump with a typed migration; per-session files mean smaller diffs and faster tests |
| Packagers    | State moves to `$XDG_STATE_HOME/rttx`; packaging docs updated; no cache-cleaner collateral damage |

---

## Considered Options

### Option A — Keep monolithic `state.json`, add `schema_version` only

**Pros**: Smallest change. Only adds a version field and a migration hook.

**Cons**: Does nothing for cache-eviction, write scaling, corruption blast radius,
or dead-session pruning. We will be back here within a release.

### Option B — Per-session directory, versioned files, durable/transient split

**Pros**: Addresses every pain point listed. Each session is its own unit of
durability, corruption, and migration. Diffs are tractable.

**Cons**: Breaking layout change. Needs a one-time v1→v2 importer. More files on
disk (trivial; filesystems handle this fine — tmux/zellij already do).

### Option C — Embedded SQLite

**Pros**: Transactional. Single file. Battle-tested. Easy to query from tooling.

**Cons**: Binary file is not diff-able or hand-editable; adds a non-trivial
dependency; the concurrency model (single writer, many readers) gives us no
benefit over per-session JSON since the daemon is already the sole writer.
Scrollback as BLOBs in SQLite is measurably worse than append-only log files.

### Option D — Event-sourced log + periodic snapshot

**Pros**: Crash-consistent by construction; trivially replicable; easy to export
recent history.

**Cons**: Operational complexity (compaction, snapshot cadence, log GC) that
materially exceeds the problem we have. Good target for a *later* RFC once we
have replication requirements.

---

## Decision

**Chosen option: Option B — per-session versioned files with a clean durable/transient split.**

Rationale:

- It is the minimum change that addresses every pain point simultaneously
- Stays on JSON, so diffs, review, and manual recovery all keep working
- Leaves the door open to swap specific files (screen snapshot, scrollback) to
  binary later without changing the layout
- Does not preclude Option D as a future addition: the per-session file becomes
  the "snapshot" in an event-sourced model

We take the one-time break now while we still can. Migration is a single
best-effort import of v1 `state.json` on first v2 startup.

---

## Design

### 1. On-disk layout

```
$XDG_STATE_HOME/rttx/                   # (was: $XDG_CACHE_HOME/rttx-server/)
├── daemon.json                         # server-level index, schema v1
├── daemon.json.bak                     # previous good copy
└── sessions/
    └── <session_id>/
        ├── session.json                # durable spec + last-known instance, schema v1
        ├── session.json.bak            # previous good copy
        ├── screen/<pane_id>.snap       # deterministic screen snapshot, schema v1
        ├── scrollback/<pane_id>.log    # append-only, rotated not truncated
        ├── scrollback/<pane_id>.log.1  # rotated segments (keep last N)
        └── history/<pane_id>.hist      # unchanged semantics
```

The scrollback log remains under the session directory so deleting the session
directory is one `remove_dir_all` call.

The `OsInterface` trait currently exposes `cache_dir()` backed by
`$XDG_CACHE_HOME`. Implementation will add a `state_dir()` method (or rename
`cache_dir`) backed by `$XDG_STATE_HOME`, with the dev-mode variant using
`rttx-server-devel` as it does today.

### 2. Versioning & migration

Every persisted file has a top-level `schema_version: u32` field.

```rust
#[derive(Serialize, Deserialize)]
struct DaemonIndexV1 {
    schema_version: u32,    // must be 1
    server_version: String, // informational only
    session_ids: Vec<Uuid>,
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
// Durable: goes to session.json
struct SessionSpecV1 {
    schema_version: u32,
    id: Uuid,
    name: String,
    policy: RuntimePolicy,
    created_at: SystemTime,
    panes: Vec<PaneSpecV1>,
    active_pane_id: Option<Uuid>,
    command_history: Vec<HistoryEntry>,
}

// Semi-durable: also in session.json, but bounded & bounded-age
struct SessionInstanceV1 {
    revision: u64,
    last_active_at: SystemTime,
    last_snapshot_at: SystemTime,
}

// Ephemeral: never written
// - attached_clients: HashMap<Uuid, ClientRole> (always rebuilt on attach)
// - reconstructed: bool
// - pending PTY replies
// - in-memory PaneScreen (raw_bytes, cursor, terminal mode flags)
```

`session.json` wraps both:
```rust
struct SessionFileV1 {
    schema_version: u32,
    spec: SessionSpecV1,
    instance: SessionInstanceV1,
}
```

### 4. Screen snapshot as a first-class type

The current design persists raw PTY bytes and replays them into VTE on the
client. `PaneScreen` stores a `raw_bytes: Vec<u8>` stream and tracks cursor
position, title, CWD, and terminal mode flags (bracketed paste, application
cursor keys, mouse tracking, etc.) via a VTE parser. The code explicitly notes
that a full cell-grid model is deferred to a later iteration.

New contract:

- `screen/<pane_id>.snap` is a **serialized grid**: dimensions + cells with
  (codepoint, foreground, background, attributes) — the same data the screen
  engine already holds in memory
- Produced on flush from `PaneScreen`; consumed on resurrection
- Reconnect `AttachSession` snapshot encodes this grid directly over the wire
  (proto change deferred to RFC-021; until then, render grid → ANSI for
  backward compatibility)
- The append-only **scrollback log** is separate: it is the history stream,
  not the reconnect seed. It need not be replayed to restore the visible
  screen — only when the user scrolls back

Consequence: we stop truncating scrollback to fix a rendering bug, because the
rendering bug moves to the grid file which is bounded by rows×cols, not bytes.
Scrollback becomes a rotate-not-truncate stream:

- Rotate at 10 MB: `log` → `log.1`, `log.1` → `log.2`, keep up to 3 segments
- Tail of the oldest segment may still start mid-sequence, but it is only ever
  rendered on user-initiated scrollback and is clearly labelled as history

### 5. Dirty-flag-driven writes

`Session` gains a `persisted_revision: u64` alongside `revision`. Serialization
loop writes only sessions where `revision > persisted_revision`. After a
successful write, `persisted_revision = revision`.

The top-level `daemon.json` is rewritten only when `session_ids` changes
(session created or removed), not every tick.

Expected result: idle daemons with N sessions do ~0 writes per tick, not N.

### 6. Corruption containment

On load:

1. Parse `daemon.json`. On failure, try `daemon.json.bak`. On second failure,
   start fresh and log loudly (do not delete the files — let the user
   inspect).
2. For each `session_id` in the index, parse its `session.json` / `.bak`
   independently. A corrupt session is dropped from the working set and logged;
   the rest load normally.
3. Screen snapshots are best-effort: a corrupt `.snap` resurrects the pane as a
   blank screen, not a failed session.

On write:

1. Write to `session.json.tmp`
2. Rename `session.json` → `session.json.bak` (if it exists)
3. Rename `session.json.tmp` → `session.json`

This gives us exactly one previous good copy per session at all times.

### 7. Cleanup

When a session is removed from the daemon:

- Remove `sessions/<session_id>/` recursively in a background task
- Remove its entry from `daemon.json`'s index on the next tick

Orphan sweep on startup: any `sessions/<id>/` directory not referenced by
`daemon.json` is moved to `sessions/.orphans/<id>/` (not deleted) for manual
recovery. `.orphans/` is pruned after 30 days.

### 8. Atomic rename semantics

All writes use the existing tmp-rename pattern. The `.bak` rotation is also a
rename, so the "either the old or the new file is visible" invariant is
preserved across a crash at any point.

### 9. Secrets and future encryption

Add a `no_persist: bool` hint per pane (default false). When true, scrollback
and history are not flushed to disk; the screen snapshot is still written so
reconnect works, but it is marked `confidential: true` in the file and is
excluded from any future export / sync action.

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

This RFC is in Draft status. None of the proposed changes have been implemented.
The sections below document the current v1 state as a baseline for future
implementation work.

### Current source locations

| Component | File |
| --- | --- |
| State serialization | `services/rttx-server/src/serialization.rs` — `write_state_atomic()`, `load_state()`, `ServerState`, path helpers |
| Serialization loop | `services/rttx-server/src/server.rs` — `serialization_loop()` (1-second tick, unconditional full rewrite) |
| Session persistence | `services/rttx-server/src/session.rs` — `PersistedSession`, `Session::persist()`, `Session::resurrect()` |
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

### What exists vs what RFC-022 proposes

| RFC Feature | Current Status |
| --- | --- |
| XDG_STATE_HOME | ❌ Uses `$XDG_CACHE_HOME/rttx-server/` via `OsInterface::cache_dir()` |
| Per-session files | ❌ Monolithic `state.json` with all sessions |
| `schema_version` field | ❌ Only `server_version: String` (Cargo package version) |
| Migration module | ❌ No `state/` module; ad-hoc `#[serde(default)]` on individual fields |
| Dirty-flag writes | ❌ Full rewrite every 1-second tick; `revision` exists but no `persisted_revision` |
| Screen snapshot type | ❌ Raw byte tails via `PaneScreen::snapshot_bytes()` |
| Scrollback rotation | ❌ `truncate_log_tail` slices at byte boundary (can corrupt escape sequences) |
| Corruption containment | ❌ One bad byte in `state.json` loses all sessions |
| `.bak` backup files | ❌ Atomic tmp+rename only, no backup copy |
| Dead session cleanup | ❌ Scrollback/history directories accumulate on disk |
| Durable/transient split | ❌ Mixed in `PersistedSession` (durable + semi-durable fields together) |
| Orphan sweep | ❌ Not implemented |
| `no_persist` pane flag | ❌ Not implemented |

### Deviations from original text

**Current layout path.** The original Background section listed the current
layout as `$XDG_CACHE_HOME/rttx/`. The actual production path is
`$XDG_CACHE_HOME/rttx-server/` (with `rttx-server-devel/` in dev mode). The
proposed v2 path `$XDG_STATE_HOME/rttx/` drops the `-server` suffix, which is
intentional — the daemon is the only writer to this directory.

**PersistedSession fields.** The original text listed `revision`,
`last_active_at`, `policy`, and `panes` as the persisted fields. The actual
struct also includes `command_history: Vec<HistoryEntry>` and
`active_pane_id: Option<Uuid>`, both of which are durable and belong in the
proposed `SessionSpecV1`.

**Screen model.** The original text described the screen model abstractly.
`PaneScreen` wraps a VTE parser (`vte::Parser`) and a `ScreenPerformer` that
tracks `raw_bytes`, cursor position, title, CWD, and terminal mode flags
(bracketed paste, application cursor keys, application keypad, mouse tracking,
SGR mouse, focus events, cursor visibility). The code contains an explicit
comment noting that a full cell-grid model is deferred to a later iteration.

**Serialization interval.** The original §10 proposed a default of 5 seconds
for `serialize_interval_secs`. The current implementation uses 1 second. The
proposed default should be reconciled with the current behavior during
implementation — the dirty-flag optimization (§5) makes the interval less
performance-critical, so a longer default may be acceptable.

### Relationship to RFC-023

RFC-023 (Client Configuration and State Store) proposes a parallel redesign for
the client side, also targeting `$XDG_STATE_HOME/rttx/`. The two RFCs share the
same XDG base directory but govern different subdirectories: RFC-022 covers
daemon-owned runtime state (`daemon.json`, `sessions/`), while RFC-023 covers
client-owned configuration and UI state. Implementation should coordinate the
directory layout to avoid conflicts.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 — Survive cache cleanup | §1 moves to `$XDG_STATE_HOME` |
| G2 — Scale with churn | §5 dirty-flag writes; §1 per-session files |
| G3 — Schema version & migration | §2 explicit `schema_version` + migration chain |
| G4 — Corruption isolation | §6 per-file load, `.bak` fallback, orphan quarantine |
| G5 — Durable vs transient split | §3 Spec/Instance/Ephemeral tiers |
| G6 — Deterministic screen replay | §4 grid snapshot separate from scrollback |
| G7 — Dead-session pruning | §7 directory removal + orphan sweep |
| G8 — Extensibility | §2 + §9 leave encryption, sync, export as additive changes |

---

## Development Plan

- [ ] **Step 1** — Introduce `state::layout` module: path helpers for the new
  directory tree, feature-gated alongside v1 paths *(prerequisite: —)*
- [ ] **Step 2** — Define `DaemonIndexV1`, `SessionFileV1`, `PaneSpecV1` structs
  with `schema_version` *(prerequisite: Step 1)*
- [ ] **Step 3** — Implement `state::migrations::from_legacy_state_json` — one
  best-effort import from the old monolith *(prerequisite: Step 2)*
- [ ] **Step 4** — Swap `load_persisted_state` and `serialization_loop` to use
  per-session files; keep the monolith reader only as a fallback during import
  *(prerequisite: Step 3)*
- [ ] **Step 5** — Add `persisted_revision` to `Session` and skip clean-session
  writes *(prerequisite: Step 4)*
- [ ] **Step 6** — Define `ScreenSnapshotV1` and replace the raw-bytes
  reconstruction path; keep scrollback logs but switch truncation to rotation
  *(prerequisite: Step 4)*
- [ ] **Step 7** — Implement session-directory removal on delete and the
  startup orphan sweep *(prerequisite: Step 4)*
- [ ] **Step 8** — Add `no_persist` pane flag and plumb to flush/snapshot
  *(prerequisite: Step 6)*
- [ ] **Step 9** — Tests: migration from a captured v1 `state.json`, corrupt
  `session.json` does not kill the daemon, dirty-flag skips writes, rotation
  keeps N segments, orphan sweep quarantines unreferenced directories
  *(prerequisite: Steps 4–7)*
- [ ] **Step 10** — Update packaging docs and the first-run log line to
  announce the new state location *(prerequisite: Step 4)*
- [ ] **Step 11** — Coordinate with RFC-023 on shared `$XDG_STATE_HOME/rttx/`
  directory layout to avoid conflicts between daemon and client state
  *(prerequisite: Step 1)*

Steps 1–4 can land as a single PR behind a feature flag. Steps 5–8 are
additive. Step 9 is gating. Step 10 ships with the release that removes the
v1 flag.

---

## Open Questions

- [x] **Q1** — Should we keep the legacy `state.json` importer in the code
  indefinitely, or remove it one minor release after the v2 cutover? **Resolved:**
  remove after one release; users who skip that release get a clean start and
  a loud log line.
- [x] **Q2** — Should `daemon.json` hold any per-session summary (name, pane
  count) to render a sidebar without reading every `session.json`, or keep it
  minimal (ids only)? **Resolved:** minimal — the daemon always has sessions
  in-memory after startup, so the index is only consulted once.
- [x] **Q3** — Scrollback rotation default of 3×10 MB = 30 MB per pane may be
  higher than current effective limit. Acceptable? **Resolved:** yes, the old cap
  was an artefact of the truncation bug, not a user-facing promise.
- [x] **Q4** — Is `$XDG_STATE_HOME` correct on macOS / sandboxed Flatpak?
  **Resolved:** rttx targets GNOME/Linux only (RFC-001 principle 1); macOS is
  permanently out of scope. Flatpak sandboxes map `$XDG_STATE_HOME` correctly
  via the portal filesystem, so no special handling is needed.

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
- `services/rttx-server/src/session.rs` — `PersistedSession` current schema
- `services/rttx-server/src/pane.rs` — `flush_scrollback()`, `truncate_log_tail()`
- `services/rttx-server/src/screen.rs` — `PaneScreen` raw-bytes model
- `services/rttx-server/src/os/unix.rs` — `OsInterface::cache_dir()` path abstraction
- [#622](https://github.com/IllyaYalovyy/rttx/issues/622) — tracking issue for this review
