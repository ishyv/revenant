# Client API reference

The generated `$lib/revenant` facade is the application authoring entry. Its
operation inputs, outputs, schemas, descriptions and Rust source locations come
from the compiled manifest. Client classes project native ownership and do not
redeclare built-in Rust DTOs or supply execution callbacks. Every public source
declaration has JSDoc, including constructors and DTO fields.

## Root and scopes

| API | Behavior and ownership |
| --- | --- |
| `createApp<Checksum, Metadata>({manifest?, transport?})` | Connect a root using Tauri by default. An expected manifest is checked before admitting capability requests. Plain primitives default built-in result types to `unknown`; compiled facades provide authoring types. |
| `App.scope`, `files`, `media`, `settings`, `runtime` | Native owner and capability facades for this scope. |
| `App.ready`, `status` | Await the validated manifest or observe root loading, ready, failed and disposed states. Shared by child apps. |
| `App.operation<I, O>(id)` | Resolve a registration from the connected manifest. Caller type parameters do not replace compiled schema validation. Prefer generated bindings for inferred DTOs. |
| `App.bindOperation<I, O>(definition)` | Bind a generated registration and check execution identity and schemas against the connected host. |
| `App.collection(items?)` | Own a small local immutable collection; native file datasets use folder views. |
| `App.createScope()` | Return a child app immediately. Native work waits for child-scope admission. |
| `App.dispose()`, `disposeAsync()` | Begin teardown synchronously, or await resource and native scope cleanup. |
| `App.cleanupErrors` | Observe classified resource and native teardown failures. |
| `confirm(message)` | Await a native Yes/No dialog. Only Yes returns `true`; cancellation returns `false`, and a dialog failure rejects. Generated hosts permit message dialogs for their main window. |
| `Scope.ready`, `id`, `parent`, `transport`, `disposed` | Advanced native ownership state; read `id` after readiness. |
| `Scope.assert()`, `own(resource)`, `release(resource)` | Check local lifetime, register idempotent cleanup, or forget a released resource. |
| `Scope.child()`, `request(request, updates?)` | Create an owned native child or dispatch a scoped capability after startup. Updates are direct snapshots. |
| `Scope.dispose()`, `disposeAsync()`, `cleanupErrors` | Close registered owners in reverse order, await their cleanup, then dispose the native scope. |

Class constructors are documented low-level integration APIs. Application code
should use `createApp`, `folder.files`, `files.selection`, operation submission,
preview admission and settings definition to acquire native owners. Constructing
a projection does not independently authorize a folder path or resource handle.

## Files and selections

| API | Behavior and ownership |
| --- | --- |
| `Files.openFolder()` | Admit a picked native folder; cancellation maps native null to `undefined`. |
| `Files.reopenBookmark({path})` | Readmit a saved location through native folder restoration. |
| `Files.checksum` | Compiled `files.checksum` operation; schemas come from the manifest. |
| `Folder.owner`, `descriptor`, `name`, `path`, `bookmark` | Scoped identity and display/restoration metadata. `bookmark` returns a defensive copy. Paths do not authorize file reads. |
| `Folder.files(options?)` | Begin a scoped native index query. Supports recursive traversal, search, sort and descending order. |
| `Folder.dispose()` | Close its views and folder owner; admitted native task leases retain their own lifetime. |
| `FileCollection.subscribe`, `snapshot`, `items` | Observe one bounded viewport and coherent scan metadata. Never a complete file inventory. |
| `FileCollection.ready`, `loaded` | Await the latest query admission or scan completion. Both can complete while internal page ACK work remains pending. |
| `FileCollection.owner`, `id`, `assert()` | View owner, native identity after readiness and lifetime check. |
| `FileCollection.window(offset, limit=128)` | Serialize bounded viewport requests, coalesce superseded calls, and drain known ACK publications before admission. Limits are 1..512. |
| `FileCollection.query(options)`, `filter(search)`, `sort(key, descending?)` | Replace the native view and advance the client query generation. Filter/sort preserve the other current options. Query changes clear explicit selections. |
| `FileCollection.selection()` | Own an explicit native selection of at most 512 files. |
| `FileCollection.allMatching()` | Capture this query generation without enumerating its matches in JavaScript. |
| `FileCollection.selector(generation)` | Advanced selector resolution; returns `{view, allMatching:true, generation}` or rejects a changed query. |
| `FileCollection.protectHandles(promise)` | Advanced capture/admission barrier preventing page ACK from releasing earlier handles until settlement. |
| `FileCollection.dispose()` | Reclaim pending view creation, viewport requests, received page ACKs and retired views. |
| `FileSelection.subscribe`, `items`, `status`, `ready` | Observe bounded selected metadata and native capture readiness/failure. |
| `FileSelection.set(items)`, `toggle(entry)`, `clear()` | Serialize native lease capture. Inputs must be current viewport capabilities or already selected capabilities. Paths are only local row identities. |
| `FileSelection.has(entry)` | Check the selected view-relative row identity for UI presentation. |
| `FileSelection.run(operation)` | Return a batch task after pending capture. Later selection edits wait for native task admission to freeze its leases. |
| `FileSelection.allMatching()` | Checked alias for the collection's complete current query, independently of explicit selected items. |
| `FileSelection.dispose()` | Await pending capture/admission and release native selection ownership. |
| `AllMatchingSelection.run(operation)` | Return a native batch task. Host validates the captured query generation, awaits enumeration and freezes matching inputs. |

