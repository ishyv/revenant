# Native workflows SDK recovery and restart evidence

Local Windows evidence for `revenant-ldj.17.2`, `revenant-ldj.20` and phases 1/7 of
`revenant-378`. Authored reference examples were not edited. Consumer copies live
under `target/native-workflows-consumers/{file-media,image-notes,prime-sieve}`;
`image-notes` is Luma. The restart-only PrimeSieve copy uses the distinct identifier
`app.revenant.prime-sieve-restart-proof` to avoid sharing a profile with an authored
or installed instance. No commit, push, remote sync or CI dispatch occurred.

## SDK freshness: reproduced cause and correction

The pre-fix CLI was a freshly built 0.4.0 executable using manifest/protocol/SDK
6/5/4. Its SHA-256 is
`feb97e8514bca5dd08be6a20ca2e90094350c4314ab4b4000c75a98e38cb5020`.
The copies started at SDK/client 3, manifest 5 and protocol 4.

Normal `sdk upgrade` succeeded on FileMedia and PrimeSieve, but immediate
`inspect --json` reported `sourceChangedDuringBuild=true` and
`consistency.sourceToFacade=stale`. No authored source edit was needed. FileMedia's
fingerprint grew from 172 files / 1,389,291 bytes to 173 files / 1,512,313 bytes.
The 123,022-byte delta exactly matches the newly generated
`.revenant/desktop/Cargo.lock`. PrimeSieve's analogous delta was 123,025 bytes,
again precisely its new desktop host lockfile. Native `Cargo.lock` was already
resolved before compilation by the existing SDK upgrade implementation; that
earlier correction did not settle the generated host's separate lock.

An initial progress message mistakenly interpreted abbreviated PowerShell object
formatting as current freshness. Full JSON inspection corrected it before any
completion claim; the actual pre-fix results above are stale.

`DesktopBuild::new` now prepares the generated host and resolves its lock with a
bounded `cargo fetch` before recording the build-start fingerprint. Fetch retains
valid existing pins; unconditional `generate-lockfile` was rejected because it
could reselect versions during ordinary development rebuilds. The SDK upgrade's
native lock generation also uses a bounded 300-second subprocess. Compilation and
publication still retain the source identity captured before compilation, so a
real authored edit remains observable as stale.

The first fixed CLI SHA-256 is
`6c524009b4dbe0ce15eaf84eddab574bc4e5e21f5b5b5cc17c3b9c33507dd9a0`.
Normal upgrades using that CLI produced identical start/completion fingerprints,
`sourceChangedDuringBuild=false`, and `sourceToFacade=current` on all three copies.
This binary included the host-lock correction; the later timeout-only native-lock
change is independently checked by the focused recovery suite.

| Copy | Compiled facade digest after fixed upgrade | Freshness |
| --- | --- | --- |
| FileMedia | `sha256:a4bb7ce66c29da2bb93f931877b883a42fb290c2bd85321b032d45993e2ffd9c` | current |
| Luma | `sha256:cf970c662a6ddb91c8a9e1d7a2ffc3318e86bd5efe57cef5aefdaf8ddbe1f38d` | current |
| PrimeSieve | `sha256:88c43a934066ce31b3ad6612c2bffaa372c73649acf7bf96df78a2a20849c358` | current |

The opt-in integration test `sdk_upgrade_recovery` passed (one test, 563.62 s).
It uses the ordinary CLI outside the checkout, adds the existing custom-native
fixture, upgrades with no native lock present, checks a successful/current build,
then makes an authored edit and verifies stale inspection. Restore recovers the
original absence of `native/Cargo.lock` and preserves that authored edit.

## Recovery state is distinct from current compatibility

FileMedia's original upgrade backup was
`.revenant/sdk-backups/<local-backup>` inside its isolated copy. Before
restore, SHA-256 hashes were recorded for all 138 files present in its managed
backup paths. Ordinary `sdk restore` recovered all 138 exactly, with zero changed
files and zero unexpectedly present paths that had originally been absent.
The recovered snapshot was the original 5/4/3 tuple; the current CLI's subsequent
`sdk check` failed as expected. Restored bytes and compatibility with a newer CLI
are separate observations. The copy was then upgraded normally again.

All four focused SDK recovery unit tests passed, including failed replacement,
successful restore/redo, interrupted save with an actual rollback filesystem
failure, and interrupted restore before/after the recorded transfer rename.
The latter cases deterministically model process exit and preserve both complete
states; they do not claim a forced live CLI interruption.

A separate actual CLI interruption was performed in the isolated
`interrupted-file-media` copy. Upgrade process 33488 exited while its recovery
journal was in `saved` state. The ordinary `sdk restore` command then exited zero:
all 138 managed backup files matched their original hashes, with zero changed
files and zero unexpectedly present paths. The local machine-readable evidence is
`target/native-workflows-consumers/interrupted-file-media/interrupted-recovery-evidence.json`.
This is additional process evidence, separate from the deterministic unit cases.

## WebView restart investigation

