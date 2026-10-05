# Revenant 0.3 approved design

The active plan is the desktop-only ownership and authoring redesign tracked in
Beads `revenant-gl5`. The earlier `revenant-2jz` implementation and recon remain
historical. [September 30 design](history/DESIGN-2026-09-30.md),
[0.2 implementation](history/IMPLEMENTATION-0.2.md) and
[0.2 validation](history/VALIDATION-0.2.md) preserve their findings; their
web-first host defaults and APIs are superseded.

## Product promise

Make useful native applications easier to express and change. Ordinary Svelte
handles views. Typed Rust capabilities own filesystem access, jobs, resources,
persistence and replacement. Tauri supplies the desktop window/IPC. HyvUI is
replaceable presentation. Python/ezsgame remain ergonomic references for less
handwritten glue and discoverable capabilities, not embedded runtimes.

A minimal app consists of configuration and Svelte. Built-in files, media,
checksums and settings need no handwritten Rust. Custom native behavior is an
ordinary library exporting `app() -> Application`; macros define each operation
and contract once. The framework owns bootstrap and generated bindings.

## Ownership requirements

| Boundary | Required behavior |
| --- | --- |
| Folder inventory | SQLite owns the full index; Svelte receives bounded windows and metadata |
| Selection | Explicit rows are bounded and acknowledged; all-matching query selection stays native |
| Jobs | Native admission captures inputs/provider generations; bounded queues/workers execute |
| Outcomes | Batch results persist natively and are paged; UI snapshots retain bounded state |
| Cancellation | Request and acknowledged terminal outcome differ; partial effects remain accountable |
| Settings | Native validated bounded JSON; synced tempfile with atomic replacement |
| Runtime customization | Schema/dependency/ownership checks and candidate rollback; compiled alternatives only |
| Build publication | Facade generated from the compiled native manifest; failed stages retain the prior generation |
| Startup guard | Generated and connected native contracts must agree before operations execute |
| Lifecycle | Component scopes release owned resources; active executors retain leases until exit |

The earlier source-export parser, browser inventory, worker broker, HTTP session
and upload model are removed from the active product. No hidden fallback host
or handwritten transport-registration layer is part of the authoring model.

## Discoverability and acceptance

Rustdoc and TypeScript JSDoc describe public APIs, including interface fields,
methods, types, subscriptions and exported components. Operation Rustdoc/source
metadata appears in generated hovers. Modular packages have explicit ownership;
large folder/result modules remain storage boundaries rather than being folded
into transport or application code.

Acceptance requires a useful minimal app, the file/media workflow, an ordinary
record operation, runtime inspection/replacement, cooperative cancellation and
accountable shutdown. Actual 500,000-file behavior must demonstrate bounded
native/UI memory, query/selection correctness and usable cancellation. Zed must
resolve Rust/TypeScript hovers and source navigation. An installed Windows
package must run independently of Vite, Node, Cargo and the CLI checkout.

These are criteria, not claimed results. Implementation and compile/docs checks
come first. Formal tests are the final phase, tracked with Windows/editor/scale
acceptance in `revenant-gl5.7`. See [validation status](VALIDATION.md).
