<script lang="ts">
  import { setupApp } from '$lib/revenant';
  import type { Snippet } from 'svelte';
  import '@hyvnt/hyvui/styles.css';
  import '@hyvnt/hyvui/fonts.css';
  import '@hyvnt/hyvui/themes.css';

  let { children }: { children: Snippet } = $props();
  setupApp();
</script>

{@render children()}
