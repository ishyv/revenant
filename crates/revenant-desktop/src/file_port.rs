//! Native file capabilities pin one open file at construction and read lazily.
//! Subsequent path replacement or symlink retargeting cannot redirect that open
//! handle. Size and Unix millisecond mtime are checked before and after each
//! bounded read; observed changes and premature EOF become structured errors.
//! Metadata alone cannot detect an edit restoring both size and mtime.
//!
//! The host must canonicalize the chosen root, reject symlink/reparse traversal,
//! and pass an admitted canonical path inside that root before calling new.
//! Noncanonical paths fail on read rather than resolving new symlink authority.
//! This API receives no root and cannot independently establish root authority.
//! Construction and polling the read future perform blocking native I/O: the
//! host must schedule both on its bounded blocking worker, never the UI thread.
//! No unbounded executor or per-read thread is spawned here.

use revenant_core::serde_json::json;
use revenant_core::{BoxFuture, Error, FileMetadata, Handle, ResourcePort, Result};
use std::{
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};

const MAX_CHUNK: u32 = 4 * 1024 * 1024;

/// Host-owned native file and captured metadata. The original path is retained
/// for native diagnostics only; it is never used to reopen between chunks.
pub struct FilePort {
    path: PathBuf,
    expected: FileMetadata,
    file: Mutex<Result<File>>,
}

impl FilePort {
    /// Pins the canonical file immediately without reading its contents. Because
    /// this constructor is infallible, resolve/open failures are retained and
    /// returned by reads. Pass only canonical paths admitted beneath a chosen
    /// root; a changed resolution returns `file_path_changed` without opening it.
    pub fn new(path: PathBuf, expected: FileMetadata) -> Arc<Self> {
        let file = (|| {
            let canonical = path.canonicalize().map_err(|e| io_error("resolve", e))?;
            if canonical != path {
                return Err(Error::new(
                    "file_path_changed",
                    "Host must admit the canonical file path beneath its chosen root",
                ));
            }
            let file = File::open(&canonical).map_err(|e| io_error("open", e))?;
            // Catch an observable retarget during opening. Once open, the handle
            // remains the authority even if its former name disappears.
            if path.canonicalize().map_err(|e| io_error("resolve", e))? != canonical {
                return Err(Error::new(
                    "file_changed",
                    "File path changed while opening",
                ));
            }
            verify(
                &file.metadata().map_err(|e| io_error("stat", e))?,
                &expected,
            )?;
            Ok(file)
        })();
        Arc::new(Self {
            path,
            expected,
            file: Mutex::new(file),
        })
    }

    fn read_chunk(&self, handle: &Handle, offset: u64, length: u32) -> Result<Vec<u8>> {
        if handle.kind != "file" {
            return Err(Error::new("wrong_kind", "Expected a file resource"));
        }
        if length == 0 || length > MAX_CHUNK {
            return Err(Error::new(
                "invalid_chunk",
                "File chunks must contain 1 byte through 4 MiB",
            ));
        }
        let size = self.expected.size.get();
        if offset > size {
            return Err(Error::new(
                "invalid_offset",
                "Read offset exceeds captured file size",
            ));
        }
        let mut state = self.file.lock().map_err(|_| {
            Error::new(
                "file_unavailable",
                "Native file synchronization was poisoned",
            )
        })?;
        let file = state.as_mut().map_err(|e| e.clone())?;
        verify(
            &file.metadata().map_err(|e| io_error("stat", e))?,
            &self.expected,
        )?;
        let length = (size - offset).min(u64::from(length)) as usize;
        if length == 0 {
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| io_error("seek", e))?;
        let mut bytes = vec![0; length];
        file.read_exact(&mut bytes).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                Error::new("file_unexpected_eof", "File ended before its captured size")
                    .details(json!({"offset": offset.to_string(), "length": length}))
            } else {
                io_error("read", e)
            }
        })?;
        verify(
            &file.metadata().map_err(|e| io_error("stat", e))?,
            &self.expected,
        )?;
        Ok(bytes)
    }
}

impl ResourcePort for FilePort {
    fn validate<'a>(&'a self, handle: &'a Handle) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if handle.kind != "file" {
                return Err(Error::new("wrong_kind", "Expected a file resource"));
            }
            let state = self.file.lock().map_err(|_| {
                Error::new(
                    "file_unavailable",
                    "Native file synchronization was poisoned",
                )
            })?;
            let file = state.as_ref().map_err(Clone::clone)?;
            verify(
                &file.metadata().map_err(|e| io_error("stat", e))?,
                &self.expected,
            )
        })
    }
    fn read<'a>(
        &'a self,
        handle: &'a Handle,
        offset: u64,
        length: u32,
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move { self.read_chunk(handle, offset, length) })
    }
}

impl std::fmt::Debug for FilePort {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilePort")
            .field("path", &self.path)
            .field("expected", &self.expected)
            .finish_non_exhaustive()
    }
}

fn verify(actual: &Metadata, expected: &FileMetadata) -> Result<()> {
    if !actual.is_file() || actual.len() != expected.size.get() {
        return Err(Error::new("file_changed", "File kind or size changed after enumeration")
            .details(json!({"expectedSize": expected.size.to_string(), "actualSize": actual.len().to_string()})));
    }
    if let Some(expected_modified) = expected.modified {
        let actual_modified = actual
            .modified()
            .map_err(|e| io_error("stat modification time", e))?;
        let millis: i128 = match actual_modified.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_millis() as i128,
            Err(error) => -(error.duration().as_millis() as i128),
        };
        if millis != i128::from(expected_modified.get()) {
            return Err(Error::new("file_changed", "File modification time changed after enumeration")
                .details(json!({"expectedModified": expected_modified.to_string(), "actualModified": millis.to_string()})));
        }
    }
    Ok(())
}

fn io_error(operation: &str, error: std::io::Error) -> Error {
    Error::new(
        "file_io",
        format!("Cannot {operation} native file: {error}"),
    )
    .details(json!({"operation": operation, "ioKind": format!("{:?}", error.kind())}))
}
