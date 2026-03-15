use anyhow::{Context, Result};

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::config::SHUTDOWN_TIMEOUT_SECS;

/// Build a `Command` for `program`, routing through `cmd /C` on Windows so
/// that `.cmd` scripts (npm, npx, etc.) are resolved correctly.
// NOTE: On Windows, tools installed via npm ship as .cmd batch scripts that
// cannot be spawned directly — they must go through cmd.exe.
fn make_command(program: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", program]);
        cmd
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

/// A line of output from a managed process, tagged with its source.
#[derive(Debug)]
pub struct LogLine {
    pub prefix: String,
    pub line: String,
    pub is_stderr: bool,
}

/// A child process with I/O pump threads that forward stdout/stderr to a channel.
pub struct ManagedProcess {
    child: Child,
    _stdout_thread: Option<thread::JoinHandle<()>>,
    _stderr_thread: Option<thread::JoinHandle<()>>,
}

impl ManagedProcess {
    /// Spawn a process with the given command and args in `working_dir`.
    /// Output lines are sent to `log_tx` with the given `prefix`.
    pub fn spawn(
        working_dir: &Path,
        program: &str,
        args: &[&str],
        prefix: &str,
        log_tx: mpsc::Sender<LogLine>,
    ) -> Result<Self> {
        let mut child = make_command(program)
            .args(args)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn {program}"))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let prefix_owned = prefix.to_string();
        let tx1 = log_tx.clone();
        let _stdout_thread = stdout.map(|out| {
            let prefix = prefix_owned.clone();
            thread::spawn(move || {
                let reader = BufReader::new(out);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let _ = tx1.send(LogLine {
                        prefix: prefix.clone(),
                        line,
                        is_stderr: false,
                    });
                }
            })
        });

        let tx2 = log_tx;
        let _stderr_thread = stderr.map(|err| {
            let prefix = prefix_owned;
            thread::spawn(move || {
                let reader = BufReader::new(err);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let _ = tx2.send(LogLine {
                        prefix: prefix.clone(),
                        line,
                        is_stderr: true,
                    });
                }
            })
        });

        Ok(Self {
            child,
            _stdout_thread,
            _stderr_thread,
        })
    }

    /// Check if the process has exited (non-blocking).
    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        self.child
            .try_wait()
            .context("failed to check process status")
    }

    /// Terminate the process gracefully, escalating to kill after timeout.
    pub fn terminate_with_timeout(&mut self) {
        // On Windows, there is no SIGTERM — just kill
        #[cfg(windows)]
        {
            let _ = self.child.kill();
        }

        // On Unix, send SIGTERM first, then SIGKILL after timeout
        #[cfg(unix)]
        {
            use nix::sys::signal::{self, Signal};
            use nix::unistd::Pid;

            let pid = Pid::from_raw(self.child.id() as i32);
            let _ = signal::kill(pid, Signal::SIGTERM);
        }

        // Wait up to SHUTDOWN_TIMEOUT_SECS for the process to exit
        let deadline = std::time::Instant::now() + Duration::from_secs(SHUTDOWN_TIMEOUT_SECS);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(_) => return,
            }
        }
    }
}

/// Run a process synchronously (blocking).
///
/// When `verbose` is true, stdout/stderr are inherited (visible to user).
/// When false, output is captured and only stderr is included in error messages.
pub fn run_blocking(working_dir: &Path, program: &str, args: &[&str], verbose: bool) -> Result<()> {
    if verbose {
        let status = make_command(program)
            .args(args)
            .current_dir(working_dir)
            .status()
            .with_context(|| format!("failed to run {program}"))?;

        if status.success() {
            Ok(())
        } else {
            anyhow::bail!(
                "{} exited with code {}",
                program,
                status.code().unwrap_or(-1)
            )
        }
    } else {
        let output = make_command(program)
            .args(args)
            .current_dir(working_dir)
            .output()
            .with_context(|| format!("failed to run {program}"))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr_trimmed = stderr.trim();
            if stderr_trimmed.is_empty() {
                anyhow::bail!(
                    "{} exited with code {}. Run with --verbose for full output",
                    program,
                    output.status.code().unwrap_or(-1)
                )
            } else {
                anyhow::bail!(
                    "{} exited with code {}:\n{}",
                    program,
                    output.status.code().unwrap_or(-1),
                    stderr_trimmed
                )
            }
        }
    }
}
