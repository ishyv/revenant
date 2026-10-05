<script lang="ts">
  import { onDestroy } from 'svelte';
  import { createApp, provideApp } from '$lib/revenant';
  import type { Snippet } from 'svelte';
  import '$lib/styles.css';
  let { children }: { children: Snippet } = $props();
  const app = provideApp(createApp());
  onDestroy(() => app.dispose());
</script>
{@render children()}
