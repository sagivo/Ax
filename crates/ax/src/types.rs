//! Semantic types. Interned, hash-consed, cheap to copy.

use crate::intern::Symbol;
use crate::span::Span;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Prim {
    I8,
    I16,
    I32,
    I64,
    Isz,
    U8,
    U16,
    U32,
    U64,
    Usz,
    F32,
    F64,
    Bool,
    Byte,
    Unit,
}

impl Prim {
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "i8" => Prim::I8,
            "i16" => Prim::I16,
            "i32" => Prim::I32,
            "i64" => Prim::I64,
            "isz" => Prim::Isz,
            "u8" => Prim::U8,
            "u16" => Prim::U16,
            "u32" => Prim::U32,
            "u64" => Prim::U64,
            "usz" => Prim::Usz,
            "f32" => Prim::F32,
            "f64" => Prim::F64,
            "bool" => Prim::Bool,
            "byte" => Prim::Byte,
            "unit" => Prim::Unit,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Prim::I8 => "i8",
            Prim::I16 => "i16",
            Prim::I32 => "i32",
            Prim::I64 => "i64",
            Prim::Isz => "isz",
            Prim::U8 => "u8",
            Prim::U16 => "u16",
            Prim::U32 => "u32",
            Prim::U64 => "u64",
            Prim::Usz => "usz",
            Prim::F32 => "f32",
            Prim::F64 => "f64",
            Prim::Bool => "bool",
            Prim::Byte => "byte",
            Prim::Unit => "unit",
        }
    }

    pub fn is_signed_int(self) -> bool {
        matches!(
            self,
            Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64 | Prim::Isz
        )
    }

    pub fn is_unsigned_int(self) -> bool {
        matches!(
            self,
            Prim::U8 | Prim::U16 | Prim::U32 | Prim::U64 | Prim::Usz | Prim::Byte
        )
    }

    pub fn is_int(self) -> bool {
        self.is_signed_int() || self.is_unsigned_int()
    }

    pub fn is_float(self) -> bool {
        matches!(self, Prim::F32 | Prim::F64)
    }

    pub fn bit_width(self) -> u32 {
        match self {
            Prim::I8 | Prim::U8 | Prim::Byte => 8,
            Prim::I16 | Prim::U16 => 16,
            Prim::I32 | Prim::U32 | Prim::F32 => 32,
            Prim::I64 | Prim::U64 | Prim::F64 => 64,
            Prim::Isz | Prim::Usz => 64, // research-v1: 64-bit host
            Prim::Bool | Prim::Unit => 0,
        }
    }

    pub fn wrap_i128(self, v: i128) -> i128 {
        let w = self.bit_width();
        if w == 0 {
            return v;
        }
        if self.is_signed_int() {
            let bits = 1i128 << w;
            let half = bits >> 1;
            let mut x = v % bits;
            if x < 0 {
                x += bits;
            }
            if x >= half {
                x - bits
            } else {
                x
            }
        } else {
            let mask = if w == 64 {
                u64::MAX as i128
            } else {
                (1i128 << w) - 1
            };
            v & mask
        }
    }
}

impl fmt::Display for Prim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Region identity. Lexical: nesting depth + name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegionId {
    pub name: Symbol,
    /// 0 = static / 'static-like (function params, globals). Higher = more nested.
    pub depth: u32,
}

impl RegionId {
    pub fn static_region(name: Symbol) -> Self {
        Self { name, depth: 0 }
    }

