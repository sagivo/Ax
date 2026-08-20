# Ax card (v0.3 / research-v1)

**This is Ax.** `#name`, `:=`, `c??t:e`, `i~n`, `+/n`, type glyphs.
Default parse and `ax fmt`. A file that opens with `(` is the prefix tree.
Rust-shaped `fn` / `module` / `let` still parses so the corpus keeps proving
the IR. An agent does not write it.

`own T` is affine. `Untrusted[T]` / `Secret[T]` cannot reach sinks / logs.
`ax perf --json` is the second diagnostic loop.

Compiled systems language for LLM agents. Extension `.ax`. Text is
authoritative. Primary consumer: a program. Humans are second.

## Axioms

Minimize attempts-to-green. One spelling per construct. Interfaces expose all
language-level observable behavior. Pure core is deterministic; effectful runs
are transcript-replayable. Every trust boundary is explicit.

## File

```
#add(a,b)=a+b
#sum(n:Z)=+/n
#sumv(xs V[Z])Z=+/xs#
#dot(a,b:V[W])W=+/a*b
#maxv(xs V[Z])Z=|/xs#
#pick(b B)=b??1:0
#get(m M[S,L],k S)L=m[k]?0
#f={m:={e:2,o:3};m[e]?+m[o]?}
#main()Z={s Z:=1;i~n{s=s*6364136223846793005+i};s}
```

`#name(a T)R=body` is a function. Omitted signature types are `I`;
`a,b:T` shares `T` across parameters and result. `name T:=e` binds.
`i~n` loops from 0 to `n`. `c??t:e` is if/else. `@c{body}` is while. `^e`
returns. `e?d` is option-or. `e|d` is result-or.

Type atoms: `I` i32 · `L` i64 · `Y` isz · `U` u32 · `W` u64 · `Z` usz ·
`B` bool · `F` f64 · `f` f32 · `S` string · `O` option · `R` result ·
`M` map · `V` vec.

`7L` is 7 as `L`. `s += e` / `s++` assign. `xs#` is length. `xs[i]`
is the element (check drops inside `i~xs#`). `{"k":2L}` is an inferred
homogeneous map literal. `+/n` / `*/n` sum / product of a range as `Z`.
`+/a*b` is a `W`-vec dot product. `+/xs#` / `*/xs#` / `|/xs#` / `&/xs#`
sum / product / max / min of a `Z`-vec. `[]` empty vec. `m:={}` binds an
empty default map. `M` is the default string-to-`I` map shape. `xs<-e` append.
`xs[i]<-v` store. `m[k]<-v` insert. `m[k]?d` gets with fallback `d`;
bare `m[k]?` defaults to zero. Simple literal keys may omit quotes.

A file that opens with `(` is the tree:
`(fn add ((a i32) (b i32)) i32 (+ a b))`.

## Types

Write `I L Y U W Z B F f S O R M V`. Spelled `i32` / `usz` / `Vec` are the
same types in the corpus dialect. `own T` is affine (use exactly once).
No implicit numeric conversion; `as` is the only one.

## Ops

`+ - * / % == != < <= > >= && || & | ^ << >>`. Wrap; shift counts mask to
width. `/` and `%` raise `err[DivError]` only when the divisor may be zero.
`xs[i]` aborts out of range unless proven.

## Control

`c??t:e`  `$c{t}`  `match s { p => e; … }`  `i~n { … }`  `i~xs# { … }`
`@c{body}`  `loop` `break` `continue` `^e`  `s += e`  `s++`.
`i~n` is bounded. `@` / `loop` add `diverge`.

## Effects

Inferred in body, checked against declaration. Omitted row reconstructs
`diverge`. Explicit empty row = effect-free, including termination.
`--strict-det` rejects `io`/`race`/`nondet`, not `diverge`.
`!a` is `!alloc[a]`.

## Errors

`raise` / `catch` / `attempt` → result. At most one `err[E]`.
Injections declared once, single-step, unambiguous. `e|d` is ok/err.
`e?` is result propagation.

## Memory

