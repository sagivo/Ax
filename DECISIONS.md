# Ax decisions

Open items from spec v0.3 §2. These MUST be a CI report, not a judgment call.

## Kill criteria (week 12 / M0.5)

Status: **K1's numeric condition is met; the language proceeds on other
grounds, which are now measured rather than asserted.**

The earlier entry here cited `ax eval-loop` as "the in-repo evidence that
shipping the language is not the K1 case". That citation was wrong, and the
reason it was wrong matters more than the number: `eval-loop` compares
**ax + protocol** against **rust − protocol**, which is one diagonal of a
two-factor design. The language and the protocol are perfectly confounded in
it, so its result is *equally* consistent with "the protocol was the whole
value" — the K1 hypothesis. A diagonal cannot refute K1; it cannot address it
at all. `ax k1` now runs all four cells.

| ID | Condition | Status | Notes |
|---|---|---|---|
| K1 | `rust + tooling` improves attempts-to-green ≥ 35% over bare Rust | **YES, fires** (harness: `ax k1`) | 3.0 → 1.0 median attempts, a 67% improvement. See E1 below |
| K2 | `ax-mock` improves < 10% over `rust + tooling` | **open** | needs n=200 model completions |
| K3 | Model pass@1 on `ax-mock` prompts < 60% of its Rust pass@1 | **open** | needs a model |
| K4 | Human reviewers rate `ax-mock` diffs harder to review than Rust | **open** | needs a review panel |

If any of K1–K4 hold, abandon the language and ship the tooling.

**K1 fires, and the rule is not applied.** The reason is recorded here rather
than left to a judgment call. K1's *condition* is about attempts-to-green, and
that condition is met: a protocol over Rust reaches green in as few attempts as
Ax does. K1's *inference* is "the tooling was the whole value", and E2 measures
that inference directly and contradicts it — the protocol and the semantics buy
different things, and only one of them is retrofittable. The rule as written
would have Ax abandoned on evidence that does not speak to what E2 measures.
Anyone re-reading this should treat that as a defect in the criterion's wording,
not as a licence to ignore it: the replacement condition should be **"a protocol
over Rust closes the silent-wrongness and mechanism-coverage gap in E2"**, which
is the claim that would actually make the language redundant.

### E1 — attempts-to-green, all four cells (`ax k1`)

Medians over 4 seeds, n=12 each. An attempt is one build-and-run cycle; a probe
is a static query that builds nothing. The `rust + proto` arm verifies each
candidate with `rustc --emit=metadata` (a type-check with no codegen and no
link) and then ranks bare in-scope bindings ahead of synthesised literals, which
is what rust-analyzer's expected-type completion does for free. The `ax − proto`
arm pays a full native build per candidate, as the Rust arms always did.

| cell | pass | median attempts | median wall | median tokens |
|---|---|---:|---:|---:|
| ax + proto | 48/48 | 1.0 (2.5 on seed 7) | **0.6–1.0 ms** | 64–77 |
| ax − proto | 48/48 | 3.0 | 872–1099 ms | 51–63 |
| rust + proto | 48/48 | **1.0** (2.0 on seed 7) | 361–612 ms | 79–204 |
| rust − proto | 48/48 | 3.0 | 673–877 ms | 75–224 |

Wall-clock ranges are wide because these are process-spawn-bound: the same seed
varies by ~20% between runs. Attempts and tokens are deterministic given a seed.

Read off the cells:

- **The attempt-count win was the protocol, not the language.** Holding the
  protocol fixed, Rust matches Ax at 1.0 attempts, and on seed 7 it *beats* Ax
  (2.0 vs 2.5). Nothing about Ax's semantics reduces attempts once Rust is
  allowed to check before it builds.
- **The latency win survives, and it is ~400×, not ~600×.** 0.6–1.0 ms vs
  361–612 ms.
  This is a real property of the implementation — in-process checking and a
  Cranelift tier that needs no `cc` and no process spawn — and no wrapper over
  `rustc` closes it, because the floor is one process launch per probe (~37 ms
  for `--emit=metadata` alone).
