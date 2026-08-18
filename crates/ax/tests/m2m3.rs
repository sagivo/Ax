//! M2 soundness + M3 workspace: independent checkers, three surfaces,
//! transactional patch, G2 replay.

use ax::driver::Session;
use ax::frontend::{rewrite_terse, Surface};
use ax::hash;
use ax::indep::{self, store_legal, store_legal_v01_inverted};
use ax::workspace::{self, PatchTx};

fn compile_ok(src: &str) {
    let mut s = Session::new();
    s.compile("t.ax", src).unwrap_or_else(|d| panic!("{d:#?}"));
}

#[test]
fn terse_rewrites_to_conventional() {
    let terse = "module t;\nfn add(a i32, b i32) i32 = a + b;\n";
    let conv = rewrite_terse(terse);
    assert!(conv.contains("a: i32"), "{conv}");
    assert!(conv.contains("-> i32"), "{conv}");
    let mut s = Session::new();
    s.surface = Surface::Terse;
    let out = s.compile("t.ax", terse).unwrap();
    assert_eq!(out.fns.len(), 1);
}

#[test]
fn terse_and_conventional_same_interface_hash() {
    let conv = "module t;\nfn add(a: i32, b: i32) -> i32 = a + b;\n";
    let terse = "module t;\nfn add(a i32, b i32) i32 = a + b;\n";
    let mut a = Session::new();
    let mut b = Session::new();
    b.surface = Surface::Terse;
    let oa = a.compile("a.ax", conv).unwrap();
    let ob = b.compile("b.ax", terse).unwrap();
    assert_eq!(oa.hashes[0].interface_hash, ob.hashes[0].interface_hash);
}

#[test]
fn verbose_rejects_unannotated_let() {
    let src = "module t;\nfn main() -> i32 = { let x = 1; x };\n";
    let mut s = Session::new();
    s.surface = Surface::Verbose;
    match s.compile("t.ax", src) {
        Ok(_) => panic!("verbose must require let annotations"),
        Err(d) => assert!(d.iter().any(|x| x.msg.contains("S-verbose") || x.code == "E0101")),
    }
}

#[test]
fn verbose_accepts_annotated_let() {
    let src = "module t;\nfn main() -> i32 = { let x: i32 = 1; x };\n";
    let mut s = Session::new();
    s.surface = Surface::Verbose;
    s.compile("t.ax", src).unwrap();
}

#[test]
fn indep_effect_agrees_on_pure() {
    let src = "module t;\nfn add(a: i32, b: i32) -> i32 = a + b;\n";
    let mut s = Session::new();
    let file = s.parse("t.ax", src).unwrap();
    let checked = s.check(&file);
    let facts = indep::TypeFacts::new(&checked.node_types, &checked.nonzero_div);
    let rs = indep::infer_effects(&file, &s.intern, facts);
    assert!(rs.iter().all(|r| r.permitted), "{:?}", rs.iter().map(|r| format!("{} {} ⊆ {}", r.fn_name, r.inferred.display(), r.declared.display())).collect::<Vec<_>>());
}

#[test]
fn indep_effect_catches_undeclared_io() {
    let src = "module t;\nfn boom() -> unit = print(\"x\");\n";
    let mut s = Session::new();
    match s.compile("t.ax", src) {
        Ok(_) => panic!("io from effect-free must fail"),
        Err(d) => assert!(d.iter().any(|x| x.code == "E0200")),
    }
}

#[test]
fn region_rule_rejects_escape_accepts_inward() {
    assert!(store_legal(0, 1), "static ref may be stored in nested location");
    assert!(store_legal(1, 1), "same-depth store is inward");
    assert!(!store_legal(1, 0), "nested ref must not escape to static");
    // inverted v0.1 rule disagrees on the escape case — regression sentinel
    assert!(store_legal_v01_inverted(1, 0));
    assert_ne!(store_legal(1, 0), store_legal_v01_inverted(1, 0));
}

#[test]
fn patch_fails_closed_on_hash_mismatch() {
    let src = "module t;\nfn add(a: i32, b: i32) -> i32 = a + b;\n";
    let mut intern = ax::Interner::new();
    let file = ax::parser::Parser::parse_file(src, ax::span::FileId(0), &mut intern).unwrap();
    let tx = PatchTx {
        base_module_hash: "deadbeef".into(),
        def_id: "t::fn:add".into(),
        path: vec![],
        expected_subtree_hash: "abc".into(),
        replacement_src: Some("a - b".into()),
        replacement_ast: None,
    };
    let r = workspace::apply_patch(&mut intern, src, &file, &tx);
    assert!(!r.ok);
    assert!(!r.applied);
}

#[test]
fn patch_applies_body_rewrite() {
    let src = "module t;\nfn add(a: i32, b: i32) -> i32 = a + b;\n";
    let mut intern = ax::Interner::new();
    let file = ax::parser::Parser::parse_file(src, ax::span::FileId(0), &mut intern).unwrap();
    let base = hash::sha256_hex(src.as_bytes());
    let body_h = hash::body_hash(&format!("{:?}", file.decls[0].kind));
    // Use the body's kind hash the same way apply_patch does
    let subtree = match &file.decls[0].kind {
        ax::ast::DeclKind::Fn(f) => format!("{:?}", f.body.kind),
        _ => panic!("fn"),
    };
    let tx = PatchTx {
        base_module_hash: base,
        def_id: "t::fn:add".into(),
        path: vec![],
        expected_subtree_hash: hash::body_hash(&subtree),
        replacement_src: Some("a - b".into()),
        replacement_ast: None,
    };
    let r = workspace::apply_patch(&mut intern, src, &file, &tx);
    assert!(r.ok, "{:?}", r.reason);
    assert!(r.applied);
    let new_src = r.source.unwrap();
    assert!(new_src.contains("a - b"), "{new_src}");
    let _ = body_h;
}

#[test]
fn replay_round_trip() {
    let src = "module t;\nfn main() -> i32 = 40 + 2;\n";
    let mut s = Session::new();
    let out = s.compile("t.ax", src).unwrap();
    let v = ax::driver::run_main(&s.intern, &out, 7).unwrap();
    let tr = workspace::encode_trace(
        7,
        src,
        &v.display(),
        &hex::encode(v.canonical_bytes()),
        &[],
    );
    assert_eq!(tr.source_hash, hash::sha256_hex(src.as_bytes()));
    let v2 = ax::driver::run_main(&s.intern, &out, tr.seed).unwrap();
    assert_eq!(hex::encode(v2.canonical_bytes()), tr.canonical);
}

#[test]
fn conventional_still_compiles() {
    compile_ok("module t;\nfn main() -> i32 = 1 + 2;\n");
}
