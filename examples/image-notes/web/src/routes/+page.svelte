<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { confirm, askSaveChanges, onDesktopCloseRequested } from "@revenant/client";
  import type { Unsubscribe } from "@revenant/client";
  import { useApp, useOperations } from "#lib/revenant.ts";
  import LibrarySidebar from "#lib/components/LibrarySidebar.svelte";
  import Gallery from "#lib/components/Gallery.svelte";
  import ImageInspector from "#lib/components/ImageInspector.svelte";
  import { libraryQuery } from "#lib/models/library-query.ts";
  import { noteEditor } from "#lib/models/note-editor.ts";
  import { noteCloseGuard } from "#lib/models/note-close.ts";
  import { scanModel } from "#lib/models/scan.ts";
  import { coordinator } from "#lib/models/coordinator.ts";
  import { createImageRenderer } from "#lib/image-queue.ts";
  import {
    message,
    type Asset,
    type ImageRenderer,
    type Root,
    type ScanReport,
    type CacheUsage,
  } from "#lib/types.ts";

  const app = useApp();
  const operations = useOperations(app).images;
  const library = libraryQuery(operations);
  const editor = noteEditor(operations.annotate, library.noteSaved);
  const scan = scanModel(operations.scan);
  const navigation = coordinator(editor);
  let render = $state<ImageRenderer>(createImageRenderer(operations.render, reportError));
  let search = $state("");
  let adding = $state(false);
  let error = $state("");
  let cacheUsage = $state<CacheUsage>();
  let clearingCache = $state(false);
  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopClose: Unsubscribe | undefined;
  const closeGuard = noteCloseGuard(editor, () => askSaveChanges(
    "¿Guardar los cambios de tu anotación antes de cerrar Luma?",
    { save: "Guardar", discard: "Descartar", cancel: "Cancelar" },
  ));
  const currentRoot = $derived($library.roots.find((root) => root.id === $library.rootId));
  const displayedError = $derived(error || $editor.error || $library.error || $scan.error);

  onMount(() => {
    void library.start().catch(reportError);
    void loadCacheUsage();
    void onDesktopCloseRequested(closeGuard.decide, {
      canClose: closeGuard.canClose,
      onError: reportError,
    }).then((stop) => {
      if (alive) stopClose = stop;
      else stop();
    }).catch(reportError);
  });

  function reportError(failure: unknown) {
    if (alive) error = message(failure);
  }

  async function loadCacheUsage() {
    try {
      const usage = await operations.cacheUsage({});
      if (alive) cacheUsage = usage;
    } catch (failure) { reportError(failure); }
  }

  async function clearCache() {
    if (clearingCache) return;
    clearingCache = true;
    try {
      if (!await confirm("¿Limpiar las imágenes en caché? Se volverán a generar cuando las abras. Tus notas y originales se conservan.")) return;
      const result = await operations.clearCache({});
      if (!alive) return;
      cacheUsage = result.usage;
      render = createImageRenderer(operations.render, reportError);
    } catch (failure) { reportError(failure); }
    finally { if (alive) clearingCache = false; }
  }

  function cacheMiB(bytes: string) {
    return (Number(bytes) / 1048576).toFixed(1);
  }

  async function act(
    action: () => void | Promise<void>,
    confirm?: () => boolean | Promise<boolean>,
  ) {
    try {
      return await navigation.run(action, confirm);
    } catch (failure) {
      reportError(failure);
      return false;
    }
  }

  function dismissError() {
    error = "";
    editor.dismissError();
    library.dismissError();
    scan.dismissError();
  }

  function searchChanged(value: string) {
    search = value;
    clearTimeout(timer);
    // Suppress an admitted old read now, before the debounce/save barrier.
    library.invalidate();
    timer = setTimeout(() => {
      void act(async () => {
        if (value === search) await library.search(value);
      });
    }, 140);
  }

  function chooseRoot(id: string, annotatedOnly = $library.annotatedOnly) {
    clearTimeout(timer);
    library.invalidate();
    void act(() => library.chooseRoot(id, annotatedOnly, search));
  }

  function toggleAnnotations() {
    clearTimeout(timer);
    library.invalidate();
    void act(() => library.chooseRoot($library.rootId, !$library.annotatedOnly, search));
  }

  function selectAsset(asset: Asset) {
    if ($editor.asset?.id === asset.id) return;
    void act(() => {
      // A queued selection may have been annotated by an earlier save.
      const current = $library.page.items.find((item) => item.id === asset.id);
      editor.select(current ?? asset);
    });
  }

  function closeInspector() {
    void act(() => editor.select());
  }

  function saveAndRefresh() {
    clearTimeout(timer);
    void act(() => library.reload(search));
  }

  async function openFolder() {
    if (adding || $scan.scanning) return;
    adding = true;
    error = "";
    try {
      const folder = await app.files.openFolder();
      if (!folder) return;
      let path: string;
      try {
        path = folder.bookmark.path;
      } finally {
        await folder.dispose();
      }
      if (alive) await scanFolder(path);
    } catch (failure) {
      reportError(failure);
    } finally {
      if (alive) adding = false;
    }
  }

  async function scanFolder(path: string) {
    if ($scan.scanning) return;
    let running: Promise<ScanReport | undefined> | undefined;
    const started = await act(() => {
      if ($scan.scanning) return;
      error = "";
      library.invalidate();
      running = scan.run(path);
      // Observe immediately; task admission can reject before act resolves.
      void running.catch(() => {});
    });
    if (!started || !running) return;
    try {
      const result = await running;
      if (!alive || !result) return;
      // Do not hold navigation behind a long scan. Flush notes again at
      // completion, since users can keep writing while indexing runs.
      await act(async () => {
        render = createImageRenderer(operations.render, reportError);
        await library.refreshRoots();
        await library.chooseRoot(result.rootId, $library.annotatedOnly, search);
      });
    } catch (failure) {
      reportError(failure);
      if (alive) {
        await act(async () => {
          await library.refreshRoots();
          await library.reload(search);
        });
      }
    }
  }

  async function removeRoot(root: Root) {
    await act(async () => {
      await operations.removeRoot({ id: root.id });
      if (!alive) return;
      if ($editor.asset?.rootId === root.id) editor.select();
      await library.refreshRoots();
      await library.chooseRoot("", $library.annotatedOnly, search);
    }, () => confirm(
      `¿Quitar «${root.name}» de Luma? Se eliminarán sus notas del catálogo. Las imágenes originales permanecen intactas.`,
    ));
  }

  function navigate(delta: number) {
    clearTimeout(timer);
    library.invalidate();
    void act(() => library.navigate(delta, search));
  }

  function shortcut(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      saveAndRefresh();
    }
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      document.getElementById("search")?.focus();
    }
    if (event.key === "Escape" && !$editor.dirty) closeInspector();
  }

  function beforeLeave(event: BeforeUnloadEvent) {
    if ($editor.dirty || $editor.status === "saving") {
      event.preventDefault();
      event.returnValue = "";
    }
  }

  onDestroy(() => {
    alive = false;
    stopClose?.();
    clearTimeout(timer);
    navigation.dispose();
    editor.dispose();
    void library.dispose().catch(() => {});
    void scan.dispose().catch(() => {});
  });
