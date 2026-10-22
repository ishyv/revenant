# Typed task updates

Use application updates for a bounded current view of ongoing work. Progress
continues to report completed work units; the operation return value remains its
terminal result.

```rust
use revenant_sdk::{Contract, DecimalU64, Result, TaskContext, operation};

#[derive(Contract)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScanUpdate {
    discovered: DecimalU64,
    current_directory: String,
}

#[operation(id = "scan.start", updates = ScanUpdate)]
async fn scan(context: TaskContext) -> Result<String> {
    context.publish(ScanUpdate {
        discovered: 42_u64.into(),
        current_directory: "Documents".into(),
    })?;
    Ok("finished".into())
}
```

The operation attribute compiles the update DTO alongside its input and output.
Installation prepares immutable input, output and update validators once per
contract generation. Admitted tasks share the pinned update preparation through
completion; repeated publication does not compile the schema again. Replacing
or disposing a generation releases its preparation when admitted work finishes.
The public ad hoc `validate_json` function continues to compile each supplied
schema independently.
The generated operation returns `Task<Output, ScanUpdate>` from `.run()`:

```ts
const task = app.operations.scan.start.run({});
const stop = task.updates.subscribe(update => {
  if (update) console.log(update.discovered);
});
try {
  console.log(await task.result);
} finally {
  stop();
  await task.dispose();
}
```

`task.updates` is a Svelte readable store. It initially contains `undefined`,
then retains only the newest validated application value. Late subscribers get
that value immediately. There is no update history: this is appropriate for
scan counts, current status, or a small analysis summary, not an event log or
the scene/file inventory itself.

Each native task snapshot has a lossless decimal `revision`; its optional
`update` envelope records the revision at publication. The client ignores older
task snapshots and repeated update revisions, including delayed refresh replies.
Desktop delivery throttles task snapshots to at most twenty per second and
always forwards finalization. Intermediate application snapshots can be skipped.
Applications should publish only at a useful rate, such as ten updates per second.

Publication serializes through a bounded writer and rejects values over **64 KiB
of encoded JSON**, including UTF-8 bytes and JSON escaping. It validates against
the operation's compiled update schema. Failure retains the last valid value.
`TaskContext::publish` accepts serializable Rust values, but the compiled schema
is the authoritative runtime fence. Structured errors include
`task_updates_undeclared`, `invalid_task_update`, `task_update_limit`,
`task_not_running`, `cancelled`, and `task_terminal`.

Cancellation prevents further updates. Finalization preserves the last valid
value separately from its result, then makes the task immutable. Disposing the
task detaches subscriptions and releases its native ownership. Batch operations
retain individual updates in their local child contexts; batch parents expose
progress and paged results rather than a merged application-update stream.

Advanced hosts configure the pinned schema through
`TaskContext::configure_updates` before running user code. Standard registry and
prepared-operation dispatch do this automatically. Reconfiguration with another
schema fails, preserving the operation contract for the task lifetime.
