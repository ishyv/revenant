# Revenant 0.4 migration

The project layout and `revenant.toml` stay at version 3. The compiled manifest
is version 6, the native bridge protocol is version 5, and SDK/client compatibility
revision is 4.
Framework packages are 0.4.0. Old contracts fail compatibility checks before
execution; regenerate the SDK and bindings together.

Run `revenant sdk upgrade` in each application. It backs up the SDK, dependencies,
lockfiles and generated contracts, validates the replacement, and restores the
previous snapshots on failure. Authored native/frontend files remain yours.
Record the returned backup path for `revenant sdk restore <backup>`.

## Rust and Spell

Use `revenant::app!()` at crate root. Module functions with `#[operation]` are
collected by the linker, filtered to the calling package/crate, and sorted before
validation. Duplicate IDs fail manifest export. Dependency operations require
explicit inclusion using `app!(Application::new().operations(operations![dep::op]))`.
An application with custom providers can pass its existing composition builder
to `app!(...)`.

```rust
revenant::app!();

/// Trim surrounding whitespace.
#[revenant::operation(id = "titles.normalize", read_only, independent, agent)]
pub fn normalize(text: String) -> String {
    text.trim().to_owned()
}
```

The wire input is `{ "text": " hello " }`. Named arguments become one owned
record with camelCase keys: `root_id` becomes `rootId`. Zero arguments use `{}`.
An old function `fn normalize(input: Title)` now receives `{input: {...}}`.
Update its callers, decompose the wrapper into named arguments, or explicitly
use `legacy_input` during migration to retain a single raw input. Built-in file
operations preserve their existing `FileEntry` shape with that flag.

Sync/async functions, plain values, `revenant::Result<T>`, and an optional trailing
`TaskContext` share the same generated contract path. Borrowed arguments and
lossy wire integers remain rejected. `#[contract]` remains the domain-type API.
Inferred IDs follow the module/function; use an explicit ID for a stable public API.
Provider replacement preserves the entire compiled operation descriptor, including
argument names, effects, scheduling, exposure, documentation and source identity.
Implementation changes may reuse that descriptor; contract changes require a rebuild.

```ts
import { useOperations } from '#lib/revenant.ts';
const api = useOperations();
const title = await api.titles.normalize({ text: ' hello ' });
const task = api.titles.normalize.run({ text: ' hello ' });
const query = api.titles.normalize.query();
```

Functions remain component-owned. Direct calls dispose consumed tasks; `.run()`
retains explicit task control. Queries require `read_only`. Keep `useApp()` for
native files, media, settings and advanced operation/batch objects.

## Application and development agents

`revenant inspect --json` reports the project, source fingerprint, SDK, latest
build, generated contract and discovered instances. `revenant check --json`
runs the predefined checks and returns structured diagnostics.
`revenant mcp --project <path>` provides project/code/operation/diagnostic/runtime/
trace queries plus fixed check profiles. Rust-analyzer and TypeScript/Svelte
services establish semantic references; unavailable, incomplete and dynamic
relationships are reported explicitly. Code edits use the agent's normal editor.

Enable author-selected live application tools separately:

```toml
[agents]
enabled = true
```

Run the installed executable with `--mcp` from an MCP client. It attaches to an
existing app through a current-user-only Windows pipe or Unix socket. Select
`--instance <id>` if multiple app instances match. Identity includes the app
instance and compiled contract digest. Neither Node nor Cargo is required by
this mode. It does not launch a headless application.

Only operations marked `agent` become application tools. Connected agents may
invoke every exposed read or write without per-call approval. Native dispatch
enforces exposure; MCP annotations describe behavior. Operation calls return
receipts, with `revenant.tasks.get/results/cancel/dispose` for session-owned work.
Large terminal results are omitted from status replies and remain available through
`revenant.tasks.result`: concatenate its UTF-8 JSON chunks, then parse the result.
Batch result pages return one outcome at a time; a large outcome uses the same
chunk tool with `itemOffset`. Message limits report a structured error while
retaining the connection and its ownership.
One session cannot adopt another session's task or resource handles. Disconnect
requests cancellation and releases ownership. Uncooperative Rust can outlive
the cleanup deadline; its leases remain held until actual execution ends and
cleanup failure remains observable.

Development source access and runtime diagnostics are separate from packaged
tools. Trace buffers exclude argument/result payloads by default. Development
frontend reports include bounded errors and route/title/visibility state, not
arbitrary DOM contents or JavaScript execution.

## Execution and evidence

Only operations declared `independent` overlap within the existing global native
worker budget. Other batch operations remain sequential. Result ordinals retain
source order, cancellation preserves completed outcomes, and buffers are bounded.
The declaration does not prove purity, authorization or application correctness.

Release readiness requires exact-source multi-platform CI, installed Windows
workflows and visual editor acceptance. Local passes do not substitute for those
gates. The implementation epic and its remaining acceptance work are tracked in
Beads; [VALIDATION.md](VALIDATION.md) records completed evidence separately.
