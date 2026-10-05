# Desktop 0.3 implementation boundaries

The approved design is [DESIGN.md](DESIGN.md). Active work is Beads
`revenant-gl5`; [0.2 contracts](history/IMPLEMENTATION-0.2.md) are historical.

## Public application surface

Configuration version 3 has `[project]`, optional `[toolchain]` and `[desktop]`.
Unknown fields are errors. The frontend lives in `web/`; `native/Cargo.toml` is
optional and must describe a library exporting `pub fn app() -> revenant::Application`.
The SDK dependency is the same 0.3 source snapshot used by the generated host.

`Application::new()` provides the built-in contract. `#[contract]`,
`#[operation(id = "group.operation")]` and `operations![module::function]`
define/register custom behavior without repeating TypeScript schemas or command
handlers. `Application::capability` adds providers with configuration;
`Application::replacement` declares compiled alternatives. Definitions are inert:
constructing/exporting an application does not start windows or acquire resources.

Generated bindings expose grouped `app.operations`, typed input/output contracts,
`setupApp`, `createApp`, `provideApp`, `useApp`, and scoped lifecycle. Operation
queries provide explicit latest-result read stores; input/output type helpers
infer DTOs without handwritten duplicate contracts. Rustdoc and operation
source metadata are carried into generated documentation. Core, SDK, macros and
desktop enforce public documentation. Strict Rustdoc and actual generated
TypeScript hover/source-link checks pass; CI requires the same standard.

## Native contract and projection

The compiled manifest uses version 3 / protocol 2 with digest, operation IDs,
schemas, TypeScript definitions, descriptions and source metadata. Wire-safe
contracts reject unsupported/loss-prone serialization. Large numeric file sizes
and progress values use canonical decimal strings; presentation may convert
bounded values for ratios.

Native contract export runs before startup. The CLI validates it and atomically
publishes a facade pointer to an immutable generation. Client root connection
compares manifest/digest and each bound operation checks the native schema.
The production pipeline rechecks the final embedded executable contract before bundling.
A stale or incompatible generated facade fails explicitly. This is compiled
contract extraction, not a source-parser approximation.

Tauri IPC projects scope tokens, query windows, selections, tasks, previews,
settings and provider state. It is not an HTTP endpoint or a JavaScript execution
provider. `packages/client/docs/WIRE.md` belongs to the client/native wire owners;
[architecture](ARCHITECTURE.md) maps the source modules.

## Bounded storage and execution

Folder metadata and frozen query selections live in SQLite. Query pages default
to 128 rows and cap at 512. Explicit file selections cap at 512; all-matching
queries are captured natively and iterated lazily. Scan state/warnings are bounded
metadata, not a second inventory in JavaScript. Native path display never replaces
handle validation. Indexed metadata is a snapshot, not a filesystem transaction;
files may change or disappear before later reads.

Admission validates/captures inputs and pins provider generations before returning.
Native options bound concurrency, queues, scopes, views, retained tasks, JSON bytes
and shutdown grace. Rejection returns a structured error such as `queue_full`.
Batch outcome SQLite storage exposes pages capped at 512; task snapshots carry
summary counters, not every outcome. Row bounds alone do not bound arbitrary JSON;
input/output byte limits are also required.

Task states distinguish queued/running/cancelling from succeeded/partial/failed/
cancelled/interrupted. Terminal snapshots are immutable. Cancellation is cooperative:
custom Rust checkpoints and yields, captured leases survive until executor exit,
and completed effects/results are not universally rolled back. Shutdown stops
admission and requests cancellation within a grace period; uncooperative code
cannot be safely forcibly interrupted. Cleanup, persistence and cancellation share
one deadline. Unfinished work becomes interrupted; a native regression verifies
that late success cannot revise that terminal snapshot.

## Native settings and replacement

The native settings store serializes loads/writes, validates keys and remembers
successfully loaded defaults. Missing object fields merge recursively; unknown
stored fields survive. Malformed JSON, type mismatch and oversized data return
structured errors. Encoded records are bounded to 1 MiB. Writes sync a temporary
file in the same directory and atomically overwrite the target. Power-loss
survival of the directory entry is platform-dependent; separate processes are
last-writer-wins. Client edits/status are projections and do not move storage
into browser localStorage.

Provider changes validate configuration schema, full operation compatibility,
dependencies and idle resource boundaries. Prepare candidates before activation;
cleanup failed candidates and retain the old provider. Compiled alternatives are
chosen by identity; JavaScript does not inject operation executors. Changing Rust
requires rebuilding. Arbitrary memory mutation/state transfer and Python embedding
are outside the supported surface.

## Removed hosts and migration

Old unversioned/version 2 configuration fails before generation/startup. Remove
WASM targets, `app.server` and server sections, and migrate source/ownership through
a new desktop scaffold; merely changing the version number is insufficient.
The obsolete `revenant-hosts` browser/HTTP source is removed. Old SDK snapshots
fail the desktop guard and must be preserved/migrated before regeneration.

Implementation is in progress. The 500,000-file run, real Zed navigation and offline
Windows package execution require direct evidence. Historical passing web/HTTP
checks do not establish them. See [validation](VALIDATION.md).
