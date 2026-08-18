//! Cranelift backend — the in-process tier, and a third opinion for the oracle.
//!
//! Called the *in-process* tier rather than the dev tier: `ax build --tier dev`
//! already means `cc -O0`, and one name for two things is how a claim about one
//! ends up being read as a claim about the other.
//!
//! This consumes [`crate::ir`] and nothing else, exactly as [`crate::backend_c`]
//! does. That is the point: two backends written against the same IR, run
//! against the interpreter on every conformance case, disagree loudly when
//! lowering is wrong. Agreement between a C emitter and a C compiler proves
//! much less than agreement between a C emitter, a machine-code emitter, and a
//! tree-walking interpreter.
//!
//! Differences from the C tier, all deliberate:
//!
//! - **Layout is the IR's.** `backend_c` uses C struct members and `sizeof`;
//!   here field offsets and sizes come from `AggDef`. Neither scheme leaks into
//!   the other, and layout is not observable in v1 (no struct FFI).
//! - **Exact integer semantics are open-coded.** The C tier calls `ax_div_i32`
//!   and friends, which are `static inline` in the header and therefore have no
//!   symbol to call. The guards are re-derived here as `select` chains, which
//!   also keeps Cranelift's trapping `sdiv` away from `INT_MIN / -1`.
//! - **No memoisation.** `Func::memoize` is ignored: a cache cannot change a
//!   pure function's result, so skipping it costs time and no correctness. This
//!   tier is for checking behaviour, and it doubles as the control that shows the
//!   cache changes nothing observable.
//! - **Runs in this process.** There is no linker step and no binary; `ax jit`
//!   compiles and calls. An abort therefore ends `ax jit` itself, which is why
//!   the conformance harness spawns it as a child. Code memory is released when
//!   the process exits — freeing it early is unsafe while any compiled function
//!   may still be called, and a one-shot run has nothing to gain from it.
//!
//! Anything not handled returns `Err`. A backend that silently skips an
//! operation is worse than one that refuses it: the differential suite would
//! then be comparing against something that never ran.

use crate::intern::Interner;
use crate::interp::Value;
use crate::ir::*;
use crate::types::Prim;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    types as clty, AbiParam, BlockArg, InstBuilder, MemFlagsData, Signature, StackSlot,
    StackSlotData, StackSlotKind, TrapCode, Type as ClType,
};
use cranelift_codegen::settings::{self, Configurable as _};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, FuncId as ClFuncId, Linkage, Module};
use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};

/// A trap that means "lowering produced something unreachable and we reached
/// it". Distinct from an Ax `abort`, which calls `ax_abort` first.
const TRAP_UNREACHABLE: u8 = 17;

// dlopen/dlsym rather than a `libc` dependency: three declarations against a
// stable platform ABI is less than a crate.
extern "C" {
    fn dlopen(path: *const std::ffi::c_char, flags: i32) -> *mut std::ffi::c_void;
    fn dlsym(
        handle: *mut std::ffi::c_void,
        symbol: *const std::ffi::c_char,
    ) -> *mut std::ffi::c_void;
    fn dlerror() -> *const std::ffi::c_char;
}

/// `RTLD_NOW` on both macOS and Linux.
const RTLD_NOW: i32 = 2;

/// The shared runtime, opened once per process.
struct Runtime {
    handle: *mut std::ffi::c_void,
}

