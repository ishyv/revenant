<script lang="ts">
  import { setupApp } from '#lib/revenant.ts';
  import type { Snippet } from 'svelte';
  import '#lib/styles.css';
  let { children }: { children: Snippet } = $props();
  setupApp();
</script>
{@render children()}
