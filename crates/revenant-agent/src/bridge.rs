//! Authenticated current-user IPC, bounded frames, and per-connection authority.
use crate::{AgentTool, Backend, BackendFuture};
use revenant_core::{Error, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Mutex, oneshot},
    task::{JoinHandle, JoinSet},
};

const MAX_FRAME: usize = 1_048_576;
const MAX_CONNECTIONS: usize = 32;
const IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Construct isolated authority for each authenticated connection.
pub type BackendFactory = Arc<dyn Fn(BridgeAccess) -> Result<Arc<dyn Backend>> + Send + Sync>;
trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
type LocalStream = Box<dyn Stream>;

/// Diagnostic access is available only from an explicitly development instance.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BridgeAccess {
    /// Author-exposed application operations with owned task controls.
    Application,
    /// Read-only native diagnostics in an explicitly development instance.
    Development,
}

/// Public instance identity; authentication material is never returned by discovery.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstanceInfo {
    /// Unique identity of this running native runtime.
    pub instance_id: String,
    /// Stable author-configured application identity.
    pub app_id: String,
    /// Digest of the exact compiled operation contracts in this process.
    pub contract_digest: String,
    /// Native process ID useful for diagnosing stale discovery records.
    pub process_id: u32,
    /// Development project path; absent for packaged applications.
    pub project_root: Option<PathBuf>,
    /// Whether separate development observation is enabled.
    pub development: bool,
    /// Protected local endpoint; never a remote network listener.
    pub endpoint: String,
}
impl InstanceInfo {
    /// Allocate instance identity without opening a bridge or starting an app.
    pub fn new(
        app_id: impl Into<String>,
        digest: impl Into<String>,
        project_root: Option<PathBuf>,
        development: bool,
    ) -> Self {
        Self {
            instance_id: uuid::Uuid::new_v4().to_string(),
            app_id: app_id.into(),
            contract_digest: digest.into(),
            process_id: std::process::id(),
            project_root,
            development,
            endpoint: String::new(),
        }
    }
}

