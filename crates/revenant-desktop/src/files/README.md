# Native folder indexes

`FolderIndex` is the desktop provider's metadata store. It is not a resource
registry or file reader. Its records contain native paths and IDs; the host turns
only the requested page into leased core `FileEntry` objects with core `Handle`
objects. Never serialize an `IndexedFile` access path to the webview.

## Host lifecycle

1. Construct an index with a selected directory, a cache directory, and the
   provider's recursion flag. Construction creates SQLite tables without walking
   the folder. It returns an `Arc<FolderIndex>`.
2. Schedule `scan(cancel, notify)` on the host's bounded blocking executor.
   Notifications run outside database locks and can query the index. There is
   one active scan per index; a second scan returns `scan_running`.
3. Query pages with a `FileQuery`, offset, and limit. Zero requests the 128-row
   default; larger limits clamp to 512. A page's matching count, metadata, state,
   discovered count, generation, and warnings describe one committed revision.
4. Once state is `Ready`, create a `selection(&query)` for a job. Stream its
   iterator into a bounded host queue. Do not collect the selection into a Vec.
5. Keep the selection until its job has consumed or abandoned it. Dropping it
   deletes the frozen ID rows while SQLite remains open. Its index Arc protects
   the database lifetime. Dropping the final index owner deletes temporary data.

Queries, snapshot creation, and selection/final-owner cleanup also perform
blocking storage work. The host should schedule them away from its UI thread,
especially deletion of a large selection. This module starts no cleanup threads.

`Ready` can include warnings. `Cancelled` and `Failed` contain inspectable partial
metadata but reject batch selections. Cancellation publishes its final partial
chunk/status, then `scan` returns the core `cancelled` error. Fatal scan errors
publish `Failed` and a diagnostic before returning. A later scan resets the
inventory; native file IDs are never reused for different files.

## Query semantics

Search is an optional **literal substring** of the name or slash-separated
relative path. Both stored fields and the search use Rust Unicode lowercase.
This is not locale-specific collation or full Unicode case folding. Percent,
underscore, and backslash do not have wildcard meaning.

The primary sort is `name` by default, or `size`, `modified`, or `path`. An unknown
sort returns `invalid_query`. Descending reverses the primary key only. Ties
always use ascending relative path, exact native path bytes, then ID. Unknown
modification times sort first ascending and last descending. File size retains
the full core unsigned 64-bit range; SQLite sorts a padded 20-digit key.

Every committed scan chunk changes generation, including the final state change.
Restart interactive offset paging if a later page has a different generation.
A `FileSelection` captures both the ordering and IDs in SQLite after readiness,
so rescans cannot insert, remove, or reorder its items. Its `total()` and
`generation()` refer to creation time. Its iterator buffers at most 128 files,
uses ordinal keyset paging, and stops after yielding a database error once.

The snapshot freezes metadata and native identity records, **not filesystem
contents**. Files can be renamed, removed, or changed before execution. The host
must validate actual access and retain its resource leases while an operation
runs. Do not infer that a snapshot holds open files or grants read permission.

## Bounds and traversal

| Storage/work | Bound |
| --- | --- |
| Rust scan insertion buffer | 256 files |
| Rust directory enqueue buffer | 256 paths |
| Directory traversal streams | One shallow directory walk at a time |
| Interactive query page | 128 default, 512 maximum |
| Selection iterator buffer | 128 files |
| Retained warning examples | 32 |
| Path/message text per warning | 1024 Unicode scalar values each |
| Cached substring counts | 16 search keys, 1024 UTF-8 bytes per key |
| SQLite page cache | 4 MiB target; sort temporary data is file-backed |

The complete inventory, pending-directory frontier, and frozen selections live
in SQLite. A shallow `ignore` walk visits each queued directory; it never enables
recursive walker buffering or directory sorting. Consumed queue rows are removed
inside bounded insert transactions rather than committing once per directory.
The queue is cleared at completion/cancellation, on a recoverable fatal failure,
or before another scan starts. Hidden files and
ignore files are not implicitly excluded. Symlinks and Windows reparse points,
including junctions, are skipped before enqueue and again before a queued
directory is traversed; the root is checked at setup
and before each scan. The index's own private cache directory is excluded if it
lies beneath the selected folder. Neither file inventory nor traversal breadth
is collected in a Rust Vec.

Permission/metadata failures and disappearing entries add warnings and skip the
affected record. Warning `total` continues increasing after the example buffer
fills. Unavailable modification times produce a warning but retain the file.

## Large-folder cost

Unfiltered totals use the committed count directly. Search totals are cached
independent of sort and updated against each committed insertion chunk. This
avoids a full recount on each progress notification. Rescanning clears the
cache; failed commits never install count deltas. Known empty/out-of-range pages
return without another row scan, and partial last pages limit reads to the
remaining match count.
A cache miss for a literal substring still requires a SQL scan. Sort indexes
cover the supported primary orders, and SQLite performs any remaining tie sort.
Interactive offset queries have increasing offset cost. Jobs instead use the
selection's ordinal keyset cursor. Materializing/deleting a selection scales
with its match count and uses disk. Cleanup checks only older indexed epochs,
so disposing a current selection does not scan the current inventory to prune
it. These describe the algorithm; 500k-file latency requires the later validation
phase rather than an assumed timing claim.
