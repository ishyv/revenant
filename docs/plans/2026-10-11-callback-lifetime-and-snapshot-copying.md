# Revenant: callback lifetime, unwind safety, and snapshot-copying experiment

Approved design and fresh-session handoff, 2026-10-11. Implementation status and
evidence belong in Beads and `docs/research/core-integrity`, not in a checklist in
this document. The broader audit remains incomplete.

Measurement policy was revised on 2026-10-11: this experiment uses paired
fresh-process collection under normal desktop load, without an idle prerequisite
or CPU-utilization cutoff. The quiet-window instructions below are superseded by
the [paired protocol](../BENCHMARKS.md#paired-callback-comparison-under-desktop-load).
The frozen A/B workloads and correctness, allocation and 5% latency-regression
acceptance criteria remain; the rejected quiet attempt keeps its historical status.

## Purpose and context

Complete a focused correctness pass over callback ownership and task unwinding,
then evaluate a narrow snapshot-copying optimization against a corrected frozen
baseline. Allocation reduction is not proof of faster complete applications.

- Checkout: `<revenant-checkout>`, branch `main`, HEAD
  `564e4142c8c4d1b57f2473f7a09d05d16da2a0d4`. Preserve the unpublished working
  tree; the starting status contains 304 changed/untracked paths.
- Compatibility: manifest 6, bridge protocol 5, SDK 4, package 0.4.0.
- Consumers: `<strata-checkout>` and
  `<image-notes-checkout>`.
- Epic: `revenant-sun`; ownership audit: `revenant-sun.3`. Callback capture fixes
  are `revenant-sun.15`, unwind/completion fixes `revenant-sun.16`, and the
  experiment `revenant-sun.17`, dependent on both corrections.
- Starting coverage: 13 interpreted, 15 partial, 411 unreviewed, 15
  dependency/assets. Later reading and probes have not yet been persisted.
- Prior corrections and SDK delivery:
  implemented-upgrades (historical local evidence; not included in this checkout).
- Original dirty-source baseline:
  `<local-evidence>\core-integrity\baseline-20261011\manifest.json`;
  inventory SHA256
  `a475ab94b033da3fdd43fc242b93dfd505846b3589f33a394e45057bf97e2244`.
  HEAD alone is not this baseline. Executable/source correspondence was not
  assumed merely from existing executable hashes.

No commits, pushes, remote synchronization, CI dispatch, or publication are
authorized. Experimental compute retains its existing NO-GO classification.

## Reproduced diagnosis

| Finding | Reproduction and actual behavior | Correction |
| --- | --- | --- |
| Listener destructor under task lock | Drop a subscription whose capture's destructor calls `context.snapshot()`: child watchdog detects deadlock. | Detach under lock, destroy afterward. |
| Probe destructor under task lock | Replace a cancellation probe with the same reentrant destructor: deadlock. | Replace under lock, destroy afterward. |
| Late execution guard | Observer panics on Running in `begin()`: task stays Running; second begin fails, cancellation stays Cancelling, removal is busy. | Guard after successful ownership claim, before notification. |
| Late subscription guard | Immediate delivery panics; no guard returns, yet later cancellation invokes that observer again. | Guard before immediate delivery; unwind unregisters it. |
| Queued cancellation skips rollback | Task owns native value before begin; Cancelled observer panics; created owner remains admissible. | Rollback and release before notification. |
| Double panic in executor cleanup | Operation panics, then Interrupted observer panics during guard destruction: Windows child exits `0xc0000409`. | Contain cleanup observer unwinds. |
| Finalized probe retained | Counted probe capture remains alive after successful finalization. | Release terminal probe outside lock. |

Sources: `crates/revenant-core/src/tasks/{context,cancellation,execution,managed}.rs`
and desktop `runtime/jobs.rs`. The reproduced mechanisms require durable Cargo
regressions; session-only probe provenance is not published. Watchdogs are
liveness checks, not performance measurements.

A public native-dispatch probe also showed that disposal acknowledgement is not
executor exit: disposal requests cancellation and history removal follows actual
completion. Clarify this boundary without making disposal wait indefinitely.

## Guarantees and design

Public snapshots remain owned independent values with unchanged serialization
and generated contracts. Ordinary observer panics propagate to their publisher.
Removing a subscription prevents future lookup, but an already captured callback
may still run. Concurrent callbacks can overlap; revision checks protect clients
against stale publication rather than serializing every Rust callback.

Cancellation is cooperative. Logical interruption cannot kill a native thread.
Admitted execution leases remain alive until actual execution exits. Successful
created owners retain the existing result contract; unsuccessful execution rolls
them back. Cleanup must not undo already committed successful terminal results.

Implement the following sequence:

1. Detach removed listeners and replaced probes within short mutex scopes, then
   destroy them after unlocking. Interpret adjacent replacement/removal paths.
2. Construct subscription guards before immediate delivery. Claim execution under
   the task mutex and establish the execution guard before notifying observers.
   Failed competing claims must not interrupt an existing executor.
3. Separate state settlement, detached cleanup, and notification. Commit terminal
   state, perform rollback and lease release, then publish. Queued cancellation
   must complete rollback even when its observer panics.
4. On execution-guard cleanup, finish required ownership work first; catch each
   observer unwind outside locks and continue remaining cleanup deliveries.
   Ordinary publication keeps its existing panic propagation. Document the
   distinction. Panic hooks still run before a caught panic.
5. Detach probes at finalization, preserving independently cloned probes already
   being checked. Destroy detached captures outside the task lock.
6. Test desktop single, serial, and independent execution with panicking sinks
   and operations. If bookkeeping fails, completion guards follow actual worker
   lifetimes; independent children share counted completion ownership. A
   supervising future unwinding must not release history before blocking children
   exit. Preserve admission/provider/input/result lifetimes and cancellation.
7. Clarify cancellation requested, logical settlement, executor exit, and history
   removal in client/native documentation. No new join API is planned.

Unwind containment covers ordinary Rust unwinding. Application destructors and
panic-payload destructors must not panic; aborting panics and foreign exceptions
are not a recovery guarantee. See Rust's
[catch_unwind](https://doc.rust-lang.org/std/panic/fn.catch_unwind.html) and
[Drop](https://doc.rust-lang.org/std/ops/trait.Drop.html) documentation.

No new public resource or snapshot representation is planned. Keep revisions
6/5/4 unless an actual public contract break is demonstrated; internal refactors
alone do not require a revision bump.

## Correctness acceptance

Durable regressions cover reentrant destruction on unsubscribe, replacement,
finalization, and failed subscription; observer panics at initial, Running,
progress, cancellation, and terminal delivery; original operation panic plus
cleanup observer panic in a subprocess; competing begins; exactly-once resource
destruction; failed-owner rollback; preserved successful terminal results;
queued/running disposal; held independent children; and twenty clean lifecycle
cycles. Assert history retention until actual workers exit.

The preceding audit passed 103 targeted existing core/desktop/client checks.
Those checks did not cover the newly reproduced defects and are not substitute
acceptance evidence for this implementation.

## Controlled copying experiment

Current `change()` clones retained task data even when its caller only returns
`()`, then clones it again for every observer. Desktop throttling is later.
Counting only 100 no-observer progress calls gave:

| Retained update | Allocation calls | Cumulative requested bytes |
| --- | ---: | ---: |
| None | 200 | 4,700 |
| 60,000-character JSON string | 300 | 6,004,700 |

The extra 6,000,000 requested bytes diagnose repeated unchanged-data copying.
They are not retained RAM, peak memory, elapsed time, or an A/B comparison.

After correctness acceptance, freeze baseline A: corrected lifecycle plus the
experiment harness before copying optimization. Candidate B has identical
correctness fixes, harness, locks, compiler, features, profile, workloads and
oracles. Preserve the original audit baseline separately.

Candidate B only avoids snapshots discarded by unit-returning mutations with no
observers, and moves the publication snapshot into the final observer instead of
cloning it again. Construct required snapshots under the task mutex. APIs returning
snapshots still supply an owned caller value. Preserve revisions, membership,
callback order, reentry, and already captured deliveries. Do not introduce shared
JSON, public Arc snapshots, typed caches, GPU execution, or scheduler changes.

Extend the existing benchmark protocol with core progress cases crossing update
sizes 0/1,024/60,000 characters and 0/1/8 observers; 100 verification iterations
and 5,000 measured iterations per repetition. Add a complete embedded native
dispatch/progress/disposal case. It excludes WebView IPC and Svelte presentation.
Correctness covers legacy outcomes, terminal results, update limits, concurrent
revisions, reentry, unsubscribe during delivery, and independent owned snapshots.

Prepare inputs outside measurement. Count allocation calls and cumulative
requested bytes independently of retained memory. Instrument both sides equally;
disable allocation counting during timing. Use the normal preparation workflow:

```powershell
python scripts/benchmark_suite.py prepare callback-copy-baseline
python scripts/benchmark_suite.py verify callback-copy-baseline
python scripts/benchmark_suite.py prepare callback-copy-candidate
python scripts/benchmark_suite.py verify callback-copy-candidate
```

Bundles preserve copied executable hashes, dirty source identities, locks,
compiler/features/profiles/configuration and fixed workload descriptors. Timed
collection requires fresh explicit machine-readiness confirmation, then
`run --idle-confirmed`, quiet preflight, three warmups, fifteen repetitions and
identical case selection. Stop on failed preflight; do not relax thresholds.
Use the comparator's 95% intervals and 5% practical threshold; report each metric.

Retain B only with correct equivalent outputs, demonstrated allocation reduction,
bounded retention and no material measured regression. Allocation reduction can
be verified while timing remains inconclusive; that does not mean Revenant is
faster. Rejecting B removes only its isolated patch, preserving correctness fixes
and prior unpublished work. Pending timing leaves integration provisional.

## Adoption and continuation

Run the new regressions and affected existing suites; verify generated contract
stability and docs. Rebuild the CLI and deliver accepted changes through ordinary
SDK upgrades, without editing consumer snapshot files by hand. Verify Strata and
Luma source identities, SDK checks, existing tests/builds, retained backups and
recovery fixtures. Update architecture, decisions, implemented evidence, and
source-hash coverage only for interpreted files.

Classify correctness, allocation evidence, authorized timing, authoring costs and
delivery independently as necessary correctness improvement, verified benefit,
accepted tradeoff, inconclusive, or rejected. Continue broader provider,
authoring/generation, graphics, support, physical viewport, packaged operation,
application-performance and file-coverage work afterward. This pass must not
close `revenant-sun`.
