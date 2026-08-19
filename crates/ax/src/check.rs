//! Type, effect, and region checker. Effects are inferred in bodies and
//! checked against the declared row. Region store rule: r must outlive l.

use crate::ast::*;
use crate::builtins::{self, Builtins};
use crate::diag::{Diagnostic, Fix, FixSafety};
use crate::effects::{EffectAtom, EffectSet};
use crate::hash;
use crate::intern::{Interner, Symbol};
use crate::span::Span;
use crate::types::{
    types_eq, DictDef, FnSig, Prim, RegionId, ResolvedInjection, Type, TypeDef, TypeDefKind,
};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct HoleInfo {
    pub def_id: String,
    pub path: String,
    pub span: Span,
    pub expected: Type,
    pub in_scope: Vec<(String, Type)>,
    pub candidates: Vec<HoleCandidate>,
}

#[derive(Clone, Debug)]
pub struct HoleCandidate {
    pub rank: u32,
    pub name: String,
    pub ty: String,
    pub note: String,
}

#[derive(Clone, Debug)]
pub struct CheckedFn {
    pub sig: FnSig,
    pub body: Expr,
    pub inferred: EffectSet,
    pub contracts: Vec<Contract>,
}

#[derive(Clone, Debug)]
pub struct CheckedTest {
    pub name: String,
    pub body: Expr,
    pub def_id: String,
}

pub struct CheckOutput {
    pub module: String,
    pub exports: Vec<String>,
    pub fns: Vec<CheckedFn>,
    pub tests: Vec<CheckedTest>,
    pub types: Vec<TypeDef>,
    pub dicts: Vec<DictDef>,
    /// Dictionary declarations as written, in the same order as `dicts`. The
    /// field *expressions* are what build a vtable; `dicts` only has the types.
    pub dict_decls: Vec<DictDecl>,
    pub diags: Vec<Diagnostic>,
    pub holes: Vec<HoleInfo>,
    pub hashes: Vec<DefHash>,
    /// Callee name → (from_type_display, into_variant_name)
    pub injections: Vec<(String, String, String)>,
    /// Type of every expression and pattern, indexed by `NodeId`. Total over
    /// all nodes the checker visited; `Type::Error` for unvisited slots.
    pub node_types: Vec<Type>,
    /// Integer `/` and `%` nodes whose divisor the checker proved non-zero, so
    /// they carry no `err[DivError]` and need no raise path. See
    /// `Checker::nonzero_locals`.
    pub nonzero_div: std::collections::HashSet<NodeId>,
    /// The subset of `nonzero_div` whose proof rests on the absence of
    /// wrap-around: the divisor was reached through `d = d + k`, and adding to a
    /// non-zero value only stays non-zero until it wraps past the maximum. The
    /// row is still free of `err[DivError]` — no *recoverable* error can occur —
    /// but the backend keeps a guard that aborts, because the alternative is a
    /// silently wrong answer (or a hardware trap) in the case the analysis waved
    /// away. Divisors *not* in this set are unconditionally non-zero and need no
    /// guard at all.
    pub nonzero_div_needs_guard: std::collections::HashSet<NodeId>,
    /// Every callable visible to the module: user functions and the prelude,
    /// as (name, parameter types, result type, effects). The agent layer uses
    /// this to synthesise hole fills; publishing it avoids a second, divergent
    /// notion of "what is in scope".
    pub callables: Vec<CallableInfo>,
    /// Which dictionary a `= default` parameter resolved to, keyed by
    /// (call node, parameter index). Resolution is a checker judgement (the
    /// unique visible `dict D[T]`), so it is published rather than repeated.
    pub dict_defaults: HashMap<(NodeId, u32), usize>,
    /// Patterns written as a bare name that actually name a variant of the
    /// scrutinee type, e.g. `Dot` in `match s { Dot => .. }`. Without this the
    /// parser's `PatKind::Bind` would bind a fresh variable and match anything,
    /// which silently selects the wrong arm.
    pub pat_variant: HashMap<NodeId, String>,
    /// Error type each `catch` / `attempt` node handles. Only the checker knows
    /// this (it is the row's `err[E]` at that point, which `catch` discharges),
    /// so it is published rather than re-derived during lowering.
    pub caught: HashMap<NodeId, Type>,
    /// Ownership-ladder report. Published so lowering can honour the chosen
    /// strategy (register / unique-heap / residual RC) instead of re-deriving it.
    pub ownership: crate::ownership::OwnershipReport,
}

impl CheckOutput {
    /// Type recorded for a node. Panics only on an id outside the table,
    /// which means the node was never checked — a lowering bug, not user error.
    pub fn ty(&self, id: NodeId) -> &Type {
        // Out of range means the node was synthesised after checking; treat it
        // like an unvisited node rather than crashing the compiler.
        self.node_types.get(id.index()).unwrap_or(&Type::Error)
    }
}

/// A callable the module can name. See `CheckOutput::callables`.
#[derive(Clone, Debug)]
pub struct CallableInfo {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
    pub effects: crate::effects::EffectSet,
    /// True for prelude entries, false for functions declared in this module.
    pub from_prelude: bool,
}

#[derive(Clone, Debug)]
pub struct DefHash {
    pub def_id: String,
    pub interface_hash: String,
    pub body_hash: String,
    pub build_hash: String,
}

struct Binding {
    name: Symbol,
    ty: Type,
    mutable: bool,
    #[allow(dead_code)]
    region: Option<RegionId>,
}

struct Scope {
    bindings: Vec<Binding>,
}

pub struct Checker<'a> {
    intern: &'a mut Interner,
    b: Builtins,
    type_defs: HashMap<Symbol, TypeDef>,
    /// Qualified and bare function names.
    fns: HashMap<String, FnSig>,
    fns_by_sym: HashMap<Symbol, FnSig>,
    dicts: Vec<DictDef>,
    /// Variant name -> (parent type def, variant fields)
    variants: HashMap<Symbol, (Symbol, Vec<(Symbol, Type)>)>,
    diags: Vec<Diagnostic>,
    holes: Vec<HoleInfo>,
    allow_holes: bool,
    strict_det: bool,
    require_annotations: bool,
    module: String,
    /// Imports: last segment / alias -> prefix used for lookup (e.g. "fs")
    imports: HashMap<Symbol, String>,
    /// Type of every `Expr` / `Pattern`, indexed by `NodeId`. This is the
    /// checker's output contract for all backends: nobody re-derives types
    /// from syntax.
    node_types: Vec<Type>,
    /// See `CheckOutput::caught`.
    caught: HashMap<NodeId, Type>,
    /// See `CheckOutput::pat_variant`.
    pat_variant: HashMap<NodeId, String>,
    /// See `CheckOutput::dict_defaults`.
    dict_defaults: HashMap<(NodeId, u32), usize>,
    /// Call node currently being checked, for recording dict resolutions.
    cur_call: Vec<NodeId>,
    /// Names currently known non-zero, innermost scope last.
    ///
    /// Scoped *and* flow-sensitive: a guard's implication (`while b != 0 { .. }`)
    /// holds only inside the guarded region, and only until something assigns to
    /// the name a value that could be zero. Euclid's algorithm needs exactly
    /// that: `a % b` is safe, and the `b = t` two lines later is what ends the
    /// fact.
    nonzero_scope: Vec<Vec<Symbol>>,
    /// See `CheckOutput::nonzero_div`.
    nonzero_div: std::collections::HashSet<NodeId>,
    nonzero_div_needs_guard: std::collections::HashSet<NodeId>,
    /// Names whose non-zero fact survives only because `d = d + k` was assumed
    /// not to wrap. Collected by a pre-pass over the whole function body, because
    /// the use (`n % d`) is visited before the increment that weakens it.
    wrap_assumed: std::collections::HashSet<Symbol>,
}

struct Env {
    scopes: Vec<Scope>,
    expected_ret: Type,
    declared: EffectSet,
    inferred: EffectSet,
    region_stack: Vec<RegionId>,
    next_depth: u32,
    in_contract: bool,
    /// How many loops enclose the expression being checked; `break` and
    /// `continue` are only legal inside one.
    loop_depth: u32,
    def_id: String,
    /// Exclusive mut-borrowed locals in the current statement.
    mut_borrowed: Vec<Symbol>,
}

impl Env {
    fn lookup(&self, name: Symbol) -> Option<&Binding> {
        for s in self.scopes.iter().rev() {
            if let Some(b) = s.bindings.iter().rev().find(|b| b.name == name) {
                return Some(b);
            }
        }
        None
    }

    fn push(&mut self) {
        self.scopes.push(Scope {
            bindings: Vec::new(),
        });
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn bind(&mut self, name: Symbol, ty: Type, mutable: bool, region: Option<RegionId>) {
        if let Some(s) = self.scopes.last_mut() {
            s.bindings.push(Binding {
                name,
                ty,
                mutable,
                region,
            });
        }
    }

    fn in_scope(&self, intern: &Interner) -> Vec<(String, Type)> {
        let mut out = Vec::new();
        for s in &self.scopes {
            for b in &s.bindings {
                out.push((intern.get(b.name).to_string(), b.ty.clone()));
            }
        }
        out
    }
}

impl<'a> Checker<'a> {
    pub fn new(intern: &'a mut Interner, allow_holes: bool, strict_det: bool) -> Self {
        let b = Builtins::intern(intern);
        let mut type_defs = HashMap::new();
        for td in builtins::core_type_defs(intern, &b) {
            type_defs.insert(td.name, td);
        }
        for td in builtins::extra_type_defs(intern) {
            type_defs.insert(td.name, td);
        }
        let mut fns = HashMap::new();
        let mut fns_by_sym = HashMap::new();
        for (qid, sig) in builtins::core_fns(intern, &b) {
            fns_by_sym.insert(sig.name, sig.clone());
            fns.insert(qid, sig);
        }
        let mut variants = HashMap::new();
        // Seed from known defs
        let seed_names: Vec<Symbol> = type_defs.keys().copied().collect();
        for name in seed_names {
            if let Some(td) = type_defs.get(&name) {
                if let TypeDefKind::Variants(vs) = &td.kind {
                    for (vn, fs) in vs {
                        variants.insert(*vn, (name, fs.clone()));
                    }
                }
            }
        }
        Self {
            intern,
            b,
            type_defs,
            fns,
            fns_by_sym,
            dicts: Vec::new(),
            variants,
            diags: Vec::new(),
            holes: Vec::new(),
            allow_holes,
            strict_det,
            require_annotations: false,
            module: String::new(),
            imports: HashMap::new(),
            node_types: Vec::new(),
            caught: HashMap::new(),
            pat_variant: HashMap::new(),
            dict_defaults: HashMap::new(),
            cur_call: Vec::new(),
            nonzero_scope: Vec::new(),
            nonzero_div: std::collections::HashSet::new(),
            nonzero_div_needs_guard: std::collections::HashSet::new(),
            wrap_assumed: std::collections::HashSet::new(),
        }
    }

    /// Record `ty` as the type of node `id` and hand it back unchanged.
    ///
    /// Called at the single exit of `check_expr` and at each of its early
    /// returns, so the table is total over every expression the checker
    /// visits. Later writes win, which is what we want: an outer pass that
    /// refines a type (e.g. an integer literal against an expected width)
    /// should be the one the backend sees.
    fn rec(&mut self, id: NodeId, ty: Type) -> Type {
        if id != NodeId::NONE {
            let i = id.index();
            if self.node_types.len() <= i {
                self.node_types.resize(i + 1, Type::Error);
            }
            self.node_types[i] = ty.clone();
        }
        ty
    }

