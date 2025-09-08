/// Utility functions that affect Rust's primitive types or standard library types.
    

use std::io;

/// Extension trait adding utility methods to `io::Result<T>`.
pub trait IoResultDialog {
    /// The success type carried by the io::Result.
    type Output;

    /// Modifies the error message of an `io::Result` by prepending a custom message.
    /// If the result is `Ok(_)`, it is returned unchanged.
    fn dialog(self, msg: &str) -> Self::Output;
}

impl<T> IoResultDialog for io::Result<T> {
    type Output = io::Result<T>;

    fn dialog(self, msg: &str) -> io::Result<T> {
        self.map_err(|e| io::Error::new(e.kind(), format!("{}: {}", msg, e)))
    }
}