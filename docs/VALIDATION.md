# Revenant validation status

## Work-in-progress checkpoint — October 22, 2026

This checkpoint preserves the unpublished Revenant 0.4 work for cloud
continuation. It is **not release approval**. It was developed on `main` over
base `564e4142c8c4d1b57f2473f7a09d05d16da2a0d4`; old working-tree measurements
do not automatically validate this checkpoint. Project layout version 3,
compiled manifest version 6, bridge protocol version 5, and SDK/client
compatibility revision 4 are distinct contracts.

## Fresh Windows checkpoint checks

| Check | October 22 result |
| --- | --- |
| Client TypeScript and Svelte | Passed |
| Public client documentation | Passed |
| Ordinary client tests | 96 passed |
| Mounted UI tests | 3 passed |
| Python benchmark correctness/ownership tests | 112 passed |
| Rust formatting and locked all-feature workspace tests (including doctests) | Passed |
| Node tooling tests | 33 passed; rust-analyzer case skipped |
| Exact-checkpoint remote CI | Pending publication/execution |

These checks do not establish performance, installed-app acceptance, or
cross-platform support. Ignored/hardware tests are separate gates.

## Known limitations carried into continuation

The prepublication audit `revenant-c8p` remains open in the local Beads tracker.
The following concerns are not represented as fixed by this checkpoint:

- MCP async-writer interruption has regressions, but native process exit with a
  blocked Tokio stdout worker remains unverified. EOF during blocked SDK
  initialization/error replies also needs independent validation.
- Native-surface lifecycle/queue bounds, canvas prop changes, showcase ownership,
  and graphics behavior have source-review concerns requiring focused reproduction.
- Folder path identity under concurrent namespace mutation needs investigation;
  a selected path is not itself an immutable directory identity.
- Windows installed workflows, clean-machine WebView2 provisioning, CSP negative
  tests, active-work shutdown, and visual editor behavior require evidence tied
  to final source/artifact identity. Historical local results are insufficient.
- Linux/macOS smoke workflow configuration is not proof of execution. A Linux
  cloud host cannot establish Windows GPU or WebView2 acceptance.
- Restricted compute is experimental; no automatic application speedup is claimed.
- The existing HyvUI archive has font notices but no verified top-level software
  license grant. No new third-party redistribution-rights claim is made.

### MCP shutdown continuation

Linux cloud reproduction confirmed that the checkpoint CLI stayed alive after
stdin EOF with unread stdout, both during initialization and after a completed
handshake. The updated adapter passes seven CLI process regressions in
`tests/mcp_shutdown.rs`: blocked initialization, blocked pre-initialization error,
blocked post-initialization output, closed stdout before/after initialization
with stdin open, oversized input with stdin open, and buffered responses at EOF.
Blocked-output children must exit within five seconds without the test first
dropping unread stdout. A separate ten-second watchdog covers blocking setup I/O
and kills/reaps a timed-out child. An additional regression proves that watchdog
using a deliberately stalled subprocess; its child-only fixture is otherwise
ignored. Expected exit codes prevent premature crashes from passing as shutdown.

Adapter tests cover prompt cleanup during blocked initialization/error replies,
responses resuming during grace, buffered requests at EOF, byte-fragmented input,
bounded backlog, input/output failure races, exactly-once backend cleanup,
combined failures, frame limits and latched output interruption. Native
Unix tests cover blocked read/write worker joins, ordered partial writes and
restoration of shared descriptor flags after both workers finish. The Windows
agent code cross-compiles for `x86_64-pc-windows-gnu`; this is not Windows runtime
evidence. Existing Windows and Linux/macOS workspace CI commands include these
regressions, but were not dispatched from this cloud session. Native Windows and
macOS process acceptance remains outstanding, as does installed-app acceptance.

Cloud Rust validation completed with 381 tests passed, 0 failed and 13 ignored
using `cargo test --workspace --all-features --locked --no-fail-fast`. The ignored
count includes the subprocess-only watchdog fixture exercised by its parent test;
other ignored/hardware checks remain separate gates. Locked all-feature workspace
checking, desktop checking without default features, formatting, and strict agent
Rustdoc also passed. Thirty repeated native-directory-input failures retained
their I/O cause, and thirty pipelined EOF runs preserved both expected replies.

An earlier workspace attempt failed
`native_store::tests::a_replaced_lease_file_does_not_prove_abandonment` with a
preserved-count assertion. That untouched test passed alone and in all 29 desktop
unit tests on an archived checkpoint, and passed in the complete final run. No
native-store assertion, recovery budget or implementation was changed; the
initial failure is retained here rather than attributed to a confirmed cause.

Private operational ledgers, reviewer payloads, raw local captures, and the local
Beads database do not automatically travel with a cloud clone. Original local
records are preserved separately, not converted into invented passing evidence.

## Reproducible checks

Use Node 24 and the platform prerequisites declared in the workflows/manifests.
From the repository root:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-features --locked
cargo check -p revenant-desktop --no-default-features --locked
cargo test --workspace --all-features --locked
npm ci --install-links --prefix packages/client
npm run check --prefix packages/client
npm run check:ui --prefix packages/client
node scripts/check-api-docs.mjs
npm test --prefix packages/client
npm run test:ui --prefix packages/client
node --test scripts/inspection-semantic.test.mjs scripts/evaluate-agent-workflow.test.mjs
python -m unittest discover -s scripts/tests -p 'test_benchmark_*.py'
```

Strict Rust documentation uses `cargo doc --workspace --all-features --no-deps --locked`
with `RUSTDOCFLAGS='-D missing_docs -D rustdoc::broken_intra_doc_links -D rustdoc::invalid_html_tags'`.
The workflows additionally build migrated reference apps and an outside-checkout
scaffold, compare compiled contracts, and record package provenance. Consult them
for exact platform-specific commands.

## Acceptance boundaries

- **Packages:** build real consumers with the normal CLI/SDK workflow and verify
  installer/source identity; installer output alone is not installed acceptance.
- **Windows:** exercise folder access, previews, operations, bounded results,
  persistence, failures, cancellation and close with active work without Vite or
  development tools. Measure WebView2 absence before claiming offline provisioning;
  never remove the user's host runtime merely to manufacture a baseline.
- **Editor:** verify actual Rust/generated TypeScript hover/source navigation;
  language-server output alone does not establish visible editor behavior.
- **Performance:** follow [BENCHMARKS.md](BENCHMARKS.md), recording source, workload,
  hardware, contention, transfer/preparation costs, memory, bounds and cleanup.
  Tiny fixtures or uncontrolled historical comparisons do not prove a speedup.

See [Windows acceptance](WINDOWS-ACCEPTANCE.md), [platform smoke](PLATFORM-SMOKE.md),
[SDK recovery](SDK.md), and [native workflows](native-workflows.md).
[Earlier 0.2 evidence](history/VALIDATION-0.2.md) remains explicitly historical.