    pub fn check_file(&mut self, file: &File) -> CheckOutput {
        // Size the type table to the whole AST up front. Some nodes are never
        // visited (the base of a qualified path like `test.alloc` resolves as a
        // unit), and a total table means no consumer has to handle a gap.
        self.node_types.resize(file.node_count, Type::Error);
        // Kept alongside `self.dicts` so the two stay index-aligned.
        let mut dict_decls: Vec<DictDecl> = Vec::new();
        self.module = path_str(&file.module, self.intern);
        // Accept-and-elide: `unsafe` is parsed as decl meta and is meaningless.
        // Name Rust in the message per [R-8.1.3] / [T-3.4.2].
        for d in &file.decls {
            for m in &d.meta {
                if self.intern.get(m.key.name) == "unsafe" {
                    self.diags.push(Diagnostic::warn(
                        "A0106",
                        m.span,
                        "`unsafe` is meaningless in Ax (no unsafe subset); this is a documented divergence from Rust",
                    ));
                }
            }
        }
        for u in &file.uses {
            let last = u.path.segs.last().map(|s| s.name);
            let key = u.alias.as_ref().map(|a| a.name).or(last);
            if let Some(k) = key {
                let prefix = self.intern.get(k).to_string();
                self.imports.insert(k, prefix);
            }
        }

        // Pass 1: collect type decls (names first, then bodies).
        let mut pending_types: Vec<&TypeDecl> = Vec::new();
        for d in &file.decls {
            if let DeclKind::Type(td) = &d.kind {
                pending_types.push(td);
                self.type_defs.insert(
                    td.name.name,
                    TypeDef {
                        name: td.name.name,
                        generics: td.generics.iter().map(|g| g.name.name).collect(),
                        kind: TypeDefKind::Alias(Type::Named {
                            def: td.name.name,
                            args: vec![],
                        }),
                        injections: vec![],
                        span: td.name.span,
                        def_id: hash::def_id(&self.module, "type", self.intern.get(td.name.name)),
                    },
                );
            }
        }
        for td in pending_types {
            self.define_type(td);
        }

        // Pass 2: function signatures + dicts
        let mut pending_fns: Vec<(&FnDecl, bool)> = Vec::new();
        for d in &file.decls {
            match &d.kind {
                DeclKind::Fn(f) => {
                    let sig = self.lower_fn_sig(f, false);
                    let q = self.intern.get(f.name.name).to_string();
                    self.fns.insert(q, sig.clone());
                    self.fns_by_sym.insert(f.name.name, sig);
                    pending_fns.push((f, false));
                }
                DeclKind::ContractFn(f) => {
                    let sig = self.lower_fn_sig(f, true);
                    let q = self.intern.get(f.name.name).to_string();
                    self.fns.insert(q, sig.clone());
                    self.fns_by_sym.insert(f.name.name, sig);
                    pending_fns.push((f, true));
                }
                DeclKind::Dict(dd) => {
                    dict_decls.push(dd.clone());
                    let for_ty = self.lower_type(&dd.for_ty, None);
                    let mut fields = Vec::new();
                    for (n, e) in &dd.fields {
                        let t = match &e.kind {
                            ExprKind::Path(p) if p.segs.len() == 1 => {
                                if let Some(sig) = self.fns_by_sym.get(&p.segs[0].name) {
                                    Type::Fn {
                                        params: sig
                                            .params
                                            .iter()
                                            .map(|(_, t, _)| t.clone())
                                            .collect(),
                                        ret: Box::new(sig.ret.clone()),
                                        effects: sig.effects.clone(),
                                    }
                                } else {
                                    Type::Hole
                                }
                            }
                            _ => Type::Hole,
                        };
                        fields.push((n.name, t));
                    }
                    self.dicts.push(DictDef {
                        name: dd.name.name,
                        for_ty,
                        fields,
                        span: dd.name.span,
                        def_id: hash::def_id(&self.module, "dict", self.intern.get(dd.name.name)),
                    });
                }
                _ => {}
            }
        }

        let mut checked_fns = Vec::new();
        let mut hashes = Vec::new();
        for (f, is_c) in pending_fns {
            let mut sig = self.fns_by_sym.get(&f.name.name).cloned().expect("sig");
            if self.strict_det {
                self.check_strict_det(&sig);
            }
            if sig.effects.err_count() > 1 {
                self.err("E0201", f.name.span, "at most one err[E] per concrete row");
            }
            for c in &f.contracts {
                self.check_contract(c, &sig);
            }
            self.nonzero_scope.clear();
            // Which names are only non-zero if no increment wraps. Computed for
            // the whole body up front: `n % d` is checked before the `d = d + 1`
            // that weakens the fact, so discovering it in statement order would
            // report the first division as unconditionally safe.
            self.wrap_assumed.clear();
            self.collect_wrap_assumed(&f.body);
            let mut env = Env {
                scopes: vec![Scope {
                    bindings: Vec::new(),
                }],
                expected_ret: sig.ret.clone(),
                declared: sig.effects.clone(),
                inferred: EffectSet::empty(),
                region_stack: vec![RegionId::static_region(self.intern.intern("static"))],
                next_depth: 1,
                in_contract: false,
                loop_depth: 0,
                def_id: sig.def_id.clone(),
                mut_borrowed: Vec::new(),
            };
            for (n, ty, _) in &sig.params {
                env.bind(*n, ty.clone(), false, None);
            }
            // `ret` is bound in postconditions only; body uses return type as expected.
            let got = self.check_expr(&f.body, Some(&sig.ret), &mut env);
            if !types_eq(&got, &sig.ret) && !got.is_error() {
                self.type_mismatch(f.body.span, &sig.ret, &got, &sig.def_id);
            }
            if f.effects.omitted {
                // An omitted row is not a claim. Reconstruct `diverge` from the
                // body — it does not change the ABI — and still reject anything
                // a caller would have to know about (`err`, `io`, `alloc`, …).
                let mut declared = env.declared.clone();
                if env
                    .inferred
                    .atoms
                    .iter()
                    .any(|a| matches!(a, EffectAtom::Diverge))
                {
                    declared.insert(EffectAtom::Diverge);
                }
                self.check_row_subset(&env.inferred, &declared, f.body.span, &sig.def_id);
                env.declared = declared.clone();
                sig.effects = declared;
                let q = self.intern.get(sig.name).to_string();
                self.fns.insert(q, sig.clone());
                self.fns_by_sym.insert(sig.name, sig.clone());
            } else {
                self.check_row_subset(&env.inferred, &env.declared, f.body.span, &sig.def_id);
            }
            let iface = format!(
                "{}{}{}",
                self.intern.get(sig.name),
                sig.ret.display(self.intern),
                sig.effects.display(self.intern)
            );
            let body_s = format!("{:?}", f.body.kind);
            hashes.push(DefHash {
                def_id: sig.def_id.clone(),
                interface_hash: hash::interface_hash(&iface),
                body_hash: hash::body_hash(&body_s),
                build_hash: hash::build_hash(&body_s, &[], "oracle", ""),
            });
            checked_fns.push(CheckedFn {
                sig,
                body: f.body.clone(),
                inferred: env.inferred,
                contracts: f.contracts.clone(),
            });
            // Affine `own` is the one place the ownership ladder may reject.
            // Run it per function as we go so A2020/A2021 land with the rest
            // of the diagnostics.
            let _ = is_c;
        }

        let mut tests = Vec::new();
        for d in &file.decls {
            if let DeclKind::Test(t) = &d.kind {
                let def_id = hash::def_id(&self.module, "test", &t.name);
                let mut env = Env {
                    scopes: vec![Scope {
                        bindings: Vec::new(),
                    }],
                    expected_ret: Type::unit(),
                    declared: {
                        let mut e = EffectSet::new();
                        e.insert(EffectAtom::Abort);
                        e.insert(EffectAtom::Diverge);
                        e
                    },
                    inferred: EffectSet::empty(),
                    region_stack: vec![RegionId::static_region(self.intern.intern("static"))],
                    next_depth: 1,
                    in_contract: false,
                    loop_depth: 0,
                    def_id: def_id.clone(),
                    mut_borrowed: Vec::new(),
                };
                let _ = self.check_expr(&t.body, Some(&Type::unit()), &mut env);
                tests.push(CheckedTest {
                    name: t.name.clone(),
                    body: t.body.clone(),
                    def_id,
                });
            }
        }

        let exports: Vec<String> = file
            .exports
            .iter()
            .map(|i| self.intern.get(i.name).to_string())
            .collect();

        let mut injections = Vec::new();
        for td in self.type_defs.values() {
            let into = self.intern.get(td.name).to_string();
            for inj in &td.injections {
                injections.push((
                    into.clone(),
                    inj.from.display(self.intern),
                    self.intern.get(inj.into_variant).to_string(),
                ));
            }
        }
        // Built before `checked_fns` is moved into the output.
        let callables: Vec<CallableInfo> = {
            let local_ids: Vec<String> = checked_fns.iter().map(|f| f.sig.def_id.clone()).collect();
            let mut out: Vec<CallableInfo> = Vec::new();
            for (name, sig) in &self.fns {
                out.push(CallableInfo {
                    name: name.clone(),
                    params: sig.params.iter().map(|(_, t, _)| t.clone()).collect(),
                    ret: sig.ret.clone(),
                    effects: sig.effects.clone(),
                    from_prelude: !local_ids.contains(&sig.def_id),
                });
            }
            out.sort_by(|a, b| a.name.cmp(&b.name));
            out.dedup_by(|a, b| a.name == b.name);
            out
        };
        let prelim = CheckOutput {
            module: self.module.clone(),
            exports: exports.clone(),
            fns: checked_fns.clone(),
            tests: tests.clone(),
            types: self.type_defs.values().cloned().collect(),
            dicts: self.dicts.clone(),
            dict_decls: dict_decls.clone(),
            diags: Vec::new(),
            holes: Vec::new(),
            hashes: Vec::new(),
            injections: Vec::new(),
            node_types: Vec::new(),
            caught: HashMap::new(),
            pat_variant: HashMap::new(),
            dict_defaults: HashMap::new(),
            nonzero_div: Default::default(),
            nonzero_div_needs_guard: Default::default(),
            callables: Vec::new(),
            ownership: crate::ownership::OwnershipReport {
                schema_version: "1.0",
                functions: Vec::new(),
                residual_rc_rate: 0.0,
                unique_heap_share: 1.0,
            },
        };
        let (own, affine) = crate::ownership::analyze(self.intern, &prelim);
        for e in affine {
            self.err(e.code, e.span, e.msg);
        }

        CheckOutput {
            module: self.module.clone(),
            exports,
            fns: checked_fns,
            tests,
            types: self.type_defs.values().cloned().collect(),
            dicts: self.dicts.clone(),
            dict_decls: dict_decls.clone(),
            diags: std::mem::take(&mut self.diags),
            holes: std::mem::take(&mut self.holes),
            hashes,
            injections,
            node_types: std::mem::take(&mut self.node_types),
            caught: std::mem::take(&mut self.caught),
            pat_variant: std::mem::take(&mut self.pat_variant),
            dict_defaults: std::mem::take(&mut self.dict_defaults),
            nonzero_div: std::mem::take(&mut self.nonzero_div),
            nonzero_div_needs_guard: std::mem::take(&mut self.nonzero_div_needs_guard),
            callables,
            ownership: own,
        }
    }

    fn define_type(&mut self, td: &TypeDecl) {
        let generics: Vec<Symbol> = td.generics.iter().map(|g| g.name.name).collect();
        let kind = match &td.body {
            TypeBody::Alias(t) => TypeDefKind::Alias(self.lower_type(t, None)),
            TypeBody::Record(fs) => TypeDefKind::Record(
                fs.iter()
                    .map(|f| (f.name.name, self.lower_type(&f.ty, None)))
                    .collect(),
            ),
            TypeBody::Variants(vs) => {
                let mut out = Vec::new();
                for v in vs {
                    let fields: Vec<(Symbol, Type)> = v
                        .fields
                        .iter()
                        .map(|f| (f.name.name, self.lower_type(&f.ty, None)))
                        .collect();
                    self.variants
                        .insert(v.name.name, (td.name.name, fields.clone()));
                    out.push((v.name.name, fields));
                }
                TypeDefKind::Variants(out)
            }
        };
        let mut injections = Vec::new();
        let mut seen_from: HashMap<String, Span> = HashMap::new();
        for inj in &td.injections {
            let from = self.lower_type(&inj.from, None);
            let key = from.display(self.intern);
            if let Some(_prev) = seen_from.insert(key.clone(), inj.span) {
                self.err(
                    "E0203",
                    inj.span,
                    format!("ambiguous injection from `{key}`"),
                );
            }
            injections.push(ResolvedInjection {
                from,
                into_variant: inj.into_variant.name,
                span: inj.span,
            });
        }
        let def_id = hash::def_id(&self.module, "type", self.intern.get(td.name.name));
        self.type_defs.insert(
            td.name.name,
            TypeDef {
                name: td.name.name,
                generics,
                kind,
                injections,
                span: td.name.span,
                def_id,
            },
        );
    }

    fn lower_fn_sig(&mut self, f: &FnDecl, is_contract: bool) -> FnSig {
        let generics: Vec<Symbol> = f.generics.iter().map(|g| g.name.name).collect();
        let params = f
            .params
            .iter()
            .map(|p| (p.name.name, self.lower_type(&p.ty, None), p.default_dict))
            .collect();
        let ret = self.lower_type(&f.ret, None);
        let effects = self.lower_effects(&f.effects);
        FnSig {
            name: f.name.name,
            generics,
            params,
            ret,
            effects,
            is_contract_fn: is_contract,
            span: f.name.span,
            def_id: hash::def_id(&self.module, "fn", self.intern.get(f.name.name)),
        }
    }

    fn lower_effects(&mut self, row: &EffectRow) -> EffectSet {
        let mut set = EffectSet::new();
        for e in &row.items {
            let atom = match &e.kind {
                EffectKind::Err(t) => EffectAtom::Err(self.lower_type(t, None)),
                EffectKind::Io(id) => EffectAtom::Io(id.name),
                EffectKind::Alloc(id) => EffectAtom::Alloc(id.name),
                EffectKind::Susp => EffectAtom::Susp,
                EffectKind::Diverge => EffectAtom::Diverge,
                EffectKind::Race => EffectAtom::Race,
                EffectKind::Nondet => EffectAtom::Nondet,
                EffectKind::Abort => EffectAtom::Abort,
            };
            set.insert(atom);
        }
        set
    }

    fn lower_type(&mut self, t: &TypeExpr, env_region: Option<RegionId>) -> Type {
        match &t.kind {
            TypeExprKind::Prim(p) => Type::Prim(*p),
            TypeExprKind::Hole => Type::Hole,
            TypeExprKind::Own(inner) => Type::Own(Box::new(self.lower_type(inner, env_region))),
            TypeExprKind::Untrusted(inner) => {
                Type::Untrusted(Box::new(self.lower_type(inner, env_region)))
            }
            TypeExprKind::Secret(inner) => {
                Type::Secret(Box::new(self.lower_type(inner, env_region)))
            }
            TypeExprKind::Ref {
                region,
                mutable,
                inner,
            } => {
                let r = env_region.unwrap_or(RegionId {
                    name: region.name,
                    depth: 0,
                });
                // Prefer the named region's identity; depth 0 unless we know better.
                Type::Ref {
                    region: RegionId {
                        name: region.name,
                        depth: r.depth,
                    },
                    mutable: *mutable,
                    inner: Box::new(self.lower_type(inner, env_region)),
                }
            }
            TypeExprKind::Fn {
                params,
                ret,
                effects,
            } => Type::Fn {
                params: params
                    .iter()
                    .map(|p| self.lower_type(p, env_region))
                    .collect(),
                ret: Box::new(self.lower_type(ret, env_region)),
                effects: self.lower_effects(effects),
            },
            TypeExprKind::Tuple(ts) => {
                Type::Tuple(ts.iter().map(|p| self.lower_type(p, env_region)).collect())
            }
            TypeExprKind::Named { path, args } => {
                let name = self.resolve_type_name(path);
                let n = self.intern.get(name).to_string();
                if let Some(p) = Prim::from_str(&n) {
                    return Type::Prim(p);
                }
                // Accept-and-elide: Box/Rc/Arc/RefCell are the inner value
                // ([T-3.3.1] A0104 / A0105).
                if matches!(n.as_str(), "Box" | "Rc" | "Arc") {
                    self.diags.push(Diagnostic::warn(
                        "A0104",
                        path.span,
                        format!("`{n}` is treated as the inner value; this is a documented divergence from Rust"),
                    ));
                    if let Some(inner) = args.first() {
                        return self.lower_type(inner, env_region);
                    }
                }
                if n == "RefCell" {
                    self.diags.push(Diagnostic::warn(
                        "A0105",
                        path.span,
                        "`RefCell` is identity; Ax has no interior mutability (documented divergence from Rust)",
                    ));
                    if let Some(inner) = args.first() {
                        return self.lower_type(inner, env_region);
                    }
                }
                // slice syntax via named `[T]` is represented as named slice
                let lowered_args: Vec<Type> = args
                    .iter()
                    .map(|a| self.lower_type(a, env_region))
                    .collect();
                Type::Named {
                    def: name,
                    args: lowered_args,
                }
            }
        }
    }

    fn resolve_type_name(&mut self, path: &Path) -> Symbol {
        if path.segs.len() == 1 {
            return path.segs[0].name;
        }
        // fs.Error, json.Error → intern "fs.Error"
        let joined = path
            .segs
            .iter()
            .map(|s| self.intern.get(s.name))
            .collect::<Vec<_>>()
            .join(".");
        self.intern.intern(&joined)
    }

    fn check_strict_det(&mut self, sig: &FnSig) {
        if sig.effects.has_io() || sig.effects.has_race() || sig.effects.has_nondet() {
            self.err(
                "E0502",
                sig.span,
                "--strict-det rejects io, race, and nondet",
            );
        }
    }

    fn check_contract(&mut self, c: &Contract, sig: &FnSig) {
        let mut env = Env {
            scopes: vec![Scope {
                bindings: Vec::new(),
            }],
            expected_ret: Type::bool(),
            declared: EffectSet::empty(),
            inferred: EffectSet::empty(),
            region_stack: vec![RegionId::static_region(self.intern.intern("static"))],
            next_depth: 1,
            in_contract: true,
            loop_depth: 0,
            def_id: sig.def_id.clone(),
            mut_borrowed: Vec::new(),
        };
        for (n, ty, _) in &sig.params {
            env.bind(*n, ty.clone(), false, None);
        }
        if c.kind == ContractKind::Post {
            let ret_n = self.intern.intern("ret");
            env.bind(ret_n, sig.ret.clone(), false, None);
        }
        let got = self.check_expr(&c.expr, Some(&Type::bool()), &mut env);
        if !types_eq(&got, &Type::bool()) && !got.is_error() {
            self.type_mismatch(c.span, &Type::bool(), &got, &sig.def_id);
        }
        if !env.inferred.is_empty() {
            self.err("E0501", c.span, "contracts must be effect-free");
        }
    }

