//! Native capabilities owned by one Revenant desktop application.
//!
//! The runtime is independent of webview transport so filesystem, scheduling,
//! and shutdown guarantees can be exercised directly. The Tauri adapter projects
//! those guarantees into scoped UI objects; no network listener is involved.
#![deny(missing_docs, rustdoc::broken_intra_doc_links)]

mod agent_bridge;
mod file_port;
pub mod files;
mod native_store;
#[cfg(all(feature = "native-surfaces", windows))]
mod native_surfaces;
mod options;
mod previews;
pub mod results;
mod runtime;
mod settings;
#[cfg(feature = "tauri-host")]
mod tauri_host;
mod work;

pub use agent_bridge::mcp_if_requested;
pub use native_store::{NativeStoreIssue, NativeStoreRecovery};
pub use options::DesktopOptions;
pub use previews::PreviewResponse;
pub use revenant_core::{Error, Result};
pub use runtime::{AgentSession, DesktopRuntime, UpdateSink};
#[cfg(feature = "tauri-host")]
pub use tauri_host::launch;
pub use work::NativeWorkService;

/// Writes the compiled contract for the private CLI export request.
///
/// Generated bootstrap code calls this before creating a native window or
/// preparing providers. Application authors never implement export handling.
pub fn export_contract_if_requested(application: &revenant_sdk::Application) -> Result<bool> {
    if std::env::args().any(|arg| arg == "--revenant-contract") {
        println!("{}", serde_json::to_string(&application.manifest()?)?);
        return Ok(true);
    }
    Ok(false)
}
