//! Auditable basic-use-case corpus: the same task in TypeScript, Python, C,
//! Rust, and Ax, counted with the real model BPE vocabularies.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BpeEncoding {
    O200kBase,
    Cl100kBase,
}

fn count_bpe(src: &str, encoding: BpeEncoding) -> usize {
    match encoding {
        BpeEncoding::O200kBase => tiktoken_rs::o200k_base_singleton()
            .encode_with_special_tokens(src)
            .len(),
        BpeEncoding::Cl100kBase => tiktoken_rs::cl100k_base_singleton()
            .encode_with_special_tokens(src)
            .len(),
    }
}

pub struct LangSrc {
    pub typescript: &'static str,
    pub python: &'static str,
    pub c: &'static str,
    pub rust: &'static str,
    /// Conventional corpus form. The report mechanically packs this to Ax.
    pub ax: &'static str,
}

pub struct Case {
    pub id: &'static str,
    pub title: &'static str,
    pub what: &'static str,
    pub src: LangSrc,
}

pub fn cases() -> &'static [Case] {
    CASES
}

const CASES: &[Case] = &[
    Case {
        id: "add",
        title: "Add two integers",
        what: "Define a two-argument integer addition function.",
        src: LangSrc {
            typescript: "const add=(a:number,b:number):number=>a+b",
            python: "def add(a,b):return a+b",
            c: "int add(int a,int b){return a+b;}",
            rust: "fn add(a:i32,b:i32)->i32{a+b}",
            ax: "fn add(a: i32, b: i32) -> i32 = a + b;",
        },
    },
    Case {
        id: "if_else",
        title: "If / else",
        what: "Choose one of two integers from a boolean.",
        src: LangSrc {
            typescript: "const pick=(b:boolean):number=>b?1:0",
            python: "def pick(b):return 1 if b else 0",
            c: "int pick(int b){return b?1:0;}",
            rust: "fn pick(b:bool)->i32{if b{1}else{0}}",
            ax: "fn pick(b: bool) -> i32 = if b { 1 } else { 0 };",
        },
    },
    Case {
        id: "sum_range",
        title: "Sum a range",
        what: "Accumulate the integers from zero through n-1.",
        src: LangSrc {
            typescript: "function sum(n:number){let s=0;for(let i=0;i<n;i++)s+=i;return s}",
            python: "def sum_n(n):return sum(range(n))",
            c: "unsigned long sum(unsigned n){unsigned long s=0;for(unsigned i=0;i<n;i++)s+=i;return s;}",
            rust: "fn sum(n:usize)->usize{(0..n).sum()}",
            ax: "fn sum(n: usz) -> usz = { let mut s: usz = 0; for i in range(0, n) { s = s + i; }; s };",
        },
    },
    Case {
        id: "rec",
        title: "Recursion",
        what: "Define factorial with a single recursive call.",
        src: LangSrc {
            typescript: "const fac=(n:number):number=>n<2?1:n*fac(n-1)",
            python: "def fac(n):return 1 if n<2 else n*fac(n-1)",
            c: "int fac(int n){return n<2?1:n*fac(n-1);}",
            rust: "fn fac(n:i32)->i32{if n<2{1}else{n*fac(n-1)}}",
            ax: "fn fac(n: i32) -> i32 = if n < 2 { 1 } else { n * fac(n - 1) };",
        },
    },
    Case {
        id: "option",
        title: "Map get-or",
        what: "Read a map entry and return zero when the key is absent.",
        src: LangSrc {
            typescript: "const get=(m:Map<string,number>,k:string)=>m.get(k)??0",
            python: "def get(m,k):return m.get(k,0)",
            c: "long get(AxMap*m,AxStr*k){long v=0;ax_map_get(m,k,&v);return v;}",
            rust: "fn get(m:&HashMap<String,i64>,k:&str)->i64{*m.get(k).unwrap_or(&0)}",
            ax: "fn get(m: Map[String, i64], k: String) -> i64 = match m.get(k) { Some(v) => v; None => 0; };",
        },
    },
    Case {
        id: "map_build",
        title: "Map insert + get",
        what: "Allocate a map, insert two keys, and return their sum.",
        src: LangSrc {
            typescript: "function f(){const m=new Map<string,number>();m.set(\"e\",2);m.set(\"o\",3);return(m.get(\"e\")??0)+(m.get(\"o\")??0)}",
            python: "def f():m={\"e\":2,\"o\":3};return m.get(\"e\",0)+m.get(\"o\",0)",
            c: "long f(){AxMap*m=ax_map_new();ax_map_put(m,\"e\",2);ax_map_put(m,\"o\",3);return ax_map_get0(m,\"e\")+ax_map_get0(m,\"o\");}",
            rust: "fn f()->i64{let mut m=HashMap::new();m.insert(\"e\",2);m.insert(\"o\",3);m.get(\"e\").unwrap_or(&0)+m.get(\"o\").unwrap_or(&0)}",
            ax: "fn main() -> i64 !{alloc[a]} = { let mut m: Map[String, i64] = map.new(test.alloc); m.insert(\"e\", 2i64); m.insert(\"o\", 3i64); match m.get(\"e\") { Some(v) => v; None => 0; } + match m.get(\"o\") { Some(v) => v; None => 0; } };",
        },
    },
    Case {
        id: "error",
        title: "Fallible parse",
        what: "Parse an integer and return zero on failure.",
        src: LangSrc {
            typescript: "const parseOr=(s:string)=>Number.parseInt(s)||0",
            python: "def parse_or(s):\n try:return int(s)\n except ValueError:return 0",
            c: "int parse_or(char*s){char*e;long v=strtol(s,&e,10);return e==s?0:(int)v;}",
            rust: "fn parse_or(s:&str)->i32{s.parse().unwrap_or(0)}",
            ax: "fn parse_or(s: String) -> i32 = match attempt parse_i32(s) { Ok(v) => v; Err(_) => 0; };",
        },
    },
    Case {
        id: "interp",
        title: "String interpolation",
        what: "Build `hello {name}` with the language's standard interpolation.",
        src: LangSrc {
            typescript: "const greet=(name:string)=>`hello ${name}`",
            python: "def greet(name):return f\"hello {name}\"",
            c: "void greet(char*name,char*out,int n){snprintf(out,n,\"hello %s\",name);}",
            rust: "fn greet(name:&str)->String{format!(\"hello {name}\")}",
            ax: "fn greet(name: String) -> String = f\"hello {name}\";",
        },
    },
];

