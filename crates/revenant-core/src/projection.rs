//! Opt-in portable model projections. Domain values need no serialization traits.
use crate::Contract;

/// Converts an owned source model to its lossless generated boundary model.
/// Portable engines can enable the derive through an optional integration feature.
pub trait WireProjection: Sized {
    /// Generated model providing ordinary Serde, schema, TypeScript and handle visitation.
    type Wire: Contract;
    /// Moves this domain value into its boundary representation without lossy casts.
    fn into_wire(self) -> Self::Wire;
    /// Restores domain values from a validated boundary model.
    fn from_wire(wire: Self::Wire) -> Self;
}
macro_rules! identity {
    ($($ty:ty),*) => { $(impl WireProjection for $ty {
        type Wire = Self;
        fn into_wire(self) -> Self { self }
        fn from_wire(wire: Self) -> Self { wire }
    })* };
}
identity!(
    (),
    bool,
    String,
    char,
    i8,
    i16,
    i32,
    u8,
    u16,
    u32,
    f32,
    f64,
    crate::DecimalU64,
    crate::DecimalI64,
    crate::Handle,
    crate::FileEntry,
    crate::FileMetadata
);
impl<T: WireProjection> WireProjection for Option<T> {
    type Wire = Option<T::Wire>;
    fn into_wire(self) -> Self::Wire {
        self.map(T::into_wire)
    }
    fn from_wire(wire: Self::Wire) -> Self {
        wire.map(T::from_wire)
    }
}
impl<T: WireProjection> WireProjection for Vec<T> {
    type Wire = Vec<T::Wire>;
    fn into_wire(self) -> Self::Wire {
        self.into_iter().map(T::into_wire).collect()
    }
    fn from_wire(wire: Self::Wire) -> Self {
        wire.into_iter().map(T::from_wire).collect()
    }
}
impl<T: WireProjection> WireProjection for Box<T> {
    type Wire = Box<T::Wire>;
    fn into_wire(self) -> Self::Wire {
        Box::new((*self).into_wire())
    }
    fn from_wire(wire: Self::Wire) -> Self {
        Box::new(T::from_wire(*wire))
    }
}
impl<T: 'static> WireProjection for crate::Resource<T> {
    type Wire = Self;
    fn into_wire(self) -> Self {
        self
    }
    fn from_wire(wire: Self) -> Self {
        wire
    }
}
