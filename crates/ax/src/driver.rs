//! Shared compile pipeline used by the CLI and tests.

use crate::ast::File;
use crate::check::{CheckOutput, Checker};
use crate::diag::Diagnostic;
use crate::fmt;
use crate::frontend::{self, Surface};
use crate::hash;
use crate::indep;
use crate::intern::Interner;
use crate::interp::{Interpreter, TestResult, Value};
use crate::span::SourceMap;
use serde::Serialize;

pub struct Session {
    pub intern: Interner,
    pub sm: SourceMap,
    pub allow_holes: bool,
    pub strict_det: bool,
    pub surface: Surface,
    pub indep_check: bool,
}

impl Session {
    pub fn new() -> Self {
        Self {
            intern: Interner::new(),
            sm: SourceMap::new(),
            allow_holes: false,
            strict_det: false,
            surface: Surface::Dense,
            indep_check: true,
        }
    }

    pub fn parse(&mut self, name: &str, src: &str) -> Result<File, Vec<Diagnostic>> {
        let id = self.sm.add(name.to_string(), src.to_string());
        // The file stem is the default module name for terse sources that omit
        // the declaration.
        let stem = std::path::Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("m")
            .replace(['-', '.'], "_");
        // Tree if the file opens with `(`. Everything else is the short
        // syntax (legacy conventional is rewritten, not a separate mode).
        let surface = crate::tree::detect_surface(src, self.surface);
        frontend::parse_surface_named(src, id, &mut self.intern, surface, &stem)
    }

    pub fn check(&mut self, file: &File) -> CheckOutput {
        let mut c = Checker::new(&mut self.intern, self.allow_holes, self.strict_det);
        if self.surface == Surface::Verbose {
            c.set_verbose(true);
        }
        let mut out = c.check_file(file);
        if self.indep_check {
            let facts = indep::TypeFacts::new(&out.node_types, &out.nonzero_div);
            for r in indep::infer_effects(file, &self.intern, facts) {
                if !r.permitted {
                    out.diags.push(Diagnostic::error(
                        "E0200",
                        crate::span::Span::DUMMY,
                        format!(
                            "independent effect checker: {} inferred {} not ⊆ {}",
                            r.fn_name,
                            r.inferred.display(),
                            r.declared.display()
                        ),
                    ));
                }
            }
        }
        out
    }

    pub fn compile(&mut self, name: &str, src: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
        let file = self.parse(name, src)?;
        let out = self.check(&file);
        if out.diags.iter().any(|d| d.is_error()) {
            return Err(out.diags);
        }
        Ok(out)
    }

    pub fn format(&mut self, name: &str, src: &str) -> Result<String, Vec<Diagnostic>> {
        let file = self.parse(name, src)?;
        match crate::tree::detect_surface(src, self.surface) {
            Surface::Tree => Ok(crate::tree::format_file(&file, &self.intern)),
            _ => {
                let conv = fmt::format_file(&file, &self.intern);
                Ok(crate::frontend::to_dense(&conv))
            }
        }
    }
}

pub fn render_diags(sm: &SourceMap, intern: &Interner, diags: &[Diagnostic]) -> String {
    let _ = intern;
    let mut o = String::new();
    for d in diags {
        let file = sm.get(crate::span::FileId(d.span.file));
        let (line, col) = file.map(|f| f.line_col(d.span.start)).unwrap_or((0, 0));
        let name = file.map(|f| f.name.as_str()).unwrap_or("<input>");
        o.push_str(&format!(
            "{name}:{line}:{col}: {}: {}: {}\n",
            match d.severity {
                crate::diag::Severity::Error => "error",
                crate::diag::Severity::Warning => "warning",
                crate::diag::Severity::Note => "note",
            },
            d.code,
            d.msg
        ));
        if let Some(f) = file {
            o.push_str(&format!("    {}\n", f.line_text(line)));
        }
    }
    o
}

#[derive(Serialize)]
pub struct CheckReport {
    pub ok: bool,
    pub module: String,
    pub diagnostics: Vec<Diagnostic>,
    pub holes: usize,
    pub defs: Vec<DefReport>,
}

#[derive(Serialize)]
pub struct DefReport {
    pub def_id: String,
    pub interface_hash: String,
    pub body_hash: String,
    pub build_hash: String,
}

