//! Durable identities, full-text notes and bounded cached image renditions.
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{ImageDecoder, ImageReader, codecs::jpeg::JpegEncoder};
use revenant::{Error, Progress, Result, TaskContext, contract, operation};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, UNIX_EPOCH},
};

const MAX_NOTE_BYTES: usize = 16 * 1024;
const MAX_ENCODED_IMAGE: usize = 700 * 1024;
static SCANNER: OnceLock<Mutex<()>> = OnceLock::new();

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

fn problem(code: &str, error: impl std::fmt::Display) -> Error {
    Error::new(code, error.to_string())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| problem("catalog_worker", error))?
}

/// Load registered folders without re-enumerating the filesystem.
#[operation(id = "images.bootstrap")]
pub async fn bootstrap(_input: Empty, context: TaskContext) -> Result<Bootstrap> {
    blocking(move || {
        context.checkpoint()?;
        Catalog::open()?.bootstrap()
    })
    .await
}

/// Index a chosen folder recursively, preserving notes and yielding scan progress.
/// Files are read only for metadata; image decoding happens on demand.
#[operation(id = "images.scan")]
pub async fn scan(input: ScanInput, context: TaskContext) -> Result<ScanReport> {
    blocking(move || {
        let _guard = SCANNER
            .get_or_init(|| Mutex::new(()))
            .try_lock()
            .map_err(|_| Error::new("scan_busy", "Another folder scan is still running"))?;
        context.checkpoint()?;
        Catalog::open()?.scan(&input.path, &context)
    })
    .await
}

/// Query persistent image names and notes using SQLite FTS5, with bounded pages.
#[operation(id = "images.query")]
pub async fn query(input: QueryInput, context: TaskContext) -> Result<AssetPage> {
    blocking(move || {
        context.checkpoint()?;
        Catalog::open()?.query(input)
    })
    .await
}

/// Atomically save a note and update its full-text index.
#[operation(id = "images.annotate")]
pub async fn annotate(input: NoteInput, context: TaskContext) -> Result<SavedNote> {
    blocking(move || {
        context.checkpoint()?;
        Catalog::open()?.annotate(input)
    })
    .await
}

/// Decode an image on a blocking worker and cache a bounded JPEG rendition.
#[operation(id = "images.render")]
pub async fn render(input: RenderInput, context: TaskContext) -> Result<Rendition> {
    blocking(move || {
        context.checkpoint()?;
        let catalog = Catalog::open()?;
        catalog.render(&input, &context)
    })
    .await
}

/// Remove the root and its annotations from the catalog, leaving original files intact.
#[operation(id = "images.removeRoot")]
pub async fn remove_root(input: RootInput, context: TaskContext) -> Result<Removed> {
    blocking(move || {
        context.checkpoint()?;
        let catalog = Catalog::open()?;
        let removed = catalog
            .db
            .execute("DELETE FROM roots WHERE id=?1", [input.id])
            .map_err(|e| problem("catalog_write", e))?;
        Ok(Removed {
            removed: removed > 0,
        })
    })
    .await
}

struct Catalog {
    db: Connection,
    directory: PathBuf,
}

impl Catalog {
    fn open() -> Result<Self> {
        let base = dirs::data_local_dir().ok_or_else(|| {
            Error::new(
                "catalog_location",
                "No local application data directory is available",
            )
        })?;
        Self::open_at(&base.join("app.revenant.image-notes"))
    }

