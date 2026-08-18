//! M4 four-way differential (oracle / dev / release / portable) + M5 packs.

use ax::codegen::{self, Tier};
use ax::driver::{run_main, Session};
use ax::packages::{self, PackKind};
use std::path::PathBuf;

fn native_out(src: &str, stem: &str, tier: Tier) -> String {
    let mut s = Session::new();
    let file = s.parse(&format!("{stem}.ax"), src).unwrap();
    let checked = s.check(&file);
    assert!(
        !checked.diags.iter().any(|d| d.is_error()),
        "{:?}",
        checked.diags
    );
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/diff");
    let br = codegen::build_tier(&s.intern, &checked, stem, &dir, tier)
        .unwrap_or_else(|e| panic!("{e}"));
    codegen::run_bin(&br.bin_path).unwrap_or_else(|e| panic!("{e}"))
}

fn oracle_out(src: &str) -> String {
    let mut s = Session::new();
    let out = s.compile("t.ax", src).unwrap();
    // Native renders `main`'s value in this same canonical form, so all four
    // tiers are compared as strings.
    run_main(&s.intern, &out, 0).unwrap().display()
}

fn four_way(src: &str, stem: &str) {
    let o = oracle_out(src);
    let d = native_out(src, stem, Tier::Dev);
    let r = native_out(src, stem, Tier::Release);
    let p = native_out(src, stem, Tier::Portable);
    assert_eq!(o, d, "oracle != dev");
    assert_eq!(o, r, "oracle != release");
    assert_eq!(o, p, "oracle != portable");
}

#[test]
fn diff_add() {
    four_way("module t;\nfn main() -> i32 = 1 + 2;\n", "d_add");
}

#[test]
fn diff_if() {
    four_way(
        "module t;\nfn main() -> i32 = if 1 == 1 { 7 } else { 8 };\n",
        "d_if",
    );
}

#[test]
fn diff_call() {
    four_way(
        "module t;\nfn add(a: i32, b: i32) -> i32 = a + b;\nfn main() -> i32 = add(10, 32);\n",
        "d_call",
    );
}

#[test]
fn diff_for() {
    four_way(
        r#"
module t;
fn main() -> i32 = {
    let mut s: i32 = 0;
    for i in range(0, 5) {
        s = s + 1;
    };
    s
};
"#,
        "d_for",
    );
}

#[test]
fn diff_wrap() {
    four_way("module t;\nfn main() -> i32 = 40 + 2;\n", "d_wrap");
}

#[test]
fn packs_builtin_set() {
    let names: Vec<_> = packages::builtin_packs()
        .into_iter()
        .map(|p| p.name)
        .collect();
    for n in [
        "core",
        "alloc",
        "str",
        "fmt",
        "collections",
        "json",
        "fs",
        "test",
    ] {
        assert!(names.contains(&n.to_string()), "missing builtin {n}");
    }
}

#[test]
fn packs_components_reserved() {
    let stubs = packages::component_stubs();
    assert!(stubs.iter().all(|p| p.kind == PackKind::Component));
    assert!(stubs.iter().any(|p| p.name == "net"));
    assert!(stubs.iter().any(|p| p.name == "crypto"));
}

#[test]
fn packs_round_trip_manifest() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/packs");
    let written = packages::write_registry(&dir).unwrap();
    assert_eq!(written.len(), 13);
    let core = packages::load_pack(&dir, "core").unwrap();
    assert_eq!(core.name, "core");
    assert_eq!(core.kind, PackKind::Builtin);
    assert!(!core.source_hash.is_empty());
}

#[test]
fn pack_list_mentions_deferred() {
    let t = packages::list_text();
    assert!(t.contains("crypto"));
    assert!(t.contains("reserved"));
}
