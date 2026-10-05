<script lang="ts">
  import ImageTile from "./ImageTile.svelte";
  import { pageSize, type LibrarySnapshot } from "../models/library-query";
  import type { ScanSnapshot } from "../models/scan";
  import type { Asset, ImageRenderer, Root } from "../types";

  let {
    library,
    scan,
    currentRoot,
    search,
    selectedId,
    adding,
    error,
    render,
    onselect,
    onopen,
    onscan,
    onremove,
    onnavigate,
    onsearch,
    oncancel,
    ondismissError,
    ondismissToast,
  }: {
    library: LibrarySnapshot;
    scan: ScanSnapshot;
    currentRoot?: Root;
    search: string;
    selectedId?: string;
    adding: boolean;
    error: string;
    render: ImageRenderer;
    onselect: (asset: Asset) => void;
    onopen: () => void;
    onscan: (path: string) => void;
    onremove: (root: Root) => void;
    onnavigate: (delta: number) => void;
    onsearch: (value: string) => void;
    oncancel: () => void;
    ondismissError: () => void;
    ondismissToast: () => void;
  } = $props();
</script>

<section class="gallery-panel" aria-label="Imágenes">
  <div class="collection-heading">
    <div>
      <p class="eyebrow">UN POCO DE ORDEN, MUCHAS IDEAS</p>
      <h1>
        {library.annotatedOnly
          ? "Tus anotaciones"
          : (currentRoot?.name ?? "Tu colección")}
      </h1>
      <p class="collection-description">
        {search
          ? `Resultados para «${search}»`
          : "Guarda lo que ves. Encuentra lo que recuerdas."}
      </p>
    </div>
    <div class="heading-actions">
      {#if currentRoot}
        <button
          class="quiet-button"
          disabled={scan.scanning}
          onclick={() => onscan(currentRoot!.path)}
        >
          ↻ Actualizar
        </button>
      {/if}
      <span class="result-count" aria-live="polite">
        {library.loading ? "Buscando…" : `${library.page.total} imágenes`}
      </span>
    </div>
  </div>
  {#if error}
    <div class="notice error" role="alert">
      <span>{error}</span>
      <button aria-label="Cerrar error" onclick={ondismissError}>×</button>
    </div>
  {/if}
  {#if scan.toast && !scan.scanning}
    <div class="notice" role="status">
      <span>✓ {scan.toast}</span>
      <button aria-label="Cerrar aviso" onclick={ondismissToast}>×</button>
    </div>
  {/if}
  {#if scan.scanning}
    <div class="scan-notice" role="status">
      <span class="spinner"></span>
      <div>
        <strong>Preparando tu colección</strong>
        <small>{scan.message || "Explorando la carpeta…"}</small>
      </div>
      <button class="quiet-button" onclick={oncancel}>Cancelar</button>
    </div>
  {/if}
  {#if !library.ready && !error}
    <div class="empty">
      <span class="spinner"></span>
      <h2>Abriendo tu biblioteca…</h2>
    </div>
  {:else if !library.roots.length}
    <div class="empty welcome">
      <div class="empty-art" aria-hidden="true">
        <span>▧</span><span>▧</span><span>▧</span>
      </div>
      <p class="eyebrow">TU ARCHIVO, CON CONTEXTO</p>
      <h2>Cada imagen tiene una historia.</h2>
      <p>
        Añade una carpeta, escribe lo que te inspira<br />y
        encuéntralo después con una búsqueda.
      </p>
      <button
        class="primary-button"
        disabled={adding || !library.ready}
        onclick={onopen}
      >
        + Añadir mi primera carpeta
      </button>
      <small>JPEG · PNG · WebP · GIF · BMP</small>
    </div>
  {:else if !library.page.items.length && !library.loading && !scan.scanning}
    <div class="empty">
      <span class="empty-search" aria-hidden="true">⌕</span>
      <h2>
        {search ? "Todavía no hay coincidencias" : "Un espacio por descubrir"}
      </h2>
      <p>
        {search
          ? "Prueba otra palabra del nombre, la ruta o tus notas."
          : "Añade imágenes a tu carpeta y pulsa Actualizar."}
      </p>
      {#if search}
        <button class="quiet-button" onclick={() => onsearch("")}>
          Limpiar búsqueda
        </button>
      {/if}
    </div>
  {:else}
    <div class="gallery" class:pending={library.loading} aria-busy={library.loading}>
      {#each library.page.items as asset (asset.id)}
        <ImageTile
          {asset}
          selected={selectedId === asset.id}
          {render}
          {onselect}
        />
      {/each}
    </div>
  {/if}
  {#if library.page.total > pageSize}
    <nav class="pagination" aria-label="Páginas de imágenes">
      <button
        class="quiet-button"
        disabled={library.offset === 0 || library.loading}
        onclick={() => onnavigate(-1)}
      >
        ← Anterior
      </button>
      <span>
        {library.offset + 1}–{Math.min(library.offset + pageSize, library.page.total)} de {library.page.total}
      </span>
      <button
        class="quiet-button"
        disabled={library.offset + pageSize >= library.page.total || library.loading}
        onclick={() => onnavigate(1)}
      >
        Siguiente →
      </button>
    </nav>
  {/if}
  {#if currentRoot}
    <footer class="folder-footer">
      <span title={currentRoot.path}>{currentRoot.path}</span>
      <button disabled={scan.scanning} onclick={() => onremove(currentRoot!)}>
        Quitar de la biblioteca
      </button>
    </footer>
  {/if}
</section>