    fn open_at(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory).map_err(|e| problem("catalog_io", e))?;
        let db = Connection::open(directory.join("catalog.sqlite3"))
            .map_err(|e| problem("catalog_open", e))?;
        db.busy_timeout(Duration::from_secs(5))
            .map_err(|e| problem("catalog_open", e))?;
        db.execute_batch(
            "PRAGMA foreign_keys=ON;
             PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS roots(id TEXT PRIMARY KEY,path TEXT NOT NULL UNIQUE,name TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS assets(
               rowid INTEGER PRIMARY KEY,id TEXT NOT NULL UNIQUE,
               root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
               relative_path TEXT NOT NULL,name TEXT NOT NULL,note TEXT NOT NULL DEFAULT '',
               size TEXT NOT NULL,modified TEXT NOT NULL,seen TEXT NOT NULL,
               available INTEGER NOT NULL DEFAULT 1,UNIQUE(root_id,relative_path));
             CREATE INDEX IF NOT EXISTS assets_root ON assets(root_id,available,name);
             CREATE VIRTUAL TABLE IF NOT EXISTS asset_search USING fts5(
               name,relative_path,note,content='assets',content_rowid='rowid',
               tokenize='unicode61 remove_diacritics 2');
             CREATE TRIGGER IF NOT EXISTS assets_insert AFTER INSERT ON assets BEGIN
               INSERT INTO asset_search(rowid,name,relative_path,note)
               VALUES(new.rowid,new.name,new.relative_path,new.note); END;
             CREATE TRIGGER IF NOT EXISTS assets_delete AFTER DELETE ON assets BEGIN
               INSERT INTO asset_search(asset_search,rowid,name,relative_path,note)
               VALUES('delete',old.rowid,old.name,old.relative_path,old.note); END;
             CREATE TRIGGER IF NOT EXISTS assets_update AFTER UPDATE ON assets BEGIN
               INSERT INTO asset_search(asset_search,rowid,name,relative_path,note)
               VALUES('delete',old.rowid,old.name,old.relative_path,old.note);
               INSERT INTO asset_search(rowid,name,relative_path,note)
               VALUES(new.rowid,new.name,new.relative_path,new.note); END;",
        )
        .map_err(|e| problem("catalog_schema", e))?;
        Ok(Self {
            db,
            directory: directory.to_owned(),
        })
    }