    fn check_expr(&mut self, e: &Expr, expected: Option<&Type>, env: &mut Env) -> Type {
        if env.in_contract {
            if !contract_legal(&e.kind) {
                self.err("E0501", e.span, "illegal construct in contract sublanguage");
            }
        }
        let ty = match &e.kind {
            ExprKind::Lit(l) => self.lit_type(l, expected),
            ExprKind::Hole => {
                let exp = expected.cloned().unwrap_or(Type::Hole);
                if !self.allow_holes {
                    let mut d = Diagnostic::error(
                        "E0500",
                        e.span,
                        "typed hole rejected (use ax check --allow-holes)",
                    )
                    .with_def(&env.def_id);
                    d.fixes.push(Fix {
                        kind: "fill_hole".into(),
                        safety: FixSafety::BehaviorChanging,
                        rank: 1,
                        patch: format!("/* expected {} */", exp.display(self.intern)),
                        note: Some("replace ? with a value of the expected type".into()),
                    });
                    self.diags.push(d);
                }
                let cands = self.rank_candidates(&exp, env);
                self.holes.push(HoleInfo {
                    def_id: env.def_id.clone(),
                    path: "body".into(),
                    span: e.span,
                    expected: exp.clone(),
                    in_scope: env.in_scope(self.intern),
                    candidates: cands,
                });
                exp
            }
            ExprKind::Path(p) => self.check_path(p, expected, env),
            ExprKind::Call { callee, args } => {
                self.cur_call.push(e.id);
                let t = self.check_call(callee, args, expected, env, e.span);
                self.cur_call.pop();
                t
            }
            ExprKind::Field { base, field } => {
                if let Some(q) = self.qualified_name(e) {
                    if q == "test.alloc" {
                        let t = builtins::alloc_type(&self.b);
                        return self.rec(e.id, t);
                    }
                    if let Some(sig) = self.lookup_fn(&q) {
                        let t = Type::Fn {
                            params: sig.params.iter().map(|(_, t, _)| t.clone()).collect(),
                            ret: Box::new(sig.ret.clone()),
                            effects: sig.effects.clone(),
                        };
                        return self.rec(e.id, t);
                    }
                }
                let bt = self.check_expr(base, None, env);
                self.field_type(&bt, field, e.span)
            }
            ExprKind::Index { base, index } => {
                let bt = self.check_expr(base, None, env);
                let it = self.check_expr(index, Some(&Type::usz()), env);
                if let Some(p) = it.as_prim() {
                    if !p.is_int() {
                        self.type_mismatch(index.span, &Type::usz(), &it, &env.def_id);
                    }
                }
                self.elem_type(&bt, e.span)
            }
            ExprKind::Unary { op, expr } => self.check_unary(*op, expr, expected, env, e.span),
            ExprKind::Binary { op, lhs, rhs } => {
                self.check_binary(*op, lhs, rhs, expected, env, e.span)
            }
            ExprKind::Block { stmts, tail } => {
                env.push();
                let mut block_nonzero: Vec<Symbol> = Vec::new();
                self.push_nonzero(Vec::new());
                for s in stmts {
                    // A statement that may assign zero to a tracked name ends the
                    // fact *before* that statement is checked, so nothing after it
                    // relies on a stale proof.
                    self.invalidate_nonzero(&s.kind);
                    match &s.kind {
                        StmtKind::Let(l) => {
                            self.check_let(l, env);
                            // `let d = 2` makes `d` non-zero for the rest of the
                            // block.
                            if let (PatKind::Bind(v), ExprKind::Lit(Lit::Int { value, .. })) =
                                (&l.pat.kind, &l.init.kind)
                            {
                                if *value != 0 {
                                    block_nonzero.push(v.name);
                                    if let Some(top) = self.nonzero_scope.last_mut() {
                                        top.push(v.name);
                                    }
                                }
                            }
                        }
                        StmtKind::Expr(ex) => {
                            let _ = self.check_expr(ex, None, env);
                        }
                    }
                }
                let _ = block_nonzero;
                let t = if let Some(tail) = tail {
                    self.check_expr(tail, expected, env)
                } else {
                    Type::unit()
                };
                self.pop_nonzero();
                env.pop();
                t
            }
            ExprKind::If {
                cond,
                then_b,
                else_b,
            } => {
                let ct = self.check_expr(cond, Some(&Type::bool()), env);
                if !types_eq(&ct, &Type::bool()) {
                    self.type_mismatch(cond.span, &Type::bool(), &ct, &env.def_id);
                }
                // The same implication holds for the taken branch of an `if`.
                let guarded = self
                    .guard_nonzero(cond)
                    .map(|n| vec![n])
                    .unwrap_or_default();
                self.push_nonzero(guarded);
                let th = self.check_expr(then_b, expected, env);
                self.pop_nonzero();
                if let Some(el) = else_b {
                    let hint = if matches!(th, Type::Never) {
                        expected
                    } else {
                        expected.or(Some(&th))
                    };
                    let et = self.check_expr(el, hint, env);
                    if !types_eq(&th, &et) {
                        self.type_mismatch(el.span, &th, &et, &env.def_id);
                    }
                    // `if c { raise e } else { 1 }` is an i32: the branch that
                    // leaves via control flow contributes no type.
                    join_types(th, et)
                } else {
                    Type::unit()
                }
            }
            ExprKind::Match { scrut, arms } => self.check_match(scrut, arms, expected, env, e.span),
            ExprKind::For { pat, iter, body } => {
                let it = self.check_expr(iter, None, env);
                let elem = self.iter_elem(&it);
                env.push();
                self.bind_pat(pat, &elem, false, env);
                env.loop_depth += 1;
                // A range starting at 1 or more never yields zero.
                let mut guarded = Vec::new();
                if let (PatKind::Bind(v), ExprKind::Call { callee, args }) = (&pat.kind, &iter.kind)
                {
                    let is_range = matches!(&callee.kind, ExprKind::Path(p)
                        if p.segs.len() == 1 && self.intern.get(p.segs[0].name) == "range");
                    let lo_ok = matches!(
                        args.first().map(|a| &a.kind),
                        Some(ExprKind::Lit(Lit::Int { value, .. })) if *value >= 1
                    );
                    if is_range && lo_ok {
                        guarded.push(v.name);
                    }
                }
                self.push_nonzero(guarded);
                let _ = self.check_expr(body, None, env);
                self.pop_nonzero();
                env.loop_depth -= 1;
                env.pop();
                // finite for over range/vec does not introduce diverge
                Type::unit()
            }
            ExprKind::Loop { body } => {
                env.inferred.insert(EffectAtom::Diverge);
                env.push();
                env.loop_depth += 1;
                let _ = self.check_expr(body, None, env);
                env.loop_depth -= 1;
                env.pop();
                expected.cloned().unwrap_or(Type::unit())
            }
            ExprKind::While { cond, body } => {
                // A `while` has no static bound, so it contributes `diverge` —
                // the same signal `loop` gives. Only `for` over a finite
                // sequence is bounded.
                env.inferred.insert(EffectAtom::Diverge);
                let ct = self.check_expr(cond, Some(&Type::bool()), env);
                if !types_eq(&ct, &Type::bool()) {
                    self.type_mismatch(cond.span, &Type::bool(), &ct, &env.def_id);
                }
                env.push();
                env.loop_depth += 1;
                // `while b != 0 { .. }` proves `b` non-zero for the body, which is
                // exactly the shape Euclid's algorithm is written in.
                let guarded = self
                    .guard_nonzero(cond)
                    .map(|n| vec![n])
                    .unwrap_or_default();
                self.push_nonzero(guarded);
                let _ = self.check_expr(body, None, env);
                self.pop_nonzero();
                env.loop_depth -= 1;
                env.pop();
                Type::unit()
            }
            ExprKind::Break | ExprKind::Continue => {
                if env.loop_depth == 0 {
                    self.err(
                        "E0110",
                        e.span,
                        format!(
                            "`{}` outside a loop",
                            if matches!(e.kind, ExprKind::Break) {
                                "break"
                            } else {
                                "continue"
                            }
                        ),
                    );
                }
                Type::Never
            }
            ExprKind::Cast { expr, ty } => {
                let from = self.check_expr(expr, None, env);
                let to = self.lower_type(ty, None);
                self.check_cast(&from, &to, e.span, &env.def_id);
                to
            }
            ExprKind::Let(l) => {
                self.check_let(l, env);
                Type::unit()
            }
            ExprKind::Lambda { params, ret, body } => {
                // Accept-and-elide: `move` on a closure is ignored ([T-3.3.1] A0107).
                self.diags.push(Diagnostic::warn(
                    "A0107",
                    e.span,
                    "`move` on a closure is ignored; Ax captures by value when needed (documented divergence from Rust)",
                ));
                env.push();
                let mut pts = Vec::new();
                for p in params {
                    let t = self.lower_type(&p.ty, None);
                    env.bind(p.name.name, t.clone(), false, None);
                    pts.push(t);
                }
                let rt = ret
                    .as_ref()
                    .map(|t| self.lower_type(t, None))
                    .or_else(|| {
                        expected.and_then(|e| match e {
                            Type::Fn { ret, .. } => Some((**ret).clone()),
                            _ => None,
                        })
                    })
                    .unwrap_or(Type::Hole);
                let got = self.check_expr(body, Some(&rt), env);
                env.pop();
                Type::Fn {
                    params: pts,
                    ret: Box::new(if rt.is_hole() { got } else { rt }),
                    effects: EffectSet::empty(),
                }
            }
            ExprKind::Record(fs) => {
                let mut fields = Vec::new();
                if let Some(Type::Named { def, .. }) = expected {
                    if let Some(td) = self.type_defs.get(def).cloned() {
                        if let TypeDefKind::Record(spec) = td.kind {
                            for (n, ty) in &spec {
                                if let Some((_, ex)) = fs.iter().find(|(i, _)| i.name == *n) {
                                    let g = self.check_expr(ex, Some(ty), env);
                                    if !types_eq(&g, ty) {
                                        self.type_mismatch(ex.span, ty, &g, &env.def_id);
                                    }
                                }
                                fields.push((*n, ty.clone()));
                            }
                            let t = Type::Named {
                                def: *def,
                                args: vec![],
                            };
                            return self.rec(e.id, t);
                        }
                    }
                }
                for (n, ex) in fs {
                    let t = self.check_expr(ex, None, env);
                    fields.push((n.name, t));
                }
                Type::Record(fields)
            }
            ExprKind::Variant { name, fields } => {
                self.check_variant(name, fields, expected, env, e.span)
            }
            ExprKind::Return(inner) => {
                let expected_ret = env.expected_ret.clone();
                let t = if let Some(x) = inner {
                    self.check_expr(x, Some(&expected_ret), env)
                } else {
                    Type::unit()
                };
                if !types_eq(&t, &expected_ret) {
                    self.type_mismatch(e.span, &expected_ret, &t, &env.def_id);
                }
                Type::Never // control flow leaves here; no value is produced
            }
            ExprKind::Raise(inner) => {
                let et = env
                    .declared
                    .err_type()
                    .cloned()
                    .or_else(|| expected.cloned());
                let got = self.check_expr(inner, et.as_ref(), env);
                // raise introduces err[E] — E is the type of the raised value
                // if it's a variant of the declared error, use declared.
                let err_ty = if let Some(d) = env.declared.err_type() {
                    if self.is_variant_of(&got, d) || types_eq(&got, d) {
                        d.clone()
                    } else {
                        got.clone()
                    }
                } else {
                    got.clone()
                };
                env.inferred.insert(EffectAtom::Err(err_ty));
                Type::Never // `raise` transfers control; it yields no value
            }
            ExprKind::Catch { expr, arms } => {
                let inner = self.check_expr(expr, expected, env);
                let (rest, err) = env.inferred.remove_err();
                env.inferred = rest;
                if let Some(et) = err {
                    self.caught.insert(e.id, et.clone());
                    env.push();
                    let mut result = inner.clone();
                    for a in arms {
                        env.push();
                        self.bind_pat(&a.pat, &et, false, env);
                        let hint = expected.or(Some(&inner));
                        let ht = self.check_expr(&a.body, hint, env);
                        result = ht;
                        env.pop();
                    }
                    // [T-1.2.4] / catalog E0204: a catch that drops a variant
                    // is a static error, same exhaustiveness rule as match.
                    self.check_catch_exhaustive(&et, arms, e.span, &env.def_id);
                    env.pop();
                    result
                } else {
                    inner
                }
            }
            ExprKind::Attempt(inner) => {
                let t = self.check_expr(inner, None, env);
                let (rest, err) = env.inferred.remove_err();
                env.inferred = rest;
                let caught = err.unwrap_or(Type::Named {
                    def: self.b.parse_error,
                    args: vec![],
                });
                self.caught.insert(e.id, caught.clone());
                builtins::result_type(&self.b, t, caught)
            }
            ExprKind::Try(inner) => {
                // v0.3: postfix `?` is Result propagation. The inner type is
                // Result[T, E]; the expression yields T and admits err[E].
                let t = self.check_expr(inner, None, env);
                match peel_result(&t, &self.b) {
                    Some((ok, err)) => {
                        env.inferred.insert(crate::effects::EffectAtom::Err(err));
                        ok
                    }
                    None => {
                        // Accept-and-elide: `?` on a non-Result is a no-op with
                        // an informational diagnostic, matching "never reject
                        // a generator for a missing conversion".
                        self.diags.push(crate::diag::Diagnostic::warn(
                            "A0109",
                            e.span,
                            "`?` on a non-Result is ignored; Ax already returns the value",
                        ));
                        t
                    }
                }
            }
            ExprKind::Interpolate { parts } => {
                for p in parts {
                    if let crate::ast::InterpPart::Expr(x) = p {
                        let t = self.check_expr(x, None, env);
                        if matches!(t, Type::Secret(_)) {
                            self.err(
                                "A5102",
                                x.span,
                                "Secret[T] cannot be interpolated / formatted",
                            );
                        }
                        if matches!(t, Type::Untrusted(_)) {
                            self.err(
                                "A5101",
                                x.span,
                                "Untrusted[T] cannot reach an f-string sink without declassify",
                            );
                        }
                    }
                }
                builtins::string_type(&self.b)
            }
            ExprKind::Region { name, body } => {
                let rid = RegionId {
                    name: name.name,
                    depth: env.next_depth,
                };
                env.next_depth += 1;
                env.region_stack.push(rid);
                env.push();
                // The region's name is also an allocator handle inside the body,
                // so `vec.new(r)` allocates in the arena. This is what makes a
                // region change how code allocates instead of only annotating
                // lifetimes.
                let alloc_ty = builtins::alloc_type(&self.b);
                env.bind(name.name, alloc_ty, false, Some(rid));
                let t = self.check_expr(body, expected, env);
                env.pop();
                env.region_stack.pop();
                if type_mentions_region(&t, rid) {
                    self.err(
                        "E0302",
                        e.span,
                        "borrow escapes its region (result must not mention the region)",
                    );
                }
                t
            }
            ExprKind::Par { bindings } => {
                // Mutable captures must be statically disjoint.
                let mut written: Vec<Symbol> = Vec::new();
                for l in bindings {
                    self.collect_mut_captures(&l.init, &mut written);
                    self.check_let(l, env);
                }
                // naive disjointness: duplicate write of same local is an error
                let mut seen = HashMap::new();
                for w in &written {
                    *seen.entry(*w).or_insert(0) += 1;
                }
                for (n, c) in seen {
                    if c > 1 {
                        self.err(
                            "E0600",
                            e.span,
                            format!(
                                "par mutable captures are not statically disjoint: {}",
                                self.intern.get(n)
                            ),
                        );
                    }
                }
                Type::unit()
            }
            ExprKind::Assign { lhs, rhs } => {
                let lt = self.check_expr(lhs, None, env);
                let rt = self.check_expr(rhs, Some(&lt), env);
                if !types_eq(&lt, &rt) {
                    self.type_mismatch(e.span, &lt, &rt, &env.def_id);
                }
                Type::unit()
            }
        };
        if let Some(exp) = expected {
            if !types_eq(&ty, exp) && !ty.is_error() && !exp.is_hole() && !ty.is_hole() {
                // Don't double-report for blocks/if that already compared.
                // Only coerce nothing — implicit conversion is forbidden.
                if matches!(
                    (&ty, exp),
                    (Type::Prim(a), Type::Prim(b)) if a != b && a.is_int() && b.is_int()
                ) {
                    let (from, to) = (ty.as_prim().unwrap(), exp.as_prim().unwrap());
                    let mut d = Diagnostic::error(
                        "E0108",
                        e.span,
                        format!(
                            "implicit numeric conversion {} -> {} is forbidden",
                            ty.display(self.intern),
                            exp.display(self.intern)
                        ),
                    )
                    .with_def(&env.def_id);
                    // The fix is a cast. It is semantics-preserving exactly when
                    // the conversion cannot change the value: same signedness and
                    // no narrowing, or unsigned into a strictly wider signed type.
                    let value_preserving = (from.is_signed_int() == to.is_signed_int()
                        && to.bit_width() >= from.bit_width())
                        || (from.is_unsigned_int()
                            && to.is_signed_int()
                            && to.bit_width() > from.bit_width());
                    d.fixes.push(Fix {
                        kind: "insert_cast".into(),
                        safety: if value_preserving {
                            FixSafety::SemanticsPreserving
                        } else {
                            FixSafety::BehaviorChanging
                        },
                        rank: 1,
                        // `$0` stands for the text the diagnostic's span covers.
                        patch: format!("($0) as {}", to.as_str()),
                        note: Some(if value_preserving {
                            format!("widen to {} — the value cannot change", to.as_str())
                        } else {
                            format!("cast to {} — this narrows and may wrap", to.as_str())
                        }),
                    });
                    self.diags.push(d);
                }
            }
        }
        self.rec(e.id, ty)
    }

