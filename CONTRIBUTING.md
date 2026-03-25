# Contributing to rttx

Thank you for contributing to rttx. This document covers everything you need to submit high-quality
changes — from environment setup through code standards, testing requirements, and the design
process. Read it fully before opening a pull request.

---

## Table of Contents

- [Project philosophy](#project-philosophy)
- [What belongs in rttx](#what-belongs-in-rttx)
- [Development environment](#development-environment)
- [Building and running](#building-and-running)
- [Code standards](#code-standards)
- [Testing requirements](#testing-requirements)
- [Commit and branch conventions](#commit-and-branch-conventions)
- [Pull request process](#pull-request-process)
- [Design process (RFCs)](#design-process-rfcs)
- [Filing issues](#filing-issues)

---

## Project philosophy

rttx has five core principles (see `designs/RFC-001-manifesto.md` for the full rationale):

1. **Native GNOME integration over cross-platform portability** — Libadwaita widgets, GNOME HIG,
   system light/dark mode. No portability shims.
2. **Rock-solid stability over feature breadth** — A crashing terminal is worse than no terminal.
   Every critical path has tests.
3. **Workflow context over layout geometry** — Per-pane recovery recipes reconstruct what the user
   was doing, not just the shape of the window.
4. **Composable building blocks over monolithic workflows** — Bookmarks, commands, and templates
   are distinct but composable concepts.
5. **Practical tools over impressive features** — Every feature must answer: does this help a
   developer or sysadmin get real work done faster?

6. **Infrastructure serves the application, never the reverse** — Do not downgrade dependencies,
   remove features, or lower version requirements to satisfy CI runners, old distributions, or
   test environments. If CI fails because a runner lacks a library, fix the runner. If a
   distribution ships an outdated package, use a different distribution or build from source.
   The application's requirements are ground truth. Everything else adapts.

Contributions that conflict with these principles will not be accepted regardless of implementation
quality.

---

## What belongs in rttx

**In scope:**
- Features that serve developers and sysadmins who live in a GNOME terminal all day
- Improvements to session recovery, split management, and bookmark/command workflows
- GNOME HIG compliance and Libadwaita integration
- Test coverage for existing or new crash categories (see `designs/RFC-003-testing-strategy.md`)
- Bug fixes with regression tests

**Permanently out of scope:**
- Quake/drop-down mode
- Client/server or daemon architecture
- Custom scripting or macro language
- Cross-platform support (Windows, macOS)
- Remote GUI or web interface

When in doubt, open an issue to discuss before writing code.

---

## Development environment

**Required:**
- Rust 1.85+ (edition 2024)
- GTK4 4.14+
- Libadwaita 1.5+
- VTE 0.78+ (GTK4 variant)

**Fedora:**
```bash
sudo dnf install cargo meson pkg-config gtk4-devel libadwaita-devel vte291-gtk4-devel
```

**Ubuntu/Debian:**
```bash
sudo apt install cargo meson pkg-config libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev
```

**For UI behavioral tests** (optional but required for changes touching GTK layout or widget interaction):
- `weston` — headless Wayland compositor
- `python3-gobject` / `python3-atspi` — AT-SPI2 Python bindings

```bash
# Fedora
sudo dnf install weston python3-gobject

# Ubuntu/Debian
sudo apt install weston python3-gi gir1.2-atspi-2.0
```

---

## Building and running

```bash
cargo build
./target/debug/rttx
```

The build runs `rustfmt` and Clippy automatically via a build script. A build that does not pass
Clippy is not a valid build.

For a release build:
```bash
cargo build --release
./target/release/rttx
```

For full system integration (desktop file, icons, AppStream metadata):
```bash
meson setup build --prefix="$HOME/.local"
meson install -C build
gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor"
update-desktop-database "$HOME/.local/share/applications"
```

---

## Code standards

### Rust and Clippy

rttx enforces Clippy at the **pedantic + nursery** level. Every warning is a build error. The
`Cargo.toml` lists the small set of allowed exceptions; do not add new `#[allow(...)]` attributes
without a comment explaining why the specific lint does not apply.

`unsafe` code is denied project-wide. Do not use it.

### Style

- Format with `rustfmt` before every commit. The build enforces this.
- Write self-documenting code. If a comment feels necessary to explain *what* the code does,
  rewrite the code until it does not. Comments are reserved for non-obvious *why* — a workaround
  rationale, a non-obvious invariant, or a link to an external spec.
- Avoid over-engineering. Only make changes that are directly requested or clearly necessary.
  Three similar lines are better than a premature abstraction.
- Do not add error handling for scenarios that cannot happen. Trust internal invariants and
  framework guarantees. Validate only at system boundaries (user input, external APIs, file I/O).

### GTK/GObject patterns

- Never hold a `RefCell` borrow across a point where a GTK signal could fire. This is the primary
  cause of `BorrowMutError` panics in GTK-Rust applications.
- Disconnect signal handlers before destroying a widget.
- Do not set `GtkPaned` positions before the widget is realized; use `connect_realize` for
  deferred position application.
- Prefer `gtk4::prelude::*` and `libadwaita::prelude::*` wildcard imports (explicitly allowed
  in `Cargo.toml`) over per-trait imports for GTK types.

### UI and UX

- Follow the [GNOME Human Interface Guidelines](https://developer.gnome.org/hig/).
- Use Libadwaita widgets (`adw::ActionRow`, `adw::Toast`, `adw::AlertDialog`, etc.) wherever an
  Adwaita equivalent exists for a GTK primitive.
- Destructive actions (delete, close with running processes) require an `adw::AlertDialog`
  confirmation.
- Errors visible to the user are surfaced as `adw::Toast` notifications, not modal dialogs or
  console output.

---

## Testing requirements

rttx uses a **crash-taxonomy-driven** testing approach (see `designs/RFC-003-testing-strategy.md`).
Coverage percentage is not a goal. Every test must answer: *which specific failure does this
test prevent?*

### Test layers

| Layer | Location | Purpose |
|---|---|---|
| Unit tests | `src/**` (inline `#[cfg(test)]`) | Data model, serialization, tree invariants. No GTK. |
| Property-based tests | `src/session/layout.rs` | Randomized layout trees via `proptest`. |
| Integration tests | `tests/session_lifecycle.rs`, etc. | End-to-end persistence and compatibility. |
| GTK contract tests | `tests/gtk_boundary_contracts.rs` | Data-model invariants that `window.rs` depends on. |
| GTK widget tests | `tests/gtk_widget_tests.rs`, etc. | Real GTK4 widget instantiation and widget-tree structure. |
| **Behavioral UI tests** | `tests/ui/` (Python + AT-SPI2) | Functional behaviour observed through the accessibility tree. |

The behavioral layer exists because silent functional failures — a blank pane after split, a
horizontal sidebar that should be vertical — do not cause crashes and are invisible to the Rust
test layers. AT-SPI2 observes the live widget tree the same way a screen reader would, catching
layout and interaction regressions that unit tests cannot.

### Running tests

Standard (no display required for pure-Rust tests):
```bash
cargo test
```

Full suite including GTK widget tests:
```bash
GDK_BACKEND=broadway GTK_A11Y=none cargo test
```

Behavioral UI tests (requires `weston` and `python3-gobject`):
```bash
cargo build && ./run_ui_tests.sh
```

The UI tests launch a private `RTTX_DEV_MODE=1` instance on a headless weston compositor. They
are safe to run while rttx is open for normal work — the dev instance uses a separate D-Bus name
(`io.github.IllyaYalovyy.rttx.Devel`) and a throwaway config directory.

### Requirements for new code

- **Bug fixes** must include a regression test that fails before the fix and passes after.
- **New features** must include tests covering the primary success path and any GTK-boundary
  interactions the feature introduces.
- **New crash categories** must be added to the crash taxonomy in `designs/RFC-003-testing-strategy.md`.
- Tautological tests (tests that assert the code does what it obviously does, without validating a
  user-visible invariant or a specific crash category) will be rejected.

### GTK widget test pattern

Tests that require GTK must guard against headless environments:

```rust
static GTK_INIT: Once = Once::new();

fn ensure_gtk_init() -> bool {
    // ... Once + catch_unwind pattern (see existing tests for the full implementation)
}

macro_rules! require_display {
    () => {
        if !ensure_gtk_init() {
            eprintln!("SKIPPED: no display available");
            return;
        }
    };
}
```

Do not use `OnceLock<bool>` for GTK initialization; GTK requires all calls to originate from the
main thread and `OnceLock` can allow cross-thread dispatch.

---

## Commit and branch conventions

### Branches

- Base all work on `mainline`.
- Use short descriptive branch names: `feat/session-templates`, `fix/paned-ratio-restore`,
  `refactor/pane-source-module`.

### Commit messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>: <short imperative summary>

<optional body — explain why, not what>
```

**Types:** `feat`, `fix`, `refactor`, `test`, `docs`, `build`, `chore`

**Rules:**
- Summary line: 72 characters maximum, imperative mood, no trailing period.
- Use the body to explain motivation and context when it is not obvious from the diff.
- Every pushed commit must reference the tracked GitHub issue in a footer.
- If the commit fully resolves the issue, use a GitHub closing keyword in that footer so the issue
  closes automatically when the commit lands on the default branch. Preferred forms:
  `Fixes #123` or `Closes #123`.
- If the commit is only partial work, use a non-closing footer such as `Refs #123`.
- One logical change per commit. Do not mix feature work with unrelated cleanup.
- Do not amend or force-push commits that have been pushed to a shared branch.

**Examples:**
```
feat: resizable session and tools sidebars via GtkPaned

Replace OverlaySplitView and Revealer with Paned widgets so users can
drag either divider to their preferred width. Widths persist across
restarts via WindowState.

Fixes #42
```

```
fix: restore right sidebar width after restart

right_paned position must be set in connect_realize because the total
allocated width is not known until the widget is mapped.

Refs #57
```

---

## Pull request process

1. **Open an issue first** for any non-trivial change. Align on scope before writing code.
2. **One PR, one concern.** Do not bundle unrelated changes.
3. **All checks must pass:**
   - `cargo build` (enforces `rustfmt` + Clippy pedantic/nursery)
   - `cargo test` (pure-Rust and GTK widget tests)
   - `./run_ui_tests.sh` (for changes touching GTK layout, widget interaction, or the split/sidebar paths)
4. **PR description** must explain:
   - What the change does and why
   - How to manually verify it
   - Which tests cover it
5. **Backward compatibility:** changes to persisted state (`WindowState`, `SessionState`,
   `LayoutNode`, preferences) must use `#[serde(default)]` so existing saved files continue to
   load without error.
6. **Breaking changes to public data structures** require updating all construction sites in the
   codebase — do not leave compile errors for the reviewer to fix.
7. A PR that breaks any existing test will not be merged.

---

## Design process (RFCs)

Significant changes — new subsystems, changes to the data model, changes to persistence format,
new UI patterns, or anything that affects the project's core principles — require an RFC before
implementation begins.

RFCs live in `designs/`. Use `designs/RFC-000-template.md` as the starting point. Number
sequentially from the last existing RFC.

An RFC is required when:
- A new data structure is added to the persisted state
- A new UI pattern or interaction model is introduced
- An existing core behaviour is changed in a user-visible way
- A permanent scope boundary (see [What belongs in rttx](#what-belongs-in-rttx)) is being
  reconsidered

An RFC is not required for:
- Bug fixes
- Refactors that do not change behaviour
- New tests
- Documentation updates
- Small, self-contained features with an obvious implementation

RFC status values: `Draft` → `Review` → `Accepted` → `Implemented` (or `Superseded`).
Implementation should not begin until the RFC reaches `Accepted`.

---

## Filing issues

Use [GitHub Issues](https://github.com/IllyaYalovyy/rttx/issues).

**Bug reports** should include:
- rttx version or commit hash
- Steps to reproduce
- Expected vs. actual behaviour
- Whether the issue is reproducible and how frequently

**Feature requests** should explain the workflow problem being solved, not just the desired
solution. rttx is opinionated — a feature request framed as "I want X" is less useful than
"when I do Y, I have to Z, which takes too long."
