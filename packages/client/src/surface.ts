/** Scoped placement and events for native-only Scene2D resources. */
import type { Handle } from './contracts.js';
import { Scope } from './scope.js';
import { failure } from './contracts.js';

/** Native geometry interaction without inventory or scene payloads. */
export interface SurfaceEvent {
  /** Native viewport interaction classification. */
  kind: 'hover' | 'select' | 'activate' | 'error';
  /** Stable native primitive identity; absent for empty space. */
  key?: string | null;
  /** Physical-pixel position in the viewport. */
  point?: [number, number];
  /** Lossless scene revision used for picking. */
  revision?: string;
  /** Explicit GPU error, with the surrounding analysis UI remaining usable. */
  message?: string;
}
/** Diagnostic admission summary; no scene data. */
export interface SurfaceInfo {
  /** Opaque native attachment identity. */
  viewport: string;
  /** Actual GPU adapter name. */
  adapter: string;
  /** Framework-owned buffer estimate; excludes driver-managed memory. */
  bufferBytes: number;
}

/** A native attachment owned by the same scope that owns its scene resource. */
export class NativeSurface {
  private closed = false;
  private info?: SurfaceInfo;
  private cleanup?: Promise<void>;
  /** Native GPU initialization and attachment acknowledgement. */
  readonly ready: Promise<SurfaceInfo>;
  /** Attach an admitted scene; window and scope authority are validated natively. */
  constructor(private scope: Scope, scene: Handle, onEvent?: (event: SurfaceEvent) => void) {
    scope.own(this);
    this.ready = scope.request<SurfaceInfo, SurfaceEvent>({ action: 'surface.attach', scene }, event => {
      if (!this.closed && !scope.disposed) onEvent?.(event);
    }).then(info => { this.info = info; return info; });
    void this.ready.catch(() => {});
  }
  private async request(request: Record<string, unknown>): Promise<void> {
    if (this.closed) failure('surface_disposed', 'Native viewport has been disposed');
    const info = await this.ready;
    if (this.closed) failure('surface_disposed', 'Native viewport has been disposed');
    await this.scope.request({ ...request, action: String(request.action), viewport: info.viewport });
  }
  /** Set untransformed CSS bounds. Resolves after native show/hide acknowledgement. */
  async place(bounds: [number, number, number, number], visible: boolean, pixelRatio?: number): Promise<void> {
    await this.request({ action: 'surface.place', bounds, visible, ...(pixelRatio === undefined ? {} : { pixelRatio }) });
  }
  /** Set motion, labels, and linear background color. */
  async options(motion: boolean, labels: boolean, background?: [number, number, number, number]): Promise<void> {
    await this.request({ action: 'surface.options', motion, labels, ...(background ? { background } : {}) });
  }
  /** Reset native pan and zoom to fit scene content. */
  async fit(): Promise<void> { await this.request({ action: 'surface.fit' }); }
  /** Destroy the attachment before relinquishing the owning scope, even after late admission. */
  dispose(): Promise<void> {
    if (this.cleanup) return this.cleanup;
    this.closed = true;
    this.scope.release(this);
    this.cleanup = (async () => {
      try { await this.ready; } catch { return; }
      if (this.info) await this.scope.transport.dispatch({ action: 'surface.dispose', scope: await this.scope.ready, viewport: this.info.viewport });
    })();
    return this.cleanup;
  }
}
