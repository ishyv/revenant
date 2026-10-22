//! Windows current-user SID ACLs. Never accept remote named-pipe clients.
use super::*;
use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, LocalFree},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        DACL_SECURITY_INFORMATION, GetTokenInformation, SECURITY_ATTRIBUTES, SetFileSecurityW,
        TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

fn last_error() -> Error {
    io_error(std::io::Error::last_os_error())
}
pub fn user_sid() -> Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: output pointers are valid; the token is closed on every path.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error());
    }
    let result = (|| {
        let mut length = 0;
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut length);
        }
        let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(last_error());
        }
        // SAFETY: Windows initialized aligned TOKEN_USER followed by its SID.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut string = ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut string) } == 0 {
            return Err(last_error());
        }
        let mut length = 0;
        unsafe {
            while *string.add(length) != 0 {
                length += 1;
            }
        }
        let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(string, length) });
        unsafe {
            LocalFree(string.cast());
        }
        Ok(sid)
    })();
    unsafe {
        CloseHandle(token);
    }
    result
}
struct Descriptor(*mut c_void);
impl Descriptor {
    fn new(inherit: bool) -> Result<Self> {
        let flags = if inherit { "OICI" } else { "" };
        let sddl: Vec<u16> = format!("D:P(A;{flags};GA;;;{})", user_sid()?)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: nul-terminated SDDL and valid output pointer; LocalFree in Drop.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(Self(descriptor))
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn protect(path: &Path, inherit: bool) -> Result<()> {
    let descriptor = Descriptor::new(inherit)?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: descriptor and nul-terminated path remain alive through the call.
    if unsafe {
        SetFileSecurityW(
            wide.as_ptr(),
            DACL_SECURITY_INFORMATION | 0x8000_0000,
            descriptor.0,
        )
    } == 0
    {
        return Err(last_error());
    }
    Ok(())
}
pub fn directory(path: &Path) -> Result<()> {
    no_reparse_points(path)?;
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
    std::fs::create_dir_all(path).map_err(io_error)?;
    protect(path, true)
}
pub fn file(path: &Path) -> Result<()> {
    protect(path, false)
}
fn no_reparse_points(path: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    for ancestor in path.ancestors() {
        if let Ok(metadata) = std::fs::symlink_metadata(ancestor)
            && metadata.file_attributes() & 0x400 != 0
        {
            return Err(Error::new(
                "bridge_directory",
                "Discovery paths cannot contain reparse points",
            ));
        }
    }
    Ok(())
}
fn sid_string(sid: *mut c_void) -> Result<String> {
    let mut string = ptr::null_mut();
    // SAFETY: caller supplies a SID in the live Windows security descriptor.
    if unsafe { ConvertSidToStringSidW(sid, &mut string) } == 0 {
        return Err(last_error());
    }
    let mut length = 0;
    unsafe {
        while *string.add(length) != 0 {
            length += 1;
        }
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(string, length) });
    unsafe {
        LocalFree(string.cast());
    }
    Ok(value)
}
pub fn validate(path: &Path, directory: bool) -> Result<()> {
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER,
        Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT},
        GetAce, GetSecurityDescriptorControl, OWNER_SECURITY_INFORMATION,
    };
    no_reparse_points(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(io_error)?;
    if directory && !metadata.is_dir() {
        return Err(Error::new(
            "bridge_directory",
            "Discovery path is not a directory",
        ));
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut owner = ptr::null_mut();
    let mut acl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: valid nul-terminated path and output pointers. Descriptor is freed below.
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut acl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io_error(std::io::Error::from_raw_os_error(status as i32)));
    }
    let descriptor = Descriptor(descriptor);
    let current = user_sid()?;
    let owner = sid_string(owner)?;
    if ![current.as_str(), "S-1-5-18", "S-1-5-32-544"].contains(&owner.as_str()) || acl.is_null() {
        return Err(Error::new(
            "bridge_identity",
            "Discovery owner or permissions are not trusted",
        ));
    }
    let mut control = 0;
    let mut revision = 0;
    if unsafe { GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) } == 0 {
        return Err(last_error());
    }
    if control & 0x1000 == 0 || unsafe { (*acl).AceCount } != 1 {
        return Err(Error::new(
            "bridge_identity",
            "Discovery ACL must be protected for the current user only",
        ));
    }
    let mut ace = ptr::null_mut();
    if unsafe { GetAce(acl, 0, &mut ace) } == 0 {
        return Err(last_error());
    }
    if unsafe { (*ace.cast::<ACE_HEADER>()).AceType } != 0 {
        return Err(Error::new(
            "bridge_identity",
            "Unexpected discovery access rule",
        ));
    }
    let sid = unsafe { ptr::addr_of_mut!((*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart).cast() };
    if sid_string(sid)? != current {
        return Err(Error::new(
            "bridge_identity",
            "Discovery access belongs to another user",
        ));
    }
    Ok(())
}
pub fn pipe(
    endpoint: &str,
    first: bool,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    let descriptor = Descriptor::new(false)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true);
    // SAFETY: Windows copies descriptor data before returning the Tokio handle.
    unsafe {
        options.create_with_security_attributes_raw(
            endpoint,
            (&attributes as *const SECURITY_ATTRIBUTES)
                .cast_mut()
                .cast(),
        )
    }
    .map_err(io_error)
}
