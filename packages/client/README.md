# @revenant/client 0.3

Svelte projections for Revenant desktop applications. Rust owns files, scans,
operation execution, task state, result storage, previews and persistent settings.
The webview retains UI state and bounded metadata windows. This package requires
the generated Tauri host; package version 0.3 uses manifest version 3, protocol 2.

## Configure one root

The generated `$lib/revenant` facade supplies the compiled manifest and operation
bindings. Its root should be created during parent component initialization.
Components use child scopes so leaving a screen releases its native resources.
The lower-level primitives used by that generated facade are:

```ts
import { createApp, useApp } from "@revenant/client/svelte";
import type { ContractManifest } from "@revenant/client";

// `manifest` is emitted from the application's Rust registrations.
declare const manifest: ContractManifest;
const root = createApp({ manifest });
const app = useApp(root);
```

Calling `app.bindOperation<I, O>(definition).run(input)` before `app.ready` is safe:
it waits for root connection, validates the generated definition against the native
manifest, and then submits. Every file/settings/runtime/preview request waits too.
`app.status` reports loading, ready, failed or disposed. A manifest mismatch closes
the acquired native root and prevents capability requests.

Use `@revenant/client` for primitives outside component initialization. It does
not install Svelte context; `provideApp(root)` does that without transferring root
ownership. `useApp()` resolves an existing ancestor context, while `useApp(root)`
uses an explicit generated root. Both own a native child scope until unmount.
`useApp` also provides that child in its component context. Descendants inherit
the nearest scoped app, so their native owners follow component ancestry.

## Folder windows and native selection

```ts
const folder = await app.files.openFolder(); // undefined if the picker is cancelled
if (folder) {
  const files = folder.files({ recursive: true, search: ".png", sort: "name" });
  await files.ready; // Query acknowledgement, not scan completion
  await files.window(0); // 128 entries by default
  await files.window(128, 256); // maximum 512
  await files.filter("holiday");
  await files.sort("size", true);
  await files.loaded; // scan completion; returns metadata for one window
}
```

`FileCollection` is a Svelte store of `{view, items, offset, total, discovered, state,
generation, revision, indexGeneration, warnings, warningCount, error?}`.
`items` is **only the current window**. `total`
counts indexed matches; `discovered` counts indexed files before filtering. Both
can grow during scanning. `state: 'ready'` means the scan is complete. Native
search/sort applies to the indexed dataset rather than the currently rendered
rows. Query changes clear the window and advance the requested generation; late
responses from earlier queries or windows cannot replace the current viewport.
Every received page is internally acknowledged with its own native view identity
and publication revision, including discarded pages. Native code retains at most
the current and prior windows until that ACK, then may send one coalesced refresh.
The client waits for pending selection captures and batch admission before ACK,
and drains that ACK sequence before a viewport request. Initial `ready` resolves
independently of ACK barriers. `indexGeneration` reports the real inventory
generation; it is distinct from the client query and native publication counters.
Native `window_pending` remains a retryable failure if a scan races admission.

`files.selection()` keeps at most 512 explicitly selected full file handles.
`await selection.toggle(entry)` captures native leases for a current window entry. `selection.ready` awaits the latest serialized capture, and `selection.status` reports capture failures. Scrolling and query changes wait for pending capture acknowledgements before releasing the old row handles. Native selections survive
scrolling; changing the query clears them. Arbitrary entries from another view
cannot be inserted through `selection.set`. The host revalidates capabilities.

`selection.run(operation)` returns `Task<BatchResult<O>>`. To process an entire
query, use `files.allMatching().run(operation)`. That selector sends the view ID
rather than a huge array and **waits for native scan completion** before freezing
matches. A selector captured before a query change fails with `stale_selection`.
File handles are objects `{id, scope, generation, kind}` throughout; string
folder/view/task resource IDs are never substituted for file handles. `size` and
optional `modified` stay strings to preserve the core representation.
The accepted `files.selection().allMatching().run(operation)` recipe is an alias
for the same native query selector, with selection/scope lifetime checks. Its
query generation is sent to native batch admission. It selects the complete
query, independently of the explicit selection's current items.