    pub fn set_require_annotations(&mut self, require: bool) {
        self.require_annotations = require;
    }

    fn check_let(&mut self, l: &LetStmt, env: &mut Env) {
        if self.require_annotations && l.ty.is_none() {
            self.err(
                "E0101",
                l.span,
                "annotation checking requires a type on every let binding",
            );
        }
        let hint = l.ty.as_ref().map(|t| self.lower_type(t, None));
        let it = self.check_expr(&l.init, hint.as_ref(), env);
        let ty = hint.unwrap_or(it.clone());
        if !types_eq(&it, &ty) {
            self.type_mismatch(l.init.span, &ty, &it, &env.def_id);
        }
        // store rule: if init is a ref, destination is this let's location
        if let Type::Ref { region: r, .. } = &it {
            let loc = env.region_stack.last().copied().unwrap_or(RegionId {
                name: self.intern.intern("static"),
                depth: 0,
            });
            // store(&r T, l) legal iff r outlives l  (r.depth <= l.depth)
            if !r.outlives(loc) {
                self.err(
                    "E0300",
                    l.span,
                    format!(
                        "illegal region store: {} (depth {}) does not outlive location (depth {})",
                        self.intern.get(r.name),
                        r.depth,
                        loc.depth
                    ),
                );
            }
        }
        self.bind_pat(&l.pat, &ty, l.mutable, env);
    }

    fn bind_pat(&mut self, pat: &Pattern, ty: &Type, mutable: bool, env: &mut Env) {
        // Patterns share the expression id space; backends read the scrutinee
        // type of each sub-pattern from here when compiling matches.
        self.rec(pat.id, ty.clone());
        match &pat.kind {
            PatKind::Wild | PatKind::Lit(_) => {}
            PatKind::Bind(id) => {
                // A bare name is a variant pattern when it names a variant of
                // the scrutinee type, and a new binding otherwise.
                if let Some((parent, _)) = self.variants.get(&id.name).cloned() {
                    if scrutinee_def(ty) == Some(parent) {
                        self.pat_variant
                            .insert(pat.id, self.intern.get(id.name).to_string());
                        return;
                    }
                }
                env.bind(id.name, ty.clone(), mutable, None);
            }
            PatKind::Variant { name, fields } => {
                let field_tys = self.variant_fields(name.name, ty);
                for (n, p) in fields {
                    let ft = field_tys
                        .iter()
                        .find(|(fnm, _)| *fnm == n.name)
                        .map(|(_, t)| t.clone())
                        .or_else(|| {
                            // `Some(v)` is parsed as field `_0`; accept that as
                            // the first payload when the type names it `value`.
                            let nm = self.intern.get(n.name);
                            let idx = nm.strip_prefix('_')?.parse::<usize>().ok()?;
                            field_tys.get(idx).map(|(_, t)| t.clone())
                        })
                        .unwrap_or(Type::Error);
                    self.bind_pat(p, &ft, false, env);
                }
            }
            PatKind::Record(fs) => {
                for (n, p) in fs {
                    let ft = self.field_type(ty, n, n.span);
                    self.bind_pat(p, &ft, false, env);
                }
            }
            PatKind::Tuple(ps) => {
                if let Type::Tuple(ts) = ty {
                    for (p, t) in ps.iter().zip(ts.iter()) {
                        self.bind_pat(p, t, false, env);
                    }
                }
            }
        }
    }

    fn check_path(&mut self, p: &Path, expected: Option<&Type>, env: &mut Env) -> Type {
        if p.segs.is_empty() {
            return Type::Error;
        }
        let first = p.segs[0].name;
        if let Some(b) = env.lookup(first) {
            let mut ty = b.ty.clone();
            for seg in &p.segs[1..] {
                ty = self.field_type(&ty, seg, seg.span);
            }
            return ty;
        }
        // qualified builtin / fn
        let q = path_str(p, self.intern);
        // Nullary values in the prelude (not functions).
        if q == "test.alloc" {
            return builtins::alloc_type(&self.b);
        }
        if let Some(sig) = self.lookup_fn(&q) {
            return Type::Fn {
                params: sig.params.iter().map(|(_, t, _)| t.clone()).collect(),
                ret: Box::new(sig.ret.clone()),
                effects: sig.effects.clone(),
            };
        }
        if let Some(sig) = self.fns_by_sym.get(&first) {
            if p.segs.len() == 1 {
                return Type::Fn {
                    params: sig.params.iter().map(|(_, t, _)| t.clone()).collect(),
                    ret: Box::new(sig.ret.clone()),
                    effects: sig.effects.clone(),
                };
            }
        }
        // bare variant
        if p.segs.len() == 1 {
            if let Some((parent, fields)) = self.variants.get(&first).cloned() {
                if fields.is_empty() {
                    return Type::Named {
                        def: parent,
                        args: vec![],
                    };
                }
            }
        }
        // test.alloc is a value, not a call
        if let Some(exp) = expected {
            if matches!(exp, Type::Named { .. } | Type::Variant { .. }) {
                // might be a unit variant of expected
                return exp.clone();
            }
        }
        self.err("E0100", p.span, format!("unknown name `{q}`"));
        Type::Error
    }

    fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        expected: Option<&Type>,
        env: &mut Env,
        span: Span,
    ) -> Type {
        // Method-style: Field(base, name)(args) → inherent, UFCS, or qualified fn
        if let ExprKind::Field { base, field } = &callee.kind {
            if let Some(q) = self.qualified_name(callee) {
                if let Some(sig) = self.lookup_fn(&q).cloned() {
                    return self.apply_sig(&sig, args, expected, env, span, callee);
                }
            }
            let recv = self.check_expr(base, None, env);
            let fname = self.intern.get(field.name).to_string();
            let recv_place_mut = self.is_mut_place(base, env);
            if let Some(ty) = self.check_method_at(&recv, &fname, args, env, span, recv_place_mut) {
                return ty;
            }
            // Not a method: it may be a field holding a function, which is how
            // dictionary dispatch is written (`o.cmp(a, b)`).
            match self.try_field_type(&recv, field) {
                Some(Type::Fn {
                    params,
                    ret,
                    effects,
                }) => {
                    self.check_args(&params, args, env, span);
                    self.admit_effects(&effects, env, span);
                    return *ret;
                }
                Some(other) => {
                    self.err(
                        "E0106",
                        span,
                        format!(
                            "field `{fname}` is not callable: {}",
                            other.display(self.intern)
                        ),
                    );
                    return Type::Error;
                }
                None => {
                    if !recv.is_error() && !matches!(recv, Type::Fn { .. }) {
                        self.err(
                            "E0104",
                            span,
                            format!("no method `{fname}` on {}", recv.display(self.intern)),
                        );
                        return Type::Error;
                    }
                }
            }
        }

        // Direct path call
        if let ExprKind::Path(p) = &callee.kind {
            let q = path_str(p, self.intern);
            if let Some(sig) = self.lookup_fn(&q).cloned() {
                return self.apply_sig(&sig, args, expected, env, span, callee);
            }
            // Tree surface: `(xs.at i)` / `(m.insert k v)` is a dotted path
            // call, not a field. If the first segment is a local, the rest is
            // a method name. `int.div_trunc` is not a local, so it stays above.
            if p.segs.len() >= 2 && env.lookup(p.segs[0].name).is_some() {
                let fname = self.intern.get(p.segs.last().unwrap().name).to_string();
                let recv_path = Path {
                    segs: p.segs[..p.segs.len() - 1].to_vec(),
                    span: p.span,
                };
                let recv_expr = Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Path(recv_path),
                    span: p.span,
                };
                let recv = self.check_expr(&recv_expr, None, env);
                let place_mut = self.is_mut_place(&recv_expr, env);
                if let Some(ty) = self.check_method_at(&recv, &fname, args, env, span, place_mut) {
                    return ty;
                }
            }
            // first-seg local then field? already handled
            if p.segs.len() == 1 {
                if let Some(sig) = self.fns_by_sym.get(&p.segs[0].name).cloned() {
                    return self.apply_sig(&sig, args, expected, env, span, callee);
                }
            }
        }

        // Variant constructor applied positionally: `Some(3)`, `Err(Zero)`.
        if let ExprKind::Path(p) = &callee.kind {
            if p.segs.len() == 1 {
                if let Some(ty) = self.check_variant_call(p.segs[0].name, args, expected, env, span)
                {
                    return ty;
                }
            }
        }

