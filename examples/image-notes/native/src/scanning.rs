//! Metadata-only folder traversal, bounded batch writes and scan reconciliation.
use crate::{
    catalog::{Catalog, ScannedImage},
    contracts::ScanReport,
    error::problem,
    filesystem::{is_link, modified},
};
use revenant::{Error, Progress, Result, TaskContext};
use std::{
    fs,
    path::{Path, PathBuf},
};

const BATCH_SIZE: usize = 128;

struct ScanProgress {
    discovered: u32,
    warnings: u32,
    pending: Vec<ScannedImage>,
}

enum Inspection {
    Skip,
    Unreadable,
    Image(ScannedImage),
}

/// Refresh a real folder; partial writes retain existing IDs and annotations.
pub(crate) fn scan(
    catalog: &mut Catalog,
    selected: &str,
    context: &TaskContext,
) -> Result<ScanReport> {
    let root = resolve_root(selected)?;
    let id = catalog.register_root(&root)?;
    let epoch = uuid::Uuid::new_v4().to_string();
    let progress = walk_images(catalog, &root, &id, &epoch, context)?;
    finish_scan(catalog, id, &epoch, progress, context)
}

fn resolve_root(selected: &str) -> Result<PathBuf> {
    let selected = PathBuf::from(selected);
    if !selected.is_dir() || is_link(&selected)? {
        return Err(Error::new(
            "invalid_folder",
            "Choose a real folder, not a symbolic link or junction",
        ));
    }
    fs::canonicalize(selected).map_err(|e| problem("folder_unavailable", e))
}

fn walk_images(
    catalog: &mut Catalog,
    root: &Path,
    id: &str,
    epoch: &str,
    context: &TaskContext,
) -> Result<ScanProgress> {
    let cache =
        fs::canonicalize(catalog.directory()).unwrap_or_else(|_| catalog.directory().to_owned());
    let mut progress = ScanProgress {
        discovered: 0,
        warnings: 0,
        pending: Vec::with_capacity(BATCH_SIZE),
    };
    let walker = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !entry.path().starts_with(&cache)
                && !entry.file_type().is_symlink()
                && (!entry.file_type().is_dir() || !is_link(entry.path()).unwrap_or(true))
        });
    for entry in walker {
        context.checkpoint()?;
        let inspection = match entry {
            Ok(entry) => inspect_image(&entry, root)?,
            Err(_) => Inspection::Unreadable,
        };
        match inspection {
            Inspection::Skip => continue,
            Inspection::Unreadable => {
                progress.warnings = progress.warnings.saturating_add(1);
                continue;
            }
            Inspection::Image(image) => progress.pending.push(image),
        }
        progress.discovered = progress.discovered.checked_add(1).ok_or_else(|| {
            Error::new("catalog_limit", "Image count exceeds this example's limit")
        })?;
        if progress.pending.len() == BATCH_SIZE {
            catalog.insert_batch(id, epoch, &progress.pending)?;
            progress.pending.clear();
            context.progress(Progress {
                completed: u64::from(progress.discovered).into(),
                total: None,
                message: Some(format!("{} imágenes encontradas", progress.discovered)),
            })?;
        }
    }
    Ok(progress)
}

fn inspect_image(entry: &walkdir::DirEntry, root: &Path) -> Result<Inspection> {
    if !entry.file_type().is_file() || !supported(entry.path()) || is_link(entry.path())? {
        return Ok(Inspection::Skip);
    }
    let metadata = match entry.metadata() {
        Ok(value) => value,
        Err(_) => return Ok(Inspection::Unreadable),
    };
    let relative_path = entry
        .path()
        .strip_prefix(root)
        .map_err(|e| problem("catalog_path", e))?
        .to_string_lossy()
        .into_owned();
    Ok(Inspection::Image(ScannedImage {
        relative_path,
        name: entry.file_name().to_string_lossy().into_owned(),
        size: metadata.len().to_string(),
        modified: modified(&metadata),
    }))
}

fn finish_scan(
    catalog: &mut Catalog,
    id: String,
    epoch: &str,
    progress: ScanProgress,
    context: &TaskContext,
) -> Result<ScanReport> {
    // No reconciliation until traversal and its final cancellation checkpoint succeed.
    // Warnings mean the inventory may be incomplete, so unseen files stay available.
    context.checkpoint()?;
    catalog.insert_batch(&id, epoch, &progress.pending)?;
    if progress.warnings == 0 {
        catalog.reconcile_scan(&id, epoch)?;
    }
    context.progress(Progress {
        completed: u64::from(progress.discovered).into(),
        total: Some(u64::from(progress.discovered).into()),
        message: Some("Carpeta actualizada".into()),
    })?;
    Ok(ScanReport {
        root_id: id,
        discovered: progress.discovered,
        warnings: progress.warnings,
    })
}

fn supported(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp"
        )
    })
}
