# RFC-027: Places UX v2 — Host-Safe Navigation, Open Modes, Capture, and Scaling

| Field         | Value         |
|---------------|---------------|
| Status        | Draft         |
| Author(s)     | Illya Yalovyy |
| Supersedes    | —             |
| Superseded by | —             |

---

## Summary

Extend Places with the affordances and safety rules that the current model is
missing: **host-safe activation**, **multiple open actions**, **duplicate**,
**copy path**, and a much better **add current path** flow. Unlike Commands,
Places are navigation objects, not executable text. The core problem is not
parameterization. The core problem is that a saved place currently degenerates to
"send `cd` to the current pane", which is too weak for split/new-workspace
workflows and unsafe in cross-host views such as **All Hosts**.

This RFC keeps Places as saved directory targets, but upgrades them from thin
`cd` aliases into first-class navigation tools:

- row activation becomes host-aware and never targets the wrong runtime
- places can explicitly open in the current pane or a new workspace
- split is an alternate action, not an accidental side effect
- current-directory capture becomes edit-first and dedupe-aware
- optional metadata helps large place libraries stay usable

---

## Goals

- **G1** — No place activation may navigate the wrong host or runtime
- **G2** — Places support the workflows users actually need: current pane, split, and new workspace
- **G3** — Common management affordances are one click: duplicate and copy path
- **G4** — "Add current path to Places" produces a good saved object instead of silent duplicates
- **G5** — The Places library scales past a small flat list with metadata and future ranking/filtering improvements

## Non-Goals

- **NG1** — No path templating or parameter DSL
- **NG2** — No filesystem browser, file chooser, or project picker replacing Places
- **NG3** — No remote existence validation or SSH-side path probing during editing
- **NG4** — No file-level bookmarks; Places remain directory targets
- **NG5** — No direct replacement of the New Workspace dialog; it remains the explicit workspace-creation launcher

---

## Background & Motivation

RFC-006 and RFC-016 gave Places a clean model: a place is a named directory path
with optional host tags. That simplification was correct. The problem is the UX
around the model:

1. **Activation is too thin.** The current sidebar action is effectively
   `cd <path>\n` into the focused pane. That works only when the visible workspace
   already belongs to the right host and the user actually wants to repurpose the
   current pane.

2. **All Hosts view is unsafe.** A host-tagged place shown under a remote host
   section can still route through the current pane action path. That is the wrong
   abstraction. Browsing a cross-host management view must not silently mutate the
   wrong runtime.

3. **Places lack basic management affordances.** Commands now support duplicate and
   copy-body actions. Places still require manual copy/edit work for common variants
   such as "same repo, different host tag" or "same path, different default action".

4. **Current-path capture is too naive.** "Add to Places" silently saves an
   auto-derived name and can create duplicates. Places are reusable navigation
   objects; they deserve an edit-first capture flow.

5. **Scaling is weak.** A list of places with just `name` + `path` does not age
   well when several entries share the same leaf directory name across multiple
   hosts or roles.

Commands and Places should feel equally intentional, but not identical. Commands
needed parameterization because they represent shell work. Places need better
navigation semantics because they represent context.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Places become safe to use across host contexts and much faster to manage. Opening a saved place no longer means "hope the focused pane is the right target." |
| Contributors | Place activation becomes a resolved action path instead of raw `cd` string injection from multiple UI surfaces. More state is explicit and testable. |
| Packagers    | None. No new external dependencies. |

---

## Considered Options

### Place activation model

#### Option A — Keep Places as thin `cd` aliases *(rejected)*

**Pros**: Minimal code. No new data model fields.

**Cons**: Unsafe in All Hosts view, underpowered for split/new-workspace flows,
and too dependent on whichever pane happens to be focused.

#### Option B — Always open a new workspace *(rejected)*

**Pros**: Always host-safe. No pane-target ambiguity.

**Cons**: Too heavy for the common "take the current pane to a known directory"
workflow. Places would stop being quick context switches.

#### Option C — Host-safe resolved actions with explicit open modes *(chosen)*

**Pros**: Keeps current-pane navigation where it makes sense, but makes the host
context explicit and safe. Supports richer workflows without conflating navigation
with execution.

**Cons**: More UI and resolver logic. Requires shared action semantics across the
sidebar, terminal context menu, and other launch surfaces.

### Add-current-path flow

#### Option A — Silent save from current directory *(rejected)*

**Pros**: Fastest path in the happy case.

**Cons**: Produces weak names, easy duplicates, and no chance to set description or
default open mode while the context is fresh.

#### Option B — Open a prefilled editor, dedupe-aware *(chosen)*

**Pros**: Still fast, but produces much better saved objects. Duplicate detection can
reuse the existing place rather than adding another nearly-identical entry.

**Cons**: One more confirmation step than silent save.

### Scaling primitives

#### Option A — Stop at clone/copy/open modes *(rejected)*

**Pros**: Smaller RFC.

**Cons**: Repeats the same mistake RFC-006 made for Commands: solve the first-order
interaction, but not the way the library grows over time.

#### Option B — Include lightweight scaling primitives and follow-ups *(chosen)*

