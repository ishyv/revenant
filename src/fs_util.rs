use std::{
    fs,
    io,
    path::{Path, PathBuf},
    time::Duration,
    thread,
};
use walkdir::WalkDir;


/// Recursively copies the *contents* of `source` into `target`.
/// - Creates `target` and any intermediate directories if they don't exist.
/// - Overwrites existing files in `target`.
/// - Preserves directory structure relative to `source`.
/// - Skips special files (e.g., sockets, block devices).
/// - Symlinks: detected via `symlink_metadata`. To keep behavior safe and portable,
///   this function **does not** recreate symlinks. It skips them to avoid cycles and
///   platform-specific privilege requirements (especially on Windows).
///
/// Returns `Ok(())` if all entries copy successfully, or the first encountered `io::Error`.
pub fn copy_dir<S: AsRef<Path>, T: AsRef<Path>>(source: S, target: T) -> io::Result<()> {
    let source = source.as_ref();
    let target = target.as_ref();

    // Validate source.
    let src_meta = fs::metadata(source)?;
    if !src_meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("source is not a directory: {}", source.display()),
        ));
    }

    // Ensure target exists.
    fs::create_dir_all(target)?;

    // Depth-first traversal using a small stack to avoid recursion limits.
    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(source.to_path_buf(), target.to_path_buf())];

    while let Some((src_dir, dst_dir)) = stack.pop() {
        // Read directory entries.
        for entry_res in fs::read_dir(&src_dir)? {
            let entry = entry_res?;
            let src_path = entry.path();
            let file_name = entry
                .file_name(); // keep original name exactly
            let dst_path = dst_dir.join(file_name);

            // Use symlink_metadata to see the *link* itself (not its target).
            let meta = fs::symlink_metadata(&src_path)?;

            if meta.is_dir() {
                // Recreate directory and push to stack.
                fs::create_dir_all(&dst_path)?;
                stack.push((src_path, dst_path));
            } else if meta.is_file() {
                // Ensure parent exists (defensive; should already exist).
                if let Some(parent) = dst_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                // Copy file, overwriting if it already exists.
                // `fs::copy` truncates/overwrites the destination file if present.
                fs::copy(&src_path, &dst_path).map(|_bytes| ())?;
            } else if meta.file_type().is_symlink() {
                // Conservative choice: skip symlinks to avoid infinite loops and
                // platform-specific linking privileges. If you need to *recreate*
                // symlinks or *dereference* them, implement that policy explicitly.
                eprintln!(
                    "[ i ] Skipping symbolic link: {}",
                    src_path.display()
                );
                continue;
            } else {
                // Skip other special file types silently (or log if you care).
                eprintln!(
                    "[ i ] Skipping special file: {}",
                    src_path.display()
                );
                continue;
            }
        }
    }

    Ok(())
}



pub fn force_delete_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();

    // Nothing to do.
    if !path.exists() {
        return Ok(());
    }

    // Deleting the directory that contains our current working directory will fail on Windows.
    // If we're inside, step out to its parent (or project root) first.
    let cwd = std::env::current_dir()?;
    if cwd.starts_with(path) {
        let fall_back = path.parent().unwrap_or_else(|| Path::new("."));
        std::env::set_current_dir(fall_back)?;
    }

    // Windows: clear read-only attributes recursively so deletion won't choke.
    #[cfg(windows)]
    {
        for entry in WalkDir::new(path).into_iter().filter_map(Result::ok) {
            let p = entry.path();
            // Clear read-only on files AND directories.
            // This uses std permissions; no WinAPI dance needed.
            if let Ok(meta) = fs::metadata(p) {
                let mut perms = meta.permissions();
                if perms.readonly() {
                    perms.set_readonly(false);
                    // Best-effort; ignore errors here, deletion may still succeed.
                    let _ = fs::set_permissions(p, perms);
                }
            }
        }
    }

    // Try to delete contents bottom-up, then remove the root.
    // Doing it manually gives clearer failure points than one big remove_dir_all.
    for entry in WalkDir::new(path).contents_first(true) {
        let entry = entry?;
        let p = entry.path();

        // Skip the root; we'll remove it after its children.
        if p == path { continue; }

        if entry.file_type().is_file() {
            // Best-effort remove; if it fails with PermissionDenied, a retry after a short sleep
            // often works due to transient file locks (watchers/antivirus).
            match fs::remove_file(p) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    thread::sleep(Duration::from_millis(50));
                    fs::remove_file(p)?;
                }
                Err(e) => return Err(e),
            }
        } else if entry.file_type().is_dir() {
            // Remove empty dir (children are already gone due to contents_first).
            match fs::remove_dir(p) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    thread::sleep(Duration::from_millis(50));
                    fs::remove_dir(p)?;
                }
                Err(e) => return Err(e),
            }
        } else {
            // Symlinks/special: try remove_file; if it fails, ignore or log.
            let _ = fs::remove_file(p);
        }
    }

    // Finally remove the root directory itself.
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            // One more nudge; Windows sometimes releases locks a hair late.
            thread::sleep(Duration::from_millis(50));
            fs::remove_dir(path)
        }
        Err(e) => Err(e),
    }
}