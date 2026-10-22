<script lang="ts">
  import type { Readable } from '../../src/store.js';
  let { group }: { group: Readable<{ status: string; dataKey?: string; data?: { title: string } }> } = $props();
</script>
<p data-result>{$group.status}:{$group.dataKey ?? ''}:{$group.data?.title ?? ''}</p>
