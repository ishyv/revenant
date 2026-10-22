use revenant_core::{ResourceRegistry, TaskManager, TaskServices, WorkClass, WorkService};
use revenant_desktop::NativeWorkService;
use std::sync::Arc;

#[test]
fn admission_probe_panic_settles_admitted_work() {
    use revenant_core::WorkJob;
    use std::{
        panic::{AssertUnwindSafe, catch_unwind, panic_any},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        time::Duration,
    };
    struct ReleaseOnDrop(mpsc::Sender<()>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let service = NativeWorkService::new(1, 1, 2).unwrap();
    let resources = ResourceRegistry::new();
    let owner = resources.create_scope(None).unwrap();
    let manager = TaskManager::new(resources);
    let task = manager.create(&owner, "probe-panic").unwrap();
    let _execution = task.begin().unwrap();
    let caller = std::thread::current().id();
    let calls = AtomicUsize::new(0);
    let (release, gate) = mpsc::channel();
    task.set_cancellation_probe(Arc::new(move || {
        if std::thread::current().id() == caller && calls.fetch_add(1, Ordering::SeqCst) == 1 {
            panic_any(ReleaseOnDrop(release.clone()));
        }
        false
    }))
    .unwrap();
    let finished = Arc::new(AtomicBool::new(false));
    let mark = finished.clone();
    let (completed, observed) = mpsc::channel();
    let jobs: Vec<WorkJob> = vec![
        Box::new(move || {
            gate.recv().unwrap();
            mark.store(true, Ordering::SeqCst);
            completed.send(()).unwrap();
            Ok(())
        }),
        Box::new(|| Ok(())),
    ];
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        service.execute(&owner, WorkClass::Cpu, task.cancellation(), jobs)
    }));
    let finished_at_return = finished.load(Ordering::SeqCst);
    let classified = matches!(&outcome, Ok(Err(error)) if error.code == "work_panicked");
    drop(outcome); // Release the gate on the buggy unwind path too.
    observed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(classified, "checkpoint panic escaped execute");
    assert!(
        finished_at_return,
        "admitted work survived execute's return"
    );
    assert_eq!(service.stats().queued, 0);
    assert_eq!(service.stats().cpu_active, 0);
}

#[test]
fn nested_budget_one_and_ordered_stream_complete() {
    let service = Arc::new(NativeWorkService::new(1, 1, 2).unwrap());
    let resources = ResourceRegistry::new();
    let owner = resources.create_scope(None).unwrap();
    let manager = TaskManager::with_services(
        resources,
        Arc::new(TaskServices::new(None, Some(service.clone()))),
    );
    let task = manager.create(&owner, "nested").unwrap();
    let _execution = task.begin().unwrap();
    let nested = task.work().unwrap();
    let result = nested
        .clone()
        .map_ordered(WorkClass::Cpu, 0..10u32, move |value, token| {
            token.checkpoint()?;
            let inner = nested.map_reduce(
                WorkClass::Cpu,
                0..3u32,
                move |item, _| Ok(value + item),
                0,
                |sum, item| {
                    *sum += item;
                    Ok(())
                },
            )?;
            Ok(inner)
        })
        .unwrap();
    assert_eq!(
        result,
        (0..10).map(|value| 3 * value + 3).collect::<Vec<_>>()
    );
    let stats = service.stats();
    assert_eq!(stats.cpu_active, 0);
    assert!(stats.cpu_peak <= 1);
    assert_eq!(stats.queued, 0);
}

#[test]
fn cancelling_a_window_joins_started_chunks() {
    let service = Arc::new(NativeWorkService::new(2, 2, 2).unwrap());
    let resources = ResourceRegistry::new();
    let owner = resources.create_scope(None).unwrap();
    let manager = TaskManager::with_services(
        resources,
        Arc::new(TaskServices::new(None, Some(service.clone()))),
    );
    let task = manager.create(&owner, "cancel").unwrap();
    let _execution = task.begin().unwrap();
    let cancel = task.clone();
    let result = task
        .work()
        .unwrap()
        .map_ordered(WorkClass::Io, 0..100, move |value, token| {
            if value == 0 {
                cancel.request_cancel()?;
            }
            token.checkpoint()?;
            Ok(value)
        });
    assert!(result.is_err());
    assert_eq!(service.stats().io_active, 0);
    assert_eq!(service.stats().queued, 0);
}

#[test]
fn same_owner_interactive_work_bypasses_blocked_cpu_class() {
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;
    let service = Arc::new(NativeWorkService::new(2, 1, 16).unwrap());
    let resources = ResourceRegistry::new();
    let owner = resources.create_scope(None).unwrap();
    let manager = TaskManager::with_services(
        resources,
        Arc::new(TaskServices::new(None, Some(service.clone()))),
    );
    let bulk = manager.create(&owner, "bulk").unwrap();
    let interactive = manager.create(&owner, "interactive").unwrap();
    interactive
        .configure_work_class(WorkClass::Interactive)
        .unwrap();
    let (started, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let wait = Arc::new(Mutex::new(wait));
    let worker = std::thread::spawn(move || {
        bulk.work()
            .unwrap()
            .map_ordered(WorkClass::Cpu, 0..32, move |value, _| {
                if value == 0 {
                    started.send(()).unwrap();
                    wait.lock().unwrap().recv().unwrap();
                }
                Ok(value)
            })
    });
    ready.recv().unwrap();
    let (done, observed) = mpsc::channel();
    let read = std::thread::spawn(move || {
        done.send(interactive.work().unwrap().map_ordered(
            WorkClass::Interactive,
            [7],
            |value, _| Ok(value),
        ))
        .unwrap();
    });
    let result = observed.recv_timeout(Duration::from_millis(300));
    if result.is_err() {
        eprintln!("blocked interactive: {:?}", service.stats());
    }
    release.send(()).unwrap();
    worker.join().unwrap().unwrap();
    read.join().unwrap();
    assert_eq!(result.unwrap().unwrap(), vec![7]);
}

#[test]
fn queued_cancel_settles_without_a_heavy_permit() {
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;
    let service = Arc::new(NativeWorkService::new(2, 1, 16).unwrap());
    let resources = ResourceRegistry::new();
    let owner = resources.create_scope(None).unwrap();
    let manager = TaskManager::with_services(
        resources,
        Arc::new(TaskServices::new(None, Some(service.clone()))),
    );
    let bulk = manager.create(&owner, "blocker").unwrap();
    let queued = manager.create(&owner, "queued").unwrap();
    let cancel = queued.clone();
    let (started, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let wait = Arc::new(Mutex::new(wait));
    let worker = std::thread::spawn(move || {
        bulk.work()
            .unwrap()
            .map_ordered(WorkClass::Cpu, [0], move |value, _| {
                started.send(()).unwrap();
                wait.lock().unwrap().recv().unwrap();
                Ok(value)
            })
    });
    ready.recv().unwrap();
    let (done, observed) = mpsc::channel();
    let read = std::thread::spawn(move || {
        done.send(
            queued
                .work()
                .unwrap()
                .map_ordered(WorkClass::Cpu, 0..32, |value, _| Ok(value)),
        )
        .unwrap();
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while service.stats().queued == 0 && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    cancel.request_cancel().unwrap();
    let result = observed.recv_timeout(Duration::from_millis(300));
    release.send(()).unwrap();
    worker.join().unwrap().unwrap();
    read.join().unwrap();
    assert!(result.unwrap().is_err());
    assert_eq!(service.stats().queued, 0);
}
