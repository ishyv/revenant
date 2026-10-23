//! MCP stdio adapter backed by the official Rust SDK.
use crate::Backend;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
        ToolAnnotations,
    },
    service::RequestContext,
};
use std::sync::Arc;

#[derive(Clone)]
struct Adapter(Arc<dyn Backend>);

impl ServerHandler for Adapter {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("revenant", env!("CARGO_PKG_VERSION")))
            .with_instructions("Application operations return owned task receipts. Use the advertised task tools to inspect results and cancel work. Closing this connection releases its ownership.")
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = self
            .0
            .tools()
            .into_iter()
            .map(|definition| {
                let mut tool = Tool::new(
                    definition.name,
                    definition.description,
                    definition
                        .input_schema
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                );
                tool.annotations = Some(ToolAnnotations::new().read_only(definition.read_only));
                tool.output_schema = definition
                    .output_schema
                    .and_then(|schema| schema.as_object().cloned())
                    .map(Arc::new);
                tool
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let arguments = serde_json::Value::Object(request.arguments.unwrap_or_default());
        let outcome = tokio::select! {
            _ = context.ct.cancelled() => Err(revenant_core::Error::new("request_cancelled", "MCP request was cancelled")),
            result = self.0.call(&request.name, arguments) => result,
        };
        let result = match outcome {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => {
                let value = serde_json::to_value(error)
                    .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                let mut result = CallToolResult::error(vec![ContentBlock::text(value.to_string())]);
                result.structured_content = Some(value);
                result
            }
        };
        Ok(result.into())
    }
}

/// Serve newline-delimited MCP on stdin/stdout; diagnostics belong on stderr.
/// The backend closes on EOF, protocol failure, and orderly peer shutdown.
/// Standard I/O is exclusively owned until backend cleanup and native worker
/// joins finish. Pending responses get one second to drain after terminal input;
/// this does not impose a deadline on arbitrary backend work.
pub async fn serve_stdio(backend: Arc<dyn Backend>) -> anyhow::Result<()> {
    serve_native(backend, crate::stdio::Stdio::open()).await
}

async fn serve_native(
    backend: Arc<dyn Backend>,
    stdio: std::io::Result<(
        crate::stdio::Stdio,
        crate::stdio::Input,
        crate::stdio::Output,
    )>,
) -> anyhow::Result<()> {
    let (stdio, input, output) = match stdio {
        Ok(stdio) => stdio,
        Err(error) => {
            return match backend.close().await {
                Ok(()) => Err(error.into()),
                Err(cleanup) => Err(anyhow::Error::new(error)
                    .context(format!("backend cleanup also failed: {cleanup}"))),
            };
        }
    };
    let result = serve_io(backend, input, output).await;
    let stopped = stdio.shutdown().await;
    match (result, stopped) {
        (Err(error), Err(stopped)) => {
            Err(error.context(format!("stdio cleanup also failed: {stopped}")))
        }
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error.into()),
        _ => Ok(()),
    }
}

// Bound bytes before the SDK's newline accumulator allocates them. Keep the
// partial-frame count across polls: concurrent responses may cancel a read.
const MAX_INPUT_FRAME: usize = 1024 * 1024;
struct FrameBounded<R> {
    input: R,
    frame_bytes: usize,
}
impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for FrameBounded<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        output: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let mut bytes = [0; 8192];
        let length = output.remaining().min(bytes.len());
        let mut buffer = tokio::io::ReadBuf::new(&mut bytes[..length]);
        match std::pin::Pin::new(&mut this.input).poll_read(cx, &mut buffer) {
            std::task::Poll::Ready(Ok(())) => {
                for byte in buffer.filled() {
                    if *byte == b'\n' {
                        this.frame_bytes = 0;
                    } else {
                        this.frame_bytes += 1;
                        if this.frame_bytes > MAX_INPUT_FRAME {
                            return std::task::Poll::Ready(Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "MCP input frame exceeds 1 MiB",
                            )));
                        }
                    }
                }
                output.put_slice(buffer.filled());
                std::task::Poll::Ready(Ok(()))
            }
            std::task::Poll::Ready(Err(error)) => std::task::Poll::Ready(Err(error)),
            other => other,
        }
    }
}

