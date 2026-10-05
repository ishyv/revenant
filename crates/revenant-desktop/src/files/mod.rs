//! Native folder metadata index, with bounded pages and frozen batch selections.
//!
//! Construction only validates the root and initializes a temporary SQLite store.
//! The host schedules [`FolderIndex::scan`] on its bounded blocking executor and
//! supplies cancellation. File contents stay unopened; no worker threads or
//! resource leases are allocated here. Each committed scan chunk advances generation;
//! clients must restart offset paging when that generation changes.
//!
//! Traversal includes hidden files and ignores no `.gitignore` rules. Symlinks
//! and Windows reparse points (including junctions) are skipped. A SQLite directory
//! frontier drives shallow walks, keeping traversal breadth on disk. The store's own
//! temporary directory is excluded if it lives under the chosen root. Metadata
//! is a point-in-time observation, not a guarantee that a file still exists.
//! Hosts must validate native access and attach leases when turning records into
//! resource entries; native paths must not be serialized to the webview.
//!
//! A ready index can create a [`FileSelection`]. SQLite freezes matching IDs in
//! query order, and the iterator buffers at most 128 records. A rescan can proceed
//! while that selection retains its old records. Dropping the selection releases
//! them; dropping the final index owner closes SQLite and deletes its temp data.

#![doc = include_str!("README.md")]

pub(crate) mod storage;

use ignore::WalkBuilder;
use revenant_core::{
    Error, FileMetadata, Result,
    serde::{Deserialize, Serialize},
};
use rusqlite::{OptionalExtension, params};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::UNIX_EPOCH,
};
use storage::{
    Database, decode_path, encode_path, io_error, lock, page_limit, sql_error, sql_offset,
};

const INSERT_CHUNK: usize = 256;
const WARNING_LIMIT: usize = 32;
const COUNT_CACHE_LIMIT: usize = 16;
const COUNT_CACHE_SEARCH_BYTES: usize = 1024;
const FILE_COLUMNS: &str = "f.id,f.path,f.metadata";
const MATCH: &str = "(instr(f.name_fold,?2)>0 OR instr(f.relative_fold,?2)>0)";

/// Case-insensitive literal substring matching of name and relative path.
/// `sort` accepts `name` (default), `size`, `modified`, or `path`; invalid values
/// return `invalid_query`. Recursion belongs to the provider, not this query.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(
    crate = "revenant_core::serde",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub struct FileQuery {
    /// Optional literal substring of a file name or relative path.
    pub search: Option<String>,
    /// Primary ordering: name, size, modified, or path; omitted means name.
    pub sort: Option<String>,
    /// Reverses only the primary ordering; ties always sort by ascending path.
    #[serde(default)]
    pub descending: bool,
}

impl FileQuery {
    fn search(&self) -> String {
        self.search.as_deref().unwrap_or("").to_lowercase()
    }
    fn order(&self) -> Result<String> {
        let column = match self.sort.as_deref().unwrap_or("name") {
            "name" => "f.name_fold",
            "path" => "f.relative_fold",
            "size" => "f.size_key",
            "modified" => "f.modified",
            _ => {
                return Err(Error::new(
                    "invalid_query",
                    "Sort must be name, size, modified, or path",
                ));
            }
        };
        let direction = if self.descending { "DESC" } else { "ASC" };
        // Exact native path bytes and monotonically issued IDs break display ties.
        Ok(format!(
            "{column} {direction},f.relative_fold ASC,f.path ASC,f.id ASC"
        ))
    }
}

/// Native-only metadata match. This intentionally does not implement Serialize.
#[derive(Clone, Debug)]
pub struct IndexedFile {
    /// Monotonic provider-local native ID, not a resource handle.
    pub id: i64,
    /// Lossless absolute native access path; never expose it as wire metadata.
    pub path: PathBuf,
    /// Scan-time metadata with Unix millisecond modification time, when known.
    pub metadata: FileMetadata,
}