The existing launcher assigns the suspended host to a kill-on-close Windows job.
Termination calls `TerminateJobObject`, waits for the root process, queries job
accounting until `ActiveProcesses == 0`, and then joins its output pumps. The
development restart calls this termination before launching the replacement.
No fixed restart delay or profile deletion was introduced.

The first real-window run used FileMedia's existing identifier and Vite. It failed
the test's stricter post-termination profile-owner invariant after 77.75 s:
ancestry rooted under that isolated copy's `.revenant/run` was empty, but CIM still
reported WebView browser PID 20544, parent PID 13808, with profile
`app.revenant.file-media/EBWebView`. That process was gone on subsequent inspection;
it was not killed outside the owned job. This does not establish WebView startup
HRESULT `0x800700AA`, job escape or an unrelated reused browser as the cause.

The repeat test now persists per-cycle prelaunch/process/profile inventories,
exact `IsProcessInJob` membership against the launcher's own job handle, observed
native window/WebView readiness, termination timing and post-termination state.
It uses the distinct isolated PrimeSieve identifier. These diagnostics are CLI
process ownership observations, not an author SDK operation or application tool.

Holding kernel process handles across termination refined that result: after job
accounting reached zero, five of eight exactly owned process handles were still
unsignaled. This establishes a shutdown lifetime gap, rather than merely a stale
CIM inventory. The original HRESULT was not reproduced by this probe.

The launcher now captures exit handles from its own job before requesting
termination, verifies membership to exclude reused foreign PIDs, and waits for
those processes to signal within the existing five-second cleanup budget. The
development replacement path propagates cleanup failure before launching another
generation. Failure remains sticky; later cleanup calls cannot report success
without completing the checks. No fixed restart delay was introduced.

The immediate-replacement reproduction passed all twenty cycles in 57.18 seconds:
all 160 held kernel probes were signaled, all were members of the owned job, and
observed shutdown took 31–37 ms. Evidence is
`target/native-workflows-consumers/prime-sieve/webview-restart-evidence.json`.
This proves the observed workload, not all WebView startup races. A process spawned
after the pre-termination handle snapshot remains a limitation of this mechanism;
the recorded job accounting and real relaunch checks must not be treated as a
general proof of every descendant lifetime or physical/platform behavior.

## Consumer validation boundaries

The first FileMedia `check --json` failed on preserved authored frontend setup:
TypeScript could not find `$app/tsconfig`, generated `.ts` extension imports lacked
`allowImportingTsExtensions`, and the language-service diagnostic request timed
out. The reference's `vite.config.ts` also does not match the development wrapper's
hardcoded `vite.config.js` import. These facts are distinct from successful native
SDK upgrade and corrected inspection freshness. No authored reference source or
configuration was silently patched to produce a passing report.

The tested consumer host lock uses Tauri 2.12.2, tauri-runtime-wry 2.12.1 and wry
0.57.0; the observed installed WebView2 executable was 154.0.4258.62. This is local
desktop evidence, not exact-commit CI, another platform, or clean-machine evidence.

## Final ordinary consumer checks

The earlier attribution of the frontend failures to authored setup was incomplete.
SDK upgrade had replaced installed dependencies without generating SvelteKit's
`node_modules/$app/tsconfig.json`. The author's ordinary `npm run check` generated
that configuration before checking types. Revenant's normal check now runs the
installed SvelteKit package's declared `svelte-kit sync` binary with a bounded
subprocess before semantic diagnostics. A failed preparation reports the actual
sync failure and skips the misleading downstream type errors. No authored
configuration or generic read-only semantic query was changed.

The final CLI SHA-256 is
`f4108a04306832f18f2918b903f860f987e230946ead753108543fa37134afd1`.
The freshly rebuilt executable was copied to
`target/native-workflows-consumers/revenant-final-check.exe` before running normal
`check --project <copy> --json`, so the checkout's executable was not held open.
All three checks exited zero, reported `succeeded=true`, and passed SDK, Rust,
frontend preparation and frontend stages. Each start/completion source fingerprint
matched and reported `sourceChangedDuringBuild=false`.

| Copy | Check duration | Diagnostics | Checked source digest |
| --- | --- | --- | --- |
| FileMedia | 29.175 s | none | `sha256:1d11d9ad0b445dfb5b228a1a406786f7f0ac1ace947e7750ec0af345336ee1a3` |
| Luma | 28.605 s | one informational TS6385 notice: `returnValue` is deprecated; zero errors or warnings | `sha256:adc7d4fea279157531b1e3e5c7e31133252eeb809fae4fded96d927c03662da4` |
| PrimeSieve | 27.263 s | none | `sha256:aad4eb3f2d5be390a14046ae6c679a2789d576a057d04b637ccab53e80f26298` |

Actual reports and stderr logs are the `check-final.json` and `check-final.log`
files under each isolated consumer directory. TypeScript 6.0.3 was ready in all
three; Svelte indexed 3, 6 and 2 components respectively. The focused
`tooling_process` integration suite passed all three tests, covering descendant
cleanup on parent success, on managed-process drop, and failed-command output
preservation with a spaced executable path. Strict Clippy for the CLI library and
that integration target passed. The earlier frontend preparation regressions
passed within the complete 26-test `inspection04` suite.
