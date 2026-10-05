# Authoring desktop applications

Write ordinary Svelte. The generated `$lib/revenant` facade supplies the compiled
native contract, typed operation groups and scoped helpers. Use TypeScript for
view behavior. Rust owns native execution and resources.

## Minimal application

```toml
version = 3
[project]
name = "my-app"
```

The scaffold includes a root layout:

```svelte
<script lang="ts">
  import { setupApp } from '$lib/revenant';
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
  import { useApp } from '$lib/revenant';
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

For replaceable reads, use `operation.query()`. Its store exposes
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
revenant = { package = "revenant-sdk", version = "0.3.0", path = "../.revenant/sdk/crates/revenant-sdk" }
```

```rust
//! Application-specific native operations.
use revenant::{Application, Result, TaskContext, contract, operation, operations};

/// A title to normalize.
#[contract]
pub struct Title {
    /// Human-readable title text.
    pub text: String,
}

/// Trim surrounding whitespace from a title.
#[operation(id = "records.normalize")]
pub async fn normalize(input: Title, context: TaskContext) -> Result<Title> {
    context.checkpoint()?;
    Ok(Title { text: input.text.trim().to_owned() })
}

/// Compose custom operations with native defaults.
pub fn app() -> Application {
    Application::new().operations(operations![normalize])
}
```

Register module paths with `operations![records::normalize]`. Rustdoc and source
locations feed generated TypeScript hovers; input/output types are defined once.
After native rebuild, use `app.operations.records.normalize.run(...)` from the
facade. Authors do not write `main`, Tauri commands, IPC DTOs, contract-export
handling, schedulers, or per-item batch loops.

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

`app.media.preview(entry)` returns an owned preview; dispose it when hidden.
Native handles authorize reads. Display paths are not authorization tokens.

`app.runtime.inspect()` exposes provider state. `configure(descriptor, config)`
validates configuration. `replace(provider, replacement)` selects a compiled
native alternative. Rust `Application::capability` composes providers and
`Application::replacement` registers factories. Busy dependencies, owned resources
and incompatible contracts reject replacement. Failed preparation keeps the old
provider. JavaScript executor callbacks and arbitrary Rust hot reload are unsupported.

## Cancellation and cleanup

`task.cancel()` requests cancellation. Observe terminal state to know execution
stopped; cancellation does not promise rollback. Custom Rust must yield and
checkpoint. Executors retain leases until exit. `dispose()` starts teardown;
`disposeAsync()` waits for scope cleanup. Shutdown stops admission and waits
within a configured grace period. Arbitrary native code cannot safely be
force-stopped. Final Windows close/rebuild behavior remains an acceptance check.

## Migrating 0.2 and earlier projects

Version 2/unversioned apps fail before generation or process startup. Unknown
configuration keys also fail; the legacy bridge is removed. Create a desktop
scaffold, copy Svelte UI into `web/`, and move custom Rust to optional `native/`
with `app()`. Remove `wasm_target`, `app.server`, server sections, WASM imports,
HTTP upload/session code and worker/provider callbacks. Changing only the version
number does not migrate contracts or ownership.

Replace browser inventories with native windows/selections, browser storage with
keyed native settings, and HTTP transport with generated desktop projections.
Old SDK snapshots fail the 0.3 guard. Move them aside and regenerate after saving
local changes. Built-ins need no custom Rust. See [architecture](ARCHITECTURE.md)
and [validation](VALIDATION.md).