File snapshots contain `view`, `revision` and `indexGeneration` separately from
the client query `generation`. Every returned or streamed page gets an internal
`view.ack` after capture/admission barriers, including stale pages. ACK may publish
another coalesced Channel page. Native code owns at most two windows per view
until ACK. An initial short page grows through native scan refreshes; the client
does not mistake its current item count for the complete dataset. See
[WIRE.md](WIRE.md) for publication and viewport admission ordering.

## Operations, tasks and results

| API | Behavior and ownership |
| --- | --- |
| `Operation.id`, `definition`, `owner` | Stable registration ID, compiled definition after lazy connection and task admission owner. |
| `Operation.run(input)` | Return a task immediately; native readiness and compiled input/output validation are automatic. |
| `Operation.call(input)` | Await one successful result and dispose its task in `finally`. |
| `Operation.query()` | Own an explicitly loaded, replaceable read projection; no requests run until `load`. |
| `OperationInput<T>`, `OperationOutput<T>` | Infer a generated operation's compiled input and output types. |
| `Query.subscribe`, `snapshot` | Observe `{status: idle/loading/ready/failed, data?, error?}`; previous data survives reloads and failures. |
| `Query.load(input)` | Resolve after this load settles and its task is reclaimed. Current failures become state; stale successes and failures cannot publish. Loading a closed query rejects. |
| `Query.invalidate()` | Synchronously suppress pending results and clear current error before submitting a debounced replacement. Retain the previous data. |
| `Query.dispose()` | Freeze publication and await all pending task cleanup. Scope teardown does this automatically; cleanup failures remain in scope `cleanupErrors`. |
| `Operation.runBatch(inputs)` | Native JSON-record lane, bounded to 512 inputs and one MiB of encoded input values. |
| `Operation.runSelection(selector)` | Advanced native batch submission through a captured ID, generation-aware all-matching selector or bounded JSON inputs. |
| `Operation.forScope(scope)` | Rebind the same compiled registration within the same native transport. |
| `operationFactory(manifest, id, bind)` | Resolve a generated registration without redeclaring its schema. |
| `Task.subscribe`, `snapshot`, `owner` | Observe native lifecycle independently of UI subscriber count. |
| `Task.ready`, `result`, `terminal` | Await native admission, await a terminal result, or inspect terminal state. Batch success/partial returns `BatchResult`; failed/cancelled/interrupted rejects. |
| `Task.cancel()`, `refresh()` | Request cancellation or inspect current native state. Cancellation does not fabricate completion. |
| `Task.results(offset=0, limit=128)` | Read retained outcomes during execution or after partial completion/cancellation, at most 512 per page. |
| `Task.dispose()` | Release native task/result ownership and reject a pending result consumer. |
| `BatchResult.page(offset=0, limit=128)`, `pages(limit=128)` | Validate successful outputs and iterate bounded pages without retaining the whole result set. Keep the task alive while consuming results. |

Outcome ordinals, task progress and summary counts are decimal strings. Result
offsets and totals are safe integer row positions, independent of source ordinals.
Offsets can shift while completions arrive; complete sequential export belongs
after task termination. Output-resource leasing remains a host concern and does
not add another client API.

## Settings, previews and providers

| API | Behavior and ownership |
| --- | --- |
| `SettingsFactory.define(defaults, {key})` | Infer a typed local settings shape and load native application persistence. |
| `Settings.subscribe`, `value`, `ready`, `status` | Observe immutable values, get a defensive copy, await load and inspect load/save state. Edits made during loading are retained. |
| `Settings.set(value)`, `update(fn)`, `flush()` | Validate the defaults shape, queue serialized/coalesced native saves or await durability. Failed saves retain the value for explicit retry. |
| `Settings.dispose()` | Stop local changes and await already queued native saves. |
| `Media.readMetadata` | Compiled `media.readMetadata` operation; native DTO/schema remains authoritative. |
| `Media.preview(fileOrHandle)` | Admit a full file capability and return an owned native preview. |
| `Preview.descriptor`, `name`, `kind`, `mime`, `url`, `text`, `truncated`, `disposed` | Trusted display metadata and bounded content. URLs are host-formatted and rendered verbatim; URL/text becomes unavailable after disposal. |
| `Preview.dispose()` | Revoke the native preview token and its leases. |
| `Runtime.subscribe`, `snapshot`, `inspect()` | Observe the most recent explicit native registry inspection or refresh it. |
| `Runtime.configure(descriptor, config)` | Validate the typed descriptor's configuration schema and request a native transaction, then refresh inspection. |
| `Runtime.replace(provider, replacement)` | Select compiled provider IDs or descriptors for native transactional replacement, then refresh inspection. |
| `Runtime.dispose()` | Release its local inspection subscriptions. Native providers belong to the host registry. |

