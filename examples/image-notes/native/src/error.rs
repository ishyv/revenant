//! Translate native failures into the existing wire error codes.
use revenant::Error;

pub(crate) fn problem(code: &str, error: impl std::fmt::Display) -> Error {
    Error::new(code, error.to_string())
}
