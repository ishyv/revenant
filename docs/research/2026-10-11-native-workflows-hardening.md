# Native workflow implementation hardening

This review continued the framework milestone on unpublished `main`. The broad
specification review passed for its approved framework and representative-fixture
scope. Two runtime defects were reproduced, corrected and independently reviewed.
Beads records are `revenant-378.4.1` and `revenant-378.5.2`.

## Scratch admission

`Work::stream_ordered` collected its input before requesting scratch admission.
The rejecting-host regression observed 32 iterator advances where none should
occur. It now reserves planned input/result capacities before advancing the
iterator, prepares vectors fallibly and retains the reservation through ordered
consumer settlement.

Quality review caught an additional boundary: an exact empty input was rejected
when its item exceeded the scratch limit. Tests reproduced insufficient/zero
scratch and cancellation precedence. Proven-empty inputs now succeed after the
checkpoint and configuration validation; oversized nonempty inputs still reject.

The reservation models `T`/`Option<R>` window capacities. Job/control allocations,
temporary captures, allocator overhead and pointees remain separate costs. An
unknown-length input requires admission before its first probe, even when empty.
These counters do not establish a bound on total process memory.

## Native coverage acknowledgement

A failed late-registration hide left its surface marked hidden. Another cover
could then resolve without native acknowledgement. Both failure and deferred
retry regressions failed before the correction.

The coordinator now distinguishes attempted hiding from acknowledged hiding.
Further covers retry and await unacknowledged live surfaces; a rejected cover
retains earlier coverage. Attempted hides remain eligible for final restoration.
The public API and compatibility tuple are unchanged by these corrections.

## Verification and consumer delivery

| Command | Final result |
| --- | --- |
| `cargo test -p revenant-core --all-features` | 67 passed; one explicit measurement benchmark ignored |
| `cargo test -p revenant-desktop --test work_scheduling` | 4 passed, including nested one-worker and cancellation settlement |
| `cargo clippy -p revenant-core --all-targets --all-features -- -D warnings` | Passed |
| Client `npm test` | 81 passed |
| Client `npm run check` | Passed |
| Client `npm run test:ui` | 3 mounted UI tests passed |
| Client `npm run check:ui` | Zero errors and warnings |
| Refreshed consumer `revenant check --json` | All SDK, Rust, frontend preparation and frontend service stages succeeded; no diagnostics or source changes during checking |

The CLI was rebuilt with `cargo build -p revenant --bin revenant`. Ordinary
`sdk upgrade` was run from `examples/native-workflows` using
`<revenant-checkout>/target/debug/revenant.exe`. The retained final backup is
`.revenant/sdk-backups/<local-backup>` in that consumer.

Inspection reports manifest 6 / protocol 5 / SDK 4, a current source-to-facade
relationship and equal complete start/completion source identities: 135 files,
1,292,083 bytes,
`sha256:a0a35543382dac36cb7d87ddc4ca6f5afe19bf58b3a6037404b18589ae311eea`.
Both changed modules match their consumer SDK snapshot byte-for-byte:

| Module | SHA-256 |
| --- | --- |
| `crates/revenant-core/src/tasks/services.rs` | `3d280152219de208776a8dd86b3e367fa8c72bbfccc5f842e622f6fbd7b1f8c9` |
| `packages/client/src/overlay.ts` | `8640a163fad3e3648788e425f0b3ac2cfc83bc4c4ba48ab438c577973d077193` |

No handwritten SDK changes, performance measurements or new packaged runtime run
were made in this pass. Earlier packaged/measurement evidence retains its original
build identity in the [integration report](2026-10-09-native-workflows-integration.md).
The compute promotion decision remains NO-GO. Strata migration, the unresolved
original WebView restart cause and unavailable physical/platform evidence remain
separate work. No commits, pushes, remote synchronization or CI dispatch occurred.
