/** Minimal Svelte-compatible signals with deterministic synchronous publication ordering. */

/** Stop a subscription. */
export type Unsubscribe = () => void;
/** Svelte readable store contract; subscribing immediately publishes its current value. */
export interface Readable<T> {
  /** Publish the current value immediately and return a subscription cleanup function. */
  subscribe(run: (value: T) => void, invalidate?: () => void): Unsubscribe;
}
const queue: Array<() => void> = [];
let flushing = false;

/** Internal mutable signal used by the public read-only capability projections. */
export class Signal<T> implements Readable<T> {
  private subscribers = new Set<{
    run: (value: T) => void;
    invalidate?: () => void;
  }>();
  /** Start with a synchronously available snapshot. */
  constructor(private value: T) {}
  /** Current snapshot, without allocating a temporary subscription. */
  get current(): T {
    return this.value;
  }
  /** Publish immediately, then observe subsequent changes until unsubscribed. */
  subscribe = (
    run: (value: T) => void,
    invalidate?: () => void,
  ): Unsubscribe => {
    const subscriber = { run, invalidate };
    this.subscribers.add(subscriber);
    run(this.value);
    return () => {
      this.subscribers.delete(subscriber);
    };
  };
  /** Queue publications so nested writes preserve observable ordering. */
  set(value: T): void {
    this.value = value;
    for (const subscriber of this.subscribers) {
      subscriber.invalidate?.();
      queue.push(() => {
        if (this.subscribers.has(subscriber)) subscriber.run(value);
      });
    }
    if (flushing) return;
    flushing = true;
    try {
      while (queue.length) {
        try {
          queue.shift()!();
        } catch (error) {
          queueMicrotask(() => {
            throw error;
          });
        }
      }
    } finally {
      flushing = false;
    }
  }
  /** Detach all consumers when the owning projection closes. */
  clear(): void {
    this.subscribers.clear();
  }
}
