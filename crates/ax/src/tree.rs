//! Prefix-tree surface (s-expression). A file that opens with `(` is this
//! surface. The default language is Ax in `frontend.rs`.
//!
//! This is Ax, not a rewrite of the Rust-shaped parser. There is no operator
//! precedence, no infix, no accept-and-elide, and exactly one way to write
//! each construct. The tree *is* the AST; this module is a 1:1 mapping.
//!
//! Why a tree, from first principles, for a program that writes programs:
//!
//! - An LLM samples tokens. Infix makes `a+b*c` a silent-wrongness class;
//!   a prefix list cannot encode the wrong grouping without writing it.
//! - Humans need sugar (`v.x`, `?`, `a + b`) because they read. An agent
//!   reads the protocol (`ax types`, `ax hole`, `ax ir`) and writes once.
//! - Multiple surfaces train format oscillation. One printer is the inverse
//!   of one parser, so `fmt` is a bijection and patches are tree edits.
//! - Constrained decoding (GBNF) is trivial over a single list grammar.
//!
//! Rust-shaped text exists only as an internal expanded representation for
//! generated code and legacy fixtures.
//!
//! Grammar (informal):
//!
//! ```text
//! file     ::= (module path form*) | form*
//! form     ::= (export ident*) | (use path [as ident]) | decl
//! decl     ::= (fn …) | (type …) | (dict …) | (test …) | (contract …)
//! expr     ::= atom | (head form*)
//! head     ::= + - * / % == != < <= > >= && || & | ^ << >>
//!            | not bnot neg ref refmut deref
//!            | let set block if match arm for while loop
//!            | return raise catch attempt try region par
//!            | rec var field index as interp fn
//!            | <path>          ; otherwise a call
//! ```

use crate::ast::*;
use crate::diag::Diagnostic;
use crate::intern::Interner;
use crate::span::{FileId, Span};
use crate::types::Prim;

// ---------------------------------------------------------------------------
// tokens
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Tok {
    kind: TokKind,
    span: Span,
}

