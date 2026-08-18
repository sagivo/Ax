//! Attempts-to-green harness: does the compiler protocol actually save an agent
//! work?
//!
//! **What this measures.** Both arms face the same task: a program with one hole
//! and a known expected output. Both draw from the *same candidate pool*, in the
//! same generation order. The only difference is what the toolchain offers:
//!
//! - **ax** can ask `ax hole --fills`, which ranks candidates and rejects the
//!   ones that do not typecheck, using a checker that costs microseconds.
//! - **rust** has no such query. It discovers whether a candidate is viable the
//!   only way it can: compile it and run it.
//!
//! An **attempt** is one compile-and-run cycle of a candidate program — the
//! expensive step an agent pays for. A **probe** is a cheap static query that
//! does not produce a runnable artifact. The claim being tested is narrow: that
//! ranking plus cheap verification reduces attempts, and that `ax check` being
//! fast makes the probes nearly free in wall-clock terms.
//!
//! **What this is not.** No language model is involved. The agent is mechanical:
//! it tries candidates in the order its toolchain gives it. That measures the
//! protocol's utility, not a model's skill, and the numbers should never be
//! quoted as an LLM benchmark. The Rust arm is not a strawman — it runs a real
//! `rustc` — but it is also not an expert Rust programmer.

use crate::agent;
use crate::driver::{run_main, Session};
use crate::frontend::Surface;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

