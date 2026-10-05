<script lang="ts">
  import { onDestroy } from 'svelte';
  import { fromStore } from 'svelte/store';
  import { useApp } from '$lib/revenant';
  import type { Application } from '$lib/revenant';
  import { CollectionView, FilePreview, TaskStatus } from '@revenant/client/ui';
  import type { BatchResult, FileCollection, FileEntry, FileSelection, Folder, Preview, Readable, ResultPage, TaskSnapshot } from '@revenant/client';

  const app = useApp().createScope();
  const preferences = app.settings.define({ search: '', sort: 'name', recursive: true }, { key: 'file-media.preferences' });
  const persistence = preferences.status;
  type BatchTask = ReturnType<typeof app.files.checksum.runSelection> | ReturnType<typeof app.media.readMetadata.runSelection>;
  let folder = $state<Folder>();
  let library = $state<FileCollection>();
  let selection = $state<FileSelection>();
  let preview = $state<Preview>();
  let activeFile = $state<FileEntry>();
  let task = $state<BatchTask>();
  let batch = $state<BatchResult<unknown>>();
  let page = $state<ResultPage<unknown>>();
  let error = $state('');
  let opening = $state(false);
  let previewVersion = 0;
  let alive = true;
  let folderApp: Application | undefined;
  const taskProjection = $derived(task ? fromStore(task as Readable<TaskSnapshot<unknown>>) : undefined);
  const selectionProjection = $derived(selection ? fromStore(selection) : undefined);
  const taskActive = $derived(!!(taskProjection?.current && task && !task.terminal));
  const selectedCount = $derived(selectionProjection?.current.length ?? 0);

  // These options apply to the complete native index, never just the JS viewport.
  $effect(() => {
    const current = library;
    const options = { search: $preferences.search, recursive: $preferences.recursive,
      sort: $preferences.sort as 'name' | 'relativePath' | 'size' | 'modified' };
    if (current) void current.query(options).catch(report);
  });
  function report(failure: unknown) {
    if (alive) error = failure instanceof Error ? failure.message : String(failure);
  }
  async function openFolder() {
    opening = true; error = '';
    const candidate = app.createScope();
    try {
      const next = await candidate.files.openFolder();
      if (!next || !alive) { candidate.dispose(); return; }
      previewVersion++;
      await preview?.dispose(); preview = undefined; activeFile = undefined;
      await task?.dispose(); task = undefined; batch = undefined; page = undefined;
      selection?.dispose(); await library?.dispose();
      // Folder tokens belong to their native scope. Replacing that scope
      // releases the previous folder as well as its views and file leases.
      folderApp?.dispose(); folderApp = candidate;
      folder = next;
      library = next.files({ recursive: preferences.value.recursive });
      selection = library.selection();
    } catch (failure) { if (folderApp !== candidate) candidate.dispose(); report(failure); }
    finally { if (alive) opening = false; }
  }
  async function show(file: FileEntry) {
    const version = ++previewVersion;
    activeFile = file; error = '';
    await preview?.dispose(); preview = undefined;
    try {
      if (!folderApp) return;
      const next = await folderApp.media.preview(file);
      if (!alive || version !== previewVersion) await next.dispose(); else preview = next;
    } catch (failure) { if (version === previewVersion) report(failure); }
  }
  async function processFiles(kind: 'metadata' | 'checksum', all: boolean) {
    error = '';
    const target = all ? selection?.allMatching() : selection;
    if (!target) return;
    try {
      await task?.dispose(); batch = undefined; page = undefined;
      // The chosen folder and its operation share the same native owner scope.
      if (!folderApp) return;
      const next = kind === 'metadata' ? target.run(folderApp.media.readMetadata) : target.run(folderApp.files.checksum);
      task = next;
      try {
        const result = await next.result;
        const resultPage = await result.page(0, 64);
        if (alive && task === next) { batch = result; page = resultPage; }
      } catch (failure) {
        // Native cancellation and partial failure retain bounded outcome pages.
        if (alive && task === next) {
          report(failure);
          const resultPage = await next.results(0, 64);
          if (alive && task === next) page = resultPage;
        }
      }
    } catch (failure) { report(failure); }
  }
  async function resultWindow(offset: number) {
    const current = task;
    if (!current) return;
    try {
      const resultPage = batch ? await batch.page(offset, 64) : await current.results(offset, 64);
      if (alive && task === current) page = resultPage;
    }
    catch (failure) { report(failure); }
  }
  function size(value: string) {
    const bytes = BigInt(value);
    return bytes < 1024n ? `${value} B` : bytes < 1048576n ? `${bytes / 1024n} KiB` : `${bytes / 1048576n} MiB`;
  }
  onDestroy(() => { alive = false; previewVersion++; app.dispose(); });