#[derive(Clone, Debug)]
enum TokKind {
    LParen,
    RParen,
    String(String),
    Atom(String),
    Eof,
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    file: FileId,
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str, file: FileId) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            file,
            pos: 0,
        }
    }

    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(self.file, start as u32, end as u32)
    }

    fn skip(&mut self) {
        loop {
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            if self.pos >= self.bytes.len() {
                return;
            }
            // `;` and `//` line comments. `;` is the tree-native form;
            // `//` is accepted so a model that still emits it is not stuck.
            if self.bytes[self.pos] == b';'
                || (self.bytes[self.pos] == b'/'
                    && self.pos + 1 < self.bytes.len()
                    && self.bytes[self.pos + 1] == b'/')
            {
                while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            if self.bytes[self.pos] == b'/'
                && self.pos + 1 < self.bytes.len()
                && self.bytes[self.pos + 1] == b'*'
            {
                self.pos += 2;
                while self.pos + 1 < self.bytes.len()
                    && !(self.bytes[self.pos] == b'*' && self.bytes[self.pos + 1] == b'/')
                {
                    self.pos += 1;
                }
                if self.pos + 1 < self.bytes.len() {
                    self.pos += 2;
                }
                continue;
            }
            return;
        }
    }

    fn next(&mut self) -> Result<Tok, Diagnostic> {
        self.skip();
        let start = self.pos;
        if self.pos >= self.bytes.len() {
            return Ok(Tok {
                kind: TokKind::Eof,
                span: self.span(start, start),
            });
        }
        match self.bytes[self.pos] {
            b'(' => {
                self.pos += 1;
                Ok(Tok {
                    kind: TokKind::LParen,
                    span: self.span(start, self.pos),
                })
            }
            b')' => {
                self.pos += 1;
                Ok(Tok {
                    kind: TokKind::RParen,
                    span: self.span(start, self.pos),
                })
            }
            b'"' => {
                self.pos += 1;
                let mut s = String::new();
                while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
                    if self.bytes[self.pos] == b'\\' && self.pos + 1 < self.bytes.len() {
                        self.pos += 1;
                        s.push(match self.bytes[self.pos] {
                            b'n' => '\n',
                            b't' => '\t',
                            b'r' => '\r',
                            b'0' => '\0',
                            b => b as char,
                        });
                        self.pos += 1;
                    } else {
                        s.push(self.bytes[self.pos] as char);
                        self.pos += 1;
                    }
                }
                if self.pos >= self.bytes.len() {
                    return Err(Diagnostic::error(
                        "E0001",
                        self.span(start, self.pos),
                        "unterminated string in tree surface",
                    ));
                }
                self.pos += 1;
                Ok(Tok {
                    kind: TokKind::String(s),
                    span: self.span(start, self.pos),
                })
            }
            _ => {
                while self.pos < self.bytes.len() {
                    let b = self.bytes[self.pos];
                    if b.is_ascii_whitespace() || b == b'(' || b == b')' || b == b';' || b == b'"' {
                        break;
                    }
                    self.pos += 1;
                }
                let atom = self.src[start..self.pos].to_string();
                if atom.is_empty() {
                    return Err(Diagnostic::error(
                        "E0001",
                        self.span(start, self.pos),
                        "empty atom in tree surface",
                    ));
                }
                Ok(Tok {
                    kind: TokKind::Atom(atom),
                    span: self.span(start, self.pos),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// sexpr
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Sexp {
    Atom { text: String, span: Span },
    String { text: String, span: Span },
    List { items: Vec<Sexp>, span: Span },
}

impl Sexp {
    fn span(&self) -> Span {
        match self {
            Sexp::Atom { span, .. } | Sexp::String { span, .. } | Sexp::List { span, .. } => *span,
        }
    }

    fn atom(&self) -> Option<&str> {
        match self {
            Sexp::Atom { text, .. } => Some(text.as_str()),
            _ => None,
        }
    }

    fn is_atom(&self, s: &str) -> bool {
        self.atom() == Some(s)
    }
}

fn read_all(src: &str, file: FileId) -> Result<Vec<Sexp>, Vec<Diagnostic>> {
    let mut lx = Lexer::new(src, file);
    let mut out = Vec::new();
    loop {
        let t = lx.next().map_err(|d| vec![d])?;
        match t.kind {
            TokKind::Eof => return Ok(out),
            TokKind::RParen => {
                return Err(vec![Diagnostic::error(
                    "E0001",
                    t.span,
                    "unexpected `)` in tree surface",
                )]);
            }
            _ => out.push(read_sexp(&mut lx, t)?),
        }
    }
}

fn read_sexp(lx: &mut Lexer, first: Tok) -> Result<Sexp, Vec<Diagnostic>> {
    match first.kind {
        TokKind::Atom(text) => Ok(Sexp::Atom {
            text,
            span: first.span,
        }),
        TokKind::String(text) => Ok(Sexp::String {
            text,
            span: first.span,
        }),
        TokKind::LParen => {
            let mut items = Vec::new();
            loop {
                let t = lx.next().map_err(|d| vec![d])?;
                match t.kind {
                    TokKind::RParen => {
                        return Ok(Sexp::List {
                            items,
                            span: first.span.merge(t.span),
                        });
                    }
                    TokKind::Eof => {
                        return Err(vec![Diagnostic::error(
                            "E0001",
                            first.span,
                            "unterminated `(` in tree surface",
                        )]);
                    }
                    _ => items.push(read_sexp(lx, t)?),
                }
            }
        }
        TokKind::RParen | TokKind::Eof => Err(vec![Diagnostic::error(
            "E0001",
            first.span,
            "expected a tree form",
        )]),
    }
}

// ---------------------------------------------------------------------------
// public entry
// ---------------------------------------------------------------------------

/// True when the first non-comment, non-whitespace character is `(`.
/// Conventional Ax never starts a file that way, so this is a safe detect.
pub fn looks_like_tree(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i] == b';' || (b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = i.saturating_add(2);
            continue;
        }
        return b[i] == b'(';
    }
    false
}

/// First non-comment token of a conventional / terse file.
/// Used so a Tree-default session still accepts the corpus dialect.
pub fn looks_like_conventional(src: &str) -> bool {
    match first_word(src).as_deref() {
        Some(
            "module" | "export" | "use" | "fn" | "type" | "dict" | "test" | "contract" | "pub"
            | "unsafe" | "struct" | "enum" | "impl" | "trait" | "let",
        ) => true,
        _ => false,
    }
}

fn first_word(src: &str) -> Option<String> {
    let b = src.as_bytes();
    let mut i = 0;
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        if b[i] == b';' || (b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = i.saturating_add(2);
            continue;
        }
        if b[i] == b'@' || (b[i] == b'#' && i + 1 < b.len() && b[i + 1] == b'[') {
            // skip a meta / attribute line (`#[…]`). A dense `#name(` is a fn.
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
            i += 1;
        }
        if i == start {
            return None;
        }
        return Some(src[start..i].to_string());
    }
}

/// Pick the parser path for this source. Tree is an optional machine format,
/// Ax is the language, and expanded source is an internal compatibility form.
pub fn detect_surface(src: &str, preferred: crate::frontend::Surface) -> crate::frontend::Surface {
    use crate::frontend::Surface;
    if looks_like_tree(src) {
        return Surface::Tree;
    }
    if crate::frontend::looks_like_dense(src) {
        return Surface::Ax;
    }
    if looks_like_conventional(src) {
        return Surface::Ax;
    }
    preferred
}

pub fn parse_file(
    src: &str,
    file: FileId,
    intern: &mut Interner,
    module_fallback: &str,
) -> Result<File, Vec<Diagnostic>> {
    let forms = read_all(src, file)?;
    let mut p = TreeParser {
        intern,
        diags: Vec::new(),
        file,
    };
    let ast = p.parse_module(&forms, module_fallback);
    if !p.diags.is_empty() {
        return Err(p.diags);
    }
    let mut ast = ast?;
    crate::ast::renumber(&mut ast);
    Ok(ast)
}

// ---------------------------------------------------------------------------
// parser
// ---------------------------------------------------------------------------

struct TreeParser<'a> {
    intern: &'a mut Interner,
    diags: Vec<Diagnostic>,
    file: FileId,
}

impl<'a> TreeParser<'a> {
    fn err(&mut self, span: Span, msg: impl Into<String>) {
        self.diags
            .push(Diagnostic::error("E0002", span, msg.into()));
    }

    fn ident(&mut self, name: &str, span: Span) -> Ident {
        Ident {
            name: self.intern.intern(name),
            span,
        }
    }

    fn path_dotted(&mut self, s: &str, span: Span) -> Path {
        let segs = s
            .split('.')
            .map(|part| Ident {
                name: self.intern.intern(part),
                span,
            })
            .collect();
        Path { segs, span }
    }

    fn parse_module(&mut self, forms: &[Sexp], fallback: &str) -> Result<File, Vec<Diagnostic>> {
        let dummy = Span::new(self.file, 0, 0);
        if forms.len() == 1 {
            if let Sexp::List { items, span } = &forms[0] {
                if items.first().is_some_and(|h| h.is_atom("module")) {
                    return self.parse_module_list(items, *span, fallback);
                }
            }
        }
        let (exports, uses, decls) = self.parse_forms(forms)?;
        Ok(File {
            module: self.path_dotted(fallback, dummy),
            exports,
            uses,
            decls,
            span: dummy,
            node_count: 0,
        })
    }

    fn parse_module_list(
        &mut self,
        items: &[Sexp],
        span: Span,
        fallback: &str,
    ) -> Result<File, Vec<Diagnostic>> {
        // (module path form*)
        let name = if let Some(s) = items.get(1).and_then(Sexp::atom) {
            self.path_dotted(s, items[1].span())
        } else {
            self.path_dotted(fallback, span)
        };
        let rest = if items.len() > 2 { &items[2..] } else { &[] };
        let (exports, uses, decls) = self.parse_forms(rest)?;
        Ok(File {
            module: name,
            exports,
            uses,
            decls,
            span,
            node_count: 0,
        })
    }

    fn parse_forms(
        &mut self,
        forms: &[Sexp],
    ) -> Result<(Vec<Ident>, Vec<UseDecl>, Vec<Decl>), Vec<Diagnostic>> {
        let mut exports = Vec::new();
        let mut uses = Vec::new();
        let mut decls = Vec::new();
        let mut pending_meta: Vec<Meta> = Vec::new();
        for f in forms {
            match f {
                Sexp::List { items, span: _ }
                    if items.first().is_some_and(|h| h.is_atom("export")) =>
                {
                    for it in items.iter().skip(1) {
                        if let Some(n) = it.atom() {
                            exports.push(self.ident(n, it.span()));
                        } else {
                            self.err(it.span(), "export expects identifiers");
                        }
                    }
                }
                Sexp::List { items, span } if items.first().is_some_and(|h| h.is_atom("use")) => {
                    let path = match items.get(1).and_then(Sexp::atom) {
                        Some(s) => self.path_dotted(s, items[1].span()),
                        None => {
                            self.err(*span, "use expects a path");
                            continue;
                        }
                    };
                    let alias = if items.get(2).is_some_and(|x| x.is_atom("as")) {
                        items
                            .get(3)
                            .and_then(Sexp::atom)
                            .map(|n| self.ident(n, items[3].span()))
                    } else {
                        None
                    };
                    uses.push(UseDecl {
                        path,
                        alias,
                        span: *span,
                    });
                }
                Sexp::List { items, span } if items.first().is_some_and(|h| h.is_atom("@")) => {
                    if let Some(key) = items.get(1).and_then(Sexp::atom) {
                        let value = match items.get(2) {
                            Some(Sexp::String { text, .. }) => {
                                Some(MetaValue::String(text.clone()))
                            }
                            Some(Sexp::Atom { text, span }) => {
                                if let Ok(n) = text.parse::<i64>() {
                                    Some(MetaValue::Int(n))
                                } else {
                                    Some(MetaValue::Ident(self.ident(text, *span)))
                                }
                            }
                            _ => None,
                        };
                        pending_meta.push(Meta {
                            key: self.ident(key, items[1].span()),
                            value,
                            span: *span,
                        });
                    } else {
                        self.err(*span, "@ expects a key");
                    }
                }
                _ => {
                    let mut d = self.parse_decl(f)?;
                    if !pending_meta.is_empty() {
                        d.meta.append(&mut pending_meta);
                    }
                    decls.push(d);
                }
            }
        }
        Ok((exports, uses, decls))
    }

    fn parse_decl(&mut self, f: &Sexp) -> Result<Decl, Vec<Diagnostic>> {
        let Sexp::List { items, span } = f else {
            self.err(f.span(), "declaration must be a list");
            return Err(std::mem::take(&mut self.diags));
        };
        let head = items.first().and_then(Sexp::atom).unwrap_or("");
        let kind = match head {
            "fn" => DeclKind::Fn(self.parse_fn_decl(items, *span, false)?),
            "contract" => DeclKind::ContractFn(self.parse_fn_decl(items, *span, true)?),
            "type" => DeclKind::Type(self.parse_type_decl(items, *span)?),
            "dict" => DeclKind::Dict(self.parse_dict_decl(items, *span)?),
            "test" => DeclKind::Test(self.parse_test_decl(items, *span)?),
            other => {
                self.err(*span, format!("unknown declaration `{other}`"));
                return Err(std::mem::take(&mut self.diags));
            }
        };
        Ok(Decl {
            meta: Vec::new(),
            kind,
            span: *span,
        })
    }

    fn parse_fn_decl(
        &mut self,
        items: &[Sexp],
        span: Span,
        contract: bool,
    ) -> Result<FnDecl, Vec<Diagnostic>> {
        // (fn [(T…)] name () Ret body)
        // (fn name ((x T) (y U)) Ret [(! …)] [(pre e)] body)
        // (contract fn name …) — head is already consumed by caller as "contract"
        let mut i = 1;
        if contract {
            // (contract fn name …) or (contract name …)
            if items.get(i).is_some_and(|x| x.is_atom("fn")) {
                i += 1;
            }
        }
        let mut generics = Vec::new();
        if let Some(Sexp::List { items: g, .. }) = items.get(i) {
            if !g.is_empty() && g.iter().all(|x| x.atom().is_some()) {
                // (T U) generics — only when the next form is an ident (the name).
                if items.get(i + 1).and_then(Sexp::atom).is_some() {
                    for gitem in g {
                        let n = gitem.atom().unwrap();
                        generics.push(GParam {
                            name: self.ident(n, gitem.span()),
                            bound: None,
                        });
                    }
                    i += 1;
                }
            }
        }
        let name = match items.get(i).and_then(Sexp::atom) {
            Some(n) => {
                let id = self.ident(n, items[i].span());
                i += 1;
                id
            }
            None => {
                self.err(span, "fn needs a name");
                self.ident("_", span)
            }
        };
        // Params are exactly one list: () or ((x T) (y U) …). Grouping is
        // what makes `(Result i32 E)` a return type instead of a parameter.
        let params = match items.get(i) {
            Some(Sexp::List {
                items: ps,
                span: psp,
            }) => {
                i += 1;
                self.parse_param_list(ps, *psp)?
            }
            Some(other) => {
                self.err(other.span(), "fn params are one list: () or ((x T) …)");
                Vec::new()
            }
            None => {
                self.err(span, "fn needs a param list");
                Vec::new()
            }
        };
        let ret = if let Some(t) = items.get(i) {
            if t.atom() == Some("!")
                || matches!(t, Sexp::List { items, .. } if items.first().is_some_and(|h| h.is_atom("!")))
            {
                TypeExpr {
                    kind: TypeExprKind::Prim(Prim::Unit),
                    span,
                }
            } else {
                i += 1;
                self.parse_type(t)?
            }
        } else {
            self.err(span, "fn needs a return type");
            TypeExpr {
                kind: TypeExprKind::Hole,
                span,
            }
        };
        let mut effects = EffectRow {
            omitted: true,
            ..EffectRow::default()
        };
        if let Some(e) = items.get(i) {
            if e.atom() == Some("!")
                || matches!(e, Sexp::List { items, .. } if items.first().is_some_and(|h| h.is_atom("!")))
            {
                effects = self.parse_effects(e)?;
                i += 1;
            }
        }
        let mut contracts = Vec::new();
        while let Some(Sexp::List {
            items: c,
            span: csp,
        }) = items.get(i)
        {
            let head = c.first().and_then(Sexp::atom).unwrap_or("");
            let kind = match head {
                "pre" => ContractKind::Pre,
                "post" => ContractKind::Post,
                "inv" => ContractKind::Inv,
                _ => break,
            };
            if let Some(body) = c.get(1) {
                contracts.push(Contract {
                    kind,
                    expr: self.parse_expr(body)?,
                    span: *csp,
                });
            }
            i += 1;
        }
        let body = if let Some(b) = items.get(i) {
            i += 1;
            self.parse_expr(b)?
        } else {
            self.err(span, "fn needs a body");
            expr_lit(Lit::Unit, span)
        };
        if i < items.len() {
            self.err(
                items[i].span(),
                "unexpected form after fn body — the tree surface has no infix; write (+ a b)",
            );
        }
        let _ = contract;
        Ok(FnDecl {
            name,
            generics,
            params,
            ret,
            effects,
            contracts,
            body,
        })
    }

    fn parse_param_list(
        &mut self,
        items: &[Sexp],
        span: Span,
    ) -> Result<Vec<Param>, Vec<Diagnostic>> {
        let _ = span;
        // () — no params
        if items.is_empty() {
            return Ok(Vec::new());
        }
        // ((x T) (y U) …)
        if items.iter().all(|it| matches!(it, Sexp::List { .. })) {
            let mut params = Vec::new();
            for it in items {
                let Sexp::List {
                    items: p,
                    span: psp,
                } = it
                else {
                    continue;
                };
                let pname = match p.first().and_then(Sexp::atom) {
                    Some(n) => n,
                    None => {
                        self.err(*psp, "param is (name type)");
                        continue;
                    }
                };
                let ty = match p.get(1) {
                    Some(t) => self.parse_type(t)?,
                    None => {
                        self.err(*psp, "param is (name type)");
                        TypeExpr {
                            kind: TypeExprKind::Hole,
                            span: *psp,
                        }
                    }
                };
                params.push(Param {
                    name: self.ident(pname, p[0].span()),
                    ty,
                    default_dict: p.get(2).is_some_and(|x| x.is_atom("default")),
                });
            }
            return Ok(params);
        }
        // Bare (x T) — a single param written without the extra list.
        // Rejected: grouping must be uniform so `(Result i32 E)` cannot be
        // mistaken for a parameter.
        self.err(
            span,
            "fn params are one list of (name type): write ((x T) (y U)) or ()",
        );
        Ok(Vec::new())
    }

    fn parse_type_decl(&mut self, items: &[Sexp], span: Span) -> Result<TypeDecl, Vec<Diagnostic>> {
        // (type Name [(T…)] body [(from T V)…])
        let name = match items.get(1).and_then(Sexp::atom) {
            Some(n) => self.ident(n, items[1].span()),
            None => {
                self.err(span, "type needs a name");
                self.ident("_", span)
            }
        };
        let mut i = 2;
        let mut generics = Vec::new();
        if let Some(Sexp::List { items: g, .. }) = items.get(i) {
            if !g.is_empty() && g.iter().all(|x| x.atom().is_some()) {
                // could be generics or the body `(rec …)` / `(or …)` — those have a keyword head
                let head = g[0].atom().unwrap_or("");
                if !matches!(
                    head,
                    "rec" | "or" | "fn" | "ref" | "own" | "untrusted" | "secret" | "tuple"
                ) {
                    for gitem in g {
                        generics.push(GParam {
                            name: self.ident(gitem.atom().unwrap(), gitem.span()),
                            bound: None,
                        });
                    }
                    i += 1;
                }
            }
        }
        let body_form = items.get(i);
        i += 1;
        let body = match body_form {
            Some(Sexp::List { items: b, .. }) if b.first().is_some_and(|h| h.is_atom("rec")) => {
                TypeBody::Record(self.parse_fields(&b[1..])?)
            }
            Some(Sexp::List { items: b, .. }) if b.first().is_some_and(|h| h.is_atom("or")) => {
                TypeBody::Variants(self.parse_variants(&b[1..])?)
            }
            Some(t) => TypeBody::Alias(self.parse_type(t)?),
            None => {
                self.err(span, "type needs a body");
                TypeBody::Alias(TypeExpr {
                    kind: TypeExprKind::Hole,
                    span,
                })
            }
        };
        let mut injections = Vec::new();
        while let Some(Sexp::List {
            items: inj,
            span: isp,
        }) = items.get(i)
        {
            if !inj.first().is_some_and(|h| h.is_atom("from")) {
                break;
            }
            if inj.len() >= 3 {
                let from = self.parse_type(&inj[1])?;
                let into = inj[2]
                    .atom()
                    .map(|n| self.ident(n, inj[2].span()))
                    .unwrap_or_else(|| self.ident("_", *isp));
                injections.push(Injection {
                    from,
                    into_variant: into,
                    span: *isp,
                });
            }
            i += 1;
        }
        Ok(TypeDecl {
            name,
            generics,
            body,
            injections,
        })
    }

    fn parse_fields(&mut self, items: &[Sexp]) -> Result<Vec<Field>, Vec<Diagnostic>> {
        let mut out = Vec::new();
        for it in items {
            match it {
                Sexp::List { items: kv, span } if kv.len() >= 2 => {
                    let n = kv[0].atom().unwrap_or("_");
                    out.push(Field {
                        name: self.ident(n, kv[0].span()),
                        ty: self.parse_type(&kv[1])?,
                    });
                    let _ = span;
                }
                other => self.err(other.span(), "field is (name type)"),
            }
        }
        Ok(out)
    }

    fn parse_variants(&mut self, items: &[Sexp]) -> Result<Vec<Variant>, Vec<Diagnostic>> {
        let mut out = Vec::new();
        for it in items {
            match it {
                Sexp::Atom { text, span } => out.push(Variant {
                    name: self.ident(text, *span),
                    fields: Vec::new(),
                }),
                Sexp::List { items: v, span } if !v.is_empty() => {
                    let n = v[0].atom().unwrap_or("_");
                    out.push(Variant {
                        name: self.ident(n, v[0].span()),
                        fields: self.parse_fields(&v[1..])?,
                    });
                    let _ = span;
                }
                other => self.err(other.span(), "variant is Name or (Name (field type)…)"),
            }
        }
        Ok(out)
    }

    fn parse_dict_decl(&mut self, items: &[Sexp], span: Span) -> Result<DictDecl, Vec<Diagnostic>> {
        // (dict Name T (field expr)…)
        let name = match items.get(1).and_then(Sexp::atom) {
            Some(n) => self.ident(n, items[1].span()),
            None => {
                self.err(span, "dict needs a name");
                self.ident("_", span)
            }
        };
        let for_ty = if let Some(t) = items.get(2) {
            self.parse_type(t)?
        } else {
            self.err(span, "dict needs a type argument");
            TypeExpr {
                kind: TypeExprKind::Hole,
                span,
            }
        };
        let mut fields = Vec::new();
        for it in items.iter().skip(3) {
            if let Sexp::List { items: kv, .. } = it {
                if kv.len() >= 2 {
                    if let Some(n) = kv[0].atom() {
                        fields.push((self.ident(n, kv[0].span()), self.parse_expr(&kv[1])?));
                    }
                }
            }
        }
        Ok(DictDecl {
            name,
            for_ty,
            fields,
        })
    }

    fn parse_test_decl(&mut self, items: &[Sexp], span: Span) -> Result<TestDecl, Vec<Diagnostic>> {
        let name = match items.get(1) {
            Some(Sexp::String { text, .. }) => text.clone(),
            Some(Sexp::Atom { text, .. }) => text.clone(),
            _ => {
                self.err(span, "test needs a name");
                String::new()
            }
        };
        let body = if let Some(b) = items.get(2) {
            self.parse_expr(b)?
        } else {
            self.err(span, "test needs a body");
            expr_lit(Lit::Unit, span)
        };
        Ok(TestDecl { name, body })
    }

    fn parse_effects(&mut self, e: &Sexp) -> Result<EffectRow, Vec<Diagnostic>> {
        let items = match e {
            Sexp::List { items, .. } if items.first().is_some_and(|h| h.is_atom("!")) => {
                &items[1..]
            }
            Sexp::Atom { text, .. } if text == "!" => &[][..],
            other => {
                self.err(other.span(), "effects are (! atom-or-list…)");
                return Ok(EffectRow {
                    omitted: false,
                    ..EffectRow::default()
                });
            }
        };
        let mut out = Vec::new();
        let span = e.span();
        for it in items {
            out.push(self.parse_effect(it)?);
        }
        Ok(EffectRow {
            items: out,
            span,
            omitted: false,
        })
    }

    fn parse_effect(&mut self, e: &Sexp) -> Result<Effect, Vec<Diagnostic>> {
        let span = e.span();
        match e {
            Sexp::Atom { text, .. } => {
                let kind = match text.as_str() {
                    "susp" => EffectKind::Susp,
                    "diverge" => EffectKind::Diverge,
                    "race" => EffectKind::Race,
                    "nondet" => EffectKind::Nondet,
                    "abort" => EffectKind::Abort,
                    other => {
                        self.err(span, format!("unknown effect `{other}`"));
                        EffectKind::Abort
                    }
                };
                Ok(Effect { kind, span })
            }
            Sexp::List { items, .. } if !items.is_empty() => {
                let head = items[0].atom().unwrap_or("");
                let kind = match head {
                    "err" => {
                        let t = items.get(1).ok_or_else(|| {
                            self.err(span, "err needs a type");
                            std::mem::take(&mut self.diags)
                        })?;
                        EffectKind::Err(self.parse_type(t)?)
                    }
                    "io" => {
                        let n = items.get(1).and_then(Sexp::atom).unwrap_or("_");
                        EffectKind::Io(self.ident(n, items.get(1).map(Sexp::span).unwrap_or(span)))
                    }
                    "alloc" => {
                        let n = items.get(1).and_then(Sexp::atom).unwrap_or("_");
                        EffectKind::Alloc(
                            self.ident(n, items.get(1).map(Sexp::span).unwrap_or(span)),
                        )
                    }
                    other => {
                        self.err(span, format!("unknown effect `{other}`"));
                        EffectKind::Abort
                    }
                };
                Ok(Effect { kind, span })
            }
            _ => {
                self.err(span, "bad effect");
                Ok(Effect {
                    kind: EffectKind::Abort,
                    span,
                })
            }
        }
    }

    fn parse_type(&mut self, t: &Sexp) -> Result<TypeExpr, Vec<Diagnostic>> {
        let span = t.span();
        match t {
            Sexp::Atom { text, .. } if text == "?" => Ok(TypeExpr {
                kind: TypeExprKind::Hole,
                span,
            }),
            Sexp::Atom { text, .. } => {
                if let Some(p) = Prim::from_str(text) {
                    return Ok(TypeExpr {
                        kind: TypeExprKind::Prim(p),
                        span,
                    });
                }
                match text.as_str() {
                    "String" | "str" => Ok(named_ty(self, text, Vec::new(), span)),
                    _ => Ok(named_ty(self, text, Vec::new(), span)),
                }
            }
            Sexp::List { items, .. } if items.is_empty() => Ok(TypeExpr {
                kind: TypeExprKind::Tuple(Vec::new()),
                span,
            }),
            Sexp::List { items, .. } => {
                let head = items[0].atom().unwrap_or("");
                match head {
                    "?" => Ok(TypeExpr {
                        kind: TypeExprKind::Hole,
                        span,
                    }),
                    "own" => {
                        let inner = items.get(1).ok_or_else(|| {
                            self.err(span, "own needs a type");
                            std::mem::take(&mut self.diags)
                        })?;
                        Ok(TypeExpr {
                            kind: TypeExprKind::Own(Box::new(self.parse_type(inner)?)),
                            span,
                        })
                    }
                    "untrusted" | "Untrusted" => {
                        let inner = items.get(1).ok_or_else(|| {
                            self.err(span, "untrusted needs a type");
                            std::mem::take(&mut self.diags)
                        })?;
                        Ok(TypeExpr {
                            kind: TypeExprKind::Untrusted(Box::new(self.parse_type(inner)?)),
                            span,
                        })
                    }
                    "secret" | "Secret" => {
                        let inner = items.get(1).ok_or_else(|| {
                            self.err(span, "secret needs a type");
                            std::mem::take(&mut self.diags)
                        })?;
                        Ok(TypeExpr {
                            kind: TypeExprKind::Secret(Box::new(self.parse_type(inner)?)),
                            span,
                        })
                    }
                    "ref" => {
                        // (ref r T) | (ref r mut T) | (ref T)
                        let mut i = 1;
                        let mut region_name = "_";
                        let mut region_span = span;
                        let mut mutable = false;
                        if let Some(a) = items.get(i).and_then(Sexp::atom) {
                            if a == "mut" {
                                mutable = true;
                                i += 1;
                            } else if items.get(i + 1).is_some() {
                                region_name = a;
                                region_span = items[i].span();
                                i += 1;
                                if items.get(i).is_some_and(|x| x.is_atom("mut")) {
                                    mutable = true;
                                    i += 1;
                                }
                            }
                        }
                        let inner = items.get(i).ok_or_else(|| {
                            self.err(span, "ref needs a type");
                            std::mem::take(&mut self.diags)
                        })?;
                        Ok(TypeExpr {
                            kind: TypeExprKind::Ref {
                                region: self.ident(region_name, region_span),
                                mutable,
                                inner: Box::new(self.parse_type(inner)?),
                            },
                            span,
                        })
                    }
                    "fn" | "fn-type" => {
                        // (fn (T…) R [(! …)])
                        let params_form = items.get(1);
                        let mut params = Vec::new();
                        if let Some(Sexp::List { items: ps, .. }) = params_form {
                            for p in ps {
                                params.push(self.parse_type(p)?);
                            }
                        }
                        let ret = if let Some(r) = items.get(2) {
                            Box::new(self.parse_type(r)?)
                        } else {
                            Box::new(TypeExpr {
                                kind: TypeExprKind::Prim(Prim::Unit),
                                span,
                            })
                        };
                        let effects = if let Some(e) = items.get(3) {
                            self.parse_effects(e)?
                        } else {
                            EffectRow::default()
                        };
                        Ok(TypeExpr {
                            kind: TypeExprKind::Fn {
                                params,
                                ret,
                                effects,
                            },
                            span,
                        })
                    }
                    "tuple" => {
                        let mut ts = Vec::new();
                        for it in items.iter().skip(1) {
                            ts.push(self.parse_type(it)?);
                        }
                        Ok(TypeExpr {
                            kind: TypeExprKind::Tuple(ts),
                            span,
                        })
                    }
                    _ => {
                        // (Name T…)  — named type with args, including Vec / Option / Result
                        let mut args = Vec::new();
                        for it in items.iter().skip(1) {
                            args.push(self.parse_type(it)?);
                        }
                        if head == "Untrusted" && args.len() == 1 {
                            return Ok(TypeExpr {
                                kind: TypeExprKind::Untrusted(Box::new(args.remove(0))),
                                span,
                            });
                        }
                        if head == "Secret" && args.len() == 1 {
                            return Ok(TypeExpr {
                                kind: TypeExprKind::Secret(Box::new(args.remove(0))),
                                span,
                            });
                        }
                        Ok(named_ty(self, head, args, span))
                    }
                }
            }
            Sexp::String { .. } => {
                self.err(span, "a string is not a type");
                Ok(TypeExpr {
                    kind: TypeExprKind::Hole,
                    span,
                })
            }
        }
    }

    fn parse_expr(&mut self, e: &Sexp) -> Result<Expr, Vec<Diagnostic>> {
        let span = e.span();
        match e {
            Sexp::String { text, .. } => Ok(expr_lit(Lit::Str(text.clone()), span)),
            Sexp::Atom { text, .. } => self.parse_atom_expr(text, span),
            Sexp::List { items, .. } if items.is_empty() => Ok(expr_lit(Lit::Unit, span)),
            Sexp::List { items, .. } => self.parse_list_expr(items, span),
        }
    }

    fn parse_atom_expr(&mut self, text: &str, span: Span) -> Result<Expr, Vec<Diagnostic>> {
        if text == "?" {
            return Ok(Expr {
                id: NodeId::NONE,
                kind: ExprKind::Hole,
                span,
            });
        }
        if text == "true" {
            return Ok(expr_lit(Lit::Bool(true), span));
        }
        if text == "false" {
            return Ok(expr_lit(Lit::Bool(false), span));
        }
        if text == "break" {
            return Ok(Expr {
                id: NodeId::NONE,
                kind: ExprKind::Break,
                span,
            });
        }
        if text == "continue" {
            return Ok(Expr {
                id: NodeId::NONE,
                kind: ExprKind::Continue,
                span,
            });
        }
        if let Some(lit) = parse_num_atom(text) {
            return Ok(expr_lit(lit, span));
        }
        Ok(Expr {
            id: NodeId::NONE,
            kind: ExprKind::Path(self.path_dotted(text, span)),
            span,
        })
    }

    fn parse_list_expr(&mut self, items: &[Sexp], span: Span) -> Result<Expr, Vec<Diagnostic>> {
        let head = match items.first() {
            Some(h) => h,
            None => return Ok(expr_lit(Lit::Unit, span)),
        };
        let op = head.atom().unwrap_or("");
        if let Some(bop) = binop(op) {
            if items.len() != 3 {
                self.err(span, format!("`{op}` takes exactly two operands"));
            }
            let lhs = items
                .get(1)
                .map(|x| self.parse_expr(x))
                .transpose()?
                .unwrap_or_else(|| expr_lit(Lit::Unit, span));
            let rhs = items
                .get(2)
                .map(|x| self.parse_expr(x))
                .transpose()?
                .unwrap_or_else(|| expr_lit(Lit::Unit, span));
            return Ok(Expr {
                id: NodeId::NONE,
                kind: ExprKind::Binary {
                    op: bop,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            });
        }
        match op {
            "not" | "!" => self.unary(UnOp::Not, items, span),
            "bnot" | "~" => self.unary(UnOp::BitNot, items, span),
            "neg" => self.unary(UnOp::Neg, items, span),
            "ref" => self.unary(UnOp::Ref, items, span),
            "refmut" => self.unary(UnOp::RefMut, items, span),
            "deref" => self.unary(UnOp::Deref, items, span),
            "as" => {
                let e = items.get(1).ok_or_else(|| {
                    self.err(span, "as needs an expression");
                    std::mem::take(&mut self.diags)
                })?;
                let ty = items.get(2).ok_or_else(|| {
                    self.err(span, "as needs a type");
                    std::mem::take(&mut self.diags)
                })?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Cast {
                        expr: Box::new(self.parse_expr(e)?),
                        ty: self.parse_type(ty)?,
                    },
                    span,
                })
            }
            "field" => {
                let base = items.get(1).ok_or_else(|| {
                    self.err(span, "field needs a base");
                    std::mem::take(&mut self.diags)
                })?;
                let name = items.get(2).and_then(Sexp::atom).unwrap_or("_");
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Field {
                        base: Box::new(self.parse_expr(base)?),
                        field: self.ident(name, items.get(2).map(Sexp::span).unwrap_or(span)),
                    },
                    span,
                })
            }
            "index" => {
                let base = items.get(1).ok_or_else(|| {
                    self.err(span, "index needs a base");
                    std::mem::take(&mut self.diags)
                })?;
                let ix = items.get(2).ok_or_else(|| {
                    self.err(span, "index needs an index");
                    std::mem::take(&mut self.diags)
                })?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Index {
                        base: Box::new(self.parse_expr(base)?),
                        index: Box::new(self.parse_expr(ix)?),
                    },
                    span,
                })
            }
            "let" => {
                let l = self.parse_let(&items[1..], span)?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Let(Box::new(l)),
                    span,
                })
            }
            "set" => {
                let lhs = items.get(1).ok_or_else(|| {
                    self.err(span, "set needs a lhs");
                    std::mem::take(&mut self.diags)
                })?;
                let rhs = items.get(2).ok_or_else(|| {
                    self.err(span, "set needs a rhs");
                    std::mem::take(&mut self.diags)
                })?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Assign {
                        lhs: Box::new(self.parse_expr(lhs)?),
                        rhs: Box::new(self.parse_expr(rhs)?),
                    },
                    span,
                })
            }
            "block" => {
                let mut stmts = Vec::new();
                let mut tail = None;
                for (k, it) in items.iter().skip(1).enumerate() {
                    let is_last = k + 2 == items.len();
                    if is_last && !is_stmt_form(it) {
                        tail = Some(Box::new(self.parse_expr(it)?));
                    } else if is_let_form(it) {
                        if let Sexp::List {
                            items: li,
                            span: lsp,
                        } = it
                        {
                            let l = self.parse_let(&li[1..], *lsp)?;
                            stmts.push(Stmt {
                                kind: StmtKind::Let(l),
                                span: *lsp,
                            });
                        }
                    } else {
                        let ex = self.parse_expr(it)?;
                        stmts.push(Stmt {
                            kind: StmtKind::Expr(ex),
                            span: it.span(),
                        });
                    }
                }
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Block { stmts, tail },
                    span,
                })
            }
            "if" => {
                let cond = items.get(1).ok_or_else(|| {
                    self.err(span, "if needs a condition");
                    std::mem::take(&mut self.diags)
                })?;
                let then_b = items.get(2).ok_or_else(|| {
                    self.err(span, "if needs a then branch");
                    std::mem::take(&mut self.diags)
                })?;
                let else_b = match items.get(3) {
                    Some(e) => Some(Box::new(self.parse_expr(e)?)),
                    None => None,
                };
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::If {
                        cond: Box::new(self.parse_expr(cond)?),
                        then_b: Box::new(self.parse_expr(then_b)?),
                        else_b,
                    },
                    span,
                })
            }
            "match" => {
                let scrut = items.get(1).ok_or_else(|| {
                    self.err(span, "match needs a scrutinee");
                    std::mem::take(&mut self.diags)
                })?;
                let mut arms = Vec::new();
                for it in items.iter().skip(2) {
                    arms.push(self.parse_arm(it)?);
                }
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Match {
                        scrut: Box::new(self.parse_expr(scrut)?),
                        arms,
                    },
                    span,
                })
            }
            "for" => {
                let pat = items.get(1).ok_or_else(|| {
                    self.err(span, "for needs a pattern");
                    std::mem::take(&mut self.diags)
                })?;
                let iter = items.get(2).ok_or_else(|| {
                    self.err(span, "for needs an iterator");
                    std::mem::take(&mut self.diags)
                })?;
                let body = items.get(3).ok_or_else(|| {
                    self.err(span, "for needs a body");
                    std::mem::take(&mut self.diags)
                })?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::For {
                        pat: self.parse_pattern(pat)?,
                        iter: Box::new(self.parse_expr(iter)?),
                        body: Box::new(self.parse_expr(body)?),
                    },
                    span,
                })
            }
            "while" => {
                let cond = items.get(1).ok_or_else(|| {
                    self.err(span, "while needs a condition");
                    std::mem::take(&mut self.diags)
                })?;
                let body = items.get(2).ok_or_else(|| {
                    self.err(span, "while needs a body");
                    std::mem::take(&mut self.diags)
                })?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::While {
                        cond: Box::new(self.parse_expr(cond)?),
                        body: Box::new(self.parse_expr(body)?),
                    },
                    span,
                })
            }
            "loop" => {
                let body = items.get(1).ok_or_else(|| {
                    self.err(span, "loop needs a body");
                    std::mem::take(&mut self.diags)
                })?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Loop {
                        body: Box::new(self.parse_expr(body)?),
                    },
                    span,
                })
            }
            "return" => {
                let inner = match items.get(1) {
                    Some(e) => Some(Box::new(self.parse_expr(e)?)),
                    None => None,
                };
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Return(inner),
                    span,
                })
            }
            "raise" => {
                let e = items.get(1).ok_or_else(|| {
                    self.err(span, "raise needs a value");
                    std::mem::take(&mut self.diags)
                })?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Raise(Box::new(self.parse_expr(e)?)),
                    span,
                })
            }
            "catch" => {
                let e = items.get(1).ok_or_else(|| {
                    self.err(span, "catch needs an expression");
                    std::mem::take(&mut self.diags)
                })?;
                let mut arms = Vec::new();
                for it in items.iter().skip(2) {
                    arms.push(self.parse_arm(it)?);
                }
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Catch {
                        expr: Box::new(self.parse_expr(e)?),
                        arms,
                    },
                    span,
                })
            }
            "attempt" => {
                let e = items.get(1).ok_or_else(|| {
                    self.err(span, "attempt needs an expression");
                    std::mem::take(&mut self.diags)
                })?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Attempt(Box::new(self.parse_expr(e)?)),
                    span,
                })
            }
            "try" => {
                let e = items.get(1).ok_or_else(|| {
                    self.err(span, "try needs an expression");
                    std::mem::take(&mut self.diags)
                })?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Try(Box::new(self.parse_expr(e)?)),
                    span,
                })
            }
            "rec" => {
                let mut fields = Vec::new();
                for it in items.iter().skip(1) {
                    if let Sexp::List { items: kv, .. } = it {
                        if kv.len() >= 2 {
                            if let Some(n) = kv[0].atom() {
                                fields
                                    .push((self.ident(n, kv[0].span()), self.parse_expr(&kv[1])?));
                            }
                        }
                    }
                }
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Record(fields),
                    span,
                })
            }
            "var" => {
                let name = items.get(1).and_then(Sexp::atom).unwrap_or("_");
                let mut fields = Vec::new();
                for it in items.iter().skip(2) {
                    if let Sexp::List { items: kv, .. } = it {
                        if kv.len() >= 2 {
                            if let Some(n) = kv[0].atom() {
                                fields
                                    .push((self.ident(n, kv[0].span()), self.parse_expr(&kv[1])?));
                            }
                        } else if kv.len() == 1 {
                            // positional (var Some x) handled below
                        }
                    } else {
                        // positional payload
                        let idx = fields.len();
                        let fname = self.intern.intern(&format!("_{idx}"));
                        fields.push((
                            Ident {
                                name: fname,
                                span: it.span(),
                            },
                            self.parse_expr(it)?,
                        ));
                    }
                }
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Variant {
                        name: self.ident(name, items.get(1).map(Sexp::span).unwrap_or(span)),
                        fields,
                    },
                    span,
                })
            }
            "fn" => {
                // lambda: (fn () body) | (fn ((x T) …) [Ret] body)
                let mut i = 1;
                let params = match items.get(i) {
                    Some(Sexp::List {
                        items: ps,
                        span: psp,
                    }) => {
                        i += 1;
                        self.parse_param_list(ps, *psp)?
                    }
                    _ => Vec::new(),
                };
                let mut ret = None;
                // If two forms remain, first is ret type.
                if items.len() - i == 2 {
                    ret = Some(self.parse_type(&items[i])?);
                    i += 1;
                }
                let body = if let Some(b) = items.get(i) {
                    self.parse_expr(b)?
                } else {
                    expr_lit(Lit::Unit, span)
                };
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Lambda {
                        params,
                        ret,
                        body: Box::new(body),
                    },
                    span,
                })
            }
            "region" => {
                let name = items.get(1).and_then(Sexp::atom).unwrap_or("r");
                let body = items.get(2).ok_or_else(|| {
                    self.err(span, "region needs a body");
                    std::mem::take(&mut self.diags)
                })?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Region {
                        name: self.ident(name, items.get(1).map(Sexp::span).unwrap_or(span)),
                        body: Box::new(self.parse_expr(body)?),
                    },
                    span,
                })
            }
            "tuple" => {
                let mut es = Vec::new();
                for it in items.iter().skip(1) {
                    es.push(self.parse_expr(it)?);
                }
                // Represent a tuple as a record with positional `_0`, `_1`, …
                // fields so the rest of the pipeline (which already accepts
                // that encoding for variant payloads) can lower it.
                let mut fields = Vec::new();
                for (idx, e) in es.into_iter().enumerate() {
                    let fname = self.intern.intern(&format!("_{idx}"));
                    fields.push((
                        Ident {
                            name: fname,
                            span: e.span,
                        },
                        e,
                    ));
                }
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Record(fields),
                    span,
                })
            }
            "par" => {
                let mut bindings = Vec::new();
                for it in items.iter().skip(1) {
                    if let Sexp::List {
                        items: li,
                        span: lsp,
                    } = it
                    {
                        if li.first().is_some_and(|h| h.is_atom("let")) {
                            bindings.push(self.parse_let(&li[1..], *lsp)?);
                        }
                    }
                }
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Par { bindings },
                    span,
                })
            }
            "interp" => {
                let mut parts = Vec::new();
                for it in items.iter().skip(1) {
                    match it {
                        Sexp::String { text, .. } => parts.push(InterpPart::Lit(text.clone())),
                        other => parts.push(InterpPart::Expr(self.parse_expr(other)?)),
                    }
                }
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Interpolate { parts },
                    span,
                })
            }
            "arm" => {
                self.err(span, "`arm` is only legal inside match/catch");
                Ok(expr_lit(Lit::Unit, span))
            }
            _ => {
                // call: (callee arg…)
                // A dotted head `(xs.at i)` is a method: Field(xs, at)(i).
                // `int.div_trunc` is the same shape; the checker treats a
                // non-local first segment as a qualified function.
                let callee = if let Some(text) = head.atom() {
                    if text.contains('.') {
                        let mut segs: Vec<&str> = text.split('.').collect();
                        let field = segs.pop().unwrap_or("_");
                        let base_name = segs.join(".");
                        let base = self.parse_atom_expr(&base_name, head.span())?;
                        Expr {
                            id: NodeId::NONE,
                            kind: ExprKind::Field {
                                base: Box::new(base),
                                field: self.ident(field, head.span()),
                            },
                            span: head.span(),
                        }
                    } else {
                        self.parse_expr(head)?
                    }
                } else {
                    self.parse_expr(head)?
                };
                let mut args = Vec::new();
                for a in items.iter().skip(1) {
                    args.push(self.parse_expr(a)?);
                }
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Call {
                        callee: Box::new(callee),
                        args,
                    },
                    span,
                })
            }
        }
    }

    fn unary(&mut self, op: UnOp, items: &[Sexp], span: Span) -> Result<Expr, Vec<Diagnostic>> {
        let e = items.get(1).ok_or_else(|| {
            self.err(span, "unary operator needs one operand");
            std::mem::take(&mut self.diags)
        })?;
        Ok(Expr {
            id: NodeId::NONE,
            kind: ExprKind::Unary {
                op,
                expr: Box::new(self.parse_expr(e)?),
            },
            span,
        })
    }

    fn parse_let(&mut self, items: &[Sexp], span: Span) -> Result<LetStmt, Vec<Diagnostic>> {
        let mut i = 0;
        let mutable = if items.first().is_some_and(|x| x.is_atom("mut")) {
            i = 1;
            true
        } else {
            false
        };
        let pat_form = items.get(i).ok_or_else(|| {
            self.err(span, "let needs a pattern");
            std::mem::take(&mut self.diags)
        })?;
        i += 1;
        let rest = items.len().saturating_sub(i);
        let (ty, init) = if rest == 0 {
            self.err(span, "let needs an initializer");
            (None, expr_lit(Lit::Unit, span))
        } else if rest == 1 {
            (None, self.parse_expr(&items[i])?)
        } else {
            let t = self.parse_type(&items[i])?;
            let e = self.parse_expr(&items[i + 1])?;
            (Some(t), e)
        };
        Ok(LetStmt {
            mutable,
            pat: self.parse_pattern(pat_form)?,
            ty,
            init,
            span,
        })
    }

    fn parse_arm(&mut self, e: &Sexp) -> Result<Arm, Vec<Diagnostic>> {
        let span = e.span();
        match e {
            Sexp::List { items, .. } if items.first().is_some_and(|h| h.is_atom("arm")) => {
                let pat = items.get(1).ok_or_else(|| {
                    self.err(span, "arm needs a pattern");
                    std::mem::take(&mut self.diags)
                })?;
                let body = items.get(2).ok_or_else(|| {
                    self.err(span, "arm needs a body");
                    std::mem::take(&mut self.diags)
                })?;
                Ok(Arm {
                    pat: self.parse_pattern(pat)?,
                    body: self.parse_expr(body)?,
                    span,
                })
            }
            _ => {
                self.err(span, "match/catch arm is (arm pat body)");
                Ok(Arm {
                    pat: Pattern {
                        id: NodeId::NONE,
                        kind: PatKind::Wild,
                        span,
                    },
                    body: expr_lit(Lit::Unit, span),
                    span,
                })
            }
        }
    }

    fn parse_pattern(&mut self, e: &Sexp) -> Result<Pattern, Vec<Diagnostic>> {
        let span = e.span();
        match e {
            Sexp::Atom { text, .. } if text == "_" => Ok(Pattern {
                id: NodeId::NONE,
                kind: PatKind::Wild,
                span,
            }),
            Sexp::Atom { text, .. } if text == "true" => Ok(Pattern {
                id: NodeId::NONE,
                kind: PatKind::Lit(Lit::Bool(true)),
                span,
            }),
            Sexp::Atom { text, .. } if text == "false" => Ok(Pattern {
                id: NodeId::NONE,
                kind: PatKind::Lit(Lit::Bool(false)),
                span,
            }),
            Sexp::Atom { text, .. } => {
                if let Some(lit) = parse_num_atom(text) {
                    return Ok(Pattern {
                        id: NodeId::NONE,
                        kind: PatKind::Lit(lit),
                        span,
                    });
                }
                Ok(Pattern {
                    id: NodeId::NONE,
                    kind: PatKind::Bind(self.ident(text, span)),
                    span,
                })
            }
            Sexp::String { text, .. } => Ok(Pattern {
                id: NodeId::NONE,
                kind: PatKind::Lit(Lit::Str(text.clone())),
                span,
            }),
            Sexp::List { items, .. } if items.is_empty() => Ok(Pattern {
                id: NodeId::NONE,
                kind: PatKind::Tuple(Vec::new()),
                span,
            }),
            Sexp::List { items, .. } => {
                let head = items.first().and_then(Sexp::atom).unwrap_or("");
                match head {
                    "rec" => {
                        let mut fields = Vec::new();
                        for it in items.iter().skip(1) {
                            if let Sexp::List { items: kv, .. } = it {
                                if kv.len() >= 2 {
                                    if let Some(n) = kv[0].atom() {
                                        fields.push((
                                            self.ident(n, kv[0].span()),
                                            self.parse_pattern(&kv[1])?,
                                        ));
                                    }
                                } else if kv.len() == 1 {
                                    if let Some(n) = kv[0].atom() {
                                        let id = self.ident(n, kv[0].span());
                                        fields.push((
                                            id.clone(),
                                            Pattern {
                                                id: NodeId::NONE,
                                                kind: PatKind::Bind(id),
                                                span: kv[0].span(),
                                            },
                                        ));
                                    }
                                }
                            }
                        }
                        Ok(Pattern {
                            id: NodeId::NONE,
                            kind: PatKind::Record(fields),
                            span,
                        })
                    }
                    "tuple" => {
                        let mut ps = Vec::new();
                        for it in items.iter().skip(1) {
                            ps.push(self.parse_pattern(it)?);
                        }
                        Ok(Pattern {
                            id: NodeId::NONE,
                            kind: PatKind::Tuple(ps),
                            span,
                        })
                    }
                    "var" => {
                        let name = items.get(1).and_then(Sexp::atom).unwrap_or("_");
                        let mut fields = Vec::new();
                        for it in items.iter().skip(2) {
                            if let Sexp::List { items: kv, .. } = it {
                                if kv.len() >= 2 {
                                    if let Some(n) = kv[0].atom() {
                                        fields.push((
                                            self.ident(n, kv[0].span()),
                                            self.parse_pattern(&kv[1])?,
                                        ));
                                        continue;
                                    }
                                }
                            }
                            let idx = fields.len();
                            let fname = self.intern.intern(&format!("_{idx}"));
                            fields.push((
                                Ident {
                                    name: fname,
                                    span: it.span(),
                                },
                                self.parse_pattern(it)?,
                            ));
                        }
                        Ok(Pattern {
                            id: NodeId::NONE,
                            kind: PatKind::Variant {
                                name: self
                                    .ident(name, items.get(1).map(Sexp::span).unwrap_or(span)),
                                fields,
                            },
                            span,
                        })
                    }
                    _ => {
                        // (Name payload…) positional variant
                        let name = head;
                        let mut fields = Vec::new();
                        for it in items.iter().skip(1) {
                            let idx = fields.len();
                            let fname = self.intern.intern(&format!("_{idx}"));
                            fields.push((
                                Ident {
                                    name: fname,
                                    span: it.span(),
                                },
                                self.parse_pattern(it)?,
                            ));
                        }
                        Ok(Pattern {
                            id: NodeId::NONE,
                            kind: PatKind::Variant {
                                name: self.ident(name, items[0].span()),
                                fields,
                            },
                            span,
                        })
                    }
                }
            }
        }
    }
}

