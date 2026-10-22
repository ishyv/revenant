use revenant_core::{
    ApplicationPaths, CancellationToken, Error, ResourceRegistry, Result, TaskContext, TaskManager,
    TaskServices, WorkClass, WorkJob, WorkLimits, WorkService, WorkStats,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

struct InlineBudget {
    limits: WorkLimits,
    budget: usize,
    retained: Arc<AtomicUsize>,
    admissions: AtomicUsize,
}

impl InlineBudget {
    fn new(limits: WorkLimits, budget: usize) -> Arc<Self> {
        Arc::new(Self {
            limits,
            budget,
            retained: Arc::new(AtomicUsize::new(0)),
            admissions: AtomicUsize::new(0),
        })
    }
}

impl WorkService for InlineBudget {
    fn limits(&self) -> WorkLimits {
        self.limits
    }

    fn reserve_scratch(&self, bytes: usize) -> Result<Box<dyn Send>> {
        self.retained
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |retained| {
                retained
                    .checked_add(bytes)
                    .filter(|sum| *sum <= self.budget)
            })
            .map_err(|_| Error::new("work_scratch_limit", "Fixture budget exhausted"))?;
        self.admissions.fetch_add(1, Ordering::SeqCst);
        struct Reservation(Arc<AtomicUsize>, usize);
        impl Drop for Reservation {
            fn drop(&mut self) {
                self.0.fetch_sub(self.1, Ordering::SeqCst);
            }
        }
        Ok(Box::new(Reservation(self.retained.clone(), bytes)))
    }

    fn execute(
        &self,
        _owner: &str,
        _class: WorkClass,
        cancellation: CancellationToken,
        jobs: Vec<WorkJob>,
    ) -> Result<()> {
        // Deliberately settle in reverse order; the helper must publish in input order.
        for job in jobs.into_iter().rev() {
            cancellation.checkpoint()?;
            job()?;
        }
        Ok(())
    }

    fn stats(&self) -> WorkStats {
        WorkStats {
            scratch_bytes: self.retained.load(Ordering::SeqCst),
            ..WorkStats::default()
        }
    }
}

fn task_with_work(service: Arc<dyn WorkService>) -> TaskContext {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    TaskManager::with_services(resources, Arc::new(TaskServices::new(None, Some(service))))
        .create(&scope, "fixture-work")
        .unwrap()
}

#[test]
fn resolved_paths_are_immutable_services_and_children_inherit_them() {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let paths = ApplicationPaths {
        app_id: "fixture.paths".into(),
        data_dir: PathBuf::from("data-override"),
        cache_dir: PathBuf::from("cache-override"),
    };
    let manager = TaskManager::with_services(
        resources,
        Arc::new(TaskServices::new(Some(paths.clone()), None)),
    );
    let task = manager.create(&scope, "paths").unwrap();
    assert_eq!(task.application_paths().unwrap(), &paths);
    let child = task.batch_child(0, 1).unwrap();
    assert_eq!(child.application_paths().unwrap(), &paths);
    assert_eq!(child.work().err().unwrap().code, "work_unavailable");
}

#[test]
fn token_observes_parent_cancel_and_scope_disposal() {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let manager = TaskManager::new(resources.clone());
    let task = manager.create(&scope, "cancel").unwrap();
    let execution = task.begin().unwrap();
    let child = task.batch_child(0, 1).unwrap();
    let token = child.cancellation();
    assert!(!token.is_cancelled());
    task.request_cancel().unwrap();
    assert!(token.is_cancelled());
    drop(execution);
    let other = manager.create(&scope, "dispose").unwrap();
    let token = other.cancellation();
    resources.dispose_scope(&scope).unwrap();
    assert!(token.is_cancelled());
}

