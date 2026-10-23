//! Exercise actual stdio pipes and runtime destruction, not only async duplex I/O.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

struct Mcp {
    child: Arc<Mutex<Child>>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    timed_out: Arc<AtomicBool>,
    stop: mpsc::Sender<()>,
    watchdog: Option<std::thread::JoinHandle<()>>,
    _project: Option<tempfile::TempDir>,
}
impl Mcp {
    fn start() -> Self {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("revenant.toml"),
            "version = 3\n[project]\nname = 'shutdown-probe'\n",
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_revenant"));
        command.args(["mcp", "--project"]).arg(project.path());
        Self::spawn(command, Duration::from_secs(10), Some(project))
    }
    fn spawn(mut command: Command, deadline: Duration, project: Option<tempfile::TempDir>) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let child = Arc::new(Mutex::new(child));
        let timed_out = Arc::new(AtomicBool::new(false));
        let (stop, stopped) = mpsc::channel();
        let owned_child = child.clone();
        let timeout_flag = timed_out.clone();
        // This thread can kill/reap even while the scenario is blocked writing
        // stdin or reading a handshake. No I/O operation holds the child lock.
        let watchdog = std::thread::spawn(move || {
            if matches!(
                stopped.recv_timeout(deadline),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                let mut child = owned_child.lock().unwrap();
                if child.try_wait().unwrap().is_none() {
                    timeout_flag.store(true, Ordering::SeqCst);
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        });
        Self {
            child,
            stdin,
            stdout,
            timed_out,
            stop,
            watchdog: Some(watchdog),
            _project: project,
        }
    }
    fn send(&mut self, request: Value) {
        writeln!(self.stdin.as_mut().unwrap(), "{request}").unwrap();
        self.stdin.as_mut().unwrap().flush().unwrap();
    }
    fn handshake(&mut self) -> BufReader<ChildStdout> {
        self.send(initialize(json!(1)));
        let mut output = BufReader::new(self.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert!(serde_json::from_str::<Value>(&line).unwrap()["result"].is_object());
        output
    }
    fn assert_exited(&mut self, expected_code: i32) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.lock().unwrap().try_wait().unwrap() {
                assert!(
                    !self.timed_out.load(Ordering::SeqCst),
                    "whole-scenario MCP watchdog expired"
                );
                assert_eq!(
                    status.code(),
                    Some(expected_code),
                    "unexpected MCP exit: {status}"
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "MCP process failed to exit with its stdout pipe still unread"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        {
            let mut child = self.child.lock().unwrap();
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(watchdog) = self.watchdog.take() {
            watchdog.join().unwrap();
        }
    }
}
fn initialize(id: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{
        "protocolVersion":"2025-03-26","capabilities":{},
        "clientInfo":{"name":"shutdown-probe","version":"1"}
    }})
}
fn large_id() -> Value {
    // A valid frame below 1 MiB, with a response larger than ordinary OS pipes.
    Value::String("x".repeat(512 * 1024))
}

#[test]
fn eof_during_blocked_initialization_exits_without_reading_stdout() {
    let mut mcp = Mcp::start();
    mcp.send(initialize(large_id()));
    drop(mcp.stdin.take());
    mcp.assert_exited(1);
}

#[test]
fn eof_during_blocked_preinitialize_error_exits_without_reading_stdout() {
    let mut mcp = Mcp::start();
    mcp.send(json!({"jsonrpc":"2.0","id":large_id(),"method":"tools/list"}));
    drop(mcp.stdin.take());
    mcp.assert_exited(1);
}

#[test]
fn eof_after_initialization_exits_with_a_blocked_native_writer() {
    let mut mcp = Mcp::start();
    let output = mcp.handshake();
    mcp.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    mcp.send(json!({"jsonrpc":"2.0","id":large_id(),"method":"ping"}));
    drop(mcp.stdin.take());
    mcp.assert_exited(0);
    // Keep the reader alive until exit; dropping it first would unblock writes.
    drop(output);
}

#[test]
fn closed_stdout_exits_while_stdin_stays_open() {
    let mut mcp = Mcp::start();
    drop(mcp.stdout.take());
    mcp.send(initialize(json!(1)));
    mcp.assert_exited(1);
}

#[test]
fn oversized_input_exits_without_waiting_for_stdin_eof() {
    let mut mcp = Mcp::start();
    let _ = mcp
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&vec![b'x'; 1024 * 1024 + 1]);
    mcp.assert_exited(1);
}

#[test]
fn closed_stdout_after_handshake_exits_while_stdin_stays_open() {
    let mut mcp = Mcp::start();
    drop(mcp.handshake());
    mcp.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    mcp.send(json!({"jsonrpc":"2.0","id":2,"method":"ping"}));
    mcp.assert_exited(1);
}

#[test]
fn clean_eof_preserves_pipelined_responses() {
    let mut mcp = Mcp::start();
    mcp.send(initialize(json!(1)));
    mcp.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    mcp.send(json!({"jsonrpc":"2.0","id":2,"method":"ping"}));
    drop(mcp.stdin.take());
    let mut output = String::new();
    mcp.stdout
        .as_mut()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    mcp.assert_exited(0);
    let ids: Vec<_> = output
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["id"]
                .as_u64()
                .unwrap()
        })
        .collect();
    assert_eq!(ids, vec![1, 2], "{output}");
}

#[test]
fn whole_scenario_watchdog_unblocks_and_reaps_a_stalled_child() {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--ignored", "--exact", "watchdog_child_fixture"]);
    let mut mcp = Mcp::spawn(command, Duration::from_millis(200), None);
    let result = mcp
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&vec![0; 2 * 1024 * 1024]);
    assert!(
        result.is_err(),
        "watchdog must release a blocked setup write"
    );
    assert!(mcp.timed_out.load(Ordering::SeqCst));
    assert!(mcp.child.lock().unwrap().try_wait().unwrap().is_some());
}

#[test]
#[ignore = "subprocess-only fixture for the whole-scenario watchdog"]
fn watchdog_child_fixture() {
    loop {
        std::thread::park();
    }
}
