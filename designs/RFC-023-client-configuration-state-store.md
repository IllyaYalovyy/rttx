# RFC-023: Client Configuration and State Store

| Field         | Value  |
|---------------|--------|
| Status        | Review |
| Author(s)     | jd2023 |
| Supersedes    | -      |
| Superseded by | -      |

---

## Summary

Redesign the client-side configuration and state store before v1 so workspace, host, command,
place, preference, and UI state evolve from a stable persistence contract instead of several
loosely related JSON files.

The current storage shape was useful while the product model was still moving quickly. It now
gets in the way of RFC-016 workspace management, host-aware places and commands, explicit
daemon session discovery, and future protocol evolution. Because rttx is
not yet v1, this RFC intentionally allows a breaking storage migration with a one-time importer
from the current development files.

---

## Goals

- **G1** - Separate durable user configuration, restorable client state, and disposable runtime
  cache using XDG-appropriate locations.
- **G2** - Introduce explicit schema versions and typed migrations for every persisted client
  document.
- **G3** - Make the host/endpoint model first-class so RFC-016 creation, attach, sidebar, place,
  and command flows do not need legacy session translation.
- **G4** - Remove legacy concepts from the canonical store: `SessionMode` becomes an
  import-only compatibility type. (`Bookmark` has already been removed.)
- **G5** - Stop silently losing user data on malformed JSON or partial writes.
- **G6** - Keep the implementation approachable: plain JSON documents, atomic file writes, and
  targeted migration tests instead of a database.
- **G7** - Make future additions natural by giving each product domain an obvious persistence
  home.

## Non-Goals

- **NG1** - Implement the store redesign in this RFC change.
- **NG2** - Redesign daemon/server persistence. This RFC covers the GUI client's local files.
- **NG3** - Add cloud sync, multi-device sync, or conflict resolution.
- **NG4** - Preserve indefinite compatibility with every pre-v1 development file shape.
- **NG5** - Change RFC-016's one-workspace-one-endpoint rule.
- **NG6** - Replace JSON with SQLite, sled, or another embedded database.

---

## Background & Motivation

The client currently persists state through independent JSON files under the profile config
directory, such as `~/.config/rttx` or `~/.config/rttx-devel`.

Current files and responsibilities:

- `sessions.json` stores window geometry, sidebar widths, workspace/tab list, layouts,
  active workspace index, direct/persistent/remote mode, runtime metadata, pane recovery,
  input sync, dismissed runtime IDs, and several legacy normalization fields.
- `preferences.json` stores terminal and UI preferences.
- `hosts.json` stores saved remote host metadata while local is built in.
- `places.json` stores saved places with host tags, plus built-in Home and Root generated in
  code.
- `commands.json` stores saved commands with host tags.
- `schemes/` stores custom color schemes.

That shape creates several structural problems.

### Current Problems

1. **Configuration, restorable state, and cache are mixed.**
   User preferences, window geometry, workspace restore data, dismissed runtime IDs, and live
   daemon attachment identifiers are all effectively treated as config. Some of those values are
   user-owned settings, some are app restore state, and some are disposable runtime cache.

2. **There is no explicit schema contract.**
   Files are parsed directly into Rust structs with `#[serde(default)]` on many fields. This is
   useful for small migrations, but it does not tell the loader which schema was written, which
   migration ran, or which fields were intentionally absent.

3. **Malformed files silently become defaults.**
   Several load paths return default state when a file is missing or cannot be parsed. Missing
   files should be normal. Malformed files should be reported, backed up, and recovered
   intentionally so user data is not silently discarded.

4. **Cross-file changes are not coherent.**
   Host deletion can touch hosts, places, and commands. Today those writes are independent, so a
   crash or write error can leave the store partially updated.

5. **Legacy and canonical concepts coexist.**
   `SessionMode` and `WorkspaceRuntime` both represent runtime intent. Code must keep translating
   between old and new concepts instead of relying on one canonical model.

