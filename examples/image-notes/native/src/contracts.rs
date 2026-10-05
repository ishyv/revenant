//! JSON contracts shared by operation adapters and native services.
//! Catalog IDs persist across refreshes; none of these types carry scoped file handles.
use revenant::contract;

/// Empty input for loading persisted catalog information.
#[contract]
pub struct Empty {}

/// One folder registered by the user. Paths are restored from the native catalog.
#[contract]
#[serde(rename_all = "camelCase")]
pub struct Root {
    /// Persistent catalog identity; unrelated to scoped file handles.
    pub id: String,
    /// Folder name shown in the sidebar.
    pub name: String,
    /// Canonical folder location, also used to explicitly refresh this root.
    pub path: String,
    /// Number of currently available images in this root.
    pub count: u32,
}

/// Startup state stored outside the checkout, shared between development and builds.
#[contract]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    /// Registered folders and their current image counts.
    pub roots: Vec<Root>,
    /// Native application data directory, for backup and troubleshooting.
    pub data_path: String,
}

/// Explicitly selected folder to add or refresh.
#[contract]
pub struct ScanInput {
    /// Location returned by the native folder picker or an existing root record.
    pub path: String,
}

/// Completed scan summary. Cancelled scans preserve existing annotations.
#[contract]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    /// Persistent root identity.
    pub root_id: String,
    /// Images visited during this scan.
    pub discovered: u32,
    /// Unreadable entries skipped; a nonzero value suppresses missing-file cleanup.
    pub warnings: u32,
}

/// Full-text query across names, relative paths and annotations.
#[contract]
#[serde(rename_all = "camelCase")]
pub struct QueryInput {
    /// Whitespace-separated terms; each is matched as a literal token prefix.
    pub search: String,
    /// One root identity, or an empty string for all roots.
    pub root_id: String,
    /// Whether to return only images with a nonblank annotation.
    pub annotated_only: bool,
    /// Zero-based result offset.
    pub offset: u32,
    /// Requested page size, clamped to 1..60.
    pub limit: u32,
}

/// One durable image record, without file bytes or native resource handles.
#[contract]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    /// Stable identity that survives refreshes at the same root-relative path.
    pub id: String,
    /// Root identity used for folder presentation.
    pub root_id: String,
    /// Original file name.
    pub name: String,
    /// File location relative to its registered root.
    pub relative_path: String,
    /// Plain text annotation, up to 16 KiB of UTF-8.
    pub note: String,
    /// Exact original byte size as a decimal string.
    pub size: String,
}

/// Bounded query results from the persistent catalog.
#[contract]
pub struct AssetPage {
    /// Only this requested page, never the complete image inventory.
    pub items: Vec<Asset>,
    /// Matching image count.
    pub total: u32,
    /// Zero-based page position.
    pub offset: u32,
}

/// Save one image annotation without acquiring its original file.
#[contract]
pub struct NoteInput {
    /// Persistent asset identity returned by the catalog query.
    pub id: String,
    /// Plain text annotation. Empty text clears the annotation.
    pub note: String,
}

/// Confirmation of an annotation committed to the native database.
#[contract]
pub struct SavedNote {
    /// Updated persistent asset identity.
    pub id: String,
    /// Exact committed annotation.
    pub note: String,
}

/// Request a cached thumbnail or a larger viewing rendition.
#[contract]
pub struct RenderInput {
    /// Persistent asset identity, resolved only within its stored root.
    pub id: String,
    /// False returns a 320-pixel thumbnail; true returns a 1440-pixel rendition.
    pub large: bool,
}

/// Encoded display rendition, bounded below the native contract byte limit.
#[contract]
#[serde(rename_all = "camelCase")]
pub struct Rendition {
    /// JPEG data URL with bounded bytes, independent of the native preview protocol.
    pub data_url: String,
    /// Display rendition width in pixels.
    pub width: u32,
    /// Display rendition height in pixels.
    pub height: u32,
}

/// Remove a folder from the catalog after explicit UI confirmation.
#[contract]
pub struct RootInput {
    /// Persistent root identity. Original files are never modified.
    pub id: String,
}

/// Acknowledgment of catalog removal.
#[contract]
pub struct Removed {
    /// Whether an existing catalog root was removed.
    pub removed: bool,
}
