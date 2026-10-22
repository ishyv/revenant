# Native dispatcher wire (0.4)

The authoritative client types are [`src/wire.ts`](../src/wire.ts), with data models
in [`src/contracts.ts`](../src/contracts.ts) and [`src/files.ts`](../src/files.ts).
Manifest version is **6**, protocol is **5**, and SDK/client compatibility revision is **4**.
The generated operation descriptor carries `arguments`, `effect`, `independent`
and `agent`, along with `id`, `description`, `input`, `output`, optional `updates`, and optional
`source: {file, line, module}`.

Frontend requests carry a bounded `requestId`, correlated with native task
admission, execution, completion and errors. Trace retention is bounded and
excludes arguments/results. Development connections additionally report bounded
frontend errors and route/title/visibility state, with source authority retained
in native runtime data. Production rejects frontend diagnostic snapshots.

```ts
import { Channel, invoke } from "@tauri-apps/api/core";
await invoke("revenant_dispatch", {
  request,
  updates: new Channel(onSnapshot),
});
```

`updates` is omitted for actions without ongoing snapshots. Each task Channel
receives **TaskSnapshot directly**, not an event envelope. Each folder/view Channel
receives **FileCollectionSnapshot directly**. Submitting `folder.query` and
`view.query` supplies the view subscription. Subsequent `view.window` changes the
window without another Channel. Every update echoes the requested query generation.
A Channel can deliver a terminal task update before the invoke reply. Initial
responses must not regress a more recent Channel update. File snapshots include
their own `view` identity even before a query invoke resolves. Every received file
snapshot, including an initial reply or a discarded stale page, is acknowledged
with `view.ack {scope, view: snapshot.view, revision: snapshot.revision}`.

The Tauri handler authenticates its invoking window out-of-band. The root request
does not accept a client-supplied window identity. Native errors reject the invoke
with `{code, message, details?, retryable}`. Void actions return null/void and their
callers ignore the reply.

## Requests and responses

All requests include `action`. `scope` is required except for root connection and
scope creation. Other fields below are in addition to `action` and that scope.

| Action              | Request fields                                        | Response                                                                           |
| ------------------- | ----------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `root.connect`      | none                                                  | `{scope, manifest}`                                                                |
| `scope.create`      | `parent: string`                                      | `{scope: string}`                                                                  |
| `scope.dispose`     | none                                                  | void                                                                               |
| `resource.release`  | `handle: Handle`                                      | void; native owner revoked, existing leases retained                               |
| `folder.open`       | none                                                  | `{folder: string, name: string, path: string, bookmark: {path: string}}` or null |
| `folder.reopen`     | `bookmark: {path: string}`                            | `FolderDescriptor` or null                                                         |
| `folder.dispose`    | `folder: string`                                      | void                                                                               |
| `folder.query`      | `folder, options, generation, offset, limit`          | `{view: string, snapshot: FileCollectionSnapshot}`                                 |
| `view.query`        | `view, options, generation, offset, limit`            | `{view: string, snapshot: FileCollectionSnapshot}`                                 |
| `view.refresh`      | `view, options, generation, offset, limit`            | Fresh collection-owned inventory; `{view, snapshot}` and scan Channel updates       |
| `view.cancel`       | `view`                                               | void; release collection discovery ownership, preserving admitted batch inventory leases |
| `view.window`       | `view, generation, offset, limit`                     | `FileCollectionSnapshot`                                                           |
| `view.ack`          | `view: string, revision: number`                      | void; may publish the next coalesced Channel page                                  |
| `view.dispose`      | `view`                                                | void                                                                               |
| `selection.set`     | `view: string, selection?: string, handles: Handle[]` | `{selection: string, count: number}`                                               |
| `selection.dispose` | `id: string`                                          | void                                                                               |
| `task.run`          | `operation: string, input: JSON`                      | `TaskSnapshot`                                                                     |
| `task.batch`        | `operation: string, selection: NativeSelection`       | `TaskSnapshot`                                                                     |
| `task.snapshot`     | `task: string`                                        | `TaskSnapshot`                                                                     |
| `task.cancel`       | `task: string`                                        | `TaskSnapshot`                                                                     |
| `task.dispose`      | `task: string`                                        | void                                                                               |
| `task.results`      | `task: string, offset, limit`                         | `ResultPage`                                                                       |
| `settings.load`     | `key: string, defaults: JSON`                         | JSON value                                                                         |
| `settings.save`     | `key: string, value: JSON`                            | void                                                                               |
| `media.preview`     | `handle: Handle`                                      | `PreviewDescriptor`                                                                |
| `media.dispose`     | `preview: string`                                     | void                                                                               |
| `runtime.inspect`   | none                                                  | `ProviderSnapshot[]`                                                               |
| `runtime.configure` | `provider: string, config: JSON`                      | void                                                                               |
| `runtime.replace`   | `provider: string, replacement: string`               | void                                                                               |