#[test]
fn rejected_scratch_does_not_advance_lazy_input() {
    struct RejectScratch;
    impl WorkService for RejectScratch {
        fn reserve_scratch(&self, _bytes: usize) -> Result<Box<dyn Send>> {
            Err(Error::new("work_scratch_limit", "Fixture budget exhausted"))
        }

        fn execute(
            &self,
            _owner: &str,
            _class: WorkClass,
            _cancellation: CancellationToken,
            _jobs: Vec<WorkJob>,
        ) -> Result<()> {
            panic!("Rejected work must not execute")
        }

        fn stats(&self) -> WorkStats {
            WorkStats::default()
        }
    }

    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let manager = TaskManager::with_services(
        resources,
        Arc::new(TaskServices::new(None, Some(Arc::new(RejectScratch)))),
    );
    let task = manager.create(&scope, "rejected-scratch").unwrap();
    let advanced = AtomicUsize::new(0);
    let input = std::iter::from_fn(|| {
        advanced.fetch_add(1, Ordering::SeqCst);
        Some([0u8; 1024])
    });
    let result = task.work().unwrap().stream_ordered(
        WorkClass::Cpu,
        input,
        |item, _| Ok(item),
        |_| panic!("Rejected work must not publish"),
    );

    assert_eq!(result.unwrap_err().code, "work_scratch_limit");
    assert_eq!(advanced.load(Ordering::SeqCst), 0);
}

