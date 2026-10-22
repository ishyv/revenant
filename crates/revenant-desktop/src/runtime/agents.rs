//! Native agent principals never borrow webview scope authority.
use super::{DesktopRuntime, Error, Result, id, lock};
use super::{inspect::TraceContext, scopes::ScopeOwner};
use revenant_core::TaskSnapshot;
use revenant_sdk::{ContractManifest, OperationDescriptor};
use serde_json::{Value, json};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

/// One native agent connection owning an isolated scope, tasks, and result handles.
/// Dropping requests cancellation and releases ownership; [`Self::close`] additionally
/// waits for in-flight executors up to the configured shutdown grace period.
pub struct AgentSession {
    runtime: DesktopRuntime,
    scope: String,
    id: String,
    closed: AtomicBool,
    admission: Mutex<()>,
}
impl DesktopRuntime {
    /// Opens an isolated local agent principal when application agent access is enabled.
    pub fn open_agent_session(&self) -> Result<AgentSession> {
        self.accepting()?;
        if !self.inner.options.agents_enabled {
            return Err(Error::new(
                "agents_disabled",
                "Application agent access is disabled",
            ));
        }
        let session = id("agent");
        let scope = self.create_scope_inner(None, ScopeOwner::Agent(session.clone()))?;
        Ok(AgentSession {
            runtime: self.clone(),
            scope,
            id: session,
            closed: AtomicBool::new(false),
            admission: Mutex::new(()),
        })
    }
}
impl AgentSession {
    fn active(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::new(
                "session_closed",
                "Agent session has disconnected",
            ));
        }
        if self.runtime.scope_owner(&self.scope)? != ScopeOwner::Agent(self.id.clone()) {
            return Err(Error::new(
                "scope_mismatch",
                "Agent scope belongs to another principal",
            ));
        }
        Ok(())
    }
    /// Borrows this connection's native ownership scope; it cannot be used as a window scope.
    pub fn scope(&self) -> &str {
        &self.scope
    }
    /// Returns authoritative application, instance, contract, and session identity.
    pub fn identity(&self) -> Value {
        json!({"appId":self.runtime.inner.options.app_id,"instanceId":self.runtime.inner.instance_id,"contractDigest":self.runtime.inner.contract_digest,"sessionId":self.id})
    }
    /// Returns the authoritative compiled app manifest; tool exposure is separately enforced.
    pub fn manifest(&self) -> Result<ContractManifest> {
        self.active()?;
        self.runtime.inner.registry.manifest()
    }
    /// Lists only operations explicitly marked for app agents by their Rust authors.
    pub fn list_tools(&self) -> Result<Vec<OperationDescriptor>> {
        Ok(self
            .manifest()?
            .operations
            .into_iter()
            .filter(|operation| operation.agent)
            .collect())
    }
    /// Admits an author-exposed operation and returns its native task receipt.
    pub fn run(&self, operation: &str, input: Value) -> Result<TaskSnapshot> {
        self.run_with_request_id(operation, input, None)
    }
    /// Admits an exposed operation with a request identity for cross-boundary tracing.
    pub fn run_with_request_id(
        &self,
        operation: &str,
        input: Value,
        request_id: Option<&str>,
    ) -> Result<TaskSnapshot> {
        // An admitted job must enter the session's ownership set before close
        // captures that set. Native execution never acquires this admission lock.
        let _admission = lock(&self.admission);
        self.active()?;
        let prepared = self.runtime.inner.registry.prepare_operation(operation)?;
        if !prepared.descriptor().agent {
            return Err(Error::new(
                "operation_not_exposed",
                "Operation is not exposed to application agents",
            ));
        }
        // Keep this policy guard pinned while the executor captures the same generation.
        self.runtime.run_task_traced(
            &self.scope,
            operation,
            input,
            None,
            TraceContext::agent(&self.id, request_id),
        )
    }
    /// Observes one task owned by this connection.
    pub fn snapshot(&self, task: &str) -> Result<TaskSnapshot> {
        self.active()?;
        self.runtime.task_snapshot(&self.scope, task)
    }
    /// Reads one bounded page of this connection's native batch outcomes.
    pub fn results(&self, task: &str, offset: u64, limit: u32) -> Result<Value> {
        self.active()?;
        self.runtime.task_results(&self.scope, task, offset, limit)
    }
    /// Requests cancellation of this connection's task.
    pub fn cancel(&self, task: &str) -> Result<TaskSnapshot> {
        self.active()?;
        self.runtime.cancel_task(&self.scope, task)
    }
    /// Disposes this connection's task history, cancelling execution when necessary.
    pub fn dispose(&self, task: &str) -> Result<()> {
        self.active()?;
        self.runtime.dispose_task(&self.scope, task)
    }
    /// Cancels and releases this session and waits for actual native executor exit.
    /// A timeout leaves executing workers and their captured leases intact.
    pub async fn close(&self) -> Result<()> {
        let jobs: Vec<_> = {
            let _admission = lock(&self.admission);
            let jobs = lock(&self.runtime.inner.jobs)
                .values()
                .filter(|job| job.context.scope() == self.scope)
                .cloned()
                .collect();
            if !self.closed.swap(true, Ordering::AcqRel)
                && let Err(error) = self.runtime.dispose_scope(&self.scope)
            {
                self.runtime.inner.record_cleanup_failure(error.clone());
                return Err(error);
            }
            jobs
        };
        tokio::time::timeout(self.runtime.inner.options.shutdown_grace, async {
            while jobs.iter().any(|job| !job.finished.load(Ordering::Acquire)) {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .map_err(|_| {
            let error = Error::new(
                "session_cleanup_timeout",
                "Agent session cancellation exceeded the grace period",
            );
            self.runtime.inner.record_cleanup_failure(error.clone());
            error
        })?;
        Ok(())
    }
}
impl Drop for AgentSession {
    fn drop(&mut self) {
        if !self.closed.swap(true, Ordering::AcqRel)
            && let Err(error) = self.runtime.dispose_scope(&self.scope)
        {
            self.runtime.inner.record_cleanup_failure(error);
        }
    }
}
