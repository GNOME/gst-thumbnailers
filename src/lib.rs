mod cli;
mod error;

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use clap::Parser;
pub use error::*;
use gio::glib;
use gio::prelude::*;
use gst::prelude::*;

const SCALE_FILTER1: image::imageops::FilterType = image::imageops::FilterType::Nearest;
const SCALE_FILTER2: image::imageops::FilterType = image::imageops::FilterType::Triangle;

fn init<I, T>(args: I) -> cli::Args
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

    cli::Args::parse_from(args)
}

pub fn main_audio_thumbnailer<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = init(args);

    get_audio_thumbnail_source(&args.source.uri())?
        .ok_or(Error::other("No tag image found"))?
        .write_png(&args.output, args.size)
        .unwrap();

    Ok(())
}

pub fn main_video_thumbnailer<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = init(args);

    get_video_thumbnail_source(&args.source.uri(), args.size)?
        .write_png(&args.output, args.size)
        .unwrap();

    Ok(())
}

fn get_audio_thumbnail_source(input_uri: &str) -> Result<Option<ThumbnailSource>> {
    let pipeline = Pipeline::new();

    // Source
    let uridecodebin = gst::ElementFactory::make("uridecodebin3")
        .property("uri", input_uri)
        .build()?;

    // Sink
    let fakesink = gst::ElementFactory::make("fakesink")
        .property("sync", false)
        .build()?;

    pipeline.add_many([&uridecodebin, &fakesink])?;

    // Connect dynamic pad from uridecodebin3 to fakesink
    uridecodebin.connect_pad_added(move |_, src_pad| {
        let sink_pad = fakesink.static_pad("sink").unwrap();
        if !sink_pad.is_linked() {
            src_pad.link(&sink_pad).unwrap();
        }
    });

    // Get stream initialized
    match pipeline.set_state(gst::State::Paused) {
        Ok(gst::StateChangeSuccess::NoPreroll) => {
            return Err(Error::other(
                "Error: thumbnails of live streams make little sense",
            ));
        }
        Err(_) => {
            return Err(Error::other(state_change_error_details(&pipeline)));
        }
        Ok(_) => {}
    }

    // Wait until stream is initialized
    while let Some(message) = pipeline.bus().unwrap().timed_pop(gst::ClockTime::NONE) {
        match message.view() {
            gst::MessageView::AsyncDone(_) => return Ok(None),
            gst::MessageView::Error(err) => {
                return Err(Error::other(format!(
                    "Error: Failed pre-rolling pipeline: {err}"
                )));
            }
            gst::MessageView::Tag(tag) => {
                if let Some(sample) = get_thumbnail_from_tag(tag) {
                    return Ok(Some(ThumbnailSource::CoverArt(sample)));
                }
            }
            _ => {}
        }
    }

    Ok(None)
}

