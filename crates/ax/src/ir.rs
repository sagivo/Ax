//! Typed SSA IR — the single lowering target.
//!
//! The checked AST is lowered here once (`crate::lower`), and every backend
//! consumes *only* this. Types, effects, and regions are still attached: the
//! IR is where they pay rent (error channels, bump arenas, aliasing), and
//! backends erase them, not the frontend.
//!
//! Shape: basic blocks with block parameters (no phi nodes), one value per
//! instruction, aggregates in memory. That maps 1:1 onto both C and
//! Cranelift without either backend re-inventing control flow.
//!
//! Error ABI: a function whose row contains `err[E]` returns two values —
//! a tag (`0` ok, `1` raised) and a payload slot. Scalar payloads travel by
//! value, aggregates by pointer. This is the same shape as a Rust
//! `Result<T, E>`, so the fallible path costs one register test per call.

use crate::effects::EffectSet;
use std::fmt::Write as _;

pub type FuncId = u32;
pub type BlockId = u32;
pub type ValId = u32;
pub type TypeId = u32;
pub type RegionIdx = u32;

/// Machine-level type. Aggregates are always behind a `Ptr`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IrTy {
    Unit,
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    /// Untyped address: aggregate, slice data, string data, or capability handle.
    Ptr,
}

impl IrTy {
    pub fn is_int(self) -> bool {
        matches!(
            self,
            IrTy::I8
                | IrTy::I16
                | IrTy::I32
                | IrTy::I64
                | IrTy::U8
                | IrTy::U16
                | IrTy::U32
                | IrTy::U64
        )
    }

    pub fn is_signed(self) -> bool {
        matches!(self, IrTy::I8 | IrTy::I16 | IrTy::I32 | IrTy::I64)
    }

    pub fn is_float(self) -> bool {
        matches!(self, IrTy::F32 | IrTy::F64)
    }

    pub fn bits(self) -> u32 {
        match self {
            IrTy::Unit => 0,
            IrTy::Bool | IrTy::I8 | IrTy::U8 => 8,
            IrTy::I16 | IrTy::U16 => 16,
            IrTy::I32 | IrTy::U32 | IrTy::F32 => 32,
            IrTy::I64 | IrTy::U64 | IrTy::F64 | IrTy::Ptr => 64,
        }
    }

    pub fn size(self) -> u32 {
        match self {
            IrTy::Unit => 0,
            _ => self.bits() / 8,
        }
    }

    pub fn align(self) -> u32 {
        self.size().max(1)
    }

    pub fn name(self) -> &'static str {
        match self {
            IrTy::Unit => "unit",
            IrTy::Bool => "bool",
            IrTy::I8 => "i8",
            IrTy::I16 => "i16",
            IrTy::I32 => "i32",
            IrTy::I64 => "i64",
            IrTy::U8 => "u8",
            IrTy::U16 => "u16",
            IrTy::U32 => "u32",
            IrTy::U64 => "u64",
            IrTy::F32 => "f32",
            IrTy::F64 => "f64",
            IrTy::Ptr => "ptr",
        }
    }
}

/// An in-memory aggregate: record, or tagged union.
#[derive(Clone, Debug)]
pub struct AggDef {
    pub name: String,
    pub kind: AggKind,
    /// Flat field list. For variants this is the tag followed by the union of
    /// all case payloads; `cases` indexes into it.
    pub fields: Vec<FieldDef>,
    pub size: u32,
    pub align: u32,
}

#[derive(Clone, Debug)]
pub enum AggKind {
    Record,
    Variant { cases: Vec<VariantCase> },
}

#[derive(Clone, Debug)]
pub struct FieldDef {
    pub name: String,
    pub ty: IrTy,
    /// Aggregate-typed field (inline nested struct), if any.
    pub agg: Option<TypeId>,
    pub offset: u32,
    /// Source-level type as written (`i32`, `usz`, `String`, ...).
    ///
    /// Carried purely so a backend can render a value in the oracle's canonical
    /// form (`{ a: 11i32 }`). Differential testing against the interpreter is
    /// only as strong as the two sides agreeing on how to print a result, and
    /// `IrTy` alone cannot tell `i64` from `isz`.
    pub src: String,
}

