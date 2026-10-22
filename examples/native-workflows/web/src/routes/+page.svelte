<script lang="ts">
  import { onDestroy } from 'svelte';
  import { useApp } from '#lib/revenant.ts';
  import { OverlayCoordinator, callableOperation, queryGroup, type Handle } from '@revenant/client';
  import { CoveredModal, NativeCanvas } from '@revenant/client/ui';
  const owner = useApp();
  const api = owner.operations;
  const overlays = new OverlayCoordinator(owner.scope);
  const reads = queryGroup(owner.scope, { required: { summary: callableOperation(api.fixture.summary, true) }, optional: { status: callableOperation(api.fixture.optional, true) } });
  let scene = $state<Handle>();
  let controller = $state<Handle>();
  let entries = $state(500_000);
  let failOptional = $state(false);
  let first = $state(false);
  let second = $state(false);
  let error = $state('');
  let shift = 0;
  async function build() {
    try {
      const dataset = await api.fixture.build.call({ entries });
      await reads.load(`entries-${entries}`, { summary: { dataset }, status: { fail: failOptional } });
      if (!scene) { const handles = await api.fixture.scene.call({}); scene = handles.scene; controller = handles.controller; }
    } catch (failure) { error = String(failure); }
  }
  async function patch() {
    if (!controller) return;
    try { await api.fixture.patch.call({ controller, shift: ++shift % 20 }); } catch (failure) { error = String(failure); }
  }
  onDestroy(() => { void owner.dispose(); });
</script>
<svelte:head><title>Native workflows fixture</title></svelte:head>
<main>
  <h1>Native workflows</h1>
  <p>Deterministic native work, grouped reads and a keyed scene.</p>
  <label>Entries <select bind:value={entries}><option value={500_000}>500,000</option><option value={1_000_000}>1,000,000</option></select></label>
  <label><input type="checkbox" bind:checked={failOptional}/> Fail optional read</label>
  <button onclick={build}>Build fixture</button><button onclick={patch} disabled={!scene}>Patch 500 keys</button>
  <button onclick={() => first = true}>Open dialog</button>
  <p>Request {$reads.requestKey ?? 'none'} · data {$reads.dataKey ?? 'none'} · {$reads.status}</p>
  {#if $reads.data}<p>{$reads.data.summary.entries} entries · checksum {$reads.data.summary.checksum}</p>{/if}
  {#if $reads.errors.status}<p>Optional read: {$reads.errors.status.message}</p>{/if}
  {#if $reads.errors.summary}<p role="alert">Required read: {$reads.errors.summary.message}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  {#if scene}<div class="scene"><NativeCanvas scope={owner.scope} {scene} {overlays} labels={false} onerror={failure => error = String(failure)}/></div>{/if}
</main>
<CoveredModal {overlays} open={first} title="First cover" onclose={() => first = false}><p>The native scene hides before this dialog opens.</p><button onclick={() => second = true}>Open overlapping dialog</button><button onclick={() => first = false}>Close first</button></CoveredModal>
<CoveredModal {overlays} open={second} title="Second cover" onclose={() => second = false}><p>Closing one dialog keeps the scene hidden while another remains.</p><button onclick={() => second = false}>Close second</button></CoveredModal>
<style>main { padding: 24px; color: #e7edf3; background: #18212a; min-height: 100vh; } button, label { margin: 8px; } .scene { height: min(40vh, 300px); border: 1px solid #41596c; }</style>
