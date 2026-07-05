# Revenant

A CLI that scaffolds and drives Rust/WebAssembly + SvelteKit projects. Write
logic in Rust, compile to WASM, and consume it from Svelte 5 — one tool
manages the entire build loop, including a generated TypeScript facade that
hides the raw `wasm-bindgen` package.

<div align="center">
  <img src="./imgs/image.png" alt="Revenant" width="200"/>
  <img src="./imgs/image2.png" alt="Revenant" width="200"/>
</div>

## Why

Setting up Rust/WASM with SvelteKit by hand means wiring together `wasm-pack`,
Vite plugins, path aliases, and a file watcher — then hoping they stay in sync.
`wasm-pack` has no `--watch` flag
([wasm-pack#457](https://github.com/rustwasm/wasm-pack/issues/457)), so you
either poll manually or write your own watcher. Revenant does this once,
correctly, so you can focus on code.

## Requirements

- **Rust** (with `cargo`) — [rustup.rs](https://rustup.rs)
- **Node.js 18+** (with `npm`) — [nodejs.org](https://nodejs.org)
- **wasm-pack** — auto-installed via `cargo install wasm-pack` if missing

Not sure what you have? Run `revenant setup` for a guided checklist.

## Quick Start

```sh
cargo install --path .

revenant new my-app
cd my-app
revenant dev
```

Open the URL printed by the dev server. You should see `"Hello from my-app, World!"` rendered by the Svelte page calling your Rust WASM function.

## Commands

### `revenant new <name>`

Scaffolds a complete project: Rust WASM crate, SvelteKit app, Vite config with
WASM plugins, and a working hello-world example. Runs `npm install`
automatically.

### `revenant dev`

Runs an initial WASM build, starts a file watcher on `rust/src/`, and launches
the SvelteKit dev server. Rust changes trigger automatic WASM rebuilds plus
regeneration of `web/src/lib/wasm.ts` and `pkg/revenant.contract.json`.
Ctrl+C cleanly stops all processes.

### `revenant build`

Production build: `wasm-pack build --release` followed by `npm run build`.
Before the web build, Revenant regenerates the typed Svelte facade from the
Rust exports. Fails fast if either step errors.

### `revenant setup`

Interactive checklist that verifies each required tool is installed and provides
OS-specific installation instructions for anything missing.

### `--verbose`

All commands accept `--verbose` to show full child process output (wasm-pack,
npm). Without it, output is kept clean and errors include relevant details.

## Project Structure

```
my-app/
├── revenant.toml         # project config (name, wasm target, package manager)
├── GETTING_STARTED.md    # tutorial: adding functions, calling from Svelte
├── rust/                 # wasm-pack crate
│   ├── Cargo.toml
│   └── src/lib.rs        # your Rust code — #[wasm_bindgen] exports
└── web/                  # SvelteKit app
    ├── package.json
    ├── svelte.config.js
    ├── vite.config.js    # wasm plugin + path alias to pkg/
    └── src/
        ├── app.html
        ├── lib/wasm.ts   # generated typed facade; import this, not pkg/
        └── routes/
            └── +page.svelte  # imports from $lib/wasm
```

WASM output goes to `pkg/` at the project root, alongside
`pkg/revenant.contract.json` — the intermediate contract manifest Revenant
derives from `rust/src/lib.rs`. `vite-plugin-wasm` and
`vite-plugin-top-level-await` handle browser-side loading.

## Known Limitations

The generated TypeScript facade only bridges what `wasm-bindgen` can expose
cleanly at runtime today: primitives, `String`, and typed numeric vectors
(`Vec<i32>`, `Vec<f64>`, etc.). Exported functions using `Vec<String>`,
custom structs, or other richer shapes will fail bindings generation with an
explicit error rather than silently producing a broken facade — see
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#generated-bindings) for the full
bridge strategy.

## Roadmap

**v2** — Vite plugin that triggers `wasm-pack` on import resolution, replacing
the custom file watcher. Also: WASI target support, pnpm/yarn/bun as package
manager options, scaffold template variants.

**v3** — Post-build transform pipeline (wasm-opt, size reporting, custom
codegen) as a trait-based extension point.
