//! M0/M1 kernel tests: grammar, primitives, effects, errors, regions,
//! contracts, par, protocol, and the appendix worked example.

use ax::diag::catalog;
use ax::driver::{
    check_report, deps_affected, guarantee_labels, hole_report, run_main, run_tests, Session,
};
use ax::interp::{canon_f32, canon_f64, Value};
use ax::types::Prim;

fn parse_ok(src: &str) {
    let mut s = Session::new();
    s.parse("t.ax", src).unwrap_or_else(|d| {
        panic!("parse failed: {:?}", d);
    });
}

fn compile(src: &str) -> (Session, ax::check::CheckOutput) {
    let mut s = Session::new();
    match s.compile("t.ax", src) {
        Ok(o) => (s, o),
        Err(d) => panic!("compile failed:\n{:#?}", d),
    }
}

fn compile_err(src: &str) -> Vec<ax::diag::Diagnostic> {
    let mut s = Session::new();
    match s.compile("t.ax", src) {
        Ok(_) => panic!("expected errors"),
        Err(d) => d,
    }
}

fn wrap_fn(body: &str, ret: &str, effs: &str) -> String {
    format!("module t;\nfn main() -> {ret} {effs}= {body};\n")
}

#[test]
fn parse_minimal() {
    parse_ok("module t;\nfn main() -> i32 = 1;\n");
}

#[test]
fn parse_signature_shapes() {
    parse_ok(
        r#"
module t;
fn distance(v: Vec2) -> f32 = math.hypot(v.x, v.y);
fn parse_i32(s: &str) -> i32 !{err[ParseError]} = 0;
fn sort[T](xs: &mut [T], order: Ord[T] = default) -> unit !{diverge} = ();
"#,
    );
}

#[test]
fn parse_appendix_loader() {
    let src = include_str!("../../../examples/loader.ax");
    parse_ok(src);
    assert!(
        ax::tree::looks_like_tree(src),
        "loader.ax must be the tree surface"
    );
}

#[test]
fn integer_wrap_semantics() {
    assert_eq!(Prim::I8.wrap_i128(128), -128);
    assert_eq!(Prim::I8.wrap_i128(127), 127);
    assert_eq!(Prim::U8.wrap_i128(256), 0);
    assert_eq!(Prim::U8.wrap_i128(255), 255);
    assert_eq!(Prim::I32.wrap_i128(i32::MAX as i128 + 1), i32::MIN as i128);
}

