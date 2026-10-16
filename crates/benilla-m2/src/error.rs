//! The crate's parse error type.

/// M2 parse error.
#[derive(Debug)]
pub enum Error {
    NotMd20,
    NotSkin,
    UnsupportedVersion(u32),
    Truncated,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotMd20 => write!(f, "not an MD20 model"),
            Error::NotSkin => write!(f, "not a SKIN profile"),
            Error::UnsupportedVersion(v) => {
                write!(f, "unsupported M2 version {v} (expected 256..=263)")
            }
            Error::Truncated => write!(f, "truncated M2"),
        }
    }
}
impl std::error::Error for Error {}
pub(crate) type Result<T> = std::result::Result<T, Error>;