#[derive(Clone, Debug)]
pub struct VariantCase {
    pub name: String,
    pub tag: i64,
    /// Indices into `AggDef::fields` carrying this case's payload.
    pub fields: Vec<u32>,
}

impl AggDef {
    pub fn field(&self, i: u32) -> &FieldDef {
        &self.fields[i as usize]
    }

    pub fn field_index(&self, name: &str) -> Option<u32> {
        self.fields
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u32)
    }

    pub fn case(&self, name: &str) -> Option<&VariantCase> {
        match &self.kind {
            AggKind::Variant { cases } => cases.iter().find(|c| c.name == name),
            AggKind::Record => None,
        }
    }
}

/// Tag field of every variant aggregate. Always first, always `i32`.
pub const VARIANT_TAG_FIELD: u32 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinKind {
    /// Wrapping integer add/sub/mul (§3.3: `+ - *` wrap, never trap).
    Add,
    Sub,
    Mul,
    /// Truncating integer division / remainder. The zero check is *not*
    /// implicit: lowering emits an explicit test and a `raise` edge, so the
    /// backend sees ordinary control flow.
    DivTrunc,
    RemTrunc,
    /// Division / remainder on a path where the divisor is already proven
    /// non-zero, because lowering emitted the test that got us here.
    ///
    /// Without this the generated code tests the divisor twice: once in the IR's
    /// explicit branch and again inside the runtime helper. For unsigned
    /// operands the second test is the *only* thing standing between us and a
    /// bare machine divide, and it sat in the middle of every division-heavy
    /// loop.
    DivTruncNZ,
    RemTruncNZ,
    FAdd,
    FSub,
    FMul,
    FDiv,
    FRem,
    And,
    Or,
    Xor,
    Shl,
    /// Arithmetic (signed) / logical (unsigned) right shift.
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinKind {
    pub fn is_cmp(self) -> bool {
        matches!(
            self,
            BinKind::Eq | BinKind::Ne | BinKind::Lt | BinKind::Le | BinKind::Gt | BinKind::Ge
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnKind {
    /// Two's-complement negation (wraps).
    Neg,
    FNeg,
    /// Boolean not.
    Not,
    /// Bitwise complement.
    BitNot,
    /// Force a float to the canonical NaN bit pattern when it is NaN.
    CanonNaN,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastKind {
    /// Integer width change, sign-extending.
    SExt,
    /// Integer width change, zero-extending.
    ZExt,
    /// Integer width truncation.
    Trunc,
    /// Signed int -> float.
    SToF,
    /// Unsigned int -> float.
    UToF,
    /// Float -> signed int (truncating toward zero, saturating).
    FToS,
    /// Float -> unsigned int (truncating toward zero, saturating).
    FToU,
    /// f32 <-> f64.
    FCast,
    /// Reinterpret without changing bits (bool <-> i8, ptr <-> u64).
    Bitcast,
}

/// Why a program stopped. Aborts are observable and must match the oracle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortCode {
    IndexOutOfBounds,
    NonExhaustiveMatch,
    ContractPre,
    ContractPost,
    DivExactZero,
    /// A `raise` reached a frame with no handler and no declared `err[E]`.
    UncaughtRaise,
    /// `assert(cond)` with a false condition.
    Assert,
    Explicit,
}

impl AbortCode {
    pub fn message(self) -> &'static str {
        match self {
            AbortCode::IndexOutOfBounds => "index out of bounds",
            AbortCode::NonExhaustiveMatch => "non-exhaustive match",
            AbortCode::ContractPre => "precondition violated",
            AbortCode::ContractPost => "postcondition violated",
            AbortCode::DivExactZero => "div_exact by zero",
            AbortCode::UncaughtRaise => "uncaught raise",
            // Same text as the oracle, so a differential test can compare stderr.
            AbortCode::Assert => "assertion failed",
            AbortCode::Explicit => "abort",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AbortCode::IndexOutOfBounds => "oob",
            AbortCode::NonExhaustiveMatch => "match",
            AbortCode::ContractPre => "pre",
            AbortCode::ContractPost => "post",
            AbortCode::DivExactZero => "div_exact",
            AbortCode::UncaughtRaise => "uncaught",
            AbortCode::Assert => "assert",
            AbortCode::Explicit => "explicit",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Op {
    ConstInt(i128),
    ConstFloat(f64),
    ConstBool(bool),
    ConstUnit,
    /// Pointer to a static NUL-terminated string blob; `len` is separate.
    ConstStr(u32),
    Bin {
        op: BinKind,
        l: ValId,
        r: ValId,
    },
    Un {
        op: UnKind,
        v: ValId,
    },
    Cast {
        kind: CastKind,
        v: ValId,
    },
    /// Direct call. If the callee is fallible the call defines two values:
    /// this one (the payload) and `err_tag`, retrievable via `Op::CallTag`.
    Call {
        f: FuncId,
        args: Vec<ValId>,
    },
    /// Call into the runtime (`axrt`) or a `trusted extern`.
    CallExt {
        name: String,
        args: Vec<ValId>,
        ret: IrTy,
        /// Runtime entry points that can fail set the error channel too.
        fallible: bool,
    },
    CallIndirect {
        ptr: ValId,
        args: Vec<ValId>,
        ret: IrTy,
    },
    FuncAddr(FuncId),
    /// Address of a function-scoped stack slot (see `Func::slots`).
    ///
    /// Every local variable gets a slot and is accessed by load/store rather
    /// than being threaded through block parameters. That keeps lowering a
    /// straight walk of the AST; `clang -O2` promotes the slots back to
    /// registers via SROA, so it costs nothing in the release tier.
    SlotAddr(u32),
    /// Bump-allocate `size` bytes in `region`. Regions are why this is not
    /// `malloc`: the whole arena is released at region exit.
    RegionAlloc {
        region: RegionIdx,
        size: ValId,
        align: u32,
    },
    Load {
        ty: IrTy,
        ptr: ValId,
    },
    /// No result value.
    Store {
        ty: IrTy,
        ptr: ValId,
        val: ValId,
    },
    FieldPtr {
        agg: TypeId,
        field: u32,
        ptr: ValId,
    },
    ElemPtr {
        elem: Repr,
        ptr: ValId,
        idx: ValId,
    },
    /// Byte-wise aggregate copy. No result value.
    CopyAgg {
        ty: TypeId,
        dst: ValId,
        src: ValId,
    },
    Select {
        c: ValId,
        a: ValId,
        b: ValId,
    },
    /// Enter / leave a lexical region's arena. No result value.
    RegionEnter(RegionIdx),
    RegionExit(RegionIdx),
    /// Allocator handle for a region's arena, so code inside can allocate from
    /// it (`vec.new(r)`). This is the operation that turns a region annotation
    /// into a different allocation strategy.
    RegionAllocHandle(RegionIdx),
    /// Size in bytes of a scalar or aggregate, as the *backend* computes it.
    ///
    /// Lowering must never bake a size in as a literal: the C backend lets the C
    /// compiler choose padding, so only it can say how big a struct is. Same
    /// reason descriptors use `offsetof`.
    SizeOf(Repr),
    /// Address of a static description of `ty`'s layout (field names, offsets,
    /// kinds). Lets data-driven runtime code — JSON decoding — fill a record
    /// without a parser generated per type. Each backend emits its own offsets,
    /// so no layout knowledge crosses the boundary.
    TypeDescriptor(TypeId),
    /// Unique-heap allocation: `malloc` with no RC word. Freed at last use.
    UniqueAlloc {
        size: ValId,
        align: u32,
    },
    /// Free a unique-heap object. No result.
    UniqueFree(ValId),
    /// Residual RC: allocate with a leading refcount word. Returns payload ptr.
    RcAlloc {
        size: ValId,
        align: u32,
        atomic: bool,
    },
    RcRetain(ValId),
    RcRelease(ValId),
}

#[derive(Clone, Debug)]
pub struct Inst {
    /// Values this instruction defines. Empty for stores and region markers,
    /// one for most operations, two for a call to a fallible function
    /// (payload, then error tag).
    pub results: Vec<ValId>,
    pub op: Op,
}

impl Inst {
    pub fn result(&self) -> Option<ValId> {
        self.results.first().copied()
    }
}

/// Where control goes, with the arguments passed to the target's block params.
#[derive(Clone, Debug)]
pub struct Edge {
    pub to: BlockId,
    pub args: Vec<ValId>,
}

#[derive(Clone, Debug)]
pub enum Term {
    Jump(Edge),
    Br {
        cond: ValId,
        then_e: Edge,
        else_e: Edge,
    },
    Switch {
        on: ValId,
        cases: Vec<(i64, Edge)>,
        default: Edge,
    },
    /// Normal return. `None` for `unit`.
    Ret(Option<ValId>),
    /// Return along the error channel with this payload.
    RetErr(ValId),
    Abort(AbortCode),
    Unreachable,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub id: BlockId,
    pub params: Vec<ValId>,
    pub insts: Vec<Inst>,
    pub term: Term,
}

#[derive(Clone, Debug)]
pub struct RegionInfo {
    pub name: String,
    /// Lexical nesting depth; 0 is the function's own frame.
    pub depth: u32,
}

/// Storage in the function frame: a local variable, a temporary, or the
/// backing memory for an aggregate value.
#[derive(Clone, Debug)]
pub struct SlotInfo {
    pub kind: SlotKind,
    /// Source name where one exists; `""` for compiler temporaries.
    pub name: String,
}

/// How a piece of storage is shaped: a scalar, or an aggregate laid out inline.
///
/// Array elements need this as much as slots do — a `Vec[Point]` stores points
/// inline, so its stride is `sizeof(Point)`, not the size of a pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repr {
    Scalar(IrTy),
    Agg(TypeId),
}

/// Storage shape of a frame slot. Alias of [`Repr`].
pub type SlotKind = Repr;

#[derive(Clone, Debug)]
pub struct Func {
    pub id: FuncId,
    /// Link name.
    pub name: String,
    /// Source-level def id, for diagnostics and hashing.
    pub def_id: String,
    pub params: Vec<ValId>,
    pub ret: IrTy,
    /// Aggregate returns are written through a caller-provided pointer, which
    /// is appended to `params` as a hidden trailing argument.
    pub ret_agg: Option<TypeId>,
    /// Payload type of `err[E]`, if the row declares one.
    pub err: Option<ErrChannel>,
    /// Source-level return type, for canonical rendering (see `FieldDef::src`).
    pub ret_src: String,
    pub effects: EffectSet,
    pub blocks: Vec<Block>,
    pub val_tys: Vec<IrTy>,
    pub regions: Vec<RegionInfo>,
    pub slots: Vec<SlotInfo>,
    pub entry: BlockId,
    /// No `io`/`race`/`nondet`: safe to constant-fold or reorder.
    pub pure: bool,
    /// Row has no `diverge`: every loop in the body is provably bounded.
    pub bounded: bool,
    /// Cache results across calls.
    ///
    /// Set only for a function the effect row proves pure, with one or two
    /// integer parameters and more than one recursive call to itself — the
    /// shape that recomputes the same subproblem exponentially often. Two
    /// arguments cover binomial coefficients; three or more is left alone
    /// because the key would stop being a pair of registers. Memoising a pure
    /// function cannot change any value it returns, so this is invisible except
    /// in time and memory. A language without purity in its types cannot do it:
    /// the compiler would have no way to know the function observes nothing.
    pub memoize: bool,
    pub exported: bool,
}

/// How `err[E]` is carried out of a fallible function.
#[derive(Clone, Debug)]
pub struct ErrChannel {
    pub ty: IrTy,
    pub agg: Option<TypeId>,
    pub display: String,
}

impl Func {
    pub fn block(&self, id: BlockId) -> &Block {
        &self.blocks[id as usize]
    }

    pub fn block_mut(&mut self, id: BlockId) -> &mut Block {
        &mut self.blocks[id as usize]
    }

    pub fn ty_of(&self, v: ValId) -> IrTy {
        self.val_tys[v as usize]
    }

    pub fn is_fallible(&self) -> bool {
        self.err.is_some()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Program {
    pub module: String,
    pub funcs: Vec<Func>,
    pub aggs: Vec<AggDef>,
    /// Static string blobs, referenced by `Op::ConstStr`.
    pub strings: Vec<String>,
    /// Test name -> function implementing it.
    pub tests: Vec<(String, FuncId)>,
    /// Entry point, if the module defines `main`.
    pub main: Option<FuncId>,
}

impl Program {
    pub fn agg(&self, id: TypeId) -> &AggDef {
        &self.aggs[id as usize]
    }

    pub fn func(&self, id: FuncId) -> &Func {
        &self.funcs[id as usize]
    }

    pub fn find_func(&self, name: &str) -> Option<&Func> {
        self.funcs.iter().find(|f| f.name == name)
    }

    /// Stable textual form. Used by `ax ir`, golden tests, and for eyeballing
    /// what the backends actually receive.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "; module {}", self.module);
        for (i, a) in self.aggs.iter().enumerate() {
            let kind = match &a.kind {
                AggKind::Record => "record".to_string(),
                AggKind::Variant { cases } => format!("variant({})", cases.len()),
            };
            let _ = writeln!(
                s,
                "type %{i} {} {} size={} align={}",
                a.name, kind, a.size, a.align
            );
            for (fi, f) in a.fields.iter().enumerate() {
                let _ = writeln!(
                    s,
                    "  .{fi} {} : {}{} @{}",
                    f.name,
                    f.ty.name(),
                    f.agg.map(|t| format!(" %{t}")).unwrap_or_default(),
                    f.offset
                );
            }
            if let AggKind::Variant { cases } = &a.kind {
                for c in cases {
                    let fs: Vec<String> = c.fields.iter().map(|i| format!(".{i}")).collect();
                    let _ = writeln!(s, "  case {} tag={} [{}]", c.name, c.tag, fs.join(" "));
                }
            }
        }
        for f in &self.funcs {
            let _ = writeln!(s, "\n{}", self.func_to_text(f));
        }
        for (name, fid) in &self.tests {
            let _ = writeln!(s, "test {:?} -> @{}", name, self.func(*fid).name);
        }
        if let Some(m) = self.main {
            let _ = writeln!(s, "main -> @{}", self.func(m).name);
        }
        s
    }

    fn func_to_text(&self, f: &Func) -> String {
        let mut s = String::new();
        let ps: Vec<String> = f
            .params
            .iter()
            .map(|p| format!("v{p}: {}", f.ty_of(*p).name()))
            .collect();
        let mut attrs = Vec::new();
        if f.pure {
            attrs.push("pure".to_string());
        }
        if f.bounded {
            attrs.push("bounded".to_string());
        }
        if f.memoize {
            attrs.push("memoize".to_string());
        }
        if f.exported {
            attrs.push("export".to_string());
        }
        if let Some(e) = &f.err {
            attrs.push(format!("raises({})", e.display));
        }
        for r in &f.regions {
            attrs.push(format!("region({}@{})", r.name, r.depth));
        }
        let _ = writeln!(
            s,
            "fn @{}({}) -> {}{} {}{{",
            f.name,
            ps.join(", "),
            f.ret.name(),
            f.ret_agg.map(|t| format!(" %{t}")).unwrap_or_default(),
            if attrs.is_empty() {
                String::new()
            } else {
                format!("[{}] ", attrs.join(" "))
            }
        );
        for b in &f.blocks {
            let bps: Vec<String> = b
                .params
                .iter()
                .map(|p| format!("v{p}: {}", f.ty_of(*p).name()))
                .collect();
            if bps.is_empty() {
                let _ = writeln!(s, "  bb{}:", b.id);
            } else {
                let _ = writeln!(s, "  bb{}({}):", b.id, bps.join(", "));
            }
            for i in &b.insts {
                let lhs = if i.results.is_empty() {
                    String::new()
                } else {
                    let vs: Vec<String> = i
                        .results
                        .iter()
                        .map(|v| format!("v{v}: {}", f.ty_of(*v).name()))
                        .collect();
                    format!("{} = ", vs.join(", "))
                };
                let _ = writeln!(s, "    {lhs}{}", op_to_text(&i.op, self));
            }
            let _ = writeln!(s, "    {}", term_to_text(&b.term));
        }
        s.push('}');
        s
    }
}

fn edge_to_text(e: &Edge) -> String {
    if e.args.is_empty() {
        format!("bb{}", e.to)
    } else {
        let a: Vec<String> = e.args.iter().map(|v| format!("v{v}")).collect();
        format!("bb{}({})", e.to, a.join(", "))
    }
}

fn term_to_text(t: &Term) -> String {
    match t {
        Term::Jump(e) => format!("jump {}", edge_to_text(e)),
        Term::Br {
            cond,
            then_e,
            else_e,
        } => format!(
            "br v{cond} ? {} : {}",
            edge_to_text(then_e),
            edge_to_text(else_e)
        ),
        Term::Switch { on, cases, default } => {
            let cs: Vec<String> = cases
                .iter()
                .map(|(k, e)| format!("{k} => {}", edge_to_text(e)))
                .collect();
            format!(
                "switch v{on} [{}] default {}",
                cs.join(", "),
                edge_to_text(default)
            )
        }
        Term::Ret(Some(v)) => format!("ret v{v}"),
        Term::Ret(None) => "ret".to_string(),
        Term::RetErr(v) => format!("ret.err v{v}"),
        Term::Abort(c) => format!("abort {}", c.as_str()),
        Term::Unreachable => "unreachable".to_string(),
    }
}

fn args_to_text(args: &[ValId]) -> String {
    args.iter()
        .map(|v| format!("v{v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn op_to_text(op: &Op, p: &Program) -> String {
    match op {
        Op::ConstInt(v) => format!("const.int {v}"),
        Op::ConstFloat(v) => format!("const.float {v:?}"),
        Op::ConstBool(v) => format!("const.bool {v}"),
        Op::ConstUnit => "const.unit".to_string(),
        Op::ConstStr(i) => format!("const.str {:?}", p.strings[*i as usize]),
        Op::Bin { op, l, r } => format!("{:?} v{l}, v{r}", op).to_lowercase(),
        Op::Un { op, v } => format!("{:?} v{v}", op).to_lowercase(),
        Op::Cast { kind, v } => format!("{:?} v{v}", kind).to_lowercase(),
        Op::Call { f, args } => format!("call @{}({})", p.func(*f).name, args_to_text(args)),
        Op::CallExt { name, args, .. } => format!("call.ext {name}({})", args_to_text(args)),
        Op::CallIndirect { ptr, args, .. } => {
            format!("call.ind v{ptr}({})", args_to_text(args))
        }
        Op::FuncAddr(f) => format!("func.addr @{}", p.func(*f).name),
        Op::SlotAddr(s) => format!("slot.addr s{s}"),
        Op::RegionAlloc {
            region,
            size,
            align,
        } => format!("region.alloc r{region}, v{size}, align {align}"),
        Op::Load { ty, ptr } => format!("load.{} v{ptr}", ty.name()),
        Op::Store { ty, ptr, val } => format!("store.{} v{ptr}, v{val}", ty.name()),
        Op::FieldPtr { agg, field, ptr } => format!("field.ptr %{agg}.{field} v{ptr}"),
        Op::ElemPtr { elem, ptr, idx } => match elem {
            Repr::Scalar(t) => format!("elem.ptr.{} v{ptr}, v{idx}", t.name()),
            Repr::Agg(a) => format!("elem.ptr.%{a} v{ptr}, v{idx}"),
        },
        Op::CopyAgg { ty, dst, src } => format!("copy.agg %{ty} v{dst} <- v{src}"),
        Op::Select { c, a, b } => format!("select v{c} ? v{a} : v{b}"),
        Op::RegionEnter(r) => format!("region.enter r{r}"),
        Op::RegionExit(r) => format!("region.exit r{r}"),
        Op::RegionAllocHandle(r) => format!("region.alloc_handle r{r}"),
        Op::TypeDescriptor(t) => format!("type.descriptor %{t}"),
        Op::SizeOf(r) => match r {
            Repr::Scalar(t) => format!("size_of.{}", t.name()),
            Repr::Agg(a) => format!("size_of %{a}"),
        },
        Op::UniqueAlloc { size, align } => format!("unique.alloc v{size}, align {align}"),
        Op::UniqueFree(p) => format!("unique.free v{p}"),
        Op::RcAlloc {
            size,
            align,
            atomic,
        } => format!(
            "rc.alloc v{size}, align {align}{}",
            if *atomic { ", atomic" } else { "" }
        ),
        Op::RcRetain(p) => format!("rc.retain v{p}"),
        Op::RcRelease(p) => format!("rc.release v{p}"),
    }
}

/// Incremental builder. Tracks the current block so lowering reads as a
/// straight-line walk of the AST.
pub struct FuncBuilder {
    pub func: Func,
    pub cur: BlockId,
    /// Blocks known to be unreachable — a join every branch bypassed because it
    /// diverged. Emitting a terminator into one would fabricate control flow
    /// (and, if the function returns a value, a bogus return type), so writes to
    /// a sealed block are dropped.
    sealed: std::collections::HashSet<BlockId>,
}

impl FuncBuilder {
    pub fn new(id: FuncId, name: String, def_id: String, ret: IrTy) -> Self {
        let entry = Block {
            id: 0,
            params: Vec::new(),
            insts: Vec::new(),
            term: Term::Unreachable,
        };
        Self {
            func: Func {
                id,
                name,
                def_id,
                params: Vec::new(),
                ret,
                ret_agg: None,
                err: None,
                ret_src: String::new(),
                effects: EffectSet::empty(),
                blocks: vec![entry],
                val_tys: Vec::new(),
                regions: Vec::new(),
                slots: Vec::new(),
                entry: 0,
                pure: false,
                bounded: true,
                memoize: false,
                exported: false,
            },
            cur: 0,
            sealed: std::collections::HashSet::new(),
        }
    }

    pub fn new_val(&mut self, ty: IrTy) -> ValId {
        self.func.val_tys.push(ty);
        (self.func.val_tys.len() - 1) as ValId
    }

    pub fn new_block(&mut self) -> BlockId {
        let id = self.func.blocks.len() as BlockId;
        self.func.blocks.push(Block {
            id,
            params: Vec::new(),
            insts: Vec::new(),
            term: Term::Unreachable,
        });
        id
    }

    pub fn new_block_with(&mut self, param_tys: &[IrTy]) -> (BlockId, Vec<ValId>) {
        let id = self.new_block();
        let mut ps = Vec::with_capacity(param_tys.len());
        for t in param_tys {
            let v = self.new_val(*t);
            ps.push(v);
        }
        self.func.blocks[id as usize].params = ps.clone();
        (id, ps)
    }

    pub fn switch_to(&mut self, b: BlockId) {
        self.cur = b;
    }

    /// Push an instruction that defines a value.
    pub fn push(&mut self, op: Op, ty: IrTy) -> ValId {
        let v = self.new_val(ty);
        self.func.blocks[self.cur as usize].insts.push(Inst {
            results: vec![v],
            op,
        });
        v
    }

    /// Push a call to a fallible callee: yields the payload and the error tag.
    pub fn push2(&mut self, op: Op, ty: IrTy, tag_ty: IrTy) -> (ValId, ValId) {
        let v = self.new_val(ty);
        let t = self.new_val(tag_ty);
        self.func.blocks[self.cur as usize].insts.push(Inst {
            results: vec![v, t],
            op,
        });
        (v, t)
    }

    /// Push a side-effecting instruction with no result.
    pub fn push_void(&mut self, op: Op) {
        self.func.blocks[self.cur as usize].insts.push(Inst {
            results: Vec::new(),
            op,
        });
    }

    /// Declare frame storage and return its address.
    pub fn alloc_slot(&mut self, kind: SlotKind, name: &str) -> ValId {
        let idx = self.func.slots.len() as u32;
        self.func.slots.push(SlotInfo {
            kind,
            name: name.to_string(),
        });
        self.push(Op::SlotAddr(idx), IrTy::Ptr)
    }

    pub fn set_term(&mut self, t: Term) {
        if self.sealed.contains(&self.cur) {
            return;
        }
        let b = &mut self.func.blocks[self.cur as usize];
        // First terminator wins: later ones are unreachable code produced by
        // lowering `return` inside an expression position.
        if matches!(b.term, Term::Unreachable) {
            b.term = t;
        }
    }

    /// Mark the current block unreachable. Nothing further is emitted into it.
    pub fn seal(&mut self) {
        self.sealed.insert(self.cur);
        self.func.blocks[self.cur as usize].term = Term::Unreachable;
    }

    pub fn terminated(&self) -> bool {
        self.sealed.contains(&self.cur)
            || !matches!(self.func.blocks[self.cur as usize].term, Term::Unreachable)
    }

    pub fn const_int(&mut self, v: i128, ty: IrTy) -> ValId {
        self.push(Op::ConstInt(v), ty)
    }

    pub fn const_bool(&mut self, v: bool) -> ValId {
        self.push(Op::ConstBool(v), IrTy::Bool)
    }

    pub fn unit(&mut self) -> ValId {
        self.push(Op::ConstUnit, IrTy::Unit)
    }

    pub fn bin(&mut self, op: BinKind, l: ValId, r: ValId) -> ValId {
        let ty = if op.is_cmp() {
            IrTy::Bool
        } else {
            self.func.ty_of(l)
        };
        self.push(Op::Bin { op, l, r }, ty)
    }

    pub fn load(&mut self, ty: IrTy, ptr: ValId) -> ValId {
        self.push(Op::Load { ty, ptr }, ty)
    }

    pub fn store(&mut self, ty: IrTy, ptr: ValId, val: ValId) {
        self.push_void(Op::Store { ty, ptr, val });
    }

    pub fn field_ptr(&mut self, agg: TypeId, field: u32, ptr: ValId) -> ValId {
        self.push(Op::FieldPtr { agg, field, ptr }, IrTy::Ptr)
    }

    pub fn finish(self) -> Func {
        self.func
    }
}

/// Verify structural invariants. Cheap enough to run on every build; a failure
/// here is a lowering bug, and catching it before a backend is far easier than
/// debugging generated C.
pub fn verify(p: &Program) -> Result<(), String> {
    for f in &p.funcs {
        for b in &f.blocks {
            if matches!(b.term, Term::Unreachable) && !b.insts.is_empty() {
                // Allowed: an explicitly unreachable block. Only flag blocks
                // that fall off the end with real work in them.
            }
            for i in &b.insts {
                for v in &i.results {
                    if *v as usize >= f.val_tys.len() {
                        return Err(format!("@{}: bb{} defines unknown v{v}", f.name, b.id));
                    }
                }
                match &i.op {
                    Op::Store { .. }
                    | Op::CopyAgg { .. }
                    | Op::RegionEnter(_)
                    | Op::RegionExit(_) => {
                        if !i.results.is_empty() {
                            return Err(format!(
                                "@{}: bb{}: void op defines a value",
                                f.name, b.id
                            ));
                        }
                    }
                    Op::Call { f: callee, .. } => {
                        if *callee as usize >= p.funcs.len() {
                            return Err(format!("@{}: call to unknown func {callee}", f.name));
                        }
                    }
                    Op::FieldPtr { agg, field, .. } => {
                        let a = p.agg(*agg);
                        if *field as usize >= a.fields.len() {
                            return Err(format!(
                                "@{}: field.ptr %{agg}.{field} out of range ({} fields)",
                                f.name,
                                a.fields.len()
                            ));
                        }
                    }
                    _ => {}
                }
            }
            let check_edge = |e: &Edge| -> Result<(), String> {
                let target = f
                    .blocks
                    .get(e.to as usize)
                    .ok_or_else(|| format!("@{}: edge to unknown bb{}", f.name, e.to))?;
                if target.params.len() != e.args.len() {
                    return Err(format!(
                        "@{}: bb{} -> bb{} passes {} args, expects {}",
                        f.name,
                        b.id,
                        e.to,
                        e.args.len(),
                        target.params.len()
                    ));
                }
                Ok(())
            };
            match &b.term {
                Term::Jump(e) => check_edge(e)?,
                Term::Br { then_e, else_e, .. } => {
                    check_edge(then_e)?;
                    check_edge(else_e)?;
                }
                Term::Switch { cases, default, .. } => {
                    for (_, e) in cases {
                        check_edge(e)?;
                    }
                    check_edge(default)?;
                }
                Term::RetErr(_) => {
                    if !f.is_fallible() {
                        return Err(format!("@{}: ret.err in an infallible function", f.name));
                    }
                }
                // A mismatch here means lowering computed the wrong type for the
                // returned expression. Caught cheaply now; otherwise it shows up
                // as a silently wrong value after C's implicit conversions.
                Term::Ret(v) => {
                    let want = if f.ret_agg.is_some() {
                        IrTy::Unit
                    } else {
                        f.ret
                    };
                    match v {
                        Some(x) => {
                            let got = f.ty_of(*x);
                            if got != want && want != IrTy::Unit {
                                return Err(format!(
                                    "@{}: bb{} returns {} but the signature says {}",
                                    f.name,
                                    b.id,
                                    got.name(),
                                    want.name()
                                ));
                            }
                        }
                        None => {
                            if want != IrTy::Unit {
                                return Err(format!(
                                    "@{}: bb{} returns nothing but the signature says {}",
                                    f.name,
                                    b.id,
                                    want.name()
                                ));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
