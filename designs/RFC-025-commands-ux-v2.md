# RFC-025: Commands UX v2 — Clone, Clipboard, Parameters, and Scaling

| Field         | Value         |
|---------------|---------------|
| Status        | Implemented   |
| Author(s)     | Illya Yalovyy |
| Supersedes    | —             |
| Superseded by | —             |

---

## Summary

Extend saved commands (RFC-006) with three commonly-missed affordances — **Clone**,
**Copy to clipboard**, and **Parameterized commands** — and record the additional UX
moves that let the commands library scale to hundreds of entries without turning the
sidebar into a dumping ground. Parameters use a fixed-choice drop-down model and
bash-native environment-variable references in the command body. At runtime rttx
prompts for all declared parameters, shell-escapes the chosen values, injects them as
`export` statements inside a subshell wrapper, and then runs/inserts the original
body unchanged.

---

## Goals

- **G1** — Duplicate an existing command in one click and land in the editor pre-filled
- **G2** — Copy a command body to the system clipboard without needing an active pane
- **G3** — Parameterize a command body with fixed-choice drop-down values using
  standard shell environment variables
- **G4** — Surface a runtime parameter prompt that is fast for 1–2 parameters and
  remains legible for 4–6
- **G5** — Identify (not necessarily implement) the next set of UX moves that help the
  commands library scale past a flat list of ~30 entries

## Non-Goals

