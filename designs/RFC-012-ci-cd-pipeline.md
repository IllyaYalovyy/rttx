# RFC-012: CI/CD Pipeline

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Accepted                |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

rttx uses two GitHub Actions pipelines: a **quality gate** that runs on every push and pull
request, and a **release pipeline** that builds all distribution artifacts on a version tag.
Both pipelines are entirely free (GitHub Actions is free for public repositories; COPR and
Flathub are free services). No paid CI infrastructure is required.

---

## Goals

- **G1** — Every commit to `mainline` is automatically checked for formatting, lint errors, and
  test failures before it can be merged
- **G2** — A version tag produces all distribution artifacts (Flatpak, DEB, RPM) as GitHub
  Release attachments without manual packaging steps
- **G3** — GTK widget tests run in CI (not just unit tests); the Broadway headless backend is used
  so no display server is needed on the CI runner
- **G4** — The entire pipeline uses only free-tier services

## Non-Goals

- **NG1** — Flathub submission is not automated; the bundle is published to GitHub Releases and
  the Flathub PR is a separate manual step
- **NG2** — End-to-end AT-SPI2 UI tests are not in the initial CI pipeline; they are tracked as a
  future job (see Development Plan)
- **NG3** — No Windows or macOS builds; rttx is Linux/GNOME-only

---

## Background & Motivation

As of the date this RFC was accepted there are no `.github/workflows/` files in the repository.
The pre-commit hook (fmt + clippy) catches lint issues locally but has no equivalent in CI.
Pull requests and branch pushes receive no automated checking. The distribution plan (RFC-005)
specifies a GitHub Actions release workflow but does not design it.

This RFC fills the gap and provides the complete design and the actual workflow files.

---

## Cost analysis

| Service | Usage | Cost |
| --- | --- | --- |
| GitHub Actions | CI + release pipeline | Free (public repo, unlimited minutes) |
| GitHub Releases | Artifact hosting | Free |
| Fedora COPR | RPM builds and repository | Free |
| Flathub | Flatpak distribution | Free (requires review) |

---

## Design

### Pipeline overview

```
Push / PR to mainline
└── quality.yml
    ├── fmt          cargo fmt --check
    ├── clippy       cargo clippy -- -D warnings
    └── test         broadwayd + cargo test (GTK widget tests run)

Version tag v*
└── release.yml
    ├── needs: quality jobs
    ├── build-flatpak   flatpak-builder → bundle
    ├── build-deb       cargo-deb → .deb
    ├── build-rpm       cargo-generate-rpm → .rpm
    ├── github-release  upload all artifacts
    └── copr-submit     submit .src.rpm to COPR
```

---

### Quality pipeline (`quality.yml`)

**Trigger**: `push` to `mainline`; `pull_request` targeting `mainline`.

**Runner**: `ubuntu-latest` (Ubuntu 24.04 LTS).

**System dependencies**:

```
libgtk-4-dev libgtk-4-bin libadwaita-1-dev libvte-2.91-gtk4-dev
meson ninja-build
```

`libgtk-4-bin` provides `gtkbroadwayd`, the headless Broadway display server used by the test
suite. `meson` and `ninja-build` are needed for the Meson build configuration check.

**Rust toolchain**: `dtolnay/rust-toolchain@stable` with `components: rustfmt, clippy`.
Edition 2024 with let-chains requires Rust ≥ 1.88; stable is always ahead of that.

**Broadway setup**:
`gtkbroadwayd :0 &` starts the headless server on port 8080 before the test step. Tests are
then run with `GDK_BACKEND=broadway GTK_A11Y=none cargo test`. Without broadwayd, GTK
initialization fails gracefully and widget tests are skipped — but running broadwayd ensures
the full test suite executes.

**Cargo caching**: `actions/cache@v4` on `~/.cargo/registry`, `~/.cargo/git`, and `target/`
keyed on `Cargo.lock`. Cuts subsequent job times significantly.

**Jobs**:

| Job | Command | Blocks merge if fails |
| --- | --- | --- |
| `fmt` | `cargo fmt --check` | Yes |
| `clippy` | `cargo clippy -- -D warnings` | Yes |
| `test` | `GDK_BACKEND=broadway GTK_A11Y=none cargo test` | Yes |
| `manifest` | `python3 -m json.tool io.github.IllyaYalovyy.rttx.json > /dev/null` | Yes |

The manifest validation job is a cheap JSON parse of the Flatpak manifest. It does not require
`flatpak-builder` and catches the most common breakage (malformed JSON from manual edits).

---

### Release pipeline (`release.yml`)

**Trigger**: `push` of a tag matching `v[0-9]*.*`.

**Precondition**: `needs: [quality]` — release jobs only start after all quality jobs pass.

**Artifact jobs** (run in parallel after quality):

#### `build-flatpak`

Runner: `ubuntu-latest`.

