<script lang="ts">
  import type { Asset, ImageRenderer } from "../types";
  let { asset, selected = false, render, onselect }: {
    asset: Asset;
    selected?: boolean;
    render: ImageRenderer;
    onselect: (asset: Asset) => void;
  } = $props();
  let element = $state<HTMLButtonElement>();
  let url = $state("");
  let failed = $state(false);
  $effect(() => {
    const current = asset;
    const rendition = render;
    const node = element;
    url = "";
    failed = false;
    if (!node) return;
    let alive = true;
    let requested = false;
    const controller = new AbortController();
    const load = () => {
      if (requested) return;
      requested = true;
      void rendition(current.id, false, controller.signal)
        .then((value) => {
          if (alive) url = value;
        })
        .catch(() => {
          if (alive) failed = true;
        });
    };
    // Decode only tiles near the viewport, and drop queued work on unmount.
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          load();
          observer.disconnect();
        }
      },
      { rootMargin: "160px" },
    );
    observer.observe(node);
    return () => {
      alive = false;
      controller.abort();
      observer.disconnect();
    };
  });
</script>
<button
  bind:this={element}
  class="tile"
  class:selected
  onclick={() => onselect(asset)}
  aria-pressed={selected}
  aria-label={`Abrir ${asset.name}${asset.note ? ", con nota" : ""}`}
>
  <div class="tile-image">
    {#if url}
      <img src={url} alt="" draggable="false" />
    {:else if failed}
      <span class="tile-failure">Vista no disponible</span>
    {:else}
      <span class="tile-placeholder" aria-hidden="true"></span>
    {/if}
    {#if asset.note}
      <span class="note-dot" aria-label="Tiene una nota"></span>
    {/if}
    {#if selected}
      <span class="selected-badge" aria-hidden="true">✓</span>
    {/if}
  </div>
  <span class="tile-name" title={asset.name}>{asset.name}</span>
  <span class="tile-caption">{asset.note || asset.relativePath}</span>
</button>
