//! An admitted Arc is the read capability; reads never look up registry entries again. Chunk bounds and host responses remain checked.
use super::*;
pub(super) struct Resource {
    pub(super) handle: Handle,
    pub(super) metadata: FileMetadata,
    pub(super) port: Arc<dyn ResourcePort>,
    pub(super) cleanup: Option<Arc<dyn ResourceCleanup>>,
    pub(super) provider_count: Option<Arc<AtomicUsize>>,
}
impl Drop for Resource {
    fn drop(&mut self) {
        if let Some(count) = &self.provider_count {
            count.fetch_sub(1, Ordering::AcqRel);
        }
        if let Some(cleanup) = &self.cleanup {
            cleanup.cleanup(&self.handle);
        }
    }
}

/// An admitted capability retaining its resource independently of registry ownership.
/// Reads remain authorized after owner release or scope disposal; task checkpoints
/// enforce task cancellation separately. Cleanup runs after the final reference drops.
#[derive(Clone)]
pub struct ResourceLease {
    pub(super) resource: Arc<Resource>,
}
impl ResourceLease {
    /// Checks native availability without reading content or reacquiring ownership.
    /// A retained lease may remain valid after its former filename is deleted.
    pub async fn validate(&self) -> Result<()> {
        self.resource.port.validate(&self.resource.handle).await
    }
    /// Borrows the original admitted capability identity for this lease borrow.
    pub fn handle(&self) -> &Handle {
        &self.resource.handle
    }
    /// Borrows file metadata captured at registration; does not query the host again.
    pub fn metadata(&self) -> &FileMetadata {
        &self.resource.metadata
    }
    /// Reads from a byte offset, requesting 1 byte through 4 MiB per call.
    /// Clamps length to the captured size and returns an empty buffer at EOF.
    /// Returns `invalid_chunk` for invalid length, `invalid_offset` beyond size,
    /// and `invalid_resource_read` for an empty or oversized host response before EOF.
    /// Host I/O errors propagate. Owner release and scope disposal do not revoke
    /// this admitted capability; task-level cancellation is checked by TaskContext.
    pub async fn read(&self, offset: u64, length: u32) -> Result<Vec<u8>> {
        let size = self.resource.metadata.size.get();
        if length == 0 || length > 4 * 1024 * 1024 {
            return Err(Error::new(
                "invalid_chunk",
                "Chunk length must be between 1 byte and 4 MiB",
            ));
        }
        if offset > size {
            return Err(Error::new(
                "invalid_offset",
                "Read offset exceeds resource size",
            ));
        }
        let length = u32::try_from((size - offset).min(u64::from(length))).unwrap();
        if length == 0 {
            return Ok(Vec::new());
        }
        let bytes = self
            .resource
            .port
            .read(&self.resource.handle, offset, length)
            .await?;
        if bytes.is_empty() || bytes.len() > length as usize {
            return Err(Error::new(
                "invalid_resource_read",
                "Host returned an empty or oversized chunk before EOF",
            ));
        }
        Ok(bytes)
    }
}