impl Runtime {
    fn open() -> Result<Self, String> {
        let lib = build_runtime_dylib()?;
        let c = CString::new(lib.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let handle = unsafe { dlopen(c.as_ptr(), RTLD_NOW) };
        if handle.is_null() {
            let e = unsafe { dlerror() };
            let msg = if e.is_null() {
                "unknown dlopen failure".to_string()
            } else {
                unsafe { std::ffi::CStr::from_ptr(e) }
                    .to_string_lossy()
                    .into_owned()
            };
            return Err(format!("dlopen {}: {msg}", lib.display()));
        }
        Ok(Self { handle })
    }

    fn sym(&self, name: &str) -> Option<*const u8> {
        let c = CString::new(name).ok()?;
        let p = unsafe { dlsym(self.handle, c.as_ptr()) };
        if p.is_null() {
            None
        } else {
            Some(p as *const u8)
        }
    }

    fn require(&self, name: &str) -> Result<*const u8, String> {
        self.sym(name)
            .ok_or_else(|| format!("runtime symbol `{name}` not found in libaxrt"))
    }
}

/// Compile `runtime/*.c` into a shared library, reusing it when it is newer than
/// its sources. The JIT resolves every `ax_rt_*` call through this.
fn build_runtime_dylib() -> Result<PathBuf, String> {
    let rt = crate::codegen::runtime_dir();
    let sources = [rt.join("axrt.c"), rt.join("axlang.c")];
    let header = rt.join("axrt.h");
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/axrt");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let lib = out_dir.join(format!("libaxrt.{ext}"));

    let newest_src = sources
        .iter()
        .chain(std::iter::once(&header))
        .filter_map(|p| std::fs::metadata(p).ok()?.modified().ok())
        .max();
    let lib_time = std::fs::metadata(&lib).ok().and_then(|m| m.modified().ok());
    if let (Some(l), Some(s)) = (lib_time, newest_src) {
        if l >= s {
            return Ok(lib);
        }
    }

    let mut cmd = std::process::Command::new("cc");
    cmd.args(["-O2", "-std=c11", "-fPIC", "-shared", "-pthread"])
        .arg(format!("-I{}", rt.display()));
    for s in &sources {
        cmd.arg(s);
    }
    cmd.arg("-lm").arg("-o").arg(&lib);
    let out = cmd.output().map_err(|e| format!("spawn cc: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "building libaxrt failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(lib)
}

/// A JIT-compiled program, ready to run.
pub struct Jit {
    module: JITModule,
    rt: Runtime,
    ids: Vec<ClFuncId>,
    entry: ClFuncId,
    /// Test name -> shim, when the program has no `main`.
    test_entries: Vec<(String, ClFuncId)>,
    prog: Program,
}

/// What the entry shim writes back. Sized from the program, not guessed.
struct Slots {
    val: Vec<u8>,
    err: Vec<u8>,
    tag: i32,
}

pub fn compile(intern: &Interner, checked: &crate::check::CheckOutput) -> Result<Jit, String> {
    let prog = crate::lower::lower_program(intern, checked)?;
    Jit::new(prog)
}

impl Jit {
    pub fn new(prog: Program) -> Result<Self, String> {
        let rt = Runtime::open()?;
        // Anything the generated code needs that is not an Ax function — the
        // runtime, plus libcalls like `memcpy` that Cranelift inserts itself —
        // resolves through the runtime library, which is linked against libc.
        let mut flags = settings::builder();
        // Speed of compilation is the point of this tier.
        flags
            .set("opt_level", "none")
            .map_err(|e| format!("cranelift flag: {e}"))?;
        let isa_builder = cranelift_native::builder().map_err(|e| e.to_string())?;
        let isa = isa_builder
            .finish(settings::Flags::new(flags))
            .map_err(|e| e.to_string())?;
        let mut builder = JITBuilder::with_isa(isa, default_libcall_names());
        let handle = rt.handle as usize;
        builder.symbol_lookup_fn(Box::new(move |name: &str| {
            let c = CString::new(name).ok()?;
            let p = unsafe { dlsym(handle as *mut std::ffi::c_void, c.as_ptr()) };
            if p.is_null() {
                None
            } else {
                Some(p as *const u8)
            }
        }));
        let mut module = JITModule::new(builder);
        let ptr_ty = module.target_config().pointer_type();

        // Declare every function first: calls and function addresses are
        // resolved by id, so mutual recursion needs no ordering.
        let mut ids = Vec::with_capacity(prog.funcs.len());
        for f in &prog.funcs {
            let sig = signature_of(&module, f, ptr_ty);
            let id = module
                .declare_function(&f.name, Linkage::Local, &sig)
                .map_err(|e| format!("declare @{}: {e}", f.name))?;
            ids.push(id);
        }

        let mut ctx = module.make_context();
        let mut fbctx = FunctionBuilderContext::new();

        // Static data the generated code refers to by address. Both are built
        // now, in this process, and baked in as constants — a JIT may do that,
        // where an object-file backend would need relocations.
        let mut strings = Vec::with_capacity(prog.strings.len());
        for s in &prog.strings {
            let c = CString::new(s.as_bytes()).map_err(|_| "string literal contains a NUL")?;
            strings.push(c.into_raw() as usize);
        }
        let descriptors = build_descriptors(&prog, &rt)?;

        for (i, f) in prog.funcs.iter().enumerate() {
            ctx.func.signature = signature_of(&module, f, ptr_ty);
            ctx.func.name = cranelift_codegen::ir::UserFuncName::user(0, i as u32);
            {
                let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
                let mut t = Trans {
                    prog: &prog,
                    f,
                    ids: &ids,
                    module: &mut module,
                    b: &mut b,
                    ptr_ty,
                    vars: HashMap::new(),
                    blocks: Vec::new(),
                    slots: Vec::new(),
                    arenas: HashMap::new(),
                    allocs: HashMap::new(),
                    strings: &strings,
                    descriptors: &descriptors,
                    ext: HashMap::new(),
                };
                t.func()?;
                let fc = module.target_config();
                b.finalize(fc);
            }
            module
                .define_function(ids[i], &mut ctx)
                .map_err(|e| format!("cranelift rejected @{}: {e}", f.name))?;
            module.clear_context(&mut ctx);
        }

        // Entry shims. `main`'s value has to come back to Rust to be rendered,
        // and Cranelift's multi-value return is not a Rust ABI, so the shim
        // writes through pointers instead.
        let (entry, test_entries) = build_entries(&prog, &mut module, &mut ctx, &mut fbctx, &ids, ptr_ty)?;

        module
            .finalize_definitions()
            .map_err(|e| format!("finalize: {e}"))?;
        Ok(Self {
            module,
            rt,
            ids,
            entry,
            test_entries,
            prog,
        })
    }

    /// Number of functions compiled. Used by tests to check the tier ran at all.
    pub fn func_count(&self) -> usize {
        self.ids.len()
    }

    /// Run `main` (or the tests) and return what should be printed.
    pub fn run(&self, argv: &[String]) -> Result<String, String> {
        // The runtime holds argv for `argv(i)`, so it must be initialised with
        // the real arguments exactly as the C tier's `main` does.
        let init = self.rt.require("ax_rt_init")?;
        let cargs: Vec<CString> = argv
            .iter()
            .map(|a| CString::new(a.as_str()).unwrap_or_default())
            .collect();
        let mut ptrs: Vec<*const std::ffi::c_char> = cargs.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        unsafe {
            let f: extern "C" fn(i32, *const *const std::ffi::c_char) =
                std::mem::transmute(init);
            f(cargs.len() as i32, ptrs.as_ptr());
        }

        let out = match self.prog.main {
            Some(m) => self.run_main(m)?,
            None => self.run_tests()?,
        };

        let shutdown = self.rt.require("ax_rt_shutdown")?;
        unsafe {
            let f: extern "C" fn() = std::mem::transmute(shutdown);
            f();
        }
        Ok(out)
    }

    fn run_main(&self, m: FuncId) -> Result<String, String> {
        let f = self.prog.func(m);
        let mut slots = Slots {
            val: vec![0u8; self.prog.slot_size(f.ret_agg, f.ret)],
            err: vec![0u8; err_slot_size(&self.prog, f)],
            tag: 0,
        };
        let code = self.module.get_finalized_function(self.entry);
        unsafe {
            let g: extern "C" fn(*mut u8, *mut u8, *mut i32) = std::mem::transmute(code);
            g(slots.val.as_mut_ptr(), slots.err.as_mut_ptr(), &mut slots.tag);
        }
        if f.is_fallible() && slots.tag != 0 {
            // Same text as the C tier and the oracle: an uncaught raise out of
            // `main` is an abort, not a value.
            return Err("uncaught raise from main".to_string());
        }
        Ok(render_return(&self.prog, f, &slots.val))
    }

    fn run_tests(&self) -> Result<String, String> {
        let mut out = String::new();
        let mut failed = 0usize;
        for (name, id) in &self.test_entries {
            let code = self.module.get_finalized_function(*id);
            let ok = unsafe {
                let g: extern "C" fn() -> i32 = std::mem::transmute(code);
                g() != 0
            };
            if ok {
                out.push_str(&format!("pass {name}\n"));
            } else {
                out.push_str(&format!("FAIL {name}\n"));
                failed += 1;
            }
        }
        if failed > 0 {
            return Err(format!("{failed} test(s) failed\n{out}"));
        }
        Ok(out.trim_end().to_string())
    }
}

impl Program {
    /// Bytes a returned value needs. Aggregates are written through a pointer.
    fn slot_size(&self, agg: Option<TypeId>, ret: IrTy) -> usize {
        match agg {
            Some(a) => self.agg(a).size.max(1) as usize,
            None => ret.size().max(8) as usize,
        }
    }
}

fn err_slot_size(p: &Program, f: &Func) -> usize {
    match f.err.as_ref().and_then(|c| c.agg) {
        Some(a) => p.agg(a).size.max(1) as usize,
        None => 8,
    }
}

/// Build a runtime layout descriptor for every aggregate the program asks about.
fn build_descriptors(p: &Program, rt: &Runtime) -> Result<HashMap<TypeId, usize>, String> {
    let mut needed: Vec<TypeId> = Vec::new();
    for f in &p.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                if let Op::TypeDescriptor(t) = i.op {
                    if !needed.contains(&t) {
                        needed.push(t);
                    }
                }
            }
        }
    }
    let mut out = HashMap::new();
    if needed.is_empty() {
        return Ok(out);
    }
    let new_fn = rt.require("ax_desc_new")?;
    let field_fn = rt.require("ax_desc_field")?;
    let new_fn: extern "C" fn(*const std::ffi::c_char, u32, u32) -> *mut std::ffi::c_void =
        unsafe { std::mem::transmute(new_fn) };
    let field_fn: extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, u32, i32) =
        unsafe { std::mem::transmute(field_fn) };
    for t in needed {
        let a = p.agg(t);
        let described: Vec<&FieldDef> = a.fields.iter().filter(|f| field_kind(f).is_some()).collect();
        let name = CString::new(a.name.as_str()).map_err(|e| e.to_string())?;
        let d = new_fn(name.into_raw(), a.size, described.len() as u32);
        for f in described {
            let fname = CString::new(f.name.as_str()).map_err(|e| e.to_string())?;
            field_fn(d, fname.into_raw(), f.offset, field_kind(f).unwrap());
        }
        out.insert(t, d as usize);
    }
    Ok(out)
}

/// Runtime field-kind code, matching `AxFieldKind` and `backend_c::field_kind`.
fn field_kind(f: &FieldDef) -> Option<i32> {
    if f.agg.is_some() {
        return if f.src == "String" || f.src == "str" || f.src.ends_with(" str") {
            Some(12) // AX_FLD_STR
        } else {
            None
        };
    }
    Some(match f.ty {
        IrTy::I8 => 0,
        IrTy::I16 => 1,
        IrTy::I32 => 2,
        IrTy::I64 => 3,
        IrTy::U8 => 4,
        IrTy::U16 => 5,
        IrTy::U32 => 6,
        IrTy::U64 => 7,
        IrTy::F32 => 8,
        IrTy::F64 => 9,
        IrTy::Bool => 10,
        IrTy::Unit | IrTy::Ptr => return None,
    })
}