/// Scanner lifecycle. Cancelled/failed inventories remain queryable but cannot
/// be selected for a batch; a subsequent scan starts a new inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "revenant_core::serde", rename_all = "camelCase")]
pub enum IndexState {
    /// Store initialized; scan not yet scheduled or started.
    Pending,
    /// A blocking scan is publishing partial metadata chunks.
    Scanning,
    /// Traversal completed; warning-bearing results are still selectable.
    Ready,
    /// Host cancellation was acknowledged; inventory remains partial.
    Cancelled,
    /// Fatal store/path failure interrupted traversal.
    Failed,
}

/// A bounded display warning. Native paths use lossy display only in diagnostics.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "revenant_core::serde", rename_all = "camelCase")]
pub struct IndexWarning {
    /// Bounded native display path, for diagnostics only.
    pub path: String,
    /// Diagnostic truncated to at most 1024 Unicode scalar values.
    pub message: String,
}

/// `total` counts all warnings, even after the 32 retained examples are full.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(crate = "revenant_core::serde", rename_all = "camelCase")]
pub struct IndexWarnings {
    /// Total encountered warnings, including omitted examples.
    pub total: u64,
    /// First 32 warning examples, each with bounded path/message text.
    pub items: Vec<IndexWarning>,
}

impl IndexWarnings {
    fn push(&mut self, path: &Path, message: impl std::fmt::Display) {
        self.total = self.total.saturating_add(1);
        if self.items.len() < WARNING_LIMIT {
            self.items.push(IndexWarning {
                path: path.to_string_lossy().chars().take(1024).collect(),
                message: message.to_string().chars().take(1024).collect(),
            });
        }
    }
}

/// A coherent status for the inventory revision published alongside committed rows.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "revenant_core::serde", rename_all = "camelCase")]
pub struct IndexStatus {
    /// Number of regular file records committed in the current scan.
    pub discovered: u64,
    /// Current scanner lifecycle state.
    pub state: IndexState,
    /// Revision used to invalidate offset pages, independent of resource handles.
    pub generation: u64,
    /// Bounded warning examples and complete warning count.
    pub warnings: IndexWarnings,
}

/// At most 512 native metadata matches; `limit=0` requests the default 128.
/// `total` is the matching count; `discovered` counts all committed file records.
#[derive(Clone, Debug)]
pub struct QueryPage {
    /// At most 512 metadata matches, with native IDs and no leased handles.
    pub items: Vec<IndexedFile>,
    /// Requested zero-based position within the matching query.
    pub offset: u64,
    /// Count of matches in the same revision as this page.
    pub total: u64,
    /// All committed files, including those outside the query.
    pub discovered: u64,
    /// Lifecycle state for this page's inventory revision.
    pub state: IndexState,
    /// Revision; restart offset pagination if a later page differs.
    pub generation: u64,
    /// Complete warning count with at most 32 examples.
    pub warnings: IndexWarnings,
}

struct Inner {
    db: Database,
    status: IndexStatus,
    epoch: i64,
    next_selection: i64,
    directory_cursor: i64,
    // Counts depend on the search and committed inventory, never the sort.
    // Bound both retained keys and their bytes; long queries are not cached.
    counts: VecDeque<(String, u64)>,
}

/// Shared native provider. One scan may run at a time; queries interleave between
/// bounded insert transactions. SQLite owns the inventory rather than a Rust Vec.
pub struct FolderIndex {
    root: PathBuf,
    recursive: bool,
    scanning: AtomicBool,
    inner: Mutex<Inner>,
}

