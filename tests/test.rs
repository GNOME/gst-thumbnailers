use gio::prelude::*;

#[test]
fn test() {
    for (path, var_ref) in [
        ("1.webm", 2200.),
        ("2.webm", 2200.),
        ("3.webm", 2200.),
        ("long.webm", 1000.),
        ("uneven.webm", 2200.),
        // Should use embedded cover image instead of frame
        ("1-cover.mkv", 5118.),
    ] {
        let data = run_thumbnailer(path);
        let var = gst_thumbnailers::variance(&data);

        assert!(
            f32::abs(var - var_ref) < 200.,
            "{path}: {var:.0} is not approx equal {var_ref}"
        )
    }
}

fn run_thumbnailer(video: &str) -> Vec<u8> {
    gst_thumbnailers::main_video_thumbnailer(&[
        "gst-video-thumbnailer",
        "-i",
        &gio::File::for_path(format!("tests/{video}")).uri(),
        "-o",
        "tests/test-output.png",
        "-s",
        "256",
    ])
    .unwrap();

    read_png("tests/test-output.png")
}

fn read_png(path: &str) -> Vec<u8> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];

    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());

    buf.iter().flat_map(|x| x.to_ne_bytes()).collect()
}
