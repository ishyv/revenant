//! Bounded native admission, captured inputs, and disk-backed batch outcomes.
//!
//! Queued jobs pin provider generations and inputs before returning to Svelte.
//! A batch pins one native selector, then owns a bounded set of in-flight files. Completed
//! outcomes are committed in chunks; task snapshots retain counters, not inventory.

use super::inspect::TraceContext;
use super::views::{IndexLease, ViewRecord};
use super::{DesktopRuntime, Error, Result, UpdateSink, id, lock};
use crate::{
    file_port::FilePort,
    files::IndexState,
    results::{BatchOutcome, ResultStore},
};
use revenant_core::{
    Handle, NativeResourceLease, Progress, ResourceLease, Subscription, TaskContext, TaskSnapshot,
};
use revenant_sdk::PreparedOperation;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) struct JobRecord {
    pub context: TaskContext,
    pub results: Mutex<Option<Arc<ResultStore>>>,
    pub disposed: AtomicBool,
    admitted: Arc<AtomicBool>,
    pub _subscription: Subscription,
    pub resources: Arc<ResultResources>,
    pub finished: AtomicBool,
    pub operation: String,
    pub trace: TraceContext,
}
/// Each scheduled native reader holds a clone. The last exit settles history,
/// even if its async supervisor or independent-batch coordinator unwound.
#[derive(Clone)]
struct ExecutionCompletion {
    _state: Arc<ExecutionCompletionState>,
}
struct ExecutionCompletionState {
    runtime: DesktopRuntime,
    job: Arc<JobRecord>,
}
impl ExecutionCompletion {
    fn new(runtime: DesktopRuntime, job: Arc<JobRecord>) -> Self {
        Self {
            _state: Arc::new(ExecutionCompletionState { runtime, job }),
        }
    }
}
impl Drop for ExecutionCompletionState {
    fn drop(&mut self) {
        // Normal executors already finalized through their core guard. Queue or
        // supervisor failure can happen before begin; cleanup must still settle
        // that logical task without a second observer unwind escaping this Drop.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !self.job.context.snapshot().state.is_terminal() {
                let _ = self.job.context.interrupt();
            }
        }));
        self.runtime.finish_execution(&self.job);
    }
}
/// Result leases are independent of executor caches and live with native history.
pub(crate) struct ResultResources {
    pub leases: Mutex<std::collections::HashMap<Handle, ResourceLease>>,
    pub native_leases: Mutex<std::collections::HashMap<Handle, NativeResourceLease>>,
}
impl ResultResources {
    fn retain(
        &self,
        prepared: &PreparedOperation,
        output: &Value,
        context: &TaskContext,
    ) -> Result<()> {
        let handles = prepared.output_handles(output)?;
        let missing = {
            let retained = lock(&self.leases);
            let native_retained = lock(&self.native_leases);
            handles
                .into_iter()
                .filter(|handle| {
                    if handle.kind == "file" {
                        !retained.contains_key(handle)
                    } else {
                        !native_retained.contains_key(handle)
                    }
                })
                .collect::<std::collections::HashSet<_>>()
        };
        // Lease acquisition checkpoints invoke application callbacks. In particular,
        // reentrant task admission may consult these same history maps.
        let mut additions = Vec::new();
        let mut native_additions = Vec::new();
        for handle in missing {
            if handle.kind == "file" {
                additions.push((handle.clone(), context.lease(&handle)?));
            } else {
                native_additions.push((handle.clone(), context.native_lease(&handle)?));
            }
        }
        // Batch items can publish concurrently. Recheck absence and combined
        // capacity atomically, and drop all candidate leases after unlocking.
        let mut retained = lock(&self.leases);
        let mut native_retained = lock(&self.native_leases);
        let extra = additions
            .iter()
            .filter(|(handle, _)| !retained.contains_key(handle))
            .count()
            + native_additions
                .iter()
                .filter(|(handle, _)| !native_retained.contains_key(handle))
                .count();
        if retained.len() + native_retained.len() + extra > 512 {
            return Err(Error::new(
                "result_resource_limit",
                "A result may retain at most 512 distinct resources; return metadata or dispose earlier results",
            ));
        }
        for (handle, lease) in &additions {
            retained
                .entry(handle.clone())
                .or_insert_with(|| lease.clone());
        }
        for (handle, lease) in &native_additions {
            native_retained
                .entry(handle.clone())
                .or_insert_with(|| lease.clone());
        }
        Ok(())
    }
}
struct FileOwner {
    resources: revenant_core::ResourceRegistry,
    scope: String,
    handle: Handle,
}
impl Drop for FileOwner {
    fn drop(&mut self) {
        let _ = self.resources.release(&self.scope, &self.handle);
    }
}
pub(crate) struct SelectionRecord {
    pub scope: String,
    pub _view: Arc<ViewRecord>,
    pub leases: Vec<ResourceLease>,
}
enum BatchSource {
    Matching {
        view: Arc<ViewRecord>,
        query: crate::files::FileQuery,
        _inventory: IndexLease,
    },
    Captured(Arc<SelectionRecord>),
    Inputs(Vec<(Value, TaskContext)>),
}

