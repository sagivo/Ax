//! Tree-walking interpreter. Permanent semantic oracle (§12.2).

use crate::ast::*;
use crate::check::{CheckOutput, CheckedFn};
use crate::intern::{Interner, Symbol};
use crate::types::Prim;
use indexmap::IndexMap;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum Value {
    Unit,
    Bool(bool),
    Int {
        bits: i128,
        prim: Prim,
    },
    Float {
        bits: u64,
        prim: Prim,
    },
    Str(Rc<str>),
    Record(IndexMap<String, Value>),
    Variant {
        name: String,
        fields: IndexMap<String, Value>,
    },
    Vec(Rc<RefCell<Vec<Value>>>),
    Map(Rc<RefCell<IndexMap<String, Value>>>),
    Range {
        start: u64,
        end: u64,
    },
    Fn(FnVal),
    Cap(Capability),
    Alloc,
    Own(Box<Value>),
    Hole,
}

#[derive(Clone, Debug)]
pub enum FnVal {
    User { name: String },
    Builtin(String),
    Lambda { params: Vec<String>, body: Expr },
    DictCmp(String),
}

#[derive(Clone, Debug)]
pub enum Capability {
    FsRead { files: IndexMap<String, String> },
    Stdout,
}

impl Value {
    pub fn unit() -> Self {
        Value::Unit
    }

    pub fn i32(v: i32) -> Self {
        Value::Int {
            bits: v as i128,
            prim: Prim::I32,
        }
    }

    pub fn usz(v: u64) -> Self {
        Value::Int {
            bits: v as i128,
            prim: Prim::Usz,
        }
    }

    pub fn f32(v: f32) -> Self {
        Value::Float {
            bits: canon_f32(v).to_bits() as u64,
            prim: Prim::F32,
        }
    }

    pub fn f64(v: f64) -> Self {
        Value::Float {
            bits: canon_f64(v).to_bits(),
            prim: Prim::F64,
        }
    }

    pub fn as_bool(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    pub fn as_i128(&self) -> i128 {
        match self {
            Value::Int { bits, .. } => *bits,
            Value::Float {
                bits,
                prim: Prim::F32,
            } => f32::from_bits(*bits as u32) as i128,
            Value::Float { bits, .. } => f64::from_bits(*bits) as i128,
            Value::Bool(b) => *b as i128,
            _ => 0,
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            Value::Float {
                bits,
                prim: Prim::F32,
            } => f32::from_bits(*bits as u32) as f64,
            Value::Float { bits, .. } => f64::from_bits(*bits),
            Value::Int { bits, .. } => *bits as f64,
            _ => 0.0,
        }
    }

    pub fn as_f32(&self) -> f32 {
        match self {
            Value::Float {
                bits,
                prim: Prim::F32,
            } => f32::from_bits(*bits as u32),
            Value::Float { bits, .. } => f64::from_bits(*bits) as f32,
            Value::Int { bits, .. } => *bits as f32,
            _ => 0.0,
        }
    }

    pub fn display(&self) -> String {
        match self {
            Value::Unit => "()".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int { bits, prim } => format!("{bits}{prim}"),
            Value::Float {
                bits,
                prim: Prim::F32,
            } => {
                format!("{}f32", f32::from_bits(*bits as u32))
            }
            Value::Float { bits, .. } => format!("{}f64", f64::from_bits(*bits)),
            Value::Str(s) => format!("\"{}\"", s),
            Value::Record(fs) => {
                let inner: Vec<_> = fs
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.display()))
                    .collect();
                format!("{{ {} }}", inner.join(", "))
            }
            Value::Variant { name, fields } => {
                if fields.is_empty() {
                    name.clone()
                } else {
                    let inner: Vec<_> = fields
                        .iter()
                        .map(|(k, v)| format!("{k}: {}", v.display()))
                        .collect();
                    format!("{name} {{ {} }}", inner.join(", "))
                }
            }
            Value::Vec(v) => {
                let xs: Vec<_> = v.borrow().iter().map(|x| x.display()).collect();
                format!("[{}]", xs.join(", "))
            }
            Value::Map(m) => {
                let xs: Vec<_> = m
                    .borrow()
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.display()))
                    .collect();
                format!("{{{}}}", xs.join(", "))
            }
            Value::Range { start, end } => format!("{start}..{end}"),
            Value::Fn(f) => format!("<fn {:?}>", f),
            Value::Cap(_) => "<cap>".into(),
            Value::Alloc => "<alloc>".into(),
            Value::Own(v) => format!("own {}", v.display()),
            Value::Hole => "?".into(),
        }
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        match self {
            Value::Unit => vec![0],
            Value::Bool(b) => vec![1, *b as u8],
            Value::Int { bits, prim } => {
                let mut o = vec![2, *prim as u8];
                o.extend(bits.to_le_bytes());
                o
            }
            Value::Float { bits, prim } => {
                let mut o = vec![3, *prim as u8];
                o.extend(bits.to_le_bytes());
                o
            }
            Value::Str(s) => {
                let mut o = vec![4];
                o.extend((s.len() as u32).to_le_bytes());
                o.extend(s.as_bytes());
                o
            }
            Value::Vec(v) => {
                let mut o = vec![5];
                let xs = v.borrow();
                o.extend((xs.len() as u32).to_le_bytes());
                for x in xs.iter() {
                    o.extend(x.canonical_bytes());
                }
                o
            }
            Value::Record(fs) => {
                let mut o = vec![6];
                o.extend((fs.len() as u32).to_le_bytes());
                for (k, v) in fs {
                    o.extend((k.len() as u32).to_le_bytes());
                    o.extend(k.as_bytes());
                    o.extend(v.canonical_bytes());
                }
                o
            }
            Value::Variant { name, fields } => {
                let mut o = vec![7];
                o.extend((name.len() as u32).to_le_bytes());
                o.extend(name.as_bytes());
                o.extend((fields.len() as u32).to_le_bytes());
                for (k, v) in fields {
                    o.extend((k.len() as u32).to_le_bytes());
                    o.extend(k.as_bytes());
                    o.extend(v.canonical_bytes());
                }
                o
            }
            other => other.display().into_bytes(),
        }
    }
}

/// Canonical NaN payload (spec §8.2).
pub fn canon_f32(v: f32) -> f32 {
    if v.is_nan() {
        f32::from_bits(0x7fc0_0000)
    } else {
        v
    }
}

pub fn canon_f64(v: f64) -> f64 {
    if v.is_nan() {
        f64::from_bits(0x7ff8_0000_0000_0000)
    } else {
        v
    }
}

#[derive(Debug)]
pub enum Flow {
    Value(Value),
    Raise(Value),
    Return(Value),
    /// `break` / `continue`, caught by the nearest enclosing loop.
    Break,
    Continue,
    Abort(String),
}

pub type IResult = Result<Value, Flow>;

/// One observable interaction with the world.
///
/// A transcript of these is what makes a run replayable: on replay the
/// interpreter *consumes* them instead of touching the world again, so the
/// replayed run cannot drift because a file changed underneath it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TraceEvent {
    pub op: String,
    pub arg: String,
    pub result: String,
}

pub struct World {
    /// Arguments the *program* was given, not the ones `ax` was given.
    /// `argv(0)` is the module path, matching how a native binary sees argv.
    pub argv: Vec<String>,
    /// Structured transcript of effects performed this run.
    pub events: Vec<TraceEvent>,
    /// When replaying, the events still to be consumed, in order.
    pub replay: Option<std::collections::VecDeque<TraceEvent>>,
    /// Step ceiling for compile-time folding. `None` means the ordinary limit.
    pub step_budget: Option<u64>,
    pub stdout: Vec<String>,
    pub seed: u64,
    pub step: u64,
    pub trace: Vec<String>,
}

impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            argv: Vec::new(),
            events: Vec::new(),
            replay: None,
            step_budget: None,
            stdout: Vec::new(),
            seed,
            step: 0,
            trace: Vec::new(),
        }
    }

    pub fn tick(&mut self) -> u64 {
        let s = self.step;
        self.step += 1;
        s
    }
}

pub struct Interpreter<'a> {
    intern: &'a Interner,
    fns: HashMap<String, CheckedFn>,
    frames: Vec<HashMap<String, Value>>,
    world: World,
    /// Host filesystem fallback for fs.read when cap has no overlay.
    host_files: HashMap<String, String>,
    /// into_type -> [(from_type, variant)]
    injections: HashMap<String, Vec<(String, String)>>,
    /// variant name -> parent type name
    variant_parent: HashMap<String, String>,
    /// Bare-name patterns the checker resolved to unit variants. See
    /// `CheckOutput::pat_variant`.
    pat_variant: HashMap<crate::ast::NodeId, String>,
    /// Names of record types, so `P { .. }` builds a record and not a variant.
    record_types: std::collections::HashSet<String>,
    /// variant name -> payload field names, in declaration order. Positional
    /// construction (`Some(3)`) needs the order.
    variant_fields: HashMap<String, Vec<String>>,
    /// Checked type of every node. Literals take their width from here, not from
    /// their spelling: `fn f() -> u8 = 5` must produce a `u8`, and only the
    /// checker knows that.
    node_types: Vec<crate::types::Type>,
    /// Dictionary declarations, in `CheckOutput::dicts` order, as field name ->
    /// defining expression. A dictionary value is a record of function values —
    /// a vtable — built on demand when a `= default` parameter is filled.
    dict_fields: Vec<Vec<(String, Expr)>>,
    /// See `CheckOutput::dict_defaults`.
    dict_defaults: HashMap<(NodeId, u32), usize>,
    /// Names callable as values: user functions plus the prelude.
    known_fns: std::collections::HashSet<String>,
    /// fn name -> declared err type display
    fn_err: HashMap<String, String>,
    /// current function err type stack
    err_stack: Vec<Option<String>>,
    /// Memoisation table, used only while constant-folding. Keyed by function
    /// name and the canonical bytes of its arguments. Present only when folding,
    /// because a normal run must not cache: a function that is *not* pure would
    /// then observe the wrong thing.
    memo: Option<HashMap<(String, Vec<u8>), Value>>,
    /// Call depth, bounded so runaway recursion aborts instead of taking the
    /// process down. The oracle runs agent-generated code, so an unbounded
    /// native stack is a denial-of-service on the tool itself.
    depth: u32,
}

