mod cli;

use clap::Parser;
use std::ffi::OsString;
use std::io::Cursor;
use std::path::Path;

use gio::glib;
use gio::prelude::*;
use gst::prelude::*;
use image::ImageReader;

pub fn main<I, T>(args: I) -> glib::ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    gst::init().unwrap();

    // This could be solved in a cleaner way when GStreamer adds support for
    // sorting decoder factories in uridecodebin3.
    // See: https://gitlab.freedesktop.org/gstreamer/gstreamer/-/issues/959
    // and  https://gitlab.freedesktop.org/gstreamer/gstreamer/-/merge_requests/9672
    disable_hardware_decoders();

    let args = cli::Args::parse_from(args);

    match create_thumbnail(&args.source.uri(), &args.output, args.size) {
        Ok(_) => glib::ExitCode::SUCCESS,
        Err(_) => glib::ExitCode::FAILURE,
    }
}

#[derive(Debug)]
pub enum ThumbnailSource {
    VideoFrame(u32, u32, Vec<u8>),
    CoverArt(gst::Sample),
}

fn create_thumbnail(input_uri: &str, output_path: &Path, thumbnail_size: u16) -> Result<(), ()> {
    match thumbnail_sample(input_uri, thumbnail_size)? {
        ThumbnailSource::VideoFrame(width, height, frame) => {
            write_png(output_path, width, height, frame.as_slice());
            Ok(())
        }
        ThumbnailSource::CoverArt(sample) => {
            let buffer = sample.buffer().ok_or(())?;
            let map = buffer.map_readable().map_err(|_| ())?;
            let decoded_img = ImageReader::new(Cursor::new(map.as_slice()))
                .with_guessed_format()
                .map_err(|err| {
                    eprintln!("Failed to guess image format: {err}");
                })?
                .decode()
                .map_err(|err| {
                    eprintln!("Failed to decode image: {err}");
                })?;
            let (new_width, new_height) = scale_thumbnail_dimensions(
                decoded_img.width() as f32,
                decoded_img.height() as f32,
                thumbnail_size,
            );
            decoded_img
                .resize(new_width, new_height, image::imageops::FilterType::Lanczos3)
                .save(output_path)
                .map_err(|err| {
                    eprintln!(
                        "Error: Failed writing file {}: {err}",
                        output_path.display()
                    );
                })?;
            Ok(())
        }
    }
}

fn thumbnail_sample(input_uri: &str, thumbnail_size: u16) -> Result<ThumbnailSource, ()> {
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

        let (new_width, new_height) = scale_thumbnail_dimensions(width, height, thumbnail_size);

        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGB")
            .field("width", new_width as i32)
            .field("height", new_height as i32)
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
    while let Some(message) = pipeline.bus().unwrap().timed_pop(gst::ClockTime::NONE) {
        match message.view() {
            gst::MessageView::AsyncDone(_) => break,
            gst::MessageView::Error(err) => {
                eprintln!("Error: Failed pre-rolling pipeline: {}", err.error());
                return Err(());
            }
            gst::MessageView::Tag(tag) => {
                // Check for any cover art.
                let tags = tag.tags();
                let mut cover_sample = None;
                for sample_value in tags.iter_tag::<gst::tags::Image>() {
                    let sample = sample_value.get();
                    let Some(caps) = sample.caps() else { continue };

                    let image_type = caps
                        .structure(0)
                        .and_then(|s| s.get::<i32>("image-type").ok());

                    // TODO: Use gst_tag::TagImageType when it's properly exported
                    // Hardcoding values: 0 = None, 1 = Undefined, 3 = FrontCover
                    // See: https://gitlab.gnome.org/sophie-h/gst-video-thumbnailer/-/issues/4
                    match image_type {
                        Some(3) => {
                            // Front cover found - use it immediately
                            return Ok(ThumbnailSource::CoverArt(sample));
                        }
                        Some(1) | None if cover_sample.is_none() => {
                            // Save as fallback
                            cover_sample = Some(sample);
                        }
                        _ => {}
                    }
                }
                if let Some(sample) = cover_sample {
                    return Ok(ThumbnailSource::CoverArt(sample));
                }
            }
            _ => {}
        }
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
        // For long videos, take frames at 10%, 15%, 20%, 25%, 30% of the
        // video This only uses the first third of the video to not spoiler
        // films
        [10, 15, 20, 25, 30]
    } else {
        // For short videos, sample from the complete video
        [10, 20, 30, 60, 90]
    };

    let mut samples = vec![appsink.pull_preroll().unwrap()];

    // Pull frames at seek positions
    for percentage in seek_at {
        let seek_to = duration.mul_div_ceil(percentage, 100).unwrap();

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
    let stride = info.stride()[0] as usize;

    let new_stride = width as usize * 3;
    let sample_map = sample.buffer().unwrap().map_readable().unwrap();

    // Get rid of padding after stride
    let mut buf = vec![0; height as usize * new_stride];
    for (out_line, in_line) in Iterator::zip(
        buf.chunks_exact_mut(new_stride),
        sample_map.chunks_exact(stride),
    ) {
        out_line.copy_from_slice(&in_line[0..new_stride]);
    }

    Ok(ThumbnailSource::VideoFrame(width, height, buf))
}

fn scale_thumbnail_dimensions(width: f32, height: f32, thumbnail_size: u16) -> (u32, u32) {
    let thumbnail_size = thumbnail_size as f32;
    let scale = if width < thumbnail_size && height < thumbnail_size {
        // avoid upscaling
        1.0
    } else if width > height {
        thumbnail_size / width
    } else {
        thumbnail_size / height
    };
    let new_width = (width * scale).round() as u32;
    let new_height = (height * scale).round() as u32;

    (new_width, new_height)
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

fn write_png(output_path: &Path, thumbnail_width: u32, thumbnail_height: u32, buf: &[u8]) {
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