    fn bootstrap(&self) -> Result<Bootstrap> {
        let mut statement = self
            .db
            .prepare(
                "SELECT r.id,r.name,r.path,count(a.rowid) FROM roots r
             LEFT JOIN assets a ON a.root_id=r.id AND a.available=1
             GROUP BY r.id ORDER BY r.name COLLATE NOCASE,r.id",
            )
            .map_err(|e| problem("catalog_query", e))?;
        let roots = statement
            .query_map([], |row| {
                Ok(Root {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    count: count(row.get::<_, i64>(3)?),
                })
            })
            .map_err(|e| problem("catalog_query", e))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| problem("catalog_query", e))?;
        Ok(Bootstrap {
            roots,
            data_path: self.directory.to_string_lossy().into_owned(),
        })
    }

    fn scan(&mut self, selected: &str, context: &TaskContext) -> Result<ScanReport> {
        let selected = PathBuf::from(selected);
        if !selected.is_dir() || is_link(&selected)? {
            return Err(Error::new(
                "invalid_folder",
                "Choose a real folder, not a symbolic link or junction",
            ));
        }
        let root = fs::canonicalize(selected).map_err(|e| problem("folder_unavailable", e))?;
        let path = root.to_string_lossy().into_owned();
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let existing: Option<String> = self
            .db
            .query_row("SELECT id FROM roots WHERE path=?1", [&path], |r| r.get(0))
            .optional()
            .map_err(|e| problem("catalog_query", e))?;
        let id = existing.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        self.db.execute("INSERT INTO roots(id,path,name) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET name=excluded.name",params![id,path,name])
            .map_err(|e|problem("catalog_write",e))?;
        let epoch = uuid::Uuid::new_v4().to_string();
        let cache = fs::canonicalize(&self.directory).unwrap_or_else(|_| self.directory.clone());
        let mut discovered = 0u32;
        let mut warnings = 0u32;
        let mut batch = Vec::with_capacity(128);
        let walker = walkdir::WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                !entry.path().starts_with(&cache)
                    && !entry.file_type().is_symlink()
                    && (!entry.file_type().is_dir() || !is_link(entry.path()).unwrap_or(true))
            });
        for entry in walker {
            context.checkpoint()?;
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    warnings = warnings.saturating_add(1);
                    continue;
                }
            };
            if !entry.file_type().is_file() || !supported(entry.path()) {
                continue;
            }
            if is_link(entry.path())? {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(value) => value,
                Err(_) => {
                    warnings = warnings.saturating_add(1);
                    continue;
                }
            };
            let relative = entry
                .path()
                .strip_prefix(&root)
                .map_err(|e| problem("catalog_path", e))?
                .to_string_lossy()
                .into_owned();
            let name = entry.file_name().to_string_lossy().into_owned();
            batch.push((
                relative,
                name,
                metadata.len().to_string(),
                modified(&metadata),
            ));
            discovered = discovered.checked_add(1).ok_or_else(|| {
                Error::new("catalog_limit", "Image count exceeds this example's limit")
            })?;
            if batch.len() == 128 {
                self.insert_batch(&id, &epoch, &batch)?;
                batch.clear();
                context.progress(Progress {
                    completed: u64::from(discovered).into(),
                    total: None,
                    message: Some(format!("{discovered} imágenes encontradas")),
                })?;
            }
        }
        context.checkpoint()?;
        self.insert_batch(&id, &epoch, &batch)?;
        if warnings == 0 {
            self.db
                .execute(
                    "UPDATE assets SET available=0 WHERE root_id=?1 AND seen<>?2",
                    params![id, epoch],
                )
                .map_err(|e| problem("catalog_write", e))?;
        }
        context.progress(Progress {
            completed: u64::from(discovered).into(),
            total: Some(u64::from(discovered).into()),
            message: Some("Carpeta actualizada".into()),
        })?;
        Ok(ScanReport {
            root_id: id,
            discovered,
            warnings,
        })
    }

    fn insert_batch(
        &mut self,
        root: &str,
        epoch: &str,
        batch: &[(String, String, String, String)],
    ) -> Result<()> {
        let transaction = self
            .db
            .transaction()
            .map_err(|e| problem("catalog_write", e))?;
        {
            let mut statement = transaction
                .prepare_cached(
                    "INSERT INTO assets(id,root_id,relative_path,name,size,modified,seen)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(root_id,relative_path) DO UPDATE SET name=excluded.name,
                 size=excluded.size,modified=excluded.modified,seen=excluded.seen,available=1",
                )
                .map_err(|e| problem("catalog_write", e))?;
            for (relative, name, size, modified) in batch {
                statement
                    .execute(params![
                        uuid::Uuid::new_v4().to_string(),
                        root,
                        relative,
                        name,
                        size,
                        modified,
                        epoch
                    ])
                    .map_err(|e| problem("catalog_write", e))?;
            }
        }
        transaction
            .commit()
            .map_err(|e| problem("catalog_write", e))
    }

    fn query(&self, input: QueryInput) -> Result<AssetPage> {
        if input.search.len() > 1024 {
            return Err(Error::new(
                "query_too_long",
                "Search is limited to 1024 UTF-8 bytes",
            ));
        }
        let search = search_expression(&input.search);
        let full_text = if search.is_empty() {
            "1"
        } else {
            "a.rowid IN (SELECT rowid FROM asset_search WHERE asset_search MATCH ?1)"
        };
        let predicate = format!(
            "a.available=1 AND {full_text} AND (?2='' OR a.root_id=?2) AND (?3=0 OR length(trim(a.note))>0)"
        );
        // Include ?1 even for an empty query so positional arguments remain stable.
        let total: i64 = self
            .db
            .query_row(
                &format!("SELECT count(*) FROM assets a WHERE (?1 IS NOT NULL) AND {predicate}"),
                params![search, input.root_id, input.annotated_only],
                |r| r.get(0),
            )
            .map_err(|e| problem("catalog_query", e))?;
        let mut statement = self.db.prepare(&format!(
            "SELECT a.id,a.root_id,a.name,a.relative_path,a.note,a.size FROM assets a
             WHERE (?1 IS NOT NULL) AND {predicate} ORDER BY a.name COLLATE NOCASE,a.relative_path,a.id LIMIT ?4 OFFSET ?5"
        )).map_err(|e|problem("catalog_query",e))?;
        let items = statement
            .query_map(
                params![
                    search,
                    input.root_id,
                    input.annotated_only,
                    input.limit.clamp(1, 60),
                    input.offset
                ],
                |r| {
                    Ok(Asset {
                        id: r.get(0)?,
                        root_id: r.get(1)?,
                        name: r.get(2)?,
                        relative_path: r.get(3)?,
                        note: r.get(4)?,
                        size: r.get(5)?,
                    })
                },
            )
            .map_err(|e| problem("catalog_query", e))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| problem("catalog_query", e))?;
        Ok(AssetPage {
            items,
            total: count(total),
            offset: input.offset,
        })
    }

    fn annotate(&self, input: NoteInput) -> Result<SavedNote> {
        if input.note.len() > MAX_NOTE_BYTES {
            return Err(Error::new(
                "note_too_large",
                "Annotations are limited to 16 KiB of UTF-8",
            ));
        }
        let changed = self
            .db
            .execute(
                "UPDATE assets SET note=?2 WHERE id=?1",
                params![input.id, input.note],
            )
            .map_err(|e| problem("catalog_write", e))?;
        if changed == 0 {
            return Err(Error::new(
                "asset_missing",
                "This image is no longer in the catalog",
            ));
        }
        Ok(SavedNote {
            id: input.id,
            note: input.note,
        })
    }

    fn render(&self, input: &RenderInput, context: &TaskContext) -> Result<Rendition> {
        let record:Option<(String,String)> = self.db.query_row(
            "SELECT r.path,a.relative_path FROM assets a JOIN roots r ON r.id=a.root_id WHERE a.id=?1 AND a.available=1",
            [&input.id],|r|Ok((r.get(0)?,r.get(1)?)),
        ).optional().map_err(|e|problem("catalog_query",e))?;
        let (root, relative) = record
            .ok_or_else(|| Error::new("asset_missing", "This image is no longer available"))?;
        let root = fs::canonicalize(root).map_err(|e| problem("folder_unavailable", e))?;
        let unresolved = root.join(relative);
        if is_link(&unresolved)? {
            return Err(Error::new(
                "image_unavailable",
                "Image was replaced by a link",
            ));
        }
        let path = fs::canonicalize(unresolved).map_err(|e| problem("image_unavailable", e))?;
        if !path.starts_with(&root) {
            return Err(Error::new(
                "image_unavailable",
                "Image is outside its registered folder",
            ));
        }
        let metadata = fs::metadata(&path).map_err(|e| problem("image_unavailable", e))?;
        let bound = if input.large { 1440 } else { 320 };
        let key = format!(
            "{}-{}-{}-{bound}.jpg",
            input.id,
            metadata.len(),
            modified(&metadata)
        );
        let directory = self.directory.join("renditions");
        fs::create_dir_all(&directory).map_err(|e| problem("cache_io", e))?;
        let cached = directory.join(key);
        context.checkpoint()?;
        let bytes = if cached.is_file() {
            fs::read(&cached).map_err(|e| problem("cache_io", e))?
        } else {
            let mut reader = ImageReader::open(&path)
                .map_err(|e| problem("image_decode", e))?
                .with_guessed_format()
                .map_err(|e| problem("image_decode", e))?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(20_000);
            limits.max_image_height = Some(20_000);
            limits.max_alloc = Some(256 * 1024 * 1024);
            reader.limits(limits);
            let mut decoder = reader
                .into_decoder()
                .map_err(|e| problem("image_decode", e))?;
            let orientation = decoder
                .orientation()
                .map_err(|e| problem("image_decode", e))?;
            let mut decoded = image::DynamicImage::from_decoder(decoder)
                .map_err(|e| problem("image_decode", e))?;
            decoded.apply_orientation(orientation);
            let rgba = decoded.thumbnail(bound, bound).to_rgba8();
            drop(decoded);
            let mut image = image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
                let pixel = rgba.get_pixel(x, y);
                let alpha = u32::from(pixel[3]);
                image::Rgb(
                    [0, 1, 2]
                        .map(|i| ((u32::from(pixel[i]) * alpha + 255 * (255 - alpha)) / 255) as u8),
                )
            });
            drop(rgba);
            context.checkpoint()?;
            let mut bytes = Vec::new();
            JpegEncoder::new_with_quality(&mut bytes, if input.large { 82 } else { 76 })
                .encode_image(&image)
                .map_err(|e| problem("image_encode", e))?;
            let mut fallback = 1024;
            while bytes.len() > MAX_ENCODED_IMAGE && fallback >= 256 {
                image = image::imageops::thumbnail(&image, fallback, fallback);
                bytes.clear();
                JpegEncoder::new_with_quality(&mut bytes, 65)
                    .encode_image(&image)
                    .map_err(|e| problem("image_encode", e))?;
                fallback /= 2;
            }
            if bytes.len() > MAX_ENCODED_IMAGE {
                return Err(Error::new(
                    "image_too_large",
                    "Rendition exceeds its bounded byte budget",
                ));
            }
            context.checkpoint()?;
            let temporary = directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
            fs::write(&temporary, &bytes).map_err(|e| problem("cache_io", e))?;
            // Another request may have already produced the identical rendition.
            if fs::rename(&temporary, &cached).is_err() {
                let _ = fs::remove_file(&temporary);
            }
            bytes
        };
        if bytes.len() > MAX_ENCODED_IMAGE {
            return Err(Error::new(
                "cache_invalid",
                "Cached rendition exceeds its budget",
            ));
        }
        let dimensions =
            image::load_from_memory(&bytes).map_err(|e| problem("cache_invalid", e))?;
        context.checkpoint()?;
        Ok(Rendition {
            data_url: format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)),
            width: dimensions.width(),
            height: dimensions.height(),
        })
    }
}

