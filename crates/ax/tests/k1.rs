//! E1 design integrity. The 2×2 exists because the older `eval-loop` measured
//! one diagonal of it, and a diagonal cannot separate the language from the
//! protocol. These tests pin the properties that make the four cells
//! comparable, so a later change cannot quietly restore the diagonal.

use ax::evalloop;

fn report() -> evalloop::FactorialReport {
    evalloop::run_2x2(42, 4, 12)
}

/// All four cells must be named, whether or not the toolchain to run them is
/// present. A missing cell has to show up as "not measured", never as absent.
#[test]
fn all_four_cells_are_reported() {
    let r = report();
    let arms: Vec<&str> = r.cells.iter().map(|c| c.arm.as_str()).collect();
    for want in ["ax+proto", "ax−proto", "rust+proto", "rust−proto"] {
        assert!(arms.contains(&want), "cell {want} missing from {arms:?}");
    }
    assert_eq!(r.cells.len(), 4);
    // One factor varies per axis, and both levels of each appear.
    assert_eq!(r.cells.iter().filter(|c| c.protocol).count(), 2);
    assert_eq!(r.cells.iter().filter(|c| !c.protocol).count(), 2);
    assert_eq!(r.cells.iter().filter(|c| c.language == "ax").count(), 2);
    assert_eq!(r.cells.iter().filter(|c| c.language == "rust").count(), 2);
}

/// The control arm has to actually verify before building, or it is the
/// strawman this experiment was written to remove. Probing without ranking is
/// half a protocol, and the omitted half is the one `ax hole --fills` uses, so
/// the arm is only a control if its probes cut its attempts.
#[test]
fn the_rust_control_arm_uses_its_protocol() {
    if !evalloop::rustc_available() {
        return;
    }
    let tasks = evalloop::generate_hidden(42, 6);
    let mut probed = 0;
    let mut cheaper = 0;
    for t in &tasks {
        let tooled = evalloop::run_rust_tooled_loop(t, 12);
        let bare = evalloop::run_rust_loop(t, 12);
        assert!(tooled.green, "control arm failed {}", t.id);
        if tooled.probes > 0 {
            probed += 1;
        }
        if tooled.attempts <= bare.attempts {
            cheaper += 1;
        }
    }
    assert_eq!(probed, tasks.len(), "every task must be probed, not built blind");
    assert_eq!(
        cheaper,
        tasks.len(),
        "the tooled arm must never need more attempts than the bare one"
    );
}

/// Token accounting has to be uniform across the cells or the token column
/// compares nothing. Probes bill the candidate expression; attempts bill the
/// whole program. Both protocol arms must therefore charge for their probes.
#[test]
fn probes_are_billed_in_both_protocol_arms() {
    let tasks = evalloop::generate_hidden(42, 3);
    for t in &tasks {
        let ax = evalloop::run_ax_loop(t, 12);
        assert!(ax.probes > 0 && ax.tokens_written > 0);
        if evalloop::rustc_available() {
            let rs = evalloop::run_rust_tooled_loop(t, 12);
            assert!(
                rs.probes > 0 && rs.tokens_written > 0,
                "the rust control arm must bill its probes too"
            );
        }
    }
}

/// A cell that could not run a task excludes it and says so. Scoring an
/// unsupported task as a failure would understate whichever arm the harness
/// happened to break in.
#[test]
fn unsupported_tasks_are_excluded_not_failed() {
    let r = report();
    for c in &r.cells {
        assert_eq!(
            c.scored + c.excluded,
            if c.scored == 0 && c.excluded == 0 { 0 } else { r.n },
            "cell {} loses tasks: scored {} + excluded {} != n {}",
            c.arm,
            c.scored,
            c.excluded,
            r.n
        );
    }
}