        let ct = self.check_expr(callee, None, env);
        match ct {
            Type::Fn {
                params,
                ret,
                effects,
            } => {
                self.check_args(&params, args, env, span);
                self.admit_effects(&effects, env, span);
                *ret
            }
            Type::Error => Type::Error,
            other => {
                self.err(
                    "E0106",
                    span,
                    format!("not a function: {}", other.display(self.intern)),
                );
                Type::Error
            }
        }
    }

    fn apply_sig(
        &mut self,
        sig: &FnSig,
        args: &[Expr],
        expected: Option<&Type>,
        env: &mut Env,
        span: Span,
        _callee: &Expr,
    ) -> Type {
        let params: Vec<Type> = sig.params.iter().map(|(_, t, _)| t.clone()).collect();
        // defaultable dictionary params
        let needed = params.len();
        let mut given = args.len();
        if given < needed {
            for i in given..needed {
                if sig.params[i].2 {
                    // Resolve the unique visible dictionary and remember which
                    // one, so the interpreter and the backends can materialise
                    // the same vtable without re-running resolution.
                    let dt = sig.params[i].1.clone();
                    if let Some(idx) = self.resolve_default_dict(&dt, span) {
                        if let Some(call) = self.cur_call.last().copied() {
                            self.dict_defaults.insert((call, i as u32), idx);
                        }
                    }
                    given += 1;
                }
            }
        }
        if args.len() > needed {
            self.err(
                "E0103",
                span,
                format!("expected at most {needed} arguments, got {}", args.len()),
            );
        } else if given < needed {
            self.err(
                "E0103",
                span,
                format!("expected {needed} arguments, got {}", args.len()),
            );
        }
        // Resolve type parameters at the call site: first from the expected
        // type against the declared result, then from each argument. Without
        // this a generic result stays `Vec[T]` in the type table and every
        // consumer downstream is stuck with an unsubstituted parameter.
        let mut binds: HashMap<Symbol, Type> = HashMap::new();
        let mut effect_binds: HashMap<Symbol, EffectSet> = HashMap::new();
        if !sig.generics.is_empty() {
            if let Some(exp) = expected {
                crate::types::unify_param(&sig.ret, exp, &mut binds);
            }
        }
        for (i, a) in args.iter().enumerate() {
            if i < params.len() {
                let exp = if binds.is_empty() {
                    params[i].clone()
                } else {
                    crate::types::subst(&params[i], &binds)
                };
                // Special-case: &mut / & receivers accept owned values (auto-ref)
                let hint = match &exp {
                    Type::Ref { inner, .. } => (**inner).clone(),
                    other => other.clone(),
                };
                // An unresolved parameter is no hint at all; passing one as the
                // expectation would make literals default against `T`.
                let hint = if type_mentions_param(&hint) {
                    None
                } else {
                    Some(hint)
                };
                let got = self.check_expr(a, hint.as_ref(), env);
                if !sig.generics.is_empty() {
                    crate::types::unify_param(&params[i], &got, &mut binds);
                }
                bind_effect_params(&params[i], &got, &mut effect_binds);
                if !type_mentions_param(&exp) && !self.arg_compatible(&exp, &got) {
                    self.type_mismatch(a.span, &exp, &got, &env.def_id);
                }
            }
        }
        for g in &sig.generics {
            if !binds.contains_key(g) {
                self.err(
                    "E0109",
                    span,
                    format!(
                        "cannot infer `{}` for `{}`; annotate the result type",
                        self.intern.get(*g),
                        self.intern.get(sig.name)
                    ),
                );
            }
        }
        let mut effects = EffectSet::new();
        for effect in &sig.effects.atoms {
            if let EffectAtom::Var(symbol) = effect {
                if let Some(bound) = effect_binds.get(symbol) {
                    effects = effects.union(bound);
                } else {
                    effects.insert(effect.clone());
                }
            } else {
                effects.insert(effect.clone());
            }
        }
        // inject err[F] → err[E] if needed
        if let Some(callee_err) = effects.err_type().cloned() {
            self.inject_or_admit(&callee_err, env, span);
            let (rest, _) = effects.remove_err();
            effects = rest;
        }
        self.admit_effects(&effects, env, span);
        if binds.is_empty() {
            sig.ret.clone()
        } else {
            crate::types::subst(&sig.ret, &binds)
        }
    }

    fn arg_compatible(&self, exp: &Type, got: &Type) -> bool {
        if types_eq(exp, got) {
            return true;
        }
        // Untrusted[T] flows through pure computation (spec §4.4). Sinks
        // reject it separately (A5101).
        if let Type::Untrusted(inner) = got {
            if self.arg_compatible(exp, inner) {
                return true;
            }
        }
        if let Type::Untrusted(inner) = exp {
            if self.arg_compatible(inner, got) {
                return true;
            }
        }
        // auto-ref: expected &T or &mut T, got T
        if let Type::Ref { inner, .. } = exp {
            if types_eq(inner, got) {
                return true;
            }
            // A Vec is a slice plus capacity and an allocator, and its layout
            // starts with the same {data, len}, so `&mut Vec[T]` is accepted
            // where `&mut [T]` is expected — with the element types checked.
            if let Type::Named {
                def,
                args: exp_args,
            } = inner.as_ref()
            {
                if self.intern.get(*def) == "slice" {
                    let elem_ok = |gd_args: &Vec<Type>| match (exp_args.first(), gd_args.first()) {
                        (Some(a), Some(b)) => types_eq(a, b),
                        _ => false,
                    };
                    if let Type::Named { def: gd, args } = got {
                        let n = self.intern.get(*gd);
                        if (n == "Vec" || n == "slice") && elem_ok(args) {
                            return true;
                        }
                    }
                    if let Type::Ref { inner: gi, .. } = got {
                        if let Type::Named { def: gd, args } = gi.as_ref() {
                            let n = self.intern.get(*gd);
                            if (n == "Vec" || n == "slice") && elem_ok(args) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        // String / str / argv-str accepted as &str
        if let Type::Ref { inner, .. } = exp {
            if let Type::Named { def, .. } = inner.as_ref() {
                if self.intern.get(*def) == "str" {
                    if let Type::Named { def: gd, .. } = got {
                        let n = self.intern.get(*gd);
                        if n == "String" || n == "str" {
                            return true;
                        }
                    }
                }
            }
        }
        // Function types: compare shape; generic params are wildcards.
        if let (
            Type::Fn {
                params: ep,
                ret: er,
                ..
            },
            Type::Fn {
                params: gp,
                ret: gr,
                ..
            },
        ) = (exp, got)
        {
            if ep.len() == gp.len()
                && ep.iter().zip(gp.iter()).all(|(a, b)| {
                    types_eq(a, b) || matches!(a, Type::Param(_)) || matches!(b, Type::Param(_))
                })
                && (types_eq(er, gr)
                    || matches!(er.as_ref(), Type::Param(_))
                    || matches!(gr.as_ref(), Type::Param(_)))
            {
                return true;
            }
        }
        // A function is accepted where Ord[T] is expected (explicit cmp override).
        if let Type::Named { def, .. } = exp {
            if self.intern.get(*def) == "Ord" && matches!(got, Type::Fn { .. }) {
                return true;
            }
        }
        // test.read_cap overlay: any record is fine
        if let Type::Record(exp_fs) = exp {
            if exp_fs.is_empty() && matches!(got, Type::Record(_)) {
                return true;
            }
        }
        // generic params in builtins are permissive
        if matches!(exp, Type::Param(_)) || matches!(got, Type::Param(_)) {
            return true;
        }
        // Range / Vec accepted as Seq
        if let Type::Param(s) = exp {
            if self.intern.get(*s) == "Seq" {
                return true;
            }
        }
        false
    }

    fn check_args(&mut self, params: &[Type], args: &[Expr], env: &mut Env, span: Span) {
        if params.len() != args.len() {
            self.err(
                "E0103",
                span,
                format!("expected {} arguments, got {}", params.len(), args.len()),
            );
        }
        for (p, a) in params.iter().zip(args.iter()) {
            let g = self.check_expr(a, Some(p), env);
            if !self.arg_compatible(p, &g) {
                self.type_mismatch(a.span, p, &g, &env.def_id);
            }
        }
    }

    /// Type a method call, dispatching on the receiver's type.
    ///
    /// The receiver decides which methods exist: `xs.len()` is a `Vec`/`slice`/
    /// `String` operation, not something every value has. An unknown method
    /// returns `None` so the caller can report it against the receiver type.
    /// Is `e` a place whose contents may be mutated — a `let mut` local, or a
    /// field/element of one? `xs.push(v)` on a `let mut xs` is allowed without
    /// writing `&mut xs`, exactly as in Rust.
    fn is_mut_place(&self, e: &Expr, env: &Env) -> bool {
        match &e.kind {
            ExprKind::Path(p) => env
                .lookup(p.segs[0].name)
                .map(|b| b.mutable || matches!(b.ty, Type::Ref { mutable: true, .. }))
                .unwrap_or(false),
            ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => {
                self.is_mut_place(base, env)
            }
            ExprKind::Unary {
                op: UnOp::RefMut | UnOp::Deref,
                ..
            } => true,
            _ => false,
        }
    }

    fn check_method_at(
        &mut self,
        recv: &Type,
        name: &str,
        args: &[Expr],
        env: &mut Env,
        span: Span,
        place_mut: bool,
    ) -> Option<Type> {
        // Peel references: `xs.len()` works through `&Vec[T]` and `&mut Vec[T]`.
        let (bare, recv_mut) = match recv {
            Type::Ref { inner, mutable, .. } => ((**inner).clone(), *mutable),
            Type::Own(inner) | Type::Untrusted(inner) | Type::Secret(inner) => {
                ((**inner).clone(), true)
            }
            other => (other.clone(), place_mut),
        };
        // Accept-and-elide: `.clone()` is identity ([T-3.3.1] A0103).
        if name == "clone" && args.is_empty() {
            self.diags.push(Diagnostic::warn(
                "A0103",
                span,
                "`.clone()` is elided; Ax copies on conflict and reports P1010 (Rust would require an explicit Clone impl)",
            ));
            return Some(recv.clone());
        }
        let kind = match &bare {
            Type::Named { def, .. } => {
                let n = self.intern.get(*def);
                match n {
                    "Vec" => Some(SeqKind::Vec),
                    "slice" => Some(SeqKind::Slice),
                    "String" | "str" => Some(SeqKind::Str),
                    "Map" | "SortedMap" => Some(SeqKind::Map),
                    _ => None,
                }
            }
            _ => None,
        };
        let Some(kind) = kind else {
            // Not a container: check the arguments so their nodes get types, then
            // let the caller report an unknown method.
            for a in args {
                let _ = self.check_expr(a, None, env);
            }
            return None;
        };

        let elem = match kind {
            SeqKind::Str => Type::Prim(Prim::U8),
            SeqKind::Map => self.map_val_type(&bare),
            _ => self.elem_type(&bare, span),
        };
        let map_key = match kind {
            SeqKind::Map => self.map_key_type(&bare),
            _ => Type::usz(),
        };
        let arity = |n: usize, this: &mut Self| {
            if args.len() != n {
                this.err(
                    "E0107",
                    span,
                    format!("`{name}` takes {n} argument(s), given {}", args.len()),
                );
            }
        };
        match name {
            "len" => {
                arity(0, self);
                Some(Type::usz())
            }
            // `at` is bounds-checked always and aborts out of range. The abort is
            // language-defined and does not appear in the row (§3.3).
            "at" => {
                arity(1, self);
                if let Some(a) = args.first() {
                    let t = self.check_expr(a, Some(&Type::usz()), env);
                    self.want_index(&t, a.span, env);
                }
                Some(elem)
            }
            "get" => {
                arity(1, self);
                if let Some(a) = args.first() {
                    let hint = if kind == SeqKind::Map {
                        Some(&map_key)
                    } else {
                        Some(&Type::usz())
                    };
                    let t = self.check_expr(a, hint, env);
                    if kind != SeqKind::Map {
                        self.want_index(&t, a.span, env);
                    }
                }
                Some(builtins::option_type(&self.b, elem))
            }
            "add" => {
                arity(2, self);
                if kind != SeqKind::Map {
                    self.err(
                        "E0108",
                        span,
                        format!("`add` needs a Map, got {}", recv.display(self.intern)),
                    );
                }
                if !recv_mut {
                    self.err("E0300", span, "`add` needs `&mut`");
                }
                if let Some(k) = args.first() {
                    let t = self.check_expr(k, Some(&map_key), env);
                    if !types_eq(&t, &map_key) {
                        self.type_mismatch(k.span, &map_key, &t, &env.def_id);
                    }
                }
                if let Some(v) = args.get(1) {
                    let t = self.check_expr(v, Some(&elem), env);
                    if !types_eq(&t, &elem) {
                        self.type_mismatch(v.span, &elem, &t, &env.def_id);
                    }
                }
                env.inferred
                    .insert(EffectAtom::Alloc(self.intern.intern("a")));
                Some(Type::unit())
            }
            "insert" | "put" => {
                arity(2, self);
                if kind != SeqKind::Map {
                    self.err(
                        "E0108",
                        span,
                        format!("`{name}` needs a Map, got {}", recv.display(self.intern)),
                    );
                }
                if !recv_mut {
                    self.err("E0300", span, format!("`{name}` needs `&mut`"));
                }
                if let Some(k) = args.first() {
                    let t = self.check_expr(k, Some(&map_key), env);
                    if !types_eq(&t, &map_key) {
                        self.type_mismatch(k.span, &map_key, &t, &env.def_id);
                    }
                }
                if let Some(v) = args.get(1) {
                    let t = self.check_expr(v, Some(&elem), env);
                    if !types_eq(&t, &elem) {
                        self.type_mismatch(v.span, &elem, &t, &env.def_id);
                    }
                }
                env.inferred
                    .insert(EffectAtom::Alloc(self.intern.intern("a")));
                Some(Type::unit())
            }
            "contains" => {
                arity(1, self);
                if let Some(a) = args.first() {
                    let _ = self.check_expr(a, Some(&map_key), env);
                }
                Some(Type::bool())
            }
            "push" => {
                arity(1, self);
                if kind != SeqKind::Vec {
                    self.err(
                        "E0108",
                        span,
                        format!("`push` needs a Vec, got {}", recv.display(self.intern)),
                    );
                }
                if !recv_mut {
                    self.err("E0300", span, "`push` needs `&mut`");
                }
                if let Some(a) = args.first() {
                    let t = self.check_expr(a, Some(&elem), env);
                    if !types_eq(&t, &elem) {
                        self.type_mismatch(a.span, &elem, &t, &env.def_id);
                    }
                }
                // Growth allocates through the Vec's own allocator handle.
                env.inferred
                    .insert(EffectAtom::Alloc(self.intern.intern("a")));
                Some(Type::unit())
            }
            "reserve" => {
                arity(1, self);
                if kind != SeqKind::Vec {
                    self.err(
                        "E0108",
                        span,
                        format!("`reserve` needs a Vec, got {}", recv.display(self.intern)),
                    );
                }
                if !recv_mut {
                    self.err("E0300", span, "`reserve` needs `&mut`");
                }
                if let Some(a) = args.first() {
                    let t = self.check_expr(a, Some(&Type::usz()), env);
                    self.want_index(&t, a.span, env);
                }
                env.inferred
                    .insert(EffectAtom::Alloc(self.intern.intern("a")));
                Some(Type::unit())
            }
            "eq" => {
                arity(1, self);
                if kind != SeqKind::Vec && kind != SeqKind::Str {
                    self.err(
                        "E0108",
                        span,
                        format!(
                            "`eq` needs a Vec or String, got {}",
                            recv.display(self.intern)
                        ),
                    );
                }
                if let Some(a) = args.first() {
                    let t = self.check_expr(a, Some(&bare), env);
                    if !types_eq(&t, &bare) && !matches!(t, Type::Ref { .. }) {
                        self.type_mismatch(a.span, &bare, &t, &env.def_id);
                    }
                }
                Some(Type::bool())
            }
            "set" => {
                arity(2, self);
                if !recv_mut {
                    self.err("E0300", span, "`set` needs `&mut`");
                }
                if let Some(a) = args.first() {
                    let t = self.check_expr(a, Some(&Type::usz()), env);
                    self.want_index(&t, a.span, env);
                }
                if let Some(a) = args.get(1) {
                    let t = self.check_expr(a, Some(&elem), env);
                    if !types_eq(&t, &elem) {
                        self.type_mismatch(a.span, &elem, &t, &env.def_id);
                    }
                }
                Some(Type::unit())
            }
            _ => {
                for a in args {
                    let _ = self.check_expr(a, None, env);
                }
                None
            }
        }
    }

    /// Can this divisor never be zero?
    ///
    /// Deliberately narrow, because being wrong here would turn a recoverable
    /// error into an abort: a non-zero literal, or a local the pre-pass proved
    /// non-zero.
    /// `None` if the divisor may be zero; `Some(true)` if it is non-zero
    /// unconditionally; `Some(false)` if the proof assumes no wrap-around.
    fn divisor_strength(&self, rhs: &Expr) -> Option<bool> {
        match &rhs.kind {
            ExprKind::Lit(Lit::Int { value, .. }) => (*value != 0).then_some(true),
            ExprKind::Path(p) if p.segs.len() == 1 => {
                let name = p.segs[0].name;
                if !self.nonzero_scope.iter().any(|s| s.contains(&name)) {
                    return None;
                }
                Some(!self.wrap_assumed.contains(&name))
            }
            _ => None,
        }
    }

    /// Names assigned `x = x + <positive literal>` anywhere in the body.
    fn collect_wrap_assumed(&mut self, e: &Expr) {
        if let ExprKind::Assign { lhs, rhs } = &e.kind {
            if let ExprKind::Path(p) = &lhs.kind {
                if p.segs.len() == 1 {
                    let name = p.segs[0].name;
                    let increments = matches!(&rhs.kind, ExprKind::Binary { op: BinOp::Add, lhs: l, .. }
                        if matches!(&l.kind, ExprKind::Path(q)
                            if q.segs.len() == 1 && q.segs[0].name == name));
                    if increments {
                        self.wrap_assumed.insert(name);
                    }
                }
            }
        }
        let mut f = |c: &Expr| self.collect_wrap_assumed(c);
        for_each_child(e, &mut f);
    }

    /// The name a condition proves non-zero, if it is of the form `x != 0`.
    fn guard_nonzero(&self, cond: &Expr) -> Option<Symbol> {
        let ExprKind::Binary {
            op: BinOp::Ne,
            lhs,
            rhs,
        } = &cond.kind
        else {
            return None;
        };
        let zero = |e: &Expr| matches!(&e.kind, ExprKind::Lit(Lit::Int { value: 0, .. }));
        let name = |e: &Expr| match &e.kind {
            ExprKind::Path(p) if p.segs.len() == 1 => Some(p.segs[0].name),
            _ => None,
        };
        if zero(rhs) {
            name(lhs)
        } else if zero(lhs) {
            name(rhs)
        } else {
            None
        }
    }

    /// Drop non-zero facts this statement could falsify.
    fn invalidate_nonzero(&mut self, st: &StmtKind) {
        let e = match st {
            StmtKind::Let(l) => &l.init,
            StmtKind::Expr(x) => x,
        };
        let tracked: Vec<Symbol> = self
            .nonzero_scope
            .iter()
            .flat_map(|s| s.iter().copied())
            .collect();
        for name in tracked {
            if self.may_zero_assign(e, name) {
                for scope in self.nonzero_scope.iter_mut() {
                    scope.retain(|n| *n != name);
                }
            }
        }
    }

    /// Does `e` contain an assignment to `name` that could make it zero?
    fn may_zero_assign(&self, e: &Expr, name: Symbol) -> bool {
        let mut found = false;
        self.walk_zero_assign(e, name, &mut found);
        found
    }

    fn walk_zero_assign(&self, e: &Expr, name: Symbol, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprKind::Assign { lhs, rhs } = &e.kind {
            if let ExprKind::Path(p) = &lhs.kind {
                if p.segs.len() == 1
                    && p.segs[0].name == name
                    && !self.assignment_keeps_nonzero(name, rhs)
                {
                    *found = true;
                    return;
                }
            }
        }
        let mut f = |c: &Expr| self.walk_zero_assign(c, name, found);
        for_each_child(e, &mut f);
    }

    fn push_nonzero(&mut self, names: Vec<Symbol>) {
        self.nonzero_scope.push(names);
    }

    fn pop_nonzero(&mut self) {
        self.nonzero_scope.pop();
    }

    /// Does assigning `rhs` to `name` preserve "never zero"?
    fn assignment_keeps_nonzero(&self, name: Symbol, rhs: &Expr) -> bool {
        match &rhs.kind {
            ExprKind::Lit(Lit::Int { value, .. }) => *value != 0,
            // `d = d + k` for a positive literal k.
            ExprKind::Binary {
                op: BinOp::Add,
                lhs,
                rhs: r,
            } => {
                let same = matches!(&lhs.kind, ExprKind::Path(p)
                    if p.segs.len() == 1 && p.segs[0].name == name);
                let positive =
                    matches!(&r.kind, ExprKind::Lit(Lit::Int { value, .. }) if *value > 0);
                same && positive
            }
            _ => false,
        }
    }

    /// `as` converts between numeric types and nothing else.
    ///
    /// Narrowing an integer wraps, float-to-int saturates, and int-to-float
    /// rounds to nearest — the same choices Rust's `as` makes, so a reader
    /// coming from Rust is not surprised. Conversions that would be
    /// representation puns (pointer/bool/aggregate) are rejected outright.
    fn check_cast(&mut self, from: &Type, to: &Type, span: Span, def_id: &str) {
        let _ = def_id;
        let (Some(f), Some(t)) = (from.as_prim(), to.as_prim()) else {
            if !from.is_error() && !to.is_error() {
                self.err(
                    "E0111",
                    span,
                    format!(
                        "cannot cast {} to {}: `as` converts numbers, not representations",
                        from.display(self.intern),
                        to.display(self.intern)
                    ),
                );
            }
            return;
        };
        let numeric = |p: Prim| p.is_int() || p.is_float();
        if !numeric(f) || !numeric(t) {
            self.err(
                "E0111",
                span,
                format!(
                    "cannot cast {} to {}: `as` converts numbers only",
                    from.display(self.intern),
                    to.display(self.intern)
                ),
            );
        }
    }

    fn want_index(&mut self, got: &Type, span: Span, env: &mut Env) {
        if let Some(p) = got.as_prim() {
            if !p.is_int() {
                self.type_mismatch(span, &Type::usz(), got, &env.def_id);
            }
        }
    }

    fn check_unary(
        &mut self,
        op: UnOp,
        expr: &Expr,
        expected: Option<&Type>,
        env: &mut Env,
        span: Span,
    ) -> Type {
        match op {
            UnOp::Not => {
                let t = self.check_expr(expr, Some(&Type::bool()), env);
                if !types_eq(&t, &Type::bool()) {
                    self.type_mismatch(span, &Type::bool(), &t, &env.def_id);
                }
                Type::bool()
            }
            UnOp::Neg => {
                let t = self.check_expr(expr, expected, env);
                t
            }
            UnOp::BitNot => {
                let t = self.check_expr(expr, expected, env);
                if let Some(p) = t.as_prim() {
                    if !p.is_int() {
                        self.err(
                            "E0108",
                            span,
                            format!("`~` needs an integer, got {}", t.display(self.intern)),
                        );
                    }
                }
                t
            }
            UnOp::Ref | UnOp::RefMut => {
                let t = self.check_expr(expr, None, env);
                if op == UnOp::Ref {
                    self.diags.push(Diagnostic::warn(
                        "A0101",
                        span,
                        "`&` is elided; Ax treats references as hints (Rust would treat this as a borrow)",
                    ));
                }
                if op == UnOp::RefMut {
                    if let ExprKind::Path(p) = &expr.kind {
                        if let Some(first) = p.segs.first() {
                            // Accept-and-elide ([R-1.1.2], [R-1.2.2]): a second
                            // `&mut` or a `&mut` of an immutable binding is a
                            // Rust borrow-check error (E0499/E0502). Ax must
                            // compile, copy-on-conflict, and report the cost
                            // (A0101), never reject.
                            if let Some(b) = env.lookup(first.name) {
                                if !b.mutable {
                                    self.diags.push(Diagnostic::warn(
                                        "A0101",
                                        span,
                                        "`&mut` of an immutable binding is elided; Ax treats `&`/`&mut` as hints (Rust would reject this as E0502)",
                                    ));
                                }
                            }
                            if env.mut_borrowed.contains(&first.name) {
                                self.diags.push(Diagnostic::warn(
                                    "A0101",
                                    span,
                                    "second `&mut` is elided; Ax copies on conflict (Rust would reject this as E0499)",
                                ));
                            }
                            env.mut_borrowed.push(first.name);
                        }
                    }
                }
                let r = env.region_stack.last().copied().unwrap_or(RegionId {
                    name: self.intern.intern("static"),
                    depth: 0,
                });
                Type::Ref {
                    region: r,
                    mutable: op == UnOp::RefMut,
                    inner: Box::new(t),
                }
            }
            UnOp::Deref => {
                let t = self.check_expr(expr, None, env);
                match t {
                    Type::Ref { inner, .. }
                    | Type::Own(inner)
                    | Type::Untrusted(inner)
                    | Type::Secret(inner) => *inner,
                    other => {
                        self.err(
                            "E0101",
                            span,
                            format!("cannot dereference {}", other.display(self.intern)),
                        );
                        Type::Error
                    }
                }
            }
        }
    }

    fn check_binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        expected: Option<&Type>,
        env: &mut Env,
        span: Span,
    ) -> Type {
        let lt = self.check_expr(
            lhs,
            expected.filter(|_| {
                matches!(
                    op,
                    BinOp::Add
                        | BinOp::Sub
                        | BinOp::Mul
                        | BinOp::Div
                        | BinOp::Rem
                        | BinOp::BitAnd
                        | BinOp::BitOr
                        | BinOp::BitXor
                        | BinOp::Shl
                        | BinOp::Shr
                )
            }),
            env,
        );
        // A shift's count is independent of the value's type: `x << 3u8` is
        // fine, so the right side is not checked against the left's type.
        let rt = if matches!(op, BinOp::Shl | BinOp::Shr) {
            self.check_expr(rhs, None, env)
        } else {
            self.check_expr(rhs, Some(&lt), env)
        };
        match op {
            BinOp::And | BinOp::Or => Type::bool(),
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if !types_eq(&lt, &rt) && !lt.is_error() && !rt.is_error() {
                    self.type_mismatch(span, &lt, &rt, &env.def_id);
                }
                Type::bool()
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                if !types_eq(&lt, &rt) {
                    self.type_mismatch(span, &lt, &rt, &env.def_id);
                }
                lt
            }
            // `& | ^` work on integers of the same width, and on bool as logical
            // operators without short-circuiting.
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                if !types_eq(&lt, &rt) {
                    self.type_mismatch(span, &lt, &rt, &env.def_id);
                }
                if let Some(p) = lt.as_prim() {
                    if !p.is_int() && p != Prim::Bool {
                        self.err(
                            "E0108",
                            span,
                            format!(
                                "`{}` needs integers or bools, got {}",
                                op.as_str(),
                                lt.display(self.intern)
                            ),
                        );
                    }
                }
                lt
            }
            BinOp::Shl | BinOp::Shr => {
                for (t, e) in [(&lt, lhs), (&rt, rhs)] {
                    if let Some(p) = t.as_prim() {
                        if !p.is_int() {
                            self.err(
                                "E0108",
                                e.span,
                                format!(
                                    "`{}` needs integers, got {}",
                                    op.as_str(),
                                    t.display(self.intern)
                                ),
                            );
                        }
                    }
                }
                lt
            }
            BinOp::Div | BinOp::Rem => {
                // `/` and `%` on integers raise `DivError` — unless the divisor
                // provably cannot be zero, in which case the row would be
                // claiming an error that cannot occur. That claim is not free: it
                // forces every caller through the fallible ABI and puts
                // `err[DivError]` in every signature that touches `%`.
                if let Some(p) = lt.as_prim() {
                    if let (true, Some(unconditional)) = (p.is_int(), self.divisor_strength(rhs)) {
                        self.nonzero_div.insert(rhs.id);
                        if !unconditional {
                            self.nonzero_div_needs_guard.insert(rhs.id);
                        }
                    } else if p.is_int() {
                        env.inferred.insert(EffectAtom::Err(Type::Named {
                            def: self.b.div_error,
                            args: vec![],
                        }));
                    }
                }
                if !types_eq(&lt, &rt) {
                    self.type_mismatch(span, &lt, &rt, &env.def_id);
                }
                lt
            }
        }
    }

    fn check_match(
        &mut self,
        scrut: &Expr,
        arms: &[Arm],
        expected: Option<&Type>,
        env: &mut Env,
        _span: Span,
    ) -> Type {
        let st = self.check_expr(scrut, None, env);
        self.check_exhaustive(&st, arms, _span, &env.def_id);
        let mut result: Option<Type> = expected.cloned();
        for a in arms {
            env.push();
            self.bind_pat(&a.pat, &st, false, env);
            let t = self.check_expr(&a.body, expected, env);
            env.pop();
            result = Some(match result {
                // Arms that diverge (`raise`, `return`) do not constrain the
                // match's type; any arm that yields a value does.
                Some(prev) => join_types(prev, t),
                None => t,
            });
        }
        result.unwrap_or(Type::unit())
    }

    /// Reject a `match` that does not cover every case of a variant scrutinee.
    ///
    /// A missing case is otherwise a runtime abort, which for an agent means one
    /// more compile-run cycle to discover something the types already knew. Only
    /// variant scrutinees are checked: integers and strings have no finite case
    fn check_catch_exhaustive(&mut self, err_ty: &Type, arms: &[Arm], span: Span, def_id: &str) {
        // Same coverage walk as match, but the catalog code is E0204.
        let bare = match err_ty {
            Type::Ref { inner, .. }
            | Type::Own(inner)
            | Type::Untrusted(inner)
            | Type::Secret(inner) => (**inner).clone(),
            other => other.clone(),
        };
        let Type::Named { def, .. } = &bare else {
            return;
        };
        let Some(td) = self.type_defs.get(def).cloned() else {
            return;
        };
        let TypeDefKind::Variants(cases) = &td.kind else {
            return;
        };
        let mut covered: Vec<Symbol> = Vec::new();
        for a in arms {
            match &a.pat.kind {
                PatKind::Wild => return,
                PatKind::Bind(id) => {
                    if self.variants.contains_key(&id.name) {
                        covered.push(id.name);
                    } else {
                        return;
                    }
                }
                PatKind::Variant { name, .. } => covered.push(name.name),
                _ => return,
            }
        }
        let missing: Vec<String> = cases
            .iter()
            .filter(|(n, _)| !covered.contains(n))
            .map(|(n, _)| self.intern.get(*n).to_string())
            .collect();
        if !missing.is_empty() {
            self.err(
                "E0204",
                span,
                format!("catch not exhaustive: missing {}", missing.join(", ")),
            );
        }
        let _ = def_id;
    }

    /// list, so they still require a `_` arm to be total.
    fn check_exhaustive(&mut self, scrut: &Type, arms: &[Arm], span: Span, def_id: &str) {
        let bare = match scrut {
            Type::Ref { inner, .. }
            | Type::Own(inner)
            | Type::Untrusted(inner)
            | Type::Secret(inner) => (**inner).clone(),
            other => other.clone(),
        };
        let Type::Named { def, .. } = &bare else {
            return;
        };
        let Some(td) = self.type_defs.get(def).cloned() else {
            return;
        };
        let TypeDefKind::Variants(cases) = &td.kind else {
            return;
        };
        let mut covered: Vec<Symbol> = Vec::new();
        for a in arms {
            match &a.pat.kind {
                // A wildcard or a plain binding covers everything that is left.
                PatKind::Wild => return,
                PatKind::Bind(id) => {
                    if self.variants.contains_key(&id.name) {
                        covered.push(id.name);
                    } else {
                        return;
                    }
                }
                PatKind::Variant { name, .. } => covered.push(name.name),
                _ => return,
            }
        }
        let missing: Vec<String> = cases
            .iter()
            .filter(|(n, _)| !covered.contains(n))
            .map(|(n, _)| self.intern.get(*n).to_string())
            .collect();
        if !missing.is_empty() {
            let mut d = Diagnostic::error(
                "E0112",
                span,
                format!(
                    "non-exhaustive match on {}: missing {}",
                    bare.display(self.intern),
                    missing.join(", ")
                ),
            )
            .with_def(def_id);
            d.fixes.push(Fix {
                kind: "add_arms".into(),
                safety: FixSafety::InterfaceWidening,
                rank: 1,
                patch: missing
                    .iter()
                    .map(|m| format!("{m} => ?;"))
                    .collect::<Vec<_>>()
                    .join(" "),
                note: Some("add an arm per missing case, or a `_` arm".into()),
            });
            self.diags.push(d);
        }
    }

    /// Type a positional variant construction, or `None` if the name is not a
    /// variant. Payload fields are matched by declaration order, which is the
    /// same rule the parser uses for `Some(x)` patterns.
    fn check_variant_call(
        &mut self,
        name: Symbol,
        args: &[Expr],
        expected: Option<&Type>,
        env: &mut Env,
        span: Span,
    ) -> Option<Type> {
        // Resolve the variant's parent, preferring the expected type so that
        // generic sums (`Option[T]`) instantiate against the annotation.
        let (parent, spec) = match self.variants.get(&name).cloned() {
            Some(v) => v,
            None => return None,
        };
        let subst_map: HashMap<Symbol, Type> = match expected {
            Some(Type::Named { def, args: targs }) if *def == parent => self
                .type_defs
                .get(&parent)
                .map(|td| {
                    td.generics
                        .iter()
                        .copied()
                        .zip(targs.iter().cloned())
                        .collect()
                })
                .unwrap_or_default(),
            _ => HashMap::new(),
        };
        if args.len() != spec.len() {
            self.err(
                "E0107",
                span,
                format!(
                    "`{}` takes {} payload field(s), given {}",
                    self.intern.get(name),
                    spec.len(),
                    args.len()
                ),
            );
        }
        // Infer any type parameter the annotation did not pin from the args.
        let mut inferred = subst_map.clone();
        for ((_, ft), a) in spec.iter().zip(args) {
            let want = crate::types::subst(ft, &inferred);
            let hint = if matches!(want, Type::Param(_)) {
                None
            } else {
                Some(want.clone())
            };
            let got = self.check_expr(a, hint.as_ref(), env);
            if let Type::Param(pv) = ft {
                inferred.entry(*pv).or_insert(got);
            } else if !types_eq(&got, &want) {
                self.type_mismatch(a.span, &want, &got, &env.def_id);
            }
        }
        let targs: Vec<Type> = self
            .type_defs
            .get(&parent)
            .map(|td| {
                td.generics
                    .iter()
                    .map(|g| inferred.get(g).cloned().unwrap_or(Type::Hole))
                    .collect()
            })
            .unwrap_or_default();
        Some(Type::Named {
            def: parent,
            args: targs,
        })
    }

    fn check_variant(
        &mut self,
        name: &Ident,
        fields: &[(Ident, Expr)],
        expected: Option<&Type>,
        env: &mut Env,
        span: Span,
    ) -> Type {
        // `P { x: 1 }` where `P` is a record type, not a variant. The parser
        // cannot tell the two apart, so the checker resolves it by name.
        if let Some(td) = self.type_defs.get(&name.name).cloned() {
            if let TypeDefKind::Record(spec) = &td.kind {
                for (n, e) in fields {
                    let ft = spec
                        .iter()
                        .find(|(fnm, _)| *fnm == n.name)
                        .map(|(_, t)| t.clone());
                    match ft {
                        Some(ft) => {
                            let g = self.check_expr(e, Some(&ft), env);
                            if !types_eq(&g, &ft) {
                                self.type_mismatch(e.span, &ft, &g, &env.def_id);
                            }
                        }
                        None => {
                            let _ = self.check_expr(e, None, env);
                            self.err(
                                "E0104",
                                n.span,
                                format!(
                                    "`{}` has no field `{}`",
                                    self.intern.get(name.name),
                                    self.intern.get(n.name)
                                ),
                            );
                        }
                    }
                }
                for (fname, _) in spec {
                    if !fields.iter().any(|(n, _)| n.name == *fname) {
                        self.err(
                            "E0107",
                            span,
                            format!(
                                "missing field `{}` in `{}` literal",
                                self.intern.get(*fname),
                                self.intern.get(name.name)
                            ),
                        );
                    }
                }
                return Type::Named {
                    def: name.name,
                    args: vec![],
                };
            }
        }
        if let Some((parent, spec)) = self.variants.get(&name.name).cloned() {
            for (n, e) in fields {
                let ft = spec
                    .iter()
                    .find(|(fnm, _)| *fnm == n.name)
                    .map(|(_, t)| t.clone());
                let g = self.check_expr(e, ft.as_ref(), env);
                if let Some(ft) = ft {
                    if !types_eq(&g, &ft) {
                        self.type_mismatch(e.span, &ft, &g, &env.def_id);
                    }
                }
            }
            return Type::Named {
                def: parent,
                args: vec![],
            };
        }
        // expected sum type
        if let Some(Type::Named { def, .. } | Type::Variant { def, .. }) = expected {
            let def = *def;
            if let Some(td) = self.type_defs.get(&def).cloned() {
                if let TypeDefKind::Variants(vs) = td.kind {
                    if let Some((_, spec)) = vs.iter().find(|(n, _)| *n == name.name) {
                        for (n, e) in fields {
                            let ft = spec.iter().find(|(fnm, _)| *fnm == n.name).map(|(_, t)| t);
                            let _ = self.check_expr(e, ft, env);
                        }
                        return Type::Named { def, args: vec![] };
                    }
                }
            }
        }
        // construct anyway (user type not yet in variants table)
        for (_, e) in fields {
            let _ = self.check_expr(e, None, env);
        }
        if let Some(exp) = expected {
            return exp.clone();
        }
        self.err(
            "E0105",
            span,
            format!("unknown variant `{}`", self.intern.get(name.name)),
        );
        Type::Error
    }

    fn qualified_name(&self, e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Path(p) => Some(path_str(p, self.intern)),
            ExprKind::Field { base, field } => {
                let left = self.qualified_name(base)?;
                Some(format!("{}.{}", left, self.intern.get(field.name)))
            }
            _ => None,
        }
    }

    /// Field lookup that reports nothing when the field is absent. Callers that
    /// have a fallback (method dispatch, callable fields) need to look without
    /// committing to an error.
    fn try_field_type(&mut self, base: &Type, field: &Ident) -> Option<Type> {
        let base = match base {
            Type::Ref { inner, .. }
            | Type::Own(inner)
            | Type::Untrusted(inner)
            | Type::Secret(inner) => inner.as_ref(),
            other => other,
        };
        match base {
            Type::Record(fs) => fs
                .iter()
                .find(|(n, _)| *n == field.name)
                .map(|(_, t)| t.clone()),
            Type::Named { def, args } => {
                let td = self.type_defs.get(def)?;
                let TypeDefKind::Record(fs) = &td.kind else {
                    return None;
                };
                let (_, t) = fs.iter().find(|(n, _)| *n == field.name)?;
                let map: HashMap<Symbol, Type> = td
                    .generics
                    .iter()
                    .copied()
                    .zip(args.iter().cloned())
                    .collect();
                let t = t.clone();
                Some(if map.is_empty() {
                    t
                } else {
                    crate::types::subst(&t, &map)
                })
            }
            _ => None,
        }
    }

    fn field_type(&mut self, base: &Type, field: &Ident, span: Span) -> Type {
        let base = match base {
            Type::Ref { inner, .. }
            | Type::Own(inner)
            | Type::Untrusted(inner)
            | Type::Secret(inner) => inner.as_ref(),
            other => other,
        };
        match base {
            Type::Record(fs) => {
                if let Some((_, t)) = fs.iter().find(|(n, _)| *n == field.name) {
                    return t.clone();
                }
            }
            Type::Named { def, args } => {
                if let Some(td) = self.type_defs.get(def) {
                    if let TypeDefKind::Record(fs) = &td.kind {
                        if let Some((_, t)) = fs.iter().find(|(n, _)| *n == field.name) {
                            // Substitute the type arguments: reading `cmp` off an
                            // `Ord[i32]` must give `fn(&i32, &i32) -> Ordering`,
                            // not the declaration's `fn(&T, &T) -> Ordering`.
                            let map: HashMap<Symbol, Type> = td
                                .generics
                                .iter()
                                .copied()
                                .zip(args.iter().cloned())
                                .collect();
                            let t = t.clone();
                            return if map.is_empty() {
                                t
                            } else {
                                crate::types::subst(&t, &map)
                            };
                        }
                    }
                }
            }
            _ => {}
        }
        self.err(
            "E0104",
            span,
            format!(
                "`{}` has no field `{}`",
                base.display(self.intern),
                self.intern.get(field.name)
            ),
        );
        Type::Error
    }

    fn map_key_type(&self, base: &Type) -> Type {
        match base {
            Type::Named { args, .. } => args
                .first()
                .cloned()
                .unwrap_or(builtins::string_type(&self.b)),
            Type::Ref { inner, .. } | Type::Own(inner) => self.map_key_type(inner),
            _ => builtins::string_type(&self.b),
        }
    }

    fn map_val_type(&self, base: &Type) -> Type {
        match base {
            Type::Named { args, .. } => args.get(1).cloned().unwrap_or(Type::i32()),
            Type::Ref { inner, .. } | Type::Own(inner) => self.map_val_type(inner),
            _ => Type::i32(),
        }
    }

    fn elem_type(&mut self, base: &Type, span: Span) -> Type {
        let base = match base {
            Type::Ref { inner, .. } => inner.as_ref(),
            other => other,
        };
        match base {
            Type::Named { def, args } => {
                let n = self.intern.get(*def);
                if n == "Vec" || n == "slice" {
                    return args.first().cloned().unwrap_or(Type::Error);
                }
            }
            Type::Tuple(ts) => return ts.first().cloned().unwrap_or(Type::Error),
            _ => {}
        }
        self.err("E0107", span, "not indexable");
        Type::Error
    }

    fn iter_elem(&self, ty: &Type) -> Type {
        match ty {
            Type::Named { def, args } => {
                let n = self.intern.get(*def);
                if n == "Range" {
                    return args.first().cloned().unwrap_or(Type::usz());
                }
                if n == "Vec" || n == "slice" {
                    return args.first().cloned().unwrap_or(Type::Error);
                }
                Type::usz()
            }
            _ => Type::usz(),
        }
    }

    fn variant_fields(&mut self, name: Symbol, ty: &Type) -> Vec<(Symbol, Type)> {
        // Instantiate Option / Result payloads from the scrutinee type args.
        if let Type::Named { def, args } = ty {
            let n = self.intern.get(*def).to_string();
            if n == "Result" && args.len() == 2 {
                let ok = self.intern.intern("Ok");
                let err = self.intern.intern("Err");
                let slot = self.intern.intern("_0");
                if name == ok {
                    return vec![(slot, args[0].clone())];
                }
                if name == err {
                    return vec![(slot, args[1].clone())];
                }
            }
            if n == "Option" && args.len() == 1 {
                let some = self.intern.intern("Some");
                if name == some {
                    let value = self.intern.intern("value");
                    let slot = self.intern.intern("_0");
                    return vec![(value, args[0].clone()), (slot, args[0].clone())];
                }
            }
        }
        if let Some((_, fs)) = self.variants.get(&name) {
            return fs.clone();
        }
        if let Type::Named { def, .. } | Type::Variant { def, .. } = ty {
            if let Some(td) = self.type_defs.get(def) {
                if let TypeDefKind::Variants(vs) = &td.kind {
                    if let Some((_, fs)) = vs.iter().find(|(n, _)| *n == name) {
                        return fs.clone();
                    }
                }
            }
        }
        Vec::new()
    }

    fn is_variant_of(&self, got: &Type, parent: &Type) -> bool {
        match (got, parent) {
            (Type::Named { def: a, .. }, Type::Named { def: b, .. }) => a == b,
            (Type::Variant { def: a, .. }, Type::Named { def: b, .. }) => a == b,
            (Type::Named { def: a, .. }, Type::Variant { def: b, .. }) => a == b,
            (Type::Variant { def: a, .. }, Type::Variant { def: b, .. }) => a == b,
            _ => false,
        }
    }

    fn inject_or_admit(&mut self, callee_err: &Type, env: &mut Env, span: Span) {
        if let Some(declared) = env.declared.err_type().cloned() {
            if types_eq(callee_err, &declared)
                || self.is_variant_of(callee_err, &declared)
                || self.same_err(callee_err, &declared)
            {
                env.inferred.insert(EffectAtom::Err(declared));
                return;
            }
            // look for a single-step injection into declared
            if let Type::Named { def, .. } = &declared {
                if let Some(td) = self.type_defs.get(def) {
                    let matches: Vec<_> = td
                        .injections
                        .iter()
                        .filter(|i| {
                            types_eq(&i.from, callee_err) || self.same_err(&i.from, callee_err)
                        })
                        .cloned()
                        .collect();
                    if matches.len() == 1 {
                        env.inferred.insert(EffectAtom::Err(declared));
                        return;
                    }
                    if matches.len() > 1 {
                        self.err("E0203", span, "ambiguous injection");
                        return;
                    }
                }
            }
            let mut d = Diagnostic::error(
                "E0202",
                span,
                format!(
                    "missing declared injection from {} into {}",
                    callee_err.display(self.intern),
                    declared.display(self.intern)
                ),
            )
            .with_def(&env.def_id);
            d.kind = "missing_injection".into();
            d.fixes.push(Fix {
                kind: "declare_injection".into(),
                safety: FixSafety::InterfaceWidening,
                rank: 1,
                patch: format!("from {} => /* variant */;", callee_err.display(self.intern)),
                note: None,
            });
            d.fixes.push(Fix {
                kind: "wrap_attempt".into(),
                safety: FixSafety::BehaviorChanging,
                rank: 2,
                patch: "attempt <expr>".into(),
                note: None,
            });
            self.diags.push(d);
            env.inferred.insert(EffectAtom::Err(declared));
            return;
        }
        // caller has no err — require catch/attempt or it is not permitted
        env.inferred.insert(EffectAtom::Err(callee_err.clone()));
    }

    fn same_err(&self, a: &Type, b: &Type) -> bool {
        if types_eq(a, b) {
            return true;
        }
        let da = match a {
            Type::Named { def, .. } | Type::Variant { def, .. } => Some(*def),
            _ => None,
        };
        let db = match b {
            Type::Named { def, .. } | Type::Variant { def, .. } => Some(*def),
            _ => None,
        };
        if let (Some(x), Some(y)) = (da, db) {
            if x == y {
                return true;
            }
            return self.intern.get(x) == self.intern.get(y);
        }
        a.display(self.intern) == b.display(self.intern)
    }

    fn admit_effects(&mut self, effects: &EffectSet, env: &mut Env, _span: Span) {
        for a in &effects.atoms {
            env.inferred.insert(a.clone());
        }
    }

    fn check_row_subset(
        &mut self,
        inferred: &EffectSet,
        declared: &EffectSet,
        span: Span,
        def_id: &str,
    ) {
        for a in &inferred.atoms {
            let ok = declared.atoms.iter().any(|d| match (a, d) {
                (EffectAtom::Err(x), EffectAtom::Err(y)) => types_eq(x, y) || self.same_err(x, y),
                (EffectAtom::Alloc(_), EffectAtom::Alloc(_)) => true,
                (EffectAtom::Io(_), EffectAtom::Io(_)) => true,
                _ => a == d,
            });
            if !ok {
                let mut d = Diagnostic::error(
                    "E0200",
                    span,
                    format!(
                        "effect {} is not permitted by the declared row {}",
                        a.display(self.intern),
                        declared.display(self.intern)
                    ),
                )
                .with_def(def_id)
                .with_rows(
                    declared
                        .atoms
                        .iter()
                        .map(|x| x.display(self.intern))
                        .collect(),
                    inferred
                        .atoms
                        .iter()
                        .map(|x| x.display(self.intern))
                        .collect(),
                );
                d.kind = "effect_not_permitted".into();
                d.fixes.push(Fix {
                    kind: "add_effect".into(),
                    safety: FixSafety::InterfaceWidening,
                    rank: 1,
                    patch: format!(
                        "!{{{}, {}}}",
                        declared.display(self.intern),
                        a.display(self.intern)
                    ),
                    note: None,
                });
                if matches!(a, EffectAtom::Err(_)) {
                    d.fixes.push(Fix {
                        kind: "wrap_attempt".into(),
                        safety: FixSafety::BehaviorChanging,
                        rank: 2,
                        patch: "attempt <expr>".into(),
                        note: None,
                    });
                }
                self.diags.push(d);
            }
        }
    }

    /// Index into `self.dicts` of the unique visible dictionary for `dt`.
    fn resolve_default_dict(&mut self, dt: &Type, span: Span) -> Option<usize> {
        let name = match dt {
            Type::Named { def, .. } => *def,
            _ => return None,
        };
        let matches: Vec<usize> = self
            .dicts
            .iter()
            .enumerate()
            .filter(|(_, d)| d.name == name)
            .map(|(i, _)| i)
            .collect();
        if matches.is_empty() {
            // also match by dict type name vs dict ident (dict Ord[Rec])
            let alt: Vec<usize> = self
                .dicts
                .iter()
                .enumerate()
                .filter(|(_, d)| match dt {
                    Type::Named { args, .. } => {
                        args.first()
                            .map(|a| {
                                types_eq(a, &d.for_ty) || {
                                    if let (
                                        Type::Named { def: x, .. },
                                        Type::Named { def: y, .. },
                                    ) = (a, &d.for_ty)
                                    {
                                        x == y
                                    } else {
                                        false
                                    }
                                }
                            })
                            .unwrap_or(false)
                            && d.name == name
                    }
                    _ => false,
                })
                .map(|(i, _)| i)
                .collect();
            if alt.len() == 1 {
                return Some(alt[0]);
            }
            if alt.is_empty() {
                // not fatal if a dict was declared with that name at all
                if let Some((i, _)) = self.dicts.iter().enumerate().find(|(_, d)| d.name == name) {
                    return Some(i);
                }
                self.err(
                    "E0401",
                    span,
                    format!(
                        "no visible default dictionary for {}",
                        dt.display(self.intern)
                    ),
                );
            } else if alt.len() > 1 {
                self.err("E0400", span, "ambiguous default dictionary");
            }
            return None;
        }
        if matches.len() > 1 {
            self.err("E0400", span, "ambiguous default dictionary");
        }
        matches.first().copied()
    }

    fn rank_candidates(&self, expected: &Type, env: &Env) -> Vec<HoleCandidate> {
        let mut cands = Vec::new();
        let mut rank = 1u32;
        for (name, ty) in env.in_scope(self.intern) {
            if types_eq(&ty, expected) {
                cands.push(HoleCandidate {
                    rank,
                    name: name.clone(),
                    ty: ty.display(self.intern),
                    note: "in scope, exact type".into(),
                });
                rank += 1;
            }
        }
        for (qid, sig) in &self.fns {
            if types_eq(&sig.ret, expected) {
                cands.push(HoleCandidate {
                    rank,
                    name: qid.clone(),
                    ty: format!("fn(...) -> {}", sig.ret.display(self.intern)),
                    note: "library, exact return".into(),
                });
                rank += 1;
                if rank > 8 {
                    break;
                }
            }
        }
        cands
    }

    fn lit_type(&self, l: &Lit, expected: Option<&Type>) -> Type {
        match l {
            Lit::Bool(_) => Type::bool(),
            Lit::Unit => Type::unit(),
            Lit::Str(_) => builtins::string_type(&self.b),
            Lit::Int { suffix, .. } => {
                if let Some(p) = suffix {
                    Type::Prim(*p)
                } else if let Some(Type::Prim(p)) = expected {
                    if p.is_int() {
                        return Type::Prim(*p);
                    }
                    Type::i32()
                } else {
                    Type::i32()
                }
            }
            Lit::Float { suffix, .. } => {
                if let Some(p) = suffix {
                    Type::Prim(*p)
                } else if let Some(Type::Prim(p)) = expected {
                    if p.is_float() {
                        return Type::Prim(*p);
                    }
                    Type::f64()
                } else {
                    // An unsuffixed float literal is f64, as in Rust and Go.
                    Type::f64()
                }
            }
        }
    }

    fn collect_mut_captures(&self, e: &Expr, out: &mut Vec<Symbol>) {
        match &e.kind {
            ExprKind::Unary {
                op: UnOp::RefMut,
                expr,
            } => {
                if let ExprKind::Path(p) = &expr.kind {
                    if let Some(s) = p.segs.first() {
                        out.push(s.name);
                    }
                }
                self.collect_mut_captures(expr, out);
            }
            ExprKind::Call { callee, args } => {
                self.collect_mut_captures(callee, out);
                for a in args {
                    self.collect_mut_captures(a, out);
                }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.collect_mut_captures(lhs, out);
                self.collect_mut_captures(rhs, out);
            }
            ExprKind::Assign { lhs, rhs } => {
                if let ExprKind::Path(p) = &lhs.kind {
                    if let Some(s) = p.segs.first() {
                        out.push(s.name);
                    }
                }
                self.collect_mut_captures(lhs, out);
                self.collect_mut_captures(rhs, out);
            }
            ExprKind::Block { stmts, tail } => {
                for s in stmts {
                    match &s.kind {
                        StmtKind::Let(l) => self.collect_mut_captures(&l.init, out),
                        StmtKind::Expr(x) => self.collect_mut_captures(x, out),
                    }
                }
                if let Some(t) = tail {
                    self.collect_mut_captures(t, out);
                }
            }
            _ => {}
        }
    }

    fn type_mismatch(&mut self, span: Span, expected: &Type, actual: &Type, def_id: &str) {
        if expected.is_error() || actual.is_error() || expected.is_hole() || actual.is_hole() {
            return;
        }
        if types_eq(expected, actual) {
            return;
        }
        // Seq / generic params: don't report
        if matches!(expected, Type::Param(_)) || matches!(actual, Type::Param(_)) {
            return;
        }
        self.diags.push(
            Diagnostic::error(
                "E0101",
                span,
                format!(
                    "type mismatch: expected {}, got {}",
                    expected.display(self.intern),
                    actual.display(self.intern)
                ),
            )
            .with_def(def_id)
            .with_expected_actual(expected.display(self.intern), actual.display(self.intern)),
        );
    }

    fn err(&mut self, code: &str, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::error(code, span, msg.into()));
    }

    pub fn intern(&self) -> &Interner {
        self.intern
    }

    pub fn type_defs(&self) -> &HashMap<Symbol, TypeDef> {
        &self.type_defs
    }

    fn lookup_fn(&self, q: &str) -> Option<&FnSig> {
        if let Some(s) = self.fns.get(q) {
            return Some(s);
        }
        // fs.read after `use std.fs`
        if let Some((_, rest)) = q.split_once('.') {
            if let Some(s) = self.fns.get(q) {
                return Some(s);
            }
            // try as-is already failed; try last two segments
            let _ = rest;
        }
        self.fns.get(q)
    }
}

