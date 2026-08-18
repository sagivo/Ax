//! Checked AST -> typed SSA IR.
//!
//! This is the only place that reads syntax and the only place that decides
//! representation. Everything downstream (C, Cranelift) reads [`crate::ir`].
//!
//! Types come from the checker's node table, never re-derived from syntax.
//! Three semantic obligations are discharged here, not in a backend:
//!
//! - **Errors.** `err[E]` becomes an explicit two-value return plus a caller
//!   allocated payload slot. `raise`, `catch`, `attempt` and `?` all lower to
//!   ordinary branches on the tag, so no backend needs unwinding.
//! - **Regions.** A `region r { .. }` becomes a bump arena entered and exited
//!   at the lexical boundary; allocations inside it are pointer bumps.
//! - **Aborts.** Bounds checks, non-exhaustive matches, and contract failures
//!   become explicit `Term::Abort`, matching the oracle's observable behaviour.

use crate::ast::*;
use crate::check::{CheckOutput, CheckedFn};
use crate::effects::{EffectAtom, EffectSet};
use crate::intern::{Interner, Symbol};
use crate::ir::*;
use crate::types::{subst, Prim, Type, TypeDefKind};
use std::collections::HashMap;

/// Fixed field indices of the built-in string layout. Vec and slice fields are
/// looked up by name (`data` / `len` / `cap`) so a layout change cannot silently
/// mis-index them.
const STR_DATA: u32 = 0;
const STR_LEN: u32 = 1;

/// Interpreter steps a compile-time fold may spend before giving up. Small
/// enough that a pathological constant expression cannot stall a build; large
/// enough for the arithmetic that shows up in real code.
const FOLD_STEPS: u64 = 200_000;

pub fn lower_program(intern: &Interner, co: &CheckOutput) -> Result<Program, String> {
    let mut l = Lowerer::new(intern, co);
    l.run()?;
    let prog = l.prog;
    verify(&prog).map_err(|e| format!("internal: IR verification failed: {e}"))?;
    Ok(prog)
}

struct Lowerer<'a> {
    intern: &'a Interner,
    co: &'a CheckOutput,
    prog: Program,
    /// Canonical type key -> aggregate id.
    aggs: HashMap<String, TypeId>,
    strings: HashMap<String, u32>,
    /// Mangled name -> id, for both already-lowered and queued functions.
    fn_ids: HashMap<String, FuncId>,
    queue: Vec<Pending>,
}

struct Pending {
    id: FuncId,
    /// Index into `co.fns`.
    src: usize,
    subst: HashMap<Symbol, Type>,
    /// Dictionary chosen for each dictionary-typed parameter, by parameter
    /// index. Part of the instantiation key: two calls that resolve different
    /// dictionaries produce two functions.
    dicts: Vec<(u32, usize)>,
}

/// A lowered expression. Aggregates are represented by a pointer to storage.
#[derive(Clone, Copy, Debug)]
struct LVal {
    v: ValId,
    ty: IrTy,
    agg: Option<TypeId>,
}

impl LVal {
    fn scalar(v: ValId, ty: IrTy) -> Self {
        Self { v, ty, agg: None }
    }
}

/// Where a raised error goes. An empty handler stack means "propagate out of
/// the function along the error channel".
///
/// Each handler owns the storage its payload lands in, so `catch` works in a
/// function that is itself infallible — the common case, and the reason the
/// slot cannot simply be the function's own error destination.
#[derive(Clone, Copy)]
struct Handler {
    block: BlockId,
    slot: ValId,
    agg: TypeId,
}

impl<'a> Lowerer<'a> {
    fn new(intern: &'a Interner, co: &'a CheckOutput) -> Self {
        Self {
            intern,
            co,
            prog: Program {
                module: co.module.clone(),
                ..Default::default()
            },
            aggs: HashMap::new(),
            strings: HashMap::new(),
            fn_ids: HashMap::new(),
            queue: Vec::new(),
        }
    }

    fn run(&mut self) -> Result<(), String> {
        // Honour the ownership ladder: unique-heap / register strategies are
        // already how scalars and last-use moves lower. Residual RC is
        // recorded on `co.ownership` for `ax perf`; the C backend emits
        // ordinary malloc/free for unique heap values (no RC word).
        // Non-generic functions are roots. Generic ones are lowered on demand
        // from call sites, once per distinct instantiation.
        for (i, f) in self.co.fns.iter().enumerate() {
            // A dictionary parameter is resolved per call site, so such a
            // function is instantiated on demand exactly like a generic one.
            let takes_dict = f.sig.params.iter().any(|(_, _, is_dict)| *is_dict);
            if f.sig.generics.is_empty() && !f.sig.is_contract_fn && !takes_dict {
                let name = self.mangle(&self.sym(f.sig.name), &[]);
                self.enqueue(i, name, HashMap::new(), Vec::new());
            }
        }
        let mut test_fns = Vec::new();
        for (ti, _t) in self.co.tests.iter().enumerate() {
            let id = self.prog.funcs.len() as FuncId;
            self.prog.funcs.push(placeholder(id, format!("ax_test_{ti}")));
            test_fns.push((ti, id));
        }

        // Drain the worklist; lowering a body can enqueue new instantiations.
        let mut done = 0;
        while done < self.queue.len() {
            let p = Pending {
                id: self.queue[done].id,
                src: self.queue[done].src,
                subst: self.queue[done].subst.clone(),
                dicts: self.queue[done].dicts.clone(),
            };
            done += 1;
            let f = &self.co.fns[p.src];
            let func = self.lower_fn(f, p.id, &p.subst, &p.dicts)?;
            self.prog.funcs[p.id as usize] = func;
        }

        for (ti, id) in test_fns {
            let t = &self.co.tests[ti];
            let func = self.lower_test(t.name.clone(), &t.body, id, &t.def_id)?;
            self.prog.funcs[id as usize] = func;
            self.prog.tests.push((t.name.clone(), id));
        }

        self.prog.main = self
            .fn_ids
            .get("ax_main")
            .copied()
            .or_else(|| self.prog.find_func("ax_main").map(|f| f.id));
        Ok(())
    }

    fn enqueue(
        &mut self,
        src: usize,
        mangled: String,
        subst: HashMap<Symbol, Type>,
        dicts: Vec<(u32, usize)>,
    ) -> FuncId {
        if let Some(id) = self.fn_ids.get(&mangled) {
            return *id;
        }
        let id = self.prog.funcs.len() as FuncId;
        self.prog.funcs.push(placeholder(id, mangled.clone()));
        self.fn_ids.insert(mangled, id);
        self.queue.push(Pending {
            id,
            src,
            subst,
            dicts,
        });
        id
    }

    /// Symbol for a known name. The interner is read-only here, so this looks up
    /// a name the prelude already interned.
    fn intern_sym(&self, name: &str) -> Symbol {
        self.intern
            .lookup(name)
            .unwrap_or_else(|| panic!("prelude symbol `{name}` was never interned"))
    }

    fn sym(&self, s: Symbol) -> String {
        self.intern.get(s).to_string()
    }

    /// Link name. Generic instantiations get the argument types appended so
    /// each monomorphisation is a distinct symbol.
    fn mangle(&self, name: &str, args: &[Type]) -> String {
        let mut s = format!("ax_{}", sanitize(name));
        for a in args {
            s.push('_');
            s.push_str(&sanitize(&a.display(self.intern)));
        }
        s
    }

    /// Reserve a fresh function id with a link name.
    fn reserve_func(&mut self, name: String) -> FuncId {
        let id = self.prog.funcs.len() as FuncId;
        self.prog.funcs.push(placeholder(id, name.clone()));
        self.fn_ids.insert(name, id);
        id
    }

    fn intern_string(&mut self, text: &str) -> u32 {
        if let Some(i) = self.strings.get(text) {
            return *i;
        }
        let i = self.prog.strings.len() as u32;
        self.prog.strings.push(text.to_string());
        self.strings.insert(text.to_string(), i);
        i
    }

    // ---- type mapping -------------------------------------------------

    fn resolve(&self, t: &Type, subst_map: &HashMap<Symbol, Type>) -> Type {
        if subst_map.is_empty() {
            t.clone()
        } else {
            subst(t, subst_map)
        }
    }

    /// Machine type of a semantic type. Aggregates answer `Ptr`; use
    /// [`Self::agg_of`] to get their layout.
    fn ir_ty(&mut self, t: &Type) -> Result<IrTy, String> {
        Ok(match t {
            Type::Prim(p) => prim_ir(*p),
            Type::Ref { .. }
            | Type::Own(_)
            | Type::Untrusted(_)
            | Type::Secret(_)
            | Type::Fn { .. } => IrTy::Ptr,
            Type::Named { def, .. } => {
                let n = self.sym(*def);
                match n.as_str() {
                    // Opaque runtime handles.
                    "Alloc" | "ReadCap" | "fs.ReadCap" | "Map" | "SortedMap" => IrTy::Ptr,
                    _ => {
                        if self.agg_of(t)?.is_some() {
                            IrTy::Ptr
                        } else {
                            return Err(format!(
                                "native backend: unsupported type `{}`",
                                t.display(self.intern)
                            ));
                        }
                    }
                }
            }
            Type::Tuple(_) | Type::Record(_) | Type::Variant { .. } => IrTy::Ptr,
            Type::Param(s) => {
                return Err(format!(
                    "native backend: unsubstituted type parameter `{}`",
                    self.sym(*s)
                ))
            }
            // A `never`-typed expression is unreachable by construction, so its
            // representation is irrelevant; `unit` is the cheapest placeholder.
            Type::Never => IrTy::Unit,
            Type::Hole => return Err("native backend: program still contains a hole".into()),
            Type::Error => {
                if std::env::var("AX_DEBUG_IR").is_ok() {
                    panic!("ir_ty on Type::Error");
                }
                return Err("native backend: program has type errors".into());
            }
        })
    }

    /// Layout for aggregate types; `None` for scalars and opaque handles.
    fn agg_of(&mut self, t: &Type) -> Result<Option<TypeId>, String> {
        let key = self.type_key(t);
        if let Some(id) = self.aggs.get(&key) {
            return Ok(Some(*id));
        }
        let built = match t {
            Type::Named { def, args } => {
                let name = self.sym(*def);
                match name.as_str() {
                    "String" | "str" => Some(self.build_str_agg(&key)),
                    "Vec" => {
                        let elem = args.first().cloned().unwrap_or(Type::unit());
                        Some(self.build_vec_agg(&key, &elem)?)
                    }
                    "slice" => {
                        let elem = args.first().cloned().unwrap_or(Type::unit());
                        Some(self.build_slice_agg(&key, &elem)?)
                    }
                    "Alloc" | "ReadCap" | "fs.ReadCap" | "Map" | "SortedMap" => None,
                    _ => {
                        let def_sym = *def;
                        let td = self
                            .co
                            .types
                            .iter()
                            .find(|d| d.name == def_sym)
                            .cloned()
                            .ok_or_else(|| format!("unknown type `{name}`"))?;
                        let map: HashMap<Symbol, Type> = td
                            .generics
                            .iter()
                            .copied()
                            .zip(args.iter().cloned())
                            .collect();
                        match &td.kind {
                            TypeDefKind::Record(fs) => {
                                let fs: Vec<(Symbol, Type)> = fs
                                    .iter()
                                    .map(|(n, ft)| (*n, self.resolve(ft, &map)))
                                    .collect();
                                Some(self.build_record(&key, &name, &fs)?)
                            }
                            TypeDefKind::Variants(vs) => {
                                let vs: Vec<(Symbol, Vec<(Symbol, Type)>)> = vs
                                    .iter()
                                    .map(|(vn, fs)| {
                                        (
                                            *vn,
                                            fs.iter()
                                                .map(|(n, ft)| (*n, self.resolve(ft, &map)))
                                                .collect(),
                                        )
                                    })
                                    .collect();
                                Some(self.build_variant(&key, &name, &vs)?)
                            }
                            TypeDefKind::Alias(inner) => {
                                let inner = self.resolve(&inner.clone(), &map);
                                // `type X = Y` shares Y's layout.
                                return self.agg_of(&inner);
                            }
                        }
                    }
                }
            }
            Type::Record(fs) => {
                let fs = fs.clone();
                Some(self.build_record(&key, "rec", &fs)?)
            }
            Type::Tuple(ts) => {
                // Tuple fields are positional: `f0`, `f1`, ... so pattern and
                // index access share one naming scheme.
                let names: Vec<String> = (0..ts.len()).map(|i| format!("f{i}")).collect();
                let ts = ts.clone();
                Some(self.build_tuple(&key, &names, &ts)?)
            }
            Type::Variant { def, variants } => {
                let name = self.sym(*def);
                let vs = variants.clone();
                Some(self.build_variant(&key, &name, &vs)?)
            }
            // A reference to an aggregate is a pointer to that layout, so field
            // access through `&Item` works without an explicit dereference.
            Type::Ref { inner, .. }
            | Type::Own(inner)
            | Type::Untrusted(inner)
            | Type::Secret(inner) => {
                let inner = (**inner).clone();
                return self.agg_of(&inner);
            }
            _ => None,
        };
        Ok(built)
    }

    /// Structural key: two types with the same key share a layout.
    fn type_key(&self, t: &Type) -> String {
        t.display(self.intern)
    }

    fn record_agg(&mut self, key: &str, name: &str, fields: Vec<FieldDef>) -> TypeId {
        // Inline aggregate fields contribute their own size and alignment, not a
        // pointer's. Getting this wrong made a record containing a String 8
        // bytes short of what C computed.
        let sizes: Vec<(u32, u32)> = fields
            .iter()
            .map(|f| match f.agg {
                Some(a) => {
                    let d = self.prog.agg(a);
                    (d.size, d.align)
                }
                None => (f.ty.size().max(1), f.ty.align()),
            })
            .collect();
        let (fields, size, align) = layout_sequential(fields, &sizes);
        let id = self.prog.aggs.len() as TypeId;
        self.prog.aggs.push(AggDef {
            name: name.to_string(),
            kind: AggKind::Record,
            fields,
            size,
            align,
        });
        self.aggs.insert(key.to_string(), id);
        id
    }

    fn build_str_agg(&mut self, key: &str) -> TypeId {
        let fields = vec![
            FieldDef {
                name: "data".into(),
                ty: IrTy::Ptr,
                agg: None,
                offset: 0,
                src: "ptr".into(),
            },
            FieldDef {
                name: "len".into(),
                ty: IrTy::U64,
                agg: None,
                offset: 0,
                src: "usz".into(),
            },
        ];
        self.record_agg(key, "str", fields)
    }

    fn build_slice_agg(&mut self, key: &str, elem: &Type) -> Result<TypeId, String> {
        // Element type is validated so a slice of an unsupported type fails
        // here rather than at a load site.
        let _ = self.ir_ty(elem)?;
        let fields = vec![
            FieldDef {
                name: "data".into(),
                ty: IrTy::Ptr,
                agg: None,
                offset: 0,
                src: "ptr".into(),
            },
            FieldDef {
                name: "len".into(),
                ty: IrTy::U64,
                agg: None,
                offset: 0,
                src: "usz".into(),
            },
        ];
        Ok(self.record_agg(key, "slice", fields))
    }

    fn build_vec_agg(&mut self, key: &str, elem: &Type) -> Result<TypeId, String> {
        let _ = self.ir_ty(elem)?;
        let fields = vec![
            FieldDef {
                name: "data".into(),
                ty: IrTy::Ptr,
                agg: None,
                offset: 0,
                src: "ptr".into(),
            },
            FieldDef {
                name: "len".into(),
                ty: IrTy::U64,
                agg: None,
                offset: 0,
                src: "usz".into(),
            },
            FieldDef {
                name: "cap".into(),
                ty: IrTy::U64,
                agg: None,
                offset: 0,
                src: "usz".into(),
            },
            // A Vec carries the allocator that owns its storage, so `push` can
            // grow without the call site threading a handle through — and so a
            // Vec allocated in a region cannot silently grow onto the heap.
            FieldDef {
                name: "alloc_kind".into(),
                ty: IrTy::I32,
                agg: None,
                offset: 0,
                src: "i32".into(),
            },
            FieldDef {
                name: "alloc_arena".into(),
                ty: IrTy::Ptr,
                agg: None,
                offset: 0,
                src: "ptr".into(),
            },
        ];
        Ok(self.record_agg(key, "vec", fields))
    }

    fn build_record(
        &mut self,
        key: &str,
        name: &str,
        fs: &[(Symbol, Type)],
    ) -> Result<TypeId, String> {
        // Reserve the id before recursing: a record may contain itself behind
        // a reference, and the recursive call must find the in-progress key.
        let mut fields = Vec::with_capacity(fs.len());
        for (n, ft) in fs {
            let ty = self.ir_ty(ft)?;
            let agg = if ty == IrTy::Ptr { self.agg_of(ft)? } else { None };
            fields.push(FieldDef {
                name: self.sym(*n),
                ty,
                agg: if matches!(ft, Type::Ref { .. } | Type::Own(_)) {
                    None
                } else {
                    agg
                },
                offset: 0,
                src: ft.display(self.intern),
            });
        }
        Ok(self.record_agg(key, name, fields))
    }

    fn build_tuple(&mut self, key: &str, names: &[String], ts: &[Type]) -> Result<TypeId, String> {
        let mut fields = Vec::with_capacity(ts.len());
        for (n, ft) in names.iter().zip(ts) {
            let ty = self.ir_ty(ft)?;
            let agg = if ty == IrTy::Ptr { self.agg_of(ft)? } else { None };
            fields.push(FieldDef {
                name: n.clone(),
                ty,
                agg: if matches!(ft, Type::Ref { .. } | Type::Own(_)) {
                    None
                } else {
                    agg
                },
                offset: 0,
                src: ft.display(self.intern),
            });
        }
        Ok(self.record_agg(key, "tuple", fields))
    }

    fn build_variant(
        &mut self,
        key: &str,
        name: &str,
        vs: &[(Symbol, Vec<(Symbol, Type)>)],
    ) -> Result<TypeId, String> {
        // Layout: i32 tag, then every case's payload overlapping at a common
        // base. Only one case is live, so the cases form a union.
        let mut fields = vec![FieldDef {
            name: "tag".into(),
            ty: IrTy::I32,
            agg: None,
            offset: 0,
            src: "i32".into(),
        }];
        let mut cases = Vec::with_capacity(vs.len());
        let mut per_case: Vec<Vec<(String, IrTy, Option<TypeId>, u32, u32, String)>> = Vec::new();
        for (_, fs) in vs {
            let mut row = Vec::new();
            for (n, ft) in fs {
                let ty = self.ir_ty(ft)?;
                let agg = if ty == IrTy::Ptr { self.agg_of(ft)? } else { None };
                let inline = if matches!(ft, Type::Ref { .. } | Type::Own(_)) {
                    None
                } else {
                    agg
                };
                let (size, align) = match inline {
                    Some(a) => (self.prog.agg(a).size, self.prog.agg(a).align),
                    None => (ty.size(), ty.align()),
                };
                row.push((self.sym(*n), ty, inline, size, align, ft.display(self.intern)));
            }
            per_case.push(row);
        }
        let payload_align = per_case
            .iter()
            .flat_map(|r| r.iter().map(|f| f.4))
            .max()
            .unwrap_or(1)
            .max(4);
        let base = align_up(4, payload_align);
        let mut total = base;
        for ((vn, _), row) in vs.iter().zip(&per_case) {
            let mut cursor = base;
            let mut idxs = Vec::new();
            for (fname, ty, inline, size, align, src) in row {
                let off = align_up(cursor, *align);
                cursor = off + size;
                idxs.push(fields.len() as u32);
                fields.push(FieldDef {
                    name: format!("{}_{}", self.sym(*vn), fname),
                    ty: *ty,
                    agg: *inline,
                    offset: off,
                    src: src.clone(),
                });
            }
            total = total.max(cursor);
            cases.push(VariantCase {
                name: self.sym(*vn),
                tag: cases.len() as i64,
                fields: idxs,
            });
        }
        let align = payload_align.max(4);
        let size = align_up(total.max(4), align);
        let id = self.prog.aggs.len() as TypeId;
        self.prog.aggs.push(AggDef {
            name: name.to_string(),
            kind: AggKind::Variant { cases },
            fields,
            size,
            align,
        });
        self.aggs.insert(key.to_string(), id);
        Ok(id)
    }

    // ---- functions ---------------------------------------------------

    fn lower_fn(
        &mut self,
        f: &CheckedFn,
        id: FuncId,
        subst_map: &HashMap<Symbol, Type>,
        dict_args: &[(u32, usize)],
    ) -> Result<Func, String> {
        let name = self.prog.funcs[id as usize].name.clone();
        let ret_ty = self.resolve(&f.sig.ret, subst_map);
        let ret_ir = self.ir_ty(&ret_ty)?;
        let ret_agg = if ret_ir == IrTy::Ptr {
            self.agg_of(&ret_ty)?
        } else {
            None
        };
        let mut fb = FuncBuilder::new(id, name, f.sig.def_id.clone(), ret_ir);
        fb.func.ret_agg = ret_agg;
        fb.func.ret_src = ret_ty.display(self.intern);
        fb.func.effects = f.inferred.clone();
        fb.func.pure = is_pure(&f.inferred);
        fb.func.bounded = !f.inferred.atoms.iter().any(|a| a == &EffectAtom::Diverge);
        // Worth caching? Only for a provably pure function whose recursion can
        // recompute the same subproblem many times.
        fb.func.memoize = worth_memoizing(f, &ret_ty);

        let err = self.err_channel(&f.inferred, subst_map)?;
        fb.func.err = err.clone();
        let same_len = same_len_pairs(&f.body, self.intern);

        let mut fl = FnLower {
            l: self,
            fb,
            scopes: vec![Vec::new()],
            subst: subst_map.clone(),
            ret_agg,
            ret_dest: None,
            err_dest: None,
            handlers: Vec::new(),
            regions: Vec::new(),
            dicts: HashMap::new(),
            loops: Vec::new(),
            recips: HashMap::new(),
            index_facts: Vec::new(),
            same_len,
            data_ptrs: HashMap::new(),
            test_fail: None,
        };

        // Params, then the hidden destination pointers in ABI order.
        for (i, (pname, pty, _)) in f.sig.params.iter().enumerate() {
            // A dictionary parameter is a compile-time witness, not a value.
            // Resolution is unique, so every call through it is statically known
            // and lowers to a direct call — a vtable would be pure overhead.
            if let Some((_, d)) = dict_args.iter().find(|(idx, _)| *idx == i as u32) {
                fl.dicts.insert(*pname, *d);
                continue;
            }
            let t = fl.l.resolve(pty, subst_map);
            let ir = fl.l.ir_ty(&t)?;
            let v = fl.fb.new_val(ir);
            fl.fb.func.params.push(v);
            let agg = if ir == IrTy::Ptr { fl.l.agg_of(&t)? } else { None };
            // Only spill when the body actually needs an address for it.
            let needs = param_needs_slot(&f.body, *pname);
            fl.bind_param(*pname, v, ir, agg, &t, needs);
            // Residual RC: a shared incoming heap value needs a retain so the
            // caller's reference cannot die during the call (§5.2.3).
            if ir == IrTy::Ptr {
                let fname = fl.l.intern.get(f.sig.name);
                if let Some(fo) = fl
                    .l
                    .co
                    .ownership
                    .functions
                    .iter()
                    .find(|x| x.function == fname)
                {
                    let pn = fl.l.intern.get(*pname);
                    if let Some(vs) = fo.values.iter().find(|v| v.name == pn) {
                        if matches!(
                            vs.strategy,
                            crate::ownership::Strategy::RcNonatomic
                                | crate::ownership::Strategy::RcAtomic
                        ) {
                            fl.fb.push_void(Op::RcRetain(v));
                        }
                    }
                }
            }
        }
        if ret_agg.is_some() {
            let v = fl.fb.new_val(IrTy::Ptr);
            fl.fb.func.params.push(v);
            fl.ret_dest = Some(v);
        }
        if err.is_some() {
            let v = fl.fb.new_val(IrTy::Ptr);
            fl.fb.func.params.push(v);
            fl.err_dest = Some(v);
        }

        fl.lower_contracts(&f.contracts, ContractKind::Pre)?;
        let body = fl.expr(&f.body)?;
        if !fl.fb.terminated() {
            fl.emit_return(body)?;
        }
        let mut func = fl.fb.finish();
        func.exported = self
            .co
            .exports
            .iter()
            .any(|e| e == &self.sym(f.sig.name))
            || self.sym(f.sig.name) == "main";
        Ok(func)
    }

