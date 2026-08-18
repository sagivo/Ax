//! E2 harness integrity. The measurement is only worth quoting if the corpus
//! can embarrass Ax and the harness can fail, so both are pinned here.

use ax::silent::{self, Expect, Verdict};

/// The control program is correct and both languages must get it right on every
/// tier that can run. This is the check that caught the first version of the
/// harness handing the in-process tiers an argv with no `argv(0)`, which made
/// every hazard look like a ParseError.
#[test]
fn control_case_is_right_on_every_available_tier() {
    let r = silent::run(Some("control"));
    assert_eq!(r.hazards.len(), 1, "control filter selects one hazard");
    let h = &r.hazards[0];
    for arm in [&h.ax, &h.rust] {
        for (tier, v) in &arm.tiers {
            match v {
                Verdict::Right { .. } | Verdict::Unavailable { .. } => {}
                other => panic!(
                    "control case must be right or unavailable on {} {tier}, got {other:?}",
                    arm.language
                ),
            }
        }
        assert!(!arm.silent, "control case cannot be silently wrong");
        assert!(
            !arm.divergent,
            "{} tiers disagree on a correct program",
            arm.language
        );
    }
}

/// A corpus with no family where Rust wins is advocacy, not measurement. This
/// pins that at least one hazard is expected to favour Rust, so removing the
/// unflattering rows breaks the build.
#[test]
fn corpus_contains_a_family_where_rust_is_better() {
    let c = silent::corpus();
    assert!(
        c.iter().any(|h| h.note.contains("RUST IS BETTER")),
        "the corpus must retain at least one hazard Rust handles better"
    );
    assert!(
        c.iter().filter(|h| h.note.contains("PARITY")).count() >= 2,
        "the corpus must retain parity families so the summary is not all wins"
    );
    assert!(
        c.iter().any(|h| matches!(h.expect, Expect::Value(_)) && h.family == "control"),
        "the corpus must contain a correct control program"
    );
}

/// Every hazard states both mechanism columns and a note explaining them. An
/// unexplained `no mechanism` is an assertion about another language, and those
/// need a reason attached.
#[test]
fn every_hazard_justifies_its_mechanism_columns() {
    for h in silent::corpus() {
        assert!(!h.note.is_empty(), "{} has no note", h.id);
        assert!(!h.intent.is_empty(), "{} has no intent", h.id);
        if !h.rust_mechanism {
            assert!(
                h.note.to_lowercase().contains("rust"),
                "{} claims rust has no mechanism without saying why",
                h.id
            );
        }
    }
}

/// The properties the language is being defended on. If a future change makes
/// Ax accept an undeclared failure mode, a broken effect row, a taint flow, or
/// an over-budget capability, this fails rather than quietly weakening the
/// argument in `DECISIONS.md`.
#[test]
fn ax_rejects_the_language_level_hazards() {
    for id in [
        "div-zero-undeclared",
        "effect-row-io",
        "termination-claim",
        "taint-sink",
        "cap-budget",
    ] {
        let r = silent::run(Some(id));
        let h = r
            .hazards
            .iter()
            .find(|h| h.id == id)
            .unwrap_or_else(|| panic!("{id} missing from corpus"));
        let rejected = h
            .ax
            .tiers
            .iter()
            .all(|(_, v)| matches!(v, Verdict::Rejected { .. }));
        assert!(rejected, "ax must reject {id}, got {:?}", h.ax.tiers);
    }
}

/// Tier agreement is the claim `README.md` makes for the four-implementation
/// design. It is checked on the hazards, not only on the conformance corpus.
#[test]
fn ax_tiers_never_disagree() {
    let r = silent::run(None);
    for h in &r.hazards {
        assert!(
            !h.ax.divergent,
            "ax tiers disagree on {}: {:?}",
            h.id, h.ax.tiers
        );
    }
}
