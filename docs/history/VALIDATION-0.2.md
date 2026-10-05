> Historical October 1 evidence for 0.2 only. These reported results have not been rerun for desktop 0.3 and do not establish its acceptance. See [current validation status](../VALIDATION.md). Reproduction commands below target the removed hosts.

# Implementation and validation handoff

Verified on Windows on October 1, 2026. The implementation is on
`codex/capability-runtime`; no commits or pushes were made.

## What is implemented

Revenant 0.2.0 expresses applications through typed operations, collections,
selections, observable tasks, settings, scoped resources and inspectable
providers. Rust registrations generate both schemas and TypeScript contracts.
Application code does not repeat DTOs, transport calls, per-item orchestration,
task progress plumbing or settings persistence.

Browser/WASM is the default. A bounded worker pool reads browser files through
leased chunk brokers. The optional native Rust host supplies explicit uploads,
session isolation, bounded jobs, streaming snapshots and scoped inspection.
Both hosts use the compiled contract. Ordinary Svelte, TypeScript providers and
supported provider replacement remain available to applications.

The CLI embeds the SDK and the actual pinned HyvUI package into generated
projects. Builds publish complete contract/WASM generations atomically. Failed
rebuilds retain the working generation; the watcher retains edits arriving
during compilation, and the CLI owns child-process cleanup. Existing
unversioned projects retain their legacy bridge.

## Verified results

| Check | Result and evidence |
| --- | --- |
| Rust formatting and workspace tests | Pass: 50 tests, including the original 26 CLI tests, ownership, wire contracts, task lifecycle, provider transactions and native quota/batch tests. |
| TypeScript client | Pass: 17 tests, strict TypeScript check and Svelte check with zero errors or warnings. Tests cover deferred WASM readiness, queued leases, cancellation, immutable snapshots, partial failures, candidate rollback and asynchronous scope cleanup. |
| Independent host features | Pass: no features on native; browser on `wasm32-unknown-unknown`; combined browser/server on native. |
| Reference production app | Pass: static production build and Svelte check with zero errors or warnings. |
| Real browser workflows | Pass in headless Edge with a fresh profile: text, image, audio and video previews; metadata; all SHA-256 results match independent Python hashes; report download; runtime inspection; persisted settings; ordinary record normalization. |
| Browser cancellation | Pass: cancel while hashing the second, 128 MiB file; retain the completed first checksum and partial task result; UI remains responsive. |
| Native protocol | Pass: explicit upload/checksum, session isolation, mixed valid/invalid batch outcomes, quota rejection without hidden history, SSE snapshots, cancellation, path rejection and disposal. |
| Actual browser ServerClient | Pass: contract handshake, XHR upload, compiled operation binding, SSE single and batch results, 128 MiB partial cancellation, foreign-handle rejection, scoped inspection and application disposal. |
| Fresh standalone packaging | Pass: `C:/T/revenant-final-proof` was created outside this repository with its embedded SDK. Native-enabled production build produces `build/web` and `build/server.exe`; generated UI runs the real WASM checksum; packaged native server runs the same operation. Svelte check has zero errors or warnings. |
| Watcher and process lifecycle | Pass against disposable `C:/T/revenant-standalone`: failed compilation preserves publication; rapid saves coalesce; an edit during compilation gets a subsequent build; local SDK changes rebuild; shutting down the CLI closes the owned Vite socket. Source fixtures are restored. |
| Legacy example | Pass: the unversioned prime-sieve app still builds its static web output. |
| Change hygiene | Pass: formatting and Git whitespace checks; no tracked example target artifacts remain. The 324 previous artifacts are removed from the Git index while their local files remain. HyvUI archive hash matches its recorded pin. |

The integration runners own and stop their native processes. No test listeners
remain on ports 8787, 8788, 5173 or 4173 after validation.

Independent review found and resolved native batch limits, invalid-item
preleasing, unreturned task-history quota leaks, registry-local resource IDs,
provider teardown, same-instance replacement and dependent-provider transaction
reservations. Real browser validation also caught and fixed task-button
reactivity and standalone npm package resolution.

## Reproduce

From the repository root, with Rust, the WASM target, Node/npm, wasm-pack and
Python available:

```powershell
cargo fmt --all -- --check
cargo test --workspace --features revenant-hosts/server
cargo check -p revenant-hosts --no-default-features
cargo check -p revenant-hosts --no-default-features --features browser --target wasm32-unknown-unknown
cargo check -p revenant-hosts --no-default-features --features server,browser
cargo build -p revenant

Push-Location packages/client
npm install
npm run check
npm test
npx svelte-check --tsconfig ./tsconfig.json
Pop-Location

Push-Location examples/file-media
../../target/debug/revenant.exe build
Push-Location web
npm run check
Pop-Location
Pop-Location

python -m pip install -r tests/requirements.txt
python -m playwright install chromium
python tests/integration_runner.py
```

Use `--channel msedge` for the installed Edge browser. On Unix, omit `.exe`.
The local WASM tooling is under `.tooling/bin`; add it to PATH when using the
outside-repository scaffold. Local Python dependencies for this run are under
`.tooling/python`; the bundled Python interpreter used `PYTHONPATH` pointing
there. No global Python installation was changed.

For a disposable scaffold, run `revenant new`, set `[app] server = true`, then
`revenant build` and:

```text
python tests/standalone_workflows.py --project <absolute-project-directory>
```

Watcher verification requires a disposable scaffold with its server option
disabled:

```text
python tests/watch_workflows.py --cli <absolute-cli-path> --project <absolute-project-directory>
```

Browser screenshots and reports from this run are in `.tooling/browser`.
They are local evidence and intentionally ignored by Git.

## Limits and delivery state

Windows is verified locally. CI now covers Windows and Ubuntu, the independent
host features, browser/native workflows, standalone packaging and watcher
regressions; the remote jobs have not been run in this session.

Cancellation is cooperative. Custom Rust must yield and checkpoint; native
threads cannot safely be forcibly stopped. Provider callbacks must finish
their own asynchronous work for cleanup to finish. Runtime replacement requires
compatible complete operation sets and an idle boundary; new Rust still needs
a rebuild. Desktop/Tauri hosting, arbitrary live code replacement and an
embedded Python runtime are outside this release.

The SDK is materialized locally; no public package release or deployment was
performed. New scaffolds include `install-links=true` so npm installs the
embedded client with its application dependencies. The actual HyvUI archive
and included licenses are described in [vendor/README.md](../../vendor/README.md).
The sibling HyvUI and ezsgame source projects were not modified.

See [AUTHORING.md](../AUTHORING.md) for recipes and
[ARCHITECTURE.md](../ARCHITECTURE.md) for package boundaries. Beads remains the
durable task tracker. The machine remains on under the later no-shutdown
instruction.
