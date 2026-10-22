# Controlled optimization benchmarks

This suite measures framework changes against frozen workloads, with correctness
oracles, exact build identities and sampled machine conditions. Building and
functional verification are separate from performance collection. The legacy
`run --idle-confirmed` path requires authorization of its declared conditions.
The `paired` desktop path has no idle or CPU-utilization prerequisite.

The first complete desktop-load baseline is
`desktop-baseline-20261010-06` (historical local evidence; not included in this checkout):
21 cases and 88 metrics, collected from immutable bundle
`framework-desktop-20261010-h`. The [collection report](research/2026-10-10-controlled-benchmark-baseline.md)
records identities, conditions and limitations. A quiet-machine baseline remains
uncollected. Earlier reports and rejected attempts retain their original evidence
status and are not imported into this baseline.

## Prepare and verify

From the Revenant checkout, with Rust, Cargo and Python 3.11 or newer available:

```powershell
python scripts/benchmark_suite.py prepare baseline-20261010
python scripts/benchmark_suite.py verify baseline-20261010
```

`prepare` builds release test executables with `--locked`, using the existing
desktop and graphics crates. Desktop uses `--no-default-features`. Add `--gpu`
to preparation to compile optional Windows GPU cases; this creates no GPU device.
The default graphics build is CPU-only. Preparation uses metadata-only `describe`
calls to freeze the workload definitions in the bundle.

Bundles live in `target/benchmark-suite/prepared/<build-label>`. They include
copied executable hashes, Git HEAD and dirty source digest, Cargo.lock hash,
compiler and Cargo versions, features, relevant compiler environment flags,
Cargo configuration hashes, profile settings, descriptors and build logs.
Source changes during preparation cause failure. Failed labels cannot be reused;
prepare a fresh label after resolving the failure. Preparation does not commit
or modify application sources.
Compiler/wrapper overrides are rejected by the controlled v1 profile. Relevant
Cargo build/target/profile flags and effective artifact features are recorded
and must match when comparing runs.

`verify` executes all standard cases on reduced inputs, checks their exact
oracles and records zero warmups, one repetition and no performance samples.
It stays CPU-only even when the bundle includes GPU support. Verification outputs
live in `target/benchmark-suite/verification/<build-label>`. Labels are not
overwritten. Verification is a correctness check, not a baseline.

For the callback copying experiment, `diagnose` collects allocation counts from
the immutable native executable without timing, warmups, machine sampling or
WebView execution:

```powershell
python scripts/benchmark_suite.py prepare callback-copy-baseline
python scripts/benchmark_suite.py verify callback-copy-baseline
python scripts/benchmark_suite.py diagnose callback-copy-baseline
```

It exercises 100 progress calls for each retained update size (absent, 1,024 and
60,000 characters) with zero, one and eight observers. Preparation and full
verification remain prerequisites to comparison. Diagnostic outputs live in
`target/benchmark-suite/diagnosis/<build-label>` and cannot be overwritten.
Use `diagnose <build-label> --label <fresh-output-label>` to retry or repeat without
altering a prepared bundle or an earlier diagnostic artifact.
Allocation calls and cumulative requested bytes describe this region only;
they are neither retained RAM nor timing evidence. Measured cases use 5,000
calls per repetition, with counting in a separate pass. The timing pass disables
counting, but its allocator wrapper still performs an inactive thread-local check
on each allocation. Both sides must use that same instrumentation. Timing is
specific to this harness and cannot establish a whole-framework speed advantage.

## Paired callback comparison under desktop load

Use the existing frozen, successfully verified A/B bundles:

```powershell
python scripts/benchmark_suite.py paired callback-copy-baseline callback-copy-candidate --label callback-copy-paired-desktop-01 --background-label browser-running
```

The default selection is the nine `task.progress.bytes{0,1024,60000}.listeners{0,1,8}`
cases plus `dispatch.progress`. Repeated `--case` arguments can select other
matching native standard cases. The command neither compiles nor upgrades an SDK.
Every output label is immutable; failed attempts require a new label.

The collector saves a seed-378 schedule before execution: fifteen fresh-process
pairs with eight AB and seven BA orders, shuffled in advance. Processes execute
sequentially, and each runs the complete selected case list using the frozen
protocol's three warmups and fifteen measured repetitions. Native v1's legacy
authorization environment token is retained for executable compatibility; the
paired report explicitly declares desktop conditions and makes no idle claim.

CPU utilization is recorded at any observed percentage, including 100%, and does
not decide admission. Unavailable optional utilization or CPU-time observations
are `null`. Executable/configuration/workload identity, exact output oracles,
finite complete samples, timeouts and owned-process containment still decide
whether collection is valid. Raw output, logs, requests and optional observations
remain in each process directory, including failed attempts.

