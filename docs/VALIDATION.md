# Desktop 0.3 validation status

Validation performed on Windows on October 2, 2026. Beads `revenant-gl5` tracks
the migration; `revenant-gl5.7` remains open for the release limitations below.
Formal tests began after the native/client/tooling workflow compiled together.

## Evidence boundaries

| Check | Current evidence/state |
| --- | --- |
| Public Rust documentation | Core, SDK, macros and desktop strict Rustdoc pass, including broken links and invalid HTML tags |
| TypeScript API documentation | 476 public declarations across three exports pass |
| Windows compile/docs/frontend CI | Workflow configured; remote result pending |
| Formal Rust/client tests | 65 Rust tests including one doctest pass; native tests include Windows concurrent-reader persistence and uncooperative shutdown. Client: 20 tests pass. Two environment-dependent tests are ignored by the ordinary suite and were run separately |
| 500,000-file folder/query/selection/results | Real native fixture passes; bounded native and installed UI evidence below |
| Zed / rust-analyzer discovery | Actual Zed project discovery, rust-analyzer hover/definition and macro expansion pass; generated TypeScript consumer hover/source link passes |
| Packaged Windows app | Both NSIS bundles built. File/media installed and launched; embedded assets reload offline with Node/Cargo absent from child PATH |
| Native window close/rebuild/cancellation | Fresh scaffold launches; CLI stops owned descendants. Installed UI cancellation retains partial paged results; route disposal reports zero resources/jobs |

API documentation checking is separate from typechecking and does not prove IPC,
performance, packaging or runtime correctness. Its diagnostics include source
paths/lines for public declarations, interface members, methods/properties,
constructors, type members, subscriptions and exported Svelte components.
It follows package exports/reexports and named local types, skips private/protected
members and generic type parameters, and rejects missing/placeholder descriptions.

## Development documentation gate

With the existing client dependencies installed:

```powershell
node scripts/check-api-docs.mjs
```

For Rust, coordinate the Cargo slot with the active owners before compiling.
The final/CI documentation command denies missing docs and broken intra-doc links
for core, SDK, macros and desktop. A source-level deny attribute alone is not a
passed documentation build.

## Final-phase commands

