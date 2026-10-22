use revenant_core::{
    LatestJobs, LatestSubmission, Resource, ResourceRegistry, TaskContext, TaskManager,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

fn setup() -> (ResourceRegistry, TaskManager, String) {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let manager = TaskManager::new(resources.clone());
    (resources, manager, scope)
}
fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for latest request"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn distinct_requests_settle_without_publishing_superseded_inputs() {
    let (_, manager, scope) = setup();
    let creator = manager.create(&scope, "creator").unwrap();
    let state = Arc::new(Mutex::new((1u32, 0u32)));
    let (started, observed) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let wait = Arc::new(Mutex::new(wait));
    let jobs = Arc::new(
        LatestJobs::new(
            creator,
            state.clone(),
            2,
            move |value, context| {
                if value == 1 {
                    started.send(()).unwrap();
                    wait.lock().unwrap().recv().unwrap();
                }
                context.checkpoint()?;
                Ok(value)
            },
            |state| state.0,
            |state, value| {
                state.1 = value;
                Ok(())
            },
        )
        .unwrap(),
    );
    let first = manager.create(&scope, "first").unwrap();
    let active_jobs = jobs.clone();
    let active = std::thread::spawn(move || {
        let _execution = first.begin().unwrap();
        active_jobs.submit(&first, "scene", 1, 1)
    });
    observed.recv_timeout(Duration::from_secs(2)).unwrap();
    state.lock().unwrap().0 = 2;
    let second = manager.create(&scope, "second").unwrap();
    let second_jobs = jobs.clone();
    let (done, result) = mpsc::channel();
    let pending = std::thread::spawn(move || {
        let _execution = second.begin().unwrap();
        done.send(second_jobs.submit(&second, "scene", 2, 2))
            .unwrap();
    });
    wait_for(|| jobs.pending_count() == 1);
    assert!(matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty)));
    let third = manager.create(&scope, "third").unwrap();
    let third_jobs = jobs.clone();
    let newest = std::thread::spawn(move || {
        let _execution = third.begin().unwrap();
        third_jobs.submit(&third, "scene", 3, 2)
    });
    assert_eq!(
        result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap(),
        LatestSubmission::Superseded
    );
    pending.join().unwrap();
    assert_eq!(jobs.pending_count(), 1);
    release.send(()).unwrap();
    assert_eq!(
        active.join().unwrap().unwrap(),
        LatestSubmission::Superseded
    );
    assert_eq!(newest.join().unwrap().unwrap(), LatestSubmission::Committed);
    assert_eq!(*state.lock().unwrap(), (2, 3));
    assert_eq!(jobs.active_count(), 0);
    let stale = manager.create(&scope, "stale-version").unwrap();
    assert_eq!(
        jobs.submit(&stale, "scene", 4, 1).unwrap(),
        LatestSubmission::Superseded
    );
    assert_eq!(state.lock().unwrap().1, 3);
}

#[test]
fn controller_survives_creator_completion_and_rejects_other_owners() {
    let (resources, manager, scope) = setup();
    let creator = manager.create(&scope, "creator").unwrap();
    let execution = creator.begin().unwrap();
    let state = Arc::new(Mutex::new((1u32, 0u32)));
    let observed_task = Arc::new(Mutex::new(String::new()));
    let observed = observed_task.clone();
    let jobs = LatestJobs::new(
        creator,
        state.clone(),
        1,
        move |value, context| {
            *observed.lock().unwrap() = context.id().into();
            Ok(value)
        },
        |state| state.0,
        |state, value| {
            state.1 = value;
            Ok(())
        },
    )
    .unwrap();
    execution.finish(Ok(serde_json::Value::Null)).unwrap();
    let request = manager.create(&scope, "later-request").unwrap();
    let execution = request.begin().unwrap();
    assert_eq!(
        jobs.capture(&request, "scene", |_| Ok(7)).unwrap(),
        LatestSubmission::Committed
    );
    execution.finish(Ok(serde_json::Value::Null)).unwrap();
    assert_eq!(*observed_task.lock().unwrap(), "later-request");
    let other_scope = resources.create_scope(None).unwrap();
    let other = manager.create(&other_scope, "other-scope").unwrap();
    assert_eq!(
        jobs.submit(&other, "scene", 8, 1).unwrap_err().code,
        "scope_mismatch"
    );
    let (_, foreign_manager, foreign_scope) = setup();
    let foreign = foreign_manager
        .create(&foreign_scope, "foreign-host")
        .unwrap();
    assert_eq!(
        jobs.submit(&foreign, "scene", 9, 1).unwrap_err().code,
        "scope_mismatch"
    );
    let surviving = manager.create(&scope, "disposed-owner").unwrap();
    resources.dispose_scope(&scope).unwrap();
    assert!(
        jobs.submit(&surviving, "scene", 10, 1)
            .unwrap_err()
            .is_cancelled()
    );
    assert_eq!(state.lock().unwrap().1, 7);
}

