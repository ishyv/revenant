//! Filesystem checks shared by scanning and rendition resolution.
use crate::error::problem;
use revenant::Result;
use std::{fs, path::Path, time::UNIX_EPOCH};

pub(crate) fn modified(metadata: &fs::Metadata) -> String {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_else(|| "0".into())
}

pub(crate) fn is_link(path: &Path) -> Result<bool> {
    let metadata = fs::symlink_metadata(path).map_err(|e| problem("image_unavailable", e))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0)
    }
    #[cfg(not(windows))]
    {
        Ok(metadata.file_type().is_symlink())
    }
}
