//! Validate and copy authored icons without retaining paths outside the app.
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

pub(super) fn resolve(root: &Path, paths: Option<&[PathBuf]>) -> Result<Vec<(String, Vec<u8>)>> {
    let Some(paths) = paths else {
        return Ok(Vec::new());
    };
    let canonical_root = dunce::canonicalize(root)?;
    let mut assets = Vec::new();
    for (index, relative) in paths.iter().enumerate() {
        ensure!(
            !relative.is_absolute(),
            "desktop.icons paths must be relative to the project root"
        );
        let path = dunce::canonicalize(root.join(relative))
            .with_context(|| format!("resolve icon {}", relative.display()))?;
        ensure!(
            path.starts_with(&canonical_root),
            "desktop.icons asset escapes the project root"
        );
        let bytes = std::fs::read(&path)?;
        let extension = relative
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let valid = match extension.as_str() {
            "png" => {
                bytes.len() >= 33
                    && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                    && &bytes[12..16] == b"IHDR"
                    && bytes[24] == 8
                    && bytes[25] == 6
                    && u32::from_be_bytes(bytes[16..20].try_into()?) > 0
                    && u32::from_be_bytes(bytes[20..24].try_into()?) > 0
            }
            "ico" => valid_ico(&bytes),
            "icns" => {
                bytes.len() >= 8
                    && &bytes[..4] == b"icns"
                    && u32::from_be_bytes(bytes[4..8].try_into()?) as usize == bytes.len()
            }
            _ => anyhow::bail!(
                "unsupported icon format: {} (use PNG, ICO or ICNS)",
                relative.display()
            ),
        };
        ensure!(
            valid,
            "invalid {extension} icon {} (PNG must be 8-bit RGBA)",
            relative.display()
        );
        assets.push((format!("icons/application-{index}.{extension}"), bytes));
    }
    // Tauri falls back to its conventional bundled files for missing icon
    // formats. An authored set must cover the window and platform resources.
    ensure!(
        assets.iter().any(|(name, _)| name.ends_with(".png")),
        "desktop.icons must include an 8-bit RGBA PNG for the window icon"
    );
    #[cfg(windows)]
    ensure!(
        assets.iter().any(|(name, _)| name.ends_with(".ico")),
        "desktop.icons must include an ICO for Windows executable and installer resources; generate it with `tauri icon`"
    );
    #[cfg(target_os = "macos")]
    ensure!(
        assets.iter().any(|(name, _)| name.ends_with(".icns")),
        "desktop.icons must include an ICNS for the macOS application bundle; generate it with `tauri icon`"
    );
    Ok(assets)
}

fn valid_ico(bytes: &[u8]) -> bool {
    if bytes.len() < 6 || bytes[..4] != [0, 0, 1, 0] {
        return false;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if count == 0 || bytes.len() < 6 + 16 * count {
        return false;
    }
    (0..count).all(|index| {
        let record = 6 + 16 * index;
        let length =
            u32::from_le_bytes(bytes[record + 8..record + 12].try_into().unwrap()) as usize;
        let offset =
            u32::from_le_bytes(bytes[record + 12..record + 16].try_into().unwrap()) as usize;
        length > 0
            && offset >= 6 + 16 * count
            && offset
                .checked_add(length)
                .is_some_and(|end| end <= bytes.len())
    })
}
