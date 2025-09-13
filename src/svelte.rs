//! Svelte project orchestration helpers.
//!
//! Centralizes process management and project mutations so the CLI stays lean.

use std::io;
use std::path::PathBuf;

use crate::data::AppContext;

/// Ensures the Svelte project folder exists.
pub fn ensure_svelte_exists(ctx: &AppContext) -> io::Result<()> {
    if !ctx.svelte_path.exists() {
        Err(
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Svelte project not found at {}. Run 'revenant full' first.",
                    ctx.svelte_path.display()
                )
            )
        )
    } else {
        Ok(())
    }
}

/// Best-effort resolution of the `revenantc` compiler binary relative to the main binary.
pub fn resolve_revenantc_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = Vec::new();
    if cfg!(windows) {
        candidates.push(dir.join("revenantc.exe"));
        candidates.push(dir.join("..\\..\\debug\\revenantc.exe"));
    } else {
        candidates.push(dir.join("revenantc"));
        candidates.push(dir.join("../../debug/revenantc"));
    }
    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }
    None
}

/// Runs `npm run <script>` and streams stdout/stderr to the console.
/// Automatically wires Revenant preprocessor env vars.
pub fn run_npm_script_streaming(
    ctx: &AppContext,
    script: &str,
    extra_args: &[String]
) -> io::Result<()> {
    let mut cmd = std::process::Command::new("npm");
    cmd.arg("run").arg(script);
    if !extra_args.is_empty() {
        cmd.arg("--");
        for a in extra_args {
            cmd.arg(a);
        }
    }
    cmd.current_dir(&ctx.svelte_path)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());

    // Preprocessor wiring
    cmd.env("REVENANT_PREPROCESS", "1");
    if let Some(bin) = resolve_revenantc_path() {
        cmd.env("REVENANTC_BIN", bin);
    }

    let mut child = cmd.spawn()?;
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Other, format!("npm run {} failed: {}", script, status)))
    }
}

/// Runs `npm run <script>` and returns captured stdout; errors include stderr.
pub fn run_npm_script_capture(
    ctx: &AppContext,
    script: &str,
    extra_args: &[String]
) -> io::Result<String> {
    let mut cmd = std::process::Command::new("npm");
    cmd.arg("run").arg(script);
    if !extra_args.is_empty() {
        cmd.arg("--");
        for a in extra_args {
            cmd.arg(a);
        }
    }
    cmd.current_dir(&ctx.svelte_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Preprocessor wiring
    cmd.env("REVENANT_PREPROCESS", "1");
    if let Some(bin) = resolve_revenantc_path() {
        cmd.env("REVENANTC_BIN", bin);
    }

    let out = cmd.output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(
            io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "npm run {} failed: {}\nstderr: {}",
                    script,
                    out.status,
                    String::from_utf8_lossy(&out.stderr)
                )
            )
        )
    }
}

/// Generates a small TS module and ambient types to expose a global `Revevant` object.
pub fn inject_globals(ctx: &AppContext) -> io::Result<()> {
    use std::fs;

    let src_dir = ctx.svelte_path.join("src");
    let lib_dir = src_dir.join("lib");
    fs::create_dir_all(&lib_dir)?;

    let compiler_path = resolve_revenantc_path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| String::from("revenantc"));

    // Render global TS via a typed template
    let rendered = crate::templates::RevenantGlobal::render(
        env!("CARGO_PKG_VERSION"),
        &ctx.root_path.to_string_lossy(),
        &ctx.svelte_path.to_string_lossy(),
        &compiler_path,
    );
    fs::write(lib_dir.join("revenant.global.ts"), rendered)?;

    // Ambient typings and runtime global hookup from templates
    fs::write(src_dir.join("app.d.ts"), crate::templates::AppDTs::render())?;

    let hooks_client = crate::templates::HooksClientTs::render();
    let hooks_path = src_dir.join("hooks.client.ts");
    let need_write = match fs::read_to_string(&hooks_path) {
        Ok(existing) => existing != hooks_client,
        Err(_) => true,
    };
    if need_write {
        fs::write(hooks_path, hooks_client)?;
    }
    Ok(())
}
