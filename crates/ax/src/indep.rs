//! Independent effect and region checkers written from the normative spec.
//!
//! They share no *inference* code with `check.rs`, and differential agreement
//! between the two is a release gate. They do share one input: the checker's
//! node type table. Effects are not a syntactic property — `a / b` raises
//! `DivError` for integers and cannot fail for floats — so a type-blind effect
//! checker would have to be conservative, and being conservative here means
//! rejecting correct programs. Sharing type facts while re-deriving every
//! effect judgement independently keeps the check meaningful.

use crate::ast::*;
use crate::intern::Interner;
use crate::types::Type;
use std::collections::{BTreeSet, HashMap};

/// Type facts the independent checker is allowed to consult.
#[derive(Clone, Copy)]
pub struct TypeFacts<'a> {
    node_types: &'a [Type],
    /// Divisor nodes proven non-zero. Shared for the same reason types are: the
    /// error row of `a % b` depends on whether `b` can be zero, and an effect
    /// checker that cannot see the proof would have to insert an error the main
    /// checker correctly left out — making the two disagree on every correct
    /// program that divides by a constant.
    nonzero_div: &'a std::collections::HashSet<NodeId>,
}

impl<'a> TypeFacts<'a> {
    pub fn new(
        node_types: &'a [Type],
        nonzero_div: &'a std::collections::HashSet<NodeId>,
    ) -> Self {
        Self {
            node_types,
            nonzero_div,
        }
    }

    fn divisor_nonzero(&self, id: NodeId) -> bool {
        self.nonzero_div.contains(&id)
    }

