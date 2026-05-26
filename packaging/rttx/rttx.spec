Name:           rttx
Version:        0.6.3
Release:        1%{?dist}
Summary:        A tiling terminal emulator for GNOME

License:        GPL-3.0-or-later
URL:            https://github.com/IllyaYalovyy/rttx
Source0:        %{url}/archive/v%{version}.tar.gz

BuildRequires:  meson
BuildRequires:  cargo
BuildRequires:  rust-packaging
BuildRequires:  gtk4-devel >= 4.14
BuildRequires:  libadwaita-devel >= 1.5
BuildRequires:  vte291-gtk4-devel >= 0.76
BuildRequires:  desktop-file-utils
BuildRequires:  libappstream-glib

%description
A tiling terminal emulator for GNOME built with Rust, GTK4, and Libadwaita.
Focuses on practicality, stability, and deep integration with the GNOME desktop.

%prep
%autosetup

%build
%meson
%meson_build

%install
%meson_install

%check
%meson_test
desktop-file-validate %{buildroot}%{_datadir}/applications/io.github.IllyaYalovyy.rttx.desktop
appstream-util validate-relax --nonet %{buildroot}%{_datadir}/metainfo/io.github.IllyaYalovyy.rttx.metainfo.xml

%files
%license LICENSE
%doc clients/rttx/README.md
%{_bindir}/rttx
%{_bindir}/rttx-server
%{_datadir}/applications/io.github.IllyaYalovyy.rttx.desktop
%{_datadir}/metainfo/io.github.IllyaYalovyy.rttx.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/io.github.IllyaYalovyy.rttx.svg

%changelog
* Fri Mar 20 2026 Yaroslav Yalovyi <yalovoy@gmail.com> - 0.1.0-1
- Initial release
