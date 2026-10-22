//! Task-owned admission of arbitrary typed native capabilities.
use super::*;
use crate::{NativeLease, NativeResourceLease, Resource};

impl TaskContext {
    /// Owns a native value in this task's scope and retains an execution lease.
    /// The scope owner remains after successful execution so a returned resource
    /// can be consumed by subsequent operations. An unsuccessful task without a
    /// returned result or successful item output rolls back its newly created
    /// owners. Already admitted external leases retain the value independently.
    /// Scope disposal releases successful owners.
    pub fn own<T: Send + Sync + 'static>(&self, value: T) -> Result<Resource<T>> {
        self.checkpoint()?;
        let resource = self.task.resources.register_native(self.scope(), value)?;
        let capture = (|| {
            let lease = self
                .task
                .resources
                .lease_native_handle(self.scope(), resource.handle())?;
            // Record capture and ownership together: finalization cannot drain
            // the task between admission and recording a new owner for rollback.
            let mut data = lock(&self.task.data);
            Self::mutable(&data)?;
            if data.snapshot.cancel_requested {
                return Err(Error::cancelled());
            }
            data.created_native
                .insert(resource.handle().id.clone(), resource.handle().clone());
            data.native_leases
                .insert(resource.handle().id.clone(), lease);
            Ok(())
        })();
        if let Err(error) = capture {
            let _ = self.task.resources.release(self.scope(), resource.handle());
            return Err(error);
        }
        Ok(resource)
    }

    pub(super) fn rollback_created(&self, handles: HashMap<String, Handle>) {
        for handle in handles.into_values() {
            // Scope disposal or an explicit owner release can win this race.
            // In either case the owner is already gone; never resurrect it.
            let _ = self.task.resources.release(self.scope(), &handle);
        }
    }
    /// Captures a file or arbitrary native handle before custom user code runs.
    /// Wire possession alone is insufficient: scope, identity and ownership are
    /// validated. A cached capability remains admitted after owner release.
    pub fn admit_handle(&self, handle: &Handle) -> Result<()> {
        if handle.kind == "file" {
            return self.lease(handle).map(|_| ());
        }
        self.checkpoint()?;
        if handle.scope != self.scope() {
            return Err(Error::new(
                "scope_mismatch",
                "Resource belongs to another scope",
            ));
        }
        {
            let data = lock(&self.task.data);
            Self::mutable(&data)?;
            if let Some(lease) = data.native_leases.get(&handle.id) {
                if lease.handle() != handle {
                    return Err(Error::new(
                        "stale_handle",
                        "Handle does not match captured native lease",
                    ));
                }
                return Ok(());
            }
        }
        let lease = self
            .task
            .resources
            .lease_native_handle(self.scope(), handle)?;
        self.capture_native(lease)
    }
    /// Captures an already admitted native lease from a trusted host, retaining
    /// it until task exit. This permits history-owned capabilities after original
    /// owner release, while preserving scope and complete identity validation.
    pub fn capture_native(&self, lease: NativeResourceLease) -> Result<()> {
        self.checkpoint()?;
        let handle = lease.handle();
        if handle.scope != self.scope() {
            return Err(Error::new(
                "scope_mismatch",
                "Resource belongs to another scope",
            ));
        }
        let mut data = lock(&self.task.data);
        Self::mutable(&data)?;
        if let Some(old) = data.native_leases.get(&handle.id) {
            if old.handle() != handle {
                return Err(Error::new(
                    "stale_handle",
                    "Handle conflicts with existing native lease",
                ));
            }
            return Ok(());
        }
        data.native_leases.insert(handle.id.clone(), lease);
        Ok(())
    }
    /// Returns the task's retained erased lease for native host output ownership.
    /// The handle must have been freshly admitted or captured from a trusted host.
    pub fn native_lease(&self, handle: &Handle) -> Result<NativeResourceLease> {
        self.admit_handle(handle)?;
        let data = lock(&self.task.data);
        Self::mutable(&data)?;
        data.native_leases
            .get(&handle.id)
            .cloned()
            .ok_or_else(|| Error::new("wrong_kind", "Expected a typed native resource"))
    }
    /// Returns a typed retained lease, checking the native Rust type in addition
    /// to wire admission. Cancellation/disposal checkpoints apply to every call.
    pub fn lease_resource<T: Send + Sync + 'static>(
        &self,
        resource: &Resource<T>,
    ) -> Result<NativeLease<T>> {
        self.admit_handle(resource.handle())?;
        let data = lock(&self.task.data);
        Self::mutable(&data)?;
        data.native_leases
            .get(&resource.handle().id)
            .ok_or_else(|| Error::new("wrong_kind", "Expected a typed native resource"))?
            .typed()
    }
}
