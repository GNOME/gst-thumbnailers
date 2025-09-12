fn main() -> gio::glib::ExitCode {
    gst_video_thumbnailer::main(&std::env::args().collect::<Vec<_>>())
}
