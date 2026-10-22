//! Opt-in real WebView ownership evidence, using an upgraded isolated consumer.
#![cfg(windows)]
use revenant::{
    config::RevenantConfig,
    toolchain::{desktop::DesktopBuild, process},
};
use serde_json::Value;
use std::{
    net::TcpStream,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

fn owned_processes(root: &std::path::Path, identifier: &str) -> Value {
    let script = r#"
        $all = @(Get-CimInstance Win32_Process)
        $owned = @($all | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith(($env:REVENANT_RESTART_ROOT + '\.revenant\run\'), [StringComparison]::OrdinalIgnoreCase) })
        $ids = @($owned | ForEach-Object { $_.ProcessId })
        do {
            $next = @($all | Where-Object { $_.ParentProcessId -in $ids -and $_.ProcessId -notin $ids })
            $owned += $next
            $ids += @($next | ForEach-Object { $_.ProcessId })
        } while ($next.Count -gt 0)
        $profile = @($all | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and $_.CommandLine -and $_.CommandLine.Contains($env:REVENANT_RESTART_IDENTIFIER) })
        $windows = @($owned | Where-Object { (Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue).MainWindowHandle -ne 0 })
        @{ owned=@($owned | Select-Object ProcessId,ParentProcessId,Name,CommandLine); profile=@($profile | Select-Object ProcessId,ParentProcessId,CommandLine); windows=$windows.Count; webviews=@($owned | Where-Object Name -eq 'msedgewebview2.exe').Count } | ConvertTo-Json -Depth 5 -Compress
    "#;
    let output = process::run_capture_with_env(
        root,
        "powershell.exe",
        &["-NoProfile", "-Command", script],
        &[
            ("REVENANT_RESTART_ROOT", &root.to_string_lossy()),
            ("REVENANT_RESTART_IDENTIFIER", identifier),
        ],
        false,
    )
    .unwrap();
    serde_json::from_str(&output).unwrap()
}

struct ExitProbe {
    pid: u32,
    handle: windows_sys::Win32::Foundation::HANDLE,
}
impl ExitProbe {
    fn open(pid: u32) -> Self {
        let handle = unsafe {
            windows_sys::Win32::System::Threading::OpenProcess(
                windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION
                    | 0x00100000, // SYNCHRONIZE: observe termination, never alter it.
                0,
                pid,
            )
        };
        assert!(
            !handle.is_null(),
            "open owned process {pid}: {}",
            std::io::Error::last_os_error()
        );
        Self { pid, handle }
    }
    fn exited(&self) -> bool {
        let status =
            unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(self.handle, 0) };
        status == windows_sys::Win32::Foundation::WAIT_OBJECT_0
    }
}
impl Drop for ExitProbe {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[test]
#[ignore = "opens twenty real windows; requires REVENANT_RESTART_PROJECT pointing at an upgraded isolated consumer"]
fn real_windows_release_owned_webviews_before_immediate_replacement() {
    let root = PathBuf::from(
        std::env::var_os("REVENANT_RESTART_PROJECT").expect("isolated consumer path"),
    );
    let root = dunce::canonicalize(root).unwrap();
    assert!(
        root.starts_with(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/native-workflows-consumers")
        ),
        "use an isolated consumer, never an authored reference"
    );
    let config = RevenantConfig::load(&root).unwrap();
    let identifier = config.identifier();
    let baseline = owned_processes(&root, &identifier);
    assert_eq!(baseline["owned"].as_array().unwrap().len(), 0);
    assert_eq!(
        baseline["profile"].as_array().unwrap().len(),
        0,
        "an unrelated instance already owns this profile"
    );
    let (logs, output) = mpsc::channel();
    let vite = root.join("web/node_modules/vite/bin/vite.js");
    let port = config.desktop.dev_port.to_string();
    let mut frontend = process::ManagedProcess::spawn(
        &root.join("web"),
        "node",
        &[
            &vite.to_string_lossy(),
            "--host",
            "127.0.0.1",
            "--port",
            &port,
            "--strictPort",
        ],
        "vite",
        logs.clone(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while TcpStream::connect(("127.0.0.1", config.desktop.dev_port)).is_err() {
        assert!(frontend.try_wait().unwrap().is_none(), "frontend exited");
        assert!(Instant::now() < deadline, "frontend did not become ready");
        std::thread::sleep(Duration::from_millis(25));
    }
    let build = DesktopBuild::new(&root, &config, false).unwrap();
    let mut evidence = Vec::new();
    let report = root.join("webview-restart-evidence.json");
    let mut replacement = None;
    for cycle in 0..20 {
        let started = Instant::now();
        let mut desktop = replacement
            .take()
            .unwrap_or_else(|| build.launch(&root, logs.clone()).unwrap());
        let ready = loop {
            for line in output.try_iter() {
                assert!(
                    !line.line.contains("0x800700AA"),
                    "resource-in-use: {}",
                    line.line
                );
                assert!(
                    !line.line.contains("Failed to setup app"),
                    "startup failure: {}",
                    line.line
                );
            }
            assert!(
                desktop.try_wait().unwrap().is_none(),
                "window exited during cycle {cycle}"
            );
            let state = owned_processes(&root, &identifier);
            if state["windows"].as_u64().unwrap() > 0 && state["webviews"].as_u64().unwrap() > 0 {
                break state;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "window/WebView did not become ready: {state}"
            );
        };
        let memberships: Vec<_> = ready["owned"]
            .as_array()
            .unwrap()
            .iter()
            .map(|process| {
                let pid = process["ProcessId"].as_u64().unwrap() as u32;
                serde_json::json!({"pid":pid,"inOwnedJob":desktop.owns_process(pid).unwrap()})
            })
            .collect();
        let probes: Vec<_> = ready["owned"]
            .as_array()
            .unwrap()
            .iter()
            .map(|process| ExitProbe::open(process["ProcessId"].as_u64().unwrap() as u32))
            .collect();
        let stopping = Instant::now();
        desktop.terminate_checked().unwrap();
        let shutdown_millis = stopping.elapsed().as_millis();
        let kernel_exits: Vec<_> = probes
            .iter()
            .map(|probe| serde_json::json!({"pid":probe.pid,"exited":probe.exited()}))
            .collect();
        assert!(
            kernel_exits.iter().all(|state| state["exited"] == true),
            "owned process still running after termination: {kernel_exits:?}"
        );
        drop(desktop);
        assert_eq!(
            std::fs::read_dir(root.join(".revenant/run"))
                .unwrap()
                .count(),
            0
        );
        // Match the actual development admission order: launch immediately after
        // owned termination, before a slow CIM query can add an artificial gap.
        let relaunch_started = Instant::now();
        if cycle < 19 {
            replacement = Some(build.launch(&root, logs.clone()).unwrap());
        }
        let relaunch_millis = relaunch_started.elapsed().as_millis();
        let stopped = owned_processes(&root, &identifier);
        evidence.push(serde_json::json!({"cycle":cycle,"startupMillis":started.elapsed().as_millis(),"shutdownMillis":shutdown_millis,"relaunchMillis":relaunch_millis,"jobMembership":memberships,"kernelExits":kernel_exits,"ready":ready,"afterRelaunch":stopped}));
        std::fs::write(&report, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    }
    drop(frontend);
    println!("Real WebView restart evidence: {}", report.display());
}
