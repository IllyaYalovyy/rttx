# RFC-015: Workspace Sidebar Row Content Specification

| Field         | Value         |
|---------------|---------------|
| Status        | Implemented   |
| Author(s)     | Illya Yalovyy |
| Supersedes    | —             |
| Superseded by | —             |

---

## Summary

Define exactly what text and icons appear in each workspace sidebar row, for every
workspace type and connection state. This RFC exists because the sidebar content has been
rewritten six times in ten PRs without a stable spec, causing a cycle of regressions.

RFC-002 defined the widget tree (ActionRow with prefix icon, title, subtitle, suffix button)
but left the content rules underspecified. This RFC fills that gap.

## Current implementation snapshot (2026-04)

The sidebar row content spec is fully implemented. The `SessionRow` widget
(`clients/rttx/src/sidebar.rs`) is an `adw::ActionRow` subclass with prefix connection icon,
position label, suffix action button, and a single-line subtitle.

**Subtitle pipeline:**
- `pane_description()` in `clients/rttx/src/runtime.rs` computes per-pane text: CWD
  (tilde-collapsed via `collapse_home`) is preferred; when no CWD is available, the VTE title
  is used as a fallback after filtering generic titles via `is_generic_title()`.
- `workspace_connection_summary()` wraps the pane description with the host prefix for remote
  endpoints (`{host} · {pane_info}`).
- `refresh_sidebar_subtitle()` in `clients/rttx/src/window/sidebar.rs` drives the update on
  CWD changes, title changes, focus changes, and connection status changes.

**Title generation:**
- `workspace_display_name()` in `clients/rttx/src/session/state.rs` sets the initial title:
  CWD basename for local, short hostname for remote, `Workspace N` fallback.
- `maybe_auto_rename_workspace()` in `clients/rttx/src/window/runtime.rs` auto-renames
  workspaces on CWD change unless the user has manually renamed (tracked by `user_renamed`
  field on `SessionState`).

**Connection icon:**
- `connection_icon()` in `clients/rttx/src/runtime.rs` maps endpoint type to icon shape and
  connection status to CSS color class.
- The icon is a `gtk4::Image` (16px) added as a prefix to the `ActionRow`.

**Activity indicator:**
- Three-state model (`None` → `Active` → `Idle`) with CSS-driven left bar animation.
- Debounced: repeated output resets the idle timer (1200ms production, 30ms tests).

**Test coverage:**
- AT-SPI UI tests in `clients/rttx/tests/ui/test_sidebar_content.py` verify subtitle
  compliance (forbidden patterns, path format) and icon presence for both direct and managed
  workspaces.
- Rust unit tests in `clients/rttx/src/sidebar.rs` cover icon visibility, CSS class switching,
  tooltip updates, activity state transitions, position labels, and suffix button styling.
- Rust unit tests in `clients/rttx/src/runtime.rs` cover `pane_description`,
  `workspace_connection_summary`, `connection_icon`, and `is_generic_title`.

---

## Goals

- **G1** — Every workspace row shows the same information structure regardless of how it was
  created (+ button, bookmark, remote dialog, recovery)
- **G2** — The subtitle shows only information the user cannot already see elsewhere in the row
- **G3** — No garbage: VTE prompt titles (`user@host:path`), generic shell names (`bash`, `zsh`),
  and runtime metadata (`Terminal (persistent)`) never appear in the subtitle
- **G4** — The spec is testable: an automated UI test can verify compliance by inspecting
  AT-SPI accessible text

## Non-Goals

- **NG1** — This RFC does not redesign the widget tree; it uses the ActionRow structure from
  RFC-002
- **NG2** — Color coding, activity indicators, and drag-and-drop are out of scope (covered by
  RFC-002)
- **NG3** — Multi-pane subtitle aggregation (showing info from all panes, not just the active
  one) is deferred

---

## Background & Motivation

The sidebar is the primary navigation surface. A user glances at it to answer three questions:

1. **Which workspace is this?** → title
2. **Where is it?** → subtitle
3. **What kind of connection?** → icon

