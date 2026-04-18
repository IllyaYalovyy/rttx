# RFC-003: GTK/Rust Crash-Taxonomy Testing Strategy

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented             |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

rttx uses a crash-taxonomy-driven testing approach rather than code-coverage metrics. GTK is a C
library; failures at the Rust/C boundary produce segfaults and process aborts, not Rust panics.
The test suite is organized around specific crash categories and is designed to catch each class
of GTK-Rust failure as a failing test before it reaches a user.

## Current implementation snapshot (2026-04)

- The project is a monorepo with test surfaces spanning:
  - `clients/rttx/` — GTK/unit/integration/UI tests (25+ unit test modules, 16 integration test
    files, 8 AT-SPI behavioral UI tests)
  - `services/rttx-server/` — daemon unit/integration tests (13 unit test modules, 40+
    integration test files)
  - `protocols/rttx-proto/` — protocol framing/unit tests
- GTK-heavy suites run as `#[ignore]` tests so plain `cargo test --workspace` stays reliable.
  The quality script (`run-quality-tests.sh`) runs each ignored test in its own process with
  Broadway.
- GTK client tests are deterministic under the plain Rust harness (#222 resolved). The
  `std::sync::Once` + `catch_unwind` pattern ensures GTK initialization is main-thread-only.
- Pull request CI includes:
  - **Runtime behavior gate** — diff-aware policy enforcing both a pure-state regression test and
    an integration or behavioral regression test for tracked daemon/runtime/UI reconciliation
    paths
  - **Quality tests** — full Clippy, library, binary, integration, and doc test matrix via
    Broadway
  - **UI behavioral tests** — AT-SPI2 suite on headless Weston, running on every PR
  - **Coverage reporting** — `cargo-llvm-cov` for rttx-server and rttx-proto (GTK client excluded
    due to display server requirements)
  - **Memory profiling gate** — application-level leak detection via diagnostics protocol
  - **Flatpak manifest validation**
- The daemon adversarial test backlog (#144–#153) is fully resolved: negative protocol input,
  persistence failure injection, ownership races, recovery matrix, scale/stress, PTY chaos,
  persistence compatibility, stdio failure paths, lifecycle leak loops, and shared test harness
  utilities are all implemented.
- Client+daemon end-to-end integration coverage (#185) is implemented with black-box GTK tests
  for restore, restart, and selection sync.
- All crash taxonomy categories (C1–C7) now have test coverage. C4 (signal doubling) is covered
  by contract tests (UUID preservation) and GTK widget tests (rebuild reuse verification). C5
  (GLib source leaks) is covered by widget tests verifying weak-ref timer safety and source
  cancellation.

---

## Goals

- **G1** — Every known GTK-Rust crash class is covered by at least one test that fails when the corresponding invariant is violated
- **G2** — Tests validate requirements (what the user expects), not implementation (what the code does)
- **G3** — Widget tests run headless in CI via the broadway GDK backend

## Non-Goals

- **NG1** — Coverage percentage is not a primary metric; a 100%-covered test suite with tautological tests provides false confidence
- **NG2** — Real-environment tests (actual SSH, actual tmux, Flatpak sandbox) are out of scope for automated CI

---

## Background & Motivation

A typical Rust project can rely on the compiler and the type system to eliminate large classes of
bugs. GTK-Rust breaks this assumption. The `gtk4-rs` bindings expose a GObject system with
reference-counted heap objects, synchronous signal dispatch, and C-defined parent/child widget
invariants. Violations don't return `Err` — they abort the process or silently corrupt state.

Three crash categories were already experienced in rttx development before the test suite existed:
RefCell re-entrancy from `child_exited`, `stack.remove` failing on a single-terminal session, and
nested Paned positions set to zero because `connect_realize` fires before the outer Paned has
been allocated. These are documented in git history and now have regression tests.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | Fewer crashes; regressions caught before release |
| Contributors | Clear taxonomy of what can go wrong; test stubs for new crash categories |
| Packagers | CI can run the full suite headless without a display server |

---

## Considered Options

### Option A — Coverage-metrics-driven testing *(reconstructed)*

Write tests to achieve a target line coverage percentage. Use `cargo-tarpaulin` or similar.

**Pros**: Measurable; easy to gate CI on.
**Cons**: Coverage metrics incentivize tautological tests that mirror the code. A test that calls
`split_terminal` and checks it returns a split is useless — it doesn't test anything the type
system didn't already guarantee. High coverage with bad tests gives false confidence.

### Option B — Crash-taxonomy-driven testing

Enumerate the ways GTK-Rust programs die. Write one test per crash category. Tests document the
invariant and prove the fix holds.

**Pros**: Every test has a clear reason to exist. A new contributor can read the crash taxonomy
and immediately understand what the tests are protecting. Regressions produce clear failure
messages.
**Cons**: Requires discipline to maintain the taxonomy as new features are added.

---

## Decision

Chosen option: B

Tautological tests provide coverage numbers but not safety. The crash taxonomy approach forces
every test to answer the question: "which specific way does GTK crash if this invariant is
violated?" Tests that can't answer that question don't belong in the suite.

---

## Design

### Crash taxonomy

| ID | Category | Manifestation |
| --- | --- | --- |
| C1 | GTK parent/child invariant violation | `g_return_if_fail` → process abort |
| C2 | RefCell re-entrancy | `BorrowMutError` → Rust panic |
| C3 | GObject use-after-free | Segfault or UB |
| C4 | Signal handler doubling | Logic corruption → eventual crash |
| C5 | GLib source leaks | Timer fires after owning object destroyed → crash |
| C6 | Index out of bounds on session state | Rust panic on startup |
| C7 | Duplicate UUID in layout tree | Silent widget leak → zombie PTY |

### Test layers

**Unit tests** (`clients/rttx/src/**`, `services/rttx-server/src/**`, `protocols/rttx-proto/src/`)
— data model correctness, serialization, tree invariants, daemon state, protocol framing. No GTK
required. Client modules with inline tests include `session/layout.rs`, `session/state.rs`,
`session/recovery.rs`, `config.rs`, `color_scheme.rs`, `preferences.rs`, `commands.rs`,
`places.rs`, `host.rs`, `workspace_state.rs`, `runtime.rs`, `daemon.rs`, `daemon_bridge.rs`,
`sidebar.rs`, `terminal/links.rs`, `terminal/paste_guard.rs`, `terminal/mod.rs`,
`terminal/widget.rs`, `terminal/persistent_widget.rs`, `window/mod.rs`,
`connect_existing_dialog.rs`, `new_workspace_dialog.rs`, and `host_tag_picker.rs`. Server modules
with inline tests include `session.rs`, `protocol.rs`, `serialization.rs`, `server.rs`, `pane.rs`,
`screen.rs`, `pty.rs`, `ipc.rs`, `diagnostics.rs`, `logging.rs`, `single_instance.rs`,
`os/unix.rs`, and `main.rs`.

**Property-based tests** (`proptest`) — randomized layout trees with guaranteed UUID uniqueness
(`clients/rttx/src/session/layout.rs`). Found a real duplicate-UUID bug in early development.

**Integration tests** — end-to-end persistence, compatibility, daemon lifecycle, and runtime
behavior:

- *Client* (`clients/rttx/tests/`): `session_lifecycle.rs`, `color_scheme_compat.rs`,
  `commands_integration.rs`, `places_integration.rs`, `preferences_integration.rs`,
  `reconnect_layout_stability.rs`, `reconnect_scheduling.rs`, `retry_connection.rs`,
  `ssh_connection.rs`, `stale_terminal_cleanup.rs`, `vte_parity.rs`
- *Daemon* (`services/rttx-server/tests/`): PTY I/O and chaos (`pty_basic.rs`, `pty_io.rs`,
  `pty_chaos.rs`, `pty_coalesce.rs`), lifecycle (`session_lifecycle.rs`, `client_lifecycle.rs`,
  `lifecycle_leaks.rs`, `lifecycle_logging.rs`, `shutdown.rs`, `clean_sessions.rs`), persistence
  (`serialization.rs`, `persistence_compat.rs`, `persistence_failures.rs`, `scrollback.rs`,
  `exited_pane_scrollback.rs`), reconnect and recovery (`reconnect.rs`, `reconstruction.rs`,
  `recovery_matrix.rs`, `gui_restore_flow.rs`), ownership and concurrency (`ownership.rs`,
  `ownership_races.rs`, `lock_free_broadcast.rs`, `writer_priority.rs`, `bounded_channels.rs`),
  protocol and transport (`negative_protocol.rs`, `stdio_transport.rs`, `stdio_failures.rs`,
  `managed_input_parity.rs`, `dsr_response.rs`, `dsr_stripped_from_client.rs`,
  `device_attributes.rs`, `colorfgbg.rs`), runtime policy (`runtime_policy.rs`,
  `make_pane_persistent.rs`), scale and stress (`scale_stress.rs`, `buffer_capacity.rs`),
  diagnostics and observability (`diagnostics.rs`, `memory_cleanup.rs`,
  `mutex_hold_instrumentation.rs`, `log_context.rs`, `logging_integration.rs`, `heartbeat.rs`,
  `inventory.rs`), and features (`split_cwd.rs`, `cwd_propagation.rs`, `title_propagation.rs`,
  `shell_editing.rs`, `single_instance.rs`)

**GTK contract tests** (`clients/rttx/tests/gtk_boundary_contracts.rs`) — validate data-model
assumptions that `window.rs` relies on. These are the C1/C4/C6/C7 defenses: UUID uniqueness,
terminal count consistency, split/remove invariants, serialization roundtrips, backward
compatibility, signal-doubling prevention on widget reuse, and active-index bounds clamping.

**GTK widget tests** (`clients/rttx/tests/gtk_widget_tests.rs`,
`clients/rttx/tests/layout_widget_tests.rs`, `clients/rttx/tests/terminal_lifecycle_tests.rs`,
`clients/rttx/tests/host_sidebar_tests.rs`) — instantiate real GTK4 widgets via broadway backend.
Test exact Stack→Paned→unparent→rebuild sequences, GObject ref-count survival, Paned position
application after allocation, and host sidebar widget tree structure.

**Behavioral UI tests** (`clients/rttx/tests/ui/`) — Python + AT-SPI2 tests running on headless
Weston. Cover launch, split, zoom, sidebar toggling, sidebar content, workspace close/reorder,
workspace rename, and managed workspace black-box behavior. These catch silent functional failures
invisible to Rust test layers.

### Headless execution

```bash
bash .github/scripts/run-quality-tests.sh
```

The quality script starts a Broadway display server, then runs each `#[ignore]` GTK test in its
own `cargo test` invocation for process isolation. Non-GTK tests run normally.

For focused local client-only runs, `GDK_BACKEND=broadway GTK_A11Y=none cargo test -p rttx`
remains useful. Tests that run on non-GTK threads are skipped via `std::sync::Once` +
`std::panic::catch_unwind` pattern (not `OnceLock<bool>`, which would dispatch GTK calls from
non-main threads).

Behavioral UI tests require Weston and AT-SPI2:

```bash
cargo build -p rttx && ./run_ui_tests.sh
```

### Coverage map

| Crash category | Contract tests | Widget tests | Proptest |
| --- | --- | --- | --- |
| C1 — parent/child invariant | ✓ | ✓ | — |
| C2 — RefCell re-entrancy | fix in code | — | — |
| C3 — use-after-free | ✓ | ✓ | — |
| C4 — signal doubling | ✓ | ✓ | — |
| C5 — GLib source leaks | — | ✓ | — |
| C6 — index out of bounds | ✓ | — | — |
| C7 — duplicate UUID | ✓ | — | ✓ |

C4 now has full coverage: contract tests verify UUID preservation across split/remove (preventing
widget recreation), and GTK widget tests verify that `rebuild_session_content` reuses the same
widget instances across multiple rebuild cycles for both direct and managed workspaces. C5 has
widget tests verifying that the `SessionRow` idle timer uses weak references correctly and that
`clear_activity` cancels the `GLib` source.

### Highest-value remaining gap

All crash taxonomy categories (C1–C7) now have test coverage. The remaining testing gaps are in
expanding behavioral UI test coverage for new features as they are added.

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — crash class coverage | Taxonomy C1–C7; each category has dedicated test layer. All categories covered. |
| G2 — requirement validation | Tests assert user-visible invariants; tautological tests explicitly excluded |
| G3 — headless CI | Broadway GDK backend for widget tests; Weston for AT-SPI behavioral tests |

---

## Development Plan

- [x] Unit + proptest suite for layout model
- [x] GTK contract tests (C1, C3, C6, C7)
- [x] GTK widget tests (C1, C3)
- [x] Paned position regression tests (nested split goes dark)
- [x] Active-index bounds contract coverage
- [x] Activity indicator timer regression coverage
- [x] Nested split ratio restore regression coverage
- [x] Runtime-affecting PR gate requiring pure-state plus behavior-layer evidence (#202)
- [x] AT-SPI behavioral UI tests running in CI on every PR (Weston headless)
- [x] Deterministic plain-harness GTK execution (#222)
- [x] Client+daemon restart/reconcile integration coverage (#185)
- [x] Daemon adversarial test matrix (#144–#153)
- [x] Coverage reporting CI job (`cargo-llvm-cov`)
- [x] Memory profiling CI gate (diagnostics-based leak detection)
- [x] Signal-doubling prevention contract test (C4 partial)
- [x] Full C4/C5 crash taxonomy coverage (#324)

---
