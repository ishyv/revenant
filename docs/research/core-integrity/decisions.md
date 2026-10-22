# Decisions and evidence states

The baseline has been frozen outside the checkout. The accepted changes below are narrow corrections against reproduced behavior; compatibility remains manifest 6 / protocol 5 / SDK 4 because signatures and contracts did not change. Completion of these corrections is not completion of the integrity milestone.

## Settings compatibility — `revenant-sun.6`

**Diagnosis.** Native merge preserves unknown object fields and validates array positions against corresponding defaults. Client `compatible()` required exactly the default key count and checked every array element against the first default element. Persisted settings with an old extra key failed to load; heterogeneous defaults could reject their own unchanged value.

**Design.** Keep defaults as constraints on known fields. Require each declared object field in a client edit, allow unknown JSON, and compare array elements only where a corresponding default exists. Preserve short arrays and unknown tails. Retain JSON validation, immutable copies, save serialization, error classification and native byte/depth limits. Rejecting unknown fields would instead make schema evolution destructive; treating the first array element as a homogeneous schema was inconsistent with native persistence.

**Experiment.** Three new Settings tests failed before the correction. The corrected client passes unknown-field load/edit/save, heterogeneous/default arrays, shortened arrays, unknown tails and rejection of wrong known types/non-JSON. Two public native-dispatch tests verify matching persistence behavior and unchanged storage after rejected saves.

**Outcome: necessary correctness improvement.** This removes an author-visible incompatibility. No throughput, memory or independent-reader comprehension claim follows from these tests. Preserving unknown data can retain storage that an explicit application migration would otherwise remove.

## Store reentry — `revenant-sun.7`

**Diagnosis.** An invalidator writing `2` during `set(1)` produced subscriber history `[0,2,1]` while the current value was `2`. The original publication was enqueued after calling the invalidator.

**Design.** Reserve every delivery for the original publication first, establish the global flush guard, then invalidate synchronously and drain in order. Keep delivery-time unsubscribe checks. Deferring invalidators until a later publication was rejected because Svelte derived stores use synchronous invalidation to coordinate dirty inputs.

**Experiment.** The original implementation failed the ordering fixture. The corrected implementation passes two-subscriber invalidation reentry, cross-store nested run callbacks, unsubscribe/clear during invalidation, and a real Svelte derived-store graph. The full client and mounted UI suites were also exercised.

**Outcome: necessary correctness improvement; accepted allocation tradeoff.** One live-subscriber snapshot per write prevents mutation of the iteration from changing the publication membership. It has not been profiled. Callback exceptions continue to be surfaced asynchronously without stranding the publication queue.

## Native sink reentry — `revenant-sun.8`

**Diagnosis.** The task subscription held its throttle mutex while invoking a sink. A sink cancelling its running task reentered the subscription and blocked on the same mutex. View publication similarly retained a sink mutex temporary through an `if let` body; synchronous disposal tried to take that mutex.

**Design.** Decide whether to publish under the task mutex and release it before serialization/callback. Clone each view sink in a standalone statement, then invoke it after the guard drops. Retain the 50 ms progress throttle, terminal publication, page acknowledgement and selection leases. There is no new thread, public abstraction or SDK revision.

**Experiment.** Both public-runtime fixtures failed against the original implementation with a bounded child-process watchdog. Both pass after the correction. Child execution prevents a failing deadlock fixture from hanging the test runner. Native admission, cancellation, scope disposal, page acknowledgement, refresh and resource retention tests were retested.

**Outcome: necessary correctness improvement.** Reentrant cancellation and disposal now complete. This does not prove that every callback in the repository is invoked outside locks; full callback/lock interpretation remains part of the coverage ledger.

## Preview scope reclamation and benchmark startup — `revenant-sun.9`, `.18`

The 2026-10-13 corrections replace preview historical tombstones with current
resource-scope authority and start Windows benchmark children suspended until
job assignment. The correction evidence (historical local evidence; not included in this checkout) records
the failing reproductions, disposal/creation interleavings, retained-read and
descendant guarantees, source hashes, fresh checks and remaining limits. These
are local correctness corrections, with no new performance or release claim.

