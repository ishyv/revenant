<script lang="ts">
  import { onMount } from 'svelte';
  import { NativeSurface, type SurfaceEvent, type SurfaceInfo } from '../surface.js';
  import type { Scope } from '../scope.js';
  import type { Handle } from '../contracts.js';
  import type { OverlayCoordinator } from '../overlay.js';
  let { scope, scene, suspended = false, motion = true, labels = true,
    background, oninteraction, onerror, onready, overlays, name = 'Native visualization' }: {
    scope: Scope; scene: Handle; suspended?: boolean; motion?: boolean; labels?: boolean;
    background?: [number, number, number, number]; oninteraction?: (event: SurfaceEvent) => void;
    onerror?: (error: unknown) => void; onready?: (info: SurfaceInfo) => void; name?: string;
    overlays?: OverlayCoordinator;
  } = $props();
  let element: HTMLDivElement;
  let surface = $state<NativeSurface>();
  let error = $state('');
  let forcedHidden = false;
  let coveredHidden = false;
  let queued = false;
  let alive = true;
  let lastPlacement = Promise.resolve();
  let pendingPlacement: { bounds: [number, number, number, number]; visible: boolean; scale: number } | undefined;
  let placing = false;
  let placementFailure: unknown;

  function report(e: unknown) { if (!alive) return; error = e instanceof Error ? e.message : String(e); onerror?.(e); }
  function geometry(): [number, number, number, number] {
    const rect = element.getBoundingClientRect();
    return [rect.x, rect.y, rect.width, rect.height];
  }
  function supported(): boolean {
    for (let node: HTMLElement | null = element; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (style.transform !== 'none' || style.perspective !== 'none') {
        report(new Error('NativeCanvas requires a rectangular viewport with untransformed ancestors.')); return false;
      }
    }
    const [x,y,w,h] = geometry();
    return x >= 0 && y >= 0 && x+w <= window.innerWidth+1 && y+h <= window.innerHeight+1;
  }
  function place() {
    if (!surface || !alive) return;
    const visible = !forcedHidden && !coveredHidden && !suspended && !document.hidden && supported() && !error;
    pendingPlacement = { bounds: geometry(), visible, scale: window.devicePixelRatio };
    if (placing) return;
    placing = true;
    const target = surface;
    lastPlacement = (async () => {
      try {
        while (pendingPlacement && alive) {
          const next = pendingPlacement;
          pendingPlacement = undefined;
          try { await target.place(next.bounds, next.visible, next.scale); placementFailure = undefined; }
          catch (e) { placementFailure = e; throw e; }
        }
      } finally { placing = false; }
    })().catch(report);
  }
  function schedule() {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => { queued = false; place(); });
  }
  /** Await this before opening a modal covering the native child window. */
  export async function suspend(): Promise<void> { forcedHidden = true; await surface?.ready; place(); await lastPlacement; if (placementFailure) throw placementFailure; }
  /** Restore after the covering modal has been removed. */
  export async function restore(): Promise<void> { forcedHidden = false; await surface?.ready; place(); await lastPlacement; if (placementFailure) throw placementFailure; }
  /** Reset native pan and zoom. */
  export async function fit(): Promise<void> { await surface?.fit(); }

  $effect(() => { suspended; schedule(); });
  $effect(() => { if (surface) void surface.options(motion, labels, background).catch(report); });
  onMount(() => {
    coveredHidden = overlays?.covered ?? false;
    const target = new NativeSurface(scope, scene, event => {
      if (event.kind === 'error') report(new Error(event.message ?? 'Native GPU failed'));
      else oninteraction?.(event);
    });
    surface = target;
    const registration = overlays?.register({
      suspend: async () => { coveredHidden = true; await target.ready; if (!alive) return; place(); await lastPlacement; if (placementFailure) throw placementFailure; },
      restore: async () => { coveredHidden = false; await target.ready; if (!alive) return; place(); await lastPlacement; if (placementFailure) throw placementFailure; },
    }, scope);
    void target.ready.then(async info => { await registration?.ready; if (alive) { onready?.(info); place(); void target.fit().catch(report); } }).catch(report);
    const observer = new ResizeObserver(schedule); observer.observe(element);
    window.addEventListener('resize', schedule);
    window.addEventListener('scroll', schedule, true);
    document.addEventListener('visibilitychange', schedule);
    return () => {
      alive = false; observer.disconnect();
      registration?.dispose();
      window.removeEventListener('resize', schedule);
      window.removeEventListener('scroll', schedule, true);
      document.removeEventListener('visibilitychange', schedule);
      void target.dispose().catch(e => {
        scope.reportCleanup(e);
        onerror?.(e);
      });
    };
  });
</script>

<div bind:this={element} class="native-canvas" role="img" aria-label={name}>
  {#if error}<p role="alert">{error}</p>{/if}
</div>

<style>
  .native-canvas { width: 100%; height: 100%; min-height: 1px; overflow: hidden; }
  p { padding: 1rem; color: var(--text-soft, #b8c5d1); }
</style>
