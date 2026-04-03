# RFC-010: Maintainability Refactor for Window, Terminal, and Session Boundaries

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Accepted / In Progress  |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

Refactor the current UI orchestration and session/recovery code to reduce module size, remove
duplication, and restore clear boundaries between pure data, GTK wiring, and runtime behavior.

This RFC remains deliberately conservative. It does not propose a rewrite. It proposes small
structural moves that make the code easier to reason about and harder to accidentally bloat.

After the daemon-backed runtime rollout, this RFC is no longer just about maintainability in the
abstract. It is part of the stability plan. The highest-risk regressions now come from client-side
runtime reconciliation, duplicate terminal-behavior policy, and the lack of black-box
client+daemon coverage around startup and restart.

## Current implementation snapshot (2026-04)

Several slices of this RFC are already on `mainline`:

- the repository was consolidated into a monorepo, which removed the cross-repo protocol/daemon
  coordination overhead that had made structural cleanup harder
- `clients/rttx/src/runtime.rs` now holds pure connection-state and workspace-action logic
- `clients/rttx/src/workspace_state.rs` now owns a first extracted slice of pure managed-workspace
  transitions
- `clients/rttx/src/terminal/handle.rs` is the shared terminal abstraction for direct and
  daemon-backed panes
- `window.rs` has been split into `window/mod.rs` and `window/runtime.rs` (#204), moving
  endpoint-event dispatch and managed-workspace rendering hooks into their own module
- `session/layout.rs` has been split into `layout.rs`, `recovery.rs`, and `state.rs` (#205),
  separating layout tree operations from recovery types and persisted state
- terminal search is now wired to VTE's buffer search API (#221), resolving the dead-UI concern
- direct and managed terminals share shortcut policy via a unified input handler (#201)
- the daemon test harness has been consolidated into shared helpers (#219) with polling-based
  assertions instead of fixed sleeps
- AT-SPI2 behavioral UI tests now run in CI on every push and PR (#220)
- the daemon has comprehensive lifecycle, adversarial, and recovery matrix coverage (#144–#153)
- the remaining test gap is the black-box client+daemon GTK path (#185)

---

## Goals

- **G1** — Reduce the size and responsibility surface of `clients/rttx/src/window.rs`
- **G2** — Separate pure layout data from pane recovery/runtime behavior
- **G3** — Extract managed-runtime reconciliation out of ad hoc GTK handlers into explicit,
  testable state transitions
- **G4** — Eliminate obvious duplication in sidebar CRUD, dialog code, and direct-vs-managed
  terminal shortcut policy
- **G5** — Make terminal lifecycle wiring explicit and less error-prone
- **G6** — Keep behavior stable while refactoring internals, with startup/restart regressions
  treated as first-class requirements

## Non-Goals

- **NG1** — No large architectural rewrite or new state-management framework
- **NG2** — No GTK template migration in this RFC
- **NG3** — No feature expansion beyond what is required to support the refactor
- **NG4** — No premature abstraction of every repeated line into a generic helper

---

## Background & Motivation

The project has grown in the right direction functionally, but some core modules are now carrying
too many responsibilities at once.

Concrete pressure points:

- `clients/rttx/src/window.rs` now owns window construction, action registration, signal wiring,
  session
  orchestration, terminal lifecycle, recovery logic, bookmark/command CRUD, sidebars, dialogs,
  notifications, and a large in-file test suite.
- daemon-backed correctness is still concentrated in window-level event handlers instead of a pure
  reducer-style transition layer
- `clients/rttx/src/session/layout.rs` mixes two different domains:
  - pure layout tree structure and transforms
  - pane recovery types plus shell/SSH/tmux command generation
- bookmark and command sidebars are rendered with near-duplicate code paths
- terminal lifecycle behavior is spread across `Window`, `TerminalWidget`, and
  `PersistentPaneView`, which makes retry, child-exit, title-sync, and shortcut behavior easier to
  get wrong
- the client test suite still lacks a black-box layer that drives a real daemon-backed GTK session
  through startup restore and daemon restart

This is a maintainability risk, not an aesthetic one. The project explicitly values stability,
clarity, and long-term maintainability. A large multifunction file invites the exact failure modes
we want to avoid:

- duplicated fixes
- leaky abstractions
- accidental behavior coupling
- AI-assisted code growth that keeps adding more code to the same module

The right move is not to add a layer of abstraction everywhere. The right move is to restore
coherent ownership boundaries.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | No intentional UX change; behavior should remain the same except for bug fixes made safer by the refactor |
| Contributors | Smaller modules, clearer ownership, lower risk when modifying session or terminal logic |
| Packagers | No packaging impact |

---

## Considered Options

### Option A — Keep the current structure and only patch individual issues

**Pros**: Lowest immediate effort.
**Cons**: Reinforces the existing hotspot files and makes future changes more expensive.

### Option B — Incremental refactor around responsibility boundaries

**Pros**: Improves clarity without destabilizing the app. Lets tests stay green throughout the
process. Matches the project's preference for practical, low-risk engineering.
**Cons**: Requires discipline to stop at useful boundaries and avoid over-abstraction.

### Option C — Large rewrite of window/session orchestration

**Pros**: Could yield a cleaner end-state on paper.
**Cons**: High risk, high churn, weak fit for a project that values stability and practical
progress.

---

## Decision

**Chosen option: Option B**

Refactor incrementally, with explicit responsibility boundaries and tight test coverage after each
step.

The guiding rule is:

> Move code into a new module only when that module can own one clear job.

This RFC prefers a few meaningful modules over a maze of tiny helpers.

---

## Design

### Principles

1. Keep pure logic separate from GTK object orchestration.
2. Prefer extracting cohesive chunks over inventing generic frameworks.
3. Reduce repeated tree scans and ad hoc state lookups where a domain method already exists.
4. Keep public APIs small and obvious.
5. Refactors should preserve behavior and pass the current test suite at each step.
6. When choosing between a mechanical cleanup and a seam that improves daemon-backed correctness,
   prefer the correctness seam first.

### 1. Split `window.rs` by responsibility

`Window` should remain the central object, but not the central file for every behavior.

Proposed internal module split:

- `clients/rttx/src/window/mod.rs`
  - object definition
  - `glib::wrapper!`
  - top-level construction entry points
- `clients/rttx/src/window/build.rs`
  - header bar and main window widget construction
  - left/right sidebar layout assembly
- `clients/rttx/src/window/actions.rs`
  - action registration
  - accelerator wiring
- `clients/rttx/src/window/runtime.rs`
  - endpoint connection-manager wiring
  - endpoint-event dispatch entry points
  - workspace/runtime status updates
  - managed workspace open/inventory/reconcile rendering hooks
- `clients/rttx/src/window/sessions.rs`
  - add/switch/close session
  - split/close/rebuild session content
  - sidebar row bookkeeping
- `clients/rttx/src/window/recovery.rs`
  - terminal recovery lookup and retry flow
  - bookmark/command execution to `PaneRecovery`
  - child-exit recovery handling
- `clients/rttx/src/window/sidebars.rs`
  - bookmark sidebar rendering
  - command sidebar rendering
  - delete confirmation dialogs

This is not a new architecture. It is the same `Window` type with its methods grouped into files
that reflect ownership.

The important update is that managed-runtime behavior should be treated as its own boundary, not as
"just another session helper". The daemon-backed path now carries enough correctness risk that
`window/runtime.rs` should exist even if other module splits stay partially in `window.rs` for a
while.

### 2. Split layout tree code from recovery/runtime code

`clients/rttx/src/session/layout.rs` currently mixes:

- layout tree operations
- session/window state structs
- pane recovery types
- runtime shell command generation for recovery targets

Proposed split:

- `clients/rttx/src/session/layout_tree.rs`
  - `LayoutNode`
  - `SplitOrientation`
  - layout transforms and queries
- `clients/rttx/src/session/recovery.rs`
  - `PaneSource`
  - `PaneTarget`
  - `PaneRecovery`
  - `StartupStep`
  - shell/ssh/tmux command generation
- `clients/rttx/src/session/state.rs`
  - `SessionState`
  - `WindowState`
  - persistence helpers and normalization helpers
- `clients/rttx/src/session/mod.rs`
  - re-exports and filesystem save/load entry points

The key design rule is simple:

- `layout_tree` should know nothing about shell commands
- `recovery` should know nothing about GTK widgets

### 3. Make terminal lifecycle responsibilities explicit

`TerminalWidget` should own terminal-local behavior. `Window` should own cross-terminal and
cross-session orchestration.

That implies:

- title synchronization should be wired once per terminal widget, not per spawn attempt
- retry-related launch state should be explicit and complete
- search UI should either be fully implemented inside `TerminalWidget` or removed until it is
  implemented

Recommended cleanup inside `TerminalWidget`:

- add explicit handler storage for title-sync if a signal connection is needed
- move all spawn-related state into a small internal lifecycle block
- expose narrow methods such as:
  - `spawn_shell_if_needed()`
  - `queue_input(...)`
  - `show_recovery_error(...)`
  - `clear_recovery_error()`

Avoid exposing raw internal widgets unless the window genuinely needs them.

### 3a. Share direct and managed terminal shortcut policy

The project now has one product-level terminal model, but direct and managed panes still encode
overlapping shortcut policy in separate implementations.

Recommended cleanup:

- move modifier normalization and shortcut classification into shared terminal logic
- keep direct/managed transport differences separate from shortcut policy
- make managed keyboard forwarding tests and direct smart-clipboard tests assert the same contract
- treat lock-modifier handling, accelerator pass-through, and shell-input forwarding as shared
  requirements

### 4. Deduplicate sidebar CRUD row construction

Bookmark and command sidebars share the same structure:

- clear list
- filter saved items
- build `ActionRow`
- add primary action buttons
- add a small edit/delete menu
- wire callbacks
- toggle empty state

This should be reduced to shared composition, not forced generic abstraction.

Recommended shape:

- one small helper for a standard sidebar row shell:
  - title
  - subtitle
  - suffix buttons
  - overflow menu
- per-domain logic remains local:
  - bookmark-specific actions stay bookmark-specific
  - command-specific actions stay command-specific

The goal is to remove repeated UI scaffolding, not to erase the domain distinction.

### 5. Replace ad hoc tree scans with domain methods

Where the code already has a domain method like `contains_terminal`, use it consistently instead of
building temporary UUID vectors and checking membership.

This applies especially to:

- locating the session for a terminal
- input-sync forwarding
- split/close session mutations

This change is small, but it matters because repeated ad hoc queries are how domain logic starts to
leak into unrelated modules.

### 6. Move test modules closer to the code they validate

`window.rs` currently carries a very large in-file test block. That makes the file even harder to
navigate.

Recommended approach:

- keep focused unit tests next to the refactored module they validate
- keep integration-style GTK behavior tests in `tests/`
- avoid one monolithic test section tied to a giant implementation file

This keeps production code readable without losing local test coverage.

---

## Refactoring Sequence

The order matters. Start with the steps that reduce risk and create cleaner seams for later work.

### Phase 1 — Stability seams for daemon-backed correctness

1. Complete inventory-driven recovered-workspace synthesis and regression coverage.
2. Extract endpoint-event -> workspace/runtime reconciliation into pure tested logic before further
   widening GTK handlers.
3. Share direct/managed terminal shortcut policy and add regression coverage for lock modifiers,
   accelerators, and shell-input forwarding.
4. Add black-box client+daemon integration coverage for startup restore, daemon restart, selection
   sync, and managed reattach behavior.

### Phase 2 — Safe correctness cleanup

1. Fix terminal retry/title-sync lifecycle so repeated recovery attempts do not accumulate signal
   handlers.
2. Replace remaining `terminal_uuids().contains(...)` lookups in `window.rs` with
   `contains_terminal(...)`.
3. Extract the sidebar row lookup helper used by session close/update paths.

### Phase 3 — Mechanical file split of `window.rs`

1. Move action registration into `src/window/actions.rs`
2. Move endpoint wiring and managed-runtime rendering hooks into `src/window/runtime.rs`
3. Move bookmark/command sidebar rendering into `src/window/sidebars.rs`
4. Move session mutation/rebuild logic into `src/window/sessions.rs`
5. Move recovery logic into `src/window/recovery.rs`

This phase should be mostly mechanical and behavior-preserving once the daemon-backed correctness
seams already exist.

### Phase 4 — Session model separation

1. Split `session/layout.rs` into layout tree, state, and recovery modules
2. Update call sites to import through `session/mod.rs`
3. Keep serialized schema stable unless a separate RFC intentionally changes it

### Phase 5 — Terminal API tightening and CI follow-through

1. Reduce direct `imp()` access from `window.rs` where possible
2. Make `TerminalWidget` and `PersistentPaneView` expose intent methods instead of raw widget
   details
3. Either implement terminal search properly inside the terminal widgets or remove the inactive UI
4. Promote daemon-backed stability coverage into normal CI gates, including the UI path

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 | `window.rs` is split by coherent responsibilities |
| G2 | layout tree and recovery/runtime code are separated |
| G3 | endpoint-event reconciliation becomes a pure tested transition layer |
| G4 | duplicated sidebar and terminal-policy scaffolding is reduced |
| G5 | terminal lifecycle state becomes explicit and local |
| G6 | the plan is incremental and test-driven rather than rewrite-heavy |

---

## Development Plan

- [ ] **Step 1** — Complete inventory-driven recovered-workspace synthesis with regression coverage (`#184`) *(prerequisite: —)*
- [ ] **Step 2** — Extract endpoint-event reconciliation into a pure tested workspace/runtime reducer (`#186`) *(prerequisite: Step 1)*
- [x] **Step 3** — Share direct/managed terminal shortcut policy and add regression coverage (`#187`, `#201`) *(prerequisite: Step 2)*
- [ ] **Step 4** — Add black-box client+daemon GTK integration coverage for restore/restart/selection sync (`#185`) *(prerequisite: Steps 1–3 can land incrementally)*
- [ ] **Step 5** — Fix terminal retry/title-sync lifecycle and remaining low-risk correctness cleanup *(prerequisite: Step 2)*
- [ ] **Step 6** — Split `window.rs` into `build`, `actions`, `runtime`, `sessions`, `recovery`, and `sidebars` modules (`#98`, `#204` partial) *(prerequisite: Steps 1–5)*
- [x] **Step 7** — Split `session/layout.rs` into layout tree, state, and recovery modules (`#101`, `#205`) *(prerequisite: Step 6)*
- [x] **Step 8** — Tighten the terminal widget API and finish or remove inactive search UI (`#24`, `#221`) *(prerequisite: Step 7)*
- [x] **Step 9** — Promote daemon-backed stability coverage into routine CI gating (`#109`→`#220`, `#147`→`#209`, `#153`→`#219`) *(prerequisite: Step 4)*

---

## Open Questions

- [x] Should terminal search be finished as part of the refactor, or explicitly removed until the feature is real?
  **Resolved**: Implemented in #221. Search entry is now wired to VTE's `search_set_regex`/`search_find_next`/`search_find_previous`.
- [x] Do we want `clients/rttx/src/window/` as a module directory immediately, or only after
  Phase 3 extracts enough code to justify it?
  **Resolved**: `window/` is already a module directory with `mod.rs` and `runtime.rs` (#204). Further splits will add files to this directory.

---

## References

- `clients/rttx/src/window.rs`
- `clients/rttx/src/terminal/widget.rs`
- `clients/rttx/src/terminal/persistent_widget.rs`
- `clients/rttx/src/session/layout.rs`
- `clients/rttx/src/runtime.rs`
- `clients/rttx/src/workspace_state.rs`
- GitHub issues #24, #98, #101, #109, #132, #147, #153, #183, #184, #185, #186, and #187