Additional dependencies:
```
flatpak flatpak-builder elfutils
```

Runtimes installed at build time:
```
flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install -y flathub org.gnome.Platform//49 org.gnome.Sdk//49
flatpak install -y flathub org.freedesktop.Sdk.Extension.rust-stable//25.08
```

Build command: `packaging/flatpak/build-bundle.sh`

Output artifact: `rttx.flatpak`

Note: the Flatpak runtime download is large (~1–2 GB). Cache the `~/.local/share/flatpak`
directory with `actions/cache` to make subsequent release builds fast.

#### `build-deb`

Runner: `ubuntu-latest`.

Install: `cargo install cargo-deb` (or use a pre-built action).

Command: `cargo deb`

Output artifact: `target/debian/rttx_*.deb`

Requires `[package.metadata.deb]` in `Cargo.toml` (see Development Plan).

#### `build-rpm`

Runner: `ubuntu-latest` (builds RPM on Ubuntu using `cargo-generate-rpm`).

Install: `cargo install cargo-generate-rpm`

Command: `cargo build --release && cargo generate-rpm`

Output artifact: `target/generate-rpm/rttx-*.rpm`

Requires `[package.metadata.generate-rpm]` in `Cargo.toml` (see Development Plan).

#### `github-release`

Runner: `ubuntu-latest`.

Uses `softprops/action-gh-release@v2` to create a GitHub Release (or update the draft) and
upload all three artifacts. The release body is populated from the tag annotation message.

#### `copr-submit`

Runner: `ubuntu-latest`.

Needs: `build-rpm`

Uses `copr-cli` with a token stored in `secrets.COPR_API_TOKEN`:
```bash
copr-cli build --nowait illya/rttx target/generate-rpm/rttx-*.src.rpm
```

The COPR token is a repository secret set by the maintainer. It is the only secret the
pipeline requires. All other steps need no secrets beyond the default `GITHUB_TOKEN`.

---

### Free-tier constraints

GitHub Actions free tier for public repos provides unlimited minutes on `ubuntu-latest` runners.
The quality pipeline completes in approximately 5–10 minutes (dominated by dependency
compilation on the first run; subsequent runs use the cargo cache). The release pipeline adds
Flatpak runtime installation which is the slowest step (~15 minutes first time, ~5 minutes with
cache). All steps are within the free tier.

---

## AT-SPI2 UI tests (deferred)

The AT-SPI2 test suite (`tests/ui/`, `run_ui_tests.sh`) requires the following on the runner:

**Packages:**
```
weston python3-atspi python3-gi gir1.2-atspi-2.0 dbus-x11
```

**Runtime setup:**
```bash
export XDG_RUNTIME_DIR=/run/user/$(id -u)
weston --backend=headless --socket=rttx-test &
```

**Environment for the app under test:**
```
WAYLAND_DISPLAY=rttx-test
GDK_BACKEND=wayland
RTTX_DEV_MODE=1
RTTX_DISABLE_SHELL_SPAWN=1
XDG_CONFIG_HOME=<tmpdir>
NO_AT_BRIDGE=0
# Unset: GTK_A11Y (must not disable a11y), DISPLAY (prevent X11 fallback)
```

Running Weston on a GitHub Actions `ubuntu-latest` runner requires `XDG_RUNTIME_DIR` to exist
and appropriate permissions. This is doable but adds ~60 lines of setup compared to the quality
pipeline. Kept in a separate file to not slow down the main gate.

The AT-SPI2 suite is deferred to a separate follow-on job (`ui-tests.yml`) to keep the initial
quality pipeline simple and fast. It will run on a schedule (nightly) rather than on every push.

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — quality gate on every push/PR | `quality.yml`: fmt + clippy + test + manifest validation |
| G2 — automated release artifacts | `release.yml`: Flatpak + DEB + RPM + GitHub Release upload |
| G3 — widget tests run in CI | Broadway daemon started before `cargo test` |
| G4 — 100% free | GitHub Actions (public repo) + COPR (free) + Flathub (free); only secret is COPR token |

---

## Development Plan

- [ ] **W1** — Add `[package.metadata.deb]` to `Cargo.toml` for `cargo-deb`
- [ ] **W2** — Add `[package.metadata.generate-rpm]` to `Cargo.toml` for `cargo-generate-rpm`
- [ ] **W3** — Create `.github/workflows/quality.yml` — fmt, clippy, test, manifest validation
- [ ] **W4** — Create `.github/workflows/release.yml` — scaffold with build-flatpak, build-deb, build-rpm, github-release, copr-submit jobs
- [ ] **W5** — Set `COPR_API_TOKEN` as a repository secret
- [ ] **W6** — Validate Flatpak runtime caching on first release run; tune cache key
- [ ] **W7** — Add nightly `ui-tests.yml` job for AT-SPI2 suite once weston headless setup is worked out

---
