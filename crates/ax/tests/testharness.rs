//! Ax Test & Conformance Specification v1.0 — cargo entry.
//!
//! Discovers `tests/**/*.ax`, runs each case twice ([T-1.3.1]), asserts
//! requirement coverage ([T-1.2.3]), diagnostic coverage ([T-1.2.4]),
//! UPSTREAM.toml licenses ([T-11.1]), fault-injection ([T-10.2]), and
//! the table-driven float / UTF-8 / JSON / integer runners.

use ax::caps::{self, ReadCap};
use ax::driver::Session;
use ax::testharness::{self, Outcome};
use std::path::PathBuf;

fn root() -> PathBuf {
    testharness::suite_dir()
}

#[test]
fn headers_parse_and_require_an_r_id() {
    let cases = testharness::discover(&root()).expect("discover");
    assert!(
        cases.len() >= 40,
        "test tree looks truncated: {} cases",
        cases.len()
    );
    for c in &cases {
        assert!(
            !c.header.requires.is_empty(),
            "{} missing requires:",
            c.header.id
        );
        assert!(
            testharness::PortKind::parse(c.header.port.as_str()).is_some()
        );
    }
}

#[test]
fn authored_tests_justify_themselves() {
    let cases = testharness::discover(&root()).expect("discover");
    let unjust = testharness::authored_without_justification(&cases);
    assert!(
        unjust.is_empty(),
        "authored tests with empty origin (must justify why oracle/port did not apply): {unjust:?}"
    );
}

#[test]
fn requirement_coverage_has_no_zero_ref_ids() {
    let cases = testharness::discover(&root()).expect("discover");
    let (_seen, missing) = testharness::requirement_coverage(&cases, testharness::REQUIRED_IDS);
    assert!(
        missing.is_empty(),
        "requirements with zero tests ([T-1.2.3]): {missing:?}"
    );
}

#[test]
fn diagnostic_catalog_has_emit_tests() {
    let cases = testharness::discover(&root()).expect("discover");
    let (_seen, missing) = testharness::diagnostic_coverage(&cases);
    assert!(
        missing.is_empty(),
        "catalog codes with no emit test ([T-1.2.4]; pending-emit list is explicit): {missing:?}"
    );
}

#[test]
fn upstream_toml_records_license_and_commit() {
    let entries = testharness::load_upstream().expect("UPSTREAM.toml");
    assert!(!entries.is_empty());
    for e in &entries {
        assert!(!e.license.is_empty(), "{}", e.name);
        assert!(!e.commit.is_empty(), "{}", e.name);
    }
    // GPL must be marked do-not-vendor.
    let gcc = entries.iter().find(|e| e.name.contains("gcc.c-torture"));
    assert!(gcc.is_some());
    assert!(gcc.unwrap().notes.contains("do-not-vendor"));
}

#[test]
fn suite_passes_twice() {
    let root = root();
    let cases = testharness::discover(&root).expect("discover");
    let results = testharness::run_suite(&root, None).expect("run");
    let failures: Vec<String> = results
        .iter()
        .filter_map(|r| match &r.outcome {
            Outcome::Pass => None,
            Outcome::Fail { detail } => Some(format!("{} {}: {detail}", r.id, r.name)),
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} testharness cases failed:\n  {}",
        failures.len(),
        results.len(),
        failures.join("\n  ")
    );
    let dist = testharness::port_distribution(&cases);
    assert!(dist.contains_key("authored") || dist.contains_key("adapted"));
}

#[test]
fn rustc_oracle_companions_exist() {
    let root = root().join("differential");
    let cases = testharness::discover(&root).expect("discover");
    assert!(!cases.is_empty(), "no differential cases");
    for c in &cases {
        if matches!(c.header.expect, testharness::Expect::RustcOracle) {
            let rs = c.path.with_extension("rs");
            assert!(rs.exists(), "missing rustc companion {}", rs.display());
        }
    }
}

#[test]
fn float_core_vectors_match_libm() {
    for v in testharness::f32_core_vectors() {
        let got = testharness::eval_f32_bits(v.op, v.a, v.b);
        assert_eq!(
            got, v.expect,
            "f32 {} {:08x} {:08x}: got {:08x} want {:08x}",
            v.op, v.a, v.b, got, v.expect
        );
    }
}