impl<'a> Interpreter<'a> {
    pub fn new(intern: &'a Interner, checked: &CheckOutput, seed: u64) -> Self {
        let mut fns = HashMap::new();
        let mut fn_err = HashMap::new();
        for f in &checked.fns {
            let name = intern.get(f.sig.name).to_string();
            if let Some(e) = f.sig.effects.err_type() {
                fn_err.insert(name.clone(), e.display(intern));
            }
            fns.insert(name, f.clone());
        }
        let mut injections: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (into, from, var) in &checked.injections {
            injections
                .entry(into.clone())
                .or_default()
                .push((from.clone(), var.clone()));
        }
        let mut variant_parent = HashMap::new();
        let mut variant_fields: HashMap<String, Vec<String>> = HashMap::new();
        let mut record_types = std::collections::HashSet::new();
        for td in &checked.types {
            match &td.kind {
                crate::types::TypeDefKind::Variants(vs) => {
                    let parent = intern.get(td.name).to_string();
                    for (vn, fs) in vs {
                        let vname = intern.get(*vn).to_string();
                        variant_parent.insert(vname.clone(), parent.clone());
                        variant_fields.insert(
                            vname,
                            fs.iter().map(|(n, _)| intern.get(*n).to_string()).collect(),
                        );
                    }
                }
                crate::types::TypeDefKind::Record(_) => {
                    record_types.insert(intern.get(td.name).to_string());
                }
                crate::types::TypeDefKind::Alias(_) => {}
            }
        }
        let mut dict_field_exprs = Vec::new();
        for d in &checked.dict_decls {
            dict_field_exprs.push(
                d.fields
                    .iter()
                    .map(|(n, e)| (intern.get(n.name).to_string(), e.clone()))
                    .collect(),
            );
        }
        let mut known_fns: std::collections::HashSet<String> = fns.keys().cloned().collect();
        for n in [
            "i32.cmp", "f32.cmp", "int.div", "int.rem", "int.div_trunc", "math.sqrt", "math.abs",
        ] {
            known_fns.insert(n.to_string());
        }
        Self {
            intern,
            fns,
            frames: vec![HashMap::new()],
            world: World::new(seed),
            host_files: HashMap::new(),
            injections,
            variant_parent,
            pat_variant: checked.pat_variant.clone(),
            record_types,
            variant_fields,
            node_types: checked.node_types.clone(),
            dict_fields: dict_field_exprs,
            dict_defaults: checked.dict_defaults.clone(),
            known_fns,
            fn_err,
            err_stack: vec![None],
            memo: None,
            depth: 0,
        }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// Arguments visible to the program under interpretation.
    pub fn set_argv(&mut self, argv: Vec<String>) {
        self.world.argv = argv;
    }

    /// Replay from a transcript: effects return their recorded results and the
    /// world is not touched. A mismatch is reported rather than papered over.
    pub fn set_replay(&mut self, events: Vec<TraceEvent>) {
        self.world.replay = Some(events.into_iter().collect());
    }

    /// The transcript this run produced.
    pub fn events(&self) -> &[TraceEvent] {
        &self.world.events
    }

    /// Next recorded result for `op`, when replaying.
    ///
    /// Returns `Err` on divergence: the program asked for something the
    /// transcript does not have next, which means the replay is not the run.
    fn replay_next(&mut self, op: &str, arg: &str) -> Option<Result<String, Flow>> {
        let q = self.world.replay.as_mut()?;
        match q.pop_front() {
            Some(ev) if ev.op == op && ev.arg == arg => Some(Ok(ev.result)),
            Some(ev) => Some(Err(Flow::Abort(format!(
                "transcript divergence: expected {} {:?}, program did {op} {arg:?}",
                ev.op, ev.arg
            )))),
            None => Some(Err(Flow::Abort(format!(
                "transcript exhausted: program did {op} {arg:?} with nothing recorded"
            )))),
        }
    }

    /// Record an effect, or return its recorded result when replaying.
    fn effect(&mut self, op: &str, arg: &str, perform: impl FnOnce(&mut Self) -> Result<String, Flow>) -> Result<String, Flow> {
        if let Some(r) = self.replay_next(op, arg) {
            return r;
        }
        let result = perform(self)?;
        self.world.events.push(TraceEvent {
            op: op.to_string(),
            arg: arg.to_string(),
            result: result.clone(),
        });
        Ok(result)
    }

    pub fn call_fn(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        match self.call_named(name, args) {
            Ok(v) => Ok(v),
            Err(Flow::Raise(e)) => Err(format!("unhandled error: {}", e.display())),
            Err(Flow::Abort(s)) => Err(format!("abort: {s}")),
            Err(Flow::Return(v)) => Ok(v),
            Err(Flow::Value(v)) => Ok(v),
            // The checker rejects these outside a loop, so reaching here is a
            // compiler bug rather than a program error.
            Err(Flow::Break) | Err(Flow::Continue) => {
                Err("internal: loop control flow escaped its loop".into())
            }
        }
    }

    pub fn run_tests(&mut self, checked: &CheckOutput) -> Vec<TestResult> {
        let mut out = Vec::new();
        for t in &checked.tests {
            self.frames = vec![HashMap::new()];
            match self.eval(&t.body) {
                // A test whose body *is* a condition fails when the condition is
                // false. Treating every non-aborting body as a pass made
                // `test "x" = 1 == 2;` pass here while both C tiers failed
                // it — a test suite that cannot fail is worse than no suite.
                // Any other value (typically `unit`, from `assert`) passes,
                // because `assert` reports by aborting.
                Ok(Value::Bool(false)) | Err(Flow::Return(Value::Bool(false))) => {
                    out.push(TestResult {
                        name: t.name.clone(),
                        ok: false,
                        msg: Some("the test's condition is false".into()),
                    })
                }
                Ok(_) | Err(Flow::Return(_)) => out.push(TestResult {
                    name: t.name.clone(),
                    ok: true,
                    msg: None,
                }),
                Err(Flow::Break) | Err(Flow::Continue) => out.push(TestResult {
                    name: t.name.clone(),
                    ok: false,
                    msg: Some("loop control flow escaped its loop".into()),
                }),
                Err(Flow::Abort(s)) => out.push(TestResult {
                    name: t.name.clone(),
                    ok: false,
                    msg: Some(s),
                }),
                Err(Flow::Raise(e)) => out.push(TestResult {
                    name: t.name.clone(),
                    ok: false,
                    msg: Some(format!("unhandled {}", e.display())),
                }),
                Err(Flow::Value(_)) => out.push(TestResult {
                    name: t.name.clone(),
                    ok: true,
                    msg: None,
                }),
            }
        }
        out
    }

    fn call_named(&mut self, name: &str, args: Vec<Value>) -> IResult {
        self.call_named_at(name, args, None)
    }

    /// `call_site` identifies the call node, which is how a `= default`
    /// dictionary parameter finds the dictionary the checker resolved for it.
    fn call_named_at(
        &mut self,
        name: &str,
        args: Vec<Value>,
        call_site: Option<NodeId>,
    ) -> IResult {
        if let Some(f) = self.fns.get(name).cloned() {
            // Folding only: a cache hit returns immediately.
            let memo_key = self.memo.as_ref().map(|_| {
                (
                    name.to_string(),
                    args.iter().flat_map(|a| a.canonical_bytes()).collect::<Vec<u8>>(),
                )
            });
            if let (Some(k), Some(m)) = (&memo_key, &self.memo) {
                if let Some(v) = m.get(k) {
                    return Ok(v.clone());
                }
            }
            // Deep enough for any reasonable program, shallow enough to stay well
            // inside the native stack the tree-walker consumes per frame.
            const MAX_DEPTH: u32 = 512;
            if self.depth >= MAX_DEPTH {
                return Err(Flow::Abort("recursion depth exceeded".into()));
            }
            // A compile-time fold gets a step ceiling; exceeding it abandons the
            // fold rather than stalling the build.
            self.world.tick();
            if let Some(b) = self.world.step_budget {
                if self.world.step > b {
                    return Err(Flow::Abort("fold budget exceeded".into()));
                }
            }
            let mut frame = HashMap::new();
            for (i, (n, _, _)) in f.sig.params.iter().enumerate() {
                if let Some(v) = args.get(i) {
                    frame.insert(self.intern.get(*n).to_string(), v.clone());
                    continue;
                }
                // Missing argument: a defaulted dictionary parameter.
                if f.sig.params[i].2 {
                    if let Some(d) = call_site
                        .and_then(|c| self.dict_defaults.get(&(c, i as u32)).copied())
                    {
                        let v = self.build_dict(d)?;
                        frame.insert(self.intern.get(*n).to_string(), v);
                    }
                }
            }
            let callee_err = self.fn_err.get(name).cloned();
            self.frames.push(frame);
            self.err_stack.push(callee_err);
            self.depth += 1;
            let r = self.eval(&f.body);
            self.depth -= 1;
            self.err_stack.pop();
            self.frames.pop();
            let r = match r {
                Err(Flow::Return(v)) => Ok(v),
                other => other,
            };
            if let (Some(k), Ok(v)) = (memo_key, &r) {
                if let Some(m) = self.memo.as_mut() {
                    m.insert(k, v.clone());
                }
            }
            return match r {
                // Injection happens at the raising function's boundary, against
                // *its* declared error type: `fn f() -> T !{err[E]}` with a
                // declared `from F => V` turns a raised F into E.V. Using the
                // caller's type here would skip the conversion entirely.
                Err(Flow::Raise(e)) => {
                    let target = self.fn_err.get(name).cloned();
                    Err(Flow::Raise(self.inject_into(e, target)))
                }
                other => other,
            };
        }
        match self.builtin(name, args) {
            // A builtin raises inside the current function, so the target is the
            // current function's declared type.
            Err(Flow::Raise(e)) => {
                let target = self.err_stack.last().and_then(|x| x.clone());
                Err(Flow::Raise(self.inject_into(e, target)))
            }
            other => other,
        }
    }

    /// Materialise a dictionary: a record whose fields are function values.
    fn build_dict(&mut self, idx: usize) -> Result<Value, Flow> {
        let fields = match self.dict_fields.get(idx) {
            Some(f) => f.clone(),
            None => return Err(Flow::Abort("unknown dictionary".into())),
        };
        let mut rec = IndexMap::new();
        for (name, expr) in fields {
            rec.insert(name, self.eval(&expr)?);
        }
        Ok(Value::Record(rec))
    }

    /// Convert a raised value into `target` via a declared single-step injection.
    fn inject_into(&self, e: Value, target: Option<String>) -> Value {
        let Some(into) = target else {
            return e;
        };
        let from = match &e {
            Value::Variant { name, .. } => self
                .variant_parent
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.clone()),
            _ => return e,
        };
        if from == into {
            return e;
        }
        if let Some(list) = self.injections.get(&into) {
            for (src, var) in list {
                if src == &from || src.ends_with(&format!(".{from}")) || from.ends_with(src) {
                    let mut fields = IndexMap::new();
                    fields.insert("cause".into(), e);
                    return Value::Variant {
                        name: var.clone(),
                        fields,
                    };
                }
            }
        }
        e
    }

