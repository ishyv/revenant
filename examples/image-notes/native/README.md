# Luma native cache

Luma retains generated JPEG renditions within a **512 MiB payload budget**. When a
rendition's original changes, older renditions for that asset become obsolete.
Publication evicts the least recently used owned renditions until the new image
fits. Reading a cached rendition updates its use order. Unavailable catalog
entries are pruned when cache usage is requested.

`images.cacheUsage` reports total, managed and unmanaged payload bytes, the entry
count, the budget and whether preserved files consume the entire budget. Byte
counts use Revenant's canonical decimal strings. `images.clearCache` returns the
removed byte/entry counts and the remaining usage. Clearing retains catalog
records, registered folders, annotations and original images. Viewing an image
after clearing regenerates its rendition.

Reads, decoding, publication, eviction and clearing hold the same operating-system
file lease across Luma processes. A clear waits for the current writer rather than
removing its temporary file. Waiting is cancellation-aware and limited to five
seconds; a busy cache returns `cache_busy` for a later retry. File failures return
classified errors, and an interrupted clear can have removed some entries before
reporting failure. Already returned image payloads own their bytes and remain
valid when disk entries are evicted.

The cache's ownership marker and SQLite write journal identify exact generated
filenames. Ownership is committed before a write starts. After a forced exit, the
next lease holder recovers only those recorded incomplete writes, including a
rename interrupted before index publication. The separate ownership marker also
allows SQLite to recover an abandoned rollback journal. Cache reads enforce the
700 KiB rendition limit before accepting bytes.

Unmarked legacy files, unfamiliar paths, directory links and junctions are
preserved. Unrecognized payloads still count toward the budget; their link targets
are never traversed. If they leave insufficient space, rendering succeeds without
disk caching and returns a `cacheWarning` explaining why. If these files alone
exceed the budget, usage reports that excess instead of claiming it was bounded.
Clearing removes only owned files, so preserved content requires a user's separate
decision. The SQLite index, ownership marker and stable lease file are cache
metadata and are excluded from the rendition payload budget.

Native regressions use small injected budgets and separate test processes for LRU
eviction, shared budgeting, locked files, active writers, forced exits, interrupted
SQLite writes and repeated recovery. They also cover cache regeneration, retained
notes/originals, malformed ownership paths and Windows junction preservation.
