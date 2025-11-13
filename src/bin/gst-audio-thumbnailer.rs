fn main() {
    if let Err(err) = gst_thumbnailers::main_audio_thumbnailer(std::env::args()) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
