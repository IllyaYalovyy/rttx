# RFC-020: Terminal Response Ownership

| Field         | Value                                    |
|---------------|------------------------------------------|
| Status        | Accepted                                 |
| Author(s)     | Illya Yalovyy                            |
| Supersedes    | —                                        |
| Superseded by | —                                        |

---

## Summary

The daemon must be the single authoritative owner of terminal response semantics so that
applications behave identically whether or not a GUI client is attached.

---

## Goals

- **G1** — Daemon answers all terminal queries that applications depend on, without requiring an
  attached client.
- **G2** — Detached persistent sessions never hang or misconfigure applications because no client
  VTE is present to generate responses.
- **G3** — Responses are consistent regardless of client attachment state.

## Non-Goals

- **NG1** — Full cell-grid terminal emulation in the daemon. The daemon only needs to answer
  queries, not render.
- **NG2** — Matching VTE's exact response strings byte-for-byte. Reasonable xterm-compatible
  responses are sufficient.
- **NG3** — Handling DCS (Device Control String) passthrough or Sixel graphics.

---

## Background & Motivation

In a normal GNOME Terminal session, VTE owns the PTY and answers all terminal queries directly.
In rttx's daemon architecture, the daemon owns the PTY while the client VTE is a renderer and
input producer. This splits responsibility for terminal responses:

1. An application writes a query (e.g., `CSI c` for device attributes) to its stdout.
2. The daemon reads it from the PTY master side and feeds it to `PaneScreen`.
3. The daemon broadcasts the output to attached clients as Delta messages.
4. The client VTE processes the output and may generate a response via `commit`.
5. The client forwards the commit data back to the daemon as Input.
6. The daemon writes it to the PTY.

If no client is attached at step 4, steps 5–6 never happen and the application hangs waiting for
a response. Even with a client attached, there is a round-trip latency penalty and a window where
the response races with other input.

### Current state

The daemon's `ScreenPerformer` currently handles:

| Query | Response | Status |
|-------|----------|--------|
| DSR 5 (operating status) | `CSI 0 n` | ✅ Daemon answers |
| DSR 6 (cursor position) | `CSI row ; col R` | ✅ Daemon answers |
| DECSET/DECRST 1 | Application cursor keys mode tracking | ✅ Tracked |
| DECSET/DECRST 1000/1002/1003 | Mouse tracking mode | ✅ Tracked |
| DECSET/DECRST 1006 | SGR mouse mode | ✅ Tracked |
| DECSET/DECRST 2004 | Bracketed paste mode | ✅ Tracked |
| DECKPAM/DECKPNM | Application keypad mode | ✅ Tracked |

The following are **not** handled by the daemon and currently depend on client VTE:

| Query | Response | Risk |
|-------|----------|------|
| DA1 (primary device attributes, `CSI c`) | `CSI ? 64 ; ... c` | Applications hang if detached |
| DA2 (secondary device attributes, `CSI > c`) | `CSI > 1 ; ... c` | Applications hang if detached |
| DA3 (tertiary device attributes, `CSI = c`) | `DCS ! \| ... ST` | Rare, low risk |
| XTVERSION (`CSI > 0 q`) | `DCS > \| ... ST` | Rare, low risk |
| DECRQM (`CSI ? Ps $ p`) | `CSI ? Ps ; Pm $ y` | vim, tmux use this |
| DSR 6 with DECOM | Adjusted CPR | Edge case |
| DECSET/DECRST 1004 (focus events) | Focus in/out sequences | Not tracked |
| DECSET/DECRST 25 (cursor visibility) | — | Not tracked |
| DECSET/DECRST 12 (cursor blink) | — | Not tracked |
| OSC 10/11 (fg/bg color query) | `OSC 10;rgb:... ST` | Rare, theme detection |
| OSC 52 (clipboard) | Clipboard data | Security-sensitive |

### Practical impact

The highest-impact gaps are DA1 and DA2. Many applications (bash, zsh, vim, tmux, fish, neovim)
send DA1 during startup to detect terminal capabilities. If the daemon cannot answer, these
applications hang until a client attaches and the VTE response round-trips back.

DECRQM is used by vim and tmux to query specific mode states. Focus event tracking (1004) is
used by neovim and tmux to detect window focus changes.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Persistent sessions work reliably when detached; no hangs on reattach |
| Contributors | Clear ownership model for where terminal responses live |
| Packagers    | None |

---

## Considered Options

### Option A — Daemon answers all queries

The daemon generates responses for DA1, DA2, and DECRQM directly in `ScreenPerformer`, using
xterm-compatible response strings. No client involvement needed.