pub fn check_report(out: &CheckOutput) -> CheckReport {
    CheckReport {
        ok: !out.diags.iter().any(|d| d.is_error()),
        module: out.module.clone(),
        diagnostics: out.diags.clone(),
        holes: out.holes.len(),
        defs: out
            .hashes
            .iter()
            .map(|h| DefReport {
                def_id: h.def_id.clone(),
                interface_hash: h.interface_hash.clone(),
                body_hash: h.body_hash.clone(),
                build_hash: h.build_hash.clone(),
            })
            .collect(),
    }
}

pub fn run_main(intern: &Interner, out: &CheckOutput, seed: u64) -> Result<Value, String> {
    run_main_with_args(intern, out, seed, &[])
}

/// Run `main` with the arguments the program should see. `argv(0)` is the module
/// path, as it would be for a native binary.
pub fn run_main_with_args(
    intern: &Interner,
    out: &CheckOutput,
    seed: u64,
    argv: &[String],
) -> Result<Value, String> {
    run_traced(intern, out, seed, argv, None).map(|(v, _)| v)
}

/// Run `main`, returning the value and the transcript of effects it performed.
/// Passing `replay` makes effects return their recorded results instead.
pub fn run_traced(
    intern: &Interner,
    out: &CheckOutput,
    seed: u64,
    argv: &[String],
    replay: Option<Vec<crate::interp::TraceEvent>>,
) -> Result<(Value, Vec<crate::interp::TraceEvent>), String> {
    let mut ip = Interpreter::new(intern, out, seed);
    ip.set_argv(argv.to_vec());
    if let Some(events) = replay {
        ip.set_replay(events);
    }
    // Only `main` is an entry point. Falling back to "the last function" used to
    // call something arbitrary with no arguments, which aborts deep inside that
    // function's body and reports a confusing error instead of the real problem.
    if out.fns.iter().any(|f| intern.get(f.sig.name) == "main") {
        let v = ip.call_fn("main", vec![])?;
        let events = ip.events().to_vec();
        return Ok((v, events));
    }
    let nullary: Vec<&str> = out
        .fns
        .iter()
        .filter(|f| f.sig.params.is_empty())
        .map(|f| intern.get(f.sig.name))
        .collect();
    Err(format!(
        "no `main` to run{}",
        if nullary.is_empty() {
            String::new()
        } else {
            format!("; nullary functions here: {}", nullary.join(", "))
        }
    ))
}

pub fn run_tests(intern: &Interner, out: &CheckOutput, seed: u64) -> Vec<TestResult> {
    let mut ip = Interpreter::new(intern, out, seed);
    ip.run_tests(out)
}

pub fn hole_report(intern: &Interner, out: &CheckOutput, filter: Option<&str>) -> String {
    let mut o = String::new();
    let mut n = 0;
    for h in &out.holes {
        if let Some(f) = filter {
            if !h.def_id.contains(f) && !h.path.contains(f) {
                continue;
            }
        }
        n += 1;
        o.push_str(&format!(
            "hole {n}  def={}  path={}\n  expects: {}\n  in scope:\n",
            h.def_id,
            h.path,
            h.expected.display_tree(intern)
        ));
        for (name, ty) in &h.in_scope {
            o.push_str(&format!("    {name}: {}\n", ty.display_tree(intern)));
        }
        o.push_str("  ranked candidates:\n");
        for c in &h.candidates {
            o.push_str(&format!(
                "    {}  {} -> {}    {}\n",
                c.rank, c.name, c.ty, c.note
            ));
        }
    }
    if n == 0 {
        o.push_str("no holes\n");
    }
    o
}

pub fn search(intern: &Interner, out: &CheckOutput, query: &str) -> String {
    let q = query.trim();
    let mut o = String::new();
    for f in &out.fns {
        let sig = format!(
            "(fn {} (…) {} {})",
            intern.get(f.sig.name),
            f.sig.ret.display_tree(intern),
            f.sig.effects.display_tree(intern)
        );
        if sig.contains(q) || intern.get(f.sig.name).contains(q) || q.is_empty() {
            o.push_str(&sig);
            o.push('\n');
        }
    }
    // also scan hashes / types
    for t in &out.types {
        let n = intern.get(t.name);
        if n.contains(q) {
            o.push_str(&format!("type {n}\n"));
        }
    }
    if o.is_empty() {
        o.push_str("no matches\n");
    }
    o
}

