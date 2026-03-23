# RFC-011: Flatpak-First Distribution with Native Host Integration

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Draft                   |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

Ship `rttx` as a carefully designed Flatpak that uses current GNOME runtime libraries while
preserving terminal-native host workflows as much as possible.

The core decision is:

- package the UI against the GNOME Flatpak runtime
- ship a conservative default manifest first
- make deeper host integration an explicit opt-in profile
- keep the sandbox narrow by default
- add only the permissions that materially improve real terminal workflows
- document optional overrides for advanced host integration instead of making the base manifest
  maximally permissive

This is a refinement of the Flatpak line item in RFC-005. It treats Flatpak not as a generic
distribution format, but as a first-class product surface for a terminal emulator.

---

## Goals

- **G1** — Make `rttx` installable on a broad range of modern Linux distributions
- **G2** — Decouple `rttx` from distro library lag, especially GTK/libadwaita/VTE packaging gaps
- **G3** — Preserve host-native terminal behavior for shell, SSH, tmux, paths, notifications, and
  theme integration when the user opts into native mode
- **G4** — Keep the base Flatpak understandable and defensible in terms of permissions
- **G5** — Provide a supportable setup story with explicit documentation for edge cases

## Non-Goals

- **NG1** — Do not preserve a strict sandbox at the expense of terminal usability
- **NG2** — Do not mirror every host customization automatically through blanket filesystem access
- **NG3** — Do not introduce a large helper daemon unless the first shipping design proves
  insufficient
- **NG4** — Do not maintain parallel packaging logic inside the app code for every distro

---

## Background & Motivation

The immediate trigger is Ubuntu 24.04.

The distro ships an older GTK4 VTE than the current crate floor requested by `Cargo.toml`, while
the app otherwise fits the GTK/libadwaita stack well. Native `.deb` packaging can solve this only
by backporting or replacing core `libvte` packages, which creates system-level package-management
risk.

Flatpak changes the problem shape:

- the app can target a newer GNOME runtime across distributions
- installation becomes consistent across distros
- host library lag stops blocking the release channel

However, a terminal emulator is a worst-case Flatpak candidate if designed naively.

`rttx` is not a document viewer or a note-taking app. Its whole job is to sit on top of the host's
real shell environment, toolchain, SSH setup, tmux sessions, and directory layout. If we simply
bundle `rttx` into a sandbox and run `/bin/bash` inside the sandbox, we get portability but lose
the product.

That makes the design question more specific:

> Can Flatpak be used as the distribution vehicle while `rttx` still feels like a native terminal?

The answer is yes, but only if host integration is designed deliberately rather than treated as an
afterthought.

---

## Current App Behavior Relevant to Flatpak

The current codebase is a good base for this design:

- [`src/terminal/widget.rs`](/path/to/rttx/src/terminal/widget.rs) spawns the shell in
  one place via `vte.spawn_async()`
- bookmark/session recovery already feeds ordinary shell commands such as `ssh` and `tmux` into the
  terminal rather than depending on custom IPC