#[test]
fn scratch_stays_admitted_through_consumer_settlement() {
    let bytes = 4 * (size_of::<u32>() + size_of::<Option<u32>>());
    let service = InlineBudget::new(
        WorkLimits {
            window_items: 4,
            scratch_bytes: bytes,
            result_bytes: 128,
        },
        bytes,
    );
    let first = task_with_work(service.clone());
    let second = task_with_work(service.clone());
    let (started, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let observations = service.clone();
    let worker = std::thread::spawn(move || {
        let mut consumed = Vec::new();
        first
            .work()
            .unwrap()
            .stream_ordered(
                WorkClass::Cpu,
                (1..=4u32).inspect(|_| assert_eq!(observations.stats().scratch_bytes, bytes)),
                |item, _| Ok(item * 2),
                |item| {
                    assert_eq!(observations.stats().scratch_bytes, bytes);
                    if consumed.is_empty() {
                        started.send(()).unwrap();
                        wait.recv().unwrap();
                    }
                    consumed.push(item);
                    Ok(())
                },
            )
            .unwrap();
        consumed
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let advanced = AtomicUsize::new(0);
    let result = second.work().unwrap().stream_ordered(
        WorkClass::Cpu,
        std::iter::from_fn(|| {
            advanced.fetch_add(1, Ordering::SeqCst);
            Some(9u32)
        }),
        |item, _| Ok(item),
        |_| panic!("Contended window must not publish"),
    );
    release.send(()).unwrap();
    assert_eq!(worker.join().unwrap(), vec![2, 4, 6, 8]);
    assert_eq!(result.unwrap_err().code, "work_scratch_limit");
    assert_eq!(advanced.load(Ordering::SeqCst), 0);
    assert_eq!(service.stats().scratch_bytes, 0);
}

#[test]
fn exact_empty_and_small_inputs_do_not_reserve_a_full_window() {
    let bytes = size_of::<u32>() + size_of::<Option<u64>>();
    let service = InlineBudget::new(WorkLimits::default(), bytes);
    let work = task_with_work(service.clone()).work().unwrap();
    let empty: Vec<u64> = work
        .map_ordered(WorkClass::Cpu, std::iter::empty::<u32>(), |item, _| {
            Ok(u64::from(item))
        })
        .unwrap();
    assert!(empty.is_empty());
    assert_eq!(service.admissions.load(Ordering::SeqCst), 0);
    assert_eq!(
        work.map_ordered(WorkClass::Cpu, [42u32], |item, _| Ok(u64::from(item)))
            .unwrap(),
        vec![42u64]
    );
    assert_eq!(service.admissions.load(Ordering::SeqCst), 1);
    assert_eq!(service.stats().scratch_bytes, 0);
}

#[test]
fn exact_empty_input_succeeds_with_zero_scratch_capacity() {
    let service = InlineBudget::new(
        WorkLimits {
            scratch_bytes: 0,
            ..WorkLimits::default()
        },
        0,
    );
    let work = task_with_work(service.clone()).work().unwrap();
    let result = work
        .map_ordered(WorkClass::Cpu, std::iter::empty::<u32>(), |item, _| {
            Ok(item)
        })
        .unwrap();
    assert!(result.is_empty());
    assert_eq!(service.admissions.load(Ordering::SeqCst), 0);
    assert_eq!(
        work.map_ordered(WorkClass::Cpu, [1u32], |item, _| Ok(item))
            .unwrap_err()
            .code,
        "work_scratch_limit"
    );
}

#[test]
fn exact_empty_input_succeeds_when_one_item_exceeds_scratch_capacity() {
    let service = InlineBudget::new(
        WorkLimits {
            scratch_bytes: 1,
            ..WorkLimits::default()
        },
        1,
    );
    let work = task_with_work(service.clone()).work().unwrap();
    let result = work
        .map_ordered(WorkClass::Cpu, std::iter::empty::<u64>(), |item, _| {
            Ok(item)
        })
        .unwrap();
    assert!(result.is_empty());
    assert_eq!(service.admissions.load(Ordering::SeqCst), 0);
    assert_eq!(
        work.map_ordered(WorkClass::Cpu, [1u64], |item, _| Ok(item))
            .unwrap_err()
            .code,
        "work_scratch_limit"
    );
}

#[test]
fn exact_empty_input_still_validates_window_configuration() {
    let service = InlineBudget::new(
        WorkLimits {
            window_items: 0,
            ..WorkLimits::default()
        },
        0,
    );
    let result = task_with_work(service).work().unwrap().map_ordered(
        WorkClass::Cpu,
        std::iter::empty::<u32>(),
        |item, _| Ok(item),
    );
    assert_eq!(result.unwrap_err().code, "invalid_work_limits");
}

#[test]
fn exact_empty_input_still_observes_cancellation() {
    let service = InlineBudget::new(
        WorkLimits {
            scratch_bytes: 1,
            ..WorkLimits::default()
        },
        1,
    );
    let task = task_with_work(service.clone());
    let _execution = task.begin().unwrap();
    let work = task.work().unwrap();
    task.request_cancel().unwrap();
    let result = work.map_ordered(WorkClass::Cpu, std::iter::empty::<u64>(), |item, _| {
        Ok(item)
    });
    assert!(result.unwrap_err().is_cancelled());
    assert_eq!(service.admissions.load(Ordering::SeqCst), 0);
}

#[test]
fn cancellation_during_materialization_releases_scratch_without_execution() {
    let service = InlineBudget::new(WorkLimits::default(), 8 * 1024 * 1024);
    let task = task_with_work(service.clone());
    let _execution = task.begin().unwrap();
    let input = std::iter::from_fn(|| {
        task.request_cancel().unwrap();
        Some(1u32)
    });
    let result = task.work().unwrap().stream_ordered(
        WorkClass::Cpu,
        input,
        |_, _| panic!("Cancelled preparation must not execute"),
        |_: u32| panic!("Cancelled preparation must not publish"),
    );
    assert!(result.unwrap_err().is_cancelled());
    assert_eq!(service.stats().scratch_bytes, 0);
}

#[test]
fn ordered_result_limit_releases_window_scratch() {
    let service = InlineBudget::new(
        WorkLimits {
            result_bytes: 3 * size_of::<u32>(),
            ..WorkLimits::default()
        },
        8 * 1024 * 1024,
    );
    let result = task_with_work(service.clone()).work().unwrap().map_ordered(
        WorkClass::Cpu,
        0..4u32,
        |item, _| Ok(item),
    );
    assert_eq!(result.unwrap_err().code, "work_result_limit");
    assert_eq!(service.stats().scratch_bytes, 0);
}

#[test]
fn zero_sized_items_do_not_overflow_inline_capacity_accounting() {
    let service = InlineBudget::new(
        WorkLimits {
            window_items: usize::MAX,
            scratch_bytes: usize::MAX,
            result_bytes: usize::MAX,
        },
        3 * size_of::<Option<()>>(),
    );
    let result = task_with_work(service.clone())
        .work()
        .unwrap()
        .map_ordered(WorkClass::Cpu, [(); 3], |item, _| Ok(item))
        .unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(service.stats().scratch_bytes, 0);
}
