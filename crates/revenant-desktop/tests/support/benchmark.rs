use super::{Harness, rows};
use revenant_core::TaskState;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Instant};

// Read-only observations of metadata stores, never an extra filesystem walk.
fn scan_epochs(h: &Harness) -> Value {
    let mut epochs = Vec::new();
    for directory in std::fs::read_dir(h.directory.path().join("cache")).unwrap() {
        let path = directory.unwrap().path();
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("revenant-index-")
        {
            let db = Connection::open_with_flags(
                path.join("store.sqlite3"),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let epoch: i64 = db
                .query_row("SELECT coalesce(max(epoch),0) FROM files", [], |row| {
                    row.get(0)
                })
                .unwrap();
            epochs.push(epoch);
        }
    }
    json!({"scanCount":epochs.iter().sum::<i64>(),"indexStores":epochs.len(),"epochs":epochs,"source":"max committed SQLite epoch per index; empty indexes report zero"})
}

pub async fn run(fixture: PathBuf) -> Value {
    assert!(fixture.is_dir(), "fixture must be an existing directory");
    let h = Harness::supplied(fixture).await;
    let started = Instant::now();
    let folder = h.runtime.open_folder(&h.scope, h.fixture.clone()).unwrap();
    let open_ms = started.elapsed().as_secs_f64() * 1000.0;
    let before_query = scan_epochs(&h);
    eprintln!("Folder admitted in {open_ms:.3} ms; starting metadata discovery");
    let started = Instant::now();
    let query = h.request(json!({"action":"folder.query","folder":folder["folder"],"limit":128,"options":{"recursive":true}})).await.unwrap();
    let initial_ms = started.elapsed().as_secs_f64() * 1000.0;
    let ready = h.ready(query["snapshot"].clone()).await;
    let scan_ms = started.elapsed().as_secs_f64() * 1000.0;
    let total = ready["total"].as_u64().unwrap();
    eprintln!("Indexed {total} files in {scan_ms:.3} ms");
    assert!(rows(&ready).len() <= 128);
    assert_eq!(ready["total"], ready["discovered"]);
    let mut windows = Vec::new();
    let mut snapshot = ready;
    for offset in [0, 128, total.saturating_sub(128)] {
        let started = Instant::now();
        snapshot = h.window(&snapshot, offset, 128).await;
        assert!(rows(&snapshot).len() <= 128);
        windows.push(json!({"offset":offset,"rows":rows(&snapshot).len(),"milliseconds":started.elapsed().as_secs_f64()*1000.0}));
    }
    let mut searches = Vec::new();
    for (index, search) in ["", "file", ".txt", "__revenant_acceptance_no_match__"]
        .iter()
        .enumerate()
    {
        h.ack(&snapshot).await;
        let started = Instant::now();
        let next = h.request(json!({"action":"view.query","view":snapshot["view"],"generation":index+2,"limit":128,"options":{"recursive":true,"search":search}})).await.unwrap();
        h.request(json!({"action":"view.dispose","view":snapshot["view"]}))
            .await
            .unwrap();
        snapshot = h.ready(next["snapshot"].clone()).await;
        assert!(rows(&snapshot).len() <= 128);
        searches.push(json!({"search":search,"total":snapshot["total"],"rows":rows(&snapshot).len(),"milliseconds":started.elapsed().as_secs_f64()*1000.0}));
    }
    h.ack(&snapshot).await;
    let reset = h.request(json!({"action":"view.query","view":snapshot["view"],"generation":6,"limit":128,"options":{"recursive":true}})).await.unwrap();
    h.request(json!({"action":"view.dispose","view":snapshot["view"]}))
        .await
        .unwrap();
    snapshot = h.ready(reset["snapshot"].clone()).await;
    let after_search = scan_epochs(&h);
    assert_eq!(before_query["indexStores"], 0);
    assert_eq!(after_search["indexStores"], 1);
    assert_eq!(after_search["scanCount"], if total == 0 { 0 } else { 1 });
    let cancellation = if total >= 5 {
        let started = Instant::now();
        let task = h.request(json!({"action":"task.batch","operation":"acceptance.file","selection":{"view":snapshot["view"],"allMatching":true,"generation":6}})).await.unwrap();
        h.gate.wait(5).await;
        let cancellation_started = Instant::now();
        h.request(json!({"action":"task.cancel","task":task["id"]}))
            .await
            .unwrap();
        let cancelled = h.terminal(&task).await;
        let cancellation_ms = cancellation_started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(cancelled.state, TaskState::Partial);
        assert_eq!(cancelled.summary.completed.get(), 4);
        let page = h
            .request(json!({"action":"task.results","task":task["id"],"limit":128}))
            .await
            .unwrap();
        assert_eq!(rows(&page).len(), 4);
        h.request(json!({"action":"task.dispose","task":task["id"]}))
            .await
            .unwrap();
        json!({"state":"partial","completed":"4","pageRows":4,"milliseconds":started.elapsed().as_secs_f64()*1000.0,"requestToTerminalMilliseconds":cancellation_ms})
    } else {
        json!({"skipped":"fixture has fewer than five files"})
    };
    let started = Instant::now();
    let task = h.request(json!({"action":"task.batch","operation":"files.checksum","selection":{"view":snapshot["view"],"allMatching":true,"generation":6}})).await.unwrap();
    eprintln!("Processing {total} files; completed outcomes remain on disk");
    let final_task = h.terminal(&task).await;
    let process_ms = started.elapsed().as_secs_f64() * 1000.0;
    eprintln!("Batch finished in {process_ms:.3} ms; validating cleanup");
    assert_eq!(final_task.summary.completed.get(), total);
    let first = h
        .request(json!({"action":"task.results","task":task["id"],"offset":0,"limit":128}))
        .await
        .unwrap();
    let last = h.request(json!({"action":"task.results","task":task["id"],"offset":total.saturating_sub(128),"limit":128})).await.unwrap();
    assert!(rows(&first).len() <= 128 && rows(&last).len() <= 128);
    let observed = h.observed.lock().unwrap();
    let publications = observed.publications;
    let max_rows = observed.max_rows;
    drop(observed);
    assert!(max_rows <= 128);
    let scan_after_process = scan_epochs(&h);
    h.request(json!({"action":"task.dispose","task":task["id"]}))
        .await
        .unwrap();
    h.request(json!({"action":"view.dispose","view":snapshot["view"]}))
        .await
        .unwrap();
    h.request(json!({"action":"folder.dispose","folder":folder["folder"]}))
        .await
        .unwrap();
    h.released().await;
    h.runtime.shutdown().await.unwrap();
    json!({
        "schemaVersion":1,"fixture":h.fixture,"total":total.to_string(),"discovered":snapshot["discovered"],
        "opening":{"milliseconds":open_ms,"indexStoresBeforeQuery":before_query["indexStores"]},
        "query":{"initialMilliseconds":initial_ms,"readyMilliseconds":scan_ms,"scanCount":scan_after_process["scanCount"],"scanEvidence":scan_after_process,
            "fileBytesRead":null,"fileBytesReadEvidence":"not instrumented through public DesktopRuntime; open/index/search paths only inspect metadata and open bounded file ports"},
        "windows":windows,"searches":searches,"sinkPublications":publications,"maxSinkRows":max_rows,
        "cancellation":cancellation,
        "checksumBatch":{"state":final_task.state,"summary":final_task.summary,"milliseconds":process_ms,"itemsPerSecond":if process_ms>0.0 { total as f64/(process_ms/1000.0) } else { 0.0 },"firstPageRows":rows(&first).len(),"lastPageRows":rows(&last).len(),"counts":first["counts"]},
        "cleanup":{"fileResources":"0","cacheStores":0},"peakProcessMemoryBytes":peak_memory(),
        "performanceThresholds":false
    })
}

#[cfg(windows)]
fn peak_memory() -> Option<u64> {
    #[repr(C)]
    struct Counters {
        cb: u32,
        faults: u32,
        peak_working: usize,
        working: usize,
        peak_paged: usize,
        paged: usize,
        peak_nonpaged: usize,
        nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut Counters,
            size: u32,
        ) -> i32;
    }
    // The C layout matches PROCESS_MEMORY_COUNTERS on both Windows pointer widths.
    let mut counters: Counters = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<Counters>() as u32;
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    (ok != 0).then_some(counters.peak_working as u64)
}
#[cfg(not(windows))]
fn peak_memory() -> Option<u64> {
    None
}