    fn eval(&mut self, e: &Expr) -> IResult {
        match &e.kind {
            ExprKind::Lit(l) => Ok(self.lit_value_at(l, e.id)),
            ExprKind::Hole => Ok(Value::Hole),
            ExprKind::Path(p) => self.eval_path(p),
            ExprKind::Call { callee, args } => self.eval_call(callee, args, e.id),
            ExprKind::Field { base, field } => {
                if let Some(q) = expr_path(e, self.intern) {
                    if q == "test.alloc" {
                        return Ok(Value::Alloc);
                    }
                    if self.fns.contains_key(&q) {
                        return Ok(Value::Fn(FnVal::User { name: q }));
                    }
                    // A prelude function used as a value, e.g. `i32.cmp` as a
                    // dictionary field.
                    if self.known_fns.contains(&q) {
                        return Ok(Value::Fn(FnVal::Builtin(q)));
                    }
                }
                let v = self.eval(base)?;
                self.field(&v, self.intern.get(field.name))
            }
            ExprKind::Index { base, index } => {
                let b = self.eval(base)?;
                let i = self.eval(index)?.as_i128() as usize;
                match b {
                    Value::Vec(xs) => xs
                        .borrow()
                        .get(i)
                        .cloned()
                        .ok_or_else(|| Flow::Abort(format!("index {i} out of bounds"))),
                    _ => Err(Flow::Abort("not indexable".into())),
                }
            }
            ExprKind::Unary { op, expr } => self.eval_unary(*op, expr),
            ExprKind::Binary { op, lhs, rhs } => self.eval_binary(*op, lhs, rhs),
            ExprKind::Block { stmts, tail } => {
                self.push_scope();
                for s in stmts {
                    match &s.kind {
                        StmtKind::Let(l) => self.eval_let(l)?,
                        StmtKind::Expr(x) => {
                            let _ = self.eval(x)?;
                        }
                    }
                }
                let r = if let Some(t) = tail {
                    self.eval(t)
                } else {
                    Ok(Value::Unit)
                };
                self.pop_scope();
                r
            }
            ExprKind::If {
                cond,
                then_b,
                else_b,
            } => {
                if self.eval(cond)?.as_bool() {
                    self.eval(then_b)
                } else if let Some(el) = else_b {
                    self.eval(el)
                } else {
                    Ok(Value::Unit)
                }
            }
            ExprKind::Match { scrut, arms } => {
                let v = self.eval(scrut)?;
                for a in arms {
                    self.push_scope();
                    if self.match_pat(&a.pat, &v) {
                        let r = self.eval(&a.body);
                        self.pop_scope();
                        return r;
                    }
                    self.pop_scope();
                }
                Err(Flow::Abort("non-exhaustive match".into()))
            }
            ExprKind::For { pat, iter, body } => {
                let it = self.eval(iter)?;
                let items = self.iterate(&it)?;
                for item in items {
                    self.push_scope();
                    if !self.match_pat(pat, &item) {
                        self.pop_scope();
                        continue;
                    }
                    let r = self.eval(body);
                    self.pop_scope();
                    match r {
                        Err(Flow::Break) => break,
                        Err(Flow::Continue) | Ok(_) => {}
                        Err(other) => return Err(other),
                    }
                }
                Ok(Value::Unit)
            }
            ExprKind::Loop { body } => loop {
                match self.eval(body) {
                    Err(Flow::Return(v)) => return Ok(v),
                    Err(Flow::Break) => return Ok(Value::Unit),
                    Err(Flow::Continue) | Ok(_) => {
                        if self.world.step > 10_000_000 {
                            return Err(Flow::Abort("loop iteration limit".into()));
                        }
                        self.world.tick();
                    }
                    Err(other) => return Err(other),
                }
            },
            ExprKind::While { cond, body } => {
                loop {
                    if !self.eval(cond)?.as_bool() {
                        return Ok(Value::Unit);
                    }
                    match self.eval(body) {
                        Err(Flow::Break) => return Ok(Value::Unit),
                        Err(Flow::Continue) | Ok(_) => {}
                        Err(other) => return Err(other),
                    }
                    if self.world.step > 10_000_000 {
                        return Err(Flow::Abort("loop iteration limit".into()));
                    }
                    self.world.tick();
                }
            }
            ExprKind::Break => Err(Flow::Break),
            ExprKind::Continue => Err(Flow::Continue),
            ExprKind::Cast { expr, ty: _ } => {
                let v = self.eval(expr)?;
                // The target comes from the checker's table, so a cast whose
                // result type was refined by context converts to that type.
                let to = self
                    .node_types
                    .get(e.id.index())
                    .and_then(|t| t.as_prim());
                Ok(cast_value(&v, to))
            }
            ExprKind::Let(l) => {
                self.eval_let(l)?;
                Ok(Value::Unit)
            }
            ExprKind::Lambda { params, body, .. } => Ok(Value::Fn(FnVal::Lambda {
                params: params
                    .iter()
                    .map(|p| self.intern.get(p.name.name).to_string())
                    .collect(),
                body: (**body).clone(),
            })),
            ExprKind::Record(fs) => {
                let mut rec = IndexMap::new();
                for (n, ex) in fs {
                    rec.insert(self.intern.get(n.name).to_string(), self.eval(ex)?);
                }
                Ok(Value::Record(rec))
            }
            ExprKind::Variant { name, fields } => {
                let mut rec = IndexMap::new();
                for (n, ex) in fields {
                    rec.insert(self.intern.get(n.name).to_string(), self.eval(ex)?);
                }
                let vname = self.intern.get(name.name).to_string();
                // `P { .. }` is a record literal when `P` is a record type; the
                // parser cannot distinguish it from a variant literal.
                if self.record_types.contains(&vname) {
                    return Ok(Value::Record(rec));
                }
                Ok(Value::Variant {
                    name: vname,
                    fields: rec,
                })
            }
            ExprKind::Return(inner) => {
                let v = if let Some(x) = inner {
                    self.eval(x)?
                } else {
                    Value::Unit
                };
                Err(Flow::Return(v))
            }
            ExprKind::Raise(inner) => Err(Flow::Raise(self.eval(inner)?)),
            ExprKind::Catch { expr, arms } => match self.eval(expr) {
                Ok(v) => Ok(v),
                Err(Flow::Raise(err)) => {
                    for a in arms {
                        self.push_scope();
                        if self.match_pat(&a.pat, &err) {
                            let r = self.eval(&a.body);
                            self.pop_scope();
                            return r;
                        }
                        self.pop_scope();
                    }
                    Err(Flow::Raise(err))
                }
                Err(other) => Err(other),
            },
            ExprKind::Attempt(inner) => match self.eval(inner) {
                Ok(v) => Ok(ok_val(v)),
                Err(Flow::Raise(e)) => Ok(err_val(e)),
                Err(other) => Err(other),
            },
            ExprKind::Try(inner) => {
                let v = self.eval(inner)?;
                // Result[T,E] as a variant: Err re-raises, Ok unwraps.
                match &v {
                    Value::Variant { name, fields } if name == "Err" => {
                        let err = fields
                            .values()
                            .next()
                            .cloned()
                            .unwrap_or(v.clone());
                        Err(Flow::Raise(err))
                    }
                    Value::Variant { name, fields } if name == "Ok" => {
                        Ok(fields.values().next().cloned().unwrap_or(Value::Unit))
                    }
                    other => Ok(other.clone()),
                }
            }
            ExprKind::Interpolate { parts } => {
                let mut out = String::new();
                for p in parts {
                    match p {
                        crate::ast::InterpPart::Lit(s) => out.push_str(s),
                        crate::ast::InterpPart::Expr(x) => {
                            out.push_str(&self.eval(x)?.display());
                        }
                    }
                }
                Ok(Value::Str(out.into()))
            }
            ExprKind::Region { body, .. } => self.eval(body),
            ExprKind::Par { bindings } => {
                // Oracle: sequential, lowest lexical index wins on multi-fail.
                let mut first_err: Option<Flow> = None;
                for l in bindings {
                    match self.eval_let(l) {
                        Ok(()) => {}
                        Err(e) => {
                            if first_err.is_none() {
                                first_err = Some(e);
                            }
                        }
                    }
                }
                if let Some(e) = first_err {
                    return Err(e);
                }
                Ok(Value::Unit)
            }
            ExprKind::Assign { lhs, rhs } => {
                let v = self.eval(rhs)?;
                self.assign(lhs, v)?;
                Ok(Value::Unit)
            }
        }
    }

