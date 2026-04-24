# RFC-025: Commands UX v2 — Clone, Clipboard, Parameters, and Scaling

| Field         | Value         |
|---------------|---------------|
| Status        | Draft         |
| Author(s)     | Illya Yalovyy |
| Supersedes    | —             |
| Superseded by | —             |

---

## Summary

Extend saved commands (RFC-006) with three commonly-missed affordances — **Clone**,
**Copy to clipboard**, and **Parameterized commands** — and record the additional UX
moves that let the commands library scale to hundreds of entries without turning the
sidebar into a dumping ground. Parameters use a fixed-choice drop-down model and a
bash-safe `{{NAME}}` placeholder syntax. The runtime parameter prompt is a modal dialog
with one combo row per parameter and a live-substituted preview.

---

## Goals

- **G1** — Duplicate an existing command in one click and land in the editor pre-filled
- **G2** — Copy a command body to the system clipboard without needing an active pane
- **G3** — Parameterize a command body with fixed-choice drop-down values using a
  bash-safe placeholder syntax
- **G4** — Surface a runtime parameter prompt that is fast for 1–2 parameters and
  remains legible for 4–6
- **G5** — Identify (not necessarily implement) the next set of UX moves that help the
  commands library scale past a flat list of ~30 entries

## Non-Goals

- **NG1** — No arbitrary scripting / macro DSL. No loops, conditionals, arithmetic,
  command substitution, or nested placeholders in the command body beyond what bash
  itself already does after substitution
