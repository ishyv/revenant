/** Atomic, explicitly keyed groups of generated declared reads. No database transaction is implied. */
import { cloneJson, errorOf, failure } from './contracts.js';
import type { RuntimeError } from './contracts.js';
import type { QuerySnapshot } from './query.js';
import type { Scope } from './scope.js';
import { Signal } from './store.js';

/** Generated declared-read bindings, limited to sixteen slots per group. */
export type QuerySlots = Record<string, {
  /** Run this declared read once in its bound scope. */
  (input: any): Promise<any>;
  /** Exact scope owning the operation; groups preserve this handle authority. */
  readonly owner: Scope;
  /** Create a replaceable reader whose admitted tasks can be disposed independently. */
  query(): GroupRead;
  /** Bind the operation to an explicit scope without transferring existing handles. */
  forScope(owner: Scope): {
    /** Create a reader bound to the explicitly supplied scope. */
    query(): GroupRead;
  };
}>;
/** Reader lifecycle used to compose and clean up each group slot. */
interface GroupRead {
  /** Load explicitly; failures become snapshot state and settlement includes task cleanup. */
  load(input: any): Promise<void>;
  /** Latest settled or retained result with its classified failure state. */
  readonly snapshot: QuerySnapshot<any>;
  /** Suppress publication and await cancellation and native task disposal. */
  dispose(): Promise<void>;
}
/** Slot configuration shared by the group factory and its constructor. */
interface GroupConfig<R extends QuerySlots, O extends QuerySlots> {
  /** Named declared reads which must all succeed before replacing complete data. */
  required: R;
  /** Named declared reads whose failures omit only their own current result. */
  optional?: O;
}
/** Every declared slot receives its own compiled input. */
export type QueryGroupInputs<R extends QuerySlots, O extends QuerySlots> = {
  [K in keyof R | keyof O]: Parameters<(R & O)[K]>[0];
};
/** Per-load admission policy; skipped optional reads have no current data or error. */
export interface QueryGroupLoadOptions<O extends QuerySlots> {
  /** Omit selected optional reads while keeping every required read admitted. */
  skipOptional?: readonly (keyof O & string)[];
}
/** Required results are complete; failed or skipped optional results are absent for this data key. */
export type QueryGroupData<R extends QuerySlots, O extends QuerySlots> = {
  [K in keyof R]: Awaited<ReturnType<R[K]>>;
} & { [K in keyof O]?: Awaited<ReturnType<O[K]>> };
/** Latest request and retained complete data always carry separate identities. */
export interface QueryGroupSnapshot<R extends QuerySlots, O extends QuerySlots> {
  /** Required-slot failure retains the preceding complete result. */
  status: 'idle' | 'loading' | 'ready' | 'failed';
  /** Explicit key of the latest requested load. */
  requestKey?: string;
  /** Key which produced data, including when it is retained across a failure. */
  dataKey?: string;
  /** At most one complete result, with no previous-key optional values merged in. */
  data?: QueryGroupData<R, O>;
  /** Failures from this request, kept separately for each declared slot. */
  errors: Partial<Record<keyof R | keyof O, RuntimeError>>;
}
interface Load {
  key: string;
  inputs: Record<string, unknown>;
  skipOptional: ReadonlySet<string>;
  generation: number;
  done(): void;
  queries?: GroupRead[];
}

/** Scope-owned composition with one active client load and one replaceable pending load.
 * Superseded native reads may continue until cooperative cancellation is acknowledged.
 */