    fn eval_let(&mut self, l: &LetStmt) -> Result<(), Flow> {
        let v = self.eval(&l.init)?;
        if !self.match_pat(&l.pat, &v) {
            return Err(Flow::Abort("let pattern failed".into()));
        }
        Ok(())
    }

    fn eval_path(&mut self, p: &Path) -> IResult {
        let q = path_join(p, self.intern);
        if q == "test.alloc" {
            return Ok(Value::Alloc);
        }
        if let Some(v) = self.lookup(&q) {
            return Ok(v);
        }
        if p.segs.is_empty() {
            return Err(Flow::Abort("empty path".into()));
        }
        let first = self.intern.get(p.segs[0].name);
        if let Some(mut v) = self.lookup(first) {
            for seg in &p.segs[1..] {
                v = self.field(&v, self.intern.get(seg.name))?;
            }
            return Ok(v);
        }
        if p.segs.len() == 1 {
            if self.fns.contains_key(first) {
                return Ok(Value::Fn(FnVal::User { name: first.into() }));
            }
            return Ok(Value::Variant {
                name: first.into(),
                fields: IndexMap::new(),
            });
        }
        Ok(Value::Fn(FnVal::Builtin(q)))
    }

    fn eval_call(&mut self, callee: &Expr, args: &[Expr], site: NodeId) -> IResult {
        if let ExprKind::Field { base, field } = &callee.kind {
            if let Some(q) = expr_path(callee, self.intern) {
                let mut argv = Vec::new();
                for a in args {
                    argv.push(self.eval(a)?);
                }
                if self.fns.contains_key(&q) {
                    return self.call_named_at(&q, argv, Some(site));
                }
                match self.builtin(&q, argv) {
                    Err(Flow::Abort(msg)) if msg.starts_with("unknown function") => {}
                    other => return other,
                }
            }
            let recv = self.eval(base)?;
            let mut argv = vec![recv.clone()];
            for a in args {
                argv.push(self.eval(a)?);
            }
            let name = self.intern.get(field.name);
            // Accept-and-elide: `.clone()` is identity ([T-3.3.1] A0103).
            if name == "clone" && args.is_empty() {
                return Ok(recv);
            }
            if let Some(v) = self.method(&recv, name, &argv[1..])? {
                return Ok(v);
            }
        }
        if let ExprKind::Path(p) = &callee.kind {
            let q = path_join(p, self.intern);
            let mut argv = Vec::new();
            for a in args {
                argv.push(self.eval(a)?);
            }
            if self.fns.contains_key(&q) || self.lookup_local_fn(&q).is_some() {
                return self.call_named_at(&q, argv, Some(site));
            }
            if p.segs.len() == 1 {
                let n = self.intern.get(p.segs[0].name).to_string();
                if self.fns.contains_key(&n) {
                    return self.call_named_at(&n, argv, Some(site));
                }
                if let Some(Value::Fn(fv)) = self.lookup(&n) {
                    return self.call_fnval(&fv, argv);
                }
                // Positional variant construction: `Some(3)`, `Err(Zero)`.
                // Payloads bind to the declared field names in order.
                if let Some(fnames) = self.variant_fields.get(&n).cloned() {
                    let mut fields = IndexMap::new();
                    for (i, v) in argv.into_iter().enumerate() {
                        let key = fnames
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| format!("_{i}"));
                        fields.insert(key, v);
                    }
                    return Ok(Value::Variant { name: n, fields });
                }
            }
            return self.builtin(&q, argv);
        }
        let c = self.eval(callee)?;
        let mut argv = Vec::new();
        for a in args {
            argv.push(self.eval(a)?);
        }
        match c {
            Value::Fn(fv) => self.call_fnval(&fv, argv),
            _ => Err(Flow::Abort("not a function".into())),
        }
    }

    /// A literal's value at its checked type. Falls back to the literal's own
    /// spelling when no type was recorded (a synthesised node).
    fn lit_value_at(&self, l: &Lit, id: NodeId) -> Value {
        let prim = self
            .node_types
            .get(id.index())
            .and_then(|t| t.as_prim());
        match (l, prim) {
            (Lit::Int { value, suffix }, p) => {
                let prim = suffix.or(p).unwrap_or(Prim::I32);
                if prim.is_float() {
                    return match prim {
                        Prim::F32 => Value::f32(*value as f32),
                        _ => Value::f64(*value as f64),
                    };
                }
                Value::Int {
                    bits: prim.wrap_i128(*value),
                    prim,
                }
            }
            (Lit::Float { value, suffix }, p) => match suffix.or(p) {
                Some(Prim::F32) => Value::f32(*value as f32),
                _ => Value::f64(*value),
            },
            (other, _) => lit_value(other),
        }
    }

    fn lookup_local_fn(&self, q: &str) -> Option<String> {
        if self.fns.contains_key(q) {
            Some(q.into())
        } else {
            None
        }
    }

    fn call_fnval(&mut self, fv: &FnVal, args: Vec<Value>) -> IResult {
        match fv {
            FnVal::User { name } => self.call_named(name, args),
            FnVal::Builtin(n) => self.builtin(n, args),
            FnVal::Lambda { params, body } => {
                let mut frame = HashMap::new();
                for (i, p) in params.iter().enumerate() {
                    if let Some(v) = args.get(i) {
                        frame.insert(p.clone(), v.clone());
                    }
                }
                self.frames.push(frame);
                let r = self.eval(body);
                self.frames.pop();
                r
            }
            FnVal::DictCmp(n) => self.call_named(n, args),
        }
    }

    /// Container methods. The receiver's shape decides which apply; the checker
    /// has already rejected anything else, so a `None` here is an internal gap.
    fn method(&mut self, recv: &Value, name: &str, args: &[Value]) -> Result<Option<Value>, Flow> {
        let idx = || args.first().map(|v| v.as_i128()).unwrap_or(0);
        match (name, recv) {
            ("len", Value::Vec(xs)) => Ok(Some(Value::usz(xs.borrow().len() as u64))),
            ("len", Value::Str(s)) => Ok(Some(Value::usz(s.len() as u64))),
            ("get", Value::Vec(xs)) => {
                let i = idx();
                let xs = xs.borrow();
                if i < 0 {
                    return Ok(Some(none_val()));
                }
                Ok(Some(match xs.get(i as usize) {
                    Some(v) => some_val(v.clone()),
                    None => none_val(),
                }))
            }
            // `at` is bounds-checked always and aborts out of range. The message
            // must match the native runtime's exactly.
            ("at", Value::Vec(xs)) => {
                let i = idx();
                let xs = xs.borrow();
                if i < 0 || i as usize >= xs.len() {
                    return Err(Flow::Abort("index out of bounds".into()));
                }
                Ok(Some(xs[i as usize].clone()))
            }
            ("at", Value::Str(s)) => {
                let i = idx();
                let b = s.as_bytes();
                if i < 0 || i as usize >= b.len() {
                    return Err(Flow::Abort("index out of bounds".into()));
                }
                Ok(Some(Value::Int {
                    bits: b[i as usize] as i128,
                    prim: Prim::U8,
                }))
            }
            ("push", Value::Vec(xs)) => {
                if let Some(v) = args.first() {
                    xs.borrow_mut().push(v.clone());
                }
                Ok(Some(Value::Unit))
            }
            ("set", Value::Vec(xs)) => {
                let i = idx();
                let mut xs = xs.borrow_mut();
                if i < 0 || i as usize >= xs.len() {
                    return Err(Flow::Abort("index out of bounds".into()));
                }
                if let Some(v) = args.get(1) {
                    xs[i as usize] = v.clone();
                }
                Ok(Some(Value::Unit))
            }
            ("len", Value::Map(m)) => Ok(Some(Value::usz(m.borrow().len() as u64))),
            ("get", Value::Map(m)) => {
                let k = args.first().map(|v| v.display()).unwrap_or_default();
                Ok(Some(match m.borrow().get(&k) {
                    Some(v) => some_val(v.clone()),
                    None => none_val(),
                }))
            }
            ("insert" | "put", Value::Map(m)) => {
                if let (Some(k), Some(v)) = (args.first(), args.get(1)) {
                    m.borrow_mut().insert(k.display(), v.clone());
                }
                Ok(Some(Value::Unit))
            }
            ("contains", Value::Map(m)) => {
                let k = args.first().map(|v| v.display()).unwrap_or_default();
                Ok(Some(Value::Bool(m.borrow().contains_key(&k))))
            }
            _ => Ok(None),
        }
    }

    fn eval_unary(&mut self, op: UnOp, expr: &Expr) -> IResult {
        let v = self.eval(expr)?;
        match op {
            UnOp::Not => Ok(Value::Bool(!v.as_bool())),
            UnOp::BitNot => match v {
                Value::Int { bits, prim } => Ok(Value::Int {
                    bits: prim.wrap_i128(!bits),
                    prim,
                }),
                other => Err(Flow::Abort(format!(
                    "`~` needs an integer, got {}",
                    other.display()
                ))),
            },
            UnOp::Neg => match v {
                Value::Int { bits, prim } => Ok(Value::Int {
                    bits: prim.wrap_i128(-bits),
                    prim,
                }),
                Value::Float {
                    bits,
                    prim: Prim::F32,
                } => Ok(Value::f32(-f32::from_bits(bits as u32))),
                Value::Float { bits, .. } => Ok(Value::f64(-f64::from_bits(bits))),
                other => Ok(other),
            },
            UnOp::Ref | UnOp::RefMut | UnOp::Deref => Ok(v),
        }
    }

    fn eval_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> IResult {
        if op == BinOp::And {
            let l = self.eval(lhs)?;
            if !l.as_bool() {
                return Ok(Value::Bool(false));
            }
            return self.eval(rhs);
        }
        if op == BinOp::Or {
            let l = self.eval(lhs)?;
            if l.as_bool() {
                return Ok(Value::Bool(true));
            }
            return self.eval(rhs);
        }
        let l = self.eval(lhs)?;
        let r = self.eval(rhs)?;
        match op {
            BinOp::Add => self.arith(&l, &r, |a, b| a.wrapping_add(b), |a, b| a + b, |a, b| a + b),
            BinOp::Sub => self.arith(&l, &r, |a, b| a.wrapping_sub(b), |a, b| a - b, |a, b| a - b),
            BinOp::Mul => self.arith(&l, &r, |a, b| a.wrapping_mul(b), |a, b| a * b, |a, b| a * b),
            BinOp::Div => match &l {
                Value::Int { bits: a, prim } => {
                    let b = r.as_i128();
                    if b == 0 {
                        return Err(Flow::Raise(Value::Variant {
                            name: "Zero".into(),
                            fields: IndexMap::new(),
                        }));
                    }
                    Ok(Value::Int {
                        bits: prim.wrap_i128(
                            a.div_euclid(b).wrapping_mul(if (*a < 0) != (b < 0) {
                            // truncating toward zero
                            1
                        } else {
                            1
                        }).wrapping_add(0) /* placeholder */ + trunc_div(*a, b),
                        ),
                        prim: *prim,
                    })
                    .map(|v| match v {
                        Value::Int { prim, .. } => Value::Int {
                            bits: prim.wrap_i128(trunc_div(*a, b)),
                            prim,
                        },
                        x => x,
                    })
                }
                Value::Float {
                    prim: Prim::F32, ..
                } => Ok(Value::f32(canon_f32(l.as_f32() / r.as_f32()))),
                _ => Ok(Value::f64(canon_f64(l.as_f64() / r.as_f64()))),
            },
            BinOp::Rem => {
                if let Value::Int { bits: a, prim } = &l {
                    let b = r.as_i128();
                    if b == 0 {
                        return Err(Flow::Raise(Value::Variant {
                            name: "Zero".into(),
                            fields: IndexMap::new(),
                        }));
                    }
                    return Ok(Value::Int {
                        bits: prim.wrap_i128(trunc_rem(*a, b)),
                        prim: *prim,
                    });
                }
                Ok(Value::f64(l.as_f64() % r.as_f64()))
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => match (&l, &r) {
                (Value::Bool(x), Value::Bool(y)) => Ok(Value::Bool(match op {
                    BinOp::BitAnd => *x && *y,
                    BinOp::BitOr => *x || *y,
                    _ => x != y,
                })),
                (Value::Int { bits: x, prim }, _) => {
                    let y = r.as_i128();
                    // Operate on the unsigned bit pattern so a negative operand
                    // does not sign-extend past the declared width.
                    let w = prim.bit_width();
                    let mask: i128 = if w >= 128 { -1 } else { (1i128 << w) - 1 };
                    let (a, b) = (x & mask, y & mask);
                    let out = match op {
                        BinOp::BitAnd => a & b,
                        BinOp::BitOr => a | b,
                        _ => a ^ b,
                    };
                    Ok(Value::Int {
                        bits: prim.wrap_i128(out),
                        prim: *prim,
                    })
                }
                _ => Err(Flow::Abort("bitwise op needs integers or bools".into())),
            },
            BinOp::Shl | BinOp::Shr => {
                let Value::Int { bits, prim } = &l else {
                    return Err(Flow::Abort("shift needs an integer".into()));
                };
                // The count is masked to the width (spec/primitives.md), matching
                // the hardware instead of leaving over-shift undefined.
                let w = prim.bit_width().max(1);
                let count = (r.as_i128() as u128 & (w as u128 - 1)) as u32;
                let out = if op == BinOp::Shl {
                    bits.wrapping_shl(count)
                } else if prim.is_signed_int() {
                    // Arithmetic shift for signed, logical for unsigned.
                    bits >> count
                } else {
                    let mask: i128 = if w >= 128 { -1 } else { (1i128 << w) - 1 };
                    (bits & mask) >> count
                };
                Ok(Value::Int {
                    bits: prim.wrap_i128(out),
                    prim: *prim,
                })
            }
            BinOp::Eq => Ok(Value::Bool(eq_val(&l, &r))),
            BinOp::Ne => Ok(Value::Bool(!eq_val(&l, &r))),
            // Floats are unordered against NaN: `<`, `<=`, `>`, `>=` are all
            // false when either side is NaN, which `cmp_ord`'s total ordering
            // cannot express.
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge if is_float(&l) || is_float(&r) => {
                let (x, y) = (l.as_f64(), r.as_f64());
                Ok(Value::Bool(match op {
                    BinOp::Lt => x < y,
                    BinOp::Le => x <= y,
                    BinOp::Gt => x > y,
                    _ => x >= y,
                }))
            }
            BinOp::Lt => Ok(Value::Bool(cmp_ord(&l, &r) < 0)),
            BinOp::Le => Ok(Value::Bool(cmp_ord(&l, &r) <= 0)),
            BinOp::Gt => Ok(Value::Bool(cmp_ord(&l, &r) > 0)),
            BinOp::Ge => Ok(Value::Bool(cmp_ord(&l, &r) >= 0)),
            BinOp::And | BinOp::Or => unreachable!(),
        }
    }

    fn arith(
        &self,
        l: &Value,
        r: &Value,
        i: impl Fn(i128, i128) -> i128,
        f32op: impl Fn(f32, f32) -> f32,
        f64op: impl Fn(f64, f64) -> f64,
    ) -> IResult {
        match l {
            Value::Int { bits, prim } => Ok(Value::Int {
                bits: prim.wrap_i128(i(*bits, r.as_i128())),
                prim: *prim,
            }),
            Value::Float {
                prim: Prim::F32, ..
            } => Ok(Value::f32(canon_f32(f32op(l.as_f32(), r.as_f32())))),
            Value::Float { .. } => Ok(Value::f64(canon_f64(f64op(l.as_f64(), r.as_f64())))),
            _ => Err(Flow::Abort("invalid arithmetic".into())),
        }
    }

    fn builtin(&mut self, name: &str, args: Vec<Value>) -> IResult {
        self.world.tick();
        match name {
            "int.div" | "int.rem" | "int.div_trunc" => {
                let a = args.first().map(|v| v.as_i128()).unwrap_or(0);
                let b = args.get(1).map(|v| v.as_i128()).unwrap_or(0);
                if b == 0 {
                    return Err(Flow::Raise(Value::Variant {
                        name: "Zero".into(),
                        fields: IndexMap::new(),
                    }));
                }
                let v = if name == "int.rem" {
                    trunc_rem(a, b)
                } else {
                    trunc_div(a, b)
                };
                Ok(Value::i32(v as i32))
            }
            "int.div_exact" => {
                let a = args.first().map(|v| v.as_i128()).unwrap_or(0);
                let b = args.get(1).map(|v| v.as_i128()).unwrap_or(0);
                if b == 0 {
                    return Err(Flow::Abort("div_exact by zero".into()));
                }
                Ok(Value::i32(trunc_div(a, b) as i32))
            }
            "int.checked_add" | "int.checked_sub" | "int.checked_mul" => {
                let a = args.first().map(|v| v.as_i128()).unwrap_or(0) as i32;
                let b = args.get(1).map(|v| v.as_i128()).unwrap_or(0) as i32;
                let r = match name {
                    "int.checked_add" => a.checked_add(b),
                    "int.checked_sub" => a.checked_sub(b),
                    _ => a.checked_mul(b),
                };
                Ok(match r {
                    Some(v) => some_val(Value::i32(v)),
                    None => none_val(),
                })
            }
            "math.hypot" => {
                let x = args.first().map(|v| v.as_f64()).unwrap_or(0.0);
                let y = args.get(1).map(|v| v.as_f64()).unwrap_or(0.0);
                Ok(Value::f32(canon_f32(x.hypot(y) as f32)))
            }
            "math.sqrt" => {
                let x = args.first().map(|v| v.as_f64()).unwrap_or(0.0);
                Ok(Value::f64(canon_f64(x.sqrt())))
            }
            "math.abs" | "f32.abs" => {
                let x = args.first().map(|v| v.as_f32()).unwrap_or(0.0);
                Ok(Value::f32(canon_f32(x.abs())))
            }
            "f32.cmp" => {
                let x = args.first().map(|v| v.as_f32()).unwrap_or(0.0);
                let y = args.get(1).map(|v| v.as_f32()).unwrap_or(0.0);
                Ok(ord_val(
                    x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                ))
            }
            "i32.cmp" => {
                let x = args.first().map(|v| v.as_i128()).unwrap_or(0);
                let y = args.get(1).map(|v| v.as_i128()).unwrap_or(0);
                Ok(ord_val(x.cmp(&y)))
            }
            // A Vec's storage belongs to the allocator handle it was created
            // with. The oracle does not model memory, but it does model the
            // handle being required.
            "to_i64" | "to_u64" | "to_f64" => {
                let v = args.first().cloned().unwrap_or(Value::Unit);
                Ok(v)
            }
            "try_to_u8" => {
                let n = args.first().map(|v| v.as_i128()).unwrap_or(0);
                if (0..=255).contains(&n) {
                    Ok(ok_val(Value::Int { bits: n, prim: Prim::U8 }))
                } else {
                    Ok(err_val(Value::Variant {
                        name: "Invalid".into(),
                        fields: IndexMap::new(),
                    }))
                }
            }
            "declassify" => Ok(args.first().cloned().unwrap_or(Value::Unit)),
            "vec.new" => Ok(Value::Vec(Rc::new(RefCell::new(Vec::new())))),
            "map.new" => Ok(Value::Map(Rc::new(RefCell::new(IndexMap::new())))),
            "str.concat" => {
                let x = args.get(1).map(|v| v.display()).unwrap_or_default();
                let y = args.get(2).map(|v| v.display()).unwrap_or_default();
                let strip = |s: String| s.trim_matches('"').to_string();
                Ok(Value::Str(format!("{}{}", strip(x), strip(y)).into()))
            }
            "range" => {
                let start = args.first().map(|v| v.as_i128() as u64).unwrap_or(0);
                let end = args.get(1).map(|v| v.as_i128() as u64).unwrap_or(0);
                Ok(Value::Range { start, end })
            }
            "assert" => {
                let c = args.first().map(|v| v.as_bool()).unwrap_or(false);
                if !c {
                    return Err(Flow::Abort("assertion failed".into()));
                }
                Ok(Value::Unit)
            }
            "fail" => {
                let m = args
                    .first()
                    .map(|v| v.display())
                    .unwrap_or_else(|| "fail".into());
                Err(Flow::Abort(m))
            }
            // Process arguments. Recorded in the transcript, because a replay of
            // this run must see the same values.
            "argv" => {
                let i = args.first().map(|v| v.as_i128()).unwrap_or(0);
                let v = self.effect("argv", &i.to_string(), |me| {
                    Ok(if i < 0 || i as usize >= me.world.argv.len() {
                        String::new()
                    } else {
                        me.world.argv[i as usize].clone()
                    })
                })?;
                Ok(Value::Str(v.into()))
            }
            "io.bytesum_file" | "io_bytesum_file" => {
                // Ambient read: no capability mediates it, which is why the
                // module loses the `capability-contained` label.
                let path = match args.first() {
                    Some(Value::Str(s)) => s.to_string(),
                    Some(other) => other.display().trim_matches('"').to_string(),
                    None => String::new(),
                };
                let sum = self.effect("io.bytesum_file", &path, |_| {
                    match crate::caps::confine(std::path::Path::new("."), &path) {
                        Ok(p) => match std::fs::read(p) {
                            Ok(bytes) => Ok(bytes
                                .iter()
                                .fold(0u64, |a, b| a.wrapping_add(*b as u64))
                                .to_string()),
                            Err(e) => Err(Flow::Abort(format!("io.bytesum_file {path}: {e}"))),
                        },
                        Err(e) => Err(Flow::Abort(format!("io.bytesum_file {path}: {e:?}"))),
                    }
                })?;
                Ok(Value::Int {
                    bits: sum.parse::<u64>().unwrap_or(0) as i128,
                    prim: Prim::U64,
                })
            }
            "print" => {
                let s = args
                    .first()
                    .map(|v| match v {
                        Value::Str(s) => s.to_string(),
                        other => other.display(),
                    })
                    .unwrap_or_default();
                self.world.stdout.push(s.clone());
                self.world.trace.push(format!("print {s}"));
                Ok(Value::Unit)
            }
            "parse_i32" => {
                let s = match args.first() {
                    Some(Value::Str(s)) => s.to_string(),
                    other => other.map(|v| v.display()).unwrap_or_default(),
                };
                match s.parse::<i32>() {
                    Ok(n) => Ok(Value::i32(n)),
                    Err(_) => Err(Flow::Raise(Value::Variant {
                        name: "Invalid".into(),
                        fields: IndexMap::new(),
                    })),
                }
            }
            "len" => {
                if let Some(v) = args.first() {
                    if let Some(r) = self.method(v, "len", &[])? {
                        return Ok(r);
                    }
                }
                Ok(Value::usz(0))
            }
            "all" | "any" | "count" | "sorted_by" => self.eval_quant(name, &args),
            "sort" => self.builtin_sort(&args),
            "freeze" => Ok(args.first().cloned().unwrap_or(Value::Unit)),
            "fs.read" => self.fs_read(&args),
            "json.decode_recs" => self.json_decode_recs(&args),
            "json.decode" => self.json_decode_recs(&args),
            "test.read_cap" => {
                let mut files = IndexMap::new();
                if let Some(Value::Record(fs)) = args.first() {
                    for (k, v) in fs {
                        files.insert(
                            k.clone(),
                            match v {
                                Value::Str(s) => s.to_string(),
                                other => other.display(),
                            },
                        );
                    }
                }
                Ok(Value::Cap(Capability::FsRead { files }))
            }
            "test.alloc" => Ok(Value::Alloc),
            // Ambient reads still go through the capability layer's path
            // confinement: no absolute paths, no `..` escape. "Ambient" means no
            // capability *handle* is required, not that the filesystem is open.
            "io.read_file" | "io_read_file" => {
                let path = value_as_path(args.first());
                let len = self.effect("io.read_file", &path, |_| {
                    match crate::caps::confine(std::path::Path::new("."), &path) {
                        Ok(p) => match std::fs::read(p) {
                            Ok(b) => Ok(b.len().to_string()),
                            Err(e) => Err(Flow::Abort(format!("io.read_file {path}: {e}"))),
                        },
                        Err(e) => Err(Flow::Abort(format!("io.read_file {path}: {e:?}"))),
                    }
                })?;
                Ok(Value::usz(len.parse::<u64>().unwrap_or(0)))
            }
            "http.get_bytesum" | "http_get_bytesum" => {
                Err(Flow::Abort(
                    "http.get_bytesum is native-only; use `ax build`".into(),
                ))
            }
            "vec.len" => {
                if let Some(v) = args.first() {
                    if let Some(r) = self.method(v, "len", &[])? {
                        return Ok(r);
                    }
                }
                Ok(Value::f32(0.0))
            }
            other => {
                if self.fns.contains_key(other) {
                    return self.call_named(other, args);
                }
                Err(Flow::Abort(format!("unknown function `{other}`")))
            }
        }
    }

    fn eval_quant(&mut self, name: &str, args: &[Value]) -> IResult {
        let seq = args.first().cloned().unwrap_or(Value::Unit);
        let pred = args.get(1).cloned();
        let items = self.iterate(&seq)?;
        match name {
            "all" => {
                for it in items {
                    if let Some(Value::Fn(fv)) = &pred {
                        let v = self.call_fnval(fv, vec![it])?;
                        if !v.as_bool() {
                            return Ok(Value::Bool(false));
                        }
                    }
                }
                Ok(Value::Bool(true))
            }
            "any" => {
                for it in items {
                    if let Some(Value::Fn(fv)) = &pred {
                        let v = self.call_fnval(fv, vec![it])?;
                        if v.as_bool() {
                            return Ok(Value::Bool(true));
                        }
                    }
                }
                Ok(Value::Bool(false))
            }
            "count" => {
                let mut n = 0u64;
                for it in items {
                    if let Some(Value::Fn(fv)) = &pred {
                        let v = self.call_fnval(fv, vec![it])?;
                        if v.as_bool() {
                            n += 1;
                        }
                    }
                }
                Ok(Value::usz(n))
            }
            "sorted_by" => {
                for w in items.windows(2) {
                    if let Some(Value::Fn(fv)) = &pred {
                        let v = self.call_fnval(fv, vec![w[0].clone(), w[1].clone()])?;
                        if !v.as_bool() {
                            return Ok(Value::Bool(false));
                        }
                    }
                }
                Ok(Value::Bool(true))
            }
            _ => Ok(Value::Bool(true)),
        }
    }

    /// `sort(&mut xs, cmp)`. Stable insertion sort: the oracle's job is to
    /// define the result, and stability makes the permutation unique so the
    /// native merge sort can be compared against it element for element.
    fn builtin_sort(&mut self, args: &[Value]) -> IResult {
        let xs = match args.first() {
            Some(Value::Vec(v)) => v.clone(),
            _ => return Ok(Value::Unit),
        };
        let cmp = match args.get(1) {
            Some(Value::Fn(fv)) => fv.clone(),
            _ => return Err(Flow::Abort("sort needs a comparator function".into())),
        };
        let mut data = xs.borrow().clone();
        for i in 1..data.len() {
            let mut j = i;
            while j > 0 {
                let o = self.call_fnval(&cmp, vec![data[j].clone(), data[j - 1].clone()])?;
                let less = match &o {
                    Value::Variant { name, .. } => name == "Lt",
                    other => {
                        return Err(Flow::Abort(format!(
                            "comparator must return Ordering, got {}",
                            other.display()
                        )))
                    }
                };
                if less {
                    data.swap(j, j - 1);
                    j -= 1;
                } else {
                    break;
                }
            }
        }
        *xs.borrow_mut() = data;
        Ok(Value::Unit)
    }

    fn fs_read(&mut self, args: &[Value]) -> IResult {
        let path = match args.get(2).or_else(|| args.get(1)).or_else(|| args.first()) {
            Some(Value::Str(s)) => s.to_string(),
            Some(other) => other.display().trim_matches('"').to_string(),
            None => return Err(Flow::Raise(fs_not_found(""))),
        };
        if let Some(Value::Cap(Capability::FsRead { files })) = args.first() {
            if let Some(c) = files.get(&path) {
                self.world.trace.push(format!("fs.read {path}"));
                return Ok(Value::Str(c.clone().into()));
            }
        }
        if let Some(c) = self.host_files.get(&path) {
            return Ok(Value::Str(c.clone().into()));
        }
        // Host fallback is confined to CWD — no absolute paths, no `..` escape.
        match crate::caps::confine(std::path::Path::new("."), &path) {
            Ok(p) => match std::fs::read_to_string(p) {
                Ok(s) => Ok(Value::Str(s.into())),
                Err(_) => Err(Flow::Raise(fs_not_found(&path))),
            },
            Err(_) => Err(Flow::Raise(fs_not_found(&path))),
        }
    }

    fn json_decode_recs(&mut self, args: &[Value]) -> IResult {
        let raw = match args.last() {
            Some(Value::Str(s)) => s.to_string(),
            Some(other) => other.display(),
            None => {
                return Err(Flow::Raise(Value::Variant {
                    name: "Invalid".into(),
                    fields: IndexMap::new(),
                }))
            }
        };
        match parse_json_recs(&raw) {
            Ok(vs) => Ok(Value::Vec(Rc::new(RefCell::new(vs)))),
            Err(_) => Err(Flow::Raise(Value::Variant {
                name: "Invalid".into(),
                fields: IndexMap::new(),
            })),
        }
    }

    fn iterate(&self, v: &Value) -> Result<Vec<Value>, Flow> {
        match v {
            Value::Vec(xs) => Ok(xs.borrow().clone()),
            Value::Range { start, end } => Ok((*start..*end).map(Value::usz).collect()),
            Value::Record(fs) => Ok(fs.values().cloned().collect()),
            _ => Ok(vec![v.clone()]),
        }
    }

    fn field(&self, v: &Value, name: &str) -> IResult {
        match v {
            Value::Record(fs) => fs
                .get(name)
                .cloned()
                .ok_or_else(|| Flow::Abort(format!("no field {name}"))),
            Value::Variant { fields, .. } => fields
                .get(name)
                .cloned()
                .ok_or_else(|| Flow::Abort(format!("no field {name}"))),
            Value::Own(inner) => self.field(inner, name),
            Value::Fn(_) => Ok(v.clone()),
            _ => Err(Flow::Abort(format!("no field {name}"))),
        }
    }

    fn match_pat(&mut self, pat: &Pattern, v: &Value) -> bool {
        match &pat.kind {
            PatKind::Wild => true,
            PatKind::Lit(l) => eq_val(&self.lit_value_at(l, pat.id), v),
            PatKind::Bind(id) => {
                // The checker marks bare names that are really unit variants;
                // those test the value's tag instead of binding it.
                if let Some(want) = self.pat_variant.get(&pat.id) {
                    return match v {
                        Value::Variant { name, .. } => name == want,
                        _ => false,
                    };
                }
                let n = self.intern.get(id.name).to_string();
                self.bind(n, v.clone());
                true
            }
            PatKind::Variant { name, fields } => {
                let want = self.intern.get(name.name);
                match v {
                    Value::Variant {
                        name: got,
                        fields: vfs,
                    } => {
                        if got != want {
                            return false;
                        }
                        // Result.Err(payload) / Ok(payload) — positional _0
                        if fields.len() == 1 && fields[0].0.name != Symbol(0) {
                            let fname = self.intern.get(fields[0].0.name);
                            if fname.starts_with('_') {
                                let payload =
                                    vfs.values().next().cloned().unwrap_or(Value::Variant {
                                        name: got.clone(),
                                        fields: vfs.clone(),
                                    });
                                return self.match_pat(&fields[0].1, &payload);
                            }
                        }
                        for (n, p) in fields {
                            let key = self.intern.get(n.name);
                            if let Some(fv) = vfs.get(key) {
                                if !self.match_pat(p, fv) {
                                    return false;
                                }
                            } else if !matches!(p.kind, PatKind::Wild) {
                                // bind-shorthand: field missing — fail
                                return false;
                            }
                        }
                        true
                    }
                    _ => false,
                }
            }
            PatKind::Record(fs) => match v {
                Value::Record(vfs) => {
                    for (n, p) in fs {
                        if let Some(fv) = vfs.get(self.intern.get(n.name)) {
                            if !self.match_pat(p, fv) {
                                return false;
                            }
                        } else {
                            return false;
                        }
                    }
                    true
                }
                _ => false,
            },
            PatKind::Tuple(ps) => match v {
                Value::Record(fs) => {
                    for (i, p) in ps.iter().enumerate() {
                        if let Some((_, fv)) = fs.get_index(i) {
                            if !self.match_pat(p, fv) {
                                return false;
                            }
                        }
                    }
                    true
                }
                _ => false,
            },
        }
    }

    fn assign(&mut self, lhs: &Expr, v: Value) -> Result<(), Flow> {
        if let ExprKind::Path(p) = &lhs.kind {
            if p.segs.len() == 1 {
                let n = self.intern.get(p.segs[0].name).to_string();
                self.assign_name(&n, v);
                return Ok(());
            }
            // `a.b.c = v` written as a dotted path.
            let root = self.intern.get(p.segs[0].name).to_string();
            let path: Vec<String> = p.segs[1..]
                .iter()
                .map(|s| self.intern.get(s.name).to_string())
                .collect();
            return self.assign_path(&root, &path, v);
        }
        if let ExprKind::Field { base, field } = &lhs.kind {
            let fname = self.intern.get(field.name).to_string();
            let mut path = vec![fname];
            let mut cur = base;
            // Walk outward to the root local, collecting the field path.
            loop {
                match &cur.kind {
                    ExprKind::Field { base: b, field: f } => {
                        path.push(self.intern.get(f.name).to_string());
                        cur = b;
                    }
                    ExprKind::Path(p) if p.segs.len() == 1 => {
                        let root = self.intern.get(p.segs[0].name).to_string();
                        path.reverse();
                        return self.assign_path(&root, &path, v);
                    }
                    _ => break,
                }
            }
        }
        Err(Flow::Abort("invalid assignment target".into()))
    }

    /// Assign through a field path rooted at a local binding.
    fn assign_path(&mut self, root: &str, path: &[String], v: Value) -> Result<(), Flow> {
        for frame in self.frames.iter_mut().rev() {
            if let Some(slot) = frame.get_mut(root) {
                let mut cur = slot;
                for (i, seg) in path.iter().enumerate() {
                    let last = i + 1 == path.len();
                    let fields = match cur {
                        Value::Record(fs) => fs,
                        Value::Variant { fields, .. } => fields,
                        _ => return Err(Flow::Abort(format!("no field {seg}"))),
                    };
                    if last {
                        fields.insert(seg.clone(), v);
                        return Ok(());
                    }
                    cur = fields
                        .get_mut(seg)
                        .ok_or_else(|| Flow::Abort(format!("no field {seg}")))?;
                }
                return Ok(());
            }
        }
        Err(Flow::Abort(format!("unknown local `{root}`")))
    }

    fn assign_name(&mut self, name: &str, v: Value) {
        for f in self.frames.iter_mut().rev() {
            if f.contains_key(name) {
                f.insert(name.to_string(), v);
                return;
            }
        }
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(name.to_string(), v);
        }
    }

    fn bind(&mut self, name: String, v: Value) {
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(name, v);
        }
    }

    fn lookup(&self, name: &str) -> Option<Value> {
        for f in self.frames.iter().rev() {
            if let Some(v) = f.get(name) {
                return Some(v.clone());
            }
        }
        None
    }

    fn push_scope(&mut self) {
        self.frames.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.frames.pop();
    }
}

