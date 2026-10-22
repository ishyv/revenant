//! Typed native values stay behind admitted identities; no native contents cross JSON.
use super::*;
use crate::WireSafe;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    any::Any,
    marker::PhantomData,
    ops::Deref,
    path::{Path, PathBuf},
};
use ts_rs::{Config, TS, TypeVisitor};

/// An opaque wire identity for a scope-owned native value of type `T`.
/// Deserialization and `from_handle` grant no authority. Admission validates the
/// complete identity and native Rust type before returning a lease.
pub struct Resource<T: 'static> {
    handle: Handle,
    marker: PhantomData<fn() -> T>,
}
impl<T: 'static> Resource<T> {
    /// Wraps an untrusted identity; authorization occurs only when it is leased.
    pub fn from_handle(handle: Handle) -> Self {
        Self {
            handle,
            marker: PhantomData,
        }
    }
    /// Borrows the registry-issued identity; native data remains off the wire.
    pub fn handle(&self) -> &Handle {
        &self.handle
    }
}
impl<T: 'static> Clone for Resource<T> {
    fn clone(&self) -> Self {
        Self::from_handle(self.handle.clone())
    }
}
impl<T: 'static> std::fmt::Debug for Resource<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.handle.fmt(f)
    }
}
impl<T: 'static> PartialEq for Resource<T> {
    fn eq(&self, other: &Self) -> bool {
        self.handle == other.handle
    }
}
impl<T: 'static> Eq for Resource<T> {}
impl<T: 'static> Serialize for Resource<T> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.handle.serialize(s)
    }
}
impl<'de, T: 'static> Deserialize<'de> for Resource<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(Self::from_handle(Handle::deserialize(d)?))
    }
}
impl<T: 'static> JsonSchema for Resource<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        Handle::schema_name()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        Handle::json_schema(generator)
    }
}
impl<T: 'static> TS for Resource<T> {
    type WithoutGenerics = Handle;
    type OptionInnerType = Self;
    fn name(cfg: &Config) -> String {
        Handle::name(cfg)
    }
    fn inline(cfg: &Config) -> String {
        Handle::inline(cfg)
    }
    fn visit_dependencies(visitor: &mut impl TypeVisitor) {
        visitor.visit::<Handle>();
    }
}
impl<T: 'static> WireSafe for Resource<T> {
    fn visit_handles(&self, visitor: &mut dyn FnMut(&Handle) -> Result<()>) -> Result<()> {
        visitor(&self.handle)
    }
}

/// Type-erased admitted native lease for trusted hosts retaining operation inputs
/// and outputs. Its private payload cannot be forged from a wire identity.
#[derive(Clone)]
pub struct NativeResourceLease {
    pub(super) handle: Handle,
    value: Arc<dyn Any + Send + Sync>,
    revoked: Arc<std::sync::atomic::AtomicBool>,
    lifetime: Arc<NativeLifetime>,
}

// One token per admitted allocation, shared by owners and every lease. No
// identities or historical entries are retained for development accounting.
struct NativeLifetime(Arc<AtomicUsize>);
impl Drop for NativeLifetime {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
impl NativeResourceLease {
    /// Explicit UI disposal denies new host admissions from retained result
    /// history. Already admitted leases still retain and may use the value.
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(std::sync::atomic::Ordering::Acquire)
    }
    /// Borrows the complete identity validated when the lease was admitted.
    pub fn handle(&self) -> &Handle {
        &self.handle
    }
    /// Resolves the native Rust type without reacquiring the released owner.
    pub fn typed<T: Send + Sync + 'static>(&self) -> Result<NativeLease<T>> {
        let value =
            self.value.clone().downcast::<T>().map_err(|_| {
                Error::new("wrong_type", "Native resource has a different Rust type")
            })?;
        Ok(NativeLease {
            handle: self.handle.clone(),
            value,
            lifetime: self.lifetime.clone(),
        })
    }
}

/// An admitted native capability retaining `T` independently of its registry owner.
/// A clone extends its lifetime. The native value drops exactly once, after the
/// owner and all typed/erased leases have gone; task checkpoints remain separate.
pub struct NativeLease<T: Send + Sync + 'static> {
    handle: Handle,
    value: Arc<T>,
    lifetime: Arc<NativeLifetime>,
}
impl<T: Send + Sync + 'static> NativeLease<T> {
    /// Borrows the full admitted identity associated with this native value.
    pub fn handle(&self) -> &Handle {
        &self.handle
    }
}
impl<T: Send + Sync + 'static> Clone for NativeLease<T> {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            value: self.value.clone(),
            lifetime: self.lifetime.clone(),
        }
    }
}
impl<T: Send + Sync + 'static> Deref for NativeLease<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}
impl<T: Send + Sync + 'static> std::fmt::Debug for NativeLease<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.handle.fmt(f)
    }
}

