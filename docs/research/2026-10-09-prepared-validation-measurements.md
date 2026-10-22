# Prepared validator measurements

The release fixture `crates/revenant-core/tests/prepared_validation_benchmark.rs`
measured actual generated FileEntry contracts on this Windows host. It used two
warmup generations, five measured generations and 1,000 calls per stage, alternating
the prepared and ad hoc order. Concurrent external activity was not controlled.
The raw report is `target/prepared-validation-benchmark.json`; it records executable,
compiler, resolved lockfile, compiled-source, schema and input SHA-256 identities.

| Workload | Schema / input bytes | Prepare once, median | Ad hoc validation, median | Prepared validation, median | Native roundtrip ad hoc / prepared, median |
| --- | ---: | ---: | ---: | ---: | ---: |
| One FileEntry | 2,049 / 253 | 217.8 µs | 403.224 µs | 3.0122 µs | 530.588 / 9.3491 µs |
| 512 FileEntry records | 2,124 / 124,087 | 407.8 µs | 781.978 µs | 633.582 µs | 6,172.757 / 6,325.734 µs |

Each generation constructed one prepared validator and retained it for 2,000
validations: 1,000 isolated validations and 1,000 roundtrips. The equivalent ad hoc
path invoked `validate_json` 2,000 times. These are explicit API and compiler-entry
counts established from the call sites and source, not internal profiler counts.
Preparation is shared with the installed generation, rather than repeated per call.

The isolated roundtrip covers typed model → JSON Value → schema validation → bytes
→ JSON Value → typed model. It excludes operation lookup, resource authorization,
task lifecycle, IPC, frontend publication and application work. The raw report gives
isolated validation-stage contribution estimates for every round. Because those
stages were timed separately under uncontrolled activity, they are noisy and need
not add to exactly 100%; some exceed the independently timed total. Full application
contribution remains unknown. No application speedup is claimed.

The large fixture is a large serialized typed collection with the same nested record
schema, rather than a large unrelated schema. Prepared validation reduces repeated
preparation, but the measured large-payload roundtrip did **not** improve. Serialization
and scheduling noise remain significant. The fixed API preparation counts and pinned
generation regressions establish the contract change; these timings describe its
local cost and limits.

Reproduce with `cargo test -p revenant-core --release --test prepared_validation_benchmark
-- --ignored --nocapture`. Functional prepared-validator parity, concurrent validation
and retired-generation reclamation tests are separate from this ignored measurement.
