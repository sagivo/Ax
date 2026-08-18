//! Canonical formatter. One legal layout, no options, idempotent.

use crate::ast::*;
use crate::intern::Interner;

pub fn format_file(file: &File, intern: &Interner) -> String {
    let mut o = String::new();
    o.push_str("module ");
    o.push_str(&path_s(&file.module, intern));
    o.push_str(";\n");
    if !file.exports.is_empty() {
        o.push_str("export { ");
        for (i, e) in file.exports.iter().enumerate() {
            if i > 0 {
                o.push_str(", ");
            }
            o.push_str(intern.get(e.name));
        }
        o.push_str(" };\n");
    }
    for u in &file.uses {
        o.push_str("use ");
        o.push_str(&path_s(&u.path, intern));
        if let Some(a) = &u.alias {
            o.push_str(" as ");
            o.push_str(intern.get(a.name));
        }
        o.push_str(";\n");
    }
    if !file.uses.is_empty() {
        o.push('\n');
    }
    for d in &file.decls {
        fmt_decl(&mut o, d, intern, 0);
        o.push('\n');
    }
    o
}

fn fmt_decl(o: &mut String, d: &Decl, intern: &Interner, indent: usize) {
    for m in &d.meta {
        o.push('@');
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
        o.push('\n');
    }
    match &d.kind {
        DeclKind::Fn(f) => fmt_fn(o, f, intern, false),
        DeclKind::ContractFn(f) => fmt_fn(o, f, intern, true),
        DeclKind::Type(t) => fmt_type_decl(o, t, intern),
        DeclKind::Dict(dd) => {
            o.push_str("dict ");
            o.push_str(intern.get(dd.name.name));
            o.push('[');
            fmt_ty(o, &dd.for_ty, intern);
            o.push_str("] = { ");
            for (i, (n, e)) in dd.fields.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                o.push_str(intern.get(n.name));
                o.push_str(": ");
                fmt_expr(o, e, intern, indent);
            }
            o.push_str(" };\n");
        }
        DeclKind::Test(t) => {
            o.push_str("test \"");
            o.push_str(&escape(&t.name));
            o.push_str("\" = ");
            fmt_expr(o, &t.body, intern, indent);
            o.push_str(";\n");
        }
    }
}

fn fmt_fn(o: &mut String, f: &FnDecl, intern: &Interner, contract: bool) {
    if contract {
        o.push_str("contract ");
    }
    o.push_str("fn ");
    o.push_str(intern.get(f.name.name));
    if !f.generics.is_empty() {
        o.push('[');
        for (i, g) in f.generics.iter().enumerate() {
            if i > 0 {
                o.push_str(", ");
            }
            o.push_str(intern.get(g.name.name));
            if let Some(b) = &g.bound {
                o.push_str(": ");
                fmt_ty(o, b, intern);
            }
        }
        o.push(']');
    }
    o.push('(');
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            o.push_str(", ");
        }
        o.push_str(intern.get(p.name.name));
        o.push_str(": ");
        fmt_ty(o, &p.ty, intern);
        if p.default_dict {
            o.push_str(" = default");
        }
    }
    o.push_str(") -> ");
    fmt_ty(o, &f.ret, intern);
    if !f.effects.items.is_empty() {
        o.push(' ');
        fmt_effects(o, &f.effects, intern);
    }
    for c in &f.contracts {
        o.push('\n');
        o.push_str("    ");
        o.push_str(match c.kind {
            ContractKind::Pre => "pre ",
            ContractKind::Post => "post ",
            ContractKind::Inv => "inv ",
        });
        fmt_expr(o, &c.expr, intern, 1);
    }
    o.push_str("\n= ");
    fmt_expr(o, &f.body, intern, 0);
    o.push_str(";\n");
}

