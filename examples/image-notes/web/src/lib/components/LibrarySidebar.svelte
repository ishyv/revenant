<script lang="ts">
  import type { Root } from "../types";

  let {
    roots,
    rootId,
    annotatedOnly,
    adding,
    scanning,
    ready,
    dataPath,
    onchoose,
    onannotations,
    onopen,
  }: {
    roots: Root[];
    rootId: string;
    annotatedOnly: boolean;
    adding: boolean;
    scanning: boolean;
    ready: boolean;
    dataPath: string;
    onchoose: (id: string, annotatedOnly: boolean) => void;
    onannotations: () => void;
    onopen: () => void;
  } = $props();

  const allCount = $derived(roots.reduce((sum, root) => sum + root.count, 0));
</script>

<aside class="sidebar" aria-label="Biblioteca">
  <div class="brand">
    <span class="brand-symbol" aria-hidden="true">◈</span>
    <span>Luma<small>IMAGE NOTES</small></span>
  </div>
  <p class="nav-label">TU BIBLIOTECA</p>
  <button
    class="nav-item"
    class:active={!rootId && !annotatedOnly}
    onclick={() => onchoose("", false)}
  >
    <span aria-hidden="true">▦</span> Todas las imágenes
    <span class="nav-count">{allCount}</span>
  </button>
  <button
    class="nav-item"
    class:active={annotatedOnly}
    onclick={onannotations}
  >
    <span aria-hidden="true">✎</span> Con anotaciones
  </button>
  <div class="folder-heading">
    <p class="nav-label">CARPETAS</p>
    <button
      class="icon-button"
      aria-label="Añadir carpeta"
      disabled={adding || scanning}
      onclick={onopen}
    >
      +
    </button>
  </div>
  <div class="folder-list">
    {#each roots as root (root.id)}
      <button
        class="nav-item folder"
        class:active={rootId === root.id}
        title={root.path}
        onclick={() => onchoose(root.id, false)}
      >
        <span aria-hidden="true">▱</span>
        <span class="folder-name">{root.name}</span>
        <span class="nav-count">{root.count}</span>
      </button>
    {/each}
    {#if !roots.length}
      <p class="sidebar-hint">
        Tus carpetas aparecerán aquí. Las imágenes se quedan donde
        están.
      </p>
    {/if}
  </div>
  <button
    class="add-folder"
    disabled={adding || scanning || !ready}
    onclick={onopen}
  >
    <span>+</span> {adding ? "Abriendo…" : "Añadir carpeta"}
  </button>
  <div class="sidebar-bottom">
    <span class="local-dot"></span> Solo en tu computadora
    <small title={dataPath}>Hecho con Revenant 0.3</small>
  </div>
</aside>
