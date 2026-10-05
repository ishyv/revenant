mod support;

struct UncooperativeProvider {
    entered: std::sync::Arc<tokio::sync::Semaphore>,
    release: std::sync::Arc<tokio::sync::Semaphore>,
    returned: std::sync::Arc<tokio::sync::Semaphore>,
}
impl revenant_sdk::Provider for UncooperativeProvider {
    fn descriptor(&self) -> revenant_sdk::ProviderDescriptor {
        revenant_sdk::ProviderDescriptor {
            id: "uncooperative".into(),
            description: "Shutdown regression gate".into(),
            dependencies: vec![],
            configuration_schema: serde_json::json!({"type":"object"}),
        }
    }
    fn operations(&self) -> revenant_core::Result<Vec<revenant_sdk::OperationRegistration>> {
        let entered = self.entered.clone();
        let release = self.release.clone();
        let returned = self.returned.clone();
        Ok(vec![revenant_sdk::OperationRegistration::new::<
            u32,
            u32,
            _,
            _,
        >(
            "uncooperative.hold",
            "Deliberately ignores task cancellation",
            move |input, _| {
                let entered = entered.clone();
                let release = release.clone();
                let returned = returned.clone();
                async move {
                    entered.add_permits(1);
                    release.acquire().await.unwrap().forget();
                    returned.add_permits(1);
                    Ok(input)
                }
            },
        )?])
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_interrupts_uncooperative_work_and_preserves_terminal_snapshot() {
    use std::sync::{Arc, Mutex};
    let directory = tempfile::tempdir().unwrap();
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let returned = Arc::new(tokio::sync::Semaphore::new(0));
    let runtime = DesktopRuntime::new(
        Application::new().capability(
            UncooperativeProvider {
                entered: entered.clone(),
                release: release.clone(),
                returned: returned.clone(),
            },
            json!({}),
        ),
        DesktopOptions {
            data_dir: Some(directory.path().join("data")),
            cache_dir: Some(directory.path().join("cache")),
            shutdown_grace: Duration::from_millis(20),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let root = runtime.connect("shutdown-window").unwrap();
    let scope = root["scope"].as_str().unwrap();
    let snapshots = Arc::new(Mutex::new(Vec::<revenant_core::TaskSnapshot>::new()));
    let received = snapshots.clone();
    runtime
        .run_task(
            scope,
            "uncooperative.hold",
            json!(7),
            Some(Arc::new(move |snapshot| {
                received
                    .lock()
                    .unwrap()
                    .push(serde_json::from_value(snapshot).unwrap());
            })),
        )
        .unwrap();
    entered.acquire().await.unwrap().forget();
    let shutdown = tokio::time::timeout(Duration::from_secs(2), runtime.shutdown()).await;
    // Release the native gate even when a assertion fails, so Tokio can tear down.
    release.add_permits(1);
    returned.acquire().await.unwrap().forget();
    shutdown
        .expect("Shutdown must not await an uncooperative executor indefinitely")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let snapshots = snapshots.lock().unwrap();
    let terminal = snapshots.last().unwrap();
    assert_eq!(terminal.state, TaskState::Interrupted);
    assert_eq!(terminal.error.as_ref().unwrap().code, "interrupted");
    assert!(
        snapshots
            .iter()
            .all(|snapshot| snapshot.state != TaskState::Succeeded)
    );
    assert_eq!(
        runtime.connect("late-window").unwrap_err().code,
        "app_closing"
    );
}

use revenant_core::{ResourceRegistry, TaskManager, TaskState};
use revenant_desktop::{DesktopOptions, DesktopRuntime};
use revenant_sdk::Application;
use serde_json::{Value, json};
use std::{sync::atomic::Ordering, time::Duration};
use support::{Harness, error, rows};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_windows_ack_retention_and_selection_capture() {
    let h = Harness::new(500, 999, 2, 4).await;
    let (folder, initial) = h.open(512).await;
    let first = h.ready(initial).await;
    assert_eq!(rows(&first).len(), 500);
    assert_eq!(first["total"], 500);
    assert!(rows(&first).iter().all(|row| row.get("path").is_none()));
    error(
        h.request(json!({"action":"view.window","view":first["view"],"limit":513}))
            .await,
        "invalid_window",
    );
    error(h.request(json!({"action":"selection.set","view":first["view"],"handles":vec![rows(&first)[0]["handle"].clone();513]})).await, "selection_limit");
    let first = h.window(&first, 0, 128).await;
    let entry = rows(&first)[0].clone();
    let next = h.window(&first, 128, 128).await;
    assert_eq!(h.inspect("files").await["resources"], "256");
    // A stale revision must leave BOTH the new offer and its predecessor live.
    h.ack(&first).await;
    assert_eq!(h.inspect("files").await["resources"], "256");
    assert!(
        error(
            h.request(json!({"action":"view.window","view":next["view"],"offset":256}))
                .await,
            "window_pending"
        )
        .retryable
    );
    let captured = h
        .request(json!({"action":"selection.set","view":next["view"],"handles":[entry["handle"]]}))
        .await
        .unwrap();
    let mut claimed = entry.clone();
    claimed["name"] = json!("untrusted-name");
    claimed["size"] = json!("999999");
    let metadata = h
        .request(json!({"action":"task.run","operation":"media.readMetadata","input":claimed}))
        .await
        .unwrap();
    h.ack(&next).await;
    let result = h.terminal(&metadata).await;
    assert_eq!(result.state, TaskState::Succeeded);
    assert_eq!(result.result.as_ref().unwrap()["name"], entry["name"]);
    assert_eq!(result.result.as_ref().unwrap()["size"], "3");
    let checksum = h
        .request(json!({"action":"task.run","operation":"files.checksum","input":entry}))
        .await
        .unwrap();
    let result = h.terminal(&checksum).await;
    assert_eq!(result.state, TaskState::Succeeded);
    assert_eq!(
        result.result.as_ref().unwrap()["digest"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(result.result.as_ref().unwrap()["bytes"], "3");
    let batch = h.request(json!({"action":"task.batch","operation":"acceptance.file","selection":{"id":captured["selection"]}})).await.unwrap();
    assert_eq!(h.terminal(&batch).await.summary.succeeded.get(), 1);
    h.request(json!({"action":"selection.dispose","id":captured["selection"]}))
        .await
        .unwrap();
    for task in [&metadata, &checksum, &batch] {
        h.request(json!({"action":"task.dispose","task":task["id"]}))
            .await
            .unwrap();
    }
    assert_eq!(h.inspect("files").await["resources"], "128");
    h.request(json!({"action":"view.dispose","view":next["view"]}))
        .await
        .unwrap();
    h.request(json!({"action":"folder.dispose","folder":folder}))
        .await
        .unwrap();
    h.released().await;
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_admission_does_not_publish_a_cancelled_task() {
    let h = Harness::new(1, 999, 1, 2).await;
    let (_, initial) = h.open(1).await;
    let ready = h.ready(initial).await;
    let mut foreign = rows(&ready)[0].clone();
    foreign["handle"]["id"] = json!("missing-file");
    let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let sink = received.clone();
    let error = h
        .runtime
        .run_task(
            &h.scope,
            "files.checksum",
            foreign,
            Some(std::sync::Arc::new(move |snapshot| {
                sink.lock().unwrap().push(snapshot)
            })),
        )
        .unwrap_err();
    assert_ne!(error.code, "cancelled");
    assert!(
        received.lock().unwrap().is_empty(),
        "Admission failure must preserve its invoke error instead of publishing cancellation"
    );
    assert_eq!(h.inspect("files").await["activeTasks"], "0");
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_matching_frozen_query_and_paged_outcomes() {
    let h = Harness::new(500, 999, 2, 4).await;
    let (_, initial) = h.open(128).await;
    let ready = h.ready(initial).await;
    let batch = h.request(json!({"action":"task.batch","operation":"acceptance.file","selection":{"view":ready["view"],"generation":ready["generation"],"allMatching":true}})).await.unwrap();
    h.ack(&ready).await;
    let changed = h.request(json!({"action":"view.query","view":ready["view"],"generation":2,"options":{"search":"does-not-match"}})).await.unwrap();
    let changed = h.ready(changed["snapshot"].clone()).await;
    assert_eq!(changed["total"], 0);
    let final_task = h.terminal(&batch).await;
    assert_eq!(final_task.state, TaskState::Succeeded);
    assert_eq!(final_task.summary.completed.get(), 500);
    assert_eq!(final_task.summary.succeeded.get(), 500);
    assert_eq!(final_task.summary.failed.get(), 0);
    assert!(final_task.outcomes.is_empty());
    assert_eq!(final_task.result.as_ref().unwrap()["paged"], true);
    let mut count = 0;
    for offset in (0..500).step_by(128) {
        let page = h
            .request(
                json!({"action":"task.results","task":batch["id"],"offset":offset,"limit":128}),
            )
            .await
            .unwrap();
        assert_eq!(page["total"], 500);
        assert_eq!(page["counts"]["succeeded"], "500");
        assert!(rows(&page).len() <= 128);
        for (index, outcome) in rows(&page).iter().enumerate() {
            assert_eq!(outcome["ordinal"], (offset + index).to_string());
            assert_eq!(outcome["output"], format!("file-{:04}.txt", offset + index));
            assert!(outcome.get("error").is_none_or(Value::is_null));
            count += 1;
        }
    }
    assert_eq!(count, 500);
    error(
        h.request(json!({"action":"task.results","task":batch["id"],"limit":513}))
            .await,
        "invalid_window",
    );
    h.runtime.dispose_window("acceptance-window").unwrap();
    error(
        h.request(json!({"action":"task.snapshot","task":batch["id"]}))
            .await,
        "scope_disposed",
    );
    // Observe cleanup from a new root after the old window and its objects vanish.
    let fresh = h.runtime.connect("cleanup").unwrap();
    let inspected = h
        .runtime
        .dispatch(
            "cleanup",
            json!({"action":"runtime.inspect","scope":fresh["scope"]}),
            None,
        )
        .await
        .unwrap();
    assert!(
        inspected
            .as_array()
            .unwrap()
            .iter()
            .all(|provider| provider["resources"] == "0")
    );
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_cancel_keeps_completed_outcomes_and_releases_scope() {
    let h = Harness::new(500, 5, 2, 4).await;
    let (_, initial) = h.open(128).await;
    let ready = h.ready(initial).await;
    let batch = h.request(json!({"action":"task.batch","operation":"acceptance.file","selection":{"view":ready["view"],"allMatching":true}})).await.unwrap();
    h.gate.wait(5).await;
    h.request(json!({"action":"task.cancel","task":batch["id"]}))
        .await
        .unwrap();
    let result = h.terminal(&batch).await;
    assert_eq!(result.state, TaskState::Partial);
    assert_eq!(result.summary.completed.get(), 4);
    assert_eq!(result.summary.succeeded.get(), 4);
    assert_eq!(result.error.unwrap().code, "cancelled");
    let page = h
        .request(json!({"action":"task.results","task":batch["id"],"limit":128}))
        .await
        .unwrap();
    assert_eq!(rows(&page).len(), 4);
    assert_eq!(page["counts"]["completed"], "4");
    assert_eq!(h.gate.entered.load(Ordering::SeqCst), 5);
    h.request(json!({"action":"task.dispose","task":batch["id"]}))
        .await
        .unwrap();
    error(
        h.request(json!({"action":"task.results","task":batch["id"]}))
            .await,
        "task_disposed",
    );
    h.runtime.dispose_window("acceptance-window").unwrap();
    h.runtime.shutdown().await.unwrap();
    assert!(
        std::fs::read_dir(h.directory.path().join("cache"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_admission_queued_cancellation_and_retry() {
    let h = Harness::new(0, 0, 1, 1).await;
    let running = h
        .request(json!({"action":"task.run","operation":"acceptance.hold","input":1}))
        .await
        .unwrap();
    h.gate.wait(1).await;
    let queued = h
        .request(json!({"action":"task.run","operation":"acceptance.hold","input":2}))
        .await
        .unwrap();
    assert_eq!(queued["state"], "queued");
    assert!(
        error(
            h.request(json!({"action":"task.run","operation":"acceptance.hold","input":3}))
                .await,
            "queue_full"
        )
        .retryable
    );
    h.request(json!({"action":"task.cancel","task":queued["id"]}))
        .await
        .unwrap();
    assert_eq!(h.terminal(&queued).await.state, TaskState::Cancelled);
    assert_eq!(h.gate.entered.load(Ordering::SeqCst), 1);
    // Capacity release happens when the queued executor drops its admission guard.
    let retry = tokio::time::timeout(support::WATCHDOG, async {
        loop {
            match h
                .request(json!({"action":"task.run","operation":"acceptance.hold","input":3}))
                .await
            {
                Ok(task) => break task,
                Err(e) if e.code == "queue_full" => {
                    tokio::time::sleep(Duration::from_millis(5)).await
                }
                Err(e) => panic!("{e}"),
            }
        }
    })
    .await
    .unwrap();
    h.gate.permits.add_permits(1);
    assert_eq!(h.terminal(&running).await.state, TaskState::Succeeded);
    h.gate.wait(2).await;
    h.request(json!({"action":"task.cancel","task":retry["id"]}))
        .await
        .unwrap();
    assert_eq!(h.terminal(&retry).await.state, TaskState::Cancelled);
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_window_and_sibling_scope_admission() {
    let h = Harness::new(500, 999, 2, 4).await;
    let (folder, initial) = h.open(128).await;
    let ready = h.ready(initial).await;
    let forged = json!({"action":"view.window","scope":h.scope,"view":ready["view"]});
    error(
        h.runtime.dispatch("other-window", forged, None).await,
        "scope_mismatch",
    );
    let sibling = h
        .request(json!({"action":"scope.create","parent":h.scope}))
        .await
        .unwrap();
    for request in [
        json!({"action":"folder.query","folder":folder}),
        json!({"action":"view.window","view":ready["view"]}),
        json!({"action":"task.run","operation":"files.checksum","input":rows(&ready)[0]}),
    ] {
        let mut request = request;
        request["scope"] = sibling["scope"].clone();
        error(
            h.runtime.dispatch("acceptance-window", request, None).await,
            "scope_mismatch",
        );
    }
    h.runtime.dispose_window("acceptance-window").unwrap();
    error(
        h.runtime
            .dispatch(
                "acceptance-window",
                json!({"action":"settings.load","scope":sibling["scope"],"key":"x"}),
                None,
            )
            .await,
        "scope_disposed",
    );
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_settings_persist_atomically_and_reject_invalid_save() {
    let h = Harness::new(0, 999, 2, 4).await;
    let defaults = json!({"theme":"dark","nested":{"enabled":true}});
    assert_eq!(
        h.request(json!({"action":"settings.load","key":"preferences","defaults":defaults}))
            .await
            .unwrap(),
        defaults
    );
    h.request(
        json!({"action":"settings.save","key":"preferences","value":{"theme":"light","extra":7}}),
    )
    .await
    .unwrap();
    let path = h
        .directory
        .path()
        .join("data/settings/k-707265666572656e636573.json");
    let before = std::fs::read(&path).unwrap();
    error(
        h.request(json!({"action":"settings.save","key":"preferences","value":{"theme":false}}))
            .await,
        "settings_type_mismatch",
    );
    error(
        h.request(json!({"action":"settings.save","key":"../escape","value":{}}))
            .await,
        "settings_invalid_key",
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // A native observer sees complete old or new JSON across repeated replacements.
    let observed_path = path.clone();
    let observer = tokio::task::spawn_blocking(move || {
        for _ in 0..200 {
            // Atomic replacement on Windows requires observers to share deletion.
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(1 | 2 | 4);
            }
            let mut bytes = Vec::new();
            {
                use std::io::Read;
                options
                    .open(&observed_path)
                    .unwrap()
                    .read_to_end(&mut bytes)
                    .unwrap();
            }
            let record: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(record["theme"].is_string());
        }
    });
    for ordinal in 0..20 {
        h.request(json!({"action":"settings.save","key":"preferences","value":{"theme":format!("theme-{ordinal}"),"extra":7}})).await.unwrap();
    }
    observer.await.unwrap();
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
    h.runtime.shutdown().await.unwrap();
    let reopened = DesktopRuntime::new(
        Application::new(),
        DesktopOptions {
            data_dir: Some(h.directory.path().join("data")),
            cache_dir: Some(h.directory.path().join("cache-reopened")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let root = reopened.connect("reopened").unwrap();
    let persisted = reopened.dispatch("reopened", json!({"action":"settings.load","scope":root["scope"],"key":"preferences","defaults":defaults}), None).await.unwrap();
    assert_eq!(
        persisted,
        json!({"theme":"theme-19","extra":7,"nested":{"enabled":true}})
    );
    reopened.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn leased_preview_ranges_changes_deletion_and_disposal() {
    let h = Harness::new(500, 999, 2, 4).await;
    let (_, initial) = h.open(1).await;
    let ready = h.ready(initial).await;
    let entry = rows(&ready)[0].clone();
    let preview = h
        .request(json!({"action":"media.preview","handle":entry["handle"]}))
        .await
        .unwrap();
    let token = preview["preview"].as_str().unwrap().to_owned();
    assert_eq!(preview["text"], "abc");
    let next = h.window(&ready, 1, 1).await;
    h.ack(&next).await;
    let whole = h.runtime.read_preview(token.clone(), None).await.unwrap();
    assert_eq!(whole.body, b"abc");
    assert_eq!(whole.status, 200);
    for (range, expected, content_range) in [
        ("bytes=1-1", "b", "bytes 1-1/3"),
        ("bytes=1-", "bc", "bytes 1-2/3"),
        ("bytes=-2", "bc", "bytes 1-2/3"),
    ] {
        let read = h
            .runtime
            .read_preview(token.clone(), Some(range.into()))
            .await
            .unwrap();
        assert_eq!(read.body, expected.as_bytes());
        assert_eq!(read.status, 206);
        assert_eq!(read.content_range.as_deref(), Some(content_range));
    }
    for range in [
        "bytes=3-",
        "bytes=-0",
        "bytes=2-1",
        "bytes=0-1,2-2",
        "items=0-1",
    ] {
        let failure = h
            .runtime
            .read_preview(token.clone(), Some(range.into()))
            .await
            .unwrap_err();
        assert_eq!(failure.code, "preview_invalid_range");
        assert_eq!(failure.details.unwrap()["status"], 416);
    }
    std::fs::write(
        h.fixture.join(entry["name"].as_str().unwrap()),
        b"changed-size",
    )
    .unwrap();
    assert_eq!(
        h.runtime
            .read_preview(token.clone(), None)
            .await
            .unwrap_err()
            .code,
        "file_changed"
    );
    // Delete an indexed file BEFORE it receives an open native port. Once pinned,
    // deleting a path intentionally cannot revoke the already admitted file.
    std::fs::remove_file(h.fixture.join("file-0002.txt")).unwrap();
    let deleted = h.window(&next, 2, 1).await;
    let dead_entry = rows(&deleted)[0].clone();
    let checksum = h
        .request(json!({"action":"task.run","operation":"files.checksum","input":dead_entry}))
        .await
        .unwrap();
    let failed = h.terminal(&checksum).await;
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.error.unwrap().code, "file_io");
    error(
        h.request(json!({"action":"media.preview","handle":rows(&deleted)[0]["handle"]}))
            .await,
        "file_io",
    );
    h.request(json!({"action":"media.dispose","preview":token}))
        .await
        .unwrap();
    assert_eq!(
        h.runtime
            .read_preview(token.clone(), None)
            .await
            .unwrap_err()
            .code,
        "preview_not_found"
    );
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_inspection_compatibility_failed_prepare_and_busy_replacement() {
    let h = Harness::new(0, 0, 1, 2).await;
    let before = h.inspect("acceptance").await;
    assert_eq!(before["activeTasks"], "0");
    assert_eq!(before["generation"], 1);
    error(h.request(json!({"action":"runtime.replace","provider":"acceptance","replacement":"incompatible"})).await, "provider_contract_mismatch");
    error(
        h.request(
            json!({"action":"runtime.replace","provider":"acceptance","replacement":"failed"}),
        )
        .await,
        "acceptance_prepare_failed",
    );
    assert_eq!(h.inspect("acceptance").await, before);
    let task = h
        .request(json!({"action":"task.run","operation":"acceptance.hold","input":7}))
        .await
        .unwrap();
    h.gate.wait(1).await;
    assert_eq!(h.inspect("acceptance").await["activeTasks"], "1");
    assert_eq!(h.inspect("files").await["activeTasks"], "1");
    assert!(
        error(
            h.request(
                json!({"action":"runtime.replace","provider":"acceptance","replacement":"good"})
            )
            .await,
            "provider_busy"
        )
        .retryable
    );
    h.gate.permits.add_permits(1);
    assert_eq!(h.terminal(&task).await.state, TaskState::Succeeded);
    tokio::time::timeout(support::WATCHDOG, async {
        while h.inspect("acceptance").await["activeTasks"] != "0" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    h.request(json!({"action":"runtime.replace","provider":"acceptance","replacement":"good"}))
        .await
        .unwrap();
    assert_eq!(h.inspect("acceptance").await["generation"], 2);
    h.runtime.shutdown().await.unwrap();
}

#[test]
fn public_prepared_operation_handles_and_core_interrupt_remain_usable() {
    let registry = revenant_sdk::Registry::new();
    registry
        .register(
            revenant_sdk::OperationRegistration::new::<
                revenant_core::FileEntry,
                revenant_core::FileEntry,
                _,
                _,
            >("echo", "Typed handle regression", |input, _| async move {
                Ok(input)
            })
            .unwrap(),
        )
        .unwrap();
    let prepared = registry.prepare_operation("echo").unwrap();
    let resources = ResourceRegistry::new();
    resources.create_scope("scope", None).unwrap();
    let tasks = TaskManager::new(resources);
    let context = tasks.create("scope", "interrupt").unwrap();
    let execution = context.begin().unwrap();
    assert_eq!(context.interrupt().unwrap().state, TaskState::Interrupted);
    assert_eq!(
        execution.finish(Ok(json!(null))).unwrap_err().code,
        "task_terminal"
    );
    assert_eq!(context.snapshot().error.unwrap().code, "interrupted");
    let handle = json!({"scope":"scope","id":"1","generation":1,"kind":"file"});
    let input =
        json!({"handle":handle,"name":"a","relativePath":"a","size":"3","mime":"text/plain"});
    let handles = prepared.input_handles(&input).unwrap();
    assert_eq!(handles.len(), 1);
    assert_eq!(prepared.output_handles(&input).unwrap(), handles);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_output_resource_is_retained_only_until_history_disposal() {
    let h = Harness::new(500, 999, 2, 4).await;
    let (_, initial) = h.open(1).await;
    let ready = h.ready(initial).await;
    let entry = rows(&ready)[0].clone();
    let echo = h
        .request(json!({"action":"task.run","operation":"acceptance.echo","input":entry}))
        .await
        .unwrap();
    let result = h.terminal(&echo).await;
    assert_eq!(result.state, TaskState::Succeeded);
    assert_eq!(result.result.as_ref().unwrap()["handle"], entry["handle"]);
    let next = h.window(&ready, 1, 1).await;
    h.ack(&next).await;
    assert_eq!(h.inspect("files").await["resources"], "2");
    // Only history owns the old capability now. Admission must resolve it there.
    let checksum = h.request(json!({"action":"task.run","operation":"files.checksum","input":result.result.unwrap()})).await.unwrap();
    assert_eq!(h.terminal(&checksum).await.state, TaskState::Succeeded);
    h.request(json!({"action":"task.dispose","task":echo["id"]}))
        .await
        .unwrap();
    h.request(json!({"action":"task.dispose","task":checksum["id"]}))
        .await
        .unwrap();
    assert_eq!(h.inspect("files").await["resources"], "1");
    assert!(
        h.request(json!({"action":"task.run","operation":"files.checksum","input":entry}))
            .await
            .is_err()
    );
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scope_disposal_cancels_worker_and_reclaims_limits() {
    let h = Harness::new(500, 0, 2, 4).await;
    let child = h
        .request(json!({"action":"scope.create","parent":h.scope}))
        .await
        .unwrap();
    let scope = child["scope"].as_str().unwrap();
    let folder = h.runtime.open_folder(scope, h.fixture.clone()).unwrap();
    let query = h
        .runtime
        .dispatch(
            "acceptance-window",
            json!({"action":"folder.query","scope":scope,"folder":folder["folder"]}),
            None,
        )
        .await
        .unwrap();
    let task = h
        .runtime
        .dispatch(
            "acceptance-window",
            json!({"action":"task.run","scope":scope,"operation":"acceptance.hold","input":1}),
            None,
        )
        .await
        .unwrap();
    h.gate.wait(1).await;
    h.runtime
        .dispatch(
            "acceptance-window",
            json!({"action":"scope.dispose","scope":scope}),
            None,
        )
        .await
        .unwrap();
    error(
        h.runtime
            .dispatch(
                "acceptance-window",
                json!({"action":"task.snapshot","scope":scope,"task":task["id"]}),
                None,
            )
            .await,
        "scope_disposed",
    );
    error(
        h.request(json!({"action":"view.window","view":query["view"]}))
            .await,
        "view_disposed",
    );
    h.released().await;
    tokio::time::timeout(support::WATCHDOG, async {
        while h.inspect("acceptance").await["activeTasks"] != "0" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "supplied large fixture only; set REVENANT_ACCEPTANCE_FIXTURE, or use cargo run -p revenant-desktop --no-default-features --bin acceptance -- <fixture>"]
async fn supplied_large_fixture_benchmark() {
    let fixture =
        std::env::var_os("REVENANT_ACCEPTANCE_FIXTURE").expect("supply an existing fixture");
    println!("{}", support::benchmark::run(fixture.into()).await);
}
