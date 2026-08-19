# Ax metrics

Measured on this machine (Apple arm64, Darwin 25.5), `ax-dev bench metrics`. Every
row is the *same algorithm* in each language, and the harness refuses to report a
time unless all backends printed the same value. Times include process spawn.

The tiers are: the interpreter (normative), Cranelift (in-process, via `ax jit`),
and C via `cc` (`--tier dev`, `release`, `portable`). Ratios below compare
`ax build --tier release` against Rust and Go unless stated otherwise.

Two statistics are shown because short kernels are noisy: the **fastest of 21
runs** (used for the ratios, as it is least polluted by scheduling) and the median.
A large gap between them means the measurement is unreliable. The count was 9
until a single outlier in Rust's `gcd` time changed that row's verdict by 8%
between two consecutive runs.

Toolchains: `cc -O3 -flto -fno-asynchronous-unwind-tables -DNDEBUG`,
`rustc -C opt-level=3 -C lto=thin -C codegen-units=1 -C panic=abort`,
`go build` (defaults), `ax build --tier release` (the same `cc` flags plus
`axrt`).

## The headline, stated plainly

Ax is much faster than Rust where a language guarantee licenses something Rust
cannot express, at parity on plain compute, and **slower than Go on one kernel**.

| where | ax/rust | ax/go | mechanism |
|---|---:|---:|---|
| pure recursion, argument from `argv` | **0.014×** | **0.010×** | the effect row proves purity, so results are cached |
| two-arg recursion `C(30,14)` | **0.011×** | **0.007×** | the same cache, now for two integer arguments |
| pure call with constant arguments | **0.009×** | — | the same proof, applied during the build |
| loop-invariant unsigned remainder | **0.90×** | **0.88×** | reciprocal hoisted; rustc and gc leave a `udiv` |
| many short-lived allocations | **0.58×** | **0.44×** | region arena: no per-object free at all |
| capability-typed IO | **0.62×** | — | `axrt` mmaps and sums in place instead of copying |
| push-heavy container build | **0.82×** | **0.43×** | arena grows in place, `push` inlined, checks dropped |
| gcd / LCG | 0.99–1.00× | 0.98× | parity — LCG is **provably optimal**, see the roofline check |
| nested FNV | 1.02× | 0.80× | within 3% of C; Go trails |
| **primes** | **1.00×** | **1.13×** | **a loss to Go.** scheduling, not work; see below |
| check latency | ~10⁻⁴× | — | 89 µs, which is what makes the agent loop cheap |
| compile *and run* latency | ~10⁻⁴× | — | 305 µs via Cranelift; `cc` needs 68 ms to compile alone |
| process startup | ~1.0× | ~1.0× | no difference to claim; see below |

### Two benchmarks that flattered Ax, and what they cost

Earlier versions of this file reported `primes` as a 0.93× win over Rust. It was
not one:

- the **Rust** kernel used `d.saturating_mul(d)` for the loop bound, which emits an
  overflow check every iteration. C, Go and Ax all used plain wrapping `d * d`.
- the **Go** kernel was the only one with `isPrime` inlined into `main`, so it paid
  no call per candidate while the other three did.

With both corrected, the row is Ax 1.04× Rust and 1.19× Go — a parity result and a
loss. The rule this enforces, from the project's own notes: *drop or relabel
"faster than Rust" unless the opponent is the same algorithm.*

### Why Go wins `primes`

All four inner loops were disassembled on arm64. C is 7 instructions; Rust and Ax
are 8, both paying a divide-by-zero branch; **Go is also 8**, including a
`CBZ → panicdivide` it cannot remove. Go is 15% faster at the same instruction
count, so this is instruction scheduling in the code generator, not less work. Ax
is within 3% of C on every loop kernel — where clang trails, Ax trails with it.
`modmix` is the exception that proves the rule: the divisor is not a constant, so
clang cannot strength-reduce it, and Ax's hoist is 0.89× C.

### What the non-zero divisor proof does and does not buy

This was listed here as a mechanism that "made the numbers move". That was wrong,
and checking it found a bug:

- The **interface** win is real: `%` no longer forces `err[DivError]` into a
  signature when the divisor cannot be zero, which removes the fallible ABI from
  callers and two rows from the token comparison below.