fn named_ty(p: &mut TreeParser, name: &str, args: Vec<TypeExpr>, span: Span) -> TypeExpr {
    TypeExpr {
        kind: TypeExprKind::Named {
            path: p.path_dotted(name, span),
            args,
        },
        span,
    }
}

fn expr_lit(l: Lit, span: Span) -> Expr {
    Expr {
        id: NodeId::NONE,
        kind: ExprKind::Lit(l),
        span,
    }
}

fn is_let_form(e: &Sexp) -> bool {
    matches!(e, Sexp::List { items, .. } if items.first().is_some_and(|h| h.is_atom("let")))
}

fn is_stmt_form(e: &Sexp) -> bool {
    is_let_form(e)
        || matches!(e, Sexp::List { items, .. } if items.first().is_some_and(|h| h.is_atom("set")))
}

fn binop(s: &str) -> Option<BinOp> {
    Some(match s {
        "+" => BinOp::Add,
        "-" => BinOp::Sub,
        "*" => BinOp::Mul,
        "/" => BinOp::Div,
        "%" => BinOp::Rem,
        "==" => BinOp::Eq,
        "!=" => BinOp::Ne,
        "<" => BinOp::Lt,
        "<=" => BinOp::Le,
        ">" => BinOp::Gt,
        ">=" => BinOp::Ge,
        "&&" => BinOp::And,
        "||" => BinOp::Or,
        "&" => BinOp::BitAnd,
        "|" => BinOp::BitOr,
        "^" => BinOp::BitXor,
        "<<" => BinOp::Shl,
        ">>" => BinOp::Shr,
        _ => return None,
    })
}