const LANGUAGES: [&str; 5] = ["TypeScript", "Python", "C", "Rust", "Ax"];

fn sources<'a>(case: &'a Case, dense: &'a str) -> [&'a str; 5] {
    [
        case.src.typescript,
        case.src.python,
        case.src.c,
        case.src.rust,
        dense,
    ]
}

fn encoding_name(encoding: BpeEncoding) -> &'static str {
    match encoding {
        BpeEncoding::O200kBase => "o200k_base",
        BpeEncoding::Cl100kBase => "cl100k_base",
    }
}

/// Corpus totals in the public table order. Used as a regression gate so a
/// syntax change cannot silently make Ax larger than the best comparison arm.
pub fn token_totals(encoding: BpeEncoding) -> [usize; 5] {
    let mut totals = [0usize; 5];
    for case in CASES {
        let dense = ax::frontend::to_dense(case.src.ax);
        let counts = sources(case, &dense).map(|src| count_bpe(src, encoding));
        for (total, count) in totals.iter_mut().zip(counts) {
            *total += count;
        }
    }
    totals
}

fn count_table(encoding: BpeEncoding) -> String {
    let mut md = format!(
        "| case | {} | Ax vs best mainstream |\n|---|{}|---:|\n",
        LANGUAGES.join(" | "),
        LANGUAGES.map(|_| "---:").join("|")
    );
    let totals = token_totals(encoding);
    for case in CASES {
        let dense = ax::frontend::to_dense(case.src.ax);
        let counts = sources(case, &dense).map(|src| count_bpe(src, encoding));
        let best = *counts[..4].iter().min().unwrap();
        let saving = (1.0 - counts[4] as f64 / best as f64) * 100.0;
        md.push_str(&format!(
            "| {} | {} | {:.0}% |\n",
            case.title,
            counts.map(|n| n.to_string()).join(" | "),
            saving
        ));
    }
    let best = *totals[..4].iter().min().unwrap();
    let saving = (1.0 - totals[4] as f64 / best as f64) * 100.0;
    md.push_str(&format!(
        "| **total** | {} | **{:.0}%** |\n",
        totals.map(|n| format!("**{n}**")).join(" | "),
        saving
    ));
    md
}

fn html_escape(src: &str) -> String {
    src.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
}

