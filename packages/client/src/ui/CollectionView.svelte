<script lang="ts">
  /** Virtualized native file viewport using the actual TanStack Svelte adapter. */
  import { createVirtualizer } from '@tanstack/svelte-virtual';
  import { Button, Checkbox, EmptyState, Text } from '@hyvnt/hyvui';
  import { get } from 'svelte/store';
  import type { Snippet } from 'svelte';
  import type { FileEntry, RuntimeError } from '../contracts.js';
  import { errorOf } from '../contracts.js';
  import type { FileCollection, FileSelection, SelectionStatus } from '../files.js';
  import ErrorNotice from './ErrorNotice.svelte';

  let { collection, selection, row, onactivate, height = '28rem', rowHeight = 48, label = 'Files' }: {
    /** Native indexed view; only bounded metadata windows enter the webview. */
    collection: FileCollection;
    /** Optional bounded explicit selection owned by the application scope. */
    selection?: FileSelection;
    /** Custom row content; fixed rowHeight remains the virtualization contract. */
    row?: Snippet<[FileEntry, number]>;
    /** Activate a loaded file, for example to create a native preview. */
    onactivate?: (entry: FileEntry) => void;
    /** CSS height of the scroll viewport. */
    height?: string;
    /** Fixed row height in pixels; values below 24 are clamped. */
    rowHeight?: number;
    /** Accessible region name. */
    label?: string;
  } = $props();

  let viewport = $state<HTMLDivElement>();
  let selected = $state<readonly FileEntry[]>([]);
  let requestError = $state<RuntimeError>();
  let selectionError = $state<RuntimeError>();
  let selectionStatus = $state<SelectionStatus>();
  // Virtualizers still allocate measurements for their count, and browser CSS
  // heights have finite limits. Keep both independent of native folder size.
  const segmentRows = 4096;
  let segmentStart = $state(0);
  let segmentCollection: FileCollection | undefined;
  let segmentGeneration = -1;
  const virtualizer = createVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: 0, getScrollElement: () => viewport ?? null,
    estimateSize: () => Math.max(24, rowHeight), overscan: 8,
  });
  let rows = $derived($virtualizer.getVirtualItems());
  let totalSize = $derived($virtualizer.getTotalSize());
  let requestedCollection: FileCollection | undefined;
  let requestedGeneration = -1;
  let requestedOffset = -1;
  let requestedEnd = -1;

  $effect(() => {
    const count = Math.min(segmentRows, Math.max(0, $collection.total - segmentStart));
    const element = viewport;
    const size = Math.max(24, rowHeight);
    // get() avoids tracking the adapter store that setOptions itself republishes.
    get(virtualizer).setOptions({ count, getScrollElement: () => element ?? null, estimateSize: () => size });
  });
  $effect(() => {
    if (segmentCollection !== collection || segmentGeneration !== $collection.generation) {
      segmentCollection = collection;
      segmentGeneration = $collection.generation;
      jump(0);
    } else if (segmentStart >= $collection.total && segmentStart > 0) {
      jump(Math.max(0, $collection.total - segmentRows));
    }
  });
  $effect(() => {
    selected = [];
    return selection?.subscribe((items) => { selected = items; });
  });
  $effect(() => {
    selectionStatus = undefined;
    return selection?.status.subscribe((status) => { selectionStatus = status; });
  });
  $effect(() => {
    const generation = $collection.generation;
    const first = rows[0] ? segmentStart + rows[0].index : undefined;
    const last = rows.at(-1) ? segmentStart + rows.at(-1)!.index : undefined;
    if (first === undefined || last === undefined || $collection.state === 'disposed' || requestError) return;
    if (requestedCollection === collection && requestedGeneration === generation &&
      first >= requestedOffset && last < requestedEnd) return;
    const offset = Math.max(0, first - 8);
    const limit = Math.min(512, Math.max(128, last - offset + 9));
    requestedCollection = collection;
    requestedGeneration = generation;
    requestedOffset = offset;
    requestedEnd = offset + limit;
    requestError = undefined;
    const active = collection;
    void active.window(offset, limit).catch((error) => {
      if (active === requestedCollection && generation === requestedGeneration && offset === requestedOffset) {
        requestError = errorOf(error);
        requestedOffset = -1;
        requestedEnd = -1;
      }
    });
  });
  function isSelected(entry: FileEntry): boolean {
    // A reappearing row may receive a fresh capability. Path is only a view-local UI key.
    return selected.some((item) => item.relativePath === entry.relativePath);
  }
  function jump(offset: number): void {
    segmentStart = Math.max(0, Math.min(Math.floor(offset), Math.max(0, $collection.total - segmentRows)));
    requestError = undefined;
    requestedOffset = -1;
    requestedEnd = -1;
    if (viewport) viewport.scrollTop = 0;
    get(virtualizer).scrollToOffset(0);
  }
  async function toggle(entry: FileEntry): Promise<void> {
    selectionError = undefined;
    try { await selection?.toggle(entry); }
    catch (error) { selectionError = errorOf(error); }
  }
