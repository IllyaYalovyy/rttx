# RPM spec for rttx — built the Fedora way: vendored crates, offline build via
# cargo-rpm-macros, one SRPM that COPR/mock rebuilds for every Fedora release.
#
# Build an SRPM (and optionally test it in mock) with:
#   packaging/rttx/rpm/build-srpm.sh [--mock fedora-43-x86_64,...]
# See packaging/rttx/rpm/README.md for the full workflow.

# Run the daemon/protocol test suites in %%check (pass `--without check` to skip).
%bcond_without check

%global app_id io.github.IllyaYalovyy.rttx

Name:           rttx
Version:        1.0.1
# build-srpm.sh injects `%%global snapinfo .YYYYMMDDgitHASH` (and %%rttx_commit)
# at the top of the spec it packs into the SRPM for untagged snapshot builds.
Release:        1%{?snapinfo}%{?dist}
Summary:        Tiling terminal emulator for GNOME with a persistent session daemon

# rttx itself: GPL-3.0-or-later. The remainder covers the vendored Rust crates;
# regenerate with `%%cargo_license_summary` (printed during %%build) and compare
# against cargo-vendor.txt / LICENSE.dependencies shipped in %%license.
License:        GPL-3.0-or-later AND Apache-2.0 AND MIT AND (Unlicense OR MIT) AND (MIT OR Apache-2.0 OR LGPL-2.1-or-later) AND (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT)
URL:            https://github.com/IllyaYalovyy/rttx
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz
# `cargo vendor` output for the exact Cargo.lock in Source0; produced by
# packaging/rttx/rpm/build-srpm.sh so the build needs no network access.
Source1:        %{name}-%{version}-vendor.tar.xz

ExclusiveArch:  %{rust_arches}

BuildRequires:  cargo-rpm-macros >= 26
BuildRequires:  pkgconfig(gtk4) >= 4.14
BuildRequires:  pkgconfig(libadwaita-1) >= 1.5
# The default `vte-0_78` cargo feature needs VTE >= 0.78 (Fedora 43+ ships 0.82+).
BuildRequires:  pkgconfig(vte-2.91-gtk4) >= 0.78
# prost-build invokes protoc to compile protocols/rttx-proto/proto/*.proto.
BuildRequires:  protobuf-compiler
BuildRequires:  desktop-file-utils
BuildRequires:  appstream

# Runtime library dependencies (gtk4, libadwaita, vte291-gtk4, glibc, ...) are
# generated automatically from the linked sonames — do not list them by hand.

%description
rttx is a tiling terminal emulator for GNOME built with Rust, GTK4 and
Libadwaita. It provides split-screen terminal panes, named workspaces grouped
by host, input synchronization across panes, and durable sessions.

The rttx-server daemon owns the PTYs and keeps workspaces and per-pane shell
history alive independently of the GUI: closing or restarting the rttx window
(or crashing it) leaves every terminal running, and reconnecting restores the
full layout and scroll-back history. Both the GUI client and the daemon are
shipped in this package.

%prep
# -a1 unpacks the vendor tarball into ./vendor after the main source is extracted.
%autosetup -p1 -a1
# Writes .cargo/config.toml: offline, crates.io -> ./vendor, `rpm` profile with
# Fedora's opt-level/debuginfo/hardening flags, output under target/rpm/.
%cargo_prep -v vendor

%build
# Bake the upstream commit into `rttx --version` (build.rs falls back to `git`,
# which is unavailable in a tarball build). %%rttx_commit is injected by build-srpm.sh.
%{?rttx_commit:export RTTX_GIT_HASH=%{rttx_commit}}
# Build only the shipped binaries (skips the pty-exerciser dev helper).
%cargo_build -- -p rttx -p rttx-server
# Provenance for the vendored crates: list + effective licenses (both shipped).
%cargo_vendor_manifest
%cargo_license_summary
%{cargo_license} > LICENSE.dependencies

%install
install -Dpm0755 target/rpm/rttx        %{buildroot}%{_bindir}/rttx
install -Dpm0755 target/rpm/rttx-server %{buildroot}%{_bindir}/rttx-server

install -Dpm0644 clients/rttx/data/%{app_id}.desktop \
    %{buildroot}%{_datadir}/applications/%{app_id}.desktop
install -Dpm0644 clients/rttx/data/%{app_id}.metainfo.xml \
    %{buildroot}%{_datadir}/metainfo/%{app_id}.metainfo.xml
install -Dpm0644 clients/rttx/data/icons/hicolor/scalable/apps/%{app_id}.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
install -Dpm0644 services/rttx-server/man/rttx-server.1 \
    %{buildroot}%{_mandir}/man1/rttx-server.1

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{app_id}.desktop
appstreamcli validate --no-net %{buildroot}%{_datadir}/metainfo/%{app_id}.metainfo.xml
%if %{with check}
# Headless suites only: the GTK client tests need a display server (they run
# in the quality CI). Cap concurrency — the daemon integration tests each spawn
# a real rttx-server process (mirrors .cargo/config.toml, which %%cargo_prep
# replaces).
# mock/COPR export PROMPT_COMMAND (an OSC title printf) and PS1 into the build;
# the daemon tests spawn real shells that would inherit them and emit
# TitleChanged events the tests don't expect. Give the shells a clean slate.
unset PROMPT_COMMAND PS1
RUST_TEST_THREADS=5 %cargo_test -- -p rttx-proto -p rttx-server
%endif

%files
%license LICENSE LICENSE.dependencies cargo-vendor.txt
%doc README.md CHANGELOG.md
%{_bindir}/rttx
%{_bindir}/rttx-server
%{_datadir}/applications/%{app_id}.desktop
%{_datadir}/metainfo/%{app_id}.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
%{_mandir}/man1/rttx-server.1*

%changelog
* Thu Sep 03 2026 Illya Yalovyy <yalovoy@gmail.com> - 1.0.1-1
- Add a "Support rttx" About-window link and an AppStream donation URL
- Ship the reworked Fedora source package (cargo-rpm-macros, vendored crates)
- Fix daemon-restart terminal-mode reset for panes that ran a full-screen TUI

* Wed Aug 26 2026 Illya Yalovyy <yalovoy@gmail.com> - 1.0.0-1
- Rebuild as a source package: vendored crates, offline cargo-rpm-macros build,
  rttx-server daemon and man page included, desktop/AppStream validation in %%check