impl ResourceRegistry {
    /// Returns native registry owners and live admitted allocations. Live
    /// allocations include values retained by task/result/typed leases after
    /// owner release. Counts exclude internal allocations and author-held
    /// clones inside `T`; they are lifecycle evidence, not proof of leak freedom.
    /// Accounting uses constant storage and retains no resource history.
    pub fn native_resource_counts(&self) -> (usize, usize) {
        let inner = lock(&self.inner);
        (
            inner.native_resources.len(),
            inner.native_live.load(Ordering::Acquire),
        )
    }
    /// Takes ownership of a native value in an active scope, returning its typed
    /// identity. `T` requires no serialization or contract implementation. The
    /// native value is released after scope disposal/owner release and final lease drop.
    pub fn register_native<T: Send + Sync + 'static>(
        &self,
        scope: &str,
        value: T,
    ) -> Result<Resource<T>> {
        // Retain the value outside the registry lock even when admission fails,
        // so a user-defined destructor cannot reenter a locked registry.
        let value: Arc<dyn Any + Send + Sync> = Arc::new(value);
        let handle = {
            let mut inner = lock(&self.inner);
            Self::active(&inner, scope)?;
            inner.next_resource = inner.next_resource.checked_add(1).ok_or_else(|| {
                Error::new("resource_exhausted", "Resource id space is exhausted")
            })?;
            let handle = Handle {
                id: format!("r-{}", inner.next_resource),
                scope: scope.into(),
                generation: 1,
                kind: format!("native:{}", std::any::type_name::<T>()),
            };
            inner.native_live.fetch_add(1, Ordering::AcqRel);
            let lifetime = Arc::new(NativeLifetime(inner.native_live.clone()));
            inner.native_resources.insert(
                handle.id.clone(),
                NativeResourceLease {
                    handle: handle.clone(),
                    value: value.clone(),
                    revoked: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    lifetime,
                },
            );
            handle
        };
        Ok(Resource::from_handle(handle))
    }
    pub(super) fn checked_native(
        inner: &Resources,
        scope: &str,
        handle: &Handle,
    ) -> Result<NativeResourceLease> {
        Self::active(inner, scope)?;
        if handle.scope != scope {
            return Err(Error::new(
                "scope_mismatch",
                "Handles cannot be adopted between scopes",
            ));
        }
        let lease = inner
            .native_resources
            .get(&handle.id)
            .ok_or_else(|| Error::new("stale_handle", "Native resource is missing or released"))?;
        if lease.handle != *handle {
            return Err(Error::new(
                "stale_handle",
                "Handle generation or kind does not match",
            ));
        }
        Ok(lease.clone())
    }
    /// Admits an erased native lease for trusted host result retention. Validates
    /// the complete handle and scope; later typed conversion checks its Rust type.
    pub fn lease_native_handle(&self, scope: &str, handle: &Handle) -> Result<NativeResourceLease> {
        Self::checked_native(&lock(&self.inner), scope, handle)
    }
    /// Explicitly releases a native owner and revokes fresh host admissions from
    /// its retained result leases. Validates complete identity and active scope;
    /// file owners are unsupported. Existing admitted leases remain usable.
    pub fn revoke_native(&self, scope: &str, handle: &Handle) -> Result<()> {
        let owner = {
            let mut inner = lock(&self.inner);
            let lease = Self::checked_native(&inner, scope, handle)?;
            lease
                .revoked
                .store(true, std::sync::atomic::Ordering::Release);
            inner.native_resources.remove(&handle.id)
        };
        drop(owner);
        Ok(())
    }
    /// Validates the complete wire identity and Rust type, returning an admitted
    /// lease. Existing leases survive owner release; fresh admission does not.
    pub fn lease_native<T: Send + Sync + 'static>(
        &self,
        scope: &str,
        resource: &Resource<T>,
    ) -> Result<NativeLease<T>> {
        self.lease_native_handle(scope, resource.handle())?.typed()
    }
}

