/** Explicit read projections retain one current result without owning native execution policy. */
import { errorOf, failure } from "./contracts.js";
import type { RuntimeError } from "./contracts.js";
import type { Operation } from "./operation.js";
import type { Readable } from "./store.js";
import { Signal } from "./store.js";
import type { Task } from "./task.js";

/** State of an explicitly loaded operation query, independent of native task progress. */
export interface QuerySnapshot<O> {
  /** Idle until loaded; failed loads retain the last successful result. */
  status: "idle" | "loading" | "ready" | "failed";
  /** Last successful result, retained during refreshes and failures. */
  data?: O;
  /** Classified failure from the current load; cleared by loading or invalidation. */
  error?: RuntimeError;
}

/** Scope-owned readable for replaceable reads. Mutations should use explicit operation calls. */
export class Query<I, O> implements Readable<QuerySnapshot<O>> {
  private store = new Signal<QuerySnapshot<O>>({ status: "idle" });
  private generation = 0;
  private closed = false;
  private active?: Task<O>;
  private releases = new Set<Promise<void>>();
  private taskCleanup = new WeakMap<Task<O>, Promise<void>>();
  private cleanup?: Promise<void>;

  /** Observe query state using the standard Svelte readable-store interface. */
  readonly subscribe = this.store.subscribe;

  /** Register this read projection with the operation's native scope. */
  constructor(private operation: Operation<I, O>) {
    operation.owner.own(this);
  }

  /** Latest query state without allocating a subscription. */
  get snapshot(): QuerySnapshot<O> {
    return this.store.current;
  }

  /** Load explicitly; only the newest load publishes, and current failures become readable state. */
  async load(input: I): Promise<void> {
    this.assert();
    const version = this.supersede();
    this.store.set({ status: "loading", ...this.previousData() });

    // A synchronous subscriber may replace this load or dispose the scope.
    if (!this.current(version)) return;
    let task: Task<O> | undefined;
    try {
      task = this.operation.run(input);
      // The query takes ownership so a closing scope cannot report a task's
      // cleanup failure twice through both the task and its enclosing query.
      this.operation.owner.release(task);
      this.active = task;
      const data = await task.result;
      if (this.current(version)) this.store.set({ status: "ready", data });
    } catch (error) {
      if (this.current(version)) {
        this.store.set({
          status: "failed",
          ...this.previousData(),
          error: errorOf(error),
        });
      }
    } finally {
      if (task) {
        // Supersession already started this task's cleanup; dispose is idempotent.
        if (this.active === task) this.active = undefined;
        await this.release(task);
      }
    }
  }

  /** Suppress pending results immediately, before a debounced replacement is submitted. */
  invalidate(): void {
    this.assert();
    this.supersede();
    this.store.set({ status: "idle", ...this.previousData() });
  }

  private previousData(): Pick<QuerySnapshot<O>, "data"> {
    return "data" in this.snapshot ? { data: this.snapshot.data } : {};
  }

  private current(version: number): boolean {
    return !this.closed && !this.operation.owner.disposed && version === this.generation;
  }

  private assert(): void {
    this.operation.owner.assert();
    if (this.closed) failure("query_disposed", "This operation query has been disposed");
  }

  private supersede(): number {
    const version = ++this.generation;
    const task = this.active;
    this.active = undefined;
    if (task) void this.release(task);
    return version;
  }

  private release(task: Task<O>): Promise<void> {
    const existing = this.taskCleanup.get(task);
    if (existing) return existing;
    const pending = task.dispose().catch((error) => {
      const errors = this.operation.owner.cleanupErrors;
      errors.set([...errors.current, errorOf(error, "cleanup_failed")]);
    }).finally(() => this.releases.delete(pending));
    this.releases.add(pending);
    this.taskCleanup.set(task, pending);
    return pending;
  }

  /** Freeze publication and await all owned task cleanup; repeated calls share completion. */
  dispose(): Promise<void> {
    if (this.cleanup) return this.cleanup;
    this.closed = true;
    this.supersede();
    this.store.clear();
    this.operation.owner.release(this);
    this.cleanup = Promise.all([...this.releases]).then(() => {});
    return this.cleanup;
  }
}
