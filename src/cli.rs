use gio::prelude::*;
use std::path::PathBuf;

#[derive(Debug, clap::Parser)]
#[command(version, about)]
pub struct Args {
    #[clap(flatten)]
    pub source: Source,
    #[clap(short, long)]
    /// Path under which to output the thumbnail as PNG
    pub output: PathBuf,
    #[clap(short, long)]
    /// Maximum size for width and height of the thumbnail
    pub size: u16,
}

#[derive(Debug, clap::Args)]
#[group(required = true, multiple = false)]
pub struct Source {
    /// URI of file to create the thumbnail for
    #[clap(short, long)]
    pub input_uri: Option<String>,
    /// Path of the file to create the thumbnail for
    #[clap(short = 'p', long)]
    pub input_path: Option<PathBuf>,
}

impl Source {
    pub fn uri(&self) -> String {
        self.input_uri.clone().unwrap_or_else(|| {
            gio::File::for_path(self.input_path.clone().unwrap())
                .uri()
                .to_string()
        })
    }
}