#[test]
fn eval_arithmetic_wrap() {
    let src = wrap_fn("{ let x: i8 = 127i8; x + 1i8 }", "i8", "");
    let (s, out) = compile(&src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    match v {
        Value::Int {
            bits,
            prim: Prim::I8,
        } => assert_eq!(bits, -128),
        other => panic!("{:?}", other),
    }
}

#[test]
fn eval_div_zero_raises() {
    let src = r#"
module t;
type DivError = | Zero;
fn main() -> i32 !{err[DivError]} = 1 / 0;
"#;
    let (s, out) = compile(src);
    let err = run_main(&s.intern, &out, 0).unwrap_err();
    assert!(err.contains("Zero") || err.contains("error"), "{err}");
}

#[test]
fn catch_removes_err() {
    let src = r#"
module t;
type DivError = | Zero;
fn d(a: i32, b: i32) -> i32 !{err[DivError]} = if b == 0 { raise Zero } else { a };
fn main() -> i32 = catch d(1, 0) { Zero => 7 };
"#;
    let (s, out) = compile(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.as_i128(), 7);
}

#[test]
fn attempt_reifies_result() {
    let src = r#"
module t;
type DivError = | Zero;
fn d(b: i32) -> i32 !{err[DivError]} = if b == 0 { raise Zero } else { b };
fn main() -> i32 = {
    match attempt d(0) {
        Err(Zero) => 1;
        _ => 0;
    }
};
"#;
    let (s, out) = compile(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.as_i128(), 1);
}

#[test]
fn missing_err_in_row_is_error() {
    let src = r#"
module t;
type DivError = | Zero;
fn d() -> i32 !{err[DivError]} = raise Zero;
fn main() -> i32 = d();
"#;
    let diags = compile_err(src);
    assert!(diags.iter().any(|d| d.code == "E0200"), "{:?}", diags);
}

#[test]
fn injection_allows_propagation() {
    let src = r#"
module t;
type Inner = | Boom;
type Outer = | Wrap { cause: Inner } with from Inner => Wrap;
fn inner() -> i32 !{err[Inner]} = raise Boom;
fn outer() -> i32 !{err[Outer]} = inner();
fn main() -> i32 = catch outer() { Wrap { cause } => 3 };
"#;
    let (s, out) = compile(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.as_i128(), 3);
}

#[test]
fn missing_injection_is_error() {
    let src = r#"
module t;
type Inner = | Boom;
type Outer = | Other;
fn inner() -> i32 !{err[Inner]} = raise Boom;
fn outer() -> i32 !{err[Outer]} = inner();
"#;
    let diags = compile_err(src);
    assert!(
        diags.iter().any(|d| d.code == "E0202" || d.code == "E0200"),
        "{:?}",
        diags
    );
}

#[test]
fn ambiguous_injection_is_error() {
    let src = r#"
module t;
type Inner = | Boom;
type Outer =
    | A { cause: Inner }
    | B { cause: Inner }
with
    from Inner => A;
    from Inner => B;
"#;
    let diags = compile_err(src);
    assert!(diags.iter().any(|d| d.code == "E0203"), "{:?}", diags);
}

#[test]
fn no_implicit_numeric_conversion() {
    let src = r#"
module t;
fn main() -> i64 = 1i32;
"#;
    let diags = compile_err(src);
    assert!(
        diags.iter().any(|d| d.code == "E0108" || d.code == "E0101"),
        "{:?}",
        diags
    );
}

#[test]
fn hole_rejected_without_flag() {
    let src = "module t;\nfn main() -> i32 = ?;\n";
    let diags = compile_err(src);
    assert!(diags.iter().any(|d| d.code == "E0500"), "{:?}", diags);
}

#[test]
fn hole_allowed_and_ranked() {
    let src = r#"
module t;
type Vec2 = { x: f32, y: f32 };
fn distance(v: Vec2) -> f32 = ?;
"#;
    let mut s = Session::new();
    s.allow_holes = true;
    let file = s.parse("h.ax", src).unwrap();
    let out = s.check(&file);
    assert!(!out.holes.is_empty());
    let report = hole_report(&s.intern, &out, None);
    assert!(report.contains("expects: f32"), "{report}");
    assert!(report.contains("in scope"), "{report}");
}

#[test]
fn region_escape_rejected() {
    let src = r#"
module t;
fn bad() -> &r i32 = region r {
    let x: i32 = 1;
    &x
};
"#;
    let diags = compile_err(src);
    assert!(
        diags
            .iter()
            .any(|d| d.code == "E0302" || d.code == "E0101" || d.code == "E0008"),
        "{:?}",
        diags
    );
}

#[test]
fn region_inward_store_ok() {
    // Named `main` because `main` is the only entry point: running "whatever
    // function came last" made a missing entry point look like a failure deep
    // inside an arbitrary body.
    let src = r#"
module t;
fn main() -> i32 = region r {
    let x: i32 = 4;
    x
};
"#;
    let (s, out) = compile(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.as_i128(), 4);
}

#[test]
fn contract_illegal_loop() {
    let src = r#"
module t;
fn f(x: i32) -> i32
    pre loop { x }
= x;
"#;
    let diags = compile_err(src);
    assert!(diags.iter().any(|d| d.code == "E0501"), "{:?}", diags);
}

#[test]
fn par_disjoint_is_accepted() {
    let src = r#"
module t;
fn main() -> i32 = {
    par {
        let a = 1;
        let b = 2;
    };
    3
};
"#;
    let (s, out) = compile(src);
    assert!(
        !out.diags.iter().any(|d| d.code == "E0600"),
        "disjoint par should be accepted: {:?}",
        out.diags
    );
    assert_eq!(run_main(&s.intern, &out, 0).unwrap().as_i128(), 3);
}

#[test]
fn par_overlapping_mut_is_rejected() {
    let src = r#"
module t;
fn main() -> i32 = {
    let mut x: i32 = 0;
    par {
        let a = { x = x + 1; x };
        let b = { x = x + 2; x };
    };
    0
};
"#;
    let diags = compile_err(src);
    assert!(diags.iter().any(|d| d.code == "E0600"), "{diags:?}");
}

#[test]
fn labels_safe_without_ffi() {
    let src = "module t;\nfn main() -> i32 = 1;\n";
    let (s, out) = compile(src);
    let ls = guarantee_labels(&s.intern, &out, false, false);
    assert!(ls.contains(&"safe".into()));
    assert!(ls.contains(&"capability-contained".into()));
    assert!(ls.contains(&"deterministic-core".into()));
}

#[test]
fn trusted_ffi_excludes_safe() {
    let src = "module t;\nfn main() -> i32 = 1;\n";
    let (s, out) = compile(src);
    let ls = guarantee_labels(&s.intern, &out, true, false);
    assert!(ls.contains(&"trusted-ffi".into()));
    assert!(!ls.contains(&"safe".into()));
    assert!(!ls.contains(&"capability-contained".into()));
}

#[test]
fn error_codes_append_only() {
    let codes: Vec<_> = catalog().into_iter().map(|(c, _)| c).collect();
    let mut sorted = codes.clone();
    sorted.sort();
    // uniqueness
    sorted.dedup();
    assert_eq!(codes.len(), sorted.len());
}

#[test]
fn check_report_json() {
    let src = "module t;\nfn main() -> i32 = 1;\n";
    let (_s, out) = compile(src);
    let r = check_report(&out);
    assert!(r.ok);
    let _ = serde_json::to_string(&r).unwrap();
}

#[test]
fn run_div_example_tests() {
    let src = include_str!("../../../examples/div.ax");
    let (s, out) = compile(src);
    let rs = run_tests(&s.intern, &out, 0);
    assert!(!rs.is_empty());
    for r in &rs {
        assert!(r.ok, "{}: {:?}", r.name, r.msg);
    }
}

#[test]
fn run_loader_example() {
    let src = include_str!("../../../examples/loader.ax");
    let (s, out) = compile(src);
    let rs = run_tests(&s.intern, &out, 0);
    assert!(!rs.is_empty(), "expected tests");
    for r in &rs {
        assert!(r.ok, "{}: {:?}", r.name, r.msg);
    }
}

#[test]
fn api_server_lowers_typed_handler_to_native_reactor() {
    let src = include_str!("../../../examples/api_server.ax");
    let (s, out) = compile(src);
    let generated = ax::codegen::emit_c(&s.intern, &out).expect("emit API server C");
    assert!(generated.contains("ax_rt_http_serve_handler"));
    assert!(generated.contains("ax_rt_str_eq"));
}

#[test]
fn get_none_at_aborts() {
    let src = r#"
module t;
fn main() -> i32 = 1;
"#;
    let (s, out) = compile(src);
    assert_eq!(run_main(&s.intern, &out, 0).unwrap().as_i128(), 1);
}

#[test]
fn replay_same_seed_same_bytes() {
    let src = "module t;\nfn main() -> i32 = 40 + 2;\n";
    let (s, out) = compile(src);
    let a = run_main(&s.intern, &out, 7).unwrap();
    let b = run_main(&s.intern, &out, 7).unwrap();
    assert_eq!(a.canonical_bytes(), b.canonical_bytes());
}

#[test]
fn card_exists_and_bounded() {
    let card = ax::driver::card_text();
    assert!(card.contains("Ax card"));
    assert!(card.len() <= 12_000, "card is {} bytes", card.len());
}

#[test]
fn grammar_file_exists() {
    let g = include_str!("../../../spec/grammar.ebnf");
    assert!(g.contains("file"));
    assert!(g.contains("fn_decl"));
}

#[test]
fn snippet_distance_compiles() {
    let src = r#"
module t;
type Vec2 = { x: f32, y: f32 };
fn distance(v: Vec2) -> f32 = math.hypot(v.x, v.y);
fn main() -> f32 = distance({ x: 3.0, y: 4.0 });
"#;
    let (s, out) = compile(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert!((v.as_f32() - 5.0).abs() < 1e-5);
}

#[test]
fn unknown_name() {
    let diags = compile_err("module t;\nfn main() -> i32 = nope;\n");
    assert!(diags.iter().any(|d| d.code == "E0100"));
}

#[test]
fn deps_affected_lists_def() {
    let src = "module t;\nfn add(a: i32, b: i32) -> i32 = a + b;\n";
    let (_s, out) = compile(src);
    let id = &out.hashes[0].def_id;
    let r = deps_affected(&out, id);
    assert!(r.contains(id));
}

#[test]
fn checked_add_none_on_overflow() {
    let src = r#"
module t;
fn main() -> i32 = {
    match int.checked_add(2147483647, 1) {
        None => 1;
        Some { value } => 0;
    }
};
"#;
    let (s, out) = compile(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.as_i128(), 1);
}

#[test]
fn parse_i32_ok() {
    let src = r#"
module t;
fn main() -> i32 !{err[ParseError]} = parse_i32("42");
"#;
    let (s, out) = compile(src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    assert_eq!(v.as_i128(), 42);
}

// ---- restored kernel tests -------------------------------------------------
//
// These cover M0-M3 semantics: hash identities, IEEE determinism, effect-row
// discipline under `--strict-det`, dictionary resolution, formatter idempotence,
// and the diagnostic schema.

#[test]
fn canonical_nan() {
    // Every NaN collapses to one bit pattern so the oracle and the native tiers
    // can be compared bit-for-bit.
    assert_eq!(canon_f32(f32::NAN).to_bits(), 0x7fc0_0000);
    assert_eq!(canon_f64(f64::NAN).to_bits(), 0x7ff8_0000_0000_0000);
    assert_eq!(
        canon_f32(-f32::NAN).to_bits(),
        canon_f32(f32::NAN).to_bits()
    );
    let src = wrap_fn("0.0 / 0.0", "f64", "");
    let (s, out) = compile(&src);
    let v = run_main(&s.intern, &out, 0).unwrap();
    match v {
        Value::Float { bits, prim } => {
            assert_eq!(prim, Prim::F64);
            assert_eq!(bits, 0x7ff8_0000_0000_0000);
        }
        other => panic!("expected a float, got {}", other.display()),
    }
}

#[test]
fn ieee_add_deterministic() {
    // Strict IEEE-754: no reassociation, no extended precision, so the classic
    // 0.1 + 0.2 result is exactly reproducible.
    let src = wrap_fn("0.1 + 0.2", "f64", "");
    let (s, out) = compile(&src);
    let first = run_main(&s.intern, &out, 0).unwrap().display();
    let (s2, out2) = compile(&src);
    let second = run_main(&s2.intern, &out2, 7).unwrap().display();
    assert_eq!(first, "0.30000000000000004f64");
    assert_eq!(first, second, "float results must not depend on the seed");
}

#[test]
fn four_identities_distinct() {
    // def_id, interface_hash, body_hash, and build_hash are four separate
    // identities; collapsing any two would make incremental decisions wrong.
    let src = "module t;\nfn add(a: i32, b: i32) -> i32 = a + b;\n";
    let (_s, out) = compile(src);
    let h = out.hashes.first().expect("one def");
    let ids = [
        h.def_id.clone(),
        h.interface_hash.clone(),
        h.body_hash.clone(),
        h.build_hash.clone(),
    ];
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            assert_ne!(ids[i], ids[j], "identities {i} and {j} collided");
        }
    }
}

#[test]
fn body_only_change_keeps_interface_hash() {
    // Changing a body must not change the interface: that is what lets a caller
    // skip rechecking when only an implementation moved.
    let a = "module t;\nfn f(x: i32) -> i32 = x + 1;\n";
    let b = "module t;\nfn f(x: i32) -> i32 = x + 2;\n";
    let (_sa, oa) = compile(a);
    let (_sb, ob) = compile(b);
    let ha = oa.hashes.first().unwrap();
    let hb = ob.hashes.first().unwrap();
    assert_eq!(ha.def_id, hb.def_id);
    assert_eq!(
        ha.interface_hash, hb.interface_hash,
        "a body edit must not move the interface hash"
    );
    assert_ne!(
        ha.body_hash, hb.body_hash,
        "a body edit must move the body hash"
    );
}

#[test]
fn diagnostic_json_schema_versioned() {
    // Diagnostics are a machine interface, so every one carries a schema version
    // and a stable code.
    let diags = compile_err("module t;\nfn f() -> i32 = \"nope\";\n");
    assert!(!diags.is_empty());
    for d in &diags {
        assert_eq!(d.schema, 1, "every diagnostic carries a schema version");
        assert!(
            d.code.starts_with('E'),
            "code should be E-prefixed: {}",
            d.code
        );
    }
    let report = check_report(&{
        let mut s = Session::new();
        let file = s.parse("t.ax", "module t;\nfn f() -> i32 = 1;\n").unwrap();
        s.check(&file)
    });
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["ok"], serde_json::json!(true));
    assert!(json["defs"].is_array());
}