#[test]
fn disposed_owner_cannot_publish() {
    let (resources, manager, scope) = setup();
    let task = manager.create(&scope, "disposed").unwrap();
    let state = Arc::new(Mutex::new((1u32, 0)));
    let close = resources.clone();
    let owner = scope.clone();
    let jobs = LatestJobs::new(
        task.clone(),
        state.clone(),
        1,
        move |value, _| {
            close.dispose_scope(&owner)?;
            Ok(value)
        },
        |state| state.0,
        |state, value| {
            state.1 = value;
            Ok(())
        },
    )
    .unwrap();
    assert!(jobs.submit(&task, "scene", 4, 1).is_err());
    assert_eq!(state.lock().unwrap().1, 0);
    assert_eq!(jobs.active_count(), 0);
}

#[test]
fn cancelling_pending_request_removes_it_without_running() {
    let (_, manager, scope) = setup();
    let creator = manager.create(&scope, "creator").unwrap();
    let (started, observed) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let wait = Arc::new(Mutex::new(wait));
    let computations = Arc::new(AtomicUsize::new(0));
    let counted = computations.clone();
    let jobs = Arc::new(
        LatestJobs::new(
            creator,
            Arc::new(Mutex::new((1u32, 0u32))),
            1,
            move |value, _| {
                counted.fetch_add(1, Ordering::SeqCst);
                started.send(()).unwrap();
                wait.lock().unwrap().recv().unwrap();
                Ok(value)
            },
            |state| state.0,
            |state, value| {
                state.1 = value;
                Ok(())
            },
        )
        .unwrap(),
    );
    let first = manager.create(&scope, "first").unwrap();
    let active_jobs = jobs.clone();
    let active = std::thread::spawn(move || {
        let _execution = first.begin().unwrap();
        active_jobs.submit(&first, "key", 1, 1)
    });
    observed.recv_timeout(Duration::from_secs(2)).unwrap();
    let second = manager.create(&scope, "cancel-pending").unwrap();
    let cancel = second.clone();
    let pending_jobs = jobs.clone();
    let (done, result) = mpsc::channel();
    let pending = std::thread::spawn(move || {
        let _execution = second.begin().unwrap();
        done.send(pending_jobs.submit(&second, "key", 2, 1))
            .unwrap();
    });
    wait_for(|| jobs.pending_count() == 1);
    cancel.request_cancel().unwrap();
    assert!(
        result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    pending.join().unwrap();
    assert_eq!(jobs.pending_count(), 0);
    assert_eq!(computations.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    assert_eq!(
        active.join().unwrap().unwrap(),
        LatestSubmission::Superseded
    );
}

struct Tracked(Arc<AtomicUsize>);
impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn cancelling_started_pending_request_waits_for_compute_and_keeps_its_leases() {
    let (resources, manager, scope) = setup();
    let creator = manager.create(&scope, "creator").unwrap();
    let dropped = Arc::new(AtomicUsize::new(0));
    let resource = resources
        .register_native(&scope, Tracked(dropped.clone()))
        .unwrap();
    let (started, observed) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let wait = Arc::new(Mutex::new(wait));
    let jobs = Arc::new(
        LatestJobs::new(
            creator,
            Arc::new(Mutex::new((1u32, 0u32))),
            1,
            move |(value, resource): (u32, Option<Resource<Tracked>>), context: TaskContext| {
                let retained = resource
                    .as_ref()
                    .map(|resource| context.lease_resource(resource).unwrap());
                started.send((value, context.id().to_owned())).unwrap();
                wait.lock().unwrap().recv().unwrap();
                drop(retained);
                context.checkpoint()?;
                Ok(value)
            },
            |state| state.0,
            |state, value| {
                state.1 = value;
                Ok(())
            },
        )
        .unwrap(),
    );
    let first = manager.create(&scope, "first").unwrap();
    let active_jobs = jobs.clone();
    let active = std::thread::spawn(move || {
        let _execution = first.begin().unwrap();
        active_jobs.submit(&first, "key", (1, None), 1)
    });
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(2)).unwrap(),
        (1, "first".into())
    );
    let second = manager.create(&scope, "pending-lease").unwrap();
    second.admit_handle(resource.handle()).unwrap();
    let cancel = second.clone();
    let pending_jobs = jobs.clone();
    let input_resource = resource.clone();
    let (done, result) = mpsc::channel();
    let pending = std::thread::spawn(move || {
        let execution = second.begin().unwrap();
        let outcome = pending_jobs.submit(&second, "key", (2, Some(input_resource)), 1);
        execution
            .finish(outcome.clone().map(|_| serde_json::Value::Null))
            .unwrap();
        done.send(outcome).unwrap();
    });
    wait_for(|| jobs.pending_count() == 1);
    resources.release(&scope, resource.handle()).unwrap();
    release.send(()).unwrap();
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(2)).unwrap(),
        (2, "pending-lease".into())
    );
    cancel.request_cancel().unwrap();
    assert!(matches!(
        result.recv_timeout(Duration::from_millis(30)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert!(!cancel.snapshot().state.is_terminal());
    release.send(()).unwrap();
    assert!(
        result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    pending.join().unwrap();
    assert_eq!(
        active.join().unwrap().unwrap(),
        LatestSubmission::Superseded
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(jobs.active_count(), 0);
}

fn cancellation_while_waiting_to_commit(parent_cancel: bool) {
    use std::sync::atomic::AtomicBool;
    let (_, manager, scope) = setup();
    let parent = manager.create(&scope, "contention-parent").unwrap();
    let _parent_execution = parent.begin().unwrap();
    let task = if parent_cancel {
        parent.batch_child(0, 1).unwrap()
    } else {
        manager.create(&scope, "contention-host").unwrap()
    };
    let state = Arc::new(Mutex::new((1u32, 0u32)));
    let host = Arc::new(AtomicBool::new(false));
    let computed = Arc::new(AtomicBool::new(false));
    let notified = AtomicBool::new(false);
    let (polled, polls) = mpsc::channel();
    let signal = computed.clone();
    let flag = host.clone();
    task.set_cancellation_probe(Arc::new(move || {
        let cancelled = flag.load(Ordering::SeqCst);
        if signal.load(Ordering::SeqCst) && !notified.swap(true, Ordering::SeqCst) {
            polled.send(()).unwrap();
        }
        cancelled
    }))
    .unwrap();
    let (entered, entry) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let jobs = Arc::new(
        LatestJobs::new(
            task.clone(),
            state.clone(),
            1,
            move |value, _| {
                entered.send(()).unwrap();
                gate.lock().unwrap().recv().unwrap();
                computed.store(true, Ordering::SeqCst);
                Ok(value)
            },
            |state| state.0,
            |state, value| {
                state.1 = value;
                Ok(())
            },
        )
        .unwrap(),
    );
    let worker_jobs = jobs.clone();
    let (done, result) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _execution = task.begin().unwrap();
        let outcome = worker_jobs.submit(&task, "scene", 7, 1);
        done.send(outcome.clone()).unwrap();
        outcome
    });
    entry.recv_timeout(Duration::from_secs(2)).unwrap();
    let application = state.lock().unwrap();
    release.send(()).unwrap();
    polls.recv_timeout(Duration::from_secs(2)).unwrap();
    if parent_cancel {
        parent.request_cancel().unwrap();
    } else {
        host.store(true, Ordering::SeqCst);
    }
    let before_unlock = result.recv_timeout(Duration::from_secs(2));
    drop(application);
    let eventual = worker.join().unwrap();
    assert!(
        before_unlock.is_ok(),
        "cancellation must settle without acquiring application state: {eventual:?}"
    );
    assert!(before_unlock.unwrap().unwrap_err().is_cancelled());
    assert_eq!(*state.lock().unwrap(), (1, 0));
    assert_eq!(jobs.active_count(), 0);
    assert_eq!(jobs.pending_count(), 0);
}

#[test]
fn host_cancellation_settles_during_commit_lock_contention() {
    cancellation_while_waiting_to_commit(false);
}

#[test]
fn parent_cancellation_settles_during_commit_lock_contention() {
    cancellation_while_waiting_to_commit(true);
}

#[test]
fn commit_cancellation_observer_can_read_application_state() {
    let (_, manager, scope) = setup();
    let task = manager.create(&scope, "commit-cancel").unwrap();
    let state = Arc::new(Mutex::new((1u32, 0u32)));
    let computed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal = computed.clone();
    let jobs = LatestJobs::new(
        task.clone(),
        state.clone(),
        1,
        move |value, _| {
            signal.store(true, Ordering::SeqCst);
            Ok(value)
        },
        |state| state.0,
        |state, value| {
            state.1 = value;
            Ok(())
        },
    )
    .unwrap();
    let execution = task.begin().unwrap();
    let observed = Arc::new(AtomicUsize::new(0));
    let count = observed.clone();
    let application = state.clone();
    let _subscription = task.subscribe(Arc::new(move |snapshot: revenant_core::TaskSnapshot| {
        if snapshot.state == revenant_core::TaskState::Cancelling {
            let _state = application
                .try_lock()
                .expect("cancellation observer must run outside application lock");
            count.fetch_add(1, Ordering::SeqCst);
        }
    }));
    task.set_cancellation_probe(Arc::new(move || computed.load(Ordering::SeqCst)))
        .unwrap();
    let outcome = jobs.submit(&task, "scene", 7, 1);
    assert!(outcome.as_ref().unwrap_err().is_cancelled());
    assert!(observed.load(Ordering::SeqCst) > 0);
    assert_eq!(*state.lock().unwrap(), (1, 0));
    assert_eq!(jobs.active_count(), 0);
    execution
        .finish(outcome.map(|_| serde_json::Value::Null))
        .unwrap();
}

#[test]
fn cancellation_probe_can_observe_latest_counters() {
    let (_, manager, scope) = setup();
    let task = manager.create(&scope, "reentry").unwrap();
    let jobs = Arc::new(
        LatestJobs::new(
            task.clone(),
            Arc::new(Mutex::new((1u32, 0u32))),
            1,
            |value, _| Ok(value),
            |state| state.0,
            |state, value| {
                state.1 = value;
                Ok(())
            },
        )
        .unwrap(),
    );
    let weak = Arc::downgrade(&jobs);
    task.set_cancellation_probe(Arc::new(move || {
        if let Some(jobs) = weak.upgrade() {
            let _ = jobs.pending_count();
        }
        false
    }))
    .unwrap();
    let (done, observed) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(jobs.submit(&task, "key", 3, 1));
    });
    assert_eq!(
        observed
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap(),
        LatestSubmission::Committed
    );
}