</script>

<svelte:head><title>File & media — Revenant desktop</title></svelte:head>
<main>
  <header><p class="eyebrow">REVENANT / DESKTOP LIBRARY</p><h1>Your files,<br />at native scale.</h1>
    <p>Browse an indexed folder, preview a file, or run native operations across every match.</p>
    <button disabled={opening || taskActive} onclick={openFolder}>{opening ? 'Opening…' : 'Open folder'}</button>
    <a href="/records">Ordinary records →</a>
  </header>
  <section class="controls" aria-label="Native query">
    <label>Search <input value={$preferences.search} oninput={(event) => preferences.update((value) => ({ ...value, search: event.currentTarget.value }))} /></label>
    <label>Sort <select value={$preferences.sort} onchange={(event) => preferences.update((value) => ({ ...value, sort: event.currentTarget.value }))}>
      <option value="name">Name</option><option value="relativePath">Path</option><option value="size">Size</option><option value="modified">Modified</option>
    </select></label>
    <label class="check"><input type="checkbox" checked={$preferences.recursive} onchange={(event) => preferences.update((value) => ({ ...value, recursive: event.currentTarget.checked }))} /> Include subfolders</label>
  </section>
  {#if library && selection}
    <h2>{folder?.name}</h2>
    <div class="library">
      <CollectionView collection={library} {selection} rowHeight={76}>
        {#snippet row(file: FileEntry)}
          <button class="file" onclick={() => show(file)}><span>{file.relativePath}</span><small>{size(file.size)} / {file.mime}</small></button>
        {/snippet}
      </CollectionView>
      <FilePreview {preview} name={activeFile?.name ?? 'Selected file'} />
    </div>
    <div class="actions">
      <button disabled={!selectedCount || taskActive} onclick={() => processFiles('metadata', false)}>Metadata for selected</button>
      <button disabled={!selectedCount || taskActive} onclick={() => processFiles('checksum', false)}>Checksum selected</button>
      <button disabled={taskActive} onclick={() => processFiles('checksum', true)}>Checksum all matching</button>
    </div>
  {:else}<p>Choose a folder to begin. Large scans and batch selections remain native.</p>{/if}
  {#if task}<TaskStatus {task} label="File operation" />{/if}
  {#if page}
    <section aria-label="Paged results">
      <h2>Results: {page.total.toLocaleString()} items</h2>
      <p>Succeeded {page.counts.succeeded} / failed {page.counts.failed}</p>
      {#each page.items as outcome, index (page.offset + index)}<pre>{JSON.stringify(outcome, null, 2)}</pre>{/each}
      <div class="actions">
        <button disabled={page.offset === 0} onclick={() => resultWindow(Math.max(0, (page?.offset ?? 0) - 64))}>Previous results</button>
        <button disabled={page.offset + page.items.length >= page.total} onclick={() => resultWindow((page?.offset ?? 0) + 64)}>Next results</button>
      </div>
    </section>
  {/if}
  <p class="muted">Preferences: {$persistence.state}</p>
  {#if $persistence.error}<p role="alert">{$persistence.error.message}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
</main>
<style>
  :global(body){margin:0;background:#14181e;color:#edf1f6;font-family:system-ui,sans-serif}main{max-width:1200px;margin:auto;padding:48px 28px}
  h1{font-size:clamp(40px,6vw,68px);line-height:1.05;font-weight:500;margin:20px 0}p{line-height:1.7}.eyebrow,.muted{font-size:12px;color:#9db5c4}.eyebrow{letter-spacing:.16em}
  .controls,.actions{display:flex;gap:16px;flex-wrap:wrap;margin:24px 0}.controls{border-top:1px solid #35424e;padding-top:24px}label{display:flex;flex-direction:column;gap:8px}.check{flex-direction:row;align-items:center}
  input,select,button{font:inherit;border:1px solid #546876;background:#202a33;color:inherit;padding:10px;border-radius:4px}button{cursor:pointer}button:disabled{opacity:.5;cursor:default}
  a{color:#a6d6ec;margin-left:20px}.library{display:grid;grid-template-columns:1fr 1fr;gap:24px}.file{width:100%;text-align:left;background:transparent;border:0;padding:12px}.file span,.file small{display:block;overflow-wrap:anywhere}.file small{color:#9db5c4;margin-top:6px}
  pre{font-size:12px;max-height:260px;overflow:auto;background:#202a33;padding:16px}:global(:focus-visible){outline:2px solid #a6d6ec;outline-offset:3px}@media(max-width:760px){.library{grid-template-columns:1fr}}
</style>
