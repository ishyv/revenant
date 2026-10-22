//! Payload-free execution evidence and bounded development reports.
use super::{DesktopRuntime, Error, Result, RuntimeInner, id, lock};
use revenant_core::TaskState;
use serde_json::{Value, json};
use std::collections::VecDeque;

#[derive(Clone)]
pub(crate) struct TraceContext {
    pub request_id: String,
    pub origin: &'static str,
    pub principal: Option<String>,
}
impl TraceContext {
    pub fn native() -> Self {
        Self {
            request_id: id("request"),
            origin: "native",
            principal: None,
        }
    }
    pub fn frontend(request: &Value, window: &str) -> Self {
        Self {
            request_id: request
                .get("requestId")
                .and_then(Value::as_str)
                .filter(|id| id.len() <= 256)
                .map(str::to_owned)
                .unwrap_or_else(|| id("request")),
            origin: "frontend",
            principal: Some(window.into()),
        }
    }
    pub fn agent(session: &str, request_id: Option<&str>) -> Self {
        Self {
            request_id: request_id
                .filter(|id| id.len() <= 256)
                .map(str::to_owned)
                .unwrap_or_else(|| id("request")),
            origin: "agent",
            principal: Some(session.into()),
        }
    }
}

pub(crate) struct TraceRing {
    capacity: usize,
    sequence: u64,
    dropped: u64,
    events: VecDeque<Value>,
}
impl TraceRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            sequence: 0,
            dropped: 0,
            events: VecDeque::new(),
        }
    }
    pub fn push(
        &mut self,
        context: &TraceContext,
        action: &str,
        operation: Option<&str>,
        task: Option<&str>,
        state: Option<TaskState>,
        error: Option<&str>,
    ) {
        self.sequence = self.sequence.saturating_add(1);
        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_string();
        let bounded = |text: &str| text.chars().take(256).collect::<String>();
        self.events.push_back(json!({"sequence":self.sequence,"timeUnixMs":time,"requestId":bounded(&context.request_id),"origin":context.origin,"principal":context.principal.as_deref().map(bounded),"action":bounded(action),"operationId":operation.map(bounded),"taskId":task.map(bounded),"state":state,"errorCode":error.map(bounded)}));
    }

    pub fn batch_item_failed(
        &mut self,
        context: &TraceContext,
        operation: &str,
        task: &str,
        ordinal: u64,
        error_code: &str,
    ) {
        self.push(
            context,
            "task.item_failed",
            Some(operation),
            Some(task),
            None,
            Some(error_code),
        );
        if let Some(event) = self.events.back_mut() {
            event["ordinal"] = json!(ordinal.to_string());
        }
    }
}

impl RuntimeInner {
    pub(crate) fn record_cleanup_failure(&self, error: Error) {
        let mut failures = lock(&self.cleanup_failures);
        if failures.len() == 32 {
            failures.pop_front();
        }
        failures.push_back(error);
    }
}

impl DesktopRuntime {
    /// Returns stable identity for this app process and its compiled operation contract.
    pub fn identity(&self) -> Value {
        json!({"appId":self.inner.options.app_id,"instanceId":self.inner.instance_id,"contractDigest":self.inner.contract_digest})
    }

    pub(crate) fn trace_task_admission(&self, job: &super::jobs::JobRecord) {
        lock(&self.inner.trace).push(
            &job.trace,
            "task.admitted",
            Some(&job.operation),
            Some(job.context.id()),
            Some(job.context.snapshot().state),
            None,
        );
    }

