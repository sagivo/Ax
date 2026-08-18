//! M2 soundness + M3 workspace: independent checkers, three surfaces,
//! transactional patch, G2 replay.

use ax::driver::Session;
use ax::frontend::{rewrite_dense_to_terse, rewrite_terse, to_dense, Surface};
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
fn dense_rewrites_and_compiles() {
    let dense = "#add(a I, b I) I = a + b\n";
    let terse = rewrite_dense_to_terse(dense);
    assert!(terse.contains("fn add"), "{terse}");
    assert!(terse.contains("i32"), "{terse}");
    let mut s = Session::new();
    s.surface = Surface::Dense;
    let out = s.compile("t.ax", dense).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(out.fns.len(), 1);
}

#[test]
fn dense_loop_and_bind_same_value() {
    let conv = "module t;\nfn main() -> usz = { let mut s: usz = 1; for i in range(0, 4) { s = s + i; }; s };\n";
    let dense = to_dense(conv);
    assert!(dense.contains("#"), "{dense}");
    assert!(dense.contains(":="), "{dense}");
    assert!(dense.contains('~'), "{dense}");
    let mut a = Session::new();
    let mut b = Session::new();
    b.surface = Surface::Dense;
    let oa = a.compile("a.ax", conv).unwrap();
    let ob = b
        .compile("b.ax", &dense)
        .unwrap_or_else(|d| panic!("dense={dense}\n{d:?}"));
    let va = ax::driver::run_main(&a.intern, &oa, 0).unwrap();
    let vb = ax::driver::run_main(&b.intern, &ob, 0).unwrap();
    assert_eq!(va.display(), vb.display());
    let ax_t = ax::tokens::count(conv).tokens;
    let d_t = ax::tokens::count(&dense).tokens;
    assert!(
        d_t < ax_t,
        "dense {d_t} should be < conventional {ax_t}\n{dense}"
    );
}

#[test]
fn dense_if_and_option_or() {
    let conv = r#"
module t;
fn main() -> i64 !{alloc[a]} = {
    let mut m: Map[String, i64] = map.new(test.alloc);
    m.insert("k", 7i64);
    if 1 < 2 { match m.get("k") { Some(v) => v; None => 0; } } else { 0 }
};
"#;
    let dense = to_dense(conv);
    assert!(dense.contains('$'), "{dense}");
    assert!(dense.contains('?'), "{dense}");
    let mut a = Session::new();
    let mut b = Session::new();
    b.surface = Surface::Dense;
    let oa = a.compile("a.ax", conv).unwrap();
    let ob = b
        .compile("b.ax", &dense)
        .unwrap_or_else(|d| panic!("dense={dense}\n{d:?}"));
    let va = ax::driver::run_main(&a.intern, &oa, 0).unwrap();
    let vb = ax::driver::run_main(&b.intern, &ob, 0).unwrap();
    assert_eq!(va.display(), vb.display());
}

#[test]
fn dense_while_map_lit_result() {
    let conv = r#"
module t;
fn main() -> i64 !{alloc[a], diverge} = {
    let mut m: Map[String, i64] = map.new(test.alloc);
    m.insert("k", 7i64);
    let mut i: usz = 0;
    while i < 1 { i = i + 1; };
    match m.get("k") { Some(v) => v; None => 0; }
};
"#;
    let dense = to_dense(conv);
    assert!(dense.contains('%') || dense.contains("map.new"), "{dense}");
    assert!(dense.contains('@') || dense.contains("while"), "{dense}");
    assert!(dense.contains('L') || dense.contains("i64"), "{dense}");
    let mut a = Session::new();
    let mut b = Session::new();
    b.surface = Surface::Dense;
    let oa = a.compile("a.ax", conv).unwrap();
    let ob = b
        .compile("b.ax", &dense)
        .unwrap_or_else(|d| panic!("dense={dense}\n{d:?}"));
    let va = ax::driver::run_main(&a.intern, &oa, 0).unwrap();
    let vb = ax::driver::run_main(&b.intern, &ob, 0).unwrap();
    assert_eq!(va.display(), vb.display());
}

#[test]
fn looks_like_dense_detects_hash_fn() {
    assert!(ax::frontend::looks_like_dense("#main() I = 1"));
    assert!(ax::frontend::looks_like_dense("s += 1"));
    assert!(ax::frontend::looks_like_dense("+/n"));
    assert!(!ax::frontend::looks_like_dense("fn main() -> i32 = 1"));
}

#[test]
fn dense_compound_assign_same_value() {
    let conv = "module t;\nfn main() -> i32 = { let mut s: i32 = 1; s = s + 2; s };\n";
    let dense = to_dense(conv);
    assert!(dense.contains("+="), "{dense}");
    let mut a = Session::new();
    let mut b = Session::new();
    b.surface = Surface::Dense;
    let oa = a.compile("a.ax", conv).unwrap();
    let ob = b
        .compile("b.ax", &dense)
        .unwrap_or_else(|d| panic!("dense={dense}\n{d:?}"));
    let va = ax::driver::run_main(&a.intern, &oa, 0).unwrap();
    let vb = ax::driver::run_main(&b.intern, &ob, 0).unwrap();
    assert_eq!(va.display(), vb.display());
    assert_eq!(va.display(), "3i32");
    assert!(
        ax::tokens::count("s += i").tokens < ax::tokens::count("s = s + i").tokens,
        "+= must be cheaper than s = s + i"
    );
}

