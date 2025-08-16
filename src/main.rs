use std::ffi::{OsStr, OsString};

use gio::prelude::*;
use gst::prelude::*;
use {gstreamer as gst, gstreamer_app as gst_app, gstreamer_video as gst_video};

fn main() {
    let app = gio::Application::new(None, gio::ApplicationFlags::HANDLES_COMMAND_LINE);

    app.add_main_option(
        "input",
        glib::Char::from(b'i'),
        glib::OptionFlags::NONE,
        glib::OptionArg::String,
        "Input URL",
        Some("INPUT_URL"),
    );

    app.add_main_option(
        "output",
        glib::Char::from(b'o'),
        glib::OptionFlags::NONE,
        glib::OptionArg::Filename,
        "Output path",
        Some("OUTPUT_PATH"),
    );

    app.add_main_option(
        "size",
        glib::Char::from(b's'),
        glib::OptionFlags::NONE,
        glib::OptionArg::Int,
        "Maximum thumbnail size",
        Some("SIZE"),
    );

    app.connect_command_line(move |_, args| {
        let args_dict = args.options_dict();

        let Some(input_uri) = args_dict.lookup::<String>("input").unwrap() else {
            eprintln!("Error: Input URI not supplied.");
            return glib::ExitCode::from(2);
        };

        let Some(output_path) = args_dict.lookup::<OsString>("output").unwrap() else {
            eprintln!("Error: Output path not supplied.");
            return glib::ExitCode::from(2);
        };

        let Some(thumbnail_size) = args_dict.lookup::<i32>("size").unwrap() else {
            eprintln!("Error: Size not supplied.");
            return glib::ExitCode::from(2);
        };

        xx(&input_uri, &output_path, thumbnail_size.try_into().unwrap());

        glib::ExitCode::from(0)
    });

    app.run();
}

fn xx(input_uri: &str, output_path: &OsStr, thumbnail_size: u16) {
    let input_file = gio::File::for_uri(input_uri);

    let file_info = input_file
        .query_info(
            &format!(
                "{},{}",
                gio::FILE_ATTRIBUTE_TIME_MODIFIED,
                gio::FILE_ATTRIBUTE_STANDARD_SIZE
            ),
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .unwrap();

    let (width, height, sample) = grab_frame(input_uri, thumbnail_size);
    let buffer = sample.buffer().unwrap();
    let map = buffer.map_readable().unwrap();

    write_png(
        output_path,
        input_uri,
        &file_info,
        width,
        height,
        map.as_slice(),
    );
}

fn grab_frame(input_uri: &str, thumbnail_size: u16) -> (u32, u32, gst::Sample) {
    gst::init().unwrap();

    let pipeline = gst::Pipeline::new();

    // Source
    let uridecodebin = gst::ElementFactory::make("uridecodebin")
        .property("uri", input_uri)
        .build()
        .unwrap();

    // Filters
    let videoscale = gst::ElementFactory::make("videoscale").build().unwrap();
    let videoconvert = gst::ElementFactory::make("videoconvert").build().unwrap();
    let capsfilter = gst::ElementFactory::make("capsfilter").build().unwrap();

    // Sink
    let appsink = gst::ElementFactory::make("appsink").build().unwrap();

    pipeline
        .add_many([
            &uridecodebin,
            &videoscale,
            &videoconvert,
            &capsfilter,
            &appsink,
        ])
        .unwrap();

    // Static links
    gst::Element::link_many([&videoscale, &videoconvert, &capsfilter, &appsink]).unwrap();

    uridecodebin.connect_pad_added(move |_, src_pad| {
        let caps = src_pad.current_caps().unwrap();
        let s = caps.structure(0).unwrap();

        if !s.name().starts_with("video/") {
            return;
        }

        let width = s.get::<i32>("width").unwrap() as f32;
        let height = s.get::<i32>("height").unwrap() as f32;

        let thumbnail_size = thumbnail_size as f32;

        let scale = if width < thumbnail_size && height < thumbnail_size {
            // avoid upscaling
            1.
        } else if width > height {
            thumbnail_size / width
        } else {
            thumbnail_size / height
        };

        let new_width = (width * scale).round() as i32;
        let new_height = (height * scale).round() as i32;

        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGB")
            .field("width", new_width)
            .field("height", new_height)
            .build();

        capsfilter.set_property("caps", &caps);

        // Link source pad to sink of first filter
        let sink_pad = videoscale.static_pad("sink").unwrap();
        if !sink_pad.is_linked() {
            src_pad.link(&sink_pad).unwrap();
        }
    });

    let appsink = appsink.dynamic_cast::<gst_app::AppSink>().unwrap();

    // Only keep one frame in buffer and drop the rest
    appsink.set_property("max-buffers", 1u32);
    appsink.set_property("drop", true);

    // Get stream initialized
    pipeline.set_state(gst::State::Paused).unwrap();

    // Wait until stream is initialized
    pipeline
        .bus()
        .unwrap()
        .timed_pop_filtered(gst::ClockTime::NONE, &[gst::MessageType::AsyncDone]);

    // Determine position in video we want to take as thumbnail
    let seek_to_sec = if let Some(secs) = pipeline
        .query_duration::<gst::ClockTime>()
        .map(|x| x.seconds())
    {
        if secs < 180 {
            // Take frame after 1/3 of the video is over for short videos
            secs / 3
        } else {
            // For longer videos take 2 minutes after which films should have started
            120
        }
    } else {
        eprintln!("Failed to get video length.");
        120
    };

    // Seek to calculated position
    //
    // Allow to fail in the hope that we still get a frame
    if let Err(err) = pipeline.seek_simple(
        gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
        gst::ClockTime::from_seconds(seek_to_sec),
    ) {
        eprintln!("Failed to seek to second {seek_to_sec}: {err}");
    }

    // Set to playing for appsink to return frames
    pipeline.set_state(gst::State::Playing).unwrap();

    // Pull one frame
    let sample = appsink.pull_sample().unwrap();
    let caps = sample.caps().unwrap();
    let info = gst_video::VideoInfo::from_caps(caps).unwrap();
    let width = info.width();
    let height = info.height();

    (width, height, sample)
}

fn write_png(
    output_path: &OsStr,
    input_uri: &str,
    file_info: &gio::FileInfo,
    thumbnail_width: u32,
    thumbnail_height: u32,
    buf: &[u8],
) {
    let out_file = std::fs::File::create(output_path).unwrap();
    let buf_writer = std::io::BufWriter::new(out_file);

    let mut encoder = png::Encoder::new(buf_writer, thumbnail_width, thumbnail_height);
    encoder.set_color(png::ColorType::Rgb);

    // <https://specifications.freedesktop.org/thumbnail-spec/latest/creation.html#addinfos>
    encoder
        .add_text_chunk(String::from("Thumb::URI"), input_uri.to_string())
        .unwrap();
    if let Some(mtime) = file_info.modification_date_time() {
        encoder
            .add_text_chunk(String::from("Thumb::MTime"), mtime.to_unix().to_string())
            .unwrap();
    } else {
        glib::g_warning!("gstreamer-thumbnailer", "Could not read mtime.");
    }
    encoder
        .add_text_chunk(String::from("Thumb::Size"), file_info.size().to_string())
        .unwrap();
    encoder
        .add_text_chunk(
            String::from("Software"),
            format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        )
        .unwrap();

    let mut writer = encoder.write_header().unwrap();

    writer.write_image_data(buf).unwrap();
}
