# Root CLI/tooling validation

These tests own configuration, scaffolding, portable SDK materialization,
compiled-contract publication, managed CLI process trees and native source
watching. Native dispatch, core/SDK contracts, client ACK semantics and desktop
UI/packaging acceptance belong to their separate owners' suites.

After main releases the shared Cargo slot:

```powershell
cargo check -p revenant --all-targets --locked --target-dir target/desktop-check
cargo test -p revenant --locked --target-dir target/desktop-check
```

The default root suite uses disposable directories. Publication and process
tests compile a tiny SDK-independent fixture with `rustc`; they do not build a
second native application. Process tests verify a descendant's owned socket
closes on both parent exit and owner drop, exercising Windows JobObject cleanup.
Watcher tests use real filesystem notifications for native source, local path
dependencies and SDK source, and ensure compiler outputs do not cause rebuilds.

The real authoring proof is explicitly slow and runs serially:

```powershell
cargo test -p revenant --locked --target-dir target/desktop-check --test desktop_cli_workflows -- --ignored --nocapture --test-threads=1
```

It invokes the actual `revenant new` CLI outside the repository, checks compiled
built-in exports, adds the file-backed custom native fixture, regenerates its
real documented contracts, runs frontend type checks and a static SPA build,
then introduces a Rust compile failure. The previous facade, immutable
generation and SPA must survive byte-for-byte; repaired source must regenerate
the original contract. It requires desktop build prerequisites and network or
cached Cargo/npm dependencies. It does not open a window or validate installers.

The standard-library Python wrapper can run both phases:

```powershell
python tests/integration_runner.py --ready --standalone
```

`--ready` records explicit Cargo-slot authorization; scripts do not acquire a
global build slot themselves. An optional `--cli` on `standalone_workflows.py`
selects an existing binary; otherwise Cargo supplies the newly built root CLI.

The real dev/rebuild workflow opens an owned disposable native window and
requires a separate explicit flag:

```powershell
python tests/watch_workflows.py --ready --allow-desktop-window --cli target/desktop-check/debug/revenant.exe
```

It checks compile-failure preservation, trailing edits during native builds,
local SDK watching, and desktop/Vite cleanup. It uses a temporary project and
an OS-selected dev port. Root tests never shut down the machine.

The former HTTP upload/server and browser-worker workflows were removed because
their transport and authoring model are unsupported in version 0.3. Historical
validation documents describe the old commands; they are not active tests.
