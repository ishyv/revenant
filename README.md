> [!WARNING]
> This is an early prototype. The design and implementation are subject to change.

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
  installs defaults, copies a small starter, and runs dev/build/preview.
  
Note: the experimental Rust compiler/preprocessor has been removed for now until
the syntax and parser approach are finalized.

## Why this design

- Keep Svelte’s official toolchain intact; only add a preprocessor stage.
- Keep Rust transforms small, testable, and independent using a `Pass` trait and
  a linear `Pipeline`.
- Make the preprocessor opt-in via an environment flag for safety and easy opt-out.
  
For a deeper dive into the design and code organization, see `docs/ARCHITECTURE.md`.


## Quick start

1. Build the binaries:

   - `cargo build`

2. Let Revenant own the app lifecycle:

   - Initial setup (scaffold app, install deps, build):

      `cargo run -- full`

   - Dev server (env wiring is automatic):

      `cargo run -- dev`

   - Production build:

      `cargo run -- build`

   - Preview build:

      `cargo run -- preview`

3. Optional preprocessor (disabled):

   - Preprocessor integration is not available in this revision. When the
     compiler returns with a proper parser-backed design, documentation will be
     updated with usage instructions.

## Architecture

- `src/main.rs`: `revenant` CLI that scaffolds and builds the app.
  - Commands: `full`, `dev`, `build`, `preview`, and `step <name>`.
  
Historical note: compiler-related modules and the `revenantc` binary have been
removed pending a redesigned, parser-backed transform pipeline.

## Adding real transforms

Not applicable in this revision. The compiler was removed to avoid confusing or
brittle behavior. If/when transforms return, they will be parser-backed and
documented with clear syntax and guarantees.

## Rust ↔ Svelte data sharing

- Build-time: generate `output/svelte/src/lib/revenant.generated.ts` from Rust with
  `export const RUST_DATA = { ... }`, then have a pass expand `$rev('key')` into
  reads from that module.
- Runtime: expose a JSON endpoint from a Rust server and inject into `window.__REV`
  on the Svelte side; expand helpers to read from there.

## Notes

- Auto-install of tools is intentionally not implemented; environments and
  package managers vary. We detect missing tools and provide clear guidance.
  
  
