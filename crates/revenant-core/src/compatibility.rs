//! Shared native compatibility identities, independent of package release versions.

/// Compiled operation-manifest revision for native resources, typed updates and surfaces.
pub const CONTRACT_MANIFEST_VERSION: u32 = 6;
/// Native bridge protocol revision shared by the desktop host and generated client.
pub const BRIDGE_PROTOCOL_VERSION: u32 = 5;
/// SDK/client source compatibility revision for the public authoring capability set.
pub const SDK_COMPATIBILITY_REVISION: u32 = 4;
