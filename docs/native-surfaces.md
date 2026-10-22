# Native 2D viewports

Enable `[desktop] native_surfaces = true` in `revenant.toml`. Add the public
`revenant-graphics` crate from the same SDK snapshot as `revenant-sdk`; ordinary
applications keep this capability disabled and do not acquire GPU dependencies.
The Windows backend uses wgpu 30.0.1 with DX12 and a bundled, verified Microsoft
DXC compiler. It does not download a compiler at runtime. Other desktop viewport
backends are deferred.

Rust owns a `Scene2D` and returns `context.own(scene)` as `Resource<Scene2D>` from
an ordinary operation. Replace its snapshot with keyed `Primitive` rectangles
or circles and labels. Replacement validates finite geometry, unique keys,
bounded strings, and at most 50,000 primitives before publishing a revision.
The scene stays native. Svelte receives an opaque handle and interaction events,
not an inventory of primitives.

```svelte
<script lang="ts">
  import { NativeCanvas } from '@revenant/client/ui';
  // app and scene are the generated application and operation result.
  let { app, scene } = $props();
  let canvas = $state();
  async function openDialog() {
    await canvas.suspend();
    // Mount the covering Svelte dialog only after the native hide acknowledgement.
  }
</script>
<div style="height: 480px">
  <NativeCanvas bind:this={canvas} scope={app.scope} {scene}
    oninteraction={(event) => console.log(event.kind, event.key)} />
</div>
```

After removing the dialog, call `canvas.restore()`. `canvas.fit()` resets native
pan and zoom. Motion, labels, and background are component properties; an author
should map the operating system's reduced-motion preference to `motion`.

The first backend supports rectangular viewports with untransformed ancestors
fully inside the WebView client area. CSS transforms, perspective, clipping
paths, rounded clipping and arbitrary Svelte overlays are not composited with
the native child window. Suspend the viewport for any covering modal, menu, or
other overlay. Placement uses CSS bounds and WebView device pixel ratio, so page
zoom and monitor DPI are distinct from scene coordinates. Resize and scroll
updates retain one pending placement while a request is active.

The child window handles pointer selection, double-click activation, wheel zoom,
middle/right-button captured panning, and arrow/plus/minus navigation. Authors
provide accessible tree/table equivalents. Hover, selection and activation carry
the primitive key, native scene revision and viewport-local pixel position.
Labels use a bundled open-licensed Latin font atlas; unsupported glyphs appear
as `?`, so provide full Unicode names in the surrounding interface.

The host validates window and scope authority, retains the scene lease during
attachment, and disposes the GPU surface before destroying its child window.
Unmounting, scope disposal, navigation and shutdown dispose attachments. Explicit
`app.releaseResource(handle)` revokes future resource admissions; active leases
remain valid until their users finish. Dispose creating tasks as well to release
their retained results.

Rendering stops after interpolation settles and while hidden or minimized.
Initialization and device failures produce explicit errors; keep ordinary
analysis controls available when `onerror` fires. The reported `bufferBytes` is
an estimate of framework-owned buffers and atlas storage, not total driver or
GPU-process memory. Validate frame timing and resource reclamation on the actual
application workload before making performance claims.
