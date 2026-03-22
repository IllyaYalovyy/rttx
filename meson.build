project('rttx',
  version: '0.1.0',
  license: 'GPL-3.0-or-later',
  meson_version: '>= 0.59',
)

# Paths
prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
datadir = prefix / get_option('datadir')
iconsdir = datadir / 'icons'
desktopdir = datadir / 'applications'
metainfodir = datadir / 'metainfo'

app_id = 'io.github.IllyaYalovyy.rttx'

# Dependencies (checked early for better user feedback)
dependency('gtk4', version: '>= 4.14')
dependency('libadwaita-1', version: '>= 1.5')
dependency('vte-2.91-gtk4', version: '>= 0.76')

# Find cargo
cargo = find_program('cargo')

# Build the Rust binary with auto-format and clippy
custom_target('rttx-binary',
  output: 'rttx',
  command: [
    'sh', '-c',
    cargo.full_path() + ' fmt && ' +
    cargo.full_path() + ' clippy -- -D warnings && ' +
    cargo.full_path() + ' build --release --manifest-path ' + meson.project_source_root() / 'Cargo.toml' +
    ' --target-dir ' + meson.project_build_root() / 'cargo-target && ' +
    'cp ' + meson.project_build_root() / 'cargo-target' / 'release' / 'rttx' + ' @OUTPUT@'
  ],
  install: true,
  install_dir: bindir,
  console: true,
  build_always_stale: true,
)

# Desktop file
install_data(
  'data' / app_id + '.desktop',
  install_dir: desktopdir,
)

# AppStream metainfo
install_data(
  'data' / app_id + '.metainfo.xml',
  install_dir: metainfodir,
)

# Icon
install_data(
  'data' / 'icons' / 'hicolor' / 'scalable' / 'apps' / app_id + '.svg',
  install_dir: iconsdir / 'hicolor' / 'scalable' / 'apps',
)
