//! Private Tauri transport and lifecycle wiring for the generated desktop host.
//! Native ownership, dispatch semantics, and preview URL formatting stay in the
//! runtime; application authors only supply their application and host options.

#![cfg(feature = "tauri-host")]

use crate::{DesktopOptions, DesktopRuntime, Error, Result, UpdateSink};
use revenant_sdk::Application;
use serde_json::Value;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tauri::{
    Manager, RunEvent, State, WebviewWindow, WindowEvent, Wry,
    http::{self, HeaderValue, Method, Response, StatusCode, header},
    ipc::{Channel, JavaScriptChannelId},
    webview::PageLoadEvent,
};
use tauri_plugin_dialog::DialogExt;

const SHUTDOWN_LIMIT: Duration = Duration::from_secs(10);

/// Launches the native desktop host with private IPC, previews, and lifecycle hooks.
///
/// Generated bootstrap supplies the builder and context. Missing storage paths
/// use Tauri's platform application directories; explicit paths retain priority.
pub fn launch(
    builder: tauri::Builder<Wry>,
    context: tauri::Context<Wry>,
    application: Application,
    mut options: DesktopOptions,
) -> Result<()> {
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![revenant_dispatch])
        .on_page_load(|webview, payload| {
            if !matches!(payload.event(), PageLoadEvent::Started) {
                return;
            }
            let Some(runtime) = webview.try_state::<DesktopRuntime>() else {
                return;
            };
            let runtime = runtime.inner().clone();
            let label = webview.window().label().to_owned();
            // Capture old roots at navigation start, before the new page can
            // connect. A queued cleanup must never discover and retire its roots.
            let roots: Vec<_> = crate::runtime::lock(&runtime.inner.scopes)
                .iter()
                .filter(|(_, scope)| scope.window == label && scope.parent.is_none())
                .map(|(id, _)| id.clone())
                .collect();
            if !roots.is_empty() {
                tauri::async_runtime::spawn(cleanup_window(runtime, label, Some(roots)));
            }
        })
        .register_asynchronous_uri_scheme_protocol(
            "revenant-preview",
            |context, request, responder| {
                let Some(runtime) = context.app_handle().try_state::<DesktopRuntime>() else {
                    responder.respond(protocol_error(Error::new(
                        "app_closing",
                        "Native runtime is unavailable",
                    )));
                    return;
                };
                let runtime = runtime.inner().clone();
                let Some(webview) = context
                    .app_handle()
                    .get_webview_window(context.webview_label())
                else {
                    responder.respond(protocol_error(Error::new(
                        "scope_disposed",
                        "Webview has been destroyed",
                    )));
                    return;
                };
                tauri::async_runtime::spawn(async move {
                    // File ports perform synchronous I/O while their future is polled.
                    // Keep that entire poll on the runtime's bounded control worker.
                    let response = runtime
                        .control(move |runtime| preview_response(&runtime, &webview, request))
                        .await
                        .unwrap_or_else(protocol_error);
                    responder.respond(response);
                });
            },
        )
        .setup(move |app| {
            if options.data_dir.is_none() {
                options.data_dir = Some(app.path().app_data_dir()?);
            }
            if options.cache_dir.is_none() {
                options.cache_dir = Some(app.path().app_cache_dir()?);
            }
            // Setup runs on Tauri's event thread. Provider preparation and native
            // directory creation run on a blocking worker with Tokio available.
            let runtime = tauri::async_runtime::block_on(async move {
                tauri::async_runtime::spawn_blocking(move || {
                    tauri::async_runtime::block_on(DesktopRuntime::new(application, options))
                })
                .await
                .map_err(host_error)?
            })?;
            if !app.manage(runtime) {
                return Err(Error::new(
                    "native_runtime_exists",
                    "A desktop runtime is already managed",
                )
                .into());
            }
            Ok(())
        })
        .build(context)
        .map_err(host_error)?;

    let mut shutdown_started = false;
    let exit_ready = Arc::new(AtomicBool::new(false));
    app.run(move |app, event| match event {
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Destroyed,
            ..
        } => {
            if let Some(runtime) = app.try_state::<DesktopRuntime>() {
                let runtime = runtime.inner().clone();
                tauri::async_runtime::spawn(cleanup_window(runtime, label, None));
            }
        }
        RunEvent::ExitRequested { api, code, .. } if !exit_ready.load(Ordering::Acquire) => {
            api.prevent_exit();
            // Repeated close requests cannot bypass the single shutdown pass.
            // The exit requested by that pass is allowed through after cleanup.
            if !shutdown_started {
                shutdown_started = true;
                let app = app.clone();
                let runtime = app
                    .try_state::<DesktopRuntime>()
                    .map(|state| state.inner().clone());
                let exit_ready = exit_ready.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(runtime) = runtime {
                        let shutdown = tauri::async_runtime::spawn_blocking(move || {
                            tauri::async_runtime::block_on(runtime.shutdown())
                        });
                        // The watchdog is independent of blocking flush/provider
                        // work: an uncooperative worker cannot extend this deadline.
                        match tokio::time::timeout(SHUTDOWN_LIMIT, shutdown).await {
                            Ok(Ok(Ok(()))) => {}
                            Ok(Ok(Err(error))) => eprintln!("Revenant shutdown: {error}"),
                            Ok(Err(error)) => eprintln!("Revenant shutdown worker: {error}"),
                            Err(_) => eprintln!("Revenant shutdown exceeded its ten-second grace"),
                        }
                    }
                    exit_ready.store(true, Ordering::Release);
                    app.exit(code.unwrap_or(0));
                });
            }
        }
        _ => {}
    });
    Ok(())
}