fn clif_ty(t: IrTy, ptr: ClType) -> ClType {
    match t {
        // Unit has no bits, but a value has to have a type; one byte, never read.
        IrTy::Unit | IrTy::Bool | IrTy::I8 | IrTy::U8 => clty::I8,
        IrTy::I16 | IrTy::U16 => clty::I16,
        IrTy::I32 | IrTy::U32 => clty::I32,
        IrTy::I64 | IrTy::U64 => clty::I64,
        IrTy::F32 => clty::F32,
        IrTy::F64 => clty::F64,
        IrTy::Ptr => ptr,
    }
}

/// Signature of an Ax function.
///
/// Mirrors the C tier's shape so both are reading the same IR the same way: the
/// hidden aggregate-return and error-payload pointers are already in
/// `Func::params`, and a fallible function returns its payload plus an `i32`
/// tag. As in C, a scalar error payload travels *in* the tag.
fn signature_of(m: &JITModule, f: &Func, ptr: ClType) -> Signature {
    let mut sig = m.make_signature();
    for p in &f.params {
        sig.params.push(AbiParam::new(clif_ty(f.ty_of(*p), ptr)));
    }
    let has_payload = f.ret_agg.is_none() && f.ret != IrTy::Unit;
    if has_payload {
        sig.returns.push(AbiParam::new(clif_ty(f.ret, ptr)));
    }
    if f.is_fallible() {
        sig.returns.push(AbiParam::new(clty::I32));
    }
    sig
}

struct Trans<'a, 'b> {
    prog: &'a Program,
    f: &'a Func,
    ids: &'a [ClFuncId],
    module: &'a mut JITModule,
    b: &'a mut FunctionBuilder<'b>,
    ptr_ty: ClType,
    /// One Cranelift variable per IR value. Using variables rather than raw SSA
    /// values means the frontend inserts the block arguments, so lowering is
    /// free to use a value defined in another block the way the C tier does with
    /// function-scoped locals.
    vars: HashMap<ValId, Variable>,
    blocks: Vec<cranelift_codegen::ir::Block>,
    slots: Vec<StackSlot>,
    arenas: HashMap<RegionIdx, StackSlot>,
    allocs: HashMap<RegionIdx, StackSlot>,
    strings: &'a [usize],
    descriptors: &'a HashMap<TypeId, usize>,
    ext: HashMap<String, cranelift_codegen::ir::FuncRef>,
}

