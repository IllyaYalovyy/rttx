# RFC-005: Distribution & Packaging Strategy

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Accepted                |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

rttx targets Linux distributions via multiple packaging formats, prioritized by GNOME user reach.
Fedora/COPR is the first distribution channel (already live). Flatpak/Flathub, DEB/PPA, and
AppImage follow. A CI/CD release pipeline automates artifact generation on each tagged release.

---

## Goals

- **G1** — Users on Fedora can install rttx with `dnf` from a native RPM repository
- **G2** — Users on Ubuntu/Debian can install rttx without building from source
- **G3** — Users on any Linux distro can run rttx without system-level installation (AppImage)
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
already live (`dnf copr enable illya/rttx`), providing a reference for all other formats.

---

## User Impact

| Audience | Impact |
| --- | --- |
| End users | Native package management on Fedora/Ubuntu; zero-install AppImage on any distro |
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
| RPM | `cargo-generate-rpm` + COPR | `dnf copr enable illya/rttx` | Live |
| DEB | `cargo-deb` | GitHub Releases / PPA | Pending |
| Flatpak | `flatpak-builder` | Flathub | Pending |
| AppImage | `linuxdeploy` + GTK plugin | GitHub Releases | Pending |

### COPR repository setup

The live repository is `https://copr.fedorainfracloud.org/coprs/illya/rttx/`.

#### Creating the project (one-time)

1. Sign in at `https://copr.fedorainfracloud.org` with a Fedora Account System (FAS) account
   (`https://accounts.fedoraproject.org`)
2. Click **New Project**, fill in name (`rttx`), homepage, and description
3. Under **Build options**, enable the target chroots:
   `fedora-rawhide-x86_64`, `fedora-41-x86_64`, `fedora-40-x86_64` and their `aarch64` variants
4. Click **Create**

#### API token for CI

1. Go to `https://copr.fedorainfracloud.org/api/` and copy the token configuration block
2. Add three repository secrets in GitHub settings:
   - `COPR_LOGIN` — the `login` value from the token block
   - `COPR_USERNAME` — the `username` value
   - `COPR_TOKEN` — the `token` value

The release pipeline reads these and calls `copr-cli build` automatically on each version tag.

#### Build trigger options

- **GitHub Actions (gated)** *(chosen)*: Tests pass → `copr-cli` submits RPM to COPR.
  Only tested code reaches users. Implemented in `.github/workflows/release.yml`.
- **Webhook (lazy)** *(alternative)*: GitHub push webhook → COPR build servers directly.
  No test gate. Simpler but less safe.

#### Adding a new Fedora release to the build matrix

1. Go to the project settings page on copr.fedorainfracloud.org
2. **Edit → Build options** → enable the new chroot (e.g., `fedora-42-x86_64`)
3. Optionally resubmit the latest release to build against the new chroot immediately

#### Manual build submission

Install `copr-cli` (`sudo dnf install copr-cli`), configure `~/.config/copr` with the token
block from the API page, then:

```bash
# Submit a local RPM
copr-cli build illya/rttx target/generate-rpm/rttx-<version>-1.x86_64.rpm

# Watch build status
copr-cli watch-build <build-id>
```

Build history is also visible at `https://copr.fedorainfracloud.org/coprs/illya/rttx/builds/`.

### Release pipeline (GitHub Actions)

On a version tag (`v*`):

1. Run `cargo test`
2. `flatpak-builder` → bundle
3. `cargo-deb` → `.deb`
4. `cargo-generate-rpm` → `.rpm` → submit to COPR via `copr-cli`
5. `linuxdeploy` → `.AppImage`
6. Upload all artifacts to the GitHub Release

### RPM spec generation

`rust2rpm` generates a Fedora-compliant `.spec` from `Cargo.toml`. The spec requires
`BuildRequires: libadwaita-devel gtk4-devel vte291-gtk4-devel`.

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — Fedora native install | COPR repository live at `dnf copr enable illya/rttx` |
| G2 — Ubuntu/Debian install | `cargo-deb` → DEB + PPA (pending) |
| G3 — Distro-agnostic AppImage | `linuxdeploy` with GTK plugin bundles all shared libs (pending) |
| G4 — Automated releases | GitHub Actions release workflow generates all formats on tag |

---

## Development Plan

- [x] COPR RPM repository live
- [ ] **DEB packaging** — Add `[package.metadata.deb]` to `Cargo.toml`; configure `cargo-deb` — tracked in #108
- [x] **Flatpak manifest** — `packaging/rttx/io.github.IllyaYalovyy.rttx.json`; offline Cargo dependencies via `rust-bundle` extension (implemented; see RFC-011)
- [ ] **AppImage** — `linuxdeploy` with GTK + Rust plugins
- [ ] **GitHub Actions release workflow** — `.github/workflows/release.yml`; all formats on version tag — designed in RFC-012

---

## Open Questions

- [ ] **Q1** — Flatpak sandbox and PTY access: verify that `org.freedesktop.Flatpak` portal or the `pty` device plug provides sufficient access for VTE to spawn shells in the Flatpak sandbox

---