For each metric, summarize each process's fifteen samples by its median, then
estimate the candidate/baseline ratio across the fifteen paired process medians.
Bootstrap complete A/B pair tuples with seed 378 and 10,000 resamples for each
circular block length 1, 2 and 3. Report the envelope of those 95% intervals and
the separate AB/BA effects and chronological process summaries. Inner callbacks
and the fifteen samples within a process are not independent process trials.
Slow observations remain; utilization is never used to normalize elapsed time.
This adapts [Google Benchmark interleaving](https://google.github.io/benchmark/random_interleaving.html)
and [circular block resampling](https://bashtage.github.io/arch/bootstrap/timeseries-bootstraps.html).

Final kernel/user durations from [GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes)
are whole-process diagnostics across all threads, including startup, warmups,
allocation counting, setup and teardown. They are separate from callback latency.
Allocation calls, requested bytes, retained JSON and observed process memory also
retain their own boundaries. Known ownership/JSON counters do not establish a
whole-process leak or peak-memory claim.

The output is a distinct `paired-measure` report with its own comparison; it is
not an input to the older independent-report `compare` command. Callback B is
admissible when correctness holds, allocations decrease, retained counters stay
bounded and every timing interval's upper B/A bound is at most 1.05. A lower bound
above 1.05 establishes a material regression. Wider uncertainty remains
inconclusive. The fifteen-pair count is fixed before observing results.

## Collect a baseline and a candidate

After the user declares the machine idle, run the prepared baseline:

```powershell
python scripts/benchmark_suite.py run baseline-20261010 --label baseline-idle-01 --idle-confirmed
```

For a GPU-enabled prepared bundle, add `--gpu` to include its GPU cases as well
as all standard cases. Without that flag, its standard cases remain usable.
Use repeated `--case <id>` options to select a fixed subset. Unknown, duplicate
or GPU-only selections without `--gpu` are rejected. The same case selection,
features and measurement policy are required on both sides of a comparison.

After an implementation change, prepare and verify a new candidate label outside
the measurement window. Collect that candidate during a later explicitly idle
window:

```powershell
python scripts/benchmark_suite.py prepare candidate-20261010
python scripts/benchmark_suite.py verify candidate-20261010
python scripts/benchmark_suite.py run candidate-20261010 --label candidate-idle-01 --idle-confirmed
```

Each run uses only the copied, hash-verified executables. It does not compile,
install dependencies, stop unrelated applications or alter machine settings.
Preparation and verification must finish before performance collection begins.
Avoid changing power plans, drivers or machine settings between baseline and
candidate. Alternate baseline/candidate collection order in subsequent idle
windows when confirming an apparent effect; a single comparison cannot establish
that the effect persists across days or temperatures.

Reports stay under
`<local-evidence>\benchmarks\<run-label>`. They retain
the full build manifest, host identity, case requests, raw component JSON, driver
logs, preflight/execution observations and `report.json`. Output labels are
immutable and escaping paths are rejected. Work files and temporary compiler
extraction stay within each run's own work directory. Reports are not published.

## Workloads and interpretation

| Cases | Fixed measurement workload and boundary |
| --- | --- |
| `validation.small`, `validation.records512` | Real small/512-record contracts: preparation, prepared/ad hoc validation and serialization roundtrips. Wide values have exact decimal-string oracles. |
| `ownership.churn`, `ownership.deep-disposal` | 100,000 create/dispose operations and an 8,000-deep scope tree; active scope count must return to zero. |
| `cpu.500k.sessions1`, `.sessions4`, `cpu.1m.sessions1`, `.sessions4` | Seed 378, 32 wrapping-u32 rounds, chunks of 1,024 entries; serial and bounded results match an independent exact oracle. CPU/IO workers 4/4, queue capacity 32. |
| `dispatch.light`, `dispatch.records512` | Mean of 16 complete embedded native dispatch calls per repetition, including JSON encode/decode, admission, native execution, terminal/result observation and disposal. |
| `task.progress.bytes{0,1024,60000}.listeners{0,1,8}` | 5,000 core progress publications. Input and observer preparation precede collection; exact revisions, progress, payloads and ownership are checked afterward. Thread-local allocation counting excludes unrelated workers. |
| `dispatch.progress` | Mean of 16 embedded dispatch workflows retaining a 60,000-character typed update and publishing 5,000 progress calls, including terminal/result observation and disposal acknowledgement. Excludes WebView IPC and Svelte presentation. |
| `dispatch.interactive-contention` | Four finite CPU/I/O owners and 32 interactive reads per repetition. Saturation and exact owner results are required. Each repetition contributes one read p95; individual read durations and active-load observations are retained. |
| `lifecycle.runtime20` | Total elapsed time for 20 embedded runtime construction/work/settlement/disposal/shutdown cycles per repetition, including nested budget-one work. |
| `graphics.scene.replace`, `.patch`, `.pick`, `.lifecycle` | 50,000 primitives, 500-key settled patches, deterministic picking and 20 scene cycles. Correct geometry/order and settlement are asserted. |
| `graphics.gpu.setup.1`, `.setup.4`, `.patch`, `.render`, `.lifecycle` | Optional real Windows GPU setup/reuse, changed uploads, settled render calls and 20 lifecycle cycles. Known shared assets and upload proportions have explicit oracles. |

Every measured metric has 15 raw repetition samples after three warmups. A sample
can represent a repetition mean, a repetition read percentile or a lifecycle
total; its name and descriptor state that boundary. The comparator does not treat
the 32 correlated reads within a repetition as independent bootstrap samples.

Embedded native dispatch excludes WebView IPC, Svelte publication and display
presentation. Read p95 therefore does not establish full application read latency.
Render-call timing includes submission/present calls, but excludes compositor
timestamps and cannot establish 60 Hz presentation or physical driver-reset
behavior. Pipeline/device/atlas reuse counters are separate from driver memory.

Known scratch, retained JSON, GPU buffer/asset bytes and allocation counts remain
separate metrics. Requested allocation bytes include reallocations and do not
represent retained memory. Process working set/private commit are sampled
observations, not exact peaks; unavailable post-exit memory stays `null`.
Background active counts describe load validity, not an optimization win.
This suite does not establish Strata scan throughput or close application and
physical/platform acceptance gates. Restricted compute remains its separate
documented no-go experiment.

The contention fixture admits one finite window per owner: CPU chunks contain
262,144 entries; I/O chunks contain up to 64 individually cancellable slow
operations. Each owner still computes 8 million entries and performs 256
10 ms slow operations. This avoids brief empty-worker intervals between helper
windows while the read observations are taken. Numerical throughput cases keep
their 1,024-entry chunks. The per-read CPU/I/O worker and task checks remain strict.
Initial saturation also requires running, unfinished background tasks in both
classes. A reserved worker permit can precede the task's running state and is
insufficient evidence that the contention workload has started.

The contention CPU fixture uses an optimization barrier after each of its
32 arithmetic rounds. Release compilation otherwise collapses those rounds
into a single affine expression, allowing the load to finish before all reads.
The barrier preserves the independent exact checksum. It is confined to the
contention fixture; numerical serial/bounded workloads retain ordinary compiler
optimization. This parameter is included in workload identity.

## Machine checks and invalid runs

The default `--conditions quiet` profile keeps the 5% CPU/GPU limits below.
An explicitly authorized desktop workload can instead use:

```powershell
python scripts/benchmark_suite.py run framework-desktop-20261010 `
  --label desktop-baseline-01 --idle-confirmed --gpu `
  --conditions desktop --background-label brave-playing
```

Desktop conditions record external CPU and GPU-engine utilization without
rejecting a run for utilization alone. Readings above 20% produce diagnostic
warnings in the report and comparison. This retains the same memory minimum,
ten preflight observations, execution sampling, required telemetry, correctness
checks and invalid-report retention. The policy explicitly records
`utilizationEnforcement: record-only`; quiet runs record `reject`. The condition profile
and required background label are recorded in the comparison policy. Quiet and
desktop reports, or different background labels, cannot be compared.

For this profile, `--idle-confirmed` acknowledges that the user has authorized
the declared background load and is ready for measurement. It does not assert
that background applications have been closed. Baseline and candidate must keep
the actual background workload equivalent: the same playback/content, browser
tabs and playback settings. A label is an operator declaration; the runner does
not inspect private browser content or prove that two labels describe identical
activity. Raw utilization observations remain the evidence of observed load.
Higher background load can increase variation and leave a comparison inconclusive.
These results describe desktop-load performance and do not satisfy quiet-host
acceptance gates. The runner never falls back to this profile automatically.
Desktop diagnostic warning thresholds can be declared explicitly with
`--max-external-cpu-percent 25 --max-external-gpu-percent 30`. These overrides
require desktop conditions, accept finite percentages from 0 to 100, and are
recorded in policy. Comparisons reject changed thresholds or enforcement rules.
These flags never normalize timings or establish stability. Desktop confidence
intervals describe the observed elapsed-time samples, including background
interference; they cannot isolate its cause or prove an implementation effect.

Before each case, the runner records ten one-second condition observations.
Quiet runs require external CPU estimate at most 5%; both profiles require
available physical memory at least 2 GiB and valid required telemetry.
Execution is sampled once per second. When the child finishes partway through
an interval, the observer waits for the rest of that second before its final
sample. This includes a post-exit tail in the stability check, keeps quantized
Windows counters reliable, and leaves the driver's timed repetitions unchanged.
The final-observation rule is part of comparison policy. Each case has a
600-second timeout. Correctness failures, missing or
malformed output, hash/workload mismatches, timeouts, observer failures and noisy
conditions in the quiet profile invalidate the run. Desktop utilization warnings
are retained independently of invalidation reasons. Invalid reports and raw files are retained, the
command exits with code 2, and comparisons refuse them.

CPU utilization comes from cumulative Windows system counters, subtracting CPU
time from the owned benchmark process. This is an estimate: kernel/driver work
attributable to the benchmark can appear external. CPU monitoring supports one
processor group and at most 64 logical processors; unsupported machines fail
closed. The suite records CPU model/count, physical memory, OS, architecture,
active power-plan identity and GPU adapter/driver metadata. Unknown required
identity fields prevent a valid run; identity changes at the end invalidate it.
GPU profiles also fingerprint active display geometry, refresh and DPI, and
record the actual selected adapter/backend. Adapter changes within or between
runs are rejected. Each requested benchmark render must increment a successful
presentation counter; skipped/occluded/timeout frames invalidate the case.
Windows mode refresh is an integer and cannot distinguish fractional rates;
display metadata also does not establish VRR or compositor state.

Quiet GPU runs additionally require external GPU-engine utilization at most 5%.
Desktop GPU runs retain these observations as diagnostics.
A persistent, owned PowerShell observer queries Windows GPU performance counters
once per second. It sums process shares within each adapter/engine, excludes the
benchmark PID and reports the busiest external engine. Missing, stale, malformed
or incomplete sensors remain unknown and invalidate GPU runs. Its own CPU overhead
is included in the observed conditions. It creates no rendering device.

These sampled checks can miss short interference and do not control temperature,
clock throttling, storage contention or every driver allocation. The user's idle
signal and compatible conditions are necessary alongside the observations. The
runner automatically terminates only its own timed-out driver/observer jobs.

Elapsed timings use start/end differences from Rust's monotonic `Instant` clock.
CPU observations separately use differences in cumulative system and owned
process CPU time. Elapsed time includes scheduling waits and synchronization;
process CPU time counts execution across threads. A background CPU percentage
cannot reliably correct elapsed time: scheduling, shared caches, memory traffic
and device queues affect different operations differently. The suite records
raw durations and conditions rather than inventing a utilization-adjusted result.
Windows documents the distinct [elapsed-time counter](https://learn.microsoft.com/en-us/windows/win32/sysinfo/acquiring-high-resolution-time-stamps)
and [process CPU-time counters](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes).

## Compare and validate the suite

```powershell
python scripts/benchmark_suite.py compare `
  <local-evidence>/benchmarks/baseline-idle-01/report.json `
  <local-evidence>/benchmarks/candidate-idle-01/report.json `
  --label baseline-v-candidate-01
```

The comparator requires matching schema, workload versions/digest, cases,
parameters, units/directions, compiler/Cargo, release configuration/features,
machine fingerprint and policy. Both reports must be valid and every oracle must
pass. Source and executable hashes may differ: those differences identify the
implementation being evaluated. Changed compiler settings require a separate
experiment instead of quietly mixing conditions.

It writes comparison JSON and Markdown under the same acceptance grouping.
Each metric includes median, empirical p95, absolute/percentage change and a
deterministic independent median-ratio bootstrap interval (95%, seed 378, 2,000
resamples). Timing changes are classified only if the entire interval passes the
5% practical threshold; exact 5% or overlapping intervals are inconclusive.
Counts and bytes are diagnostic. Zero counter baselines have no invented ratios.
All metrics remain visible; there is no aggregate score hiding regressions.
Empirical p95 from 15 repetitions is not a proven population tail, and intervals
are exploratory rather than guarantees across machine states or many comparisons.

Synthetic guard/integration tests do not benchmark the machine:

```powershell
python -m unittest discover -s scripts/tests -p 'test_benchmark_*.py'
cargo test -p revenant-desktop --no-default-features --test optimization_benchmarks
cargo test -p revenant-graphics --test optimization_benchmarks
cargo test -p revenant-graphics --features gpu --test optimization_benchmarks
```

Ordinary Rust tests exercise reduced correctness/guards; the measurement protocol
test is ignored unless invoked by the runner. GPU-feature smoke tests still use
the standard CPU profile. Actual GPU execution was separately exercised in the
complete desktop baseline; those smoke tests alone do not establish that result.