impl<'a, 'b> Trans<'a, 'b> {
    fn func(&mut self) -> Result<(), String> {
        let flags = MemFlagsData::new();
        let _ = flags;
        // Frame storage: one slot per IR slot, sized from the IR's layout.
        for s in &self.f.slots {
            let (size, align) = repr_size_align(self.prog, s.kind);
            let ss = self.b.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                size.max(1),
                align_shift(align),
            ));
            self.slots.push(ss);
        }
        // Region arenas and their allocator handles, sized by asking the runtime
        // rather than by assuming its layout.
        if !self.f.regions.is_empty() {
            let (asz, aal, hsz, hal) = runtime_slot_sizes()?;
            for (i, _) in self.f.regions.iter().enumerate() {
                let arena = self.b.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    asz,
                    align_shift(aal),
                ));
                let handle = self.b.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    hsz,
                    align_shift(hal),
                ));
                self.arenas.insert(i as RegionIdx, arena);
                self.allocs.insert(i as RegionIdx, handle);
            }
        }

        // A dedicated entry block receives the parameters; the IR's entry block
        // may be a branch target, and Cranelift's entry block may not be.
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        for _ in 0..self.f.blocks.len() {
            let blk = self.b.create_block();
            self.blocks.push(blk);
        }
        for (i, ib) in self.f.blocks.iter().enumerate() {
            for p in &ib.params {
                let t = clif_ty(self.f.ty_of(*p), self.ptr_ty);
                self.b.append_block_param(self.blocks[i], t);
            }
        }

        self.b.switch_to_block(entry);
        let params: Vec<_> = self.b.block_params(entry).to_vec();
        for (p, v) in self.f.params.iter().zip(params) {
            let var = self.declare(*p);
            self.b.def_var(var, v);
        }
        let target = self.blocks[self.f.entry as usize];
        self.b.ins().jump(target, &[]);

        for (i, ib) in self.f.blocks.iter().enumerate() {
            let blk = self.blocks[i];
            self.b.switch_to_block(blk);
            let bps: Vec<_> = self.b.block_params(blk).to_vec();
            for (p, v) in ib.params.iter().zip(bps) {
                let var = self.declare(*p);
                self.b.def_var(var, v);
            }
            for inst in &ib.insts {
                self.inst(inst)?;
            }
            self.term(&ib.term)?;
        }
        self.b.seal_all_blocks();
        Ok(())
    }

    fn declare(&mut self, v: ValId) -> Variable {
        let ty = clif_ty(self.f.ty_of(v), self.ptr_ty);
        *self
            .vars
            .entry(v)
            .or_insert_with(|| self.b.declare_var(ty))
    }

    fn get(&mut self, v: ValId) -> cranelift_codegen::ir::Value {
        let var = self.declare(v);
        self.b.use_var(var)
    }

    fn set(&mut self, v: ValId, val: cranelift_codegen::ir::Value) {
        let var = self.declare(v);
        self.b.def_var(var, val);
    }

    fn inst(&mut self, i: &Inst) -> Result<(), String> {
        match &i.op {
            Op::ConstInt(n) => {
                let d = i.results[0];
                let t = clif_ty(self.f.ty_of(d), self.ptr_ty);
                let bits = t.bits().min(64);
                let masked = (*n as u64) & mask(bits);
                // iconst takes the raw bit pattern; sign is carried by the type.
                let v = self.b.ins().iconst(t, masked as i64);
                self.set(d, v);
            }
            Op::ConstFloat(x) => {
                let d = i.results[0];
                let t = self.f.ty_of(d);
                let v = if x.is_nan() {
                    // Take the runtime's canonical NaN, so every tier agrees on
                    // the bit pattern a NaN literal has.
                    let name = if t == IrTy::F32 { "ax_nan_f32" } else { "ax_nan_f64" };
                    let r = self.call_ext_raw(name, &[], Some(t))?;
                    r.ok_or("ax_nan returned nothing")?
                } else if t == IrTy::F32 {
                    self.b.ins().f32const(*x as f32)
                } else {
                    self.b.ins().f64const(*x)
                };
                self.set(d, v);
            }
            Op::ConstBool(x) => {
                let d = i.results[0];
                let v = self.b.ins().iconst(clty::I8, i64::from(*x));
                self.set(d, v);
            }
            Op::ConstUnit => {
                let d = i.results[0];
                let v = self.b.ins().iconst(clty::I8, 0);
                self.set(d, v);
            }
            Op::ConstStr(idx) => {
                let d = i.results[0];
                let addr = *self
                    .strings
                    .get(*idx as usize)
                    .ok_or("string index out of range")?;
                let v = self.b.ins().iconst(self.ptr_ty, addr as i64);
                self.set(d, v);
            }
            Op::Bin { op, l, r } => {
                let d = i.results[0];
                let v = self.bin(*op, *l, *r)?;
                self.set(d, v);
            }
            Op::Un { op, v } => {
                let d = i.results[0];
                let x = self.get(*v);
                let t = self.f.ty_of(*v);
                let out = match op {
                    UnKind::Neg => self.b.ins().ineg(x),
                    UnKind::FNeg => self.b.ins().fneg(x),
                    // `!b` on a 0/1 byte.
                    UnKind::Not => self.b.ins().icmp_imm_s(IntCC::Equal, x, 0),
                    UnKind::BitNot => self.b.ins().bnot(x),
                    UnKind::CanonNaN => {
                        let name = if t == IrTy::F32 { "ax_canon_f32" } else { "ax_canon_f64" };
                        self.call_ext_raw(name, &[x], Some(t))?
                            .ok_or("ax_canon returned nothing")?
                    }
                };
                self.set(d, out);
            }
            Op::Cast { kind, v } => {
                let d = i.results[0];
                let out = self.cast(*kind, *v, self.f.ty_of(d))?;
                self.set(d, out);
            }
            Op::Select { c, a, b } => {
                let d = i.results[0];
                let (c, a, b) = (self.get(*c), self.get(*a), self.get(*b));
                let v = self.b.ins().select(c, a, b);
                self.set(d, v);
            }
            Op::Load { ty, ptr } => {
                let d = i.results[0];
                if *ty == IrTy::Unit {
                    let v = self.b.ins().iconst(clty::I8, 0);
                    self.set(d, v);
                } else {
                    let p = self.get(*ptr);
                    let t = clif_ty(*ty, self.ptr_ty);
                    let v = self.b.ins().load(t, MemFlagsData::new(), p, 0);
                    self.set(d, v);
                }
            }
            Op::Store { ty, ptr, val } => {
                if *ty != IrTy::Unit {
                    let (p, x) = (self.get(*ptr), self.get(*val));
                    self.b.ins().store(MemFlagsData::new(), x, p, 0);
                }
            }
            Op::FieldPtr { agg, field, ptr } => {
                let d = i.results[0];
                let off = self.prog.agg(*agg).field(*field).offset as i64;
                let p = self.get(*ptr);
                let v = self.b.ins().iadd_imm_s(p, off);
                self.set(d, v);
            }
            Op::ElemPtr { elem, ptr, idx } => {
                let d = i.results[0];
                let (stride, _) = repr_size_align(self.prog, *elem);
                let p = self.get(*ptr);
                let ix = self.get(*idx);
                let ix = self.to_index(ix, self.f.ty_of(*idx));
                let off = self.b.ins().imul_imm_s(ix, stride.max(1) as i64);
                let v = self.b.ins().iadd(p, off);
                self.set(d, v);
            }
            Op::CopyAgg { ty, dst, src } => {
                let size = self.prog.agg(*ty).size.max(1) as u64;
                let (dp, sp) = (self.get(*dst), self.get(*src));
                let config = self.module.target_config();
                // Alignment claimed as 1: `emit_small_memory_copy` asserts that
                // the largest power of two dividing `size` is at least the
                // claimed alignment, which a padded aggregate need not satisfy.
                self.b.emit_small_memory_copy(
                    config,
                    dp,
                    sp,
                    size,
                    1,
                    1,
                    true,
                    MemFlagsData::new(),
                );
            }
            Op::SlotAddr(s) => {
                let d = i.results[0];
                let ss = *self
                    .slots
                    .get(*s as usize)
                    .ok_or_else(|| format!("@{}: slot {s} out of range", self.f.name))?;
                let v = self.b.ins().stack_addr(self.ptr_ty, ss, 0);
                self.set(d, v);
            }
            Op::FuncAddr(fid) => {
                let d = i.results[0];
                let cl = self.ids[*fid as usize];
                let fr = self.module.declare_func_in_func(cl, self.b.func);
                let v = self.b.ins().func_addr(self.ptr_ty, fr);
                self.set(d, v);
            }
            Op::RegionEnter(r) => {
                let arena = self.arena_addr(*r)?;
                self.call_ext_raw("ax_arena_init", &[arena], None)?;
                let handle = self.alloc_addr(*r)?;
                self.call_ext_raw("ax_alloc_bind_arena", &[handle, arena], None)?;
            }
            Op::RegionExit(r) => {
                let arena = self.arena_addr(*r)?;
                self.call_ext_raw("ax_arena_release", &[arena], None)?;
            }
            Op::RegionAllocHandle(r) => {
                let d = i.results[0];
                let v = self.alloc_addr(*r)?;
                self.set(d, v);
            }
            Op::RegionAlloc {
                region,
                size,
                align,
            } => {
                let d = i.results[0];
                let arena = self.arena_addr(*region)?;
                let sz = self.get(*size);
                let sz = self.to_index(sz, self.f.ty_of(*size));
                let al = self.b.ins().iconst(clty::I32, *align as i64);
                let v = self
                    .call_ext_raw("ax_arena_alloc", &[arena, sz, al], Some(IrTy::Ptr))?
                    .ok_or("ax_arena_alloc returned nothing")?;
                self.set(d, v);
            }
            Op::TypeDescriptor(t) => {
                let d = i.results[0];
                let addr = *self
                    .descriptors
                    .get(t)
                    .ok_or_else(|| format!("no descriptor built for %{t}"))?;
                let v = self.b.ins().iconst(self.ptr_ty, addr as i64);
                self.set(d, v);
            }
            Op::SizeOf(r) => {
                let d = i.results[0];
                let (size, _) = repr_size_align(self.prog, *r);
                let v = self.b.ins().iconst(clty::I64, size as i64);
                self.set(d, v);
            }
            Op::UniqueAlloc { size, align } => {
                let d = i.results[0];
                let argv = [self.get(*size), self.b.ins().iconst(clty::I32, *align as i64)];
                let v = self
                    .call_ext_raw("ax_rt_unique_alloc", &argv, Some(IrTy::Ptr))?
                    .ok_or("unique_alloc returned nothing")?;
                self.set(d, v);
            }
            Op::UniqueFree(p) => {
                let argv = [self.get(*p)];
                let _ = self.call_ext_raw("ax_rt_unique_free", &argv, None)?;
            }
            Op::RcAlloc {
                size,
                align,
                atomic,
            } => {
                let d = i.results[0];
                let argv = [
                    self.get(*size),
                    self.b.ins().iconst(clty::I32, *align as i64),
                    self.b.ins().iconst(clty::I32, if *atomic { 1 } else { 0 }),
                ];
                let v = self
                    .call_ext_raw("ax_rt_rc_alloc", &argv, Some(IrTy::Ptr))?
                    .ok_or("rc_alloc returned nothing")?;
                self.set(d, v);
            }
            Op::RcRetain(p) => {
                let argv = [self.get(*p)];
                let _ = self.call_ext_raw("ax_rt_rc_retain", &argv, None)?;
            }
            Op::RcRelease(p) => {
                let argv = [self.get(*p)];
                let _ = self.call_ext_raw("ax_rt_rc_release", &argv, None)?;
            }
            Op::Call { f: callee, args } => {
                let c = self.prog.func(*callee);
                let cl = self.ids[*callee as usize];
                let fr = self.module.declare_func_in_func(cl, self.b.func);
                let argv: Vec<_> = args.iter().map(|a| self.get(*a)).collect();
                let call = self.b.ins().call(fr, &argv);
                let results: Vec<_> = self.b.inst_results(call).to_vec();
                self.bind_call_results(c, i, &results)?;
            }
            Op::CallExt {
                name, args, ret, ..
            } => {
                let argv: Vec<_> = args.iter().map(|a| self.get(*a)).collect();
                let want = if *ret == IrTy::Unit { None } else { Some(*ret) };
                let out = self.call_ext_raw(name, &argv, want)?;
                if let (Some(d), Some(v)) = (i.result(), out) {
                    self.set(d, v);
                }
            }
            Op::CallIndirect { ptr, args, ret } => {
                let mut sig = self.module.make_signature();
                for a in args {
                    sig.params
                        .push(AbiParam::new(clif_ty(self.f.ty_of(*a), self.ptr_ty)));
                }
                if *ret != IrTy::Unit {
                    sig.returns
                        .push(AbiParam::new(clif_ty(*ret, self.ptr_ty)));
                }
                let sr = self.b.import_signature(sig);
                let callee = self.get(*ptr);
                let argv: Vec<_> = args.iter().map(|a| self.get(*a)).collect();
                let call = self.b.ins().call_indirect(sr, callee, &argv);
                let results: Vec<_> = self.b.inst_results(call).to_vec();
                if let (Some(d), Some(v)) = (i.result(), results.first().copied()) {
                    self.set(d, v);
                }
            }
        }
        Ok(())
    }

    /// Bind the results of a call to an Ax function: payload, then error tag.
    fn bind_call_results(
        &mut self,
        c: &Func,
        i: &Inst,
        results: &[cranelift_codegen::ir::Value],
    ) -> Result<(), String> {
        let has_payload = c.ret_agg.is_none() && c.ret != IrTy::Unit;
        let mut k = 0usize;
        if has_payload {
            if let Some(d) = i.results.first().copied() {
                if self.f.ty_of(d) != IrTy::Unit {
                    let v = results[k];
                    self.set(d, v);
                }
            }
            k += 1;
        }
        if c.is_fallible() {
            let tag = i
                .results
                .get(1)
                .copied()
                .ok_or("internal: call to a fallible function must define a tag")?;
            let v = results
                .get(k)
                .copied()
                .ok_or("internal: fallible callee returned no tag")?;
            let v = self.fit_int(v, clif_ty(self.f.ty_of(tag), self.ptr_ty));
            self.set(tag, v);
        }
        Ok(())
    }

    fn arena_addr(&mut self, r: RegionIdx) -> Result<cranelift_codegen::ir::Value, String> {
        let ss = *self
            .arenas
            .get(&r)
            .ok_or_else(|| format!("@{}: region r{r} has no arena", self.f.name))?;
        Ok(self.b.ins().stack_addr(self.ptr_ty, ss, 0))
    }

    fn alloc_addr(&mut self, r: RegionIdx) -> Result<cranelift_codegen::ir::Value, String> {
        let ss = *self
            .allocs
            .get(&r)
            .ok_or_else(|| format!("@{}: region r{r} has no allocator handle", self.f.name))?;
        Ok(self.b.ins().stack_addr(self.ptr_ty, ss, 0))
    }

    /// Call a C function in the runtime by name.
    fn call_ext_raw(
        &mut self,
        name: &str,
        args: &[cranelift_codegen::ir::Value],
        ret: Option<IrTy>,
    ) -> Result<Option<cranelift_codegen::ir::Value>, String> {
        // Inline helpers in `axrt.h` have no `dlsym` symbol. The exported
        // `ax_rt_*` wrappers are the same code; map the names Cranelift can
        // actually resolve.
        let name = match name {
            "ax_recip_m" => "ax_rt_recip_m",
            "ax_recip_more" => "ax_rt_recip_more",
            "ax_div_recip" => "ax_rt_div_recip",
            "ax_rem_recip" => "ax_rt_rem_recip",
            other => other,
        };
        let fr = match self.ext.get(name) {
            Some(fr) => *fr,
            None => {
                let mut sig = self.module.make_signature();
                for a in args {
                    let t = self.b.func.dfg.value_type(*a);
                    sig.params.push(AbiParam::new(t));
                }
                if let Some(r) = ret {
                    sig.returns.push(AbiParam::new(clif_ty(r, self.ptr_ty)));
                }
                let id = self
                    .module
                    .declare_function(name, Linkage::Import, &sig)
                    .map_err(|e| format!("declare external {name}: {e}"))?;
                let fr = self.module.declare_func_in_func(id, self.b.func);
                self.ext.insert(name.to_string(), fr);
                fr
            }
        };
        let call = self.b.ins().call(fr, args);
        let results = self.b.inst_results(call);
        Ok(results.first().copied())
    }

    /// Widen or narrow an integer to a pointer-sized index.
    fn to_index(
        &mut self,
        v: cranelift_codegen::ir::Value,
        ty: IrTy,
    ) -> cranelift_codegen::ir::Value {
        let want = clty::I64;
        let have = self.b.func.dfg.value_type(v);
        if have == want {
            return v;
        }
        if have.bits() < want.bits() {
            if ty.is_signed() {
                self.b.ins().sextend(want, v)
            } else {
                self.b.ins().uextend(want, v)
            }
        } else {
            self.b.ins().ireduce(want, v)
        }
    }

    fn fit_int(
        &mut self,
        v: cranelift_codegen::ir::Value,
        want: ClType,
    ) -> cranelift_codegen::ir::Value {
        let have = self.b.func.dfg.value_type(v);
        if have == want {
            v
        } else if have.bits() < want.bits() {
            self.b.ins().uextend(want, v)
        } else {
            self.b.ins().ireduce(want, v)
        }
    }

    fn cast(
        &mut self,
        kind: CastKind,
        v: ValId,
        to: IrTy,
    ) -> Result<cranelift_codegen::ir::Value, String> {
        let from = self.f.ty_of(v);
        let x = self.get(v);
        let tt = clif_ty(to, self.ptr_ty);
        let ft = clif_ty(from, self.ptr_ty);
        Ok(match kind {
            CastKind::SExt => {
                if tt.bits() > ft.bits() {
                    self.b.ins().sextend(tt, x)
                } else if tt.bits() < ft.bits() {
                    self.b.ins().ireduce(tt, x)
                } else {
                    x
                }
            }
            CastKind::ZExt => {
                if tt.bits() > ft.bits() {
                    self.b.ins().uextend(tt, x)
                } else if tt.bits() < ft.bits() {
                    self.b.ins().ireduce(tt, x)
                } else {
                    x
                }
            }
            CastKind::Trunc => {
                if tt.bits() < ft.bits() {
                    self.b.ins().ireduce(tt, x)
                } else if tt.bits() > ft.bits() {
                    self.b.ins().uextend(tt, x)
                } else {
                    x
                }
            }
            CastKind::SToF => {
                // Cranelift converts from i32/i64 only.
                let w = if ft.bits() < 32 {
                    self.b.ins().sextend(clty::I32, x)
                } else {
                    x
                };
                self.b.ins().fcvt_from_sint(tt, w)
            }
            CastKind::UToF => {
                let w = if ft.bits() < 32 {
                    self.b.ins().uextend(clty::I32, x)
                } else {
                    x
                };
                self.b.ins().fcvt_from_uint(tt, w)
            }
            // Through the runtime, so the saturation bounds and the NaN rule are
            // literally the same code the C tier runs.
            CastKind::FToS | CastKind::FToU => {
                let d = if from == IrTy::F32 {
                    self.b.ins().fpromote(clty::F64, x)
                } else {
                    x
                };
                let stem = if matches!(kind, CastKind::FToS) { "ax_f2i" } else { "ax_f2u" };
                let name = format!("{stem}_{}", to.name());
                self.call_ext_raw(&name, &[d], Some(to))?
                    .ok_or_else(|| format!("{name} returned nothing"))?
            }
            CastKind::FCast => {
                if to == IrTy::F64 && from == IrTy::F32 {
                    self.b.ins().fpromote(clty::F64, x)
                } else if to == IrTy::F32 && from == IrTy::F64 {
                    self.b.ins().fdemote(clty::F32, x)
                } else {
                    x
                }
            }
            CastKind::Bitcast => {
                if ft == tt {
                    x
                } else if ft.is_int() && tt.is_int() {
                    self.fit_int(x, tt)
                } else {
                    self.b.ins().bitcast(tt, MemFlagsData::new(), x)
                }
            }
        })
    }

    fn bin(
        &mut self,
        op: BinKind,
        l: ValId,
        r: ValId,
    ) -> Result<cranelift_codegen::ir::Value, String> {
        let t = self.f.ty_of(l);
        let (a, b) = (self.get(l), self.get(r));
        let signed = t.is_signed();
        Ok(match op {
            BinKind::Add => self.b.ins().iadd(a, b),
            BinKind::Sub => self.b.ins().isub(a, b),
            BinKind::Mul => self.b.ins().imul(a, b),
            BinKind::DivTrunc | BinKind::DivTruncNZ => self.div_or_rem(t, a, b, true),
            BinKind::RemTrunc | BinKind::RemTruncNZ => self.div_or_rem(t, a, b, false),
            BinKind::FAdd => self.b.ins().fadd(a, b),
            BinKind::FSub => self.b.ins().fsub(a, b),
            BinKind::FMul => self.b.ins().fmul(a, b),
            BinKind::FDiv => self.b.ins().fdiv(a, b),
            BinKind::FRem => {
                let name = if t == IrTy::F32 { "ax_fmodf" } else { "ax_fmod" };
                self.call_ext_raw(name, &[a, b], Some(t))?
                    .ok_or("ax_fmod returned nothing")?
            }
            BinKind::And => self.b.ins().band(a, b),
            BinKind::Or => self.b.ins().bor(a, b),
            BinKind::Xor => self.b.ins().bxor(a, b),
            // Shift counts mask to the operand width, as in `AX_SHIFT`.
            BinKind::Shl => {
                let c = self.shift_count(t, b);
                self.b.ins().ishl(a, c)
            }
            BinKind::Shr => {
                let c = self.shift_count(t, b);
                if signed {
                    self.b.ins().sshr(a, c)
                } else {
                    self.b.ins().ushr(a, c)
                }
            }
            BinKind::Eq | BinKind::Ne | BinKind::Lt | BinKind::Le | BinKind::Gt | BinKind::Ge => {
                if t.is_float() {
                    let cc = match op {
                        BinKind::Eq => FloatCC::Equal,
                        BinKind::Ne => FloatCC::NotEqual,
                        BinKind::Lt => FloatCC::LessThan,
                        BinKind::Le => FloatCC::LessThanOrEqual,
                        BinKind::Gt => FloatCC::GreaterThan,
                        _ => FloatCC::GreaterThanOrEqual,
                    };
                    self.b.ins().fcmp(cc, a, b)
                } else {
                    let cc = match (op, signed) {
                        (BinKind::Eq, _) => IntCC::Equal,
                        (BinKind::Ne, _) => IntCC::NotEqual,
                        (BinKind::Lt, true) => IntCC::SignedLessThan,
                        (BinKind::Lt, false) => IntCC::UnsignedLessThan,
                        (BinKind::Le, true) => IntCC::SignedLessThanOrEqual,
                        (BinKind::Le, false) => IntCC::UnsignedLessThanOrEqual,
                        (BinKind::Gt, true) => IntCC::SignedGreaterThan,
                        (BinKind::Gt, false) => IntCC::UnsignedGreaterThan,
                        (_, true) => IntCC::SignedGreaterThanOrEqual,
                        (_, false) => IntCC::UnsignedGreaterThanOrEqual,
                    };
                    self.b.ins().icmp(cc, a, b)
                }
            }
        })
    }

    fn shift_count(
        &mut self,
        t: IrTy,
        b: cranelift_codegen::ir::Value,
    ) -> cranelift_codegen::ir::Value {
        // Deliberately redundant: Cranelift also masks the shift amount to the
        // operand width, but an exact language rule should not rest on a backend
        // detail a future version may relax.
        let bits = clif_ty(t, self.ptr_ty).bits() as i64;
        self.b.ins().band_imm_s(b, bits - 1)
    }

    /// Truncating division and remainder with Ax's exact semantics.
    ///
    /// Reproduces `AX_DIV_*` from the runtime header, which cannot be called
    /// because it is `static inline`: divide-by-zero yields 0 rather than
    /// trapping, and `INT_MIN / -1` wraps. The guards are `select`s over an
    /// already-safe divisor, so no path can reach a trapping `sdiv`.
    fn div_or_rem(
        &mut self,
        t: IrTy,
        a: cranelift_codegen::ir::Value,
        b: cranelift_codegen::ir::Value,
        div: bool,
    ) -> cranelift_codegen::ir::Value {
        let ct = clif_ty(t, self.ptr_ty);
        // i8/i16 division is not uniformly available; widen, divide, narrow.
        let (wide, wt) = if ct.bits() < 32 {
            let w = if t.is_signed() {
                (self.b.ins().sextend(clty::I32, a), self.b.ins().sextend(clty::I32, b))
            } else {
                (self.b.ins().uextend(clty::I32, a), self.b.ins().uextend(clty::I32, b))
            };
            (w, clty::I32)
        } else {
            ((a, b), ct)
        };
        let (wa, wb) = wide;
        let is_zero = self.b.ins().icmp_imm_s(IntCC::Equal, wb, 0);
        let one = self.b.ins().iconst(wt, 1);
        let safe = self.b.ins().select(is_zero, one, wb);
        let out = if t.is_signed() {
            // The other trapping case: the quotient of the minimum value by -1
            // does not fit. Ax says it wraps, which is negation.
            let is_neg1 = self.b.ins().icmp_imm_s(IntCC::Equal, wb, -1);
            let safe = self.b.ins().select(is_neg1, one, safe);
            let q = if div {
                self.b.ins().sdiv(wa, safe)
            } else {
                self.b.ins().srem(wa, safe)
            };
            let special = if div {
                self.b.ins().ineg(wa)
            } else {
                self.b.ins().iconst(wt, 0)
            };
            let q = self.b.ins().select(is_neg1, special, q);
            let zero = self.b.ins().iconst(wt, 0);
            self.b.ins().select(is_zero, zero, q)
        } else {
            let q = if div {
                self.b.ins().udiv(wa, safe)
            } else {
                self.b.ins().urem(wa, safe)
            };
            let zero = self.b.ins().iconst(wt, 0);
            self.b.ins().select(is_zero, zero, q)
        };
        if wt != ct {
            self.b.ins().ireduce(ct, out)
        } else {
            out
        }
    }

    fn edge_args(&mut self, e: &Edge) -> Vec<BlockArg> {
        e.args
            .iter()
            .map(|a| BlockArg::Value(self.get(*a)))
            .collect()
    }

    fn term(&mut self, t: &Term) -> Result<(), String> {
        match t {
            Term::Jump(e) => {
                let args = self.edge_args(e);
                let to = self.blocks[e.to as usize];
                self.b.ins().jump(to, &args);
            }
            Term::Br {
                cond,
                then_e,
                else_e,
            } => {
                let c = self.get(*cond);
                let ta = self.edge_args(then_e);
                let ea = self.edge_args(else_e);
                let tb = self.blocks[then_e.to as usize];
                let eb = self.blocks[else_e.to as usize];
                self.b.ins().brif(c, tb, &ta, eb, &ea);
            }
            Term::Switch { on, cases, default } => {
                // A comparison chain rather than a jump table: the cases are
                // sparse tags, and each edge may carry block arguments, which a
                // Cranelift jump table cannot.
                let v = self.get(*on);
                for (k, e) in cases {
                    let hit = self.b.ins().icmp_imm_s(IntCC::Equal, v, *k);
                    let args = self.edge_args(e);
                    let to = self.blocks[e.to as usize];
                    let next = self.b.create_block();
                    self.b.ins().brif(hit, to, &args, next, &[]);
                    self.b.switch_to_block(next);
                }
                let args = self.edge_args(default);
                let to = self.blocks[default.to as usize];
                self.b.ins().jump(to, &args);
            }
            Term::Ret(v) => {
                let mut out = Vec::new();
                let has_payload = self.f.ret_agg.is_none() && self.f.ret != IrTy::Unit;
                if has_payload {
                    let want = clif_ty(self.f.ret, self.ptr_ty);
                    match v {
                        Some(x) => {
                            let val = self.get(*x);
                            out.push(val);
                        }
                        None => {
                            let z = if want.is_float() {
                                if want == clty::F32 {
                                    self.b.ins().f32const(0.0)
                                } else {
                                    self.b.ins().f64const(0.0)
                                }
                            } else {
                                self.b.ins().iconst(want, 0)
                            };
                            out.push(z);
                        }
                    }
                }
                if self.f.is_fallible() {
                    let ok = self.b.ins().iconst(clty::I32, 0);
                    out.push(ok);
                }
                self.b.ins().return_(&out);
            }
            Term::RetErr(tag) => {
                if !self.f.is_fallible() {
                    return Err(format!("@{}: ret.err in an infallible function", self.f.name));
                }
                let mut out = Vec::new();
                let has_payload = self.f.ret_agg.is_none() && self.f.ret != IrTy::Unit;
                if has_payload {
                    let want = clif_ty(self.f.ret, self.ptr_ty);
                    let z = if want.is_float() {
                        if want == clty::F32 {
                            self.b.ins().f32const(0.0)
                        } else {
                            self.b.ins().f64const(0.0)
                        }
                    } else {
                        self.b.ins().iconst(want, 0)
                    };
                    out.push(z);
                }
                let v = self.get(*tag);
                let v = self.fit_int(v, clty::I32);
                out.push(v);
                self.b.ins().return_(&out);
            }
            Term::Abort(code) => {
                let msg = CString::new(code.message()).map_err(|e| e.to_string())?;
                let addr = msg.into_raw() as i64;
                let p = self.b.ins().iconst(self.ptr_ty, addr);
                self.call_ext_raw("ax_abort", &[p], None)?;
                // `ax_abort` does not return; a terminator is still required.
                self.b
                    .ins()
                    .trap(TrapCode::user(TRAP_UNREACHABLE).expect("nonzero"));
            }
            Term::Unreachable => {
                self.b
                    .ins()
                    .trap(TrapCode::user(TRAP_UNREACHABLE).expect("nonzero"));
            }
        }
        Ok(())
    }
}