/// Multiple matching live instances require an explicit identity.
#[derive(Clone, Debug, Default)]
pub struct InstanceSelector {
    /// Restrict matches to one configured application.
    pub app_id: Option<String>,
    /// Restrict matches to one canonical development project.
    pub project_root: Option<PathBuf>,
    /// Explicit identity resolving ambiguity among matching app instances.
    pub instance_id: Option<String>,
    /// Select separate development authority, excluding packaged instances.
    pub development_only: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    info: InstanceInfo,
    token: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Hello {
    instance_id: String,
    contract_digest: String,
    token: String,
    access: BridgeAccess,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
enum Request {
    Call { name: String, arguments: Value },
    Close,
}
fn io_error(error: impl std::fmt::Display) -> Error {
    Error::new("bridge_io", error.to_string())
}

/// Dedicated current-user discovery directory, separate from project source.
pub fn discovery_directory() -> Result<PathBuf> {
    #[cfg(windows)]
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| Error::new("bridge_directory", "LOCALAPPDATA is unavailable"))?;
    #[cfg(unix)]
    let root = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache")))
        .ok_or_else(|| Error::new("bridge_directory", "No user runtime directory"))?;
    Ok(root.join("Revenant").join("agents"))
}
/// List bounded discovery records; live authenticated connections establish liveness.
pub fn discover_instances() -> Result<Vec<InstanceInfo>> {
    discover_instances_in(&discovery_directory()?)
}
/// List bounded discovery records in an explicit test or user directory.
pub fn discover_instances_in(directory: &Path) -> Result<Vec<InstanceInfo>> {
    Ok(records(directory)?
        .into_iter()
        .map(|(_, record)| record.info)
        .collect())
}
fn records(directory: &Path) -> Result<Vec<(PathBuf, Record)>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    secure::validate(directory, true)?;
    if std::fs::symlink_metadata(directory)
        .map_err(io_error)?
        .file_type()
        .is_symlink()
    {
        return Err(Error::new(
            "bridge_directory",
            "Discovery directory cannot be a symlink",
        ));
    }
    let mut records = Vec::new();
    for item in std::fs::read_dir(directory).map_err(io_error)?.take(1024) {
        let entry = item.map_err(io_error)?;
        if entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "json")
        {
            continue;
        }
        let metadata = entry.metadata().map_err(io_error)?;
        if !entry.file_type().map_err(io_error)?.is_file() || metadata.len() > 65_536 {
            continue;
        }
        secure::validate(&entry.path(), false)?;
        if let Ok(record) =
            serde_json::from_slice::<Record>(&std::fs::read(entry.path()).map_err(io_error)?)
            && valid_instance_id(&record.info.instance_id)
        {
            records.push((entry.path(), record));
        }
    }
    records.sort_by(|a, b| a.1.info.instance_id.cmp(&b.1.info.instance_id));
    Ok(records)
}
fn valid_instance_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id.strip_prefix("instance-").unwrap_or(id)).is_ok()
}
async fn write_frame<T: Serialize>(stream: &mut LocalStream, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME {
        return Err(Error::new(
            "bridge_frame_limit",
            "Local bridge message exceeds 1 MiB",
        ));
    }
    stream
        .write_u32(bytes.len() as u32)
        .await
        .map_err(io_error)?;
    stream.write_all(&bytes).await.map_err(io_error)?;
    stream.flush().await.map_err(io_error)
}
async fn read_frame<T: DeserializeOwned>(stream: &mut LocalStream) -> Result<T> {
    let length = stream.read_u32().await.map_err(io_error)? as usize;
    if length > MAX_FRAME {
        return Err(Error::new(
            "bridge_frame_limit",
            "Local bridge message exceeds 1 MiB",
        ));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await.map_err(io_error)?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// Lifetime owner of one local listener and its active native sessions.
pub struct BridgeServer {
    info: InstanceInfo,
    record_path: PathBuf,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<()>>>,
}
impl BridgeServer {
    /// Open a protected current-user local listener and publish its identity.
    pub async fn start(info: InstanceInfo, factory: BackendFactory) -> Result<Self> {
        Self::start_in(&discovery_directory()?, info, factory).await
    }
    /// Open a protected listener in an explicit discovery directory.
    pub async fn start_in(
        directory: &Path,
        mut info: InstanceInfo,
        factory: BackendFactory,
    ) -> Result<Self> {
        if !valid_instance_id(&info.instance_id) {
            return Err(Error::new(
                "bridge_identity",
                "Instance identity must contain a UUID",
            ));
        }
        secure::directory(directory)?;
        if let Some(root) = &info.project_root {
            info.project_root = Some(std::fs::canonicalize(root).map_err(io_error)?);
        }
        info.endpoint = endpoint(directory, &info.instance_id)?;
        let listener = Listener::new(&info.endpoint)?;
        let record_path = directory.join(format!("{}.json", info.instance_id));
        let token = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let record = Record {
            info: info.clone(),
            token: token.clone(),
        };
        // Protected directory inheritance covers the file before it is published.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&record_path)
            .map_err(io_error)?;
        secure::file(&record_path)?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(&record)?).map_err(io_error)?;
        std::io::Write::flush(&mut file).map_err(io_error)?;
        let (stop, stopped) = oneshot::channel();
        let connection_info = info.clone();
        let task = tokio::spawn(async move {
            listen(listener, connection_info, token, factory, stopped).await
        });
        Ok(Self {
            info,
            record_path,
            stop: Some(stop),
            task: Some(task),
        })
    }
    /// Public identity of the exact native instance represented by this listener.
    pub fn info(&self) -> &InstanceInfo {
        &self.info
    }
    /// Stop admission, close all connection scopes, and remove discovery files.
    pub async fn stop(mut self) -> Result<()> {
        self.request_stop();
        let result = match self.task.take() {
            Some(task) => task.await.map_err(io_error)?,
            None => Ok(()),
        };
        self.remove_record();
        result
    }
    /// Signal shutdown immediately before awaiting native cleanup or other work.
    pub fn request_stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.remove_record();
    }
    fn remove_record(&self) {
        let _ = std::fs::remove_file(&self.record_path);
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.info.endpoint);
        }
    }
}
impl Drop for BridgeServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.remove_record();
    }
}
async fn listen(
    mut listener: Listener,
    info: InstanceInfo,
    token: String,
    factory: BackendFactory,
    mut stopped: oneshot::Receiver<()>,
) -> Result<()> {
    let mut sessions = JoinSet::new();
    let (ending, _) = tokio::sync::watch::channel(false);
    loop {
        tokio::select! {
            biased;
            _ = &mut stopped => break,
            Some(outcome) = sessions.join_next(), if !sessions.is_empty() => { if let Err(error) = outcome { eprintln!("Revenant bridge session failed: {error}"); } },
            connection = listener.accept(), if sessions.len() < MAX_CONNECTIONS => {
                let stream = connection?;
                let info = info.clone(); let token = token.clone(); let factory = factory.clone(); let ending = ending.subscribe();
                sessions.spawn(async move { session(stream, info, token, factory, ending).await });
            }
        }
    }
    let _ = ending.send(true);
    while let Some(outcome) = sessions.join_next().await {
        if let Err(error) = outcome {
            eprintln!("Revenant bridge shutdown failed: {error}");
        }
    }
    Ok(())
}
async fn session(
    mut stream: LocalStream,
    info: InstanceInfo,
    token: String,
    factory: BackendFactory,
    mut ending: tokio::sync::watch::Receiver<bool>,
) {
    let hello: Hello = tokio::select! {
        _ = ending.changed() => return,
        received = tokio::time::timeout(IO_TIMEOUT, read_frame(&mut stream)) => match received { Ok(Ok(hello)) => hello, _ => return },
    };
    let opened = if hello.token != token || hello.instance_id != info.instance_id {
        Err(Error::new(
            "bridge_identity",
            "Local bridge authentication failed",
        ))
    } else if hello.contract_digest != info.contract_digest {
        Err(Error::new(
            "stale_contract",
            "Selected instance has a different contract digest",
        ))
    } else if hello.access == BridgeAccess::Development && !info.development {
        Err(Error::new(
            "development_disabled",
            "Development access is disabled in this instance",
        ))
    } else {
        factory(hello.access)
    };
    let backend = match opened {
        Ok(backend) => backend,
        Err(error) => {
            let rejection: Result<Vec<AgentTool>> = Err(error);
            tokio::select! { _ = ending.changed() => {}, _ = tokio::time::timeout(IO_TIMEOUT, write_frame(&mut stream, &rejection)) => {} }
            return;
        }
    };
    let tools = Ok::<_, Error>(backend.tools());
    let catalogue = tokio::select! {
        _ = ending.changed() => false,
        result = tokio::time::timeout(IO_TIMEOUT, write_frame(&mut stream, &tools)) => matches!(result, Ok(Ok(()))),
    };
    if catalogue {
        loop {
            let request = tokio::select! { _ = ending.changed() => break, request = read_frame::<Request>(&mut stream) => request };
            let result = match request {
                Ok(Request::Call { name, arguments }) => tokio::select! {
                    _ = ending.changed() => break,
                    // The client serializes calls. An EOF cancels the active
                    // backend future; pipelined frames violate this protocol.
                    _ = read_frame::<Request>(&mut stream) => break,
                    result = backend.call(&name, arguments) => result
                },
                Ok(Request::Close) => break,
                Err(_) => break,
            };
            let result = if serde_json::to_vec(&result)
                .map_or(true, |bytes| bytes.len() > MAX_FRAME)
            {
                Err(Error::new(
                    "bridge_result_limit",
                    "Result exceeds the local bridge frame budget; request a smaller result page",
                ))
            } else {
                result
            };
            let delivered = tokio::select! {
                _ = ending.changed() => false,
                outcome = tokio::time::timeout(IO_TIMEOUT, write_frame(&mut stream, &result)) => matches!(outcome, Ok(Ok(()))),
            };
            if !delivered {
                break;
            }
        }
    }
    if let Err(error) = backend.close().await {
        eprintln!("Revenant session cleanup: {error}");
    }
}

