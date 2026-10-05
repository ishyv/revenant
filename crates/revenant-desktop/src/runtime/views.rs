//! Native folder ownership and bounded reactive result windows.
//!
//! A folder outlives the picker. Views keep only their current window's resource
//! references; tasks capture leases before a window can release those references.
//! Query changes never mutate a frozen batch selection.

use super::{DesktopRuntime, Error, Result, UpdateSink, id, lock};
use crate::{
    file_port::FilePort,
    files::{FileQuery, FolderIndex, IndexState},
};
use revenant_core::FileEntry;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) struct IndexRun {
    pub index: Arc<FolderIndex>,
    pub cancelled: Arc<AtomicBool>,
}
pub(crate) struct FolderRecord {
    pub id: String,
    pub scope: String,
    pub path: PathBuf,
    pub indexes: Mutex<HashMap<bool, Arc<IndexRun>>>,
}
impl Drop for FolderRecord {
    fn drop(&mut self) {
        for index in lock(&self.indexes).values() {
            index.cancelled.store(true, Ordering::Release);
        }
    }
}
pub(crate) struct ViewRecord {
    pub id: String,
    pub scope: String,
    pub folder: Arc<FolderRecord>,
    pub index: Arc<IndexRun>,
    pub query: Mutex<FileQuery>,
    pub window: Mutex<(u64, u32)>,
    pub generation: AtomicU64,
    pub disposed: AtomicBool,
    pub entries: Mutex<Vec<FileEntry>>,
    pub sink: Mutex<Option<UpdateSink>>,
    pub last_publish: Mutex<Instant>,
    pub publish_lock: Mutex<()>,
    // Exactly one prior window is retained until its replacement is acknowledged.
    pub retired: Mutex<Option<Vec<FileEntry>>>,
    pub published: Mutex<Option<Value>>,
    pub revision: AtomicU64,
    pub dirty: AtomicBool,
}

impl DesktopRuntime {
    /// Admits a selected native directory without reading its contents.
    ///
    /// Platform adapters call this after a picker or bookmark restore. No count
    /// confirmation or upload occurs. Traversal starts only when a view is created.
    pub fn open_folder(&self, scope: &str, path: PathBuf) -> Result<Value> {
        self.accepting()?;
        self.scope_window(scope)?;
        if lock(&self.inner.folders).len() >= self.inner.options.max_views {
            return Err(Error::new(
                "folder_limit",
                "Too many owned folders; dispose folders no longer in use",
            ));
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(super::io_error)?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if !metadata.is_dir() || metadata.file_type().is_symlink() || reparse {
            return Err(Error::new(
                "invalid_folder",
                "Choose a directory that is not a symbolic link or junction",
            ));
        }
        let path = std::fs::canonicalize(path).map_err(super::io_error)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let folder = id("folder");
        lock(&self.inner.folders).insert(
            folder.clone(),
            Arc::new(FolderRecord {
                id: folder.clone(),
                scope: scope.into(),
                path: path.clone(),
                indexes: Mutex::new(HashMap::new()),
            }),
        );
        Ok(
            json!({"folder":folder,"name":name,"path":path.to_string_lossy(),"bookmark":{"path":path.to_string_lossy()}}),
        )
    }

    pub(crate) fn folder(&self, scope: &str, id: &str) -> Result<Arc<FolderRecord>> {
        let folder = lock(&self.inner.folders)
            .get(id)
            .cloned()
            .ok_or_else(|| Error::new("folder_disposed", "Folder has been disposed"))?;
        if folder.scope != scope {
            return Err(Error::new(
                "scope_mismatch",
                "Folder belongs to another scope",
            ));
        }
        Ok(folder)
    }
    pub(crate) fn view(&self, scope: &str, id: &str) -> Result<Arc<ViewRecord>> {
        let view = lock(&self.inner.views)
            .get(id)
            .cloned()
            .ok_or_else(|| Error::new("view_disposed", "File view has been disposed"))?;
        if view.scope != scope {
            return Err(Error::new(
                "scope_mismatch",
                "File view belongs to another scope",
            ));
        }
        Ok(view)
    }

    fn index(&self, folder: &Arc<FolderRecord>, recursive: bool) -> Result<Arc<IndexRun>> {
        let mut indexes = lock(&folder.indexes);
        if let Some(index) = indexes.get(&recursive) {
            return Ok(index.clone());
        }
        let admission = self
            .inner
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::new("queue_full", "Native work queue is full").retryable(true))?;
        let index = Arc::new(IndexRun {
            index: FolderIndex::new(folder.path.clone(), &self.inner.cache_dir, recursive)?,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        indexes.insert(recursive, index.clone());
        let runtime = self.clone();
        let run = index.clone();
        let folder_id = folder.id.clone();
        tokio::spawn(async move {
            let Ok(worker) = runtime.inner.workers.clone().acquire_owned().await else {
                return;
            };
            let rt = runtime.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let _worker = worker;
                let _admission = admission;
                let _ = run.index.scan(&run.cancelled, |_| {
                    let views: Vec<_> = lock(&rt.inner.views)
                        .values()
                        .filter(|v| v.folder.id == folder_id && Arc::ptr_eq(&v.index, &run))
                        .cloned()
                        .collect();
                    for view in views {
                        let terminal = run.index.status().is_ok_and(|s| {
                            !matches!(s.state, IndexState::Pending | IndexState::Scanning)
                        });
                        if terminal
                            || lock(&view.last_publish).elapsed() >= Duration::from_millis(100)
                        {
                            if lock(&view.retired).is_some() {
                                view.dirty.store(true, Ordering::Release);
                                continue;
                            }
                            if let Ok(snapshot) = rt.publish_view(&view) {
                                if let Some(sink) = lock(&view.sink).clone() {
                                    sink(snapshot);
                                }
                            }
                        }
                    }
                });
            })
            .await;
        });
        Ok(index)
    }