/// Entry shims: one for `main`, or one per test.
fn build_entries(
    prog: &Program,
    module: &mut JITModule,
    ctx: &mut Context,
    fbctx: &mut FunctionBuilderContext,
    ids: &[ClFuncId],
    ptr_ty: ClType,
) -> Result<(ClFuncId, Vec<(String, ClFuncId)>), String> {
    let fc = module.target_config();
    let mut tests = Vec::new();
    let main_id = match prog.main {
        Some(m) => {
            let f = prog.func(m);
            let mut sig = module.make_signature();
            for _ in 0..3 {
                sig.params.push(AbiParam::new(ptr_ty));
            }
            let id = module
                .declare_function("ax_jit_entry", Linkage::Export, &sig)
                .map_err(|e| format!("declare entry: {e}"))?;
            ctx.func.signature = sig;
            ctx.func.name = cranelift_codegen::ir::UserFuncName::user(1, 0);
            {
                let mut b = FunctionBuilder::new(&mut ctx.func, fbctx);
                let blk = b.create_block();
                b.append_block_params_for_function_params(blk);
                b.switch_to_block(blk);
                let ps: Vec<_> = b.block_params(blk).to_vec();
                let (out_val, out_err, out_tag) = (ps[0], ps[1], ps[2]);

                let fr = module.declare_func_in_func(ids[m as usize], b.func);
                let mut argv = Vec::new();
                if f.ret_agg.is_some() {
                    argv.push(out_val);
                }
                if f.is_fallible() {
                    // The C tier passes a buffer only when the payload is an
                    // aggregate, and 0 otherwise. Passing the buffer always is
                    // harmless and keeps the shim uniform.
                    argv.push(out_err);
                }
                let call = b.ins().call(fr, &argv);
                let results: Vec<_> = b.inst_results(call).to_vec();
                let mut k = 0usize;
                if f.ret_agg.is_none() && f.ret != IrTy::Unit {
                    b.ins()
                        .store(MemFlagsData::new(), results[k], out_val, 0);
                    k += 1;
                }
                if f.is_fallible() {
                    b.ins().store(MemFlagsData::new(), results[k], out_tag, 0);
                }
                b.ins().return_(&[]);
                b.seal_all_blocks();
                b.finalize(fc);
            }
            module
                .define_function(id, ctx)
                .map_err(|e| format!("cranelift rejected the entry shim: {e}"))?;
            module.clear_context(ctx);
            id
        }
        None => {
            // No `main`: shims that run one test each and report a bool.
            for (idx, (name, fid)) in prog.tests.iter().enumerate() {
                let mut sig = module.make_signature();
                sig.returns.push(AbiParam::new(clty::I32));
                let sym = format!("ax_jit_test_{idx}");
                let id = module
                    .declare_function(&sym, Linkage::Export, &sig)
                    .map_err(|e| format!("declare {sym}: {e}"))?;
                ctx.func.signature = sig;
                ctx.func.name = cranelift_codegen::ir::UserFuncName::user(2, idx as u32);
                {
                    let mut b = FunctionBuilder::new(&mut ctx.func, fbctx);
                    let blk = b.create_block();
                    b.switch_to_block(blk);
                    let tf = prog.func(*fid);
                    let fr = module.declare_func_in_func(ids[*fid as usize], b.func);
                    let call = b.ins().call(fr, &[]);
                    let results: Vec<_> = b.inst_results(call).to_vec();
                    let v = match results.first() {
                        Some(v) if tf.ret != IrTy::Unit && tf.ret_agg.is_none() => {
                            let t = b.func.dfg.value_type(*v);
                            if t == clty::I32 {
                                *v
                            } else if t.bits() < 32 {
                                b.ins().uextend(clty::I32, *v)
                            } else {
                                b.ins().ireduce(clty::I32, *v)
                            }
                        }
                        // A test returning unit passes by not aborting.
                        _ => b.ins().iconst(clty::I32, 1),
                    };
                    b.ins().return_(&[v]);
                    b.seal_all_blocks();
                    b.finalize(fc);
                }
                module
                    .define_function(id, ctx)
                    .map_err(|e| format!("cranelift rejected {sym}: {e}"))?;
                module.clear_context(ctx);
                tests.push((name.clone(), id));
            }
            // A placeholder so the struct has an entry: never called when
            // `prog.main` is None.
            let mut sig = module.make_signature();
            for _ in 0..3 {
                sig.params.push(AbiParam::new(ptr_ty));
            }
            let id = module
                .declare_function("ax_jit_entry", Linkage::Export, &sig)
                .map_err(|e| format!("declare entry: {e}"))?;
            ctx.func.signature = sig;
            ctx.func.name = cranelift_codegen::ir::UserFuncName::user(1, 0);
            {
                let mut b = FunctionBuilder::new(&mut ctx.func, fbctx);
                let blk = b.create_block();
                b.append_block_params_for_function_params(blk);
                b.switch_to_block(blk);
                b.ins().return_(&[]);
                b.seal_all_blocks();
                b.finalize(fc);
            }
            module
                .define_function(id, ctx)
                .map_err(|e| format!("cranelift rejected the empty entry: {e}"))?;
            module.clear_context(ctx);
            id
        }
    };
    Ok((main_id, tests))
}

