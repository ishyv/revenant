/** Generated operations submit native contracts. JavaScript never implements execution providers. */
import { assertJson, failure } from "./contracts.js";
import type { OperationDefinition } from "./contracts.js";
import { validator } from "./schema.js";
import { BatchResult, Task } from "./task.js";
import type { Scope } from "./scope.js";
import { Query } from "./query.js";

/** Infer an operation's compiled input without repeating its wire contract. */
export type OperationInput<T> = T extends Operation<infer I, infer _O> ? I : never;
/** Infer an operation's compiled result without nesting Promise and method utility types. */
export type OperationOutput<T> = T extends Operation<infer _I, infer O> ? O : never;
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
export class Operation<I, O> {
  private registration?: OperationDefinition;
  private checkInput?: (value: unknown) => void;
  private checkOutput?: (value: unknown) => void;
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
  run(input: I): Task<O> {
    this.owner.assert();
    assertJson(input);
    this.checkInput?.(input);
    const value = structuredClone(input);
    return new Task<O>(this.owner, { action: "task.run", operation: this.id, input: value }, async () => {
      await this.connected();
      this.checkInput!(value);
    }, (snapshot) => {
      this.checkOutput!(snapshot.result);
      return snapshot.result as O;
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
  query(): Query<I, O> {
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
    for (const input of inputs) {
      assertJson(input);
      this.checkInput?.(input);
    }
    if (new TextEncoder().encode(JSON.stringify(inputs)).byteLength >
      1024 * 1024) {
      failure("batch_limit", "JSON batches allow at most 1 MiB of encoded inputs");
    }
    const values = structuredClone([...inputs]);
    return this.runSelection(async () => {
      for (const value of values)
        this.checkInput!(value);
      return { inputs: values };
    });
  }
  /** Rebind the same compiled registration to another scope of this native transport. */
  forScope(owner: Scope): Operation<I, O> {
    if (owner.transport !== this.owner.transport)
      failure("scope_mismatch", "Operation belongs to another native connection");
    return new Operation(this.registration ?? this.id, owner, this.prepare);
  }
}
/** Bind a registration from the generated manifest without redeclaring schemas. */
export function operationFactory<I, O>(manifest: {
  /** Compiled operation registrations from the generated application manifest. */
  operations: OperationDefinition[];
}, id: string, bind: (definition: OperationDefinition) => Operation<I, O>): Operation<I, O> {
  const definition = manifest.operations.find((operation) => operation.id === id);
  if (!definition)
    failure("unknown_operation", `Operation ${id} is absent from the generated manifest`);
  return bind(definition);
}
