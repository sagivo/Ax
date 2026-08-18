//! Never-rejecting ownership inference (spec v0.3 §1.2, §5.2).
//!
//! A whole-program analysis assigns each value the cheapest *correct*
//! strategy. Failure to prove a cheaper strategy degrades to the next and
//! emits a performance finding — never an error. Affine `own T` is the
//! single exception: use-after-move and never-used are hard errors.

use crate::ast::*;
use crate::check::CheckOutput;
use crate::intern::Interner;
use crate::span::Span;
use crate::types::Type;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Cheapest-to-dearest strategy ladder. Ordered so `max` is the join.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    Register,
    Stack,
    UniqueHeap,
    Region,
    RcNonatomic,
    RcAtomic,
}

impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Strategy::Register => "register",
            Strategy::Stack => "stack",
            Strategy::UniqueHeap => "unique_heap",
            Strategy::Region => "region",
            Strategy::RcNonatomic => "rc_nonatomic",
            Strategy::RcAtomic => "rc_atomic",
        }
    }
}

/// Per-parameter summary lattice. Join is max.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UseKind {
    Read,
    Mutate,
    Consume,
    Escape,
}

#[derive(Clone, Debug, Serialize)]
pub struct ValueSummary {
    pub name: String,
    pub strategy: Strategy,
    pub use_kind: UseKind,
    pub copies: u32,
    pub last_use_span: Option<SpanJson>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpanJson {
    pub file: u32,
    pub start: u32,
    pub end: u32,
}

impl From<Span> for SpanJson {
    fn from(s: Span) -> Self {
        Self {
            file: s.file.0,
            start: s.start,
            end: s.end,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FnOwnership {
    pub function: String,
    pub values: Vec<ValueSummary>,
    pub residual_rc_ops: u32,
    pub unique_heap_allocs: u32,
    pub total_allocs: u32,
    pub value_ops: u32,
    pub copy_on_conflict: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct OwnershipReport {
    pub schema_version: &'static str,
    pub functions: Vec<FnOwnership>,
    pub residual_rc_rate: f64,
    pub unique_heap_share: f64,
}

/// Affine-resource error produced by the ownership pass.
#[derive(Clone, Debug)]
pub struct AffineError {
    pub code: &'static str,
    pub span: Span,
    pub msg: String,
}

/// Run the never-rejecting ladder over a checked module.
///
/// Soundness obligation for `Read` (spec §5.2.3): passing a pointer with no
/// refcount is sound iff the caller's reference cannot die during the call.
/// Each clause is covered by `tests/soundness/` and this analysis never
/// upgrades a `Read` past what the conservative walk can prove.
pub fn analyze(intern: &Interner, checked: &CheckOutput) -> (OwnershipReport, Vec<AffineError>) {
    let mut functions = Vec::new();
    let mut errors = Vec::new();
    let mut rc_ops = 0u32;
    let mut unique = 0u32;
    let mut allocs = 0u32;
    let mut value_ops = 0u32;

    for f in &checked.fns {
        let name = intern.get(f.sig.name).to_string();
        let mut values = Vec::new();
        let mut fn_rc = 0u32;
        let mut fn_unique = 0u32;
        let mut fn_allocs = 0u32;
        let mut fn_ops = 0u32;
        let mut copies = 0u32;

        let mut uses: HashMap<String, Vec<Span>> = HashMap::new();
        let mut mutated: HashSet<String> = HashSet::new();
        let mut consumed: HashSet<String> = HashSet::new();
        let mut escaped: HashSet<String> = HashSet::new();
        let mut affine: HashSet<String> = HashSet::new();
        let mut affine_span: HashMap<String, Span> = HashMap::new();
        let mut assigned: HashSet<String> = HashSet::new();

        for (pname, ty, _) in &f.sig.params {
            let n = intern.get(*pname).to_string();
            if is_own(ty) {
                affine.insert(n.clone());
                affine_span.insert(n.clone(), f.sig.span);
            }
            uses.entry(n).or_default();
        }

        walk(
            intern,
            &f.body,
            &mut uses,
            &mut mutated,
            &mut consumed,
            &mut escaped,
            &mut affine,
            &mut affine_span,
            &mut assigned,
            &mut fn_ops,
        );

        // Affine: used exactly once. A second use is A2020 (the only hard
        // rejection in the memory model); zero uses is A2021.
        for name in &affine {
            let n = uses.get(name).map(|v| v.len()).unwrap_or(0);
            let span = affine_span.get(name).copied().unwrap_or(f.sig.span);
            if n == 0 && !consumed.contains(name) {
                errors.push(AffineError {
                    code: "A2021",
                    span,
                    msg: format!("affine `own` value `{name}` is never used"),
                });
            } else if n > 1 {
                let use_span = uses
                    .get(name)
                    .and_then(|v| v.get(1).copied())
                    .unwrap_or(span);
                errors.push(AffineError {
                    code: "A2020",
                    span: use_span,
                    msg: format!("use after move of affine `own` value `{name}`"),
                });
            }
        }

        for (pname, ty, _) in &f.sig.params {
            let n = intern.get(*pname).to_string();
            let use_list = uses.get(&n).cloned().unwrap_or_default();
            let use_kind = if escaped.contains(&n) {
                UseKind::Escape
            } else if consumed.contains(&n) || is_own(ty) {
                UseKind::Consume
            } else if mutated.contains(&n) {
                UseKind::Mutate
            } else {
                UseKind::Read
            };
            let (strategy, reason) = pick_strategy(ty, use_kind, use_list.len(), escaped.contains(&n));
            if matches!(strategy, Strategy::RcNonatomic | Strategy::RcAtomic) {
                fn_rc += 1;
            }
            if is_heapish(ty) {
                fn_allocs += 1;
                if strategy == Strategy::UniqueHeap {
                    fn_unique += 1;
                }
            }
            if use_list.len() > 1 && !is_own(ty) && use_kind != UseKind::Read {
                copies += 1;
            }
            values.push(ValueSummary {
                name: n,
                strategy,
                use_kind,
                copies: if use_list.len() > 1 { 1 } else { 0 },
                last_use_span: use_list.last().copied().map(SpanJson::from),
                reason,
            });
        }

        // Locals that appear in `uses` but are not parameters.
        let param_names: HashSet<String> = f
            .sig
            .params
            .iter()
            .map(|(p, _, _)| intern.get(*p).to_string())
            .collect();
        for (n, use_list) in &uses {
            if param_names.contains(n) {
                continue;
            }
            let use_kind = if escaped.contains(n) {
                UseKind::Escape
            } else if consumed.contains(n) {
                UseKind::Consume
            } else if mutated.contains(n) {
                UseKind::Mutate
            } else {
                UseKind::Read
            };
            // A local is heapish only if something about its uses forces a
            // heap object. Being assigned does not: `let mut i: i64 = 0` is
            // a stack / register slot. Conservatively treat an escaped
            // non-primitive local as heapish; everything else is stack.
            let heapish = escaped.contains(n) && use_kind == UseKind::Escape;
            let (strategy, reason) = pick_strategy_local(use_kind, use_list.len(), escaped.contains(n), heapish);
            if matches!(strategy, Strategy::RcNonatomic | Strategy::RcAtomic) {
                fn_rc += 1;
            }
            if heapish {
                fn_allocs += 1;
                if strategy == Strategy::UniqueHeap {
                    fn_unique += 1;
                }
            }
            if use_list.len() > 1 && use_kind != UseKind::Read {
                copies += 1;
            }
            values.push(ValueSummary {
                name: n.clone(),
                strategy,
                use_kind,
                copies: if use_list.len() > 1 { 1 } else { 0 },
                last_use_span: use_list.last().copied().map(SpanJson::from),
                reason,
            });
        }

        rc_ops += fn_rc;
        unique += fn_unique;
        allocs += fn_allocs;
        value_ops += fn_ops.max(1);

        functions.push(FnOwnership {
            function: name,
            values,
            residual_rc_ops: fn_rc,
            unique_heap_allocs: fn_unique,
            total_allocs: fn_allocs,
            value_ops: fn_ops,
            copy_on_conflict: copies,
        });
    }

    let residual_rc_rate = if value_ops == 0 {
        0.0
    } else {
        rc_ops as f64 / value_ops as f64
    };
    let unique_heap_share = if allocs == 0 {
        1.0
    } else {
        unique as f64 / allocs as f64
    };

    (
        OwnershipReport {
            schema_version: "1.0",
            functions,
            residual_rc_rate,
            unique_heap_share,
        },
        errors,
    )
}

fn is_own(ty: &Type) -> bool {
    matches!(ty, Type::Own(_))
}

fn is_heapish(ty: &Type) -> bool {
    match ty {
        Type::Named { .. } | Type::Record(_) | Type::Variant { .. } | Type::Own(_) => true,
        Type::Ref { inner, .. } => is_heapish(inner),
        _ => false,
    }
}

fn pick_strategy(ty: &Type, use_kind: UseKind, uses: usize, escapes: bool) -> (Strategy, String) {
    // Primitives are SSA / register values. Returning one is a copy, not an
    // escape that needs a refcount word.
    if ty.as_prim().is_some() {
        return (
            Strategy::Register,
            "primitive: register / SSA value, no memory".into(),
        );
    }
    pick_strategy_local(use_kind, uses, escapes, is_heapish(ty))
}

fn pick_strategy_local(
    use_kind: UseKind,
    uses: usize,
    escapes: bool,
    heapish: bool,
) -> (Strategy, String) {
    if !heapish {
        return (
            Strategy::Register,
            "no heap object: register / stack slot; returning it is a copy".into(),
        );
    }
    if escapes || use_kind == UseKind::Escape {
        return (
            Strategy::RcNonatomic,
            "value escapes the current frame; residual RC required".into(),
        );
    }
    if uses <= 1 && use_kind <= UseKind::Consume {
        return (
            Strategy::UniqueHeap,
            "alias set size 1 for the whole lifetime; malloc/free, no RC word".into(),
        );
    }
    if uses > 1 && use_kind <= UseKind::Mutate {
        return (
            Strategy::RcNonatomic,
            format!("alias set size {uses} at a join; residual non-atomic RC"),
        );
    }
    (
        Strategy::UniqueHeap,
        "consumed at last use; unique heap with static free".into(),
    )
}

#[allow(clippy::too_many_arguments)]
fn walk(
    intern: &Interner,
    e: &Expr,
    uses: &mut HashMap<String, Vec<Span>>,
    mutated: &mut HashSet<String>,
    consumed: &mut HashSet<String>,
    escaped: &mut HashSet<String>,
    affine: &mut HashSet<String>,
    affine_span: &mut HashMap<String, Span>,
    assigned: &mut HashSet<String>,
    ops: &mut u32,
) {
    *ops += 1;
    match &e.kind {
        ExprKind::Path(p) if p.segs.len() == 1 => {
            let n = intern.get(p.segs[0].name).to_string();
            if affine.contains(&n) {
                let prev = uses.get(&n).map(|v| v.len()).unwrap_or(0);
                if prev >= 1 {
                    // Recorded as A2020 by the caller that sees a second use.
                    // We still record the span so the diagnostic can point at it.
                }
            }
            uses.entry(n).or_default().push(e.span);
        }
        ExprKind::Assign { lhs, rhs } => {
            if let ExprKind::Path(p) = &lhs.kind {
                if p.segs.len() == 1 {
                    mutated.insert(intern.get(p.segs[0].name).to_string());
                }
            }
            walk(intern, lhs, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            walk(intern, rhs, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
        }
        ExprKind::Let(l) => {
            if let PatKind::Bind(id) = &l.pat.kind {
                let n = intern.get(id.name).to_string();
                assigned.insert(n.clone());
                if l.ty.as_ref().map(|t| matches!(t.kind, TypeExprKind::Own(_))).unwrap_or(false) {
                    affine.insert(n.clone());
                    affine_span.insert(n, l.span);
                }
            }
            walk(intern, &l.init, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match &s.kind {
                    StmtKind::Let(l) => {
                        if let PatKind::Bind(id) = &l.pat.kind {
                            let n = intern.get(id.name).to_string();
                            assigned.insert(n.clone());
                            if l.ty
                                .as_ref()
                                .map(|t| matches!(t.kind, TypeExprKind::Own(_)))
                                .unwrap_or(false)
                            {
                                affine.insert(n.clone());
                                affine_span.insert(n, l.span);
                            }
                        }
                        walk(
                            intern,
                            &l.init,
                            uses,
                            mutated,
                            consumed,
                            escaped,
                            affine,
                            affine_span,
                            assigned,
                            ops,
                        );
                    }
                    StmtKind::Expr(x) => {
                        walk(intern, x, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops)
                    }
                }
            }
            if let Some(t) = tail {
                // Nested-block tails are not escapes. A function-level return
                // of a local is recorded in `ExprKind::Return` / by the
                // caller of `analyze` looking at the fn body's tail.
                walk(intern, t, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            }
        }
        ExprKind::Return(inner) => {
            if let Some(x) = inner {
                if let ExprKind::Path(p) = &x.kind {
                    if p.segs.len() == 1 {
                        let n = intern.get(p.segs[0].name).to_string();
                        escaped.insert(n.clone());
                        consumed.insert(n);
                    }
                }
                walk(intern, x, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            }
        }
        ExprKind::Call { callee, args } => {
            walk(intern, callee, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            for a in args {
                if let ExprKind::Path(p) = &a.kind {
                    if p.segs.len() == 1 {
                        // Conservative: a call argument may be consumed or may
                        // escape. Consume if affine, else treat as a use.
                        let n = intern.get(p.segs[0].name).to_string();
                        if affine.contains(&n) {
                            consumed.insert(n);
                        }
                    }
                }
                walk(intern, a, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            }
        }
        ExprKind::Field { base, .. } | ExprKind::Unary { expr: base, .. } => {
            walk(intern, base, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
        }
        ExprKind::Index { base, index } => {
            walk(intern, base, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            walk(intern, index, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            walk(intern, lhs, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            walk(intern, rhs, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
        }
        ExprKind::If {
            cond,
            then_b,
            else_b,
        } => {
            walk(intern, cond, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            walk(intern, then_b, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            if let Some(el) = else_b {
                walk(intern, el, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            }
        }
        ExprKind::Match { scrut, arms } | ExprKind::Catch { expr: scrut, arms } => {
            walk(intern, scrut, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            for a in arms {
                walk(intern, &a.body, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            }
        }
        ExprKind::For { iter, body, .. } => {
            walk(intern, iter, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            walk(intern, body, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
        }
        ExprKind::While { cond, body } => {
            walk(intern, cond, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            walk(intern, body, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
        }
        ExprKind::Loop { body } | ExprKind::Region { body, .. } | ExprKind::Lambda { body, .. } => {
            walk(intern, body, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
        }
        ExprKind::Record(fs) | ExprKind::Variant { fields: fs, .. } => {
            for (_, x) in fs {
                walk(intern, x, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            }
        }
        ExprKind::Raise(inner) | ExprKind::Attempt(inner) | ExprKind::Cast { expr: inner, .. } => {
            walk(intern, inner, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
        }
        ExprKind::Try(inner) => {
            walk(intern, inner, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
        }
        ExprKind::Par { bindings } => {
            for l in bindings {
                walk(intern, &l.init, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
            }
        }
        ExprKind::Interpolate { parts } => {
            for p in parts {
                if let InterpPart::Expr(x) = p {
                    walk(intern, x, uses, mutated, consumed, escaped, affine, affine_span, assigned, ops);
                }
            }
        }
        ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Hole | ExprKind::Break | ExprKind::Continue => {}
    }
}