- The **speed** win was not there. Lowering emitted a guard *and* the
  already-proven-safe divide, so every division still paid a compare and a branch —
  the code was byte-identical to Rust's, including the check the analysis had just
  proved dead.

Now the proof is split by strength. A divisor that is unconditionally non-zero (a
non-zero literal, a `range(lo, _)` variable with `lo >= 1`, a name guarded by
`while c != 0` and never reassigned) emits a bare machine divide with no guard. A
divisor reached through `d = d + k` keeps the guard, because that fact only holds
until the increment wraps, and the alternative to the guard is a wrong answer.
Measured effect on `gcd` and `primes`: **none that survives 21 samples** — those
loops are division-latency-bound, so a removed compare is free anyway. It is
reported as a consistency fix, not a speed-up.

### Purity in the type system, spent twice

An empty effect row is a proof that a function reads no memory it was not given
and performs no IO, so it returns the same value for the same arguments. Ax
spends that proof in two places:

**During the build**, when every argument is a literal, the call is evaluated by
the same interpreter that defines the language, under a step budget, and replaced
by its result. The folder memoises, which is sound for a pure function and turns
an exponential call tree linear.

**At run time**, when the function is a single-argument integer recursion that
calls itself more than once — the shape that recomputes subproblems
exponentially — the backend wraps it in a cache:

```c
typedef struct { int32_t k; int32_t v; unsigned char live; } AxMemo_fib;
static _Thread_local AxMemo_fib axmemo_fib[1u << 12];
```

Direct-mapped, fixed size, thread-local, and it **compares the stored key**, so a
collision costs a recomputation rather than producing a wrong answer.
`conformance/opt/memo_cache_collision.ax` pins that: it calls with 0, 4096, and
8192, which all land in slot 0, and requires the same result from the oracle
(which has no cache), from both C tiers (which do), and from the Cranelift tier
(which deliberately does not implement the cache at all, and is therefore the
control: it computes the answer the long way and must still agree).

Rust cannot do either. `const fn` plus a const context gets the first case; a
plain recursive `fn` called from `main` runs in full on every execution, because
nothing in the signature says it may not.

| program | ax | rust | go | ratio vs rust |
|---|---:|---:|---:|---:|
| `fib(40)`, argument from `argv` | **1.779 ms** | 159.100 ms | 220.305 ms | **0.011×** |
| `fib(40)`, literal argument | **2.111 ms** | 162.409 ms | — | **0.013×** |

Two-argument tree recursion is now cached the same way. The compute table still
includes `comb` so the harness checks that the cache cannot change the value;
the four loop kernels remain the honest measure of code generation.

### Why compute parity is the ceiling, not a shortfall

`ax-dev bench metrics` runs a roofline check: the same LCG loop as one dependent
chain, then as **four independent chains** doing four times the multiplies.

```
roofline     dependent vs independent multiply chains
  1 chain         91.774 ms   1.09e9 multiplies/s
  4 chains        97.366 ms   4.11e9 multiplies/s  (1.06× the wall time for 4× the work)
  verdict        latency-bound; parity with C and Rust is the optimum here
```

Four times the work for six percent more time. The single-chain loop spends
three-quarters of every multiply slot waiting on its own previous result, so the
program — not the compiler — sets the speed. No compiler can beat it, and the
4.11e9 multiplies/s Ax reaches once the dependency is gone shows the code
generation has nothing left in it.

That argument covers `int_mix` and, by the same shape (a serial multiply–xor
chain), `nested`. It is a claim about those kernels only; it is not a general
excuse for parity elsewhere.

### What made the allocation numbers move

Three mechanisms, all absent when this file first reported parity everywhere,
plus one that turned out to be an interface improvement rather than a speed one:

1. **Proven-non-zero division** — an *interface* win, not a speed one; see the
   section above for what it actually buys and the bug that checking it found.
2. **Bounds-check elimination.** `for i in range(0, xs.len()) { xs.at(i) }` is
   provably in range. A v1 `Vec` has no `pop`, `truncate`, or `clear`, so its
   length never decreases — that is the licence, and adding a shrinking operation
   must revisit it.
3. **Arena grow-in-place.** Growth extends the bump pointer when the buffer is the
   arena's newest allocation. A general-purpose allocator cannot do this; it does
   not know what was handed out afterwards.
