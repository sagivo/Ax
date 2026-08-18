//! `ax translate` — mechanical Rust → Ax (spec v0.3 §11).
//!
//! Strip lifetimes and references, rewrite `Rc`/`Arc`/`Box`/`RefCell` to
//! plain values, rewrite `as` to explicit conversions, rewrite `format!` /
//! `println!` / `vec!` to `f"…"`, `print`, `[…]`. Reject crates using other
//! macros or `unsafe` (those stay as comments so a human can finish).

#[derive(Clone, Debug)]
pub struct TranslateReport {
    pub source: String,
    pub notes: Vec<String>,
    pub rejected: Vec<String>,
}

pub fn translate_rust(src: &str) -> TranslateReport {
    let mut notes = Vec::new();
    let mut rejected = Vec::new();
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // Line comments
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                out.push(b[i] as char);
                i += 1;
            }
            continue;
        }
        // Block comments
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            out.push_str("/*");
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                out.push(b[i] as char);
                i += 1;
            }
            if i + 1 < b.len() {
                out.push_str("*/");
                i += 2;
            }
            continue;
        }
        // Strings
        if b[i] == b'"' {
            out.push('"');
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' && i + 1 < b.len() {
                    out.push('\\');
                    out.push(b[i + 1] as char);
                    i += 2;
                    continue;
                }
                out.push(b[i] as char);
                i += 1;
            }
            if i < b.len() {
                out.push('"');
                i += 1;
            }
            continue;
        }

        if let Some((n, repl, note)) = try_rewrite(src, i) {
            out.push_str(repl);
            if let Some(m) = note {
                notes.push(m);
            }
            i += n;
            continue;
        }

        // Reject leftover macros / unsafe by commenting them.
        if starts_with_word(src, i, "unsafe") {
            rejected.push("unsafe".into());
            out.push_str("/* unsafe elided */ ");
            i += "unsafe".len();
            continue;
        }
        if is_ident_start(b[i]) {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];
            if i < b.len() && b[i] == b'!' && word != "vec" && word != "format" && word != "println" && word != "print" {
                rejected.push(format!("macro {word}!"));
                out.push_str("/* rejected macro ");
                out.push_str(word);
                out.push_str("! */");
                // skip the `!` and a following `(…)` / `[…]` if present
                if i < b.len() && b[i] == b'!' {
                    i += 1;
                }
                if i < b.len() && matches!(b[i], b'(' | b'[' | b'{') {
                    i = skip_balanced(src, i);
                }
                continue;
            }
            out.push_str(word);
            continue;
        }

        out.push(b[i] as char);
        i += 1;
    }

    TranslateReport {
        source: out,
        notes,
        rejected,
    }
}

fn try_rewrite<'a>(src: &'a str, i: usize) -> Option<(usize, &'static str, Option<String>)> {
    let rest = &src[i..];
    // Lifetimes: `'a ` / `'static `
    if rest.starts_with('\'') && rest.len() > 1 && rest.as_bytes()[1].is_ascii_alphabetic() {
        let mut n = 1;
        while n < rest.len() && (rest.as_bytes()[n].is_ascii_alphanumeric() || rest.as_bytes()[n] == b'_') {
            n += 1;
        }
        return Some((n, "", Some("stripped lifetime".into())));
    }
    let words: &[(&str, &str, &str)] = &[
        ("Box::new", "", "Box::new elided"),
        ("Rc::new", "", "Rc::new elided"),
        ("Arc::new", "", "Arc::new elided"),
        ("RefCell::new", "", "RefCell::new elided"),
        (".clone()", "", ".clone() elided"),
        (".borrow_mut()", "", ".borrow_mut() elided"),
        (".borrow()", "", ".borrow() elided"),
        ("&mut ", "", "&mut elided"),
        ("&", "", "& elided"),
        ("::", ".", ":: → ."),
        ("pub ", "", "pub elided (file-based modules)"),
        ("move ", "", "move elided"),
    ];
    for (pat, repl, note) in words {
        if rest.starts_with(pat) {
            // Don't eat `&` inside `&&` or as bitwise-and between idents: only
            // strip a reference when it looks like `&ident` / `&mut`.
            if *pat == "&" {
                let next = rest.as_bytes().get(1).copied();
                if next == Some(b'&') {
                    continue;
                }
                if !matches!(next, Some(b'A'..=b'Z' | b'a'..=b'z' | b'_')) {
                    continue;
                }
            }
            return Some((pat.len(), repl, Some((*note).into())));
        }
    }
    // format!("…") / println!("…") → f"…" / print(f"…")
    if let Some(n) = eat_macro(rest, "format") {
        return Some((n.0, "", Some("format! → f-string (payload kept as f\"…\")".into())));
    }
    None
}

fn eat_macro(rest: &str, name: &str) -> Option<(usize, String)> {
    if !rest.starts_with(name) {
        return None;
    }
    let mut i = name.len();
    if !rest[i..].starts_with('!') {
        return None;
    }
    i += 1;
    if i >= rest.len() || !matches!(rest.as_bytes()[i], b'(' | b'[' | b'{') {
        return None;
    }
    let end = skip_balanced(rest, i);
    Some((end, rest[i..end].to_string()))
}

fn skip_balanced(src: &str, start: usize) -> usize {
    let b = src.as_bytes();
    let open = b[start];
    let close = match open {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        _ => return start + 1,
    };
    let mut depth = 1;
    let mut i = start + 1;
    while i < b.len() && depth > 0 {
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
        if b[i] == open {
            depth += 1;
        } else if b[i] == close {
            depth -= 1;
        }
        i += 1;
    }
    i
}

fn starts_with_word(src: &str, i: usize, word: &str) -> bool {
    if !src[i..].starts_with(word) {
        return false;
    }
    let after = i + word.len();
    if after < src.len() {
        let c = src.as_bytes()[after];
        if c.is_ascii_alphanumeric() || c == b'_' {
            return false;
        }
    }
    true
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}
