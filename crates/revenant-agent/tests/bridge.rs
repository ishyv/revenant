use revenant_agent::{
    AgentTool, Backend, BackendFuture, BridgeAccess, BridgeServer, InstanceInfo, InstanceSelector,
    RemoteBackend,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Echo {
    closed: Arc<AtomicUsize>,
}
struct Blocked {
    entered: Arc<tokio::sync::Notify>,
    closed: Arc<AtomicUsize>,
}
impl Backend for Blocked {
    fn tools(&self) -> Vec<AgentTool> {
        Echo {
            closed: self.closed.clone(),
        }
        .tools()
    }
    fn call<'a>(&'a self, _: &'a str, _: Value) -> BackendFuture<'a, Value> {
        Box::pin(async move {
            self.entered.notify_one();
            std::future::pending().await
        })
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        self.closed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn cancelled_bridge_call_retires_the_connection_instead_of_reusing_unread_frames() {
    let directory = tempfile::tempdir().unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let closed = Arc::new(AtomicUsize::new(0));
    let backend = Arc::new(Blocked {
        entered: entered.clone(),
        closed: closed.clone(),
    });
    let server = BridgeServer::start_in(
        directory.path(),
        InstanceInfo::new("app.test", "digest", None, false),
        Arc::new(move |_| Ok(backend.clone() as Arc<dyn Backend>)),
    )
    .await
    .unwrap();
    let client = RemoteBackend::connect_in(directory.path(), InstanceSelector::default())
        .await
        .unwrap();
    {
        let call = client.call("echo", json!({}));
        tokio::pin!(call);
        tokio::select! { _ = entered.notified() => {}, _ = &mut call => panic!("blocked call unexpectedly returned") }
    }
    assert_eq!(
        client.call("echo", json!({})).await.unwrap_err().code,
        "session_closed"
    );
    client.close().await.unwrap();
    server.stop().await.unwrap();
    assert_eq!(closed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn bridge_close_interrupts_an_in_flight_reply_without_waiting_for_its_mutex() {
    let directory = tempfile::tempdir().unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let backend = Arc::new(Blocked {
        entered: entered.clone(),
        closed: Arc::new(AtomicUsize::new(0)),
    });
    let server = BridgeServer::start_in(
        directory.path(),
        InstanceInfo::new("app.test", "digest", None, false),
        Arc::new(move |_| Ok(backend.clone() as Arc<dyn Backend>)),
    )
    .await
    .unwrap();
    let client = Arc::new(
        RemoteBackend::connect_in(directory.path(), InstanceSelector::default())
            .await
            .unwrap(),
    );
    let caller = client.clone();
    let call = tokio::spawn(async move { caller.call("echo", json!({})).await });
    entered.notified().await;
    tokio::time::timeout(std::time::Duration::from_millis(200), client.close())
        .await
        .expect("close must interrupt a blocked request")
        .unwrap();
    assert_eq!(call.await.unwrap().unwrap_err().code, "session_closed");
    server.stop().await.unwrap();
}
impl Backend for Echo {
    fn tools(&self) -> Vec<AgentTool> {
        vec![AgentTool {
            name: "echo".into(),
            description: "Echo an input".into(),
            input_schema: json!({"type":"object"}),
            output_schema: None,
            read_only: true,
        }]
    }
    fn call<'a>(&'a self, name: &'a str, arguments: Value) -> BackendFuture<'a, Value> {
        Box::pin(async move {
            if name != "echo" {
                return Err(revenant_core::Error::new("hidden_operation", "Not exposed"));
            }
            if arguments["large"] == true {
                return Ok(json!("x".repeat(1_100_000)));
            }
            Ok(arguments)
        })
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        self.closed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn discovery_rejects_records_without_protected_user_permissions() {
    let directory = tempfile::tempdir().unwrap();
    let server = BridgeServer::start_in(
        directory.path(),
        InstanceInfo::new("app.test", "digest", None, false),
        Arc::new(|_| {
            Ok(Arc::new(Echo {
                closed: Arc::new(AtomicUsize::new(0)),
            }) as Arc<dyn Backend>)
        }),
    )
    .await
    .unwrap();
    let unprotected = directory.path().join("unprotected.json");
    std::fs::write(&unprotected, "{}").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&unprotected, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    assert!(revenant_agent::bridge::discover_instances_in(directory.path()).is_err());
    std::fs::remove_file(unprotected).unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn local_bridge_has_identity_bounded_calls_and_disconnect_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let closed = Arc::new(AtomicUsize::new(0));
    let factory_closed = closed.clone();
    let server = BridgeServer::start_in(
        directory.path(),
        InstanceInfo::new("app.test", "sha256:test", None, true),
        Arc::new(move |_: BridgeAccess| {
            Ok(Arc::new(Echo {
                closed: factory_closed.clone(),
            }) as Arc<dyn Backend>)
        }),
    )
    .await
    .unwrap();
    let client = RemoteBackend::connect_in(directory.path(), InstanceSelector::default())
        .await
        .unwrap();
    assert_eq!(client.tools()[0].name, "echo");
    assert_eq!(
        client.call("echo", json!({"text":"hello"})).await.unwrap(),
        json!({"text":"hello"})
    );
    assert_eq!(
        client.call("hidden", json!({})).await.unwrap_err().code,
        "hidden_operation"
    );
    assert!(
        client
            .call("echo", json!({"text":"x".repeat(1_100_000)}))
            .await
            .is_err()
    );
    assert_eq!(
        client
            .call("echo", json!({"large":true}))
            .await
            .unwrap_err()
            .code,
        "bridge_result_limit"
    );
    assert_eq!(
        client
            .call("echo", json!({"still":"connected"}))
            .await
            .unwrap(),
        json!({"still":"connected"})
    );
    client.close().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while closed.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    server.stop().await.unwrap();
    assert!(
        revenant_agent::bridge::discover_instances_in(directory.path())
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn ambiguous_instances_need_explicit_selection_and_development_access_is_separate() {
    let directory = tempfile::tempdir().unwrap();
    let factory = Arc::new(|_: BridgeAccess| {
        Ok(Arc::new(Echo {
            closed: Arc::new(AtomicUsize::new(0)),
        }) as Arc<dyn Backend>)
    });
    let first = BridgeServer::start_in(
        directory.path(),
        InstanceInfo::new("app.test", "digest", None, false),
        factory.clone(),
    )
    .await
    .unwrap();
    let second = BridgeServer::start_in(
        directory.path(),
        InstanceInfo::new("app.test", "digest", None, false),
        factory,
    )
    .await
    .unwrap();
    assert_eq!(
        RemoteBackend::connect_in(directory.path(), InstanceSelector::default())
            .await
            .err()
            .unwrap()
            .code,
        "instance_ambiguous"
    );
    let selected = InstanceSelector {
        instance_id: Some(first.info().instance_id.clone()),
        ..Default::default()
    };
    let client = RemoteBackend::connect_in(directory.path(), selected.clone())
        .await
        .unwrap();
    client.close().await.unwrap();
    assert_eq!(
        RemoteBackend::connect_in(
            directory.path(),
            InstanceSelector {
                development_only: true,
                ..selected
            }
        )
        .await
        .err()
        .unwrap()
        .code,
        "instance_missing"
    );
    first.stop().await.unwrap();
    second.stop().await.unwrap();
}