4. **Inline `push`.** A capacity test and a typed store, calling the runtime only
   to grow.

## Tiers, and whether the differential suite has teeth

| tier | how | what it is for |
|---|---|---|
| oracle | tree-walking interpreter | normative semantics |
| `ax jit` | Cranelift, in-process | compile and run in 0.3 ms |
| `--tier dev` | C11 via `cc -O0 -g` | a debuggable binary |
| `--tier release` / `portable` | C11 via `cc -O3 -flto` | the tier the numbers above measure |

All three consume the same typed IR and nothing else. Two of them read layout
differently *on purpose*: the C backend uses `sizeof` and `offsetof`, so the C
compiler chooses padding, while the Cranelift backend uses the offsets computed
during lowering. Layout is not observable in v1 (no struct FFI), and keeping the
schemes independent means a layout mistake shows up as disagreement instead of as
a bug both backends share.

Agreement is only evidence if disagreement were possible, so the suite is
mutation-tested rather than trusted:

| mutation to the Cranelift backend | cases failed (of 113) |
|---|---:|
| every aggregate field offset shifted by 4 bytes | 36 |
| `INT_MIN / -1` fixup removed | **0**, then 1 after the gap was filled |
| shift-count mask removed | 0 — not a fault: Cranelift masks it too |

The middle row is the useful one. The corpus already had `div_min_by_neg_one`, but
at the minimum value dividing by `-1` and dividing by `1` give the same answer, so
a backend that quietly substituted the divisor passed. Three cases were added
(`div_by_neg_one_negates` is the one with teeth) and the mutation now fails. The
third row is worth recording as a non-finding: the mask is kept anyway, because an
exact language rule should not rest on a backend detail.

The Cranelift tier also found a defect on its first run — `test "x" = 1 == 2;` was
reported as passing by the oracle while both C tiers failed it. The oracle counted
any non-aborting body as a pass. A test suite that cannot fail is worse than none,
so that is fixed and pinned.

## Compute (same algorithm, identical output)

| kernel | n | ax | c | rust | go | ax/c | ax/rust | ax/go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| LCG mix (loop-carried) | 2e8 | 183.916 ms | 184.087 ms | 183.988 ms | 187.042 ms | 1.00× | 1.00× | 0.98× |
| FNV mix over `i×j` | 6000² | 35.191 ms | 34.308 ms | 34.631 ms | 43.995 ms | 1.03× | 1.02× | 0.80× |
| Euclid `gcd` reduction | 3e6 | 105.394 ms | 105.382 ms | 105.993 ms | 107.196 ms | 1.00× | 0.99× | 0.98× |
| invariant-divisor `%` | 8e7 | **36.545 ms** | 40.885 ms | 40.501 ms | 41.587 ms | **0.89×** | **0.90×** | **0.88×** |
| two-arg recursion `C(30,14)` | 30 | **1.845 ms** | 172.847 ms | 168.463 ms | 258.398 ms | **0.01×** | **0.01×** | **0.01×** |
| trial-division primes | 6e5 | 23.151 ms | 23.304 ms | 23.056 ms | **20.433 ms** | 0.99× | 1.00× | 1.13× |

### Strict floating point without a check after every operation

The C backend preserves separate IEEE-754 operations with
`-ffp-contract=off`, but defers canonical-NaN payload normalization until the
value is observable. Intermediate NaN payloads cannot affect comparisons or
arithmetic results, and result/aggregate rendering still canonicalizes them.
This removed redundant payload checks from floating-point recurrence loops:
the full gate moved `nbody` from 2.32× to 1.19× C and `mandelbrot` from 2.02×
to 1.01× C while keeping Ax/Rust parity and identical outputs.

The ray gate now uses the exact integer circle predicate
`dx² + dy² < n²` in every language. Its former floating boundary admitted four
different pixels under C contraction, so that row was invalid rather than a
performance result.

`modmix` is the new compute row: the divisor comes from `argv`, so it is
loop-invariant but not a compile-time constant. Ax hoists a Granlund–Montgomery
reciprocal; rustc and gc leave a `udiv`. A constant divisor is deliberately
*not* hoisted — clang already vectorises `n % 7`, and inserting a runtime
reciprocal lost by 2.19× until that case was carved out.

