# Building and distributing the rttx RPM

This directory holds the Fedora packaging workflow. The spec itself lives one
level up at [`../rttx.spec`](../rttx.spec). This document is the practical guide:
how an RPM gets from this repository onto a Fedora user's machine, how to build
it for several Fedora releases at once, and what to do at release time.

## 1. How RPM distribution works (the 5-minute version)

| Term | What it is | Where it comes from here |
| --- | --- | --- |
| **spec** | Recipe: metadata, `BuildRequires`, how to build, what files to ship | `packaging/rttx/rttx.spec` |
| **SRPM** (`.src.rpm`) | Spec + source tarball(s) in one file. Architecture- and release-independent | `build-srpm.sh` |
| **RPM** (`.x86_64.rpm`) | Binary package built *from an SRPM* on a specific Fedora release/arch. Its name carries the **dist tag** (`.fc43`, `.fc44`) | `mock`, COPR, or `rpmbuild --rebuild` |
| **chroot** | A clean, minimal Fedora N install that `mock`/COPR build inside | `fedora-43-x86_64`, `fedora-rawhide-aarch64`, … |
| **repository** | Directory of RPMs + metadata (`repodata/`) that `dnf` can install from | COPR hosts one per project per chroot |

The one idea to hold on to: **you never ship a binary you built on your own
machine.** You ship one SRPM; the *same* SRPM is rebuilt once per (Fedora
release × architecture) in a clean chroot, so each RPM links against that
release's exact GTK/VTE/glibc. That is what makes "build for several versions of
Fedora" cheap — it's the same input, N builders.

Two consequences shape the spec:

- **Builds are offline.** mock and COPR disable networking during `rpmbuild`, so
  `cargo` cannot download crates. `build-srpm.sh` runs `cargo vendor` and packs
  the result as `Source1`; the spec's `%cargo_prep -v vendor` points cargo at it.
- **Fedora's Rust macros do the heavy lifting.** `%cargo_build` builds with the
  distro's hardening/debuginfo flags into `target/rpm/`; `%cargo_vendor_manifest`
  and `%cargo_license` record what got bundled (shipped as `%license`).

## 2. One-time setup on your Fedora machine

```bash
sudo dnf install rpm-build rpmdevtools rpmlint mock copr-cli cargo-rpm-macros
sudo usermod -aG mock "$USER"   # then log out/in (or `newgrp mock`)
```

`mock` is the tool that gives you "several Fedora versions" locally: it creates
a throw-away chroot for any release from `/etc/mock/*.cfg`, installs the spec's
`BuildRequires` into it, and runs `rpmbuild` inside. It caches the chroots, so
the second build for a release is fast.

## 3. Build the SRPM

```bash
./packaging/rttx/rpm/build-srpm.sh
# -> target/rpmbuild/rttx-1.0.0-1.fc43.src.rpm   (on a v1.0.0 checkout)
# -> target/rpmbuild/rttx-1.0.0-1.20260826git394dc1d.fc43.src.rpm  (any other commit)
```

What it does, in order:

1. Refuses to run if `Version:` in the spec differs from the workspace version
   in `Cargo.toml` — the two are bumped together at release time (see §6).
2. `git archive HEAD` → `rttx-<ver>.tar.gz` (same layout as GitHub's
   `archive/v<ver>/` tarball, which `Source0` points at). **Uncommitted changes
   are not included.**
3. `cargo vendor --locked` → `rttx-<ver>-vendor.tar.xz` (~18 MB).
4. `rpmbuild -bs` → the SRPM. If `HEAD` isn't the `v<ver>` tag the release
   gets a `.YYYYMMDDgitHASH` snapshot suffix so test builds sort above the
   real release and are obviously not it. The suffix and the commit hash (shown
   by `rttx --version`) are written as `%global` lines into the spec copy inside
   the SRPM — `rpmbuild --define` values would not survive a mock/COPR rebuild.

The dist tag in the SRPM's *filename* (`.fc43`) is just where it was created;
it is irrelevant — the SRPM builds for any release.

## 4. Build it for several Fedora versions (locally, with mock)

```bash
# One chroot:
mock -r fedora-43-x86_64 --rebuild target/rpmbuild/rttx-*.src.rpm

# Or let the script loop over several:
./packaging/rttx/rpm/build-srpm.sh \
    --mock fedora-43-x86_64,fedora-44-x86_64,fedora-rawhide-x86_64
# results: target/rpmbuild/<chroot>/rttx-1.0.0-1.fc43.x86_64.rpm, build.log, root.log …
```

