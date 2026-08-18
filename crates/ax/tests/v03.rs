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

#[test]
fn gbnf_generated_strings_parse() {
    let fails = ax::gbnf::check_generator_parses(200, 42);
    assert_eq!(fails, 0, "generator produced {fails} unparsable strings");
    let fails = ax::gbnf::check_parser_subset(200, 7);
    assert_eq!(fails, 0, "format round-trip failed on {fails} strings");
}

#[test]
fn translate_strips_rust_noise() {
    let rust = r#"
pub fn add<'a>(x: &'a i32, y: &'a i32) -> i32 {
    let z = Box::new(*x);
    z.clone() + *y
}
"#;
    let r = ax::translate::translate_rust(rust);
    assert!(!r.source.contains("Box::new"), "{}", r.source);
    assert!(!r.source.contains(".clone()"), "{}", r.source);
    assert!(!r.source.contains("'a"), "{}", r.source);
    assert!(r.notes.iter().any(|n| n.contains("lifetime") || n.contains("Box") || n.contains("clone")));
}

#[test]
fn translate_rejects_unknown_macros() {
    let rust = r#"fn f() { println!("hi"); todo!("no"); }"#;
    let r = ax::translate::translate_rust(rust);
    assert!(r.rejected.iter().any(|x| x.contains("todo")), "{:?}", r.rejected);
}

#[test]
fn caps_reports_shortest_path() {
    let src = r#"
module t;
fn inner() -> u64 !{io[fs], abort} = io.bytesum_file("x");
fn main() -> u64 !{io[fs], abort} = inner();
"#;
    let (s, out) = compile(src);
    let r = ax::reach::analyze(&s.intern, &out);
    let io = r.capabilities.iter().find(|c| c.cap == "io").expect("io cap");
    assert!(io.reachable, "{r:?}");
    assert!(io.path.len() >= 2, "path {:?}", io.path);
}

#[test]
fn own_unused_is_check_error() {
    let src = r#"
module t;
fn take(p: own i32) -> i32 = 0;
fn main() -> i32 = 0;
"#;
    let mut s = Session::new();
    match s.compile("t.ax", src) {
        Ok(_) => panic!("expected A2021"),
        Err(d) => assert!(d.iter().any(|x| x.code == "A2021"), "{d:?}"),
    }
}

#[test]
fn formatter_strips_ref_and_pub() {
    let src = "pub fn go(x: i32) -> i32 { &x }\n";
    let mut intern = ax::Interner::new();
    let file = Parser::parse_file(src, FileId(0), &mut intern).unwrap();
    let out = ax::fmt::format_file(&file, &intern);
    assert!(!out.contains("pub "), "{out}");
    assert!(!out.contains("&x"), "{out}");
}

#[test]
fn par_disjoint_compiles() {
    let src = r#"
module t;
fn main() -> i32 = { par { let a = 1; let b = 2; }; 3 };
"#;
    let (_s, out) = compile(src);
    assert!(!out.diags.iter().any(|d| d.code == "E0600"));
}

#[test]
fn map_insert_get_eval() {
    let src = r#"
module t;
fn main() -> i64 !{alloc[a]} = {
    let mut m: Map[String, i64] = map.new(test.alloc);
    m.insert("k", 7i64);
    match m.get("k") {
        Some(v) => v;
        None => 0;
    }
};
"#;
    let (s, out) = compile(src);
    let v = ax::driver::run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.as_i128(), 7);
}

#[test]
fn declassify_and_to_i64() {
    let src = r#"
module t;
fn main() -> i64 = to_i64(3);
"#;
    let (s, out) = compile(src);
    let v = ax::driver::run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.as_i128(), 3);
}

#[test]
fn impl_from_parses() {
    parse_ok(
        r#"
type E = | Wrap;
impl From[ParseError] for E { }
fn main() -> i32 { 0 }
"#,
    );
}

#[test]
fn daemon_check_roundtrip() {
    let line = r#"{"id":1,"method":"check","params":{"source":"module t;\nfn main() -> i32 = 1;\n"}}"#;
    let resp = ax::daemon::handle_line(line);
    assert!(resp.contains("\"ok\":true") || resp.contains("\"ok\": true"), "{resp}");
}

#[test]
fn cap_budget_flags_extra() {
    let src = r#"
module t;
fn main() -> u64 !{io[fs], abort} = io.bytesum_file("x");
"#;
    let (s, out) = compile(src);
    let r = ax::reach::analyze(&s.intern, &out);
    let b = ax::reach::CapBudget::from_toml("[caps]\nallow = [\"fs\"]\n");
    let extra = b.check(&r);
    assert!(extra.iter().any(|(c, _)| c == "io"), "{extra:?}");
}

#[test]
fn gbnf_equivalence_1k() {
    let (a, b) = ax::gbnf::check_equivalence(1000);
    assert_eq!((a, b), (0, 0));
}

#[test]
fn fs_read_returns_untrusted() {
    let src = "module t;\nfn main() -> i32 = 0;\n";
    let (s, out) = compile(src);
    let fs = out
        .callables
        .iter()
        .find(|c| c.name == "fs.read" || c.name.ends_with(".read"))
        .expect("fs.read in prelude");
    let d = fs.ret.display(&s.intern);
    assert!(d.contains("Untrusted"), "fs.read should return Untrusted[…], got {d}");
}

#[test]
fn unique_and_rc_ops_exist() {
    let u = ax::ir::Op::UniqueAlloc { size: 0, align: 8 };
    let r = ax::ir::Op::RcRetain(1);
    assert!(format!("{u:?}").contains("UniqueAlloc"));
    assert!(format!("{r:?}").contains("RcRetain"));
}

#[test]
fn ax_mock_accepts_restricted_rust() {
    assert!(ax::axmock::PROMPT.contains("no lifetimes"));
    let src = "module t;\nfn main() -> i32 = 1 + 2;\n";
    assert!(ax::axmock::validity(src));
    assert!(ax::axmock::score_corpus(&[src]) > 0.9);
}

#[test]
fn a5002_detects_widening() {
    let extra = ax::reach::cap_widened(&["fs".into()], &["fs".into(), "net".into()]);
    assert_eq!(extra, vec!["net".to_string()]);
}
