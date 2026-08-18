//! ax-mock: restricted-Rust prompt + validity check (spec §2.2, M12).
//!
//! The prompt instructs a model to write Rust without lifetimes, borrows,
//! clones, macros, or unsafe. Completions that parse as Ax after `repair`
//! count as valid prior transfer.

pub const PROMPT: &str = "\
Write Ax as if it were Rust, with these restrictions:
- no lifetimes, no &'a / &mut T as types (bare names are fine)
- no .clone(), Box/Rc/Arc/RefCell, unsafe, macros, async
- use Result/Option/? as in Rust
- integer overflow panics; use wrapping_* or prove the range
- f\"…\" for interpolation, not format!
Return only a complete .ax module.
";

pub fn validity(src: &str) -> bool {
    let repaired = crate::perf::repair("ax-mock.ax", src);
    repaired.clean
}

pub fn score_corpus(srcs: &[&str]) -> f64 {
    if srcs.is_empty() {
        return 0.0;
    }
    let ok = srcs.iter().filter(|s| validity(s)).count();
    ok as f64 / srcs.len() as f64
}
