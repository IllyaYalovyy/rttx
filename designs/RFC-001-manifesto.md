# RFC-001: rttx Project Manifesto

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Accepted                |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## What rttx Is

rttx is a tiling terminal emulator for GNOME, written in Rust, built as a spiritual successor to
Tilix. Where Tilix is stuck on GTK3 and D, rttx starts from scratch with GTK4, Libadwaita, and
VTE4. The defining features — sidebar workspaces and split-screen panes — are preserved and deepened.
Everything else is reconsidered from first principles.

---

## Core Principles

### 1. Native GNOME integration over cross-platform portability

rttx is built for one platform and one desktop. It uses Libadwaita widgets, follows the GNOME HIG,
respects system light/dark mode, and integrates with system notifications. No abstraction layers,
no portability shims, no cross-platform UI toolkits.

### 2. Rock-solid stability over feature breadth

A terminal emulator that crashes is worse than no terminal at all. Every critical code path is
covered by tests. Crashes at the Rust/C boundary — the place most GTK-Rust apps silently fail —
are caught by a dedicated contract-testing layer before they reach users.

### 3. Workflow context over layout geometry

Saving a grid of terminal panes is not workspace recovery. rttx persists per-pane recovery
recipes and daemon-backed runtime state so it can reconstruct what the user was doing, not just
the shape of the window they were doing it in. This includes working directories, startup
provenance, SSH targets, reconnect state, and tmux sessions where applicable.

### 4. Composable building blocks over monolithic workflows

Bookmarks, commands, and workspace templates are distinct but composable. A bookmark sets context
(where you are). A command executes work (what you do). A template composes both into a
ready-to-use working environment. These remain separate concepts in the data model and the UI.

### 5. Practical tools over impressive features

Every feature must answer the question: does this help a developer or sysadmin get real work done
faster? URL detection, smart clipboard, searchable command launcher — yes. Custom scripting
languages, remote GUI, Quake mode — no.

---

## Target User

rttx is built for **developers and sysadmins** who:

- Live in the terminal for most of their workday
- Use GNOME as their primary desktop
- Work across multiple contexts simultaneously (local projects, SSH hosts, tmux sessions)
- Value a tool that stays out of the way until they need it

rttx is **not** for:

- Users who want a minimal single-window terminal (use GNOME Terminal)
- Users who need cross-platform portability (use Alacritty, WezTerm, or Kitty)
- Users who want a terminal embedded inside an IDE (use the IDE's built-in)
- Users who want a Quake-style drop-down terminal

---

## Scope Boundaries

These are permanent, not just deferred:

- **No Quake/drop-down mode** — the use case exists, but it conflicts with the GNOME HIG and the
  focus on daemon-backed workspaces
- **No product-level direct-terminal architecture** — managed local and remote execution goes
  through the daemon-backed runtime model rather than maintaining a separate first-class direct
  path
- **No custom scripting or macro language** — shell variables and shell functions already exist;
  rttx does not invent a second scripting layer
- **No cross-platform support** — Windows and macOS are explicitly out of scope; GNOME-specific
  APIs are first-class citizens, not abstractions
- **No remote GUI or web interface** — rttx is a local desktop application

---

## Goals Alignment

| Principle                      | Capability already present or on roadmap                                            |
|--------------------------------|-------------------------------------------------------------------------------------|
| Native GNOME integration       | `adw::ToolbarView`, `adw::ActionRow`, `adw::Toast`, system light/dark mode, `gio::Notification`, GNOME HIG-compliant layout |
| Rock-solid stability           | Unit, proptest, integration, GTK contract, GTK widget, and behavioral UI tests; crash taxonomy with dedicated test categories |
| Workflow context over geometry | Per-pane recovery recipes, daemon-backed runtimes, CWD persistence, pane origin tracking, startup replay, SSH/tmux reconnect roadmap |
| Composable building blocks     | Bookmarks (context), commands (actions), workspace templates (roadmap); right utility sidebar separates them from workspace nav |
| Practical tools                | Smart clipboard, searchable command launcher, searchable bookmark sidebar, workspace renaming, input sync |
