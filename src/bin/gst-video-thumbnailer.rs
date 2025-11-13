fn main() {
    if let Err(err) = gst_thumbnailers::main_video_thumbnailer(std::env::args()) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
