//! Nonblocking descriptors keep cancellation independent of a pipe's reader.
use super::{POLL_INTERVAL, cancelled};
use std::{
    fs::File,
    io,
    os::fd::{AsFd, AsRawFd},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

#[derive(Clone)]
pub(super) struct Handle(Arc<Descriptor>);
struct Descriptor {
    file: File,
    flags: libc::c_int,
}
impl Handle {
    pub(super) fn stdin() -> io::Result<Self> {
        Self::new(File::from(std::io::stdin().as_fd().try_clone_to_owned()?))
    }
    pub(super) fn stdout() -> io::Result<Self> {
        Self::new(File::from(std::io::stdout().as_fd().try_clone_to_owned()?))
    }
    pub(super) fn new(file: File) -> io::Result<Self> {
        // SAFETY: the owned file keeps its descriptor valid through these calls.
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        if flags == -1 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(Arc::new(Descriptor { file, flags })))
    }
    fn ready(&self, events: libc::c_short, stopped: &AtomicBool) -> io::Result<()> {
        loop {
            if stopped.load(Ordering::Acquire) {
                return Err(cancelled());
            }
            let mut pollfd = libc::pollfd {
                fd: self.0.file.as_raw_fd(),
                events,
                revents: 0,
            };
            // SAFETY: pollfd is valid for exactly one element; timeout is finite.
            let result =
                unsafe { libc::poll(&mut pollfd, 1, POLL_INTERVAL.as_millis() as libc::c_int) };
            if result > 0 {
                return Ok(());
            }
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
    pub(super) fn read(&self, bytes: &mut [u8], stopped: &AtomicBool) -> io::Result<usize> {
        use std::io::Read;
        loop {
            self.ready(libc::POLLIN, stopped)?;
            match (&self.0.file).read(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) => {}
                result => return result,
            }
        }
    }
    pub(super) fn write(&self, mut bytes: &[u8], stopped: &AtomicBool) -> io::Result<()> {
        use std::io::Write;
        while !bytes.is_empty() {
            self.ready(libc::POLLOUT, stopped)?;
            match (&self.0.file).write(bytes) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(length) => bytes = &bytes[length..],
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        // dup shares status flags with the original. MCP owns stdio exclusively
        // while serving; restore the original state before releasing that owner.
        unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_SETFL, self.flags) };
    }
}
pub(super) fn cancel(_: &JoinHandle<io::Result<()>>) -> io::Result<()> {
    // Nonblocking syscalls and finite poll waits observe the shared stop flag.
    Ok(())
}
