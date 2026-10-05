//! Native JSON persistence. One store serializes loads and writes, and remembers
//! the last successfully loaded defaults for each key as its validation schema.
//! Unknown stored fields survive; defaults fill only missing object fields.
//! Arrays retain their length and validate corresponding default elements.
//! Every encoded record, including merged defaults, is bounded to 1 MiB.
//! Writes use a synced same-directory tempfile and atomic overwrite, including
//! on Windows filesystems supporting POSIX rename semantics. Unsupported Windows
//! filesystems fail without falling back to replacement that can hide the target
//! from concurrent readers. There is no deferred write queue or webview storage dependency.
//! The host must run these synchronous filesystem methods on its bounded worker.

use revenant_core::serde_json::{self, Value, json};
use revenant_core::{Error, Result};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_DEPTH: usize = 128;

/// A native store with serialized per-instance operations and remembered schemas.
/// Use one instance per data directory; separate processes are last-writer-wins.
pub struct SettingsStore {
    directory: PathBuf,
    schemas: Mutex<HashMap<String, Value>>,
}

impl SettingsStore {
    /// Creates the settings directory beneath the host-owned application data
    /// directory. Native paths never come from a client key.
    pub fn new(data_dir: &Path) -> Result<Self> {
        let directory = data_dir.join("settings");
        fs::create_dir_all(&directory).map_err(|e| io_error("create", e))?;
        let directory = directory
            .canonicalize()
            .map_err(|e| io_error("resolve", e))?;
        Ok(Self {
            directory,
            schemas: Mutex::new(HashMap::new()),
        })
    }

    /// Loads a bounded record and merges missing defaults without replacing
    /// existing data. Missing files return defaults; malformed JSON, oversized
    /// records and type mismatches return structured errors. A successful load
    /// remembers defaults without automatically rewriting the stored file.
    pub fn load(&self, key: &str, defaults: Value) -> Result<Value> {
        let path = self.path(key)?;
        encode(&defaults)?;
        let mut schemas = self.schemas.lock().map_err(|_| poisoned())?;
        let mut value = match File::open(path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take((MAX_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|e| io_error("read", e))?;
                if bytes.len() > MAX_BYTES {
                    return Err(too_large());
                }
                serde_json::from_slice(&bytes).map_err(|e| {
                    Error::new("settings_invalid_json", e.to_string()).details(json!({"key": key}))
                })?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => defaults.clone(),
            Err(e) => return Err(io_error("open", e)),
        };
        merge(&mut value, &defaults, "", 0)?;
        encode(&value)?;
        schemas.insert(key.into(), defaults);
        Ok(value)
    }

    /// Validates against remembered defaults, merges missing object fields, and
    /// atomically persists JSON. Before the first load, any bounded JSON value
    /// is accepted. Success means the temporary file was synced and replaced;
    /// directory-entry survival across sudden power loss is platform-dependent.
    pub fn save(&self, key: &str, mut value: Value) -> Result<()> {
        let path = self.path(key)?;
        encode(&value)?;
        let schemas = self.schemas.lock().map_err(|_| poisoned())?;
        if let Some(defaults) = schemas.get(key) {
            merge(&mut value, defaults, "", 0)?;
        }
        let bytes = encode(&value)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|e| io_error("create temporary", e))?;
        temporary
            .write_all(&bytes)
            .map_err(|e| io_error("write", e))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|e| io_error("sync", e))?;
        persist(temporary, &path).map_err(|e| io_error("replace", e))?;
        // Keep the serialization lock through replacement, not only validation.
        drop(schemas);
        Ok(())
    }

    /// Waits for any current write to finish. All writes are immediate, so there
    /// are no pending records to flush after acquiring the serialization lock.
    pub fn flush(&self) -> Result<()> {
        drop(self.schemas.lock().map_err(|_| poisoned())?);
        Ok(())
    }

    fn path(&self, key: &str) -> Result<PathBuf> {
        if key.is_empty()
            || key.len() > 96
            || key == "."
            || key == ".."
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(Error::new(
                "settings_invalid_key",
                "Keys must contain 1..=96 ASCII letters, digits, dots, underscores or hyphens",
            ));
        }
        // Hex preserves case distinctions on Windows and avoids reserved device
        // names, separators, trailing dots, and filesystem normalization aliases.
        let mut filename = String::from("k-");
        for byte in key.bytes() {
            use std::fmt::Write as _;
            write!(filename, "{byte:02x}").expect("writing to String cannot fail");
        }
        filename.push_str(".json");
        Ok(self.directory.join(filename))
    }
}

