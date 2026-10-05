/** Native folders and bounded indexed windows. No file bytes or complete scans enter JS. */
import {
  CapabilityError,
  errorOf,
  failure,
  windowBounds,
} from "./contracts.js";
import type { FileEntry, Handle, RuntimeError } from "./contracts.js";
import type { Scope } from "./scope.js";
import type { Readable } from "./store.js";
import { Signal } from "./store.js";
import type { NativeSelection, Operation } from "./operation.js";
import type { BatchResult, Task } from "./task.js";
import type { SelectionConnection } from "./wire.js";

/** Native query options. Filters and ordering apply to the complete native index. */
export interface FileQueryOptions {
  /** Whether the native index traverses descendants of the chosen folder. */
  recursive?: boolean;
  /** Native search applied to the indexed dataset rather than the loaded window. */
  search?: string;
  /** Native ordering key applied to all indexed matching entries. */
  sort?: "name" | "relativePath" | "size" | "modified";
  /** Whether native ordering runs in descending order. */
  descending?: boolean;
}
/** One native viewport plus scan metadata; total is the currently indexed matching count. */
export interface FileCollectionSnapshot {
  /** Native view identity, available even when a Channel precedes its invoke reply. */
  view: string;
  /** Immutable bounded page of native file metadata, never the complete inventory. */
  items: readonly FileEntry[];
  /** Safe integer position of the requested viewport in the matching dataset. */
  offset: number;
  /** Safe integer count of currently indexed matching files. */
  total: number;
  /** Safe integer count of indexed regular files before query filtering. */
  discovered: number;
  /** Native scan lifecycle; ready means enumeration has completed. */
  state: "loading" | "ready" | "failed" | "disposed";
  /** Client query generation echoed by native snapshots. */
  generation: number;
  /** Monotonic native page revision for this view, acknowledged even when discarded. */
  revision: number;
  /** Actual index inventory generation; independent of page/query revisions. */
  indexGeneration: number;
  /** Bounded examples of classified native enumeration failures. */
  warnings: readonly RuntimeError[];
  /** Total native enumeration warnings, including omitted examples. */
  warningCount: number;
  /** Optional classified native view or client acknowledgement failure. */
  error?: RuntimeError;
}
/** Serializable host folder restoration payload. Reopening always readmits native ownership. */
export interface FolderBookmark {
  /** Serializable native restoration location; reopening readmits folder ownership. */
  path: string;
}
/** Native picker/reopen descriptor; bookmark is the host-authorized restoration payload. */
export interface FolderDescriptor {
  /** Opaque scoped native folder resource identity. */
  folder: string;
  /** Native folder display name suitable for presentation. */
  name: string;
  /** Native display path; it is not a file capability. */
  path: string;
  /** Native restoration payload suitable for persistent settings. */
  bookmark: FolderBookmark;
}
interface ViewResponse {
  view: string;
  snapshot: FileCollectionSnapshot;
}

/** Native folder picker. User cancellation resolves to undefined. */
export class Files<Checksum = unknown> {
  /** Bind the native folder picker and compiled checksum operation to a scope. */
  constructor(
    private owner: Scope,
    /** Manifest-backed built-in; generated facades refine its output type without manual schemas. */
    readonly checksum: Operation<FileEntry, Checksum>,
  ) {}
  /** Open the host's directory picker and project its scoped folder resource. */
  async openFolder(): Promise<Folder | undefined> {
    const descriptor = await this.owner.request<FolderDescriptor | null>({
      action: "folder.open",
    });
    this.owner.assert();
    return descriptor ? new Folder(this.owner, descriptor) : undefined;
  }
  /** Restore a saved native folder bookmark through the host's normal capability admission. */
  async reopenBookmark(bookmark: FolderBookmark): Promise<Folder | undefined> {
    const descriptor = await this.owner.request<FolderDescriptor | null>({
      action: "folder.reopen",
      bookmark,
    });
    this.owner.assert();
    return descriptor ? new Folder(this.owner, descriptor) : undefined;
  }
}