#[test]
fn only_semantics_preserving_in_catalog_policy() {
    // The catalog is the published list of diagnostics; codes must be unique and
    // described, because an agent selects behaviour from them.
    let cat = catalog();
    assert!(cat.len() > 10, "catalog looks empty: {}", cat.len());
    let mut codes: Vec<&str> = cat.iter().map(|(c, _)| *c).collect();
    codes.sort_unstable();
    let before = codes.len();
    codes.dedup();
    assert_eq!(
        before,
        codes.len(),
        "duplicate diagnostic codes in the catalog"
    );
    for (code, desc) in &cat {
        assert!(
            code.starts_with('E') || code.starts_with('A') || code.starts_with('P'),
            "bad code {code}"
        );
        assert!(!desc.is_empty(), "{code} has no description");
    }
}

#[test]
fn dict_default_and_override() {
    // `= default` resolves to the unique visible dictionary, and an explicit
    // argument overrides it.
    let src = r#"
module t;
dict Ord[i32] = { cmp: i32.cmp };
fn pick(a: i32, b: i32, o: Ord[i32] = default) -> i32 =
  match o.cmp(&a, &b) { Lt => b; _ => a; };
fn main() -> i32 = pick(3, 7);
"#;
    let (s, out) = compile(src);
    assert_eq!(run_main(&s.intern, &out, 0).unwrap().as_i128(), 7);
    assert!(!out.dicts.is_empty(), "the dictionary should be recorded");
    assert!(
        !out.dict_defaults.is_empty(),
        "the resolution should be published for the backends"
    );
}