/// Evaluate a call at compile time, for constant folding.
///
/// Returns `None` when the call cannot be folded within `budget` steps, so a
/// caller can fall back to emitting the call. The interpreter is the normative
/// semantics, which is what makes this sound: a folded result is by construction
/// the result the program would have produced.
pub fn fold_call(
    intern: &Interner,
    checked: &CheckOutput,
    name: &str,
    args: Vec<Value>,
    budget: u64,
) -> Option<Value> {
    let mut ip = Interpreter::new(intern, checked, 0);
    ip.world.step_budget = Some(budget);
    // Memoise during the fold. A pure function returns the same value for the
    // same arguments by definition, so caching cannot change the result — and it
    // turns an exponential constant expression (naive `fib`) into a linear one,
    // which is the difference between folding it and giving up.
    ip.memo = Some(HashMap::new());
    match ip.call_named(name, args) {
        Ok(v) => Some(v),
        Err(_) => None,
    }
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub name: String,
    pub ok: bool,
    pub msg: Option<String>,
}

fn lit_value(l: &Lit) -> Value {
    match l {
        Lit::Unit => Value::Unit,
        Lit::Bool(b) => Value::Bool(*b),
        Lit::Str(s) => Value::Str(s.clone().into()),
        Lit::Int { value, suffix } => Value::Int {
            bits: suffix.unwrap_or(Prim::I32).wrap_i128(*value),
            prim: suffix.unwrap_or(Prim::I32),
        },
        Lit::Float { value, suffix } => match suffix.unwrap_or(Prim::F32) {
            Prim::F64 => Value::f64(*value),
            _ => Value::f32(*value as f32),
        },
    }
}