async fn cleanup_window(runtime: DesktopRuntime, label: String, roots: Option<Vec<String>>) {
    loop {
        let label = label.clone();
        let roots = roots.clone();
        let result = runtime
            .control(move |runtime| {
                if let Some(roots) = roots {
                    for scope in roots {
                        runtime.dispose_scope(&scope)?;
                    }
                    Ok(())
                } else {
                    runtime.dispose_window(&label)
                }
            })
            .await;
        match result {
            // Lifecycle cleanup has no caller to retry admission. Keep one
            // pending cleanup rather than leaking roots during a busy queue.
            Err(error) if error.code == "control_queue_full" => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => {
                eprintln!("Revenant window cleanup: {error}");
                return;
            }
            Ok(()) => return,
        }
    }
}

#[tauri::command]
async fn revenant_dispatch(
    window: WebviewWindow<Wry>,
    runtime: State<'_, DesktopRuntime>,
    request: Value,
    updates: Option<JavaScriptChannelId>,
) -> Result<Value> {
    if request.get("action").and_then(Value::as_str) == Some("folder.open") {
        if serde_json::to_vec(&request)?.len() > runtime.inner.options.max_contract_bytes {
            return Err(Error::new(
                "request_too_large",
                "Use native resources instead of large IPC payloads",
            ));
        }
        let scope = request
            .get("scope")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("invalid_request", "Missing string field scope"))?
            .to_owned();
        let label = window.label().to_owned();
        runtime.validate_window(&scope, &label)?;
        runtime.accepting()?;
        let picker = runtime
            .inner
            .pickers
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                Error::new("picker_busy", "A native folder picker is already open").retryable(true)
            })?;
        // A human may leave the picker open indefinitely. Its wait must not
        // occupy a bounded control worker needed by views and cancellation.
        let selected = tauri::async_runtime::spawn_blocking(move || {
            let _picker = picker;
            window
                .dialog()
                .file()
                .set_parent(&window)
                .blocking_pick_folder()
        })
        .await
        .map_err(host_error)?;
        let Some(selected) = selected else {
            return Ok(Value::Null);
        };
        let path = selected.into_path().map_err(|_| {
            Error::new("invalid_folder", "Picker did not return a native directory")
        })?;
        return runtime
            .control(move |runtime| {
                // Ownership may have been disposed while the picker was open.
                runtime.validate_window(&scope, &label)?;
                runtime.open_folder(&scope, path)
            })
            .await;
    }
    let sink = updates.map(|id| {
        let channel: Channel<Value> = id.channel_on(window.as_ref().clone());
        Arc::new(move |snapshot| {
            // Channels carry the same snapshots returned by dispatch, without an
            // envelope. A closed webview does not turn native work into a failure.
            let _ = channel.send(snapshot);
        }) as UpdateSink
    });
    runtime.dispatch(window.label(), request, sink).await
}