pub fn types_at(intern: &Interner, out: &CheckOutput, def_id: &str) -> String {
    for f in &out.fns {
        if f.sig.def_id.contains(def_id) || intern.get(f.sig.name) == def_id {
            return format!(
                "(fn {} (…) {} {})\n",
                intern.get(f.sig.name),
                f.sig.ret.display_tree(intern),
                f.sig.effects.display_tree(intern)
            );
        }
    }
    "unknown def\n".into()
}

pub fn effs_at(intern: &Interner, out: &CheckOutput, def_id: &str) -> String {
    for f in &out.fns {
        if f.sig.def_id.contains(def_id) || intern.get(f.sig.name) == def_id {
            return format!(
                "declared {}\ninferred {}\n",
                f.sig.effects.display_tree(intern),
                f.inferred.display_tree(intern)
            );
        }
    }
    "unknown def\n".into()
}

pub fn errs_into(intern: &Interner, out: &CheckOutput, ty_name: &str) -> String {
    let mut o = String::new();
    for t in &out.types {
        if intern.get(t.name) == ty_name || intern.get(t.name).ends_with(ty_name) {
            if t.injections.is_empty() {
                o.push_str("no injections\n");
            }
            for inj in &t.injections {
                o.push_str(&format!(
                    "from {} => {}\n",
                    inj.from.display_tree(intern),
                    intern.get(inj.into_variant)
                ));
            }
            return o;
        }
    }
    format!("unknown type {ty_name}\n")
}

pub fn deps_affected(out: &CheckOutput, def_id: &str) -> String {
    let mut o = String::new();
    // interface_hash drives caller invalidation: body-only change of this
    // def invalidates nothing else. We report the def itself plus any
    // definition whose interface mentions it (best-effort for M1).
    o.push_str(&format!("{def_id}\n"));
    for h in &out.hashes {
        if h.def_id != def_id && h.interface_hash.contains(&def_id[..def_id.len().min(8)]) {
            o.push_str(&format!("{}\n", h.def_id));
        }
    }
    o
}

/// Prelude entry points that perform IO without taking a capability handle.
///
/// These are the reason a program can fail to be capability-contained: they take
/// a raw path, URL, or process argument and act on it with the process's ambient
/// authority. Anything reached through a `ReadCap` (or another handle) is
/// contained by construction.
const AMBIENT_IO: &[&str] = &[
    "io.bytesum_file",
    "io.read_file",
    "io.write_file",
    "http.get",
    "http.get_bytesum",
    "http.serve",
    "argv",
];

/// Guarantee labels for a checked module.
///
/// Labels are claims, so each one is earned rather than asserted: a program that
/// calls an ambient-authority builtin is not capability-contained, and saying so
/// anyway would make the label worthless.
pub fn guarantee_labels(
    intern: &Interner,
    out: &CheckOutput,
    trusted_ffi: bool,
    sandboxed: bool,
) -> Vec<String> {
    let mut labels = Vec::new();
    let mut has_io = false;
    let mut has_race = false;
    let mut has_nondet = false;
    for f in &out.fns {
        if f.sig.effects.has_io() {
            has_io = true;
        }
        if f.sig.effects.has_race() {
            has_race = true;
        }
        if f.sig.effects.has_nondet() {
            has_nondet = true;
        }
    }
    let mut ambient: Vec<String> = Vec::new();
    for f in &out.fns {
        collect_ambient_calls(intern, &f.body, &mut ambient);
    }
    for t in &out.tests {
        collect_ambient_calls(intern, &t.body, &mut ambient);
    }
    ambient.sort();
    ambient.dedup();

    if trusted_ffi {
        labels.push("trusted-ffi".into());
        if sandboxed {
            labels.push("capability-contained".into());
        }
    } else {
        labels.push("safe".into());
        if ambient.is_empty() {
            labels.push("capability-contained".into());
        }
    }
    if !ambient.is_empty() {
        // Name the offenders: "not contained" is only actionable if you know why.
        labels.push(format!("ambient-io({})", ambient.join(" ")));
    }
    if !has_io && !has_race && !has_nondet {
        labels.push("deterministic-core".into());
    }
    // A run is replayable from a transcript only if every effect went through a
    // handle the world can record. Ambient IO is not in the transcript.
    if ambient.is_empty() {
        labels.push("replay-deterministic".into());
    }
    labels
}

