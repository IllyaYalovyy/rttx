# RFC-012: CI/CD Pipeline

| Field         | Value                   |
|---------------|-------------------------|
| Status        | Implemented             |
| Author(s)     | Illya Yalovyy           |
| Supersedes    | —                       |
| Superseded by | —                       |

---

## Summary

rttx uses two GitHub Actions pipelines: a **quality gate** that runs on every push and pull
request, and a **release pipeline** that builds all distribution artifacts on a version tag.
Both pipelines are entirely free (GitHub Actions is free for public repositories; COPR and
Flathub are free services). No paid CI infrastructure is required.

## Current implementation snapshot (2026-04)

- `mainline` is protected by required checks from `.github/workflows/quality.yml`:
  `Runtime behavior gate`, `Format`, `Clippy`, `Test`, `UI behavioral tests`, `Test coverage`,
  `Memory profiling gate`, and `Flatpak manifest`
- the quality workflow runs `Clippy` and `Test` inside Fedora containers and delegates the
  detailed commands to repo-local scripts:
  - `.github/scripts/run-clippy.sh`
  - `.github/scripts/run-quality-tests.sh`
  - `.github/scripts/run-coverage.sh`
  - `.github/scripts/run-memory-gate.sh`
  - `.github/scripts/check-version-consistency.sh`
  - `.github/scripts/ensure-workspace-layout.sh`
  - `.github/scripts/check_runtime_behavior_policy.py`
- the quality workflow is reusable via `workflow_call` so the release pipeline can invoke it
  as a precondition
- AT-SPI behavioral UI tests run on every push and PR as the `ui-test` job in `quality.yml`,
  using a Fedora container with Weston and D-Bus
- the release workflow exists in `.github/workflows/release.yml` and builds Flatpak, DEB, and RPM
  artifacts plus GitHub Release publication and COPR submission
- AppImage is not part of the current release workflow

---

## Goals

- **G1** — Every commit to `mainline` is automatically checked for formatting, lint errors, and
  test failures before it can be merged
- **G2** — A version tag produces all distribution artifacts (Flatpak, DEB, RPM) as GitHub
  Release attachments without manual packaging steps
- **G3** — GTK widget tests run in CI (not just unit tests); the Broadway headless backend is used
  so no display server is needed on the CI runner
- **G4** — The entire pipeline uses only free-tier services
- **G5** — Runtime-affecting changes require both pure-state and behavioral test evidence
- **G6** — Memory cleanup and resource leak regressions are caught before merge

## Non-Goals

- **NG1** — Flathub submission is not automated; the bundle is published to GitHub Releases and
  the Flathub PR is a separate manual step
- **NG2** — No Windows or macOS builds; rttx is Linux/GNOME-only

---

## Historical background

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
    ├── runtime-behavior-gate   policy check for runtime-affecting PRs
    ├── fmt                     cargo fmt --check + version consistency
    ├── clippy                  run-clippy.sh (Fedora container)
    ├── test                    run-quality-tests.sh (Fedora container)
    ├── ui-test                 AT-SPI2 behavioral tests (Fedora container + Weston)
    ├── coverage                cargo-llvm-cov (Fedora container)
    ├── memory-gate             memory cleanup + diagnostics tests (Fedora container)
    └── manifest                Flatpak manifest JSON validation

Version tag v*
└── release.yml
    ├── quality         reuses quality.yml via workflow_call
    ├── build-flatpak   flatpak-builder → bundle
    ├── build-deb       cargo-deb → .deb
    ├── build-rpm       Fedora container: build-srpm.sh → .src.rpm, rpmbuild --rebuild → .rpm
    ├── github-release  upload all artifacts
    └── copr-submit     submit SRPM to COPR
