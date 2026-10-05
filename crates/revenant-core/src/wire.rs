//! Wire handles preserve ownership identity. Decimal wrappers encode 64-bit values as canonical base-10 strings, rejecting leading zeros, plus signs, and negative zero.
use crate::{Error, Result, WireSafe};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ts_rs::TS;

macro_rules! decimal {
    ($name:ident, $inner:ty, $pattern:literal) => {
        /// A 64-bit integer represented as a canonical decimal string on the wire.
        /// Defaults to zero. Deserialization rejects noncanonical spelling and
        /// values outside the backing integer range; JSON numbers are not accepted.
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
        #[ts(type = "string")]
        pub struct $name(
            /// Native integer value; serialization always emits its canonical decimal string.
            pub $inner
        );
        impl $name {
            /// Returns the native integer without parsing or changing its unit.
            pub fn get(self) -> $inner { self.0 }
        }
        impl From<$inner> for $name { fn from(value: $inner) -> Self { Self(value) } }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                let text = String::deserialize(d)?;
                let value: $inner = text.parse().map_err(serde::de::Error::custom)?;
                if value.to_string() != text {
                    return Err(serde::de::Error::custom("expected a canonical decimal string"));
                }
                Ok(Self(value))
            }
        }
        impl JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> { stringify!($name).into() }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                schemars::json_schema!({"type": "string", "pattern": $pattern})
            }
        }
        impl WireSafe for $name {}
    };
}
decimal!(DecimalU64, u64, "^(0|[1-9][0-9]*)$");
decimal!(DecimalI64, i64, "^(0|-?[1-9][0-9]*)$");

/// A scoped capability identity; possession of JSON alone does not authorize access.
/// Registry admission compares all four fields against a live owner entry.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Handle {
    /// Opaque registry-issued resource identifier; never reused within its registry.
    pub id: String,
    /// Original owning scope identifier; handles cannot be adopted across scopes.
    pub scope: String,
    /// Identity generation compared at admission; native file registration uses one.
    pub generation: u32,
    /// Resource discriminator; file admission requires `file`.
    pub kind: String,
}
impl WireSafe for Handle {
    fn visit_handles(&self, visitor: &mut dyn FnMut(&Handle) -> Result<()>) -> Result<()> {
        visitor(self)
    }
}

/// Enumerated file metadata paired with its scoped capability handle.
/// Paths are display metadata, not native filesystem authority.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileEntry {
    /// Capability to admit before reading; metadata alone grants no access.
    pub handle: Handle,
    /// Display file name supplied by the host.
    pub name: String,
    /// Host-supplied path relative to its enumeration root; not an absolute native path.
    pub relative_path: String,
    /// Captured file length in bytes, encoded as a decimal u64 string.
    pub size: DecimalU64,
    /// Host-supplied media type string; the core does not infer or validate its contents.
    pub mime: String,
    /// Optional host-supplied modification timestamp; absent when unavailable.
    /// The host defines its epoch and unit; this core stores the value unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub modified: Option<DecimalI64>,
}
impl WireSafe for FileEntry {
    fn visit_handles(&self, visitor: &mut dyn FnMut(&Handle) -> Result<()>) -> Result<()> {
        visitor(&self.handle)
    }
}

/// Host-supplied file description captured at registration.
/// Size bounds lease reads; names and paths do not select the native resource.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileMetadata {
    /// Display file name supplied by the host.
    pub name: String,
    /// Host-supplied path relative to its enumeration root; not an absolute native path.
    pub relative_path: String,
    /// Captured file length in bytes, encoded as a decimal u64 string.
    pub size: DecimalU64,
    /// Host-supplied media type string; the core does not infer or validate its contents.
    pub mime: String,
    /// Optional host-supplied modification timestamp; absent when unavailable.
    /// The host defines its epoch and unit; this core stores the value unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub modified: Option<DecimalI64>,
}
impl WireSafe for FileMetadata {}
impl FileMetadata {
    /// Pairs this metadata with a handle without registering or authorizing it.
    /// Returns `wrong_kind` unless the handle kind is `file`.
    pub fn entry(self, handle: Handle) -> Result<FileEntry> {
        if handle.kind != "file" {
            return Err(Error::new("wrong_kind", "Expected a file handle"));
        }
        Ok(FileEntry {
            handle,
            name: self.name,
            relative_path: self.relative_path,
            size: self.size,
            mime: self.mime,
            modified: self.modified,
        })
    }
}