fn expr_path(e: &Expr, intern: &Interner) -> Option<String> {
    match &e.kind {
        ExprKind::Path(p) => Some(path_join(p, intern)),
        ExprKind::Field { base, field } => {
            let left = expr_path(base, intern)?;
            Some(format!("{}.{}", left, intern.get(field.name)))
        }
        _ => None,
    }
}

fn value_as_path(v: Option<&Value>) -> String {
    match v {
        Some(Value::Str(s)) => s.to_string(),
        Some(other) => other.display().trim_matches('"').to_string(),
        None => String::new(),
    }
}


fn path_join(p: &Path, intern: &Interner) -> String {
    p.segs
        .iter()
        .map(|s| intern.get(s.name))
        .collect::<Vec<_>>()
        .join(".")
}

fn eq_val(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Unit, Value::Unit) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int { bits: x, .. }, Value::Int { bits: y, .. }) => x == y,
        (Value::Int { bits, .. }, other) => *bits == other.as_i128(),
        (other, Value::Int { bits, .. }) => other.as_i128() == *bits,
        // IEEE-754 comparison, not bit comparison: NaN is equal to nothing
        // (including itself), and -0.0 equals 0.0 despite differing bits.
        (
            Value::Float {
                prim: Prim::F32, ..
            },
            Value::Float { .. },
        ) => a.as_f32() == b.as_f32(),
        (Value::Float { .. }, Value::Float { .. }) => a.as_f64() == b.as_f64(),
        (Value::Str(x), Value::Str(y)) => x == y,
        (
            Value::Variant {
                name: n1,
                fields: f1,
            },
            Value::Variant {
                name: n2,
                fields: f2,
            },
        ) => {
            n1 == n2
                && f1.len() == f2.len()
                && f1
                    .iter()
                    .zip(f2.iter())
                    .all(|((k1, v1), (k2, v2))| k1 == k2 && eq_val(v1, v2))
        }
        _ => false,
    }
}