fn preview_response(
    runtime: &DesktopRuntime,
    webview: &tauri::WebviewWindow<Wry>,
    request: http::Request<Vec<u8>>,
) -> Result<Response<Vec<u8>>> {
    // Match CORS against the actual requesting webview, never a caller-supplied
    // origin. Element loads may omit Origin; the native context still identifies
    // their webview and the random token remains their read capability.
    let url = webview.url().map_err(host_error)?;
    let mut origin = url.origin().ascii_serialization();
    if origin == "null" {
        if let Some(host) = url.host_str() {
            origin = format!("{}://{host}", url.scheme());
            if let Some(port) = url.port() {
                origin.push_str(&format!(":{port}"));
            }
        }
    }
    let origin = HeaderValue::from_str(&origin)
        .map_err(|_| Error::new("scope_mismatch", "Webview origin is invalid"))?;
    if request
        .headers()
        .get_all(header::ORIGIN)
        .iter()
        .any(|value| value != &origin)
    {
        return Err(Error::new(
            "scope_mismatch",
            "Preview request belongs to another origin",
        ));
    }

    let mut response = read_preview(runtime, request).unwrap_or_else(protocol_error);
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("Content-Range, Content-Length, Accept-Ranges"),
    );
    Ok(response)
}

fn read_preview(
    runtime: &DesktopRuntime,
    request: http::Request<Vec<u8>>,
) -> Result<Response<Vec<u8>>> {
    let token = request.uri().path().strip_prefix('/').unwrap_or("");
    if token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || request.uri().query().is_some()
    {
        return Err(Error::new("preview_not_found", "Invalid preview token"));
    }
    if request.method() == Method::OPTIONS {
        let mut response = protocol_status(StatusCode::NO_CONTENT);
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, HEAD, OPTIONS"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Range"),
        );
        return Ok(response);
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        let mut response = protocol_status(StatusCode::METHOD_NOT_ALLOWED);
        response.headers_mut().insert(
            header::ALLOW,
            HeaderValue::from_static("GET, HEAD, OPTIONS"),
        );
        return Ok(response);
    }
    let mut ranges = request.headers().get_all(header::RANGE).iter();
    let mut range = ranges.next().map(|value| value.to_str().unwrap_or(""));
    // Let the store reject malformed ranges so its 416 includes the captured
    // total. Never silently treat duplicate/invalid headers as an entire file.
    if ranges.next().is_some() || range.is_some_and(|value| value.len() > 128) {
        range = Some("");
    }
    if request.method() == Method::HEAD {
        range = None;
    }
    let preview =
        tokio::runtime::Handle::current().block_on(runtime.inner.previews.read(token, range))?;
    if request.method() == Method::HEAD {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, preview.mime)
            .header(header::CONTENT_LENGTH, preview.total.to_string())
            .header(header::CACHE_CONTROL, "no-store")
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Vec::new())
            .map_err(|_| Error::new("preview_response", "Invalid preview response metadata"));
    }
    let mut response = Response::builder()
        .status(preview.status)
        .header(header::CONTENT_TYPE, preview.mime)
        .header(header::CONTENT_LENGTH, preview.body.len().to_string())
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCEPT_RANGES, "bytes");
    if let Some(range) = preview.content_range {
        response = response.header(header::CONTENT_RANGE, range);
    }
    response
        .body(if request.method() == Method::HEAD {
            Vec::new()
        } else {
            preview.body
        })
        .map_err(|_| Error::new("preview_response", "Invalid preview response metadata"))
}

fn protocol_status(status: StatusCode) -> Response<Vec<u8>> {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn protocol_error(error: Error) -> Response<Vec<u8>> {
    let status = match error.code.as_str() {
        "preview_invalid_range" => StatusCode::RANGE_NOT_SATISFIABLE,
        "preview_not_found" | "scope_disposed" => StatusCode::NOT_FOUND,
        "scope_mismatch" => StatusCode::FORBIDDEN,
        "preview_unsupported" => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "file_changed" | "file_path_changed" => StatusCode::CONFLICT,
        "control_queue_full" | "app_closing" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut response = protocol_status(status);
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        if let Some(range) = error
            .details
            .as_ref()
            .and_then(|details| details.get("contentRange"))
            .and_then(Value::as_str)
            .and_then(|range| HeaderValue::from_str(range).ok())
        {
            response.headers_mut().insert(header::CONTENT_RANGE, range);
        }
        response
            .headers_mut()
            .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    }
    // Native I/O diagnostics can contain paths. Protocol failures expose only
    // HTTP status; structured operation errors stay on the authenticated IPC.
    response
}

fn host_error(error: impl std::fmt::Display) -> Error {
    Error::new("tauri_host", error.to_string())
}
