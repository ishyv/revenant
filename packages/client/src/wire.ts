/** Stable desktop dispatcher protocol: manifest 3 / protocol 2, command revenant_dispatch. */
import type { ContractManifest, Handle, ResultPage, TaskSnapshot, } from "./contracts.js";
import type { FileCollectionSnapshot, FileQueryOptions, FolderDescriptor, FolderBookmark, } from "./files.js";
import type { NativeSelection } from "./operation.js";
import type { PreviewDescriptor } from "./media.js";
import type { ProviderSnapshot } from "./runtime.js";
/** Root identity and the host's compiled manifest. Window authentication is out-of-band in Tauri. */
export interface RootConnection {
  /** New native root scope owned by the authenticated calling window. */
  scope: string;
  /** Exact compiled contract manifest returned by the native host. */
  manifest: ContractManifest;
}
/** Native child scope identity. */
export interface ScopeConnection {
  /** New native child identity owned by the supplied parent scope. */
  scope: string;
}
/** Native view creation/query acknowledgement. Updates contain only its snapshot. */
export interface ViewConnection {
  /** New native view identity, including every view.query replacement. */
  view: string;
  /** Initial bounded page; every received revision requires a matching view ACK. */
  snapshot: FileCollectionSnapshot;
}
/** Fields shared by every scoped capability request. */
export interface ScopedRequest {
  /** Native scope authorizing ownership of the requested capability. */
  scope: string;
}
/** Query acknowledgement parameters. generation is supplied by the client and echoed by the host. */
export interface ViewQueryRequest {
  /** Native filter, sort, and recursive traversal parameters. */
  options: FileQueryOptions;
  /** Client query generation echoed by every snapshot of this view. */
  generation: number;
  /** Safe integer initial matching-row position for the bounded viewport. */
  offset: number;
  /** Initial viewport size from one through 512, inclusive. */
  limit: number;
}
/** Bounded viewport parameters. Offsets/counts must be nonnegative JS-safe integers. */
export interface WindowRequest {
  /** Safe integer row position in a native file or result dataset. */
  offset: number;
  /** Requested bounded page size from one through 512, inclusive. */
  limit: number;
}
/** Acknowledgement that the native selection owns its captured input leases. */
export interface SelectionConnection {
  /** Native identity owning the captured file leases. */
  selection: string;
  /** Number of acknowledged explicit inputs, at most 512. */
  count: number;
}
/** Exact admitted request union. Input/config/defaults/value are validated lossless JSON data. */
export type DispatchRequest = {
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "root.connect";
} | {
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "scope.create";
  /** Native parent scope under which the new child is created. */
  parent: string;
} | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "scope.dispose";
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "folder.open";
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "folder.reopen";
  /** Saved native folder location readmitted through folder restoration. */
  bookmark: FolderBookmark;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "folder.dispose";
  /** Owned native folder identity used for query creation or disposal. */
  folder: string;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "folder.query";
  /** Owned native folder identity used for query creation or disposal. */
  folder: string;
} & ScopedRequest & ViewQueryRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "view.query";
  /** Owned native view identity used for bounded page or selection admission. */
  view: string;
} & ScopedRequest & ViewQueryRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "view.window";
  /** Owned native view identity used for bounded page or selection admission. */
  view: string;
  /** Expected client query generation echoed or validated by the native host. */
  generation: number;
} & ScopedRequest & WindowRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "view.ack";
  /** Owned native view identity used for bounded page or selection admission. */
  view: string;
  /** Exact native page publication revision acknowledged after capture barriers. */
  revision: number;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "view.dispose";
  /** Owned native view identity used for bounded page or selection admission. */
  view: string;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "selection.set";
  /** Owned native view identity used for bounded page or selection admission. */
  view: string;
  /** Optional existing selection identity whose captured leases can be reused. */
  selection?: string;
  /** At most 512 full native capabilities captured into explicit selection leases. */
  handles: Handle[];
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "selection.dispose";
  /** Native captured selection identity whose ownership will be released. */
  id: string;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "task.run";
  /** Stable compiled operation identity selected for native execution. */
  operation: string;
  /** Lossless JSON input validated against the compiled native operation contract. */
  input: unknown;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "task.batch";
  /** Stable compiled operation identity selected for native execution. */
  operation: string;
  /** Captured inputs, all-matching selector, or bounded JSON records for batch admission. */
  selection: NativeSelection;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "task.snapshot" | "task.cancel" | "task.dispose";
  /** Owned native task identity used for state, cancellation, disposal, or results. */
  task: string;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "task.results";
  /** Owned native task identity used for state, cancellation, disposal, or results. */
  task: string;
} & ScopedRequest & WindowRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "settings.load";
  /** Application settings key resolved within native persistence storage. */
  key: string;
  /** Typed JSON defaults used to admit and validate stored settings. */
  defaults: unknown;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "settings.save";
  /** Application settings key resolved within native persistence storage. */
  key: string;
  /** Validated JSON value committed through native atomic settings persistence. */
  value: unknown;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "media.preview";
  /** Full native file capability admitted for scoped preview ownership. */
  handle: Handle;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "media.dispose";
  /** Owned native preview token whose resource lease will be released. */
  preview: string;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "runtime.inspect";
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "runtime.configure";
  /** Compiled provider identity used for native configuration or replacement. */
  provider: string;
  /** JSON configuration validated before isolated native candidate preparation. */
  config: unknown;
} & ScopedRequest) | ({
  /** Exact dispatcher action discriminant for this admitted request. */
  action: "runtime.replace";
  /** Compiled provider identity used for native configuration or replacement. */
  provider: string;
  /** Compatible provider factory identity compiled into the native application. */
  replacement: string;
} & ScopedRequest);
/** Reply map. Void actions may return native null; callers ignore the returned value. */
export interface DispatchReplies {
  /** Native root identity and the compiled host contract manifest. */
  "root.connect": RootConnection;
  /** Native child identity owned by the supplied parent scope. */
  "scope.create": ScopeConnection;
  /** Completion of native scope and resource teardown. */
  "scope.dispose": void;
  /** Admitted folder descriptor, or null when the picker is cancelled. */
  "folder.open": FolderDescriptor | null;
  /** Readmitted folder descriptor, or null when restoration is declined. */
  "folder.reopen": FolderDescriptor | null;
  /** Completion of native folder ownership release. */
  "folder.dispose": void;
  /** New native view identity and its initial bounded page. */
  "folder.query": ViewConnection;
  /** Replacement native view identity and its initial bounded page. */
  "view.query": ViewConnection;
  /** Bounded viewport snapshot requiring a matching publication ACK. */
  "view.window": FileCollectionSnapshot;
  /** Void acknowledgement; it may publish another coalesced Channel snapshot. */
  "view.ack": void;
  /** Completion of native view and retained page ownership release. */
  "view.dispose": void;
  /** Identity and count of the acknowledged native selection leases. */
  "selection.set": SelectionConnection;
  /** Completion of captured native selection ownership release. */
  "selection.dispose": void;
  /** Native task admission snapshot; ongoing updates use its Channel. */
  "task.run": TaskSnapshot;
  /** Native batch admission snapshot; outcomes remain in paged native storage. */
  "task.batch": TaskSnapshot;
  /** Latest native task lifecycle and progress projection. */
  "task.snapshot": TaskSnapshot;
  /** Native snapshot after a cancellation request, which may remain nonterminal. */
  "task.cancel": TaskSnapshot;
  /** Completion of task-owner release, with admitted work handled natively. */
  "task.dispose": void;
  /** Bounded outcome rows and coherent native result counts. */
  "task.results": ResultPage<unknown>;
  /** Merged and validated JSON settings value without a response envelope. */
  "settings.load": unknown;
  /** Completion of native atomic settings persistence. */
  "settings.save": void;
  /** Owned native preview token, display metadata, and bounded content descriptor. */
  "media.preview": PreviewDescriptor;
  /** Completion of native preview token and lease release. */
  "media.dispose": void;
  /** Current native provider descriptors, configurations, and ownership counts. */
  "runtime.inspect": ProviderSnapshot[];
  /** Completion of transactional native provider configuration. */
  "runtime.configure": void;
  /** Completion of a compatible compiled provider replacement transaction. */
  "runtime.replace": void;
}
/** Resolve the exact response type for an admitted action. */
export type DispatchReply<R extends DispatchRequest> = DispatchReplies[R["action"]];
/** Direct, unwrapped snapshot Channel payloads, selected by the action that creates the stream. */
export interface DispatchUpdates {
  /** Direct native task lifecycle snapshot delivered by the submission Channel. */
  "task.run": TaskSnapshot;
  /** Direct native task lifecycle snapshot delivered by the submission Channel. */
  "task.batch": TaskSnapshot;
  /** Direct bounded native file snapshot; every revision requires a matching ACK. */
  "folder.query": FileCollectionSnapshot;
  /** Direct bounded native file snapshot; every revision requires a matching ACK. */
  "view.query": FileCollectionSnapshot;
}
