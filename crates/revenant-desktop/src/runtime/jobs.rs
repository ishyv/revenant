//! Bounded native admission, captured inputs, and disk-backed batch outcomes.
//!
//! Queued jobs pin provider generations and inputs before returning to Svelte.
//! A batch pins one native selector, then owns one item's file at a time. Completed
//! outcomes are committed in chunks; task snapshots retain counters, not inventory.

use super::views::ViewRecord;
use super::{DesktopRuntime, Error, Result, UpdateSink, id, lock};
use crate::{
    file_port::FilePort,
    files::IndexState,
    results::{BatchOutcome, ResultStore},
};
use revenant_core::{Handle, Progress, ResourceLease, Subscription, TaskContext, TaskSnapshot};
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
}
/// Result leases are independent of executor caches and live with native history.
pub(crate) struct ResultResources {
    pub leases: Mutex<std::collections::HashMap<Handle, ResourceLease>>,
}
impl ResultResources {
    fn retain(
        &self,
        prepared: &PreparedOperation,
        output: &Value,
        context: &TaskContext,
    ) -> Result<()> {
        let handles = prepared.output_handles(output)?;
        let mut retained = lock(&self.leases);
        let mut additions = Vec::new();
        for handle in handles {
            if !retained.contains_key(&handle)
                && !additions.iter().any(|(known, _)| known == &handle)
            {
                additions.push((handle.clone(), context.lease(&handle)?));
            }
        }
        if retained.len() + additions.len() > 512 {
            return Err(Error::new(
                "result_resource_limit",
                "A result may retain at most 512 distinct resources; return metadata or dispose earlier results",
            ));
        }
        retained.extend(additions);
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
    },
    Captured(Arc<SelectionRecord>),
    Inputs(Vec<(Value, TaskContext)>),
}

