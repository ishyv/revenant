# Typed native resources

Ownership scopes are issued by the host. Core integrations call
`ResourceRegistry::create_scope(parent)` and retain the returned identity; callers
cannot register a chosen scope name. Each registry has an independent instance
identity and a monotonically increasing, non-reused sequence. Disposing a scope
reclaims its active records and descendants. Repeated disposal of its issued
identity succeeds, but malformed, foreign, and never-issued IDs are rejected.
This requires manifest 6 / protocol 5 / SDK revision 4.

`Resource<T>` is a scoped identity for native Rust state. Only its `Handle` crosses
the JSON boundary; `T` needs no serialization, schema, or TypeScript implementation.
An operation creates scope-owned state with `TaskContext::own(value)` and obtains
a retained `NativeLease<T>` with `context.lease_resource(&resource)`.

```rust
use revenant_sdk::{operation, Resource, Result, TaskContext};

struct Analysis { count: u32 }

#[operation]
fn create_analysis(context: TaskContext) -> Result<Resource<Analysis>> {
    context.own(Analysis { count: 0 })
}

#[operation(read_only)]
fn count(analysis: Resource<Analysis>, context: TaskContext) -> Result<u32> {
    Ok(context.lease_resource(&analysis)?.count)
}
```

Macro-generated input admission visits resources nested in contracts. It checks
scope, ID, generation, kind, and registry ownership before user code starts.
Obtaining a typed lease additionally checks the actual Rust type. Deserializing a
resource or calling `Resource::from_handle` merely wraps an untrusted identity.

Task leases survive owner release. Task completion, interruption, or queued
cancellation releases their captured references. A scope owner created by `own`
survives a successful operation so another operation can use its result. Explicit
registry owner release or scope disposal removes that owner; the value drops once
after its last lease drops. Handles cannot be saved across application restarts.
External native side effects must still cooperate with task checkpoints.

When a task fails, acknowledges cancellation, or is interrupted without returning
a result or successful item output, it rolls back registry owners created by that
task's `own` calls. Repeated failed session creation therefore does not retain
abandoned native state until the UI scope closes. Previously registered resources
borrowed by the task keep their owners; an independent admitted lease still keeps
its native value alive after rollback. Successful results and partial item outputs
preserve their scope owners for later operations. Cleanup runs outside task locks,
so native destructors may safely inspect task state or use the registry.

Native hosts use `NativeResourceLease` for erased input and output retention.
`capture_native` admits such a previously validated lease without requiring its
original owner to remain. Native operation results are retained independently of
the executor and can be used while their native task history owns them.

Call `await app.releaseResource(handle)` when a native analysis or review plan is
no longer needed. This explicitly releases the scope owner and blocks new
operations from admitting that handle, including through retained task results.
Already admitted tasks may finish safely. A completed task's result lease keeps
the value alive until that task is disposed, so also dispose completed tasks when
their result history is no longer useful. Wrong scopes and changed identities
are rejected; repeated release reports `stale_handle`. This method accepts native
resources only. Existing file and folder disposal APIs keep their behavior.

Native hosts use `ResourceRegistry::revoke_native` for explicit disposal, and
check `NativeResourceLease::is_revoked` before admitting a retained result into
new work. Ordinary registry `release` preserves the existing retained-result
admission behavior. The revocation flag belongs to the leases themselves and
disappears with them; the host does not accumulate released-handle tombstones.

The folder picker also returns `Folder.capability`, accepted by a Rust operation
as `FolderHandle`. `context.lease_resource(&root)` yields `NativeFolder`, whose
`path()` is the admitted canonical root and whose `directory()` retains the open
directory object. The folder's display `path` cannot authorize an operation.
The host rejects a selected symbolic link or junction before canonicalization.
Directory traversal still needs to exclude descendant links and revalidate targets
before destructive actions; holding a root capability does not freeze a filesystem.

The existing file `ResourceLease`, byte-read, checksum, paged collection, and
folder bookmark APIs retain their behavior. Folder descriptors keep the older
opaque folder ID alongside the new typed capability.

Development inspection separates file-provider resource counts from
`ownership.nativeOwners` and `ownership.nativeLive`. The latter includes native
allocations retained by admitted task, result, or typed leases after owner
release. `ResourceRegistry::native_resource_counts()` exposes the same pair to
native hosts. Accounting retains no identities or release history and uses
constant storage. These counts describe admitted allocations; they do not count
an author's internal clones, GPU driver allocations, or prove leak freedom.
