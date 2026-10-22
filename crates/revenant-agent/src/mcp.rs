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
pub async fn serve_stdio(backend: Arc<dyn Backend>) -> anyhow::Result<()> {
    let (input, output) = rmcp::transport::stdio();
    serve_io(backend, input, output).await
}

// Bound bytes before the SDK's newline accumulator allocates them. Keep the
// partial-frame count across polls: concurrent responses may cancel a read.
const MAX_INPUT_FRAME: usize = 1024 * 1024;
struct FrameBounded<R> {
    input: R,
    frame_bytes: usize,
    terminal: Option<tokio::sync::oneshot::Sender<()>>,
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
                if length > 0 && buffer.filled().is_empty() {
                    if let Some(sender) = this.terminal.take() {
                        let _ = sender.send(());
                    }
                }
                for byte in buffer.filled() {
                    if *byte == b'\n' {
                        this.frame_bytes = 0;
                    } else {
                        this.frame_bytes += 1;
                        if this.frame_bytes > MAX_INPUT_FRAME {
                            if let Some(sender) = this.terminal.take() {
                                let _ = sender.send(());
                            }
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
            std::task::Poll::Ready(Err(error)) => {
                if let Some(sender) = this.terminal.take() {
                    let _ = sender.send(());
                }
                std::task::Poll::Ready(Err(error))
            }
            other => other,
        }
    }
}

// ponytail: interrupts async SDK sends, not an already-running native stdout
// blocking worker. Native pipe cancellation needs separate platform acceptance.
struct InterruptibleWrite<W> {
    output: W,
    interrupt: tokio::sync::oneshot::Receiver<()>,
    interrupted: bool,
}
impl<W> InterruptibleWrite<W> {
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
        std::pin::Pin::new(&mut this.output).poll_write(cx, bytes)
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        this.check(cx)?;
        std::pin::Pin::new(&mut this.output).poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        this.check(cx)?;
        std::pin::Pin::new(&mut this.output).poll_shutdown(cx)
    }
}

async fn serve_io(
    backend: Arc<dyn Backend>,
    input: impl tokio::io::AsyncRead + Send + Unpin + 'static,
    output: impl tokio::io::AsyncWrite + Send + Unpin + 'static,
) -> anyhow::Result<()> {
    let mut cleanup = None;
    let (terminal, mut input_ended) = tokio::sync::oneshot::channel();
    let (interrupt, interrupted) = tokio::sync::oneshot::channel();
    let outcome = async {
        let service = Adapter(backend.clone())
            .serve((
                FrameBounded {
                    input,
                    frame_bytes: 0,
                    terminal: Some(terminal),
                },
                InterruptibleWrite { output, interrupt: interrupted, interrupted: false },
            ))
            .await?;
        let cancellation = service.cancellation_token();
        let waiting = service.waiting();
        tokio::pin!(waiting);
        tokio::select! {
            result = &mut waiting => { result?; }
            terminal = &mut input_ended => {
                if terminal.is_ok() {
                    cancellation.cancel();
                    let drain = async {
                        // Borrow waiting so expiry never detaches the SDK service.
                        match tokio::time::timeout(std::time::Duration::from_secs(1), &mut waiting).await {
                            Ok(result) => result,
                            Err(_) => {
                                let _ = interrupt.send(());
                                waiting.await
                            }
                        }
                    };
                    let (closed, drained) = tokio::join!(backend.close(), drain);
                    cleanup = Some(closed);
                    drained?;
                } else { waiting.await?; }
            }
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
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
            terminal: None,
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
            terminal: None,
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