6. **Runtime attachment state is too durable.**
   A workspace needs durable intent such as endpoint, policy, layout, and persistent runtime
   identity. It should not persist current connection status or temporary client/daemon
   bookkeeping as if those values were user configuration.

7. **Host-tag semantics are ambiguous during migration.**
   Empty `host_tags` now means global, but some legacy command data was originally local by
   implication. A migration needs to distinguish "field missing in legacy data" from "field
   present and intentionally empty".

8. **Future features have no obvious home.**
   RFC-016 needs explicit host-scoped create and attach flows. RFC-021 needs stable client
   identity and reconnect semantics. Terminal and session recovery work needs clear boundaries
   between durable restore state and live protocol state. The current files do not express those
   boundaries.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | More reliable startup/restart behavior, clearer host-aware content, safer migration before v1, fewer confusing bookmark/session artifacts |
| Contributors | A typed persistence contract with obvious domains, less legacy translation, and explicit migration tests |
| Packagers    | No packaging changes expected; profile directories remain under standard XDG locations |

---

## Considered Options

### Option A - Keep the current files and add more `serde(default)`

**Pros**: Lowest immediate implementation cost. Minimal code movement.

**Cons**: Keeps the root problem. New features would continue adding fields to ad hoc files,
silently defaulting malformed data, and translating between legacy and canonical concepts.

### Option B - Move client persistence into SQLite

**Pros**: Strong transactional semantics, clear migrations, good query support, and a familiar
data-evolution story.

**Cons**: Too much machinery for the current client state. The data is document-shaped, small,
and usually loaded as complete domain models. SQLite would add dependency, schema, and query
complexity without enough value before v1.

### Option C - Use versioned JSON domain documents in XDG config/state/cache

Keep JSON, but define explicit document boundaries, version every document, use typed migrations,
write atomically, and store each domain in the XDG location that matches its meaning.

**Pros**: Fixes the persistence contract without introducing a database. Easy to inspect,
backup, test with fixtures, and evolve. Matches the product model in RFC-016.

**Cons**: Cross-document operations still need careful repository code. JSON migration code must
be maintained deliberately.

---

## Decision

**Chosen option: Option C** - versioned JSON domain documents split across XDG config, state, and
cache locations.

Rationale: rttx needs a real persistence contract, not a database. JSON remains the right format
for small user-editable documents, but the current unversioned file set must be replaced by a
schema-aware store with clear ownership boundaries and migration tests.

---

## Design

### 1. Store Locations

The client should use three roots.

```text
$XDG_CONFIG_HOME/rttx/
  preferences.json
  hosts.json
  library.json
  schemes/

$XDG_STATE_HOME/rttx/
  workspaces.json
  ui.json
  migrations.json
  backups/

$XDG_CACHE_HOME/rttx/
  runtime-cache.json
```

Development profiles keep their existing isolation, for example `rttx-devel`, but follow the same
config/state/cache split.

Location rules:

- Config contains durable user choices and user-authored content.
- State contains restorable application state that should survive restart but is not user-authored
  configuration.
- Cache contains data that can be deleted without data loss.
- If `XDG_STATE_HOME` is unavailable, use the platform-standard fallback `~/.local/state/rttx`.
- Custom color schemes remain user configuration under `schemes/`.

### 2. Document Envelope

Every persisted JSON document must use a top-level envelope.

```json
{
  "schema": "rttx.client.preferences",
  "version": 1,
  "app_version": "0.7.0",
  "written_at": "2026-04-13T00:00:00Z",
  "data": {}
}
```

Envelope rules:

- `schema` identifies the document domain.
- `version` is the domain schema version, not the application version.
- `app_version` is diagnostic only.
- `written_at` is diagnostic only.
- `data` contains the typed payload.
- Unknown `schema` values are rejected.
- Unsupported future `version` values are rejected with a clear user-visible error.
- Older supported `version` values are migrated in memory and then rewritten using the latest
  version.

### 3. Canonical Documents

#### `preferences.json`

Durable user preferences only.

Examples:

- font family and size
- color scheme choice
- theme preference
- scrollback and bell behavior
- pane navigation keys
- daemon auto-start preference
- reconnect delay preference

