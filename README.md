# Ax

A compiled systems language whose primary consumer is a program, not a person.
You write `#add(a,b)=a+b`. A file that opens with `(` is the
same language as a prefix tree. The bet under that is a compiler which
answers questions in microseconds, classifies its own fixes by safety,
and ships a normative interpreter to check itself against.

```
.ax → Ax (`#fn`, `:=`, `$if`, `+/`)  |  tree if the file opens with `(`
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
#add(a,b)=a+b
#sum(n:Z)=+/n
#parse(s S)=parse_i32(s)|0
```

That is Ax. Language reference: `spec/dense.md`. Card: `ax card` /
`spec/card.md`. A file that opens with `(` is the prefix tree
(`spec/tree.md`).

- Effects in the signature: `!err[E]`, `!io[c]`, `!alloc[a]` (`!a`), `diverge`, `abort`.
- Errors: `raise` / `catch` / `attempt` plus declared single-step `from`
  injections. No unwinding: a raise is a branch on a returned tag. `e|d`
  is ok-or.
- Regions: a named bump arena is an `Alloc` you can hand to `[]`.
  Store of a region ref is legal iff the region outlives the location.
- Dictionaries: `dict` with `default` unique resolution, resolved
  statically and called directly — no vtable.
- `as` is the only numeric conversion; there are no implicit ones.

Side-by-side TypeScript / Python / C / Rust / Ax: `docs/usecases.md`
(`cargo run -p ax-density -- --write`, development workspace only).
The corpus grammar (`spec/grammar.ebnf`) is not what you write.

**v0.3:** `e?` is result propagation; `f"…"` interpolates; `own T` is
affine; `Untrusted[T]` / `Secret[T]` are lattice annotations;
`ax perf --json` reports the ownership ladder. `par` is rejected until
disjointness is proven. Capturing closures remain out of v1.

## API servers

Ax handlers are ordinary typed functions. `http.serve_handler` runs them on a
non-blocking kqueue/epoll reactor with keep-alive and pipelining:

```ax
fn handle(request: http.Request) -> http.Response =
    if request.path == "/health" {
        http.response(200u16, "{\"ok\":true}")
    } else {
        http.response(404u16, "{\"error\":\"not_found\"}")
    };

fn main() -> unit !{io[net], abort} = http.serve_handler(8080u16, handle);
```

Build with `ax build --tier release examples/api_server.ax`. Literal response
bodies are serialized once; dynamic bodies remain request-local. The
reproducible Rust/Go/Python/Node comparison is in `bench/http/`.

### HTTP performance

Ax reached **143,273 requests/second at 256 concurrent connections** in the
included routed JSON benchmark, finishing ahead of the equivalent Go, Rust,
Node.js, and Python implementations. The fast path combines a kqueue/epoll
reactor, compiled typed handlers, reusable connection buffers, and
compiler-proven static response caching, with no per-request allocation for
literal responses.

See [HTTP performance](docs/http-performance.md) for the complete results,
methodology, architectural explanation, reproduction command, and precise
scope of the performance claim.

### Build a REST API quickly

The standalone **Ax API** framework adds FastAPI-style verb routes and
Rails-style resource paths without adding framework policy to the language:

```ax
// ax-api port 8080
// ax-api GET /v1/items/{id} -> show

fn show(request: http.Request, id: String) -> http.Response = api.ok(id);
```

Run it with `ax-api run app.ax`. The separate `frameworks/ax-api` package
generates an ordinary `http.serve_handler` program; the compiler and core
runtime contain no REST router or `api.*` framework builtins.

See the [Ax API quick start](docs/api-framework.md) for REST routing, request
fields, JSON responses, automatic errors, performance, and the complete MVP
example.

### Database-backed APIs

The standalone **Ax DB** component provides SQLite connections, bound
parameters, typed record decoding, explicit transactions, statement timeouts,
and idempotent migrations. Ax API can open one application database and pass it
explicitly to handlers with `// ax-api database app.sqlite`. The core language
only owns the opaque-resource and stateful-handler ABI; SQL and migration policy
stay in `packages/ax-db`.

See [Ax DB](packages/ax-db/README.md) and the executable
[`examples/db_sqlite.ax`](examples/db_sqlite.ax).

## Protocol

```
ax check [--json] [--allow-holes] [--strict-det] [--surface ax|tree]
ax hole [--fills] [--json]      ranked fills, each verified by compiling it
ax fix [--apply]                applies only semantics_preserving fixes
ax test                         run language-level tests
ax run --seed N --trace f       records a transcript (oracle)
ax jit <file> [args]            Cranelift: compile and run, no cc, ~0.3 ms
ax replay f source.ax           replays from the transcript, performing no IO
ax ir | types | effs | search | errs --into | fmt | patch --tx | deps --affected
ax build [-o bin] [--tier dev|release|portable]
ax merge --semantic | label | card | pkg list | pkg write
ax perf [--json] [--diff baseline.json] | complete | context | repair
ax caps | gbnf | daemon
```

Repository validation and experiments are deliberately separate from the
shipped compiler. They run through the non-publishable `ax-dev` workspace tool:

```sh
cargo run -p ax-dev -- conform [filter]
cargo run -p ax-dev -- attempts-to-green [--json] <file>
cargo run -p ax-dev -- bench metrics|tokens|software|gate|gate-check|all
cargo run -p ax-dev -- k1|silent-wrongness|eval-loop|kill-criteria
cargo run -p ax-dev -- translate|harvest|testharness|gbnf-check
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
$ cargo run -p ax-dev -- attempts-to-green distance.ax
green  distance.ax  holes filled 1  attempts 3  probes 2  2.5 ms
  applied: math.hypot(v.x, v.y)
```

## Performance

Same algorithm, identical output — the harness refuses to report a time unless
every backend printed the same value. **Fastest of 21 runs** (raised from 9 after a
single outlier in Rust's `gcd` time moved that row's verdict by 8% between runs).
`cargo run -p ax-dev -- bench metrics`.

Where Ax wins, a language guarantee is doing the work. Where the work is a plain
loop, Ax lands on clang's output and clang's result is what you get.

The broader [22-case software suite](docs/software_usecases.md) currently records
14 Ax wins, 7 parity results, 0 scored losses, and 1 excluded scheduler-noise
row. The enforced full-size §5.6 gate reports median Ax/C **0.93×**, worst Ax/C
**1.19×**, and median Ax/Rust **0.84×**.

| workload | ax | rust | go | ax/rust | why |
|---|---:|---:|---:|---:|---|
| `fib(40)` from argv | **1.88 ms** | 166.9 ms | 226.8 ms | **0.011×** | the row proves `fib` pure, so results are cached |
| `C(30,14)` two-arg recursion | **1.99 ms** | 177.3 ms | 264.2 ms | **0.011×** | same cache, now for two integer arguments |
| `fib(40)`, literal argument | **1.55 ms** | 173.0 ms | — | **0.009×** | same proof, applied during the build |
| invariant-divisor `%` 8e7 | **37.1 ms** | 43.7 ms | 43.7 ms | **0.85×** | reciprocal hoisted; rustc/gc leave a `udiv` |
| 1e6 short-lived buffers | **10.54 ms** | 21.28 ms | 22.97 ms | **0.50×** | region arena: no per-object free |
| IO 64 MiB bytesum | **11.49 ms** | 16.92 ms | — | **0.68×** | `axrt` mmaps in place; idiomatic Rust copies |
| 2e6 records pushed + summed | **6.13 ms** | 7.46 ms | 14.36 ms | **0.82×** | arena grows in place, `push` inlined, checks dropped |
| Euclid gcd 3e6 | 115.5 ms | 114.6 ms | 114.6 ms | 1.01× | parity |
| FNV nest 6000² | **37.1 ms** | 37.9 ms | 47.2 ms | 0.98× | parity with Rust; faster than Go |
| LCG mix 2e8 | 205.6 ms | 201.3 ms | 205.5 ms | 1.02× | **parity is provably optimal** |
| primes < 6e5 | 24.9 ms | 24.6 ms | **21.7 ms** | 1.01× | **within 1% of Rust; a 15% loss to Go** |
| check a module | **176 µs** | 1.18 s | — | ~10⁻⁴× | the checker is what an agent calls in a loop |
| compile **and run** a module | **417 µs** | 1.18 s (compile only) | — | ~10⁻⁴× | Cranelift, in-process; `cc` alone is 74 ms |

**Stated plainly:** Ax is far faster than Rust where its type system licenses
something Rust's cannot express (proven purity, region allocation, capability IO,
loop-invariant division), at parity on plain compute, and 15% *slower than Go* on
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