fn parse_num_atom(text: &str) -> Option<Lit> {
    // bool / hole / names are handled by the caller
    if text.is_empty() {
        return None;
    }
    let (num, suf) = split_suffix(text);
    if num.contains('.') || num.contains('e') || num.contains('E') {
        let clean: String = num.chars().filter(|c| *c != '_').collect();
        if let Ok(value) = clean.parse::<f64>() {
            return Some(Lit::Float { value, suffix: suf });
        }
    }
    if looks_like_int(num) {
        if let Some(value) = parse_int_text(num) {
            return Some(Lit::Int { value, suffix: suf });
        }
    }
    None
}

fn looks_like_int(s: &str) -> bool {
    let t = s.trim_start_matches('-');
    if t.is_empty() {
        return false;
    }
    t.starts_with("0x")
        || t.starts_with("0X")
        || t.starts_with("0o")
        || t.starts_with("0b")
        || t.starts_with("0O")
        || t.starts_with("0B")
        || t.chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn parse_int_text(s: &str) -> Option<i128> {
    let clean: String = s.chars().filter(|c| *c != '_').collect();
    let lower = clean.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        return i128::from_str_radix(hex, 16).ok();
    }
    if let Some(oct) = lower.strip_prefix("0o") {
        return i128::from_str_radix(oct, 8).ok();
    }
    if let Some(bin) = lower.strip_prefix("0b") {
        return i128::from_str_radix(bin, 2).ok();
    }
    clean
        .parse()
        .ok()
        .or_else(|| clean.parse::<u128>().ok().map(|v| v as i128))
}

