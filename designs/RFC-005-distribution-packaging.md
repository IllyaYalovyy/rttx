# RFC-005: Distribution & Packaging Strategy

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented             |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

rttx targets Linux distributions via multiple packaging formats, prioritized by GNOME user reach.
Fedora/COPR is the first distribution channel. The repository contains a Flatpak
manifest, an RPM spec, and a GitHub Actions release workflow that builds Flatpak, DEB, and RPM
artifacts on each version tag. All active packaging goals are met. AppImage is deferred.

## Current implementation snapshot (2026-05)

- Packaging assets live under `packaging/rttx/` in the monorepo.
- The release workflow exists in `.github/workflows/release.yml` and runs on version tags (`v*`).
- COPR project `etf2026/rttx` exists (created 2026-08-29; chroots F43/F44/F45/rawhide × x86_64/aarch64,
  no network during builds): `dnf copr enable etf2026/rttx`.
- The Flatpak manifest (`packaging/rttx/io.github.IllyaYalovyy.rttx.json`) bundles VTE 0.78.7
  and uses the GNOME 49 runtime. Offline Cargo dependencies are listed in
  `packaging/rttx/flatpak/cargo-sources.json`.
- The RPM spec (`packaging/rttx/rttx.spec`) is hand-written, uses Fedora's `cargo-rpm-macros`
  with a vendored-crates tarball (offline build), and ships both the GUI and daemon binaries.
  `packaging/rttx/rpm/build-srpm.sh` produces the SRPM; `packaging/rttx/rpm/README.md` is the
  build/distribution guide.
- `[package.metadata.deb]` is in `clients/rttx/Cargo.toml`; the DEB includes both binaries.
- AppImage is not part of the live workflow (deferred indefinitely).

---

## Goals

- **G1** — Users on Fedora can install rttx with `dnf` from a native RPM repository
- **G2** — Users on Ubuntu/Debian can install rttx without building from source
- **G3** — Users on any Linux distro can install via Flatpak or build from source without distro
  packaging lag
- **G4** — Release artifacts are generated automatically; no manual packaging steps per release

## Non-Goals

- **NG1** — No Windows or macOS packages; rttx is Linux/GNOME-only
- **NG2** — Not targeting inclusion in official Fedora/Debian repositories in the near term
- **NG3** — Snap is out of scope; it adds complexity without meaningful additional reach vs Flatpak

---

## Background & Motivation

rttx has a Meson build system that handles binary, icons, `.desktop` file, and AppStream metainfo
installation. This makes packaging straightforward — Meson's `DESTDIR` install works directly
with Flatpak builder, `cargo-deb`, and `cargo-generate-rpm`. The Fedora COPR repository is
already live (`dnf copr enable etf2026/rttx`), providing a reference for all other formats.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | Native package management on Fedora; Flatpak bundle or source build elsewhere; DEB remains follow-up work |
| Contributors | Release process is documented and automated; no manual fiddling per release |
| Packagers | Each format has a dedicated config file in `packaging/`; maintainable independently |

---

## Considered Options

### Option A — Flatpak only *(reconstructed)*

**Pros**: One format, one build configuration; Flathub provides a single distribution channel.
**Cons**: Flatpak sandboxing constrains PTY access, filesystem visibility, and SSH agent integration
in ways that require portal workarounds. Native packages (RPM/DEB) avoid these issues entirely and
are the preference for power users who run rttx in production terminal workflows.

### Option B — Native packages only (RPM + DEB) *(reconstructed)*

**Pros**: Full system access; no sandbox; integrates with system SSH agents and GPG correctly.
**Cons**: Distro-specific maintenance; no single install path for non-Fedora/Debian users.

### Option C — All formats, prioritized by effort

Implement formats in priority order: COPR (lowest effort, highest GNOME/Fedora alignment) → DEB
→ Flatpak → AppImage. Each format is independently maintainable. CI automates all of them.

**Pros**: Maximum reach; native experience where possible, sandboxed where convenient.
**Cons**: More configuration to maintain.

---

## Decision

Chosen option: C

COPR was the natural first step (Fedora is the primary development platform). The other formats
follow from the Meson build system with minimal additional configuration. CI automation ensures
that adding a format does not add per-release manual work.

---

## Design

### Format priority

