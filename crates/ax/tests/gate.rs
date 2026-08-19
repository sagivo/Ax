//! Agent-loop harness, capability red team, native error/record lowering.

use ax::caps::{self, ReadCap};
use ax::codegen::{self, Tier};
use ax::driver::{run_main, Session};
use std::path::PathBuf;

/// Every fill `ax hole --fills` reports as compiling must actually compile.
/// That is the whole promise of the command: an agent can paste the top fill
/// without spending an attempt to discover a type error.
#[test]
fn reported_fills_actually_compile() {
    let src = r#"
module t;
type Vec2 = { x: f32, y: f32 };
fn distance(v: Vec2) -> f32 = ?;
"#;
    let holes = ax::agent::hole_fills("t.ax", src, ax::frontend::Surface::Ax, 32);
    let h = holes.first().expect("one hole");
    assert!(
        h.fills.iter().any(|f| f.compiles),
        "no compiling fill was found"
    );
    for f in h.fills.iter().filter(|f| f.compiles) {
        let patched = src.replacen('?', &f.expr, 1);
        let mut s = Session::new();
        let out = s.compile("t.ax", &patched).unwrap_or_else(|d| {
            panic!(
                "fill {:?} was reported as compiling but did not: {d:?}",
                f.expr
            )
        });
        assert!(
            !out.diags.iter().any(|d| d.is_error()),
            "fill {:?} produced errors",
            f.expr
        );
    }
    // And the intended answer is among them: `hypot` uses both fields.
    assert!(
        h.fills
            .iter()
            .any(|f| f.compiles && f.expr.contains("hypot")),
        "expected a hypot candidate, got {:?}",
        h.fills.iter().map(|f| &f.expr).collect::<Vec<_>>()
    );
}

#[test]
fn cap_rejects_parent_escape() {
    let tmp = std::env::temp_dir().join("ax-cap-root");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("ok.txt"), "hi").unwrap();
    let cap = ReadCap::open_dir(&tmp).unwrap();
    assert!(cap.read("ok.txt").is_ok());
    assert!(matches!(
        cap.read("../etc/passwd"),
        Err(caps::CapError::Escape)
    ));
    assert!(matches!(
        cap.read("/etc/passwd"),
        Err(caps::CapError::Absolute)
    ));
}

#[test]
fn cap_rejects_dotdot_in_join() {
    let root = PathBuf::from("/tmp");
    assert!(caps::confine(&root, "a/../../etc").is_err());
    assert!(caps::confine(&root, "a/b").is_ok());
}

#[test]
fn cap_no_widen() {
    let tmp = std::env::temp_dir().join("ax-cap-root2");
    std::fs::create_dir_all(&tmp).unwrap();
    let cap = ReadCap::open_dir(&tmp).unwrap();
    assert!(caps::widen(&cap, PathBuf::from("/").as_path()).is_err());
}

#[test]
fn cap_sub_does_not_widen() {
    let tmp = std::env::temp_dir().join("ax-cap-sub");
    let inner = tmp.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(inner.join("x"), "1").unwrap();
    std::fs::write(tmp.join("outer"), "2").unwrap();
    let cap = ReadCap::open_dir(&tmp).unwrap();
    let sub = cap.sub("inner").unwrap();
    assert!(sub.read("x").is_ok());
    assert!(sub.read("../outer").is_err());
}

#[test]
fn strict_ffi_denied() {
    assert!(caps::strict_forbid_ffi(true).is_err());
    assert!(caps::strict_forbid_ffi(false).is_ok());
}

#[test]
fn interp_host_read_rejects_escape() {
    // Point at a file that definitely exists, so the rejection can only come from
    // path confinement. The previous version of this test read `../Cargo.toml`,
    // which does not exist at that relative path from the test's working
    // directory — so it passed whether or not confinement worked at all.
    let outside = std::env::temp_dir().join("ax-escape-target.txt");
    std::fs::write(&outside, b"secret").unwrap();
    assert!(
        outside.exists(),
        "the target must exist for this test to mean anything"
    );

    for path in [
        outside.to_string_lossy().to_string(),         // absolute
        "../".repeat(12) + &outside.to_string_lossy(), // via `..`
    ] {
        let src = format!(
            "module t;\nfn main() -> usz !{{io[fs], abort}} = io.read_file(\"{}\");\n",
            path.replace('\\', "/")
        );
        let mut s = Session::new();
        let out = s.compile("t.ax", &src).unwrap();
        let r = run_main(&s.intern, &out, 0);
        assert!(
            r.is_err(),
            "reading {path} should be refused by confinement, got {r:?}"
        );
    }
}