This document must not contain workspace layout, active tab, connection status, or runtime
inventory.

#### `hosts.json`

Saved endpoint metadata.

Conceptual model:

```rust
struct HostCatalog {
    hosts: Vec<HostRecord>,
}

struct HostRecord {
    key: String,
    name: String,
    kind: HostKind,
    ssh_target: Option<String>,
    labels: Vec<String>,
}
```

Rules:

- `local` remains a reserved built-in endpoint and does not need to be persisted.
- Remote host keys remain normalized endpoint identities.
- Saved hosts are metadata layered over endpoint identity.
- Unknown host keys referenced by workspaces or library tags must remain visible as orphaned
  references until the user deletes or retags them.

#### `library.json`

User-authored launch content: places and commands.

Conceptual model:

```rust
struct Library {
    places: Vec<PlaceRecord>,
    commands: Vec<CommandRecord>,
}

struct PlaceRecord {
    id: String,
    name: String,
    path: String,
    host_tags: Vec<String>,
}

struct CommandRecord {
    id: String,
    title: String,
    body: String,
    default_run_mode: RunMode,
    host_tags: Vec<String>,
}
```

Rules:

- Empty `host_tags` means global.
- Host tags reference endpoint keys, not host display names.
- Home and Root are built-in global entries and are not persisted.
- Orphaned tags are preserved so users can find and repair content for removed hosts.

#### `workspaces.json`

Restorable workspace/tab state.

Conceptual model:

```rust
struct WorkspaceStore {
    active_workspace_id: Option<String>,
    workspaces: Vec<WorkspaceRecord>,
}

struct WorkspaceRecord {
    id: String,
    name: String,
    user_renamed: bool,
    endpoint_key: String,
    policy: WorkspacePolicy,
    runtime_ref: Option<RuntimeRef>,
    layout: LayoutNode,
    active_pane_id: Option<String>,
    zoomed_pane_id: Option<String>,
    input_sync: InputSyncState,
    color: WorkspaceColor,
    pane_recovery: Vec<PaneRecoveryRecord>,
}

struct RuntimeRef {
    runtime_id: String,
    attachment_kind: RuntimeAttachmentKind,
}
```

Rules:

- A workspace remains one tab and one endpoint, consistent with RFC-016.
- `endpoint_key` is canonical. It may be `local` or a normalized remote key.
- `policy` records the workspace runtime policy (ephemeral or persistent); both are
  daemon-backed.
- `runtime_ref` is durable identity for reconnecting to a persistent runtime, not current
  connection status.
- Live state such as connected/disconnected/reconnecting belongs in memory and protocol events,
  not in this document.
- Dismissed runtime IDs and host inventory snapshots do not belong here; they are cache.
- `SessionMode` is import-only. New code must persist `WorkspaceRecord`.

#### `ui.json`

Restorable UI state that is not workspace data.

Examples:

- window size
- maximized state
- left and right sidebar widths
- selected right-sidebar tool
- sidebar visibility

This avoids growing `workspaces.json` into a catch-all file.

#### `runtime-cache.json`

Disposable client cache.

Examples:

- dismissed runtime IDs
- last-seen runtime inventory per endpoint
- transient daemon discovery metadata
- short-lived reconnect hints

The application must behave correctly if this file is deleted.

#### `migrations.json`

Diagnostic migration ledger.

Examples:

- source files imported
- migration version applied
- backup directory created
- warnings encountered during import

This is not required for normal startup correctness, but it gives users and contributors a clear
audit trail when pre-v1 data moves.

### 4. Atomic Writes And Recovery

All document writes must be atomic.

Required write flow:

1. Serialize to a temporary file in the same directory.
2. Flush and fsync the temporary file.
3. Rename the current file to a last-good backup when replacing an existing document.
4. Rename the temporary file into place.
5. Fsync the parent directory where supported.

Load behavior:

- Missing document: create an in-memory default for that document.
- Malformed current document: move it to `backups/`, try the last-good backup, and report the
  recovery.