| Format | Tool | Channel | Status |
| --- | --- | --- | --- |
| RPM | `rttx.spec` + `cargo-rpm-macros` → SRPM → mock/COPR | `dnf copr enable etf2026/rttx` | Spec + pipeline done; COPR project created 2026-08-29, first build pending |
| DEB | `cargo-deb` | GitHub Releases / PPA | Pipeline builds DEB; `Cargo.toml` metadata pending (#108) |
| Flatpak | `flatpak-builder` | GitHub Releases / Flathub follow-up | Implemented; bundle built on each release tag |
| Source install | Meson + Cargo | Manual / docs | Live |
| AppImage | `linuxdeploy` + GTK plugin | GitHub Releases | Deferred |

### COPR repository setup

The live repository is `https://copr.fedorainfracloud.org/coprs/etf2026/rttx/`.

#### Creating the project (one-time)

1. Sign in at `https://copr.fedorainfracloud.org` with a Fedora Account System (FAS) account
   (`https://accounts.fedoraproject.org`)
2. Click **New Project**, fill in name (`rttx`), homepage, and description
3. Under **Build options**, enable the target chroots:
   `fedora-rawhide-x86_64`, `fedora-44-x86_64`, `fedora-43-x86_64` and their `aarch64` variants
   (leave "internet access during builds" off — the SRPM carries vendored crates)
4. Click **Create**

#### API token for CI

1. Go to `https://copr.fedorainfracloud.org/api/` and copy the token configuration block
2. Add three repository secrets in GitHub settings:
   - `COPR_LOGIN` — the `login` value from the token block
   - `COPR_USERNAME` — the `username` value
   - `COPR_TOKEN` — the `token` value

The release pipeline reads these and calls `copr-cli build` with the SRPM automatically on each
version tag. An optional repository variable `COPR_PROJECT` overrides the default `etf2026/rttx`.

#### Build trigger options

- **GitHub Actions (gated)** *(chosen)*: Tests pass → `copr-cli` submits the SRPM to COPR.
  Only tested code reaches users. Implemented in `.github/workflows/release.yml`.
- **Webhook / SCM (lazy)** *(fallback, also supported)*: COPR clones the repo and runs
  `.copr/Makefile` (`make srpm`) itself. No test gate; useful when CI is unavailable.

#### Adding a new Fedora release to the build matrix

1. Go to the project settings page on copr.fedorainfracloud.org
2. **Edit → Build options** → enable the new chroot (e.g., `fedora-45-x86_64`)
3. Optionally resubmit the latest release to build against the new chroot immediately

#### Manual build submission

Install `copr-cli` (`sudo dnf install copr-cli`), configure `~/.config/copr` with the token
block from the API page, then:

```bash
# Build the SRPM, then submit it (COPR rebuilds it once per enabled chroot)
./packaging/rttx/rpm/build-srpm.sh
copr-cli build etf2026/rttx target/rpmbuild/rttx-<version>-1.fc43.src.rpm

# Watch build status
copr-cli watch-build <build-id>
```

Build history is also visible at `https://copr.fedorainfracloud.org/coprs/etf2026/rttx/builds/`.

### Release pipeline (GitHub Actions)

The workflow (`.github/workflows/release.yml`) triggers on version tags (`v*`). It runs a quality
gate first, then builds all artifacts in parallel:

1. **Quality gate** — reuses `.github/workflows/quality.yml` on the exact tag commit
2. **Build Flatpak** — `flatpak-builder` → `build-bundle` → `.flatpak` artifact
3. **Build DEB** — `cargo build --release` → `cargo deb --no-build` → `.deb` artifact
4. **Build RPM** — in a Fedora container: `build-srpm.sh` → `.src.rpm`, then `rpmbuild --rebuild`
   → `.x86_64.rpm` artifact
5. **Publish GitHub Release** — collects all artifacts and creates a release with auto-generated
   notes
6. **Submit to COPR** — downloads the SRPM artifact and submits via `copr-cli build --nowait`

Steps 2–4 run in parallel after the quality gate passes. Steps 5–6 run after all builds complete.

### RPM spec

The hand-written spec lives at `packaging/rttx/rttx.spec`. It declares `BuildRequires` for
`meson`, `cargo`, `gtk4-devel`, `libadwaita-devel`, and `vte291-gtk4-devel`. The spec currently
packages only the GUI client (`rttx`); the daemon (`rttx-server`) is not included.

### Flatpak manifest

The manifest at `packaging/rttx/io.github.IllyaYalovyy.rttx.json` uses the GNOME 49 runtime and
SDK with the `org.freedesktop.Sdk.Extension.rust-stable` extension. It bundles VTE 0.78.7 as a
source module because the GNOME SDK does not include `vte-2.91-gtk4`.

The conservative default `finish-args` grant IPC, network, Wayland/X11 sockets, and DRI access.
Host shell access, home filesystem, SSH agent, and GPG agent are opt-in overrides documented in
the README.

Offline Cargo dependencies are listed in `packaging/rttx/flatpak/cargo-sources.json`. This file
must be regenerated whenever `Cargo.lock` changes:

```bash
flatpak-cargo-generator Cargo.lock -o packaging/rttx/flatpak/cargo-sources.json
```

The `packaging/rttx/flatpak/build-bundle.sh` script builds a standalone `.flatpak` bundle for
local testing and CI.

---

## Implementation Snapshot

### Packaging asset locations

| Asset | Path |
|-------|------|
| Flatpak manifest | `packaging/rttx/io.github.IllyaYalovyy.rttx.json` |
| Flatpak offline Cargo deps | `packaging/rttx/flatpak/cargo-sources.json` |
| Flatpak bundle build script | `packaging/rttx/flatpak/build-bundle.sh` |
| RPM spec | `packaging/rttx/rttx.spec` |
| RPM SRPM build script + guide | `packaging/rttx/rpm/build-srpm.sh`, `packaging/rttx/rpm/README.md` |
| COPR SCM build entry point | `.copr/Makefile` |
| Release workflow | `.github/workflows/release.yml` |
| Quality workflow (reused) | `.github/workflows/quality.yml` |
| User-local install script | `install-user-local.sh` |
| Meson build definition | `meson.build` |

### Deviations from original design

**RPM spec is hand-written.** The RFC originally mentioned `rust2rpm` for generating a
Fedora-compliant spec. The spec at `packaging/rttx/rttx.spec` is hand-written but follows the
same conventions (`cargo-rpm-macros`, vendored crates via `%cargo_prep -v vendor`,
`%cargo_vendor_manifest`/`%cargo_license` shipped as `%license`). It packages both `rttx` and
`rttx-server` (plus the man page) and lists `protobuf-compiler` in `BuildRequires`.

**`cargo-generate-rpm` was dropped (2026-08).** The original pipeline built a binary RPM on an
Ubuntu runner with `cargo-generate-rpm` and tried to submit *that* to COPR. That RPM linked
against Ubuntu's libraries and `copr-cli build` only accepts SRPMs, so the COPR channel never
actually worked. The pipeline now builds an SRPM in a Fedora container and COPR rebuilds it per
chroot. `[package.metadata.generate-rpm]` was removed from `Cargo.toml`.

**DEB metadata not yet in Cargo.toml.** The release workflow runs `cargo deb --no-build` but
`clients/rttx/Cargo.toml` does not contain a `[package.metadata.deb]` section. This is tracked
in #108.

**Flatpak sandbox and host access.** The default manifest is conservative (no host shell, no
filesystem, no SSH agent). Users opt in to host integration via `flatpak override` commands
documented in the README. The `org.freedesktop.Flatpak` portal provides host shell access when
enabled. This resolves the original open question Q1 about PTY access in the sandbox.

### Related issues

- #86 — Packaging: Publish an RPM repository (open)
- #87 — Packaging: Publish a DEB package (open)
- #107 — CI: Add release pipeline workflow (open)
- #108 — CI: Add Cargo.toml metadata for cargo-deb and cargo-generate-rpm (open)
- #103 — Maintenance: Add CI pipeline with Flatpak manifest validation (closed)

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — Fedora native install | SRPM + COPR pipeline in place; COPR project `etf2026/rttx` created 2026-08-29 |
| G2 — Ubuntu/Debian install | `cargo-deb` → DEB built in release pipeline; `Cargo.toml` metadata pending (#108) |
| G3 — Distro-agnostic install path | Flatpak bundle built on each release tag; documented source install via Meson + Cargo |
| G4 — Automated releases | GitHub Actions release workflow generates all formats on version tag |

---

## Development Plan

- [ ] COPR RPM repository live — project `etf2026/rttx` created; first build + GitHub `COPR_*` secrets pending (see `packaging/rttx/rpm/README.md` §5)
- [x] **DEB packaging** — `[package.metadata.deb]` in `Cargo.toml`; `cargo-deb` configured; DEB includes both GUI and daemon — PR #873, #875
- [x] **Flatpak manifest** — `packaging/rttx/io.github.IllyaYalovyy.rttx.json`; offline Cargo dependencies via `cargo-sources.json`; bundle build script in `packaging/rttx/flatpak/` (see also RFC-011)
- [ ] **AppImage** — deferred indefinitely; no active implementation work
- [x] **GitHub Actions release workflow** — `.github/workflows/release.yml` with quality gate, parallel builds, GitHub Release publishing, and COPR submission
- [x] **RPM spec** — `packaging/rttx/rttx.spec` (hand-written, Fedora cargo macros, vendored crates); SRPM script `packaging/rttx/rpm/build-srpm.sh`; `.copr/Makefile` — PR #873, 2026-08 rework
- [x] **User-local install script** — `install-user-local.sh` builds and installs both client and daemon

---

## Open Questions

- [x] **Q1** — Flatpak sandbox and PTY access: resolved. The `org.freedesktop.Flatpak` portal
  provides host shell access when the user enables it via `flatpak override`. The default manifest
  is conservative; host integration is opt-in. See the README for the override commands.

---

## References

- [RFC-011: Flatpak-First Distribution with Native Host Integration](./RFC-011-flatpak-native-host-integration.md)
- [RFC-012: CI/CD Pipeline](./RFC-012-ci-cd-pipeline.md)
- [RFC-013: Persistent Host Sessions](./RFC-013-persistent-host-sessions.md)
