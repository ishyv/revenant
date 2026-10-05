<script lang="ts">
  import { onDestroy } from 'svelte';
  import { createApp, provideApp } from '$lib/revenant';
  import type { Snippet } from 'svelte';
  import '@hyvnt/hyvui/styles.css';
  import '@hyvnt/hyvui/fonts.css';
  import '@hyvnt/hyvui/themes.css';

  let { children }: { children: Snippet } = $props();
  const app = provideApp(createApp());
  onDestroy(() => app.dispose());
</script>

{@render children()}