impl FolderIndex {
    /// Fast setup; performs no enumeration. The selected root must itself be a
    /// real directory, not a symlink or reparse point. The cache is temporary.
    pub fn new(root: PathBuf, cache_dir: &Path, recursive: bool) -> Result<Arc<Self>> {
        let root = std::path::absolute(root).map_err(io_error)?;
        let metadata = std::fs::symlink_metadata(&root).map_err(io_error)?;
        if !metadata.is_dir() || is_link(&metadata) {
            return Err(Error::new(
                "invalid_folder",
                "Root must be a directory without a symlink or reparse point",
            ));
        }
        let db = Database::new(cache_dir, "revenant-index-")?;
        db.connection.execute_batch(
            "CREATE TABLE files(id INTEGER PRIMARY KEY AUTOINCREMENT,epoch INTEGER NOT NULL,path BLOB NOT NULL,
             metadata TEXT NOT NULL,name_fold TEXT NOT NULL,relative_fold TEXT NOT NULL,size_key TEXT NOT NULL,modified INTEGER);
             CREATE INDEX files_epoch ON files(epoch);
             CREATE INDEX files_name ON files(epoch,name_fold,relative_fold,path,id);
             CREATE INDEX files_path ON files(epoch,relative_fold,path,id);
             CREATE INDEX files_size ON files(epoch,size_key,relative_fold,path,id);
             CREATE INDEX files_modified ON files(epoch,modified,relative_fold,path,id);
             CREATE TABLE pending_dirs(id INTEGER PRIMARY KEY AUTOINCREMENT,path BLOB NOT NULL);
             CREATE TABLE selections(id INTEGER PRIMARY KEY);
             CREATE TABLE selected(selection INTEGER NOT NULL REFERENCES selections(id) ON DELETE CASCADE,
             ordinal INTEGER NOT NULL,file INTEGER NOT NULL REFERENCES files(id),PRIMARY KEY(selection,ordinal)) WITHOUT ROWID;
             CREATE INDEX selected_file ON selected(file);"
        ).map_err(sql_error)?;
        Ok(Arc::new(Self {
            root,
            recursive,
            scanning: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                db,
                epoch: 0,
                next_selection: 0,
                directory_cursor: 0,
                counts: VecDeque::new(),
                status: IndexStatus {
                    discovered: 0,
                    state: IndexState::Pending,
                    generation: 0,
                    warnings: IndexWarnings::default(),
                },
            }),
        }))
    }

    /// Blocking traversal for the host's bounded scheduler. Notifications run
    /// without database locks and may call `query`. Concurrent scans are rejected.
    /// Cancellation commits the partial bounded chunk and returns `cancelled`.
    /// Files that disappear or deny metadata access add warnings and are skipped.
    pub fn scan(&self, cancel: &AtomicBool, notify: impl Fn(IndexStatus)) -> Result<IndexStatus> {
        if self
            .scanning
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error::new(
                "scan_running",
                "A folder scan is already running",
            ));
        }
        struct ScanGuard<'a>(&'a FolderIndex);
        impl Drop for ScanGuard<'_> {
            fn drop(&mut self) {
                if let Ok(mut inner) = self.0.inner.lock() {
                    if inner.status.state == IndexState::Scanning {
                        inner.status.state = IndexState::Failed;
                        inner.status.generation = inner.status.generation.saturating_add(1);
                        inner.counts.clear();
                        let _ = inner.db.connection.execute("DELETE FROM pending_dirs", []);
                    }
                }
                self.0.scanning.store(false, Ordering::Release);
            }
        }
        let _guard = ScanGuard(self);
        let result = self.scan_inner(cancel, &notify);
        if let Err(error) = &result {
            if !error.is_cancelled() {
                // Publish fatal failure after rolling back any failed chunk.
                // The notification may query again, so release the lock first.
                let status = {
                    let mut inner = lock(&self.inner)?;
                    inner.status.state = IndexState::Failed;
                    inner.status.generation = inner.status.generation.saturating_add(1);
                    inner.status.warnings.push(&self.root, error);
                    inner.counts.clear();
                    let _ = inner.db.connection.execute("DELETE FROM pending_dirs", []);
                    inner.status.clone()
                };
                notify(status);
            }
        }
        result
    }

    fn scan_inner(
        &self,
        cancel: &AtomicBool,
        notify: &impl Fn(IndexStatus),
    ) -> Result<IndexStatus> {
        let (epoch, store_dir, status) =
            {
                let mut inner = lock(&self.inner)?;
                let epoch = inner.epoch.checked_add(1).ok_or_else(|| {
                    Error::new("index_exhausted", "Inventory generation exhausted")
                })?;
                let generation =
                    inner.status.generation.checked_add(1).ok_or_else(|| {
                        Error::new("index_exhausted", "Query generation exhausted")
                    })?;
                inner.epoch = epoch;
                inner.status = IndexStatus {
                    discovered: 0,
                    state: IndexState::Scanning,
                    generation,
                    warnings: IndexWarnings::default(),
                };
                inner.counts.clear();
                inner.directory_cursor = 0;
                let tx = inner.db.connection.transaction().map_err(sql_error)?;
                tx.execute("DELETE FROM pending_dirs", [])
                    .map_err(sql_error)?;
                tx.execute(
                    "INSERT INTO pending_dirs(path) VALUES(?1)",
                    [encode_path(&self.root)],
                )
                .map_err(sql_error)?;
                tx.commit().map_err(sql_error)?;
                prune(&inner)?;
                (
                    inner.epoch,
                    inner.db.directory.path().to_path_buf(),
                    inner.status.clone(),
                )
            };
        notify(status);
        // A provider can outlive a root rename/replacement. Recheck it before
        // traversal so a replacement junction cannot become a followed root.
        let root_metadata = std::fs::symlink_metadata(&self.root).map_err(io_error)?;
        if !root_metadata.is_dir() || is_link(&root_metadata) {
            return Err(Error::new(
                "invalid_folder",
                "Selected root is no longer a directory without a symlink or reparse point",
            ));
        }
        let mut warnings = IndexWarnings::default();
        let mut batch = Vec::with_capacity(INSERT_CHUNK);
        let mut directories = Vec::with_capacity(INSERT_CHUNK);
        loop {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            let Some(directory) = self.next_directory()? else {
                // Flush deferred directories before declaring traversal done.
                if !directories.is_empty() {
                    notify(self.publish(
                        epoch,
                        &batch,
                        &directories,
                        &warnings,
                        IndexState::Scanning,
                    )?);
                    batch.clear();
                    directories.clear();
                    continue;
                }
                break;
            };
            let metadata = match std::fs::symlink_metadata(&directory) {
                Ok(metadata) => metadata,
                Err(error) => {
                    warnings.push(&directory, error);
                    continue;
                }
            };
            if !metadata.is_dir() || is_link(&metadata) {
                warnings.push(
                    &directory,
                    "Directory was replaced before it could be indexed",
                );
                continue;
            }
            // Shallow walks keep only one directory stream open. Recursive
            // walkdir can buffer entire sibling directories when descriptors
            // spill; the SQLite frontier avoids that hidden inventory buffer.
            let mut builder = WalkBuilder::new(&directory);
            builder
                .standard_filters(false)
                .follow_links(false)
                .max_depth(Some(1));
            let excluded = store_dir.clone();
            builder.filter_entry(move |entry| !entry.path().starts_with(&excluded));
            for entry in builder.build() {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        warnings.push(&directory, error);
                        continue;
                    }
                };
                if let Some(error) = entry.error() {
                    warnings.push(entry.path(), error);
                }
                let metadata = match std::fs::symlink_metadata(entry.path()) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        warnings.push(entry.path(), error);
                        continue;
                    }
                };
                if is_link(&metadata) {
                    continue;
                }
                if metadata.is_dir() {
                    if self.recursive && entry.depth() == 1 {
                        directories.push(entry.path().to_path_buf());
                        if directories.len() == INSERT_CHUNK {
                            notify(self.publish(
                                epoch,
                                &batch,
                                &directories,
                                &warnings,
                                IndexState::Scanning,
                            )?);
                            batch.clear();
                            directories.clear();
                        }
                    }
                    continue;
                }
                if !metadata.is_file() {
                    continue;
                }
                let relative = entry
                    .path()
                    .strip_prefix(&self.root)
                    .map_err(|_| Error::new("invalid_path", "Walker escaped the selected root"))?;
                let modified = match metadata.modified() {
                    Ok(time) => {
                        let millis = match time.duration_since(UNIX_EPOCH) {
                            Ok(duration) => i128::try_from(duration.as_millis()).ok(),
                            Err(error) => i128::try_from(error.duration().as_millis())
                                .ok()
                                .map(|n| -n),
                        };
                        millis
                            .and_then(|value| i64::try_from(value).ok())
                            .map(Into::into)
                    }
                    Err(error) => {
                        warnings.push(entry.path(), error);
                        None
                    }
                };
                let file = IndexedFile {
                    id: 0,
                    path: entry.path().to_path_buf(),
                    metadata: FileMetadata {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        relative_path: relative
                            .components()
                            .map(|c| c.as_os_str().to_string_lossy())
                            .collect::<Vec<_>>()
                            .join("/"),
                        size: metadata.len().into(),
                        modified,
                        mime: mime_guess::from_path(entry.path())
                            .first_or_octet_stream()
                            .essence_str()
                            .into(),
                    },
                };
                batch.push(file);
                if batch.len() == INSERT_CHUNK {
                    notify(self.publish(
                        epoch,
                        &batch,
                        &directories,
                        &warnings,
                        IndexState::Scanning,
                    )?);
                    batch.clear();
                    directories.clear();
                }
            }
        }
        let state = if cancel.load(Ordering::Acquire) {
            IndexState::Cancelled
        } else {
            IndexState::Ready
        };
        let status = self.publish(epoch, &batch, &directories, &warnings, state)?;
        notify(status.clone());
        if state == IndexState::Cancelled {
            Err(Error::cancelled())
        } else {
            Ok(status)
        }
    }

    fn next_directory(&self) -> Result<Option<PathBuf>> {
        let mut inner = lock(&self.inner)?;
        let next: Option<(i64, Vec<u8>)> = inner
            .db
            .connection
            .query_row(
                "SELECT id,path FROM pending_dirs WHERE id>?1 ORDER BY id LIMIT 1",
                [inner.directory_cursor],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sql_error)?;
        match next {
            None => Ok(None),
            Some((id, bytes)) => {
                let path = decode_path(bytes)?;
                // Prune consumed queue rows at the next bounded write commit,
                // avoiding a durable transaction for every visited directory.
                inner.directory_cursor = id;
                Ok(Some(path))
            }
        }
    }

    fn publish(
        &self,
        epoch: i64,
        batch: &[IndexedFile],
        directories: &[PathBuf],
        warnings: &IndexWarnings,
        state: IndexState,
    ) -> Result<IndexStatus> {
        let mut inner = lock(&self.inner)?;
        let discovered = inner
            .status
            .discovered
            .checked_add(batch.len() as u64)
            .ok_or_else(|| Error::new("index_exhausted", "File count exhausted"))?;
        let generation = inner
            .status
            .generation
            .checked_add(1)
            .ok_or_else(|| Error::new("index_exhausted", "Query generation exhausted"))?;
        // Track the bounded active search counts against this committed chunk.
        // A progress view otherwise recounts the entire growing inventory at
        // every notification. Only install these deltas after the SQL commit.
        let mut counts = inner.counts.clone();
        let cursor = inner.directory_cursor;
        let tx = inner.db.connection.transaction().map_err(sql_error)?;
        tx.execute("DELETE FROM pending_dirs WHERE id<=?1", [cursor])
            .map_err(sql_error)?;
        {
            let mut insert = tx.prepare_cached("INSERT INTO files(epoch,path,metadata,name_fold,relative_fold,size_key,modified) VALUES(?1,?2,?3,?4,?5,?6,?7)").map_err(sql_error)?;
            for file in batch {
                let name_fold = file.metadata.name.to_lowercase();
                let relative_fold = file.metadata.relative_path.to_lowercase();
                insert
                    .execute(params![
                        epoch,
                        encode_path(&file.path),
                        revenant_core::serde_json::to_string(&file.metadata)?,
                        name_fold,
                        relative_fold,
                        format!("{:020}", file.metadata.size.get()),
                        file.metadata.modified.map(|v| v.get())
                    ])
                    .map_err(sql_error)?;
                for (search, count) in &mut counts {
                    if name_fold.contains(search.as_str())
                        || relative_fold.contains(search.as_str())
                    {
                        *count += 1;
                    }
                }
            }
        }
        {
            let mut insert = tx
                .prepare_cached("INSERT INTO pending_dirs(path) VALUES(?1)")
                .map_err(sql_error)?;
            for directory in directories {
                insert
                    .execute([encode_path(directory)])
                    .map_err(sql_error)?;
            }
        }
        if state != IndexState::Scanning {
            tx.execute("DELETE FROM pending_dirs", [])
                .map_err(sql_error)?;
        }
        tx.commit().map_err(sql_error)?;
        inner.status.discovered = discovered;
        inner.status.generation = generation;
        inner.status.state = state;
        inner.status.warnings = warnings.clone();
        inner.counts = counts;
        Ok(inner.status.clone())
    }

    /// Reads status without enumerating or fetching metadata rows.
    pub fn status(&self) -> Result<IndexStatus> {
        Ok(lock(&self.inner)?.status.clone())
    }

    /// Returns a coherent page and count from the current inventory revision.
    /// During scanning this is a partial count. Search uses Unicode lowercase
    /// normalization with literal `instr`, so `%`, `_`, and `\\` have no wildcard
    /// meaning. This is lowercase matching, not full Unicode case folding.
    /// Empty-search totals are O(1); up to 16 short search totals are cached and
    /// updated with each committed chunk. A new substring search still scans
    /// SQLite rows, not a Rust Vec. Rescanning clears the bounded count cache.
    pub fn query(&self, query: &FileQuery, offset: u64, limit: u32) -> Result<QueryPage> {
        let order = query.order()?;
        let search = query.search();
        let native_offset = sql_offset(offset)?;
        let mut inner = lock(&self.inner)?;
        let total = if search.is_empty() {
            inner.status.discovered
        } else if let Some((_, count)) = inner.counts.iter().find(|(key, _)| key == &search) {
            *count
        } else {
            let count: i64 = inner
                .db
                .connection
                .query_row(
                    &format!("SELECT count(*) FROM files f WHERE epoch=?1 AND {MATCH}"),
                    params![inner.epoch, search],
                    |r| r.get(0),
                )
                .map_err(sql_error)?;
            if search.len() <= COUNT_CACHE_SEARCH_BYTES {
                if inner.counts.len() == COUNT_CACHE_LIMIT {
                    inner.counts.pop_front();
                }
                inner.counts.push_back((search.clone(), count as u64));
            }
            count as u64
        };
        let items = if offset >= total {
            // A zero-match progress view should not scan the inventory again
            // for its empty page after obtaining a known matching count.
            Vec::new()
        } else {
            // Avoid evaluating substring expressions for inventory browsing.
            let predicate = if search.is_empty() { "1" } else { MATCH };
            let limit = u64::from(page_limit(limit)).min(total - offset) as u32;
            let mut statement = inner.db.connection.prepare(&format!("SELECT {FILE_COLUMNS} FROM files f WHERE epoch=?1 AND {predicate} ORDER BY {order} LIMIT ?3 OFFSET ?4")).map_err(sql_error)?;
            let mut rows = statement
                .query(params![inner.epoch, search, limit, native_offset])
                .map_err(sql_error)?;
            read_files(&mut rows)?
        };
        let status = &inner.status;
        Ok(QueryPage {
            items,
            offset,
            total,
            discovered: status.discovered,
            state: status.state,
            generation: status.generation,
            warnings: status.warnings.clone(),
        })
    }

    /// Looks up a current or selection-pinned ID. IDs are never reused across
    /// rescans; unpinned stale IDs return `file_missing` rather than another file.
    pub fn file(&self, id: i64) -> Result<IndexedFile> {
        let inner = lock(&self.inner)?;
        let mut statement = inner
            .db
            .connection
            .prepare(&format!("SELECT {FILE_COLUMNS} FROM files f WHERE id=?1"))
            .map_err(sql_error)?;
        let mut rows = statement.query([id]).map_err(sql_error)?;
        match rows.next().map_err(sql_error)? {
            Some(row) => read_file(row),
            None => Err(Error::new(
                "file_missing",
                "Indexed file is missing or no longer retained",
            )),
        }
    }

    /// Freezes the ready query in SQLite. Matching IDs and ordinal positions are
    /// inserted using SQL, never collected into a Rust inventory. Native metadata
    /// remains pinned through rescans until the returned cursor drops.
    pub fn selection(self: &Arc<Self>, query: &FileQuery) -> Result<FileSelection> {
        let order = query.order()?;
        let mut inner = lock(&self.inner)?;
        if inner.status.state != IndexState::Ready {
            return Err(Error::new(
                "index_not_ready",
                "Batch selection requires a completed scan",
            ));
        }
        let id = inner
            .next_selection
            .checked_add(1)
            .ok_or_else(|| Error::new("selection_exhausted", "Selection identifiers exhausted"))?;
        let epoch = inner.epoch;
        let tx = inner.db.connection.transaction().map_err(sql_error)?;
        tx.execute("INSERT INTO selections(id) VALUES(?1)", [id])
            .map_err(sql_error)?;
        tx.execute(&format!("INSERT INTO selected(selection,ordinal,file) SELECT ?3,row_number() OVER(ORDER BY {order}),f.id FROM files f WHERE epoch=?1 AND {MATCH}"), params![epoch, query.search(), id]).map_err(sql_error)?;
        let total: i64 = tx
            .query_row(
                "SELECT count(*) FROM selected WHERE selection=?1",
                [id],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        inner.next_selection = id;
        Ok(FileSelection {
            index: self.clone(),
            id,
            total: total as u64,
            generation: inner.status.generation,
            position: 0,
            buffer: VecDeque::new(),
            stopped: false,
        })
    }
}