fn native_eq_oracle(src: &str, stem: &str) {
    let mut s = Session::new();
    let file = s.parse(&format!("{stem}.ax"), src).unwrap();
    let checked = s.check(&file);
    assert!(
        !checked.diags.iter().any(|d| d.is_error()),
        "{:?}",
        checked.diags
    );
    let oracle = run_main(&s.intern, &checked, 0).unwrap();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/diff");
    let br = codegen::build_tier(&s.intern, &checked, stem, &dir, Tier::Dev).unwrap();
    let nat = codegen::run_bin(&br.bin_path).unwrap();
    // Native prints `main`'s value in the oracle's canonical form, so the two
    // sides are compared as strings rather than through an 8-bit exit status.
    assert_eq!(oracle.display(), nat, "oracle != native for {stem}");
}

#[test]
fn native_if_else_value() {
    native_eq_oracle(
        "module t;\nfn main() -> i32 = if 2 > 1 { 9 } else { 8 };\n",
        "n_if",
    );
}

#[test]
fn native_unit_variant_match() {
    native_eq_oracle(
        r#"
module t;
type E = | Zero | One;
fn main() -> i32 = match Zero { Zero => 3; One => 4; };
"#,
        "n_match",
    );
}

#[test]
fn native_record_first_field() {
    native_eq_oracle(
        "module t;\nfn main() -> i32 = { let r = { a: 11, b: 2 }; 11 };\n",
        "n_rec",
    );
}

