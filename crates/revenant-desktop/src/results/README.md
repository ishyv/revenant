# Native batch outcomes

`ResultStore` writes completed item inputs, outputs, and errors as SQLite JSON
records. It does not schedule work, retain a whole batch in Rust, or manage the
task's planned total. The host creates one store per batch and streams outcomes
from its bounded worker queue.

## Writing and querying

`BatchOutcome` contains a zero-based source `ordinal`, the actual per-item JSON
`input`, and exactly one present `output` or `error`. A successful JSON null is
represented by `Some(Value::Null)`; serialization omits absent slots and preserves
present nulls when deserializing. Errors may hold the serialized core error DTO.
Source ordinal is independent of completion order, so parallel workers can write
out of order while the UI sees results in source order.

Use `append_many` to flush at most 128 outcomes per transaction. It consumes an
iterator and buffers at most that chunk. The SQL insert and completed counts
commit together. Duplicate source ordinals return `duplicate_outcome` rather
than replacing an existing record. An invalid/duplicate item rolls back its
entire current chunk; earlier chunks remain committed. A retry must respect that
partial-commit boundary. `append` intentionally commits a single item for small
or immediate writes; calling it 500k times creates 500k transactions.

For large jobs, the host should collect only a bounded completion chunk and
flush it using `append_many`. Flush the remaining completed chunk **before**
publishing final success, cancellation, or failure. Otherwise counts and pages
omit items that finished but were still buffered outside the store.

`counts()` reads maintained `total`, `succeeded`, and `failed` values without
scanning result JSON. Missing ordinals are pending/unrecorded items, not failures.
Task cancellation and planned batch counts remain with the task scheduler;
per-item cancellation errors can be recorded as failed outcomes.

`query(offset, limit)` returns items in ordinal order plus coherent counts and
generation. Zero uses 128 rows and requests above 512 clamp to 512. Offset is a
position among completed records, not a source ordinal. Every committed chunk
advances generation. Earlier ordinal completions can shift pages during a job;
restart offset paging if the generation changes. Once writes stop, paging is
stable. The host enforces operation-specific input/output JSON size limits;
bounded row counts alone do not bound arbitrary JSON bytes.

## Ownership and cleanup

`new(cache_dir)` returns an `Arc<ResultStore>` for plain JSON outcomes. For
outcomes referencing native resources, use `new_with_owner(cache_dir, owner)`.
The single `Arc<dyn Send + Sync>` owner should be the host's root batch scope
guard, retaining the index and any resource/file leases required by the results.
Its final Drop should release that scope's resources. JSON handles alone do not
retain resources. Keep this guard alive rather than prematurely disposing its
scope while result consumers still need it.

`store.lease()` produces a clonable `ResultLease`, which dereferences to the
store for queries and counts. Dropping the scheduler's Arc therefore does not
remove result data or release the owner while an inspector holds a lease.
Dropping the final store/lease closes the SQLite connection, removes its private
temporary directory, then releases the batch owner. This ordering permits
temporary cleanup on Windows without an open SQLite connection.

All store methods and final cleanup are synchronous storage operations. The
host owns bounded scheduling and should keep large writes, queries, and final
owner disposal off its UI thread; this module creates no background workers.

Stores are temporary inspection state. Export required results before releasing
the last owner; creating a result store does not create a durable project file.
