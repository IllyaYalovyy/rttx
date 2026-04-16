# RFC-010: Maintainability Refactor for Window, Terminal, and Session Boundaries

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented             |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

Refactor the UI orchestration and session/recovery code to reduce module size, remove duplication,
and restore clear boundaries between pure data, GTK wiring, and runtime behavior.

This RFC was deliberately conservative. It did not propose a rewrite. It proposed small structural
moves that make the code easier to reason about and harder to accidentally bloat.

After the daemon-backed runtime rollout, this RFC became part of the stability plan. The
highest-risk regressions came from client-side runtime reconciliation, duplicate terminal-behavior
policy, and the lack of black-box client+daemon coverage around startup and restart. All three
areas now have dedicated modules and test coverage.

## Implementation outcome (2026-04)

All development plan steps are complete and merged to `mainline`. The refactor landed
incrementally across multiple PRs without breaking behavior at any step.

### Window module decomposition

`window.rs` was split into a module directory (`clients/rttx/src/window/`) with focused
submodules:

| Module | Responsibility |
|---|---|
| `mod.rs` (~1200 lines) | Object definition, `glib::wrapper!`, construction, session orchestration |
| `runtime.rs` (~1040 lines) | Managed workspace lifecycle, endpoint-event dispatch, reconciliation hooks |
| `actions.rs` (~435 lines) | Action registration and accelerator wiring |
| `sidebar.rs` (~535 lines) | Bookmark and command sidebar rendering, host filtering |
| `terminal.rs` (~720 lines) | Terminal materialization, spawn, lifecycle, and recovery |
| `dialogs.rs` (~700 lines) | Confirmation dialogs, rename, close, and delete flows |
| `input.rs` (~115 lines) | Input sync toggle and broadcast |
| `tests.rs` (~5375 lines) | GTK widget and integration tests for window behavior |

The final split differs from the original proposal in naming and grouping. `build.rs`,
`sessions.rs`, and `recovery.rs` were not created as separate files. Instead, construction stayed
in `mod.rs`, session mutation logic stayed alongside construction, and recovery logic moved into
`terminal.rs` alongside terminal lifecycle. `dialogs.rs` and `input.rs` were added as natural
extraction targets that the original proposal did not anticipate.

### Session model separation

