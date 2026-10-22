//! A shared, bounded application view for humans, CLI checks and developer MCP.
//! Compiled metadata establishes Rust/contract edges; language services establish
//! references. Missing tools or fingerprints never become a guessed current state.
pub mod backend;
mod checks;

use crate::{
    config::{self, RevenantConfig},
    toolchain::process::{self, ManagedProcess},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub use checks::parse_cargo_diagnostics;

const FILE_LIMIT: usize = 4096;
const ENTRY_LIMIT: usize = 8192;
const FILE_BYTES: u64 = 4 * 1024 * 1024;
const SOURCE_BYTES: u64 = 64 * 1024 * 1024;
pub const RESULT_BYTES: usize = 1024 * 1024;
pub const MAX_RESULTS: usize = 200;

/// One diagnostic shape shared across compiler, frontend and framework stages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub stage: String,
    pub code: Option<String>,
    pub severity: String,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub end_line: Option<u64>,
    pub end_column: Option<u64>,
}
impl Diagnostic {
    pub fn error(stage: &str, code: &str, message: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            code: Some(code.into()),
            severity: "error".into(),
            message: message.into(),
            file: None,
            line: None,
            column: None,
            end_line: None,
            end_column: None,
        }
    }
}

/// A deterministic hash of authored sources and the application's SDK snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFingerprint {
    pub digest: String,
    pub complete: bool,
    pub files: usize,
    pub bytes: u64,
    pub omitted: Vec<String>,
    pub visited_entries: usize,
}
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
fn excluded(name: &str) -> bool {
    matches!(
        name,
        "node_modules"
            | "target"
            | ".git"
            | ".revenant"
            | ".svelte-kit"
            | "build"
            | "dist"
            | ".env"
    )
}
fn source_extension(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some("Cargo.lock")
        || matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some(
                "rs" | "ts"
                    | "tsx"
                    | "js"
                    | "jsx"
                    | "mjs"
                    | "cjs"
                    | "cts"
                    | "svelte"
                    | "toml"
                    | "json"
                    | "css"
                    | "html"
                    | "svg"
                    | "sql"
                    | "png"
                    | "ico"
                    | "icns"
                    | "wgsl"
                    | "ttf"
                    | "otf"
                    | "woff"
                    | "woff2"
            )
        )
}
pub fn source_fingerprint(root: &Path) -> Result<SourceFingerprint> {
    let root = dunce::canonicalize(root)?;
    let mut files = Vec::new();
    let mut omitted = Vec::new();
    let mut bytes = 0;
    let mut visited_entries = 0;
    fn omit(root: &Path, path: &Path, omitted: &mut Vec<String>) {
        if omitted.len() < 100 {
            omitted.push(relative(root, path));
        }
    }
    fn visit(
        root: &Path,
        path: &Path,
        depth: usize,
        files: &mut Vec<PathBuf>,
        omitted: &mut Vec<String>,
        bytes: &mut u64,
        visited: &mut usize,
    ) -> Result<()> {
        if files.len() >= FILE_LIMIT || *bytes >= SOURCE_BYTES {
            omit(root, path, omitted);
            return Ok(());
        }
        if !path.try_exists()? {
            return Ok(());
        }
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !dunce::canonicalize(path)?.starts_with(root) {
            omit(root, path, omitted);
            return Ok(());
        }
        if depth > 32 {
            omit(root, path, omitted);
            return Ok(());
        }
        if metadata.is_dir() {
            if *visited >= ENTRY_LIMIT {
                omit(root, path, omitted);
                return Ok(());
            }
            let remaining = ENTRY_LIMIT.saturating_sub(*visited);
            let mut reader = fs::read_dir(path)?;
            let mut entries = reader
                .by_ref()
                .take(remaining)
                .collect::<std::io::Result<Vec<_>>>()?;
            *visited += entries.len();
            if reader.next().is_some() {
                omit(root, path, omitted);
            }
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if excluded(&name) || entry.path() == root.join("web/src/lib/revenant.ts") {
                    continue;
                }
                if files.len() >= FILE_LIMIT || *bytes >= SOURCE_BYTES {
                    omit(root, path, omitted);
                    break;
                }
                visit(
                    root,
                    &entry.path(),
                    depth + 1,
                    files,
                    omitted,
                    bytes,
                    visited,
                )?;
            }
        } else if metadata.is_file() && source_extension(path) {
            if metadata.len() > FILE_BYTES || *bytes + metadata.len() > SOURCE_BYTES {
                omit(root, path, omitted);
                return Ok(());
            }
            *bytes += metadata.len();
            files.push(path.to_owned());
        }
        Ok(())
    }
    for directory in [
        "revenant.toml",
        "native",
        "web",
        ".revenant/sdk/Cargo.toml",
        ".revenant/sdk/Cargo.lock",
        ".revenant/desktop/Cargo.lock",
        ".revenant/sdk/compatibility.json",
        ".revenant/sdk/crates",
        ".revenant/sdk/packages/client",
    ] {
        visit(
            &root,
            &root.join(directory),
            0,
            &mut files,
            &mut omitted,
            &mut bytes,
            &mut visited_entries,
        )?;
    }
    // Authored icons may live outside native/ and web/, but still affect the
    // executable. Use the same bounded traversal and containment checks.
    if let Ok(config) = fs::read_to_string(root.join("revenant.toml"))
        && let Ok(config) = toml::from_str::<toml::Value>(&config)
        && let Some(icons) = config
            .get("desktop")
            .and_then(|v| v.get("icons"))
            .and_then(toml::Value::as_array)
    {
        for icon in icons {
            let Some(icon) = icon.as_str() else { continue };
            let path = root.join(icon);
            if !path.try_exists()? {
                omit(&root, &path, &mut omitted);
            } else if !files.contains(&path) {
                visit(
                    &root,
                    &path,
                    0,
                    &mut files,
                    &mut omitted,
                    &mut bytes,
                    &mut visited_entries,
                )?;
            }
        }
    }
    files.sort();
    files.dedup();
    let mut hash = Sha256::new();
    for path in &files {
        let name = relative(&root, path);
        let content = fs::read(path)?;
        // Length prefixes avoid ambiguous path/content concatenations.
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((content.len() as u64).to_le_bytes());
        hash.update(content);
    }
    let complete = omitted.is_empty();
    Ok(SourceFingerprint {
        digest: format!("sha256:{:x}", hash.finalize()),
        complete,
        files: files.len(),
        bytes,
        omitted,
        visited_entries,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildAttempt {
    id: String,
    stage: String,
    status: String,
    started_at: u128,
    finished_at: Option<u128>,
    source_fingerprint: SourceFingerprint,
    completed_source_fingerprint: Option<SourceFingerprint>,
    source_changed_during_build: bool,
    diagnostics: Vec<Diagnostic>,
}
fn now() -> Result<u128> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
}
pub(crate) fn inspection_directory(root: &Path) -> Result<PathBuf> {
    let root = dunce::canonicalize(root)?;
    let path = root.join(".revenant/inspection");
    // Check each existing ancestor before creating a directory through it.
    for directory in [root.join(".revenant"), path.clone()] {
        if directory.try_exists()? {
            ensure!(
                dunce::canonicalize(directory)?.starts_with(&root),
                "inspection directory escapes project root"
            );
        }
    }
    fs::create_dir_all(&path)?;
    Ok(path)
}
/// Serialize native host preparation/publication and fixed checks across CLI
/// processes. Dropping the returned file releases the operating-system lock.
pub(crate) fn acquire_project_lock(root: &Path) -> Result<fs::File> {
    let root = dunce::canonicalize(root)?;
    let path = inspection_directory(&root)?.join("project.lock");
    if path.try_exists()? {
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink()
                && dunce::canonicalize(&path)?.starts_with(&root),
            "project lock escapes project root"
        );
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    lock.try_lock().map_err(|error| {
        anyhow::anyhow!(
            "Project native build or check already running, or its lock is unavailable: {error}"
        )
    })?;
    Ok(lock)
}

fn read_json(root: &Path, path: &Path) -> Result<Value> {
    let file = dunce::canonicalize(path)?;
    ensure!(
        file.starts_with(root),
        "inspection artifact escapes project root"
    );
    ensure!(
        fs::metadata(&file)?.len() <= 4 * 1024 * 1024,
        "inspection artifact exceeds size budget"
    );
    Ok(serde_json::from_slice(&fs::read(file)?)?)
}
/// Capture the exact source identity before starting compilation or fixed checks.
pub fn record_build_start(root: &Path, stage: &str) -> Result<()> {
    let attempt = start_attempt(root, stage)?;
    crate::compiled::atomic_write(
        &inspection_directory(root)?.join("build.json"),
        &serde_json::to_vec(&attempt)?,
    )
}
/// Finish one build without replacing the previously published contract facade.
pub fn record_build_finish(
    root: &Path,
    succeeded: bool,
    mut diagnostics: Vec<Diagnostic>,
) -> Result<()> {
    let root = dunce::canonicalize(root)?;
    let directory = inspection_directory(&root)?;
    let attempt: BuildAttempt =
        serde_json::from_value(read_json(&root, &directory.join("build.json"))?)?;
    let attempt = finish_attempt(&root, attempt, succeeded, std::mem::take(&mut diagnostics))?;
    crate::compiled::atomic_write(
        &directory.join("build.json"),
        &serde_json::to_vec(&attempt)?,
    )
}
fn start_attempt(root: &Path, stage: &str) -> Result<BuildAttempt> {
    let started = now()?;
    Ok(BuildAttempt {
        id: format!("{}-{started}", std::process::id()),
        stage: stage.into(),
        status: "running".into(),
        started_at: started,
        finished_at: None,
        source_fingerprint: source_fingerprint(root)?,
        completed_source_fingerprint: None,
        source_changed_during_build: false,
        diagnostics: vec![],
    })
}
fn finish_attempt(
    root: &Path,
    mut attempt: BuildAttempt,
    succeeded: bool,
    mut diagnostics: Vec<Diagnostic>,
) -> Result<BuildAttempt> {
    let completed = source_fingerprint(root)?;
    attempt.source_changed_during_build = attempt.source_fingerprint.digest != completed.digest;
    attempt.completed_source_fingerprint = Some(completed);
    attempt.finished_at = Some(now()?);
    attempt.status = if succeeded { "succeeded" } else { "failed" }.into();
    diagnostics.truncate(MAX_RESULTS);
    attempt.diagnostics = diagnostics;
    Ok(attempt)
}
/// Complete source identity captured before the latest attempt, for compiled
/// publication. Partial indexing never establishes a current facade or runtime.
pub fn latest_build_source_fingerprint(root: &Path) -> Result<Option<String>> {
    let root = dunce::canonicalize(root)?;
    let path = root.join(".revenant/inspection/build.json");
    if !path.try_exists()? {
        return Ok(None);
    }
    let value = read_json(&root, &path)?;
    if value["sourceFingerprint"]["complete"] != true {
        return Ok(None);
    }
    Ok(value["sourceFingerprint"]["digest"]
        .as_str()
        .map(String::from))
}

/// Attach authenticated runtime evidence and compare it with current source and
/// the published facade. Discovery records alone do not establish a live build.
pub fn attach_runtime(snapshot: &mut Value, state: Value) {
    let source = snapshot["sourceFingerprint"]["digest"].as_str();
    let live_source = state["sourceFingerprint"]
        .as_str()
        .or_else(|| state["sourceFingerprint"]["digest"].as_str());
    let source_to_runtime = if snapshot["sourceFingerprint"]["complete"] != true {
        "unknown"
    } else {
        match (source, live_source) {
            (Some(source), Some(runtime)) if source == runtime => "current",
            (Some(_), Some(_)) => "stale",
            _ => "unknown",
        }
    };
    let facade_to_runtime = match (
        snapshot["facade"]["digest"].as_str(),
        state["contractDigest"].as_str(),
    ) {
        (Some(facade), Some(runtime)) if facade == runtime => "current",
        (Some(_), Some(_)) => "stale",
        _ => "unknown",
    };
    snapshot["consistency"]["sourceToRuntime"] = json!(source_to_runtime);
    snapshot["consistency"]["selectedFacadeToRuntime"] = json!(facade_to_runtime);
    snapshot["runtime"] = json!({"status":"connected","state":state});
}

/// Connect an authenticated, bounded trace page to its operation graph. Native
/// observations retain their own instance and contract identity when stale.
pub fn attach_runtime_traces(query: &mut Value, page: Value) {
    let matches = page["instance"]["contractDigest"] == query["contractDigest"]
        && !query["contractDigest"].is_null();
    let operation = query["operation"]["id"].as_str().unwrap_or("").to_owned();
    for event in page["events"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_RESULTS)
    {
        if event["operationId"] != operation {
            continue;
        }
        let id = format!(
            "execution:{}:{}",
            page["instance"]["instanceId"].as_str().unwrap_or("unknown"),
            event["sequence"]
        );
        if let Some(nodes) = query["graph"]["nodes"].as_array_mut() {
            nodes.push(json!({"id":id,"kind":"native_execution","event":event,"instance":page["instance"],"contractMatchesManifest":matches,"provenance":"native_runtime"}));
        }
        if let Some(edges) = query["graph"]["edges"].as_array_mut() {
            edges.push(json!({"from":format!("operation:{operation}"),"to":id,"relation":"observed","contractMatchesManifest":matches,"provenance":"native_runtime"}));
        }
    }
    query["runtimeTraces"] =
        json!({"status":"connected","page":page,"contractMatchesManifest":matches});
}

/// Payload-free failures derived from authenticated native state. Frontend
/// reports remain labeled as observations, rather than native authority.
pub fn runtime_diagnostics(state: &Value) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    for task in state["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_RESULTS)
    {
        if let Some(code) = task["errorCode"].as_str() {
            diagnostics.push(json!({"stage":"runtime","code":code,"severity":"error","message":format!("Native task failed: {code}"),"taskId":task["id"],"operationId":task["operationId"],"requestId":task["requestId"],"instanceId":state["instanceId"],"provenance":"native_runtime"}));
        }
    }
    for failure in state["cleanupFailures"]
        .as_array()
        .into_iter()
        .flatten()
        .take(32)
    {
        diagnostics.push(json!({"stage":"runtime","code":failure["code"],"severity":"error","message":failure["message"].as_str().unwrap_or("Native cleanup failed").chars().take(4000).collect::<String>(),"instanceId":state["instanceId"],"provenance":"native_runtime"}));
    }
    for report in state["frontendReports"]
        .as_array()
        .into_iter()
        .flatten()
        .take(32)
    {
        if report["kind"] == "error" {
            diagnostics.push(json!({"stage":"frontend","code":"frontend_error","severity":"error","message":report["message"].as_str().unwrap_or("Frontend error report").chars().take(4000).collect::<String>(),"instanceId":state["instanceId"],"provenance":"frontend_report"}));
        }
    }
    diagnostics.truncate(MAX_RESULTS);
    diagnostics
}

/// Project-scoped access; all externally supplied paths remain below this root.
#[derive(Debug, Clone)]
pub struct ProjectInspector {
    root: PathBuf,
    cancellation: Cancellation,
}

#[derive(Debug, Clone, Default)]
struct Cancellation(
    Option<(
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    )>,
);
impl Cancellation {
    fn checkpoint(&self) -> Result<()> {
        if self.0.as_ref().is_some_and(|(request, session)| {
            request.load(std::sync::atomic::Ordering::Acquire)
                || session.load(std::sync::atomic::Ordering::Acquire)
        }) {
            return Err(revenant_core::Error::new(
                "request_cancelled",
                "Developer inspection was cancelled",
            )
            .into());
        }
        Ok(())
    }
}

impl ProjectInspector {
    #[cfg(test)]
    fn with_cancellation(
        mut self,
        request: std::sync::Arc<std::sync::atomic::AtomicBool>,
        session: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.cancellation = Cancellation(Some((request, session)));
        self
    }
    pub fn open(start: &Path) -> Result<Self> {
        let root = config::find_project_root(start)
            .context("No revenant.toml found; pass a Revenant application project")?;
        let root = dunce::canonicalize(root)?;
        RevenantConfig::load(&root)?;
        Ok(Self {
            root,
            cancellation: Cancellation::default(),
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn artifact(&self) -> Result<Option<(PathBuf, Value, Value)>> {
        let pointer = self.root.join("web/src/lib/revenant.ts");
        if !pointer.try_exists()? {
            return Ok(None);
        }
        ensure!(
            dunce::canonicalize(&pointer)?.starts_with(&self.root),
            "facade pointer escapes project root"
        );
        ensure!(
            fs::metadata(&pointer)?.len() < 65536,
            "generated facade pointer exceeds size budget"
        );
        let entry = fs::read_to_string(pointer)?;
        let Some((_, suffix)) = entry.split_once("./.revenant/") else {
            anyhow::bail!("generated facade pointer does not reference an immutable generation");
        };
        let generation = suffix.split('/').next().context("missing generation")?;
        ensure!(
            !generation.is_empty()
                && generation.len() <= 128
                && generation
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')),
            "invalid facade generation"
        );
        let directory = self.root.join("web/src/lib/.revenant").join(generation);
        let manifest = read_json(&self.root, &directory.join("revenant.contract.json"))?;
        let metadata = if directory.join("generation.json").try_exists()? {
            read_json(&self.root, &directory.join("generation.json"))?
        } else {
            json!({})
        };
        Ok(Some((directory, manifest, metadata)))
    }
    pub(crate) fn build_attempt(&self) -> Result<Value> {
        let path = self.root.join(".revenant/inspection/build.json");
        if path.try_exists()? {
            read_json(&self.root, &path)
        } else {
            Ok(Value::Null)
        }
    }
    pub fn diagnostics(&self) -> Result<Value> {
        let mut diagnostics = Vec::new();
        for attempt in [self.build_attempt()?, self.check_attempt()?] {
            diagnostics.extend(
                attempt["diagnostics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        diagnostics.truncate(MAX_RESULTS);
        Ok(json!(diagnostics))
    }
    fn check_attempt(&self) -> Result<Value> {
        let path = self.root.join(".revenant/inspection/check.json");
        if path.try_exists()? {
            read_json(&self.root, &path)
        } else {
            Ok(Value::Null)
        }
    }
    pub fn snapshot(&self) -> Result<Value> {
        self.cancellation.checkpoint()?;
        let config = RevenantConfig::load(&self.root)?;
        let fingerprint = source_fingerprint(&self.root)?;
        let attempt = self.build_attempt()?;
        let check_attempt = self.check_attempt()?;
        let mut diagnostics = self.diagnostics()?.as_array().cloned().unwrap_or_default();
        let artifact = match self.artifact() {
            Ok(value) => value,
            Err(error) => {
                diagnostics.push(serde_json::to_value(Diagnostic::error(
                    "contract",
                    "invalid_generated_contract",
                    error.to_string(),
                ))?);
                None
            }
        };
        let compatibility = self.root.join(".revenant/sdk/compatibility.json");
        let sdk = if compatibility.try_exists()? {
            read_json(&self.root, &compatibility).unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let facade=artifact.as_ref().map(|(directory,manifest,metadata)|json!({"directory":relative(&self.root,directory),"version":manifest["version"],"protocol":manifest["protocol"],"digest":manifest["digest"],"sourceFingerprint":metadata["sourceFingerprint"],"operationCount":manifest["operations"].as_array().map(Vec::len)})).unwrap_or(Value::Null);
        let consistency = if !fingerprint.complete {
            "unknown"
        } else {
            match facade["sourceFingerprint"].as_str() {
                Some(value) if value == fingerprint.digest => "current",
                Some(_) => "stale",
                None if artifact.is_none() => "missing",
                None => "unknown",
            }
        };
        let operation_total = artifact
            .as_ref()
            .and_then(|(_, manifest, _)| manifest["operations"].as_array())
            .map_or(0, Vec::len);
        let operations=artifact.as_ref().and_then(|(_,manifest,_)|manifest["operations"].as_array()).map(|operations|operations.iter().take(MAX_RESULTS).map(|op|json!({"id":op["id"],"description":op["description"],"arguments":op["arguments"],"effect":op["effect"],"independent":op["independent"],"agent":op["agent"],"source":self.operation_source(op)})).collect::<Vec<_>>()).unwrap_or_default();
        let instances = crate::inspection::backend::instances_for(&self.root);
        let live_consistency=instances.as_array().map(|instances|instances.iter().map(|instance|json!({"instanceId":instance["instanceId"],"facadeToRuntime":if facade["digest"].is_null(){"unknown"}else if instance["contractDigest"]==facade["digest"]{"current"}else{"stale"}})).collect::<Vec<_>>()).unwrap_or_default();
        Ok(
            json!({"project":{"root":self.root,"name":config.project.name,"identifier":config.identifier(),"layoutVersion":config.version},"expected":{"sdkRevision":revenant_core::SDK_COMPATIBILITY_REVISION,"manifestVersion":revenant_core::CONTRACT_MANIFEST_VERSION,"bridgeProtocol":revenant_core::BRIDGE_PROTOCOL_VERSION},"sdk":sdk,"sourceFingerprint":fingerprint,"buildAttempt":attempt,"checkAttempt":check_attempt,"facade":facade,"discoveredInstances":instances,"consistency":{"sourceToFacade":consistency,"discoveredFacadeComparisons":live_consistency},"diagnostics":diagnostics,"operations":operations,"catalogue":{"total":operation_total,"returned":operations.len(),"truncated":operation_total>operations.len()},"graph":{"provenance":"compiled_manifest","index":"not_requested","operationCount":operation_total}}),
        )
    }
    fn operation_source(&self, operation: &Value) -> Value {
        let source = &operation["source"];
        let Some(file) = source["file"].as_str() else {
            return Value::Null;
        };
        let module = source["module"].as_str().unwrap_or("");
        let file = file.replace('\\', "/");
        let path = if Path::new(&file).is_absolute() {
            PathBuf::from(&file)
        } else if file.starts_with("native/") || file.starts_with(".revenant/") {
            self.root.join(&file)
        } else if file.starts_with("crates/") {
            self.root.join(".revenant/sdk").join(&file)
        } else if module.starts_with("revenant_sdk") {
            self.root
                .join(".revenant/sdk/crates/revenant-sdk")
                .join(&file)
        } else {
            self.root.join("native").join(&file)
        };
        let canonical = dunce::canonicalize(&path).ok();
        let in_project = canonical
            .as_ref()
            .is_some_and(|path| path.starts_with(&self.root));
        json!({"file":if in_project {relative(&self.root,canonical.as_ref().unwrap())} else {relative(&self.root,&path)},"line":source["line"],"column":source["column"],"module":source["module"],"exists":in_project,"provenance":"compiled_manifest"})
    }
    pub fn explain(&self, id: &str) -> Result<Value> {
        self.cancellation.checkpoint()?;
        let (directory, manifest, metadata) = self
            .artifact()?
            .context("No compiled contract; run revenant dev or revenant build first")?;
        let operation = manifest["operations"]
            .as_array()
            .context("compiled operation list missing")?
            .iter()
            .find(|op| op["id"] == id)
            .with_context(|| format!("unknown compiled operation: {id}"))?;
        Ok(
            json!({"operation":operation,"source":self.operation_source(operation),"binding":{"file":relative(&self.root,&directory.join("facade.ts")),"provenance":"compiled_manifest"},"contractDigest":manifest["digest"],"sourceFingerprint":metadata["sourceFingerprint"],"index":{"status":"not_requested","hint":"code.query with operation establishes semantic frontend consumers"}}),
        )
    }
    pub fn query(&self, arguments: Value) -> Result<Value> {
        self.cancellation.checkpoint()?;
        let arguments = arguments
            .as_object()
            .context("code.query requires an argument object")?;
        ensure!(
            arguments.keys().all(|key| matches!(
                key.as_str(),
                "operation" | "file" | "line" | "column" | "symbol" | "maxResults"
            )),
            "unknown code.query argument"
        );
        let limit = arguments
            .get("maxResults")
            .map(|value| value.as_u64().context("maxResults must be an integer"))
            .transpose()?
            .unwrap_or(100);
        ensure!(
            (1..=MAX_RESULTS as u64).contains(&limit),
            "maxResults must be between 1 and 200"
        );
        let mut request = json!({"project":self.root,"maxResults":limit,"timeoutMs":20000});
        if let Some(id) = arguments.get("operation").and_then(Value::as_str) {
            let explanation = self.explain(id)?;
            request["action"] = json!("operation");
            request["operation"] = json!(id);
            request["facade"] = explanation["binding"]["file"].clone();
            let mut index = self.semantic(request)?;
            index["operation"] = explanation["operation"].clone();
            index["rustDefinition"] = explanation["source"].clone();
            index["contractDigest"] = explanation["contractDigest"].clone();
            let operation_id = format!("operation:{id}");
            let rust_id = format!("rust:{id}");
            let mut nodes = vec![
                json!({"id":rust_id,"kind":"rust_definition","location":explanation["source"],"provenance":"compiled_manifest"}),
                json!({"id":operation_id,"kind":"operation","operationId":id,"contractDigest":explanation["contractDigest"],"provenance":"compiled_manifest"}),
            ];
            let mut edges = vec![
                json!({"from":rust_id,"to":operation_id,"relation":"declares","provenance":"compiled_manifest"}),
            ];
            let bindings = index["bindings"]
                .as_array()
                .filter(|items| !items.is_empty())
                .cloned()
                .unwrap_or_else(|| {
                    vec![
                        index
                            .get("binding")
                            .cloned()
                            .unwrap_or_else(|| explanation["binding"].clone()),
                    ]
                });
            let binding_nodes = bindings.iter().enumerate().map(|(ordinal, binding)| {
                let binding_id = if ordinal == 0 { format!("typescript:{id}") } else { format!("typescript:{id}:{ordinal}") };
                nodes.push(json!({"id":binding_id,"kind":"typescript_binding","location":binding,"provenance":"compiled_manifest"}));
                edges.push(json!({"from":operation_id,"to":binding_id,"relation":"generates","provenance":"compiled_manifest"}));
                binding_id
            }).collect::<Vec<_>>();
            for (ordinal, reference) in index["references"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let consumer_id = format!("consumer:{id}:{ordinal}");
                nodes.push(json!({"id":consumer_id,"kind":"frontend_consumer","location":reference,"provenance":reference["provenance"]}));
                let matched = bindings
                    .iter()
                    .position(|binding| {
                        ["file", "line", "column"]
                            .iter()
                            .all(|field| binding[field] == reference["binding"][field])
                    })
                    .or_else(|| (bindings.len() == 1).then_some(0));
                if let Some(matched) = matched {
                    edges.push(json!({"from":binding_nodes[matched],"to":consumer_id,"relation":"used_by","provenance":reference["provenance"]}));
                }
            }
            index["graph"] = json!({"nodes":nodes,"edges":edges,"indexStatus":index["status"],"unresolved":index["unresolved"]});
            let snapshot = self.snapshot()?;
            index["consistency"] = snapshot["consistency"].clone();
            index["sourceFingerprint"] = snapshot["sourceFingerprint"].clone();
            return Ok(index);
        }
        let file = arguments
            .get("file")
            .and_then(Value::as_str)
            .context("code.query needs operation, or file and one-based line/column")?;
        let path = dunce::canonicalize(self.root.join(file))
            .context("query source file does not exist")?;
        ensure!(
            path.starts_with(&self.root),
            "query source escapes project root"
        );
        ensure!(
            !path
                .components()
                .any(|part| part.as_os_str() == "node_modules"
                    || part.as_os_str() == ".git"
                    || part.as_os_str() == "target"),
            "dependency and build artifact queries are excluded"
        );
        ensure!(
            fs::metadata(&path)?.len() <= FILE_BYTES,
            "query source exceeds size budget"
        );
        let line = arguments.get("line").and_then(Value::as_u64).unwrap_or(1);
        let column = arguments.get("column").and_then(Value::as_u64).unwrap_or(1);
        ensure!(
            line > 0 && column > 0,
            "line and column are one-based positive integers"
        );
        request["action"] = json!("query");
        request["file"] = json!(relative(&self.root, &path));
        request["line"] = json!(line);
        request["column"] = json!(column);
        if let Some(symbol) = arguments.get("symbol") {
            request["symbol"] = symbol.clone();
        }
        self.semantic(request)
    }
    pub(crate) fn semantic(&self, mut request: Value) -> Result<Value> {
        self.cancellation.checkpoint()?;
        request["project"] = json!(self.root);
        // Checkout dependencies are a development fallback. Distributed CLIs use
        // the application-local packages; this path is never supplied by an agent.
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/client");
        if checkout.join("node_modules/typescript").is_dir() {
            request["dependencyRoot"] = json!(checkout);
        }
        let encoded = serde_json::to_string(&request)?;
        let script = include_str!("../../scripts/inspection-semantic.mjs");
        match cancellable_process(
            &self.root,
            "node",
            &[
                "--input-type=module",
                "--eval",
                script,
                "--",
                "--request-env",
            ],
            &[("REVENANT_INSPECT_REQUEST", &encoded)],
            Duration::from_secs(55),
            &self.cancellation,
        ) {
            Ok((_, output, _)) => serde_json::from_str(&output)
                .context("language inspection must emit one JSON result"),
            Err(error)
                if process_cleanup_failure(&error) || self.cancellation.checkpoint().is_err() =>
            {
                Err(error)
            }
            Err(error) => Ok(
                json!({"status":"unavailable","services":[],"issues":[{"service":"node","message":error.to_string()}],"references":[],"definitions":[],"diagnostics":[],"truncated":false}),
            ),
        }
    }
    pub fn check(&self, profile: &str) -> Result<Value> {
        self.cancellation.checkpoint()?;
        checks::run(self, profile)
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    #[test]
    fn cancellation_stops_a_ready_inspection_process() {
        let root = tempfile::tempdir().unwrap();
        let ready = root.path().join("ready");
        let request = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancellation = Cancellation(Some((request.clone(), request.clone())));
        let observer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !ready.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            let started = ready.exists();
            request.store(true, std::sync::atomic::Ordering::Release);
            started
        });
        let result = cancellable_process(
            root.path(),
            "node",
            &[
                "--eval",
                "require('node:fs').writeFileSync('ready', String(process.pid)); setInterval(() => {}, 1000);",
            ],
            &[],
            Duration::from_secs(2),
            &cancellation,
        );
        assert!(
            observer.join().unwrap(),
            "child must acknowledge readiness before cancellation"
        );
        let error = result.unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<revenant_core::Error>()
                .map(|e| e.code.as_str()),
            Some("request_cancelled"),
            "cancellation must not degrade to timeout: {error:#}"
        );
    }

    #[test]
    fn cancelled_frontend_check_finishes_evidence_and_releases_locks() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("revenant.toml"),
            "version = 3\n[project]\nname = 'cancel-check'\n",
        )
        .unwrap();
        let kit = root.path().join("web/node_modules/@sveltejs/kit");
        fs::create_dir_all(&kit).unwrap();
        fs::write(
            kit.join("package.json"),
            r#"{"bin":{"svelte-kit":"sync.cjs"}}"#,
        )
        .unwrap();
        fs::write(kit.join("sync.cjs"), "require('node:fs').writeFileSync('ready', 'yes'); setTimeout(() => process.exit(0), 1000);").unwrap();
        let ready = root.path().join("web/ready");
        let request = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let inspector = ProjectInspector::open(root.path())
            .unwrap()
            .with_cancellation(request.clone(), request.clone());
        let observer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !ready.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            let started = ready.exists();
            request.store(true, std::sync::atomic::Ordering::Release);
            started
        });
        let result = inspector.check("frontend");
        assert!(
            observer.join().unwrap(),
            "sync stage must acknowledge readiness"
        );
        let error = result.unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<revenant_core::Error>()
                .map(|e| e.code.as_str()),
            Some("request_cancelled")
        );
        let attempt: Value = serde_json::from_slice(
            &fs::read(root.path().join(".revenant/inspection/check.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(attempt["status"], "failed");
        assert!(attempt["finishedAt"].is_number());
        assert_eq!(attempt["diagnostics"][0]["code"], "request_cancelled");
        assert!(attempt["completedSourceFingerprint"].is_null());
        let _lease = acquire_project_lock(root.path()).unwrap();
    }

    #[test]
    fn cancelled_inspection_does_not_become_unavailable() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("revenant.toml"),
            "version = 3\n[project]\nname = 'cancelled'\n",
        )
        .unwrap();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let inspector = ProjectInspector::open(root.path())
            .unwrap()
            .with_cancellation(cancelled.clone(), cancelled);
        let error = inspector
            .semantic(json!({"action":"diagnostics"}))
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<revenant_core::Error>().unwrap().code,
            "request_cancelled"
        );
        assert!(inspector.snapshot().is_err());
        assert!(inspector.check("frontend").is_err());
        assert!(!root.path().join(".revenant/inspection/check.json").exists());
    }
}

/// Run only framework-selected programs with bounded output and tree ownership.
pub(crate) fn bounded_process(
    root: &Path,
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<(bool, String, String)> {
    cancellable_process(root, program, args, env, timeout, &Cancellation::default())
}

fn process_cleanup_failure(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<revenant_core::Error>())
        .any(|error| error.code == "inspection_process_cleanup")
}

fn cancellable_process(
    root: &Path,
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<(bool, String, String)> {
    cancellation.checkpoint()?;
    let (tx, rx) = mpsc::channel();
    let mut child = ManagedProcess::spawn_with_env(root, program, args, env, "inspection", tx)?;
    let started = Instant::now();
    let mut stdout = String::new();
    let mut stderr = String::new();
    let collect =
        |line: process::LogLine, stdout: &mut String, stderr: &mut String| -> Result<()> {
            let buffer = if line.is_stderr { stderr } else { stdout };
            ensure!(
                buffer.len() + line.line.len() < 2 * RESULT_BYTES,
                "inspection subprocess exceeds output budget"
            );
            buffer.push_str(&line.line);
            buffer.push('\n');
            Ok(())
        };
    let outcome = (|| -> Result<_> {
        loop {
            cancellation.checkpoint()?;
            // A chatty producer must not starve cancellation/timeout checks.
            for _ in 0..256 {
                let Ok(line) = rx.try_recv() else { break };
                collect(line, &mut stdout, &mut stderr)?;
            }
            ensure!(!process::interrupted(), "inspection interrupted");
            ensure!(
                started.elapsed() < timeout,
                "inspection subprocess timed out"
            );
            if let Some(status) = child.try_wait()? {
                break Ok(status);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })();
    let cleanup = child.terminate_checked();
    if let Err(error) = cleanup {
        return Err(revenant_core::Error::new(
            "inspection_process_cleanup",
            format!(
                "{error:#}; inspection outcome: {}",
                match &outcome {
                    Ok(_) => "completed".to_owned(),
                    Err(original) => format!("{original:#}"),
                }
            ),
        )
        .into());
    }
    let status = outcome?;
    cancellation.checkpoint()?;
    while let Ok(line) = rx.try_recv() {
        collect(line, &mut stdout, &mut stderr)?;
    }
    Ok((status.success(), stdout, stderr))
}
