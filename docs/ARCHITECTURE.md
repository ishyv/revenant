# Revenant 0.3 architecture

The application is a native desktop executable with a Svelte webview. Tauri is
the window and IPC adapter; it does not define the operation authoring API.
Rust owns resources and execution; the client owns scoped observable projections.
There is no active WASM worker or HTTP server host.

## Source ownership

| Package/module | Responsibility |
| --- | --- |
| `src/config.rs`, `src/commands/` | Strict version 3 configuration and CLI orchestration |
| `src/scaffold/`, `templates/` | Minimal author files, local SDK, hidden desktop bootstrap |
| `src/compiled.rs` | Native contract export, validation, typed facade, atomic publication |
| `crates/revenant-core/` | Errors, wire-safe contracts, leases and task lifecycle; no Tauri dependency |
| `crates/revenant-sdk/src/application.rs` | Inert `Application` composition, operation factories and compiled replacements |
| `crates/revenant-sdk/src/operations/`, `providers/` | Typed registration/dispatch and transactional provider lifecycle |
| `crates/revenant-macros/` | Contract and operation macros; source metadata and Rustdoc extraction |
| `crates/revenant-desktop/src/files/` | SQLite folder index, native filters/sort and frozen selections |
| `crates/revenant-desktop/src/results/` | Disk-backed batch outcomes and bounded pages |
| `crates/revenant-desktop/src/runtime/` | Scoped admission, views, native jobs, shutdown and IPC dispatch |
| `crates/revenant-desktop/src/settings.rs` | Bounded validated JSON and atomic persistence |
| `crates/revenant-desktop/src/previews.rs`, `file_port.rs` | Owned previews and leased chunk reads |
| `packages/client/src/` | Typed scoped projections, stores, operations and Tauri transport |
| `packages/client/src/ui/` | Optional Svelte/HyvUI presentation and virtualized collection views |

The native adapter is integrated. Compilation, strict Rustdoc, installed-window
workflows and the large-folder run are recorded in [validation](VALIDATION.md).

## Authoring and compiled contract

An app starts with `revenant.toml` version 3 and `web/`. Built-ins need no native
source. If `native/Cargo.toml` exists, its library exports
`pub fn app() -> revenant::Application` using the same local SDK snapshot as the
generated host. Macros derive schemas, TypeScript types, descriptions and source
metadata from ordinary Rust definitions across modules.

`.revenant/desktop/` contains framework-owned Cargo/Tauri configuration and
bootstrap. It exports the contract before creating a window or acquiring native
resources. The CLI compiles this executable and runs `--revenant-contract`; it
does not infer APIs by parsing handwritten source. Version 3 / protocol 2,
operation IDs, supported definitions and the digest are validated. An immutable
generation beneath `web/src/lib/.revenant/` is selected by atomic replacement
of `web/src/lib/revenant.ts`. Failed compile/export keeps the previous facade.

The generated facade supplies the expected manifest. Root connection checks the
native manifest/digest before execution. Operation bindings also check schemas;
stale generated code fails with `incompatible_contract`. Native rebuilds replace
the executable generation; Vite handles frontend changes.

## Native ownership and bounds

Folders are scoped native capabilities. SQLite holds metadata instead of a JS
inventory or full Rust vector. Windows default to 128 rows, bounded to 512.
Sorting/filtering operate over the native index. Query generations reject stale
updates. Explicit selections are limited to 512 leased entries. `allMatching()`
captures a native query selection for larger work, rather than an array of files.

Admission captures inputs and provider generations before returning a task.
Bounded queues/workers limit execution. Batch outcomes persist to SQLite with
pages up to 512 rows. Snapshots carry progress and summary counters instead of
an ever-growing outcome inventory. Application options also bound scopes,
views, history, JSON size and shutdown grace. A real 500,000-file run verified
bounded windows, paged outcomes and cleanup; timings are workload-specific.

The presentation virtualizer receives at most 4,096 rows at a time. First/last,
adjacent-range controls and a position slider navigate the complete native query.
This bounds measurement caches and avoids WebView2's finite CSS height limit;
one folder-wide scroll spacer would make sufficiently distant files unreachable.

Scope teardown requests cancellation and releases owned resources. Executors
retain captured leases until exit. Subscriptions observe tasks; unsubscribing
does not stop execution. Cancellation is cooperative, with no universal rollback.

## Persistence and replacement

Settings live in native application data. Keys are validated, records bounded
to 1 MiB, and missing defaults merge without discarding unknown object fields.
Writes sync a same-directory temporary file and atomically overwrite the record,
including on Windows. Atomic visibility is not a promise of directory-entry
survival through power loss. Separate processes are last-writer-wins. Settings
are not a secret store.

Inspection exposes supported provider descriptors, configuration and ownership
counts. Configuration/replacement validate schemas, operation sets, dependencies
and idle ownership boundaries. Preparation succeeds before activation; failed
candidates are cleaned up and the old provider stays active. Alternatives must
already be compiled into the app. JavaScript is not an execution provider.

See [authoring](AUTHORING.md), [implementation](IMPLEMENTATION.md), and
[validation](VALIDATION.md). Removed host contracts remain in
[historical 0.2 documents](history/IMPLEMENTATION-0.2.md).
