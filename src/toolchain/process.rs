//! Child process ownership shared by build, export, dev, and packaging stages.
//!
//! Windows children start suspended until assigned to a kill-on-close job. Unix
//! children own process groups. Every exit path reaps descendants and pipe pumps.
use anyhow::{Context, Result};
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use crate::config::SHUTDOWN_TIMEOUT_SECS;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static SIGNAL_HANDLER: OnceLock<std::result::Result<(), String>> = OnceLock::new();

pub fn install_signal_handler() -> Result<()> {
    let result = SIGNAL_HANDLER.get_or_init(|| {
        ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst)).map_err(|e| e.to_string())
    });
    result
        .as_ref()
        .map_err(|e| anyhow::anyhow!("install interrupt handler: {e}"))?;
    Ok(())
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

pub fn resolve_program(working_dir: &Path, program: &str) -> PathBuf {
    let _ = working_dir;
    PathBuf::from(program)
}

fn make_command(working_dir: &Path, program: &str) -> Command {
    let resolved = resolve_program(working_dir, program);
    #[cfg(windows)]
    let mut cmd = if matches!(program, "npm" | "pnpm" | "yarn" | "npx") {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/d", "/c", program]);
        cmd
    } else {
        Command::new(resolved)
    };
    #[cfg(not(windows))]
    let mut cmd = Command::new(resolved);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Suspend until job assignment closes the spawn/cleanup race.
        cmd.creation_flags(0x08000000 | 0x00000004);
    }
    cmd.current_dir(working_dir);
    cmd
}

#[derive(Debug)]
pub struct LogLine {
    pub prefix: String,
    pub line: String,
    pub is_stderr: bool,
}

#[cfg(windows)]
struct ProcessTree(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProcessTree {
    fn attach(child: &Child) -> Result<Self> {
        use std::{
            mem::{size_of, zeroed},
            os::windows::io::AsRawHandle,
        };
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            anyhow::ensure!(
                !handle.is_null(),
                "create process job: {}",
                std::io::Error::last_os_error()
            );
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            anyhow::ensure!(
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32
                ) != 0,
                "configure process job: {}",
                std::io::Error::last_os_error()
            );
            anyhow::ensure!(
                AssignProcessToJobObject(handle, child.as_raw_handle() as _) != 0,
                "attach process job: {}",
                std::io::Error::last_os_error()
            );
            resume_initial_thread(child.id())?;
            Ok(job)
        }
    }
    fn kill(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
    fn wait_for_exit(&self) -> Result<()> {
        use windows_sys::Win32::System::JobObjects::*;
        let deadline = std::time::Instant::now() + Duration::from_secs(SHUTDOWN_TIMEOUT_SECS);
        loop {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            let queried = unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut _,
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                )
            };
            anyhow::ensure!(
                queried != 0,
                "query process job: {}",
                std::io::Error::last_os_error()
            );
            if info.ActiveProcesses == 0 {
                return Ok(());
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "owned process job did not terminate within {SHUTDOWN_TIMEOUT_SECS} seconds"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(windows)]
unsafe fn resume_initial_thread(process_id: u32) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::*,
        System::{Diagnostics::ToolHelp::*, Threading::*},
    };
    struct HandleGuard(HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        anyhow::ensure!(
            snapshot != INVALID_HANDLE_VALUE,
            "snapshot child threads: {}",
            std::io::Error::last_os_error()
        );
        let _snapshot = HandleGuard(snapshot);
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut present = Thread32First(snapshot, &mut entry);
        while present != 0 {
            if entry.th32OwnerProcessID == process_id {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                anyhow::ensure!(
                    !thread.is_null(),
                    "open child thread: {}",
                    std::io::Error::last_os_error()
                );
                let _thread = HandleGuard(thread);
                anyhow::ensure!(
                    ResumeThread(thread) != u32::MAX,
                    "resume child thread: {}",
                    std::io::Error::last_os_error()
                );
                return Ok(());
            }
            present = Thread32Next(snapshot, &mut entry);
        }
    }
    anyhow::bail!("suspended child thread is absent")
}
#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// Owns the complete child process tree, including cleanup on every error path.
pub struct ManagedProcess {
    child: Child,
    threads: Vec<thread::JoinHandle<()>>,
    stopped: bool,
    #[cfg(windows)]
    tree: ProcessTree,
}