- **The token margin is ~1.0×–2.7×, not 1.04×–8.6×.** Two accounting bugs
  inflated the old figure, both now fixed and both of which had favoured Ax:
  probes in the ax arm were billed as free, and the tooled Rust arm was not
  billed for its build attempts.
- **Without the protocol, Ax is slower than Rust.** Over nine seeds the
  `ax − proto` / `rust − proto` ratio is 1.04×–1.32× and never below 1.0,
  because `build_tier` recompiles `axrt.c` and `axlang.c` on every invocation.
  The language does not carry the wall-clock result on its own; the in-process
  checker and the Cranelift tier do.

Limit worth stating: every E1 task is a hole-fill task, which is the shape the
Ax protocol is built for. Both protocol arms now benefit from that equally, so
the comparison is fair *within* the shape — but no task here can exercise the
semantics, which is why E2 exists.

### E2 — silent-wrongness and tier divergence (`ax silent-wrongness`)

The axis E1 cannot reach. 11 hand-written hazards; for each, the same intent in
both languages, run on every tier. Nothing here depends on `ax check` being
fast, on holes, on ranked fills, or on structured diagnostics, so whatever Ax
wins here it wins on semantics.

| | ax | rust |
|---|---:|---:|
| silent (accepted, completed, violated the intent) | **4/11 (36%)** | 7/11 (64%) |
| tier-divergent (one source, different outcomes across its own tiers) | **0/11 (0%)** | 2/11 (18%) |
| has-mechanism (the language ships something that could catch this class) | **11/11 (100%)** | 6/11 (55%) |

Where the gap comes from, and where it does not:

- **Ax only, no Rust mechanism exists** — undeclared failure modes
  (`div-zero-undeclared`, E0200), effect rows (`effect-row-io`,
  `termination-claim`, E0200), taint (`taint-sink`, A5101), capability budgets
  (`cap-budget`, A5001). Rust accepts all five and four of them run to
  completion with nothing said. These are not lint gaps: `std::fs` is ambient,
  `String` carries no provenance, and there is no row to violate.
- **Rust is better on one row.** `overflow-literal`: the deny-by-default
  `arithmetic_overflow` lint const-folds and rejects it. Ax accepts and wraps.
- **Parity on two rows.** Neither language allows implicit numeric conversion
  (E0108 / E0308), and both truncate silently once `as` is written.
- **Tier divergence is Rust's, not Ax's.** `overflow-argv` and `shift-width`
  panic in debug and produce a wrapped answer in release — an agent tests on one
  and ships the other. All four Ax tiers agree on every hazard, which is the
  four-implementation design paying rent outside the conformance corpus.

Harness integrity, since the corpus is hand-written and therefore the whole
experiment: `control-sum` is a correct program both languages must get right on
every tier (it caught the first version handing the in-process tiers an argv
with no `argv(0)`), and `crates/ax/tests/silent.rs` fails the build if the
Rust-wins row or the parity rows are ever removed.

## Performance re-baseline (M5)

| ID | Condition | Status | Notes |
|---|---|---|---|
| P1 | Residual dynamic RC rate on `bench/perf/` > 8% after the perf loop | **no** (0.00 at verification sizes) | Unique/RC IR ops (`UniqueAlloc`/`RcRetain`…) lower to `ax_rt_*`; residual RC retain emitted for shared ptr params |
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

## R-14 — the language is a tree, not a Rust dialect (2026-08-18)

The v0.3 surface rule ("look like Rust so models can write it") is
reversed. The agent-canonical surface is a prefix tree (`S-tree`,
`--surface tree`, auto-detected when a file opens with `(`). Reasons,
from first principles, for a program that writes programs:

- Infix is a silent-wrongness class (`a+b*c`). A prefix list cannot
  encode the wrong grouping without writing it.
- Humans need sugar because they read. An agent reads the protocol
  (`ax types`, `ax hole`, `ax ir`) and writes once. Sugar is cost.