    fn lower_test(
        &mut self,
        _name: String,
        body: &Expr,
        id: FuncId,
        def_id: &str,
    ) -> Result<Func, String> {
        // A test is a nullary function returning bool: true iff it passed.
        let fname = self.prog.funcs[id as usize].name.clone();
        let fb = FuncBuilder::new(id, fname, def_id.to_string(), IrTy::Bool);
        let mut fl = FnLower {
            l: self,
            fb,
            scopes: vec![Vec::new()],
            subst: HashMap::new(),
            ret_agg: None,
            ret_dest: None,
            err_dest: None,
            handlers: Vec::new(),
            regions: Vec::new(),
            dicts: HashMap::new(),
            loops: Vec::new(),
            recips: HashMap::new(),
            index_facts: Vec::new(),
            same_len: Vec::new(),
            data_ptrs: HashMap::new(),
            test_fail: None,
        };
        // A test that raises without catching fails, exactly as in the oracle.
        let fail = fl.fb.new_block();
        fl.test_fail = Some(fail);
        let v = fl.expr(body)?;
        if !fl.fb.terminated() {
            // `test "x" = assert(...)` yields unit; a test that runs to the end
            // without aborting has passed.
            let ok = if v.ty == IrTy::Bool {
                v.v
            } else {
                fl.fb.const_bool(true)
            };
            fl.fb.set_term(Term::Ret(Some(ok)));
        }
        fl.fb.switch_to(fail);
        let no = fl.fb.const_bool(false);
        fl.fb.set_term(Term::Ret(Some(no)));
        Ok(fl.fb.finish())
    }

    /// Error channel for a row. The payload is always an aggregate so the ABI
    /// is uniform; a scalar `err[T]` is boxed into a one-field record.
    fn err_channel(
        &mut self,
        eff: &EffectSet,
        subst_map: &HashMap<Symbol, Type>,
    ) -> Result<Option<ErrChannel>, String> {
        let mut found = None;
        for a in &eff.atoms {
            if let EffectAtom::Err(t) = a {
                found = Some(self.resolve(t, subst_map));
                break;
            }
        }
        let Some(t) = found else { return Ok(None) };
        let display = t.display(self.intern);
        let ir = self.ir_ty(&t)?;
        let agg = if ir == IrTy::Ptr { self.agg_of(&t)? } else { None };
        let agg = match agg {
            Some(a) => Some(a),
            None => {
                let key = format!("errbox<{display}>");
                if let Some(id) = self.aggs.get(&key) {
                    Some(*id)
                } else {
                    let fields = vec![FieldDef {
                        name: "value".into(),
                        ty: ir,
                        agg: None,
                        offset: 0,
                        src: display.clone(),
                    }];
                    Some(self.record_agg(&key, "errbox", fields))
                }
            }
        };
        Ok(Some(ErrChannel { ty: ir, agg, display }))
    }
}

/// Should this function cache its results?
///
/// Requires all of: an empty effect row (so the result depends on nothing but the
/// arguments, and the call cannot be observed), one or two integer parameters and
/// a scalar result (so a cache key is one or two machine words), and **more than
/// one** recursive call to itself — one recursive call is a linear walk that a
/// cache would only slow down, while two or more is the shape that recomputes
/// subproblems exponentially. Two arguments cover binomial coefficients and the
/// other classic tree recurrences; three or more is left alone because the key
/// would stop being a pair of registers.
fn worth_memoizing(f: &CheckedFn, ret: &Type) -> bool {
    if !f.inferred.is_empty() || !f.sig.generics.is_empty() {
        return false;
    }
    let n = f.sig.params.len();
    if n != 1 && n != 2 {
        return false;
    }
    let args_ok = f.sig.params.iter().all(|(_, t, is_dict)| {
        !*is_dict && t.as_prim().map(|p| p.is_int()).unwrap_or(false)
    });
    let ret_ok = ret.as_prim().map(|p| p.is_int()).unwrap_or(false);
    if !args_ok || !ret_ok {
        return false;
    }
    let mut calls = 0usize;
    count_self_calls(&f.body, f.sig.name, &mut calls);
    calls > 1
}

fn count_self_calls(e: &Expr, name: Symbol, n: &mut usize) {
    if let ExprKind::Call { callee, .. } = &e.kind {
        if let ExprKind::Path(p) = &callee.kind {
            if p.segs.len() == 1 && p.segs[0].name == name {
                *n += 1;
            }
        }
    }
    let mut f = |c: &Expr| count_self_calls(c, name, n);
    each_child(e, &mut f);
}

fn is_pure(eff: &EffectSet) -> bool {
    !eff.atoms.iter().any(|a| {
        matches!(
            a,
            EffectAtom::Io(_) | EffectAtom::Race | EffectAtom::Nondet | EffectAtom::Susp
        )
    })
}

fn placeholder(id: FuncId, name: String) -> Func {
    let mut fb = FuncBuilder::new(id, name, String::new(), IrTy::Unit);
    fb.set_term(Term::Unreachable);
    fb.finish()
}

fn prim_ir(p: Prim) -> IrTy {
    match p {
        Prim::I8 => IrTy::I8,
        Prim::I16 => IrTy::I16,
        Prim::I32 => IrTy::I32,
        Prim::I64 | Prim::Isz => IrTy::I64,
        Prim::U8 | Prim::Byte => IrTy::U8,
        Prim::U16 => IrTy::U16,
        Prim::U32 => IrTy::U32,
        Prim::U64 | Prim::Usz => IrTy::U64,
        Prim::F32 => IrTy::F32,
        Prim::F64 => IrTy::F64,
        Prim::Bool => IrTy::Bool,
        Prim::Unit => IrTy::Unit,
    }
}

fn align_up(v: u32, a: u32) -> u32 {
    let a = a.max(1);
    (v + a - 1) / a * a
}

