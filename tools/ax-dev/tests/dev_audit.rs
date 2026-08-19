use ax_dev::{axmock, evalloop, fuzz, gbnf_check, tokens, translate};

#[test]
fn proxy_tokenizer_survives_non_ascii() {
    let count = tokens::count("a × b -> c\n");
    assert_eq!(count.tokens, 6);
    assert_eq!(tokens::count("→").tokens, 1);
    let prose = "1.02× the wall time → parity";
    for index in 0..=prose.len() {
        if prose.is_char_boundary(index) {
            let _ = tokens::count(&prose[..index]);
        }
    }
}

#[test]
fn ax_mock_accepts_restricted_rust() {
    assert!(axmock::PROMPT.contains("no lifetimes"));
    let src = "module t;\nfn main() -> i32 = 1 + 2;\n";
    assert!(axmock::validity(src));
    assert!(axmock::score_corpus(&[src]) > 0.9);
    assert!(axmock::m12_sample_score() >= 0.8);
}

#[test]
fn ax_mock_n200_validity() {
    let corpus = axmock::generated_corpus(200, 42);
    assert_eq!(corpus.len(), 200);
    let score = axmock::score_corpus(&corpus);
    assert!(score >= 0.95, "M12 sample score {score}");
}

#[test]
fn fuzz_oracle_vs_native_small() {
    let report = fuzz::differential_report(32, 7);
    assert_eq!(
        report.fails,
        0,
        "{} disagreements:\n{}",
        report.fails,
        report.details.join("\n")
    );
}

#[test]
fn attempts_to_green_fills_a_hole_against_a_test() {
    let src = r#"
module t;
type Vec2 = { x: f32, y: f32 };
fn distance(v: Vec2) -> f32 = ?;
test "3-4-5" = assert(distance(Vec2 { x: 3.0f32, y: 4.0f32 }) == 5.0f32);
"#;
    let result = evalloop::attempts_to_green("t.ax", src, ax_dev::frontend::Surface::Conventional);
    assert!(result.green, "the loop should reach green: {result:?}");
    assert_eq!(result.holes, 1);
    assert!(result.applied.iter().any(|fill| fill.contains("hypot")));
    assert!(result.attempts <= 6, "too many cycles: {result:?}");
}

#[test]
fn translate_strips_rust_noise() {
    let rust = r#"
pub fn add<'a>(x: &'a i32, y: &'a i32) -> i32 {
    let z = Box::new(*x);
    z.clone() + *y
}
"#;
    let report = translate::translate_rust(rust);
    assert!(!report.source.contains("Box::new"), "{}", report.source);
    assert!(!report.source.contains(".clone()"), "{}", report.source);
    assert!(!report.source.contains("'a"), "{}", report.source);
    assert!(report
        .notes
        .iter()
        .any(|note| note.contains("lifetime") || note.contains("Box") || note.contains("clone")));
}

#[test]
fn translate_rejects_unknown_macros() {
    let rust = r#"fn f() { println!("hi"); todo!("no"); }"#;
    let report = translate::translate_rust(rust);
    assert!(
        report.rejected.iter().any(|item| item.contains("todo")),
        "{:?}",
        report.rejected
    );
}

#[test]
fn gbnf_generated_strings_parse() {
    let generated = gbnf_check::check_generator_parses(200, 42);
    assert_eq!(generated, 0, "generator produced {generated} bad strings");
    let roundtrip = gbnf_check::check_parser_subset(200, 7);
    assert_eq!(
        roundtrip, 0,
        "format round-trip failed on {roundtrip} strings"
    );
}

#[test]
fn gbnf_equivalence_1k() {
    assert_eq!(gbnf_check::check_equivalence(1000), (0, 0));
}
