//! SQLite storage and FTS5 search. Filesystem traversal and decoding live elsewhere.
//! Refresh upserts retain identities and annotations; missing files are marked unavailable.
use crate::{contracts::*, error::problem};
use revenant::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_NOTE_BYTES: usize = 16 * 1024;

/// Metadata collected without decoding an image; each batch is committed atomically.
pub(crate) struct ScannedImage {
    pub(crate) relative_path: String,
    pub(crate) name: String,
    pub(crate) size: String,
    pub(crate) modified: String,
}

pub(crate) struct Catalog {
    db: Connection,
    directory: PathBuf,
}

impl Catalog {
    pub(crate) fn open() -> Result<Self> {
        let base = dirs::data_local_dir().ok_or_else(|| {
            Error::new(
                "catalog_location",
                "No local application data directory is available",
            )
        })?;
        Self::open_at(&base.join("app.revenant.image-notes"))
    }

    pub(crate) fn open_at(directory: &Path) -> Result<Self> {
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

    pub(crate) fn bootstrap(&self) -> Result<Bootstrap> {
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

    pub(crate) fn insert_batch(
        &mut self,
        root: &str,
        epoch: &str,
        batch: &[ScannedImage],
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
            for image in batch {
                statement
                    .execute(params![
                        uuid::Uuid::new_v4().to_string(),
                        root,
                        image.relative_path,
                        image.name,
                        image.size,
                        image.modified,
                        epoch
                    ])
                    .map_err(|e| problem("catalog_write", e))?;
            }
        }
        transaction
            .commit()
            .map_err(|e| problem("catalog_write", e))
    }

    pub(crate) fn query(&self, input: QueryInput) -> Result<AssetPage> {
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

    pub(crate) fn annotate(&self, input: NoteInput) -> Result<SavedNote> {
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

    /// Locate a cataloged asset; the renderer validates its current filesystem location.
    pub(crate) fn asset_location(&self, id: &str) -> Result<(PathBuf, PathBuf)> {
        let record: Option<(String, String)> = self.db.query_row(
            "SELECT r.path,a.relative_path FROM assets a JOIN roots r ON r.id=a.root_id WHERE a.id=?1 AND a.available=1",
            [id], |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional().map_err(|e| problem("catalog_query", e))?;
        let (root, relative) = record
            .ok_or_else(|| Error::new("asset_missing", "This image is no longer available"))?;
        Ok((PathBuf::from(root), PathBuf::from(relative)))
    }

    /// Reuse the canonical folder identity, updating only its display name.
    pub(crate) fn register_root(&self, root: &Path) -> Result<String> {
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
        self.db.execute("INSERT INTO roots(id,path,name) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET name=excluded.name", params![id,path,name])
            .map_err(|e| problem("catalog_write", e))?;
        Ok(id)
    }

    /// Only a complete, warning-free scan may hide unseen records. Notes remain stored.
    pub(crate) fn reconcile_scan(&self, root: &str, epoch: &str) -> Result<()> {
        self.db
            .execute(
                "UPDATE assets SET available=0 WHERE root_id=?1 AND seen<>?2",
                params![root, epoch],
            )
            .map_err(|e| problem("catalog_write", e))?;
        Ok(())
    }

    /// Cascading catalog deletion updates FTS through the existing schema triggers.
    pub(crate) fn remove_root(&self, input: RootInput) -> Result<Removed> {
        let removed = self
            .db
            .execute("DELETE FROM roots WHERE id=?1", [input.id])
            .map_err(|e| problem("catalog_write", e))?;
        Ok(Removed {
            removed: removed > 0,
        })
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }
}

fn count(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
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
#[path = "tests.rs"]
mod tests;
