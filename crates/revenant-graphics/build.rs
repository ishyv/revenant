//! Pins the Windows GPU compiler; ordinary applications do not fetch it.
#[cfg(feature = "gpu")]
#[path = "src/atlas_build.rs"]
mod atlas;
#[cfg(feature = "gpu")]
#[path = "src/compiler_bundle.rs"]
mod bundle;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/compiler_bundle.rs");
    println!("cargo:rerun-if-changed=src/atlas_build.rs");
    println!("cargo:rerun-if-changed=assets/AtkinsonHyperlegible-Regular.ttf");
    println!("cargo:rerun-if-env-changed=REVENANT_DXC_ARCHIVE");
    #[cfg(feature = "gpu")]
    atlas::prepare().expect("Cannot prepare fixed native font atlas");
    #[cfg(feature = "gpu")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        prepare().unwrap_or_else(|error| {
            panic!("Cannot prepare Revenant's bundled Windows shader compiler: {error}")
        });
    }
}

#[cfg(feature = "gpu")]
fn prepare() -> std::io::Result<()> {
    use std::{
        env, fs,
        io::{self, Read},
        path::PathBuf,
        process::Command,
    };
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86_64") {
        return Err(io::Error::other(
            "Native Windows graphics currently supports x86_64 only",
        ));
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR missing"));
    let archive = env::var_os("REVENANT_DXC_ARCHIVE")
        .map(PathBuf::from)
        .unwrap_or_else(|| out.join("dxc_2026_09_29.zip"));
    let read = |path: &std::path::Path| -> io::Result<Vec<u8>> {
        let file = fs::File::open(path)?;
        if file.metadata()?.len() > bundle::ARCHIVE_LIMIT as u64 {
            return Err(io::Error::other("DXC archive size limit exceeded"));
        }
        let mut bytes = Vec::new();
        file.take(bundle::ARCHIVE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    let cached = read(&archive)
        .ok()
        .filter(|bytes| bundle::checksum(bytes, bundle::ARCHIVE_SHA256));
    let bytes = if let Some(bytes) = cached {
        bytes
    } else {
        if env::var_os("REVENANT_DXC_ARCHIVE").is_some() {
            return Err(io::Error::other(
                "REVENANT_DXC_ARCHIVE must name the pinned archive with its correct SHA-256",
            ));
        }
        let download = out.join("dxc-download.zip");
        let status = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--tlsv1.2",
                "--retry",
                "2",
                "--connect-timeout",
                "20",
                "--max-time",
                "120",
                "--max-filesize",
                "50331648",
                "--output",
            ])
            .arg(&download)
            .arg(bundle::ARCHIVE_URL)
            .status()?;
        if !status.success() {
            return Err(io::Error::other(
                "Downloading pinned Microsoft DXC failed; for offline builds set REVENANT_DXC_ARCHIVE to the verified archive",
            ));
        }
        let bytes = read(&download)?;
        if !bundle::checksum(&bytes, bundle::ARCHIVE_SHA256) {
            return Err(io::Error::other("Downloaded DXC archive checksum mismatch"));
        }
        // The old cache may be corrupt. Replace only after validating its successor.
        if archive.exists() {
            fs::remove_file(&archive)?;
        }
        fs::rename(download, &archive)?;
        bytes
    };
    bundle::extract_verified(&bytes, bundle::ARCHIVE_SHA256, bundle::DLLS, &out)?;
    fs::write(
        out.join("dxc_manifest.rs"),
        format!(
            "const DXCOMPILER_SHA256: &str = {:?};\nconst DXIL_SHA256: &str = {:?};\n",
            bundle::DLLS[0].sha256,
            bundle::DLLS[1].sha256
        ),
    )?;
    Ok(())
}
