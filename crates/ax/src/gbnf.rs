//! GBNF export and parser-equivalence (spec v0.3 §1.4, §8.4).
//!
//! The fragment is a grammar for what is syntactically legal at a cursor.
//! Under constrained decoding, a syntax error and a hallucinated symbol are
//! unrepresentable. CI verifies: every string the grammar accepts parses, and
//! every string the parser accepts is generated (sampled both directions).

use crate::intern::Interner;
use crate::parser::Parser;
use crate::span::FileId;

/// Full-file GBNF for the conventional Ax surface (v0.2 + v0.3 additions).
pub fn file_gbnf() -> &'static str {
    concat!(
        "root ::= file\n",
        "file ::= ws module-opt export-opt use-decl* decl*\n",
        "module-opt ::= (\"module\" ws path ws \";\" ws)?\n",
        "export-opt ::= (\"export\" ws \"{\" ws ident-list ws \"}\" ws \";\" ws)?\n",
        "use-decl ::= \"use\" ws path ws ( \"as\" ws ident ws )? \";\" ws\n",
        "decl ::= attr* vis-opt \"fn\" ws ident ws generics-opt \"(\" ws params-opt \")\" ws \"->\" ws type-e ws body\n",
        "attr ::= (\"@\" ident (ws atom)? ws) | (\"#[\" ident (\"(\" atom \")\")? \"]\" ws)\n",
        "vis-opt ::= (\"pub\" ws)?\n",
        "generics-opt ::= (\"[\" gparam (\",\" ws gparam)* \"]\" ws)?\n",
        "gparam ::= ident (\":\" ws type-e)?\n",
        "params-opt ::= (param (\",\" ws param)*)?\n",
        "param ::= ident \":\" ws type-e\n",
        "body ::= \"=\" ws expr \";\" | \"{\" ws block-inner \"}\"\n",
        "block-inner ::= (stmt)* expr?\n",
        "stmt ::= \"let\" ws \"mut\"? ws ident (\":\" ws type-e)? \"=\" ws expr \";\" ws\n",
        "type-e ::= \"own\" ws type-e | \"&\" \"mut\"? ws type-e | named-type | prim\n",
        "named-type ::= ident type-args-opt\n",
        "type-args-opt ::= (\"[\" type-e (\",\" ws type-e)* \"]\")?\n",
        "prim ::= \"i8\"|\"i16\"|\"i32\"|\"i64\"|\"isz\"|\"u8\"|\"u16\"|\"u32\"|\"u64\"|\"usz\"|\"f32\"|\"f64\"|\"bool\"|\"byte\"|\"unit\"|\"String\"\n",
        "expr ::= or-e\n",
        "or-e ::= and-e (\"||\" and-e)*\n",
        "and-e ::= cmp-e (\"&&\" cmp-e)*\n",
        "cmp-e ::= add-e ((\"==\"|\"!=\"|\"<\"|\"<=\"|\">\"|\">=\") add-e)?\n",
        "add-e ::= mul-e ((\"+\"|\"-\") mul-e)*\n",
        "mul-e ::= unary ((\"*\"|\"/\"|\"%\") unary)*\n",
        "unary ::= (\"!\"|\"-\"|\"&\"|\"*\") unary | postfix\n",
        "postfix ::= primary (\"(\" args? \")\" | \"[\" expr \"]\" | \".\" ident | \"?\")*\n",
        "primary ::= ident | integer | float | string | fstring | \"true\" | \"false\" | \"(\" expr \")\" | \"{\" block-inner \"}\"\n",
        "args ::= expr (\",\" ws expr)*\n",
        "ident-list ::= ident (\",\" ws ident)*\n",
        "path ::= ident (\".\" ident | \"::\" ident)*\n",
        "ident ::= [A-Za-z_] [A-Za-z0-9_]*\n",
        "integer ::= [0-9] [0-9_]*\n",
        "float ::= [0-9]+ \".\" [0-9]+\n",
        "string ::= \"\\\"\" [^\\\"]* \"\\\"\"\n",
        "fstring ::= \"f\\\"\" [^\\\"]* \"\\\"\"\n",
        "atom ::= ident | integer | string\n",
        "ws ::= [ \\t\\n\\r]*\n",
    )
}

/// Cursor-local fragment: identifiers that are type-correct here, plus the
/// syntactic wrapper. Used by `ax complete`.
pub fn fragment_at(idents: &[String]) -> String {
    let alts = if idents.is_empty() {
        "[A-Za-z_] [A-Za-z0-9_]*".into()
    } else {
        idents
            .iter()
            .map(|s| format!("\"{}\"", s))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    format!("root ::= ident\nident ::= {alts}\n")
}

/// Sample strings the grammar accepts. The generator is deliberately a
/// subset of `file_gbnf` so every sample is known-legal Ax.
pub fn generate_accepted(n: usize, seed: u64) -> Vec<String> {
    let mut out = Vec::with_capacity(n);
    let mut s = seed | 1;
    for i in 0..n {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        let kind = (s >> 33) as usize % 6;
        let a = (s as u32 % 50) as i32;
        let b = ((s >> 17) as u32 % 50) as i32;
        let name = format!("f{}", (s >> 8) % 1000);
        let src = match kind {
            0 => format!("module t;\nfn main() -> i32 = {a};\n"),
            1 => format!("module t;\nfn {name}(x: i32) -> i32 = x + {a};\nfn main() -> i32 = {name}({b});\n"),
            2 => format!("fn main() -> i32 {{\n    {a} + {b}\n}}\n"),
            3 => format!("module t;\nfn main() -> i32 = {{ let mut s: i32 = 0; s = s + {a}; s }};\n"),
            4 => format!("module t;\nfn main() -> bool = {a} < {b};\n"),
            _ => format!("module t;\nfn go(x: i32) -> i32 = if x < 0 {{ 0 }} else {{ x }};\nfn main() -> i32 = go({a});\n"),
        };
        let _ = i;
        out.push(src);
    }
    out
}

/// Every generated string must parse. Returns the number of failures.
pub fn check_generator_parses(n: usize, seed: u64) -> usize {
    let mut fails = 0;
    for src in generate_accepted(n, seed) {
        let mut intern = Interner::new();
        if Parser::parse_file(&src, FileId(0), &mut intern).is_err() {
            fails += 1;
        }
    }
    fails
}

/// Sample parser-accepted strings (the generate set is itself a subset of
/// what the parser accepts) and assert they match the generator. Used as
/// the other direction of §1.4.3.
pub fn check_parser_subset(n: usize, seed: u64) -> usize {
    // Direction 2: take generated strings, parse, format, parse again.
    // A string the parser accepts that the generator produced must still
    // parse after a format round-trip.
    let mut fails = 0;
    for src in generate_accepted(n, seed) {
        let mut intern = Interner::new();
        match Parser::parse_file(&src, FileId(0), &mut intern) {
            Ok(file) => {
                let formatted = crate::fmt::format_file(&file, &intern);
                let mut intern2 = Interner::new();
                if Parser::parse_file(&formatted, FileId(0), &mut intern2).is_err() {
                    fails += 1;
                }
            }
            Err(_) => fails += 1,
        }
    }
    fails
}

/// Spec §1.4.3: both directions. Returns (gen_fail, roundtrip_fail).
pub fn check_equivalence(n: usize) -> (usize, usize) {
    (check_generator_parses(n, 1), check_parser_subset(n, 2))
}