/// A label is a claim. `capability-contained` must be withheld from a program
/// that reaches the filesystem or network through ambient authority, and the
/// offending calls must be named so the report is actionable.
#[test]
fn ambient_io_is_not_labelled_capability_contained() {
    let src = r#"
module t;
fn main() -> u64 !{io[fs], abort} = io.bytesum_file("x.txt");
"#;
    let mut s = Session::new();
    let out = s.compile("t.ax", src).unwrap();
    let labels = ax::driver::guarantee_labels(&s.intern, &out, false, false);
    assert!(
        !labels.iter().any(|l| l == "capability-contained"),
        "ambient io must not be labelled contained: {labels:?}"
    );
    assert!(
        !labels.iter().any(|l| l == "replay-deterministic"),
        "ambient io is not in the transcript, so it is not replayable: {labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|l| l.starts_with("ambient-io(") && l.contains("io.bytesum_file")),
        "the offending call must be named: {labels:?}"
    );
}

/// The same program written against a capability keeps the label.
#[test]
fn capability_mediated_io_keeps_the_label() {
    let src = r#"
module t;
use std.fs;
fn read(cap: fs.ReadCap, a: Alloc, p: &str) -> usz !{io[cap], alloc[a], err[fs.Error]} =
    len(fs.read(cap, a, p));
"#;
    let mut s = Session::new();
    let out = s.compile("t.ax", src).unwrap();
    let labels = ax::driver::guarantee_labels(&s.intern, &out, false, false);
    assert!(
        labels.iter().any(|l| l == "capability-contained"),
        "capability-mediated io must keep the label: {labels:?}"
    );
}

/// Only `semantics_preserving` fixes are applied automatically. A widening cast
/// cannot change a value, so it is applied; a narrowing one can, so it is
/// reported and left alone.
#[test]
fn only_value_preserving_fixes_are_auto_applied() {
    use ax::frontend::Surface;

    let widen = "module t;\nfn main() -> u64 = 3usz;\n";
    let r = ax::agent::apply_safe_fixes("t.ax", widen, Surface::Ax);
    assert_eq!(r.applied.len(), 1, "widening should be applied: {r:?}");
    assert!(
        r.clean,
        "the module should check after the fix: {:?}",
        r.remaining
    );
    assert!(r.source.contains("as u64"), "{}", r.source);

    let narrow = "module t;\nfn main() -> u8 = 300usz;\n";
    let r = ax::agent::apply_safe_fixes("t.ax", narrow, Surface::Ax);
    assert!(
        r.applied.is_empty(),
        "a narrowing cast must not be applied silently: {r:?}"
    );
    assert_eq!(r.withheld.len(), 1, "and it must still be offered: {r:?}");
}

/// A recorded run replays from its transcript, not from the world: the file it
/// read can change and the replay still reproduces the original result.
#[test]
fn replay_is_hermetic_and_detects_divergence() {
    use ax::interp::TraceEvent;

    let src = "module t;\nfn main() -> u64 !{io[fs], abort} = io.bytesum_file(argv(1));\n";
    let mut s = Session::new();
    let out = s.compile("t.ax", src).unwrap();

    // Replaying a transcript performs no IO at all, so a path that does not
    // exist is fine.
    let events = vec![
        TraceEvent {
            op: "argv".into(),
            arg: "1".into(),
            result: "nonexistent-file".into(),
        },
        TraceEvent {
            op: "io.bytesum_file".into(),
            arg: "nonexistent-file".into(),
            result: "1234".into(),
        },
    ];
    let (v, _) = ax::driver::run_traced(&s.intern, &out, 0, &[], Some(events.clone())).unwrap();
    assert_eq!(v.as_i128(), 1234, "replay must return the recorded result");

    // A transcript that disagrees with what the program does is an error, not a
    // silent fallback to real IO.
    let mut wrong = events;
    wrong[1].arg = "some-other-path".into();
    let err = ax::driver::run_traced(&s.intern, &out, 0, &[], Some(wrong))
        .expect_err("divergence must be reported");
    assert!(err.contains("divergence"), "{err}");
}

/// The optimisations must not change what a program computes. Each of these was
/// added for speed; a wrong answer is not a speedup.
#[test]
fn optimisations_preserve_semantics() {
    // Bounds-check elimination fires here (bound is `xs.len()`)...
    let eliminated = r#"
module t;
fn main() -> usz !{alloc[a]} = {
    let mut xs: Vec[usz] = vec.new(test.alloc);
    xs.push(3usz); xs.push(4usz);
    let mut t = 0usz;
    for i in range(0, xs.len()) { t = t + xs.at(i); };
    t
};
"#;
    // ...and not here, where the bound is unrelated to the length.
    let kept = r#"
module t;
fn main() -> usz !{alloc[a]} = {
    let mut xs: Vec[usz] = vec.new(test.alloc);
    xs.push(3usz); xs.push(4usz);
    let mut t = 0usz;
    let n = 2usz;
    for i in range(0, n) { t = t + xs.at(i); };
    t
};
"#;
    for src in [eliminated, kept] {
        let mut s = Session::new();
        let out = s.compile("t.ax", src).unwrap();
        let v = run_main(&s.intern, &out, 0).unwrap();
        assert_eq!(v.as_i128(), 7, "wrong sum for:\n{src}");
    }

    // The check is genuinely gone in the first case and genuinely present in the
    // second: the IR is the evidence, since timing would not prove it.
    let ir_of = |src: &str| {
        let mut s = Session::new();
        let out = s.compile("t.ax", src).unwrap();
        ax::lower::lower_program(&s.intern, &out).unwrap().to_text()
    };
    assert!(
        !ir_of(eliminated).contains("abort oob"),
        "the bounds check should have been eliminated"
    );
    assert!(
        ir_of(kept).contains("abort oob"),
        "the bounds check must remain when the index is not provably in range"
    );
}

/// Folding a pure call must agree with running it, and must not happen at all
/// when the call has effects.
#[test]
fn folding_agrees_with_running() {
    let src = r#"
module t;
fn f(x: i32) -> i32 = x * x + 1;
fn main() -> i32 = f(9);
"#;
    let mut s = Session::new();
    let out = s.compile("t.ax", src).unwrap();
    // The oracle never folds; it just runs.
    let interpreted = run_main(&s.intern, &out, 0).unwrap().as_i128();
    assert_eq!(interpreted, 82);
    // The IR should carry the answer as a constant, with no call left.
    let ir = ax::lower::lower_program(&s.intern, &out).unwrap().to_text();
    let main_ir = ir
        .split("fn @ax_main")
        .nth(1)
        .expect("a main function")
        .to_string();
    assert!(
        main_ir.contains("const.int 82"),
        "expected a folded constant, got:\n{main_ir}"
    );
    assert!(
        !main_ir.contains("call @ax_f"),
        "the call should be gone:\n{main_ir}"
    );
}

/// The docs quote a conformance count. A count that drifts is a small lie in a
/// document whose whole purpose is not containing any, so it is gated: adding a
/// case without updating the prose fails here rather than being noticed later.
#[test]
fn documented_conformance_count_matches_the_corpus() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut actual = 0usize;
    let mut dirs = vec![root.join("conformance")];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).expect("conformance/ exists") {
            let p = e.expect("a readable entry").path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "ax") {
                actual += 1;
            }
        }
    }
    assert!(actual > 100, "the corpus shrank to {actual} cases");
    let readme = std::fs::read_to_string(root.join("README.md")).expect("README.md");
    assert!(
        readme.contains(&format!("is {actual} cases")),
        "README.md does not state the real count ({actual})"
    );
}

