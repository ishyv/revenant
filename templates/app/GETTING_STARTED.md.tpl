# {{project_name}}

Revenant 0.3 builds a native desktop app with a static SvelteKit SPA. Edit
`revenant.toml` and `web/src/routes/+page.svelte`, then run `revenant dev` from
this folder. Vite supplies HMR inside the desktop window. Closing the window or
pressing Ctrl+C stops all processes owned by this CLI invocation.

## Your files and generated files

`web/` and `revenant.toml` are yours. `.revenant/desktop` is the generated Tauri
host; `.revenant/sdk` contains the CLI's embedded SDK, client and HyvUI snapshot.
Do not customize the generated bootstrap. It exports the compiled application's
contract before creating any window. `web/src/lib/revenant.ts` points at an
immutable generated facade, with Rustdoc and source links for each operation.
An unsuccessful compile or export leaves the previous facade available.

The CLI adds `.zed/settings.json` discovery for `.revenant/desktop/Cargo.toml`
and, when present, `native/Cargo.toml`. It preserves existing linked projects,
other settings and comments. Narrow scan inclusions make the hidden host and SDK
sources available to Zed without indexing Cargo outputs. Explicit project-panel
visibility preferences are preserved. Invalid settings are left untouched and
reported without blocking a build.

## Optional Rust capabilities

You do not need Rust application source to use built-in files, media, settings
and tasks. To add operations, create `native/Cargo.toml` and `native/src/lib.rs`:

```toml
[package]
name = "{{project_name}}-native"
version = "0.3.0"
edition = "2024"

[workspace]

[dependencies]
revenant = { package = "revenant-sdk", version = "0.3.0", path = "../.revenant/sdk/crates/revenant-sdk" }
```

```rust
use revenant::{Application, operations};
mod records;

/// Register the application capabilities; Revenant supplies the desktop runtime.
pub fn app() -> Application {
    Application::new().operations(operations![records::normalize])
}
```

For example, put this in `native/src/records.rs`:

```rust
use revenant::{contract, operation, Result, TaskContext};

/// A record whose title is normalized without changing its identity.
#[contract]
pub struct Record {
    /// Stable application identity.
    pub id: String,
    /// Human-readable title.
    pub title: String,
}

/// Trim and uppercase a record title.
#[operation(id = "records.normalize")]
pub async fn normalize(input: Record, context: TaskContext) -> Result<Record> {
    context.checkpoint()?;
    Ok(Record { title: input.title.trim().to_uppercase(), ..input })
}
```

Declare payloads with `#[revenant::contract]` and operations with
`#[revenant::operation(id = "records.normalize")]`. Rust owns validation, public
types and operation documentation. Use `TaskContext` for cancellation and
progress. Return the application from `app()` without writing a `main`, Tokio
bootstrap, target cfgs, or registry/export glue. Restart `revenant dev` after
adding `native/`; later source edits rebuild automatically.

The layout creates and provides the generated app. Components import `useApp`
from `$lib/revenant` and use typed `app.operations.records.normalize.run(input)`.
Await `app.ready` before capability calls. Give resources and tasks a component
scope and dispose it on teardown. Built-ins, including `files.checksum`, are
already registered: do not register duplicate operation IDs.

## Native folders and tasks

Use `app.files.checksum` and `app.media.readMetadata` for typed built-ins. Their
types, validation schemas, field documentation and source links come from the
same compiled manifest as your custom operations. For a native folder batch:

```ts
const folder = await app.files.openFolder();
if (folder) {
  const files = folder.files({ recursive: true, search: '.txt' });
  const task = files.selection().allMatching().run(app.files.checksum);
  const results = await task.result;
  const firstPage = await results.page(0, 128);
  console.log(firstPage.counts, firstPage.items);
}
```

Use `CollectionView` from `@revenant/client/ui` to render bounded native windows.
`allMatching()` selects in the host without copying file handles into JavaScript;
`files.selection()` creates a bounded explicit selection. Task counts and file
sizes use decimal strings so 64-bit values remain exact. Inspect partial results
with `task.results(...)` after cancellation or failure, and dispose the task
after consuming its results. `app.media.preview(file)` returns an owned native
preview; dispose it before replacing it. Put a chosen folder in a child scope
and dispose that scope when replacing the folder.

## Configuration

Optional `[desktop]` keys are `title`, `width`, `height` and `dev_port` (5173 by
default). `revenant dev` supplies that port to Vite with strict port selection.
Restart dev after changing it. `[toolchain] pkg_manager` accepts npm, pnpm, yarn
or bun. The SPA output belongs in `build/web`; `frontendDist` is generated from
that location. Set `[project] identifier` before distributing to give native
settings and installed packages a stable identity.

## Packaging

`revenant build` compiles the native host, extracts and publishes its contract,
builds `build/web`, then invokes Tauri 2 to embed the frontend and package the host.
Before bundling it re-exports the final executable's contract and checks that it
matches the generated UI. A contract edit during a build requires a fresh build.
Artifacts live under `.revenant/target/release/bundle`. Windows builds produce
NSIS installers with the offline WebView2 installer bundled. Vite is a dev tool;
it is not a production server. A built app runs without Node or this checkout.

Run `revenant setup` for read-only toolchain diagnosis. Install prerequisites
explicitly using the links it reports. The first build needs network access to
resolve Cargo/Node dependencies and packaging tooling. Tauri's platform
prerequisites: https://v2.tauri.app/start/prerequisites/.

## Migration boundary

Version 2 and unversioned WASM/server configs are rejected. Copy UI into a new
version 3 app, move Rust operations to an optional `native/` library, and remove
`wasm_target`, server features and server sections. Switching the version number
alone does not migrate transport, registration or lifecycle ownership.