For small application-owned datasets, `app.collection(items)` retains local
`filter(predicate)`, `sort(compare)`, selection and Svelte store support. A local
`Collection` is deliberately separate from a native `FileCollection`.

`Operation.runBatch(inputs)` and local `Selection.run(operation)` preserve custom
record workflows through native execution. Inputs are limited to 512 JSON records
and 1 MiB encoded JSON; successful results use the same paged `BatchResult` API.
File datasets should use captured native selections instead of copying metadata
into a local collection.

`app.files.checksum` and `app.media.readMetadata` resolve their built-in schemas
from the compiled host manifest. The generated facade intersects those capability
objects with its exact generated `Operation<FileEntry, Checksum>` and metadata
types. The client does not maintain a second built-in schema definition. Plain
primitives default these result types to `unknown`; generated bindings must
replace the broad capability signature when adding compiled result typing.
An intersection with an existing `Operation<FileEntry, unknown>` can retain its
broad overload and is not sufficient to provide a typed `call()` result.

## Operations, tasks and retained results

Generated bindings use the existing operation descriptor format, now with
optional `source: {file, line, module}` for IDE navigation. `Operation.run(input)`
returns immediately. `Operation.call(input)` awaits the result and disposes the
native task in a `finally` block. Input and successful output values are checked
against their generated schemas.

A task's `result` promise is maintained from creation, independently of store
subscriptions. Subscribe for UI progress, or simply await it. Terminal states are
`succeeded`, `partial`, `failed`, `cancelled`, and `interrupted`. Partial execution
resolves with retained results; failed/cancelled/interrupted execution rejects.
`cancel()` sets `cancelRequested` and asks the native executor to stop. It does
not invent a terminal state: native checkpoints acknowledge cancellation.
Progress and summary counters remain canonical decimal strings.

```ts
// `operation` is the generated Operation<FileEntry, Output> binding.
const task = files.allMatching().run(operation);
try {
  const batch = await task.result;
  for await (const page of batch.pages(128)) {
    for (const outcome of page.items) {
      if (outcome.error) console.error(outcome.error.message);
      else console.log(outcome.ordinal, outcome.input, outcome.output);
    }
  }
} finally {
  await task.dispose();
}
```

`BatchResult` is a result-store projection, **not an Outcome array**. `page(offset,
limit)` returns bounded items and coherent counts; `pages(limit)` iterates them
without retaining an entire dataset. Individual outcomes include their original
string ordinal and captured input. `task.results(offset, limit)` also exposes
retained outcomes after a rejected result or while a batch runs. During execution,
earlier completions can shift offset pages; inspect one current page, then do
complete sequential exports after the task becomes terminal.

Tasks and native batch results remain owned by their application scope until
`task.dispose()` or scope disposal. Awaiting `task.result` does not release batch
results before consumers read them. `TaskStatus` is an observer and does not
cancel or dispose a task when that status widget unmounts.

## Settings and previews

```ts
const settings = app.settings.define(
  { search: "", recursive: true },
  { key: "files" },
);
await settings.ready;
settings.update((value) => ({ ...value, search: "holiday" }));
await settings.flush(); // host atomic save; no localStorage

const preview = await app.media.preview(entry); // also accepts entry.handle
// preview.name, kind, mime, bounded text/truncated, or host-formatted url
await preview.dispose();
```

`folder.bookmark` returns the host's serializable `{path}` bookmark. Persist it in
settings and use `await app.files.reopenBookmark(bookmark)` to restore the folder.
The native host readmits the location; a display path alone is never a file handle.
`await folder.dispose()` closes its views and folder owner explicitly. A cancelled
or declined reopen resolves to undefined; native restoration failures reject with
a structured error.

Settings publish defaults immediately, validate the loaded shape, preserve edits
made during loading, and serialize/coalesce saves. `status` exposes loading,
dirty, saving, saved, failed and disposed. Save failures keep the current local
value for an explicit `flush()` retry. Await `flush()` where durability matters.
Scope disposal waits for already queued native saves before closing the scope.
Keys identify application settings; the host owns path selection and atomic IO.

