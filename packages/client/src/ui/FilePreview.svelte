<script lang="ts">
  /** Render scoped native previews; the application scope owns disposal. */
  import { EmptyState, Text } from '@hyvnt/hyvui';
  import type { Preview } from '../media.js';
  let { preview, name }: {
    /** Native preview URL is used without client-side reconstruction. */
    preview?: Preview;
    /** Accessible file name. */
    name?: string;
  } = $props();
  let displayName = $derived(name ?? preview?.name ?? 'Selected file');
</script>

<section aria-label={`${displayName} preview`}>
  {#if preview && !preview.disposed}
    <Text as="h3">{displayName}</Text>
    {#if preview.kind === 'text'}
      <pre>{preview.text}</pre>
      {#if preview.truncated}<p>Showing the beginning of this file.</p>{/if}
    {:else if preview.kind === 'image'}<img src={preview.url} alt={displayName} />
    {:else if preview.kind === 'audio'}<audio controls src={preview.url}><track kind="captions" /></audio>
    {:else if preview.kind === 'video'}<video controls src={preview.url}><track kind="captions" /></video>
    {:else}<EmptyState title="Preview unavailable" description="Other file operations remain available." />{/if}
  {:else}<EmptyState title="Choose a file" description="Select a file to preview its content." />{/if}
</section>

<style>
  section { padding: 1rem; }
  img, video { max-width: 100%; max-height: 30rem; object-fit: contain; }
  audio { width: 100%; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 30rem; overflow: auto; }
  p { color: var(--text-soft); font-size: .875rem; }
</style>
