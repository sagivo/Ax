//! M3 semantic workspace: transactional AST edits, exact invalidation,
//! and semantic merge reporting. Text remains authoritative.

use crate::ast::{DeclKind, File};
use crate::fmt;
use crate::hash;
use crate::intern::Interner;
use crate::parser::Parser;
use crate::span::FileId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatchTx {
    pub base_module_hash: String,
    pub def_id: String,
    pub path: Vec<serde_json::Value>,
    pub expected_subtree_hash: String,
    /// Replacement as Ax source for the targeted subtree (expression or full fn body).
    pub replacement_src: Option<String>,
    pub replacement_ast: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatchResult {
    pub ok: bool,
    pub applied: bool,
    pub reason: Option<String>,
    pub new_module_hash: Option<String>,
    pub source: Option<String>,
}

pub fn apply_patch(
    intern: &mut Interner,
    src: &str,
    file: &File,
    tx: &PatchTx,
) -> PatchResult {
    let base = hash::sha256_hex(src.as_bytes());
    if !tx.base_module_hash.is_empty()
        && tx.base_module_hash != "…"
        && tx.base_module_hash != base
    {
        return PatchResult {
            ok: false,
            applied: false,
            reason: Some("base_module_hash mismatch".into()),
            new_module_hash: None,
            source: None,
        };
    }

    let Some(decl) = find_decl(intern, file, &tx.def_id) else {
        return PatchResult {
            ok: false,
            applied: false,
            reason: Some(format!("unknown def_id {}", tx.def_id)),
            new_module_hash: None,
            source: None,
        };
    };

    let subtree = match &decl.kind {
        DeclKind::Fn(f) | DeclKind::ContractFn(f) => format!("{:?}", f.body.kind),
        DeclKind::Test(t) => format!("{:?}", t.body.kind),
        DeclKind::Type(t) => format!("{:?}", t.name.name),
        DeclKind::Dict(d) => format!("{:?}", d.name.name),
    };
    let got = hash::body_hash(&subtree);
    if !tx.expected_subtree_hash.is_empty()
        && tx.expected_subtree_hash != "…"
        && tx.expected_subtree_hash != got
    {
        return PatchResult {
            ok: false,
            applied: false,
            reason: Some("expected_subtree_hash mismatch (target shifted or edited)".into()),
            new_module_hash: None,
            source: None,
        };
    }

    let Some(repl) = tx.replacement_src.as_ref().or(None) else {
        // Validate-only transaction (hash checks passed).
        return PatchResult {
            ok: true,
            applied: false,
            reason: Some("validated; no replacement_src".into()),
            new_module_hash: Some(base),
            source: Some(src.to_string()),
        };
    };

    // Rewrite the named function's body in the formatted source.
    let name = def_name(&tx.def_id);
    let tree = crate::tree::looks_like_tree(src);
    let formatted = if tree {
        crate::tree::format_file(file, intern)
    } else {
        fmt::format_file(file, intern)
    };
    let rewritten = if tree {
        replace_fn_body_tree(&formatted, &name, repl)
    } else {
        replace_fn_body(&formatted, &name, repl)
    };
    let Some(rewritten) = rewritten else {
        return PatchResult {
            ok: false,
            applied: false,
            reason: Some("could not locate function body to rewrite".into()),
            new_module_hash: None,
            source: None,
        };
    };

    // Re-parse to confirm the result is still a program.
    let reparse_ok = if tree {
        crate::tree::parse_file(&rewritten, FileId(0), intern, "m").is_ok()
    } else {
        Parser::parse_file(&rewritten, FileId(0), intern).is_ok()
    };
    if !reparse_ok {
        return PatchResult {
            ok: false,
            applied: false,
            reason: Some("replacement does not parse".into()),
            new_module_hash: None,
            source: None,
        };
    }

    PatchResult {
        ok: true,
        applied: true,
        reason: None,
        new_module_hash: Some(hash::sha256_hex(rewritten.as_bytes())),
        source: Some(rewritten),
    }
}

fn find_decl<'a>(intern: &Interner, file: &'a File, def_id: &str) -> Option<&'a crate::ast::Decl> {
    let name = def_name(def_id);
    file.decls.iter().find(|d| match &d.kind {
        DeclKind::Fn(f) | DeclKind::ContractFn(f) => intern.get(f.name.name) == name,
        DeclKind::Type(t) => intern.get(t.name.name) == name,
        DeclKind::Dict(dd) => intern.get(dd.name.name) == name,
        DeclKind::Test(t) => t.name == name,
    })
}

