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

**Unit tests** (`src/session/layout.rs`, `src/config.rs`, `src/color_scheme.rs`, `src/preferences.rs`)
— data model correctness, serialization, tree invariants. No GTK required.

**Property-based tests** (`proptest`) — randomized layout trees with guaranteed UUID uniqueness.
Found a real duplicate-UUID bug in early development.

**Integration tests** (`tests/session_lifecycle.rs`, `tests/color_scheme_compat.rs`) — end-to-end
persistence, Tilix color scheme compatibility.

**GTK contract tests** (`tests/gtk_boundary_contracts.rs`) — validate data-model assumptions that
window.rs relies on. These are the C1/C6/C7 defenses: UUID uniqueness, terminal count consistency,
split/remove invariants, serialization roundtrips, backward compatibility.

**GTK widget tests** (`tests/gtk_widget_tests.rs`, `tests/layout_widget_tests.rs`,
`tests/terminal_lifecycle_tests.rs`) — instantiate real GTK4 widgets via broadway backend. Test
exact Stack→Paned→unparent→rebuild sequences, GObject ref-count survival, and Paned position
application after allocation.

### Headless execution

```bash
GDK_BACKEND=broadway GTK_A11Y=none cargo test
```

`broadwayd :5` serves as a framebuffer. Tests that run on non-GTK threads are skipped via
`std::sync::Once` + `std::panic::catch_unwind` pattern (not `OnceLock<bool>`, which would
dispatch GTK calls from non-main threads).

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

### Missing tests (priority order)

- **M3** — `active_session_index` out-of-bounds safety (pure data, no display)
- **M2** — RefCell re-entrancy proof with `#[should_panic]` counterpart
- **M1** — Signal handler doubling: split preserves existing UUID for widget reuse
- **M7** — `disconnect_child_exited` before VTE destruction
- **M4** — GObject weak reference invalidated after last strong ref drop
- **M5** — Extreme ratios (0.1, 0.9) produce non-zero Paned positions
- **M8** — `SessionColor` backward compat (add alongside that feature)
- **M6** — Activity timer safe after session close (add alongside activity detection)

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
- [ ] **M3** — `active_session_index` bounds contract test — *tracked in todo.md — Stability, Testing & Maintenance*
- [ ] **M2** — RefCell re-entrancy proof
- [ ] **M1** — Signal handler doubling / widget reuse contract
- [ ] **M7** — `child_exited` disconnect test
- [ ] **M4** — Weak reference lifecycle test
- [ ] **M5** — Extreme ratio Paned position test

---
