# Native workflows fixture

This standalone non-disk application exercises issued scope ownership, generated
source-model projections, bounded native map/reduce, explicit keyed grouped reads,
optional-slot failures, native capture/compute/commit, 50,000 scene primitives,
500-key patches and overlapping covered dialogs. Bulk models stay native.

Use ordinary `revenant sdk upgrade`, `revenant check` and `revenant build` from
this directory. Run the packaged output outside the checkout. The generated
facade is produced from the compiled Rust contract. Package execution requires
neither Node nor Cargo.

Keep local acceptance runs and evidence under
`<local-evidence>/<run-name>`, outside the source checkouts
and within the existing Revenant project grouping.

The deterministic seed is 378; CPU transforms use 32 wrapping-u32 iterations and
an exact u64 ordered checksum. Choose 500k or 1M entries. Fixture results do not
establish Strata's first-map or disk-scan performance gates.
