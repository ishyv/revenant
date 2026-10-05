/** Settings are local observable projections of native atomic persistence. */
import { assertJson, errorOf, failure } from "./contracts.js";
import type { RuntimeError } from "./contracts.js";
import { Signal } from "./store.js";
import type { Readable } from "./store.js";
import type { Scope } from "./scope.js";

/** Native persistence state; failed saves keep the latest local value for retry. */
export interface PersistenceStatus {
  /** Native load and save lifecycle; failed saves preserve the local value. */
  state: "loading" | "dirty" | "saving" | "saved" | "failed" | "disposed";
  /** Optional classified native loading or atomic persistence failure. */
  error?: RuntimeError;
}
/** Native key names are scoped to the application by the desktop host. */
export interface SettingsOptions {
  /** Application settings key; native code owns its storage location. */
  key: string;
}

function compatible(defaults: unknown, value: unknown): boolean {
  if (defaults === null) return value === null;
  if (Array.isArray(defaults))
    return (
      Array.isArray(value) &&
      (defaults.length === 0 ||
        value.every((item) => compatible(defaults[0], item)))
    );
  if (typeof defaults === "object") {
    if (!value || typeof value !== "object" || Array.isArray(value))
      return false;
    const properties = Object.entries(defaults as object);
    return (
      Object.keys(value).length === properties.length &&
      properties.every(
        ([key, expected]) =>
          key in value &&
          compatible(expected, (value as Record<string, unknown>)[key]),
      )
    );
  }
  return typeof defaults === typeof value;
}
function freeze<T>(value: T): T {
  if (value && typeof value === "object") {
    Object.values(value).forEach(freeze);
    Object.freeze(value);
  }
  return value;
}

/** Typed settings store. Writes serialize and coalesce while preserving load/edit races. */
export class Settings<T extends Record<string, unknown>>
  implements Readable<T>
{
  private store: Signal<T>;
  private defaults: T;
  private revision = 0;
  private savedRevision = 0;
  private disposed = false;
  private saving?: Promise<void>;
  /** Load promise; set() can be called before it resolves without losing edits. */
  readonly ready: Promise<void>;
  /** Observable native persistence status. */
  readonly status = new Signal<PersistenceStatus>({ state: "loading" });
  /** Svelte value subscription. */
  readonly subscribe: Readable<T>["subscribe"];

  /** Publish typed defaults while loading the application settings key from native storage. */
  constructor(
    private owner: Scope,
    defaults: T,
    private options: SettingsOptions,
  ) {
    assertJson(defaults);
    if (!options.key)
      failure("invalid_settings_key", "Native settings require a nonempty key");
    this.defaults = structuredClone(defaults);
    this.store = new Signal(freeze(structuredClone(defaults)));
    this.subscribe = this.store.subscribe;
    owner.own(this);
    this.ready = owner
      .request<T>({
        action: "settings.load",
        key: options.key,
        defaults: this.defaults,
      })
      .then((value) => {
        if (this.disposed) return;
        this.validate(value);
        if (this.revision === 0) this.store.set(freeze(structuredClone(value)));
        this.status.set({ state: this.revision ? "dirty" : "saved" });
      })
      .catch((error) => {
        if (!this.disposed)
          this.status.set({
            state: "failed",
            error: errorOf(error, "persistence_failed"),
          });
        throw error;
      });
    void this.ready.catch(() => {});
  }

  /** Defensive copy of the latest local value. */
  get value(): T {
    return structuredClone(this.store.current);
  }
  private validate(value: unknown): void {
    assertJson(value);
    if (!compatible(this.defaults, value))
      failure("invalid_settings", "Settings value does not match its defaults");
  }
  private assert(): void {
    this.owner.assert();
    if (this.disposed)
      failure("settings_disposed", "Settings have been disposed");
  }
  /** Update immediately and automatically queue native persistence. */
  set(value: T): void {
    this.assert();
    this.validate(value);
    this.revision++;
    this.store.set(freeze(structuredClone(value)));
    this.status.set({ state: "dirty" });
    void this.flush().catch(() => {});
  }
  /** Transform a defensive copy of the latest local settings. */
  update(change: (value: T) => T): void {
    this.set(change(this.value));
  }
  /** Await serialized native atomic saves, including edits made during a save. */
  flush(): Promise<void> {
    this.assert();
    if (this.saving) return this.saving;
    this.saving = (async () => {
      await this.ready;
      const scope = await this.owner.ready;
      while (this.savedRevision < this.revision) {
        const revision = this.revision;
        const value = structuredClone(this.store.current);
        if (!this.disposed) this.status.set({ state: "saving" });
        // Parent teardown waits for this save queue before closing the native scope.
        await this.owner.transport.dispatch({
          action: "settings.save",
          scope,
          key: this.options.key,
          value,
        });
        this.savedRevision = revision;
      }
      if (!this.disposed) this.status.set({ state: "saved" });
    })()
      .catch((error) => {
        if (!this.disposed)
          this.status.set({
            state: "failed",
            error: errorOf(error, "persistence_failed"),
          });
        throw error;
      })
      .finally(() => {
        this.saving = undefined;
        // A saved-status subscriber may enqueue one more edit while this promise settles.
        if (
          !this.disposed &&
          this.savedRevision < this.revision &&
          this.status.current.state !== "failed"
        ) {
          void this.flush().catch(() => {});
        }
      });
    return this.saving;
  }
  /** Stop local updates and await an already submitted save before parent teardown. */
  async dispose(): Promise<void> {
    if (this.disposed) return;
    this.disposed = true;
    this.status.set({ state: "disposed" });
    this.store.clear();
    this.owner.release(this);
    await this.saving;
  }
}

/** Scoped factory for settings stores backed by the native host. */
export class SettingsFactory {
  /** Bind typed native settings creation to an application scope. */
  constructor(private owner: Scope) {}
  /** Infer the settings shape from defaults and open the application's native key. */
  define<T extends Record<string, unknown>>(
    defaults: T,
    options: SettingsOptions,
  ): Settings<T> {
    this.owner.assert();
    return new Settings(this.owner, defaults, options);
  }
}