fn runtime_slot_sizes() -> Result<(u32, u32, u32, u32), String> {
    // Cached: the runtime is one library per process, so these never change.
    use std::sync::OnceLock;
    static SIZES: OnceLock<Result<(u32, u32, u32, u32), String>> = OnceLock::new();
    SIZES
        .get_or_init(|| {
            let rt = Runtime::open()?;
            let g = |n: &str| -> Result<u64, String> {
                let p = rt.require(n)?;
                let f: extern "C" fn() -> u64 = unsafe { std::mem::transmute(p) };
                Ok(f())
            };
            let g32 = |n: &str| -> Result<u32, String> {
                let p = rt.require(n)?;
                let f: extern "C" fn() -> u32 = unsafe { std::mem::transmute(p) };
                Ok(f())
            };
            Ok((
                g("ax_arena_slot_size")? as u32,
                g32("ax_arena_slot_align")?,
                g("ax_alloc_slot_size")? as u32,
                g32("ax_alloc_slot_align")?,
            ))
        })
        .clone()
}

fn repr_size_align(p: &Program, r: Repr) -> (u32, u32) {
    match r {
        Repr::Scalar(t) => (t.size().max(1), t.align()),
        Repr::Agg(a) => {
            let d = p.agg(a);
            (d.size.max(1), d.align.max(1))
        }
    }
}

