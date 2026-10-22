# Disposable offline Windows acceptance

Use a new disposable Hyper-V guest, never the development host, for runtime
absence/removal checks. Obtain evaluation media from Microsoft and verify its
SHA256 against Microsoft's authenticity information before creating the VM.

## Prepare the fixture

Run `scripts/New-AcceptanceVM.ps1` from elevated host PowerShell with an explicit
ISO path and verified digest. Inspect its parameters first. It refuses existing
VM names/directories and creates no attached virtual switch by default. Keep
an explicit identity and recovery checkpoint; VM creation is not app acceptance.
Install Windows through the guest console and disconnect networking before
recording the offline baseline. Never generalize guest cleanup to the host.

Build the Windows runtime probe with a static CRT before transferring it:

```powershell
cargo rustc -p revenant-desktop --bin webview_baseline --features tauri-host --release --locked -- -C target-feature=+crt-static
$probe = & .\target\release\webview_baseline.exe | ConvertFrom-Json
if ($probe.unsupported -or $probe.available -isnot [bool]) { throw 'Invalid runtime probe build' }
```

A no-default-feature stub cannot establish WebView2 availability. A host probe
cannot establish guest state. Record the binary digest, inspect imports, and
execute it in the guest rather than inferring compatibility from compilation.

Transfer the verified probe and `scripts/Get-AcceptanceGuestBaseline.ps1` through
guest services or an evidence disk. The read-only script checks Hyper-V identity,
OS, probe hash, WebView2 availability, development tools and network state.
An installed runtime, present development tools or active networking fails the
runtime-absence baseline. Preserve existing-runtime smoke evidence separately.
If the evaluation OS includes a protected runtime, use a reviewed guest-only
procedure and recovery checkpoint; do not weaken the host to construct a test.

## Install and exercise the actual artifact

After the appropriate baseline passes, transfer the source-verified NSIS artifact,
`source-provenance.json` and `SHA256SUMS`. Compare hashes with the exact workflow
artifact before installation and keep networking disconnected.

Record installation context (interactive standard-user versus remote session),
installer exit status, runtime probe results and installed executable identity.
Wait on the installer process handle when measuring its exit: a persistent updater
descendant can make whole-process-tree waiting misleading. An observed installed
runtime does not establish an unrecorded installer exit code.

Exercise startup, folder access, text/image/audio previews, custom operations,
bounded results, cancellation, settings persistence, reopen, permission/path
errors, and close with active work. For image notes also exercise failed saves,
cache clearing/regeneration and dirty-note Save/Discard/Cancel. Cancellation must
preserve the draft/window; Save must persist; Discard must restore the prior saved
state on reopening. Check resource cleanup after completion and route disposal.
Record actual results, screenshots, source commit and installer hash. Prepared
scripts and VM metadata are not acceptance proof.

## Editor acceptance

Create minimal and custom-native projects outside the framework checkout. Open
each root in actual Zed. Check `Application::new` in the generated bootstrap;
for custom native code also inspect operation macros, `TaskContext` and contract
fields. Inspect generated TypeScript operation documentation and follow its Rust
source link into the installed SDK or authored native crate. Record screenshots
identifying the project and source state. Semantic/LSP checks supplement, but do
not replace, visible hover/source-navigation acceptance.

Local VM identities, account details, recovery transcripts and transfer locations
are operator records rather than portable acceptance instructions. See
[VALIDATION.md](VALIDATION.md) for the checkpoint's actual evidence boundaries.