A preview owns a native lease/token, not a JavaScript Blob or object URL. Render
its `url` exactly as returned; the host formats Windows custom protocol URLs.
Replacement and component lifetime should be managed by the owning scope or an
explicit `preview.dispose()`. `FilePreview` only renders, so swapping a preview
does not silently transfer or destroy another consumer's ownership.
Its accessible name defaults to the trusted native preview name; `name` can
override it for presentation.

## Inspect and change native providers

`await app.runtime.inspect()` returns native descriptors, configuration,
generation, mutability, and string `activeTasks`/`resources` counts. Descriptor
fields match the SDK: `id`, `description`, `dependencies`, `configurationSchema`.
The runtime itself is a readable store of the latest inspection.

Use `ProviderDescriptor<Config>` from a generated/custom capability declaration
for typed configuration, then `await app.runtime.configure(descriptor, config)`.
The client checks its schema; the host prepares an isolated candidate and commits
transactionally. `await app.runtime.replace(providerId, compiledReplacementId)`
selects a compatible replacement registered in the native application. JavaScript
provider callbacks, browser file readers, and worker schedulers are not supported.

## Optional Svelte UI

```svelte
<script lang="ts">
  import { CollectionView, TaskStatus, ErrorNotice, FilePreview } from '@revenant/client/ui';
  import '@hyvnt/hyvui/styles.css';
  // files, selection, task and preview come from the component's scoped app.
</script>

<CollectionView collection={files} {selection} onactivate={showPreview} />
<TaskStatus {task} label="Analyze files" />
<FilePreview {preview} name={selectedName} />
```

`CollectionView` uses the actual `@tanstack/svelte-virtual` Svelte adapter with
Svelte 5 support, fixed-height rows, overscan and bounded native window requests.
It virtualizes the native matching count, rather than merely virtualizing a copied
array. The optional `row` snippet receives `(entry, absoluteIndex)`; `rowHeight`
controls its fixed height (minimum 24px). `height` defaults to `28rem`. Explicit
selection admits full handle identities; view-relative paths identify reappearing
rows only for UI presentation. Native scan warnings and failures are shown
through HyvUI defaults. The scroll region supports keyboard focus.

The `/ui` entry requires the optional peer `@hyvnt/hyvui` pinned to **1.0.0**.
Repository development uses the existing vendored snapshot tarball. Importing the
core entry does not import HyvUI. No application theme is injected automatically.
Adapter references: [TanStack Svelte Virtual](https://tanstack.com/virtual/latest/docs/framework/svelte/svelte-virtual)
and [Tauri core IPC](https://v2.tauri.app/reference/javascript/api/namespacecore/).

## Ownership and migration

`app.dispose()` begins cleanup; `await app.disposeAsync()` waits for it. Scopes
release local owners in reverse order, then dispose the native scope. Native
teardown remains authoritative if the webview disappears. Cleanup failures are
reported by `app.cleanupErrors`. Resource widgets never define execution lifetime.

Version 0.3 removes active `ServerClient`, `FileBroker`, `WasmWorkerPool`, worker
entrypoints, JavaScript `Scheduler`, browser permission bookmarks/files and localStorage
settings. The `/runtime` alias is removed. Configure the generated desktop facade
instead of `wasm`, `server`, browser provider or worker options. Local collection
selections submit bounded JSON-record batches; file selections capture native
leases and submit native IDs.

`npm test` runs native fake-transport workflows for page ownership/ACK ordering,
selection capture, query and viewport races, streamed task completion, retained
partial/cancelled results, settings persistence and teardown. The peer enforces
native capability ownership independently of client internals. `npm run check`
also checks native provider/generated-operation typing in `tests/provider-typing.ts`.
Obsolete browser/server/worker tests and the delayed-WASM fixture were removed.
These checks complement actual Tauri bridge and large native-index acceptance;
they do not prove a packaged desktop application's behavior.

See [the exact dispatcher wire](docs/WIRE.md) for host integration.
Every reachable public constructor, method and DTO field carries source JSDoc;
`node scripts/check-api-docs.mjs` from the repository root checks all three package
entry points. See [the public API reference](docs/API.md) for ownership and usage.
