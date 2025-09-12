//! Shell command helpers and a small `cmd!` macro.
//!
//! These utilities centralize how external commands are executed so that
//! logging, error handling, and platform differences are consistently handled.

use std::io;
use std::process::{Command, Stdio};

/// Runs a shell command and returns its stdout as a `String`.
///
/// - Uses `cmd /C` on Windows and `sh -c` elsewhere.
/// - Captures both stdout and stderr in a single run (no double-spawn).
/// - On nonzero exit, returns an error that includes the process status and stderr.
///
/// Returns: `Ok(stdout)` on success; `Err(io::Error)` on failure.
pub fn run(line: &str) -> io::Result<String> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", line]);
        c
    };

    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.args(["-c", line]);
        c
    };

    // Capture both stdout and stderr in one go.
    let out = cmd
        .stdin(Stdio::null())         // or inherit if you need input
        .stdout(Stdio::piped())       // CAPTURE stdout
        .stderr(Stdio::piped())       // CAPTURE stderr (optional)
        .output()?;                   // <-- run ONCE and collect

    if out.status.success() {
        // trim to drop the trailing newline most CLIs print
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        // include stderr in the error so you actually see why it failed
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "command failed: {}\nstderr: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            ),
        ))
    }
}


#[macro_export]
/// * Utility macro to run shell commands, exiting on failure.
/// - Usage (single): `cmd!("echo hello") -> String`
/// - Usage (multiple): `cmd!("echo a", "echo b") -> Vec<String>`
///
/// In debug builds, prints each command and its captured output to stdout.
macro_rules! cmd {
    // Single command
    ($raw:expr) => {
        match crate::commands::run($raw) {
            Ok(output) => {
                // If on debug show the output
                #[cfg(debug_assertions)]
                println!("Command '{}' output: {}", $raw, output);
                output

            } Err(e) => {
                eprintln!("Error running command '{}': {}", $raw, e);
                std::process::exit(1);
            }
        }
    };

    // Multiple commands -> Vec<String>
    ($($raw:expr),+ $(,)?) => {{
        let mut outputs = ::std::vec::Vec::new();
        $(
            match crate::commands::run($raw) {
                Ok(out) => {
                    #[cfg(debug_assertions)]
                    println!("Command '{}' output: {}", $raw, out);
                    outputs.push(out);
                }
                Err(e) => {
                    eprintln!("Error running command '{}': {}", $raw, e);
                    std::process::exit(1);
                }
            }
        )+
        outputs
    }};
}

            
