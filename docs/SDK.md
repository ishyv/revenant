# SDK checks, upgrades and recovery

Application snapshots are checked against SDK compatibility revision **4**,
compiled-manifest version **6**, and native protocol **5**. Framework and client
package versions are **0.4.0**. The revision is recorded independently in
`.revenant/sdk/compatibility.json` and the client's `revenant.compatibility`
package metadata.

Run these commands from an application directory:

```sh
revenant sdk check
revenant sdk upgrade
revenant sdk restore .revenant/sdk-backups/<backup>
```

`check` is read-only. It checks SDK sources and the installed frontend client
separately, including the declared package entry points and their required
exports. A package with version 0.4.0 but without `setupApp` or `callableOperation`
is incompatible.
Compatible source edits are allowed; compatibility checks do not require the
source bytes to match the CLI's embedded snapshot. Compilation still checks the
native implementation and frontend types.

Development dependency URLs include an identity derived from the installed
client's source bytes. This prevents a persistent WebView from reusing old
optimized modules after an SDK replacement with the same package version.
`revenant dev` adds this integration through a generated Vite configuration wrapper
and preserves the authored `web/vite.config.js`. New scaffolds also include
`revenantSdk()` from `@revenant/client/vite` for direct frontend development.
No manual browser cache clearing is required after a normal upgrade and restart.

Revision 4 adds grouped reads, acknowledged native overlays, declared work
classes, bounded CPU helpers, portable projections, and experimental compute
to the revision 3 capabilities. Experimental compute remains opt-in and has
not demonstrated a general speed advantage; see [native workflows](native-workflows.md).

Revision 3 introduced typed native resources, declared task update contracts, and
the optional native graphics capability. The snapshot contains the
`revenant-graphics` source crate, GPU shader, bundled font, and font license.
Windows x64 GPU builds embed the pinned Microsoft DXC compiler and its license
notices. The first GPU build downloads and verifies an immutable upstream
archive; subsequent builds reuse its verified cache. For offline builds,
`REVENANT_DXC_ARCHIVE` can point to that exact archive, with the same checksum
enforced. Packaged applications load their embedded compiler from a private
absolute path and require no ATL installation or global DXC. See the graphics
crate's `assets/dxc/PROVENANCE.md` for the pinned release and checksums.
Ordinary apps retain the default features without GPU rendering. Older revision
2 snapshots require the normal SDK upgrade; restoring their backup preserves the
older source bytes, but current compatibility checks still reject that snapshot.

Host preparation automatically materializes an SDK only when the whole SDK
directory is absent. It rejects partial or incompatible snapshots before writing
host files, and rejects a stale installed client even when SDK sources are valid.
Contract publication requires both checks and preserves the last successful
facade when either check fails.

Stop development processes before upgrading or restoring. SDK operations use an
operating-system file lease to exclude other upgrades and restores. An upgrade
stages and checks the replacement, then archives the old SDK, generated host,
dependency installation, frontend lockfiles, native lockfile and published
contract generations. The SDK workspace lockfile is preserved inside the archived
SDK; the host lockfile is preserved inside the archived host.

The replacement installs dependencies at their normal application paths, resolves
a new frontend lockfile from the **unchanged handwritten package configuration**,
regenerates an existing native library's independent Cargo lockfile, compiles the
native application and publishes its validated contract. Installation,
compilation or publication failures restore the archived paths. No native source,
Svelte configuration, frontend source, application manifest or package declaration
is migrated by this command. Frontend migration is a separate authoring change.

When migrating a 0.3 application, update its handwritten native SDK dependency
to 0.4 before upgrading. Operations retaining the old single-DTO input shape
must explicitly declare `legacy_input`; the default operation API generates named
arguments. Native test fixtures must use the identity returned by
`ResourceRegistry::create_scope(None)` rather than inventing a scope string.
Generated facade imports use actual `.ts` paths: older handwritten frontend
configurations need `allowImportingTsExtensions: true` and `noEmit: true`.
After upgrade, run the application's native tests, frontend check and build.
Successful contract export validates the native library and contracts, not its
test-only code or every frontend configuration.

Ordinary locked npm installations use `npm ci --install-links`. Fresh applications
use `npm install --install-links` once to create their application lockfile. Keep
that lockfile in source control for subsequent installs.

Successful upgrades print their backup directory. A restore swaps that backup's
paths into the application and saves the displaced state in a new inverse backup,
whose path is printed for redo. The selected backup is consumed rather than
duplicating potentially large dependency trees. Backups are restricted to the
owning application's `.revenant/sdk-backups` directory. An interrupted replacement
uses its journal to recover in place; failures retain the backup and report the
recovery path. Restore records link the selected and inverse backups and record
each transfer before its rename. Recovery returns transferred old paths to the
selected backup before restoring the displaced installation, preserving both
states after a forced exit. The inverse backup can recover an interrupted restore;
retrying the original selection then applies the preserved old state. Never delete
a backup until recovery and application validation are complete.
