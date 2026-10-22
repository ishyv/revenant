# Revenant native performance strategy

Research decision record · 2026-10-13 · `revenant-sun.19`

Revenant should make native applications inexpensive to coordinate and easier to
accelerate, while preserving ownership, cancellation, bounded resources, and the
UI the author chose. My recommendation is a lean runtime, adaptive execution of
suitable native work, and an evidence loop that lets agents explain and improve
the application. These are three connected responsibilities. A small framework
alone would leave the acceleration ambition unfinished; a universal parallel
runtime would promise semantics and speed that we cannot establish.

The problem is that application authors currently have to assemble resource
ownership, background work, admission, cancellation, publication, and profiling
themselves. They also have to decide which algorithms, data layouts, and execution
backends fit their workload. Agents face a second problem: they can edit all the
code, but often lack trustworthy evidence about where time and memory go. Revenant
can remove repeated coordination machinery and give both groups useful execution
primitives. Its value must exceed the cost of those primitives.

This report is research and a proposed experiment sequence. It introduces no
runtime API or changed production default, installs no profiler, collects no new
performance timings, and makes no new speedup claim. The checkout is unpublished
`main`, with substantial existing work. The accompanying
evidence inventory (historical local evidence; not included in this checkout) binds inspected
files and four 9Router reviews to their actual bytes. Historical measurements
below were checked against current repository records, not rerun here.

## The performance promise to aim for

For an author: **keep writing ordinary Rust operations and your own Svelte UI;
get safe native work with low coordination cost, and useful acceleration when
the workload supports it.** For an agent: **get bounded captures, reproducible
comparisons, execution explanations, and reversible controls.** The user’s
reference to “Spell” is interpreted here as the authored Svelte frontend.

“Use every available core” is a mechanism, not the objective. The objectives are
less time to a useful result, responsive interaction, bounded memory, and sensible
energy use. Sometimes the winning policy is serial execution or avoiding work.
Parallelism should respect a shared application budget rather than compete with
the UI and other operations for every core.

Framework footprint needs several separate measurements:

| Dimension | What to compare |
| --- | --- |
| Distribution/build | Native executable and dependency bytes; build time; optional feature cost. Keep WebView runtime packaging separate. |
| Startup | Process launch to usable app, service creation, contract preparation, and first native request. |
| Idle | Incremental CPU time, wakeups/context switches, helper thread count, private/committed memory, and retained framework objects. |
| Native request | Admission, decode/schema/handle work, queue wait, application computation, output encoding/retention, and publication. |
| Application response | Useful-result latency, throughput, cancellation settlement, and interactive tail latency under competing work. |
| Memory | Allocation calls/requested bytes, live retained bytes, working set, known resource/scratch bytes, and separately unavailable GPU driver heaps. |

