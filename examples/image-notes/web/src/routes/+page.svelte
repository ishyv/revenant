<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { confirm } from "@revenant/client";
  import { useApp } from "$lib/revenant";
  import LibrarySidebar from "$lib/components/LibrarySidebar.svelte";
  import Gallery from "$lib/components/Gallery.svelte";
  import ImageInspector from "$lib/components/ImageInspector.svelte";
  import { libraryQuery } from "$lib/models/library-query";
  import { noteEditor } from "$lib/models/note-editor";
  import { scanModel } from "$lib/models/scan";
  import { coordinator } from "$lib/models/coordinator";
  import { createImageRenderer } from "$lib/image-queue";
  import {
    message,
    type Asset,
    type ImageRenderer,
    type Root,
    type ScanReport,
  } from "$lib/types";

  const app = useApp();
  const operations = app.operations.images;
  const library = libraryQuery(operations);
  const editor = noteEditor(operations.annotate, library.noteSaved);
  const scan = scanModel(operations.scan);
  const navigation = coordinator(editor);
  let render = $state<ImageRenderer>(createImageRenderer(operations.render));
  let search = $state("");
  let adding = $state(false);
  let error = $state("");
  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const currentRoot = $derived($library.roots.find((root) => root.id === $library.rootId));
  const displayedError = $derived(error || $editor.error || $library.error || $scan.error);

  onMount(() => {
    void library.start().catch(reportError);
  });

  function reportError(failure: unknown) {
    if (alive) error = message(failure);
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
        render = createImageRenderer(operations.render);
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
      await operations.removeRoot.call({ id: root.id });
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
