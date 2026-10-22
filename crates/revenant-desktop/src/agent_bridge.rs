//! Author-selected application tools and a separate development observation surface.
use crate::{AgentSession, DesktopOptions, DesktopRuntime, Error, Result};
use revenant_agent::{
    AgentTool, Backend, BackendFuture, BridgeAccess, BridgeServer, InstanceInfo, InstanceSelector,
    RemoteBackend,
};
use revenant_sdk::{Application, OperationEffect};
use serde_json::{Value, json};
use std::sync::Arc;

const TASK_PREFIX: &str = "revenant.tasks.";
const REPLY_BYTES: usize = 512 * 1024;

struct ApplicationBackend {
    session: AgentSession,
    tools: Vec<AgentTool>,
}
impl ApplicationBackend {
    fn new(runtime: &DesktopRuntime) -> Result<Self> {
        let session = runtime.open_agent_session()?;
        let mut tools = Vec::new();
        for operation in session.list_tools()? {
            if operation.id.starts_with(TASK_PREFIX) {
                return Err(Error::new(
                    "reserved_tool_id",
                    "Application operations cannot use revenant.tasks IDs",
                ));
            }
            // Named argument records use the exact compiled schema. Legacy scalar
            // inputs are wrapped only for MCP's required object argument shape.
            let mut schema = operation.input.schema.clone();
            if operation.arguments.is_empty() && schema.get("type") != Some(&json!("object")) {
                let definitions = schema
                    .as_object_mut()
                    .and_then(|object| object.remove("$defs"));
                schema = json!({"type":"object","properties":{"input":schema},"required":["input"],"additionalProperties":false});
                if let Some(definitions) = definitions {
                    schema["$defs"] = definitions;
                }
            }
            tools.push(AgentTool {
                name: operation.id,
                description: operation.description,
                input_schema: schema,
                output_schema: Some(receipt_schema()),
                read_only: operation.effect == OperationEffect::ReadOnly,
            });
        }
        for (name, description, extra) in [
            (
                "get",
                "Inspect an owned task, including its terminal result or failure.",
                None,
            ),
            (
                "result",
                "Read a UTF-8 page of serialized JSON from an owned terminal result. Concatenate chunks and parse JSON. itemOffset selects one batch outcome.",
                Some(
                    json!({"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":4,"maximum":65536},"itemOffset":{"type":"integer","minimum":0}}),
                ),
            ),
            (
                "results",
                "Read one batch outcome. Large outcomes require result with itemOffset.",
                Some(
                    json!({"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":1}}),
                ),
            ),
            (
                "cancel",
                "Request cooperative cancellation of an owned task.",
                None,
            ),
            (
                "dispose",
                "Release an owned task and its retained result resources.",
                None,
            ),
        ] {
            let mut properties = json!({"task":{"type":"string","minLength":1,"maxLength":256}});
            if let Some(extra) = extra {
                properties
                    .as_object_mut()
                    .unwrap()
                    .extend(extra.as_object().unwrap().clone());
            }
            tools.push(AgentTool { name: format!("{TASK_PREFIX}{name}"), description: description.into(), input_schema: json!({"type":"object","properties":properties,"required":["task"],"additionalProperties":false}), output_schema: None, read_only: matches!(name, "get" | "result" | "results") });
        }
        Ok(Self { session, tools })
    }
}
fn receipt_schema() -> Value {
    json!({"type":"object","properties":{"task":{"type":"string"},"sessionId":{"type":"string"},"instanceId":{"type":"string"},"contractDigest":{"type":"string"},"state":{"type":"string"}},"required":["task","sessionId","instanceId","contractDigest","state"],"additionalProperties":false})
}
fn text<'a>(arguments: &'a Value, field: &str) -> Result<&'a str> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= 256)
        .ok_or_else(|| {
            Error::new(
                "invalid_input",
                format!("Missing bounded string field {field}"),
            )
        })
}
fn bounded_snapshot(snapshot: revenant_core::TaskSnapshot) -> Result<Value> {
    let mut value = serde_json::to_value(snapshot)?;
    if serde_json::to_vec(&value)?.len() > REPLY_BYTES
        && let Some(result) = value.as_object_mut().unwrap().remove("result")
    {
        value["resultOmitted"] = json!(true);
        value["resultBytes"] = json!(serde_json::to_vec(&result)?.len());
    }
    Ok(value)
}
fn result_chunk(value: Value, arguments: &Value) -> Result<Value> {
    let encoded = serde_json::to_string(&value)?;
    let offset = usize::try_from(arguments.get("offset").and_then(Value::as_u64).unwrap_or(0))
        .map_err(|_| Error::new("invalid_input", "Result offset exceeds this platform"))?;
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(65536) as usize;
    if offset > encoded.len() || !encoded.is_char_boundary(offset) {
        return Err(Error::new(
            "invalid_input",
            "Result offset must lie on a UTF-8 character boundary",
        ));
    }
    let mut end = offset.saturating_add(limit).min(encoded.len());
    while !encoded.is_char_boundary(end) {
        end -= 1;
    }
    Ok(
        json!({"encoding":"json","chunk":&encoded[offset..end],"offset":offset,"nextOffset":end,"totalBytes":encoded.len(),"complete":end==encoded.len()}),
    )
}
impl Backend for ApplicationBackend {
    fn tools(&self) -> Vec<AgentTool> {
        self.tools.clone()
    }
    fn call<'a>(&'a self, name: &'a str, arguments: Value) -> BackendFuture<'a, Value> {
        Box::pin(async move {
            let tool = self
                .tools
                .iter()
                .find(|tool| tool.name == name)
                .ok_or_else(|| Error::new("hidden_operation", "Operation is not author-exposed"))?;
            revenant_core::validate_json(&tool.input_schema, &arguments)?;
            if let Some(action) = name.strip_prefix(TASK_PREFIX) {
                let task = text(&arguments, "task")?;
                return match action {
                    "get" => bounded_snapshot(self.session.snapshot(task)?),
                    "cancel" => bounded_snapshot(self.session.cancel(task)?),
                    "dispose" => {
                        self.session.dispose(task)?;
                        Ok(json!({"disposed":true}))
                    }
                    "result" => {
                        let result = if let Some(item) =
                            arguments.get("itemOffset").and_then(Value::as_u64)
                        {
                            self.session.results(task, item, 1)?["items"]
                                .as_array()
                                .and_then(|items| items.first())
                                .cloned()
                                .ok_or_else(|| {
                                    Error::new(
                                        "result_missing",
                                        "No completed outcome at this offset",
                                    )
                                })?
                        } else {
                            self.session.snapshot(task)?.result.ok_or_else(|| {
                                Error::new("result_pending", "Task has no retained result yet")
                            })?
                        };
                        result_chunk(result, &arguments)
                    }
                    "results" => {
                        let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0);
                        let mut page = self.session.results(task, offset, 1)?;
                        let count = page["items"].as_array().map_or(0, Vec::len);
                        if serde_json::to_vec(&page)?.len() > REPLY_BYTES {
                            let ordinal = page["items"][0]["ordinal"].clone();
                            page["items"] = json!([{"ordinal":ordinal,"resultOmitted":true,"itemOffset":offset}]);
                        }
                        page["nextOffset"] = json!(offset.saturating_add(count as u64));
                        Ok(page)
                    }
                    _ => Err(Error::new("hidden_operation", "Unknown session tool")),
                };
            }
            let manifest = self.session.manifest()?;
            let operation = manifest
                .operations
                .iter()
                .find(|operation| operation.id == name)
                .ok_or_else(|| {
                    Error::new("unknown_operation", "Compiled operation is unavailable")
                })?;
            let arguments = if operation.arguments.is_empty()
                && operation.input.schema.get("type") != Some(&json!("object"))
            {
                arguments["input"].clone()
            } else {
                arguments
            };
            let task = self.session.run(name, arguments)?;
            let identity = self.session.identity();
            Ok(
                json!({"task":task.id,"state":task.state,"sessionId":identity["sessionId"],"instanceId":identity["instanceId"],"contractDigest":identity["contractDigest"]}),
            )
        })
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async { self.session.close().await })
    }
}

