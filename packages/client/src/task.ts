/** Tasks are continuously updated native projections; JS never schedules execution. */
import {
  CapabilityError,
  errorOf,
  failure,
  terminalStates,
  windowBounds,
} from "./contracts.js";
import type { Outcome, ResultPage, TaskSnapshot } from "./contracts.js";
import type { Readable } from "./store.js";
import { Signal } from "./store.js";
import type { Scope } from "./scope.js";
import type { NativeRequest } from "./transport.js";

/** Paged native batch outcomes. Client access ends with task/scope disposal;
 * native history may remain until executing workers exit.
 */
export class BatchResult<O> {
  /** Project retained native task outcomes through bounded, schema-checked pages. */
  constructor(
    private task: Task<unknown>,
    private checkOutput?: (output: unknown) => void,
  ) {}
  /** Read at most 512 outcomes; defaults to 128. Failed item outcomes stay available. */
  async page(offset = 0, limit = 128): Promise<ResultPage<O>> {
    const page = await this.task.results<O>(offset, limit);
    for (const item of page.items)
      if (!item.error) this.checkOutput?.(item.output);
    return page;
  }
  /** Iterate bounded pages without materializing the entire batch in JavaScript. */
  async *pages(limit = 128): AsyncIterable<ResultPage<O>> {
    windowBounds(0, limit);
    let offset = 0;
    while (true) {
      const page = await this.page(offset, limit);
      yield page;
      const next = page.offset + page.items.length;
      if (next >= page.total) return;
      if (next <= offset)
        failure("invalid_result_page", "Native result paging made no progress");
      offset = next;
    }
  }
}

/** Native task observable with an eagerly maintained result promise, even without subscribers. */
export class Task<O, U = never> implements Readable<TaskSnapshot<O, U>> {
  private store = new Signal<TaskSnapshot<O, U>>({
    id: "",
    scope: "",
    state: "queued",
    progress: { completed: "0" },
    outcomes: [],
    cancelRequested: false,
    summary: { completed: "0", succeeded: "0", failed: "0" },
  });
  private disposed = false;
  private settled = false;
  private cancelPending = false;
  private resolve!: (value: O) => void;
  private reject!: (error: unknown) => void;
  private started: Promise<string>;
  private cleanup?: Promise<void>;
  private updateStore = new Signal<U | undefined>(undefined);
  private updateRevision = -1n;
  private snapshotRevision = -1n;
  /** Latest schema-checked application data. New subscribers receive the current value. */
  readonly updates: Readable<U | undefined> = { subscribe: this.updateStore.subscribe };
  /** Standard Svelte store subscription; execution never depends on subscriber count. */
  readonly subscribe = this.store.subscribe;
  /** Settles from the native terminal snapshot; scope disposal rejects pending consumers. */
  readonly result: Promise<O>;

  /** Begin native task submission and maintain its result independently of store subscriptions. */
  constructor(
    /** Application scope retaining native task and result-store ownership. */
    readonly owner: Scope,
    request: NativeRequest | (() => Promise<NativeRequest>),
    prepare: () => Promise<void> = async () => {},
    private project: (snapshot: TaskSnapshot<unknown>, task: Task<O, U>) => O = (
      snapshot,
    ) => snapshot.result as O,
    private checkUpdate?: (value: unknown) => void,
  ) {
    owner.own(this);
    this.result = new Promise<O>((resolve, reject) => {
      this.resolve = resolve;
      this.reject = reject;
    });
    void this.result.catch(() => {});
    this.started = (async () => {
      await prepare();
      owner.assert();
      if (this.disposed)
        failure("task_disposed", "Task was disposed before native submission");
      const payload = typeof request === "function" ? await request() : request;
      if (this.disposed)
        failure("task_disposed", "Task was disposed before native submission");
      let streamed = false;
      const initial = await owner.request<TaskSnapshot<unknown>>(
        payload,
        (snapshot) => {
          streamed = true;
          this.accept(snapshot);
        },
      );
      // A Channel terminal update can arrive before the invocation resolves.
      if (!streamed && !this.terminal && !this.disposed) this.accept(initial);
      return initial.id;
    })();
    void this.started.catch((error) => {
      if (this.disposed) return;
      const failed = errorOf(error);
      this.store.set({ ...this.snapshot, state: "failed", error: failed });
      this.finishFailure(failed);
    });
  }

  /** Latest host projection. Empty identity means native submission is still pending. */
  get snapshot(): TaskSnapshot<O, U> {
    return this.store.current;
  }
  /** Native admission acknowledgement. Selection edits may use it as an input-freeze barrier. */
  get ready(): Promise<void> {
    return this.started.then(() => {});
  }
  /** Whether the native snapshot is logically terminal; this does not prove worker exit. */
  get terminal(): boolean {
    return terminalStates.has(this.snapshot.state);
  }

