# RFC-002: Adwaita Modernization & SessionRow Redesign

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented             |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

Replace the hand-rolled `gtk4::Box`-based `SessionRow` widget with a proper Libadwaita
`adw::ActionRow` subclass, and add `adw::ToastOverlay` + `adw::Toast` for in-app notifications.
This brings rttx in line with GNOME HIG and unlocks two-tier notifications, workspace color
coding, and activity indicators.

The original plan also called for `adw::ToolbarView` as the top-level window chrome. In practice
the main window kept a `gtk4::Box` + `adw::HeaderBar` layout because the sidebar was later
replaced with resizable `gtk4::Paned` widgets (see RFC-010), which made `ToolbarView` unnecessary
for the main window. `adw::ToolbarView` is used in dialog windows instead.

---

## Goals

- **G1** — Window chrome follows GNOME HIG with `adw::HeaderBar` and `adw::ToastOverlay`
- **G2** — In-app toasts for background-workspace process exit; desktop notifications only when the window is unfocused
- **G3** — `SessionRow` is an `adw::ActionRow` subclass with activity accent bar and inline rename

## Non-Goals

- **NG1** — Not changing the `gtk4::Stack` workspace container (the original `adw::OverlaySplitView`
  sidebar was later replaced by `gtk4::Paned` in RFC-010, independent of this RFC)
- **NG2** — Not implementing workspace color assignment UI in this RFC (color data model is included; picker UI is deferred)
- **NG3** — Not changing terminal header or split/close button layout

---

## Background & Motivation

The original window layout used a `gtk4::Box` (vertical) containing an `adw::HeaderBar` then
an `adw::OverlaySplitView`. `SessionRow` was a hand-rolled `gtk4::Box` subclass which duplicated
what `adw::ActionRow` provides out of the box. Process notifications fired `gio::Notification`
unconditionally, even when the window was in focus and showing the relevant workspace, resulting
in distracting desktop popups.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | In-app toasts for background workspaces; activity indicators and color dots in workspace list |
| Contributors | `SessionRow` is now `adw::ActionRow`; less custom layout code to maintain |
| Packagers | Requires Libadwaita ≥ 1.5; no new system dependencies |

---

## Considered Options

### Option A — Keep `gtk4::Box` window layout, add `adw::ToolbarView` later *(reconstructed)*

**Pros**: No churn; safe.
**Cons**: Every new feature built on top of the wrong foundation.

### Option B — Migrate to `adw::ToolbarView` + `adw::ToastOverlay` now

**Pros**: Correct structural foundation. All downstream features (toasts, collapsible toolbars,
mobile safe areas) work automatically.
**Cons**: One-time refactor of `window.rs::constructed`.

**Implementation note:** `adw::ToastOverlay` was adopted as planned. `adw::ToolbarView` was
adopted for dialog windows but not for the main window — the sidebar was later replaced with
resizable `gtk4::Paned` widgets, and the main window kept a `gtk4::Box` + `adw::HeaderBar`
layout which serves the same purpose.

### Option C — Keep hand-rolled `SessionRow`, extend with spinner and color *(reconstructed)*

**Pros**: Avoids subclassing change.
**Cons**: Reimplements what `adw::ActionRow` provides (title, subtitle, prefix/suffix slots,
accessible semantics, selection styling). More code to maintain.

### Option D — Rewrite `SessionRow` as `adw::ActionRow` subclass

**Pros**: Built-in title/subtitle, accessible by default, correct Adwaita selection styling,
prefix/suffix widget slots, no custom layout code.
**Cons**: Requires changing the GObject parent type (one-time).

---

## Decision

Chosen options: B (partially) + D

`adw::ActionRow` eliminates the custom layout code in `SessionRow` and gives better accessibility
for free. `adw::ToastOverlay` provides the in-app notification tier. The main window kept
`gtk4::Box` + `adw::HeaderBar` rather than adopting `adw::ToolbarView`, because the sidebar
evolved to use `gtk4::Paned` for resizable widths.

---

## Design

### Window layout

The original RFC proposed `adw::ToolbarView` as the top-level container. The actual implementation
uses `gtk4::Box` with `adw::HeaderBar` and two nested `gtk4::Paned` widgets for resizable
sidebars:

```text
adw::ApplicationWindow
  └── gtk4::Box (Vertical)
        ├── adw::HeaderBar
        └── gtk4::Paned (left_paned, Horizontal)
              ├── gtk4::ScrolledWindow → gtk4::ListBox (workspace sidebar)
              └── adw::ToastOverlay
                    └── gtk4::Paned (right_paned, Horizontal)
                          ├── gtk4::Stack (workspace content)
                          └── gtk4::Box (utility sidebar)
```

`adw::ToolbarView` is used in dialog windows (`form_dialog.rs`, `new_workspace_dialog.rs`,
`connect_existing_dialog.rs`).

### Notification tiers

**Status: implemented.** Three tiers in `NotificationTier` enum (`window/mod.rs`):

