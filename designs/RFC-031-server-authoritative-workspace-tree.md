# RFC-031: Server-Authoritative Workspaces

| Field         | Value                                                                        |
|---------------|------------------------------------------------------------------------------|
| Status        | Implemented                                                                  |
| Author(s)     | yalovyyi                                                                      |
| Supersedes    | RFC-026 (pane-state-clone-on-split); structural parts of RFC-016, RFC-018    |
| Superseded by | —                                                                            |

---

> **Implementation status (2026-07): Implemented.** This RFC shipped as the
> flagship of the **v1.0.0** release (see `CHANGELOG.md`). The server-authoritative
> workspace tree (`services/rttx-server/src/pane_tree.rs`, `workspace.rs`),
> immutable server-assigned `PaneId`, per-shell durable history
> (`shell_init.rs`), the `AttachWorkspace`/`WorkspaceSnapshot` protocol, and the
> deletion of the client-side `pane_bindings`/reconciliation layer (#1049) all
> landed on `mainline`. The Development Plan below is checked against the merged
> code. Multi-window interaction (Q3) remains a V2 follow-up under RFC-029.

---

## Summary

The daemon currently stores a workspace as a flat bag of panes keyed by
**randomly minted, per-process pane UUIDs**, while the GUI client owns the
split/layout tree and binds its layout leaves to those random ids. Identity and
structure are split across two owners and held together by a `pane_bindings`
table and reconciliation logic. When that binding desyncs — or a reconnect path
issues a fresh `CreatePane` — the random id changes and **every durable artifact
keyed on it (shell history, scrollback, screen snapshot) is silently orphaned.**
This is confirmed, reproducible data loss.

This RFC makes the **server the single source of truth for the entire workspace:
its identity, the tree of shells, their durable identities, and their logical
layout.** A client connects to a *workspace*, reads the authoritative tree, and
renders it. Pane identity is durable and immutable for the pane's lifetime; all
per-pane state is keyed on it. Per-client concerns (window pixel size, focus,
scroll position) are decoupled into an ephemeral **viewport** so that multiple
windows/clients are never forced to mirror pixels while still sharing one
canonical structure.

This is a **full refactor with no compatibility layer.** The client-side layout
tree, `pane_bindings`, reconciliation/recovery, random id minting, the
orphan-masking sweep, the `command_history` field, and the bash-only history
hack are **deleted, not adapted.** Existing on-disk state is reset once (pre-1.0).

---

## Goals

- **G1 — Immutable identity.** A pane's id is server-assigned, persisted, and
  immutable from creation until the user closes it. It never changes across
  shell respawn, daemon crash/restart, client restart, or reconnect.
- **G2 — Single owner of structure.** The server owns the workspace tree (panes,
  splits, logical ratios, ordering, default active pane). The client holds no
  independent copy of identity or structure.
- **G3 — Durable state, always.** All per-pane durable state (shell history,
  scrollback, screen snapshot, cwd, title) is keyed solely on the durable pane
  id and survives every crash class short of deleting the state directory.
- **G4 — Shell-correct history.** History is captured incrementally and
  correctly for bash, zsh, and fish via robust shell-init injection, not a
  clobberable env var.
- **G5 — Reconnect is a read.** Reattach re-fetches the workspace tree and
  re-renders. The client never creates, re-creates, or reconciles identity.
- **G6 — Viewport decoupling.** Window size, focused pane, and scroll position
  are per-client ephemeral state. Multiple clients share one logical tree
  without being forced into identical pixel layouts.
- **G7 — Net deletion.** The dual-identity binding layer and its failure modes
  are removed entirely; terminology is unified (`Runtime` → `Workspace`).

## Non-Goals

- **NG1 — No state migration.** Pre-1.0 clean break; old-schema state is reset
  once. No compatibility code. (A one-time, opt-in salvage of orphaned histfiles
  is offered as a separate utility, not a code path.)
- **NG2 — In-app state.** Capturing the internal state of programs running
  *inside* a pane (a TUI's own history) is out of scope and impossible without
  that program's cooperation.
- **NG3 — Multi-user collaboration / cloud sync** (RFC-014 territory).
- **NG4 — PTY byte-capture mechanism** is already shell-agnostic and unchanged.

---

## Background & Motivation

### Current model (verified in code)

- **Server** (`runtime.rs`): `Runtime` = `HashMap<Uuid, Pane>` + `active_pane_id`.
  No tree, no splits, no ratios. `CreatePane` mints `pane_id = Uuid::new_v4()`.
- **Persistence** (RFC-022): `PaneSpecV1` is a flat list; durable artifacts keyed
  on the random pane id (`history/<id>.hist`, `screen/<id>.snap`,
  `scrollback/<id>.log`).
- **Client** (`workspace/layout.rs`): owns the `LayoutNode` split tree and maps
  leaves → server pane ids via persisted `pane_bindings`, plus
  `reconcile_bindings`/recovery (`runtime.rs`, `workspace_state.rs`).
- **Protocol v3**: exchanges a flat `RuntimeSnapshot { panes[] }`. The server has
  no knowledge of arrangement.

### The defect

Durable state is keyed on an ephemeral, randomly-minted id that the client binds
to indirectly. When the binding is lost or a reconnect path re-creates a pane,
the id changes, the respawned shell gets an empty `HISTFILE`, and prior history
is orphaned. The screen only *appears* to survive because it is re-snapshotted
under the new id each cycle. This is structural: identity lives in the wrong
place, is duplicated, and is used as a durable key while being process-ephemeral.

### Why a full refactor, not a patch

Any fix that keeps structure split across client and server preserves the
desync surface. Hardening the binding or copying-forward orphaned files defends
the broken model instead of removing it. The correct fix is to give the server
sole ownership of workspace structure and identity, which also deletes a large,
fragile client subsystem.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | History, scrollback, layout, and identity survive every crash/reconnect. zsh/fish history works. One-time state reset on upgrade (pre-1.0). |
| Contributors | One source of truth for structure; large net code deletion; unified `Workspace` terminology; simpler protocol. |
| Packagers    | Version bump only. |

---

## Considered Options

### Option A — Harden the dual-identity model
Keep client-owned layout + server random ids; make the binding bulletproof and
copy-forward orphaned state.
**Pros**: smallest diff. **Cons**: preserves the root cause; endless edge cases.

### Option B — Server-owned immutable id, client still owns the layout tree
Server assigns a stable id; client keeps its tree and binds to it.
**Pros**: fixes id stability. **Cons**: still two owners of structure; binding +
reconciliation remain; layout not durable. A half-measure.

### Option C — Server-authoritative workspace; client is a view
Server owns the full tree (panes, splits, logical ratios, active default) and
durable identities. The client reads the tree and renders it; per-client
concerns live in an ephemeral viewport. Structural changes are server mutations.
**Pros**: single source of truth; identity stability is structural, not
defended; deletes the binding/reconciliation subsystem; layout durable;
coherent with rttx's identity as a persistent-session daemon (tmux model).
**Cons**: largest change; touches server, persistence, protocol, and client;
requires a clean state break.

---

## Decision

**Option C, without reservation.**

rttx *is* a persistent-session daemon — a server that owns long-lived shells you
reattach to. Server-authoritative structure is the architecturally honest model
for that, the same reason tmux/screen have used it for two decades. Option B was
considered and rejected: keeping the layout client-side preserves the exact
split-brain that causes the data loss, for the sake of a smaller diff. That
trade is not worth making.

The earlier worry that "server-owned tree forces tmux-style pixel mirroring" is
resolved by **decoupling the canonical logical tree (server, durable) from the
per-client viewport (ephemeral, client-local)** — see Design §3. Sharing
structure does not require sharing pixels.

**Clean break on state (NG1):** the persisted schema is replaced, not migrated.
On first run, old-schema state is ignored and removed. No compatibility code.

---

## Design

### 1. Identity

- `WorkspaceId` and `PaneId` are server-assigned UUIDs, created once, persisted,
  and **immutable** until the workspace/pane is destroyed.
- `PaneId` is the sole key for all durable per-pane state:
  `history/<pane_id>.hist`, `screen/<pane_id>.snap`,
  `scrollback/<pane_id>.log`.
- A pane's PTY/shell is ephemeral and may be respawned any number of times under
  the same `PaneId`.

### 2. Canonical workspace tree (server-owned, durable)

`Runtime` is renamed `Workspace` and gains the authoritative tree, moved out of
the client:

```text
PaneTree =
  | Leaf  { pane: PaneId }
  | Split { axis: Horizontal | Vertical, ratio: f32,
            first: PaneTree, second: PaneTree }

Workspace {
  id: WorkspaceId,
  name: String,
  policy: Persistent | Ephemeral,
  tree: PaneTree,                  // structure + logical ratios (durable)
  default_active: PaneId,          // fallback focus for a fresh attach
  panes: Map<PaneId, PaneState>,   // shell/runtime state per leaf
}
```

`tree` (structure, logical ratios as fractions, ordering, default active) is
durable workspace state, persisted via the RFC-022 mechanism under a new schema
version.

### 3. Per-client viewport (ephemeral, client-local)

Attachment is role-aware: one client holds the write lease and any number hold
read-only attachments. A reader — including a writer that was demoted by an
explicit take-over — renders the same server tree and receives the same deltas,
but every structural command (`SplitPane`, `ClosePane`, `ResizeSplit`) and all
terminal input are refused for it by the daemon. See
[RFC-021 Section 10](RFC-021-client-server-protocol-v3.md#10-ownership-and-multi-client-semantics)
for the ownership and take-over rules.

A client attaching to a workspace holds a **viewport** that is *not* durable and
*not* server-authoritative:

- window/render pixel dimensions,
- focused pane (per client),
- scroll position per pane.

The server tree stores logical ratios; each client renders them to its own pixel
size. Two windows showing the same workspace share the same *structure* but each
focuses/scrolls/sizes independently. This removes the forced-pixel-mirror wart
without reintroducing client-owned structure.

### 4. PTY sizing policy (multi-client)

A pane's PTY has one (cols, rows). Policy:
- Single attached client → PTY tracks that client's rendered size for the pane.
- Multiple attached clients → PTY size = **min** across clients for that pane
  (so no client sees truncated output); clients larger than the PTY letterbox.
  The controlling-client refinement is deferred (Open Q2).

### 5. Protocol (v3 changes)

- `AttachWorkspace { workspace_id }` → `WorkspaceSnapshot { tree, panes[], default_active }`,
  each pane carrying `PaneId` + screen/cwd/title/modes. The client builds its
  entire render layout from this; it holds no local structure.
- Structural mutations — each applied, persisted, and broadcast as a tree delta:
  - `SplitPane { target: PaneId, axis, ratio }` → server mints a new `PaneId`,
    updates the tree, spawns the shell, returns the delta.
  - `ClosePane { pane: PaneId }`
  - `ResizeSplit { split, ratio }` (durable logical ratio)
- Viewport messages (ephemeral, not persisted): `SetFocus { pane }`,
  `ReportClientSize { per-pane render dims }` (drives §4).
- **Removed**: client-identity `CreatePane`. The first pane is created by the
  server at `CreateWorkspace`. All pane identity is server-assigned.

### 6. Persistence (extends RFC-022)

- New `WorkspaceFileV2`: durable `tree` (structure + ratios + default active) +
  per-pane spec keyed by `PaneId`. Bump `RUNTIME_FILE_SCHEMA_VERSION`.
- Clean break: old-schema files are ignored and removed on first load. No
  migration code path.

### 7. Shell history (durable + shell-aware, G4)

Replace `PROMPT_COMMAND=history -a` env injection with per-shell init keyed on
`PaneId`, robust against the user's rc:

- **bash** → `--rcfile <generated>` that sources `~/.bashrc`, then sets
  `HISTFILE=history/<pane_id>.hist` and *appends* `history -a` to
  `PROMPT_COMMAND` (never overwrites).
- **zsh** → generated `ZDOTDIR` whose `.zshrc` sources the user's config, then
  sets `HISTFILE` + `setopt INC_APPEND_HISTORY`.
- **fish** → per-session history file via a sourced snippet.
- **other** → set `HISTFILE` best-effort; documented limitation.

On respawn the shell loads `HISTFILE` at startup (verified behavior), so
up-arrow / Ctrl-R history persists across crashes.

### 8. What is deleted (no legacy left behind)

- Client `LayoutNode` ownership of structure and its persistence;
  `pane_bindings`, `reconcile_bindings`, recovery logic
  (`workspace/state.rs`, `workspace_state.rs`, `runtime.rs`).
- Server `Uuid::new_v4()` pane minting on client request.
- The orphan-sweep behavior that hid the bug → replaced by explicit
  close-driven cleanup keyed on tree membership.
- `RuntimeFileV1.command_history`.
- The bash-only `PROMPT_COMMAND` env hack.
- v3 `CreatePane` as a client-identity operation; redundant flat-pane messaging.
- `Runtime` terminology → `Workspace` everywhere.

### 9. Cutover

Client and server ship together (one repo, one version). The refactor lands on a
branch as coordinated phases (Development Plan), server-first then client, with
the protocol changed in one cohesive step. No transitional dual-protocol period;
no compatibility shims.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1   | `PaneId` minted once, persisted in the tree, immutable; respawn never changes it. |
| G2   | Server owns `PaneTree`; client holds no identity/structure. |
| G3   | History/scrollback/snapshot keyed on `PaneId`, stable by construction. |
| G4   | Per-shell history init (bash/zsh/fish). |
| G5   | Reattach = read `WorkspaceSnapshot`; no client identity logic. |
| G6   | Viewport (size/focus/scroll) decoupled from the durable tree. |
| G7   | Bindings/reconciliation/client-layout-persistence/random ids deleted; `Workspace` unified. |

---

## Development Plan

- [x] **Step 1** — Server: `Workspace` + `PaneTree` + immutable `PaneId` as
  durable tree nodes; ratios and default-active server-side. *(prereq: —)*
- [x] **Step 2** — Persistence: `WorkspaceFileV2` (tree + per-pane); clean-break
  load (ignore/remove old schema). *(prereq: Step 1)* — #1008, #1028
- [x] **Step 3** — Protocol: `AttachWorkspace`/`WorkspaceSnapshot` with tree;
  `SplitPane`/`ClosePane`/`ResizeSplit` mutations; viewport messages; remove
  client-identity `CreatePane`. *(prereq: Step 1)*
- [x] **Step 4** — Client: render from server tree; per-client viewport; delete
  `pane_bindings`, `reconcile_bindings`, client layout persistence. *(prereq: Step 3)* — #1049
- [x] **Step 5** — Shell history: per-shell rc/ZDOTDIR injection keyed on
  `PaneId`; crash-survival integration tests for bash/zsh/fish. *(prereq: Step 1)* — #1011
- [x] **Step 6** — Delete dead paths (§8); `Runtime`→`Workspace` rename;
  crash-recovery integration test asserting **zero** orphaned state. *(prereq: 1–5)* — #1017, #1018, #1050–#1053
- [x] **Step 7** — One-time histfile salvage utility (separate binary/subcommand,
  not a runtime code path) for users upgrading from the old layout. *(prereq: Step 2)*
  — implemented pre-1.0, then **removed in #1052**: the clean-break reset made
  salvage unnecessary, so no runtime code path remains.

---

## Open Questions

- [x] **Q1 — Shared structure vs mirrored pixels.** Resolved: server owns the
  logical tree; per-client viewport owns size/focus/scroll (Design §3).
- [x] **Q2 — Multi-client PTY sizing refinement.** Resolved: shipped in v1.0.0
  with the min-size policy (§4). A "controlling client" refinement is deferred to
  RFC-029.
- [ ] **Q3 — RFC-029 multi-window.** Each window is a client/viewport; a window
  hosts one workspace; the same workspace may appear in multiple windows
  (shared tree, independent viewports). Confirm this is the intended interaction.
- [ ] **Q4 — Ephemeral-policy workspaces** with no attached clients: confirm
  they terminate on last detach as today, now that structure is server-owned.

---

## References

- RFC-016 (workspace management), RFC-018 (connection state machine),
  RFC-021 (protocol v3), RFC-022 (state storage), RFC-026 (pane clone on split),
  RFC-029 (multi-window).
- Data-loss evidence: pane `b6904221` history orphaned; live pane `612a1064`
  empty; workspace `a2c827e9`.
