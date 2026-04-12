project('rttx',
  version: '0.2.9',
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

# Register tracked git hooks so they apply in every fresh clone automatically
git = find_program('git', required: false)
if git.found()
  run_command(git, 'config', 'core.hooksPath', '.githooks', check: false)
endif

# Dependencies (checked early for better user feedback)
dependency('gtk4', version: '>= 4.14')
dependency('libadwaita-1', version: '>= 1.5')

vte_version = get_option('vte_version')
dependency('vte-2.91-gtk4', version: '>= ' + vte_version)

if vte_version.version_compare('>= 0.78')
  cargo_vte_features = '--features=vte-0_78'
else
  cargo_vte_features = '--no-default-features --features=vte-0_76'
endif

# Find cargo
cargo = find_program('cargo')
workspace_manifest = meson.project_source_root() / 'Cargo.toml'
client_dir = meson.project_source_root() / 'clients' / 'rttx'
client_data_dir = client_dir / 'data'

# Build the Rust binary with formatting and clippy verification
custom_target('rttx-binary',
  output: 'rttx',
  command: [
    'sh', '-c',
    cargo.full_path() + ' fmt --manifest-path ' + workspace_manifest + ' --all --check && ' +
    cargo.full_path() + ' clippy --manifest-path ' + client_dir / 'Cargo.toml' + ' ' + cargo_vte_features + ' -- -D warnings && ' +
    cargo.full_path() + ' build --release -p rttx ' + cargo_vte_features + ' --manifest-path ' + workspace_manifest +
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
  client_data_dir / app_id + '.desktop',
  install_dir: desktopdir,
)

# AppStream metainfo
install_data(
  client_data_dir / app_id + '.metainfo.xml',
  install_dir: metainfodir,
)

# Icon
install_data(
  client_data_dir / 'icons' / 'hicolor' / 'scalable' / 'apps' / app_id + '.svg',
  install_dir: iconsdir / 'hicolor' / 'scalable' / 'apps',
)

install_data(
  client_data_dir / 'icons' / 'hicolor' / 'scalable' / 'apps' / app_id + '.Devel.svg',
  install_dir: iconsdir / 'hicolor' / 'scalable' / 'apps',
)
