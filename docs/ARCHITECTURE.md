# Revenant Architecture

Revenant is a CLI tool that bridges Rust/WebAssembly and SvelteKit, providing
`new`, `dev`, `build`, and `setup` subcommands.

## Module Layout

```
src/
├── main.rs              Clap CLI dispatch + error display
├── lib.rs               Public module re-exports (enables integration tests)
├── bindings.rs          Rust -> TypeScript contract extraction + facade generation
├── config.rs            RevenantConfig, revenant.toml I/O, named constants
├── errors.rs            RevenantError enum (thiserror)
├── commands/
│   ├── new.rs           Project scaffolding command
│   ├── dev.rs           Dev server with WASM watch loop
│   ├── build.rs         Production build (WASM + web)
│   └── setup.rs         Interactive tool installation guide
├── toolchain/
│   ├── detect.rs        Tool enum + require_tools() availability check
│   ├── wasm.rs          WasmBuilder trait, WasmPackBuilder, WasmWatcher
│   └── process.rs       ManagedProcess with I/O pump threads
└── scaffold/
    ├── mod.rs           create_project() entry point
    └── templates.rs     Embedded template constants
```

### Why this structure

The codebase splits into three concerns:

- **`commands/`** — User-facing CLI logic. Each file maps to a subcommand and
  orchestrates the other modules. Contains no system interaction beyond calling
  into `toolchain/` and `scaffold/`.

- **`toolchain/`** — System interaction: spawning processes, detecting tools,
  building WASM, watching files. This is where platform-specific code lives
  (Windows `cmd /C` routing, Unix SIGTERM handling).

- **`scaffold/`** — Code generation. Templates and directory creation. Pure
  filesystem writes with no process spawning.

This separation keeps each module testable in isolation. `commands/` can be
tested by mocking toolchain calls; `scaffold/` can be tested by writing to a
temp directory; `toolchain/` logic tests don't need a project structure.

## Key Design Decisions

### Tool detection as prerequisite

`revenant new` checks all required tools (cargo, wasm-pack, node, npm) before
writing any files. This is a deliberate choice over lazy detection:

- The user gets a single error listing everything they need to install, not a
  sequence of failures after partial work.
- Tool probes are cheap (~100ms total for four `--version` subprocess spawns).
- No partial project directories are left behind on failure.

The tradeoff is that `revenant new` checks tools it won't use until later (e.g.,
`npm` isn't needed until `npm install` at the end). This is acceptable because
all four tools are always needed for a working project.

### No async runtime

Process management uses OS threads + `mpsc` channels. The dev event loop runs
on the main thread with a 100ms tick, draining log lines and checking for
shutdown/rebuild signals.

The 100ms tick is a deliberate tradeoff: lower wastes CPU cycles polling empty
channels, higher adds noticeable latency to log output. At 100ms, CPU usage is
negligible and output feels real-time.

An async runtime (tokio, async-std) would eliminate polling but add significant
dependency weight and complexity for a tool that manages at most three child
processes.

### No template engine

All scaffold templates are `const &str` in `templates.rs` with a tiny string
substitution layer for `{project_name}` and `{wasm_name}`.

The tradeoff: adding conditionals or loops would still be clumsy. If v2
template variants (minimal, full) need conditional sections, a proper template
engine may become worthwhile.

### File watching

`wasm-pack` has no `--watch` flag. Revenant owns the watch loop via the `notify`
crate watching `rust/src/` with a 500ms debounce, spawning a new `wasm-pack`
process on each change.

Debouncing happens inside the `notify` callback, not in the consumer. A single
file save can fire 3-5 filesystem events; debouncing at the source prevents
redundant rebuild signals.

### Generated bindings

After every successful WASM build, Revenant reads `rust/src/lib.rs`, extracts a
contract from public `#[wasm_bindgen]` exports, writes that manifest to
`pkg/revenant.contract.json`, and regenerates `web/src/lib/wasm.ts`.

The current bridge strategy is deliberately strict:

- **Supported runtime surface**: primitives, `String`, and typed numeric
  vectors that `wasm-bindgen` already exposes cleanly.
- **Supported contract surface**: the manifest understands richer Rust shapes
  like `Option<T>`, `Result<T, E>`, tuples, structs, and enums.
- **Failure mode**: if an exported function uses a type the TypeScript contract
  can describe but the runtime bridge cannot preserve yet, Revenant fails the
  build with an explicit error instead of emitting `unknown` or `any`.

This keeps the public Svelte API stable and invisible while leaving room for a
future serialized bridge behind the same generated facade.

### Error handling

Two-layer model: `thiserror` for structured domain errors (`RevenantError`),
`anyhow` for propagation with context. `main()` walks the error chain and
prints each cause indented.

Every error message follows a three-part pattern:
1. What failed
2. Why it failed (if determinable)
3. What the user should do next

