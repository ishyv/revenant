# Revenant 0.4 architecture

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
| `src/inspection/`, `scripts/inspection-semantic.mjs` | Shared source/build/runtime evidence and Rust/TypeScript/Svelte semantic queries |
| `crates/revenant-agent/` | Official MCP stdio adapter and authenticated current-user local bridge |
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
source. If `native/Cargo.toml` exists, `revenant::app!()` exports its application
and discovers annotated functions in that crate through linker registrations.
Dependency operations require explicit inclusion. The generated host uses the
same local SDK snapshot. Macros derive named input records, schemas, TypeScript
types, descriptions, declared execution policy and source metadata from Rust.

`.revenant/desktop/` contains framework-owned Cargo/Tauri configuration and
bootstrap. It exports the contract before creating a window or acquiring native
resources. The CLI compiles this executable and runs `--revenant-contract`; it
does not infer APIs by parsing handwritten source. Manifest 6 / protocol 5,
operation IDs, supported definitions and the digest are validated. An immutable
generation beneath `web/src/lib/.revenant/` is selected by atomic replacement
of `web/src/lib/revenant.ts`. Failed compile/export keeps the previous facade.

The generated facade supplies the expected manifest. Root connection checks the
native manifest/digest before execution. Operation bindings also check schemas;
stale generated code fails with `incompatible_contract`. Native rebuilds replace
the executable generation; Vite handles frontend changes. Generated
`useOperations()` functions share component ownership and retain `.run()` task
control; `.query()` exists only for declared reads.

Source fingerprints, independent native-build and check attempts, generation
metadata and running-instance identity make freshness explicit. Failed rebuilds
retain the older executable and facade while exposing current diagnostics.
Rust-analyzer and TypeScript/Svelte services establish semantic relationships;
unavailable services, bounded indexing and dynamic references remain explicit.
Bounded traces correlate frontend request identities with native tasks without
recording arguments or results.

Development MCP exposes project/code/diagnostic checks separately from packaged
application tools. An installed executable's `--mcp` mode attaches to an existing
app through protected local IPC; author-exposed operations are enforced natively.
Each connection owns its tasks and resources. Disconnect requests cancellation;
captured leases survive until execution ends. See the [migration](MIGRATION-0.4.md)
for configuration, tool behavior and compatibility boundaries.

The MCP adapter monitors input independently of SDK initialization and output.
Frames remain bounded to 1 MiB; a saturated 2 MiB read-ahead queue closes the
connection with a transport error instead of accumulating unlimited input or
waiting behind an unread response. Terminal input starts backend cleanup
concurrently with a one-second response grace. Output that resumes during grace
can complete; after grace, pending writes are interrupted and the SDK is awaited.
An output failure starts teardown even if the peer keeps stdin open. Terminal
I/O errors remain observable when SDK completion races their notification.

Native stdin/stdout use owned, dedicated workers rather than Tokio's blocking
pool. Unix workers use nonblocking descriptors and finite polling; Windows
workers use synchronous pipe I/O with repeated thread-specific cancellation.
Both workers are joined before `serve_stdio` returns, including when stdin is
still open after an output failure. MCP exclusively owns standard I/O while
serving; Unix descriptor flags are restored after both workers have joined.
This bounds pipe-related shutdown, not arbitrary backend cleanup or application
work. Cancellation still preserves the backend's resource and task lifetimes.

## Native ownership and bounds

Folders are scoped native capabilities. SQLite holds metadata instead of a JS
inventory or full Rust vector. Windows default to 128 rows, bounded to 512.
Sorting/filtering operate over the native index. Query generations reject stale
updates. Explicit selections are limited to 512 leased entries. `allMatching()`
captures a native query selection for larger work, rather than an array of files.

Admission captures inputs and provider generations before returning a task.
Bounded queues/workers limit execution. Declared-independent batch operations
overlap within the shared worker budget; unmarked operations remain sequential.
The declaration is an author's scheduling promise, not a proof of purity.
Batch outcomes persist to SQLite with
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

The [native workflows API](native-workflows.md) documents issued ownership,
prepared contracts and projections, bounded native work, grouped reads, latest
jobs, awaited covers and incremental graphics. The
[milestone plan](plans/2026-10-09-native-workflows-milestone.md) defines the gates;
[local integration evidence](research/2026-10-09-native-workflows-integration.md)
records implementation, the compute no-go result and remaining platform/application
evidence. Strata migration and release publication remain separate work.
