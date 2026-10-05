<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { fromStore } from 'svelte/store';
  import { useApp } from '$lib/revenant';
  import ImageTile from '$lib/components/ImageTile.svelte';
  import { imageQueue } from '$lib/image-queue';
  import type { Asset, AssetPage, ImageRenderer, Root } from '$lib/types';

  const app = useApp();
  const operations = app.operations.images;
  const pageSize = 48;
  let roots = $state<Root[]>([]);
  let page = $state<AssetPage>({ items: [], total: 0, offset: 0 });
  let rootId = $state('');
  let search = $state('');
  let annotatedOnly = $state(false);
  let offset = $state(0);
  let ready = $state(false);
  let loading = $state(false);
  let adding = $state(false);
  let selected = $state<Asset>();
  let draft = $state('');
  let saveState = $state<'saved' | 'dirty' | 'saving' | 'failed'>('saved');
  let preview = $state('');
  let previewLoading = $state(false);
  let previewError = $state('');
  let error = $state('');
  let toast = $state('');
  let dataPath = $state('');
  let scanTask = $state<ReturnType<typeof operations.scan.run>>();
  const scanProjection = $derived(scanTask ? fromStore(scanTask) : undefined);
  const scanning = $derived(!!scanTask && !scanTask.terminal);
  const currentRoot = $derived(roots.find((root) => root.id === rootId));
  const dirty = $derived(!!selected && draft !== selected.note);
  const allCount = $derived(roots.reduce((sum, root) => sum + root.count, 0));
  let alive = true;
  let queryVersion = 0;
  let previewVersion = 0;
  let timer: ReturnType<typeof setTimeout>;
  let saveWork: Promise<boolean> | undefined;
  let render = $state<ImageRenderer>(imageQueue(async (id, large) => (await operations.render.call({ id, large })).dataUrl));

  function message(failure: unknown) { return failure instanceof Error ? failure.message : String(failure); }
  async function startup() {
    const state = await operations.bootstrap.call({});
    if (alive) { roots = state.roots; dataPath = state.dataPath; }
  }
  onMount(() => {
    void startup().then(() => { if (alive) { ready = true; void loadPage(); } })
      .catch((failure) => { if (alive) error = message(failure); });
  });
  async function loadPage() {
    const version = ++queryVersion;
    const options = { search, rootId, annotatedOnly, offset, limit: pageSize };
    loading = true;
    try {
      const next = await operations.query.call(options);
      if (alive && version === queryVersion) {
        if (next.total > 0 && next.offset >= next.total) { offset = 0; void loadPage(); return; }
        page = next;
      }
    } catch (failure) { if (alive && version === queryVersion) error = message(failure); }
    finally { if (alive && version === queryVersion) loading = false; }
  }
  function searchChanged(value: string) {
    search = value; offset = 0; clearTimeout(timer);
    // Invalidate an old response immediately, before the next request is admitted.
    queryVersion++;
    timer = setTimeout(() => { if (ready) void loadPage(); }, 140);
  }
  async function chooseRoot(id: string) {
    if (!(await saveNote())) return;
    rootId = id; offset = 0; clearTimeout(timer); void loadPage();
  }
  async function selectAsset(asset: Asset) {
    if (selected?.id === asset.id) return;
    if (!(await saveNote())) return;
    selected = asset; draft = asset.note; saveState = 'saved';
    preview = ''; previewError = ''; previewLoading = true;
    const version = ++previewVersion;
    try {
      const url = await render(asset.id, true);
      if (alive && version === previewVersion) preview = url;
    } catch (failure) { if (alive && version === previewVersion) previewError = message(failure); }
    finally { if (alive && version === previewVersion) previewLoading = false; }
  }
  function edit(value: string) { draft = value; saveState = 'dirty'; }
  function saveNote(): Promise<boolean> {
    if (saveWork) return saveWork.then((ok) => ok ? saveNote() : false);
    const asset = selected;
    if (!asset || draft === asset.note) return Promise.resolve(true);
    const value = draft;
    saveState = 'saving'; error = '';
    saveWork = operations.annotate.call({ id: asset.id, note: value }).then((saved) => {
      if (alive) {
        if (selected?.id === saved.id) {
          selected = { ...selected, note: saved.note };
          saveState = draft === saved.note ? 'saved' : 'dirty';
        }
        page = { ...page, items: page.items.map((item) => item.id === saved.id ? { ...item, note: saved.note } : item) };
      }
      return true;
    }).catch((failure) => {
      if (alive) { saveState = 'failed'; error = message(failure); }
      return false;
    }).finally(() => { saveWork = undefined; });
    return saveWork;
  }
  async function saveAndRefresh() { if (await saveNote()) void loadPage(); }
  async function openFolder() {
    adding = true; error = '';
    try {
      const folder = await app.files.openFolder();
      if (!folder) return;
      const path = folder.bookmark.path;
      await folder.dispose();
      await scanFolder(path);
    } catch (failure) { if (alive) error = message(failure); }
    finally { if (alive) adding = false; }
  }
  async function scanFolder(path: string) {
    if (!(await saveNote())) return;
    error = '';
    await scanTask?.dispose();
    const task = operations.scan.run({ path }); scanTask = task;
    try {
      const result = await task.result;
      if (alive) {
        toast = result.warnings ? `${result.discovered} imágenes · ${result.warnings} archivos no se pudieron leer` : `${result.discovered} imágenes listas`;
        // Fresh queue prevents stale in-memory renditions after a file changed.
        render = imageQueue(async (id, large) => (await operations.render.call({ id, large })).dataUrl);
        await startup(); rootId = result.rootId; offset = 0; await loadPage();
        if (selected) { const previous = selected; selected = undefined; await selectAsset(previous); }
      }
    } catch (failure) {
      if (alive) {
        error = message(failure); await startup().catch(() => {}); await loadPage();
      }
    }
  }
  async function removeRoot(root: Root) {
    if (!confirm(`¿Quitar «${root.name}» de Luma? Se eliminarán sus notas del catálogo. Las imágenes originales permanecen intactas.`)) return;
    if (!(await saveNote())) return;
    try {
      await operations.removeRoot.call({ id: root.id });
      if (selected?.rootId === root.id) { selected = undefined; previewVersion++; preview = ''; }
      rootId = ''; offset = 0; await startup(); await loadPage();
    } catch (failure) { error = message(failure); }
  }
  function navigate(delta: number) { offset = Math.max(0, offset + delta * pageSize); void loadPage(); }
  function shortcut(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') { event.preventDefault(); void saveAndRefresh(); }
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') { event.preventDefault(); document.getElementById('search')?.focus(); }
    if (event.key === 'Escape' && !dirty) { selected = undefined; previewVersion++; preview = ''; }
  }
  function beforeLeave(event: BeforeUnloadEvent) { if (dirty || saveState === 'saving') { event.preventDefault(); event.returnValue = ''; } }
  function bytes(value: string) { const size = Number(value); return size < 1048576 ? `${Math.max(1, Math.round(size / 1024))} KB` : `${(size / 1048576).toFixed(1)} MB`; }
  onDestroy(() => { alive = false; queryVersion++; previewVersion++; clearTimeout(timer); });
