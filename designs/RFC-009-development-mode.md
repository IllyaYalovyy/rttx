# RFC-009: Development Mode for Parallel Installed and Local Runs

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented             |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

Add an explicit development mode so a locally built `cargo run` instance can run alongside the
installed application without stealing activation, sharing state, or being mistaken for the
production build. Development mode uses a distinct application identity, a separate config root,
and an unmistakable visual indicator. This is not cosmetic. It removes day-to-day friction for
contributors and makes the project materially easier to develop, test, and adopt.

---

## Goals

- **G1** — A dev instance can run at the same time as the installed app
- **G2** — Dev runs do not read or overwrite production sessions, bookmarks, commands, or preferences
- **G3** — The window clearly tells the user it is a dev build
- **G4** — Activation is simple enough for daily use during feature work
- **G5** — The design remains compatible with future GSettings and single-instance work

## Non-Goals

- **NG1** — Not adding a permanent second desktop application entry for development mode
- **NG2** — Not supporting arbitrary multi-profile runtime management in v1
- **NG3** — Not making dev mode the default for normal `rttx` launches
- **NG4** — Not solving multiple-production-window behavior; this RFC is about dev-vs-installed coexistence

---

## Background & Motivation

Today the installed app and a `cargo run` build share the same GNOME application ID and the same
XDG config directory. That creates two concrete problems:

1. A locally run build activates the already-running installed instance instead of launching its
   own process, because GTK/GApplication treats them as the same single-instance app.
2. Even if forced to launch separately, both builds would share `~/.config/rttx`, so development
   work can mutate real sessions, preferences, bookmarks, and commands.

That is the wrong ergonomic baseline for a project that wants contributions. rttx is aimed at
developers and sysadmins. Convenient development is not an internal nicety; it is part of the
product's adoption strategy. If running the dev build is annoying or risky, fewer people will test
changes, fewer people will contribute, and more work will pile up behind fear of breaking a daily
driver.

The project manifesto already frames rttx as a practical tool for developers. The same standard
should apply to developing rttx itself.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | No change to normal installed behavior |
| Contributors | Can keep the installed app open while running a local dev build with isolated state |
| Packagers | No packaging change required for the production app; dev mode is runtime-only |

---

## Considered Options

### Option A — Visual label only

Add `rttx (Devel)` to the title bar or show a badge, but keep the same application ID and config
directory.

**Pros**: Very small implementation.
**Cons**: Does not solve the real problem. The installed instance still captures activation, and
the dev build still shares production state.

### Option B — Different application ID only

Use a different app ID for dev runs so both processes can run at once, but keep the same config
directory.

**Pros**: Solves the single-instance collision.
**Cons**: Still dangerous. A dev build can overwrite real sessions and preferences. Visual
distinction is still weak.

### Option C — Full development profile: distinct app ID, isolated config root, explicit visual marker

Dev mode becomes an intentional runtime profile with:

- a different application ID
- a different config directory
- visible UI labeling
- a simple activation mechanism

**Pros**: Solves both functional collisions and human error. Supports daily feature work.
**Cons**: Slightly more plumbing across config and application setup.

### Option D — Separate compile-time dev binary / crate feature

Build a separate `rttx-devel` artifact with different constants baked in.

**Pros**: Very explicit separation.
**Cons**: More build complexity than needed. Makes the common contributor workflow heavier, not
lighter. The problem is runtime identity, not binary architecture.

---

## Decision

**Chosen option: Option C**

Development mode is a runtime profile, activated explicitly, with its own application identity and
state root. The key principle is that dev mode must be safe enough to use casually every day. If a
contributor has to think hard before `cargo run`, the design failed.

---

## Design

### Activation model

Development mode is enabled by an environment variable:

```bash
RTTX_DEV_MODE=1 cargo run
```

This is the primary supported workflow because it is simple, shell-friendly, and does not require
special command-line parsing in the GTK application startup path.

The application may later expose more granular environment overrides, but v1 should keep one clear
switch:

- `RTTX_DEV_MODE=1` → development profile
- unset / any other value → production profile

### Runtime profile abstraction

Introduce a small runtime profile abstraction in config/application code rather than scattering
`if dev mode` checks across the codebase.

Example conceptual shape:

```rust
struct AppProfile {
    app_id: &'static str,
    config_dir: &'static str,
    display_name: &'static str,
    is_development: bool,
}
```

Production profile:

- `app_id = "io.github.IllyaYalovyy.rttx"`
- `config_dir = "rttx"`
- `display_name = "rttx"`

Development profile:

- `app_id = "io.github.IllyaYalovyy.rttx.Devel"`
- `config_dir = "rttx-devel"`
- `display_name = "rttx (Devel)"`

This profile should become the single source of truth for:

- `adw::Application::builder().application_id(...)`
- window title / visible labeling
- icon name if needed
- XDG config path resolution in preferences, sessions, bookmarks, commands, and schemes

This is preferable to ad-hoc environment lookups in every module.

### App identity

The dev application ID must differ from production so `cargo run` launches a separate process even
while the installed app is running.

Recommended dev app ID:

```text
io.github.IllyaYalovyy.rttx.Devel
```

This preserves reverse-DNS shape and stays clearly derived from the production ID.

### Config isolation

Development mode uses a distinct XDG config directory:

```text
~/.config/rttx-devel
```

Everything that currently writes under `~/.config/rttx/...` must instead resolve through the
active profile:

- preferences
- session state
- bookmarks
- commands
- color schemes

The rule is strict: production and development state do not mix.

### Visual indication

Development mode should be visible at a glance. One signal is not enough; users stop noticing a
small title suffix after a while. The UI should include:

1. Window title: `rttx (Devel)`
2. Header-level visual marker: a small `Devel` badge or pill in the header bar
3. Distinct development icon name so the app is visually separable in the dock, app switcher, and
   GNOME overview

The marker should be obvious but restrained. This is a tool window, not a warning dialog.

The visual cue exists to prevent category errors:

- typing in the wrong window
- assuming production state is being tested
- forgetting that settings/bookmarks were created in the dev profile

### Scope boundaries

This RFC intentionally does not add:

- a second `.desktop` file
- packaging logic for a devel build

Those are optional later conveniences. The primary contributor workflow is terminal-first and
already centered on `cargo run`.

### Relationship to future GSettings

The current app uses JSON files, but `src/config.rs` already carries `SETTINGS_ID` and
`SETTINGS_PATH`. Development mode should be designed so a future GSettings migration stays
straightforward:

- production settings schema ID/path remain production-specific
- development mode can later derive a distinct settings ID/path if and when GSettings is adopted

The profile abstraction should therefore be designed around app identity, not only JSON paths.

### Testing strategy

The feature should be covered at the requirement level:

- profile selection returns production defaults when `RTTX_DEV_MODE` is unset
- profile selection returns dev values when `RTTX_DEV_MODE=1`
- config path builders use `rttx-devel` in dev mode
- application title / visible label reflect the profile
- a dev-mode session save/load roundtrip does not touch production paths

No test should require a real installed copy of rttx. The contract is profile-driven path and app
identity selection.

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — parallel installed and dev runs | Distinct application ID for development mode |
| G2 — isolated state | Separate config directory root `rttx-devel` |
| G3 — visible dev build | Window title suffix plus header badge |
| G4 — daily convenience | Single env var activation: `RTTX_DEV_MODE=1 cargo run` |
| G5 — future compatibility | Runtime profile abstraction owns identity and path decisions |

---

## Development Plan

- [x] **Profile abstraction** — add a runtime `AppProfile` and centralize profile selection
- [x] **Application identity** — use the active profile's application ID in app startup
- [x] **Config path plumbing** — route all XDG config path builders through the active profile
- [x] **Visual dev labeling** — add `rttx (Devel)` title and a visible header badge
- [x] **Distinct dev icon** — use a development-only icon name/asset so the shell chrome also distinguishes the profile
- [x] **Tests** — add unit coverage for profile selection, path isolation, and dev icon asset presence
- [x] **Docs** — document the contributor workflow in `README.md`

---

## Open Questions

- [x] **Q1** — Development mode should use a distinct icon in addition to title + badge
- [x] **Q2** — Keep activation simple in v1: `RTTX_DEV_MODE` only; no granular override layer yet

---

## References

- [designs/RFC-000-template.md](RFC-000-template.md)
- [designs/RFC-001-manifesto.md](RFC-001-manifesto.md)