fn split_suffix(text: &str) -> (&str, Option<Prim>) {
    for suf in [
        "i8", "i16", "i32", "i64", "isz", "u8", "u16", "u32", "u64", "usz", "f32", "f64",
    ] {
        if let Some(rest) = text.strip_suffix(suf) {
            if !rest.is_empty()
                && rest
                    .chars()
                    .last()
                    .map(|c| c.is_ascii_digit() || c == '_')
                    .unwrap_or(false)
                && !rest.to_ascii_lowercase().starts_with("0x")
            {
                return (rest, Prim::from_str(suf));
            }
        }
    }
    (text, None)
}

// ---------------------------------------------------------------------------
// printer — inverse of the parser
// ---------------------------------------------------------------------------

pub fn format_file(file: &File, intern: &Interner) -> String {
    let mut o = String::new();
    o.push_str("(module ");
    o.push_str(&path_s(&file.module, intern));
    o.push('\n');
    if !file.exports.is_empty() {
        o.push_str("  (export");
        for e in &file.exports {
            o.push(' ');
            o.push_str(intern.get(e.name));
        }
        o.push_str(")\n");
    }
    for u in &file.uses {
        o.push_str("  (use ");
        o.push_str(&path_s(&u.path, intern));
        if let Some(a) = &u.alias {
            o.push_str(" as ");
            o.push_str(intern.get(a.name));
        }
        o.push_str(")\n");
    }
    for d in &file.decls {
        for m in &d.meta {
            o.push_str("  (@ ");
            o.push_str(intern.get(m.key.name));
            match &m.value {
                Some(MetaValue::String(s)) => {
                    o.push(' ');
                    o.push('"');
                    o.push_str(&escape(s));
                    o.push('"');
                }
                Some(MetaValue::Ident(i)) => {
                    o.push(' ');
                    o.push_str(intern.get(i.name));
                }
                Some(MetaValue::Int(n)) => {
                    o.push(' ');
                    o.push_str(&n.to_string());
                }
                None => {}
            }
            o.push_str(")\n");
        }
        o.push_str("  ");
        fmt_decl(&mut o, d, intern, 1);
        o.push('\n');
    }
    o.push_str(")\n");
    o
}