- clickable paths and links open through
  [`gio::AppInfo::launch_default_for_uri()`](/path/to/rttx/src/terminal/widget.rs#L520),
  which aligns well with portal-backed desktop integration
- notifications go through
  [`gio::Notification`](/path/to/rttx/src/window.rs#L1718), which GTK desktops and
  portals already understand
- config lives under `glib::user_config_dir()` in
  [`src/config.rs`](/path/to/rttx/src/config.rs), so the app already respects XDG-style
  storage

This means we do not need to redesign session recovery or UI architecture to make Flatpak possible.
The main work is in terminal launch policy, manifest permissions, and polish around integration.

---

## External Constraints from Flatpak / Flathub

The current Flatpak and Flathub documentation establishes a few hard constraints:

- Flatpak sandboxes have no host file access, no network, and no host process visibility by
  default. Portals are intended to replace some of that access rather than blanket permissions.
- GTK integrates with portals for URI opening and notifications, which is a good fit for `rttx`'s
  existing GTK usage.
- Theme integration should use Flatpak theme extensions and the Settings portal, not direct access
  to `~/.themes`.
- `--talk-name=org.freedesktop.Flatpak` is considered sensitive because it allows launching
  arbitrary host commands with `flatpak-spawn --host`; Flathub grants it only case-by-case.
- `--socket=ssh-auth` is also sensitive, but it is a standard documented permission for SSH-aware
  applications.
- Blanket `home`, `host`, `xdg-config`, `xdg-data`, or `xdg-cache` access is discouraged and
  receives linter warnings on Flathub.

The relevant consequence is simple:

- a terminal-quality Flatpak is feasible
- but it must justify its exceptions and avoid sloppy permissions

---

## Considered Options

### Option A — Pure sandbox terminal

Build `rttx` against the Flatpak runtime and run the default shell inside the sandbox.

**Pros**
- easiest manifest
- strongest sandbox
- easiest Flathub review

**Cons**
- wrong shell/tooling context
- host SSH config, host tmux, and host project layout stop being the primary execution context
- does not meet the product goal for a developer terminal

### Option B — Broad-permission Flatpak with sandbox shell

Grant `home`, network, SSH agent, and related access, but still spawn a sandbox shell.

**Pros**
- fewer app code changes
- many host files become visible

**Cons**
- still not a real host terminal
- `ssh`, `tmux`, shell init, and tooling come from the sandbox image unless separately bundled
- wide permissions without fixing the real abstraction breach

### Option C — Flatpak UI + host shell via `flatpak-spawn --host`

Keep the app sandboxed as a GUI, but have each terminal session launch its shell on the host.

**Pros**
- preserves host shell, host toolchain, host SSH/tmux, and host directory behavior
- matches what users mean by a "native-feeling" terminal
- keeps the GUI/runtime packaging portable across distros

**Cons**
- requires restricted `org.freedesktop.Flatpak` access
- increases implementation and QA complexity
- introduces a mixed execution model: sandboxed UI, host processes

### Option D — Flatpak UI + full host-side agent from day one

Ship a dedicated host helper to manage shell startup and process inspection.

**Pros**
- strongest long-term base for advanced shell/process semantics
- can solve namespace visibility problems more comprehensively

**Cons**
- significantly more implementation and support complexity
- premature for current `rttx`

---

## Decision

**Chosen option: conservative base manifest plus Option C as an opt-in mode, with Option D
deferred**

The initial Flatpak should be safe and reviewable by default:

- sandbox-shell mode out of the box
- portal-backed desktop integration where available
- no restricted host-command permission in the base manifest

Then `rttx` should document and support an opt-in native mode:

- host shell execution through `flatpak-spawn --host`
- optional extra overrides for specific advanced workflows

This lowers the policy and support burden of the default install while still preserving a path to
the terminal experience power users actually want.

---

## Design

### 1. Runtime strategy

Package `rttx` against the current stable `org.gnome.Platform` / `org.gnome.Sdk` runtime branch at
the time of release. As of this research, Flathub is already shipping GNOME 49-era apps, which is
new enough to make Flatpak a credible "latest libraries" channel.

Practical policy:

- target the latest stable GNOME runtime branch in the manifest
- verify the runtime's `gtk4`, `libadwaita-1`, and `vte-2.91-gtk4` versions in CI
- if the runtime VTE version ever falls below the Rust crate floor, bundle VTE as a manifest module
  rather than lowering the app to the lowest common distro denominator

This keeps the release channel aligned with application needs, not distro timing.

Implementation note from the first local Flatpak build:

- `org.gnome.Sdk//49` provides `gtk4` and `libadwaita-1`
- it does **not** provide `vte-2.91-gtk4`

So for the current GNOME 49 target, bundling VTE is not a hypothetical fallback. It is required for
the build to succeed.

### 2. Product profiles

The Flatpak design should explicitly support two profiles.

#### Profile A — Safe default

Properties:

- shipped to all users by default
- no host-command permission
- runtime shell inside the sandbox
- relies on portals and standard desktop integration

This profile is for:

- broad distro reach
- safe installation
- predictable Flathub review
- users who value simple install over full host parity

#### Profile B — Native host integration

Properties:

- user enables it intentionally after install
- grants `org.freedesktop.Flatpak` access
- switches terminal launching to host-shell mode
- may add further optional overrides for specific workflows

This profile is for:

- developers and sysadmins who want the terminal to behave like a native host terminal
- SSH, tmux, and shell-tooling parity with the host
- users willing to trade some sandbox strictness for workflow fidelity

### 3. Terminal execution model

`rttx` should support both shell launch paths internally.

Baseline launch path:

- `vte.spawn_async(..., [shell])`

Native-mode launch path:

- `vte.spawn_async(..., ["flatpak-spawn", "--host", "--watch-bus", ...])`

with explicit arguments that start the user's host shell as a login shell in the desired working
directory.

Recommended implementation shape:

- extract shell launch construction into a small dedicated module such as `src/terminal/launch.rs`
- detect Flatpak via `FLATPAK_ID` or `/.flatpak-info`
- keep three execution modes internally:
  - native host mode outside Flatpak
  - Flatpak host-shell mode
  - Flatpak sandbox-shell mode for debugging/fallback
- make Flatpak sandbox-shell mode the default when `FLATPAK_ID` is present and no host permission
  is available
- use Flatpak host-shell mode only when the required permission has been granted
- allow an escape hatch such as `RTTX_FLATPAK_SHELL_MODE=sandbox` for debugging

This should stay small. The goal is not a framework. The goal is one clear function that builds the
argv/env for terminal startup.

### 4. Host shell resolution

The shell command in Flatpak host mode should prefer the user's actual host login shell, not the
runtime default.

Design rules:

- prefer the host user's login shell
- fall back to `SHELL`
- fall back to `/bin/bash`
- use `--directory=<path>` when launching on the host instead of depending on sandbox cwd
- use `--watch-bus` so host processes are tied to the app lifecycle

This matters because the user expectation is not "a shell"; it is "my shell."

### 5. Permissions policy

Base manifest permissions should be explicit and minimal.

Recommended base finish args:

- `--share=ipc`
- `--share=network`
- `--socket=wayland`
- `--socket=fallback-x11`
- `--device=dri`

Permissions intentionally not included in the base manifest:

- no `--talk-name=org.freedesktop.Flatpak`
- no `--socket=ssh-auth`
- no `--socket=session-bus`
- no `--socket=system-bus`
- no blanket `--filesystem=host`
- no blanket `--filesystem=home`
- no blanket `--filesystem=xdg-config`
- no blanket `--filesystem=xdg-data`
- no blanket `--filesystem=xdg-cache`
- no `~/.themes` access

Rationale:

- GTK portals already cover URI opening and notifications
- theme integration should come from runtime/extensions, not host theme directory scraping
- the default manifest should stay shippable and conservative
- broad app filesystem access should be added only in response to a concrete, reproducible
  integration gap

Recommended native-mode override:

```bash
flatpak override --user io.github.IllyaYalovyy.rttx \
  --talk-name=org.freedesktop.Flatpak
```

Optional workflow-specific overrides:

```bash
flatpak override --user io.github.IllyaYalovyy.rttx \
  --socket=ssh-auth
```

The first override unlocks host-shell mode. The second is only needed if a user ends up relying on
sandbox-side components that still need direct SSH agent socket access.

### 6. Optional advanced overrides

Some advanced workflows should be documented as opt-in overrides rather than shipped by default.

Examples:

- `--socket=gpg-agent` for users whose SSH flow is mediated through GPG agent
- removable media access if users want terminal-triggered work rooted under `/run/media`
- broader host filesystem access for users who explicitly prefer it

This keeps the default install tighter while still giving power users a supported path.

### 7. Session recovery and external tools

The current session recovery design works in our favor.

Because bookmark and recovery actions already feed shell commands into the terminal:

- safe default mode remains functional, but uses sandbox tools/environment
- native mode switches those same flows to host tools/environment

Once the root shell is on the host:

- local folder recovery uses host paths
- SSH recovery uses host `ssh`
- tmux recovery uses host `tmux`
- combined SSH + tmux recovery uses host tooling and host config

That means the Flatpak-specific design does not need a second recovery system.

### 8. Desktop integration policy

#### Notifications

Keep the current GTK notification path. This should integrate through the desktop and portal stack
without custom work.

#### URI and path opening

Keep the current `gio::AppInfo::launch_default_for_uri()` path.

Expected outcome:

- HTTP(S) and mailto links go through the desktop defaults
- file URIs and detected file paths go through the system handler as allowed by GTK/portal

#### Themes, icons, and fonts

Policy:

- rely on GNOME runtime theming behavior first
- rely on Flatpak theme extensions and the Settings portal for host theme matching
- do not read host theme directories directly

User documentation should explicitly say:

- Adwaita is the safe fallback
- third-party GTK themes depend on the corresponding Flatpak extension being available
- if the host desktop lacks a functioning Settings portal backend, fallback visuals are expected

This is not a defect in `rttx`; it is part of the Flatpak integration contract.

### 9. First-run UX and user guidance

The app should make the profile distinction obvious without becoming noisy.

Recommended behavior in Flatpak builds:

- expose the current execution mode in About or Preferences:
  - `Sandbox shell`
  - `Host shell`
- if running in sandbox-shell mode, show a one-time non-modal hint that full host integration is
  available
- include a compact action such as "Show Flatpak setup" that opens the documentation
- do not nag repeatedly once the user dismisses the hint

The key principle is clarity:

- default install should work safely
- power users should be able to discover native mode quickly

### 10. When a host agent becomes necessary

The current design deliberately avoids shipping a helper daemon on day one.

However, a host agent becomes justified if we later need:

- reliable host PID / foreground process inspection
- shell semantic tracking that breaks under PID namespace boundaries
- stronger current-directory/process-title tracking than the shell escape sequence path provides
- container or remote-host awareness comparable to Ptyxis

For the current feature set, this is not a blocker.

The important distinction is:

- host shell launching is required now
- host process introspection is not

### 11. Manifest and repository layout

Recommended repo additions:

- `packaging/flatpak/io.github.IllyaYalovyy.rttx.yml`
- `packaging/flatpak/cargo-sources.json`
- `packaging/flatpak/README.md`

Recommended CI tasks:

1. regenerate cargo sources when `Cargo.lock` changes
2. build the Flatpak manifest in CI
3. run the app smoke test in Flatpak
4. export a test bundle for manual QA

The Flatpak packaging should live in `packaging/flatpak/`, not in ad hoc root-level files.

### 12. User-facing setup guidance

The Flatpak should ship with a short but serious support guide.

Topics to document:

- the default Flatpak runs a sandbox shell
- full native support is available as an opt-in mode
- how to enable host-shell mode with `flatpak override --user ... --talk-name=org.freedesktop.Flatpak`
- when `ssh-auth` and `gpg-agent` overrides are actually needed
- theme mismatches are usually a portal/theme-extension issue, not an `rttx` bug
- how to inspect and change permissions with `flatpak info --show-permissions` and
  `flatpak override`
- how to test native mode quickly, for example by checking `echo $SHELL`, `pwd`, and
  availability of host tools such as `tmux`

This documentation is part of the product. Without it, support cost will go up immediately.

---

## Feasibility

### Overall assessment

**Feasibility: high**

Not trivial, but very realistic.

### Why it is feasible

- the app already centralizes shell spawning in one place
- the recovery model already uses shell commands rather than deep host-specific plumbing
- GTK already aligns with portal-backed notifications and URI handling
- config and app identity already fit modern desktop packaging conventions
- current `rttx` does not yet depend on advanced host PID inspection that would force a host agent

### Main risks

#### Flathub permission review

This is the largest non-technical risk.

`--talk-name=org.freedesktop.Flatpak` is restricted and must be justified carefully. The argument
for `rttx` is strong:

- it is a terminal emulator
- its primary user expectation is to run host shells and host tools
- existing portal APIs do not replace that use case

I expect this to be defensible, but it is still a review surface.

#### Host-shell edge cases

There will be cross-distro QA work around:

- login shell resolution
- environment propagation
- SSH agent path behavior
- X11 vs Wayland launch behavior

These are real risks, but they are engineering risks, not blockers.

#### Some host-customization gaps will remain

Flatpak will never reproduce every host customization automatically. The design should aim for:

- correct shell/tooling behavior
- good GNOME/desktop integration
- clear documentation for optional overrides

not for magical parity with every hand-tuned host setup.

### Practical effort estimate

- Packaging + manifest work: moderate
- App code changes for dual sandbox/host-shell mode: moderate
- Documentation and QA: moderate to high
- Need for a host agent immediately: low

### What this means strategically

If the goal is "support Ubuntu 24.04 without backporting system VTE and reach more distros with one
channel", this Flatpak plan is more attractive than a `.deb` + custom VTE repo.

If the goal were "preserve a perfect sandbox", this plan would not fit.

But that is not this product's goal.

---

## Rollout Plan

### Phase 1 — Packaging skeleton

- add Flatpak manifest and cargo source generation
- build successfully against GNOME runtime
- verify app launches in sandbox-shell fallback mode

### Phase 2 — Safe default productization

- verify sandbox-shell mode is stable and clearly documented
- add first-run/setup UX for optional native mode
- verify links, notifications, themes, fonts, and persistence in the default profile

### Phase 3 — Native host mode

- implement Flatpak-aware shell launch builder
- make host-shell mode activate when permission is present
- verify local-folder, SSH, and tmux recovery on host

### Phase 4 — Native-feel polish

- validate notifications, URI opening, theme behavior, fonts, icons
- test on Fedora, Ubuntu, and one non-GNOME distro
- write user-facing setup and troubleshooting guide

### Phase 5 — Flathub submission hardening

- run Flathub linter locally
- keep the shipped manifest at the minimum proven-safe permission set
- treat native mode as documented user opt-in unless later review/support experience justifies
  baking more into the base package

### Phase 6 — Reassess helper needs

- only if real bugs show namespace/process-tracking limitations
- consider a small host helper then, not earlier

---

## Success Criteria

- `rttx` installs and launches on Ubuntu 24.04 without depending on host VTE upgrades
- default Flatpak install works safely on mainstream systems
- native mode can be enabled with documented, reproducible overrides
- in native mode, SSH bookmarks and tmux recovery use host tools/config without user hacks
- clickable links, notifications, and theme behavior are acceptable on mainstream desktops
- the base manifest remains understandable and justified
- Flathub review concerns are limited because the shipped manifest is conservative

---

## References

Primary references used for this RFC:

- Flatpak sandbox permissions:
  https://docs.flatpak.org/en/latest/sandbox-permissions.html
- Flatpak desktop integration:
  https://docs.flatpak.org/en/latest/desktop-integration.html
- Flatpak available runtimes:
  https://docs.flatpak.org/en/latest/available-runtimes.html
- Flatpak command reference (`flatpak-spawn`, `--host`, `--directory`, `--watch-bus`):
  https://docs.flatpak.org/en/latest/flatpak-command-reference.html
- XDG Desktop Portal documentation:
  https://flatpak.github.io/xdg-desktop-portal/
- Flathub linter and permission policy notes:
  https://docs.flathub.org/docs/for-app-authors/linter

Supplementary references:

- Flathub Ptyxis page, showing that a terminal Flatpak can exist with broad host-facing
  permissions and still ship successfully:
  https://flathub.org/en/apps/app.devsuite.Ptyxis
- Christian Hergert's "Prompt" article, useful for understanding where terminal Flatpaks hit real
  namespace/process-tracking limits:
  https://blogs.gnome.org/chergert/2023/12/14/prompt/

Local implementation references:

- [`Cargo.toml`](/path/to/rttx/Cargo.toml)
- [`src/terminal/widget.rs`](/path/to/rttx/src/terminal/widget.rs)
- [`src/window.rs`](/path/to/rttx/src/window.rs)
- [`src/config.rs`](/path/to/rttx/src/config.rs)
- [`designs/RFC-005-distribution-packaging.md`](/path/to/rttx/designs/RFC-005-distribution-packaging.md)
