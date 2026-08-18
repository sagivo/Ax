# Ax

A compiled systems language whose primary consumer is a program, not a person.
The bet is not that the syntax is better; it is that a compiler which answers
questions in microseconds, classifies its own fixes by safety, and ships a
normative interpreter to check itself against is worth more to an agent than any
surface-level cleverness.

```
.ax → lexer → parser (Rust subset + accept-and-elide; terse rewrite still works)
    → check (types, effects, regions, Untrusted/Secret/own)
    → ownership ladder + ax perf --json
    → typed SSA IR
    → ├─ Cranelift      (in-process: compile and run in 0.3 ms, no cc)
      ├─ C11 backend    (dev / release / portable, via cc — the LLVM-class path)
      └─ oracle interpreter (normative)
```

**Three implementations, one IR.** Every conformance case runs on the
interpreter, both C tiers, and the Cranelift backend, and all four must produce
the stated answer. Two of the three read layout differently on purpose — C uses
`sizeof`/`offsetof`, Cranelift uses the IR's own offsets — so a lowering mistake
shows up as disagreement rather than as a shared bug.

The **IR is the only thing backends see**. Types, effects, and regions are still
attached to it, and they pay rent there: `err[E]` becomes a two-value return, a
`region` becomes a bump arena, `at` becomes an explicit bounds check and abort.
Text (`.ax`) is authoritative and the interpreter is the normative oracle.

## Install

```sh
cargo install --path crates/ax
```