export class QueryGroup<R extends QuerySlots, O extends QuerySlots = Record<never, never>> {
  private store = new Signal<QueryGroupSnapshot<R, O>>({ status: 'idle', errors: {} });
  private generation = 0;
  private active?: Load;
  private pending?: Load;
  private running?: Promise<void>;
  private cleanup?: Promise<void>;
  private closed = false;
  private slots: Array<[string, QuerySlots[string], boolean]>;
  /** Standard Svelte readable subscription. */
  readonly subscribe = this.store.subscribe;
  /** Slots must be declared reads bound to this exact owner. */
  constructor(private owner: Scope, config: GroupConfig<R, O>) {
    owner.assert();
    this.slots = [
      ...Object.entries(config.required).map(([key, op]) => [key, op, true] as [string, typeof op, boolean]),
      ...Object.entries(config.optional ?? {}).map(([key, op]) => [key, op, false] as [string, typeof op, boolean]),
    ];
    if (!Object.keys(config.required).length || this.slots.length > 16 || new Set(this.slots.map(([key]) => key)).size !== this.slots.length)
      failure('query_group_slots', 'Query groups need required reads and at most sixteen distinct slots');
    for (const [, op] of this.slots) {
      if (typeof op.query !== 'function' || typeof op.forScope !== 'function')
        failure('query_not_read_only', 'Query group slots require declared read operations');
      if (op.owner !== owner) failure('scope_mismatch', 'Query group reads must be bound to its owner');
    }
    owner.own(this);
  }
  /** Latest atomic publication without allocating a subscription. */
  get snapshot(): QueryGroupSnapshot<R, O> { return this.store.current; }
  /** Freeze at most one MiB of inputs and replace pending work. Settlement includes disposal acknowledgements. */
  load(requestKey: string, inputs: QueryGroupInputs<R, O>, options: QueryGroupLoadOptions<O> = {}): Promise<void> {
    this.owner.assert();
    if (this.closed) failure('query_disposed', 'Query group has been disposed');
    if (typeof requestKey !== 'string') failure('query_group_key', 'Query group keys must be strings');
    const frozen = cloneJson(inputs) as Record<string, unknown>;
    if (new TextEncoder().encode(JSON.stringify(frozen)).byteLength > 1024 * 1024)
      failure('query_group_input_limit', 'Query group inputs allow at most one MiB of encoded JSON');
    const names = Object.keys(frozen);
    if (names.length !== this.slots.length || this.slots.some(([key]) => !Object.hasOwn(frozen, key)))
      failure('query_group_inputs', 'Provide exactly one input for every declared slot');
    if (!options || typeof options !== 'object' || Array.isArray(options) || Object.keys(options).some(key => key !== 'skipOptional'))
      failure('query_group_optional', 'Load options may only select optional reads to skip');
    const selection = options.skipOptional === undefined ? [] : options.skipOptional;
    if (!Array.isArray(selection) || selection.length > 16)
      failure('query_group_optional', 'Select at most sixteen optional reads to skip');
    const skipOptional = new Set<string>(selection);
    if (skipOptional.size !== selection.length || [...skipOptional].some(key => typeof key !== 'string' || !this.slots.some(([name, , required]) => name === key && !required)))
      failure('query_group_optional', 'Only distinct declared optional reads may be skipped');
    let done!: () => void;
    const result = new Promise<void>(resolve => { done = resolve; });
    const load: Load = { key: requestKey, inputs: frozen, skipOptional, generation: ++this.generation, done };
    this.pending?.done();
    this.pending = load;
    this.cancelActive();
    this.store.set({ status: 'loading', requestKey, ...this.previous(), errors: {} });
    if (!this.running && !this.closed) {
      // Start in a microtask so reentrant subscribers can replace or close this load.
      this.start();
    }
    return result;
  }
  private start(): void {
    this.running = Promise.resolve().then(() => this.pump()).finally(() => {
      this.running = undefined;
      if (this.pending && !this.closed && !this.owner.disposed) this.start();
    });
  }
  private previous(): Pick<QueryGroupSnapshot<R, O>, 'data' | 'dataKey'> {
    return this.snapshot.dataKey === undefined ? {} : { data: this.snapshot.data, dataKey: this.snapshot.dataKey };
  }
  private cancelActive(): void {
    for (const query of this.active?.queries ?? []) void query.dispose();
  }
  private async pump(): Promise<void> {
    while (this.pending && !this.closed && !this.owner.disposed) {
      const load = this.pending;
      this.pending = undefined;
      this.active = load;
      let queries: Array<[string, GroupRead, boolean]> = [];
      try {
        // Handles remain authoritative only in their original native scope.
        // Per-load query controllers provide cancellation without rebinding.
        load.queries = [];
        queries = this.slots.filter(([key]) => !load.skipOptional.has(key)).map(([key, op, required]) => {
          const query = op.query();
          load.queries!.push(query);
          return [key, query, required];
        });
        await Promise.all(queries.map(([key, query]) => query.load(load.inputs[key])));
        if (!this.closed && !this.owner.disposed && load.generation === this.generation) {
          const data: Record<string, unknown> = {};
          const errors: Record<string, RuntimeError> = {};
          let failed = false;
          for (const [key, query, required] of queries) {
            if (query.snapshot.status === 'ready') Object.defineProperty(data, key, { value: query.snapshot.data, enumerable: true });
            else {
              Object.defineProperty(errors, key, { value: query.snapshot.error ?? errorOf(new Error('Read did not complete')), enumerable: true });
              failed ||= required;
            }
          }
          this.store.set({ status: failed ? 'failed' : 'ready', requestKey: load.key, errors: errors as QueryGroupSnapshot<R, O>['errors'],
            ...(failed ? this.previous() : { data: data as QueryGroupData<R, O>, dataKey: load.key }) });
        }
      } catch (error) {
        if (!this.closed && !this.owner.disposed && load.generation === this.generation)
          this.store.set({ status: 'failed', requestKey: load.key, ...this.previous(),
            errors: Object.fromEntries(this.slots.filter(([key]) => !load.skipOptional.has(key)).map(([key]) => [key, errorOf(error)])) as QueryGroupSnapshot<R, O>['errors'] });
      } finally {
        await Promise.all((load.queries ?? []).map(query => query.dispose()));
        this.active = undefined;
        load.done();
      }
    }
  }
  /** Freeze publication, discard pending work and await disposal acknowledgements, without joining native threads. */
  dispose(): Promise<void> {
    if (this.cleanup) return this.cleanup;
    this.closed = true;
    ++this.generation;
    this.pending?.done(); this.pending = undefined;
    this.cancelActive();
    this.store.clear();
    this.owner.release(this);
    this.cleanup = (this.running ?? Promise.resolve()).then(() => {});
    return this.cleanup;
  }
}
/** Compose required and optional declared reads with explicit request keys. */
export function queryGroup<R extends QuerySlots, O extends QuerySlots = Record<never, never>>(
  owner: Scope, config: GroupConfig<R, O>,
): QueryGroup<R, O> { return new QueryGroup(owner, config); }