</script>

<svelte:head>
  <title>Luma — Image Notes</title>
  <meta
    name="description"
    content="Un lugar para tus imágenes y las ideas que no quieres perder."
  />
</svelte:head>
<svelte:window onkeydown={shortcut} onbeforeunload={beforeLeave} />

<div class="app-shell">
  <LibrarySidebar
    roots={$library.roots}
    rootId={$library.rootId}
    annotatedOnly={$library.annotatedOnly}
    {adding}
    scanning={$scan.scanning}
    ready={$library.ready}
    dataPath={$library.dataPath}
    onchoose={chooseRoot}
    onannotations={toggleAnnotations}
    onopen={() => void openFolder()}
  />
  <main class="workspace">
    <header class="toolbar">
      <div class="breadcrumb">
        Biblioteca <span>/</span>
        <strong>
          {$library.annotatedOnly
            ? "Con anotaciones"
            : (currentRoot?.name ?? "Todas las imágenes")}
        </strong>
      </div>
      <label class="search-box" for="search">
        <span aria-hidden="true">⌕</span>
        <input
          id="search"
          type="search"
          placeholder="Buscar imágenes o notas…"
          value={search}
          oninput={(event) => searchChanged(event.currentTarget.value)}
          disabled={!$library.ready}
        />
        <kbd>Ctrl K</kbd>
      </label>
      <div class="cache-actions">
        <button class="subtle-button" title="Actualizar uso de caché" onclick={() => void loadCacheUsage()}>
          Caché {cacheUsage ? `${cacheMiB(cacheUsage.bytes)} / ${cacheMiB(cacheUsage.budgetBytes)} MiB` : "…"}
        </button>
        <button class="subtle-button" disabled={clearingCache || !$library.ready} onclick={() => void clearCache()}>
          {clearingCache ? "Limpiando…" : "Limpiar caché"}
        </button>
      </div>
    </header>
    <div class="workspace-body" class:has-inspector={!!$editor.asset}>
      <Gallery
        library={$library}
        scan={$scan}
        {currentRoot}
        {search}
        selectedId={$editor.asset?.id}
        {adding}
        error={displayedError}
        {render}
        onselect={selectAsset}
        onopen={() => void openFolder()}
        onscan={(path) => void scanFolder(path)}
        onremove={(root) => void removeRoot(root)}
        onnavigate={navigate}
        onsearch={searchChanged}
        oncancel={() => void scan.cancel()}
        ondismissError={dismissError}
        ondismissToast={scan.dismissToast}
      />
      {#if $editor.asset}
        <ImageInspector
          asset={$editor.asset}
          {editor}
          roots={$library.roots}
          {render}
          onclose={closeInspector}
          onsave={saveAndRefresh}
        />
      {/if}
    </div>
  </main>
</div>
