<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  import { Modal } from '@hyvnt/hyvui';
  import type { OverlayCoordinator, OverlayCover } from '../overlay.js';

  let { overlays, open = false, title = '', ariaLabel, children, header, footer, onclose, onerror }: {
    overlays: OverlayCoordinator; open?: boolean; title?: string; ariaLabel?: string;
    children?: Snippet; header?: Snippet; footer?: Snippet;
    onclose?: () => void; onerror?: (error: unknown) => void;
  } = $props();
  let visible = $state(false);
  const overlayState = { subscribe: (run: (value: { disposed: boolean }) => void) => overlays.state.subscribe(run) };

  // Mount ordinary HyvUI content only after native hiding. Effect teardown removes
  // content and awaits Svelte's DOM removal before relinquishing the cover token.
  $effect(() => {
    if (!open || $overlayState.disposed) return;
    let cancelled = false;
    let token: OverlayCover | undefined;
    void overlays.cover().then(async cover => {
      token = cover;
      if (cancelled) await cover.close(tick);
      else visible = true;
    }).catch(error => onerror?.(error));
    return () => {
      cancelled = true;
      visible = false;
      if (token) void token.close(tick).catch(error => onerror?.(error));
    };
  });
</script>

{#if visible}
  <Modal open title={title} {ariaLabel} {children} {header} {footer} {onclose} />
{/if}