fn fmt_type_decl(o: &mut String, t: &TypeDecl, intern: &Interner) {
    o.push_str("type ");
    o.push_str(intern.get(t.name.name));
    if !t.generics.is_empty() {
        o.push('[');
        for (i, g) in t.generics.iter().enumerate() {
            if i > 0 {
                o.push_str(", ");
            }
            o.push_str(intern.get(g.name.name));
        }
        o.push(']');
    }
    o.push_str(" = ");
    match &t.body {
        TypeBody::Alias(ty) => fmt_ty(o, ty, intern),
        TypeBody::Record(fs) => fmt_fields(o, fs, intern),
        TypeBody::Variants(vs) => {
            for v in vs {
                o.push_str("| ");
                o.push_str(intern.get(v.name.name));
                if !v.fields.is_empty() {
                    o.push(' ');
                    fmt_fields(o, &v.fields, intern);
                }
                o.push('\n');
            }
        }
    }
    if !t.injections.is_empty() {
        o.push_str("with\n");
        for inj in &t.injections {
            o.push_str("    from ");
            fmt_ty(o, &inj.from, intern);
            o.push_str(" => ");
            o.push_str(intern.get(inj.into_variant.name));
            o.push_str(";\n");
        }
    }
    o.push_str(";\n");
}

fn fmt_fields(o: &mut String, fs: &[Field], intern: &Interner) {
    o.push('{');
    for (i, f) in fs.iter().enumerate() {
        if i > 0 {
            o.push_str(", ");
        } else {
            o.push(' ');
        }
        o.push_str(intern.get(f.name.name));
        o.push_str(": ");
        fmt_ty(o, &f.ty, intern);
    }
    if !fs.is_empty() {
        o.push(' ');
    }
    o.push('}');
}

fn fmt_effects(o: &mut String, e: &EffectRow, intern: &Interner) {
    o.push_str("!{");
    for (i, it) in e.items.iter().enumerate() {
        if i > 0 {
            o.push_str(", ");
        }
        match &it.kind {
            EffectKind::Err(t) => {
                o.push_str("err[");
                fmt_ty(o, t, intern);
                o.push(']');
            }
            EffectKind::Io(id) => {
                o.push_str("io[");
                o.push_str(intern.get(id.name));
                o.push(']');
            }
            EffectKind::Alloc(id) => {
                o.push_str("alloc[");
                o.push_str(intern.get(id.name));
                o.push(']');
            }
            EffectKind::Susp => o.push_str("susp"),
            EffectKind::Diverge => o.push_str("diverge"),
            EffectKind::Race => o.push_str("race"),
            EffectKind::Nondet => o.push_str("nondet"),
            EffectKind::Abort => o.push_str("abort"),
        }
    }
    o.push('}');
}