  private accept(snapshot: TaskSnapshot<unknown>): void {
    if (this.disposed || this.terminal) return;
    let pendingUpdate: U | undefined;
    let hasUpdate = false;
    let acceptedRevision: bigint | undefined;
    try {
      // Compatibility snapshots without revisions never replace a revisioned projection.
      if (snapshot.revision !== undefined) {
        if (!/^(0|[1-9][0-9]*)$/.test(snapshot.revision)) failure("invalid_task_revision", "Task revision is not a decimal counter");
        const revision = BigInt(snapshot.revision);
        if (revision <= this.snapshotRevision) return;
        if (snapshot.update) {
          if (!/^(0|[1-9][0-9]*)$/.test(snapshot.update.revision)) failure("invalid_task_revision", "Update revision is not a decimal counter");
          const updateRevision = BigInt(snapshot.update.revision);
          if (updateRevision > revision) failure("invalid_task_revision", "Update revision exceeds task revision");
          if (updateRevision > this.updateRevision) {
            if (new TextEncoder().encode(JSON.stringify(snapshot.update.value)).byteLength > 64 * 1024)
              failure("task_update_limit", "Task update exceeds 64 KiB encoded JSON");
            if (!this.checkUpdate) failure("task_updates_undeclared", "Operation has no compiled update contract");
            this.checkUpdate(snapshot.update.value);
            this.updateRevision = updateRevision;
            pendingUpdate = structuredClone(snapshot.update.value) as U;
            hasUpdate = true;
          }
        }
        this.snapshotRevision = revision;
        acceptedRevision = revision;
      } else if (this.snapshotRevision >= 0n) return;
      else if (snapshot.update) failure("invalid_task_revision", "Application updates require task revisions");
    } catch (error) {
      const failed = errorOf(error, "contract_violation");
      this.store.set({ ...this.snapshot, state: "failed", error: failed });
      this.finishFailure(failed);
      return;
    }
    const local = {
      ...snapshot,
      outcomes: snapshot.outcomes ?? [],
      cancelRequested: snapshot.cancelRequested || this.cancelPending,
    } as TaskSnapshot<O, U>;
    this.store.set(local);
    if (this.disposed || (acceptedRevision !== undefined && acceptedRevision !== this.snapshotRevision)) return;
    if (hasUpdate) this.updateStore.set(pendingUpdate);
    if (this.disposed || (acceptedRevision !== undefined && acceptedRevision !== this.snapshotRevision)) return;
    if (!terminalStates.has(snapshot.state) || this.settled) return;
    if (snapshot.state === "succeeded" || snapshot.state === "partial") {
      try {
        const result = this.project(snapshot, this);
        this.store.set({ ...local, result });
        this.settled = true;
        this.resolve(result);
      } catch (error) {
        this.store.set({
          ...local,
          error: errorOf(error, "contract_violation"),
        });
        this.finishFailure(error);
      }
    } else {
      this.finishFailure(
        snapshot.error ??
          new CapabilityError(snapshot.state, `Native task ${snapshot.state}`),
      );
    }
  }

  private finishFailure(error: unknown): void {
    if (this.settled) return;
    this.settled = true;
    this.reject(error);
  }

  private assert(): void {
    this.owner.assert();
    if (this.disposed) failure("task_disposed", "Task has been disposed");
  }

  /** Request native cancellation, including while startup is pending. */
  async cancel(): Promise<void> {
    this.assert();
    if (this.terminal) return;
    this.cancelPending = true;
    this.store.set({ ...this.snapshot, cancelRequested: true });
    const task = await this.started;
    this.assert();
    if (!this.terminal)
      this.accept(
        await this.owner.request<TaskSnapshot<unknown>>({
          action: "task.cancel",
          task,
        }),
      );
  }

  /** Explicitly refresh a snapshot; normal updates arrive over the submission Channel. */
  async refresh(): Promise<void> {
    this.assert();
    const task = await this.started;
    this.assert();
    this.accept(
      await this.owner.request<TaskSnapshot<unknown>>({
        action: "task.snapshot",
        task,
      }),
    );
  }

  /** Read retained item outcomes, including after partial completion or cancellation. */
  async results<R = unknown>(offset = 0, limit = 128): Promise<ResultPage<R>> {
    this.assert();
    const bounds = windowBounds(offset, limit);
    const task = await this.started;
    this.assert();
    return this.owner.request({ action: "task.results", task, ...bounds });
  }

  /** Close local publication and acknowledge native disposal/cancellation requests.
   * Running executors retain their history/resources until actual exit; this promise is not a join.
   */
  dispose(): Promise<void> {
    if (this.cleanup) return this.cleanup;
    this.disposed = true;
    this.finishFailure(
      new CapabilityError("task_disposed", "Task ownership was disposed"),
    );
    this.store.clear();
    this.updateStore.clear();
    this.owner.release(this);
    this.cleanup = (async () => {
      let task: string;
      try {
        task = await this.started;
      } catch {
        return;
      }
      const scope = await this.owner.ready;
      // Cleanup bypasses scope.assert: the parent may already be closing.
      await this.owner.transport.dispatch({
        action: "task.dispose",
        scope,
        task,
      });
    })();
    return this.cleanup;
  }
}
