# Revenant

Revenant 0.3 builds native desktop applications with ordinary Svelte and typed
Rust capabilities. Tauri hosts the window; Rust owns filesystem access, jobs,
leases, previews, settings, and provider lifecycle. Svelte observes bounded
projections. HyvUI is the default, replaceable presentation layer.

A minimal application needs Svelte and configuration. Built-in files, metadata,
checksums, and settings require no handwritten Rust. Custom operations belong
in an optional `native/` library; Revenant generates their TypeScript facade
from the compiled application contract.

## Start

Install Rust/Cargo and Node/npm plus the native desktop prerequisites.
On Windows, this includes MSVC C++ build tools and WebView2.
`revenant setup` reports tool availability and prerequisite guidance.

```sh
cargo install --path .
revenant setup
revenant new my-app
cd my-app
revenant dev
```

`new` materializes the local SDK, installs frontend dependencies, compiles the
hidden native host, and generates the facade. `dev` runs Vite and the native
window, rebuilding native source/configuration changes. `build` compiles the
native application, generates its contract, builds the static frontend, and
bundles the desktop application. Windows installers are produced beneath
`.revenant/target/release/bundle/nsis/`. Installed Windows and 500,000-file
validation results are recorded below. Pass `--verbose` for child-process logs.

## Author-owned files

```text
my-app/
  revenant.toml             # version = 3, [project] name = "my-app"
  web/src/routes/           # Ordinary Svelte application
  native/                  # Optional Rust library exporting app() -> Application
  .revenant/sdk/           # Local SDK snapshot; retain with the application
  .revenant/desktop/       # Generated bootstrap, Cargo/Tauri configuration
  web/src/lib/revenant.ts  # Generated compiled-contract facade
```

Use `setupApp()` in the layout and `useApp()` in descendants from `$lib/revenant`.
The explicit `createApp` and `provideApp` helpers remain available. Native folders
provide indexed query windows; large selections and batch results stay native
and are paged into the UI. Child components own child scopes. Cancellation
requests remain distinct from executor completion.

Version 2 and unversioned WASM/HTTP projects are rejected with migration
instructions. Changing the version alone is not a migration. See
[authoring and migration](docs/AUTHORING.md), [architecture](docs/ARCHITECTURE.md),
[approved design](docs/DESIGN.md), and [implementation](docs/IMPLEMENTATION.md).
[Validation](docs/VALIDATION.md) records measured native/UI bounds, installed
offline workflows, language-service navigation, and remaining release limitations.
Earlier web/HTTP results remain labeled historical evidence.
