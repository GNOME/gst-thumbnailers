use std::ffi::{OsStr, OsString};

use gio::glib;
use gio::prelude::*;
use gst::prelude::*;

pub fn main(args: &[impl AsRef<str>]) -> glib::ExitCode {
    gst::init().unwrap();

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
    let sample = get_png_sample(input_uri, thumbnail_size)?;

    let buffer = sample.buffer().unwrap();
    let map = buffer.map_readable().unwrap();

    std::fs::write(output_path, map.as_slice()).map_err(|err| {
        eprint!(
            "Error: Failed writing file {}: {err}",
            output_path.display()
        );
    })?;

    Ok(())
}

fn get_png_sample(input_uri: &str, thumbnail_size: u16) -> Result<gst::Sample, ()> {
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
    let uridecodebin = gst::ElementFactory::make("uridecodebin")
        .property("uri", input_uri)
        .build()
        .unwrap();

    // Filters
    let videoscale = gst::ElementFactory::make("videoscale").build().unwrap();
    let videoconvert = gst::ElementFactory::make("videoconvert").build().unwrap();
    let capsfilter = gst::ElementFactory::make("capsfilter").build().unwrap();
    let pngenc = gst::ElementFactory::make("pngenc")
        .property("snapshot", true)
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
            &pngenc,
            appsink.upcast_ref(),
        ])
        .unwrap();

    // Static links
    gst::Element::link_many([
        &videoscale,
        &videoconvert,
        &capsfilter,
        &pngenc,
        appsink.upcast_ref(),
    ])
    .unwrap();

    disable_hardware_decoders();

    uridecodebin.connect_pad_added(move |_, src_pad| {
        let caps = src_pad.current_caps().unwrap();
        let s = caps.structure(0).unwrap();

        if !s.name().starts_with("video/") {
            return;
        }

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

    // Determine position in video we want to take as thumbnail
    let seek_to = if let Some(duration) = pipeline.query_duration::<gst::ClockTime>() {
        if duration < 180.seconds() {
            // Take frame after 1/3 of the video is over for short videos
            duration / 3
        } else {
            // For longer videos take 2 minutes after which films should have started
            120.seconds()
        }
    } else {
        eprintln!("Failed to get video length.");
        gst::ClockTime::ZERO
    };

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

    // Pull one frame
    appsink.pull_preroll().map_err(|_| ())
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

pub fn variance(xs: &[u8]) -> f32 {
    let avg = xs.iter().map(|x| *x as f32).sum::<f32>() / xs.len() as f32;
    let sq_diff = xs.iter().map(|x| (*x as f32 - avg).powi(2)).sum::<f32>();

    sq_diff / xs.len() as f32
}
