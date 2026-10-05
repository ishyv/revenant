/** Local ownership mirrors native ancestry through asynchronous startup and teardown. */
import { errorOf, failure } from "./contracts.js";
import type { RuntimeError } from "./contracts.js";
import { Signal } from "./store.js";
import type { NativeRequest, NativeTransport } from "./transport.js";

/** Scope resources support deterministic, idempotent cleanup. */
export interface Disposable {
  /** Release local and native ownership; implementations must tolerate repeated calls. */
  dispose(): void | Promise<void>;
}

/** Resource owner. Every capability call automatically awaits its native identity. */
export class Scope implements Disposable {
  private resources = new Set<Disposable>();
  private closed = false;
  private nativeId?: string;
  private cleanup?: Promise<void>;
  /** Cleanup failures remain observable rather than becoming unhandled rejections. */
  readonly cleanupErrors = new Signal<readonly RuntimeError[]>([]);
  /** Resolves after both parent and child have been created on the host. */
  readonly ready: Promise<string>;

  /** Mirror a native owner identity and optional parent; requests await startup prerequisites. */
  constructor(
    /** Native IPC connection used for scope and resource admission. */
    readonly transport: NativeTransport,
    private identity: Promise<string>,
    /** Optional ancestor owner responsible for this scope during teardown. */
    readonly parent?: Scope,
    prerequisite?: Promise<unknown>,
  ) {
    this.ready = identity.then(async (id) => {
      await prerequisite;
      this.nativeId = id;
      return id;
    });
    void this.ready.catch(() => {});
    parent?.own(this);
  }

  /** Host identity; await ready before reading during initialization. */
  get id(): string {
    if (!this.nativeId)
      failure("scope_not_ready", "Native scope is still connecting");
    return this.nativeId;
  }

  /** Local ownership may close before native cleanup completes. */
  get disposed(): boolean {
    return this.closed;
  }

  /** Reject new work after disposal. */
  assert(): void {
    if (this.closed)
      failure("scope_disposed", "This application scope has been disposed");
  }

  /** Register a resource for reverse-order cleanup. */
  own<T extends Disposable>(resource: T): T {
    this.assert();
    this.resources.add(resource);
    return resource;
  }

  /** Forget an already released resource. */
  release(resource: Disposable): void {
    this.resources.delete(resource);
  }

  /** Create a native child; its operations await creation automatically. */
  child(): Scope {
    this.assert();
    const identity = this.ready.then(async (parent) => {
      this.assert();
      return (
        await this.transport.dispatch<{ scope: string }>({
          action: "scope.create",
          parent,
        })
      ).scope;
    });
    return new Scope(this.transport, identity, this);
  }

  /** Dispatch with this scope, checking disposal before and after startup. */
  async request<T, U = T>(
    request: NativeRequest,
    updates?: (snapshot: U) => void,
  ): Promise<T> {
    this.assert();
    const scope = await this.ready;
    this.assert();
    return this.transport.dispatch<T, U>({ ...request, scope }, updates);
  }

  /** Close local ownership synchronously and begin native teardown. */
  dispose(): void {
    if (this.closed) return;
    this.closed = true;
    const pending: Promise<unknown>[] = [];
    for (const resource of [...this.resources].reverse()) {
      try {
        pending.push(
          Promise.resolve(
            resource instanceof Scope
              ? resource.disposeAsync()
              : resource.dispose(),
          ).catch((error) => this.report(error)),
        );
      } catch (error) {
        this.report(error);
      }
    }
    this.resources.clear();
    this.parent?.release(this);
    this.cleanup = (async () => {
      await Promise.all(pending);
      try {
        const scope = await this.identity;
        await this.transport.dispatch({ action: "scope.dispose", scope });
      } catch (error) {
        this.report(error);
      }
    })();
  }

  private report(error: unknown): void {
    this.cleanupErrors.set([
      ...this.cleanupErrors.current,
      errorOf(error, "cleanup_failed"),
    ]);
  }

  /** Wait for resources and native scope cleanup. */
  async disposeAsync(): Promise<void> {
    this.dispose();
    await this.cleanup;
  }
}
