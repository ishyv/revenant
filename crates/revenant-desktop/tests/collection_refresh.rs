mod support;

use revenant_core::TaskState;
use serde_json::{Value, json};
use std::time::Duration;
use support::{Harness, error, rows};

async fn refresh(h: &Harness, previous: &Value, generation: u64) -> Value {
    let response = h
        .request(json!({"action":"view.refresh", "view":previous["view"],
        "generation":generation,"options":{"recursive":true},"offset":0,"limit":128}))
        .await
        .unwrap();
    h.ready(response["snapshot"].clone()).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_observes_added_changed_deleted_files_without_mutating_other_inventory() {
    let h = Harness::new(4, 999, 2, 8).await;
    let (folder, initial) = h.open(128).await;
    let original = h.ready(initial).await;
    let other = h
        .request(json!({"action":"folder.query","folder":folder,"options":{"recursive":true}}))
        .await
        .unwrap();
    let other = h.ready(other["snapshot"].clone()).await;
    std::fs::remove_file(h.fixture.join("file-0001.txt")).unwrap();
    std::fs::write(h.fixture.join("file-0000.txt"), b"changed contents").unwrap();
    std::fs::write(h.fixture.join("added.txt"), b"new").unwrap();
    let refreshed = refresh(&h, &original, 2).await;
    let names: Vec<_> = rows(&refreshed)
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "added.txt",
            "file-0000.txt",
            "file-0002.txt",
            "file-0003.txt"
        ]
    );
    assert_eq!(rows(&refreshed)[1]["size"], "16");
    assert_eq!(refreshed["offset"], 0);
    let old = h.window(&other, 0, 128).await;
    assert!(rows(&old).iter().any(|row| row["name"] == "file-0001.txt"));
    assert_eq!(rows(&old)[0]["size"], "3");
    let filtered = h
        .request(json!({"action":"view.query","view":refreshed["view"],
        "generation":3,"options":{"recursive":true,"search":"added","sort":"relativePath"}}))
        .await
        .unwrap();
    let filtered = h.ready(filtered["snapshot"].clone()).await;
    assert_eq!(filtered["total"], 1);
    assert_eq!(rows(&filtered)[0]["name"], "added.txt");
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_disposal_preserves_admitted_matching_batch_inventory_and_inputs() {
    let h = Harness::new(500, 1, 1, 8).await;
    let (_, initial) = h.open(128).await;
    let original = h.ready(initial).await;
    let batch = h
        .request(json!({"action":"task.batch", "operation":"acceptance.file",
        "selection":{"view":original["view"],"generation":1,"allMatching":true}}))
        .await
        .unwrap();
    h.gate.wait(1).await;
    let pending = h
        .request(json!({"action":"view.refresh","view":original["view"],
        "generation":2,"options":{"recursive":true}}))
        .await
        .unwrap();
    h.request(json!({"action":"view.dispose","view":original["view"]}))
        .await
        .unwrap();
    std::fs::write(h.fixture.join("new-after-batch.txt"), b"new").unwrap();
    h.gate.permits.add_permits(1);
    let completed = h.terminal(&batch).await;
    assert_eq!(completed.state, TaskState::Succeeded);
    assert_eq!(completed.summary.succeeded.get(), 500);
    let refreshed = h.ready(pending["snapshot"].clone()).await;
    assert_eq!(refreshed["total"], 501);
    let last = h
        .request(json!({"action":"task.results","task":batch["id"],"offset":499,"limit":1}))
        .await
        .unwrap();
    assert_eq!(rows(&last)[0]["output"], "file-0499.txt");
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_and_retiring_queued_scans_releases_discovery_ownership() {
    let h = Harness::new(2_000, 1, 1, 10).await;
    let blocker = h
        .request(json!({"action":"task.run","operation":"acceptance.hold","input":1}))
        .await
        .unwrap();
    h.gate.wait(1).await;
    let (_, initial) = h.open(128).await;
    assert_eq!(initial["state"], "loading");
    let cancelled = h
        .request(json!({"action":"view.refresh","view":initial["view"],
        "generation":2,"options":{"recursive":true}}))
        .await
        .unwrap();
    h.request(json!({"action":"view.dispose","view":initial["view"]}))
        .await
        .unwrap();
    h.request(json!({"action":"view.cancel","view":cancelled["view"]}))
        .await
        .unwrap();
    error(
        h.request(json!({"action":"view.query","view":cancelled["view"],
        "generation":3,"options":{"recursive":true}}))
            .await,
        "index_cancelled",
    );
    let newest = h
        .request(json!({"action":"view.refresh","view":cancelled["view"],
        "generation":4,"options":{"recursive":true},"limit":512}))
        .await
        .unwrap();
    h.request(json!({"action":"view.dispose","view":cancelled["view"]}))
        .await
        .unwrap();
    h.gate.permits.add_permits(1);
    assert_eq!(h.terminal(&blocker).await.state, TaskState::Succeeded);
    let newest = tokio::time::timeout(Duration::from_secs(30), h.ready(newest["snapshot"].clone()))
        .await
        .unwrap();
    assert_eq!(newest["total"], 2_000);
    assert_eq!(rows(&newest).len(), 512);
    assert!(h.observed.lock().unwrap().max_rows <= 512);
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires REVENANT_ACCEPTANCE_FIXTURE pointing to an existing 500000-file fixture"]
async fn large_inventory_refresh_retains_other_views_and_bounds_admitted_batch_outcomes() {
    let fixture = std::env::var_os("REVENANT_ACCEPTANCE_FIXTURE")
        .expect("set REVENANT_ACCEPTANCE_FIXTURE to the existing fixture");
    let h = Harness::supplied(fixture.into()).await;
    let started = std::time::Instant::now();
    let (folder, initial) = h.open(128).await;
    let original = large_ready(&h, initial).await;
    assert_eq!(original["total"], 500_000);
    let initial_ms = started.elapsed().as_secs_f64() * 1000.0;
    let other = h
        .request(json!({"action":"folder.query","folder":folder,"limit":128,
            "options":{"recursive":true}}))
        .await
        .unwrap();
    let other = h.ready(other["snapshot"].clone()).await;
    let batch = h
        .request(json!({"action":"task.batch","operation":"acceptance.file",
            "selection":{"view":original["view"],"generation":1,"allMatching":true}}))
        .await
        .unwrap();
    h.gate.wait(5).await;
    h.ack(&original).await;
    let started = std::time::Instant::now();
    let pending = h
        .request(
            json!({"action":"view.refresh","view":original["view"],"generation":2,
            "options":{"recursive":true},"offset":0,"limit":128}),
        )
        .await
        .unwrap();
    h.request(json!({"action":"view.dispose","view":original["view"]}))
        .await
        .unwrap();
    let queried = h
        .request(
            json!({"action":"view.query","view":pending["view"],"generation":3,
            "options":{"recursive":true,"search":".txt","sort":"relativePath"},"limit":128}),
        )
        .await
        .unwrap();
    h.request(json!({"action":"view.dispose","view":pending["view"]}))
        .await
        .unwrap();
    h.request(json!({"action":"task.cancel","task":batch["id"]}))
        .await
        .unwrap();
    let terminal = h.terminal(&batch).await;
    assert_eq!(terminal.state, TaskState::Partial);
    assert_eq!(terminal.summary.succeeded.get(), 4);
    let outcomes = h
        .request(json!({"action":"task.results","task":batch["id"],"offset":0,"limit":128}))
        .await
        .unwrap();
    assert_eq!(rows(&outcomes).len(), 4);
    assert_eq!(rows(&outcomes)[0]["output"], rows(&original)[0]["name"]);
    let refreshed = large_ready(&h, queried["snapshot"].clone()).await;
    let refresh_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(refreshed["total"], 500_000);
    assert_eq!(refreshed["offset"], 0);
    let last = h.window(&refreshed, 499_872, 128).await;
    assert_eq!(rows(&last).len(), 128);
    let retained = h.window(&other, 0, 128).await;
    assert_eq!(retained["total"], 500_000);
    assert_eq!(rows(&retained)[0]["name"], rows(&original)[0]["name"]);
    let scans = support::benchmark::scan_epochs(&h);
    assert_eq!(scans["indexStores"], 2);
    assert_eq!(scans["scanCount"], 2);
    let max_rows = h.observed.lock().unwrap().max_rows;
    assert!(max_rows <= 128);
    // Keep the observation scope admitted while checking resource reclamation.
    h.request(json!({"action":"task.dispose","task":batch["id"]}))
        .await
        .unwrap();
    for view in [&last, &retained] {
        h.request(json!({"action":"view.dispose","view":view["view"]}))
            .await
            .unwrap();
    }
    h.request(json!({"action":"folder.dispose","folder":folder}))
        .await
        .unwrap();
    h.released().await;
    h.runtime.shutdown().await.unwrap();
    let report = json!({"schemaVersion":1,"fixture":h.fixture,"total":500_000,
        "initialScanMilliseconds":initial_ms,"refreshMilliseconds":refresh_ms,
        "queryChangedDuringRefresh":true,"otherInventoryRetained":true,
        "scanEvidence":scans,"maxSinkRows":max_rows,"lastPageRows":rows(&last).len(),
        "admittedBatch":{"state":terminal.state,"completed":terminal.summary.completed,
            "retainedOutcomeRows":rows(&outcomes).len()},
        "peakProcessMemoryBytes":support::benchmark::peak_memory(),
        "cleanup":{"fileResources":"0","cacheStores":0},"performanceThresholds":false});
    if let Some(path) = std::env::var_os("REVENANT_REFRESH_REPORT") {
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(&report).unwrap()),
        )
        .unwrap();
    }
    println!("Large refresh acceptance: {report}");
}

async fn large_ready(h: &Harness, mut snapshot: Value) -> Value {
    tokio::time::timeout(support::WATCHDOG, async {
        loop {
            if snapshot["state"] == "ready" {
                return snapshot;
            }
            assert_ne!(snapshot["state"], "failed", "scan failed: {snapshot}");
            h.ack(&snapshot).await;
            // Observe the native stream without making 200 extra SQL queries
            // per second dominate a large, filtered-inventory measurement.
            tokio::time::sleep(Duration::from_millis(250)).await;
            let latest = h
                .observed
                .lock()
                .unwrap()
                .views
                .get(snapshot["view"].as_str().unwrap())
                .cloned();
            if let Some(latest) = latest
                && latest["revision"].as_u64() > snapshot["revision"].as_u64()
            {
                snapshot = latest;
            } else {
                match h
                    .request(json!({"action":"view.window","view":snapshot["view"],
                    "generation":snapshot["generation"],"offset":0,"limit":128}))
                    .await
                {
                    Ok(next) => snapshot = next,
                    Err(error) if error.code == "window_pending" => (),
                    Err(error) => panic!("large scan observation failed: {error}"),
                }
            }
        }
    })
    .await
    .expect("large refresh scan stopped responding")
}
