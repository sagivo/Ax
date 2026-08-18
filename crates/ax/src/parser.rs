//! Recursive-descent parser. Generated conceptually from spec/grammar.ebnf.
//! One AST; this is the S-conventional frontend.

use crate::ast::*;
use crate::diag::{Diagnostic, Severity};
use crate::intern::{Interner, Symbol};
use crate::lexer::{LexError, Lexer, Token, TokenKind};
use crate::span::{FileId, Span};
use crate::types::Prim;

pub struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    intern: &'a mut Interner,
    #[allow(dead_code)]
    file: FileId,
    pub diags: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    pub fn parse_file(
        src: &str,
        file: FileId,
        intern: &'a mut Interner,
    ) -> Result<File, Vec<Diagnostic>> {
        let mut lexer = Lexer::new(src, file, intern);
        let tokens = match lexer.tokenize() {
            Ok(t) => t,
            Err(LexError { span, msg }) => {
                return Err(vec![Diagnostic::error("E0001", span, msg)]);
            }
        };
        let mut p = Parser {
            tokens,
            pos: 0,
            intern,
            file,
            diags: Vec::new(),
        };
        match p.parse_file_inner() {
            Ok(mut f) if p.diags.is_empty() => {
                // Every AST leaves the parser numbered; the checker's type
                // tables are indexed by these ids.
                crate::ast::renumber(&mut f);
                Ok(f)
            }
            Ok(_) => Err(std::mem::take(&mut p.diags)),
            Err(()) => Err(std::mem::take(&mut p.diags)),
        }
    }

    fn parse_file_inner(&mut self) -> Result<File, ()> {
        let start = self.cur().span;
        self.expect(TokenKind::Module)?;
        let module = self.parse_path()?;
        self.expect(TokenKind::Semi)?;

        let mut exports = Vec::new();
        if self.at(TokenKind::Export) {
            self.bump();
            self.expect(TokenKind::LBrace)?;
            if !self.at(TokenKind::RBrace) {
                loop {
                    exports.push(self.parse_ident()?);
                    if self.at(TokenKind::Comma) {
                        self.bump();
                        continue;
                    }
                    break;
                }
            }
            self.expect(TokenKind::RBrace)?;
            self.expect(TokenKind::Semi)?;
        }

        let mut uses = Vec::new();
        while self.at(TokenKind::Use) {
            let us = self.cur().span;
            self.bump();
            let path = self.parse_path()?;
            let alias = if self.at(TokenKind::As) {
                self.bump();
                Some(self.parse_ident()?)
            } else {
                None
            };
            self.expect(TokenKind::Semi)?;
            uses.push(UseDecl {
                path,
                alias,
                span: us.merge(self.prev_span()),
            });
        }

        let mut decls = Vec::new();
        while !self.at(TokenKind::Eof) {
            decls.push(self.parse_decl()?);
        }

        Ok(File {
            node_count: 0,
            module,
            exports,
            uses,
            decls,
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_decl(&mut self) -> Result<Decl, ()> {
        let start = self.cur().span;
        let mut meta = Vec::new();
        while self.at(TokenKind::At) {
            meta.push(self.parse_meta()?);
        }
        let kind = if self.at(TokenKind::Contract) {
            self.bump();
            self.expect(TokenKind::Fn)?;
            DeclKind::ContractFn(self.parse_fn_rest()?)
        } else if self.at(TokenKind::Fn) {
            self.bump();
            DeclKind::Fn(self.parse_fn_rest()?)
        } else if self.at(TokenKind::Type) {
            self.bump();
            DeclKind::Type(self.parse_type_decl()?)
        } else if self.at(TokenKind::Dict) {
            self.bump();
            DeclKind::Dict(self.parse_dict_decl()?)
        } else if self.at(TokenKind::Test) {
            self.bump();
            DeclKind::Test(self.parse_test_decl()?)
        } else {
            self.err("E0002", "expected declaration (fn, type, dict, test)");
            return Err(());
        };
        Ok(Decl {
            meta,
            kind,
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_meta(&mut self) -> Result<Meta, ()> {
        let start = self.cur().span;
        self.expect(TokenKind::At)?;
        let key = self.parse_ident()?;
        let value = if self.at(TokenKind::String) {
            let t = self.bump();
            Some(MetaValue::String(self.intern.get(t.symbol).to_string()))
        } else if self.at(TokenKind::Ident) {
            let first = self.parse_ident()?;
            // Allow hyphenated meta values (`REQ-118`) without making `-` identifier syntax.
            if self.at(TokenKind::Minus) {
                let mut text = self.intern.get(first.name).to_string();
                while self.at(TokenKind::Minus) {
                    self.bump();
                    text.push('-');
                    if self.at(TokenKind::Ident) || self.at(TokenKind::Integer) {
                        let t = self.bump();
                        text.push_str(self.intern.get(t.symbol));
                    }
                }
                let sym = self.intern.intern(&text);
                Some(MetaValue::Ident(Ident {
                    name: sym,
                    span: first.span.merge(self.prev_span()),
                }))
            } else {
                Some(MetaValue::Ident(first))
            }
        } else if self.at(TokenKind::Integer) {
            let t = self.bump();
            let n = parse_int_text(self.intern.get(t.symbol)).unwrap_or(0) as i64;
            Some(MetaValue::Int(n))
        } else {
            None
        };
        Ok(Meta {
            key,
            value,
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_fn_rest(&mut self) -> Result<FnDecl, ()> {
        let name = self.parse_ident()?;
        let generics = if self.at(TokenKind::LBracket) {
            self.parse_generics()?
        } else {
            Vec::new()
        };
        self.expect(TokenKind::LParen)?;
        let params = if self.at(TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_params()?
        };
        self.expect(TokenKind::RParen)?;
        self.expect(TokenKind::Arrow)?;
        let ret = self.parse_type()?;
        let effects = if self.at(TokenKind::BangLBrace) {
            self.parse_effects()?
        } else {
            EffectRow {
                omitted: true,
                ..EffectRow::default()
            }
        };
        let mut contracts = Vec::new();
        while matches!(
            self.cur().kind,
            TokenKind::Pre | TokenKind::Post | TokenKind::Inv
        ) {
            contracts.push(self.parse_contract()?);
        }
        self.expect(TokenKind::Eq)?;
        let body = self.parse_expr()?;
        self.expect(TokenKind::Semi)?;
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

    fn parse_generics(&mut self) -> Result<Vec<GParam>, ()> {
        self.expect(TokenKind::LBracket)?;
        let mut gs = Vec::new();
        if !self.at(TokenKind::RBracket) {
            loop {
                let name = self.parse_ident()?;
                let bound = if self.at(TokenKind::Colon) {
                    self.bump();
                    Some(self.parse_type()?)
                } else {
                    None
                };
                gs.push(GParam { name, bound });
                if self.at(TokenKind::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RBracket)?;
        Ok(gs)
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, ()> {
        let mut ps = Vec::new();
        loop {
            let name = self.parse_ident()?;
            self.expect(TokenKind::Colon)?;
            let ty = self.parse_type()?;
            let default_dict = if self.at(TokenKind::Eq) {
                self.bump();
                self.expect(TokenKind::Default)?;
                true
            } else {
                false
            };
            ps.push(Param {
                name,
                ty,
                default_dict,
            });
            if self.at(TokenKind::Comma) {
                self.bump();
                if self.at(TokenKind::RParen) {
                    break;
                }
                continue;
            }
            break;
        }
        Ok(ps)
    }

    fn parse_effects(&mut self) -> Result<EffectRow, ()> {
        let start = self.cur().span;
        self.expect(TokenKind::BangLBrace)?;
        let mut items = Vec::new();
        if !self.at(TokenKind::RBrace) {
            loop {
                items.push(self.parse_effect()?);
                if self.at(TokenKind::Comma) {
                    self.bump();
                    if self.at(TokenKind::RBrace) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(EffectRow {
            items,
            span: start.merge(self.prev_span()),
            omitted: false,
        })
    }

    fn parse_effect(&mut self) -> Result<Effect, ()> {
        let start = self.cur().span;
        let ident = self.parse_ident()?;
        let name = self.intern.get(ident.name).to_string();
        let kind = match name.as_str() {
            "err" => {
                self.expect(TokenKind::LBracket)?;
                let t = self.parse_type()?;
                self.expect(TokenKind::RBracket)?;
                EffectKind::Err(t)
            }
            "io" => {
                self.expect(TokenKind::LBracket)?;
                let c = self.parse_ident()?;
                self.expect(TokenKind::RBracket)?;
                EffectKind::Io(c)
            }
            "alloc" => {
                self.expect(TokenKind::LBracket)?;
                let a = self.parse_ident()?;
                self.expect(TokenKind::RBracket)?;
                EffectKind::Alloc(a)
            }
            "susp" => EffectKind::Susp,
            "diverge" => EffectKind::Diverge,
            "race" => EffectKind::Race,
            "nondet" => EffectKind::Nondet,
            "abort" => EffectKind::Abort,
            _ => {
                self.err_span(ident.span, "E0003", format!("unknown effect `{name}`"));
                EffectKind::Abort
            }
        };
        Ok(Effect {
            kind,
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_contract(&mut self) -> Result<Contract, ()> {
        let start = self.cur().span;
        let kind = match self.cur().kind {
            TokenKind::Pre => ContractKind::Pre,
            TokenKind::Post => ContractKind::Post,
            TokenKind::Inv => ContractKind::Inv,
            _ => unreachable!(),
        };
        self.bump();
        // Contracts must not consume the `=` that starts the function body.
        let expr = self.parse_or()?;
        Ok(Contract {
            kind,
            expr,
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_type_decl(&mut self) -> Result<TypeDecl, ()> {
        let name = self.parse_ident()?;
        let generics = if self.at(TokenKind::LBracket) {
            self.parse_generics()?
        } else {
            Vec::new()
        };
        self.expect(TokenKind::Eq)?;
        let body = if self.at(TokenKind::LBrace) && self.looks_like_record_type() {
            TypeBody::Record(self.parse_record_fields()?)
        } else if self.at(TokenKind::Pipe) {
            TypeBody::Variants(self.parse_variants()?)
        } else {
            TypeBody::Alias(self.parse_type()?)
        };
        let mut injections = Vec::new();
        if self.at(TokenKind::With) {
            self.bump();
            while self.at(TokenKind::From) {
                let start = self.cur().span;
                self.bump();
                let from = self.parse_type()?;
                self.expect(TokenKind::FatArrow)?;
                let into_variant = self.parse_ident()?;
                self.expect(TokenKind::Semi)?;
                injections.push(Injection {
                    from,
                    into_variant,
                    span: start.merge(self.prev_span()),
                });
            }
            // Each injection already ends with `;`. A further terminator is
            // optional so spec examples (Appendix A) parse as written.
            if self.at(TokenKind::Semi) {
                self.bump();
            }
        } else {
            self.expect(TokenKind::Semi)?;
        }
        Ok(TypeDecl {
            name,
            generics,
            body,
            injections,
        })
    }

    fn looks_like_record_type(&self) -> bool {
        // `{ ident :` is a record type; `{ ident :` could also be record lit
        // in expr position. Here we're in type position so `{` starts a record.
        true
    }

    fn parse_record_fields(&mut self) -> Result<Vec<Field>, ()> {
        self.expect(TokenKind::LBrace)?;
        let mut fields = Vec::new();
        if !self.at(TokenKind::RBrace) {
            loop {
                let name = self.parse_ident()?;
                self.expect(TokenKind::Colon)?;
                let ty = self.parse_type()?;
                fields.push(Field { name, ty });
                if self.at(TokenKind::Comma) {
                    self.bump();
                    if self.at(TokenKind::RBrace) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(fields)
    }

    fn parse_variants(&mut self) -> Result<Vec<Variant>, ()> {
        let mut vs = Vec::new();
        while self.at(TokenKind::Pipe) {
            self.bump();
            let name = self.parse_ident()?;
            let fields = if self.at(TokenKind::LBrace) {
                self.parse_record_fields()?
            } else {
                Vec::new()
            };
            vs.push(Variant { name, fields });
        }
        Ok(vs)
    }

    fn parse_dict_decl(&mut self) -> Result<DictDecl, ()> {
        let name = self.parse_ident()?;
        self.expect(TokenKind::LBracket)?;
        let for_ty = self.parse_type()?;
        self.expect(TokenKind::RBracket)?;
        self.expect(TokenKind::Eq)?;
        let rec = self.parse_record_lit_fields()?;
        self.expect(TokenKind::Semi)?;
        Ok(DictDecl {
            name,
            for_ty,
            fields: rec,
        })
    }

    fn parse_test_decl(&mut self) -> Result<TestDecl, ()> {
        if !self.at(TokenKind::String) {
            self.err("E0004", "expected test name string");
            return Err(());
        }
        let t = self.bump();
        let name = self.intern.get(t.symbol).to_string();
        self.expect(TokenKind::Eq)?;
        let body = self.parse_expr()?;
        self.expect(TokenKind::Semi)?;
        Ok(TestDecl { name, body })
    }

    // ---------- types ----------

    fn parse_type_args(&mut self) -> Result<Vec<TypeExpr>, ()> {
        self.expect(TokenKind::LBracket)?;
        let mut args = Vec::new();
        if !self.at(TokenKind::RBracket) {
            loop {
                args.push(self.parse_type()?);
                if self.at(TokenKind::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RBracket)?;
        Ok(args)
    }

    fn can_start_type(&self) -> bool {
        matches!(
            self.cur().kind,
            TokenKind::Ident
                | TokenKind::Fn
                | TokenKind::Amp
                | TokenKind::Own
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::Question
                | TokenKind::Mut
        )
    }

    fn parse_type(&mut self) -> Result<TypeExpr, ()> {
        let start = self.cur().span;
        if self.at(TokenKind::Fn) {
            self.bump();
            self.expect(TokenKind::LParen)?;
            let mut params = Vec::new();
            if !self.at(TokenKind::RParen) {
                loop {
                    params.push(self.parse_type()?);
                    if self.at(TokenKind::Comma) {
                        self.bump();
                        continue;
                    }
                    break;
                }
            }
            self.expect(TokenKind::RParen)?;
            self.expect(TokenKind::Arrow)?;
            let ret = Box::new(self.parse_type()?);
            let effects = if self.at(TokenKind::BangLBrace) {
                self.parse_effects()?
            } else {
                EffectRow::default()
            };
            return Ok(TypeExpr {
                kind: TypeExprKind::Fn {
                    params,
                    ret,
                    effects,
                },
                span: start.merge(self.prev_span()),
            });
        }
        if self.at(TokenKind::Amp) {
            self.bump();
            // Forms: &T | &mut T | &r T | &r mut T | &[T] | &mut [T]
            let anon = self.intern.intern("_");
            if self.at(TokenKind::Mut) {
                self.bump();
                let inner = Box::new(self.parse_type()?);
                return Ok(TypeExpr {
                    kind: TypeExprKind::Ref {
                        region: Ident {
                            name: anon,
                            span: start,
                        },
                        mutable: true,
                        inner,
                    },
                    span: start.merge(self.prev_span()),
                });
            }
            if self.at(TokenKind::LBracket) {
                let inner = Box::new(self.parse_type()?);
                return Ok(TypeExpr {
                    kind: TypeExprKind::Ref {
                        region: Ident {
                            name: anon,
                            span: start,
                        },
                        mutable: false,
                        inner,
                    },
                    span: start.merge(self.prev_span()),
                });
            }
            if self.at(TokenKind::Ident) {
                let ident = self.parse_ident()?;
                let n = self.intern.get(ident.name).to_string();
                let next_is_type = self.can_start_type();
                if next_is_type {
                    let mutable = if self.at(TokenKind::Mut) {
                        self.bump();
                        true
                    } else {
                        false
                    };
                    let inner = Box::new(self.parse_type()?);
                    return Ok(TypeExpr {
                        kind: TypeExprKind::Ref {
                            region: ident,
                            mutable,
                            inner,
                        },
                        span: start.merge(self.prev_span()),
                    });
                }
                // `&str` / `&Rec` — ident is the type, region elided
                let args = if self.at(TokenKind::LBracket) {
                    self.parse_type_args()?
                } else {
                    Vec::new()
                };
                let named = if let Some(p) = Prim::from_str(&n) {
                    TypeExpr {
                        kind: TypeExprKind::Prim(p),
                        span: ident.span,
                    }
                } else {
                    TypeExpr {
                        kind: TypeExprKind::Named {
                            path: Path {
                                segs: vec![ident.clone()],
                                span: ident.span,
                            },
                            args,
                        },
                        span: ident.span,
                    }
                };
                return Ok(TypeExpr {
                    kind: TypeExprKind::Ref {
                        region: Ident {
                            name: anon,
                            span: start,
                        },
                        mutable: false,
                        inner: Box::new(named),
                    },
                    span: start.merge(self.prev_span()),
                });
            }
            let inner = Box::new(self.parse_type()?);
            return Ok(TypeExpr {
                kind: TypeExprKind::Ref {
                    region: Ident {
                        name: anon,
                        span: start,
                    },
                    mutable: false,
                    inner,
                },
                span: start.merge(self.prev_span()),
            });
        }
        if self.at(TokenKind::Own) {
            // `own T` is reserved, not implemented. v1 has no introduction form
            // for it and no move checker, so a program using it would get a
            // unique-ownership guarantee nothing enforces. Regions plus `&mut`
            // exclusivity carry the memory model instead.
            self.err(
                "E0009",
                "`own` is reserved for a future version: v1 has no move checker, \
                 so uniqueness would be unenforced. Use a region for ownership \
                 with a lexical lifetime, or `&mut` for exclusive access",
            );
            self.bump();
            let inner = Box::new(self.parse_type()?);
            return Ok(TypeExpr {
                kind: TypeExprKind::Own(inner),
                span: start.merge(self.prev_span()),
            });
        }
        if self.at(TokenKind::LParen) {
            self.bump();
            let first = self.parse_type()?;
            if self.at(TokenKind::Comma) {
                let mut ts = vec![first];
                while self.at(TokenKind::Comma) {
                    self.bump();
                    if self.at(TokenKind::RParen) {
                        break;
                    }
                    ts.push(self.parse_type()?);
                }
                self.expect(TokenKind::RParen)?;
                return Ok(TypeExpr {
                    kind: TypeExprKind::Tuple(ts),
                    span: start.merge(self.prev_span()),
                });
            }
            self.expect(TokenKind::RParen)?;
            return Ok(first);
        }
        if self.at(TokenKind::Question) {
            self.bump();
            return Ok(TypeExpr {
                kind: TypeExprKind::Hole,
                span: start,
            });
        }
        // `[T]` slice type
        if self.at(TokenKind::LBracket) {
            self.bump();
            let inner = self.parse_type()?;
            self.expect(TokenKind::RBracket)?;
            let slice = self.intern.intern("slice");
            return Ok(TypeExpr {
                kind: TypeExprKind::Named {
                    path: Path {
                        segs: vec![Ident {
                            name: slice,
                            span: start,
                        }],
                        span: start.merge(self.prev_span()),
                    },
                    args: vec![inner],
                },
                span: start.merge(self.prev_span()),
            });
        }
        // named / primitive
        let path = self.parse_path()?;
        if path.segs.len() == 1 {
            let n = self.intern.get(path.segs[0].name);
            if let Some(p) = Prim::from_str(n) {
                return Ok(TypeExpr {
                    kind: TypeExprKind::Prim(p),
                    span: start.merge(self.prev_span()),
                });
            }
        }
        let args = if self.at(TokenKind::LBracket) {
            self.parse_type_args()?
        } else {
            Vec::new()
        };
        Ok(TypeExpr {
            kind: TypeExprKind::Named { path, args },
            span: start.merge(self.prev_span()),
        })
    }

    // ---------- expressions ----------

    fn parse_expr(&mut self) -> Result<Expr, ()> {
        let lhs = self.parse_or()?;
        if self.at(TokenKind::Eq) {
            self.bump();
            let rhs = self.parse_expr()?;
            let span = lhs.span.merge(rhs.span);
            return Ok(Expr { id: NodeId::NONE,
                kind: ExprKind::Assign {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            });
        }
        Ok(lhs)
    }

    fn parse_or(&mut self) -> Result<Expr, ()> {
        let mut e = self.parse_and()?;
        while self.at(TokenKind::OrOr) {
            let op_span = self.bump().span;
            let rhs = self.parse_and()?;
            let span = e.span.merge(rhs.span);
            e = Expr { id: NodeId::NONE,
                kind: ExprKind::Binary {
                    op: BinOp::Or,
                    lhs: Box::new(e),
                    rhs: Box::new(rhs),
                },
                span: span.merge(op_span),
            };
        }
        Ok(e)
    }

    fn parse_and(&mut self) -> Result<Expr, ()> {
        let mut e = self.parse_cmp()?;
        while self.at(TokenKind::AndAnd) {
            self.bump();
            let rhs = self.parse_cmp()?;
            let span = e.span.merge(rhs.span);
            e = Expr { id: NodeId::NONE,
                kind: ExprKind::Binary {
                    op: BinOp::And,
                    lhs: Box::new(e),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(e)
    }

    fn parse_cmp(&mut self) -> Result<Expr, ()> {
        let lhs = self.parse_bitor()?;
        let op = match self.cur().kind {
            TokenKind::EqEq => Some(BinOp::Eq),
            TokenKind::Ne => Some(BinOp::Ne),
            TokenKind::Lt => Some(BinOp::Lt),
            TokenKind::Le => Some(BinOp::Le),
            TokenKind::Gt => Some(BinOp::Gt),
            TokenKind::Ge => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let rhs = self.parse_bitor()?;
            let span = lhs.span.merge(rhs.span);
            return Ok(Expr { id: NodeId::NONE,
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            });
        }
        Ok(lhs)
    }

    /// Bitwise levels, in Rust's order: `|` binds loosest, then `^`, then `&`,
    /// then the shifts. Spelling them out (rather than one table) keeps each
    /// level's operand parser explicit.
    fn parse_bitor(&mut self) -> Result<Expr, ()> {
        let mut e = self.parse_bitxor()?;
        while self.at(TokenKind::Pipe) {
            self.bump();
            let rhs = self.parse_bitxor()?;
            let span = e.span.merge(rhs.span);
            e = Expr {
                id: NodeId::NONE,
                kind: ExprKind::Binary {
                    op: BinOp::BitOr,
                    lhs: Box::new(e),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(e)
    }

    fn parse_bitxor(&mut self) -> Result<Expr, ()> {
        let mut e = self.parse_bitand()?;
        while self.at(TokenKind::Caret) {
            self.bump();
            let rhs = self.parse_bitand()?;
            let span = e.span.merge(rhs.span);
            e = Expr {
                id: NodeId::NONE,
                kind: ExprKind::Binary {
                    op: BinOp::BitXor,
                    lhs: Box::new(e),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(e)
    }

    fn parse_bitand(&mut self) -> Result<Expr, ()> {
        let mut e = self.parse_shift()?;
        // `&` is also the reference operator, but only in prefix position, so an
        // infix `&` here is unambiguous.
        while self.at(TokenKind::Amp) {
            self.bump();
            let rhs = self.parse_shift()?;
            let span = e.span.merge(rhs.span);
            e = Expr {
                id: NodeId::NONE,
                kind: ExprKind::Binary {
                    op: BinOp::BitAnd,
                    lhs: Box::new(e),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(e)
    }

    fn parse_shift(&mut self) -> Result<Expr, ()> {
        let mut e = self.parse_add()?;
        loop {
            let op = match self.cur().kind {
                TokenKind::Shl => BinOp::Shl,
                TokenKind::Shr => BinOp::Shr,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_add()?;
            let span = e.span.merge(rhs.span);
            e = Expr {
                id: NodeId::NONE,
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(e),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(e)
    }

    fn parse_add(&mut self) -> Result<Expr, ()> {
        let mut e = self.parse_mul()?;
        loop {
            let op = match self.cur().kind {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_mul()?;
            let span = e.span.merge(rhs.span);
            e = Expr { id: NodeId::NONE,
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(e),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(e)
    }

    fn parse_mul(&mut self) -> Result<Expr, ()> {
        let mut e = self.parse_cast()?;
        loop {
            let op = match self.cur().kind {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Rem,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_cast()?;
            let span = e.span.merge(rhs.span);
            e = Expr { id: NodeId::NONE,
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(e),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(e)
    }

    /// `expr as T`. The only numeric conversion in the language: there are no
    /// implicit ones, so without this there is no way to change a value's width.
    fn parse_cast(&mut self) -> Result<Expr, ()> {
        let mut e = self.parse_unary()?;
        while self.at(TokenKind::As) {
            self.bump();
            let ty = self.parse_type()?;
            let span = e.span.merge(ty.span);
            e = Expr {
                id: NodeId::NONE,
                kind: ExprKind::Cast {
                    expr: Box::new(e),
                    ty,
                },
                span,
            };
        }
        Ok(e)
    }

    fn parse_unary(&mut self) -> Result<Expr, ()> {
        let start = self.cur().span;
        if self.at(TokenKind::Bang) {
            self.bump();
            let e = self.parse_unary()?;
            return Ok(Expr { id: NodeId::NONE,
                kind: ExprKind::Unary {
                    op: UnOp::Not,
                    expr: Box::new(e),
                },
                span: start.merge(self.prev_span()),
            });
        }
        if self.at(TokenKind::Tilde) {
            self.bump();
            let e = self.parse_unary()?;
            return Ok(Expr {
                id: NodeId::NONE,
                kind: ExprKind::Unary {
                    op: UnOp::BitNot,
                    expr: Box::new(e),
                },
                span: start.merge(self.prev_span()),
            });
        }
        if self.at(TokenKind::Minus) {
            self.bump();
            let e = self.parse_unary()?;
            return Ok(Expr { id: NodeId::NONE,
                kind: ExprKind::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(e),
                },
                span: start.merge(self.prev_span()),
            });
        }
        if self.at(TokenKind::Amp) {
            self.bump();
            let mut_ = if self.at(TokenKind::Mut) {
                self.bump();
                true
            } else {
                false
            };
            let e = self.parse_unary()?;
            return Ok(Expr { id: NodeId::NONE,
                kind: ExprKind::Unary {
                    op: if mut_ { UnOp::RefMut } else { UnOp::Ref },
                    expr: Box::new(e),
                },
                span: start.merge(self.prev_span()),
            });
        }
        if self.at(TokenKind::Star) {
            self.bump();
            let e = self.parse_unary()?;
            return Ok(Expr { id: NodeId::NONE,
                kind: ExprKind::Unary {
                    op: UnOp::Deref,
                    expr: Box::new(e),
                },
                span: start.merge(self.prev_span()),
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, ()> {
        let mut e = self.parse_primary()?;
        loop {
            if self.at(TokenKind::LParen) {
                self.bump();
                let mut args = Vec::new();
                if !self.at(TokenKind::RParen) {
                    loop {
                        args.push(self.parse_expr()?);
                        if self.at(TokenKind::Comma) {
                            self.bump();
                            if self.at(TokenKind::RParen) {
                                break;
                            }
                            continue;
                        }
                        break;
                    }
                }
                self.expect(TokenKind::RParen)?;
                let span = e.span.merge(self.prev_span());
                e = Expr { id: NodeId::NONE,
                    kind: ExprKind::Call {
                        callee: Box::new(e),
                        args,
                    },
                    span,
                };
                continue;
            }
            if self.at(TokenKind::LBracket) {
                self.bump();
                let index = self.parse_expr()?;
                self.expect(TokenKind::RBracket)?;
                let span = e.span.merge(self.prev_span());
                e = Expr { id: NodeId::NONE,
                    kind: ExprKind::Index {
                        base: Box::new(e),
                        index: Box::new(index),
                    },
                    span,
                };
                continue;
            }
            if self.at(TokenKind::Dot) {
                self.bump();
                let field = self.parse_ident()?;
                let span = e.span.merge(field.span);
                e = Expr { id: NodeId::NONE,
                    kind: ExprKind::Field {
                        base: Box::new(e),
                        field,
                    },
                    span,
                };
                continue;
            }
            if self.at(TokenKind::Question) {
                // `?` as postfix try-op is reserved; holes are primary.
                // Spec lists `?` as both hole and tryop. We treat a lone `?`
                // as a hole (primary) and postfix `?` as a hole-marker no-op
                // for now (holes are first-class expressions).
                break;
            }
            break;
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> Result<Expr, ()> {
        let start = self.cur().span;
        match self.cur().kind {
            TokenKind::Integer => {
                let t = self.bump();
                let text = self.intern.get(t.symbol).to_string();
                let (value, suffix) = parse_int(&text);
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Lit(Lit::Int { value, suffix }),
                    span: t.span,
                })
            }
            TokenKind::Float => {
                let t = self.bump();
                let text = self.intern.get(t.symbol).to_string();
                let (value, suffix) = parse_float(&text);
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Lit(Lit::Float { value, suffix }),
                    span: t.span,
                })
            }
            TokenKind::String => {
                let t = self.bump();
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Lit(Lit::Str(self.intern.get(t.symbol).to_string())),
                    span: t.span,
                })
            }
            TokenKind::True => {
                let t = self.bump();
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Lit(Lit::Bool(true)),
                    span: t.span,
                })
            }
            TokenKind::False => {
                let t = self.bump();
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Lit(Lit::Bool(false)),
                    span: t.span,
                })
            }
            TokenKind::Question => {
                let t = self.bump();
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Hole,
                    span: t.span,
                })
            }
            TokenKind::LBrace => self.parse_block_or_record(),
            TokenKind::LParen => {
                self.bump();
                if self.at(TokenKind::RParen) {
                    let t = self.bump();
                    return Ok(Expr { id: NodeId::NONE,
                        kind: ExprKind::Lit(Lit::Unit),
                        span: start.merge(t.span),
                    });
                }
                let e = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                Ok(e)
            }
            TokenKind::If => self.parse_if(),
            TokenKind::Match => self.parse_match(),
            TokenKind::For => self.parse_for(),
            TokenKind::Loop => {
                self.bump();
                let body = self.parse_block_expr()?;
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Loop {
                        body: Box::new(body),
                    },
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::While => {
                self.bump();
                let cond = self.parse_expr()?;
                let body = self.parse_block_expr()?;
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::While {
                        cond: Box::new(cond),
                        body: Box::new(body),
                    },
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::Break => {
                self.bump();
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Break,
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::Continue => {
                self.bump();
                Ok(Expr {
                    id: NodeId::NONE,
                    kind: ExprKind::Continue,
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::Let => {
                let l = self.parse_let_stmt()?;
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Let(Box::new(l)),
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::Fn => {
                self.bump();
                self.expect(TokenKind::LParen)?;
                let params = if self.at(TokenKind::RParen) {
                    Vec::new()
                } else {
                    self.parse_params()?
                };
                self.expect(TokenKind::RParen)?;
                let ret = if self.at(TokenKind::Arrow) {
                    self.bump();
                    Some(self.parse_type()?)
                } else {
                    None
                };
                self.expect(TokenKind::FatArrow)?;
                let body = self.parse_expr()?;
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Lambda {
                        params,
                        ret,
                        body: Box::new(body),
                    },
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::Return => {
                self.bump();
                let e = if self.can_start_expr() {
                    Some(Box::new(self.parse_expr()?))
                } else {
                    None
                };
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Return(e),
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::Raise => {
                self.bump();
                let e = self.parse_expr()?;
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Raise(Box::new(e)),
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::Catch => self.parse_catch(),
            TokenKind::Attempt => {
                self.bump();
                let e = self.parse_expr()?;
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Attempt(Box::new(e)),
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::Region => {
                self.bump();
                let name = self.parse_ident()?;
                let body = self.parse_block_expr()?;
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Region {
                        name,
                        body: Box::new(body),
                    },
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::Par => self.parse_par(),
            TokenKind::Ident | TokenKind::Test => {
                // Expression names are a single segment. Dots are postfix
                // field/method access so `recs.at(i)` is Call(Field(recs, at)).
                // Qualified prelude calls (`fs.read`, `test.alloc`) are
                // reconstructed in the checker.
                if self.cur().kind == TokenKind::Test && self.peek_kind(1) != TokenKind::Dot {
                    self.err(
                        "E0005",
                        format!("expected expression, got {:?}", self.cur().kind),
                    );
                    return Err(());
                }
                let name = self.parse_path_seg()?;
                if self.at(TokenKind::LBrace) && self.looks_like_record_lit() {
                    let fields = self.parse_record_lit_fields()?;
                    return Ok(Expr { id: NodeId::NONE,
                        kind: ExprKind::Variant { name, fields },
                        span: start.merge(self.prev_span()),
                    });
                }
                Ok(Expr { id: NodeId::NONE,
                    kind: ExprKind::Path(Path {
                        segs: vec![name],
                        span: start.merge(self.prev_span()),
                    }),
                    span: start.merge(self.prev_span()),
                })
            }
            _ => {
                self.err(
                    "E0005",
                    format!("expected expression, got {:?}", self.cur().kind),
                );
                Err(())
            }
        }
    }

    fn parse_block_or_record(&mut self) -> Result<Expr, ()> {
        // Ambiguity: `{ x: 1 }` is a record; `{ x; }` / `{ x }` is a block.
        // Lookahead: after `{`, if we see `ident :` it's a record (unless it's
        // a typed let, but `let` starts with the keyword).
        if self.looks_like_record_lit() {
            let start = self.cur().span;
            let fields = self.parse_record_lit_fields()?;
            return Ok(Expr { id: NodeId::NONE,
                kind: ExprKind::Record(fields),
                span: start.merge(self.prev_span()),
            });
        }
        self.parse_block_expr()
    }

    fn looks_like_record_lit(&self) -> bool {
        if !self.at(TokenKind::LBrace) {
            return false;
        }
        // `{ }` empty: prefer block. `{ ident :` or `{ "str" :` is a record.
        let k1 = self.peek_kind(1);
        let k2 = self.peek_kind(2);
        (matches!(k1, TokenKind::Ident) || matches!(k1, TokenKind::String))
            && matches!(k2, TokenKind::Colon)
    }

    fn parse_block_expr(&mut self) -> Result<Expr, ()> {
        let start = self.cur().span;
        self.expect(TokenKind::LBrace)?;
        let mut stmts = Vec::new();
        let mut tail = None;
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.at(TokenKind::Let) {
                let l = self.parse_let_stmt()?;
                self.expect(TokenKind::Semi)?;
                stmts.push(Stmt {
                    span: l.span,
                    kind: StmtKind::Let(l),
                });
                continue;
            }
            let e = self.parse_expr()?;
            if self.at(TokenKind::Semi) {
                self.bump();
                stmts.push(Stmt {
                    span: e.span,
                    kind: StmtKind::Expr(e),
                });
            } else if self.at(TokenKind::RBrace) {
                tail = Some(Box::new(e));
                break;
            } else if is_blockish(&e) && self.can_start_expr() {
                // Spec examples omit `;` after for/if/match/region blocks.
                stmts.push(Stmt {
                    span: e.span,
                    kind: StmtKind::Expr(e),
                });
            } else {
                tail = Some(Box::new(e));
                break;
            }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(Expr { id: NodeId::NONE,
            kind: ExprKind::Block { stmts, tail },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_let_stmt(&mut self) -> Result<LetStmt, ()> {
        let start = self.cur().span;
        self.expect(TokenKind::Let)?;
        let mutable = if self.at(TokenKind::Mut) {
            self.bump();
            true
        } else {
            false
        };
        let pat = self.parse_pattern()?;
        let ty = if self.at(TokenKind::Colon) {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::Eq)?;
        let init = self.parse_expr()?;
        Ok(LetStmt {
            mutable,
            pat,
            ty,
            init,
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_if(&mut self) -> Result<Expr, ()> {
        let start = self.cur().span;
        self.expect(TokenKind::If)?;
        let cond = self.parse_expr()?;
        let then_b = self.parse_block_expr()?;
        let else_b = if self.at(TokenKind::Else) {
            self.bump();
            if self.at(TokenKind::If) {
                Some(Box::new(self.parse_if()?))
            } else {
                Some(Box::new(self.parse_block_expr()?))
            }
        } else {
            None
        };
        Ok(Expr { id: NodeId::NONE,
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_b: Box::new(then_b),
                else_b,
            },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_match(&mut self) -> Result<Expr, ()> {
        let start = self.cur().span;
        self.expect(TokenKind::Match)?;
        let scrut = self.parse_expr()?;
        self.expect(TokenKind::LBrace)?;
        let mut arms = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let astart = self.cur().span;
            let pat = self.parse_pattern()?;
            self.expect(TokenKind::FatArrow)?;
            let body = self.parse_expr()?;
            if self.at(TokenKind::Semi) {
                self.bump();
            } else if !self.at(TokenKind::RBrace) {
                self.expect(TokenKind::Semi)?;
            }
            arms.push(Arm {
                pat,
                body,
                span: astart.merge(self.prev_span()),
            });
        }
        self.expect(TokenKind::RBrace)?;
        Ok(Expr { id: NodeId::NONE,
            kind: ExprKind::Match {
                scrut: Box::new(scrut),
                arms,
            },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_for(&mut self) -> Result<Expr, ()> {
        let start = self.cur().span;
        self.expect(TokenKind::For)?;
        let pat = self.parse_pattern()?;
        self.expect(TokenKind::In)?;
        let iter = self.parse_expr()?;
        let body = self.parse_block_expr()?;
        Ok(Expr { id: NodeId::NONE,
            kind: ExprKind::For {
                pat,
                iter: Box::new(iter),
                body: Box::new(body),
            },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_catch(&mut self) -> Result<Expr, ()> {
        let start = self.cur().span;
        self.expect(TokenKind::Catch)?;
        let expr = self.parse_expr()?;
        self.expect(TokenKind::LBrace)?;
        let mut arms = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let astart = self.cur().span;
            let pat = self.parse_pattern()?;
            self.expect(TokenKind::FatArrow)?;
            let body = self.parse_expr()?;
            if self.at(TokenKind::Semi) {
                self.bump();
            } else if !self.at(TokenKind::RBrace) {
                self.expect(TokenKind::Semi)?;
            }
            arms.push(Arm {
                pat,
                body,
                span: astart.merge(self.prev_span()),
            });
        }
        self.expect(TokenKind::RBrace)?;
        Ok(Expr { id: NodeId::NONE,
            kind: ExprKind::Catch {
                expr: Box::new(expr),
                arms,
            },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_par(&mut self) -> Result<Expr, ()> {
        let start = self.cur().span;
        self.expect(TokenKind::Par)?;
        self.expect(TokenKind::LBrace)?;
        let mut bindings = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let l = self.parse_let_stmt()?;
            self.expect(TokenKind::Semi)?;
            bindings.push(l);
        }
        self.expect(TokenKind::RBrace)?;
        Ok(Expr { id: NodeId::NONE,
            kind: ExprKind::Par { bindings },
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_record_lit_fields(&mut self) -> Result<Vec<(Ident, Expr)>, ()> {
        self.expect(TokenKind::LBrace)?;
        let mut fields = Vec::new();
        if !self.at(TokenKind::RBrace) {
            loop {
                let name = if self.at(TokenKind::String) {
                    let t = self.bump();
                    Ident {
                        name: t.symbol,
                        span: t.span,
                    }
                } else {
                    self.parse_ident()?
                };
                self.expect(TokenKind::Colon)?;
                let e = self.parse_expr()?;
                fields.push((name, e));
                if self.at(TokenKind::Comma) {
                    self.bump();
                    if self.at(TokenKind::RBrace) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(fields)
    }

    // ---------- patterns ----------

    fn parse_pattern(&mut self) -> Result<Pattern, ()> {
        let start = self.cur().span;
        if self.at(TokenKind::Ident) && self.intern.get(self.cur().symbol) == "_" {
            let t = self.bump();
            return Ok(Pattern { id: NodeId::NONE,
                kind: PatKind::Wild,
                span: t.span,
            });
        }
        match self.cur().kind {
            TokenKind::Integer => {
                let t = self.bump();
                let text = self.intern.get(t.symbol).to_string();
                let (value, suffix) = parse_int(&text);
                Ok(Pattern { id: NodeId::NONE,
                    kind: PatKind::Lit(Lit::Int { value, suffix }),
                    span: t.span,
                })
            }
            TokenKind::True | TokenKind::False => {
                let t = self.bump();
                Ok(Pattern { id: NodeId::NONE,
                    kind: PatKind::Lit(Lit::Bool(t.kind == TokenKind::True)),
                    span: t.span,
                })
            }
            TokenKind::String => {
                let t = self.bump();
                Ok(Pattern { id: NodeId::NONE,
                    kind: PatKind::Lit(Lit::Str(self.intern.get(t.symbol).to_string())),
                    span: t.span,
                })
            }
            TokenKind::LBrace => {
                let fields = self.parse_record_pat_fields()?;
                Ok(Pattern { id: NodeId::NONE,
                    kind: PatKind::Record(fields),
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::LParen => {
                self.bump();
                let mut ps = Vec::new();
                loop {
                    ps.push(self.parse_pattern()?);
                    if self.at(TokenKind::Comma) {
                        self.bump();
                        if self.at(TokenKind::RParen) {
                            break;
                        }
                        continue;
                    }
                    break;
                }
                self.expect(TokenKind::RParen)?;
                Ok(Pattern { id: NodeId::NONE,
                    kind: PatKind::Tuple(ps),
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::Ident => {
                let name = self.parse_ident()?;
                if self.at(TokenKind::LBrace) {
                    let fields = self.parse_record_pat_fields()?;
                    Ok(Pattern { id: NodeId::NONE,
                        kind: PatKind::Variant { name, fields },
                        span: start.merge(self.prev_span()),
                    })
                } else if self.at(TokenKind::LParen) {
                    // Tuple-style variant: Err(NegativeScore { index })
                    self.bump();
                    let mut fields = Vec::new();
                    if !self.at(TokenKind::RParen) {
                        let mut idx = 0u32;
                        loop {
                            let p = self.parse_pattern()?;
                            let fname = self.intern.intern(&format!("_{idx}"));
                            fields.push((
                                Ident {
                                    name: fname,
                                    span: p.span,
                                },
                                p,
                            ));
                            idx += 1;
                            if self.at(TokenKind::Comma) {
                                self.bump();
                                if self.at(TokenKind::RParen) {
                                    break;
                                }
                                continue;
                            }
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen)?;
                    Ok(Pattern { id: NodeId::NONE,
                        kind: PatKind::Variant { name, fields },
                        span: start.merge(self.prev_span()),
                    })
                } else {
                    Ok(Pattern { id: NodeId::NONE,
                        kind: PatKind::Bind(name),
                        span: start.merge(self.prev_span()),
                    })
                }
            }
            _ => {
                self.err("E0006", "expected pattern");
                Err(())
            }
        }
    }

    fn parse_record_pat_fields(&mut self) -> Result<Vec<(Ident, Pattern)>, ()> {
        self.expect(TokenKind::LBrace)?;
        let mut fields = Vec::new();
        if !self.at(TokenKind::RBrace) {
            loop {
                let name = self.parse_ident()?;
                let pat = if self.at(TokenKind::Colon) {
                    self.bump();
                    self.parse_pattern()?
                } else {
                    Pattern { id: NodeId::NONE,
                        kind: PatKind::Bind(name.clone()),
                        span: name.span,
                    }
                };
                fields.push((name, pat));
                if self.at(TokenKind::Comma) {
                    self.bump();
                    if self.at(TokenKind::RBrace) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(fields)
    }

    fn parse_path(&mut self) -> Result<Path, ()> {
        let start = self.cur().span;
        let mut segs = vec![self.parse_path_seg()?];
        while self.at(TokenKind::Dot) {
            self.bump();
            segs.push(self.parse_path_seg()?);
        }
        Ok(Path {
            segs,
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_path_seg(&mut self) -> Result<Ident, ()> {
        // Keywords that appear as prelude module names (`test.read_cap`,
        // `int.div`, `json.decode`) must be legal path segments.
        if self.at(TokenKind::Ident)
            || matches!(
                self.cur().kind,
                TokenKind::Default
                    | TokenKind::In
                    | TokenKind::As
                    | TokenKind::From
                    | TokenKind::Test
                    | TokenKind::Type
            )
        {
            let t = self.bump();
            return Ok(Ident {
                name: t.symbol,
                span: t.span,
            });
        }
        self.parse_ident()
    }

    fn parse_ident(&mut self) -> Result<Ident, ()> {
        // Keywords that can be used as identifiers in limited positions
        // (effect names are idents; we already parse them as idents).
        if self.at(TokenKind::Ident)
            || matches!(
                self.cur().kind,
                TokenKind::Default | TokenKind::In | TokenKind::As | TokenKind::From
            )
        {
            let t = self.bump();
            return Ok(Ident {
                name: t.symbol,
                span: t.span,
            });
        }
        self.err(
            "E0007",
            format!("expected identifier, got {:?}", self.cur().kind),
        );
        Err(())
    }

    fn can_start_expr(&self) -> bool {
        matches!(
            self.cur().kind,
            TokenKind::Integer
                | TokenKind::Float
                | TokenKind::String
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Ident
                | TokenKind::LBrace
                | TokenKind::LParen
                | TokenKind::If
                | TokenKind::Match
                | TokenKind::For
                | TokenKind::Loop
                | TokenKind::Let
                | TokenKind::Fn
                | TokenKind::Return
                | TokenKind::Raise
                | TokenKind::Catch
                | TokenKind::Attempt
                | TokenKind::Region
                | TokenKind::Par
                | TokenKind::Question
                | TokenKind::Bang
                | TokenKind::Minus
                | TokenKind::Amp
                | TokenKind::Star
        )
    }

    // ---------- token helpers ----------

    fn cur(&self) -> Token {
        self.tokens.get(self.pos).copied().unwrap_or(Token {
            kind: TokenKind::Eof,
            span: Span::DUMMY,
            symbol: Symbol(0),
        })
    }

    fn peek_kind(&self, n: usize) -> TokenKind {
        self.tokens
            .get(self.pos + n)
            .map(|t| t.kind)
            .unwrap_or(TokenKind::Eof)
    }

    fn at(&self, k: TokenKind) -> bool {
        self.cur().kind == k
    }

    fn bump(&mut self) -> Token {
        let t = self.cur();
        if t.kind != TokenKind::Eof {
            self.pos += 1;
        }
        t
    }

    fn prev_span(&self) -> Span {
        if self.pos == 0 {
            Span::DUMMY
        } else {
            self.tokens[self.pos - 1].span
        }
    }

    fn expect(&mut self, k: TokenKind) -> Result<Token, ()> {
        if self.at(k) {
            Ok(self.bump())
        } else {
            self.err(
                "E0008",
                format!("expected {:?}, got {:?}", k, self.cur().kind),
            );
            Err(())
        }
    }

    fn err(&mut self, code: &str, msg: impl Into<String>) {
        self.diags
            .push(Diagnostic::error(code, self.cur().span, msg.into()));
    }

    fn err_span(&mut self, span: Span, code: &str, msg: impl Into<String>) {
        self.diags.push(Diagnostic::error(code, span, msg.into()));
    }
}

fn is_blockish(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::If { .. }
            | ExprKind::For { .. }
            | ExprKind::Loop { .. }
            | ExprKind::Match { .. }
            | ExprKind::Catch { .. }
            | ExprKind::Region { .. }
            | ExprKind::Par { .. }
            | ExprKind::Block { .. }
    )
}

fn parse_int(text: &str) -> (i128, Option<Prim>) {
    let (num, suf) = split_suffix(text);
    let value = parse_int_text(num).unwrap_or(0);
    (value, suf)
}

/// Parse an integer literal: optional radix prefix, `_` separators.
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
    // An unsuffixed literal that does not fit i128 is clamped rather than
    // silently wrapping; the checker reports the width mismatch.
    clean.parse().ok().or_else(|| clean.parse::<u128>().ok().map(|v| v as i128))
}

fn parse_float(text: &str) -> (f64, Option<Prim>) {
    let (num, suf) = split_suffix(text);
    let clean: String = num.chars().filter(|c| *c != '_').collect();
    let value = clean.parse().unwrap_or(0.0);
    (value, suf)
}

fn split_suffix(text: &str) -> (&str, Option<Prim>) {
    for suf in [
        "i8", "i16", "i32", "i64", "isz", "u8", "u16", "u32", "u64", "usz", "f32", "f64",
    ] {
        if let Some(rest) = text.strip_suffix(suf) {
            // A suffix only counts when a digit (or separator) precedes it, so
            // `0x1f` keeps its `f` and `1_000u32` still splits.
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

#[allow(dead_code)]
fn _severity_unused() -> Severity {
    Severity::Error
}
