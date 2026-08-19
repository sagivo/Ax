# Ax card (v0.3 / research-v1)

**This is Ax.** `#name`, `:=`, `c??t:e`, `i~n`, `+/n`, type glyphs.
Default parse and `ax fmt`. See `spec/dense.md`.

A file that opens with `(` is the prefix tree (`spec/tree.md`).
Rust-shaped `fn` / `module` / `let` still parses so the corpus keeps
proving the IR. An agent does not write it.

`own T` is affine. `Untrusted[T]`/`Secret[T]` cannot reach sinks / logs.
`ax perf --json` is the second diagnostic loop.

Compiled systems language for LLM agents. Extension `.ax`. Text is
authoritative.

Side-by-side TypeScript / Python / C / Rust / Ax: `docs/usecases.md`
(`cargo run -p ax-density -- --write`, development workspace only).

## Axioms
Minimize attempts-to-green. One spelling per construct. Interfaces
expose all language-level observable behavior. Pure core is
deterministic; effectful runs are transcript-replayable. Every trust
boundary is explicit.

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
homogeneous map literal. `+/n` / `*/n` sum /
product of a range as `Z`. `+/a*b` is a `W`-vec dot product. `+/xs#` /
`*/xs#` / `|/xs#` / `&/xs#`
sum / product / max / min of a `Z`-vec. `[]` empty vec. `m:={}` binds an
empty default map. `M` is the default string-to-`I` map shape. `xs<-e` append.
`xs[i]<-v` store. `m[k]<-v` insert. `m[k]?d` gets with fallback `d`;
bare `m[k]?` defaults to zero. Simple literal keys may omit quotes.

A file that opens with `(` is the tree: `(fn add ((a i32) (b i32)) i32 (+ a b))`.

## Types
Write `I L Y U W Z B F f S O R M V`. Spelled `i32` / `usz` / `Vec`
are the same types in the corpus dialect. `own T` is affine (use
exactly once; A2020 / A2021). No implicit numeric conversion; `as`
is the only one.

## Ops
`+ - * / % == != < <= > >= && || & | ^ << >>`. Wrap; shift counts
mask to width. `/` and `%` raise `err[DivError]` only when the
divisor may be zero. `xs[i]` aborts out of range unless proven.

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

## Memory
Store of a region ref is legal iff the region outlives the location.
Lexical regions. Exclusive mut. No reborrow, no interior mut.
A region is a bump arena and its name is an `Alloc`.

## Contracts
Literals, params, `ret`, fields, option/variant tests, cmp/bool,
wrapping arith, `#` / `m[k]`. No loops/alloc/io/errors.

## Concurrency
`par` is not implemented in v1 and is rejected by the native backend.

## Caps / FFI
Capability handles are required for `fs`; `io.*`/`http.*` are ambient
and labelled as such. Strict mode forbids raw FFI.

## Protocol
`ax check [--json] [--allow-holes] [--strict-det]`
`ax hole` `ax types` `ax effs` `ax search` `ax errs --into T`
`ax fmt` `ax patch --tx` `ax deps --affected`
`ax test` `ax run --seed N --trace f` `ax replay f` `ax jit <file>`
`ax merge --semantic` `ax label` `ax card` `ax ir`
Repository-only conformance: `cargo run -p ax-dev -- conform`.
Fixes: only `semantics_preserving` auto-applied.

## Dicts
`= default` resolves to the unique visible `dict D[T]`. Zero or two is
an error.
