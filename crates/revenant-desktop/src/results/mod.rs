//! Native paged batch outcomes, retained by explicit leases.
//!
//! Every completed item is persisted as input/output/error JSON under the item's
//! original ordinal. Workers may finish out of order; queries always use ordinal
//! order. This module never accumulates a whole batch or chooses task scheduling.
//! The host streams a folder selection into its bounded queue and calls `append`
//! as each item finishes. Missing ordinals represent unfinished items, not errors.
//!
//! `new_with_owner` retains one host-owned lifetime guard, such as a resource scope
//! that holds output files. Its cleanup must occur when the guard's final Arc
//! drops. `lease` retains the whole store and guard, so scope disposal cannot
//! delete files while a result consumer still owns a lease. JSON alone does not
//! retain resource handles: the host must put their ownership in that guard.
//!
//! Dropping the last store/lease closes SQLite before deleting its private temp
//! directory and releasing the owner. These are temporary inspection results,
//! not durable exports; applications must explicitly export anything to keep.

#![doc = include_str!("README.md")]

use crate::files::storage::{Database, lock, page_limit, sql_error, sql_offset};
use revenant_core::{
    Error, Result,
    serde::{Deserialize, Serialize},
    serde_json::{self, Value},
};
use rusqlite::params;
use std::{
    ops::Deref,
    path::Path,
    sync::{Arc, Mutex},
};

/// One completed batch item. Ordinals are zero-based original input positions,
/// unique within the store. Exactly one of output/error must be present; a JSON
/// null output is a valid success. Native resource ownership belongs to the host.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    crate = "revenant_core::serde",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub struct BatchOutcome {
    /// Zero-based source position, independent of worker completion order.
    pub ordinal: u64,
    /// The actual per-item operation input captured by the host.
    pub input: Value,
    /// Successful output. Absence is omitted on the wire; present JSON null
    /// remains a success when serialized and deserialized.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "json_slot"
    )]
    pub output: Option<Value>,
    /// Failed/cancelled item details. Exactly one output/error slot is present.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "json_slot"
    )]
    pub error: Option<Value>,
}

fn json_slot<'de, D: revenant_core::serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

/// Completed counts only. Cancellation leaves pending items absent; planned batch
/// totals and task lifecycle remain the scheduler's responsibility.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(crate = "revenant_core::serde", rename_all = "camelCase")]
pub struct ResultCounts {
    /// Number of completed, persisted items.
    pub total: u64,
    /// Items with an output, including present JSON null.
    pub succeeded: u64,
    /// Items with an error, including per-item cancellation errors.
    pub failed: u64,
}

/// Bounded ordinal-ordered window. Generation changes after every committed
/// insertion chunk, allowing clients to detect shifting offset pages while a
/// batch runs. Offset is a position among completed rows, not an item ordinal.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "revenant_core::serde", rename_all = "camelCase")]
pub struct ResultPage {
    /// Completed items in source ordinal order, bounded to 512 rows.
    pub items: Vec<BatchOutcome>,
    /// Position among completed rows; this is not a source ordinal.
    pub offset: u64,
    /// Same completed total as counts.total, convenient for pagination.
    pub total: u64,
    /// Counts from the same committed revision as these rows.
    pub counts: ResultCounts,
    /// Incremented on each insertion chunk; restart offset paging if it changes.
    pub generation: u64,
}

/// SQLite-backed batch result sink. Construction is fast and allocates neither
/// threads nor an in-memory item list. Methods are blocking; the desktop host
/// schedules calls and enforces operation-specific JSON size limits.
pub struct ResultStore {
    database: Mutex<Database>,
    // Keep the host scope alive after the store's native DB has closed.
    _owner: Option<Arc<dyn Send + Sync>>,
}

impl ResultStore {
    /// Creates a private temporary result database below the supplied cache.
    pub fn new(cache_dir: &Path) -> Result<Arc<Self>> {
        Self::create(cache_dir, None)
    }

    /// Retains a host-owned scope/lifetime guard until all result leases end.
    /// The guard is retained once for the batch rather than once per JSON row.
    pub fn new_with_owner(cache_dir: &Path, owner: Arc<dyn Send + Sync>) -> Result<Arc<Self>> {
        Self::create(cache_dir, Some(owner))
    }