/// Lay fields out in declaration order with natural alignment, the same rule a
/// C compiler applies to a struct with no packing attributes.
fn layout_sequential(
    mut fields: Vec<FieldDef>,
    sizes: &[(u32, u32)],
) -> (Vec<FieldDef>, u32, u32) {
    let mut cursor = 0u32;
    let mut align = 1u32;
    for (f, (size, al)) in fields.iter_mut().zip(sizes) {
        let al = (*al).max(1);
        f.offset = align_up(cursor, al);
        cursor = f.offset + size.max(&1);
        align = align.max(al);
    }
    (fields, align_up(cursor.max(1), align), align)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

/// A local binding: always addressable storage, so `&mut`, assignment, and
/// aggregates all work uniformly. SROA promotes the scalars back to registers.
#[derive(Clone, Copy)]
struct Local {
    addr: ValId,
    ir: IrTy,
    agg: Option<TypeId>,
    /// Aggregate parameters and references are already pointers: the binding
    /// holds the pointer itself rather than storage for a copy.
    by_ref: bool,
    /// Bound to a non-zero integer literal. The C compiler already turns
    /// division by a constant into a multiply; hoisting a reciprocal would
    /// only hide that from it.
    const_div: bool,
    /// Unique-heap pointer (today: `map.new`). Freed at last use with
    /// `UniqueFree` after releasing internal entries. Not set on values
    /// that escape the current frame.
    unique_heap: bool,
}

struct FnLower<'l, 'a> {
    l: &'l mut Lowerer<'a>,
    fb: FuncBuilder,
    scopes: Vec<Vec<(Symbol, Local)>>,
    subst: HashMap<Symbol, Type>,
    ret_agg: Option<TypeId>,
    ret_dest: Option<ValId>,
    err_dest: Option<ValId>,
    handlers: Vec<Handler>,
    regions: Vec<RegionIdx>,
    /// Dictionary-typed parameters bound to the dictionary the caller resolved.
    dicts: HashMap<Symbol, usize>,
    /// Enclosing loops, innermost last: `break` jumps to `exit`, `continue` to
    /// `head`. The checker has already rejected either outside a loop.
    loops: Vec<LoopTargets>,
    /// Reciprocals of loop-invariant unsigned 64-bit divisors, computed in the
    /// preheader and keyed by the local's address so a shadowed name cannot
    /// reuse the outer reciprocal. A hit replaces a body `udiv`/`urem` with a
    /// multiply-high.
    recips: HashMap<ValId, Recip>,
    /// Facts of the form "this loop variable is bounded by that container's
    /// length", collected at loop entry and used to drop provably-redundant
    /// bounds checks. See `bounded_by`.
    index_facts: Vec<(Symbol, Symbol)>,
    /// Pairs of vecs that were filled by lockstep `push` in the same loop and
    /// never reassigned, so they have equal length. `for i in range(0, a.len())`
    /// then also bounds `b.at(i)` / `b.set(i, _)`.
    same_len: Vec<(Symbol, Symbol)>,
    /// Hoisted `vec.data` pointers for the current loop. Valid only when the
    /// body cannot grow or reassign the vec, so the pointer cannot move.
    data_ptrs: HashMap<Symbol, ValId>,
    /// In a `test`, where an uncaught raise goes: the block that reports the
    /// test as failed. The oracle treats an uncaught raise the same way, so the
    /// two agree on which tests pass.
    test_fail: Option<BlockId>,
}

impl<'l, 'a> FnLower<'l, 'a> {
    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.pop_scope_keeping(None);
    }

    fn pop_scope_keeping(&mut self, keep: Option<Symbol>) {
        if let Some(s) = self.scopes.pop() {
            self.drop_unique_locals(&s, keep);
        }
    }

    /// Free unique-heap locals at last use. `keep` is a local that is the
    /// block's result (it escapes this scope; the caller owns the pointer).
    fn drop_unique_locals(&mut self, locals: &[(Symbol, Local)], keep: Option<Symbol>) {
        if self.fb.terminated() {
            return;
        }
        for (name, loc) in locals.iter().rev() {
            if !loc.unique_heap {
                continue;
            }
            if keep == Some(*name) {
                continue;
            }
            let ptr = if loc.by_ref {
                loc.addr
            } else {
                self.fb.load(IrTy::Ptr, loc.addr)
            };
            // Entries first, then the unique header. `map_free_entries` does
            // not free `ptr`; `UniqueFree` is the last use.
            self.fb.push_void(Op::CallExt {
                name: "ax_rt_map_free_entries".into(),
                args: vec![ptr],
                ret: IrTy::Unit,
                fallible: false,
            });
            self.fb.push_void(Op::UniqueFree(ptr));
        }
    }

    fn bind(&mut self, name: Symbol, local: Local) {
        if let Some(s) = self.scopes.last_mut() {
            s.push((name, local));
        }
    }

    fn lookup(&self, name: Symbol) -> Option<Local> {
        for s in self.scopes.iter().rev() {
            if let Some((_, l)) = s.iter().rev().find(|(n, _)| *n == name) {
                return Some(*l);
            }
        }
        None
    }

    /// Bind a parameter.
    ///
    /// A scalar parameter that the body never assigns to and never takes the
    /// address of stays an SSA value. Spilling every parameter to a slot made
    /// the entry of each function a store-then-reload pair that only survives
    /// because `clang -O2` promotes it back; a backend without that pass would
    /// pay for it, and the IR is smaller and clearer this way.
    fn bind_param(
        &mut self,
        name: Symbol,
        v: ValId,
        ir: IrTy,
        agg: Option<TypeId>,
        ty: &Type,
        needs_slot: bool,
    ) {
        let by_ref = ir == IrTy::Ptr;
        if by_ref {
            let _ = ty;
            self.bind(
                name,
                Local {
                    addr: v,
                    ir,
                    agg,
                    by_ref: true,
                    const_div: false,
                    unique_heap: false,
                },
            );
        } else if !needs_slot {
            self.bind(
                name,
                Local {
                    addr: v,
                    ir,
                    agg,
                    // The "address" is the value itself; `by_ref` marks that no
                    // load is needed to read it.
                    by_ref: true,
                    const_div: false,
                    unique_heap: false,
                },
            );
        } else {
            let slot = self
                .fb
                .alloc_slot(SlotKind::Scalar(ir), &format!("p{}", name.0));
            self.fb.store(ir, slot, v);
            self.bind(
                name,
                Local {
                    addr: slot,
                    ir,
                    agg,
                    by_ref: false,
                    const_div: false,
                    unique_heap: false,
                },
            );
        }
    }

    fn ty_of_node(&self, id: NodeId) -> Type {
        let t = self.l.co.ty(id).clone();
        if self.subst.is_empty() {
            t
        } else {
            subst(&t, &self.subst)
        }
    }

    fn ir_of_node(&mut self, id: NodeId) -> Result<(IrTy, Option<TypeId>), String> {
        let t = self.ty_of_node(id);
        let ir = self.l.ir_ty(&t)?;
        let agg = if ir == IrTy::Ptr {
            self.l.agg_of(&t)?
        } else {
            None
        };
        Ok((ir, agg))
    }

    /// Same as [`Self::ir_of_node`], but names the source location on failure.
    /// Lowering errors are compiler-internal, so they must be traceable to a
    /// node rather than reported as "somewhere in this program".
    fn ir_of(&mut self, e: &Expr) -> Result<(IrTy, Option<TypeId>), String> {
        self.ir_of_node(e.id).map_err(|err| {
            format!(
                "{}:{}: {err} (in {})",
                e.span.start, e.span.end, expr_kind_name(&e.kind)
            )
        })
    }

    /// Layout of the payload a `raise` here must produce: the innermost
    /// handler's, else the function's own error channel.
    fn error_agg(&self) -> Option<TypeId> {
        match self.handlers.last() {
            Some(h) => Some(h.agg),
            None => self.fb.func.err.as_ref().and_then(|c| c.agg),
        }
    }

    /// Does this pattern always match, binding at most a name?
    fn pat_is_trivial(&self, p: &Pattern) -> bool {
        match &p.kind {
            PatKind::Wild => true,
            PatKind::Bind(_) => !self.l.co.pat_variant.contains_key(&p.id),
            _ => false,
        }
    }

    /// Resolve a variant payload field named in a pattern or literal.
    ///
    /// Two spellings reach here: named (`Some { value: x }`) and positional
    /// (`Some(x)`, which the parser records as `_0`, `_1`, ...). Positional
    /// names index the case's declared field order.
    fn case_field(
        &self,
        agg: TypeId,
        case: &VariantCase,
        name: &str,
    ) -> Result<u32, String> {
        if let Some(rest) = name.strip_prefix('_') {
            if let Ok(i) = rest.parse::<usize>() {
                return case.fields.get(i).copied().ok_or_else(|| {
                    format!(
                        "native backend: `{}` has {} payload field(s), asked for #{i}",
                        case.name,
                        case.fields.len()
                    )
                });
            }
        }
        let full = format!("{}_{}", case.name, name);
        self.l
            .prog
            .agg(agg)
            .field_index(&full)
            .ok_or_else(|| format!("native backend: unknown field `{full}`"))
    }

    /// Layout of the error type a `catch` / `attempt` node handles.
    fn caught_agg(&mut self, id: NodeId) -> Result<TypeId, String> {
        let t = self
            .l
            .co
            .caught
            .get(&id)
            .cloned()
            .ok_or("internal: catch/attempt with no recorded error type")?;
        let t = if self.subst.is_empty() {
            t
        } else {
            subst(&t, &self.subst)
        };
        let ir = self.l.ir_ty(&t)?;
        match if ir == IrTy::Ptr {
            self.l.agg_of(&t)?
        } else {
            None
        } {
            Some(a) => Ok(a),
            None => {
                // Scalar error payloads are boxed so the channel is uniform.
                let key = format!("errbox<{}>", t.display(self.l.intern));
                if let Some(id) = self.l.aggs.get(&key) {
                    return Ok(*id);
                }
                let fields = vec![FieldDef {
                    name: "value".into(),
                    ty: ir,
                    agg: None,
                    offset: 0,
                    src: t.display(self.l.intern),
                }];
                Ok(self.l.record_agg(&key, "errbox", fields))
            }
        }
    }

    fn emit_return(&mut self, v: LVal) -> Result<(), String> {
        if let (Some(dest), Some(agg)) = (self.ret_dest, self.ret_agg) {
            self.fb.push_void(Op::CopyAgg {
                ty: agg,
                dst: dest,
                src: v.v,
            });
            self.fb.set_term(Term::Ret(None));
        } else if self.fb.func.ret == IrTy::Unit {
            self.fb.set_term(Term::Ret(None));
        } else {
            self.fb.set_term(Term::Ret(Some(v.v)));
        }
        Ok(())
    }

    fn lower_contracts(&mut self, cs: &[Contract], which: ContractKind) -> Result<(), String> {
        for c in cs.iter().filter(|c| c.kind == which) {
            let v = self.expr(&c.expr)?;
            let ok = self.fb.new_block();
            let code = match which {
                ContractKind::Post => AbortCode::ContractPost,
                _ => AbortCode::ContractPre,
            };
            let bad = self.fb.new_block();
            self.fb.set_term(Term::Br {
                cond: v.v,
                then_e: Edge {
                    to: ok,
                    args: vec![],
                },
                else_e: Edge {
                    to: bad,
                    args: vec![],
                },
            });
            self.fb.switch_to(bad);
            self.fb.set_term(Term::Abort(code));
            self.fb.switch_to(ok);
        }
        Ok(())
    }

    // ---- expressions -------------------------------------------------

    fn expr(&mut self, e: &Expr) -> Result<LVal, String> {
        match &e.kind {
            ExprKind::Lit(l) => self.lit(l, e),
            ExprKind::Path(p) => self.path(p, e),
            ExprKind::Block { stmts, tail } => {
                self.push_scope();
                for s in stmts {
                    match &s.kind {
                        StmtKind::Let(l) => self.let_stmt(l)?,
                        StmtKind::Expr(x) => {
                            self.expr(x)?;
                        }
                    }
                    if self.fb.terminated() {
                        break;
                    }
                }
                let out = if let Some(t) = tail {
                    if self.fb.terminated() {
                        self.undef_of(e)?
                    } else {
                        self.expr(t)?
                    }
                } else {
                    LVal::scalar(self.fb.unit(), IrTy::Unit)
                };
                // A tail that is just a unique-heap local escapes this scope.
                let keep = tail.as_ref().and_then(|t| match &t.kind {
                    ExprKind::Path(p) if p.segs.len() == 1 => Some(p.segs[0].name),
                    _ => None,
                });
                self.pop_scope_keeping(keep);
                Ok(out)
            }
            ExprKind::Let(l) => {
                self.let_stmt(l)?;
                Ok(LVal::scalar(self.fb.unit(), IrTy::Unit))
            }
            ExprKind::Unary { op, expr } => self.unary(*op, expr, e),
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e),
            ExprKind::If {
                cond,
                then_b,
                else_b,
            } => self.if_expr(cond, then_b, else_b.as_deref(), e),
            ExprKind::Assign { lhs, rhs } => self.assign(lhs, rhs),
            ExprKind::Field { base, field } => self.field(base, field, e),
            ExprKind::Index { base, index } => self.index(base, index, e),
            ExprKind::Record(fs) => self.record_lit(fs, e),
            ExprKind::Variant { name, fields } => self.variant_lit(name, fields, e),
            ExprKind::Match { scrut, arms } => self.match_expr(scrut, arms, e),
            ExprKind::For { pat, iter, body } => self.for_expr(pat, iter, body),
            ExprKind::Loop { body } => self.loop_expr(body, e),
            ExprKind::While { cond, body } => self.while_expr(cond, body),
            ExprKind::Break | ExprKind::Continue => {
                let target = self
                    .loops
                    .last()
                    .copied()
                    .ok_or("internal: loop control flow outside a loop")?;
                let to = if matches!(e.kind, ExprKind::Break) {
                    target.exit
                } else {
                    target.head
                };
                self.fb.set_term(Term::Jump(Edge { to, args: vec![] }));
                self.undef_of(e)
            }
            ExprKind::Cast { expr, .. } => self.cast_expr(expr, e),
            ExprKind::Return(inner) => {
                let v = match inner {
                    Some(i) => self.expr(i)?,
                    None => LVal::scalar(self.fb.unit(), IrTy::Unit),
                };
                self.emit_return(v)?;
                self.undef_of(e)
            }
            ExprKind::Raise(inner) => {
                let v = self.expr(inner)?;
                self.emit_raise(v)?;
                self.undef_of(e)
            }
            ExprKind::Catch { expr, arms } => self.catch_expr(expr, arms, e),
            ExprKind::Attempt(inner) => self.attempt_expr(inner, e),
            ExprKind::Try(inner) => {
                // `x?` is `attempt x` then unwrap-or-raise: lower as the
                // attempt form and immediately re-raise on Err. For a
                // non-Result the checker already treated `?` as identity.
                self.attempt_expr(inner, e)
            }
            ExprKind::Interpolate { parts } => {
                let mut last = self.undef_of(e)?;
                for p in parts {
                    match p {
                        crate::ast::InterpPart::Lit(s) => {
                            last = self.expr(&Expr {
                                id: NodeId::NONE,
                                kind: ExprKind::Lit(Lit::Str(s.clone())),
                                span: e.span,
                            })?;
                        }
                        crate::ast::InterpPart::Expr(x) => {
                            last = self.expr(x)?;
                        }
                    }
                }
                Ok(last)
            }
            ExprKind::Region { name, body } => self.region_expr(name, body),
            ExprKind::Call { callee, args } => self.call(callee, args, e),
            ExprKind::Lambda { params, ret, body } => {
                self.lambda(params, ret.as_ref(), body, e)
            }
            ExprKind::Par { bindings } => {
                // v0.3: structured concurrency. Disjointness was checked;
                // sequential evaluation is observationally identical when
                // captures do not alias (G1 deterministic core).
                for l in bindings {
                    let _ = self.let_stmt(l)?;
                }
                self.undef_of(e)
            },
            ExprKind::Hole => Err("native backend: program still contains a hole".into()),
        }
    }

    /// Lower a lambda to a top-level function and yield its address.
    ///
    /// Only non-capturing lambdas are supported: with captures a function
    /// pointer is not enough and the value would need an environment, which v1
    /// does not have. Comparators and callbacks — what the stdlib actually needs
    /// — do not capture.
    fn lambda(
        &mut self,
        params: &[Param],
        ret: Option<&TypeExpr>,
        body: &Expr,
        e: &Expr,
    ) -> Result<LVal, String> {
        let _ = ret;
        let mut captured = Vec::new();
        collect_free_names(body, &params.iter().map(|p| p.name.name).collect::<Vec<_>>(), &mut captured);
        for name in &captured {
            if self.lookup(*name).is_some() {
                let cap = self.l.sym(*name);
                return Err(format!(
                    "native backend: lambda captures `{cap}`; v1 function values do not carry an \
                     environment. Rewrite as an explicit parameter, e.g. `|x| x + {cap}` → \
                     `|x, {cap}| x + {cap}` and pass `{cap}` at the call site"
                ));
            }
        }

        // The lambda's type is on its node: parameters and result come from the
        // checker, not from re-reading the syntax.
        let fty = self.ty_of_node(e.id);
        let (ptys, rty) = match &fty {
            Type::Fn { params: ps, ret, .. } => (ps.clone(), (**ret).clone()),
            other => {
                return Err(format!(
                    "native backend: lambda has non-function type `{}`",
                    other.display(self.l.intern)
                ))
            }
        };
        let ret_ir = self.l.ir_ty(&rty)?;
        let ret_agg = if ret_ir == IrTy::Ptr {
            self.l.agg_of(&rty)?
        } else {
            None
        };
        let name = format!("ax_lambda_{}", e.id.0);
        let fid = self.l.reserve_func(name.clone());
        let mut fb = FuncBuilder::new(fid, name, format!("lambda:{}", e.id.0), ret_ir);
        fb.func.ret_agg = ret_agg;
        fb.func.ret_src = rty.display(self.l.intern);
        fb.func.pure = true;
        let mut inner = FnLower {
            l: self.l,
            fb,
            scopes: vec![Vec::new()],
            subst: self.subst.clone(),
            ret_agg,
            ret_dest: None,
            err_dest: None,
            handlers: Vec::new(),
            regions: Vec::new(),
            dicts: HashMap::new(),
            loops: Vec::new(),
            recips: HashMap::new(),
            index_facts: Vec::new(),
            same_len: Vec::new(),
            data_ptrs: HashMap::new(),
            test_fail: None,
        };
        for (p, pty) in params.iter().zip(&ptys) {
            let ir = inner.l.ir_ty(pty)?;
            let v = inner.fb.new_val(ir);
            inner.fb.func.params.push(v);
            let agg = if ir == IrTy::Ptr { inner.l.agg_of(pty)? } else { None };
            let needs = param_needs_slot(body, p.name.name);
            inner.bind_param(p.name.name, v, ir, agg, pty, needs);
        }
        if ret_agg.is_some() {
            let v = inner.fb.new_val(IrTy::Ptr);
            inner.fb.func.params.push(v);
            inner.ret_dest = Some(v);
        }
        let out = inner.expr(body)?;
        if !inner.fb.terminated() {
            inner.emit_return(out)?;
        }
        let func = inner.fb.finish();
        self.l.prog.funcs[fid as usize] = func;
        let addr = self.fb.push(Op::FuncAddr(fid), IrTy::Ptr);
        Ok(LVal::scalar(addr, IrTy::Ptr))
    }

    /// A value in a position control flow can never reach.
    fn undef_of(&mut self, e: &Expr) -> Result<LVal, String> {
        let (ir, agg) = self.ir_of(e)?;
        let v = match ir {
            IrTy::Unit => self.fb.unit(),
            IrTy::F32 | IrTy::F64 => self.fb.push(Op::ConstFloat(0.0), ir),
            IrTy::Bool => self.fb.const_bool(false),
            IrTy::Ptr => match agg {
                Some(a) => self.fb.alloc_slot(SlotKind::Agg(a), ""),
                None => self.fb.const_int(0, IrTy::Ptr),
            },
            _ => self.fb.const_int(0, ir),
        };
        Ok(LVal { v, ty: ir, agg })
    }

    fn lit(&mut self, l: &Lit, e: &Expr) -> Result<LVal, String> {
        let (ir, agg) = self.ir_of(e)?;
        Ok(match l {
            Lit::Int { value, .. } => LVal::scalar(self.fb.const_int(*value, ir), ir),
            Lit::Float { value, .. } => LVal::scalar(self.fb.push(Op::ConstFloat(*value), ir), ir),
            Lit::Bool(b) => LVal::scalar(self.fb.const_bool(*b), IrTy::Bool),
            Lit::Unit => LVal::scalar(self.fb.unit(), IrTy::Unit),
            Lit::Str(s) => {
                // A string literal is a static blob plus a length, materialised
                // into a str aggregate.
                let idx = self.l.intern_string(s);
                let agg = agg.ok_or("internal: string literal without str layout")?;
                let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
                let data = self.fb.push(Op::ConstStr(idx), IrTy::Ptr);
                let len = self.fb.const_int(s.len() as i128, IrTy::U64);
                let dp = self.fb.field_ptr(agg, STR_DATA, slot);
                self.fb.store(IrTy::Ptr, dp, data);
                let lp = self.fb.field_ptr(agg, STR_LEN, slot);
                self.fb.store(IrTy::U64, lp, len);
                LVal {
                    v: slot,
                    ty: IrTy::Ptr,
                    agg: Some(agg),
                }
            }
        })
    }

    fn path(&mut self, p: &Path, e: &Expr) -> Result<LVal, String> {
        let first = p.segs[0].name;
        if let Some(local) = self.lookup(first) {
            let mut cur = if local.by_ref {
                LVal {
                    v: local.addr,
                    ty: local.ir,
                    agg: local.agg,
                }
            } else if local.agg.is_some() {
                LVal {
                    v: local.addr,
                    ty: IrTy::Ptr,
                    agg: local.agg,
                }
            } else {
                let v = self.fb.load(local.ir, local.addr);
                LVal::scalar(v, local.ir)
            };
            // `a.b.c` where `a` is a local: walk the field path.
            for seg in &p.segs[1..] {
                cur = self.field_of(cur, &self.l.sym(seg.name))?;
            }
            return Ok(cur);
        }
        // A named function in value position: take its address.
        let dotted = path_text(p, self.l.intern);
        let bare = dotted.rsplit('.').next().unwrap_or(&dotted).to_string();
        if matches!(self.ty_of_node(e.id), Type::Fn { .. }) {
            if let Some(idx) = self
                .l
                .co
                .fns
                .iter()
                .position(|f| self.l.sym(f.sig.name) == bare)
            {
                let mangled = self.l.mangle(&bare, &[]);
                let fid = self.l.enqueue(idx, mangled, HashMap::new(), Vec::new());
                let addr = self.fb.push(Op::FuncAddr(fid), IrTy::Ptr);
                return Ok(LVal::scalar(addr, IrTy::Ptr));
            }
        }
        // A nullary variant constructor, e.g. `None` or `Zero`.
        let name = self.l.sym(first);
        let (ir, agg) = self.ir_of_node(e.id)?;
        if let Some(a) = agg {
            if let Some(case) = self.l.prog.agg(a).case(&name).cloned() {
                let slot = self.fb.alloc_slot(SlotKind::Agg(a), "");
                let tag = self.fb.const_int(case.tag as i128, IrTy::I32);
                self.fb.store(IrTy::I32, slot, tag);
                return Ok(LVal {
                    v: slot,
                    ty: IrTy::Ptr,
                    agg: Some(a),
                });
            }
        }
        let _ = ir;
        Err(format!(
            "native backend: unresolved name `{}`",
            path_text(p, self.l.intern)
        ))
    }

    fn unary(&mut self, op: UnOp, inner: &Expr, e: &Expr) -> Result<LVal, String> {
        match op {
            UnOp::Ref | UnOp::RefMut => {
                // Taking a reference is taking an address. Locals already have
                // one; aggregates are already addresses.
                if let ExprKind::Path(p) = &inner.kind {
                    if p.segs.len() == 1 {
                        if let Some(local) = self.lookup(p.segs[0].name) {
                            return Ok(LVal {
                                v: local.addr,
                                ty: IrTy::Ptr,
                                agg: local.agg,
                            });
                        }
                    }
                }
                let v = self.expr(inner)?;
                if v.ty == IrTy::Ptr {
                    return Ok(v);
                }
                // Reference to a temporary: spill it so the address is real.
                let slot = self.fb.alloc_slot(SlotKind::Scalar(v.ty), "");
                self.fb.store(v.ty, slot, v.v);
                Ok(LVal {
                    v: slot,
                    ty: IrTy::Ptr,
                    agg: None,
                })
            }
            UnOp::Deref => {
                let v = self.expr(inner)?;
                let (ir, agg) = self.ir_of_node(e.id)?;
                if ir == IrTy::Ptr && agg.is_some() {
                    return Ok(LVal {
                        v: v.v,
                        ty: ir,
                        agg,
                    });
                }
                let out = self.fb.load(ir, v.v);
                Ok(LVal::scalar(out, ir))
            }
            UnOp::Not | UnOp::BitNot => {
                let v = self.expr(inner)?;
                let k = if v.ty == IrTy::Bool {
                    UnKind::Not
                } else {
                    UnKind::BitNot
                };
                let out = self.fb.push(Op::Un { op: k, v: v.v }, v.ty);
                Ok(LVal::scalar(out, v.ty))
            }
            UnOp::Neg => {
                let v = self.expr(inner)?;
                let k = if v.ty.is_float() {
                    UnKind::FNeg
                } else {
                    UnKind::Neg
                };
                let out = self.fb.push(Op::Un { op: k, v: v.v }, v.ty);
                Ok(LVal::scalar(out, v.ty))
            }
        }
    }

    fn binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, e: &Expr) -> Result<LVal, String> {
        // `&&` / `||` short-circuit, so they are control flow, not operators.
        if matches!(op, BinOp::And | BinOp::Or) {
            let l = self.expr(lhs)?;
            let (rhs_b, _) = self.fb.new_block_with(&[]);
            let (join, params) = self.fb.new_block_with(&[IrTy::Bool]);
            let short = self.fb.const_bool(op == BinOp::Or);
            let cur_edge = Edge {
                to: join,
                args: vec![short],
            };
            if op == BinOp::And {
                self.fb.set_term(Term::Br {
                    cond: l.v,
                    then_e: Edge {
                        to: rhs_b,
                        args: vec![],
                    },
                    else_e: cur_edge,
                });
            } else {
                self.fb.set_term(Term::Br {
                    cond: l.v,
                    then_e: cur_edge,
                    else_e: Edge {
                        to: rhs_b,
                        args: vec![],
                    },
                });
            }
            self.fb.switch_to(rhs_b);
            let r = self.expr(rhs)?;
            if !self.fb.terminated() {
                self.fb.set_term(Term::Jump(Edge {
                    to: join,
                    args: vec![r.v],
                }));
            }
            self.fb.switch_to(join);
            return Ok(LVal::scalar(params[0], IrTy::Bool));
        }

        let l = self.expr(lhs)?;
        let r = self.expr(rhs)?;
        // Aggregate comparison (strings, records) goes through the runtime.
        if l.ty == IrTy::Ptr && l.agg.is_some() && matches!(op, BinOp::Eq | BinOp::Ne) {
            let agg = l.agg.unwrap();
            let name = self.l.prog.agg(agg).name.clone();
            let eq = if name == "str" {
                self.fb.push(
                    Op::CallExt {
                        name: "ax_rt_str_eq".into(),
                        args: vec![l.v, r.v],
                        ret: IrTy::Bool,
                        fallible: false,
                    },
                    IrTy::Bool,
                )
            } else {
                let size = self.l.prog.agg(agg).size;
                let n = self.fb.const_int(size as i128, IrTy::U64);
                self.fb.push(
                    Op::CallExt {
                        name: "ax_rt_mem_eq".into(),
                        args: vec![l.v, r.v, n],
                        ret: IrTy::Bool,
                        fallible: false,
                    },
                    IrTy::Bool,
                )
            };
            let out = if op == BinOp::Ne {
                self.fb.push(
                    Op::Un {
                        op: UnKind::Not,
                        v: eq,
                    },
                    IrTy::Bool,
                )
            } else {
                eq
            };
            return Ok(LVal::scalar(out, IrTy::Bool));
        }

        let float = l.ty.is_float();
        let kind = match op {
            BinOp::Add if float => BinKind::FAdd,
            BinOp::Sub if float => BinKind::FSub,
            BinOp::Mul if float => BinKind::FMul,
            BinOp::Div if float => BinKind::FDiv,
            BinOp::Rem if float => BinKind::FRem,
            BinOp::Add => BinKind::Add,
            BinOp::Sub => BinKind::Sub,
            BinOp::Mul => BinKind::Mul,
            BinOp::Div | BinOp::Rem => {
                // Integer `/` and `%` raise on a zero divisor — unless the
                // checker proved the divisor non-zero. Two strengths of proof:
                //
                // - unconditional (a non-zero literal, or a `range(lo, _)`
                //   variable with `lo >= 1`): nothing to check, so emit a bare
                //   machine divide. This used to emit a guard anyway, which cost a
                //   compare and a branch per division in every hot loop and
                //   contradicted the analysis that had just proved it dead.
                // - resting on the absence of wrap-around (the divisor was reached
                //   through `d = d + k`): keep the guard, because the case the
                //   analysis waved away would otherwise be a wrong answer.
                if self.l.co.nonzero_div.contains(&rhs.id) {
                    if self.l.co.nonzero_div_needs_guard.contains(&rhs.id) {
                        return self.guarded_div(op, l, r);
                    }
                    if let Some(v) = self.try_recip_div(op, l, rhs) {
                        return Ok(LVal::scalar(v, l.ty));
                    }
                    // Unsigned `n % 2^k` is `n & (2^k - 1)`. clang does this;
                    // we have to, because we emit a helper for `%` and clang
                    // will not rewrite a runtime call.
                    if op == BinOp::Rem && !l.ty.is_signed() {
                        if let ExprKind::Lit(Lit::Int { value, .. }) = &rhs.kind {
                            if *value > 0 && (*value as u64).is_power_of_two() {
                                let mask = self.fb.const_int(*value - 1, l.ty);
                                let v = self.fb.bin(BinKind::And, l.v, mask);
                                return Ok(LVal::scalar(v, l.ty));
                            }
                        }
                    }
                    let kind = if op == BinOp::Div {
                        BinKind::DivTruncNZ
                    } else {
                        BinKind::RemTruncNZ
                    };
                    let v = self.fb.bin(kind, l.v, r.v);
                    return Ok(LVal::scalar(v, l.ty));
                }
                return self.checked_div(op, l, r, e);
            }
            BinOp::BitAnd => BinKind::And,
            BinOp::BitOr => BinKind::Or,
            BinOp::BitXor => BinKind::Xor,
            BinOp::Shl => BinKind::Shl,
            BinOp::Shr => BinKind::Shr,
            BinOp::Eq => BinKind::Eq,
            BinOp::Ne => BinKind::Ne,
            BinOp::Lt => BinKind::Lt,
            BinOp::Le => BinKind::Le,
            BinOp::Gt => BinKind::Gt,
            BinOp::Ge => BinKind::Ge,
            BinOp::And | BinOp::Or => unreachable!("handled above"),
        };
        let out = self.fb.bin(kind, l.v, r.v);
        let ty = if kind.is_cmp() { IrTy::Bool } else { l.ty };
        // Floats canonicalise NaN so every backend agrees bit-for-bit.
        let out = if float && !kind.is_cmp() {
            self.fb.push(
                Op::Un {
                    op: UnKind::CanonNaN,
                    v: out,
                },
                ty,
            )
        } else {
            out
        };
        Ok(LVal::scalar(out, ty))
    }

    /// `a / b` and `a % b` on integers: raise `DivError.Zero` when `b == 0`.
    fn checked_div(&mut self, op: BinOp, l: LVal, r: LVal, e: &Expr) -> Result<LVal, String> {
        let zero = self.fb.const_int(0, r.ty);
        let is_zero = self.fb.bin(BinKind::Eq, r.v, zero);
        let raise_b = self.fb.new_block();
        let ok_b = self.fb.new_block();
        self.fb.set_term(Term::Br {
            cond: is_zero,
            then_e: Edge {
                to: raise_b,
                args: vec![],
            },
            else_e: Edge {
                to: ok_b,
                args: vec![],
            },
        });
        self.fb.switch_to(raise_b);
        let payload = self.div_error_zero()?;
        self.emit_raise(payload)?;
        self.fb.switch_to(ok_b);
        // Reached only when the divisor is non-zero, so the backend may divide
        // without re-testing it.
        let kind = if op == BinOp::Div {
            BinKind::DivTruncNZ
        } else {
            BinKind::RemTruncNZ
        };
        let out = self.fb.bin(kind, l.v, r.v);
        let _ = e;
        Ok(LVal::scalar(out, l.ty))
    }

    /// Division whose divisor the checker proved non-zero.
    ///
    /// The proof means no `err[DivError]` in the row and no raise path, so the
    /// function keeps an ordinary return type instead of the two-value fallible
    /// ABI. A guard remains — the proof reasons about values, and wrapping
    /// arithmetic could in principle defeat it after an astronomical number of
    /// iterations — but it aborts rather than raising, and never fires in
    /// practice.
    fn guarded_div(&mut self, op: BinOp, l: LVal, r: LVal) -> Result<LVal, String> {
        let zero = self.fb.const_int(0, r.ty);
        let is_zero = self.fb.bin(BinKind::Eq, r.v, zero);
        let bad = self.fb.new_block();
        let ok_b = self.fb.new_block();
        self.fb.set_term(Term::Br {
            cond: is_zero,
            then_e: Edge { to: bad, args: vec![] },
            else_e: Edge { to: ok_b, args: vec![] },
        });
        self.fb.switch_to(bad);
        self.fb.set_term(Term::Abort(AbortCode::DivExactZero));
        self.fb.switch_to(ok_b);
        let kind = if op == BinOp::Div {
            BinKind::DivTruncNZ
        } else {
            BinKind::RemTruncNZ
        };
        let v = self.fb.bin(kind, l.v, r.v);
        Ok(LVal::scalar(v, l.ty))
    }

    /// Build a `DivError.Zero` payload in the shape the error channel expects.
    fn div_error_zero(&mut self) -> Result<LVal, String> {
        let agg = self.error_agg().ok_or_else(|| {
            "native backend: integer div/rem needs a declared err[DivError] row".to_string()
        })?;
        let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
        let tag = match self.l.prog.agg(agg).case("Zero") {
            Some(c) => c.tag,
            None => 0,
        };
        let t = self.fb.const_int(tag as i128, IrTy::I32);
        self.fb.store(IrTy::I32, slot, t);
        Ok(LVal {
            v: slot,
            ty: IrTy::Ptr,
            agg: Some(agg),
        })
    }

    /// Route a raised payload to the innermost handler, or out of the function.
    fn emit_raise(&mut self, payload: LVal) -> Result<(), String> {
        let dest = self.err_dest;
        let channel_agg = self.fb.func.err.as_ref().and_then(|c| c.agg);
        match self.handlers.last().copied() {
            Some(h) => {
                // The handler reads the payload from its own slot.
                self.fb.push_void(Op::CopyAgg {
                    ty: h.agg,
                    dst: h.slot,
                    src: payload.v,
                });
                self.fb.set_term(Term::Jump(Edge {
                    to: h.block,
                    args: vec![],
                }));
            }
            None => match (dest, channel_agg) {
                (Some(d), Some(a)) => {
                    self.fb.push_void(Op::CopyAgg {
                        ty: a,
                        dst: d,
                        src: payload.v,
                    });
                    let tag = self.fb.const_int(1, IrTy::I32);
                    self.fb.set_term(Term::RetErr(tag));
                }
                _ => self.emit_uncaught_raise(),
            },
        }
        Ok(())
    }

    /// A raise with nowhere to go. In a test it fails the test; anywhere else it
    /// aborts, which is defined behaviour rather than an IR violation.
    fn emit_uncaught_raise(&mut self) {
        match self.test_fail {
            Some(b) => self.fb.set_term(Term::Jump(Edge {
                to: b,
                args: vec![],
            })),
            None => self.fb.set_term(Term::Abort(AbortCode::UncaughtRaise)),
        }
    }

    fn let_stmt(&mut self, l: &LetStmt) -> Result<(), String> {
        let init = self.expr(&l.init)?;
        if self.fb.terminated() {
            return Ok(());
        }
        self.bind_pattern(&l.pat, init, l.mutable)?;
        // `map.new` is a unique-heap pointer. Mark the binding so last-use
        // can emit UniqueFree. Copies / aliases stay unmarked (conservative).
        if let PatKind::Bind(id) = &l.pat.kind {
            if expr_is_map_new(&l.init, self.l.intern) {
                if let Some(scope) = self.scopes.last_mut() {
                    if let Some((_, loc)) = scope.iter_mut().rev().find(|(n, _)| *n == id.name) {
                        loc.unique_heap = true;
                    }
                }
            }
        }
        // A non-zero integer literal binding is a compile-time constant
        // divisor: clang already strength-reduces `% 7`, so the reciprocal
        // hoist would only hide that and lose. Mark it so `hoist_recips`
        // leaves it alone.
        if !l.mutable {
            if let (PatKind::Bind(id), ExprKind::Lit(Lit::Int { value, .. })) =
                (&l.pat.kind, &l.init.kind)
            {
                if *value != 0 {
                    if let Some(scope) = self.scopes.last_mut() {
                        if let Some((_, loc)) = scope.iter_mut().rev().find(|(n, _)| *n == id.name)
                        {
                            loc.const_div = true;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Bind a pattern irrefutably (let / for / params). Refutable matching is
    /// `match_pat_test`.
    fn bind_pattern(&mut self, pat: &Pattern, val: LVal, mutable: bool) -> Result<(), String> {
        match &pat.kind {
            PatKind::Wild | PatKind::Lit(_) => Ok(()),
            PatKind::Bind(_) if self.l.co.pat_variant.contains_key(&pat.id) => Ok(()),
            PatKind::Bind(id) => {
                if val.agg.is_some() {
                    // Copy-on-conflict ([R-3.3.1]): an aggregate bind is a
                    // copy. Aliasing the initialiser made `let y = x; x.s = 6`
                    // mutate `y` in native while the oracle copied — inverted
                    // E0382 (T-INV-0010) caught it. A later last-use pass may
                    // elide the copy; sharing storage is never correct when
                    // the source is used again.
                    let a = val.agg.unwrap();
                    let slot = self
                        .fb
                        .alloc_slot(SlotKind::Agg(a), &self.l.sym(id.name));
                    self.fb.push_void(Op::CopyAgg {
                        ty: a,
                        dst: slot,
                        src: val.v,
                    });
                    self.bind(
                        id.name,
                        Local {
                            addr: slot,
                            ir: IrTy::Ptr,
                            agg: val.agg,
                            by_ref: true,
                            const_div: false,
                            unique_heap: false,
                        },
                    );
                    let _ = mutable;
                } else if val.ty == IrTy::Ptr {
                    // A reference binding: keep the pointer itself.
                    let slot = self
                        .fb
                        .alloc_slot(SlotKind::Scalar(IrTy::Ptr), &self.l.sym(id.name));
                    self.fb.store(IrTy::Ptr, slot, val.v);
                    self.bind(
                        id.name,
                        Local {
                            addr: slot,
                            ir: IrTy::Ptr,
                            agg: None,
                            by_ref: false,
                            const_div: false,
                            unique_heap: false,
                        },
                    );
                } else {
                    let slot = self
                        .fb
                        .alloc_slot(SlotKind::Scalar(val.ty), &self.l.sym(id.name));
                    self.fb.store(val.ty, slot, val.v);
                    self.bind(
                        id.name,
                        Local {
                            addr: slot,
                            ir: val.ty,
                            agg: None,
                            by_ref: false,
                            const_div: false,
                            unique_heap: false,
                        },
                    );
                }
                Ok(())
            }
            PatKind::Record(fs) => {
                for (n, sub) in fs {
                    let f = self.field_of(val, &self.l.sym(n.name))?;
                    self.bind_pattern(sub, f, mutable)?;
                }
                Ok(())
            }
            PatKind::Tuple(ps) => {
                let agg = val.agg.ok_or("internal: tuple pattern on a non-aggregate")?;
                for (i, sub) in ps.iter().enumerate() {
                    let f = self.field_index(val, agg, i as u32)?;
                    self.bind_pattern(sub, f, mutable)?;
                }
                Ok(())
            }
            PatKind::Variant { name, fields } => {
                let agg = val.agg.ok_or("internal: variant pattern on a non-aggregate")?;
                let case = self
                    .l
                    .prog
                    .agg(agg)
                    .case(&self.l.sym(name.name))
                    .cloned()
                    .ok_or_else(|| {
                        format!("native backend: unknown variant `{}`", self.l.sym(name.name))
                    })?;
                for (n, sub) in fields {
                    let idx = self.case_field(agg, &case, &self.l.sym(n.name))?;
                    let f = self.field_index(val, agg, idx)?;
                    self.bind_pattern(sub, f, mutable)?;
                }
                Ok(())
            }
        }
    }

    fn field(&mut self, base: &Expr, field: &Ident, e: &Expr) -> Result<LVal, String> {
        // Some qualified names are nullary *values*, not field accesses:
        // `test.alloc` is an allocator handle, and its `test` prefix is not an
        // expression at all.
        if let Some(q) = callee_name(e, self.l.intern) {
            if let Some(v) = self.try_builtin(&q, &[], e)? {
                return Ok(v);
            }
        }
        let b = self.expr(base)?;
        self.field_of(b, &self.l.sym(field.name))
    }

    fn field_of(&mut self, base: LVal, name: &str) -> Result<LVal, String> {
        let agg = base
            .agg
            .ok_or_else(|| format!("native backend: field `{name}` on a non-aggregate"))?;
        let idx = self
            .l
            .prog
            .agg(agg)
            .field_index(name)
            .or_else(|| {
                // Variant payload fields are stored as `Case_field`.
                let a = self.l.prog.agg(agg);
                match &a.kind {
                    AggKind::Variant { cases } => cases.iter().find_map(|c| {
                        a.field_index(&format!("{}_{}", c.name, name))
                    }),
                    AggKind::Record => None,
                }
            })
            .ok_or_else(|| format!("native backend: no field `{name}`"))?;
        self.field_index(base, agg, idx)
    }

    fn field_index(&mut self, base: LVal, agg: TypeId, idx: u32) -> Result<LVal, String> {
        let f = self.l.prog.agg(agg).field(idx).clone();
        let ptr = self.fb.field_ptr(agg, idx, base.v);
        if let Some(inner) = f.agg {
            // Nested aggregate: the field's address *is* the value.
            Ok(LVal {
                v: ptr,
                ty: IrTy::Ptr,
                agg: Some(inner),
            })
        } else {
            let v = self.fb.load(f.ty, ptr);
            Ok(LVal::scalar(v, f.ty))
        }
    }

    fn index(&mut self, base: &Expr, index: &Expr, e: &Expr) -> Result<LVal, String> {
        let b = self.expr(base)?;
        let i = self.expr(index)?;
        let (ir, agg) = self.ir_of_node(e.id)?;
        let b_agg = b
            .agg
            .ok_or("native backend: indexing a value with no layout")?;
        let a = self.l.prog.agg(b_agg).clone();
        let len_idx = a
            .field_index("len")
            .ok_or("native backend: indexing a type with no `len`")?;
        let data_idx = a
            .field_index("data")
            .ok_or("native backend: indexing a type with no `data`")?;
        // `at` is bounds-checked always (§3.3), so the check is unconditional
        // and the abort is explicit in the IR.
        let len_ptr = self.fb.field_ptr(b_agg, len_idx, b.v);
        let len = self.fb.load(a.field(len_idx).ty, len_ptr);
        let idx64 = self.coerce_int(i, IrTy::U64);
        let in_bounds = self.fb.bin(BinKind::Lt, idx64, len);
        let ok = self.fb.new_block();
        let bad = self.fb.new_block();
        self.fb.set_term(Term::Br {
            cond: in_bounds,
            then_e: Edge {
                to: ok,
                args: vec![],
            },
            else_e: Edge {
                to: bad,
                args: vec![],
            },
        });
        self.fb.switch_to(bad);
        self.fb.set_term(Term::Abort(AbortCode::IndexOutOfBounds));
        self.fb.switch_to(ok);
        let data_ptr = self.fb.field_ptr(b_agg, data_idx, b.v);
        let data = self.fb.load(IrTy::Ptr, data_ptr);
        let ep = self.fb.push(
            Op::ElemPtr {
                elem: match agg {
                    Some(a) => Repr::Agg(a),
                    None => Repr::Scalar(ir),
                },
                ptr: data,
                idx: idx64,
            },
            IrTy::Ptr,
        );
        if let Some(inner) = agg {
            Ok(LVal {
                v: ep,
                ty: IrTy::Ptr,
                agg: Some(inner),
            })
        } else {
            let v = self.fb.load(ir, ep);
            Ok(LVal::scalar(v, ir))
        }
    }

    /// Widen/narrow an integer value to `to` without changing its value.
    fn coerce_int(&mut self, v: LVal, to: IrTy) -> ValId {
        if v.ty == to || !v.ty.is_int() {
            return v.v;
        }
        let kind = if v.ty.bits() > to.bits() {
            CastKind::Trunc
        } else if v.ty.is_signed() {
            CastKind::SExt
        } else {
            CastKind::ZExt
        };
        self.fb.push(Op::Cast { kind, v: v.v }, to)
    }

    fn record_lit(&mut self, fs: &[(Ident, Expr)], e: &Expr) -> Result<LVal, String> {
        let (_, agg) = self.ir_of(e)?;
        let agg = agg.ok_or("native backend: record literal with no layout")?;
        let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
        for (n, ex) in fs {
            let v = self.expr(ex)?;
            let name = self.l.sym(n.name);
            let idx = self
                .l
                .prog
                .agg(agg)
                .field_index(&name)
                .ok_or_else(|| format!("native backend: no field `{name}` in record literal"))?;
            self.store_field(slot, agg, idx, v)?;
        }
        Ok(LVal {
            v: slot,
            ty: IrTy::Ptr,
            agg: Some(agg),
        })
    }

    fn store_field(
        &mut self,
        base: ValId,
        agg: TypeId,
        idx: u32,
        v: LVal,
    ) -> Result<(), String> {
        let f = self.l.prog.agg(agg).field(idx).clone();
        let ptr = self.fb.field_ptr(agg, idx, base);
        match f.agg {
            Some(inner) => self.fb.push_void(Op::CopyAgg {
                ty: inner,
                dst: ptr,
                src: v.v,
            }),
            None => self.fb.store(f.ty, ptr, v.v),
        }
        Ok(())
    }

    fn variant_lit(
        &mut self,
        name: &Ident,
        fields: &[(Ident, Expr)],
        e: &Expr,
    ) -> Result<LVal, String> {
        let (_, agg) = self.ir_of(e)?;
        let agg = agg.ok_or("native backend: variant literal with no layout")?;
        // `P { .. }` on a record type is a record literal; the checker already
        // resolved which one this is, and the layout kind tells us here.
        if matches!(self.l.prog.agg(agg).kind, AggKind::Record) {
            return self.record_lit(fields, e);
        }
        let vname = self.l.sym(name.name);
        let case = self
            .l
            .prog
            .agg(agg)
            .case(&vname)
            .cloned()
            .ok_or_else(|| format!("native backend: unknown variant `{vname}`"))?;
        let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
        let tag = self.fb.const_int(case.tag as i128, IrTy::I32);
        self.fb.store(IrTy::I32, slot, tag);
        for (n, ex) in fields {
            let v = self.expr(ex)?;
            let idx = self.case_field(agg, &case, &self.l.sym(n.name))?;
            self.store_field(slot, agg, idx, v)?;
        }
        Ok(LVal {
            v: slot,
            ty: IrTy::Ptr,
            agg: Some(agg),
        })
    }

    fn assign(&mut self, lhs: &Expr, rhs: &Expr) -> Result<LVal, String> {
        let v = self.expr(rhs)?;
        let place = self.place(lhs)?;
        match place {
            Place::Scalar { addr, ty } => self.fb.store(ty, addr, v.v),
            Place::Agg { addr, agg } => self.fb.push_void(Op::CopyAgg {
                ty: agg,
                dst: addr,
                src: v.v,
            }),
        }
        Ok(LVal::scalar(self.fb.unit(), IrTy::Unit))
    }

    /// Address of an assignable location.
    fn place(&mut self, e: &Expr) -> Result<Place, String> {
        match &e.kind {
            ExprKind::Path(p) if p.segs.len() == 1 => {
                let local = self
                    .lookup(p.segs[0].name)
                    .ok_or_else(|| format!("native backend: assignment to unknown local"))?;
                if let Some(a) = local.agg {
                    Ok(Place::Agg {
                        addr: local.addr,
                        agg: a,
                    })
                } else if local.by_ref {
                    Ok(Place::Scalar {
                        addr: local.addr,
                        ty: local.ir,
                    })
                } else {
                    Ok(Place::Scalar {
                        addr: local.addr,
                        ty: local.ir,
                    })
                }
            }
            ExprKind::Path(p) => {
                // `a.b = v` written as a path.
                let base = self.path(&Path {
                    segs: p.segs[..p.segs.len() - 1].to_vec(),
                    span: p.span,
                }, e)?;
                let name = self.l.sym(p.segs[p.segs.len() - 1].name);
                self.field_place(base, &name)
            }
            ExprKind::Field { base, field } => {
                let b = self.expr(base)?;
                let name = self.l.sym(field.name);
                self.field_place(b, &name)
            }
            ExprKind::Unary {
                op: UnOp::Deref,
                expr,
            } => {
                let v = self.expr(expr)?;
                let (ir, agg) = self.ir_of_node(e.id)?;
                match agg {
                    Some(a) => Ok(Place::Agg { addr: v.v, agg: a }),
                    None => Ok(Place::Scalar { addr: v.v, ty: ir }),
                }
            }
            ExprKind::Index { base, index } => {
                let b = self.expr(base)?;
                let i = self.expr(index)?;
                let (ir, agg) = self.ir_of_node(e.id)?;
                let b_agg = b.agg.ok_or("native backend: indexed assign to a non-slice")?;
                let a = self.l.prog.agg(b_agg).clone();
                let data_idx = a
                    .field_index("data")
                    .ok_or("native backend: indexed assign to a type with no `data`")?;
                let len_idx = a
                    .field_index("len")
                    .ok_or("native backend: indexed assign to a type with no `len`")?;
                let lp = self.fb.field_ptr(b_agg, len_idx, b.v);
                let len = self.fb.load(a.field(len_idx).ty, lp);
                let idx64 = self.coerce_int(i, IrTy::U64);
                let in_bounds = self.fb.bin(BinKind::Lt, idx64, len);
                let ok = self.fb.new_block();
                let bad = self.fb.new_block();
                self.fb.set_term(Term::Br {
                    cond: in_bounds,
                    then_e: Edge {
                        to: ok,
                        args: vec![],
                    },
                    else_e: Edge {
                        to: bad,
                        args: vec![],
                    },
                });
                self.fb.switch_to(bad);
                self.fb.set_term(Term::Abort(AbortCode::IndexOutOfBounds));
                self.fb.switch_to(ok);
                let dp = self.fb.field_ptr(b_agg, data_idx, b.v);
                let data = self.fb.load(IrTy::Ptr, dp);
                let ep = self.fb.push(
                    Op::ElemPtr {
                        elem: match agg {
                            Some(a) => Repr::Agg(a),
                            None => Repr::Scalar(ir),
                        },
                        ptr: data,
                        idx: idx64,
                    },
                    IrTy::Ptr,
                );
                match agg {
                    Some(a) => Ok(Place::Agg { addr: ep, agg: a }),
                    None => Ok(Place::Scalar { addr: ep, ty: ir }),
                }
            }
            _ => Err("native backend: expression is not assignable".into()),
        }
    }

    fn field_place(&mut self, base: LVal, name: &str) -> Result<Place, String> {
        let agg = base
            .agg
            .ok_or_else(|| format!("native backend: field assign `{name}` on a non-aggregate"))?;
        let idx = self
            .l
            .prog
            .agg(agg)
            .field_index(name)
            .ok_or_else(|| format!("native backend: no field `{name}`"))?;
        let f = self.l.prog.agg(agg).field(idx).clone();
        let ptr = self.fb.field_ptr(agg, idx, base.v);
        match f.agg {
            Some(inner) => Ok(Place::Agg {
                addr: ptr,
                agg: inner,
            }),
            None => Ok(Place::Scalar { addr: ptr, ty: f.ty }),
        }
    }

    fn if_expr(
        &mut self,
        cond: &Expr,
        then_b: &Expr,
        else_b: Option<&Expr>,
        e: &Expr,
    ) -> Result<LVal, String> {
        let c = self.expr(cond)?;
        let (ir, agg) = self.ir_of(e)?;
        let yields = ir != IrTy::Unit;
        let then_bb = self.fb.new_block();
        let else_bb = self.fb.new_block();
        let (join, params) = if yields {
            self.fb.new_block_with(&[ir])
        } else {
            (self.fb.new_block(), Vec::new())
        };
        self.fb.set_term(Term::Br {
            cond: c.v,
            then_e: Edge {
                to: then_bb,
                args: vec![],
            },
            else_e: Edge {
                to: else_bb,
                args: vec![],
            },
        });

        self.fb.switch_to(then_bb);
        let tv = self.expr(then_b)?;
        let mut reaches_join = false;
        if !self.fb.terminated() {
            reaches_join = true;
            self.fb.set_term(Term::Jump(Edge {
                to: join,
                args: if yields { vec![tv.v] } else { vec![] },
            }));
        }

        self.fb.switch_to(else_bb);
        let ev = match else_b {
            Some(x) => self.expr(x)?,
            None => LVal::scalar(self.fb.unit(), IrTy::Unit),
        };
        if !self.fb.terminated() {
            reaches_join = true;
            self.fb.set_term(Term::Jump(Edge {
                to: join,
                args: if yields { vec![ev.v] } else { vec![] },
            }));
        }

        self.fb.switch_to(join);
        if !reaches_join {
            // Both branches diverged: nothing follows this `if`.
            self.fb.seal();
        }
        if yields {
            Ok(LVal {
                v: params[0],
                ty: ir,
                agg,
            })
        } else {
            Ok(LVal::scalar(self.fb.unit(), IrTy::Unit))
        }
    }

    fn match_expr(&mut self, scrut: &Expr, arms: &[Arm], e: &Expr) -> Result<LVal, String> {
        let s = self.expr(scrut)?;
        let (ir, agg) = self.ir_of(e)?;
        let yields = ir != IrTy::Unit;
        let (join, params) = if yields {
            self.fb.new_block_with(&[ir])
        } else {
            (self.fb.new_block(), Vec::new())
        };
        let fail = self.fb.new_block();

        // Arms are tried in order, so the lowering is a chain of tests. A
        // variant scrutinee with only variant patterns could switch on the tag;
        // the chain keeps oracle-identical first-match semantics either way and
        // clang turns dense equality chains into a switch.
        let mut reaches_join = false;
        for arm in arms {
            let next = self.fb.new_block();
            let matched = self.match_test(&arm.pat, s)?;
            let body_bb = self.fb.new_block();
            self.fb.set_term(Term::Br {
                cond: matched,
                then_e: Edge {
                    to: body_bb,
                    args: vec![],
                },
                else_e: Edge {
                    to: next,
                    args: vec![],
                },
            });
            self.fb.switch_to(body_bb);
            self.push_scope();
            self.bind_pattern(&arm.pat, s, false)?;
            let v = self.expr(&arm.body)?;
            self.pop_scope();
            if !self.fb.terminated() {
                reaches_join = true;
                self.fb.set_term(Term::Jump(Edge {
                    to: join,
                    args: if yields { vec![v.v] } else { vec![] },
                }));
            }
            self.fb.switch_to(next);
        }
        self.fb.set_term(Term::Jump(Edge {
            to: fail,
            args: vec![],
        }));
        self.fb.switch_to(fail);
        self.fb.set_term(Term::Abort(AbortCode::NonExhaustiveMatch));

        self.fb.switch_to(join);
        if !reaches_join {
            // Every arm diverged, so the match yields nothing.
            self.fb.seal();
        }
        if yields {
            Ok(LVal {
                v: params[0],
                ty: ir,
                agg,
            })
        } else {
            Ok(LVal::scalar(self.fb.unit(), IrTy::Unit))
        }
    }

    /// Does `pat` match `val`? Emits only the test, never the bindings.
    fn match_test(&mut self, pat: &Pattern, val: LVal) -> Result<ValId, String> {
        match &pat.kind {
            PatKind::Wild => Ok(self.fb.const_bool(true)),
            PatKind::Bind(_) => {
                // The checker resolved which bare names are unit variants; those
                // test the tag rather than matching unconditionally.
                match self.l.co.pat_variant.get(&pat.id).cloned() {
                    Some(vname) => {
                        let agg = val
                            .agg
                            .ok_or("internal: variant pattern against a non-aggregate")?;
                        let tag = match self.l.prog.agg(agg).case(&vname) {
                            Some(c) => c.tag,
                            None => {
                                return Err(format!("native backend: unknown variant `{vname}`"))
                            }
                        };
                        let tp = self.fb.field_ptr(agg, VARIANT_TAG_FIELD, val.v);
                        let got = self.fb.load(IrTy::I32, tp);
                        let want = self.fb.const_int(tag as i128, IrTy::I32);
                        Ok(self.fb.bin(BinKind::Eq, got, want))
                    }
                    None => Ok(self.fb.const_bool(true)),
                }
            }
            PatKind::Lit(l) => {
                let lit = match l {
                    Lit::Int { value, .. } => self.fb.const_int(*value, val.ty),
                    Lit::Bool(b) => self.fb.const_bool(*b),
                    Lit::Float { value, .. } => self.fb.push(Op::ConstFloat(*value), val.ty),
                    Lit::Unit => return Ok(self.fb.const_bool(true)),
                    Lit::Str(s) => {
                        let idx = self.l.intern_string(s);
                        let data = self.fb.push(Op::ConstStr(idx), IrTy::Ptr);
                        let len = self.fb.const_int(s.len() as i128, IrTy::U64);
                        return Ok(self.fb.push(
                            Op::CallExt {
                                name: "ax_rt_str_eq_raw".into(),
                                args: vec![val.v, data, len],
                                ret: IrTy::Bool,
                                fallible: false,
                            },
                            IrTy::Bool,
                        ));
                    }
                };
                Ok(self.fb.bin(BinKind::Eq, val.v, lit))
            }
            PatKind::Variant { name, fields } => {
                let agg = val
                    .agg
                    .ok_or("internal: variant pattern against a non-aggregate")?;
                let vname = self.l.sym(name.name);
                let case = self
                    .l
                    .prog
                    .agg(agg)
                    .case(&vname)
                    .cloned()
                    .ok_or_else(|| format!("native backend: unknown variant `{vname}`"))?;
                let tag_ptr = self.fb.field_ptr(agg, VARIANT_TAG_FIELD, val.v);
                let tag = self.fb.load(IrTy::I32, tag_ptr);
                let want = self.fb.const_int(case.tag as i128, IrTy::I32);
                let mut acc = self.fb.bin(BinKind::Eq, tag, want);
                // Nested sub-patterns refine the test. They are only evaluated
                // when the tag already matched, which the `&&` chain preserves.
                for (n, sub) in fields {
                    if self.pat_is_trivial(sub) {
                        continue;
                    }
                    let idx = self.case_field(agg, &case, &self.l.sym(n.name))?;
                    let f = self.field_index(val, agg, idx)?;
                    let sub_ok = self.match_test(sub, f)?;
                    acc = self.fb.bin(BinKind::And, acc, sub_ok);
                }
                Ok(acc)
            }
            PatKind::Record(fs) => {
                let mut acc = self.fb.const_bool(true);
                for (n, sub) in fs {
                    if self.pat_is_trivial(sub) {
                        continue;
                    }
                    let f = self.field_of(val, &self.l.sym(n.name))?;
                    let sub_ok = self.match_test(sub, f)?;
                    acc = self.fb.bin(BinKind::And, acc, sub_ok);
                }
                Ok(acc)
            }
            PatKind::Tuple(ps) => {
                let agg = val.agg.ok_or("internal: tuple pattern on a non-aggregate")?;
                let mut acc = self.fb.const_bool(true);
                for (i, sub) in ps.iter().enumerate() {
                    if self.pat_is_trivial(sub) {
                        continue;
                    }
                    let f = self.field_index(val, agg, i as u32)?;
                    let sub_ok = self.match_test(sub, f)?;
                    acc = self.fb.bin(BinKind::And, acc, sub_ok);
                }
                Ok(acc)
            }
        }
    }

    fn for_expr(&mut self, pat: &Pattern, iter: &Expr, body: &Expr) -> Result<LVal, String> {
        // Two iteration shapes: an integer range, and anything with data+len.
        //
        // Before lowering the body, note whether this loop's variable is bounded
        // by some container's length: `for i in range(0, xs.len())` makes every
        // `xs.at(i)` inside provably in range, so the check can go.
        let fact = self.index_fact(pat, iter, body);
        if let Some(f) = fact {
            self.index_facts.push(f);
        }
        let out = self.for_expr_inner(pat, iter, body);
        if fact.is_some() {
            self.index_facts.pop();
        }
        out
    }

    /// `(loop variable, container)` when this `for` bounds its index by that
    /// container's length, and the container cannot change identity inside the
    /// body.
    ///
    /// Soundness rests on two v1 properties: a `Vec` has no `pop`, `truncate`, or
    /// `clear`, so its length never decreases; and a reassignment of the binding
    /// itself would swap the container, which is why the body is scanned for one.
    /// Adding a shrinking operation later must revisit this.
    fn index_fact(&self, pat: &Pattern, iter: &Expr, body: &Expr) -> Option<(Symbol, Symbol)> {
        let PatKind::Bind(var) = &pat.kind else {
            return None;
        };
        let ExprKind::Call { callee, args } = &iter.kind else {
            return None;
        };
        if callee_name(callee, self.l.intern).as_deref() != Some("range") || args.len() != 2 {
            return None;
        }
        // Lower bound must be a literal zero; a non-zero start is still in range
        // but keeping this narrow makes the rule easy to audit.
        if !matches!(&args[0].kind, ExprKind::Lit(Lit::Int { value: 0, .. })) {
            return None;
        }
        // Upper bound must be `container.len()`.
        let ExprKind::Call {
            callee: len_callee,
            args: len_args,
        } = &args[1].kind
        else {
            return None;
        };
        if !len_args.is_empty() {
            return None;
        }
        let ExprKind::Field { base, field } = &len_callee.kind else {
            return None;
        };
        if self.l.sym(field.name) != "len" {
            return None;
        }
        let ExprKind::Path(p) = &base.kind else {
            return None;
        };
        if p.segs.len() != 1 {
            return None;
        }
        let container = p.segs[0].name;
        if assigns_to(body, container) {
            return None;
        }
        Some((var.name, container))
    }

    /// Load `xs.data` once before a loop that cannot grow or reassign `xs`.
    fn hoist_data_ptrs(&mut self, body: &Expr) {
        let mut names = Vec::new();
        for (_, cont) in &self.index_facts {
            names.push(*cont);
            for (a, b) in &self.same_len {
                if a == cont {
                    names.push(*b);
                }
                if b == cont {
                    names.push(*a);
                }
            }
        }
        names.sort_by_key(|s| s.0);
        names.dedup();
        for name in names {
            if assigns_to(body, name) || grows_vec(body, name, self.l.intern) {
                continue;
            }
            if self.data_ptrs.contains_key(&name) {
                continue;
            }
            let Some(loc) = self.lookup(name) else {
                continue;
            };
            let Some(agg) = loc.agg else {
                continue;
            };
            let Some(data_i) = self.l.prog.agg(agg).field_index("data") else {
                continue;
            };
            let dp = self.fb.field_ptr(agg, data_i, loc.addr);
            let data = self.fb.load(IrTy::Ptr, dp);
            self.data_ptrs.insert(name, data);
        }
    }

    fn hoisted_data(&self, recv: &Expr) -> Option<ValId> {
        let ExprKind::Path(p) = &recv.kind else {
            return None;
        };
        if p.segs.len() != 1 {
            return None;
        }
        self.data_ptrs.get(&p.segs[0].name).copied()
    }

    /// Is `idx` an index this function already knows is within `recv`'s length?
    fn bounded_by(&self, recv: &Expr, idx: &Expr) -> bool {
        let (ExprKind::Path(rp), ExprKind::Path(ip)) = (&recv.kind, &idx.kind) else {
            return false;
        };
        if rp.segs.len() != 1 || ip.segs.len() != 1 {
            return false;
        }
        let var = ip.segs[0].name;
        let rec = rp.segs[0].name;
        self.index_facts.iter().any(|(v, c)| {
            *v == var
                && (*c == rec
                    || self
                        .same_len
                        .iter()
                        .any(|(a, b)| (*a == *c && *b == rec) || (*b == *c && *a == rec)))
        })
    }

    fn for_expr_inner(
        &mut self,
        pat: &Pattern,
        iter: &Expr,
        body: &Expr,
    ) -> Result<LVal, String> {
        if let Some((lo, hi)) = self.range_bounds(iter)? {
            let i_slot = self.fb.alloc_slot(SlotKind::Scalar(IrTy::U64), "i");
            self.fb.store(IrTy::U64, i_slot, lo);
            let head = self.fb.new_block();
            let body_bb = self.fb.new_block();
            let exit = self.fb.new_block();
            let loop_var = match &pat.kind {
                PatKind::Bind(v) => Some(v.name),
                _ => None,
            };
            let hoisted = self.hoist_recips(&[body], loop_var);
            let saved_data = self.data_ptrs.clone();
            self.hoist_data_ptrs(body);
            self.fb.set_term(Term::Jump(Edge {
                to: head,
                args: vec![],
            }));
            self.fb.switch_to(head);
            let i = self.fb.load(IrTy::U64, i_slot);
            let go = self.fb.bin(BinKind::Lt, i, hi);
            self.fb.set_term(Term::Br {
                cond: go,
                then_e: Edge {
                    to: body_bb,
                    args: vec![],
                },
                else_e: Edge {
                    to: exit,
                    args: vec![],
                },
            });
            self.fb.switch_to(body_bb);
            // `continue` must still advance the counter, so it targets the
            // increment block rather than the loop head.
            let step = self.fb.new_block();
            self.loops.push(LoopTargets { head: step, exit });
            self.push_scope();
            let iv = self.fb.load(IrTy::U64, i_slot);
            self.bind_pattern(pat, LVal::scalar(iv, IrTy::U64), false)?;
            self.expr(body)?;
            self.pop_scope();
            self.loops.pop();
            if !self.fb.terminated() {
                self.fb.set_term(Term::Jump(Edge {
                    to: step,
                    args: vec![],
                }));
            }
            self.fb.switch_to(step);
            let cur = self.fb.load(IrTy::U64, i_slot);
            let one = self.fb.const_int(1, IrTy::U64);
            let next = self.fb.bin(BinKind::Add, cur, one);
            self.fb.store(IrTy::U64, i_slot, next);
            self.fb.set_term(Term::Jump(Edge {
                to: head,
                args: vec![],
            }));
            self.fb.switch_to(exit);
            self.data_ptrs = saved_data;
            self.drop_recips(&hoisted);
            return Ok(LVal::scalar(self.fb.unit(), IrTy::Unit));
        }

        let seq = self.expr(iter)?;
        let agg = seq
            .agg
            .ok_or("native backend: `for` over a value with no layout")?;
        let a = self.l.prog.agg(agg).clone();
        let data_idx = a
            .field_index("data")
            .ok_or("native backend: `for` over a type with no `data`")?;
        let len_idx = a.field_index("len").unwrap();
        let dp = self.fb.field_ptr(agg, data_idx, seq.v);
        let data = self.fb.load(IrTy::Ptr, dp);
        let lp = self.fb.field_ptr(agg, len_idx, seq.v);
        let len = self.fb.load(IrTy::U64, lp);
        let elem_ty = self.elem_type_of(iter)?;
        let elem_ir = self.l.ir_ty(&elem_ty)?;
        let elem_agg = if elem_ir == IrTy::Ptr {
            self.l.agg_of(&elem_ty)?
        } else {
            None
        };
        let i_slot = self.fb.alloc_slot(SlotKind::Scalar(IrTy::U64), "i");
        let z = self.fb.const_int(0, IrTy::U64);
        self.fb.store(IrTy::U64, i_slot, z);
        let head = self.fb.new_block();
        let body_bb = self.fb.new_block();
        let exit = self.fb.new_block();
        let loop_var = match &pat.kind {
            PatKind::Bind(v) => Some(v.name),
            _ => None,
        };
        let hoisted = self.hoist_recips(&[body], loop_var);
        self.fb.set_term(Term::Jump(Edge {
            to: head,
            args: vec![],
        }));
        self.fb.switch_to(head);
        let i = self.fb.load(IrTy::U64, i_slot);
        let go = self.fb.bin(BinKind::Lt, i, len);
        self.fb.set_term(Term::Br {
            cond: go,
            then_e: Edge {
                to: body_bb,
                args: vec![],
            },
            else_e: Edge {
                to: exit,
                args: vec![],
            },
        });
        self.fb.switch_to(body_bb);
        let step = self.fb.new_block();
        self.loops.push(LoopTargets {
            head: step,
            exit,
        });
        self.push_scope();
        let i = self.fb.load(IrTy::U64, i_slot);
        let ep = self.fb.push(
            Op::ElemPtr {
                elem: match elem_agg {
                    Some(a) => Repr::Agg(a),
                    None => Repr::Scalar(elem_ir),
                },
                ptr: data,
                idx: i,
            },
            IrTy::Ptr,
        );
        let item = match elem_agg {
            Some(x) => LVal {
                v: ep,
                ty: IrTy::Ptr,
                agg: Some(x),
            },
            None => {
                let v = self.fb.load(elem_ir, ep);
                LVal::scalar(v, elem_ir)
            }
        };
        self.bind_pattern(pat, item, false)?;
        self.expr(body)?;
        self.pop_scope();
        self.loops.pop();
        if !self.fb.terminated() {
            self.fb.set_term(Term::Jump(Edge {
                to: step,
                args: vec![],
            }));
        }
        self.fb.switch_to(step);
        if !self.fb.terminated() {
            let cur = self.fb.load(IrTy::U64, i_slot);
            let one = self.fb.const_int(1, IrTy::U64);
            let next = self.fb.bin(BinKind::Add, cur, one);
            self.fb.store(IrTy::U64, i_slot, next);
            self.fb.set_term(Term::Jump(Edge {
                to: head,
                args: vec![],
            }));
        }
        self.fb.switch_to(exit);
        self.drop_recips(&hoisted);
        Ok(LVal::scalar(self.fb.unit(), IrTy::Unit))
    }

    /// `range(a, b)` in iterator position becomes a counted loop.
    fn range_bounds(&mut self, iter: &Expr) -> Result<Option<(ValId, ValId)>, String> {
        if let ExprKind::Call { callee, args } = &iter.kind {
            if let Some(name) = callee_name(callee, self.l.intern) {
                if name == "range" && args.len() == 2 {
                    let lo = self.expr(&args[0])?;
                    let hi = self.expr(&args[1])?;
                    let lo = self.coerce_int(lo, IrTy::U64);
                    let hi = self.coerce_int(hi, IrTy::U64);
                    return Ok(Some((lo, hi)));
                }
            }
        }
        Ok(None)
    }

    fn elem_type_of(&mut self, iter: &Expr) -> Result<Type, String> {
        let t = self.ty_of_node(iter.id);
        let t = match &t {
            Type::Ref { inner, .. }
            | Type::Own(inner)
            | Type::Untrusted(inner)
            | Type::Secret(inner) => (**inner).clone(),
            other => other.clone(),
        };
        match &t {
            Type::Named { args, .. } if !args.is_empty() => Ok(args[0].clone()),
            _ => Err(format!(
                "native backend: cannot iterate `{}`",
                t.display(self.l.intern)
            )),
        }
    }

    fn loop_expr(&mut self, body: &Expr, e: &Expr) -> Result<LVal, String> {
        let head = self.fb.new_block();
        let exit = self.fb.new_block();
        let hoisted = self.hoist_recips(&[body], None);
        self.fb.set_term(Term::Jump(Edge {
            to: head,
            args: vec![],
        }));
        self.fb.switch_to(head);
        self.loops.push(LoopTargets { head, exit });
        self.push_scope();
        self.expr(body)?;
        self.pop_scope();
        self.loops.pop();
        if !self.fb.terminated() {
            self.fb.set_term(Term::Jump(Edge {
                to: head,
                args: vec![],
            }));
        }
        // Reached only via `break`; a `loop` without one never yields.
        self.fb.switch_to(exit);
        self.drop_recips(&hoisted);
        self.undef_of(e)
    }

    fn while_expr(&mut self, cond: &Expr, body: &Expr) -> Result<LVal, String> {
        let head = self.fb.new_block();
        let body_bb = self.fb.new_block();
        let exit = self.fb.new_block();
        // The condition is evaluated every iteration, so only a divisor that is
        // also invariant of the condition (not assigned there either) hoists.
        let hoisted = self.hoist_recips(&[cond, body], None);
        self.fb.set_term(Term::Jump(Edge {
            to: head,
            args: vec![],
        }));
        self.fb.switch_to(head);
        let c = self.expr(cond)?;
        self.fb.set_term(Term::Br {
            cond: c.v,
            then_e: Edge {
                to: body_bb,
                args: vec![],
            },
            else_e: Edge {
                to: exit,
                args: vec![],
            },
        });
        self.fb.switch_to(body_bb);
        self.loops.push(LoopTargets { head, exit });
        self.push_scope();
        self.expr(body)?;
        self.pop_scope();
        self.loops.pop();
        if !self.fb.terminated() {
            self.fb.set_term(Term::Jump(Edge {
                to: head,
                args: vec![],
            }));
        }
        self.fb.switch_to(exit);
        self.drop_recips(&hoisted);
        Ok(LVal::scalar(self.fb.unit(), IrTy::Unit))
    }

    /// Compute reciprocals of every loop-invariant unsigned 64-bit divisor in
    /// `regions` and install them so body `/` and `%` can use them.
    ///
    /// A name qualifies when all of: it is a single-segment path, its type is
    /// unsigned 64-bit, the checker proved it unconditionally non-zero, it is
    /// not assigned anywhere in `regions`, and it is not the loop variable.
    /// Nested loops keep the outer map: an already-hoisted binding is left alone.
    ///
    /// A compile-time-constant divisor is left to the C compiler: clang turns
    /// `n % 7` into a vectorised multiply-high, and inserting a runtime
    /// reciprocal blocks that and loses by more than 2×. The hoist is for a
    /// divisor that is invariant of the loop but not a constant — the case
    /// rustc and gc do not hoist.
    fn hoist_recips(&mut self, regions: &[&Expr], loop_var: Option<Symbol>) -> Vec<ValId> {
        let mut names = Vec::new();
        for r in regions {
            collect_invariant_divisors(r, &self.l.co.nonzero_div, &self.l.co.nonzero_div_needs_guard, &mut names);
        }
        names.sort_by_key(|s| s.0);
        names.dedup();
        let mut installed = Vec::new();
        for name in names {
            if loop_var == Some(name) {
                continue;
            }
            if regions.iter().any(|r| assigns_to(r, name) || takes_address_of(r, name)) {
                continue;
            }
            let Some(local) = self.lookup(name) else {
                continue;
            };
            if self.recips.contains_key(&local.addr) {
                continue;
            }
            if local.ir != IrTy::U64 || local.agg.is_some() {
                continue;
            }
            if local.const_div {
                continue;
            }
            let d = if local.by_ref {
                local.addr
            } else {
                self.fb.load(local.ir, local.addr)
            };
            // Inline header names: the C tier inlines them; Cranelift maps them
            // to the exported `ax_rt_*` symbols because the inlines have no
            // dlsym.
            let m = self.fb.push(
                Op::CallExt {
                    name: "ax_recip_m".into(),
                    args: vec![d],
                    ret: IrTy::U64,
                    fallible: false,
                },
                IrTy::U64,
            );
            let more = self.fb.push(
                Op::CallExt {
                    name: "ax_recip_more".into(),
                    args: vec![d],
                    ret: IrTy::U64,
                    fallible: false,
                },
                IrTy::U64,
            );
            self.recips.insert(local.addr, Recip { d, m, more });
            installed.push(local.addr);
        }
        installed
    }

    fn drop_recips(&mut self, addrs: &[ValId]) {
        for a in addrs {
            self.recips.remove(a);
        }
    }

    /// Use a preheader reciprocal when the divisor is the same binding that was
    /// hoisted — looked up by address, so a shadow of the same name misses.
    fn try_recip_div(&mut self, op: BinOp, l: LVal, rhs: &Expr) -> Option<ValId> {
        if l.ty != IrTy::U64 {
            return None;
        }
        let ExprKind::Path(p) = &rhs.kind else {
            return None;
        };
        if p.segs.len() != 1 {
            return None;
        }
        let local = self.lookup(p.segs[0].name)?;
        let recip = *self.recips.get(&local.addr)?;
        let name = if op == BinOp::Div {
            "ax_div_recip"
        } else {
            "ax_rem_recip"
        };
        let args = if op == BinOp::Div {
            vec![l.v, recip.m, recip.more]
        } else {
            vec![l.v, recip.d, recip.m, recip.more]
        };
        Some(self.fb.push(
            Op::CallExt {
                name: name.into(),
                args,
                ret: IrTy::U64,
                fallible: false,
            },
            IrTy::U64,
        ))
    }

    /// `expr as T`: width change, saturating float-to-int, rounding int-to-float.
    fn cast_expr(&mut self, inner: &Expr, e: &Expr) -> Result<LVal, String> {
        let v = self.expr(inner)?;
        let (to, _) = self.ir_of(e)?;
        if v.ty == to {
            return Ok(LVal::scalar(v.v, to));
        }
        let kind = match (v.ty, to) {
            (f, t) if f.is_int() && t.is_int() => {
                if t.bits() < f.bits() {
                    CastKind::Trunc
                } else if f.is_signed() {
                    CastKind::SExt
                } else {
                    CastKind::ZExt
                }
            }
            (f, t) if f.is_int() && t.is_float() => {
                if f.is_signed() {
                    CastKind::SToF
                } else {
                    CastKind::UToF
                }
            }
            (f, t) if f.is_float() && t.is_int() => {
                if t.is_signed() {
                    CastKind::FToS
                } else {
                    CastKind::FToU
                }
            }
            (f, t) if f.is_float() && t.is_float() => CastKind::FCast,
            (f, t) => {
                return Err(format!(
                    "native backend: cannot cast {} to {}",
                    f.name(),
                    t.name()
                ))
            }
        };
        let out = self.fb.push(Op::Cast { kind, v: v.v }, to);
        Ok(LVal::scalar(out, to))
    }

    fn region_expr(&mut self, name: &Ident, body: &Expr) -> Result<LVal, String> {
        // A region is a bump arena with lexical extent: everything allocated
        // inside dies at the closing brace, which is why `store` is checked.
        let idx = self.fb.func.regions.len() as RegionIdx;
        let depth = self.regions.len() as u32 + 1;
        self.fb.func.regions.push(RegionInfo {
            name: self.l.sym(name.name),
            depth,
        });
        self.fb.push_void(Op::RegionEnter(idx));
        self.regions.push(idx);
        self.push_scope();
        // Bind the region name as an allocator handle: `{kind: arena, arena: &r}`.
        // Allocation inside the region is a pointer bump, and everything it
        // allocated dies at `region.exit`.
        let handle = self.fb.push(Op::RegionAllocHandle(idx), IrTy::Ptr);
        self.bind(
            name.name,
            Local {
                addr: handle,
                ir: IrTy::Ptr,
                agg: None,
                by_ref: true,
                const_div: false,
                unique_heap: false,
            },
        );
        let v = self.expr(body)?;
        self.pop_scope();
        self.regions.pop();
        // A value escaping the region must be copied out before the arena dies.
        let out = if let Some(agg) = v.agg {
            let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
            self.fb.push_void(Op::CopyAgg {
                ty: agg,
                dst: slot,
                src: v.v,
            });
            LVal {
                v: slot,
                ty: IrTy::Ptr,
                agg: Some(agg),
            }
        } else {
            v
        };
        if !self.fb.terminated() {
            self.fb.push_void(Op::RegionExit(idx));
        }
        Ok(out)
    }

    fn catch_expr(&mut self, inner: &Expr, arms: &[Arm], e: &Expr) -> Result<LVal, String> {
        // Nothing inside can raise (an inner `catch` already discharged the
        // row), so the handler is dead code and this is just the body.
        if !self.l.co.caught.contains_key(&e.id) {
            return self.expr(inner);
        }
        let (ir, agg) = self.ir_of(e)?;
        let yields = ir != IrTy::Unit;
        let handler = self.fb.new_block();
        let (join, params) = if yields {
            self.fb.new_block_with(&[ir])
        } else {
            (self.fb.new_block(), Vec::new())
        };
        // The caught type comes from the checker: it is the `err[E]` this
        // `catch` discharges, which need not be the enclosing function's.
        let err_agg = self.caught_agg(e.id)?;
        let slot = self.fb.alloc_slot(SlotKind::Agg(err_agg), "caught");
        self.handlers.push(Handler {
            block: handler,
            slot,
            agg: err_agg,
        });
        let v = self.expr(inner)?;
        self.handlers.pop();
        if !self.fb.terminated() {
            self.fb.set_term(Term::Jump(Edge {
                to: join,
                args: if yields { vec![v.v] } else { vec![] },
            }));
        }
        // The handler reads the payload the raise wrote into the slot.
        self.fb.switch_to(handler);
        let payload = LVal {
            v: slot,
            ty: IrTy::Ptr,
            agg: Some(err_agg),
        };
        for arm in arms {
            let next = self.fb.new_block();
            let matched = self.match_test(&arm.pat, payload)?;
            let body_bb = self.fb.new_block();
            self.fb.set_term(Term::Br {
                cond: matched,
                then_e: Edge {
                    to: body_bb,
                    args: vec![],
                },
                else_e: Edge {
                    to: next,
                    args: vec![],
                },
            });
            self.fb.switch_to(body_bb);
            self.push_scope();
            self.bind_pattern(&arm.pat, payload, false)?;
            let hv = self.expr(&arm.body)?;
            self.pop_scope();
            if !self.fb.terminated() {
                self.fb.set_term(Term::Jump(Edge {
                    to: join,
                    args: if yields { vec![hv.v] } else { vec![] },
                }));
            }
            self.fb.switch_to(next);
        }
        // No arm matched: the error keeps propagating, matching the oracle. An
        // infallible enclosing function has nowhere to send it, so it aborts.
        if self.fb.func.is_fallible() {
            if let (Some(d), Some(a)) = (self.err_dest, self.fb.func.err.as_ref().and_then(|c| c.agg))
            {
                if a == err_agg {
                    self.fb.push_void(Op::CopyAgg {
                        ty: a,
                        dst: d,
                        src: slot,
                    });
                }
            }
            let tag = self.fb.const_int(1, IrTy::I32);
            self.fb.set_term(Term::RetErr(tag));
        } else {
            self.emit_uncaught_raise();
        }
        self.fb.switch_to(join);
        if yields {
            Ok(LVal {
                v: params[0],
                ty: ir,
                agg,
            })
        } else {
            Ok(LVal::scalar(self.fb.unit(), IrTy::Unit))
        }
    }

    fn attempt_expr(&mut self, inner: &Expr, e: &Expr) -> Result<LVal, String> {
        // `attempt x` : T !{err[E]} -> Result[T, E]
        let (_, agg) = self.ir_of(e)?;
        let res_agg = agg.ok_or("native backend: `attempt` with no Result layout")?;
        let slot = self.fb.alloc_slot(SlotKind::Agg(res_agg), "");
        let handler = self.fb.new_block();
        let join = self.fb.new_block();
        let err_agg = self.caught_agg(e.id)?;
        let err_slot = self.fb.alloc_slot(SlotKind::Agg(err_agg), "caught");
        self.handlers.push(Handler {
            block: handler,
            slot: err_slot,
            agg: err_agg,
        });
        let v = self.expr(inner)?;
        self.handlers.pop();
        if !self.fb.terminated() {
            let ok_case = self
                .l
                .prog
                .agg(res_agg)
                .case("Ok")
                .cloned()
                .ok_or("internal: Result without an Ok case")?;
            let tag = self.fb.const_int(ok_case.tag as i128, IrTy::I32);
            self.fb.store(IrTy::I32, slot, tag);
            if let Some(fi) = ok_case.fields.first() {
                self.store_field(slot, res_agg, *fi, v)?;
            }
            self.fb.set_term(Term::Jump(Edge {
                to: join,
                args: vec![],
            }));
        }
        self.fb.switch_to(handler);
        let err_case = self
            .l
            .prog
            .agg(res_agg)
            .case("Err")
            .cloned()
            .ok_or("internal: Result without an Err case")?;
        let tag = self.fb.const_int(err_case.tag as i128, IrTy::I32);
        self.fb.store(IrTy::I32, slot, tag);
        if let Some(fi) = err_case.fields.first() {
            let payload = LVal {
                v: err_slot,
                ty: IrTy::Ptr,
                agg: Some(err_agg),
            };
            self.store_field(slot, res_agg, *fi, payload)?;
        }
        self.fb.set_term(Term::Jump(Edge {
            to: join,
            args: vec![],
        }));
        self.fb.switch_to(join);
        Ok(LVal {
            v: slot,
            ty: IrTy::Ptr,
            agg: Some(res_agg),
        })
    }

    // ---- calls -------------------------------------------------------

    fn call(&mut self, callee: &Expr, args: &[Expr], e: &Expr) -> Result<LVal, String> {
        // A method on a dictionary parameter: resolve statically to the function
        // the dictionary names and call it directly.
        if let Some(v) = self.try_dict_dispatch(callee, args, e)? {
            return Ok(v);
        }
        if let Some(v) = self.try_method(callee, args, e)? {
            return Ok(v);
        }
        // A call through a function-typed local: an indirect call.
        if let ExprKind::Path(p) = &callee.kind {
            if p.segs.len() == 1 && self.lookup(p.segs[0].name).is_some() {
                if let Type::Fn { .. } = self.ty_of_node(callee.id) {
                    return self.call_indirect(callee, args, e);
                }
            }
        }
        let name = callee_name(callee, self.l.intern)
            .ok_or("native backend: this call form is not lowered (v1 scope)")?;

        // Variant constructors called with positional payloads, e.g. `Some(x)`.
        if let Some(v) = self.try_variant_call(&name, args, e)? {
            return Ok(v);
        }
        if let Some(v) = self.try_builtin(&name, args, e)? {
            return Ok(v);
        }
        self.call_user(&name, args, e)
    }

    /// Container methods: `len`, `at`, `get`, `push`, `set`.
    ///
    /// Element size comes from the layout, so one runtime routine serves every
    /// element type without boxing.
    fn lower_map_method(
        &mut self,
        base: &Expr,
        name: &str,
        args: &[Expr],
        e: &Expr,
    ) -> Result<Option<LVal>, String> {
        let recv = self.expr(base)?;
        match name {
            "len" => {
                let v = self.fb.push(
                    Op::CallExt {
                        name: "ax_rt_map_len".into(),
                        args: vec![recv.v],
                        ret: IrTy::U64,
                        fallible: false,
                    },
                    IrTy::U64,
                );
                Ok(Some(LVal::scalar(v, IrTy::U64)))
            }
            "insert" | "put" => {
                let k = self.expr(&args[0])?;
                let val = self.expr(&args[1])?;
                self.fb.push_void(Op::CallExt {
                    name: "ax_rt_map_insert".into(),
                    args: vec![recv.v, k.v, val.v],
                    ret: IrTy::Unit,
                    fallible: false,
                });
                Ok(Some(LVal::scalar(self.fb.unit(), IrTy::Unit)))
            }
            "add" => {
                let k = self.expr(&args[0])?;
                let delta = self.expr(&args[1])?;
                self.fb.push_void(Op::CallExt {
                    name: "ax_rt_map_add".into(),
                    args: vec![recv.v, k.v, delta.v],
                    ret: IrTy::Unit,
                    fallible: false,
                });
                Ok(Some(LVal::scalar(self.fb.unit(), IrTy::Unit)))
            }
            "get" => {
                let k = self.expr(&args[0])?;
                let tmp = self.fb.alloc_slot(SlotKind::Scalar(IrTy::I64), "mapget");
                let found = self.fb.push(
                    Op::CallExt {
                        name: "ax_rt_map_get".into(),
                        args: vec![recv.v, k.v, tmp],
                        ret: IrTy::I32,
                        fallible: false,
                    },
                    IrTy::I32,
                );
                let loaded = self.fb.load(IrTy::I64, tmp);
                let opt_ty = self.ty_of_node(e.id);
                let opt = self
                    .l
                    .agg_of(&opt_ty)?
                    .ok_or_else(|| {
                        format!(
                            "map.get: no Option layout for {}",
                            opt_ty.display(self.l.intern)
                        )
                    })?;
                let slot = self.fb.alloc_slot(SlotKind::Agg(opt), "mapopt");
                let some = self
                    .l
                    .prog
                    .agg(opt)
                    .case("Some")
                    .cloned()
                    .ok_or("Option Some")?;
                let none = self
                    .l
                    .prog
                    .agg(opt)
                    .case("None")
                    .cloned()
                    .ok_or("Option None")?;
                let join = self.fb.new_block();
                let yes = self.fb.new_block();
                let no = self.fb.new_block();
                let zero = self.fb.const_int(0, IrTy::I32);
                let pred = self.fb.bin(BinKind::Ne, found, zero);
                self.fb.set_term(Term::Br {
                    cond: pred,
                    then_e: Edge {
                        to: yes,
                        args: vec![],
                    },
                    else_e: Edge {
                        to: no,
                        args: vec![],
                    },
                });
                self.fb.switch_to(yes);
                let tag = self.fb.const_int(some.tag as i128, IrTy::I32);
                self.fb.store(IrTy::I32, slot, tag);
                if let Some(fi) = some.fields.first() {
                    self.store_field(slot, opt, *fi, LVal::scalar(loaded, IrTy::I64))?;
                }
                self.fb.set_term(Term::Jump(Edge {
                    to: join,
                    args: vec![],
                }));
                self.fb.switch_to(no);
                let tag = self.fb.const_int(none.tag as i128, IrTy::I32);
                self.fb.store(IrTy::I32, slot, tag);
                self.fb.set_term(Term::Jump(Edge {
                    to: join,
                    args: vec![],
                }));
                self.fb.switch_to(join);
                Ok(Some(LVal {
                    v: slot,
                    ty: IrTy::Ptr,
                    agg: Some(opt),
                }))
            }
            "contains" => {
                let k = self.expr(&args[0])?;
                let tmp = self.fb.alloc_slot(SlotKind::Scalar(IrTy::I64), "mapc");
                let found = self.fb.push(
                    Op::CallExt {
                        name: "ax_rt_map_get".into(),
                        args: vec![recv.v, k.v, tmp],
                        ret: IrTy::I32,
                        fallible: false,
                    },
                    IrTy::I32,
                );
                let zero = self.fb.const_int(0, IrTy::I32);
                let pred = self.fb.bin(BinKind::Ne, found, zero);
                Ok(Some(LVal::scalar(pred, IrTy::Bool)))
            }
            _ => Ok(None),
        }
    }

    fn try_method(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        e: &Expr,
    ) -> Result<Option<LVal>, String> {
        // Tree surface: `(xs.at i)` is a dotted Path, not a Field.
        if let ExprKind::Path(p) = &callee.kind {
            if p.segs.len() >= 2 && self.lookup(p.segs[0].name).is_some() {
                let fname = self.l.sym(p.segs.last().unwrap().name);
                let recv_path = Path {
                    segs: p.segs[..p.segs.len() - 1].to_vec(),
                    span: p.span,
                };
                let recv_expr = Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Path(recv_path),
                    span: p.span,
                };
                return self.try_method_on(&recv_expr, &fname, args, e);
            }
        }
        let (base, field) = match &callee.kind {
            ExprKind::Field { base, field } => (base, field),
            _ => return Ok(None),
        };
        let name = self.l.sym(field.name);
        self.try_method_on(base, &name, args, e)
    }

    fn try_method_on(
        &mut self,
        base: &Expr,
        name: &str,
        args: &[Expr],
        e: &Expr,
    ) -> Result<Option<LVal>, String> {
        let name = name.to_string();
        if matches!(name.as_str(), "len" | "get" | "insert" | "put" | "contains" | "add") {
            let recv_ty = self.ty_of_node(base.id);
            if type_is_map(&recv_ty, self.l.intern) {
                return self.lower_map_method(base, &name, args, e);
            }
        }
        if !matches!(name.as_str(), "len" | "at" | "get" | "push" | "set" | "reserve" | "eq") {
            return Ok(None);
        }
        // Module-qualified calls (`fs.read`) also parse as field access.
        if callee_name(base, self.l.intern)
            .map(|n| self.l.co.fns.iter().all(|f| self.l.sym(f.sig.name) != n))
            .unwrap_or(false)
            && matches!(base.kind, ExprKind::Path(_))
            && self.path_is_local(base).is_none()
        {
            return Ok(None);
        }
        let recv = self.expr(base)?;
        let Some(agg) = recv.agg else {
            return Ok(None);
        };
        let a = self.l.prog.agg(agg).clone();
        let is_str = a.name == "str";
        let (Some(data_i), Some(len_i)) = (a.field_index("data"), a.field_index("len")) else {
            return Ok(None);
        };
        // Element type from the receiver's checked type.
        let recv_ty = self.ty_of_node(base.id);
        let elem_ty = self.container_elem(&recv_ty)?;
        let elem_ir = self.l.ir_ty(&elem_ty)?;
        let elem_agg = if elem_ir == IrTy::Ptr {
            self.l.agg_of(&elem_ty)?
        } else {
            None
        };
        let elem_repr = match elem_agg {
            Some(x) => Repr::Agg(x),
            None => Repr::Scalar(elem_ir),
        };
        // Ask the backend for the size; a literal here could disagree with the
        // padding the C compiler chooses.
        let elem_size = elem_repr;

        match name.as_str() {
            "len" => {
                let p = self.fb.field_ptr(agg, len_i, recv.v);
                let v = self.fb.load(IrTy::U64, p);
                Ok(Some(LVal::scalar(v, IrTy::U64)))
            }
            "eq" => {
                let other = self.expr(&args[0])?;
                let oagg = other
                    .agg
                    .ok_or("native backend: `eq` needs a Vec or String argument")?;
                let oa = self.l.prog.agg(oagg).clone();
                let (Some(odata_i), Some(olen_i)) = (oa.field_index("data"), oa.field_index("len"))
                else {
                    return Err("native backend: `eq` argument has no data/len".into());
                };
                let lp = self.fb.field_ptr(agg, len_i, recv.v);
                let len = self.fb.load(IrTy::U64, lp);
                let olp = self.fb.field_ptr(oagg, olen_i, other.v);
                let olen = self.fb.load(IrTy::U64, olp);
                let same = self.fb.bin(BinKind::Eq, len, olen);
                let (join, params) = self.fb.new_block_with(&[IrTy::Bool]);
                let cmp_b = self.fb.new_block();
                let no_b = self.fb.new_block();
                self.fb.set_term(Term::Br {
                    cond: same,
                    then_e: Edge {
                        to: cmp_b,
                        args: vec![],
                    },
                    else_e: Edge {
                        to: no_b,
                        args: vec![],
                    },
                });
                self.fb.switch_to(no_b);
                let f = self.fb.const_bool(false);
                self.fb.set_term(Term::Jump(Edge {
                    to: join,
                    args: vec![f],
                }));
                self.fb.switch_to(cmp_b);
                let dp = self.fb.field_ptr(agg, data_i, recv.v);
                let data = self.fb.load(IrTy::Ptr, dp);
                let odp = self.fb.field_ptr(oagg, odata_i, other.v);
                let odata = self.fb.load(IrTy::Ptr, odp);
                let sz = self.fb.push(Op::SizeOf(elem_size), IrTy::U64);
                let nbytes = self.fb.bin(BinKind::Mul, len, sz);
                let eq = self.fb.push(
                    Op::CallExt {
                        name: "ax_rt_mem_eq".into(),
                        args: vec![data, odata, nbytes],
                        ret: IrTy::Bool,
                        fallible: false,
                    },
                    IrTy::Bool,
                );
                self.fb.set_term(Term::Jump(Edge {
                    to: join,
                    args: vec![eq],
                }));
                self.fb.switch_to(join);
                Ok(Some(LVal::scalar(params[0], IrTy::Bool)))
            }
            "at" | "get" => {
                let i = self.expr(&args[0])?;
                let idx = self.coerce_int(i, IrTy::U64);
                let proven = name == "at" && self.bounded_by(base, &args[0]);
                if name == "at" {
                    // `at` is checked always (§3.3) — unless this function has
                    // already established that the index is in range, in which
                    // case the compare is dead and emitting it blocks clang
                    // from vectorising the walk.
                    if !proven {
                        let lp = self.fb.field_ptr(agg, len_i, recv.v);
                        let len = self.fb.load(IrTy::U64, lp);
                        let in_bounds = self.fb.bin(BinKind::Lt, idx, len);
                        let ok = self.fb.new_block();
                        let bad = self.fb.new_block();
                        self.fb.set_term(Term::Br {
                            cond: in_bounds,
                            then_e: Edge { to: ok, args: vec![] },
                            else_e: Edge { to: bad, args: vec![] },
                        });
                        self.fb.switch_to(bad);
                        self.fb.set_term(Term::Abort(AbortCode::IndexOutOfBounds));
                        self.fb.switch_to(ok);
                    }
                    let data = match self.hoisted_data(base) {
                        Some(p) => p,
                        None => {
                            let dp = self.fb.field_ptr(agg, data_i, recv.v);
                            self.fb.load(IrTy::Ptr, dp)
                        }
                    };
                    let ep = self.fb.push(
                        Op::ElemPtr {
                            elem: elem_repr,
                            ptr: data,
                            idx,
                        },
                        IrTy::Ptr,
                    );
                    return Ok(Some(match elem_agg {
                        Some(x) => LVal {
                            v: ep,
                            ty: IrTy::Ptr,
                            agg: Some(x),
                        },
                        None => {
                            let ir = if is_str { IrTy::U8 } else { elem_ir };
                            let v = self.fb.load(ir, ep);
                            LVal::scalar(v, ir)
                        }
                    }));
                }
                // `get` yields Option[T] instead of aborting.
                let lp = self.fb.field_ptr(agg, len_i, recv.v);
                let len = self.fb.load(IrTy::U64, lp);
                let in_bounds = self.fb.bin(BinKind::Lt, idx, len);
                let (_, opt_agg) = self.ir_of(e)?;
                let opt = opt_agg.ok_or("native backend: `get` with no Option layout")?;
                let slot = self.fb.alloc_slot(SlotKind::Agg(opt), "");
                let some = self.l.prog.agg(opt).case("Some").cloned();
                let none = self.l.prog.agg(opt).case("None").cloned();
                let some_b = self.fb.new_block();
                let none_b = self.fb.new_block();
                let join = self.fb.new_block();
                self.fb.set_term(Term::Br {
                    cond: in_bounds,
                    then_e: Edge { to: some_b, args: vec![] },
                    else_e: Edge { to: none_b, args: vec![] },
                });
                self.fb.switch_to(some_b);
                if let Some(c) = some {
                    let tag = self.fb.const_int(c.tag as i128, IrTy::I32);
                    self.fb.store(IrTy::I32, slot, tag);
                    if let Some(fi) = c.fields.first() {
                        let dp = self.fb.field_ptr(agg, data_i, recv.v);
                        let data = self.fb.load(IrTy::Ptr, dp);
                        let ep = self.fb.push(
                            Op::ElemPtr {
                                elem: elem_repr,
                                ptr: data,
                                idx,
                            },
                            IrTy::Ptr,
                        );
                        let val = match elem_agg {
                            Some(x) => LVal {
                                v: ep,
                                ty: IrTy::Ptr,
                                agg: Some(x),
                            },
                            None => {
                                let v = self.fb.load(elem_ir, ep);
                                LVal::scalar(v, elem_ir)
                            }
                        };
                        self.store_field(slot, opt, *fi, val)?;
                    }
                }
                self.fb.set_term(Term::Jump(Edge { to: join, args: vec![] }));
                self.fb.switch_to(none_b);
                if let Some(c) = none {
                    let tag = self.fb.const_int(c.tag as i128, IrTy::I32);
                    self.fb.store(IrTy::I32, slot, tag);
                }
                self.fb.set_term(Term::Jump(Edge { to: join, args: vec![] }));
                self.fb.switch_to(join);
                Ok(Some(LVal {
                    v: slot,
                    ty: IrTy::Ptr,
                    agg: Some(opt),
                }))
            }
            "reserve" => {
                let n = self.expr(&args[0])?;
                let want = self.coerce_int(n, IrTy::U64);
                let sz = self.fb.push(Op::SizeOf(elem_size), IrTy::U64);
                self.fb.push_void(Op::CallExt {
                    name: "ax_rt_vec_reserve".into(),
                    args: vec![recv.v, sz, want],
                    ret: IrTy::Unit,
                    fallible: false,
                });
                Ok(Some(LVal::scalar(self.fb.unit(), IrTy::Unit)))
            }
            "push" => {
                // Lowered inline: a capacity test, a typed store, and a length
                // bump. Only growth calls the runtime. Going through a function
                // that `memcpy`s the element made every push an out-of-line call
                // for no reason — the layout is known here.
                let val = self.expr(&args[0])?;
                let cap_i = a
                    .field_index("cap")
                    .ok_or("native backend: `push` needs a Vec, which has `cap`")?;
                let lp = self.fb.field_ptr(agg, len_i, recv.v);
                let len = self.fb.load(IrTy::U64, lp);
                let cp = self.fb.field_ptr(agg, cap_i, recv.v);
                let cap = self.fb.load(IrTy::U64, cp);
                let full = self.fb.bin(BinKind::Eq, len, cap);
                let grow_b = self.fb.new_block();
                let store_b = self.fb.new_block();
                self.fb.set_term(Term::Br {
                    cond: full,
                    then_e: Edge { to: grow_b, args: vec![] },
                    else_e: Edge { to: store_b, args: vec![] },
                });
                self.fb.switch_to(grow_b);
                let sz = self.fb.push(Op::SizeOf(elem_size), IrTy::U64);
                self.fb.push_void(Op::CallExt {
                    name: "ax_rt_vec_grow".into(),
                    args: vec![recv.v, sz],
                    ret: IrTy::Unit,
                    fallible: false,
                });
                self.fb.set_term(Term::Jump(Edge {
                    to: store_b,
                    args: vec![],
                }));
                self.fb.switch_to(store_b);
                // Reload data and len: growth may have moved the buffer.
                let dp = self.fb.field_ptr(agg, data_i, recv.v);
                let data = self.fb.load(IrTy::Ptr, dp);
                let lp2 = self.fb.field_ptr(agg, len_i, recv.v);
                let len2 = self.fb.load(IrTy::U64, lp2);
                let ep = self.fb.push(
                    Op::ElemPtr {
                        elem: elem_size,
                        ptr: data,
                        idx: len2,
                    },
                    IrTy::Ptr,
                );
                match elem_agg {
                    Some(x) => self.fb.push_void(Op::CopyAgg {
                        ty: x,
                        dst: ep,
                        src: val.v,
                    }),
                    None => self.fb.store(elem_ir, ep, val.v),
                }
                let one = self.fb.const_int(1, IrTy::U64);
                let next = self.fb.bin(BinKind::Add, len2, one);
                let lp3 = self.fb.field_ptr(agg, len_i, recv.v);
                self.fb.store(IrTy::U64, lp3, next);
                Ok(Some(LVal::scalar(self.fb.unit(), IrTy::Unit)))
            }
            "set" => {
                let i = self.expr(&args[0])?;
                let idx = self.coerce_int(i, IrTy::U64);
                let val = self.expr(&args[1])?;
                if !self.bounded_by(base, &args[0]) {
                    let lp = self.fb.field_ptr(agg, len_i, recv.v);
                    let len = self.fb.load(IrTy::U64, lp);
                    let in_bounds = self.fb.bin(BinKind::Lt, idx, len);
                    let ok = self.fb.new_block();
                    let bad = self.fb.new_block();
                    self.fb.set_term(Term::Br {
                        cond: in_bounds,
                        then_e: Edge { to: ok, args: vec![] },
                        else_e: Edge { to: bad, args: vec![] },
                    });
                    self.fb.switch_to(bad);
                    self.fb.set_term(Term::Abort(AbortCode::IndexOutOfBounds));
                    self.fb.switch_to(ok);
                }
                let data = match self.hoisted_data(base) {
                    Some(p) => p,
                    None => {
                        let dp = self.fb.field_ptr(agg, data_i, recv.v);
                        self.fb.load(IrTy::Ptr, dp)
                    }
                };
                let ep = self.fb.push(
                    Op::ElemPtr {
                        elem: elem_repr,
                        ptr: data,
                        idx,
                    },
                    IrTy::Ptr,
                );
                match elem_agg {
                    Some(x) => self.fb.push_void(Op::CopyAgg {
                        ty: x,
                        dst: ep,
                        src: val.v,
                    }),
                    None => self.fb.store(elem_ir, ep, val.v),
                }
                Ok(Some(LVal::scalar(self.fb.unit(), IrTy::Unit)))
            }
            _ => Ok(None),
        }
    }

    /// Is this path a local binding? Distinguishes `xs.len()` from `fs.read()`.
    fn path_is_local(&self, e: &Expr) -> Option<Local> {
        match &e.kind {
            ExprKind::Path(p) if p.segs.len() == 1 => self.lookup(p.segs[0].name),
            _ => None,
        }
    }

    /// Element type of a container type, peeling references.
    fn container_elem(&mut self, t: &Type) -> Result<Type, String> {
        let bare = match t {
            Type::Ref { inner, .. }
            | Type::Own(inner)
            | Type::Untrusted(inner)
            | Type::Secret(inner) => (**inner).clone(),
            other => other.clone(),
        };
        match &bare {
            Type::Named { def, args } => {
                let n = self.l.sym(*def);
                if n == "String" || n == "str" {
                    return Ok(Type::Prim(Prim::U8));
                }
                args.first().cloned().ok_or_else(|| {
                    format!(
                        "native backend: `{}` has no element type",
                        bare.display(self.l.intern)
                    )
                })
            }
            other => Err(format!(
                "native backend: `{}` is not a container",
                other.display(self.l.intern)
            )),
        }
    }

    /// `o.cmp(a, b)` where `o` is a dictionary witness. Returns the lowered
    /// direct call, or `None` if this is not dictionary dispatch.
    /// Call a function value. The signature comes from the callee's checked
    /// type, so the backend can reconstruct the C function-pointer type.
    fn call_indirect(&mut self, callee: &Expr, args: &[Expr], e: &Expr) -> Result<LVal, String> {
        let f = self.expr(callee)?;
        let (ir, agg) = self.ir_of(e)?;
        let mut argv = Vec::with_capacity(args.len() + 1);
        for a in args {
            argv.push(self.expr(a)?.v);
        }
        let ret_slot = match agg {
            Some(a) => {
                let s = self.fb.alloc_slot(SlotKind::Agg(a), "");
                argv.push(s);
                Some(s)
            }
            None => None,
        };
        let v = self.fb.push(
            Op::CallIndirect {
                ptr: f.v,
                args: argv,
                ret: if agg.is_some() { IrTy::Unit } else { ir },
            },
            if agg.is_some() { IrTy::Unit } else { ir },
        );
        Ok(match (ret_slot, agg) {
            (Some(s), Some(a)) => LVal {
                v: s,
                ty: IrTy::Ptr,
                agg: Some(a),
            },
            _ => LVal::scalar(v, ir),
        })
    }

    fn try_dict_dispatch(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        e: &Expr,
    ) -> Result<Option<LVal>, String> {
        let (base, field) = match &callee.kind {
            ExprKind::Field { base, field } => (base, field),
            _ => return Ok(None),
        };
        let root = match &base.kind {
            ExprKind::Path(p) if p.segs.len() == 1 => p.segs[0].name,
            _ => return Ok(None),
        };
        let Some(dict_idx) = self.dicts.get(&root).copied() else {
            return Ok(None);
        };
        let fname = self.l.sym(field.name);
        let decl = self
            .l
            .co
            .dict_decls
            .get(dict_idx)
            .ok_or("internal: dictionary index out of range")?
            .clone();
        let target = decl
            .fields
            .iter()
            .find(|(n, _)| self.l.sym(n.name) == fname)
            .map(|(_, ex)| ex.clone())
            .ok_or_else(|| format!("native backend: dictionary has no field `{fname}`"))?;
        let target_name = callee_name(&target, self.l.intern).ok_or_else(|| {
            format!("native backend: dictionary field `{fname}` is not a named function")
        })?;
        if let Some(v) = self.try_builtin(&target_name, args, e)? {
            return Ok(Some(v));
        }
        Ok(Some(self.call_user(&target_name, args, e)?))
    }

    fn try_variant_call(
        &mut self,
        name: &str,
        args: &[Expr],
        e: &Expr,
    ) -> Result<Option<LVal>, String> {
        let (_, agg) = self.ir_of_node(e.id)?;
        let Some(a) = agg else { return Ok(None) };
        let Some(case) = self.l.prog.agg(a).case(name).cloned() else {
            return Ok(None);
        };
        let slot = self.fb.alloc_slot(SlotKind::Agg(a), "");
        let tag = self.fb.const_int(case.tag as i128, IrTy::I32);
        self.fb.store(IrTy::I32, slot, tag);
        for (i, arg) in args.iter().enumerate() {
            let v = self.expr(arg)?;
            if let Some(fi) = case.fields.get(i) {
                self.store_field(slot, a, *fi, v)?;
            }
        }
        Ok(Some(LVal {
            v: slot,
            ty: IrTy::Ptr,
            agg: Some(a),
        }))
    }

    fn call_user(&mut self, name: &str, args: &[Expr], e: &Expr) -> Result<LVal, String> {
        // A pure function applied to constants has a constant result, and the
        // effect row is what proves the purity. Evaluating it now, with the
        // oracle, cannot change behaviour: the oracle *is* the semantics.
        if let Some(v) = self.try_fold_pure_call(name, args, e)? {
            return Ok(v);
        }
        let bare = name.rsplit('.').next().unwrap_or(name);
        let idx = self
            .l
            .co
            .fns
            .iter()
            .position(|f| self.l.sym(f.sig.name) == bare)
            .ok_or_else(|| format!("native backend: unknown function `{name}`"))?;
        let sig = self.l.co.fns[idx].sig.clone();

        // Dictionary parameters the caller left to `= default`. The checker
        // recorded which dictionary each one resolved to; it travels as part of
        // the instantiation key rather than as a runtime argument.
        let mut dict_args: Vec<(u32, usize)> = Vec::new();
        for (i, (_, _, is_dict)) in sig.params.iter().enumerate() {
            if !*is_dict {
                continue;
            }
            if let Some(d) = self.l.co.dict_defaults.get(&(e.id, i as u32)).copied() {
                dict_args.push((i as u32, d));
            }
        }

        // Lower arguments first: their types drive monomorphisation.
        let mut lowered = Vec::with_capacity(args.len());
        for a in args {
            lowered.push((self.expr(a)?, self.ty_of_node(a.id)));
        }
        let mut map = HashMap::new();
        if !sig.generics.is_empty() {
            for ((_, pty, _), (_, aty)) in sig.params.iter().zip(&lowered) {
                crate::types::unify_param(pty, aty, &mut map);
            }
            for g in &sig.generics {
                if !map.contains_key(g) {
                    return Err(format!(
                        "native backend: cannot infer `{}` at a call to `{name}`",
                        self.l.sym(*g)
                    ));
                }
            }
        }
        let targs: Vec<Type> = sig.generics.iter().map(|g| map[g].clone()).collect();
        let mut mangled = self.l.mangle(bare, &targs);
        for (pi, d) in &dict_args {
            // Two call sites resolving different dictionaries are two functions.
            mangled.push_str(&format!("_d{pi}x{d}"));
        }
        let fid = self.l.enqueue(idx, mangled, map.clone(), dict_args.clone());

        let ret_ty = self.l.resolve(&sig.ret, &map);
        let ret_ir = self.l.ir_ty(&ret_ty)?;
        let ret_agg = if ret_ir == IrTy::Ptr {
            self.l.agg_of(&ret_ty)?
        } else {
            None
        };
        let mut argv: Vec<ValId> = lowered
            .iter()
            .enumerate()
            .filter(|(i, _)| !dict_args.iter().any(|(pi, _)| *pi == *i as u32))
            .map(|(_, (v, _))| v.v)
            .collect();
        let ret_slot = ret_agg.map(|a| self.fb.alloc_slot(SlotKind::Agg(a), ""));
        if let Some(s) = ret_slot {
            argv.push(s);
        }

        // Does the callee raise? If so, thread the error slot and branch.
        let callee_err = self.l.err_channel(&self.l.co.fns[idx].inferred.clone(), &map)?;
        if let Some(ch) = callee_err {
            let payload_agg = ch.agg.ok_or("internal: error channel without a layout")?;
            // Where does the callee's payload land? If its error type is the same
            // as the destination's, straight into the destination. If it differs,
            // into a temporary that the raised path then injects — forwarding the
            // bytes directly would reinterpret one variant's tag as another's.
            let dest_agg = match self.handlers.last().copied() {
                Some(h) => Some(h.agg),
                None => self.fb.func.err.as_ref().and_then(|c| c.agg),
            };
            // With no destination at all (an infallible frame with no handler,
            // e.g. a `test` calling a fallible function directly), there is
            // nothing to inject into: the raise is simply uncaught.
            let needs_injection = dest_agg.is_some() && dest_agg != Some(payload_agg);
            let err_slot = if needs_injection {
                self.fb.alloc_slot(SlotKind::Agg(payload_agg), "raised")
            } else {
                match self.handlers.last().copied() {
                    Some(h) => h.slot,
                    None => match self.err_dest {
                        Some(d) => d,
                        None => self.fb.alloc_slot(SlotKind::Agg(payload_agg), ""),
                    },
                }
            };
            argv.push(err_slot);
            let (val, tag) = self.fb.push2(
                Op::Call {
                    f: fid,
                    args: argv,
                },
                if ret_agg.is_some() { IrTy::Unit } else { ret_ir },
                IrTy::I32,
            );
            let raised = self.fb.new_block();
            let cont = self.fb.new_block();
            let zero = self.fb.const_int(0, IrTy::I32);
            let ok = self.fb.bin(BinKind::Eq, tag, zero);
            self.fb.set_term(Term::Br {
                cond: ok,
                then_e: Edge {
                    to: cont,
                    args: vec![],
                },
                else_e: Edge {
                    to: raised,
                    args: vec![],
                },
            });
            self.fb.switch_to(raised);
            if needs_injection {
                // Convert the callee's error into ours through the declared
                // single-step injection, then re-raise.
                let inner = LVal {
                    v: err_slot,
                    ty: IrTy::Ptr,
                    agg: Some(payload_agg),
                };
                let wrapped = self.inject_payload(inner, &ch.display)?;
                self.emit_raise(wrapped)?;
            } else {
                match self.handlers.last().copied() {
                    Some(h) => {
                        self.fb.set_term(Term::Jump(Edge {
                            to: h.block,
                            args: vec![],
                        }));
                    }
                    None => {
                        if self.fb.func.is_fallible() {
                            let t = self.fb.const_int(1, IrTy::I32);
                            self.fb.set_term(Term::RetErr(t));
                        } else {
                            self.emit_uncaught_raise();
                        }
                    }
                }
            }
            self.fb.switch_to(cont);
            return Ok(match (ret_slot, ret_agg) {
                (Some(s), Some(a)) => LVal {
                    v: s,
                    ty: IrTy::Ptr,
                    agg: Some(a),
                },
                _ => LVal::scalar(val, ret_ir),
            });
        }

        let v = self.fb.push(
            Op::Call {
                f: fid,
                args: argv,
            },
            if ret_agg.is_some() { IrTy::Unit } else { ret_ir },
        );
        Ok(match (ret_slot, ret_agg) {
            (Some(s), Some(a)) => LVal {
                v: s,
                ty: IrTy::Ptr,
                agg: Some(a),
            },
            _ => LVal::scalar(v, ret_ir),
        })
    }
}

/// Where `break` and `continue` go for one enclosing loop.
#[derive(Clone, Copy)]
struct LoopTargets {
    head: BlockId,
    exit: BlockId,
}

/// Reciprocal of a loop-invariant unsigned 64-bit divisor.
///
/// Computed once in the preheader (`ax_recip_m` / `ax_recip_more`) and consumed
/// by every `/` or `%` against that name in the body. The values are SSA, so
/// they stay live across the loop without a slot.
#[derive(Clone, Copy)]
struct Recip {
    d: ValId,
    m: ValId,
    more: ValId,
}

enum Place {
    Scalar { addr: ValId, ty: IrTy },
    Agg { addr: ValId, agg: TypeId },
}

/// Prelude calls. Anything not handled here falls through to `call_user`,
/// and an unknown name is a hard error rather than silently wrong code.
impl<'l, 'a> FnLower<'l, 'a> {
    fn try_builtin(
        &mut self,
        name: &str,
        args: &[Expr],
        e: &Expr,
    ) -> Result<Option<LVal>, String> {
        // Never shadow a user function of the same name.
        let bare = name.rsplit('.').next().unwrap_or(name);
        if self
            .l
            .co
            .fns
            .iter()
            .any(|f| self.l.sym(f.sig.name) == bare)
        {
            return Ok(None);
        }
        let unit = |s: &mut Self| LVal::scalar(s.fb.unit(), IrTy::Unit);
        match name {
            "assert" => {
                let c = self.expr(&args[0])?;
                let ok = self.fb.new_block();
                let bad = self.fb.new_block();
                self.fb.set_term(Term::Br {
                    cond: c.v,
                    then_e: Edge { to: ok, args: vec![] },
                    else_e: Edge { to: bad, args: vec![] },
                });
                self.fb.switch_to(bad);
                self.fb.set_term(Term::Abort(AbortCode::Assert));
                self.fb.switch_to(ok);
                Ok(Some(unit(self)))
            }
            "fail" => {
                if let Some(a) = args.first() {
                    let msg = self.expr(a)?;
                    self.fb.push_void(Op::CallExt {
                        name: "ax_rt_print".into(),
                        args: vec![msg.v],
                        ret: IrTy::Unit,
                        fallible: false,
                    });
                }
                self.fb.set_term(Term::Abort(AbortCode::Explicit));
                let v = self.undef_of(e)?;
                Ok(Some(v))
            }
            "print" => {
                let s = self.expr(&args[0])?;
                let op = if s.agg.is_some() {
                    Op::CallExt {
                        name: "ax_rt_print".into(),
                        args: vec![s.v],
                        ret: IrTy::Unit,
                        fallible: false,
                    }
                } else {
                    // Printing a scalar: the runtime formats by width.
                    Op::CallExt {
                        name: print_fn(s.ty).into(),
                        args: vec![s.v],
                        ret: IrTy::Unit,
                        fallible: false,
                    }
                };
                self.fb.push_void(op);
                Ok(Some(unit(self)))
            }
            "int.div" | "int.rem" | "int.div_trunc" => {
                let l = self.expr(&args[0])?;
                let r = self.expr(&args[1])?;
                let op = if name == "int.rem" {
                    BinOp::Rem
                } else {
                    BinOp::Div
                };
                Ok(Some(self.checked_div(op, l, r, e)?))
            }
            "int.div_exact" => {
                // `pre b != 0` — a violated precondition aborts, never raises.
                let l = self.expr(&args[0])?;
                let r = self.expr(&args[1])?;
                let zero = self.fb.const_int(0, r.ty);
                let is_zero = self.fb.bin(BinKind::Eq, r.v, zero);
                let ok = self.fb.new_block();
                let bad = self.fb.new_block();
                self.fb.set_term(Term::Br {
                    cond: is_zero,
                    then_e: Edge { to: bad, args: vec![] },
                    else_e: Edge { to: ok, args: vec![] },
                });
                self.fb.switch_to(bad);
                self.fb.set_term(Term::Abort(AbortCode::DivExactZero));
                self.fb.switch_to(ok);
                let v = self.fb.bin(BinKind::DivTruncNZ, l.v, r.v);
                Ok(Some(LVal::scalar(v, l.ty)))
            }
            "checked_add" | "checked_sub" | "checked_mul" => {
                let v = self.checked_arith(name, args, e)?;
                Ok(Some(v))
            }
            "len" => {
                let s = self.expr(&args[0])?;
                let agg = s.agg.ok_or("native backend: `len` of a value with no layout")?;
                let idx = self
                    .l
                    .prog
                    .agg(agg)
                    .field_index("len")
                    .ok_or("native backend: `len` of a type with no length")?;
                let p = self.fb.field_ptr(agg, idx, s.v);
                let v = self.fb.load(IrTy::U64, p);
                Ok(Some(LVal::scalar(v, IrTy::U64)))
            }
            "range" => {
                // Outside iterator position a range is just its record.
                let (_, agg) = self.ir_of_node(e.id)?;
                let agg = agg.ok_or("native backend: `range` with no layout")?;
                let lo = self.expr(&args[0])?;
                let hi = self.expr(&args[1])?;
                let lo = self.coerce_int(lo, IrTy::U64);
                let hi = self.coerce_int(hi, IrTy::U64);
                let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
                let sp = self.fb.field_ptr(agg, 0, slot);
                self.fb.store(IrTy::U64, sp, lo);
                let ep = self.fb.field_ptr(agg, 1, slot);
                self.fb.store(IrTy::U64, ep, hi);
                Ok(Some(LVal {
                    v: slot,
                    ty: IrTy::Ptr,
                    agg: Some(agg),
                }))
            }
            "math.sqrt" | "math.abs" | "math.hypot" | "f32.abs" => {
                let mut argv = Vec::new();
                let mut ty = IrTy::F64;
                for a in args {
                    let v = self.expr(a)?;
                    ty = v.ty;
                    argv.push(v.v);
                }
                let rt = match (name, ty) {
                    ("math.sqrt", IrTy::F32) => "ax_rt_sqrtf",
                    ("math.sqrt", _) => "ax_rt_sqrt",
                    ("math.hypot", IrTy::F32) => "ax_rt_hypotf",
                    ("math.hypot", _) => "ax_rt_hypot",
                    (_, IrTy::F32) => "ax_rt_fabsf",
                    _ => "ax_rt_fabs",
                };
                let v = self.fb.push(
                    Op::CallExt {
                        name: rt.into(),
                        args: argv,
                        ret: ty,
                        fallible: false,
                    },
                    ty,
                );
                Ok(Some(LVal::scalar(v, ty)))
            }
            "f32.cmp" | "i32.cmp" => {
                let a = self.scalar_arg(&args[0])?;
                let b = self.scalar_arg(&args[1])?;
                let (_, agg) = self.ir_of_node(e.id)?;
                let agg = agg.ok_or("native backend: cmp with no Ordering layout")?;
                let lt = self.fb.bin(BinKind::Lt, a.v, b.v);
                let gt = self.fb.bin(BinKind::Gt, a.v, b.v);
                let tag_lt = self.l.prog.agg(agg).case("Lt").map(|c| c.tag).unwrap_or(0);
                let tag_eq = self.l.prog.agg(agg).case("Eq").map(|c| c.tag).unwrap_or(1);
                let tag_gt = self.l.prog.agg(agg).case("Gt").map(|c| c.tag).unwrap_or(2);
                let vlt = self.fb.const_int(tag_lt as i128, IrTy::I32);
                let veq = self.fb.const_int(tag_eq as i128, IrTy::I32);
                let vgt = self.fb.const_int(tag_gt as i128, IrTy::I32);
                let gt_or_eq = self.fb.push(
                    Op::Select {
                        c: gt,
                        a: vgt,
                        b: veq,
                    },
                    IrTy::I32,
                );
                let tag = self.fb.push(
                    Op::Select {
                        c: lt,
                        a: vlt,
                        b: gt_or_eq,
                    },
                    IrTy::I32,
                );
                let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
                self.fb.store(IrTy::I32, slot, tag);
                Ok(Some(LVal {
                    v: slot,
                    ty: IrTy::Ptr,
                    agg: Some(agg),
                }))
            }
            "io.bytesum_file" | "io.read_file" | "io.write_file" | "http.get_bytesum"
            | "http.get" | "http.serve" | "argv" => {
                let mut argv = Vec::new();
                for a in args {
                    argv.push(self.expr(a)?.v);
                }
                let (ir, agg) = self.ir_of_node(e.id)?;
                let rt = match name {
                    "io.bytesum_file" => "ax_rt_io_bytesum_file",
                    "io.read_file" => "ax_rt_io_read_file",
                    "io.write_file" => "ax_rt_io_write_file",
                    "http.get_bytesum" => "ax_rt_http_get_bytesum",
                    "http.get" => "ax_rt_http_get",
                    "http.serve" => "ax_rt_http_serve",
                    _ => "ax_rt_argv",
                };
                if let Some(a) = agg {
                    // Runtime writes the aggregate result through a slot.
                    let slot = self.fb.alloc_slot(SlotKind::Agg(a), "");
                    argv.push(slot);
                    self.fb.push_void(Op::CallExt {
                        name: rt.into(),
                        args: argv,
                        ret: IrTy::Unit,
                        fallible: false,
                    });
                    Ok(Some(LVal {
                        v: slot,
                        ty: IrTy::Ptr,
                        agg: Some(a),
                    }))
                } else {
                    let v = self.fb.push(
                        Op::CallExt {
                            name: rt.into(),
                            args: argv,
                            ret: ir,
                            fallible: false,
                        },
                        ir,
                    );
                    Ok(Some(LVal::scalar(v, ir)))
                }
            }
            "test.alloc" => {
                // The default allocator handle: the heap.
                let v = self.fb.push(
                    Op::CallExt {
                        name: "ax_rt_alloc_default".into(),
                        args: Vec::new(),
                        ret: IrTy::Ptr,
                        fallible: false,
                    },
                    IrTy::Ptr,
                );
                Ok(Some(LVal::scalar(v, IrTy::Ptr)))
            }
            "test.read_cap" => Ok(Some(self.lower_read_cap(args)?)),
            "fs.read" => Ok(Some(self.lower_fs_read(args, e)?)),
            "json.decode_recs" | "json.decode" => {
                Ok(Some(self.lower_json_decode(args, e)?))
            }
            "parse_i32" => {
                let s = self.expr(&args[0])?;
                let out = self.fb.alloc_slot(SlotKind::Scalar(IrTy::I32), "");
                let ok = self.fb.push(
                    Op::CallExt {
                        name: "ax_rt_parse_i32".into(),
                        args: vec![s.v, out],
                        ret: IrTy::Bool,
                        fallible: false,
                    },
                    IrTy::Bool,
                );
                let good = self.fb.new_block();
                let bad = self.fb.new_block();
                self.fb.set_term(Term::Br {
                    cond: ok,
                    then_e: Edge { to: good, args: vec![] },
                    else_e: Edge { to: bad, args: vec![] },
                });
                self.fb.switch_to(bad);
                let payload = self.error_payload("Invalid")?;
                self.emit_raise(payload)?;
                self.fb.switch_to(good);
                let v = self.fb.load(IrTy::I32, out);
                Ok(Some(LVal::scalar(v, IrTy::I32)))
            }
            "map.new" => {
                let v = self.fb.push(
                    Op::CallExt {
                        name: "ax_rt_map_new".into(),
                        args: vec![],
                        ret: IrTy::Ptr,
                        fallible: false,
                    },
                    IrTy::Ptr,
                );
                Ok(Some(LVal {
                    v,
                    ty: IrTy::Ptr,
                    agg: None,
                }))
            }
            "vec.new" => {
                let (_, agg) = self.ir_of(e)?;
                let agg = agg.ok_or("native backend: `vec.new` with no Vec layout")?;
                let alloc = self.expr(&args[0])?;
                let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
                // Element size is only needed by `push`, which reads it from the
                // call site; `new` just records the allocator.
                let zero = self.fb.const_int(0, IrTy::U64);
                self.fb.push_void(Op::CallExt {
                    name: "ax_rt_vec_new".into(),
                    args: vec![alloc.v, zero, slot],
                    ret: IrTy::Unit,
                    fallible: false,
                });
                Ok(Some(LVal {
                    v: slot,
                    ty: IrTy::Ptr,
                    agg: Some(agg),
                }))
            }
            "str.from_byte" => {
                let (_, agg) = self.ir_of(e)?;
                let agg = agg.ok_or("native backend: `str.from_byte` with no str layout")?;
                let alloc = self.expr(&args[0])?;
                let b = self.expr(&args[1])?;
                let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
                self.fb.push_void(Op::CallExt {
                    name: "ax_rt_str_from_byte".into(),
                    args: vec![alloc.v, b.v, slot],
                    ret: IrTy::Unit,
                    fallible: false,
                });
                Ok(Some(LVal {
                    v: slot,
                    ty: IrTy::Ptr,
                    agg: Some(agg),
                }))
            }
            "str.concat" => {
                let (_, agg) = self.ir_of(e)?;
                let agg = agg.ok_or("native backend: `str.concat` with no str layout")?;
                let alloc = self.expr(&args[0])?;
                let x = self.expr(&args[1])?;
                let y = self.expr(&args[2])?;
                let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
                self.fb.push_void(Op::CallExt {
                    name: "ax_rt_str_concat".into(),
                    args: vec![alloc.v, x.v, y.v, slot],
                    ret: IrTy::Unit,
                    fallible: false,
                });
                Ok(Some(LVal {
                    v: slot,
                    ty: IrTy::Ptr,
                    agg: Some(agg),
                }))
            }
            "freeze" => {
                // Identity at runtime: `freeze` only changes what the checker
                // permits, not the representation.
                let v = self.expr(&args[0])?;
                Ok(Some(v))
            }
            "sort" => {
                let v = self.lower_sort(args, e)?;
                Ok(Some(v))
            }
            "all" | "any" | "count" | "sorted_by" => Err(format!(
                "native backend: `{name}` is not lowered yet (stdlib task); \
                 use the interpreter for now"
            )),
            _ => Ok(None),
        }
    }

    /// Lower an argument that a prelude function declares as `&T` for scalar
    /// `T`. Comparators are written `cmp(&a, &b)`, so the lowered argument is an
    /// address; the builtin needs the value behind it, not the pointer.
    fn scalar_arg(&mut self, e: &Expr) -> Result<LVal, String> {
        let v = self.expr(e)?;
        if v.ty != IrTy::Ptr || v.agg.is_some() {
            return Ok(v);
        }
        let t = self.ty_of_node(e.id);
        let inner = match &t {
            Type::Ref { inner, .. }
            | Type::Own(inner)
            | Type::Untrusted(inner)
            | Type::Secret(inner) => (**inner).clone(),
            _ => return Ok(v),
        };
        match self.l.ir_ty(&inner)? {
            IrTy::Ptr => Ok(v),
            ir => {
                let loaded = self.fb.load(ir, v.v);
                Ok(LVal::scalar(loaded, ir))
            }
        }
    }

    /// `checked_add` and friends: `Option[T]`, `None` on overflow.
    fn checked_arith(&mut self, name: &str, args: &[Expr], e: &Expr) -> Result<LVal, String> {
        let a = self.expr(&args[0])?;
        let b = self.expr(&args[1])?;
        let (_, agg) = self.ir_of_node(e.id)?;
        let agg = agg.ok_or("native backend: checked arithmetic with no Option layout")?;
        let rt = match (name, a.ty) {
            ("checked_add", t) => format!("ax_rt_checked_add_{}", t.name()),
            ("checked_sub", t) => format!("ax_rt_checked_sub_{}", t.name()),
            _ => format!("ax_rt_checked_mul_{}", a.ty.name()),
        };
        let out = self.fb.alloc_slot(SlotKind::Scalar(a.ty), "");
        let ok = self.fb.push(
            Op::CallExt {
                name: rt,
                args: vec![a.v, b.v, out],
                ret: IrTy::Bool,
                fallible: false,
            },
            IrTy::Bool,
        );
        let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
        let some = self.l.prog.agg(agg).case("Some").cloned();
        let none = self.l.prog.agg(agg).case("None").cloned();
        let some_b = self.fb.new_block();
        let none_b = self.fb.new_block();
        let join = self.fb.new_block();
        self.fb.set_term(Term::Br {
            cond: ok,
            then_e: Edge { to: some_b, args: vec![] },
            else_e: Edge { to: none_b, args: vec![] },
        });
        self.fb.switch_to(some_b);
        if let Some(c) = some {
            let tag = self.fb.const_int(c.tag as i128, IrTy::I32);
            self.fb.store(IrTy::I32, slot, tag);
            if let Some(fi) = c.fields.first() {
                let v = self.fb.load(a.ty, out);
                self.store_field(slot, agg, *fi, LVal::scalar(v, a.ty))?;
            }
        }
        self.fb.set_term(Term::Jump(Edge { to: join, args: vec![] }));
        self.fb.switch_to(none_b);
        if let Some(c) = none {
            let tag = self.fb.const_int(c.tag as i128, IrTy::I32);
            self.fb.store(IrTy::I32, slot, tag);
        }
        self.fb.set_term(Term::Jump(Edge { to: join, args: vec![] }));
        self.fb.switch_to(join);
        let _ = e;
        Ok(LVal {
            v: slot,
            ty: IrTy::Ptr,
            agg: Some(agg),
        })
    }

    /// Fold a call to a pure function whose arguments are all literals.
    ///
    /// Licensed by the effect row: an empty row means no IO, no allocation, no
    /// raise, no divergence, so the call is a function of its arguments alone.
    /// The oracle interpreter performs the evaluation, which means folding cannot
    /// disagree with running — the interpreter is the normative semantics.
    ///
    /// Bounded by a step budget. Folding is an optimisation, not a promise, and a
    /// compiler that hangs on `fib(40)` would be a worse trade than one that
    /// leaves the call alone.
    fn try_fold_pure_call(
        &mut self,
        name: &str,
        args: &[Expr],
        e: &Expr,
    ) -> Result<Option<LVal>, String> {
        let bare = name.rsplit('.').next().unwrap_or(name);
        let Some(idx) = self
            .l
            .co
            .fns
            .iter()
            .position(|f| self.l.sym(f.sig.name) == bare)
        else {
            return Ok(None);
        };
        let f = &self.l.co.fns[idx];
        // Empty row: nothing observable, and it terminates.
        if !f.inferred.is_empty() || !f.sig.generics.is_empty() {
            return Ok(None);
        }
        // Every argument must be a literal.
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            let ExprKind::Lit(l) = &a.kind else {
                return Ok(None);
            };
            let prim = self.ty_of_node(a.id).as_prim();
            match (l, prim) {
                (Lit::Int { value, .. }, Some(p)) if p.is_int() => vals.push(
                    crate::interp::Value::Int {
                        bits: p.wrap_i128(*value),
                        prim: p,
                    },
                ),
                (Lit::Float { value, .. }, Some(Prim::F32)) => {
                    vals.push(crate::interp::Value::f32(*value as f32))
                }
                (Lit::Float { value, .. }, Some(Prim::F64)) => {
                    vals.push(crate::interp::Value::f64(*value))
                }
                (Lit::Bool(b), _) => vals.push(crate::interp::Value::Bool(*b)),
                _ => return Ok(None),
            }
        }
        // Only scalar results are folded: materialising an aggregate constant
        // would need static initialiser support that is not there yet.
        let (ir, agg) = self.ir_of(e)?;
        if agg.is_some() || ir == IrTy::Unit || ir == IrTy::Ptr {
            return Ok(None);
        }
        let folded = crate::interp::fold_call(self.l.intern, self.l.co, bare, vals, FOLD_STEPS);
        let Some(v) = folded else { return Ok(None) };
        Ok(Some(match v {
            crate::interp::Value::Int { bits, .. } => {
                LVal::scalar(self.fb.const_int(bits, ir), ir)
            }
            crate::interp::Value::Bool(b) => LVal::scalar(self.fb.const_bool(b), IrTy::Bool),
            crate::interp::Value::Float { bits, prim } => {
                let x = if prim == Prim::F32 {
                    f32::from_bits(bits as u32) as f64
                } else {
                    f64::from_bits(bits)
                };
                LVal::scalar(self.fb.push(Op::ConstFloat(x), ir), ir)
            }
            _ => return Ok(None),
        }))
    }

    /// `test.read_cap({ "a.json": "..." })` — a capability backed by an
    /// in-memory file set, so a test never touches the real filesystem.
    fn lower_read_cap(&mut self, args: &[Expr]) -> Result<LVal, String> {
        let entries: Vec<(String, Expr)> = match args.first().map(|a| &a.kind) {
            Some(ExprKind::Record(fs)) => fs
                .iter()
                .map(|(n, e)| (self.l.sym(n.name), e.clone()))
                .collect(),
            _ => Vec::new(),
        };
        let str_agg = self.str_agg()?;
        let names = self.fb.alloc_slot(SlotKind::Agg(str_agg), "cap_names");
        let contents = self.fb.alloc_slot(SlotKind::Agg(str_agg), "cap_contents");
        // One slot pair per entry, laid out consecutively so the runtime can read
        // them as arrays. A single entry is the common case in tests.
        let mut extra = Vec::new();
        for i in 1..entries.len() {
            let n = self.fb.alloc_slot(SlotKind::Agg(str_agg), &format!("cap_names{i}"));
            let c = self
                .fb
                .alloc_slot(SlotKind::Agg(str_agg), &format!("cap_contents{i}"));
            extra.push((n, c));
        }
        if entries.len() > 1 {
            return Err(
                "native backend: `test.read_cap` currently supports one file per capability"
                    .into(),
            );
        }
        for (i, (name, value)) in entries.iter().enumerate() {
            let idx = self.l.intern_string(name);
            let data = self.fb.push(Op::ConstStr(idx), IrTy::Ptr);
            let len = self.fb.const_int(name.len() as i128, IrTy::U64);
            let (nslot, cslot) = if i == 0 {
                (names, contents)
            } else {
                extra[i - 1]
            };
            let dp = self.fb.field_ptr(str_agg, STR_DATA, nslot);
            self.fb.store(IrTy::Ptr, dp, data);
            let lp = self.fb.field_ptr(str_agg, STR_LEN, nslot);
            self.fb.store(IrTy::U64, lp, len);
            let v = self.expr(value)?;
            self.fb.push_void(Op::CopyAgg {
                ty: str_agg,
                dst: cslot,
                src: v.v,
            });
        }
        let n = self.fb.const_int(entries.len() as i128, IrTy::U64);
        let cap = self.fb.push(
            Op::CallExt {
                name: "ax_rt_read_cap_files".into(),
                args: vec![names, contents, n],
                ret: IrTy::Ptr,
                fallible: false,
            },
            IrTy::Ptr,
        );
        Ok(LVal::scalar(cap, IrTy::Ptr))
    }

    /// `fs.read(cap, a, path)` — raises `fs.Error.NotFound` when the capability
    /// does not name the file, or the path tries to leave its directory.
    fn lower_fs_read(&mut self, args: &[Expr], e: &Expr) -> Result<LVal, String> {
        let cap = self.expr(&args[0])?;
        let alloc = self.expr(&args[1])?;
        let path = self.expr(&args[2])?;
        let str_agg = self.str_agg()?;
        let out = self.fb.alloc_slot(SlotKind::Agg(str_agg), "contents");
        let ok = self.fb.push(
            Op::CallExt {
                name: "ax_rt_fs_read".into(),
                args: vec![cap.v, alloc.v, path.v, out],
                ret: IrTy::Bool,
                fallible: false,
            },
            IrTy::Bool,
        );
        let good = self.fb.new_block();
        let bad = self.fb.new_block();
        self.fb.set_term(Term::Br {
            cond: ok,
            then_e: Edge { to: good, args: vec![] },
            else_e: Edge { to: bad, args: vec![] },
        });
        self.fb.switch_to(bad);
        let payload = self.error_payload_for("fs.Error", "NotFound")?;
        self.emit_raise(payload)?;
        self.fb.switch_to(good);
        let _ = e;
        Ok(LVal {
            v: out,
            ty: IrTy::Ptr,
            agg: Some(str_agg),
        })
    }

    /// `json.decode_recs(a, raw)` — decode into `Vec[R]` using `R`'s layout
    /// descriptor, so there is one decoder rather than one per record type.
    fn lower_json_decode(&mut self, args: &[Expr], e: &Expr) -> Result<LVal, String> {
        let alloc = self.expr(&args[0])?;
        let raw = self.expr(&args[1])?;
        let (_, vec_agg) = self.ir_of(e)?;
        let vec_agg = vec_agg.ok_or("native backend: `json.decode_recs` with no Vec layout")?;
        let elem_ty = self.container_elem(&self.ty_of_node(e.id))?;
        let elem_agg = self
            .l
            .agg_of(&elem_ty)?
            .ok_or("native backend: `json.decode_recs` needs a record element type")?;
        let desc = self.fb.push(Op::TypeDescriptor(elem_agg), IrTy::Ptr);
        let out = self.fb.alloc_slot(SlotKind::Agg(vec_agg), "recs");
        let ok = self.fb.push(
            Op::CallExt {
                name: "ax_rt_json_decode_recs".into(),
                args: vec![alloc.v, raw.v, desc, out],
                ret: IrTy::Bool,
                fallible: false,
            },
            IrTy::Bool,
        );
        let good = self.fb.new_block();
        let bad = self.fb.new_block();
        self.fb.set_term(Term::Br {
            cond: ok,
            then_e: Edge { to: good, args: vec![] },
            else_e: Edge { to: bad, args: vec![] },
        });
        self.fb.switch_to(bad);
        let payload = self.error_payload_for("json.Error", "Invalid")?;
        self.emit_raise(payload)?;
        self.fb.switch_to(good);
        Ok(LVal {
            v: out,
            ty: IrTy::Ptr,
            agg: Some(vec_agg),
        })
    }

    /// Layout of the built-in string aggregate.
    fn str_agg(&mut self) -> Result<TypeId, String> {
        let t = Type::Named {
            def: self.l.intern_sym("String"),
            args: vec![],
        };
        self.l
            .agg_of(&t)?
            .ok_or_else(|| "internal: String has no layout".to_string())
    }

    /// `sort(&mut xs, cmp)`.
    ///
    /// The runtime sort takes a `int(*)(const void*, const void*)` callback, but
    /// an Ax comparator returns `Ordering` through a destination pointer. A
    /// generated trampoline bridges the two, so the sort itself stays one
    /// element-size-generic routine rather than being monomorphised per type.
    fn lower_sort(&mut self, args: &[Expr], e: &Expr) -> Result<LVal, String> {
        let seq = self.expr(&args[0])?;
        let agg = seq
            .agg
            .ok_or("native backend: `sort` needs a Vec or slice receiver")?;
        let a = self.l.prog.agg(agg).clone();
        let (Some(data_i), Some(len_i)) = (a.field_index("data"), a.field_index("len")) else {
            return Err("native backend: `sort` receiver has no data/len".into());
        };
        let elem_ty = self.container_elem(&self.ty_of_node(args[0].id))?;
        let elem_ir = self.l.ir_ty(&elem_ty)?;
        let elem_agg = if elem_ir == IrTy::Ptr {
            self.l.agg_of(&elem_ty)?
        } else {
            None
        };
        let elem_repr = match elem_agg {
            Some(x) => Repr::Agg(x),
            None => Repr::Scalar(elem_ir),
        };
        let dp = self.fb.field_ptr(agg, data_i, seq.v);
        let data = self.fb.load(IrTy::Ptr, dp);
        let lp = self.fb.field_ptr(agg, len_i, seq.v);
        let len = self.fb.load(IrTy::U64, lp);
        // `i32.cmp` is the default integer order: emit the specialised
        // mergesort and skip the Ordering trampoline.
        if matches!(elem_ir, IrTy::I32)
            && callee_name(&args[1], self.l.intern)
                .as_deref()
                .is_some_and(|n| n == "i32.cmp" || n.ends_with(".i32.cmp"))
        {
            self.fb.push_void(Op::CallExt {
                name: "ax_rt_sort_i32".into(),
                args: vec![data, len],
                ret: IrTy::Unit,
                fallible: false,
            });
            let _ = e;
            return Ok(LVal::scalar(self.fb.unit(), IrTy::Unit));
        }
        let cmp = self.expr(&args[1])?;
        let tramp = self.sort_trampoline(&elem_ty)?;
        let tramp_addr = self.fb.push(Op::FuncAddr(tramp), IrTy::Ptr);
        let sz = self.fb.push(Op::SizeOf(elem_repr), IrTy::U64);
        // The trampoline needs the comparator; it is passed through a
        // thread-local slot rather than a closure, because the C callback
        // signature has no room for an environment.
        self.fb.push_void(Op::CallExt {
            name: "ax_rt_sort_set_cmp".into(),
            args: vec![cmp.v],
            ret: IrTy::Unit,
            fallible: false,
        });
        self.fb.push_void(Op::CallExt {
            name: "ax_rt_sort".into(),
            args: vec![data, len, sz, tramp_addr],
            ret: IrTy::Unit,
            fallible: false,
        });
        let _ = e;
        Ok(LVal::scalar(self.fb.unit(), IrTy::Unit))
    }

    /// Generate (once per element type) the `int(const void*, const void*)`
    /// adapter that calls the current comparator and maps `Ordering` to -1/0/1.
    fn sort_trampoline(&mut self, elem_ty: &Type) -> Result<FuncId, String> {
        let key = format!("ax_sort_cmp_{}", sanitize(&elem_ty.display(self.l.intern)));
        if let Some(id) = self.l.fn_ids.get(&key) {
            return Ok(*id);
        }
        let ord_ty = Type::Named {
            def: self.l.intern_sym("Ordering"),
            args: vec![],
        };
        let ord_agg = self
            .l
            .agg_of(&ord_ty)?
            .ok_or("internal: Ordering has no layout")?;
        let fid = self.l.reserve_func(key.clone());
        let mut fb = FuncBuilder::new(fid, key, "core::fn:sort.cmp".into(), IrTy::I32);
        fb.func.pure = true;
        let a = fb.new_val(IrTy::Ptr);
        let b = fb.new_val(IrTy::Ptr);
        fb.func.params.push(a);
        fb.func.params.push(b);
        // Load the comparator, call it with the two element addresses, and read
        // the resulting tag.
        let cmp = fb.push(
            Op::CallExt {
                name: "ax_rt_sort_get_cmp".into(),
                args: Vec::new(),
                ret: IrTy::Ptr,
                fallible: false,
            },
            IrTy::Ptr,
        );
        let out = fb.alloc_slot(SlotKind::Agg(ord_agg), "ord");
        fb.push_void(Op::CallIndirect {
            ptr: cmp,
            args: vec![a, b, out],
            ret: IrTy::Unit,
        });
        let tag_ptr = fb.field_ptr(ord_agg, VARIANT_TAG_FIELD, out);
        let tag = fb.load(IrTy::I32, tag_ptr);
        let lt = self.l.prog.agg(ord_agg).case("Lt").map(|c| c.tag).unwrap_or(0);
        let gt = self.l.prog.agg(ord_agg).case("Gt").map(|c| c.tag).unwrap_or(2);
        let lt_v = fb.const_int(lt as i128, IrTy::I32);
        let gt_v = fb.const_int(gt as i128, IrTy::I32);
        let is_lt = fb.bin(BinKind::Eq, tag, lt_v);
        let is_gt = fb.bin(BinKind::Eq, tag, gt_v);
        let neg1 = fb.const_int(-1, IrTy::I32);
        let zero = fb.const_int(0, IrTy::I32);
        let one = fb.const_int(1, IrTy::I32);
        let gt_or_eq = fb.push(
            Op::Select {
                c: is_gt,
                a: one,
                b: zero,
            },
            IrTy::I32,
        );
        let res = fb.push(
            Op::Select {
                c: is_lt,
                a: neg1,
                b: gt_or_eq,
            },
            IrTy::I32,
        );
        fb.set_term(Term::Ret(Some(res)));
        let func = fb.finish();
        self.l.prog.funcs[fid as usize] = func;
        Ok(fid)
    }

    /// Build an error payload of source type `from_ty`, injecting it into the
    /// enclosing function's declared error type when they differ.
    ///
    /// A declared `from fs.Error => LoadIo` means a raised `fs.Error` arrives at
    /// the caller as `LoadError.LoadIo { cause }`. The injection is single-step
    /// and declared once, so this is a lookup rather than a search.
    fn error_payload_for(&mut self, from_ty: &str, case: &str) -> Result<LVal, String> {
        let target = self
            .error_agg()
            .ok_or_else(|| format!("native backend: raising `{from_ty}` needs an err[E] row"))?;
        let target_name = self.l.prog.agg(target).name.clone();

        // Same type: build the case directly.
        if target_name == from_ty {
            return self.build_variant_payload(target, case);
        }

        let inner_ty = Type::Named {
            def: self.l.intern_sym(from_ty),
            args: vec![],
        };
        let inner_agg = self
            .l
            .agg_of(&inner_ty)?
            .ok_or_else(|| format!("internal: `{from_ty}` has no layout"))?;
        let inner = self.build_variant_payload(inner_agg, case)?;
        self.inject_payload(inner, from_ty)
    }

    /// Wrap an already-built payload of type `from_ty` in the enclosing error
    /// type's declared injection variant.
    fn inject_payload(&mut self, inner: LVal, from_ty: &str) -> Result<LVal, String> {
        let target = self
            .error_agg()
            .ok_or_else(|| format!("native backend: raising `{from_ty}` needs an err[E] row"))?;
        let target_name = self.l.prog.agg(target).name.clone();
        if target_name == from_ty {
            return Ok(inner);
        }
        // The row must declare an injection from this type.
        let inj = self
            .l
            .co
            .injections
            .iter()
            .find(|(into, from, _)| *into == target_name && from == from_ty)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "native backend: no declared injection `from {from_ty} => ...` into \
                     `{target_name}`; add one to the type declaration"
                )
            })?;
        let (_, _, variant) = inj;
        let case_def = self
            .l
            .prog
            .agg(target)
            .case(&variant)
            .cloned()
            .ok_or_else(|| format!("native backend: `{target_name}` has no variant `{variant}`"))?;
        let slot = self.fb.alloc_slot(SlotKind::Agg(target), "injected");
        let tag = self.fb.const_int(case_def.tag as i128, IrTy::I32);
        self.fb.store(IrTy::I32, slot, tag);
        if let Some(fi) = case_def.fields.first() {
            self.store_field(slot, target, *fi, inner)?;
        }
        Ok(LVal {
            v: slot,
            ty: IrTy::Ptr,
            agg: Some(target),
        })
    }

    /// A payload value for one case of a variant aggregate, with no fields set.
    fn build_variant_payload(&mut self, agg: TypeId, case: &str) -> Result<LVal, String> {
        let tag = self
            .l
            .prog
            .agg(agg)
            .case(case)
            .map(|c| c.tag)
            .ok_or_else(|| {
                format!(
                    "native backend: `{}` has no variant `{case}`",
                    self.l.prog.agg(agg).name
                )
            })?;
        let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
        let t = self.fb.const_int(tag as i128, IrTy::I32);
        self.fb.store(IrTy::I32, slot, t);
        Ok(LVal {
            v: slot,
            ty: IrTy::Ptr,
            agg: Some(agg),
        })
    }

    /// Build an error payload naming one of the channel type's cases.
    fn error_payload(&mut self, case: &str) -> Result<LVal, String> {
        let agg = self
            .error_agg()
            .ok_or_else(|| format!("native backend: raising `{case}` needs an err[E] row"))?;
        let slot = self.fb.alloc_slot(SlotKind::Agg(agg), "");
        let tag = self
            .l
            .prog
            .agg(agg)
            .case(case)
            .map(|c| c.tag)
            .unwrap_or(0);
        let t = self.fb.const_int(tag as i128, IrTy::I32);
        self.fb.store(IrTy::I32, slot, t);
        Ok(LVal {
            v: slot,
            ty: IrTy::Ptr,
            agg: Some(agg),
        })
    }
}