fn fmt_ty(o: &mut String, t: &TypeExpr, intern: &Interner) {
    match &t.kind {
        TypeExprKind::Prim(p) => o.push_str(p.as_str()),
        TypeExprKind::Hole => o.push('?'),
        TypeExprKind::Own(inner) => {
            o.push_str("own ");
            fmt_ty(o, inner, intern);
        }
        TypeExprKind::Untrusted(inner) => {
            o.push_str("Untrusted[");
            fmt_ty(o, inner, intern);
            o.push(']');
        }
        TypeExprKind::Secret(inner) => {
            o.push_str("Secret[");
            fmt_ty(o, inner, intern);
            o.push(']');
        }
        TypeExprKind::Ref {
            region,
            mutable,
            inner,
        } => {
            o.push('&');
            let rn = intern.get(region.name);
            if rn != "_" && !rn.is_empty() {
                o.push_str(rn);
                o.push(' ');
            }
            if *mutable {
                o.push_str("mut ");
            }
            fmt_ty(o, inner, intern);
        }
        TypeExprKind::Fn {
            params,
            ret,
            effects,
        } => {
            o.push_str("fn(");
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                fmt_ty(o, p, intern);
            }
            o.push_str(") -> ");
            fmt_ty(o, ret, intern);
            if !effects.items.is_empty() {
                o.push(' ');
                fmt_effects(o, effects, intern);
            }
        }
        TypeExprKind::Tuple(ts) => {
            o.push('(');
            for (i, t) in ts.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                fmt_ty(o, t, intern);
            }
            o.push(')');
        }
        TypeExprKind::Named { path, args } => {
            o.push_str(&path_s(path, intern));
            if !args.is_empty() {
                o.push('[');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        o.push_str(", ");
                    }
                    fmt_ty(o, a, intern);
                }
                o.push(']');
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
            fmt_expr(o, callee, intern, indent);
            o.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                fmt_expr(o, a, intern, indent);
            }
            o.push(')');
        }
        ExprKind::Field { base, field } => {
            fmt_expr(o, base, intern, indent);
            o.push('.');
            o.push_str(intern.get(field.name));
        }
        ExprKind::Index { base, index } => {
            fmt_expr(o, base, intern, indent);
            o.push('[');
            fmt_expr(o, index, intern, indent);
            o.push(']');
        }
        ExprKind::Unary { op, expr } => {
            o.push_str(match op {
                UnOp::Not => "!",
            UnOp::BitNot => "~",
                UnOp::Neg => "-",
                UnOp::Ref => "&",
                UnOp::RefMut => "&mut ",
                UnOp::Deref => "*",
            });
            fmt_expr(o, expr, intern, indent);
        }
        ExprKind::Binary { op, lhs, rhs } => {
            fmt_expr(o, lhs, intern, indent);
            o.push(' ');
            o.push_str(op.as_str());
            o.push(' ');
            fmt_expr(o, rhs, intern, indent);
        }
        ExprKind::Block { stmts, tail } => {
            o.push('{');
            if stmts.is_empty() && tail.is_none() {
                o.push('}');
                return;
            }
            o.push('\n');
            for s in stmts {
                pad(o, indent + 1);
                match &s.kind {
                    StmtKind::Let(l) => {
                        fmt_let(o, l, intern, indent + 1);
                        o.push(';');
                    }
                    StmtKind::Expr(x) => {
                        fmt_expr(o, x, intern, indent + 1);
                        o.push(';');
                    }
                }
                o.push('\n');
            }
            if let Some(t) = tail {
                pad(o, indent + 1);
                fmt_expr(o, t, intern, indent + 1);
                o.push('\n');
            }
            pad(o, indent);
            o.push('}');
        }
        ExprKind::If {
            cond,
            then_b,
            else_b,
        } => {
            o.push_str("if ");
            fmt_expr(o, cond, intern, indent);
            o.push(' ');
            fmt_expr(o, then_b, intern, indent);
            if let Some(el) = else_b {
                o.push_str(" else ");
                fmt_expr(o, el, intern, indent);
            }
        }
        ExprKind::Match { scrut, arms } => {
            o.push_str("match ");
            fmt_expr(o, scrut, intern, indent);
            o.push_str(" {\n");
            for a in arms {
                pad(o, indent + 1);
                fmt_pat(o, &a.pat, intern);
                o.push_str(" => ");
                fmt_expr(o, &a.body, intern, indent + 1);
                o.push_str(";\n");
            }
            pad(o, indent);
            o.push('}');
        }
        ExprKind::For { pat, iter, body } => {
            o.push_str("for ");
            fmt_pat(o, pat, intern);
            o.push_str(" in ");
            fmt_expr(o, iter, intern, indent);
            o.push(' ');
            fmt_expr(o, body, intern, indent);
        }
        ExprKind::While { cond, body } => {
            o.push_str("while ");
            fmt_expr(o, cond, intern, indent);
            o.push(' ');
            fmt_expr(o, body, intern, indent);
        }
        ExprKind::Break => o.push_str("break"),
        ExprKind::Continue => o.push_str("continue"),
        ExprKind::Cast { expr: inner, ty } => {
            fmt_expr(o, inner, intern, indent);
            o.push_str(" as ");
            fmt_ty(o, ty, intern);
        }
        ExprKind::Loop { body } => {
            o.push_str("loop ");
            fmt_expr(o, body, intern, indent);
        }
        ExprKind::Let(l) => fmt_let(o, l, intern, indent),
        ExprKind::Lambda { params, ret, body } => {
            o.push_str("fn(");
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                o.push_str(intern.get(p.name.name));
                o.push_str(": ");
                fmt_ty(o, &p.ty, intern);
            }
            o.push(')');
            if let Some(r) = ret {
                o.push_str(" -> ");
                fmt_ty(o, r, intern);
            }
            o.push_str(" => ");
            fmt_expr(o, body, intern, indent);
        }
        ExprKind::Record(fs) => {
            o.push_str("{ ");
            for (i, (n, e)) in fs.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                o.push_str(intern.get(n.name));
                o.push_str(": ");
                fmt_expr(o, e, intern, indent);
            }
            o.push_str(" }");
        }
        ExprKind::Variant { name, fields } => {
            o.push_str(intern.get(name.name));
            if !fields.is_empty() {
                o.push_str(" { ");
                for (i, (n, e)) in fields.iter().enumerate() {
                    if i > 0 {
                        o.push_str(", ");
                    }
                    o.push_str(intern.get(n.name));
                    o.push_str(": ");
                    fmt_expr(o, e, intern, indent);
                }
                o.push_str(" }");
            }
        }
        ExprKind::Return(inner) => {
            o.push_str("return");
            if let Some(e) = inner {
                o.push(' ');
                fmt_expr(o, e, intern, indent);
            }
        }
        ExprKind::Raise(e) => {
            o.push_str("raise ");
            fmt_expr(o, e, intern, indent);
        }
        ExprKind::Catch { expr, arms } => {
            o.push_str("catch ");
            fmt_expr(o, expr, intern, indent);
            o.push_str(" {\n");
            for a in arms {
                pad(o, indent + 1);
                fmt_pat(o, &a.pat, intern);
                o.push_str(" => ");
                fmt_expr(o, &a.body, intern, indent + 1);
                o.push_str(";\n");
            }
            pad(o, indent);
            o.push('}');
        }
        ExprKind::Attempt(e) => {
            o.push_str("attempt ");
            fmt_expr(o, e, intern, indent);
        }
        ExprKind::Try(e) => {
            fmt_expr(o, e, intern, indent);
            o.push('?');
        }
        ExprKind::Interpolate { parts } => {
            o.push_str("f\"");
            for p in parts {
                match p {
                    InterpPart::Lit(s) => {
                        o.push_str(&s.replace('\\', "\\\\").replace('"', "\\\"").replace('{', "{{").replace('}', "}}"));
                    }
                    InterpPart::Expr(e) => {
                        o.push('{');
                        fmt_expr(o, e, intern, indent);
                        o.push('}');
                    }
                }
            }
            o.push('"');
        }
        ExprKind::Region { name, body } => {
            o.push_str("region ");
            o.push_str(intern.get(name.name));
            o.push(' ');
            fmt_expr(o, body, intern, indent);
        }
        ExprKind::Par { bindings } => {
            o.push_str("par {\n");
            for l in bindings {
                pad(o, indent + 1);
                fmt_let(o, l, intern, indent + 1);
                o.push_str(";\n");
            }
            pad(o, indent);
            o.push('}');
        }
        ExprKind::Assign { lhs, rhs } => {
            fmt_expr(o, lhs, intern, indent);
            o.push_str(" = ");
            fmt_expr(o, rhs, intern, indent);
        }
    }
}