#[test]
fn panicking_driver_settles_pending_request_and_reclaims_key() {
    let (_, manager, scope) = setup();
    let creator = manager.create(&scope, "creator").unwrap();
    let (started, observed) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let wait = Arc::new(Mutex::new(wait));
    let jobs = Arc::new(
        LatestJobs::new(
            creator,
            Arc::new(Mutex::new((1u32, 0u32))),
            1,
            move |value, _| {
                if value == 1 {
                    started.send(()).unwrap();
                    wait.lock().unwrap().recv().unwrap();
                    panic!("fixture compute failure");
                }
                Ok(value)
            },
            |state| state.0,
            |state, value| {
                state.1 = value;
                Ok(())
            },
        )
        .unwrap(),
    );
    let first = manager.create(&scope, "first").unwrap();
    let active_jobs = jobs.clone();
    let active = std::thread::spawn(move || {
        let _execution = first.begin().unwrap();
        active_jobs.submit(&first, "scene", 1, 1)
    });
    observed.recv_timeout(Duration::from_secs(2)).unwrap();
    let second = manager.create(&scope, "pending").unwrap();
    let pending_jobs = jobs.clone();
    let (done, result) = mpsc::channel();
    let pending = std::thread::spawn(move || {
        let _execution = second.begin().unwrap();
        done.send(pending_jobs.submit(&second, "scene", 2, 1))
            .unwrap();
    });
    wait_for(|| jobs.pending_count() == 1);
    release.send(()).unwrap();
    assert!(active.join().is_err());
    assert_eq!(
        result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap_err()
            .code,
        "latest_interrupted"
    );
    pending.join().unwrap();
    assert_eq!(jobs.active_count(), 0);
    assert_eq!(jobs.pending_count(), 0);
    let later = manager.create(&scope, "later").unwrap();
    assert_eq!(
        jobs.submit(&later, "scene", 3, 1).unwrap(),
        LatestSubmission::Committed
    );
}