## Files and query generations

```ts
type Handle = { id: string; scope: string; generation: number; kind: string };
type FileEntry = {
  handle: Handle;
  name: string;
  relativePath: string;
  size: string;
  mime: string;
  modified?: string;
};
type FileQueryOptions = {
  recursive?: boolean;
  search?: string;
  sort?: "name" | "relativePath" | "size" | "modified";
  descending?: boolean;
};
type FileCollectionSnapshot = {
  view: string;
  items: FileEntry[];
  offset: number;
  total: number;
  discovered: number;
  state: "loading" | "ready" | "failed" | "disposed";
  generation: number;
  revision: number;
  indexGeneration: number;
  warnings: RuntimeError[];
  warningCount: number;
  error?: RuntimeError;
};
type NativeSelection =
  | { id: string }
  | { view: string; allMatching: true; generation?: number }
  | { inputs: unknown[] };
```

Folder, view and task IDs identify native resources; **file handles are always
objects**, including explicitly selected `handles` and preview `handle`. `path` is
presentation-only. Size is an unsigned decimal string; modified follows core's
optional string representation.

Query generation starts at 1 and advances on `FileCollection.query/filter/sort/refresh`.
This is a **client query generation**, not the index's scan revision. Host snapshots
must echo it even when more files are discovered. Old query streams can continue
briefly but their snapshots are discarded and still acknowledged. `revision` is
the native monotonically increasing page publication counter **within one view**.
`indexGeneration` reports the actual inventory generation independently of both
the query generation and the publication revision. These native u64 fields are
JSON numbers; progress, summary and outcome ordinals use decimal strings.

`view.query` creates a replacement view using the previous view's inventory.
A recursion change starts a fresh inventory for that collection. It returns a new `view`
and initial snapshot. The client retires the previous view after its in-flight
viewport and ACK work has completed; query readiness never waits on that cleanup.

`view.refresh` always starts a fresh inventory for this collection, preserving
query options. Its initial invoke is admission, while the public `refresh()`
promise waits for a current terminal scan snapshot. Later query/sort changes
inherit this inventory rather than returning to the folder's original cache.
Other collections keep their previous inventories. Refresh resets offset and
increments the selection generation immediately. Abort/supersession rejects
the associated public promise; returned pages are still ACKed and retired.
The host holds explicit discovery leases for active views and admitted matching
batches. Retiring the last such owner cancels scanning even if the worker still
owns its storage. A cancellation leaves partial metadata; another explicit
refresh creates a usable inventory. Already admitted batch inputs remain pinned.

Native publication retains the prior page handles until its replacement's ACK.
Only one replacement can await acknowledgement, with at most two windows of
handle ownership per view. Scan refreshes coalesce while that ACK is pending.
The client defers ACK work to a microtask, awaits earlier pending selection capture
and batch-admission barriers, and ACKs the snapshot's own identity and revision.
Initial query readiness resolves independently, allowing captures awaiting it to
finish. ACK returns void; any latest coalesced page arrives over the Channel and
requires another ACK. A duplicate initial reply cannot regress a newer streamed
revision. A mismatched viewport discards its rows instead of retaining handles
that its ACK can revoke.

