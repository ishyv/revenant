use revenant_core::{FolderHandle, TaskState};
use revenant_desktop::{DesktopOptions, DesktopRuntime};
use revenant_sdk::{Application, Result, TaskContext, operation, operations};
use serde_json::json;

#[operation(read_only)]
fn folder_name(root: FolderHandle, context: TaskContext) -> Result<String> {
    let folder = context.lease_resource(&root)?;
    Ok(folder
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned())
}

#[operation(read_only)]
fn echo_folder(root: FolderHandle) -> FolderHandle {
    root
}

async fn terminal(
    runtime: &DesktopRuntime,
    scope: &str,
    task: &revenant_core::TaskSnapshot,
) -> revenant_core::TaskSnapshot {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let snapshot = runtime.task_snapshot(scope, &task.id).unwrap();
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn picker_admitted_folder_can_be_consumed_by_custom_typed_operation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("chosen-root");
    std::fs::create_dir(&root).unwrap();
    let runtime = DesktopRuntime::new(
        Application::new().operations(operations![folder_name]),
        DesktopOptions {
            data_dir: Some(temporary.path().join("data")),
            cache_dir: Some(temporary.path().join("cache")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let scope = runtime.connect("main").unwrap()["scope"]
        .as_str()
        .unwrap()
        .to_owned();
    let descriptor = runtime.open_folder(&scope, root).unwrap();
    assert!(
        descriptor["capability"].is_object(),
        "picker must expose a typed capability independently of the display path"
    );
    let handle: FolderHandle = serde_json::from_value(descriptor["capability"].clone()).unwrap();
    let task = runtime
        .run_task(&scope, "folderName", json!({"root":handle}), None)
        .unwrap();
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let snapshot = runtime.task_snapshot(&scope, &task.id).unwrap();
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(terminal.state, TaskState::Succeeded, "{:?}", terminal.error);
    assert_eq!(terminal.result, Some(json!("chosen-root")));
    runtime
        .dispose_folder(&scope, descriptor["folder"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        runtime
            .run_task(&scope, "folderName", json!({"root":handle}), None)
            .unwrap_err()
            .code,
        "stale_handle"
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_result_history_retains_folder_after_owner_disposal_until_result_is_disposed() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("history-root");
    std::fs::create_dir(&root).unwrap();
    let runtime = DesktopRuntime::new(
        Application::new().operations(operations![folder_name, echo_folder]),
        DesktopOptions {
            data_dir: Some(temporary.path().join("data")),
            cache_dir: Some(temporary.path().join("cache")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let scope = runtime.connect("main").unwrap()["scope"]
        .as_str()
        .unwrap()
        .to_owned();
    let descriptor = runtime.open_folder(&scope, root).unwrap();
    let echo = runtime
        .run_task(
            &scope,
            "echoFolder",
            json!({"root":descriptor["capability"]}),
            None,
        )
        .unwrap();
    let echo_done = terminal(&runtime, &scope, &echo).await;
    assert_eq!(
        echo_done.state,
        TaskState::Succeeded,
        "{:?}",
        echo_done.error
    );
    runtime
        .dispose_folder(&scope, descriptor["folder"].as_str().unwrap())
        .unwrap();
    let consume = runtime
        .run_task(
            &scope,
            "folderName",
            json!({"root":echo_done.result.clone().unwrap()}),
            None,
        )
        .unwrap();
    let consumed = terminal(&runtime, &scope, &consume).await;
    assert_eq!(consumed.result, Some(json!("history-root")));
    runtime.dispose_task(&scope, &echo.id).unwrap();
    // A terminal task's result can precede the worker's disposal bookkeeping.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match runtime.run_task(
                &scope,
                "folderName",
                json!({"root":descriptor["capability"]}),
                None,
            ) {
                Err(error) => {
                    assert_eq!(error.code, "stale_handle");
                    break;
                }
                Ok(task) => {
                    terminal(&runtime, &scope, &task).await;
                    runtime.dispose_task(&scope, &task.id).unwrap();
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    runtime.shutdown().await.unwrap();
}