/** Scoped native folder token. Native scope disposal closes the folder and its capabilities. */
export class Folder {
  private disposed = false;
  private views = new Set<FileCollection>();
  private cleanup?: Promise<void>;
  /** Own a native folder descriptor returned by picker or bookmark admission. */
  constructor(
    /** Scope owning the admitted native folder and its view projections. */
    readonly owner: Scope,
    /** Native picker or bookmark response describing this owned folder. */
    readonly descriptor: FolderDescriptor,
  ) {
    owner.own(this);
  }
  /** Name suitable for presentation. */
  get name(): string {
    return this.descriptor.name;
  }
  /** Native display path, not an authorization token. */
  get path(): string {
    return this.descriptor.path;
  }
  /** Serializable host bookmark, suitable for native settings restoration. */
  get bookmark(): FolderBookmark {
    return { ...this.descriptor.bookmark };
  }
  /** Begin an indexed native view and return its store immediately. */
  files(options: FileQueryOptions = {}): FileCollection {
    this.owner.assert();
    if (this.disposed)
      failure("folder_disposed", "Native folder has been disposed");
    const view = new FileCollection(
      this.owner,
      this.descriptor.folder,
      options,
      () => this.views.delete(view),
    );
    this.views.add(view);
    return view;
  }
  /** Close the folder's views and its native owner; captured task leases remain host-owned. */
  dispose(): Promise<void> {
    if (this.cleanup) return this.cleanup;
    this.disposed = true;
    this.owner.release(this);
    this.cleanup = (async () => {
      const views = await Promise.allSettled(
        [...this.views].map((view) => view.dispose()),
      );
      this.views.clear();
      await this.owner.transport.dispatch({
        action: "folder.dispose",
        scope: await this.owner.ready,
        folder: this.descriptor.folder,
      });
      const rejected = views.find((result) => result.status === "rejected");
      if (rejected?.status === "rejected") throw rejected.reason;
    })();
    return this.cleanup;
  }
}

/** Native view with a single bounded window. Query generations reject late responses. */
export class FileCollection implements Readable<FileCollectionSnapshot> {
  private store = new Signal<FileCollectionSnapshot>({
    view: "",
    items: [],
    offset: 0,
    total: 0,
    discovered: 0,
    state: "loading",
    generation: 1,
    revision: 0,
    indexGeneration: 0,
    warnings: [],
    warningCount: 0,
  });
  private view?: string;
  private options: FileQueryOptions;
  private generation = 1;
  private windowRevision = 0;
  private desiredOffset = 0;
  private desiredLimit = 128;
  private disposed = false;
  private queryWork: Promise<void>;
  private windowWork: Promise<void> = Promise.resolve();
  private cleanup?: Promise<void>;
  private captures = new Set<Promise<void>>();
  private acknowledged: Promise<void> = Promise.resolve();
  private acknowledgementError?: RuntimeError;
  private retiredViews = new Set<Promise<void>>();
  private lastSnapshotRevision = -1;
  private lastSnapshotView?: string;
  /** Standard Svelte store subscription. */
  readonly subscribe = this.store.subscribe;

  /** Create a scoped native indexed query and begin its initial bounded subscription. */
  constructor(
    /** Scope owning this native indexed view and its selections. */
    readonly owner: Scope,
    private folder: string,
    options: FileQueryOptions = {},
    private released?: () => void,
  ) {
    this.options = { ...options };
    owner.own(this);
    this.queryWork = this.submitQuery(this.generation, this.options);
    void this.queryWork.catch(() => {});
  }

  /** First or most recent query acknowledgement; the native scan can still be loading. */
  get ready(): Promise<void> {
    return this.queryWork;
  }
  /** Latest viewport and native scan metadata. */
  get snapshot(): FileCollectionSnapshot {
    return this.store.current;
  }
  /** Only the loaded window, never the whole dataset. */
  get items(): readonly FileEntry[] {
    return this.snapshot.items;
  }
  /** Native view identity; await ready first. */
  get id(): string {
    this.assert();
    if (!this.view)
      failure("view_not_ready", "Native view has not been created");
    return this.view;
  }
  /** Await scan completion, returning metadata without copying all discovered entries. */
  get loaded(): Promise<FileCollectionSnapshot> {
    return new Promise((resolve, reject) => {
      let stop = () => {};
      let settled = false;
      stop = this.subscribe((snapshot) => {
        if (snapshot.state === "loading") return;
        settled = true;
        stop();
        if (snapshot.state === "ready") resolve(snapshot);
        else
          reject(
            snapshot.error ??
              new CapabilityError("view_disposed", "Native view was disposed"),
          );
      });
      if (settled) stop();
    });
  }