- Malformed current document and no usable backup: start with defaults only after preserving the
  bad file and showing a clear diagnostic.
- Unsupported future version: do not rewrite; report that the running client is too old.

### 5. Pre-v1 Migration

The implementation may make a breaking storage change, but it must provide a one-time importer for
the current development files because that is low-cost and protects active users.

Legacy sources:

- `preferences.json`
- `hosts.json`
- `places.json`
- `commands.json`
- `sessions.json`
- `schemes/`

Migration flow:

1. Detect absence of envelope-based documents and presence of legacy files.
2. Copy legacy files into `backups/pre-v1-<timestamp>/` before writing anything new.
3. Import preferences into the new `preferences.json`.
4. Import hosts into `hosts.json`.
5. Import places into `library.json`.
6. Import commands into `library.json`.
7. Import sessions into `workspaces.json` and `ui.json`.
8. Move dismissed runtime IDs into `runtime-cache.json`.
9. Record the migration result in `migrations.json`.

Legacy ambiguity rules:

- Missing `host_tags` on old commands means legacy local-only content.
  (`commands::migrate_legacy()` already handles this by tagging untagged commands with `"local"`.)
- Present but empty `host_tags` means intentionally global content.
- Unknown remote hosts are preserved as orphaned endpoint keys rather than discarded.
- Unsupported or malformed individual records are skipped only after being recorded in the
  migration ledger.

Post-migration rules:

- The canonical store is the new envelope-based document set.
- Old files are not written again.
- Old files may remain in the backup directory for manual recovery.
- No code path should create new canonical `SessionMode` records.

### 6. Store API

The implementation should introduce a small client store layer instead of letting UI modules read
and write files directly.

Conceptual API:

```rust
struct ClientStore {
    paths: StorePaths,
}

impl ClientStore {
    fn load_preferences(&self) -> Result<Preferences>;
    fn save_preferences(&self, preferences: &Preferences) -> Result<()>;
    fn load_hosts(&self) -> Result<HostCatalog>;
    fn save_hosts(&self, hosts: &HostCatalog) -> Result<()>;
    fn load_library(&self) -> Result<Library>;
    fn save_library(&self, library: &Library) -> Result<()>;
    fn load_workspaces(&self) -> Result<WorkspaceStore>;
    fn save_workspaces(&self, workspaces: &WorkspaceStore) -> Result<()>;
    fn load_ui_state(&self) -> Result<UiState>;
    fn save_ui_state(&self, ui: &UiState) -> Result<()>;
    fn load_runtime_cache(&self) -> Result<RuntimeCache>;
    fn save_runtime_cache(&self, cache: &RuntimeCache) -> Result<()>;
}
```

Rules:

- UI code should depend on domain repositories, not raw paths.
- Store paths should be injectable for tests.
- Migrations should be pure functions over versioned document values where possible.
- The store layer owns diagnostics for malformed, migrated, and recovered files.

### 7. Testing Requirements

Implementation must include regression coverage for:

- Loading each new document from a valid fixture.
- Rejecting unsupported future versions without overwriting them.
- Migrating representative current development files into the new store.
- Preserving orphaned host tags.
- Distinguishing missing `host_tags` from intentionally empty `host_tags`.
- Recovering from malformed current documents with a usable backup.
- Preserving bad files when no usable backup exists.
- Atomic write behavior at the store abstraction boundary.
- Round-tripping workspace state without persisting live connection status.

The migration fixtures should live in the repo so future persistence changes are reviewed against
real old file shapes, not hand-waved defaults.

---

## Implementation Snapshot

This RFC is in Review status. None of the proposed changes have been implemented.
The sections below document the current v1 persistence code as a baseline for future
implementation work.

### Current source locations