fn fmt_decl(o: &mut String, d: &Decl, intern: &Interner, indent: usize) {
    match &d.kind {
        DeclKind::Fn(f) => fmt_fn(o, f, intern, false, indent),
        DeclKind::ContractFn(f) => fmt_fn(o, f, intern, true, indent),
        DeclKind::Type(t) => fmt_type_decl(o, t, intern),
        DeclKind::Dict(dd) => {
            o.push_str("(dict ");
            o.push_str(intern.get(dd.name.name));
            o.push(' ');
            fmt_ty(o, &dd.for_ty, intern);
            for (n, e) in &dd.fields {
                o.push_str(" (");
                o.push_str(intern.get(n.name));
                o.push(' ');
                fmt_expr(o, e, intern, indent);
                o.push(')');
            }
            o.push(')');
        }
        DeclKind::Test(t) => {
            o.push_str("(test \"");
            o.push_str(&escape(&t.name));
            o.push_str("\" ");
            fmt_expr(o, &t.body, intern, indent);
            o.push(')');
        }
    }
}

fn fmt_fn(o: &mut String, f: &FnDecl, intern: &Interner, contract: bool, indent: usize) {
    if contract {
        o.push_str("(contract ");
    }
    o.push_str("(fn ");
    if !f.generics.is_empty() {
        o.push('(');
        for (i, g) in f.generics.iter().enumerate() {
            if i > 0 {
                o.push(' ');
            }
            o.push_str(intern.get(g.name.name));
        }
        o.push_str(") ");
    }
    o.push_str(intern.get(f.name.name));
    o.push_str(" (");
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            o.push(' ');
        }
        o.push('(');
        o.push_str(intern.get(p.name.name));
        o.push(' ');
        fmt_ty(o, &p.ty, intern);
        if p.default_dict {
            o.push_str(" default");
        }
        o.push(')');
    }
    o.push(')');
    o.push(' ');
    fmt_ty(o, &f.ret, intern);
    if !f.effects.omitted {
        o.push(' ');
        fmt_effects(o, &f.effects, intern);
    }
    for c in &f.contracts {
        o.push(' ');
        o.push('(');
        o.push_str(match c.kind {
            ContractKind::Pre => "pre ",
            ContractKind::Post => "post ",
            ContractKind::Inv => "inv ",
        });
        fmt_expr(o, &c.expr, intern, indent);
        o.push(')');
    }
    o.push(' ');
    fmt_expr(o, &f.body, intern, indent);
    o.push(')');
    if contract {
        o.push(')');
    }
}