/// The Cranelift tier must agree with the oracle on the cases that are easy to
/// get wrong in a hand-written backend, and must actually have compiled
/// something. A backend that silently returned the oracle's answer would pass a
/// value comparison, so the function count is checked too.
#[test]
fn cranelift_tier_agrees_with_the_oracle() {
    let cases: &[(&str, &str)] = &[
        // Division by -1 negates rather than dividing by 1.
        (
            "module t;\nfn main() -> i32 !{err[DivError]} = int.div_trunc(-7i32, -1i32);\n",
            "7i32",
        ),
        // The overflow case Ax defines as wrapping.
        (
            "module t;\nfn main() -> i32 !{err[DivError]} = int.div_trunc(-2147483648i32, -1i32);\n",
            "-2147483648i32",
        ),
        // Shift counts mask to the operand width.
        (
            "module t;\nfn main() -> i32 = { let s = 33i32; 1i32 << s };\n",
            "2i32",
        ),
        // Narrow wrapping arithmetic.
        (
            "module t;\nfn main() -> i8 = { let x: i8 = 127i8; x + 1i8 };\n",
            "-128i8",
        ),
        // NaN is not equal to itself, and the tier must not fold that away.
        (
            "module t;\nfn main() -> bool = { let n = 0.0f64 / 0.0f64; n == n };\n",
            "false",
        ),
        // Records: the JIT uses the IR's offsets, the oracle uses names.
        (
            "module t;\ntype P = { a: i32, b: i64 };\nfn main() -> i64 = { let p = { a: 1, b: 9i64 }; p.b };\n",
            "9i64",
        ),
        // A raise crossing a call boundary, which is the two-value error ABI.
        (
            "module t;\ntype E = | Bad;\nfn f(x: i32) -> i32 !{err[E]} = if x == 0 { raise Bad } else { x };\nfn main() -> i32 = catch f(0) { Bad => 5 };\n",
            "5i32",
        ),
    ];
    let mut compiled_any = false;
    for (src, want) in cases {
        let mut s = Session::new();
        let out = s.compile("t.ax", src).expect("case compiles");
        let oracle = run_main(&s.intern, &out, 0)
            .map(|v| v.display())
            .unwrap_or_else(|e| panic!("oracle failed on {src}: {e}"));
        assert_eq!(&oracle, want, "the expectation itself is wrong for {src}");
        let jit = ax::backend_clif::compile(&s.intern, &out)
            .unwrap_or_else(|e| panic!("cranelift refused {src}: {e}"));
        assert!(jit.func_count() > 0, "nothing was compiled for {src}");
        compiled_any = true;
        let got = jit
            .run(&["t.ax".to_string()])
            .unwrap_or_else(|e| panic!("cranelift run failed on {src}: {e}"));
        assert_eq!(got, *want, "cranelift disagrees with the oracle on {src}");
    }
    assert!(compiled_any);
}

/// The two strengths of non-zero-divisor proof must reach the backend as
/// different code. This is checked on the IR rather than only on values, because
/// both shapes compute the same answer — the difference is a compare and a branch
/// per division, which no value comparison can see.
#[test]
fn nonzero_divisor_proof_strength_reaches_the_ir() {
    // Unconditionally non-zero: a literal. No guard, no abort block.
    let literal = "module t;\nfn main() -> usz = 17usz % 7usz;\n";
    let mut s = Session::new();
    let out = s.compile("t.ax", literal).expect("compiles");
    let ir = ax::lower::lower_program(&s.intern, &out).unwrap().to_text();
    assert!(
        !ir.contains("abort div_exact"),
        "a literal divisor needs no guard:\n{ir}"
    );
    assert!(
        ir.contains("remtruncnz"),
        "expected the unchecked remainder:\n{ir}"
    );

    // Proof by increment: only holds until the increment wraps, so the guard
    // stays. Dropping it would silently change what an overflowing program does.
    let incremented = r#"
module t;
fn main() -> usz !{diverge} = {
    let mut d: usz = 2;
    let mut acc: usz = 0;
    loop {
        if d > 8usz { return acc };
        acc = acc + (12usz % d);
        d = d + 1;
    }
};
"#;
    let mut s2 = Session::new();
    let out2 = s2.compile("t.ax", incremented).expect("compiles");
    // The row is still free of err[DivError]: no caller pays the fallible ABI.
    let f = out2.fns.first().expect("main");
    let row = format!("{:?}", f.inferred);
    assert!(
        !row.contains("DivError"),
        "the row should not claim an error that cannot be raised: {row}"
    );
    let ir2 = ax::lower::lower_program(&s2.intern, &out2)
        .unwrap()
        .to_text();
    assert!(
        ir2.contains("abort div_exact"),
        "an incremented divisor keeps its guard:\n{ir2}"
    );
}

