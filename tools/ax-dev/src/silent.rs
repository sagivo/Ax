//! Development-only E2 silent-wrongness and tier-divergence experiment.
//!
//! **The question this answers.** `eval-loop` measures the *protocol* — how
//! cheaply an agent can find a program that works. It cannot separate the
//! protocol from the language, because a fast checker over Rust would score the
//! same. This harness measures the other axis: given a program an agent
//! plausibly writes, does the toolchain **catch** the defect, **crash** on it,
//! or **complete and hand back a wrong answer**? Only the last one is silent,
//! and only the last one an agent cannot detect without an oracle.
//!
//! Nothing here depends on `ax check` being fast, on holes, on ranked fills, or
//! on structured diagnostics. Anything Ax wins here, it wins because of its
//! *semantics*, which is exactly the claim `DECISIONS.md` K1 leaves open.
//!
//! **Three numbers, not one.**
//!
//! - `silent_rate` — accepted, ran to completion, violated the stated intent.
//! - `divergence_rate` — the *same source* produced different observable
//!   outcomes across the language's own tiers. An agent tests on one tier and
//!   ships another, so this is a correctness property, not a packaging detail.
//! - `mechanism_coverage` — for how many hazards does the language have *any*
//!   construct that could catch this class? This separates "the checker missed
//!   it" from "there is no checker for this", which are different arguments and
//!   should not be summed.
//!
//! **What this is not.** No model is involved, and the hazard corpus is written
//! by hand, so it measures what the languages do with a chosen set of defects —
//! not the frequency with which a model writes them. Family selection is the
//! whole experiment: a corpus of only effect-row and taint cases would report a
//! sweep for Ax and mean nothing. Families where Rust is equal or better are
//! included and labelled, and `control-sum` is a correct program both languages
//! must get right — a harness that cannot fail is not evidence.

use crate::codegen::{self, Tier};
use crate::driver::{run_main_with_args, Session};
use crate::reach::{self, CapBudget};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What the toolchain did with one source on one tier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Verdict {
    /// Refused before producing an artifact. The cheapest possible outcome.
    Rejected { code: String },
    /// Built, then died at run time. Loud and late: an agent sees it, but only
    /// on the inputs it happened to try.
    Crashed { note: String },
    /// Built, ran to completion, answer honours the intent.
    Right { got: String },
    /// Built, ran to completion, answer does not honour the intent, and nothing
    /// said so. This is the outcome that costs an agent a wrong belief.
    Wrong { got: String },
    /// The tier could not run here (no `cc`, no `rustc`, unimplemented). Scored
    /// as absent rather than as a pass.
    Unavailable { note: String },
}

impl Verdict {
    fn is_silent(&self) -> bool {
        matches!(self, Verdict::Wrong { .. })
    }
    /// Two tiers "agree" when an agent could not tell them apart from the
    /// outside: same class of outcome, and for completed runs the same value.
    fn observable(&self) -> String {
        match self {
            Verdict::Rejected { .. } => "rejected".into(),
            Verdict::Crashed { .. } => "crashed".into(),
            Verdict::Right { got } | Verdict::Wrong { got } => format!("value:{got}"),
            Verdict::Unavailable { .. } => "unavailable".into(),
        }
    }
}

