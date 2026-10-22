# Non-Windows desktop smoke

Windows remains the primary release and interactive acceptance target.
The **Non-Windows desktop smoke** workflow declares Linux WebKitGTK 4.1,
compiler/OpenSSL/AppIndicator/SVG prerequisites and checks macOS Xcode command
line tools. It runs Rust and client checks, documentation, native/client tests,
and owned-process shutdown tests.

Each job creates and packages a fresh scaffold outside the checkout under a
path containing spaces and non-ASCII characters. Linux checks DEB output;
macOS checks application and DMG output. Each bundle contains SHA256SUMS and
source-provenance.json, recording the actual checked-out source commit and
artifact hashes. The workflow checks out the pull request head explicitly
and rejects a different or dirty source state.

These jobs provide build and headless lifecycle smoke evidence. They do not
establish interactive Linux/macOS behavior, signing, notarization, or clean
machine acceptance. Record actual run URLs and source commits in VALIDATION.md
after publication; configured jobs alone are not verified platform evidence.
