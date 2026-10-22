/** Generated operations submit native contracts. JavaScript never implements execution providers. */
import { cloneJson, failure } from "./contracts.js";
import type { OperationDefinition } from "./contracts.js";
import { validator } from "./schema.js";
import { BatchResult, Task } from "./task.js";
import type { Scope } from "./scope.js";
import { Query } from "./query.js";

/** Infer an operation's compiled input without repeating its wire contract. */
export type OperationInput<T> = T extends Operation<infer I, infer _O, infer _U> ? I : T extends OperationFunction<infer I, infer _O, infer _U> ? I : never;
/** Infer an operation's compiled result without nesting Promise and method utility types. */
export type OperationOutput<T> = T extends Operation<infer _I, infer O, infer _U> ? O : T extends OperationFunction<infer _I, infer O, infer _U> ? O : never;
/** Infer an operation's compiled application update type. */
export type OperationUpdates<T> = T extends Operation<infer _I, infer _O, infer U> ? U : T extends OperationFunction<infer _I, infer _O, infer U> ? U : never;
/** Captured native files, native all-matching view, or a small JSON application-record batch. */
export type NativeSelection = {
  /** Captured native selection identity whose leases are frozen at task admission. */
  id: string;
} | NativeSelectionMatching | {
  /** At most 512 JSON records and one MiB of encoded input values. */
  inputs: unknown[];
};
/** Host-owned query selection; generation is checked atomically at batch admission. */
export interface NativeSelectionMatching {
  /** Native view whose query is frozen for all-matching execution. */
  view: string;
  /** Select every native query match without transferring an inventory to JavaScript. */
  allMatching: true;
  /** Optional expected client query generation, checked at native batch admission. */
  generation?: number;
}
/** Typed generated operation. Host execution owns leases, scheduling and cancellation. */
export class Operation<I, O, U = never> {
  private registration?: OperationDefinition;
  private checkInput?: (value: unknown) => void;
  private checkOutput?: (value: unknown) => void;
  private checkUpdate?: (value: unknown) => void;
  private operationId: string;
  /** Bind a compiled registration or lazy manifest resolver to native task submission. */
  constructor(definition: OperationDefinition | string,
    /** Native scope used to admit tasks submitted through this binding. */
    readonly owner: Scope, private prepare: () => Promise<OperationDefinition | void>) {
    this.operationId =
      typeof definition === "string" ? definition : definition.id;
    if (typeof definition !== "string")
      this.install(definition);
  }
  private install(definition: OperationDefinition): void {
    this.registration = definition;
    this.checkInput = validator(definition.input.schema);
    this.checkOutput = validator(definition.output.schema);
    this.checkUpdate = definition.updates ? validator(definition.updates.schema) : undefined;
  }
  private async connected(): Promise<void> {
    const definition = await this.prepare();
    if (!this.registration && definition)
      this.install(definition);
    if (!this.registration)
      failure("unknown_operation", `Native operation ${this.id} has no compiled contract`);
  }
  /** Compiled registration. Lazy built-ins expose it after connection and first submission. */
  get definition(): OperationDefinition {
    if (!this.registration)
      failure("operation_not_ready", `Native operation ${this.id} is still connecting`);
    return this.registration;
  }
  /** Stable native registration id, available before root readiness. */
  get id(): string {
    return this.operationId;
  }
  /** Submit immediately; root readiness and compiled input validation are awaited automatically. */
  run(input: I): Task<O, U> {
    this.owner.assert();
    const value = cloneJson(input);
    this.checkInput?.(value);
    return new Task<O, U>(this.owner, { action: "task.run", operation: this.id, input: value }, async () => {
      await this.connected();
      this.checkInput!(value);
    }, (snapshot) => {
      this.checkOutput!(snapshot.result);
      return snapshot.result as O;
    }, value => {
      if (!this.checkUpdate) failure("task_updates_undeclared", "Operation has no compiled update contract");
      this.checkUpdate(value);
    });
  }
  /** Await one result and release native task ownership after consumption. */
  async call(input: I): Promise<O> {
    const task = this.run(input);
    try {
      return await task.result;
    }
    finally {
      await task.dispose();
    }
  }
  /** Own an explicitly loaded, latest-result read projection in this operation's scope. */
  query(): Query<I, O, U> {
    if (this.definition.effect !== "readOnly")
      failure("query_not_read_only", `Operation ${this.id} is not a declared read`);
    return new Query(this);
  }
  /** Submit captured native inputs or an all-matching view without copying the file dataset. */
  runSelection(selection: () => Promise<NativeSelection>): Task<BatchResult<O>> {
    this.owner.assert();
    return new Task<BatchResult<O>>(this.owner, async () => ({
      action: "task.batch",
      operation: this.id,
      selection: await selection(),
    }), () => this.connected(), (_snapshot, task) => new BatchResult<O>(task as Task<unknown>, this.checkOutput));
  }
  /** Run a small JSON-record batch, bounded to 512 records and 1 MiB encoded input. */
  runBatch(inputs: readonly I[]): Task<BatchResult<O>> {
    this.owner.assert();
    if (inputs.length > 512)
      failure("batch_limit", "JSON batches allow at most 512 inputs; use native allMatching for files");
    const values = cloneJson([...inputs]);
    for (const input of values) {
      this.checkInput?.(input);
    }
    if (new TextEncoder().encode(JSON.stringify(values)).byteLength >
      1024 * 1024) {
      failure("batch_limit", "JSON batches allow at most 1 MiB of encoded inputs");
    }
    return this.runSelection(async () => {
      for (const value of values)
        this.checkInput!(value);
      return { inputs: values };
    });
  }
  /** Rebind the same compiled registration to another scope of this native transport. */
  forScope(owner: Scope): Operation<I, O, U> {
    if (owner.transport !== this.owner.transport)
      failure("scope_mismatch", "Operation belongs to another native connection");
    return new Operation(this.registration ?? this.id, owner, this.prepare);
  }
}
/** A generated function retaining explicit task control beneath direct calls. */
export interface OperationFunction<I, O, U = never> {
  /** Await the result and release task ownership after consumption. */
  (input: I): Promise<O>;
  /** Keep the task receipt for progress, cancellation and explicit disposal. */
  run(input: I): Task<O, U>;
}
/** Query helpers exist only on operations explicitly declared read_only in Rust. */
export interface ReadOperationFunction<I, O, U = never> extends OperationFunction<I, O, U> {
  /** Scope of this declared read binding; query groups reject unrelated owners. */
  readonly owner: Scope;
  /** Rebind a declared read to an owned child scope on the same connection. */
  forScope(owner: Scope): ReadOperationFunction<I, O, U>;
  /** Own a latest-result projection for an explicitly declared read. */
  query(): Query<I, O, U>;
}
/** Attach callable helpers to a component-owned compiled binding. */
export function callableOperation<I, O, U = never>(operation: Operation<I, O, U>, readOnly: true): ReadOperationFunction<I, O, U>;
/** Bind a callable operation without a read projection. */
export function callableOperation<I, O, U = never>(operation: Operation<I, O, U>, readOnly: false): OperationFunction<I, O, U>;
/** Infer query availability from the operation's authoritative metadata. */
export function callableOperation<I, O, U = never>(operation: Operation<I, O, U>): OperationFunction<I, O, U> | ReadOperationFunction<I, O, U>;
/** Bind a callable function using the compiled read policy. */
export function callableOperation<I, O, U = never>(operation: Operation<I, O, U>, readOnly?: boolean): OperationFunction<I, O, U> | ReadOperationFunction<I, O, U> {
  if (readOnly === true && operation.definition.effect !== "readOnly")
    failure("query_not_read_only", `Operation ${operation.id} is not a declared read`);
  const callable: OperationFunction<I, O, U> = Object.assign((input: I) => operation.call(input), {
    run: (input: I) => operation.run(input),
  });
  return (readOnly ?? operation.definition.effect === "readOnly")
    ? Object.assign(callable, {
      owner: operation.owner,
      forScope: (owner: Scope) => callableOperation(operation.forScope(owner), true),
      query: () => operation.query(),
    }) : callable;
}
/** Bind a registration from the generated manifest without redeclaring schemas. */
export function operationFactory<I, O, U = never>(manifest: {
  /** Compiled operation registrations from the generated application manifest. */
  operations: OperationDefinition[];
}, id: string, bind: (definition: OperationDefinition) => Operation<I, O, U>): Operation<I, O, U> {
  const definition = manifest.operations.find((operation) => operation.id === id);
  if (!definition)
    failure("unknown_operation", `Operation ${id} is absent from the generated manifest`);
  return bind(definition);
}