/// What honouring the intent means for a hazard.
#[derive(Clone, Debug)]
pub enum Expect {
    /// The program is defective in a way the language ought to name. Any
    /// completed run is silent wrongness.
    Rejected,
    /// The program is well-formed and this is the answer.
    Value(&'static str),
}

/// One hazard: the same intent expressed in both languages.
pub struct Hazard {
    pub id: &'static str,
    pub family: &'static str,
    /// What the agent meant, in one line. The defect is the gap between this
    /// and what the program does.
    pub intent: &'static str,
    pub expect: Expect,
    pub ax_src: &'static str,
    pub rust_src: &'static str,
    pub argv: &'static [&'static str],
    /// `ax.toml` in force, when the hazard is about a declared budget.
    pub ax_toml: Option<&'static str>,
    /// Does Ax have any construct that could catch this class?
    pub ax_mechanism: bool,
    /// Does Rust? Answered from the language as it ships, not from a lint one
    /// could write.
    pub rust_mechanism: bool,
    /// Why the mechanism columns read the way they do.
    pub note: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct ArmResult {
    pub language: String,
    /// One entry per tier, in a fixed order.
    pub tiers: Vec<(String, Verdict)>,
    /// Collapsed verdict: what an agent would conclude from the tier it ran.
    pub silent: bool,
    pub divergent: bool,
    pub mechanism: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct HazardResult {
    pub id: String,
    pub family: String,
    pub intent: String,
    pub expect: String,
    pub note: String,
    pub ax: ArmResult,
    pub rust: ArmResult,
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub language: String,
    pub scored: usize,
    pub silent: usize,
    pub divergent: usize,
    pub mechanism: usize,
    pub silent_rate: f64,
    pub divergence_rate: f64,
    pub mechanism_coverage: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub hazards: Vec<HazardResult>,
    pub ax: Summary,
    pub rust: Summary,
    pub rustc_available: bool,
    pub cc_available: bool,
}

/// Strip an Ax type suffix so a value can be compared against Rust's `println!`.
/// `-2147483648i32` and `-2147483648` are the same answer.
fn normalize(v: &str) -> String {
    let t = v.trim();
    for suf in [
        "i8", "i16", "i32", "i64", "isz", "u8", "u16", "u32", "u64", "usz", "f32", "f64",
    ] {
        if let Some(base) = t.strip_suffix(suf) {
            if base.chars().any(|c| c.is_ascii_digit()) {
                return base.to_string();
            }
        }
    }
    t.to_string()
}

fn judge(expect: &Expect, got: &str) -> Verdict {
    let got = normalize(got);
    match expect {
        // A completed run is the defect: the language never named the problem.
        Expect::Rejected => Verdict::Wrong { got },
        Expect::Value(want) => {
            if got == normalize(want) {
                Verdict::Right { got }
            } else {
                Verdict::Wrong { got }
            }
        }
    }
}

/// The first error code, which is what an agent reads first.
fn first_error(diags: &[crate::diag::Diagnostic]) -> String {
    diags
        .iter()
        .find(|d| d.is_error())
        .map(|d| d.code.clone())
        .unwrap_or_else(|| "?".into())
}

/// Check a hazard's Ax source, applying the same `ax.toml` budget the CLI
/// applies (`main.rs` does this after `check`, so `Session::compile` alone
/// would miss A5001).
fn ax_check(
    s: &mut Session,
    name: &str,
    src: &str,
    ax_toml: Option<&str>,
) -> Result<crate::check::CheckOutput, Vec<crate::diag::Diagnostic>> {
    let file = match s.parse(name, src) {
        Ok(f) => f,
        Err(d) => return Err(d),
    };
    let mut out = s.check(&file);
    if let Some(toml) = ax_toml {
        let budget = CapBudget::from_toml(toml);
        let caps = reach::analyze(&s.intern, &out);
        for (cap, path) in budget.check(&caps) {
            out.diags.push(crate::diag::Diagnostic::error(
                "A5001",
                crate::span::Span::DUMMY,
                format!(
                    "capability `{cap}` is reachable from {} via {} but not permitted by ax.toml",
                    caps.from,
                    path.join(" → ")
                ),
            ));
        }
    }
    if out.diags.iter().any(|d| d.is_error()) {
        return Err(out.diags);
    }
    Ok(out)
}

fn run_ax(h: &Hazard, out_dir: &Path, cc: bool) -> ArmResult {
    let name = format!("{}.ax", h.id.replace('-', "_"));
    let mut s = Session::new();
    let checked = match ax_check(&mut s, &name, h.ax_src, h.ax_toml) {
        Err(diags) => {
            // A compile-time rejection is one answer for every tier: no
            // artifact exists to disagree about.
            let code = first_error(&diags);
            let v = Verdict::Rejected { code };
            return ArmResult {
                language: "ax".into(),
                tiers: ["oracle", "cranelift", "c-dev", "c-release"]
                    .iter()
                    .map(|t| (t.to_string(), v.clone()))
                    .collect(),
                silent: false,
                divergent: false,
                mechanism: h.ax_mechanism,
            };
        }
        Ok(o) => o,
    };

    // `argv(0)` is the program name. A native binary gets it from the OS, so the
    // in-process tiers have to be handed it explicitly or `argv(1)` comes back
    // empty and every hazard looks like a ParseError. `control-sum` exists to
    // catch exactly this kind of harness bug.
    let mut argv: Vec<String> = vec![name.clone()];
    argv.extend(h.argv.iter().map(|a| a.to_string()));
    let mut tiers: Vec<(String, Verdict)> = Vec::new();

    // Oracle: the normative answer.
    tiers.push((
        "oracle".into(),
        match run_main_with_args(&s.intern, &checked, 0, &argv) {
            Ok(v) => judge(&h.expect, &v.display()),
            Err(e) => Verdict::Crashed { note: e },
        },
    ));

    // Cranelift: reads the IR's own offsets.
    tiers.push((
        "cranelift".into(),
        if crate::backend_clif::available() {
            match crate::backend_clif::run_source(&s.intern, &checked, &argv) {
                Ok(v) => judge(&h.expect, &v),
                Err(e) => Verdict::Crashed { note: e },
            }
        } else {
            Verdict::Unavailable {
                note: "cranelift runtime needs cc".into(),
            }
        },
    ));

    // Both C tiers: the divergence question is whether -O0 and -O3 -DNDEBUG
    // can be told apart, which is the axis Rust loses on.
    for (label, tier) in [("c-dev", Tier::Dev), ("c-release", Tier::Release)] {
        let v = if !cc {
            Verdict::Unavailable {
                note: "no cc".into(),
            }
        } else {
            match codegen::build_tier(&s.intern, &checked, &name, out_dir, tier) {
                Err(e) => Verdict::Unavailable { note: e },
                Ok(b) => {
                    let refs: Vec<&str> = h.argv.to_vec();
                    match codegen::run_bin_args(&b.bin_path, &refs) {
                        Ok(v) => judge(&h.expect, &v),
                        Err(e) => Verdict::Crashed { note: e },
                    }
                }
            }
        };
        tiers.push((label.to_string(), v));
    }

    summarize("ax", tiers, h.ax_mechanism)
}

fn run_rust(h: &Hazard, out_dir: &Path, rustc: bool) -> ArmResult {
    if !rustc {
        return ArmResult {
            language: "rust".into(),
            tiers: ["debug", "release"]
                .iter()
                .map(|t| {
                    (
                        t.to_string(),
                        Verdict::Unavailable {
                            note: "no rustc".into(),
                        },
                    )
                })
                .collect(),
            silent: false,
            divergent: false,
            mechanism: h.rust_mechanism,
        };
    }
    let stem = h.id.replace('-', "_");
    let src_path = out_dir.join(format!("{stem}.rs"));
    let _ = std::fs::create_dir_all(out_dir);
    if std::fs::write(&src_path, h.rust_src).is_err() {
        return ArmResult {
            language: "rust".into(),
            tiers: vec![(
                "debug".into(),
                Verdict::Unavailable {
                    note: "write failed".into(),
                },
            )],
            silent: false,
            divergent: false,
            mechanism: h.rust_mechanism,
        };
    }

    let mut tiers: Vec<(String, Verdict)> = Vec::new();
    // `debug` and `release` differ in exactly one thing that matters here:
    // debug-assertions, which is what turns an overflow into a panic.
    for (label, opt) in [("debug", false), ("release", true)] {
        let bin = out_dir.join(format!("{stem}_{label}"));
        let mut cmd = Command::new("rustc");
        cmd.args(["--edition", "2021"]);
        if opt {
            cmd.arg("-O");
        }
        cmd.arg("-o").arg(&bin).arg(&src_path);
        let built = cmd.output();
        let v = match built {
            Err(e) => Verdict::Unavailable {
                note: format!("spawn rustc: {e}"),
            },
            Ok(o) if !o.status.success() => {
                // rustc has no error codes on every diagnostic; the `error[E…]`
                // tag when present is the closest analogue to an Ax code.
                let err = String::from_utf8_lossy(&o.stderr);
                let code = err
                    .split("error[")
                    .nth(1)
                    .and_then(|r| r.split(']').next())
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "rustc-error".into());
                Verdict::Rejected { code }
            }
            Ok(_) => match Command::new(&bin).args(h.argv).output() {
                Err(e) => Verdict::Crashed {
                    note: e.to_string(),
                },
                Ok(r) if !r.status.success() => Verdict::Crashed {
                    note: String::from_utf8_lossy(&r.stderr)
                        .lines()
                        .next()
                        .unwrap_or("panicked")
                        .to_string(),
                },
                Ok(r) => judge(&h.expect, &String::from_utf8_lossy(&r.stdout)),
            },
        };
        tiers.push((label.to_string(), v));
    }
    summarize("rust", tiers, h.rust_mechanism)
}

fn summarize(language: &str, tiers: Vec<(String, Verdict)>, mechanism: bool) -> ArmResult {
    let present: Vec<&Verdict> = tiers
        .iter()
        .map(|(_, v)| v)
        .filter(|v| !matches!(v, Verdict::Unavailable { .. }))
        .collect();
    let silent = present.iter().any(|v| v.is_silent());
    let mut obs: Vec<String> = present.iter().map(|v| v.observable()).collect();
    obs.sort();
    obs.dedup();
    ArmResult {
        language: language.into(),
        tiers,
        silent,
        divergent: obs.len() > 1,
        mechanism,
    }
}

fn summary(language: &str, arms: &[&ArmResult]) -> Summary {
    // A hazard with no tier available is not evidence either way.
    let scored: Vec<&&ArmResult> = arms
        .iter()
        .filter(|a| {
            a.tiers
                .iter()
                .any(|(_, v)| !matches!(v, Verdict::Unavailable { .. }))
        })
        .collect();
    let n = scored.len();
    let silent = scored.iter().filter(|a| a.silent).count();
    let divergent = scored.iter().filter(|a| a.divergent).count();
    let mechanism = scored.iter().filter(|a| a.mechanism).count();
    let rate = |k: usize| if n == 0 { 0.0 } else { k as f64 / n as f64 };
    Summary {
        language: language.into(),
        scored: n,
        silent,
        divergent,
        mechanism,
        silent_rate: rate(silent),
        divergence_rate: rate(divergent),
        mechanism_coverage: rate(mechanism),
    }
}

pub fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn out_dir() -> PathBuf {
    std::env::temp_dir().join("ax_silent")
}

pub fn run(filter: Option<&str>) -> Report {
    let dir = out_dir();
    let _ = std::fs::create_dir_all(&dir);
    let rustc = crate::evalloop::rustc_available();
    let cc = cc_available();

    let mut hazards = Vec::new();
    for h in corpus() {
        if let Some(f) = filter {
            if !h.id.contains(f) && !h.family.contains(f) {
                continue;
            }
        }
        let ax = run_ax(&h, &dir, cc);
        let rust = run_rust(&h, &dir, rustc);
        hazards.push(HazardResult {
            id: h.id.into(),
            family: h.family.into(),
            intent: h.intent.into(),
            expect: match h.expect {
                Expect::Rejected => "rejected".into(),
                Expect::Value(v) => format!("value:{v}"),
            },
            note: h.note.into(),
            ax,
            rust,
        });
    }
    let ax_arms: Vec<&ArmResult> = hazards.iter().map(|h| &h.ax).collect();
    let rust_arms: Vec<&ArmResult> = hazards.iter().map(|h| &h.rust).collect();
    Report {
        ax: summary("ax", &ax_arms),
        rust: summary("rust", &rust_arms),
        rustc_available: rustc,
        cc_available: cc,
        hazards,
    }
}

pub fn render(r: &Report) -> String {
    let mut s = String::new();
    s.push_str("E2 silent-wrongness (no model; hand-written hazard corpus)\n\n");
    if !r.rustc_available {
        s.push_str("  rustc absent — the rust arm is not measured\n\n");
    }
    if !r.cc_available {
        s.push_str("  cc absent — ax native tiers are not measured\n\n");
    }
    s.push_str(&format!(
        "{:<22} {:<11} {:<26} {:<26}\n",
        "hazard", "expect", "ax", "rust"
    ));
    for h in &r.hazards {
        let f = |a: &ArmResult| -> String {
            let head = a
                .tiers
                .iter()
                .find(|(_, v)| !matches!(v, Verdict::Unavailable { .. }))
                .map(|(_, v)| match v {
                    Verdict::Rejected { code } => format!("rejected {code}"),
                    Verdict::Crashed { .. } => "crashed".into(),
                    Verdict::Right { .. } => "right".into(),
                    Verdict::Wrong { got } => format!("WRONG {got}"),
                    Verdict::Unavailable { .. } => "n/a".into(),
                })
                .unwrap_or_else(|| "n/a".into());
            let mut tags = Vec::new();
            if a.divergent {
                tags.push("tier-divergent");
            }
            if !a.mechanism {
                tags.push("no mechanism");
            }
            if tags.is_empty() {
                head
            } else {
                format!("{head} [{}]", tags.join(", "))
            }
        };
        s.push_str(&format!(
            "{:<22} {:<11} {:<26} {:<26}\n",
            h.id,
            match h.expect.as_str() {
                "rejected" => "rejected",
                _ => "a value",
            },
            f(&h.ax),
            f(&h.rust)
        ));
    }
    s.push('\n');
    for sm in [&r.ax, &r.rust] {
        s.push_str(&format!(
            "{:<5} scored {:<3} silent {}/{} ({:.0}%)  tier-divergent {}/{} ({:.0}%)  has-mechanism {}/{} ({:.0}%)\n",
            sm.language,
            sm.scored,
            sm.silent,
            sm.scored,
            sm.silent_rate * 100.0,
            sm.divergent,
            sm.scored,
            sm.divergence_rate * 100.0,
            sm.mechanism,
            sm.scored,
            sm.mechanism_coverage * 100.0,
        ));
    }
    s.push_str(
        "\nsilent = accepted, ran to completion, violated the intent, nothing said so.\n\
         tier-divergent = one source, different observable outcomes across that language's own tiers.\n\
         has-mechanism = the language ships a construct that could catch this class at all.\n",
    );
    s
}

/// The hazard corpus. Families where Rust is equal or better are included on
/// purpose: a corpus that cannot embarrass Ax is not a measurement.
pub fn corpus() -> Vec<Hazard> {
    vec![
        // ---- control -------------------------------------------------------
        Hazard {
            id: "control-sum",
            family: "control",
            intent: "sum 1..=n for n from argv; n = 10 so the answer is 55",
            expect: Expect::Value("55"),
            ax_src: "module control_sum;\nexport { main };\nfn main() -> i32 !{io[argv], err[ParseError], diverge} = {\n    let n: i32 = parse_i32(argv(1));\n    let mut s: i32 = 0;\n    let mut i: i32 = 1;\n    while i <= n { s = s + i; i = i + 1 };\n    s\n};\n",
            rust_src: "fn main() {\n    let n: i32 = std::env::args().nth(1).unwrap().parse().unwrap();\n    let mut s: i32 = 0;\n    let mut i: i32 = 1;\n    while i <= n { s += i; i += 1; }\n    println!(\"{}\", s);\n}\n",
            argv: &["10"],
            ax_toml: None,
            ax_mechanism: true,
            rust_mechanism: true,
            note: "teeth check: a correct program both languages must get right",
        },
        // ---- arithmetic ----------------------------------------------------
        Hazard {
            id: "overflow-argv",
            family: "integer-overflow",
            intent: "add 1 to i32::MAX read from argv; the arithmetic answer is 2147483648",
            expect: Expect::Value("2147483648"),
            ax_src: "module overflow_argv;\nexport { main };\nfn main() -> i32 !{io[argv], err[ParseError], abort} = parse_i32(argv(1)) + 1;\n",
            rust_src: "fn main() {\n    let a: i32 = std::env::args().nth(1).unwrap().parse().unwrap();\n    println!(\"{}\", a + 1);\n}\n",
            argv: &["2147483647"],
            ax_toml: None,
            ax_mechanism: true,
            rust_mechanism: true,
            note: "both ship checked_add; neither rejects the unchecked form. ax wraps identically on all four tiers; rust panics in debug and wraps in release",
        },
        Hazard {
            id: "overflow-literal",
            family: "integer-overflow",
            intent: "i32::MAX + 1 with both operands constant",
            expect: Expect::Value("2147483648"),
            ax_src: "module overflow_literal;\nexport { main };\nfn main() -> i32 = 2147483647 + 1;\n",
            rust_src: "fn main() { let a: i32 = 2147483647; println!(\"{}\", a + 1); }\n",
            argv: &[],
            ax_toml: None,
            ax_mechanism: true,
            rust_mechanism: true,
            note: "RUST IS BETTER HERE: the deny-by-default arithmetic_overflow lint const-folds and rejects. ax accepts and wraps",
        },
        Hazard {
            id: "shift-width",
            family: "shift-overflow",
            intent: "shift 1 left by 40 in i32; the arithmetic answer is 1099511627776",
            expect: Expect::Value("1099511627776"),
            ax_src: "module shift_width;\nexport { main };\nfn main() -> i32 !{io[argv], err[ParseError], abort} = 1i32 << parse_i32(argv(1));\n",
            rust_src: "fn main() {\n    let n: u32 = std::env::args().nth(1).unwrap().parse().unwrap();\n    let a: i32 = 1;\n    println!(\"{}\", a << n);\n}\n",
            argv: &["40"],
            ax_toml: None,
            ax_mechanism: true,
            rust_mechanism: true,
            note: "ax masks the count to the operand width on every tier and says so in the card; rust panics in debug and masks in release",
        },
        // ---- conversions (expected parity) ---------------------------------
        Hazard {
            id: "narrow-implicit",
            family: "numeric-conversion",
            intent: "bind a usz to an i32 without saying so",
            expect: Expect::Rejected,
            ax_src: "module narrow_implicit;\nexport { main };\nfn main() -> i32 = { let x: i32 = 1usz; x };\n",
            rust_src: "fn main() { let y: usize = 1; let x: i32 = y; println!(\"{}\", x); }\n",
            argv: &[],
            ax_toml: None,
            ax_mechanism: true,
            rust_mechanism: true,
            note: "PARITY: ax E0108, rust E0308. neither language allows implicit numeric conversion",
        },
        Hazard {
            id: "narrow-explicit",
            family: "numeric-conversion",
            intent: "cast 4000000000 down to i32, which cannot hold it",
            expect: Expect::Value("4000000000"),
            ax_src: "module narrow_explicit;\nexport { main };\nfn main() -> i32 = 4000000000i64 as i32;\n",
            rust_src: "fn main() { let a: i64 = 4000000000; println!(\"{}\", a as i32); }\n",
            argv: &[],
            ax_toml: None,
            ax_mechanism: true,
            rust_mechanism: true,
            note: "PARITY: both truncate silently once the cast is written. `as` is an assertion in both languages",
        },
        // ---- failure modes in the interface --------------------------------
        Hazard {
            id: "div-zero-undeclared",
            family: "undeclared-failure",
            intent: "divide by a divisor from argv without declaring that it can fail",
            expect: Expect::Rejected,
            ax_src: "module div_zero_undeclared;\nexport { main };\nfn d(a: i32, b: i32) -> i32 = a / b;\nfn main() -> i32 !{io[argv], err[ParseError]} = d(6, parse_i32(argv(1)));\n",
            rust_src: "fn d(a: i32, b: i32) -> i32 { a / b }\nfn main() {\n    let b: i32 = std::env::args().nth(1).unwrap().parse().unwrap();\n    println!(\"{}\", d(6, b));\n}\n",
            argv: &["0"],
            ax_toml: None,
            ax_mechanism: true,
            rust_mechanism: false,
            note: "ax E0200 at check time: `/` carries err[DivError] unless the divisor is proven non-zero. rust has no way to state a fallible signature for `/`, so it panics at run time on the inputs that reach it",
        },
        Hazard {
            id: "effect-row-io",
            family: "effect-row",
            intent: "a function declared effect-free that reads a file",
            expect: Expect::Rejected,
            ax_src: "module effect_row_io;\nexport { main };\nfn tally() -> u64 !{} = io.bytesum_file(\"/etc/hosts\");\nfn main() -> u64 !{} = tally();\n",
            rust_src: "/// Pure: same value for the same arguments.\nfn tally() -> u64 {\n    std::fs::read(\"/etc/hosts\").map(|b| b.len() as u64).unwrap_or(0)\n}\nfn main() { println!(\"{}\", tally()); }\n",
            argv: &[],
            ax_toml: None,
            ax_mechanism: true,
            rust_mechanism: false,
            note: "AX ONLY: E0200 names io[fs] as absent from the declared row. rust has no effect row, so the doc comment is the only claim and nothing checks it",
        },
        Hazard {
            id: "termination-claim",
            family: "effect-row",
            intent: "a function whose signature claims it terminates, with an unbounded while",
            expect: Expect::Rejected,
            ax_src: "module termination_claim;\nexport { main };\nfn spin(n: i32) -> i32 !{} = { let mut x = n; while x > 0 { x = x - 1 }; x };\nfn main() -> i32 = spin(3);\n",
            rust_src: "fn spin(n: i32) -> i32 { let mut x = n; while x > 0 { x -= 1; } x }\nfn main() { println!(\"{}\", spin(3)); }\n",
            argv: &[],
            ax_toml: None,
            ax_mechanism: true,
            rust_mechanism: false,
            note: "AX ONLY: an explicit empty row is a termination claim and E0200 rejects it. rust cannot express the claim, so there is nothing to violate",
        },
        // ---- trust boundaries ----------------------------------------------
        Hazard {
            id: "taint-sink",
            family: "taint",
            intent: "interpolate file contents into a formatted string without declassifying",
            expect: Expect::Rejected,
            ax_src: "module taint_sink;\nexport { load };\nuse std.fs;\nfn load(cap: fs.ReadCap, a: Alloc, p: &str) -> String !{io[cap], alloc[a], err[fs.Error]}\n= { let body = fs.read(cap, a, p); f\"got {body}\" };\n",
            rust_src: "fn load(p: &str) -> String {\n    let body = std::fs::read_to_string(p).unwrap_or_default();\n    format!(\"got {body}\")\n}\nfn main() { println!(\"{}\", load(\"/etc/hosts\").len()); }\n",
            argv: &[],
            ax_toml: None,
            ax_mechanism: true,
            rust_mechanism: false,
            note: "AX ONLY: fs.read returns Untrusted[String] and A5101 stops it at the sink. rust's String carries no provenance, so the same program is unremarkable",
        },
        Hazard {
            id: "cap-budget",
            family: "capability",
            intent: "reach the filesystem from a module whose manifest permits only env",
            expect: Expect::Rejected,
            ax_src: "module cap_budget;\nexport { load };\nuse std.fs;\nfn load(cap: fs.ReadCap, a: Alloc) -> Untrusted[String] !{io[cap], alloc[a], err[fs.Error]}\n= fs.read(cap, a, \"d.json\");\n",
            rust_src: "fn load() -> String { std::fs::read_to_string(\"d.json\").unwrap_or_default() }\nfn main() { println!(\"{}\", load().len()); }\n",
            argv: &[],
            ax_toml: Some("[caps]\nallow = [\"env\"]\n"),
            ax_mechanism: true,
            rust_mechanism: false,
            note: "AX ONLY: A5001 decides from reachability and names the path load → fs.read. rust has no manifest-level capability concept; any dependency can open a file",
        },
    ]
}