</script>

<section aria-label={label}>
  <Text as="h3">{label}</Text>
  <p aria-live="polite">{$collection.total} matches · {$collection.discovered} discovered
    {#if $collection.state === 'loading'} · Scanning…{/if}</p>
  <ErrorNotice error={requestError ?? selectionError ?? selectionStatus?.error ?? $collection.error} />
  {#if requestError}<Button size="sm" onclick={() => { requestError = undefined; }}>Reload window</Button>{/if}
  {#if $collection.warningCount > 0}
    <p role="status">{$collection.warningCount} scan warnings. Some files could not be indexed.</p>
  {/if}
  {#if $collection.total === 0 && $collection.state === 'ready'}
    <EmptyState title="No matching files" description="Try another search or folder." />
  {/if}
  {#if $collection.total > segmentRows}
    <nav aria-label="Browse file ranges">
      <Button size="sm" disabled={segmentStart === 0} onclick={() => jump(0)}>First files</Button>
      <Button size="sm" disabled={segmentStart === 0} onclick={() => jump(segmentStart - segmentRows)}>Previous range</Button>
      <span>{segmentStart + 1}–{Math.min($collection.total, segmentStart + segmentRows)} of {$collection.total}</span>
      <Button size="sm" disabled={segmentStart + segmentRows >= $collection.total} onclick={() => jump(segmentStart + segmentRows)}>Next range</Button>
      <Button size="sm" disabled={segmentStart + segmentRows >= $collection.total} onclick={() => jump($collection.total - segmentRows)}>Last files</Button>
      <input aria-label="Browse position" type="range" min="0" max={Math.max(0, $collection.total - segmentRows)}
        value={segmentStart} onchange={(event) => jump(Number(event.currentTarget.value))} />
    </nav>
  {/if}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex (The scroll region must support keyboard scrolling.) -->
  <div class="viewport" bind:this={viewport} style:height aria-label={`${label} viewport`} role="region" tabindex="0">
    <div class="canvas" style:height={`${totalSize}px`}>
      {#each rows as virtualRow (virtualRow.key)}
        {@const entry = $collection.items[segmentStart + virtualRow.index - $collection.offset]}
        <div class="row" style:height={`${virtualRow.size}px`} style:transform={`translateY(${virtualRow.start}px)`}>
          {#if entry}
            {#if selection}
              <Checkbox aria-label={`Select ${entry.name}`} checked={isSelected(entry)}
                disabled={selected.length >= 512 && !isSelected(entry)}
                onchange={() => toggle(entry)} />
            {/if}
            {#if row}
              {@render row(entry, segmentStart + virtualRow.index)}
            {:else}
              <Button variant="ghost" onclick={() => onactivate?.(entry)} disabled={!onactivate}>{entry.name}</Button>
              <span class="path">{entry.relativePath}</span>
              <span class="size">{entry.size} B</span>
            {/if}
          {:else}<span aria-label="Loading file">Loading…</span>{/if}
        </div>
      {/each}
    </div>
  </div>
</section>

<style>
  .viewport { overflow: auto; border: 1px solid var(--line); border-radius: .25rem; contain: strict; }
  .canvas { position: relative; width: 100%; }
  .row { position: absolute; top: 0; left: 0; width: 100%; display: flex; align-items: center; gap: .75rem; padding: 0 .75rem; box-sizing: border-box; }
  .path { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-soft); }
  .size { white-space: nowrap; font-variant-numeric: tabular-nums; }
  p { color: var(--text-soft); font-size: .875rem; }
  nav { display: flex; align-items: center; gap: .5rem; flex-wrap: wrap; margin-block: .75rem; }
  nav input { flex: 1; min-width: 8rem; }
</style>