// Interrupt SDK sends after grace. Dropping the native writer then signals its
// worker; serve_stdio cancels and joins native I/O before returning to its caller.
struct InterruptibleWrite<W> {
    output: W,
    interrupt: tokio::sync::oneshot::Receiver<()>,
    interrupted: bool,
    failed: Option<tokio::sync::oneshot::Sender<std::io::Error>>,
}
impl<W> InterruptibleWrite<W> {
    fn observe<T>(
        &mut self,
        result: std::task::Poll<std::io::Result<T>>,
    ) -> std::task::Poll<std::io::Result<T>> {
        if let std::task::Poll::Ready(Err(error)) = &result {
            if let Some(sender) = self.failed.take() {
                let _ = sender.send(std::io::Error::new(error.kind(), error.to_string()));
            }
        }
        result
    }
    fn check(&mut self, cx: &mut std::task::Context<'_>) -> std::io::Result<()> {
        use std::future::Future;
        if !self.interrupted {
            self.interrupted = std::pin::Pin::new(&mut self.interrupt).poll(cx).is_ready();
        }
        if self.interrupted {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "MCP output shutdown grace expired",
            ))
        } else {
            Ok(())
        }
    }
}
impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for InterruptibleWrite<W> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        this.check(cx)?;
        let result = std::pin::Pin::new(&mut this.output).poll_write(cx, bytes);
        this.observe(result)
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        this.check(cx)?;
        let result = std::pin::Pin::new(&mut this.output).poll_flush(cx);
        this.observe(result)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        this.check(cx)?;
        let result = std::pin::Pin::new(&mut this.output).poll_shutdown(cx);
        this.observe(result)
    }
}