</script>

<svelte:head><title>Luma — Image Notes</title><meta name="description" content="Un lugar para tus imágenes y las ideas que no quieres perder." /></svelte:head>
<svelte:window onkeydown={shortcut} onbeforeunload={beforeLeave} />

<div class="app-shell">
  <aside class="sidebar" aria-label="Biblioteca">
    <div class="brand"><span class="brand-symbol" aria-hidden="true">◈</span><span>Luma<small>IMAGE NOTES</small></span></div>
    <p class="nav-label">TU BIBLIOTECA</p>
    <button class="nav-item" class:active={!rootId && !annotatedOnly} onclick={() => { annotatedOnly = false; void chooseRoot(''); }}>
      <span aria-hidden="true">▦</span> Todas las imágenes <span class="nav-count">{allCount}</span>
    </button>
    <button class="nav-item" class:active={annotatedOnly} onclick={() => { annotatedOnly = !annotatedOnly; offset = 0; void loadPage(); }}>
      <span aria-hidden="true">✎</span> Con anotaciones
    </button>
    <div class="folder-heading"><p class="nav-label">CARPETAS</p><button class="icon-button" aria-label="Añadir carpeta" disabled={adding || scanning} onclick={openFolder}>+</button></div>
    <div class="folder-list">
      {#each roots as root (root.id)}
        <button class="nav-item folder" class:active={rootId === root.id} title={root.path} onclick={() => { annotatedOnly = false; void chooseRoot(root.id); }}>
          <span aria-hidden="true">▱</span><span class="folder-name">{root.name}</span><span class="nav-count">{root.count}</span>
        </button>
      {/each}
      {#if !roots.length}<p class="sidebar-hint">Tus carpetas aparecerán aquí. Las imágenes se quedan donde están.</p>{/if}
    </div>
    <button class="add-folder" disabled={adding || scanning || !ready} onclick={openFolder}><span>+</span> {adding ? 'Abriendo…' : 'Añadir carpeta'}</button>
    <div class="sidebar-bottom"><span class="local-dot"></span> Solo en tu computadora <small title={dataPath}>Hecho con Revenant 0.3</small></div>
  </aside>

  <main class="workspace">
    <header class="toolbar">
      <div class="breadcrumb">Biblioteca <span>/</span> <strong>{annotatedOnly ? 'Con anotaciones' : currentRoot?.name ?? 'Todas las imágenes'}</strong></div>
      <label class="search-box" for="search"><span aria-hidden="true">⌕</span>
        <input id="search" type="search" placeholder="Buscar imágenes o notas…" value={search} oninput={(event) => searchChanged(event.currentTarget.value)} disabled={!ready} />
        <kbd>Ctrl K</kbd>
      </label>
    </header>
    <div class="workspace-body" class:has-inspector={!!selected}>
      <section class="gallery-panel" aria-label="Imágenes">
        <div class="collection-heading"><div><p class="eyebrow">UN POCO DE ORDEN, MUCHAS IDEAS</p><h1>{annotatedOnly ? 'Tus anotaciones' : currentRoot?.name ?? 'Tu colección'}</h1>
          <p class="collection-description">{search ? `Resultados para «${search}»` : 'Guarda lo que ves. Encuentra lo que recuerdas.'}</p></div>
          <div class="heading-actions">
            {#if currentRoot}<button class="quiet-button" disabled={scanning} onclick={() => scanFolder(currentRoot!.path)}>↻ Actualizar</button>{/if}
            <span class="result-count" aria-live="polite">{loading ? 'Buscando…' : `${page.total} imágenes`}</span>
          </div>
        </div>
        {#if error}<div class="notice error" role="alert"><span>{error}</span><button aria-label="Cerrar error" onclick={() => error = ''}>×</button></div>{/if}
        {#if toast && !scanning}<div class="notice" role="status"><span>✓ {toast}</span><button aria-label="Cerrar aviso" onclick={() => toast = ''}>×</button></div>{/if}
        {#if scanning}
          <div class="scan-notice" role="status"><span class="spinner"></span><div><strong>Preparando tu colección</strong><small>{scanProjection?.current.progress.message ?? 'Explorando la carpeta…'}</small></div><button class="quiet-button" onclick={() => scanTask?.cancel()}>Cancelar</button></div>
        {/if}
        {#if !ready && !error}<div class="empty"><span class="spinner"></span><h2>Abriendo tu biblioteca…</h2></div>
        {:else if !roots.length}
          <div class="empty welcome"><div class="empty-art" aria-hidden="true"><span>▧</span><span>▧</span><span>▧</span></div>
            <p class="eyebrow">TU ARCHIVO, CON CONTEXTO</p><h2>Cada imagen tiene una historia.</h2><p>Añade una carpeta, escribe lo que te inspira<br />y encuéntralo después con una búsqueda.</p>
            <button class="primary-button" disabled={adding || !ready} onclick={openFolder}>+ Añadir mi primera carpeta</button><small>JPEG · PNG · WebP · GIF · BMP</small>
          </div>
        {:else if !page.items.length && !loading && !scanning}
          <div class="empty"><span class="empty-search" aria-hidden="true">⌕</span><h2>{search ? 'Todavía no hay coincidencias' : 'Un espacio por descubrir'}</h2><p>{search ? 'Prueba otra palabra del nombre, la ruta o tus notas.' : 'Añade imágenes a tu carpeta y pulsa Actualizar.'}</p>
            {#if search}<button class="quiet-button" onclick={() => searchChanged('')}>Limpiar búsqueda</button>{/if}
          </div>
        {:else}
          <div class="gallery" class:pending={loading} aria-busy={loading}>
            {#each page.items as asset (asset.id)}<ImageTile {asset} selected={selected?.id === asset.id} {render} onselect={selectAsset} />{/each}
          </div>
        {/if}
        {#if page.total > pageSize}
          <nav class="pagination" aria-label="Páginas de imágenes"><button class="quiet-button" disabled={offset === 0 || loading} onclick={() => navigate(-1)}>← Anterior</button>
            <span>{offset + 1}–{Math.min(offset + pageSize, page.total)} de {page.total}</span><button class="quiet-button" disabled={offset + pageSize >= page.total || loading} onclick={() => navigate(1)}>Siguiente →</button>
          </nav>
        {/if}
        {#if currentRoot}<footer class="folder-footer"><span title={currentRoot.path}>{currentRoot.path}</span><button disabled={scanning} onclick={() => removeRoot(currentRoot!)}>Quitar de la biblioteca</button></footer>{/if}
      </section>
      {#if selected}
        <aside class="inspector" aria-label="Imagen y anotación">
          <div class="inspector-heading"><span>DETALLES</span><button class="icon-button" aria-label="Cerrar detalles" onclick={async () => { if (await saveNote()) { selected = undefined; previewVersion++; preview = ''; } }}>×</button></div>
          <div class="preview-stage">
            {#if preview}<img src={preview} alt={selected.name} />
            {:else if previewLoading}<span class="spinner"></span>
            {:else}<p>{previewError || 'Vista no disponible'}</p>{/if}
          </div>
          <div class="inspector-content"><h2>{selected.name}</h2><p class="file-info">{bytes(selected.size)} <span>·</span> {roots.find((root) => root.id === selected?.rootId)?.name}</p>
            <div class="note-heading"><label for="note">Tu anotación</label><span class="save-status" class:unsaved={dirty || saveState === 'failed'} aria-live="polite">{saveState === 'saving' ? 'Guardando…' : saveState === 'failed' ? 'No se pudo guardar' : dirty ? 'Sin guardar' : 'Guardado'}</span></div>
            <textarea id="note" placeholder="¿Qué quieres recordar de esta imagen?\nIdeas, detalles, referencias…" value={draft} oninput={(event) => edit(event.currentTarget.value)} onblur={() => void saveAndRefresh()}></textarea>
            <div class="note-actions"><span>Ctrl / ⌘ S para guardar</span><button class="primary-button small" disabled={!dirty || saveState === 'saving'} onclick={saveAndRefresh}>Guardar nota</button></div>
            <div class="detail-divider"></div><p class="nav-label">UBICACIÓN</p><p class="image-location">{selected.relativePath}</p><p class="inspector-tip">Las notas se guardan en tu biblioteca local. Tus imágenes originales permanecen intactas.</p>
          </div>
        </aside>
      {/if}
    </div>
  </main>
</div>