| Condition | Tier | Action |
| --- | --- | --- |
| Terminal in visible workspace | `Suppress` | No notification |
| Window focused, terminal in background workspace | `Toast` | `adw::Toast` via `toast_overlay` |
| Window unfocused | `Desktop` | `gio::Notification` |

The `notification_tier()` function determines the tier based on the terminal's workspace
visibility and window focus state.

### SessionRow widget tree

**Status: implemented** in `sidebar.rs`. `SessionRow` is an `adw::ActionRow` subclass
(`RttxSessionRow`):

```text
adw::ActionRow  (.session-row)
├── prefix: gtk4::Image  (connection icon — always visible, 16px)
├── prefix: gtk4::Label  (position number 1–9, dim-label + caption CSS)
├── [title]    — workspace name  (ActionRow built-in)
├── [subtitle] — pane info  (ActionRow built-in, 1 line max)
└── suffix:
      └── gtk4::Button
            "window-close-symbolic" for direct workspaces
            "view-more-symbolic" for managed workspaces (opens popover menu)
```

The suffix button tooltip and click handler differ by workspace type:
- Direct workspaces: "Close workspace" → confirmation dialog
- Managed workspaces: "Workspace actions" → popover menu

Activity is indicated by CSS classes on the row itself (no extra widgets):
- `.session-activity-active` — 3px accent left bar with pulse animation (1.8s ease-in-out)
- `.session-activity-idle` — 3px accent left bar, static, 45% opacity

The original RFC proposed a spinner and color dot for activity; the implementation uses CSS
`box-shadow` accent bars instead, which are less intrusive and require no extra widgets.

### Workspace color data model

**Status: implemented** in `session/state.rs`:

```rust
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SessionColor {
    #[default] Blue, Green, Yellow, Red, Purple, Pink, Teal, Orange,
}
```

Each variant maps to a CSS class (`accent-blue`, `accent-green`, …) applied to the row.
Colors are assigned round-robin on workspace creation. The color picker UI remains deferred
(NG2).

### Activity detection

**Status: implemented** in `sidebar.rs`.

The `ActivityState` enum has three variants: `None`, `Active`, `Idle`. When terminal output
is detected, the row transitions to `Active` (pulsing accent bar). After a debounce delay
(1200 ms), it transitions to `Idle` (static accent bar). The timer closure captures a weak
reference to avoid use-after-free when workspaces are closed before the timer fires.

### Workspace renaming

**Status: implemented** in `window/dialogs.rs`.

The rename UI uses a `gtk4::Popover` containing a `gtk4::Entry` with Cancel/Rename buttons.
The original RFC proposed `adw::EntryRow`; the implementation uses a plain `gtk4::Entry`
which is simpler and sufficient for a single-field popover.

---

## Goals Alignment

| Goal | How addressed | Status |
| --- | --- | --- |
| G1 — HIG window chrome | `adw::HeaderBar` + `adw::ToastOverlay`; `adw::ToolbarView` in dialogs | Implemented (diverged from ToolbarView for main window) |
| G2 — Two-tier notifications | `adw::Toast` for focused-window/background-workspace; `gio::Notification` when unfocused | Implemented |
| G3 — SessionRow as ActionRow | Full rewrite with connection icon prefix, accent bar activity, managed actions | Implemented |

---

## Development Plan

- [x] **Window layout** — `adw::ToastOverlay` added; main window kept `gtk4::Box` + `adw::HeaderBar`
  (ToolbarView adopted for dialogs only)
- [x] **Two-tier notifications** — `notification_tier()` function with `Suppress`/`Toast`/`Desktop`
  tiers; `notify_process_completed` dispatches accordingly
- [x] **SessionRow base** — Rewrite as `adw::ActionRow` subclass in `sidebar.rs`
- [x] **Workspace renaming** — Inline popover with `gtk4::Entry` (diverged from proposed `adw::EntryRow`)
- [x] **Activity indicator** — Accent bar via CSS `box-shadow` with pulse animation; replaces
  earlier spinner/dot design proposed in the original RFC
- [x] **Workspace color coding** — Round-robin assignment on creation; CSS accent classes; color
  picker UI deferred (NG2)
- [x] **Connection icon** — Always-visible prefix icon: `computer-symbolic` (local),
  `network-server-symbolic` (remote), `utilities-terminal-symbolic` (direct/no daemon);
  CSS classes encode connection state (`accent`, `dim-label`, `warning`, `error`).
  Content rules further specified by RFC-015; state machine by RFC-018.

---

## Related RFCs

- **RFC-010** — Replaced `adw::OverlaySplitView` with `gtk4::Paned` for resizable sidebars,
  which is why the main window did not adopt `adw::ToolbarView`
- **RFC-015** — Extends this RFC's SessionRow widget tree with detailed content rules for
  title, subtitle, and icon content
- **RFC-016** — Redesigns workspace management (top bar actions, Places replacing Bookmarks,
  right sidebar), changing the broader context around RFC-002's components
- **RFC-018** — Formalizes the connection state machine and icon/color presentation mapping
  that RFC-002 introduced

---

## Open Questions

All resolved.