/// Peel `Result[T, E]` (and the same type wrapped in Untrusted/Secret/Ref/Own).
fn peel_result(ty: &Type, b: &builtins::Builtins) -> Option<(Type, Type)> {
    match ty {
        Type::Named { def, args } if *def == b.result && args.len() == 2 => {
            Some((args[0].clone(), args[1].clone()))
        }
        Type::Untrusted(inner) | Type::Secret(inner) | Type::Own(inner) => peel_result(inner, b),
        Type::Ref { inner, .. } => peel_result(inner, b),
        _ => None,
    }
}

fn path_str(p: &Path, intern: &Interner) -> String {
    p.segs
        .iter()
        .map(|s| intern.get(s.name))
        .collect::<Vec<_>>()
        .join(".")
}

fn type_mentions_region(ty: &Type, r: RegionId) -> bool {
    match ty {
        Type::Ref { region, inner, .. } => region.name == r.name || type_mentions_region(inner, r),
        Type::Own(t) | Type::Untrusted(t) | Type::Secret(t) => type_mentions_region(t, r),
        Type::Named { args, .. } => args.iter().any(|a| type_mentions_region(a, r)),
        Type::Tuple(ts) => ts.iter().any(|t| type_mentions_region(t, r)),
        Type::Record(fs) => fs.iter().any(|(_, t)| type_mentions_region(t, r)),
        Type::Fn { params, ret, .. } => {
            params.iter().any(|t| type_mentions_region(t, r)) || type_mentions_region(ret, r)
        }
        _ => false,
    }
}