| Component | File | Key symbols |
|---|---|---|
| Config paths | `clients/rttx/src/config.rs` | `config_dir_path()`, `AppProfile`, `config_dir` |
| Session persistence | `clients/rttx/src/session/mod.rs` | `save_window_state()`, `load_window_state()`, `sessions_dir()` |
| Session/workspace state | `clients/rttx/src/session/state.rs` | `WindowState`, `SessionState`, `SessionMode`, `SessionColor`, `LayoutNode` |
| Runtime metadata | `clients/rttx/src/runtime.rs` | `WorkspaceRuntime`, `RuntimeEndpoint`, `WorkspacePolicy` |
| Pane recovery | `clients/rttx/src/session/recovery.rs` | `PaneRecovery`, `PaneSource`, `PaneTarget` |
| Preferences | `clients/rttx/src/preferences.rs` | `Preferences`, `PreferencesDisk`, `load()`, `save()` |
| Places | `clients/rttx/src/places.rs` | `Place`, `load()`, `save()`, `builtins()` |
| Commands | `clients/rttx/src/commands.rs` | `SavedCommand`, `load()`, `save()`, `migrate_legacy()` |
| Hosts | `clients/rttx/src/host.rs` | `Host`, `load()`, `save()`, `LOCAL_KEY` |
| Workspace state helper | `clients/rttx/src/workspace_state.rs` | `WorkspaceState` (in-memory runtime state, not persisted) |

### Current test coverage for persistence

| Test | Layer | Location |
|---|---|---|
| `save_and_load_roundtrip` | Unit | `clients/rttx/src/session/mod.rs` |
| `save_complex_layout_and_reload` | Unit | `clients/rttx/src/session/mod.rs` |
| `window_state_active_index_preserved` | Unit | `clients/rttx/src/session/mod.rs` |
| `load_returns_default_when_no_file` | Unit | `clients/rttx/src/session/mod.rs` |
| `load_returns_default_on_corrupt_json` | Unit | `clients/rttx/src/session/mod.rs` |
| `window_state_roundtrip` | Unit | `clients/rttx/src/session/state.rs` |
| `session_mode_roundtrips_through_json` | Unit | `clients/rttx/src/session/state.rs` |
| `backward_compat_session_without_mode_field` | Unit | `clients/rttx/src/session/state.rs` |
| `persistent_session_in_window_state_roundtrips` | Unit | `clients/rttx/src/session/state.rs` |
| `normalize_runtime_metadata_migrates_remote_legacy_mode` | Unit | `clients/rttx/src/session/state.rs` |
| `zoom_state_defaults_to_none_for_backward_compat` | Unit | `clients/rttx/src/session/state.rs` |
| `pane_recovery_roundtrips_structured_target` | Unit | `clients/rttx/src/session/state.rs` |
| `user_renamed_defaults_to_false_on_deserialize` | Unit | `clients/rttx/src/session/state.rs` |
| `roundtrip_via_file` | Unit | `clients/rttx/src/preferences.rs` |
| `missing_file_returns_default` | Unit | `clients/rttx/src/preferences.rs` |
| `corrupt_json_returns_default` | Unit | `clients/rttx/src/preferences.rs` |
| `partial_json_fills_defaults` | Unit | `clients/rttx/src/preferences.rs` |
| `legacy_single_color_scheme_populates_light_and_dark` | Unit | `clients/rttx/src/preferences.rs` |
| `roundtrip_via_file` | Unit | `clients/rttx/src/places.rs` |
| `missing_file_returns_empty_list` | Unit | `clients/rttx/src/places.rs` |
| `legacy_json_without_host_tags_deserializes_with_empty_vec` | Unit | `clients/rttx/src/places.rs` |
| `roundtrip_via_file` | Unit | `clients/rttx/src/host.rs` |
| `missing_file_returns_empty_list` | Unit | `clients/rttx/src/host.rs` |
| `deserialize_without_ssh_target_defaults_to_none` | Unit | `clients/rttx/src/host.rs` |
| `host_tags_roundtrip_via_file` | Unit | `clients/rttx/src/commands.rs` |
| `legacy_json_without_host_tags_deserializes_with_empty_vec` | Unit | `clients/rttx/src/commands.rs` |
| `migrate_legacy_tags_untagged_commands_with_local` | Unit | `clients/rttx/src/commands.rs` |
| `migrate_legacy_preserves_existing_tags` | Unit | `clients/rttx/src/commands.rs` |
| `workflow_persist_and_restore_with_cwds` | Integration | `clients/rttx/tests/session_lifecycle.rs` |
| `session_order_persists_through_serialization` | Integration | `clients/rttx/tests/session_lifecycle.rs` |
| `dismissed_runtime_ids_persist_through_save_load` | Integration | `clients/rttx/tests/session_lifecycle.rs` |
| `remote_managed_session_persists_and_restores` | Integration | `clients/rttx/tests/session_lifecycle.rs` |