/// Host-admitted directory capability. Its path is native authority available
/// only after leasing a picker-created `FolderHandle`, never from display metadata.
pub struct NativeFolder {
    path: PathBuf,
    directory: std::fs::File,
}
/// Typed identity for a directory admitted by the desktop folder picker.
pub type FolderHandle = Resource<NativeFolder>;
impl NativeFolder {
    /// Opens and retains a native directory. This trusted host API must only be
    /// used after user selection or authorized bookmark restoration.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_after_check(path.as_ref(), || {})
    }
    fn open_after_check(path: &Path, before_open: impl FnOnce()) -> Result<Self> {
        let original =
            std::fs::symlink_metadata(path).map_err(|e| Error::new("folder_io", e.to_string()))?;
        if !original.is_dir() || original.file_type().is_symlink() {
            return Err(Error::new(
                "invalid_folder",
                "Choose a directory without a symbolic link or junction",
            ));
        }
        before_open();
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Attribute-only, no recall. Opening the original name with
            // OPEN_REPARSE_POINT prevents replacement junctions redirecting it.
            options.access_mode(0x80); // FILE_READ_ATTRIBUTES
            options.custom_flags(0x02000000 | 0x00200000 | 0x00100000);
        }
        let directory = options
            .open(path)
            .map_err(|e| Error::new("folder_io", e.to_string()))?;
        let metadata = directory
            .metadata()
            .map_err(|e| Error::new("folder_io", e.to_string()))?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0 && directory_redirects_name(&directory)?
        };
        #[cfg(not(windows))]
        let reparse = false;
        if !metadata.is_dir() || reparse {
            return Err(Error::new(
                "invalid_folder",
                "Expected a native directory without a junction",
            ));
        }
        #[cfg(windows)]
        let path = path_from_directory_handle(&directory)?;
        #[cfg(not(windows))]
        let path = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if original.dev() != metadata.dev() || original.ino() != metadata.ino() {
                    return Err(Error::new(
                        "invalid_folder",
                        "Directory identity changed during admission",
                    ));
                }
            }
            std::fs::canonicalize(path).map_err(|e| Error::new("folder_io", e.to_string()))?
        };
        Ok(Self { path, directory })
    }
    /// Returns the admitted canonical root for native traversal. UI display paths
    /// are never passed here and cannot replace this captured root.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Borrows the retained directory object for metadata or handle-relative access.
    pub fn directory(&self) -> &std::fs::File {
        &self.directory
    }
}

#[cfg(windows)]
fn name_surrogate_tag(tag: u32) -> bool {
    tag & 0x2000_0000 != 0
}

#[cfg(windows)]
fn directory_redirects_name(directory: &std::fs::File) -> Result<bool> {
    use std::{ffi::c_void, os::windows::io::AsRawHandle};
    #[repr(C)]
    struct AttributeTag {
        attributes: u32,
        tag: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut c_void,
            class: i32,
            information: *mut c_void,
            size: u32,
        ) -> i32;
    }
    let mut value = AttributeTag {
        attributes: 0,
        tag: 0,
    };
    // FileAttributeTagInfo is read from the retained handle, not a path that
    // could now name something else. Cloud reparse tags do not substitute names.
    if unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle(),
            9,
            (&mut value as *mut AttributeTag).cast(),
            std::mem::size_of::<AttributeTag>() as u32,
        )
    } == 0
    {
        return Err(Error::new(
            "folder_io",
            format!(
                "Cannot verify directory reparse tag: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    Ok(name_surrogate_tag(value.tag))
}

#[cfg(windows)]
fn path_from_directory_handle(directory: &std::fs::File) -> Result<PathBuf> {
    use std::{
        ffi::{OsString, c_void},
        os::windows::{ffi::OsStringExt, io::AsRawHandle},
    };
    // Derive authority from the retained directory object, never a second
    // path lookup that could follow a replacement junction.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFinalPathNameByHandleW(
            file: *mut c_void,
            path: *mut u16,
            length: u32,
            flags: u32,
        ) -> u32;
    }
    let required =
        unsafe { GetFinalPathNameByHandleW(directory.as_raw_handle(), std::ptr::null_mut(), 0, 0) };
    if required == 0 {
        return Err(Error::new(
            "folder_io",
            std::io::Error::last_os_error().to_string(),
        ));
    }
    let mut value = vec![0u16; required as usize + 1];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            directory.as_raw_handle(),
            value.as_mut_ptr(),
            value.len() as u32,
            0,
        )
    };
    if length == 0 || length as usize >= value.len() {
        return Err(Error::new(
            "folder_io",
            "Directory path changed while capturing its native name",
        ));
    }
    Ok(OsString::from_wide(&value[..length as usize]).into())
}

#[cfg(all(test, windows))]
mod folder_admission_tests {
    use super::*;
    #[test]
    fn cloud_tags_do_not_redirect_names_but_symlinks_and_mount_points_do() {
        assert!(!name_surrogate_tag(0x9000_001a)); // IO_REPARSE_TAG_CLOUD
        assert!(!name_surrogate_tag(0x9000_101a)); // CLOUD_1
        assert!(name_surrogate_tag(0xa000_000c)); // SYMLINK
        assert!(name_surrogate_tag(0xa000_0003)); // MOUNT_POINT
    }
    #[test]
    fn root_swapped_to_junction_between_metadata_check_and_open_is_rejected() {
        let base = std::env::temp_dir().join(format!(
            "revenant-folder-swap-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let chosen = base.join("chosen");
        let moved = base.join("moved");
        let outside = base.join("outside");
        std::fs::create_dir_all(&chosen).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let result = NativeFolder::open_after_check(&chosen, || {
            std::fs::rename(&chosen, &moved).unwrap();
            assert!(
                std::process::Command::new("cmd.exe")
                    .args(["/C", "mklink", "/J"])
                    .arg(&chosen)
                    .arg(&outside)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        });
        std::fs::remove_dir(&chosen).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(
            result
                .err()
                .expect("swapped junction must not admit the outside target")
                .code,
            "invalid_folder"
        );
    }
}
