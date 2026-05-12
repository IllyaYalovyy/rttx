# RFC-028: Remote Host Management — Edit, Delete, and Organize Hosts

| Field         | Value         |
|---------------|---------------|
| Status        | Draft         |
| Author(s)     | Illya Yalovyy |
| Supersedes    | —             |
| Superseded by | —             |

---

## Summary

Add a complete host management experience to rttx. Today hosts can be added via a
minimal dialog and deleted via a sidebar button, but there is no way to edit a host
(rename it, change its SSH target, or assign labels) and no dedicated view for
reviewing the full host inventory. This RFC introduces an **Edit Host** dialog, a
**Hosts page in Preferences**, and lightweight organizational primitives (display
name, labels) so that users with many remote endpoints can manage them without
friction.

---

## Goals

- **G1** — Edit any host property (display name, SSH target, labels) without
  delete-and-recreate
- **G2** — Provide a dedicated host list in Preferences for bulk review and
  management
- **G3** — Make delete discoverable and safe with affected-items preview (refine
  existing behavior)
- **G4** — Support connection testing from the management UI so users can verify SSH
  reachability before creating workspaces
- **G5** — Allow host organization via labels for filtering in the sidebar host
  selector

## Non-Goals

- **NG1** — No SSH key management, agent forwarding configuration, or credential
  storage inside rttx
- **NG2** — No automatic host discovery (mDNS, cloud provider APIs, SSH config
  parsing)
- **NG3** — No host grouping hierarchy or folder structure — labels are flat tags
- **NG4** — No changes to the daemon, the wire protocol, or `rttx-server`
- **NG5** — No multi-select bulk operations in the initial implementation
- **NG6** — No import/export of hosts independent of the full config export (tracked
  separately)

---

## Background & Motivation

