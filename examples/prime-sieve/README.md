# Prime sieve desktop example

Run `revenant dev` from this folder with Revenant 0.3. The CLI launches a native
window with Vite HMR. `revenant build` compiles contracts, builds the static SPA,
and packages the app through Tauri 2. Windows uses NSIS with offline WebView2.
The CLI adds Zed linked projects for the native library and hidden host while
preserving existing project settings and comments.

The optional `native/` library exports only `app() -> revenant::Application` and
ordinary operation functions. The framework generates `.revenant/desktop` and
owns startup, native execution, settings persistence and contract export. No
WASM module, Tokio main, feature-gated registration or server is authored here.

`primes.upTo` accepts a documented `SieveInput` and returns `SieveResult`. The
native implementation rejects limits above 1,000,000 to bound allocation and
serialized output. Cancellation checkpoints occur during marking and result
collection. The UI receives a `Task`, displays native progress, and only reads
the final bounded prime list for summaries. `primes.isPrime` handles the full
unsigned 32-bit range without multiplication overflow; `primes.count` uses the
same sieve while returning only a count.

The generated facade exposes readable `PrimesUpTo.Input` / `PrimesUpTo.Output`
contract namespaces. Hover operations and payload fields for original Rustdoc;
operation docs link to the Rust source. The manifest comes from the actual
compiled executable's `--revenant-contract` export, before a window is created.
Unsuccessful generation keeps the previous facade.

Preferences use native atomic settings persistence under the app identifier.
The layout owns the root app, and the page creates a child scope. Route teardown
cancels/reclaims native tasks and settings ownership. Install prerequisites with
[the Tauri platform guide](https://v2.tauri.app/start/prerequisites/) after using
`revenant setup` for read-only diagnosis.
