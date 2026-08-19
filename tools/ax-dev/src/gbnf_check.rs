//! Development-only GBNF generator/parser equivalence checks.

use ax_core::intern::Interner;
use ax_core::parser::Parser;
use ax_core::span::FileId;

pub fn generate_accepted(n: usize, seed: u64) -> Vec<String> {
    let mut out = Vec::with_capacity(n);
    let mut state = seed | 1;
    for _ in 0..n {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let kind = (state >> 33) as usize % 6;
        let a = (state as u32 % 50) as i32;
        let b = ((state >> 17) as u32 % 50) as i32;
        let name = format!("f{}", (state >> 8) % 1000);
        out.push(match kind {
            0 => format!("module t;\nfn main() -> i32 = {a};\n"),
            1 => format!(
                "module t;\nfn {name}(x: i32) -> i32 = x + {a};\nfn main() -> i32 = {name}({b});\n"
            ),
            2 => format!("fn main() -> i32 {{\n    {a} + {b}\n}}\n"),
            3 => format!(
                "module t;\nfn main() -> i32 = {{ let mut s: i32 = 0; s = s + {a}; s }};\n"
            ),
            4 => format!("module t;\nfn main() -> bool = {a} < {b};\n"),
            _ => format!(
                "module t;\nfn go(x: i32) -> i32 = if x < 0 {{ 0 }} else {{ x }};\nfn main() -> i32 = go({a});\n"
            ),
        });
    }
    out
}

pub fn check_generator_parses(n: usize, seed: u64) -> usize {
    generate_accepted(n, seed)
        .into_iter()
        .filter(|source| {
            let mut intern = Interner::new();
            Parser::parse_file(source, FileId(0), &mut intern).is_err()
        })
        .count()
}

pub fn check_parser_subset(n: usize, seed: u64) -> usize {
    generate_accepted(n, seed)
        .into_iter()
        .filter(|source| {
            let mut intern = Interner::new();
            let Ok(file) = Parser::parse_file(source, FileId(0), &mut intern) else {
                return true;
            };
            let formatted = ax_core::fmt::format_file(&file, &intern);
            let mut second = Interner::new();
            Parser::parse_file(&formatted, FileId(0), &mut second).is_err()
        })
        .count()
}

pub fn check_equivalence(n: usize) -> (usize, usize) {
    (check_generator_parses(n, 1), check_parser_subset(n, 2))
}
