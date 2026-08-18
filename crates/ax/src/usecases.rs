//! Basic-use-case snippets: same task in Rust / C / Go / Ax, counted
//! with the documented proxy tokenizer.

use crate::tokens;

pub struct LangSrc {
    pub rust: &'static str,
    pub c: &'static str,
    pub go: &'static str,
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
        what: "A named function that returns `a + b`.",
        src: LangSrc {
            rust: "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
            c: "int add(int a, int b) {\n    return a + b;\n}\n",
            go: "func add(a int32, b int32) int32 {\n    return a + b\n}\n",
            ax: "fn add(a: i32, b: i32) -> i32 = a + b;\n",
        },
    },
    Case {
        id: "if_else",
        title: "If / else",
        what: "Pick one of two integers from a boolean.",
        src: LangSrc {
            rust: "fn pick(b: bool) -> i32 {\n    if b { 1 } else { 0 }\n}\n",
            c: "int pick(int b) {\n    if (b) return 1;\n    return 0;\n}\n",
            go: "func pick(b bool) int32 {\n    if b {\n        return 1\n    }\n    return 0\n}\n",
            ax: "fn pick(b: bool) -> i32 = if b { 1 } else { 0 };\n",
        },
    },
    Case {
        id: "sum_range",
        title: "Sum a range",
        what: "Accumulate `0 + 1 + … + (n-1)`.",
        src: LangSrc {
            rust: "fn sum(n: u64) -> u64 {\n    let mut s = 0u64;\n    let mut i = 0u64;\n    while i < n {\n        s = s.wrapping_add(i);\n        i += 1;\n    }\n    s\n}\n",
            c: "uint64_t sum(uint64_t n) {\n    uint64_t s = 0;\n    for (uint64_t i = 0; i < n; i++) s += i;\n    return s;\n}\n",
            go: "func sum(n uint64) uint64 {\n    var s uint64\n    var i uint64\n    for i < n {\n        s += i\n        i++\n    }\n    return s\n}\n",
            ax: "fn sum(n: usz) -> usz = {\n    let mut s: usz = 0;\n    for i in range(0, n) { s = s + i; };\n    s\n};\n",
        },
    },
    Case {
        id: "rec",
        title: "Recursion",
        what: "Factorial by a single recursive call.",
        src: LangSrc {
            rust: "fn fac(n: i32) -> i32 {\n    if n < 2 { 1 } else { n * fac(n - 1) }\n}\n",
            c: "int fac(int n) {\n    if (n < 2) return 1;\n    return n * fac(n - 1);\n}\n",
            go: "func fac(n int32) int32 {\n    if n < 2 {\n        return 1\n    }\n    return n * fac(n-1)\n}\n",
            ax: "fn fac(n: i32) -> i32 = if n < 2 { 1 } else { n * fac(n - 1) };\n",
        },
    },
    Case {
        id: "option",
        title: "Option unwrap-or",
        what: "Read a map entry, default to `0` when missing.",
        src: LangSrc {
            rust: "fn get(m: &HashMap<String, i64>, k: &str) -> i64 {\n    match m.get(k) {\n        Some(v) => *v,\n        None => 0,\n    }\n}\n",
            c: "int64_t get(AxMap *m, const AxStr *k) {\n    int64_t v = 0;\n    if (!ax_rt_map_get(m, k, &v)) return 0;\n    return v;\n}\n",
            go: "func get(m map[string]int64, k string) int64 {\n    if v, ok := m[k]; ok {\n        return v\n    }\n    return 0\n}\n",
            ax: "fn get(m: Map[String, i64], k: String) -> i64 =\n    match m.get(k) { Some(v) => v; None => 0; };\n",
        },
    },
    Case {
        id: "map_build",
        title: "Map insert + get",
        what: "Allocate a map, insert two keys, return their sum.",
        src: LangSrc {
            rust: "fn main() {\n    let mut m = HashMap::new();\n    m.insert(\"e\".into(), 2i64);\n    m.insert(\"o\".into(), 3i64);\n    let e = *m.get(\"e\").unwrap_or(&0);\n    let o = *m.get(\"o\").unwrap_or(&0);\n    println!(\"{}\", e + o);\n}\n",
            c: "int main(void) {\n    AxMap *m = ax_rt_map_new();\n    AxStr e = {\"e\", 1}, o = {\"o\", 1};\n    ax_rt_map_insert(m, &e, 2);\n    ax_rt_map_insert(m, &o, 3);\n    int64_t ev = 0, ov = 0;\n    ax_rt_map_get(m, &e, &ev);\n    ax_rt_map_get(m, &o, &ov);\n    printf(\"%lld\\n\", (long long)(ev + ov));\n    return 0;\n}\n",
            go: "func main() {\n    m := map[string]int64{}\n    m[\"e\"] = 2\n    m[\"o\"] = 3\n    fmt.Println(m[\"e\"] + m[\"o\"])\n}\n",
            ax: "fn main() -> i64 !{alloc[a]} = {\n    let mut m: Map[String, i64] = map.new(test.alloc);\n    m.insert(\"e\", 2i64);\n    m.insert(\"o\", 3i64);\n    let e = match m.get(\"e\") { Some(v) => v; None => 0; };\n    let o = match m.get(\"o\") { Some(v) => v; None => 0; };\n    e + o\n};\n",
        },
    },
    Case {
        id: "error",
        title: "Fallible parse",
        what: "Parse an integer; on failure return a default.",
        src: LangSrc {
            rust: "fn parse_or(s: &str) -> i32 {\n    s.parse::<i32>().unwrap_or(0)\n}\n",
            c: "int parse_or(const char *s) {\n    char *end = NULL;\n    long v = strtol(s, &end, 10);\n    if (end == s) return 0;\n    return (int)v;\n}\n",
            go: "func parseOr(s string) int32 {\n    v, err := strconv.Atoi(s)\n    if err != nil {\n        return 0\n    }\n    return int32(v)\n}\n",
            ax: "fn parse_or(s: String) -> i32 !{err[ParseError]} =\n    match parse_i32(s) { Ok(v) => v; Err(_) => 0; };\n",
        },
    },
    Case {
        id: "interp",
        title: "String interpolation",
        what: "Build `hello {name}` without a format macro.",
        src: LangSrc {
            rust: "fn greet(name: &str) -> String {\n    format!(\"hello {name}\")\n}\n",
            c: "void greet(const char *name, char *out, size_t n) {\n    snprintf(out, n, \"hello %s\", name);\n}\n",
            go: "func greet(name string) string {\n    return fmt.Sprintf(\"hello %s\", name)\n}\n",
            ax: "fn greet(name: String) -> String = f\"hello {name}\";\n",
        },
    },
];

