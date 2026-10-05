mod common;
use revenant::toolchain::process::{self, ManagedProcess};
use std::{
    fs,
    net::{SocketAddr, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn address(path: &std::path::Path) -> SocketAddr {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(port) = fs::read_to_string(path)
            .ok()
            .and_then(|text| text.parse::<u16>().ok())
        {
            return SocketAddr::from(([127, 0, 0, 1], port));
        }
        assert!(
            Instant::now() < deadline,
            "descendant did not bind its owned test socket"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
fn assert_closed(address: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
        assert!(
            Instant::now() < deadline,
            "owned descendant survived cleanup on {address}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn failed_command_retains_stdout_stderr_and_status_with_space_in_executable_path() {
    let temp = TempDir::new().unwrap();
    let error = process::run_capture(
        temp.path(),
        &common::fixture().to_string_lossy(),
        &["--exit-failure"],
        false,
    )
    .unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(diagnostic.contains("stdout evidence"));
    assert!(diagnostic.contains("stderr evidence"));
    assert!(diagnostic.contains("23"));
}

#[test]
fn dropping_managed_process_stops_its_descendant_not_just_the_parent() {
    let temp = TempDir::new().unwrap();
    let port = temp.path().join("descendant.port");
    let (tx, _rx) = mpsc::channel();
    let mut process = ManagedProcess::spawn(
        temp.path(),
        &common::fixture().to_string_lossy(),
        &["--spawn-descendant", &port.to_string_lossy()],
        "fixture",
        tx,
    )
    .unwrap();
    let endpoint = address(&port);
    assert!(process.try_wait().unwrap().is_none());
    assert!(TcpStream::connect_timeout(&endpoint, Duration::from_secs(1)).is_ok());
    drop(process);
    assert_closed(endpoint);
}

#[test]
fn successful_parent_exit_also_reaps_descendants_and_inherited_pipes() {
    let temp = TempDir::new().unwrap();
    let port = temp.path().join("descendant.port");
    let output = process::run_capture(
        temp.path(),
        &common::fixture().to_string_lossy(),
        &["--spawn-and-exit", &port.to_string_lossy()],
        false,
    )
    .unwrap();
    assert!(output.contains("descendant started"));
    assert_closed(address(&port));
}