/// Names of ambient-authority prelude calls appearing in an expression.
fn collect_ambient_calls(intern: &Interner, e: &crate::ast::Expr, out: &mut Vec<String>) {
    use crate::ast::ExprKind as K;
    let note = |name: Option<String>, out: &mut Vec<String>| {
        if let Some(n) = name {
            if AMBIENT_IO.contains(&n.as_str()) {
                out.push(n);
            }
        }
    };
    match &e.kind {
        K::Call { callee, args } => {
            note(dotted_name(intern, callee), out);
            collect_ambient_calls(intern, callee, out);
            for a in args {
                collect_ambient_calls(intern, a, out);
            }
        }
        K::Field { base, .. } => collect_ambient_calls(intern, base, out),
        K::Index { base, index } => {
            collect_ambient_calls(intern, base, out);
            collect_ambient_calls(intern, index, out);
        }
        K::Unary { expr, .. } => collect_ambient_calls(intern, expr, out),
        K::Binary { lhs, rhs, .. } => {
            collect_ambient_calls(intern, lhs, out);
            collect_ambient_calls(intern, rhs, out);
        }
        K::Block { stmts, tail } => {
            for st in stmts {
                match &st.kind {
                    crate::ast::StmtKind::Let(l) => collect_ambient_calls(intern, &l.init, out),
                    crate::ast::StmtKind::Expr(x) => collect_ambient_calls(intern, x, out),
                }
            }
            if let Some(t) = tail {
                collect_ambient_calls(intern, t, out);
            }
        }
        K::If {
            cond,
            then_b,
            else_b,
        } => {
            collect_ambient_calls(intern, cond, out);
            collect_ambient_calls(intern, then_b, out);
            if let Some(x) = else_b {
                collect_ambient_calls(intern, x, out);
            }
        }
        K::Match { scrut, arms } | K::Catch { expr: scrut, arms } => {
            collect_ambient_calls(intern, scrut, out);
            for a in arms {
                collect_ambient_calls(intern, &a.body, out);
            }
        }
        K::For { iter, body, .. } => {
            collect_ambient_calls(intern, iter, out);
            collect_ambient_calls(intern, body, out);
        }
        K::Loop { body } | K::Region { body, .. } => collect_ambient_calls(intern, body, out),
        K::While { cond, body } => {
            collect_ambient_calls(intern, cond, out);
            collect_ambient_calls(intern, body, out);
        }
        K::Cast { expr, .. } => collect_ambient_calls(intern, expr, out),
        K::Break | K::Continue => {}
        K::Let(l) => collect_ambient_calls(intern, &l.init, out),
        K::Lambda { body, .. } => collect_ambient_calls(intern, body, out),
        K::Record(fs) | K::Variant { fields: fs, .. } => {
            for (_, x) in fs {
                collect_ambient_calls(intern, x, out);
            }
        }
        K::Return(inner) => {
            if let Some(x) = inner {
                collect_ambient_calls(intern, x, out);
            }
        }
        K::Raise(inner) | K::Attempt(inner) | K::Try(inner) => {
            collect_ambient_calls(intern, inner, out)
        }
        K::Interpolate { parts } => {
            for p in parts {
                if let crate::ast::InterpPart::Expr(x) = p {
                    collect_ambient_calls(intern, x, out);
                }
            }
        }
        K::Par { bindings } => {
            for l in bindings {
                collect_ambient_calls(intern, &l.init, out);
            }
        }
        K::Assign { lhs, rhs } => {
            collect_ambient_calls(intern, lhs, out);
            collect_ambient_calls(intern, rhs, out);
        }
        K::Lit(_) | K::Path(_) | K::Hole => {}
    }
}

/// Dotted name of a static call target.
fn dotted_name(intern: &Interner, e: &crate::ast::Expr) -> Option<String> {
    match &e.kind {
        crate::ast::ExprKind::Path(p) => Some(
            p.segs
                .iter()
                .map(|s| intern.get(s.name).to_string())
                .collect::<Vec<_>>()
                .join("."),
        ),
        // `io.bytesum_file(..)` parses as a call on a field access, so a
        // Path-only match would miss every qualified prelude call.
        crate::ast::ExprKind::Field { base, field } => {
            let left = dotted_name(intern, base)?;
            Some(format!("{left}.{}", intern.get(field.name)))
        }
        _ => None,
    }
}

pub fn card_text() -> &'static str {
    include_str!("../../../spec/card.md")
}

pub fn source_hash(src: &str) -> String {
    hash::sha256_hex(src.as_bytes())
}