Store of a region ref is legal iff the region outlives the location.
Lexical regions. Exclusive mut. No reborrow, no interior mut.
A region is a bump arena and its name is an `Alloc`.

## Trust

`Untrusted[T]` and `Secret[T]` are lattice annotations (same layout as `T`).
IO produces `Untrusted`. Sinks reject it. `Secret` cannot be logged,
formatted, serialized, or sent over FFI.

## Contracts

Literals, params, `ret`, fields, option/variant tests, cmp/bool,
wrapping arith, `#` / `m[k]`. No loops/alloc/io/errors.

## Concurrency

`par` is not implemented in v1 and is rejected by the native backend.

## Caps / FFI

Capability handles are required for `fs`; `io.*`/`http.*` are ambient
and labelled as such. Strict mode forbids raw FFI.

## Protocol

```
ax check [--json] [--allow-holes] [--strict-det] [--surface ax|tree]
ax hole [--fills] [--json]
ax fix [--apply]
ax test
ax run --seed N --trace f
ax jit <file> [args]
ax replay f source.ax
ax ir | types | effs | search | errs --into | fmt | patch --tx | deps --affected
ax build [-o bin] [--tier dev|release|portable]
ax merge --semantic | label | card | pkg list | pkg write
ax perf [--json] [--diff baseline.json] | complete | context | repair
ax caps | gbnf | daemon
```

Fixes: only `semantics_preserving` auto-applied.
`ax hole --fills` synthesises candidates, then verifies each by substituting
it and running the checker. What comes back is known to compile.

## HTTP

Handlers are ordinary typed functions.

```
fn handle(request: http.Request) -> http.Response =
    if request.path == "/health" {
        http.response(200u16, "{\"ok\":true}")
    } else {
        http.response(404u16, "{\"error\":\"not_found\"}")
    };

fn main() -> unit !{io[net], abort} = http.serve_handler(8080u16, handle);
```

Build: `ax build --tier release app.ax`.

## Ax API (standalone)

Not in the compiler. `ax-api` expands comment directives to
`http.serve_handler`. Do not define `fn main`.

```
// ax-api port 8080
// ax-api GET /v1/items/{id} -> show
fn show(request: http.Request, id: String) -> http.Response = api.ok(id);
```

```
ax-api new my-api
ax-api run app.ax
ax-api build [-o bin] [--tier dev|release] app.ax
ax-api expand app.ax
```

Methods: GET POST PUT PATCH DELETE.
Path `{name}` = one segment. `*name` = suffix, must be last.
Helpers: `api.ok` 200, `api.created` 201, `api.no_content` 204,
`api.bad_request` 400, `api.not_found` 404, `api.json(status, body)`.
Generated: `GET /openapi.json`, `GET /docs`. Do not claim `/openapi.json`.

## Dicts

`= default` resolves to the unique visible `dict D[T]`. Zero or two is
an error.

## Measured (quote with scope)

- check a module: 176 µs (rustc 1.18 s)
- compile and run a module: 417 µs (Cranelift; `cc` alone is 74 ms)
- ax + protocol median wall: 0.6–1.0 ms vs rust + protocol 361–612 ms
- token corpus o200k_base: Ax 107 / Python 133 / TS 185 / Rust 226 / C 237
- silent wrongness: Ax 36% / Rust 64%. tier-divergent: Ax 0% / Rust 18%
- HTTP routed JSON, wrk 4×256×10s, Darwin arm64: Ax 143273 r/s p99 2.34 ms
- fib(40) from argv: Ax 1.88 ms / Rust 166.9 ms (purity cache)
- 1e6 short-lived buffers: Ax 10.54 ms / Rust 21.28 ms (region arena)

Where the work is a plain loop, Ax lands on clang. Parity on LCG/gcd is
expected. Go wins `primes` by ~15% (scheduler, same instruction class).
HTTP lead over Go/Rust is ~2% on the reference machine; treat as same band
under thermal noise. "Faster than Rust on identical machine-level work" is
only claimed where a language guarantee does the work (purity, regions,
capability IO, invariant division) or on the agent loop (check / jit).