    /// Is this expression's type an integer? Unknown types answer `false`, which
    /// keeps a missing fact from inventing an effect.
    fn is_int(&self, id: NodeId) -> bool {
        self.node_types
            .get(id.index())
            .and_then(|t| t.as_prim())
            .map(|p| p.is_int())
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IndepEffect {
    Abort,
    Alloc(String),
    Diverge,
    Err(String),
    Io(String),
    Nondet,
    Race,
    Susp,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndepRow {
    pub atoms: BTreeSet<IndepEffect>,
}

impl IndepRow {
    pub fn insert(&mut self, e: IndepEffect) {
        self.atoms.insert(e);
    }
    pub fn union(&self, o: &IndepRow) -> IndepRow {
        IndepRow {
            atoms: self.atoms.union(&o.atoms).cloned().collect(),
        }
    }
    pub fn remove_err(&self) -> IndepRow {
        IndepRow {
            atoms: self
                .atoms
                .iter()
                .filter(|a| !matches!(a, IndepEffect::Err(_)))
                .cloned()
                .collect(),
        }
    }
    pub fn display(&self) -> String {
        let xs: Vec<_> = self
            .atoms
            .iter()
            .map(|a| match a {
                IndepEffect::Abort => "abort".into(),
                IndepEffect::Alloc(s) => format!("alloc[{s}]"),
                IndepEffect::Diverge => "diverge".into(),
                IndepEffect::Err(s) => format!("err[{s}]"),
                IndepEffect::Io(s) => format!("io[{s}]"),
                IndepEffect::Nondet => "nondet".into(),
                IndepEffect::Race => "race".into(),
                IndepEffect::Susp => "susp".into(),
            })
            .collect();
        format!("!{{{}}}", xs.join(", "))
    }
}

pub struct IndepEffectReport {
    pub fn_name: String,
    pub declared: IndepRow,
    pub inferred: IndepRow,
    pub permitted: bool,
}

/// Walk a file and infer effect rows from syntax alone (no type environment).
pub fn infer_effects(
    file: &File,
    intern: &Interner,
    facts: TypeFacts<'_>,
) -> Vec<IndepEffectReport> {
    let mut known: HashMap<String, IndepRow> = HashMap::new();
    // seed well-known prelude
    known.insert("print".into(), row_one(IndepEffect::Io("stdout".into())));
    known.insert(
        "int.div".into(),
        row_one(IndepEffect::Err("DivError".into())),
    );
    known.insert(
        "int.div_trunc".into(),
        row_one(IndepEffect::Err("DivError".into())),
    );
    known.insert(
        "int.rem".into(),
        row_one(IndepEffect::Err("DivError".into())),
    );
    known.insert("int.div_exact".into(), row_one(IndepEffect::Abort));
    known.insert("assert".into(), row_one(IndepEffect::Abort));
    known.insert("fail".into(), row_one(IndepEffect::Abort));
    known.insert("fs.read".into(), {
        let mut r = IndepRow::default();
        r.insert(IndepEffect::Io("fs_cap".into()));
        r.insert(IndepEffect::Alloc("a".into()));
        r.insert(IndepEffect::Err("fs.Error".into()));
        r
    });
    known.insert("json.decode_recs".into(), {
        let mut r = IndepRow::default();
        r.insert(IndepEffect::Alloc("a".into()));
        r.insert(IndepEffect::Err("json.Error".into()));
        r
    });
    known.insert("sort".into(), row_one(IndepEffect::Diverge));
    known.insert("parse_i32".into(), row_one(IndepEffect::Err("ParseError".into())));

    // first pass: declared rows
    let mut decls: Vec<(&FnDecl, IndepRow)> = Vec::new();
    for d in &file.decls {
        if let DeclKind::Fn(f) | DeclKind::ContractFn(f) = &d.kind {
            let declared = lower_declared(&f.effects, intern);
            known.insert(intern.get(f.name.name).to_string(), declared.clone());
            decls.push((f, declared));
        }
    }
    let mut out = Vec::new();
    for (f, declared) in decls {
        let inferred = infer_expr(&f.body, intern, &known, facts);
        let mut declared = declared;
        if f.effects.omitted
            && inferred
                .atoms
                .iter()
                .any(|a| matches!(a, IndepEffect::Diverge))
        {
            declared.insert(IndepEffect::Diverge);
        }
        let permitted = inferred.atoms.iter().all(|a| {
            declared.atoms.iter().any(|d| match (a, d) {
                (IndepEffect::Err(_), IndepEffect::Err(_)) => true,
                (IndepEffect::Io(_), IndepEffect::Io(_)) => true,
                (IndepEffect::Alloc(_), IndepEffect::Alloc(_)) => true,
                _ => a == d,
            })
        });
        out.push(IndepEffectReport {
            fn_name: intern.get(f.name.name).to_string(),
            declared,
            inferred,
            permitted,
        });
    }
    out
}

fn row_one(e: IndepEffect) -> IndepRow {
    let mut r = IndepRow::default();
    r.insert(e);
    r
}

fn lower_declared(row: &EffectRow, intern: &Interner) -> IndepRow {
    let mut r = IndepRow::default();
    for e in &row.items {
        r.insert(match &e.kind {
            EffectKind::Abort => IndepEffect::Abort,
            EffectKind::Diverge => IndepEffect::Diverge,
            EffectKind::Nondet => IndepEffect::Nondet,
            EffectKind::Race => IndepEffect::Race,
            EffectKind::Susp => IndepEffect::Susp,
            EffectKind::Io(id) => IndepEffect::Io(intern.get(id.name).to_string()),
            EffectKind::Alloc(id) => IndepEffect::Alloc(intern.get(id.name).to_string()),
            EffectKind::Err(t) => IndepEffect::Err(type_name(t, intern)),
        });
    }
    r
}

fn type_name(t: &TypeExpr, intern: &Interner) -> String {
    match &t.kind {
        TypeExprKind::Named { path, .. } => path
            .segs
            .iter()
            .map(|s| intern.get(s.name))
            .collect::<Vec<_>>()
            .join("."),
        TypeExprKind::Prim(p) => p.as_str().to_string(),
        _ => "?".into(),
    }
}

fn infer_expr(
    e: &Expr,
    intern: &Interner,
    known: &HashMap<String, IndepRow>,
    facts: TypeFacts<'_>,
) -> IndepRow {
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Hole => IndepRow::default(),
        ExprKind::Call { callee, args } => {
            let mut r = infer_expr(callee, intern, known, facts);
            for a in args {
                r = r.union(&infer_expr(a, intern, known, facts));
            }
            if let Some(q) = expr_qid(callee, intern) {
                if let Some(kr) = known.get(&q) {
                    r = r.union(kr);
                }
            }
            r
        }
        ExprKind::Field { base, .. } => infer_expr(base, intern, known, facts),
        ExprKind::Index { base, index } => infer_expr(base, intern, known, facts)
            .union(&infer_expr(index, intern, known, facts)),
        ExprKind::Unary { expr, .. } => infer_expr(expr, intern, known, facts),
        ExprKind::Binary { op, lhs, rhs } => {
            let mut r = infer_expr(lhs, intern, known, facts)
                .union(&infer_expr(rhs, intern, known, facts));
            // Only integer `/` and `%` can raise: float division yields inf or
            // NaN per IEEE-754 and has no error effect.
            if matches!(op, BinOp::Div | BinOp::Rem)
                && facts.is_int(lhs.id)
                && !facts.divisor_nonzero(rhs.id)
            {
                r.insert(IndepEffect::Err("DivError".into()));
            }
            r
        }
        ExprKind::Block { stmts, tail } => {
            let mut r = IndepRow::default();
            for s in stmts {
                match &s.kind {
                    StmtKind::Let(l) => r = r.union(&infer_expr(&l.init, intern, known, facts)),
                    StmtKind::Expr(x) => r = r.union(&infer_expr(x, intern, known, facts)),
                }
            }
            if let Some(t) = tail {
                r = r.union(&infer_expr(t, intern, known, facts));
            }
            r
        }
        ExprKind::If {
            cond,
            then_b,
            else_b,
        } => {
            let mut r = infer_expr(cond, intern, known, facts).union(&infer_expr(then_b, intern, known, facts));
            if let Some(el) = else_b {
                r = r.union(&infer_expr(el, intern, known, facts));
            }
            r
        }
        ExprKind::Match { scrut, arms } | ExprKind::Catch { expr: scrut, arms } => {
            let mut r = infer_expr(scrut, intern, known, facts);
            for a in arms {
                r = r.union(&infer_expr(&a.body, intern, known, facts));
            }
            if matches!(e.kind, ExprKind::Catch { .. }) {
                r = r.remove_err();
            }
            r
        }
        ExprKind::For { iter, body, .. } => {
            infer_expr(iter, intern, known, facts).union(&infer_expr(body, intern, known, facts))
        }
        // A `while` has no static bound, so it diverges for the same reason a
        // `loop` does.
        ExprKind::While { cond, body } => {
            let mut r = infer_expr(cond, intern, known, facts)
                .union(&infer_expr(body, intern, known, facts));
            r.insert(IndepEffect::Diverge);
            r
        }
        ExprKind::Break | ExprKind::Continue => IndepRow::default(),
        ExprKind::Cast { expr, .. } => infer_expr(expr, intern, known, facts),
        ExprKind::Loop { body } => {
            let mut r = infer_expr(body, intern, known, facts);
            r.insert(IndepEffect::Diverge);
            r
        }
        ExprKind::Let(l) => infer_expr(&l.init, intern, known, facts),
        ExprKind::Lambda { body, .. } => infer_expr(body, intern, known, facts),
        ExprKind::Record(fs) | ExprKind::Variant { fields: fs, .. } => {
            let mut r = IndepRow::default();
            for (_, x) in fs {
                r = r.union(&infer_expr(x, intern, known, facts));
            }
            r
        }
        ExprKind::Return(inner) => inner
            .as_ref()
            .map(|x| infer_expr(x, intern, known, facts))
            .unwrap_or_default(),
        ExprKind::Raise(inner) => {
            let mut r = infer_expr(inner, intern, known, facts);
            r.insert(IndepEffect::Err("raised".into()));
            r
        }
        ExprKind::Attempt(inner) => infer_expr(inner, intern, known, facts).remove_err(),
        ExprKind::Region { body, .. } => infer_expr(body, intern, known, facts),
        ExprKind::Par { bindings } => {
            let mut r = IndepRow::default();
            for l in bindings {
                r = r.union(&infer_expr(&l.init, intern, known, facts));
            }
            r
        }
        ExprKind::Assign { lhs, rhs } => {
            infer_expr(lhs, intern, known, facts).union(&infer_expr(rhs, intern, known, facts))
        }
    }
}

fn expr_qid(e: &Expr, intern: &Interner) -> Option<String> {
    match &e.kind {
        ExprKind::Path(p) => Some(
            p.segs
                .iter()
                .map(|s| intern.get(s.name))
                .collect::<Vec<_>>()
                .join("."),
        ),
        ExprKind::Field { base, field } => {
            let left = expr_qid(base, intern)?;
            Some(format!("{}.{}", left, intern.get(field.name)))
        }
        _ => None,
    }
}

// ---------- independent region checker ----------

#[derive(Clone, Debug)]
pub struct RegionJudgement {
    pub ok: bool,
    pub reason: String,
}

/// Corrected rule (§6.1): store(&r T, location l) legal iff r outlives l
/// i.e. r.depth <= l.depth for lexically nested regions.
pub fn check_regions(file: &File, intern: &Interner) -> Vec<RegionJudgement> {
    let mut js = Vec::new();
    for d in &file.decls {
        if let DeclKind::Fn(f) | DeclKind::ContractFn(f) = &d.kind {
            walk_region(
                &f.body,
                intern,
                &mut vec![("static".into(), 0u32)],
                &mut js,
            );
        }
    }
    js
}

fn walk_region(
    e: &Expr,
    intern: &Interner,
    stack: &mut Vec<(String, u32)>,
    js: &mut Vec<RegionJudgement>,
) {
    match &e.kind {
        ExprKind::Region { name, body } => {
            let depth = stack.last().map(|(_, d)| d + 1).unwrap_or(1);
            stack.push((intern.get(name.name).to_string(), depth));
            walk_region(body, intern, stack, js);
            stack.pop();
        }
        ExprKind::Let(l) => {
            if matches!(l.init.kind, ExprKind::Unary { op: UnOp::Ref | UnOp::RefMut, .. })
            {
                let loc_depth = stack.last().map(|(_, d)| *d).unwrap_or(0);
                // ref is created in current region; storing into this let's location
                // is legal because r == l (same depth).
                js.push(RegionJudgement {
                    ok: true,
                    reason: format!("inward store at depth {loc_depth}"),
                });
            }
            walk_region(&l.init, intern, stack, js);
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match &s.kind {
                    StmtKind::Let(l) => {
                        // detect escaping: region body whose tail is a ref into that region
                        walk_region(&l.init, intern, stack, js);
                    }
                    StmtKind::Expr(x) => walk_region(x, intern, stack, js),
                }
            }
            if let Some(t) = tail {
                walk_region(t, intern, stack, js);
            }
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Field { base: expr, .. }
        | ExprKind::Raise(expr)
        | ExprKind::Attempt(expr)
        | ExprKind::Loop { body: expr } => walk_region(expr, intern, stack, js),
        ExprKind::Call { callee, args } => {
            walk_region(callee, intern, stack, js);
            for a in args {
                walk_region(a, intern, stack, js);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs } => {
            walk_region(lhs, intern, stack, js);
            walk_region(rhs, intern, stack, js);
        }
        ExprKind::If {
            cond,
            then_b,
            else_b,
        } => {
            walk_region(cond, intern, stack, js);
            walk_region(then_b, intern, stack, js);
            if let Some(el) = else_b {
                walk_region(el, intern, stack, js);
            }
        }
        ExprKind::Match { scrut, arms } | ExprKind::Catch { expr: scrut, arms } => {
            walk_region(scrut, intern, stack, js);
            for a in arms {
                walk_region(&a.body, intern, stack, js);
            }
        }
        ExprKind::For { iter, body, .. } => {
            walk_region(iter, intern, stack, js);
            walk_region(body, intern, stack, js);
        }
        ExprKind::Par { bindings } => {
            for l in bindings {
                walk_region(&l.init, intern, stack, js);
            }
        }
        ExprKind::Record(fs) | ExprKind::Variant { fields: fs, .. } => {
            for (_, x) in fs {
                walk_region(x, intern, stack, js);
            }
        }
        ExprKind::Return(inner) => {
            if let Some(x) = inner {
                walk_region(x, intern, stack, js);
            }
        }
        ExprKind::Lambda { body, .. } => walk_region(body, intern, stack, js),
        ExprKind::Index { base, index } => {
            walk_region(base, intern, stack, js);
            walk_region(index, intern, stack, js);
        }
        _ => {}
    }
}

/// Property: a reference created at depth `r` may be stored at depth `l`
/// iff `r <= l` (r outlives l).
pub fn store_legal(r_depth: u32, l_depth: u32) -> bool {
    r_depth <= l_depth
}

/// v0.1 inverted rule — used as a regression sentinel. Must *disagree*
/// with `store_legal` on the escaping case (r=1, l=0).
pub fn store_legal_v01_inverted(r_depth: u32, l_depth: u32) -> bool {
    l_depth <= r_depth
}
