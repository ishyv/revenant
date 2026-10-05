/** Native provider inspection and transactional configuration. No JS execution providers. */
import { assertJson } from "./contracts.js";
import type { Json, JsonSchema } from "./contracts.js";
import { validator } from "./schema.js";
import type { Scope } from "./scope.js";
import { Signal } from "./store.js";
import type { Readable } from "./store.js";

/** Native provider descriptor. Config parameter supplies authoring-time configuration typing. */
export interface ProviderDescriptor<C = Json> {
  /** Stable compiled provider identity used for native configuration. */
  id: string;
  /** Native provider documentation suitable for runtime inspection. */
  description: string;
  /** Compiled capability identities pinned by provider execution. */
  dependencies: string[];
  /** Native configuration schema checked before candidate preparation. */
  configurationSchema: JsonSchema;
  /** Type-only configuration witness; never serialized to the host. */
  readonly configurationType?: C;
}
/** Current native ownership and configuration, matching revenant-sdk field names. */
export interface ProviderSnapshot<C = Json> extends ProviderDescriptor<C> {
  /** Validated configuration of the currently installed native generation. */
  config: C;
  /** Numeric provider generation advanced by a successful replacement transaction. */
  generation: number;
  /** Admitted or executing provider task count as an unsigned decimal string. */
  activeTasks: string;
  /** Owned or leased provider resource count as an unsigned decimal string. */
  resources: string;
  /** Whether the native provider participates in replacement transactions. */
  mutable: boolean;
}

/** Scoped provider projection. Host rejects busy, incompatible or dependency-breaking changes. */
export class Runtime implements Readable<readonly ProviderSnapshot[]> {
  private store = new Signal<readonly ProviderSnapshot[]>([]);
  /** Latest explicitly inspected native provider set. */
  readonly subscribe = this.store.subscribe;
  /** Own an observable projection of explicitly inspected native providers. */
  constructor(private owner: Scope) {
    owner.own(this);
  }
  /** Most recent inspection. Call inspect() to refresh native counts. */
  get snapshot(): readonly ProviderSnapshot[] {
    return this.store.current;
  }
  /** Retrieve provider descriptors, configs and ownership counts from the native registry. */
  async inspect(): Promise<readonly ProviderSnapshot[]> {
    const snapshots = await this.owner.request<ProviderSnapshot[]>({
      action: "runtime.inspect",
    });
    this.owner.assert();
    this.store.set(
      Object.freeze(snapshots.map((snapshot) => Object.freeze(snapshot))),
    );
    return this.snapshot;
  }
  /** Configure a typed descriptor. Native preparation commits atomically or retains the old provider. */
  async configure<C>(
    provider: ProviderDescriptor<C>,
    config: C,
  ): Promise<void> {
    this.owner.assert();
    assertJson(config);
    validator(provider.configurationSchema)(config);
    await this.owner.request({
      action: "runtime.configure",
      provider: provider.id,
      config: structuredClone(config),
    });
    await this.inspect();
  }
  /** Replace with a compiled native provider id; JavaScript callbacks are never accepted. */
  async replace(
    provider: string | ProviderDescriptor,
    replacement: string | ProviderDescriptor,
  ): Promise<void> {
    await this.owner.request({
      action: "runtime.replace",
      provider: typeof provider === "string" ? provider : provider.id,
      replacement:
        typeof replacement === "string" ? replacement : replacement.id,
    });
    await this.inspect();
  }
  /** Release this scope's local provider inspection subscriptions. */
  dispose(): void {
    this.store.set([]);
    this.store.clear();
    this.owner.release(this);
  }
}
