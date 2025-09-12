project(
  'gst-video-thumbnailer',
  'rust',
  version: '0.1.0',
  meson_version: '>=1.2',
)

if get_option('profile') == 'release'
  rust_target = 'release'
  cargo_profile = 'release'
else
  rust_target = 'debug'
  cargo_profile = 'dev'
endif

bindir = get_option('prefix') / get_option('bindir')

cargo_target_dir = meson.project_build_root() / 'cargo-target'
cargo_home = meson.project_build_root() / 'cargo-home'
cargo_manifest = meson.project_source_root() / 'Cargo.toml'

cargo_bin = find_program('cargo')

cargo_env = environment()
cargo_env.set('CARGO_HOME', cargo_home)

cargo_build = custom_target(
  'gst-video-thumbnailer',
  build_by_default: true,
  build_always_stale: true,
  output: 'gst-video-thumbnailer-bin',
  console: true,
  env: cargo_env,
  command: [
    cargo_bin,
    'build',
    '--target-dir', cargo_target_dir,
    '--profile', cargo_profile,
    '--manifest-path', cargo_manifest,
  ],
)

custom_target(
  'gst-video-thumbnailer-cp-binary',
  depends: cargo_build,
  build_by_default: true,
  build_always_stale: true,
  install: true,
  install_dir: bindir,
  output: 'gst-video-thumbnailer',
  command: [
    'cp',
    cargo_target_dir / rust_target / 'gst-video-thumbnailer',
    '@OUTPUT@',
  ],
)
