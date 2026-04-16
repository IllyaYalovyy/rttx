# RFC-017: File Logging

| Field         | Value                                                       |
|---------------|-------------------------------------------------------------|
| Status        | Implemented                                                 |
| Author(s)     | Illya Yalovyy                                               |
| Supersedes    | —                                                           |
| Superseded by | —                                                           |

---

## Summary

Both the GUI and daemon write structured logs to files with daily rotation and automatic cleanup,
replacing the previous stderr-only logging that was invisible in production.

---

## Goals

- **G1** — Reliable post-mortem troubleshooting for sporadic production failures
- **G2** — Zero-configuration: logging works out of the box with sensible defaults
- **G3** — Bounded resource usage: logs never consume unbounded disk, CPU, or memory

## Non-Goals

- **NG1** — Structured logging (JSON format) — not needed until we have log aggregation
- **NG2** — Remote log shipping or centralized collection
- **NG3** — Per-workspace or per-pane log files
- **NG4** — GUI log viewer widget

---

## Background & Motivation

Before this change, production troubleshooting was nearly impossible:

- The GUI logged to stderr via `pretty_env_logger`. When launched from the desktop file, stderr
  went to the systemd journal — but only at ERROR level by default. All reconnect, disconnect,
  heartbeat, and attach logic logged at info/debug and was completely invisible.
- The daemon's stderr went to `/dev/null` when auto-started (daemonized). No log file was written.
- There was no built-in command to view logs for either component.