fn fmt_type_decl(o: &mut String, t: &TypeDecl, intern: &Interner) {
    o.push_str("(type ");
    o.push_str(intern.get(t.name.name));
    if !t.generics.is_empty() {
        o.push_str(" (");
        for (i, g) in t.generics.iter().enumerate() {
            if i > 0 {
                o.push(' ');
            }
            o.push_str(intern.get(g.name.name));
        }
        o.push(')');
    }
    o.push(' ');
    match &t.body {
        TypeBody::Alias(ty) => fmt_ty(o, ty, intern),
        TypeBody::Record(fs) => {
            o.push_str("(rec");
            for f in fs {
                o.push_str(" (");
                o.push_str(intern.get(f.name.name));
                o.push(' ');
                fmt_ty(o, &f.ty, intern);
                o.push(')');
            }
            o.push(')');
        }
        TypeBody::Variants(vs) => {
            o.push_str("(or");
            for v in vs {
                if v.fields.is_empty() {
                    o.push(' ');
                    o.push_str(intern.get(v.name.name));
                } else {
                    o.push_str(" (");
                    o.push_str(intern.get(v.name.name));
                    for f in &v.fields {
                        o.push_str(" (");
                        o.push_str(intern.get(f.name.name));
                        o.push(' ');
                        fmt_ty(o, &f.ty, intern);
                        o.push(')');
                    }
                    o.push(')');
                }
            }
            o.push(')');
        }
    }
    for inj in &t.injections {
        o.push_str(" (from ");
        fmt_ty(o, &inj.from, intern);
        o.push(' ');
        o.push_str(intern.get(inj.into_variant.name));
        o.push(')');
    }
    o.push(')');
}

fn fmt_effects(o: &mut String, e: &EffectRow, intern: &Interner) {
    o.push_str("(!");
    for it in &e.items {
        o.push(' ');
        match &it.kind {
            EffectKind::Err(t) => {
                o.push_str("(err ");
                fmt_ty(o, t, intern);
                o.push(')');
            }
            EffectKind::Io(id) => {
                o.push_str("(io ");
                o.push_str(intern.get(id.name));
                o.push(')');
            }
            EffectKind::Alloc(id) => {
                o.push_str("(alloc ");
                o.push_str(intern.get(id.name));
                o.push(')');
            }
            EffectKind::Susp => o.push_str("susp"),
            EffectKind::Diverge => o.push_str("diverge"),
            EffectKind::Race => o.push_str("race"),
            EffectKind::Nondet => o.push_str("nondet"),
            EffectKind::Abort => o.push_str("abort"),
        }
    }
    o.push(')');
}

fn fmt_ty(o: &mut String, t: &TypeExpr, intern: &Interner) {
    match &t.kind {
        TypeExprKind::Prim(p) => o.push_str(p.as_str()),
        TypeExprKind::Hole => o.push('?'),
        TypeExprKind::Own(inner) => {
            o.push_str("(own ");
            fmt_ty(o, inner, intern);
            o.push(')');
        }
        TypeExprKind::Untrusted(inner) => {
            o.push_str("(untrusted ");
            fmt_ty(o, inner, intern);
            o.push(')');
        }
        TypeExprKind::Secret(inner) => {
            o.push_str("(secret ");
            fmt_ty(o, inner, intern);
            o.push(')');
        }
        TypeExprKind::Ref {
            region,
            mutable,
            inner,
        } => {
            o.push_str("(ref");
            let rn = intern.get(region.name);
            if rn != "_" && !rn.is_empty() {
                o.push(' ');
                o.push_str(rn);
            }
            if *mutable {
                o.push_str(" mut");
            }
            o.push(' ');
            fmt_ty(o, inner, intern);
            o.push(')');
        }
        TypeExprKind::Fn {
            params,
            ret,
            effects,
        } => {
            o.push_str("(fn (");
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    o.push(' ');
                }
                fmt_ty(o, p, intern);
            }
            o.push_str(") ");
            fmt_ty(o, ret, intern);
            if !effects.items.is_empty() {
                o.push(' ');
                fmt_effects(o, effects, intern);
            }
            o.push(')');
        }
        TypeExprKind::Tuple(ts) => {
            o.push_str("(tuple");
            for t in ts {
                o.push(' ');
                fmt_ty(o, t, intern);
            }
            o.push(')');
        }
        TypeExprKind::Named { path, args } => {
            if args.is_empty() {
                o.push_str(&path_s(path, intern));
            } else {
                o.push('(');
                o.push_str(&path_s(path, intern));
                for a in args {
                    o.push(' ');
                    fmt_ty(o, a, intern);
                }
                o.push(')');
            }
        }
    }
}

fn fmt_expr(o: &mut String, e: &Expr, intern: &Interner, indent: usize) {
    match &e.kind {
        ExprKind::Lit(l) => fmt_lit(o, l),
        ExprKind::Hole => o.push('?'),
        ExprKind::Path(p) => o.push_str(&path_s(p, intern)),
        ExprKind::Call { callee, args } => {
            o.push('(');
            fmt_expr(o, callee, intern, indent);
            for a in args {
                o.push(' ');
                fmt_expr(o, a, intern, indent);
            }
            o.push(')');
        }
        ExprKind::Field { base, field } => {
            o.push_str("(field ");
            fmt_expr(o, base, intern, indent);
            o.push(' ');
            o.push_str(intern.get(field.name));
            o.push(')');
        }
        ExprKind::Index { base, index } => {
            o.push_str("(index ");
            fmt_expr(o, base, intern, indent);
            o.push(' ');
            fmt_expr(o, index, intern, indent);
            o.push(')');
        }
        ExprKind::Unary { op, expr } => {
            o.push('(');
            o.push_str(match op {
                UnOp::Not => "not",
                UnOp::BitNot => "bnot",
                UnOp::Neg => "neg",
                UnOp::Ref => "ref",
                UnOp::RefMut => "refmut",
                UnOp::Deref => "deref",
            });
            o.push(' ');
            fmt_expr(o, expr, intern, indent);
            o.push(')');
        }
        ExprKind::Binary { op, lhs, rhs } => {
            o.push('(');
            o.push_str(op.as_str());
            o.push(' ');
            fmt_expr(o, lhs, intern, indent);
            o.push(' ');
            fmt_expr(o, rhs, intern, indent);
            o.push(')');
        }
        ExprKind::Block { stmts, tail } => {
            o.push_str("(block");
            for s in stmts {
                o.push(' ');
                match &s.kind {
                    StmtKind::Let(l) => fmt_let(o, l, intern, indent),
                    StmtKind::Expr(x) => fmt_expr(o, x, intern, indent),
                }
            }
            if let Some(t) = tail {
                o.push(' ');
                fmt_expr(o, t, intern, indent);
            }
            o.push(')');
        }
        ExprKind::If {
            cond,
            then_b,
            else_b,
        } => {
            o.push_str("(if ");
            fmt_expr(o, cond, intern, indent);
            o.push(' ');
            fmt_expr(o, then_b, intern, indent);
            if let Some(el) = else_b {
                o.push(' ');
                fmt_expr(o, el, intern, indent);
            }
            o.push(')');
        }
        ExprKind::Match { scrut, arms } => {
            o.push_str("(match ");
            fmt_expr(o, scrut, intern, indent);
            for a in arms {
                o.push(' ');
                fmt_arm(o, a, intern, indent);
            }
            o.push(')');
        }
        ExprKind::For { pat, iter, body } => {
            o.push_str("(for ");
            fmt_pat(o, pat, intern);
            o.push(' ');
            fmt_expr(o, iter, intern, indent);
            o.push(' ');
            fmt_expr(o, body, intern, indent);
            o.push(')');
        }
        ExprKind::While { cond, body } => {
            o.push_str("(while ");
            fmt_expr(o, cond, intern, indent);
            o.push(' ');
            fmt_expr(o, body, intern, indent);
            o.push(')');
        }
        ExprKind::Loop { body } => {
            o.push_str("(loop ");
            fmt_expr(o, body, intern, indent);
            o.push(')');
        }
        ExprKind::Break => o.push_str("break"),
        ExprKind::Continue => o.push_str("continue"),
        ExprKind::Cast { expr: inner, ty } => {
            o.push_str("(as ");
            fmt_expr(o, inner, intern, indent);
            o.push(' ');
            fmt_ty(o, ty, intern);
            o.push(')');
        }
        ExprKind::Let(l) => fmt_let(o, l, intern, indent),
        ExprKind::Lambda { params, ret, body } => {
            o.push_str("(fn (");
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    o.push(' ');
                }
                o.push('(');
                o.push_str(intern.get(p.name.name));
                o.push(' ');
                fmt_ty(o, &p.ty, intern);
                o.push(')');
            }
            o.push(')');
            if let Some(r) = ret {
                o.push(' ');
                fmt_ty(o, r, intern);
            }
            o.push(' ');
            fmt_expr(o, body, intern, indent);
            o.push(')');
        }
        ExprKind::Record(fs) => {
            o.push_str("(rec");
            for (n, e) in fs {
                o.push_str(" (");
                o.push_str(intern.get(n.name));
                o.push(' ');
                fmt_expr(o, e, intern, indent);
                o.push(')');
            }
            o.push(')');
        }
        ExprKind::Variant { name, fields } => {
            o.push_str("(var ");
            o.push_str(intern.get(name.name));
            let positional = fields
                .iter()
                .all(|(n, _)| intern.get(n.name).starts_with('_'));
            for (n, e) in fields {
                o.push(' ');
                if positional {
                    fmt_expr(o, e, intern, indent);
                } else {
                    o.push('(');
                    o.push_str(intern.get(n.name));
                    o.push(' ');
                    fmt_expr(o, e, intern, indent);
                    o.push(')');
                }
            }
            o.push(')');
        }
        ExprKind::Return(inner) => {
            o.push_str("(return");
            if let Some(e) = inner {
                o.push(' ');
                fmt_expr(o, e, intern, indent);
            }
            o.push(')');
        }
        ExprKind::Raise(e) => {
            o.push_str("(raise ");
            fmt_expr(o, e, intern, indent);
            o.push(')');
        }
        ExprKind::Catch { expr, arms } => {
            o.push_str("(catch ");
            fmt_expr(o, expr, intern, indent);
            for a in arms {
                o.push(' ');
                fmt_arm(o, a, intern, indent);
            }
            o.push(')');
        }
        ExprKind::Attempt(e) => {
            o.push_str("(attempt ");
            fmt_expr(o, e, intern, indent);
            o.push(')');
        }
        ExprKind::Try(e) => {
            o.push_str("(try ");
            fmt_expr(o, e, intern, indent);
            o.push(')');
        }
        ExprKind::Interpolate { parts } => {
            o.push_str("(interp");
            for p in parts {
                o.push(' ');
                match p {
                    InterpPart::Lit(s) => {
                        o.push('"');
                        o.push_str(&escape(s));
                        o.push('"');
                    }
                    InterpPart::Expr(e) => fmt_expr(o, e, intern, indent),
                }
            }
            o.push(')');
        }
        ExprKind::Region { name, body } => {
            o.push_str("(region ");
            o.push_str(intern.get(name.name));
            o.push(' ');
            fmt_expr(o, body, intern, indent);
            o.push(')');
        }
        ExprKind::Par { bindings } => {
            o.push_str("(par");
            for l in bindings {
                o.push(' ');
                fmt_let(o, l, intern, indent);
            }
            o.push(')');
        }
        ExprKind::Assign { lhs, rhs } => {
            o.push_str("(set ");
            fmt_expr(o, lhs, intern, indent);
            o.push(' ');
            fmt_expr(o, rhs, intern, indent);
            o.push(')');
        }
    }
}

