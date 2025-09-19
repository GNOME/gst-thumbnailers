use std::ffi::{OsStr, OsString};

use gio::glib;
use gio::prelude::*;
use gst::prelude::*;

pub fn main(args: &[impl AsRef<str>]) -> glib::ExitCode {
    gst::init().unwrap();

    // This could be solved in a cleaner way when GStreamer adds support for
    // sorting decoder factories in uridecodebin3.
    // See: https://gitlab.freedesktop.org/gstreamer/gstreamer/-/issues/959
    // and  https://gitlab.freedesktop.org/gstreamer/gstreamer/-/merge_requests/9672
    disable_hardware_decoders();

    let app = gio::Application::new(
        None,
        gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::NON_UNIQUE,
    );

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

    app.connect_command_line(move |_, args: &gio::ApplicationCommandLine| {
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

        let Ok(thumbnail_size) = u16::try_from(thumbnail_size) else {
            eprintln!("Error: Size not supported.");
            return glib::ExitCode::from(2);
        };

        match create_thumbnail(&input_uri, &output_path, thumbnail_size) {
            Ok(_) => glib::ExitCode::SUCCESS,
            Err(_) => glib::ExitCode::FAILURE,
        }
    });

    app.run_with_args(args)
}

fn create_thumbnail(input_uri: &str, output_path: &OsStr, thumbnail_size: u16) -> Result<(), ()> {
    let (width, height, frame) = get_interesting_frame(input_uri, thumbnail_size)?;

    write_png(output_path, width, height, frame.as_slice());

    Ok(())
}