### What exists vs what RFC-023 proposes

| RFC Feature | Current Status |
|---|---|
| XDG config/state/cache split | ❌ Everything in `$XDG_CONFIG_HOME/rttx/` via `config::config_dir_path()` |
| Document envelope (`schema`, `version`) | ❌ Bare JSON structs with `#[serde(default)]` for backward compat |
| `ClientStore` abstraction | ❌ Each module has independent `load()`/`save()` free functions |
| Atomic writes (temp + rename) | ❌ Plain `std::fs::write()` — crash during write can corrupt files |
| Malformed file recovery | ❌ Malformed files silently return defaults via `unwrap_or_default()` |
| Separate `workspaces.json` | ❌ Workspace state embedded in `sessions.json` as `WindowState` |
| Separate `ui.json` | ❌ Window geometry and sidebar widths embedded in `WindowState` |
| Separate `library.json` | ❌ Places and commands are separate files (`places.json`, `commands.json`) |
| `runtime-cache.json` | ❌ Dismissed runtime IDs embedded in `WindowState.dismissed_runtime_ids` |
| `migrations.json` ledger | ❌ No migration ledger; ad-hoc inline serde migrations only |
| `SessionMode` removal | ❌ Still serialized alongside `WorkspaceRuntime`; `sync_legacy_mode_from_runtime()` keeps both in sync |
| `Bookmark` removal | ✅ Fully removed — `PaneSource::Bookmark` only exists as a backward-compat deserialization fallback that maps to `Manual` |
| Host-tag migration | Partial — `commands::migrate_legacy()` tags untagged commands with `"local"` |
| Versioned migrations | ❌ Only inline `#[serde(default)]`, custom `Deserialize` impls, and `normalize_*()` methods |

### Deviations from original text

**`bookmarks.json` no longer exists.** The original Background section listed
`bookmarks.json` as a current file. Bookmarks have been fully removed from the
codebase since the RFC was written. The `PaneSource::Bookmark` enum variant was
removed and only exists as a backward-compat deserialization fallback in
`session/recovery.rs` that maps to `PaneSource::Manual`. No `bookmarks.json`
file is read or written. References to bookmark import in the migration flow
and testing requirements have been removed from this RFC.

**No direct-mode fallback.** The original Summary mentioned "direct-mode
fallback" as a motivation. The current architecture has no direct-mode fallback
— all workspaces are daemon-backed per RFC-013 and RFC-016. The `SessionMode`
enum still has a `Direct` variant as the default for backward compatibility,
but new workspaces always use `WorkspaceRuntime` with a daemon endpoint.

**`host_tags` migration already partially implemented.** The original Problem 7
described host-tag ambiguity as an open problem. `commands::migrate_legacy()`
already handles the primary case by tagging commands with empty `host_tags`
as `"local"`. The places module does not have an equivalent migration — legacy
places without `host_tags` deserialize with an empty vec (global visibility).

**Five current files, not six.** With `bookmarks.json` removed, the client
persists exactly five JSON files: `sessions.json`, `preferences.json`,
`hosts.json`, `places.json`, and `commands.json`, plus the `schemes/`
directory for custom color schemes.

### Relationship to RFC-022