/// Authenticated connection that forwards a cached tool catalogue to MCP.
pub struct RemoteBackend {
    info: InstanceInfo,
    tools: Vec<AgentTool>,
    stream: Mutex<Option<LocalStream>>,
    closed: tokio::sync::watch::Sender<bool>,
}
impl RemoteBackend {
    /// Authenticate one live matching instance from the current user's directory.
    pub async fn connect(selector: InstanceSelector) -> Result<Self> {
        Self::connect_in(&discovery_directory()?, selector).await
    }
    /// Authenticate one live matching instance from an explicit discovery directory.
    pub async fn connect_in(directory: &Path, selector: InstanceSelector) -> Result<Self> {
        let project = selector
            .project_root
            .as_deref()
            .map(std::fs::canonicalize)
            .transpose()
            .map_err(io_error)?;
        let candidates = records(directory)?.into_iter().filter(|(_, record)| {
            selector
                .app_id
                .as_ref()
                .is_none_or(|id| id == &record.info.app_id)
                && selector
                    .instance_id
                    .as_ref()
                    .is_none_or(|id| id == &record.info.instance_id)
                && project
                    .as_ref()
                    .is_none_or(|root| record.info.project_root.as_ref() == Some(root))
                && (!selector.development_only || record.info.development)
        });
        let mut live = Vec::new();
        for (_, record) in candidates {
            if record.info.endpoint != endpoint(directory, &record.info.instance_id)? {
                continue;
            }
            if let Ok(Ok(stream)) = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                connect_stream(&record.info.endpoint),
            )
            .await
            {
                live.push((record, stream));
            }
        }
        if live.len() != 1 {
            return Err(Error::new(if live.is_empty() { "instance_missing" } else { "instance_ambiguous" }, if live.is_empty() { "No matching running application" } else { "Multiple running applications match; select an instance ID" }).details(serde_json::json!({"instances":live.iter().map(|(r,_)| &r.info).collect::<Vec<_>>()})));
        }
        let (record, mut stream) = live.pop().unwrap();
        let hello = Hello {
            instance_id: record.info.instance_id.clone(),
            contract_digest: record.info.contract_digest.clone(),
            token: record.token,
            access: if selector.development_only {
                BridgeAccess::Development
            } else {
                BridgeAccess::Application
            },
        };
        write_frame(&mut stream, &hello).await?;
        let tools: Result<Vec<AgentTool>> =
            tokio::time::timeout(IO_TIMEOUT, read_frame(&mut stream))
                .await
                .map_err(io_error)??;
        let (closed, _) = tokio::sync::watch::channel(false);
        Ok(Self {
            info: record.info,
            tools: tools?,
            stream: Mutex::new(Some(stream)),
            closed,
        })
    }
    /// Identity authenticated before accepting the connection's catalogue.
    pub fn info(&self) -> &InstanceInfo {
        &self.info
    }
}
impl Backend for RemoteBackend {
    fn tools(&self) -> Vec<AgentTool> {
        self.tools.clone()
    }
    fn call<'a>(&'a self, name: &'a str, arguments: Value) -> BackendFuture<'a, Value> {
        Box::pin(async move {
            if !self.tools.iter().any(|tool| tool.name == name) {
                return Err(Error::new(
                    "hidden_operation",
                    "Tool is unavailable on this connection",
                ));
            }
            let request = Request::Call {
                name: name.to_owned(),
                arguments,
            };
            if serde_json::to_vec(&request)?.len() > MAX_FRAME {
                return Err(Error::new(
                    "bridge_frame_limit",
                    "Local bridge message exceeds 1 MiB",
                ));
            }
            let mut stream = self.stream.lock().await;
            let mut ending = self.closed.subscribe();
            if *ending.borrow_and_update() {
                return Err(Error::new(
                    "session_closed",
                    "Application connection has closed",
                ));
            }
            // A cancelled future drops this loan. Never reuse unread or partially
            // read response bytes as the reply to a subsequent call.
            let mut connection = stream
                .take()
                .ok_or_else(|| Error::new("session_closed", "Application connection has closed"))?;
            let response = tokio::select! {
                _ = ending.changed() => Err(Error::new("session_closed", "Application connection has closed")),
                response = async {
                    write_frame(&mut connection, &request).await?;
                    read_frame::<Result<Value>>(&mut connection).await
                } => response,
            };
            match response {
                Ok(result) => {
                    if !*self.closed.borrow() {
                        *stream = Some(connection);
                    }
                    result
                }
                Err(error) => Err(error),
            }
        })
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            self.closed.send_replace(true);
            if let Some(mut stream) = self.stream.lock().await.take() {
                let _ = tokio::time::timeout(IO_TIMEOUT, async {
                    write_frame(&mut stream, &Request::Close).await?;
                    stream.shutdown().await.map_err(io_error)
                })
                .await;
            }
            Ok(())
        })
    }
}
#[cfg(windows)]
fn endpoint(_: &Path, id: &str) -> Result<String> {
    Ok(format!(r"\\.\pipe\revenant-{}-{id}", secure::user_sid()?))
}
#[cfg(unix)]
fn endpoint(directory: &Path, id: &str) -> Result<String> {
    Ok(directory
        .join(format!("{id}.sock"))
        .to_string_lossy()
        .into_owned())
}