Use two controls. An identical plain Tauri shell isolates the incremental cost of
loading Revenant. A feature-equivalent direct Rust implementation includes the
ownership, cancellation, validation, and publication behavior needed by the real
application; it compares what Revenant replaces. A bare function is a useful
compute lower bound, but not a substitute for that second control. Report absolute
deltas as well as ratios, especially when the baseline is tiny. Do not attribute
the entire WebView process footprint to Revenant: Tauri uses platform WebViews,
whose distribution details vary by OS. [Tauri WebView reference](https://v2.tauri.app/reference/webview-versions/)

## What the current implementation tells us

These are source observations and candidate costs, not profiler attribution.
Line anchors refer to the inspected snapshot; hashes are in the inventory.

| Current behavior | Source | Implication and question |
| --- | --- | --- |
| Defaults specify four CPU and four blocking-I/O slots; construction eagerly starts their combined eight helper threads. | [options.rs](../../crates/revenant-desktop/src/options.rs#L58), [work.rs](../../crates/revenant-desktop/src/work.rs#L93), lines 122–137 | This is eight work-service helpers, not the application's total thread count. Can unused helpers be lazy, and can truly empty queues park on events? |
| Helpers and waiting callers use 2 ms timed condition-variable waits when they cannot progress. | [work.rs](../../crates/revenant-desktop/src/work.rs#L128), lines 272–278 | A plausible wakeup cost. No idle CPU percentage or actual wake rate was measured. Removing every timeout could strand work awaiting external completion or released permits. |
| Native dispatch builds context/labels and records structured lifecycle traces; its size check serializes a `Value` into a temporary byte vector. The Tauri host has a separate request size guard. | [dispatch.rs](../../crates/revenant-desktop/src/runtime/dispatch.rs#L43), [tauri_host.rs](../../crates/revenant-desktop/src/tauri_host.rs#L311) | Inspect call paths before calling a guard redundant. Measure bounded counting serialization versus allocating the byte vector. |
| The trace ring stores bounded JSON events, labels, sequence/drop counts, and wall-clock milliseconds. | [inspect.rs](../../crates/revenant-desktop/src/runtime/inspect.rs#L45) | Valuable lifecycle evidence, but neither a native CPU profiler nor a monotonic phase trace. Consider deferred encoding while preserving the inspection contract. |
| Typed input decoding occurs in handle discovery, typed admission, and execution methods; the prepared host path validates again at execution. Output handle discovery also decodes. | [registration.rs](../../crates/revenant-sdk/src/operations/registration.rs#L53), [prepared.rs](../../crates/revenant-sdk/src/operations/prepared.rs#L26), [registry.rs](../../crates/revenant-sdk/src/operations/registry.rs#L115), [jobs.rs](../../crates/revenant-desktop/src/runtime/jobs.rs#L931) | Count invocations on each successful/error path before claiming a total or dominant cost. A prepared typed fast path must retain pinned generations, leases, custom decoding, and non-`Send` author input support. |
| Contract validators already belong to a prepared installed operation generation. | [prepared.rs](../../crates/revenant-sdk/src/operations/prepared.rs#L12) | “Cache schemas” is not a missing feature. Remaining work is measuring repeated traversal/decoding and what each boundary requires. |
| Ordered work materializes bounded windows, reserves known inline scratch, boxes jobs, and coordinates result slots through an `Arc<Mutex<_>>`. The desktop service adds completion channels, queue tickets, and notifications. | [services.rs](../../crates/revenant-core/src/tasks/services.rs#L196), lines 245–285; [work.rs](../../crates/revenant-desktop/src/work.rs#L406) | Grain, preparation, result coordination, and queueing are candidate overheads. Pointer-owned application allocations and job/control storage are outside the inline scratch counter. |
| Waiting parents already lend permits; execution can help make progress. | [work.rs](../../crates/revenant-desktop/src/work.rs#L201) and the `execute` path | Inline help is already present. Adapt grain and execution policy within admitted native work; never move CPU work onto an arbitrary UI caller. |
| Folder traversal directly acquires shared worker capacity outside work-class statistics. | [views.rs](../../crates/revenant-desktop/src/runtime/views.rs#L168), `revenant-sun.10` | Establish actual contention and CPU/I/O mix before choosing a class or changing capacity. |
| Native surfaces/GPU dependencies are optional; SDK experimental compute is opt-in. | [desktop Cargo.toml](../../crates/revenant-desktop/Cargo.toml#L9), [SDK Cargo.toml](../../crates/revenant-sdk/Cargo.toml#L8) | Preserve modularity. Audit linked/default costs rather than treating every workspace crate as shipped in every application. |
| Development inspection already exposes source freshness, semantic/diagnostic queries, runtime inspection, and traces. Native snapshots include work counters and ownership. | [inspection backend](../../src/inspection/backend.rs#L69), [source fingerprints](../../src/inspection/mod.rs#L116), [runtime snapshot](../../crates/revenant-desktop/src/runtime/inspect.rs#L161) | Extend this foundation with performance provenance and attribution; do not build a competing development control plane. |

The root manifest has no explicit profile stanza in this snapshot. That does not
establish the effective build configuration: Cargo configuration, environment,
target flags, and dependency overrides also matter.

## Existing results that should discipline the design

The [October 9 scheduler measurement](2026-10-09-native-workflow-scheduler-measurements.md)
used the same wrapping `u32` algorithm, 32 rounds, 1,024-element chunks, four CPU
slots, four I/O slots, and queue capacity 32. Exact results and budget/cleanup
checks passed. Its median results were:

| Entries/session | Sessions | Serial ms | Bounded ms | Bounded/serial |
| --- | ---: | ---: | ---: | ---: |
| 500,000 | 1 | 29.86 | 140.30 | 4.70 |
| 500,000 | 4 | 35.61 | 61.75 | 1.73 |
| 1,000,000 | 1 | 63.79 | 295.02 | 4.63 |
| 1,000,000 | 4 | 69.43 | 106.16 | 1.53 |

These five-repeat, alternating-order samples under normal desktop activity
establish a loss for this fixture and configuration. They do not locate its cause
or establish that all native work should be serial. With five repetitions, the
reported nearest-rank p95 is the worst observed sample, not a population tail
estimate. The competing interactive fixture stayed within its 50 ms guard.
`revenant-378.11` retains later frozen-baseline evidence as well; do not combine
different campaigns into a claimed before/after improvement.

The [restricted compute decision](2026-10-09-compute-gpu-decision.md) remains no-go
for promotion. For 1,048,576 `u32` elements, 32 alternating multiply/add nodes, and
16 resident passes, the scalar CPU interpreter took 6,904.23 ms, equivalent
composed affine CPU work 1.63 ms, GPU cold total 227.10 ms, and warm total 5.77 ms.
For 128 elements and one add, CPU was 0.0047 ms and warm GPU 0.702 ms. These are
local samples, not robust comparisons across adapters. GPU stages used CPU wall
timing, not GPU timestamps. The lesson is strong: an interpreter-only baseline
can manufacture a spectacular-looking GPU win while losing to a better CPU
algorithm. Simplify work before choosing its processor.

The [prepared-validation measurements](2026-10-09-prepared-validation-measurements.md)
also resist a universal story: the small `FileEntry` native roundtrip improved
from 530.588 to 9.3491 µs, while the 512-entry case was about 6.17 versus 6.33 ms.
Preparation helped one shape without establishing an end-to-end win for every
shape. Independently measured or overlapping stages must not be added into a
fictional total.

There is accepted evidence that narrower framework changes can help. The
[callback-copy campaign](core-integrity/callback-upgrades.md)
has fifteen fresh process pairs, exact correctness checks, and bounded-copy ratio
intervals below its acceptance limit. Its `dispatch.progress` medians were
16.4180 versus 8.1366 ms. That change is already accepted and delivered in the
current repository record. It excludes WebView IPC and Svelte presentation; it
is not a new result of this research.

## What to borrow from Bend, and what to leave behind

The reference must be versioned. Current
[Bend 2](https://github.com/bendlang/bend/blob/32d23b434be1634b7e2858870b2f9d7da8f8912a/README.md)
is a different language/runtime direction from
[Bend 1](https://github.com/HigherOrderCO/Bend1/blob/814453670d0e0d6777c1313c972764dba0491b7f/README.md)
and [HVM2](https://github.com/HigherOrderCO/HVM/blob/654276018084b8f44a22b562dd68ab18583bfb5b/paper/HVM2.typst).
The pinned Bend 2 README explicitly says Bend 1 programs and HVM do not carry
over. Bend 1's interaction-net runtime exposed independent graph rewrites; this
is not automatic optimization of arbitrary compiled Rust instructions. Its
single-core limitations also matter when comparing a whole application.

The [Bend 2 guide](https://github.com/bendlang/bend/blob/32d23b434be1634b7e2858870b2f9d7da8f8912a/guide/GUIDE.md)
uses pure affine semantics and explicit parallel calls. A parallel call promises
independence and roughly balanced duration; native fork/join work is assigned
without work stealing. CPU and GPU execution have different forms, and its
JavaScript target runs sequentially. The inspected README does not support native
Windows; numeric types and effects are narrower than general Rust. Its performance
claims are project claims, not Revenant measurements.

Borrow the low author burden, structured task lifetime, explicit independence,
and the connection between semantics and execution. Do not import a new VM to
host all application Rust, promise its headline scaling, or claim that a Rust
closure is pure because it implements `Send + Sync`. A `Sync` mutex or atomic can
still mutate shared state. [Rust's Send/Sync explanation](https://doc.rust-lang.org/nomicon/send-and-sync.html)

“Rust is low level” does not give Revenant enough information to reorder it.
I/O, aliasing, interior mutability, panics, cancellation, error selection,
observable side effects, and floating-point association are semantics. Ordered
result publication does not impose ordered side effects. Parallel reduction
requires a declared law or tolerance; a fixed parallel tree need not match a
sequential floating-point fold. [Rayon reduction contract](https://docs.rs/rayon/1.12.0/rayon/iter/trait.ParallelIterator.html#method.reduce)

## Execution approaches worth testing

| Approach | Useful contribution | Decision |
| --- | --- | --- |
| Improve the existing bounded CPU service | Adaptive grain, fewer per-item control objects, shared budget, bounded tails, nested progress. | First execution experiment. Keep serial fallback and existing ownership semantics. |
| Bounded Rayon-style fork/join control | Mature work stealing for independent CPU work; useful comparison for skewed jobs. | Benchmark control first. A second default pool would risk oversubscription; shared admission must be designed before adoption. Blocking dependent branches can deadlock. [Rayon join](https://docs.rs/rayon/1.12.0/rayon/fn.join.html) |
| Tokio async/blocking facilities | Async coordinates waiting; native blocking workers execute CPU operations. | Retain the distinction. `spawn_blocking` alone is not a CPU policy, and started blocking work is not forcibly abortable. [Tokio documentation](https://docs.rs/tokio/1.53.3/tokio/task/fn.spawn_blocking.html) |
| Optional typed native pipeline | Map/filter/projection, explicit reductions, fusion, cacheable pure stages, and resource-resident data can expose optimization opportunities. | Prototype a few operators only after a CPU control exists. Require semantic declarations; retain ordinary Rust operations. |
| Compiler assistance | Better inlining/vectorization, profile-guided layout/inlining, and LTO can improve existing code without a new execution language. | Useful development tooling and measured build experiments. LLVM already has loop and SLP vectorizers. [LLVM vectorizers](https://llvm.org/docs/Vectorizers.html) |
| Polyhedral loop optimization | Fusion, tiling, locality, and parallel generation for supported loop regions. | Research reference, not a turnkey supported optimizer for arbitrary Rust. Polly documents a Clang integration. [Polly architecture](https://polly.llvm.org/docs/Architecture.html), [Clang/OpenMP path](https://polly.llvm.org/docs/UsingPollyWithClang.html#automatic-openmp-code-generation) |
| Optional GPU kernel/operator backend | Enough independent work or data already resident on the GPU can amortize preparation and transfers. | Keep experimental. `wgpu` consumes shader modules/entry points, not arbitrary Rust closures. [wgpu pipeline descriptor](https://docs.rs/wgpu/30.0.1/wgpu/struct.ComputePipelineDescriptor.html) |
| Replace application execution with Bend/HVM | A different language and semantic universe. | Reject as the default Revenant architecture. It expands the project before proving value for native Rust authors. |

Adaptive execution should estimate whether the work saved exceeds scheduling,
materialization, synchronization, and publication costs. Small work stays serial
on an admitted native worker. Larger independent work gets bounded chunks; skewed
work may benefit from stealing. Bounds, semantic mode, and measured history constrain
the choice. An element-count threshold alone is insufficient: an element can be
an integer addition or a large image transform. Default scheduling can learn a
cost estimate without secretly rerunning effectful application operations.

The existing ordered helper has finite window barriers. It is not automatically
a resident producer/consumer engine. `revenant-sps` contains a concrete bounded
channel saturation case and an alternative finite-frontier prototype. Preserve
that evidence and compare actual consumer designs before adding another streaming
abstraction. A faster general map is not a cure for a mismatched streaming model.

## More opportunities than “add threads”

The following are hypotheses to investigate, not features to announce:

| Idea | Why it might help | Constraint or rejection rule |
| --- | --- | --- |
| Event parking and lazy service creation | Reduce unnecessary idle wakeups, threads, and startup work. | Reliable notifications for enqueue, capacity changes, external readiness, cancellation, and shutdown; preserve progress under races. No universal zero-idle-CPU promise. |
| Compact trace records, encode on inspection | Move formatting and JSON construction off the request hot path. | Owned labels/IDs, compatible sequence/drop/error evidence, bounded storage, and off/on measurement. “Disabled” must have measured cost. |
| Bounded counting writer for size checks | Avoid allocating a byte vector merely to measure JSON. | Same serializer and exact escaped byte accounting; fail at the bound. It still traverses the payload. |
| Generation-pinned prepared input | Avoid repeated decode/validation when trust and lifetime proofs permit reuse. | No weakened schema/handle checks, lost leases, provider replacement races, or accidental `Send` requirement. Measure each path first. |
| Coarser jobs and notification batching | Amortize boxes, tickets, channels, mutexes, and wakeups. | Coarse work worsens cancellation and interactive tails; `notify_one` is not automatically safe or fair. |
| Filter/projection pushdown and fusion | Avoid decoding, copying, computing, or publishing data that the consumer does not need. | Preserve errors, effects, field contracts, and cancellation checkpoints. Polars is a useful example of semantic lazy optimization. [Polars optimizer](https://docs.pola.rs/user-guide/lazy/optimizations/) |
| Typed resident/columnar resources | Reuse native data across operations and expose contiguous access to vectorization. | Scoped leases and mutation/publication rules; columnar data can penalize mutation. Shared-memory zero-copy does not mean zero-copy across WebView IPC. [Arrow format](https://arrow.apache.org/docs/format/Columnar.html) |
| Incremental recomputation and latest-only work | Avoid recomputing stable inputs and obsolete results. | Use existing resource, latest-job, and grouped-publication machinery where it fits. Memoization needs purity plus revision/version keys; cancellation cannot undo external effects. |
| Native binary transfer or bounded channels | Reduce JSON work for suitable bulk data while preserving the chosen UI. | Small control messages remain typed; account transfer ownership/copies and backpressure. Tauri provides raw IPC responses and channels. [Calling Rust](https://v2.tauri.app/develop/calling-rust/) |
| SIMD and locality-aware batches | Improve ordinary CPU kernels before distributing them. | Inspect generated code first; require runtime feature detection/portable fallbacks for explicit intrinsics. [Rust architecture intrinsics](https://doc.rust-lang.org/std/arch/index.html) |
| PGO/LTO and binary layout | Optimize a real workload's compiled code and footprint. | Representative training, held-out cases, identical build identities, startup/code-size tradeoffs. [Rust PGO](https://doc.rust-lang.org/rustc/profile-guided-optimization.html), [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) |
| Allocation/lifetime improvements | Reduce temporary churn or retained histories where profiles identify them. | Requested bytes are not live memory. Start with ownership/dataflow; compare allocator substitution only afterward. [mimalloc project](https://github.com/microsoft/mimalloc) |
| CPU scene publication before GPU tuning | Reduce full-scene work for small changes. | Existing `revenant-378.12` records allocation evidence; preserve immutable revisions, ordering, and interpolation. Low GPU upload share does not excuse CPU cost. |
| Resident GPU execution | Avoid repeated upload/readback and preparation. | Best equivalent fused CPU baseline; full cold/warm app costs; device-loss/fallback behavior. Data transfers are a first-class cost. [GPU transfer guidance](https://docs.nvidia.com/cuda/cuda-c-best-practices-guide/index.html#data-transfer-between-host-and-device) |

The CUDA source supplies a general data-movement principle, not a recommendation
to add CUDA to this Windows/`wgpu` stack. None of these ideas warrants changing an
author's UI design. Agents can measure IPC, publication cadence, and expensive
native requests, then optimize their implementation within the same UI contract.

## A simple author surface and a detailed agent surface

For authors, evolve existing operation/context and work helpers before inventing
another framework language. Shared limits, ownership, and cancellation remain
automatic. Suitable finite work can choose serial or bounded parallel execution
without exposing thread bookkeeping. A small optional typed pipeline would let
authors express independent transforms and reductions where stronger guarantees
enable fusion or backend selection. Effectful ordinary Rust remains ordinary Rust.
Do not infer purity, speculate by running it twice, or silently change arithmetic.

For agents, build a development loop around **capture → explain → propose →
compare → adopt**. There is already a useful foundation: development tools include
`project.inspect`, `code.query`, `diagnostics.list`, `runtime.inspect`, and
`trace.query`; source fingerprints and running generations make freshness
explicit. Native snapshots expose ownership and work counters. The missing layer
is trustworthy performance attribution and the reproducible optimization loop.
Build on the existing [development/application boundary](../ARCHITECTURE.md#L56).
The following describe proposed performance capabilities, not newly available commands:

| Capability | Required evidence/control |
| --- | --- |
| Capture | Bounded duration/bytes; owned app/process tree; explicit native phases and external sampling; optional actual WebView trace; completeness/drop counters. |
| Fingerprint | Source/SDK/artifact/fixture hashes, build/target/features, adapter/driver, workload and session identity, execution policy, profiler settings, and clock domains. |
| Explain | Separate queueing, admission, framework traversal, user computation, output/IPC/publication; source-bound sampled stacks and compiler remarks; uncertainty and missing fields. |
| Compare | Frozen equivalent work, exact/tolerance-defined results, all samples, environment observations, allocation/retention separation, instrumentation off/on, and matched controls. |
| Control | Explicit budget/grain/backend settings for experiments, finite allowed values, automatic expiry/reset, no change to unrelated apps or stable defaults. |
| Adopt | A reviewed source diff and reproducible evidence, followed by normal SDK/consumer checks. A profile never authorizes publication or an unrelated application rewrite. |

A proposed capture record needs no invented public schema URL. It should contain
`schemaVersion`, provenance, phase events, sampled evidence, clocks, availability,
drops, correctness, and comparison results. Unknown counters remain `null` with a
reason. GPU buffer accounting and unknown driver heaps are different fields.
Native wall time, sampled CPU time, browser events, and GPU timestamps are
different measurements. Clock calibration must report its error; overlapping
phases are not additive. Linux-only clock names do not belong in a fabricated
Windows record.

Useful native phases are request ingress, admission/schema/handle work, queue
wait, user computation, output validation/encoding/retention, and publication.
Use monotonic duration events where possible, plus IDs and parent relationships.
Do not log application payloads or credentials by default. Maintain development
inspection separately from application-author-exposed tools and retain bounded
native execution.

There are complementary profiler lanes:

| Lane | What it can establish | Limit |
| --- | --- | --- |
| Existing lifecycle inspection | Requests, task/resource ownership, state/error history, work counters. | Current wall-clock JSON ring is not phase/CPU attribution. |
| Native instrumented zones | Phase duration, nested regions, selected locks/allocations/GPU work. Tracy is one option. [Tracy](https://github.com/wolfpld/tracy) | Rust integration and Windows cost need a real prototype; measure disabled and enabled builds. |
| OS sampling/ETW | Native sampled stacks, CPU scheduling/waits, system/process context. [Windows Performance Recorder](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/windows-performance-recorder) | Samples are statistical, not every exact instruction; symbols/provider selection and privilege requirements must be verified. |
| Actual WebView performance | Browser events, main-thread work, IPC and presentation context. [Chrome Performance](https://developer.chrome.com/docs/devtools/performance), [Playwright WebView2](https://playwright.dev/docs/webview2) | Verify attachment to the owned Tauri WebView2 host. A plain browser tab is not the packaged native app. |
| Interaction replay | Repeatable actions, screenshots and network context. [Playwright traces](https://playwright.dev/docs/trace-viewer), [Tauri WebDriver limits](https://v2.tauri.app/develop/tests/webdriver/) | Playwright action traces alone do not establish CPU sampling or paint cost. Platform/driver support differs. |
| Compiler diagnostics | Passed/missed/analysis optimization remarks and PGO experiments. [LLVM remarks](https://llvm.org/docs/Remarks.html) | Remarks explain compiler decisions, not observed app bottlenecks. PGO training instrumentation has its own cost. |

Local inventory found `wpr.exe` and `llvm-profdata.exe`. `wpa`, `xperf`,
`cargo-bloat`, `cargo-flamegraph`, and `samply` did not resolve on PATH; that does
not prove they are absent from the machine. `rustc 1.98.1` reports LLVM 22.1.8;
the discovered external `llvm-profdata` is 23.1.2. Their profile compatibility was
not tested. No live ETW capture, profiler integration, or WebView attachment was
performed in this research.

## Design considerations and disagreements

These are technical perspectives, not independent-model consensus or fresh
validation of the current checkpoint.

| Perspective | Useful contribution | My adjudication |
| --- | --- | --- |
| Runtime critic | Decode reuse, counting writer, event parking, deferred trace formatting, batching notifications. | Good candidate list. Its strong attribution of the scheduler loss to governance is unsupported without profiles. Borrowed labels and changed notifications require ownership/progress proofs. |
| Adaptive-compute advocate | Fusion, serial fallback, deterministic reductions, vector-friendly batches, balanced tasks, execution tiers. | Keep the semantic direction. Reject a universal item threshold, arbitrary caller-thread execution, and the suggestion that a fixed reduction tree equals sequential floating arithmetic. Existing caller help must be acknowledged. |
| Profiling-methods reviewer | Lifecycle, native zones, ETW, UI replay, compiler evidence; off/on instrumentation comparisons. | Keep distinct lanes. Reject fictional schema URLs/sample timings, Linux clocks in Windows examples, “exact stacks” from sampling, and treating Playwright traces as a paint profiler. |
| Product director | Lean defaults, strong development feedback, plain product promise. | Keep those. Reject the lean-only conclusion that Revenant should abandon app acceleration, and universal zero-percent CPU/zero-allocation promises. The user's intended second role remains essential. |

## Experiment sequence and adoption rules

Beads owns the durable tasks. This table explains the design, not a second TODO
tracker. All experiments below are proposed; the current research task does not
authorize a new timed campaign or production implementation.

| Stage / Beads | Bounded experiment | Closing decision |
| --- | --- | --- |
| E0 · `revenant-sun.20` | Freeze identical plain-Tauri and feature-equivalent native controls. Design/verify phase fingerprints and instrumentation off/on. Cover idle/startup, tiny request, bulk request, cancellation, nested work, competing sessions, and one actual WebView interaction. | A reusable cost map with truthful availability and agreed per-workload budgets; identify the first measured bottleneck. |
| E1 · `revenant-sun.21` | Test one lean-runtime/transport candidate at a time: reliable empty-queue parking/lazy helpers, trace representation, counting writer, or prepared input. | Adopt only an isolated useful improvement with preserved authority, wakeups, compatibility and tails; otherwise retain the current path. |
| E2 · `revenant-378.11`, `revenant-sun.10`, `revenant-sps` | Compare current helpers, optimized serial, and a budgeted Rayon-style control. Sweep grain and workload cost/skew; include nested calls, saturated bounded streams, cancellation, and several sessions. | Select policy and fit from evidence. Preserve serial fallback; do not replace the resident streaming engine on a map-only result. |
| E3 · `revenant-sun.22` | Prototype a few typed CPU pipeline operators, fusion/pushdown, and scoped resident data. Compare against the best equivalent hand-written CPU algorithm. | Keep only operators that remove real author machinery or improve the declared app objective. Reject a large generic IR before that proof. |
| E4 · `revenant-sun.23` | Verify profiler/toolchain integration, then representative PGO/LTO/SIMD and agent explanation experiments with held-out cases. | Adopt useful development feedback or a build option without assuming one profile fits every user. Report portability, startup and size tradeoffs. |
| E5 · `revenant-sun.24` | Test one supported resident GPU kernel after optimized CPU controls; include complete cold/warm paths and fallback/device-loss behavior. | Promote only a reproducible app-level benefit. Otherwise the existing experimental no-go stands. |

E0 is first; E1 and E2 are the immediate engineering priorities. E3 and E4 can
proceed once the relevant baseline/evidence lane is usable. E5 is gated behind
the CPU semantic/algorithm experiments. Existing scene-allocation work
(`revenant-378.12`) remains a targeted application-cost investigation.

Before each timed comparison, freeze the workload, output oracle, source/build
identity, objective, metric directions, absolute latency/memory limits, and
relative improvement/regression limits. Use the existing
[benchmark protocol](../BENCHMARKS.md) where applicable: correctness preflight,
fresh paired processes and declared warmups/repetitions, preserved host observations,
and uncertainty. Fifteen process pairs are the existing paired campaign standard,
not proof that every tail percentile is precise. Do not discard loaded-desktop
samples or “adjust” their times by CPU utilization.

Keep the existing 50 ms interactive guard for its fixture and choose actual app
response/frame requirements separately. If a native timing is below the noise
floor, use absolute cost and aggregation rather than impressive percentages.
Adoption needs a useful improvement on the target objective, no unacceptable
regression in declared comparison cases, and complete correctness/resource
checks. Report failures and rejected candidates as results. A faster median with
broken cancellation, unbounded retention, or worse interaction is not acceptance.

The research is complete when this source map, primary-source comparison, review
adjudication, and staged plan are inspectable. It does not complete the wider
integrity milestone or establish app-level acceleration. The direction I would
take next is E0 followed by one measured E1 or E2 change—not a new universal VM.
