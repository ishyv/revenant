# Callback lifetime, unwind safety and copying evidence

This focused pass implements the [approved design](../../plans/2026-10-11-callback-lifetime-and-snapshot-copying.md)
inside the incomplete `revenant-sun` integrity milestone. Callback and execution
corrections and copying candidate B are adopted and delivered. Fifteen paired
desktop A/B runs verify reduced allocations and improved timings in the selected
workloads, with unchanged retention and public contracts. The rejected earlier
quiet attempt is retained as historical evidence.
No Git commit, remote synchronization, CI run or publication occurred.

## Correctness results

| Reproduction | Correction and acceptance evidence |
| --- | --- |
| Final listener/probe captures reenter `snapshot()` from their destructor | Detach under the task mutex and destroy after unlocking. Subprocess watchdog fixtures cover unsubscribe, replacement, failed immediate subscription and terminal release. |
| Initial subscription delivery panics | Registration guard exists before callback delivery; unwinding removes future lookup and releases captures. Already captured deliveries still run. |
| Running delivery panics during `begin()` | Only the successful ownership claimant constructs an execution guard before delivery. Panic interrupts the claimed execution and rolls back unreturned native owners. Eight competing begin calls produce one winning guard. |
| Queued cancellation notification panics | Terminal state, rollback and lease/probe cleanup complete before notification. Running cancellation still preserves executing leases until acknowledgement. |
| Operation and Interrupted observer both panic | Execution-guard cleanup releases ownership first and catches each cleanup observer unwind. A subprocess preserves the original panic, survives, and delivers to a later observer after resource destruction. Ordinary publications still propagate observer panics. |
| Terminal observer panics after success | The result remains Succeeded with its committed output and released execution inputs. Notification failure cannot undo successful settlement. |
| Probe is still polling during finalization | Task-owned probe reference is detached; the outstanding checkpoint's clone stays live until its poll exits. |
| Desktop batch Running observer panics before worker launch | Private completion ownership follows execution and settles unfinished history when the last scheduled owner exits. |
| Independent coordinator panics with a held blocking child | Child completion references keep history alive until the held reader exits. Disposal acknowledgement returns before a noncooperative worker finishes. Single, serial and independent paths are exercised. |

The first thirteen core regressions produced **five passes and eight failures**
before correction. The properly configured desktop fixture produced **four
passes and two failures** before its completion correction. The initial desktop
fixture accidentally requested only one ordinary CPU slot; it was corrected to
two ordinary slots plus the reserved interactive slot before using its failures
as evidence. Fixture setup failure is not a framework diagnosis.

There are now **16 core and 6 desktop adversarial regressions**, all passing.
Three core cases additionally verify independently owned snapshots, concurrent
revision/counter publication, and a probe already cloned by a checkpoint. Twenty
callback-failure cycles reclaim created resources and remove task history; the
reduced embedded runtime lifecycle oracle also performs twenty cycles. These
checks establish fixture ownership reclamation, not a universal proof of no leaks.

The desktop completion guard costs one shared allocation per admitted job and
reference counting for independent children. This is an **accepted correctness
tradeoff**, with no measured cost relative to the original implementation.
Created-owner retention follows the existing result/outcome rule; this pass does
not select only the handles appearing in output JSON.

Rust's [unwind boundary](https://doc.rust-lang.org/std/panic/fn.catch_unwind.html)
does not catch aborting panics. Application captures, resource destructors and
custom panic-payload destructors must remain nonpanicking. Panic hooks still run.
Only execution-guard cleanup contains observer unwinds; ordinary delivery remains
fail-fast. Callback overlap and stale-revision handling remain public behavior.

## Frozen identities

All paths below are under `<local-evidence>\core-integrity`:

| Inventory | SHA256 |
| --- | --- |
| Original `baseline-20261011/manifest.json` | `a475ab94b033da3fdd43fc242b93dfd505846b3589f33a394e45057bf97e2244` |
| Pre-correction `callback-before-20261011/manifest.json`, 2,018 files | `9ef6ba8f855b6f13bff24058d1c773db4c5bc89725a88418840851d3292241b1` |
| Corrected A `callback-corrected-A-20261011/manifest.json`, 2,022 files | `ea33da3f3e50bfb0e2ecc994e7603b83314d4aefef6c39b45fba09742a195d39` |

