# First complete framework benchmark baseline

The desktop-load collection completed locally on unpublished `main`: all 21
cases passed, with three warmups and 15 measured repetitions for each of 88
metrics. This is a baseline, not an optimization comparison or release decision.

The authorized desktop-load profile recorded CPU/GPU load without rejecting
utilization alone. The quiet
profile retains its strict 5% limits. Desktop warnings are independent of
invalidation: missing telemetry, incorrect results, malformed samples, insufficient
memory, timeout, identity changes and executable tampering remain rejecting.

## Canonical evidence

- All metric summaries (historical local evidence; not included in this checkout)
- Original report and raw condition observations (historical local evidence; not included in this checkout)
- Summary identities and condition distributions (historical local evidence; not included in this checkout)
- Actual comparator self-check (historical local evidence; not included in this checkout)
- Prepared manifest (historical local evidence; not included in this checkout)
- Reduced verification (historical local evidence; not included in this checkout)

Report SHA-256 is
`50a83f820bcb0f55b93b10817c998b71d97d98c9280f183741c5723b48906e69`.
The comparison self-check produced 44 inconclusive timing metrics and 44
diagnostic byte/count metrics. It exercises comparison admission and rendering;
comparing a report to itself is not candidate-performance evidence.

## Frozen implementation and workloads

| Identity | Recorded value |
| --- | --- |
| Build | `framework-desktop-20261010-h`, release |
| Git HEAD | `564e4142c8c4d1b57f2473f7a09d05d16da2a0d4`, dirty |
| Source digest | `12d5d2dc0e83622c01856504f0d776f575d41af78cdf8e684538a78c20b3018d`, 446 files |
| Native executable SHA-256 | `df22f8bf565b8d3216bd34e5bbe9ad9fef702c4d81146ecdd736e57f32e818f0` |
| Graphics executable SHA-256 | `3812ab3774602624976ea33253355c48415406625dfbd2ebcfd514e5f0d5f868` |
| Cargo.lock SHA-256 | `a45a67b19cf26b5201ca5ab3933d9b14095fc8ac420d54b34e8355a99dd8e0ef` |
| Compiler | Rust 1.98.1, LLVM 22.1.8, `x86_64-pc-windows-msvc` |
| Features | Native default features disabled; graphics `gpu` enabled |
| Workload digest | `b742537fa496b33f35930739513b9b47002af32bd731ad704176e2a42c4085c7` |

Preparation built, copied and hashed executables before measurement. Reduced
verification reported zero warmups, one repetition and no performance metrics.
Collection ran only prebuilt executables; no compilation or installation occurred
during its measurement window. Later documentation changes do not replace the
immutable source identity recorded in this bundle.

The workload covers prepared/ad hoc validation, exact wide roundtrips, scope churn
and deep disposal, serial/bounded CPU execution at 500k/1M entries with one/four
sessions, embedded native dispatch, reads under CPU/I/O contention, twenty runtime
cycles, 50,000-primitive scene replacement/patching/picking and five real GPU cases.

The contention fixture has finite 8M-entry CPU work and 256 slow operations per
owner. Its CPU chunks contain 262,144 entries; I/O chunks contain up to 64
operations, each sleeping 10 ms. One ordered window per owner avoids brief empty
worker intervals between helper windows. The CPU transform preserves 32 rounds
with an optimization barrier; the independent checksum remains exact. Initial
saturation requires occupied workers and running unfinished CPU/I/O tasks, since
permit reservation precedes execution. Every read retains both boundary checks;
all 480 measured reads passed. These fixture changes alter workload identity and
prevent comparisons with the older contention definition.

## Observed machine conditions

The host was a Ryzen 7 5800X with 16 logical processors and approximately 64 GiB
physical RAM, Windows `10.0.26300`, High Performance power plan, and Radeon RX 7800 XT driver
`32.0.31041.1004`. Real graphics execution selected DX12. The active display was
2560×1440 at reported 144 Hz and 96 DPI. Start/end machine identity checks matched; private host fingerprints remain local.

This was a declared desktop-load run. Utilization samples cannot establish
which external process caused the load or isolate an implementation effect.

| Observed condition | Preflight | Execution |
| --- | --- | --- |
| Samples | 210: ten per case | 210: one-second intervals |
| External CPU estimate, median / maximum | 43.80% / 79.30% | 40.02% / 67.50% |
| Busiest external GPU engine, median / maximum | 89% / 94% | 85% / 94% |
| Minimum available physical memory | 38,783,856,640 bytes | 38,731,214,848 bytes |

There are 38 diagnostic utilization warnings and zero invalidation reasons.
Maximum sampled driver-process working set was 444,985,344 bytes; maximum sampled
private commit was 599,863,296 bytes. These are sampled process observations,
distinct from known allocation counters and total driver/GPU memory. They include
post-exit observations and are not exact peak-memory proofs. No unrelated process
was stopped and no machine settings were changed.

## Representative results and interpretation

| Metric | Median | Empirical p95 across 15 repetitions |
| --- | --- | --- |
| Interactive read p95 per repetition | 13.5911 ms | 15.2156 ms |
| GPU settled 1% patch render call | 8.8849 ms | 11.4516 ms |
| CPU scene patch publication | 9.0735 ms | 10.6664 ms |
| 1M entries × four sessions, serial | 65.3919 ms | 73.2745 ms |
| 1M entries × four sessions, bounded | 98.8808 ms | 123.8600 ms |
| Prepared validation, 512 records | 0.198835 ms | 0.255777 ms |
| Ad hoc validation, 512 records | 0.392643 ms | 0.493854 ms |

The 480 correlated individual reads had pooled empirical p95 13.4308 ms and
maximum 16.2565 ms. Independent comparison uses 15 repetition percentiles;
individual reads are not treated as 480 independent samples. These fixture
observations meet the initial 50 ms embedded-native read target on this run.

The 500-key settled GPU patch uploaded 40,000 bytes against 4,000,000 bytes for a
full 50,000-instance upload: 1%, below the initial 10% target in every repetition.
All requested renders presented successfully. Twenty GPU lifecycle cycles per
repetition returned known retained devices and atlas bytes to zero. Four surfaces
shared the expected device/pipeline/immutable atlas assets.

The numerical bounded helper is slower than the serial control in the displayed
four-session workload. CPU scene patch publication still requested 5,795,252
allocation bytes through 100,003 allocation calls per repetition. These are
visible baseline findings, not before/after regressions or retained-memory leaks.
The complete summary shows every metric without an aggregate score.

Elapsed timings already use clock differences. Background utilization does not
provide a valid scalar adjustment for scheduling waits, shared caches, memory
traffic or device queues. This baseline includes substantial changing load;
matching policy/labels alone cannot prove equivalent interference in a future run.
Its confidence intervals will describe observed sample differences, not establish
a causal optimization gain. A controlled quiet baseline remains a distinct gate.

Embedded dispatch excludes WebView/frontend publication. GPU render-call timing
does not establish compositor timing, physical display latency, driver loss,
mixed-DPI behavior or other-platform acceptance. Strata scans and first-useful-map
timing remain separate application work. Earlier rejected runs and diagnostics
remain preserved; none were salvaged into this complete report.

## Local validation

Python guards and synthetic runner/comparator tests passed 71/71. Native reduced
tests passed 7 with one measurement protocol ignored. Graphics CPU and GPU-feature
tests passed 3 each with one measurement protocol ignored. Touched native/GPU
strict Clippy, formatting and documentation checks passed separately from the
actual measurement. These checks do not replace the raw runtime evidence above.

No commits, pushes, remote synchronization, CI dispatch or release publication
were performed.