fn def_name(def_id: &str) -> &str {
    def_id.rsplit(':').next().unwrap_or(def_id)
}

/// Replace `= <body>;` of `fn <name>` with `= <repl>;`.
fn replace_fn_body(src: &str, name: &str, repl: &str) -> Option<String> {
    let needle = format!("fn {name}");
    let start = src.find(&needle)?;
    let eq = src[start..].find("\n=").or_else(|| src[start..].find('='))?;
    let eq_abs = start + eq;
    let after_eq = src[eq_abs..].find('=')? + eq_abs + 1;
    // body runs to the matching top-level `;\n` after the `=`.
    let rest = &src[after_eq..];
    let end_rel = find_fn_semi(rest)?;
    let mut out = String::new();
    out.push_str(&src[..after_eq]);
    if !repl.starts_with(' ') && !repl.starts_with('\n') {
        out.push(' ');
    }
    let repl = repl.trim().trim_end_matches(';');
    out.push_str(repl);
    out.push(';');
    out.push_str(&src[after_eq + end_rel..]);
    Some(out)
}

/// Replace the last form of `(fn name … body)` with `repl`.
fn replace_fn_body_tree(src: &str, name: &str, repl: &str) -> Option<String> {
    let needle = format!("(fn {name}");
    let start = src.find(&needle)?;
    // Walk the fn list and record the last top-level form's span.
    let bytes = src.as_bytes();
    let mut i = start + 1; // skip the opening '(' of `(fn …)`
    let mut depth = 1i32;
    let mut form_start = None;
    let mut last = None;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'(' => {
                if depth == 1 {
                    form_start = Some(i);
                }
                depth += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 1 {
                    if let Some(s) = form_start.take() {
                        last = Some((s, i + 1));
                    }
                }
                if depth == 0 {
                    break;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b if depth == 1 && !b.is_ascii_whitespace() => {
                if form_start.is_none() {
                    let s = i;
                    while i + 1 < bytes.len()
                        && !bytes[i + 1].is_ascii_whitespace()
                        && bytes[i + 1] != b'('
                        && bytes[i + 1] != b')'
                    {
                        i += 1;
                    }
                    last = Some((s, i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    let (s, e) = last?;
    let mut out = String::new();
    out.push_str(&src[..s]);
    out.push_str(repl.trim());
    out.push_str(&src[e..]);
    Some(out)
}

fn find_fn_semi(rest: &str) -> Option<usize> {
    let mut depth = 0i32;
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            b';' if depth <= 0 => return Some(i + 1),
            b'"' | b'`' => {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayTrace {
    pub schema: u32,
    pub seed: u64,
    pub source_hash: String,
    pub result: String,
    pub canonical: String,
    /// Every effect the run performed, in order. Replay consumes these instead
    /// of touching the world, which is what makes a replayed run identical
    /// rather than merely similar.
    pub events: Vec<crate::interp::TraceEvent>,
}

pub fn encode_trace(
    seed: u64,
    source: &str,
    result: &str,
    canonical: &str,
    events: &[crate::interp::TraceEvent],
) -> ReplayTrace {
    ReplayTrace {
        schema: 2,
        seed,
        source_hash: hash::sha256_hex(source.as_bytes()),
        result: result.to_string(),
        canonical: canonical.to_string(),
        events: events.to_vec(),
    }
}