struct DevelopmentBackend(DesktopRuntime);
impl Backend for DevelopmentBackend {
    fn tools(&self) -> Vec<AgentTool> {
        vec![
            AgentTool {
                name: "runtime.inspect".into(),
                description:
                    "Inspect native execution and ownership without argument/result payloads."
                        .into(),
                input_schema: json!({"type":"object","additionalProperties":false}),
                output_schema: None,
                read_only: true,
            },
            AgentTool {
                name: "trace.query".into(),
                description:
                    "Read bounded correlated execution events; filters apply to the returned page."
                        .into(),
                input_schema: json!({"type":"object","properties":{"afterSequence":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":200},"operationId":{"type":"string"},"taskId":{"type":"string"}},"additionalProperties":false}),
                output_schema: None,
                read_only: true,
            },
        ]
    }
    fn call<'a>(&'a self, name: &'a str, arguments: Value) -> BackendFuture<'a, Value> {
        Box::pin(async move {
            let tools = self.tools();
            let definition = tools.iter().find(|tool| tool.name == name).ok_or_else(|| {
                Error::new(
                    "hidden_operation",
                    "Only development inspection is available",
                )
            })?;
            revenant_core::validate_json(&definition.input_schema, &arguments)?;
            match name {
                "runtime.inspect" => self.0.runtime_inspect(),
                "trace.query" => {
                    let mut page = self.0.trace_query(
                        arguments
                            .get("afterSequence")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        arguments
                            .get("limit")
                            .and_then(Value::as_u64)
                            .unwrap_or(100) as u32,
                    )?;
                    if let Some(events) = page["events"].as_array_mut() {
                        events.retain(|event| {
                            ["operationId", "taskId"].iter().all(|field| {
                                arguments
                                    .get(*field)
                                    .is_none_or(|value| value == &event[*field])
                            })
                        });
                    }
                    page["filteredPageOnly"] = json!(true);
                    Ok(page)
                }
                _ => Err(Error::new("hidden_operation", "Unknown inspection tool")),
            }
        })
    }
}

impl DesktopRuntime {
    /// Start one current-user bridge; application and development authority are separate.
    pub async fn start_agent_bridge(&self) -> Result<Option<BridgeServer>> {
        if !self.inner.options.agents_enabled && !self.inner.options.development {
            return Ok(None);
        }
        let identity = self.identity();
        let mut info = InstanceInfo::new(
            self.inner.options.app_id.clone(),
            identity["contractDigest"].as_str().unwrap_or_default(),
            self.inner.options.project_root.clone(),
            self.inner.options.development,
        );
        info.instance_id = identity["instanceId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let runtime = self.clone();
        Ok(Some(
            BridgeServer::start(
                info,
                Arc::new(move |access| match access {
                    BridgeAccess::Application => {
                        Ok(Arc::new(ApplicationBackend::new(&runtime)?) as Arc<dyn Backend>)
                    }
                    BridgeAccess::Development if runtime.inner.options.development => {
                        Ok(Arc::new(DevelopmentBackend(runtime.clone())) as Arc<dyn Backend>)
                    }
                    _ => Err(Error::new(
                        "development_disabled",
                        "Development access is disabled",
                    )),
                }),
            )
            .await?,
        ))
    }
}

/// Handle installed `--mcp` before window startup, attaching to an existing app.
/// `--instance <id>` is required when multiple matching app instances are running.
pub fn mcp_if_requested(application: &Application, options: &DesktopOptions) -> Result<bool> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !args.iter().any(|argument| argument == "--mcp") {
        return Ok(false);
    }
    let instance_id = args
        .iter()
        .position(|argument| argument == "--instance")
        .map(|index| {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| Error::new("invalid_input", "--instance requires an ID"))
        })
        .transpose()?;
    let digest = application.manifest()?.digest;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::new("mcp_runtime", error.to_string()))?;
    runtime.block_on(async {
        let backend = RemoteBackend::connect(InstanceSelector {
            app_id: Some(options.app_id.clone()),
            instance_id,
            ..Default::default()
        })
        .await?;
        if backend.info().contract_digest != digest {
            return Err(Error::new(
                "stale_contract",
                "Running application and this executable have different contracts",
            ));
        }
        revenant_agent::serve_stdio(Arc::new(backend))
            .await
            .map_err(|error| Error::new("mcp_transport", error.to_string()))
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use revenant_sdk::{operation, operations};

    #[operation(id = "fixture.read", read_only, agent)]
    fn read(value: String) -> String {
        value
    }
    #[operation(id = "fixture.write", effect = "write", agent)]
    fn write(value: String) -> String {
        format!("written:{value}")
    }
    #[operation(id = "fixture.hidden")]
    fn hidden() -> String {
        "hidden".into()
    }
    #[operation(id = "fixture.union", legacy_input, read_only, agent)]
    fn union(input: Option<String>) -> Option<String> {
        input
    }
    #[operation(id = "fixture.large", read_only, agent)]
    fn large() -> String {
        "é".repeat(524_287)
    }

    async fn runtime() -> (tempfile::TempDir, DesktopRuntime) {
        let directory = tempfile::tempdir().unwrap();
        let runtime = DesktopRuntime::new(
            Application::new().operations(operations![read, write, hidden, union, large]),
            DesktopOptions {
                app_id: format!("test.revenant.{}", uuid::Uuid::new_v4()),
                agents_enabled: true,
                data_dir: Some(directory.path().join("data")),
                cache_dir: Some(directory.path().join("cache")),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        (directory, runtime)
    }
    async fn result(backend: &ApplicationBackend, receipt: &Value) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let snapshot = backend
                    .call("revenant.tasks.get", json!({"task":receipt["task"]}))
                    .await
                    .unwrap();
                if snapshot["state"] == "succeeded" {
                    return snapshot["result"].clone();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn near_limit_native_results_are_retrievable_in_owned_utf8_pages() {
        let (_directory, runtime) = runtime().await;
        let backend = ApplicationBackend::new(&runtime).unwrap();
        let receipt = backend.call("fixture.large", json!({})).await.unwrap();
        let snapshot = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let snapshot = backend
                    .call("revenant.tasks.get", json!({"task":receipt["task"]}))
                    .await
                    .unwrap();
                if snapshot["state"] == "succeeded" {
                    break snapshot;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(snapshot["resultOmitted"], true);
        let mut result = String::new();
        let mut offset = 0;
        loop {
            let page = backend
                .call(
                    "revenant.tasks.result",
                    json!({"task":receipt["task"],"offset":offset,"limit":65_535}),
                )
                .await
                .unwrap();
            result.push_str(page["chunk"].as_str().unwrap());
            offset = page["nextOffset"].as_u64().unwrap();
            if page["complete"] == true {
                break;
            }
        }
        assert_eq!(serde_json::from_str::<Value>(&result).unwrap(), large());
        assert!(
            backend
                .call(
                    "revenant.tasks.result",
                    json!({"task":receipt["task"],"offset":2,"limit":10})
                )
                .await
                .is_err()
        );
        backend.close().await.unwrap();
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn exposed_reads_and_writes_return_receipts_and_session_tools_reject_foreign_tasks() {
        let (_directory, runtime) = runtime().await;
        let first = ApplicationBackend::new(&runtime).unwrap();
        let other = ApplicationBackend::new(&runtime).unwrap();
        assert!(
            !first
                .tools()
                .iter()
                .any(|tool| tool.name == "fixture.hidden")
        );
        assert_eq!(
            first
                .call("fixture.hidden", json!({}))
                .await
                .unwrap_err()
                .code,
            "hidden_operation"
        );
        for (operation, expected) in [
            ("fixture.read", "hello"),
            ("fixture.write", "written:hello"),
        ] {
            let receipt = first
                .call(operation, json!({"value":"hello"}))
                .await
                .unwrap();
            assert_eq!(
                receipt["contractDigest"],
                runtime.identity()["contractDigest"]
            );
            assert_eq!(result(&first, &receipt).await, expected);
            assert!(
                other
                    .call("revenant.tasks.get", json!({"task":receipt["task"]}))
                    .await
                    .is_err()
            );
            first
                .call("revenant.tasks.dispose", json!({"task":receipt["task"]}))
                .await
                .unwrap();
        }
        let receipt = first
            .call("fixture.union", json!({"input":null}))
            .await
            .unwrap();
        assert!(result(&first, &receipt).await.is_null());
        first.close().await.unwrap();
        other.close().await.unwrap();
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn live_bridge_identity_matches_native_runtime_and_disconnect_reclaims_session() {
        let (_directory, runtime) = runtime().await;
        let server = runtime.start_agent_bridge().await.unwrap().unwrap();
        let remote = RemoteBackend::connect(InstanceSelector {
            instance_id: Some(runtime.identity()["instanceId"].as_str().unwrap().into()),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(
            remote.info().instance_id,
            runtime.identity()["instanceId"].as_str().unwrap()
        );
        let receipt = remote
            .call("fixture.write", json!({"value":"hello"}))
            .await
            .unwrap();
        assert_eq!(receipt["instanceId"], runtime.identity()["instanceId"]);
        remote.close().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while runtime.runtime_inspect().unwrap()["ownership"]["scopes"] != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        server.stop().await.unwrap();
        runtime.shutdown().await.unwrap();
    }
}
