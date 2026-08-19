//! Surfaces lower to one AST.
//!
//! Ax is `#name`, `:=`, `c??t:e`, `+/`, type glyphs. There is no opt-in
//! mode. A file that opens with `(` is the prefix tree.
//! Rust-shaped conventional / terse / verbose remain as a corpus dialect so
//! existing tests keep proving the IR; they rewrite into the same parser.

use crate::ast::File;
use crate::diag::Diagnostic;
use crate::intern::Interner;
use crate::parser::Parser;
use crate::span::FileId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Prefix tree. Detected automatically when a file starts with `(`.
    /// Also selected by `--surface tree`.
    Tree,
    Conventional,
    Terse,
    /// Token-minimal pack of terse. Same AST; fewer BPE pieces.
    Dense,
    Verbose,
}

impl Surface {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "tree" | "s-tree" | "canonical" => Some(Surface::Tree),
            "ax" | "dense" | "s-dense" | "mini" | "short" => Some(Surface::Dense),
            "conventional" | "s-conventional" | "conv" => Some(Surface::Conventional),
            "terse" | "s-terse" => Some(Surface::Terse),
            "verbose" | "s-verbose" => Some(Surface::Verbose),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Tree => "tree",
            Surface::Conventional => "conventional",
            Surface::Terse => "terse",
            Surface::Dense => "dense",
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
            // copy type until `,` or `)` at depth 0 (Map[K, V] has inner commas)
            let mut depth = 0i32;
            while *i < b.len() {
                let c = b[*i];
                match c {
                    b'[' | b'(' => depth += 1,
                    b']' | b')' => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                    }
                    b',' | b'=' if depth == 0 => break,
                    b' ' if depth == 0 => {
                        *i += 1;
                        continue;
                    }
                    _ => {}
                }
                out.push(c as char);
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
        Surface::Tree => crate::tree::parse_file(src, file, intern, module),
        Surface::Conventional | Surface::Verbose => Parser::parse_file(src, file, intern),
        Surface::Terse => {
            let rewritten = rewrite_terse_named(src, module);
            Parser::parse_file(&rewritten, file, intern)
        }
        Surface::Dense => {
            let terse = rewrite_dense_to_terse(src);
            let rewritten = rewrite_terse_named(&terse, module);
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

/// Dense type glyphs. Each is one BPE piece; the spelled forms they replace
/// (`i32`, `bool`, `String`) usually split (`i`+`32`, `String` → `String` or
/// `Str`+`ing`). Reserved as type atoms on this surface only.
const DENSE_TYPES: &[(&str, &str)] = &[
    ("I", "i32"),
    ("L", "i64"),
    ("Y", "isz"),
    ("U", "u32"),
    ("W", "u64"),
    ("Z", "usz"),
    ("B", "bool"),
    ("F", "f64"),
    ("f", "f32"),
    ("S", "String"),
    ("O", "Option"),
    ("R", "Result"),
    ("M", "Map"),
    ("V", "Vec"),
];

/// First significant token is `#`, or the file uses `:=` / `i~n` range sugar.
pub fn looks_like_dense(src: &str) -> bool {
    let t = src
        .lines()
        .map(str::trim_start)
        .find(|l| {
            !l.is_empty() && !l.starts_with("//") && !l.starts_with("#[") && !l.starts_with("/*")
        })
        .unwrap_or("");
    // `#name(` is a short-syntax fn. `#[attr]` is conventional meta.
    (t.starts_with('#') && t.len() > 1 && t.as_bytes()[1].is_ascii_alphabetic())
        || contains_outside_comments(src, ":=")
        || contains_outside_comments(src, "$")
        || contains_outside_comments(src, "??")
        || dense_at_sugar(src)
        || dense_range_sugar(src)
        || dense_assign_sugar(src)
        || dense_reduce_sugar(src)
        || dense_inc_sugar(src)
        || dense_len_sugar(src)
        || dense_put_sugar(src)
        || has_hash_fn(src)
}

/// `@cond{` is while-sugar. `//@` comments are not.
fn dense_at_sugar(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if b[i] == b'@'
            && (i == 0 || !is_ident_char(b[i - 1]))
            && i + 1 < b.len()
            && (is_ident_char(b[i + 1]) || b[i + 1] == b'(')
        {
            return true;
        }
        i += 1;
    }
    false
}

fn contains_outside_comments(src: &str, needle: &str) -> bool {
    let b = src.as_bytes();
    let n = needle.as_bytes();
    let mut i = 0;
    while i + n.len() <= b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if &b[i..i + n.len()] == n {
            return true;
        }
        i += 1;
    }
    false
}

fn has_hash_fn(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'/' && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'#' && is_ident_char(b[i + 1]) {
            return true;
        }
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
        }
        i += 1;
    }
    false
}

fn dense_range_sugar(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if b[i] == b'~'
            && i > 0
            && is_ident_char(b[i - 1])
            && i + 1 < b.len()
            && (is_ident_char(b[i + 1]) || b[i + 1].is_ascii_digit())
        {
            return true;
        }
        i += 1;
    }
    false
}

/// `s += e` (and `-=` `*=` `/=` `%=` `&=` `|=` `^=`) is compound assignment.
fn dense_assign_sugar(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'/' && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if matches!(b[i], b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^') && b[i + 1] == b'='
        {
            return true;
        }
        i += 1;
    }
    false
}

/// `+/n` / `*/n` is K-style reduce-over-range.
fn dense_reduce_sugar(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'/' && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if matches!(b[i], b'+' | b'*' | b'|' | b'&')
            && b[i + 1] == b'/'
            && i + 2 < b.len()
            && (is_ident_char(b[i + 2]) || b[i + 2].is_ascii_digit())
            && (i == 0 || !is_ident_char(b[i - 1]))
        {
            return true;
        }
        i += 1;
    }
    false
}

/// `s++` / `s--` after an ident.
fn dense_inc_sugar(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'/' && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if matches!(&src[i..i + 2], "++" | "--") && i > 0 && is_ident_char(b[i - 1]) {
            return true;
        }
        i += 1;
    }
    false
}

/// `xs#` is `xs.len()`. `#name(` is a function, not this.
fn dense_len_sugar(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if b[i] == b'#' && i > 0 && is_ident_char(b[i - 1]) {
            return true;
        }
        i += 1;
    }
    false
}

/// `m[k]<-v` or `xs<-e`.
fn dense_put_sugar(src: &str) -> bool {
    contains_outside_comments(src, "<-")
}

/// Expand short syntax into S-terse. Same program; more tokens; one parser.
///
///   `#add(a I, b I) I = a + b`     → `fn add(a i32, b i32) i32 = a + b`
///   `s Z:= 1`                      → `let mut s: usz = 1`
///   `s:= 1`                        → `let mut s = 1`
///   `i~n { … }`                    → `for i in range(0, n) { … }`
///   `i~2..n { … }`                 → `for i in range(2, n) { … }`
///   `^ true`                       → `return true`
///   `$c{t}{e}`                     → `if c { t } else { e }`
///   `e?d`                          → `match e { Some(v) => v; None => d }`
///   `e|d`                          → `match e { Ok(v) => v; Err(_) => d }`
///   `@c{body}`                     → `while c { body }`
///   `%`                            → `map.new(test.alloc)`
///   `7L`                           → `7i64`
///   `s += i`                       → `s = s + i`
///   `s++` / `s--`                  → `s = s + 1` / `s = s - 1`
///   `xs#`                          → `xs.len()`
///   `+/n` / `+/a..b`               → sum of the range (K plus-over)
///   `*/n` / `*/a..b`               → product of the range (K times-over)
///   `+/xs#` / `*/xs#`              → sum / product of a usz vec (same walk)
///   `|/xs#` / `&/xs#`              → max / min of a usz vec (seed at(0); empty aborts)
///   `m[k]<-v`                      → `m.insert(k, v)`
///   `xs<-e`                        → `xs.push(e)`
///   `m[k]?d`                       → `m.get(k)?d`
pub fn rewrite_dense_to_terse(src: &str) -> String {
    let mut s = expand_dense_shared_signatures(src);
    s = expand_dense_default_map_alias(&s);
    s = expand_dense_zero_arg_fns(&s);
    s = expand_dense_i32_defaults(&s);
    s = expand_dense_interpolation_alias(&s);
    s = expand_dense_effect_aliases(&s);
    s = expand_dense_map_indices(&s);
    s = expand_dense_map_literals(&s);
    s = expand_dense_mapnew(&s);
    s = expand_dense_lits(&s);
    s = expand_dense_len(&s);
    s = expand_dense_reduce(&s);
    s = expand_dense_binds(&s);
    s = expand_dense_ranges(&s);
    s = expand_dense_put(&s);
    s = expand_dense_inc(&s);
    s = expand_dense_assign(&s);
    s = expand_dense_returns(&s);
    s = expand_dense_while(&s);
    s = expand_dense_if(&s);
    s = expand_dense_string_map_gets(&s);
    s = expand_dense_index_get(&s);
    s = expand_dense_result_or(&s);
    s = expand_dense_option_or_real(&s);
    s = expand_dense_fns(&s);
    s = expand_dense_types(&s);
    expand_dense_inferred_alloc_effects(&s)
}