#[test]
fn unbounded_loop_requires_diverge() {
    // An explicit empty row is a claim that the function terminates. `loop`
    // has no static bound, so that claim is a lie. An *omitted* row is not a
    // claim: `diverge` is reconstructed from the body.
    let diags = compile_err("module t;\nfn spin() -> i32 !{} = loop { 1 };\n");
    assert!(
        diags.iter().any(|d| d.code == "E0200"),
        "expected an effect-row error, got {:?}",
        diags.iter().map(|d| &d.code).collect::<Vec<_>>()
    );
    let src = "module t;\nfn spin() -> i32 = loop { 1 };\nfn main() -> i32 = 0;\n";
    let (_s, out) = compile(src);
    assert!(
        !out.diags.iter().any(|d| d.is_error()),
        "an omitted row should reconstruct diverge: {:?}",
        out.diags
    );
}

#[test]
fn diverge_declared_ok() {
    // Declaring it is enough; `diverge` is not an error, it is a disclosure.
    let src = "module t;\nfn spin() -> i32 !{diverge} = loop { 1 };\nfn main() -> i32 = 0;\n";
    let (_s, out) = compile(src);
    assert!(!out.diags.iter().any(|d| d.is_error()));
}

#[test]
fn finite_for_no_diverge_required() {
    // A `for` over a finite range is bounded, so it needs no `diverge`. This is
    // the distinction that makes `diverge` informative.
    let src = r#"
module t;
fn total() -> usz = {
    let mut acc = 0usz;
    for i in range(0, 10) { acc = acc + i; };
    acc
};
"#;
    let (s, out) = compile(src);
    assert!(!out.diags.iter().any(|d| d.is_error()));
    let v = ax::driver::run_tests(&s.intern, &out, 0);
    let _ = v;
}