Requires Rust 1.94+ (the Cranelift backend's MSRV) and a C11 compiler. Go is
optional (one benchmark column).

## Language

```ax
module app;
export { add };

fn add(a: i32, b: i32) -> i32 = a + b;

fn parse(s: &str) -> i32 !{err[ParseError]} = parse_i32(s);

test "add" = assert(add(1, 2) == 3);
```

- Rust/Go-shaped surface. Mandatory types on every top-level `fn`.
- Effects in the signature: `err[E]`, `io[c]`, `alloc[a]`, `diverge`, `abort`.
- Errors: `raise` / `catch` / `attempt` plus declared single-step `from`
  injections. No unwinding: a raise is a branch on a returned tag.
- Regions: `region r { .. }` is a bump arena, and `r` is an allocator you can
  hand to `vec.new(r)`. `store(&r T, l)` is legal iff `r` outlives `l`.
- Dictionaries: `dict Ord[T] = { cmp: … }` with `= default` unique resolution,
  resolved statically and called directly — no vtable.
- `as` is the only numeric conversion; there are no implicit ones.

Full card: `ax card` or `spec/card.md`. Normative grammar: `spec/grammar.ebnf`.

**v0.3 additions:** Rust-shaped `fn f() { … }` / `struct` / `enum` parse;
postfix `?` is `Result` propagation; `f"…"` interpolates; `own T` is affine
(use exactly once); `Untrusted[T]` / `Secret[T]` are lattice annotations;
`ax perf --json` reports the ownership ladder and surviving checks.
`par` is still rejected until disjointness is proven. `Map`/`SortedMap` and
capturing closures remain out of v1.

## Protocol

```
ax check [--json] [--allow-holes] [--strict-det] [--surface conventional|terse|verbose]
ax hole [--fills] [--json]      ranked fills, each verified by compiling it
ax fix [--apply]                applies only semantics_preserving fixes
ax test [--attempts-to-green]   the north-star metric
ax run --seed N --trace f       records a transcript (oracle)
ax jit <file> [args]            Cranelift: compile and run, no cc, ~0.3 ms
ax replay f source.ax           replays from the transcript, performing no IO
ax ir | types | effs | search | errs --into | fmt | patch --tx | deps --affected
ax conform [filter]             conformance suite, every tier
ax build [-o bin] [--tier dev|release|portable]
ax bench io|http|metrics|tokens|all  |  ax eval-loop [--seed N] [--n K]
ax merge --semantic | label | card | pkg list | pkg write
ax perf [--json] [--diff baseline.json] | complete | context | repair
ax bench gate | gate-check
ax caps | translate | gbnf --check N | daemon | kill-criteria
```

`ax hole --fills` is the piece that matters. It synthesises candidate
expressions, then verifies each by substituting it and running the checker, so
what comes back is *known to compile*:

```
$ ax hole --fills examples/holes.ax
hole examples.holes::fn:distance  expects: f32
  7 of 7 candidates compile
    1  v.x    in scope, exact type
    2  v.y    in scope, exact type
    3  math.hypot(v.x, v.y)    prelude call, matching result type
    ...
```

```
$ ax test --attempts-to-green distance.ax
green  distance.ax  holes filled 1  attempts 3  probes 2  2.5 ms
  applied: math.hypot(v.x, v.y)
```

## Performance

Same algorithm, identical output — the harness refuses to report a time unless
every backend printed the same value. **Fastest of 21 runs** (raised from 9 after a
single outlier in Rust's `gcd` time moved that row's verdict by 8% between runs).
`ax bench metrics`.

Where Ax wins, a language guarantee is doing the work. Where the work is a plain
loop, Ax lands on clang's output and clang's result is what you get.

| workload | ax | rust | go | ax/rust | why |
|---|---:|---:|---:|---:|---|
| `fib(40)` from argv | **2.23 ms** | 161.8 ms | 220.4 ms | **0.014×** | the row proves `fib` pure, so results are cached |
| `C(30,14)` two-arg recursion | **1.85 ms** | 168.5 ms | 258.4 ms | **0.011×** | same cache, now for two integer arguments |
| `fib(40)`, literal argument | **1.56 ms** | 165.5 ms | — | **0.009×** | same proof, applied during the build |
| invariant-divisor `%` 8e7 | **36.5 ms** | 40.5 ms | 41.6 ms | **0.90×** | reciprocal hoisted; rustc/gc leave a `udiv` |
| 1e6 short-lived buffers | **9.70 ms** | 16.72 ms | 21.80 ms | **0.58×** | region arena: no per-object free |
| IO 64 MiB bytesum | **9.46 ms** | 15.14 ms | — | **0.62×** | `axrt` mmaps in place; idiomatic Rust copies |
| 2e6 records pushed + summed | **5.57 ms** | 6.83 ms | 12.87 ms | **0.82×** | arena grows in place, `push` inlined, checks dropped |
| Euclid gcd 3e6 | 105.4 ms | 106.0 ms | 107.2 ms | 0.99× | parity |
| FNV nest 6000² | 35.2 ms | 34.6 ms | 44.0 ms | 1.02× | within 3% of C |
| LCG mix 2e8 | 183.9 ms | 184.0 ms | 187.0 ms | 1.00× | **parity is provably optimal** |
| primes < 6e5 | 23.2 ms | 23.1 ms | **20.4 ms** | 1.00× | **parity with Rust; a 13% loss to Go** |
| check a module | **89 µs** | 1.10 s | — | ~10⁻⁴× | the checker is what an agent calls in a loop |
| compile **and run** a module | **305 µs** | 1.10 s (compile only) | — | ~10⁻⁴× | Cranelift, in-process; `cc` alone is 68 ms |

**Stated plainly:** Ax is far faster than Rust where its type system licenses
something Rust's cannot express (proven purity, region allocation, capability IO,
loop-invariant division), at parity on plain compute, and 13% *slower than Go* on
`primes`. "Faster than Rust on identical machine-level work" is not a claim this
table supports except where a language guarantee does the work.

An earlier version of this file claimed no workload was meaningfully slower than
Rust. That rested on two unfair benchmarks, both now fixed: the Rust `primes`
kernel used `d.saturating_mul(d)` — an overflow check per iteration nobody else
paid — and the Go one was the only version with the helper inlined into `main`.
Correcting them erased an Ax "win" and produced a Go win. Fixing a benchmark that
flatters you is the only kind of benchmark fix that counts.

