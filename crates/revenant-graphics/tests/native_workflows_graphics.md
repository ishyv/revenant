# Native workflows graphics evidence

The local Windows DX12 runs on 2026-10-09 used an AMD Radeon RX 7800 XT, wgpu
30.0.1, the pinned bundled DXC compiler and release builds. The incremental
harness records compiled scene, renderer, shared-service, atlas-builder and WGSL
source SHA-256 values in its JSON result. Source changes invalidate these samples.

The 50,000-primitive settled fixture used 1280x768 physical-pixel surfaces with
labels disabled. Four independent surfaces retained one device, one format
pipeline and one 1,048,576-byte immutable atlas. Initial cold renderer creation
took 168.745 ms; the next three creations using retained assets took 3.55–3.96 ms.
Updating 500 colors uploaded 40,000 instance bytes per surface against a
4,000,000-byte complete frame, meeting the initial 10% upload target at 1%.
Each surface retained 5,600,032 known GPU buffer bytes and 15,200,000 known CPU
instance staging bytes. These staging capacities did not grow across the patch.

Patch publication took 5.507 ms, with 100,003 observed process allocation calls
and 5,295,252 cumulative allocated bytes. Immutable public `Vec<Primitive>`
snapshots still require an O(N) publication copy; partial GPU uploads do not erase
that CPU cost. The allocation observation excludes author change construction and
can include concurrent Rust driver housekeeping. Rendering the patch took
8.87–11.53 ms per surface in that sample, including surface acquisition and submit.
Worst-order native picking over 50,000 primitives had p95 0.148 ms and p99 0.205 ms
over 1,000 samples. That result does not justify adding a broad-phase index yet.

Twenty complete warm surface/window cycles passed, with the surface's window
reference released before native window destruction. Explicit idle eviction left
zero cached devices and zero cached atlas bytes. All four opt-in GPU library
checks passed, including bundled compiler pipeline creation, settled validation
error wake-up, explicit device destruction and failure wake-up for both surfaces
sharing the affected device. This injects failures into owned devices; it does
not simulate physical adapter removal or a driver reset.

The existing release acceptance fixture also passed its 300-frame native pan/zoom
workload with background replacements, visible-label workload, resize/minimize
and restore, moving-key interpolation settling, and twenty complete cold device
surface/window disposal cycles. It measured render-call p95 9.45 ms and p99
15.27 ms in that run. The revised motion assertion first verifies that unchanged
geometry settles, then patches a key's bounds and verifies real animation.

Reproduce using the commands in `renderer_acceptance.md`. Local raw reports are
`target/native-workflows-incremental-render.json` and
`target/native-workflows-full-render.json`; reports are generated artifacts.
Compositor synchronization is not a GPU timestamp or proof that every overlapping
surface was presented. Driver heaps, swapchains and pipeline sizes remain unknown
and separate from counted buffers/atlas. Mixed-DPI interaction, physical device
loss, cross-platform acceptance and Strata application performance remain open.
