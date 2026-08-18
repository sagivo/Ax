//! Three surface frontends lower to one AST (§2.3).
//!
//! - `S-conventional` — Rust/Go priors (default, already the main parser)
//! - `S-terse` — v0.1: no colons, no arrows, `!eff+eff` rows
//! - `S-verbose` — explicit dictionaries, no local inference (enforced in check)

use crate::ast::File;
use crate::diag::Diagnostic;
use crate::intern::Interner;
use crate::parser::Parser;
use crate::span::FileId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Conventional,
    Terse,
    Verbose,
}

impl Surface {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "conventional" | "s-conventional" | "conv" => Some(Surface::Conventional),
            "terse" | "s-terse" => Some(Surface::Terse),
            "verbose" | "s-verbose" => Some(Surface::Verbose),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Conventional => "conventional",
            Surface::Terse => "terse",
            Surface::Verbose => "verbose",
        }
    }
}

/// Rewrite S-terse source into S-conventional, then parse with the one parser.
///
/// Terse rules (v0.1):
///   `fn name(a T, b U) R !err[E]+io = body`
/// becomes
///   `fn name(a: T, b: U) -> R !{err[E], io} = body`
pub fn rewrite_terse(src: &str) -> String {
    rewrite_terse_named(src, "m")
}

/// As [`rewrite_terse`], supplying the module name to use when the source omits
/// the `module` declaration.
///
/// The terse surface exists for a program that is generating and re-reading code,
/// and a header it can always reconstruct is pure cost: `module x; export { .. };`
/// is about twelve tokens per file that say nothing the toolchain does not
/// already know. Omitting them is allowed here and only here — the conventional
/// surface still requires the declaration, because a human reader benefits from
/// it.
pub fn rewrite_terse_named(src: &str, module: &str) -> String {
    let body = rewrite_terse_inner(src);
    // Reinstate the header if the source left it out.
    let has_module = body
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("//"))
        .map(|l| l.trim_start().starts_with("module"))
        .unwrap_or(false);
    if has_module {
        body
    } else {
        format!("module {module};\n{body}")
    }
}

