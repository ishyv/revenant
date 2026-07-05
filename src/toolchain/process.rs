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
        let mut cmd = Command::new(program);
        // DESIGN: put the child in its own process group so terminate_with_timeout
        // can signal the whole tree (e.g. `npm run dev`'s vite/node grandchildren)
        // via killpg instead of leaving them orphaned when only the direct child
        // (npm) is signaled.
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        cmd
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
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
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
        let stdout_thread = stdout.map(|out| {
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
        let stderr_thread = stderr.map(|err| {
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
            stdout_thread,
            stderr_thread,
        })
    }

    /// Check if the process has exited (non-blocking).
    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        self.child
            .try_wait()
            .context("failed to check process status")
    }

    /// Terminate the process (and its children) gracefully, escalating to a
    /// forceful kill after timeout.
    pub fn terminate_with_timeout(&mut self) {
        // On Windows, `self.child` is the `cmd /C` wrapper (see `make_command`),
        // so killing it directly leaves the real program (npm/vite/node) running.
        // `taskkill /T` kills the whole process tree instead.
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &self.child.id().to_string(), "/T", "/F"])
                .output();
        }

        // On Unix, the child was placed in its own process group (see
        // `make_command`), so signaling the group reaches grandchildren
        // (e.g. vite/node forked by npm) too, not just the direct child.
        #[cfg(unix)]
        {
            use nix::sys::signal::{self, Signal};
            use nix::unistd::Pid;

            let pgid = Pid::from_raw(self.child.id() as i32);
            let _ = signal::killpg(pgid, Signal::SIGTERM);
        }

        // Wait up to SHUTDOWN_TIMEOUT_SECS for the process to exit
        let deadline = std::time::Instant::now() + Duration::from_secs(SHUTDOWN_TIMEOUT_SECS);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        #[cfg(unix)]
                        {
                            use nix::sys::signal::{self, Signal};
                            use nix::unistd::Pid;
                            let pgid = Pid::from_raw(self.child.id() as i32);
                            let _ = signal::killpg(pgid, Signal::SIGKILL);
                        }
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(_) => break,
            }
        }

        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ManagedProcess {
    /// Structural safety net: if a `ManagedProcess` is ever dropped without an
    /// explicit `terminate_with_timeout()` call (e.g. a future early-return
    /// bug), don't leak the child process instead of silently orphaning it.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.terminate_with_timeout();
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// True if `pid` is running and not a zombie. `kill(pid, 0)` alone isn't
    /// enough on Linux: a terminated-but-not-yet-reaped process still holds its
    /// pid slot and answers signal 0 until its (possibly reparented) parent
    /// reaps it, which can lag briefly — check `/proc` state to look past that.
    fn process_alive(pid: i32) -> bool {
        use nix::sys::signal::kill;
        use nix::unistd::Pid;
        if kill(Pid::from_raw(pid), None).is_err() {
            return false;
        }
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .is_some_and(|state| state != "Z"),
            Err(_) => true,
        }
    }

    /// Regression test for the orphaned-grandchild bug: `npm run dev` forking
    /// `vite`/`node` as a grandchild used to survive `terminate_with_timeout`,
    /// since only the direct child was signaled. The process-group fix in
    /// `make_command`/`terminate_with_timeout` should reap the whole tree.
    #[test]
    fn terminate_with_timeout_kills_grandchildren() {
        let (tx, _rx) = mpsc::channel();
        let pid_file =
            std::env::temp_dir().join(format!("revenant_test_grandchild_{}", std::process::id()));
        let script = format!("sleep 5 & echo $! > {} ; wait", pid_file.display());

        let mut proc =
            ManagedProcess::spawn(Path::new("."), "sh", &["-c", &script], "test", tx).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut grandchild_pid = None;
        while std::time::Instant::now() < deadline {
            if let Ok(contents) = std::fs::read_to_string(&pid_file)
                && let Ok(pid) = contents.trim().parse::<i32>()
            {
                grandchild_pid = Some(pid);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let grandchild_pid = grandchild_pid.expect("grandchild pid should have been written");
        assert!(
            process_alive(grandchild_pid),
            "grandchild should be running before termination"
        );

        proc.terminate_with_timeout();
        thread::sleep(Duration::from_millis(500));

        assert!(
            !process_alive(grandchild_pid),
            "grandchild should be dead after terminate_with_timeout"
        );

        let _ = std::fs::remove_file(&pid_file);
    }
}