  /** Reject use after either view or scope disposal. */
  assert(): void {
    this.owner.assert();
    if (this.disposed)
      failure("view_disposed", "Native view has been disposed");
  }

  /** Hold old row ownership until a native selection has acknowledged its leases. */
  protectHandles(capture: Promise<void>): void {
    this.captures.add(capture);
    void capture.finally(() => this.captures.delete(capture)).catch(() => {});
  }

  private accept(snapshot: FileCollectionSnapshot): void {
    if (this.disposed || snapshot.generation !== this.generation) return;
    if (snapshot.view !== this.lastSnapshotView) {
      this.lastSnapshotView = snapshot.view;
      this.lastSnapshotRevision = -1;
    }
    if (snapshot.revision <= this.lastSnapshotRevision) return;
    this.lastSnapshotRevision = snapshot.revision;
    // An ACK can revoke prior rows. Never keep prior handles under metadata for
    // another viewport; wait for a page that actually covers the requested one.
    const matches = snapshot.offset === this.desiredOffset;
    this.store.set({
      ...snapshot,
      items: matches
        ? Object.freeze([...snapshot.items].slice(0, this.desiredLimit))
        : [],
      offset: this.desiredOffset,
      warnings: Object.freeze([...snapshot.warnings]),
    });
  }

  private receive(
    snapshot: FileCollectionSnapshot,
    current = () => true,
  ): void {
    // Defer past the invoke continuation: initial ready must resolve before a
    // selection capture that awaits it can satisfy the page's release barrier.
    const received = Promise.resolve().then(() => {
      const prior = [...this.captures];
      if (current()) this.accept(snapshot);
      // Synchronous store consumers may capture rows while accepting a page.
      return [...new Set([...prior, ...this.captures])];
    });
    this.acknowledged = this.acknowledged.catch(() => {}).then(async () => {
      const barriers = await received;
      await Promise.allSettled(barriers);
      // ACK is void, but can synchronously publish the latest coalesced Channel
      // page. Stale generations/windows still ACK their own native revision.
      await this.owner.transport.dispatch<void>({
        action: "view.ack",
        scope: await this.owner.ready,
        view: snapshot.view,
        revision: snapshot.revision,
      });
    }).catch((error) => {
      this.acknowledgementError = errorOf(error, "view_ack_failed");
      if (!this.disposed) this.store.set({
        ...this.snapshot, state: "failed", error: this.acknowledgementError,
      });
      throw error;
    });
    void received.catch(() => {});
    void this.acknowledged.catch(() => {});
  }

  private async drainAcknowledgements(): Promise<void> {
    let acknowledgements: Promise<void>;
    do {
      acknowledgements = this.acknowledged;
      await acknowledgements;
    } while (acknowledgements !== this.acknowledged);
    if (this.acknowledgementError) throw this.acknowledgementError;
  }

  private retire(view: string): void {
    // Replacing a recursive index can return another view identity. Cleanup
    // must not block query readiness, which pending capture barriers may await.
    const windows = this.windowWork;
    const cleanup = windows.catch(() => {}).then(async () => {
      await this.drainAcknowledgements().catch(() => {});
      await this.owner.transport.dispatch<void>({
        action: "view.dispose", scope: await this.owner.ready, view,
      });
    });
    this.retiredViews.add(cleanup);
    void cleanup.then(() => this.retiredViews.delete(cleanup), (error) => {
      if (!this.disposed) this.store.set({
        ...this.snapshot, state: "failed", error: errorOf(error),
      });
    });
  }