- Multiple surfaces train format oscillation. One printer is the
  inverse of one parser, so `fmt` is a bijection and patches are tree
  edits.
- Constrained decoding is a list grammar, not a reconstruction of Rust.
- Accept-and-elide exists to forgive models that emit Rust. That is
  training the language to be Rust. The tree has nothing to elide.

**What stays Rust, and why.** The compiler is still a Rust program.
The conventional / terse / verbose parsers stay so the four backends
keep proving the same IR against the existing corpus, `ax k1`, and
`ax silent-wrongness`. Those measurements are about *semantics*, not
syntax, and rewriting 120 conformance files in the same change would
confound them. Rust is the implementation language and the corpus
dialect. It is not the identity of Ax.

**What is not a coat.** Effects, regions, `own`, `Untrusted`/`Secret`,
holes, the four-implementation oracle, and the protocol were never
Rust. The tree is the surface those already deserved.

New agent-facing examples live as trees in `examples/*.ax`. The card and
`spec/v0.3.md` §1 / §3 describe the tree. The conventional grammar in
`spec/grammar.ebnf` remains the corpus grammar until the corpus moves.

## R-15 — accept-and-elide is frozen (2026-08-18)

No agent-facing command emits the conventional dialect. `ax fmt`,
`ax hole --fills`, `ax types`, `ax complete`, `ax translate`, and
`ax patch` speak tree when the source is tree. Accept-and-elide
(`pub`, `unsafe`, `.clone()`, Rust `struct`/`enum`/`impl`) stays in
the corpus parser so `conformance/`, `ax k1`'s rust cell, and
`ax silent-wrongness` keep measuring *semantics*. It is not extended.
A new elision is a language change and needs an entry here.

`std/core/lib.ax` and `ax-mock` remain conventional on purpose: they
are compiler fixtures, not agent programs.

## R-16 — measure the tree, not the coat (2026-08-18)

`ax eval-loop` / `ax k1` hidden tasks now start as trees
(`(+ a ?)`, `(block (let x i32 n) ?)`). Attempts-to-green on
`examples/holes.ax` is a tree file. Silent-wrongness stays on the
corpus dialect because E2 is a *semantic* comparison with Rust; a
surface rewrite would confound it. Tree equivalence is pinned by
`tree_and_conventional_same_answer` and the ported examples.

## Test-spec v1.0 (this tree)

- **T-3.4.1 `as` casts:** the language card (`spec/card.md`) still defines `as` as the only numeric conversion. The test-spec's "reject `as`; suggest `to_*`" is recorded as a *future* divergence, not implemented. Tests pin the card.
- **T-2.1.3 / T-3.4.1 overflow:** `+ - *` wrap in every profile (`spec/primitives.md`, card). The test-spec's "panic in all profiles" is not the language. rustc-oracle comparisons use wrapping semantics, not `-C overflow-checks=on` as a *requirement that Ax panic*. The overflow-checks flag is still passed so a *documented* wrap/panic split can be classified ([T-2.1.2]) rather than silently compared.
- **T-11.3 GCC torture:** skipped; LLVM SingleSource / Zig behavior / Go `test/` cover the same ground. Do not vendor GPL.
- **Catalog codes pending emit** (`testharness::catalog_codes_pending_emit`):
  `E0301` (now A0101), `E0303`, `E0402`, `E0502` (needs `--strict-det` in the
  file harness), `E0700`, `A0102`, `A0108`. A code that starts firing must
  leave that list and gain a test in the same change.
- **Copy-on-conflict for aggregates (T-INV-0010):** native `let y = x` on a
  record used to alias `x`'s storage unless `y` was `mut`. The interpreter
  copied. After `x.s = 6`, oracle printed 11 and C/Cranelift printed 12.
  Lowering now always `CopyAgg` on aggregate bind ([R-3.3.1]). Last-use may
  later elide the copy; sharing storage when the source is used again is never
  correct. rustc 1.85.0 pin: `4d91de4e48198da2e33413efdcd9cd2cc0c46688`.
