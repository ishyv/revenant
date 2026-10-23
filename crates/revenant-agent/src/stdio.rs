//! Owned native stdio. No OS pipe operation runs on Tokio's blocking pool.
use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    task::{Context, Poll},
    thread::JoinHandle,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{mpsc as async_mpsc, oneshot},
};

#[cfg(unix)]
#[path = "stdio/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "stdio/windows.rs"]
mod platform;

pub(super) const CHUNK: usize = 8192;
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(10);

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "MCP stdio stopped")
}
fn copy_error(error: &io::Error) -> io::Error {
    io::Error::new(error.kind(), error.to_string())
}

/// Bounded native input chunks, consumed by the independently polled input pump.
pub(super) struct Input {
    receiver: async_mpsc::Receiver<io::Result<Vec<u8>>>,
    bytes: Vec<u8>,
    position: usize,
}
impl Input {
    pub(super) fn new(receiver: async_mpsc::Receiver<io::Result<Vec<u8>>>) -> Self {
        Self {
            receiver,
            bytes: Vec::new(),
            position: 0,
        }
    }
}
impl AsyncRead for Input {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if this.position == this.bytes.len() {
            match this.receiver.poll_recv(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    this.bytes = bytes;
                    this.position = 0;
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
        let length = output.remaining().min(this.bytes.len() - this.position);
        output.put_slice(&this.bytes[this.position..this.position + length]);
        this.position += length;
        Poll::Ready(Ok(()))
    }
}

struct Write {
    bytes: Vec<u8>,
    completed: oneshot::Sender<io::Result<()>>,
}
pub(super) struct Output {
    sender: mpsc::Sender<Write>,
    pending: Option<oneshot::Receiver<io::Result<()>>>,
    failure: Option<io::Error>,
    stopped: Arc<AtomicBool>,
}
impl AsyncWrite for Output {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Acknowledgement precedes admission of the next chunk: at most one
        // 8 KiB native write is retained, even if the peer never reads stdout.
        std::task::ready!(self.as_mut().poll_flush(cx))?;
        if self.stopped.load(Ordering::Acquire) {
            return Poll::Ready(Err(cancelled()));
        }
        let length = bytes.len().min(CHUNK);
        if length != 0 {
            let (completed, pending) = oneshot::channel();
            self.sender
                .send(Write {
                    bytes: bytes[..length].to_vec(),
                    completed,
                })
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "MCP writer exited"))?;
            self.pending = Some(pending);
        }
        Poll::Ready(Ok(length))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(failure) = &self.failure {
            return Poll::Ready(Err(copy_error(failure)));
        }
        if let Some(pending) = &mut self.pending {
            let result = std::task::ready!(Pin::new(pending).poll(cx)).unwrap_or_else(|_| {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "MCP writer exited",
                ))
            });
            self.pending = None;
            if let Err(error) = result {
                self.failure = Some(copy_error(&error));
                return Poll::Ready(Err(error));
            }
        }
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        std::task::ready!(self.as_mut().poll_flush(cx))?;
        self.stopped.store(true, Ordering::Release);
        Poll::Ready(Ok(()))
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

struct Worker {
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<io::Result<()>>>,
}
impl Worker {
    fn stop(&self) -> io::Result<()> {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = &self.thread {
            platform::cancel(thread)?;
        }
        Ok(())
    }
    fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
    fn join(&mut self) -> io::Result<()> {
        self.thread.take().map_or(Ok(()), |thread| {
            thread
                .join()
                .map_err(|_| io::Error::other("MCP stdio worker panicked"))?
        })
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // Covers failed construction and cancellation of the owning future too.
        // Repeated cancellation closes the Windows check-before-syscall race.
        while !self.finished() {
            let _ = self.stop();
            std::thread::sleep(POLL_INTERVAL);
        }
        let _ = self.join();
    }
}

