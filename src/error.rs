use std::fmt::Display;
use std::panic::Location;

use gio::glib;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    location: String,
}

impl Error {
    #[track_caller]
    pub fn other(err: impl Display) -> Self {
        Self {
            kind: ErrorKind::Other(err.to_string()),
            location: location(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {}: {}",
            env!("CARGO_PKG_NAME"),
            self.location,
            self.kind
        )
    }
}
impl std::error::Error for Error {}

#[derive(Debug)]
pub enum ErrorKind {
    GLibBool(glib::BoolError),
    Other(String),
    StdIo(std::io::Error),
    GLib(glib::Error),
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GLibBool(err) => f.write_str(&err.to_string()),
            Self::Other(err) => f.write_str(err),
            Self::StdIo(err) => f.write_str(&err.to_string()),
            Self::GLib(err) => f.write_str(&err.to_string()),
        }
    }
}

impl From<glib::BoolError> for Error {
    #[track_caller]
    fn from(value: glib::BoolError) -> Self {
        Self {
            kind: ErrorKind::GLibBool(value),
            location: location(),
        }
    }
}

impl From<std::io::Error> for Error {
    #[track_caller]
    fn from(value: std::io::Error) -> Self {
        Self {
            kind: ErrorKind::StdIo(value),
            location: location(),
        }
    }
}

impl From<glib::Error> for Error {
    #[track_caller]
    fn from(value: glib::Error) -> Self {
        Self {
            kind: ErrorKind::GLib(value),
            location: location(),
        }
    }
}

#[track_caller]
fn location() -> String {
    let location = Location::caller();
    let path = location
        .file()
        .strip_prefix(env!("CARGO_MANIFEST_DIR"))
        .and_then(|x| x.strip_prefix("/"))
        .unwrap_or(location.file());

    format!("{path}:{}:{}", location.line(), location.column())
}