**Why Go wins `primes`, and why it is not a language difference.** All four inner
loops were disassembled. C's is 7 instructions, Rust's and Ax's are 8 (both pay a
divide-by-zero branch), and *Go's is 8 as well* — including a `CBZ → panicdivide`
Go cannot remove. Go is 15% faster with the same instruction count, so the
difference is scheduling in the code generator, not work. Ax matches C to within
3% on every loop kernel; where clang trails, Ax trails with it. Closing the Go
`primes` gap means becoming LLVM, which is not the plan. The exception is
`modmix`: the divisor is not a constant, so clang cannot strength-reduce it, and
Ax's hoist is 0.89× C.

**Purity is proven, so results can be reused.** An empty effect row means a
function observes nothing and returns the same value for the same arguments. Ax
spends that twice: it evaluates such a call during the build when the arguments are
literals, and caches results at run time for self-recursive one- or two-argument integer
functions. The cache is direct-mapped, fixed size, thread-local, and compares the
stored key, so a collision costs a recomputation rather than a wrong answer. Rust
has no purity in its type system, so `rustc` must run the whole call tree.
Conformance pins that the oracle and the Cranelift tier — neither of which caches
— return the same values as the C tiers, and that a function with `io` in its row
is never cached.

**On the LCG row, parity is the ceiling for everyone.** `ax bench metrics` runs a
roofline check: the same loop as four independent chains, doing four times the
multiplies, costs **1.06× the wall time**. The single chain leaves the multiplier
three-quarters idle waiting on its own previous result, so the program sets the
speed, not the compiler. Ax reaching 4.11e9 multiplies/s once the dependency is
removed says the code generation has nothing left in it.

**Not a claim:** process startup. All four land between 1.5 and 2.6 ms, the
ordering changes between runs, and it is mostly process creation.

Also true and worth stating: raw `malloc`/`free` of one fixed-size block beats an
arena by 5.8×, and plain `malloc`/`realloc` beats it by 1.3× on the vector build.
The arena wins against a *growable container abstraction*, which is what Rust and
Go actually give you.

## Token cost

Two questions hide behind "optimised for LLMs", and only one favours Ax.

**Tokens per source file** (`ax bench tokens`, proxy tokenizer applied identically
to each language; the terse form is derived mechanically and verified by compiling
and running it):

| language | tokens | vs ax-terse |
|---|---:|---:|
| **ax-terse** | **532** | 1.00× |
| go | 632 | 1.19× |
| ax (conventional) | 633 | 1.19× |
| rust | 685 | 1.29× |
| c | 766 | 1.44× |

The terse surface is the one an agent writes, and it is the most compact of the
five. Two things got it there, and neither is cosmetic:

- **the header is inferable** — `module x; export { .. };` is about twelve tokens
  per file that the toolchain already knows, so terse sources may omit both and
  the module name comes from the file stem. Only the terse surface allows this;
  the conventional one still requires the declaration;
- **effect rows say only what is true** — `%` no longer forces `err[DivError]`
  into a signature when the divisor is provably non-zero, and an omitted `!{…}`
  reconstructs `diverge` from `while`/`loop`. An explicit empty row is still a
  termination claim. Precision in the checker is a token optimisation as well
  as an interface one.

Shortening keywords would *not* have helped: `fn` and `f` are both one token. What
costs tokens is syntax that exists, so the wins came from removing it.

**Cost to reach a working program** — every candidate written out plus every byte
of compiler output read back, over five seeds (`ax eval-loop --seed S --n 6`):

| metric | ax | rust (real `rustc`) | ratio |
|---|---:|---:|---:|
| wall time to green | 1.0–1.4 ms | 685–904 ms | **~600× less** |
| compile-and-run cycles | 1.0–2.5 | 3.0 every seed | fewer or equal |
| tokens written + read | 64–72 | 75–569 | 1.04×–8.6× less |