pub(super) struct Stdio {
    input: Worker,
    output: Worker,
    // Release shared descriptor flags only after BOTH workers have joined, in
    // reverse acquisition order (stdin and stdout may share an open description).
    _output_handle: platform::Handle,
    _input_handle: platform::Handle,
}
impl Stdio {
    pub(super) fn open() -> io::Result<(Self, Input, Output)> {
        Self::from_handles(platform::Handle::stdin()?, platform::Handle::stdout()?)
    }
    fn from_handles(
        input: platform::Handle,
        output: platform::Handle,
    ) -> io::Result<(Self, Input, Output)> {
        let (sender, input_receiver) = async_mpsc::channel(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let read_stop = stopped.clone();
        let input_handle = input.clone();
        let thread = std::thread::Builder::new()
            .name("revenant-mcp-input".into())
            .spawn(move || {
                while !read_stop.load(Ordering::Acquire) {
                    let mut bytes = vec![0; CHUNK];
                    let result = match input_handle.read(&mut bytes, &read_stop) {
                        Ok(0) => return Ok(()),
                        Ok(length) => {
                            bytes.truncate(length);
                            Ok(bytes)
                        }
                        Err(_) if read_stop.load(Ordering::Acquire) => return Ok(()),
                        Err(error) => Err(error),
                    };
                    let mut value = result;
                    loop {
                        let failed = value.is_err();
                        match sender.try_send(value) {
                            Ok(()) if failed => return Ok(()),
                            Ok(()) => break,
                            Err(async_mpsc::error::TrySendError::Closed(_)) => return Ok(()),
                            Err(async_mpsc::error::TrySendError::Full(returned)) => {
                                if read_stop.load(Ordering::Acquire) {
                                    return Ok(());
                                }
                                value = returned;
                                std::thread::sleep(POLL_INTERVAL);
                            }
                        }
                    }
                }
                Ok(())
            })?;
        let input_worker = Worker {
            stopped,
            thread: Some(thread),
        };
        let (sender, receiver) = mpsc::channel::<Write>();
        let stopped = Arc::new(AtomicBool::new(false));
        let write_stop = stopped.clone();
        let output_handle = output.clone();
        let thread = std::thread::Builder::new()
            .name("revenant-mcp-output".into())
            .spawn(move || {
                while !write_stop.load(Ordering::Acquire) {
                    let job = match receiver.recv_timeout(POLL_INTERVAL) {
                        Ok(job) => job,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                    };
                    let result = output_handle.write(&job.bytes, &write_stop);
                    let failed = result.is_err();
                    let _ = job.completed.send(result);
                    if failed {
                        return Ok(());
                    }
                }
                Ok(())
            })?;
        let output_worker = Worker {
            stopped: stopped.clone(),
            thread: Some(thread),
        };
        Ok((
            Self {
                input: input_worker,
                output: output_worker,
                _output_handle: output,
                _input_handle: input,
            },
            Input::new(input_receiver),
            Output {
                sender,
                pending: None,
                failure: None,
                stopped,
            },
        ))
    }
    pub(super) async fn shutdown(mut self) -> io::Result<()> {
        let mut failure = None;
        while !self.input.finished() || !self.output.finished() {
            // ERROR_NOT_FOUND is expected when cancellation races the next I/O.
            for worker in [&self.input, &self.output] {
                if let Err(error) = worker.stop() {
                    failure.get_or_insert(error);
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        for worker in [&mut self.input, &mut self.output] {
            if let Err(error) = worker.join() {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs::File,
        os::fd::{AsRawFd, OwnedFd},
        os::unix::net::UnixStream,
    };

    #[tokio::test]
    async fn descriptor_flags_stay_nonblocking_until_all_workers_are_joined() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let file = File::from(OwnedFd::from(stream));
        let original = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        let (stdio, _input, _output) = Stdio::from_handles(
            platform::Handle::new(file.try_clone().unwrap()).unwrap(),
            platform::Handle::new(file.try_clone().unwrap()).unwrap(),
        )
        .unwrap();
        stdio.input.stop().unwrap();
        while !stdio.input.finished() {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        let during = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        stdio.shutdown().await.unwrap();
        let after = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(
            during & libc::O_NONBLOCK,
            0,
            "one worker cannot restore flags used by the other"
        );
        assert_eq!(
            after, original,
            "restore the shared descriptor's original flags"
        );
    }

    #[tokio::test]
    async fn shutdown_joins_blocked_native_reads_and_writes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (read_stream, _stdin_peer) = UnixStream::pair().unwrap();
        let (write_stream, _stdout_peer) = UnixStream::pair().unwrap();
        let (stdio, mut input, mut output) = Stdio::from_handles(
            platform::Handle::new(File::from(OwnedFd::from(read_stream))).unwrap(),
            platform::Handle::new(File::from(OwnedFd::from(write_stream))).unwrap(),
        )
        .unwrap();
        let payload = vec![7; 1024 * 1024];
        let write = output.write_all(&payload);
        tokio::pin!(write);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut write)
                .await
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(2), stdio.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(
            write.await.is_err(),
            "cancellation must wake the pending writer"
        );
        assert_eq!(
            input.read(&mut [0]).await.unwrap(),
            0,
            "joined reader releases its channel"
        );
    }

    #[tokio::test]
    async fn partial_native_writes_preserve_all_bytes_and_flush_acknowledgement() {
        use tokio::io::AsyncWriteExt;
        let (read_stream, _stdin_peer) = UnixStream::pair().unwrap();
        let (write_stream, mut stdout_peer) = UnixStream::pair().unwrap();
        // Force writes larger than the socket buffer through the partial-write path.
        let capacity: libc::c_int = 4096;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    write_stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&capacity as *const libc::c_int).cast(),
                    std::mem::size_of_val(&capacity) as libc::socklen_t,
                )
            },
            0
        );
        let (stdio, _input, mut output) = Stdio::from_handles(
            platform::Handle::new(File::from(OwnedFd::from(read_stream))).unwrap(),
            platform::Handle::new(File::from(OwnedFd::from(write_stream))).unwrap(),
        )
        .unwrap();
        let reader = std::thread::spawn(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            stdout_peer.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let payload: Vec<_> = (0..100_000).map(|index| (index % 251) as u8).collect();
        output.write_all(&payload).await.unwrap();
        output.flush().await.unwrap();
        drop(output);
        stdio.shutdown().await.unwrap();
        assert_eq!(reader.join().unwrap(), payload);
    }
}