```

---

### Quality pipeline (`quality.yml`)

**Trigger**: `push` to `mainline`; `pull_request` targeting `mainline`; `workflow_call` (reused
by the release pipeline).

**Current implementation**:

- `Format` runs on `ubuntu-latest`
- `Clippy`, `Test`, `UI behavioral tests`, `Test coverage`, and `Memory profiling gate` run on
  `ubuntu-latest` with `fedora:latest` containers
- `Runtime behavior gate` runs on `ubuntu-latest` (Python only, no Fedora container needed)
- the manifest job validates `packaging/rttx/io.github.IllyaYalovyy.rttx.json`

**System dependencies** for the Fedora container jobs:

```
gtk4-devel libadwaita-devel vte291-gtk4-devel protobuf-compiler protobuf-devel
```

Additional dependencies for the `ui-test` job:

```
weston python3-gobject at-spi2-core at-spi2-atk dbus-daemon
```

The Broadway and targeted test orchestration now lives in `.github/scripts/run-quality-tests.sh`
rather than being spelled out inline in the workflow YAML.

**Jobs**:

| Job | Command | Blocks merge if fails |
| --- | --- | --- |
| `runtime-behavior-gate` | `python3 .github/scripts/check_runtime_behavior_policy.py` | Yes (PRs only) |
| `fmt` | `cargo fmt --check` + `bash .github/scripts/check-version-consistency.sh` | Yes |
| `clippy` | `bash .github/scripts/run-clippy.sh` | Yes |
| `test` | `bash .github/scripts/run-quality-tests.sh` | Yes |
| `ui-test` | `dbus-run-session -- ./run_ui_tests.sh` | Yes |
| `coverage` | `bash .github/scripts/run-coverage.sh` | Yes |
| `memory-gate` | `bash .github/scripts/run-memory-gate.sh` | Yes |
| `manifest` | `python3 -m json.tool packaging/rttx/io.github.IllyaYalovyy.rttx.json > /dev/null` | Yes |

The manifest validation job is a cheap JSON parse of the Flatpak manifest. It does not require
`flatpak-builder` and catches the most common breakage (malformed JSON from manual edits).

#### Runtime behavior gate

The runtime behavior gate (`check_runtime_behavior_policy.py`) enforces that pull requests
touching runtime-affecting source paths include both:

1. At least one new pure-state regression test (unit-style, in a source test host)
2. At least one new integration, GTK boundary/widget, or AT-SPI behavioral regression test

The gate runs its own unit tests (`python3 -m unittest discover`) on every invocation and only
evaluates the policy diff on pull request events.

#### Version consistency check

The `check-version-consistency.sh` script verifies that the version in the workspace
`Cargo.toml` matches the version in `meson.build`, preventing accidental version drift between
the Cargo and Meson build systems.

#### Test coverage

The `coverage` job runs `cargo-llvm-cov` against `rttx-server` and `rttx-proto` (non-GTK
packages). The GTK client is excluded because its tests require a display server and GTK global
state isolation that conflict with coverage instrumentation. The job uploads an LCOV report as a
workflow artifact.

#### Memory profiling gate

The `memory-gate` job runs the daemon's memory cleanup, diagnostics, lifecycle leak, and bounded
channel integration tests. It verifies that no sessions or panes leak after lifecycle scenarios
complete.

#### AT-SPI2 UI tests

The `ui-test` job runs the full AT-SPI2 behavioral test suite on every push and PR. It uses a
Fedora container with Weston (headless Wayland compositor), D-Bus, and AT-SPI2 bindings. The
tests launch a private `RTTX_DEV_MODE=1` instance and observe the live widget tree through the
accessibility API.

---

### CI scripts

| Script | Purpose |
| --- | --- |
| `run-clippy.sh` | Runs Clippy on all three workspace packages with correct VTE feature flags |
| `run-quality-tests.sh` | Starts Broadway, runs all test suites (library, binary, integration, GTK ignored, doc, protocol, daemon) |
| `run-coverage.sh` | Generates LCOV coverage for `rttx-server` and `rttx-proto` |
| `run-memory-gate.sh` | Runs memory cleanup, diagnostics, lifecycle leak, and bounded channel tests |
| `check-version-consistency.sh` | Verifies Cargo.toml and meson.build versions match |
| `ensure-workspace-layout.sh` | Validates that all required workspace manifests exist |
| `check_runtime_behavior_policy.py` | Enforces dual-layer test coverage for runtime-affecting PRs |

All scripts use `ensure-workspace-layout.sh` as a prerequisite check where applicable.

---

### Release pipeline (`release.yml`)

**Trigger**: `push` of a tag matching `v[0-9]*.*`.

**Precondition**: `needs: [quality]` — release jobs only start after all quality jobs pass. The
quality gate is invoked via `workflow_call` reuse of `quality.yml`.

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

Build command: `packaging/rttx/flatpak/build-bundle.sh`

Output artifact: `rttx.flatpak`

Note: the Flatpak runtime download is large (~1–2 GB). Cache the `~/.local/share/flatpak`
directory with `actions/cache` to make subsequent release builds fast.

#### `build-deb`

Runner: `ubuntu-latest`.

Install: `cargo install cargo-deb` (or use a pre-built action).

Command: `cargo deb`

Output artifact: `target/debian/rttx_*.deb`

Requires `[package.metadata.deb]` in `clients/rttx/Cargo.toml` (see Development Plan).

#### `build-rpm`

Runner: `ubuntu-latest` with `container: registry.fedoraproject.org/fedora:latest` — the RPM must be
built on Fedora so it links against Fedora's GTK/VTE/glibc and uses `cargo-rpm-macros`.

Install: `dnf install rpm-build rpmdevtools cargo rust dnf5-plugins` + `dnf builddep packaging/rttx/rttx.spec`

Command: `packaging/rttx/rpm/build-srpm.sh --outdir dist/rpm` then
`rpmbuild --rebuild dist/rpm/*.src.rpm --without check`

Output artifacts: `dist/rpm/rttx-*.src.rpm` (what COPR consumes) and `dist/rpm/rttx-*.x86_64.rpm`
(GitHub-release convenience asset). See `packaging/rttx/rpm/README.md`.

#### `github-release`

Runner: `ubuntu-latest`.

Uses `softprops/action-gh-release@v2` to create a GitHub Release (or update the draft) and
upload all three artifacts. Release notes are auto-generated by GitHub.

#### `copr-submit`

Runner: `ubuntu-latest`.

Needs: `build-rpm`

Uses `copr-cli` with credentials stored in three repository secrets:
- `secrets.COPR_LOGIN`
- `secrets.COPR_USERNAME`
- `secrets.COPR_TOKEN`

These are written to `~/.config/copr` at runtime and used by `copr-cli build <project> <srpm>`.
COPR rebuilds the SRPM for every chroot enabled in the project; the optional repository variable
`COPR_PROJECT` overrides the default `etf2026/rttx`.

---

### Free-tier constraints

GitHub Actions free tier for public repos provides unlimited minutes on `ubuntu-latest` runners.
The quality pipeline completes in approximately 5–10 minutes (dominated by dependency
compilation on the first run; subsequent runs use the cargo cache). The release pipeline adds
Flatpak runtime installation which is the slowest step (~15 minutes first time, ~5 minutes with
cache). All steps are within the free tier.

---

## Goals Alignment

| Goal | How addressed |
| --- | --- |
| G1 — quality gate on every push/PR | `quality.yml`: runtime-behavior-gate + fmt + clippy + test + ui-test + coverage + memory-gate + manifest |
| G2 — automated release artifacts | `release.yml`: Flatpak + DEB + RPM + GitHub Release upload |
| G3 — widget tests run in CI | Broadway daemon started before `cargo test`; AT-SPI2 UI tests via Weston |
| G4 — 100% free | GitHub Actions (public repo) + COPR (free) + Flathub (free); secrets are COPR credentials only |
| G5 — runtime behavior coverage | `runtime-behavior-gate` enforces dual-layer test evidence on PRs |
| G6 — memory leak prevention | `memory-gate` runs cleanup and diagnostics tests on every push/PR |

---

## Development Plan

- [x] **W1** — Add `[package.metadata.deb]` to `clients/rttx/Cargo.toml` for `cargo-deb` — PR #873
- [x] **W2** — ~~Add `[package.metadata.generate-rpm]`~~ Replaced (2026-08) by the SRPM flow: `packaging/rttx/rttx.spec` + `build-srpm.sh` — PR #873
- [x] **W3** — Create `.github/workflows/quality.yml` — fmt, clippy, test, manifest validation
- [x] **W4** — Create `.github/workflows/release.yml` — scaffold with build-flatpak, build-deb, build-rpm, github-release, copr-submit jobs
- [x] **W5** — Set COPR credentials as repository secrets
- [x] **W6** — Validate Flatpak runtime caching on first release run; tune cache key
- [x] **W7** — AT-SPI behavioral UI tests running in CI on every PR via Weston headless compositor
- [x] **W8** — Runtime behavior gate enforcing dual-layer test coverage for runtime-affecting PRs
- [x] **W9** — Test coverage reporting via `cargo-llvm-cov`
- [x] **W10** — Memory profiling gate for daemon resource leak detection
- [x] **W11** — Version consistency check between Cargo.toml and meson.build

---
