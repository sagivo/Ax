//! Token accounting for source text.
//!
//! This is the historical proxy used by protocol experiments. Exact model-BPE
//! measurement is development tooling and lives in `tools/ax-density`, keeping
//! model tokenizer dependencies out of the shipped compiler.
//!
//! Proxy rules, chosen to mirror what byte-level BPE vocabularies actually do
//! with source code:
//!
//! - a run of letters/digits/underscore is one token, except that it splits at
//!   `_` and at lower→upper case transitions (`read_file` → `read`, `_`, `file`;
//!   `TypeName` → `Type`, `Name`), because subword vocabularies split there;
//! - each punctuation character is one token, except that common two-character
//!   operators (`->`, `==`, `!=`, `<=`, `>=`, `&&`, `||`, `::`, `<<`, `>>`,
//!   `=>`, `:=`, `+=`, `+/`, `*/`, …) are one, since they appear in vocabularies as units;
//! - whitespace is free except that a newline is one token, matching the usual
//!   treatment of indentation-bearing source;
//! - a character outside ASCII is one token, which is a floor;
//! - a comment counts, because the model pays for it too.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Count {
    pub bytes: usize,
    pub tokens: usize,
    pub lines: usize,
}

/// Count a source file under the proxy described above.
pub fn count(src: &str) -> Count {
    let b = src.as_bytes();
    let mut i = 0;
    let mut tokens = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            tokens += 1;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_alphanumeric() || c == b'_' {
            // One identifier run, split at underscores and case transitions.
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            tokens += subword_pieces(&src[start..i]);
            continue;
        }
        // A character outside ASCII counts as one token. A byte-level vocabulary
        // usually spends more than one on it, so this is a floor rather than an
        // estimate; it is applied identically to every language and source text is
        // almost entirely ASCII, so it does not move a comparison. Consuming the
        // whole UTF-8 sequence here is also what keeps the slice below on a
        // character boundary — indexing bytes into `src` otherwise panics.
        if !c.is_ascii() {
            tokens += 1;
            i += 1;
            while i < b.len() && b[i] & 0b1100_0000 == 0b1000_0000 {
                i += 1;
            }
            continue;
        }
        // Two-character operators that vocabularies carry as single units.
        if i + 1 < b.len() && b[i + 1].is_ascii() {
            let pair = &src[i..i + 2];
            if matches!(
                pair,
                "->" | "=="
                    | "!="
                    | "<="
                    | ">="
                    | "&&"
                    | "||"
                    | "::"
                    | "<<"
                    | ">>"
                    | "=>"
                    | ":="
                    | "+="
                    | "-="
                    | "*="
                    | "/="
                    | "%="
                    | "&="
                    | "|="
                    | "^="
                    | "+/"
                    | "*/"
                    | "|/"
                    | "&/"
                    | "++"
                    | "--"
                    | "??"
                    | "<-"
            ) {
                tokens += 1;
                i += 2;
                continue;
            }
        }
        tokens += 1;
        i += 1;
    }
    Count {
        bytes: src.len(),
        tokens,
        lines: src.lines().count(),
    }
}

/// How many pieces a subword vocabulary would likely split this word into.
fn subword_pieces(word: &str) -> usize {
    let mut pieces = 1;
    let chars: Vec<char> = word.chars().collect();
    for w in chars.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b == '_' {
            // The separator is its own piece, and what follows starts another.
            pieces += 1;
        } else if a == '_' {
            pieces += 1;
        } else if a.is_lowercase() && b.is_uppercase() {
            pieces += 1;
        } else if a.is_alphabetic() && b.is_ascii_digit() {
            pieces += 1;
        }
    }
    pieces
}

/// A comparison of the same program expressed in several languages.
#[derive(Clone, Debug, Serialize)]
pub struct Comparison {
    pub program: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub language: String,
    pub count: Count,
    /// Tokens relative to the conventional Ax surface.
    pub vs_ax: f64,
}

pub fn compare(program: &str, sources: &[(&str, &str)]) -> Comparison {
    let base = sources
        .iter()
        .find(|(l, _)| *l == "ax")
        .map(|(_, s)| count(s).tokens)
        .unwrap_or(1)
        .max(1);
    Comparison {
        program: program.to_string(),
        entries: sources
            .iter()
            .map(|(lang, src)| {
                let c = count(src);
                Entry {
                    language: (*lang).to_string(),
                    count: c,
                    vs_ax: c.tokens as f64 / base as f64,
                }
            })
            .collect(),
    }
}
