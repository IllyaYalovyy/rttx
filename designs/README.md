# Design Notes

This directory contains both current architectural RFCs and older design records that describe
earlier phases of the project.

## Current terminology

- **Workspace** — top-level GUI object in the sidebar
- **Runtime** — daemon-owned live backend object attached to a workspace
- **Pane** — one terminal tile inside a workspace/runtime
- **Layout** — pane arrangement inside a workspace
- **Endpoint** — the local daemon or one remote host daemon
- **Policy** — `ephemeral` or `persistent`; both are daemon-backed

Current Rust code still uses `Session*` names in several modules and persisted types. When a
design doc says `session`, check whether it is using the historical product term or referring to a
concrete code type.

## Current architecture baseline

- Managed execution is daemon-backed for both local and remote endpoints.
- Policy is `ephemeral` or `persistent`; both are daemon-backed.
- There is no implicit fallback from a managed workspace to a different execution model.
- Workspaces are homogeneous: one endpoint and one policy per workspace.
- The same window may contain multiple workspaces with different endpoints and policies.
- GUI/daemon reconciliation is non-destructive. Missing GUI metadata must never delete a daemon
  runtime or pane automatically.

## Reading older RFCs

- [RFC-001-manifesto.md](./RFC-001-manifesto.md) and
  [RFC-013-persistent-host-sessions.md](./RFC-013-persistent-host-sessions.md) define the current
  product direction.
- [RFC-010-maintainability-refactor.md](./RFC-010-maintainability-refactor.md) is the active
  maintainability roadmap. Some slices are implemented on `mainline`, but the large file/module
  decomposition work is still in progress.
- [RFC-012-ci-cd-pipeline.md](./RFC-012-ci-cd-pipeline.md) now describes the live GitHub Actions
  workflows rather than a future-only plan.
- [RFC-007-session-recovery.md](./RFC-007-session-recovery.md) still matters for recipe-based
  recovery and retry UX, but its architecture assumptions are historical.
- [RFC-011-flatpak-native-host-integration.md](./RFC-011-flatpak-native-host-integration.md)
  remains relevant for packaging and host integration, but daemon-related assumptions are
  superseded by RFC-013.