**On the LCG row, parity is the ceiling for everyone.** The `ax-dev bench
metrics` development command runs a
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

The development-only `ax-density` workspace tool counts exact source with the
real `o200k_base` and `cl100k_base` BPE vocabularies. It compares compact
implementations of nine common methods in the five requested languages; every
generated Ax snippet is then compiled. Its tokenizer dependency is not part of
the shipped `ax` package or default workspace build.

| encoding | TypeScript | Python | C | Rust | **Ax** | Ax vs best mainstream |
|---|---:|---:|---:|---:|---:|---:|
| `o200k_base` | 185 | 133 | 237 | 226 | **107** | **20% fewer** |
| `cl100k_base` | 180 | 132 | 236 | 219 | **105** | **20% fewer** |

Ax is the smallest overall corpus in both vocabularies and wins or ties all
nine individual cases. A controlled corpus is evidence, not proof for every
possible program.

The measured changes are structural: omitted `i32` signatures, nullary `#f=...`
declarations, default map type `M`, `name:={...}` map binding, inferred allocation effects, bare map keys,
implicit zero-default lookup, map-bound bare keys, marker-free interpolation, the one-token `??`
conditional, the compact `+/a*b` dot-product reduction, and a formatter that removes BPE-costing whitespace and
terminators. Shortening `fn` to `f` would not help—both are already one token.

