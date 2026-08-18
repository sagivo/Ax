//! Surfaces lower to one AST.
//!
//! Ax **is** the short syntax (`#fn`, `:=`, `$if`, type glyphs). There is no
//! opt-in dense mode. A file that opens with `(` is still the prefix tree.
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
        .find(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with("#[") && !l.starts_with("/*"))
        .unwrap_or("");
    // `#name(` is a short-syntax fn. `#[attr]` is conventional meta.
    (t.starts_with('#') && t.len() > 1 && t.as_bytes()[1].is_ascii_alphabetic())
        || contains_outside_comments(src, ":=")
        || contains_outside_comments(src, "$")
        || dense_at_sugar(src)
        || dense_range_sugar(src)
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
pub fn rewrite_dense_to_terse(src: &str) -> String {
    let mut s = expand_dense_mapnew(src);
    s = expand_dense_lits(&s);
    s = expand_dense_binds(&s);
    s = expand_dense_ranges(&s);
    s = expand_dense_returns(&s);
    s = expand_dense_while(&s);
    s = expand_dense_if(&s);
    s = expand_dense_result_or(&s);
    s = expand_dense_option_or_real(&s);
    s = expand_dense_fns(&s);
    expand_dense_types(&s)
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
            && !t.ends_with('}')
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
                while j < b.len() && (is_ident_char(b[j]) || b[j].is_ascii_digit()) {
                    j += 1;
                }
                if j > lo_s {
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
                        while k < b.len() && (is_ident_char(b[k]) || b[k].is_ascii_digit()) {
                            k += 1;
                        }
                        if k > hi_s {
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

/// `e|d` after `)` / ident → `match e { Ok(v) => v; Err(_) => d }`
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
            out.push_str("match ");
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
    s = pack_dense_loops(&s);
    s = pack_dense_lets(&s);
    s = pack_dense_returns(&s);
    s = pack_dense_while(&s);
    s = pack_dense_if(&s);
    s = pack_dense_option_or(&s);
    s = pack_dense_result_or(&s);
    s = pack_dense_mapnew(&s);
    s = pack_dense_types(&s);
    s = pack_dense_lits(&s);
    s = pack_dense_semis(&s);
    minify_dense(&s)
}

/// Spaces are free in the proxy tokenizer; newlines are not. Collapse
/// layout so a dense file is one token cheaper per dropped line.
fn minify_dense(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for (i, line) in src.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if !out.is_empty() {
            // Keep a line break before the next `#fn` so two decls stay separate.
            if t.starts_with('#') {
                out.push('\n');
            } else {
                out.push(' ');
            }
        }
        let _ = i;
        out.push_str(t);
    }
    out
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
                // Only pack atom bounds (`n`, `0`) — `xs.len()` stays as range().
                let atom = |s: &str| {
                    !s.is_empty()
                        && s.chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_')
                };
                if parts.len() == 2 && atom(parts[0]) && atom(parts[1]) {
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
                out.push('$');
                out.push_str(cond);
                out.push('{');
                if let Some(t) = then_b {
                    out.push_str(t.trim());
                }
                out.push('}');
                if let Some(e) = else_b {
                    out.push('{');
                    out.push_str(e.trim());
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
                        out.push_str(scrut);
                        out.push('?');
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
                while j < b.len() && (is_ident_char(b[j]) || b[j].is_ascii_digit() || b[j] == b'"') {
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
                while k > 0 && (is_ident_char(out.as_bytes()[k - 1]) || out.as_bytes()[k - 1] == b'.')
                {
                    k -= 1;
                }
            } else {
                while k > 0 && (is_ident_char(out.as_bytes()[k - 1]) || out.as_bytes()[k - 1] == b'.')
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
                        out.push_str(scrut);
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