## Investigation candidates with no accepted replacement

| Finding | Current evidence | Decision boundary |
| --- | --- | --- |
| Discovery worker policy (`sun.10`) | Traversal directly acquires the shared worker semaphore and bypasses work-class counters/gates. | Reproduce contention and classify mixed CPU/I/O work before selecting a replacement policy. |
| Repeated operation decoding | Typed discovery, validation and execution each decode JSON; admission and execution repeat some checks. | Count decodes/allocations on equivalent inputs; preserve advanced executor and non-Send input support, direct execution validation, leases and provider pinning. |
| Preview HEAD I/O | The host currently reads preview bytes before returning an empty HEAD body. | Verify identical token, identity and range/header behavior before introducing a metadata-only response path. |
| Wire documentation drift | Transport refers to absent `docs/WIRE.md`; the request-union comment describes a narrower set than runtime dispatch. | Reconcile documentation and generated/public consumer behavior, rather than introducing another handwritten protocol authority. |
| Settings default inference (`sun.14`) | A real store with defaults `{ nullable: null, items: [] }` accepts persisted `[7]` as `items` but rejects a later string for `nullable`. | Erased TypeScript unions/element types cannot be reconstructed from defaults. Evaluate explicit or generated schemas with migration and authoring costs; do not describe the current check as validation of arbitrary `T`. |

These are diagnoses or source-backed candidates, not verified performance improvements. Gated changes require their own comparative evidence and migration decisions. Physical mixed-DPI/window interaction, twenty graphics lifecycle cycles, clean packaged operation, full coverage and ordinary SDK adoption remain milestone acceptance requirements in Beads.

## Cleanup diagnostics — `revenant-sun.13`

**Diagnosis.** Scope teardown, query cleanup and canvas teardown each appended to the same unbounded array. A fixture with 512 failing disposables retained 512 errors; each append copied the preceding history. This was diagnostic retention, separate from whether the native task itself was reclaimed.

**Design.** Centralize framework reporting in `Scope.reportCleanup()` and retain the newest 64 records. Preserve notification of every failure for live subscribers, reverse teardown order, native-scope cleanup and the existing Signal interface. The snapshot is recent history; an application requiring a durable log must collect the event stream with its own retention policy. The bound is on record count, not total bytes. Public manual writes to the existing Signal remain possible and are outside this framework reporting policy.

**Experiment.** The same fixture fails before the change and passes afterward: 512 notifications in teardown order, 64 retained records, correct classification, and one native-scope disposal. Full client, mounted UI and TypeScript checks were exercised after the change.

**Outcome: verified bounded-record benefit and accepted history tradeoff.** Retained record count falls from 512 to 64 in this fixture and remains capped for subsequent framework reports. No throughput, allocation count or elapsed-time claim is made. Historical records leave the snapshot; live notification is retained.

## Public guidance — `revenant-sun.11`

Corrected the nonexistent wire-document link, manifest-version comment and claim that the handwritten core request union was exhaustive. The SDK guide now explains what revision 4 adds to revision 3. The API-documentation checker also exposed 14 missing descriptions in grouped reads and surface events. The affected sources and checker were byte-identical to the frozen baseline before these documentation edits.

The grouped-read factory and constructor now share one documented slot-configuration type. Its fields describe required-result replacement and optional-result omission; the execution policy did not change. All 601 reachable public declarations pass the documentation checker. This is verified documentation coverage, not proof that an independent author understands the API.

## Interpreted alternatives rejected before implementation

A possible grouped-read hang after failed native cleanup was investigated. `Query.release()` catches disposal errors and reports them through the owning scope, so an unconditional repair to the group's `finally` block would have targeted the wrong path. The actual repeated diagnostic retention was reproduced and corrected instead. Admission after an unacknowledged cleanup remains a separate policy question; passing a delayed-ack test does not answer every transport-failure case.

