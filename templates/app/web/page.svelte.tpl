<script lang="ts">
  import { useApp } from '$lib/revenant';
  const app = useApp();
</script>

<svelte:head><title>{{project_name}}</title></svelte:head>
<main>
  <p class="eyebrow">REVENANT / DESKTOP</p>
  <h1>{{project_name}}</h1>
  <p>Your app has a native home. Build your interface here.</p>
  {#await app.ready}
    <p role="status">Connecting to native capabilities…</p>
  {:then}
    <p role="status">Native capabilities ready.</p>
  {:catch error}
    <p role="alert">{error instanceof Error ? error.message : String(error)}</p>
  {/await}
</main>

<style>
  :global(body) { margin: 0; background: #14181e; color: #edf1f6; font-family: system-ui, sans-serif; }
  main { max-width: 760px; margin: auto; padding: 12vh 32px; }
  .eyebrow { font-size: 12px; letter-spacing: .18em; color: #94b6cc; }
  h1 { font-size: clamp(40px, 7vw, 76px); font-weight: 500; }
  p { line-height: 1.7; }
</style>