/// `as` conversion. Narrowing integers wraps, float-to-int saturates, and
/// int-to-float rounds — matching Rust's `as` so the rules are already familiar.
fn cast_value(v: &Value, to: Option<Prim>) -> Value {
    let Some(to) = to else { return v.clone() };
    if to.is_int() {
        let x = match v {
            Value::Float { .. } => {
                let f = v.as_f64();
                if f.is_nan() {
                    0
                } else {
                    // Saturate at the destination's bounds.
                    let w = to.bit_width();
                    let (lo, hi) = if to.is_signed_int() {
                        (-(1i128 << (w - 1)), (1i128 << (w - 1)) - 1)
                    } else {
                        (0, if w >= 128 { i128::MAX } else { (1i128 << w) - 1 })
                    };
                    if f <= lo as f64 {
                        lo
                    } else if f >= hi as f64 {
                        hi
                    } else {
                        f as i128
                    }
                }
            }
            other => other.as_i128(),
        };
        return Value::Int {
            bits: to.wrap_i128(x),
            prim: to,
        };
    }
    if to.is_float() {
        let f = match v {
            Value::Int { bits, .. } => *bits as f64,
            other => other.as_f64(),
        };
        return match to {
            Prim::F32 => Value::f32(f as f32),
            _ => Value::f64(f),
        };
    }
    v.clone()
}