fn rewrite_terse_inner(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + src.len() / 8);
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // comments / strings pass through
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                out.push(b[i] as char);
                i += 1;
            }
            continue;
        }
        if b[i] == b'"' || b[i] == b'`' {
            let q = b[i];
            out.push(q as char);
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == q {
                    i += 1;
                    break;
                }
                if b[i] == b'\\' && i + 1 < b.len() {
                    i += 1;
                    out.push(b[i] as char);
                }
                i += 1;
            }
            continue;
        }
        // `fn name(...) TYPE`  or  `fn name(...) !effs TYPE` — insert colons in params
        if is_ident_at(b, i, "fn") {
            // copy `fn`
            out.push_str("fn");
            i += 2;
            skip_ws_copy(b, &mut i, &mut out);
            // name
            copy_ident(b, &mut i, &mut out);
            skip_ws_copy(b, &mut i, &mut out);
            // optional generics `[...]`
            if i < b.len() && b[i] == b'[' {
                copy_balanced(b, &mut i, &mut out, b'[', b']');
                skip_ws_copy(b, &mut i, &mut out);
            }
            if i < b.len() && b[i] == b'(' {
                out.push('(');
                i += 1;
                rewrite_terse_params(b, &mut i, &mut out);
                skip_ws_copy(b, &mut i, &mut out);
                // effects `!err[E]+io` or `!{...}` already conventional
                if i < b.len() && b[i] == b'!' && i + 1 < b.len() && b[i + 1] != b'{' {
                    out.push(' ');
                    rewrite_terse_effects(b, &mut i, &mut out);
                    skip_ws_copy(b, &mut i, &mut out);
                }
                // return type (if not `=` and not already `->`)
                if i < b.len() && b[i] != b'=' && !starts_with(b, i, "->") {
                    if !starts_with(b, i, "pre")
                        && !starts_with(b, i, "post")
                        && !starts_with(b, i, "inv")
                    {
                        out.push_str(" -> ");
                        // Copy the result type, then accept a terse effect row
                        // after it. The card documents this order
                        // (`fn f(a T) R !err[E]+io`), and only accepting rows
                        // *before* the type meant the documented form did not
                        // parse at all.
                        copy_type(b, &mut i, &mut out);
                        skip_ws_only(b, &mut i);
                        if i < b.len()
                            && b[i] == b'!'
                            && i + 1 < b.len()
                            && b[i + 1].is_ascii_alphabetic()
                        {
                            out.push(' ');
                            rewrite_terse_effects(b, &mut i, &mut out);
                        }
                    }
                }
            }
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn rewrite_terse_params(b: &[u8], i: &mut usize, out: &mut String) {
    let mut first = true;
    while *i < b.len() && b[*i] != b')' {
        if b[*i] == b',' {
            out.push(',');
            *i += 1;
            first = false;
            continue;
        }
        if b[*i].is_ascii_whitespace() {
            out.push(b[*i] as char);
            *i += 1;
            continue;
        }
        // name
        if !first && out.ends_with(',') {
            out.push(' ');
        }
        copy_ident(b, i, out);
        skip_ws_only(b, i);
        // if next is a type ident / `&` / `[` insert colon
        if *i < b.len() && b[*i] != b',' && b[*i] != b')' && b[*i] != b'=' {
            out.push_str(": ");
            // copy type until `,` or `)`
            while *i < b.len() && b[*i] != b',' && b[*i] != b')' {
                if b[*i].is_ascii_whitespace() {
                    // collapse trailing
                    *i += 1;
                    continue;
                }
                out.push(b[*i] as char);
                *i += 1;
            }
        }
        first = false;
    }
    if *i < b.len() && b[*i] == b')' {
        out.push(')');
        *i += 1;
    }
}

fn rewrite_terse_effects(b: &[u8], i: &mut usize, out: &mut String) {
    // `!err[E]+io+alloc[a]` → `!{err[E], io, alloc[a]}`
    debug_assert_eq!(b[*i], b'!');
    *i += 1;
    out.push_str("!{");
    let mut first = true;
    while *i < b.len() {
        if b[*i] == b'+' {
            *i += 1;
            first = false;
            out.push_str(", ");
            continue;
        }
        if b[*i].is_ascii_whitespace() || b[*i] == b'=' {
            break;
        }
        if !first && !out.ends_with("{") && !out.ends_with(", ") && b[*i].is_ascii_alphabetic() {
            // next effect
        }
        out.push(b[*i] as char);
        *i += 1;
        first = false;
        // stop at whitespace / `=` / start of type-looking leftover handled by caller
        if *i < b.len() && (b[*i].is_ascii_whitespace() || b[*i] == b'=') {
            break;
        }
    }
    out.push('}');
}

fn is_ident_at(b: &[u8], i: usize, kw: &str) -> bool {
    let k = kw.as_bytes();
    if i + k.len() > b.len() {
        return false;
    }
    if &b[i..i + k.len()] != k {
        return false;
    }
    let after = i + k.len();
    let before_ok = i == 0 || !is_ident_char(b[i - 1]);
    let after_ok = after == b.len() || !is_ident_char(b[after]);
    before_ok && after_ok
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn starts_with(b: &[u8], i: usize, s: &str) -> bool {
    let k = s.as_bytes();
    i + k.len() <= b.len() && &b[i..i + k.len()] == k
}

fn copy_ident(b: &[u8], i: &mut usize, out: &mut String) {
    while *i < b.len() && is_ident_char(b[*i]) {
        out.push(b[*i] as char);
        *i += 1;
    }
}

fn skip_ws_copy(b: &[u8], i: &mut usize, out: &mut String) {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        out.push(b[*i] as char);
        *i += 1;
    }
}

/// Copy one type expression: identifier characters plus the punctuation types
/// are built from. Stops at `!` (an effect row), `=`, or a contract keyword.
fn copy_type(b: &[u8], i: &mut usize, out: &mut String) {
    let mut depth = 0i32;
    while *i < b.len() {
        let c = b[*i];
        match c {
            b'[' | b'(' => depth += 1,
            b']' | b')' => depth -= 1,
            b'!' | b'=' if depth == 0 => break,
            b' ' if depth == 0 => {
                // A space may separate `&r mut T`; keep going while the next
                // token still looks like part of a type.
                let mut j = *i;
                while j < b.len() && b[j] == b' ' {
                    j += 1;
                }
                if j < b.len()
                    && (is_ident_char(b[j]) || b[j] == b'&' || b[j] == b'[')
                    && !starts_with(b, j, "pre")
                    && !starts_with(b, j, "post")
                    && !starts_with(b, j, "inv")
                {
                    out.push(' ');
                    *i = j;
                    continue;
                }
                break;
            }
            _ => {}
        }
        if depth < 0 {
            break;
        }
        out.push(c as char);
        *i += 1;
    }
}

fn skip_ws_only(b: &[u8], i: &mut usize) {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
}

fn copy_balanced(b: &[u8], i: &mut usize, out: &mut String, open: u8, close: u8) {
    let mut depth = 0;
    while *i < b.len() {
        let c = b[*i];
        out.push(c as char);
        *i += 1;
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
    }
}

pub fn parse_surface(
    src: &str,
    file: FileId,
    intern: &mut Interner,
    surface: Surface,
) -> Result<File, Vec<Diagnostic>> {
    parse_surface_named(src, file, intern, surface, "m")
}

/// As [`parse_surface`], with the name to use for an omitted terse `module`
/// declaration (normally the file stem).
pub fn parse_surface_named(
    src: &str,
    file: FileId,
    intern: &mut Interner,
    surface: Surface,
    module: &str,
) -> Result<File, Vec<Diagnostic>> {
    // `Parser::parse_file` assigns node ids, so every surface shares one dense
    // id space regardless of which entry point produced the AST.
    match surface {
        Surface::Conventional | Surface::Verbose => Parser::parse_file(src, file, intern),
        Surface::Terse => {
            let rewritten = rewrite_terse_named(src, module);
            Parser::parse_file(&rewritten, file, intern)
        }
    }
}

/// Rewrite S-conventional source into S-terse: the inverse of [`rewrite_terse`].
///
/// Mechanical and syntax-only — it removes the punctuation the terse surface
/// makes optional (`:` in parameter lists, `->` before a result type) and
/// contracts effect rows from `!{a, b}` to `!a+b`. Nothing about the program's
/// meaning changes, which is the point: the two surfaces are the same AST, so a
/// token count can be compared without comparing programs.
pub fn to_terse(src: &str) -> String {
    // Drop the header: the terse surface reconstructs it.
    let stripped: String = src
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("module ") || t.starts_with("export "))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let src = stripped.as_str();
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        // Comments and strings pass through untouched.
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                out.push(b[i] as char);
                i += 1;
            }
            continue;
        }
        if b[i] == b'"' || b[i] == b'`' {
            let q = b[i];
            out.push(q as char);
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == q {
                    i += 1;
                    break;
                }
                if b[i] == b'\\' && i + 1 < b.len() {
                    i += 1;
                    out.push(b[i] as char);
                }
                i += 1;
            }
            continue;
        }
        if is_ident_at(b, i, "fn") {
            out.push_str("fn");
            i += 2;
            copy_ws(b, &mut i, &mut out);
            copy_ident(b, &mut i, &mut out);
            copy_ws(b, &mut i, &mut out);
            if i < b.len() && b[i] == b'[' {
                copy_balanced(b, &mut i, &mut out, b'[', b']');
                copy_ws(b, &mut i, &mut out);
            }
            if i < b.len() && b[i] == b'(' {
                out.push('(');
                i += 1;
                // Drop `: ` between a parameter name and its type.
                let mut depth = 1;
                while i < b.len() && depth > 0 {
                    match b[i] {
                        b'(' | b'[' => {
                            depth += 1;
                            out.push(b[i] as char);
                            i += 1;
                        }
                        b')' | b']' => {
                            depth -= 1;
                            if depth > 0 {
                                out.push(b[i] as char);
                            }
                            i += 1;
                        }
                        b':' if depth == 1 => {
                            i += 1;
                            while i < b.len() && b[i] == b' ' {
                                i += 1;
                            }
                            out.push(' ');
                        }
                        _ => {
                            out.push(b[i] as char);
                            i += 1;
                        }
                    }
                }
                out.push(')');
                // Drop the arrow before the result type. The effect row keeps
                // its position after the type, which is the order the card
                // documents.
                let save = i;
                let mut j = i;
                while j < b.len() && (b[j] == b' ' || b[j] == b'\n' || b[j] == b'\t') {
                    j += 1;
                }
                if starts_with(b, j, "->") {
                    out.push(' ');
                    i = j + 2;
                    while i < b.len() && b[i] == b' ' {
                        i += 1;
                    }
                } else {
                    i = save;
                }
            }
            continue;
        }
        // Effect rows: `!{err[E], io[c]}` becomes `!err[E]+io[c]`.
        if b[i] == b'!' && i + 1 < b.len() && b[i + 1] == b'{' {
            out.push('!');
            i += 2;
            let mut depth = 1;
            while i < b.len() && depth > 0 {
                match b[i] {
                    b'{' => {
                        depth += 1;
                        out.push('{');
                        i += 1;
                    }
                    b'}' => {
                        depth -= 1;
                        if depth > 0 {
                            out.push('}');
                        }
                        i += 1;
                    }
                    b',' if depth == 1 => {
                        out.push('+');
                        i += 1;
                        while i < b.len() && b[i] == b' ' {
                            i += 1;
                        }
                    }
                    _ => {
                        out.push(b[i] as char);
                        i += 1;
                    }
                }
            }
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn copy_ws(b: &[u8], i: &mut usize, out: &mut String) {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        out.push(b[*i] as char);
        *i += 1;
    }
}