fn print_fn(ty: IrTy) -> &'static str {
    match ty {
        IrTy::F32 | IrTy::F64 => "ax_rt_print_f64",
        IrTy::Bool => "ax_rt_print_bool",
        t if t.is_signed() => "ax_rt_print_i64",
        _ => "ax_rt_print_u64",
    }
}

/// `map.new(...)` — unique-heap Map allocation.
fn expr_is_map_new(e: &Expr, intern: &Interner) -> bool {
    match &e.kind {
        ExprKind::Call { callee, .. } => {
            callee_name(callee, intern).is_some_and(|n| n == "map.new" || n.ends_with(".map.new"))
        }
        _ => false,
    }
}

fn path_text(p: &Path, intern: &Interner) -> String {
    p.segs
        .iter()
        .map(|s| intern.get(s.name).to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// Dotted name of a call target, if it is a static path.
fn callee_name(callee: &Expr, intern: &Interner) -> Option<String> {
    match &callee.kind {
        ExprKind::Path(p) => Some(path_text(p, intern)),
        ExprKind::Field { base, field } => {
            let b = callee_name(base, intern)?;
            Some(format!("{b}.{}", intern.get(field.name)))
        }
        _ => None,
    }
}


/// Short name of an expression form, for lowering diagnostics.
fn expr_kind_name(k: &ExprKind) -> &'static str {
    match k {
        ExprKind::Lit(_) => "literal",
        ExprKind::Path(_) => "path",
        ExprKind::Hole => "hole",
        ExprKind::Call { .. } => "call",
        ExprKind::Field { .. } => "field access",
        ExprKind::Index { .. } => "index",
        ExprKind::Unary { .. } => "unary op",
        ExprKind::Binary { .. } => "binary op",
        ExprKind::Block { .. } => "block",
        ExprKind::If { .. } => "if",
        ExprKind::Match { .. } => "match",
        ExprKind::For { .. } => "for",
        ExprKind::Loop { .. } => "loop",
        ExprKind::While { .. } => "while",
        ExprKind::Break => "break",
        ExprKind::Continue => "continue",
        ExprKind::Cast { .. } => "cast",
        ExprKind::Let(_) => "let",
        ExprKind::Lambda { .. } => "lambda",
        ExprKind::Record(_) => "record literal",
        ExprKind::Variant { .. } => "variant literal",
        ExprKind::Return(_) => "return",
        ExprKind::Raise(_) => "raise",
        ExprKind::Catch { .. } => "catch",
        ExprKind::Attempt(_) => "attempt",
        ExprKind::Try(_) => "try",
        ExprKind::Interpolate { .. } => "interpolate",
        ExprKind::Region { .. } => "region",
        ExprKind::Par { .. } => "par",
        ExprKind::Assign { .. } => "assignment",
    }
}

fn type_is_map(t: &Type, intern: &Interner) -> bool {
    let bare = match t {
        Type::Ref { inner, .. }
        | Type::Own(inner)
        | Type::Untrusted(inner)
        | Type::Secret(inner) => inner.as_ref(),
        other => other,
    };
    match bare {
        Type::Named { def, .. } => {
            let n = intern.get(*def);
            n == "Map" || n == "SortedMap"
        }
        _ => false,
    }
}

/// Names referenced by an expression that are not in `bound`. Used to reject
/// capturing lambdas with a message that names the offending variable.
fn collect_free_names(e: &Expr, bound: &[Symbol], out: &mut Vec<Symbol>) {
    let mut bound = bound.to_vec();
    walk_free(e, &mut bound, out);
}

fn walk_free(e: &Expr, bound: &mut Vec<Symbol>, out: &mut Vec<Symbol>) {
    match &e.kind {
        ExprKind::Path(p) => {
            if p.segs.len() == 1 {
                let n = p.segs[0].name;
                if !bound.contains(&n) {
                    out.push(n);
                }
            }
        }
        ExprKind::Call { callee, args } => {
            walk_free(callee, bound, out);
            for a in args {
                walk_free(a, bound, out);
            }
        }
        ExprKind::Field { base, .. } => walk_free(base, bound, out),
        ExprKind::Index { base, index } => {
            walk_free(base, bound, out);
            walk_free(index, bound, out);
        }
        ExprKind::Unary { expr, .. } => walk_free(expr, bound, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            walk_free(lhs, bound, out);
            walk_free(rhs, bound, out);
        }
        ExprKind::Block { stmts, tail } => {
            let depth = bound.len();
            for s in stmts {
                match &s.kind {
                    StmtKind::Let(l) => {
                        walk_free(&l.init, bound, out);
                        bind_pat_names(&l.pat, bound);
                    }
                    StmtKind::Expr(x) => walk_free(x, bound, out),
                }
            }
            if let Some(t) = tail {
                walk_free(t, bound, out);
            }
            bound.truncate(depth);
        }
        ExprKind::If { cond, then_b, else_b } => {
            walk_free(cond, bound, out);
            walk_free(then_b, bound, out);
            if let Some(x) = else_b {
                walk_free(x, bound, out);
            }
        }
        ExprKind::Match { scrut, arms } | ExprKind::Catch { expr: scrut, arms } => {
            walk_free(scrut, bound, out);
            for a in arms {
                let depth = bound.len();
                bind_pat_names(&a.pat, bound);
                walk_free(&a.body, bound, out);
                bound.truncate(depth);
            }
        }
        ExprKind::For { pat, iter, body } => {
            walk_free(iter, bound, out);
            let depth = bound.len();
            bind_pat_names(pat, bound);
            walk_free(body, bound, out);
            bound.truncate(depth);
        }
        ExprKind::Loop { body } | ExprKind::Region { body, .. } => walk_free(body, bound, out),
        ExprKind::While { cond, body } => {
            walk_free(cond, bound, out);
            walk_free(body, bound, out);
        }
        ExprKind::Break | ExprKind::Continue => {}
        ExprKind::Cast { expr, .. } => walk_free(expr, bound, out),
        ExprKind::Let(l) => {
            walk_free(&l.init, bound, out);
            bind_pat_names(&l.pat, bound);
        }
        ExprKind::Lambda { params, body, .. } => {
            let depth = bound.len();
            for p in params {
                bound.push(p.name.name);
            }
            walk_free(body, bound, out);
            bound.truncate(depth);
        }
        ExprKind::Record(fs) | ExprKind::Variant { fields: fs, .. } => {
            for (_, x) in fs {
                walk_free(x, bound, out);
            }
        }
        ExprKind::Return(inner) => {
            if let Some(x) = inner {
                walk_free(x, bound, out);
            }
        }
        ExprKind::Raise(inner) | ExprKind::Attempt(inner) | ExprKind::Try(inner) => {
            walk_free(inner, bound, out)
        }
        ExprKind::Interpolate { parts } => {
            for p in parts {
                if let crate::ast::InterpPart::Expr(x) = p {
                    walk_free(x, bound, out);
                }
            }
        }
        ExprKind::Par { bindings } => {
            for l in bindings {
                walk_free(&l.init, bound, out);
                bind_pat_names(&l.pat, bound);
            }
        }
        ExprKind::Assign { lhs, rhs } => {
            walk_free(lhs, bound, out);
            walk_free(rhs, bound, out);
        }
        ExprKind::Lit(_) | ExprKind::Hole => {}
    }
}

fn bind_pat_names(p: &Pattern, bound: &mut Vec<Symbol>) {
    match &p.kind {
        PatKind::Bind(id) => bound.push(id.name),
        PatKind::Variant { fields, .. } | PatKind::Record(fields) => {
            for (_, sub) in fields {
                bind_pat_names(sub, bound);
            }
        }
        PatKind::Tuple(ps) => {
            for sub in ps {
                bind_pat_names(sub, bound);
            }
        }
        PatKind::Wild | PatKind::Lit(_) => {}
    }
}

/// Collect names used as the divisor of an integer `/` or `%` whose node the
/// checker marked unconditionally non-zero. The caller still has to prove the
/// name is loop-invariant; this walk only finds the candidates.
fn collect_invariant_divisors(
    e: &Expr,
    nonzero: &std::collections::HashSet<NodeId>,
    needs_guard: &std::collections::HashSet<NodeId>,
    out: &mut Vec<Symbol>,
) {
    if let ExprKind::Binary {
        op: BinOp::Div | BinOp::Rem,
        rhs,
        ..
    } = &e.kind
    {
        if nonzero.contains(&rhs.id) && !needs_guard.contains(&rhs.id) {
            if let ExprKind::Path(p) = &rhs.kind {
                if p.segs.len() == 1 {
                    out.push(p.segs[0].name);
                }
            }
        }
    }
    each_child(e, &mut |c| collect_invariant_divisors(c, nonzero, needs_guard, out));
}

/// Vecs filled by lockstep `push` in the same loop, never reassigned.
/// Equal length afterwards, so an index bounded by one is bounded by the other.
fn same_len_pairs(body: &Expr, intern: &Interner) -> Vec<(Symbol, Symbol)> {
    let mut out = Vec::new();
    walk_same_len(body, intern, &mut out);
    out
}

fn walk_same_len(e: &Expr, intern: &Interner, out: &mut Vec<(Symbol, Symbol)>) {
    match &e.kind {
        ExprKind::For { body, .. } | ExprKind::While { body, .. } | ExprKind::Loop { body } => {
            collect_lockstep_pushes(body, intern, out);
            walk_same_len(body, intern, out);
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match &s.kind {
                    StmtKind::Expr(x) => walk_same_len(x, intern, out),
                    StmtKind::Let(l) => walk_same_len(&l.init, intern, out),
                }
            }
            if let Some(t) = tail {
                walk_same_len(t, intern, out);
            }
        }
        ExprKind::If {
            then_b, else_b, ..
        } => {
            walk_same_len(then_b, intern, out);
            if let Some(el) = else_b {
                walk_same_len(el, intern, out);
            }
        }
        ExprKind::Region { body, .. } => walk_same_len(body, intern, out),
        _ => {}
    }
}

