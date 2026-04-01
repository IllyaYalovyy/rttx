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

Current implementation snapshot (2026-03):

- link detection and launch are now shared across both `TerminalWidget` and
  `PersistentPaneView`
- activation is covered by GTK regression tests for both direct and daemon-backed panes
- the launch path still uses `gio::AppInfo::launch_default_for_uri()`

---

## Goals

- **G1** — Detect common developer-facing links in terminal output
- **G2** — Open them directly with the system default handler
- **G3** — Keep the implementation scoped to terminal widgets rather than `window.rs`

## Non-Goals

- **NG1** — No custom context menu or hover popover in this RFC
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
| End users | Links and local paths in terminal output become directly clickable |
| Contributors | Behavior is isolated to shared terminal-widget helpers; no `window.rs` involvement |
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
resolved target with the system default application on click.

---

## Design

The terminal widgets configure VTE with:

- `allow-hyperlink = true` for OSC 8 support
- One regex for `http(s)` / `mailto:` / `file://` URIs
- One regex for local-looking file paths (`/tmp/x`, `./x`, `../x`, `~/x`, `src/x`)

Click handling lives on the VTE widget in both pane implementations:

1. Check for an OSC 8 hyperlink at the click position
2. Otherwise check for a regex match
3. Resolve matched text into an openable URI
4. Launch via `gio::AppInfo::launch_default_for_uri`

Path resolution rules:

- Absolute paths open directly
- `~/...` expands from the current user's home directory
- Relative paths resolve against the terminal's current working directory
- `:line[:column]` suffixes are ignored for opening purposes

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — detect useful links | VTE regex matches for URLs and local paths, plus OSC 8 support |
| G2 — open directly | `gio::AppInfo::launch_default_for_uri` |
| G3 — keep it local | All behavior lives in terminal-widget helper logic and event handlers |

---

## Development Plan

- [x] Add target-resolution tests for URLs and paths
- [x] Enable required VTE API level in `Cargo.toml`
- [x] Register VTE regex matches for URLs and paths
- [x] Add click handling that opens matches via Gio
- [x] Update user-facing docs and `META/todo.md`
