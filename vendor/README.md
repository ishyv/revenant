# HyvUI snapshot

`hyvui-1.0.0.tgz` is the public package built from the current local HyvUI source on October 1, 2026. It includes the working-tree changes present in `C:/T/Projects/hyvui`, rather than the older 0.6.1 npm release or stale built output. The sibling checkout was read only.

The snapshot was built in `.tooling/hyvui` with the package's `prepack` script, including Svelte packaging, public-boundary preparation and publint. Private experiments are excluded by the upstream preparation script. Font licenses are included in the archive.

SHA-256: `1778fabf06982495db7695918ba6bcfc18e88438a8ef2a0b4961e8c9e88b82eb`.

This package supplies replaceable UI defaults. The capability runtime imports no UI components. Applications can use their own presentation and CSS without replacing the runtime.
