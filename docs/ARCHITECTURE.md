# Revenant Architecture

This document explains how the pieces of Revenant fit together: how the CLI
owns the Svelte project lifecycle, how the Rust-based compiler integrates via a
preprocessor, how transforms are structured internally, and how TypeScript/JS
code can see values defined on the Rust side.

## Overview

- Binaries
  - `revenant` — main CLI. Scaffolds a Svelte app, runs dev/build/preview, and
    injects runtime globals. Lives in `src/main.rs` (thin) and the modules it
    delegates to (`cli`, `setup`, `svelte`).
  - `revenantc` — tiny compiler CLI used by the Svelte preprocessor. Reads a
    JSON `TransformInput` from stdin, applies a `Pipeline` of passes, and prints
    a JSON `TransformOutput` to stdout.

- Library
  - `src/lib.rs` exports the `compiler` module so the transform pipeline can be
    tested in isolation or reused elsewhere.
  - `src/compiler/` contains the protocol types, the `Pass` trait, the
    `Pipeline` runner, and a couple of example passes (`noop`, `demo_uppercase`).

- Svelte integration (Node wrapper)
  - `output/svelte/revenant-preprocess.js` exposes `{ markup, script, style }`
    hooks for SvelteKit. When enabled, it spawns `revenantc` and forwards code
    through the Rust pipeline.

## Repository Layout

- `src/main.rs` — minimal entrypoint that defers to the `cli` module.
- `src/cli.rs` — Clap subcommand definitions and dispatch.
- `src/setup.rs` — the setup pipeline (named steps run by `full` or individually).
- `src/svelte.rs` — process orchestration helpers for Svelte dev/build/preview,
  compiler path resolution, and TS global injection.
- `src/compiler/` — transform core and passes (see below).
- `output/svelte/` — the generated Svelte app + preprocessor glue.
- `default-svelte-root/` — template files copied into the app during setup.

## Execution Model

### 1) Setup Pipeline (scaffolding)

Run with: `cargo run -- full` or individual steps via `cargo run -- step <name>`.

Steps (in `src/setup.rs`):
- `create_root` — ensures the output root exists; clears any previous Svelte app.
- `verify_packages` — checks required external tools (git/node/npm) and reports
  missing ones. Auto-install is intentionally not implemented.
- `setup_svelte` — scaffolds the Svelte project (via `sv`), installs `npm`
  dependencies, copies template files, runs an initial build.
- `inject_globals` — generates TS files to expose a global `Revevant` object
  (see “Rust → TS Globals”).
- `compiler` — demo-only: runs the Rust pipeline over `.svelte` files in-place
  (shows the plumbing works end-to-end). Safe to remove for production.

### 2) Owning the App Lifecycle

Run with:
- `cargo run -- dev [--host] [--port <n>]` — starts `npm run dev` inside the app
  and wires Revenant’s preprocessor automatically via env vars.
- `cargo run -- build` — runs the production build.
- `cargo run -- preview [--port <n>]` — previews the production build.

The CLI sets these env vars while launching the Svelte process:
- `REVENANT_PREPROCESS=1` — opt-in switch that enables the preprocessor wrapper.
- `REVENANTC_BIN=<absolute path>` — points the wrapper at the compiled
  `revenantc` binary. The path is resolved relative to the main binary
  (debug/dev output); if not found, PATH resolution is used.

### 3) Preprocessor Bridge (Node → Rust)

`output/svelte/revenant-preprocess.js` implements SvelteKit preprocess hooks and
bridges to Rust:
- For each hook (`markup`, `script`, `style`):
  1. Build a JSON payload `{ version, path, kind, source, options }`.
  2. Spawn `revenantc` with the payload on stdin; capture stdout.
  3. Parse the JSON `TransformOutput { code, map, diagnostics }` and return the
     `code` (and `map`) to Svelte.
- Diagnostics (`info`/`warn`/`error`) are surfaced in the dev console.
- If `REVENANT_PREPROCESS` is not set to `1`, the wrapper returns the original
  content unchanged.

## Compiler Architecture (Rust)