async fn serve_io(
    backend: Arc<dyn Backend>,
    input: impl tokio::io::AsyncRead + Send + Unpin + 'static,
    output: impl tokio::io::AsyncWrite + Send + Unpin + 'static,
) -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWrite};
    // Read independently of rmcp, which can await output while initializing or
    // sending a protocol error. Backlog saturation is terminal, not an unbounded
    // read-ahead allocation or a reader waiting forever behind a blocked writer.
    // Count bytes, not reads: even byte-at-a-time peers can use the full bound.
    let (mut buffered, sdk_input) = tokio::io::duplex(2 * MAX_INPUT_FRAME);
    let (terminal, mut input_ended) = tokio::sync::oneshot::channel();
    let mut pump = InputPump(tokio::spawn(async move {
        let result = async {
            let mut input = FrameBounded {
                input,
                frame_bytes: 0,
            };
            loop {
                let mut bytes = vec![0; crate::stdio::CHUNK];
                let length = input.read(&mut bytes).await?;
                if length == 0 {
                    return Ok(());
                }
                bytes.truncate(length);
                let mut pending = &bytes[..];
                while !pending.is_empty() {
                    // Scope out cooperative scheduling for this *try*-write:
                    // a quota yield is not a full buffer. Yield after the chunk.
                    let length = tokio::task::unconstrained(std::future::poll_fn(|cx| {
                        match std::pin::Pin::new(&mut buffered).poll_write(cx, pending) {
                            std::task::Poll::Ready(result) => std::task::Poll::Ready(result),
                            std::task::Poll::Pending => std::task::Poll::Ready(Err(
                                std::io::Error::other("MCP input backlog exceeds 2 MiB"),
                            )),
                        }
                    }))
                    .await?;
                    if length == 0 {
                        return Err(std::io::ErrorKind::WriteZero.into());
                    }
                    pending = &pending[length..];
                }
                tokio::task::yield_now().await;
            }
        }
        .await;
        let _ = terminal.send(result);
    }));
    let mut cleanup = None;
    let (interrupt, interrupted) = tokio::sync::oneshot::channel();
    let (failed, mut output_failed) = tokio::sync::oneshot::channel();
    let mut input_failure = None;
    let mut output_failure = None;
    let stopping = tokio::sync::Notify::new();
    let session = async {
        let service = Adapter(backend.clone())
            .serve((
                sdk_input,
                InterruptibleWrite {
                    output,
                    interrupt: interrupted,
                    interrupted: false,
                    failed: Some(failed),
                },
            ))
            .await?;
        let cancellation = service.cancellation_token();
        let waiting = service.waiting();
        tokio::pin!(waiting);
        tokio::select! {
            result = &mut waiting => { result?; }
            _ = stopping.notified() => {
                cancellation.cancel();
                waiting.await?;
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    let outcome = {
        tokio::pin!(session);
        enum End {
            Session(anyhow::Result<()>),
            Input(Result<std::io::Result<()>, tokio::sync::oneshot::error::RecvError>),
            Output(std::io::Error),
        }
        let end = tokio::select! {
            result = &mut session => End::Session(result),
            terminal = &mut input_ended => End::Input(terminal),
            Ok(error) = &mut output_failed => End::Output(error),
        };
        match end {
            End::Session(result) => result,
            terminal => {
                match terminal {
                    End::Input(Ok(Ok(()))) => {
                        // The monitor can see EOF ahead of buffered requests.
                        // Let rmcp consume those bytes and reach EOF naturally.
                    }
                    End::Input(result) => {
                        input_failure = Some(match result {
                            Ok(Err(error)) => error,
                            Err(error) => {
                                std::io::Error::other(format!("MCP input monitor failed: {error}"))
                            }
                            _ => unreachable!(),
                        });
                        stopping.notify_one();
                    }
                    End::Output(error) => {
                        output_failure = Some(error);
                        stopping.notify_one();
                    }
                    End::Session(_) => unreachable!(),
                }
                let drain = async {
                    match tokio::time::timeout(std::time::Duration::from_secs(1), &mut session)
                        .await
                    {
                        Ok(result) => result,
                        Err(_) => {
                            // Notify retains a permit if still initializing.
                            stopping.notify_one();
                            let _ = interrupt.send(());
                            session.await
                        }
                    }
                };
                let (closed, drained) = tokio::join!(backend.close(), drain);
                cleanup = Some(closed);
                drained
            }
        }
    };
    // Do not detach the pump if the SDK exits with stdin still open. Native
    // read cancellation is subsequently owned by serve_stdio, not Tokio's pool.
    pump.0.abort();
    let monitored = (&mut pump.0).await;
    if let Err(error) = monitored {
        if !error.is_cancelled() {
            input_failure.get_or_insert_with(|| std::io::Error::other(error));
        }
    }
    // EOF closes SDK ingress only after the pump publishes its outcome. Collect
    // it after joining even when the SDK won select!, or an I/O error can become
    // a generic initialization failure (or a successful established close).
    if let Ok(Err(error)) = input_ended.try_recv() {
        input_failure = Some(error);
    }
    if let Ok(error) = output_failed.try_recv() {
        output_failure = Some(error);
    }
    let mut outcome = outcome;
    for (label, failure) in [("input", input_failure), ("output", output_failure)] {
        if let Some(error) = failure {
            outcome = Err(match outcome {
                Ok(()) => error.into(),
                Err(previous) => previous.context(format!("MCP {label} failed: {error}")),
            });
        }
    }
    let cleanup = match cleanup {
        Some(cleanup) => cleanup,
        None => backend.close().await,
    };
    match (outcome, cleanup) {
        (Err(error), Err(cleanup)) => {
            return Err(error.context(format!("backend cleanup also failed: {cleanup}")));
        }
        (Err(error), _) => return Err(error),
        (_, Err(error)) => return Err(error.into()),
        _ => {}
    }
    Ok(())
}

struct InputPump(tokio::task::JoinHandle<()>);
impl Drop for InputPump {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentTool, BackendFuture};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::AsyncWriteExt;

    struct BackendProbe(AtomicBool);
    impl Backend for BackendProbe {
        fn tools(&self) -> Vec<AgentTool> {
            Vec::new()
        }
        fn call<'a>(
            &'a self,
            _: &'a str,
            _: serde_json::Value,
        ) -> BackendFuture<'a, serde_json::Value> {
            Box::pin(async { Ok(serde_json::Value::Null) })
        }
        fn close(&self) -> BackendFuture<'_, ()> {
            Box::pin(async {
                self.0.store(true, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    struct ActiveProbe {
        active: tokio::sync::Notify,
        closing: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    impl Backend for ActiveProbe {
        fn tools(&self) -> Vec<AgentTool> {
            Vec::new()
        }
        fn call<'a>(
            &'a self,
            _: &'a str,
            _: serde_json::Value,
        ) -> BackendFuture<'a, serde_json::Value> {
            Box::pin(async move {
                self.active.notify_one();
                std::future::pending().await
            })
        }
        fn close(&self) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                self.closing.notify_one();
                self.release.notified().await;
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn eof_starts_cleanup_before_active_request_drain_and_awaits_it() {
        use tokio::io::AsyncBufReadExt;
        let backend = Arc::new(ActiveProbe {
            active: tokio::sync::Notify::new(),
            closing: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let (peer, server_io) = tokio::io::duplex(8192);
        let (input, output) = tokio::io::split(server_io);
        let (reader, mut writer) = tokio::io::split(peer);
        let server = tokio::spawn(serve_io(backend.clone(), input, output));
        writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"probe\",\"version\":\"1\"}}}\n").await.unwrap();
        let mut reader = tokio::io::BufReader::new(reader);
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reader.read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&line)
                .unwrap()
                .get("result")
                .is_some(),
            "{line}"
        );
        writer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"probe\",\"arguments\":{}}}\n").await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), backend.active.notified())
            .await
            .unwrap();
        writer.shutdown().await.unwrap();
        let prompt = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            backend.closing.notified(),
        )
        .await
        .is_ok();
        let returned_before_cleanup = server.is_finished();
        backend.release.notify_one();
        let finished = tokio::time::timeout(std::time::Duration::from_secs(7), server).await;
        assert!(
            prompt,
            "EOF must start cleanup before rmcp drains active requests"
        );
        assert!(!returned_before_cleanup, "serve must await backend cleanup");
        finished.unwrap().unwrap().unwrap();
    }

    #[tokio::test]
    async fn output_interrupt_wakes_blocked_flush_and_stays_latched() {
        use tokio::io::AsyncWriteExt;
        struct BlockedFlush;
        impl tokio::io::AsyncWrite for BlockedFlush {
            fn poll_write(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                bytes: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::task::Poll::Ready(Ok(bytes.len()))
            }
            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Pending
            }
            fn poll_shutdown(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Pending
            }
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut output = InterruptibleWrite {
            output: BlockedFlush,
            interrupt: receiver,
            interrupted: false,
            failed: None,
        };
        output.write_all(b"accepted before flush").await.unwrap();
        {
            use std::future::Future;
            let flush = output.flush();
            tokio::pin!(flush);
            struct WakeProbe(AtomicBool);
            impl std::task::Wake for WakeProbe {
                fn wake(self: Arc<Self>) {
                    self.0.store(true, Ordering::SeqCst);
                }
                fn wake_by_ref(self: &Arc<Self>) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let probe = Arc::new(WakeProbe(AtomicBool::new(false)));
            let waker = std::task::Waker::from(probe.clone());
            assert!(
                flush
                    .as_mut()
                    .poll(&mut std::task::Context::from_waker(&waker))
                    .is_pending()
            );
            sender.send(()).unwrap();
            assert!(
                probe.0.load(Ordering::SeqCst),
                "interrupt must wake the pending flush before it is repolled"
            );
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(1), flush)
                    .await
                    .unwrap()
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::BrokenPipe
            );
        }
        assert_eq!(
            output.flush().await.unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert_eq!(
            output.write(b"after").await.unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert_eq!(
            output.shutdown().await.unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }

    struct ObservedWrite<W> {
        output: W,
        armed: Arc<AtomicBool>,
        blocked: Arc<tokio::sync::Notify>,
        dropped: Arc<AtomicBool>,
    }
    impl<W> Drop for ObservedWrite<W> {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }
    impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for ObservedWrite<W> {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            let this = self.get_mut();
            let result = std::pin::Pin::new(&mut this.output).poll_write(cx, bytes);
            if result.is_pending() && this.armed.load(Ordering::SeqCst) {
                this.blocked.notify_one();
            }
            result
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let this = self.get_mut();
            let result = std::pin::Pin::new(&mut this.output).poll_flush(cx);
            if result.is_pending() && this.armed.load(Ordering::SeqCst) {
                this.blocked.notify_one();
            }
            result
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.get_mut().output).poll_shutdown(cx)
        }
    }

    async fn blocked_response_eof(resume_output: bool) {
        use std::time::Duration;
        use tokio::io::AsyncBufReadExt;
        let backend = Arc::new(BackendProbe(AtomicBool::new(false)));
        let (mut writer, input) = tokio::io::duplex(8192);
        let (output, reader) = tokio::io::duplex(1);
        let mut reader = tokio::io::BufReader::new(reader);
        let armed = Arc::new(AtomicBool::new(false));
        let blocked = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let mut server = tokio::spawn(serve_io(
            backend.clone(),
            input,
            ObservedWrite {
                output,
                armed: armed.clone(),
                blocked: blocked.clone(),
                dropped: dropped.clone(),
            },
        ));
        writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"probe\",\"version\":\"1\"}}}\n").await.unwrap();
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&line)
                .unwrap()
                .get("result")
                .is_some()
        );
        armed.store(true, Ordering::SeqCst);
        writer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n").await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), blocked.notified())
            .await
            .unwrap();
        writer.shutdown().await.unwrap();
        let prompt_cleanup = tokio::time::timeout(Duration::from_secs(1), async {
            while !backend.0.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        if resume_output {
            line.clear();
            tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            let response: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["id"], 2);
            assert!(response.get("result").is_some());
        }
        // Borrow the handle: on failure unblock and reap it, never detach it.
        let result = tokio::time::timeout(Duration::from_secs(7), &mut server).await;
        let finished = result.is_ok();
        drop(reader);
        if let Ok(result) = result {
            result.unwrap().unwrap();
        } else {
            server.await.unwrap().unwrap();
        }
        assert!(prompt_cleanup, "cleanup must not wait for blocked stdout");
        assert!(
            finished,
            "EOF left rmcp transport close stuck behind a blocked response"
        );
        assert!(
            dropped.load(Ordering::SeqCst),
            "writer must be released before serve returns"
        );
    }

    #[tokio::test]
    async fn eof_releases_blocked_async_writer_and_joins_service() {
        blocked_response_eof(false).await;
    }

    #[tokio::test]
    async fn eof_preserves_response_when_output_resumes_during_grace() {
        blocked_response_eof(true).await;
    }

    async fn blocked_handshake_eof(request: &[u8], resume_output: bool) {
        use std::time::Duration;
        let backend = Arc::new(BackendProbe(AtomicBool::new(false)));
        let (mut writer, input) = tokio::io::duplex(8192);
        let (output, mut reader) = tokio::io::duplex(1);
        let blocked = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let mut server = tokio::spawn(serve_io(
            backend.clone(),
            input,
            ObservedWrite {
                output,
                armed: Arc::new(AtomicBool::new(true)),
                blocked: blocked.clone(),
                dropped: dropped.clone(),
            },
        ));
        writer.write_all(request).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), blocked.notified())
            .await
            .unwrap();
        writer.shutdown().await.unwrap();
        let closed = tokio::time::timeout(Duration::from_millis(500), async {
            while !backend.0.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        if resume_output {
            use tokio::io::AsyncBufReadExt;
            let mut reader = tokio::io::BufReader::new(&mut reader);
            let mut line = String::new();
            tokio::time::timeout(Duration::from_millis(500), reader.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            let response: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["id"], 1);
            assert!(response.get("result").is_some() || response.get("error").is_some());
        }
        let result = tokio::time::timeout(Duration::from_secs(3), &mut server).await;
        let finished = result.is_ok();
        // Release every owned task on failure as well as on success.
        drop(reader);
        match result {
            Ok(result) => {
                let _ = result.expect("MCP server task panicked");
            }
            Err(_) => {
                let _ = server
                    .await
                    .expect("MCP server task panicked during cleanup");
            }
        }
        assert!(closed, "EOF must start cleanup during a blocked handshake");
        assert!(
            finished,
            "blocked handshake must be interrupted after grace"
        );
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn eof_during_initialization_starts_cleanup_and_interrupts_output() {
        blocked_handshake_eof(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"probe\",\"version\":\"1\"}}}\n", false).await;
    }

    #[tokio::test]
    async fn eof_during_initialization_error_starts_cleanup_and_interrupts_output() {
        blocked_handshake_eof(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
            false,
        )
        .await;
    }

    #[tokio::test]
    async fn initialization_response_is_preserved_when_output_resumes_during_grace() {
        blocked_handshake_eof(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"probe\",\"version\":\"1\"}}}\n", true).await;
    }

    #[tokio::test]
    async fn initialization_error_is_preserved_when_output_resumes_during_grace() {
        blocked_handshake_eof(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
            true,
        )
        .await;
    }

    #[tokio::test]
    async fn cleanup_runs_once_and_retains_transport_and_backend_failures() {
        use std::sync::atomic::AtomicUsize;
        struct FailingClose(AtomicUsize);
        impl Backend for FailingClose {
            fn tools(&self) -> Vec<AgentTool> {
                Vec::new()
            }
            fn call<'a>(
                &'a self,
                _: &'a str,
                _: serde_json::Value,
            ) -> BackendFuture<'a, serde_json::Value> {
                Box::pin(async { unreachable!("no tool call is sent") })
            }
            fn close(&self) -> BackendFuture<'_, ()> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    Err(revenant_core::Error::new(
                        "cleanup_fixture",
                        "backend close failed",
                    ))
                })
            }
        }
        let backend = Arc::new(FailingClose(AtomicUsize::new(0)));
        let result = serve_io(backend.clone(), tokio::io::empty(), tokio::io::sink()).await;
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("backend cleanup also failed"), "{error}");
        assert!(
            error.contains("initialize"),
            "retain the transport cause: {error}"
        );
        assert_eq!(backend.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stdio_open_failure_still_closes_backend() {
        let backend = Arc::new(BackendProbe(AtomicBool::new(false)));
        let result = serve_native(
            backend.clone(),
            Err(std::io::Error::other("fixture cannot open stdio")),
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot open stdio")
        );
        assert!(backend.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn input_backlog_saturation_is_a_transport_error_and_closes_backend() {
        let backend = Arc::new(BackendProbe(AtomicBool::new(false)));
        let mut payload = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_vec();
        // Empty frames obey the individual frame limit. The SDK cannot consume
        // them while the initial ping response is blocked on the one-byte sink.
        payload.resize(3 * MAX_INPUT_FRAME, b'\n');
        let (output, _reader) = tokio::io::duplex(1);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            serve_io(backend.clone(), std::io::Cursor::new(payload), output),
        )
        .await
        .expect("saturated input must not deadlock behind output");
        assert!(result.unwrap_err().to_string().contains("backlog exceeds"));
        assert!(backend.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn fragmented_input_does_not_exhaust_the_byte_backlog_limit() {
        struct OneByte(std::io::Cursor<Vec<u8>>);
        impl tokio::io::AsyncRead for OneByte {
            fn poll_read(
                mut self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                output: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                use std::io::Read;
                if output.remaining() != 0 {
                    let mut byte = [0];
                    let length = self.0.read(&mut byte)?;
                    output.put_slice(&byte[..length]);
                }
                std::task::Poll::Ready(Ok(()))
            }
        }
        let mut bytes = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"probe\",\"version\":\"1\"}}}\n".to_vec();
        bytes.resize(2048, b'\n');
        let backend = Arc::new(BackendProbe(AtomicBool::new(false)));
        serve_io(
            backend.clone(),
            OneByte(std::io::Cursor::new(bytes)),
            tokio::io::sink(),
        )
        .await
        .unwrap();
        assert!(backend.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn postinitialize_output_failure_closes_backend_while_input_is_open() {
        use tokio::io::AsyncBufReadExt;
        let backend = Arc::new(BackendProbe(AtomicBool::new(false)));
        let (mut writer, input) = tokio::io::duplex(8192);
        let (output, reader) = tokio::io::duplex(8192);
        let mut reader = tokio::io::BufReader::new(reader);
        let mut server = tokio::spawn(serve_io(backend.clone(), input, output));
        writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"probe\",\"version\":\"1\"}}}\n").await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reader.read_line(&mut String::new()),
        )
        .await
        .expect("MCP initialization response timeout")
        .unwrap();
        drop(reader);
        writer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n").await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), &mut server).await;
        let finished = result.is_ok();
        if let Ok(result) = result {
            assert!(result.unwrap().is_err(), "report the broken output");
        }
        drop(writer);
        if !finished {
            let _ = server.await.unwrap();
        }
        assert!(finished, "stdout failure must not wait for stdin EOF");
        assert!(backend.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn clean_eof_preserves_buffered_ping_responses() {
        use tokio::io::AsyncReadExt;
        for _ in 0..32 {
            let backend = Arc::new(BackendProbe(AtomicBool::new(false)));
            let bytes = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"probe\",\"version\":\"1\"}}}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n";
            let (output, mut reader) = tokio::io::duplex(8192);
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                serve_io(backend, &bytes[..], output),
            )
            .await
            .expect("MCP EOF drain timeout")
            .unwrap();
            let mut responses = String::new();
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                reader.read_to_string(&mut responses),
            )
            .await
            .expect("MCP writer was not released after EOF")
            .unwrap();
            let ids: Vec<_> = responses
                .lines()
                .map(|line| {
                    serde_json::from_str::<serde_json::Value>(line).unwrap()["id"]
                        .as_u64()
                        .unwrap()
                })
                .collect();
            assert_eq!(ids, vec![1, 2], "EOF must drain accepted ping: {responses}");
        }
    }

    #[tokio::test]
    async fn input_error_is_retained_when_the_sdk_finishes_first() {
        struct ReadError;
        impl tokio::io::AsyncRead for ReadError {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Err(std::io::Error::other("unique input failure")))
            }
        }
        for _ in 0..32 {
            let backend = Arc::new(BackendProbe(AtomicBool::new(false)));
            let error = serve_io(backend, ReadError, tokio::io::sink())
                .await
                .unwrap_err();
            assert!(
                format!("{error:#}").contains("unique input failure"),
                "{error:#}"
            );
        }
    }

    #[tokio::test]
    async fn frame_bound_accepts_exact_limit_and_resets_for_each_newline() {
        use tokio::io::AsyncReadExt;
        let mut payload = vec![b'x'; MAX_INPUT_FRAME];
        payload.push(b'\n');
        payload.extend(vec![b'y'; MAX_INPUT_FRAME]);
        payload.push(b'\n');
        let mut input = FrameBounded {
            input: std::io::Cursor::new(payload.clone()),
            frame_bytes: 0,
        };
        let mut received = Vec::new();
        input.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, payload);
    }

    #[tokio::test]
    async fn frame_bound_rejects_oversized_complete_line() {
        use tokio::io::AsyncReadExt;
        let mut payload = vec![b'x'; MAX_INPUT_FRAME + 1];
        payload.push(b'\n');
        let mut input = FrameBounded {
            input: std::io::Cursor::new(payload),
            frame_bytes: 0,
        };
        let mut received = Vec::new();
        assert_eq!(
            input.read_to_end(&mut received).await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert!(received.len() <= MAX_INPUT_FRAME);
    }

    #[tokio::test]
    async fn oversized_unterminated_stdio_frame_closes_backend_without_waiting_for_eof() {
        let backend = Arc::new(BackendProbe(AtomicBool::new(false)));
        let (mut peer, input) = tokio::io::duplex(8192);
        let server_backend = backend.clone();
        let server = tokio::spawn(serve_io(server_backend, input, tokio::io::sink()));
        let writer = tokio::spawn(async move {
            let _ = peer.write_all(&vec![b'x'; 1024 * 1024 + 1]).await;
            // Keep stdin open: a frame limit must not rely on newline or EOF.
            std::future::pending::<()>().await;
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), server).await;
        writer.abort();
        assert!(
            result.is_ok(),
            "oversized input remained buffered until EOF"
        );
        assert!(result.unwrap().unwrap().is_err());
        assert!(
            backend.0.load(Ordering::SeqCst),
            "backend cleanup was skipped"
        );
    }
}