fn align_shift(align: u32) -> u8 {
    let mut s = 0u8;
    let mut a = align.max(1);
    while a > 1 {
        a >>= 1;
        s += 1;
    }
    s
}

fn mask(bits: u32) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

/// Render `main`'s result exactly as the oracle prints it.
///
/// This deliberately reuses `interp::Value::display` rather than re-deriving the
/// formatting: a difference between the tiers should mean a difference in the
/// computed value, not in how two printers spell a float.
fn render_return(p: &Program, f: &Func, val: &[u8]) -> String {
    match f.ret_agg {
        Some(a) => {
            let ptr = val.as_ptr();
            render_agg(p, a, ptr)
        }
        None => {
            if f.ret == IrTy::Unit {
                return String::new();
            }
            let mut bits = [0u8; 8];
            let n = f.ret.size().min(8) as usize;
            bits[..n].copy_from_slice(&val[..n]);
            render_scalar(f.ret, &f.ret_src, u64::from_le_bytes(bits))
        }
    }
}

fn render_scalar(ty: IrTy, src: &str, raw: u64) -> String {
    let v = match ty {
        IrTy::Bool => Value::Bool(raw & 1 != 0),
        IrTy::Unit => Value::Unit,
        IrTy::F32 => Value::Float {
            bits: (raw as u32) as u64,
            prim: Prim::F32,
        },
        IrTy::F64 => Value::Float {
            bits: raw,
            prim: Prim::F64,
        },
        IrTy::Ptr => return format!("{raw:#x}"),
        t => {
            let prim = prim_of(src).unwrap_or(if t.is_signed() { Prim::I64 } else { Prim::U64 });
            let bits = if t.is_signed() {
                sign_extend(raw, t.bits())
            } else {
                (raw & mask(t.bits())) as i128
            };
            Value::Int { bits, prim }
        }
    };
    v.display()
}

