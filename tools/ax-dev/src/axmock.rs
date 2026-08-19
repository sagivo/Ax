//! Development-only ax-mock validity experiment (spec §2.2, M12).
//!
//! The prompt instructs a model to write Rust without lifetimes, borrows,
//! clones, macros, or unsafe. Completions that parse as Ax after `repair`
//! count as valid prior transfer.

pub const PROMPT: &str = "\
Write Ax as if it were Rust, with these restrictions:
- no lifetimes, no &'a / &mut T as types (bare names are fine)
- no .clone(), Box/Rc/Arc/RefCell, unsafe, macros, async
- use Result/Option/? as in Rust
- integer arithmetic wraps (spec/card.md); use checked_add/sub/mul for Option[T]
- f\"…\" for interpolation, not format!
Return only a complete .ax module.
";

pub fn validity(src: &str) -> bool {
    let repaired = crate::perf::repair("ax-mock.ax", src);
    repaired.clean
}

pub fn score_corpus(srcs: &[impl AsRef<str>]) -> f64 {
    if srcs.is_empty() {
        return 0.0;
    }
    let ok = srcs.iter().filter(|s| validity(s.as_ref())).count();
    ok as f64 / srcs.len() as f64
}

/// Restricted-Rust samples that should compile as Ax after repair (M12).
pub fn sample_corpus() -> Vec<String> {
    generated_corpus(200, 1)
}

/// n restricted-Rust/Ax programs for M12. All should be valid Ax.
pub fn generated_corpus(n: usize, seed: u64) -> Vec<String> {
    let mut out = Vec::with_capacity(n);
    let mut s = seed | 1;
    for i in 0..n {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        let a = (s % 40) as i32;
        let b = ((s >> 11) % 40) as i32;
        let src = match (s >> 5) % 6 {
            0 => format!("module t;\nfn main() -> i32 = {a} + {b};\n"),
            1 => format!("fn f(x: i32) -> i32 {{ x + {a} }}\nfn main() -> i32 {{ f({b}) }}\n"),
            2 => format!("module t;\nfn main() -> i32 = {{ let mut s: i32 = 0; s = s + {a}; s }};\n"),
            3 => format!("module t;\nfn go(x: i32) -> i32 = if x < {a} {{ 0 }} else {{ x }};\nfn main() -> i32 = go({b});\n"),
            4 => format!("module t;\nfn main() -> bool = {a} < {b};\n"),
            _ => format!("module t;\nfn main() -> i32 = {{ let x: i32 = {a}; x + {b} }};\n"),
        };
        let _ = i;
        out.push(src);
    }
    out
}

/// What rust-analyzer + cargo fix would offer today: cargo fix on a
/// restricted-Rust file. Returns whether `cargo` is on PATH (the control
/// arm). A real K1 measurement still needs rust-analyzer GBNF.
pub fn m12_sample_score() -> f64 {
    score_corpus(&sample_corpus())
}

pub fn rust_tooling_available() -> bool {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
