//! Official MCP transport and a current-user local bridge to an existing app.
pub mod bridge;
mod mcp;
pub use bridge::{
    BridgeAccess, BridgeServer, InstanceInfo, InstanceSelector, RemoteBackend, discover_instances,
};
pub use mcp::serve_stdio;

use revenant_core::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin};

/// An operation advertised by one authoritative backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTool {
    /// Stable tool identifier resolved by the native backend.
    pub name: String,
    /// Rust or framework documentation presented to the client.
    pub description: String,
    /// Authoritative JSON Schema for object-shaped tool arguments.
    pub input_schema: Value,
    /// Optional schema for the structured result, usually a task receipt.
    pub output_schema: Option<Value>,
    /// Descriptive effect hint; never an authorization grant.
    pub read_only: bool,
}

/// Object-safe asynchronous backend call.
pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Tool authority stays in the native backend; MCP annotations are descriptive.
pub trait Backend: Send + Sync + 'static {
    /// Return the catalogue available to this particular connection.
    fn tools(&self) -> Vec<AgentTool>;
    /// Validate authority and invoke one advertised operation.
    fn call<'a>(&'a self, name: &'a str, arguments: Value) -> BackendFuture<'a, Value>;
    /// Release connection ownership and request cancellation of remaining work.
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