- **NG2** — No free-text parameters. Parameter types in this RFC are drop-down only
- **NG3** — No shell-history capture, no auto-suggestion of commands from pane activity
- **NG4** — No pin / recent commands (already tracked in
  [#44](https://github.com/IllyaYalovyy/rttx/issues/44))
- **NG5** — No per-command export/import (config-level export is tracked in
  [#755](https://github.com/IllyaYalovyy/rttx/issues/755))
- **NG6** — No change to the Places tab or the host selector

---

## Background & Motivation

RFC-006 established the commands sidebar with a simple data model: title, body,
default run mode (Run / Insert), and host tags. The body is treated as literal shell
text — that was an explicit non-goal at the time (`NG1` in RFC-006).

After a year of real use, three friction points dominate:

1. **Near-duplicates are painful.** The recurring pattern is "same command, one word
   different". The only path today is: open editor → select body → copy → close →
   New command → paste → edit → save. A Clone action collapses that to one click.

2. **The body is trapped inside rttx.** Users sometimes want to paste a saved command
   into another window — a Slack message, an external ssh session, a doc. Today they
   have to either open the editor and select-copy the body, or Run it into a pane and
   copy it from the terminal. A clipboard action is one click.

3. **Parameter-shaped commands force N copies.** The same shape appears over and over:
   `kubectl logs -n <ns> <pod>`, `systemctl restart <service>`, `ssh root@<host>`.
   Without parameters, the user either saves N near-identical entries (pollutes the
   list) or keeps a single entry and edits it before each run (slow, error-prone).
   Even a very restricted parameter system — fixed drop-down choices only — covers
   the majority of these cases.

The secondary motivation is scale. The sidebar list works at 10 commands and breaks
down at 100. Several scaling primitives (description field, labels, confirmation flag,
command palette) are individually small, but the decisions compound. This RFC
documents them as a coherent set so we do not accrete them ad-hoc.

Related prior art in the repo:

- RFC-006 — base commands model
- RFC-024 — customizable keyboard shortcuts (reuse the shortcut registry if we bind
  individual commands to keys later)
- [#44](https://github.com/IllyaYalovyy/rttx/issues/44) — pin / recent commands

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Clone/copy-to-clipboard become one click. One parameterized command replaces N near-duplicates. Library scales with description, labels, and confirm-before-run. |
| Contributors | New optional fields on `SavedCommand` via `#[serde(default)]`. New parameter dialog widget. No changes to the daemon or the protocol. |
| Packagers    | None. No new dependencies; no new persisted files. |

---

## Considered Options

### Placeholder syntax

#### Option A — rttx-specific braces: `{{NAME}}` *(chosen)*

**Pros**: Unambiguous. Does not collide with real bash `$VAR` or `${VAR}`, so the user
cannot accidentally leak an environment-variable reference through the template. Easy
to parse (`{{` then `[A-Z][A-Z0-9_]*` then `}}`). Matches widely-understood template
conventions (Handlebars, Jinja, Go templates). Visually obvious that substitution is
happening — "this body is a template, not a shell snippet".

**Cons**: Body is not valid bash as-is (a user pasting the raw body into a shell will
see `{{FOO}}` literally). Mitigation: "Copy with parameters…" produces a substituted
form; "Copy body" produces the raw template. Either is usually what the user wants.

#### Option B — bash-style: `${NAME}`

**Pros**: Body is still legal bash. A reader familiar with shell recognises the
pattern instantly.

**Cons**: Collides with real environment variables. If rttx treats `${PATH}` as a
parameter, and the user also has `$PATH` in their shell, the semantics diverge
depending on where the command runs. If rttx *does not* treat it as a parameter
(requires declaration), the rule becomes subtle and the user has to learn which form
is which. Also: it means we must parse a subset of shell grammar correctly (quoting,
escaping) to avoid picking up real `${foo}` in a heredoc body — a trap.

#### Option C — positional: `$1`, `$2`

**Pros**: Terse.

**Cons**: Positional order is brittle — reordering parameters breaks existing command
bodies. No self-documenting name. Same collision issue as Option B when the user
saves bash that invokes a function which itself uses `$1`.

### Runtime parameter UI

#### Option A — Modal dialog, one combo row per parameter *(chosen)*

**Pros**: Clean keyboard navigation (Tab between rows, Enter to run, Esc to cancel).
Scales to 4–6 parameters without running out of space. Room for a live-substituted
preview block. Matches the GNOME HIG pattern for "ask the user for input before
acting" (`adw::Dialog`, `adw::ComboRow`). Same widget vocabulary as the existing
command editor.

**Cons**: One extra click compared to a single-click Run. Mitigation: Enter invokes
the primary action; the default values are pre-selected, so a one-key flow is
possible when the defaults are right.

#### Option B — Inline in the sidebar row

**Pros**: No modal, no extra click.

**Cons**: Sidebar is already 320 px wide. An `adw::ExpanderRow` with N combo rows and
a preview would dominate the panel and obscure other commands. Accessibility is worse
(focus management inside a dynamically-expanded row). Preview rendering is awkward
when the body is multiline.

#### Option C — Nested context menus ("Run → env=prod → svc=api → …")

**Pros**: Zero new widget; pure GMenu.

**Cons**: The combinatorial explosion is user-visible. Three parameters with three
choices each = 27 leaves. No preview. No way to show labels or help text. Reordering
parameters changes the menu structure.

#### Option D — Command palette (Ctrl+K)

Overlaps but is orthogonal. A palette that fuzzy-matches across *all* commands and
runs the selected one is a valuable scaling primitive, but it still needs the
parameter prompt when the selected command has placeholders. Documented in this RFC
as a scaling follow-up, not as the primary parameter UI.

### Parameter value model

#### Option A — Fixed string choices only *(chosen)*

Parameters carry a non-empty `Vec<String>` of choices plus an optional default. The
user picks one at runtime.

**Pros**: No free-text validation. No escaping/quoting concerns beyond what the
template author already expressed. Covers the overwhelmingly common case
(`env=prod|staging|dev`, `cluster=us|eu|apac`). Trivial to serialize and diff.

**Cons**: Cannot express continuous values like "line count" or "arbitrary pod name".
Acceptable for v1 — the explicit ask is to constrain the parameter vocabulary.
Free-text parameters can be added in a later RFC if the need proves out.

#### Option B — Typed values (string / enum / number / boolean)

**Pros**: More expressive. Could drive widget selection automatically.

**Cons**: Expands the surface area significantly. Boolean parameters are better
modelled as a two-choice drop-down today (`enabled=true|false`). Numeric parameters
imply validation rules (range, step) that do not exist yet. Out of scope for this
RFC.

---

## Decision

**Chosen options**:
- Placeholder syntax: **Option A** (`{{NAME}}`).
- Runtime parameter UI: **Option A** (modal dialog with one combo row per parameter).
- Parameter value model: **Option A** (fixed drop-down choices only).

Rationale: each choice minimises the ways a user can create an invalid or
surprising command while still covering the dominant workflow. The model can be
extended (free-text, typed values, inline entry for 1-parameter commands) in a
successor RFC without breaking the stored format.

---

## Design

### Data model

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandParameter {
    /// Placeholder name. Must match /^[A-Z][A-Z0-9_]*$/.
    pub name: String,
    /// Human-readable label shown in the runtime dialog.
    pub label: String,
    /// Non-empty list of allowed values. Empty is a save-time validation error.
    pub choices: Vec<String>,
    /// Optional default. If present, must be one of `choices`.
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedCommand {
    pub uuid: String,
    pub title: String,
    pub body: String,
    #[serde(default = "default_run_mode")]
    pub default_run_mode: CommandRunMode,
    #[serde(default)]
    pub host_tags: Vec<String>,

    // --- RFC-025 additions ---
    #[serde(default)]
    pub parameters: Vec<CommandParameter>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm_before_run: bool,
}
```

All new fields use `#[serde(default)]` so existing `commands.json` files keep loading
unchanged. Default values are skipped on write to keep the on-disk JSON readable for
commands that do not use the new features.

### Placeholder grammar and substitution

- A placeholder is `{{` + `<NAME>` + `}}` where `<NAME>` matches
  `^[A-Z][A-Z0-9_]*$`. Names are case-sensitive; uppercase is the only accepted form
  to keep scanning cheap and to avoid case-collision surprises.
- No escape mechanism in v1. If a body literally needs `{{FOO}}`, the command is not
  parameterizable — use a non-parameterized copy instead. (An `{{{{ ... }}}}` escape
  can be added later without breaking stored commands, since today the grammar
  rejects it.)
- Substitution is pure string replacement. Values are inserted verbatim without
  automatic shell-quoting. Quoting is the template author's responsibility:
  `--env={{ENV}}` vs `--name='{{NAME}}'` vs `"{{MSG}}"`. This matches the current
  contract that command bodies are raw shell text.

```rust
pub fn substitute(body: &str, values: &BTreeMap<String, String>) -> String;
pub fn scan_placeholders(body: &str) -> Vec<String>; // sorted, unique
```

### Save-time validation

On save the editor must accept the command only if:

1. Every placeholder found by `scan_placeholders(body)` has a matching
   `CommandParameter` with the same `name`.
2. Every `CommandParameter` has a non-empty `choices` list.
3. Each `default`, if set, appears in that parameter's `choices`.
4. Parameter `name`s are unique within the command.

A parameter declared but not referenced in the body is a **warning**, not an error
(the user may be mid-edit). The warning is shown as a subtle banner in the editor;
save is not blocked.

### Editor dialog — new "Parameters" section

A new `adw::PreferencesGroup` titled "Parameters" sits below the behavior group and
above the host-tag picker. Each parameter is one `adw::ExpanderRow` containing:

- `adw::EntryRow` for `name` (validated against the placeholder grammar)
- `adw::EntryRow` for `label`
- A list of `adw::EntryRow`s for `choices`, with an "Add choice" button
- An `adw::ComboRow` bound to the current choices to set the default

Below the group: an "Add parameter" button. A live banner summarises validation
state ("3 placeholders, 2 declared — `HOST` is missing").

Nice-to-have (post-MVP): an "Insert placeholder" action on the body text view that
inserts `{{SELECTED_NAME}}` at the cursor.

### Runtime parameter dialog

Triggered when the user activates (Run or Insert) a command with
`parameters.is_empty() == false`.

- `adw::Dialog` titled with the command's `title`
- One `adw::ComboRow` per parameter, in declaration order, pre-selected to its
  `default` (first choice if no default)
- A "Preview" `gtk4::TextView` at the bottom with monospace font, read-only,
  updated on every combo-row change. It renders the body with substitutions applied.
- Primary button matches the action that triggered the dialog: "Run" or "Insert".
  Enter activates it.
- Secondary button: "Cancel". Esc activates it.
- When `confirm_before_run == true`, the primary button style is `.destructive-action`
  (red).

### Sidebar row changes

Per-row suffixes (left → right):

- **Insert button** (existing) — stays one click for non-parameterized commands; for
  parameterized commands it opens the runtime dialog with Insert as the primary
  action.
- **More menu** (existing) — new items:
  - **Run** (redundant with click-to-activate, but useful when click-to-activate is
    reassigned, and discoverable)
  - **Insert** (when primary is already Insert, this becomes **Run**)
  - **Copy body** — copies the raw template (with `{{...}}` placeholders intact)
  - **Copy with parameters…** — opens the runtime parameter dialog with a "Copy"
    primary button; on confirm, writes the substituted body to the clipboard
  - **Duplicate** — creates a new `SavedCommand` with a new UUID, title suffixed
    ` (copy)` (or ` (copy N)` if the base already exists), same everything else,
    saves, and opens the editor on the new entry
  - **Edit** (existing)
  - **Delete** (existing)

Row title gets a small chip "`{{…}}`" to the right of the title when the command has
parameters, so users know the interaction will include a prompt.

### Primary-click semantics

For backwards compatibility, activating a command row still maps to its
`default_run_mode` (Run or Insert). The difference for parameterized commands is that
activation opens the runtime dialog first.

### Clipboard

Uses `gdk::Display::default().unwrap().clipboard().set_text(...)`. Success path fires
an `adw::Toast` ("Copied to clipboard"). No failure path is expected — the clipboard
API on GNOME is infallible at the GTK layer.

### Scaling primitives (bundled into this RFC; implemented in follow-ups)

Each of the items below is called out here so the cumulative design is coherent, but
each gets its own issue and is not a gate for the MVP.

- **Description field** (`SavedCommand::description`, already in the data model
  above) — multiline note, not executed, shown as row tooltip and as a section in
  the editor.
- **Labels** (`SavedCommand::labels`) — free-text tags independent of host tags,
  intended for grouping (`deploy`, `diag`, `tmux`). Proposed UI: a chip bar at the
  top of the Commands tab; clicking a chip filters the list. Interacts with the
  existing search field by AND.
- **Confirm before run** (`SavedCommand::confirm_before_run`) — when set, Run
  triggers an `adw::AlertDialog` ("Run `<title>`? This command is marked as
  requiring confirmation.") before sending to the pane. For parameterized commands,
  this replaces the primary-button styling in the parameter dialog instead.
- **Run in new pane** — a third run mode alongside Run / Insert that splits the
  active pane and runs the command in the new one. Data-model impact:
  `CommandRunMode` gains a `RunInNewPane { split: SplitDirection }` variant.
- **Command palette** — global Ctrl+K opens a fuzzy-search overlay of all commands
  across all hosts. Bound through the RFC-024 shortcut registry. The palette hands
  the selected command to the same activation path the sidebar uses, so
  parameterized commands go through the runtime dialog.

These are listed in priority order: description and labels are the most common
scaling asks; confirm-before-run and run-in-new-pane are smaller; the palette is
the largest follow-up.

### Persistence and backwards compatibility

- All new fields on `SavedCommand` use `#[serde(default)]`.
- Defaults that compare equal to their zero value are skipped on write to keep
  older JSON round-tripping byte-identical.
- No new files. `commands.json` stays the single source of truth.
- Existing tests for `commands.rs` serialization remain valid (they cover
  `SavedCommand::new()` which produces the zero-value shape for the new fields).

### Testing

Unit tests (pure, no GTK):

- `scan_placeholders` — empty body, single placeholder, duplicate placeholders,
  placeholders inside strings, adjacent placeholders, malformed `{{ }}` ignored
- `substitute` — all placeholders replaced, unknown keys preserved, empty values
  allowed, values with `$` or `"` not interpreted
- Save-time validation — missing parameter, undeclared parameter, empty choices,
  default-not-in-choices, duplicate parameter names
- Serde round-trip including the new fields, and `#[serde(default)]` load of a
  legacy command

GTK widget tests:

- Editor dialog instantiates with a parameterized command and round-trips it
- Runtime parameter dialog instantiates, preview updates on combo change,
  primary button emits expected substituted text

AT-SPI behavioural tests (follow-up):

- Activate a parameterized command from the sidebar → dialog appears → select
  default values → Enter → body reaches the pane
- "Copy body" fires a toast and places text on the clipboard (verified via a
  helper that reads the clipboard in the test)
- "Duplicate" creates a new entry and opens the editor

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 — Clone a command in one click | "Duplicate" action in the per-row overflow menu; opens the editor on the copy |
| G2 — Copy to clipboard | "Copy body" and "Copy with parameters…" actions; uses `gdk::Clipboard` with toast confirmation |
| G3 — Parameterized commands with fixed-choice drop-downs | `CommandParameter { name, label, choices, default }` model; `{{NAME}}` placeholder syntax; save-time validation |
| G4 — Parameter prompt that scales 1–6 parameters | `adw::Dialog` with one `adw::ComboRow` per parameter plus a live-substituted preview |
| G5 — Scaling primitives identified | Description, labels, confirm-before-run, run-in-new-pane, command palette — documented here, implemented in follow-up issues |

---

## Development Plan

- [ ] **Step 1** — Add `CommandParameter`, extend `SavedCommand` with
  `parameters`/`description`/`labels`/`confirm_before_run`. Add `scan_placeholders`,
  `substitute`, validation helpers. Unit tests. *(prerequisite: —)*
- [ ] **Step 2** — Extend the command editor dialog with the Parameters group and
  save-time validation banner. GTK widget tests. *(prerequisite: Step 1)*
- [ ] **Step 3** — Implement the runtime parameter dialog (`adw::Dialog` + combo
  rows + live preview) and route sidebar activation through it for parameterized
  commands. *(prerequisite: Step 2)*
- [ ] **Step 4** — "Duplicate" action in the per-row overflow menu. *(prerequisite:
  Step 1)*
- [ ] **Step 5** — "Copy body" and "Copy with parameters…" actions; toast feedback.
  *(prerequisite: Step 3 for the substituted variant)*
- [ ] **Step 6** — Sidebar chip/icon for parameterized commands. *(prerequisite:
  Step 1)*
- [ ] **Step 7** — Description field in the editor and row tooltip. *(prerequisite:
  Step 1, tracked as a follow-up issue)*
- [ ] **Step 8** — Labels + filter chip bar in the Commands tab. *(prerequisite:
  Step 1, tracked as a follow-up issue)*
- [ ] **Step 9** — Confirm-before-run flag and dialog. *(prerequisite: Step 1,
  tracked as a follow-up issue)*
- [ ] **Step 10** — Run-in-new-pane mode. *(follow-up issue; requires touching the
  layout module and may warrant its own mini-RFC)*
- [ ] **Step 11** — Command palette (Ctrl+K). *(follow-up issue; binds through the
  RFC-024 shortcut registry)*

Steps 1–6 are the MVP that satisfies the user-raised asks. Steps 7–11 are the
scaling roadmap — each gets its own issue once this RFC reaches Accepted.

---

## Open Questions

- [ ] **Q1** — Should "Copy body" copy the raw template or the substituted form when
  the command has parameters? Current answer: raw template. Substituted form is the
  separate "Copy with parameters…" action. Confirm this split matches user
  expectations.
- [ ] **Q2** — Should the confirm-before-run flag be in this RFC or split out? It is
  simple, but it has UX interactions with the parameter dialog (destructive
  styling). Current answer: keep here.
- [ ] **Q3** — Should the editor show a preview of the substituted body (using
  defaults) while editing, to help the template author see what Run will produce?
  Current answer: nice-to-have; not in the MVP.
- [ ] **Q4** — Do we need parameter reordering in the editor? With `Vec<CommandParameter>`
  the on-disk order is stable and drives dialog order. A drag handle per parameter
  row is cheap but adds widget complexity. Current answer: defer to a follow-up;
  ship with fixed insertion order.
- [ ] **Q5** — Should `Copy with parameters…` write anything other than plain text
  to the clipboard (e.g., also offer to preserve the command as a shareable JSON
  blob)? Current answer: no — plain text only. Sharing at the library level is
  RFC-023/#755 territory.

Mark questions `[x]` once resolved and record the answer inline.

---

## References

- [Tracking issue #786](https://github.com/IllyaYalovyy/rttx/issues/786)
- [RFC-006 — Places & Commands Right Sidebar](./RFC-006-commands-sidebar.md)
- [RFC-024 — Customizable Keyboard Shortcuts](./RFC-024-customizable-keyboard-shortcuts.md)
- [#44 — Pin commands and surface recent commands](https://github.com/IllyaYalovyy/rttx/issues/44)
- [#755 — Export and import user configuration](https://github.com/IllyaYalovyy/rttx/issues/755)