    fn create(cache_dir: &Path, owner: Option<Arc<dyn Send + Sync>>) -> Result<Arc<Self>> {
        let database = Database::new(cache_dir, "revenant-results-")?;
        database.connection.execute_batch(
            "CREATE TABLE outcomes(ordinal INTEGER PRIMARY KEY,input TEXT NOT NULL,output TEXT,error TEXT,
             CHECK((output IS NULL) <> (error IS NULL)));
             CREATE TABLE counts(id INTEGER PRIMARY KEY CHECK(id=1),total INTEGER NOT NULL,
             succeeded INTEGER NOT NULL,failed INTEGER NOT NULL,generation INTEGER NOT NULL);
             INSERT INTO counts VALUES(1,0,0,0,0);"
        ).map_err(sql_error)?;
        Ok(Arc::new(Self {
            database: Mutex::new(database),
            _owner: owner,
        }))
    }

    /// Appends one immutable completion atomically with counts. Duplicate
    /// ordinals return `duplicate_outcome`; existing JSON/counts stay unchanged.
    pub fn append(&self, outcome: &BatchOutcome) -> Result<()> {
        self.append_chunk(std::slice::from_ref(outcome))
    }

    /// Streams completed outcomes through transactions of at most 128 rows.
    /// Earlier chunks remain committed if a later item is invalid. A failed
    /// chunk rolls back entirely. Neither input nor output is ever collected
    /// beyond that bounded chunk by this method.
    pub fn append_many(&self, outcomes: impl IntoIterator<Item = BatchOutcome>) -> Result<()> {
        let mut chunk = Vec::with_capacity(128);
        for outcome in outcomes {
            chunk.push(outcome);
            if chunk.len() == 128 {
                self.append_chunk(&chunk)?;
                chunk.clear();
            }
        }
        if !chunk.is_empty() {
            self.append_chunk(&chunk)?;
        }
        Ok(())
    }

    fn append_chunk(&self, outcomes: &[BatchOutcome]) -> Result<()> {
        let mut database = lock(&self.database)?;
        let tx = database.connection.transaction().map_err(sql_error)?;
        let mut succeeded = 0i64;
        let mut failed = 0i64;
        {
            let mut insert = tx
                .prepare_cached(
                    "INSERT INTO outcomes(ordinal,input,output,error) VALUES(?1,?2,?3,?4)",
                )
                .map_err(sql_error)?;
            for outcome in outcomes {
                if outcome.output.is_some() == outcome.error.is_some() {
                    return Err(Error::new(
                        "invalid_outcome",
                        "An outcome requires exactly one of output or error",
                    ));
                }
                let ordinal = sql_offset(outcome.ordinal)?;
                let input = serde_json::to_string(&outcome.input)?;
                let output = outcome
                    .output
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?;
                let error = outcome
                    .error
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?;
                if let Err(error) = insert.execute(params![ordinal, input, output, error]) {
                    if matches!(&error, rusqlite::Error::SqliteFailure(code, _) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY)
                    {
                        return Err(Error::new(
                            "duplicate_outcome",
                            "Batch ordinal already has a completed outcome",
                        ));
                    }
                    return Err(sql_error(error));
                }
                if outcome.error.is_some() {
                    failed += 1;
                } else {
                    succeeded += 1;
                }
            }
        }
        tx.execute("UPDATE counts SET total=total+?1,succeeded=succeeded+?2,failed=failed+?3,generation=generation+1 WHERE id=1", params![outcomes.len() as i64, succeeded, failed]).map_err(sql_error)?;
        tx.commit().map_err(sql_error)
    }

    /// Reads incrementally maintained completed counts without scanning rows.
    pub fn counts(&self) -> Result<ResultCounts> {
        let database = lock(&self.database)?;
        read_counts(&database.connection).map(|(counts, _)| counts)
    }

    /// Returns at most 512 outcomes (`limit=0` uses 128) plus coherent counts.
    /// Workers may fill earlier ordinals later; consumers paging a running batch
    /// should compare generations and restart when they change.
    pub fn query(&self, offset: u64, limit: u32) -> Result<ResultPage> {
        let database = lock(&self.database)?;
        let (counts, generation) = read_counts(&database.connection)?;
        let mut statement = database.connection.prepare("SELECT ordinal,input,output,error FROM outcomes ORDER BY ordinal LIMIT ?1 OFFSET ?2").map_err(sql_error)?;
        let mut rows = statement
            .query(params![page_limit(limit), sql_offset(offset)?])
            .map_err(sql_error)?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let ordinal: i64 = row.get(0).map_err(sql_error)?;
            let input: String = row.get(1).map_err(sql_error)?;
            let output: Option<String> = row.get(2).map_err(sql_error)?;
            let error: Option<String> = row.get(3).map_err(sql_error)?;
            items.push(BatchOutcome {
                ordinal: ordinal as u64,
                input: serde_json::from_str(&input)?,
                output: output.map(|json| serde_json::from_str(&json)).transpose()?,
                error: error.map(|json| serde_json::from_str(&json)).transpose()?,
            });
        }
        Ok(ResultPage {
            items,
            offset,
            total: counts.total,
            counts,
            generation,
        })
    }

    /// Holds the database and its scope owner until this consumer drops its lease.
    pub fn lease(self: &Arc<Self>) -> ResultLease {
        ResultLease {
            store: self.clone(),
        }
    }
}

/// A clonable native result lifetime lease. Accessors dereference to the store;
/// releasing the last store/lease performs temp cleanup and host guard release.
#[derive(Clone)]
pub struct ResultLease {
    store: Arc<ResultStore>,
}

impl Deref for ResultLease {
    type Target = ResultStore;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

fn read_counts(connection: &rusqlite::Connection) -> Result<(ResultCounts, u64)> {
    connection
        .query_row(
            "SELECT total,succeeded,failed,generation FROM counts WHERE id=1",
            [],
            |row| {
                Ok((
                    ResultCounts {
                        total: row.get::<_, i64>(0)? as u64,
                        succeeded: row.get::<_, i64>(1)? as u64,
                        failed: row.get::<_, i64>(2)? as u64,
                    },
                    row.get::<_, i64>(3)? as u64,
                ))
            },
        )
        .map_err(sql_error)
}
