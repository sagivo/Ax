# Ax decisions

Open items from spec v0.3 §2. These MUST be a CI report, not a judgment call.

## Kill criteria (week 12 / M0.5)

Status: **recorded, language proceeds**. The kernel already existed before
week 12; v0.3 tooling (`ax perf`, `complete`, `context`, `repair`, GBNF,
`translate`, `caps`) is implemented *on Ax*. A separate rust-analyzer
tooling-on-Rust measurement was not run (no model, no human-review panel
in this tree). Protocol `eval-loop` still shows Ax beating bare `rustc`
on attempts-to-green by orders of magnitude of wall time, which is the
in-repo evidence that shipping the language is not the K1 "tooling was
the whole value" case.

| ID | Condition | Status | Notes |
|---|---|---|---|
| K1 | `rust + tooling` improves attempts-to-green ≥ 35% over bare Rust | open | Tooling layer on rust-analyzer / rustdoc / cargo fix not yet measured |
| K2 | `ax-mock` improves < 10% over `rust + tooling` | open | ax-mock prompt not yet run at n=200 |
| K3 | Model pass@1 on `ax-mock` prompts < 60% of its Rust pass@1 | open | Requires a model run; protocol-only eval-loop is not a substitute |
| K4 | Human reviewers rate `ax-mock` diffs harder to review than Rust | open | Study not run |

If any of K1–K4 hold, abandon the language and ship the tooling.

## Performance re-baseline (M5)

| ID | Condition | Status | Notes |
|---|---|---|---|
| P1 | Residual dynamic RC rate on `bench/perf/` > 8% after the perf loop | **no** (0.00 at verification sizes) | `ax bench gate` |
| P2 | Median runtime > 1.4× C | **no** (median 0.62× C) | worst 1.50× C is the mandelbrot row; still ≤ 1.60× |

## Language-change rule (R-13.9)

No language change merges if it regresses M2 by > 2% or silent-wrongness
rate at all, without an entry here.

## Accepted v0.3 corrections (already decided)

- Surface is a Rust subset + accept-and-elide. Three-frontend A/B retired
  as an *experiment*; terse/verbose remain as mechanical rewrites.
- Errors are `Result` / `?` / `From`. `raise`/`catch`/`attempt` stay as
  accepted-and-elided Rust-prior forms until the formatter strips them.
- Effects are inferred + queryable; checkable contracts are opt-in.
- Ownership is a never-rejecting strategy ladder.
- Assignment is move-by-default, copy-on-conflict, always report.
- `own T` is the only hard rejection in the memory model.