fn count(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
fn modified(metadata: &fs::Metadata) -> String {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_else(|| "0".into())
}
fn supported(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp"
        )
    })
}
fn is_link(path: &Path) -> Result<bool> {
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
fn search_expression(query: &str) -> String {
    query
        .split_whitespace()
        .take(16)
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> (TaskContext, revenant::TaskExecution) {
        let resources = revenant::ResourceRegistry::new();
        resources.create_scope("test", None).unwrap();
        let context = revenant::TaskManager::new(resources)
            .create("test", "work")
            .unwrap();
        let execution = context.begin().unwrap();
        (context, execution)
    }

    fn fixture() -> (tempfile::TempDir, Catalog, String) {
        let directory = tempfile::tempdir().unwrap();
        let mut catalog = Catalog::open_at(directory.path()).unwrap();
        catalog
            .db
            .execute(
                "INSERT INTO roots(id,path,name) VALUES('root','/photos','Photos')",
                [],
            )
            .unwrap();
        catalog
            .insert_batch(
                "root",
                "first",
                &[(
                    "photo.jpg".into(),
                    "photo.jpg".into(),
                    "42".into(),
                    "1".into(),
                )],
            )
            .unwrap();
        let id = catalog
            .db
            .query_row("SELECT id FROM assets", [], |r| r.get::<_, String>(0))
            .unwrap();
        (directory, catalog, id)
    }
    fn request(search: &str) -> QueryInput {
        QueryInput {
            search: search.into(),
            root_id: String::new(),
            annotated_only: false,
            offset: 0,
            limit: 60,
        }
    }
    #[test]
    fn notes_survive_reopen_refresh_and_accent_insensitive_search() {
        let (directory, mut catalog, id) = fixture();
        catalog
            .annotate(NoteInput {
                id: id.clone(),
                note: "Atardecer en Málaga".into(),
            })
            .unwrap();
        catalog
            .insert_batch(
                "root",
                "second",
                &[(
                    "photo.jpg".into(),
                    "photo.jpg".into(),
                    "64".into(),
                    "2".into(),
                )],
            )
            .unwrap();
        drop(catalog);
        let catalog = Catalog::open_at(directory.path()).unwrap();
        let result = catalog.query(request("malag atardec")).unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.items[0].id, id);
        assert_eq!(result.items[0].size, "64");
        assert_eq!(result.items[0].note, "Atardecer en Málaga");
    }
    #[test]
    fn notes_update_search_and_pages_remain_bounded() {
        let (_directory, mut catalog, id) = fixture();
        let rows: Vec<_> = (0..140)
            .map(|i| {
                (
                    format!("{i}.png"),
                    format!("{i}.png"),
                    "1".into(),
                    "1".into(),
                )
            })
            .collect();
        catalog.insert_batch("root", "first", &rows).unwrap();
        let mut input = request("");
        input.limit = 1000;
        let page = catalog.query(input).unwrap();
        assert_eq!(page.total, 141);
        assert_eq!(page.items.len(), 60);
        catalog
            .annotate(NoteInput {
                id: id.clone(),
                note: "playa".into(),
            })
            .unwrap();
        assert_eq!(catalog.query(request("play")).unwrap().total, 1);
        catalog
            .annotate(NoteInput {
                id,
                note: "montaña".into(),
            })
            .unwrap();
        assert_eq!(catalog.query(request("play")).unwrap().total, 0);
        assert_eq!(catalog.query(request("montana")).unwrap().total, 1);
    }
    #[test]
    fn literal_search_and_oversized_notes_are_handled() {
        let (_directory, catalog, id) = fixture();
        catalog.query(request("\" OR *")).unwrap();
        assert!(
            catalog
                .annotate(NoteInput {
                    id,
                    note: "x".repeat(MAX_NOTE_BYTES + 1)
                })
                .is_err()
        );
    }

    #[test]
    fn real_folder_refresh_keeps_notes_and_renders_large_originals() {
        let directory = tempfile::tempdir().unwrap();
        let photos = directory.path().join("photos");
        fs::create_dir(&photos).unwrap();
        let mut seed = 42u32;
        let pixels = image::RgbImage::from_fn(1500, 1500, |_, _| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8])
        });
        let original = photos.join("large.png");
        pixels.save(&original).unwrap();
        assert!(fs::metadata(&original).unwrap().len() > 4 * 1024 * 1024);
        let mut catalog = Catalog::open_at(&directory.path().join("data")).unwrap();
        let (context, _execution) = running();
        let report = catalog.scan(photos.to_str().unwrap(), &context).unwrap();
        assert_eq!(report.discovered, 1);
        let asset = catalog.query(request("")).unwrap().items.remove(0);
        catalog
            .annotate(NoteInput {
                id: asset.id.clone(),
                note: "Una fotografía grande".into(),
            })
            .unwrap();
        let rendition = catalog
            .render(
                &RenderInput {
                    id: asset.id.clone(),
                    large: true,
                },
                &context,
            )
            .unwrap();
        assert!(rendition.data_url.len() < 1024 * 1024);
        assert!(rendition.width <= 1440 && rendition.height <= 1440);
        let again = catalog
            .render(
                &RenderInput {
                    id: asset.id.clone(),
                    large: true,
                },
                &context,
            )
            .unwrap();
        assert_eq!(rendition.data_url, again.data_url);
        let (refresh, _refresh_execution) = running();
        catalog.scan(photos.to_str().unwrap(), &refresh).unwrap();
        assert_eq!(
            catalog.query(request("fotografia")).unwrap().items[0].id,
            asset.id
        );
        fs::remove_file(&original).unwrap();
        let (refresh, _refresh_execution) = running();
        catalog.scan(photos.to_str().unwrap(), &refresh).unwrap();
        assert_eq!(catalog.query(request("")).unwrap().total, 0);
        let note: String = catalog
            .db
            .query_row("SELECT note FROM assets WHERE id=?1", [asset.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(note, "Una fotografía grande");
    }
}