These are source inventory digests, not the hash of formatted manifest bytes.
The original baseline remains separate. Generated consumer facades were not
included in the source capture; SDK recovery backups preserve their prior bytes.

The prepared release bundles are in
`<revenant-checkout>\target\benchmark-suite\prepared`:

| Bundle | Dirty source digest | Copied native executable SHA256 |
| --- | --- | --- |
| `callback-copy-baseline` | `56fdf825f56a0048d6628099ba2e959c9ca3f2d3ff38d95ee9d7a601650f31f9` | `e90e7c217990f6d43ce0c4e54e48f84d7f10a38d7c7be9f201302e538db79801` |
| `callback-copy-candidate` | `9cbbfc8af8d78a806863e347e4da4578de419a20e8c397cb8fa434701abcd5cd` | `3d784353a5d0f9bf0b7e53f0edfcb48cb6c86348d8485eb0887a33560764ad7e` |

The compiler, Cargo, Cargo.lock, features, profiles, configuration and native
workload descriptors match. Both reduced verification bundles pass all 26
standard native/CPU graphics cases, with zero warmups, one repetition and no
performance metrics. No GPU device is involved in this verification.

## Allocation comparison

Machine-readable evidence (historical local evidence; not included in this checkout) binds the exact
diagnostic reports and executable identities. Each case makes **100 progress
calls**; setup, final oracle checks, finalization and removal are outside the
counted region. Observer payload checks and atomics are inside delivery on both
sides. Thread-local counting excludes other threads.

| Retained characters | Observers | Allocation calls A → B | Cumulative requested bytes A → B |
| ---: | ---: | ---: | ---: |
| absent | 0 | 200 → 0 | 4,700 → 0 |
| absent | 1 | 500 → 300 | 11,000 → 6,300 |
| absent | 8 | 1,900 → 1,700 | 55,100 → 50,400 |
| 1,024 | 0 | 300 → 0 | 107,100 → 0 |
| 1,024 | 1 | 700 → 400 | 215,800 → 108,700 |
| 1,024 | 8 | 2,800 → 2,500 | 976,700 → 869,600 |
| 60,000 | 0 | 300 → 0 | 6,004,700 → 0 |
| 60,000 | 1 | 700 → 400 | 12,011,000 → 6,006,300 |
| 60,000 | 8 | 2,800 → 2,500 | 54,055,100 → 48,050,400 |

**Verified benefit: redundant allocation reduction in these regions.** Owned
snapshot independence, revisions, updates, legacy outcomes, terminal results,
reentry, unsubscribe during delivery and concurrent publication still pass.
No retained history is added: the single newest update keeps its 64 KiB encoded
bound and removed task/scope fixture state returns to zero. That does not measure
process peak memory or prove zero growth in arbitrary applications.

**Pre-timing diagnostic boundary.** These allocation diagnostics collected no
elapsed time. Allocation counts and cumulative requested bytes are neither retained
RAM nor speed. Measured core cases use 5,000 calls; counting occurs in a separate
pass from timing. The disabled counting wrapper still incurs the same inactive
thread-local check on allocation in A and B. Complete embedded native dispatch
retains the update, publishes bounded progress, observes the result and requests
disposal. It excludes WebView IPC and Svelte presentation.

The first candidate diagnostic failed with `WinError 5 AssignProcessToJobObject`;
its child log showed successful exit but no validated diagnosis was published.
It remains immutable and is not used as acceptance evidence. The valid retry is
`target/benchmark-suite/diagnosis/callback-copy-candidate-retry1/diagnosis.json`.
The cause (short-child exit race or inherited job policy) remains unconfirmed in
the historical artifact. The independently reproduced post-exit assignment race
was corrected under `revenant-sun.18` on 2026-10-13; see
the ownership correction evidence (historical local evidence; not included in this checkout).
A diagnostic `--label` option was added afterward to allow
fresh output directories; the two frozen native drivers and descriptors were
not rebuilt or changed by that output-label addition.

