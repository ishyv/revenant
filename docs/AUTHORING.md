# Authoring desktop applications

Write ordinary Svelte. The generated `#lib/revenant.ts` facade supplies the compiled
native contract, typed operation groups and scoped helpers. Use TypeScript for
view behavior. Rust owns native execution and resources.

Use Node 24 with the pinned SvelteKit 3, Svelte 5, Vite 8 and TypeScript 6
toolchain. Svelte configuration belongs in `sveltekit({...})` in Vite.
Declare `"#lib/*": "./src/lib/*"` in package imports, include extensions in
`#lib` imports, and extend `$app/tsconfig` with explicit source inclusion.
Commit application lockfiles after first installation; subsequent installs use
`npm ci --install-links`. SDK upgrades preserve handwritten frontend configuration.

Application identity may include `project.version = "1.2.3"` and
`desktop.icons = ["assets/icon.png", "assets/icon.ico", "assets/icon.icns"]`.
The version defaults to 0.4.0 independently of SDK compatibility. Icon paths
resolve inside the project root; PNG icons use 8-bit RGBA. Host preparation
copies these assets and retains configured identity on every rebuild. An authored
set must include PNG for the window, plus ICO on Windows or ICNS on macOS. Use
`tauri icon` to generate platform variants; incomplete sets fail before host
metadata changes instead of falling back to the framework's bundled branding.

Packaged CSP permits bundled scripts, native IPC, local previews and media
data URLs. It excludes dynamic code execution and remote scripts. Development
adds connections only to the configured loopback HMR port. Inline styles are
permitted for Svelte and virtualized geometry. Explicit extra sources are additive:

```toml
[desktop.csp_allowances]
connect-src = ["https://api.example.com"]
img-src = ["https://images.example.com"]
```

An allowance applies to both packaged and development policies. Add only the
origins your application needs; script allowances expand executable content.

## Portable models and scheduling

Portable source models can opt into generated wire projections. The domain model
does not need Serde, schema, TypeScript or Clone implementations:

```rust
use revenant_sdk::WireProjection;

#[derive(WireProjection)]
struct Summary {
    entries: u32,
    #[wire(decimal, rename = "byteTotal")]
    bytes: u64,
}
```

This generates `SummaryWire` with the usual contract metadata and handle visitor.
Use `summary.into_wire()` and `Summary::from_wire(wire)` for owned conversions;
both `From` directions are also generated. Fields default to camelCase.
Nested models, Option, Vec and Box compose recursively. `#[wire(decimal)]`
preserves i64/u64 values as canonical strings, including in these containers.
`#[wire(id)]` on a single-field String newtype preserves its named identity.
`#[wire(contract)]` retains an already authored boundary representation.
An optional integration feature with `cfg_attr` keeps standalone engines
independent. Foreign models, enums, generic models and unsupported domain types
retain explicit native adapters; the derive does not inspect foreign source.

Operation scheduling defaults to CPU. Declare `#[operation(work = "io")]`
for blocking I/O or `#[operation(read_only, work = "interactive")]` for reads
that need the reserved CPU slot. The host pins this metadata at admission;
user code cannot upgrade a CPU operation to interactive work later.

See [native workflows](native-workflows.md) for work helpers, immutable host paths,
scope-owned grouped reads, latest native commits, covered dialogs and scene
patches, with separate resource budgets and the experimental compute decision.

## Minimal application

```toml
version = 3
[project]
name = "my-app"
```

The scaffold includes a root layout:

```svelte
<script lang="ts">
  import { setupApp } from '#lib/revenant.ts';
  import type { Snippet } from 'svelte';
  let { children }: { children: Snippet } = $props();
  setupApp();
</script>
{@render children()}
```

A descendant calls `useApp()` during initialization. It gets a component-owned
child scope with automatic disposal and generated custom operation groups.
Lower-level `@revenant/client/svelte` helpers also manage context/scopes, but do
not add generated groups. `app.ready` waits for native connection and contract
validation. Built-in capabilities require no handwritten Rust:

```svelte
<script lang="ts">
  import { useApp } from '#lib/revenant.ts';
  const app = useApp();
  let folderName = $state('nothing selected');
  async function open() {
    const folder = await app.files.openFolder();
    if (folder) folderName = folder.name;
  }
</script>
<button onclick={open}>Open folder</button>
<p>{folderName}</p>
```