The `C(30,14)` row is now memoised by the two-argument rule (`ax ir` shows
`[pure bounded memoize]`). It no longer measures call-and-branch code
generation; it measures the cache, the same way `fib` does. The four loop
kernels plus `modmix` remain the honest compute rows.

For the record, on the old `fib` row Ax was 1.10× Rust and matched hand-written C
to 1.01%. Disassembling both showed an identical eight-instruction loop with one
extra serial `mov` in the clang output — a clang-versus-rustc difference, not a
language one. Removing that row because a language feature made it trivial is
legitimate; claiming the language got faster at recursion would not be.

## Allocation

Two shapes, because they stress different things. Each language grows its
container idiomatically. Independently re-timed with a separate harness (25 runs)
to confirm the ordering is not an artefact of this one.

**Build a large vector** — 2e6 records pushed, then summed:

| backend | min | vs C | vs Rust |
|---|---:|---:|---:|
| c (`malloc`/`realloc`) | 4.398 ms | 1.00× | — |
| **ax (region arena)** | **5.747 ms** | 1.31× | **0.92×** |
| rust (`Vec::push`) | 6.278 ms | 1.43× | 1.00× |
| go (`append`, GC) | 12.677 ms | 2.88× | 2.02× |

**Many short-lived buffers** — 1e6 iterations, each allocating a small vector:

| backend | min | vs C | vs Rust |
|---|---:|---:|---:|
| c (`malloc`/`free` per iteration) | 1.779 ms | 1.00× | — |
| **ax (region arena, no frees)** | **10.277 ms** | 5.78× | **0.62×** |
| rust (`Vec` allocated and dropped) | 16.608 ms | 9.34× | 1.00× |
| go (slice per iteration, GC) | 21.400 ms | 12.03× | 1.29× |

Ax is 1.6× faster than Rust here because it never frees. C wins outright because
`malloc`/`free` of the same 16-byte block is nearly free — the block stays hot and
there is no capacity logic at all. Against a growable container abstraction the
arena wins; against a raw fixed-size `malloc` it does not, and comparing those two
would be comparing different abstractions.

## Startup: no difference to claim

| backend | min |
|---|---:|
| c | 1.532 ms |
| go | 1.672 ms |
| ax | 1.828 ms |
| rust | 2.552 ms |

An earlier version of this file reported 0.68× Rust here and treated it as a win.
Re-measuring with 51 samples, and again with an independent harness, showed the
ordering changes between runs and that nearly all of the time is process creation.
The row is kept because the *absence* of a difference is worth recording.

## IO (64 MiB bytesum, identical checksum)

| setup | min | vs C mmap |
|---|---:|---:|
| **ax (`io.bytesum_file`, mmap in place)** | **9.399 ms** | **0.91×** |
| c (mmap + same mix) | 10.280 ms | 1.00× |
| rust (mmap + same mix) | 10.396 ms | 1.01× |
| rust (`read_to_end`, idiomatic) | 15.282 ms | 1.49× |

Against idiomatic Rust this is **0.62×**. The credit belongs to `axrt` avoiding a
copy, not to code generation — a Rust program that mmaps gets within 11% of it.
The point is that the capability-typed call *is* the mmap path, so the idiomatic
Ax program gets it without the author choosing an unsafe API.

## Compile and startup

| metric | time | note |
|---|---:|---|
| `ax check` (in-process) | 95 µs | parse + type/effect/region check |
| **`ax jit`: Cranelift compile *and run*** | **296 µs** | in-process, no `cc`, no object file |
| `cc -O3 -flto` | 67 ms | C only, compile alone |
| `ax build --tier release` | 197 ms | emit C + compile the program **and** `axrt` |
| `rustc -O -lto=thin` | 1.10 s | ~16× `cc` |

The second row is the one that changed the shape of the agent loop. Before it, an
agent could reject a candidate for 96 µs but had to spend ~200 ms to *see what it
did*; a wrong answer is not a type error, so behaviour has to be observed.
Cranelift closes that: compiling and running the primes kernel costs 0.3 ms,
228× less than `cc` takes to compile it and 665× less than the full native build.
The C tier stays for release, where 200 ms buys `-O3 -flto`.

