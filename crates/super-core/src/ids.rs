//! Arena-indexed handles used throughout the Super frontend and runtime.

use core::fmt;
use core::num::NonZeroU32;

/// Index into the AST node arena.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct NodeId(NonZeroU32);

impl NodeId {
    #[inline]
    pub const fn from_index(index: u32) -> Option<Self> {
        match NonZeroU32::new(index.wrapping_add(1)) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    #[inline]
    pub const fn index(self) -> u32 {
        self.0.get().wrapping_sub(1)
    }

    #[inline]
    pub const fn as_u32(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({})", self.index())
    }
}

/// Index into the type table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct TypeId(NonZeroU32);

impl TypeId {
    #[inline]
    pub const fn from_index(index: u32) -> Option<Self> {
        match NonZeroU32::new(index.wrapping_add(1)) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    #[inline]
    pub const fn index(self) -> u32 {
        self.0.get().wrapping_sub(1)
    }

    #[inline]
    pub const fn as_u32(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Debug for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TypeId({})", self.index())
    }
}

/// Deduplicated string / symbol identifier from the interner.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct SymId(NonZeroU32);

impl SymId {
    #[inline]
    pub const fn from_index(index: u32) -> Option<Self> {
        match NonZeroU32::new(index.wrapping_add(1)) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    #[inline]
    pub const fn index(self) -> u32 {
        self.0.get().wrapping_sub(1)
    }

    #[inline]
    pub const fn as_u32(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Debug for SymId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SymId({})", self.index())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_roundtrip() {
        let id = NodeId::from_index(0).unwrap();
        assert_eq!(id.index(), 0);
        let id = NodeId::from_index(42).unwrap();
        assert_eq!(id.index(), 42);
    }

    #[test]
    fn type_and_sym_ids_distinct_debug() {
        let t = TypeId::from_index(1).unwrap();
        let s = SymId::from_index(1).unwrap();
        assert_ne!(format!("{t:?}"), format!("{s:?}"));
    }
}
