use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

pub const TEST_VIDEOS: &[(&str, &str)] = &[
    (
        "https://download.blender.org/demo/movies/ToS/tearsofsteel_4k.mov",
        "tearsofsteel_4k.mov",
    ),
    (
        "https://download.blender.org/demo/movies/ToS/tears_of_steel_720p.mov",
        "tears_of_steel_720p.mov",
    ),
];

pub fn criterion_benchmark(c: &mut Criterion) {
    for (video_url, video_file) in TEST_VIDEOS {
        let path = format!("benches/{video_file}");
        if std::fs::File::open(&path).is_err() {
            eprintln!("Downloading video data from <{video_url}>.\nThis might take a while …");

            std::process::Command::new("curl")
                .args([video_url, "--output", path.as_str()])
                .status()
                .unwrap();
        }

        c.bench_function(&video_file, |b| {
            b.iter(|| run_thumbnailer(black_box(&path)))
        });
    }
    eprintln!(
        "Used GStreamer version:\n{}",
        std::process::Command::new("gst-launch-1.0")
            .arg("--version")
            .output()
            .ok()
            .map(|x| String::from_utf8_lossy(&x.stdout).to_string())
            .unwrap_or_else(|| String::from("'gst-launch-1.0 --version' failed"))
    )
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);

fn run_thumbnailer(video: &str) {
    assert_eq!(
        (),
        gst_video_thumbnailer::main_video_thumbnailer(&[
            "gst-video-thumbnailer",
            "-p",
            video,
            "-o",
            "/dev/null",
            "-s",
            "256"
        ])
    );
}
