//! Callback captures are application code too. Bound hangs/aborts in child processes.
use revenant_core::{
    CancellationProbe, Progress, ResourceRegistry, SnapshotListener, TaskContext, TaskManager,
    TaskSnapshot, TaskState,
};
use serde_json::json;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    process::{Command, Stdio},
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

fn bounded(name: &str, exercise: impl FnOnce()) {
    if std::env::var("REVENANT_CALLBACK_SAFETY_CASE").as_deref() == Ok(name) {
        exercise();
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("REVENANT_CALLBACK_SAFETY_CASE", name)
        .env("RUST_BACKTRACE", "0")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{name}: child failed/aborted: {status}");
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("{name}: callback cleanup blocked (liveness watchdog, not a timing assertion)");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn setup() -> (ResourceRegistry, TaskManager, TaskContext, String) {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let tasks = TaskManager::new(resources.clone());
    let context = tasks.create(&scope, "callback-test").unwrap();
    (resources, tasks, context, scope)
}

struct ReadOnDrop(TaskContext, Arc<AtomicUsize>);
impl Drop for ReadOnDrop {
    fn drop(&mut self) {
        let _ = self.0.snapshot();
        self.1.fetch_add(1, Ordering::SeqCst);
    }
}
impl SnapshotListener for ReadOnDrop {
    fn changed(&self, _: TaskSnapshot) {}
}
impl CancellationProbe for ReadOnDrop {
    fn is_cancelled(&self) -> bool {
        false
    }
}
struct CountDrop(Arc<AtomicUsize>);
impl Drop for CountDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn unsubscribed_capture_can_read_its_task_during_destruction() {
    bounded(
        "unsubscribed_capture_can_read_its_task_during_destruction",
        || {
            let (_, _, context, _) = setup();
            let drops = Arc::new(AtomicUsize::new(0));
            let subscription =
                context.subscribe(Arc::new(ReadOnDrop(context.clone(), drops.clone())));
            drop(subscription);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        },
    );
}

#[test]
fn replaced_probe_can_read_its_task_during_destruction() {
    bounded(
        "replaced_probe_can_read_its_task_during_destruction",
        || {
            let (_, _, context, _) = setup();
            let drops = Arc::new(AtomicUsize::new(0));
            context
                .set_cancellation_probe(Arc::new(ReadOnDrop(context.clone(), drops.clone())))
                .unwrap();
            context.set_cancellation_probe(Arc::new(|| false)).unwrap();
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        },
    );
}

#[test]
fn finalized_probe_is_released_outside_lock() {
    bounded("finalized_probe_is_released_outside_lock", || {
        for queued in [false, true] {
            let (_, _, context, _) = setup();
            let drops = Arc::new(AtomicUsize::new(0));
            context
                .set_cancellation_probe(Arc::new(ReadOnDrop(context.clone(), drops.clone())))
                .unwrap();
            if queued {
                context.request_cancel().unwrap();
            } else {
                context.begin().unwrap().finish(Ok(json!(null))).unwrap();
            }
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        }
    });
}

#[test]
fn finalization_keeps_a_probe_already_cloned_by_a_checkpoint_alive() {
    bounded(
        "finalization_keeps_a_probe_already_cloned_by_a_checkpoint_alive",
        || {
            let (_, _, context, _) = setup();
            let execution = context.begin().unwrap();
            let entered = Arc::new(Barrier::new(2));
            let release = Arc::new(Barrier::new(2));
            let drops = Arc::new(AtomicUsize::new(0));
            let capture = ReadOnDrop(context.clone(), drops.clone());
            let inside = entered.clone();
            let finish = release.clone();
            context
                .set_cancellation_probe(Arc::new(move || {
                    let _ = &capture;
                    inside.wait();
                    finish.wait();
                    false
                }))
                .unwrap();
            let checking = context.clone();
            let worker = std::thread::spawn(move || checking.checkpoint());
            entered.wait();
            execution.finish(Ok(json!(null))).unwrap();
            assert_eq!(drops.load(Ordering::SeqCst), 0);
            release.wait();
            worker.join().unwrap().unwrap();
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        },
    );
}

#[test]
fn failed_initial_delivery_unregisters_and_releases_capture() {
    bounded(
        "failed_initial_delivery_unregisters_and_releases_capture",
        || {
            let (_, _, context, _) = setup();
            let drops = Arc::new(AtomicUsize::new(0));
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            let capture = ReadOnDrop(context.clone(), drops.clone());
            assert!(
                catch_unwind(AssertUnwindSafe(|| context.subscribe(Arc::new(
                    move |snapshot: TaskSnapshot| {
                        let _ = &capture;
                        count.fetch_add(1, Ordering::SeqCst);
                        if snapshot.state == TaskState::Queued {
                            panic!("immediate observer");
                        }
                    }
                ))))
                .is_err()
            );
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            context.request_cancel().unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        },
    );
}

#[test]
fn begin_observer_panic_settles_and_rolls_back_claimed_execution() {
    let (resources, tasks, context, scope) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let created = context.own(CountDrop(drops.clone())).unwrap();
    let _subscription = context.subscribe(Arc::new(|snapshot: TaskSnapshot| {
        if snapshot.state == TaskState::Running {
            panic!("running observer");
        }
    }));
    assert!(catch_unwind(AssertUnwindSafe(|| context.begin())).is_err());
    assert_eq!(context.snapshot().state, TaskState::Interrupted);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(
        resources.lease_native(&scope, &created).unwrap_err().code,
        "stale_handle"
    );
    tasks.remove(&scope, context.id()).unwrap();
}

#[test]
fn queued_cancel_observer_panic_cannot_skip_created_owner_rollback() {
    let (resources, _, context, scope) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let created = context.own(CountDrop(drops.clone())).unwrap();
    let _subscription = context.subscribe(Arc::new(|snapshot: TaskSnapshot| {
        if snapshot.state == TaskState::Cancelled {
            panic!("cancel observer");
        }
    }));
    assert!(catch_unwind(AssertUnwindSafe(|| context.request_cancel())).is_err());
    assert_eq!(context.snapshot().state, TaskState::Cancelled);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(
        resources.lease_native(&scope, &created).unwrap_err().code,
        "stale_handle"
    );
}

#[test]
fn ordinary_progress_panic_propagates_without_releasing_live_execution() {
    let (resources, _, context, scope) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let input = resources
        .register_native(&scope, CountDrop(drops.clone()))
        .unwrap();
    context.lease_resource(&input).unwrap();
    resources.release(&scope, input.handle()).unwrap();
    let execution = context.begin().unwrap();
    let _subscription = context.subscribe(Arc::new(|snapshot: TaskSnapshot| {
        if snapshot.state == TaskState::Running && snapshot.progress.completed.get() == 1 {
            panic!("progress observer");
        }
    }));
    assert!(
        catch_unwind(AssertUnwindSafe(|| context.progress(Progress {
            completed: 1.into(),
            total: Some(1.into()),
            message: None,
        })))
        .is_err()
    );
    assert_eq!(context.snapshot().progress.completed.get(), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    execution.finish(Ok(json!(42))).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn running_cancel_panic_preserves_leases_until_executor_acknowledgement() {
    let (resources, _, context, scope) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let input = resources
        .register_native(&scope, CountDrop(drops.clone()))
        .unwrap();
    context.lease_resource(&input).unwrap();
    resources.release(&scope, input.handle()).unwrap();
    let execution = context.begin().unwrap();
    let _subscription = context.subscribe(Arc::new(|snapshot: TaskSnapshot| {
        if snapshot.state == TaskState::Cancelling {
            panic!("running cancel observer");
        }
    }));
    assert!(catch_unwind(AssertUnwindSafe(|| context.request_cancel())).is_err());
    assert_eq!(context.snapshot().state, TaskState::Cancelling);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    execution
        .finish(Err(revenant_core::Error::cancelled()))
        .unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn terminal_observer_panic_preserves_committed_success_and_releases_input() {
    let (resources, _, context, scope) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let input = resources
        .register_native(&scope, CountDrop(drops.clone()))
        .unwrap();
    context.lease_resource(&input).unwrap();
    resources.release(&scope, input.handle()).unwrap();
    let execution = context.begin().unwrap();
    let _subscription = context.subscribe(Arc::new(|snapshot: TaskSnapshot| {
        if snapshot.state.is_terminal() {
            panic!("terminal observer");
        }
    }));
    assert!(catch_unwind(AssertUnwindSafe(|| execution.finish(Ok(json!(42))))).is_err());
    let snapshot = context.snapshot();
    assert_eq!(snapshot.state, TaskState::Succeeded);
    assert_eq!(snapshot.result, Some(json!(42)));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn executor_unwind_contains_cleanup_panics_and_notifies_other_observers_after_release() {
    bounded(
        "executor_unwind_contains_cleanup_panics_and_notifies_other_observers_after_release",
        || {
            let (resources, _, context, scope) = setup();
            let drops = Arc::new(AtomicUsize::new(0));
            let input = resources
                .register_native(&scope, CountDrop(drops.clone()))
                .unwrap();
            context.lease_resource(&input).unwrap();
            resources.release(&scope, input.handle()).unwrap();
            let execution = context.begin().unwrap();
            let _bad = context.subscribe(Arc::new(|snapshot: TaskSnapshot| {
                if snapshot.state == TaskState::Interrupted {
                    panic!("cleanup observer");
                }
            }));
            let notified = Arc::new(AtomicUsize::new(0));
            let healthy = notified.clone();
            let released = drops.clone();
            let _good = context.subscribe(Arc::new(move |snapshot: TaskSnapshot| {
                if snapshot.state == TaskState::Interrupted {
                    assert_eq!(
                        released.load(Ordering::SeqCst),
                        1,
                        "notify after actual lease release"
                    );
                    healthy.fetch_add(1, Ordering::SeqCst);
                }
            }));
            let panic = catch_unwind(AssertUnwindSafe(|| {
                let _execution = execution;
                panic!("operation panic");
            }))
            .unwrap_err();
            assert_eq!(panic.downcast_ref::<&str>(), Some(&"operation panic"));
            assert_eq!(context.snapshot().state, TaskState::Interrupted);
            assert_eq!(notified.load(Ordering::SeqCst), 1);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        },
    );
}

#[test]
fn competing_failed_begins_do_not_interrupt_the_winning_guard() {
    let (_, _, context, _) = setup();
    let start = Arc::new(Barrier::new(9));
    let claimed = Arc::new(Barrier::new(9));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let context = context.clone();
        let start = start.clone();
        let claimed = claimed.clone();
        workers.push(std::thread::spawn(move || {
            start.wait();
            let execution = context.begin();
            claimed.wait();
            execution
        }));
    }
    start.wait();
    claimed.wait();
    let mut winners = Vec::new();
    for worker in workers {
        match worker.join().unwrap() {
            Ok(guard) => winners.push(guard),
            Err(error) => assert_eq!(error.code, "task_started"),
        }
    }
    assert_eq!(winners.len(), 1);
    assert_eq!(context.snapshot().state, TaskState::Running);
    winners.pop().unwrap().finish(Ok(json!(null))).unwrap();
}

#[test]
fn unsubscribe_during_delivery_preserves_captured_delivery_only() {
    let (_, _, context, _) = setup();
    let execution = context.begin().unwrap();
    let later = Arc::new(Mutex::new(None));
    let remove = later.clone();
    let _first = context.subscribe(Arc::new(move |snapshot: TaskSnapshot| {
        if snapshot.progress.completed.get() == 1 {
            drop(remove.lock().unwrap().take());
        }
    }));
    let calls = Arc::new(AtomicUsize::new(0));
    let received = calls.clone();
    *later.lock().unwrap() = Some(context.subscribe(Arc::new(move |snapshot: TaskSnapshot| {
        if snapshot.progress.completed.get() != 0 {
            received.fetch_add(1, Ordering::SeqCst);
        }
    })));
    for completed in 1..=2 {
        context
            .progress(Progress {
                completed: completed.into(),
                total: None,
                message: None,
            })
            .unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    execution.finish(Ok(json!(null))).unwrap();
}

#[test]
fn observers_receive_independent_owned_updates_and_legacy_outcomes() {
    let (_, _, context, _) = setup();
    context.configure_updates(json!({"type":"string"})).unwrap();
    let execution = context.begin().unwrap();
    let expected = "x".repeat(1024);
    let _mutating = context.subscribe(Arc::new(|mut snapshot: TaskSnapshot| {
        if let Some(update) = &mut snapshot.update {
            update.value = json!("observer-local edit");
        }
        snapshot.outcomes.clear();
    }));
    let expected_by_observer = expected.clone();
    let last = Arc::new(Mutex::new(None));
    let received = last.clone();
    let _inspecting = context.subscribe(Arc::new(move |snapshot: TaskSnapshot| {
        if let Some(update) = &snapshot.update {
            assert_eq!(update.value, json!(expected_by_observer));
        }
        *received.lock().unwrap() = Some(snapshot);
    }));
    context.publish(expected.clone()).unwrap();
    context
        .record_outcome(revenant_core::Outcome {
            index: 0,
            output: Some(json!({"value":42})),
            error: None,
        })
        .unwrap();
    assert_eq!(context.snapshot().outcomes.len(), 1);
    execution.finish(Ok(json!({"result": "kept"}))).unwrap();
    let delivered = last.lock().unwrap().take().unwrap();
    assert_eq!(delivered.outcomes.len(), 1);
    assert_eq!(delivered.result, Some(json!({"result":"kept"})));
    assert_eq!(delivered.update.unwrap().value, json!(expected));
}

#[test]
fn concurrent_publications_have_unique_revisions_and_exact_batch_counts() {
    let (_, _, context, _) = setup();
    let revisions = Arc::new(Mutex::new(Vec::new()));
    let seen = revisions.clone();
    let _observer = context.subscribe(Arc::new(move |snapshot: TaskSnapshot| {
        seen.lock().unwrap().push(snapshot.revision.get());
    }));
    let execution = context.begin().unwrap();
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let context = context.clone();
            std::thread::spawn(move || {
                for _ in 0..32 {
                    context.record_batch_outcome(true).unwrap();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let snapshot = execution.finish(Ok(json!(null))).unwrap();
    assert_eq!(snapshot.summary.completed.get(), 128);
    assert_eq!(snapshot.summary.succeeded.get(), 128);
    let mut seen = revisions.lock().unwrap().clone();
    seen.sort_unstable();
    assert_eq!(seen, (0..=snapshot.revision.get()).collect::<Vec<_>>());
}

#[test]
fn twenty_callback_failure_cycles_reclaim_created_resources() {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let tasks = TaskManager::new(resources.clone());
    let drops = Arc::new(AtomicUsize::new(0));
    for cycle in 0..20 {
        let context = tasks.create(&scope, "reused-after-removal").unwrap();
        let created = context.own(CountDrop(drops.clone())).unwrap();
        let _subscription = context.subscribe(Arc::new(|snapshot: TaskSnapshot| {
            if matches!(snapshot.state, TaskState::Running | TaskState::Cancelled) {
                panic!("cycle observer");
            }
        }));
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                if cycle % 2 == 0 {
                    let _ = context.begin();
                } else {
                    let _ = context.request_cancel();
                }
            }))
            .is_err()
        );
        assert!(context.snapshot().state.is_terminal());
        assert_eq!(drops.load(Ordering::SeqCst), cycle + 1);
        assert!(resources.lease_native(&scope, &created).is_err());
        tasks.remove(&scope, context.id()).unwrap();
        assert!(tasks.list(&scope).is_empty());
    }
    tasks.dispose_scope(&scope).unwrap();
}
