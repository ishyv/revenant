use super::{Samples, allocations};
use revenant_core::{Progress, ResourceRegistry, TaskManager, TaskSnapshot};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

pub(super) fn spec(id: &str) -> (usize, usize) {
    let (bytes, listeners) = id
        .strip_prefix("task.progress.bytes")
        .unwrap()
        .split_once(".listeners")
        .unwrap();
    (bytes.parse().unwrap(), listeners.parse().unwrap())
}
struct Observed {
    calls: AtomicU64,
    revision: AtomicU64,
    completed: AtomicU64,
}

fn exercise(
    bytes: usize,
    observers: usize,
    iterations: u64,
    counted: bool,
    timed: bool,
) -> (allocations::Counts, u64) {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let tasks = TaskManager::new(resources.clone());
    // Match the earlier diagnosis's constant task identity; UUID scope lengths are fixed.
    let context = tasks.create(&scope, "counter").unwrap();
    context.configure_updates(json!({"type":"string"})).unwrap();
    let execution = context.begin().unwrap();
    if bytes != 0 {
        context.publish("x".repeat(bytes)).unwrap();
    }
    let initial_revision = context.snapshot().revision.get();
    let mut observed = Vec::with_capacity(observers);
    let mut subscriptions = Vec::with_capacity(observers);
    for _ in 0..observers {
        let state = Arc::new(Observed {
            calls: AtomicU64::new(0),
            revision: AtomicU64::new(0),
            completed: AtomicU64::new(0),
        });
        let seen = state.clone();
        subscriptions.push(context.subscribe(Arc::new(move |snapshot: TaskSnapshot| {
            if let Some(update) = &snapshot.update {
                let value = update.value.as_str().unwrap();
                assert_eq!(value.len(), bytes);
                assert_eq!(value.as_bytes().first(), Some(&b'x'));
                assert_eq!(value.as_bytes().last(), Some(&b'x'));
                assert_eq!(update.revision.get(), 2);
            } else {
                assert_eq!(bytes, 0);
            }
            assert!(snapshot.outcomes.is_empty());
            let previous = seen
                .revision
                .swap(snapshot.revision.get(), Ordering::Relaxed);
            assert!(previous < snapshot.revision.get());
            seen.completed
                .store(snapshot.progress.completed.get(), Ordering::Relaxed);
            seen.calls.fetch_add(1, Ordering::Relaxed);
        })));
        observed.push(state);
    }
    let window = counted.then(allocations::Window::start);
    let start = timed.then(std::time::Instant::now);
    for completed in 1..=iterations {
        context
            .progress(Progress {
                completed: completed.into(),
                total: Some(iterations.into()),
                message: None,
            })
            .unwrap();
    }
    let elapsed = start.map_or(0, |start| start.elapsed().as_nanos() as u64);
    let counts = window.map_or_else(allocations::Counts::default, allocations::Window::stop);
    let snapshot = context.snapshot();
    assert_eq!(snapshot.revision.get(), initial_revision + iterations);
    assert_eq!(snapshot.progress.completed.get(), iterations);
    let expected_update = (bytes != 0).then(|| json!("x".repeat(bytes)));
    assert_eq!(
        snapshot.update.as_ref().map(|update| &update.value),
        expected_update.as_ref()
    );
    for state in observed {
        assert_eq!(state.calls.load(Ordering::Relaxed), iterations + 1);
        assert_eq!(
            state.revision.load(Ordering::Relaxed),
            snapshot.revision.get()
        );
        assert_eq!(state.completed.load(Ordering::Relaxed), iterations);
    }
    execution.finish(Ok(json!(iterations))).unwrap();
    drop(subscriptions);
    tasks.remove(&scope, context.id()).unwrap();
    assert!(tasks.list(&scope).is_empty());
    resources.dispose_scope(&scope).unwrap();
    (counts, elapsed)
}

pub(super) fn run(id: &str, measure: bool, diagnose: bool, samples: &mut Samples) -> Value {
    let (bytes, observers) = spec(id);
    let iterations = if measure { 5000 } else { 100 };
    if measure || diagnose {
        let (counts, _) = exercise(bytes, observers, iterations, true, false);
        if diagnose {
            samples.observations.push(
                json!({"kind":"allocation-diagnosis","iterations":iterations,
                "allocationCalls":counts.calls,"cumulativeRequestedBytes":counts.requested_bytes}),
            );
        } else {
            samples.push("allocation_calls", counts.calls);
            samples.push("cumulative_requested_bytes", counts.requested_bytes);
        }
    }
    if !diagnose {
        let (_, elapsed) = exercise(bytes, observers, iterations, false, samples.enabled);
        samples.push("progress_publication_total", elapsed);
        samples.push(
            "retained_update_json",
            if bytes == 0 { 0 } else { bytes as u64 + 2 },
        );
    }
    json!({"passed":true,"progressCalls":iterations,"observers":observers,
        "ownedUpdateCharacters":bytes,"exactLatestUpdateAndRevisions":true,"historyAfterRemoval":0})
}
