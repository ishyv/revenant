<script lang="ts">
  import { setupApp } from '$lib/revenant';
  import type { Snippet } from 'svelte';
  import '$lib/styles.css';
  let { children }: { children: Snippet } = $props();
  setupApp();
</script>
{@render children()}