fn get_video_thumbnail_source(input_uri: &str, thumbnail_size: u16) -> Result<ThumbnailSource> {
    let pipeline = Pipeline::new();

    // Source
    let uridecodebin = gst::ElementFactory::make("uridecodebin3")
        .property("uri", input_uri)
        .build()?;

    // Filters
    let videoscale = gst::ElementFactory::make("videoscale").build()?;
    let videoconvert = gst::ElementFactory::make("videoconvert").build()?;
    let capsfilter = gst::ElementFactory::make("capsfilter").build()?;
    let videoflip = gst::ElementFactory::make("videoflip")
        .property("video-direction", gst_video::VideoOrientationMethod::Auto)
        .build()?;

    // Sink
    let appsink = gst_app::AppSink::builder()
        .sync(false)
        // Only keep one frame in buffer and block on it
        .max_buffers(1)
        .build();

    pipeline.add_many([
        &uridecodebin,
        &videoscale,
        &videoconvert,
        &capsfilter,
        &videoflip,
        appsink.upcast_ref(),
    ])?;

    // Static links
    gst::Element::link_many([
        &videoscale,
        &videoconvert,
        &capsfilter,
        &videoflip,
        appsink.upcast_ref(),
    ])?;

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

    // This error message will be replace once pads are detected
    let source_link_status = Arc::new(Mutex::new(Err(Error::other("No pad added for source."))));
    uridecodebin.connect_pad_added(glib::clone!(
        #[strong]
        source_link_status,
        move |_, src_pad| {
            let link_source = || {
                let stream = src_pad.stream().unwrap();
                if stream.stream_type() != gst::StreamType::VIDEO {
                    return Err(Error::other(format!(
                        "Stream is of type '{}' instead of 'video'",
                        stream.stream_type()
                    )));
                }
                let caps = stream.caps().unwrap();
                let s = caps.structure(0).unwrap();

                let mut width = s.get::<i32>("width").unwrap() as f32;
                let height = s.get::<i32>("height").unwrap() as f32;
                if let Some(par) = s
                    .get_optional::<gst::Fraction>("pixel-aspect-ratio")
                    .map_err(Error::other)?
                {
                    width *= par.numer() as f32 / par.denom() as f32;
                }

                let (new_width, new_height) =
                    scale_thumbnail_dimensions(width, height, thumbnail_size);

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
                    src_pad.link(&sink_pad).map_err(Error::other)?;
                }

                Ok(())
            };

            let result = link_source();
            let mut status = source_link_status.lock().unwrap();
            if status.is_err() {
                *status = result
            }
        }
    ));

    // Get stream initialized
    match pipeline.set_state(gst::State::Paused) {
        Ok(gst::StateChangeSuccess::NoPreroll) => {
            return Err(Error::other(
                "Error: thumbnails of live streams make little sense",
            ));
        }
        Err(_) => {
            return Err(Error::other(state_change_error_details(&pipeline)));
        }
        Ok(_) => {}
    }

    // Wait until stream is initialized
    while let Some(message) = pipeline.bus().unwrap().timed_pop(gst::ClockTime::NONE) {
        match message.view() {
            gst::MessageView::StreamsSelected(_) => {
                // This is fired after all pads have been connected. So check here if a usable
                // pad has been connected.
                std::mem::replace(&mut *source_link_status.lock().unwrap(), Ok(()))?;
            }
            gst::MessageView::AsyncDone(_) => {
                // We didn't find a stored thumbnail/cover, so continue with extracting frames
                break;
            }
            gst::MessageView::Error(err) => {
                return Err(Error::other(format!("Failed pre-rolling pipeline: {err}")));
            }
            gst::MessageView::Tag(tag) => {
                if let Some(sample) = get_thumbnail_from_tag(tag) {
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

    let mut samples = vec![appsink.pull_preroll()?];

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
            return Err(Error::other(format!(
                "Error: Failed pre-rolling pipeline after seek: {err}"
            )));
        }

        samples.push(appsink.pull_preroll()?);
    }

    let samples_with_variance = samples
        .into_iter()
        .filter_map(|x| {
            let caps = x.caps().unwrap();
            let info = gst_video::VideoInfo::from_caps(caps).ok()?;

            let data = x.buffer()?.map_readable().ok()?;
            let var = variance(&data, info.width(), info.stride()[0] as u32, info.height());
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
    let info = gst_video::VideoInfo::from_caps(caps)?;
    let width = info.width();
    let height = info.height();
    let stride = info.stride()[0] as usize;

    let new_stride = width as usize * 3;
    let sample_map = sample.buffer().unwrap().map_readable()?;

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

fn state_change_error_details(pipeline: &gst::Pipeline) -> String {
    let mut err_msg = String::from("Error: Failed setting pipeline to PAUSED");
    if let Some(msg) = pipeline
        .bus()
        .unwrap()
        .pop_filtered(&[gst::MessageType::Error])
    {
        let gst::MessageView::Error(msg) = msg.view() else {
            unreachable!();
        };

        err_msg.push('\n');
        err_msg.push_str(&msg.to_string());
        if let Some(debug) = msg.debug() {
            err_msg.push('\n');
            err_msg.push_str(&debug);
        }
    }

    err_msg
}

fn get_thumbnail_from_tag(tag: &gst::message::Tag) -> Option<gst::Sample> {
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
        // See: https://gitlab.gnome.org/sophie-h/gst-thumbnailers/-/issues/4
        match image_type {
            Some(3) => {
                // Front cover found - use it immediately
                return Some(sample);
            }
            Some(1) | None if cover_sample.is_none() => {
                // Save as fallback
                cover_sample = Some(sample);
            }
            _ => {}
        }
    }

    cover_sample
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

struct Pipeline(gst::Pipeline);

impl Pipeline {
    pub fn new() -> Self {
        Self(gst::Pipeline::new())
    }
}

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

#[derive(Debug)]
pub enum ThumbnailSource {
    VideoFrame(u32, u32, Vec<u8>),
    CoverArt(gst::Sample),
}

impl ThumbnailSource {
    fn write_png(&self, output_path: &Path, thumbnail_size: u16) -> Result<()> {
        match self {
            ThumbnailSource::VideoFrame(width, height, frame) => {
                write_png(output_path, *width, *height, frame)?;
                Ok(())
            }
            ThumbnailSource::CoverArt(sample) => {
                let buffer = sample.buffer().unwrap();
                let map = buffer.map_readable()?;

                let loader = gly::Loader::for_bytes(&gly::glib::Bytes::from_owned(map.to_vec()));
                loader.set_accepted_memory_formats(gly::MemoryFormatSelection::R8G8B8);

                let image = loader.load()?;
                let frame = image.next_frame()?;

                let (thumbnail_width, thumbnail_height) = scale_thumbnail_dimensions(
                    frame.width() as f32,
                    frame.height() as f32,
                    thumbnail_size,
                );
                let data = resize::<image::Rgb<u8>>(&frame, thumbnail_width, thumbnail_height);

                let creator = gly::Creator::new("image/png")?;
                creator.add_frame(
                    thumbnail_width,
                    thumbnail_height,
                    frame.memory_format(),
                    &gly::glib::Bytes::from_owned(data),
                )?;

                let image_data = creator.create()?.unwrap();

                std::fs::File::create(output_path)
                    .unwrap()
                    .write_all(&image_data.data())?;

                Ok(())
            }
        }
    }
}

fn write_png(
    output_path: &Path,
    thumbnail_width: u32,
    thumbnail_height: u32,
    buf: &[u8],
) -> Result<()> {
    let creator = gly::Creator::new("image/png")?;
    creator.add_frame(
        thumbnail_width,
        thumbnail_height,
        gly::MemoryFormat::R8g8b8,
        &gly::glib::Bytes::from_owned(buf.to_vec()),
    )?;

    let encoded_image = creator.create()?.unwrap();

    let data = encoded_image.data();

    let mut out_file = std::fs::File::create(output_path)?;
    out_file.write_all(&data)?;

    Ok(())
}

fn resize<T: image::Pixel<Subpixel = u8> + 'static>(
    frame: &gly::Frame,
    thumbnail_width: u32,
    thumbnail_height: u32,
) -> Vec<u8> {
    let img =
        image::ImageBuffer::<T, _>::from_raw(frame.width(), frame.height(), frame.buf_bytes())
            .unwrap();

    let rought_scaled = image::imageops::resize(
        &img,
        thumbnail_width * 2,
        thumbnail_height * 2,
        SCALE_FILTER1,
    );

    image::imageops::resize(
        &rought_scaled,
        thumbnail_width,
        thumbnail_height,
        SCALE_FILTER2,
    )
    .into_raw()
}

pub fn variance(xs: &[u8], width: u32, stride: u32, height: u32) -> f32 {
    let effective_stride = width as usize * 3; // format == "RGB"
    let len = (effective_stride * height as usize) as f32;

    let avg = xs
        .chunks_exact(stride as usize)
        .map(|line| {
            line[0..effective_stride]
                .iter()
                .map(|&x| x as f32)
                .sum::<f32>()
        })
        .sum::<f32>()
        / len;

    let sq_diff = xs
        .chunks_exact(stride as usize)
        .map(|line| {
            line[0..effective_stride]
                .iter()
                .map(|&x| (x as f32 - avg).powi(2))
                .sum::<f32>()
        })
        .sum::<f32>();

    sq_diff / len
}