fn fmt_let(o: &mut String, l: &LetStmt, intern: &Interner, indent: usize) {
    o.push_str("(let ");
    if l.mutable {
        o.push_str("mut ");
    }
    fmt_pat(o, &l.pat, intern);
    if let Some(t) = &l.ty {
        o.push(' ');
        fmt_ty(o, t, intern);
    }
    o.push(' ');
    fmt_expr(o, &l.init, intern, indent);
    o.push(')');
}

fn fmt_arm(o: &mut String, a: &Arm, intern: &Interner, indent: usize) {
    o.push_str("(arm ");
    fmt_pat(o, &a.pat, intern);
    o.push(' ');
    fmt_expr(o, &a.body, intern, indent);
    o.push(')');
}

fn fmt_pat(o: &mut String, p: &Pattern, intern: &Interner) {
    match &p.kind {
        PatKind::Wild => o.push('_'),
        PatKind::Lit(l) => fmt_lit(o, l),
        PatKind::Bind(i) => o.push_str(intern.get(i.name)),
        PatKind::Variant { name, fields } => {
            o.push_str("(var ");
            o.push_str(intern.get(name.name));
            let positional = fields
                .iter()
                .all(|(n, _)| intern.get(n.name).starts_with('_'));
            for (n, p) in fields {
                o.push(' ');
                if positional {
                    fmt_pat(o, p, intern);
                } else {
                    o.push('(');
                    o.push_str(intern.get(n.name));
                    o.push(' ');
                    fmt_pat(o, p, intern);
                    o.push(')');
                }
            }
            o.push(')');
        }
        PatKind::Record(fs) => {
            o.push_str("(rec");
            for (n, p) in fs {
                o.push_str(" (");
                o.push_str(intern.get(n.name));
                o.push(' ');
                fmt_pat(o, p, intern);
                o.push(')');
            }
            o.push(')');
        }
        PatKind::Tuple(ps) => {
            o.push_str("(tuple");
            for p in ps {
                o.push(' ');
                fmt_pat(o, p, intern);
            }
            o.push(')');
        }
    }
}

fn fmt_lit(o: &mut String, l: &Lit) {
    match l {
        Lit::Unit => o.push_str("()"),
        Lit::Bool(b) => o.push_str(if *b { "true" } else { "false" }),
        Lit::Str(s) => {
            o.push('"');
            o.push_str(&escape(s));
            o.push('"');
        }
        Lit::Int { value, suffix } => {
            o.push_str(&value.to_string());
            if let Some(p) = suffix {
                o.push_str(p.as_str());
            }
        }
        Lit::Float { value, suffix } => {
            o.push_str(&value.to_string());
            if let Some(p) = suffix {
                o.push_str(p.as_str());
            }
        }
    }
}

fn path_s(p: &Path, intern: &Interner) -> String {
    p.segs
        .iter()
        .map(|s| intern.get(s.name))
        .collect::<Vec<_>>()
        .join(".")
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

/// Full-file GBNF for constrained decoding of the tree surface.
///
/// Heads are enumerated so a sampler cannot emit `(potato …)` as a form
/// the parser would have to reject. A call is still a list whose head is
/// a path, not a reserved head.
pub fn file_gbnf() -> &'static str {
    concat!(
        "root ::= file\n",
        "file ::= ws ( module | form+ )\n",
        "module ::= \"(module\" ws path ws form* \")\"\n",
        "form ::= decl | export | use | meta | expr\n",
        "export ::= \"(export\" ws ident* \")\"\n",
        "use ::= \"(use\" ws path (ws \"as\" ws ident)? \")\"\n",
        "meta ::= \"(@\" ws ident (ws atom)? \")\"\n",
        "decl ::= fn-decl | type-decl | dict-decl | test-decl | contract-decl\n",
        "fn-decl ::= \"(fn\" ws generics-opt ident ws params ws type-e ws effects-opt contract* expr \")\"\n",
        "contract-decl ::= \"(contract\" ws \"fn\" ws ident ws params ws type-e ws expr \")\"\n",
        "generics-opt ::= (\"(\" ws ident+ \")\" ws)?\n",
        "params ::= \"()\" | \"(\" ws param+ \")\"\n",
        "param ::= \"(\" ws ident ws type-e (ws \"default\")? \")\"\n",
        "effects-opt ::= (effects ws)?\n",
        "effects ::= \"(!\" ws effect* \")\"\n",
        "effect ::= \"susp\" | \"diverge\" | \"race\" | \"nondet\" | \"abort\" | \"(err\" ws type-e \")\" | \"(io\" ws ident \")\" | \"(alloc\" ws ident \")\"\n",
        "contract ::= \"(pre\" ws expr \")\" | \"(post\" ws expr \")\" | \"(inv\" ws expr \")\"\n",
        "type-decl ::= \"(type\" ws ident ws generics-opt type-body from* \")\"\n",
        "type-body ::= rec-type | or-type | type-e\n",
        "rec-type ::= \"(rec\" ws field-ty* \")\"\n",
        "or-type ::= \"(or\" ws variant-ty+ \")\"\n",
        "variant-ty ::= ident | \"(\" ws ident ws field-ty* \")\"\n",
        "field-ty ::= \"(\" ws ident ws type-e \")\"\n",
        "from ::= \"(from\" ws type-e ws ident \")\"\n",
        "dict-decl ::= \"(dict\" ws ident ws type-e ws field-ex* \")\"\n",
        "field-ex ::= \"(\" ws ident ws expr \")\"\n",
        "test-decl ::= \"(test\" ws string ws expr \")\"\n",
        "type-e ::= prim | \"?\" | named | type-list\n",
        "type-list ::= \"(own\" ws type-e \")\" | \"(untrusted\" ws type-e \")\" | \"(secret\" ws type-e \")\" | \"(ref\" ws (ident ws)? (\"mut\" ws)? type-e \")\" | \"(fn\" ws \"(\" type-e* \")\" ws type-e (ws effects)? \")\" | \"(tuple\" ws type-e* \")\" | \"(\" ws ident ws type-e+ \")\"\n",
        "named ::= ident\n",
        "prim ::= \"i8\"|\"i16\"|\"i32\"|\"i64\"|\"isz\"|\"u8\"|\"u16\"|\"u32\"|\"u64\"|\"usz\"|\"f32\"|\"f64\"|\"bool\"|\"byte\"|\"unit\"|\"String\"\n",
        "expr ::= atom | string | \"?\" | \"true\" | \"false\" | \"break\" | \"continue\" | \"()\" | list-expr\n",
        "list-expr ::= bin | un | special | call\n",
        "bin ::= \"(\" binop ws expr ws expr \")\"\n",
        "binop ::= \"+\"|\"-\"|\"*\"|\"/\"|\"%\"|\"==\"|\"!=\"|\"<\"|\"<=\"|\">\"|\">=\"|\"&&\"|\"||\"|\"&\"|\"|\"|\"^\"|\"<<\"|\">>\"\n",
        "un ::= \"(\" unop ws expr \")\"\n",
        "unop ::= \"not\"|\"bnot\"|\"neg\"|\"ref\"|\"refmut\"|\"deref\"\n",
        "special ::= if-e | match-e | let-e | set-e | block-e | for-e | while-e | loop-e | return-e | raise-e | catch-e | attempt-e | try-e | rec-e | var-e | field-e | index-e | as-e | fn-e | region-e | par-e | interp-e | arm-e\n",
        "if-e ::= \"(if\" ws expr ws expr (ws expr)? \")\"\n",
        "match-e ::= \"(match\" ws expr ws arm-e+ \")\"\n",
        "arm-e ::= \"(arm\" ws pat ws expr \")\"\n",
        "let-e ::= \"(let\" ws (\"mut\" ws)? pat (ws type-e)? ws expr \")\"\n",
        "set-e ::= \"(set\" ws expr ws expr \")\"\n",
        "block-e ::= \"(block\" ws form-or-expr* \")\"\n",
        "form-or-expr ::= let-e | set-e | expr\n",
        "for-e ::= \"(for\" ws pat ws expr ws expr \")\"\n",
        "while-e ::= \"(while\" ws expr ws expr \")\"\n",
        "loop-e ::= \"(loop\" ws expr \")\"\n",
        "return-e ::= \"(return\" (ws expr)? \")\"\n",
        "raise-e ::= \"(raise\" ws expr \")\"\n",
        "catch-e ::= \"(catch\" ws expr ws arm-e+ \")\"\n",
        "attempt-e ::= \"(attempt\" ws expr \")\"\n",
        "try-e ::= \"(try\" ws expr \")\"\n",
        "rec-e ::= \"(rec\" ws field-ex* \")\"\n",
        "var-e ::= \"(var\" ws ident ws expr* \")\"\n",
        "field-e ::= \"(field\" ws expr ws ident \")\"\n",
        "index-e ::= \"(index\" ws expr ws expr \")\"\n",
        "as-e ::= \"(as\" ws expr ws type-e \")\"\n",
        "fn-e ::= \"(fn\" ws params (ws type-e)? ws expr \")\"\n",
        "region-e ::= \"(region\" ws ident ws expr \")\"\n",
        "par-e ::= \"(par\" ws let-e* \")\"\n",
        "interp-e ::= \"(interp\" ws (string | expr)+ \")\"\n",
        "call ::= \"(\" ws path ws expr* \")\"\n",
        "pat ::= \"_\" | atom | string | \"true\" | \"false\" | rec-pat | tuple-pat | var-pat | call-pat\n",
        "rec-pat ::= \"(rec\" ws field-pat* \")\"\n",
        "field-pat ::= \"(\" ws ident ws pat \")\"\n",
        "tuple-pat ::= \"(tuple\" ws pat* \")\"\n",
        "var-pat ::= \"(var\" ws ident ws pat* \")\"\n",
        "call-pat ::= \"(\" ws ident ws pat* \")\"\n",
        "path ::= ident (\".\" ident)*\n",
        "ident ::= [A-Za-z_] [A-Za-z0-9_]*\n",
        "atom ::= [A-Za-z0-9_+\\-*/%<>=!&|.?~^]+\n",
        "string ::= \"\\\"\" [^\\\"]* \"\\\"\"\n",
        "ws ::= [ \\t\\n\\r]*\n",
    )
}