  private async submitQuery(
    generation: number,
    options: FileQueryOptions,
  ): Promise<void> {
    let response: ViewResponse;
    try {
      response = await this.owner.request<ViewResponse, FileCollectionSnapshot>(
        {
          action: this.view ? "view.query" : "folder.query",
          ...(this.view ? { view: this.view } : { folder: this.folder }),
          options,
          generation,
          offset: this.desiredOffset,
          limit: this.desiredLimit,
        },
        (snapshot) => {
          this.receive(snapshot);
        },
      );
      const previous = this.view;
      this.view = response.view;
      this.receive(response.snapshot);
      if (previous && previous !== response.view) {
        this.retire(previous);
      }
    } catch (error) {
      if (!this.disposed && generation === this.generation) {
        this.store.set({
          ...this.snapshot,
          state: "failed",
          error: errorOf(error),
        });
      }
      throw error;
    }
  }

  /** Request a bounded viewport; defaults to 128 and rejects limits over 512. */
  window(offset: number, limit = 128): Promise<void> {
    this.assert();
    const bounds = windowBounds(offset, limit);
    const revision = ++this.windowRevision;
    const generation = this.generation;
    this.desiredOffset = offset;
    this.desiredLimit = limit;
    this.store.set({ ...this.snapshot, offset, items: [] });
    // Capture readiness before queuing so later queries cannot retarget this call.
    const viewReady = this.ready;
    const captures = [...this.captures];
    this.windowWork = this.windowWork.catch(() => {}).then(async () => {
      await viewReady;
      this.assert();
      if (revision !== this.windowRevision || generation !== this.generation) return;
      await Promise.allSettled(captures);
      // Native window admission rejects an outstanding page with window_pending.
      // Include any coalesced Channel page published by an ACK in this drain.
      await this.drainAcknowledgements();
      this.assert();
      if (revision !== this.windowRevision || generation !== this.generation) return;
      const snapshot = await this.owner.request<FileCollectionSnapshot>({
        action: "view.window", view: this.id, generation, ...bounds,
      });
      this.receive(snapshot, () => revision === this.windowRevision);
    }).catch((error) => {
      if (!this.disposed && revision === this.windowRevision && generation === this.generation) {
        this.store.set({ ...this.snapshot, state: "failed", error: errorOf(error) });
      }
      throw error;
    });
    void this.windowWork.catch(() => {});
    return this.windowWork;
  }

  /** Replace native filter/sort options. Selection generations change immediately. */
  query(options: FileQueryOptions): Promise<void> {
    this.assert();
    const generation = ++this.generation;
    this.windowRevision++;
    this.options = { ...options };
    this.desiredOffset = 0;
    this.store.set({
      ...this.snapshot,
      generation,
      offset: 0,
      items: [],
      state: "loading",
      error: undefined,
    });
    const previous = this.queryWork;
    const captures = [...this.captures];
    this.queryWork = previous
      .catch(() => {})
      .then(async () => {
        this.assert();
        // Rapid edits coalesce before submission; native stale generations are never reinstated.
        if (generation !== this.generation) return;
        return Promise.allSettled(captures).then(() =>
          this.submitQuery(generation, { ...options }),
        );
      });
    void this.queryWork.catch(() => {});
    return this.queryWork;
  }

  /** Change search using native filtering, preserving other current options. */
  filter(search: string): Promise<void> {
    return this.query({ ...this.options, search });
  }
  /** Change native ordering, preserving search and recursive traversal. */
  sort(
    sort: NonNullable<FileQueryOptions["sort"]>,
    descending = false,
  ): Promise<void> {
    return this.query({ ...this.options, sort, descending });
  }
  /** Create a bounded explicit selection that persists across viewport changes. */
  selection(): FileSelection {
    this.assert();
    return new FileSelection(this);
  }
  /** Select every native match, including matches discovered after this call. */
  allMatching(): AllMatchingSelection {
    this.assert();
    return new AllMatchingSelection(this);
  }

  /** Capture this query's native selector; a subsequent query invalidates pending submission. */
  async selector(generation: number): Promise<NativeSelection> {
    await this.ready;
    this.assert();
    if (generation !== this.generation)
      failure("stale_selection", "The selected native query has changed");
    return { view: this.id, allMatching: true, generation };
  }

