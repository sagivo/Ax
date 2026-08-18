//! Interned identifiers. Every name in the compiler is a 32-bit handle.

use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(pub u32);

impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sym({})", self.0)
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Interner {
    map: HashMap<String, Symbol>,
    strs: Vec<String>,
}

impl Interner {
    pub fn new() -> Self {
        let mut i = Self {
            map: HashMap::new(),
            strs: Vec::new(),
        };
        // Reserve 0 as the empty / missing name.
        i.intern("");
        i
    }

    pub fn intern(&mut self, s: &str) -> Symbol {
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let sym = Symbol(self.strs.len() as u32);
        self.strs.push(s.to_string());
        self.map.insert(s.to_string(), sym);
        sym
    }

    /// Symbol for an already-interned name, without interning it. Read-only
    /// passes (lowering) need to name prelude types without a mutable interner.
    pub fn lookup(&self, s: &str) -> Option<Symbol> {
        self.map.get(s).copied()
    }

    #[inline]
    pub fn get(&self, s: Symbol) -> &str {
        &self.strs[s.0 as usize]
    }

    pub fn is_empty_name(&self, s: Symbol) -> bool {
        s.0 == 0
    }

    pub fn len(&self) -> usize {
        self.strs.len()
    }
}