## Adoption and verification

Corrected A was delivered with the rebuilt release CLI through ordinary
`sdk upgrade` in Strata and Luma. Delivery evidence (historical local evidence; not included in this checkout)
matches all seven affected framework files to both SDKs and all three affected
client files to each installed npm package. Public contract files remain
byte-identical to each application's recovery backup. Compatibility stays
**manifest 6 / protocol 5 / SDK 4**.
Two normal upgrade passes were used: the second includes final terminal and
paged-result documentation clarification only. All four recovery backups remain
available. No SDK snapshot was edited by hand.

| Gate | Result |
| --- | --- |
| Full core and desktop tests, `--no-default-features --locked --offline` | Passed, including the new adversarial regressions; explicit measured/physical cases remain ignored |
| SDK and macro tests, including wire/update authoring and doc tests | 47 passed; pre-existing ts-rs transparent-attribute warning remains |
| Client tests, TypeScript and public documentation | 89 passed, TypeScript passed, all 601 declarations documented |
| Benchmark runner/comparator/host guards | 73 passed |
| New native benchmark protocol checks | 8 passed; timed protocol test remains explicitly ignored |
| SDK replacement, inspection and recovery fixtures | 19 passed; failure rollback, interrupted restore and redo fixtures included |
| Compiled consumer input/output schema parity | 54 schemas passed after delivery |
| Strata frontend check and production build | Passed, zero Svelte errors/warnings |
| Strata native and engine | 19 native plus 41 engine tests passed; shell recycling fixture remains explicitly ignored |
| Luma frontend check, tests and production build | Passed, zero Svelte errors/warnings; 18 frontend tests passed |
| Luma native | 13 passed |
| Normal SDK upgrades and compiled development hosts | Both succeeded; `sdk check` reports revision 4 |

Strata's full native command initially found an existing example migration gap:
`adapter_benchmark.rs` still chose a scope ID even though its frozen prior SDK
already returned opaque IDs. `strata-hef` migrates every access to the returned
scope identity; the example now compiles in the full native test command. No
adapter benchmark timing was collected. Actual consumer restore/redo and
packaged operation outside the checkouts remain distinct from passing recovery
fixtures and development host builds.

## Timed attempt: 2026-10-11

After fresh explicit idle-machine confirmation, the unchanged frozen baseline
was selected with quiet conditions, three warmups, fifteen repetitions and all
ten copying cases. The runner sorts selected cases, so `dispatch.progress` was
first. Its ten-second preflight rejected external CPU estimates above the
unchanged 5% limit: nine of ten samples exceeded the limit, with a range of
4.8828125% to 25.5859375%.

The immutable report is
`<local-evidence>\benchmarks\callback-copy-A-quiet-01\report.json`,
SHA256 `c26390704cb50f0a2337f0da4d6871e697965f32c1122e2b2e9a80b5b8937985`.
It records `valid: false` and
`dispatch.progress: externalCpuEstimatePercent outside stability limit 5`.
Its execution observations and component results are empty: no benchmark driver
was launched and no performance samples were collected. Collection stopped as
required. No candidate run or comparison was attempted.

Timing attempt evidence (historical local evidence; not included in this checkout) binds the
report, frozen executable identities, selected cases and decision. **Inconclusive:
timing and material runtime regressions.** Corrected A remains live in the
framework and both consumers; B stays isolated and `revenant-sun.17` stays open.
No SDK upgrade or rebuild was needed for this deferred outcome. The quiet limits
and machine settings were unchanged; the source of the background CPU activity
was not investigated. The separate short-child containment issue `.18` was not
encountered by this preflight-only attempt; its later 2026-10-13 correction
preserves this evidence.

+## Paired desktop results: 2026-10-11

Candidate B is **adopted and delivered**. The immutable
`callback-copy-paired-desktop-01/report.json` is under
`<local-evidence>\benchmarks`;
SHA256 `2c15eb0fae44c177488a66a7fcbea823e822a16f0d8591829bf46878bfbd04ac`.
Hash-bound paired evidence (historical local evidence; not included in this checkout) verifies all
thirty raw process-result hashes, frozen driver identities and the delivered
source bytes. The source patch retains its earlier identity; Git's CRLF
conversion was checked as text-equivalent before preserving the exact frozen LF
candidate files.

