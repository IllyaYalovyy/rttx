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

## Current implementation snapshot (2026-04)

Development mode is fully implemented across both the GUI client and the daemon. Activation is a
single environment variable (`RTTX_DEV_MODE=1`) that isolates every runtime path.

**Client (rttx):**
- App ID: `io.github.IllyaYalovyy.rttx.Devel`
- Config dir: `$XDG_CONFIG_HOME/rttx-devel/`
- Log dir: `$XDG_CACHE_HOME/rttx-devel/`
- Default log level: `debug` (production: `rttx=info,warn`)
- Visual indicators: window title `rttx (Devel)`, header bar pill badge, distinct icon

**Daemon (rttx-server):**
- Socket: `$XDG_RUNTIME_DIR/rttx-server-devel/v1/`
- State/cache: `$XDG_CACHE_HOME/rttx-server-devel/`
- Log dir: `$XDG_CACHE_HOME/rttx-server-devel/`
- Default log level: `debug` (production: `info`)

**Integration:**
- The GUI propagates `RTTX_DEV_MODE=1` to the daemon when auto-starting it, so a single env var
  on `cargo run -p rttx` activates dev mode for the entire stack.
- The client resolves the daemon socket path through the same dev-mode flag, connecting to the dev
  daemon automatically.

The `AppProfile` struct in `clients/rttx/src/config.rs` centralizes all identity and path
decisions. The daemon uses a parallel `dir_name_for(is_dev)` helper in
`services/rttx-server/src/os/unix.rs`.

