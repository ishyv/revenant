The opt-in renderer test creates its own visible top-level Windows window and a
child viewport, then uses the public `Renderer` and `Scene2D` APIs. Ordinary test
runs skip it. It never captures or operates other applications.

From the Revenant checkout on an interactive Windows desktop:

```powershell
$env:REVENANT_RENDER_REPORT = '<strata-checkout>\engine\measurements\renderer-dx12-50k-background.json'
cargo test -p revenant-graphics --features gpu --test renderer_acceptance --release -- --ignored --nocapture
```

Use a writable report path of your choice. Without the environment variable, the
test prints JSON. `REVENANT_RENDER_BACKGROUND_UPDATES=0` runs a synthetic baseline
that constructs and replaces scenes on the render thread. The default prepares
and publishes scenes on a native worker at most ten times per second, matching
the intended application topology. Both retain upload/update frames in timings.

The workload contains 25,000 rectangles and 25,000 circles in a deterministic
grid. It measures 300 pan frames with periodic zoom and live scene revisions.
Tiny grid cells suppress labels; a separate 50-rectangle scene makes labels
visible and exercises the glyph atlas. Tests also exercise picking, physical
resize, minimize/restore, motion settling, and twenty complete GPU surface/window
creation and disposal cycles. Every cycle verifies that dropping `Renderer`
releases the surface's owned window reference before `DestroyWindow` runs. All
window operations and final disposal stay on the owning test thread.

The camera regression also starts a renderer at 1×1, resizes it to a short
784×279 viewport, and publishes a scene with a negative origin. It checks complete
framing and picking before any manual Fit, then verifies that pan/zoom preserve
the camera across updates and that Fit restores automatic framing on resize.
Run just this check with
`cargo test -p revenant-graphics --features gpu --test renderer_acceptance dx12_initial_scene_and_resize_fit_until_camera_interaction -- --ignored --nocapture`.
The 50,000-primitive harness also checks every primitive's extents after its first
render, before its first explicit Fit.

Private owned-device tests cover a settled renderer receiving an uncaptured GPU
validation error and a separately destroyed test device. They use hidden windows,
nonblocking device polling, and a two-second callback deadline, then require an
explicit `native_gpu` render error. Run them with
`cargo test -p revenant-graphics --features gpu --lib device_failure_tests -- --ignored --nocapture --test-threads=1`.

The native workflows harness adds four independent surfaces sharing an explicit
`GraphicsService`, a settled 500-key color patch in 50,000 primitives, per-surface
instance upload counters, retained CPU staging capacity, observed process allocation
counts during publication, worst-order picking timings, and twenty warm disposal
cycles followed by explicit idle eviction. `REVENANT_INCREMENTAL_RENDER_REPORT`
selects its JSON output. Run it in release mode with:

```powershell
$env:REVENANT_INCREMENTAL_RENDER_REPORT = '<revenant-checkout>\target\native-workflows-incremental-render.json'
cargo test -p revenant-graphics --features gpu --test renderer_acceptance --release dx12_incremental_shared_assets_and_warm_eviction_cycles -- --ignored --nocapture --test-threads=1
```

It disables labels to isolate stable geometry uploads and records compiled source
hashes. Patch publication currently clones immutable `Vec<Primitive>` snapshots;
the report exposes that remaining allocation cost. Process allocation observations
can include concurrent Rust driver housekeeping. Known surface buffer bytes and
shared atlas bytes are separate; driver heaps, swapchains and pipeline allocation
sizes remain unknown. Compositor synchronization does not prove presentation of
each overlapping child surface or mixed-DPI interaction acceptance.
These controlled tests verify callback delivery and failure reporting; they do
not simulate a physical driver reset or establish physical device-loss recovery.

The first real run caught a WGSL reserved identifier in the actual Scene2D shader
(`target`, subsequently renamed to `destination`). A compute-only compiler test
had not detected this pipeline initialization failure. The corrected real
renderer test passes on the verified Ryzen 7 5800X / Radeon RX 7800 XT host.

Measured release results from the October 8, 2026 background-publisher run:

| Measurement | Result |
| --- | --- |
| Full loop, including message pump and compositor synchronization | p95 13.992 ms; p99 14.212 ms |
| Frames exceeding 16.667 ms | 1 of 300; maximum 20.852 ms |
| `Renderer::render` wall time | p95 9.170 ms; p99 11.839 ms |
| Background scene construction, validation and replacement | p95 8.618 ms |
| `DwmFlush` wall time | p95 6.849 ms |
| Paced loop average | 133.82 frames/second |
| Label-visible 50-rectangle render call | p95 0.385 ms |
| Renderer-reported instance, atlas and uniform storage | 6,648,608 bytes |

These values describe one short measured workload. They support most frames
meeting a 60 Hz interaction budget here; the outlier means this is not a promise
of uninterrupted 60 Hz. The render-thread construction baseline produced a full
loop p95 of 27.955 ms, with 23 of 300 frames exceeding that budget. Preparing
scenes away from the render thread matters.

A repeated background run with handle accounting recorded p95 13.967 ms and
p99 20.805 ms for the full loop; 5 of 300 frames exceeded 16.667 ms. Its average
was 132.20 frames/second. Keep this variability visible: the shorter p99 in the
earlier run is not a guaranteed tail-latency bound. Raw results are preserved as
`renderer-dx12-50k-background.json` and `renderer-dx12-50k-handles.json` in Strata's
engine measurements directory.

`Renderer::render` currently combines scene upload, surface acquisition, CPU
command submission and presentation. The public API cannot isolate CPU
submission or present latency, so the report labels this combined wall time.
`DwmFlush` measures compositor synchronization, not GPU execution or proof of an
on-screen present. FIFO plus explicit compositor synchronization pace this test;
its average rate is not uncapped renderer throughput. No GPU timestamps are
collected. The storage estimate excludes driver allocations, swapchains and
pipeline heaps; it is not measured total GPU allocation.

Across repeated runs, working set grows during the first four disposal cycles,
then stays near 322–325 MB, with a transient later sample around 330 MB. Private
commit also fluctuates near 315–324 MB after warm-up. The background run ends at
316 MB working set after dropping the scene. HWND reference and destruction
checks pass each cycle. The initial growth is reproducible; its allocator/driver
attribution is unproven. A plateau over twenty cycles does not prove leak
freedom. JSON contains per-cycle samples for reviewing trends; later harness
versions also record process handles and GDI/user object counts.

The handle-accounting run records exactly 258 process handles, 9 GDI objects and
6 user objects after every one of its twenty cycles. Those counts rose from a
fresh-process baseline of 136/0/3 during initial GPU initialization, then remained
constant across disposal cycles. This narrows the evidence: retained process
memory is not accompanied by continuing growth in those native handle counts.
It still does not identify which allocator or driver cache retains the bytes.

Mixed-DPI monitor movement, pointer capture, covering Svelte dialogs, keyboard
focus/navigation, and forced device loss remain separate embedded-application
acceptance checks. Minimize/restore here verifies recovery; host scheduling while
hidden or minimized is outside this public-renderer harness. No claim is made
about dense 20,000-glyph rendering or the full WebView application footprint.
