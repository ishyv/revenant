/** App roots connect once. Child projections share contracts but own native resources separately. */
import { errorOf, failure } from "./contracts.js";
import type {
  ContractManifest,
  OperationDefinition,
  RuntimeError,
  FileEntry,
} from "./contracts.js";
import { Collection } from "./collection.js";
import { Files } from "./files.js";
import { Media } from "./media.js";
import { Operation } from "./operation.js";
import { Runtime } from "./runtime.js";
import { Scope } from "./scope.js";
import { SettingsFactory } from "./settings.js";
import { Signal } from "./store.js";
import { sameContract, validateManifest } from "./schema.js";
import { TauriTransport } from "./transport.js";
import type { NativeTransport } from "./transport.js";

/** Generated facades supply manifest; transport can be injected for native host integration. */
export interface AppOptions {
  /** Expected compiled host contract; connection rejects an incompatible manifest. */
  manifest?: ContractManifest;
  /** Native IPC boundary; defaults to the Tauri dispatcher. */
  transport?: NativeTransport;
}
/** Root connection status. */
export interface AppStatus {
  /** Shared root connection lifecycle, independent of component subscriptions. */
  state: "ready" | "loading" | "failed" | "disposed";
  /** Classified connection failure when root startup fails. */
  error?: RuntimeError;
}
/** Shared root connection state used by low-level application construction. */ interface Shared {
  /** Root owner shared by all scoped application facades. */
  root: Scope;
  /** Validated compiled manifest resolved by the root connection. */
  ready: Promise<ContractManifest>;
  /** Connection status shared across parent and child facades. */
  status: Signal<AppStatus>;
  /** Expected or connected compiled manifest used for operation binding. */
  manifest?: ContractManifest;
}

/** Scoped desktop capability facade. Resource lifetimes follow the scope, not UI subscriptions. */
export class App<Checksum = unknown, Metadata = unknown> {
  /** Native directory picker and indexed folder views. */
  readonly files: Files<Checksum>;
  /** Native preview resources. */
  readonly media: Media<Metadata>;
  /** Native atomic settings persistence. */
  readonly settings: SettingsFactory;
  /** Inspection and transactional native provider changes. */
  readonly runtime: Runtime;
  /** Automatically awaited by operations; useful explicitly for startup UI. */
  readonly ready: Promise<ContractManifest>;
  /** Root connection status shared by every child. */
  readonly status: Signal<AppStatus>;

  /** Bind capability facades to an existing native scope and shared root connection; prefer createApp. */
  constructor(
    /** Native resource owner used by every capability of this application facade. */
    readonly scope: Scope,
    private shared: Shared,
  ) {
    this.ready = shared.ready;
    this.status = shared.status;
    this.files = new Files(
      scope,
      this.operation<FileEntry, Checksum>("files.checksum"),
    );
    this.settings = new SettingsFactory(scope);
    this.media = new Media(
      scope,
      this.operation<FileEntry, Metadata>("media.readMetadata"),
    );
    this.runtime = new Runtime(scope);
  }

  /** Construct a small in-memory collection. File datasets use Folder.files instead. */
  collection<T>(items: readonly T[] = []): Collection<T> {
    this.scope.assert();
    return new Collection(this.scope, items);
  }

  /** Bind by id, resolving its compiled schema automatically once the root connects. */
  operation<I, O>(id: string): Operation<I, O> {
    this.scope.assert();
    const definition = this.shared.manifest?.operations.find(
      (operation) => operation.id === id,
    );
    if (definition) return this.bindOperation(definition);
    return new Operation(id, this.scope, async () => {
      const manifest = await this.ready;
      const compiled = manifest.operations.find((entry) => entry.id === id);
      if (!compiled)
        failure(
          "unknown_operation",
          `Operation ${id} is absent from the native manifest`,
        );
      return compiled;
    });
  }

  /** Bind a generated registration now; validate against the connected host before execution. */
  bindOperation<I, O>(definition: OperationDefinition): Operation<I, O> {
    this.scope.assert();
    return new Operation(definition, this.scope, async () => {
      const manifest = await this.ready;
      const expected = manifest.operations.find(
        (operation) => operation.id === definition.id,
      );
      if (!expected || !sameContract(expected, definition)) {
        failure(
          "incompatible_contract",
          "Generated operation differs from the native manifest",
          definition.id,
        );
      }
    });
  }

  /** Create a child facade immediately; native calls await scope creation. */
  createScope(): App<Checksum, Metadata> {
    this.scope.assert();
    return new App<Checksum, Metadata>(this.scope.child(), this.shared);
  }
  /** Cleanup failures for this scope. */
  get cleanupErrors(): Scope["cleanupErrors"] {
    return this.scope.cleanupErrors;
  }
  /** Begin scope teardown; root teardown also closes application status. */
  dispose(): void {
    this.scope.dispose();
    if (this.scope === this.shared.root) this.status.set({ state: "disposed" });
  }
  /** Await resource and native scope teardown. */
  async disposeAsync(): Promise<void> {
    this.dispose();
    await this.scope.disposeAsync();
  }
}

/** Connect to the desktop host. There is no browser, worker, or HTTP fallback. */
export function createApp<Checksum = unknown, Metadata = unknown>(
  options: AppOptions = {},
): App<Checksum, Metadata> {
  const transport = options.transport ?? new TauriTransport();
  const expected = options.manifest
    ? validateManifest(options.manifest)
    : undefined;
  const status = new Signal<AppStatus>({ state: "loading" });
  const connection = transport.dispatch<{
    scope: string;
    manifest: ContractManifest;
  }>({ action: "root.connect" });
  const validated = connection.then((connected) => {
    const loaded = validateManifest(connected.manifest);
    if (
      expected &&
      (loaded.digest !== expected.digest ||
        loaded.operations.length !== expected.operations.length ||
        !loaded.operations.every((operation) =>
          expected.operations.some((entry) => sameContract(entry, operation)),
        ))
    ) {
      failure(
        "incompatible_contract",
        "Connected host differs from the generated application manifest",
      );
    }
    return loaded;
  });
  const root = new Scope(
    transport,
    connection.then((connected) => connected.scope),
    undefined,
    validated,
  );
  const ready = validated
    .then((loaded) => {
      shared.manifest = loaded;
      if (!root.disposed) status.set({ state: "ready" });
      return loaded;
    })
    .catch((error) => {
      if (!root.disposed)
        status.set({
          state: "failed",
          error: errorOf(error, "native_startup_failed"),
        });
      root.dispose();
      throw error;
    });
  const shared: Shared = { root, status, ready, manifest: expected };
  void ready.catch(() => {});
  return new App<Checksum, Metadata>(root, shared);
}