`ls /etc/mock/ | grep fedora` shows every chroot mock knows about, including
`aarch64` ones (those run under QEMU user emulation — slow, but they work).
Add `--without-check` while iterating on the spec to skip the daemon test suite.

Inspect the result before shipping anything:

```bash
rpm -qpl   target/rpmbuild/fedora-43-x86_64/rttx-*.x86_64.rpm   # file list
rpm -qpR   target/rpmbuild/fedora-43-x86_64/rttx-*.x86_64.rpm   # auto-generated Requires
rpmlint    target/rpmbuild/fedora-43-x86_64/rttx-*.rpm packaging/rttx/rttx.spec
sudo dnf install target/rpmbuild/fedora-43-x86_64/rttx-*.x86_64.rpm   # try it
```

If the build fails, `target/rpmbuild/<chroot>/build.log` is the rpmbuild
output and `root.log` is the chroot setup (missing `BuildRequires` show up
there). `mock -r <chroot> --shell` drops you into the chroot to poke around.

## 5. Distribute it: Fedora COPR

COPR (<https://copr.fedorainfracloud.org>) is Fedora's free build service +
package repository for third-party projects. You upload an SRPM; it runs the
mock builds for every chroot you enabled and publishes the resulting repos.
Users install with two commands and get updates through normal `dnf upgrade`.

### 5.1 Create the project (once)

1. Sign in with a Fedora Account (FAS): <https://accounts.fedoraproject.org>.
2. **New Project** → name `rttx`, homepage `https://github.com/IllyaYalovyy/rttx`,
   description (reuse `%description` from the spec).
3. **Build options → Chroots**: enable `fedora-43-x86_64`, `fedora-44-x86_64`,
   `fedora-rawhide-x86_64` and the `aarch64` twins. Rawhide is worth keeping on:
   it's the early warning that a new GTK/VTE/Rust breaks the build.
4. Leave *"Enable internet access during builds"* **off** — the vendored
   tarball makes it unnecessary, and offline builds prove the SRPM is complete.
5. Create. The project URL is `https://copr.fedorainfracloud.org/coprs/<fas-user>/rttx/`.

> **Note:** the COPR owner is the FAS username (`etf2026`), not the GitHub one,
> so the project is `etf2026/rttx`. If it ever moves, set the GitHub repository
> variable `COPR_PROJECT` (see §5.3) and update the `dnf copr enable` line in
> `README.md`.

### 5.2 Submit a build by hand

```bash
copr-cli --help >/dev/null || sudo dnf install copr-cli
# Paste the token block from https://copr.fedorainfracloud.org/api/ into ~/.config/copr

copr-cli build etf2026/rttx target/rpmbuild/rttx-1.0.0-1.fc43.src.rpm
#  -> one build ID, one sub-build per enabled chroot
copr-cli watch-build <id>
# limit to specific chroots while testing:
copr-cli build -r fedora-rawhide-x86_64 etf2026/rttx target/rpmbuild/rttx-*.src.rpm
```

When it turns green, users do:

```bash
sudo dnf copr enable etf2026/rttx
sudo dnf install rttx
```

### 5.3 Automatic submission from CI

`.github/workflows/release.yml` runs on every `v*` tag: after the quality gate
it builds the SRPM inside a Fedora container (`build-rpm` job), attaches the
SRPM and an `x86_64` RPM to the GitHub Release, and the `copr-submit` job runs
`copr-cli build` with the SRPM. It needs three repository **secrets** from the
COPR API page — `COPR_LOGIN`, `COPR_USERNAME`, `COPR_TOKEN` — and optionally a
repository **variable** `COPR_PROJECT` (default `etf2026/rttx`). Without the
secrets the job logs a notice and skips.

**Which Fedora does the GitHub Release RPM target?** The `build-rpm` job pins
its container to `registry.fedoraproject.org/fedora:43` — the oldest supported
Fedora — so the attached `rttx-X.Y.Z-1.fc43.x86_64.rpm` is a deliberate choice
and does not change when a new Fedora ships. (It used to be `fedora:latest`,
which drifted to F44 and silently changed the dist tag of the v1.0.1 asset.)
A `.fc43` RPM installs on newer releases too — it links against the oldest
glibc/GTK/VTE of the supported set — but it is only a convenience asset:

> **COPR is the supported way to install binary RPMs.** It rebuilds the same
> SRPM in a clean chroot for every enabled release and architecture (F43/F44/
> rawhide, x86_64/aarch64) and delivers updates through `dnf upgrade`:
> `sudo dnf copr enable etf2026/rttx && sudo dnf install rttx`.

Use the GitHub Release RPM only when COPR is not an option; if you need an
architecture or release the asset does not cover, take the `.src.rpm` from the
same release and `mock -r fedora-<N>-<arch> --rebuild` it (§4).

### 5.4 Alternative trigger: let COPR build from git

`.copr/Makefile` lets COPR produce the SRPM itself. Set the project's default
source to **SCM** → `https://github.com/IllyaYalovyy/rttx`, *Type* `make srpm`,
and either press **Rebuild** in the web UI or add the COPR webhook to the GitHub
repository (**Settings → Integrations** on COPR gives the URL). This needs no
local tooling and no CI secrets; it's a good fallback if CI is down. The
trade-off is that nothing gates the build on tests.

### 5.5 When a new Fedora comes out

1. COPR project → **Settings → Chroots** → enable `fedora-45-*`.
2. Resubmit the latest SRPM (or press **Rebuild** on the latest build) so the
   new repo isn't empty.
3. Add the chroot to `--mock` in your local test loop.
4. When the *oldest* supported release goes EOL, drop its chroot and bump the
   `container:` pin of the `build-rpm` job in `.github/workflows/release.yml`
   to the new oldest one (§5.3). Leaving it on an EOL Fedora means the release
   RPM is built in a container that no longer gets updates.

### 5.6 What about hosting our own repository instead?

Possible (`createrepo_c` + `gpg --detach-sign` + upload the tree to GitHub
Pages, users add a `.repo` file), but then *you* run mock for every release ×
arch, sign every RPM, and keep the signing key safe. COPR does all of that,
users trust its key via `dnf copr enable`, and it is what Fedora users expect
for third-party software. Own hosting only makes sense if COPR's terms (open
source only, no non-free bits) stop fitting.