- **NG1** — No arbitrary scripting / macro DSL. No loops, conditionals, arithmetic,
  command substitution, or custom templating beyond what bash itself already does
  with normal variable expansion
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
down at 100. Several scaling primitives (description field, labels, run-in-new-pane,
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
| End users    | Clone/copy-to-clipboard become one click. One parameterized command replaces N near-duplicates. Library scales with description, labels, and future command-library affordances. |
| Contributors | New optional fields on `SavedCommand` via `#[serde(default)]`. New parameter dialog widget. No changes to the daemon or the protocol. |
| Packagers    | None. No new dependencies; no new persisted files. |

---

## Considered Options

### Parameter reference syntax

#### Option A — shell environment variables: `$NAME` / `${NAME}` *(chosen)*

**Pros**: Body remains normal shell text. Users can copy the command body and run or
debug it directly in a shell. rttx does not need a custom placeholder grammar or a
body parser. Values can be injected using standard `export` statements and shell
escaping.

**Cons**: rttx gives up exact body-to-parameter validation. A command may declare a
parameter that the body never reads, or the body may reference shell variables that
are not declared in rttx. This RFC accepts that trade-off intentionally to stay out
of the debugging/validation path.

#### Option B — rttx-specific braces: `{{NAME}}`

**Pros**: Exact matching and exact discrepancy reporting are easy.

**Cons**: Requires a custom template syntax, blocks literal `{{...}}` use unless an
escape mechanism exists, and pushes rttx into a validation/debugging role that is not
worth the product complexity.

#### Option C — positional: `$1`, `$2`

**Pros**: Terse.

**Cons**: Positional order is brittle — reordering parameters breaks existing command
bodies. No self-documenting name. Same collision issue as Option B when the user
saves bash that invokes a function which itself uses `$1`.

### Runtime parameter UI

#### Option A — Modal dialog, one combo row per parameter *(chosen)*

**Pros**: Clean keyboard navigation (Tab between rows, Enter to run, Esc to cancel).
Scales to 4–6 parameters without running out of space. Room for a live preview of the
effective shell block. Matches the GNOME HIG pattern for "ask the user for input before
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
parameter prompt when the selected command has parameters. Documented in this RFC
as a scaling follow-up, not as the primary parameter UI.

### Parameter value model

#### Option A — Fixed string choices only *(chosen)*

Parameters carry a `Vec<String>` of suggested choices plus an optional default. The
user picks one at runtime; empty choices are allowed and resolve to an empty string.

**Pros**: No free-text validation. rttx only needs to present a list of values, select
one, shell-escape it, and inject it. Covers the overwhelmingly common case
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
- Parameter reference syntax: **Option A** (`$NAME` / `${NAME}` with env-var injection).
- Runtime parameter UI: **Option A** (modal dialog with one combo row per parameter).
- Parameter value model: **Option A** (fixed drop-down choices only).

Rationale: each choice minimises the ways a user can create an invalid or
surprising command while still covering the dominant workflow. The model avoids
custom template parsing and avoids turning rttx into a shell validator. It can be
extended (free-text, typed values, inline entry for 1-parameter commands) in a
successor RFC without breaking the stored format.

---

## Design

### Data model

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandParameter {
    /// Environment variable name used by the command body (for example `ENV`).
    /// rttx expects shell-compatible names but does not validate the body against them.
    pub name: String,
    /// Human-readable label shown in the runtime dialog.
    pub label: String,
    /// Suggested allowed values presented in the runtime dialog.
    pub choices: Vec<String>,
    /// Optional default.
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
}
```

All new fields use `#[serde(default)]` so existing `commands.json` files keep loading
unchanged. Default values are skipped on write to keep the on-disk JSON readable for
commands that do not use the new features.

### Runtime injection model

- The command body stays plain shell text. The author writes normal shell variable
  references such as `$ENV` or `${ENV}` if they want to use parameters.
- rttx prompts for **all declared parameters**. It does not inspect the body to decide
  whether a parameter is "used".
- rttx does not validate or rewrite the command body. If the body does not use a
  declared variable, nothing special happens; if the body references a shell variable
  that was not declared in rttx, the shell resolves it normally.
- Selected values are shell-escaped once, then injected as `export` statements inside
  a subshell wrapper so they do not leak into the user's interactive shell after the
  command finishes.
- If a parameter has no choices, or its default is absent / not present in `choices`,
  the runtime dialog falls back to the first available choice, or to the empty string
  when no choices exist. This keeps the model permissive and avoids save-time linting.

```rust
pub fn shell_escape(value: &str) -> String;
pub fn render_env_block(body: &str, values: &[(String, String)]) -> String;
```

Example rendering:

```bash
(
export ENV='prod'
export SERVICE='api'
systemctl restart "$SERVICE"
)
```

### Editor behavior

- No save-time validation of body content.
- No placeholder scanning.
- No advisory discrepancy hints. The editor stays out of shell analysis and debugging.

### Editor dialog — new "Parameters" section

A new `adw::PreferencesGroup` titled "Parameters" sits below the behavior group and
above the host-tag picker. Each parameter is one `adw::ExpanderRow` containing:

- `adw::EntryRow` for `name`
- `adw::EntryRow` for `label`
- A list of `adw::EntryRow`s for `choices`, with an "Add choice" button
- An `adw::ComboRow` bound to the current choices to set the default

Below the group: an "Add parameter" button.

### Runtime parameter dialog

Triggered when the user activates (Run or Insert) a command with
`parameters.is_empty() == false`.

- `adw::Dialog` titled with the command's `title`
- One `adw::ComboRow` per parameter, in declaration order, pre-selected to its
  `default` (first choice if no default; empty string if no choices exist)
- A "Preview" `gtk4::TextView` at the bottom with monospace font, read-only,
  updated on every combo-row change. It renders the effective shell block with the
  injected `export` statements plus the original body.
- Primary button matches the action that triggered the dialog: "Run" or "Insert".
  Enter activates it.
- Secondary button: "Cancel". Esc activates it.

### Sidebar row changes

Per-row suffixes (left → right):

- **Insert button** (existing) — stays one click for non-parameterized commands; for
  parameterized commands it opens the runtime dialog with Insert as the primary
  action.
- **More menu** (existing) — new items:
  - **Run** (redundant with click-to-activate, but useful when click-to-activate is
    reassigned, and discoverable)
  - **Insert** (when primary is already Insert, this becomes **Run**)
  - **Copy body** — copies the raw command body unchanged
  - **Duplicate** — creates a new `SavedCommand` with a new UUID, title suffixed
    ` (copy)` (or ` (copy N)` if the base already exists), same everything else,
    saves, and opens the editor on the new entry
  - **Edit** (existing)
  - **Delete** (existing)

Row title gets a small "`ENV`" chip to the right of the title when the command has
parameters, so users know the interaction will include a prompt.

### Primary-click semantics

For backwards compatibility, activating a command row still maps to its
`default_run_mode` (Run or Insert). The difference for parameterized commands is that
activation opens the runtime dialog first. The resolved action then uses the same
rendered shell block:

- **Run** — sends the rendered shell block to the pane and executes it
- **Insert** — inserts the rendered shell block into the pane without executing

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
- **Run in new pane** — a third run mode alongside Run / Insert that splits the
  active pane and runs the command in the new one. Data-model impact:
  `CommandRunMode` gains a `RunInNewPane { split: SplitDirection }` variant.
- **Command palette** — global Ctrl+K opens a fuzzy-search overlay of all commands
  across all hosts. Bound through the RFC-024 shortcut registry. The palette hands
  the selected command to the same activation path the sidebar uses, so
  parameterized commands go through the runtime dialog.
- **Per-command keyboard shortcuts (leader-prefix chord)** — optional bindings on
  individual commands so frequent ones can be invoked from the keyboard without
  opening the sidebar or the palette. Sketched below; the design choice that needs
  early sign-off is leader-prefix vs. flat global accelerators.

  **Why a leader prefix over flat accelerators.** A flat global accelerator
  (`Ctrl+Shift+D` for "deploy") works for the first handful of commands but
  collapses fast: the namespace is small, every binding competes with the terminal
  and the system shortcuts, and the user has to memorise an opaque modifier soup.
  A leader prefix (e.g. default `Ctrl+;`) opens a transient "leader active" mode,
  after which the next one or two keystrokes select a command. The namespace is
  effectively unbounded, no command shortcut collides with the terminal except for
  the single leader binding, and the chord (`Ctrl+; d k`) reads as a memorable
  mnemonic ("**d**eploy → **k**ubectl"). Spacemacs and modern editors (VS Code's
  "chord shortcut") have proven the pattern.

  **Data model**: reuse the RFC-024 shortcut registry. Action name is
  `command:<uuid>`, value is the key sequence after the leader (e.g.
  `["d", "k"]`). The leader itself is a single registry entry
  (`commands.leader-key`) so it is rebindable like any other shortcut.

  **GTK4 plumbing**: a window-level `gtk::ShortcutController` for the leader
  binding flips a transient mode flag and shows an `adw::Toast` ("Leader active —
  press a key"). While the flag is set, key events are matched against the
  per-command sequences from the registry. After a hit, no-match, or timeout
  (~3 seconds), the mode resets. For the matching loop we can either keep a small
  trie keyed on the command sequences and walk it manually inside a key controller
  callback (simpler, full control of partial-match feedback) or compose a
  `gtk::ShortcutTrigger` chain per command (less code but harder to surface
  partial-match UX).

  **Editor surface**: a single "Shortcut" row in the command editor that opens a
  capture dialog ("Press the key sequence after the leader…"). The dialog
  acknowledges each keystroke and shows the running prefix; Esc cancels, Enter
  confirms. Clearing the field removes the binding. Duplicate sequences are
  allowed intentionally so host-specific commands can reuse the same mnemonic.

  **Discoverability**: when leader mode is active, a popover lists matching
  prefixes (Spacemacs `which-key` style) so users can see what is bound under the
  current prefix without memorising. Optional, can ship later than the binding
  itself.

These are listed in priority order: description and labels are the most common
scaling asks; run-in-new-pane is smaller; the palette and per-command shortcuts are
the largest follow-ups.

### Persistence and backwards compatibility

- All new fields on `SavedCommand` use `#[serde(default)]`.
- Defaults that compare equal to their zero value are skipped on write to keep the
  on-disk JSON compact and readable.
- No new files. `commands.json` stays the single source of truth.
- Existing tests for `commands.rs` serialization remain valid (they cover
  `SavedCommand::new()` which produces the zero-value shape for the new fields).

### Testing

Unit tests (pure, no GTK):

- `shell_escape` — spaces, quotes, dollar signs, semicolons, and newlines are escaped
  into a shell-safe single value
- `render_env_block` — renders a stable wrapper block with exports in declaration
  order and leaves the original body unchanged
- Runtime fallback selection — default chosen when present, first choice otherwise,
  empty string when there are no choices
- Serde round-trip including the new fields, and `#[serde(default)]` load of a
  legacy command

GTK widget tests:

- Editor dialog instantiates with a parameterized command and round-trips it
- Runtime parameter dialog instantiates, preview updates on combo change,
  primary button emits the expected rendered shell block

AT-SPI behavioural tests (follow-up):

- Activate a parameterized command from the sidebar → dialog appears → select
  default values → Enter → rendered shell block reaches the pane
- "Copy body" fires a toast and places text on the clipboard (verified via a
  helper that reads the clipboard in the test)
- "Duplicate" creates a new entry and opens the editor

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1 — Clone a command in one click | "Duplicate" action in the per-row overflow menu; opens the editor on the copy |
| G2 — Copy to clipboard | "Copy body" action; uses `gdk::Clipboard` with toast confirmation |
| G3 — Parameterized commands with fixed-choice drop-downs | `CommandParameter { name, label, choices, default }` model; bash env-var injection; no body validation |
| G4 — Parameter prompt that scales 1–6 parameters | `adw::Dialog` with one `adw::ComboRow` per parameter plus a live preview of the rendered shell block |
| G5 — Scaling primitives identified | Description, labels, run-in-new-pane, command palette — documented here, implemented in follow-up issues |

---

## Development Plan

- [x] **Step 1** — Add `CommandParameter`, extend `SavedCommand` with
  `parameters`/`description`/`labels`. Add `shell_escape`, `render_env_block`, and
  runtime fallback helpers. Unit tests. *(prerequisite: —)* — PR #801
- [x] **Step 2** — Extend the command editor dialog with the Parameters group. GTK
  widget tests. *(prerequisite: Step 1)* — PR #801
- [x] **Step 3** — Implement the runtime parameter dialog (`adw::Dialog` + combo
  rows + live preview) and route sidebar activation through it for parameterized
  commands. *(prerequisite: Step 2)* — PR #801
- [x] **Step 4** — "Duplicate" action in the per-row overflow menu. *(prerequisite:
  Step 1)* — PR #801
- [x] **Step 5** — "Copy body" action; toast feedback. *(prerequisite: Step 1)* — PR #801
- [x] **Step 6** — Sidebar chip/icon for parameterized commands. *(prerequisite:
  Step 1)* — PR #801
- [x] **Step 7** — Description field in the editor and row tooltip. *(prerequisite:
  Step 1)* — PR #802
- [x] **Step 8** — Labels + filter chip bar in the Commands tab. *(prerequisite:
  Step 1)* — PR #803
- [x] **Step 9** — Run-in-new-pane mode. *(follow-up issue)* — PR #806
- [ ] **Step 10** — Command palette (Ctrl+K). *(follow-up issue; binds through the
  RFC-024 shortcut registry)* — tracked in #796
- [x] **Step 11** — Per-command keyboard shortcuts via leader-prefix chord.
  Reuses the RFC-024 registry with `command:<uuid>` keys; adds a leader-mode
  controller, capture dialog, and (optionally, later) a `which-key`-style
  popover. Duplicate sequences remain allowed; resolution is host/context-aware
  and uses the first matching visible command. — PR #807

Steps 1–9 and 11 are implemented. Step 10 (command palette) is tracked as a
separate follow-up in #796.

---

## Open Questions

- [x] **Q1** — Should "Copy body" copy the raw body or the rendered shell block when the command has parameters?
  - **Answer**: raw body only.
- [x] **Q2** — Should the editor show a preview of the rendered shell block (using current defaults) while editing, to help the author see what Run will produce?
  - **Answer**: not in the MVP.
- [x] **Q3** — Do we need parameter reordering in the editor? With `Vec<CommandParameter>` the on-disk order is stable and drives dialog order. A drag handle per parameter row is cheap but adds widget complexity.
  - **Answer**: fixed insertion order.
- [x] **Q4** — Should there be any clipboard action other than raw body copy?
  - **Answer**: no. There is no "Copy with parameters…" action in this RFC.
- [x] **Q5** — Per-command keyboard shortcuts: leader-prefix chord (default `Ctrl+;` then a 1–2 key sequence) vs. flat global accelerators (`Ctrl+Shift+D` per command) vs. both?
  - **Answer**: leader-prefix only. Commands may intentionally share the same sequence across hosts. Initial implementation does not reject duplicates globally; it resolves against the current host/context and uses the first matching visible command.

Mark questions `[x]` once resolved and record the answer inline.

---

## References

- [Tracking issue #786](https://github.com/IllyaYalovyy/rttx/issues/786)
- [RFC-006 — Places & Commands Right Sidebar](./RFC-006-commands-sidebar.md)
- [RFC-024 — Customizable Keyboard Shortcuts](./RFC-024-customizable-keyboard-shortcuts.md)
- [#44 — Pin commands and surface recent commands](https://github.com/IllyaYalovyy/rttx/issues/44)
- [#755 — Export and import user configuration](https://github.com/IllyaYalovyy/rttx/issues/755)