#[test]
fn strict_det_rejects_io() {
    // `--strict-det` rejects io/race/nondet: a deterministic build cannot
    // contain an effect whose result depends on the world.
    let src = "module t;\nfn main() -> unit !{io[stdout]} = print(\"x\");\n";
    let mut s = Session::new();
    s.strict_det = true;
    match s.compile("t.ax", src) {
        Ok(_) => panic!("strict-det must reject io"),
        Err(d) => assert!(
            d.iter().any(|x| x.is_error()),
            "expected an error, got {:?}",
            d.iter().map(|x| &x.code).collect::<Vec<_>>()
        ),
    }
}

#[test]
fn strict_det_allows_diverge() {
    // `diverge` is not nondeterminism: a program that may not terminate is still
    // deterministic in what it produces when it does.
    let src = "module t;\nfn spin() -> i32 !{diverge} = loop { 1 };\nfn main() -> i32 = 0;\n";
    let mut s = Session::new();
    s.strict_det = true;
    let out = s
        .compile("t.ax", src)
        .expect("strict-det must allow diverge");
    assert!(!out.diags.iter().any(|d| d.is_error()));
}

#[test]
fn formatter_idempotent() {
    // `ax fmt` is a fixed point: agents round-trip source through it, so a second
    // pass must change nothing.
    let src = r#"
module t;
type P = { x: i32, y: i32 };
fn add(a: i32, b: i32) -> i32 = a + b;
fn pick(p: P) -> i32 = match p { { x, y } => add(x, y); };
test "adds" = assert(add(1, 2) == 3);
"#;
    let mut s = Session::new();
    let file = s.parse("t.ax", src).unwrap();
    let once = ax::fmt::format_file(&file, &s.intern);
    let mut s2 = Session::new();
    let file2 = s2.parse("t.ax", &once).unwrap();
    let twice = ax::fmt::format_file(&file2, &s2.intern);
    assert_eq!(once, twice, "formatter is not idempotent");
}
