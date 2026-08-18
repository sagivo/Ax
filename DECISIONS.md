# Ax decisions

Open items from spec v0.3 §2. These MUST be a CI report, not a judgment call.

## Kill criteria (week 12 / M0.5)

Status: **open**. The language kernel already exists (pre-v0.3 research-v1).
v0.3 still requires the tooling-on-Rust measurement before claiming the
language is the right product. Until that table exists, these stay open.

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
| P1 | Residual dynamic RC rate on `bench/perf/` > 8% after the perf loop | open | Ownership ladder reports residual RC; gate is `ax bench gate` |
| P2 | Median runtime > 1.4× C | open | Then ship at Go-class and say so |

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