Picker cancellation returns `undefined`. Errors retain structured codes/details;
surface them in your UI. HyvUI and `@revenant/client/ui` are replaceable defaults.

## Indexed files, selections and tasks

`Folder.files(options)` starts a native indexed view. Its readable snapshot has
one bounded window, total/discovered counts, scan state and warnings. `items`
contains only the current window. `loaded` waits for scan completion metadata;
it never returns the whole folder as an array.

```ts
const folder = await app.files.openFolder();
if (folder) {
  const files = folder.files({ recursive: true, sort: 'name' });
  await files.ready;
  await files.window(0, 128);
  const task = files.allMatching().run(app.files.checksum);
  await task.ready; // Native admission and input capture acknowledgement.
  // Subscribe to task state, decimal-string progress and summary counts in UI.
}
```

Use `selection()` for explicit rows (maximum 512). The host acknowledges selection
captures before dependent execution; query edits do not retarget admitted work.
Use `allMatching()` for larger query selections. Native iteration and scheduling
stay bounded. Small ordinary records use `app.collection(records)` and the same
operation model.

`operation.run(input)` returns a task; `operation.call(input)` returns its result
promise. Batch results expose `page(offset, limit)` and `pages(limit)`: maximum
512 outcomes, default 128. Retain the task while consuming results; disposal
releases native history/results. Partial/failed item outcomes stay inspectable
while their owner lives. Supported media previews are also owned resources.

For operations declared `read_only`, use `operation.query()`. Its store exposes
`{status, data?, error?}`; `load(input)` retains the previous data while loading
and publishes only the newest result. `invalidate()` immediately suppresses an
old response before a debounced replacement begins. The owning component scope
cleans up pending tasks. Failed reads become query state, so inspect `error`
after awaiting `load`; use explicit `call` for mutations such as saving notes.

Infer DTOs with `OperationInput<typeof operation>` and
`OperationOutput<typeof operation>` from `@revenant/client`. These helpers
preserve the Rust-generated types without handwritten duplicate interfaces.

## Optional custom Rust

Add a `native/` library for application-specific native behavior:

```toml
# native/Cargo.toml
[package]
name = "my-app-native"
version = "0.1.0"
edition = "2024"
[workspace]
[dependencies]
revenant = { package = "revenant-sdk", version = "0.4.0", path = "../.revenant/sdk/crates/revenant-sdk" }
```

```rust
//! Application-specific native operations.
revenant::app!();

/// Trim surrounding whitespace from a title.
#[revenant::operation(id = "records.normalize", read_only, independent, agent)]
pub fn normalize(text: String) -> String {
    text.trim().to_owned()
}
```

Functions can be sync or async, take zero or multiple owned named arguments,
return a value or `revenant::Result<T>`, and optionally receive a trailing
`TaskContext`. Discovery covers this application's crate; dependency operations
require explicit `operations![dependency::function]` inclusion in the builder
passed to `app!(...)`. Keep `#[contract]` for authored domain structs and enums.
Rustdoc and source locations feed generated TypeScript hovers. After rebuilding:

```ts
const api = useOperations();
const title = await api.records.normalize({ text: ' hello ' });
const task = api.records.normalize.run({ text: ' hello ' });
const projection = api.records.normalize.query();
```

`useOperations()` owns one component scope. An unmarked operation is unavailable
to application agents and its batch items run sequentially. `independent` is an
author declaration: eligible items share the global worker budget, with stable
source ordinals and retained leases. It does not prove Rust purity or safe
concurrent access to arbitrary external state. Authors do not write `main`, Tauri commands, IPC DTOs, contract-export
handling, schedulers, or per-item batch loops.

`#[contract]` makes `Option<T>` fields optional by omission, including DTOs
returned inside arrays and maps. For example, `struct Node { id: DecimalU64,
parent: Option<DecimalU64> }` serializes a missing parent as `{ "id": "1" }`.
The macro supplies Serde's default and `Option::is_none` omission and marks the
TypeScript field optional. Existing explicit omission attributes are preserved.
Boundary validation rejects explicit null for optional object fields.

`.revenant/desktop/` is generated bootstrap. Retain `.revenant/sdk/` with the app:
it contains framework/client sources and the pinned UI archive, so dependencies
do not point back to the CLI checkout. Edit authored files, not facade generations.

## Settings and supported customization