fn collect_lockstep_pushes(body: &Expr, intern: &Interner, out: &mut Vec<(Symbol, Symbol)>) {
    let mut pushed = Vec::new();
    gather_pushes(body, intern, &mut pushed);
    if pushed.len() < 2 {
        return;
    }
    for i in 0..pushed.len() {
        for j in (i + 1)..pushed.len() {
            let a = pushed[i];
            let b = pushed[j];
            if assigns_to(body, a) || assigns_to(body, b) {
                continue;
            }
            out.push((a, b));
        }
    }
}

fn gather_pushes(e: &Expr, intern: &Interner, out: &mut Vec<Symbol>) {
    match &e.kind {
        ExprKind::Call { callee, .. } => {
            if let Some(recv) = push_receiver(callee, intern) {
                if !out.contains(&recv) {
                    out.push(recv);
                }
            }
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match &s.kind {
                    StmtKind::Expr(x) => gather_pushes(x, intern, out),
                    StmtKind::Let(l) => gather_pushes(&l.init, intern, out),
                }
            }
            if let Some(t) = tail {
                gather_pushes(t, intern, out);
            }
        }
        ExprKind::If {
            then_b, else_b, ..
        } => {
            gather_pushes(then_b, intern, out);
            if let Some(el) = else_b {
                gather_pushes(el, intern, out);
            }
        }
        _ => {}
    }
}