`session/layout.rs` was split into three modules (#205):

| Module | Responsibility |
|---|---|
| `layout.rs` (~1240 lines) | `LayoutNode`, `SplitOrientation`, layout transforms and queries |
| `recovery.rs` (~220 lines) | `PaneSource`, `PaneTarget`, `PaneRecovery`, `StartupStep`, shell/SSH/tmux command generation |
| `state.rs` (~960 lines) | `SessionState`, `WindowState`, persistence and normalization helpers |
| `mod.rs` (~450 lines) | Re-exports and filesystem save/load entry points |

The file was named `layout.rs` rather than the proposed `layout_tree.rs` — the shorter name was
sufficient since the module directory already provides context.

### Pure runtime and workspace-state logic

- `clients/rttx/src/runtime.rs` holds pure connection-state and workspace-action presentation
  logic, decoupled from GTK
- `clients/rttx/src/workspace_state.rs` owns pure managed-workspace transitions
  (`EndpointEventTransition`, `WorkspacePaneRestore`) that `window/runtime.rs` consumes

### Terminal abstraction

- `clients/rttx/src/terminal/handle.rs` is the shared terminal abstraction for direct and
  daemon-backed panes
- Direct and managed terminals share shortcut policy via a unified input handler (#201)
- Terminal search is wired to VTE's buffer search API (#221)

### Test infrastructure

- The daemon test harness uses shared helpers (#219) with polling-based assertions instead of
  fixed sleeps
- AT-SPI2 behavioral UI tests run in CI on every push and PR (#220)
- The daemon has comprehensive lifecycle, adversarial, and recovery matrix coverage (#144–#153)
- Black-box client+daemon GTK integration tests cover startup restore, daemon restart, and
  selection sync (#185)

---

## Goals

All goals have been met:

- **G1** — `window.rs` was split into 8 focused submodules under `window/`
- **G2** — Layout tree, recovery types, and persisted state are in separate session modules
- **G3** — Managed-runtime reconciliation lives in `workspace_state.rs` as pure tested transitions
  consumed by `window/runtime.rs`
- **G4** — Direct and managed terminal shortcut policy is unified; sidebar rendering is in its own
  module; dialog code is extracted
- **G5** — Terminal lifecycle wiring is explicit in `window/terminal.rs` with clear
  materialization, spawn, and recovery paths
- **G6** — Every refactoring step preserved behavior and passed the test suite; startup/restart
  regressions are covered by black-box integration tests

## Non-Goals

These constraints were maintained throughout:

- **NG1** — No large architectural rewrite or new state-management framework was introduced
- **NG2** — No GTK template migration
- **NG3** — No feature expansion beyond what was required to support the refactor
- **NG4** — No premature abstraction of every repeated line into a generic helper

---

## Background & Motivation

The project had grown in the right direction functionally, but core modules were carrying too many
responsibilities at once.

The concrete pressure points that motivated this RFC:

- `window.rs` owned window construction, action registration, signal wiring, session
  orchestration, terminal lifecycle, recovery logic, bookmark/command CRUD, sidebars, dialogs,
  notifications, and a large in-file test suite
- Daemon-backed correctness was concentrated in window-level event handlers instead of a pure
  reducer-style transition layer
- `session/layout.rs` mixed pure layout tree structure with pane recovery types and shell/SSH/tmux
  command generation
- Bookmark and command sidebars were rendered with near-duplicate code paths
- Terminal lifecycle behavior was spread across `Window`, `TerminalWidget`, and
  `PersistentPaneView`
- The client test suite lacked a black-box layer that drove a real daemon-backed GTK session
  through startup restore and daemon restart

All of these have been addressed by the completed refactor.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | No intentional UX change; behavior remained the same throughout the refactor |
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

The guiding rule was:

> Move code into a new module only when that module can own one clear job.

This RFC preferred a few meaningful modules over a maze of tiny helpers. The final result follows
this principle — 8 window submodules, 4 session submodules, each with a clear single
responsibility.

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

### 1. Window module split (completed)

`Window` remains the central object, but its methods are grouped into files that reflect
ownership.

Actual module structure:

- `clients/rttx/src/window/mod.rs` — object definition, `glib::wrapper!`, construction,
  session orchestration, and core window logic
- `clients/rttx/src/window/actions.rs` — action registration and accelerator wiring
- `clients/rttx/src/window/runtime.rs` — managed workspace lifecycle, endpoint connection-manager
  wiring, endpoint-event dispatch, workspace/runtime status updates, reconciliation rendering
  hooks
- `clients/rttx/src/window/sidebar.rs` — bookmark and command sidebar rendering, host filtering
- `clients/rttx/src/window/terminal.rs` — terminal materialization, spawn, lifecycle, recovery
  lookup and retry flow, child-exit handling
- `clients/rttx/src/window/dialogs.rs` — confirmation dialogs for delete, close, and rename flows
- `clients/rttx/src/window/input.rs` — input sync toggle and broadcast
- `clients/rttx/src/window/tests.rs` — GTK widget and integration tests

The split diverged from the original proposal in practical ways: `build.rs` was not needed because
construction logic is tightly coupled with the object definition in `mod.rs`. `sessions.rs` and
`recovery.rs` were not created as separate files because session mutation and recovery logic
grouped more naturally with their primary consumers (`mod.rs` and `terminal.rs` respectively).
`dialogs.rs` and `input.rs` emerged as natural extraction targets during implementation.

### 2. Session model separation (completed)

`session/layout.rs` was split into focused modules:

- `clients/rttx/src/session/layout.rs` — `LayoutNode`, `SplitOrientation`, layout transforms
  and queries
- `clients/rttx/src/session/recovery.rs` — `PaneSource`, `PaneTarget`, `PaneRecovery`,
  `StartupStep`, shell/SSH/tmux command generation
- `clients/rttx/src/session/state.rs` — `SessionState`, `WindowState`, persistence and
  normalization helpers
- `clients/rttx/src/session/mod.rs` — re-exports and filesystem save/load entry points

The design rule held: `layout.rs` knows nothing about shell commands, `recovery.rs` knows nothing
about GTK widgets.

### 3. Terminal lifecycle (completed)

Terminal lifecycle responsibilities are now explicit:

- `TerminalWidget` owns terminal-local behavior (spawn, title sync, search, child-exit)
- `PersistentPaneView` owns daemon-backed pane rendering and reconnection
- `window/terminal.rs` owns cross-terminal orchestration from the window's perspective
- Direct and managed terminals share shortcut policy via a unified input handler

Terminal search is fully implemented and wired to VTE's `search_set_regex` /
`search_find_next` / `search_find_previous` (#221).

### 4. Sidebar and dialog extraction (completed)

Bookmark and command sidebar rendering moved to `window/sidebar.rs`. Dialog code (confirmation,
rename, close) moved to `window/dialogs.rs`. Per-domain logic remains local to each sidebar type
rather than being forced into a generic abstraction.

### 5. Domain method consistency (completed)

Ad hoc tree scans were replaced with domain methods like `contains_terminal` where they already
existed. This applies to terminal lookup, input-sync forwarding, and split/close mutations.

### 6. Test organization (completed)

Window tests live in `window/tests.rs` as a dedicated submodule. Integration-style GTK behavior
tests remain in `clients/rttx/tests/`. The test file is large (~5375 lines) but is a single
cohesive test module rather than being interleaved with production code.

---

## Refactoring Sequence (completed)

All phases landed incrementally on `mainline`.

### Phase 1 — Stability seams for daemon-backed correctness ✅

1. Inventory-driven recovered-workspace synthesis with regression coverage (#184).
2. Endpoint-event reconciliation extracted into pure tested logic in `workspace_state.rs` (#186).
3. Direct/managed terminal shortcut policy unified with regression coverage (#187, #201).
4. Black-box client+daemon integration coverage for startup restore, daemon restart, and selection
   sync (#185).

### Phase 2 — Safe correctness cleanup ✅

1. Terminal retry/title-sync lifecycle fixed.
2. Ad hoc `terminal_uuids().contains(...)` lookups replaced with `contains_terminal(...)`.
3. Sidebar row lookup helpers extracted.

### Phase 3 — Mechanical file split of `window.rs` ✅

Split into `mod.rs`, `actions.rs`, `runtime.rs`, `sidebar.rs`, `terminal.rs`, `dialogs.rs`,
`input.rs`, and `tests.rs` (#98, #204).

### Phase 4 — Session model separation ✅

Split `session/layout.rs` into `layout.rs`, `recovery.rs`, and `state.rs` (#101, #205).

### Phase 5 — Terminal API tightening and CI follow-through ✅

1. Terminal search implemented inside terminal widgets (#24, #221).
2. Daemon-backed stability coverage promoted into routine CI gating (#109→#220, #147→#209,
   #153→#219).

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 | `window.rs` split into 8 focused submodules under `window/` |
| G2 | Layout tree, recovery types, and persisted state in separate session modules |
| G3 | Endpoint-event reconciliation is a pure tested transition layer in `workspace_state.rs` |
| G4 | Unified terminal shortcut policy; sidebar and dialog code extracted into own modules |
| G5 | Terminal lifecycle is explicit in `window/terminal.rs` with clear materialization and recovery paths |
| G6 | Incremental, test-driven execution; startup/restart regressions covered by black-box tests |

---

## Development Plan (completed)

- [x] **Step 1** — Complete inventory-driven recovered-workspace synthesis with regression coverage (`#184`)
- [x] **Step 2** — Extract endpoint-event reconciliation into a pure tested workspace/runtime reducer (`#186`)
- [x] **Step 3** — Share direct/managed terminal shortcut policy and add regression coverage (`#187`, `#201`)
- [x] **Step 4** — Add black-box client+daemon GTK integration coverage for restore/restart/selection sync (`#185`)
- [x] **Step 5** — Fix terminal retry/title-sync lifecycle and remaining low-risk correctness cleanup
- [x] **Step 6** — Split `window.rs` into module directory with `actions`, `runtime`, `sidebar`, `terminal`, `dialogs`, `input` modules (`#98`, `#204`)
- [x] **Step 7** — Split `session/layout.rs` into layout tree, state, and recovery modules (`#101`, `#205`)
- [x] **Step 8** — Tighten the terminal widget API and finish or remove inactive search UI (`#24`, `#221`)
- [x] **Step 9** — Promote daemon-backed stability coverage into routine CI gating (`#109`→`#220`, `#147`→`#209`, `#153`→`#219`)

---

## Resolved Questions

- **Should terminal search be finished as part of the refactor, or explicitly removed until the
  feature is real?**
  Implemented in #221. Search entry is wired to VTE's `search_set_regex` /
  `search_find_next` / `search_find_previous`.

- **Do we want `clients/rttx/src/window/` as a module directory immediately, or only after
  Phase 3 extracts enough code to justify it?**
  `window/` is a module directory with 8 submodules (#204 and subsequent PRs).

---

## References

- `clients/rttx/src/window/mod.rs` (and submodules: `actions.rs`, `runtime.rs`, `sidebar.rs`,
  `terminal.rs`, `dialogs.rs`, `input.rs`, `tests.rs`)
- `clients/rttx/src/terminal/widget.rs`
- `clients/rttx/src/terminal/persistent_widget.rs`
- `clients/rttx/src/terminal/handle.rs`
- `clients/rttx/src/session/layout.rs`
- `clients/rttx/src/session/recovery.rs`
- `clients/rttx/src/session/state.rs`
- `clients/rttx/src/runtime.rs`
- `clients/rttx/src/workspace_state.rs`
- GitHub issues: #24, #98, #101, #109, #132, #144–#153, #183, #184, #185, #186, #187, #201,
  #204, #205, #209, #219, #220, #221
