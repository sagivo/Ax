//! Compiler protocol surface (§10.7) and spec-integrity tests.

use ax::driver::Session;
use std::process::Command;

fn ax_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ax")
}

fn workspace_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn example(name: &str) -> String {
    workspace_root()
        .join("examples")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn run_ax(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(ax_bin())
        .args(args)
        .current_dir(workspace_root())
        .output()
        .expect("spawn ax");
    (
        out.status.code().unwrap_or(255),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn cli_check_hello() {
    let (c, stdout, stderr) = run_ax(&["check", &example("hello.ax")]);
    assert_eq!(c, 0, "{stderr}");
    assert!(stdout.contains("ok"), "{stdout}");
}

#[test]
fn cli_check_json() {
    let (c, stdout, _) = run_ax(&["check", "--json", &example("hello.ax")]);
    assert_eq!(c, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["ok"], true);
}

#[test]
fn cli_run_hello() {
    let (c, stdout, stderr) = run_ax(&["run", &example("hello.ax")]);
    assert_eq!(c, 0, "{stderr}");
    assert!(stdout.contains("3"), "{stdout}");
}

#[test]
fn cli_test_div() {
    let (c, stdout, stderr) = run_ax(&["test", &example("div.ax")]);
    assert_eq!(c, 0, "{stderr}\n{stdout}");
    assert!(stdout.contains("passed"), "{stdout}");
}

#[test]
fn cli_test_loader() {
    let (c, stdout, stderr) = run_ax(&["test", &example("loader.ax")]);
    assert_eq!(c, 0, "{stderr}\n{stdout}");
}

#[test]
fn cli_fmt_idempotent() {
    let (c1, a, e1) = run_ax(&["fmt", &example("hello.ax")]);
    assert_eq!(c1, 0, "{e1}");
    let tmp = std::env::temp_dir().join("ax-fmt-hello.ax");
    std::fs::write(&tmp, &a).unwrap();
    let (c2, b, e2) = run_ax(&["fmt", tmp.to_str().unwrap()]);
    assert_eq!(c2, 0, "{e2}");
    assert_eq!(a, b);
}

#[test]
fn cli_hole() {
    let (c, stdout, stderr) = run_ax(&["hole", &example("holes.ax")]);
    assert_eq!(c, 0, "{stderr}");
    assert!(stdout.contains("expects: f32"), "{stdout}");
}

#[test]
fn cli_card() {
    let (c, stdout, _) = run_ax(&["card"]);
    assert_eq!(c, 0);
    assert!(stdout.contains("Ax card"));
    assert!(stdout.contains("E0200"));
}

#[test]
fn cli_label() {
    let (c, stdout, stderr) = run_ax(&["label", &example("hello.ax")]);
    assert_eq!(c, 0, "{stderr}");
    assert!(stdout.contains("safe"));
    assert!(stdout.contains("deterministic-core"));
}

#[test]
fn cli_errs_into() {
    let (c, stdout, stderr) = run_ax(&["errs", "--into", "LoadError", &example("loader.ax")]);
    assert_eq!(c, 0, "{stderr}");
    assert!(
        stdout.contains("from") || stdout.contains("LoadIo") || stdout.contains("fs.Error"),
        "{stdout}"
    );
}

#[test]
fn cli_types_and_effs() {
    let (c, stdout, stderr) = run_ax(&["types", &example("div.ax"), "div"]);
    assert_eq!(c, 0, "{stderr}");
    assert!(stdout.contains("i32"), "{stdout}");
    let (c, stdout, stderr) = run_ax(&["effs", &example("div.ax"), "div"]);
    assert_eq!(c, 0, "{stderr}");
    assert!(stdout.contains("err"), "{stdout}");
}

#[test]
fn cli_run_rejects_holes() {
    let (c, _, stderr) = run_ax(&["run", &example("holes.ax")]);
    assert_ne!(c, 0);
    assert!(
        stderr.contains("hole") || stderr.contains("E0500") || stderr.contains("error"),
        "{stderr}"
    );
}

#[test]
fn cli_search() {
    let (c, stdout, stderr) = run_ax(&["search", &example("div.ax"), "div"]);
    assert_eq!(c, 0, "{stderr}");
    assert!(stdout.contains("div"), "{stdout}");
}

#[test]
fn patch_tx_fails_on_hash_mismatch() {
    let dir = std::env::temp_dir();
    let src = dir.join("ax-patch-src.ax");
    let tx = dir.join("ax-patch-tx.json");
    std::fs::write(&src, "module t;\nfn main() -> i32 = 1;\n").unwrap();
    std::fs::write(
        &tx,
        r#"{"base_module_hash":"deadbeef","def_id":"t::fn:main","path":["body"],"expected_subtree_hash":"abc","replacement_ast":{}}"#,
    )
    .unwrap();
    let (c, _, stderr) = run_ax(&["patch", "--tx", tx.to_str().unwrap(), src.to_str().unwrap()]);
    assert_ne!(c, 0, "expected mismatch fail, got ok. stderr={stderr}");
}

#[test]
fn native_build_hello() {
    // A unique output path: this used to carry two `#[test]` attributes, so it
    // ran twice concurrently and the two runs raced on one binary.
    let out = std::env::temp_dir().join("ax-hello-bin-native-build");
    let hello = example("hello.ax");
    let (c, stdout, stderr) = run_ax(&["build", "-o", out.to_str().unwrap(), &hello]);
    assert_eq!(c, 0, "{stderr}");
    assert!(out.exists() || stdout.contains("hello"), "{stdout}{stderr}");
}

/// Snippets shown in the spec must compile. This lost its `#[test]` attribute at
/// some point and silently stopped running.
#[test]
fn spec_snippets_compile() {
    let snippets = [
        r#"
module t;
fn distance(v: Vec2) -> f32 = math.hypot(v.x, v.y);
type Vec2 = { x: f32, y: f32 };
fn main() -> f32 = distance({ x: 3.0, y: 4.0 });
"#,
        r#"
module t;
type DivError = | Zero;
fn div(a: i32, b: i32) -> i32 !{err[DivError]}
= if b == 0 { raise Zero } else { int.div_trunc(a, b) };
fn main() -> i32 = catch div(4, 2) { Zero => 0 };
"#,
        r#"
module t;
fn add(a: i32, b: i32) -> i32 = a + b;
fn main() -> i32 = add(1, 2);
"#,
        r#"
module t;
fn main() -> i32 = region r { 7 };
"#,
        // `par` is deliberately rejected in v1 (no disjointness proof), so the
        // snippet that used it was replaced with one exercising the container
        // stdlib, which the spec does document as available.
        r#"
module t;
fn main() -> usz !{alloc[a], diverge} = {
    let mut xs: Vec[usz] = vec.new(test.alloc);
    xs.push(2usz);
    xs.push(1usz);
    sort(&mut xs, asc);
    xs.at(0)
};
fn asc(x: &usz, y: &usz) -> Ordering = if x < y { Lt } else { if x > y { Gt } else { Eq } };
"#,
    ];
    for (i, src) in snippets.iter().enumerate() {
        let mut s = Session::new();
        s.compile(&format!("snip{i}.ax"), src)
            .unwrap_or_else(|d| panic!("snippet {i} failed: {d:#?}"));
    }
}

#[test]
fn card_snippets_are_descriptive_not_runnable_blocks() {
    let card = ax::driver::card_text();
    assert!(card.contains("#add(") || card.contains("#fn") || card.contains("#name("));
    assert!(card.contains("raise") && card.contains("catch"));
    assert!(card.contains("short syntax"));
}

#[test]
fn region_store_rule_rejects_escape() {
    let src = r#"
module t;
fn escape() -> &r i32 = region r {
    let x: i32 = 1;
    &x
};
"#;
    let mut s = Session::new();
    match s.compile("r.ax", src) {
        Ok(_) => panic!("escaped region must be rejected"),
        Err(d) => assert!(d.iter().any(|x| x.code == "E0302" || x.code == "E0101")),
    }
}

#[test]
fn effect_free_cannot_print() {
    let src = r#"
module t;
fn pure() -> unit = print("no");
"#;
    let mut s = Session::new();
    match s.compile("e.ax", src) {
        Ok(_) => panic!("io from effect-free must fail"),
        Err(d) => assert!(d.iter().any(|x| x.code == "E0200")),
    }
}
