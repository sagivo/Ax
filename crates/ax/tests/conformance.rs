//! The conformance corpus as a cargo test.
//!
//! Every case under `conformance/` runs on the oracle, both C tiers, and the
//! Cranelift JIT. One test per case would be nicer to read, but cases are
//! discovered at runtime, so the suite is one test that reports every failure at
//! once — failing fast would hide how much is broken.

use ax::conform::{self, Outcome};

#[test]
fn conformance_suite_passes_on_every_tier() {
    let root = conform::suite_dir();
    let cases = conform::discover(&root).expect("discover conformance cases");
    assert!(
        cases.len() >= 20,
        "conformance corpus looks truncated: {} cases",
        cases.len()
    );
    // The JIT tier runs in a child process, so it needs a real `ax` binary.
    // Cargo builds one for integration tests and tells us where it is; without
    // this the tier would silently not run.
    std::env::set_var("AX_JIT_BIN", env!("CARGO_BIN_EXE_ax"));
    let results = conform::run_suite(&root, None).expect("run conformance suite");
    let failures: Vec<String> = results
        .iter()
        .filter_map(|r| match &r.outcome {
            Outcome::Pass => None,
            Outcome::Fail { tier, detail } => Some(format!("{} [{tier}] {detail}", r.name)),
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} conformance cases failed:\n  {}",
        failures.len(),
        results.len(),
        failures.join("\n  ")
    );
    // Four tiers or the claim is wrong. A case that skipped the Cranelift tier
    // was compared against two backends, not three, and the summary would say
    // otherwise.
    let skipped: Vec<&str> = results
        .iter()
        .filter(|r| r.runnable && !r.jit_ran)
        .map(|r| r.name.as_str())
        .collect();
    assert!(
        skipped.is_empty(),
        "the cranelift tier did not run on {} case(s): {}",
        skipped.len(),
        skipped.join(", ")
    );
}
