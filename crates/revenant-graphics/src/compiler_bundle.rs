//! Shared build-time verification, independent of GPU initialization.
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Cursor, Read},
    path::Path,
};

pub const ARCHIVE_LIMIT: usize = 48 * 1024 * 1024;
pub const ARCHIVE_SHA256: &str = "ad31b1fc8443175d204f77a611fdb3ef2ec42759bdc2f1167368de24a4a7e7f1";
pub const ARCHIVE_URL: &str = "https://github.com/microsoft/DirectXShaderCompiler/releases/download/v1.9.2609/dxc_2026_09_29.zip";

pub struct Artifact<'a> {
    pub archive_path: &'a str,
    pub file_name: &'a str,
    pub sha256: &'a str,
    pub max_bytes: usize,
}

pub const DLLS: &[Artifact<'static>] = &[
    Artifact {
        archive_path: "bin\\x64\\dxcompiler.dll",
        file_name: "dxcompiler.dll",
        sha256: "0689e2fee0a1c9df8662a59680469f24e2a3ad7ce6bbbcc9284f7a83172bfc59",
        max_bytes: 22_274_872,
    },
    Artifact {
        archive_path: "bin\\x64\\dxil.dll",
        file_name: "dxil.dll",
        sha256: "64be2368211d257a038a7d4ea6bb3e24c878eb8d588c7b8414fccf759b69c2ac",
        max_bytes: 1_995_584,
    },
];

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub fn checksum(bytes: &[u8], expected: &str) -> bool {
    format!("{:x}", Sha256::digest(bytes)) == expected
}

/// Verifies the archive and every requested entry before writing any output.
/// Unrequested paths are never extracted; names come from the compiled allowlist.
pub fn extract_verified(
    bytes: &[u8],
    digest: &str,
    entries: &[Artifact<'_>],
    out: &Path,
) -> io::Result<()> {
    if bytes.len() > ARCHIVE_LIMIT {
        return Err(invalid("DXC archive size limit exceeded"));
    }
    if !checksum(bytes, digest) {
        return Err(invalid("DXC archive checksum mismatch"));
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut verified = Vec::with_capacity(entries.len());
    for spec in entries {
        if Path::new(spec.file_name).components().count() != 1
            || spec.file_name.contains(['/', '\\'])
        {
            return Err(invalid("Invalid bundled artifact output name"));
        }
        let entry = zip.by_name(spec.archive_path)?;
        if entry.size() > spec.max_bytes as u64 {
            return Err(invalid(format!("{} size limit exceeded", spec.file_name)));
        }
        let mut payload = Vec::with_capacity(entry.size() as usize);
        entry
            .take(spec.max_bytes as u64 + 1)
            .read_to_end(&mut payload)?;
        if payload.len() > spec.max_bytes {
            return Err(invalid(format!("{} size limit exceeded", spec.file_name)));
        }
        if !checksum(&payload, spec.sha256) {
            return Err(invalid(format!("{} checksum mismatch", spec.file_name)));
        }
        verified.push((spec.file_name, payload));
    }
    fs::create_dir_all(out)?;
    for (name, payload) in verified {
        fs::write(out.join(name), payload)?;
    }
    Ok(())
}
