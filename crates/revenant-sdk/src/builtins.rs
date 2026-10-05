//! Built-in metadata uses the host's leased resource metadata, never input claims.
use crate::{
    Contract, DecimalI64, DecimalU64, FileEntry, Registry, Result, TaskContext, contract, operation,
};

/// Trusted native file metadata and its MIME-based preview category.
/// The metadata operation does not decode media; optional dimensions, duration,
/// and text metrics remain absent until an implementation actually measures them.
#[derive(Contract)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaMetadata {
    /// Host-captured display name, independent of caller-supplied FileEntry claims.
    pub name: String,
    /// Path relative to the native enumeration root; not native access authority.
    pub relative_path: String,
    /// Captured file size in bytes, encoded as a canonical decimal u64 string.
    pub size: DecimalU64,
    /// Host-supplied MIME type used to select the preview category.
    pub mime: String,
    /// Preview category inferred from MIME metadata, without decoding the contents.
    pub kind: MediaKind,
    /// Optional modification time in milliseconds since the Unix epoch,
    /// as a signed decimal string. Absent when the native host cannot supply it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<DecimalI64>,
    /// Optional decoded image/video width in pixels; this metadata-only operation omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Optional decoded image/video height in pixels; this metadata-only operation omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Optional media duration in seconds; this metadata-only operation omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Optional Unicode scalar-value count for decoded text; this operation omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_characters: Option<u32>,
}
/// MIME-based preview category. A category identifies a possible decoder,
/// not a guarantee that the file is valid or that the native viewer can decode it.
#[contract]
#[derive(Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    /// Text-like MIME types, including JSON and XML, suitable for bounded text previews.
    Text,
    /// An image MIME type; dimensions are unavailable until decoding occurs.
    Image,
    /// An audio MIME type, suitable for leased native media playback.
    Audio,
    /// A video MIME type, suitable for leased native media playback.
    Video,
    /// A MIME type without a supported preview category.
    Unsupported,
}

/// Reads trusted leased metadata and identifies a MIME-based preview category.
/// Ignores caller-supplied names and sizes. Returns cancellation or handle admission
/// errors from the task context; does not read or decode file contents.
#[operation(id = "media.readMetadata")]
pub async fn read_metadata(input: FileEntry, ctx: TaskContext) -> Result<MediaMetadata> {
    ctx.checkpoint()?;
    let metadata = ctx.file_metadata(&input.handle)?;
    let kind = if metadata.mime.starts_with("image/") {
        MediaKind::Image
    } else if metadata.mime.starts_with("audio/") {
        MediaKind::Audio
    } else if metadata.mime.starts_with("video/") {
        MediaKind::Video
    } else if metadata.mime.starts_with("text/")
        || matches!(
            metadata.mime.as_str(),
            "application/json" | "application/xml"
        )
    {
        MediaKind::Text
    } else {
        MediaKind::Unsupported
    };
    Ok(MediaMetadata {
        name: metadata.name,
        relative_path: metadata.relative_path,
        size: metadata.size,
        mime: metadata.mime,
        modified: metadata.modified,
        kind,
        width: None,
        height: None,
        duration: None,
        text_characters: None,
    })
}

/// Registers the built-in metadata operation in an advanced host registry.
/// Returns registration conflicts or compiled contract validation errors.
/// Application composition installs metadata and checksum by default; this helper
/// itself registers metadata only and does not acquire files.
pub fn register(registry: &Registry) -> Result<()> {
    registry.register(read_metadata_operation()?)
}

/// SHA-256 digest of the bytes successfully read from one admitted native file.
#[derive(Contract)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Checksum {
    /// Digest algorithm identifier; the built-in operation returns `sha256`.
    pub algorithm: String,
    /// Lowercase hexadecimal SHA-256 digest, containing 64 hexadecimal characters.
    pub digest: String,
    /// Number of bytes hashed, encoded as a decimal u64 string; equals captured size on success.
    pub bytes: DecimalU64,
}
/// Hashes a leased file with SHA-256 using reads of at most 256 KiB.
/// Application and native-host composition include this operation by default.
/// Reports byte progress and checks cancellation before each chunk and completion.
/// Propagates admission, native I/O, changed-file, and cancellation errors; an empty
/// file succeeds with the SHA-256 empty-input digest and zero bytes hashed.
#[operation(id = "files.checksum")]
pub async fn checksum(input: FileEntry, ctx: TaskContext) -> Result<Checksum> {
    use sha2::{Digest, Sha256};
    let metadata = ctx.file_metadata(&input.handle)?;
    // Empty files do not enter the byte loop, but native admission failures and
    // observed metadata changes must still produce honest per-item failures.
    ctx.validate_resource(&input.handle).await?;
    let mut hash = Sha256::new();
    let mut offset = 0;
    while offset < metadata.size.get() {
        ctx.checkpoint()?;
        let chunk = ctx.read(&input.handle, offset, 256 * 1024).await?;
        offset += chunk.len() as u64;
        hash.update(&chunk);
        ctx.progress(crate::Progress {
            completed: offset.into(),
            total: Some(metadata.size),
            message: Some(metadata.name.clone()),
        })?;
    }
    ctx.checkpoint()?;
    Ok(Checksum {
        algorithm: "sha256".into(),
        digest: format!("{:x}", hash.finalize()),
        bytes: offset.into(),
    })
}