#[cfg(not(windows))]
fn persist(temporary: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
    temporary.persist(path).map(|_| ()).map_err(|e| e.error)
}

#[cfg(windows)]
fn persist(temporary: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
    use std::{
        ffi::c_void,
        mem::{offset_of, size_of},
        os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
        ptr,
    };

    // Win32 FILE_RENAME_INFO's initial union is represented by its DWORD Flags
    // member. The variable UTF-16 tail follows this C ABI header on 32/64-bit.
    // Definitions: windows-sys 0.61.2 / Win32 Storage FileSystem and winbase.h.
    #[repr(C)]
    struct RenameInfo {
        flags: u32,
        root_directory: *mut c_void,
        name_bytes: u32,
        name: [u16; 1],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetFileAttributesW(name: *const u16, attributes: u32) -> i32;
        fn SetFileInformationByHandle(
            file: *mut c_void,
            class: i32,
            information: *const c_void,
            bytes: u32,
        ) -> i32;
    }
    const DELETE: u32 = 0x0001_0000;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    const FILE_RENAME_INFO_EX: i32 = 22;
    const FILE_RENAME_REPLACE_IF_EXISTS: u32 = 1;
    const FILE_RENAME_POSIX_SEMANTICS: u32 = 2;

    let mut temporary = temporary.into_temp_path();
    let source_name: Vec<_> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    // Match tempfile's normalization before publication: a persisted record
    // must no longer carry FILE_ATTRIBUTE_TEMPORARY's caching hint.
    // SAFETY: source_name is a live, NUL-terminated UTF-16 pathname.
    if unsafe { SetFileAttributesW(source_name.as_ptr(), FILE_ATTRIBUTE_NORMAL) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let source = fs::OpenOptions::new()
        .access_mode(DELETE)
        .share_mode(1 | 2 | 4)
        .open(&temporary)?;
    let name: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let name_offset = offset_of!(RenameInfo, name);
    let buffer_bytes = name_offset + name.len() * size_of::<u16>();
    let bytes = u32::try_from(buffer_bytes).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Settings path is too long",
        )
    })?;
    // usize storage supplies the alignment required by the HANDLE member. Keep
    // at least sizeof(header), including its padding, plus the full UTF-16 tail.
    let mut buffer = vec![
        0usize;
        buffer_bytes
            .max(size_of::<RenameInfo>())
            .div_ceil(size_of::<usize>())
    ];
    let information = buffer.as_mut_ptr().cast::<RenameInfo>();
    // SAFETY: the aligned, zero-initialized allocation covers all header fields
    // and the variable tail. The name copy derives its pointer from the entire
    // allocation, not from the one-element header array. source owns a live
    // DELETE-capable handle; all pointers remain valid through this synchronous call.
    let renamed = unsafe {
        (*information).flags = FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS;
        (*information).root_directory = ptr::null_mut();
        (*information).name_bytes = ((name.len() - 1) * size_of::<u16>()) as u32;
        ptr::copy_nonoverlapping(
            name.as_ptr(),
            buffer
                .as_mut_ptr()
                .cast::<u8>()
                .add(name_offset)
                .cast::<u16>(),
            name.len(),
        );
        SetFileInformationByHandle(
            source.as_raw_handle(),
            FILE_RENAME_INFO_EX,
            information.cast(),
            bytes,
        )
    };
    if renamed == 0 {
        let error = std::io::Error::last_os_error();
        // A weaker MoveFileEx/ReplaceFile fallback can make the pathname absent
        // or delete-pending between complete writes. Retrying the writer cannot
        // repair that hole for external readers, so unsupported volumes fail closed.
        return Err(match error.raw_os_error() {
            Some(1 | 50 | 87) => std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!(
                    "Settings filesystem does not support atomic replacement with open readers: {error}"
                ),
            ),
            _ => error,
        });
    }
    // The rename transferred ownership to the target. Do not attempt cleanup
    // of the old temporary pathname after another writer could reuse that name.
    temporary.disable_cleanup(true);
    Ok(())
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn merge(value: &mut Value, defaults: &Value, pointer: &str, depth: usize) -> Result<()> {
    if depth >= MAX_DEPTH {
        return Err(Error::new(
            "settings_depth_exceeded",
            "Settings nesting exceeds 128 levels",
        ));
    }
    if kind(value) != kind(defaults) {
        return Err(Error::new(
            "settings_type_mismatch",
            "Settings value does not match defaults",
        )
        .details(json!({"pointer": pointer, "expected": kind(defaults), "actual": kind(value)})));
    }
    match (value, defaults) {
        (Value::Object(value), Value::Object(defaults)) => {
            for (key, default) in defaults {
                if let Some(existing) = value.get_mut(key) {
                    let escaped = key.replace('~', "~0").replace('/', "~1");
                    merge(
                        existing,
                        default,
                        &format!("{pointer}/{escaped}"),
                        depth + 1,
                    )?;
                } else {
                    value.insert(key.clone(), default.clone());
                }
            }
        }
        (Value::Array(value), Value::Array(defaults)) => {
            for (index, (existing, default)) in value.iter_mut().zip(defaults).enumerate() {
                merge(existing, default, &format!("{pointer}/{index}"), depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

struct BoundedJson(Vec<u8>);

impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_BYTES - self.0.len() {
            return Err(std::io::Error::other("settings byte limit exceeded"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encode(value: &Value) -> Result<Vec<u8>> {
    validate_depth(value, 0)?;
    let mut bytes = BoundedJson(Vec::new());
    serde_json::to_writer(&mut bytes, value).map_err(|_| too_large())?;
    Ok(bytes.0)
}

fn validate_depth(value: &Value, depth: usize) -> Result<()> {
    if depth >= MAX_DEPTH {
        return Err(Error::new(
            "settings_depth_exceeded",
            "Settings nesting exceeds 128 levels",
        ));
    }
    match value {
        Value::Array(values) => {
            for value in values {
                validate_depth(value, depth + 1)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                validate_depth(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn too_large() -> Error {
    Error::new("settings_too_large", "Settings JSON must not exceed 1 MiB")
        .details(json!({"maxBytes": MAX_BYTES}))
}

fn io_error(operation: &str, error: std::io::Error) -> Error {
    Error::new(
        "settings_io",
        format!("Cannot {operation} settings: {error}"),
    )
    .details(json!({"operation": operation, "ioKind": format!("{:?}", error.kind())}))
}

fn poisoned() -> Error {
    Error::new(
        "settings_unavailable",
        "Settings synchronization was poisoned",
    )
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn native_settings_persist_while_old_reader_remains_open() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(directory.path()).unwrap();
        let original = json!({"generation": 0, "payload": "old".repeat(128)});
        store.save("observed", original.clone()).unwrap();
        let path = store.path("observed").unwrap();
        // Hold the old file across every replacement, rather than relying on
        // the scheduler to overlap a short read with one rename by chance.
        let mut old_reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2 | 4)
            .open(&path)
            .unwrap();
        for generation in 1..=32 {
            let replacement = json!({"generation": generation, "payload": "new".repeat(128)});
            store.save("observed", replacement.clone()).unwrap();
            let observed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(observed, replacement);
        }
        let mut old_bytes = Vec::new();
        old_reader.read_to_end(&mut old_bytes).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&old_bytes).unwrap(),
            original
        );
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
}