RFC-013 established that every managed workspace connects to an endpoint — either the
local daemon or a remote daemon reached over SSH. The host model (introduced in #424)
gives each endpoint a stable key derived from the SSH target, a display name, and a
kind (local/remote). Places and commands reference hosts via `host_tags`.

The current host management surface is minimal:

1. **Add Host** — a single-field dialog (`show_add_host_dialog`) that accepts an SSH
   target string. The display name is auto-derived from the hostname portion.
2. **Delete Host** — a trash-icon button visible when a remote host is selected in
   the sidebar dropdown. Clicking it shows an affected-items dialog
   (`confirm_delete_host`) listing tagged places and commands.
3. **No Edit** — once a host is added, the only way to change its display name or SSH
   target is to delete it and re-add it. This loses all host-tag associations on
   places and commands.

Pain points observed in real use:

- **Typos in SSH targets** require delete + re-add + re-tag all affected items.
- **Display names** default to the short hostname, which is often ambiguous when
  multiple hosts share a domain (e.g., `prod1`, `prod2`, `staging1` all show as
  single-word names that do not convey role).
- **The delete button** is only visible when the host is selected in the dropdown —
  users who want to clean up stale hosts must cycle through the selector one by one.
- **No connection test** means users discover SSH misconfigurations only after
  creating a workspace and watching it fail to connect.
- **No labels** means the host selector is a flat alphabetical list that does not
  scale past ~10 entries.

---

## User Impact

| Audience     | Impact |
|--------------|--------|
| End users    | Can rename hosts, fix SSH targets, assign labels, test connections, and manage the full host inventory from one place. |
| Contributors | New optional fields on `HostRecord` via `#[serde(default)]`. New Preferences page. Edit dialog reuses patterns from place/command editors. |
| Packagers    | None. No new external dependencies or persisted files. |

---

## Considered Options

### Host editing surface

#### Option A — Inline editing in the sidebar dropdown *(rejected)*

**Pros**: No new dialog. Minimal UI surface.

**Cons**: Dropdowns are selection widgets, not editing surfaces. Inline editing
conflicts with the primary purpose of the host selector (filtering sidebar content).
No room for labels, connection test, or SSH target editing.

#### Option B — Edit dialog launched from sidebar context *(chosen for quick edits)*

**Pros**: Fast access from the existing workflow. Consistent with place and command
editing patterns. Supports all fields including labels and connection test.

**Cons**: Requires a new dialog. Users managing many hosts still need to open one
dialog per host.

#### Option C — Dedicated Preferences page for host inventory *(chosen for bulk management)*

**Pros**: Single view of all hosts. Supports add, edit, delete, reorder, and
connection test without switching context. Natural home for future bulk operations.

**Cons**: Requires navigating to Preferences for management tasks. Not as fast as
sidebar-inline for single-host edits.

**Decision**: Both B and C. The sidebar provides a quick "Edit Host…" action for the
currently selected host. Preferences provides the full inventory view for bulk
management. Both launch the same Edit Host dialog.

### Connection testing

#### Option A — No connection test *(rejected)*

**Pros**: Simpler implementation.

**Cons**: Users discover SSH problems only after workspace creation fails. The error
path is slow and confusing.

#### Option B — Test button in the Edit Host dialog *(chosen)*

**Pros**: Users verify reachability at the point of configuration. Feedback is
immediate and contextual. Implementation is a simple async SSH probe.

**Cons**: Test results are point-in-time and may not reflect runtime conditions.

#### Option C — Background periodic health checks *(rejected for now)*

**Pros**: Always-current status.

**Cons**: Significant complexity. Network traffic when idle. Privacy concerns for
users who do not want rttx probing hosts in the background.

### Host organization

#### Option A — No organization beyond the flat list *(rejected)*

**Pros**: No new data model.

**Cons**: Does not scale. Users with 10+ hosts cannot find what they need quickly.

#### Option B — Flat labels (tags) on hosts *(chosen)*

**Pros**: Consistent with the existing host-tag model on places and commands. Simple
data model (`labels: Vec<String>`). Enables future sidebar filtering by label.

**Cons**: No hierarchy. Users who want nested groups must use naming conventions.

#### Option C — Hierarchical groups/folders *(rejected)*

**Pros**: Richer organization.

**Cons**: Over-engineered for the current scale. Adds tree-management complexity.
Conflicts with the flat host-selector dropdown.

---

## Decision

**Chosen options:**

- Host editing: **Options B + C** — Edit dialog from sidebar + Preferences host page
- Connection testing: **Option B** — Test button in the Edit Host dialog
- Host organization: **Option B** — Flat labels on hosts

Rationale: The edit dialog is the minimum viable fix for the "no edit" problem. The
Preferences page addresses bulk management. Labels reuse the existing tagging pattern
and prepare for future sidebar filtering. Connection testing catches the most common
failure mode (SSH misconfiguration) at the point where the user can fix it.

---

## Design

### Data model changes

The `HostRecord` model gains an optional field:

```rust
/// A saved endpoint record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostRecord {
    pub key: String,
    pub name: String,
    pub kind: HostKind,
    #[serde(default)]
    pub ssh_target: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,          // already exists, now user-editable
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,          // RFC-028 addition
}
```

The `hosts.json` schema version remains at 1 — all new fields use `#[serde(default)]`
for backward compatibility.

The domain `Host` type gains a matching `description` field:

```rust
pub struct Host {
    pub key: String,
    pub name: String,
    pub kind: HostKind,
    #[serde(default)]
    pub ssh_target: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}
```

### Key stability on SSH target change

When a user edits the SSH target, the host key changes (keys are derived from the
normalized hostname). This is a destructive operation because all `host_tags`
referencing the old key become orphaned.

Rules:

1. If the SSH target edit produces a **different normalized key**, the dialog warns:
   "Changing the SSH target will update all places and commands tagged with this
   host." On confirmation, all `host_tags` referencing the old key are rewritten to
   the new key.
2. If the new key **collides with an existing host**, the dialog shows an error:
   "A host with this target already exists." The edit is blocked.
3. Display name and label edits never change the key.

### Edit Host dialog

The Edit Host dialog is an `adw::Dialog` with:

- **Header bar** with Cancel and Save buttons
- **SSH target** — `adw::EntryRow` (read-only for the local host)
- **Display name** — `adw::EntryRow` (editable, defaults to short hostname)
- **Description** — `adw::EntryRow` (optional one-line note)
- **Labels** — `adw::EntryRow` with comma-separated input (or a tag-chip widget if
  available in the Adwaita version)
- **Test Connection** — `gtk4::Button` that runs an async SSH probe and shows
  success/failure inline via a status label
- **Delete Host** — destructive button at the bottom (launches the existing
  `confirm_delete_host` flow)

The dialog is launched from:
- Sidebar: a new "Edit Host…" item in the host selector's context/overflow menu
- Preferences: the host list row's edit action

### Preferences host page

A new page in the Preferences window (`adw::PreferencesPage`) titled "Hosts":

- **Host list** — `adw::PreferencesGroup` with one `adw::ActionRow` per saved host.
  Each row shows the display name as title, SSH target as subtitle, and an edit
  button as suffix.
- **Add Host** button at the bottom of the group (launches the existing Add Host
  dialog or a combined Add/Edit dialog).
- Local host is shown but not editable or deletable.

### Sidebar integration

The host selector dropdown gains a small overflow/context menu (accessible via a
"⋮" button or right-click on the dropdown) with:

- **Edit Host…** — opens the Edit Host dialog for the currently selected host
- **Delete Host…** — launches the existing delete confirmation flow
- **Add Host…** — launches the Add Host dialog

This replaces the current standalone delete-icon button with a more discoverable
menu that groups all host management actions.

### Connection test implementation

The test button spawns an async task that:

1. Runs `ssh -o ConnectTimeout=5 -o BatchMode=yes <ssh_target> echo ok`
2. On success: shows a green checkmark and "Connected" label
3. On failure: shows a red X and the SSH error message (first line)
4. While running: shows a spinner

The test does not require `rttx-server` on the remote host — it only verifies SSH
reachability. A separate "Test Daemon" action (future work) could verify that
`rttx-server` is installed and reachable.

### Host tag rewriting on key change

When the SSH target edit produces a new key:

```rust
pub fn rewrite_host_tags(
    old_key: &str,
    new_key: &str,
    places: &mut [Place],
    commands: &mut [SavedCommand],
) {
    for place in places.iter_mut() {
        for tag in &mut place.host_tags {
            if tag == old_key {
                *tag = new_key.to_string();
            }
        }
    }
    for cmd in commands.iter_mut() {
        for tag in &mut cmd.host_tags {
            if tag == old_key {
                *tag = new_key.to_string();
            }
        }
    }
}
```

This runs inside the Edit Host dialog's save handler, atomically with the host
record update.

---

## Goals Alignment

| Goal | How addressed |
|------|---------------|
| G1   | Edit Host dialog with name, SSH target, description, and labels fields |
| G2   | Preferences Hosts page with full inventory list and per-row edit actions |
| G3   | Sidebar overflow menu with Edit/Delete/Add; delete reuses existing affected-items dialog |
| G4   | Test Connection button in Edit Host dialog with async SSH probe |
| G5   | Labels field on HostRecord; future sidebar filtering by label |

---

## Development Plan

- [ ] **Step 1** — Add `description` field to `Host` and `HostRecord` with
  `#[serde(default)]` *(prerequisite: —)*
- [ ] **Step 2** — Implement `rewrite_host_tags` utility and unit tests
  *(prerequisite: —)*
- [ ] **Step 3** — Build Edit Host dialog with name, SSH target, description, labels,
  and key-change warning *(prerequisite: Steps 1–2)*
- [ ] **Step 4** — Add Test Connection button with async SSH probe
  *(prerequisite: Step 3)*
- [ ] **Step 5** — Replace sidebar delete button with overflow menu (Edit / Delete /
  Add) *(prerequisite: Step 3)*
- [ ] **Step 6** — Add Hosts page to Preferences window *(prerequisite: Step 3)*
- [ ] **Step 7** — Add sidebar host-selector label filtering (future follow-up)
  *(prerequisite: Step 6)*

Steps 1–6 are the initial implementation scope. Step 7 is a follow-up tracked
separately.

---

## Open Questions

- [ ] **Q1** — Should the Edit Host dialog support editing the SSH target for hosts
  that were auto-discovered from workspace connections (ad-hoc hosts not in the saved
  list)? Proposed answer: yes, editing an ad-hoc host promotes it to a saved host.
- [ ] **Q2** — Should labels be free-form text or drawn from a predefined set?
  Proposed answer: free-form, consistent with command/place host tags.
- [ ] **Q3** — Should the Preferences host page show connection status (last
  successful connect time)? Proposed answer: not in the initial implementation;
  track as future work.

---

## References

- [Tracking issue](https://github.com/IllyaYalovyy/rttx/issues/893)
- [RFC-013: Daemon-Backed Workspaces and Runtimes](./RFC-013-persistent-host-sessions.md)
- [RFC-023: Client Configuration State Store](./RFC-023-client-configuration-state-store.md)
- [RFC-027: Places UX v2](./RFC-027-places-ux-v2.md)
- [#424: Introduce canonical Host model](https://github.com/IllyaYalovyy/rttx/issues/424)
- [#431: Add host deletion cleanup dialog](https://github.com/IllyaYalovyy/rttx/issues/431)
