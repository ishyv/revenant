# Native workflows framework integration

Implementation is on unpublished `main`, preserving the existing working tree.
The starting Git HEAD is `564e4142c8c4d1b57f2473f7a09d05d16da2a0d4`; no commit,
push, remote tracker synchronization, CI dispatch or release publication was made.
The host/client tuple is manifest 6, protocol 5, SDK revision 4. Project layout 3
and package version 0.4.0 remain unchanged.

The framework supplies issued ownership, resolved application paths, shared
generation validators, source-owned wire projections, bounded CPU/I/O helpers,
scope-owned grouped reads, capture/compute/commit controllers, awaited covers,
incremental scenes and restricted experimental numerical kernels. Public API and
limits are described in [native-workflows.md](../native-workflows.md). Services
remain in the existing core and desktop crates.

The standalone `examples/native-workflows` fixture exercises those public APIs
without disk scanning: seeded 500k/1M transforms and ordered exact checksums,
generated nested/decimal projections, keyed required/optional reads, a persistent
latest-job controller, 50,000 scene keys, 500-key patches and overlapping covers.
Only its two explicitly annotated reads are enabled for agent access. Write and
scene operations are not agent tools. The generated facade is published through
ordinary SDK commands, never authored by hand.

The final ordinary SDK upgrade and production build succeeded. Inspection reported
135 complete source files / 1,289,637 bytes with identical start/completion digest
`sha256:84c53ecdd01fd17f1ad7006b29712c56aa97f2954e49bc1445fae0d7eb7bdb27`,
`sourceChangedDuringBuild=false` and `sourceToFacade=current`. The static packaged
executable SHA-256 is
`87dac4907b2ebd89e4051ba5ad08555b3e3dad6eb535a73be97f9138f100505c`.
It ran outside the checkout with its PATH restricted to Windows System32:
neither Node nor Cargo was discoverable. Its artifacts now live under
`<local-evidence>/native-workflows-20261009-complete`.
The external observer retained developer tools; they were not available to the app.

The packaged MCP session exposed only `fixture.summary`, `fixture.agentSummary`
and bounded task lifecycle/result controls. A maximum-u64 decimal checksum survived
four 32-byte result pages, host paths were available, and MCP EOF exited zero.
The 500k and 1M UI reads produced exact checksums `1073747902252720` and
`2147482922439008`, independently checked by composing the frozen integer transform.
The 1M optional slot failed while the required group remained ready with matching
`entries-1000000` request/data keys. The 500-key patch button was exercised without
an application error; patch revision/upload correctness is established separately
by the graphics and latest-job tests.

Actual Win32 observations tracked the same native scene HWND at 1023×302 throughout
the cover sequence: visible with no dialogs, hidden with one dialog, hidden with
two, hidden after closing the second, then visible after closing the last dialog.
Final DOM focus returned to “Open dialog”. These are native window observations,
not an inference from WebView canvas screenshots. The scene contains 50,000 keys.
Raw observations are `package-acceptance.json` and `native-ui-evidence.json` in that
acceptance directory; the checked observer and evidence scripts are under
`scripts/native-workflows-*.py`.

After the user corrected artifact placement, all four run directories were moved
under `<local-evidence>`. All twelve files retained their
SHA-256 hashes and lengths (208,937,530 bytes total); the misplaced top-level
directory was removed by the move. Raw captures retain their original execution
paths as historical evidence. This relocation does not claim a new runtime run.
Further local acceptance artifacts belong under the existing Revenant grouping.

The actual run found a scope-admission bug missed by mocked tests: grouped reads
rebound parent-owned Dataset handles into a child scope. Groups now preserve the
declared owner and cancel/dispose each load's query controllers, awaiting cleanup
before replacement admission. Strict native authority remains intact. Two added
regressions reproduced the original failure and verify cleanup acknowledgement.