/// An owned query snapshot with bounded lazy iteration. Ordinal keyset paging
/// avoids progressively scanning offsets for large batches. The iterator yields
/// one database error and then stops; dropping it always releases its snapshot.
pub struct FileSelection {
    index: Arc<FolderIndex>,
    id: i64,
    total: u64,
    generation: u64,
    position: u64,
    buffer: VecDeque<IndexedFile>,
    stopped: bool,
}

impl FileSelection {
    /// Matching count at snapshot creation, independent of subsequent rescans.
    pub fn total(&self) -> u64 {
        self.total
    }
    /// Inventory revision captured by this selection.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn refill(&mut self) -> Result<()> {
        let inner = lock(&self.index.inner)?;
        let mut statement = inner.db.connection.prepare(&format!("SELECT {FILE_COLUMNS} FROM selected s JOIN files f ON f.id=s.file WHERE s.selection=?1 AND s.ordinal>?2 ORDER BY s.ordinal LIMIT 128")).map_err(sql_error)?;
        let mut rows = statement
            .query(params![self.id, sql_offset(self.position)?])
            .map_err(sql_error)?;
        self.buffer = read_files(&mut rows)?.into();
        Ok(())
    }
}

impl Iterator for FileSelection {
    type Item = Result<IndexedFile>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.stopped || self.position >= self.total {
            return None;
        }
        if self.buffer.is_empty() {
            if let Err(error) = self.refill() {
                self.stopped = true;
                return Some(Err(error));
            }
        }
        match self.buffer.pop_front() {
            Some(file) => {
                self.position += 1;
                Some(Ok(file))
            }
            None => {
                self.stopped = true;
                Some(Err(Error::new(
                    "selection_missing",
                    "Snapshot rows disappeared before iteration completed",
                )))
            }
        }
    }
}