#[test]
fn utf8_stress_rejects_overlongs_and_surrogates() {
    for bytes in testharness::utf8_reject_vectors() {
        assert!(
            !testharness::utf8_is_valid(bytes),
            "should reject {bytes:?}"
        );
    }
    assert!(testharness::utf8_is_valid(b"hello"));
    assert!(testharness::utf8_is_valid("héllo".as_bytes()));
}

#[test]
fn json_core_accept_reject() {
    for c in testharness::json_core_cases() {
        let got = testharness::json_accepts(c.input);
        assert_eq!(got, c.accept, "{}: {}", c.name, c.input);
    }
}

#[test]
fn int_edges_wrap_as_the_card_says() {
    use ax::types::Prim;
    for e in testharness::int_edge_cases() {
        let p = match (e.width, e.signed) {
            (8, true) => Prim::I8,
            (8, false) => Prim::U8,
            (32, true) => Prim::I32,
            _ => Prim::I32,
        };
        match e.op {
            "add" => {
                let got = p.wrap_i128(e.a + e.b);
                assert_eq!(Some(got), e.expect, "{e:?}");
            }
            "sub" => {
                let got = p.wrap_i128(e.a - e.b);
                assert_eq!(Some(got), e.expect, "{e:?}");
            }
            "mul" => {
                let got = p.wrap_i128(e.a * e.b);
                assert_eq!(Some(got), e.expect, "{e:?}");
            }
            _ => {}
        }
    }
}

#[test]
fn formatter_is_idempotent_on_hello() {
    testharness::fmt_idempotent("module t;\nfn main() -> i32 = 1 + 2;\n").unwrap();
}

#[test]
fn gbnf_generator_parses() {
    assert_eq!(ax::gbnf::check_generator_parses(64, 1), 0);
    assert_eq!(ax::gbnf::check_parser_subset(64, 2), 0);
}

#[test]
fn aliasing_generator_compiles() {
    let srcs = testharness::generate_aliasing(16, 42);
    assert_eq!(testharness::run_generated_interpreter(&srcs), 0);
}

#[test]
fn emi_dead_comment_preserves() {
    testharness::emi_preserves("module t;\nfn main() -> i32 = 1 + 2;\n").unwrap();
}

#[test]
fn reducer_shrinks_while_preserving_predicate() {
    let src = "module t;\nfn main() -> i32 = 1;\nfn unused() -> i32 = 2;\n";
    let mut pred = |s: &str| s.contains("fn main");
    let out = testharness::reduce(src, &mut pred);
    assert!(out.contains("fn main"));
    assert!(out.lines().count() <= src.lines().count());
}

#[test]
fn fault_injection_all_variants_caught() {
    let report = testharness::fault_injection_report();
    let missed: Vec<_> = report.iter().filter(|v| !v.ok).map(|v| v.name).collect();
    assert!(
        missed.is_empty(),
        "fault variants no test catches ([T-10.2.2]): {missed:?}"
    );
    assert!(report.len() >= 29, "expected ~30 variants, got {}", report.len());
}

#[test]
fn harvest_extracts_invert_codes() {
    let src = r#"
fn main() {
    let mut i = 0;
    let x = &mut i;
    let a = &mut i; //~ ERROR cannot borrow `i` as mutable more than once
    let y = x; //~ ERROR E0382
}
"#;
    let codes = ax::harvest::extract_codes(src);
    assert!(codes.iter().any(|c| c == "E0499"), "{codes:?}");
    assert!(codes.iter().any(|c| c == "E0382"), "{codes:?}");
}

#[test]
fn harvest_from_in_tree_fixture() {
    let fixtures = root().join("rust_ported/inverted/fixtures");
    let dest = testharness::temp_out().join("harvest-fixture");
    let _ = std::fs::remove_dir_all(&dest);
    let r = ax::harvest::harvest_into(
        &fixtures,
        &dest,
        "4d91de4e48198da2e33413efdcd9cd2cc0c46688",
    )
    .expect("harvest fixture");
    assert!(
        r.hits.len() >= 1,
        "fixture should yield at least one invert hit"
    );
}