fn push_receiver(callee: &Expr, intern: &Interner) -> Option<Symbol> {
    match &callee.kind {
        ExprKind::Field { base, field } => {
            if intern.get(field.name) == "push" {
                if let ExprKind::Path(p) = &base.kind {
                    if p.segs.len() == 1 {
                        return Some(p.segs[0].name);
                    }
                }
            }
            None
        }
        ExprKind::Path(p) if p.segs.len() >= 2 => {
            let last = p.segs.last()?;
            if intern.get(last.name) != "push" {
                return None;
            }
            (p.segs.len() == 2).then_some(p.segs[0].name)
        }
        _ => None,
    }
}

/// Does `e` grow `name` via `push` or `reserve`? Those can move `data`.
fn grows_vec(e: &Expr, name: Symbol, intern: &Interner) -> bool {
    match &e.kind {
        ExprKind::Call { callee, args } => {
            if let Some(recv) = push_receiver(callee, intern) {
                if recv == name {
                    return true;
                }
            }
            if let ExprKind::Field { base, field } = &callee.kind {
                if intern.get(field.name) == "reserve" {
                    if let ExprKind::Path(p) = &base.kind {
                        if p.segs.len() == 1 && p.segs[0].name == name {
                            return true;
                        }
                    }
                }
            }
            if grows_vec(callee, name, intern) {
                return true;
            }
            args.iter().any(|a| grows_vec(a, name, intern))
        }
        ExprKind::Block { stmts, tail } => {
            stmts.iter().any(|s| match &s.kind {
                StmtKind::Expr(x) => grows_vec(x, name, intern),
                StmtKind::Let(l) => grows_vec(&l.init, name, intern),
            }) || tail
                .as_ref()
                .is_some_and(|t| grows_vec(t, name, intern))
        }
        ExprKind::If {
            then_b, else_b, ..
        } => {
            grows_vec(then_b, name, intern)
                || else_b
                    .as_ref()
                    .is_some_and(|el| grows_vec(el, name, intern))
        }
        ExprKind::For { body, .. } | ExprKind::While { body, .. } | ExprKind::Loop { body } => {
            grows_vec(body, name, intern)
        }
        ExprKind::Region { body, .. } => grows_vec(body, name, intern),
        ExprKind::Binary { lhs, rhs, .. } => {
            grows_vec(lhs, name, intern) || grows_vec(rhs, name, intern)
        }
        ExprKind::Unary { expr, .. } | ExprKind::Field { base: expr, .. } => {
            grows_vec(expr, name, intern)
        }
        _ => false,
    }
}