fn contract_legal(k: &ExprKind) -> bool {
    match k {
        ExprKind::Lit(_)
        | ExprKind::Path(_)
        | ExprKind::Field { .. }
        | ExprKind::Binary { .. }
        | ExprKind::Unary { .. }
        | ExprKind::Call { .. }
        | ExprKind::Lambda { .. }
        | ExprKind::Hole => true,
        ExprKind::Loop { .. }
        | ExprKind::Raise(_)
        | ExprKind::Par { .. }
        | ExprKind::Region { .. }
        | ExprKind::Attempt(_)
        | ExprKind::Catch { .. } => false,
        _ => true,
    }
}

/// Combine the types of two control-flow branches.
///
/// `never` is absorbing in the other direction from `Error`: a branch that
/// diverges imposes no constraint, so the sibling's type wins.
fn join_types(a: Type, b: Type) -> Type {
    match (&a, &b) {
        (Type::Never, _) => b,
        (_, Type::Never) => a,
        _ => a,
    }
}

/// Does this type still mention an unresolved type parameter?
fn type_mentions_param(t: &Type) -> bool {
    match t {
        Type::Param(_) => true,
        Type::Named { args, .. } | Type::Tuple(args) => args.iter().any(type_mentions_param),
        Type::Ref { inner, .. }
        | Type::Own(inner)
        | Type::Untrusted(inner)
        | Type::Secret(inner) => type_mentions_param(inner),
        Type::Record(fs) => fs.iter().any(|(_, x)| type_mentions_param(x)),
        Type::Fn { params, ret, .. } => {
            params.iter().any(type_mentions_param) || type_mentions_param(ret)
        }
        Type::Variant { variants, .. } => variants
            .iter()
            .any(|(_, fs)| fs.iter().any(|(_, x)| type_mentions_param(x))),
        _ => false,
    }
}

