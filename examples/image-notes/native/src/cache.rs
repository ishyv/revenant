//! Owned rendition files, a durable write journal and a cross-process cache lease.
//! The 512 MiB payload budget includes unrecognized files, which are reported but
//! never removed. Catalog records, notes and originals live outside this store.
use crate::{catalog::Catalog, contracts::CacheUsage, error::problem};
use revenant::{Error, Result, TaskContext};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// Maximum rendition payload bytes retained by Luma: 512 MiB.
pub(crate) const BUDGET_BYTES: u64 = 512 * 1024 * 1024;
const OWNER: &str = "app.revenant.image-notes/renditions/1";
const MAX_WAIT: Duration = Duration::from_secs(5);

/// Closing the file releases the OS lease, including after a forced process exit.
/// The database closes first; the stable lease file is never renamed or removed.
pub(crate) struct Cache {
    db: Connection,
    _lease: File,
    directory: PathBuf,
    budget: u64,
}

impl Cache {
    pub(crate) fn open(data: &Path, context: &TaskContext) -> Result<Self> {
        Self::open_with_budget(data, BUDGET_BYTES, context)
    }

    fn open_with_budget(data: &Path, budget: u64, context: &TaskContext) -> Result<Self> {
        let directory = data.join("renditions");
        fs::create_dir_all(&directory).map_err(|e| problem("cache_io", e))?;
        require_regular_directory(&directory)?;
        let lock = directory.join("cache.lease");
        require_regular_if_present(&lock)?;
        let lease = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock)
            .map_err(|e| problem("cache_io", e))?;
        let deadline = Instant::now() + MAX_WAIT;
        loop {
            context.checkpoint()?;
            match lease.try_lock() {
                Ok(()) => break,
                Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(fs::TryLockError::WouldBlock) => {
                    return Err(Error::new(
                        "cache_busy",
                        "Another Luma process is using the rendition cache; retry shortly",
                    ));
                }
                Err(fs::TryLockError::Error(error)) => return Err(problem("cache_io", error)),
            }
        }
        let index = directory.join("cache.sqlite3");
        require_regular_if_present(&index)?;
        let fresh = !index.exists();
        let marker = directory.join("cache.owner");
        require_regular_if_present(&marker)?;
        // Ownership is outside SQLite so a proven owned database can recover a
        // hot rollback journal after a process exit. An unmarked coincidental
        // database is rejected before opening it or changing any of its bytes.
        if marker.exists() {
            if marker.metadata().map_err(|e| problem("cache_io", e))?.len() != OWNER.len() as u64
                || fs::read(&marker).map_err(|e| problem("cache_io", e))? != OWNER.as_bytes()
            {
                return Err(Error::new(
                    "cache_invalid",
                    "Rendition ownership marker is incompatible; files were preserved",
                ));
            }
        } else if !fresh {
            return Err(Error::new(
                "cache_invalid",
                "Rendition cache index has no ownership marker; files were preserved",
            ));
        } else {
            let mut record = File::create_new(&marker).map_err(|e| problem("cache_io", e))?;
            record
                .write_all(OWNER.as_bytes())
                .and_then(|_| record.sync_all())
                .map_err(|e| problem("cache_io", e))?;
        }
        if fresh {
            File::create_new(&index).map_err(|e| problem("cache_io", e))?;
        }
        let db = Connection::open(index).map_err(|e| problem("cache_io", e))?;
        let tables: u32 = db
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| problem("cache_io", e))?;
        if tables != 0 {
            require_owner(&db)?;
        }
        db.execute_batch("PRAGMA synchronous=FULL; PRAGMA journal_mode=DELETE;")
            .map_err(|e| problem("cache_io", e))?;
        if tables == 0 {
            db.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE owner(identity TEXT NOT NULL);
                 INSERT INTO owner VALUES('app.revenant.image-notes/renditions/1');
                 CREATE TABLE clock(value INTEGER NOT NULL);
                 INSERT INTO clock VALUES(0);
                 CREATE TABLE entries(file TEXT PRIMARY KEY,asset TEXT NOT NULL,revision TEXT NOT NULL,bound INTEGER NOT NULL,size INTEGER NOT NULL,used INTEGER NOT NULL,UNIQUE(asset,revision,bound));
                 CREATE INDEX entries_lru ON entries(used,file);
                 CREATE TABLE writes(file TEXT PRIMARY KEY,temporary TEXT NOT NULL);
                 COMMIT;",
            ).map_err(|e| problem("cache_io", e))?;
        }
        require_owner(&db)?;
        let cache = Self {
            db,
            _lease: lease,
            directory,
            budget,
        };
        cache.recover_writes(context)?;
        Ok(cache)
    }

    /// Recover only durable journal records. A live writer always holds our lease.
    fn recover_writes(&self, context: &TaskContext) -> Result<()> {
        loop {
            context.checkpoint()?;
            let pending: Option<(String, String)> = self
                .db
                .query_row("SELECT file,temporary FROM writes LIMIT 1", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .optional()
                .map_err(|e| problem("cache_io", e))?;
            let Some((file, temporary)) = pending else {
                break;
            };
            self.remove_owned_file(&temporary, "tmp")?;
            self.remove_owned_file(&file, "jpg")?;
            self.db
                .execute("DELETE FROM writes WHERE file=?1", [file])
                .map_err(|e| problem("cache_io", e))?;
        }
        Ok(())
    }

    pub(crate) fn read(
        &self,
        asset: &str,
        revision: &str,
        bound: u32,
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>> {
        let file: Option<String> = self
            .db
            .query_row(
                "SELECT file FROM entries WHERE asset=?1 AND revision=?2 AND bound=?3",
                params![asset, revision, bound],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| problem("cache_io", e))?;
        let Some(file) = file else { return Ok(None) };
        let path = self.owned_path(&file, "jpg")?;
        require_regular_if_present(&path)?;
        if path
            .metadata()
            .is_ok_and(|metadata| metadata.len() > max_bytes as u64)
        {
            return Err(Error::new(
                "cache_invalid",
                "Cached rendition exceeds its budget",
            ));
        }
        let input = match File::open(path) {
            Ok(input) => input,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.db
                    .execute("DELETE FROM entries WHERE file=?1", [file])
                    .map_err(|e| problem("cache_io", e))?;
                return Ok(None);
            }
            Err(error) => return Err(problem("cache_io", error)),
        };
        let mut bytes = Vec::new();
        input
            .take(max_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| problem("cache_io", e))?;
        if bytes.len() > max_bytes {
            return Err(Error::new(
                "cache_invalid",
                "Cached rendition exceeds its budget",
            ));
        }
        self.db
            .execute("UPDATE clock SET value=value+1", [])
            .map_err(|e| problem("cache_io", e))?;
        self.db
            .execute(
                "UPDATE entries SET used=(SELECT value FROM clock) WHERE file=?1",
                [file],
            )
            .map_err(|e| problem("cache_io", e))?;
        Ok(Some(bytes))
    }

    /// Changed originals invalidate both rendition sizes; exact ownership stays authoritative.
    pub(crate) fn remove_obsolete(
        &self,
        asset: &str,
        revision: &str,
        context: &TaskContext,
    ) -> Result<()> {
        loop {
            context.checkpoint()?;
            let file: Option<String> = self
                .db
                .query_row(
                    "SELECT file FROM entries WHERE asset=?1 AND revision<>?2 LIMIT 1",
                    params![asset, revision],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| problem("cache_io", e))?;
            let Some(file) = file else { return Ok(()) };
            self.remove_entry(&file)?;
        }
    }

    /// Returns a visible warning when preserved files prevent caching this rendition.
    /// The generated payload can still be displayed without creating a disk entry.
    pub(crate) fn publish(
        &self,
        asset: &str,
        revision: &str,
        bound: u32,
        bytes: &[u8],
        context: &TaskContext,
    ) -> Result<Option<String>> {
        let required = bytes.len() as u64;
        let usage = self.trim(required, context)?;
        if usage.unmanaged_bytes.get().saturating_add(required) > self.budget {
            return Ok(Some("Rendition was generated without disk caching: preserved unrecognized files leave insufficient space in the 512 MiB cache budget".into()));
        }
        context.checkpoint()?;
        let token = uuid::Uuid::new_v4().to_string();
        let file = format!("{token}.jpg");
        let temporary = format!("{token}.tmp");
        // Commit ownership before creating either file: a restart can recover both
        // sides of a process exit between rename and the final index transaction.
        self.db
            .execute(
                "INSERT INTO writes(file,temporary) VALUES(?1,?2)",
                params![file, temporary],
            )
            .map_err(|e| problem("cache_io", e))?;
        let mut output = File::create_new(self.owned_path(&temporary, "tmp")?)
            .map_err(|e| problem("cache_io", e))?;
        output
            .write_all(bytes)
            .and_then(|_| output.sync_all())
            .map_err(|e| problem("cache_io", e))?;
        drop(output);
        context.checkpoint()?;
        fs::rename(
            self.owned_path(&temporary, "tmp")?,
            self.owned_path(&file, "jpg")?,
        )
        .map_err(|e| problem("cache_io", e))?;
        self.db
            .execute_batch("BEGIN IMMEDIATE; UPDATE clock SET value=value+1;")
            .map_err(|e| problem("cache_io", e))?;
        let published = self.db.execute(
            "INSERT INTO entries(file,asset,revision,bound,size,used) VALUES(?1,?2,?3,?4,?5,(SELECT value FROM clock))",
            params![file, asset, revision, bound, bytes.len() as i64],
        ).and_then(|_| self.db.execute("DELETE FROM writes WHERE file=?1", [&file]))
         .and_then(|_| self.db.execute_batch("COMMIT"));
        if let Err(error) = published {
            let _ = self.db.execute_batch("ROLLBACK");
            return Err(problem("cache_io", error));
        }
        Ok(None)
    }

    /// Prune unavailable catalog entries, then enforce the payload budget. LRU
    /// eviction cannot remove unrecognized paths even when they exceed the budget.
    pub(crate) fn maintain(&self, catalog: &Catalog, context: &TaskContext) -> Result<CacheUsage> {
        let mut cursor = String::new();
        loop {
            context.checkpoint()?;
            let entry: Option<(String, String)> = self
                .db
                .query_row(
                    "SELECT file,asset FROM entries WHERE file>?1 ORDER BY file LIMIT 1",
                    [&cursor],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| problem("cache_io", e))?;
            let Some((file, asset)) = entry else { break };
            match catalog.asset_location(&asset) {
                Ok(_) => {}
                Err(error) if error.code == "asset_missing" => {
                    self.remove_entry(&file)?;
                }
                Err(error) => return Err(error),
            }
            cursor = file;
        }
        self.trim(0, context)
    }

    fn trim(&self, required: u64, context: &TaskContext) -> Result<CacheUsage> {
        let mut usage = self.usage(context)?;
        // If this publication cannot fit around preserved files, reclaim only an
        // already exceeded budget; generating uncached must not evict useful data.
        let required = if usage.unmanaged_bytes.get().saturating_add(required) > self.budget {
            0
        } else {
            required
        };
        while usage.bytes.get().saturating_add(required) > self.budget {
            context.checkpoint()?;
            let oldest: Option<String> = self
                .db
                .query_row(
                    "SELECT file FROM entries ORDER BY used,file LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| problem("cache_io", e))?;
            let Some(oldest) = oldest else {
                // Only preserved files remain. Report the excess instead of
                // erasing unrecognized content to satisfy an impossible budget.
                break;
            };
            let removed = self.remove_entry(&oldest)?;
            usage.bytes = usage.bytes.get().saturating_sub(removed).into();
        }
        self.usage(context)
    }

    /// Measure actual payload sizes, including unrecognized files and nested paths.
    /// Link targets are never traversed; control files are outside the payload budget.
    pub(crate) fn usage(&self, context: &TaskContext) -> Result<CacheUsage> {
        let mut bytes = 0_u64;
        let mut managed_bytes = 0_u64;
        let mut entries = 0_u32;
        let mut owned = self
            .db
            .prepare("SELECT EXISTS(SELECT 1 FROM entries WHERE file=?1)")
            .map_err(|e| problem("cache_io", e))?;
        let mut inventory = walkdir::WalkDir::new(&self.directory)
            .follow_links(false)
            .min_depth(1)
            .into_iter();
        while let Some(item) = inventory.next() {
            context.checkpoint()?;
            let item = item.map_err(|e| problem("cache_io", e))?;
            let metadata = fs::symlink_metadata(item.path()).map_err(|e| problem("cache_io", e))?;
            let link = is_link(&metadata);
            if metadata.is_dir() && link {
                // Windows directory junctions are reparse points too; never
                // descend through one even when the walker sees a directory.
                inventory.skip_current_dir();
            }
            if metadata.is_dir() && !link {
                continue;
            }
            let name = item.file_name().to_string_lossy();
            if item.depth() == 1
                && matches!(
                    name.as_ref(),
                    "cache.lease" | "cache.owner" | "cache.sqlite3" | "cache.sqlite3-journal"
                )
            {
                require_regular_if_present(item.path())?;
                continue;
            }
            let size = metadata.len();
            bytes = bytes.saturating_add(size);
            let is_owned: bool = item.depth() == 1
                && owned
                    .query_row([name.as_ref()], |row| row.get(0))
                    .map_err(|e| problem("cache_io", e))?;
            if is_owned {
                require_regular_if_present(item.path())?;
                managed_bytes = managed_bytes.saturating_add(size);
                entries = entries.saturating_add(1);
            }
        }
        let unmanaged_bytes = bytes.saturating_sub(managed_bytes);
        Ok(CacheUsage {
            bytes: bytes.into(),
            managed_bytes: managed_bytes.into(),
            unmanaged_bytes: unmanaged_bytes.into(),
            entries,
            budget_bytes: self.budget.into(),
            publication_blocked: unmanaged_bytes >= self.budget,
        })
    }

    pub(crate) fn clear(&self, context: &TaskContext) -> Result<(u64, u32, CacheUsage)> {
        let mut bytes = 0_u64;
        let mut entries = 0_u32;
        loop {
            context.checkpoint()?;
            let file: Option<String> = self
                .db
                .query_row("SELECT file FROM entries LIMIT 1", [], |row| row.get(0))
                .optional()
                .map_err(|e| problem("cache_io", e))?;
            let Some(file) = file else { break };
            bytes = bytes.saturating_add(self.remove_entry(&file)?);
            entries = entries.saturating_add(1);
        }
        Ok((bytes, entries, self.usage(context)?))
    }

    fn remove_entry(&self, file: &str) -> Result<u64> {
        let size = self.remove_owned_file(file, "jpg")?;
        self.db
            .execute("DELETE FROM entries WHERE file=?1", [file])
            .map_err(|e| problem("cache_io", e))?;
        Ok(size)
    }

    fn remove_owned_file(&self, file: &str, extension: &str) -> Result<u64> {
        let path = self.owned_path(file, extension)?;
        require_regular_if_present(&path)?;
        match fs::metadata(&path) {
            Ok(metadata) => {
                fs::remove_file(path).map_err(|e| problem("cache_io", e))?;
                Ok(metadata.len())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(problem("cache_io", error)),
        }
    }

    fn owned_path(&self, file: &str, extension: &str) -> Result<PathBuf> {
        let stem = file.strip_suffix(&format!(".{extension}")).ok_or_else(|| {
            Error::new(
                "cache_invalid",
                "Invalid rendition ownership record; files were preserved",
            )
        })?;
        let token = uuid::Uuid::parse_str(stem).map_err(|_| {
            Error::new(
                "cache_invalid",
                "Invalid rendition ownership record; files were preserved",
            )
        })?;
        if stem != token.to_string() {
            return Err(Error::new(
                "cache_invalid",
                "Invalid rendition ownership record; files were preserved",
            ));
        }
        Ok(self.directory.join(file))
    }
}

fn require_owner(db: &Connection) -> Result<()> {
    let (owner, count): (String, u32) = db
        .query_row(
            "SELECT identity,(SELECT count(*) FROM owner) FROM owner",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| {
            Error::new(
                "cache_invalid",
                "Rendition cache has no valid ownership record; its files were preserved",
            )
        })?;
    if owner == OWNER && count == 1 {
        Ok(())
    } else {
        Err(Error::new(
            "cache_invalid",
            "Rendition cache ownership is incompatible; its files were preserved",
        ))
    }
}

fn require_regular_if_present(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !is_link(&metadata) => Ok(()),
        Ok(_) => Err(Error::new(
            "cache_invalid",
            format!(
                "Cache path is not a regular file; preserved {}",
                path.display()
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(problem("cache_io", error)),
    }
}

fn require_regular_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|e| problem("cache_io", e))?;
    if metadata.is_dir() && !is_link(&metadata) {
        Ok(())
    } else {
        Err(Error::new(
            "cache_invalid",
            "Rendition cache directory is a link or an incompatible file; it was preserved",
        ))
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
