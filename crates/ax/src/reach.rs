//! Capability reachability (spec v0.3 §7).
//!
//! Each stdlib IO primitive is tagged with a capability label. No user code
//! carries annotations. `ax caps --json` reports each capability's reachability
//! from `main` with the shortest call path.

use crate::ast::*;
use crate::check::CheckOutput;
use crate::intern::Interner;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Debug, Serialize)]
pub struct CapsReport {
    pub schema_version: String,
    pub from: String,
    pub capabilities: Vec<CapReach>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CapReach {
    pub cap: String,
    pub reachable: bool,
    pub path: Vec<String>,
}

const IO_CAPS: &[(&str, &str)] = &[
    ("fs.read", "fs"),
    ("fs.write", "fs"),
    ("io.bytesum_file", "io"),
    ("io.read_file", "io"),
    ("io.write_file", "io"),
    ("http.get", "net"),
    ("http.get_bytesum", "net"),
    ("http.serve", "net"),
    ("http.listen", "net"),
    ("http.accept", "net"),
    ("http.respond", "net"),
    ("http.close", "net"),
    ("http.serve_handler", "net"),
    ("http.serve_handler_config", "net"),
    ("http.serve_handler_state", "net"),
    ("http.serve_handler_state_config", "net"),
    ("db.open", "db"),
    ("db.open_timeout", "db"),
    ("db.set_timeout", "db"),
    ("db.close", "db"),
    ("db.exec", "db"),
    ("db.exec0", "db"),
    ("db.exec_values", "db"),
    ("db.query", "db"),
    ("db.query0", "db"),
    ("db.query_values", "db"),
    ("db.begin", "db"),
    ("db.tx_exec", "db"),
    ("db.tx_exec0", "db"),
    ("db.tx_exec_values", "db"),
    ("db.tx_query", "db"),
    ("db.tx_query0", "db"),
    ("db.tx_query_values", "db"),
    ("db.commit", "db"),
    ("db.rollback", "db"),
    ("argv", "env"),
    ("env.get_or", "env"),
    ("print", "io"),
];

pub fn analyze(intern: &Interner, checked: &CheckOutput) -> CapsReport {
    let mut callees: HashMap<String, Vec<String>> = HashMap::new();
    for f in &checked.fns {
        let name = intern.get(f.sig.name).to_string();
        let mut cs = Vec::new();
        collect_calls(intern, &f.body, &mut cs);
        callees.insert(name, cs);
    }

    let start = if callees.contains_key("main") {
        "main".to_string()
    } else {
        checked
            .fns
            .first()
            .map(|f| intern.get(f.sig.name).to_string())
            .unwrap_or_else(|| "main".into())
    };

    let mut capabilities = Vec::new();
    for &(prim, cap) in IO_CAPS {
        let path = shortest_path(&callees, &start, prim);
        capabilities.push(CapReach {
            cap: cap.into(),
            reachable: path.is_some(),
            path: path.unwrap_or_default(),
        });
    }
    // Dedup by cap, keep the shortest path that found it.
    let mut best: HashMap<String, CapReach> = HashMap::new();
    for c in capabilities {
        best.entry(c.cap.clone())
            .and_modify(|e| {
                if c.reachable && (!e.reachable || c.path.len() < e.path.len()) {
                    *e = c.clone();
                }
            })
            .or_insert(c);
    }
    let mut capabilities: Vec<_> = best.into_values().collect();
    capabilities.sort_by(|a, b| a.cap.cmp(&b.cap));

    CapsReport {
        schema_version: "1.0".into(),
        from: start,
        capabilities,
    }
}

/// Permitted capability set from `ax.toml`. Exceeding it is error `A5001`.
#[derive(Clone, Debug, Default)]
pub struct CapBudget {
    pub allowed: Vec<String>,
}

impl CapBudget {
    pub fn from_toml(text: &str) -> Self {
        let mut allowed = Vec::new();
        let mut in_caps = false;
        for line in text.lines() {
            let t = line.trim();
            if t == "[caps]" || t == "[capabilities]" {
                in_caps = true;
                continue;
            }
            if t.starts_with('[') {
                in_caps = false;
                continue;
            }
            if in_caps {
                if let Some(rest) = t.strip_prefix("allow") {
                    for part in rest.split(['=', '[', ']', ',', '"', ' ']) {
                        let p = part.trim();
                        if !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase()) {
                            allowed.push(p.to_string());
                        }
                    }
                }
            }
        }
        Self { allowed }
    }

    pub fn check(&self, report: &CapsReport) -> Vec<(String, Vec<String>)> {
        if self.allowed.is_empty() {
            return Vec::new();
        }
        report
            .capabilities
            .iter()
            .filter(|c| c.reachable && !self.allowed.iter().any(|a| a == &c.cap))
            .map(|c| (c.cap.clone(), c.path.clone()))
            .collect()
    }
}