    /// Returns native identity, bounded execution state, ownership counts, and cleanup diagnostics.
    /// Task arguments/results and provider configuration are deliberately excluded.
    pub fn runtime_inspect(&self) -> Result<Value> {
        let jobs: Vec<_> = lock(&self.inner.jobs).values().cloned().collect();
        let mut tasks: Vec<_> = jobs.iter().map(|job| {
            let snapshot = job.context.snapshot();
            json!({"id":snapshot.id,"scope":snapshot.scope,"operationId":job.operation,"requestId":job.trace.request_id,"origin":job.trace.origin,"state":snapshot.state,"cancelRequested":snapshot.cancel_requested,"progress":{"completed":snapshot.progress.completed,"total":snapshot.progress.total},"summary":snapshot.summary,"errorCode":snapshot.error.as_ref().map(|error| &error.code),"executionFinished":job.finished.load(std::sync::atomic::Ordering::Acquire)})
        }).collect();
        tasks.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        let queued = jobs
            .iter()
            .filter(|job| job.context.snapshot().state == TaskState::Queued)
            .count();
        let providers = self.inner.registry.inspect();
        let resources: u64 = providers
            .iter()
            .map(|provider| provider.resources.get())
            .sum();
        let providers: Vec<_> = providers.into_iter().map(|provider| json!({"id":provider.id,"generation":provider.generation,"activeTasks":provider.active_tasks,"resources":provider.resources,"dependencies":provider.dependencies})).collect();
        let (native_owners, native_live) = self.inner.resources.native_resource_counts();
        let retained_result_bytes: usize = jobs
            .iter()
            .filter_map(|job| job.context.snapshot().result)
            .map(|result| serde_json::to_vec(&result).map_or(0, |bytes| bytes.len()))
            .sum();
        Ok(json!({
            "appId":self.inner.options.app_id,"instanceId":self.inner.instance_id,"contractDigest":self.inner.contract_digest,"sdkRevision":revenant_core::SDK_COMPATIBILITY_REVISION,
            "projectRoot":self.inner.options.project_root,"sourceFingerprint":self.inner.options.source_fingerprint,"buildAttempt":self.inner.options.build_attempt,
            "closing":self.inner.closing.load(std::sync::atomic::Ordering::Acquire),
            "execution":{"activeWorkers":self.inner.options.concurrency.saturating_sub(self.inner.workers.available_permits()),"workerLimit":self.inner.options.concurrency,"admittedTasks":(self.inner.options.concurrency+self.inner.options.queue_capacity).saturating_sub(self.inner.admission.available_permits()),"queuedTasks":queued,"queueCapacity":self.inner.options.queue_capacity},
            "ownership":{"scopes":lock(&self.inner.scopes).len(),"views":lock(&self.inner.views).len(),"folders":lock(&self.inner.folders).len(),"selections":lock(&self.inner.selections).len(),"previews":self.inner.previews.owner_record_count()?,"tasks":jobs.len(),"resources":resources.to_string(),"nativeOwners":native_owners.to_string(),"nativeLive":native_live.to_string()},
            "work":revenant_core::WorkService::stats(self.inner.work.as_ref()),
            "workLimits":{"cpu":self.inner.options.concurrency,"blockingIo":self.inner.options.blocking_io_concurrency,"queuedChunks":self.inner.options.work_queue_capacity,"knownScratchBytes":8388608,"orderedResultBytes":67108864,"streamWindowItems":32,"unobservedApplicationAllocations":null},
            "retainedResults":{"serializedJsonBytes":retained_result_bytes,"maxTasks":self.inner.options.max_tasks,"maxBytesPerResult":self.inner.options.max_contract_bytes,"nativeResourceAllocationBytes":null},
            "workflowModules":{"issuedScopes":true,"applicationPaths":true,"preparedContracts":true,"wireProjection":true,"boundedWork":true,"latestJobs":true,"queryGroups":true,"overlayCovers":true,"nativeGraphics":cfg!(all(feature="native-surfaces", windows)),"computePromotion":"experimental-no-go"},
            "tasks":tasks,"providers":providers,"storeRecovery":self.inner.store_recovery,"cleanupFailures":*lock(&self.inner.cleanup_failures),
            "frontendReports":*lock(&self.inner.frontend_reports)
        }))
    }

    /// Queries at most 512 payload-free trace events after a sequence cursor.
    /// `dropped` reports eviction, and `oldestSequence` allows a caller to detect a gap.
    pub fn trace_query(&self, after_sequence: u64, limit: u32) -> Result<Value> {
        if !(1..=512).contains(&limit) {
            return Err(Error::new(
                "invalid_window",
                "Trace page size must be between 1 and 512",
            ));
        }
        let trace = lock(&self.inner.trace);
        let events: Vec<_> = trace
            .events
            .iter()
            .filter(|event| event["sequence"].as_u64().unwrap_or(0) > after_sequence)
            .take(limit as usize)
            .cloned()
            .collect();
        let next = events
            .last()
            .and_then(|event| event["sequence"].as_u64())
            .unwrap_or(after_sequence);
        Ok(
            json!({"events":events,"nextSequence":next,"latestSequence":trace.sequence,"oldestSequence":trace.events.front().and_then(|event| event["sequence"].as_u64()),"dropped":trace.dropped}),
        )
    }

    /// Retains at most 32 development frontend reports, each bounded to 16 KiB.
    /// Reports are untrusted observations supplied by the frontend, not native authority.
    pub fn record_frontend_snapshot(&self, snapshot: Value) -> Result<()> {
        if !self.inner.options.development {
            return Err(Error::new(
                "development_only",
                "Frontend diagnostic reports require development mode",
            ));
        }
        if serde_json::to_vec(&snapshot)?.len() > 16 * 1024 {
            return Err(Error::new(
                "frontend_report_limit",
                "Frontend diagnostic reports allow at most 16 KiB",
            ));
        }
        let mut reports = lock(&self.inner.frontend_reports);
        if reports.len() == 32 {
            reports.pop_front();
        }
        reports.push_back(snapshot);
        Ok(())
    }
}