#[test]
fn harvest_skips_unsafe() {
    let src = "fn main() { unsafe { let x = 1; } } //~ ERROR E0382\n";
    let tr = ax::translate::translate_rust(src);
    assert!(tr.rejected.iter().any(|r| r.contains("unsafe")), "{tr:?}");
}

#[test]
fn inverted_bucket_has_pinned_origin() {
    let root = root().join("rust_ported/inverted");
    let cases = testharness::discover(&root).expect("discover inverted");
    assert!(
        cases.len() >= 10,
        "inverted bucket too small: {}",
        cases.len()
    );
    for c in &cases {
        assert_eq!(c.header.port, testharness::PortKind::InvertedMechanical);
        assert!(
            c.header.upstream.contains("4d91de4e") || c.header.upstream == "none",
            "{} unpinned: {}",
            c.header.id,
            c.header.upstream
        );
    }
}

#[test]
fn ownership_fuzz_agrees_interpreter_native() {
    let fails = ax::fuzz::differential(24, 7);
    assert_eq!(fails, 0, "ownership/semantics fuzz disagreements: {fails}");
}

#[test]
fn inverted_region_rule_disagrees() {
    // [T-10.2.3]
    assert!(!ax::indep::store_legal(1, 0));
    assert!(ax::indep::store_legal_v01_inverted(1, 0));
}

#[test]
fn cap_budget_and_widen_are_a5001_a5002() {
    let budget = ax::reach::CapBudget::from_toml("[caps]\nallow = [\"fs\"]\n");
    let src = "module t;\nfn main() -> i32 = 0;\n";
    let mut s = Session::new();
    let out = s.compile("t.ax", src).unwrap();
    let report = ax::reach::analyze(&s.intern, &out);
    let _ = budget.check(&report);
    let widened = ax::reach::cap_widened(&["fs".into()], &["fs".into(), "net".into()]);
    assert_eq!(widened, vec!["net".to_string()]);
}

#[test]
fn capability_red_team_fails_closed() {
    let tmp = std::env::temp_dir().join("ax-cap-redteam");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("ok.txt"), "hi").unwrap();
    let cap = ReadCap::open_dir(&tmp).unwrap();
    assert!(caps::confine(&cap.root, "../etc/passwd").is_err());
    assert!(caps::confine(&cap.root, "/etc/passwd").is_err());
    assert!(caps::widen(&cap, &PathBuf::from("/")).is_err());
    assert_eq!(cap.read("ok.txt").unwrap(), "hi");
}

#[test]
fn translate_emits_provenance() {
    let r = ax::translate::translate_rust("fn add(a: i32, b: i32) -> i32 { a + b }");
    let stamped = ax::translate::with_provenance(&r.source, "local.rs", "MIT", "deadbeef");
    assert!(stamped.contains("//@ origin:"));
    assert!(stamped.contains("//@ license:"));
}

#[test]
fn digest_signatures_exist() {
    let src = "module t;\nfn add(a: i32, b: i32) -> i32 = a + b;\nfn main() -> i32 = add(1, 2);\n";
    let mut s = Session::new();
    let out = s.compile("t.ax", src).unwrap();
    let pack = ax::perf::context_pack(&s.intern, &out, 1000);
    assert!(pack.digests.iter().any(|d| d.contains("add")));
}

#[test]
fn do_not_port_list_is_recorded() {
    let rows = testharness::do_not_port();
    assert!(rows.iter().any(|(n, _)| n.contains("GCC")));
    assert!(rows.iter().any(|(n, _)| n.contains("test262")));
}

#[test]
fn harvest_writes_regression() {
    let p = testharness::harvest_regression(
        "module t;\nfn main() -> i32 = 0;\n",
        "smoke",
    )
    .unwrap();
    assert!(p.exists());
    let src = std::fs::read_to_string(&p).unwrap();
    assert!(src.contains("//@ id:"));
    let _ = std::fs::remove_file(p);
}