enum BatchIterator {
    Matching(crate::files::FileSelection),
    Captured(std::vec::IntoIter<ResourceLease>),
    Inputs(std::vec::IntoIter<(Value, TaskContext)>),
}
struct BatchItems {
    iterator: BatchIterator,
    total: u64,
    _inventory: Option<IndexLease>,
    _selection: Option<Arc<SelectionRecord>>,
}
struct BatchItem {
    ordinal: u64,
    input: Value,
    child: TaskContext,
    _owner: Option<FileOwner>,
}

impl DesktopRuntime {
    fn make_job(
        &self,
        scope: &str,
        operation: &str,
        trace: TraceContext,
        sink: Option<UpdateSink>,
    ) -> Result<Arc<JobRecord>> {
        self.accepting()?;
        self.scope_owner(scope)?;
        let mut jobs = lock(&self.inner.jobs);
        if jobs.len() >= self.inner.options.max_tasks {
            return Err(Error::new(
                "task_limit",
                "Dispose completed tasks before creating more",
            ));
        }
        let context = self.inner.tasks.create(scope, &id("task"))?;
        let admitted = Arc::new(AtomicBool::new(false));
        let publish_admitted = admitted.clone();
        let last = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(1)));
        let trace_runtime = Arc::downgrade(&self.inner);
        let trace_context = trace.clone();
        let trace_operation = operation.to_owned();
        let last_state = Mutex::new(None);
        let subscription = context.subscribe(Arc::new(move |snapshot: TaskSnapshot| {
            // Failed admission reports the invoke error, never a misleading
            // cancellation snapshot from its private cleanup task.
            if !publish_admitted.load(Ordering::Acquire) {
                return;
            }
            let mut previous = lock(&last_state);
            if previous.as_ref() != Some(&snapshot.state) {
                *previous = Some(snapshot.state);
                if let Some(inner) = trace_runtime.upgrade() {
                    lock(&inner.trace).push(
                        &trace_context,
                        "task.state",
                        Some(&trace_operation),
                        Some(&snapshot.id),
                        Some(snapshot.state),
                        snapshot.error.as_ref().map(|error| error.code.as_str()),
                    );
                }
            }
            drop(previous);
            if let Some(sink) = &sink {
                let publish = {
                    let mut last = lock(&last);
                    let publish =
                        snapshot.state.is_terminal() || last.elapsed() >= Duration::from_millis(50);
                    if publish {
                        *last = Instant::now();
                    }
                    publish
                };
                // A sink may cancel or dispose this task synchronously.
                if publish {
                    if let Ok(value) = serde_json::to_value(snapshot) {
                        sink(value);
                    }
                }
            }
        }));
        let job = Arc::new(JobRecord {
            context,
            admitted,
            results: Mutex::new(None),
            disposed: AtomicBool::new(false),
            _subscription: subscription,
            resources: Arc::new(ResultResources {
                leases: Mutex::new(std::collections::HashMap::new()),
                native_leases: Mutex::new(std::collections::HashMap::new()),
            }),
            finished: AtomicBool::new(false),
            operation: operation.into(),
            trace,
        });
        jobs.insert(job.context.id().into(), job.clone());
        Ok(job)
    }

    /// Validates and captures one operation's inputs before native queue admission.
    pub fn run_task(
        &self,
        scope: &str,
        operation: &str,
        input: Value,
        sink: Option<UpdateSink>,
    ) -> Result<TaskSnapshot> {
        self.run_task_traced(scope, operation, input, sink, TraceContext::native())
    }

    pub(crate) fn run_task_traced(
        &self,
        scope: &str,
        operation: &str,
        input: Value,
        sink: Option<UpdateSink>,
        trace: TraceContext,
    ) -> Result<TaskSnapshot> {
        if serde_json::to_vec(&input)?.len() > self.inner.options.max_contract_bytes {
            return Err(Error::new(
                "input_too_large",
                "Use a native resource for large operation inputs",
            ));
        }
        let admission = self
            .inner
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::new("queue_full", "Native work queue is full").retryable(true))?;
        let prepared = self.inner.registry.prepare_operation(operation)?;
        let job = self.make_job(scope, operation, trace, sink)?;
        if let Err(error) = self.admit_input(&prepared, &input, &job.context) {
            let _ = job.context.request_cancel();
            lock(&self.inner.jobs).remove(job.context.id());
            let _ = self.inner.tasks.remove(scope, job.context.id());
            return Err(error);
        }
        job.admitted.store(true, Ordering::Release);
        let initial = job.context.snapshot();
        self.trace_task_admission(&job);
        let runtime = self.clone();
        let completion = ExecutionCompletion::new(runtime.clone(), job.clone());
        tokio::spawn(async move {
            let worker = loop {
                if job.context.snapshot().state.is_terminal() {
                    return;
                }
                tokio::select! {
                    permit = runtime.inner.work.acquire(prepared.descriptor().work_class) => { let Ok(permit) = permit else { return }; break permit; }
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            };
            let rt = runtime.clone();
            let native = job.clone();
            let executor = tokio::runtime::Handle::current();
            let result = tokio::task::spawn_blocking(move || {
                let _completion = completion;
                let _admission = admission;
                let _worker = rt.inner.work.enter(worker);
                let execution = native.context.begin()?;
                let mut result = executor.block_on(prepared.execute(input, native.context.clone()));
                if result.as_ref().is_ok_and(|value| {
                    serde_json::to_vec(value)
                        .is_ok_and(|bytes| bytes.len() > rt.inner.options.max_contract_bytes)
                }) {
                    result = Err(Error::new(
                        "output_too_large",
                        "Return a native resource or paged result for large output",
                    ));
                }
                if let Ok(output) = &result
                    && let Err(error) = native.resources.retain(&prepared, output, &native.context)
                {
                    result = Err(error);
                }
                execution.finish(result)?;
                Ok::<(), Error>(())
            })
            .await;
            if let Err(error) = result {
                let _ = job.context.request_cancel();
                let _ = error;
            }
        });
        Ok(initial)
    }

    /// Captures selected window entries so later viewport changes cannot revoke them.
    pub fn set_selection(
        &self,
        scope: &str,
        view_id: &str,
        selection_id: Option<&str>,
        handles: Vec<Handle>,
    ) -> Result<Value> {
        self.scope_owner(scope)?;
        if handles.len() > 512 {
            return Err(Error::new(
                "selection_limit",
                "Use allMatching for selections above 512 explicit entries",
            ));
        }
        let view = self.view(scope, view_id)?;
        let previous = selection_id.and_then(|id| lock(&self.inner.selections).get(id).cloned());
        if previous
            .as_ref()
            .is_some_and(|old| old.scope != scope || old._view.id != view_id)
        {
            return Err(Error::new(
                "scope_mismatch",
                "Selection belongs to another scope or view",
            ));
        }
        if previous.is_none() && lock(&self.inner.selections).len() >= self.inner.options.max_views
        {
            return Err(Error::new("selection_limit", "Too many native selections"));
        }
        let mut leases = Vec::with_capacity(handles.len());
        for handle in handles {
            if let Some(lease) = previous
                .as_ref()
                .and_then(|old| old.leases.iter().find(|lease| lease.handle() == &handle))
            {
                leases.push(lease.clone());
            } else {
                let visible = lock(&view.entries)
                    .iter()
                    .any(|entry| entry.handle == handle)
                    || lock(&view.retired)
                        .as_ref()
                        .is_some_and(|entries| entries.iter().any(|entry| entry.handle == handle));
                if !visible {
                    return Err(Error::new(
                        "invalid_selection",
                        "File does not belong to the admitted view window",
                    ));
                }
                leases.push(self.lease_resource(scope, &handle)?);
            }
        }
        let selection = selection_id
            .map(str::to_owned)
            .unwrap_or_else(|| id("selection"));
        let mut selections = lock(&self.inner.selections);
        if let Some(old) = selections.get(&selection)
            && old.scope != scope
        {
            return Err(Error::new(
                "scope_mismatch",
                "Selection belongs to another scope",
            ));
        }
        let count = leases.len();
        selections.insert(
            selection.clone(),
            Arc::new(SelectionRecord {
                scope: scope.into(),
                _view: view,
                leases,
            }),
        );
        Ok(json!({"selection":selection,"count":count}))
    }

    /// Releases native selection ownership; admitted jobs retain their captured selection.
    pub fn dispose_selection(&self, scope: &str, selection: &str) -> Result<()> {
        let mut selections = lock(&self.inner.selections);
        if selections.get(selection).is_some_and(|s| s.scope != scope) {
            return Err(Error::new(
                "scope_mismatch",
                "Selection belongs to another scope",
            ));
        }
        selections.remove(selection);
        Ok(())
    }

    /// Submits a native query selection; discovery and preparation remain cancellable.
    pub fn run_batch(
        &self,
        scope: &str,
        operation: &str,
        selection: Value,
        sink: Option<UpdateSink>,
    ) -> Result<TaskSnapshot> {
        self.run_batch_traced(scope, operation, selection, sink, TraceContext::native())
    }

    pub(crate) fn run_batch_traced(
        &self,
        scope: &str,
        operation: &str,
        selection: Value,
        sink: Option<UpdateSink>,
        trace: TraceContext,
    ) -> Result<TaskSnapshot> {
        let admission = self
            .inner
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::new("queue_full", "Native work queue is full").retryable(true))?;
        let prepared = self.inner.registry.prepare_operation(operation)?;
        let job = self.make_job(scope, operation, trace, sink)?;
        let source_result = (|| {
            Ok(if let Some(inputs) = selection.get("inputs") {
                let inputs: Vec<Value> = serde_json::from_value(inputs.clone())?;
                if inputs.len() > 512
                    || serde_json::to_vec(&inputs)?.len() > self.inner.options.max_contract_bytes
                {
                    return Err(Error::new(
                        "batch_limit",
                        "JSON batches allow at most 512 records and 1 MiB",
                    ));
                }
                let total = inputs.len() as u32;
                let mut children = Vec::with_capacity(inputs.len());
                for (index, input) in inputs.into_iter().enumerate() {
                    let child = if prepared.descriptor().independent {
                        job.context.independent_batch_child(index as u32, total)?
                    } else {
                        job.context.batch_child(index as u32, total)?
                    };
                    self.admit_input(&prepared, &input, &child)?;
                    children.push((input, child));
                }
                BatchSource::Inputs(children)
            } else if let Some(selection_id) = selection.get("id").and_then(Value::as_str) {
                let captured = lock(&self.inner.selections)
                    .get(selection_id)
                    .cloned()
                    .ok_or_else(|| {
                        Error::new("selection_disposed", "Selection has been disposed")
                    })?;
                if captured.scope != scope {
                    return Err(Error::new(
                        "scope_mismatch",
                        "Selection belongs to another scope",
                    ));
                }
                BatchSource::Captured(captured)
            } else {
                let view_id = selection
                    .get("view")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Error::new("invalid_selection", "A native selection must name its view")
                    })?;
                let view = self.view(scope, view_id)?;
                // Query and generation form one admission decision, including for
                // embedded callers updating a view outside the IPC control queue.
                let publishing = lock(&view.publish_lock);
                if selection
                    .get("generation")
                    .and_then(Value::as_u64)
                    .is_some_and(|generation| generation != view.generation.load(Ordering::Acquire))
                {
                    return Err(Error::new(
                        "stale_selection",
                        "Native query changed before batch admission",
                    ));
                }
                if selection
                    .get("allMatching")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    let query = lock(&view.query).clone();
                    drop(publishing);
                    let inventory = view.index.retain()?;
                    BatchSource::Matching {
                        view,
                        query,
                        _inventory: inventory,
                    }
                } else {
                    drop(publishing);
                    let handles: Vec<Handle> = serde_json::from_value(
                        selection
                            .get("handles")
                            .cloned()
                            .unwrap_or_else(|| json!([])),
                    )?;
                    let captured = self.set_selection(scope, view_id, None, handles)?;
                    let selection_id = captured["selection"].as_str().unwrap();
                    let captured = lock(&self.inner.selections).remove(selection_id).unwrap();
                    BatchSource::Captured(captured)
                }
            })
        })();
        let source = match source_result {
            Ok(source) => source,
            Err(error) => {
                self.remove_unadmitted(&job);
                return Err(error);
            }
        };
        let store = match ResultStore::new_with_owner(&self.inner.cache_dir, job.resources.clone())
        {
            Ok(store) => store,
            Err(error) => {
                self.remove_unadmitted(&job);
                return Err(error);
            }
        };
        *lock(&job.results) = Some(store.clone());
        job.admitted.store(true, Ordering::Release);
        let initial = job.context.snapshot();
        self.trace_task_admission(&job);
        let runtime = self.clone();
        let completion = ExecutionCompletion::new(runtime.clone(), job.clone());
        tokio::spawn(async move {
            if job.context.snapshot().state.is_terminal() {
                return;
            }
            let Ok(execution) = job.context.begin() else {
                return;
            };
            let reference = json!({"task":job.context.id(),"paged":true});
            if let BatchSource::Matching { view, .. } = &source {
                let _ = job.context.progress(Progress {
                    completed: 0.into(),
                    total: None,
                    message: Some("Discovering matching files".into()),
                });
                loop {
                    if let Err(error) = job.context.checkpoint() {
                        let _ = execution.finish_batch(reference, error.is_cancelled());
                        return;
                    }
                    match view.index.index.status() {
                        Ok(status) if status.state == IndexState::Ready => break,
                        Ok(status)
                            if matches!(
                                status.state,
                                IndexState::Failed | IndexState::Cancelled
                            ) =>
                        {
                            let _ = execution.finish(Err(Error::new(
                                "index_incomplete",
                                "Folder discovery did not complete",
                            )));
                            return;
                        }
                        Err(error) => {
                            let _ = execution.finish(Err(error));
                            return;
                        }
                        _ => tokio::time::sleep(Duration::from_millis(20)).await,
                    }
                }
            }
            let independent = prepared.descriptor().independent;
            let worker = if independent {
                None
            } else {
                Some(loop {
                    if job.context.checkpoint().is_err() {
                        let _ = execution.finish_batch(reference, true);
                        return;
                    }
                    tokio::select! {
                        permit = runtime.inner.work.acquire(prepared.descriptor().work_class) => { let Ok(permit) = permit else { return }; break permit; }
                        _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                    }
                })
            };
            let native = job.clone();
            let rt = runtime.clone();
            let executor = tokio::runtime::Handle::current();
            let _ = tokio::task::spawn_blocking(move || {
                let _completion = completion;
                // Move the captured core guard into a local so it is destroyed
                // before completion bookkeeping when this worker unwinds.
                let execution = execution;
                let _worker = worker.map(|permit| rt.inner.work.enter(permit));
                let _admission = admission;
                let result = if independent {
                    executor.block_on(rt.execute_independent_batch(
                        native.clone(),
                        Arc::new(prepared),
                        source,
                        store,
                        executor.clone(),
                        &_completion,
                    ))
                } else {
                    rt.execute_batch(&native, &prepared, source, &store, &executor)
                };
                match result {
                    Ok(()) => {
                        let _ = execution.finish_batch(reference, false);
                    }
                    Err(error) if error.is_cancelled() => {
                        let _ = execution.finish_batch(reference, true);
                    }
                    Err(error) => {
                        let _ = execution.finish(Err(error));
                    }
                }
            })
            .await;
        });
        Ok(initial)
    }

    fn execute_batch(
        &self,
        job: &JobRecord,
        prepared: &PreparedOperation,
        source: BatchSource,
        store: &Arc<ResultStore>,
        executor: &tokio::runtime::Handle,
    ) -> Result<()> {
        let context = &job.context;
        let mut items = self.batch_items(source)?;
        let total = items.total;
        context.progress(Progress {
            completed: 0.into(),
            total: Some(total.into()),
            message: Some("Processing selected files".into()),
        })?;
        let mut ordinal = 0u64;
        let mut pending = Vec::with_capacity(128);
        let result = (|| {
            loop {
                context.checkpoint()?;
                let Some(item) = self.next_batch_item(context, &mut items, ordinal, false)? else {
                    break;
                };
                let outcome = self.execute_batch_item(job, prepared, item, executor)?;
                pending.push(outcome);
                ordinal += 1;
                context.progress(Progress {
                    completed: ordinal.into(),
                    total: Some(total.into()),
                    message: Some("Processing selected items".into()),
                })?;
                if pending.len() == 128 {
                    self.commit_outcomes(context, store, &mut pending)?;
                }
            }
            Ok(())
        })();
        // Persist completed work even when the next item acknowledges cancellation.
        self.commit_outcomes(context, store, &mut pending)?;
        result
    }

    fn batch_items(&self, source: BatchSource) -> Result<BatchItems> {
        let items = match source {
            BatchSource::Matching {
                view,
                query,
                _inventory,
            } => {
                let selection = view.index.index.selection(&query)?;
                BatchItems {
                    total: selection.total(),
                    iterator: BatchIterator::Matching(selection),
                    _inventory: Some(_inventory),
                    _selection: None,
                }
            }
            BatchSource::Captured(selection) => BatchItems {
                total: selection.leases.len() as u64,
                iterator: BatchIterator::Captured(selection.leases.clone().into_iter()),
                _inventory: None,
                _selection: Some(selection),
            },
            BatchSource::Inputs(inputs) => BatchItems {
                total: inputs.len() as u64,
                iterator: BatchIterator::Inputs(inputs.into_iter()),
                _inventory: None,
                _selection: None,
            },
        };
        u32::try_from(items.total).map_err(|_| {
            Error::new(
                "batch_too_large",
                "Batch exceeds supported item-index space",
            )
        })?;
        Ok(items)
    }

    fn next_batch_item(
        &self,
        context: &TaskContext,
        items: &mut BatchItems,
        ordinal: u64,
        independent: bool,
    ) -> Result<Option<BatchItem>> {
        let (input, retained, owner, admitted_child) = match &mut items.iterator {
            BatchIterator::Matching(selection) => {
                let Some(item) = selection.next() else {
                    return Ok(None);
                };
                let item = item?;
                let entry = self.inner.resources.register_file_owned(
                    context.scope(),
                    item.metadata.clone(),
                    FilePort::new(item.path, item.metadata),
                    Some("files"),
                    None,
                )?;
                (
                    serde_json::to_value(&entry)?,
                    None,
                    Some(FileOwner {
                        resources: self.inner.resources.clone(),
                        scope: context.scope().into(),
                        handle: entry.handle,
                    }),
                    None,
                )
            }
            BatchIterator::Captured(leases) => {
                let Some(lease) = leases.next() else {
                    return Ok(None);
                };
                (
                    serde_json::to_value(lease.metadata().clone().entry(lease.handle().clone())?)?,
                    Some(lease),
                    None,
                    None,
                )
            }
            BatchIterator::Inputs(inputs) => {
                let Some((input, child)) = inputs.next() else {
                    return Ok(None);
                };
                (input, None, None, Some(child))
            }
        };
        let child = match admitted_child {
            Some(child) => child,
            None if independent => {
                context.independent_batch_child(ordinal as u32, items.total as u32)?
            }
            None => context.batch_child(ordinal as u32, items.total as u32)?,
        };
        if let Some(lease) = retained {
            child.capture(lease)?;
        }
        Ok(Some(BatchItem {
            ordinal,
            input,
            child,
            _owner: owner,
        }))
    }

    fn execute_batch_item(
        &self,
        job: &JobRecord,
        prepared: &PreparedOperation,
        item: BatchItem,
        executor: &tokio::runtime::Handle,
    ) -> Result<BatchOutcome> {
        let BatchItem {
            ordinal,
            input,
            child,
            _owner,
        } = item;
        child.checkpoint()?;
        let execution = child.begin()?;
        let mut output = executor.block_on(prepared.execute(input.clone(), child.clone()));
        if let Ok(value) = &output {
            if serde_json::to_vec(value)?.len() > self.inner.options.max_contract_bytes {
                output = Err(Error::new(
                    "output_too_large",
                    "Return a native resource for large results",
                ));
            } else if let Err(error) = job.resources.retain(prepared, value, &child) {
                output = Err(error);
            }
        }
        execution.finish(output.clone())?;
        match output {
            Ok(output) => Ok(BatchOutcome {
                ordinal,
                input,
                output: Some(output),
                error: None,
            }),
            Err(error) if error.is_cancelled() => Err(error),
            Err(error) => {
                lock(&self.inner.trace).batch_item_failed(
                    &job.trace,
                    &job.operation,
                    job.context.id(),
                    ordinal,
                    &error.code,
                );
                Ok(BatchOutcome {
                    ordinal,
                    input,
                    output: None,
                    error: Some(serde_json::to_value(error)?),
                })
            }
        }
    }

    async fn execute_independent_batch(
        &self,
        job: Arc<JobRecord>,
        prepared: Arc<PreparedOperation>,
        source: BatchSource,
        store: Arc<ResultStore>,
        executor: tokio::runtime::Handle,
        completion: &ExecutionCompletion,
    ) -> Result<()> {
        let mut items = self.batch_items(source)?;
        let context = &job.context;
        context.progress(Progress {
            completed: 0.into(),
            total: Some(items.total.into()),
            message: Some("Processing selected items".into()),
        })?;
        let mut running = tokio::task::JoinSet::new();
        let mut ordinal = 0;
        let mut completed = 0;
        let mut pending = Vec::with_capacity(128);
        let mut failure = None;
        let maximum_in_flight = self.inner.options.concurrency.min(128);
        while ordinal < items.total || !running.is_empty() {
            if failure.is_none() {
                failure = context.checkpoint().err();
            }
            if failure.is_some() && running.is_empty() {
                break;
            }
            let can_schedule =
                failure.is_none() && ordinal < items.total && running.len() < maximum_in_flight;
            tokio::select! {
                permit = self.inner.work.acquire(prepared.descriptor().work_class), if can_schedule => {
                    let permit = match permit { Ok(permit) => permit, Err(_) => { failure = Some(Error::new("app_closing", "Native executor is closed")); continue; } };
                    if let Err(error) = context.checkpoint() { failure = Some(error); continue; }
                    let item = match self.next_batch_item(context, &mut items, ordinal, true) {
                        Ok(Some(item)) => item,
                        Ok(None) => { ordinal = items.total; continue; }
                        Err(error) => { failure = Some(error); let _ = context.request_cancel(); continue; }
                    };
                    ordinal += 1;
                    let runtime = self.clone();
                    let native = job.clone();
                    let prepared = prepared.clone();
                    let executor = executor.clone();
                    let completion = completion.clone();
                    running.spawn_blocking(move || {
                        let _completion = completion;
                        let _permit = runtime.inner.work.enter(permit);
                        runtime.execute_batch_item(&native, &prepared, item, &executor)
                    });
                }
                result = running.join_next(), if !running.is_empty() => {
                    match result {
                        Some(Ok(Ok(outcome))) => {
                            pending.push(outcome);
                            completed += 1;
                            let _ = context.progress(Progress { completed: completed.into(), total: Some(items.total.into()), message: Some("Processing selected items".into()) });
                            if pending.len() == 128
                                && let Err(error) = self.commit_outcomes(context, &store, &mut pending) { failure = Some(error); let _ = context.request_cancel(); }
                        }
                        Some(Ok(Err(error))) => { if failure.is_none() { failure = Some(error); } let _ = context.request_cancel(); }
                        Some(Err(error)) => { failure = Some(Error::new("native_executor", error.to_string())); let _ = context.request_cancel(); }
                        None => {}
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
        // Never abandon a native reader on cancellation: all child threads have
        // exited before these source/provider guards and admission are released.
        self.commit_outcomes(context, &store, &mut pending)?;
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn remove_unadmitted(&self, job: &Arc<JobRecord>) {
        let _ = job.context.request_cancel();
        lock(&self.inner.jobs).remove(job.context.id());
        let _ = self
            .inner
            .tasks
            .remove(job.context.scope(), job.context.id());
    }
    fn admit_input(
        &self,
        prepared: &PreparedOperation,
        input: &Value,
        context: &TaskContext,
    ) -> Result<()> {
        for handle in prepared.input_handles(input)? {
            if handle.kind == "file" {
                context.capture(self.lease_resource(context.scope(), &handle)?)?;
            } else {
                context.capture_native(self.lease_native_resource(context.scope(), &handle)?)?;
            }
        }
        prepared.prepare_input(input, context)
    }
    pub(crate) fn lease_resource(&self, scope: &str, handle: &Handle) -> Result<ResourceLease> {
        self.scope_owner(scope)?;
        if handle.scope != scope {
            return Err(Error::new(
                "scope_mismatch",
                "Resource belongs to another scope",
            ));
        }
        match self.inner.resources.lease(scope, handle) {
            Ok(lease) => Ok(lease),
            Err(error) => {
                for job in lock(&self.inner.jobs)
                    .values()
                    .filter(|job| job.context.scope() == scope)
                {
                    if let Some(lease) = lock(&job.resources.leases).get(handle) {
                        return Ok(lease.clone());
                    }
                }
                for selection in lock(&self.inner.selections)
                    .values()
                    .filter(|selection| selection.scope == scope)
                {
                    if let Some(lease) = selection
                        .leases
                        .iter()
                        .find(|lease| lease.handle() == handle)
                    {
                        return Ok(lease.clone());
                    }
                }
                Err(error)
            }
        }
    }

    pub(crate) fn lease_native_resource(
        &self,
        scope: &str,
        handle: &Handle,
    ) -> Result<NativeResourceLease> {
        self.scope_owner(scope)?;
        if handle.scope != scope {
            return Err(Error::new(
                "scope_mismatch",
                "Resource belongs to another scope",
            ));
        }
        match self.inner.resources.lease_native_handle(scope, handle) {
            Ok(lease) => Ok(lease),
            Err(error) => {
                for job in lock(&self.inner.jobs)
                    .values()
                    .filter(|job| job.context.scope() == scope)
                {
                    if let Some(lease) = lock(&job.resources.native_leases).get(handle)
                        && !lease.is_revoked()
                    {
                        return Ok(lease.clone());
                    }
                }
                Err(error)
            }
        }
    }

    fn commit_outcomes(
        &self,
        context: &TaskContext,
        store: &ResultStore,
        pending: &mut Vec<BatchOutcome>,
    ) -> Result<()> {
        if pending.is_empty() {
            return Ok(());
        }
        let succeeded: Vec<_> = pending.iter().map(|item| item.output.is_some()).collect();
        store.append_many(pending.drain(..))?;
        for success in succeeded {
            context.record_batch_outcome(success)?;
        }
        Ok(())
    }

    pub(crate) fn job(&self, scope: &str, task: &str) -> Result<Arc<JobRecord>> {
        let job = lock(&self.inner.jobs)
            .get(task)
            .cloned()
            .ok_or_else(|| Error::new("task_disposed", "Task history has been disposed"))?;
        if job.context.scope() != scope {
            return Err(Error::new(
                "scope_mismatch",
                "Task belongs to another scope",
            ));
        }
        Ok(job)
    }
    pub(crate) fn cancel_scope_jobs(&self, scope: &str) {
        let jobs: Vec<_> = lock(&self.inner.jobs)
            .values()
            .filter(|job| job.context.scope() == scope)
            .cloned()
            .collect();
        let mut panic = None;
        for job in jobs {
            job.disposed.store(true, Ordering::Release);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = job.context.request_cancel();
            }));
            self.finish_disposed_job(&job);
            if let Err(payload) = outcome {
                if panic.is_none() {
                    panic = Some(payload);
                }
            }
        }
        if let Some(payload) = panic {
            std::panic::resume_unwind(payload);
        }
    }
    fn finish_disposed_job(&self, job: &Arc<JobRecord>) {
        if job.disposed.load(Ordering::Acquire)
            && job.finished.load(Ordering::Acquire)
            && job.context.snapshot().state.is_terminal()
        {
            lock(&self.inner.jobs).remove(job.context.id());
            let _ = self
                .inner
                .tasks
                .remove(job.context.scope(), job.context.id());
        }
    }
    fn finish_execution(&self, job: &Arc<JobRecord>) {
        job.finished.store(true, Ordering::Release);
        self.finish_disposed_job(job);
    }
    /// Requests cancellation; terminal task history remains available until disposal.
    pub fn cancel_task(&self, scope: &str, task: &str) -> Result<TaskSnapshot> {
        self.job(scope, task)?.context.request_cancel()
    }
    /// Returns the current bounded native task snapshot.
    pub fn task_snapshot(&self, scope: &str, task: &str) -> Result<TaskSnapshot> {
        Ok(self.job(scope, task)?.context.snapshot())
    }
    /// Acknowledges disposal and requests cancellation if needed, without joining workers.
    /// History and result ownership remain until every scheduled native reader exits.
    pub fn dispose_task(&self, scope: &str, task: &str) -> Result<()> {
        let job = self.job(scope, task)?;
        job.disposed.store(true, Ordering::Release);
        if !job.context.snapshot().state.is_terminal() {
            let _ = job.context.request_cancel();
        }
        self.finish_disposed_job(&job);
        Ok(())
    }
    /// Returns at most 512 outcomes from a native batch result, with lossless counters.
    pub fn task_results(&self, scope: &str, task: &str, offset: u64, limit: u32) -> Result<Value> {
        let job = self.job(scope, task)?;
        let store = lock(&job.results)
            .clone()
            .ok_or_else(|| Error::new("not_batch", "This task has no paged batch results"))?;
        let page = store.query(offset, limit)?;
        let items: Vec<_> = page
            .items
            .into_iter()
            .map(|item| {
                let mut value = serde_json::to_value(&item).unwrap();
                value["ordinal"] = Value::String(item.ordinal.to_string());
                value
            })
            .collect();
        Ok(
            json!({"items":items,"offset":page.offset,"total":page.total,"counts":{
                "completed":page.counts.total.to_string(),"succeeded":page.counts.succeeded.to_string(),"failed":page.counts.failed.to_string()
            }}),
        )
    }
}
