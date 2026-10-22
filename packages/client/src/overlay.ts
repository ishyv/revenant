/** Awaited native coverage around ordinary dialogs and pickers. */
import { errorOf, failure } from './contracts.js';
import type { RuntimeError } from './contracts.js';
import type { Scope, Disposable } from './scope.js';
import { Signal } from './store.js';

/** Canvas adapter; restore must recompute current visibility rather than force showing. */
export interface OverlaySurface {
  /** Hide and await native placement acknowledgement, including late attachment. */
  suspend(): Promise<void>;
  /** Restore eligible current placement and await acknowledgement. */
  restore(): Promise<void>;
}
// Failed hides still need restoration; only acknowledged hides admit covering content.
interface Entry { surface: OverlaySurface; alive: boolean; hidden: boolean; hideAcknowledged: boolean; }
/** Component registration; readiness settles coverage before the initial visible placement. */
export interface OverlayRegistration extends Disposable {
  /** Late-mounted canvases are hidden while any cover is active. */
  readonly ready: Promise<void>;
}
/** A cover can be released only after its content-close acknowledgement. */
export interface OverlayCover {
  /** Remove content and await DOM/outro removal before releasing native coverage. */
  close(removeContent: () => Promise<void>): Promise<void>;
}
class CoverToken implements OverlayCover {
  private released = false;
  private closing?: Promise<void>;
  /** Coordinator-issued token; authors obtain these through cover(). */
  constructor(private release: () => Promise<void>, private report: (error: unknown) => void) {}
  /** Remove content (and await its DOM/outro removal), then restore eligible native surfaces. */
  close(removeContent: () => Promise<void>): Promise<void> {
    if (this.released) return this.closing ?? Promise.resolve();
    if (this.closing) return this.closing;
    this.closing = (async () => {
      try { await removeContent(); }
      catch (error) { this.report(error); throw error; }
      this.released = true;
      await this.release();
    })();
    void this.closing.catch(() => { if (!this.released) this.closing = undefined; });
    return this.closing;
  }
}
/** One scope owns overlapping covers, surface registrations and observable placement failures. */
export class OverlayCoordinator implements Disposable {
  private entries = new Set<Entry>();
  private count = 0;
  private closed = false;
  private queue: Promise<void> = Promise.resolve();
  private focus?: HTMLElement;
  /** Hide, content-close and restore errors remain observable. */
  readonly errors = new Signal<readonly RuntimeError[]>([]);
  /** Owner teardown signal allows mounted adapters to remove their content. */
  readonly state = new Signal<{ disposed: boolean }>({ disposed: false });
  /** Register this coordinator for owner teardown. */
  constructor(private owner: Scope) { owner.own(this); }
  /** Whether native surfaces are covered by at least one admitted token. */
  get covered(): boolean { return this.count > 0; }
  /** Keep every failure available without creating an unhandled rejection. */
  report(error: unknown): void { this.errors.set([...this.errors.current.slice(-63), errorOf(error, 'overlay_failed')]); }
  private assert(): void {
    this.owner.assert();
    if (this.closed) failure('overlay_disposed', 'Overlay coordinator has been disposed');
  }
  private enqueue<T>(action: () => Promise<T>): Promise<T> {
    const pending = this.queue.then(action).catch(error => { this.report(error); throw error; });
    this.queue = pending.then(() => {}, () => {});
    return pending;
  }
  /** Register before a canvas's first placement; teardown makes it immediately ineligible. */
  register(surface: OverlaySurface, surfaceOwner: Scope = this.owner): OverlayRegistration {
    this.assert();
    let ancestor: Scope | undefined = surfaceOwner;
    while (ancestor && ancestor !== this.owner) ancestor = ancestor.parent;
    if (!ancestor) failure('scope_mismatch', 'Covered surfaces must belong to the coordinator owner or a descendant');
    const entry = { surface, alive: true, hidden: false, hideAcknowledged: false };
    this.entries.add(entry);
    const ready = this.enqueue(async () => {
      if (entry.alive && !this.closed && this.count) {
        entry.hidden = true;
        await surface.suspend();
        entry.hideAcknowledged = true;
      }
    });
    void ready.catch(() => {});
    return { ready, dispose: () => { entry.alive = false; this.entries.delete(entry); } };
  }
  /** Resolve only when all currently registered live surfaces acknowledge hiding. */
  cover(): Promise<OverlayCover> {
    this.assert();
    return this.enqueue(async () => {
      this.assert();
      if (!this.count && typeof document !== 'undefined') {
        const element = document.activeElement;
        this.focus = element instanceof HTMLElement ? element : undefined;
      }
      ++this.count;
      try {
        // Registrations added during hiding must also settle before content opens.
        while (true) {
          const targets = [...this.entries].filter(entry => entry.alive && !entry.hideAcknowledged);
          if (!targets.length) break;
          const results = await Promise.allSettled(targets.map(async entry => {
            entry.hidden = true;
            await entry.surface.suspend();
            entry.hideAcknowledged = true;
          }));
          const failed = results.find(result => result.status === 'rejected');
          if (failed?.status === 'rejected') throw failed.reason;
          this.assert();
        }
        return new CoverToken(() => this.releaseCover(), error => this.report(error));
      } catch (error) {
        --this.count;
        if (!this.count && !this.closed && !this.owner.disposed) {
          try { await this.restore(); } catch (restoreError) { this.report(restoreError); }
        }
        throw error;
      }
    });
  }
  /** Internal token acknowledgement; root teardown deliberately never restores. */
  private releaseCover(): Promise<void> {
    return this.enqueue(async () => {
      if (this.closed || this.owner.disposed) return;
      if (this.count) --this.count;
      if (!this.count) await this.restore();
    });
  }
  private async restore(): Promise<void> {
    const results = await Promise.allSettled([...this.entries].filter(entry => entry.alive && entry.hidden).map(async entry => {
      entry.hidden = false;
      entry.hideAcknowledged = false;
      await entry.surface.restore();
    }));
    const failed = results.find(result => result.status === 'rejected');
    if (failed?.status === 'rejected') throw failed.reason;
    if (this.focus?.isConnected && typeof document !== 'undefined' && !document.hidden && !this.closed && !this.owner.disposed)
      this.focus.focus({ preventScroll: true });
    this.focus = undefined;
  }
  /** Freeze registrations and await in-flight acknowledgements; never reveal surfaces on teardown. */
  dispose(): Promise<void> {
    this.closed = true;
    this.state.set({ disposed: true });
    this.state.clear();
    for (const entry of this.entries) entry.alive = false;
    this.entries.clear(); this.focus = undefined;
    this.owner.release(this);
    return this.queue;
  }
}
