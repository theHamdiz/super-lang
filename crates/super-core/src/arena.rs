//! Dense arena storage indexed by [`NodeId`](crate::NodeId) / [`TypeId`](crate::TypeId).

use crate::ids::{NodeId, TypeId};

/// Generic bump-style arena: push-only allocation with O(1) index lookup.
#[derive(Debug, Clone)]
pub struct Arena<T> {
    items: Vec<T>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            items: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[inline]
    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }
}

impl<T> Arena<T> {
    pub fn alloc_node(&mut self, value: T) -> NodeId {
        let index = self.items.len() as u32;
        self.items.push(value);
        NodeId::from_index(index).expect("node arena overflow")
    }

    pub fn alloc_type(&mut self, value: T) -> TypeId {
        let index = self.items.len() as u32;
        self.items.push(value);
        TypeId::from_index(index).expect("type arena overflow")
    }

    #[inline]
    pub fn get_node(&self, id: NodeId) -> &T {
        &self.items[id.index() as usize]
    }

    #[inline]
    pub fn get_node_mut(&mut self, id: NodeId) -> &mut T {
        &mut self.items[id.index() as usize]
    }

    #[inline]
    pub fn get_type(&self, id: TypeId) -> &T {
        &self.items[id.index() as usize]
    }

    #[inline]
    pub fn get_type_mut(&mut self, id: TypeId) -> &mut T {
        &mut self.items[id.index() as usize]
    }
}

/// Convenience alias for AST node storage.
pub type NodeStore<T> = Arena<T>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_lookup() {
        let mut arena = Arena::new();
        let a = arena.alloc_node(10u32);
        let b = arena.alloc_node(20u32);
        assert_eq!(*arena.get_node(a), 10);
        assert_eq!(*arena.get_node(b), 20);
        *arena.get_node_mut(a) = 11;
        assert_eq!(*arena.get_node(a), 11);
    }

    #[test]
    fn type_ids_independent_of_node_ids() {
        let mut types = Arena::new();
        let t0 = types.alloc_type("i64");
        let t1 = types.alloc_type("f64");
        assert_eq!(*types.get_type(t0), "i64");
        assert_eq!(*types.get_type(t1), "f64");
    }
}
