project(
  'gst-thumbnailers',
  'rust',
  version: '1.0.alpha.1',
  meson_version: '>=1.2',
)

dependency('gstreamer-1.0', version: '>=1.26.0')
dependency('glycin-2', version: '>=2.0.0')

if get_option('profile') == 'release'
  rust_target = 'release'
  cargo_profile = 'release'
else
  rust_target = 'debug'
  cargo_profile = 'dev'
endif

bindir = get_option('prefix') / get_option('bindir')
datadir = get_option('prefix') / get_option('datadir')

cargo_target_dir = meson.project_build_root() / 'cargo-target'
cargo_home = meson.project_build_root() / 'cargo-home'
cargo_manifest = meson.project_source_root() / 'Cargo.toml'

cargo_bin = find_program('cargo')

cargo_env = environment()
cargo_env.set('CARGO_HOME', cargo_home)

cargo_options = ['--manifest-path', cargo_manifest]
cargo_options += ['--target-dir', cargo_target_dir]
cargo_options += ['--profile', cargo_profile]

mime_types = {
  'gst-video-thumbnailer': [
    'application/mxf',
    'image/vnd.rn-realpix',
    'image/x-pict',
    'video/3gp',
    'video/3gpp',
    'video/3gpp2',
    'video/dv',
    'video/divx',
    'video/fli',
    'video/flv',
    'video/mp2t',
    'video/mp4',
    'video/mp4v-es',
    'video/mpeg',
    'video/mpeg-system',
    'video/msvideo',
    'video/ogg',
    'video/quicktime',
    'video/vivo',
    'video/vnd.avi',
    'video/vnd.divx',
    'video/vnd.rn-realvideo',
    'video/vnd.vivo',
    'video/webm',
    'video/x-anim',
    'video/x-avi',
    'video/x-flc',
    'video/x-fli',
    'video/x-flic',
    'video/x-flv',
    'video/x-m4v',
    'video/x-matroska',
    'video/x-mjpeg',
    'video/x-mpeg',
    'video/x-mpeg2',
    'video/x-ms-asf',
    'video/x-ms-asf-plugin',
    'video/x-ms-asx',
    'video/x-msvideo',
    'video/x-ms-wm',
    'video/x-ms-wmv',
    'video/x-ms-wmx',
    'video/x-ms-wvx',
    'video/x-nsv',
    'video/x-ogm+ogg',
    'video/x-theora',
    'video/x-theora+ogg',
    'video/x-totem-stream',
  ],
  'gst-audio-thumbnailer': [
    'application/x-shorten',
    'audio/x-pn-realaudio',
    'audio/3gpp',
    'audio/3gpp2',
    'audio/aac',
    'audio/ac3',
    'audio/AMR',
    'audio/AMR-WB',
    'audio/basic',
    'audio/dv',
    'audio/eac3',
    'audio/flac',
    'audio/m4a',
    'audio/midi',
    'audio/mp1',
    'audio/mp2',
    'audio/mp3',
    'audio/mp4',
    'audio/mpeg',
    'audio/mpg',
    'audio/ogg',
    'audio/opus',
    'audio/prs.sid',
    'audio/scpls',
    'audio/vnd.rn-realaudio',
    'audio/wav',
    'audio/webm',
    'audio/x-aac',
    'audio/x-aiff',
    'audio/x-ape',
    'audio/x-flac',
    'audio/x-gsm',
    'audio/x-it',
    'audio/x-m4a',
    'audio/x-m4b',
    'audio/x-matroska',
    'audio/x-mod',
    'audio/x-mp1',
    'audio/x-mp2',
    'audio/x-mp3',
    'audio/x-mpg',
    'audio/x-mpeg',
    'audio/x-ms-asf',
    'audio/x-ms-asx',
    'audio/x-ms-wax',
    'audio/x-ms-wma',
    'audio/x-musepack',
    'audio/x-opus+ogg',
    'audio/x-pn-aiff',
    'audio/x-pn-au',
    'audio/x-pn-wav',
    'audio/x-pn-windows-acm',
    'audio/x-realaudio',
    'audio/x-real-audio',
    'audio/x-s3m',
    'audio/x-sbc',
    'audio/x-shorten',
    'audio/x-speex',
    'audio/x-stm',
    'audio/x-tta',
    'audio/x-wav',
    'audio/x-wavpack',
    'audio/x-vorbis',
    'audio/x-vorbis+ogg',
    'audio/x-xm',
  ],
}

binaries = [
  'gst-video-thumbnailer',
  'gst-audio-thumbnailer',
]

test(
  'Cargo tests (main application)',
  cargo_bin,
  args: ['test', cargo_options],
  timeout: 3600,
  env: cargo_env,
)

gst_thumbnailers = custom_target(
  'gst-thumbnailers',
  build_by_default: true,
  build_always_stale: true,
  output: f'gst-thumbnailers',
  console: true,
  env: cargo_env,
  command: [
    cargo_bin,
    'build',
    cargo_options,
  ],
)

foreach binary_name : binaries

  custom_target(
    f'@binary_name@-cp-binary',
    depends: gst_thumbnailers,
    build_by_default: true,
    build_always_stale: true,
    install: true,
    install_dir: bindir,
    output: binary_name,
    command: [
      'cp',
      cargo_target_dir / rust_target / binary_name,
      '@OUTPUT@',
    ],
  )

  # Install .thumbnailer file
  conf = configuration_data()
  conf.set('BINDIR', bindir)
  conf.set('MIME_TYPES', ';'.join(mime_types[binary_name]) + ';')

  configure_file(
    input: f'@binary_name@.thumbnailer.in',
    output: f'@binary_name@.thumbnailer',
    configuration: conf,
    install: true,
    install_dir: datadir / 'thumbnailers',
  )

endforeach