## 6. Release checklist (RPM part)

When cutting `vX.Y.Z`:

1. Bump the version in `Cargo.toml` (`[workspace.package]`), `meson.build`,
   and `packaging/rttx/rttx.spec` (`Version:`); reset `Release:` to `1%{?snapinfo}%{?dist}`.
2. Add a `%changelog` entry at the top of the spec
   (`* Day Mon DD YYYY Name <email> - X.Y.Z-1`).
3. Add the `<release>` to `clients/rttx/data/io.github.IllyaYalovyy.rttx.metainfo.xml`
   and move `CHANGELOG.md`'s *Unreleased* section under the new version.
4. Run `./packaging/rttx/rpm/build-srpm.sh --mock fedora-43-x86_64` and make
   sure it's green — cheaper than finding out from the tag pipeline.
5. Commit, tag `vX.Y.Z`, push the tag. CI does the rest (§5.3).

If you only need a rebuild of the *same* version (spec fix, rebuild against a
new VTE), bump `Release:` to `2%{?snapinfo}%{?dist}` and add a changelog line —
never reuse a version-release pair that has been published.

## 7. Troubleshooting

- **`error: workspace version X != spec Version Y`** — step 1 of §6 was skipped.
- **`no matching package` / network errors in `build.log`** — the vendor
  tarball is stale; rerun `build-srpm.sh` (it regenerates it from `Cargo.lock`).
- **`%check` fails only in mock/COPR** — the daemon integration tests spawn real
  shells, and mock's `systemd-nspawn` exports an unusual environment
  (`TERM=vt100`, `PROMPT_COMMAND=printf "\033]0;<mock-chroot>\007"`, `PS1`).
  The spec already unsets `PROMPT_COMMAND`/`PS1` before running the tests; if a
  new test starts depending on something else from the environment, the panic
  message in `build.log` usually names it (look for `<mock-chroot>`). Use
  `--without-check` to unblock a release while the test is fixed (note it in
  the changelog).
- **Older Fedora (VTE < 0.78)** — the client's default cargo feature is
  `vte-0_78`. Building for a release with VTE 0.76 needs
  `%cargo_build -n -f vte-0_76 -- -p rttx -p rttx-server` and a matching
  `pkgconfig(vte-2.91-gtk4) >= 0.76`. Not needed for Fedora 43+.
- **Expected `rpmlint` output** — `no-manual-page-for-binary rttx` (only the
  daemon has a man page today) and, for snapshot builds only,
  `incoherent-version-in-changelog` (the `%changelog` names `1.0.0-1`, the
  package is `1.0.0-1.<date>git<hash>`). Anything else is worth a look.
- **`rpmlint` complains about the `License:` field** — regenerate the crate
  license list: `%cargo_license_summary` prints it during `%build` (search
  `build.log` for lines starting with `# `) and update the SPDX expression.
- **Building a snapshot that must look like a release** (e.g. tag not fetched
  in a shallow clone): `build-srpm.sh --release`.