Docs in `README.md` and `CONTRIBUTING.md` document both client and daemon dev mode together
because a safe dev run requires both sides to stay isolated from production state.

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
RTTX_DEV_MODE=1 cargo run -p rttx
RTTX_DEV_MODE=1 cargo run -p rttx-server -- start --foreground
```

This is the primary supported workflow because it is simple, shell-friendly, and does not require
special command-line parsing in the GTK application startup path.

The variable is checked with `std::env::var_os` and treated as enabled when the value is non-empty
and not `"0"`.

When the GUI auto-starts the daemon (via `daemon_bridge.rs`), it propagates `RTTX_DEV_MODE=1` to
the child process so a single env var on the client activates dev mode for the entire stack.

### Runtime profile abstraction

A `const fn` profile constructor centralizes all identity and path decisions in
`clients/rttx/src/config.rs`:

```rust
pub struct AppProfile {
    pub app_id: &'static str,
    pub icon_name: &'static str,
    pub display_name: &'static str,
    pub config_dir: &'static str,
    pub settings_id: &'static str,
    pub settings_path: &'static str,
    pub badge_label: Option<&'static str>,
    pub is_development: bool,
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
- `badge_label = Some("Devel")`

This profile is the single source of truth for:

- `adw::Application::builder().application_id(...)`
- window title and visible labeling
- icon name
- XDG config path resolution for preferences, sessions, bookmarks, commands, and schemes
- GSettings schema ID and path (for future migration)

### Daemon-side isolation

The daemon mirrors the client's dev-mode pattern with its own directory helpers in
`services/rttx-server/src/os/unix.rs`:

| Resource | Production | Development |
|---|---|---|
| Socket | `$XDG_RUNTIME_DIR/rttx-server/v1/` | `$XDG_RUNTIME_DIR/rttx-server-devel/v1/` |
| State/cache | `$XDG_CACHE_HOME/rttx-server/` | `$XDG_CACHE_HOME/rttx-server-devel/` |
| Logs | `$XDG_CACHE_HOME/rttx-server/` | `$XDG_CACHE_HOME/rttx-server-devel/` |

The daemon checks `RTTX_DEV_MODE` at startup and logs `"Running in DEVELOPMENT mode"` with the
resolved paths. The single-instance lock is per-directory, so a dev daemon and a production daemon
can run simultaneously without conflict.

### Logging isolation

Both the GUI and daemon write logs to dev-specific directories when in dev mode:

| Component | Production log dir | Development log dir | Default level |
|---|---|---|---|
| GUI (`rttx`) | `$XDG_CACHE_HOME/rttx/` | `$XDG_CACHE_HOME/rttx-devel/` | `debug` (prod: `rttx=info,warn`) |
| Daemon (`rttx-server`) | `$XDG_CACHE_HOME/rttx-server/` | `$XDG_CACHE_HOME/rttx-server-devel/` | `debug` (prod: `info`) |

`RUST_LOG` overrides the default level in both modes. See RFC-017 for the full logging design.

### App identity

The dev application ID differs from production so `cargo run` launches a separate process even
while the installed app is running.

Dev app ID:

```text
io.github.IllyaYalovyy.rttx.Devel
```

This preserves reverse-DNS shape and stays clearly derived from the production ID.

### Config isolation

Development mode uses a distinct XDG config directory:

```text
~/.config/rttx-devel
```

Everything that writes under `~/.config/rttx/...` resolves through the active profile:

- preferences
- session state
- bookmarks / places
- commands
- color schemes

The rule is strict: production and development state do not mix.

### Visual indication

Development mode is visible at a glance through multiple signals:

1. **Window title**: `rttx (Devel)`
2. **Header bar badge**: a small `Devel` pill with accent styling and a tooltip explaining that
   development mode uses a separate app profile
3. **Distinct icon**: `io.github.IllyaYalovyy.rttx.Devel` — visually separable in the dock, app
   switcher, and GNOME overview

The marker is obvious but restrained. The visual cue prevents category errors: typing in the wrong
window, assuming production state is being tested, or forgetting that settings were created in the
dev profile.

### Scope boundaries

This RFC intentionally does not add:

- a second `.desktop` file
- packaging logic for a devel build

Those are optional later conveniences. The primary contributor workflow is terminal-first and
already centered on `cargo run`.

### Relationship to future GSettings

The current app uses JSON files, but `config.rs` carries `SETTINGS_ID` and `SETTINGS_PATH`
constants alongside their dev-mode counterparts (`DEV_SETTINGS_ID`, `DEV_SETTINGS_PATH`). The
`AppProfile` struct includes `settings_id` and `settings_path` fields so a future GSettings
migration can derive distinct schema paths from the active profile without restructuring.

### Testing strategy

The feature is covered at the requirement level:

- profile selection returns production defaults when `RTTX_DEV_MODE` is unset
- profile selection returns dev values when `RTTX_DEV_MODE=1`
- config path builders use `rttx-devel` in dev mode
- application title and visible label reflect the profile
- settings path follows the runtime settings ID for both profiles
- development icon asset exists at the expected path
- daemon path helpers produce `rttx-server-devel` directories in dev mode

No test requires a real installed copy of rttx. The contract is profile-driven path and app
identity selection.

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — parallel installed and dev runs | Distinct application ID and daemon socket path |
| G2 — isolated state | Separate config, cache, log, and socket directories for both client and daemon |
| G3 — visible dev build | Window title suffix, header badge pill, and distinct icon |
| G4 — daily convenience | Single env var activation: `RTTX_DEV_MODE=1 cargo run -p rttx` with automatic daemon propagation |
| G5 — future compatibility | `AppProfile` struct owns identity, path, and GSettings decisions |

---

## Development Plan

- [x] **Profile abstraction** — `AppProfile` struct centralizes profile selection in `config.rs`
- [x] **Application identity** — active profile's app ID used in app startup
- [x] **Config path plumbing** — all XDG config path builders route through the active profile
- [x] **Visual dev labeling** — `rttx (Devel)` title and header badge pill
- [x] **Distinct dev icon** — development-only icon asset at `io.github.IllyaYalovyy.rttx.Devel`
- [x] **Daemon isolation** — separate socket, state, cache, and log directories for dev daemon
- [x] **Daemon auto-start propagation** — GUI propagates `RTTX_DEV_MODE=1` when starting daemon
- [x] **Logging isolation** — separate log directories and debug-level defaults in dev mode
- [x] **Tests** — unit coverage for profile selection, path isolation, daemon paths, and dev icon
- [x] **Docs** — contributor workflow documented in `README.md` and `CONTRIBUTING.md`

---

## Open Questions

- [x] **Q1** — Development mode uses a distinct icon in addition to title + badge
- [x] **Q2** — Activation kept simple: `RTTX_DEV_MODE` only; no granular override layer

---

## References

- [designs/RFC-000-template.md](RFC-000-template.md)
- [designs/RFC-001-manifesto.md](RFC-001-manifesto.md)
- [designs/RFC-013-persistent-host-sessions.md](RFC-013-persistent-host-sessions.md) — daemon architecture that dev mode isolates
- [designs/RFC-017-logging.md](RFC-017-logging.md) — logging design with dev-mode directory separation
