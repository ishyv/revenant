# Native workflows

Manifest6, protocol5 and SDK revision4 introduce issued scope identities and the
native workflow APIs. Project layout3 and package0.4.0 are unchanged. Upgrade
through `revenant sdk upgrade`; backup and restore use the ordinary SDK recovery
path. Scope creation takes an optional parent and returns the host identity.
Caller-selected IDs no longer admit scopes. Disposed issued IDs are recognized
without retaining one registry record per historical scope.

`TaskContext::application_paths()` borrows immutable host-resolved app/data/cache
information, including host overrides. It provides information rather than file
authority. `cancellation()` clones the task/parent/scope cancellation view;
`checkpoint()` remains available.

`TaskContext::work()` supplies ordered `map_ordered`, `stream_ordered` and
`map_reduce` helpers. Streams materialize32 items per window by default; outputs
from a failed window are withheld. Already consumed earlier windows and external
side effects cannot be rolled back. Native helpers join admitted chunks before
returning and retain captured execution leases through actual completion.

Desktop defaults are4 CPU workers,4 blocking-I/O workers and32 queued chunks.
Queued operation admission remains a separate budget. One CPU slot and one queue
slot are reserved for explicitly declared interactive reads when capacity permits.
Use `#[operation(read_only, work = "interactive")]` for bounded latency-sensitive
reads and `#[operation(work = "io")]` for blocking-I/O stages. Declared reads do
not automatically receive interactive scheduling. Nested waits lend the parent's
capacity, including a one-worker host. Waiting callers help their own window;
pool workers rotate owners and choose the first eligible chunk of each class.

Planned input/result window capacities are admitted before advancing the input
iterator, with an8MiB shared reservation limit. A rejected admission leaves the
iterator untouched. Exact empty inputs need no reservation; an unknown-length
iterator needs admission before its first probe, even if it turns out empty.
Ordered result storage is limited to64MiB per helper. Native runtime inspection
reports queue wait, active CPU/I/O work, cancellation settlements and reservation
peaks. These counters exclude temporary job captures and control storage, allocator
overhead, allocations behind application-owned pointers and driver allocations.
A returned ordinary `Vec` becomes caller-owned allocation; streaming avoids retaining all
outputs. These limits do not claim bounds on arbitrary application allocations.

| Budget | Default bound | Observable accounting |
| --- | --- | --- |
| Admitted operations / queued chunks | 4 active CPU operations + 32 queued operations; separate 32-chunk work queue | admitted/queued tasks, queued/active chunks, queue wait and cancellation settlements |
| Planned input/result window capacities | 8 MiB per service | current and peak reserved capacities; job/control allocations are separate |
| Helper / retained task results | 64 MiB per ordered helper; at most 256 tasks with 1 MiB encoded JSON per result | serialized retained JSON bytes; caller-owned output vectors and native pointees are separate |
| GPU surface allocations | independent bounded 50,000-primitive renderers and at most 16 attached native viewports | requested buffer bytes and reusable CPU staging capacity in graphics measurements; driver commitment unknown |
| Warm graphics assets | at most 2 devices and 4 format pipelines per device; immutable atlas per device | device/pipeline counts and known atlas bytes; measured atlas 1 MiB per device |

Limits for operation admission, retained JSON and file views are separate desktop
options; the native viewport ceiling is independent of the 64-file-view default.
Client groups retain one complete result with at most sixteen bounded
read outputs; group count and application-owned models remain author-owned memory.

Source-owned models can opt into `#[derive(WireProjection)]`. The derive generates
`ModelWire`, ordinary contract metadata, reversible `into_wire`/`from_wire` and
`From` conversions. Models support nested, optional, boxed and vector projections,
field renames, `#[wire(id)]` string nominal IDs and explicitly annotated
`#[wire(decimal)]` u64/i64 fields. Foreign models retain explicit adapters.
Installed input/output/update validators are immutable shared preparations pinned
to each admitted contract generation; ad hoc `validate_json` behavior is unchanged.

`queryGroup(scope, { required, optional })` accepts declared reads bound to that
exact scope. `load(requestKey, inputs)` publishes all eligible slots atomically.
Required failure retains the preceding complete group with its `dataKey`;
optional failure is a slot error and contributes no older optional data to the
new result. There are at most16 slots,1MiB pending inputs, one active load and one
newest pending client load. Disposal awaits native teardown acknowledgement;
cooperative native readers can retain their leases until actual exit. Group
settlement does not join native threads.

`LatestJobs` persists with its issuing scope and shared application state.
`capture(&context, key, capture)` captures native input and application versions
under the application's mutex, computes using that request's context outside the
lock, then compares owner/version and publishes under the same commit lock.
Each key has one active and one replaceable pending input; each caller waits for
its own Committed/Superseded/error settlement. Creation-task completion does not
close the coordinator. Scope closure stops further publication. The controller
does not create threads or infer ordering from application versions.

`OverlayCoordinator(scope)` issues awaited cover tokens. Native hiding settles
before covered content opens. A failed late-registration hide remains
unacknowledged: another cover retries and waits for success, or rejects. Attempted
hides remain eligible for restoration. `cover.close(removeContent)` awaits content removal
before eligible surfaces and focus restore. `CoveredModal` is the thin HyvUI
adapter; pass the coordinator to `NativeCanvas`. Covers overlap by reference
count, preserving manual suspension. Ordinary dialogs and manual canvas suspension
continue to work.

`Scene2D::patch(expected_revision, changes)` atomically validates explicit Add,
Update and Remove edits. Updates retain paint order; additions append; full
replacement determines complete order. A rejected patch leaves the scene intact.
Shared devices/pipelines/immutable atlas assets have bounded caches, while native
surfaces retain independent lifetimes. Settled patches upload changed ranges and
preserve each key's interpolation. CPU scene publication currently copies the
immutable primitive vector; the measurement report includes this cost.

Restricted compute remains feature-gated. CPU-only `experimental-compute` supplies
bounded typed buffers, numerical IR and SDK authoring. Graphics supplies optional
GPU execution of checked generated WGSL. Integer overflow wraps; ordered CPU
reductions and floating tolerance are explicit. Wide exact accounting stays on
the supported CPU path. The completed experiment's public-promotion decision is
NO-GO: GPU lost to the exact optimized CPU baseline. See the
[CPU control](research/2026-10-09-compute-cpu-control.md) and
[GPU decision](research/2026-10-09-compute-gpu-decision.md).

The standalone [native workflows fixture](../examples/native-workflows/README.md)
exercises the public modules without disk scanning. It does not establish Strata's
application performance gates. Publication and application migration are separate.