RFC-022 (Daemon State Storage) proposes a parallel redesign for the daemon
side, also targeting `$XDG_STATE_HOME/rttx/`. The two RFCs share the same XDG
base directory but govern different subdirectories: RFC-022 covers daemon-owned
runtime state, while RFC-023 covers client-owned configuration and UI state.
Implementation should coordinate the directory layout to avoid conflicts.
RFC-022 is also in Draft status with no implementation started.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1   | Config, state, and cache move into separate XDG roots based on ownership and durability |
| G2   | Every document has an envelope with `schema` and `version` |
| G3   | Hosts, library tags, and workspace endpoints all use canonical endpoint keys |
| G4   | `SessionMode` is import-only and excluded from canonical documents. `Bookmark` is already removed. |
| G5   | Malformed files are backed up, diagnosed, and recovered intentionally |
| G6   | The design keeps JSON and avoids a database while requiring atomic writes |
| G7   | Preferences, hosts, library, workspaces, UI, and cache each have a clear home |

---

## Development Plan

- [ ] **Step 1** - Create the `ClientStore` path and document-envelope infrastructure
  *(prerequisite: this RFC accepted)*
- [ ] **Step 2** - Add canonical document models and fixture tests
  *(prerequisite: Step 1)*
- [ ] **Step 3** - Implement legacy import with backups and migration ledger
  *(prerequisite: Step 2)*
- [ ] **Step 4** - Move preferences, hosts, places, and commands behind the store API
  *(prerequisite: Step 3)*
- [ ] **Step 5** - Move workspace, UI, and runtime-cache state behind the store API
  *(prerequisite: Step 3)*
- [ ] **Step 6** - Remove canonical writes of legacy `SessionMode`
  *(prerequisite: Steps 4 and 5)*
- [ ] **Step 7** - Add follow-up issues for cleanup, diagnostics polish, and migration UX
  *(prerequisite: Steps 4 and 5)*

---

## Open Questions

- [x] **Q1** - Should malformed-file diagnostics be shown as a startup dialog, a toast with a
  details button, or a log entry plus status indicator? **Resolved:** use an `adw::Toast` with a
  details button. This follows the project's existing pattern (CONTRIBUTING.md: "Errors visible
  to the user are surfaced as `adw::Toast` notifications, not modal dialogs or console output")
  and avoids blocking startup. A log entry should also be written for diagnostic purposes.
- [ ] **Q2** - Should `runtime_ref` be stored in `workspaces.json`, or should all daemon runtime
  identity live in a separate state document once protocol v3 lands? RFC-021 (protocol v3) is
  still in Review status, so this remains open until the protocol design stabilizes.
- [x] **Q3** - Should imported legacy files be left in place after backup, or moved out of the
  old root to make accidental fallback impossible? **Resolved:** leave legacy files in place
  after copying to `backups/`. Moving files risks breaking users who downgrade. The envelope
  detection (presence of `schema` field) is sufficient to distinguish new from legacy files.

---

## References

- [Issue #510: RFC: redesign client configuration and state storage](https://github.com/IllyaYalovyy/rttx/issues/510)
  (Closed — RFC written)
- [Issue #630: Review and update RFC-023](https://github.com/IllyaYalovyy/rttx/issues/630)
- [RFC-013: Persistent Host Sessions](./RFC-013-persistent-host-sessions.md) (Implemented) —
  established the daemon-backed runtime model; no direct-mode fallback
- [RFC-016: Workspace Management v2](./RFC-016-workspace-management-v2.md) (Implemented) —
  one-workspace-one-endpoint rule that this RFC's storage model serves
- [RFC-021: Client/Server Protocol v3](./RFC-021-client-server-protocol-v3.md) (Review) —
  protocol evolution that may affect `runtime_ref` storage location (Q2)
- [RFC-022: Daemon State Storage](./RFC-022-daemon-state-storage.md) (Draft) —
  parallel daemon-side storage redesign sharing `$XDG_STATE_HOME/rttx/`
- `clients/rttx/src/config.rs` — profile and path configuration
- `clients/rttx/src/session/mod.rs` — current `WindowState` persistence
- `clients/rttx/src/session/state.rs` — `SessionState`, `SessionMode`, `WorkspaceRuntime`