impl ManagedProcess {
    pub fn spawn(
        working_dir: &Path,
        program: &str,
        args: &[&str],
        prefix: &str,
        log_tx: mpsc::Sender<LogLine>,
    ) -> Result<Self> {
        Self::spawn_with_env(working_dir, program, args, &[], prefix, log_tx)
    }

    pub fn spawn_with_env(
        working_dir: &Path,
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
        prefix: &str,
        log_tx: mpsc::Sender<LogLine>,
    ) -> Result<Self> {
        install_signal_handler()?;
        anyhow::ensure!(!interrupted(), "interrupted");
        let mut child = make_command(working_dir, program)
            .args(args)
            .envs(env.iter().copied())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn {program}"))?;
        #[cfg(windows)]
        let tree = match ProcessTree::attach(&child) {
            Ok(tree) => tree,
            Err(error) => {
                let _ = Command::new("taskkill.exe")
                    .args(["/PID", &child.id().to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let mut threads = Vec::new();
        fn pump<R: std::io::Read + Send + 'static>(
            reader: R,
            prefix: String,
            is_stderr: bool,
            tx: mpsc::Sender<LogLine>,
        ) -> thread::JoinHandle<()> {
            thread::spawn(move || {
                for line in BufReader::new(reader).lines() {
                    let Ok(line) = line else { break };
                    if tx
                        .send(LogLine {
                            prefix: prefix.clone(),
                            line,
                            is_stderr,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
        }
        if let Some(out) = child.stdout.take() {
            threads.push(pump(out, prefix.into(), false, log_tx.clone()));
        }
        if let Some(err) = child.stderr.take() {
            threads.push(pump(err, prefix.into(), true, log_tx));
        }
        Ok(Self {
            child,
            threads,
            stopped: false,
            #[cfg(windows)]
            tree,
        })
    }

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child.try_wait().context("check child status")
    }

    pub fn terminate_with_timeout(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        #[cfg(windows)]
        self.tree.kill();
        #[cfg(unix)]
        {
            use nix::{
                sys::signal::{Signal, killpg},
                unistd::Pid,
            };
            let group = Pid::from_raw(self.child.id() as i32);
            let _ = killpg(group, Signal::SIGTERM);
            let deadline = std::time::Instant::now() + Duration::from_secs(SHUTDOWN_TIMEOUT_SECS);
            while std::time::Instant::now() < deadline {
                let _ = self.child.try_wait();
                if killpg(group, None).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            let _ = killpg(group, Signal::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        #[cfg(windows)]
        if let Err(error) = self.tree.wait_for_exit() {
            // Parent exit alone is insufficient: a descendant with redirected
            // pipes can still hold a shadow executable mapped into memory.
            eprintln!("Owned process-tree cleanup incomplete: {error:#}");
        }
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        self.terminate_with_timeout();
    }
}

pub fn run_capture(
    working_dir: &Path,
    program: &str,
    args: &[&str],
    verbose: bool,
) -> Result<String> {
    run_capture_with_env(working_dir, program, args, &[], verbose)
}

/// Capture stdout separately from stderr under the same descendant guarantees.
pub fn run_capture_with_env(
    working_dir: &Path,
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
    verbose: bool,
) -> Result<String> {
    let (tx, rx) = mpsc::channel();
    let mut child = ManagedProcess::spawn_with_env(working_dir, program, args, env, program, tx)?;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut collect = |log: LogLine| {
        if verbose {
            if log.is_stderr {
                eprintln!("{}", log.line);
            } else {
                println!("{}", log.line);
            }
        }
        let buffer = if log.is_stderr {
            &mut stderr
        } else {
            &mut stdout
        };
        buffer.push_str(&log.line);
        buffer.push('\n');
    };
    let status = loop {
        while let Ok(log) = rx.try_recv() {
            collect(log);
        }
        anyhow::ensure!(!interrupted(), "{program} interrupted");
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(30));
    };
    child.terminate_with_timeout();
    while let Ok(log) = rx.try_recv() {
        collect(log);
    }
    anyhow::ensure!(
        status.success(),
        "{program} exited with {status}:\n{stderr}{stdout}"
    );
    Ok(stdout)
}

pub fn run_blocking(working_dir: &Path, program: &str, args: &[&str], verbose: bool) -> Result<()> {
    run_capture(working_dir, program, args, verbose).map(|_| ())
}