Fifteen fresh-process pairs completed with eight AB and seven BA orders, shuffled
using seed 378. Each process ran all ten cases with three warmups and fifteen
repetitions: 225 measured samples per variant per case, grouped into fifteen
process summaries. CPU utilization was metadata throughout. The 244 available
external CPU estimates ranged from 0% to 30.4167%, with no telemetry warnings.

Timing estimates use ratios of process medians. The paired circular-block
bootstrap uses 10,000 resamples, seed 378 and block lengths 1, 2 and 3; the table
reports their 95% interval envelope. Every upper bound is below 0.95, satisfying
the agreed 1.05 non-regression limit. All ten timings are **verified benefits for
these workloads and desktop conditions**. Slow observations and order effects
remain in the raw report; no CPU normalization or utilization filtering occurred.

Core times below are totals for 5,000 progress calls. `dispatch.progress` is the
mean duration of one complete embedded operation across sixteen iterations,
including JSON admission, updates, result observation and disposal acknowledgement.

| Case | A median (ms) | B median (ms) | B/A 95% envelope |
| --- | ---: | ---: | --- |
| `task.progress.bytes0.listeners0` | 0.6537 | 0.1134 | 0.1630–0.1876 |
| `task.progress.bytes0.listeners1` | 1.1545 | 0.7621 | 0.6394–0.6844 |
| `task.progress.bytes0.listeners8` | 3.9536 | 3.4920 | 0.8362–0.9209 |
| `task.progress.bytes1024.listeners0` | 0.8476 | 0.1166 | 0.1308–0.1413 |
| `task.progress.bytes1024.listeners1` | 1.5576 | 0.9764 | 0.6091–0.6446 |
| `task.progress.bytes1024.listeners8` | 5.6676 | 5.1385 | 0.8573–0.9150 |
| `task.progress.bytes60000.listeners0` | 5.8014 | 0.1166 | 0.0189–0.0233 |
| `task.progress.bytes60000.listeners1` | 11.4895 | 6.0764 | 0.5091–0.5419 |
| `task.progress.bytes60000.listeners8` | 48.6374 | 43.0638 | 0.8513–0.9087 |
| `dispatch.progress` | 16.4180 | 8.1366 | 0.4837–0.5077 |

Allocation calls and requested bytes decreased in every core case, with
retention counters equal across every raw repetition. For 5,000 calls with a
60,000-character retained update and zero observers, requested bytes fall from
300,235,000 to zero. Ownership/update/result oracles pass on both sides. This is
bounded fixture evidence, with no whole-process leak or peak-memory conclusion.

Whole-process CPU diagnostics have fifteen complete pairs: total CPU median
13.828125 seconds in A and 7.4375 seconds in B (ratio 0.53785). Kernel/user
observations are retained separately. These totals include startup, warmups,
allocation counting, setup and teardown and do not substitute for callback
latency. Embedded measurements exclude WebView IPC and Svelte presentation.

The exact B source hashes below now match the framework and both consumer SDKs.
Full core/desktop acceptance passes 160 tests, including 16 core and 6 desktop
adversarial regressions; five explicit cases remain ignored. The paired tool
passes 95 benchmark tests, including 22 paired guards. Independent reviews of
the collector and native patch passed. New modules pass Ruff; the modified older
host/runner retain their baseline lint diagnostics (3/7).

The rebuilt release CLI is managed by `binmgr` at `C:\T\bin\revenant.exe`,
version 0.4.0, SHA256 `5a48ce49101a1dbba89206ddb11c6fa0dc3454b7631c4dcd279c0facef416706`.
A fresh shell outside the checkout resolves that command. The prior shadowing
Cargo 0.3.0 executable is preserved as
`C:\Users\theyu\.cargo\bin\revenant.exe.pre-binmgr-20261011.bak`.
One ordinary SDK upgrade per consumer delivered B; all earlier and new SDK
backups remain complete. Both `sdk check` commands pass revision 4. Public
contracts are byte-identical to their newest recovery states and earlier A
contract hashes; 54 input/output schemas and compatibility 6/5/4 stay unchanged.

