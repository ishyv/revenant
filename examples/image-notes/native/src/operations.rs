//! Async operation adapters: cooperative admission and blocking worker dispatch.
use crate::{cache::Cache, catalog::Catalog, contracts::*, error::problem, rendition, scanning};
use revenant::{Error, Result, TaskContext, operation};
use std::sync::{Mutex, OnceLock};

// A process-wide guard preserves scan exclusion across roots and task workers.
static SCANNER: OnceLock<Mutex<()>> = OnceLock::new();

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| problem("catalog_worker", error))?
}

/// Load registered folders without re-enumerating the filesystem.
#[operation(id = "images.bootstrap", read_only, independent, agent)]
pub async fn bootstrap(context: TaskContext) -> Result<Bootstrap> {
    blocking(move || {
        context.checkpoint()?;
        Catalog::open()?.bootstrap()
    })
    .await
}

/// Index a real folder recursively, preserving IDs and notes at existing relative paths.
/// Only one scan runs at a time. Metadata commits in batches of 128; interrupted
/// traversal or unreadable entries suppress missing-file reconciliation. Image decoding is on demand.
#[operation(id = "images.scan", effect = "write", agent)]
pub async fn scan(path: String, context: TaskContext) -> Result<ScanReport> {
    blocking(move || {
        let _guard = SCANNER
            .get_or_init(|| Mutex::new(()))
            .try_lock()
            .map_err(|_| Error::new("scan_busy", "Another folder scan is still running"))?;
        context.checkpoint()?;
        scanning::scan(&mut Catalog::open()?, &path, &context)
    })
    .await
}

/// Query persistent image names and notes using SQLite FTS5, with bounded pages.
#[operation(id = "images.query", read_only, independent, agent)]
pub async fn query(
    search: String,
    root_id: String,
    annotated_only: bool,
    offset: u32,
    limit: u32,
    context: TaskContext,
) -> Result<AssetPage> {
    blocking(move || {
        context.checkpoint()?;
        Catalog::open()?.query(QueryInput {
            search,
            root_id,
            annotated_only,
            offset,
            limit,
        })
    })
    .await
}

/// Atomically save a note and update its full-text index.
#[operation(id = "images.annotate", effect = "write", agent)]
pub async fn annotate(id: String, note: String, context: TaskContext) -> Result<SavedNote> {
    blocking(move || {
        context.checkpoint()?;
        Catalog::open()?.annotate(NoteInput { id, note })
    })
    .await
}

/// Resolve an available asset inside its stored root and cache a JPEG on a blocking worker.
/// Decode limits are 20,000 pixels per axis and 256 MiB of allocation. Thumbnails
/// fit within 320 pixels, large previews within 1440; encoded bytes are at most 700 KiB.
/// Renditions share a 512 MiB disk budget. Reads and writes hold an OS file lease,
/// so eviction and clearing wait for active writers across Luma processes.
#[operation(id = "images.render", effect = "write", independent, agent)]
pub async fn render(id: String, large: bool, context: TaskContext) -> Result<Rendition> {
    blocking(move || {
        context.checkpoint()?;
        let catalog = Catalog::open()?;
        rendition::render(&catalog, &RenderInput { id, large }, &context)
    })
    .await
}

/// Remove the root and its annotations from the catalog, leaving original files intact.
#[operation(id = "images.removeRoot", effect = "write", agent)]
pub async fn remove_root(id: String, context: TaskContext) -> Result<Removed> {
    blocking(move || {
        context.checkpoint()?;
        Catalog::open()?.remove_root(RootInput { id })
    })
    .await
}

/// Measure the 512 MiB rendition payload budget, including preserved legacy files.
/// This never changes registered folders, annotations or original images.
#[operation(id = "images.cacheUsage", effect = "write", agent)]
pub async fn cache_usage(context: TaskContext) -> Result<CacheUsage> {
    blocking(move || {
        context.checkpoint()?;
        let catalog = Catalog::open()?;
        Cache::open(catalog.directory(), &context)?.maintain(&catalog, &context)
    })
    .await
}

/// Clear only renditions and interrupted writes recorded as Luma-owned.
/// The cross-process lease preserves active writers. Busy, inaccessible or locked
/// files return an error; an interrupted clear may have removed some cache entries.
/// Subsequent viewing regenerates renditions from the unchanged original images.
#[operation(id = "images.clearCache", effect = "write", agent)]
pub async fn clear_cache(context: TaskContext) -> Result<ClearedCache> {
    blocking(move || {
        context.checkpoint()?;
        let catalog = Catalog::open()?;
        let cache = Cache::open(catalog.directory(), &context)?;
        let (removed_bytes, removed_entries, usage) = cache.clear(&context)?;
        Ok(ClearedCache {
            removed_bytes: removed_bytes.into(),
            removed_entries,
            usage,
        })
    })
    .await
}