  /** Dispose view ownership; late creation acknowledgements are reclaimed too. */
  dispose(): Promise<void> {
    if (this.cleanup) return this.cleanup;
    this.disposed = true;
    this.released?.();
    this.store.set({ ...this.snapshot, items: [], state: "disposed" });
    this.store.clear();
    this.owner.release(this);
    this.cleanup = (async () => {
      await Promise.allSettled([...this.captures]);
      try {
        await this.queryWork;
      } catch {
        /* Creation failures acquire no view. */
      }
      await this.windowWork.catch(() => {});
      // ACK can publish one more coalesced replacement. Drain that work before
      // revoking the view, so disposal cannot strand a received page revision.
      await this.drainAcknowledgements().catch(() => {});
      const retired = await Promise.allSettled([...this.retiredViews]);
      if (this.view)
        await this.owner.transport.dispatch({
          action: "view.dispose",
          scope: await this.owner.ready,
          view: this.view,
        });
      const failed = retired.find((result) => result.status === "rejected");
      if (failed?.status === "rejected") throw failed.reason;
      if (this.acknowledgementError) throw this.acknowledgementError;
    })();
    return this.cleanup;
  }
}

function handleKey(handle: Handle): string {
  return JSON.stringify([
    handle.id,
    handle.scope,
    handle.generation,
    handle.kind,
  ]);
}

/** Bounded explicit selection persistence state. Native acknowledgement owns file leases. */
export interface SelectionStatus {
  /** Native selection capture lifecycle, including failure and disposal. */
  state: "saving" | "ready" | "failed" | "disposed";
  /** Bounded number of explicitly selected files, at most 512. */
  count: number;
  /** Optional classified native capture or batch-admission failure. */
  error?: RuntimeError;
}

/** Explicit selection with serialized native lease capture. Scrolling cannot invalidate its inputs. */
export class FileSelection implements Readable<readonly FileEntry[]> {
  private store = new Signal<readonly FileEntry[]>([]);
  private stop: () => void;
  private disposed = false;
  private generation: number;
  private nativeId?: string;
  private nativeView?: string;
  private work: Promise<void> = Promise.resolve();
  private revision = 0;
  private cleanup?: Promise<void>;
  /** Current native capture status, including structured failures. */
  readonly status = new Signal<SelectionStatus>({ state: "saving", count: 0 });
  /** Store of explicit file metadata; at most 512 selected entries. */
  readonly subscribe = this.store.subscribe;