Local quality evidence includes core/SDK/macros default regressions; six CPU IR
tests and two typed SDK compute tests; default graphics and GPU-enabled tests;
four actual Windows DX12 device/compiler/error/asset lifetime tests; twelve desktop
agentic and four scheduler regressions; 79 client tests; three mounted UI tests;
client types/UI checks; and 16 generated fixture schema parity checks. Strict
Clippy and documentation builds were run separately for default, CPU experimental,
graphics experimental and native-surface configurations. Workspace Rust formatting
is clean. The mounted fixture regression is self-contained and does not depend on
ignored generated SDK files; packaged execution separately checks real generation.

The scheduler [measurement report](2026-10-09-native-workflow-scheduler-measurements.md)
records one/four sessions, 500k/1M entries, blocked I/O, nested budget one, twenty
runtime cycles and compiled identities. Interactive p95 was 0.1205–0.1973 ms on
the measured host, below the initial 50 ms ceiling. The bounded CPU helper was
1.53–4.70 times slower than the same serial transform. Uneven I/O progress and
unknown external allocations are disclosed. The [validator measurement](2026-10-09-prepared-validation-measurements.md)
records real small/large payloads and preparation counts; its large native
serialization roundtrip did not improve. Neither report establishes an application
throughput gain.

Graphics measurements and limitations are in
`crates/revenant-graphics/tests/native_workflows_graphics.md`. A settled 1% patch
uploaded 40,000 of 4,000,000 bytes, meeting the initial 10% upload target. Four
surfaces shared one device, pipeline and immutable atlas. Twenty lifecycle cycles
and explicit test-device destruction were exercised. CPU scene publication still
copies the immutable primitive vector. Picking measurements did not justify adding
an index. Actual native window rendering was measured; CPU render-call timing is
not a GPU timestamp or proof of compositor presentation at 60 Hz.

The [recovery report](2026-10-09-native-workflows-recovery.md) records the host-lockfile
freshness reproduction/fix, ordinary isolated reference upgrades, byte-exact restore,
actual interrupted CLI recovery, all three normal consumer checks and real restart
ownership traces. Twenty immediate relaunch cycles passed with all 160 held kernel
process probes signaled. The original reported WebView HRESULT was not reproduced;
a descendant spawned after the exit-handle snapshot remains an explicit mechanism
limit. The corresponding finding remains open.

Restricted compute completed with **NO-GO** for public promotion. The resident GPU
workload lost to an optimized exact CPU algorithm and the small workload was dominated
by transfer/preparation costs. [CPU control](2026-10-09-compute-cpu-control.md) and
[GPU decision](2026-10-09-compute-gpu-decision.md) include complete-cost and conformance
evidence. Experimental support stays opt-in; exact wide accounting stays on CPU.

Physical driver loss, mixed DPI, other platforms, clean-machine installation and
compositor/GPU timing remain unavailable evidence. Strata source refactoring,
first-useful-map timing and real disk-scan comparisons are separate application
work. Fixture results cannot close those application gates. Beads contains the
phase/finding records and demonstrated closures; this document records evidence,
not a replacement task list.

On 2026-10-10, the complete [framework benchmark suite baseline](2026-10-10-controlled-benchmark-baseline.md)
passed all 21 CPU/GPU cases with 15 repetitions per metric and immutable build
identities. At the user's direction, desktop utilization was recorded as
diagnostics: observed outside load was substantial, so this is not quiet-host
acceptance. All raw measurements and rejected attempts remain under
`<local-evidence>/benchmarks`. The report distinguishes
480 individual reads from 15 independent repetition percentiles and preserves
the slower bounded CPU result and scene-patch allocation findings. It does not
close the application or unavailable physical/platform gates above.

The [2026-10-11 hardening review](2026-10-11-native-workflows-hardening.md) records
two further corrections: scratch admission before lazy input materialization and
acknowledgement of late native hides before admitting another cover. Its current
source/SDK checks are separate from the earlier packaged and measured builds.