Protocol and pipeline (in `src/compiler/mod.rs`):
- `TransformInput { version, path, kind, source, options }` — payload received
  from Node (preprocessor wrapper). `kind` ∈ { `Markup`, `Script`, `Style` }.
- `Diagnostic { level, message }` — developer-facing messages collected during
  transforms and printed by the Node wrapper.
- `TransformOutput { code, map, diagnostics }` — final result passed back to
  Svelte (only `code` is required).
- `trait Pass { fn name(&self) -> &'static str; fn run(&self, input, code) -> (String, Vec<Diagnostic>) }` —
  the interface for a small, testable transform.
- `Pipeline` — chains passes sequentially; each pass receives the previous
  output. Diagnostics are concatenated.

Included passes:
- `NoopPass` — returns the input unchanged; useful as a baseline.
- `DemoUppercasePass` — demo-only; in `Markup`, replaces `<rv:upper text="x" />`
  with `x.to_uppercase()` (emits an info diagnostic). Not intended for production.

Adding a new pass:
1. Create `src/compiler/passes/<name>.rs` implementing `Pass`.
2. Re-export it in `src/compiler/passes/mod.rs`.
3. Add it to the pipeline in `src/bin/revenantc.rs`.
4. Scope by `TransformKind` (e.g., early-return for non-`Markup` when your pass
   only applies to markup).

Guidance:
- Keep passes small and focused; compose multiple passes rather than one large
  transform.
- Prefer string rewrites initially; for structure/sourcemaps, integrate a parser
  (e.g., tree-sitter) and build a mapping layer.

## Rust → TS/JS Globals

The setup step `inject_globals` generates:
- `output/svelte/src/lib/revenant.global.ts` — exports a constant `Revevant` with
  fields: `version`, `root`, `svelte`, `compiler`.
- `output/svelte/src/app.d.ts` — declares an ambient `Revevant` global type for TS.
- `output/svelte/src/hooks.client.ts` — assigns the exported object to
  `globalThis.Revevant` at runtime so application code can do `Revevant.compiler`
  (etc.) without importing.

This demonstrates how Revenant can define values on the Rust side and expose
them to the browser environment in a controlled, typed manner.

## Data Flow (End to End)

1. `cargo run -- full`
   - Clears/creates output dir, scaffolds Svelte, installs deps, copies template,
     initial build, injects global TS runtime.
2. `cargo run -- dev`
   - Spawns `npm run dev` with envs pointing to `revenantc`.
3. Svelte preprocess hook → Node wrapper
   - Builds JSON input for `revenantc` per `(kind, content, filename)`.
4. `revenantc` executes the pipeline
   - Runs passes (e.g., `NoopPass`, custom passes) → `TransformOutput`.
5. Node wrapper returns code to Svelte
   - Diagnostics are logged; transformed code flows into Svelte compilation.

## Extensibility & Customization

- New passes: follow the pattern above; keep them idempotent and scoped by
  `TransformKind`.
- New setup steps: add closures/functions in `src/setup.rs` via the
  `named_function_vec!` macro so step names and functions stay paired.
- Alternative orchestration: add new subcommands in `src/cli.rs` to script more
  flows (e.g., codegen, integration tests, publishing).

## Error Handling & Platform Notes

- External commands are executed via small helpers (`cmd!` macro and `commands::run`)
  and by direct `Command` invocation in `svelte.rs` for streaming use-cases.
- Package verification is parallelized (Rayon) and installation is left to the
  user for safety and environment parity.
- Windows specifics are handled in places like directory deletion and binary
  path resolution (e.g., `.exe` suffix).

## Commands Reference

- `revenant full` — runs the full setup pipeline.
- `revenant step <name>` — runs a specific setup step by name.
- `revenant dev [--host] [--port <n>]` — launches the dev server under Revenant.
- `revenant build` — runs a production build.
- `revenant preview [--port <n>]` — previews the production build.

## Design Decisions

- Keep Svelte’s official toolchain intact and integrate only via preprocessors.
- Use small, composable Rust passes with a clear interface and JSON protocol.
- Let the Rust CLI “own” the Svelte lifecycle to remove JS-side configuration
  complexity; wiring (envs, binary paths) is handled from Rust.

