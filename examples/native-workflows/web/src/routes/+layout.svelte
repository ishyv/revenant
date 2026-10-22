<script lang="ts">
  import { onDestroy, type Snippet } from 'svelte';
  import { createApp, provideApp } from '#lib/revenant.ts';
  import '@hyvnt/hyvui/styles.css';
  import '@hyvnt/hyvui/themes.css';
  let { children }: { children: Snippet } = $props();
  const app = provideApp(createApp());
  onDestroy(() => { void app.dispose(); });
</script>
{@render children()}