fn fence(lang: &str, src: &str) -> String {
    format!("```{lang}\n{}```", src.trim_start_matches('\n'))
}

/// Markdown report: syntax + proxy-token counts, same counter as `ax bench tokens`.
pub fn render_doc() -> String {
    let mut md = String::from(
        "# Syntax & token cost — basic use cases\n\n\
         Same task in Rust, C, Go, and Ax. Token counts use the **in-repo proxy** \
         (`ax::tokens`): letters split at `_` / case / digit boundaries; each \
         punctuation mark is one token except two-char operators (`->`, `:=`, `==` …); \
         a newline is one token; spaces are free. Absolute numbers are not any \
         particular model’s vocabulary; the comparison is identical across languages.\n\n\
         **Ax** is the language (short syntax). Conventional is the corpus \
         dialect, shown only so the same idea can be compared with Rust / C / Go. \
         `ax fmt` prints Ax.\n\n",
    );

    md.push_str("## Summary\n\n");
    md.push_str("| case | rust | c | go | ax-conv | ax | ax/rust |\n");
    md.push_str("|---|---:|---:|---:|---:|---:|---:|\n");

    let mut tot = [0usize; 5];
    let mut rows: Vec<(String, [usize; 5])> = Vec::new();
    for c in CASES {
        let dense = crate::frontend::to_dense(c.src.ax);
        let counts = [
            tokens::count(c.src.rust).tokens,
            tokens::count(c.src.c).tokens,
            tokens::count(c.src.go).tokens,
            tokens::count(c.src.ax).tokens,
            tokens::count(&dense).tokens,
        ];
        for i in 0..5 {
            tot[i] += counts[i];
        }
        rows.push((c.title.to_string(), counts));
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {:.2}× |\n",
            c.title,
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            counts[4],
            counts[4] as f64 / counts[0].max(1) as f64,
        ));
    }
    md.push_str(&format!(
        "| **total** | {} | {} | {} | {} | {} | {:.2}× |\n\n",
        tot[0],
        tot[1],
        tot[2],
        tot[3],
        tot[4],
        tot[4] as f64 / tot[0].max(1) as f64,
    ));

    md.push_str(
        "Read the totals as *how much text an agent pays to write the same idea*. \
         **Ax** is the short syntax (the language). Conventional is the corpus \
         dialect, kept so the same idea can be compared with Rust / C / Go.\n\n",
    );

    for c in CASES {
        let dense = crate::frontend::to_dense(c.src.ax);
        let n = |s: &str| tokens::count(s).tokens;
        md.push_str(&format!("## {}\n\n{}\n\n", c.title, c.what));
        md.push_str(&format!(
            "| rust | c | go | ax-conv | ax |\n|---:|---:|---:|---:|---:|\n| {} | {} | {} | {} | {} |\n\n",
            n(c.src.rust),
            n(c.src.c),
            n(c.src.go),
            n(c.src.ax),
            n(&dense),
        ));
        md.push_str("**Rust**\n\n");
        md.push_str(&fence("rust", c.src.rust));
        md.push_str("\n\n**C**\n\n");
        md.push_str(&fence("c", c.src.c));
        md.push_str("\n\n**Go**\n\n");
        md.push_str(&fence("go", c.src.go));
        md.push_str("\n\n**Ax (corpus / conventional)**\n\n");
        md.push_str(&fence("ax", c.src.ax));
        md.push_str("\n\n**Ax**\n\n");
        md.push_str(&fence("ax", &format!("{dense}\n")));
        md.push('\n');
    }

    md.push_str(
        "## How to regenerate\n\n\
         ```\n\
         ax bench usecases\n\
         ```\n\n\
         Writes this file from `ax::usecases` so the numbers cannot drift from \
         the tokenizer. Ax snippets are complete enough to type-check as a \
         module body (wrap in `module t; export { main };` plus a `main` if needed).\n",
    );
    md
}

pub fn write_doc(path: &std::path::Path) -> Result<(), String> {
    std::fs::write(path, render_doc()).map_err(|e| e.to_string())
}
