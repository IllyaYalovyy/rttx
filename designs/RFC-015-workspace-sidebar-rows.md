# RFC-015: Workspace Sidebar Row Content Specification

| Field         | Value         |
|---------------|---------------|
| Status        | Draft         |
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

**Chosen option: Option B — CWD-only subtitle**

Rationale:

1. The subtitle has regressed six times because of title-handling complexity. The simplest
   rule that eliminates all garbage is: never show the VTE title in the subtitle.
2. CWD from OSC 7 is a structured, machine-readable signal. VTE title is a free-form string
   set by arbitrary shell configurations. Structured data wins.
3. The running command is visible in the terminal pane itself. The sidebar's job is to help
   the user identify and navigate workspaces, not to replicate the terminal content.
4. If command visibility in the sidebar becomes a user need, it can be added later as a
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

Always visible on every row. Determined by endpoint type and connection status.

| Endpoint | Status | Icon | CSS class | Tooltip |
|----------|--------|------|-----------|---------|
| Local | Connected | `computer-symbolic` | `dim-label` | Local workspace |
| Local | Connecting/Starting | `content-loading-symbolic` | `dim-label` | Connecting… |
| Local | Recovered | `emblem-ok-symbolic` | `accent` | Connection recovered |
| Local | Disconnected | `network-offline-symbolic` | `warning` | Disconnected |
| Local | Blocked | `network-offline-symbolic` | `error` | Connection blocked |
| Remote | Connected | `network-server-symbolic` | `accent` | Connected to remote host |
| Remote | Connecting | `network-server-symbolic` | `dim-label` | Connecting… |
| Remote | Recovered | `emblem-ok-symbolic` | `accent` | Connection recovered |
| Remote | Disconnected | `network-offline-symbolic` | `warning` | Disconnected |
| Remote | Blocked | `network-offline-symbolic` | `error` | Connection blocked |
| Direct (no daemon) | — | `computer-symbolic` | `dim-label` | Local workspace |

The icon is set in `append_session_row` at creation time and updated by
`refresh_workspace_row_status` on connection state changes.

### Title

The workspace name. User-controlled. Set at creation, editable via double-click rename.
The application never changes the title after creation — it belongs to the user.

| Creation path | Initial title |
|---------------|---------------|
| + button / Ctrl+Shift+T | CWD leaf folder, or `Workspace N` |
| New Remote dialog | Short hostname (before first `.`) |
| Bookmark (any) | Bookmark name |
| Recovery (daemon restart) | Preserved from before restart |

Once created, the title is static unless the user explicitly renames it. No auto-rename
on CWD change, no auto-rename on reconnect, no auto-rename ever.

### Subtitle

One line. Shows where the active pane is. Content depends on endpoint type.

| Endpoint | Subtitle format | Example |
|----------|----------------|---------|
| Local | `{cwd}` | `~/projects/rttx` |
| Remote | `{host} · {cwd}` | `builder · ~/src/rttx` |
| Any (no CWD) | `""` (empty) | *(blank)* |

Rules:
1. CWD comes from OSC 7 (`current_directory()`), tilde-collapsed.
2. The VTE window title is **never** shown in the subtitle.
3. When no CWD is available (shell doesn't support OSC 7), the subtitle is empty.
4. `subtitle_lines` is 1. No multi-line subtitles.
5. For remote endpoints, the host is the full `RuntimeEndpoint::Remote.host` string
   (which may include `user@`), followed by ` · ` separator, followed by the CWD.

### Suffix button

| Workspace type | Button | Tooltip |
|----------------|--------|---------|
| Managed (daemon-backed) | `view-more-symbolic` (⋮) | "Workspace actions" |
| Direct (no daemon) | `window-close-symbolic` (✕) | "Close workspace" |

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
2. Output stops (debounce) → state transitions to **Idle**
3. User switches to the workspace → state resets to **None**

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

- VTE window title (OSC 0/2 content)
- Shell name: `bash`, `zsh`, `sh`, `fish`, `nu`
- Runtime metadata: `Terminal`, `persistent`, `ephemeral`
- Prompt-set strings matching `*@*:*` pattern
- The word "Session" or "Workspace" (that's the title's job)
- Connection status text (that's the icon's job)
- Pane count (removed in #303)

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 — Uniform structure | Same title/subtitle/icon rules for all creation paths |
| G2 — No redundancy | Subtitle shows only CWD; title shows name; icon shows connection |
| G3 — No garbage | VTE title explicitly excluded; no blocklist needed |
| G4 — Testable | AT-SPI test can check subtitle text against forbidden patterns |

---

## Development Plan

- [x] **Connection icon on all rows** — PR #358
- [x] **Consistent initial title generation** — PR #358 (`workspace_display_name`)
- [x] **CWD-only subtitle** — PR #360
- [ ] **Remove auto-rename on CWD change** — delete `maybe_auto_rename_workspace` and
  `user_renamed` field; title is user-controlled after creation
- [ ] **AT-SPI UI test: subtitle compliance** — verify no forbidden patterns in subtitle
  text across local, remote, and bookmark-created workspaces
- [ ] **AT-SPI UI test: icon presence** — verify every sidebar row has a connection icon
- [ ] **Screenshot-in-PR policy** — document in CONTRIBUTING.md that sidebar-affecting PRs
  require a screenshot with at least 3 workspace types

---

## Open Questions

- **Q1** — Should the subtitle show the running command when it can be reliably detected
  (e.g. via foreground process group query instead of VTE title)? Deferred to a future RFC
  if user demand exists.
- **Q2** — Should multi-pane workspaces show aggregated info (e.g. "3 panes") or always
  show only the active pane's CWD? Current: active pane only.

---

## References

- [RFC-002: Adwaita Modernization & SessionRow Redesign](./RFC-002-adwaita-modernization.md)
- [GNOME Human Interface Guidelines — Lists](https://developer.gnome.org/hig/patterns/containers/lists.html)
- PR #332, #351, #353, #358, #359, #360 — the regression chain this RFC prevents
