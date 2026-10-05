# Revenant

A CLI that scaffolds and drives native Rust + SvelteKit applications. Write
logic in Rust and consume it from Svelte 5 — one tool manages the entire build
loop, including a generated TypeScript facade for native operations.

<div align="center">
  <img src="./imgs/image.png" alt="Revenant" width="200"/>
  <img src="./imgs/image2.png" alt="Revenant" width="200"/>
</div>

## Why

Setting up Rust with SvelteKit for desktop means wiring together Tauri,
Vite, native bindings, and a file watcher — then keeping them in sync.
Revenant manages the build loop, generates bindings from the compiled Rust
contract, and packages the frontend with the native executable.

Filesystem access, media previews, checksums, and settings are built-in
capabilities. Custom operations live in an optional Rust library. The frontend
uses the same typed API for both.

## Requirements

- **Rust** with `cargo`
- **Node.js** with `npm`
- **Native desktop prerequisites** — MSVC C++ Build Tools and WebView2 on Windows

Run `revenant setup` to check tool availability and platform prerequisites.

## Quick Start

```sh
cargo install --path . --locked
revenant setup
revenant new my-app
cd my-app
revenant dev
```

`revenant dev` opens the application in a Tauri window with Vite serving the
frontend. Svelte changes reload the interface; Rust changes rebuild the native
host and regenerate bindings.

## Commands

### `revenant new <name>`

Scaffolds a SvelteKit application, materializes a local SDK snapshot, and
generates the Tauri host. Installs frontend dependencies, compiles the native
application, and generates its TypeScript facade.

### `revenant dev`

Runs an initial native build, starts Vite and the desktop window, and watches
native source, configuration, and local SDK dependencies. Failed rebuilds
preserve the running application and its previous bindings. Ctrl+C stops the
owned processes.

### `revenant build`

Compiles the native application, exports its contract, builds the static
frontend, and packages the desktop application. The final executable's contract
is checked against the generated facade before bundling. Windows installers
are produced under `.revenant/target/release/bundle/nsis/`.

### `revenant setup`

Checks the required tools and reports platform-specific installation guidance.

### `--verbose`

All commands accept `--verbose` to show full child-process output.

## Project Structure

```text
my-app/
  revenant.toml             # Project configuration, version = 3
  web/src/routes/           # SvelteKit routes and components
  native/                  # Optional Rust application library
  web/src/lib/revenant.ts  # Generated TypeScript facade
  .revenant/sdk/           # Local SDK source snapshot
  .revenant/desktop/       # Generated Tauri host
```

The generated facade exports `setupApp()` for the layout and `useApp()` for
descendant components. A custom `native/` library exports
`app() -> Application`; operation macros define the contracts used to generate
the frontend API. Applications using only built-in capabilities need no
handwritten Rust.

Examples: [file/media](examples/file-media/README.md),
[prime sieve](examples/prime-sieve/README.md), and
[Luma image notes](examples/image-notes/README.md).

## Architecture

Revenant 0.3 uses Tauri for the desktop window and IPC. Rust owns filesystem
access, jobs, resource lifetimes, and persistence. Folder indexes, large
selections, and batch results stay in native SQLite storage; the client reads
bounded query and result pages. HyvUI is the default, replaceable UI layer.

Resources belong to component scopes. Active operations retain leases until
execution finishes; requesting cancellation does not imply completion.
Generated bindings carry the compiled contract's types, Rust documentation,
and source metadata.

The CLI embeds an SDK snapshot so generated apps can build independently of
this checkout. Keep `.revenant/sdk/` with the application; host preparation
preserves local SDK edits.

Version 2 and unversioned WASM/HTTP configurations are rejected with migration
instructions. Migration requires changes to the host, native library, and
frontend API, not just the configuration version.

- [Authoring and migration](docs/AUTHORING.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Design](docs/DESIGN.md) and [implementation](docs/IMPLEMENTATION.md)
- [Validation and release limits](docs/VALIDATION.md): measured native/UI
  bounds, installed Windows workflows, and editor navigation. Earlier web/HTTP
  results are kept as historical evidence.
