//! Deduplicating string interner producing [`SymId`](crate::SymId) handles.

use crate::ids::SymId;
use std::collections::HashMap;

/// Interns strings once; subsequent lookups reuse the same [`SymId`].
#[derive(Debug, Default)]
pub struct Interner {
    map: HashMap<String, SymId>,
    strings: Vec<String>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            map: HashMap::with_capacity(cap),
            strings: Vec::with_capacity(cap),
        }
    }

    pub fn intern(&mut self, s: &str) -> SymId {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let index = self.strings.len() as u32;
        let id = SymId::from_index(index).expect("interner overflow");
        self.strings.push(s.to_owned());
        self.map.insert(s.to_owned(), id);
        id
    }

    pub fn intern_owned(&mut self, s: String) -> SymId {
        if let Some(&id) = self.map.get(&s) {
            return id;
        }
        let index = self.strings.len() as u32;
        let id = SymId::from_index(index).expect("interner overflow");
        self.map.insert(s.clone(), id);
        self.strings.push(s);
        id
    }

    #[inline]
    pub fn resolve(&self, id: SymId) -> &str {
        &self.strings[id.index() as usize]
    }

    /// Lookup without inserting. Returns `None` if the string was never interned.
    #[inline]
    pub fn lookup(&self, s: &str) -> Option<SymId> {
        self.map.get(s).copied()
    }

    /// Lookup requiring prior intern (lex/sema).
    pub fn must_lookup(&self, s: &str) -> SymId {
        self.lookup(s).unwrap_or_else(|| {
            panic!("symbol `{s}` was not interned; intern during lex/sema first")
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deduplicates() {
        let mut i = Interner::new();
        let a = i.intern("host");
        let b = i.intern("host");
        let c = i.intern("port");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(i.resolve(a), "host");
        assert_eq!(i.len(), 2);
    }
}