## Callback captures — `revenant-sun.15`

**Diagnosis.** Removing a listener or replacing a cancellation probe destroyed
its final captured value under the task mutex. A captured destructor calling
`snapshot()` hit a bounded subprocess watchdog. Terminal settlement also retained
the probe longer than its documented lifetime. Immediate subscription delivery
could unwind before returning a registration guard and leave its observer live.

**Design and outcome: necessary correctness improvement.** Detach under a short
mutex scope, destroy outside it, and establish the subscription guard before
delivery. Terminal settlement releases the task's probe reference while an
outstanding checkpoint retains its clone. Owned public snapshots, callback order,
concurrent delivery and outstanding-delivery semantics are unchanged. Avoiding
callbacks entirely or serializing them would change useful public behavior.

## Settlement and worker lifetime — `revenant-sun.16`

**Diagnosis.** A Running observer panic preceded execution-guard construction;
queued cancellation could notify before rolling back created owners; a second
observer panic during executor unwind aborted the original child process.
Desktop batch Running panic left history unfinished, while coordinator panic
marked independent execution finished before a held child exited.

**Design and outcome: necessary correctness improvement; accepted ownership
cost.** Separate committed state and mandatory cleanup from notification. Ordinary
notification remains fail-fast; execution-guard notification catches each unwind
after cleanup. A private shared desktop completion guard counts actual blocking
readers, including independent children. One allocation per job and reference
counting replace manual supervisor completion marks. No new public type or
compatibility change is required. A join-on-disposal alternative is rejected for
this pass because a noncooperative native thread cannot be bounded by that API.

The existing owner-retention rule remains: a result or successful legacy outcome
preserves created owners; tasks without either roll them back. A retained batch
result descriptor counts as a result. This pass does not narrow that rule to
specific handles found inside the returned JSON. Resource and panic-payload
destructors remain the application's nonpanicking responsibility, consistent
with Rust's [unwind](https://doc.rust-lang.org/std/panic/fn.catch_unwind.html) and
[destructor](https://doc.rust-lang.org/std/ops/trait.Drop.html) boundaries.

## Copying candidate — `revenant-sun.17`

**Diagnosis.** Unit-returning mutations constructed an owned retained snapshot
even with no observers; publication then cloned it for every observer and
discarded the original. Desktop throttling occurs after that copying.

**Design.** Candidate B skips snapshot construction when no observer needs it,
captures membership and the required snapshot together otherwise, and transfers
the publication snapshot to the last observer of a unit-returning mutation.
Snapshot-returning APIs keep their independent caller value. Progress, legacy
outcomes, batch counters and typed publication use this private path. Guard
cleanup and snapshot-returning settlement remain unchanged. Shared JSON, public
Arc snapshots, input caches, scheduling and GPU changes are excluded.

**Outcome: verified benefit, adopted and delivered.** Corrected baseline A and B
keep identical compiler, locks, features, configuration, workloads and oracles.
The prior 100-call allocation diagnostics remain valid. Fifteen fresh-process
pairs now compare the ten copying cases under ordinary desktop load, with a
counterbalanced seed-378 schedule and the unchanged three-warmup/fifteen-repetition
native protocol. Utilization is metadata rather than an admission gate; external
CPU estimates reached 30.4167%. The earlier rejected quiet attempt remains
historical evidence.

Every paired timing interval envelope is below 0.95 B/A, passing the 1.05
non-regression criterion. Allocations decrease and all raw retention repetitions
match. B's exact source bytes are live and normally delivered to Strata/Luma;
public contracts and compatibility 6/5/4 remain unchanged. Whole-process CPU
diagnostics retain their separate setup/warmup/counting boundary. These results
establish improvements in the callback/embedded workloads and observed desktop
conditions; full application performance and the broader milestone stay open.
See [callback-upgrades.md](callback-upgrades.md#paired-desktop-results-2026-10-11)
and hash-bound evidence (historical local evidence; not included in this checkout).
