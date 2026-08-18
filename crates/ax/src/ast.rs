//! Concrete AST. One AST, three surface frontends lower to it.

use crate::intern::Symbol;
use crate::span::Span;
use crate::types::Prim;

#[derive(Clone, Debug)]
pub struct File {
    pub module: Path,
    pub exports: Vec<Ident>,
    pub uses: Vec<UseDecl>,
    pub decls: Vec<Decl>,
    pub span: Span,
    /// Number of ids [`renumber`] assigned. The checker sizes its type table to
    /// this, so the table is total over the AST even for nodes no pass visits.
    pub node_count: usize,
}

#[derive(Clone, Debug)]
pub struct UseDecl {
    pub path: Path,
    pub alias: Option<Ident>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Ident {
    pub name: Symbol,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Path {
    pub segs: Vec<Ident>,
    pub span: Span,
}

impl Path {
    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct Decl {
    pub meta: Vec<Meta>,
    pub kind: DeclKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Meta {
    pub key: Ident,
    pub value: Option<MetaValue>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum MetaValue {
    String(String),
    Ident(Ident),
    Int(i64),
}

#[derive(Clone, Debug)]
pub enum DeclKind {
    Fn(FnDecl),
    Type(TypeDecl),
    Dict(DictDecl),
    Test(TestDecl),
    ContractFn(FnDecl),
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub name: Ident,
    pub generics: Vec<GParam>,
    pub params: Vec<Param>,
    pub ret: TypeExpr,
    pub effects: EffectRow,
    pub contracts: Vec<Contract>,
    pub body: Expr,
}

#[derive(Clone, Debug)]
pub struct GParam {
    pub name: Ident,
    pub bound: Option<TypeExpr>,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: Ident,
    pub ty: TypeExpr,
    pub default_dict: bool,
}

#[derive(Clone, Debug)]
pub struct TypeDecl {
    pub name: Ident,
    pub generics: Vec<GParam>,
    pub body: TypeBody,
    pub injections: Vec<Injection>,
}

#[derive(Clone, Debug)]
pub enum TypeBody {
    Alias(TypeExpr),
    Record(Vec<Field>),
    Variants(Vec<Variant>),
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: Ident,
    pub ty: TypeExpr,
}

#[derive(Clone, Debug)]
pub struct Variant {
    pub name: Ident,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug)]
pub struct Injection {
    pub from: TypeExpr,
    pub into_variant: Ident,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct DictDecl {
    pub name: Ident,
    pub for_ty: TypeExpr,
    pub fields: Vec<(Ident, Expr)>,
}

#[derive(Clone, Debug)]
pub struct TestDecl {
    pub name: String,
    pub body: Expr,
}

#[derive(Clone, Debug, Default)]
pub struct EffectRow {
    pub items: Vec<Effect>,
    pub span: Span,
    /// True when the source wrote no `!{…}` at all. An omitted row is not a
    /// claim that the function is effect-free: `diverge` is reconstructible
    /// from the body (`while` / `loop`), the same way a terse module header
    /// is reconstructible from the file stem. An explicit `!{}` still means
    /// "this terminates" and is checked.
    pub omitted: bool,
}

#[derive(Clone, Debug)]
pub struct Effect {
    pub kind: EffectKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum EffectKind {
    Err(TypeExpr),
    Io(Ident),
    Alloc(Ident),
    Susp,
    Diverge,
    Race,
    Nondet,
    Abort,
}

#[derive(Clone, Debug)]
pub struct Contract {
    pub kind: ContractKind,
    pub expr: Expr,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContractKind {
    Pre,
    Post,
    Inv,
}

#[derive(Clone, Debug)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TypeExprKind {
    Prim(Prim),
    Named {
        path: Path,
        args: Vec<TypeExpr>,
    },
    Ref {
        region: Ident,
        mutable: bool,
        inner: Box<TypeExpr>,
    },
    Own(Box<TypeExpr>),
    /// Lattice annotation: data that crossed an IO boundary (§4.4).
    Untrusted(Box<TypeExpr>),
    /// Lattice annotation: secret that cannot be logged / formatted / FFI'd.
    Secret(Box<TypeExpr>),
    Fn {
        params: Vec<TypeExpr>,
        ret: Box<TypeExpr>,
        effects: EffectRow,
    },
    Tuple(Vec<TypeExpr>),
    Hole,
}

/// Stable per-node identity, assigned by [`renumber`] after parsing.
///
/// One id space covers `Expr` and `Pattern`. The checker publishes types
/// indexed by this id; every backend reads types from that table instead of
/// re-deriving them from syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    /// Pre-numbering placeholder. Never appears after [`renumber`].
    pub const NONE: NodeId = NodeId(u32::MAX);

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub id: NodeId,
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Lit(Lit),
    Path(Path),
    Hole,
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Field {
        base: Box<Expr>,
        field: Ident,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Block {
        stmts: Vec<Stmt>,
        tail: Option<Box<Expr>>,
    },
    If {
        cond: Box<Expr>,
        then_b: Box<Expr>,
        else_b: Option<Box<Expr>>,
    },
    Match {
        scrut: Box<Expr>,
        arms: Vec<Arm>,
    },
    For {
        pat: Pattern,
        iter: Box<Expr>,
        body: Box<Expr>,
    },
    Loop {
        body: Box<Expr>,
    },
    While {
        cond: Box<Expr>,
        body: Box<Expr>,
    },
    /// `break` / `continue`, valid only inside a loop. No value: a loop is a
    /// statement form here, so there is nothing for `break` to carry.
    Break,
    Continue,
    /// `expr as T` — the only numeric conversion. There is no implicit one.
    Cast {
        expr: Box<Expr>,
        ty: TypeExpr,
    },
    Let(Box<LetStmt>),
    Lambda {
        params: Vec<Param>,
        ret: Option<TypeExpr>,
        body: Box<Expr>,
    },
    Record(Vec<(Ident, Expr)>),
    Variant {
        name: Ident,
        fields: Vec<(Ident, Expr)>,
    },
    Return(Option<Box<Expr>>),
    Raise(Box<Expr>),
    Catch {
        expr: Box<Expr>,
        arms: Vec<Arm>,
    },
    Attempt(Box<Expr>),
    /// Postfix `?` — Rust `Result` propagation (v0.3). Distinct from a
    /// primary-position hole.
    Try(Box<Expr>),
    /// `f"hello {name}"` interpolation.
    Interpolate {
        parts: Vec<InterpPart>,
    },
    Region {
        name: Ident,
        body: Box<Expr>,
    },
    Par {
        bindings: Vec<LetStmt>,
    },
    Assign {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
}

#[derive(Clone, Debug)]
pub enum InterpPart {
    Lit(String),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    Let(LetStmt),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct LetStmt {
    pub mutable: bool,
    pub pat: Pattern,
    pub ty: Option<TypeExpr>,
    pub init: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Arm {
    pub pat: Pattern,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub id: NodeId,
    pub kind: PatKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum PatKind {
    Wild,
    Lit(Lit),
    Bind(Ident),
    Variant {
        name: Ident,
        fields: Vec<(Ident, Pattern)>,
    },
    Record(Vec<(Ident, Pattern)>),
    Tuple(Vec<Pattern>),
}

#[derive(Clone, Debug)]
pub enum Lit {
    Int { value: i128, suffix: Option<Prim> },
    Float { value: f64, suffix: Option<Prim> },
    Bool(bool),
    Str(String),
    Unit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Not,
    /// Bitwise complement, `~x`.
    BitNot,
    Neg,
    Ref,
    RefMut,
    Deref,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    /// Left shift. The count is masked to the operand width.
    Shl,
    /// Right shift: arithmetic for signed operands, logical for unsigned.
    Shr,
}

/// Assign every `Expr` and `Pattern` in the file a unique [`NodeId`].
///
/// Runs once per file after parsing (all three surfaces), before checking.
/// Returns the id count, which is the required width of the checker's type
/// tables. Ids are dense and assigned in pre-order.
pub fn renumber(file: &mut File) -> usize {
    let mut n = 0u32;
    // Assigned below; recorded on the file so consumers can size side tables.
    for d in &mut file.decls {
        match &mut d.kind {
            DeclKind::Fn(f) | DeclKind::ContractFn(f) => {
                for c in &mut f.contracts {
                    number_expr(&mut c.expr, &mut n);
                }
                number_expr(&mut f.body, &mut n);
            }
            DeclKind::Dict(dd) => {
                for (_, e) in &mut dd.fields {
                    number_expr(e, &mut n);
                }
            }
            DeclKind::Test(t) => number_expr(&mut t.body, &mut n),
            DeclKind::Type(_) => {}
        }
    }
    file.node_count = n as usize;
    n as usize
}

fn fresh(n: &mut u32) -> NodeId {
    let id = NodeId(*n);
    *n += 1;
    id
}

fn number_pat(p: &mut Pattern, n: &mut u32) {
    p.id = fresh(n);
    match &mut p.kind {
        PatKind::Wild | PatKind::Lit(_) | PatKind::Bind(_) => {}
        PatKind::Variant { fields, .. } | PatKind::Record(fields) => {
            for (_, sub) in fields {
                number_pat(sub, n);
            }
        }
        PatKind::Tuple(ps) => {
            for sub in ps {
                number_pat(sub, n);
            }
        }
    }
}

fn number_let(l: &mut LetStmt, n: &mut u32) {
    number_pat(&mut l.pat, n);
    number_expr(&mut l.init, n);
}

fn number_expr(e: &mut Expr, n: &mut u32) {
    e.id = fresh(n);
    match &mut e.kind {
        ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Hole => {}
        ExprKind::Call { callee, args } => {
            number_expr(callee, n);
            for a in args {
                number_expr(a, n);
            }
        }
        ExprKind::Field { base, .. } => number_expr(base, n),
        ExprKind::Index { base, index } => {
            number_expr(base, n);
            number_expr(index, n);
        }
        ExprKind::Unary { expr, .. } => number_expr(expr, n),
        ExprKind::Binary { lhs, rhs, .. } => {
            number_expr(lhs, n);
            number_expr(rhs, n);
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match &mut s.kind {
                    StmtKind::Let(l) => number_let(l, n),
                    StmtKind::Expr(ex) => number_expr(ex, n),
                }
            }
            if let Some(t) = tail {
                number_expr(t, n);
            }
        }
        ExprKind::If {
            cond,
            then_b,
            else_b,
        } => {
            number_expr(cond, n);
            number_expr(then_b, n);
            if let Some(el) = else_b {
                number_expr(el, n);
            }
        }
        ExprKind::Match { scrut, arms } | ExprKind::Catch { expr: scrut, arms } => {
            number_expr(scrut, n);
            for a in arms {
                number_pat(&mut a.pat, n);
                number_expr(&mut a.body, n);
            }
        }
        ExprKind::For { pat, iter, body } => {
            number_expr(iter, n);
            number_pat(pat, n);
            number_expr(body, n);
        }
        ExprKind::Loop { body } => number_expr(body, n),
        ExprKind::While { cond, body } => {
            number_expr(cond, n);
            number_expr(body, n);
        }
        ExprKind::Break | ExprKind::Continue => {}
        ExprKind::Cast { expr, .. } => number_expr(expr, n),
        ExprKind::Let(l) => number_let(l, n),
        ExprKind::Lambda { body, .. } => number_expr(body, n),
        ExprKind::Record(fs) | ExprKind::Variant { fields: fs, .. } => {
            for (_, ex) in fs {
                number_expr(ex, n);
            }
        }
        ExprKind::Return(inner) => {
            if let Some(i) = inner {
                number_expr(i, n);
            }
        }
        ExprKind::Raise(inner) | ExprKind::Attempt(inner) | ExprKind::Try(inner) => {
            number_expr(inner, n)
        }
        ExprKind::Interpolate { parts } => {
            for p in parts {
                if let InterpPart::Expr(ex) = p {
                    number_expr(ex, n);
                }
            }
        }
        ExprKind::Region { body, .. } => number_expr(body, n),
        ExprKind::Par { bindings } => {
            for l in bindings {
                number_let(l, n);
            }
        }
        ExprKind::Assign { lhs, rhs } => {
            number_expr(lhs, n);
            number_expr(rhs, n);
        }
    }
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
        }
    }
}
