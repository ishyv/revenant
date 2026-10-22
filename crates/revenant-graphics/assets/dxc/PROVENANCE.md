# Bundled Windows shader compiler

Revenant's optional Windows GPU feature embeds Microsoft's unmodified x64
`dxcompiler.dll` and `dxil.dll` from the immutable
[DirectX Shader Compiler v1.9.2609 release](https://github.com/microsoft/DirectXShaderCompiler/releases/tag/v1.9.2609).
The release was published on September 29, 2026.

Source archive: `dxc_2026_09_29.zip`.

SHA-256: `ad31b1fc8443175d204f77a611fdb3ef2ec42759bdc2f1167368de24a4a7e7f1`.

The build verifies this digest before opening the archive, extracts only the two
named x64 DLLs, bounds each entry, and verifies each DLL's digest. The compiler is
loaded from an absolute path in a private, uniquely created temporary directory.
That directory belongs to the renderer and is removed after the GPU objects are
destroyed. Applications need no global shader compiler, ATL component, Node, or
Cargo to run the packaged executable.

The DLLs import only Windows system libraries. The official archive's
`LICENCE-MIT.txt`, `LICENSE-LLVM.txt`, and `LICENSE-MS.txt` are preserved alongside
this notice and embedded with the compiler. These are upstream licenses, separate
from Revenant's own MIT license.

The archive is downloaded only for Windows x64 builds with `gpu` enabled; normal
Revenant applications do not fetch or load it. A validated archive in the Cargo
build output is reused. For offline builds, set `REVENANT_DXC_ARCHIVE` to a local
copy of the exact archive above. Its checksum is still enforced; this setting
cannot select arbitrary compiler binaries.

Keep the compiler bundle alive after all GPU objects that may use it. Explicitly
select `Dx12Compiler::DynamicDxc`; `Auto` would allow an unrelated PATH compiler
or FXC fallback and would weaken the bundled-compiler contract.
