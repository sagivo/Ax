//! Agent-canonical tree surface: parse, check, run, and print as a bijection.
//!
//! The conventional surface stays as the corpus dialect. These tests pin that
//! a tree program is the same language — same IR, same answers — not a coat.

use ax::driver::{run_main, Session};
use ax::frontend::Surface;
use ax::span::FileId;

fn compile_tree(src: &str) -> (Session, ax::check::CheckOutput) {
    let mut s = Session::new();
    s.surface = Surface::Tree;
    match s.compile("t.ax", src) {
        Ok(o) => (s, o),
        Err(d) => panic!("tree compile failed:\n{d:#?}"),
    }
}

fn compile_conv(src: &str) -> (Session, ax::check::CheckOutput) {
    let mut s = Session::new();
    match s.compile("t.ax", src) {
        Ok(o) => (s, o),
        Err(d) => panic!("conventional compile failed:\n{d:#?}"),
    }
}

#[test]
fn looks_like_tree_detects_open_paren() {
    assert!(ax::tree::looks_like_tree("(module t\n  (fn main () i32 1)\n)\n"));
    assert!(ax::tree::looks_like_tree("; comment\n(fn main () i32 1)\n"));
    assert!(!ax::tree::looks_like_tree("module t;\nfn main() -> i32 = 1;\n"));
    assert!(!ax::tree::looks_like_tree(""));
}

#[test]
fn tree_add_runs() {
    let src = r#"(module t
  (export main)
  (fn main () i32 (+ 1 2))
)
"#;
    let (s, out) = compile_tree(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.display(), "3i32");
}

#[test]
fn tree_auto_detect_without_surface_flag() {
    let src = r#"(module t
  (fn main () i32 (+ 40 2))
)
"#;
    let mut s = Session::new();
    // Default surface is conventional; detection must still take the tree path.
    let out = s.compile("t.ax", src).unwrap_or_else(|d| panic!("{d:#?}"));
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.display(), "42i32");
}

#[test]
fn tree_and_conventional_same_answer() {
    let tree = r#"(module t
  (fn add ((a i32) (b i32)) i32 (+ a b))
  (fn main () i32 (add 20 22))
)
"#;
    let conv = "module t;\nfn add(a: i32, b: i32) -> i32 = a + b;\nfn main() -> i32 = add(20, 22);\n";
    let (st, ot) = compile_tree(tree);
    let (sc, oc) = compile_conv(conv);
    let vt = run_main(&st.intern, &ot, 0).unwrap();
    let vc = run_main(&sc.intern, &oc, 0).unwrap();
    assert_eq!(vt.display(), vc.display());
    assert_eq!(vt.display(), "42i32");
}

#[test]
fn tree_if_match_while() {
    let src = r#"(module t
  (fn pick ((b bool)) i32 (if b 1 0))
  (fn main () i32
    (block
      (let mut s i32 0)
      (let i i32 0)
      (while (!= i 3)
        (block
          (set s (+ s (pick true)))
          (set i (+ i 1))
          s))
      s))
)
"#;
    let (s, out) = compile_tree(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.display(), "3i32");
}

#[test]
fn tree_raise_catch() {
    let src = r#"(module t
  (type DivError (or Zero))
  (fn boom () i32 (! (err DivError)) (raise Zero))
  (fn safe () i32 (catch (boom) (arm Zero 0)))
  (fn main () i32 (safe))
)
"#;
    let (s, out) = compile_tree(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.display(), "0i32");
}

#[test]
fn tree_record_field() {
    let src = r#"(module t
  (type Vec2 (rec (x f32) (y f32)))
  (fn main () f32
    (block
      (let v Vec2 (rec (x 3.0) (y 4.0)))
      (field v x)))
)
"#;
    let (s, out) = compile_tree(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.display(), "3f32");
}