**Pros**: Keeps the MVP focused while documenting the next obvious steps so Places do
not stagnate again.

**Cons**: More design surface to document up front.

---

## Decision

Chosen options:

- Place activation model: **Option C** — host-safe resolved actions with explicit open modes
- Add-current-path flow: **Option B** — prefilled editor with duplicate reuse
- Scaling primitives: **Option B** — description now; labels and recents as follow-ups

Rationale: Places should remain simple directory objects, but the action semantics
around them must become explicit and safe. Unlike Commands, the meaningful v2
improvements are about **where** a place opens and **how** it is captured, not
about substituting text into a shell command.

---

## Design

### Data model

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum PlaceOpenMode {
    #[default]
    CurrentPane,
    NewWorkspace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Place {
    pub uuid: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub host_tags: Vec<String>,

    // --- RFC-027 additions ---
    #[serde(default = "default_place_open_mode")]
    pub default_open_mode: PlaceOpenMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}
```

Notes:

- `path` remains literal user-authored path text; no template parsing is added
- built-in places (`Home`, `Root`) remain non-persisted and always global
- `default_open_mode` defaults to `CurrentPane` for backward compatibility
- `description` is optional and purely informational

### Resolved navigation model

Places no longer map directly to "send `cd` to the focused pane". Instead every
activation goes through a resolver that produces:

```rust
pub enum ResolvedPlaceAction {
    CdCurrentPane {
        terminal_uuid: String,
        path: String,
    },
    OpenSplit {
        terminal_uuid: String,
        path: String,
    },
    OpenWorkspace {
        host_key: String,
        path: String,
    },
}
```

The resolver takes:

- the selected or implied target host context
- the active workspace host
- the place's `default_open_mode`
- whether the UI surface is host-specific or cross-host
- whether a compatible terminal target exists

### Target host rules

#### Host-specific sidebar view

When the host selector is set to a specific host key:

- host-specific places target that selected host
- global places also target that selected host
- `CurrentPane` is allowed only when the active workspace host matches the selected host
- otherwise the action resolves to `OpenWorkspace`

This guarantees that a remote-selected place never tries to `cd` a local pane, and
vice versa.

#### All Hosts view

When the host selector is **All Hosts**:

- host-tagged places under a host section target that section host
- global places target the active workspace host
- primary activation for host-tagged places resolves to:
  - `CdCurrentPane` when the active workspace host matches the section host and the
    place default is `CurrentPane`
  - otherwise `OpenWorkspace`

This keeps All Hosts usable as a launcher without allowing cross-host confusion.

### Open modes and actions

#### Primary activation

For backward compatibility, row activation still uses the place's default action:

- `CurrentPane`
- `NewWorkspace`

The difference is that `CurrentPane` is now **host-safe**. When it cannot safely
target the current pane, it falls back to `OpenWorkspace`.

#### Alternate actions

Each non-built-in place row gains a more-menu with:

- **Open Here** — navigate the current pane when safe; otherwise open a workspace
- **Open in Split** — create a split from the active pane and navigate the new pane
- **Open in New Workspace**
- **Copy path**
- **Duplicate**
- **Edit**
- **Delete**

Built-in places keep the navigation actions but not duplicate/edit/delete.

`Open in Split` is an explicit alternate action rather than persisted default state.
This keeps the stored model small while still covering the useful navigation path.

### Sidebar row presentation

The Places row remains lighter than Commands. It should not inherit every command row
feature mechanically.

Per-row rendering:

- title: `Place::name`
- subtitle: `path` when different from `name`
- tooltip: `description` when present
- optional small mode chip:
  - `WS` when `default_open_mode == NewWorkspace`
  - no chip for the default `CurrentPane` mode

This gives just enough signal without turning Places into a dense control row.

### Copy semantics

Places get a raw clipboard action:

- **Copy path** copies the raw stored `path`

There is no separate "Copy cd command" action in this RFC. The value of a Place is
the path itself; the shell command wrapper is incidental.

### Duplicate semantics

**Duplicate** creates a copy with:

- new UUID
- title suffixed with ` (copy)` / ` (copy N)`
- same path
- same host tags
- same default open mode
- same description

The editor opens immediately on the duplicate.

### Add-current-path flow

The terminal context menu action **Add to Places** changes from silent save to an
edit-first flow:

1. Read the current terminal working directory
2. Derive host tags from the active workspace host as today
3. Check for an existing non-built-in place with the same:
   - normalized path text
   - normalized host-tag set
4. If a match exists, open the editor on that existing place instead of creating a duplicate
5. Otherwise open the editor prefilled with:
   - derived `name`
   - current `path`
   - inferred `host_tags`
   - default open mode = `CurrentPane`

This preserves the convenience of capture while producing cleaner libraries.

### Editor changes

The Places editor grows from a 2-field form into a minimal but deliberate editor:

- `Name`
- `Path`
- `Default action` (`Open Here`, `Open in New Workspace`)
- `Description`
- `Host tags`

There is still no path validation beyond non-empty trimmed text. Remote correctness is
the user's responsibility.

### Search

Search for Places expands to match:

- `name`
- `path`
- `description`

Host tags remain part of scoping, not free-text search content.

### New Workspace dialog

The New Workspace dialog remains the explicit place launcher for creating a new
workspace on a chosen host. Its semantics do not change:

- selecting a place always creates a new workspace on that host

This is intentionally different from the sidebar, whose job is mixed navigation:
current pane when safe, workspace when necessary.

### Terminal context menu "Places" submenu

The terminal context menu must stop routing by raw path alone. The current action
shape (`open-place(path)`) is too lossy for host-safe resolution.

The submenu action payload should carry either:

- place UUID, with host context resolved at activation time, or
- a richer value containing both place UUID and target host key

The important point is architectural: place activation must route through the same
resolver as the sidebar. There should be one navigation contract, not multiple
slightly-different `cd` paths.

### Scaling primitives

The items below are not gates for the MVP, but they are the right next layer for
Places specifically.

- **Labels** — free-text grouping like `repo`, `logs`, `prod`, `infra`
- **Label filters** — same chip-bar pattern as Commands when labels arrive
- **Recents / pinning** — ranking and quick access matter more for Places than for
  Commands because navigation habits are repetitive and spatial
- **Local "Reveal in Files" action** — only for local places, if later demand justifies it

Of these, recents/pinning is the most Places-specific and likely the highest-value
follow-up after the MVP.

### Persistence and compatibility

- New persisted fields on `Place` use `#[serde(default)]`
- Missing `default_open_mode` loads as `CurrentPane`
- Missing `description` loads as empty
- `places.json` remains the source of truth for the library record
- recents, if added later, belong in client state rather than the library document

---

## Testing

Unit tests:

- `PlaceOpenMode` serde default and round-trip
- place activation resolver:
  - host-specific local place on local workspace → `CdCurrentPane`
  - remote place while local workspace active → `OpenWorkspace`
  - All Hosts remote section while different workspace visible → `OpenWorkspace`
  - global place in All Hosts view uses active workspace host
  - `NewWorkspace` mode always resolves to `OpenWorkspace`
- duplicate detection for add-current-path:
  - same normalized path + same tag set reuses existing place
  - different host tags do not collide
- search matches description

GTK widget tests:

- place editor round-trips `default_open_mode` and `description`
- place row renders `WS` chip only for `NewWorkspace`
- more-menu exposes copy/duplicate/open actions as expected

AT-SPI behavioral tests:

- clicking a host-matched place navigates the current pane
- clicking a host-mismatched place from All Hosts opens a new workspace instead of
  corrupting the active pane
- `Copy path` writes the raw path to the clipboard
- `Add to Places` opens a prefilled editor instead of silently saving
- duplicate current-path capture reopens the existing place instead of creating another row

Regression focus:

- no cross-host `cd` regressions from sidebar or context menu activation
- no duplicate spam from repeated "Add to Places" on the same directory

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 — Host-safe activation | All place actions resolve through host-aware navigation rules; mismatched hosts fall back to workspace creation |
| G2 — Real navigation workflows | Primary open modes plus explicit split/new-workspace actions |
| G3 — One-click management affordances | More-menu gains duplicate and copy-path |
| G4 — Better current-path capture | Add-to-Places becomes prefilled and dedupe-aware |
| G5 — Scaling beyond a flat list | Description now; labels and recents documented as the next layer |

---

## Development Plan

- [ ] **Step 1** — Extend `Place` with `default_open_mode` and `description`. Add serde coverage.
- [ ] **Step 2** — Introduce a shared place-action resolver used by sidebar and terminal context menu.
- [ ] **Step 3** — Rework sidebar place rows and menus around host-safe actions, copy path, and duplicate.
- [ ] **Step 4** — Replace silent `Add to Places` with the prefilled dedupe-aware editor flow.
- [ ] **Step 5** — Expand tests across unit, widget, and AT-SPI layers for cross-host safety and capture behavior.
- [ ] **Step 6** — Labels + label filter chips. *(follow-up issue)*
- [ ] **Step 7** — Recents / pinning for Places. *(follow-up issue; likely client-state work, not library work)*
- [ ] **Step 8** — Optional local-only "Reveal in Files". *(follow-up issue if justified by use)*

Steps 1–5 are the MVP. Steps 6–8 are the scaling roadmap.

---

## Open Questions

- [ ] **Q1** — Should `Open in Split` choose a fixed split direction or reuse whatever split-default policy the workspace already exposes?
- [ ] **Q2** — Should built-in global places (`Home`, `Root`) support `description`-style help text, or stay intentionally bare?
- [ ] **Q3** — When recents arrive, should pinned places stay separate from recency ranking or simply float to the top of one combined list?

---

## References

- [Tracking issue #804](https://github.com/IllyaYalovyy/rttx/issues/804)
- [RFC-006 — Places & Commands Right Sidebar](./RFC-006-commands-sidebar.md)
- [RFC-016 — Workspace Management v2](./RFC-016-workspace-management-v2.md)
- [RFC-025 — Commands UX v2](./RFC-025-commands-ux-v2.md)
