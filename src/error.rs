use std::error::Error;
use std::fmt::{Display, Formatter};

pub type Result<T> = std::result::Result<T, Jp2LamError>;

#[derive(Debug)]
pub enum Jp2LamError {
    InvalidInput(String),
    EncodeFailed(String),
}

impl Display for Jp2LamError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            Self::EncodeFailed(msg) => write!(f, "encode failed: {msg}"),
        }
    }
}

impl Error for Jp2LamError {}
