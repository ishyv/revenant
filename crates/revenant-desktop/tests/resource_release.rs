use revenant_core::{Resource, Result};
use revenant_desktop::{DesktopOptions, DesktopRuntime};
use revenant_sdk::{Application, OperationRegistration, Provider, ProviderDescriptor};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Semaphore;

struct Tracked(Arc<AtomicUsize>);

#[tokio::test]
async fn issued_scope_subtrees_release_desktop_records_without_affecting_siblings() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = DesktopRuntime::new(
        Application::new(),
        DesktopOptions {
            max_scopes: 9_000,
            data_dir: Some(directory.path().join("data")),
            cache_dir: Some(directory.path().join("cache")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let root = runtime.connect("main").unwrap()["scope"]
        .as_str()
        .unwrap()
        .to_owned();
    let sibling = runtime.create_scope(&root).unwrap();
    let branch = runtime.create_scope(&root).unwrap();
    let mut leaf = branch.clone();
    for _ in 0..8_192 {
        leaf = runtime.create_scope(&leaf).unwrap();
    }
    runtime.dispose_scope(&branch).unwrap();
    runtime.dispose_scope(&leaf).unwrap();
    assert_eq!(runtime.runtime_inspect().unwrap()["ownership"]["scopes"], 2);
    runtime.validate_window(&sibling, "main").unwrap();
    assert_eq!(
        runtime.create_scope(&leaf).unwrap_err().code,
        "scope_disposed"
    );
    assert_eq!(
        runtime.dispose_scope("caller-chosen-id").unwrap_err().code,
        "scope_missing"
    );
    runtime.dispose_scope(&root).unwrap();
    runtime.dispose_scope(&root).unwrap();
    assert_eq!(runtime.runtime_inspect().unwrap()["ownership"]["scopes"], 0);
    runtime.shutdown().await.unwrap();
}
impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Resources {
    drops: Arc<AtomicUsize>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
impl Provider for Resources {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "resources".into(),
            description: "Resource release fixture".into(),
            dependencies: vec![],
            configuration_schema: json!({"type":"object"}),
        }
    }
    fn operations(&self) -> Result<Vec<OperationRegistration>> {
        let drops = self.drops.clone();
        let create = OperationRegistration::new::<(), Resource<Tracked>, _, _>(
            "resources.create",
            "Create an owned resource",
            move |_, context| {
                let drops = drops.clone();
                async move { context.own(Tracked(drops)) }
            },
        )?;
        let entered = self.entered.clone();
        let release = self.release.clone();
        let mut hold = OperationRegistration::new::<Resource<Tracked>, u32, _, _>(
            "resources.hold",
            "Retain an admitted resource",
            move |resource, context| {
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    let lease = context.lease_resource(&resource)?;
                    entered.add_permits(1);
                    tokio::time::timeout(std::time::Duration::from_secs(3), release.acquire())
                        .await
                        .unwrap()
                        .unwrap()
                        .forget();
                    drop(lease);
                    Ok(1)
                }
            },
        )?;
        hold.descriptor.independent = true;
        Ok(vec![create, hold])
    }
}
async fn terminal(runtime: &DesktopRuntime, scope: &str, task: &str) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let snapshot = runtime.task_snapshot(scope, task).unwrap();
            if matches!(
                snapshot.state,
                revenant_core::TaskState::Succeeded
                    | revenant_core::TaskState::Failed
                    | revenant_core::TaskState::Cancelled
            ) {
                return serde_json::to_value(snapshot).unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn release_owner_rejects_fresh_admission_but_preserves_active_and_result_leases() {
    let directory = tempfile::tempdir().unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let runtime = DesktopRuntime::new(
        Application::new().capability(
            Resources {
                drops: drops.clone(),
                entered: entered.clone(),
                release: release.clone(),
            },
            json!({}),
        ),
        DesktopOptions {
            data_dir: Some(directory.path().join("data")),
            cache_dir: Some(directory.path().join("cache")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let scope = runtime.connect("main").unwrap()["scope"]
        .as_str()
        .unwrap()
        .to_owned();
    let creator = runtime
        .dispatch(
            "main",
            json!({"scope":scope,"action":"task.run","operation":"resources.create","input":null}),
            None,
        )
        .await
        .unwrap();
    let result = terminal(&runtime, &scope, creator["id"].as_str().unwrap()).await;
    let handle = result["result"].clone();
    let ownership = runtime.runtime_inspect().unwrap()["ownership"].clone();
    assert_eq!(ownership["nativeOwners"], "1");
    assert_eq!(ownership["nativeLive"], "1");
    let holder = runtime
        .dispatch(
            "main",
            json!({"scope":scope,"action":"task.run","operation":"resources.hold","input":handle}),
            None,
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let other = runtime.create_scope(&scope).unwrap();
    assert_eq!(
        runtime
            .dispatch(
                "main",
                json!({"scope":other,"action":"resource.release","handle":handle}),
                None
            )
            .await
            .unwrap_err()
            .code,
        "scope_mismatch"
    );
    let mut forged = handle.clone();
    forged["generation"] = json!(99);
    assert_eq!(
        runtime
            .dispatch(
                "main",
                json!({"scope":scope,"action":"resource.release","handle":forged}),
                None
            )
            .await
            .unwrap_err()
            .code,
        "stale_handle"
    );
    let mut forged = handle.clone();
    forged["kind"] = json!("file");
    assert_eq!(
        runtime
            .dispatch(
                "main",
                json!({"scope":scope,"action":"resource.release","handle":forged}),
                None
            )
            .await
            .unwrap_err()
            .code,
        "stale_handle"
    );
    assert_eq!(
        runtime
            .dispatch(
                "other-window",
                json!({"scope":scope,"action":"resource.release","handle":handle}),
                None
            )
            .await
            .unwrap_err()
            .code,
        "scope_mismatch"
    );
    runtime
        .dispatch(
            "main",
            json!({"scope":scope,"action":"resource.release","handle":handle}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    let ownership = runtime.runtime_inspect().unwrap()["ownership"].clone();
    assert_eq!(ownership["nativeOwners"], "0");
    assert_eq!(ownership["nativeLive"], "1");
    assert_eq!(runtime.dispatch("main",json!({"scope":scope,"action":"task.run","operation":"resources.hold","input":handle}),None).await.unwrap_err().code,"stale_handle");
    assert_eq!(
        runtime
            .dispatch(
                "main",
                json!({"scope":scope,"action":"resource.release","handle":handle}),
                None
            )
            .await
            .unwrap_err()
            .code,
        "stale_handle"
    );
    release.add_permits(1);
    terminal(&runtime, &scope, holder["id"].as_str().unwrap()).await;
    assert_eq!(
        drops.load(Ordering::SeqCst),
        0,
        "creator task result still retains its lease"
    );
    runtime
        .dispose_task(&scope, creator["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime.runtime_inspect().unwrap()["ownership"]["nativeLive"],
        "0"
    );
    runtime.shutdown().await.unwrap();
}