    /// Creates a reactive native query; returned snapshots contain at most 512 entries.
    pub fn query_folder(
        &self,
        scope: &str,
        folder_id: &str,
        options: Value,
        sink: Option<UpdateSink>,
    ) -> Result<Value> {
        self.query_folder_at(scope, folder_id, options, sink, 0, 128, 1)
    }

    pub(crate) fn query_folder_at(
        &self,
        scope: &str,
        folder_id: &str,
        options: Value,
        sink: Option<UpdateSink>,
        offset: u64,
        limit: u32,
        generation: u64,
    ) -> Result<Value> {
        self.accepting()?;
        if !(1..=512).contains(&limit) {
            return Err(Error::new(
                "invalid_window",
                "Window size must be between 1 and 512",
            ));
        }
        if lock(&self.inner.views).len() >= self.inner.options.max_views {
            return Err(Error::new("view_limit", "Too many active file views"));
        }
        let folder = self.folder(scope, folder_id)?;
        let recursive = options
            .get("recursive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut query_options = options.clone();
        if let Some(object) = query_options.as_object_mut() {
            object.remove("recursive");
        }
        if query_options.get("sort").and_then(Value::as_str) == Some("relativePath") {
            query_options["sort"] = Value::String("path".into());
        }
        let query: FileQuery = serde_json::from_value(query_options)?;
        let index = self.index(&folder, recursive)?;
        // Validate query before publication; malformed ordering never creates a view.
        index.index.query(&query, 0, 128)?;
        let view = Arc::new(ViewRecord {
            id: id("view"),
            scope: scope.into(),
            folder,
            index,
            query: Mutex::new(query),
            window: Mutex::new((offset, limit)),
            generation: AtomicU64::new(generation),
            disposed: AtomicBool::new(false),
            entries: Mutex::new(Vec::new()),
            sink: Mutex::new(sink),
            last_publish: Mutex::new(Instant::now()),
            publish_lock: Mutex::new(()),
            retired: Mutex::new(None),
            published: Mutex::new(None),
            revision: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
        });
        lock(&self.inner.views).insert(view.id.clone(), view.clone());
        Ok(json!({"view":view.id,"snapshot":self.publish_view(&view)?}))
    }

    pub(crate) fn publish_view(&self, view: &Arc<ViewRecord>) -> Result<Value> {
        let _publishing = lock(&view.publish_lock);
        if view.disposed.load(Ordering::Acquire) {
            return Err(Error::new("view_disposed", "File view has been disposed"));
        }
        if lock(&view.retired).is_some() {
            view.dirty.store(true, Ordering::Release);
            return lock(&view.published).clone().ok_or_else(|| {
                Error::new(
                    "window_pending",
                    "Window publication is awaiting acknowledgment",
                )
                .retryable(true)
            });
        }
        let query = lock(&view.query).clone();
        let (offset, limit) = *lock(&view.window);
        let generation = view.generation.load(Ordering::Acquire);
        let page = view.index.index.query(&query, offset, limit)?;
        if lock(&view.published).as_ref().is_some_and(|last| {
            last["indexGeneration"].as_u64() == Some(page.generation)
                && last["generation"].as_u64() == Some(generation)
                && last["offset"].as_u64() == Some(offset)
                && last["limit"].as_u64() == Some(limit as u64)
        }) {
            return Ok(lock(&view.published).clone().unwrap());
        }
        let mut entries = Vec::with_capacity(page.items.len());
        let previous: HashMap<_, _> = lock(&view.entries)
            .iter()
            .cloned()
            .map(|entry| (entry.relative_path.clone(), entry))
            .collect();
        let mut created = Vec::new();
        for item in page.items {
            if let Some(old) = previous.get(&item.metadata.relative_path).filter(|old| {
                old.name == item.metadata.name
                    && old.size == item.metadata.size
                    && old.mime == item.metadata.mime
                    && old.modified == item.metadata.modified
            }) {
                entries.push(old.clone());
                continue;
            }
            let port = FilePort::new(item.path, item.metadata.clone());
            match self.inner.resources.register_file_owned(
                &view.scope,
                item.metadata,
                port,
                Some("files"),
                None,
            ) {
                Ok(entry) => {
                    created.push(entry.handle.clone());
                    entries.push(entry);
                }
                Err(error) => {
                    for handle in created {
                        let _ = self.inner.resources.release(&view.scope, &handle);
                    }
                    return Err(error);
                }
            }
        }
        let old = std::mem::replace(&mut *lock(&view.entries), entries.clone());
        *lock(&view.retired) = Some(
            old.into_iter()
                .filter(|old| !entries.iter().any(|entry| entry.handle == old.handle))
                .collect(),
        );
        *lock(&view.last_publish) = Instant::now();
        let state = match page.state {
            IndexState::Pending | IndexState::Scanning => "loading",
            IndexState::Ready => "ready",
            _ => "failed",
        };
        let warnings: Vec<_> = page.warnings.items.into_iter().map(|warning| json!({"code":"file_scan_warning","message":warning.message,"details":{"path":warning.path},"retryable":false})).collect();
        let revision = view.revision.fetch_add(1, Ordering::AcqRel) + 1;
        let snapshot = json!({"view":view.id,"items":entries,"offset":page.offset,"limit":limit,"total":page.total,"discovered":page.discovered,"state":state,
            "generation":generation,"revision":revision,"indexGeneration":page.generation,"warnings":warnings,"warningCount":page.warnings.total});
        *lock(&view.published) = Some(snapshot.clone());
        Ok(snapshot)
    }

    /// Changes query or window parameters and rejects stale requested generations.
    pub fn update_view(
        &self,
        scope: &str,
        view_id: &str,
        query: Option<Value>,
        offset: Option<u64>,
        limit: Option<u32>,
        generation: Option<u64>,
    ) -> Result<Value> {
        let view = self.view(scope, view_id)?;
        let _publishing = lock(&view.publish_lock);
        if lock(&view.retired).is_some() {
            return Err(Error::new(
                "window_pending",
                "Acknowledge the current window before requesting another",
            )
            .retryable(true));
        }
        if let Some(query) = query {
            let query: FileQuery = serde_json::from_value(query)?;
            view.index.index.query(&query, 0, 1)?;
            *lock(&view.query) = query;
            view.generation.store(
                generation.unwrap_or_else(|| view.generation.load(Ordering::Acquire) + 1),
                Ordering::Release,
            );
        } else if generation.is_some_and(|g| g != view.generation.load(Ordering::Acquire)) {
            return Err(Error::new(
                "stale_query",
                "Query changed before this window request completed",
            )
            .retryable(true));
        }
        let mut window = lock(&view.window);
        if let Some(offset) = offset {
            window.0 = offset;
        }
        if let Some(limit) = limit {
            if limit == 0 || limit > 512 {
                return Err(Error::new(
                    "invalid_window",
                    "Window size must be between 1 and 512",
                ));
            }
            window.1 = limit;
        }
        drop(window);
        drop(_publishing);
        self.publish_view(&view)
    }

    /// Releases a view's window references while preserving already admitted tasks.
    pub fn dispose_view(&self, scope: &str, view_id: &str) -> Result<()> {
        let view = self.view(scope, view_id)?;
        let _publishing = lock(&view.publish_lock);
        view.disposed.store(true, Ordering::Release);
        lock(&view.sink).take();
        let old = std::mem::take(&mut *lock(&view.entries));
        for entry in old {
            let _ = self.inner.resources.release(scope, &entry.handle);
        }
        if let Some(retired) = lock(&view.retired).take() {
            for entry in retired {
                let _ = self.inner.resources.release(scope, &entry.handle);
            }
        }
        lock(&self.inner.views).remove(view_id);
        Ok(())
    }

    /// Acknowledges one offered window after its pending selection captures finish.
    /// Stale acknowledgments never revoke a newer page; deferred scans are coalesced.
    pub fn acknowledge_view(&self, scope: &str, view_id: &str, revision: u64) -> Result<()> {
        let view = self.view(scope, view_id)?;
        let publishing = lock(&view.publish_lock);
        if revision != view.revision.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(old) = lock(&view.retired).take() {
            for entry in old {
                let _ = self.inner.resources.release(scope, &entry.handle);
            }
        }
        let refresh = view.dirty.swap(false, Ordering::AcqRel);
        drop(publishing);
        if refresh {
            let snapshot = self.publish_view(&view)?;
            if let Some(sink) = lock(&view.sink).clone() {
                sink(snapshot);
            }
        }
        Ok(())
    }

    /// Releases folder ownership; existing views and admitted batches keep their leases.
    pub fn dispose_folder(&self, scope: &str, folder_id: &str) -> Result<()> {
        self.folder(scope, folder_id)?;
        lock(&self.inner.folders).remove(folder_id);
        Ok(())
    }
}