### Signal handling

`ctrlc` with `termination` feature catches SIGINT/SIGTERM (+ Windows console
events). The handler sends on an mpsc channel; the dev loop polls it.

Children are killed as a whole tree, not just the direct process, since
`npm run dev` commonly forks `vite`/`node` as grandchildren. On Unix, each
child is spawned into its own process group (`CommandExt::process_group(0)`)
and `terminate_with_timeout` sends SIGTERM via `nix::sys::signal::killpg` to
the whole group. On Windows, where the tracked child is actually the
`cmd /C` wrapper (see below), `taskkill /PID <pid> /T /F` kills the wrapper
and everything it spawned. Either way, the process waits up to 5 seconds
before escalating to a forceful kill (SIGKILL on Unix).

### Auto-install

`detect.rs` will auto-install `wasm-pack` via `cargo install wasm-pack` if it
is missing. This is a deliberate deviation from the "no auto-install" principle —
wasm-pack is rarely pre-installed and trivially installable. All other tools
(cargo, node, npm) require manual installation.

### Verbose mode

The `--verbose` global flag controls child process output visibility.
`run_blocking` accepts a `verbose: bool` parameter:

- **verbose=true**: inherits stdio (user sees raw output from wasm-pack, npm)
- **verbose=false**: captures output, only surfaces stderr in error messages

This keeps default output clean while still giving full diagnostics when needed.

### Process management

`toolchain/process.rs` provides two execution models:

- **`run_blocking`** — synchronous execution; verbose mode inherits stdio,
  quiet mode captures output. Used for one-shot commands (npm install,
  wasm-pack release build).

- **`ManagedProcess`** — spawns the process with piped stdout/stderr, then
  creates two reader threads that forward lines as `LogLine` messages to an
  `mpsc` channel. Used for long-running processes (web dev server, background
  wasm-pack rebuilds) where output must be labeled and interleaved.

The two-model split exists because `run_blocking` is simpler and preserves
terminal formatting (colors, progress bars) that piping would lose.
`ManagedProcess` sacrifices raw terminal output for labeled, interleaved
streaming from multiple concurrent processes.

### Setup command

`commands/setup.rs` provides an interactive guided setup that checks each tool
sequentially and provides OS-specific installation instructions. It detects the
OS via `std::env::consts::OS` and tailors commands for Windows (winget), macOS
(brew), and Linux (package managers). The setup command never installs anything
automatically — it guides the user through each step.

### Windows compatibility

All tool invocations on Windows route through `cmd /C` to resolve `.cmd` shims
(npm, npx). This adds one process to the tree but is necessary because
`Command::new("npm")` fails on Windows where npm is a `.cmd` script, not a
binary. Because of this wrapping, the `ManagedProcess` child Revenant tracks
is `cmd.exe`, not the real program — killing just that process would leave
the actual npm/vite/node tree running, which is why `terminate_with_timeout`
uses `taskkill /T` (see "Signal handling" above) instead of killing the
wrapper directly.

## Extension Points

### v2: Vite plugin

The `WasmBuilder` trait (`toolchain/wasm.rs`) is the seam for v2. A Vite plugin
could trigger `wasm-pack` on WASM import resolution, removing the need for
Revenant's custom file watcher entirely. The trait allows swapping the build
backend without changing command logic.

`TODO(v2)` markers in the codebase:
- `commands/dev.rs` — Vite plugin replacing the watcher
- `config.rs` — optional fields (wasi_target, custom watch paths, build flags)
- `scaffold/mod.rs` — template variants via `--template` flag
- `bindings.rs` — richer bridge strategies beyond direct `wasm-bindgen` exports
- `toolchain/detect.rs` — pnpm/yarn/bun support
- `toolchain/wasm.rs` — WasiBuilder variant

### v3: Transform pipeline

Post-WASM-build transforms (wasm-opt, size reporting, custom codegen) could slot
in as a trait-based pipeline between `WasmBuilder::build_*()` returning and the
command reporting success. Each transform would implement a `WasmTransform` trait
with a `transform(&self, wasm_path: &Path) -> Result<()>` method, chained in
sequence.

## Tradeoffs Summary

| Decision | Benefit | Cost |
|----------|---------|------|
| No async | Simpler code, fewer deps | 100ms polling tick |
| No template engine | Zero deps for templates | String substitution only; richer templates stay manual |
| Upfront tool checks | Single error, no partial state | Checks tools not immediately needed |
| Auto-install wasm-pack only | Pragmatic UX win | Inconsistent with "no auto-install" principle |
| Windows `cmd /C` | npm works on Windows | Extra process in tree; needs `taskkill /T` to clean up the whole tree, not a plain kill |
| Two process models | Right tool for each job | Two code paths to maintain |
