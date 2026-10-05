//! Private transport vocabulary projects native domain objects, never browser execution.
//! Window admission precedes every scoped lookup. Blocking SQLite/filesystem work
//! uses the control executor so an occupied CPU queue cannot freeze interaction.

use super::{DesktopRuntime, Error, Result, UpdateSink, lock};
use serde_json::{Value, json};

fn string<'a>(request: &'a Value, key: &str) -> Result<&'a str> {
    request
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("invalid_request", format!("Missing string field {key}")))
}
fn number(request: &Value, key: &str, fallback: u64) -> Result<u64> {
    match request.get(key) {
        None => Ok(fallback),
        Some(value) => value.as_u64().ok_or_else(|| {
            Error::new(
                "invalid_request",
                format!("{key} must be a nonnegative integer"),
            )
        }),
    }
}
fn window(request: &Value) -> Result<(u64, u32)> {
    let offset = number(request, "offset", 0)?;
    let limit = number(request, "limit", 128)?;
    if !(1..=512).contains(&limit) {
        return Err(Error::new(
            "invalid_window",
            "Window size must be between 1 and 512",
        ));
    }
    Ok((offset, limit as u32))
}

impl DesktopRuntime {
    /// Dispatches one bounded desktop request for its actual calling window.
    ///
    /// Embedders provide a snapshot sink; ordinary applications use the generated
    /// Svelte API. Unknown actions, cross-window references and disposed scopes
    /// fail with structured errors. Native picker admission belongs to the adapter.
    pub async fn dispatch(
        &self,
        calling_window: &str,
        request: Value,
        sink: Option<UpdateSink>,
    ) -> Result<Value> {
        if serde_json::to_vec(&request)?.len() > self.inner.options.max_contract_bytes {
            return Err(Error::new(
                "request_too_large",
                "Use native resources instead of large IPC payloads",
            ));
        }
        let calling_window = calling_window.to_owned();
        // Cancellation and observation never wait behind a slow SQLite query.
        if matches!(
            request.get("action").and_then(Value::as_str),
            Some("task.cancel" | "task.snapshot")
        ) {
            let scope = string(&request, "scope")?;
            self.validate_window(scope, &calling_window)?;
            let task = string(&request, "task")?;
            return Ok(serde_json::to_value(
                if request["action"] == "task.cancel" {
                    self.cancel_task(scope, task)?
                } else {
                    self.task_snapshot(scope, task)?
                },
            )?);
        }
        self.control(move |runtime| {
            let action = string(&request, "action")?;
            if action == "root.connect" {
                return runtime.connect(&calling_window);
            }
            if action == "scope.create" {
                let parent = string(&request, "parent")?;
                runtime.validate_window(parent, &calling_window)?;
                return Ok(json!({"scope":runtime.create_scope(parent)?}));
            }
            let scope = string(&request, "scope")?;
            runtime.validate_window(scope, &calling_window)?;
            // Disposal and cancellation remain admitted while the app is closing.
            if !matches!(
                action,
                "scope.dispose"
                    | "view.dispose"
                    | "folder.dispose"
                    | "selection.dispose"
                    | "task.cancel"
                    | "task.dispose"
                    | "media.dispose"
                    | "settings.save"
            ) {
                runtime.accepting()?;
            }
            let executor = tokio::runtime::Handle::current();
            match action {
                "scope.dispose" => {
                    runtime.dispose_scope(scope)?;
                    Ok(Value::Null)
                }
                "folder.reopen" => {
                    let bookmark = request.get("bookmark").ok_or_else(|| {
                        Error::new("invalid_bookmark", "A saved folder bookmark is required")
                    })?;
                    runtime.open_folder(scope, std::path::PathBuf::from(string(bookmark, "path")?))
                }
                "folder.dispose" => {
                    runtime.dispose_folder(scope, string(&request, "folder")?)?;
                    Ok(Value::Null)
                }
                "folder.query" | "view.query" => {
                    let (offset, limit) = window(&request)?;
                    let generation = number(&request, "generation", 1)?;
                    let options = request.get("options").cloned().unwrap_or_else(|| json!({}));
                    let folder = if action == "view.query" {
                        runtime
                            .view(scope, string(&request, "view")?)?
                            .folder
                            .id
                            .clone()
                    } else {
                        string(&request, "folder")?.to_owned()
                    };
                    runtime
                        .query_folder_at(scope, &folder, options, sink, offset, limit, generation)
                }
                "view.window" => {
                    let (offset, limit) = window(&request)?;
                    runtime.update_view(
                        scope,
                        string(&request, "view")?,
                        None,
                        Some(offset),
                        Some(limit),
                        Some(number(&request, "generation", 1)?),
                    )
                }
                "view.ack" => {
                    runtime.acknowledge_view(
                        scope,
                        string(&request, "view")?,
                        number(&request, "revision", 0)?,
                    )?;
                    Ok(Value::Null)
                }
                "view.dispose" => {
                    runtime.dispose_view(scope, string(&request, "view")?)?;
                    Ok(Value::Null)
                }
                "selection.set" => runtime.set_selection(
                    scope,
                    string(&request, "view")?,
                    request.get("selection").and_then(Value::as_str),
                    serde_json::from_value(
                        request.get("handles").cloned().unwrap_or_else(|| json!([])),
                    )?,
                ),
                "selection.dispose" => {
                    runtime.dispose_selection(
                        scope,
                        request
                            .get("id")
                            .or_else(|| request.get("selection"))
                            .and_then(Value::as_str)
                            .ok_or_else(|| Error::new("invalid_request", "Missing selection ID"))?,
                    )?;
                    Ok(Value::Null)
                }
                "task.run" => Ok(serde_json::to_value(runtime.run_task(
                    scope,
                    string(&request, "operation")?,
                    request.get("input").cloned().unwrap_or(Value::Null),
                    sink,
                )?)?),
                "task.batch" => Ok(serde_json::to_value(runtime.run_batch(
                    scope,
                    string(&request, "operation")?,
                    request.get("selection").cloned().ok_or_else(|| {
                        Error::new(
                            "invalid_selection",
                            "Batch requires a native selection or bounded inputs",
                        )
                    })?,
                    sink,
                )?)?),
                "task.snapshot" => Ok(serde_json::to_value(
                    runtime.task_snapshot(scope, string(&request, "task")?)?,
                )?),
                "task.cancel" => Ok(serde_json::to_value(
                    runtime.cancel_task(scope, string(&request, "task")?)?,
                )?),
                "task.dispose" => {
                    runtime.dispose_task(scope, string(&request, "task")?)?;
                    Ok(Value::Null)
                }
                "task.results" => {
                    let (offset, limit) = window(&request)?;
                    runtime.task_results(scope, string(&request, "task")?, offset, limit)
                }
                "settings.load" => runtime.inner.settings.load(
                    string(&request, "key")?,
                    request
                        .get("defaults")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                ),
                "settings.save" => {
                    runtime.inner.settings.save(
                        string(&request, "key")?,
                        request.get("value").cloned().unwrap_or(Value::Null),
                    )?;
                    Ok(Value::Null)
                }
                "media.preview" => {
                    let handle =
                        serde_json::from_value(request.get("handle").cloned().ok_or_else(
                            || Error::new("invalid_request", "Preview requires a file handle"),
                        )?)?;
                    let lease = runtime.lease_resource(scope, &handle)?;
                    let preview = executor.block_on(runtime.inner.previews.create(scope, lease))?;
                    let mut value = serde_json::to_value(preview)?;
                    value["preview"] = value["id"].take();
                    value.as_object_mut().unwrap().remove("id");
                    if let Some(token) = value.get("url").and_then(Value::as_str) {
                        let url = if cfg!(target_os = "windows") {
                            format!("http://revenant-preview.localhost/{token}")
                        } else {
                            format!("revenant-preview://localhost/{token}")
                        };
                        value["url"] = Value::String(url);
                    }
                    Ok(value)
                }
                "media.dispose" => {
                    runtime
                        .inner
                        .previews
                        .dispose(scope, string(&request, "preview")?)?;
                    Ok(Value::Null)
                }
                "runtime.inspect" => Ok(serde_json::to_value(runtime.inner.registry.inspect())?),
                "runtime.configure" => {
                    executor.block_on(runtime.inner.registry.configure(
                        string(&request, "provider")?,
                        request.get("config").cloned().unwrap_or_else(|| json!({})),
                    ))?;
                    Ok(Value::Null)
                }
                "runtime.replace" => {
                    let provider = string(&request, "provider")?;
                    let name = string(&request, "replacement")?;
                    let candidate = runtime.inner.replacements.get(name).ok_or_else(|| {
                        Error::new(
                            "replacement_missing",
                            "Replacement must be compiled into the application",
                        )
                    })?;
                    let config = runtime
                        .inner
                        .registry
                        .inspect()
                        .into_iter()
                        .find(|p| p.id == provider)
                        .ok_or_else(|| Error::new("provider_missing", "Unknown provider"))?
                        .config;
                    executor.block_on(runtime.inner.registry.replace(
                        provider,
                        candidate(),
                        config,
                    ))?;
                    Ok(Value::Null)
                }
                _ => Err(Error::new(
                    "unknown_action",
                    format!("Unknown native action {action}"),
                )),
            }
        })
        .await
    }

    /// Releases root scopes owned by a window after reload or destruction.
    pub fn dispose_window(&self, window: &str) -> Result<()> {
        let scopes: Vec<_> = lock(&self.inner.scopes)
            .iter()
            .filter(|(_, s)| s.window == window && s.parent.is_none())
            .map(|(id, _)| id.clone())
            .collect();
        for scope in scopes {
            self.dispose_scope(&scope)?;
        }
        Ok(())
    }
}
