# Revenant

Revenant is a Rust CLI that bootstraps a SvelteKit app and (optionally) applies
custom compile-time transforms through a tiny Rust-based compiler pipeline. This
lets you introduce ergonomic syntax sugar in `.svelte` files without forking the
Svelte compiler.

<div align="center">
  <img src="./imgs/image.png" alt="logo 1" width="200"/>
  <img src="./imgs/image2.png" alt="logo 2" width="200"/>
</div>

## What’s in the box

- `revenant` (Rust binary): sets up a Svelte project in `./output/svelte`,
  installs defaults, copies a small starter, and runs a sanity build.
- `revenantc` (Rust binary): a small, modular transform pipeline used as a
  Svelte preprocessor. It reads a JSON payload on stdin and prints JSON to stdout.
- `output/svelte/revenant-preprocess.js`: a Node wrapper that spawns `revenantc`.

## Why this design

- Keep Svelte’s official toolchain intact; only add a preprocessor stage.
- Keep Rust transforms small, testable, and independent using a `Pass` trait and
  a linear `Pipeline`.
- Make the preprocessor opt-in via an environment flag for safety and easy opt-out.

## Quick start

1. Build the binaries:

   - `cargo build`

2. Enable the preprocessor (opt-in):

   - From `output/svelte/` run:

     ```bash
     REVENANT_PREPROCESS=1 \
     REVENANTC_BIN=../../target/debug/revenantc \
     npm run dev
     ```

3. Try demo syntax:

   - In any `.svelte` file:

     ```svelte
     <rv:upper text="hello world" />
     ```

   - Should render as `HELLO WORLD` in place (from the demo pass).

## Architecture

- `src/compiler/`:
  - `mod.rs`: JSON protocol + `Pass` trait + `Pipeline` runner.
  - `passes/`: small, focused transforms. Start with `noop` and `demo_uppercase`.
- `src/bin/revenantc.rs`: the compiler CLI — reads JSON, executes pipeline, writes JSON.
- `output/svelte/revenant-preprocess.js`: Node wrapper exposing `{ markup, script, style }` hooks.
- `src/main.rs`: `revenant` CLI that scaffolds and builds the app.

## Adding real transforms

1. Create `src/compiler/passes/my_pass.rs` implementing `Pass`.
2. Re-export it in `src/compiler/passes/mod.rs`.
3. Add it to the pipeline in `src/bin/revenantc.rs`.

Prefer small, composable passes. If a transform is markup-only, early-return for
other kinds. Start with string transforms; when you need structure/sourcemaps,
integrate a parser (e.g., tree-sitter) and a mapper.

## Rust ↔ Svelte data sharing

- Build-time: generate `output/svelte/src/lib/revenant.generated.ts` from Rust with
  `export const RUST_DATA = { ... }`, then have a pass expand `$rev('key')` into
  reads from that module.
- Runtime: expose a JSON endpoint from a Rust server and inject into `window.__REV`
  on the Svelte side; expand helpers to read from there.

## Notes

- Auto-install of tools is intentionally not implemented; environments and
  package managers vary. We detect missing tools and provide clear guidance.
- The demo pass is intentionally trivial and used to validate the pipeline end-to-end.