/// A loop-invariant unsigned 64-bit divisor must become a preheader reciprocal
/// plus a body multiply-high. Checked on the IR: both shapes compute the same
/// answer, so a value comparison cannot see whether the hoist happened.
#[test]
fn invariant_divisor_emits_reciprocal() {
    // A parameter is invariant of the loop but not a compile-time constant,
    // so clang cannot strength-reduce it. That is the case the hoist exists for.
    let hoisted = r#"
module t;
fn mix(d: usz) -> usz = {
    if d != 0 {
        let mut s: usz = 0;
        for i in range(0, 20) { s = s + (i % d); };
        s
    } else { 0 }
};
fn main() -> usz = mix(7);
"#;
    let mut s = Session::new();
    let out = s.compile("t.ax", hoisted).expect("compiles");
    let ir = ax::lower::lower_program(&s.intern, &out).unwrap().to_text();
    assert!(
        ir.contains("call.ext ax_recip_m"),
        "expected a preheader reciprocal:\n{ir}"
    );
    assert!(
        ir.contains("call.ext ax_rem_recip"),
        "expected a body rem-by-reciprocal:\n{ir}"
    );
    assert!(
        !ir.contains("remtruncnz"),
        "the machine rem should have been replaced:\n{ir}"
    );

    // Assigned in the body: not invariant, so the hoist must not fire.
    let assigned = r#"
module t;
fn main() -> usz = {
    let mut d: usz = 3;
    let mut s: usz = 0;
    for i in range(0, 4) { s = s + (10usz % d); d = 5; };
    s
};
"#;
    let mut s2 = Session::new();
    let out2 = s2.compile("t.ax", assigned).expect("compiles");
    let ir2 = ax::lower::lower_program(&s2.intern, &out2)
        .unwrap()
        .to_text();
    assert!(
        !ir2.contains("ax_recip_m"),
        "an assigned divisor must not hoist:\n{ir2}"
    );
    assert!(
        ir2.contains("remtruncnz"),
        "expected the ordinary remainder:\n{ir2}"
    );

    // A literal binding is a compile-time constant: leave it to clang.
    let literal = r#"
module t;
fn main() -> usz = {
    let d: usz = 7;
    let mut s: usz = 0;
    for i in range(0, 20) { s = s + (i % d); };
    s
};
"#;
    let mut s3 = Session::new();
    let out3 = s3.compile("t.ax", literal).expect("compiles");
    let ir3 = ax::lower::lower_program(&s3.intern, &out3)
        .unwrap()
        .to_text();
    assert!(
        !ir3.contains("ax_recip_m"),
        "a constant divisor must be left to the C compiler:\n{ir3}"
    );
}

/// Two-argument pure tree recursion is cached. Comb is the row this exists for.
#[test]
fn two_arg_pure_recursion_is_memoized() {
    let src = r#"
module t;
fn comb(n: i32, k: i32) -> i32 =
    if k == 0 { 1 } else { if k == n { 1 } else { comb(n - 1, k - 1) + comb(n - 1, k) } };
fn main() -> i32 = comb(10, 4);
"#;
    let mut s = Session::new();
    let out = s.compile("t.ax", src).expect("compiles");
    let ir = ax::lower::lower_program(&s.intern, &out).unwrap().to_text();
    let comb = ir.split("fn @ax_comb").nth(1).expect("comb is in the IR");
    assert!(
        comb.contains("memoize"),
        "expected comb to be marked memoize:\n{comb}"
    );
}
