# Restricted native compute: no-go for promotion

Decision: keep the CPU/checked-IR/GPU modules behind `experimental-compute`.
Do not promote an automatic GPU path, a general closure compiler or a stable
production compute API from these results.

The local release experiment ran on 2026-10-09 using Windows DX12, AMD Radeon RX
7800 XT, wgpu 30.0.1 and the pinned bundled DXC compiler. CPU and GPU used the
same frozen core fixtures at package 0.4.0, with shared compiled fixture identity
`sha256:03a1798780e8b25e83e20bd22a48764cd8ad3090e8a7a8782dd77547cebce8b9`.
The JSON report also records source hashes for the core IR, GPU executor and test.

| Workload | CPU IR | Equivalent composed CPU | GPU cold total | GPU warm total |
|---|---:|---:|---:|---:|
| 1,048,576 u32 elements, 32 alternating multiply-three/add-seven nodes, 16 resident passes | 6904.23 ms | 1.63 ms | 227.10 ms | 5.77 ms |
| 128 u32 elements, one add-seven transform, complete upload/readback | 0.0047 ms | Same single transform | 116.98 ms | 0.702 ms |

The resident GPU result is much faster than the scalar IR interpreter, but loses
to the equivalent CPU algorithm. Wrapping multiply/add can be composed into one
exact affine transform before visiting the buffer. That CPU reference produces
exactly the same values, verified over the complete output. Calling the scalar
interpreter the only CPU baseline would manufacture an acceleration claim.
The transfer-heavy workload also strongly favors CPU. These are individual
local samples, rather than statistically established timings or cross-adapter
performance guarantees. The independent CPU-only control uses the same frozen
fixtures; see `2026-10-09-compute-cpu-control.md`.

Cold resident GPU preparation took 212.98 ms, allocation/upload enqueueing 7.87 ms,
encoding/submission 2.87 ms, completion/map wait 1.24 ms, and mapped copy/typed
decode 2.13 ms. Warm resident costs were 1.63, 0.47, 1.21 and 2.46 ms respectively.
Only the final resident output is read back; input upload is included. Upload and
submission timings measure enqueueing, while completion includes transfers and
GPU execution. These are CPU wall stages, not GPU timestamps.

Known GPU buffer allocations total 12,582,928 bytes for the resident workload and
1,552 bytes for the small workload. Conservative transfer payload scratch bounds
are 16,777,216 and 2,048 bytes, excluding container/allocator bookkeeping. Device,
pipeline and driver heaps remain unknown and are separate from these counts.
Resident passes are capped at 64, individual buffers at 1,048,576 elements, and
the checked IR at 32 instructions. Already submitted task-owned work retains a
cancellation token through the queue completion callback. Adapter/device errors
remain explicit; callers may select the CPU executor. Experiments use a separate
headless device and do not silently change scene rendering policy.

Actual GPU conformance passed u32, i32 and finite f32 element output and ordered
sum/min/max reductions, integer wrapping, direct floating overflow rejection and
intermediate overflow rejection even when a later minimum could otherwise hide
it. Reductions use the declared sequential order, including on GPU. Integer
results compare exactly; finite float results use abs/rel tolerance 1e-5. Exact
wide accounting remains on ordinary CPU paths.

Reproduce conformance and complete-cost measurements with:

```powershell
$env:REVENANT_COMPUTE_REPORT = '<revenant-checkout>\target\native-workflows-compute.json'
cargo test -p revenant-graphics --features experimental-compute --test compute_experiment --release -- --ignored --nocapture --test-threads=1
cargo run -p revenant-core --example compute_experiment --features experimental-compute --release
```

Default graphics builds do not expose compute. Core's CPU-only experiment does
not require GPU dependencies. Graphics' experimental feature deliberately enables
its GPU backend and the checked core IR together. A future promotion needs a
workload that beats a fair optimized CPU algorithm at complete end-to-end cost,
more adapters/platforms, production ownership/budget integration and explicit
acceptance evidence. This completed no-go result is independent of Strata
application migration or scan performance acceptance.