#[test]
fn dense_k_reduce_sum_same_value() {
    let conv = "module t;\nfn main() -> usz = { let mut s: usz = 0; for i in range(0, 10) { s = s + i; }; s };\n";
    let dense = to_dense(conv);
    assert!(dense.contains("+/"), "expected +/ pack, got {dense}");
    let mut a = Session::new();
    let mut b = Session::new();
    b.surface = Surface::Dense;
    let oa = a.compile("a.ax", conv).unwrap();
    let ob = b
        .compile("b.ax", &dense)
        .unwrap_or_else(|d| panic!("dense={dense}\n{d:?}"));
    let va = ax::driver::run_main(&a.intern, &oa, 0).unwrap();
    let vb = ax::driver::run_main(&b.intern, &ob, 0).unwrap();
    assert_eq!(va.display(), vb.display());
    assert_eq!(va.display(), "45usz");
    // Written form, not packed: agent writes +/n.
    let written = "#main() Z = +/10Z\n";
    let mut c = Session::new();
    c.surface = Surface::Dense;
    let oc = c
        .compile("c.ax", written)
        .unwrap_or_else(|d| panic!("written={written}\n{d:?}"));
    let vc = ax::driver::run_main(&c.intern, &oc, 0).unwrap();
    assert_eq!(vc.display(), "45usz");
    // Bare `+/10` — integer literals infer as usz from `range`.
    let bare = "#main() Z = +/10\n";
    let mut d = Session::new();
    d.surface = Surface::Dense;
    let od = d
        .compile("d10.ax", bare)
        .unwrap_or_else(|err| panic!("bare={bare}\n{err:?}"));
    let vd = ax::driver::run_main(&d.intern, &od, 0).unwrap();
    assert_eq!(vd.display(), "45usz");
    assert_eq!(ax::tokens::count("+/n").tokens, 2);
}

#[test]
fn dense_k_reduce_product_and_range() {
    let prod = "#main() Z = */1..5\n";
    let mut s = Session::new();
    s.surface = Surface::Dense;
    let out = s.compile("p.ax", prod).unwrap_or_else(|d| panic!("{d:?}"));
    let v = ax::driver::run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.display(), "24usz"); // 1*2*3*4
}

#[test]
fn dense_compound_does_not_eat_division() {
    let src = "#main() I = { a I:= 8; b I:= 2; a / b }\n";
    let mut s = Session::new();
    s.surface = Surface::Dense;
    let out = s.compile("d.ax", src).unwrap_or_else(|d| panic!("{d:?}"));
    let v = ax::driver::run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.display(), "4i32");
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
        Err(d) => assert!(d
            .iter()
            .any(|x| x.msg.contains("S-verbose") || x.code == "E0101")),
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
    assert!(
        rs.iter().all(|r| r.permitted),
        "{:?}",
        rs.iter()
            .map(|r| format!(
                "{} {} ⊆ {}",
                r.fn_name,
                r.inferred.display(),
                r.declared.display()
            ))
            .collect::<Vec<_>>()
    );
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
    assert!(
        store_legal(0, 1),
        "static ref may be stored in nested location"
    );
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
    let tr = workspace::encode_trace(7, src, &v.display(), &hex::encode(v.canonical_bytes()), &[]);
    assert_eq!(tr.source_hash, hash::sha256_hex(src.as_bytes()));
    let v2 = ax::driver::run_main(&s.intern, &out, tr.seed).unwrap();
    assert_eq!(hex::encode(v2.canonical_bytes()), tr.canonical);
}

#[test]
fn conventional_still_compiles() {
    compile_ok("module t;\nfn main() -> i32 = 1 + 2;\n");
}

#[test]
fn default_session_compiles_short_syntax() {
    let mut s = Session::new();
    s.compile("t.ax", "#add(a I, b I) I = a + b\n")
        .unwrap_or_else(|d| panic!("{d:?}"));
}

#[test]
fn usecase_ax_snippets_compile() {
    for c in ax::usecases::cases() {
        let has_main = c.src.ax.contains("fn main(");
        let src = if has_main {
            format!("module t;\nexport {{ main }};\n{}", c.src.ax)
        } else {
            format!(
                "module t;\nexport {{ main }};\n{}\nfn main() -> i32 = 0;\n",
                c.src.ax
            )
        };
        let mut s = Session::new();
        s.compile(&format!("{}.ax", c.id), &src)
            .unwrap_or_else(|d| panic!("{}:\n{src}\n{d:?}", c.id));
        let dense = ax::frontend::to_dense(c.src.ax);
        let dsrc = if dense.contains("#main(") || dense.contains("fn main(") {
            format!("module t;\nexport {{ main }};\n{dense}\n")
        } else {
            format!("module t;\nexport {{ main }};\n{dense}\nfn main() -> i32 = 0;\n")
        };
        let mut b = Session::new();
        b.surface = Surface::Dense;
        b.compile(&format!("{}d.ax", c.id), &dsrc)
            .unwrap_or_else(|d| panic!("{} dense:\n{dsrc}\n{d:?}", c.id));
    }
}