/// Interpolated strings do not need a marker on the dense surface: braces make
/// the intent unambiguous. The parser still receives the conventional `f"…"`
/// form after this expansion.
fn expand_dense_interpolation_alias(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 8);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let start = i;
            i += 1;
            let mut braces = false;
            while i < b.len() {
                if b[i] == b'\\' {
                    i = (i + 2).min(b.len());
                    continue;
                }
                if b[i] == b'{' || b[i] == b'}' {
                    braces = true;
                }
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let prefixed = start > 0 && b[start - 1] == b'f';
            if braces && !prefixed {
                out.push('f');
            }
            out.push_str(&src[start..i]);
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// A nullary dense declaration may use `#name=body`; the parser receives the
/// ordinary `#name()=body` form.
fn expand_dense_zero_arg_fns(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 8);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'#'
            && (i == 0 || b[i - 1].is_ascii_whitespace() || matches!(b[i - 1], b';' | b'}'))
            && i + 1 < b.len()
            && is_ident_char(b[i + 1])
        {
            let mut end = i + 2;
            while end < b.len() && is_ident_char(b[end]) {
                end += 1;
            }
            if end < b.len() && b[end] == b'=' {
                out.push_str(&src[i..end]);
                out.push_str("()=");
                i = end + 1;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `M` is the default string-to-i32 map type on the dense surface. Generic
/// maps keep the explicit `M[K,V]` spelling; the alias targets the common
/// dictionary shape used by agent-generated code.
fn expand_dense_default_map_alias(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0;
    while i < b.len() {
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
                i += 1;
            }
            continue;
        }
        if b[i] == b'M'
            && (i == 0 || (!is_ident_char(b[i - 1]) && b[i - 1] != b'#'))
            && (i + 1 == b.len() || !is_ident_char(b[i + 1]))
            && b.get(i + 1) != Some(&b'[')
        {
            out.push_str("Map[String,i32]");
            i += 1;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Dense functions may omit an effect row when the body makes allocation
/// obvious.  The conventional surface keeps explicit rows mandatory, but an
/// agent-facing spelling should not pay for metadata the compiler can derive.
/// This pass runs after map literals have expanded to `map.new(test.alloc)` so
/// the inferred row is still the ordinary checked `alloc[a]` capability.
fn expand_dense_inferred_alloc_effects(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'f'
            && src[i..].starts_with("fn ")
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric() && b[i - 1] != b'_')
        {
            let start = i;
            let Some(open_rel) = src[i..].find('(') else {
                out.push(b[i] as char);
                i += 1;
                continue;
            };
            let open = i + open_rel;
            let Some(close) = find_matching_ascii(b, open, b'(', b')') else {
                out.push(b[i] as char);
                i += 1;
                continue;
            };
            let mut eq = close + 1;
            while eq < b.len() && b[eq].is_ascii_whitespace() {
                eq += 1;
            }
            while eq + 1 < b.len() && b[eq] != b'=' {
                if b[eq] == b'!' {
                    break;
                }
                eq += 1;
            }
            if eq >= b.len() || b[eq] != b'=' {
                out.push_str(&src[start..=close]);
                i = close + 1;
                continue;
            }
            let body_start = eq + 1;
            let mut body_end = body_start;
            while body_end < b.len() && b[body_end].is_ascii_whitespace() {
                body_end += 1;
            }
            let has_alloc = if body_end < b.len() && b[body_end] == b'{' {
                find_matching_ascii(b, body_end, b'{', b'}')
                    .map(|close_body| src[body_end..=close_body].contains("map.new(test.alloc)"))
                    .unwrap_or(false)
            } else {
                false
            };
            if has_alloc {
                out.push_str(&src[start..eq]);
                out.push_str(" !{alloc[a]}");
                out.push('=');
                i = eq + 1;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn expand_dense_types(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 8);
    let mut i = 0;
    while i < b.len() {
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
                i += 1;
            }
            continue;
        }
        if is_ident_char(b[i]) {
            let start = i;
            while i < b.len() && is_ident_char(b[i]) {
                i += 1;
            }
            let word = &src[start..i];
            // `f"..."` is interpolation, not the f32 glyph.
            if word == "f" && i < b.len() && b[i] == b'"' {
                out.push_str(word);
            } else if let Some((_, full)) = DENSE_TYPES.iter().find(|(g, _)| *g == word) {
                out.push_str(full);
            } else {
                out.push_str(word);
            }
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Dense signatures default omitted parameter and result types to `I` (i32).
///
/// The default removes two BPE pieces per ordinary integer parameter and one
/// for the result while preserving explicit annotations for every other type:
/// `#add(a,b)=a+b` is exactly `#add(a I,b I) I=a+b`.
fn expand_dense_i32_defaults(src: &str) -> String {
    rewrite_dense_signatures(src, false)
}

/// Mechanical inverse used by `ax fmt`/`to_dense`: explicit i32 annotations in
/// function signatures are omitted because the dense surface reconstructs them.
fn pack_dense_i32_defaults(src: &str) -> String {
    rewrite_dense_signatures(src, true)
}

fn rewrite_dense_signatures(src: &str, pack: bool) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + if pack { 0 } else { 16 });
    let mut cursor = 0;
    let mut search = 0;

    while let Some(rel) = src[search..].find('#') {
        let hash = search + rel;
        let at_edge =
            hash == 0 || b[hash - 1].is_ascii_whitespace() || matches!(b[hash - 1], b';' | b'}');
        if !at_edge || hash + 1 >= b.len() || !b[hash + 1].is_ascii_alphabetic() {
            search = hash + 1;
            continue;
        }
        let mut name_end = hash + 1;
        while name_end < b.len() && is_ident_char(b[name_end]) {
            name_end += 1;
        }
        let mut open = name_end;
        while open < b.len() && b[open].is_ascii_whitespace() {
            open += 1;
        }
        if open >= b.len() || b[open] != b'(' {
            search = hash + 1;
            continue;
        }
        let Some(close) = find_matching_ascii(b, open, b'(', b')') else {
            break;
        };

        out.push_str(&src[cursor..open + 1]);
        out.push_str(&rewrite_dense_params(&src[open + 1..close], pack));
        out.push(')');

        let mut after_ws = close + 1;
        while after_ws < b.len() && b[after_ws].is_ascii_whitespace() {
            after_ws += 1;
        }
        if pack
            && after_ws < b.len()
            && b[after_ws] == b'I'
            && (after_ws + 1 == b.len() || !is_ident_char(b[after_ws + 1]))
        {
            let mut next = after_ws + 1;
            while next < b.len() && b[next].is_ascii_whitespace() {
                next += 1;
            }
            if next < b.len() && matches!(b[next], b'=' | b'!') {
                cursor = after_ws + 1;
                search = cursor;
                continue;
            }
        } else if !pack && after_ws < b.len() && matches!(b[after_ws], b'=' | b'!') {
            out.push_str(" I");
        }
        cursor = close + 1;
        search = cursor;
    }
    out.push_str(&src[cursor..]);
    out
}

fn rewrite_dense_params(params: &str, pack: bool) -> String {
    let b = params.as_bytes();
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut square = 0i32;
    let mut paren = 0i32;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'[' => square += 1,
            b']' => square -= 1,
            b'(' => paren += 1,
            b')' => paren -= 1,
            b',' if square == 0 && paren == 0 => {
                pieces.push(&params[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < params.len() || !params.trim().is_empty() {
        pieces.push(&params[start..]);
    }
    pieces
        .into_iter()
        .map(|piece| {
            let p = piece.trim();
            if pack {
                let mut words = p.split_whitespace();
                match (words.next(), words.next(), words.next()) {
                    (Some(name), Some("I"), None)
                        if name.as_bytes().iter().all(|c| is_ident_char(*c)) =>
                    {
                        name.to_string()
                    }
                    _ => p.to_string(),
                }
            } else if !p.is_empty() && p.as_bytes().iter().all(|c| is_ident_char(*c)) {
                format!("{p} I")
            } else {
                p.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn find_matching_ascii(b: &[u8], open_at: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open_at;
    while i < b.len() {
        if matches!(b[i], b'"' | b'`') {
            let quote = b[i];
            i += 1;
            while i < b.len() && b[i] != quote {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
        } else if b[i] == open {
            depth += 1;
        } else if b[i] == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// `#f(a,b:T)=body` shares `T` across every parameter and the result.
/// This is useful for non-i32 monomorphic helpers while the even shorter
/// `#f(a,b)=body` remains the i32 default.
fn expand_dense_shared_signatures(src: &str) -> String {
    rewrite_shared_signatures(src, false)
}

fn pack_dense_shared_signatures(src: &str) -> String {
    rewrite_shared_signatures(src, true)
}

fn rewrite_shared_signatures(src: &str, pack: bool) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0;
    let mut search = 0;
    while let Some(rel) = src[search..].find('#') {
        let hash = search + rel;
        if hash + 1 >= b.len() || !b[hash + 1].is_ascii_alphabetic() {
            search = hash + 1;
            continue;
        }
        let mut open = hash + 1;
        while open < b.len() && is_ident_char(b[open]) {
            open += 1;
        }
        while open < b.len() && b[open].is_ascii_whitespace() {
            open += 1;
        }
        if open >= b.len() || b[open] != b'(' {
            search = hash + 1;
            continue;
        }
        let Some(close) = find_matching_ascii(b, open, b'(', b')') else {
            break;
        };
        let params = &src[open + 1..close];
        let mut after = close + 1;
        while after < b.len() && b[after].is_ascii_whitespace() {
            after += 1;
        }

        if pack {
            let Some((names, ty)) = common_dense_param_type(params) else {
                search = close + 1;
                continue;
            };
            if !src[after..].starts_with(&ty)
                || src
                    .as_bytes()
                    .get(after + ty.len())
                    .is_some_and(|c| is_ident_char(*c))
            {
                search = close + 1;
                continue;
            }
            let mut next = after + ty.len();
            while next < b.len() && b[next].is_ascii_whitespace() {
                next += 1;
            }
            if next >= b.len() || !matches!(b[next], b'=' | b'!') {
                search = close + 1;
                continue;
            }
            out.push_str(&src[cursor..open + 1]);
            out.push_str(&names.join(","));
            out.push(':');
            out.push_str(&ty);
            out.push(')');
            cursor = after + ty.len();
            search = cursor;
        } else {
            let Some(colon) = top_level_colon(params) else {
                search = close + 1;
                continue;
            };
            let names: Vec<&str> = params[..colon].split(',').map(str::trim).collect();
            let ty = params[colon + 1..].trim();
            if names.is_empty()
                || ty.is_empty()
                || names
                    .iter()
                    .any(|n| n.is_empty() || !n.as_bytes().iter().all(|c| is_ident_char(*c)))
                || (after < b.len() && !matches!(b[after], b'=' | b'!'))
            {
                search = close + 1;
                continue;
            }
            out.push_str(&src[cursor..open + 1]);
            out.push_str(
                &names
                    .iter()
                    .map(|name| format!("{name} {ty}"))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.push(')');
            out.push(' ');
            out.push_str(ty);
            cursor = close + 1;
            search = cursor;
        }
    }
    out.push_str(&src[cursor..]);
    out
}

fn top_level_colon(params: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in params.bytes().enumerate() {
        match c {
            b'[' | b'(' => depth += 1,
            b']' | b')' => depth -= 1,
            b':' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

fn common_dense_param_type(params: &str) -> Option<(Vec<&str>, String)> {
    let mut names = Vec::new();
    let mut common = None::<&str>;
    for piece in params.split(',') {
        let mut words = piece.split_whitespace();
        let (Some(name), Some(ty), None) = (words.next(), words.next(), words.next()) else {
            return None;
        };
        if !name.as_bytes().iter().all(|c| is_ident_char(*c)) {
            return None;
        }
        if common.is_some_and(|c| c != ty) {
            return None;
        }
        common = Some(ty);
        names.push(name);
    }
    Some((names, common?.to_string()))
}

/// The test allocator is the overwhelmingly common allocator in generated Ax
/// snippets. `!a` is a two-token spelling of `!alloc[a]` (four BPE tokens).
fn expand_dense_effect_aliases(src: &str) -> String {
    replace_dense_outside_strings(src, "!a", "!alloc[a]", true)
}

fn pack_dense_effect_aliases(src: &str) -> String {
    replace_dense_outside_strings(src, "!alloc[a]", "!a", false)
}

fn replace_dense_outside_strings(
    src: &str,
    from: &str,
    to: &str,
    require_boundary: bool,
) -> String {
    let b = src.as_bytes();
    let needle = from.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if matches!(b[i], b'"' | b'`') {
            let quote = b[i];
            let start = i;
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    i += 1;
                } else if b[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push_str(&src[start..i]);
            continue;
        }
        if i + needle.len() <= b.len()
            && &b[i..i + needle.len()] == needle
            && (!require_boundary
                || i + needle.len() == b.len()
                || !is_ident_char(b[i + needle.len()]))
        {
            out.push_str(to);
            i += needle.len();
            continue;
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn expand_dense_fns(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        // `#name` at a statement edge → `fn name`. Not `#[attr]`.
        if b[i] == b'#'
            && i + 1 < b.len()
            && is_ident_char(b[i + 1])
            && (i == 0 || b[i - 1].is_ascii_whitespace() || b[i - 1] == b';' || b[i - 1] == b'}')
        {
            out.push_str("fn ");
            i += 1;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    ensure_decl_semicolons(&out)
}

/// A dense one-liner may omit `;` after `= body`. Terse still wants it.
fn ensure_decl_semicolons(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 4);
    for line in src.split_inclusive('\n') {
        let t = line.trim_end();
        let body = t.trim_start();
        if (body.starts_with("fn ") || looks_like_bare_fn(body))
            && !t.ends_with(';')
            && !t.ends_with('{')
        {
            out.push_str(t);
            out.push(';');
            if line.ends_with('\n') {
                out.push('\n');
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

fn looks_like_bare_fn(t: &str) -> bool {
    let b = t.as_bytes();
    if b.is_empty() || !is_ident_char(b[0]) {
        return false;
    }
    if t.starts_with("fn ") || t.starts_with("type ") || t.starts_with("test ") {
        return false;
    }
    let mut i = 0;
    while i < b.len() && is_ident_char(b[i]) {
        i += 1;
    }
    i < b.len() && b[i] == b'(' && t.contains('=')
}

fn expand_dense_binds(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 32);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        // `name [Type] :=` → `let mut name[: Type] =`
        if is_ident_char(b[i]) && (i == 0 || !is_ident_char(b[i - 1])) {
            let start = i;
            while i < b.len() && is_ident_char(b[i]) {
                i += 1;
            }
            let name = &src[start..i];
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            // optional type: `I` or `M[S, L]`
            let mut ty = None;
            let mut k = j;
            if k < b.len() && is_ident_char(b[k]) {
                let ts = k;
                while k < b.len() && is_ident_char(b[k]) {
                    k += 1;
                }
                if k < b.len() && b[k] == b'[' {
                    let mut depth = 0i32;
                    while k < b.len() {
                        match b[k] {
                            b'[' => depth += 1,
                            b']' => {
                                depth -= 1;
                                k += 1;
                                if depth == 0 {
                                    break;
                                }
                                continue;
                            }
                            _ => {}
                        }
                        k += 1;
                    }
                }
                let mut m = k;
                while m < b.len() && b[m].is_ascii_whitespace() {
                    m += 1;
                }
                if m + 1 < b.len() && b[m] == b':' && b[m + 1] == b'=' {
                    ty = Some(&src[ts..k]);
                    j = m;
                }
            }
            if j + 1 < b.len() && b[j] == b':' && b[j + 1] == b'=' {
                out.push_str("let mut ");
                out.push_str(name);
                if let Some(t) = ty {
                    out.push_str(": ");
                    out.push_str(t);
                }
                out.push_str(" =");
                i = j + 2;
                continue;
            }
            out.push_str(name);
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn expand_dense_ranges(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 32);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        // `ident ~ lo[..hi]` just before `{`
        if is_ident_char(b[i]) && (i == 0 || !is_ident_char(b[i - 1])) {
            let start = i;
            while i < b.len() && is_ident_char(b[i]) {
                i += 1;
            }
            let name = &src[start..i];
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'~' {
                j += 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let lo_s = j;
                if take_dense_bound(src, b, &mut j) {
                    let first = &src[lo_s..j];
                    let mut k = j;
                    while k < b.len() && b[k].is_ascii_whitespace() {
                        k += 1;
                    }
                    if k + 1 < b.len() && &src[k..k + 2] == ".." {
                        k += 2;
                        while k < b.len() && b[k].is_ascii_whitespace() {
                            k += 1;
                        }
                        let hi_s = k;
                        if take_dense_bound(src, b, &mut k) {
                            out.push_str("for ");
                            out.push_str(name);
                            out.push_str(" in range(");
                            out.push_str(first);
                            out.push_str(", ");
                            out.push_str(&src[hi_s..k]);
                            out.push(')');
                            i = k;
                            continue;
                        }
                    } else {
                        out.push_str("for ");
                        out.push_str(name);
                        out.push_str(" in range(0, ");
                        out.push_str(first);
                        out.push(')');
                        i = j;
                        continue;
                    }
                }
            }
            out.push_str(name);
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Ident, integer, `xs.len()`, or `xs#` (the last is already expanded if
/// [`expand_dense_len`] ran first).
fn take_dense_bound(src: &str, b: &[u8], j: &mut usize) -> bool {
    let start = *j;
    if *j >= b.len() || !(is_ident_char(b[*j]) || b[*j].is_ascii_digit()) {
        return false;
    }
    while *j < b.len() && (is_ident_char(b[*j]) || b[*j].is_ascii_digit()) {
        *j += 1;
    }
    if *j + 6 <= b.len() && &src[*j..*j + 6] == ".len()" {
        *j += 6;
    }
    start < *j
}

/// `s++` / `s--` → `s = s + 1` / `s = s - 1`.
fn expand_dense_inc(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if i + 1 < b.len()
            && matches!(&src[i..i + 2], "++" | "--")
            && !out.is_empty()
            && (out.as_bytes().last().unwrap().is_ascii_alphabetic()
                || *out.as_bytes().last().unwrap() == b'_')
        {
            let op = if b[i] == b'+' { '+' } else { '-' };
            let mut k = out.len();
            while k > 0 && (is_ident_char(out.as_bytes()[k - 1]) || out.as_bytes()[k - 1] == b'.') {
                k -= 1;
            }
            let lhs = out[k..].to_string();
            if !lhs.is_empty() {
                out.truncate(k);
                out.push_str(&lhs);
                out.push_str(" = ");
                out.push_str(&lhs);
                out.push(' ');
                out.push(op);
                out.push_str(" 1");
                i += 2;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `xs#` → `xs.len()`. Not `#name` (a function) and not `7#`.
fn expand_dense_len(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 8);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b[i] == b'#' && i > 0 && (b[i - 1].is_ascii_alphabetic() || b[i - 1] == b'_') {
            out.push_str(".len()");
            i += 1;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `s += e` → `s = s + e` (same IR; the name is not re-evaluated).
fn expand_dense_assign(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if i + 1 < b.len()
            && matches!(b[i], b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^')
            && b[i + 1] == b'='
        {
            let op = b[i] as char;
            // Walk back over the just-written lvalue atom (`name` or `name.field`).
            let mut k = out.len();
            while k > 0 && out.as_bytes()[k - 1].is_ascii_whitespace() {
                k -= 1;
            }
            let end = k;
            while k > 0 && (is_ident_char(out.as_bytes()[k - 1]) || out.as_bytes()[k - 1] == b'.') {
                k -= 1;
            }
            if k < end {
                let lhs = out[k..end].to_string();
                out.truncate(k);
                out.push_str(&lhs);
                out.push_str(" = ");
                out.push_str(&lhs);
                out.push(' ');
                out.push(op);
                out.push(' ');
                i += 2;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `m[k]<-v` → `m.insert(k, v)`; `xs<-e` → `xs.push(e)`.
/// `m[k]?d` is already `Index` + option-or after parse; here we only
/// rewrite the write form. Key/value atoms: ident, number, or `"…"`.
fn expand_dense_put(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if i + 1 < b.len() && &src[i..i + 2] == "<-" {
            let mut k = out.len();
            while k > 0 && out.as_bytes()[k - 1].is_ascii_whitespace() {
                k -= 1;
            }
            // `recv[key]` just written?
            if k > 0 && out.as_bytes()[k - 1] == b']' {
                let mut d = 0i32;
                let end = k;
                while k > 0 {
                    k -= 1;
                    match out.as_bytes()[k] {
                        b']' => d += 1,
                        b'[' => {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let key = out[k + 1..end - 1].trim().to_string();
                while k > 0
                    && (is_ident_char(out.as_bytes()[k - 1]) || out.as_bytes()[k - 1] == b'.')
                {
                    k -= 1;
                }
                let recv = out[k..]
                    .trim_end_matches(|c: char| c == '[' || c.is_ascii_whitespace())
                    .to_string();
                // `recv` still includes `[key]`; strip it.
                let recv = if let Some(p) = recv.rfind('[') {
                    recv[..p].to_string()
                } else {
                    recv
                };
                if !recv.is_empty() && !key.is_empty() {
                    out.truncate(k);
                    out.push_str(&recv);
                    // Numeric / ident index on a vec is `set`; a string key is `insert`.
                    let meth = if key_looks_like_map(&key) {
                        ".insert("
                    } else {
                        ".set("
                    };
                    out.push_str(meth);
                    out.push_str(&key);
                    out.push_str(", ");
                    i += 2;
                    while i < b.len() && b[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    continue;
                }
            } else {
                // `recv<-e`
                while k > 0
                    && (is_ident_char(out.as_bytes()[k - 1]) || out.as_bytes()[k - 1] == b'.')
                {
                    k -= 1;
                }
                let recv = out[k..].to_string();
                if !recv.is_empty() {
                    out.truncate(k);
                    out.push_str(&recv);
                    out.push_str(".push(");
                    i += 2;
                    while i < b.len() && b[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    // take one atom / string / call, then close
                    let vs = i;
                    if i < b.len() && b[i] == b'"' {
                        i += 1;
                        while i < b.len() && b[i] != b'"' {
                            i += 1;
                        }
                        if i < b.len() {
                            i += 1;
                        }
                    } else if i < b.len() && (is_ident_char(b[i]) || b[i].is_ascii_digit()) {
                        while i < b.len()
                            && (is_ident_char(b[i]) || b[i].is_ascii_digit() || b[i] == b'.')
                        {
                            i += 1;
                        }
                        if i < b.len() && b[i] == b'(' {
                            let mut d = 0i32;
                            while i < b.len() {
                                if b[i] == b'(' {
                                    d += 1;
                                } else if b[i] == b')' {
                                    d -= 1;
                                    i += 1;
                                    if d == 0 {
                                        break;
                                    }
                                    continue;
                                }
                                i += 1;
                            }
                        }
                    }
                    out.push_str(&src[vs..i]);
                    out.push(')');
                    continue;
                }
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    // Close any insert( that is still open: we left the value for the rest
    // of the scan. A following `;` / `}` / newline closes it.
    close_open_inserts(&out)
}

fn close_open_inserts(src: &str) -> String {
    // `recv.insert(k, VALUE` without `)` — close before `;` `}` newline.
    let mut out = String::with_capacity(src.len() + 4);
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if i + 5 <= b.len()
            && (&src[i..i + 5] == ".set(" || (i + 8 <= b.len() && &src[i..i + 8] == ".insert("))
        {
            let n = if &src[i..i + 5] == ".set(" { 5 } else { 8 };
            out.push_str(&src[i..i + n]);
            i += n;
            let mut d = 1i32;
            while i < b.len() {
                if b[i] == b'(' {
                    d += 1;
                } else if b[i] == b')' {
                    d -= 1;
                    if d == 0 {
                        out.push(')');
                        i += 1;
                        break;
                    }
                } else if d == 1 && matches!(b[i], b';' | b'}' | b'\n') {
                    out.push(')');
                    break;
                }
                out.push(b[i] as char);
                i += 1;
            }
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// A string literal cannot be a vector index, so `m["key"]` is an implicit
/// zero-default map read on the dense surface. The explicit `m[k]?d` spelling
/// remains for variable keys and nonzero fallbacks.
fn expand_dense_string_map_gets(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 8);
    let mut i = 0;
    while i < b.len() {
        if matches!(b[i], b'"' | b'`') {
            let q = b[i];
            out.push(q as char);
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'\\' {
                    if i + 1 < b.len() {
                        i += 1;
                        out.push(b[i] as char);
                    }
                } else if b[i] == q {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b[i] == b']' && !out.is_empty() {
            let mut k = out.len() - 1;
            let mut depth = 0i32;
            while k > 0 {
                match out.as_bytes()[k] {
                    b']' => depth += 1,
                    b'[' => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
                k -= 1;
            }
            if depth == 0 && out.as_bytes()[k] == b'[' {
                let key = out[k + 1..].trim();
                let mut next = i + 1;
                while next < b.len() && b[next].is_ascii_whitespace() {
                    next += 1;
                }
                let is_get = key.len() >= 2
                    && key.starts_with('"')
                    && key.ends_with('"')
                    && !matches!(b.get(next).copied(), Some(b'?') | Some(b'<') | Some(b'='));
                out.push(']');
                if is_get {
                    out.push('?');
                }
                i += 1;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `recv[k]?` → `recv.get(k)?` so the existing option-or rewrite applies.
fn expand_dense_index_get(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 8);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b[i] == b'?'
            && b.get(i + 1) != Some(&b'?')
            && !out.is_empty()
            && out.as_bytes().last() == Some(&b']')
        {
            let mut k = out.len() - 1;
            let mut d = 0i32;
            while k > 0 {
                match out.as_bytes()[k] {
                    b']' => d += 1,
                    b'[' => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                k -= 1;
            }
            if d == 0 && out.as_bytes()[k] == b'[' {
                let key = out[k + 1..out.len() - 1].trim().to_string();
                let mut r = k;
                while r > 0
                    && (is_ident_char(out.as_bytes()[r - 1]) || out.as_bytes()[r - 1] == b'.')
                {
                    r -= 1;
                }
                let recv = out[r..k].to_string();
                if !recv.is_empty() && !key.is_empty() {
                    out.truncate(r);
                    out.push_str(&recv);
                    out.push_str(".get(");
                    out.push_str(&key);
                    out.push(')');
                    out.push('?');
                    // A bare postfix `?` is the dense zero-default lookup;
                    // the explicit `?d` form remains available for any other
                    // fallback. This saves the repeated `0` in map-heavy code.
                    let mut next = i + 1;
                    let mut newline = false;
                    while next < b.len() && b[next].is_ascii_whitespace() {
                        newline |= b[next] == b'\n';
                        next += 1;
                    }
                    if newline
                        || matches!(
                            b.get(next).copied(),
                            None | Some(b';')
                                | Some(b'}')
                                | Some(b')')
                                | Some(b']')
                                | Some(b'+')
                                | Some(b'-')
                                | Some(b'*')
                                | Some(b'/')
                                | Some(b',')
                                | Some(b':')
                        )
                    {
                        out.push('0');
                    }
                    i += 1;
                    continue;
                }
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `+/n` → `{ let mut _r = 0; for _i in range(0, n) { _r = _r + _i; }; _r }`
/// `+/a..b` same with `range(a, b)`. `*/` is the product (init 1, `*`).
/// Hygienic temps: `_r` / `_i` cannot appear in a user program as a type glyph
/// and are not reserved — they are ordinary idents the rest of the rewrite
/// leaves alone. Nested `+/` is expanded left-to-right; the inner form is
/// already gone before the outer body is parsed.
fn expand_dense_reduce(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 64);
    let mut i = 0;
    let mut gen = 0u32;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if i + 2 < b.len()
            && matches!(b[i], b'+' | b'*' | b'|' | b'&')
            && b[i + 1] == b'/'
            && (is_ident_char(b[i + 2]) || b[i + 2].is_ascii_digit())
            && (i == 0 || !is_ident_char(b[i - 1]))
        {
            let op = b[i] as char;
            i += 2;
            let lo_s = i;
            while i < b.len() && (is_ident_char(b[i]) || b[i].is_ascii_digit()) {
                i += 1;
            }
            if i > lo_s {
                let first = &src[lo_s..i];
                // `+/xs#` / `+/xs.len()` is a vec walk, not `range(0, xs)`.
                // `#` may already have become `.len()` if expand_dense_len ran first.
                let mut is_vec = false;
                if i < b.len() && b[i] == b'#' {
                    i += 1;
                    is_vec = true;
                } else if i + 6 <= b.len() && &src[i..i + 6] == ".len()" {
                    i += 6;
                    is_vec = true;
                }
                if is_vec && first.chars().all(|c| c.is_ascii_alphabetic() || c == '_') {
                    let acc = format!("_r{gen}");
                    let ix = format!("_i{gen}");
                    gen += 1;
                    if matches!(op, '|' | '&') {
                        // Seed with xs.at(0); walk 1..len. Empty vec aborts
                        // on at(0) — there is no identity for max/min.
                        let cmp = if op == '|' { '>' } else { '<' };
                        out.push_str("{ let mut ");
                        out.push_str(&acc);
                        out.push_str(" = ");
                        out.push_str(first);
                        out.push_str(".at(0); for ");
                        out.push_str(&ix);
                        out.push_str(" in range(1, ");
                        out.push_str(first);
                        out.push_str(".len()) { if ");
                        out.push_str(first);
                        out.push_str(".at(");
                        out.push_str(&ix);
                        out.push_str(") ");
                        out.push(cmp);
                        out.push(' ');
                        out.push_str(&acc);
                        out.push_str(" { ");
                        out.push_str(&acc);
                        out.push_str(" = ");
                        out.push_str(first);
                        out.push_str(".at(");
                        out.push_str(&ix);
                        out.push_str("); }; }; ");
                        out.push_str(&acc);
                        out.push_str(" }");
                    } else {
                        let init = if op == '*' { "1usz" } else { "0usz" };
                        out.push_str("{ let mut ");
                        out.push_str(&acc);
                        out.push_str(" = ");
                        out.push_str(init);
                        out.push_str("; for ");
                        out.push_str(&ix);
                        out.push_str(" in range(0, ");
                        out.push_str(first);
                        out.push_str(".len()) { ");
                        out.push_str(&acc);
                        out.push_str(" = ");
                        out.push_str(&acc);
                        out.push(' ');
                        out.push(op);
                        out.push(' ');
                        out.push_str(first);
                        out.push_str(".at(");
                        out.push_str(&ix);
                        out.push_str("); }; ");
                        out.push_str(&acc);
                        out.push_str(" }");
                    }
                    continue;
                }
                let mut lo = "0";
                let mut hi = first;
                let mut k = i;
                while k < b.len() && b[k].is_ascii_whitespace() {
                    k += 1;
                }
                if k + 1 < b.len() && &src[k..k + 2] == ".." {
                    k += 2;
                    while k < b.len() && b[k].is_ascii_whitespace() {
                        k += 1;
                    }
                    let hi_s = k;
                    while k < b.len() && (is_ident_char(b[k]) || b[k].is_ascii_digit()) {
                        k += 1;
                    }
                    if k > hi_s {
                        lo = first;
                        hi = &src[hi_s..k];
                        i = k;
                    }
                }
                let acc = format!("_r{gen}");
                let ix = format!("_i{gen}");
                gen += 1;
                // `range` is usz → usz, so the reduction is usz. A bare `0`
                // would default to i32 and fail at `_r + _i`.
                let init = if op == '*' { "1usz" } else { "0usz" };
                out.push_str("{ let mut ");
                out.push_str(&acc);
                out.push_str(" = ");
                out.push_str(init);
                out.push_str("; for ");
                out.push_str(&ix);
                out.push_str(" in range(");
                out.push_str(lo);
                out.push_str(", ");
                out.push_str(hi);
                out.push_str(") { ");
                out.push_str(&acc);
                out.push_str(" = ");
                out.push_str(&acc);
                out.push(' ');
                out.push(op);
                out.push(' ');
                out.push_str(&ix);
                out.push_str("; }; ");
                out.push_str(&acc);
                out.push_str(" }");
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn expand_dense_returns(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 8);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'^'
            && (i == 0 || b[i - 1].is_ascii_whitespace() || b[i - 1] == b'{' || b[i - 1] == b';')
            && i + 1 < b.len()
            && !matches!(b[i + 1], b'=' | b'^')
        {
            out.push_str("return ");
            i += 1;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `%{"k":2L}` is an inferred `M[S,L]` literal allocated from `test.alloc`.
/// It lowers to the existing map-new and insert operations, so effects, types,
/// and backend behavior remain identical to the expanded spelling.
fn expand_dense_map_literals(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 32);
    let mut i = 0;
    let mut serial = 0usize;
    while i < b.len() {
        if matches!(b[i], b'"' | b'`') {
            let quote = b[i];
            let start = i;
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    i += 1;
                } else if b[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push_str(&src[start..i]);
            continue;
        }
        if b[i] == b'%' && b.get(i + 1) == Some(&b'{') {
            if let Some(close) = find_matching_ascii(b, i + 1, b'{', b'}') {
                let inner = &src[i + 2..close];
                if let Some(entries) = dense_map_entries(inner) {
                    if !entries.is_empty() {
                        let key_ty = dense_atom_type(&entries[0].0);
                        let value_ty = dense_atom_type(&entries[0].1);
                        if let (Some(key_ty), Some(value_ty)) = (key_ty, value_ty) {
                            if entries.iter().all(|(k, v)| {
                                dense_atom_type(k) == Some(key_ty)
                                    && dense_atom_type(v) == Some(value_ty)
                            }) {
                                let mut name = format!("__axm{serial}");
                                while src.contains(&name) {
                                    serial += 1;
                                    name = format!("__axm{serial}");
                                }
                                serial += 1;
                                if out.ends_with('=') && !out.ends_with(":=") {
                                    out.pop();
                                    out.push_str(":=");
                                } else if out.as_bytes().last().is_some_and(|c| is_ident_char(*c)) {
                                    let mut name_start = out.len() - 1;
                                    while name_start > 0
                                        && is_ident_char(out.as_bytes()[name_start - 1])
                                    {
                                        name_start -= 1;
                                    }
                                    if name_start == 0
                                        || matches!(
                                            out.as_bytes()[name_start - 1],
                                            b'{' | b';' | b'\n'
                                        )
                                    {
                                        out.push_str(":=");
                                    }
                                }
                                out.push_str("{ ");
                                out.push_str(&name);
                                out.push_str(" M[");
                                out.push_str(key_ty);
                                out.push(',');
                                out.push_str(value_ty);
                                out.push_str("]:= %; ");
                                for (key, value) in &entries {
                                    out.push_str(&name);
                                    out.push('[');
                                    out.push_str(&dense_map_key_expr(key));
                                    out.push_str("]<-");
                                    out.push_str(value);
                                    out.push_str("; ");
                                }
                                out.push_str(&name);
                                out.push_str(" }");
                                i = close + 1;
                                continue;
                            }
                        }
                    }
                }
            }
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn dense_map_entries(inner: &str) -> Option<Vec<(String, String)>> {
    let mut entries = Vec::new();
    for piece in split_dense_top_level(inner, b',') {
        let colon = find_dense_top_level(piece, b':')?;
        let key = piece[..colon].trim();
        let value = piece[colon + 1..].trim();
        if key.is_empty() || value.is_empty() {
            return None;
        }
        entries.push((key.to_string(), value.to_string()));
    }
    Some(entries)
}

fn split_dense_top_level(src: &str, separator: u8) -> Vec<&str> {
    let b = src.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut depths = [0i32; 3];
    let mut quote = None;
    let mut i = 0;
    while i < b.len() {
        if let Some(q) = quote {
            if b[i] == b'\\' {
                i += 1;
            } else if b[i] == q {
                quote = None;
            }
        } else {
            match b[i] {
                b'"' | b'`' => quote = Some(b[i]),
                b'(' => depths[0] += 1,
                b')' => depths[0] -= 1,
                b'[' => depths[1] += 1,
                b']' => depths[1] -= 1,
                b'{' => depths[2] += 1,
                b'}' => depths[2] -= 1,
                c if c == separator && depths == [0, 0, 0] => {
                    result.push(&src[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    if start < src.len() || !src.trim().is_empty() {
        result.push(&src[start..]);
    }
    result
}

fn find_dense_top_level(src: &str, needle: u8) -> Option<usize> {
    let pieces = split_dense_top_level(src, needle);
    if pieces.len() != 2 {
        return None;
    }
    Some(pieces[0].len())
}

fn dense_atom_type(atom: &str) -> Option<&'static str> {
    let atom = atom.trim().trim_start_matches('-');
    if atom.starts_with('"') && atom.ends_with('"') {
        return Some("S");
    }
    if !atom.is_empty()
        && atom.as_bytes().iter().all(|c| is_ident_char(*c))
        && atom.as_bytes()[0].is_ascii_alphabetic()
    {
        return Some("S");
    }
    if !atom
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .chars()
        .all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    match atom.as_bytes().last().copied() {
        Some(b'L') => Some("L"),
        Some(b'Z') => Some("Z"),
        Some(b'W') => Some("W"),
        Some(b'I') | Some(b'0'..=b'9') => Some("I"),
        _ => None,
    }
}

fn dense_map_key_spelling(key: &str) -> String {
    let key = key.trim();
    if key.len() >= 2 && key.starts_with('"') && key.ends_with('"') {
        let inner = &key[1..key.len() - 1];
        if !inner.is_empty()
            && inner.as_bytes()[0].is_ascii_alphabetic()
            && inner.as_bytes().iter().all(|c| is_ident_char(*c))
        {
            return inner.to_string();
        }
    }
    key.to_string()
}

fn dense_map_key_expr(key: &str) -> String {
    let key = key.trim();
    if key.len() >= 2 && key.starts_with('"') && key.ends_with('"') {
        key.to_string()
    } else {
        format!("\"{key}\"")
    }
}

fn map_literal_bindings(src: &str) -> Vec<String> {
    let b = src.as_bytes();
    let mut names = Vec::new();
    let mut scan = 0;
    while let Some(rel) = src[scan..].find("%{") {
        let at = scan + rel;
        let mut start = at;
        while start > 0 && is_ident_char(b[start - 1]) {
            start -= 1;
        }
        if start < at
            && (start == 0 || matches!(b[start - 1], b'{' | b';' | b'\n'))
            && !names.iter().any(|n| n == &src[start..at])
        {
            names.push(src[start..at].to_string());
        }
        scan = at + 2;
    }
    names
}

fn rewrite_known_map_indices(src: &str, pack: bool) -> String {
    let mut out = src.to_string();
    for name in map_literal_bindings(src) {
        out = rewrite_one_map_indices(&out, &name, pack);
    }
    out
}

fn rewrite_one_map_indices(src: &str, name: &str, pack: bool) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if matches!(b[i], b'"' | b'`') {
            let q = b[i];
            let start = i;
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    i = (i + 2).min(b.len());
                } else {
                    i += 1;
                    if b[i - 1] == q {
                        break;
                    }
                }
            }
            out.push_str(&src[start..i]);
            continue;
        }
        if src[i..].starts_with(name)
            && (i == 0 || !is_ident_char(b[i - 1]))
            && i + name.len() < b.len()
            && b[i + name.len()] == b'['
        {
            let open = i + name.len();
            let Some(close) = find_matching_ascii(b, open, b'[', b']') else {
                out.push(b[i] as char);
                i += 1;
                continue;
            };
            let key = src[open + 1..close].trim();
            let replacement = if pack {
                dense_map_key_spelling(key)
            } else if key.len() >= 1
                && key.as_bytes()[0].is_ascii_alphabetic()
                && key.as_bytes().iter().all(|c| is_ident_char(*c))
            {
                format!("\"{key}\"")
            } else {
                key.to_string()
            };
            out.push_str(name);
            out.push('[');
            out.push_str(&replacement);
            out.push(']');
            i = close + 1;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn expand_dense_map_indices(src: &str) -> String {
    rewrite_known_map_indices(src, false)
}

fn pack_dense_map_indices(src: &str) -> String {
    rewrite_known_map_indices(src, true)
}

/// Fuse a compact typed map construction produced by `to_dense` back into a
/// literal. This runs after whitespace minimization so the pattern is stable.
fn pack_dense_map_literals(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0;
    let mut scan = 0;
    while let Some(rel) = src[scan..].find(":=%;") {
        let bind = scan + rel;
        let stmt_start = src[..bind]
            .rfind(|c| matches!(c, '{' | ';' | '\n'))
            .map_or(0, |p| p + 1);
        let binding = src[stmt_start..bind].trim();
        let Some(space) = binding.find(' ') else {
            scan = bind + 4;
            continue;
        };
        let name = binding[..space].trim();
        let map_ty = binding[space + 1..].trim();
        if !map_ty.starts_with("M[") || !map_ty.ends_with(']') {
            scan = bind + 4;
            continue;
        }
        let mut at = bind + 4;
        let mut entries = Vec::new();
        loop {
            if !src[at..].starts_with(name) || src.as_bytes().get(at + name.len()) != Some(&b'[') {
                break;
            }
            let open = at + name.len();
            let Some(close) = find_matching_ascii(src.as_bytes(), open, b'[', b']') else {
                break;
            };
            if !src[close + 1..].starts_with("<-") {
                break;
            }
            let value_start = close + 3;
            let Some(value_len) = src[value_start..].find(';') else {
                break;
            };
            let key = src[open + 1..close].to_string();
            let value = src[value_start..value_start + value_len].to_string();
            entries.push((key, value));
            at = value_start + value_len + 1;
        }
        if entries.is_empty() {
            scan = bind + 4;
            continue;
        }
        let expected = format!(
            "M[{},{}]",
            dense_atom_type(&entries[0].0).unwrap_or("?"),
            dense_atom_type(&entries[0].1).unwrap_or("?")
        );
        if map_ty != expected
            || !entries.iter().all(|(k, v)| {
                dense_atom_type(k) == dense_atom_type(&entries[0].0)
                    && dense_atom_type(v) == dense_atom_type(&entries[0].1)
            })
        {
            scan = bind + 4;
            continue;
        }
        out.push_str(&src[cursor..stmt_start]);
        out.push_str(name);
        out.push_str("%{");
        out.push_str(
            &entries
                .iter()
                .map(|(k, v)| {
                    let key = dense_map_key_spelling(k);
                    let value = v.strip_suffix('I').unwrap_or(v);
                    format!("{key}:{value}")
                })
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push_str("};");
        cursor = at;
        scan = at;
    }
    out.push_str(&src[cursor..]);
    out
}

fn expand_dense_mapnew(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b[i] == b'[' {
            let prev = out
                .bytes()
                .rev()
                .find(|c| !c.is_ascii_whitespace())
                .unwrap_or(b'=');
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b']' && matches!(prev, b'=' | b',' | b'(' | b'{' | b';') {
                out.push_str("vec.new(test.alloc)");
                i = j + 1;
                continue;
            }
        }
        if b[i] == b'%' {
            let prev = out
                .bytes()
                .rev()
                .find(|c| !c.is_ascii_whitespace())
                .unwrap_or(b'=');
            let next = {
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < b.len() {
                    b[j]
                } else {
                    b';'
                }
            };
            // Remainder is `a % b`. Bare `%` after `=` / `:=` / `,` / `(` is map.new.
            if matches!(prev, b'=' | b',' | b'(' | b'{' | b';')
                && !is_ident_char(next)
                && !next.is_ascii_digit()
            {
                out.push_str("map.new(test.alloc)");
                i += 1;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `7L` / `7I` / `7Z` after a digit → typed literal.
fn expand_dense_lits(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 8);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            out.push_str(&src[start..i]);
            if i < b.len() {
                let suf = match b[i] {
                    b'L' => Some("i64"),
                    b'I' => Some("i32"),
                    b'Z' => Some("usz"),
                    b'W' => Some("u64"),
                    _ => None,
                };
                if let Some(s) = suf {
                    if i + 1 == b.len() || !is_ident_char(b[i + 1]) {
                        out.push_str(s);
                        i += 1;
                        continue;
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

/// `@cond{body}` → `while cond { body }`
fn expand_dense_while(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b[i] == b'@'
            && (i == 0 || !is_ident_char(b[i - 1]))
            && i + 1 < b.len()
            && b[i + 1] != b'{'
        {
            i += 1;
            let cond_s = i;
            let mut depth = 0i32;
            while i < b.len() {
                if b[i] == b'{' && depth == 0 {
                    break;
                }
                if b[i] == b'(' {
                    depth += 1;
                } else if b[i] == b')' {
                    depth -= 1;
                }
                i += 1;
            }
            let cond = src[cond_s..i].trim();
            let body = take_brace(b, &mut i);
            out.push_str("while ");
            out.push_str(cond);
            out.push_str(" { ");
            if let Some(t) = body {
                out.push_str(t.trim());
            }
            out.push_str(" }");
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `e|d` after `)` / ident → `match attempt e { Ok(v) => v; Err(_) => d }`.
/// The `attempt` is what removes the handled error from the outward effect row.
fn expand_dense_result_or(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 32);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b[i] == b'|'
            && i > 0
            && (is_ident_char(b[i - 1]) || b[i - 1] == b')')
            && i + 1 < b.len()
            && !matches!(b[i + 1], b'|' | b'=' | b' ' | b'\n' | b';' | b'}')
        {
            let def_s = i + 1;
            let mut j = def_s;
            if j < b.len() && b[j] == b'(' {
                let mut d = 0i32;
                while j < b.len() {
                    if b[j] == b'(' {
                        d += 1;
                    } else if b[j] == b')' {
                        d -= 1;
                        if d == 0 {
                            j += 1;
                            break;
                        }
                    }
                    j += 1;
                }
            } else {
                while j < b.len() && (is_ident_char(b[j]) || b[j].is_ascii_digit()) {
                    j += 1;
                }
            }
            let def = &src[def_s..j];
            let mut k = out.len();
            if out.ends_with(')') {
                let mut d = 0i32;
                while k > 0 {
                    k -= 1;
                    match out.as_bytes()[k] {
                        b')' => d += 1,
                        b'(' => {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                while k > 0
                    && (is_ident_char(out.as_bytes()[k - 1]) || out.as_bytes()[k - 1] == b'.')
                {
                    k -= 1;
                }
            } else {
                while k > 0
                    && (is_ident_char(out.as_bytes()[k - 1]) || out.as_bytes()[k - 1] == b'.')
                {
                    k -= 1;
                }
            }
            let scrut = out[k..].to_string();
            out.truncate(k);
            out.push_str("match attempt ");
            out.push_str(&scrut);
            out.push_str(" { Ok(v) => v; Err(_) => ");
            out.push_str(def);
            out.push_str(" }");
            i = j;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `$cond{then}{else}` → `if cond { then } else { else }`
/// `$cond{then}` → `if cond { then }`
fn expand_dense_if(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b[i] == b'$'
            && (i == 0 || !is_ident_char(b[i - 1]))
            && i + 1 < b.len()
            && b[i + 1] != b'{'
        {
            i += 1;
            let cond_s = i;
            let mut depth = 0i32;
            while i < b.len() {
                if b[i] == b'{' && depth == 0 {
                    break;
                }
                if b[i] == b'(' {
                    depth += 1;
                } else if b[i] == b')' {
                    depth -= 1;
                }
                i += 1;
            }
            let cond = src[cond_s..i].trim();
            let then_b = take_brace(b, &mut i);
            let mut else_b = None;
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'{' {
                i = j;
                else_b = take_brace(b, &mut i);
            }
            out.push_str("if ");
            out.push_str(cond);
            out.push_str(" { ");
            if let Some(t) = then_b {
                out.push_str(t.trim());
            }
            out.push_str(" }");
            if let Some(e) = else_b {
                out.push_str(" else { ");
                out.push_str(e.trim());
                out.push_str(" }");
            }
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn take_brace<'a>(b: &'a [u8], i: &mut usize) -> Option<&'a str> {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
    if *i >= b.len() || b[*i] != b'{' {
        return None;
    }
    let start = *i + 1;
    let mut depth = 0i32;
    while *i < b.len() {
        if b[*i] == b'{' {
            depth += 1;
        } else if b[*i] == b'}' {
            depth -= 1;
            if depth == 0 {
                let inner = std::str::from_utf8(&b[start..*i]).ok()?;
                *i += 1;
                return Some(inner);
            }
        }
        *i += 1;
    }
    None
}

/// Pack conventional/terse Ax into the language (short syntax). Inverse of [`rewrite_dense_to_terse`]
/// on the constructs it knows; anything else is left as terse.
pub fn to_dense(src: &str) -> String {
    let terse = to_terse(src);
    let mut s = pack_dense_fns(&terse);
    // Reductions while `let mut` / `for i in range` are still visible.
    s = pack_dense_reduce(&s);
    s = pack_dense_loops(&s);
    s = pack_dense_lets(&s);
    s = pack_dense_returns(&s);
    s = pack_dense_while(&s);
    s = pack_dense_if(&s);
    s = pack_dense_option_or(&s);
    s = pack_dense_result_or(&s);
    s = pack_dense_mapnew(&s);
    s = pack_dense_types(&s);
    s = pack_dense_i32_defaults(&s);
    s = pack_dense_shared_signatures(&s);
    s = pack_dense_effect_aliases(&s);
    s = pack_dense_lits(&s);
    s = pack_dense_assign(&s);
    s = pack_dense_inc(&s);
    s = pack_dense_index(&s);
    s = pack_dense_len(&s);
    s = pack_dense_vec_reduce(&s);
    s = pack_dense_put(&s);
    s = pack_dense_semis(&s);
    let s = minify_dense(&s);
    let s = pack_dense_interpolation_alias(&s);
    let s = pack_dense_map_literals(&s);
    let s = pack_dense_map_indices(&s);
    let s = pack_dense_default_map_alias(&s);
    let s = pack_dense_inferred_alloc_effects(&s);
    pack_dense_zero_arg_fns(&s)
}

/// Inverse of [`expand_dense_default_map_alias`], applied after map-literal
/// packing so the explicit type is still available to that recognizer.
fn pack_dense_default_map_alias(src: &str) -> String {
    src.replace("M[S,I]", "M")
}

/// Inverse of [`expand_dense_interpolation_alias`].
fn pack_dense_interpolation_alias(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if i + 1 < b.len() && b[i] == b'f' && b[i + 1] == b'"' {
            let start = i + 1;
            let mut j = start + 1;
            let mut braces = false;
            while j < b.len() {
                if b[j] == b'\\' {
                    j = (j + 2).min(b.len());
                    continue;
                }
                if b[j] == b'{' || b[j] == b'}' {
                    braces = true;
                }
                if b[j] == b'"' {
                    j += 1;
                    break;
                }
                j += 1;
            }
            if braces {
                out.push_str(&src[start..j]);
                i = j;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Inverse of [`expand_dense_zero_arg_fns`].
fn pack_dense_zero_arg_fns(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'#'
            && i + 3 < b.len()
            && (i == 0 || b[i - 1].is_ascii_whitespace() || matches!(b[i - 1], b';' | b'}'))
        {
            let mut end = i + 1;
            while end < b.len() && is_ident_char(b[end]) {
                end += 1;
            }
            if end + 2 < b.len() && &b[end..end + 3] == b"()=" {
                out.push_str(&src[i..end]);
                out.push('=');
                i = end + 3;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Omit `!a` when the dense body contains an inferred map literal.  The
/// conventional source has already declared allocation explicitly, so this is
/// a semantics-preserving inverse of `expand_dense_inferred_alloc_effects`.
fn pack_dense_inferred_alloc_effects(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if i + 3 <= b.len() && &b[i..i + 3] == b"!a=" {
            let body_start = i + 3;
            let has_map = if body_start < b.len() && b[body_start] == b'{' {
                find_matching_ascii(b, body_start, b'{', b'}')
                    .map(|close| src[body_start..=close].contains("%{"))
                    .unwrap_or(false)
            } else {
                false
            };
            if has_map {
                out.push('=');
                i += 3;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Collapse layout, then remove every space or newline that is not a lexical
/// separator. Real BPE vocabularies charge for both.
fn minify_dense(src: &str) -> String {
    let mut laid_out = String::with_capacity(src.len());
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if !laid_out.is_empty() {
            // Keep a line break before the next `#fn` so two decls stay separate.
            if t.starts_with('#') {
                laid_out.push('\n');
            } else {
                laid_out.push(' ');
            }
        }
        laid_out.push_str(t);
    }
    let compact = remove_optional_dense_space(&laid_out);
    compact
        .replace(";\n#", "\n#")
        .trim_end_matches(';')
        .to_string()
}

/// Strip whitespace that is not a lexical separator. BPE vocabularies charge
/// for many spaces/newlines even though the old in-repo proxy did not, so this
/// is part of the language pack rather than a cosmetic formatter pass.
fn remove_optional_dense_space(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        if matches!(chars[i], '"' | '`') {
            let quote = chars[i];
            out.push(quote);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    out.push(chars[i]);
                } else if chars[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if chars[i] == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() {
                out.push(chars[i]);
                let end = chars[i] == '\n';
                i += 1;
                if end {
                    break;
                }
            }
            continue;
        }
        if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    i += 1;
                    out.push('/');
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if chars[i].is_whitespace() {
            let start = i;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            let prev = out.chars().next_back();
            let next = chars.get(i).copied();
            let had_newline = chars[start..i].contains(&'\n');
            if had_newline && next == Some('#') {
                out.push('\n');
            } else if whitespace_is_separator(prev, next) {
                out.push(' ');
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn whitespace_is_separator(prev: Option<char>, next: Option<char>) -> bool {
    let (Some(a), Some(b)) = (prev, next) else {
        return false;
    };
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    if ident(a) && ident(b) {
        return true;
    }
    matches!(
        (a, b),
        ('/', '/')
            | ('/', '*')
            | ('*', '/')
            | ('+', '+')
            | ('-', '-')
            | ('-', '>')
            | ('=', '=')
            | ('=', '>')
            | ('!', '=')
            | ('<', '=')
            | ('>', '=')
            | ('&', '&')
            | ('|', '|')
            | (':', ':')
            | (':', '=')
            | ('<', '<')
            | ('>', '>')
            | ('<', '-')
            | ('?', '?')
            | ('.', '.')
    )
}

fn pack_dense_types(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if is_ident_char(b[i]) {
            let start = i;
            while i < b.len() && is_ident_char(b[i]) {
                i += 1;
            }
            let word = &src[start..i];
            if let Some((g, _)) = DENSE_TYPES.iter().find(|(_, full)| *full == word) {
                out.push_str(g);
            } else {
                out.push_str(word);
            }
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn pack_dense_fns(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.split_inclusive('\n') {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("fn ") {
            let pad = &line[..line.len() - t.len()];
            out.push_str(pad);
            out.push('#');
            out.push_str(rest);
        } else {
            out.push_str(line);
        }
    }
    out
}

fn pack_dense_lets(src: &str) -> String {
    // `let mut name: T =` → `name T:=` ; `let mut name =` → `name:=`
    // `let name: T =` → `name T:=` (dense treats := as the only binder)
    let mut out = String::new();
    let mut rest = src;
    while let Some(p) = rest.find("let ") {
        out.push_str(&rest[..p]);
        rest = &rest[p + 4..];
        let mut r = rest;
        if let Some(x) = r.strip_prefix("mut ") {
            r = x;
        }
        let name_end = r
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(r.len());
        if name_end == 0 {
            out.push_str("let ");
            continue;
        }
        let name = &r[..name_end];
        r = &r[name_end..];
        r = r.trim_start();
        let mut ty = None;
        if r.starts_with(':') {
            r = r[1..].trim_start();
            let bytes = r.as_bytes();
            let mut k = 0;
            let mut depth = 0i32;
            while k < bytes.len() {
                match bytes[k] {
                    b'[' | b'(' => depth += 1,
                    b']' | b')' => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                    }
                    b'=' if depth == 0 => break,
                    c if depth == 0
                        && !c.is_ascii_alphanumeric()
                        && c != b'_'
                        && c != b'['
                        && c != b']'
                        && c != b','
                        && c != b' ' =>
                    {
                        break;
                    }
                    _ => {}
                }
                k += 1;
            }
            ty = Some(r[..k].trim());
            r = r[k..].trim_start();
        }
        if r.starts_with('=') {
            out.push_str(name);
            if let Some(t) = ty {
                out.push(' ');
                out.push_str(t);
            }
            out.push_str(":=");
            rest = &r[1..];
        } else {
            out.push_str("let ");
        }
    }
    out.push_str(rest);
    out
}

fn pack_dense_loops(src: &str) -> String {
    // `for i in range(0, n)` → `i~n` ; `for i in range(a, b)` → `i~a..b`
    let mut out = String::new();
    let mut rest = src;
    while let Some(p) = rest.find("for ") {
        out.push_str(&rest[..p]);
        let after = &rest[p + 4..];
        let name_end = after
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(after.len());
        if name_end == 0 {
            out.push_str("for ");
            rest = after;
            continue;
        }
        let name = &after[..name_end];
        let mut r = after[name_end..].trim_start();
        if let Some(x) = r.strip_prefix("in") {
            r = x.trim_start();
        } else {
            out.push_str("for ");
            rest = after;
            continue;
        }
        if let Some(x) = r.strip_prefix("range(") {
            let mut depth = 1i32;
            let mut close = None;
            for (k, c) in x.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(k);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(close) = close {
                let args = &x[..close];
                let parts: Vec<&str> = args.split(',').map(str::trim).collect();
                // Atoms and `xs.len()` (packed later to `xs#`).
                let bound = |s: &str| {
                    !s.is_empty()
                        && (s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                            || (s.ends_with(".len()")
                                && s[..s.len() - 6]
                                    .chars()
                                    .all(|c| c.is_ascii_alphanumeric() || c == '_')))
                };
                if parts.len() == 2 && bound(parts[0]) && bound(parts[1]) {
                    out.push_str(name);
                    out.push('~');
                    if parts[0] == "0" {
                        out.push_str(parts[1]);
                    } else {
                        out.push_str(parts[0]);
                        out.push_str("..");
                        out.push_str(parts[1]);
                    }
                    rest = &x[close + 1..];
                    continue;
                }
            }
        }
        out.push_str("for ");
        rest = after;
    }
    out.push_str(rest);
    out
}

fn pack_dense_returns(src: &str) -> String {
    src.replace("return ", "^")
}

/// `if c { t } else { e }` → `$c{t}{e}` ; `if c { t }` → `$c{t}`
fn pack_dense_if(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if is_ident_at(b, i, "if") {
            let after = i + 2;
            if after < b.len() && b[after].is_ascii_whitespace() {
                i = after;
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                let cond_s = i;
                let mut depth = 0i32;
                while i < b.len() {
                    if b[i] == b'{' && depth == 0 {
                        break;
                    }
                    if b[i] == b'(' {
                        depth += 1;
                    } else if b[i] == b')' {
                        depth -= 1;
                    }
                    i += 1;
                }
                let cond = src[cond_s..i].trim();
                let then_b = take_brace(b, &mut i);
                let mut else_b = None;
                let mut j = i;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if is_ident_at(b, j, "else") {
                    j += 4;
                    while j < b.len() && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    i = j;
                    else_b = take_brace(b, &mut i);
                }
                if let Some(e) = else_b {
                    // `??` is one BPE token in both target vocabularies and a
                    // simple branch needs no braces: `b??1:0` versus `$b{1}{0}`.
                    out.push_str(cond);
                    out.push_str("??");
                    out.push_str(&pack_dense_if_branch(then_b.unwrap_or("")));
                    out.push(':');
                    out.push_str(&pack_dense_if_branch(e));
                } else {
                    // A one-armed conditional still uses the established form.
                    out.push('$');
                    out.push_str(cond);
                    out.push('{');
                    if let Some(t) = then_b {
                        out.push_str(t.trim());
                    }
                    out.push('}');
                }
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn pack_dense_if_branch(branch: &str) -> String {
    let packed = pack_dense_if(branch);
    let t = packed.trim();
    let mut braces = 0i32;
    let mut brackets = 0i32;
    let mut parens = 0i32;
    let mut quote = None;
    let mut has_top_level_semi = false;
    for c in t.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '`' => quote = Some(c),
            '{' => braces += 1,
            '}' => braces -= 1,
            '[' => brackets += 1,
            ']' => brackets -= 1,
            '(' => parens += 1,
            ')' => parens -= 1,
            ';' if braces == 0 && brackets == 0 && parens == 0 => {
                has_top_level_semi = true;
            }
            _ => {}
        }
    }
    if !t.is_empty() && !has_top_level_semi {
        t.to_string()
    } else {
        format!("{{{t}}}")
    }
}

/// `match e { Some(v) => v; None => d }` → `e?d`
fn pack_dense_option_or(src: &str) -> String {
    let mut out = String::new();
    let mut rest = src;
    while let Some(p) = rest.find("match ") {
        out.push_str(&rest[..p]);
        let after = &rest[p + 6..];
        if let Some(open) = after.find('{') {
            let scrut = after[..open].trim();
            let mut i = open;
            let bytes = after.as_bytes();
            if let Some(inner) = take_brace(bytes, &mut i) {
                let t = inner.split_whitespace().collect::<Vec<_>>().join(" ");
                if let Some(d) = t
                    .strip_prefix("Some(v) => v; None => ")
                    .or_else(|| t.strip_prefix("Some(v) => v ; None => "))
                {
                    let d = d.trim().trim_end_matches(';');
                    if !scrut.is_empty() && !d.is_empty() && !d.contains("match ") {
                        out.push_str(scrut.strip_prefix("attempt ").unwrap_or(scrut));
                        let bare_string_get = d == "0" && scrut.contains(".get(\"");
                        if !bare_string_get {
                            out.push('?');
                        }
                        if d != "0" {
                            out.push_str(d);
                        }
                        rest = &after[i..];
                        continue;
                    }
                }
            }
        }
        out.push_str("match ");
        rest = after;
    }
    out.push_str(rest);
    out
}

fn expand_dense_option_or_real(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 32);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b[i] == b'?'
            && i > 0
            && b.get(i + 1) != Some(&b'?')
            && (is_ident_char(b[i - 1]) || b[i - 1] == b')')
            && i + 1 < b.len()
            && !matches!(b[i + 1], b';' | b'}' | b')' | b',' | b' ' | b'\n')
        {
            // Rewrite postfix `SCRUT?DEF` as `match SCRUT { Some(v) => v; None => DEF }`
            // by walking back over the just-written atom.
            let def_s = i + 1;
            let mut j = def_s;
            if j < b.len() && b[j] == b'(' {
                let mut d = 0i32;
                while j < b.len() {
                    if b[j] == b'(' {
                        d += 1;
                    } else if b[j] == b')' {
                        d -= 1;
                        if d == 0 {
                            j += 1;
                            break;
                        }
                    }
                    j += 1;
                }
            } else {
                while j < b.len() && (is_ident_char(b[j]) || b[j].is_ascii_digit() || b[j] == b'"')
                {
                    if b[j] == b'"' {
                        j += 1;
                        while j < b.len() && b[j] != b'"' {
                            j += 1;
                        }
                        j += 1;
                        break;
                    }
                    j += 1;
                }
            }
            let def = &src[def_s..j];
            // Pull last atom off `out`.
            let mut k = out.len();
            if out.ends_with(')') {
                let mut d = 0i32;
                while k > 0 {
                    k -= 1;
                    match out.as_bytes()[k] {
                        b')' => d += 1,
                        b'(' => {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                // include callee ident before `(`
                while k > 0
                    && (is_ident_char(out.as_bytes()[k - 1]) || out.as_bytes()[k - 1] == b'.')
                {
                    k -= 1;
                }
            } else {
                while k > 0
                    && (is_ident_char(out.as_bytes()[k - 1]) || out.as_bytes()[k - 1] == b'.')
                {
                    k -= 1;
                }
            }
            let scrut = out[k..].to_string();
            out.truncate(k);
            out.push_str("match ");
            out.push_str(&scrut);
            out.push_str(" { Some(v) => v; None => ");
            out.push_str(def);
            out.push_str(" }");
            i = j;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Drop `;` immediately before `}` — the parser treats a block tail / for-if
/// closer as a terminator already.
fn pack_dense_semis(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b';' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'}' {
                i += 1;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `while c { body }` → `@c{body}`
fn pack_dense_while(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if is_ident_at(b, i, "while") {
            let after = i + 5;
            if after < b.len() && b[after].is_ascii_whitespace() {
                i = after;
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                let cond_s = i;
                let mut depth = 0i32;
                while i < b.len() {
                    if b[i] == b'{' && depth == 0 {
                        break;
                    }
                    if b[i] == b'(' {
                        depth += 1;
                    } else if b[i] == b')' {
                        depth -= 1;
                    }
                    i += 1;
                }
                let cond = src[cond_s..i].trim();
                let body = take_brace(b, &mut i);
                out.push('@');
                out.push_str(cond);
                out.push('{');
                if let Some(t) = body {
                    out.push_str(t.trim());
                }
                out.push('}');
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `match e { Ok(v) => v; Err(_) => d }` → `e|d`
fn pack_dense_result_or(src: &str) -> String {
    let mut out = String::new();
    let mut rest = src;
    while let Some(p) = rest.find("match ") {
        out.push_str(&rest[..p]);
        let after = &rest[p + 6..];
        if let Some(open) = after.find('{') {
            let scrut = after[..open].trim();
            let mut i = open;
            let bytes = after.as_bytes();
            if let Some(inner) = take_brace(bytes, &mut i) {
                let t = inner.split_whitespace().collect::<Vec<_>>().join(" ");
                if let Some(d) = t
                    .strip_prefix("Ok(v) => v; Err(_) => ")
                    .or_else(|| t.strip_prefix("Ok(v) => v ; Err(_) => "))
                {
                    let d = d.trim().trim_end_matches(';');
                    if !scrut.is_empty() && !d.is_empty() && !d.contains("match ") {
                        out.push_str(scrut.strip_prefix("attempt ").unwrap_or(scrut));
                        out.push('|');
                        out.push_str(d);
                        rest = &after[i..];
                        continue;
                    }
                }
            }
        }
        out.push_str("match ");
        rest = after;
    }
    out.push_str(rest);
    out
}

fn pack_dense_mapnew(src: &str) -> String {
    src.replace("map.new(test.alloc)", "%")
        .replace("vec.new(test.alloc)", "[]")
}

fn pack_dense_lits(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            out.push_str(&src[start..i]);
            if i + 3 <= b.len() && &src[i..i + 3] == "i64" {
                out.push('L');
                i += 3;
                continue;
            }
            if i + 3 <= b.len() && &src[i..i + 3] == "i32" {
                out.push('I');
                i += 3;
                continue;
            }
            if i + 3 <= b.len() && &src[i..i + 3] == "usz" {
                out.push('Z');
                i += 3;
                continue;
            }
            if i + 3 <= b.len() && &src[i..i + 3] == "u64" {
                out.push('W');
                i += 3;
                continue;
            }
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `name = name + e` → `name += e` when both sides are the same atom.
fn pack_dense_assign(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if is_ident_char(b[i]) && (i == 0 || !is_ident_char(b[i - 1])) {
            let start = i;
            while i < b.len() && (is_ident_char(b[i]) || b[i] == b'.') {
                i += 1;
            }
            let lhs = &src[start..i];
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'=' && (j + 1 == b.len() || b[j + 1] != b'=') {
                j += 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j + lhs.len() < b.len() && &src[j..j + lhs.len()] == lhs {
                    let after = j + lhs.len();
                    if after == b.len() || !is_ident_char(b[after]) {
                        let mut k = after;
                        while k < b.len() && b[k].is_ascii_whitespace() {
                            k += 1;
                        }
                        if k < b.len()
                            && matches!(b[k], b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^')
                        {
                            let op = b[k] as char;
                            let next = k + 1;
                            if next == b.len() || b[next] != b'=' {
                                out.push_str(lhs);
                                out.push(' ');
                                out.push(op);
                                out.push('=');
                                i = next;
                                continue;
                            }
                        }
                    }
                }
            }
            out.push_str(lhs);
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `{ let mut s[: T] = 0; for i in range(lo, hi) { s = s + i; }; s }` → `+/hi` or `+/lo..hi`.
/// Same with `*` / init `1` → `*/`. Anything else in the block is left as a loop.
fn pack_dense_reduce(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if let Some((consumed, packed)) = try_pack_reduce_at(src, i) {
            out.push_str(&packed);
            i += consumed;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn try_pack_reduce_at(src: &str, start: usize) -> Option<(usize, String)> {
    let rest = &src[start..];
    if !rest.starts_with('{') {
        return None;
    }
    let inner = rest[1..].trim_start();
    let after_let = inner.strip_prefix("let mut ")?;
    let acc_end = after_let
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(after_let.len());
    if acc_end == 0 {
        return None;
    }
    let acc = &after_let[..acc_end];
    let mut r = after_let[acc_end..].trim_start();
    let mut acc_ty: Option<&str> = None;
    if let Some(x) = r.strip_prefix(':') {
        r = x.trim_start();
        let ty_end = r.find('=').unwrap_or(r.len());
        acc_ty = Some(r[..ty_end].trim());
        r = r[ty_end..].trim_start();
    }
    r = r.strip_prefix('=')?.trim_start();
    let init_end = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
    if init_end == 0 {
        return None;
    }
    let init = &r[..init_end];
    r = r[init_end..].trim_start().strip_prefix(';')?.trim_start();
    r = r.strip_prefix("for ")?;
    let ix_end = r
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(r.len());
    if ix_end == 0 {
        return None;
    }
    let ix = &r[..ix_end];
    r = r[ix_end..].trim_start().strip_prefix("in")?.trim_start();
    r = r.strip_prefix("range(")?;
    let close = r.find(')')?;
    let args = &r[..close];
    let parts: Vec<&str> = args.split(',').map(str::trim).collect();
    let atom = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let vec_len = |s: &str| {
        s.ends_with(".len()")
            && s.len() > 6
            && s[..s.len() - 6]
                .chars()
                .all(|c| c.is_ascii_alphabetic() || c == '_')
    };
    if parts.len() != 2 || parts[0] != "0" || !(atom(parts[1]) || vec_len(parts[1])) {
        return None;
    }
    let vec_name = parts[1].strip_suffix(".len()");
    r = r[close + 1..].trim_start().strip_prefix('{')?.trim_start();
    let want_add = format!("{acc} = {acc} + {ix}");
    let want_mul = format!("{acc} = {acc} * {ix}");
    let want_vat = vec_name.map(|v| format!("{acc} = {acc} + {v}.at({ix})"));
    let want_vmu = vec_name.map(|v| format!("{acc} = {acc} * {v}.at({ix})"));
    let (op, consumed_body) = if r.starts_with(&want_add) && init == "0" && vec_name.is_none() {
        ('+', want_add.len())
    } else if r.starts_with(&want_mul) && init == "1" && vec_name.is_none() {
        ('*', want_mul.len())
    } else if let (Some(a), Some(_)) = (&want_vat, vec_name) {
        if r.starts_with(a) && init == "0" {
            ('+', a.len())
        } else if let Some(m) = &want_vmu {
            if r.starts_with(m) && init == "1" {
                ('*', m.len())
            } else {
                return None;
            }
        } else {
            return None;
        }
    } else {
        return None;
    };
    r = r[consumed_body..].trim_start();
    r = r.strip_prefix(';')?.trim_start();
    r = r.strip_prefix('}')?.trim_start();
    r = r.strip_prefix(';')?.trim_start();
    if !r.starts_with(acc) {
        return None;
    }
    r = r[acc.len()..].trim_start();
    r = r.strip_prefix('}')?;
    let consumed = src.len() - start - r.len();
    let mut packed = String::new();
    packed.push(op);
    packed.push('/');
    if let Some(v) = vec_name {
        packed.push_str(v);
        packed.push('#');
    } else if parts[0] == "0" {
        packed.push_str(parts[1]);
    } else {
        packed.push_str(parts[0]);
        packed.push_str("..");
        packed.push_str(parts[1]);
    }
    // A literal bound is i32 by default; keep a non-i32 accumulator's type
    // as a suffix (`+/10usz`) so expand types it the same way.
    if let Some(ty) = acc_ty {
        if ty != "i32" && parts[1].chars().all(|c| c.is_ascii_digit()) {
            packed.push_str(ty);
        }
    }
    Some((consumed, packed))
}

/// `name = name + 1` / `name += 1` → `name++` (same for `-` / `--`).
fn pack_dense_inc(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() {
                out.push(b[i] as char);
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if is_ident_char(b[i]) && (i == 0 || !is_ident_char(b[i - 1])) {
            let start = i;
            while i < b.len() && (is_ident_char(b[i]) || b[i] == b'.') {
                i += 1;
            }
            let lhs = &src[start..i];
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j + 1 < b.len() && matches!(&src[j..j + 2], "+=" | "-=") {
                let op = src.as_bytes()[j];
                j += 2;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < b.len() && b[j] == b'1' && (j + 1 == b.len() || !is_ident_char(b[j + 1])) {
                    out.push_str(lhs);
                    out.push(op as char);
                    out.push(op as char);
                    i = j + 1;
                    continue;
                }
            }
            out.push_str(lhs);
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `recv.at(idx)` → `recv[idx]` when `idx` is an atom.
fn pack_dense_index(src: &str) -> String {
    let mut out = String::new();
    let mut rest = src;
    while let Some(p) = rest.find(".at(") {
        out.push_str(&rest[..p]);
        let after = &rest[p + 4..];
        if let Some(close) = after.find(')') {
            let idx = after[..close].trim();
            if !idx.is_empty()
                && idx
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
                && !idx.contains("..")
            {
                out.push('[');
                out.push_str(idx);
                out.push(']');
                rest = &after[close + 1..];
                continue;
            }
        }
        out.push_str(".at(");
        rest = after;
    }
    out.push_str(rest);
    out
}

/// `xs.len()` → `xs#`.
fn pack_dense_len(src: &str) -> String {
    src.replace(".len()", "#")
}

/// `{ s T:= 0; i~xs# { s += xs[i] }; s }` → `+/xs#` (same for `*=` / `*/`).
fn pack_dense_vec_reduce(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if let Some((n, packed)) = try_pack_vec_reduce_at(src, i) {
            out.push_str(&packed);
            i += n;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn try_pack_vec_reduce_at(src: &str, start: usize) -> Option<(usize, String)> {
    let rest = &src[start..];
    // Sequential form after the other packers:
    //   `s Z:= 0; i~xs# { s += xs[i] }; s`
    // Optional wrapping `{ … }` from a standalone expression.
    let mut r = rest;
    let wrapped = r.starts_with('{');
    if wrapped {
        r = r[1..].trim_start();
    }
    if start > 0 && is_ident_char(src.as_bytes()[start - 1]) {
        return None;
    }
    let acc_end = r.find(|c: char| !c.is_ascii_alphanumeric() && c != '_')?;
    if acc_end == 0 {
        return None;
    }
    let acc = &r[..acc_end];
    r = r[acc_end..].trim_start();
    // `s Z:= 0` or `s: usz := 0` or `s:= 0`.
    if let Some(x) = r.strip_prefix(":=") {
        r = x.trim_start();
    } else if r.starts_with(':') {
        r = r[1..].trim_start();
        let ty_end = r.find(":=").unwrap_or(r.len());
        r = r[ty_end..].trim_start().strip_prefix(":=")?.trim_start();
    } else if let Some(ty_end) = r.find(":=") {
        let ty = r[..ty_end].trim();
        if ty.is_empty() || !ty.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        r = r[ty_end..].trim_start().strip_prefix(":=")?.trim_start();
    } else {
        return None;
    }
    let init_end = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
    if init_end == 0 {
        return None;
    }
    let init = &r[..init_end];
    r = r[init_end..].trim_start();
    // Optional type suffix left on the literal (`0usz`) or a leftover glyph.
    if r.starts_with("usz") {
        r = r[3..].trim_start();
    } else if r.starts_with('Z') && (r.len() == 1 || !is_ident_char(r.as_bytes()[1])) {
        r = r[1..].trim_start();
    }
    r = r.strip_prefix(';')?.trim_start();
    let ix_end = r.find('~')?;
    let ix = r[..ix_end].trim();
    if ix.is_empty() || !ix.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    r = r[ix_end + 1..].trim_start();
    let hash = r.find('#')?;
    let vec = r[..hash].trim();
    if vec.is_empty() || !vec.chars().all(|c| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    r = r[hash + 1..].trim_start().strip_prefix('{')?.trim_start();
    let add = format!("{acc} += {vec}[{ix}]");
    let mul = format!("{acc} *= {vec}[{ix}]");
    let op = if r.starts_with(&add) && init == "0" {
        r = r[add.len()..].trim_start();
        '+'
    } else if r.starts_with(&mul) && init == "1" {
        r = r[mul.len()..].trim_start();
        '*'
    } else {
        return None;
    };
    r = r.strip_prefix(';')?.trim_start();
    r = r.strip_prefix('}')?.trim_start();
    r = r.strip_prefix(';')?.trim_start();
    if !r.starts_with(acc) {
        return None;
    }
    let after = &r[acc.len()..];
    // Bare result, not `s++` / `s +=` / `s[i]`.
    if let Some(&c) = after.as_bytes().first() {
        if is_ident_char(c) || matches!(c, b'+' | b'-' | b'=' | b'[' | b'.' | b'(') {
            return None;
        }
    }
    r = after.trim_start();
    if wrapped {
        r = r.strip_prefix('}')?;
    }
    let consumed = src.len() - start - r.len();
    Some((consumed, format!("{op}/{vec}#")))
}

/// `recv.insert(k, v)` → `recv[k]<-v`; `recv.push(e)` → `recv<-e`;
/// `recv.get(k)` → `recv[k]` (get-or via `?` stays attached).
fn pack_dense_put(src: &str) -> String {
    let mut s = pack_method_to_put(src, ".insert(", true);
    s = pack_method_to_put(&s, ".set(", true);
    s = pack_method_to_put(&s, ".push(", false);
    pack_method_to_put(&s, ".get(", false)
}

fn key_looks_like_map(key: &str) -> bool {
    let t = key.trim();
    t.starts_with('"') || t.starts_with('\'')
}

fn pack_method_to_put(src: &str, meth: &str, two_args: bool) -> String {
    let mut out = String::new();
    let mut rest = src;
    while let Some(p) = rest.find(meth) {
        let before = &rest[..p];
        let mut k = before.len();
        while k > 0 && (is_ident_char(before.as_bytes()[k - 1]) || before.as_bytes()[k - 1] == b'.')
        {
            k -= 1;
        }
        let recv = before[k..].trim();
        out.push_str(&before[..k]);
        let after = &rest[p + meth.len()..];
        let mut d = 1i32;
        let mut close = None;
        for (i, c) in after.char_indices() {
            match c {
                '(' => d += 1,
                ')' => {
                    d -= 1;
                    if d == 0 {
                        close = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(close) = close {
            let args = after[..close].trim();
            if two_args {
                if let Some(comma) = split_top_comma(args) {
                    let key = args[..comma].trim();
                    let val = args[comma + 1..].trim();
                    if atomish(key) && !val.is_empty() {
                        out.push_str(recv);
                        out.push('[');
                        out.push_str(key);
                        out.push_str("]<-");
                        out.push_str(val);
                        rest = &after[close + 1..];
                        continue;
                    }
                }
            } else if atomish(args) && !args.is_empty() {
                out.push_str(recv);
                if meth == ".get(" {
                    out.push('[');
                    out.push_str(args);
                    out.push(']');
                } else {
                    out.push_str("<-");
                    out.push_str(args);
                }
                rest = &after[close + 1..];
                continue;
            }
        }
        out.push_str(recv);
        out.push_str(meth);
        rest = after;
    }
    out.push_str(rest);
    out
}

fn split_top_comma(s: &str) -> Option<usize> {
    let mut d = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' => d += 1,
            ')' | ']' => d -= 1,
            ',' if d == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

fn atomish(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '"' | '\'' | '-'))
}
