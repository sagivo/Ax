//! Effect rows. Sets with a canonical total order for hashing.

use crate::intern::{Interner, Symbol};
use crate::types::Type;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Canonical order (spec §4.1): abort, alloc, diverge, err, io, nondet, race, susp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EffectTag {
    Abort,
    Alloc,
    Diverge,
    Err,
    Io,
    Nondet,
    Race,
    Susp,
}

impl EffectTag {
    pub fn as_str(self) -> &'static str {
        match self {
            EffectTag::Abort => "abort",
            EffectTag::Alloc => "alloc",
            EffectTag::Diverge => "diverge",
            EffectTag::Err => "err",
            EffectTag::Io => "io",
            EffectTag::Nondet => "nondet",
            EffectTag::Race => "race",
            EffectTag::Susp => "susp",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EffectAtom {
    Abort,
    Alloc(Symbol),
    Diverge,
    Err(Type),
    Io(Symbol),
    Nondet,
    Race,
    Susp,
    /// Effect variable (higher-order polymorphism).
    Var(Symbol),
}

impl EffectAtom {
    pub fn tag(&self) -> EffectTag {
        match self {
            EffectAtom::Abort => EffectTag::Abort,
            EffectAtom::Alloc(_) => EffectTag::Alloc,
            EffectAtom::Diverge => EffectTag::Diverge,
            EffectAtom::Err(_) => EffectTag::Err,
            EffectAtom::Io(_) => EffectTag::Io,
            EffectAtom::Nondet => EffectTag::Nondet,
            EffectAtom::Race => EffectTag::Race,
            EffectAtom::Susp => EffectTag::Susp,
            EffectAtom::Var(_) => EffectTag::Susp, // vars sort last-ish
        }
    }

    pub fn display(&self, intern: &Interner) -> String {
        self.display_surface(intern, false)
    }

    pub fn display_tree(&self, intern: &Interner) -> String {
        self.display_surface(intern, true)
    }

    pub fn display_surface(&self, intern: &Interner, tree: bool) -> String {
        match self {
            EffectAtom::Abort => "abort".into(),
            EffectAtom::Alloc(s) => {
                if tree {
                    format!("(alloc {})", intern.get(*s))
                } else {
                    format!("alloc[{}]", intern.get(*s))
                }
            }
            EffectAtom::Diverge => "diverge".into(),
            EffectAtom::Err(t) => {
                if tree {
                    format!("(err {})", t.display_tree(intern))
                } else {
                    format!("err[{}]", t.display(intern))
                }
            }
            EffectAtom::Io(s) => {
                if tree {
                    format!("(io {})", intern.get(*s))
                } else {
                    format!("io[{}]", intern.get(*s))
                }
            }
            EffectAtom::Nondet => "nondet".into(),
            EffectAtom::Race => "race".into(),
            EffectAtom::Susp => "susp".into(),
            EffectAtom::Var(s) => intern.get(*s).to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct EffectSet {
    pub atoms: Vec<EffectAtom>,
}

impl EffectSet {
    pub fn new() -> Self {
        Self { atoms: Vec::new() }
    }

    pub fn empty() -> Self {
        Self::new()
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    pub fn insert(&mut self, atom: EffectAtom) {
        if !self.atoms.iter().any(|a| a == &atom) {
            self.atoms.push(atom);
            self.canonicalize();
        }
    }

    pub fn union(&self, other: &EffectSet) -> EffectSet {
        let mut out = self.clone();
        for a in &other.atoms {
            out.insert(a.clone());
        }
        out
    }

    pub fn remove_err(&self) -> (EffectSet, Option<Type>) {
        let mut err = None;
        let mut atoms = Vec::new();
        for a in &self.atoms {
            match a {
                EffectAtom::Err(t) => err = Some(t.clone()),
                other => atoms.push(other.clone()),
            }
        }
        (EffectSet { atoms }, err)
    }

    pub fn err_type(&self) -> Option<&Type> {
        self.atoms.iter().find_map(|a| match a {
            EffectAtom::Err(t) => Some(t),
            _ => None,
        })
    }

    pub fn has(&self, tag: EffectTag) -> bool {
        self.atoms.iter().any(|a| a.tag() == tag)
    }

    pub fn has_io(&self) -> bool {
        self.has(EffectTag::Io)
    }

    pub fn has_race(&self) -> bool {
        self.has(EffectTag::Race)
    }

    pub fn has_nondet(&self) -> bool {
        self.has(EffectTag::Nondet)
    }

    pub fn canonicalize(&mut self) {
        self.atoms.sort_by(|a, b| a.tag().cmp(&b.tag()));
        self.atoms.dedup();
    }

    pub fn display(&self, intern: &Interner) -> String {
        self.display_surface(intern, false)
    }

    pub fn display_tree(&self, intern: &Interner) -> String {
        self.display_surface(intern, true)
    }

    pub fn display_surface(&self, intern: &Interner, tree: bool) -> String {
        if self.atoms.is_empty() {
            return if tree { "(!)".into() } else { String::new() };
        }
        if tree {
            let inner: Vec<_> = self.atoms.iter().map(|a| a.display_tree(intern)).collect();
            format!("(! {})", inner.join(" "))
        } else {
            let inner: Vec<_> = self.atoms.iter().map(|a| a.display(intern)).collect();
            format!("!{{{}}}", inner.join(", "))
        }
    }

    pub fn subset_of(&self, other: &EffectSet) -> bool {
        self.atoms
            .iter()
            .all(|a| other.atoms.iter().any(|b| a == b))
    }

    /// At most one err[E] per concrete row.
    pub fn err_count(&self) -> usize {
        self.atoms
            .iter()
            .filter(|a| matches!(a, EffectAtom::Err(_)))
            .count()
    }
}

impl fmt::Display for EffectSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "!{{{} effects}}", self.atoms.len())
    }
}
