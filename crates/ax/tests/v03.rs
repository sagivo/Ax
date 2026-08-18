//! v0.3 surface, ownership ladder, taint, affine `own`, and `ax perf`.

use ax::driver::Session;
use ax::parser::Parser;
use ax::span::FileId;

fn parse_ok(src: &str) {
    let mut intern = ax::Interner::new();
    Parser::parse_file(src, FileId(0), &mut intern).unwrap_or_else(|d| {
        panic!("parse failed: {d:?}");
    });
}

fn compile(src: &str) -> (Session, ax::check::CheckOutput) {
    let mut s = Session::new();
    match s.compile("t.ax", src) {
        Ok(o) => (s, o),
        Err(d) => panic!("compile failed:\n{d:#?}"),
    }
}

#[test]
fn rust_shaped_fn_body_parses() {
    parse_ok(
        r#"
fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#,
    );
}

#[test]
fn rust_struct_and_enum_parse() {
    parse_ok(
        r#"
struct Rec { id: i64, name: String }
enum LoadError { Io, Malformed, NegativeScore }
fn main() -> i32 { 0 }
"#,
    );
}

#[test]
fn fstring_and_try_parse() {
    parse_ok(
        r#"
fn go(name: String) -> String {
    let x = f"hello {name}";
    x
}
fn prop(r: Result[i32, ParseError]) -> i32 {
    r?
}
"#,
    );
}

#[test]
fn own_and_lattice_types_parse() {
    parse_ok(
        r#"
fn take(p: own i32) -> i32 { p }
fn wrap(x: Untrusted[i32]) -> Untrusted[i32] { x }
fn hide(x: Secret[i32]) -> Secret[i32] { x }
"#,
    );
}

#[test]
fn hash_attrs_parse() {
    parse_ok(
        r#"
#[no_alloc]
#[no_panic]
fn checksum(ids: i64) -> i64 { ids }
"#,
    );
}

#[test]
fn rust_use_colon_colon_parses() {
    parse_ok(
        r#"
use std::fs;
fn main() -> i32 { 0 }
"#,
    );
}

#[test]
fn v02_sources_still_parse() {
    parse_ok("module t;\nfn main() -> i32 = 1 + 2;\n");
}

#[test]
fn try_on_result_compiles() {
    let src = r#"
module t;
type E = | Bad;
fn f(b: bool) -> i32 !{err[E]} = if b { raise Bad } else { 3 };
fn go() -> i32 !{err[E]} = (attempt f(false))?;
fn main() -> i32 !{err[E]} = go();
"#;
    let (s, out) = compile(src);
    let v = ax::driver::run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.as_i128(), 3);
}

#[test]
fn interpolate_eval() {
    let src = r#"
module t;
fn main() -> String = {
    let name: String = "ax";
    f"hi {name}"
};
"#;
    let (s, out) = compile(src);
    let v = ax::driver::run_main(&s.intern, &out, 0).unwrap();
    assert!(v.display().contains("hi"));
}

#[test]
fn own_unused_is_a2021() {
    let src = r#"
module t;
fn take(p: own i32) -> i32 = 0;
fn main() -> i32 = 0;
"#;
    let mut s = Session::new();
    let file = s.parse("t.ax", src).unwrap();
    let out = s.check(&file);
    let (_own, errs) = ax::ownership::analyze(&s.intern, &out);
    assert!(
        errs.iter().any(|e| e.code == "A2021"),
        "expected A2021, got {errs:?}"
    );
}

#[test]
fn perf_report_has_schema_and_fixes() {
    let src = r#"
module t;
fn main() -> i32 = {
    let mut s: i32 = 0;
    for i in range(0, 10) { s = s + (i as i32); };
    s
};
"#;
    let (s, out) = compile(src);
    let r = ax::perf::analyze_module(&s.intern, &out, "t.ax");
    assert_eq!(r.schema_version, "1.0");
    for f in &r.functions {
        for find in &f.findings {
            assert!(
                !find.fixes.is_empty(),
                "finding {} has no fix",
                find.id
            );
        }
    }
}

#[test]
fn complete_returns_gbnf_and_names() {
    let src = "module t;\nfn add(a: i32, b: i32) -> i32 = a + b;\nfn main() -> i32 = add(1, 2);\n";
    let (s, out) = compile(src);
    let c = ax::perf::complete(&s.intern, &out);
    assert!(c.completions.iter().any(|x| x.name == "add"));
    assert!(c.gbnf_fragment.contains("ident"));
}

#[test]
fn context_pack_is_divergences() {
    let src = "module t;\nfn main() -> i32 = 1;\n";
    let (s, out) = compile(src);
    let p = ax::perf::context_pack(&s.intern, &out, 1000);
    assert!(p.cheatsheet.contains("no lifetimes"));
    assert!(p.tokens > 0);
}

#[test]
fn checksum_no_alloc_contract() {
    let src = r#"
module t;
fn checksum_no_alloc(x: i64) -> i64 = x * 31;
fn main() -> i64 = checksum_no_alloc(2);
"#;
    let (s, out) = compile(src);
    let r = ax::perf::analyze_module(&s.intern, &out, "t.ax");
    assert!(
        r.contracts.iter().any(|c| c.attribute == "no_alloc" && c.ok),
        "contracts: {:?}",
        r.contracts
    );
}