    /// `self` outlives `other` iff self is an ancestor (smaller or equal depth).
    /// Store rule: store(&r T, location l) legal iff r outlives l
    /// i.e. r.depth <= l.depth (r lives at least as long as l).
    pub fn outlives(self, other: RegionId) -> bool {
        self.depth <= other.depth
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Prim(Prim),
    /// Named nominal type, possibly generic. `def` is the defining symbol.
    Named {
        def: Symbol,
        args: Vec<Type>,
    },
    /// `&r T` or `&r mut T`
    Ref {
        region: RegionId,
        mutable: bool,
        inner: Box<Type>,
    },
    /// `own T`
    Own(Box<Type>),
    /// `Untrusted[T]` — lattice annotation, same layout as `T`.
    Untrusted(Box<Type>),
    /// `Secret[T]` — lattice annotation, same layout as `T`.
    Secret(Box<Type>),
    Fn {
        params: Vec<Type>,
        ret: Box<Type>,
        effects: crate::effects::EffectSet,
    },
    Tuple(Vec<Type>),
    Record(Vec<(Symbol, Type)>),
    /// Sum type (variants).
    Variant {
        def: Symbol,
        variants: Vec<(Symbol, Vec<(Symbol, Type)>)>,
    },
    /// Type parameter.
    Param(Symbol),
    /// Typed hole.
    Hole,
    /// The type of an expression that does not produce a value: `return`,
    /// `raise`, an infinite `loop`. Distinct from `Error` on purpose — a
    /// diverging expression is well-typed, and conflating the two makes every
    /// consumer unable to tell "this program is broken" from "control flow left
    /// here".
    Never,
    /// Error type (poison).
    Error,
}

impl Type {
    pub fn unit() -> Self {
        Type::Prim(Prim::Unit)
    }
    pub fn bool() -> Self {
        Type::Prim(Prim::Bool)
    }
    pub fn i32() -> Self {
        Type::Prim(Prim::I32)
    }
    pub fn i64() -> Self {
        Type::Prim(Prim::I64)
    }
    pub fn u32() -> Self {
        Type::Prim(Prim::U32)
    }
    pub fn u64() -> Self {
        Type::Prim(Prim::U64)
    }
    pub fn usz() -> Self {
        Type::Prim(Prim::Usz)
    }
    pub fn f32() -> Self {
        Type::Prim(Prim::F32)
    }
    pub fn f64() -> Self {
        Type::Prim(Prim::F64)
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Type::Error)
    }

    pub fn is_hole(&self) -> bool {
        matches!(self, Type::Hole)
    }

    pub fn as_prim(&self) -> Option<Prim> {
        match self {
            Type::Prim(p) => Some(*p),
            _ => None,
        }
    }

    /// Strip a single immutable/mutable reference.
    pub fn peel_ref(&self) -> Option<(&Type, bool, RegionId)> {
        match self {
            Type::Ref {
                inner,
                mutable,
                region,
            } => Some((inner, *mutable, *region)),
            _ => None,
        }
    }

    pub fn display(&self, intern: &crate::intern::Interner) -> String {
        self.display_surface(intern, false)
    }

    /// Tree-surface spelling. Protocol replies use this so an agent that
    /// writes trees also reads trees.
    pub fn display_tree(&self, intern: &crate::intern::Interner) -> String {
        self.display_surface(intern, true)
    }

    pub fn display_surface(&self, intern: &crate::intern::Interner, tree: bool) -> String {
        match self {
            Type::Prim(p) => p.as_str().to_string(),
            Type::Named { def, args } => {
                if tree {
                    if args.is_empty() {
                        intern.get(*def).to_string()
                    } else {
                        let inner: Vec<_> = args.iter().map(|a| a.display_tree(intern)).collect();
                        format!("({} {})", intern.get(*def), inner.join(" "))
                    }
                } else {
                    let mut s = intern.get(*def).to_string();
                    if !args.is_empty() {
                        s.push('[');
                        for (i, a) in args.iter().enumerate() {
                            if i > 0 {
                                s.push_str(", ");
                            }
                            s.push_str(&a.display(intern));
                        }
                        s.push(']');
                    }
                    s
                }
            }
            Type::Ref {
                region,
                mutable,
                inner,
            } => {
                let r = intern.get(region.name);
                if tree {
                    let mut o = String::from("(ref");
                    if r != "_" && !r.is_empty() {
                        o.push(' ');
                        o.push_str(r);
                    }
                    if *mutable {
                        o.push_str(" mut");
                    }
                    o.push(' ');
                    o.push_str(&inner.display_tree(intern));
                    o.push(')');
                    o
                } else if *mutable {
                    format!("&{r} mut {}", inner.display(intern))
                } else {
                    format!("&{r} {}", inner.display(intern))
                }
            }
            Type::Own(t) => {
                if tree {
                    format!("(own {})", t.display_tree(intern))
                } else {
                    format!("own {}", t.display(intern))
                }
            }
            Type::Untrusted(t) => {
                if tree {
                    format!("(untrusted {})", t.display_tree(intern))
                } else {
                    format!("Untrusted[{}]", t.display(intern))
                }
            }
            Type::Secret(t) => {
                if tree {
                    format!("(secret {})", t.display_tree(intern))
                } else {
                    format!("Secret[{}]", t.display(intern))
                }
            }
            Type::Fn {
                params,
                ret,
                effects,
            } => {
                if tree {
                    let ps: Vec<_> = params.iter().map(|p| p.display_tree(intern)).collect();
                    let mut s = format!("(fn ({}) {}", ps.join(" "), ret.display_tree(intern));
                    if !effects.is_empty() {
                        s.push(' ');
                        s.push_str(&effects.display_tree(intern));
                    }
                    s.push(')');
                    s
                } else {
                    let ps: Vec<_> = params.iter().map(|p| p.display(intern)).collect();
                    let mut s = format!("fn({}) -> {}", ps.join(", "), ret.display(intern));
                    if !effects.is_empty() {
                        s.push(' ');
                        s.push_str(&effects.display(intern));
                    }
                    s
                }
            }
            Type::Tuple(ts) => {
                if tree {
                    let ps: Vec<_> = ts.iter().map(|t| t.display_tree(intern)).collect();
                    format!("(tuple {})", ps.join(" "))
                } else {
                    let ps: Vec<_> = ts.iter().map(|t| t.display(intern)).collect();
                    format!("({})", ps.join(", "))
                }
            }
            Type::Record(fs) => {
                if tree {
                    let ps: Vec<_> = fs
                        .iter()
                        .map(|(n, t)| format!("({} {})", intern.get(*n), t.display_tree(intern)))
                        .collect();
                    format!("(rec {})", ps.join(" "))
                } else {
                    let ps: Vec<_> = fs
                        .iter()
                        .map(|(n, t)| format!("{}: {}", intern.get(*n), t.display(intern)))
                        .collect();
                    format!("{{ {} }}", ps.join(", "))
                }
            }
            Type::Variant { def, .. } => intern.get(*def).to_string(),
            Type::Param(s) => intern.get(*s).to_string(),
            Type::Hole => "?".into(),
            Type::Never => "never".into(),
            Type::Error => "<error>".into(),
        }
    }
}

