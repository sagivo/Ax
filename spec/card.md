# Ax card (v0.3 / research-v1)

**The language is the short syntax.** `#fn`, `:=`, `$if`, `i~n`, type
glyphs. That is the default parse and `ax fmt`. There is no opt-in
dense mode. See `spec/dense.md`.

A file that opens with `(` is the prefix tree (`spec/tree.md`).
Rust-shaped `fn` / `module` / `let` still parses so the corpus keeps
proving the IR. An agent does not write it.

`own T` is affine. `Untrusted[T]`/`Secret[T]` cannot reach sinks / logs.
`ax perf --json` is the second diagnostic loop.

Compiled systems language for LLM agents. Extension `.ax`. Text is
authoritative.

Side-by-side Rust / C / Go / Ax syntax + tokens: `docs/usecases.md`
(`ax bench usecases`).

## Axioms
Minimize attempts-to-green. One spelling per construct. Interfaces
expose all language-level observable behavior. Pure core is
deterministic; effectful runs are transcript-replayable. Every trust
boundary is explicit.

## File (what an agent writes)
```
#add(a I, b I) I = a + b
#sum(n Z) Z = +/n
#main() Z = { s Z:= 1; i~n { s = s * 6364136223846793005 + i }; s }
#pick(b B) I = $b{1}{0}
#get(m M[S, L], k S) L = m.get(k)?0}
```

`#name(a T) R = body` is a function. `name T:= e` is `let mut`.
`i~n` is `for i in range(0, n)`. `$c{t}{e}` is if/else. `e?d` is
Option unwrap-or. `e|d` is Result unwrap-or. `@c{body}` is while.
`^e` is return. `%` is `map.new(test.alloc)`. `7L` is `7i64`.
`s += e` is `s = s + e` (also `-=` `*=` `/=` `%=` `&=` `|=` `^=`).
`+/n` / `+/a..b` is the usz-sum of the range; `*/n` is the usz-product.
Both expand to the same `range` loop as writing it out.

Type atoms: `I` i32 · `L` i64 · `Y` isz · `U` u32 · `W` u64 · `Z` usz ·
`B` bool · `F` f64 · `f` f32 · `S` String · `O` Option · `R` Result ·
`M` Map · `V` Vec.

A file that opens with `(` is the tree. Example:

```
(fn add ((a i32) (b i32)) i32 (+ a b))
```

## Types
`i8 i16 i32 i64 isz u8 u16 u32 u64 usz f32 f64 bool byte unit`
`String` `(ref r str)` `(Vec T)` `(ref r (slice T))` `(Option T)`
`(Result T E)` `Ordering` `Alloc` `(ref r T)` `(ref r mut T)`
`(own T)` is affine (use exactly once; A2020 / A2021).
No implicit numeric conversion; `as` / `(as expr T)` is the only one.

## Ops
`+ - * / % == != < <= > >= && || & | ^ << >>`. Wrap; shift counts
mask to width. `/` and `%` raise `err[DivError]` only when the
divisor may be zero. `get` → Option. `at` always bounds-checks → abort.

## Control
`$c{t}{e}`  `match s { p => e; … }`  `i~n { … }`  `@c{body}`  `loop`
`break` `continue` `^e`  `s += e`  `+/n`  `*/n`. `for` over a
finite sequence is bounded; `while`/`loop` add `diverge`.

## Effects
Inferred in body, checked against declaration. Omitted row reconstructs
`diverge`. Explicit empty row = effect-free, including termination.
`--strict-det` rejects `io`/`race`/`nondet`, not `diverge`.

## Errors
`raise` / `catch` / `attempt` → `Result`. At most one `err[E]`.
Injections declared once, single-step, unambiguous. `e|d` is Ok/Err.

## Memory
Store of a region ref is legal iff the region outlives the location.
Lexical regions. Exclusive mut. No reborrow, no interior mut.
A region is a bump arena and its name is an `Alloc`.

## Contracts
Literals, params, `ret`, fields, Option/variant tests, cmp/bool,
wrapping arith, `len`/`get`. No loops/alloc/io/errors.

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
`ax merge --semantic` `ax label` `ax card` `ax ir` `ax conform`
Fixes: only `semantics_preserving` auto-applied.

## Dicts
`= default` resolves to the unique visible `dict D[T]`. Zero or two is
an error.