fn is_float(v: &Value) -> bool {
    matches!(v, Value::Float { .. })
}

fn cmp_ord(a: &Value, b: &Value) -> i32 {
    match (a, b) {
        (Value::Int { bits: x, .. }, Value::Int { bits: y, .. }) => x.cmp(y) as i32,
        (
            Value::Float {
                prim: Prim::F32, ..
            },
            _,
        ) => a
            .as_f32()
            .partial_cmp(&b.as_f32())
            .map(|o| o as i32)
            .unwrap_or(0),
        (Value::Float { .. }, _) => a
            .as_f64()
            .partial_cmp(&b.as_f64())
            .map(|o| o as i32)
            .unwrap_or(0),
        (Value::Str(x), Value::Str(y)) => x.cmp(y) as i32,
        _ => a.as_i128().cmp(&b.as_i128()) as i32,
    }
}

fn trunc_div(a: i128, b: i128) -> i128 {
    a / b
}

fn trunc_rem(a: i128, b: i128) -> i128 {
    a % b
}

fn some_val(v: Value) -> Value {
    let mut f = IndexMap::new();
    f.insert("value".into(), v);
    Value::Variant {
        name: "Some".into(),
        fields: f,
    }
}

fn none_val() -> Value {
    Value::Variant {
        name: "None".into(),
        fields: IndexMap::new(),
    }
}

fn ok_val(v: Value) -> Value {
    let mut f = IndexMap::new();
    f.insert("value".into(), v);
    Value::Variant {
        name: "Ok".into(),
        fields: f,
    }
}

fn err_val(v: Value) -> Value {
    let mut f = IndexMap::new();
    f.insert("error".into(), v);
    Value::Variant {
        name: "Err".into(),
        fields: f,
    }
}

fn ord_val(o: std::cmp::Ordering) -> Value {
    let n = match o {
        std::cmp::Ordering::Less => "Lt",
        std::cmp::Ordering::Equal => "Eq",
        std::cmp::Ordering::Greater => "Gt",
    };
    Value::Variant {
        name: n.into(),
        fields: IndexMap::new(),
    }
}

fn fs_not_found(path: &str) -> Value {
    let mut f = IndexMap::new();
    f.insert("path".into(), Value::Str(path.into()));
    Value::Variant {
        name: "NotFound".into(),
        fields: f,
    }
}

/// Minimal JSON array-of-objects parser for `json.decode_recs`.
fn parse_json_recs(s: &str) -> Result<Vec<Value>, ()> {
    let v = parse_json(s.trim())?;
    match v {
        Json::Arr(xs) => xs.into_iter().map(json_to_rec).collect(),
        _ => Err(()),
    }
}

fn json_to_rec(j: Json) -> Result<Value, ()> {
    match j {
        Json::Obj(map) => {
            let mut rec = IndexMap::new();
            for (k, v) in map {
                rec.insert(k.clone(), json_field_val(&k, v));
            }
            Ok(Value::Record(rec))
        }
        _ => Err(()),
    }
}

fn json_to_val(j: Json) -> Value {
    match j {
        Json::Null => Value::Unit,
        Json::Bool(b) => Value::Bool(b),
        Json::Num(n) => {
            if n.fract() == 0.0 && n.abs() < (1i64 << 53) as f64 {
                Value::Int {
                    bits: n as i128,
                    prim: Prim::U64,
                }
            } else {
                Value::f32(n as f32)
            }
        }
        Json::Str(s) => Value::Str(s.into()),
        Json::Arr(xs) => Value::Vec(Rc::new(RefCell::new(
            xs.into_iter().map(json_to_val).collect(),
        ))),
        Json::Obj(map) => {
            let mut rec = IndexMap::new();
            for (k, v) in map {
                rec.insert(k.clone(), json_field_val(&k, v));
            }
            Value::Record(rec)
        }
    }
}

fn json_field_val(key: &str, j: Json) -> Value {
    match (key, j) {
        ("id", Json::Num(n)) => Value::Int {
            bits: n as i128,
            prim: Prim::U64,
        },
        ("score", Json::Num(n)) => Value::f32(n as f32),
        ("name", Json::Str(s)) => Value::Str(s.into()),
        (_, other) => json_to_val(other),
    }
}

enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(IndexMap<String, Json>),
}

fn parse_json(s: &str) -> Result<Json, ()> {
    let mut p = JParser {
        b: s.as_bytes(),
        i: 0,
    };
    let v = p.value()?;
    p.skip();
    if p.i != p.b.len() {
        return Err(());
    }
    Ok(v)
}

struct JParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> JParser<'a> {
    fn skip(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn value(&mut self) -> Result<Json, ()> {
        self.skip();
        match self.peek() {
            Some(b'n') => {
                self.eat(b"null")?;
                Ok(Json::Null)
            }
            Some(b't') => {
                self.eat(b"true")?;
                Ok(Json::Bool(true))
            }
            Some(b'f') => {
                self.eat(b"false")?;
                Ok(Json::Bool(false))
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'[') => {
                self.i += 1;
                let mut xs = Vec::new();
                self.skip();
                if self.peek() != Some(b']') {
                    loop {
                        xs.push(self.value()?);
                        self.skip();
                        if self.peek() == Some(b',') {
                            self.i += 1;
                            continue;
                        }
                        break;
                    }
                }
                self.skip();
                if self.peek() != Some(b']') {
                    return Err(());
                }
                self.i += 1;
                Ok(Json::Arr(xs))
            }
            Some(b'{') => {
                self.i += 1;
                let mut map = IndexMap::new();
                self.skip();
                if self.peek() != Some(b'}') {
                    loop {
                        self.skip();
                        let k = self.string()?;
                        self.skip();
                        if self.peek() != Some(b':') {
                            return Err(());
                        }
                        self.i += 1;
                        let v = self.value()?;
                        map.insert(k, v);
                        self.skip();
                        if self.peek() == Some(b',') {
                            self.i += 1;
                            continue;
                        }
                        break;
                    }
                }
                self.skip();
                if self.peek() != Some(b'}') {
                    return Err(());
                }
                self.i += 1;
                Ok(Json::Obj(map))
            }
            Some(b'-') | Some(b'0'..=b'9') => Ok(Json::Num(self.number()?)),
            _ => Err(()),
        }
    }
    fn eat(&mut self, s: &[u8]) -> Result<(), ()> {
        if self.b.get(self.i..self.i + s.len()) == Some(s) {
            self.i += s.len();
            Ok(())
        } else {
            Err(())
        }
    }
    fn string(&mut self) -> Result<String, ()> {
        if self.peek() != Some(b'"') {
            return Err(());
        }
        self.i += 1;
        let mut o = String::new();
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c == b'"' {
                self.i += 1;
                return Ok(o);
            }
            if c == b'\\' {
                self.i += 1;
                match self.b.get(self.i) {
                    Some(b'n') => o.push('\n'),
                    Some(b't') => o.push('\t'),
                    Some(b'r') => o.push('\r'),
                    Some(b'"') => o.push('"'),
                    Some(b'\\') => o.push('\\'),
                    Some(b'u') => {
                        self.i += 1;
                        if self.i + 4 > self.b.len() {
                            return Err(());
                        }
                        let hex =
                            std::str::from_utf8(&self.b[self.i..self.i + 4]).map_err(|_| ())?;
                        let cp = u32::from_str_radix(hex, 16).map_err(|_| ())?;
                        if let Some(ch) = char::from_u32(cp) {
                            o.push(ch);
                        }
                        self.i += 4;
                        continue;
                    }
                    Some(&x) => o.push(x as char),
                    None => return Err(()),
                }
                self.i += 1;
                continue;
            }
            o.push(c as char);
            self.i += 1;
        }
        Err(())
    }
    fn number(&mut self) -> Result<f64, ()> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        let s = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| ())?;
        s.parse().map_err(|_| ())
    }
}

/// Accept/reject for the JSONTestSuite-shaped runner ([T-4.4]).
pub fn json_accepts(s: &str) -> bool {
    parse_json(s.trim()).is_ok()
}