When workspaces died sporadically (#407), the only evidence was a handful of ERROR lines in the
journal — not enough to diagnose root cause. The investigation required `sudo journalctl` with
specific PID filters, and even then the logs lacked context.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Logs are written automatically; `rttx-server logs` shows the log directory |
| Contributors | Can diagnose issues from log files without special setup |
| Packagers    | No packaging changes; logs use standard XDG cache directories |

---

## Considered Options

### Option A — Keep stderr logging, add `RUST_LOG` documentation

**Pros**: No code changes.
**Cons**: Doesn't solve the core problem — stderr goes to /dev/null for daemonized processes and
to the journal for desktop-launched apps. Users must configure `RUST_LOG` manually and know where
to look.

### Option B — `tracing` + `tracing-appender` file logging

**Pros**: Both components already use `tracing` (daemon) or `log` (GUI, compatible via
tracing-log bridge). `tracing-appender` provides daily rotation out of the box. Minimal new
dependencies.
**Cons**: `tracing-appender` rotation creates one file per day regardless of size. Very large
single-day logs are possible under pathological conditions.

### Option C — `log` + `fern` with manual rotation

**Pros**: More control over rotation (size-based).
**Cons**: Adds a new dependency (`fern`). The GUI would need to keep `log` and add `fern`, while
the daemon already uses `tracing` — two different logging stacks.

---

## Decision

**Chosen option: Option B** — `tracing` + `tracing-appender`

Rationale: Unifies both components on the same logging stack. The GUI switches from
`pretty_env_logger` to `tracing-subscriber`, which captures existing `log::info!()` calls via the
built-in tracing-log bridge. Daily rotation is sufficient for the current scale — a single rttx
instance produces at most a few MB of logs per day at info level.

---

## Design

### Log file locations

| Component | Production | Development |
|---|---|---|
| GUI | `$XDG_CACHE_HOME/rttx/rttx.log.<date>` | `$XDG_CACHE_HOME/rttx-devel/rttx.log.<date>` |
| Daemon | `$XDG_CACHE_HOME/rttx-server/rttx-server.log.<date>` | `$XDG_CACHE_HOME/rttx-server-devel/rttx-server.log.<date>` |

The daemon's log files share the same directory as `state.json` and `scrollback/`. This keeps all
daemon artifacts in one place.

### Default log levels

| Mode | GUI | Daemon |
|---|---|---|
| Production | `rttx=info,warn` | `info` |
| Development | `debug` | `debug` |

`RUST_LOG` environment variable overrides the default when set.

### Rotation and cleanup

- **Rotation**: daily, via `tracing_appender::rolling::daily`. A new file is created at midnight.
- **Cleanup**: on startup, `cleanup_old_logs()` removes rotated files beyond `keep_days + 1`
  (currently 3 days → keeps 4 files: today + 3 previous days).
- **Disk budget**: ~50 MB worst case across both components. At info level, typical daily output
  is 1–5 MB per component.

Cleanup runs only at startup, not continuously. This is intentional — it avoids background timers
and filesystem polling. If the daemon runs for weeks without restart, old logs accumulate but are
bounded by the rotation (one file per day, each a few MB).

### CLI access

```
rttx-server logs    # prints the log directory path
```

The command prints the directory, not a specific file, because the current log file name includes
the date. Users can then `ls`, `tail -f`, or `cat` as needed:

```bash
tail -f "$(rttx-server logs)"/rttx-server.log.*
```

### Implementation details

- **Daemon** (`services/rttx-server/src/logging.rs`): `init_file_logging()` and
  `cleanup_old_logs()` are public functions in the `logging` module. `main.rs` calls
  `init_tracing()` — a local wrapper around `init_file_logging()` — in both `start()` and
  `attach_stdio()` paths.
- **GUI** (`clients/rttx/src/application.rs`): `init_logging()` replaces the previous
  `pretty_env_logger` initialization. The GUI uses `log` crate macros (`log::info!()`,
  `log::warn!()`, etc.) with `tracing-subscriber` as the backend — the built-in tracing-log
  bridge captures all `log` calls automatically. `cleanup_old_logs()` is duplicated (not shared
  across crates) to avoid adding a shared utility crate for a single function.
- **ANSI disabled**: file output uses `.with_ansi(false)` since log files are not terminals.
- **No stderr output**: tracing output goes only to files, not to stderr. Pre-logging startup
  errors (e.g., "already running", "failed to daemonize") still use `eprintln!` since the
  tracing subscriber is not yet initialized at that point. For interactive debugging, use
  `RUST_LOG=debug cargo run -p rttx-server -- start --foreground` which still writes to the
  log file.

### Future considerations

- **Size-based rotation**: if daily files grow too large, a custom appender or a third-party
  crate (e.g., `tracing-rolling-file`) would be needed — `tracing-appender` only supports
  time-based rotation (`DAILY`, `HOURLY`, `MINUTELY`), not size-based.
- **Log deduplication**: repeated identical errors (e.g., "pane not found" ×5) could be collapsed
  with a rate-limiting layer. Not implemented yet — the current volume is manageable.
- **GUI log access**: a menu action or `--show-logs` CLI flag to open the log directory in the
  file manager. Deferred — `rttx-server logs` covers the immediate need for the daemon, and the
  GUI log path is documented.

---

## Current implementation snapshot (2026-04)

File logging is fully implemented for both the GUI client and the daemon. All design goals are met.

### Dependencies

| Crate | Version | Used by |
|---|---|---|
| `tracing` | 0.1 | Daemon (direct macros) |
| `tracing-appender` | 0.2 | GUI and daemon (daily file rotation) |
| `tracing-subscriber` | 0.3 (with `env-filter`) | GUI and daemon (formatting + `RUST_LOG` filtering) |
| `log` | 0.4 | GUI (macros, captured by tracing-log bridge) |

### Source locations

| Component | File | Key symbols |
|---|---|---|
| Daemon logging | `services/rttx-server/src/logging.rs` | `init_file_logging()`, `cleanup_old_logs()` |
| Daemon init | `services/rttx-server/src/main.rs` | `init_tracing()` — calls `init_file_logging()` in `start()` and `attach_stdio()` |
| Daemon CLI | `services/rttx-server/src/main.rs` | `logs()` — prints `cache_dir()` path |
| GUI logging | `clients/rttx/src/application.rs` | `init_logging()`, `cleanup_old_logs()`, `log_dir_path()` |
| GUI log dir | `clients/rttx/src/application.rs` | `log_dir_path()` — `$XDG_CACHE_HOME/{rttx,rttx-devel}/` |
| Daemon log dir | `services/rttx-server/src/os/unix.rs` | `cache_dir()` — `$XDG_CACHE_HOME/{rttx-server,rttx-server-devel}/` |

### Test coverage

| Test | Layer | Location |
|---|---|---|
| `cleanup_removes_oldest_files_beyond_keep_days` | Unit | `services/rttx-server/src/logging.rs` |
| `cleanup_is_noop_when_fewer_files_than_limit` | Unit | `services/rttx-server/src/logging.rs` |
| `cleanup_ignores_unrelated_files` | Unit | `services/rttx-server/src/logging.rs` |
| `cleanup_handles_missing_directory` | Unit | `services/rttx-server/src/logging.rs` |
| `cleanup_old_logs_keeps_correct_number_of_files` | Integration | `services/rttx-server/tests/logging_integration.rs` |
| `cleanup_old_logs_does_not_panic_on_missing_dir` | Integration | `services/rttx-server/tests/logging_integration.rs` |

### Deviations from original design

None. The implementation matches the design as specified.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1   | Logs are always written at info level, capturing reconnect/disconnect/attach events |
| G2   | No configuration needed; works immediately after install |
| G3   | Daily rotation + 3-day cleanup; ~50 MB worst-case budget |

---

## Development Plan

- [x] **Step 1** — Add `tracing-appender` to daemon, write to cache dir
- [x] **Step 2** — Replace `pretty_env_logger` with `tracing-subscriber` in GUI
- [x] **Step 3** — Add `rttx-server logs` command
- [x] **Step 4** — Add unit and integration tests for cleanup logic
- [x] **Step 5** — Update README and write this RFC

---

## Open Questions

*None remaining.*

---

## References

- [#407 — unsolicited pane-not-found errors leave workspace stuck](https://github.com/IllyaYalovyy/rttx/issues/407) — the incident that motivated this work
- [#408 — feat: first-class log access](https://github.com/IllyaYalovyy/rttx/issues/408) — the tracking issue
- [#409 — implementation PR](https://github.com/IllyaYalovyy/rttx/pull/409)
- [tracing-appender docs](https://docs.rs/tracing-appender/latest/tracing_appender/)
