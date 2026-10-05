//! Async operation adapters: cooperative admission and blocking worker dispatch.
use crate::{catalog::Catalog, contracts::*, error::problem, rendition, scanning};
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
#[operation(id = "images.bootstrap")]
pub async fn bootstrap(_input: Empty, context: TaskContext) -> Result<Bootstrap> {
    blocking(move || {
        context.checkpoint()?;
        Catalog::open()?.bootstrap()
    })
    .await
}

/// Index a real folder recursively, preserving IDs and notes at existing relative paths.
/// Only one scan runs at a time. Metadata commits in batches of 128; interrupted
/// traversal or unreadable entries suppress missing-file reconciliation. Image decoding is on demand.
#[operation(id = "images.scan")]
pub async fn scan(input: ScanInput, context: TaskContext) -> Result<ScanReport> {
    blocking(move || {
        let _guard = SCANNER
            .get_or_init(|| Mutex::new(()))
            .try_lock()
            .map_err(|_| Error::new("scan_busy", "Another folder scan is still running"))?;
        context.checkpoint()?;
        scanning::scan(&mut Catalog::open()?, &input.path, &context)
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

/// Resolve an available asset inside its stored root and cache a JPEG on a blocking worker.
/// Decode limits are 20,000 pixels per axis and 256 MiB of allocation. Thumbnails
/// fit within 320 pixels, large previews within 1440; encoded bytes are at most 700 KiB.
#[operation(id = "images.render")]
pub async fn render(input: RenderInput, context: TaskContext) -> Result<Rendition> {
    blocking(move || {
        context.checkpoint()?;
        let catalog = Catalog::open()?;
        rendition::render(&catalog, &input, &context)
    })
    .await
}

/// Remove the root and its annotations from the catalog, leaving original files intact.
#[operation(id = "images.removeRoot")]
pub async fn remove_root(input: RootInput, context: TaskContext) -> Result<Removed> {
    blocking(move || {
        context.checkpoint()?;
        Catalog::open()?.remove_root(input)
    })
    .await
}
