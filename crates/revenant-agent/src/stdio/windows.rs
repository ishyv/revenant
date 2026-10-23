//! Synchronous anonymous pipes need cancellation on their actual owning thread.
use super::cancelled;
use std::{
    fs::File,
    io,
    os::windows::io::{AsHandle, AsRawHandle},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};
use windows_sys::Win32::{Foundation::ERROR_NOT_FOUND, System::IO::CancelSynchronousIo};

#[derive(Clone)]
pub(super) struct Handle(Arc<File>);
impl Handle {
    pub(super) fn stdin() -> io::Result<Self> {
        Ok(Self(Arc::new(File::from(
            std::io::stdin().as_handle().try_clone_to_owned()?,
        ))))
    }
    pub(super) fn stdout() -> io::Result<Self> {
        Ok(Self(Arc::new(File::from(
            std::io::stdout().as_handle().try_clone_to_owned()?,
        ))))
    }
    pub(super) fn read(&self, bytes: &mut [u8], stopped: &AtomicBool) -> io::Result<usize> {
        use std::io::Read;
        if stopped.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        (&*self.0).read(bytes)
    }
    pub(super) fn write(&self, mut bytes: &[u8], stopped: &AtomicBool) -> io::Result<()> {
        use std::io::Write;
        while !bytes.is_empty() {
            if stopped.load(Ordering::Acquire) {
                return Err(cancelled());
            }
            match (&*self.0).write(bytes) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(length) => bytes = &bytes[length..],
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}
pub(super) fn cancel(thread: &JoinHandle<io::Result<()>>) -> io::Result<()> {
    if thread.is_finished() {
        return Ok(());
    }
    // SAFETY: the retained JoinHandle owns this thread handle until join. Only
    // these dedicated workers issue synchronous I/O on it, never application jobs.
    if unsafe { CancelSynchronousIo(thread.as_raw_handle()) } != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
        // It may be between checking its stop flag and entering Read/WriteFile.
        // The owner retries until the worker acknowledges exit; never detach it.
        Ok(())
    } else {
        Err(error)
    }
}