**Cost to reach a working program.** An earlier version of this section compared
Ax-with-its-protocol against bare `rustc` and reported ~600× less wall time,
fewer cycles, and up to 8.6× fewer tokens. That was the wrong control, and not
by a little: it varied the language and the protocol at the same time, so it
could not tell which one produced the result. Since the whole question is
whether the language is needed or only the protocol, that comparison answered
nothing. `ax-dev k1` runs all four cells; medians over four seeds, n=12:

| cell | median attempts | median wall | median tokens |
|---|---:|---:|---:|
| ax + protocol | 1.0 | **0.6–1.0 ms** | 64–77 |
| ax − protocol | 3.0 | 872–1099 ms | 51–63 |
| rust + protocol | **1.0** | 361–612 ms | 79–204 |
| rust − protocol | 3.0 | 673–877 ms | 75–224 |

The `rust + protocol` arm is the control that was missing. It verifies each
candidate with `rustc --emit=metadata` — a full type-check with no codegen and
no link — then ranks in-scope bindings ahead of literals, which is what
rust-analyzer's expected-type completion does for free.

**What survived and what did not:**

- **The attempt-count advantage was the protocol, not the language.** Rust with
  a protocol matches Ax at 1.0 attempts, and on one seed beats it (2.0 vs 2.5).
- **The latency advantage is real and is ~400×, not ~600×.** 0.6–1.0 ms vs
  ~390 ms. It is a property of in-process checking and a Cranelift tier that
  needs no `cc`, and no wrapper over `rustc` reaches it: the floor is one
  process launch per probe, and `--emit=metadata` alone is 37 ms.
- **The token margin is ~1.0×–2.7×, not 1.04×–8.6×.** Two accounting bugs
  inflated the old number, both of which had favoured Ax: ax probes were billed
  as free, and the tooled Rust arm was not billed for its build attempts.
- **Strip the protocol and Ax is 1.04×–1.32× *slower* than Rust** over nine
  seeds, because `build_tier` recompiles the runtime every time.

So this section is evidence for the protocol, and — read honestly — evidence
*against* the language on this axis. The language's own case is measured
separately, by `ax-dev silent-wrongness`:

| | ax | rust |
|---|---:|---:|
| silent wrongness (accepted, ran, wrong, nothing said) | **36%** | 64% |
| tier-divergent (one source, different answers across its own tiers) | **0%** | 18% |
| has a mechanism for the hazard class at all | **100%** | 55% |

Nothing in that table depends on `ax check` being fast, on holes, on ranked
fills, or on structured diagnostics — so it is the part a protocol over Rust
cannot copy. It also includes a row where Rust is better (`overflow-literal`:
the `arithmetic_overflow` lint rejects what Ax accepts and wraps) and two rows
of parity, and a test fails the build if those rows are ever removed.
`DECISIONS.md` records both experiments, including that K1's numeric condition
now **fires**.

## Tests

```sh
cargo test --workspace                  # core plus development suites
cargo run -p ax-dev -- conform          # conformance corpus, every tier
cargo run -p ax-dev -- testharness      # Test Spec v1.0 tree under tests/
```

The conformance corpus (`conformance/`) is 134 cases ported by scenario from Go's
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
cargo run -p ax-dev -- k1 --seed 42 --n 12
cargo run -p ax-dev -- silent-wrongness
cargo run -p ax-dev -- eval-loop --seed 42 --n 8
```

`ax-dev k1` is the four-cell version and the one to quote: `eval-loop`'s two arms
vary the language and the protocol together, so its ratio cannot be attributed
to either. `ax-dev silent-wrongness` is the axis neither can reach, because every
`eval-loop` task is a hole-fill task and no hole-fill task exercises semantics.

No model is involved in any of the three. They measure the protocol and the
language, not an LLM.

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
spec/dense.md         Ax (the language; default parse + fmt)
spec/tree.md          prefix tree (file opens with `(`)
spec/grammar.ebnf     corpus dialect (Rust-shaped; not what agents write)
spec/card.md          ≤ 3,000-token agent card
std/tree/lib.ax       self-hosted tree lexer + printer
spec/metrics.md       compute-kernel methodology and results
docs/software_usecases.md  common software shapes vs C / Rust / Go
crates/ax/            checker, typed IR, C backend, oracle, CLI
runtime/axrt.c        IO / HTTP core (mmap, keep-alive pool)
runtime/axlang.c      language ABI: aborts, arenas, exact semantics, stdlib
conformance/          the corpus, one case per file
tests/                Test Spec v1.0 tree (headers, oracles, inverted, authored)
examples/             snippets CI compiles and runs
```

## Design stance

Ax has zero training data. The old stance was "look like Rust so models can
write it"; that made the language a coat. The tree spends novelty on a
surface a program can sample without silent grouping errors. The protocol,
exact effect-aware interfaces, typed holes, and honest capability labels
are still the rest of the bet. The compiler is still a Rust program —
that is an implementation choice, not a dialect.

`net` / `tls` / `crypto` / `regex` / `time` are reserved versioned components
(`ax pkg list`) and are **not** compiled into the toolchain.