#[test]
fn tree_fmt_roundtrip() {
    let src = r#"(module t
  (export main)
  (fn add ((a i32) (b i32)) i32 (+ a b))
  (fn main () i32 (add 1 2))
)
"#;
    let mut intern = ax::Interner::new();
    let file = ax::tree::parse_file(src, FileId(0), &mut intern, "t").unwrap();
    let once = ax::tree::format_file(&file, &intern);
    let mut intern2 = ax::Interner::new();
    let file2 = ax::tree::parse_file(&once, FileId(0), &mut intern2, "t").unwrap();
    let twice = ax::tree::format_file(&file2, &intern2);
    assert_eq!(once, twice, "tree printer is not idempotent:\n{once}\n---\n{twice}");
    let mut s = Session::new();
    let out = s.compile("t.ax", &once).unwrap_or_else(|d| panic!("{d:#?}"));
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.display(), "3i32");
}

#[test]
fn tree_rejects_infix() {
    let src = "(module t (fn main () i32 a + b))\n";
    let mut intern = ax::Interner::new();
    let r = ax::tree::parse_file(src, FileId(0), &mut intern, "t");
    assert!(r.is_err(), "infix must not parse on the tree surface");
}

#[test]
fn tree_rejects_ungrouped_params() {
    // `(a i32)` as a sibling of the return type is not a parameter list.
    let src = "(module t (fn add (a i32) (b i32) i32 (+ a b)))\n";
    let mut intern = ax::Interner::new();
    let r = ax::tree::parse_file(src, FileId(0), &mut intern, "t");
    assert!(r.is_err(), "ungrouped params must not parse");
}

#[test]
fn tree_hole_is_a_hole() {
    let src = r#"(module t
  (fn distance ((v i32)) i32 ?)
)
"#;
    let mut s = Session::new();
    s.allow_holes = true;
    let out = s
        .compile("t.ax", src)
        .unwrap_or_else(|d| panic!("{d:#?}"));
    assert!(!out.holes.is_empty(), "expected a typed hole");
}

#[test]
fn tree_hole_fills_are_tree_forms() {
    let src = r#"(module t
  (type Vec2 (rec (x f32) (y f32)))
  (fn distance ((v Vec2)) f32 ?)
)
"#;
    let fills = ax::agent::hole_fills("t.ax", src, Surface::Tree, 32);
    assert!(!fills.is_empty(), "expected fills");
    let exprs: Vec<&str> = fills[0].fills.iter().map(|f| f.expr.as_str()).collect();
    assert!(
        exprs.iter().any(|e| e.starts_with("(field ")),
        "tree fills must propose (field v x), got {exprs:?}"
    );
    assert!(
        exprs.iter().any(|e| e.starts_with("(math.hypot ")),
        "tree fills must propose a prefix call, got {exprs:?}"
    );
    assert!(
        !exprs.iter().any(|e| e.contains(".x") || e.contains(".y") && !e.starts_with("(field ")),
        "tree fills must not emit infix field access: {exprs:?}"
    );
}

#[test]
fn tree_no_rust_elision_needed() {
    // There is no `pub`, `unsafe`, `&mut`, or `?` sugar in the tree. The
    // forms an agent writes are the forms the AST stores.
    let src = r#"(module t
  (fn main () i32 (neg 1))
)
"#;
    let (s, out) = compile_tree(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.display(), "-1i32");
}

#[test]
fn tree_gbnf_is_a_list_grammar() {
    let g = ax::tree::file_gbnf();
    assert!(g.contains("fn-decl"), "{g}");
    assert!(g.contains("binop"), "{g}");
    assert!(!g.contains("->"), "tree GBNF must not encode Rust arrows");
}

#[test]
fn session_default_is_short() {
    let s = Session::new();
    assert_eq!(s.surface, Surface::Dense);
}

#[test]
fn conventional_files_still_parse_under_tree_default() {
    let src = "module t;\nfn main() -> i32 = 1 + 2;\n";
    let (s, out) = compile_conv(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.display(), "3i32");
}

#[test]
fn self_hosted_frontend_compiles() {
    let src = include_str!("../../../std/tree/lib.ax");
    assert!(ax::tree::looks_like_tree(src));
    let mut s = Session::new();
    s.compile("std/tree/lib.ax", src)
        .unwrap_or_else(|d| panic!("{d:#?}"));
}
