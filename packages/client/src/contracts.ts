/** Shared JSON contracts match revenant-core; 64-bit counters never become JS numbers. */
/** Lossless JSON data accepted by native operation contracts. */
export type Json = null | boolean | number | string | Json[] | {
  /** Object properties recursively contain only lossless JSON values. */
  [key: string]: Json;
};
/** JSON Schema 2020-12 emitted by the native contract generator. */
export type JsonSchema = boolean | {
  /** Schema keywords emitted by the compiled native contract generator. */
  [key: string]: unknown;
};
/** Structured host failure, suitable for UI display and retry decisions. */
export interface RuntimeError {
  /** Stable failure classification for application decisions. */
  code: string;
  /** Human-readable failure explanation suitable for UI presentation. */
  message: string;
  /** Optional structured context accompanying the classified failure. */
  details?: unknown;
  /** Whether the native host considers retrying this failure appropriate. */
  retryable: boolean;
}
/** Throwable equivalent of a wire failure. */
export class CapabilityError extends Error implements RuntimeError {
  /** Construct a throwable classified failure while preserving native retry information. */
  constructor(
    /** Stable failure classification for application decisions. */
    public code: string, message: string,
    /** Optional structured context accompanying the classified failure. */
    public details?: unknown,
    /** Whether the native host considers retrying this failure appropriate. */
    public retryable = false) {
    super(message);
    this.name = "CapabilityError";
  }
  /** Serialize without losing the native failure classification. */
  toJSON(): RuntimeError {
    return {
      code: this.code,
      message: this.message,
      details: this.details,
      retryable: this.retryable,
    };
  }
}
/** Normalize host or JavaScript errors for public stores. */
export function errorOf(error: unknown, code = "operation_failed"): RuntimeError {
  if (error &&
    typeof error === "object" &&
    "code" in error &&
    "message" in error) {
    return {
      code: String(error.code),
      message: String(error.message),
      details: "details" in error ? error.details : undefined,
      retryable: "retryable" in error && error.retryable === true,
    };
  }
  return {
    code,
    message: error instanceof Error ? error.message : String(error),
    retryable: false,
  };
}
/** Throw a classified capability failure. */
export function failure(code: string, message: string, details?: unknown): never {
  throw new CapabilityError(code, message, details);
}
/** Opaque host capability; scope and generation are checked by native ownership. */
export interface Handle {
  /** Opaque native resource identity, never a filesystem path. */
  id: string;
  /** Native owner scope checked during capability admission. */
  scope: string;
  /** Resource generation checked together with identity and scope. */
  generation: number;
  /** Native capability category, such as a file resource. */
  kind: string;
}
/** File metadata projection. Size and optional modified timestamp remain decimal strings. */
export interface FileEntry {
  /** Full native file capability; display metadata alone grants no access. */
  handle: Handle;
  /** Native file display name from admitted metadata. */
  name: string;
  /** Path relative to the chosen folder, used only for presentation. */
  relativePath: string;
  /** Captured byte size as an unsigned decimal string. */
  size: string;
  /** Native MIME description used for renderer selection. */
  mime: string;
  /** Optional modification time in epoch milliseconds as a signed decimal string. */
  modified?: string;
}
/** Canonical native task states. Requesting cancellation does not imply completion. */
export type TaskState = "queued" | "running" | "cancelling" | "succeeded" | "partial" | "failed" | "cancelled" | "interrupted";
/** Canonical unsigned 64-bit decimal counters. */
export interface Progress {
  /** Completed units as a canonical unsigned 64-bit decimal string. */
  completed: string;
  /** Optional total units in the same canonical decimal representation. */
  total?: string;
  /** Optional native progress explanation suitable for presentation. */
  message?: string;
}
/** Convenience input for progress conversion; unsafe numeric values are rejected. */
export interface NumericProgress {
  /** Completed units as a nonnegative safe integer, normalized by progressOf. */
  completed: number;
  /** Optional nonnegative safe integer total, at least completed. */
  total?: number;
  /** Optional display message preserved during counter normalization. */
  message?: string;
}
/** Validate and normalize counters without rounding. */
export function progressOf(progress: Progress | NumericProgress): Progress {
  const count = (value: string | number): string => {
    if (typeof value === "number" &&
      (!Number.isSafeInteger(value) || value < 0)) {
      failure("invalid_progress", "Numeric progress must be a nonnegative safe integer");
    }
    const result = String(value);
    if (!/^(0|[1-9][0-9]*)$/.test(result) ||
      BigInt(result) > 18446744073709551615n) {
      failure("invalid_progress", "Progress must be a canonical unsigned 64-bit decimal string");
    }
    return result;
  };
  const completed = count(progress.completed);
  const total = progress.total === undefined ? undefined : count(progress.total);
  if (total !== undefined && BigInt(total) < BigInt(completed)) {
    failure("invalid_progress", "Progress total must be >= completed");
  }
  return {
    completed,
    ...(total !== undefined ? { total } : {}),
    ...(progress.message !== undefined ? { message: progress.message } : {}),
  };
}
/** Native batch outcome. The u64 ordinal remains a canonical decimal string. */
export interface Outcome<O> {
  /** Original input ordinal as an unsigned decimal string. */
  ordinal: string;
  /** Native captured input associated with this outcome. */
  input: unknown;
  /** Successful output validated against the compiled operation schema. */
  output?: O;
  /** Classified per-input failure; successful output is absent when supplied. */
  error?: RuntimeError;
}
/** Core compatibility outcomes. Streaming desktop batches leave this legacy array empty. */
export interface LegacyOutcome<O> {
  /** Legacy zero-based input index; streaming desktop batches omit these rows. */
  index: number;
  /** Successful output in the legacy core compatibility representation. */
  output?: O;
  /** Classified failure in the legacy core compatibility representation. */
  error?: RuntimeError;
}
/** Native task and result-store summary counts. */
export interface TaskSummary {
  /** Number of retained completed outcomes as an unsigned decimal string. */
  completed: string;
  /** Number of successful retained outcomes as an unsigned decimal string. */
  succeeded: string;
  /** Number of failed retained outcomes as an unsigned decimal string. */
  failed: string;
}
/** Native task projection. Batch outcomes are paged; this compatibility field stays bounded. */
export interface TaskSnapshot<O = unknown> {
  /** Native task identity; empty in the local pre-admission projection. */
  id: string;
  /** Native task owner scope; empty until native admission. */
  scope: string;
  /** Executor lifecycle state; cancellation requests do not invent completion. */
  state: TaskState;
  /** Native progress with lossless decimal unit counts. */
  progress: Progress;
  /** Legacy compatibility rows; streaming desktop batches keep this array empty. */
  outcomes: LegacyOutcome<unknown>[];
  /** Terminal successful result, or a client BatchResult projection for batches. */
  result?: O;
  /** Optional classified task or output-contract failure. */
  error?: RuntimeError;
  /** Whether native or local cancellation has been requested. */
  cancelRequested: boolean;
  /** Lossless decimal counts for retained batch outcomes. */
  summary: TaskSummary;
}
/** Bounded page from a native result set. */
export interface ResultPage<O> {
  /** Outcome counts from the same committed native revision as these rows. */
  counts: TaskSummary;
  /** At most the requested limit of retained native outcomes. */
  items: Outcome<O>[];
  /** Safe integer position among completed rows, independent of input ordinals. */
  offset: number;
  /** Safe integer count of currently retained completed rows. */
  total: number;
}
/** Generated DTO description and its machine-checkable schema. */
export interface TypeDefinition {
  /** Compiled Rust contract type name exported by the binding generator. */
  name: string;
  /** TypeScript declaration emitted from the compiled Rust contract. */
  typescript: string;
  /** Compiled JSON Schema used to validate inputs or successful outputs. */
  schema: JsonSchema;
}
/** Registration format retained by generated $lib/revenant facades. */
export interface OperationDefinition {
  /** Stable compiled operation registration identity. */
  id: string;
  /** Rust documentation published with the operation registration. */
  description: string;
  /** Compiled input declaration and its authoritative schema. */
  input: TypeDefinition;
  /** Compiled output declaration and its authoritative schema. */
  output: TypeDefinition;
  /** Optional Rust definition location for IDE navigation. */
  source?: {
    /** Source file containing the registered Rust operation definition. */
    file: string;
    /** One-based source line of the registered operation definition. */
    line: number;
    /** Rust module path containing the operation definition. */
    module: string;
  };
}
/** Desktop compiled contract format. */
export interface ContractManifest {
  /** Desktop contract format version; this client requires version three. */
  version: 3;
  /** Desktop dispatcher protocol version; this client requires protocol two. */
  protocol: 2;
  /** Compiled contract digest used to reject incompatible application generations. */
  digest: string;
  /** Compiled native registrations available to generated operation bindings. */
  operations: OperationDefinition[];
  /** Optional additional compiled type declarations for generated authoring facades. */
  definitions?: TypeDefinition[];
}
/** Native states after which no further progress is expected. */
export const terminalStates: ReadonlySet<TaskState> = new Set([
  "succeeded",
  "partial",
  "failed",
  "cancelled",
  "interrupted",
]);
/** Reject cycles, non-JSON values and lossy integral numbers before IPC. */
export function assertJson(value: unknown): void {
  const seen = new Set<object>();
  const visit = (v: unknown): void => {
    if (v === null || typeof v === "string" || typeof v === "boolean")
      return;
    if (typeof v === "number" &&
      Number.isFinite(v) &&
      (!Number.isInteger(v) || Number.isSafeInteger(v)))
      return;
    if (typeof v !== "object" || !v || seen.has(v))
      failure("invalid_json", "Expected finite, lossless JSON data");
    if (!Array.isArray(v) &&
      Object.getPrototypeOf(v) !== Object.prototype &&
      Object.getPrototypeOf(v) !== null) {
      failure("invalid_json", "Expected a plain JSON object");
    }
    seen.add(v);
    Object.values(v).forEach(visit);
    seen.delete(v);
  };
  visit(value);
}
/** Validate a native window or selection bound without silently clamping intent. */
export function windowBounds(offset = 0, limit = 128): {
  /** Validated safe integer row position for native page admission. */
  offset: number;
  /** Validated bounded page size from one through 512. */
  limit: number;
} {
  if (!Number.isSafeInteger(offset) ||
    offset < 0 ||
    !Number.isInteger(limit) ||
    limit < 1 ||
    limit > 512) {
    failure("invalid_window", "Offset must be a nonnegative safe integer and limit must be 1..512");
  }
  return { offset, limit };
}
