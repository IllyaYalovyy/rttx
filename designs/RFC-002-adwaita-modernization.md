# RFC-002: Adwaita Modernization & SessionRow Redesign

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented              |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

Replace the hand-rolled `gtk4::Box`-based window layout and `SessionRow` widget with proper
Libadwaita components: `adw::ToolbarView` for the window chrome, `adw::ToastOverlay` + `adw::Toast`
for in-app notifications, and `adw::ActionRow` as the base class for `SessionRow`. This brings
rttx in line with GNOME HIG and unlocks flat header transitions, two-tier notifications, session
color coding, and activity indicators.

---

## Goals

- **G1** — Window chrome follows GNOME HIG using `adw::ToolbarView`
- **G2** — In-app toasts for background-session process exit; desktop notifications only when the window is unfocused
- **G3** — `SessionRow` is an `adw::ActionRow` subclass with activity spinner, color dot, and inline rename

## Non-Goals

- **NG1** — Not changing the `adw::OverlaySplitView` structure or the `gtk::Stack` session container
- **NG2** — Not implementing session color assignment UI in this RFC (color data model is included; picker UI is deferred)
- **NG3** — Not changing terminal header or split/close button layout

---

## Background & Motivation

The original window layout used a `gtk4::Box` (vertical) containing an `adw::HeaderBar` then
an `adw::OverlaySplitView`. This predates `adw::ToolbarView` being available in Libadwaita 1.2+.
`SessionRow` was a hand-rolled `gtk4::Box` subclass which duplicated what `adw::ActionRow` provides
out of the box. Process notifications fired `gio::Notification` unconditionally, even when the
window was in focus and showing the relevant session, resulting in distracting desktop popups.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | Flat header bar; in-app toasts for background sessions; activity indicators and color dots in session list |
| Contributors | `SessionRow` is now `adw::ActionRow`; less custom layout code to maintain |
| Packagers | Requires Libadwaita ≥ 1.5; no new system dependencies |

---

## Considered Options

### Option A — Keep `gtk4::Box` window layout, add `adw::ToolbarView` later *(reconstructed)*

**Pros**: No churn; safe.
**Cons**: Every new feature built on top of the wrong foundation. Flat-header transitions never
work correctly without `ToolbarView`.

### Option B — Migrate to `adw::ToolbarView` + `adw::ToastOverlay` now

**Pros**: Correct structural foundation. All downstream features (toasts, collapsible toolbars,
mobile safe areas) work automatically.
**Cons**: One-time refactor of `window.rs::constructed`.

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

Chosen options: B + D

`adw::ToolbarView` is the correct GNOME HIG structure and the right time to adopt it is before
more features are built on top of the old layout. `adw::ActionRow` eliminates the custom layout
code in `SessionRow` and gives better accessibility for free.

---

## Design

### Window layout

```text
adw::ApplicationWindow
  └── adw::ToolbarView
        ├── top bar: adw::HeaderBar        (via .add_top_bar())
        └── content: adw::ToastOverlay
              └── adw::OverlaySplitView
                    ├── sidebar: ScrolledWindow → gtk4::ListBox
                    └── content: gtk4::Stack
```

### Notification tiers

| Condition | Action |
| --- | --- |
| Window unfocused | `gio::Notification` (existing behavior) |
| Window focused, exit in non-visible session | `adw::Toast` via `toast_overlay` |
| Window focused, exit in visible session | No notification |

### SessionRow widget tree

```text
adw::ActionRow
├── prefix: gtk4::Stack "indicator"
│     ├── page "idle":   gtk4::Image  (color dot, CSS class)
│     └── page "active": gtk4::Spinner
├── [title]    — session name  (ActionRow built-in)
├── [subtitle] — activity text (ActionRow built-in)
└── suffix:
      ├── gtk4::Label  (terminal count badge)
      ├── gtk4::Button ("document-edit-symbolic", rename trigger)
      └── gtk4::Button ("window-close-symbolic", flat circular)
```

### Session color data model

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub enum SessionColor { #[default] Blue, Green, Yellow, Red, Purple, Pink, Teal, Orange }
```

Each variant maps to an Adwaita accent CSS variable (`@blue_3`, `@green_3`, …) applied as a
CSS class on the indicator dot image.

### Activity detection

VTE fires `window_title_changed` when the shell updates its title (shells do this when starting
a command). A 5-second debounce timer resets the activity state. The sidebar spinner starts on
title change and stops after debounce. The timer closure captures a weak window reference to
avoid crashes when sessions are closed before the timer fires.

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — HIG window chrome | `adw::ToolbarView` replaces `gtk4::Box` wrapper |
| G2 — Two-tier notifications | `adw::Toast` for focused-window/background-session; `gio::Notification` when unfocused |
| G3 — SessionRow as ActionRow | Full rewrite with indicator stack, spinner, color dot, rename popover |

---

## Development Plan

- [x] **Window layout** — Replace `gtk4::Box` + `adw::HeaderBar` with `adw::ToolbarView`; add `adw::ToastOverlay`
- [x] **Two-tier notifications** — Implement `terminal_is_in_visible_session`; update `notify_process_completed`
- [x] **SessionRow base** — Rewrite as `adw::ActionRow` subclass
- [x] **Session renaming** — Inline popover with `adw::EntryRow`
- [x] **Activity indicator** — Wire `window_title_changed` signal; debounce timer; spinner vs dot
- [x] **Session color coding** — Assign colors on creation; CSS classes; color picker UI deferred (NG2)

---

## Open Questions

- [ ] **Q1** — Should the color dot use `circle-filled-symbolic` (stock icon) or a 10×10 CSS-drawn circle via a custom icon? The stock icon scales with the font; the CSS approach gives precise sizing control.