`app.settings.define(defaults, { key: 'view' })` infers a readable store. Use
`set`, `update`, `flush`, `ready` and persistence `status`. Native saves serialize
and atomically replace the JSON record. Failed saves retain local values for
retry. Settings are application data, not credentials. Persist folder bookmarks
and reopen through `app.files.reopenBookmark`; restoration readmits native
ownership and may fail if access or the path changed.

Operation inputs, JSON batches, settings, and provider configurations accept
Svelte `$state` objects directly. Revenant captures detached JSON before admission
or saving, so later edits cannot change queued work. JSON restrictions and compiled
schema validation still apply; use decimal strings for integers beyond JavaScript's
safe range.

`app.media.preview(entry)` returns an owned preview; dispose it when hidden.
Native handles authorize reads. Display paths are not authorization tokens.

`app.runtime.inspect()` exposes provider state. `configure(descriptor, config)`
validates configuration. `replace(provider, replacement)` selects a compiled
native alternative. Rust `Application::capability` composes providers and
`Application::replacement` registers factories. Busy dependencies, owned resources
and incompatible contracts reject replacement. Failed preparation keeps the old
provider. JavaScript executor callbacks and arbitrary Rust hot reload are unsupported.

## Cancellation and cleanup

`task.cancel()` requests cancellation. A terminal snapshot settles the logical
result; a host interruption can settle it while native execution still runs.
Cancellation does not promise rollback. Custom Rust must yield and checkpoint.
Executors retain leases until actual exit. `task.dispose()` closes client
publication and awaits native disposal acknowledgement; it does not join native
threads. Disposed task history and result ownership remain until all scheduled
readers exit. `scope.dispose()` starts teardown; `scope.disposeAsync()` waits for
owned disposal requests and scope teardown acknowledgement, not executor exit.
Grouped loads similarly serialize client loads and disposal acknowledgements;
superseded native reads may briefly overlap until they acknowledge cancellation.
Shutdown stops admission and waits
within a configured grace period. Arbitrary native code cannot safely be
force-stopped. Final Windows close/rebuild behavior remains an acceptance check.

Advanced Rust snapshot observers run outside task locks; removing or replacing
their captured values also destroys them outside those locks. Immediate observer
delivery is synchronous. Observer panics propagate during ordinary publication;
execution-guard destruction settles ownership first and contains observer unwinds
so other cleanup deliveries can run. Panic hooks still run. Resource/callback
capture destructors and custom panic-payload destructors must not panic; aborting
panics are not recoverable through this boundary.

## Migrating 0.2 and earlier projects

Version 2/unversioned apps fail before generation or process startup. Unknown
configuration keys also fail; the legacy bridge is removed. Create a desktop
scaffold, copy Svelte UI into `web/`, and move custom Rust to optional `native/`
with `app()`. Remove `wasm_target`, `app.server`, server sections, WASM imports,
HTTP upload/session code and worker/provider callbacks. Changing only the version
number does not migrate contracts or ownership.

Replace browser inventories with native windows/selections, browser storage with
keyed native settings, and HTTP transport with generated desktop projections.
Partial SDKs, incompatible revisions and missing client exports fail before host
generation or contract publication, even when the package version is 0.4.0.
Use `revenant sdk check` to inspect both source and installed snapshots, and
`revenant sdk upgrade` to replace them with a recoverable backup. Compatible local
edits remain usable. See [SDK recovery](SDK.md), [architecture](ARCHITECTURE.md)
and [validation](VALIDATION.md). Built-ins need no custom Rust.

For native computation and viewports, see [typed native resources](native-resources.md),
[typed task updates](task-updates.md), and [native 2D graphics](native-surfaces.md).

## Inspecting the development window

On Windows, set `REVENANT_DEVTOOLS_PORT` before `revenant dev` to expose the
WebView's local debugging endpoint. For example, in PowerShell:

```powershell
$env:REVENANT_DEVTOOLS_PORT = '9333'
revenant dev
```

Connect a Chromium debugger to `http://127.0.0.1:9333`. The CLI accepts an integer
port from 1 through 65535 and supplies the WebView2 launch argument only for its
owned development window. Clear the variable with
`Remove-Item Env:REVENANT_DEVTOOLS_PORT` when finished. This window inspection is
separate from Revenant's compiled-operation inspection and application-agent APIs.
Successful contract export captures its JSON without printing it, including in
verbose mode; compiler diagnostics still appear.