When any of these are wrong, inconsistent, or filled with noise, the sidebar becomes useless.
The user has to click each tab to figure out what it is.

The current implementation has been through six revisions because each change was made without
a written spec. The developer (human or AI) would change the logic, update the unit tests to
match the new behavior, CI would pass, and the result would look wrong with real terminal data.

### What went wrong

| PR | Change | Regression |
|----|--------|------------|
| #332 | Show active pane command/path in subtitle | VTE title `user@host:path` leaked into subtitle |
| #351 | Redesign row semantics | Inconsistent naming: "Session N" vs "Workspace N" |
| #353 | Move connection icon to prefix | Direct workspaces lost their icon entirely |
| #358 | Fix icons, titles, subtitles | VTE title still preferred over CWD |
| #359 | Show full CWD + command on two lines | Prompt title appeared as second line |
| #360 | Remove VTE title entirely | Fixed, but only after three attempts |

Root cause: no spec. Each PR defined its own rules, tested against synthetic inputs, and
merged without visual verification against real terminal data.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Consistent, predictable sidebar; no more garbage text |
| Contributors | Clear spec to code against; no ambiguity about what subtitle should show |
| Packagers    | No impact |

---

## Considered Options

### Option A — Title-first subtitle

Show the VTE window title as the primary subtitle content. Fall back to CWD when no title
is set.

**Pros**: Shows the running command (vim, htop) which is useful context.
**Cons**: VTE title is set by the shell prompt in most configurations, producing
`user@host:~/path` which is noise. Requires maintaining a blocklist of "generic" titles
that grows over time. Every new shell configuration can produce a new garbage pattern.

### Option B — CWD-only subtitle

Show only the working directory (tilde-collapsed) from OSC 7. Never show the VTE title.

**Pros**: CWD is always meaningful and never garbage. Simple rule, no blocklist needed.
One line, predictable.
**Cons**: Loses the running-command context. When the user is running `vim` or `htop`,
the subtitle just shows the directory.

### Option C — Structured multi-field subtitle

Show CWD as the primary field. Show the running command as a secondary field only when
it differs from the shell (detected via a process-tree check or title heuristic).

**Pros**: Best of both worlds — path context plus command context.
**Cons**: Process-tree detection is platform-specific and fragile. Title heuristic
requires a blocklist. Complexity increases regression risk.

---

## Decision

**Chosen option: Option B — CWD-only subtitle, with filtered title fallback**

The original decision was pure Option B (never show VTE title). The implementation evolved to
a pragmatic variant: CWD is always preferred, but when no CWD is available (shell does not
support OSC 7), the VTE title is shown as a fallback after filtering out generic titles
(`bash`, `zsh`, `sh`, `fish`, and anything containing `terminal`). This avoids a permanently
blank subtitle for shells that do not emit OSC 7 while still preventing the most common
garbage patterns.

Rationale:

1. The subtitle has regressed six times because of title-handling complexity. The simplest
   rule that eliminates all garbage is: always prefer CWD over VTE title.
2. CWD from OSC 7 is a structured, machine-readable signal. VTE title is a free-form string
   set by arbitrary shell configurations. Structured data wins.
3. The running command is visible in the terminal pane itself. The sidebar's job is to help
   the user identify and navigate workspaces, not to replicate the terminal content.
4. A blank subtitle when no CWD is available provides no value. Showing a filtered VTE title
   (e.g. `vim main.rs`) is better than nothing for shells without OSC 7 support.
5. If command visibility in the sidebar becomes a user need, it can be added later as a
   separate, well-tested feature (Option C) without changing the CWD display.

---

## Design

### Row anatomy

```text
┌─────────────────────────────────────────────────────┐
│ [icon] [N]  Title                            [⋮/✕] │
│              subtitle                               │
└─────────────────────────────────────────────────────┘
```

### Connection icon (prefix)

Always visible on every row. One icon per row.

Shape encodes workspace type (constant for the lifetime of the row):