#[cfg(windows)]
struct Listener {
    server: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    endpoint: String,
}
#[cfg(windows)]
impl Listener {
    fn new(endpoint: &str) -> Result<Self> {
        Ok(Self {
            server: Some(secure::pipe(endpoint, true)?),
            endpoint: endpoint.to_owned(),
        })
    }
    async fn accept(&mut self) -> Result<LocalStream> {
        self.server
            .as_ref()
            .unwrap()
            .connect()
            .await
            .map_err(io_error)?;
        let replacement = secure::pipe(&self.endpoint, false)?;
        Ok(Box::new(self.server.replace(replacement).unwrap()))
    }
}
#[cfg(windows)]
async fn connect_stream(endpoint: &str) -> Result<LocalStream> {
    loop {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint) {
            Ok(stream) => return Ok(Box::new(stream)),
            Err(error) if error.raw_os_error() == Some(231) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await
            }
            Err(error) => return Err(io_error(error)),
        }
    }
}
#[cfg(unix)]
struct Listener(tokio::net::UnixListener);
#[cfg(unix)]
impl Listener {
    fn new(endpoint: &str) -> Result<Self> {
        let listener = tokio::net::UnixListener::bind(endpoint).map_err(io_error)?;
        secure::file(Path::new(endpoint))?;
        Ok(Self(listener))
    }
    async fn accept(&mut self) -> Result<LocalStream> {
        let stream = self.0.accept().await.map_err(io_error)?.0;
        secure::peer(&stream)?;
        Ok(Box::new(stream))
    }
}
#[cfg(unix)]
async fn connect_stream(endpoint: &str) -> Result<LocalStream> {
    secure::validate(Path::new(endpoint), false)?;
    let stream = tokio::net::UnixStream::connect(endpoint)
        .await
        .map_err(io_error)?;
    secure::peer(&stream)?;
    Ok(Box::new(stream))
}
#[cfg(unix)]
mod secure {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    fn uid() -> u32 {
        // SAFETY: geteuid has no arguments or memory effects.
        unsafe { libc::geteuid() }
    }
    fn ancestors(path: &Path) -> Result<()> {
        for ancestor in path.ancestors().skip(1) {
            let Ok(mut metadata) = std::fs::symlink_metadata(ancestor) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                // Darwin exposes /var and /tmp through system-owned aliases.
                // A user-owned link remains replaceable and cannot establish trust.
                if metadata.uid() != 0 {
                    return Err(Error::new(
                        "bridge_directory",
                        "Discovery ancestors cannot contain user-owned symlinks",
                    ));
                }
                metadata = std::fs::metadata(ancestor).map_err(io_error)?;
            }
            let sticky_root = metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
            if !metadata.is_dir()
                || (metadata.uid() != uid() && metadata.uid() != 0)
                || (metadata.mode() & 0o022 != 0 && !sticky_root)
            {
                return Err(Error::new(
                    "bridge_directory",
                    "Discovery ancestors must be protected current-user or system directories",
                ));
            }
        }
        Ok(())
    }
    pub fn validate(path: &Path, directory: bool) -> Result<()> {
        ancestors(path)?;
        let metadata = std::fs::symlink_metadata(path).map_err(io_error)?;
        if metadata.file_type().is_symlink()
            || metadata.uid() != uid()
            || metadata.mode() & 0o077 != 0
            || (directory && !metadata.is_dir())
        {
            return Err(Error::new(
                "bridge_identity",
                "Discovery and endpoints must be protected and owned by the current user",
            ));
        }
        Ok(())
    }
    pub fn peer(stream: &tokio::net::UnixStream) -> Result<()> {
        if stream.peer_cred().map_err(io_error)?.uid() != uid() {
            return Err(Error::new(
                "bridge_identity",
                "Local socket peer belongs to another user",
            ));
        }
        Ok(())
    }
    pub fn directory(path: &Path) -> Result<()> {
        ancestors(path)?;
        if path.exists()
            && std::fs::symlink_metadata(path)
                .map_err(io_error)?
                .file_type()
                .is_symlink()
        {
            return Err(Error::new(
                "bridge_directory",
                "Discovery directory cannot be a symlink",
            ));
        }
        if path.exists() && std::fs::symlink_metadata(path).map_err(io_error)?.uid() != uid() {
            return Err(Error::new(
                "bridge_identity",
                "Discovery directory belongs to another user",
            ));
        }
        std::fs::create_dir_all(path).map_err(io_error)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(io_error)
    }
    pub fn file(path: &Path) -> Result<()> {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io_error)
    }
}
#[cfg(windows)]
mod secure;

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Catalogue {
        closed: Arc<AtomicBool>,
    }
    impl Backend for Catalogue {
        fn tools(&self) -> Vec<AgentTool> {
            vec![AgentTool {
                name: "large".into(),
                description: "x".repeat(900_000),
                input_schema: serde_json::json!({"type":"object"}),
                output_schema: None,
                read_only: true,
            }]
        }
        fn call<'a>(&'a self, _: &'a str, _: Value) -> BackendFuture<'a, Value> {
            Box::pin(async { Ok(Value::Null) })
        }
        fn close(&self) -> BackendFuture<'_, ()> {
            self.closed.store(true, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }
    #[tokio::test]
    async fn shutdown_interrupts_a_catalogue_write_when_the_authenticated_peer_stops_reading() {
        let directory = tempfile::tempdir().unwrap();
        let opened = Arc::new(tokio::sync::Notify::new());
        let closed = Arc::new(AtomicBool::new(false));
        let factory_opened = opened.clone();
        let factory_closed = closed.clone();
        let server = BridgeServer::start_in(
            directory.path(),
            InstanceInfo::new("test.catalogue", "digest", None, false),
            Arc::new(move |_| {
                factory_opened.notify_one();
                Ok(Arc::new(Catalogue {
                    closed: factory_closed.clone(),
                }) as Arc<dyn Backend>)
            }),
        )
        .await
        .unwrap();
        let record: Record =
            serde_json::from_slice(&std::fs::read(&server.record_path).unwrap()).unwrap();
        let mut stream = connect_stream(&record.info.endpoint).await.unwrap();
        write_frame(
            &mut stream,
            &Hello {
                instance_id: record.info.instance_id,
                contract_digest: record.info.contract_digest,
                token: record.token,
                access: BridgeAccess::Application,
            },
        )
        .await
        .unwrap();
        opened.notified().await;
        tokio::time::timeout(std::time::Duration::from_secs(2), server.stop())
            .await
            .expect("shutdown must interrupt catalogue delivery")
            .unwrap();
        assert!(closed.load(Ordering::SeqCst));
    }
}
