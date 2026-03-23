# RFC-010: Maintainability Refactor for Window, Terminal, and Session Boundaries

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Draft                   |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

Refactor the current UI orchestration and session/recovery code to reduce module size, remove
duplication, and restore clear boundaries between pure data, GTK wiring, and runtime behavior.
This RFC is deliberately conservative. It does not propose a rewrite. It proposes small structural
moves that make the code easier to reason about and harder to accidentally bloat.

---

## Goals

- **G1** — Reduce the size and responsibility surface of `src/window.rs`
- **G2** — Separate pure layout data from pane recovery/runtime behavior
- **G3** — Eliminate obvious duplication in sidebar CRUD and dialog code
- **G4** — Make terminal lifecycle wiring explicit and less error-prone
- **G5** — Keep behavior stable while refactoring internals

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

- `src/window.rs` now owns window construction, action registration, signal wiring, session
  orchestration, terminal lifecycle, recovery logic, bookmark/command CRUD, sidebars, dialogs,
  notifications, and a large in-file test suite.
- `src/session/layout.rs` mixes two different domains:
  - pure layout tree structure and transforms
  - pane recovery types plus shell/SSH/tmux command generation
- bookmark and command sidebars are rendered with near-duplicate code paths
- terminal lifecycle behavior is spread across `Window` and `TerminalWidget`, which makes retry,
  child-exit, and title-sync behavior easier to get wrong

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

### 1. Split `window.rs` by responsibility

`Window` should remain the central object, but not the central file for every behavior.

Proposed internal module split:

- `src/window/mod.rs`
  - object definition
  - `glib::wrapper!`
  - top-level construction entry points
- `src/window/build.rs`
  - header bar and main window widget construction
  - left/right sidebar layout assembly
- `src/window/actions.rs`
  - action registration
  - accelerator wiring
- `src/window/sessions.rs`
  - add/switch/close session
  - split/close/rebuild session content
  - sidebar row bookkeeping
- `src/window/recovery.rs`
  - terminal recovery lookup and retry flow
  - bookmark/command execution to `PaneRecovery`
  - child-exit recovery handling
- `src/window/sidebars.rs`
  - bookmark sidebar rendering
  - command sidebar rendering
  - delete confirmation dialogs

This is not a new architecture. It is the same `Window` type with its methods grouped into files
that reflect ownership.

### 2. Split layout tree code from recovery/runtime code

`src/session/layout.rs` currently mixes:

- layout tree operations
- session/window state structs
- pane recovery types
- runtime shell command generation for recovery targets

Proposed split:

- `src/session/layout_tree.rs`
  - `LayoutNode`
  - `SplitOrientation`
  - layout transforms and queries
- `src/session/recovery.rs`
  - `PaneSource`
  - `PaneTarget`
  - `PaneRecovery`
  - `StartupStep`
  - shell/ssh/tmux command generation
- `src/session/state.rs`
  - `SessionState`
  - `WindowState`
  - persistence helpers and normalization helpers
- `src/session/mod.rs`
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

### Phase 1 — Safe correctness cleanup

1. Fix terminal retry/title-sync lifecycle so repeated recovery attempts do not accumulate signal
   handlers.
2. Replace remaining `terminal_uuids().contains(...)` lookups in `window.rs` with
   `contains_terminal(...)`.
3. Extract the sidebar row lookup helper used by session close/update paths.

### Phase 2 — Mechanical file split of `window.rs`

1. Move action registration into `src/window/actions.rs`
2. Move bookmark/command sidebar rendering into `src/window/sidebars.rs`
3. Move session mutation/rebuild logic into `src/window/sessions.rs`
4. Move recovery logic into `src/window/recovery.rs`

This phase should be mostly mechanical and behavior-preserving.

### Phase 3 — Session model separation

1. Split `session/layout.rs` into layout tree, state, and recovery modules
2. Update call sites to import through `session/mod.rs`
3. Keep serialized schema stable unless a separate RFC intentionally changes it

### Phase 4 — Terminal API tightening

1. Reduce direct `imp()` access from `window.rs` where possible
2. Make `TerminalWidget` expose intent methods instead of raw widget details
3. Either implement terminal search properly inside `TerminalWidget` or remove the inactive UI

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 | `window.rs` is split by coherent responsibilities |
| G2 | layout tree and recovery/runtime code are separated |
| G3 | bookmark/command sidebar scaffolding is deduplicated |
| G4 | terminal lifecycle state becomes explicit and local |
| G5 | the plan is incremental and test-driven rather than rewrite-heavy |

---

## Development Plan

- [ ] **Step 1** — Fix terminal retry/title-sync lifecycle and add regression coverage *(prerequisite: —)*
- [ ] **Step 2** — Replace remaining ad hoc terminal membership scans with domain methods *(prerequisite: Step 1)*
- [ ] **Step 3** — Extract session/sidebar helpers that already have obvious seams *(prerequisite: Step 2)*
- [ ] **Step 4** — Split `window.rs` into `build`, `actions`, `sessions`, `recovery`, and `sidebars` modules *(prerequisite: Step 3)*
- [ ] **Step 5** — Split `session/layout.rs` into layout tree, state, and recovery modules *(prerequisite: Step 4)*
- [ ] **Step 6** — Tighten the `TerminalWidget` API and finish or remove inactive search UI *(prerequisite: Step 5)*

---

## Open Questions

- [ ] Should packaging issues keep `packaging` only, or also carry `area/integration` for filtering consistency?
- [ ] Should terminal search be finished as part of the refactor, or explicitly removed until the feature is real?
- [ ] Do we want `src/window/` as a module directory immediately, or only after Phase 2 extracts enough code to justify it?

---

## References

- `src/window.rs`
- `src/terminal/widget.rs`
- `src/session/layout.rs`
- `META/todo.md`
