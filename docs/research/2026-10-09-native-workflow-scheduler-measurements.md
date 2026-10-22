# Native workflow scheduler measurements - 2026-10-09

The frozen native fixture passed exact-result, budget and cleanup checks. Its
interactive reads met the 50 ms p95 target in all five measured rounds. The
bounded CPU helper was slower than the identical serial algorithm in this run;
these measurements establish responsiveness and ownership behavior, and do not
establish an application throughput improvement.

Complete numeric report (historical local evidence; not included in this checkout)
contains every sample, per-session completion time, counters, embedded source
identities, verified source SHA-256 hashes, executable SHA-256, compiler version,
base commit and compiled contract digest. It was measured from unpublished dirty
main with manifest 6 / protocol 5 / SDK revision 4, package version 0.4.0, on
Windows x86_64 with 16 available logical processors. Other workstation activity
was not controlled.

## Reproduction and frozen workload

```powershell
cargo test -p revenant-desktop --no-default-features --test workflow_benchmark --release -- --include-ignored --nocapture --test-threads=1
cargo clippy -p revenant-desktop --no-default-features --test workflow_benchmark --release -- -D warnings
```

Both release tests passed, including the explicitly invoked measurement test.
Strict Clippy passed after the final fixture changes. The generated measurement
file is `target/workflow-benchmark.json`; this research copy adds SHA-256 evidence
verified against the four source identities embedded in the measured executable.

The algorithm starts at `index + 378`, performs 32 wrapping u32 multiply/add
rounds, and sums exact u64 results. Both paths use the same function, with an
optimizer barrier each round. The bounded path uses 1,024-entry chunks, lazy
input, ordered reduction and one shared CPU4 / IO4 / queue32 service. Bulk CPU
uses at most three execution stages because one CPU slot is reserved for
interactive work. The serial reference uses one ordinary thread per session.
Exact reference checksums are 500,000 entries: 1073747902252720; 1,000,000 entries: 2147482922439008.

Each matrix has a reference checksum pass, two unrecorded paired warmups and
five measured repeats. Serial/bounded order alternates. Duration covers barrier
release, all session joins, terminal reporting and task/scope cleanup; worker-pool
creation is excluded. No queue observer runs during CPU timing. Percentiles use
nearest rank, so a five-repeat p95 is the worst observed repeat, not a reliable
estimate of a population tail.

## CPU results

| Entries per session | Sessions | Serial p50 / p95 ms | Bounded p50 / p95 ms | Bounded / serial p50 |
| --- | --- | --- | --- | --- |
| 500,000 | 1 | 29.86 / 31.20 | 140.30 / 155.63 | 4.70x |
| 500,000 | 4 | 35.61 / 36.23 | 61.75 / 83.13 | 1.73x |
| 1,000,000 | 1 | 63.79 / 65.96 | 295.02 / 325.69 | 4.63x |
| 1,000,000 | 4 | 69.43 / 74.03 | 106.16 / 121.50 | 1.53x |

Every timed checksum matched the serial reference. CPU helper peaks were three
active stages, with known inline scratch peaks of 640 bytes for one session and
2,560 bytes for four. Aggregate queue wait and settled-chunk counts include
warmups and are preserved in the report. All live execution, queue and scratch
counters were zero after each matrix.

The overhead and throughput differences are measured limitations. These results
do not justify automatically replacing small serial loops with the helper.
Grain-size/scheduling tuning remains a performance consideration; the fixture
does not attribute the difference to a specific bottleneck or infer Strata speed.

## Interactive service under competing work

Four owner scopes each run continuous CPU windows and 250 ms simulated blocking
I/O chunks. CPU and I/O tasks share their session scope. A separately pinned
interactive task in session zero performs 100 reads per round, five milliseconds
apart, after one coherent sample establishes three active bulk CPU stages and
four active I/O stages. There are two warmup rounds and five measured rounds.
Queue sampling runs at one millisecond in this workload. TaskManager embedding
measures the typed work service; it excludes runtime operation admission,
contract serialization, transport and frontend publication.

Measured per-round interactive p95 values were 0.1205, 0.1635, 0.1973, 0.1399, 0.1688 ms. All were below 50 ms.
The mixed service peaked at four CPU stages including the interactive stage,
four I/O stages, at most 32 sampled queued chunks and 2,764 known inline
scratch bytes. All live counters returned to zero after settlement.

Per-owner completed CPU and I/O chunks are recorded before the last interactive
read and after settlement. Every owner eventually progressed. Rounds [3, 6]
had at least one I/O owner with zero completed chunks by the last read; progress
was uneven. A bounded queue and observed eventual progress do not establish a
fixed starvation/service-time guarantee. The sleeping I/O simulation does not
represent real metadata latency or cancellation of a blocked OS call.

## Native runtime lifetime and nested admission

A separate normal regression and twenty measured cycles create a fresh embedded
DesktopRuntime, admit an I/O operation at CPU1 / IO1, then run an I/O child with
CPU grandchildren. The parent's permit is lent and every operation returns the
exact result 9. Each cycle owns one 4,096-byte native payload. Before disposal,
its native live count is one; after task/scope disposal and shutdown, scopes,
tasks, views, folders, selections, resource owners/live resources, active work,
queued chunks and known scratch bytes are zero. CPU and I/O peaks stay at one.
Each runtime has a fresh instance identity, with its contract digest recorded.

These are full embedded-runtime creation/shutdown cycles. They do not prove
executable-process restart, packaged startup, webview disposal or visual behavior.

## Memory and evidence boundaries

Known inline item/Option-result windows have an 8 MiB shared scratch budget;
ordered helper results have a 64 MiB limit and stream windows have 32 items.
The benchmark never materializes the whole bulk input. Closures, channels,
thread stacks, allocator metadata, OS allocations and arbitrary application
allocations are not included in the exact scratch counter. Their memory and
process resident memory remain `null`; no total process-memory ceiling is
claimed. This fixture excludes IPC, filesystem traversal, database persistence,
real application scans, graphics, consumer SDK upgrade and clean-machine release
acceptance. It is native framework evidence only.