Strata frontend check/build, 19 native tests and 41 engine tests pass; one
pre-existing recycling fixture remains ignored. Luma frontend check/build,
18 frontend tests and 13 native tests pass. Both Svelte checks have zero
errors/warnings. The 19 SDK replacement/recovery fixtures also pass; actual
consumer restore/redo, packaged operation and physical presentation remain
separate acceptance work. No commit, push, synchronization, CI or publication
occurred, and the broader audit stays open.

## Reproduce paired desktop collection

The quiet CPU gate above was an unnecessarily restrictive admission choice for
this assessment. Normal desktop collection now has no idle prerequisite and no
CPU-utilization cutoff. Preserve the failed quiet attempt as historical evidence.
The new `paired` path reuses the unchanged A/B executables and identical ten-case
selection, including the existing three warmups and fifteen repetitions per
process. Fifteen fresh-process pairs follow a counterbalanced seed-378 schedule;
CPU utilization is optional metadata. [BENCHMARKS.md](../../BENCHMARKS.md#paired-callback-comparison-under-desktop-load)
documents the process sampling, paired circular-block interval envelope and
separate owned-process CPU, allocation and retention diagnostics.

The following uses a fresh immutable run label and the default ten copying cases:

```powershell
python scripts/benchmark_suite.py paired callback-copy-baseline callback-copy-candidate --label callback-copy-paired-desktop-01 --background-label browser-running
if ($LASTEXITCODE -ne 0) { throw 'Collection invalid; preserve its evidence and stop.' }
```

The recorded label now exists. Reproduction must use a fresh label and retain
the complete fifteen-pair schedule; never overwrite or repeat until acceptance.

The isolated patch and exact B sources live in
`<local-evidence>\core-integrity\callback-copy-candidate-20261011`.
Patch SHA256 is `e5d208240b38d742f626e12a13e0f31c01a793fec706428ef737ccc76d55e838`.
Before applying, verify the corrected checkout file hashes:

| File | Required A SHA256 | Expected B SHA256 |
| --- | --- | --- |
| `crates/revenant-core/src/tasks/context.rs` | `15be2e95f84eb89d953c656e0c43ef0fb885a7e57033a808b1f6e545c7564e28` | `194ba5ae5534d23528899631b307d307099e616bc0990121f09e59f0ee1d2602` |
| `crates/revenant-core/src/tasks/updates.rs` | `7656538000979ea0dfe4cff48614bca0a945ff4558e8a3ae62597fc3b525e699` | `c0a8a75dc50883ddf3651e5d0bb0afc163f8e476e65e62cc0d17b5c38f0cd365` |

Adopt B only if correctness holds, redundant allocations decrease, retention
stays bounded, and every timing metric's upper 95% candidate/baseline ratio is
at most 1.05. A speed improvement is optional. A wide interval permitting more
than 5% slowdown defers adoption even when the comparator calls it inconclusive;
an established material regression rejects B. Rerun affected
checks and deliver it with a newly rebuilt CLI and ordinary SDK upgrades. If
timing is inconclusive, state that explicitly; allocation reduction alone does
not establish that Revenant is faster. If rejected, preserve A and the immutable
experiment rather than resetting the unpublished tree.

Provider ownership, remaining authoring/generation machinery, graphics lifecycle
and copying, physical viewport acceptance, supporting tools, packaged operation,
full application performance and the rest of file coverage remain open. This
pass does not close `revenant-sun` or its ownership audit `revenant-sun.3`.

The hash-bound ledger now records **25 interpreted, 23 partial, 398 unreviewed
and 15 dependency/assets** across 461 authored/dependency paths. The complete
task context, cancellation, execution and update files and focused regressions
were interpreted. Desktop jobs, client task publication exceptions and broader
benchmark machinery remain partial; opening or modifying those files does not
make the whole file reviewed.
