# Restricted compute CPU control

Measured on the Windows x86_64 development host with an optimized release build,
using `cargo run -p revenant-core --release --features experimental-compute
--example compute_experiment`. These are single-run CPU wall times, not GPU
measurements or a stable backend-selection threshold.

Compiled fixture/IR source identity:
`sha256:03a1798780e8b25e83e20bd22a48764cd8ad3090e8a7a8782dd77547cebce8b9`.
Package version remains 0.4.0. The example emits its compiled source identity on
each run; later source edits require a fresh measurement before comparison.

| Workload | Preparation | CPU IR | Exact composed CPU reference |
| --- | ---: | ---: | ---: |
| Resident: 1,048,576 u32 elements, 32 alternating wrapping multiply-three/add-seven instructions, 16 passes | 1.265 ms | 5808.810 ms | 1.493 ms |
| Transfer-heavy control: 128 u32 elements, one add-seven instruction | 0.0058 ms | 0.0037 ms | Same single instruction |

Every resident output element matched the exact affine-composition reference.
Every small output element matched wrapping input-plus-seven. Input construction
and IR preparation are disclosed separately from evaluation. No GPU dependency,
transfer, synchronization or readback is simulated by this CPU-only control.

The interpretation cost is substantial; a GPU win against that interpreter would
not establish utility against ordinary optimized CPU work. Public promotion
therefore remains **no-go** until the separate native GPU report demonstrates
complete-cost utility against the exact composed reference and discloses adapter
and preparation costs. The optional experiment has no automatic stable backend
selector. Exact 64-bit accounting remains outside the 32-bit numerical IR.

The restricted IR has six focused CPU conformance tests: wrapping boundaries,
ordered finite-float reductions, checked layouts/dispatch/byte round trips,
malformed and oversized inputs, scope-disposal/lease behavior, and cancellation
during evaluation with no partial output returned. The SDK has two focused tests
for typed builder isolation and bounded work/resource publication, plus runnable
and compile-fail authoring documentation. Ordinary CPU-only builds do not import
graphics dependencies.