impl Drop for FileSelection {
    fn drop(&mut self) {
        // Cleanup is still necessary after a panic poisons the mutex. SQLite
        // transactions roll back on unwind; no callbacks run while locked.
        let inner = self
            .index
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Best effort in Drop; final Database destruction removes all data.
        let _ = inner
            .db
            .connection
            .execute("DELETE FROM selections WHERE id=?1", [self.id]);
        let _ = prune(&inner);
    }
}

fn prune(inner: &Inner) -> Result<()> {
    // Epochs only increase. A range predicate uses files_epoch and avoids
    // rescanning the current 500k rows every time a selection drops.
    inner.db.connection.execute("DELETE FROM files WHERE epoch<?1 AND NOT EXISTS(SELECT 1 FROM selected WHERE selected.file=files.id)", [inner.epoch]).map_err(sql_error)?;
    Ok(())
}

fn read_files(rows: &mut rusqlite::Rows<'_>) -> Result<Vec<IndexedFile>> {
    let mut files = Vec::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        files.push(read_file(row)?);
    }
    Ok(files)
}

fn read_file(row: &rusqlite::Row<'_>) -> Result<IndexedFile> {
    let metadata: String = row.get(2).map_err(sql_error)?;
    Ok(IndexedFile {
        id: row.get(0).map_err(sql_error)?,
        path: decode_path(row.get(1).map_err(sql_error)?)?,
        metadata: revenant_core::serde_json::from_str(&metadata)?,
    })
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}
