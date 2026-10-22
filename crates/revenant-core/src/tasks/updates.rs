//! Application updates are bounded replaceable data snapshots, never a retained event log.
use super::*;
use std::io::Write;

/// Maximum encoded JSON bytes retained by one application task update (64 KiB).
pub const MAX_TASK_UPDATE_BYTES: usize = 64 * 1024;

struct BoundedJson(Vec<u8>);
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_TASK_UPDATE_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other(
                "Task update exceeds its encoded byte limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TaskContext {
    /// Pins author-declared scheduling metadata at host admission. A user closure
    /// cannot change the admitted class to obtain interactive worker privileges.
    pub fn configure_work_class(&self, class: WorkClass) -> Result<()> {
        self.checkpoint()?;
        let mut data = lock(&self.task.data);
        Self::mutable(&data)?;
        match data.work_class {
            Some(previous) if previous != class => Err(Error::new(
                "task_work_class_conflict",
                "Task work class is already pinned",
            )),
            _ => {
                data.work_class = Some(class);
                Ok(())
            }
        }
    }
    /// Declared scheduling class; tasks without host metadata default to CPU.
    pub fn work_class(&self) -> WorkClass {
        lock(&self.task.data).work_class.unwrap_or_default()
    }
    /// Installs an operation's compiled update schema before user work executes.
    /// Repeated configuration with the same schema is harmless; a different
    /// schema returns `task_update_contract_conflict`. Terminal tasks reject it.
    pub fn configure_updates(&self, schema: Value) -> Result<()> {
        self.checkpoint()?;
        {
            let data = lock(&self.task.data);
            Self::mutable(&data)?;
            if let Some(previous) = &data.update_schema {
                return Self::same_update_schema(previous.schema(), &schema);
            }
        }
        self.configure_prepared_updates(Arc::new(crate::PreparedValidator::new(schema)?))
    }

    /// Pins shared preparation from the admitted immutable operation generation.
    /// Batches reuse this validator across contexts without compiling per item.
    pub fn configure_prepared_updates(
        &self,
        prepared: Arc<crate::PreparedValidator>,
    ) -> Result<()> {
        self.checkpoint()?;
        let mut data = lock(&self.task.data);
        Self::mutable(&data)?;
        if let Some(previous) = &data.update_schema {
            Self::same_update_schema(previous.schema(), prepared.schema())?;
        } else {
            data.update_schema = Some(prepared);
        }
        Ok(())
    }
    fn same_update_schema(previous: &Value, schema: &Value) -> Result<()> {
        if previous == schema {
            Ok(())
        } else {
            Err(Error::new(
                "task_update_contract_conflict",
                "Task update contract is already pinned",
            ))
        }
    }

    /// Replaces the latest application update using the declared operation schema.
    /// Requires a running executor and active scope. Serialization stops at 64 KiB;
    /// invalid values and oversized payloads leave the previous update unchanged.
    /// Subscribers receive the new task revision; late subscribers receive only
    /// the latest update. Cancellation is checked before publication.
    pub fn publish<T: Serialize>(&self, update: T) -> Result<()> {
        self.checkpoint()?;
        {
            let data = lock(&self.task.data);
            if !data.started {
                return Err(Error::new(
                    "task_not_running",
                    "Updates require an active executor",
                ));
            }
            if data.update_schema.is_none() {
                return Err(Error::new(
                    "task_updates_undeclared",
                    "Operation has no declared update contract",
                ));
            }
        }
        let mut bytes = BoundedJson(Vec::new());
        serde_json::to_writer(&mut bytes, &update).map_err(|error| {
            Error::new(
                if error.is_io() {
                    "task_update_limit"
                } else {
                    "invalid_task_update"
                },
                error.to_string(),
            )
        })?;
        let value = serde_json::from_slice(&bytes.0)?;
        self.change_unit(|data| {
            Self::mutable(data)?;
            if data.snapshot.cancel_requested {
                return Err(Error::cancelled());
            }
            if !self.task.resources.is_scope_active(self.scope()) {
                return Err(Error::cancelled());
            }
            let schema = data.update_schema.as_ref().ok_or_else(|| {
                Error::new(
                    "task_updates_undeclared",
                    "Operation has no declared update contract",
                )
            })?;
            schema
                .validate(&value)
                .map_err(|error| Error::new("invalid_task_update", error.message))?;
            data.snapshot.update = Some(TaskUpdate {
                revision: (data.snapshot.revision.get() + 1).into(),
                value,
            });
            Ok(())
        })
    }
}