fn get_interesting_frame(input_uri: &str, thumbnail_size: u16) -> Result<(u32, u32, Vec<u8>), ()> {
    struct Pipeline(gst::Pipeline);

    impl std::ops::Deref for Pipeline {
        type Target = gst::Pipeline;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for Pipeline {
        fn drop(&mut self) {
            let _ = self.0.set_state(gst::State::Null);
        }
    }

    let pipeline = Pipeline(gst::Pipeline::new());

    // Source
    let uridecodebin = gst::ElementFactory::make("uridecodebin3")
        .property("uri", input_uri)
        .build()
        .unwrap();

    // Filters
    let videoscale = gst::ElementFactory::make("videoscale").build().unwrap();
    let videoconvert = gst::ElementFactory::make("videoconvert").build().unwrap();
    let capsfilter = gst::ElementFactory::make("capsfilter").build().unwrap();
    let videoflip = gst::ElementFactory::make("videoflip")
        .property("video-direction", gst_video::VideoOrientationMethod::Auto)
        .build()
        .unwrap();

    // Sink
    let appsink = gst_app::AppSink::builder()
        .sync(false)
        // Only keep one frame in buffer and block on it
        .max_buffers(1)
        .build();

    pipeline
        .add_many([
            &uridecodebin,
            &videoscale,
            &videoconvert,
            &capsfilter,
            &videoflip,
            appsink.upcast_ref(),
        ])
        .unwrap();

    // Static links
    gst::Element::link_many([
        &videoscale,
        &videoconvert,
        &capsfilter,
        &videoflip,
        appsink.upcast_ref(),
    ])
    .unwrap();

    // Manually set number of worker threads for decoders in order to reduce memory
    // usage on setups with many cores, see
    // https://gitlab.freedesktop.org/gstreamer/gstreamer/-/issues/4423
    uridecodebin.connect_closure(
        "deep-element-added",
        false,
        glib::closure!(
            |_uridecodebin: &gst::Element, _bin: &gst::Bin, element: &gst::Element| {
                // WARNING!
                // Be careful adding support for new elements in the future here. Make sure
                // your tests have covered newly added code, since it's easy to use an incorrect
                // type for the "number of threads" property. Some elements use an unsigned
                // integer, others a signed integer. Mixing them up will result
                // in a runtime crash with no compiler warning.
                if element
                    .factory()
                    .is_some_and(|factory| factory.name().starts_with("avdec_"))
                {
                    let gobject_class = element.class();
                    if gobject_class.find_property("max-threads").is_some() {
                        element.set_property("max-threads", 1i32);
                    }
                } else if element
                    .factory()
                    .is_some_and(|factory| factory.name() == "dav1ddec")
                {
                    element.set_property("n-threads", 1u32);
                } else if element
                    .factory()
                    .is_some_and(|factory| ["vp8dec", "vp9dec"].contains(&factory.name().as_str()))
                {
                    element.set_property("threads", 1u32);
                }
            }
        ),
    );

    uridecodebin.connect_pad_added(move |_, src_pad| {
        let stream = src_pad.stream().unwrap();
        if stream.stream_type() != gst::StreamType::VIDEO {
            return;
        }
        let caps = stream.caps().unwrap();
        let s = caps.structure(0).unwrap();

        let mut width = s.get::<i32>("width").unwrap() as f32;
        let height = s.get::<i32>("height").unwrap() as f32;
        if let Some(par) = s
            .get_optional::<gst::Fraction>("pixel-aspect-ratio")
            .unwrap()
        {
            width *= par.numer() as f32 / par.denom() as f32;
        }

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
            .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
            .build();

        capsfilter.set_property("caps", caps);

        // Link source pad to sink of first filter
        let sink_pad = videoscale.static_pad("sink").unwrap();
        if !sink_pad.is_linked() {
            src_pad.link(&sink_pad).unwrap();
        }
    });

    // Get stream initialized
    match pipeline.set_state(gst::State::Paused) {
        Ok(gst::StateChangeSuccess::NoPreroll) => {
            eprintln!("Error: thumbnails of live streams make little sense");
            return Err(());
        }
        Err(_) => {
            eprintln!("Error: Failed setting pipeline to PAUSED");
            if let Some(msg) = pipeline
                .bus()
                .unwrap()
                .pop_filtered(&[gst::MessageType::Error])
            {
                let gst::MessageView::Error(msg) = msg.view() else {
                    unreachable!();
                };

                eprintln!("\t{}", msg.error());
                if let Some(debug) = msg.debug() {
                    eprintln!("\t{debug}");
                }
            }
            return Err(());
        }
        _ => {}
    }

    // Wait until stream is initialized
    let msg = pipeline.bus().unwrap().timed_pop_filtered(
        gst::ClockTime::NONE,
        &[gst::MessageType::Error, gst::MessageType::AsyncDone],
    );

    if let Some(gst::MessageView::Error(err)) = msg.as_ref().map(|msg| msg.view()) {
        eprintln!("Error: Failed pre-rolling pipeline: {}", err.error());
        return Err(());
    }

    pipeline.debug_to_dot_file_with_ts(
        gst::DebugGraphDetails::all(),
        "gst_video_thumbnailer_paused",
    );

    let duration = if let Some(duration) = pipeline.query_duration::<gst::ClockTime>() {
        duration
    } else {
        eprintln!("Failed to get video length.");
        gst::ClockTime::ZERO
    };

    // Determine position in video we want to take as thumbnail
    let seek_at = if duration > 180.seconds() {
        // For long videos, take frames at 1/15, 1/15, 2/15, 3/15, 4/15, 5/15 of the
        // video This only uses the first third of the video to not spoiler
        // films
        [15 / 1, 15 / 2, 15 / 3, 15 / 4, 15 / 5]
    } else {
        // For short videos, sample from the complete video
        [10 / 1, 10 / 2, 10 / 3, 10 / 6, 10 / 9]
    };

    let mut samples = vec![appsink.pull_preroll().unwrap()];

    // Pull frames at seek positions
    for divide_by in seek_at {
        let seek_to = duration / divide_by;

        // Seek to calculated position
        //
        // Allow to fail in the hope that we still get a frame
        if pipeline
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, seek_to)
            .is_err()
        {
            eprintln!("Failed to seek to {seek_to}");
        }

        // Wait until seek is finished
        let msg = pipeline.bus().unwrap().timed_pop_filtered(
            gst::ClockTime::NONE,
            &[gst::MessageType::Error, gst::MessageType::AsyncDone],
        );

        if let Some(gst::MessageView::Error(err)) = msg.as_ref().map(|msg| msg.view()) {
            eprintln!(
                "Error: Failed pre-rolling pipeline after seek: {}",
                err.error()
            );
            return Err(());
        }

        samples.push(appsink.pull_preroll().unwrap());
    }

    let samples_with_variance = samples
        .into_iter()
        .filter_map(|x| {
            let data = x.buffer()?.map_readable().ok()?;
            let var = variance(data.as_slice());
            drop(data);
            Some((x, var))
        })
        .collect::<Vec<_>>();

    // Use sample with highest variance
    let (sample, _) = samples_with_variance
        .iter()
        .max_by(|(_, var1), (_, var2)| var1.partial_cmp(var2).unwrap())
        .unwrap();
    let caps = sample.caps().unwrap();
    let info = gst_video::VideoInfo::from_caps(caps).unwrap();
    let width = info.width();
    let height = info.height();
    let stride = info.comp_stride(0) as usize;

    let new_stride = width as usize * 3;
    let sample_map = sample.buffer().unwrap().map_readable().unwrap();

    let mut buf = Vec::with_capacity(height as usize * new_stride);
    for x in 0..height as usize {
        let p0 = x * stride;
        let p1 = p0 + new_stride;

        buf.extend_from_slice(&sample_map[p0..p1]);
    }

    Ok((width, height, buf))
}

fn filter_hw_decoders(feature: &gst::PluginFeature) -> bool {
    let factory = match feature.downcast_ref::<gst::ElementFactory>() {
        Some(f) => f,
        None => return false,
    };
    factory.has_type(gst::ElementFactoryType::MEDIA_VIDEO)
        && factory.has_type(gst::ElementFactoryType::DECODER)
        && factory.has_type(gst::ElementFactoryType::HARDWARE)
}

fn disable_hardware_decoders() {
    let registry = gst::Registry::get();
    let hw_list = registry.features_filtered(filter_hw_decoders, false);
    for l in hw_list.iter() {
        registry.remove_feature(l);
    }
}

fn write_png(output_path: &OsStr, thumbnail_width: u32, thumbnail_height: u32, buf: &[u8]) {
    let out_file = std::fs::File::create(output_path).unwrap();
    let buf_writer = std::io::BufWriter::new(out_file);

    let mut encoder = png::Encoder::new(buf_writer, thumbnail_width, thumbnail_height);
    encoder.set_color(png::ColorType::Rgb);

    let mut writer = encoder.write_header().unwrap();

    writer.write_image_data(buf).unwrap();
}

pub fn variance(xs: &[u8]) -> f32 {
    let avg = xs.iter().map(|x| *x as f32).sum::<f32>() / xs.len() as f32;
    let sq_diff = xs.iter().map(|x| (*x as f32 - avg).powi(2)).sum::<f32>();

    sq_diff / xs.len() as f32
}