#[derive(Clone, Debug, Serialize)]
pub struct HiddenTask {
    pub id: String,
    pub spec: String,
    /// Program with `?` where the answer goes.
    pub ax_starter: String,
    /// Equivalent Rust program with `{FILL}` where the answer goes.
    pub rust_template: String,
    /// Candidate expressions, in generation order, rendered for each language.
    /// The same pool, so the arms differ only in ordering and verification.
    pub candidates: Vec<Candidate>,
    pub expected: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub ax: String,
    pub rust: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LoopResult {
    pub task_id: String,
    pub arm: String,
    /// Compile-and-run cycles used.
    pub attempts: u32,
    /// Cheap static queries used (ax only).
    pub probes: u32,
    pub green: bool,
    pub wall_ms: f64,
    pub tokens_est: u32,
    /// Tokens of program text the agent had to emit across all attempts. An
    /// agent pays for every candidate it writes out, not just the one that works.
    pub tokens_written: u32,
    /// Tokens of compiler output the agent had to read back. This is where a
    /// structured diagnostic beats a paragraph of prose.
    pub tokens_read: u32,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LoopReport {
    pub seed: u64,
    pub n: usize,
    pub ax: Vec<LoopResult>,
    pub rust: Vec<LoopResult>,
    pub ax_median_attempts: f64,
    pub rust_median_attempts: f64,
    pub ax_median_wall_ms: f64,
    pub rust_median_wall_ms: f64,
    pub ax_pass: usize,
    pub rust_pass: usize,
    /// Median tokens written + read to reach green. This is what an agent is
    /// actually billed for, and it is a different question from how many tokens
    /// one source file takes.
    pub ax_median_tokens: f64,
    pub rust_median_tokens: f64,
    /// True when the Rust arm could not run because no `rustc` was found.
    pub rust_skipped: bool,
}

/// Procedural hidden tasks. Specs are randomized so they are not in
/// any model's training data.
pub fn generate_hidden(seed: u64, n: usize) -> Vec<HiddenTask> {
    let mut out = Vec::with_capacity(n);
    let mut s = seed | 1;
    for i in 0..n {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        let kind = (s >> 8) as usize % 4;
        let a = 2 + ((s >> 16) as i64 % 20);
        let b = 2 + ((s >> 32) as i64 % 20);
        let id = format!("hid-{i:04}-{:x}", (s >> 48) as u16);
        out.push(match kind {
            0 => task_add(&id, a, b),
            1 => task_ident(&id, a),
            2 => task_sum_range(&id, a.max(3) as u64),
            _ => task_clamp(&id, a, b, a + b),
        });
    }
    out
}

/// Candidate pool shared by both arms: small integers, the in-scope names, and
/// a few arithmetic combinations. Order is fixed and language-independent.
fn pool(names: &[&str], nums: &[i64]) -> Vec<Candidate> {
    let mut out = Vec::new();
    for n in nums {
        out.push(Candidate {
            ax: n.to_string(),
            rust: n.to_string(),
        });
    }
    for nm in names {
        out.push(Candidate {
            ax: (*nm).to_string(),
            rust: (*nm).to_string(),
        });
    }
    for nm in names {
        for n in nums {
            out.push(Candidate {
                ax: format!("{nm} + {n}"),
                rust: format!("{nm} + {n}"),
            });
        }
    }
    out
}

fn task_add(id: &str, a: i64, b: i64) -> HiddenTask {
    HiddenTask {
        id: id.into(),
        spec: format!("return {a} plus {b} as i32"),
        ax_starter: format!("module t;\nfn main() -> i32 = {a} + ?;\n"),
        rust_template: "fn main() { println!(\"{}\", ".to_string()
            + &format!("{a} + ({{FILL}}) as i32); }}\n"),
        candidates: pool(&[], &[0, 1, b, a]),
        expected: a + b,
    }
}

fn task_ident(id: &str, a: i64) -> HiddenTask {
    HiddenTask {
        id: id.into(),
        spec: format!("return the in-scope binding whose value is {a}"),
        ax_starter: format!("module t;\nfn main() -> i32 = {{ let x: i32 = {a}; ? }};\n"),
        rust_template: format!(
            "fn main() {{ let x: i32 = {a}; println!(\"{{}}\", ({{FILL}}) as i32); }}\n"
        ),
        candidates: pool(&["x"], &[0, 1]),
        expected: a,
    }
}

fn task_sum_range(id: &str, n: u64) -> HiddenTask {
    HiddenTask {
        id: id.into(),
        spec: format!("count the iterations of 0..{n}"),
        ax_starter: format!(
            "module t;\nfn main() -> i32 = {{\n    let mut s: i32 = 0;\n    for i in range(0, {n}) {{\n        s = s + 1;\n    }};\n    ?\n}};\n"
        ),
        rust_template: format!(
            "fn main() {{ let mut s: i32 = 0; for _i in 0..{n} {{ s += 1; }} println!(\"{{}}\", ({{FILL}}) as i32); }}\n"
        ),
        candidates: pool(&["s"], &[0, 1]),
        expected: n as i64,
    }
}

fn task_clamp(id: &str, lo: i64, hi: i64, x: i64) -> HiddenTask {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    HiddenTask {
        id: id.into(),
        spec: format!("clamp {x} into [{lo}, {hi}]"),
        ax_starter: format!(
            "module t;\nfn main() -> i32 = if {x} < {lo} {{ {lo} }} else {{ if {x} > {hi} {{ {hi} }} else {{ ? }} }};\n"
        ),
        rust_template: format!(
            "fn main() {{ let v = if {x} < {lo} {{ {lo} }} else if {x} > {hi} {{ {hi} }} else {{ ({{FILL}}) as i64 }}; println!(\"{{}}\", v); }}\n"
        ),
        candidates: pool(&[], &[0, 1, x, lo, hi]),
        expected: x.clamp(lo, hi),
    }
}

/// Apply a hole fill by replacing the first `?` with `repl`.
pub fn fill_first_hole(src: &str, repl: &str) -> String {
    match src.find('?') {
        Some(i) => format!("{}{}{}", &src[..i], repl, &src[i + 1..]),
        None => src.to_string(),
    }
}

/// The ax arm: rank and pre-verify with the protocol, then run only candidates
/// that already typecheck.
pub fn run_ax_loop(task: &HiddenTask, max_attempts: u32) -> LoopResult {
    let t0 = Instant::now();
    let mut probes = 0u32;
    let mut attempts = 0u32;
    let mut written = 0u32;
    let mut read = 0u32;
    let mut last = None;

    // One protocol query: ranked fills, each already known to typecheck.
    let holes = agent::hole_fills("task.ax", &task.ax_starter, Surface::Conventional, 64);
    probes += 1;
    let mut ordered: Vec<String> = holes
        .first()
        .map(|h| {
            h.fills
                .iter()
                .filter(|f| f.compiles)
                .map(|f| f.expr.clone())
                .collect()
        })
        .unwrap_or_default();

    // The synthesiser proposes from types alone; the task's own pool may contain
    // an answer it would not have guessed (an arithmetic combination). Append the
    // pool's remaining candidates, still pre-verified, so both arms can reach the
    // same answers.
    for c in &task.candidates {
        if !ordered.contains(&c.ax) {
            let patched = fill_first_hole(&task.ax_starter, &c.ax);
            probes += 1;
            let mut s = Session::new();
            if s.compile("task.ax", &patched).is_ok() {
                ordered.push(c.ax.clone());
            }
        }
    }

    // The protocol's reply is what the agent reads: one line per candidate.
    read += crate::tokens::count(&ordered.join("\n")).tokens as u32;
    for cand in ordered.iter().take(max_attempts as usize) {
        let patched = fill_first_hole(&task.ax_starter, cand);
        attempts += 1;
        written += crate::tokens::count(&patched).tokens as u32;
        let mut s = Session::new();
        match s.compile("task.ax", &patched) {
            Ok(out) => match run_main(&s.intern, &out, 0) {
                Ok(v) if v.as_i128() == task.expected as i128 => {
                    return LoopResult {
                        task_id: task.id.clone(),
                        arm: "ax".into(),
                        attempts,
                        probes,
                        green: true,
                        wall_ms: t0.elapsed().as_secs_f64() * 1000.0,
                        tokens_est: estimate_tokens(&patched),
                        tokens_written: written,
                        tokens_read: read,
                        last_error: None,
                    };
                }
                Ok(v) => last = Some(format!("{cand} produced {}", v.display())),
                Err(e) => last = Some(format!("{cand}: {e}")),
            },
            Err(d) => {
                // Structured diagnostics: a code and a message, not a paragraph.
                let text = d
                    .iter()
                    .map(|x| format!("{} {}", x.code, x.msg))
                    .collect::<Vec<_>>()
                    .join("\n");
                read += crate::tokens::count(&text).tokens as u32;
                last = Some(format!("{cand}: {text}"));
            }
        }
    }
    LoopResult {
        task_id: task.id.clone(),
        arm: "ax".into(),
        attempts,
        probes,
        green: false,
        wall_ms: t0.elapsed().as_secs_f64() * 1000.0,
        tokens_est: estimate_tokens(&task.ax_starter),
        tokens_written: written,
        tokens_read: read,
        last_error: last,
    }
}

/// Is a Rust toolchain available? Without one the control arm cannot run, and
/// reporting a comparison anyway would be fiction.
pub fn rustc_available() -> bool {
    Command::new("rustc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The rust arm: no hole protocol, so every candidate costs a real compile and
/// run to evaluate.
pub fn run_rust_loop(task: &HiddenTask, max_attempts: u32) -> LoopResult {
    let t0 = Instant::now();
    let dir = std::env::temp_dir().join("ax_evalloop");
    let _ = std::fs::create_dir_all(&dir);
    let src_path = dir.join(format!("{}.rs", task.id.replace('-', "_")));
    let bin_path = dir.join(format!("{}_bin", task.id.replace('-', "_")));
    let mut attempts = 0u32;
    let mut written = 0u32;
    let mut read = 0u32;
    let mut last = None;

    for cand in task.candidates.iter().take(max_attempts as usize) {
        let src = task.rust_template.replace("{FILL}", &cand.rust);
        if std::fs::write(&src_path, &src).is_err() {
            break;
        }
        attempts += 1;
        written += crate::tokens::count(&src).tokens as u32;
        let out = Command::new("rustc")
            .args(["-O", "--edition", "2021", "-o"])
            .arg(&bin_path)
            .arg(&src_path)
            .output();
        let compiled = matches!(&out, Ok(o) if o.status.success());
        if let Ok(o) = &out {
            // Everything rustc printed is text the agent has to read.
            read += crate::tokens::count(&String::from_utf8_lossy(&o.stderr)).tokens as u32;
        }
        if !compiled {
            last = Some(format!("{}: rustc rejected it", cand.rust));
            continue;
        }
        match run_bin_capture(&bin_path) {
            Some(stdout) => {
                if stdout.trim() == task.expected.to_string() {
                    return LoopResult {
                        task_id: task.id.clone(),
                        arm: "rust".into(),
                        attempts,
                        probes: 0,
                        green: true,
                        wall_ms: t0.elapsed().as_secs_f64() * 1000.0,
                        tokens_est: estimate_tokens(&src),
                        tokens_written: written,
                        tokens_read: read,
                        last_error: None,
                    };
                }
                last = Some(format!("{} produced {}", cand.rust, stdout.trim()));
            }
            None => last = Some(format!("{}: binary failed", cand.rust)),
        }
    }
    LoopResult {
        task_id: task.id.clone(),
        arm: "rust".into(),
        attempts,
        probes: 0,
        green: false,
        wall_ms: t0.elapsed().as_secs_f64() * 1000.0,
        tokens_est: estimate_tokens(&task.rust_template),
        tokens_written: written,
        tokens_read: read,
        last_error: last,
    }
}

fn run_bin_capture(p: &PathBuf) -> Option<String> {
    let out = Command::new(p).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn estimate_tokens(src: &str) -> u32 {
    src.split_whitespace().count() as u32
}

fn median(mut xs: Vec<f64>) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = xs.len();
    if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    }
}

pub fn run_eval_loop(seed: u64, n: usize, max_attempts: u32) -> LoopReport {
    let tasks = generate_hidden(seed, n);
    let have_rustc = rustc_available();
    let mut ax = Vec::new();
    let mut rust = Vec::new();
    for t in &tasks {
        ax.push(run_ax_loop(t, max_attempts));
        if have_rustc {
            rust.push(run_rust_loop(t, max_attempts));
        }
    }
    LoopReport {
        seed,
        n,
        ax_median_attempts: median(ax.iter().map(|r| r.attempts as f64).collect()),
        rust_median_attempts: median(rust.iter().map(|r| r.attempts as f64).collect()),
        ax_median_wall_ms: median(ax.iter().map(|r| r.wall_ms).collect()),
        rust_median_wall_ms: median(rust.iter().map(|r| r.wall_ms).collect()),
        ax_pass: ax.iter().filter(|r| r.green).count(),
        rust_pass: rust.iter().filter(|r| r.green).count(),
        ax_median_tokens: median(
            ax.iter()
                .map(|r| (r.tokens_written + r.tokens_read) as f64)
                .collect(),
        ),
        rust_median_tokens: median(
            rust.iter()
                .map(|r| (r.tokens_written + r.tokens_read) as f64)
                .collect(),
        ),
        rust_skipped: !have_rustc,
        ax,
        rust,
    }
}

/// Attempts-to-green for one real file with holes: the metric an agent working
/// on an actual module cares about.
#[derive(Clone, Debug, Serialize)]
pub struct FileAttempts {
    pub path: String,
    pub holes: usize,
    /// Fills tried, in rank order, until the module's tests passed.
    pub attempts: u32,
    pub probes: u32,
    pub green: bool,
    pub wall_ms: f64,
    pub applied: Vec<String>,
}

/// Fill every hole in `src` from the ranked, pre-verified candidates, taking the
/// first fill per hole that leaves the module green.
pub fn attempts_to_green(name: &str, src: &str, surface: Surface) -> FileAttempts {
    let t0 = Instant::now();
    let mut cur = src.to_string();
    let mut attempts = 0u32;
    let mut probes = 0u32;
    let mut applied = Vec::new();

    loop {
        let holes = agent::hole_fills(name, &cur, surface, 32);
        probes += 1;
        let Some(h) = holes.first() else { break };
        let mut filled = false;
        for f in h.fills.iter().filter(|f| f.compiles) {
            let patched = replace_at(&cur, h.span.0, h.span.1, &f.expr);
            attempts += 1;
            let mut s = Session::new();
            s.surface = surface;
            if let Ok(out) = s.compile(name, &patched) {
                let results = crate::driver::run_tests(&s.intern, &out, 0);
                // No tests means the fill only has to typecheck.
                if results.is_empty() || results.iter().all(|r| r.ok) {
                    cur = patched;
                    applied.push(f.expr.clone());
                    filled = true;
                    break;
                }
            }
        }
        if !filled {
            return FileAttempts {
                path: name.to_string(),
                holes: applied.len(),
                attempts,
                probes,
                green: false,
                wall_ms: t0.elapsed().as_secs_f64() * 1000.0,
                applied,
            };
        }
    }

    let mut s = Session::new();
    s.surface = surface;
    let green = match s.compile(name, &cur) {
        Ok(out) => {
            let results = crate::driver::run_tests(&s.intern, &out, 0);
            results.is_empty() || results.iter().all(|r| r.ok)
        }
        Err(_) => false,
    };
    FileAttempts {
        path: name.to_string(),
        holes: applied.len(),
        attempts,
        probes,
        green,
        wall_ms: t0.elapsed().as_secs_f64() * 1000.0,
        applied,
    }
}

fn replace_at(src: &str, start: u32, end: u32, with: &str) -> String {
    let (s, e) = (start as usize, end as usize);
    if e > src.len() || s > e {
        return src.to_string();
    }
    format!("{}{}{}", &src[..s], with, &src[e..])
}