`ax check` being ~10⁴× faster than `rustc` is the number that matters for the
agent loop: it is what makes verifying fifty candidate hole fills cheaper than
compiling once (see `ax-dev eval-loop`).

## Interpreter

The oracle is 10²–10³× slower than native (~280× per iteration on LCG mix, after
scaling for the smaller `n` it is run at and subtracting spawn). It is the normative
semantics, not a runtime. `ax run` is for checking behaviour; `ax build` is for
running.

## Token cost

The goal this serves is that a model pays less. Two different questions hide
behind "low token usage", and only one of them favours Ax.

**Tokens per source file** (`ax-dev bench tokens`, six kernels, proxy tokenizer
applied identically to every language, verified same-program):

| language | tokens | vs ax-terse |
|---|---:|---:|
| **ax-terse** | **532** | 1.00× |
| go | 632 | 1.19× |
| ax (conventional) | 633 | 1.19× |
| rust | 685 | 1.29× |
| c | 766 | 1.44× |

The terse surface is the most token-compact of the five, at 0.84× Go and 0.78×
Rust. It got there by *removing* syntax rather than shortening it:

- the `module` / `export` header is omitted and reconstructed (the module name
  comes from the file stem) — about twelve tokens per file;
- `:` and `->` are dropped, and `!{a, b}` contracts to `!a+b`;
- and the effect rows themselves got shorter once `%` stopped forcing
  `err[DivError]` into signatures where the divisor is provably non-zero, and
  once an omitted `!{…}` started reconstructing `diverge` from `while`/`loop`.
  An explicit empty row is still a termination claim and is still checked.

Note what did *not* help: renaming keywords. Under any subword tokenizer `fn` and
`f` are both a single token, so an "unreadable" abbreviation buys bytes and
nothing else. Go is close on bytes while clearly behind on tokens, which is a
reminder that the two are not the same metric.

An earlier version of this file reported Ax at 508 tokens against Go's 425 and
concluded that terseness was not a feature this language delivers. That was
accurate for the surface as it stood; the header rule and the effect-row precision
are what changed it.

**Tokens to a working program**, which is what an agent is actually billed for:
every candidate written out plus every byte of compiler output read back. Measured
across five seeds (`ax-dev eval-loop --seed S --n 6` for S in 1, 7, 13, 42, 99),
because a single seed is misleading here:

| seed | ax attempts | rust attempts | ax tokens | rust tokens | ax wall | rust wall |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1.0 | 3.0 | 66 | 226 | 1.3 ms | 805 ms |
| 7 | 2.5 | 3.0 | 72 | 75 | 1.0 ms | 685 ms |
| 13 | 2.5 | 3.0 | 64 | 75 | 1.0 ms | 870 ms |
| 42 | 1.0 | 3.0 | 66 | 569 | 1.4 ms | 838 ms |
| 99 | 1.0 | 3.0 | 66 | 421 | 1.4 ms | 904 ms |

Three claims of very different strength, which is why they are separated:

- **Wall time to green: ~600× less, and stable** (1.0–1.4 ms against 685–904 ms).
  This is the solid result. It follows from `ax check` costing 94 µs, so the
  protocol can reject a candidate without building anything.
- **Attempts: consistently fewer or equal** (1.0–2.5 against 3.0 every time).
- **Tokens: always fewer, but the margin swings from 1.04× to 8.6×.** Ax's number
  barely moves (64–72) while `rustc`'s ranges over 75–569, entirely depending on
  whether a rejected candidate produced a compile error — with spans, notes, and a
  help paragraph — or merely a wrong answer printed silently. Quoting the 8.6×
  case as the headline would be cherry-picking; the honest summary is
  "fewer, sometimes far fewer, and never more".

Both arms draw from the same candidate pool in the same order; the only
difference is that the ax arm may ask which candidates typecheck before running
one. No language model is involved in either arm — this measures the protocol,
not a model, and should not be quoted as an LLM benchmark.

## Reproducing

```sh
cargo run -p ax-dev -- bench metrics   # writes target/bench/RESULTS.md
cargo run -p ax-dev -- bench io        # IO only
cargo run -p ax-dev -- bench http      # keep-alive HTTP GET
cargo run -p ax-dev -- eval-loop --n 8 # agent loop, both arms
```

Go and Rust columns are omitted rather than estimated when the toolchain is
absent.