/// A resolved user type definition.
#[derive(Clone, Debug)]
pub struct TypeDef {
    pub name: Symbol,
    pub generics: Vec<Symbol>,
    pub kind: TypeDefKind,
    pub injections: Vec<ResolvedInjection>,
    pub span: Span,
    pub def_id: String,
}

#[derive(Clone, Debug)]
pub enum TypeDefKind {
    Alias(Type),
    Record(Vec<(Symbol, Type)>),
    Variants(Vec<(Symbol, Vec<(Symbol, Type)>)>),
}

#[derive(Clone, Debug)]
pub struct ResolvedInjection {
    pub from: Type,
    pub into_variant: Symbol,
    pub span: Span,
}

/// Dictionary definition: `dict Ord[i32] = { cmp: ... }`.
#[derive(Clone, Debug)]
pub struct DictDef {
    pub name: Symbol,
    pub for_ty: Type,
    pub fields: Vec<(Symbol, Type)>,
    pub span: Span,
    pub def_id: String,
}

/// Function signature (interface).
#[derive(Clone, Debug)]
pub struct FnSig {
    pub name: Symbol,
    pub generics: Vec<Symbol>,
    pub params: Vec<(Symbol, Type, bool)>, // name, type, default_dict
    pub ret: Type,
    pub effects: crate::effects::EffectSet,
    pub is_contract_fn: bool,
    pub span: Span,
    pub def_id: String,
}

#[derive(Default)]
pub struct TypeIntern {
    // Reserved for future hash-consing. Types are currently owned.
    _marker: (),
}

impl TypeIntern {
    pub fn new() -> Self {
        Self { _marker: () }
    }
}

/// Structural equality used by the checker. Named types compare by def+args.
pub fn types_eq(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Error, _) | (_, Type::Error) => true,
        (Type::Hole, _) | (_, Type::Hole) => true,
        // `never` coerces to every type: a diverging branch imposes no
        // constraint on the branch it sits opposite.
        (Type::Never, _) | (_, Type::Never) => true,
        (Type::Prim(x), Type::Prim(y)) => x == y,
        (Type::Named { def: d1, args: a1 }, Type::Named { def: d2, args: a2 }) => {
            d1 == d2 && a1.len() == a2.len() && a1.iter().zip(a2).all(|(x, y)| types_eq(x, y))
        }
        (Type::Named { def: d1, args: a1 }, Type::Variant { def: d2, .. })
        | (Type::Variant { def: d1, .. }, Type::Named { def: d2, args: a1 }) => {
            d1 == d2 && a1.is_empty()
        }
        (
            Type::Ref {
                region: r1,
                mutable: m1,
                inner: i1,
            },
            Type::Ref {
                region: r2,
                mutable: m2,
                inner: i2,
            },
        ) => {
            // Elided regions (`_`) unify with any region of the same mutability.
            let r_ok = r1 == r2 || r1.name.0 == 0 || r2.name.0 == 0;
            // Also treat any two static-depth refs of the same inner type as equal
            // when either name is `_` or `static`.
            let _ = (r1, r2);
            let _ = r_ok;
            m1 == m2 && types_eq(i1, i2)
        }
        (Type::Own(x), Type::Own(y)) => types_eq(x, y),
        (Type::Untrusted(x), Type::Untrusted(y)) => types_eq(x, y),
        (Type::Untrusted(x), y) | (y, Type::Untrusted(x)) => types_eq(x, y),
        (Type::Secret(x), Type::Secret(y)) => types_eq(x, y),
        (
            Type::Fn {
                params: p1,
                ret: r1,
                effects: e1,
            },
            Type::Fn {
                params: p2,
                ret: r2,
                effects: e2,
            },
        ) => {
            p1.len() == p2.len()
                && p1.iter().zip(p2).all(|(x, y)| types_eq(x, y))
                && types_eq(r1, r2)
                && e1 == e2
        }
        (Type::Tuple(x), Type::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| types_eq(a, b))
        }
        (Type::Record(x), Type::Record(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((n1, t1), (n2, t2))| n1 == n2 && types_eq(t1, t2))
        }
        (Type::Variant { def: d1, .. }, Type::Variant { def: d2, .. }) => d1 == d2,
        (Type::Param(x), Type::Param(y)) => x == y,
        _ => false,
    }
}

