#[path = "support/mod.rs"]
mod support;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduling_comparison_keeps_results_equal_and_metrics_separate() {
    let report = support::benchmark::compare(support::benchmark::ComparisonOptions {
        items: 8,
        cpu_iterations: 100_000,
        file_bytes: 256 * 1024,
        repeats: 1,
        workers: 2,
    })
    .await;
    assert_eq!(report["schemaVersion"], 2);
    assert_eq!(report["comparison"], "current scheduling declarations");
    let runs = report["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 4);
    for run in runs {
        assert_eq!(run["durationBasis"], "native terminal publication");
        assert!(
            run["observationMilliseconds"].as_f64().unwrap()
                >= run["milliseconds"].as_f64().unwrap()
        );
        assert_eq!(run["completed"], "8");
        assert_eq!(run["failed"], "0");
        assert!(run["controlResponsiveness"]["samples"].as_u64().unwrap() > 0);
        assert!(run["nativeExecution"]["maximumObserved"].as_u64().unwrap() <= 2);
        assert!(run["memory"]["samplingIntervalMilliseconds"].is_number());
        if run["policy"] == "sequential" {
            assert_eq!(run["nativeExecution"]["maximumObserved"], 1);
        }
    }
    assert_eq!(report["equivalentOrderedOutcomes"], true);
    assert_eq!(report["cleanup"]["resources"], "0");
    assert_eq!(report["cleanup"]["cacheStores"], 0);
}