`view.window` requires no outstanding page ACK; otherwise native admission returns
retryable `window_pending`. Client viewport requests serialize, coalesce obsolete
requests, and await the complete known ACK sequence, including further Channel
pages published during ACKs. Native errors remain structured and visible for an
explicit retry if a new scan publication races viewport admission.

Default window is 128; maximum is 512. Limits outside 1..512 are rejected by the
client. Offsets and matching/discovery counts must be nonnegative safe integers.
Snapshots carry bounded warning examples, converted by the dispatch adapter into
structured errors. During scanning, `total` counts currently indexed matches and
`discovered` counts currently indexed regular files. `ready` marks scan completion.

`allMatching: true` waits for scanning to finish before freezing the query natively.
Explicit selection uses `selection.set` with at most 512 full handles, capturing
native leases before viewport row ownership is released. Existing selected handles
remain admissible through the supplied native `selection` ID even after scrolling.
Updates serialize; ACK and window/query changes wait for capture acknowledgements.
`task.batch` receives only `{id}` for a captured selection. Subsequent edits wait
for batch admission so an already requested batch cannot silently change inputs.
`selection.dispose {id}` releases captured ownership.
The host also accepts its native `selection` alias; the client consistently sends
`id`. Existing captures can reuse leases retained by the supplied selection ID.

`FileCollection.selector()` supplies `{view, allMatching: true, generation}`.
Native batch admission validates that generation before freezing the query, so
an edit cannot silently change a pending all-matching batch. Both
`files.allMatching()` and `files.selection().allMatching()` create that selector.

Small application-record batches use `{inputs}` with at most 512 lossless JSON
records and at most 1 MiB encoded JSON. Native admission validates contracts and
captures any permitted resources before execution. Entire file datasets always
use native selectors.

## Tasks and results

`TaskSnapshot` matches core: `id`, `scope`, canonical `state`, `progress`, legacy
`outcomes`, `summary`, optional `result/error`, and `cancelRequested`. Progress has
`completed: string`, optional `total: string`, and optional `message`. Summary is
`{completed: string, succeeded: string, failed: string}`. These are canonical u64
decimal strings. Streaming desktop batches keep legacy `outcomes` empty; that core
compatibility array retains its old `{index, output?, error?}` shape.

```ts
type ResultPage = {
  counts: { completed: string; succeeded: string; failed: string };
  items: {
    ordinal: string;
    input: unknown;
    output?: unknown;
    error?: RuntimeError;
  }[];
  offset: number;
  total: number;
};
```

The dispatch adapter converts result-store `u64` ordinals to strings, and
`counts.total` to `counts.completed` with string counters. `offset` and `total` are
safe integer positions among completed rows, not source ordinals. Pages default
to 128 and max at 512. Successful null outputs remain present; absence of output
is distinct from JSON null. Exactly one of output/error is present in an outcome.
Counts and page rows reflect the same committed native result-store revision.

Native workers own task admission, concurrency, cancellation checkpoints, input
capture and result persistence. Webview subscriber counts never determine whether
a task runs. `task.dispose` revokes task/result ownership; it must handle active
work by requesting cancellation and retaining admitted leases until execution
finishes. Scope disposal also closes its owned tasks and native subscriptions.

## Settings, previews and providers

`settings.load` returns the merged/validated value directly, not an envelope.
`settings.save` completes only after atomic persistence. Loaded values must match
the supplied defaults shape; invalid saved data is a structured host failure.
Client edits made while loading remain local and are queued for a native save.

`PreviewDescriptor` is `{preview: string, kind, name, mime, url?, text?, truncated?}`.
Kind is text/image/audio/video/unsupported. The adapter maps the native preview
store's token ID to `preview`, and formats media URLs for the calling platform.
The client renders returned URLs verbatim, including Windows custom-protocol
conversion. It never calls `convertFileSrc` on a returned URL or constructs file
paths. `media.dispose` releases the token and its native leases.

Provider descriptors use SDK field names: `{id, description, dependencies,
configurationSchema}`. Snapshots additionally include `config`, numeric
`generation`, string `activeTasks/resources`, and `mutable`. Configure and replace
are native transactions. Replacement names refer only to compatible compiled
native provider factories registered in the application.