These gates were run locally during final validation; CI repeats them:

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-features --locked
$env:RUSTDOCFLAGS = '-D missing_docs -D rustdoc::broken_intra_doc_links'
cargo doc -p revenant-core -p revenant-sdk -p revenant-macros -p revenant-desktop --all-features --no-deps --locked
cargo test --workspace --all-features --locked
npm install --prefix packages/client
npm run check --prefix packages/client
npm run check:ui --prefix packages/client
node scripts/check-api-docs.mjs
npm test --prefix packages/client
cargo build -p revenant --locked
```

From each migrated reference app, use the compiled CLI to `build`, then run the
frontend's `npm run check` in `web/`. A fresh scaffold outside the checkout must
also build and expose its local SDK sources. The workflow archives rustdoc and
Windows bundle outputs, but producing installers is only build evidence.

## Required direct acceptance evidence

For the 500,000-file run record the dataset shape/count, hardware, elapsed scan
and query times, native/UI memory, page bounds, native queue admission/rejection,
all-matching selection behavior, paged partial outcomes, cancellation and cleanup.
Do not substitute a tiny fixture or SQLite-only benchmark for the full workflow.

For Zed, open the framework workspace and a fresh scaffold. Ensure rust-analyzer
loads the root workspace plus the optional `native/Cargo.toml` when present and
the generated `.revenant/desktop/Cargo.toml`. Link standalone app manifests explicitly
if the editor only loads one workspace. Check `Application`, operation macros,
`TaskContext` and module navigation, then generated TypeScript hovers and operation
source metadata. Presence of source links/configuration is not proof of working
editor navigation.

For packaging, install the produced Windows package independently of Vite,
Node/Cargo and the CLI checkout. Exercise folder access, supported previews,
checksum/custom operation, paged batch results, settings across relaunch,
permission/path failures, cancellation, close with active work and resource cleanup.
Record WebView2 availability; an installer output alone does not prove offline
installation on a machine lacking that runtime.

## Recorded acceptance

The [machine-readable run](validation/native-500000.json) describes the real
fixture and workload limitations. On a Ryzen 7 5800X with approximately 64 GiB
RAM, native folder admission took 0.142 ms and returned without discovery.
The first query returned in 74.7 ms; metadata discovery finished in 58.1 s.
All 500,000 checksum operations succeeded in 359.1 s. Peak native working set
was 56,864,768 bytes; result/window pages contained at most 128 rows. Native
cleanup left zero file resources and zero index/result stores. This was a
contended run, not an isolated benchmark, and almost all fixture files are empty.

The installed file/media app used the actual Windows folder picker. After
discovery it displayed `entry499999.txt`, with 14 rendered rows and 4,525,752
bytes of collected JavaScript heap. The virtualizer measures at most 4,096 rows;
the scroll surface was 311,296 pixels. End-range navigation avoids WebView2's
finite CSS height limit, which the former whole-folder spacer exceeded.
Selecting and processing entries after a search replacement succeeded.
Cancelling an all-matching batch retained 2,366 successful outcomes, displaying
64 at once; the next page began at ordinal 64. Text preview and remembered
preferences worked. Navigating away released all native file/media resources
and tasks. The custom record operation normalized three records in the installed
application. Prime sieve computed 9,592 primes through 100,000 and verified 97
as prime in its production window. Native PNG decoding and audio range reads
passed; invalid ranges returned 416 and disposed preview URLs returned 404.
Closing the installed window while discovery and batch preparation were active
exited in 69 ms. This close check exercised queued/preparation cancellation;
the native regression suite separately covers an uncooperative running executor.
The refreshed installer also launched with only Windows directories on the
child PATH, without debugger arguments; the application and its owned children
had zero TCP listeners. A real portable development run preserved its existing
window and bindings after a compile failure, rebuilt successfully after repairs
and subsequent edits, and cleaned up its desktop/Vite processes and temporary
launch copies. Those copies allow Windows to rebuild the target executable while
the current app remains open.

Actual rust-analyzer hover/definition resolved `Application::new` and `operations!`
to readable installed SDK source. Macro expansion resolved the native operation
factory. Fresh Zed worktree logs and processes confirmed both the native author
manifest and generated bootstrap. TypeScript 5.9.3 consumer hover returned the
operation's Rust documentation and a valid source link after fixing JSDoc
attachment to generated members. These are semantic/project-discovery checks;
no visual Zed hover popup is claimed. The consumer probe was repeated against
the final refreshed file/media facade and still resolved documentation and source.

Reproduction helpers are `scripts/create-file-fixture.py`, the desktop
`acceptance` binary, `scripts/check-ide.mjs`, `scripts/check-desktop-ui.mjs`, and
`scripts/select-native-folder.ps1`. UI inspection uses an explicitly enabled
local WebView2 debugger; offline emulation affects that target only.

## Remaining release limits

Remote Windows CI has not been run. Visual Zed hover inspection remains
unverified. The generated NSIS package includes offline WebView2 support and
installed successfully on this machine, which already has WebView2; installation
on a clean machine lacking that runtime remains unverified. Removing the shared
runtime from the user's machine is not an acceptance test. No release approval
is implied by these local results. Prime-sieve's frontend dependencies were
updated within their declared ranges and its package/checks rerun successfully.
The npm audit now reports three low-severity advisories in each reference app;
high/moderate advisories previously reported in prime-sieve were removed.

## Historical evidence

[October 1 validation](history/VALIDATION-0.2.md) retains the previously reported
Rust/client/browser/HTTP/scaffold/watcher results for 0.2. They were not rerun for
desktop 0.3. The old commands target removed hosts and are not current instructions.
[Recon/design findings](history/DESIGN-2026-09-30.md) remain useful history.
The old Beads issues remain historical; this sidecar does not rewrite their results.