The wall-time result is the stable one, and it follows directly from `ax check`
costing 100 µs: the protocol can reject a candidate without building anything. The
token margin swings by seed because it depends on whether a rejected candidate
made `rustc` print a paragraph or merely produced a wrong answer quietly — so
"fewer, sometimes far fewer, never more" is the claim, not the best case.

This is where designing for a model pays off, and it is a property of the protocol
rather than the syntax.

## Tests

```sh
cargo test --workspace   # unit, kernel, protocol, differential, conformance, testharness
ax conform               # the conformance corpus alone
ax testharness           # Test Spec v1.0 tree under tests/
```

The conformance corpus (`conformance/`) is 120 cases ported by scenario from Go's
`test/` and Rust's suites: integer wrapping and shift semantics, truncating
division, IEEE comparison and NaN canonicalisation, bitwise ops, casts, control
flow, records and variants, error propagation and injection, regions, the
container stdlib, sort stability, and the semantics each optimisation must
preserve. **Every case runs on the oracle, on both C tiers, and on the
Cranelift backend**, and each is checked against a stated expectation rather than
merely cross-compared — agreement between two buggy backends is not evidence.

Building the corpus found ten defects in the language itself, including a
unit-variant pattern that matched anything (so the oracle silently selected the
wrong `match` arm), `NaN == NaN` returning true, `i32.cmp(&a, &b)` comparing
pointers in native code, and — found by the Cranelift tier on its first run — a
`test` whose body evaluated to `false` being reported as **passing** by the
oracle while both C tiers failed it.

The suite is checked for teeth by fault injection rather than assumed to have
them. Shifting every aggregate field offset in the Cranelift backend by four
bytes fails 36 of the cases. Removing its `INT_MIN / -1` fixup failed
*nothing*, which exposed a gap: the corpus divided by `-1` only at the minimum
value, where substituting `1` for the divisor gives the same answer by
coincidence. Three cases were added, and that mutation now fails.

```sh
ax eval-loop --seed 42 --n 8
```

Two-arm attempts-to-green measurement. Both arms get the same candidate pool; the
ax arm may ask which candidates typecheck first. An attempt is one
compile-and-run cycle:

| arm | pass | median attempts | median wall |
|---|---:|---:|---:|
| ax | 8/8 | 1.0 | 1.3 ms |
| rust (real `rustc`) | 8/8 | 3.0 | 903 ms |

No model is involved; this measures the protocol, not an LLM.

## Capabilities

Handle-based: a `ReadCap` names a directory, and `..`, absolute paths, and
`widen` all fail closed. Labels are earned, not asserted — a program that calls
`io.*` or `http.*` (which take no capability) loses `capability-contained` and
`replay-deterministic`, and the offending call is named:

```
$ ax label examples/io_sum.ax
safe ambient-io(argv io.bytesum_file)
```

## Layout

```
spec/grammar.ebnf     normative grammar
spec/card.md          ≤ 3,000-token agent card
spec/metrics.md       benchmark methodology and results
crates/ax/            checker, typed IR, C backend, oracle, CLI
runtime/axrt.c        IO / HTTP core (mmap, keep-alive pool)
runtime/axlang.c      language ABI: aborts, arenas, exact semantics, stdlib
conformance/          the corpus, one case per file
tests/                Test Spec v1.0 tree (headers, oracles, inverted, authored)
examples/             snippets CI compiles and runs
```

## Design stance

Ax has zero training data, so recognisability plus a strong oracle beats
compression. Novelty is spent on the compiler protocol, exact effect-aware
interfaces, typed holes, and honest capability labels — not on syntax.

`net` / `tls` / `crypto` / `regex` / `time` are reserved versioned components
(`ax pkg list`) and are **not** compiled into the toolchain.