Provider fields follow the SDK's `ProviderDescriptor` and `ProviderSnapshot`:
`id`, `description`, `dependencies`, `configurationSchema`, and snapshot `config`,
numeric `generation`, decimal-string `activeTasks`/`resources`, and `mutable`.
Resource counts include leased resources; they count items, not bytes.

## Local collections and common contracts

`Collection<T>` exposes `owner`, `subscribe`, `snapshot`, `items`, `loaded`,
`assert`, `set`, `append`, `ready`, `fail`, `onDispose`, `selection`, `filter`,
`sort` and `dispose`. Derived local collections track parent changes and share
its scope. Keep these application datasets small. `Selection<T>` exposes
`subscribe`, `items`, `set`, `clear`, `run` and `dispose`; local membership uses
object identity, and `run` uses the bounded native JSON-record lane.

Public DTOs and unions are documented in source: `Json`, `JsonSchema`,
`RuntimeError`, `Handle`, `FileEntry`, `TaskState`, `Progress`, `NumericProgress`,
`Outcome`, `LegacyOutcome`, `TaskSummary`, `TaskSnapshot`, `ResultPage`,
`TypeDefinition`, `OperationDefinition`, `ContractManifest`, `AppOptions`,
`AppStatus`, `CollectionSnapshot`, file/folder/selection models, preview models,
provider models, settings models, `Disposable`, `Readable`, `Unsubscribe`, and
native transport/selection/dispatcher types. [WIRE.md](WIRE.md) covers every
admitted dispatcher request, reply and direct Channel update.

`CapabilityError` is the throwable structured failure; `toJSON` preserves its
classification and retry flag. `errorOf` normalizes native or JavaScript failures.
`progressOf` validates canonical u64 decimal strings or safe numeric progress.
`validator` compiles a generated schema and raises classified contract violations.
`validateManifest` checks protocol, registration uniqueness, compiled schemas and
source metadata before returning a defensive manifest copy. `TauriTransport`
implements `NativeTransport.dispatch` through `revenant_dispatch` with an optional
direct snapshot Channel. `NativeRequest` is the extensible transport boundary;
`DispatchRequest`, `DispatchReplies`, `DispatchReply` and `DispatchUpdates` are
the precise documented protocol shapes for host integration.

## Svelte context and optional UI

The `/svelte` entry exports `createApp`, `setupApp`, `provideApp` and `useApp`; call these
during component initialization. `createApp` provides a root and closes it on
unmount. `provideApp(existing)` installs context without taking root ownership.
`setupApp(existing)` provides a root and owns its teardown. The generated facade
instead exposes `setupApp(options?)`, which creates its typed root before
providing it. Use that generated helper in application layouts; core `createApp`
remains available outside Svelte initialization without installing context.
`useApp(parent?)` owns a child of an explicit parent or the nearest context and
provides that child to descendants. Nested components therefore follow native
scope ancestry. Component disposal begins asynchronously; use the core app's
`disposeAsync` where completion must be awaited.

The `/ui` entry uses the available HyvUI 1.0.0 components and the actual
`@tanstack/svelte-virtual` adapter. It exports:

| Component | Props and lifetime |
| --- | --- |
| `CollectionView` | Required `collection`; optional `selection`, `row(entry, absoluteIndex)` snippet, `onactivate(entry)`, `height='28rem'`, `rowHeight=48` (minimum 24), `label='Files'`. Fixed rows, overscan and native bounded windows. Displays scan/capture/request failures and allows explicit viewport retry. |
| `TaskStatus` | Required task store with `cancel()` and `terminal`; optional `label='Operation'`. Observes native state and requests cancellation without owning task disposal. |
| `ErrorNotice` | Optional `error`, `title`, `onretry`. Presents structured failures; retry is offered only when the failure is retryable. |
| `FilePreview` | Optional owned `preview` and accessible `name` override, defaulting to the native preview name. Renders bounded text or native media URLs without taking disposal ownership. |

Import `@hyvnt/hyvui/styles.css` in the application. Core imports do not require
HyvUI or install a theme. Client source checks are `npm run check` and
`npm run check:ui` inside this package. The repository sidecar documentation gate
is `node scripts/check-api-docs.mjs`. `npm test` exercises native fake-transport
workflows, while `npm run check` also checks native provider and operation typing.
Actual Tauri bridge and large-index acceptance remain separate from this suite.
