# RFC-022: Client Configuration and State Store

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
daemon session discovery, direct-mode fallback, and future protocol evolution. Because rttx is
not yet v1, this RFC intentionally allows a breaking storage migration with a one-time importer
from the current development files.

---

## Goals

- **G1** - Separate durable user configuration, restorable client state, and disposable runtime
  cache using XDG-appropriate locations.
- **G2** - Introduce explicit schema versions and typed migrations for every persisted client
  document.
- **G3** - Make the host/endpoint model first-class so RFC-016 creation, attach, sidebar, place,
  and command flows do not need legacy bookmark/session translation.
- **G4** - Remove legacy concepts from the canonical store: `Bookmark` and `SessionMode` become
  import-only compatibility types.
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
- `bookmarks.json` still stores legacy path/SSH bookmark data that overlaps with hosts and
  places.
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
   Host deletion can touch hosts, places, and commands. Bookmark migration can touch hosts and
   places. Today those writes are independent, so a crash or write error can leave the store
   partially updated.

5. **Legacy and canonical concepts coexist.**
   `SessionMode` and `WorkspaceRuntime` both represent runtime intent. `Bookmark`, `Host`, and
   `Place` overlap. Code must keep translating between old and new concepts instead of relying on
   one canonical model.

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
- `Bookmark` is not canonical. Legacy bookmarks are imported into hosts and places, then archived.
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
- `policy` records whether the workspace is daemon-backed or direct fallback.
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
- `bookmarks.json`
- `sessions.json`
- `schemes/`

Migration flow:

1. Detect absence of envelope-based documents and presence of legacy files.
2. Copy legacy files into `backups/pre-v1-<timestamp>/` before writing anything new.
3. Import preferences into the new `preferences.json`.
4. Import hosts into `hosts.json`; synthesize remote hosts from legacy bookmark `ssh_target`
   values.
5. Import places into `library.json`; import bookmark directories as places tagged to the
   matching host when `ssh_target` is present.
6. Import commands into `library.json`.
7. Import sessions into `workspaces.json` and `ui.json`.
8. Move dismissed runtime IDs into `runtime-cache.json`.
9. Record the migration result in `migrations.json`.

Legacy ambiguity rules:

- Missing `host_tags` on old commands means legacy local-only content.
- Present but empty `host_tags` means intentionally global content.
- Unknown remote hosts are preserved as orphaned endpoint keys rather than discarded.
- Unsupported or malformed individual records are skipped only after being recorded in the
  migration ledger.

Post-migration rules:

- The canonical store is the new envelope-based document set.
- Old files are not written again.
- Old files may remain in the backup directory for manual recovery.
- No code path should create new canonical bookmarks or new canonical `SessionMode` records.

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
- Importing bookmarks into hosts and places.
- Preserving orphaned host tags.
- Distinguishing missing `host_tags` from intentionally empty `host_tags`.
- Recovering from malformed current documents with a usable backup.
- Preserving bad files when no usable backup exists.
- Atomic write behavior at the store abstraction boundary.
- Round-tripping workspace state without persisting live connection status.

The migration fixtures should live in the repo so future persistence changes are reviewed against
real old file shapes, not hand-waved defaults.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1   | Config, state, and cache move into separate XDG roots based on ownership and durability |
| G2   | Every document has an envelope with `schema` and `version` |
| G3   | Hosts, library tags, and workspace endpoints all use canonical endpoint keys |
| G4   | Bookmarks and `SessionMode` are import-only and excluded from canonical documents |
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
- [ ] **Step 4** - Move preferences, hosts, places, commands, and bookmarks behind the store API
  *(prerequisite: Step 3)*
- [ ] **Step 5** - Move workspace, UI, and runtime-cache state behind the store API
  *(prerequisite: Step 3)*
- [ ] **Step 6** - Remove canonical writes of legacy bookmarks and `SessionMode`
  *(prerequisite: Steps 4 and 5)*
- [ ] **Step 7** - Add follow-up issues for cleanup, diagnostics polish, and migration UX
  *(prerequisite: Steps 4 and 5)*

---

## Open Questions

- [ ] **Q1** - Should malformed-file diagnostics be shown as a startup dialog, a toast with a
  details button, or a log entry plus status indicator?
- [ ] **Q2** - Should `runtime_ref` be stored in `workspaces.json`, or should all daemon runtime
  identity live in a separate state document once protocol v3 lands?
- [ ] **Q3** - Should imported legacy files be left in place after backup, or moved out of the
  old root to make accidental fallback impossible?

---

## References

- [Issue #510: RFC: redesign client configuration and state storage](https://github.com/IllyaYalovyy/rttx/issues/510)
- [RFC-016: Workspace Management v2](./RFC-016-workspace-management-v2.md)
- [RFC-021: Client/Server Protocol v3](./RFC-021-client-server-protocol-v3.md)
