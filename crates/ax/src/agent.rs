//! The agent-facing layer: hole fills that compile.
//!
//! `ax hole` on its own ranks *names*, which an agent cannot paste. This module
//! synthesises candidate **expressions** and then verifies each one the only way
//! that counts: by substituting it into the source and running the checker. A
//! fill that survives is known to compile, so an agent that takes the top fill
//! never burns an attempt discovering a type error the compiler already knew.
//!
//! Verification is affordable because `ax check` is a few hundred microseconds:
//! trying fifty candidates costs less than one `rustc` invocation. That is the
//! whole argument for the protocol — not that the language is cleverer, but that
//! asking it a question is cheap enough to do in a loop.

use crate::check::CheckOutput;
use crate::driver::Session;
use crate::frontend::Surface;
use crate::types::{types_eq, Prim, Type};
use serde::Serialize;

/// A candidate fill for one hole.
#[derive(Clone, Debug, Serialize)]
pub struct Fill {
    /// Source text to substitute for the `?`.
    pub expr: String,
    /// Lower is better.
    pub rank: u32,
    /// Why this was proposed.
    pub note: String,
    /// The whole module checks clean with this fill substituted.
    pub compiles: bool,
    /// Diagnostic codes the fill produced, when it does not compile.
    pub rejected_by: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HoleFills {
    pub def_id: String,
    pub expected: String,
    /// Byte offsets of the `?` being filled.
    pub span: (u32, u32),
    pub fills: Vec<Fill>,
}

/// Synthesise and verify fills for every hole in `src`.
///
/// `limit` caps how many candidates are *verified* per hole; generation is
/// ordered so the plausible ones are tried first.
pub fn hole_fills(name: &str, src: &str, surface: Surface, limit: usize) -> Vec<HoleFills> {
    let mut s = Session::new();
    s.allow_holes = true;
    s.surface = surface;
    let Ok(out) = s.compile(name, src) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for hole in &out.holes {
        let expected = hole.expected.clone();
        let mut cands = generate(&expected, hole, &out, &s);
        // Verify in generated order, then re-rank so compiling fills come first.
        let mut verified = Vec::new();
        for (i, (expr, note)) in cands.drain(..).enumerate().take(limit) {
            let patched = replace_span(src, hole.span.start, hole.span.end, &expr);
            let (compiles, codes) = check_clean(name, &patched, surface);
            verified.push(Fill {
                expr,
                rank: i as u32,
                note,
                compiles,
                rejected_by: codes,
            });
        }
        verified.sort_by_key(|f| (!f.compiles, f.rank));
        for (i, f) in verified.iter_mut().enumerate() {
            f.rank = i as u32 + 1;
        }
        result.push(HoleFills {
            def_id: hole.def_id.clone(),
            expected: expected.display(&s.intern),
            span: (hole.span.start, hole.span.end),
            fills: verified,
        });
    }
    result
}

/// Does the module check with no errors?
fn check_clean(name: &str, src: &str, surface: Surface) -> (bool, Vec<String>) {
    let mut s = Session::new();
    s.surface = surface;
    // Holes are not allowed here: a fill that leaves a hole has not filled it.
    s.allow_holes = false;
    match s.compile(name, src) {
        Ok(out) => {
            let codes: Vec<String> = out
                .diags
                .iter()
                .filter(|d| d.is_error())
                .map(|d| d.code.clone())
                .collect();
            (codes.is_empty(), codes)
        }
        Err(diags) => (
            false,
            diags
                .iter()
                .filter(|d| d.is_error())
                .map(|d| d.code.clone())
                .collect(),
        ),
    }
}

fn replace_span(src: &str, start: u32, end: u32, with: &str) -> String {
    let (s, e) = (start as usize, end as usize);
    if e > src.len() || s > e {
        return src.to_string();
    }
    format!("{}{}{}", &src[..s], with, &src[e..])
}

/// Candidate expressions for a hole of type `expected`, most plausible first.
///
/// The order encodes a preference: something already in scope beats a call,
/// a shorter call beats a longer one, and a literal is the last resort.
fn generate(
    expected: &Type,
    hole: &crate::check::HoleInfo,
    out: &CheckOutput,
    s: &Session,
) -> Vec<(String, String)> {
    let intern = &s.intern;
    // The function whose body contains the hole, e.g. `...::fn:distance`.
    // A def_id looks like `module::fn:name`; take the text after the last colon.
    let enclosing = hole
        .def_id
        .rsplit(':')
        .next()
        .unwrap_or("")
        .to_string();
    let mut cands: Vec<(String, String)> = Vec::new();

    // Values in scope, and their fields, that already have the expected type.
    let mut scope_terms: Vec<(String, Type)> = Vec::new();
    for (name, ty) in &hole.in_scope {
        scope_terms.push((name.clone(), ty.clone()));
        for (fname, fty) in record_fields(ty, out, intern) {
            scope_terms.push((format!("{name}.{fname}"), fty));
        }
    }
    for (term, ty) in &scope_terms {
        if types_eq(ty, expected) {
            cands.push((term.clone(), "in scope, exact type".into()));
        }
    }

    // Variant constructors of the expected type.
    if let Type::Named { def, .. } = expected {
        if let Some(td) = out.types.iter().find(|t| t.name == *def) {
            if let crate::types::TypeDefKind::Variants(vs) = &td.kind {
                for (vname, fields) in vs {
                    let vn = intern.get(*vname).to_string();
                    if fields.is_empty() {
                        cands.push((vn.clone(), "variant of the expected type".into()));
                    } else {
                        // One argument per payload field, drawn from scope.
                        let mut args = Vec::new();
                        for (_, fty) in fields {
                            match scope_terms.iter().find(|(_, t)| types_eq(t, fty)) {
                                Some((term, _)) => args.push(term.clone()),
                                None => {
                                    args.push(literal_for(fty).unwrap_or_else(|| "?".into()))
                                }
                            }
                        }
                        if !args.iter().any(|a| a == "?") {
                            cands.push((
                                format!("{vn}({})", args.join(", ")),
                                "variant constructor with in-scope payload".into(),
                            ));
                        }
                    }
                }
            }
        }
    }

    // Calls whose result matches and whose arguments can be satisfied.
    type CallKey = (bool, std::cmp::Reverse<usize>, usize, bool);
    let mut calls: Vec<(CallKey, String, String)> = Vec::new();
    for c in &out.callables {
        if !types_eq(&c.ret, expected) {
            continue;
        }
        // A call that can raise or do IO changes the enclosing row, so it is a
        // worse guess than a pure one; rank it later rather than dropping it.
        let impure = !c.effects.is_empty();
        // Prefer distinct arguments: a two-argument call filled with the same
        // term twice (`hypot(v.x, v.x)`) is almost never what was meant.
        let mut used: Vec<String> = Vec::new();
        let mut args: Vec<String> = Vec::new();
        let mut ok = true;
        for p in &c.params {
            match pick_arg(p, &scope_terms, &used) {
                Some(a) => {
                    used.push(a.trim_start_matches("&mut ").trim_start_matches('&').to_string());
                    args.push(a);
                }
                None => match literal_for(p) {
                    Some(l) => args.push(l),
                    None => {
                        ok = false;
                        break;
                    }
                },
            }
        }
        if !ok {
            continue;
        }
        // Calling the function whose body is being filled is legal and
        // non-terminating; it must never outrank a real answer.
        // Never propose the function whose body is being filled: the fill would
        // typecheck and then recurse forever, so it is never the answer even
        // though verification cannot tell.
        let bare = c.name.rsplit('.').next().unwrap_or(&c.name);
        if bare == enclosing {
            continue;
        }
        let expr = if args.is_empty() {
            format!("{}()", c.name)
        } else {
            format!("{}({})", c.name, args.join(", "))
        };
        // Ranking keys, in order of importance:
        //  - prefer pure calls (an impure one changes the declared row),
        //  - prefer wider use of the values in scope: a body that ignores a
        //    parameter it was given is a weaker guess,
        //  - then fewer arguments, then the module's own functions.
        let key = (
            impure,
            std::cmp::Reverse(used.len()),
            args.len(),
            c.from_prelude,
        );
        calls.push((
            key,
            expr,
            if c.from_prelude {
                "prelude call, matching result type".into()
            } else {
                "module function, matching result type".into()
            },
        ));
    }
    calls.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    for (_, expr, note) in calls {
        cands.push((expr, note));
    }

    // A literal of the expected type: always compiles, rarely what was meant,
    // so it goes last.
    if let Some(l) = literal_for(expected) {
        cands.push((l, "literal of the expected type".into()));
    }

    cands.dedup_by(|a, b| a.0 == b.0);
    cands
}

/// An in-scope term usable as an argument of type `want`, allowing auto-ref.
///
/// Terms already consumed by earlier arguments are considered only as a
/// fallback, so a multi-argument call reaches for different values first.
fn pick_arg(want: &Type, scope: &[(String, Type)], used: &[String]) -> Option<String> {
    let fresh = |t: &(String, Type)| !used.contains(&t.0);
    if let Some((term, _)) = scope.iter().find(|t| types_eq(&t.1, want) && fresh(t)) {
        return Some(term.clone());
    }
    // `&T` accepts a `T` in scope by reference.
    if let Type::Ref { inner, mutable, .. } = want {
        if let Some((term, _)) = scope.iter().find(|t| types_eq(&t.1, inner) && fresh(t)) {
            return Some(if *mutable {
                format!("&mut {term}")
            } else {
                format!("&{term}")
            });
        }
    }
    // Fall back to reuse rather than giving up on the call entirely.
    if let Some((term, _)) = scope.iter().find(|(_, t)| types_eq(t, want)) {
        return Some(term.clone());
    }
    if let Type::Ref { inner, mutable, .. } = want {
        if let Some((term, _)) = scope.iter().find(|(_, t)| types_eq(t, inner)) {
            return Some(if *mutable {
                format!("&mut {term}")
            } else {
                format!("&{term}")
            });
        }
    }
    None
}

/// Fields of a record type, for projection candidates.
fn record_fields(
    ty: &Type,
    out: &CheckOutput,
    intern: &crate::intern::Interner,
) -> Vec<(String, Type)> {
    let bare = match ty {
        Type::Ref { inner, .. }
        | Type::Own(inner)
        | Type::Untrusted(inner)
        | Type::Secret(inner) => (**inner).clone(),
        other => other.clone(),
    };
    match &bare {
        Type::Record(fs) => fs
            .iter()
            .map(|(n, t)| (intern.get(*n).to_string(), t.clone()))
            .collect(),
        Type::Named { def, .. } => out
            .types
            .iter()
            .find(|t| t.name == *def)
            .and_then(|td| match &td.kind {
                crate::types::TypeDefKind::Record(fs) => Some(fs.clone()),
                _ => None,
            })
            .map(|fs| {
                fs.iter()
                    .map(|(n, t)| (intern.get(*n).to_string(), t.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// A literal of this type, when one exists.
fn literal_for(ty: &Type) -> Option<String> {
    let p = ty.as_prim()?;
    Some(match p {
        Prim::Bool => "false".into(),
        Prim::F32 => "0.0f32".into(),
        Prim::F64 => "0.0f64".into(),
        Prim::Unit => "()".into(),
        other if other.is_int() => format!("0{}", other.as_str()),
        _ => return None,
    })
}

/// Report shape for `ax hole --fills --json`.
#[derive(Serialize)]
pub struct FillReport {
    pub holes: Vec<HoleFills>,
}


/// A fix that was, or could be, applied automatically.
#[derive(Clone, Debug, Serialize)]
pub struct AppliedFix {
    pub code: String,
    pub kind: String,
    pub note: Option<String>,
    /// The text that was replaced, and what replaced it.
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FixReport {
    pub applied: Vec<AppliedFix>,
    /// Fixes that exist but were withheld because they are not
    /// `semantics_preserving`. An agent may still choose to apply one; the
    /// toolchain will not do it silently.
    pub withheld: Vec<AppliedFix>,
    pub source: String,
    /// Does the module check cleanly after applying?
    pub clean: bool,
    pub remaining: Vec<String>,
}

/// Apply every `semantics_preserving` fix the checker offers, and nothing else.
///
/// A fix's `patch` may contain `$0`, which stands for the source text the
/// diagnostic's span covers — that is how a fix says "wrap what is here" rather
/// than having to reproduce it.
pub fn apply_safe_fixes(name: &str, src: &str, surface: Surface) -> FixReport {
    let mut s = Session::new();
    s.surface = surface;
    let diags = match s.compile(name, src) {
        Ok(out) => out.diags,
        Err(d) => d,
    };

    let mut edits: Vec<(usize, usize, String, AppliedFix)> = Vec::new();
    let mut withheld = Vec::new();
    for d in &diags {
        let (start, end) = (d.span.start as usize, d.span.end as usize);
        if end > src.len() || start > end {
            continue;
        }
        let before = src[start..end].to_string();
        for f in &d.fixes {
            let after = f.patch.replace("$0", &before);
            let record = AppliedFix {
                code: d.code.clone(),
                kind: f.kind.clone(),
                note: f.note.clone(),
                before: before.clone(),
                after: after.clone(),
            };
            match f.safety {
                crate::diag::FixSafety::SemanticsPreserving => {
                    edits.push((start, end, after, record));
                }
                _ => withheld.push(record),
            }
            // One fix per diagnostic: the highest-ranked one.
            break;
        }
    }

    // Apply from the end so earlier offsets stay valid, and skip overlaps.
    edits.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = src.to_string();
    let mut applied = Vec::new();
    let mut last_start = usize::MAX;
    for (start, end, text, record) in edits {
        if end > last_start {
            continue;
        }
        out.replace_range(start..end, &text);
        last_start = start;
        applied.push(record);
    }

    let mut s2 = Session::new();
    s2.surface = surface;
    let (clean, remaining) = match s2.compile(name, &out) {
        Ok(o) => {
            let codes: Vec<String> = o
                .diags
                .iter()
                .filter(|d| d.is_error())
                .map(|d| d.code.clone())
                .collect();
            (codes.is_empty(), codes)
        }
        Err(d) => (
            false,
            d.iter()
                .filter(|x| x.is_error())
                .map(|x| x.code.clone())
                .collect(),
        ),
    };
    FixReport {
        applied,
        withheld,
        source: out,
        clean,
        remaining,
    }
}