fn sign_extend(raw: u64, bits: u32) -> i128 {
    let m = mask(bits);
    let x = raw & m;
    if bits < 64 && x & (1u64 << (bits - 1)) != 0 {
        (x as i128) - ((m as i128) + 1)
    } else {
        x as i64 as i128
    }
}

/// Source-level primitive from its spelling, so `usz` does not print as `u64`.
fn prim_of(src: &str) -> Option<Prim> {
    Some(match src {
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
        "byte" => Prim::Byte,
        "f32" => Prim::F32,
        "f64" => Prim::F64,
        "bool" => Prim::Bool,
        _ => return None,
    })
}

/// Read an aggregate out of memory using the IR's own offsets and print it in
/// the oracle's form. Mirrors `backend_c::renderers`, which does the same job
/// with C member access.
fn render_agg(p: &Program, id: TypeId, base: *const u8) -> String {
    let def = p.agg(id);
    match &def.kind {
        AggKind::Record if def.name == "str" => {
            let data = unsafe { std::ptr::read_unaligned(base as *const *const u8) };
            let len = unsafe {
                std::ptr::read_unaligned(base.add(def.field(1).offset as usize) as *const u64)
            };
            if data.is_null() || len == 0 {
                return "\"\"".to_string();
            }
            let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) };
            format!("\"{}\"", String::from_utf8_lossy(bytes))
        }
        AggKind::Record => {
            let inner: Vec<String> = def
                .fields
                .iter()
                .map(|fd| format!("{}: {}", fd.name, render_field(p, fd, base)))
                .collect();
            format!("{{ {} }}", inner.join(", "))
        }
        AggKind::Variant { cases } => {
            let tag = unsafe {
                std::ptr::read_unaligned(
                    base.add(def.field(VARIANT_TAG_FIELD).offset as usize) as *const i32,
                )
            } as i64;
            let Some(c) = cases.iter().find(|c| c.tag == tag) else {
                return "<bad tag>".to_string();
            };
            if c.fields.is_empty() {
                return c.name.clone();
            }
            let inner: Vec<String> = c
                .fields
                .iter()
                .map(|fi| {
                    let fd = def.field(*fi);
                    let bare = fd
                        .name
                        .strip_prefix(&format!("{}_", c.name))
                        .unwrap_or(&fd.name);
                    format!("{bare}: {}", render_field(p, fd, base))
                })
                .collect();
            format!("{} {{ {} }}", c.name, inner.join(", "))
        }
    }
}

fn render_field(p: &Program, fd: &FieldDef, base: *const u8) -> String {
    let at = unsafe { base.add(fd.offset as usize) };
    match fd.agg {
        Some(inner) => render_agg(p, inner, at),
        None => {
            let n = fd.ty.size().min(8) as usize;
            let mut bits = [0u8; 8];
            if n > 0 {
                let src = unsafe { std::slice::from_raw_parts(at, n) };
                bits[..n].copy_from_slice(src);
            }
            render_scalar(fd.ty, &fd.src, u64::from_le_bytes(bits))
        }
    }
}

/// Compile and run a source file through the JIT, returning what it printed.
pub fn run_source(
    intern: &Interner,
    checked: &crate::check::CheckOutput,
    argv: &[String],
) -> Result<String, String> {
    let jit = compile(intern, checked)?;
    jit.run(argv)
}

/// The path a caller can print for diagnostics.
pub fn runtime_library() -> Result<PathBuf, String> {
    build_runtime_dylib()
}

/// Whether the tier can run at all here (needs `cc` for the runtime library).
pub fn available() -> bool {
    build_runtime_dylib().is_ok()
}

/// For tests: the JIT's view of a file, without going through the CLI.
pub fn eval_file(path: &Path) -> Result<String, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut s = crate::driver::Session::new();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("t.ax");
    let out = s
        .compile(name, &src)
        .map_err(|d| format!("{} did not compile: {d:?}", path.display()))?;
    run_source(&s.intern, &out, &[name.to_string()])
}
