//! Shared native SQLite primitives. Paths remain lossless native bytes in storage;
//! display strings belong in metadata and never serve as file access paths.

use revenant_core::{Error, Result};
use rusqlite::Connection;
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

pub(crate) struct Database {
    // Declaration order closes SQLite before removing the directory on Windows.
    pub connection: Connection,
    pub directory: tempfile::TempDir,
}

impl Database {
    pub fn new(cache_dir: &Path, prefix: &str) -> Result<Self> {
        let cache_dir = std::path::absolute(cache_dir).map_err(io_error)?;
        std::fs::create_dir_all(&cache_dir).map_err(io_error)?;
        let directory = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(&cache_dir)
            .map_err(io_error)?;
        let connection =
            Connection::open(directory.path().join("store.sqlite3")).map_err(sql_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(sql_error)?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON; PRAGMA temp_store=FILE; PRAGMA cache_size=-4096;",
            )
            .map_err(sql_error)?;
        Ok(Self {
            connection,
            directory,
        })
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| {
        Error::new(
            "store_poisoned",
            "Native store was interrupted while updating",
        )
    })
}

pub(crate) fn sql_error(error: rusqlite::Error) -> Error {
    Error::new("native_store", error.to_string())
}

pub(crate) fn io_error(error: std::io::Error) -> Error {
    Error::new("file_io", error.to_string())
}

pub(crate) fn page_limit(limit: u32) -> u32 {
    if limit == 0 { 128 } else { limit.min(512) }
}

pub(crate) fn sql_offset(offset: u64) -> Result<i64> {
    i64::try_from(offset)
        .map_err(|_| Error::new("invalid_offset", "Offset exceeds the native store range"))
}

#[cfg(windows)]
pub(crate) fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
pub(crate) fn decode_path(bytes: Vec<u8>) -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    if bytes.len() % 2 != 0 {
        return Err(Error::new(
            "invalid_path",
            "Stored native path is malformed",
        ));
    }
    let wide: Vec<_> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    Ok(std::ffi::OsString::from_wide(&wide).into())
}

#[cfg(unix)]
pub(crate) fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
pub(crate) fn decode_path(bytes: Vec<u8>) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Ok(std::ffi::OsString::from_vec(bytes).into())
}