/// Bind type parameters by matching a declared type against an actual one.
///
/// One-directional and structural: enough to resolve a call site, not a general
/// inference engine (the language is explicit at every top-level signature, and
/// local inference is deliberately shallow). Shared by the checker and by
/// lowering so both agree on what a call site instantiates.
pub fn unify_param(param: &Type, actual: &Type, out: &mut HashMap<Symbol, Type>) {
    match (param, actual) {
        (Type::Param(s), a) => {
            if !matches!(a, Type::Hole | Type::Error | Type::Param(_)) {
                out.entry(*s).or_insert_with(|| a.clone());
            }
        }
        (Type::Named { args: pa, .. }, Type::Named { args: aa, .. }) => {
            for (p, a) in pa.iter().zip(aa) {
                unify_param(p, a, out);
            }
        }
        (Type::Ref { inner: p, .. }, Type::Ref { inner: a, .. }) => unify_param(p, a, out),
        (Type::Own(p), Type::Own(a)) => unify_param(p, a, out),
        (Type::Untrusted(p), Type::Untrusted(a)) => unify_param(p, a, out),
        (Type::Secret(p), Type::Secret(a)) => unify_param(p, a, out),
        (Type::Tuple(ps), Type::Tuple(as_)) => {
            for (p, a) in ps.iter().zip(as_) {
                unify_param(p, a, out);
            }
        }
        (Type::Record(ps), Type::Record(as_)) => {
            for ((_, p), (_, a)) in ps.iter().zip(as_) {
                unify_param(p, a, out);
            }
        }
        (
            Type::Fn {
                params: pp,
                ret: pr,
                ..
            },
            Type::Fn {
                params: ap,
                ret: ar,
                ..
            },
        ) => {
            for (p, a) in pp.iter().zip(ap) {
                unify_param(p, a, out);
            }
            unify_param(pr, ar, out);
        }
        // A reference argument satisfies a by-value parameter of the same type,
        // which is how auto-ref receivers reach here.
        (p, Type::Ref { inner: a, .. }) => unify_param(p, a, out),
        _ => {}
    }
}

/// Substitute type params.
pub fn subst(ty: &Type, map: &HashMap<Symbol, Type>) -> Type {
    match ty {
        Type::Param(s) => map.get(s).cloned().unwrap_or_else(|| ty.clone()),
        Type::Named { def, args } => Type::Named {
            def: *def,
            args: args.iter().map(|a| subst(a, map)).collect(),
        },
        Type::Ref {
            region,
            mutable,
            inner,
        } => Type::Ref {
            region: *region,
            mutable: *mutable,
            inner: Box::new(subst(inner, map)),
        },
        Type::Own(t) => Type::Own(Box::new(subst(t, map))),
        Type::Untrusted(t) => Type::Untrusted(Box::new(subst(t, map))),
        Type::Secret(t) => Type::Secret(Box::new(subst(t, map))),
        Type::Fn {
            params,
            ret,
            effects,
        } => Type::Fn {
            params: params.iter().map(|p| subst(p, map)).collect(),
            ret: Box::new(subst(ret, map)),
            effects: effects.clone(),
        },
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| subst(t, map)).collect()),
        Type::Record(fs) => Type::Record(fs.iter().map(|(n, t)| (*n, subst(t, map))).collect()),
        Type::Variant { def, variants } => Type::Variant {
            def: *def,
            variants: variants
                .iter()
                .map(|(vn, fs)| {
                    (
                        *vn,
                        fs.iter().map(|(fnm, t)| (*fnm, subst(t, map))).collect(),
                    )
                })
                .collect(),
        },
        other => other.clone(),
    }
}
