# RFC-008: Clickable Terminal Links & Paths

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented             |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

Detect actionable text in terminal output and let the user open it directly from the pane. This
includes OSC 8 hyperlinks, plain `http(s)` URLs, and local file paths such as `/tmp/rttx.log` or
`src/main.rs:42`. The feature is intentionally narrow: it improves real terminal workflows without
turning rttx into a general-purpose terminal hyperlink engine.

The implementation lives in a shared `terminal/links.rs` module used by both `TerminalWidget`
(direct panes) and `PersistentPaneView` (daemon-backed panes). Activation requires Ctrl+click,
which was added post-RFC to avoid intercepting mouse events from terminal apps (vim, htop, mc).
A right-click context menu with "Open Link" and "Copy Link" actions was also added
post-RFC.

---

## Goals

- **G1** — Detect common developer-facing links in terminal output
- **G2** — Open them directly with the system default handler
- **G3** — Keep the implementation scoped to terminal widgets rather than `window.rs`

## Non-Goals

- **NG1** — No hover popover in this RFC. A right-click context menu with "Open Link" and
  "Copy Link" was added post-RFC as a natural extension, but no hover tooltip is shown.
- **NG2** — No line/column handoff to editors; `:42:7` suffixes only resolve the file path
- **NG3** — No remote path opening or SSH-aware path translation

---

## Background & Motivation

Terminal output is full of actionable text: build logs print file paths, tools emit URLs, and
modern CLIs use OSC 8 hyperlinks. Requiring users to manually select and copy these values fights
the project's "practical tools over impressive features" principle.

The useful version of this feature is modest: identify the common cases that appear in developer
and sysadmin workflows and open them with one click.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | Links and local paths in terminal output become Ctrl+clickable; right-click context menu offers "Open Link" and "Copy Link" |
| Contributors | Behavior is isolated to `terminal/links.rs`; no `window.rs` involvement |
| Packagers | No new dependencies |

---

## Considered Options

### Option A — OSC 8 hyperlinks only *(reconstructed)*

**Pros**: Smallest possible change; no regexes.
**Cons**: Misses most real-world compiler output and log paths.

### Option B — Regex-detected URLs and paths plus OSC 8 hyperlinks

**Pros**: Covers the common cases users actually see. Keeps the feature useful without expanding
into complex terminal parsing.
**Cons**: Regex matching is heuristic and must avoid being too broad.

### Option C — Full semantic parser for paths, URLs, and editor targets *(reconstructed)*

**Pros**: Richest behavior.
**Cons**: Too much complexity for a first pass and not required to make the feature valuable.

---

## Decision

**Chosen option: B**

Use VTE regex matches for plain URLs and file paths, enable OSC 8 hyperlink support, and open the
resolved target with the system default application on Ctrl+click.

---

## Design

### Link detection

**Status: implemented** in `terminal/links.rs`.

The `configure_openable_matches()` function configures VTE with:

- `set_allow_hyperlink(true)` for OSC 8 support
- One PCRE2 regex for `http(s)` / `mailto:` / `file://` URIs
- One PCRE2 regex for local-looking file paths (`/tmp/x`, `./x`, `../x`, `~/x`, `src/x`)

Regexes are registered with VTE's `match_add_regex` using PCRE2 flags matching VTE's internal
defaults (`UTF`, `NO_UTF_CHECK`, `CASELESS`, `MULTILINE`, `DOTALL`). Matched regions show a
pointer cursor.

### Click handling

**Status: implemented** in `terminal/links.rs`.

The original RFC described plain click activation. The implementation requires Ctrl+click (#459)
so that plain mouse events pass through to VTE for mouse-aware terminal apps (vim, htop, mc).

`install_openable_link_controllers()` installs two controllers on the VTE widget:

1. **Left-click gesture** (button 1, capture phase): requires Ctrl modifier. Without Ctrl, the
   gesture is denied so VTE receives the event. On Ctrl+click, resolves URI at position and
   launches it.
2. **Hover controller**: shows pointer cursor only when Ctrl is held over a link. Uses
   state-transition tracking to avoid fighting VTE's own cursor management on every motion
   event (#479).

### Context menu

**Status: implemented** in `widget.rs` and `persistent_widget.rs` (post-RFC addition).

Plain right-click opens a context menu with "Open Link" and "Copy Link" actions.
Shift+right-click is denied so VTE receives the event for mouse-aware apps. The menu resolves the
link at the click position and enables/disables the link actions accordingly. "Copy Link"
converts file URIs to filesystem paths via `display_text_for_uri()` for clipboard friendliness.

### URI resolution

**Status: implemented** in `terminal/links.rs`.

The `openable_uri_at()` function checks in order:

1. OSC 8 hyperlink at the click position (`vte.check_hyperlink_at`)
2. VTE regex match at the click position (`vte.check_match_at`)

The `openable_uri_from_match_text()` function resolves matched text:

1. Trim trailing punctuation (`.`, `,`, `;`, `!`, `?`, `)`, `]`, `}`, `>`)
2. If text starts with a known URI prefix (`http://`, `https://`, `mailto:`, `file://`), return
   as-is
3. Strip `:line[:column]` editor suffixes
4. Check if text looks like a path (starts with `/`, `~/`, `./`, `../`, or contains `/`)
5. Resolve path: absolute paths used directly, `~/` expanded to home dir, relative paths resolved
   against terminal CWD
6. Convert resolved path to `file://` URI via `gio::File::for_path().uri()`

### Launch

**Status: implemented** in `terminal/links.rs`.

`launch_uri()` opens the resolved URI via `gio::AppInfo::launch_default_for_uri()`. A
test-injectable `TEST_URI_LAUNCHER` thread-local allows GTK tests to verify link activation
without spawning external applications.

---

## Goals Alignment

| Goal | How addressed | Status |
| --- | --- | --- |
| G1 — detect useful links | VTE regex matches for URLs and local paths, plus OSC 8 support | Implemented |
| G2 — open directly | `gio::AppInfo::launch_default_for_uri` via Ctrl+click or context menu | Implemented |
| G3 — keep it local | All behavior lives in `terminal/links.rs` and per-widget event handlers | Implemented |

---

## Development Plan

- [x] Add target-resolution tests for URLs and paths — unit tests in `links.rs` cover URL
  parsing, file URI parsing, path resolution, trailing punctuation trimming, editor suffix
  stripping, relative path resolution, and bare word rejection
- [x] Enable required VTE API level in `Cargo.toml`
- [x] Register VTE regex matches for URLs and paths — `configure_openable_matches()` in
  `links.rs`
- [x] Add click handling that opens matches via Gio — Ctrl+click via
  `install_openable_link_controllers()`; right-click context menu in both widget types
- [x] Update user-facing docs and `META/todo.md`
- [x] Add GTK widget tests for link activation in both direct and daemon-backed panes
  (post-RFC) — `direct_terminal_plain_click_does_not_launch_url`,
  `persistent_terminal_plain_click_does_not_launch_url`, capture-phase gesture tests, and
  open/copy link action tests
- [x] Add hover cursor state-transition optimization (#479, post-RFC)
- [x] Add `display_text_for_uri()` for clipboard-friendly file path display (post-RFC)

---

## Related RFCs

- **RFC-001** (manifesto) — lists Ctrl+click URL/path detection as a "practical tools" example
- **RFC-010** (maintainability refactor) — the `terminal/` module directory that houses `links.rs`
  was created as part of the `window.rs` decomposition
