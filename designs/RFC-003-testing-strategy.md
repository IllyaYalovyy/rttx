# RFC-003: GTK/Rust Crash-Taxonomy Testing Strategy

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Accepted                |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

rttx uses a crash-taxonomy-driven testing approach rather than code-coverage metrics. GTK is a C
library; failures at the Rust/C boundary produce segfaults and process aborts, not Rust panics.
The test suite is organized around specific crash categories and is designed to catch each class
of GTK-Rust failure as a failing test before it reaches a user.

## Current implementation snapshot (2026-03)

- The project is now a monorepo, so the live test surfaces span:
  - `clients/rttx/` for GTK/unit/integration/UI tests
  - `services/rttx-server/` for daemon/unit/integration tests
  - `protocols/rttx-proto/` for protocol framing/unit tests
- Known GTK-heavy suites now run as ignored tests by default, so plain `cargo test --workspace`
  remains a supported baseline command without crashing in the stock Rust harness.
- The CI-equivalent local command remains `bash .github/scripts/run-quality-tests.sh`, which still
  provides the broader curated matrix and isolated GTK client test selection.
- Pull request CI now includes a diff-aware runtime behavior gate for tracked daemon/runtime/UI
  reconciliation paths. Those changes must add both a pure-state regression test and an
  integration or black-box regression test.
- `run_ui_tests.sh` and the AT-SPI suite exist and run locally/in CI-style environments, but the
  nightly GitHub job is still tracked separately.
- The highest-value remaining testing gaps are now deeper client+daemon end-to-end recovery
  coverage and the daemon adversarial test backlog now tracked in the monorepo issue set
  (`#144`–`#153`).

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

**Unit tests** (`clients/rttx/src/session/layout.rs`, `clients/rttx/src/config.rs`,
`clients/rttx/src/color_scheme.rs`, `clients/rttx/src/preferences.rs`,
`services/rttx-server/src/session.rs`, `services/rttx-server/src/protocol.rs`)
— data model correctness, serialization, tree invariants. No GTK required.

**Property-based tests** (`proptest`) — randomized layout trees with guaranteed UUID uniqueness.
Found a real duplicate-UUID bug in early development.

**Integration tests** (`clients/rttx/tests/session_lifecycle.rs`,
`clients/rttx/tests/color_scheme_compat.rs`, `services/rttx-server/tests/*.rs`) — end-to-end
persistence, Tilix color scheme compatibility, daemon lifecycle, reconnect, ownership, revisions,
and runtime policy coverage.

**GTK contract tests** (`clients/rttx/tests/gtk_boundary_contracts.rs`) — validate data-model
assumptions that `window.rs` relies on. These are the C1/C6/C7 defenses: UUID uniqueness, terminal
count consistency, split/remove invariants, serialization roundtrips, backward compatibility.

**GTK widget tests** (`clients/rttx/tests/gtk_widget_tests.rs`,
`clients/rttx/tests/layout_widget_tests.rs`, `clients/rttx/tests/terminal_lifecycle_tests.rs`) —
instantiate real GTK4 widgets via broadway backend. Test
exact Stack→Paned→unparent→rebuild sequences, GObject ref-count survival, and Paned position
application after allocation.

### Headless execution

```bash
bash .github/scripts/run-quality-tests.sh
```

For focused local client-only runs, `GDK_BACKEND=broadway GTK_A11Y=none cargo test -p rttx`
remains useful. Tests that run on non-GTK threads are skipped via `std::sync::Once` +
`std::panic::catch_unwind` pattern (not `OnceLock<bool>`, which would dispatch GTK calls from
non-main threads).

### Coverage map

| Crash category | Contract tests | Widget tests | Proptest |
| --- | --- | --- | --- |
| C1 — parent/child invariant | ✓ | ✓ | — |
| C2 — RefCell re-entrancy | fix in code | — | — |
| C3 — use-after-free | ✓ | ✓ | — |
| C4 — signal doubling | — | — | — |
| C5 — GLib source leaks | — | — | — |
| C6 — index out of bounds | partial | — | — |
| C7 — duplicate UUID | ✓ | — | ✓ |

### Highest-value remaining gaps

- **GAP1** — Make the client GTK suite deterministic under the plain Rust harness, not just the
  curated Broadway quality script
- **GAP2** — Add more real client+daemon restart/reconcile coverage now that both live in one repo
- **GAP3** — Expand daemon adversarial tests: malformed protocol input, persistence failure
  injection, race-heavy ownership, stress/load, and leak-oriented lifecycle loops

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — crash class coverage | Taxonomy C1–C7; each category has dedicated test layer |
| G2 — requirement validation | Tests assert user-visible invariants; tautological tests explicitly excluded |
| G3 — headless CI | Broadway GDK backend; `Once` + `catch_unwind` pattern for thread safety |

---

## Development Plan

- [x] Unit + proptest suite for layout model
- [x] GTK contract tests (C1, C3, C7, C6 partial)
- [x] GTK widget tests (C1, C3)
- [x] Paned position regression tests (nested split goes dark)
- [x] Active-index bounds contract coverage
- [x] Activity indicator timer regression coverage
- [x] Nested split ratio restore regression coverage
- [x] Runtime-affecting PR gate requiring pure-state plus behavior-layer evidence
- [x] AT-SPI behavioral UI tests running in CI on every PR (Weston headless)
- [ ] Deterministic plain-harness GTK execution
- [ ] Deeper client+daemon restart/reconcile integration coverage
- [ ] Expanded daemon adversarial test matrix

---