fn bind_effect_params(expected: &Type, actual: &Type, binds: &mut HashMap<Symbol, EffectSet>) {
    match (expected, actual) {
        (
            Type::Fn {
                params: expected_params,
                ret: expected_ret,
                effects: expected_effects,
            },
            Type::Fn {
                params: actual_params,
                ret: actual_ret,
                effects: actual_effects,
            },
        ) => {
            for effect in &expected_effects.atoms {
                if let EffectAtom::Var(symbol) = effect {
                    let bound = binds.entry(*symbol).or_default();
                    *bound = bound.union(actual_effects);
                }
            }
            for (expected_param, actual_param) in expected_params.iter().zip(actual_params) {
                bind_effect_params(expected_param, actual_param, binds);
            }
            bind_effect_params(expected_ret, actual_ret, binds);
        }
        (Type::Named { args: expected, .. }, Type::Named { args: actual, .. })
        | (Type::Tuple(expected), Type::Tuple(actual)) => {
            for (expected, actual) in expected.iter().zip(actual) {
                bind_effect_params(expected, actual, binds);
            }
        }
        (Type::Record(expected), Type::Record(actual)) => {
            for ((_, expected), (_, actual)) in expected.iter().zip(actual) {
                bind_effect_params(expected, actual, binds);
            }
        }
        (
            Type::Ref {
                inner: expected, ..
            },
            Type::Ref { inner: actual, .. },
        )
        | (Type::Own(expected), Type::Own(actual))
        | (Type::Untrusted(expected), Type::Untrusted(actual))
        | (Type::Secret(expected), Type::Secret(actual)) => {
            bind_effect_params(expected, actual, binds);
        }
        _ => {}
    }
}

/// Which container a method call is dispatching on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SeqKind {
    Vec,
    Slice,
    Str,
    Map,
}

/// Nominal type a pattern is being matched against, if it has one.
fn scrutinee_def(ty: &Type) -> Option<Symbol> {
    match ty {
        Type::Named { def, .. } | Type::Variant { def, .. } => Some(*def),
        Type::Ref { inner, .. }
        | Type::Own(inner)
        | Type::Untrusted(inner)
        | Type::Secret(inner) => scrutinee_def(inner),
        _ => None,
    }
}

/// Apply `f` to each direct sub-expression of `e`.
fn for_each_child(e: &Expr, f: &mut impl FnMut(&Expr)) {
    match &e.kind {
        ExprKind::Call { callee, args } => {
            f(callee);
            for a in args {
                f(a);
            }
        }
        ExprKind::Field { base, .. } => f(base),
        ExprKind::Index { base, index } => {
            f(base);
            f(index);
        }
        ExprKind::Unary { expr, .. } => f(expr),
        ExprKind::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        ExprKind::Block { stmts, tail } => {
            for st in stmts {
                match &st.kind {
                    StmtKind::Let(l) => f(&l.init),
                    StmtKind::Expr(x) => f(x),
                }
            }
            if let Some(t) = tail {
                f(t);
            }
        }
        ExprKind::If {
            cond,
            then_b,
            else_b,
        } => {
            f(cond);
            f(then_b);
            if let Some(x) = else_b {
                f(x);
            }
        }
        ExprKind::Match { scrut, arms } | ExprKind::Catch { expr: scrut, arms } => {
            f(scrut);
            for a in arms {
                f(&a.body);
            }
        }
        ExprKind::For { iter, body, .. } => {
            f(iter);
            f(body);
        }
        ExprKind::While { cond, body } => {
            f(cond);
            f(body);
        }
        ExprKind::Loop { body } | ExprKind::Region { body, .. } => f(body),
        ExprKind::Let(l) => f(&l.init),
        ExprKind::Lambda { body, .. } => f(body),
        ExprKind::Record(fs) | ExprKind::Variant { fields: fs, .. } => {
            for (_, x) in fs {
                f(x);
            }
        }
        ExprKind::Return(inner) => {
            if let Some(x) = inner {
                f(x);
            }
        }
        ExprKind::Raise(inner)
        | ExprKind::Attempt(inner)
        | ExprKind::Try(inner)
        | ExprKind::Cast { expr: inner, .. } => f(inner),
        ExprKind::Interpolate { parts } => {
            for p in parts {
                if let crate::ast::InterpPart::Expr(x) = p {
                    f(x);
                }
            }
        }
        ExprKind::Par { bindings } => {
            for l in bindings {
                f(&l.init);
            }
        }
        ExprKind::Assign { lhs, rhs } => {
            f(lhs);
            f(rhs);
        }
        ExprKind::Lit(_)
        | ExprKind::Path(_)
        | ExprKind::Hole
        | ExprKind::Break
        | ExprKind::Continue => {}
    }
}