/// Markdown report generated from the same corpus the conformance test compiles.
pub fn render_doc() -> String {
    let mut md = String::from(
        "# Ax token efficiency — side-by-side\n\n\
         Compact implementations of the same eight tasks in TypeScript, Python, C, Rust, and Ax. \
         Counts are exact BPE counts, not character counts or a home-grown approximation.\n\n",
    );
    for encoding in [BpeEncoding::O200kBase, BpeEncoding::Cl100kBase] {
        md.push_str(&format!(
            "## {}\n\n{}\n",
            encoding_name(encoding),
            count_table(encoding)
        ));
    }
    md.push_str(
        "Lower is better. The percentage compares Ax with the smallest of the four mainstream \
         implementations on that row. A negative percentage means Ax lost that case.\n\n\
         ## Source, side by side\n\n",
    );

    for case in CASES {
        let dense = ax::frontend::to_dense(case.src.ax);
        let src = sources(case, &dense);
        let counts = src.map(|s| count_bpe(s, BpeEncoding::O200kBase));
        md.push_str(&format!("### {}\n\n{}\n\n", case.title, case.what));
        md.push_str(&format!(
            "| {} |\n|{}|\n",
            LANGUAGES.join(" | "),
            ["---"; 5].join("|")
        ));
        md.push('|');
        for i in 0..5 {
            md.push_str(&format!(
                " <pre>{}</pre><small>{} tokens</small> |",
                html_escape(src[i]),
                counts[i]
            ));
        }
        md.push_str("\n\n");
    }

    md.push_str(
        "## Method\n\n\
         - Primary encoding: `o200k_base`, used by current GPT, reasoning, and Codex model families.\n\
         - Regression encoding: `cl100k_base`, used by GPT-4-era models.\n\
         - Counter: `tiktoken-rs`, using the actual OpenAI vocabulary files.\n\
         - Scope: method bodies/signatures only. Imports, tests, comments, and executable wrappers are excluded for every language.\n\
         - Formatting: optional whitespace is removed in every language. Required Python indentation remains.\n\
         - Ax is generated mechanically from the conventional corpus form with `to_dense`, then compiled by the conformance test.\n\n\
         This is a controlled corpus, not proof that Ax is shortest for every possible program or tokenizer. \
         Cases live in `tools/ax-density/src/lib.rs`; counterexamples should be added there.\n\n\
         ## Regenerate\n\n```sh\ncargo run -p ax-density -- --write\n```\n",
    );
    md
}

pub fn write_doc(path: &std::path::Path) -> Result<(), String> {
    std::fs::write(path, render_doc()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ax::driver::Session;

    #[test]
    fn exact_bpe_corpus_gate_keeps_ax_smallest() {
        for encoding in [BpeEncoding::O200kBase, BpeEncoding::Cl100kBase] {
            let totals = token_totals(encoding);
            let best_mainstream = *totals[..4].iter().min().unwrap();
            assert!(
                totals[4] < best_mainstream,
                "Ax must remain smallest under {encoding:?}: {totals:?}"
            );
        }
    }

    #[test]
    fn compact_vocabulary_is_model_efficient() {
        assert_eq!(count_bpe("??", BpeEncoding::O200kBase), 1);
        assert_eq!(count_bpe("!a", BpeEncoding::Cl100kBase), 2);
    }

    #[test]
    fn ax_usecase_snippets_compile() {
        for case in cases() {
            let has_main = case.src.ax.contains("fn main(");
            let src = if has_main {
                format!("module t;\nexport {{ main }};\n{}", case.src.ax)
            } else {
                format!(
                    "module t;\nexport {{ main }};\n{}\nfn main() -> i32 = 0;\n",
                    case.src.ax
                )
            };
            let mut conventional = Session::new();
            conventional
                .compile(&format!("{}.ax", case.id), &src)
                .unwrap_or_else(|d| panic!("{}:\n{src}\n{d:?}", case.id));

            let dense = ax::frontend::to_dense(case.src.ax);
            let dense_src = if dense.contains("#main(") || dense.contains("fn main(") {
                format!("module t;\nexport {{ main }};\n{dense}\n")
            } else {
                format!("module t;\nexport {{ main }};\n{dense}\nfn main() -> i32 = 0;\n")
            };
            let mut packed = Session::new();
            packed.surface = ax::frontend::Surface::Dense;
            packed
                .compile(&format!("{}_dense.ax", case.id), &dense_src)
                .unwrap_or_else(|d| panic!("{}:\n{dense_src}\n{d:?}", case.id));
        }
    }

    #[test]
    fn print_candidate_costs() {
        let candidates = [
            "#main()L!a={m:=%{\"e\":2L,\"o\":3L};m[\"e\"]?0+m[\"o\"]?0}",
            "#f()={m:%{\"e\":2,\"o\":3};m[\"e\"]?0+m[\"o\"]?0}",
            "#f()L={m:%{\"e\":2L,\"o\":3L};m[\"e\"]?0+m[\"o\"]?0}",
            "#f()!a={m:%{\"e\":2,\"o\":3};m[\"e\"]?0+m[\"o\"]?0}",
            "#f()={m:%{\"e\":2,\"o\":3};m[\"e\"]+m[\"o\"]}",
            "#f()={%{\"e\":2,\"o\":3}[\"e\"]?0+%{\"e\":2,\"o\":3}[\"o\"]?0}",
        ];
        for src in candidates {
            println!(
                "{} {} {:?}",
                count_bpe(src, BpeEncoding::O200kBase),
                count_bpe(src, BpeEncoding::Cl100kBase),
                src
            );
        }
        let src = "module t; export { main }; #main()={m:%{\"e\":2,\"o\":3};m[\"e\"]?0+m[\"o\"]?0}";
        let mut session = Session::new();
        session.surface = ax::frontend::Surface::Dense;
        println!("compile ok={}", session.compile("candidate.ax", src).is_ok());
    }
}