impl DesktopRuntime {
    fn make_job(&self, scope: &str, sink: Option<UpdateSink>) -> Result<Arc<JobRecord>> {
        self.accepting()?;
        self.scope_window(scope)?;
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
        let subscription = context.subscribe(Arc::new(move |snapshot: TaskSnapshot| {
            // Failed admission reports the invoke error, never a misleading
            // cancellation snapshot from its private cleanup task.
            if !publish_admitted.load(Ordering::Acquire) {
                return;
            }
            if let Some(sink) = &sink {
                let mut last = lock(&last);
                if snapshot.state.is_terminal() || last.elapsed() >= Duration::from_millis(50) {
                    *last = Instant::now();
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
            }),
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
        let job = self.make_job(scope, sink)?;
        if let Err(error) = self.admit_input(&prepared, &input, &job.context) {
            let _ = job.context.request_cancel();
            lock(&self.inner.jobs).remove(job.context.id());
            let _ = self.inner.tasks.remove(scope, job.context.id());
            return Err(error);
        }
        job.admitted.store(true, Ordering::Release);
        let initial = job.context.snapshot();
        let runtime = self.clone();
        tokio::spawn(async move {
            let worker = loop {
                if job.context.snapshot().state.is_terminal() {
                    runtime.finish_disposed_job(&job);
                    return;
                }
                tokio::select! {
                    permit = runtime.inner.workers.clone().acquire_owned() => { let Ok(permit) = permit else { return }; break permit; }
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            };
            let rt = runtime.clone();
            let native = job.clone();
            let executor = tokio::runtime::Handle::current();
            let result = tokio::task::spawn_blocking(move || {
                let _admission = admission;
                let _worker = worker;
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
                if let Ok(output) = &result {
                    if let Err(error) = native.resources.retain(&prepared, output, &native.context)
                    {
                        result = Err(error);
                    }
                }
                execution.finish(result)?;
                Ok::<(), Error>(())
            })
            .await;
            if let Err(error) = result {
                let _ = job.context.request_cancel();
                let _ = error;
            }
            runtime.finish_disposed_job(&job);
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
        self.scope_window(scope)?;
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
        if let Some(old) = selections.get(&selection) {
            if old.scope != scope {
                return Err(Error::new(
                    "scope_mismatch",
                    "Selection belongs to another scope",
                ));
            }
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
        let admission = self
            .inner
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::new("queue_full", "Native work queue is full").retryable(true))?;
        let prepared = self.inner.registry.prepare_operation(operation)?;
        let job = self.make_job(scope, sink)?;
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
                    let child = job.context.batch_child(index as u32, total)?;
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
                    BatchSource::Matching { view, query }
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
        let runtime = self.clone();
        tokio::spawn(async move {
            if job.context.snapshot().state.is_terminal() {
                runtime.finish_disposed_job(&job);
                return;
            }
            let Ok(execution) = job.context.begin() else {
                runtime.finish_disposed_job(&job);
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
                        runtime.finish_disposed_job(&job);
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
                            runtime.finish_disposed_job(&job);
                            return;
                        }
                        Err(error) => {
                            let _ = execution.finish(Err(error));
                            runtime.finish_disposed_job(&job);
                            return;
                        }
                        _ => tokio::time::sleep(Duration::from_millis(20)).await,
                    }
                }
            }
            let worker = loop {
                if job.context.checkpoint().is_err() {
                    let _ = execution.finish_batch(reference, true);
                    runtime.finish_disposed_job(&job);
                    return;
                }
                tokio::select! {
                    permit = runtime.inner.workers.clone().acquire_owned() => { let Ok(permit) = permit else { return }; break permit; }
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            };
            let native = job.clone();
            let rt = runtime.clone();
            let executor = tokio::runtime::Handle::current();
            let _ = tokio::task::spawn_blocking(move || {
                let _worker = worker;
                let _admission = admission;
                let result = rt.execute_batch(&native, &prepared, source, &store, &executor);
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
            runtime.finish_disposed_job(&job);
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
        enum Items {
            Matching(crate::files::FileSelection),
            Captured(std::vec::IntoIter<ResourceLease>),
            Inputs(std::vec::IntoIter<(Value, TaskContext)>),
        }
        let (mut items, total) = match source {
            BatchSource::Matching { view, query } => {
                let selection = view.index.index.selection(&query)?;
                let total = selection.total();
                (Items::Matching(selection), total)
            }
            BatchSource::Captured(selection) => (
                Items::Captured(selection.leases.clone().into_iter()),
                selection.leases.len() as u64,
            ),
            BatchSource::Inputs(inputs) => {
                let total = inputs.len() as u64;
                (Items::Inputs(inputs.into_iter()), total)
            }
        };
        let total32 = u32::try_from(total).map_err(|_| {
            Error::new(
                "batch_too_large",
                "Batch exceeds supported item-index space",
            )
        })?;
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
                let (input, retained, _owner, admitted_child) = match &mut items {
                    Items::Matching(selection) => {
                        let Some(item) = selection.next() else { break };
                        let item = item?;
                        let port = FilePort::new(item.path, item.metadata.clone());
                        let entry = self.inner.resources.register_file_owned(
                            context.scope(),
                            item.metadata,
                            port,
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
                    Items::Captured(leases) => {
                        let Some(lease) = leases.next() else { break };
                        (
                            serde_json::to_value(
                                lease.metadata().clone().entry(lease.handle().clone())?,
                            )?,
                            Some(lease),
                            None,
                            None,
                        )
                    }
                    Items::Inputs(inputs) => {
                        let Some((input, child)) = inputs.next() else {
                            break;
                        };
                        (input, None, None, Some(child))
                    }
                };
                let child = match admitted_child {
                    Some(child) => child,
                    None => context.batch_child(ordinal as u32, total32)?,
                };
                if let Some(lease) = retained {
                    child.capture(lease)?;
                }
                let child_execution = child.begin()?;
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
                child_execution.finish(output.clone())?;
                let outcome = match output {
                    Ok(output) => {
                        if serde_json::to_vec(&output)?.len()
                            > self.inner.options.max_contract_bytes
                        {
                            BatchOutcome {
                                ordinal,
                                input,
                                output: None,
                                error: Some(serde_json::to_value(Error::new(
                                    "output_too_large",
                                    "Return a native resource for large results",
                                ))?),
                            }
                        } else {
                            BatchOutcome {
                                ordinal,
                                input,
                                output: Some(output),
                                error: None,
                            }
                        }
                    }
                    Err(error) if error.is_cancelled() => return Err(error),
                    Err(error) => BatchOutcome {
                        ordinal,
                        input,
                        output: None,
                        error: Some(serde_json::to_value(error)?),
                    },
                };
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
            context.capture(self.lease_resource(context.scope(), &handle)?)?;
        }
        prepared.prepare_input(input, context)
    }
    pub(crate) fn lease_resource(&self, scope: &str, handle: &Handle) -> Result<ResourceLease> {
        self.scope_window(scope)?;
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
        for job in jobs {
            job.disposed.store(true, Ordering::Release);
            let _ = job.context.request_cancel();
            self.finish_disposed_job(&job);
        }
    }
    fn finish_disposed_job(&self, job: &Arc<JobRecord>) {
        if job.disposed.load(Ordering::Acquire) && job.context.snapshot().state.is_terminal() {
            lock(&self.inner.jobs).remove(job.context.id());
            let _ = self
                .inner
                .tasks
                .remove(job.context.scope(), job.context.id());
        }
    }
    /// Requests cancellation; terminal task history remains available until disposal.
    pub fn cancel_task(&self, scope: &str, task: &str) -> Result<TaskSnapshot> {
        self.job(scope, task)?.context.request_cancel()
    }
    /// Returns the current bounded native task snapshot.
    pub fn task_snapshot(&self, scope: &str, task: &str) -> Result<TaskSnapshot> {
        Ok(self.job(scope, task)?.context.snapshot())
    }
    /// Releases task history and result ownership, requesting cancellation if needed.
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