**Pros**: Detached sessions always work. No round-trip latency. Single source of truth.
**Cons**: Must maintain response tables. Cannot leverage VTE's own response logic.

### Option B — Proxy responses from attached VTE, queue queries when detached

When a client is attached, let VTE answer naturally. When detached, queue queries and replay
them when a client attaches.

**Pros**: Leverages VTE's exact responses.
**Cons**: Detached sessions still hang until reattach. Queue management is complex. Response
timing is unpredictable.

### Option C — Hybrid: daemon answers critical queries, VTE handles the rest

The daemon answers DA1, DA2, DSR, and DECRQM. OSC color queries and clipboard are left to the
client VTE since they depend on actual GUI state.

**Pros**: Covers the practical cases. Keeps daemon simple. GUI-dependent queries stay with GUI.
**Cons**: Some edge cases still depend on client attachment.

---

## Decision

**Chosen option: Option C (hybrid)**

DA1, DA2, and DECRQM are the queries that cause real application hangs. These have well-defined
xterm-compatible responses that the daemon can generate without GUI state. OSC color queries and
clipboard operations genuinely depend on the client and are rare enough that the current
proxy-via-commit behavior is acceptable.

Focus event tracking (DECSET 1004) should be tracked as mode state so the daemon can inform
newly-attached clients, but the actual focus in/out sequences are generated by the client since
only the GUI knows focus state.

---

## Design

### Phase 1: DA1 and DA2 responses (high priority)

Add DA1 and DA2 response generation to `ScreenPerformer::csi_dispatch`:

- **DA1** (`CSI c` or `CSI 0 c`): respond with `CSI ? 64 ; 1 ; 2 ; 6 ; 22 c`
  (VT420 with 132-column, printer, selective erase, ANSI color).
  This matches what VTE sends and is what applications expect from a modern terminal.

- **DA2** (`CSI > c` or `CSI > 0 c`): respond with `CSI > 65 ; 0 ; 0 c`
  (VT520-family, version 0). This is a safe generic response.

Both are generated as pending replies and written to the PTY by the existing reply mechanism.

### Phase 2: DECRQM responses (medium priority)

Add DECRQM (`CSI ? Ps $ p`) handling for modes the daemon already tracks:

| Mode | Description | Source |
|------|-------------|--------|
| 1 | Application cursor keys | `application_cursor_keys` |
| 1000 | Mouse tracking (basic) | `mouse_tracking_mode` |
| 1002 | Mouse tracking (button-event) | `mouse_tracking_mode` |
| 1003 | Mouse tracking (any-event) | `mouse_tracking_mode` |
| 1004 | Focus events | `focus_event_mode` (new) |
| 1006 | SGR mouse mode | `sgr_mouse_mode` |
| 2004 | Bracketed paste | `bracketed_paste_mode` |

Response format: `CSI ? Ps ; Pm $ y` where Pm is 1 (set), 2 (reset), or 0 (not recognized).

### Phase 3: Focus event mode tracking

Add `focus_event_mode` field to `ScreenPerformer` to track DECSET/DECRST 1004. This does not
generate focus sequences (that is the client's job) but allows DECRQM to report the mode state
and lets the daemon inform newly-attached clients whether the application expects focus events.

### Phase 4: Cursor visibility tracking

Add `cursor_visible` field to track DECSET/DECRST 25. This is informational for client
reconnection — when a client attaches, it should know whether the cursor is supposed to be
visible.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1   | DA1, DA2, DSR, and DECRQM answered by daemon directly |
| G2   | No client needed for critical query responses |
| G3   | Same response regardless of attachment state |

---

## Development Plan

- [x] **Step 1** — RFC and investigation (this document)
- [ ] **Step 2** — DA1 and DA2 responses in `ScreenPerformer` with tests *(#462)*
- [ ] **Step 3** — Focus event mode tracking (DECSET 1004) with tests *(#463)*
- [ ] **Step 4** — DECRQM responses for tracked modes with tests *(#464)*
- [ ] **Step 5** — Cursor visibility tracking (DECSET 25) *(#465)*

---

## Open Questions

- [ ] **Q1** — Should the daemon suppress DA1/DA2 queries from reaching the client VTE to avoid
  duplicate responses? Current design: yes, the daemon answers and the raw bytes still reach the
  client for display, but VTE should not generate a second response because VTE only responds to
  queries on its own PTY, not to replayed output.

---

## References

- [VTE source — response handling](https://gitlab.gnome.org/GNOME/vte)
- [xterm ctlseqs — Device Attributes](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
- [ECMA-48 — Control Functions](https://ecma-international.org/publications-and-standards/standards/ecma-48/)
- GitHub issue #461
