/** Lightweight local collections for application state, separate from native file datasets. */
import { CapabilityError, errorOf, failure } from "./contracts.js";
import type { RuntimeError } from "./contracts.js";
import { Signal } from "./store.js";
import type { Readable } from "./store.js";
import type { Scope } from "./scope.js";
import type { Operation } from "./operation.js";
import type { BatchResult, Task } from "./task.js";

/** Local collection state. FileCollection has native counts and bounded windows instead. */
export interface CollectionSnapshot<T> {
  /** Immutable local items; native file inventories use bounded FileCollection windows. */
  items: readonly T[];
  /** Loading, available, failed, or locally disposed collection state. */
  state: "loading" | "ready" | "failed" | "disposed";
  /** Classified failure retained for a failed local collection. */
  error?: RuntimeError;
}
/** Small in-memory collection. Callback filtering/sorting never applies to native files. */
export class Collection<T> implements Readable<CollectionSnapshot<T>> {
  private store: Signal<CollectionSnapshot<T>>;
  private cleanups = new Set<() => void>();
  /** Svelte store subscription. */
  readonly subscribe: Readable<CollectionSnapshot<T>>["subscribe"];
  /** Own a small immutable local collection in the supplied application scope. */
  constructor(
    /** Scope owning this local collection and its derived stores. */
    readonly owner: Scope,
    items: readonly T[] = [],
    loading = false,
  ) {
    this.store = new Signal<CollectionSnapshot<T>>({
      items: Object.freeze([...items]),
      state: loading ? "loading" : "ready",
    });
    this.subscribe = this.store.subscribe;
    owner.own(this);
  }
  /** Latest local collection projection. */
  get snapshot(): CollectionSnapshot<T> {
    return this.store.current;
  }
  /** Entire local collection; keep application datasets small. */
  get items(): readonly T[] {
    return this.snapshot.items;
  }
  /** Wait for ready or reject on failure/disposal. */
  get loaded(): Promise<readonly T[]> {
    return new Promise((resolve, reject) => {
      let stop = () => {};
      let settled = false;
      stop = this.subscribe((snapshot) => {
        if (snapshot.state === "loading") return;
        settled = true;
        stop();
        if (snapshot.state === "ready") resolve(snapshot.items);
        else
          reject(
            snapshot.error ??
              new CapabilityError(
                "collection_disposed",
                "Collection was disposed while loading",
              ),
          );
      });
      if (settled) stop();
    });
  }
  /** Check local and owning scope lifetimes. */
  assert(): void {
    this.owner.assert();
    if (this.snapshot.state === "disposed")
      failure("collection_disposed", "Collection has been disposed");
  }
  /** Replace local items with an immutable array. */
  set(items: readonly T[]): void {
    this.assert();
    this.store.set({ ...this.snapshot, items: Object.freeze([...items]) });
  }
  /** Append to this small local collection. */
  append(items: readonly T[]): void {
    this.set([...this.items, ...items]);
  }
  /** Mark local loading complete. */
  ready(): void {
    this.assert();
    this.store.set({ ...this.snapshot, state: "ready", error: undefined });
  }
  /** Publish a classified failure. */
  fail(error: unknown): void {
    if (this.snapshot.state !== "disposed")
      this.store.set({
        ...this.snapshot,
        state: "failed",
        error: errorOf(error),
      });
  }
  /** Attach local teardown, primarily for derived stores. */
  onDispose(cleanup: () => void): void {
    this.assert();
    this.cleanups.add(cleanup);
  }
  /** Select local items; native batch selection belongs to FileCollection. */
  selection(): Selection<T> {
    this.assert();
    return new Selection(this);
  }
  /** Derive a locally filtered collection, automatically kept in sync. */
  filter(predicate: (item: T) => boolean): Collection<T> {
    return this.derive((items) => items.filter(predicate));
  }
  /** Derive a locally sorted collection without mutating the original. */
  sort(compare: (a: T, b: T) => number): Collection<T> {
    return this.derive((items) => [...items].sort(compare));
  }
  private derive(transform: (items: readonly T[]) => T[]): Collection<T> {
    this.assert();
    const child = new Collection<T>(
      this.owner,
      [],
      this.snapshot.state === "loading",
    );
    const stop = this.subscribe((snapshot) => {
      if (snapshot.state === "disposed") {
        child.dispose();
        return;
      }
      try {
        child.set(transform(snapshot.items));
        if (snapshot.state === "ready") child.ready();
        if (snapshot.error) child.fail(snapshot.error);
      } catch (error) {
        child.fail(error);
      }
    });
    child.onDispose(stop);
    return child;
  }
  /** Dispose the local store and its derivation subscriptions. */
  dispose(): void {
    if (this.snapshot.state === "disposed") return;
    this.store.set({ items: [], state: "disposed" });
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups.clear();
    this.store.clear();
    this.owner.release(this);
  }
}

/** Local identity-based selection; this is not a native batch selector. */
export class Selection<T> implements Readable<readonly T[]> {
  private store = new Signal<readonly T[]>([]);
  private stop: () => void;
  private disposed = false;
  /** Svelte store subscription. */
  readonly subscribe = this.store.subscribe;
  /** Own an identity-based local selection and follow its collection membership. */
  constructor(private collection: Collection<T>) {
    collection.owner.own(this);
    this.stop = collection.subscribe(({ items, state }) => {
      if (state === "disposed") this.dispose();
      else
        this.store.set(
          Object.freeze(this.items.filter((item) => items.includes(item))),
        );
    });
  }
  /** Currently selected local identities. */
  get items(): readonly T[] {
    return this.store.current;
  }
  /** Replace selection, accepting only local collection members. */
  set(items: readonly T[]): void {
    this.collection.assert();
    if (this.disposed)
      failure("selection_disposed", "Selection has been disposed");
    if (items.some((item) => !this.collection.items.includes(item)))
      failure(
        "invalid_selection",
        "Selection items must belong to the collection",
      );
    this.store.set(Object.freeze([...new Set(items)]));
  }
  /** Clear local selection. */
  clear(): void {
    this.set([]);
  }
  /** Run a bounded JSON-record batch on the native executor. File datasets use native selectors. */
  run<O>(operation: Operation<T, O>): Task<BatchResult<O>> {
    this.collection.assert();
    if (this.disposed)
      failure("selection_disposed", "Selection has been disposed");
    return operation.forScope(this.collection.owner).runBatch(this.items);
  }
  /** Unsubscribe and release ownership. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.stop?.();
    this.store.set([]);
    this.store.clear();
    this.collection.owner.release(this);
  }
}
