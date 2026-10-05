<script lang="ts">
  import { onDestroy } from "svelte";
  import { imagePreview } from "../models/image-preview";
  import type { NoteEditor } from "../models/note-editor";
  import type { Asset, ImageRenderer, Root } from "../types";

  let { asset, editor, roots, render, onclose, onsave }: {
    asset: Asset;
    editor: NoteEditor;
    roots: Root[];
    render: ImageRenderer;
    onclose: () => void;
    onsave: () => void;
  } = $props();

  const preview = imagePreview();
  const id = $derived(asset.id);
  $effect(() => {
    // Note commits change the asset object, but only identity/rendition changes
    // restart a preview. The inspector releases its own selection lifetime.
    void preview.select(id, render);
    return () => preview.clear();
  });
  onDestroy(() => preview.dispose());

  function bytes(value: string) {
    const size = Number(value);
    return size < 1048576
      ? `${Math.max(1, Math.round(size / 1024))} KB`
      : `${(size / 1048576).toFixed(1)} MB`;
  }
</script>

<aside class="inspector" aria-label="Imagen y anotación">
  <div class="inspector-heading">
    <span>DETALLES</span>
    <button class="icon-button" aria-label="Cerrar detalles" onclick={onclose}>×</button>
  </div>
  <div class="preview-stage">
    {#if $preview.url}
      <img src={$preview.url} alt={asset.name} />
    {:else if $preview.loading}
      <span class="spinner"></span>
    {:else}
      <p>{$preview.error || "Vista no disponible"}</p>
    {/if}
  </div>
  <div class="inspector-content">
    <h2>{asset.name}</h2>
    <p class="file-info">
      {bytes(asset.size)} <span>·</span>
      {roots.find((root) => root.id === asset.rootId)?.name}
    </p>
    <div class="note-heading">
      <label for="note">Tu anotación</label>
      <span
        class="save-status"
        class:unsaved={$editor.dirty || $editor.status === "failed"}
        aria-live="polite"
      >
        {$editor.status === "saving"
          ? "Guardando…"
          : $editor.status === "failed"
            ? "No se pudo guardar"
            : $editor.dirty
              ? "Sin guardar"
              : "Guardado"}
      </span>
    </div>
    <textarea
      id="note"
      placeholder="¿Qué quieres recordar de esta imagen?\nIdeas, detalles, referencias…"
      value={$editor.draft}
      oninput={(event) => editor.edit(event.currentTarget.value)}
      onblur={onsave}
    ></textarea>
    <div class="note-actions">
      <span>Ctrl / ⌘ S para guardar</span>
      <button
        class="primary-button small"
        disabled={!$editor.dirty || $editor.status === "saving"}
        onclick={onsave}
      >
        Guardar nota
      </button>
    </div>
    <div class="detail-divider"></div>
    <p class="nav-label">UBICACIÓN</p>
    <p class="image-location">{asset.relativePath}</p>
    <p class="inspector-tip">
      Las notas se guardan en tu biblioteca local. Tus
      imágenes originales permanecen intactas.
    </p>
  </div>
</aside>