| Workspace type | Icon shape |
|----------------|------------|
| Local managed (daemon-backed) | `computer-symbolic` |
| Remote managed (daemon-backed) | `network-server-symbolic` |
| Direct (no daemon) | `utilities-terminal-symbolic` |

Color encodes connection state (changes dynamically):

| Status | CSS class | Meaning |
|--------|-----------|---------|
| Connected (local) | `dim-label` | Normal, healthy |
| Connected (remote) | `accent` | Active remote connection |
| Connecting / Starting | `dim-label` | In progress |
| Recovered | `accent` | Just reconnected |
| Disconnected | `warning` | Lost connection |
| Blocked | `error` | Cannot connect |

The icon shape never changes — you always know what kind of workspace it is at a glance.
Only the color changes to reflect connection health. Tooltip provides the detail
(e.g. "Disconnected from runtime", "Connecting to remote host").

### Title

The workspace name. Set at creation, editable via double-click rename.

| Creation path | Initial title |
|---------------|---------------|
| + button / Ctrl+Shift+T | CWD leaf folder, or `Workspace N` |
| New Remote dialog | Short hostname (before first `.`) |
| Bookmark (any) | Bookmark name |
| Recovery (daemon restart) | Preserved from before restart |

After creation, the title auto-updates to the CWD basename when the active pane's working
directory changes, unless the user has manually renamed the workspace. Manual rename sets
`user_renamed = true` on `SessionState`, which permanently disables auto-rename for that
workspace. This is implemented by `maybe_auto_rename_workspace()` in
`clients/rttx/src/window/runtime.rs`.

### Subtitle

One line (`subtitle_lines` set to 1). Shows where the active pane is. Content depends on
endpoint type and available data.

| Endpoint | CWD available | Subtitle format | Example |
|----------|---------------|----------------|---------|
| Local | Yes | `{cwd}` | `~/projects/rttx` |
| Local | No | `{filtered_title}` or `""` | `vim main.rs` or *(blank)* |
| Remote | Yes | `{host} · {cwd}` | `builder · ~/src/rttx` |
| Remote | No | `{host}` or `{host} · {filtered_title}` | `builder` |

Rules:
1. CWD comes from OSC 7 (`current_directory()`), tilde-collapsed via `collapse_home()`.
2. When CWD is available, it is always used. The VTE title is ignored.
3. When no CWD is available, the VTE title is shown only if it passes the `is_generic_title()`
   filter — generic shell names (`bash`, `zsh`, `sh`, `fish`) and strings containing
   `terminal` are suppressed.
4. When neither CWD nor a useful title is available, the subtitle is empty (local) or shows
   only the host (remote).
5. `subtitle_lines` is 1. No multi-line subtitles.
6. For remote endpoints, the host is the full `RuntimeEndpoint::Remote.host` string
   (which may include `user@`), followed by ` · ` separator, followed by the pane description.

### Suffix button

| Workspace type | Button | Tooltip |
|----------------|--------|---------|
| Managed (daemon-backed) | `view-more-symbolic` (⋮) | "Workspace actions" |
| Direct (no daemon) | `window-close-symbolic` (✕) | "Close workspace" |

Right-click on any row also opens the workspace actions popover.
Double-click opens the rename popover.

### Activity indicator

A 3px accent-colored left bar on the row, driven by CSS classes. Indicates terminal output
in background (non-visible) workspaces. No extra widgets — pure CSS on the row itself.

| State | CSS class | Appearance | Tooltip |
|-------|-----------|------------|---------|
| None | *(no class)* | No bar | *(default tooltip)* |
| Active | `.session-activity-active` | Solid accent bar, pulsing animation (1.8s ease-in-out) | "Background activity is ongoing" |
| Idle | `.session-activity-idle` | Solid accent bar, static, 45% opacity | "Unread activity in this workspace" |

Lifecycle:
1. Terminal produces output while the workspace is not visible → state becomes **Active**
2. Output stops (debounce: 1200ms production, 30ms tests) → state transitions to **Idle**
3. User switches to the workspace → state resets to **None**