/// A5002: a lockfile records the previously approved reachable set per
/// dependency. A newly wider set requires re-approval.
pub fn cap_widened(old: &[String], new: &[String]) -> Vec<String> {
    new.iter()
        .filter(|c| !old.iter().any(|o| o == *c))
        .cloned()
        .collect()
}

fn shortest_path(
    callees: &HashMap<String, Vec<String>>,
    start: &str,
    target: &str,
) -> Option<Vec<String>> {
    let mut q = VecDeque::new();
    let mut prev: HashMap<String, String> = HashMap::new();
    let mut seen = HashSet::new();
    q.push_back(start.to_string());
    seen.insert(start.to_string());
    while let Some(cur) = q.pop_front() {
        let empty = Vec::new();
        let nexts = callees.get(&cur).unwrap_or(&empty);
        for n in nexts {
            if n == target || n.ends_with(&format!(".{target}")) || n == &target.replace('.', "_") {
                let mut path = vec![n.clone()];
                let mut walk = cur;
                path.push(walk.clone());
                while let Some(p) = prev.get(&walk) {
                    path.push(p.clone());
                    walk = p.clone();
                }
                path.reverse();
                return Some(path);
            }
            if seen.insert(n.clone()) {
                prev.insert(n.clone(), cur.clone());
                q.push_back(n.clone());
            }
        }
        // Direct primitive call recorded as a callee name.
        if nexts.iter().any(|n| n == target || n.ends_with(target)) {
            return Some(vec![start.to_string(), target.to_string()]);
        }
    }
    // Also: the start function itself may call the primitive.
    if let Some(cs) = callees.get(start) {
        if cs.iter().any(|n| {
            n == target || n.ends_with(&target[target.find('.').map(|i| i + 1).unwrap_or(0)..])
        }) {
            return Some(vec![start.to_string(), target.to_string()]);
        }
    }
    None
}

fn collect_calls(intern: &Interner, e: &Expr, out: &mut Vec<String>) {
    match &e.kind {
        ExprKind::Call { callee, args } => {
            if let Some(n) = dotted(intern, callee) {
                out.push(n);
            }
            collect_calls(intern, callee, out);
            for a in args {
                collect_calls(intern, a, out);
            }
        }
        ExprKind::Field { base, .. } | ExprKind::Unary { expr: base, .. } => {
            collect_calls(intern, base, out)
        }
        ExprKind::Index { base, index } => {
            collect_calls(intern, base, out);
            collect_calls(intern, index, out);
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs } => {
            collect_calls(intern, lhs, out);
            collect_calls(intern, rhs, out);
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match &s.kind {
                    StmtKind::Let(l) => collect_calls(intern, &l.init, out),
                    StmtKind::Expr(x) => collect_calls(intern, x, out),
                }
            }
            if let Some(t) = tail {
                collect_calls(intern, t, out);
            }
        }
        ExprKind::If {
            cond,
            then_b,
            else_b,
        } => {
            collect_calls(intern, cond, out);
            collect_calls(intern, then_b, out);
            if let Some(el) = else_b {
                collect_calls(intern, el, out);
            }
        }
        ExprKind::Match { scrut, arms } | ExprKind::Catch { expr: scrut, arms } => {
            collect_calls(intern, scrut, out);
            for a in arms {
                collect_calls(intern, &a.body, out);
            }
        }
        ExprKind::For { iter, body, .. } | ExprKind::While { cond: iter, body } => {
            collect_calls(intern, iter, out);
            collect_calls(intern, body, out);
        }
        ExprKind::Loop { body } | ExprKind::Region { body, .. } | ExprKind::Lambda { body, .. } => {
            collect_calls(intern, body, out)
        }
        ExprKind::Record(fs) | ExprKind::Variant { fields: fs, .. } => {
            for (_, x) in fs {
                collect_calls(intern, x, out);
            }
        }
        ExprKind::Return(inner) => {
            if let Some(x) = inner {
                collect_calls(intern, x, out);
            }
        }
        ExprKind::Raise(inner)
        | ExprKind::Attempt(inner)
        | ExprKind::Try(inner)
        | ExprKind::Cast { expr: inner, .. } => collect_calls(intern, inner, out),
        ExprKind::Interpolate { parts } => {
            for p in parts {
                if let InterpPart::Expr(x) = p {
                    collect_calls(intern, x, out);
                }
            }
        }
        ExprKind::Let(l) => collect_calls(intern, &l.init, out),
        ExprKind::Par { bindings } => {
            for l in bindings {
                collect_calls(intern, &l.init, out);
            }
        }
        ExprKind::Lit(_)
        | ExprKind::Path(_)
        | ExprKind::Hole
        | ExprKind::Break
        | ExprKind::Continue => {}
    }
}

fn dotted(intern: &Interner, e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Path(p) => Some(
            p.segs
                .iter()
                .map(|s| intern.get(s.name).to_string())
                .collect::<Vec<_>>()
                .join("."),
        ),
        ExprKind::Field { base, field } => {
            let left = dotted(intern, base)?;
            Some(format!("{}.{}", left, intern.get(field.name)))
        }
        _ => None,
    }
}