  /** Own a bounded explicit selection and serialize its native lease captures. */
  constructor(private collection: FileCollection) {
    this.generation = collection.snapshot.generation;
    collection.owner.own(this);
    this.stop = collection.subscribe((snapshot) => {
      if (snapshot.state === "disposed") void this.dispose().catch(() => {});
      else if (snapshot.generation !== this.generation) {
        this.generation = snapshot.generation;
        void this.set([]).catch(() => {});
      }
    });
    void this.set([]).catch(() => {});
  }
  /** Latest capture acknowledgement; run() waits for the version present when called. */
  get ready(): Promise<void> {
    return this.work;
  }
  /** Selected bounded metadata; native selection owns the corresponding file leases. */
  get items(): readonly FileEntry[] {
    return this.store.current;
  }
  private assert(): void {
    this.collection.assert();
    if (this.disposed)
      failure("selection_disposed", "Selection has been disposed");
  }
  /** Replace selection. Remote capture is serialized and pins the old viewport until acknowledged. */
  set(items: readonly FileEntry[]): Promise<void> {
    this.assert();
    const allowed = new Set(
      [...this.collection.items, ...this.items].map((entry) =>
        handleKey(entry.handle),
      ),
    );
    const unique = new Map(items.map((entry) => [entry.relativePath, entry]));
    if (unique.size > 512)
      failure(
        "selection_limit",
        "Explicit selections are bounded to 512 files; use allMatching()",
      );
    if (
      [...unique.values()].some(
        (entry) => !allowed.has(handleKey(entry.handle)),
      )
    ) {
      failure("invalid_selection", "Entries must belong to this native view");
    }
    const entries = Object.freeze([...unique.values()]);
    const handles = entries.map((entry) => structuredClone(entry.handle));
    const revision = ++this.revision;
    // Capture readiness now: a later query may itself wait for this lease acknowledgement.
    const viewReady = this.collection.ready;
    this.store.set(entries);
    this.status.set({ state: "saving", count: entries.length });
    this.work = this.work
      .catch(() => {})
      .then(async () => {
        await viewReady;
        if (this.disposed) return;
        const view = this.collection.id;
        const response =
          await this.collection.owner.request<SelectionConnection>({
            action: "selection.set",
            view,
            // Query replacement creates a new native view. Its selection must
            // be captured anew; old-view leases are disposed after admission.
            ...(this.nativeId && this.nativeView === view
              ? { selection: this.nativeId }
              : {}),
            handles,
          });
        const previous = this.nativeId;
        this.nativeId = response.selection;
        this.nativeView = view;
        if (previous && previous !== response.selection) {
          await this.collection.owner.transport.dispatch({
            action: "selection.dispose",
            scope: await this.collection.owner.ready,
            id: previous,
          });
        }
        if (!this.disposed && revision === this.revision)
          this.status.set({ state: "ready", count: response.count });
      })
      .catch((error) => {
        if (!this.disposed && revision === this.revision) {
          this.status.set({
            state: "failed",
            count: entries.length,
            error: errorOf(error),
          });
        }
        throw error;
      });
    this.collection.protectHandles(this.work);
    void this.work.catch(() => {});
    return this.work;
  }
  /** Toggle by full capability identity and acknowledge native lease ownership. */
  toggle(entry: FileEntry): Promise<void> {
    return this.set(
      this.has(entry)
        ? this.items.filter((item) => item.relativePath !== entry.relativePath)
        : [...this.items, entry],
    );
  }
  /** Whether this view-relative row is selected; native admission still uses its full capability. */
  has(entry: FileEntry): boolean {
    return this.items.some((item) => item.relativePath === entry.relativePath);
  }
  /** Clear local items and native selected ownership. */
  clear(): Promise<void> {
    return this.set([]);
  }
  /** Select the complete current native query after checking selection and scope ownership. */
  allMatching(): AllMatchingSelection {
    this.assert();
    return this.collection.allMatching();
  }
  /** Run captured native inputs. Later edits wait until this batch's admission freezes them. */
  run<O>(operation: Operation<FileEntry, O>): Task<BatchResult<O>> {
    this.assert();
    const captured = this.work;
    const generation = this.generation;
    const task = operation
      .forScope(this.collection.owner)
      .runSelection(async () => {
        await captured;
        this.assert();
        if (generation !== this.generation)
          failure(
            "stale_selection",
            "Selected query changed before batch admission",
          );
        if (!this.nativeId)
          failure(
            "selection_not_ready",
            "Native selection has not acknowledged ownership",
          );
        return { id: this.nativeId };
      });
    // A subsequent set cannot mutate native inputs while the batch request is in flight.
    this.work = captured.then(() => task.ready);
    this.collection.protectHandles(this.work);
    void this.work.catch(() => {});
    return task;
  }
  /** Release native captured inputs after any pending capture/admission acknowledgement. */
  dispose(): Promise<void> {
    if (this.cleanup) return this.cleanup;
    this.disposed = true;
    this.stop?.();
    this.store.set([]);
    this.store.clear();
    this.status.set({ state: "disposed", count: 0 });
    this.collection.owner.release(this);
    this.cleanup = (async () => {
      try {
        await this.work;
      } catch {
        /* Failed captures acquired no new selection. */
      }
      if (this.nativeId)
        await this.collection.owner.transport.dispatch({
          action: "selection.dispose",
          scope: await this.collection.owner.ready,
          id: this.nativeId,
        });
    })();
    return this.cleanup;
  }
}

/** Native all-matching selector; the host awaits scanning before freezing execution input. */
export class AllMatchingSelection {
  private generation: number;
  /** Capture the current query generation for native all-matching batch admission. */
  constructor(private collection: FileCollection) {
    this.generation = collection.snapshot.generation;
  }
  /** Submit a native view selector without enumerating handles in the webview. */
  run<O>(operation: Operation<FileEntry, O>): Task<BatchResult<O>> {
    this.collection.assert();
    // Resolve against readiness captured now. A later query waits for admission;
    // resolving the selector against that later ready would create a cycle.
    const selector = this.collection.selector(this.generation);
    void selector.catch(() => {});
    const task = operation
      .forScope(this.collection.owner)
      .runSelection(() => selector);
    // Keep the query stable until native batch admission freezes its inputs.
    this.collection.protectHandles(task.ready);
    return task;
  }
}