fn fmt_let(o: &mut String, l: &LetStmt, intern: &Interner, indent: usize) {
    o.push_str("let ");
    if l.mutable {
        o.push_str("mut ");
    }
    fmt_pat(o, &l.pat, intern);
    if let Some(t) = &l.ty {
        o.push_str(": ");
        fmt_ty(o, t, intern);
    }
    o.push_str(" = ");
    fmt_expr(o, &l.init, intern, indent);
}

fn fmt_pat(o: &mut String, p: &Pattern, intern: &Interner) {
    match &p.kind {
        PatKind::Wild => o.push('_'),
        PatKind::Lit(l) => fmt_lit(o, l),
        PatKind::Bind(i) => o.push_str(intern.get(i.name)),
        PatKind::Variant { name, fields } => {
            o.push_str(intern.get(name.name));
            if !fields.is_empty() {
                let positional = fields
                    .iter()
                    .all(|(n, _)| intern.get(n.name).starts_with('_'));
                if positional {
                    o.push('(');
                    for (i, (_, p)) in fields.iter().enumerate() {
                        if i > 0 {
                            o.push_str(", ");
                        }
                        fmt_pat(o, p, intern);
                    }
                    o.push(')');
                } else {
                    o.push_str(" { ");
                    for (i, (n, p)) in fields.iter().enumerate() {
                        if i > 0 {
                            o.push_str(", ");
                        }
                        o.push_str(intern.get(n.name));
                        o.push_str(": ");
                        fmt_pat(o, p, intern);
                    }
                    o.push_str(" }");
                }
            }
        }
        PatKind::Record(fs) => {
            o.push_str("{ ");
            for (i, (n, p)) in fs.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                o.push_str(intern.get(n.name));
                o.push_str(": ");
                fmt_pat(o, p, intern);
            }
            o.push_str(" }");
        }
        PatKind::Tuple(ps) => {
            o.push('(');
            for (i, p) in ps.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
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

fn pad(o: &mut String, n: usize) {
    for _ in 0..n {
        o.push_str("    ");
    }
}