fn assigns_to(e: &Expr, name: Symbol) -> bool {
    let mut found = false;
    walk_assigns(e, name, &mut found);
    found
}

fn walk_assigns(e: &Expr, name: Symbol, found: &mut bool) {
    if *found {
        return;
    }
    match &e.kind {
        ExprKind::Assign { lhs, rhs } => {
            if let ExprKind::Path(p) = &lhs.kind {
                if p.segs.first().map(|s| s.name) == Some(name) {
                    *found = true;
                    return;
                }
            }
            walk_assigns(lhs, name, found);
            walk_assigns(rhs, name, found);
        }
        ExprKind::Call { callee, args } => {
            walk_assigns(callee, name, found);
            for a in args {
                walk_assigns(a, name, found);
            }
        }
        ExprKind::Field { base, .. } => walk_assigns(base, name, found),
        ExprKind::Index { base, index } => {
            walk_assigns(base, name, found);
            walk_assigns(index, name, found);
        }
        ExprKind::Unary { expr, .. } => walk_assigns(expr, name, found),
        ExprKind::Binary { lhs, rhs, .. } => {
            walk_assigns(lhs, name, found);
            walk_assigns(rhs, name, found);
        }
        ExprKind::Block { stmts, tail } => {
            for st in stmts {
                match &st.kind {
                    StmtKind::Let(l) => walk_assigns(&l.init, name, found),
                    StmtKind::Expr(x) => walk_assigns(x, name, found),
                }
            }
            if let Some(t) = tail {
                walk_assigns(t, name, found);
            }
        }
        ExprKind::If { cond, then_b, else_b } => {
            walk_assigns(cond, name, found);
            walk_assigns(then_b, name, found);
            if let Some(x) = else_b {
                walk_assigns(x, name, found);
            }
        }
        ExprKind::Match { scrut, arms } | ExprKind::Catch { expr: scrut, arms } => {
            walk_assigns(scrut, name, found);
            for a in arms {
                walk_assigns(&a.body, name, found);
            }
        }
        ExprKind::For { iter, body, .. } => {
            walk_assigns(iter, name, found);
            walk_assigns(body, name, found);
        }
        ExprKind::While { cond, body } => {
            walk_assigns(cond, name, found);
            walk_assigns(body, name, found);
        }
        ExprKind::Loop { body } | ExprKind::Region { body, .. } => {
            walk_assigns(body, name, found)
        }
        ExprKind::Let(l) => walk_assigns(&l.init, name, found),
        ExprKind::Lambda { body, .. } => walk_assigns(body, name, found),
        ExprKind::Record(fs) | ExprKind::Variant { fields: fs, .. } => {
            for (_, x) in fs {
                walk_assigns(x, name, found);
            }
        }
        ExprKind::Return(inner) => {
            if let Some(x) = inner {
                walk_assigns(x, name, found);
            }
        }
        ExprKind::Raise(inner)
        | ExprKind::Attempt(inner)
        | ExprKind::Try(inner)
        | ExprKind::Cast { expr: inner, .. } => walk_assigns(inner, name, found),
        ExprKind::Interpolate { parts } => {
            for p in parts {
                if let crate::ast::InterpPart::Expr(x) = p {
                    walk_assigns(x, name, found);
                }
            }
        }
        ExprKind::Par { bindings } => {
            for l in bindings {
                walk_assigns(&l.init, name, found);
            }
        }
        ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Hole | ExprKind::Break
        | ExprKind::Continue => {}
    }
}

/// Does a parameter need frame storage, or can it stay an SSA value?
///
/// It needs a slot if the body assigns to it or takes its address; otherwise
/// reads can use the incoming value directly.
fn param_needs_slot(body: &Expr, name: Symbol) -> bool {
    assigns_to(body, name) || takes_address_of(body, name)
}

fn takes_address_of(e: &Expr, name: Symbol) -> bool {
    let mut found = false;
    walk_addr(e, name, &mut found);
    found
}

fn walk_addr(e: &Expr, name: Symbol, found: &mut bool) {
    if *found {
        return;
    }
    if let ExprKind::Unary {
        op: UnOp::Ref | UnOp::RefMut,
        expr,
    } = &e.kind
    {
        if let ExprKind::Path(p) = &expr.kind {
            if p.segs.first().map(|s| s.name) == Some(name) {
                *found = true;
                return;
            }
        }
    }
    each_child(e, &mut |c| walk_addr(c, name, found));
}

/// Apply `f` to each direct sub-expression. Keeps the several walkers in this
/// module from each re-listing every `ExprKind`.
fn each_child(e: &Expr, f: &mut impl FnMut(&Expr)) {
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
        ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Hole | ExprKind::Break
        | ExprKind::Continue => {}
    }
}