Repeated `mark_activity()` calls reset the idle timer (debounce behavior).

CSS implementation (in application.rs inline stylesheet):
```css
@keyframes activity-pulse {
    0%   { box-shadow: inset 3px 0 0 0 alpha(@accent_bg_color, 0.9); }
    50%  { box-shadow: inset 3px 0 0 0 alpha(@accent_bg_color, 0.4); }
    100% { box-shadow: inset 3px 0 0 0 alpha(@accent_bg_color, 0.9); }
}
.session-activity-active {
    box-shadow: inset 3px 0 0 0 @accent_bg_color;
    animation: activity-pulse 1.8s ease-in-out infinite;
}
.session-activity-idle {
    box-shadow: inset 3px 0 0 0 alpha(@accent_bg_color, 0.45);
}
```

This indicator is stable and should not be changed without a separate RFC.

### What is explicitly excluded from the subtitle

These strings must never appear in the subtitle under any circumstances:

- Generic shell names: `bash`, `zsh`, `sh`, `fish` (filtered by `is_generic_title()`)
- Strings containing `terminal` (filtered by `is_generic_title()`)
- Runtime metadata: `persistent`, `ephemeral`
- The word "Session" or "Workspace" (that's the title's job)
- Connection status text (that's the icon's job)
- Pane count (removed in #303)

The VTE window title may appear as a fallback when no CWD is available, but only after
passing the `is_generic_title()` filter. The AT-SPI UI tests enforce a broader exclusion
list including `user@host:path` prompt patterns and the `nu` shell name.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 — Uniform structure | Same title/subtitle/icon rules for all creation paths |
| G2 — No redundancy | Subtitle shows CWD (preferred) or filtered title; title shows name; icon shows connection |
| G3 — No garbage | Generic titles filtered by `is_generic_title()`; AT-SPI tests enforce broader exclusion list |
| G4 — Testable | AT-SPI tests in `test_sidebar_content.py` check subtitle text against forbidden patterns |

---

## Development Plan

- [x] **Connection icon on all rows** — PR #358
- [x] **Consistent initial title generation** — PR #358 (`workspace_display_name`)
- [x] **CWD-only subtitle with filtered title fallback** — PR #360 and subsequent refinements
- [x] **AT-SPI UI test: subtitle compliance** — `test_sidebar_content.py` verifies no forbidden
  patterns in subtitle text for both direct and managed workspaces
- [x] **AT-SPI UI test: icon presence** — `test_sidebar_content.py` verifies every sidebar row
  has a connection icon for both direct and managed workspaces
- [ ] **Remove auto-rename on CWD change** — `maybe_auto_rename_workspace` and `user_renamed`
  field still exist; title auto-updates to CWD basename unless user has manually renamed.
  This behavior is intentional for now — it provides useful context without user action.
  Revisit if users report unwanted renames.
- [ ] **Screenshot-in-PR policy** — not yet added to CONTRIBUTING.md; document that
  sidebar-affecting PRs require a screenshot with at least 3 workspace types

---

## Open Questions

- [x] **Q1** — Should the subtitle show the running command when it can be reliably detected
  (e.g. via foreground process group query instead of VTE title)? **Answer**: The current
  implementation shows the VTE title as a fallback when no CWD is available, filtered through
  `is_generic_title()`. This is a pragmatic middle ground. A process-tree-based approach
  remains a future possibility if user demand exists.
- [ ] **Q2** — Should multi-pane workspaces show aggregated info (e.g. "3 panes") or always
  show only the active pane's CWD? Current: active pane only.

---

## References

- [RFC-002: Adwaita Modernization & SessionRow Redesign](./RFC-002-adwaita-modernization.md)
- [GNOME Human Interface Guidelines — Lists](https://developer.gnome.org/hig/patterns/containers/lists.html)
- PR #332, #351, #353, #358, #359, #360 — the regression chain this RFC prevents
- [Tracking issue](https://github.com/IllyaYalovyy/rttx/issues/612)
